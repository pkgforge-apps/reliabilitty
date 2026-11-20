//! Application state and the event loop, with a background PTY reader.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::time::{Duration, Instant, SystemTime};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use x11rb::connection::Connection as _;
use xkeysym::{Keysym, key};

use crate::cli::Cli;
use crate::config::{Config, Padding};
use crate::input::{Action, Bindings, Composer, Mods, encode_key, encode_kitty_key};
use crate::pty::Pty;
use crate::render::{Canvas, Renderer, pixel};
use crate::scale;
use crate::scrollbar;
use crate::signals::Signals;
use crate::theme::Theme;
use crate::vt::{CursorStyle, Damage, Parser, SelectionMode, Terminal};
use crate::x11::{UiEvent, X11};

fn window_pixels(config: &Config, cell_width: u16, cell_height: u16) -> (u16, u16) {
    let columns = if config.window.dimensions.columns == 0 {
        80
    } else {
        config.window.dimensions.columns
    };
    let lines = if config.window.dimensions.lines == 0 {
        24
    } else {
        config.window.dimensions.lines
    };
    let padding = config.window.padding;
    (
        columns
            .saturating_mul(cell_width)
            .saturating_add(padding.x.saturating_mul(2))
            .max(1),
        lines
            .saturating_mul(cell_height)
            .saturating_add(padding.y.saturating_mul(2))
            .max(1),
    )
}

struct Drag {
    last_x: i16,
    last_y: i16,
    last_row: usize,
    last_col: usize,
}

struct Click {
    at: Instant,
    x: i16,
    y: i16,
    count: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuItem {
    Copy,
    Paste,
    SelectAll,
    Search,
}

#[derive(Clone, Copy, Debug)]
struct ContextMenu {
    x: i16,
    y: i16,
    hover: Option<MenuItem>,
}

struct SearchUi {
    query: String,
}

struct App {
    config: Config,
    config_path: Option<PathBuf>,
    config_modified: Option<SystemTime>,
    x11: X11,
    pty: Pty,
    pty_rx: mpsc::Receiver<Vec<u8>>,
    pty_wake: OwnedFd,
    reader_stop: Arc<AtomicBool>,
    terminal: Terminal,
    parser: Parser,
    renderer: Renderer,
    font_database: Arc<fontdb::Database>,
    canvas: Canvas,
    bindings: Bindings,
    signals: Signals,
    composer: Composer,
    dpi: f32,
    font_size_override: Option<f32>,
    padding: Padding,
    cell_width: u16,
    cell_height: u16,
    focused: bool,
    blink_on: bool,
    blink_deadline: Instant,
    last_cursor: Option<(usize, usize)>,
    drag: Option<Drag>,
    scroll_drag: bool,
    last_click: Option<Click>,
    context_menu: Option<ContextMenu>,
    search: Option<SearchUi>,
    search_stale: bool,
    auto_scroll_at: Instant,
    next_config_check: Instant,
    force_reload: bool,
    bell_until: Option<Instant>,
    redraw_all: bool,
    next_frame: Instant,
    last_pty_input: Instant,
    smoke: bool,
    rendered_once: bool,
    quit: bool,
    hold: bool,
    child_exit: Option<crate::pty::ExitStatus>,
    child_exit_at: Option<Instant>,
    hold_notice: bool,
    pty_eof: bool,
}

pub fn run(cli: Cli) -> Result<(), String> {
    let loaded = Config::load(cli.config_file.as_deref())?;
    let config = loaded.config;
    let config_path = loaded.path;
    let config_modified = config_path
        .as_ref()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok());
    let theme = Theme::from_config(&config.colors)?;
    let dpi = if config.font.dpi > 0.0 {
        config.font.dpi
    } else if let Ok((connection, screen)) = x11rb::connect(None) {
        let root = connection.setup().roots[screen].root;
        scale::detect(&connection, root)
    } else {
        scale::BASE_DPI
    };
    // Show the window with its background before scanning fonts, which is
    // the slowest part of startup. Cell metrics are corrected once fonts load.
    let size_px = (config.font.size * dpi / 72.0).max(1.0);
    let mut cell_width = config
        .font
        .cell_width
        .unwrap_or_else(|| (size_px * 0.6).floor().max(1.0) as u16);
    let mut cell_height = config
        .font
        .cell_height
        .unwrap_or_else(|| size_px.round().max(1.0) as u16);
    let columns = if config.window.dimensions.columns == 0 {
        80
    } else {
        config.window.dimensions.columns
    };
    let lines = if config.window.dimensions.lines == 0 {
        24
    } else {
        config.window.dimensions.lines
    };
    let title = cli
        .title
        .clone()
        .unwrap_or_else(|| config.window.title.clone());
    let instance = cli
        .instance
        .clone()
        .unwrap_or_else(|| config.window.instance.clone());
    let class = cli
        .class
        .clone()
        .unwrap_or_else(|| config.window.class.clone());
    let mut x11 = X11::new(
        &config.window,
        &title,
        &instance,
        &class,
        config.window.dimensions,
        cell_width,
        cell_height,
        theme.background,
    )?;
    let mut canvas = Canvas::new(x11.width(), x11.height());
    canvas.argb = x11.is_argb();
    canvas.fill(pixel(&canvas, theme.background, x11.background_alpha()));
    let full = Damage {
        all: true,
        rows: Vec::new(),
        scroll: 0,
    };
    x11.present(&canvas, &full, cell_height, 0);

    let font_database = Arc::new({
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        database
    });
    let mut renderer = Renderer::new(&config.font, dpi, None, font_database.clone())?;
    renderer.set_cursor_options(config.cursor.thickness, config.cursor.unfocused_hollow);
    cell_width = renderer.cell_width();
    cell_height = renderer.cell_height();
    {
        let (width, height) = window_pixels(&config, cell_width, cell_height);
        if (x11.width(), x11.height()) != (width, height) {
            x11.resize_window(width, height)?;
            x11.set_size(width, height);
            canvas = Canvas::new(width, height);
            canvas.argb = x11.is_argb();
            canvas.fill(pixel(&canvas, theme.background, x11.background_alpha()));
        }
    }
    x11.set_size_hints(
        config.window.padding,
        cell_width,
        cell_height,
        scrollbar::width(cell_width),
    )?;

