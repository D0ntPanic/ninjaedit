//! The terminal emulator: feeds a program's output through a VT escape
//! sequence parser and keeps the screen it describes.
//!
//! The emulator understands what a modern full-screen program (an editor,
//! a coding agent) expects of an xterm-compatible terminal: the cursor and
//! erase controls, scroll regions, the alternate screen, 256 and direct
//! colors with the usual attributes, bracketed paste, mouse reporting, and
//! the queries programs use to find out where the cursor is, how big the
//! screen is, and what colors it has. Programs are told about a screen
//! that treats a grapheme cluster as one character (mode 2027), as the
//! editor does: combining marks and joiners attach to the cell before
//! them and the cell is as wide as the cluster.
//!
//! Lines that scroll off the top of the primary screen go into a
//! scrollback buffer, up to a limit; the alternate screen has none.
//!
//! Bytes go in through [`Terminal::process`]. Whatever the program should
//! be told in return (a cursor position report, the answer to a color
//! query) collects in [`Terminal::take_responses`], and things the
//! frontend might act on (the bell, text for the clipboard) in
//! [`Terminal::take_events`]. The screen is read back a row at a time with
//! [`Terminal::rows`], which takes a scrollback offset so that a frontend
//! showing older output needs no copy.

use super::cell::{Cell, Color, Style, Underline};
use super::grid::{Grid, Row};
use super::keys::{self, Key, Modifiers, MouseEvent};
use crate::text;
use compact_str::CompactString;
use std::collections::VecDeque;
use unicode_segmentation::UnicodeSegmentation;
use vte::{Params, Perform};

/// Lines of scrollback kept by default.
pub const DEFAULT_SCROLLBACK: usize = 100_000;
/// Tab stops start out this many columns apart.
const TAB_WIDTH: usize = 8;

/// Something the program did that the frontend may want to act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The program rang the bell.
    Bell,
    /// The program put text on the clipboard (OSC 52).
    Clipboard(String),
    /// The program set the window title (OSC 0 or 2).
    Title(String),
}

/// How the program asked for the cursor to be drawn (DECSCUSR).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorStyle {
    #[default]
    Block,
    Underline,
    Bar,
}

/// Which mouse events the program asked to hear about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum MouseMode {
    #[default]
    None,
    /// Button presses only (mode 9).
    X10,
    /// Presses, releases, and the wheel (mode 1000).
    Normal,
    /// Also motion with a button held (mode 1002).
    Button,
    /// All motion (mode 1003).
    Any,
}

/// The terminal's settable modes, as the program left them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modes {
    /// DECCKM: arrow keys send SS3 sequences rather than CSI.
    pub application_cursor_keys: bool,
    /// DECKPAM: the keypad sends application sequences.
    pub application_keypad: bool,
    /// DECOM: cursor positions are relative to the scroll region.
    pub origin: bool,
    /// DECAWM: text wraps at the right edge.
    pub autowrap: bool,
    /// DECTCEM: the cursor is shown.
    pub cursor_visible: bool,
    /// IRM: printed text pushes what's after it along the row.
    pub insert: bool,
    /// LNM: a line feed also returns to the start of the line.
    pub newline: bool,
    pub mouse: MouseMode,
    /// Mouse reports use the SGR encoding (mode 1006), which has no limit
    /// on the coordinates.
    pub mouse_sgr: bool,
    /// The program wants to know when the terminal gains and loses focus
    /// (mode 1004).
    pub focus_events: bool,
    /// Pasted text is wrapped in markers so the program can tell it from
    /// typing (mode 2004).
    pub bracketed_paste: bool,
    /// The program asked for output to be held until a frame is complete
    /// (mode 2026). Recorded, but not acted on.
    pub synchronized_output: bool,
}

impl Default for Modes {
    fn default() -> Modes {
        Modes {
            application_cursor_keys: false,
            application_keypad: false,
            origin: false,
            autowrap: true,
            cursor_visible: true,
            insert: false,
            newline: false,
            mouse: MouseMode::None,
            mouse_sgr: false,
            focus_events: false,
            bracketed_paste: false,
            synchronized_output: false,
        }
    }
}

/// The character set a designator selects: plain, or the DEC line
/// drawing set, in which lowercase letters become box-drawing characters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Charset {
    #[default]
    Ascii,
    DecSpecial,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Cursor {
    row: usize,
    col: usize,
    /// The last column has been written and the next character goes at
    /// the start of the next line. The cursor stays on the last column
    /// meanwhile so a program can still see it there.
    pending_wrap: bool,
}

/// What DECSC saves and DECRC restores.
#[derive(Clone, Copy, Debug)]
struct SavedCursor {
    cursor: Cursor,
    pen: Style,
    origin: bool,
    charsets: [Charset; 2],
    active_charset: usize,
}

/// A terminal emulator: the screen a program's output describes.
pub struct Terminal {
    parser: vte::Parser,
    inner: Inner,
}

/// The emulator state, separate from the parser so the parser can drive
/// it.
struct Inner {
    cols: usize,
    rows: usize,
    primary: Grid,
    alternate: Grid,
    alternate_active: bool,
    scrollback: VecDeque<Row>,
    scrollback_limit: usize,
    cursor: Cursor,
    saved_primary: Option<SavedCursor>,
    saved_alternate: Option<SavedCursor>,
    /// The attributes new text is written with.
    pen: Style,
    /// The scroll region, inclusive of both rows.
    scroll_top: usize,
    scroll_bottom: usize,
    tabs: Vec<bool>,
    modes: Modes,
    cursor_style: CursorStyle,
    charsets: [Charset; 2],
    active_charset: usize,
    title: String,
    /// The character last printed, for REP.
    last_printed: Option<char>,
    /// What the program's default colors look like on screen, for it to
    /// ask about.
    default_fg: (u8, u8, u8),
    default_bg: (u8, u8, u8),
    responses: Vec<u8>,
    events: Vec<Event>,
    generation: u64,
}

impl Terminal {
    /// A blank terminal of the given size with the default amount of
    /// scrollback. Sizes are clamped to at least one cell.
    pub fn new(cols: usize, rows: usize) -> Terminal {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Terminal {
            parser: vte::Parser::new(),
            inner: Inner {
                cols,
                rows,
                primary: Grid::new(cols, rows),
                alternate: Grid::new(cols, rows),
                alternate_active: false,
                scrollback: VecDeque::new(),
                scrollback_limit: DEFAULT_SCROLLBACK,
                cursor: Cursor::default(),
                saved_primary: None,
                saved_alternate: None,
                pen: Style::default(),
                scroll_top: 0,
                scroll_bottom: rows - 1,
                tabs: default_tabs(cols),
                modes: Modes::default(),
                cursor_style: CursorStyle::default(),
                charsets: [Charset::Ascii; 2],
                active_charset: 0,
                title: String::new(),
                last_printed: None,
                default_fg: (0xe0, 0xe0, 0xe0),
                default_bg: (0x1c, 0x1c, 0x1c),
                responses: Vec::new(),
                events: Vec::new(),
                generation: 0,
            },
        }
    }

    /// Change how many lines of scrollback are kept.
    pub fn set_scrollback_limit(&mut self, limit: usize) {
        self.inner.scrollback_limit = limit;
        self.inner.trim_scrollback();
    }

    /// Tell the emulator what its default colors look like, so that a
    /// program asking (OSC 10 and 11, which is how some find out whether
    /// they are on a dark background) gets the right answer.
    pub fn set_default_colors(&mut self, fg: (u8, u8, u8), bg: (u8, u8, u8)) {
        self.inner.default_fg = fg;
        self.inner.default_bg = bg;
    }

    /// Feed the program's output through the emulator.
    pub fn process(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.parser.advance(&mut self.inner, bytes);
        self.inner.generation += 1;
    }

