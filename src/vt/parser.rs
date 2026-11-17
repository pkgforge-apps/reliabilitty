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
        for &byte in bytes {
            self.byte(byte, terminal);
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
                let intermediates = self.intermediates[..self.intermediate_len].to_vec();
                terminal.esc_dispatch(&intermediates, byte);
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
        let intermediates = self.intermediates[..self.intermediate_len].to_vec();
        terminal.csi_dispatch(
            self.private,
            &intermediates,
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