    let cursor_style = parse_cursor_style(&config.cursor.style);
    let mut terminal = Terminal::new(
        columns as usize,
        lines as usize,
        config.scrolling.history,
        theme,
        cursor_style,
        config.cursor.blink,
    );
    terminal.set_initial_title(&title);
    terminal.cell_width = cell_width;
    terminal.cell_height = cell_height;
    terminal.set_window_pixels(x11.width(), x11.height());

    let signals = Signals::install().map_err(|error| error.to_string())?;
    let pty = Pty::spawn(
        &cli.command,
        config.terminal.shell.as_ref(),
        cli.working_directory.as_deref(),
        &config.env,
        columns as usize,
        lines as usize,
        x11.width() as usize,
        x11.height() as usize,
        x11.window_id(),
    )
    .map_err(|error| format!("cannot start the shell: {error}"))?;
    let (pty_tx, pty_rx) = mpsc::sync_channel::<Vec<u8>>(256);
    let reader_fd = pty.dup_reader().map_err(|error| error.to_string())?;
    // SAFETY: eventfd returns a fresh descriptor or -1.
    let wake = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if wake < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: eventfd returned a valid, independently owned descriptor.
    let pty_wake = unsafe { OwnedFd::from_raw_fd(wake) };
    // SAFETY: dup returns a fresh descriptor on success.
    let thread_wake = unsafe { libc::dup(wake) };
    if thread_wake < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: dup returned a valid, independently owned descriptor.
    let thread_wake = unsafe { OwnedFd::from_raw_fd(thread_wake) };
    let reader_stop = Arc::new(AtomicBool::new(false));
    let stop = reader_stop.clone();
    std::thread::spawn(move || {
        let mut pfd = libc::pollfd {
            fd: reader_fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let mut buffer = vec![0u8; 65536];
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            // SAFETY: pfd and count are valid.
            let result = unsafe { libc::poll(&mut pfd, 1, 100) };
            if result < 0 {
                break;
            }
            if result == 0 {
                continue;
            }
            loop {
                // SAFETY: buffer length is valid.
                let count = unsafe {
                    libc::read(
                        reader_fd.as_raw_fd(),
                        buffer.as_mut_ptr() as *mut libc::c_void,
                        buffer.len(),
                    )
                };
                if count == 0 {
                    return;
                }
                if count < 0 {
                    break;
                }
                if pty_tx.send(buffer[..count as usize].to_vec()).is_err() {
                    return;
                }
                let token: u64 = 1;
                // SAFETY: thread_wake is a valid eventfd and token is 8 bytes.
                unsafe {
                    libc::write(
                        thread_wake.as_raw_fd(),
                        &token as *const u64 as *const libc::c_void,
                        8,
                    );
                }
            }
        }
    });
    let bindings = Bindings::new(&config.keyboard)?;
    let now = Instant::now();
    let mut app = App {
        config,
        config_path,
        config_modified,
        x11,
        pty,
        pty_rx,
        pty_wake,
        reader_stop,
        terminal,
        parser: Parser::new(),
        renderer,
        font_database,
        canvas,
        bindings,
        signals,
        composer: Composer::default(),
        dpi,
        font_size_override: None,
        padding: Padding { x: 2, y: 2 },
        cell_width,
        cell_height,
        focused: true,
        blink_on: true,
        blink_deadline: now + Duration::from_millis(600),
        last_cursor: None,
        drag: None,
        scroll_drag: false,
        last_click: None,
        context_menu: None,
        search: None,
        search_stale: false,
        auto_scroll_at: now,
        next_config_check: now + Duration::from_secs(2),
        force_reload: false,
        bell_until: None,
        redraw_all: true,
        next_frame: now,
        last_pty_input: now,
        smoke: cli.smoke,
        rendered_once: false,
        quit: false,
        hold: cli.hold,
        child_exit: None,
        child_exit_at: None,
        hold_notice: false,
        pty_eof: false,
    };
    app.padding = app.config.window.padding;
    app.resize_to_window();
    app.run()
}

impl App {
    fn run(&mut self) -> Result<(), String> {
        self.terminal.mark_all();
        loop {
            let now = Instant::now();
            self.signals.drain();
            if self.signals.take_quit() {
                self.quit = true;
            }
            if self.signals.take_hangup() {
                self.force_reload = true;
            }
            self.timers(now);
            let events = self.x11.poll()?;
            for event in events {
                self.handle_event(event, now)?;
            }
            let mut token = [0u8; 8];
            // SAFETY: pty_wake is a valid nonblocking eventfd; token is 8 bytes.
            while unsafe {
                libc::read(
                    self.pty_wake.as_raw_fd(),
                    token.as_mut_ptr() as *mut libc::c_void,
                    8,
                )
            } == 8
            {}
            // Parse everything already buffered before drawing, so one frame
            // shows the latest state instead of a half-parsed burst.
            if self.read_pty() {
                self.last_pty_input = now;
                self.search_stale = true;
            }
            self.terminal_notifications(now);
            self.flush_input();
            self.check_child(now);
            self.resize_request();
            if self.force_reload || now >= self.next_config_check {
                self.maybe_reload(now);
            }
            let idle = now.duration_since(self.last_pty_input) >= Duration::from_millis(10);
            if idle || now >= self.next_frame {
                self.cursor_damage();
                self.render(now);
                self.next_frame = now + Duration::from_millis(8);
            }
            if self.quit {
                self.reader_stop.store(true, Ordering::Relaxed);
                break;
            }
            self.wait(now)?;
        }
        self.pty.terminate();
        self.x11.shutdown();
        Ok(())
    }

    fn timers(&mut self, now: Instant) {
        if self.cursor_blinking() && now >= self.blink_deadline {
            self.blink_on = !self.blink_on;
            let interval = self.config.cursor.blink_interval.max(50);
            self.blink_deadline = now + Duration::from_millis(interval);
            self.terminal.mark_cursor_dirty();
        }
        if let Some(until) = self.bell_until {
            if now >= until {
                self.bell_until = None;
                self.terminal.mark_all();
            }
        }
        if self.drag.is_some() && now >= self.auto_scroll_at {
            self.autoscroll(now);
        }
    }

    fn cursor_blinking(&self) -> bool {
        self.focused
            && self.config.cursor.blink
            && self.terminal.cursor.blinking
            && self.terminal.cursor_visible()
    }

