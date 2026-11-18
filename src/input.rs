//! Keyboard translation: X11 keysyms to terminal bytes, plus config bindings.

use unicode_normalization::UnicodeNormalization;
use xkeysym::{Keysym, key};

use crate::config::{KeyBinding, KeyboardConfig};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
}

impl Mods {
    fn parameter(self) -> u8 {
        1 + self.shift as u8
            + (self.alt as u8) * 2
            + (self.control as u8) * 4
            + (self.super_key as u8) * 8
    }

    fn matches(self, binding: &Binding) -> bool {
        self.shift == binding.shift
            && self.control == binding.control
            && self.alt == binding.alt
            && self.super_key == binding.super_key
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Copy,
    Paste,
    PasteSelection,
    ScrollPageUp,
    ScrollPageDown,
    ScrollLineUp,
    ScrollLineDown,
    ScrollToTop,
    ScrollToBottom,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    ClearSelection,
    SelectAll,
    Search,
    ToggleFullscreen,
    SpawnNewInstance,
    Quit,
    None,
}

#[derive(Clone, Debug)]
pub struct Binding {
    key: String,
    shift: bool,
    control: bool,
    alt: bool,
    super_key: bool,
    action: Option<Action>,
    chars: Option<String>,
}

impl Binding {
    pub fn from_config(binding: &KeyBinding) -> Result<Self, String> {
        let (shift, control, alt, super_key) = parse_mods(&binding.mods)?;
        let action = binding.action.as_deref().map(parse_action).transpose()?;
        if action.is_none() && binding.chars.is_none() {
            return Err(format!(
                "binding for {:?} needs an action or chars",
                binding.key
            ));
        }
        Ok(Self {
            key: normalize_key(&binding.key),
            shift,
            control,
            alt,
            super_key,
            action,
            chars: binding.chars.clone(),
        })
    }

    pub fn action(&self) -> Option<&Action> {
        self.action.as_ref()
    }

    pub fn chars(&self) -> Option<&str> {
        self.chars.as_deref()
    }
}

pub struct Bindings {
    entries: Vec<Binding>,
}

impl Bindings {
    pub fn new(config: &KeyboardConfig) -> Result<Self, String> {
        let entries = match &config.bindings {
            Some(bindings) => bindings
                .iter()
                .map(Binding::from_config)
                .collect::<Result<Vec<_>, _>>()?,
            None => default_bindings(),
        };
        Ok(Self { entries })
    }