    /// Bytes the program should be sent in reply to its queries,
    /// accumulated since the last call.
    pub fn take_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.inner.responses)
    }

    /// What the program did that the frontend may act on, since the
    /// last call.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.inner.events)
    }

    /// A counter that changes whenever the screen may have, so a frontend
    /// can tell when to redraw.
    pub fn generation(&self) -> u64 {
        self.inner.generation
    }

    /// The screen size as (columns, rows).
    pub fn size(&self) -> (usize, usize) {
        (self.inner.cols, self.inner.rows)
    }

    /// Change the screen size. Rows the primary screen loses at the top
    /// go to the scrollback, and rows it gains come back from there, so
    /// that what's at the bottom stays there. Text isn't reflowed.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.inner.resize(cols.max(1), rows.max(1));
    }

    /// How many lines the scrollback holds.
    pub fn scrollback_len(&self) -> usize {
        self.inner.scrollback.len()
    }

    /// The rows on screen when scrolled back `scroll` lines, top to
    /// bottom: the last `scroll` lines of scrollback and then the top of
    /// the screen. Scrollback rows may be shorter than the screen; the
    /// cells beyond their end are blank. On the alternate screen there is
    /// no scrollback, and `scroll` is ignored.
    pub fn rows(&self, scroll: usize) -> impl Iterator<Item = &Row> {
        let inner = &self.inner;
        let scroll = if inner.alternate_active {
            0
        } else {
            scroll.min(inner.scrollback.len())
        };
        let from_scrollback = inner.scrollback.len() - scroll;
        inner
            .scrollback
            .range(from_scrollback..)
            .chain(inner.grid().iter())
            .take(inner.rows)
    }

    /// The text of one screen row, trailing spaces removed.
    pub fn row_text(&self, row: usize) -> String {
        self.inner.grid().row(row).text()
    }

    /// The cursor as (column, row), when it is shown.
    pub fn cursor(&self) -> Option<(usize, usize)> {
        self.inner
            .modes
            .cursor_visible
            .then_some((self.inner.cursor.col, self.inner.cursor.row))
    }

    /// The cursor as (column, row), shown or not.
    pub fn cursor_position(&self) -> (usize, usize) {
        (self.inner.cursor.col, self.inner.cursor.row)
    }

    pub fn cursor_style(&self) -> CursorStyle {
        self.inner.cursor_style
    }

    /// The window title the program set, or empty.
    pub fn title(&self) -> &str {
        &self.inner.title
    }

    pub fn modes(&self) -> &Modes {
        &self.inner.modes
    }

    /// Whether the program has switched to the alternate screen, as
    /// full-screen programs do.
    pub fn is_alternate_screen(&self) -> bool {
        self.inner.alternate_active
    }

    /// Whether the program wants mouse events, in which case the frontend
    /// should send them along rather than scrolling with them.
    pub fn reports_mouse(&self) -> bool {
        self.inner.modes.mouse != MouseMode::None
    }

    /// The bytes a key press should send to the program, or `None` for a
    /// key that sends nothing.
    pub fn encode_key(&self, key: Key, modifiers: Modifiers) -> Option<Vec<u8>> {
        keys::encode_key(key, modifiers, self.inner.modes.application_cursor_keys)
    }

    /// The bytes a mouse event should send to the program, or `None` when
    /// the program doesn't want to hear about it.
    pub fn encode_mouse(&self, event: MouseEvent) -> Option<Vec<u8>> {
        keys::encode_mouse(event, self.inner.modes.mouse, self.inner.modes.mouse_sgr)
    }

    /// The bytes pasted text should send to the program: wrapped in
    /// bracketed paste markers if the program asked for them, and with
    /// line breaks turned into carriage returns, as the Enter key sends.
    pub fn encode_paste(&self, text: &str) -> Vec<u8> {
        keys::encode_paste(text, self.inner.modes.bracketed_paste)
    }

    /// The bytes that tell the program the terminal gained or lost
    /// focus, if it asked to know.
    pub fn encode_focus(&self, gained: bool) -> Option<Vec<u8>> {
        self.inner
            .modes
            .focus_events
            .then(|| (if gained { b"\x1b[I" } else { b"\x1b[O" }).to_vec())
    }
}

/// Tab stops every [`TAB_WIDTH`] columns.
fn default_tabs(cols: usize) -> Vec<bool> {
    (0..cols).map(|col| col % TAB_WIDTH == 0).collect()
}

impl Inner {
    fn grid(&self) -> &Grid {
        if self.alternate_active {
            &self.alternate
        } else {
            &self.primary
        }
    }

    fn grid_mut(&mut self) -> &mut Grid {
        if self.alternate_active {
            &mut self.alternate
        } else {
            &mut self.primary
        }
    }

    /// A blank cell as an erase leaves it: in the pen's background color
    /// but with no other attributes.
    fn blank_style(&self) -> Style {
        Style {
            bg: self.pen.bg,
            ..Style::default()
        }
    }

    fn cell(&self, row: usize, col: usize) -> &Cell {
        &self.grid().row(row).cells[col]
    }

    fn cell_mut(&mut self, row: usize, col: usize) -> &mut Cell {
        &mut self.grid_mut().row_mut(row).cells[col]
    }

    // ----- Scrollback -----------------------------------------------------

    fn push_scrollback(&mut self, rows: Vec<Row>) {
        if self.scrollback_limit == 0 {
            return;
        }
        for mut row in rows {
            row.trim();
            self.scrollback.push_back(row);
        }
        self.trim_scrollback();
    }

    fn trim_scrollback(&mut self) {
        while self.scrollback.len() > self.scrollback_limit {
            self.scrollback.pop_front();
        }
    }

    // ----- Scrolling ------------------------------------------------------

    /// Scroll the region up, keeping what scrolls off the top of the
    /// primary screen when the region is the whole screen.
    fn scroll_up(&mut self, count: usize) {
        let style = self.blank_style();
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let whole = top == 0 && bottom == self.rows - 1;
        let evicted = self.grid_mut().scroll_up(top, bottom, count, style);
        if whole && !self.alternate_active {
            self.push_scrollback(evicted);
        }
    }

    fn scroll_down(&mut self, count: usize) {
        let style = self.blank_style();
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        self.grid_mut().scroll_down(top, bottom, count, style);
    }

    /// Move down a line, scrolling at the bottom of the region.
    fn linefeed(&mut self) {
        self.cursor.pending_wrap = false;
        if self.cursor.row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor.row + 1 < self.rows {
            self.cursor.row += 1;
        }
    }

    /// Move up a line, scrolling at the top of the region.
    fn reverse_index(&mut self) {
        self.cursor.pending_wrap = false;
        if self.cursor.row == self.scroll_top {
            self.scroll_down(1);
        } else if self.cursor.row > 0 {
            self.cursor.row -= 1;
        }
    }

    fn carriage_return(&mut self) {
        self.cursor.col = 0;
        self.cursor.pending_wrap = false;
    }

    // ----- Cursor movement ------------------------------------------------

    /// The rows the cursor may be put on by an absolute move: the scroll
    /// region in origin mode, otherwise the whole screen.
    fn row_bounds(&self) -> (usize, usize) {
        if self.modes.origin {
            (self.scroll_top, self.scroll_bottom)
        } else {
            (0, self.rows - 1)
        }
    }

    /// Put the cursor at a row and column given relative to the bounds of
    /// [`row_bounds`](Self::row_bounds).
    fn move_to(&mut self, row: usize, col: usize) {
        let (top, bottom) = self.row_bounds();
        self.cursor.row = (top + row).min(bottom);
        self.cursor.col = col.min(self.cols - 1);
        self.cursor.pending_wrap = false;
    }

    fn move_up(&mut self, count: usize) {
        // Movement stops at the top of the region only from inside it.
        let top = if self.cursor.row >= self.scroll_top {
            self.scroll_top
        } else {
            0
        };
        self.cursor.row = self.cursor.row.saturating_sub(count).max(top);
        self.cursor.pending_wrap = false;
    }

    fn move_down(&mut self, count: usize) {
        let bottom = if self.cursor.row <= self.scroll_bottom {
            self.scroll_bottom
        } else {
            self.rows - 1
        };
        self.cursor.row = (self.cursor.row + count).min(bottom);
        self.cursor.pending_wrap = false;
    }

    fn move_left(&mut self, count: usize) {
        self.cursor.col = self.cursor.col.saturating_sub(count);
        self.cursor.pending_wrap = false;
    }

    fn move_right(&mut self, count: usize) {
        self.cursor.col = (self.cursor.col + count).min(self.cols - 1);
        self.cursor.pending_wrap = false;
    }

    fn next_tab(&mut self) {
        let mut col = self.cursor.col + 1;
        while col < self.cols && !self.tabs[col] {
            col += 1;
        }
        self.cursor.col = col.min(self.cols - 1);
        self.cursor.pending_wrap = false;
    }

