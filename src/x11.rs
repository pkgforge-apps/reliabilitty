//! Direct X11 protocol, using x11rb. No libX11 or XCB.

use std::collections::BTreeMap;
use std::os::fd::{AsRawFd, OwnedFd};

use x11rb::connection::{Connection, RequestConnection};
use x11rb::properties::WmSizeHints;
use x11rb::protocol::Event;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shm::{self, ConnectionExt as _};
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    self, Atom, AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask, KeyButMask, PropMode,
    SelectionNotifyEvent, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use xkeysym::{KeyCode, Keysym};

use crate::config::{Dimensions, Padding, WindowConfig};
use crate::input::Mods;
use crate::render::Canvas;
use crate::scrollbar;
use crate::theme::Rgb;

#[derive(Clone, Debug)]
pub enum UiEvent {
    Key {
        keysym: u32,
        mods: Mods,
        time: u32,
    },
    Button {
        button: u8,
        pressed: bool,
        x: i16,
        y: i16,
        time: u32,
        mods: Mods,
        state: u16,
    },
    Motion {
        x: i16,
        y: i16,
        mods: Mods,
        state: u16,
    },
    Resize {
        width: u16,
        height: u16,
    },
    Expose,
    Focus(bool),
    Close,
    Paste(String),
    /// A protocol error that does not stop the loop.
    Error(String),
}

struct Atoms {
    wm_protocols: Atom,
    wm_delete_window: Atom,
    net_wm_name: Atom,
    net_wm_pid: Atom,
    net_wm_window_opacity: Atom,
    net_wm_state: Atom,
    net_wm_state_fullscreen: Atom,
    net_wm_state_maximized_vert: Atom,
    net_wm_state_maximized_horz: Atom,
    motif_wm_hints: Atom,
    utf8_string: Atom,
    clipboard: Atom,
    primary: Atom,
    targets: Atom,
    text: Atom,
    plain_utf8: Atom,
    incr: Atom,
    reliabilitty_selection: Atom,
}

#[derive(Clone, Copy, Debug)]
struct PixelFormat {
    red: u32,
    green: u32,
    blue: u32,
    standard: bool,
}

impl PixelFormat {
    fn from_masks(red: u32, green: u32, blue: u32) -> Option<Self> {
        if red == 0 || green == 0 || blue == 0 {
            return None;
        }
        Some(Self {
            red: red.trailing_zeros(),
            green: green.trailing_zeros(),
            blue: blue.trailing_zeros(),
            standard: red == 0x00ff_0000 && green == 0x0000_ff00 && blue == 0x0000_00ff,
        })
    }

    fn pixel(self, color: u32) -> u32 {
        (((color >> 16) & 0xff) << self.red)
            | (((color >> 8) & 0xff) << self.green)
            | ((color & 0xff) << self.blue)
    }
}

struct ShmSegment {
    id: shm::Seg,
    address: *mut u8,
    length: usize,
}

impl ShmSegment {
    fn new(connection: &RustConnection, length: usize) -> Option<Self> {
        let id = connection.generate_id().ok()?;
        let reply = connection
            .shm_create_segment(id, length as u32, false)
            .ok()?
            .reply()
            .ok()?;
        let fd: OwnedFd = reply.shm_fd;
        // SAFETY: the descriptor is valid and fresh.
        let address = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if address == libc::MAP_FAILED {
            let _ = connection.shm_detach(id);
            return None;
        }
        Some(Self {
            id,
            address: address as *mut u8,
            length,
        })
    }

    fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: the mapping is valid for `length` bytes and not aliased.
        unsafe { std::slice::from_raw_parts_mut(self.address, self.length) }
    }

    fn destroy(&mut self, connection: &RustConnection) {
        if !self.address.is_null() {
            // SAFETY: the address and length came from mmap.
            unsafe {
                libc::munmap(self.address as *mut libc::c_void, self.length);
            }
            self.address = std::ptr::null_mut();
        }
        let _ = connection.shm_detach(self.id);
    }
}

impl Drop for ShmSegment {
    fn drop(&mut self) {
        if !self.address.is_null() {
            // SAFETY: the address and length came from mmap.
            unsafe {
                libc::munmap(self.address as *mut libc::c_void, self.length);
            }
        }
    }
}

struct Keymap {
    min_keycode: u8,
    keysyms_per_keycode: u8,
    keysyms: Vec<u32>,
    /// Modifier bit for NumLock, which selects the digit level of keypad keys.
    numlock_mask: u16,
}

impl Keymap {
    fn new(connection: &RustConnection) -> Result<Self, String> {
        let setup = connection.setup();
        let min_keycode = setup.min_keycode;
        let max_keycode = setup.max_keycode;
        let count = max_keycode.saturating_sub(min_keycode).saturating_add(1);
        let reply = connection
            .get_keyboard_mapping(min_keycode, count)
            .map_err(|error| error.to_string())?
            .reply()
            .map_err(|error| error.to_string())?;
        let mut keymap = Self {
            min_keycode,
            keysyms_per_keycode: reply.keysyms_per_keycode,
            keysyms: reply.keysyms,
            numlock_mask: 1 << 4,
        };
        keymap.numlock_mask = keymap.find_numlock_mask(connection);
        Ok(keymap)
    }

