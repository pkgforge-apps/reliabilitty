//! Shell-style configuration with environment overrides.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub const APP_NAME: &str = "rtty";

const ENV_PREFIX: &str = "RTTY_";

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub window: WindowConfig,
    pub font: FontConfig,
    pub colors: ColorsConfig,
    pub cursor: CursorConfig,
    pub scrolling: ScrollingConfig,
    pub selection: SelectionConfig,
    pub terminal: TerminalConfig,
    pub bell: BellConfig,
    pub keyboard: KeyboardConfig,
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct WindowConfig {
    pub title: String,
    pub class: String,
    pub instance: String,
    pub dimensions: Dimensions,
    pub padding: Padding,
    pub dynamic_padding: bool,
    pub decorations: String,
    pub opacity: f32,
    pub startup_mode: String,
    pub dynamic_title: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "Reliabilitty".into(),
            class: "Reliabilitty".into(),
            instance: "rtty".into(),
            dimensions: Dimensions::default(),
            padding: Padding::default(),
            dynamic_padding: false,
            decorations: "full".into(),
            opacity: 1.0,
            startup_mode: "windowed".into(),
            dynamic_title: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Dimensions {
    pub columns: u16,
    pub lines: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct Padding {
    pub x: u16,
    pub y: u16,
}

impl Default for Padding {
    fn default() -> Self {
        Self { x: 2, y: 2 }
    }
}

#[derive(Clone, Debug)]
pub struct FontConfig {
    pub size: f32,
    pub dpi: f32,
    pub normal: FontDescription,
    pub bold: Option<FontDescription>,
    pub italic: Option<FontDescription>,
    pub bold_italic: Option<FontDescription>,
    pub fallback: Vec<String>,
    pub offset: Offset,
    pub glyph_offset: Offset,
    pub hinting: bool,
    pub cell_width: Option<u16>,
    pub cell_height: Option<u16>,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            size: 11.0,
            dpi: 0.0,
            normal: FontDescription::default(),
            bold: None,
            italic: None,
            bold_italic: None,
            fallback: default_fallbacks(),
            offset: Offset::default(),
            glyph_offset: Offset::default(),
            hinting: true,
            cell_width: None,
            cell_height: None,
        }
    }
}

