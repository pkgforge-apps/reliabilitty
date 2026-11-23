//! VT screen, scrollback, modes and selection.

mod dispatch;
mod parser;

use std::collections::{HashMap, VecDeque};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::{Rgb, Theme};

pub use parser::Parser;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// The configured default, foreground for text and background for cells.
    Default,
    Indexed(u8),
    Rgb(Rgb),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attrs(u16);

impl Attrs {
    pub const BOLD: Self = Self(1 << 0);
    pub const DIM: Self = Self(1 << 1);
    pub const ITALIC: Self = Self(1 << 2);
    pub const UNDERLINE: Self = Self(1 << 3);
    pub const BLINK: Self = Self(1 << 4);
    pub const REVERSE: Self = Self(1 << 5);
    pub const HIDDEN: Self = Self(1 << 6);
    pub const STRIKETHROUGH: Self = Self(1 << 7);
    pub const DOUBLE_UNDERLINE: Self = Self(1 << 8);
    pub const CURLY_UNDERLINE: Self = Self(1 << 9);
    pub const DOTTED_UNDERLINE: Self = Self(1 << 10);
    pub const DASHED_UNDERLINE: Self = Self(1 << 11);
    pub const OVERLINE: Self = Self(1 << 12);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Pen {
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
    /// Packed `Cell::meta` for the active hyperlink (0 when no link is active).
    pub link: u32,
}

impl Default for Pen {
    fn default() -> Self {
        Self {
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::default(),
            link: 0,
        }
    }
}

/// Tag bits for `Cell::meta`. Zero means neither combining text nor hyperlink.
const META_LINK_BIT: u32 = 1 << 31;
const META_BOTH_BIT: u32 = 1 << 30;
const META_INDEX_MASK: u32 = !(META_LINK_BIT | META_BOTH_BIT);

/// One character cell. Wide characters occupy a cell of width two followed by
/// a continuation cell of width zero.
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub ch: char,
    /// Packed side-table handle for combining marks and/or OSC 8 links.
    /// Zero means neither; bit 31 = link id, bit 30 = combined (extra, link)
    /// entry, otherwise 1-based extra text id.
    pub meta: u32,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
    /// Display width: zero for a continuation, one or two for a base.
    pub width: u8,
}

impl Cell {
    pub fn blank() -> Self {
        Self {
            ch: ' ',
            meta: 0,
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::default(),
            width: 1,
        }
    }

    pub fn is_blank(&self) -> bool {
        self.ch == ' ' && (self.meta == 0 || self.meta & META_LINK_BIT != 0) && self.width != 0
    }
}

/// A physical row, with a stable identity for selections.
#[derive(Clone, Debug)]
pub struct Line {
    /// Monotonic identity. Lines keep it when moved into scrollback.
    pub id: u64,
    /// Cells, always exactly the terminal width after a resize.
    pub cells: Vec<Cell>,
    pub wrapped: bool,
    /// True if any wide (width != 1) cell may exist on this line.
    has_wide: bool,
    /// Columns written since the last clear. Cells outside the range are
    /// guaranteed blank, so recycling only has to clear the range.
    written_min: usize,
    written_max: usize,
    revision: u64,
}

impl Line {
    fn new(id: u64, cells: Vec<Cell>) -> Self {
        Self {
            id,
            cells,
            wrapped: false,
            has_wide: false,
            written_min: usize::MAX,
            written_max: 0,
            revision: 0,
        }
    }

    fn mark_written(&mut self, col: usize) {
        if col < self.written_min {
            self.written_min = col;
        }
        if col > self.written_max {
            self.written_max = col;
        }
        self.revision = self.revision.wrapping_add(1);
    }

    fn mark_written_range(&mut self, from: usize, to: usize) {
        if from < self.written_min {
            self.written_min = from;
        }
        if to > self.written_max {
            self.written_max = to;
        }
        self.revision = self.revision.wrapping_add(1);
    }

    fn mark_all_written(&mut self) {
        self.written_min = 0;
        self.written_max = self.cells.len().saturating_sub(1);
        self.revision = self.revision.wrapping_add(1);
    }

    fn clear_written(&mut self) {
        if self.written_min <= self.written_max {
            self.cells[self.written_min..=self.written_max].fill(Cell::blank());
        }
        self.has_wide = false;
        self.written_min = usize::MAX;
        self.written_max = 0;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Blanks the untouched partner of any wide character overlapped at `x`
    /// (for a single-column overwrite) and extends `written_min..=written_max`
    /// if a neighbour outside the overwritten column was cleared.
    #[inline]
    fn clear_wide_at(&mut self, x: usize) {
        let old = self.cells[x];
        if old.width == 0 {
            if x > 0 && self.cells[x - 1].width == 2 {
                let bg = self.cells[x - 1].bg;
                self.cells[x - 1] = Cell {
                    bg,
                    ..Cell::blank()
                };
                self.mark_written(x - 1);
            }
        } else if old.width == 2 && x + 1 < self.cells.len() {
            let bg = self.cells[x + 1].bg;
            self.cells[x + 1] = Cell {
                bg,
                ..Cell::blank()
            };
            self.mark_written(x + 1);
        }
    }

    #[inline]
    fn write_ascii(&mut self, x: usize, byte: u8, fg: Color, bg: Color, attrs: Attrs, meta: u32) {
        if self.cells[x].width != 1 {
            self.clear_wide_at(x);
        }
        self.cells[x] = Cell {
            ch: byte as char,
            meta,
            fg,
            bg,
            attrs,
            width: 1,
        };
    }

    /// Writes an ASCII slice onto this line starting at `x0`. When the line
    /// contains no wide characters, skips per-cell width checks altogether.
    #[inline]
    fn write_ascii_slice(
        &mut self,
        x0: usize,
        bytes: &[u8],
        fg: Color,
        bg: Color,
        attrs: Attrs,
        meta: u32,
    ) {
        if !self.has_wide {
            let cells = &mut self.cells[x0..x0 + bytes.len()];
            for (cell, &byte) in cells.iter_mut().zip(bytes) {
                *cell = Cell {
                    ch: byte as char,
                    meta,
                    fg,
                    bg,
                    attrs,
                    width: 1,
                };
            }
            return;
        }
        for (index, &byte) in bytes.iter().enumerate() {
            self.write_ascii(x0 + index, byte, fg, bg, attrs, meta);
        }
    }
}

/// Metadata for a scrollback row stored inside `ScrollbackStore`'s chunked arena.
#[derive(Clone, Debug)]
struct ScrollbackEntry {
    id: u64,
    wrapped: bool,
    revision: u64,
    chunk: usize,
    offset: usize,
    len: usize,
    cols: usize,
}

/// Chunked arena for scrollback history.
///
/// Instead of allocating a `Vec<Cell>` per scrollback line (up to 10,000+
/// individual heap allocations when scrolling), each pushed line appends only
/// its written prefix into a contiguous chunk and the active screen line is
/// recycled immediately with zero allocations.
#[derive(Debug)]
struct ScrollbackStore {
    chunks: VecDeque<Vec<Cell>>,
    free_chunks: Vec<Vec<Cell>>,
    base_chunk: usize,
    entries: VecDeque<ScrollbackEntry>,
}

const SCROLLBACK_CHUNK_CELLS: usize = 32 * 1024;

impl ScrollbackStore {
    fn new() -> Self {
        Self {
            chunks: VecDeque::new(),
            free_chunks: Vec::new(),
            base_chunk: 0,
            entries: VecDeque::new(),
        }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn clear(&mut self) {
        self.chunks.clear();
        self.free_chunks.clear();
        self.base_chunk = 0;
        self.entries.clear();
    }

    fn push_line(&mut self, line: &Line, cols: usize, history: usize) {
        if history == 0 {
            return;
        }
        // Only store up to the rightmost written column (or 0 for a blank line).
        let stored_len = if line.written_min <= line.written_max {
            (line.written_max + 1).min(line.cells.len())
        } else {
            0
        };
        let (chunk, offset) = if stored_len == 0 {
            let current = self.base_chunk + self.chunks.len().saturating_sub(1);
            (current, 0)
        } else {
            let need_new = self
                .chunks
                .back()
                .is_none_or(|buf| buf.len() + stored_len > buf.capacity());
            if need_new {
                // Recycle an evicted chunk if one is free instead of returning
                // it to the allocator and re-faulting fresh pages on the next chunk.
                let mut recycled = None;
                while let Some(mut buf) = self.free_chunks.pop() {
                    if buf.capacity() >= stored_len {
                        buf.clear();
                        recycled = Some(buf);
                        break;
                    }
                }
                let buf = recycled
                    .unwrap_or_else(|| Vec::with_capacity(SCROLLBACK_CHUNK_CELLS.max(stored_len)));
                self.chunks.push_back(buf);
            }
            let chunk_id = self.base_chunk + self.chunks.len() - 1;
            let buf = self.chunks.back_mut().unwrap();
            let offset = buf.len();
            buf.extend_from_slice(&line.cells[..stored_len]);
            (chunk_id, offset)
        };
        self.entries.push_back(ScrollbackEntry {
            id: line.id,
            wrapped: line.wrapped,
            revision: line.revision,
            chunk,
            offset,
            len: stored_len,
            cols,
        });
        while self.entries.len() > history {
            self.entries.pop_front();
        }
        self.reclaim_chunks();
    }

    fn reclaim_chunks(&mut self) {
        let Some(oldest) = self.entries.front() else {
            self.chunks.clear();
            self.free_chunks.clear();
            return;
        };
        while !self.chunks.is_empty() && self.base_chunk < oldest.chunk {
            if let Some(mut buf) = self.chunks.pop_front() {
                if self.free_chunks.len() < 2 {
                    buf.clear();
                    self.free_chunks.push(buf);
                }
            }
            self.base_chunk += 1;
        }
    }

    fn cells_of(&self, entry: &ScrollbackEntry) -> &[Cell] {
        if entry.len == 0 {
            return &[];
        }
        let local = entry.chunk.saturating_sub(self.base_chunk);
        match self.chunks.get(local) {
            Some(buf) => &buf[entry.offset..entry.offset + entry.len],
            None => &[],
        }
    }

    fn get_cells(&self, index: usize) -> Option<(&ScrollbackEntry, &[Cell])> {
        let entry = self.entries.get(index)?;
        Some((entry, self.cells_of(entry)))
    }

    fn resize_all(&mut self, cols: usize) {
        for entry in &mut self.entries {
            entry.cols = cols;
            if entry.len > cols {
                entry.len = cols;
            }
            entry.revision = entry.revision.wrapping_add(1);
        }
    }
}

/// Borrowed view of either a live screen line or a compact scrollback row.
/// Cells beyond `cells.len()` up to `cols` are blank.
#[derive(Clone, Copy)]
pub struct LineView<'a> {
    pub id: u64,
    pub cells: &'a [Cell],
    pub cols: usize,
    pub wrapped: bool,
    revision: u64,
}

impl<'a> LineView<'a> {
    fn from_line(line: &'a Line) -> Self {
        Self {
            id: line.id,
            cells: &line.cells,
            cols: line.cells.len(),
            wrapped: line.wrapped,
            revision: line.revision,
        }
    }

    fn from_scrollback(entry: &ScrollbackEntry, cells: &'a [Cell]) -> Self {
        Self {
            id: entry.id,
            cells,
            cols: entry.cols.max(1),
            wrapped: entry.wrapped,
            revision: entry.revision,
        }
    }