    pub fn find(&self, keysym: u32, mods: Mods) -> Option<&Binding> {
        let name = key_name(keysym)?;
        self.entries
            .iter()
            .find(|binding| binding.key == name && mods.matches(binding))
    }
}

fn default_bindings() -> Vec<Binding> {
    let mut bindings = Vec::new();
    let mut add = |key: &str, mods: &str, action: Action| {
        bindings.push(Binding {
            key: normalize_key(key),
            shift: mods.contains("Shift"),
            control: mods.contains("Control"),
            alt: mods.contains("Alt"),
            super_key: mods.contains("Super"),
            action: Some(action),
            chars: None,
        });
    };
    add("C", "Control|Shift", Action::Copy);
    add("V", "Control|Shift", Action::Paste);
    add("Insert", "Shift", Action::Paste);
    add("Insert", "Control", Action::Copy);
    add("F", "Control|Shift", Action::Search);
    add("PageUp", "Shift", Action::ScrollPageUp);
    add("PageDown", "Shift", Action::ScrollPageDown);
    add("Home", "Control|Shift", Action::ScrollToTop);
    add("End", "Control|Shift", Action::ScrollToBottom);
    add("Up", "Control|Shift", Action::ScrollLineUp);
    add("Down", "Control|Shift", Action::ScrollLineDown);
    add("Equal", "Control|Shift", Action::IncreaseFontSize);
    add("Plus", "Control|Shift", Action::IncreaseFontSize);
    add("Minus", "Control|Shift", Action::DecreaseFontSize);
    add("0", "Control|Shift", Action::ResetFontSize);
    add("F11", "", Action::ToggleFullscreen);
    add("C", "Control|Shift|Alt", Action::SpawnNewInstance);
    bindings
}

fn parse_mods(text: &str) -> Result<(bool, bool, bool, bool), String> {
    let mut shift = false;
    let mut control = false;
    let mut alt = false;
    let mut super_key = false;
    for part in text.split(['|', '+', ',']) {
        let part = part.trim().to_ascii_lowercase();
        match part.as_str() {
            "" | "none" => {}
            "shift" => shift = true,
            "control" | "ctrl" => control = true,
            "alt" | "option" | "meta" => alt = true,
            "super" | "command" | "cmd" | "win" => super_key = true,
            other => return Err(format!("unknown modifier {other:?}")),
        }
    }
    Ok((shift, control, alt, super_key))
}

fn parse_action(text: &str) -> Result<Action, String> {
    let name: String = text
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect();
    Ok(match name.as_str() {
        "copy" => Action::Copy,
        "paste" => Action::Paste,
        "pasteselection" => Action::PasteSelection,
        "scrollpageup" => Action::ScrollPageUp,
        "scrollpagedown" => Action::ScrollPageDown,
        "scrolllineup" => Action::ScrollLineUp,
        "scrolllinedown" => Action::ScrollLineDown,
        "scrolltotop" => Action::ScrollToTop,
        "scrolltobottom" => Action::ScrollToBottom,
        "increasefontsize" => Action::IncreaseFontSize,
        "decreasefontsize" => Action::DecreaseFontSize,
        "resetfontsize" => Action::ResetFontSize,
        "clearselection" => Action::ClearSelection,
        "selectall" => Action::SelectAll,
        "search" | "find" => Action::Search,
        "togglefullscreen" => Action::ToggleFullscreen,
        "spawnnewinstance" => Action::SpawnNewInstance,
        "quit" | "close" => Action::Quit,
        "none" | "receivechar" => Action::None,
        other => return Err(format!("unknown action {other:?}")),
    })
}

pub fn normalize_key(name: &str) -> String {
    if let Some(symbol) = symbol_key(name.trim()) {
        return symbol.to_string();
    }
    let mut lower: String = name
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '_' && *ch != '-')
        .flat_map(|ch| ch.to_lowercase())
        .collect();
    if let Some(stripped) = lower.strip_prefix("xk") {
        lower = stripped.to_string();
    }
    let canonical = match lower.as_str() {
        "esc" => "escape",
        "enter" => "return",
        "pageup" | "pgup" => "pageup",
        "pagedown" | "pgdn" => "pagedown",
        "backspace" | "bksp" => "backspace",
        "exclam" => "1",
        "at" => "2",
        "numbersign" => "3",
        "dollar" => "4",
        "percent" => "5",
        "asciicircum" => "6",
        "ampersand" => "7",
        "asterisk" => "8",
        "parenleft" => "9",
        "parenright" => "0",
        "minus" | "underscore" => "-",
        "plus" | "equal" => "=",
        "colon" => ";",
        "quotedbl" => "'",
        "less" => ",",
        "greater" => ".",
        "question" => "/",
        "asciitilde" => "`",
        "braceleft" => "[",
        "braceright" => "]",
        "bar" => "\\",
        "slash" => "/",
        "period" => ".",
        "comma" => ",",
        "semicolon" => ";",
        "apostrophe" => "'",
        "grave" => "`",
        "backslash" => "\\",
        "bracketleft" => "[",
        "bracketright" => "]",
        other => other,
    };
    canonical.to_string()
}

/// Canonical key identity for a literal punctuation character. Shifted and
/// unshifted forms map to the same key so bindings behave like physical keys.
fn symbol_key(name: &str) -> Option<&'static str> {
    Some(match name {
        "+" | "=" => "=",
        "-" | "_" => "-",
        ")" => "0",
        "(" => "9",
        "!" => "1",
        "@" => "2",
        "#" => "3",
        "$" => "4",
        "%" => "5",
        "^" => "6",
        "&" => "7",
        "*" => "8",
        ":" | ";" => ";",
        "\"" | "'" => "'",
        "<" | "," => ",",
        ">" | "." => ".",
        "?" | "/" => "/",
        "~" | "`" => "`",
        "|" | "\\" => "\\",
        "{" | "[" => "[",
        "}" | "]" => "]",
        _ => return None,
    })
}

fn key_name(keysym: u32) -> Option<String> {
    let name = Keysym::new(keysym).name()?;
    let normalized = normalize_key(name);
    Some(match normalized.as_str() {
        "prior" => "pageup".to_string(),
        "next" => "pagedown".to_string(),
        "slash" => "/".to_string(),
        "period" => ".".to_string(),
        "comma" => ",".to_string(),
        "minus" => "-".to_string(),
        "equal" => "=".to_string(),
        "plus" => "+".to_string(),
        "bracketleft" => "[".to_string(),
        "bracketright" => "]".to_string(),
        "semicolon" => ";".to_string(),
        "apostrophe" => "'".to_string(),
        "grave" => "`".to_string(),
        "backslash" => "\\".to_string(),
        other => other.to_string(),
    })
}