    fn previous_tab(&mut self) {
        let mut col = self.cursor.col;
        while col > 0 {
            col -= 1;
            if self.tabs[col] {
                break;
            }
        }
        self.cursor.col = col;
        self.cursor.pending_wrap = false;
    }

    // ----- Writing --------------------------------------------------------

    /// Map a character through the active character set.
    fn map_charset(&self, c: char) -> char {
        match self.charsets[self.active_charset] {
            Charset::Ascii => c,
            Charset::DecSpecial => dec_special(c),
        }
    }

    fn print(&mut self, c: char) {
        let c = self.map_charset(c);
        self.last_printed = Some(c);
        if !c.is_ascii() && self.join_previous(c) {
            return;
        }
        let mut buf = [0; 4];
        let width = text::width(c.encode_utf8(&mut buf), 0, TAB_WIDTH);
        if width == 0 {
            // A zero-width character with nothing before it to join: it
            // has no cell of its own.
            return;
        }
        if self.cursor.pending_wrap {
            let row = self.cursor.row;
            self.grid_mut().row_mut(row).wrapped = true;
            self.carriage_return();
            self.linefeed();
        }
        if width == 2 && self.cursor.col + 1 >= self.cols {
            // A wide character that doesn't fit on the line goes on the
            // next one, leaving a blank behind.
            if !self.modes.autowrap {
                return;
            }
            let blank = self.blank_style();
            let (row, col) = (self.cursor.row, self.cursor.col);
            self.blank_range(row, col, col + 1, blank);
            self.grid_mut().row_mut(row).wrapped = true;
            self.carriage_return();
            self.linefeed();
        }
        let (row, col) = (self.cursor.row, self.cursor.col);
        if self.modes.insert {
            let blank = Cell::blank(self.blank_style());
            let cells = &mut self.grid_mut().row_mut(row).cells;
            for _ in 0..width {
                cells.insert(col, blank.clone());
                cells.pop();
            }
            self.grid_mut().row_mut(row).normalize();
        }
        let blank = self.blank_style();
        self.blank_range(row, col, col + width, blank);
        let pen = self.pen;
        let cell = self.cell_mut(row, col);
        cell.text = CompactString::from(&*c.encode_utf8(&mut buf));
        cell.style = pen;
        cell.wide = width == 2;
        if width == 2 {
            *self.cell_mut(row, col + 1) = Cell::spacer(pen);
        }
        self.advance(width);
    }

    /// Move the cursor on after writing `width` columns.
    fn advance(&mut self, width: usize) {
        if self.cursor.col + width >= self.cols {
            self.cursor.col = self.cols - 1;
            self.cursor.pending_wrap = self.modes.autowrap;
        } else {
            self.cursor.col += width;
        }
    }

    /// Attach a character to the cell before the cursor when the two make
    /// one grapheme cluster (a combining mark on its base, an emoji
    /// joined to another). Returns whether it was attached.
    fn join_previous(&mut self, c: char) -> bool {
        let row = self.cursor.row;
        let mut col = if self.cursor.pending_wrap {
            self.cursor.col
        } else if self.cursor.col == 0 {
            return false;
        } else {
            self.cursor.col - 1
        };
        if self.cell(row, col).spacer {
            if col == 0 {
                return false;
            }
            col -= 1;
        }
        let cell = self.cell(row, col);
        if cell.spacer {
            return false;
        }
        let mut joined = cell.text.clone();
        joined.push(c);
        if joined.graphemes(true).count() != 1 {
            return false;
        }
        let old_width = cell.width();
        let new_width = text::width(&joined, 0, TAB_WIDTH).max(1);
        let style = cell.style;
        if new_width == 2 && old_width == 1 {
            if col + 1 >= self.cols {
                // No room to grow: keep it narrow. The frontend clips it.
                self.cell_mut(row, col).text = joined;
                return true;
            }
            let blank = self.blank_style();
            self.blank_range(row, col + 1, col + 2, blank);
            let cell = self.cell_mut(row, col);
            cell.text = joined;
            cell.wide = true;
            *self.cell_mut(row, col + 1) = Cell::spacer(style);
            // The cursor, just past the cell, moves past its new spacer.
            if !self.cursor.pending_wrap && self.cursor.col == col + 1 {
                self.advance(1);
            }
        } else if new_width == 1 && old_width == 2 {
            let cell = self.cell_mut(row, col);
            cell.text = joined;
            cell.wide = false;
            *self.cell_mut(row, col + 1) = Cell::blank(style);
        } else {
            self.cell_mut(row, col).text = joined;
        }
        true
    }

    /// Blank the cells `start..end` of a row, taking out the other half
    /// of any wide character cut at either end.
    fn blank_range(&mut self, row: usize, start: usize, end: usize, style: Style) {
        let end = end.min(self.cols);
        if start >= end {
            return;
        }
        let cells = &mut self.grid_mut().row_mut(row).cells;
        if cells[start].spacer && start > 0 {
            cells[start - 1] = Cell::blank(cells[start - 1].style);
        }
        if cells[end - 1].wide && end < cells.len() {
            cells[end] = Cell::blank(cells[end].style);
        }
        for cell in &mut cells[start..end] {
            *cell = Cell::blank(style);
        }
    }

    // ----- Erasing and editing --------------------------------------------

    fn erase_in_display(&mut self, mode: u16) {
        let style = self.blank_style();
        let (row, col) = (self.cursor.row, self.cursor.col);
        match mode {
            0 => {
                self.blank_range(row, col, self.cols, style);
                for r in row + 1..self.rows {
                    *self.grid_mut().row_mut(r) = Row::blank(self.cols, style);
                }
            }
            1 => {
                for r in 0..row {
                    *self.grid_mut().row_mut(r) = Row::blank(self.cols, style);
                }
                self.blank_range(row, 0, col + 1, style);
            }
            2 => self.grid_mut().clear(style),
            3 => self.scrollback.clear(),
            _ => {}
        }
    }

    fn erase_in_line(&mut self, mode: u16) {
        let style = self.blank_style();
        let (row, col) = (self.cursor.row, self.cursor.col);
        match mode {
            0 => self.blank_range(row, col, self.cols, style),
            1 => self.blank_range(row, 0, col + 1, style),
            2 => self.blank_range(row, 0, self.cols, style),
            _ => {}
        }
    }

    /// Insert blank lines at the cursor, pushing the rest of the region
    /// down. Does nothing outside the region.
    fn insert_lines(&mut self, count: usize) {
        let row = self.cursor.row;
        if row < self.scroll_top || row > self.scroll_bottom {
            return;
        }
        let style = self.blank_style();
        let bottom = self.scroll_bottom;
        self.grid_mut().scroll_down(row, bottom, count, style);
        self.cursor.col = 0;
        self.cursor.pending_wrap = false;
    }

    /// Delete lines at the cursor, pulling the rest of the region up.
    fn delete_lines(&mut self, count: usize) {
        let row = self.cursor.row;
        if row < self.scroll_top || row > self.scroll_bottom {
            return;
        }
        let style = self.blank_style();
        let bottom = self.scroll_bottom;
        self.grid_mut().scroll_up(row, bottom, count, style);
        self.cursor.col = 0;
        self.cursor.pending_wrap = false;
    }

    fn insert_chars(&mut self, count: usize) {
        let (row, col) = (self.cursor.row, self.cursor.col);
        let count = count.min(self.cols - col);
        let blank = Cell::blank(self.blank_style());
        let cells = &mut self.grid_mut().row_mut(row).cells;
        for _ in 0..count {
            cells.insert(col, blank.clone());
            cells.pop();
        }
        self.grid_mut().row_mut(row).normalize();
        self.cursor.pending_wrap = false;
    }

    fn delete_chars(&mut self, count: usize) {
        let (row, col) = (self.cursor.row, self.cursor.col);
        let count = count.min(self.cols - col);
        let blank = Cell::blank(self.blank_style());
        let cells = &mut self.grid_mut().row_mut(row).cells;
        cells.drain(col..col + count);
        cells.extend(std::iter::repeat_n(blank, count));
        self.grid_mut().row_mut(row).normalize();
        self.cursor.pending_wrap = false;
    }

    fn erase_chars(&mut self, count: usize) {
        let style = self.blank_style();
        let (row, col) = (self.cursor.row, self.cursor.col);
        self.blank_range(row, col, col + count.max(1), style);
        self.cursor.pending_wrap = false;
    }