    fn handle_event(&mut self, event: UiEvent, now: Instant) -> Result<(), String> {
        match event {
            UiEvent::Key { keysym, mods, time } => self.handle_key(keysym, mods, time),
            UiEvent::Button {
                button,
                pressed,
                x,
                y,
                time,
                mods,
                state,
            } => self.handle_button(button, pressed, x, y, time, mods, state, now),
            UiEvent::Motion { x, y, mods, state } => self.handle_motion(x, y, mods, state, now),
            UiEvent::Resize { width, height } => {
                self.x11.set_size(width, height);
                self.resize_to_window();
            }
            UiEvent::Expose => {
                self.terminal.mark_all();
                self.redraw_all = true;
            }
            UiEvent::Focus(focused) => {
                self.focused = focused;
                self.blink_on = true;
                self.blink_deadline =
                    now + Duration::from_millis(self.config.cursor.blink_interval.max(50));
                self.terminal.focus_changed(focused);
                self.terminal.mark_cursor_dirty();
                if !focused {
                    self.composer.reset();
                    self.scroll_drag = false;
                    self.drag = None;
                    self.close_context_menu();
                    self.x11.ungrab_pointer();
                }
            }
            UiEvent::Close => self.quit = true,
            UiEvent::Paste(text) => {
                if self.search.is_some() {
                    self.search_push_str(&text);
                } else {
                    self.terminal.scroll_to_bottom();
                    self.terminal.paste(&text);
                }
            }
            UiEvent::Error(error) => eprintln!("rtty: {error}"),
        }
        Ok(())
    }

    fn handle_key(&mut self, keysym: u32, mods: Mods, time: u32) {
        if keysym == 0 {
            return;
        }
        if self.context_menu.is_some() {
            if is_modifier_key(keysym) {
                return;
            }
            if keysym == key::Escape {
                self.close_context_menu();
                return;
            }
            self.close_context_menu();
        }
        let bound = self.bindings.find(keysym, mods).map(|binding| {
            (
                binding.action().cloned(),
                binding.chars().map(str::to_string),
            )
        });
        if let Some((action, chars)) = bound {
            if let Some(chars) = chars {
                if self.search.is_some() {
                    self.search_push_str(&chars);
                } else {
                    self.terminal.scroll_to_bottom();
                    self.terminal.push_input(chars.as_bytes());
                }
            } else if let Some(action) = action {
                self.perform_action(action, time);
            }
            return;
        }
        if self.search.is_some() {
            self.handle_search_key(keysym, mods);
            return;
        }
        let bytes = if self.terminal.kitty_flags() & 1 != 0 {
            encode_kitty_key(keysym, mods, &mut self.composer)
        } else {
            encode_key(
                keysym,
                mods,
                self.terminal.app_cursor(),
                self.terminal.app_keypad(),
                &mut self.composer,
            )
        };
        if let Some(bytes) = bytes {
            self.terminal.scroll_to_bottom();
            self.terminal.push_input(&bytes);
        }
    }

    fn handle_search_key(&mut self, keysym: u32, mods: Mods) {
        if keysym == key::Escape {
            self.close_search();
            return;
        }
        if keysym == key::Return || keysym == key::KP_Enter {
            self.terminal.search_step(!mods.shift);
            self.redraw_all = true;
            return;
        }
        if keysym == key::BackSpace {
            if let Some(search) = &mut self.search {
                search.query.pop();
            }
            self.apply_search_query();
            return;
        }
        if mods.control || mods.alt || mods.super_key {
            return;
        }
        if let Some(ch) = Keysym::new(keysym).key_char()
            && !ch.is_control()
        {
            self.search_push(ch);
        }
    }

    fn search_push(&mut self, ch: char) {
        if let Some(search) = &mut self.search {
            search.query.push(ch);
        }
        self.apply_search_query();
    }

    fn search_push_str(&mut self, text: &str) {
        if let Some(search) = &mut self.search {
            for ch in text.chars() {
                let ch = match ch {
                    '\r' | '\n' | '\t' => ' ',
                    other => other,
                };
                if !ch.is_control() {
                    search.query.push(ch);
                }
            }
        }
        self.apply_search_query();
    }

    fn apply_search_query(&mut self) {
        let Some(query) = self.search.as_ref().map(|search| search.query.as_str()) else {
            return;
        };
        self.terminal.set_search_query(query);
        self.search_stale = false;
        self.redraw_all = true;
    }

    fn refresh_active_search(&mut self) {
        let Some(query) = self.search.as_ref().map(|search| search.query.as_str()) else {
            return;
        };
        self.terminal.refresh_search(query);
    }

    fn open_search(&mut self) {
        if self.search.is_none() {
            self.search = Some(SearchUi {
                query: String::new(),
            });
            self.terminal.set_search_query("");
            self.search_stale = false;
            self.redraw_all = true;
        }
    }

    fn close_search(&mut self) {
        if self.search.take().is_some() {
            self.terminal.clear_search();
            self.search_stale = false;
            self.redraw_all = true;
        }
    }

    fn toggle_search(&mut self) {
        if self.search.is_some() {
            self.close_search();
        } else {
            self.open_search();
        }
    }

    fn perform_action(&mut self, action: Action, time: u32) {
        match action {
            Action::Copy => self.copy_selection(true),
            Action::Paste => {
                let time = time.max(1);
                if let Err(error) = self.x11.request_paste(false, time) {
                    eprintln!("rtty: paste: {error}");
                }
            }
            Action::PasteSelection => {
                let time = time.max(1);
                if let Err(error) = self.x11.request_paste(true, time) {
                    eprintln!("rtty: paste: {error}");
                }
            }
            Action::ScrollPageUp => self.terminal.scroll_page(true),
            Action::ScrollPageDown => self.terminal.scroll_page(false),
            Action::ScrollLineUp => self.terminal.scroll(-1),
            Action::ScrollLineDown => self.terminal.scroll(1),
            Action::ScrollToTop => self.terminal.scroll_to_top(),
            Action::ScrollToBottom => self.terminal.scroll_to_bottom(),
            Action::IncreaseFontSize => self.change_font_size(1.0),
            Action::DecreaseFontSize => self.change_font_size(-1.0),
            Action::ResetFontSize => {
                self.font_size_override = None;
                if let Err(error) = self.rebuild_renderer() {
                    eprintln!("rtty: {error}");
                }
            }
            Action::ClearSelection => self.terminal.clear_selection(),
            Action::SelectAll => self.terminal.select_all(),
            Action::Search => self.toggle_search(),
            Action::ToggleFullscreen => {
                if let Err(error) = self.x11.toggle_fullscreen() {
                    eprintln!("rtty: fullscreen: {error}");
                }
            }
            Action::SpawnNewInstance => self.spawn_instance(),
            Action::Quit => self.quit = true,
            Action::None => {}
        }
    }