#[derive(Default)]
pub struct Composer {
    pending: Option<char>,
}

impl Composer {
    pub fn reset(&mut self) {
        self.pending = None;
    }

    fn apply(&mut self, ch: char) -> Vec<u8> {
        let Some(mark) = self.pending.take() else {
            let mut buffer = [0u8; 4];
            return ch.encode_utf8(&mut buffer).as_bytes().to_vec();
        };
        if ch == ' ' {
            let spacing = match mark {
                '\u{300}' => '`',
                '\u{301}' => '\u{00b4}',
                '\u{302}' => '^',
                '\u{303}' => '~',
                '\u{304}' => '\u{00af}',
                '\u{306}' => '\u{02d8}',
                '\u{307}' => '\u{02d9}',
                '\u{308}' => '\u{00a8}',
                '\u{30a}' => '\u{02da}',
                '\u{30b}' => '\u{02dd}',
                '\u{30c}' => '\u{02c7}',
                '\u{327}' => '\u{00b8}',
                '\u{328}' => '\u{02db}',
                other => other,
            };
            let mut buffer = [0u8; 4];
            return spacing.encode_utf8(&mut buffer).as_bytes().to_vec();
        }
        let mut combined = String::new();
        combined.push(ch);
        combined.push(mark);
        combined.nfc().collect::<String>().into_bytes()
    }
}

/// Encodes a key press for the PTY. `None` means the key is consumed (a dead
/// key, or an unsupported modifier combination).
pub fn encode_key(
    keysym: u32,
    mods: Mods,
    app_cursor: bool,
    app_keypad: bool,
    composer: &mut Composer,
) -> Option<Vec<u8>> {
    let key = Keysym::new(keysym);

    if let Some(mark) = dead_key(keysym) {
        composer.pending = Some(mark);
        return None;
    }

    if let Some(bytes) = special_key(key, mods, app_cursor, app_keypad) {
        composer.reset();
        return Some(bytes);
    }

    let Some(ch) = key.key_char() else {
        composer.reset();
        return None;
    };
    if ch.is_control() {
        composer.reset();
        return None;
    }

    let mut out = Vec::new();
    if mods.control {
        if let Some(byte) = control_byte(ch) {
            if mods.alt {
                out.push(0x1b);
            }
            out.push(byte);
            composer.reset();
            return Some(out);
        }
    }
    if mods.alt {
        out.push(0x1b);
    }
    out.extend_from_slice(&composer.apply(ch));
    Some(out)
}

fn dead_key(keysym: u32) -> Option<char> {
    Some(match keysym {
        key::dead_grave => '\u{300}',
        key::dead_acute => '\u{301}',
        key::dead_circumflex => '\u{302}',
        key::dead_tilde => '\u{303}',
        key::dead_macron => '\u{304}',
        key::dead_breve => '\u{306}',
        key::dead_abovedot => '\u{307}',
        key::dead_diaeresis => '\u{308}',
        key::dead_abovering => '\u{30a}',
        key::dead_doubleacute => '\u{30b}',
        key::dead_caron => '\u{30c}',
        key::dead_cedilla => '\u{327}',
        key::dead_ogonek => '\u{328}',
        key::dead_belowdot => '\u{323}',
        key::dead_hook => '\u{309}',
        key::dead_horn => '\u{31b}',
        key::dead_stroke => '\u{338}',
        _ => return None,
    })
}