    #[inline]
    pub fn cell(&self, col: usize) -> Option<Cell> {
        if col >= self.cols {
            return None;
        }
        Some(self.cells.get(col).copied().unwrap_or_else(Cell::blank))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorStyle {
    Block,
    Beam,
    Underline,
}

#[derive(Clone, Copy, Debug)]
pub struct Cursor {
    pub x: usize,
    pub y: usize,
    pub style: CursorStyle,
    pub visible: bool,
    pub blinking: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseMode {
    #[default]
    Off,
    X10,
    Button,
    /// Press, release and motion while a button is held.
    ButtonEvent,
    AnyEvent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseEncoding {
    #[default]
    X10,
    Utf8,
    /// SGR `CSI < b ; x ; y M` form.
    Sgr,
    /// urxvt `CSI b ; x ; y M` form.
    Urxvt,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MouseMods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub line: u64,
    pub col: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionMode {
    Char,
    Word,
    Line,
}

#[derive(Clone, Debug)]
pub struct Selection {
    pub start: Anchor,
    pub end: Anchor,
    pub mode: SelectionMode,
}

/// One match found by the scrollback search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    /// Identity of the line that holds the match.
    pub line: u64,
    /// First column of the match.
    pub start: usize,
    /// Last column of the match.
    pub end: usize,
}

#[derive(Clone, Debug)]
struct CachedLine {
    revision: u64,
    matches: Vec<SearchMatch>,
}

#[derive(Clone, Debug, Default)]
struct SearchState {
    query: String,
    matches: Vec<SearchMatch>,
    current: usize,
    lines: HashMap<u64, CachedLine>,
}

#[derive(Clone, Debug, Default)]
pub struct Damage {
    pub all: bool,
    pub rows: Vec<bool>,
    /// Whole-screen rows scrolled since the last frame. Positive is up,
    /// negative is down. Zero means no uniform shift happened.
    pub scroll: i32,
}

impl Damage {
    pub fn is_empty(&self) -> bool {
        !self.all && !self.rows.iter().any(|row| *row)
    }

    /// Marks every row dirty, reusing the existing allocation.
    pub fn set_all(&mut self, rows: usize) {
        self.all = true;
        self.rows.clear();
        self.rows.resize(rows, true);
    }

    pub fn range(&self) -> Option<(usize, usize)> {
        if self.all {
            return Some((0, self.rows.len().saturating_sub(1)));
        }
        let first = self.rows.iter().position(|row| *row)?;
        let last = self.rows.iter().rposition(|row| *row)?;
        Some((first, last))
    }
}

/// A clipboard write requested by the application through OSC 52.
#[derive(Clone, Debug)]
pub struct ClipboardWrite {
    /// X selection number: 0 is CLIPBOARD, 1 is PRIMARY.
    pub selection: u8,
    pub text: String,
}

/// Saved cursor and pen for DECSC and alternate-screen switching.
#[derive(Clone, Debug)]
struct SavedCursor {
    x: usize,
    y: usize,
    pen: Pen,
    origin: bool,
    autowrap: bool,
    charsets: [Charset; 4],
    active_charset: usize,
    wrap_pending: bool,
}

/// Character set selected into one of G0 through G3.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Charset {
    #[default]
    Ascii,
    Graphics,
    UnitedKingdom,
}

pub struct Terminal {
    pub cols: usize,
    pub rows: usize,
    pub cell_width: u16,
    pub cell_height: u16,
    pub window_width: u16,
    pub window_height: u16,
    main: VecDeque<Line>,
    alt: Option<VecDeque<Line>>,
    scrollback: ScrollbackStore,
    history: usize,
    pub cursor: Cursor,
    pub pen: Pen,
    saved: Option<SavedCursor>,
    alt_saved: Option<SavedCursor>,
    scroll_top: usize,
    scroll_bottom: usize,
    /// Rotation of the scroll region's line slots. Logical row `row` lives in
    /// physical slot `physical_row(row)`; scrolling a partial region only moves
    /// this counter, so the line buffer is never shifted.
    region_offset: usize,
    tabs: Vec<bool>,
    charsets: [Charset; 4],
    active_charset: usize,
    single_shift: Option<usize>,
    insert_mode: bool,
    newline_mode: bool,
    origin_mode: bool,
    autowrap: bool,
    reverse_video: bool,
    app_cursor: bool,
    app_keypad: bool,
    bracketed_paste: bool,
    focus_events: bool,
    mouse_mode: MouseMode,
    /// Kitty keyboard protocol flags and push/pop stacks, per screen.
    kitty_flags: [u32; 2],
    kitty_stack: [Vec<u32>; 2],
    mouse_encoding: MouseEncoding,
    title: String,
    title_dirty: bool,
    response: Vec<u8>,
    bell: bool,
    theme: Theme,
    default_theme: Theme,
    default_cursor_style: CursorStyle,
    default_cursor_blink: bool,
    clipboard_writes: Vec<ClipboardWrite>,
    clipboard_read: Option<u8>,
    scroll_offset: usize,
    dirty: Vec<bool>,
    spare_dirty: Vec<bool>,
    dirty_all: bool,
    pending_scroll: i32,
    line_id_next: u64,
    extras: Vec<Box<str>>,
    extras_map: HashMap<Box<str>, u32>,
    /// Scrollback lines need reflowing after a width change.
    pending_scrollback_resize: bool,
    /// OSC 8 hyperlink targets, one-based so zero means no link.
    link_targets: Vec<Box<str>>,
    link_targets_map: HashMap<Box<str>, u32>,
    /// Side table for cells that carry both combining text and a hyperlink.
    meta_pairs: Vec<(u32, u32)>,
    meta_pairs_map: HashMap<(u32, u32), u32>,
    active_link: u32,
    last_printed: Option<(usize, usize)>,
    wrap_pending: bool,
    last_graphic: char,
    selection: Option<Selection>,
    search: Option<SearchState>,
    resize_request: Option<(usize, usize)>,
}

impl Terminal {
    pub fn new(
        cols: usize,
        rows: usize,
        history: usize,
        theme: Theme,
        cursor_style: CursorStyle,
        cursor_blink: bool,
    ) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let mut terminal = Self {
            cols,
            rows,
            cell_width: 8,
            cell_height: 16,
            window_width: 0,
            window_height: 0,
            main: VecDeque::new(),
            alt: None,
            scrollback: ScrollbackStore::new(),
            history,
            cursor: Cursor {
                x: 0,
                y: 0,
                style: cursor_style,
                visible: true,
                blinking: cursor_blink,
            },
            pen: Pen::default(),
            saved: None,
            alt_saved: None,
            scroll_top: 0,
            scroll_bottom: rows - 1,
            region_offset: 0,
            tabs: Vec::new(),
            charsets: [Charset::Ascii; 4],
            active_charset: 0,
            single_shift: None,
            insert_mode: false,
            newline_mode: false,
            origin_mode: false,
            autowrap: true,
            reverse_video: false,
            app_cursor: false,
            app_keypad: false,
            bracketed_paste: false,
            focus_events: false,
            mouse_mode: MouseMode::Off,
            kitty_flags: [0, 0],
            kitty_stack: [Vec::new(), Vec::new()],
            mouse_encoding: MouseEncoding::X10,
            title: "Reliabilitty".to_string(),
            title_dirty: false,
            response: Vec::new(),
            bell: false,
            theme: theme.clone(),
            default_theme: theme,
            default_cursor_style: cursor_style,
            default_cursor_blink: cursor_blink,
            clipboard_writes: Vec::new(),
            clipboard_read: None,
            scroll_offset: 0,
            dirty: vec![false; rows],
            spare_dirty: Vec::new(),
            dirty_all: true,
            pending_scroll: 0,
            line_id_next: 0,
            extras: Vec::new(),
            extras_map: HashMap::new(),
            pending_scrollback_resize: false,
            link_targets: Vec::new(),
            link_targets_map: HashMap::new(),
            meta_pairs: Vec::new(),
            meta_pairs_map: HashMap::new(),
            active_link: 0,
            last_printed: None,
            wrap_pending: false,
            last_graphic: ' ',
            selection: None,
            search: None,
            resize_request: None,
        };
        terminal.reset_tabs();
        for _ in 0..rows {
            let line = terminal.new_line();
            terminal.main.push_back(line);
        }
        terminal
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    pub fn in_alt(&self) -> bool {
        self.alt.is_some()
    }

    pub fn width(&self) -> usize {
        self.cols
    }

    pub fn height(&self) -> usize {
        self.rows
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    #[cfg(test)]
    pub fn scrollback_cells_len(&self, index: usize) -> usize {
        self.scrollback.entries[index].cols
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn take_title(&mut self) -> Option<String> {
        if std::mem::take(&mut self.title_dirty) {
            Some(self.title.clone())
        } else {
            None
        }
    }

    pub fn mouse_mode(&self) -> MouseMode {
        self.mouse_mode
    }

    fn kitty_index(&self) -> usize {
        usize::from(self.in_alt())
    }

    pub fn kitty_flags(&self) -> u32 {
        self.kitty_flags[self.kitty_index()]
    }

    /// Applies `CSI = flags ; mode u` to the current screen.
    pub fn set_kitty_flags(&mut self, flags: u32, mode: u32) {
        let index = self.kitty_index();
        let current = &mut self.kitty_flags[index];
        match mode {
            2 => *current |= flags,
            3 => *current &= !flags,
            _ => *current = flags,
        }
    }

    /// Pushes the current flags; the supplied flags become active.
    pub fn push_kitty_flags(&mut self, flags: u32) {
        let index = self.kitty_index();
        let stack = &mut self.kitty_stack[index];
        if stack.len() >= 8 {
            stack.remove(0);
        }
        stack.push(self.kitty_flags[index]);
        self.kitty_flags[index] = flags;
    }

    /// Pops `count` entries; an empty stack resets the flags to zero.
    pub fn pop_kitty_flags(&mut self, count: usize) {
        let index = self.kitty_index();
        for _ in 0..count {
            match self.kitty_stack[index].pop() {
                Some(flags) => self.kitty_flags[index] = flags,
                None => {
                    self.kitty_flags[index] = 0;
                    break;
                }
            }
        }
    }

    pub fn answer_kitty_flags(&mut self) {
        let flags = self.kitty_flags();
        self.response
            .extend_from_slice(format!("\x1b[?{flags}u").as_bytes());
    }

    pub fn app_cursor(&self) -> bool {
        self.app_cursor
    }

    pub fn app_keypad(&self) -> bool {
        self.app_keypad
    }

    pub fn cursor_visible(&self) -> bool {
        self.cursor.visible
    }

    pub fn lines(&self) -> &VecDeque<Line> {
        match &self.alt {
            Some(alt) => alt,
            None => &self.main,
        }
    }

    fn lines_mut(&mut self) -> &mut VecDeque<Line> {
        match &mut self.alt {
            Some(alt) => alt,
            None => &mut self.main,
        }
    }

    fn region_len(&self) -> usize {
        self.scroll_bottom.saturating_sub(self.scroll_top) + 1
    }

    /// Physical slot holding a logical row. Rows outside the scroll region are
    /// never rotated.
    #[inline]
    fn physical_row(&self, row: usize) -> usize {
        if self.region_offset == 0 || row < self.scroll_top || row > self.scroll_bottom {
            return row;
        }
        self.scroll_top + (row - self.scroll_top + self.region_offset) % self.region_len()
    }

    #[inline]
    fn line(&self, row: usize) -> &Line {
        &self.lines()[self.physical_row(row)]
    }

    #[inline]
    fn line_mut(&mut self, row: usize) -> &mut Line {
        let physical = self.physical_row(row);
        &mut self.lines_mut()[physical]
    }

    /// Rotate the region's line slots back so logical and physical rows match.
    /// Called before structural operations that address lines by physical row.
    fn normalize_region(&mut self) {
        if self.region_offset == 0 {
            return;
        }
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        let offset = self.region_offset;
        self.lines_mut().make_contiguous()[top..=bottom].rotate_left(offset);
        self.region_offset = 0;
    }

    fn find_line(&self, id: u64) -> Option<LineView<'_>> {
        if let Some(line) = self.lines().iter().find(|line| line.id == id) {
            return Some(LineView::from_line(line));
        }
        self.scrollback
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| LineView::from_scrollback(entry, self.scrollback.cells_of(entry)))
    }

    fn all_lines(&self) -> Vec<LineView<'_>> {
        let active = self.lines();
        let mut lines = Vec::with_capacity(self.scrollback.len() + active.len());
        for entry in &self.scrollback.entries {
            lines.push(LineView::from_scrollback(
                entry,
                self.scrollback.cells_of(entry),
            ));
        }
        for row in 0..self.rows {
            lines.push(LineView::from_line(&active[self.physical_row(row)]));
        }
        lines
    }

    fn search_lines(&self) -> Vec<LineView<'_>> {
        if self.in_alt() {
            let active = self.lines();
            (0..self.rows)
                .map(|row| LineView::from_line(&active[self.physical_row(row)]))
                .collect()
        } else {
            self.all_lines()
        }
    }

    pub fn visible_line(&self, row: usize) -> Option<LineView<'_>> {
        if row >= self.rows {
            return None;
        }
        if self.scroll_offset == 0 {
            return Some(LineView::from_line(self.line(row)));
        }
        let saved = self.scrollback.len();
        let start = saved.saturating_sub(self.scroll_offset);
        let index = start + row;
        if index < saved {
            self.scrollback
                .get_cells(index)
                .map(|(entry, cells)| LineView::from_scrollback(entry, cells))
        } else {
            self.lines()
                .get(self.physical_row(index - saved))
                .map(LineView::from_line)
        }
    }

    pub fn anchor_at(&self, row: usize, col: usize) -> Option<Anchor> {
        let line = self.visible_line(row)?;
        let col = col.min(line.cols.saturating_sub(1));
        Some(Anchor { line: line.id, col })
    }

    fn unpack_meta(&self, meta: u32) -> (u32, u32) {
        if meta == 0 {
            (0, 0)
        } else if meta & META_LINK_BIT != 0 {
            (0, meta & META_INDEX_MASK)
        } else if meta & META_BOTH_BIT != 0 {
            self.meta_pairs
                .get((meta & META_INDEX_MASK) as usize)
                .copied()
                .unwrap_or((0, 0))
        } else {
            (meta, 0)
        }
    }

    fn pack_meta(&mut self, extra: u32, link: u32) -> u32 {
        match (extra, link) {
            (0, 0) => 0,
            (e, 0) => e,
            (0, l) => META_LINK_BIT | l,
            (e, l) => {
                if let Some(&index) = self.meta_pairs_map.get(&(e, l)) {
                    META_BOTH_BIT | index
                } else {
                    let index = self.meta_pairs.len() as u32;
                    self.meta_pairs.push((e, l));
                    self.meta_pairs_map.insert((e, l), index);
                    META_BOTH_BIT | index
                }
            }
        }
    }

    pub fn cell_extra(&self, cell: Cell) -> Option<&str> {
        let (extra, _) = self.unpack_meta(cell.meta);
        if extra == 0 {
            None
        } else {
            self.extras.get(extra as usize - 1).map(|text| &**text)
        }
    }

    pub fn cell_has_link(&self, cell: Cell) -> bool {
        cell.meta != 0 && self.unpack_meta(cell.meta).1 != 0
    }

    /// Starts an OSC 8 hyperlink, reusing an existing target id if the URI is
    /// identical. Returns the one-based id stored in cells.
    pub fn set_link(&mut self, target: &str) -> u32 {
        if let Some(&id) = self.link_targets_map.get(target) {
            self.active_link = id;
        } else {
            let boxed: Box<str> = target.into();
            let id = self.link_targets.len() as u32 + 1;
            self.link_targets.push(boxed.clone());
            self.link_targets_map.insert(boxed, id);
            self.active_link = id;
        }
        self.pen.link = META_LINK_BIT | self.active_link;
        self.active_link
    }

    /// Ends the active OSC 8 hyperlink.
    pub fn clear_link(&mut self) {
        self.active_link = 0;
        self.pen.link = 0;
    }

    /// Resolves the OSC 8 target id of the cell at a visible row and column.
    /// Continuation cells of wide characters inherit the base cell's link by
    /// walking back to it.
    pub fn link_at(&self, row: usize, col: usize) -> Option<&str> {
        let line = self.visible_line(row)?;
        let mut col = col.min(line.cols.saturating_sub(1));
        while col > 0 && line.cells.get(col).is_some_and(|cell| cell.width == 0) {
            col -= 1;
        }
        let (_, id) = self.unpack_meta(line.cells.get(col)?.meta);
        self.link_target(id)
    }