    fn find_numlock_mask(&self, connection: &RustConnection) -> u16 {
        let Ok(cookie) = connection.get_modifier_mapping() else {
            return self.numlock_mask;
        };
        let Ok(reply) = cookie.reply() else {
            return self.numlock_mask;
        };
        let per = reply.keycodes.len() / 8;
        if per == 0 {
            return self.numlock_mask;
        }
        for (index, codes) in reply.keycodes.chunks(per).enumerate() {
            if codes.iter().any(|code| {
                *code >= self.min_keycode && self.keysym_at(*code, 0) == xkeysym::key::Num_Lock
            }) {
                return 1 << index;
            }
        }
        self.numlock_mask
    }

    fn keysym_at(&self, keycode: u8, column: u8) -> u32 {
        if keycode < self.min_keycode || column >= self.keysyms_per_keycode {
            return 0;
        }
        let index = (keycode as usize - self.min_keycode as usize)
            * self.keysyms_per_keycode as usize
            + column as usize;
        self.keysyms.get(index).copied().unwrap_or(0)
    }

    fn keysym(&self, keycode: u8, group: u8, shift: bool, alt_gr: bool) -> u32 {
        if self.keysyms_per_keycode == 0 {
            return 0;
        }
        let mut column = group.saturating_mul(2) + u8::from(shift);
        if alt_gr && group == 0 && self.keysyms_per_keycode >= 4 {
            column = 2 + u8::from(shift);
        }
        if column >= self.keysyms_per_keycode {
            column = u8::from(shift);
        }
        xkeysym::keysym(
            KeyCode::new(keycode as u32),
            column,
            KeyCode::new(self.min_keycode as u32),
            self.keysyms_per_keycode,
            &self.keysyms,
        )
        .map(|keysym| keysym.raw())
        .unwrap_or(0)
    }
}

#[derive(Default)]
struct Clipboard {
    clipboard_text: Option<String>,
    primary_text: Option<String>,
    pending: Option<PendingPaste>,
}

struct PendingPaste {
    selection: u8,
    fallback: bool,
}

pub struct X11 {
    connection: RustConnection,
    root: Window,
    window: Window,
    gc: xproto::Gcontext,
    atoms: Atoms,
    pixel_format: PixelFormat,
    depth: u8,
    colormap: Option<xproto::Colormap>,
    argb: bool,
    background_alpha: u8,
    width: u16,
    height: u16,
    keymap: Keymap,
    xkb: bool,
    shm: Option<ShmSegment>,
    shm_failed: bool,
    clipboard: Clipboard,
    last_time: u32,
    fullscreen: bool,
}

