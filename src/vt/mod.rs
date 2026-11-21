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

/// One character cell. Wide characters occupy a cell of width two followed by
/// a continuation cell of width zero.
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub ch: char,
    pub extra: u32,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
    /// One-based index into the terminal's OSC 8 link targets; zero is none.
    pub link: u32,
    /// Display width: zero for a continuation, one or two for a base.
    pub width: u8,
}

impl Cell {
    pub fn blank() -> Self {
        Self {
            ch: ' ',
            extra: 0,
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::default(),
            link: 0,
            width: 1,
        }
    }

    pub fn is_blank(&self) -> bool {
        self.ch == ' ' && self.extra == 0 && self.width != 0
    }

    /// True for an unstyled blank: the cells a rewrap drops at the end of a
    /// line. Styled blanks carry background colour and stay.
    fn is_padding(&self) -> bool {
        self.is_blank()
            && self.fg == Color::Default
            && self.bg == Color::Default
            && self.attrs == Attrs::default()
    }
}

/// A physical row, with a stable identity for selections.
#[derive(Clone, Debug)]
pub struct Line {
    /// Monotonic identity. Lines keep it when moved into scrollback.
    pub id: u64,
    /// Cells, always exactly the terminal width after a resize.
    pub cells: Vec<Cell>,
    /// Set while the line is full and autowrap continues it on the next row.
    /// A resize joins the rows of such a run and splits it at the new width.
    pub wrapped: bool,
    /// Set when the row stopped short of its last column because a double
    /// width character did not fit. The cell left at the margin is padding a
    /// rewrap drops, so it does not reappear as a space in the copied text.
    wide_wrap: bool,
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
            wide_wrap: false,
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
            for cell in &mut self.cells[self.written_min..=self.written_max] {
                *cell = Cell::blank();
            }
        }
        self.written_min = usize::MAX;
        self.written_max = 0;
        self.revision = self.revision.wrapping_add(1);
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
    scrollback: VecDeque<Line>,
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
    /// OSC 8 hyperlink targets, one-based so zero in a Cell means no link.
    link_targets: Vec<Box<str>>,
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
            scrollback: VecDeque::new(),
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
            link_targets: Vec::new(),
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

    fn all_lines(&self) -> Vec<&Line> {
        let active = self.lines();
        let mut lines = Vec::with_capacity(self.scrollback.len() + active.len());
        lines.extend(self.scrollback.iter());
        for row in 0..self.rows {
            lines.push(&active[self.physical_row(row)]);
        }
        lines
    }

    fn search_lines(&self) -> Vec<&Line> {
        if self.in_alt() {
            let active = self.lines();
            (0..self.rows)
                .map(|row| &active[self.physical_row(row)])
                .collect()
        } else {
            self.all_lines()
        }
    }

    pub fn visible_line(&self, row: usize) -> Option<&Line> {
        if row >= self.rows {
            return None;
        }
        if self.scroll_offset == 0 {
            return Some(self.line(row));
        }
        let saved = self.scrollback.len();
        let start = saved.saturating_sub(self.scroll_offset);
        let index = start + row;
        if index < saved {
            self.scrollback.get(index)
        } else {
            self.lines().get(self.physical_row(index - saved))
        }
    }

    pub fn anchor_at(&self, row: usize, col: usize) -> Option<Anchor> {
        let line = self.visible_line(row)?;
        let col = col.min(line.cells.len().saturating_sub(1));
        Some(Anchor { line: line.id, col })
    }

    pub fn extra_text(&self, id: u32) -> Option<&str> {
        self.extras.get(id as usize - 1).map(|text| &**text)
    }

    /// Starts an OSC 8 hyperlink, reusing an existing target id if the URI is
    /// identical. Returns the one-based id stored in cells.
    pub fn set_link(&mut self, target: &str) -> u32 {
        if let Some(index) = self
            .link_targets
            .iter()
            .position(|existing| &**existing == target)
        {
            self.active_link = index as u32 + 1;
        } else {
            self.link_targets.push(target.into());
            self.active_link = self.link_targets.len() as u32;
        }
        self.pen.link = self.active_link;
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
        let mut col = col.min(line.cells.len().saturating_sub(1));
        while col > 0 && line.cells.get(col).is_some_and(|cell| cell.width == 0) {
            col -= 1;
        }
        let id = line.cells.get(col)?.link;
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
            let line = *line;
            let mut from = if index == first { start_col } else { 0 };
            let mut to = if index == last {
                end_col
            } else {
                line.cells.len().saturating_sub(1)
            };
            to = to.min(line.cells.len().saturating_sub(1));
            from = from.min(to);
            while to > from && line.cells[to].is_blank() {
                to -= 1;
            }
            for cell in &line.cells[from..=to] {
                if cell.width == 0 {
                    continue;
                }
                text.push(cell.ch);
                if cell.extra != 0
                    && let Some(extra) = self.extras.get(cell.extra as usize - 1)
                {
                    text.push_str(extra);
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
                let line = self
                    .lines()
                    .iter()
                    .chain(self.scrollback.iter())
                    .find(|line| line.id == anchor.line);
                let last = line
                    .map(|line| line.cells.len().saturating_sub(1))
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
                let line = self
                    .all_lines()
                    .into_iter()
                    .find(|line| line.id == anchor.line);
                let last = line
                    .map(|line| line.cells.len().saturating_sub(1))
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
        let last_col = last.cells.len().saturating_sub(1);
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

    /// Replaces the search text and selects the nearest match at or below the
    /// top of the current view, or the last match above it when none is below.
    /// An empty query clears every highlight.
    pub fn set_search_query(&mut self, query: &str) {
        let lines = self.search_lines();
        let needle = folded_query(query);
        let mut cache = HashMap::with_capacity(lines.len());
        let mut indexed = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            let line_matches = self.scan_line(line, &needle);
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
        let mut old_cache = state.lines;
        let mut cache = HashMap::with_capacity(lines.len());
        let mut matches = Vec::new();
        for line in &lines {
            let line_matches = match old_cache.remove(&line.id) {
                Some(cached) if cached.revision == line.revision => cached.matches,
                _ => self.scan_line(line, &needle),
            };
            matches.extend(line_matches.iter().copied());
            cache.insert(
                line.id,
                CachedLine {
                    revision: line.revision,
                    matches: line_matches,
                },
            );
        }
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

    fn scan_line(&self, line: &Line, needle: &[char]) -> Vec<SearchMatch> {
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

    fn folded_line(&self, line: &Line) -> (Vec<char>, Vec<usize>) {
        let end = line
            .cells
            .iter()
            .rposition(|cell| cell.width != 0 && (cell.ch != ' ' || cell.extra != 0))
            .map_or(0, |index| index + 1);
        let mut chars = Vec::with_capacity(end);
        let mut columns = Vec::with_capacity(end);
        for (column, cell) in line.cells[..end].iter().enumerate() {
            if cell.width == 0 {
                continue;
            }
            chars.push(fold_search_char(cell.ch));
            columns.push(column);
            if cell.extra != 0
                && let Some(extra) = self.extras.get(cell.extra as usize - 1)
            {
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
        let Some(line) = self
            .all_lines()
            .into_iter()
            .find(|line| line.id == anchor.line)
        else {
            return (anchor, anchor);
        };
        let mut from = anchor.col.min(line.cells.len().saturating_sub(1));
        let mut to = from;
        if !is_word_char(self.cell_char(line, from)) {
            return (anchor, anchor);
        }
        while from > 0 && is_word_char(self.cell_char(line, from - 1)) {
            from -= 1;
        }
        while to + 1 < line.cells.len() && is_word_char(self.cell_char(line, to + 1)) {
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

    fn cell_char(&self, line: &Line, col: usize) -> char {
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
        let columns_changed = cols != self.cols;
        self.cols = cols;
        self.rows = rows;
        // Only the primary screen has rows a rewrap joins. The alternate screen
        // belongs to a full-screen application, which redraws itself from the
        // new size, so its rows are just padded or cut.
        let in_alt = self.in_alt();
        let mut pending = false;
        if columns_changed {
            // The primary rows are reflowed even on the alternate screen, so a
            // saved primary cursor is remapped; only the live cursor is restored
            // here, and on the alternate screen it belongs to the application.
            pending = self.reflow(!in_alt) && !in_alt;
            if let Some(alt) = &mut self.alt {
                for line in alt.iter_mut() {
                    resize_line(line, cols);
                }
            }
        }
        if self.alt.is_some() {
            let mut extra = VecDeque::new();
            while self.alt.as_ref().map_or(0, VecDeque::len) + extra.len() < rows {
                extra.push_back(self.new_line());
            }
            if let Some(alt) = &mut self.alt {
                alt.truncate(rows);
                alt.extend(extra);
            }
        }
        self.fit_height(!in_alt);
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
        self.wrap_pending = pending;
    }

    /// Rewraps the primary screen and its history for the current width. Rows an
    /// autowrap continued (`Line::wrapped`) join into one logical line, lose the
    /// blank cells that padded them and are split again at the width, so a
    /// paragraph keeps its flow and no cell is stored twice.
    ///
    /// Returns whether the live cursor now sits at the margin of a full row,
    /// which the caller turns back into a pending wrap.
    fn reflow(&mut self, track_cursor: bool) -> bool {
        let cols = self.cols;
        let rows = self.rows;
        let anchor = |line: usize, col: usize| (self.scrollback.len() + line, col);
        let was_pending = self.wrap_pending;
        // The live cursor and the two saved ones all address primary rows.
        // They are mapped through the same reflow, so a later restore lands on
        // the same text. Only the live cursor is left out when the alternate
        // screen owns it, but the saved ones still have to move.
        let marks = [
            track_cursor.then(|| anchor(self.cursor.y, self.cursor.x)),
            self.saved.as_ref().map(|saved| anchor(saved.y, saved.x)),
            self.alt_saved
                .as_ref()
                .map(|saved| anchor(saved.y, saved.x)),
        ];
        let mut decided = [None; 3];
        let mut source = std::mem::take(&mut self.scrollback);
        source.extend(std::mem::take(&mut self.main));

        let mut out: VecDeque<Line> = VecDeque::with_capacity(source.len());
        let mut spare: Vec<Line> = Vec::new();
        let mut content: Vec<Cell> = Vec::new();
        let mut run: Vec<Line> = Vec::new();
        let mut splits: Vec<(usize, usize)> = Vec::new();
        let mut row = 0;
        let mut source = source.into_iter();
        while let Some(line) = source.next() {
            // A row that is not continued and already fits only needs padding
            // or cutting, which keeps a full scrollback cheap to resize.
            if !line.wrapped && content_len(&line.cells) <= cols {
                for (slot, mark) in decided.iter_mut().zip(marks.iter()) {
                    if let Some((at, col)) = mark
                        && slot.is_none()
                        && *at == row
                    {
                        *slot = Some((out.len(), *col));
                    }
                }
                let mut line = line;
                resize_line(&mut line, cols);
                out.push_back(line);
                row += 1;
                continue;
            }
            run.push(line);
            while run.last().is_some_and(|line| line.wrapped) {
                let Some(line) = source.next() else {
                    break;
                };
                run.push(line);
            }
            content.clear();
            let mut offsets = [None; 3];
            for (index, line) in run.iter().enumerate() {
                // The row that ends a logical line is padded. The rows an
                // autowrap continued are full, except the margin cell a double
                // width character left behind to take the next row.
                let width = if line.wrapped && !line.wide_wrap {
                    line.cells.len()
                } else {
                    content_len(&line.cells)
                };
                for (slot, mark) in offsets.iter_mut().zip(marks.iter()) {
                    if let Some((at, col)) = mark
                        && slot.is_none()
                        && *at == row + index
                    {
                        *slot = Some(content.len() + (*col).min(width));
                    }
                }
                content.extend_from_slice(&line.cells[..width]);
            }
            row += run.len();
            spare.append(&mut run);
            split(&content, cols, &mut splits);
            let count = splits.len();
            let start = out.len();
            for (index, (from, to)) in splits.iter().enumerate() {
                let mut line = spare.pop().unwrap_or_else(|| self.new_line());
                line.id = self.next_line_id();
                line.wrapped = index + 1 < count;
                line.wide_wrap =
                    *to < content.len() && content.get(*to).is_some_and(|cell| cell.width == 2);
                line.cells.clear();
                line.cells.extend_from_slice(&content[*from..*to]);
                line.cells.resize(cols, Cell::blank());
                line.mark_all_written();
                out.push_back(line);
            }
            for (decided, offset) in decided.iter_mut().zip(offsets.iter()) {
                if let Some(offset) = *offset {
                    let index = splits
                        .iter()
                        .rposition(|(from, _)| *from <= offset)
                        .unwrap_or(0);
                    *decided = Some((start + index, offset - splits[index].0));
                }
            }
        }

        // Blank rows past the text are not content: trimming them first keeps a
        // narrow window from spending its fresh rows on nothing and scrolling
        // the prompt off the top.
        let mut total = out.len();
        while total > rows
            && out[total - 1].cells.iter().all(Cell::is_padding)
            && decided[0].is_none_or(|(line, _)| line + 1 < total)
        {
            total -= 1;
        }
        out.truncate(total);
        let moved = total.saturating_sub(rows);
        let mut scrollback: VecDeque<Line> = out.drain(..moved).collect();
        while scrollback.len() > self.history {
            scrollback.pop_front();
        }
        // Rows the screen has room for again come back above it, so the text
        // below them stays where it was and the cursor moves down with it.
        let mut restored = 0;
        while out.len() < rows {
            match scrollback.pop_back() {
                Some(line) => {
                    out.push_front(line);
                    restored += 1;
                }
                None => {
                    let line = self.new_line();
                    out.push_back(line);
                }
            }
        }
        self.scrollback = scrollback;
        self.main = out;
        let placed = |(line, col): (usize, usize)| {
            let y = (line + restored).saturating_sub(moved).min(rows - 1);
            (y, col.min(cols - 1))
        };
        let mut pending = false;
        if let Some((line, column)) = decided[0] {
            let (y, x) = placed((line, column));
            // A wrap that was pending before the resize still is when its text
            // still reaches the margin, so the next character opens a row
            // instead of overwriting the last one.
            pending = was_pending && column + 1 == cols;
            self.cursor.y = y;
            self.cursor.x = x;
        }
        if let (Some(saved), Some(at)) = (&mut self.saved, decided[1]) {
            (saved.y, saved.x) = placed(at);
        }
        if let (Some(saved), Some(at)) = (&mut self.alt_saved, decided[2]) {
            (saved.y, saved.x) = placed(at);
        }
        // Anchors hold columns, and a rewrap changes them.
        self.selection = None;
        pending
    }

    /// Fits the primary screen to the current height, keeping its bottom in
    /// place. Blank rows below the text are dropped first, so shrinking does
    /// not scroll anything off the top for no reason. Every primary cursor
    /// moves with its text; on the alternate screen only the saved ones do,
    /// since the live cursor belongs to the application.
    fn fit_height(&mut self, live: bool) {
        let rows = self.rows;
        let mut moved = 0isize;
        while self.main.len() > rows
            && self
                .main
                .back()
                .is_some_and(|line| line.cells.iter().all(Cell::is_padding))
        {
            self.main.pop_back();
        }
        while self.main.len() > rows {
            let line = self
                .main
                .pop_front()
                .expect("main has more rows than it fits");
            self.scrollback.push_back(line);
            moved += 1;
        }
        while self.scrollback.len() > self.history {
            self.scrollback.pop_front();
        }
        let mut restored = 0;
        while self.main.len() < rows {
            match self.scrollback.pop_back() {
                Some(line) => {
                    self.main.push_front(line);
                    restored += 1;
                }
                None => {
                    let line = self.new_line();
                    self.main.push_back(line);
                }
            }
        }
        let shift = |y: usize| {
            (y as isize + restored as isize - moved)
                .max(0)
                .min(rows as isize - 1) as usize
        };
        if live {
            self.cursor.y = shift(self.cursor.y);
        }
        if let Some(saved) = &mut self.saved {
            saved.y = shift(saved.y);
        }
        if let Some(saved) = &mut self.alt_saved {
            saved.y = shift(saved.y);
        }
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
            let (fg, bg, attrs, link) = (self.pen.fg, self.pen.bg, self.pen.attrs, self.pen.link);
            let line = self.line_mut(y);
            if line.cells[x].width == 1 {
                line.cells[x] = Cell {
                    ch,
                    extra: 0,
                    fg,
                    bg,
                    attrs,
                    link,
                    width: 1,
                };
                line.mark_written_range(x, x);
                line.wrapped = false;
                line.wide_wrap = false;
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
                self.wrap_line();
            }
            self.wrap_pending = false;
        }
        if width == 2 && self.cursor.x + 1 >= self.cols {
            if self.autowrap {
                self.wrap_wide();
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
        let (fg, bg, attrs, link) = (self.pen.fg, self.pen.bg, self.pen.attrs, self.pen.link);
        let cell = Cell {
            ch,
            extra: 0,
            fg,
            bg,
            attrs,
            link,
            width: width as u8,
        };
        let line = self.line_mut(y);
        // Only neighbours outside the cells we overwrite need cleaning up.
        let old = line.cells[x];
        let old_next = line.cells.get(x + 1).copied();
        if old.width == 0 && x > 0 && line.cells[x - 1].width == 2 {
            let previous = line.cells[x - 1];
            line.cells[x - 1] = Cell {
                bg: previous.bg,
                ..Cell::blank()
            };
            line.mark_written(x - 1);
        }
        if width == 1 && old.width == 2 && x + 1 < cols {
            let next = line.cells[x + 1];
            line.cells[x + 1] = Cell {
                bg: next.bg,
                ..Cell::blank()
            };
            line.mark_written(x + 1);
        }
        if width == 2 {
            if old_next.is_some_and(|next| next.width == 2) && x + 2 < cols {
                let next = line.cells[x + 2];
                line.cells[x + 2] = Cell {
                    bg: next.bg,
                    ..Cell::blank()
                };
                line.mark_written(x + 2);
            }
            let mut continuation = cell;
            continuation.ch = ' ';
            continuation.extra = 0;
            continuation.width = 0;
            line.cells[x + 1] = continuation;
        }
        line.cells[x] = cell;
        line.mark_written_range(x, x + width - 1);
        line.wrapped = false;
        line.wide_wrap = false;
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
        let (fg, bg, attrs, link) = (self.pen.fg, self.pen.bg, self.pen.attrs, self.pen.link);
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
                let mut row_start = x0;
                lines[physical].wrapped = false;
                for (index, &byte) in bytes.iter().enumerate() {
                    let line = &mut lines[physical];
                    let old = line.cells[x];
                    if old.width == 0 {
                        if x > 0 && line.cells[x - 1].width == 2 {
                            let previous = line.cells[x - 1];
                            line.cells[x - 1] = Cell {
                                bg: previous.bg,
                                ..Cell::blank()
                            };
                        }
                    } else if old.width == 2 && x + 1 < cols {
                        let next = line.cells[x + 1];
                        line.cells[x + 1] = Cell {
                            bg: next.bg,
                            ..Cell::blank()
                        };
                    }
                    line.cells[x] = Cell {
                        ch: byte as char,
                        extra: 0,
                        fg,
                        bg,
                        attrs,
                        link,
                        width: 1,
                    };
                    x += 1;
                    if x == cols {
                        line.mark_written_range(row_start, cols - 1);
                        x = 0;
                        row_start = 0;
                        let filled = physical;
                        physical += 1;
                        if physical > region_bottom {
                            physical = region_top;
                        }
                        lines[physical].wrapped = false;
                        lines[physical].wide_wrap = false;
                        if index + 1 < bytes.len() {
                            lines[filled].wrapped = true;
                        }
                    }
                }
                if x > 0 {
                    lines[physical].mark_written_range(row_start, x - 1);
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
                    self.wrap_line();
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
                for (index, &byte) in segment.iter().enumerate() {
                    let x = x0 + index;
                    let old = line.cells[x];
                    if old.width == 0 {
                        if x > 0 && line.cells[x - 1].width == 2 {
                            let previous = line.cells[x - 1];
                            line.cells[x - 1] = Cell {
                                bg: previous.bg,
                                ..Cell::blank()
                            };
                        }
                    } else if old.width == 2 && x + 1 < cols {
                        let next = line.cells[x + 1];
                        line.cells[x + 1] = Cell {
                            bg: next.bg,
                            ..Cell::blank()
                        };
                    }
                    line.cells[x] = Cell {
                        ch: byte as char,
                        extra: 0,
                        fg,
                        bg,
                        attrs,
                        link,
                        width: 1,
                    };
                }
                line.mark_written_range(x0, x0 + segment.len() - 1);
                line.wrapped = false;
                line.wide_wrap = false;
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
            let (fg, bg, attrs, link) = (self.pen.fg, self.pen.bg, self.pen.attrs, self.pen.link);
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
                        extra: 0,
                        fg,
                        bg,
                        attrs,
                        link,
                        width: width as u8,
                    };
                    line.cells[x] = cell;
                    if width == 2 {
                        line.cells[x + 1] = Cell {
                            ch: ' ',
                            width: 0,
                            ..cell
                        };
                    }
                    last = Some(ch);
                    last_x = x;
                    x += width;
                    rest.next();
                }
                if last.is_some() {
                    line.mark_written_range(start, x - 1);
                    line.wrapped = false;
                    line.wide_wrap = false;
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
        let mut cluster = String::new();
        cluster.push(cell.ch);
        if cell.extra != 0
            && let Some(extra) = self.extras.get(cell.extra as usize - 1)
        {
            cluster.push_str(extra);
        }
        cluster.push(ch);
        let old_width = cell.width;
        let new_width = UnicodeWidthStr::width(cluster.as_str()).clamp(1, 2) as u8;
        cell.extra = self.intern_extra(&cluster[cell.ch.len_utf8()..]);
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
                        continuation.extra = 0;
                        continuation.width = 0;
                        line.cells[next] = continuation;
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

    /// Marks the current row as continued and moves the cursor to column zero
    /// of the row below. This is the only way a row gets `wrapped`, so a resize
    /// knows exactly which rows to join.
    fn wrap_line(&mut self) {
        let y = self.cursor.y;
        self.line_mut(y).wrapped = true;
        self.index();
        self.cursor.x = 0;
    }

    /// Continues the row below and leaves the margin cell it could not use as
    /// padding, which is what a double width character needs when it does not
    /// fit in the last column.
    fn wrap_wide(&mut self) {
        let y = self.cursor.y;
        self.wrap_line();
        let physical = self.physical_row(y);
        if let Some(line) = self.lines_mut().get_mut(physical) {
            line.wide_wrap = true;
        }
    }

    /// Moves the cursor down one line, scrolling at the bottom margin.
    fn index(&mut self) {
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
        // Single-line full-screen scroll with history is the common case in
        // streaming output. Skip the general region machinery entirely.
        if count == 1
            && self.scroll_top == 0
            && self.scroll_bottom + 1 == self.rows
            && self.scroll_offset == 0
            && self.region_offset == 0
            && self.alt.is_none()
            && self.history != 0
            && self.scrollback.len() == self.history
        {
            let removed = self.main.pop_front().unwrap();
            self.scrollback.push_back(removed);
            let mut line = self.scrollback.pop_front().unwrap();
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
                let removed = self.main.pop_front().unwrap_or_else(|| self.new_line());
                let mut replacement = None;
                if self.history != 0 {
                    self.scrollback.push_back(removed);
                    while self.scrollback.len() > self.history {
                        if let Some(mut evicted) = self.scrollback.pop_front() {
                            let id = self.next_line_id();
                            blank_in_place(&mut evicted, id);
                            replacement = Some(evicted);
                        }
                    }
                    if self.scroll_offset > 0 {
                        self.scroll_offset = (self.scroll_offset + 1).min(self.scrollback.len());
                    }
                } else {
                    let mut blank = removed;
                    let id = self.next_line_id();
                    blank_in_place(&mut blank, id);
                    replacement = Some(blank);
                }
                let line = replacement.unwrap_or_else(|| self.new_line());
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
        let shift = full && !self.in_alt() && self.scroll_offset == 0;
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
        let tail: Vec<Cell> = line.cells[x..cols - count].to_vec();
        line.cells[x + count..].copy_from_slice(&tail);
        for cell in &mut line.cells[x..x + count] {
            *cell = Cell {
                bg,
                ..Cell::blank()
            };
        }
        line.wrapped = false;
        line.wide_wrap = false;
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
        let tail: Vec<Cell> = line.cells[x + count..].to_vec();
        line.cells[x..cols - count].copy_from_slice(&tail);
        for cell in &mut line.cells[cols - count..] {
            *cell = Cell {
                bg,
                ..Cell::blank()
            };
        }
        line.wrapped = false;
        line.wide_wrap = false;
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
        let line = self.line_mut(y);
        if from > 0 && line.cells[from].width == 0 {
            line.cells[from - 1] = Cell {
                bg,
                ..Cell::blank()
            };
        }
        if line.cells[to].width == 2 && to + 1 < cols {
            line.cells[to + 1] = Cell {
                bg,
                ..Cell::blank()
            };
        }
        for cell in &mut line.cells[from..=to] {
            *cell = Cell {
                bg,
                ..Cell::blank()
            };
        }
        line.wrapped = false;
        line.wide_wrap = false;
        line.mark_all_written();
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
        let count = count.min(self.scroll_bottom - self.cursor.y + 1);
        for _ in 0..count {
            let line = self.new_line();
            let y = self.cursor.y;
            let bottom = self.scroll_bottom;
            let lines = self.lines_mut();
            lines.remove(bottom);
            lines.insert(y, line);
        }
        self.mark_all();
    }

    /// Deletes lines at the cursor within the scroll region.
    fn delete_lines(&mut self, count: usize) {
        if self.cursor.y < self.scroll_top || self.cursor.y > self.scroll_bottom {
            return;
        }
        self.normalize_region();
        let count = count.min(self.scroll_bottom - self.cursor.y + 1);
        for _ in 0..count {
            let line = self.new_line();
            let y = self.cursor.y;
            let bottom = self.scroll_bottom;
            let lines = self.lines_mut();
            lines.remove(y);
            lines.insert(bottom, line);
        }
        self.mark_all();
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
        self.cursor.visible = true;
        self.cursor.style = self.default_cursor_style;
        self.cursor.blinking = self.default_cursor_blink;
        self.pen = Pen::default();
        self.active_link = 0;
        self.saved = None;
        self.alt_saved = None;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.charsets = [Charset::Ascii; 4];
        self.active_charset = 0;
        self.single_shift = None;
        self.insert_mode = false;
        self.newline_mode = false;
        self.origin_mode = false;
        self.autowrap = true;
        self.reverse_video = false;
        self.app_cursor = false;
        self.app_keypad = false;
        self.bracketed_paste = false;
        self.focus_events = false;
        self.mouse_mode = MouseMode::Off;
        self.kitty_flags = [0, 0];
        self.kitty_stack = [Vec::new(), Vec::new()];
        self.mouse_encoding = MouseEncoding::X10;
        self.theme = self.default_theme.clone();
        self.reset_tabs();
        self.selection = None;
        if let Some(search) = &mut self.search {
            search.matches.clear();
            search.current = 0;
            search.lines.clear();
        }
        self.dirty = vec![false; self.rows];
        self.dirty_all = true;
        self.wrap_pending = false;
        self.last_printed = None;
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
    line.wide_wrap = false;
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

/// Splits joined row content into `cols` wide rows without cutting a double
/// width character in half: when one straddles the boundary it moves to the
/// next row and the row it left is padded, as the print path does at the
/// margin. Empty content is one empty row, so a blank line stays a line.
fn split(content: &[Cell], cols: usize, rows: &mut Vec<(usize, usize)>) {
    rows.clear();
    let mut at = 0;
    while at < content.len() {
        let mut take = (content.len() - at).min(cols);
        if at + take < content.len() && content[at + take - 1].width == 2 && take > 1 {
            take -= 1;
        }
        rows.push((at, at + take));
        at += take;
    }
    if rows.is_empty() {
        rows.push((0, 0));
    }
}

/// Length of a row up to its last styled or non-blank cell. The blanks that
/// follow are padding an autowrap added to fill the row, and a rewrap drops
/// them instead of turning them into text.
fn content_len(cells: &[Cell]) -> usize {
    cells
        .iter()
        .rposition(|cell| !cell.is_padding())
        .map_or(0, |index| index + 1)
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

    /// Every row of history and screen, with the padding cells removed.
    fn all_text(terminal: &Terminal) -> String {
        let mut text = String::new();
        for line in terminal.all_lines() {
            let end = if line.wrapped {
                line.cells.len()
            } else {
                content_len(&line.cells)
            };
            for cell in &line.cells[..end] {
                if cell.width != 0 {
                    text.push(cell.ch);
                }
            }
        }
        text
    }

    /// The visible text as logical lines, joining rows an autowrap continued.
    fn reflow_text(terminal: &Terminal) -> Vec<String> {
        let mut lines = Vec::new();
        let mut current = String::new();
        for row in 0..terminal.rows {
            let line = &terminal.lines()[row];
            let end = if line.wrapped {
                line.cells.len()
            } else {
                content_len(&line.cells)
            };
            for cell in &line.cells[..end] {
                if cell.width != 0 {
                    current.push(cell.ch);
                }
            }
            if !line.wrapped {
                lines.push(std::mem::take(&mut current));
            }
        }
        lines.retain(|line| !line.is_empty());
        lines
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
        extra: u32,
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
                            extra: c.extra,
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
                        extra: 0,
                        fg: Color::Default,
                        bg: Color::Default,
                        attrs: Attrs::default(),
                        link: 0,
                        width: 2,
                    };
                    t.lines_mut()[0].cells[4] = Cell {
                        ch: ' ',
                        extra: 0,
                        fg: Color::Default,
                        bg: Color::Default,
                        attrs: Attrs::default(),
                        link: 0,
                        width: 0,
                    };
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
        assert_ne!(cell.extra, 0);
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
    fn resize_rewraps_a_paragraph_and_restores_it() {
        let mut terminal = terminal(8);
        let text = "the quick brown fox jumps over the lazy dog";
        feed(&mut terminal, text.as_bytes());
        assert_eq!(reflow_text(&terminal), vec![text]);
        assert_eq!(terminal.cursor.y, 2);

        terminal.resize(10, 8, 9, 18);
        assert_eq!(reflow_text(&terminal), vec![text]);
        assert_eq!(terminal.cursor.y, 4);
        for (row, chunk) in [
            "the quick ",
            "brown fox ",
            "jumps over",
            " the lazy ",
            "dog",
        ]
        .iter()
        .enumerate()
        {
            let line = &terminal.lines()[row];
            assert!(line.wrapped || row == 4);
            let end = if line.wrapped { line.cells.len() } else { 3 };
            let got: String = line.cells[..end].iter().map(|cell| cell.ch).collect();
            assert_eq!(&got, chunk, "row {row}");
        }

        terminal.resize(20, 8, 9, 18);
        assert_eq!(reflow_text(&terminal), vec![text]);
        assert_eq!(terminal.cursor.y, 2);
        terminal.resize(50, 8, 9, 18);
        assert_eq!(reflow_text(&terminal), vec![text]);
        assert_eq!((terminal.cursor.x, terminal.cursor.y), (43, 0));
    }

    #[test]
    fn resize_keeps_rows_separated_by_a_newline() {
        let mut terminal = terminal(3);
        feed(&mut terminal, b"one line\r\ntwo line\r\nthree line");
        terminal.resize(10, 3, 9, 18);
        assert_eq!(
            reflow_text(&terminal),
            vec!["one line", "two line", "three line"]
        );
        for row in 3..terminal.rows {
            assert!(!terminal.lines()[row].wrapped);
        }
    }

    #[test]
    fn resize_moves_the_cursor_with_its_text() {
        let mut terminal = terminal(4);
        feed(
            &mut terminal,
            b"the quick brown fox jumps over the lazy dog",
        );
        assert_eq!((terminal.cursor.x, terminal.cursor.y), (3, 2));
        terminal.resize(10, 4, 9, 18);
        // Five rows of text in four rows of screen: the first scrolls away and
        // the cursor ends just past "dog".
        assert_eq!(terminal.scrollback_len(), 1);
        assert_eq!((terminal.cursor.x, terminal.cursor.y), (3, 3));
        assert_eq!(terminal.visible_line(3).unwrap().cells[2].ch, 'g');
        terminal.resize(30, 4, 9, 18);
        assert_eq!((terminal.cursor.x, terminal.cursor.y), (13, 1));
    }

    #[test]
    fn resize_moves_rows_that_no_longer_fit_into_scrollback() {
        let mut terminal = terminal(3);
        feed(
            &mut terminal,
            b"aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii",
        );
        let text = reflow_text(&terminal).concat();
        terminal.resize(10, 3, 9, 18);
        assert!(terminal.scrollback_len() > 0);
        assert_eq!(all_text(&terminal), text);
        terminal.resize(40, 3, 9, 18);
        assert_eq!(all_text(&terminal), text);
    }

    #[test]
    fn resize_shrinking_the_height_keeps_the_bottom_row() {
        let mut terminal = terminal(4);
        feed(&mut terminal, b"one\r\ntwo\r\nthree\r\nfour");
        terminal.resize(20, 2, 9, 18);
        assert_eq!(terminal.visible_line(1).unwrap().cells[0].ch, 'f');
        assert_eq!(terminal.visible_line(0).unwrap().cells[0].ch, 't');
        assert_eq!(terminal.cursor.y, 1);
        terminal.resize(20, 4, 9, 18);
        assert_eq!(terminal.visible_line(3).unwrap().cells[0].ch, 'f');
        assert_eq!(terminal.visible_line(2).unwrap().cells[0].ch, 't');
        assert_eq!(terminal.cursor.y, 3);
    }

    #[test]
    fn resize_keeps_styled_trailing_blanks() {
        let mut terminal = terminal(3);
        feed(&mut terminal, b"\x1b[44m      \x1b[0m");
        terminal.resize(10, 3, 9, 18);
        let end = content_len(&terminal.lines()[0].cells);
        assert_eq!(end, 6);
        assert_eq!(terminal.lines()[0].cells[5].bg, Color::Indexed(4));
    }

    #[test]
    fn resize_does_not_reflow_the_alternate_screen() {
        let mut terminal = terminal(4);
        feed(&mut terminal, b"hello\x1b[?1049hALT");
        terminal.resize(10, 4, 9, 18);
        assert_eq!(terminal.visible_line(0).unwrap().cells[0].ch, 'A');
        feed(&mut terminal, b"\x1b[?1049l");
        assert_eq!(terminal.visible_line(0).unwrap().cells[0].ch, 'h');
    }

    #[test]
    fn reflow_produces_the_same_rows_whether_print_ran_fast_or_slow() {
        // The fast paths set and clear `wrapped` without touching cells, so a
        // reflow must land on exactly the same rows as the per-character path.
        let text = "the quick brown fox jumps over the lazy dog \
            and then keeps going for a couple more lines of text";
        let mut slow = terminal(6);
        let mut fast = terminal(6);
        for ch in text.chars() {
            slow.print(ch);
        }
        fast.print_text_run(text);
        for width in [7usize, 13, 20, 40] {
            slow.resize(width, 6, 9, 18);
            fast.resize(width, 6, 9, 18);
            assert_eq!(all_text(&slow), all_text(&fast), "width {width}");
        }
    }

    #[test]
    fn resize_wraps_a_row_that_did_not_wrap_before() {
        let mut terminal = terminal(2);
        feed(&mut terminal, b"one\r\ntwo lines that need to wrap now");
        terminal.resize(12, 4, 9, 18);
        assert_eq!(
            reflow_text(&terminal),
            vec!["one", "two lines that need to wrap now"]
        );
        assert_eq!(terminal.scrollback_len(), 0);
        let rows: Vec<(bool, String)> = (0..4)
            .map(|row| {
                let line = &terminal.lines()[row];
                let end = if line.wrapped { 12 } else { 7 };
                let text: String = line.cells[..end].iter().map(|cell| cell.ch).collect();
                (line.wrapped, text)
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                (false, "one    ".to_string()),
                (true, "two lines th".to_string()),
                (true, "at need to w".to_string()),
                (false, "rap now".to_string()),
            ]
        );
    }

    #[test]
    fn resize_growing_the_height_restores_scrolled_rows() {
        let mut terminal = terminal(3);
        feed(&mut terminal, b"a\r\nb\r\nc\r\nd\r\ne");
        assert_eq!(terminal.scrollback_len(), 2);
        terminal.resize(20, 5, 9, 18);
        assert_eq!(all_text(&terminal), "abcde");
        assert_eq!(terminal.scrollback_len(), 0);
        assert_eq!(terminal.cursor.y, 4);
    }

    #[test]
    fn resize_reflows_rows_already_in_scrollback() {
        let mut terminal = terminal(3);
        let text = "one two three four five six seven eight nine ten";
        feed(&mut terminal, text.as_bytes());
        terminal.resize(40, 3, 9, 18);
        assert_eq!(terminal.scrollback_len(), 0);
        terminal.resize(10, 3, 9, 18);
        assert_eq!(all_text(&terminal), text);
        terminal.resize(40, 3, 9, 18);
        assert_eq!(all_text(&terminal), text);
    }

    #[test]
    fn resize_does_not_split_a_wide_character() {
        // A width two cell that cannot fit stays whole: it moves to the next
        // row, and the row it left is padded, as when printing at the margin.
        let mut terminal = terminal(5);
        feed(&mut terminal, "a\u{4e16}b\u{4e16}".as_bytes());
        for cols in [2usize, 3, 7] {
            terminal.resize(cols, 5, 9, 18);
            let mut text = String::new();
            for row in 0..5 {
                let line = &terminal.lines()[row];
                for col in 0..line.cells.len() {
                    if line.cells[col].width == 2 {
                        assert!(col + 1 < line.cells.len(), "cols {cols} row {row}");
                        assert_eq!(line.cells[col + 1].width, 0, "cols {cols} row {row}");
                    }
                }
                let end = if line.wrapped {
                    line.cells.len()
                } else {
                    content_len(&line.cells)
                };
                text.extend(line.cells[..end].iter().map(|cell| cell.ch));
            }
            assert_eq!(text.matches('\u{4e16}').count(), 2, "cols {cols}");
            assert!(text.contains('a') && text.contains('b'), "cols {cols}");
        }
    }

    #[test]
    fn reflow_keeps_every_row_within_the_width() {
        let mut terminal = terminal(4);
        let mut parser = Parser::new();
        let mut seed = 1234u32;
        for _ in 0..400 {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let cols = 4 + (seed >> 16) as usize % 30;
            let rows = 2 + (seed >> 8) as usize % 6;
            terminal.resize(cols, rows, 9, 18);
            parser.advance(
                b"some text that wraps around a narrow window\r\n",
                &mut terminal,
            );
            for line in terminal.scrollback.iter().chain(terminal.lines().iter()) {
                assert_eq!(line.cells.len(), cols, "cols {cols}");
                for (col, cell) in line.cells.iter().enumerate() {
                    if cell.width == 2 {
                        assert!(col + 1 < line.cells.len(), "wide char split");
                        assert_eq!(line.cells[col + 1].width, 0);
                    }
                }
            }
            assert!(terminal.cursor.x < cols && terminal.cursor.y < rows);
        }
    }

    #[test]
    fn resize_while_in_alternate_reflows_the_primary_behind_it() {
        let mut terminal = terminal(4);
        let text = "some text long enough to wrap when the window narrows";
        feed(&mut terminal, text.as_bytes());
        feed(&mut terminal, b"\x1b[?1049hfull screen");
        terminal.resize(10, 4, 9, 18);
        assert_eq!(terminal.visible_line(0).unwrap().cells[0].ch, 'f');
        feed(&mut terminal, b"\x1b[?1049l");
        assert_eq!(all_text(&terminal), text);
    }

    #[test]
    fn a_run_that_ends_on_the_margin_is_still_continued() {
        // The print path sets wrap_pending at the margin, so the row is full and
        // the next one continues it, whether or not text follows in the run.
        let mut terminal = Terminal::new(
            4,
            3,
            10,
            Theme::from_config(&crate::config::ColorsConfig::default()).unwrap(),
            CursorStyle::Block,
            false,
        );
        terminal.print_ascii_run(b"abcd");
        assert!(terminal.wrap_pending);
        assert!(!terminal.lines()[0].wrapped);
        terminal.print_ascii_run(b"e");
        assert!(terminal.lines()[0].wrapped);
        assert_eq!(terminal.lines()[1].cells[0].ch, 'e');
        terminal.resize(2, 3, 9, 18);
        assert_eq!(all_text(&terminal), "abcde");
    }

    #[test]
    fn resize_a_full_row_keeps_the_next_character_on_a_new_row() {
        // A row that filled the old width ends with a wrap pending. After a
        // reflow the text still reaches the margin, so the next character must
        // open a row instead of overwriting the last one.
        let mut terminal = Terminal::new(
            20,
            3,
            10,
            Theme::from_config(&crate::config::ColorsConfig::default()).unwrap(),
            CursorStyle::Block,
            false,
        );
        terminal.print_ascii_run(b"aaaaaaaaaaaaaaaaaaaa");
        terminal.resize(10, 3, 9, 18);
        assert!(terminal.wrap_pending);
        terminal.print('b');
        assert_eq!(all_text(&terminal), "aaaaaaaaaaaaaaaaaaaab");
        assert_eq!(terminal.cursor.y, 2);
    }

    #[test]
    fn resize_in_the_alternate_screen_remaps_the_saved_cursor() {
        // The primary rows reflow while the alternate screen is up, so the
        // cursor saved for the return has to move with its text too.
        let text = "some text long enough to wrap when the window narrows";
        let theme = Theme::from_config(&crate::config::ColorsConfig::default()).unwrap();
        let mut direct = Terminal::new(4, 4, 10, theme.clone(), CursorStyle::Block, false);
        let mut round_trip = Terminal::new(4, 4, 10, theme, CursorStyle::Block, false);
        for terminal in [&mut direct, &mut round_trip] {
            feed(terminal, text.as_bytes());
        }
        direct.resize(20, 4, 9, 18);
        feed(&mut round_trip, b"\x1b[?1049hx");
        round_trip.resize(20, 4, 9, 18);
        feed(&mut round_trip, b"\x1b[?1049l");
        assert_eq!(
            (direct.cursor.x, direct.cursor.y),
            (round_trip.cursor.x, round_trip.cursor.y)
        );
        assert_eq!(
            direct.visible_line(direct.cursor.y).unwrap().cells[0].ch,
            round_trip.visible_line(round_trip.cursor.y).unwrap().cells[0].ch
        );
    }

    #[test]
    fn a_widened_row_does_not_turn_wrap_padding_into_text() {
        // A double width character that did not fit left the margin blank. A
        // reflow moves that padding away instead of copying it as a space.
        let mut terminal = Terminal::new(
            2,
            5,
            10,
            Theme::from_config(&crate::config::ColorsConfig::default()).unwrap(),
            CursorStyle::Block,
            false,
        );
        let text = "a\u{4e16}b\u{4e16}";
        feed(&mut terminal, text.as_bytes());
        terminal.select_all();
        assert_eq!(
            terminal.selection_text().as_deref(),
            Some("a\u{4e16}b\u{4e16}\n ")
        );
        terminal.resize(10, 5, 9, 18);
        terminal.select_all();
        let copied = terminal.selection_text().unwrap();
        assert!(copied.starts_with(text), "{copied:?}");
        assert!(!copied.contains("a \u{4e16}"), "{copied:?}");
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
                    c.ch, c.width, c.extra, c.fg, c.bg
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