    // ----- Screens and saved cursors --------------------------------------

    fn save_cursor(&mut self) {
        let saved = SavedCursor {
            cursor: self.cursor,
            pen: self.pen,
            origin: self.modes.origin,
            charsets: self.charsets,
            active_charset: self.active_charset,
        };
        if self.alternate_active {
            self.saved_alternate = Some(saved);
        } else {
            self.saved_primary = Some(saved);
        }
    }

    fn restore_cursor(&mut self) {
        let saved = if self.alternate_active {
            self.saved_alternate
        } else {
            self.saved_primary
        };
        match saved {
            Some(saved) => {
                self.cursor = saved.cursor;
                self.cursor.row = self.cursor.row.min(self.rows - 1);
                self.cursor.col = self.cursor.col.min(self.cols - 1);
                self.cursor.pending_wrap = false;
                self.pen = saved.pen;
                self.modes.origin = saved.origin;
                self.charsets = saved.charsets;
                self.active_charset = saved.active_charset;
            }
            None => {
                self.cursor = Cursor::default();
                self.pen = Style::default();
                self.modes.origin = false;
            }
        }
    }

    /// Switch to the alternate screen, cleared, or back to the primary.
    /// Switching resets the scroll region, which a full-screen program
    /// sets up for itself and a shell doesn't expect to find changed.
    fn set_alternate_screen(&mut self, on: bool) {
        if on == self.alternate_active {
            return;
        }
        self.alternate_active = on;
        if on {
            let style = self.blank_style();
            self.alternate.clear(style);
        }
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.cursor.pending_wrap = false;
    }

    /// Reset everything but the screen size, as RIS does.
    fn reset(&mut self) {
        self.primary.clear(Style::default());
        self.alternate.clear(Style::default());
        self.alternate_active = false;
        self.cursor = Cursor::default();
        self.saved_primary = None;
        self.saved_alternate = None;
        self.pen = Style::default();
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.tabs = default_tabs(self.cols);
        self.modes = Modes::default();
        self.cursor_style = CursorStyle::default();
        self.charsets = [Charset::Ascii; 2];
        self.active_charset = 0;
        self.last_printed = None;
    }

    /// DECSTR: the soft reset, which leaves the screen alone.
    fn soft_reset(&mut self) {
        self.pen = Style::default();
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.modes.origin = false;
        self.modes.autowrap = true;
        self.modes.cursor_visible = true;
        self.modes.insert = false;
        self.modes.application_cursor_keys = false;
        self.modes.application_keypad = false;
        self.charsets = [Charset::Ascii; 2];
        self.active_charset = 0;
        self.cursor.pending_wrap = false;
    }

    // ----- Resizing -------------------------------------------------------

    fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        let old_rows = self.rows;
        let cursor_row = self.cursor.row;
        // The active screen keeps its cursor on screen; the other keeps
        // its bottom rows, where a shell's prompt was.
        let (primary_cursor, alternate_cursor) = if self.alternate_active {
            (None, Some(cursor_row))
        } else {
            (Some(cursor_row), None)
        };
        let shift = resize_grid(
            &mut self.primary,
            cols,
            rows,
            primary_cursor,
            Some(&mut self.scrollback),
        );
        self.trim_scrollback();
        let alternate_shift = resize_grid(&mut self.alternate, cols, rows, alternate_cursor, None);
        let shift = if self.alternate_active {
            alternate_shift
        } else {
            shift
        };
        self.cols = cols;
        self.rows = rows;
        self.cursor.row = cursor_row.saturating_add_signed(shift).min(rows - 1);
        self.cursor.col = self.cursor.col.min(cols - 1);
        self.cursor.pending_wrap = false;
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
        let mut tabs = default_tabs(cols);
        for (col, stop) in self
            .tabs
            .iter()
            .enumerate()
            .take(cols.min(old_rows.max(cols)))
        {
            if col < cols {
                tabs[col] = *stop;
            }
        }
        self.tabs = tabs;
        for saved in [&mut self.saved_primary, &mut self.saved_alternate]
            .into_iter()
            .flatten()
        {
            saved.cursor.row = saved.cursor.row.min(rows - 1);
            saved.cursor.col = saved.cursor.col.min(cols - 1);
        }
        self.generation += 1;
    }

    // ----- Responses ------------------------------------------------------

    fn respond(&mut self, bytes: &[u8]) {
        self.responses.extend_from_slice(bytes);
    }

    fn respond_string(&mut self, text: String) {
        self.responses.extend_from_slice(text.as_bytes());
    }

    /// The reply to DECRQM for a private mode: whether it's set, reset,
    /// or not something this terminal has.
    fn private_mode_state(&self, mode: u16) -> u8 {
        let modes = &self.modes;
        let set = |on: bool| if on { 1 } else { 2 };
        match mode {
            1 => set(modes.application_cursor_keys),
            6 => set(modes.origin),
            7 => set(modes.autowrap),
            25 => set(modes.cursor_visible),
            9 => set(modes.mouse == MouseMode::X10),
            1000 => set(modes.mouse == MouseMode::Normal),
            1002 => set(modes.mouse == MouseMode::Button),
            1003 => set(modes.mouse == MouseMode::Any),
            1004 => set(modes.focus_events),
            1006 => set(modes.mouse_sgr),
            47 | 1047 | 1049 => set(self.alternate_active),
            2004 => set(modes.bracketed_paste),
            // Grapheme clustering is how this terminal always works.
            2027 => 3,
            _ => 0,
        }
    }

    // ----- Modes ----------------------------------------------------------

    fn set_private_mode(&mut self, mode: u16, on: bool) {
        match mode {
            1 => self.modes.application_cursor_keys = on,
            6 => {
                self.modes.origin = on;
                self.move_to(0, 0);
            }
            7 => self.modes.autowrap = on,
            9 => self.set_mouse_mode(MouseMode::X10, on),
            25 => self.modes.cursor_visible = on,
            47 | 1047 => self.set_alternate_screen(on),
            1000 => self.set_mouse_mode(MouseMode::Normal, on),
            1002 => self.set_mouse_mode(MouseMode::Button, on),
            1003 => self.set_mouse_mode(MouseMode::Any, on),
            1004 => self.modes.focus_events = on,
            1006 => self.modes.mouse_sgr = on,
            1048 => {
                if on {
                    self.save_cursor();
                } else {
                    self.restore_cursor();
                }
            }
            1049 => {
                if on {
                    self.save_cursor();
                    self.set_alternate_screen(true);
                } else {
                    self.set_alternate_screen(false);
                    self.restore_cursor();
                }
            }
            2004 => self.modes.bracketed_paste = on,
            2026 => self.modes.synchronized_output = on,
            _ => {}
        }
    }

    fn set_mouse_mode(&mut self, mode: MouseMode, on: bool) {
        if on {
            self.modes.mouse = mode;
        } else if self.modes.mouse == mode {
            self.modes.mouse = MouseMode::None;
        }
    }

    fn set_ansi_mode(&mut self, mode: u16, on: bool) {
        match mode {
            4 => self.modes.insert = on,
            20 => self.modes.newline = on,
            _ => {}
        }
    }

    // ----- SGR ------------------------------------------------------------

    fn select_graphic_rendition(&mut self, params: &Params) {
        let params: Vec<&[u16]> = params.iter().collect();
        if params.is_empty() {
            self.pen = Style::default();
            return;
        }
        let mut i = 0;
        while i < params.len() {
            let param = params[i];
            let code = param.first().copied().unwrap_or(0);
            match code {
                0 => self.pen = Style::default(),
                1 => self.pen.bold = true,
                2 => self.pen.dim = true,
                3 => self.pen.italic = true,
                4 => {
                    // 4:n picks a style; plain 4 is a single underline.
                    self.pen.underline = match param.get(1) {
                        Some(0) => Underline::None,
                        Some(2) => Underline::Double,
                        Some(3) => Underline::Curly,
                        Some(4) => Underline::Dotted,
                        Some(5) => Underline::Dashed,
                        _ => Underline::Single,
                    };
                }
                5 | 6 => self.pen.blink = true,
                7 => self.pen.inverse = true,
                8 => self.pen.hidden = true,
                9 => self.pen.strikethrough = true,
                21 => self.pen.underline = Underline::Double,
                22 => {
                    self.pen.bold = false;
                    self.pen.dim = false;
                }
                23 => self.pen.italic = false,
                24 => self.pen.underline = Underline::None,
                25 => self.pen.blink = false,
                27 => self.pen.inverse = false,
                28 => self.pen.hidden = false,
                29 => self.pen.strikethrough = false,
                30..=37 => self.pen.fg = Color::Indexed((code - 30) as u8),
                39 => self.pen.fg = Color::Default,
                40..=47 => self.pen.bg = Color::Indexed((code - 40) as u8),
                49 => self.pen.bg = Color::Default,
                59 => self.pen.underline_color = Color::Default,
                90..=97 => self.pen.fg = Color::Indexed((code - 90 + 8) as u8),
                100..=107 => self.pen.bg = Color::Indexed((code - 100 + 8) as u8),
                38 | 48 | 58 => {
                    // Either 38:2::r:g:b / 38:5:n as one parameter with
                    // subparameters, or 38;2;r;g;b / 38;5;n spread over
                    // the following parameters.
                    let (color, used) = if param.len() > 1 {
                        (extended_color(&param[1..]), 0)
                    } else {
                        // Only the parameters this color needs, since more
                        // SGR codes can follow in the same sequence (e.g.
                        // 38;2;r;g;b;48;2;r;g;b as crossterm emits it).
                        let rest: Vec<u16> = params[i + 1..]
                            .iter()
                            .map(|p| p.first().copied().unwrap_or(0))
                            .collect();
                        let used = match rest.first() {
                            Some(2) => 4,
                            Some(5) => 2,
                            _ => 0,
                        };
                        match extended_color(&rest[..used.min(rest.len())]) {
                            Some(color) => (Some(color), used),
                            None => (None, 0),
                        }
                    };
                    if let Some(color) = color {
                        match code {
                            38 => self.pen.fg = color,
                            48 => self.pen.bg = color,
                            _ => self.pen.underline_color = color,
                        }
                    }
                    i += used;
                }
                _ => {}
            }
            i += 1;
        }
    }

    // ----- OSC ------------------------------------------------------------

    fn osc(&mut self, params: &[&[u8]], bell_terminated: bool) {
        let Some(first) = params.first() else {
            return;
        };
        let Ok(code) = std::str::from_utf8(first).map(str::parse::<u16>) else {
            return;
        };
        let Ok(code) = code else {
            return;
        };
        let text = |index: usize| {
            params
                .get(index)
                .map(|p| String::from_utf8_lossy(p).into_owned())
                .unwrap_or_default()
        };
        let terminator: &[u8] = if bell_terminated { b"\x07" } else { b"\x1b\\" };
        match code {
            0 | 2 => {
                self.title = text(1);
                self.events.push(Event::Title(self.title.clone()));
            }
            10 | 11 => {
                if params.get(1) == Some(&&b"?"[..]) {
                    let (r, g, b) = if code == 10 {
                        self.default_fg
                    } else {
                        self.default_bg
                    };
                    let mut reply = format!(
                        "\x1b]{code};rgb:{:02x}{:02x}/{:02x}{:02x}/{:02x}{:02x}",
                        r, r, g, g, b, b
                    )
                    .into_bytes();
                    reply.extend_from_slice(terminator);
                    self.respond(&reply);
                }
            }
            52 => {
                // OSC 52 ; selection ; base64 text. A "?" asks for the
                // clipboard, which the program doesn't get.
                if let Some(data) = params.get(2)
                    && *data != b"?"
                    && let Some(bytes) = base64_decode(data)
                {
                    self.events.push(Event::Clipboard(
                        String::from_utf8_lossy(&bytes).into_owned(),
                    ));
                }
            }
            _ => {}
        }
    }

    // ----- CSI ------------------------------------------------------------

    fn csi(&mut self, params: &Params, intermediates: &[u8], action: char) {
        let first = |default: u16| -> usize {
            params
                .iter()
                .next()
                .and_then(|p| p.first().copied())
                .filter(|&v| v != 0)
                .unwrap_or(default) as usize
        };
        let nth = |index: usize, default: u16| -> usize {
            params
                .iter()
                .nth(index)
                .and_then(|p| p.first().copied())
                .filter(|&v| v != 0)
                .unwrap_or(default) as usize
        };
        match (intermediates, action) {
            (b"", '@') => self.insert_chars(first(1)),
            (b"", 'A') => self.move_up(first(1)),
            (b"", 'B') | (b"", 'e') => self.move_down(first(1)),
            (b"", 'C') | (b"", 'a') => self.move_right(first(1)),
            (b"", 'D') => self.move_left(first(1)),
            (b"", 'E') => {
                self.move_down(first(1));
                self.carriage_return();
            }
            (b"", 'F') => {
                self.move_up(first(1));
                self.carriage_return();
            }
            (b"", 'G') | (b"", '`') => {
                self.cursor.col = (first(1) - 1).min(self.cols - 1);
                self.cursor.pending_wrap = false;
            }
            (b"", 'H') | (b"", 'f') => self.move_to(first(1) - 1, nth(1, 1) - 1),
            (b"", 'I') => {
                for _ in 0..first(1) {
                    self.next_tab();
                }
            }
            (b"", 'J') => self.erase_in_display(first(0) as u16),
            (b"", 'K') => self.erase_in_line(first(0) as u16),
            (b"", 'L') => self.insert_lines(first(1)),
            (b"", 'M') => self.delete_lines(first(1)),
            (b"", 'P') => self.delete_chars(first(1)),
            (b"", 'S') => self.scroll_up(first(1)),
            (b"", 'T') => self.scroll_down(first(1)),
            (b"", 'X') => self.erase_chars(first(1)),
            (b"", 'Z') => {
                for _ in 0..first(1) {
                    self.previous_tab();
                }
            }
            (b"", 'b') => {
                if let Some(c) = self.last_printed {
                    for _ in 0..first(1) {
                        self.print(c);
                    }
                }
            }
            (b"", 'c') => self.respond(b"\x1b[?62;22c"),
            (b">", 'c') => self.respond(b"\x1b[>1;10;0c"),
            (b"", 'd') => {
                let row = first(1) - 1;
                let col = self.cursor.col;
                self.move_to(row, col);
            }
            (b"", 'g') => match first(0) {
                0 => {
                    let col = self.cursor.col;
                    self.tabs[col] = false;
                }
                3 => self.tabs.iter_mut().for_each(|stop| *stop = false),
                _ => {}
            },
            (b"", 'h') | (b"", 'l') => {
                for param in params.iter() {
                    if let Some(&mode) = param.first() {
                        self.set_ansi_mode(mode, action == 'h');
                    }
                }
            }
            (b"?", 'h') | (b"?", 'l') => {
                for param in params.iter() {
                    if let Some(&mode) = param.first() {
                        self.set_private_mode(mode, action == 'h');
                    }
                }
            }
            (b"", 'm') => self.select_graphic_rendition(params),
            (b"", 'n') => match first(0) {
                5 => self.respond(b"\x1b[0n"),
                6 => {
                    let (top, _) = self.row_bounds();
                    let reply = format!(
                        "\x1b[{};{}R",
                        self.cursor.row - top.min(self.cursor.row) + 1,
                        self.cursor.col + 1
                    );
                    self.respond_string(reply);
                }
                _ => {}
            },
            (b"?", 'n') => {
                if first(0) == 6 {
                    let (top, _) = self.row_bounds();
                    let reply = format!(
                        "\x1b[?{};{}R",
                        self.cursor.row - top.min(self.cursor.row) + 1,
                        self.cursor.col + 1
                    );
                    self.respond_string(reply);
                }
            }
            (b"!", 'p') => self.soft_reset(),
            (b"?$", 'p') => {
                let mode = first(0) as u16;
                let state = self.private_mode_state(mode);
                self.respond_string(format!("\x1b[?{mode};{state}$y"));
            }
            (b"$", 'p') => {
                let mode = first(0) as u16;
                let state = match mode {
                    4 => u8::from(!self.modes.insert) + 1,
                    20 => u8::from(!self.modes.newline) + 1,
                    _ => 0,
                };
                self.respond_string(format!("\x1b[{mode};{state}$y"));
            }
            (b" ", 'q') => {
                self.cursor_style = match first(0) {
                    0..=2 => CursorStyle::Block,
                    3 | 4 => CursorStyle::Underline,
                    _ => CursorStyle::Bar,
                };
            }
            (b">", 'q') => self.respond(b"\x1bP>|ninjaedit\x1b\\"),
            (b"", 'r') => {
                let top = first(1) - 1;
                let bottom = nth(1, self.rows as u16).min(self.rows) - 1;
                if top < bottom {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                    self.move_to(0, 0);
                }
            }
            (b"", 's') => self.save_cursor(),
            (b"", 't') => {
                if first(0) == 18 {
                    self.respond_string(format!("\x1b[8;{};{}t", self.rows, self.cols));
                }
            }
            (b"", 'u') => self.restore_cursor(),
            _ => {}
        }
    }

    // ----- ESC ------------------------------------------------------------

    fn esc(&mut self, intermediates: &[u8], byte: u8) {
        match (intermediates, byte) {
            (b"", b'7') => self.save_cursor(),
            (b"", b'8') => self.restore_cursor(),
            (b"", b'D') => self.linefeed(),
            (b"", b'E') => {
                self.linefeed();
                self.carriage_return();
            }
            (b"", b'H') => {
                let col = self.cursor.col;
                self.tabs[col] = true;
            }
            (b"", b'M') => self.reverse_index(),
            (b"", b'c') => self.reset(),
            (b"", b'=') => self.modes.application_keypad = true,
            (b"", b'>') => self.modes.application_keypad = false,
            (b"(", set) | (b")", set) => {
                let slot = usize::from(intermediates == b")");
                self.charsets[slot] = if set == b'0' {
                    Charset::DecSpecial
                } else {
                    Charset::Ascii
                };
            }
            (b"#", b'8') => {
                // DECALN: fill the screen with E, for alignment tests.
                let mut row = Row::blank(self.cols, Style::default());
                for cell in &mut row.cells {
                    cell.text = "E".into();
                }
                for r in 0..self.rows {
                    *self.grid_mut().row_mut(r) = row.clone();
                }
                self.cursor = Cursor::default();
            }
            _ => {}
        }
    }
}

