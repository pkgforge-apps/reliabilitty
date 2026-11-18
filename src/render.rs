//! Software text rendering with fontdb and swash. No X11 dependency.

use std::collections::HashMap;
use std::sync::Arc;

use fontdb::{Database, Family, Query, Stretch, Style, Weight};
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::{FontRef, GlyphId};
use unicode_width::UnicodeWidthChar;

use crate::config::{FontConfig, FontDescription};
use crate::scrollbar;
use crate::theme::{Rgb, Theme};
use crate::vt::{Attrs, Cell, Color, Terminal};

/// A 32-bit pixel buffer. Opaque canvases use `0x00RRGGBB`; ARGB canvases
/// use straight-alpha `0xAARRGGBB`.
pub struct Canvas {
    pub width: u16,
    pub height: u16,
    pub pixels: Vec<u32>,
    /// True when pixels contain alpha and must be composited.
    pub argb: bool,
}

impl Canvas {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width as usize * height as usize],
            argb: false,
        }
    }

    fn clip(&self, x: i32, y: i32, width: i32, height: i32) -> Option<(u32, u32, u32, u32)> {
        if width <= 0 || height <= 0 {
            return None;
        }
        let x0 = x.max(0) as u32;
        let y0 = y.max(0) as u32;
        let x1 = (x + width).min(self.width as i32).max(0) as u32;
        let y1 = (y + height).min(self.height as i32).max(0) as u32;
        (x0 < x1 && y0 < y1).then_some((x0, y0, x1, y1))
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.pixels.clear();
        self.pixels.resize(width as usize * height as usize, 0);
    }

    pub fn fill(&mut self, color: u32) {
        self.pixels.fill(color);
    }

    /// Scrolls the whole canvas vertically by whole rows. Positive rows move
    /// content up (a new blank area appears at the bottom). Returns the number
    /// of pixel rows actually shifted.
    pub fn scroll_rows(&mut self, rows: i32, cell_height: i32, background: u32) -> i32 {
        if rows == 0 || cell_height <= 0 {
            return 0;
        }
        let shift = (rows * cell_height).clamp(-(self.height as i32), self.height as i32);
        if shift == 0 {
            return 0;
        }
        let w = self.width as usize;
        let pixels = shift.unsigned_abs() as usize * w;
        let len = self.pixels.len();
        if shift > 0 {
            self.pixels.copy_within(pixels.., 0);
            self.pixels[len - pixels..].fill(background);
        } else {
            self.pixels.copy_within(..len - pixels, pixels);
            self.pixels[..pixels].fill(background);
        }
        shift
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, width: i32, height: i32, color: u32) {
        let Some((x0, y0, x1, y1)) = self.clip(x, y, width, height) else {
            return;
        };
        for row in y0..y1 {
            let start = row as usize * self.width as usize + x0 as usize;
            let end = row as usize * self.width as usize + x1 as usize;
            self.pixels[start..end].fill(color);
        }
    }

    fn blend_mask(&mut self, x: i32, y: i32, width: u32, height: u32, data: &[u8], color: Rgb) {
        let Some((x0, y0, x1, y1)) = self.clip(x, y, width as i32, height as i32) else {
            return;
        };
        let source = rgb(color);
        for row in y0..y1 {
            let source_row = (row as i32 - y) as usize * width as usize;
            let destination_row = row as usize * self.width as usize;
            for col in x0..x1 {
                let alpha = data[source_row + (col as i32 - x) as usize] as u32;
                if alpha == 0 {
                    continue;
                }
                let index = destination_row + col as usize;
                self.pixels[index] = if self.argb {
                    blend_alpha(self.pixels[index], source, alpha)
                } else {
                    blend(self.pixels[index], source, alpha)
                };
            }
        }
    }

    fn blend_rgba(&mut self, x: i32, y: i32, width: u32, height: u32, data: &[u8]) {
        let Some((x0, y0, x1, y1)) = self.clip(x, y, width as i32, height as i32) else {
            return;
        };
        for row in y0..y1 {
            let source_row = (row as i32 - y) as usize * width as usize;
            let destination_row = row as usize * self.width as usize;
            for col in x0..x1 {
                let source_index = (source_row + (col as i32 - x) as usize) * 4;
                let alpha = data[source_index + 3] as u32;
                if alpha == 0 {
                    continue;
                }
                let color = [
                    data[source_index],
                    data[source_index + 1],
                    data[source_index + 2],
                ];
                let index = destination_row + col as usize;
                self.pixels[index] = if self.argb {
                    blend_alpha(self.pixels[index], rgb(color), alpha)
                } else {
                    blend(self.pixels[index], rgb(color), alpha)
                };
            }
        }
    }
}