impl X11 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        window_config: &WindowConfig,
        title: &str,
        instance: &str,
        class: &str,
        dimensions: Dimensions,
        cell_width: u16,
        cell_height: u16,
        background: Rgb,
    ) -> Result<Self, String> {
        let (connection, screen_number) =
            x11rb::connect(None).map_err(|error| format!("cannot open X11 display: {error}"))?;
        let screen = connection.setup().roots[screen_number].clone();
        let root = screen.root;
        let root_visual = screen
            .allowed_depths
            .iter()
            .flat_map(|depth| &depth.visuals)
            .find(|visual| visual.visual_id == screen.root_visual)
            .cloned()
            .ok_or_else(|| "the root visual is not described by the server".to_string())?;
        let argb_visual = screen
            .allowed_depths
            .iter()
            .find(|depth| depth.depth == 32)
            .and_then(|depth| {
                depth
                    .visuals
                    .iter()
                    .find(|visual| {
                        visual.class == xproto::VisualClass::TRUE_COLOR
                            || visual.class == xproto::VisualClass::DIRECT_COLOR
                    })
                    .cloned()
            });
        let (depth, visual_id, visual, argb) = match argb_visual {
            Some(visual) => (32, visual.visual_id, visual, true),
            None => (screen.root_depth, screen.root_visual, root_visual, false),
        };
        if visual.class != xproto::VisualClass::TRUE_COLOR
            && visual.class != xproto::VisualClass::DIRECT_COLOR
        {
            return Err("Reliabilitty needs a TrueColor X11 visual".to_string());
        }
        let pixel_format =
            PixelFormat::from_masks(visual.red_mask, visual.green_mask, visual.blue_mask)
                .ok_or_else(|| "the X11 visual has no RGB masks".to_string())?;
        let background_alpha = (window_config.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        let atoms = Atoms::intern(&connection)?;
        let window = connection
            .generate_id()
            .map_err(|error| error.to_string())?;
        let gc = connection
            .generate_id()
            .map_err(|error| error.to_string())?;
        let colormap = if argb {
            let colormap = connection
                .generate_id()
                .map_err(|error| error.to_string())?;
            connection
                .create_colormap(xproto::ColormapAlloc::NONE, colormap, root, visual_id)
                .map_err(|error| error.to_string())?;
            Some(colormap)
        } else {
            None
        };

        let columns = if dimensions.columns == 0 {
            80
        } else {
            dimensions.columns
        };
        let lines = if dimensions.lines == 0 {
            24
        } else {
            dimensions.lines
        };
        let padding = window_config.padding;
        let width = columns
            .saturating_mul(cell_width)
            .saturating_add(padding.x.saturating_mul(2))
            .max(1);
        let height = lines
            .saturating_mul(cell_height)
            .saturating_add(padding.y.saturating_mul(2))
            .max(1);
        let mut background_pixel = pixel_format.pixel(crate::render::rgb(background));
        if argb {
            background_pixel |= (background_alpha as u32) << 24;
        }
        let event_mask = EventMask::EXPOSURE
            | EventMask::KEY_PRESS
            | EventMask::KEY_RELEASE
            | EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::POINTER_MOTION
            | EventMask::STRUCTURE_NOTIFY
            | EventMask::FOCUS_CHANGE
            | EventMask::PROPERTY_CHANGE
            | EventMask::VISIBILITY_CHANGE
            | EventMask::ENTER_WINDOW
            | EventMask::LEAVE_WINDOW;
        let mut attributes = xproto::CreateWindowAux::new()
            .background_pixel(background_pixel)
            .border_pixel(0)
            .event_mask(event_mask);
        if let Some(colormap) = colormap {
            attributes.colormap = Some(colormap);
        }
        connection
            .create_window(
                depth,
                window,
                root,
                0,
                0,
                width,
                height,
                0,
                WindowClass::INPUT_OUTPUT,
                visual_id,
                &attributes,
            )
            .map_err(|error| error.to_string())?;
        connection
            .create_gc(gc, window, &xproto::CreateGCAux::new())
            .map_err(|error| error.to_string())?;

        connection
            .change_property8(
                PropMode::REPLACE,
                window,
                xproto::AtomEnum::WM_NAME,
                xproto::AtomEnum::STRING,
                title.as_bytes(),
            )
            .map_err(|error| error.to_string())?;
        connection
            .change_property8(
                PropMode::REPLACE,
                window,
                atoms.net_wm_name,
                atoms.utf8_string,
                title.as_bytes(),
            )
            .map_err(|error| error.to_string())?;
        let mut class_name = Vec::new();
        class_name.extend_from_slice(instance.as_bytes());
        class_name.push(0);
        class_name.extend_from_slice(class.as_bytes());
        class_name.push(0);
        connection
            .change_property8(
                PropMode::REPLACE,
                window,
                xproto::AtomEnum::WM_CLASS,
                xproto::AtomEnum::STRING,
                &class_name,
            )
            .map_err(|error| error.to_string())?;
        let pid = std::process::id();
        connection
            .change_property32(
                PropMode::REPLACE,
                window,
                atoms.net_wm_pid,
                xproto::AtomEnum::CARDINAL,
                &[pid],
            )
            .map_err(|error| error.to_string())?;
        connection
            .change_property32(
                PropMode::REPLACE,
                window,
                atoms.wm_protocols,
                xproto::AtomEnum::ATOM,
                &[atoms.wm_delete_window],
            )
            .map_err(|error| error.to_string())?;
        if !argb && window_config.opacity < 1.0 {
            let opacity = (window_config.opacity.clamp(0.0, 1.0) * u32::MAX as f32) as u32;
            connection
                .change_property32(
                    PropMode::REPLACE,
                    window,
                    atoms.net_wm_window_opacity,
                    xproto::AtomEnum::CARDINAL,
                    &[opacity],
                )
                .map_err(|error| error.to_string())?;
        }
        if window_config.decorations.eq_ignore_ascii_case("none") {
            connection
                .change_property32(
                    PropMode::REPLACE,
                    window,
                    atoms.motif_wm_hints,
                    atoms.motif_wm_hints,
                    &[2, 0, 0, 0, 0],
                )
                .map_err(|error| error.to_string())?;
        }
        match window_config.startup_mode.to_ascii_lowercase().as_str() {
            "maximized" => {
                connection
                    .change_property32(
                        PropMode::REPLACE,
                        window,
                        atoms.net_wm_state,
                        xproto::AtomEnum::ATOM,
                        &[
                            atoms.net_wm_state_maximized_vert,
                            atoms.net_wm_state_maximized_horz,
                        ],
                    )
                    .map_err(|error| error.to_string())?;
            }
            "fullscreen" => {
                connection
                    .change_property32(
                        PropMode::REPLACE,
                        window,
                        atoms.net_wm_state,
                        xproto::AtomEnum::ATOM,
                        &[atoms.net_wm_state_fullscreen],
                    )
                    .map_err(|error| error.to_string())?;
            }
            _ => {}
        }

        let hints = size_hints(
            padding,
            cell_width,
            cell_height,
            scrollbar::width(cell_width),
        );
        hints
            .set_normal_hints(&connection, window)
            .map_err(|error| error.to_string())?;
        connection
            .map_window(window)
            .map_err(|error| error.to_string())?;
        connection.flush().map_err(|error| error.to_string())?;

        let keymap = Keymap::new(&connection)?;
        let xkb = connection
            .extension_information(xkb::X11_EXTENSION_NAME)
            .ok()
            .flatten()
            .is_some()
            && connection
                .xkb_use_extension(1, 0)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .is_some();

        let shm = if connection
            .extension_information(shm::X11_EXTENSION_NAME)
            .ok()
            .flatten()
            .is_some()
        {
            connection
                .shm_query_version()
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .is_some()
        } else {
            false
        };

        Ok(Self {
            connection,
            root,
            window,
            gc,
            atoms,
            pixel_format,
            depth,
            colormap,
            argb,
            background_alpha,
            width,
            height,
            keymap,
            xkb,
            shm: None,
            shm_failed: !shm,
            clipboard: Clipboard::default(),
            last_time: 0,
            fullscreen: window_config
                .startup_mode
                .eq_ignore_ascii_case("fullscreen"),
        })
    }

    pub fn fd(&self) -> std::os::fd::RawFd {
        self.connection.stream().as_raw_fd()
    }

    pub fn window_id(&self) -> u32 {
        self.window
    }

    pub fn is_argb(&self) -> bool {
        self.argb
    }

    pub fn background_alpha(&self) -> u8 {
        self.background_alpha
    }

    pub fn set_opacity(&mut self, opacity: f32) -> Result<(), String> {
        let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        self.background_alpha = alpha;
        if self.argb {
            return Ok(());
        }
        if opacity < 1.0 {
            let value = (opacity.clamp(0.0, 1.0) * u32::MAX as f32) as u32;
            self.connection
                .change_property32(
                    PropMode::REPLACE,
                    self.window,
                    self.atoms.net_wm_window_opacity,
                    xproto::AtomEnum::CARDINAL,
                    &[value],
                )
                .map_err(|error| error.to_string())?;
        } else {
            self.connection
                .delete_property(self.window, self.atoms.net_wm_window_opacity)
                .map_err(|error| error.to_string())?;
        }
        self.connection.flush().map_err(|error| error.to_string())
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    /// Duration of one display frame, from the active RandR mode's dot clock.
    /// Falls back to 16 ms when RandR is unavailable or reports no usable mode.
    pub fn frame_interval(&self) -> std::time::Duration {
        const FALLBACK: std::time::Duration = std::time::Duration::from_millis(16);
        let resources = match self
            .connection
            .randr_get_screen_resources_current(self.root)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        {
            Some(reply) => reply,
            None => return FALLBACK,
        };
        let mut active: Vec<_> = resources
            .crtcs
            .iter()
            .filter_map(|crtc| {
                self.connection
                    .randr_get_crtc_info(*crtc, resources.config_timestamp)
                    .ok()
                    .and_then(|cookie| cookie.reply().ok())
            })
            .filter(|info| info.width > 0 && info.height > 0)
            .collect();
        let origin = self
            .connection
            .translate_coordinates(self.window, self.root, 0, 0)
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        if let Some(origin) = origin {
            let x = origin.dst_x as i32 + self.width as i32 / 2;
            let y = origin.dst_y as i32 + self.height as i32 / 2;
            active.sort_by_key(|info| {
                let inside = x >= info.x as i32
                    && x < info.x as i32 + info.width as i32
                    && y >= info.y as i32
                    && y < info.y as i32 + info.height as i32;
                std::cmp::Reverse(inside)
            });
        }
        for info in active {
            if let Some(mode) = resources.modes.iter().find(|mode| mode.id == info.mode) {
                let total = mode.htotal as u64 * mode.vtotal as u64;
                if total > 0 && mode.dot_clock > 0 {
                    let micros = total * 1_000_000 / mode.dot_clock as u64;
                    let interval = std::time::Duration::from_micros(micros);
                    if interval >= std::time::Duration::from_millis(1)
                        && interval <= std::time::Duration::from_millis(100)
                    {
                        return interval;
                    }
                }
            }
        }
        FALLBACK
    }

    pub fn time(&self) -> u32 {
        self.last_time
    }

    pub fn set_title(&self, title: &str) -> Result<(), String> {
        self.connection
            .change_property8(
                PropMode::REPLACE,
                self.window,
                xproto::AtomEnum::WM_NAME,
                xproto::AtomEnum::STRING,
                title.as_bytes(),
            )
            .map_err(|error| error.to_string())?;
        self.connection
            .change_property8(
                PropMode::REPLACE,
                self.window,
                self.atoms.net_wm_name,
                self.atoms.utf8_string,
                title.as_bytes(),
            )
            .map_err(|error| error.to_string())?;
        self.connection.flush().map_err(|error| error.to_string())
    }

    pub fn set_size_hints(
        &self,
        padding: Padding,
        cell_width: u16,
        cell_height: u16,
        gutter: u16,
    ) -> Result<(), String> {
        size_hints(padding, cell_width, cell_height, gutter)
            .set_normal_hints(&self.connection, self.window)
            .map_err(|error| error.to_string())?;
        self.connection.flush().map_err(|error| error.to_string())
    }

    pub fn grab_pointer(&self) {
        let mask = EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION;
        let _ = self.connection.grab_pointer(
            false,
            self.window,
            mask,
            xproto::GrabMode::ASYNC,
            xproto::GrabMode::ASYNC,
            0u32,
            0u32,
            0u32,
        );
        let _ = self.connection.flush();
    }

    pub fn ungrab_pointer(&self) {
        let _ = self.connection.ungrab_pointer(0u32);
        let _ = self.connection.flush();
    }

    pub fn toggle_fullscreen(&mut self) -> Result<(), String> {
        let action = if self.fullscreen { 0 } else { 1 };
        self.fullscreen = !self.fullscreen;
        let event = ClientMessageEvent {
            response_type: xproto::CLIENT_MESSAGE_EVENT,
            format: 32,
            sequence: 0,
            window: self.window,
            type_: self.atoms.net_wm_state,
            data: [action, self.atoms.net_wm_state_fullscreen, 0, 1, 0].into(),
        };
        self.connection
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )
            .map_err(|error| error.to_string())?;
        self.connection.flush().map_err(|error| error.to_string())
    }

    pub fn bell(&self) {
        let _ = self.connection.bell(0);
        let _ = self.connection.flush();
    }

    pub fn resize_window(&self, width: u16, height: u16) -> Result<(), String> {
        self.connection
            .configure_window(
                self.window,
                &xproto::ConfigureWindowAux::new()
                    .width(width as u32)
                    .height(height as u32),
            )
            .map_err(|error| error.to_string())?;
        self.connection.flush().map_err(|error| error.to_string())
    }

    /// Presents damaged rows of the canvas. Damage is measured in terminal
    /// rows; `cell_height` and `origin_y` translate it into pixel rows.
    pub fn present(
        &mut self,
        canvas: &Canvas,
        damage: &crate::vt::Damage,
        cell_height: u16,
        origin_y: i32,
    ) {
        let (first, height) = if damage.all {
            (0u16, canvas.height)
        } else {
            let Some((first, last)) = damage.range() else {
                return;
            };
            let top = origin_y + first as i32 * cell_height as i32;
            let bottom = origin_y + (last as i32 + 1) * cell_height as i32;
            let top = top.clamp(0, canvas.height as i32) as u16;
            let bottom = bottom.clamp(0, canvas.height as i32) as u16;
            (top, bottom.saturating_sub(top))
        };
        if height == 0 || canvas.width == 0 {
            return;
        }
        self.ensure_shm(canvas.width, canvas.height);
        if !self.shm_failed {
            if let Some(segment) = &mut self.shm {
                let row_bytes = canvas.width as usize * 4;
                let offset = first as usize * row_bytes;
                let length = height as usize * row_bytes;
                if offset + length <= segment.length {
                    let start = first as usize * canvas.width as usize;
                    let end = (first as usize + height as usize) * canvas.width as usize;
                    let destination = &mut segment.bytes()[offset..offset + length];
                    Self::write_pixels(self.pixel_format, &canvas.pixels[start..end], destination);
                    if self
                        .connection
                        .shm_put_image(
                            self.window,
                            self.gc,
                            canvas.width,
                            canvas.height,
                            0,
                            first,
                            canvas.width,
                            height,
                            0,
                            first as i16,
                            self.depth,
                            xproto::ImageFormat::Z_PIXMAP.into(),
                            false,
                            segment.id,
                            0,
                        )
                        .is_ok()
                    {
                        let _ = self.connection.flush();
                        return;
                    }
                }
            }
        }
        self.present_slow(canvas, first, height);
    }

    fn write_pixels(format: PixelFormat, source: &[u32], destination: &mut [u8]) {
        if format.standard {
            // SAFETY: both slices have the same byte length.
            let pixels = unsafe {
                std::slice::from_raw_parts_mut(destination.as_mut_ptr() as *mut u32, source.len())
            };
            pixels.copy_from_slice(source);
        } else {
            for (index, pixel) in source.iter().enumerate() {
                let alpha = *pixel & 0xff00_0000;
                let pixel = format.pixel(*pixel) | alpha;
                destination[index * 4..index * 4 + 4].copy_from_slice(&pixel.to_ne_bytes());
            }
        }
    }

    fn present_slow(&mut self, canvas: &Canvas, first: u16, height: u16) {
        let start = first as usize * canvas.width as usize;
        let end = (first as usize + height as usize) * canvas.width as usize;
        let mut bytes = vec![0; (end - start) * 4];
        Self::write_pixels(self.pixel_format, &canvas.pixels[start..end], &mut bytes);
        let result = self.connection.put_image(
            xproto::ImageFormat::Z_PIXMAP,
            self.window,
            self.gc,
            canvas.width,
            height,
            0,
            first as i16,
            0,
            self.depth,
            &bytes,
        );
        if result.is_ok() {
            let _ = self.connection.flush();
        }
    }

    fn ensure_shm(&mut self, width: u16, height: u16) {
        let needed = width as usize * height as usize * 4;
        if self
            .shm
            .as_ref()
            .is_some_and(|segment| segment.length >= needed)
        {
            return;
        }
        self.destroy_shm();
        if self.shm_failed {
            return;
        }
        match ShmSegment::new(&self.connection, needed) {
            Some(segment) => self.shm = Some(segment),
            None => self.shm_failed = true,
        }
    }

    fn destroy_shm(&mut self) {
        if let Some(mut segment) = self.shm.take() {
            segment.destroy(&self.connection);
        }
    }

    pub fn set_size(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        if self
            .shm
            .as_ref()
            .is_some_and(|segment| segment.length < width as usize * height as usize * 4)
        {
            self.destroy_shm();
        }
    }

    pub fn poll(&mut self) -> Result<Vec<UiEvent>, String> {
        let mut events = Vec::new();
        loop {
            let event = self
                .connection
                .poll_for_event()
                .map_err(|error| error.to_string())?;
            let Some(event) = event else {
                break;
            };
            self.handle_event(event, &mut events);
        }
        Ok(events)
    }

    fn handle_event(&mut self, event: Event, output: &mut Vec<UiEvent>) {
        match event {
            Event::KeyPress(event) => {
                self.last_time = event.time;
                let mods = mods_from_mask(event.state);
                let keysym = self.key_keysym(event.detail, event.state);
                output.push(UiEvent::Key {
                    keysym,
                    mods,
                    time: event.time,
                });
            }
            Event::KeyRelease(_) => {}
            Event::ButtonPress(event) => {
                self.last_time = event.time;
                output.push(UiEvent::Button {
                    button: event.detail,
                    pressed: true,
                    x: event.event_x,
                    y: event.event_y,
                    time: event.time,
                    mods: mods_from_mask(event.state),
                    state: u16::from(event.state),
                });
            }
            Event::ButtonRelease(event) => {
                self.last_time = event.time;
                output.push(UiEvent::Button {
                    button: event.detail,
                    pressed: false,
                    x: event.event_x,
                    y: event.event_y,
                    time: event.time,
                    mods: mods_from_mask(event.state),
                    state: u16::from(event.state),
                });
            }
            Event::MotionNotify(event) => {
                self.last_time = event.time;
                output.push(UiEvent::Motion {
                    x: event.event_x,
                    y: event.event_y,
                    mods: mods_from_mask(event.state),
                    state: u16::from(event.state),
                });
            }
            Event::ConfigureNotify(event) => {
                if event.window == self.window
                    && (event.width != self.width || event.height != self.height)
                {
                    self.set_size(event.width, event.height);
                    output.push(UiEvent::Resize {
                        width: event.width,
                        height: event.height,
                    });
                }
            }
            Event::Expose(event) => {
                if event.window == self.window && event.count == 0 {
                    output.push(UiEvent::Expose);
                }
            }
            Event::FocusIn(event) => {
                if event.event == self.window {
                    output.push(UiEvent::Focus(true));
                }
            }
            Event::FocusOut(event) => {
                if event.event == self.window {
                    output.push(UiEvent::Focus(false));
                }
            }
            Event::MappingNotify(_) => {
                if let Ok(keymap) = Keymap::new(&self.connection) {
                    self.keymap = keymap;
                }
            }
            Event::ClientMessage(event) => {
                if event.window == self.window
                    && event.type_ == self.atoms.wm_protocols
                    && event.format == 32
                    && event.data.as_data32()[0] == self.atoms.wm_delete_window
                {
                    output.push(UiEvent::Close);
                }
            }
            Event::SelectionRequest(event) => {
                self.answer_selection(event);
            }
            Event::SelectionClear(event) => {
                if event.selection == self.atoms.clipboard {
                    self.clipboard.clipboard_text = None;
                } else if event.selection == self.atoms.primary {
                    self.clipboard.primary_text = None;
                }
            }
            Event::SelectionNotify(event) => {
                self.receive_selection(event, output);
            }
            Event::Error(error) => {
                output.push(UiEvent::Error(format!("X11 error: {error:?}")));
            }
            Event::PropertyNotify(_) => {}
            Event::VisibilityNotify(_) => {}
            Event::EnterNotify(_) => {}
            Event::LeaveNotify(_) => {}
            _ => {}
        }
    }

    /// Resolves a keycode to a keysym, consulting XKB for the active group.
    fn key_keysym(&self, keycode: u8, state: KeyButMask) -> u32 {
        let flags = u16::from(state);
        let shift = flags & u16::from(KeyButMask::SHIFT) != 0;
        let alt_gr = flags & u16::from(KeyButMask::MOD5) != 0;
        let group = if self.xkb && self.keymap.keysyms_per_keycode > 2 {
            self.connection
                .xkb_get_state(xkb::ID::USE_CORE_KBD.into())
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .map(|reply| u8::from(reply.group))
                .unwrap_or(0)
        } else {
            0
        };
        let mut keysym = self.keymap.keysym(keycode, group, shift, alt_gr);
        if keysym == 0 {
            keysym = self.keymap.keysym(keycode, 0, shift, false);
        }
        // XKB maps NumLock to the shifted level of keypad keys, but the core
        // mapping only indexes by Shift. Resolve the digit level explicitly.
        if flags & self.keymap.numlock_mask != 0 && Keysym::new(keysym).is_keypad_key() {
            let digits = self.keymap.keysym(keycode, group, true, alt_gr);
            if Keysym::new(digits).is_keypad_key() {
                keysym = digits;
            }
        }
        if flags & u16::from(KeyButMask::LOCK) != 0
            && let Some(ch) = Keysym::new(keysym).key_char()
            && ch.is_alphabetic()
        {
            let swapped = if ch.is_uppercase() {
                ch.to_lowercase().next()
            } else {
                ch.to_uppercase().next()
            };
            if let Some(swapped) = swapped {
                keysym = Keysym::from_char(swapped).raw();
            }
        }
        keysym
    }

    pub fn set_clipboard(
        &mut self,
        text: &str,
        primary: bool,
        clipboard: bool,
        time: u32,
    ) -> Result<(), String> {
        if primary {
            self.clipboard.primary_text = Some(text.to_string());
            self.connection
                .set_selection_owner(self.window, self.atoms.primary, time)
                .map_err(|error| error.to_string())?;
        }
        if clipboard {
            self.clipboard.clipboard_text = Some(text.to_string());
            self.connection
                .set_selection_owner(self.window, self.atoms.clipboard, time)
                .map_err(|error| error.to_string())?;
        }
        self.connection.flush().map_err(|error| error.to_string())
    }

    pub fn clipboard_text(&self, primary: bool) -> Option<&str> {
        if primary {
            self.clipboard.primary_text.as_deref()
        } else {
            self.clipboard.clipboard_text.as_deref()
        }
    }

    pub fn request_paste(&mut self, primary: bool, time: u32) -> Result<(), String> {
        let selection = if primary {
            self.atoms.primary
        } else {
            self.atoms.clipboard
        };
        self.clipboard.pending = Some(PendingPaste {
            selection: if primary { b'p' } else { b'c' },
            fallback: false,
        });
        self.connection
            .convert_selection(
                self.window,
                selection,
                self.atoms.utf8_string,
                self.atoms.reliabilitty_selection,
                time,
            )
            .map_err(|error| error.to_string())?;
        self.connection.flush().map_err(|error| error.to_string())
    }

    fn receive_selection(&mut self, event: SelectionNotifyEvent, output: &mut Vec<UiEvent>) {
        let Some(pending) = self.clipboard.pending.take() else {
            return;
        };
        if event.property == AtomEnum::NONE.into() {
            if !pending.fallback {
                let selection = if pending.selection == b'p' {
                    self.atoms.primary
                } else {
                    self.atoms.clipboard
                };
                self.clipboard.pending = Some(PendingPaste {
                    selection: pending.selection,
                    fallback: true,
                });
                let _ = self.connection.convert_selection(
                    self.window,
                    selection,
                    u32::from(xproto::AtomEnum::STRING),
                    self.atoms.reliabilitty_selection,
                    event.time,
                );
                let _ = self.connection.flush();
            }
            return;
        }
        let reply = self.connection.get_property(
            true,
            self.window,
            event.property,
            xproto::AtomEnum::ANY,
            0,
            4 * 1024 * 1024,
        );
        let Ok(cookie) = reply else {
            return;
        };
        let Ok(reply) = cookie.reply() else {
            return;
        };
        if reply.type_ == self.atoms.incr {
            // No INCR support; skip large transfers.
            let _ = self.connection.delete_property(self.window, event.property);
            return;
        }
        let text = String::from_utf8_lossy(&reply.value)
            .trim_end_matches('\0')
            .to_string();
        output.push(UiEvent::Paste(text));
    }

    fn answer_selection(&mut self, event: xproto::SelectionRequestEvent) {
        let text = if event.selection == self.atoms.clipboard {
            self.clipboard.clipboard_text.clone()
        } else if event.selection == self.atoms.primary {
            self.clipboard.primary_text.clone()
        } else {
            None
        };
        let Some(text) = text else {
            self.send_selection_notify(&event, 0);
            return;
        };
        let property = if event.property == 0 {
            event.target
        } else {
            event.property
        };
        let success = if event.target == self.atoms.targets {
            self.connection
                .change_property32(
                    PropMode::REPLACE,
                    event.requestor,
                    property,
                    xproto::AtomEnum::ATOM,
                    &[
                        self.atoms.targets,
                        self.atoms.utf8_string,
                        xproto::AtomEnum::STRING.into(),
                        self.atoms.text,
                        self.atoms.plain_utf8,
                    ],
                )
                .is_ok()
        } else if event.target == self.atoms.utf8_string
            || event.target == self.atoms.text
            || event.target == self.atoms.plain_utf8
            || event.target == u32::from(xproto::AtomEnum::STRING)
        {
            self.connection
                .change_property8(
                    PropMode::REPLACE,
                    event.requestor,
                    property,
                    event.target,
                    text.as_bytes(),
                )
                .is_ok()
        } else {
            false
        };
        if success {
            self.send_selection_notify(&event, property);
        } else {
            self.send_selection_notify(&event, 0);
        }
        let _ = self.connection.flush();
    }

    fn send_selection_notify(&self, request: &xproto::SelectionRequestEvent, property: Atom) {
        let event = SelectionNotifyEvent {
            response_type: xproto::SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property,
        };
        let _ = self
            .connection
            .send_event(false, request.requestor, EventMask::NO_EVENT, event);
    }

    pub fn shutdown(&mut self) {
        self.destroy_shm();
        let _ = self.connection.free_gc(self.gc);
        let _ = self.connection.destroy_window(self.window);
        if let Some(colormap) = self.colormap.take() {
            let _ = self.connection.free_colormap(colormap);
        }
        let _ = self.connection.flush();
    }
}