    fn spawn_instance(&self) {
        let result = std::env::current_exe().and_then(|exe| {
            let mut command = Command::new(exe);
            if let Ok(directory) = std::env::current_dir() {
                command.current_dir(directory);
            }
            command.spawn()
        });
        if let Err(error) = result {
            eprintln!("rtty: cannot open another window: {error}");
        }
    }

    fn copy_selection(&mut self, clipboard: bool) {
        let Some(text) = self.terminal.selection_text() else {
            return;
        };
        let time = self.x11.time().max(1);
        let clipboard = clipboard || self.config.selection.save_to_clipboard;
        if let Err(error) = self.x11.set_clipboard(&text, true, clipboard, time) {
            eprintln!("rtty: clipboard: {error}");
        }
    }

    fn scrollbar_hit(&self, x: i16) -> bool {
        let (origin_x, _) = self.origin();
        let text_end = origin_x + self.terminal.width() as i32 * self.cell_width as i32;
        x as i32 >= text_end && (x as i32) < text_end + scrollbar::width(self.cell_width) as i32
    }

    fn set_scrollbar_offset(&mut self, y: i16) {
        let (_, origin_y) = self.origin();
        let track = self.terminal.height() as i32 * self.cell_height as i32;
        let offset = scrollbar::offset_from_y(
            self.terminal.height(),
            self.terminal.scrollback_len(),
            y as i32 - origin_y,
            track,
        );
        self.terminal.set_scroll_offset(offset);
    }

