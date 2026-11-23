//! Escape and control sequence dispatch.

use super::{Attrs, Cell, Color, MouseEncoding, MouseMode, Terminal, base64_decode};
use crate::config::{hex_nibble, parse_color};

impl Terminal {
    pub(super) fn set_indexed_fg(&mut self, index: u8) {
        self.pen.fg = Color::Indexed(index);
    }

    pub(super) fn set_indexed_bg(&mut self, index: u8) {
        self.pen.bg = Color::Indexed(index);
    }

    pub(super) fn esc_dispatch(&mut self, intermediates: &[u8], final_byte: u8) {
        match intermediates {
            [] => match final_byte {
                b'7' => self.save_cursor(),
                b'8' => self.restore_cursor(),
                b'D' => self.index(),
                b'E' => {
                    self.set_cursor(0, self.cursor.y);
                    self.index();
                }
                b'H' => self.set_tab_stop(),
                b'M' => self.reverse_index(),
                b'N' => self.single_shift = Some(2),
                b'O' => self.single_shift = Some(3),
                b'Z' => self.response.extend_from_slice(b"\x1b[?1;2c"),
                b'c' => self.hard_reset(),
                b'n' => self.active_charset = 2,
                b'o' => self.active_charset = 3,
                b'=' => self.app_keypad = true,
                b'>' => self.app_keypad = false,
                b'\\' => {}
                _ => {}
            },
            [b'#'] => {
                if final_byte == b'8' {
                    let attrs = Attrs::default();
                    let (fg, bg) = (self.pen.fg, self.pen.bg);
                    let (rows, cols) = (self.rows, self.cols);
                    for y in 0..rows {
                        let line = &mut self.lines_mut()[y];
                        for cell in line.cells.iter_mut().take(cols) {
                            *cell = Cell {
                                ch: 'E',
                                meta: 0,
                                fg,
                                bg,
                                attrs,
                                width: 1,
                            };
                        }
                        line.mark_all_written();
                    }
                    self.mark_all();
                }
            }
            [designator @ (b'(' | b')' | b'*' | b'+')] => {
                let index = match designator {
                    b'(' => 0,
                    b')' => 1,
                    b'*' => 2,
                    _ => 3,
                };
                self.charsets[index] = match final_byte {
                    b'0' => super::Charset::Graphics,
                    b'A' => super::Charset::UnitedKingdom,
                    _ => super::Charset::Ascii,
                };
            }
            _ => {}
        }
    }