impl Drop for X11 {
    fn drop(&mut self) {
        self.destroy_shm();
    }
}

impl Atoms {
    fn intern(connection: &RustConnection) -> Result<Self, String> {
        let mut names: BTreeMap<&'static str, Atom> = BTreeMap::new();
        for name in [
            "WM_PROTOCOLS",
            "WM_DELETE_WINDOW",
            "_NET_WM_NAME",
            "_NET_WM_PID",
            "_NET_WM_WINDOW_OPACITY",
            "_NET_WM_STATE",
            "_NET_WM_STATE_FULLSCREEN",
            "_NET_WM_STATE_MAXIMIZED_VERT",
            "_NET_WM_STATE_MAXIMIZED_HORZ",
            "_MOTIF_WM_HINTS",
            "UTF8_STRING",
            "CLIPBOARD",
            "PRIMARY",
            "TARGETS",
            "TEXT",
            "text/plain;charset=utf-8",
            "INCR",
            "RELIABILITTY_SELECTION",
        ] {
            let atom = connection
                .intern_atom(false, name.as_bytes())
                .map_err(|error| error.to_string())?
                .reply()
                .map_err(|error| error.to_string())?
                .atom;
            names.insert(name, atom);
        }
        Ok(Self {
            wm_protocols: names["WM_PROTOCOLS"],
            wm_delete_window: names["WM_DELETE_WINDOW"],
            net_wm_name: names["_NET_WM_NAME"],
            net_wm_pid: names["_NET_WM_PID"],
            net_wm_window_opacity: names["_NET_WM_WINDOW_OPACITY"],
            net_wm_state: names["_NET_WM_STATE"],
            net_wm_state_fullscreen: names["_NET_WM_STATE_FULLSCREEN"],
            net_wm_state_maximized_vert: names["_NET_WM_STATE_MAXIMIZED_VERT"],
            net_wm_state_maximized_horz: names["_NET_WM_STATE_MAXIMIZED_HORZ"],
            motif_wm_hints: names["_MOTIF_WM_HINTS"],
            utf8_string: names["UTF8_STRING"],
            clipboard: names["CLIPBOARD"],
            primary: names["PRIMARY"],
            targets: names["TARGETS"],
            text: names["TEXT"],
            plain_utf8: names["text/plain;charset=utf-8"],
            incr: names["INCR"],
            reliabilitty_selection: names["RELIABILITTY_SELECTION"],
        })
    }
}

fn size_hints(padding: Padding, cell_width: u16, cell_height: u16, gutter: u16) -> WmSizeHints {
    let base_x = padding.x as i32 * 2 + gutter as i32;
    let base_y = padding.y as i32 * 2;
    WmSizeHints {
        base_size: Some((base_x, base_y)),
        size_increment: Some((cell_width as i32, cell_height as i32)),
        min_size: Some((base_x + cell_width as i32, base_y + cell_height as i32)),
        ..WmSizeHints::new()
    }
}

fn mods_from_mask(mask: KeyButMask) -> Mods {
    let flags = u16::from(mask);
    Mods {
        shift: flags & u16::from(KeyButMask::SHIFT) != 0,
        control: flags & u16::from(KeyButMask::CONTROL) != 0,
        alt: flags & u16::from(KeyButMask::MOD1) != 0,
        super_key: flags & u16::from(KeyButMask::MOD4) != 0,
    }
}
