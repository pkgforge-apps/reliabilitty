//! Application state and the single-threaded event loop.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use crate::cli::Cli;
use crate::config::{Config, Padding};
use crate::input::{Action, Bindings, Composer, Mods, encode_key};
use crate::pty::Pty;
use crate::render::{Canvas, Renderer, pixel};
use crate::scrollbar;
use crate::signals::Signals;
use crate::theme::Theme;
use crate::vt::{CursorStyle, Parser, SelectionMode, Terminal};
use crate::x11::{UiEvent, X11};

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

struct App {
    config: Config,
    config_path: Option<PathBuf>,
    config_modified: Option<SystemTime>,
    x11: X11,
    pty: Pty,
    terminal: Terminal,
    parser: Parser,
    renderer: Renderer,
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
    auto_scroll_at: Instant,
    next_config_check: Instant,
    force_reload: bool,
    bell_until: Option<Instant>,
    redraw_all: bool,
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
    } else {
        96.0
    };
    let mut renderer = Renderer::new(&config.font, dpi, None)?;
    renderer.set_cursor_options(config.cursor.thickness, config.cursor.unfocused_hollow);
    let cell_width = renderer.cell_width();
    let cell_height = renderer.cell_height();
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
    let x11 = X11::new(
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
    let bindings = Bindings::new(&config.keyboard)?;
    let now = Instant::now();
    let mut app = App {
        config,
        config_path,
        config_modified,
        x11,
        pty,
        terminal,
        parser: Parser::new(),
        renderer,
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
        auto_scroll_at: now,
        next_config_check: now + Duration::from_secs(2),
        force_reload: false,
        bell_until: None,
        redraw_all: true,
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
            self.read_pty();
            self.terminal_notifications(now);
            self.flush_input();
            self.check_child(now);
            self.resize_request();
            if self.force_reload || now >= self.next_config_check {
                self.maybe_reload(now);
            }
            self.cursor_damage();
            self.render(now);
            if self.quit {
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
                    self.x11.ungrab_pointer();
                }
            }
            UiEvent::Close => self.quit = true,
            UiEvent::Paste(text) => {
                self.terminal.scroll_to_bottom();
                self.terminal.paste(&text);
            }
            UiEvent::Error(error) => eprintln!("rtty: {error}"),
        }
        Ok(())
    }

    fn handle_key(&mut self, keysym: u32, mods: Mods, time: u32) {
        if keysym == 0 {
            return;
        }
        let bound = self.bindings.find(keysym, mods).map(|binding| {
            (
                binding.action().cloned(),
                binding.chars().map(str::to_string),
            )
        });
        if let Some((action, chars)) = bound {
            if let Some(chars) = chars {
                self.terminal.scroll_to_bottom();
                self.terminal.push_input(chars.as_bytes());
            } else if let Some(action) = action {
                self.perform_action(action, time);
            }
            return;
        }
        if let Some(bytes) = encode_key(
            keysym,
            mods,
            self.terminal.app_cursor(),
            self.terminal.app_keypad(),
            &mut self.composer,
        ) {
            self.terminal.scroll_to_bottom();
            self.terminal.push_input(&bytes);
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
            3 => {}
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

    fn read_pty(&mut self) {
        let mut buffer = [0u8; 65536];
        for _ in 0..64 {
            match self.pty.read(&mut buffer) {
                Ok(0) => {
                    self.pty_eof = true;
                    break;
                }
                Ok(count) => {
                    self.parser.advance(&buffer[..count], &mut self.terminal);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => {
                    eprintln!("rtty: read: {error}");
                    self.pty_eof = true;
                    break;
                }
            }
        }
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
        let mut renderer = Renderer::new(&self.config.font, self.dpi, self.font_size_override)?;
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
        self.redraw_all = true;
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
        let mut damage = self.terminal.take_damage();
        if self.redraw_all {
            damage.all = true;
            damage.rows = vec![true; self.terminal.height()];
            self.redraw_all = false;
        }
        if self.bell_until.is_some() {
            damage.all = true;
            damage.rows = vec![true; self.terminal.height()];
        }
        if damage.is_empty() {
            return;
        }
        let cursor_on = if self.focused {
            !self.cursor_blinking() || self.blink_on
        } else {
            true
        };
        let (origin_x, origin_y) = self.origin();
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
        self.x11
            .present(&self.canvas, &damage, self.cell_height, origin_y);
        self.rendered_once = true;
        if self.smoke && self.rendered_once {
            self.quit = true;
        }
    }

    fn wait(&mut self, now: Instant) -> Result<(), String> {
        let mut deadlines: Vec<Instant> = Vec::new();
        if self.cursor_blinking() {
            deadlines.push(self.blink_deadline);
        }
        if self.drag.is_some() {
            deadlines.push(self.auto_scroll_at);
        }
        if let Some(until) = self.bell_until {
            deadlines.push(until);
        }
        if self.config_path.is_some() {
            deadlines.push(self.next_config_check);
        }
        if let Some(exited) = self.child_exit_at
            && !self.hold
            && !self.pty_eof
        {
            deadlines.push(exited + Duration::from_millis(500));
        }
        let timeout = deadlines
            .into_iter()
            .min()
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