    pub(super) fn csi_dispatch(
        &mut self,
        private: u8,
        intermediates: &[u8],
        params: &[u32],
        separators: &[u8],
        final_byte: u8,
    ) {
        if private == b'?' && intermediates == b"$" && final_byte == b'p' {
            self.report_mode(param(params, 0, 0));
            return;
        }
        if intermediates == b"!" && final_byte == b'p' {
            self.soft_reset();
            return;
        }
        if intermediates == b" " && final_byte == b'q' {
            self.set_cursor_style(param(params, 0, 0));
            return;
        }
        if intermediates == b"$" && final_byte == b'q' {
            // Silence is a valid DECRQSS answer.
            return;
        }
        if final_byte == b'u' && intermediates.is_empty() {
            match private {
                b'=' => {
                    self.set_kitty_flags(param(params, 0, 0), param(params, 1, 1));
                    return;
                }
                b'>' => {
                    self.push_kitty_flags(param(params, 0, 0));
                    return;
                }
                b'<' => {
                    self.pop_kitty_flags(param(params, 0, 1).max(1) as usize);
                    return;
                }
                b'?' => {
                    self.answer_kitty_flags();
                    return;
                }
                _ => {}
            }
        }
        if private == b'>' && final_byte == b'c' {
            self.response.extend_from_slice(b"\x1b[>0;10;1c");
            return;
        }
        if private == b'=' && final_byte == b'c' {
            self.response.extend_from_slice(b"\x1bP!|00000000\x1b\\");
            return;
        }

        match final_byte {
            b'@' => self.insert_chars(param(params, 0, 1)),
            b'A' => self.move_up(param(params, 0, 1)),
            b'B' => self.move_down(param(params, 0, 1)),
            b'C' => self.move_right(param(params, 0, 1)),
            b'D' => self.move_left(param(params, 0, 1)),
            b'E' => {
                self.move_down(param(params, 0, 1));
                self.set_cursor(0, self.cursor.y);
            }
            b'F' => {
                self.move_up(param(params, 0, 1));
                self.set_cursor(0, self.cursor.y);
            }
            b'G' | b'`' => {
                let x = param(params, 0, 1).saturating_sub(1) as usize;
                self.set_cursor(x, self.cursor.y);
            }
            b'H' | b'f' => self.cursor_position(param(params, 0, 1), param(params, 1, 1)),
            b'I' => self.tab_forward(param(params, 0, 1) as usize),
            b'J' => self.erase_display(param(params, 0, 0)),
            b'K' => self.erase_line(param(params, 0, 0)),
            b'L' => self.insert_lines(param(params, 0, 1) as usize),
            b'M' => self.delete_lines(param(params, 0, 1) as usize),
            b'P' => self.delete_chars(param(params, 0, 1)),
            b'S' => self.scroll_region_up(
                self.scroll_top,
                self.scroll_bottom,
                param(params, 0, 1) as usize,
                true,
            ),
            b'T' => {
                if params.len() <= 1 {
                    self.scroll_region_down(
                        self.scroll_top,
                        self.scroll_bottom,
                        param(params, 0, 1) as usize,
                    );
                }
            }
            b'X' => self.erase_chars(param(params, 0, 1)),
            b'Z' => self.tab_backward(param(params, 0, 1) as usize),
            b'a' => {
                let x = self.cursor.x.saturating_add(param(params, 0, 1) as usize);
                self.set_cursor(x, self.cursor.y);
            }
            b'b' => {
                let count = param(params, 0, 1).min(1000);
                let ch = self.last_graphic;
                for _ in 0..count {
                    self.print(ch);
                }
            }
            b'c' => {
                if private == 0 {
                    self.response.extend_from_slice(b"\x1b[?62;22c");
                }
            }
            b'd' => {
                let y = param(params, 0, 1).saturating_sub(1) as usize;
                self.set_cursor(self.cursor.x, y);
            }
            b'e' => {
                let y = self.cursor.y.saturating_add(param(params, 0, 1) as usize);
                self.set_cursor(self.cursor.x, y);
            }
            b'g' => match param(params, 0, 0) {
                0 => {
                    if let Some(tab) = self.tabs.get_mut(self.cursor.x) {
                        *tab = false;
                    }
                }
                3 => self.tabs.iter_mut().for_each(|tab| *tab = false),
                _ => {}
            },
            b'h' => self.set_modes(params, private, true),
            b'l' => self.set_modes(params, private, false),
            b'm' => self.sgr(params, separators),
            b'n' => self.device_status(private, param(params, 0, 0)),
            b'q' => self.set_cursor_style(param(params, 0, 0)),
            b'r' => self.set_scroll_region(param(params, 0, 1), param(params, 1, self.rows as u32)),
            b's' => self.save_cursor(),
            b't' => self.window_operation(params),
            b'u' => self.restore_cursor(),
            _ => {}
        }
    }