    fn link_target(&self, id: u32) -> Option<&str> {
        if id == 0 {
            return None;
        }
        self.link_targets
            .get(id as usize - 1)
            .map(|target| &**target)
    }

    pub fn has_selection(&self) -> bool {
        self.selection.is_some()
    }

    pub fn is_selected(&self, line: u64, col: usize) -> bool {
        let Some(selection) = &self.selection else {
            return false;
        };
        let (start, end) = ordered(selection.start, selection.end);
        if line < start.line || line > end.line {
            return false;
        }
        if line == start.line && col < start.col {
            return false;
        }
        if line == end.line && col > end.col {
            return false;
        }
        true
    }

    pub fn selection_text(&self) -> Option<String> {
        let selection = self.selection.as_ref()?;
        let (start, end) = ordered(selection.start, selection.end);
        let lines = self.all_lines();
        let start_index = lines.iter().position(|line| line.id == start.line)?;
        let end_index = lines.iter().position(|line| line.id == end.line)?;
        let (first, last) = if start_index <= end_index {
            (start_index, end_index)
        } else {
            (end_index, start_index)
        };
        let (start_col, end_col) = if start_index <= end_index {
            (start.col, end.col)
        } else {
            (end.col, start.col)
        };

        let mut text = String::new();
        for (index, line) in lines.iter().enumerate().take(last + 1).skip(first) {
            let from = if index == first { start_col } else { 0 };
            let to = if index == last {
                end_col
            } else {
                line.cols.saturating_sub(1)
            }
            .min(line.cols.saturating_sub(1));
            if from < line.cells.len() {
                let mut stored_to = to.min(line.cells.len().saturating_sub(1));
                let stored_from = from.min(stored_to);
                while stored_to > stored_from && line.cells[stored_to].is_blank() {
                    stored_to -= 1;
                }
                for cell in &line.cells[stored_from..=stored_to] {
                    if cell.width == 0 {
                        continue;
                    }
                    text.push(cell.ch);
                    if let Some(extra) = self.cell_extra(*cell) {
                        text.push_str(extra);
                    }
                }
            }
            if index != last && (selection.mode == SelectionMode::Line || !line.wrapped) {
                text.push('\n');
            }
        }
        (!text.is_empty()).then_some(text)
    }

    pub fn begin_selection(&mut self, row: usize, col: usize, mode: SelectionMode) {
        let Some(anchor) = self.anchor_at(row, col) else {
            return;
        };
        let (start, end) = match mode {
            SelectionMode::Char => (anchor, anchor),
            SelectionMode::Word => {
                let (from, to) = self.word_bounds(anchor);
                (from, to)
            }
            SelectionMode::Line => {
                let last = self
                    .find_line(anchor.line)
                    .map(|line| line.cols.saturating_sub(1))
                    .unwrap_or(0);
                (
                    Anchor {
                        line: anchor.line,
                        col: 0,
                    },
                    Anchor {
                        line: anchor.line,
                        col: last,
                    },
                )
            }
        };
        self.selection = Some(Selection { start, end, mode });
        self.mark_all();
    }

    pub fn update_selection(&mut self, row: usize, col: usize) {
        let Some(anchor) = self.anchor_at(row, col) else {
            return;
        };
        let Some(mode) = self.selection.as_ref().map(|selection| selection.mode) else {
            return;
        };
        let end = match mode {
            SelectionMode::Char => anchor,
            SelectionMode::Word => self.word_bounds(anchor).1,
            SelectionMode::Line => {
                let last = self
                    .find_line(anchor.line)
                    .map(|line| line.cols.saturating_sub(1))
                    .unwrap_or(0);
                Anchor {
                    line: anchor.line,
                    col: last,
                }
            }
        };
        if let Some(selection) = &mut self.selection {
            selection.end = end;
            self.mark_all();
        }
    }

    pub fn select_all(&mut self) {
        let Some(first) = self.visible_line(0) else {
            return;
        };
        let first_id = first.id;
        let Some(last) = self.visible_line(self.rows - 1) else {
            return;
        };
        let last_col = last.cols.saturating_sub(1);
        self.selection = Some(Selection {
            start: Anchor {
                line: first_id,
                col: 0,
            },
            end: Anchor {
                line: last.id,
                col: last_col,
            },
            mode: SelectionMode::Char,
        });
        self.mark_all();
    }

    pub fn clear_selection(&mut self) {
        if self.selection.take().is_some() {
            self.mark_all();
        }
    }