impl Perform for Inner {
    fn print(&mut self, c: char) {
        Inner::print(self, c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => self.events.push(Event::Bell),
            0x08 => self.move_left(1),
            0x09 => self.next_tab(),
            0x0a..=0x0c => {
                self.linefeed();
                if self.modes.newline {
                    self.carriage_return();
                }
            }
            0x0d => self.carriage_return(),
            0x0e => self.active_charset = 1,
            0x0f => self.active_charset = 0,
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if !ignore {
            self.csi(params, intermediates, action);
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        if !ignore {
            self.esc(intermediates, byte);
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        self.osc(params, bell_terminated);
    }
}

/// Resize a grid, deciding which rows to drop or where to get new ones.
/// With a cursor row, rows below the cursor go first when shrinking and
/// what's left comes off the top; without one, rows come off the bottom.
/// Rows taken off the top go to the scrollback if there is one, and rows
/// added come back from it. Returns how far the rows moved, for the
/// cursor to follow.
fn resize_grid(
    grid: &mut Grid,
    cols: usize,
    rows: usize,
    cursor_row: Option<usize>,
    mut scrollback: Option<&mut VecDeque<Row>>,
) -> isize {
    let old_rows = grid.rows();
    let mut shift = 0isize;
    if rows < old_rows {
        let excess = old_rows - rows;
        let from_bottom = match cursor_row {
            Some(row) => excess.min(old_rows - 1 - row),
            None => excess,
        };
        let from_top = excess - from_bottom;
        grid.remove_bottom(from_bottom);
        let evicted = grid.remove_top(from_top);
        shift -= from_top as isize;
        if let Some(scrollback) = scrollback.as_deref_mut() {
            for mut row in evicted {
                row.trim();
                scrollback.push_back(row);
            }
        }
    } else if rows > old_rows {
        let missing = rows - old_rows;
        let restored = match scrollback {
            Some(scrollback) if cursor_row.is_some() => {
                let count = missing.min(scrollback.len());
                let rows: Vec<Row> = scrollback.drain(scrollback.len() - count..).collect();
                grid.insert_top(rows);
                count
            }
            _ => 0,
        };
        shift += restored as isize;
        grid.pad_bottom(missing - restored);
    }
    grid.set_cols(cols);
    shift
}

/// A color from the parameters after SGR 38, 48, or 58: `5;n` for a
/// palette entry, `2;r;g;b` for RGB (`2;;r;g;b` with a color space id
/// in the colon form).
fn extended_color(params: &[u16]) -> Option<Color> {
    match params.first()? {
        5 => Some(Color::Indexed(u8::try_from(*params.get(1)?).ok()?)),
        2 => {
            let channel = |v: u16| u8::try_from(v).ok();
            match params.len() {
                4 => Some(Color::Rgb(
                    channel(params[1])?,
                    channel(params[2])?,
                    channel(params[3])?,
                )),
                5.. => Some(Color::Rgb(
                    channel(params[2])?,
                    channel(params[3])?,
                    channel(params[4])?,
                )),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The DEC special graphics character set: line-drawing characters in
/// place of the lowercase letters and some punctuation.
fn dec_special(c: char) -> char {
    match c {
        '`' => '◆',
        'a' => '▒',
        'f' => '°',
        'g' => '±',
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'o' => '⎺',
        'p' => '⎻',
        'q' => '─',
        'r' => '⎼',
        's' => '⎽',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        _ => c,
    }
}

/// Decode standard base64, tolerating missing padding. `None` for other
/// characters.
fn base64_decode(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for &byte in data {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b'\n' | b'\r' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(cols: usize, rows: usize) -> Terminal {
        Terminal::new(cols, rows)
    }

    fn feed(t: &mut Terminal, s: &str) {
        t.process(s.as_bytes());
    }

    fn screen(t: &Terminal) -> Vec<String> {
        (0..t.size().1).map(|r| t.row_text(r)).collect()
    }

    #[test]
    fn prints_wraps_and_scrolls() {
        let mut t = term(5, 3);
        feed(&mut t, "abcdefg\r\nhi");
        assert_eq!(screen(&t), ["abcde", "fg", "hi"]);
        assert_eq!(t.cursor_position(), (2, 2));
        assert!(t.inner.primary.row(0).wrapped);
        assert!(!t.inner.primary.row(1).wrapped);
        feed(&mut t, "\n\n");
        assert_eq!(screen(&t), ["hi", "", ""]);
        assert_eq!(t.scrollback_len(), 2);
        let rows: Vec<String> = t.rows(2).map(Row::text).collect();
        assert_eq!(rows, ["abcde", "fg", "hi"]);
        let rows: Vec<String> = t.rows(1).map(Row::text).collect();
        assert_eq!(rows, ["fg", "hi", ""]);
        // Scrolling further back than there is stops at the start.
        let rows: Vec<String> = t.rows(50).map(Row::text).collect();
        assert_eq!(rows, ["abcde", "fg", "hi"]);
    }

    #[test]
    fn pending_wrap_holds_the_cursor_on_the_last_column() {
        let mut t = term(3, 2);
        feed(&mut t, "abc");
        assert_eq!(t.cursor_position(), (2, 0));
        // A carriage return cancels the wrap: the next text overwrites.
        feed(&mut t, "\rX");
        assert_eq!(screen(&t), ["Xbc", ""]);
        feed(&mut t, "yz");
        assert_eq!(t.cursor_position(), (2, 0));
        // Backspace cancels the wrap and moves left from the last column.
        feed(&mut t, "\x08Q");
        assert_eq!(screen(&t), ["XQz", ""]);
        assert_eq!(t.cursor_position(), (2, 0));
        // Without autowrap the last column is overwritten instead.
        feed(&mut t, "\x1b[?7lRS");
        assert_eq!(screen(&t), ["XQS", ""]);
        assert_eq!(t.cursor_position(), (2, 0));
    }

    #[test]
    fn cursor_movement_and_erasing() {
        let mut t = term(10, 4);
        feed(&mut t, "line one\r\nline two\r\nline three\r\nfour");
        feed(&mut t, "\x1b[2;3H");
        assert_eq!(t.cursor_position(), (2, 1));
        feed(&mut t, "\x1b[K");
        assert_eq!(t.row_text(1), "li");
        // Erasing to the cursor includes the cell under it.
        feed(&mut t, "\x1b[1;2H\x1b[1J");
        assert_eq!(t.row_text(0), "  ne one");
        feed(&mut t, "\x1b[3;5H\x1b[J");
        assert_eq!(screen(&t), ["  ne one", "li", "line", ""]);
        feed(&mut t, "\x1b[2J");
        assert_eq!(screen(&t), ["", "", "", ""]);
        feed(&mut t, "\x1b[Hab\x1b[2@");
        assert_eq!(t.row_text(0), "ab");
        feed(&mut t, "\x1b[Hab\x1b[1G\x1b[2@");
        assert_eq!(t.row_text(0), "  ab");
        feed(&mut t, "\x1b[1P");
        assert_eq!(t.row_text(0), " ab");
        feed(&mut t, "\x1b[2X");
        assert_eq!(t.row_text(0), "  b");
        feed(&mut t, "\x1b[3;1Hx\x1b[2A");
        assert_eq!(t.cursor_position(), (1, 0));
        feed(&mut t, "\x1b[5B");
        assert_eq!(t.cursor_position(), (1, 3));
        feed(&mut t, "\x1b[20C\x1b[3D");
        assert_eq!(t.cursor_position(), (6, 3));
    }

    #[test]
    fn scroll_regions_and_line_insertion() {
        let mut t = term(4, 5);
        feed(&mut t, "a\r\nb\r\nc\r\nd\r\ne");
        feed(&mut t, "\x1b[2;4r");
        assert_eq!(t.cursor_position(), (0, 0));
        feed(&mut t, "\x1b[4;1H\n");
        assert_eq!(screen(&t), ["a", "c", "d", "", "e"]);
        assert_eq!(t.scrollback_len(), 0, "a region scroll keeps nothing");
        feed(&mut t, "\x1b[2;1H\x1bM");
        assert_eq!(screen(&t), ["a", "", "c", "d", "e"]);
        feed(&mut t, "\x1b[3;1H\x1b[L");
        assert_eq!(screen(&t), ["a", "", "", "c", "e"]);
        feed(&mut t, "\x1b[2M");
        assert_eq!(screen(&t), ["a", "", "", "", "e"]);
        // Origin mode confines the cursor to the region.
        feed(&mut t, "\x1b[?6h\x1b[1;1Hq");
        assert_eq!(screen(&t), ["a", "q", "", "", "e"]);
        feed(&mut t, "\x1b[6n");
        assert_eq!(t.take_responses(), b"\x1b[1;2R");
        feed(&mut t, "\x1b[r\x1b[6n");
        assert_eq!(t.take_responses(), b"\x1b[1;1R");
    }

    #[test]
    fn colors_and_attributes() {
        let mut t = term(10, 1);
        feed(
            &mut t,
            "\x1b[1;31;44ma\x1b[0;38;5;200;48;2;1;2;3mb\x1b[38:2::9:8:7;4:3mc\x1b[mD",
        );
        let cells = &t.inner.primary.row(0).cells;
        assert_eq!(cells[0].style.fg, Color::Indexed(1));
        assert_eq!(cells[0].style.bg, Color::Indexed(4));
        assert!(cells[0].style.bold);
        assert_eq!(cells[1].style.fg, Color::Indexed(200));
        assert_eq!(cells[1].style.bg, Color::Rgb(1, 2, 3));
        assert!(!cells[1].style.bold);
        assert_eq!(cells[2].style.fg, Color::Rgb(9, 8, 7));
        assert_eq!(cells[2].style.underline, Underline::Curly);
        assert_eq!(cells[3].style, Style::default());
        feed(
            &mut t,
            "\x1b[92;107;7;9;2;3;5;8mx\x1b[22;23;24;25;27;28;29my",
        );
        let cells = &t.inner.primary.row(0).cells;
        let x = cells[4].style;
        assert_eq!(x.fg, Color::Indexed(10));
        assert_eq!(x.bg, Color::Indexed(15));
        assert!(x.inverse && x.strikethrough && x.dim && x.italic && x.blink && x.hidden);
        let y = cells[5].style;
        assert!(!(y.inverse || y.strikethrough || y.dim || y.italic || y.blink || y.hidden));
        assert_eq!(y.fg, Color::Indexed(10), "colors survive attribute resets");
        // Erased cells take the background color but nothing else.
        feed(&mut t, "\x1b[1;41m\x1b[K");
        let cells = &t.inner.primary.row(0).cells;
        assert_eq!(cells[7].style.bg, Color::Indexed(1));
        assert!(!cells[7].style.bold);
    }

    #[test]
    fn combined_extended_colors() {
        // crossterm's SetColors joins foreground and background into one
        // sequence, so the RGB parameters must not run into the next code.
        let mut t = term(10, 1);
        feed(
            &mut t,
            "\x1b[38;2;10;20;30;48;2;40;50;60ma\x1b[38;2;1;2;3;1mb\x1b[38;5;7;48;5;9;4mc\x1b[38;2;5;6;7;49md",
        );
        let cells = &t.inner.primary.row(0).cells;
        assert_eq!(cells[0].style.fg, Color::Rgb(10, 20, 30));
        assert_eq!(cells[0].style.bg, Color::Rgb(40, 50, 60));
        assert_eq!(cells[1].style.fg, Color::Rgb(1, 2, 3));
        assert!(cells[1].style.bold);
        assert_eq!(cells[2].style.fg, Color::Indexed(7));
        assert_eq!(cells[2].style.bg, Color::Indexed(9));
        assert_eq!(cells[2].style.underline, Underline::Single);
        assert_eq!(cells[3].style.fg, Color::Rgb(5, 6, 7));
        assert_eq!(cells[3].style.bg, Color::Default);
        // A truncated color is ignored without eating the next code.
        feed(&mut t, "\x1b[0m\x1b[38;2;1;2m\x1b[38;2;1;2;3;48;5m");
        assert_eq!(t.inner.pen.fg, Color::Rgb(1, 2, 3));
        assert_eq!(t.inner.pen.bg, Color::Default);
    }

    #[test]
    fn wide_characters_take_two_cells() {
        let mut t = term(5, 2);
        feed(&mut t, "a한b");
        let cells = &t.inner.primary.row(0).cells;
        assert_eq!(cells[1].text, "한");
        assert!(cells[1].wide);
        assert!(cells[2].spacer);
        assert_eq!(cells[3].text, "b");
        assert_eq!(t.row_text(0), "a한b");
        // Overwriting either half blanks the other.
        feed(&mut t, "\x1b[1;3Hx");
        let cells = &t.inner.primary.row(0).cells;
        assert_eq!(cells[1].text, " ");
        assert!(!cells[1].wide);
        assert_eq!(cells[2].text, "x");
        assert_eq!(t.row_text(0), "a xb");
        // A wide character that doesn't fit wraps to the next line.
        feed(&mut t, "\x1b[1;5H한");
        assert_eq!(t.row_text(0), "a xb");
        assert_eq!(t.row_text(1), "한");
        assert_eq!(t.cursor_position(), (2, 1));
        assert!(t.inner.primary.row(0).wrapped);
    }

    #[test]
    fn combining_marks_join_the_previous_cell() {
        let mut t = term(6, 2);
        feed(&mut t, "e\u{301}x");
        let cells = &t.inner.primary.row(0).cells;
        assert_eq!(cells[0].text, "e\u{301}");
        assert_eq!(cells[1].text, "x");
        assert_eq!(t.cursor_position(), (2, 0));
        // An emoji sequence joined with ZWJ stays one wide cell.
        feed(&mut t, "\x1b[2;1H👨\u{200d}👩\u{200d}👧z");
        let cells = &t.inner.primary.row(1).cells;
        assert_eq!(cells[0].text, "👨\u{200d}👩\u{200d}👧");
        assert!(cells[0].wide);
        assert!(cells[1].spacer);
        assert_eq!(cells[2].text, "z");
        // A variation selector can make a narrow character wide, and the
        // cursor moves past the spacer that appears.
        feed(&mut t, "\x1b[1;1H\x1b[2K\u{2764}");
        assert_eq!(t.cursor_position(), (1, 0));
        feed(&mut t, "\u{fe0f}");
        let cells = &t.inner.primary.row(0).cells;
        assert!(cells[0].wide);
        assert!(cells[1].spacer);
        assert_eq!(t.cursor_position(), (2, 0));
        // Joining at the last column, with the cursor waiting to wrap.
        feed(&mut t, "\x1b[1;6He");
        assert_eq!(t.cursor_position(), (5, 0));
        feed(&mut t, "\u{301}");
        assert_eq!(t.inner.primary.row(0).cells[5].text, "e\u{301}");
        assert_eq!(t.cursor_position(), (5, 0));
        feed(&mut t, "q");
        assert_eq!(t.cursor_position(), (1, 1));
    }

    #[test]
    fn alternate_screen_saves_and_restores() {
        let mut t = term(5, 2);
        feed(&mut t, "shell\x1b[1;3H");
        feed(&mut t, "\x1b[?1049h");
        assert!(t.is_alternate_screen());
        assert_eq!(screen(&t), ["", ""]);
        feed(&mut t, "\x1b[Happ\r\n\r\n\r\n");
        assert_eq!(
            t.scrollback_len(),
            0,
            "the alternate screen has no scrollback"
        );
        feed(&mut t, "\x1b[?1049l");
        assert!(!t.is_alternate_screen());
        assert_eq!(screen(&t), ["shell", ""]);
        assert_eq!(t.cursor_position(), (2, 0));
        // Setting the mode again while already there changes nothing.
        feed(&mut t, "\x1b[?1049h\x1b[?1049hX\x1b[?1049l");
        assert_eq!(screen(&t), ["shell", ""]);
    }

    #[test]
    fn modes_and_queries() {
        let mut t = term(20, 5);
        feed(
            &mut t,
            "\x1b[?1h\x1b[?1000h\x1b[?1006h\x1b[?2004h\x1b[?1004h\x1b[?25l",
        );
        let modes = t.modes();
        assert!(modes.application_cursor_keys);
        assert_eq!(modes.mouse, MouseMode::Normal);
        assert!(modes.mouse_sgr && modes.bracketed_paste && modes.focus_events);
        assert!(!modes.cursor_visible);
        assert_eq!(t.cursor(), None);
        assert!(t.reports_mouse());
        feed(&mut t, "\x1b[?1002h\x1b[?1000l");
        assert_eq!(
            t.modes().mouse,
            MouseMode::Button,
            "resetting a mode that isn't the current one changes nothing"
        );
        feed(&mut t, "\x1b[?1002l\x1b[?25h");
        assert!(!t.reports_mouse());
        assert_eq!(t.cursor(), Some((0, 0)));
        feed(&mut t, "\x1b[c\x1b[>c\x1b[5n\x1b[18t");
        assert_eq!(
            String::from_utf8(t.take_responses()).unwrap(),
            "\x1b[?62;22c\x1b[>1;10;0c\x1b[0n\x1b[8;5;20t"
        );
        feed(
            &mut t,
            "\x1b[?2004$p\x1b[?1000$p\x1b[?2027$p\x1b[?9999$p\x1b[4$p",
        );
        assert_eq!(
            String::from_utf8(t.take_responses()).unwrap(),
            "\x1b[?2004;1$y\x1b[?1000;2$y\x1b[?2027;3$y\x1b[?9999;0$y\x1b[4;2$y"
        );
        t.set_default_colors((1, 2, 3), (0xaa, 0xbb, 0xcc));
        feed(&mut t, "\x1b]11;?\x07\x1b]10;?\x1b\\");
        assert_eq!(
            String::from_utf8(t.take_responses()).unwrap(),
            "\x1b]11;rgb:aaaa/bbbb/cccc\x07\x1b]10;rgb:0101/0202/0303\x1b\\"
        );
        feed(&mut t, "\x1b[2 q");
        assert_eq!(t.cursor_style(), CursorStyle::Block);
        feed(&mut t, "\x1b[5 q");
        assert_eq!(t.cursor_style(), CursorStyle::Bar);
    }

    #[test]
    fn events_title_bell_and_clipboard() {
        let mut t = term(10, 2);
        feed(&mut t, "\x1b]0;my title\x07\x07\x1b]52;c;aGVsbG8=\x1b\\");
        assert_eq!(t.title(), "my title");
        assert_eq!(
            t.take_events(),
            [
                Event::Title("my title".to_owned()),
                Event::Bell,
                Event::Clipboard("hello".to_owned())
            ]
        );
        assert!(t.take_events().is_empty());
        assert_eq!(base64_decode(b"aGVsbG8"), Some(b"hello".to_vec()));
        assert_eq!(base64_decode(b"aGk="), Some(b"hi".to_vec()));
        assert_eq!(base64_decode(b"a*"), None);
    }

    #[test]
    fn tabs_and_line_drawing() {
        let mut t = term(20, 1);
        // Stops at 0, 8, and 16, plus one set at column 12; forward two
        // stops from the start lands on it, and back one from the end
        // lands on 16.
        feed(
            &mut t,
            "a\tb\x1b[1;12H\x1bHc\x1b[1;1H\x1b[2Id\x1b[1;20H\x1b[Ze",
        );
        assert_eq!(t.row_text(0), "a       b  d    e");
        feed(&mut t, "\x1b[2K\x1b[1;1H\x1b(0lqk\x1b(Bx");
        assert_eq!(t.row_text(0), "┌─┐x");
        feed(&mut t, "\x1b[2K\x1b[1;1Ha\x1b[3b");
        assert_eq!(t.row_text(0), "aaaa");
    }

    #[test]
    fn insert_mode_and_newline_mode() {
        let mut t = term(6, 2);
        feed(&mut t, "abcd\x1b[1;2H\x1b[4hXY\x1b[4l");
        assert_eq!(t.row_text(0), "aXYbcd");
        feed(&mut t, "\x1b[20h\x1b[1;3H\nq");
        assert_eq!(t.row_text(1), "q");
    }

    #[test]
    fn resizing_keeps_the_bottom_of_the_screen() {
        let mut t = term(10, 4);
        feed(&mut t, "one\r\ntwo\r\nthree\r\nfour");
        assert_eq!(t.cursor_position(), (4, 3));
        t.resize(10, 2);
        assert_eq!(screen(&t), ["three", "four"]);
        assert_eq!(t.cursor_position(), (4, 1));
        assert_eq!(t.scrollback_len(), 2);
        t.resize(10, 5);
        assert_eq!(screen(&t), ["one", "two", "three", "four", ""]);
        assert_eq!(t.cursor_position(), (4, 3));
        assert_eq!(t.scrollback_len(), 0);
        // Rows below the cursor go before rows above it.
        feed(&mut t, "\x1b[2;1H");
        t.resize(3, 3);
        assert_eq!(screen(&t), ["one", "two", "thr"]);
        assert_eq!(t.cursor_position(), (0, 1));
        assert_eq!(t.scrollback_len(), 0);
        // A wide character cut by the new edge disappears.
        feed(&mut t, "\x1b[3;2H한");
        assert_eq!(t.row_text(2), "t한");
        t.resize(2, 3);
        assert_eq!(t.row_text(2), "t");
        // Same size is a no-op.
        let generation = t.generation();
        t.resize(2, 3);
        assert_eq!(t.generation(), generation);
    }

    #[test]
    fn scrollback_limit() {
        let mut t = term(4, 2);
        t.set_scrollback_limit(3);
        for i in 0..10 {
            feed(&mut t, &format!("{i}\r\n"));
        }
        assert_eq!(t.scrollback_len(), 3);
        let rows: Vec<String> = t.rows(3).map(Row::text).collect();
        assert_eq!(rows, ["6", "7"]);
        feed(&mut t, "\x1b[3J");
        assert_eq!(t.scrollback_len(), 0);
    }

    #[test]
    fn reset_clears_everything() {
        let mut t = term(4, 2);
        feed(&mut t, "\x1b[?25l\x1b[31mab\x1b[?1049h\x1bc");
        assert!(!t.is_alternate_screen());
        assert_eq!(screen(&t), ["", ""]);
        assert!(t.modes().cursor_visible);
        assert_eq!(t.inner.pen, Style::default());
    }

    #[test]
    fn ignores_what_it_does_not_know() {
        let mut t = term(4, 2);
        feed(
            &mut t,
            "\x1b[?9999h\x1b[99z\x1bP+q544e\x1b\\\x1b]8;;http://x\x07a\x1b]8;;\x07",
        );
        assert_eq!(t.row_text(0), "a");
    }
}