    pub(super) fn osc_dispatch(&mut self, data: &[u8]) {
        let Some(separator) = data.iter().position(|byte| *byte == b';') else {
            return;
        };
        let code = &data[..separator];
        let rest = &data[separator + 1..];
        match code {
            b"0" | b"1" | b"2" => {
                let title = String::from_utf8_lossy(rest);
                let title: String = title
                    .chars()
                    .filter(|ch| !ch.is_control())
                    .take(1024)
                    .collect();
                if title != self.title {
                    self.title = title;
                    self.title_dirty = true;
                }
            }
            b"4" => self.osc_palette(rest),
            b"8" => {
                let target = rest
                    .iter()
                    .position(|byte| *byte == b';')
                    .map_or(&[][..], |index| &rest[index + 1..]);
                if target.is_empty() {
                    self.clear_link();
                } else {
                    let target = String::from_utf8_lossy(target);
                    if target.chars().any(char::is_control) {
                        self.clear_link();
                    } else {
                        self.set_link(&target);
                    }
                }
            }
            b"10" => self.osc_default_color(rest, 0),
            b"11" => self.osc_default_color(rest, 1),
            b"12" => self.osc_default_color(rest, 2),
            b"52" => self.osc_clipboard(rest),
            b"104" => {
                let mut indices: Vec<u8> = rest
                    .split(|byte| *byte == b';')
                    .filter_map(|value| std::str::from_utf8(value).ok()?.parse().ok())
                    .collect();
                if indices.is_empty() {
                    indices.extend(0..=255);
                }
                for index in indices {
                    self.theme.palette[index as usize] = self.default_theme.palette[index as usize];
                }
                self.mark_all();
            }
            b"110" => {
                self.theme.foreground = self.default_theme.foreground;
                self.mark_all();
            }
            b"111" => {
                self.theme.background = self.default_theme.background;
                self.mark_all();
            }
            b"112" => {
                self.theme.cursor = self.default_theme.cursor;
                self.mark_all();
            }
            _ => {}
        }
    }

    pub(super) fn dcs_dispatch(&mut self, data: &[u8]) {
        if let Some(query) = data.strip_prefix(b"+q") {
            let mut replies = Vec::new();
            for name in query.split(|byte| *byte == b';') {
                let Some(name) = decode_hex(name) else {
                    continue;
                };
                let Some(name) = String::from_utf8(name).ok() else {
                    continue;
                };
                let value = match name.as_str() {
                    "TN" => Some("xterm-256color"),
                    "Co" => Some("256"),
                    "RGB" => Some("8/8/8"),
                    "Tc" => Some("1"),
                    "colors" => Some("256"),
                    "name" => Some("Reliabilitty"),
                    _ => None,
                };
                let name_hex = encode_hex(name.as_bytes());
                match value {
                    Some(value) => {
                        let value_hex = encode_hex(value.as_bytes());
                        replies.push(format!("{name_hex}={value_hex}"));
                    }
                    None => replies.push(format!("0+{name_hex}")),
                }
            }
            if !replies.is_empty() {
                let payload = replies.join(";");
                self.response
                    .extend_from_slice(format!("\x1bP1+r{payload}\x1b\\").as_bytes());
            }
        }
    }

    fn set_modes(&mut self, params: &[u32], private: u8, enable: bool) {
        for value in params {
            if private == b'?' {
                self.set_private_mode(*value, enable);
            } else {
                self.set_mode(*value, enable);
            }
        }
    }

    fn set_mouse_mode(&mut self, mode: MouseMode, enable: bool) {
        if enable {
            self.mouse_mode = mode;
        } else if self.mouse_mode == mode {
            self.mouse_mode = MouseMode::Off;
        }
    }

    fn set_mouse_encoding(&mut self, encoding: MouseEncoding, enable: bool) {
        if enable {
            self.mouse_encoding = encoding;
        } else if self.mouse_encoding == encoding {
            self.mouse_encoding = MouseEncoding::X10;
        }
    }

