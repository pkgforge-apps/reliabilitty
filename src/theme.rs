//! Turns configured colour strings into pixels.

use crate::config::{self, AnsiColors, ColorsConfig};

pub type Rgb = [u8; 3];

#[derive(Clone, Debug)]
pub struct Theme {
    pub background: Rgb,
    pub foreground: Rgb,
    pub cursor: Rgb,
    pub cursor_text: Rgb,
    pub selection_background: Rgb,
    pub selection_foreground: Rgb,
    pub bright_foreground: Rgb,
    pub dim_foreground: Rgb,
    pub palette: [Rgb; 256],
    pub dim_palette: [Rgb; 256],
}

impl Theme {
    pub fn from_config(config: &ColorsConfig) -> Result<Self, String> {
        let background = parse(&config.primary.background)?;
        let foreground = parse(&config.primary.foreground)?;
        let cursor = match &config.cursor.cursor {
            Some(value) => parse(value)?,
            None => foreground,
        };
        let cursor_text = match &config.cursor.text {
            Some(value) => parse(value)?,
            None => background,
        };
        let selection_background = parse(&config.selection.background)?;
        let selection_foreground = parse(&config.selection.text)?;
        let bright_foreground = match &config.primary.bright_foreground {
            Some(value) => parse(value)?,
            None => foreground,
        };
        let dim_foreground = match &config.primary.dim_foreground {
            Some(value) => parse(value)?,
            None => dim(foreground),
        };

        let normal = config.normal.resolve(&AnsiColors::normal_default());
        let bright = config.bright.resolve(&AnsiColors::bright_default());
        let mut palette = [[0u8; 3]; 256];
        for (index, value) in normal.iter().enumerate() {
            palette[index] = parse(value)?;
        }
        for (index, value) in bright.iter().enumerate() {
            palette[index + 8] = parse(value)?;
        }
        let levels = [0, 95, 135, 175, 215, 255];
        for (index, slot) in palette.iter_mut().enumerate().take(232).skip(16) {
            let value = index - 16;
            *slot = [
                levels[value / 36],
                levels[(value / 6) % 6],
                levels[value % 6],
            ];
        }
        for (index, slot) in palette.iter_mut().enumerate().take(256).skip(232) {
            let level = (8 + (index - 232) * 10) as u8;
            *slot = [level, level, level];
        }

        let dim_normal = match &config.dim {
            Some(dim) => dim.resolve(&AnsiColors::normal_default()),
            None => normal.clone(),
        };
        let dim_bright = match &config.dim {
            Some(dim) => dim.resolve(&AnsiColors::bright_default()),
            None => bright.clone(),
        };
        let mut dim_palette = [[0u8; 3]; 256];
        for index in 0..16 {
            let value = if index < 8 {
                &dim_normal[index]
            } else {
                &dim_bright[index - 8]
            };
            dim_palette[index] = if config.dim.is_some() {
                parse(value)?
            } else {
                dim(palette[index])
            };
        }
        for index in 16..256 {
            dim_palette[index] = dim(palette[index]);
        }

        Ok(Self {
            background,
            foreground,
            cursor,
            cursor_text,
            selection_background,
            selection_foreground,
            bright_foreground,
            dim_foreground,
            palette,
            dim_palette,
        })
    }
}

/// Darkens a colour to two thirds, the usual terminal meaning of dim.
fn dim(color: Rgb) -> Rgb {
    [
        (color[0] as u16 * 2 / 3) as u8,
        (color[1] as u16 * 2 / 3) as u8,
        (color[2] as u16 * 2 / 3) as u8,
    ]
}

fn parse(value: &str) -> Result<Rgb, String> {
    config::parse_color(value)
}