fn blend(destination: u32, source: u32, alpha: u32) -> u32 {
    let destination_r = (destination >> 16) & 0xff;
    let destination_g = (destination >> 8) & 0xff;
    let destination_b = destination & 0xff;
    let source_r = (source >> 16) & 0xff;
    let source_g = (source >> 8) & 0xff;
    let source_b = source & 0xff;
    let inverse = 255 - alpha;
    let red = (source_r * alpha + destination_r * inverse) / 255;
    let green = (source_g * alpha + destination_g * inverse) / 255;
    let blue = (source_b * alpha + destination_b * inverse) / 255;
    (red << 16) | (green << 8) | blue
}

pub fn rgb(color: Rgb) -> u32 {
    ((color[0] as u32) << 16) | ((color[1] as u32) << 8) | color[2] as u32
}

/// Scales a colour for the scrollbar track and thumb.
fn shade(color: Rgb, factor: f32) -> Rgb {
    [
        (color[0] as f32 * factor) as u8,
        (color[1] as f32 * factor) as u8,
        (color[2] as f32 * factor) as u8,
    ]
}

pub fn argb(color: Rgb, alpha: u8) -> u32 {
    ((alpha as u32) << 24) | rgb(color)
}

pub fn pixel(canvas: &Canvas, color: Rgb, alpha: u8) -> u32 {
    if canvas.argb {
        argb(color, alpha)
    } else {
        rgb(color)
    }
}

fn blend_alpha(destination: u32, source: u32, alpha: u32) -> u32 {
    let destination_alpha = destination >> 24;
    let inverse = 255 - alpha;
    let denominator = alpha * 255 + destination_alpha * inverse;
    if denominator == 0 {
        return 0;
    }
    let red = (((source >> 16) & 0xff) * alpha * 255
        + ((destination >> 16) & 0xff) * destination_alpha * inverse
        + denominator / 2)
        / denominator;
    let green = (((source >> 8) & 0xff) * alpha * 255
        + ((destination >> 8) & 0xff) * destination_alpha * inverse
        + denominator / 2)
        / denominator;
    let blue = ((source & 0xff) * alpha * 255
        + (destination & 0xff) * destination_alpha * inverse
        + denominator / 2)
        / denominator;
    let out_alpha = (alpha * 255 + destination_alpha * inverse + 127) / 255;
    (out_alpha << 24) | (red << 16) | (green << 8) | blue
}

struct LoadedFont {
    data: Arc<Vec<u8>>,
    index: u32,
    synthetic_bold: bool,
}

impl LoadedFont {
    fn font(&self) -> Option<FontRef<'_>> {
        FontRef::from_index(&self.data, self.index as usize)
    }
}