    fn collect_search_matches(
        &self,
        lines: &[LineView<'_>],
        needle: &[char],
        mut old_cache: HashMap<u64, CachedLine>,
    ) -> (Vec<(usize, SearchMatch)>, HashMap<u64, CachedLine>) {
        let mut cache = HashMap::with_capacity(lines.len());
        let mut indexed = Vec::new();
        for (index, &line) in lines.iter().enumerate() {
            let line_matches = match old_cache.remove(&line.id) {
                Some(cached) if cached.revision == line.revision => cached.matches,
                _ => self.scan_line(line, needle),
            };
            for item in &line_matches {
                indexed.push((index, *item));
            }
            cache.insert(
                line.id,
                CachedLine {
                    revision: line.revision,
                    matches: line_matches,
                },
            );
        }
        (indexed, cache)
    }

    /// Replaces the search text and selects the nearest match at or below the
    /// top of the current view, or the last match above it when none is below.
    /// An empty query clears every highlight.
    pub fn set_search_query(&mut self, query: &str) {
        let lines = self.search_lines();
        let needle = folded_query(query);
        let (indexed, cache) = self.collect_search_matches(&lines, &needle, HashMap::new());
        let top = self
            .visible_line(0)
            .and_then(|line| lines.iter().position(|item| item.id == line.id))
            .unwrap_or(0);
        let current = indexed
            .iter()
            .position(|(index, _)| *index >= top)
            .or_else(|| indexed.iter().rposition(|(index, _)| *index < top))
            .unwrap_or(0);
        self.search = Some(SearchState {
            query: query.to_string(),
            matches: indexed.into_iter().map(|(_, item)| item).collect(),
            current,
            lines: cache,
        });
        self.reveal_search_match();
        self.mark_all();
    }

    /// Recomputes matches after the screen changed, keeping the current match
    /// when it is still present. Only lines whose content changed are scanned.
    pub fn refresh_search(&mut self, query: &str) {
        if self
            .search
            .as_ref()
            .is_some_and(|state| state.query != query)
        {
            self.set_search_query(query);
            return;
        }
        let Some(state) = self.search.take() else {
            return;
        };
        let lines = self.search_lines();
        let needle = folded_query(query);
        let (indexed, cache) = self.collect_search_matches(&lines, &needle, state.lines);
        let matches: Vec<SearchMatch> = indexed.into_iter().map(|(_, item)| item).collect();
        let previous = state.matches.get(state.current).copied();
        let changed = matches != state.matches;
        let current = previous
            .and_then(|previous| matches.iter().position(|item| *item == previous))
            .unwrap_or_else(|| state.current.min(matches.len().saturating_sub(1)));
        self.search = Some(SearchState {
            query: state.query,
            matches,
            current,
            lines: cache,
        });
        if changed {
            self.mark_all();
        }
    }

    /// Scrolls the current search match into view.
    pub fn reveal_search(&mut self) {
        self.reveal_search_match();
    }

    /// Removes the search state and its highlights.
    pub fn clear_search(&mut self) {
        if self.search.take().is_some() {
            self.mark_all();
        }
    }

    /// Moves to the next or previous match, wrapping around at the ends.
    pub fn search_step(&mut self, forward: bool) -> bool {
        let Some(state) = &mut self.search else {
            return false;
        };
        if state.matches.is_empty() {
            return false;
        }
        let count = state.matches.len();
        state.current = if forward {
            (state.current + 1) % count
        } else {
            (state.current + count - 1) % count
        };
        self.mark_all();
        self.reveal_search_match();
        true
    }

    /// Returns one-based current and total match numbers, or zero when empty.
    pub fn search_summary(&self) -> (usize, usize) {
        match &self.search {
            Some(state) if !state.matches.is_empty() => (state.current + 1, state.matches.len()),
            _ => (0, 0),
        }
    }

    /// Matches currently highlighted on the screen.
    pub fn search_matches(&self) -> &[SearchMatch] {
        self.search
            .as_ref()
            .map(|state| state.matches.as_slice())
            .unwrap_or(&[])
    }

    /// Index of the current match within [`Self::search_matches`].
    pub fn search_current(&self) -> Option<usize> {
        self.search
            .as_ref()
            .filter(|state| !state.matches.is_empty())
            .map(|state| state.current)
    }

    fn scan_line(&self, line: LineView<'_>, needle: &[char]) -> Vec<SearchMatch> {
        let mut matches = Vec::new();
        if needle.is_empty() {
            return matches;
        }
        let (folded, columns) = self.folded_line(line);
        let mut offset = 0;
        while offset + needle.len() <= folded.len() {
            if folded[offset..].starts_with(needle) {
                matches.push(SearchMatch {
                    line: line.id,
                    start: columns[offset],
                    end: columns[offset + needle.len() - 1],
                });
                offset += needle.len();
            } else {
                offset += 1;
            }
        }
        matches
    }

    fn folded_line(&self, line: LineView<'_>) -> (Vec<char>, Vec<usize>) {
        let end = line
            .cells
            .iter()
            .rposition(|cell| !cell.is_blank() && cell.width != 0)
            .map_or(0, |index| index + 1);
        let mut chars = Vec::with_capacity(end);
        let mut columns = Vec::with_capacity(end);
        for (column, cell) in line.cells[..end].iter().enumerate() {
            if cell.width == 0 {
                continue;
            }
            chars.push(fold_search_char(cell.ch));
            columns.push(column);
            if let Some(extra) = self.cell_extra(*cell) {
                for ch in extra.chars() {
                    chars.push(fold_search_char(ch));
                    columns.push(column);
                }
            }
        }
        (chars, columns)
    }

    fn reveal_search_match(&mut self) {
        let Some(current) = self
            .search
            .as_ref()
            .and_then(|state| state.matches.get(state.current).copied())
        else {
            return;
        };
        if self.in_alt() {
            self.scroll_offset = 0;
            self.mark_all();
            return;
        }
        let Some(index) = self
            .all_lines()
            .iter()
            .position(|line| line.id == current.line)
        else {
            return;
        };
        let saved = self.scrollback.len();
        let target = self.rows.saturating_sub(1) / 3;
        let offset = if index < saved {
            (saved - index + target).min(saved)
        } else {
            0
        };
        self.scroll_offset = offset;
        self.mark_all();
    }

    fn word_bounds(&self, anchor: Anchor) -> (Anchor, Anchor) {
        let Some(line) = self.find_line(anchor.line) else {
            return (anchor, anchor);
        };
        let mut from = anchor.col.min(line.cols.saturating_sub(1));
        let mut to = from;
        if !is_word_char(self.cell_char(line, from)) {
            return (anchor, anchor);
        }
        while from > 0 && is_word_char(self.cell_char(line, from - 1)) {
            from -= 1;
        }
        while to + 1 < line.cols && is_word_char(self.cell_char(line, to + 1)) {
            to += 1;
        }
        (
            Anchor {
                line: anchor.line,
                col: from,
            },
            Anchor {
                line: anchor.line,
                col: to,
            },
        )
    }

    fn cell_char(&self, line: LineView<'_>, col: usize) -> char {
        line.cells.get(col).map(|cell| cell.ch).unwrap_or(' ')
    }

    pub fn scroll(&mut self, delta: i32) {
        if self.in_alt() {
            return;
        }
        let max = self.scrollback.len();
        let next = if delta < 0 {
            self.scroll_offset
                .saturating_add(delta.unsigned_abs() as usize)
        } else {
            self.scroll_offset.saturating_sub(delta as usize)
        }
        .min(max);
        if next != self.scroll_offset {
            self.scroll_offset = next;
            self.mark_all();
        }
    }

    /// Sets the scrollback offset directly.
    pub fn set_scroll_offset(&mut self, offset: usize) {
        let offset = offset.min(self.scrollback.len());
        if offset != self.scroll_offset {
            self.scroll_offset = offset;
            self.mark_all();
        }
    }

    pub fn scroll_to_top(&mut self) {
        if !self.in_alt() && self.scroll_offset != self.scrollback.len() {
            self.scroll_offset = self.scrollback.len();
            self.mark_all();
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        if self.scroll_offset != 0 {
            self.scroll_offset = 0;
            self.mark_all();
        }
    }

    pub fn scroll_page(&mut self, up: bool) {
        let page = self.rows.saturating_sub(1).max(1) as i32;
        self.scroll(if up { -page } else { page });
    }

    pub fn push_input(&mut self, bytes: &[u8]) {
        self.response.extend_from_slice(bytes);
    }

    pub fn paste(&mut self, text: &str) {
        let clean = text.replace("\r\n", "\r").replace('\0', "");
        if self.bracketed_paste {
            self.response.extend_from_slice(b"\x1b[200~");
            self.response.extend_from_slice(clean.as_bytes());
            self.response.extend_from_slice(b"\x1b[201~");
        } else {
            self.response.extend_from_slice(clean.as_bytes());
        }
    }

    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.response)
    }

    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell)
    }

    pub fn take_clipboard_writes(&mut self) -> Vec<ClipboardWrite> {
        std::mem::take(&mut self.clipboard_writes)
    }

    pub fn take_clipboard_read(&mut self) -> Option<u8> {
        self.clipboard_read.take()
    }

    /// Takes a window size requested by CSI 8 t.
    pub fn take_resize_request(&mut self) -> Option<(usize, usize)> {
        self.resize_request.take()
    }

    pub fn answer_clipboard_read(&mut self, selection: u8, text: Option<&str>) {
        let payload = text
            .map(|text| base64_encode(text.as_bytes()))
            .unwrap_or_else(|| "?".to_string());
        self.response.extend_from_slice(
            format!("\x1b]52;{};{}\x1b\\", selection_name(selection), payload).as_bytes(),
        );
    }

    /// Reports focus changes when the application asked for them.
    pub fn focus_changed(&mut self, focused: bool) {
        if self.focus_events {
            self.response
                .extend_from_slice(if focused { b"\x1b[I" } else { b"\x1b[O" });
        }
    }

    pub fn mouse_report(
        &self,
        button: u8,
        col: usize,
        row: usize,
        pressed: bool,
        motion: bool,
        mods: MouseMods,
    ) -> Option<Vec<u8>> {
        if self.mouse_mode == MouseMode::Off {
            return None;
        }
        if self.mouse_mode == MouseMode::X10 && (!pressed || button >= 3) {
            return None;
        }
        if motion
            && self.mouse_mode != MouseMode::ButtonEvent
            && self.mouse_mode != MouseMode::AnyEvent
        {
            return None;
        }
        let x = col.min(self.cols.saturating_sub(1)) as u16 + 1;
        let y = row.min(self.rows.saturating_sub(1)) as u16 + 1;
        let mut code = if motion { 32 + 3 } else { button };
        if mods.shift {
            code += 4;
        }
        if mods.alt {
            code += 8;
        }
        if mods.ctrl {
            code += 16;
        }
        let release = !pressed && !motion && button < 64;
        match self.mouse_encoding {
            MouseEncoding::Sgr => {
                let final_byte = if release { 'm' } else { 'M' };
                Some(format!("\x1b[<{code};{x};{y}{final_byte}").into_bytes())
            }
            MouseEncoding::Urxvt => {
                let code = if release { 3 + (code & 0x1c) } else { code };
                Some(format!("\x1b[{};{};{}M", code + 32, x, y).into_bytes())
            }
            MouseEncoding::Utf8 => {
                let code = if release { 3 + (code & 0x1c) } else { code };
                let mut out = Vec::new();
                out.extend_from_slice(b"\x1b[M");
                out.push(code + 32);
                push_utf8_coord(&mut out, x as u32 + 32);
                push_utf8_coord(&mut out, y as u32 + 32);
                Some(out)
            }
            MouseEncoding::X10 => {
                let code = if release { 3 + (code & 0x1c) } else { code };
                Some(vec![
                    0x1b,
                    b'[',
                    b'M',
                    code + 32,
                    (x.min(223) + 32) as u8,
                    (y.min(223) + 32) as u8,
                ])
            }
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize, cell_width: u16, cell_height: u16) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        self.cell_width = cell_width;
        self.cell_height = cell_height;
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.normalize_region();
        let cols_changed = self.cols != cols;
        self.cols = cols;
        self.rows = rows;
        for line in &mut self.main {
            resize_line(line, cols);
        }
        if let Some(alt) = &mut self.alt {
            for line in alt {
                resize_line(line, cols);
            }
        }
        // Reflowing every scrollback line is O(history) and can take several
        // milliseconds. Defer it until the resize settles so interactive drags
        // stay responsive; visible lines are resized immediately above.
        if cols_changed {
            self.pending_scrollback_resize = true;
        }
        while self.main.len() < rows {
            let line = self.new_line();
            self.main.push_back(line);
        }
        self.main.truncate(rows);
        if self.alt.as_ref().is_some_and(|alt| alt.len() < rows) {
            let mut extra = VecDeque::new();
            while self.alt.as_ref().map_or(0, VecDeque::len) + extra.len() < rows {
                extra.push_back(self.new_line());
            }
            if let Some(alt) = &mut self.alt {
                alt.extend(extra);
            }
        }
        if let Some(alt) = &mut self.alt {
            alt.truncate(rows);
        }
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
        self.cursor.x = self.cursor.x.min(cols - 1);
        self.cursor.y = self.cursor.y.min(rows - 1);
        self.scroll_offset = self.scroll_offset.min(self.scrollback.len());
        self.tabs.resize(cols, false);
        self.reset_tabs();
        self.dirty = vec![false; rows];
        self.dirty_all = true;
        self.last_printed = None;
        self.wrap_pending = false;
    }

    /// Reflows scrollback lines after a width change. Called once the resize
    /// settles so interactive drags do not pay this cost on every event.
    pub fn finish_deferred_resize(&mut self) {
        if !self.pending_scrollback_resize {
            return;
        }
        self.pending_scrollback_resize = false;
        self.scrollback.resize_all(self.cols);
    }

    pub fn set_window_pixels(&mut self, width: u16, height: u16) {
        self.window_width = width;
        self.window_height = height;
    }

    pub fn take_damage(&mut self) -> Damage {
        let all = std::mem::take(&mut self.dirty_all);
        let scroll = std::mem::take(&mut self.pending_scroll);
        let rows = std::mem::take(&mut self.dirty);
        // Refill the dirty array from the recycled buffer instead of allocating.
        self.dirty = std::mem::take(&mut self.spare_dirty);
        self.dirty.clear();
        self.dirty.resize(self.rows, false);
        Damage { all, rows, scroll }
    }

    /// Returns a consumed damage buffer so its allocation can be reused.
    pub fn recycle_damage(&mut self, damage: Damage) {
        self.spare_dirty = damage.rows;
    }

    pub fn has_damage(&self) -> bool {
        self.dirty_all || self.pending_scroll != 0 || self.dirty.iter().any(|dirty| *dirty)
    }

    pub fn mark_all(&mut self) {
        self.dirty_all = true;
    }

    pub fn mark_row_public(&mut self, row: usize) {
        self.mark_row(row);
    }

    pub fn mark_cursor_dirty(&mut self) {
        self.mark_row(self.cursor.y);
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme.clone();
        self.default_theme = theme;
        self.mark_all();
    }

    /// Sets the initial title without marking it as an OSC change.
    pub fn set_initial_title(&mut self, title: &str) {
        self.title = title.to_string();
        self.title_dirty = false;
    }

    fn mark_row(&mut self, row: usize) {
        if let Some(dirty) = self.dirty.get_mut(row) {
            *dirty = true;
        } else {
            self.dirty_all = true;
        }
    }

    fn next_line_id(&mut self) -> u64 {
        let id = self.line_id_next;
        self.line_id_next = self.line_id_next.wrapping_add(1);
        id
    }

    fn new_line(&mut self) -> Line {
        Line::new(self.next_line_id(), vec![Cell::blank(); self.cols])
    }

    fn reset_tabs(&mut self) {
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            *tab = index % 8 == 0;
        }
    }

    fn set_cursor(&mut self, x: usize, y: usize) {
        let old = self.cursor.y;
        self.cursor.x = x.min(self.cols - 1);
        self.cursor.y = y.min(self.rows - 1);
        self.wrap_pending = false;
        self.last_printed = None;
        self.mark_row(old);
        self.mark_row(self.cursor.y);
    }

    fn print(&mut self, ch: char) {
        // Fast path for the common case: printable ASCII in the normal charset,
        // no wrap pending, no insert mode, and room for one cell on this line.
        if (' '..='~').contains(&ch)
            && self.single_shift.is_none()
            && self.charsets[self.active_charset] == Charset::Ascii
            && !self.wrap_pending
            && !self.insert_mode
            && self.cursor.x + 1 < self.cols
        {
            let y = self.cursor.y;
            let x = self.cursor.x;
            let (fg, bg, attrs, meta) = (self.pen.fg, self.pen.bg, self.pen.attrs, self.pen.link);
            let line = self.line_mut(y);
            if line.cells[x].width == 1 {
                line.cells[x] = Cell {
                    ch,
                    meta,
                    fg,
                    bg,
                    attrs,
                    width: 1,
                };
                line.mark_written_range(x, x);
                line.wrapped = false;
                self.last_printed = Some((y, x));
                self.last_graphic = ch;
                self.mark_row(y);
                self.cursor.x = x + 1;
                return;
            }
        }
        let ch = self.map_charset(ch);
        let width = ch.width().unwrap_or(0);
        if width == 0 {
            self.add_combining(ch);
            return;
        }
        let width = width.min(2);
        if self.wrap_pending {
            if self.autowrap {
                self.index();
                self.cursor.x = 0;
            }
            self.wrap_pending = false;
        }
        if width == 2 && self.cursor.x + 1 >= self.cols {
            if self.autowrap {
                self.index();
                self.cursor.x = 0;
            } else {
                self.cursor.x = self.cols.saturating_sub(2);
            }
        }
        if self.insert_mode {
            self.insert_cells(width);
        }
        let y = self.cursor.y;
        let x = self.cursor.x;
        let cols = self.cols;
        let (fg, bg, attrs, meta) = (self.pen.fg, self.pen.bg, self.pen.attrs, self.pen.link);
        let cell = Cell {
            ch,
            meta,
            fg,
            bg,
            attrs,
            width: width as u8,
        };
        let line = self.line_mut(y);
        // Only neighbours outside the cells we overwrite need cleaning up.
        if width == 1 {
            if line.cells[x].width != 1 {
                line.clear_wide_at(x);
            }
        } else {
            if line.cells[x].width == 0 && x > 0 && line.cells[x - 1].width == 2 {
                let previous = line.cells[x - 1];
                line.cells[x - 1] = Cell {
                    bg: previous.bg,
                    ..Cell::blank()
                };
                line.mark_written(x - 1);
            }
            if line.cells.get(x + 1).is_some_and(|next| next.width == 2) && x + 2 < cols {
                let next = line.cells[x + 2];
                line.cells[x + 2] = Cell {
                    bg: next.bg,
                    ..Cell::blank()
                };
                line.mark_written(x + 2);
            }
            let mut continuation = cell;
            continuation.ch = ' ';
            continuation.width = 0;
            line.cells[x + 1] = continuation;
            line.has_wide = true;
        }
        line.cells[x] = cell;
        line.mark_written_range(x, x + width - 1);
        line.wrapped = false;
        self.last_printed = Some((y, x));
        self.last_graphic = ch;
        self.mark_row(y);
        self.cursor.x += width;
        if self.cursor.x >= self.cols {
            self.cursor.x = self.cols - 1;
            self.wrap_pending = true;
        }
    }

    /// Prints a run of printable ASCII without per-byte parser dispatch.
    /// Modes that need per-character work fall back to `print`.
    pub fn print_ascii_run(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if self.insert_mode
            || self.charsets[self.active_charset] != Charset::Ascii
            || self.single_shift.is_some()
        {
            for &byte in bytes {
                self.print(byte as char);
            }
            return;
        }
        let cols = self.cols;
        let (fg, bg, attrs, meta) = (self.pen.fg, self.pen.bg, self.pen.attrs, self.pen.link);
        let (region_top, region_bottom, region_offset) =
            (self.scroll_top, self.scroll_bottom, self.region_offset);
        let map = |row: usize| -> usize {
            if region_offset == 0 || row < region_top || row > region_bottom {
                row
            } else {
                region_top + (row - region_top + region_offset) % (region_bottom - region_top + 1)
            }
        };
        let x0 = self.cursor.x.min(cols - 1);
        let end = x0 + bytes.len();
        let end_row = self.cursor.y + (end - 1) / cols;
        if self.autowrap
            && !self.wrap_pending
            && self.scroll_top == 0
            && end_row < self.scroll_bottom
        {
            let start_y = self.cursor.y;
            let last_col = (end - 1) % cols;
            let last = bytes[bytes.len() - 1] as char;
            {
                let lines = self.lines_mut();
                let mut x = x0;
                let mut physical = map(start_y);
                let mut offset = 0usize;
                while offset < bytes.len() {
                    let take = (bytes.len() - offset).min(cols - x);
                    let line = &mut lines[physical];
                    line.write_ascii_slice(x, &bytes[offset..offset + take], fg, bg, attrs, meta);
                    line.mark_written_range(x, x + take - 1);
                    line.wrapped = false;
                    x += take;
                    offset += take;
                    if x == cols {
                        x = 0;
                        physical += 1;
                        if physical > region_bottom {
                            physical = region_top;
                        }
                    }
                }
            }
            for row in start_y..=end_row {
                self.mark_row(row);
            }
            self.cursor.y = end_row;
            if end % cols == 0 {
                self.cursor.x = cols - 1;
                self.wrap_pending = true;
            } else {
                self.cursor.x = end % cols;
            }
            self.last_printed = Some((end_row, last_col));
            self.last_graphic = last;
            return;
        }
        let mut offset = 0;
        while offset < bytes.len() {
            if self.wrap_pending {
                if self.autowrap {
                    self.index();
                    self.cursor.x = 0;
                }
                self.wrap_pending = false;
                self.last_printed = None;
            }
            let y = self.cursor.y;
            let x0 = self.cursor.x.min(cols - 1);
            let take = (bytes.len() - offset).min(cols - x0);
            let segment = &bytes[offset..offset + take];
            let last = {
                let lines = self.lines_mut();
                let line = &mut lines[map(y)];
                line.write_ascii_slice(x0, segment, fg, bg, attrs, meta);
                line.mark_written_range(x0, x0 + segment.len() - 1);
                line.wrapped = false;
                segment[segment.len() - 1] as char
            };
            let end_x = x0 + segment.len();
            if end_x >= cols {
                self.cursor.x = cols - 1;
                self.wrap_pending = true;
            } else {
                self.cursor.x = end_x;
            }
            self.last_printed = Some((y, x0 + segment.len() - 1));
            self.last_graphic = last;
            self.mark_row(y);
            offset += segment.len();
        }
    }

    /// Prints text made of printable ASCII and non-combining width one or two
    /// characters without per-character dispatch. Anything else goes through
    /// `print`, so the result is identical to calling `print` per character.
    pub fn print_text_run(&mut self, text: &str) {
        if self.insert_mode
            || self.charsets[self.active_charset] != Charset::Ascii
            || self.single_shift.is_some()
        {
            for ch in text.chars() {
                self.print(ch);
            }
            return;
        }
        let cols = self.cols;
        let mut rest = text.chars().peekable();
        while let Some(&first) = rest.peek() {
            if self.wrap_pending {
                self.print(first);
                rest.next();
                continue;
            }
            let y = self.cursor.y;
            let start = self.cursor.x;
            let (fg, bg, attrs, meta) = (self.pen.fg, self.pen.bg, self.pen.attrs, self.pen.link);
            let mut x = start;
            let mut last = None;
            let mut last_x = start;
            {
                let line = self.line_mut(y);
                while let Some(&ch) = rest.peek() {
                    let width = if ch.is_ascii() || ('\u{2580}'..='\u{259f}').contains(&ch) {
                        1
                    } else {
                        ch.width().unwrap_or(0)
                    };
                    if (width != 1 && width != 2)
                        || x + width >= cols
                        || line.cells[x].width != 1
                        || (width == 2 && line.cells[x + 1].width != 1)
                    {
                        break;
                    }
                    let cell = Cell {
                        ch,
                        meta,
                        fg,
                        bg,
                        attrs,
                        width: width as u8,
                    };
                    line.cells[x] = cell;
                    if width == 2 {
                        line.cells[x + 1] = Cell {
                            ch: ' ',
                            width: 0,
                            ..cell
                        };
                        line.has_wide = true;
                    }
                    last = Some(ch);
                    last_x = x;
                    x += width;
                    rest.next();
                }
                if last.is_some() {
                    line.mark_written_range(start, x - 1);
                    line.wrapped = false;
                }
            }
            match last {
                Some(ch) => {
                    self.last_printed = Some((y, last_x));
                    self.last_graphic = ch;
                    self.mark_row(y);
                    self.cursor.x = x;
                }
                None => {
                    self.print(first);
                    rest.next();
                }
            }
        }
    }

    fn add_combining(&mut self, ch: char) {
        let (y, x) = match self.last_printed {
            Some(position) => position,
            None => {
                let y = self.cursor.y;
                let x = self.cursor.x;
                if x == 0 && self.cursor.y == 0 {
                    (y, x)
                } else if x > 0 {
                    (y, x - 1)
                } else {
                    (y.saturating_sub(1), self.cols - 1)
                }
            }
        };
        self.append_to_cluster(y, x, ch);
    }

    fn append_to_cluster(&mut self, y: usize, x: usize, ch: char) {
        if y >= self.rows || x >= self.cols {
            return;
        }
        let mut cell = self.line(y).cells[x];
        if cell.width == 0 && x > 0 {
            return self.append_to_cluster(y, x - 1, ch);
        }
        let (_, link_id) = self.unpack_meta(cell.meta);
        let mut cluster = String::new();
        cluster.push(cell.ch);
        if let Some(extra) = self.cell_extra(cell) {
            cluster.push_str(extra);
        }
        cluster.push(ch);
        let old_width = cell.width;
        let new_width = UnicodeWidthStr::width(cluster.as_str()).clamp(1, 2) as u8;
        let extra_id = self.intern_extra(&cluster[cell.ch.len_utf8()..]);
        cell.meta = self.pack_meta(extra_id, link_id);
        cell.width = new_width;
        let cols = self.cols;
        let mut advance = false;
        {
            let line = self.line_mut(y);
            line.cells[x] = cell;
            line.mark_written(x);
            let next = x + 1;
            if next < cols {
                match (old_width, new_width) {
                    (1, 2) => {
                        let mut continuation = cell;
                        continuation.ch = ' ';
                        continuation.meta = if link_id != 0 {
                            META_LINK_BIT | link_id
                        } else {
                            0
                        };
                        continuation.width = 0;
                        line.cells[next] = continuation;
                        line.has_wide = true;
                        line.mark_written(next);
                        advance = true;
                    }
                    (2, 1) if line.cells[next].width == 0 => {
                        line.cells[next] = Cell {
                            bg: cell.bg,
                            ..Cell::blank()
                        };
                        line.mark_written(next);
                    }
                    _ => {}
                }
            }
        }
        if advance && self.cursor.y == y && self.cursor.x == x + 1 {
            self.cursor.x = (x + 2).min(self.cols - 1);
        }
        self.mark_row(y);
    }

    fn intern_extra(&mut self, text: &str) -> u32 {
        if text.is_empty() {
            return 0;
        }
        if let Some(id) = self.extras_map.get(text) {
            return *id;
        }
        let boxed: Box<str> = text.into();
        let id = self.extras.len() as u32 + 1;
        self.extras.push(boxed.clone());
        self.extras_map.insert(boxed, id);
        id
    }

    fn execute_control(&mut self, byte: u8) {
        match byte {
            0x07 => self.bell = true,
            0x08 => {
                if self.cursor.x > 0 {
                    self.set_cursor(self.cursor.x - 1, self.cursor.y);
                }
            }
            0x09 => self.tab_forward(1),
            0x0a..=0x0c => self.index(),
            0x0d => {
                self.set_cursor(0, self.cursor.y);
            }
            0x0e => self.active_charset = 1,
            0x0f => self.active_charset = 0,
            0x84 => self.index(),
            0x85 => {
                self.set_cursor(0, self.cursor.y);
                self.index();
            }
            0x88 => self.set_tab_stop(),
            0x8d => self.reverse_index(),
            _ => {}
        }
    }

    fn map_charset(&mut self, ch: char) -> char {
        let set = self
            .single_shift
            .take()
            .unwrap_or(self.active_charset)
            .min(3);
        match self.charsets[set] {
            Charset::Ascii => ch,
            Charset::UnitedKingdom => match ch {
                '#' => '\u{00a3}',
                _ => ch,
            },
            Charset::Graphics => match ch {
                '`' => '\u{25c6}',
                'a' => '\u{2592}',
                'b' => '\u{2409}',
                'c' => '\u{240c}',
                'd' => '\u{240d}',
                'e' => '\u{240a}',
                'f' => '\u{00b0}',
                'g' => '\u{00b1}',
                'h' => '\u{2424}',
                'i' => '\u{240b}',
                'j' => '\u{2518}',
                'k' => '\u{2510}',
                'l' => '\u{250c}',
                'm' => '\u{2514}',
                'n' => '\u{253c}',
                'o' => '\u{23ba}',
                'p' => '\u{23bb}',
                'q' => '\u{2500}',
                'r' => '\u{23bc}',
                's' => '\u{23bd}',
                't' => '\u{251c}',
                'u' => '\u{2524}',
                'v' => '\u{2534}',
                'w' => '\u{252c}',
                'x' => '\u{2502}',
                'y' => '\u{2264}',
                'z' => '\u{2265}',
                '{' => '\u{03c0}',
                '|' => '\u{2260}',
                '}' => '\u{00a3}',
                '~' => '\u{00b7}',
                _ => ch,
            },
        }
    }

    /// Moves the cursor to column 0 without touching rows.
    #[inline]
    pub(super) fn carriage_return(&mut self) {
        self.cursor.x = 0;
        self.wrap_pending = false;
        self.last_printed = None;
    }

    /// Moves the cursor to column 0 and down one line (CR+LF fast path).
    #[inline]
    pub(super) fn crlf(&mut self) {
        self.cursor.x = 0;
        self.wrap_pending = false;
        self.last_printed = None;
        if self.cursor.y == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor.y + 1 < self.rows {
            let old = self.cursor.y;
            self.cursor.y += 1;
            self.mark_row(old);
            self.mark_row(self.cursor.y);
        }
    }

    /// Moves the cursor down one line, scrolling at the bottom margin.
    #[inline]
    pub(super) fn index(&mut self) {
        if self.cursor.y == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor.y + 1 < self.rows {
            self.set_cursor(self.cursor.x, self.cursor.y + 1);
        }
        self.mark_row(self.cursor.y);
    }

    /// Moves the cursor up one line, scrolling at the top margin.
    fn reverse_index(&mut self) {
        if self.cursor.y == self.scroll_top {
            self.scroll_down(1);
        } else if self.cursor.y > 0 {
            self.set_cursor(self.cursor.x, self.cursor.y - 1);
        }
        self.mark_row(self.cursor.y);
    }

    fn scroll_up(&mut self, count: usize) {
        // Single-line full-screen scroll on the main screen is the common case
        // in streaming output. Archive into the chunked scrollback arena and
        // recycle the live line immediately with zero heap allocations.
        if count == 1
            && self.scroll_top == 0
            && self.scroll_bottom + 1 == self.rows
            && self.scroll_offset == 0
            && self.region_offset == 0
            && self.alt.is_none()
        {
            let mut line = self.main.pop_front().unwrap();
            if self.history != 0 {
                self.scrollback.push_line(&line, self.cols, self.history);
            }
            let id = self.next_line_id();
            blank_in_place(&mut line, id);
            self.main.push_back(line);
            self.pending_scroll += 1;
            self.dirty.copy_within(1.., 0);
            *self.dirty.last_mut().unwrap() = true;
            return;
        }
        self.scroll_region_up(self.scroll_top, self.scroll_bottom, count, true);
    }

    fn scroll_down(&mut self, count: usize) {
        self.scroll_region_down(self.scroll_top, self.scroll_bottom, count);
    }

    /// Scrolls a region up, saving full-screen lines to scrollback.
    fn scroll_region_up(&mut self, top: usize, bottom: usize, count: usize, allow_history: bool) {
        let count = count.min(bottom - top + 1);
        let full = top == 0 && bottom + 1 == self.rows;
        // A whole-screen scroll with no active scrollback view just shifts the
        // pixels up uniformly (also true on the alternate screen), so record
        // it and let the renderer memmove the canvas instead of redrawing all
        // cells. Only a scrollback view breaks the uniform shift.
        let shift = full && self.scroll_offset == 0;
        if full && allow_history && !self.in_alt() {
            self.normalize_region();
            for _ in 0..count {
                let mut line = self.main.pop_front().unwrap_or_else(|| self.new_line());
                if self.history != 0 {
                    self.scrollback.push_line(&line, self.cols, self.history);
                    if self.scroll_offset > 0 {
                        self.scroll_offset = (self.scroll_offset + 1).min(self.scrollback.len());
                    }
                }
                let id = self.next_line_id();
                blank_in_place(&mut line, id);
                self.main.push_back(line);
            }
        } else {
            // The region is a ring: advancing the offset moves every line up
            // at once. Only the slots wrapping into the bottom margin need a
            // fresh line, so the cost is O(count), not O(region size).
            let len = bottom - top + 1;
            let offset = self.region_offset;
            for step in 0..count {
                let slot = top + (offset + step) % len;
                let id = self.next_line_id();
                blank_in_place(&mut self.lines_mut()[slot], id);
            }
            self.region_offset = (offset + count) % len;
        }
        if shift {
            self.pending_scroll += count as i32;
            // Rows written before the shift move up with the content. Their
            // dirty flags have to move too, or the renderer leaves them blank.
            self.dirty.copy_within(count.., 0);
            self.dirty[self.rows - count..].fill(false);
            for row in (self.rows - count)..self.rows {
                self.mark_row(row);
            }
        } else {
            for row in top..=bottom {
                self.mark_row(row);
            }
        }
    }

    fn scroll_region_down(&mut self, top: usize, bottom: usize, count: usize) {
        let count = count.min(bottom - top + 1);
        let full = top == 0 && bottom + 1 == self.rows;
        let shift = full && self.scroll_offset == 0;
        if full {
            self.normalize_region();
            for _ in 0..count {
                let mut line = self
                    .lines_mut()
                    .pop_back()
                    .unwrap_or_else(|| self.new_line());
                let id = self.next_line_id();
                blank_in_place(&mut line, id);
                self.lines_mut().push_front(line);
            }
        } else {
            let len = bottom - top + 1;
            let mut offset = self.region_offset;
            for _ in 0..count {
                offset = if offset == 0 { len - 1 } else { offset - 1 };
                let slot = top + offset;
                let id = self.next_line_id();
                blank_in_place(&mut self.lines_mut()[slot], id);
            }
            self.region_offset = offset;
        }
        if shift {
            self.pending_scroll -= count as i32;
            self.dirty.copy_within(..self.rows - count, count);
            self.dirty[..count].fill(false);
            for row in 0..count {
                self.mark_row(row);
            }
        } else {
            for row in top..=bottom {
                self.mark_row(row);
            }
        }
    }

    fn insert_cells(&mut self, count: usize) {
        let x = self.cursor.x;
        let y = self.cursor.y;
        let count = count.min(self.cols - x);
        let cols = self.cols;
        let bg = self.pen.bg;
        let line = self.line_mut(y);
        line.cells.copy_within(x..cols - count, x + count);
        let blank = Cell {
            bg,
            ..Cell::blank()
        };
        line.cells[x..x + count].fill(blank);
        line.mark_all_written();
        self.mark_row(y);
    }

    fn delete_cells(&mut self, count: usize) {
        let x = self.cursor.x;
        let y = self.cursor.y;
        let count = count.min(self.cols - x);
        let cols = self.cols;
        let bg = self.pen.bg;
        let line = self.line_mut(y);
        line.cells.copy_within(x + count..cols, x);
        let blank = Cell {
            bg,
            ..Cell::blank()
        };
        line.cells[cols - count..cols].fill(blank);
        line.mark_all_written();
        self.mark_row(y);
    }

    fn erase_cells(&mut self, y: usize, from: usize, to: usize) {
        if y >= self.rows {
            return;
        }
        let to = to.min(self.cols - 1);
        if from > to {
            return;
        }
        let bg = self.pen.bg;
        let cols = self.cols;
        let blank = Cell {
            bg,
            ..Cell::blank()
        };
        let line = self.line_mut(y);
        if bg == Color::Default {
            if line.written_min > line.written_max {
                return;
            }
            let clamped_from = from.max(line.written_min);
            let clamped_to = to.min(line.written_max);
            if clamped_from > clamped_to {
                return;
            }
            if clamped_from > 0 && line.cells[clamped_from].width == 0 {
                line.cells[clamped_from - 1] = blank;
                line.mark_written(clamped_from - 1);
            }
            if line.cells[clamped_to].width == 2 && clamped_to + 1 < cols {
                line.cells[clamped_to + 1] = blank;
                line.mark_written(clamped_to + 1);
            }
            line.cells[clamped_from..=clamped_to].fill(blank);
            if from <= line.written_min && to >= line.written_max {
                line.written_min = usize::MAX;
                line.written_max = 0;
            } else if from <= line.written_min {
                line.written_min = (to + 1).min(line.written_max);
            } else if to >= line.written_max {
                line.written_max = from.saturating_sub(1);
            }
            line.revision = line.revision.wrapping_add(1);
        } else {
            if from > 0 && line.cells[from].width == 0 {
                line.cells[from - 1] = blank;
            }
            if line.cells[to].width == 2 && to + 1 < cols {
                line.cells[to + 1] = blank;
            }
            line.cells[from..=to].fill(blank);
            line.mark_written_range(from.saturating_sub(1), (to + 1).min(cols - 1));
        }
        self.mark_row(y);
    }

    fn clear_line(&mut self, y: usize) {
        self.erase_cells(y, 0, self.cols - 1);
    }

    /// Inserts lines at the cursor within the scroll region.
    fn insert_lines(&mut self, count: usize) {
        if self.cursor.y < self.scroll_top || self.cursor.y > self.scroll_bottom {
            return;
        }
        self.normalize_region();
        let y = self.cursor.y;
        let bottom = self.scroll_bottom;
        let cols = self.cols;
        let count = count.min(bottom - y + 1);
        for _ in 0..count {
            let id = self.next_line_id();
            let lines = self.lines_mut();
            let mut recycled = lines
                .remove(bottom)
                .unwrap_or_else(|| Line::new(id, vec![Cell::blank(); cols]));
            blank_in_place(&mut recycled, id);
            lines.insert(y, recycled);
        }
        for row in y..=bottom {
            self.mark_row(row);
        }
    }

    /// Deletes lines at the cursor within the scroll region.
    fn delete_lines(&mut self, count: usize) {
        if self.cursor.y < self.scroll_top || self.cursor.y > self.scroll_bottom {
            return;
        }
        self.normalize_region();
        let y = self.cursor.y;
        let bottom = self.scroll_bottom;
        let cols = self.cols;
        let count = count.min(bottom - y + 1);
        for _ in 0..count {
            let id = self.next_line_id();
            let lines = self.lines_mut();
            let mut recycled = lines
                .remove(y)
                .unwrap_or_else(|| Line::new(id, vec![Cell::blank(); cols]));
            blank_in_place(&mut recycled, id);
            lines.insert(bottom, recycled);
        }
        for row in y..=bottom {
            self.mark_row(row);
        }
    }

    fn tab_forward(&mut self, count: usize) {
        for _ in 0..count {
            let mut x = self.cursor.x + 1;
            while x < self.cols && !self.tabs.get(x).copied().unwrap_or(false) {
                x += 1;
            }
            self.cursor.x = x.min(self.cols - 1);
        }
        self.wrap_pending = false;
        self.last_printed = None;
        self.mark_row(self.cursor.y);
    }

    fn tab_backward(&mut self, count: usize) {
        for _ in 0..count {
            let mut x = self.cursor.x;
            loop {
                if x == 0 {
                    break;
                }
                x -= 1;
                if self.tabs.get(x).copied().unwrap_or(false) {
                    break;
                }
            }
            self.cursor.x = x;
        }
        self.wrap_pending = false;
        self.last_printed = None;
        self.mark_row(self.cursor.y);
    }

    fn set_tab_stop(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.cursor.x) {
            *tab = true;
        }
    }

    fn save_cursor(&mut self) {
        self.saved = Some(self.saved_cursor());
    }

    fn restore_cursor(&mut self) {
        if let Some(saved) = self.saved.clone() {
            self.restore_saved(saved);
        }
    }

    fn saved_cursor(&self) -> SavedCursor {
        SavedCursor {
            x: self.cursor.x,
            y: self.cursor.y,
            pen: self.pen,
            origin: self.origin_mode,
            autowrap: self.autowrap,
            charsets: self.charsets,
            active_charset: self.active_charset,
            wrap_pending: self.wrap_pending,
        }
    }

    fn restore_saved(&mut self, saved: SavedCursor) {
        self.pen = saved.pen;
        self.origin_mode = saved.origin;
        self.autowrap = saved.autowrap;
        self.charsets = saved.charsets;
        self.active_charset = saved.active_charset;
        self.wrap_pending = saved.wrap_pending;
        self.set_cursor(saved.x, saved.y);
    }

    fn set_alternate(&mut self, enable: bool, save_cursor: bool) {
        if enable {
            if self.alt.is_some() {
                return;
            }
            if save_cursor {
                self.alt_saved = Some(self.saved_cursor());
            }
            self.normalize_region();
            self.region_offset = 0;
            self.alt = Some(
                (0..self.rows)
                    .map(|_| self.new_line())
                    .collect::<VecDeque<Line>>(),
            );
            self.cursor.x = 0;
            self.cursor.y = 0;
            self.wrap_pending = false;
            self.last_printed = None;
            self.mark_all();
        } else if self.alt.is_some() {
            self.normalize_region();
            self.region_offset = 0;
            self.alt.take();
            if save_cursor {
                if let Some(saved) = self.alt_saved.take() {
                    self.restore_saved(saved);
                }
            }
            self.scroll_top = 0;
            self.scroll_bottom = self.rows - 1;
            self.mark_all();
        }
    }

    fn hard_reset(&mut self) {
        self.region_offset = 0;
        self.main.clear();
        for _ in 0..self.rows {
            let line = self.new_line();
            self.main.push_back(line);
        }
        self.alt = None;
        self.scrollback.clear();
        self.scroll_offset = 0;
        self.cursor.x = 0;
        self.cursor.y = 0;
        self.cursor.style = self.default_cursor_style;
        self.cursor.blinking = self.default_cursor_blink;
        self.saved = None;
        self.alt_saved = None;
        self.newline_mode = false;
        self.reverse_video = false;
        self.bracketed_paste = false;
        self.focus_events = false;
        self.theme = self.default_theme.clone();
        self.reset_tabs();
        self.selection = None;
        self.extras.clear();
        self.extras_map.clear();
        self.link_targets.clear();
        self.link_targets_map.clear();
        self.meta_pairs.clear();
        self.meta_pairs_map.clear();
        if let Some(search) = &mut self.search {
            search.matches.clear();
            search.current = 0;
            search.lines.clear();
        }
        self.dirty = vec![false; self.rows];
        self.soft_reset();
    }

    fn soft_reset(&mut self) {
        self.cursor.visible = true;
        self.pen = Pen::default();
        self.active_link = 0;
        self.normalize_region();
        self.region_offset = 0;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.insert_mode = false;
        self.origin_mode = false;
        self.autowrap = true;
        self.app_cursor = false;
        self.app_keypad = false;
        self.mouse_mode = MouseMode::Off;
        self.kitty_flags = [0, 0];
        self.kitty_stack = [Vec::new(), Vec::new()];
        self.mouse_encoding = MouseEncoding::X10;
        self.charsets = [Charset::Ascii; 4];
        self.active_charset = 0;
        self.single_shift = None;
        self.wrap_pending = false;
        self.last_printed = None;
        self.mark_all();
    }
}

