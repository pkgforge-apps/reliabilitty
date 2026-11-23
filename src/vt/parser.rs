//! Byte-at-a-time VT parser. Malformed input is discarded.

use super::Terminal;

const MAX_STRING: usize = 65536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    Osc,
    OscEsc,
    Dcs,
    DcsEsc,
    Sos,
    /// SOS, PM or APC after ESC, waiting for `\`.
    SosEsc,
}

#[derive(Clone, Copy)]
enum StringKind {
    Osc,
    Dcs,
    Ignore,
}

enum Decoded {
    Pending,
    Char(char),
    Invalid,
}

#[derive(Default)]
struct Utf8 {
    needed: u8,
    code: u32,
    minimum: u32,
}

impl Utf8 {
    fn feed(&mut self, byte: u8) -> Decoded {
        if self.needed == 0 {
            match byte {
                0x00..=0x7f => return Decoded::Char(byte as char),
                0xc2..=0xdf => {
                    self.needed = 1;
                    self.code = (byte & 0x1f) as u32;
                    self.minimum = 0x80;
                }
                0xe0..=0xef => {
                    self.needed = 2;
                    self.code = (byte & 0x0f) as u32;
                    self.minimum = 0x800;
                }
                0xf0..=0xf4 => {
                    self.needed = 3;
                    self.code = (byte & 0x07) as u32;
                    self.minimum = 0x10000;
                }
                _ => return Decoded::Invalid,
            }
            return Decoded::Pending;
        }
        if byte & 0xc0 != 0x80 {
            self.needed = 0;
            self.code = 0;
            return Decoded::Invalid;
        }
        self.code = (self.code << 6) | (byte & 0x3f) as u32;
        self.needed -= 1;
        if self.needed != 0 {
            return Decoded::Pending;
        }
        let code = std::mem::take(&mut self.code);
        if code < self.minimum || code > 0x10ffff || (0xd800..=0xdfff).contains(&code) {
            return Decoded::Invalid;
        }
        char::from_u32(code).map_or(Decoded::Invalid, Decoded::Char)
    }
}

/// Parses a plain `ESC [ <digits> (;<digits>)* <final>` CSI sequence without
/// going through the byte state machine, writing parsed parameters directly
/// into `params`/`seps`. Returns `(param_count, final_byte, bytes_consumed)`.
/// Anything with a private marker, intermediate, `:` subparameter, or more
/// than `params.len()` parameters returns `None` and falls back to the state
/// machine.
fn parse_plain_csi(
    bytes: &[u8],
    params: &mut [u32],
    seps: &mut [u8],
) -> Option<(usize, u8, usize)> {
    if bytes.len() < 3 || bytes[0] != 0x1b || bytes[1] != b'[' {
        return None;
    }
    let mut count = 0usize;
    let mut current = 0u32;
    let mut current_set = false;
    let mut current_sep = 0u8;
    let mut i = 2;
    while i < bytes.len() {
        match bytes[i] {
            b'0'..=b'9' => {
                current = current
                    .saturating_mul(10)
                    .saturating_add((bytes[i] - b'0') as u32)
                    .min(65535);
                current_set = true;
                i += 1;
            }
            b';' => {
                if count >= params.len() {
                    return None;
                }
                params[count] = current;
                seps[count] = current_sep;
                count += 1;
                current = 0;
                current_set = false;
                current_sep = b';';
                i += 1;
            }
            final_byte @ 0x40..=0x7e => {
                if current_set || current_sep != 0 || count != 0 {
                    if count >= params.len() {
                        return None;
                    }
                    params[count] = current;
                    seps[count] = current_sep;
                    count += 1;
                }
                return Some((count, final_byte, i + 1));
            }
            _ => return None,
        }
    }
    None
}

/// Fast-path for `CSI 38 ; 5 ; N m` and `CSI 48 ; 5 ; N m` (indexed color)
/// when already confirmed to start with `ESC [`.
fn parse_indexed_color(bytes: &[u8]) -> Option<(bool, u8, usize)> {
    if bytes.len() < 9 {
        return None;
    }
    let foreground = match (bytes[2], bytes[3]) {
        (b'3', b'8') => true,
        (b'4', b'8') => false,
        _ => return None,
    };
    if bytes[4] != b';' || bytes[5] != b'5' || bytes[6] != b';' {
        return None;
    }
    let mut value = 0u16;
    let mut i = 7;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        value = value * 10 + (bytes[i] - b'0') as u16;
        if value > 255 {
            return None;
        }
        i += 1;
    }
    if i == 7 || bytes.get(i) != Some(&b'm') {
        return None;
    }
    Some((foreground, value as u8, i + 1))
}