struct GlyphBitmap {
    width: u32,
    height: u32,
    left: i32,
    top: i32,
    content: Content,
    data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct GlyphKey {
    font: u32,
    glyph: u16,
}

#[derive(Clone, Copy, Debug)]
struct GlyphRef {
    font: u32,
    glyph: u16,
}

pub struct Renderer {
    cell_width: u16,
    cell_height: u16,
    baseline: f32,
    underline_offset: f32,
    stroke_size: f32,
    hinting: bool,
    size_px: f32,
    glyph_offset_x: f32,
    glyph_offset_y: f32,
    cursor_thickness: f32,
    unfocused_hollow: bool,
    fonts: Vec<LoadedFont>,
    primary: [usize; 4],
    database: Database,
    fallback_families: Vec<String>,
    loaded_families: HashMap<String, Option<u32>>,
    glyph_cache: HashMap<GlyphKey, Option<GlyphBitmap>>,
    fallback_cache: HashMap<(char, u8), Option<GlyphRef>>,
    scale_context: ScaleContext,
}

impl Renderer {
    /// Loads fonts and computes cell metrics. `dpi` is only used when the
    /// configuration leaves it at zero.
    pub fn new(config: &FontConfig, dpi: f32, size_override: Option<f32>) -> Result<Self, String> {
        let size_points = size_override.unwrap_or(config.size);
        let dpi = if config.dpi > 0.0 {
            config.dpi
        } else {
            dpi.clamp(48.0, 384.0)
        };
        let size_px = (size_points * dpi / 72.0).max(1.0);

        let mut database = Database::new();
        database.load_system_fonts();

        let regular_id = query_face(&database, &config.normal, Weight::NORMAL, Style::Normal)
            .ok_or_else(|| format!("no font found for family {:?}", config.normal.family))?;
        let (regular_data, regular_index) = face_data(&database, regular_id)?;
        let regular_data = Arc::new(regular_data);
        let regular = LoadedFont {
            data: regular_data.clone(),
            index: regular_index,
            synthetic_bold: false,
        };

        let bold = load_style(
            &database,
            &config.normal,
            config.bold.as_ref(),
            Weight::BOLD,
            Style::Normal,
            &regular,
        );
        let italic = load_style(
            &database,
            &config.normal,
            config.italic.as_ref(),
            Weight::NORMAL,
            Style::Italic,
            &regular,
        );
        let bold_italic = load_style(
            &database,
            &config.normal,
            config.bold_italic.as_ref(),
            Weight::BOLD,
            Style::Italic,
            &regular,
        );

        let mut fonts = vec![regular, bold, italic, bold_italic];
        let primary = [0, 1, 2, 3];

        let font = fonts[0]
            .font()
            .ok_or_else(|| "the regular font could not be parsed".to_string())?;
        let metrics = font.metrics(&[]).scale(size_px);
        let ascent = metrics.ascent.max(1.0);
        let descent = metrics.descent.max(0.0);
        let leading = metrics.leading.max(0.0);
        let cell_height = config
            .cell_height
            .unwrap_or_else(|| (ascent + descent + leading).round().max(1.0) as u16)
            .max(1);
        let baseline = ascent + leading / 2.0;
        let glyph_metrics = font.glyph_metrics(&[]).scale(size_px);
        let charmap = font.charmap();
        let mut advance = glyph_metrics.advance_width(charmap.map('M'));
        if advance <= 0.0 {
            advance = glyph_metrics.advance_width(charmap.map('0'));
        }
        if advance <= 0.0 {
            advance = size_px * 0.6;
        }
        let cell_width = config
            .cell_width
            .unwrap_or_else(|| advance.floor().max(1.0) as u16)
            .max(1);

        let underline_offset = if metrics.underline_offset > 0.0 {
            metrics.underline_offset
        } else {
            size_px * 0.1
        };
        let stroke_size = if metrics.stroke_size > 0.0 {
            metrics.stroke_size.max(1.0)
        } else {
            1.0
        };

        let fallback_families = config.fallback.clone();
        let mut renderer = Self {
            cell_width,
            cell_height,
            baseline,
            underline_offset,
            stroke_size,
            hinting: config.hinting,
            size_px,
            glyph_offset_x: config.glyph_offset.x,
            glyph_offset_y: config.glyph_offset.y,
            cursor_thickness: 0.15,
            unfocused_hollow: true,
            fonts: std::mem::take(&mut fonts),
            primary,
            database,
            fallback_families,
            loaded_families: HashMap::new(),
            glyph_cache: HashMap::new(),
            fallback_cache: HashMap::new(),
            scale_context: ScaleContext::new(),
        };
        renderer.remember_primary_families(config);
        Ok(renderer)
    }

    pub fn set_cursor_options(&mut self, thickness: f32, unfocused_hollow: bool) {
        self.cursor_thickness = thickness.clamp(0.02, 1.0);
        self.unfocused_hollow = unfocused_hollow;
    }

    pub fn cell_width(&self) -> u16 {
        self.cell_width
    }

    pub fn cell_height(&self) -> u16 {
        self.cell_height
    }

    /// Draws a single line of text with the regular font. Used for overlays
    /// such as the search bar and the context menu.
    pub fn draw_label(&mut self, canvas: &mut Canvas, text: &str, x: i32, y: i32, color: Rgb) {
        self.draw_text_str(canvas, text, x, y, color, 0);
    }