/// Turns a recycled line into a fresh blank one without allocating cells.
fn blank_in_place(line: &mut Line, id: u64) {
    line.clear_written();
    line.wrapped = false;
    line.id = id;
}

/// Pads or truncates a stored line to a new width.
fn resize_line(line: &mut Line, cols: usize) {
    line.cells.resize(cols, Cell::blank());
    if line.cells.is_empty() {
        line.cells.push(Cell::blank());
    }
    line.mark_all_written();
}

fn ordered(a: Anchor, b: Anchor) -> (Anchor, Anchor) {
    if (a.line, a.col) <= (b.line, b.col) {
        (a, b)
    } else {
        (b, a)
    }
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric()
        || matches!(
            ch,
            '_' | '-' | '.' | '/' | '~' | ':' | '@' | '+' | '%' | '#' | '?' | '=' | '&'
        )
}

fn folded_query(query: &str) -> Vec<char> {
    query.chars().map(fold_search_char).collect()
}

/// Folds one character for case-insensitive search. This is single-character
/// lowercasing, so multi-character folds such as the German sharp s are not
/// treated as equal.
fn fold_search_char(ch: char) -> char {
    ch.to_lowercase().next().unwrap_or(ch)
}

/// Appends a coordinate as a UTF-8 code point for 1005 mouse reports.
fn push_utf8_coord(out: &mut Vec<u8>, value: u32) {
    if let Some(ch) = char::from_u32(value) {
        let mut buffer = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
    }
}