    fn set_private_mode(&mut self, mode: u32, enable: bool) {
        match mode {
            1 => self.app_cursor = enable,
            6 => {
                self.origin_mode = enable;
                self.set_cursor(0, 0);
            }
            7 => self.autowrap = enable,
            12 => {
                self.cursor.blinking = enable;
                self.mark_cursor_dirty();
            }
            25 => {
                self.cursor.visible = enable;
                self.mark_cursor_dirty();
            }
            47 => self.set_alternate(enable, false),
            66 => self.app_keypad = enable,
            1000 => self.set_mouse_mode(MouseMode::Button, enable),
            1002 => self.set_mouse_mode(MouseMode::ButtonEvent, enable),
            1003 => self.set_mouse_mode(MouseMode::AnyEvent, enable),
            1004 => self.focus_events = enable,
            1005 => self.set_mouse_encoding(MouseEncoding::Utf8, enable),
            1006 => self.set_mouse_encoding(MouseEncoding::Sgr, enable),
            1015 => self.set_mouse_encoding(MouseEncoding::Urxvt, enable),
            1047 => self.set_alternate(enable, false),
            1048 => {
                if enable {
                    self.save_cursor();
                } else {
                    self.restore_cursor();
                }
            }
            1049 => self.set_alternate(enable, true),
            2004 => self.bracketed_paste = enable,
            9 => self.set_mouse_mode(MouseMode::X10, enable),
            _ => {}
        }
    }

    fn set_mode(&mut self, mode: u32, enable: bool) {
        match mode {
            4 => self.insert_mode = enable,
            20 => self.newline_mode = enable,
            _ => {}
        }
    }

    fn report_mode(&mut self, mode: u32) {
        let state = match mode {
            1 => Some(self.app_cursor),
            6 => Some(self.origin_mode),
            7 => Some(self.autowrap),
            12 => Some(self.cursor.blinking),
            25 => Some(self.cursor.visible),
            47 | 1047 | 1049 => Some(self.in_alt()),
            1000 | 1002 | 1003 | 9 => Some(self.mouse_mode != MouseMode::Off),
            1004 => Some(self.focus_events),
            1006 => Some(self.mouse_encoding == MouseEncoding::Sgr),
            1015 => Some(self.mouse_encoding == MouseEncoding::Urxvt),
            1005 => Some(self.mouse_encoding == MouseEncoding::Utf8),
            2004 => Some(self.bracketed_paste),
            _ => None,
        };
        let value = match state {
            Some(true) => 1,
            Some(false) => 2,
            None => 0,
        };
        self.response
            .extend_from_slice(format!("\x1b[?{mode};{value}$y").as_bytes());
    }

    fn device_status(&mut self, private: u8, mode: u32) {
        match (private, mode) {
            (0, 5) => self.response.extend_from_slice(b"\x1b[0n"),
            (0, 6) => {
                let row = self.cursor.y + 1;
                let col = self.cursor.x + 1;
                self.response
                    .extend_from_slice(format!("\x1b[{row};{col}R").as_bytes());
            }
            (b'?', 6) => {
                let row = self.cursor.y + 1;
                let col = self.cursor.x + 1;
                self.response
                    .extend_from_slice(format!("\x1b[?{row};{col}R").as_bytes());
            }
            (b'?', 15) => self.response.extend_from_slice(b"\x1b[?13n"),
            (b'?', 25) => self.response.extend_from_slice(b"\x1b[?20n"),
            (b'?', 26) => self.response.extend_from_slice(b"\x1b[?27;1;0;0n"),
            _ => {}
        }
    }