fn default_fallbacks() -> Vec<String> {
    [
        "Noto Sans Mono",
        "DejaVu Sans Mono",
        "Liberation Mono",
        "Noto Sans Mono CJK SC",
        "Noto Sans Mono CJK JP",
        "Noto Sans Mono CJK KR",
        "Noto Sans CJK SC",
        "Noto Sans CJK JP",
        "Noto Sans CJK KR",
        "Noto Color Emoji",
        "Noto Emoji",
        "Noto Sans Symbols 2",
        "Noto Sans Symbols",
        "DejaVu Sans",
        "FreeSerif",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

#[derive(Clone, Debug)]
pub struct FontDescription {
    pub family: String,
    pub style: Option<String>,
}

impl Default for FontDescription {
    fn default() -> Self {
        Self {
            family: "monospace".into(),
            style: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Offset {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug)]
pub struct ColorsConfig {
    pub primary: PrimaryColors,
    pub cursor: CursorColors,
    pub selection: SelectionColors,
    pub normal: AnsiColors,
    pub bright: AnsiColors,
    pub dim: Option<AnsiColors>,
}

impl Default for ColorsConfig {
    fn default() -> Self {
        Self {
            primary: PrimaryColors::default(),
            cursor: CursorColors::default(),
            selection: SelectionColors::default(),
            normal: AnsiColors::normal_default(),
            bright: AnsiColors::bright_default(),
            dim: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PrimaryColors {
    pub foreground: String,
    pub background: String,
    pub dim_foreground: Option<String>,
    pub bright_foreground: Option<String>,
}

impl Default for PrimaryColors {
    fn default() -> Self {
        Self {
            foreground: "#c5c8c6".into(),
            background: "#1d1f21".into(),
            dim_foreground: None,
            bright_foreground: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CursorColors {
    pub cursor: Option<String>,
    pub text: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SelectionColors {
    pub background: String,
    pub text: String,
}

impl Default for SelectionColors {
    fn default() -> Self {
        Self {
            background: "#373b41".into(),
            text: "#c5c8c6".into(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AnsiColors {
    pub black: Option<String>,
    pub red: Option<String>,
    pub green: Option<String>,
    pub yellow: Option<String>,
    pub blue: Option<String>,
    pub magenta: Option<String>,
    pub cyan: Option<String>,
    pub white: Option<String>,
}

impl AnsiColors {
    pub fn normal_default() -> Self {
        Self::from_names([
            "#1d1f21", "#cc6666", "#b5bd68", "#f0c674", "#81a2be", "#b294bb", "#8abeb7", "#c5c8c6",
        ])
    }

    pub fn bright_default() -> Self {
        Self::from_names([
            "#666666", "#d54e53", "#b9ca4a", "#e7c547", "#7aa6da", "#c397d8", "#70c0b1", "#eaeaea",
        ])
    }

    fn from_names(names: [&str; 8]) -> Self {
        let [black, red, green, yellow, blue, magenta, cyan, white] =
            names.map(|name| Some(name.to_string()));
        Self {
            black,
            red,
            green,
            yellow,
            blue,
            magenta,
            cyan,
            white,
        }
    }

    pub fn resolve(&self, fallback: &Self) -> [String; 8] {
        [
            self.black.as_ref().or(fallback.black.as_ref()),
            self.red.as_ref().or(fallback.red.as_ref()),
            self.green.as_ref().or(fallback.green.as_ref()),
            self.yellow.as_ref().or(fallback.yellow.as_ref()),
            self.blue.as_ref().or(fallback.blue.as_ref()),
            self.magenta.as_ref().or(fallback.magenta.as_ref()),
            self.cyan.as_ref().or(fallback.cyan.as_ref()),
            self.white.as_ref().or(fallback.white.as_ref()),
        ]
        .map(|value| value.cloned().unwrap_or_default())
    }
}

#[derive(Clone, Debug)]
pub struct CursorConfig {
    pub style: String,
    pub blink: bool,
    pub blink_interval: u64,
    pub unfocused_hollow: bool,
    pub thickness: f32,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            style: "Block".into(),
            blink: true,
            blink_interval: 600,
            unfocused_hollow: true,
            thickness: 0.15,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScrollingConfig {
    pub history: usize,
    pub multiplier: usize,
}

impl Default for ScrollingConfig {
    fn default() -> Self {
        Self {
            history: 10_000,
            multiplier: 3,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SelectionConfig {
    pub save_to_clipboard: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TerminalConfig {
    pub shell: Option<Shell>,
}

#[derive(Clone, Debug)]
pub struct Shell {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct BellConfig {
    pub audible: bool,
    pub visual: bool,
}

impl Default for BellConfig {
    fn default() -> Self {
        Self {
            audible: true,
            visual: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct KeyboardConfig {
    pub bindings: Option<Vec<KeyBinding>>,
}

#[derive(Clone, Debug)]
pub struct KeyBinding {
    pub key: String,
    pub mods: String,
    pub action: Option<String>,
    pub chars: Option<String>,
}

#[derive(Clone, Debug)]
pub struct LoadedConfig {
    pub config: Config,
    pub path: Option<PathBuf>,
}

impl Config {
    pub fn load(explicit: Option<&Path>) -> Result<LoadedConfig, String> {
        let path = match explicit {
            Some(path) => Some(path.to_path_buf()),
            None => std::env::var_os("RTTY_CONFIG")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .or_else(|| default_user_path().filter(|path| path.is_file()))
                .or_else(system_path),
        };
        let mut config = Config::default();
        if let Some(path) = &path {
            load_file(path, &mut config)?;
        }
        load_env(&mut config)?;
        config.validate(path.as_deref())?;
        Ok(LoadedConfig { config, path })
    }

    fn validate(&self, path: Option<&Path>) -> Result<(), String> {
        let at = |message: &str| match path {
            Some(path) => format!("invalid {}: {message}", path.display()),
            None => format!("invalid config: {message}"),
        };
        if !(1.0..=512.0).contains(&self.font.size) {
            return Err(at("FONT_SIZE must be between 1 and 512"));
        }
        if self.font.dpi < 0.0 {
            return Err(at("FONT_DPI cannot be negative"));
        }
        if !(0.0..=1.0).contains(&self.window.opacity) {
            return Err(at("TRANSPARENCY must be between 0 and 1"));
        }
        if self.cursor.blink_interval < 50 {
            return Err(at("CURSOR_BLINK_INTERVAL must be at least 50"));
        }
        if !(0.02..=1.0).contains(&self.cursor.thickness) {
            return Err(at("CURSOR_THICKNESS must be between 0.02 and 1"));
        }
        if self.scrolling.multiplier == 0 {
            return Err(at("WHEEL_MULTIPLIER must be at least 1"));
        }
        if self.scrolling.history > 1_000_000 {
            return Err(at("HISTORY must not exceed 1000000"));
        }
        if self.window.dimensions.columns > 1000 || self.window.dimensions.lines > 1000 {
            return Err(at("grid dimensions cannot exceed 1000"));
        }
        match self.window.decorations.to_ascii_lowercase().as_str() {
            "full" | "none" => {}
            _ => return Err(at("DECORATIONS must be full or none")),
        }
        match self.window.startup_mode.to_ascii_lowercase().as_str() {
            "windowed" | "maximized" | "fullscreen" => {}
            _ => return Err(at("STARTUP_MODE is invalid")),
        }
        Ok(())
    }
}

fn default_user_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join(APP_NAME).join("rtty.conf"))
}

fn system_path() -> Option<PathBuf> {
    let path = PathBuf::from("/etc").join(APP_NAME).join("rtty.conf");
    path.is_file().then_some(path)
}

fn load_file(path: &Path, config: &mut Config) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    for (number, line) in text.lines().enumerate() {
        let Some((key, value)) = parse_line(line)? else {
            continue;
        };
        if !apply_key(config, &key, &value)? {
            return Err(format!(
                "unknown config key at {}:{}: {key}",
                path.display(),
                number + 1
            ));
        }
    }
    Ok(())
}

fn load_env(config: &mut Config) -> Result<(), String> {
    for (key, value) in std::env::vars_os() {
        let Ok(key) = key.into_string() else {
            continue;
        };
        let Some(suffix) = key.strip_prefix(ENV_PREFIX) else {
            continue;
        };
        if suffix == "CONFIG" {
            continue;
        }
        let _ = apply_key(config, suffix, &value.to_string_lossy())?;
    }
    Ok(())
}

fn parse_line(line: &str) -> Result<Option<(String, String)>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let line = line.strip_prefix("export ").unwrap_or(line).trim();
    let (key, value) = line
        .split_once('=')
        .ok_or_else(|| format!("expected KEY=value, got: {line}"))?;
    let key = key.trim().to_uppercase();
    let value = value.trim();
    let value = match value.chars().next() {
        Some(quote @ ('"' | '\'')) => {
            let (text, rest) = parse_quoted(value, quote)?;
            let rest = rest.trim();
            if !rest.is_empty() && !rest.starts_with('#') {
                return Err(format!("unexpected text after value: {rest}"));
            }
            text
        }
        _ => value.split('#').next().unwrap_or("").trim().to_string(),
    };
    Ok(Some((key, value)))
}

fn parse_quoted(value: &str, quote: char) -> Result<(String, &str), String> {
    let mut out = String::new();
    let mut chars = value[quote.len_utf8()..].char_indices();
    while let Some((index, ch)) = chars.next() {
        match ch {
            ch if ch == quote => {
                return Ok((out, &value[quote.len_utf8() + index + ch.len_utf8()..]));
            }
            '\\' => match chars.next() {
                Some((_, 'n')) => out.push('\n'),
                Some((_, 't')) => out.push('\t'),
                Some((_, 'r')) => out.push('\r'),
                Some((_, other)) => out.push(other),
                None => return Err("unfinished escape".into()),
            },
            other => out.push(other),
        }
    }
    Err("unterminated quoted value".into())
}

fn apply_key(config: &mut Config, key: &str, value: &str) -> Result<bool, String> {
    match key {
        "TITLE" => config.window.title = value.into(),
        "CLASS" => config.window.class = value.into(),
        "INSTANCE" => config.window.instance = value.into(),
        "COLUMNS" => config.window.dimensions.columns = number(value, key)?,
        "LINES" => config.window.dimensions.lines = number(value, key)?,
        "PADDING_X" => config.window.padding.x = number(value, key)?,
        "PADDING_Y" => config.window.padding.y = number(value, key)?,
        "DYNAMIC_PADDING" => config.window.dynamic_padding = boolean(value, key)?,
        "DECORATIONS" => config.window.decorations = value.into(),
        "TRANSPARENCY" => config.window.opacity = number(value, key)?,
        "STARTUP_MODE" => config.window.startup_mode = value.into(),
        "DYNAMIC_TITLE" => config.window.dynamic_title = boolean(value, key)?,

        "FONT_SIZE" => config.font.size = number(value, key)?,
        "FONT_DPI" => config.font.dpi = number(value, key)?,
        "CELL_WIDTH" => config.font.cell_width = Some(number(value, key)?),
        "CELL_HEIGHT" => config.font.cell_height = Some(number(value, key)?),
        "FONT_FAMILY" => config.font.normal.family = value.into(),
        "FONT_STYLE" => config.font.normal.style = optional(value),
        "FONT_BOLD_FAMILY" => font_desc(&mut config.font.bold).family = value.into(),
        "FONT_BOLD_STYLE" => font_desc(&mut config.font.bold).style = optional(value),
        "FONT_ITALIC_FAMILY" => font_desc(&mut config.font.italic).family = value.into(),
        "FONT_ITALIC_STYLE" => font_desc(&mut config.font.italic).style = optional(value),
        "FONT_BOLD_ITALIC_FAMILY" => font_desc(&mut config.font.bold_italic).family = value.into(),
        "FONT_BOLD_ITALIC_STYLE" => font_desc(&mut config.font.bold_italic).style = optional(value),
        "FONT_FALLBACK" => config.font.fallback = list(value),
        "FONT_OFFSET_X" => config.font.offset.x = number(value, key)?,
        "FONT_OFFSET_Y" => config.font.offset.y = number(value, key)?,
        "GLYPH_OFFSET_X" => config.font.glyph_offset.x = number(value, key)?,
        "GLYPH_OFFSET_Y" => config.font.glyph_offset.y = number(value, key)?,
        "HINTING" => config.font.hinting = boolean(value, key)?,

        "BACKGROUND" => config.colors.primary.background = value.into(),
        "FOREGROUND" => config.colors.primary.foreground = value.into(),
        "DIM_FOREGROUND" => config.colors.primary.dim_foreground = Some(value.into()),
        "BRIGHT_FOREGROUND" => config.colors.primary.bright_foreground = Some(value.into()),
        "CURSOR_COLOR" => config.colors.cursor.cursor = Some(value.into()),
        "CURSOR_TEXT_COLOR" => config.colors.cursor.text = Some(value.into()),
        "SELECTION_BACKGROUND" => config.colors.selection.background = value.into(),
        "SELECTION_FOREGROUND" => config.colors.selection.text = value.into(),
        key if key.starts_with("COLOR_") => {
            let index: usize = key[6..]
                .parse()
                .map_err(|_| format!("invalid colour index in {key}"))?;
            set_ansi(&mut config.colors, index, value.into())?;
        }

        "CURSOR_STYLE" => config.cursor.style = value.into(),
        "CURSOR_BLINK" => config.cursor.blink = boolean(value, key)?,
        "CURSOR_BLINK_INTERVAL" => config.cursor.blink_interval = number(value, key)?,
        "CURSOR_UNFOCUSED_HOLLOW" => config.cursor.unfocused_hollow = boolean(value, key)?,
        "CURSOR_THICKNESS" => config.cursor.thickness = number(value, key)?,

        "HISTORY" => config.scrolling.history = number(value, key)?,
        "WHEEL_MULTIPLIER" => config.scrolling.multiplier = number(value, key)?,
        "SAVE_TO_CLIPBOARD" => config.selection.save_to_clipboard = boolean(value, key)?,
        "SHELL" => set_shell_program(config, value),
        "SHELL_ARGS" => set_shell_args(config, value),
        "BELL_AUDIBLE" => config.bell.audible = boolean(value, key)?,
        "BELL_VISUAL" => config.bell.visual = boolean(value, key)?,
        "BINDING" => {
            let binding = parse_binding(value)?;
            config
                .keyboard
                .bindings
                .get_or_insert_with(Vec::new)
                .push(binding);
        }
        key if key.starts_with("ENV_") => {
            config.env.insert(key[4..].to_string(), value.into());
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn font_desc(slot: &mut Option<FontDescription>) -> &mut FontDescription {
    slot.get_or_insert_with(FontDescription::default)
}

fn number<T: FromStr>(value: &str, key: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("invalid value for {key}: {value}"))
}

fn boolean(value: &str, key: &str) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(format!("invalid boolean for {key}: {value}")),
    }
}

fn optional(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn set_ansi(colors: &mut ColorsConfig, index: usize, value: String) -> Result<(), String> {
    if index >= 16 {
        return Err(format!("colour index out of range: {index}"));
    }
    let palette = if index < 8 {
        &mut colors.normal
    } else {
        &mut colors.bright
    };
    match index % 8 {
        0 => palette.black = Some(value),
        1 => palette.red = Some(value),
        2 => palette.green = Some(value),
        3 => palette.yellow = Some(value),
        4 => palette.blue = Some(value),
        5 => palette.magenta = Some(value),
        6 => palette.cyan = Some(value),
        _ => palette.white = Some(value),
    }
    Ok(())
}

fn set_shell_program(config: &mut Config, value: &str) {
    match &mut config.terminal.shell {
        Some(shell) => shell.program = value.into(),
        None => {
            config.terminal.shell = Some(Shell {
                program: value.into(),
                args: Vec::new(),
            });
        }
    }
}

fn set_shell_args(config: &mut Config, value: &str) {
    let args: Vec<String> = value.split_whitespace().map(str::to_string).collect();
    match &mut config.terminal.shell {
        Some(shell) => shell.args = args,
        None => {
            config.terminal.shell = Some(Shell {
                program: String::new(),
                args,
            });
        }
    }
}

fn parse_binding(spec: &str) -> Result<KeyBinding, String> {
    let (combo, action) = spec
        .split_once(':')
        .ok_or_else(|| format!("expected Mod+Key:action, got: {spec}"))?;
    let parts: Vec<&str> = combo.split('+').map(str::trim).collect();
    let key = parts.last().copied().unwrap_or("");
    if key.is_empty() {
        return Err(format!("missing key in binding: {spec}"));
    }
    let mods = parts[..parts.len() - 1].join("|");
    let (action, chars) = match action.strip_prefix("chars=") {
        Some(text) => (None, Some(text.to_string())),
        None => (Some(action.trim().to_string()), None),
    };
    Ok(KeyBinding {
        key: key.to_string(),
        mods,
        action,
        chars,
    })
}

pub fn parse_color(value: &str) -> Result<[u8; 3], String> {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix('#') {
        return parse_hex_color(rest);
    }
    if let Some(rest) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        return parse_hex_color(rest);
    }
    if let Some(rest) = value.strip_prefix("rgb:") {
        let mut parts = rest.split('/');
        let red = u16::from_str_radix(parts.next().unwrap_or(""), 16)
            .map_err(|_| format!("invalid colour {value:?}"))?;
        let green = u16::from_str_radix(parts.next().unwrap_or(""), 16)
            .map_err(|_| format!("invalid colour {value:?}"))?;
        let blue = u16::from_str_radix(parts.next().unwrap_or(""), 16)
            .map_err(|_| format!("invalid colour {value:?}"))?;
        if parts.next().is_some() {
            return Err(format!("invalid colour {value:?}"));
        }
        return Ok([(red >> 8) as u8, (green >> 8) as u8, (blue >> 8) as u8]);
    }
    let named = match value.to_ascii_lowercase().as_str() {
        "black" => Some([0, 0, 0]),
        "red" => Some([255, 0, 0]),
        "green" => Some([0, 128, 0]),
        "yellow" => Some([255, 255, 0]),
        "blue" => Some([0, 0, 255]),
        "magenta" => Some([255, 0, 255]),
        "cyan" => Some([0, 255, 255]),
        "white" => Some([255, 255, 255]),
        "gray" | "grey" => Some([128, 128, 128]),
        _ => None,
    };
    named.ok_or_else(|| format!("invalid colour {value:?}"))
}

fn parse_hex_color(rest: &str) -> Result<[u8; 3], String> {
    let digits: Vec<u8> = rest
        .bytes()
        .map(|byte| match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            b'A'..=b'F' => Ok(byte - b'A' + 10),
            _ => Err(()),
        })
        .collect::<Result<_, _>>()
        .map_err(|_| format!("invalid colour #{rest}"))?;
    match digits.len() {
        3 => Ok([digits[0] * 17, digits[1] * 17, digits[2] * 17]),
        6 => Ok([
            digits[0] * 16 + digits[1],
            digits[2] * 16 + digits[3],
            digits[4] * 16 + digits[5],
        ]),
        12 => Ok([
            digits[0] * 16 + digits[1],
            digits[4] * 16 + digits[5],
            digits[8] * 16 + digits[9],
        ]),
        _ => Err(format!("invalid colour #{rest}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shell_style_lines() {
        let (key, value) = parse_line("FONT_SIZE=9 # comment").unwrap().unwrap();
        assert_eq!(key, "FONT_SIZE");
        assert_eq!(value, "9");
        let (_, value) = parse_line("FONT_FAMILY=\"Liberation Mono\"")
            .unwrap()
            .unwrap();
        assert_eq!(value, "Liberation Mono");
    }

    #[test]
    fn applies_keys() {
        let mut config = Config::default();
        apply_key(&mut config, "TRANSPARENCY", "0.8").unwrap();
        assert!((config.window.opacity - 0.8).abs() < 0.001);
        apply_key(&mut config, "CELL_WIDTH", "6").unwrap();
        assert_eq!(config.font.cell_width, Some(6));
        apply_key(&mut config, "COLOR_1", "#ff0000").unwrap();
        assert_eq!(config.colors.normal.red.as_deref(), Some("#ff0000"));
    }

    #[test]
    fn parses_bindings() {
        let binding = parse_binding("Control+Shift+C:copy").unwrap();
        assert_eq!(binding.key, "C");
        assert_eq!(binding.mods, "Control|Shift");
        assert_eq!(binding.action.as_deref(), Some("copy"));
    }

    #[test]
    fn rejects_unknown_keys() {
        let mut config = Config::default();
        assert!(!apply_key(&mut config, "NOPE", "1").unwrap());
    }

    #[test]
    fn parses_hex_colours() {
        assert_eq!(parse_color("#abc").unwrap(), [0xaa, 0xbb, 0xcc]);
        assert_eq!(parse_color("0x102030").unwrap(), [0x10, 0x20, 0x30]);
    }
}