    /// Marks the configured primary family as already loaded, so fallback does
    /// not duplicate it.
    fn remember_primary_families(&mut self, config: &FontConfig) {
        self.loaded_families
            .insert(config.normal.family.to_ascii_lowercase(), Some(0));
        if let Some(bold) = &config.bold {
            self.loaded_families
                .insert(bold.family.to_ascii_lowercase(), Some(1));
        }
        if let Some(italic) = &config.italic {
            self.loaded_families
                .insert(italic.family.to_ascii_lowercase(), Some(2));
        }
        if let Some(bold_italic) = &config.bold_italic {
            self.loaded_families
                .insert(bold_italic.family.to_ascii_lowercase(), Some(3));
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        terminal: &Terminal,
        damage: &crate::vt::Damage,
        padding_x: i32,
        padding_y: i32,
        cursor_on: bool,
        focused: bool,
        background_alpha: u8,
    ) {
        let theme = terminal.theme();
        let background = pixel(canvas, theme.background, background_alpha);
        if damage.all {
            canvas.fill(background);
        }
        let origin_x = padding_x;
        let origin_y = padding_y;
        let cell_w = self.cell_width as i32;
        let cell_h = self.cell_height as i32;
        let rows = terminal.height();
        let has_selection = terminal.has_selection();
        let search = terminal.search_matches();
        let search_current = terminal.search_current();
        for row in 0..rows {
            if !damage.all && !damage.rows.get(row).copied().unwrap_or(false) {
                continue;
            }
            let Some(line) = terminal.visible_line(row) else {
                continue;
            };
            let highlights: Vec<(usize, usize, bool)> = search
                .iter()
                .enumerate()
                .filter(|(_, item)| item.line == line.id)
                .map(|(index, item)| (item.start, item.end, Some(index) == search_current))
                .collect();
            let y0 = origin_y + row as i32 * cell_h;
            canvas.fill_rect(
                origin_x,
                y0,
                terminal.width() as i32 * cell_w,
                cell_h,
                background,
            );
            for col in 0..terminal.width() {
                let Some(cell) = line.cells.get(col) else {
                    break;
                };
                if cell.width == 0 {
                    continue;
                }
                let selected = has_selection && terminal.is_selected(line.id, col);
                let highlight = highlights
                    .iter()
                    .find(|(start, end, _)| col >= *start && col <= *end)
                    .map(|(_, _, current)| *current);
                self.draw_cell(
                    canvas,
                    terminal,
                    *cell,
                    origin_x + col as i32 * cell_w,
                    y0,
                    selected,
                    highlight,
                    background_alpha,
                );
            }
        }

        let saved = terminal.scrollback_len();
        if saved > 0 {
            let bar_width = scrollbar::width(self.cell_width) as i32;
            let bar_x = origin_x + terminal.width() as i32 * cell_w + 1;
            let track_height = rows as i32 * cell_h;
            let track = shade(theme.background, 0.6);
            canvas.fill_rect(
                bar_x,
                origin_y,
                bar_width - 1,
                track_height,
                pixel(canvas, track, background_alpha),
            );
            let (thumb_y, thumb_height) =
                scrollbar::thumb(rows, saved, terminal.scroll_offset(), track_height);
            let thumb_color = shade(theme.foreground, 0.7);
            canvas.fill_rect(
                bar_x,
                origin_y + thumb_y,
                bar_width - 1,
                thumb_height,
                pixel(canvas, thumb_color, 255),
            );
        }

        if cursor_on && terminal.scroll_offset() == 0 && terminal.cursor_visible() {
            let row = terminal.cursor.y;
            let col = terminal.cursor.x;
            if let Some(line) = terminal.visible_line(row)
                && let Some(cell) = line.cells.get(col).copied()
            {
                let x = origin_x + col as i32 * cell_w;
                let y = origin_y + row as i32 * cell_h;
                let style = terminal.cursor.style;
                let thickness = ((cell_w.min(cell_h) as f32) * self.cursor_thickness)
                    .max(1.0)
                    .round() as i32;
                let cursor_color = pixel(canvas, theme.cursor, 255);
                match style {
                    crate::vt::CursorStyle::Block => {
                        if focused || !self.unfocused_hollow {
                            canvas.fill_rect(x, y, cell_w, cell_h, cursor_color);
                            self.draw_text(canvas, terminal, cell, x, y, theme.cursor_text);
                        } else {
                            outline(canvas, x, y, cell_w, cell_h, cursor_color);
                        }
                    }
                    crate::vt::CursorStyle::Beam => {
                        canvas.fill_rect(x, y, thickness, cell_h, cursor_color);
                    }
                    crate::vt::CursorStyle::Underline => {
                        canvas.fill_rect(
                            x,
                            y + cell_h - thickness,
                            cell_w,
                            thickness,
                            cursor_color,
                        );
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_cell(
        &mut self,
        canvas: &mut Canvas,
        terminal: &Terminal,
        cell: Cell,
        x: i32,
        y: i32,
        selected: bool,
        highlight: Option<bool>,
        background_alpha: u8,
    ) {
        let theme = terminal.theme();
        let width = cell.width.max(1) as i32 * self.cell_width as i32;
        let height = self.cell_height as i32;
        let (mut fg, mut bg) = colors(terminal, cell);
        if selected {
            fg = theme.selection_foreground;
            bg = theme.selection_background;
        }
        if let Some(current) = highlight {
            bg = search_background(theme, current);
            fg = readable_on(bg);
        }
        let attrs = cell.attrs;
        if attrs.contains(Attrs::REVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if attrs.contains(Attrs::HIDDEN) {
            fg = bg;
        }
        let background = if canvas.argb {
            let alpha = if cell.bg == Color::Default
                && !selected
                && highlight.is_none()
                && !attrs.contains(Attrs::REVERSE)
            {
                background_alpha
            } else {
                255
            };
            argb(bg, alpha)
        } else {
            rgb(bg)
        };
        let foreground = if canvas.argb { argb(fg, 255) } else { rgb(fg) };
        canvas.fill_rect(x, y, width, height, background);

        let underline_y = y + (self.baseline + self.underline_offset).round() as i32;
        let stroke = self.stroke_size.round().max(1.0) as i32;
        if attrs.contains(Attrs::UNDERLINE) {
            canvas.fill_rect(x, underline_y, width, stroke, foreground);
        }
        if attrs.contains(Attrs::DOUBLE_UNDERLINE) {
            canvas.fill_rect(x, underline_y, width, stroke, foreground);
            canvas.fill_rect(x, underline_y + stroke + 1, width, stroke, foreground);
        }
        if attrs.contains(Attrs::STRIKETHROUGH) {
            let strike_y = y + (self.baseline - self.cell_height as f32 * 0.25).round() as i32;
            canvas.fill_rect(x, strike_y, width, stroke, foreground);
        }
        if attrs.contains(Attrs::OVERLINE) {
            canvas.fill_rect(x, y, width, stroke, foreground);
        }
        self.draw_text(canvas, terminal, cell, x, y, fg);
    }

    fn draw_text(
        &mut self,
        canvas: &mut Canvas,
        terminal: &Terminal,
        cell: Cell,
        x: i32,
        y: i32,
        color: Rgb,
    ) {
        let style = style_index(cell.attrs);
        // Most cells are a single char. Avoid a per-cell String allocation by
        // keeping the common case on the stack and only building a combined
        // buffer for grapheme clusters with trailing combining marks.
        let extra = if cell.extra != 0 {
            terminal.extra_text(cell.extra)
        } else {
            None
        };
        if cell.ch == ' ' && extra.is_none() {
            return;
        }
        let mut stack = [0u8; 8];
        let text: &str = match extra {
            None => cell.ch.encode_utf8(&mut stack),
            Some(extra) => {
                let mut combined = String::with_capacity(cell.ch.len_utf8() + extra.len());
                combined.push(cell.ch);
                combined.push_str(extra);
                // Borrow from a local that outlives the loop below.
                return self.draw_text_str(canvas, &combined, x, y, color, style);
            }
        };
        self.draw_text_str(canvas, text, x, y, color, style)
    }

    fn draw_text_str(
        &mut self,
        canvas: &mut Canvas,
        text: &str,
        x: i32,
        y: i32,
        color: Rgb,
        style: u8,
    ) {
        let baseline = y as f32 + self.baseline;
        let glyph_offset_x = self.glyph_offset_x.round() as i32;
        let glyph_offset_y = self.glyph_offset_y.round() as i32;
        let mut pen_x = x;
        for ch in text.chars() {
            let width = ch.width().unwrap_or(0);
            if ch == '\u{200d}' || (0xfe00..=0xfe0f).contains(&(ch as u32)) {
                continue;
            }
            let Some(glyph) = self.resolve_glyph(ch, style) else {
                if width > 0 {
                    missing_glyph(
                        canvas,
                        pen_x,
                        y,
                        self.cell_width as i32,
                        self.cell_height as i32,
                        if canvas.argb {
                            argb(color, 255)
                        } else {
                            rgb(color)
                        },
                    );
                }
                pen_x += width.min(2) as i32 * self.cell_width as i32;
                continue;
            };
            if let Some(bitmap) = self.rasterize(glyph) {
                let gx = pen_x + bitmap.left + glyph_offset_x;
                let gy = (baseline - bitmap.top as f32).round() as i32 + glyph_offset_y;
                match bitmap.content {
                    Content::Color => {
                        canvas.blend_rgba(gx, gy, bitmap.width, bitmap.height, &bitmap.data);
                    }
                    _ => {
                        canvas.blend_mask(gx, gy, bitmap.width, bitmap.height, &bitmap.data, color);
                    }
                }
            }
            pen_x += width.min(2) as i32 * self.cell_width as i32;
        }
    }

    fn resolve_glyph(&mut self, ch: char, style: u8) -> Option<GlyphRef> {
        if let Some(cached) = self.fallback_cache.get(&(ch, style)) {
            return *cached;
        }
        let mut found = None;
        let primary = self.primary[style as usize];
        if let Some(glyph) = glyph_in_font(&self.fonts[primary], ch) {
            found = Some(GlyphRef {
                font: primary as u32,
                glyph,
            });
        }
        if found.is_none() {
            let families = self.fallback_families.clone();
            for family in families {
                let Some(font_index) = self.load_family(&family) else {
                    continue;
                };
                if let Some(glyph) = glyph_in_font(&self.fonts[font_index as usize], ch) {
                    found = Some(GlyphRef {
                        font: font_index,
                        glyph,
                    });
                    break;
                }
            }
        }
        self.fallback_cache.insert((ch, style), found);
        found
    }

    fn load_family(&mut self, family: &str) -> Option<u32> {
        let key = family.to_ascii_lowercase();
        if let Some(value) = self.loaded_families.get(&key) {
            return *value;
        }
        let query = Query {
            families: &[family_value(family)],
            weight: Weight::NORMAL,
            stretch: Stretch::Normal,
            style: Style::Normal,
        };
        let Some(id) = self.database.query(&query) else {
            self.loaded_families.insert(key, None);
            return None;
        };
        let Ok((data, index)) = face_data(&self.database, id) else {
            self.loaded_families.insert(key, None);
            return None;
        };
        let font = LoadedFont {
            data: Arc::new(data),
            index,
            synthetic_bold: false,
        };
        let font_index = self.fonts.len() as u32;
        self.fonts.push(font);
        self.loaded_families.insert(key, Some(font_index));
        Some(font_index)
    }

    fn rasterize(&mut self, glyph: GlyphRef) -> Option<&GlyphBitmap> {
        let key = GlyphKey {
            font: glyph.font,
            glyph: glyph.glyph,
        };
        if !self.glyph_cache.contains_key(&key) {
            if self.glyph_cache.len() > 65536 {
                self.glyph_cache.clear();
            }
            let bitmap = self.rasterize_uncached(glyph);
            self.glyph_cache.insert(key, bitmap);
        }
        self.glyph_cache.get(&key).and_then(Option::as_ref)
    }

    fn rasterize_uncached(&mut self, glyph: GlyphRef) -> Option<GlyphBitmap> {
        let font = self.fonts[glyph.font as usize].font()?;
        let synthetic = self.fonts[glyph.font as usize].synthetic_bold;
        let size = self.size_px;
        let mut scaler = self
            .scale_context
            .builder(font)
            .size(size)
            .hint(self.hinting)
            .build();
        let mut render = Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Bitmap(StrikeWith::BestFit),
            Source::Outline,
        ]);
        render.format(swash::zeno::Format::Alpha);
        if synthetic {
            render.embolden((size * 0.03).max(1.0));
        }
        let image = render.render(&mut scaler, GlyphId::from(glyph.glyph))?;
        Some(GlyphBitmap {
            width: image.placement.width,
            height: image.placement.height,
            left: image.placement.left,
            top: image.placement.top,
            content: image.content,
            data: image.data,
        })
    }
}

fn family_value(name: &str) -> Family<'_> {
    match name.to_ascii_lowercase().as_str() {
        "monospace" => Family::Monospace,
        "sans-serif" | "sans" => Family::SansSerif,
        "serif" => Family::Serif,
        "cursive" => Family::Cursive,
        "fantasy" => Family::Fantasy,
        _ => Family::Name(name),
    }
}

fn style_of(desc: &FontDescription, weight: Weight, style: Style) -> (Weight, Style) {
    let Some(name) = &desc.style else {
        return (weight, style);
    };
    let name = name.to_ascii_lowercase();
    let weight = if name.contains("thin") {
        Weight::THIN
    } else if name.contains("extralight") || name.contains("ultralight") {
        Weight::EXTRA_LIGHT
    } else if name.contains("light") {
        Weight::LIGHT
    } else if name.contains("medium") {
        Weight::MEDIUM
    } else if name.contains("semibold") || name.contains("demibold") {
        Weight::SEMIBOLD
    } else if name.contains("extrabold") || name.contains("ultrabold") {
        Weight::EXTRA_BOLD
    } else if name.contains("black") || name.contains("heavy") {
        Weight::BLACK
    } else if name.contains("bold") {
        Weight::BOLD
    } else if name.contains("normal") || name.contains("regular") || name.contains("book") {
        Weight::NORMAL
    } else {
        weight
    };
    let style = if name.contains("italic") {
        Style::Italic
    } else if name.contains("oblique") {
        Style::Oblique
    } else {
        style
    };
    (weight, style)
}

fn query_face(
    database: &Database,
    desc: &FontDescription,
    weight: Weight,
    style: Style,
) -> Option<fontdb::ID> {
    let (weight, style) = style_of(desc, weight, style);
    let family = family_value(&desc.family);
    let query = Query {
        families: &[family],
        weight,
        stretch: Stretch::Normal,
        style,
    };
    if let Some(id) = database.query(&query) {
        return Some(id);
    }
    if matches!(family, Family::Monospace) {
        // fontdb maps this generic to "Courier New"; without fontconfig
        // aliases that name can be absent even when a mono font is installed.
        let face = database.faces().find(|face| {
            face.families
                .iter()
                .any(|(name, _)| name.to_ascii_lowercase().contains("mono"))
        })?;
        return Some(face.id);
    }
    None
}

fn face_data(database: &Database, id: fontdb::ID) -> Result<(Vec<u8>, u32), String> {
    database
        .with_face_data(id, |data, index| (data.to_vec(), index))
        .ok_or_else(|| "font data is unavailable".to_string())
}

#[allow(clippy::too_many_arguments)]
fn load_style(
    database: &Database,
    normal: &FontDescription,
    override_desc: Option<&FontDescription>,
    weight: Weight,
    style: Style,
    regular: &LoadedFont,
) -> LoadedFont {
    let description = override_desc.unwrap_or(normal);
    let id = query_face(database, description, weight, style);
    let Some(id) = id else {
        return LoadedFont {
            data: regular.data.clone(),
            index: regular.index,
            synthetic_bold: weight.0 > face_weight(regular),
        };
    };
    let Ok((data, index)) = face_data(database, id) else {
        return LoadedFont {
            data: regular.data.clone(),
            index: regular.index,
            synthetic_bold: weight.0 > face_weight(regular),
        };
    };
    let loaded = LoadedFont {
        data: Arc::new(data),
        index,
        synthetic_bold: false,
    };
    let synthetic_bold = weight.0 > face_weight(&loaded);
    LoadedFont {
        synthetic_bold,
        ..loaded
    }
}

fn face_weight(font: &LoadedFont) -> u16 {
    font.font()
        .map(|font| font.attributes().weight().0)
        .unwrap_or(0)
}

fn style_index(attrs: Attrs) -> u8 {
    let bold = attrs.contains(Attrs::BOLD) as u8;
    let italic = attrs.contains(Attrs::ITALIC) as u8;
    bold | (italic << 1)
}

fn glyph_in_font(font: &LoadedFont, ch: char) -> Option<u16> {
    let font = font.font()?;
    let glyph = font.charmap().map(ch);
    (glyph != 0).then_some(glyph)
}

fn colors(terminal: &Terminal, cell: Cell) -> (Rgb, Rgb) {
    let fg = resolve_color(terminal, cell.fg, true, cell.attrs);
    let bg = resolve_color(terminal, cell.bg, false, cell.attrs);
    (fg, bg)
}

fn resolve_color(terminal: &Terminal, color: Color, foreground: bool, attrs: Attrs) -> Rgb {
    let theme = terminal.theme();
    let dim = attrs.contains(Attrs::DIM);
    match color {
        Color::Default => {
            if foreground {
                if attrs.contains(Attrs::BOLD) {
                    theme.bright_foreground
                } else if dim {
                    theme.dim_foreground
                } else {
                    theme.foreground
                }
            } else {
                theme.background
            }
        }
        Color::Indexed(index) => {
            let mut index = index;
            if foreground && attrs.contains(Attrs::BOLD) && index < 8 {
                index += 8;
            }
            let palette = if dim {
                &theme.dim_palette
            } else {
                &theme.palette
            };
            palette[index as usize]
        }
        Color::Rgb(mut rgb) => {
            if dim {
                rgb = [
                    (rgb[0] as u16 * 2 / 3) as u8,
                    (rgb[1] as u16 * 2 / 3) as u8,
                    (rgb[2] as u16 * 2 / 3) as u8,
                ];
            }
            rgb
        }
    }
}

fn missing_glyph(canvas: &mut Canvas, x: i32, y: i32, width: i32, height: i32, color: u32) {
    let left = x + width / 4;
    let right = x + width - width / 4;
    let top = y + height / 4;
    let bottom = y + height - height / 4;
    canvas.fill_rect(left, top, right - left, 1, color);
    canvas.fill_rect(left, bottom, right - left, 1, color);
    canvas.fill_rect(left, top, 1, bottom - top, color);
    canvas.fill_rect(right - 1, top, 1, bottom - top, color);
}

fn search_background(theme: &Theme, current: bool) -> Rgb {
    if current {
        theme.search_current_background
    } else {
        theme.search_match_background
    }
}

fn readable_on(color: Rgb) -> Rgb {
    let luminance = (299 * color[0] as u32 + 587 * color[1] as u32 + 114 * color[2] as u32) / 1000;
    if luminance > 140 {
        [0, 0, 0]
    } else {
        [255, 255, 255]
    }
}

fn outline(canvas: &mut Canvas, x: i32, y: i32, width: i32, height: i32, color: u32) {
    canvas.fill_rect(x, y, width, 1, color);
    canvas.fill_rect(x, y + height - 1, width, 1, color);
    canvas.fill_rect(x, y, 1, height, color);
    canvas.fill_rect(x + width - 1, y, 1, height, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_clips_rectangles() {
        let mut canvas = Canvas::new(4, 4);
        canvas.fill_rect(-2, -2, 3, 3, 0xff0000);
        assert_eq!(canvas.pixels[0], 0xff0000);
        assert_eq!(canvas.pixels[3], 0);
        canvas.fill_rect(3, 3, 10, 10, 0x00ff00);
        assert_eq!(canvas.pixels[15], 0x00ff00);
    }

    #[test]
    fn alpha_blends_towards_the_source() {
        let mut canvas = Canvas::new(1, 1);
        canvas.fill(0x000000);
        canvas.blend_mask(0, 0, 1, 1, &[128], [255, 255, 255]);
        let pixel = canvas.pixels[0];
        assert!((0x7f7f7f..=0x808080).contains(&pixel));
    }

    #[test]
    fn argb_canvas_keeps_background_translucent() {
        let mut canvas = Canvas::new(2, 1);
        canvas.argb = true;
        canvas.fill(argb([0, 0, 0], 128));
        canvas.fill_rect(0, 0, 1, 1, argb([255, 255, 255], 255));
        assert_eq!(canvas.pixels[0], 0xffff_ffff);
        assert_eq!(canvas.pixels[1] >> 24, 128);
        canvas.blend_mask(1, 0, 1, 1, &[255], [255, 255, 255]);
        assert_eq!(canvas.pixels[1] >> 24, 255);
        assert_eq!(canvas.pixels[1] & 0x00ff_ffff, 0x00ff_ffff);
    }

    #[test]
    fn monospace_uses_an_installed_mono_face() {
        let mut database = Database::new();
        database.push_face_info(fontdb::FaceInfo {
            id: fontdb::ID::dummy(),
            source: fontdb::Source::Binary(Arc::new(Vec::new())),
            index: 0,
            families: vec![(
                "DejaVu Sans Mono".into(),
                fontdb::Language::English_UnitedStates,
            )],
            post_script_name: String::new(),
            style: Style::Normal,
            weight: Weight::NORMAL,
            stretch: Stretch::Normal,
            monospaced: true,
        });
        let desc = FontDescription::default();
        assert!(query_face(&database, &desc, Weight::NORMAL, Style::Normal).is_some());
    }

    #[test]
    fn search_highlights_use_a_readable_foreground() {
        assert_eq!(readable_on([240, 198, 116]), [0, 0, 0]);
        assert_eq!(readable_on([16, 16, 16]), [255, 255, 255]);
    }

    #[test]
    fn search_colours_do_not_follow_the_ansi_palette() {
        let mut colors = crate::config::ColorsConfig::default();
        colors.normal.yellow = Some("#000000".into());
        colors.bright.blue = Some("#000000".into());
        let theme = Theme::from_config(&colors).unwrap();
        assert_eq!(search_background(&theme, false), [0xf0, 0xc6, 0x74]);
        assert_eq!(search_background(&theme, true), [0x7a, 0xa6, 0xda]);
    }

    #[test]
    fn current_search_match_is_distinct_from_the_others() {
        let theme = Theme::from_config(&crate::config::ColorsConfig::default()).unwrap();
        let current = search_background(&theme, true);
        let other = search_background(&theme, false);
        let distance: u32 = current
            .iter()
            .zip(other)
            .map(|(left, right)| left.abs_diff(right) as u32)
            .sum();
        assert!(distance > 200, "{current:?} is too close to {other:?}");
    }

    #[test]
    fn style_index_maps_attributes() {
        assert_eq!(style_index(Attrs::default()), 0);
        assert_eq!(style_index(Attrs::BOLD), 1);
        assert_eq!(style_index(Attrs::ITALIC), 2);
        assert_eq!(style_index(Attrs::BOLD.union(Attrs::ITALIC)), 3);
    }
}