    fn sgr(&mut self, params: &[u32], separators: &[u8]) {
        if params.is_empty() {
            self.pen = super::Pen::default();
            return;
        }
        let mut index = 0;
        while index < params.len() {
            let code = params[index];
            match code {
                0 => self.pen = super::Pen::default(),
                1 => self.pen.attrs.insert(Attrs::BOLD),
                2 => self.pen.attrs.insert(Attrs::DIM),
                3 => self.pen.attrs.insert(Attrs::ITALIC),
                4 => self.pen.attrs.insert(Attrs::UNDERLINE),
                5 | 6 => self.pen.attrs.insert(Attrs::BLINK),
                7 => self.pen.attrs.insert(Attrs::REVERSE),
                8 => self.pen.attrs.insert(Attrs::HIDDEN),
                9 => self.pen.attrs.insert(Attrs::STRIKETHROUGH),
                21 => self.pen.attrs.insert(Attrs::DOUBLE_UNDERLINE),
                22 => self.pen.attrs.remove(Attrs::BOLD.union(Attrs::DIM)),
                23 => self.pen.attrs.remove(Attrs::ITALIC),
                24 => self.pen.attrs.remove(
                    Attrs::UNDERLINE
                        .union(Attrs::DOUBLE_UNDERLINE)
                        .union(Attrs::CURLY_UNDERLINE)
                        .union(Attrs::DOTTED_UNDERLINE)
                        .union(Attrs::DASHED_UNDERLINE),
                ),
                25 => self.pen.attrs.remove(Attrs::BLINK),
                27 => self.pen.attrs.remove(Attrs::REVERSE),
                28 => self.pen.attrs.remove(Attrs::HIDDEN),
                29 => self.pen.attrs.remove(Attrs::STRIKETHROUGH),
                30..=37 => self.pen.fg = Color::Indexed((code - 30) as u8),
                38 => {
                    let (color, next) = extended_color(params, separators, index);
                    if let Some(color) = color {
                        self.pen.fg = color;
                    }
                    index = next;
                    continue;
                }
                39 => self.pen.fg = Color::Default,
                40..=47 => self.pen.bg = Color::Indexed((code - 40) as u8),
                48 => {
                    let (color, next) = extended_color(params, separators, index);
                    if let Some(color) = color {
                        self.pen.bg = color;
                    }
                    index = next;
                    continue;
                }
                49 => self.pen.bg = Color::Default,
                53 => self.pen.attrs.insert(Attrs::OVERLINE),
                55 => self.pen.attrs.remove(Attrs::OVERLINE),
                58 => {
                    let (_, next) = extended_color(params, separators, index);
                    index = next;
                    continue;
                }
                59 => {}
                90..=97 => self.pen.fg = Color::Indexed((code - 90 + 8) as u8),
                100..=107 => self.pen.bg = Color::Indexed((code - 100 + 8) as u8),
                _ => {}
            }
            index += 1;
        }
    }

    fn osc_palette(&mut self, rest: &[u8]) {
        let text = String::from_utf8_lossy(rest);
        let parts: Vec<&str> = text.split(';').collect();
        let mut index = 0;
        let mut changed = false;
        while index + 1 < parts.len() {
            let Ok(slot) = parts[index].parse::<u8>() else {
                index += 2;
                continue;
            };
            let value = parts[index + 1];
            if value == "?" {
                let color = self.theme.palette[slot as usize];
                self.response.extend_from_slice(
                    format!(
                        "\x1b]4;{slot};rgb:{:04x}/{:04x}/{:04x}\x1b\\",
                        color[0] as u16 * 257,
                        color[1] as u16 * 257,
                        color[2] as u16 * 257
                    )
                    .as_bytes(),
                );
            } else if let Ok(color) = parse_color(value) {
                self.theme.palette[slot as usize] = color;
                changed = true;
            }
            index += 2;
        }
        if changed {
            self.mark_all();
        }
    }

    fn osc_default_color(&mut self, rest: &[u8], which: u8) {
        let text = String::from_utf8_lossy(rest);
        let value = text.split(';').next().unwrap_or("");
        let current = match which {
            0 => self.theme.foreground,
            1 => self.theme.background,
            _ => self.theme.cursor,
        };
        if value == "?" {
            self.response.extend_from_slice(
                format!(
                    "\x1b]{};rgb:{:04x}/{:04x}/{:04x}\x1b\\",
                    10 + which,
                    current[0] as u16 * 257,
                    current[1] as u16 * 257,
                    current[2] as u16 * 257
                )
                .as_bytes(),
            );
        } else if let Ok(color) = parse_color(value) {
            match which {
                0 => self.theme.foreground = color,
                1 => self.theme.background = color,
                _ => self.theme.cursor = color,
            }
            self.mark_all();
        }
    }