fn selection_name(selection: u8) -> String {
    match selection {
        b'p' => "p".to_string(),
        b'c' | 0 => "c".to_string(),
        other => (other as char).to_string(),
    }
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let a = chunk[0] as u32;
        let b = chunk.get(1).copied().unwrap_or(0) as u32;
        let c = chunk.get(2).copied().unwrap_or(0) as u32;
        let value = (a << 16) | (b << 8) | c;
        out.push(TABLE[((value >> 18) & 63) as usize] as char);
        out.push(TABLE[((value >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut values = Vec::with_capacity(text.len());
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 64,
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return None,
        };
        values.push(value);
    }
    let mut out = Vec::with_capacity(values.len() * 3 / 4);
    for chunk in values.chunks(4) {
        if chunk.len() < 2 {
            return None;
        }
        let a = chunk[0] as u32;
        let b = chunk[1] as u32;
        let c = chunk.get(2).copied().unwrap_or(64) as u32;
        let d = chunk.get(3).copied().unwrap_or(64) as u32;
        if a >= 64 || b >= 64 {
            return None;
        }
        out.push(((a << 2) | (b >> 4)) as u8);
        if c < 64 {
            out.push((((b & 15) << 4) | (c >> 2)) as u8);
        }
        if d < 64 {
            out.push((((c & 3) << 6) | d) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminal(rows: usize) -> Terminal {
        let theme = Theme::from_config(&crate::config::ColorsConfig::default()).unwrap();
        Terminal::new(20, rows, 100, theme, CursorStyle::Block, true)
    }

    fn feed(terminal: &mut Terminal, bytes: &[u8]) {
        let mut parser = Parser::new();
        parser.advance(bytes, terminal);
    }

    #[test]
    fn prints_ascii_and_wraps() {
        let mut terminal = terminal(2);
        feed(&mut terminal, b"hello");
        assert_eq!(terminal.lines()[0].cells[0].ch, 'h');
        assert_eq!(terminal.cursor.x, 5);
        feed(&mut terminal, &[b'x'; 16]);
        assert_eq!(terminal.cursor.y, 1);
    }

    #[test]
    fn ascii_run_matches_print_across_wraps() {
        let text = b"abcdefghijklmnopqrstuvwxyz0123456789!@#$%^&*()";
        let mut slow = terminal(3);
        let mut fast = terminal(3);
        for &byte in text {
            slow.print(byte as char);
        }
        fast.print_ascii_run(text);
        assert_eq!(
            (fast.cursor.x, fast.cursor.y),
            (slow.cursor.x, slow.cursor.y)
        );
        for row in 0..3 {
            for col in 0..20 {
                assert_eq!(
                    fast.lines()[row].cells[col].ch,
                    slow.lines()[row].cells[col].ch
                );
            }
        }
    }

    #[derive(Debug, PartialEq)]
    struct CellSnap {
        ch: char,
        meta: u32,
        width: u8,
        fg: Color,
        bg: Color,
        attrs: Attrs,
    }

    #[derive(Debug, PartialEq)]
    struct TermSnap {
        cells: Vec<Vec<CellSnap>>,
        cursor: (usize, usize),
        wrap_pending: bool,
        last_printed: Option<(usize, usize)>,
        last_graphic: char,
        dirty: Vec<bool>,
        region_offset: usize,
    }

    fn term_snapshot(t: &Terminal) -> TermSnap {
        TermSnap {
            cells: (0..t.rows)
                .map(|row| {
                    t.lines()[row]
                        .cells
                        .iter()
                        .map(|c| CellSnap {
                            ch: c.ch,
                            meta: c.meta,
                            width: c.width,
                            fg: c.fg,
                            bg: c.bg,
                            attrs: c.attrs,
                        })
                        .collect()
                })
                .collect(),
            cursor: (t.cursor.x, t.cursor.y),
            wrap_pending: t.wrap_pending,
            last_printed: t.last_printed,
            last_graphic: t.last_graphic,
            dirty: t.dirty.clone(),
            region_offset: t.region_offset,
        }
    }

    #[test]
    fn short_ascii_runs_match_per_byte_print() {
        // Run lengths 1..=7 at every starting column, plus a long run.
        for len in 1..=8 {
            let bytes: Vec<u8> = (0..len).map(|i| b'a' + (i as u8 % 26)).collect();
            for start in 0..20 {
                for region in [false, true] {
                    let mut slow = terminal(3);
                    let mut fast = terminal(3);
                    if region {
                        for t in [&mut slow, &mut fast] {
                            t.scroll_top = 1;
                            t.scroll_bottom = 2;
                            t.cursor.y = 1;
                            t.region_offset = 1;
                        }
                    }
                    slow.cursor.x = start;
                    fast.cursor.x = start;
                    for &b in &bytes {
                        slow.print(b as char);
                    }
                    fast.print_ascii_run(&bytes);
                    assert_eq!(
                        term_snapshot(&fast),
                        term_snapshot(&slow),
                        "len={len} start={start} region={region}"
                    );
                }
            }
        }
    }

    #[test]
    fn ascii_run_matches_print_with_flags_and_wide_overwrite() {
        for mode in 0..4 {
            for len in 1..=9 {
                let bytes: Vec<u8> = (0..len).map(|i| b'a' + (i as u8 % 26)).collect();
                let mut slow = terminal(3);
                let mut fast = terminal(3);
                for t in [&mut slow, &mut fast] {
                    if mode & 1 != 0 {
                        t.autowrap = false;
                    }
                    if mode & 2 != 0 {
                        t.wrap_pending = true;
                    }
                    // Non-default background pen and a pre-existing wide char
                    // in the run's path.
                    t.pen.bg = Color::Indexed(4);
                    t.cursor.x = 2;
                    t.lines_mut()[0].cells[3] = Cell {
                        ch: '\u{4e16}',
                        meta: 0,
                        fg: Color::Default,
                        bg: Color::Default,
                        attrs: Attrs::default(),
                        width: 2,
                    };
                    t.lines_mut()[0].cells[4] = Cell {
                        ch: ' ',
                        meta: 0,
                        fg: Color::Default,
                        bg: Color::Default,
                        attrs: Attrs::default(),
                        width: 0,
                    };
                    t.lines_mut()[0].has_wide = true;
                }
                for &b in &bytes {
                    slow.print(b as char);
                }
                fast.print_ascii_run(&bytes);
                assert_eq!(
                    term_snapshot(&fast),
                    term_snapshot(&slow),
                    "mode={mode} len={len}"
                );
            }
        }
    }

    #[test]
    fn ascii_run_written_range_covers_every_written_cell() {
        for len in 1..=45usize {
            for start in 0..20usize {
                // 0: no region; 1: full screen with offset; 2: partial region
                // whose bottom margin is above the last row.
                for region_mode in 0..3u8 {
                    let mut t = terminal(4);
                    if region_mode == 1 {
                        t.scroll_bottom = 3;
                        t.region_offset = 1;
                    } else if region_mode == 2 {
                        t.scroll_bottom = 2;
                        t.region_offset = 1;
                        // Keep the run inside the region so the wrap loop is
                        // taken (it requires end_row < scroll_bottom).
                        if start + len > 40 {
                            continue;
                        }
                    }
                    t.cursor.x = start;
                    let bytes: Vec<u8> = (0..len).map(|i| b'a' + (i % 26) as u8).collect();
                    t.print_ascii_run(&bytes);
                    // Clear and check via line_mut so scroll-region mapping is
                    // applied consistently on both sides.
                    for row in 0..t.rows {
                        t.line_mut(row).clear_written();
                        let blank = t.line_mut(row).cells.iter().all(Cell::is_blank);
                        assert!(
                            blank,
                            "stale cell: len={len} start={start} row={row} region_mode={region_mode}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn utf8_and_combining() {
        let mut terminal = terminal(2);
        feed(&mut terminal, "e\u{301}".as_bytes());
        let cell = terminal.lines()[0].cells[0];
        assert_eq!(cell.ch, 'e');
        assert_eq!(terminal.cell_extra(cell), Some("\u{301}"));
    }

    #[test]
    fn osc8_link_and_combining_marks_coexist() {
        let mut terminal = terminal(2);
        feed(
            &mut terminal,
            "\x1b]8;;https://example.com\x1b\\e\u{301}\x1b]8;;\x1b\\".as_bytes(),
        );
        let cell = terminal.lines()[0].cells[0];
        assert_eq!(cell.ch, 'e');
        assert_eq!(terminal.cell_extra(cell), Some("\u{301}"));
        assert_eq!(terminal.link_at(0, 0), Some("https://example.com"));
        assert!(terminal.cell_has_link(cell));
    }

    #[test]
    fn cell_stays_compact() {
        assert!(std::mem::size_of::<Cell>() <= 20);
    }

    #[test]
    fn non_visual_private_modes_do_not_mark_screen_dirty() {
        let mut terminal = terminal(3);
        let _ = terminal.take_damage();
        feed(&mut terminal, b"\x1b[?2004h\x1b[?1h\x1b[?1000h\x1b[?1006h");
        assert!(terminal.take_damage().is_empty());
        feed(&mut terminal, b"\x1b[?25l");
        let damage = terminal.take_damage();
        assert!(!damage.all);
        assert!(damage.rows[0]);
        assert!(!damage.rows[1]);
    }

    #[test]
    fn insert_and_delete_cells_and_lines_preserve_content() {
        let mut terminal = terminal(3);
        feed(&mut terminal, b"abcdef\r\nrow1\r\nrow2");
        feed(&mut terminal, b"\x1b[1;2H\x1b[2@");
        assert_eq!(terminal.lines()[0].cells[0].ch, 'a');
        assert_eq!(terminal.lines()[0].cells[1].ch, ' ');
        assert_eq!(terminal.lines()[0].cells[2].ch, ' ');
        assert_eq!(terminal.lines()[0].cells[3].ch, 'b');
        feed(&mut terminal, b"\x1b[1;2H\x1b[2P");
        assert_eq!(terminal.lines()[0].cells[1].ch, 'b');
        feed(&mut terminal, b"\x1b[2;1H");
        let _ = terminal.take_damage();
        feed(&mut terminal, b"\x1b[1L");
        let damage = terminal.take_damage();
        assert!(!damage.all);
        assert!(!damage.rows[0]);
        assert!(damage.rows[1]);
        assert!(damage.rows[2]);
        assert_eq!(terminal.visible_line(0).unwrap().cells[0].ch, 'a');
        assert!(terminal.visible_line(1).unwrap().cells[0].is_blank());
        assert_eq!(terminal.visible_line(2).unwrap().cells[0].ch, 'r');
        feed(&mut terminal, b"\x1b[2;1H\x1b[1M");
        assert_eq!(terminal.visible_line(1).unwrap().cells[0].ch, 'r');
        assert!(terminal.visible_line(2).unwrap().cells[0].is_blank());
    }

    #[test]
    fn wide_char_occupies_two_cells() {
        let mut terminal = terminal(2);
        feed(&mut terminal, "\u{4e16}".as_bytes());
        assert_eq!(terminal.lines()[0].cells[0].width, 2);
        assert_eq!(terminal.lines()[0].cells[1].width, 0);
        assert_eq!(terminal.cursor.x, 2);
    }

    #[test]
    fn csi_cursor_and_erase() {
        let mut terminal = terminal(3);
        feed(&mut terminal, b"abc\x1b[2;2H\x1b[K");
        assert_eq!(terminal.cursor.x, 1);
        assert_eq!(terminal.cursor.y, 1);
        assert_eq!(terminal.lines()[1].cells[1].ch, ' ');
    }

    #[test]
    fn deferred_scrollback_resize() {
        let mut terminal = terminal(2);
        feed(&mut terminal, b"a\r\nb\r\nc");
        assert_eq!(terminal.scrollback_len(), 1);
        assert_eq!(terminal.scrollback_cells_len(0), 20);

        terminal.resize(40, 2, 10, 20);
        assert_eq!(terminal.width(), 40);
        assert_eq!(terminal.scrollback_cells_len(0), 20);

        terminal.finish_deferred_resize();
        assert_eq!(terminal.scrollback_cells_len(0), 40);
    }

    #[test]
    fn scrollback_arena_handles_blanks_eviction_and_chunk_recycling() {
        let theme = Theme::from_config(&crate::config::ColorsConfig::default()).unwrap();
        let mut terminal = Terminal::new(80, 4, 600, theme, CursorStyle::Block, false);
        let mut parser = Parser::new();
        // Push enough lines (> SCROLLBACK_CHUNK_CELLS = 32768 cells) with
        // interleaved blank lines so chunks fill, evict, and recycle.
        for i in 0..1200usize {
            if i % 5 == 0 {
                parser.advance(b"\r\n", &mut terminal);
            } else {
                let line = format!(
                    "{i:04} 0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ\r\n"
                );
                parser.advance(line.as_bytes(), &mut terminal);
            }
        }
        assert_eq!(terminal.scrollback_len(), 600);
        // The oldest scrollback line is around index 596..600 depending on trailing newline.
        terminal.scroll_to_top();
        let mut non_blank_seen = 0usize;
        for row in 0..terminal.height() {
            let line = terminal.visible_line(row).unwrap();
            if line.cells.first().is_some_and(|c| c.ch != ' ') {
                non_blank_seen += 1;
                let prefix: String = line.cells[..4].iter().map(|c| c.ch).collect();
                assert!(
                    prefix.chars().all(|c| c.is_ascii_digit()),
                    "corrupted scrollback prefix: {prefix:?}"
                );
            }
        }
        assert!(non_blank_seen > 0);
        // Verify search finds a line inside scrollback after chunk recycling.
        terminal.set_search_query("0901");
        assert_eq!(terminal.search_matches().len(), 1);
    }

    #[test]
    fn scrollback_keeps_lines() {
        let mut terminal = terminal(2);
        feed(&mut terminal, b"a\r\nb\r\nc");
        assert_eq!(terminal.scrollback_len(), 1);
        assert_eq!(terminal.lines()[0].cells[0].ch, 'b');
    }

    #[test]
    fn whole_screen_scroll_keeps_dirty_rows_with_their_content() {
        let mut terminal = terminal(3);
        feed(&mut terminal, b"\x1b[?1049ha\r\nb\r\nc");
        let _ = terminal.take_damage();
        // A pager writes one line at the bottom then scrolls, all in one frame.
        feed(&mut terminal, b"d\r\ne\r\nf\r\ng");
        let damage = terminal.take_damage();
        assert_eq!(damage.scroll, 3);
        assert!(damage.rows.iter().all(|dirty| *dirty), "{:?}", damage.rows);
    }

    #[test]
    fn recycled_lines_are_blank_outside_written_range() {
        let theme = Theme::from_config(&crate::config::ColorsConfig::default()).unwrap();
        let mut terminal = Terminal::new(4, 2, 2, theme, CursorStyle::Block, false);
        let mut parser = Parser::new();
        for _ in 0..10 {
            parser.advance(b"AAAA\r\n", &mut terminal);
        }
        for _ in 0..5 {
            parser.advance(b"B\r\n", &mut terminal);
        }
        for row in 0..2 {
            let line = terminal.visible_line(row).unwrap();
            for (col, cell) in line.cells.iter().enumerate().skip(1) {
                assert!(cell.is_blank(), "row {row} col {col} = {cell:?}");
            }
        }
    }

    #[test]
    fn narrow_over_wide_clears_its_continuation() {
        let mut terminal = terminal(5);
        feed(&mut terminal, "\u{4e16}".as_bytes());
        assert_eq!(terminal.lines()[0].cells[0].width, 2);
        assert_eq!(terminal.lines()[0].cells[1].width, 0);
        terminal.cursor.x = 0;
        terminal.print('A');
        assert_eq!(terminal.lines()[0].cells[0].ch, 'A');
        assert!(terminal.lines()[0].cells[1].is_blank());
        assert_eq!(terminal.lines()[0].cells[1].width, 1);
    }

    #[test]
    fn kitty_keyboard_modes_and_stacks() {
        let mut terminal = terminal(4);
        feed(&mut terminal, b"\x1b[=1u");
        assert_eq!(terminal.kitty_flags(), 1);
        feed(&mut terminal, b"\x1b[=3;2u");
        assert_eq!(terminal.kitty_flags(), 3);
        feed(&mut terminal, b"\x1b[=2;3u");
        assert_eq!(terminal.kitty_flags(), 1);
        terminal.push_kitty_flags(5);
        assert_eq!(terminal.kitty_flags(), 5);
        terminal.pop_kitty_flags(1);
        assert_eq!(terminal.kitty_flags(), 1);
        terminal.pop_kitty_flags(1);
        assert_eq!(terminal.kitty_flags(), 0);
        terminal.answer_kitty_flags();
        assert_eq!(terminal.take_output(), b"\x1b[?0u");
    }

    #[test]
    fn kitty_keyboard_stacks_are_separate_per_screen() {
        let mut terminal = terminal(4);
        feed(&mut terminal, b"\x1b[=1u\x1b[?1049h");
        assert_eq!(terminal.kitty_flags(), 0);
        feed(&mut terminal, b"\x1b[=2u");
        assert_eq!(terminal.kitty_flags(), 2);
        feed(&mut terminal, b"\x1b[?1049l");
        assert_eq!(terminal.kitty_flags(), 1);
    }

    #[test]
    fn region_scroll_rotates_lines_in_place() {
        let mut terminal = terminal(5);
        terminal.scroll_top = 1;
        terminal.scroll_bottom = 3;
        for (row, ch) in [(1, 'A'), (2, 'B'), (3, 'C')] {
            terminal.cursor.x = 0;
            terminal.cursor.y = row;
            terminal.print(ch);
        }
        terminal.scroll_region_up(1, 3, 1, true);
        assert_eq!(terminal.region_offset, 1);
        assert_eq!(terminal.visible_line(1).unwrap().cells[0].ch, 'B');
        assert_eq!(terminal.visible_line(2).unwrap().cells[0].ch, 'C');
        assert!(terminal.visible_line(3).unwrap().cells[0].is_blank());
        terminal.scroll_region_up(1, 3, 1, true);
        assert_eq!(terminal.visible_line(1).unwrap().cells[0].ch, 'C');
        assert_eq!(terminal.region_offset, 2);
        terminal.scroll_region_up(1, 3, 1, true);
        assert_eq!(terminal.region_offset, 0);
        for row in 1..=3 {
            assert!(terminal.visible_line(row).unwrap().cells[0].is_blank());
        }
    }

    #[test]
    fn region_scroll_down_and_normalize_restore_logical_order() {
        let mut terminal = terminal(5);
        terminal.scroll_top = 1;
        terminal.scroll_bottom = 3;
        for (row, ch) in [(1, 'A'), (2, 'B'), (3, 'C')] {
            terminal.cursor.x = 0;
            terminal.cursor.y = row;
            terminal.print(ch);
        }
        terminal.scroll_region_down(1, 3, 1);
        assert!(terminal.visible_line(1).unwrap().cells[0].is_blank());
        assert_eq!(terminal.visible_line(2).unwrap().cells[0].ch, 'A');
        assert_eq!(terminal.visible_line(3).unwrap().cells[0].ch, 'B');
        terminal.normalize_region();
        assert_eq!(terminal.region_offset, 0);
        assert!(terminal.line(1).cells[0].is_blank());
        assert_eq!(terminal.line(2).cells[0].ch, 'A');
        assert_eq!(terminal.line(3).cells[0].ch, 'B');
    }

    #[test]
    fn region_scroll_then_write_hits_the_visible_row() {
        let mut terminal = terminal(5);
        terminal.scroll_top = 1;
        terminal.scroll_bottom = 3;
        for (row, ch) in [(1, 'A'), (2, 'B'), (3, 'C')] {
            terminal.cursor.x = 0;
            terminal.cursor.y = row;
            terminal.print(ch);
        }
        terminal.scroll_region_up(1, 3, 1, true);
        terminal.cursor.x = 0;
        terminal.cursor.y = 1;
        terminal.print('Z');
        assert_eq!(terminal.visible_line(1).unwrap().cells[0].ch, 'Z');
        assert_eq!(terminal.visible_line(0).unwrap().cells[0].ch, ' ');
    }

    #[test]
    fn region_offset_leaves_scrollback_view_untouched() {
        let mut terminal = terminal(3);
        feed(&mut terminal, b"one\r\ntwo\r\nthree\r\nfour");
        feed(&mut terminal, b"\x1b[2;3r\x1b[S");
        assert_eq!(terminal.region_offset, 1);
        terminal.set_scroll_offset(1);
        assert_eq!(terminal.visible_line(0).unwrap().cells[0].ch, 'o');
        terminal.set_scroll_offset(0);
        assert_eq!(terminal.visible_line(0).unwrap().cells[0].ch, 't');
        assert_eq!(terminal.visible_line(1).unwrap().cells[0].ch, 'f');
        assert!(terminal.visible_line(2).unwrap().cells[0].is_blank());
    }

    #[test]
    fn changing_scroll_region_normalizes_the_ring() {
        let mut terminal = terminal(5);
        feed(&mut terminal, b"\x1b[2;4r");
        for (row, ch) in [(1, 'A'), (2, 'B'), (3, 'C')] {
            terminal.cursor.x = 0;
            terminal.cursor.y = row;
            terminal.print(ch);
        }
        feed(&mut terminal, b"\x1b[S");
        assert_eq!(terminal.region_offset, 1);
        feed(&mut terminal, b"\x1b[1;5r");
        assert_eq!(terminal.region_offset, 0);
        assert_eq!(terminal.visible_line(1).unwrap().cells[0].ch, 'B');
        assert_eq!(terminal.visible_line(2).unwrap().cells[0].ch, 'C');
    }

    #[test]
    fn alternate_screen_restores_primary() {
        let mut terminal = terminal(3);
        feed(&mut terminal, b"one\x1b[?1049hALT\x1b[?1049l");
        assert_eq!(terminal.lines()[0].cells[0].ch, 'o');
        assert!(!terminal.in_alt());
    }

    #[test]
    fn osc8_link_is_stored_and_cleared() {
        let mut terminal = terminal(2);
        feed(
            &mut terminal,
            b"\x1b]8;;https://example.com\x1b\\click\x1b]8;;\x1b\\ end",
        );
        assert_eq!(terminal.link_at(0, 0), Some("https://example.com"));
        assert_eq!(terminal.link_at(0, 4), Some("https://example.com"));
        assert_eq!(terminal.link_at(0, 6), None);
    }

    #[test]
    fn osc8_link_bel_terminator() {
        let mut terminal = terminal(2);
        feed(
            &mut terminal,
            b"\x1b]8;;file:///tmp/x.rs\x07link\x1b]8;;\x07",
        );
        assert_eq!(terminal.link_at(0, 0), Some("file:///tmp/x.rs"));
        assert_eq!(terminal.link_at(0, 4), None);
    }

    #[test]
    fn osc8_link_survives_scroll() {
        let mut terminal = terminal(2);
        feed(
            &mut terminal,
            b"\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\\r\n",
        );
        feed(&mut terminal, b"after\r\nmore\r\nlast");
        assert_eq!(terminal.scrollback_len(), 2);
        assert_eq!(terminal.link_at(0, 0), None);
        terminal.scroll(-2);
        assert_eq!(terminal.link_at(0, 0), Some("https://example.com"));
    }

    #[test]
    fn osc8_link_dedups_targets() {
        let mut terminal = terminal(2);
        feed(
            &mut terminal,
            b"\x1b]8;;https://example.com\x1b\\a\x1b]8;;\x1b\\\x1b]8;;https://example.com\x1b\\b",
        );
        assert_eq!(terminal.link_targets.len(), 1);
        assert_eq!(terminal.link_at(0, 0), terminal.link_at(0, 1));
    }

    #[test]
    fn sgr_truecolor() {
        let mut terminal = terminal(2);
        feed(&mut terminal, b"\x1b[38;2;1;2;3mX");
        assert_eq!(terminal.lines()[0].cells[0].fg, Color::Rgb([1, 2, 3]));
    }

    #[test]
    fn selection_copies_multiple_lines() {
        let mut terminal = terminal(3);
        feed(&mut terminal, b"abc\r\ndef");
        terminal.begin_selection(0, 1, SelectionMode::Char);
        terminal.update_selection(1, 1);
        assert_eq!(terminal.selection_text().as_deref(), Some("bc\nde"));
    }

    #[test]
    fn mouse_reports_use_sgr_form() {
        let mut terminal = terminal(3);
        feed(&mut terminal, b"\x1b[?1000h\x1b[?1006h");
        let report = terminal
            .mouse_report(0, 5, 2, true, false, MouseMods::default())
            .unwrap();
        assert_eq!(report, b"\x1b[<0;6;3M");
        let release = terminal
            .mouse_report(0, 5, 2, false, false, MouseMods::default())
            .unwrap();
        assert_eq!(release, b"\x1b[<0;6;3m");
    }

    #[test]
    fn resize_pads_and_truncates_lines() {
        let mut terminal = terminal(2);
        feed(&mut terminal, b"abcdef");
        terminal.resize(4, 3, 9, 18);
        assert_eq!(terminal.width(), 4);
        assert_eq!(terminal.height(), 3);
        assert!(terminal.lines().iter().all(|line| line.cells.len() == 4));
        assert_eq!(terminal.lines()[0].cells[0].ch, 'a');
    }

    #[test]
    fn osc_title_is_reported_once() {
        let mut terminal = terminal(2);
        feed(&mut terminal, b"\x1b]0;Hello\x07");
        assert_eq!(terminal.take_title().as_deref(), Some("Hello"));
        assert!(terminal.take_title().is_none());
    }

    #[test]
    fn dec_special_graphics_map() {
        let mut terminal = terminal(2);
        feed(&mut terminal, b"\x1b(0lqk\x1b(B");
        assert_eq!(terminal.lines()[0].cells[0].ch, '\u{250c}');
        assert_eq!(terminal.lines()[0].cells[1].ch, '\u{2500}');
        assert_eq!(terminal.lines()[0].cells[2].ch, '\u{2510}');
    }

    #[test]
    fn search_is_case_insensitive_and_steps_through_matches() {
        let mut terminal = terminal(3);
        feed(&mut terminal, b"Alpha beta ALPHA\r\nbeta alpha");
        terminal.set_search_query("alpha");
        let matches = terminal.search_matches();
        assert_eq!(matches.len(), 3);
        assert_eq!((matches[0].start, matches[0].end), (0, 4));
        assert_eq!((matches[1].start, matches[1].end), (11, 15));
        assert_eq!(terminal.search_summary(), (1, 3));
        assert!(terminal.search_step(true));
        assert_eq!(terminal.search_summary(), (2, 3));
        assert!(terminal.search_step(true));
        assert_eq!(terminal.search_summary(), (3, 3));
        assert!(terminal.search_step(true));
        assert_eq!(terminal.search_summary(), (1, 3));
        assert!(terminal.search_step(false));
        assert_eq!(terminal.search_summary(), (3, 3));
        let current = terminal.search_matches()[2];
        terminal.refresh_search("alpha");
        assert_eq!(terminal.search_summary(), (3, 3));
        assert_eq!(terminal.search_matches()[2], current);
    }

    #[test]
    fn search_starts_at_the_last_match_above_the_view() {
        let mut terminal = terminal(2);
        feed(
            &mut terminal,
            b"needle one\r\na\r\nb\r\nc\r\nd\r\nneedle two\r\ne\r\nf\r\ng",
        );
        terminal.set_search_query("needle");
        assert_eq!(terminal.search_matches().len(), 2);
        assert_eq!(terminal.search_summary(), (2, 2));
    }

    #[test]
    fn search_refresh_picks_up_changed_lines() {
        let mut terminal = terminal(2);
        feed(&mut terminal, b"alpha");
        terminal.set_search_query("alpha");
        assert_eq!(terminal.search_summary(), (1, 1));
        feed(&mut terminal, b"\rBETA");
        terminal.refresh_search("alpha");
        assert!(terminal.search_matches().is_empty());
        feed(&mut terminal, b"\rALPHA");
        terminal.refresh_search("alpha");
        assert_eq!(terminal.search_summary(), (1, 1));
    }

    #[test]
    fn search_ignores_trailing_line_padding() {
        let mut terminal = terminal(2);
        feed(&mut terminal, b"a");
        terminal.set_search_query(" ");
        assert!(terminal.search_matches().is_empty());
    }

    #[test]
    fn search_reveals_matches_in_scrollback() {
        let mut terminal = terminal(2);
        feed(&mut terminal, b"one\r\ntwo\r\nneedle\r\nfour\r\nfive");
        terminal.set_search_query("needle");
        let found = terminal.search_matches()[0];
        assert_eq!(
            terminal.visible_line(0).map(|line| line.id),
            Some(found.line)
        );
    }

    #[test]
    fn search_ignores_hidden_scrollback_on_alternate_screen() {
        let mut terminal = terminal(2);
        feed(&mut terminal, b"one\r\ntwo\r\nthree");
        feed(&mut terminal, b"\x1b[?1049hALT");
        terminal.set_search_query("one");
        assert!(terminal.search_matches().is_empty());
        terminal.set_search_query("ALT");
        assert_eq!(terminal.search_summary(), (1, 1));
        assert_eq!(terminal.scroll_offset(), 0);
    }

    #[test]
    fn clearing_search_drops_highlights() {
        let mut terminal = terminal(3);
        feed(&mut terminal, b"find me");
        terminal.set_search_query("find");
        assert_eq!(terminal.search_summary(), (1, 1));
        terminal.clear_search();
        assert_eq!(terminal.search_summary(), (0, 0));
        assert!(terminal.search_matches().is_empty());
    }

    #[test]
    fn base64_round_trip() {
        for text in ["", "a", "ab", "abc", "hello world"] {
            let encoded = base64_encode(text.as_bytes());
            assert_eq!(base64_decode(&encoded).unwrap(), text.as_bytes());
        }
    }
}
#[cfg(test)]
mod textrun_equiv {
    use super::*;
    fn dump(t: &Terminal) -> String {
        let mut o = format!(
            "{:?} {} {:?} {:?} {:?} {} ",
            (t.cursor.x, t.cursor.y),
            t.wrap_pending,
            t.last_printed,
            t.dirty,
            t.region_offset,
            t.last_graphic
        );
        for r in 0..t.rows {
            let l = &t.lines()[r];
            o.push_str(&format!("[w{} ", l.wrapped));
            for c in &l.cells {
                o.push_str(&format!(
                    "{}:{}:{}:{:?}:{:?}",
                    c.ch, c.width, c.meta, c.fg, c.bg
                ));
            }
            o.push(']');
        }
        o
    }
    #[test]
    fn text_run_matches_per_char_print() {
        let samples = [
            "a",
            "\u{e9}",
            "\u{4e2d}",
            "\u{4e2d}\u{6587}\u{5b57}",
            "a\u{e9}\u{4e2d}b",
            "e\u{301}x",
            "x\u{200d}y",
            "\u{1f600}a\u{1f600}",
            "ab\u{4e2d}c",
            "\u{85}z",
            "\u{2500}\u{2500}\u{2500}\u{2502}\u{2588}",
            "\u{4e2d}\u{4e2d}\u{4e2d}\u{4e2d}\u{4e2d}\u{4e2d}\u{4e2d}\u{4e2d}\u{4e2d}\u{4e2d}\u{4e2d}\u{4e2d}\u{4e2d}",
        ];
        for sample in samples {
            for rep in [1usize, 3, 12] {
                let text = sample.repeat(rep);
                for start in 0..20usize {
                    for (aw, pending, wide, ins, rows) in [
                        (true, false, false, false, 3usize),
                        (false, false, false, false, 3),
                        (true, true, false, false, 3),
                        (true, false, true, false, 3),
                        (true, false, false, true, 3),
                        (true, false, false, false, 2),
                    ] {
                        let mut a = Terminal::new(
                            20,
                            rows,
                            100,
                            Theme::from_config(&crate::config::ColorsConfig::default()).unwrap(),
                            CursorStyle::Block,
                            true,
                        );
                        let mut b = Terminal::new(
                            20,
                            rows,
                            100,
                            Theme::from_config(&crate::config::ColorsConfig::default()).unwrap(),
                            CursorStyle::Block,
                            true,
                        );
                        for t in [&mut a, &mut b] {
                            t.autowrap = aw;
                            t.insert_mode = ins;
                            t.pen.bg = Color::Indexed(3);
                            if wide {
                                for _ in 0..8 {
                                    t.print('\u{4e2d}');
                                }
                                t.cursor.x = 0;
                                t.cursor.y = 0;
                                t.wrap_pending = false;
                            }
                            t.cursor.x = start;
                            t.wrap_pending = pending && start == 19;
                        }
                        for ch in text.chars() {
                            a.print(ch);
                        }
                        b.print_text_run(&text);
                        assert_eq!(
                            dump(&a),
                            dump(&b),
                            "sample={sample:?} rep={rep} start={start} aw={aw} pend={pending} wide={wide} ins={ins} rows={rows}"
                        );
                        for r in 0..rows {
                            a.line_mut(r).clear_written();
                            b.line_mut(r).clear_written();
                            assert!(
                                b.lines()[r].cells.iter().all(Cell::is_blank),
                                "stale row {r} sample={sample:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