pub struct Parser {
    state: State,
    intermediates: [u8; 2],
    intermediate_len: usize,
    params: Vec<u32>,
    separators: Vec<u8>,
    current: u32,
    current_set: bool,
    current_separator: u8,
    private: u8,
    string: Vec<u8>,
    utf8: Utf8,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            intermediates: [0; 2],
            intermediate_len: 0,
            params: Vec::new(),
            separators: Vec::new(),
            current: 0,
            current_set: false,
            current_separator: 0,
            private: 0,
            string: Vec::new(),
            utf8: Utf8::default(),
        }
    }

    pub fn advance(&mut self, bytes: &[u8], terminal: &mut Terminal) {
        let mut csi_params = [0u32; 32];
        let mut csi_seps = [0u8; 32];
        let mut index = 0;
        while index < bytes.len() {
            if matches!(self.state, State::Ground) && self.utf8.needed == 0 {
                let b = bytes[index];
                if b >= 0x20 && b != 0x7f {
                    let start = index;
                    let mut ascii_only = b < 0x80;
                    index += 1;
                    while index < bytes.len() && bytes[index] >= 0x20 && bytes[index] != 0x7f {
                        ascii_only &= bytes[index] < 0x80;
                        index += 1;
                    }
                    if ascii_only {
                        terminal.print_ascii_run(&bytes[start..index]);
                        continue;
                    }
                    let text = match std::str::from_utf8(&bytes[start..index]) {
                        Ok(text) => text,
                        Err(error) => {
                            let valid = error.valid_up_to();
                            std::str::from_utf8(&bytes[start..start + valid]).unwrap_or("")
                        }
                    };
                    if !text.is_empty() {
                        terminal.print_text_run(text);
                        index = start + text.len();
                        continue;
                    }
                    index = start;
                } else if b == 0x1b && bytes.get(index + 1) == Some(&b'[') {
                    let slice = &bytes[index..];
                    if let Some((foreground, color, len)) = parse_indexed_color(slice) {
                        if foreground {
                            terminal.set_indexed_fg(color);
                        } else {
                            terminal.set_indexed_bg(color);
                        }
                        index += len;
                        continue;
                    }
                    if let Some((n, final_byte, len)) =
                        parse_plain_csi(slice, &mut csi_params, &mut csi_seps)
                    {
                        terminal.csi_dispatch(0, &[], &csi_params[..n], &csi_seps[..n], final_byte);
                        index += len;
                        continue;
                    }
                }
            }
            self.byte(bytes[index], terminal);
            index += 1;
        }
    }

    fn byte(&mut self, byte: u8, terminal: &mut Terminal) {
        match self.state {
            State::Ground => self.ground(byte, terminal),
            State::Escape => self.escape(byte, terminal),
            State::EscapeIntermediate => self.escape_intermediate(byte, terminal),
            State::CsiParam => self.csi_param(byte, terminal),
            State::CsiIntermediate => self.csi_intermediate(byte, terminal),
            State::CsiIgnore => self.csi_ignore(byte, terminal),
            State::Osc => self.osc(byte, terminal),
            State::OscEsc => self.osc_esc(byte, terminal),
            State::Dcs => self.dcs(byte, terminal),
            State::DcsEsc => self.dcs_esc(byte, terminal),
            State::Sos => self.sos(byte, terminal),
            State::SosEsc => self.sos_esc(byte, terminal),
        }
    }

    fn ground(&mut self, byte: u8, terminal: &mut Terminal) {
        if byte < 0x80 {
            match byte {
                0x1b => self.state = State::Escape,
                0x18 | 0x1a => {}
                0x00..=0x1f => terminal.execute_control(byte),
                0x7f => {}
                _ => terminal.print(byte as char),
            }
            return;
        }
        if self.utf8.needed == 0 && (0x80..=0x9f).contains(&byte) {
            self.c1(byte, terminal);
            return;
        }
        match self.utf8.feed(byte) {
            Decoded::Pending => {}
            Decoded::Char(ch) => terminal.print(ch),
            Decoded::Invalid => {
                terminal.print('\u{fffd}');
                self.utf8 = Utf8::default();
                if byte >= 0xc0 {
                    // Retry as a lead byte.
                    match self.utf8.feed(byte) {
                        Decoded::Char(ch) => terminal.print(ch),
                        Decoded::Invalid => self.utf8 = Utf8::default(),
                        Decoded::Pending => {}
                    }
                }
            }
        }
    }

    fn c1(&mut self, byte: u8, terminal: &mut Terminal) {
        match byte {
            0x84 | 0x85 | 0x88 | 0x8d | 0x8e | 0x8f => terminal.execute_control(byte),
            0x90 => {
                self.string.clear();
                self.state = State::Dcs;
            }
            0x98 | 0x9e | 0x9f => {
                self.string.clear();
                self.state = State::Sos;
            }
            0x9b => self.begin_csi(),
            0x9d => {
                self.string.clear();
                self.state = State::Osc;
            }
            _ => {}
        }
    }

    fn begin_csi(&mut self) {
        self.params.clear();
        self.separators.clear();
        self.current = 0;
        self.current_set = false;
        self.current_separator = 0;
        self.private = 0;
        self.intermediate_len = 0;
        self.state = State::CsiParam;
    }

    fn escape(&mut self, byte: u8, terminal: &mut Terminal) {
        match byte {
            0x1b => self.state = State::Escape,
            0x00..=0x1f => terminal.execute_control(byte),
            0x7f => {}
            b'[' => self.begin_csi(),
            b']' => {
                self.string.clear();
                self.state = State::Osc;
            }
            b'P' => {
                self.string.clear();
                self.state = State::Dcs;
            }
            b'X' | b'^' | b'_' => {
                self.string.clear();
                self.state = State::Sos;
            }
            0x20..=0x2f => {
                self.intermediate_len = 0;
                self.intermediates[self.intermediate_len] = byte;
                self.intermediate_len += 1;
                self.state = State::EscapeIntermediate;
            }
            0x30..=0x7e => {
                self.intermediate_len = 0;
                terminal.esc_dispatch(&[], byte);
                self.state = State::Ground;
            }
            _ => {
                self.state = State::Ground;
                self.ground(byte, terminal);
            }
        }
    }

    fn escape_intermediate(&mut self, byte: u8, terminal: &mut Terminal) {
        match byte {
            0x1b => self.state = State::Escape,
            0x00..=0x1f => terminal.execute_control(byte),
            0x7f => {}
            0x20..=0x2f => {
                if self.intermediate_len < self.intermediates.len() {
                    self.intermediates[self.intermediate_len] = byte;
                    self.intermediate_len += 1;
                }
            }
            0x30..=0x7e => {
                let intermediates = &self.intermediates[..self.intermediate_len];
                terminal.esc_dispatch(intermediates, byte);
                self.state = State::Ground;
            }
            _ => {
                self.state = State::Ground;
                self.ground(byte, terminal);
            }
        }
    }

    fn csi_param(&mut self, byte: u8, terminal: &mut Terminal) {
        match byte {
            0x1b => self.state = State::Escape,
            0x00..=0x1f => terminal.execute_control(byte),
            0x7f => {}
            b'0'..=b'9' => {
                self.current = self
                    .current
                    .saturating_mul(10)
                    .saturating_add((byte - b'0') as u32)
                    .min(65535);
                self.current_set = true;
            }
            b';' | b':' => {
                self.params.push(self.current);
                self.separators.push(self.current_separator);
                self.current = 0;
                self.current_set = false;
                self.current_separator = byte;
            }
            b'<'..=b'?' => {
                if self.params.is_empty()
                    && !self.current_set
                    && self.intermediate_len == 0
                    && self.private == 0
                {
                    self.private = byte;
                } else {
                    self.state = State::CsiIgnore;
                }
            }
            0x20..=0x2f => {
                if self.intermediate_len < self.intermediates.len() {
                    self.intermediates[self.intermediate_len] = byte;
                    self.intermediate_len += 1;
                    self.state = State::CsiIntermediate;
                } else {
                    self.state = State::CsiIgnore;
                }
            }
            0x40..=0x7e => {
                self.finish_param();
                self.dispatch_csi(terminal, byte);
                self.state = State::Ground;
            }
            _ => self.state = State::CsiIgnore,
        }
    }

    fn csi_intermediate(&mut self, byte: u8, terminal: &mut Terminal) {
        match byte {
            0x1b => self.state = State::Escape,
            0x00..=0x1f => terminal.execute_control(byte),
            0x7f => {}
            0x20..=0x2f => {
                if self.intermediate_len < self.intermediates.len() {
                    self.intermediates[self.intermediate_len] = byte;
                    self.intermediate_len += 1;
                } else {
                    self.state = State::CsiIgnore;
                }
            }
            0x40..=0x7e => {
                self.finish_param();
                self.dispatch_csi(terminal, byte);
                self.state = State::Ground;
            }
            0x30..=0x3f => self.state = State::CsiIgnore,
            _ => self.state = State::Ground,
        }
    }

    fn csi_ignore(&mut self, byte: u8, terminal: &mut Terminal) {
        match byte {
            0x1b => self.state = State::Escape,
            0x00..=0x1f => terminal.execute_control(byte),
            0x40..=0x7e => self.state = State::Ground,
            _ => {}
        }
    }

    fn finish_param(&mut self) {
        if self.current_set || self.current_separator != 0 || !self.params.is_empty() {
            self.params.push(self.current);
            self.separators.push(self.current_separator);
        }
        self.current = 0;
        self.current_set = false;
        self.current_separator = 0;
    }

    fn dispatch_csi(&mut self, terminal: &mut Terminal, final_byte: u8) {
        let intermediates = &self.intermediates[..self.intermediate_len];
        terminal.csi_dispatch(
            self.private,
            intermediates,
            &self.params,
            &self.separators,
            final_byte,
        );
        self.params.clear();
        self.separators.clear();
        self.private = 0;
        self.intermediate_len = 0;
    }

    fn collect_string(
        &mut self,
        byte: u8,
        terminal: &mut Terminal,
        kind: StringKind,
        escape_state: State,
    ) {
        match byte {
            0x1b => self.state = escape_state,
            0x9c => self.end_string(terminal, kind),
            _ => self.push_string_byte(byte),
        }
    }

    fn string_escape(
        &mut self,
        byte: u8,
        terminal: &mut Terminal,
        kind: StringKind,
        body_state: State,
    ) {
        match byte {
            b'\\' => self.end_string(terminal, kind),
            0x1b => {}
            _ => {
                self.push_string_byte(0x1b);
                self.push_string_byte(byte);
                self.state = body_state;
            }
        }
    }

    /// Appends a byte to the current escape string, with a size cap.
    fn push_string_byte(&mut self, byte: u8) {
        if self.string.len() < MAX_STRING {
            self.string.push(byte);
        }
    }

    fn end_string(&mut self, terminal: &mut Terminal, kind: StringKind) {
        match kind {
            StringKind::Osc => terminal.osc_dispatch(&self.string),
            StringKind::Dcs => terminal.dcs_dispatch(&self.string),
            StringKind::Ignore => {}
        }
        self.string.clear();
        self.state = State::Ground;
    }

    fn osc(&mut self, byte: u8, terminal: &mut Terminal) {
        if byte == 0x07 {
            self.end_string(terminal, StringKind::Osc);
        } else {
            self.collect_string(byte, terminal, StringKind::Osc, State::OscEsc);
        }
    }

    fn osc_esc(&mut self, byte: u8, terminal: &mut Terminal) {
        self.string_escape(byte, terminal, StringKind::Osc, State::Osc);
    }

    fn dcs(&mut self, byte: u8, terminal: &mut Terminal) {
        self.collect_string(byte, terminal, StringKind::Dcs, State::DcsEsc);
    }

    fn dcs_esc(&mut self, byte: u8, terminal: &mut Terminal) {
        self.string_escape(byte, terminal, StringKind::Dcs, State::Dcs);
    }

    fn sos(&mut self, byte: u8, terminal: &mut Terminal) {
        self.collect_string(byte, terminal, StringKind::Ignore, State::SosEsc);
    }

    fn sos_esc(&mut self, byte: u8, terminal: &mut Terminal) {
        self.string_escape(byte, terminal, StringKind::Ignore, State::Sos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use crate::vt::{CursorStyle, Terminal};

    fn terminal() -> Terminal {
        Terminal::new(
            20,
            4,
            100,
            Theme::from_config(&crate::config::ColorsConfig::default()).unwrap(),
            CursorStyle::Block,
            false,
        )
    }

    #[test]
    fn cursor_position_fast_path_matches_state_machine() {
        let mut fast = terminal();
        let mut slow = terminal();
        let mut a = Parser::new();
        let mut b = Parser::new();
        a.advance(b"\x1b[2;3HX", &mut fast);
        b.advance(b"\x1b[2;3H", &mut slow);
        b.advance(b"X", &mut slow);
        assert_eq!(
            (fast.cursor.x, fast.cursor.y),
            (slow.cursor.x, slow.cursor.y)
        );
        assert_eq!(fast.lines()[1].cells[2].ch, 'X');
        assert_eq!(fast.lines()[1].cells[2].ch, slow.lines()[1].cells[2].ch);
    }

    #[test]
    fn decodes_multibyte_sequences_in_one_advance() {
        let mut terminal = terminal();
        let mut parser = Parser::new();
        parser.advance("a\u{4e16}\u{1f600}".as_bytes(), &mut terminal);
        assert_eq!(terminal.lines()[0].cells[0].ch, 'a');
        assert_eq!(terminal.lines()[0].cells[1].ch, '\u{4e16}');
        assert_eq!(terminal.lines()[0].cells[3].ch, '\u{1f600}');
    }

    #[test]
    fn invalid_utf8_still_falls_back_to_replacement() {
        let mut terminal = terminal();
        let mut parser = Parser::new();
        parser.advance(&[0x61, 0xc0, 0x62], &mut terminal);
        assert_eq!(terminal.lines()[0].cells[0].ch, 'a');
        assert_eq!(terminal.lines()[0].cells[1].ch, '\u{fffd}');
        assert_eq!(terminal.lines()[0].cells[2].ch, 'b');
    }

    #[test]
    fn sequences_split_across_advances_are_decoded() {
        let mut terminal = terminal();
        let mut parser = Parser::new();
        let bytes = "\u{4e16}".as_bytes();
        parser.advance(&bytes[..1], &mut terminal);
        parser.advance(&bytes[1..], &mut terminal);
        assert_eq!(terminal.lines()[0].cells[0].ch, '\u{4e16}');
    }

    #[test]
    fn text_run_split_at_every_byte_boundary_matches_whole() {
        let text = "ab\u{e9}\u{4e16}\u{1f600}cd";
        let bytes = text.as_bytes();
        for split in 1..bytes.len() {
            let mut whole = terminal();
            let mut parser = Parser::new();
            parser.advance(bytes, &mut whole);

            let mut split_term = terminal();
            let mut split_parser = Parser::new();
            split_parser.advance(&bytes[..split], &mut split_term);
            split_parser.advance(&bytes[split..], &mut split_term);

            let cell_chars = |t: &Terminal| {
                (0..t.rows)
                    .flat_map(|r| t.lines()[r].cells.iter().map(|c| (c.ch, c.width)))
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                cell_chars(&split_term),
                cell_chars(&whole),
                "split at {split}"
            );
        }
    }

    #[test]
    fn parse_sgr_matches_state_machine() {
        let cases: &[&[u8]] = &[
            b"\x1b[m",
            b"\x1b[0m",
            b"\x1b[1;3;4m",
            b"\x1b[38;5;196;48;5;21;1;3;4m",
            b"\x1b[;1m",
            b"\x1b[1;m",
            b"\x1b[10;20;30;40;50;60m",
        ];
        for &bytes in cases {
            let mut params = [0u32; 32];
            let mut seps = [0u8; 32];
            let (n, final_byte, len) = parse_plain_csi(bytes, &mut params, &mut seps)
                .unwrap_or_else(|| panic!("no parse: {:?}", bytes));
            assert_eq!(final_byte, b'm');
            assert_eq!(len, bytes.len());
            // Reference: replicate the byte state machine's param building.
            let (mut rp, mut rs, mut cur, mut set, mut csep) =
                (Vec::new(), Vec::new(), 0u32, false, 0u8);
            for &b in &bytes[2..bytes.len() - 1] {
                match b {
                    b'0'..=b'9' => {
                        cur = cur
                            .saturating_mul(10)
                            .saturating_add((b - b'0') as u32)
                            .min(65535);
                        set = true;
                    }
                    b';' => {
                        rp.push(cur);
                        rs.push(csep);
                        cur = 0;
                        set = false;
                        csep = b';';
                    }
                    _ => {}
                }
            }
            if set || csep != 0 || !rp.is_empty() {
                rp.push(cur);
                rs.push(csep);
            }
            assert_eq!(&params[..n], rp.as_slice(), "params for {:?}", bytes);
            assert_eq!(&seps[..n], rs.as_slice(), "seps for {:?}", bytes);
        }
    }

    #[test]
    fn indexed_color_fast_path_matches_byte_path_pen() {
        for seq in [
            b"\x1b[38;5;196m".as_slice(),
            b"\x1b[48;5;21m",
            b"\x1b[38;5;0m",
            b"\x1b[48;5;255m",
        ] {
            let mut fast = terminal();
            Parser::new().advance(seq, &mut fast);
            let mut slow = terminal();
            let mut parser = Parser::new();
            for &byte in seq {
                parser.byte(byte, &mut slow);
            }
            assert_eq!(fast.pen.fg, slow.pen.fg, "foreground for {seq:?}");
            assert_eq!(fast.pen.bg, slow.pen.bg, "background for {seq:?}");
        }
    }

    #[test]
    fn sgr_fast_path_matches_byte_path_pen() {
        let seq = b"\x1b[38;5;196;48;5;21;1;3;4m";
        let mut a = terminal();
        let mut parser = Parser::new();
        parser.advance(seq, &mut a);
        let mut b = terminal();
        let mut parser2 = Parser::new();
        // Force the byte state machine by feeding one byte at a time.
        for &byte in seq {
            parser2.byte(byte, &mut b);
        }
        assert_eq!(a.pen.fg, b.pen.fg);
        assert_eq!(a.pen.bg, b.pen.bg);
        assert_eq!(a.pen.attrs, b.pen.attrs);
    }
}

#[cfg(test)]
mod fuzz_sgr {
    use super::*;
    use crate::theme::Theme;
    use crate::vt::{CursorStyle, Terminal};

    fn term() -> Terminal {
        let theme = Theme::from_config(&crate::config::ColorsConfig::default()).unwrap();
        Terminal::new(20, 4, 100, theme, CursorStyle::Block, true)
    }

    fn snap(t: &Terminal) -> String {
        let mut out = format!(
            "{:?}{:?}{:?}{} {}",
            t.pen.fg, t.pen.bg, t.pen.attrs, t.cursor.x, t.cursor.y
        );
        for row in 0..t.rows {
            for c in &t.lines()[row].cells {
                out.push_str(&format!(
                    "{}{:?}{:?}{:?}{}",
                    c.ch, c.fg, c.bg, c.attrs, c.width
                ));
            }
        }
        out
    }

    #[test]
    fn sgr_fast_path_fuzz_matches_byte_at_a_time() {
        let pieces: &[&[u8]] = &[
            b"\x1b[",
            b"\x1b[",
            b"0",
            b"1",
            b"3",
            b"4",
            b"5",
            b"8",
            b"9",
            b"2",
            b";",
            b";",
            b":",
            b"m",
            b"m",
            b"a",
            b"b",
            b"\x1b",
            b"[",
            b"?",
            b"\n",
            b"\r",
            b" ",
            b"6",
            b"7",
            b"\xc3\xa9",
            b"\xe4\xb8\xad",
            b"H",
            b"K",
            b"\x9b",
            b"9999999999",
        ];
        let mut seed = 0x1234_5678_9abc_def0u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for case in 0..20_000 {
            let mut bytes = Vec::new();
            for _ in 0..1 + next() % 24 {
                bytes.extend_from_slice(pieces[(next() % pieces.len() as u64) as usize]);
            }
            let mut whole = term();
            Parser::new().advance(&bytes, &mut whole);
            let mut single = term();
            let mut parser = Parser::new();
            for &b in &bytes {
                parser.advance(&[b], &mut single);
            }
            assert_eq!(snap(&whole), snap(&single), "case {case}: {bytes:?}");
            let cut = (next() % (bytes.len() as u64 + 1)) as usize;
            let mut split = term();
            let mut parser = Parser::new();
            parser.advance(&bytes[..cut], &mut split);
            parser.advance(&bytes[cut..], &mut split);
            assert_eq!(
                snap(&split),
                snap(&single),
                "split case {case} cut {cut}: {bytes:?}"
            );
        }
    }
}