fn special_key(key: Keysym, mods: Mods, app_cursor: bool, app_keypad: bool) -> Option<Vec<u8>> {
    let parameter = mods.parameter();
    let modified = parameter != 1;
    let raw = key.raw();
    let tilde = |number: u8| {
        if modified {
            format!("\x1b[{number};{parameter}~")
        } else {
            format!("\x1b[{number}~")
        }
        .into_bytes()
    };
    let cursor = |final_byte: char| {
        if modified {
            format!("\x1b[1;{parameter}{final_byte}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        }
    };

    if raw == key::Return {
        return Some(vec![0x0d]);
    }
    if raw == key::Tab {
        return Some(if mods.shift {
            b"\x1b[Z".to_vec()
        } else {
            vec![0x09]
        });
    }
    if raw == key::ISO_Left_Tab {
        return Some(b"\x1b[Z".to_vec());
    }
    if raw == key::BackSpace {
        return Some(vec![if mods.control { 0x08 } else { 0x7f }]);
    }
    if raw == key::Escape {
        return Some(vec![0x1b]);
    }
    match raw {
        key::Up => return Some(cursor('A')),
        key::Down => return Some(cursor('B')),
        key::Right => return Some(cursor('C')),
        key::Left => return Some(cursor('D')),
        key::Home => return Some(cursor('H')),
        key::End => return Some(cursor('F')),
        key::Insert => return Some(tilde(2)),
        key::Delete => return Some(tilde(3)),
        key::Prior => return Some(tilde(5)),
        key::Next => return Some(tilde(6)),
        key::F1 => return Some(function_key('P', parameter, modified)),
        key::F2 => return Some(function_key('Q', parameter, modified)),
        key::F3 => return Some(function_key('R', parameter, modified)),
        key::F4 => return Some(function_key('S', parameter, modified)),
        key::F5 => return Some(tilde(15)),
        key::F6 => return Some(tilde(17)),
        key::F7 => return Some(tilde(18)),
        key::F8 => return Some(tilde(19)),
        key::F9 => return Some(tilde(20)),
        key::F10 => return Some(tilde(21)),
        key::F11 => return Some(tilde(23)),
        key::F12 => return Some(tilde(24)),
        _ => {}
    }
    if key.is_keypad_key() {
        if app_keypad {
            let final_byte = match raw {
                key::KP_Enter => Some('M'),
                key::KP_0 => Some('p'),
                key::KP_1 => Some('q'),
                key::KP_2 => Some('r'),
                key::KP_3 => Some('s'),
                key::KP_4 => Some('t'),
                key::KP_5 => Some('u'),
                key::KP_6 => Some('v'),
                key::KP_7 => Some('w'),
                key::KP_8 => Some('x'),
                key::KP_9 => Some('y'),
                key::KP_Add => Some('k'),
                key::KP_Subtract => Some('m'),
                key::KP_Multiply => Some('j'),
                key::KP_Divide => Some('o'),
                key::KP_Decimal => Some('n'),
                key::KP_Equal => Some('X'),
                _ => None,
            };
            if let Some(final_byte) = final_byte {
                return Some(format!("\x1bO{final_byte}").into_bytes());
            }
        } else {
            return match raw {
                key::KP_Enter => Some(vec![0x0d]),
                key::KP_Home => Some(cursor('H')),
                key::KP_Up => Some(cursor('A')),
                key::KP_Prior => Some(tilde(5)),
                key::KP_Left => Some(cursor('D')),
                key::KP_Begin => Some(cursor('E')),
                key::KP_Right => Some(cursor('C')),
                key::KP_End => Some(cursor('F')),
                key::KP_Down => Some(cursor('B')),
                key::KP_Next => Some(tilde(6)),
                key::KP_Insert => Some(tilde(2)),
                key::KP_Delete => Some(tilde(3)),
                _ => None,
            };
        }
    }

    None
}

fn function_key(final_byte: char, parameter: u8, modified: bool) -> Vec<u8> {
    if modified {
        format!("\x1b[1;{parameter}{final_byte}").into_bytes()
    } else {
        format!("\x1bO{final_byte}").into_bytes()
    }
}

fn control_byte(ch: char) -> Option<u8> {
    match ch {
        'a'..='z' => Some(ch as u8 - b'a' + 1),
        'A'..='Z' => Some(ch as u8 - b'A' + 1),
        '@' | ' ' | '`' | '2' => Some(0),
        '[' | '3' => Some(27),
        '\\' | '4' => Some(28),
        ']' | '5' => Some(29),
        '^' | '6' => Some(30),
        '_' | '7' | '/' => Some(31),
        '?' | '8' => Some(127),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_names() {
        assert_eq!(normalize_key("Page_Up"), "pageup");
        assert_eq!(normalize_key("PageUp"), "pageup");
        assert_eq!(normalize_key("Esc"), "escape");
        assert_eq!(normalize_key("V"), "v");
    }

    #[test]
    fn keypad_digits_and_navigation() {
        let mut composer = Composer::default();
        assert_eq!(
            encode_key(key::KP_1, Mods::default(), false, false, &mut composer).unwrap(),
            b"1"
        );
        assert_eq!(
            encode_key(key::KP_1, Mods::default(), false, true, &mut composer).unwrap(),
            b"\x1bOq"
        );
        assert_eq!(
            encode_key(key::KP_Add, Mods::default(), false, false, &mut composer).unwrap(),
            b"+"
        );
        assert_eq!(
            encode_key(key::KP_End, Mods::default(), false, false, &mut composer).unwrap(),
            b"\x1b[F"
        );
    }

    #[test]
    fn encodes_plain_and_ctrl_letters() {
        let mut composer = Composer::default();
        assert_eq!(
            encode_key(key::a, Mods::default(), false, false, &mut composer).unwrap(),
            b"a"
        );
        assert_eq!(
            encode_key(
                key::a,
                Mods {
                    control: true,
                    ..Mods::default()
                },
                false,
                false,
                &mut composer
            )
            .unwrap(),
            vec![1]
        );
        assert_eq!(
            encode_key(
                key::c,
                Mods {
                    control: true,
                    shift: true,
                    ..Mods::default()
                },
                false,
                false,
                &mut composer
            )
            .unwrap(),
            vec![3]
        );
    }

    #[test]
    fn composes_dead_acute_with_e() {
        let mut composer = Composer::default();
        assert!(
            encode_key(
                key::dead_acute,
                Mods::default(),
                false,
                false,
                &mut composer
            )
            .is_none()
        );
        assert_eq!(
            encode_key(key::e, Mods::default(), false, false, &mut composer).unwrap(),
            "\u{00e9}".as_bytes()
        );
    }

    #[test]
    fn arrows_follow_cursor_mode() {
        let mut composer = Composer::default();
        assert_eq!(
            encode_key(key::Up, Mods::default(), false, false, &mut composer).unwrap(),
            b"\x1b[A"
        );
        assert_eq!(
            encode_key(key::Up, Mods::default(), true, false, &mut composer).unwrap(),
            b"\x1bOA"
        );
        assert_eq!(
            encode_key(
                key::Up,
                Mods {
                    shift: true,
                    ..Mods::default()
                },
                false,
                false,
                &mut composer
            )
            .unwrap(),
            b"\x1b[1;2A"
        );
    }

    #[test]
    fn shifted_punctuation_matches_unshifted_binding() {
        let bindings = Bindings::new(&KeyboardConfig::default()).unwrap();
        let mods = Mods {
            shift: true,
            control: true,
            ..Mods::default()
        };
        assert_eq!(
            bindings.find(key::parenright, mods).unwrap().action(),
            Some(&Action::ResetFontSize)
        );
        assert_eq!(
            bindings.find(key::underscore, mods).unwrap().action(),
            Some(&Action::DecreaseFontSize)
        );
        assert_eq!(
            bindings.find(key::plus, mods).unwrap().action(),
            Some(&Action::IncreaseFontSize)
        );
    }

    #[test]
    fn binding_parsing_rejects_typos() {
        let config = KeyBinding {
            key: "V".into(),
            mods: "Control|Shift".into(),
            action: Some("Paste".into()),
            chars: None,
        };
        let binding = Binding::from_config(&config).unwrap();
        assert_eq!(binding.action(), Some(&Action::Paste));
        let config = KeyBinding {
            key: "V".into(),
            mods: "Control".into(),
            action: Some("Pastee".into()),
            chars: None,
        };
        assert!(Binding::from_config(&config).is_err());
    }
}

#[cfg(test)]
mod default_binding_tests {
    use super::*;
    #[test]
    fn page_bindings_exist() {
        let bindings = Bindings::new(&KeyboardConfig::default()).unwrap();
        let up = bindings.find(
            key::Page_Up,
            Mods {
                shift: true,
                ..Mods::default()
            },
        );
        assert!(up.is_some(), "page up missing");
        assert_eq!(up.unwrap().action(), Some(&Action::ScrollPageUp));
        let down = bindings.find(
            key::Page_Down,
            Mods {
                shift: true,
                ..Mods::default()
            },
        );
        assert!(down.is_some(), "page down missing");
    }

    #[test]
    fn search_binding_exists() {
        let bindings = Bindings::new(&KeyboardConfig::default()).unwrap();
        let binding = bindings.find(
            key::f,
            Mods {
                control: true,
                shift: true,
                ..Mods::default()
            },
        );
        assert!(binding.is_some(), "search binding missing");
        assert_eq!(binding.unwrap().action(), Some(&Action::Search));
        assert!(
            bindings
                .find(
                    key::f,
                    Mods {
                        control: true,
                        ..Mods::default()
                    },
                )
                .is_none(),
            "Ctrl+F should stay available to the shell"
        );
    }
}