    fn menu_entries(&self) -> [(MenuItem, &'static str, bool); 4] {
        [
            (MenuItem::Copy, "Copy", self.terminal.has_selection()),
            (MenuItem::Paste, "Paste", true),
            (MenuItem::SelectAll, "Select All", true),
            (MenuItem::Search, "Find...", true),
        ]
    }

    fn menu_geometry(&self, menu: &ContextMenu) -> (i32, i32, i32, i32) {
        let entries = self.menu_entries();
        let longest = entries
            .iter()
            .map(|(_, label, _)| label.chars().count())
            .max()
            .unwrap_or(0) as i32;
        let cell_width = self.cell_width as i32;
        let cell_height = self.cell_height as i32;
        let width = ((longest + 4).max(10) * cell_width).min(self.x11.width() as i32);
        let height = (entries.len() as i32 * cell_height + 4).min(self.x11.height() as i32);
        let max_x = (self.x11.width() as i32 - width).max(0);
        let max_y = (self.x11.height() as i32 - height).max(0);
        (
            (menu.x as i32).clamp(0, max_x),
            (menu.y as i32).clamp(0, max_y),
            width,
            height,
        )
    }

    fn menu_item_at(&self, menu: &ContextMenu, x: i16, y: i16) -> Option<MenuItem> {
        let entries = self.menu_entries();
        let (menu_x, menu_y, width, height) = self.menu_geometry(menu);
        let x = x as i32;
        let y = y as i32;
        if x < menu_x || x >= menu_x + width || y < menu_y || y >= menu_y + height {
            return None;
        }
        let relative = y - menu_y - 2;
        let row = menu_row_at(relative, self.cell_height as i32, entries.len())?;
        entries.get(row).map(|(item, _, _)| *item)
    }

    fn menu_item_enabled(&self, item: MenuItem) -> bool {
        self.menu_entries()
            .iter()
            .any(|(entry, _, enabled)| *entry == item && *enabled)
    }

    fn open_context_menu(&mut self, x: i16, y: i16) {
        self.context_menu = Some(ContextMenu { x, y, hover: None });
        self.redraw_all = true;
    }

    fn close_context_menu(&mut self) {
        if self.context_menu.take().is_some() {
            self.redraw_all = true;
        }
    }

    fn handle_menu_button(&mut self, button: u8, pressed: bool, x: i16, y: i16, time: u32) -> bool {
        let Some(menu) = self.context_menu else {
            return false;
        };
        if !pressed {
            return true;
        }
        match button {
            1 => {
                let (menu_x, menu_y, width, height) = self.menu_geometry(&menu);
                let x32 = x as i32;
                let y32 = y as i32;
                let inside =
                    x32 >= menu_x && x32 < menu_x + width && y32 >= menu_y && y32 < menu_y + height;
                let item = if inside {
                    self.menu_item_at(&menu, x, y)
                } else {
                    None
                };
                self.close_context_menu();
                if let Some(item) = item
                    && self.menu_item_enabled(item)
                {
                    self.perform_menu_item(item, time);
                }
                return inside;
            }
            2..=5 => {
                self.close_context_menu();
                return false;
            }
            _ => self.close_context_menu(),
        }
        true
    }

    fn handle_menu_motion(&mut self, x: i16, y: i16) -> bool {
        let Some(menu) = self.context_menu else {
            return false;
        };
        let hover = self.menu_item_at(&menu, x, y);
        if let Some(current) = &mut self.context_menu
            && current.hover != hover
        {
            current.hover = hover;
            self.redraw_all = true;
        }
        true
    }

    fn perform_menu_item(&mut self, item: MenuItem, time: u32) {
        match item {
            MenuItem::Copy => self.copy_selection(true),
            MenuItem::Paste => {
                let time = time.max(1);
                if let Err(error) = self.x11.request_paste(false, time) {
                    eprintln!("rtty: paste: {error}");
                }
            }
            MenuItem::SelectAll => {
                self.terminal.select_all();
                self.redraw_all = true;
            }
            MenuItem::Search => self.open_search(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_button(
        &mut self,
        button: u8,
        pressed: bool,
        x: i16,
        y: i16,
        time: u32,
        mods: Mods,
        state: u16,
        now: Instant,
    ) {
        if self.handle_menu_button(button, pressed, x, y, time) {
            return;
        }
        if self.scrollbar_hit(x) {
            if matches!(button, 4 | 5) {
                if pressed {
                    let lines = self.config.scrolling.multiplier as i32;
                    self.terminal
                        .scroll(if button == 4 { -lines } else { lines });
                }
            } else if button == 1 {
                if pressed {
                    self.scroll_drag = true;
                    self.x11.grab_pointer();
                    self.set_scrollbar_offset(y);
                } else {
                    self.scroll_drag = false;
                    self.x11.ungrab_pointer();
                }
            }
            return;
        }
        let Some((row, col)) = self.text_position(x, y) else {
            return;
        };
        let shift =
            mods.shift || state & u16::from(x11rb::protocol::xproto::KeyButMask::SHIFT) != 0;
        let mouse_mode = self.terminal.mouse_mode();

        if matches!(button, 4 | 5) {
            if !pressed {
                return;
            }
            let up = button == 4;
            if mouse_mode != crate::vt::MouseMode::Off && !shift {
                let code = if up { 64 } else { 65 };
                if let Some(bytes) =
                    self.terminal
                        .mouse_report(code, col, row, true, false, mods.into())
                {
                    self.terminal.push_input(&bytes);
                }
            } else if self.terminal.in_alt() {
                let sequence: &[u8] = if up { b"\x1b[A" } else { b"\x1b[B" };
                for _ in 0..self.config.scrolling.multiplier {
                    self.terminal.push_input(sequence);
                }
            } else {
                let lines = self.config.scrolling.multiplier as i32;
                self.terminal.scroll(if up { -lines } else { lines });
            }
            return;
        }
        if button > 3 {
            return;
        }
        let code = button - 1;
        if mouse_mode != crate::vt::MouseMode::Off && !shift {
            if let Some(bytes) =
                self.terminal
                    .mouse_report(code, col, row, pressed, false, mods.into())
            {
                self.terminal.push_input(&bytes);
            }
            return;
        }
        match button {
            1 => {
                if pressed {
                    let count = self.click_count(x, y, now);
                    let mode = match count {
                        1 => SelectionMode::Char,
                        2 => SelectionMode::Word,
                        _ => SelectionMode::Line,
                    };
                    self.terminal.begin_selection(row, col, mode);
                    self.drag = Some(Drag {
                        last_x: x,
                        last_y: y,
                        last_row: row,
                        last_col: col,
                    });
                    self.auto_scroll_at = now + Duration::from_millis(60);
                    self.x11.grab_pointer();
                } else if self.drag.take().is_some() {
                    self.copy_selection(false);
                    self.x11.ungrab_pointer();
                }
            }
            2 => {
                if pressed {
                    let time = time.max(1);
                    if let Err(error) = self.x11.request_paste(true, time) {
                        eprintln!("rtty: paste: {error}");
                    }
                }
            }
            3 if pressed => self.open_context_menu(x, y),
            _ => {}
        }
    }

    fn click_count(&mut self, x: i16, y: i16, now: Instant) -> u8 {
        let count = match &self.last_click {
            Some(click)
                if now.duration_since(click.at) < Duration::from_millis(400)
                    && (x - click.x).abs() <= 3
                    && (y - click.y).abs() <= 3 =>
            {
                (click.count % 3) + 1
            }
            _ => 1,
        };
        self.last_click = Some(Click {
            at: now,
            x,
            y,
            count,
        });
        count
    }

    fn handle_motion(&mut self, x: i16, y: i16, mods: Mods, state: u16, now: Instant) {
        if self.handle_menu_motion(x, y) {
            return;
        }
        let primary = u16::from(x11rb::protocol::xproto::KeyButMask::BUTTON1);
        let held = state & primary != 0;
        if self.scroll_drag {
            if !held {
                self.scroll_drag = false;
                self.x11.ungrab_pointer();
                return;
            }
            self.set_scrollbar_offset(y);
            return;
        }
        let (row, col) = self.clamped_position(x, y);
        if self.drag.is_some() && !held {
            self.drag = None;
            self.x11.ungrab_pointer();
        }
        if let Some(drag) = &mut self.drag {
            drag.last_x = x;
            drag.last_y = y;
            if row != drag.last_row || col != drag.last_col {
                drag.last_row = row;
                drag.last_col = col;
                self.terminal.update_selection(row, col);
            }
            self.auto_scroll_at = now + Duration::from_millis(60);
            return;
        }
        let mode = self.terminal.mouse_mode();
        let button_mask = u16::from(x11rb::protocol::xproto::KeyButMask::BUTTON1)
            | u16::from(x11rb::protocol::xproto::KeyButMask::BUTTON2)
            | u16::from(x11rb::protocol::xproto::KeyButMask::BUTTON3);
        let held = state & button_mask != 0;
        if mode == crate::vt::MouseMode::AnyEvent
            || (mode == crate::vt::MouseMode::ButtonEvent && held)
        {
            let Some((row, col)) = self.text_position(x, y) else {
                return;
            };
            if let Some(bytes) = self
                .terminal
                .mouse_report(0, col, row, true, true, mods.into())
            {
                self.terminal.push_input(&bytes);
            }
        }
    }

    fn text_position(&self, x: i16, y: i16) -> Option<(usize, usize)> {
        let (origin_x, origin_y) = self.origin();
        if x < origin_x as i16 || y < origin_y as i16 {
            return None;
        }
        let col = (x - origin_x as i16) as usize / self.cell_width as usize;
        let row = (y - origin_y as i16) as usize / self.cell_height as usize;
        if col >= self.terminal.width() || row >= self.terminal.height() {
            return None;
        }
        Some((row, col))
    }

    fn clamped_position(&self, x: i16, y: i16) -> (usize, usize) {
        let (origin_x, origin_y) = self.origin();
        let col = ((x - origin_x as i16).max(0) as usize / self.cell_width as usize)
            .min(self.terminal.width().saturating_sub(1));
        let row = ((y - origin_y as i16).max(0) as usize / self.cell_height as usize)
            .min(self.terminal.height().saturating_sub(1));
        (row, col)
    }

    fn origin(&self) -> (i32, i32) {
        (
            self.padding.x as i32 + self.config.font.offset.x.round() as i32,
            self.padding.y as i32 + self.config.font.offset.y.round() as i32,
        )
    }

    fn autoscroll(&mut self, now: Instant) {
        self.auto_scroll_at = now + Duration::from_millis(60);
        let Some(drag) = &self.drag else {
            return;
        };
        let (_, origin_y) = self.origin();
        let bottom = origin_y + self.terminal.height() as i32 * self.cell_height as i32;
        let (delta, row) = if drag.last_y < origin_y as i16 {
            (-1, 0)
        } else if drag.last_y >= bottom as i16 {
            (1, self.terminal.height() - 1)
        } else {
            return;
        };
        let can = if delta < 0 {
            self.terminal.scroll_offset() < self.terminal.scrollback_len()
        } else {
            self.terminal.scroll_offset() > 0
        };
        if can {
            self.terminal.scroll(delta);
            let col = ((drag.last_x - self.origin().0 as i16).max(0) as usize
                / self.cell_width as usize)
                .min(self.terminal.width().saturating_sub(1));
            self.terminal.update_selection(row, col);
        }
    }

    fn read_pty(&mut self) -> bool {
        let mut read_any = false;
        for _ in 0..256 {
            match self.pty_rx.try_recv() {
                Ok(chunk) => {
                    read_any = true;
                    self.parser.advance(&chunk, &mut self.terminal);
                }
                Err(TryRecvError::Empty) => return read_any,
                Err(TryRecvError::Disconnected) => {
                    self.pty_eof = true;
                    return read_any;
                }
            }
        }
        read_any
    }

    fn flush_input(&mut self) {
        let output = self.terminal.take_output();
        if !output.is_empty() {
            self.pty.queue(&output);
        }
        if let Err(error) = self.pty.flush()
            && error.kind() != std::io::ErrorKind::WouldBlock
        {
            eprintln!("rtty: write: {error}");
        }
    }

    fn check_child(&mut self, now: Instant) {
        if self.child_exit.is_none() {
            if let Some(exit) = self.pty.try_wait() {
                self.child_exit = Some(exit);
                self.child_exit_at = Some(now);
            }
        }
        let Some(exit) = self.child_exit else {
            return;
        };
        if self.smoke {
            return;
        }
        if self.hold {
            if !self.hold_notice {
                let description = if exit.signaled {
                    format!("signal {}", exit.code.saturating_sub(128))
                } else {
                    format!("status {}", exit.code)
                };
                let message = format!("\r\n\x1b[0m[rtty: process exited with {description}]\r\n");
                self.parser.advance(message.as_bytes(), &mut self.terminal);
                self.hold_notice = true;
            }
        } else if self.pty_eof
            || self
                .child_exit_at
                .is_some_and(|instant| now.duration_since(instant) > Duration::from_millis(500))
        {
            self.quit = true;
        }
    }

    fn terminal_notifications(&mut self, now: Instant) {
        if self.terminal.take_bell() {
            if self.config.bell.audible {
                self.x11.bell();
            }
            if self.config.bell.visual {
                self.bell_until = Some(now + Duration::from_millis(80));
                self.terminal.mark_all();
            }
        }
        if let Some(title) = self.terminal.take_title()
            && self.config.window.dynamic_title
            && let Err(error) = self.x11.set_title(&title)
        {
            eprintln!("rtty: title: {error}");
        }
        for write in self.terminal.take_clipboard_writes() {
            let primary = write.selection == b'p';
            let clipboard = !primary;
            let time = self.x11.time().max(1);
            if let Err(error) = self
                .x11
                .set_clipboard(&write.text, primary, clipboard, time)
            {
                eprintln!("rtty: clipboard: {error}");
            }
        }
        if let Some(selection) = self.terminal.take_clipboard_read() {
            let primary = selection == b'p';
            let text = self.x11.clipboard_text(primary).map(str::to_string);
            self.terminal
                .answer_clipboard_read(selection, text.as_deref());
        }
    }

    fn resize_request(&mut self) {
        if let Some((cols, rows)) = self.terminal.take_resize_request() {
            let width = (cols as u16)
                .saturating_mul(self.cell_width)
                .saturating_add(self.padding.x.saturating_mul(2));
            let height = (rows as u16)
                .saturating_mul(self.cell_height)
                .saturating_add(self.padding.y.saturating_mul(2));
            if let Err(error) = self.x11.resize_window(width, height) {
                eprintln!("rtty: resize: {error}");
            }
        }
    }

    fn maybe_reload(&mut self, now: Instant) {
        self.next_config_check = now + Duration::from_secs(2);
        let force = std::mem::take(&mut self.force_reload);
        let Some(path) = self.config_path.clone() else {
            self.force_reload = false;
            return;
        };
        let modified = std::fs::metadata(&path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        if !force && modified == self.config_modified {
            return;
        }
        match Config::load(Some(&path)) {
            Ok(loaded) => {
                let old_title = self.config.window.title.clone();
                let theme = match Theme::from_config(&loaded.config.colors) {
                    Ok(theme) => theme,
                    Err(error) => {
                        eprintln!("rtty: config: {error}");
                        return;
                    }
                };
                self.config = loaded.config;
                self.config_modified = modified;
                if let Err(error) = self.x11.set_opacity(self.config.window.opacity) {
                    eprintln!("rtty: opacity: {error}");
                }
                self.canvas.argb = self.x11.is_argb();
                self.redraw_all = true;
                self.terminal.mark_all();
                self.terminal.set_theme(theme);
                match Bindings::new(&self.config.keyboard) {
                    Ok(bindings) => self.bindings = bindings,
                    Err(error) => eprintln!("rtty: config: {error}"),
                }
                self.font_size_override = None;
                if let Err(error) = self.rebuild_renderer() {
                    eprintln!("rtty: config: {error}");
                }
                self.padding = self.config.window.padding;
                if self.terminal.title() == old_title {
                    let title = self.config.window.title.clone();
                    self.terminal.set_initial_title(&title);
                    let _ = self.x11.set_title(&title);
                }
                self.resize_to_window();
                let _ = self.x11.set_size_hints(
                    self.padding,
                    self.cell_width,
                    self.cell_height,
                    scrollbar::width(self.cell_width),
                );
                eprintln!("rtty: reloaded {}", path.display());
            }
            Err(error) => eprintln!("rtty: {error}"),
        }
    }

    fn rebuild_renderer(&mut self) -> Result<(), String> {
        let mut renderer = Renderer::new(
            &self.config.font,
            self.dpi,
            self.font_size_override,
            self.font_database.clone(),
        )?;
        renderer.set_cursor_options(
            self.config.cursor.thickness,
            self.config.cursor.unfocused_hollow,
        );
        self.cell_width = renderer.cell_width();
        self.cell_height = renderer.cell_height();
        self.renderer = renderer;
        self.resize_to_window();
        self.x11.set_size_hints(
            self.padding,
            self.cell_width,
            self.cell_height,
            scrollbar::width(self.cell_width),
        )?;
        self.redraw_all = true;
        Ok(())
    }

    fn change_font_size(&mut self, delta: f32) {
        let current = self.font_size_override.unwrap_or(self.config.font.size);
        let next = (current + delta).clamp(4.0, 96.0);
        if (next - current).abs() < f32::EPSILON {
            return;
        }
        self.font_size_override = Some(next);
        if let Err(error) = self.rebuild_renderer() {
            eprintln!("rtty: {error}");
        }
    }

    fn resize_to_window(&mut self) {
        let width = self.x11.width().max(1);
        let height = self.x11.height().max(1);
        let base = self.config.window.padding;
        let (mut pad_x, mut pad_y) = (base.x, base.y);
        if self.config.window.dynamic_padding {
            let gutter = scrollbar::width(self.cell_width);
            let available = width
                .saturating_sub(base.x.saturating_mul(2))
                .saturating_sub(gutter)
                .max(1);
            let cols = (available / self.cell_width.max(1)).max(1) as u32;
            let used = cols as u16 * self.cell_width;
            pad_x = width.saturating_sub(gutter).saturating_sub(used) / 2;
            let rows = (height.saturating_sub(base.y.saturating_mul(2)) / self.cell_height.max(1))
                .max(1) as u32;
            let used = rows as u16 * self.cell_height;
            pad_y = height.saturating_sub(used) / 2;
        }
        self.padding = Padding { x: pad_x, y: pad_y };
        let gutter = scrollbar::width(self.cell_width);
        let cols = width
            .saturating_sub(pad_x.saturating_mul(2))
            .saturating_sub(gutter)
            .max(1) as usize
            / self.cell_width.max(1) as usize;
        let rows = height.saturating_sub(pad_y.saturating_mul(2)).max(1) as usize
            / self.cell_height.max(1) as usize;
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols != self.terminal.width() || rows != self.terminal.height() {
            self.terminal
                .resize(cols, rows, self.cell_width, self.cell_height);
            self.terminal.set_window_pixels(width, height);
            self.pty.resize(cols, rows, width as usize, height as usize);
            self.terminal.mark_all();
        } else {
            self.terminal.cell_width = self.cell_width;
            self.terminal.cell_height = self.cell_height;
            self.terminal.set_window_pixels(width, height);
        }
        if self.canvas.width != width || self.canvas.height != height {
            self.canvas.resize(width, height);
            self.canvas.argb = self.x11.is_argb();
            self.canvas.fill(pixel(
                &self.canvas,
                self.terminal.theme().background,
                self.x11.background_alpha(),
            ));
            self.terminal.mark_all();
        }
        if self.search.is_some() {
            self.refresh_active_search();
            self.terminal.reveal_search();
            self.search_stale = false;
        }
        self.redraw_all = true;
    }

    fn draw_overlays(&mut self) {
        self.draw_search_bar();
        self.draw_context_menu();
    }

    fn draw_search_bar(&mut self) {
        let display = {
            let Some(search) = &self.search else {
                return;
            };
            let query = &search.query;
            let (current, total) = self.terminal.search_summary();
            let max_cells = self.terminal.width().saturating_sub(2).max(1);
            let prefix = "Find: ";
            let suffix = if query.is_empty() {
                String::new()
            } else {
                format!("  {current}/{total}")
            };
            let reserved = prefix.width() + suffix.width();
            if reserved >= max_cells {
                tail_to_width(&format!("{prefix}{query}"), max_cells)
            } else {
                let available = max_cells - reserved;
                format!("{prefix}{}{suffix}", tail_to_width(query, available))
            }
        };
        let cell_width = self.cell_width as i32;
        let cell_height = self.cell_height as i32;
        let width = (display.width() as i32 + 2) * cell_width;
        let height = cell_height + 4;
        let (origin_x, origin_y) = self.origin();
        let text_width = self.terminal.width() as i32 * cell_width;
        let x = (origin_x + text_width - width).max(origin_x);
        let y = origin_y;
        let (background, foreground, border) = {
            let theme = self.terminal.theme();
            (
                theme.selection_background,
                theme.selection_foreground,
                theme.foreground,
            )
        };
        let background = pixel(&self.canvas, background, 255);
        let border = pixel(&self.canvas, border, 255);
        self.canvas.fill_rect(x, y, width, height, background);
        self.canvas.fill_rect(x, y, width, 1, border);
        self.canvas.fill_rect(x, y + height - 1, width, 1, border);
        self.canvas.fill_rect(x, y, 1, height, border);
        self.canvas.fill_rect(x + width - 1, y, 1, height, border);
        self.renderer.draw_label(
            &mut self.canvas,
            &display,
            x + cell_width,
            y + 2,
            foreground,
        );
    }

    fn draw_context_menu(&mut self) {
        let Some(menu) = self.context_menu else {
            return;
        };
        let entries = self.menu_entries();
        let (x, y, width, height) = self.menu_geometry(&menu);
        let cell_width = self.cell_width as i32;
        let cell_height = self.cell_height as i32;
        let (background, foreground, dim, selection_background, selection_foreground) = {
            let theme = self.terminal.theme();
            (
                theme.background,
                theme.foreground,
                theme.dim_foreground,
                theme.selection_background,
                theme.selection_foreground,
            )
        };
        let background = pixel(&self.canvas, background, 255);
        let border = pixel(&self.canvas, dim, 255);
        let hover = pixel(&self.canvas, selection_background, 255);
        self.canvas.fill_rect(x, y, width, height, background);
        for (index, (item, label, enabled)) in entries.iter().enumerate() {
            let row_y = y + 2 + index as i32 * cell_height;
            let hovered = menu.hover == Some(*item);
            if hovered {
                self.canvas
                    .fill_rect(x + 1, row_y, width - 2, cell_height, hover);
            }
            let color = if !enabled {
                dim
            } else if hovered {
                selection_foreground
            } else {
                foreground
            };
            self.renderer
                .draw_label(&mut self.canvas, label, x + cell_width, row_y, color);
        }
        self.canvas.fill_rect(x, y, width, 1, border);
        self.canvas.fill_rect(x, y + height - 1, width, 1, border);
        self.canvas.fill_rect(x, y, 1, height, border);
        self.canvas.fill_rect(x + width - 1, y, 1, height, border);
    }

    fn cursor_damage(&mut self) {
        let current = (self.terminal.cursor.x, self.terminal.cursor.y);
        if self.last_cursor != Some(current) {
            if let Some((_, row)) = self.last_cursor {
                self.terminal.mark_row_public(row);
            }
            self.terminal.mark_cursor_dirty();
            self.last_cursor = Some(current);
        }
    }

    fn render(&mut self, now: Instant) {
        if self.search.is_some() && self.search_stale {
            self.search_stale = false;
            self.refresh_active_search();
        }
        let mut damage = self.terminal.take_damage();
        if self.redraw_all {
            damage.set_all(self.terminal.height());
            self.redraw_all = false;
        }
        if self.bell_until.is_some() {
            damage.set_all(self.terminal.height());
        }
        if damage.is_empty() {
            self.terminal.recycle_damage(damage);
            return;
        }
        let cursor_on = if self.focused {
            !self.cursor_blinking() || self.blink_on
        } else {
            true
        };
        let (origin_x, origin_y) = self.origin();
        // An overlay stays in place while the terminal scrolls, so the canvas
        // shift would leave a stale copy behind. Redraw everything instead.
        if damage.scroll != 0 && (self.search.is_some() || self.context_menu.is_some()) {
            damage.set_all(self.terminal.height());
        }
        // A whole-screen scroll only moves pixels. Shift the canvas once and
        // let the newly exposed rows be redrawn, instead of rasterizing every
        // cell. Presentation still needs the whole canvas since it all moved.
        let scroll = if damage.all { 0 } else { damage.scroll };
        if scroll != 0 {
            let background = pixel(
                &self.canvas,
                self.terminal.theme().background,
                self.x11.background_alpha(),
            );
            self.canvas
                .scroll_rows(scroll, self.cell_height as i32, background);
        }
        self.renderer.draw(
            &mut self.canvas,
            &self.terminal,
            &damage,
            origin_x,
            origin_y,
            cursor_on,
            self.focused,
            self.x11.background_alpha(),
        );
        if self.bell_until.is_some_and(|until| now < until) {
            for pixel in &mut self.canvas.pixels {
                *pixel ^= 0x00ff_ffff;
            }
        }
        self.draw_overlays();
        let mut present_damage = damage;
        if scroll != 0 {
            present_damage.all = true;
        }
        self.x11
            .present(&self.canvas, &present_damage, self.cell_height, origin_y);
        self.terminal.recycle_damage(present_damage);
        self.rendered_once = true;
        if self.smoke && self.rendered_once {
            self.quit = true;
        }
    }

    fn wait(&mut self, now: Instant) -> Result<(), String> {
        let mut earliest: Option<Instant> = None;
        let mut deadline = |d: Instant| {
            earliest = Some(earliest.map_or(d, |e| e.min(d)));
        };
        if self.terminal.has_damage() || self.redraw_all {
            deadline(self.last_pty_input + Duration::from_millis(10));
            deadline(self.next_frame);
        }
        if self.cursor_blinking() {
            deadline(self.blink_deadline);
        }
        if self.drag.is_some() {
            deadline(self.auto_scroll_at);
        }
        if let Some(until) = self.bell_until {
            deadline(until);
        }
        if self.config_path.is_some() {
            deadline(self.next_config_check);
        }
        if let Some(exited) = self.child_exit_at
            && !self.hold
            && !self.pty_eof
        {
            deadline(exited + Duration::from_millis(500));
        }
        let timeout = earliest
            .map(|deadline| {
                let millis = deadline.saturating_duration_since(now).as_millis();
                millis.clamp(1, i32::MAX as u128) as i32
            })
            .unwrap_or(-1);
        let mut fds = [
            libc::pollfd {
                fd: self.x11.fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.pty.fd(),
                events: libc::POLLIN
                    | if self.pty.has_pending() {
                        libc::POLLOUT
                    } else {
                        0
                    },
                revents: 0,
            },
            libc::pollfd {
                fd: self.signals.fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.pty_wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: fds is a valid array of pollfd values.
        let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                return Ok(());
            }
            return Err(format!("poll: {error}"));
        }
        Ok(())
    }
}

fn menu_row_at(relative_y: i32, cell_height: i32, rows: usize) -> Option<usize> {
    if relative_y < 0 || cell_height <= 0 {
        return None;
    }
    let row = (relative_y / cell_height) as usize;
    (row < rows).then_some(row)
}

fn tail_to_width(text: &str, max: usize) -> String {
    let mut kept = Vec::new();
    let mut used = 0;
    for ch in text.chars().rev() {
        let width = ch.width().unwrap_or(0);
        if used + width > max {
            break;
        }
        used += width;
        kept.push(ch);
    }
    kept.into_iter().rev().collect()
}

fn is_modifier_key(keysym: u32) -> bool {
    matches!(
        keysym,
        key::Shift_L
            | key::Shift_R
            | key::Control_L
            | key::Control_R
            | key::Alt_L
            | key::Alt_R
            | key::Meta_L
            | key::Meta_R
            | key::Super_L
            | key::Super_R
            | key::Hyper_L
            | key::Hyper_R
            | key::ISO_Level3_Shift
            | key::Mode_switch
            | key::Caps_Lock
            | key::Shift_Lock
            | key::Num_Lock
    )
}

fn parse_cursor_style(value: &str) -> CursorStyle {
    match value.to_ascii_lowercase().as_str() {
        "beam" | "bar" => CursorStyle::Beam,
        "underline" | "undercurl" => CursorStyle::Underline,
        _ => CursorStyle::Block,
    }
}

impl From<Mods> for crate::vt::MouseMods {
    fn from(mods: Mods) -> Self {
        Self {
            shift: mods.shift,
            alt: mods.alt,
            ctrl: mods.control,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_truncation_counts_display_cells() {
        assert_eq!(tail_to_width("abcdef", 3), "def");
        assert_eq!(tail_to_width("\u{4e16}\u{754c}ab", 3), "ab");
        assert_eq!(tail_to_width("\u{4e16}\u{754c}ab", 4), "\u{754c}ab");
    }

    #[test]
    fn menu_rows_skip_padding_and_overflow() {
        assert_eq!(menu_row_at(0, 20, 4), Some(0));
        assert_eq!(menu_row_at(20, 20, 4), Some(1));
        assert_eq!(menu_row_at(-2, 20, 4), None);
        assert_eq!(menu_row_at(80, 20, 4), None);
    }

    #[test]
    fn modifier_presses_do_not_dismiss_menus() {
        assert!(is_modifier_key(key::Shift_L));
        assert!(is_modifier_key(key::Control_R));
        assert!(!is_modifier_key(key::Escape));
        assert!(!is_modifier_key(key::f));
    }
}