    fn osc_clipboard(&mut self, rest: &[u8]) {
        let text = String::from_utf8_lossy(rest);
        let (selection, payload) = match text.split_once(';') {
            Some((selection, payload)) => (
                selection.as_bytes().first().copied().unwrap_or(b'c'),
                payload,
            ),
            None => return,
        };
        if payload == "?" {
            self.clipboard_read = Some(selection);
            return;
        }
        if payload.len() > 1_500_000 {
            return;
        }
        if let Some(bytes) = base64_decode(payload) {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            self.clipboard_writes
                .push(super::ClipboardWrite { selection, text });
        }
    }

    fn window_operation(&mut self, params: &[u32]) {
        let op = param(params, 0, 0);
        match op {
            14 => {
                self.response.extend_from_slice(
                    format!("\x1b[4;{};{}t", self.window_height, self.window_width).as_bytes(),
                );
            }
            16 => {
                self.response.extend_from_slice(
                    format!("\x1b[6;{};{}t", self.cell_height, self.cell_width).as_bytes(),
                );
            }
            18 => {
                self.response
                    .extend_from_slice(format!("\x1b[8;{};{}t", self.rows, self.cols).as_bytes());
            }
            8 => {
                let rows = param(params, 1, self.rows as u32).max(1) as usize;
                let cols = param(params, 2, self.cols as u32).max(1) as usize;
                self.resize_request = Some((cols, rows));
            }
            _ => {}
        }
    }

    fn set_scroll_region(&mut self, top: u32, bottom: u32) {
        let top = top.max(1) as usize - 1;
        let bottom = (bottom.max(1) as usize - 1).min(self.rows - 1);
        if top < bottom {
            self.normalize_region();
            self.region_offset = 0;
            self.scroll_top = top;
            self.scroll_bottom = bottom;
            let y = if self.origin_mode { top } else { 0 };
            self.set_cursor(0, y);
        }
    }

    fn set_cursor_style(&mut self, value: u32) {
        let (style, blinking) = match value {
            0 | 1 => (super::CursorStyle::Block, true),
            2 => (super::CursorStyle::Block, false),
            3 => (super::CursorStyle::Underline, true),
            4 => (super::CursorStyle::Underline, false),
            5 => (super::CursorStyle::Beam, true),
            6 => (super::CursorStyle::Beam, false),
            _ => return,
        };
        self.cursor.style = style;
        self.cursor.blinking = blinking;
        self.mark_cursor_dirty();
    }

    fn erase_display(&mut self, mode: u32) {
        match mode {
            0 => {
                self.erase_cells(self.cursor.y, self.cursor.x, self.cols - 1);
                for y in self.cursor.y + 1..self.rows {
                    self.clear_line(y);
                }
            }
            1 => {
                for y in 0..self.cursor.y {
                    self.clear_line(y);
                }
                self.erase_cells(self.cursor.y, 0, self.cursor.x);
            }
            2 => {
                for y in 0..self.rows {
                    self.clear_line(y);
                }
            }
            3 => {
                self.scrollback.clear();
                self.scroll_offset = 0;
                self.mark_all();
            }
            _ => {}
        }
        self.mark_all();
    }

    fn erase_line(&mut self, mode: u32) {
        match mode {
            0 => self.erase_cells(self.cursor.y, self.cursor.x, self.cols - 1),
            1 => self.erase_cells(self.cursor.y, 0, self.cursor.x),
            2 => self.clear_line(self.cursor.y),
            _ => {}
        }
    }

