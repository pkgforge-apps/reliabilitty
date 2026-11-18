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
}

impl Default for Pen {
    fn default() -> Self {
        Self {
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::default(),
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
            width: 1,
        }
    }

    pub fn is_blank(&self) -> bool {
        self.ch == ' ' && self.extra == 0 && self.width != 0
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
    blank_lines: Vec<Line>,
    dirty: Vec<bool>,
    dirty_all: bool,
    pending_scroll: i32,
    line_id_next: u64,
    extras: Vec<Box<str>>,
    extras_map: HashMap<Box<str>, u32>,
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
            blank_lines: Vec::new(),
            dirty: vec![false; rows],
            dirty_all: true,
            pending_scroll: 0,
            line_id_next: 0,
            extras: Vec::new(),
            extras_map: HashMap::new(),
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

    fn all_lines(&self) -> Vec<&Line> {
        let mut lines = Vec::with_capacity(self.scrollback.len() + self.lines().len());
        lines.extend(self.scrollback.iter());
        lines.extend(self.lines());
        lines
    }

    fn search_lines(&self) -> Vec<&Line> {
        if self.in_alt() {
            self.lines().iter().collect()
        } else {
            self.all_lines()
        }
    }

    pub fn visible_line(&self, row: usize) -> Option<&Line> {
        if row >= self.rows {
            return None;
        }
        let active = self.lines();
        if self.scroll_offset == 0 {
            return active.get(row);
        }
        let saved = self.scrollback.len();
        let start = saved.saturating_sub(self.scroll_offset);
        let index = start + row;
        if index < saved {
            self.scrollback.get(index)
        } else {
            active.get(index - saved)
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
        for line in &mut self.scrollback {
            resize_line(line, cols);
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

    pub fn set_window_pixels(&mut self, width: u16, height: u16) {
        self.window_width = width;
        self.window_height = height;
    }

    pub fn take_damage(&mut self) -> Damage {
        let all = std::mem::take(&mut self.dirty_all);
        let rows = std::mem::replace(&mut self.dirty, vec![false; self.rows]);
        let scroll = std::mem::take(&mut self.pending_scroll);
        Damage { all, rows, scroll }
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

    fn new_line(&mut self) -> Line {
        let id = self.line_id_next;
        self.line_id_next = self.line_id_next.wrapping_add(1);
        let cells = if let Some(mut line) = self.blank_lines.pop() {
            line.cells.resize(self.cols, Cell::blank());
            line.cells
        } else {
            vec![Cell::blank(); self.cols]
        };
        Line::new(id, cells)
    }

    fn recycle_line(&mut self, mut line: Line) {
        line.clear_written();
        if self.blank_lines.len() < 64 {
            self.blank_lines.push(line);
        }
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
        self.clear_wide_at(y, x);
        if width == 2 {
            self.clear_wide_at(y, x + 1);
        }
        let cell = Cell {
            ch,
            extra: 0,
            fg: self.pen.fg,
            bg: self.pen.bg,
            attrs: self.pen.attrs,
            width: width as u8,
        };
        {
            let line = &mut self.lines_mut()[y];
            line.cells[x] = cell;
            line.mark_written(x);
            line.wrapped = false;
            if width == 2 {
                let mut continuation = cell;
                continuation.ch = ' ';
                continuation.extra = 0;
                continuation.width = 0;
                line.cells[x + 1] = continuation;
                line.mark_written(x + 1);
            }
        }
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
        let (fg, bg, attrs) = (self.pen.fg, self.pen.bg, self.pen.attrs);
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
                let mut y = start_y;
                for &byte in bytes {
                    let line = &mut lines[y];
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
                        width: 1,
                    };
                    line.mark_written(x);
                    line.wrapped = false;
                    x += 1;
                    if x == cols {
                        x = 0;
                        y += 1;
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
                let line = &mut lines[y];
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
                        width: 1,
                    };
                    line.mark_written(x);
                }
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
        let mut cell = self.lines()[y].cells[x];
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
            let line = &mut self.lines_mut()[y];
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

    fn clear_wide_at(&mut self, y: usize, x: usize) {
        let cols = self.cols;
        let line = &mut self.lines_mut()[y];
        let Some(cell) = line.cells.get(x).copied() else {
            return;
        };
        if cell.width == 0 {
            if x > 0 {
                let previous = line.cells[x - 1];
                if previous.width == 2 {
                    line.cells[x - 1] = Cell {
                        bg: previous.bg,
                        ..Cell::blank()
                    };
                    line.mark_written(x - 1);
                }
            }
            line.cells[x] = Cell {
                bg: cell.bg,
                ..Cell::blank()
            };
            line.mark_written(x);
        } else if cell.width == 2 && x + 1 < cols {
            let next = line.cells[x + 1];
            line.cells[x + 1] = Cell {
                bg: next.bg,
                ..Cell::blank()
            };
            line.mark_written(x + 1);
        }
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
            for _ in 0..count {
                let removed = self.main.pop_front().unwrap_or_else(|| self.new_line());
                if self.history != 0 {
                    self.scrollback.push_back(removed);
                    while self.scrollback.len() > self.history {
                        if let Some(evicted) = self.scrollback.pop_front() {
                            self.recycle_line(evicted);
                        }
                    }
                    if self.scroll_offset > 0 {
                        self.scroll_offset = (self.scroll_offset + 1).min(self.scrollback.len());
                    }
                } else {
                    self.recycle_line(removed);
                }
                let line = self.new_line();
                self.main.push_back(line);
            }
        } else {
            let lines = self.lines_mut().make_contiguous();
            for _ in 0..count {
                for column in top..bottom {
                    lines.swap(column, column + 1);
                }
                lines[bottom].clear_written();
                lines[bottom].wrapped = false;
            }
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
            for _ in 0..count {
                let removed = self
                    .lines_mut()
                    .pop_back()
                    .unwrap_or_else(|| self.new_line());
                self.recycle_line(removed);
                let line = self.new_line();
                self.lines_mut().push_front(line);
            }
        } else {
            let lines = self.lines_mut().make_contiguous();
            for _ in 0..count {
                for column in (top..bottom).rev() {
                    lines.swap(column, column + 1);
                }
                lines[top].clear_written();
                lines[top].wrapped = false;
            }
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
        let line = &mut self.lines_mut()[y];
        let tail: Vec<Cell> = line.cells[x..cols - count].to_vec();
        line.cells[x + count..].copy_from_slice(&tail);
        for cell in &mut line.cells[x..x + count] {
            *cell = Cell {
                bg,
                ..Cell::blank()
            };
        }
        line.mark_all_written();
        self.mark_row(y);
    }

    fn delete_cells(&mut self, count: usize) {
        let x = self.cursor.x;
        let y = self.cursor.y;
        let count = count.min(self.cols - x);
        let cols = self.cols;
        let bg = self.pen.bg;
        let line = &mut self.lines_mut()[y];
        let tail: Vec<Cell> = line.cells[x + count..].to_vec();
        line.cells[x..cols - count].copy_from_slice(&tail);
        for cell in &mut line.cells[cols - count..] {
            *cell = Cell {
                bg,
                ..Cell::blank()
            };
        }
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
        let line = &mut self.lines_mut()[y];
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
        } else if self.alt.take().is_some() {
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
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.insert_mode = false;
        self.origin_mode = false;
        self.autowrap = true;
        self.app_cursor = false;
        self.app_keypad = false;
        self.mouse_mode = MouseMode::Off;
        self.mouse_encoding = MouseEncoding::X10;
        self.charsets = [Charset::Ascii; 4];
        self.active_charset = 0;
        self.single_shift = None;
        self.wrap_pending = false;
        self.last_printed = None;
        self.mark_all();
    }
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
    fn alternate_screen_restores_primary() {
        let mut terminal = terminal(3);
        feed(&mut terminal, b"one\x1b[?1049hALT\x1b[?1049l");
        assert_eq!(terminal.lines()[0].cells[0].ch, 'o');
        assert!(!terminal.in_alt());
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