    fn erase_chars(&mut self, count: u32) {
        let count = count.max(1) as usize;
        let from = self.cursor.x;
        let to = (from + count - 1).min(self.cols - 1);
        self.erase_cells(self.cursor.y, from, to);
    }

    fn insert_chars(&mut self, count: u32) {
        self.insert_cells(count.max(1) as usize);
    }

    fn delete_chars(&mut self, count: u32) {
        self.delete_cells(count.max(1) as usize);
    }

    fn move_up(&mut self, count: u32) {
        let floor = if self.cursor.y >= self.scroll_top {
            self.scroll_top
        } else {
            0
        };
        let y = self
            .cursor
            .y
            .saturating_sub(count.max(1) as usize)
            .max(floor);
        self.set_cursor(self.cursor.x, y);
    }

    fn move_down(&mut self, count: u32) {
        let ceiling = if self.cursor.y <= self.scroll_bottom {
            self.scroll_bottom
        } else {
            self.rows - 1
        };
        let y = self
            .cursor
            .y
            .saturating_add(count.max(1) as usize)
            .min(ceiling);
        self.set_cursor(self.cursor.x, y);
    }

    fn move_right(&mut self, count: u32) {
        let x = self
            .cursor
            .x
            .saturating_add(count.max(1) as usize)
            .min(self.cols - 1);
        self.set_cursor(x, self.cursor.y);
    }

    fn move_left(&mut self, count: u32) {
        let x = self.cursor.x.saturating_sub(count.max(1) as usize);
        self.set_cursor(x, self.cursor.y);
    }

    fn cursor_position(&mut self, row: u32, col: u32) {
        let mut y = row.max(1) as usize - 1;
        if self.origin_mode {
            y = (y + self.scroll_top).min(self.scroll_bottom);
        } else {
            y = y.min(self.rows - 1);
        }
        let x = (col.max(1) as usize - 1).min(self.cols - 1);
        self.set_cursor(x, y);
    }
}

/// Returns a parameter, substituting `default` for zero or a missing value.
fn param(params: &[u32], index: usize, default: u32) -> u32 {
    match params.get(index).copied() {
        Some(0) | None => default,
        Some(value) => value,
    }
}

fn extended_color(params: &[u32], separators: &[u8], index: usize) -> (Option<Color>, usize) {
    let colon = separators.get(index + 1) == Some(&b':');
    if colon {
        let mut end = index + 1;
        while end + 1 < params.len() && separators.get(end + 1) == Some(&b':') {
            end += 1;
        }
        let group = &params[index + 1..=end];
        let color = if group.first() == Some(&5) {
            group.get(1).map(|value| Color::Indexed(*value as u8))
        } else if group.first() == Some(&2) && group.len() >= 4 {
            let rgb = &group[group.len() - 3..];
            Some(Color::Rgb([
                rgb[0].min(255) as u8,
                rgb[1].min(255) as u8,
                rgb[2].min(255) as u8,
            ]))
        } else {
            None
        };
        return (color, end + 1);
    }

    let mode = params.get(index + 1).copied().unwrap_or(0);
    match mode {
        5 => {
            let color = params
                .get(index + 2)
                .map(|value| Color::Indexed(*value as u8));
            (color, index + 3)
        }
        2 => {
            let color = if params.len() >= index + 5 {
                Some(Color::Rgb([
                    params[index + 2].min(255) as u8,
                    params[index + 3].min(255) as u8,
                    params[index + 4].min(255) as u8,
                ]))
            } else {
                None
            };
            (color, index + 5)
        }
        _ => (None, index + 2),
    }
}

fn decode_hex(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(data.len() / 2);
    for pair in data.chunks(2) {
        let high = hex_nibble(pair[0])?;
        let low = hex_nibble(pair[1])?;
        out.push((high << 4) | low);
    }
    Some(out)
}

fn encode_hex(data: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(data.len() * 2);
    for byte in data {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 15) as usize] as char);
    }
    out
}
