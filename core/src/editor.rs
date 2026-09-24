//! Editor model: the UI-agnostic state of a text editor.
//!
//! An [`Editor`] owns a [`FileBuffer`] and layers on top of it everything
//! that makes the buffer editable: a cursor, an optional selection, and an
//! undo history. Every change to the buffer goes through the editor so the
//! history always matches the contents; readers get the buffer through
//! [`Editor::buffer`] for display and searching, but never mutably.
//!
//! Positions are byte offsets into the buffer, which is what the buffer's own
//! line and range queries use. A "character" for movement is a grapheme
//! cluster (see the [`text`](crate::text) module), so the cursor is always
//! kept on a cluster boundary: never inside a multi-byte sequence, between a
//! base character and its combining marks, or inside a CRLF pair.
//! [`Position`] converts to and from line/column pairs for display, where a
//! column is a terminal cell: wide characters take two, tabs extend to the
//! next tab stop. Vertical movement remembers the column it started from, so
//! moving across a short line and back returns to the original column.
//!
//! Undo: cursor and selection changes are not undoable on their own, but each
//! undo entry records the cursor and selection from before and after its
//! edit, and undo/redo restore them. Runs of typing, of backspaces, and of
//! forward deletes are merged into a single entry so that undo works in
//! natural units; a run is broken by any cursor movement, by a newline, by a
//! save, and when a new word starts after whitespace.
//!
//! The clipboard belongs to the UI. [`Editor::copy`] and [`Editor::cut`]
//! return the text for the caller to place on the clipboard, and
//! [`Editor::paste`] takes the text to insert. Undo entries carry the bytes
//! they removed or inserted, so undoing a cut or paste is independent of
//! whatever the clipboard holds by then.
//!
//! Modified state is tracked against the undo history: the editor remembers
//! where in the history the buffer was last saved (or loaded), and reports
//! itself unmodified whenever it is back at that point, whether by undo,
//! redo, or save.
//!
//! Syntax highlighting: the editor keeps a [`Highlighter`] for the
//! buffer's [`Language`] (guessed from the file's name, or
//! [`Language::Plain`]) in step with every edit, and
//! [`Editor::line_cells`] reports each character's [`TokenKind`] for the
//! frontend to style. Merge conflict markers are highlighted in every
//! language, and [`Editor::conflict_side`] says which side of a conflict a
//! line is on so a frontend can tint it. See the [`syntax`](crate::syntax)
//! module. [`Editor::next_conflict`] and [`Editor::previous_conflict`]
//! move the cursor from conflict to conflict, wrapping around the buffer.
//!
//! Indentation: the editor guesses the buffer's [`Indentation`] style when
//! it is created (see the [`indent`](crate::indent) module) and uses it for
//! [`Editor::indent`] and [`Editor::outdent`], which the frontend binds to
//! Tab and Shift+Tab. With a selection they shift whole lines; without one,
//! Tab inserts one level at the cursor and Shift+Tab shifts the cursor's
//! line. In a space-indented buffer, backspace within a line's leading
//! spaces removes a whole level.
//!
//! A newline at the end of an indented line does not copy the indentation
//! into the buffer right away. Instead the editor holds it as *pending*
//! indentation: the cursor is shown at the indented column, Tab and
//! backspace adjust the pending amount, and the whitespace is only written
//! when text is typed after it. Another newline leaves the line blank, with
//! no trailing whitespace, and carries the same pending indentation on.
//! Moving the cursor, or any other edit, discards it.
//!
//! Typing code: the indentation of a new line is predicted from the code
//! before it (see the [`auto_indent`](crate::auto_indent) module), so a
//! line after `if x {` or `def f():` is indented a level deeper, and one
//! after the end of a statement goes back to the level the statement
//! started at. Typing an opening bracket inserts its closing one after the
//! cursor, and typing a closing bracket over the same bracket steps over
//! it. A line break between a pair, as in `{}`, puts the closing bracket
//! on a line of its own below an indented blank line for the cursor.
//! Closing brackets (and `{` in C-like languages) typed at the start of a
//! line move the line to the level they belong at.
//!
//! Search: [`Editor::start_search`] begins a [`Search`] of the buffer from
//! the cursor, and [`Editor::set_search_query`] updates it as the query is
//! typed; the frontend reads the matches back through [`Editor::search`]
//! to highlight them. The search is a *preview* until
//! [`Editor::accept_search`] selects its current match, and
//! [`Editor::next_match`] steps through the rest. In either state, moving
//! the cursor or editing the text drops the search and its highlights;
//! selecting a match through the search itself does not.
//!
//! Files changing on disk: other programs (a formatter, a coding agent, a
//! `git checkout`) write to files while they are open here. The buffer
//! remembers the file as it last read or saved it, and
//! [`Editor::check_disk`], called periodically by the frontend, notices a
//! change and brings it in. With no unsaved edits the buffer simply takes
//! the new contents. With unsaved edits, the buffer's changes and the
//! file's are merged three ways from the remembered contents, the way git
//! merges branches (see the [`merge`](crate::merge) module); overlapping
//! changes are left as conflict markers for the user to resolve. Either
//! way the change lands as one ordinary undoable edit, so "I didn't want
//! that" is just undo, and undo then save puts the buffer's own version
//! back on disk. While nothing has been edited on top of a change from
//! disk, a further change replaces it rather than piling on it: the
//! buffer is always the user's version merged with the latest file, a
//! conflict is one block that follows the file, and undo is still one
//! step. The other way round, [`Editor::discard_changes`] reloads the
//! file over any unsaved edits, and is undoable too.
//!
//! Code completion: the editor doesn't run the model (see the
//! [`completion`](crate::completion) module) but holds the *suggestion*
//! it makes, the text proposed at the cursor that a frontend shows as
//! ghost text after it. Typing is what asks for a completion: after the
//! user types, [`Editor::take_completion_request`] gives the request to
//! send, and the answer comes back through [`Editor::offer_completion`].
//! Moving the cursor never asks for one, so someone reading a file isn't
//! shown ghost text as they go. A suggestion is dropped by any movement
//! or edit, with one exception: typing the very characters it proposes
//! (the line break included, indentation and all) consumes them from it
//! and keeps the rest, so a suggestion the user is typing along with
//! doesn't get replaced under them, and costs nothing more to compute.
//! Tab accepts a suggestion a line at a time: the first press takes its
//! first line, and a second press straight after takes all the rest.
//! Typing in between (Enter, to go on to the next line, say) puts the
//! next Tab back to taking a single line, so any number of lines can be
//! taken with Tab, Enter, Tab, Enter and so on. Once a suggestion has
//! been taken in full, or typed through to its end, the next completion
//! is asked for. A suggestion never goes past the end of the scope the
//! cursor is in, so it can't propose a closing bracket that pairing has
//! already typed (see [`Editor::offer_completion`]).

use crate::auto_indent::{self, CodeLine, CodeStyle, Indenter, Rules};
use crate::buffer::{BufferSnapshot, DiskChange, FileBuffer};
use crate::completion::CompletionRequest;
use crate::indent::{self, Indentation};
use crate::merge;
use crate::search::{Search, SearchStep};
use crate::syntax::{self, ConflictSide, Highlighter, Language, Token, TokenKind};
use crate::text::{self, Grapheme};
use std::io;
use std::ops::Range;
use std::path::Path;

/// The default number of cells between tab stops.
pub const DEFAULT_TAB_WIDTH: usize = 4;

/// How much of the text before the cursor a completion request carries,
/// at most; the model sees less, but the worker chooses which part.
const COMPLETION_PREFIX_BYTES: usize = 24 * 1024;
/// How many lines after the cursor's a completion request carries.
const COMPLETION_SUFFIX_LINES: usize = 48;

/// A cursor movement, used both to move the cursor and to extend a selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Movement {
    /// One character left; at the start of a line, to the end of the previous
    /// line.
    Left,
    /// One character right; at the end of a line, to the start of the next.
    Right,
    /// One line up, keeping the column. On the first line, to the start of
    /// the buffer.
    Up,
    /// One line down, keeping the column. On the last line, to the end of the
    /// buffer.
    Down,
    /// To the start of the previous word (skipping whitespace first); at the
    /// start of a line, to the end of the previous line.
    WordLeft,
    /// To the end of the next word (skipping whitespace first); at the end of
    /// a line, to the start of the next line.
    WordRight,
    /// To the first non-whitespace byte of the current line, or, when the
    /// cursor is already at or before it, to the first byte of the line.
    /// From the first byte the cursor moves to the first non-whitespace one
    /// again, so pressing the key twice reaches whichever the first press
    /// didn't. On a blank line, to the first byte.
    LineStart,
    /// To the end of the current line's content, before its terminator.
    LineEnd,
    /// Up by the given number of lines (the height of the view), keeping the
    /// column. On the first line, to the start of the buffer.
    PageUp(usize),
    /// Down by the given number of lines (the height of the view), keeping
    /// the column. On the last line, to the end of the buffer.
    PageDown(usize),
    DocumentStart,
    DocumentEnd,
}

/// A location in the buffer as a zero-based line and column. The column is
/// a display column in terminal cells (see [`text::width`]), not a byte or
/// code point index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

/// One character of a line laid out for display: where it lives in the
/// buffer and which cells it occupies. See [`Editor::line_cells`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The character's byte range in the buffer.
    pub range: Range<usize>,
    /// The character's text; U+FFFD for a byte of invalid UTF-8.
    pub text: String,
    /// The display column the character starts at.
    pub column: usize,
    /// The number of cells the character occupies (may be zero).
    pub width: usize,
    /// What the character is part of, for styling.
    pub kind: TokenKind,
}

/// The cursor and selection anchor, as saved in undo entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CursorState {
    cursor: usize,
    anchor: Option<usize>,
}

/// One replacement of bytes at an offset: `removed` was replaced with
/// `inserted`. Either may be empty.
struct Edit {
    offset: usize,
    removed: Vec<u8>,
    inserted: Vec<u8>,
}

/// The kind of edit an undo entry holds, used to decide whether consecutive
/// edits merge into one entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditKind {
    /// Characters typed one at a time.
    Typing,
    /// Backspace over single characters.
    Backspace,
    /// Forward deletion of single characters.
    Delete,
    /// Anything else; never merged.
    Other,
}

struct UndoEntry {
    before: CursorState,
    after: CursorState,
    edit: Edit,
    kind: EditKind,
}

/// A completion proposed at the cursor; see the [module
/// documentation](self).
struct Suggestion {
    /// What is left to insert at the cursor, with `\n` line breaks.
    /// Never empty.
    text: String,
    /// Whether Tab has taken its first line, so that the next Tab takes
    /// all the rest.
    accepted_line: bool,
    /// Where the suggestion was cut short of a closing bracket the buffer
    /// already has, the whitespace it put before that bracket, which
    /// taking the last of the suggestion puts there in place of what
    /// the buffer has (so the bracket of a block goes on a line of its
    /// own); see [`Editor::offer_completion`].
    before_closer: Option<String>,
}

/// Where a suggestion confined to the cursor's scope ends; see
/// [`Editor::confine_to_scope`].
enum ScopeEnd<'a> {
    /// It stays within the scope.
    Open,
    /// It ends with the closing bracket of the scope.
    Closed,
    /// It ends before the closing bracket of the scope, which the buffer
    /// has next; the whitespace is what it put before its own.
    BeforeCloser(&'a str),
}

/// The layout of one line's content: its characters with their byte ranges
/// (relative to the start of the line) and display columns.
struct Layout {
    /// Byte offset of the line's content in the buffer.
    start: usize,
    cells: Vec<LayoutCell>,
    /// The display column after the last character.
    end_column: usize,
}

struct LayoutCell {
    range: Range<usize>,
    column: usize,
    width: usize,
}

impl Layout {
    fn new(start: usize, bytes: &[u8], tab_width: usize) -> Layout {
        let mut column = 0;
        let cells = text::graphemes(bytes)
            .map(|Grapheme { range, text }| {
                let width = text::width(text, column, tab_width);
                let cell = LayoutCell {
                    range,
                    column,
                    width,
                };
                column += width;
                cell
            })
            .collect();
        Layout {
            start,
            cells,
            end_column: column,
        }
    }

    fn len(&self) -> usize {
        self.cells.last().map_or(0, |c| c.range.end)
    }

    /// Index of the character containing the relative offset `rel`.
    fn cell_at(&self, rel: usize) -> Option<usize> {
        let i = self.cells.partition_point(|c| c.range.end <= rel);
        (i < self.cells.len()).then_some(i)
    }

    /// The display column of a relative offset, which is snapped back to
    /// the start of the character containing it.
    fn column_of(&self, rel: usize) -> usize {
        match self.cell_at(rel) {
            Some(i) => self.cells[i].column,
            None => self.end_column,
        }
    }

    /// The relative offset of the character covering a display column, or
    /// the end of the line if the column is past its content. A column in
    /// the middle of a wide character maps to that character's start.
    fn offset_at_column(&self, column: usize) -> usize {
        let i = self.cells.partition_point(|c| c.column + c.width <= column);
        self.cells.get(i).map_or(self.len(), |c| c.range.start)
    }
}

/// Rewrite every line break in `text` (LF, CRLF, or lone CR) as `eol`.
fn normalize_line_endings(text: &str, eol: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' => out.push_str(eol),
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str(eol);
            }
            _ => out.push(c),
        }
    }
    out
}

/// A text editor over a single [`FileBuffer`]: cursor, selection, and undo
/// history. See the [module documentation](self) for an overview.
pub struct Editor {
    buffer: FileBuffer,
    cursor: usize,
    /// The fixed end of the selection; the cursor is the moving end. `None`
    /// when nothing is selected.
    anchor: Option<usize>,
    /// The column vertical movement aims for, remembered across lines that
    /// are too short to reach it. Cleared by anything that isn't a vertical
    /// movement.
    desired_column: Option<usize>,
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
    /// Whether the newest undo entry may absorb another edit of its kind.
    grouping: bool,
    /// The undo stack depth at which the buffer matched the file on disk, or
    /// `None` if that state is no longer in the history.
    save_point: Option<usize>,
    tab_width: usize,
    /// How lines are indented; see [`indentation`](Self::indentation).
    indentation: Indentation,
    /// Layout preferences for typing code; see
    /// [`code_style`](Self::code_style).
    code_style: CodeStyle,
    /// Indentation the cursor is shown at but that isn't in the buffer
    /// yet; see the [module documentation](self). Only ever spaces and
    /// tabs. When set, the cursor is at the start of an empty line.
    pending: Option<String>,
    /// Syntax highlighting.
    highlighter: Highlighter,
    /// The search in progress or accepted, if any. Its offsets are only
    /// valid for the buffer as it was when it began, so every edit drops
    /// it.
    search: Option<Search>,
    /// The last change brought in from disk, while nothing has been edited
    /// on top of it; see [`check_disk`](Self::check_disk).
    external: Option<External>,
    /// The completion proposed at the cursor, shown as ghost text; see
    /// the [module documentation](self).
    suggestion: Option<Suggestion>,
    /// Whether the user has typed since a completion was last asked for,
    /// so that a new one is wanted; see
    /// [`take_completion_request`](Self::take_completion_request).
    completion_wanted: bool,
    /// Numbers the completion requests, and moves on whenever the text
    /// or the cursor changes, so the answer to a request made before is
    /// known to be stale.
    completion_serial: u64,
}

impl Editor {
    /// Create an editor over a buffer, with the cursor at the start and no
    /// selection. The indentation style is guessed from the contents.
    pub fn new(buffer: FileBuffer) -> Editor {
        let save_point = if buffer.is_modified() { None } else { Some(0) };
        let language = buffer
            .path()
            .and_then(Language::from_path)
            .unwrap_or(Language::Plain);
        let highlighter = Highlighter::new(language, buffer.line_count());
        let indentation = indent::detect(&buffer.snapshot()).unwrap_or_default();
        Editor {
            buffer,
            cursor: 0,
            anchor: None,
            desired_column: None,
            undo: Vec::new(),
            redo: Vec::new(),
            grouping: false,
            save_point,
            tab_width: DEFAULT_TAB_WIDTH,
            indentation,
            code_style: CodeStyle::default(),
            pending: None,
            highlighter,
            search: None,
            external: None,
            suggestion: None,
            completion_wanted: false,
            completion_serial: 0,
        }
    }

    /// The buffer being edited, for reading and display. All edits go
    /// through the editor so the undo history stays consistent.
    pub fn buffer(&self) -> &FileBuffer {
        &self.buffer
    }

    // ----- Syntax highlighting --------------------------------------------

    /// The language the buffer is highlighted as: guessed from the file's
    /// name when the editor is created or the buffer is saved under a new
    /// name, [`Language::Plain`] when nothing is known about it. See
    /// [`set_language`](Self::set_language) to override.
    pub fn language(&self) -> Language {
        self.highlighter.language()
    }

    /// Highlight the buffer as `language`.
    pub fn set_language(&mut self, language: Language) {
        if language == self.language() {
            return;
        }
        self.highlighter = Highlighter::new(language, self.buffer.line_count());
    }

    /// The syntax tokens of a line, with ranges relative to the start of
    /// the line's content.
    pub fn line_tokens(&self, line: usize) -> Vec<Token> {
        self.highlighter.tokens(&self.buffer, line)
    }

    /// The side of a merge conflict that `line` is on, if any, marker
    /// lines included; see [`Highlighter::conflict_side`]. For a frontend
    /// to tint the lines of a conflict by side.
    pub fn conflict_side(&self, line: usize) -> Option<ConflictSide> {
        self.highlighter.conflict_side(&self.buffer, line)
    }

    /// Move the cursor to the start of the next merge conflict: the
    /// `<<<<<<<` line after the cursor's, or the first in the buffer
    /// when there is none after it. The selection is cleared. See
    /// [`ConflictStep`] for what is reported.
    pub fn next_conflict(&mut self) -> ConflictStep {
        let starts = self.conflict_starts();
        let line = self.buffer.line_of_offset(self.cursor);
        let after = starts.iter().find(|&&start| start > line);
        self.step_conflict(after, starts.first())
    }

    /// Move the cursor to the start of the previous merge conflict: the
    /// `<<<<<<<` line before the cursor's, or the last in the buffer
    /// when there is none before it. The selection is cleared. See
    /// [`ConflictStep`] for what is reported.
    pub fn previous_conflict(&mut self) -> ConflictStep {
        let starts = self.conflict_starts();
        let line = self.buffer.line_of_offset(self.cursor);
        let before = starts.iter().rev().find(|&&start| start < line);
        self.step_conflict(before, starts.last())
    }

    /// Go to `next` if there is one, else wrap around to `wrapped`.
    fn step_conflict(&mut self, next: Option<&usize>, wrapped: Option<&usize>) -> ConflictStep {
        let (line, step) = match (next, wrapped) {
            (Some(&line), _) => (line, ConflictStep::Moved),
            (None, Some(&line)) => (line, ConflictStep::Wrapped),
            (None, None) => return ConflictStep::NoConflicts,
        };
        self.go_to_line(line);
        step
    }

    /// Whether the file holds a merge conflict, so that stepping between
    /// conflicts has somewhere to go. Read from the buffer like
    /// [`conflict_starts`](Self::conflict_starts), stopping at the
    /// first.
    pub fn has_conflicts(&self) -> bool {
        let snapshot = self.buffer.snapshot();
        let mut lines = snapshot.lines_from(0);
        while let Some(content) = lines.next_line() {
            if syntax::is_conflict_start(content) {
                return true;
            }
        }
        false
    }

    /// The lines that open a conflict, in order. Read straight from the
    /// buffer rather than the highlighter's states, which may still be
    /// catching up on a large file: a jump has to land on what is there
    /// now.
    fn conflict_starts(&self) -> Vec<usize> {
        let snapshot = self.buffer.snapshot();
        let mut lines = snapshot.lines_from(0);
        let mut starts = Vec::new();
        let mut line = 0;
        while let Some(content) = lines.next_line() {
            if syntax::is_conflict_start(content) {
                starts.push(line);
            }
            line += 1;
        }
        starts
    }

    /// A counter that changes when background highlighting has updated
    /// lines, so a frontend knows to redraw. See
    /// [`Highlighter::generation`].
    pub fn highlight_generation(&self) -> u64 {
        self.highlighter.generation()
    }

    /// The number of cells between tab stops, used for display columns.
    pub fn tab_width(&self) -> usize {
        self.tab_width
    }

    pub fn set_tab_width(&mut self, tab_width: usize) {
        self.tab_width = tab_width.max(1);
    }

    /// Lay out a line's content for display: each character with its byte
    /// range and the cells it occupies, using the editor's tab width. The
    /// line's terminator is not included.
    /// The display width of a line in columns, without laying out its
    /// syntax highlighting. Cheaper than [`line_cells`](Self::line_cells)
    /// when only the width matters.
    pub fn line_width(&self, line: usize) -> usize {
        self.layout(line).end_column
    }

    pub fn line_cells(&self, line: usize) -> Vec<Cell> {
        let layout = self.layout(line);
        let bytes = self
            .buffer
            .bytes_in_range(layout.start..layout.start + layout.len());
        let tokens = self.highlighter.tokens_of(&self.buffer, line, &bytes);
        let mut next_token = 0;
        layout
            .cells
            .iter()
            .zip(text::graphemes(&bytes))
            .map(|(cell, grapheme)| {
                // Tokens are ordered and disjoint, as are cells, so one
                // forward pass matches them up. A character is styled by
                // the token containing its first byte.
                while next_token < tokens.len() && tokens[next_token].range.end <= cell.range.start
                {
                    next_token += 1;
                }
                let kind = match tokens.get(next_token) {
                    Some(token) if token.range.start <= cell.range.start => token.kind,
                    _ => TokenKind::Text,
                };
                Cell {
                    range: layout.start + cell.range.start..layout.start + cell.range.end,
                    text: grapheme.text.to_owned(),
                    column: cell.column,
                    width: cell.width,
                    kind,
                }
            })
            .collect()
    }

    // ----- Cursor and selection -------------------------------------------

    /// The cursor's byte offset.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The cursor as a line and column. With pending indentation, the
    /// column is where the cursor is shown, past the indentation, not the
    /// column of its byte offset.
    pub fn cursor_position(&self) -> Position {
        let mut position = self.position_of_offset(self.cursor);
        if let Some(pending) = &self.pending {
            position.column += self.indent_columns(pending);
        }
        position
    }

    /// The selection anchor: the end of the selection that doesn't move.
    /// `None` when nothing is selected.
    pub fn anchor(&self) -> Option<usize> {
        self.anchor
    }

    /// The selected byte range, or `None` if the selection is empty.
    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        if anchor == self.cursor {
            return None;
        }
        Some(anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    /// The selected text, or `None` if the selection is empty. Invalid UTF-8
    /// is replaced.
    pub fn selected_text(&self) -> Option<String> {
        let range = self.selection()?;
        Some(String::from_utf8_lossy(&self.buffer.bytes_in_range(range)).into_owned())
    }

    /// The line and column of a byte offset.
    pub fn position_of_offset(&self, offset: usize) -> Position {
        let offset = offset.min(self.buffer.len());
        let line = self.buffer.line_of_offset(offset);
        Position {
            line,
            column: self.column_of(line, offset),
        }
    }

    /// The byte offset of a line and column. Lines past the end map to the
    /// last line, and columns past the end of a line to the end of that
    /// line's content.
    pub fn offset_of_position(&self, position: Position) -> usize {
        let line = position.line.min(self.buffer.line_count() - 1);
        self.offset_at_column(line, position.column)
    }

    /// Place the cursor at a byte offset and clear the selection. The offset
    /// is clamped to the buffer and snapped back to a character boundary.
    pub fn set_cursor(&mut self, offset: usize) {
        self.cursor = self.snap(offset);
        self.anchor = None;
        self.end_movement();
    }

    /// Place the cursor at the start of a line (counted from zero) and
    /// clear the selection. A line past the end of the buffer goes to the
    /// last line.
    pub fn go_to_line(&mut self, line: usize) {
        let offset = self.offset_of_position(Position { line, column: 0 });
        self.set_cursor(offset);
    }

    /// Select from `anchor` to `cursor`, leaving the cursor at `cursor`. Both
    /// are clamped and snapped like [`set_cursor`](Self::set_cursor).
    pub fn set_selection(&mut self, anchor: usize, cursor: usize) {
        self.anchor = Some(self.snap(anchor));
        self.cursor = self.snap(cursor);
        self.end_movement();
    }

    /// Select the whole buffer, leaving the cursor at the end.
    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.buffer.len();
        self.end_movement();
    }

    /// Clear the selection, leaving the cursor where it is.
    pub fn clear_selection(&mut self) {
        self.anchor = None;
        self.end_movement();
    }

    /// Select the word at a byte offset, as a double-click does: the run
    /// of word characters (or of spaces, or of punctuation) containing
    /// it, or at the end of a line the run ending there; see
    /// [`text::word_at`]. The cursor ends up at the end of the word. On
    /// an empty line nothing is selected and the cursor moves there.
    pub fn select_word_at(&mut self, offset: usize) {
        let offset = self.snap(offset);
        let (start, bytes) = self.line_bytes_at(offset);
        let word = text::word_at(&bytes, offset - start);
        self.set_selection(start + word.start, start + word.end);
    }

    /// Move the cursor, clearing any selection. Moving left or right out of
    /// a selection collapses it to its start or end.
    pub fn move_cursor(&mut self, movement: Movement) {
        if let Some(range) = self.selection() {
            let collapsed = match movement {
                Movement::Left => Some(range.start),
                Movement::Right => Some(range.end),
                _ => None,
            };
            if let Some(offset) = collapsed {
                self.cursor = offset;
                self.anchor = None;
                self.end_movement();
                return;
            }
        }
        self.apply_movement(movement, false);
    }

    /// Move the cursor while keeping (or starting) a selection from where
    /// the cursor was.
    pub fn extend_selection(&mut self, movement: Movement) {
        self.apply_movement(movement, true);
    }

    fn apply_movement(&mut self, movement: Movement, extend: bool) {
        let vertical = matches!(
            movement,
            Movement::Up | Movement::Down | Movement::PageUp(_) | Movement::PageDown(_)
        );
        let line = self.buffer.line_of_offset(self.cursor);
        let mut column = if vertical {
            Some(
                self.desired_column
                    .unwrap_or_else(|| self.cursor_position().column),
            )
        } else {
            None
        };
        let mut vertical_target =
            |delta: isize| match self.vertical_target(line, column.unwrap(), delta) {
                Some(offset) => offset,
                // Ran off the first or last line: jump to that end of the buffer,
                // which is a horizontal move, so forget the column.
                None => {
                    column = None;
                    if delta < 0 { 0 } else { self.buffer.len() }
                }
            };
        let target = match movement {
            Movement::Left => self.prev_char(self.cursor),
            Movement::Right => self.next_char(self.cursor),
            Movement::Up => vertical_target(-1),
            Movement::Down => vertical_target(1),
            Movement::PageUp(height) => vertical_target(-(height.max(1) as isize)),
            Movement::PageDown(height) => vertical_target(height.max(1) as isize),
            Movement::WordLeft => self.word_left(self.cursor),
            Movement::WordRight => self.word_right(self.cursor),
            Movement::LineStart => self.line_start_target(line),
            Movement::LineEnd => self.buffer.line_content_range(line).end,
            Movement::DocumentStart => 0,
            Movement::DocumentEnd => self.buffer.len(),
        };
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = target;
        self.grouping = false;
        self.desired_column = column;
        self.pending = None;
        self.search = None;
        self.drop_suggestion();
    }

    /// Where [`Movement::LineStart`] goes from the cursor's line: the end of
    /// the line's leading whitespace, unless the cursor is already at or
    /// before it (but not at the very start of the line), in which case the
    /// start of the line. A line that is all whitespace has no first
    /// character to go to, so its start is the only target.
    fn line_start_target(&self, line: usize) -> usize {
        let indent = self.leading_whitespace(line);
        let content = self.buffer.line_content_range(line);
        if indent.end == content.end {
            return indent.start;
        }
        if self.cursor > indent.start && self.cursor <= indent.end {
            indent.start
        } else {
            indent.end
        }
    }

    /// Bookkeeping shared by all explicit cursor changes: they break undo
    /// grouping, forget the remembered vertical column, discard pending
    /// indentation, and drop any search and suggestion.
    fn end_movement(&mut self) {
        self.grouping = false;
        self.desired_column = None;
        self.pending = None;
        self.search = None;
        self.drop_suggestion();
    }

    // ----- Search ---------------------------------------------------------

    /// Begin a search from the cursor (or from the start of the selection,
    /// so that a selected occurrence is the first match) with an empty
    /// query. Any earlier search is dropped.
    pub fn start_search(&mut self) {
        let origin = self.selection().map_or(self.cursor, |range| range.start);
        self.search = Some(Search::new(&self.buffer, origin));
    }

    /// Change the query of the search in progress. Does nothing without
    /// one; see [`start_search`](Self::start_search).
    pub fn set_search_query(&mut self, query: &str) {
        if let Some(search) = &mut self.search {
            search.set_query(query);
        }
    }

    /// The search in progress or accepted, for the frontend to show its
    /// matches. `None` when there is none.
    pub fn search(&self) -> Option<&Search> {
        self.search.as_ref()
    }

    /// Select the search's current match, turning the preview into an
    /// accepted search whose other matches stay highlighted until the
    /// cursor moves or the text changes. Waits for the search to finish if
    /// the first match isn't known yet. Returns whether there was a match
    /// to select; with none the search stays as it was.
    pub fn accept_search(&mut self) -> bool {
        let Some(search) = &mut self.search else {
            return false;
        };
        if search.current().is_none() {
            search.wait();
        }
        let Some(range) = search.current() else {
            return false;
        };
        search.set_accepted();
        self.select_match(range);
        true
    }

    /// Step the search to its next match, wrapping around the end of the
    /// buffer; see [`Search::advance`] for how arriving back at the start
    /// is reported. With an accepted search the match is selected; while
    /// previewing only the current match changes.
    pub fn next_match(&mut self) -> SearchStep {
        let Some(search) = &mut self.search else {
            return SearchStep::NoMatches;
        };
        let step = search.advance();
        if let SearchStep::Moved(range) = &step
            && search.is_accepted()
        {
            self.select_match(range.clone());
        }
        step
    }

    /// Drop the search and its highlights, leaving the cursor and
    /// selection alone.
    pub fn clear_search(&mut self) {
        self.search = None;
    }

    /// Select a match without dropping the search, as
    /// [`set_selection`](Self::set_selection) otherwise would.
    fn select_match(&mut self, range: Range<usize>) {
        self.anchor = Some(self.snap(range.start));
        self.cursor = self.snap(range.end);
        self.grouping = false;
        self.desired_column = None;
        self.pending = None;
        self.drop_suggestion();
    }

    // ----- Position arithmetic -------------------------------------------

    fn layout(&self, line: usize) -> Layout {
        let content = self.buffer.line_content_range(line);
        let bytes = self.buffer.bytes_in_range(content.clone());
        Layout::new(content.start, &bytes, self.tab_width)
    }

    /// Clamp an offset to the buffer and move it back to the nearest
    /// character boundary (out of a grapheme cluster or a CRLF pair).
    fn snap(&self, offset: usize) -> usize {
        let offset = offset.min(self.buffer.len());
        let layout = self.layout(self.buffer.line_of_offset(offset));
        let rel = offset - layout.start;
        match layout.cell_at(rel) {
            Some(i) => layout.start + layout.cells[i].range.start,
            None => layout.start + layout.len(), // at the end, or inside a CRLF pair
        }
    }

    /// The display column of an offset that lies on `line`.
    fn column_of(&self, line: usize, offset: usize) -> usize {
        let layout = self.layout(line);
        layout.column_of(offset.saturating_sub(layout.start))
    }

    /// The offset of a display column on a line, clamped to the end of the
    /// line's content.
    fn offset_at_column(&self, line: usize, column: usize) -> usize {
        let layout = self.layout(line);
        layout.start + layout.offset_at_column(column)
    }

    /// The offset one character to the right, crossing to the next line at
    /// the end of a line. Returns `offset` itself at the end of the buffer.
    fn next_char(&self, offset: usize) -> usize {
        let line = self.buffer.line_of_offset(offset);
        let layout = self.layout(line);
        match layout.cell_at(offset - layout.start) {
            Some(i) => layout.start + layout.cells[i].range.end,
            None if line + 1 < self.buffer.line_count() => self.buffer.offset_of_line(line + 1),
            None => offset,
        }
    }

    /// The offset one character to the left, crossing to the previous line
    /// at the start of a line. Returns 0 at the start of the buffer.
    fn prev_char(&self, offset: usize) -> usize {
        let line = self.buffer.line_of_offset(offset);
        let layout = self.layout(line);
        let rel = offset - layout.start;
        if rel == 0 {
            return if line > 0 {
                self.buffer.line_content_range(line - 1).end
            } else {
                0
            };
        }
        // The character ending at (or containing) rel.
        let i = layout.cells.partition_point(|c| c.range.end < rel);
        layout.start + layout.cells[i.min(layout.cells.len() - 1)].range.start
    }

    /// The bytes of the line containing `offset`, with the offset of the
    /// line's start.
    fn line_bytes_at(&self, offset: usize) -> (usize, Vec<u8>) {
        let content = self
            .buffer
            .line_content_range(self.buffer.line_of_offset(offset));
        (content.start, self.buffer.bytes_in_range(content))
    }

    /// The next word boundary, crossing to the next line from the end of
    /// a line.
    fn word_right(&self, offset: usize) -> usize {
        let (start, bytes) = self.line_bytes_at(offset);
        if offset >= start + bytes.len() {
            return self.next_char(offset);
        }
        start + text::next_word_boundary(&bytes, offset - start)
    }

    /// The previous word boundary, crossing to the previous line from
    /// the start of a line.
    fn word_left(&self, offset: usize) -> usize {
        let (start, bytes) = self.line_bytes_at(offset);
        if offset <= start {
            return self.prev_char(offset);
        }
        start + text::prev_word_boundary(&bytes, offset - start)
    }

    /// The target of moving `delta` lines from `line` aiming for `column`,
    /// or `None` if `line` is already the first (or last) line.
    fn vertical_target(&self, line: usize, column: usize, delta: isize) -> Option<usize> {
        let last = self.buffer.line_count() - 1;
        let target = if delta < 0 {
            line.saturating_sub(delta.unsigned_abs())
        } else {
            (line + delta as usize).min(last)
        };
        if target == line {
            return None;
        }
        Some(self.offset_at_column(target, column))
    }

    // ----- Editing --------------------------------------------------------

    /// Insert a typed character at the cursor, replacing the selection if
    /// there is one. `'\n'` and `'\r'` insert the buffer's line ending and
    /// carry the line's indentation on as pending indentation (see the
    /// [module documentation](self)); any other character first commits
    /// pending indentation to the buffer. Brackets are paired and
    /// stepped over, and some characters reindent their line; see the
    /// [module documentation](self).
    ///
    /// Typing what the suggestion at the cursor proposes consumes it from
    /// the suggestion and keeps the rest; typing anything else drops the
    /// suggestion. Either way the character is typed as it would be with
    /// no suggestion, brackets paired and all, so what a bracket does
    /// doesn't depend on whether a suggestion happened to arrive before
    /// it; the rest of the suggestion is then confined to the new scope
    /// (see [`offer_completion`](Self::offer_completion)), which leaves
    /// out its closing bracket where the pair supplied one. This is
    /// typing, which asks for a completion unless a suggestion remains.
    pub fn insert_char(&mut self, c: char) {
        let newline = c == '\n' || c == '\r';
        let suggestion = self.suggestion.take();
        if let Some(suggestion) = &suggestion
            && newline
            && suggestion.text.starts_with('\n')
        {
            self.insert_newline();
            let before_closer = suggestion.before_closer.clone();
            self.continue_suggestion_on_next_line(&suggestion.text[1..], before_closer);
            return;
        }
        let mut bytes = [0u8; 4];
        let typed = c.encode_utf8(&mut bytes);
        let rest = suggestion.filter(|_| !newline).and_then(|s| {
            let rest = s.text.strip_prefix(&*typed)?.to_owned();
            Some((rest, s.before_closer))
        });
        if newline {
            self.insert_newline();
        } else if self.selection().is_some() || !c.is_ascii() || !self.type_code_char(c as u8) {
            self.type_char(c);
        }
        match rest {
            Some((rest, before_closer)) => self.keep_suggestion(&rest, before_closer),
            None => self.completion_wanted = true,
        }
    }

    /// Insert a typed character other than a line break, committing
    /// pending indentation first.
    fn type_char(&mut self, c: char) {
        let mut inserted = self.pending.take().unwrap_or_default().into_bytes();
        let mut bytes = [0u8; 4];
        inserted.extend_from_slice(c.encode_utf8(&mut bytes).as_bytes());
        self.replace_selection_with(inserted, EditKind::Typing);
    }

    /// Type a character that means something for the structure of code,
    /// with no selection. Returns whether it was handled; if not, it is
    /// for the caller to insert as usual.
    ///
    /// * A closing bracket typed over the same bracket steps over it.
    /// * An opening bracket gets its closing bracket inserted after the
    ///   cursor, unless it is typed in a string or comment, or right
    ///   before other text.
    /// * A closing bracket typed at the start of a line (after nothing but
    ///   indentation) moves the line level with the statement of the
    ///   bracket it closes; likewise `{` in C-like languages, level with
    ///   the statement it opens the body of. See the
    ///   [`auto_indent`](crate::auto_indent) module.
    /// * A `:` completing a `case` label or a Python `else:` (and the like)
    ///   moves the line level with the label or block it continues.
    fn type_code_char(&mut self, c: u8) -> bool {
        let closer = auto_indent::closer_of(c);
        if closer.is_none() && !auto_indent::is_closer(c) && c != b':' {
            return false;
        }
        let line = self.buffer.line_of_offset(self.cursor);
        let start = self.buffer.offset_of_line(line);
        let col = self.cursor - start;
        let code_line = self.code_line(line);
        let text = &code_line.text;
        let next = text.get(col).copied();
        if auto_indent::is_closer(c) && next == Some(c) && code_line.code[col] {
            self.cursor += 1;
            self.grouping = false;
            self.desired_column = None;
            self.search = None;
            return true;
        }

        let is_blank = |b: &u8| *b == b' ' || *b == b'\t';
        let at_line_start = text[..col].iter().all(is_blank);
        let rest_blank = text[col..].iter().all(is_blank);
        let lead_len = code_line.leading().len().min(col);
        let current_indent = match &self.pending {
            Some(pending) => pending.as_bytes(),
            None => &text[..lead_len],
        };
        let rules = Rules::for_language(self.language());
        let reindent = {
            let mut indenter = self.indenter();
            if at_line_start && auto_indent::is_closer(c) {
                indenter.closer_indent(line, col)
            } else if at_line_start && c == b'{' && rules.c_like {
                indenter.brace_line_indent(line)
            } else if c == b':' && !at_line_start && rest_blank {
                indenter.label_indent(line)
            } else {
                None
            }
        }
        .filter(|indent| indent[..] != *current_indent);
        let pair =
            closer.filter(|_| pairs_before(next) && self.typed_as_code(line, &text[..col], c));
        if reindent.is_none() && pair.is_none() {
            return false;
        }

        let (from, mut inserted) = match reindent {
            Some(mut indent) => {
                indent.extend_from_slice(&text[lead_len..col]);
                (start, indent)
            }
            None => (
                self.cursor,
                self.pending.clone().unwrap_or_default().into_bytes(),
            ),
        };
        inserted.push(c);
        let cursor = from + inserted.len();
        inserted.extend(pair);
        self.commit_with_cursor(from..self.cursor, inserted, EditKind::Typing, cursor, None);
        true
    }

    /// Whether `c`, typed on `line` after `before`, would be code rather
    /// than part of a string or comment.
    fn typed_as_code(&self, line: usize, before: &[u8], c: u8) -> bool {
        let mut probe = before.to_vec();
        probe.push(c);
        let at = before.len();
        self.highlighter
            .tokens_of(&self.buffer, line, &probe)
            .iter()
            .find(|token| token.range.contains(&at))
            .is_none_or(|token| auto_indent::is_code(token.kind))
    }

    /// A line's content, with which bytes are code, as the
    /// [`auto_indent`](crate::auto_indent) rules read it.
    fn code_line(&self, line: usize) -> CodeLine {
        let content = self
            .buffer
            .bytes_in_range(self.buffer.line_content_range(line));
        let tokens = self.highlighter.tokens_of(&self.buffer, line, &content);
        CodeLine::new(content, &tokens)
    }

    fn indenter(&self) -> Indenter<impl FnMut(usize) -> CodeLine + '_> {
        Indenter::new(
            Rules::for_language(self.language()),
            self.code_style,
            self.indentation,
            self.tab_width,
            |line| self.code_line(line),
        )
    }

    /// Insert a line break at the cursor (replacing the selection),
    /// indenting the new line by the rules of the
    /// [`auto_indent`](crate::auto_indent) module. When the cursor is in
    /// a line's leading whitespace, the whitespace before it is carried on
    /// instead. When nothing follows the cursor on its line, the
    /// indentation becomes pending on the new line; otherwise the text
    /// moving to the new line is indented right away. Between a pair of
    /// brackets, as in `{}`, the closing bracket moves down a further line,
    /// leaving the cursor on an indented blank line between the two. With
    /// indentation already pending, the current line is left blank and
    /// the pending indentation moves on.
    fn insert_newline(&mut self) {
        let eol = self.buffer.eol().as_str().as_bytes().to_vec();
        if let Some(pending) = self.pending.take() {
            self.replace_selection_with(eol, EditKind::Other);
            self.pending = Some(pending);
            return;
        }
        let range = self.selection().unwrap_or(self.cursor..self.cursor);
        let line = self.buffer.line_of_offset(range.start);
        let col = range.start - self.buffer.offset_of_line(line);
        let code_line = self.code_line(line);
        let before = &code_line.text[..col];
        let end_line = self.buffer.line_of_offset(range.end);
        let rest = self
            .buffer
            .bytes_in_range(range.end..self.buffer.line_content_range(end_line).end);
        let next = rest.first().copied();
        let (indent, closer_indent) = if before.iter().all(|&b| b == b' ' || b == b'\t') {
            (before.to_vec(), None)
        } else {
            let paired = code_line.code[col - 1]
                && next.is_some()
                && auto_indent::closer_of(before[col - 1]) == next;
            let mut indenter = self.indenter();
            if paired {
                let inner = indenter.newline_indent(line, col, None);
                (inner, indenter.closer_indent(line, col))
            } else {
                (indenter.newline_indent(line, col, next), None)
            }
        };
        let mut inserted = eol.clone();
        if let Some(closer_indent) = closer_indent {
            inserted.extend_from_slice(&eol);
            inserted.extend_from_slice(&closer_indent);
            let cursor = range.start + eol.len();
            self.commit_with_cursor(range, inserted, EditKind::Other, cursor, None);
            self.set_pending(indent);
        } else if rest.is_empty() {
            self.commit(range, inserted, EditKind::Other);
            self.set_pending(indent);
        } else {
            inserted.extend_from_slice(&indent);
            self.commit(range, inserted, EditKind::Other);
        }
    }

    /// Make `indent` (spaces and tabs) the pending indentation, if there
    /// is any.
    fn set_pending(&mut self, indent: Vec<u8>) {
        if !indent.is_empty() {
            self.pending = String::from_utf8(indent).ok();
        }
    }

    /// Insert text at the cursor, replacing the selection if there is one.
    /// Line breaks in the text are rewritten as the buffer's line ending.
    /// Pending indentation is committed first. Counts as typing, so a
    /// completion is asked for afterwards.
    pub fn insert_text(&mut self, text: &str) {
        self.insert_text_quietly(text);
        self.completion_wanted = true;
    }

    /// [`insert_text`](Self::insert_text) without asking for a completion.
    fn insert_text_quietly(&mut self, text: &str) {
        let text = normalize_line_endings(text, self.buffer.eol().as_str());
        if text.is_empty() {
            return;
        }
        let mut inserted = self.pending.take().unwrap_or_default().into_bytes();
        inserted.extend_from_slice(text.as_bytes());
        self.replace_selection_with(inserted, EditKind::Other);
    }

    /// Delete the selection, or the character before the cursor if nothing
    /// is selected. With pending indentation, removes one level of it
    /// instead. Between an empty pair of brackets, as in `(|)`, removes
    /// both. In a space-indented buffer, when only spaces precede the
    /// cursor on its line, removes back to the previous indentation stop.
    pub fn backspace(&mut self) {
        self.drop_suggestion();
        self.backspace_inner();
        self.completion_wanted = true;
    }

    fn backspace_inner(&mut self) {
        if let Some(mut pending) = self.pending.take() {
            self.shrink_indent(&mut pending);
            if !pending.is_empty() {
                self.pending = Some(pending);
            }
            return;
        }
        if self.delete_selection() {
            return;
        }
        if self.cursor > 0 {
            // Between an empty pair of brackets, delete both.
            let around = self
                .buffer
                .bytes_in_range(self.cursor - 1..(self.cursor + 1).min(self.buffer.len()));
            if around.len() == 2 && auto_indent::closer_of(around[0]) == Some(around[1]) {
                self.commit(
                    self.cursor - 1..self.cursor + 1,
                    Vec::new(),
                    EditKind::Backspace,
                );
                return;
            }
        }
        let start = self
            .indentation_stop_before_cursor()
            .unwrap_or_else(|| self.prev_char(self.cursor));
        if start < self.cursor {
            self.commit(start..self.cursor, Vec::new(), EditKind::Backspace);
        }
    }

    /// Delete the selection, or the character after the cursor if nothing is
    /// selected. Pending indentation is discarded.
    pub fn delete_forward(&mut self) {
        self.drop_suggestion();
        self.pending = None;
        if !self.delete_selection() {
            let end = self.next_char(self.cursor);
            if end > self.cursor {
                self.commit(self.cursor..end, Vec::new(), EditKind::Delete);
            }
        }
        self.completion_wanted = true;
    }

    /// Delete the selected text. Returns whether anything was selected.
    pub fn delete_selection(&mut self) -> bool {
        match self.selection() {
            Some(range) => {
                self.commit(range, Vec::new(), EditKind::Other);
                true
            }
            None => false,
        }
    }

    /// The selected text, for the caller to place on the clipboard. Does
    /// not change the buffer. `None` if nothing is selected.
    pub fn copy(&self) -> Option<String> {
        self.selected_text()
    }

    /// Delete the selected text and return it, for the caller to place on
    /// the clipboard. `None` (and no change) if nothing is selected.
    pub fn cut(&mut self) -> Option<String> {
        let text = self.selected_text()?;
        self.delete_selection();
        Some(text)
    }

    /// Insert clipboard text at the cursor, replacing the selection if there
    /// is one. Line breaks are rewritten as the buffer's line ending.
    pub fn paste(&mut self, text: &str) {
        self.insert_text(text);
    }

    /// Replace the selection (or insert at the cursor) with `inserted`.
    fn replace_selection_with(&mut self, inserted: Vec<u8>, kind: EditKind) {
        let range = self.selection().unwrap_or(self.cursor..self.cursor);
        self.commit(range, inserted, kind);
    }

    /// Replace `range` with `inserted`, leaving the cursor after the inserted
    /// bytes with no selection, and record the change for undo.
    fn commit(&mut self, range: Range<usize>, inserted: Vec<u8>, kind: EditKind) {
        let cursor = range.start + inserted.len();
        self.commit_with_cursor(range, inserted, kind, cursor, None);
    }

    /// Replace `range` with `inserted`, leaving the cursor and selection
    /// anchor where the caller says, and record the change for undo. Any
    /// pending indentation is discarded; a caller that wants to keep it
    /// must set it again afterwards.
    fn commit_with_cursor(
        &mut self,
        range: Range<usize>,
        inserted: Vec<u8>,
        kind: EditKind,
        cursor: usize,
        anchor: Option<usize>,
    ) {
        self.pending = None;
        self.search = None;
        self.external = None;
        self.drop_suggestion();
        let before = self.state();
        let edit = Edit {
            offset: range.start,
            removed: self.buffer.bytes_in_range(range),
            inserted,
        };
        self.apply(&edit);
        self.cursor = cursor;
        self.anchor = anchor;
        self.desired_column = None;

        self.redo.clear();
        if self.save_point.is_some_and(|depth| depth > self.undo.len()) {
            // The saved state lived in the redo history we just discarded.
            self.save_point = None;
        }
        let after = self.state();
        let merged = self.grouping
            && kind != EditKind::Other
            && self
                .undo
                .last_mut()
                .is_some_and(|last| last.try_merge(&edit, kind, after));
        if !merged {
            self.undo.push(UndoEntry {
                before,
                after,
                edit,
                kind,
            });
        }
        self.grouping = kind != EditKind::Other;
        self.sync_modified();
    }

    /// Apply an edit's replacement to the buffer.
    fn apply(&mut self, edit: &Edit) {
        self.replace_bytes(edit.offset, edit.removed.len(), &edit.inserted);
    }

    /// Reverse an edit's replacement in the buffer.
    fn revert(&mut self, edit: &Edit) {
        self.replace_bytes(edit.offset, edit.inserted.len(), &edit.removed);
    }

    /// Replace `len` bytes at `offset` with `bytes`, keeping the
    /// highlighter in step. Every change to the buffer comes through here.
    fn replace_bytes(&mut self, offset: usize, len: usize, bytes: &[u8]) {
        let start = self.buffer.line_of_offset(offset);
        let old_end = self.buffer.line_of_offset(offset + len);
        self.buffer.delete(offset..offset + len);
        self.buffer.insert_bytes(offset, bytes);
        let new_end = self.buffer.line_of_offset(offset + bytes.len());
        self.highlighter.lines_changed(
            start,
            old_end - start + 1,
            new_end - start + 1,
            self.buffer.line_count(),
        );
    }

    fn state(&self) -> CursorState {
        CursorState {
            cursor: self.cursor,
            anchor: self.anchor,
        }
    }

    // ----- Indentation ----------------------------------------------------

    /// How lines are indented: guessed from the buffer's contents when the
    /// editor was created, or four spaces if they didn't say. Used by
    /// [`indent`](Self::indent), [`outdent`](Self::outdent), and
    /// [`backspace`](Self::backspace).
    pub fn indentation(&self) -> Indentation {
        self.indentation
    }

    pub fn set_indentation(&mut self, indentation: Indentation) {
        self.indentation = match indentation {
            Indentation::Spaces(n) => Indentation::Spaces(n.max(1)),
            Indentation::Tabs => Indentation::Tabs,
        };
    }

    /// The user's preferences for laying out code where the code itself
    /// doesn't say, such as how lines continuing inside brackets are
    /// indented. Defaults to [`CodeStyle::default`] until the frontend
    /// passes on the user's settings.
    pub fn code_style(&self) -> CodeStyle {
        self.code_style
    }

    pub fn set_code_style(&mut self, style: CodeStyle) {
        self.code_style = style;
    }

    /// Indentation the cursor is shown at but that hasn't been written to
    /// the buffer; see the [module documentation](self). `None` when there
    /// is none.
    pub fn pending_indentation(&self) -> Option<&str> {
        self.pending.as_deref()
    }

    /// Indent by one level. With pending indentation, adds a level to it.
    /// With a selection, indents every line it touches, keeping the
    /// selection. Otherwise inserts, at the cursor, whatever reaches the
    /// next indentation stop: a tab, or spaces up to the next multiple of
    /// the indentation width.
    pub fn indent(&mut self) {
        self.drop_suggestion();
        self.indent_inner();
        self.completion_wanted = true;
    }

    fn indent_inner(&mut self) {
        if let Some(mut pending) = self.pending.take() {
            let step = self.indent_step(self.indent_columns(&pending));
            pending.push_str(&step);
            self.pending = Some(pending);
            return;
        }
        match self.selection() {
            Some(range) => self.shift_lines(range, true),
            None => {
                let step = self.indent_step(self.cursor_position().column);
                self.commit(
                    self.cursor..self.cursor,
                    step.into_bytes(),
                    EditKind::Typing,
                );
            }
        }
    }

    /// Remove one level of indentation. With pending indentation, takes a
    /// level off it. Otherwise shifts every line the selection touches (or
    /// the cursor's line) back to the previous indentation stop, keeping
    /// the selection. Lines that aren't indented are left alone.
    pub fn outdent(&mut self) {
        self.drop_suggestion();
        if let Some(mut pending) = self.pending.take() {
            self.shrink_indent(&mut pending);
            if !pending.is_empty() {
                self.pending = Some(pending);
            }
        } else {
            let range = self.selection().unwrap_or(self.cursor..self.cursor);
            self.shift_lines(range, false);
        }
        self.completion_wanted = true;
    }

    /// The display width of a run of spaces and tabs starting at column 0.
    fn indent_columns(&self, indent: &str) -> usize {
        indent::columns(indent.as_bytes(), self.tab_width)
    }

    /// The text that takes indentation from `column` to the next stop.
    fn indent_step(&self, column: usize) -> String {
        match self.indentation {
            Indentation::Tabs => "\t".to_owned(),
            Indentation::Spaces(n) => " ".repeat(n - column % n),
        }
    }

    /// The number of bytes to drop from the end of `indent` to reach the
    /// previous indentation stop; see [`Indentation::outdent_len`].
    fn outdent_len(&self, indent: &[u8]) -> usize {
        self.indentation.outdent_len(indent, self.tab_width)
    }

    /// Take one level off a run of spaces and tabs.
    fn shrink_indent(&self, indent: &mut String) {
        let len = indent.len() - self.outdent_len(indent.as_bytes());
        indent.truncate(len);
    }

    /// The range of spaces and tabs at the start of a line's content.
    fn leading_whitespace(&self, line: usize) -> Range<usize> {
        let content = self.buffer.line_content_range(line);
        let len = self
            .buffer
            .bytes_in_range(content.clone())
            .iter()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count();
        content.start..content.start + len
    }

    /// In a space-indented buffer, when only spaces precede the cursor on
    /// its line, the offset of the previous indentation stop.
    fn indentation_stop_before_cursor(&self) -> Option<usize> {
        let Indentation::Spaces(width) = self.indentation else {
            return None;
        };
        let start = self
            .buffer
            .offset_of_line(self.buffer.line_of_offset(self.cursor));
        let spaces = self.cursor - start;
        if spaces == 0
            || !self
                .buffer
                .bytes_in_range(start..self.cursor)
                .iter()
                .all(|&b| b == b' ')
        {
            return None;
        }
        Some(self.cursor - ((spaces - 1) % width + 1))
    }

    /// Indent or outdent every line `range` touches, as one undoable edit,
    /// moving the cursor and anchor with the text they sit on. A range
    /// ending at the very start of a line doesn't touch that line. Blank
    /// lines are never indented, so they never gain trailing whitespace.
    fn shift_lines(&mut self, range: Range<usize>, indent: bool) {
        let first = self.buffer.line_of_offset(range.start);
        let mut last = self.buffer.line_of_offset(range.end);
        if last > first && range.end == self.buffer.offset_of_line(last) {
            last -= 1;
        }
        let start = self.buffer.offset_of_line(first);
        let end = self.buffer.line_content_range(last).end;
        let old = self.buffer.bytes_in_range(start..end);
        let unit = self.indentation.unit();
        let mut new = Vec::with_capacity(old.len() + (last - first + 1) * unit.len());
        // Each line's start and how many bytes were added (or, negative,
        // removed) there, for moving offsets to match.
        let mut changes: Vec<(usize, isize)> = Vec::new();
        for line in first..=last {
            let line_start = self.buffer.offset_of_line(line);
            let line_end = self.buffer.line_range(line).end.min(end);
            let content = &old[line_start - start..line_end - start];
            if indent {
                if !content.iter().all(u8::is_ascii_whitespace) {
                    new.extend_from_slice(unit.as_bytes());
                    changes.push((line_start, unit.len() as isize));
                }
                new.extend_from_slice(content);
            } else {
                let leading = content
                    .iter()
                    .take_while(|&&b| b == b' ' || b == b'\t')
                    .count();
                let removed = self.outdent_len(&content[..leading]);
                new.extend_from_slice(&content[removed..]);
                changes.push((line_start, -(removed as isize)));
            }
        }
        if new == old {
            return;
        }
        let moved = |offset: usize| -> usize {
            changes.iter().fold(offset, |moved, &(line_start, delta)| {
                if offset <= line_start {
                    moved
                } else if delta >= 0 {
                    moved + delta as usize
                } else {
                    moved - delta.unsigned_abs().min(offset - line_start)
                }
            })
        };
        let cursor = moved(self.cursor);
        let anchor = self.anchor.map(moved);
        self.commit_with_cursor(start..end, new, EditKind::Other, cursor, anchor);
    }

    fn restore(&mut self, state: CursorState) {
        self.cursor = state.cursor;
        self.anchor = state.anchor;
        self.end_movement();
    }

    // ----- Code completion ------------------------------------------------

    /// The completion proposed at the cursor, for a frontend to show as
    /// ghost text: what would be inserted there, with `\n` line breaks.
    /// `None` when there is none. Never empty.
    pub fn suggestion(&self) -> Option<&str> {
        self.suggestion.as_ref().map(|s| s.text.as_str())
    }

    pub fn has_suggestion(&self) -> bool {
        self.suggestion.is_some()
    }

    /// Whether the next [`accept_suggestion`](Self::accept_suggestion)
    /// takes all of the suggestion rather than its first line, because
    /// the last one took a line and nothing has been typed since.
    pub fn suggestion_continues(&self) -> bool {
        self.suggestion.as_ref().is_some_and(|s| s.accepted_line)
    }

    /// Drop the suggestion without asking for another. Returns whether
    /// there was one.
    pub fn dismiss_suggestion(&mut self) -> bool {
        let had = self.suggestion.is_some();
        self.drop_suggestion();
        had
    }

    /// Take the suggestion into the buffer: its first line, or, straight
    /// after a first line was taken, all the rest (see the [module
    /// documentation](self)). A suggestion that begins with line breaks
    /// counts them as part of its first line, since taking nothing
    /// would be no use. Returns whether there was a suggestion. Taking
    /// the last of it asks for the next completion.
    ///
    /// Text taken that ends with an opening bracket has it paired as
    /// typing it would, and the rest of the suggestion is confined to
    /// the new scope, so a second Tab takes the body of a block and
    /// leaves the paired closing bracket after it. Taking the last of a
    /// suggestion cut short of a closing bracket the buffer has puts the
    /// whitespace the suggestion had before its own bracket in front of
    /// the buffer's, so that the bracket goes where it proposed.
    pub fn accept_suggestion(&mut self) -> bool {
        let Some(suggestion) = self.suggestion.take() else {
            return false;
        };
        let text = suggestion.text;
        let end = if suggestion.accepted_line {
            text.len()
        } else {
            first_line_end(&text)
        };
        let (taken, rest) = text.split_at(end);
        let pair = self.closer_to_pair(taken);
        let eol = self.buffer.eol().as_str().to_owned();
        let mut inserted = self.pending.take().unwrap_or_default();
        inserted.push_str(&normalize_line_endings(taken, &eol));
        let cursor = self.cursor + inserted.len();
        let mut replaced = self.cursor..self.cursor;
        if let Some(closer) = pair {
            inserted.push(closer as char);
        } else if rest.is_empty()
            && let Some(whitespace) = &suggestion.before_closer
            && let Some((at, b)) = self.next_text_after_cursor()
            && auto_indent::is_closer(b)
        {
            inserted.push_str(&normalize_line_endings(whitespace, &eol));
            replaced.end = at;
        }
        self.commit_with_cursor(
            replaced,
            inserted.into_bytes(),
            EditKind::Other,
            cursor,
            None,
        );
        self.suggestion = self.confine_to_scope(rest, suggestion.before_closer, true);
        if self.suggestion.is_none() {
            self.completion_wanted = true;
        }
        true
    }

    /// The closing bracket to pair with `taken`, text about to be
    /// inserted at the cursor, when it ends with an opening bracket in
    /// code that typing would pair.
    fn closer_to_pair(&self, taken: &str) -> Option<u8> {
        let last = taken.len().checked_sub(1)?;
        let closer = auto_indent::closer_of(taken.as_bytes()[last])?;
        let next = self
            .buffer
            .bytes_in_range(self.cursor..(self.cursor + 1).min(self.buffer.len()))
            .first()
            .copied();
        let in_code = self
            .brackets_of_insertion(taken)
            .last()
            .is_some_and(|&(at, _)| at == last);
        (pairs_before(next) && in_code).then_some(closer)
    }

    /// Whether the user has typed since a completion was last asked for,
    /// so that [`take_completion_request`](Self::take_completion_request)
    /// has one to give.
    pub fn completion_wanted(&self) -> bool {
        self.completion_wanted
    }

    /// The request for the completion typing has made wanted, if any,
    /// for the frontend to send to the [`Completer`](crate::Completer).
    /// Taking it settles the want; the answer is expected back through
    /// [`offer_completion`](Self::offer_completion) with the request's
    /// serial. Nothing is asked for while text is selected.
    pub fn take_completion_request(&mut self) -> Option<CompletionRequest> {
        if !self.completion_wanted {
            return None;
        }
        self.completion_wanted = false;
        if self.selection().is_some() {
            return None;
        }
        self.completion_serial = self.completion_serial.wrapping_add(1);
        let buffer = &self.buffer;
        let cursor = self.cursor;
        let line = buffer.line_of_offset(cursor);
        // The prefix opens with a line break (as the pieces the model was
        // trained on do) at most `COMPLETION_PREFIX_BYTES` back, or with
        // the start of the buffer.
        let prefix_start = if line == 0 || cursor <= COMPLETION_PREFIX_BYTES {
            0
        } else {
            let first = buffer.line_of_offset(cursor - COMPLETION_PREFIX_BYTES);
            buffer.line_content_range(first.min(line - 1)).end
        };
        let mut prefix = text_of(&buffer.bytes_in_range(prefix_start..cursor));
        if let Some(pending) = &self.pending {
            prefix.push_str(pending);
        }
        let last = (line + COMPLETION_SUFFIX_LINES).min(buffer.line_count() - 1);
        let suffix = text_of(&buffer.bytes_in_range(cursor..buffer.line_content_range(last).end));
        Some(CompletionRequest {
            serial: self.completion_serial,
            language: self.language(),
            prefix,
            prefix_start,
            suffix,
            indentation: self.indentation,
        })
    }

    /// The serial the latest request was given, and that an answer must
    /// carry to be taken; see [`offer_completion`](Self::offer_completion).
    pub fn completion_serial(&self) -> u64 {
        self.completion_serial
    }

    /// The answer to the request numbered `serial`: `text` is what the
    /// model proposes inserting at the cursor. Becomes the suggestion if
    /// the request is still the latest and the text says something;
    /// returns whether it did.
    ///
    /// A suggestion stays within the scope the cursor is in: it is cut
    /// at its first closing bracket that closes a bracket opened before
    /// the cursor. When the buffer closes that bracket next (after
    /// nothing but whitespace), as it does after typing the bracket
    /// paired, the suggestion ends before its own closing bracket, which
    /// would double it; otherwise it ends with it. Brackets in strings
    /// and comments don't count.
    ///
    /// A suggestion whose first line ends by opening a block that Tab
    /// would pair (see [`accept_suggestion`](Self::accept_suggestion))
    /// ends with that block: once the first line is taken, the rest is
    /// confined to the block, so anything after it would be shown but
    /// never taken.
    pub fn offer_completion(&mut self, serial: u64, text: &str) -> bool {
        if serial != self.completion_serial || self.selection().is_some() {
            return false;
        }
        let text = text.replace('\r', "");
        self.suggestion = self.confine_to_scope(&text, None, false);
        self.suggestion.is_some()
    }

    /// Forget the suggestion, and any answer still to come to the last
    /// request.
    fn drop_suggestion(&mut self) {
        self.suggestion = None;
        self.completion_serial = self.completion_serial.wrapping_add(1);
        self.completion_wanted = false;
    }

    /// `text`, proposed at the cursor, as a suggestion confined to the
    /// cursor's scope (see [`offer_completion`](Self::offer_completion)),
    /// or `None` if nothing is left of it. `before_closer` is what the
    /// suggestion `text` is the rest of had, kept if `text` doesn't reach
    /// the end of the scope.
    ///
    /// `accepted_line` is whether Tab has already taken a line, so that
    /// the next takes all the rest and there is no block to end with.
    fn confine_to_scope(
        &self,
        text: &str,
        before_closer: Option<String>,
        accepted_line: bool,
    ) -> Option<Suggestion> {
        let brackets = self.brackets_of_insertion(text);
        let (kept, end) = self.scope_end(text, &brackets);
        let mut kept = kept.trim_end();
        let mut before_closer = match end {
            ScopeEnd::Open => before_closer,
            ScopeEnd::Closed => None,
            ScopeEnd::BeforeCloser(whitespace) => Some(whitespace.to_owned()),
        };
        let first_end = first_line_end(kept);
        if !accepted_line
            && first_end < kept.len()
            && self.closer_to_pair(&kept[..first_end]).is_some()
            && let Some(end) = block_end(&brackets, first_end - 1).filter(|&end| end < kept.len())
        {
            kept = &kept[..=end];
            before_closer = None;
        }
        (!kept.is_empty()).then(|| Suggestion {
            text: kept.to_owned(),
            accepted_line,
            before_closer,
        })
    }

    /// The part of `text`, proposed at the cursor, within the cursor's
    /// scope, and how it ends. `brackets` are its brackets, as
    /// [`brackets_of_insertion`](Self::brackets_of_insertion) finds them.
    fn scope_end<'a>(&self, text: &'a str, brackets: &[(usize, u8)]) -> (&'a str, ScopeEnd<'a>) {
        let mut depth = 0usize;
        let mut closer = None;
        for &(at, b) in brackets {
            if auto_indent::closer_of(b).is_some() {
                depth += 1;
            } else if depth == 0 {
                closer = Some(at);
                break;
            } else {
                depth -= 1;
            }
        }
        let Some(closer) = closer else {
            return (text, ScopeEnd::Open);
        };
        let next = self.next_text_after_cursor().map(|(_, b)| b);
        if next == Some(text.as_bytes()[closer]) {
            let kept = text[..closer].trim_end();
            (kept, ScopeEnd::BeforeCloser(&text[kept.len()..closer]))
        } else {
            (&text[..=closer], ScopeEnd::Closed)
        }
    }

    /// The brackets in code in `text`, about to be inserted at the
    /// cursor, with their offsets in it, as the text before the cursor
    /// on its line (and pending indentation) would have them lexed.
    fn brackets_of_insertion(&self, text: &str) -> Vec<(usize, u8)> {
        let line = self.buffer.line_of_offset(self.cursor);
        let start = self.buffer.offset_of_line(line);
        let mut first = self.buffer.bytes_in_range(start..self.cursor);
        if let Some(pending) = &self.pending {
            first.extend_from_slice(pending.as_bytes());
        }
        let first_offset = first.len();
        let mut lines: Vec<Vec<u8>> = text.split('\n').map(|l| l.as_bytes().to_vec()).collect();
        first.append(&mut lines[0]);
        lines[0] = first;
        let contents: Vec<&[u8]> = lines.iter().map(Vec::as_slice).collect();
        let tokens = self
            .highlighter
            .tokens_of_lines(&self.buffer, line, &contents);
        let mut brackets = Vec::new();
        let mut at = 0; // offset in `text` of the current line
        for (n, (content, tokens)) in lines.into_iter().zip(&tokens).enumerate() {
            let skip = if n == 0 { first_offset } else { 0 };
            let code_line = CodeLine::new(content, tokens);
            for i in skip..code_line.text.len() {
                let b = code_line.text[i];
                let bracket = auto_indent::closer_of(b).is_some() || auto_indent::is_closer(b);
                if bracket && code_line.code[i] {
                    brackets.push((at + i - skip, b));
                }
            }
            at += code_line.text.len() - skip + 1;
        }
        brackets
    }

    /// The offset and byte of the first text after the cursor other than
    /// whitespace, within the lines a completion request's suffix covers.
    fn next_text_after_cursor(&self) -> Option<(usize, u8)> {
        let line = self.buffer.line_of_offset(self.cursor);
        let last = (line + COMPLETION_SUFFIX_LINES).min(self.buffer.line_count() - 1);
        self.buffer
            .bytes_in_range(self.cursor..self.buffer.line_content_range(last).end)
            .into_iter()
            .enumerate()
            .find(|(_, b)| !b.is_ascii_whitespace())
            .map(|(i, b)| (self.cursor + i, b))
    }

    /// What is left of a suggestion after typing part of it: kept as the
    /// suggestion, confined to the scope the cursor is now in, or, when
    /// it has all been typed, gone, with the next completion asked for.
    fn keep_suggestion(&mut self, rest: &str, before_closer: Option<String>) {
        self.suggestion = self.confine_to_scope(rest, before_closer, false);
        if self.suggestion.is_none() {
            self.completion_wanted = true;
        }
    }

    /// After a line break typed along with a suggestion: `rest` is the
    /// suggestion from its next line on. The line's indentation in the
    /// suggestion becomes the pending indentation of the new line (in
    /// place of the one copied from the line above), so the cursor
    /// shows where the suggestion goes on; where the line break split a
    /// line and indentation was written out, the suggestion has to
    /// agree with it.
    fn continue_suggestion_on_next_line(&mut self, rest: &str, before_closer: Option<String>) {
        let indent_len = rest
            .bytes()
            .take_while(|&b| b == b' ' || b == b'\t')
            .count();
        let (indent, body) = rest.split_at(indent_len);
        let line_start = self
            .buffer
            .offset_of_line(self.buffer.line_of_offset(self.cursor));
        if self.pending.is_some() || self.cursor == line_start {
            self.pending = (!indent.is_empty()).then(|| indent.to_owned());
            self.keep_suggestion(body, before_closer);
            return;
        }
        let written = self.buffer.bytes_in_range(line_start..self.cursor);
        match rest.as_bytes().strip_prefix(written.as_slice()) {
            Some(_) => self.keep_suggestion(&rest[written.len()..], before_closer),
            None => self.completion_wanted = true,
        }
    }

    // ----- Undo -----------------------------------------------------------

    /// Undo the most recent edit, restoring the cursor and selection from
    /// before it. Returns whether there was anything to undo.
    pub fn undo(&mut self) -> bool {
        let Some(entry) = self.undo.pop() else {
            return false;
        };
        self.external = None;
        self.revert(&entry.edit);
        self.restore(entry.before);
        self.redo.push(entry);
        self.sync_modified();
        true
    }

    /// Redo the most recently undone edit, restoring the cursor and
    /// selection from after it. Returns whether there was anything to redo.
    pub fn redo(&mut self) -> bool {
        let Some(entry) = self.redo.pop() else {
            return false;
        };
        self.external = None;
        self.apply(&entry.edit);
        self.restore(entry.after);
        self.undo.push(entry);
        self.sync_modified();
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    // ----- Files ----------------------------------------------------------

    /// Whether the buffer differs from the file it was loaded from or last
    /// saved to. Undoing back to the saved state clears this.
    pub fn is_modified(&self) -> bool {
        self.save_point != Some(self.undo.len())
    }

    fn sync_modified(&mut self) {
        let modified = self.is_modified();
        self.buffer.set_modified(modified);
    }

    fn mark_saved(&mut self) {
        self.save_point = Some(self.undo.len());
        self.external = None;
        self.grouping = false;
        self.sync_modified();
    }

    /// Save to the buffer's associated file. Fails if it has none.
    pub fn save(&mut self) -> io::Result<()> {
        self.buffer.save()?;
        self.mark_saved();
        Ok(())
    }

    /// Save to a file, which becomes the buffer's associated file. The
    /// language is chosen afresh from the new name.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        self.buffer.save_as(path)?;
        self.mark_saved();
        self.set_language(
            self.buffer
                .path()
                .and_then(Language::from_path)
                .unwrap_or(Language::Plain),
        );
        Ok(())
    }

    /// See whether something else has changed the buffer's file since it
    /// was read or saved, and if so bring the change into the buffer; see
    /// the [module documentation](self) and [`ExternalChange`]. Meant to be
    /// called periodically. An error reading the file leaves the buffer
    /// as it was.
    pub fn check_disk(&mut self) -> io::Result<ExternalChange> {
        let (previous, contents) = match self.buffer.check_disk()? {
            DiskChange::Unchanged => return Ok(ExternalChange::None),
            DiskChange::Missing => {
                // The buffer is now the only copy: saving must write it
                // even if nothing is edited.
                self.save_point = None;
                self.grouping = false;
                self.sync_modified();
                return Ok(ExternalChange::Deleted);
            }
            DiskChange::Changed { previous, contents } => (previous, contents),
        };
        // A change on top of an earlier one that nothing has been edited
        // since replaces it: the earlier one is taken back out and the
        // merge redone from the same ancestor, so that the buffer is
        // always the user's version merged with the latest file, a
        // conflict is one block rather than a conflict inside a conflict,
        // and one undo still gets back to the user's version. This is the
        // editor revising its own change, not an undo: the user's undo
        // (see [`undo`](Self::undo)) drops `external`, so a change the
        // user has undone stays undone and counts as their edit.
        let previous = match self.external.take() {
            Some(external)
                if external.version == self.buffer.version()
                    && external.depth == self.undo.len() =>
            {
                self.take_back_last();
                external.base
            }
            _ => previous,
        };
        let remember = |editor: &mut Editor, base: Option<BufferSnapshot>| {
            editor.external = Some(External {
                base,
                version: editor.buffer.version(),
                depth: editor.undo.len(),
            });
        };
        let base = previous
            .as_ref()
            .map(BufferSnapshot::to_bytes)
            .unwrap_or_default();
        let ours = self.buffer.to_bytes();
        // Whether the user has anything of their own is judged against
        // the ancestor, not the modified flag: after a take-back the flag
        // can't tell, and a merge that happened to match the file left
        // the buffer unmodified while the user's edit is still theirs.
        if ours == base {
            if self.replace_all(contents, None) {
                remember(self, previous);
            }
            self.mark_saved_keeping_external();
            return Ok(ExternalChange::Reloaded);
        }
        let Some(merged) = merge::merge(&base, &ours, &contents) else {
            return Ok(ExternalChange::Unmerged);
        };
        let conflict = merged
            .conflicts
            .then(|| merge::first_conflict(&merged.content))
            .flatten();
        let saved = merged.content == contents;
        if self.replace_all(merged.content, conflict) {
            remember(self, previous);
        }
        if saved {
            self.mark_saved_keeping_external();
        }
        Ok(if merged.conflicts {
            ExternalChange::Conflicted
        } else {
            ExternalChange::Merged
        })
    }

    /// Throw away unsaved edits by reloading the buffer's file, as one
    /// undoable edit. Returns whether there was anything to discard; fails
    /// if the buffer has no file or it can't be read.
    pub fn discard_changes(&mut self) -> io::Result<bool> {
        if let DiskChange::Missing = self.buffer.check_disk()? {
            // As in `check_disk`: the buffer is the only copy now.
            self.save_point = None;
            self.sync_modified();
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the file no longer exists",
            ));
        }
        let Some(snapshot) = self.buffer.disk_snapshot() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "buffer has no associated file",
            ));
        };
        let contents = snapshot.to_bytes();
        if contents == self.buffer.to_bytes() {
            self.mark_saved();
            return Ok(false);
        }
        self.replace_all(contents, None);
        self.mark_saved();
        Ok(true)
    }

    /// Replace the whole contents as one undoable edit, keeping the cursor
    /// on the line it was on (as the diff between old and new says) unless
    /// `cursor` names where to put it instead. Any selection is dropped.
    /// Returns whether anything changed; nothing is recorded if not.
    fn replace_all(&mut self, new: Vec<u8>, cursor: Option<usize>) -> bool {
        let old = self.buffer.to_bytes();
        if old == new {
            return false;
        }
        let line = self.buffer.line_of_offset(self.cursor);
        let column = self.cursor - self.buffer.offset_of_line(line);
        let map = merge::LineMap::new(&old, &new);
        // Record only the part that differs, so undo doesn't hold two
        // copies of the file for a small change.
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let range = prefix..old.len() - suffix;
        let inserted = new[prefix..new.len() - suffix].to_vec();
        self.commit_with_cursor(range, inserted, EditKind::Other, prefix, None);
        let cursor = cursor.unwrap_or_else(|| {
            let line = map.map(line).min(self.buffer.line_count() - 1);
            let content = self.buffer.line_content_range(line);
            content.start + column.min(content.len())
        });
        self.cursor = self.snap(cursor);
        if let Some(entry) = self.undo.last_mut() {
            entry.after = CursorState {
                cursor: self.cursor,
                anchor: None,
            };
        }
        true
    }

    /// Take the newest undo entry back out of the buffer and the history,
    /// as if it had never happened. Only for a change the editor made on
    /// its own (one brought in from disk) that it is about to make
    /// differently. The user's undo is [`undo`](Self::undo), which keeps
    /// the entry for redo and counts as an edit.
    fn take_back_last(&mut self) {
        let Some(entry) = self.undo.pop() else {
            return;
        };
        self.revert(&entry.edit);
        self.restore(entry.before);
        if self.save_point.is_some_and(|depth| depth > self.undo.len()) {
            self.save_point = None;
        }
        self.sync_modified();
    }

    /// As [`mark_saved`](Self::mark_saved), for a change from disk that
    /// left the buffer matching the file: the change stays remembered
    /// so a further one can replace it.
    fn mark_saved_keeping_external(&mut self) {
        let external = self.external.take();
        self.mark_saved();
        self.external = external;
    }
}

/// Text from the buffer for the completion model: lossily decoded, with
/// `\n` line breaks.
fn text_of(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.into_owned()
    }
}

/// Where the first line of a suggestion ends: at its first line break
/// after any it begins with, or at its end.
/// The offset of the closing bracket that matches the opening one at
/// `opener`, among `brackets` (offsets and bytes, in order).
fn block_end(brackets: &[(usize, u8)], opener: usize) -> Option<usize> {
    let mut depth = 0usize;
    for &(at, b) in brackets.iter().skip_while(|&&(at, _)| at < opener) {
        if auto_indent::closer_of(b).is_some() {
            depth += 1;
        } else {
            depth -= 1;
            if depth == 0 {
                return Some(at);
            }
        }
    }
    None
}

/// Whether an opening bracket is paired when `next` follows it: at the
/// end of a line, or before whitespace or the end of an expression, but
/// not right before other text.
fn pairs_before(next: Option<u8>) -> bool {
    next.is_none_or(|b| b.is_ascii_whitespace() || matches!(b, b')' | b']' | b'}' | b',' | b';'))
}

fn first_line_end(text: &str) -> usize {
    let skipped = text.len() - text.trim_start_matches('\n').len();
    text[skipped..]
        .find('\n')
        .map_or(text.len(), |i| skipped + i)
}

/// A change brought in from disk that nothing has been edited on top of,
/// so that a further change from disk can replace it instead of piling
/// on it; see [`Editor::check_disk`]. Dropped by any edit, undo, redo,
/// or save.
struct External {
    /// The file as the buffer last loaded or saved it before the change:
    /// the ancestor the merge was done from, kept so that the next is
    /// done from the same one. `None` when the file didn't exist then.
    base: Option<BufferSnapshot>,
    /// The buffer version just after the change was brought in; any
    /// edit, undo, or redo since moves it on.
    version: u64,
    /// The undo depth just after, so the entry to take back is known to
    /// be the change's own.
    depth: usize,
}

/// The outcome of [`Editor::next_conflict`] or
/// [`Editor::previous_conflict`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictStep {
    /// The cursor moved to the next conflict in the direction asked.
    Moved,
    /// There was no conflict in that direction, so the cursor wrapped
    /// around the buffer to the first (or last) one, which may be the
    /// conflict it was already on.
    Wrapped,
    /// The buffer has no conflict markers; the cursor stayed put.
    NoConflicts,
}

/// What [`Editor::check_disk`] found and did about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalChange {
    /// The file is as the buffer last read or saved it.
    None,
    /// The file changed and the buffer had no unsaved edits, so it now
    /// holds the file's new contents and counts as unmodified.
    Reloaded,
    /// The file changed and the buffer's unsaved edits have been merged
    /// with the file's changes. The buffer is modified.
    Merged,
    /// As [`Merged`](Self::Merged), but some changes overlapped: the
    /// buffer holds conflict markers around them, and the cursor is on
    /// the first.
    Conflicted,
    /// The file changed but couldn't be merged with the buffer's unsaved
    /// edits (one of them is binary). The buffer is left alone; saving
    /// overwrites the file.
    Unmerged,
    /// The file was deleted or moved away. The buffer keeps its contents
    /// and is modified; saving recreates the file.
    Deleted,
}

impl UndoEntry {
    /// Try to absorb `edit` into this entry, which is possible when both are
    /// the same kind of single-character edit and `edit` continues where
    /// this entry left off. On success the entry's `after` state is updated.
    fn try_merge(&mut self, edit: &Edit, kind: EditKind, after: CursorState) -> bool {
        if self.kind != kind {
            return false;
        }
        let last = &mut self.edit;
        let mergeable = match kind {
            EditKind::Typing => {
                let contiguous =
                    edit.removed.is_empty() && edit.offset == last.offset + last.inserted.len();
                // Start a new entry when a word begins after whitespace, so
                // undo removes one word at a time.
                let starts_word = last
                    .inserted
                    .last()
                    .is_some_and(|b| b.is_ascii_whitespace())
                    && edit
                        .inserted
                        .first()
                        .is_some_and(|b| !b.is_ascii_whitespace());
                contiguous && !starts_word
            }
            EditKind::Backspace => {
                edit.inserted.is_empty() && edit.offset + edit.removed.len() == last.offset
            }
            EditKind::Delete => edit.inserted.is_empty() && edit.offset == last.offset,
            EditKind::Other => false,
        };
        if !mergeable {
            return false;
        }
        match kind {
            EditKind::Typing => last.inserted.extend_from_slice(&edit.inserted),
            EditKind::Backspace => {
                last.removed.splice(0..0, edit.removed.iter().copied());
                last.offset = edit.offset;
            }
            EditKind::Delete => last.removed.extend_from_slice(&edit.removed),
            EditKind::Other => unreachable!(),
        }
        self.after = after;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Movement::*;
    use SearchStep::*;

    fn editor(text: &str) -> Editor {
        Editor::new(FileBuffer::from_text(text))
    }

    fn text(editor: &Editor) -> String {
        editor.buffer().to_text()
    }

    fn type_str(editor: &mut Editor, s: &str) {
        for c in s.chars() {
            editor.insert_char(c);
        }
    }

    #[test]
    fn go_to_line_moves_to_the_line_start_and_clamps() {
        let mut ed = editor("one\ntwo\nthree\n");
        ed.set_selection(1, 6);
        ed.go_to_line(1);
        assert_eq!(ed.cursor_position(), Position { line: 1, column: 0 });
        assert_eq!(ed.selection(), None);
        ed.go_to_line(0);
        assert_eq!(ed.cursor(), 0);
        // Past the end is the last line: the empty one after the final
        // line break.
        ed.go_to_line(1000);
        assert_eq!(ed.cursor_position(), Position { line: 3, column: 0 });
        assert_eq!(ed.cursor(), 14);
        let mut ed = editor("no newline");
        ed.go_to_line(5);
        assert_eq!(ed.cursor(), 0);
    }

    #[test]
    fn search_previews_then_selects_and_steps() {
        let mut ed = editor("foo bar\nfoo baz\nfoo\n");
        ed.set_cursor(5);
        ed.start_search();
        ed.set_search_query("foo");
        let search = ed.search().unwrap();
        search.wait();
        // Previewing: the first match after the cursor is current, but
        // nothing is selected yet.
        assert_eq!(search.current(), Some(8..11));
        assert_eq!(search.match_count(), 3);
        assert!(!search.is_accepted());
        assert_eq!(ed.cursor(), 5);
        assert!(ed.selection().is_none());
        // Stepping while previewing moves only the current match.
        assert_eq!(ed.next_match(), Moved(16..19));
        assert!(ed.selection().is_none());

        assert!(ed.accept_search());
        assert_eq!(ed.selection(), Some(16..19));
        assert!(ed.search().unwrap().is_accepted());
        assert_eq!(ed.next_match(), Moved(0..3));
        assert_eq!(ed.selection(), Some(0..3));
        assert_eq!(ed.next_match(), ReachedStart);
        assert_eq!(ed.selection(), Some(0..3));
        assert!(
            ed.search().is_some(),
            "arriving at the start keeps the search"
        );
        assert_eq!(ed.next_match(), Moved(8..11));
        assert_eq!(ed.selection(), Some(8..11));

        // Any other cursor movement or edit drops the search.
        ed.move_cursor(Right);
        assert!(ed.search().is_none());
        assert_eq!(ed.next_match(), NoMatches);
        ed.start_search();
        ed.set_search_query("bar");
        assert!(ed.accept_search());
        assert_eq!(ed.selected_text().as_deref(), Some("bar"));
        type_str(&mut ed, "x");
        assert!(ed.search().is_none());
        assert_eq!(text(&ed), "foo x\nfoo baz\nfoo\n");
        ed.start_search();
        ed.set_search_query("baz");
        assert!(ed.accept_search());
        ed.undo();
        assert!(ed.search().is_none());
        ed.start_search();
        ed.set_search_query("baz");
        assert!(ed.accept_search());
        ed.clear_selection();
        assert!(ed.search().is_none());
    }

    #[test]
    fn search_without_matches_or_query() {
        let mut ed = editor("abc\n");
        assert!(!ed.accept_search());
        ed.set_search_query("a");
        assert!(ed.search().is_none(), "no search to set a query on");
        ed.start_search();
        assert!(!ed.accept_search(), "empty query");
        ed.set_search_query("zzz");
        assert!(!ed.accept_search());
        assert!(ed.search().is_some(), "a failed accept keeps the search");
        assert_eq!(ed.next_match(), NoMatches);
        ed.clear_search();
        assert!(ed.search().is_none());
        // A selection makes its start the origin, so a selected occurrence
        // is itself the first match.
        let mut ed = editor("ab ab ab");
        ed.set_selection(3, 5);
        ed.start_search();
        ed.set_search_query("ab");
        assert!(ed.accept_search());
        assert_eq!(ed.selection(), Some(3..5));
        assert_eq!(ed.search().unwrap().origin(), 3);
    }

    #[test]
    fn indentation_is_detected_or_defaults() {
        assert_eq!(editor("").indentation(), Indentation::Spaces(4));
        assert_eq!(editor("a\n\tb\n").indentation(), Indentation::Tabs);
        assert_eq!(editor("a\n  b\n").indentation(), Indentation::Spaces(2));
        let mut ed = editor("");
        ed.set_indentation(Indentation::Spaces(0));
        assert_eq!(ed.indentation(), Indentation::Spaces(1));
    }

    #[test]
    fn tab_inserts_to_the_next_indentation_stop() {
        let mut ed = editor("");
        ed.indent();
        assert_eq!(text(&ed), "    ");
        type_str(&mut ed, "ab");
        ed.indent();
        assert_eq!(text(&ed), "    ab  ", "spaces up to the next stop");

        let mut ed = editor("a\n  b\n");
        ed.indent();
        assert_eq!(text(&ed), "  a\n  b\n");

        let mut ed = editor("a\n\tb\n");
        ed.indent();
        assert_eq!(text(&ed), "\ta\n\tb\n");
        assert_eq!(ed.cursor(), 1);
    }

    #[test]
    fn backspace_removes_a_level_of_space_indentation() {
        let mut ed = editor("a\n    b\n      c\n\td");
        ed.set_indentation(Indentation::Spaces(4));
        ed.set_cursor(6);
        ed.backspace();
        assert_eq!(text(&ed), "a\nb\n      c\n\td");
        assert_eq!(ed.cursor(), 2);
        ed.set_cursor(10);
        ed.backspace();
        assert_eq!(
            text(&ed),
            "a\nb\n    c\n\td",
            "back to the previous stop, not a whole level"
        );
        ed.backspace();
        assert_eq!(text(&ed), "a\nb\nc\n\td");
        ed.undo();
        assert_eq!(
            text(&ed),
            "a\nb\n      c\n\td",
            "a run of backspaces is one undo step"
        );
        ed.set_cursor(14);
        ed.backspace();
        assert_eq!(
            text(&ed),
            "a\nb\n      c\n\t",
            "text before the cursor: one character"
        );
        ed.set_cursor(1);
        ed.backspace();
        assert_eq!(text(&ed), "\nb\n      c\n\t");

        let mut ed = editor("\ta\n    b");
        ed.set_indentation(Indentation::Tabs);
        ed.set_cursor(7);
        ed.backspace();
        assert_eq!(
            text(&ed),
            "\ta\n   b",
            "one space at a time in a tab-indented buffer"
        );
    }

    #[test]
    fn tab_with_a_selection_indents_lines() {
        let mut ed = editor("a\n  b\n\nc\nd\n");
        ed.set_selection(3, 8);
        ed.indent();
        assert_eq!(
            text(&ed),
            "a\n    b\n\n  c\nd\n",
            "the blank line stays blank; the line after the selection is untouched"
        );
        assert_eq!(ed.selection(), Some(5..12));
        assert_eq!(ed.anchor(), Some(5));
        assert_eq!(ed.selected_text().as_deref(), Some(" b\n\n  c"));
        ed.undo();
        assert_eq!(text(&ed), "a\n  b\n\nc\nd\n");
        assert_eq!(ed.selection(), Some(3..8));

        // A selection starting at column 0 keeps the new indentation
        // selected, and one ending at column 0 doesn't touch that line.
        ed.set_selection(2, 7);
        ed.indent();
        assert_eq!(text(&ed), "a\n    b\n\nc\nd\n");
        assert_eq!(ed.selection(), Some(2..9));
        assert_eq!(ed.selected_text().as_deref(), Some("    b\n\n"));
    }

    #[test]
    fn shift_tab_outdents_lines() {
        let mut ed = editor("a\n    b\n   c\nd\n\te\n");
        ed.set_indentation(Indentation::Spaces(2));
        ed.set_selection(4, 13);
        ed.outdent();
        assert_eq!(text(&ed), "a\n  b\n  c\nd\n\te\n");
        assert_eq!(ed.selection(), Some(2..10), "offsets move with their text");
        ed.outdent();
        assert_eq!(text(&ed), "a\nb\nc\nd\n\te\n");
        assert_eq!(ed.selection(), Some(2..6), "clamped to the line start");
        assert!(ed.can_undo());
        let depth = {
            let mut n = 0;
            while ed.undo() {
                n += 1;
            }
            n
        };
        assert_eq!(depth, 2);
        while ed.redo() {}

        ed.set_cursor(9);
        ed.outdent();
        assert_eq!(
            text(&ed),
            "a\nb\nc\nd\ne\n",
            "the cursor's line, without a selection"
        );
        assert_eq!(ed.cursor(), 8);
        ed.outdent();
        assert_eq!(text(&ed), "a\nb\nc\nd\ne\n");
        ed.undo();
        assert_eq!(
            text(&ed),
            "a\nb\nc\nd\n\te\n",
            "an outdent that changes nothing isn't an undo step"
        );
    }

    #[test]
    fn newline_carries_indentation_as_pending() {
        let mut ed = editor("    foo");
        ed.move_cursor(LineEnd);
        ed.insert_char('\n');
        assert_eq!(text(&ed), "    foo\n", "nothing written yet");
        assert_eq!(ed.pending_indentation(), Some("    "));
        assert_eq!(ed.cursor(), 8);
        assert_eq!(ed.cursor_position(), Position { line: 1, column: 4 });
        assert_eq!(ed.position_of_offset(8), Position { line: 1, column: 0 });

        ed.indent();
        assert_eq!(ed.pending_indentation(), Some("        "));
        assert_eq!(ed.cursor_position().column, 8);
        ed.outdent();
        ed.backspace();
        assert_eq!(ed.pending_indentation(), None);
        assert_eq!(
            text(&ed),
            "    foo\n",
            "adjusting pending indentation doesn't edit"
        );
        assert_eq!(ed.cursor_position().column, 0);
        ed.backspace();
        assert_eq!(text(&ed), "    foo", "then backspace joins the lines");

        ed.insert_char('\n');
        type_str(&mut ed, "b");
        assert_eq!(
            text(&ed),
            "    foo\n    b",
            "typing commits the indentation"
        );
        assert_eq!(ed.pending_indentation(), None);
        ed.undo();
        assert_eq!(
            text(&ed),
            "    foo\n",
            "committed indentation undoes with the text"
        );
        assert_eq!(ed.pending_indentation(), None);
    }

    #[test]
    fn newline_with_pending_indentation_leaves_a_blank_line() {
        let mut ed = editor("\tfoo");
        ed.move_cursor(LineEnd);
        ed.insert_char('\n');
        ed.insert_char('\n');
        assert_eq!(text(&ed), "\tfoo\n\n");
        assert_eq!(ed.pending_indentation(), Some("\t"));
        ed.indent();
        ed.insert_char('\n');
        assert_eq!(
            ed.pending_indentation(),
            Some("\t\t"),
            "carried on, with the extra level"
        );
        ed.paste("x");
        assert_eq!(text(&ed), "\tfoo\n\n\n\t\tx", "paste commits too");
    }

    #[test]
    fn pending_indentation_is_discarded_by_movement_and_edits() {
        let mut ed = editor("  foo\nbar");
        ed.set_cursor(5);
        ed.insert_char('\n');
        assert_eq!(ed.pending_indentation(), Some("  "));
        ed.move_cursor(Left);
        assert_eq!(ed.pending_indentation(), None);
        assert_eq!(ed.cursor(), 5);

        ed.insert_char('\n');
        ed.insert_char('\n');
        assert_eq!(text(&ed), "  foo\n\n\n\nbar");
        assert_eq!(ed.cursor_position(), Position { line: 2, column: 2 });
        ed.move_cursor(Down);
        assert_eq!(ed.pending_indentation(), None);
        assert_eq!(ed.cursor_position(), Position { line: 3, column: 0 });
        ed.move_cursor(Down);
        assert_eq!(
            ed.cursor_position(),
            Position { line: 4, column: 2 },
            "vertical movement aims for the shown column"
        );
        ed.move_cursor(Up);
        assert_eq!(ed.cursor_position(), Position { line: 3, column: 0 });

        ed.insert_char('\n');
        assert_eq!(
            ed.pending_indentation(),
            None,
            "an unindented line has none to carry"
        );
        ed.set_cursor(5);
        ed.insert_char('\n');
        ed.delete_forward();
        assert_eq!(ed.pending_indentation(), None);
        assert_eq!(ed.cursor_position().column, 0);
        ed.set_cursor(5);
        ed.insert_char('\n');
        ed.undo();
        assert_eq!(ed.pending_indentation(), None);
        ed.redo();
        assert_eq!(ed.pending_indentation(), None);
    }

    #[test]
    fn newline_in_the_middle_of_a_line_indents_immediately() {
        let mut ed = editor("    foo bar");
        ed.set_cursor(8);
        ed.insert_char('\n');
        assert_eq!(text(&ed), "    foo \n    bar");
        assert_eq!(ed.cursor(), 13);
        assert_eq!(ed.pending_indentation(), None);
        ed.undo();
        assert_eq!(text(&ed), "    foo bar");

        // Inside the leading whitespace, only the part before the cursor
        // is carried.
        ed.set_cursor(2);
        ed.insert_char('\n');
        assert_eq!(text(&ed), "  \n    foo bar");

        // The selection is replaced, and the indentation comes from the
        // line the selection starts on.
        let mut ed = editor("  a\n      b");
        ed.set_selection(2, 9);
        ed.insert_char('\n');
        assert_eq!(
            text(&ed),
            "  \n   b",
            "the rest of the line keeps its own spaces"
        );
        ed.undo();
        ed.set_selection(11, 2);
        ed.insert_char('\n');
        assert_eq!(text(&ed), "  \n");
        assert_eq!(ed.pending_indentation(), Some("  "));
    }

    #[test]
    fn highlighting_follows_edits() {
        let mut ed = editor("fn main() {\n    let x = 1;\n}\n");
        assert_eq!(ed.language(), Language::Plain);
        assert!(ed.line_cells(0).iter().all(|c| c.kind == TokenKind::Text));
        ed.set_language(Language::Rust);
        let kinds = |ed: &Editor, line: usize| -> Vec<TokenKind> {
            ed.line_cells(line).iter().map(|c| c.kind).collect()
        };
        assert_eq!(kinds(&ed, 0)[..2], [TokenKind::Keyword, TokenKind::Keyword]);
        assert_eq!(kinds(&ed, 0)[3], TokenKind::FunctionDefinition);
        assert_eq!(kinds(&ed, 1)[8], TokenKind::VariableDefinition);
        assert_eq!(kinds(&ed, 1)[12], TokenKind::Number);

        // Opening a comment at the top recolors the lines below, in step
        // with the edit; closing it with undo restores them.
        ed.set_cursor(0);
        type_str(&mut ed, "/* ");
        assert!(kinds(&ed, 1).iter().all(|&k| k == TokenKind::Comment));
        assert_eq!(
            ed.line_tokens(2),
            vec![Token {
                range: 0..1,
                kind: TokenKind::Comment
            }]
        );
        ed.undo();
        assert_eq!(kinds(&ed, 1)[12], TokenKind::Number);
        ed.redo();
        assert!(kinds(&ed, 2).iter().all(|&k| k == TokenKind::Comment));

        // Pasting and cutting lines keeps the line bookkeeping in step.
        ed.set_cursor(0);
        ed.paste("a\nb\nc\n");
        ed.set_selection(0, 4);
        ed.cut();
        ed.select_all();
        ed.insert_text("x = \"s\n");
        assert_eq!(
            ed.line_tokens(0)[2],
            Token {
                range: 4..6,
                kind: TokenKind::String
            }
        );
        assert!(ed.line_tokens(1).is_empty(), "empty line inside the string");
        ed.set_language(Language::Plain);
        assert!(ed.line_tokens(0).is_empty());
    }

    #[test]
    fn conflict_markers_in_any_language() {
        // Plain text, as a file of unknown kind is.
        let mut ed = editor("a\n<<<<<<< editor\nours\n=======\ntheirs\n>>>>>>> disk\nb\n");
        assert_eq!(ed.language(), Language::Plain);
        let sides: Vec<Option<ConflictSide>> = (0..7).map(|line| ed.conflict_side(line)).collect();
        use ConflictSide::*;
        assert_eq!(
            sides,
            [
                None,
                Some(Ours),
                Some(Ours),
                Some(Theirs),
                Some(Theirs),
                Some(Theirs),
                None
            ]
        );
        assert!(
            ed.line_cells(1)
                .iter()
                .all(|c| c.kind == TokenKind::ConflictMarker)
        );
        assert!(ed.line_cells(2).iter().all(|c| c.kind == TokenKind::Text));

        // Resolving the conflict by deleting its markers takes the
        // highlighting with it, in step with the edits.
        ed.set_selection(ed.buffer().offset_of_line(1), ed.buffer().offset_of_line(2));
        ed.cut();
        assert!((0..6).all(|line| ed.conflict_side(line).is_none()));
        assert!(ed.line_cells(2).iter().all(|c| c.kind == TokenKind::Text));
        ed.undo();
        assert_eq!(ed.conflict_side(4), Some(Theirs));
    }

    #[test]
    fn next_and_previous_conflict_wrap_around() {
        let mut ed = editor(
            "a\n<<<<<<< x\nb\n=======\nc\n>>>>>>> y\nd\n<<<<<<< x\ne\n=======\nf\n>>>>>>> y\ng\n",
        );
        let line = |ed: &Editor| ed.cursor_position().line;
        assert_eq!(ed.next_conflict(), ConflictStep::Moved);
        assert_eq!(line(&ed), 1);
        assert_eq!(ed.next_conflict(), ConflictStep::Moved);
        assert_eq!(line(&ed), 7);
        assert_eq!(ed.next_conflict(), ConflictStep::Wrapped);
        assert_eq!(line(&ed), 1);
        assert_eq!(ed.previous_conflict(), ConflictStep::Wrapped);
        assert_eq!(line(&ed), 7);
        assert_eq!(ed.previous_conflict(), ConflictStep::Moved);
        assert_eq!(line(&ed), 1);

        // From inside a conflict, "next" is the following one and
        // "previous" is the start of this one; the cursor lands at the
        // start of the marker line with no selection.
        ed.set_selection(22, 23);
        assert_eq!(line(&ed), 4);
        assert_eq!(ed.previous_conflict(), ConflictStep::Moved);
        assert_eq!(ed.cursor(), 2);
        assert_eq!(ed.anchor(), None);
        ed.go_to_line(4);
        assert_eq!(ed.next_conflict(), ConflictStep::Moved);
        assert_eq!(line(&ed), 7);

        // Markers with other shapes don't count, and without any the
        // cursor stays put.
        let mut ed = editor("<<<<<<<<\n <<<<<<< x\n=======\n>>>>>>> y\n");
        ed.go_to_line(2);
        assert_eq!(ed.next_conflict(), ConflictStep::NoConflicts);
        assert_eq!(ed.previous_conflict(), ConflictStep::NoConflicts);
        assert_eq!(line(&ed), 2);
        assert_eq!(editor("").next_conflict(), ConflictStep::NoConflicts);
    }
    #[test]
    fn character_movement_handles_utf8_and_crlf() {
        let mut ed = editor("aé\r\nb");
        let expected_right = [1, 3, 5, 6, 6];
        for &offset in &expected_right {
            ed.move_cursor(Right);
            assert_eq!(ed.cursor(), offset);
        }
        let expected_left = [5, 3, 1, 0, 0];
        for &offset in &expected_left {
            ed.move_cursor(Left);
            assert_eq!(ed.cursor(), offset);
        }
    }

    #[test]
    fn combining_marks_move_as_one_character() {
        // "e" + combining acute accent, then "x".
        let mut ed = editor("e\u{301}x");
        ed.move_cursor(Right);
        assert_eq!(ed.cursor(), 3, "past the whole cluster");
        assert_eq!(ed.cursor_position(), Position { line: 0, column: 1 });
        ed.move_cursor(Right);
        assert_eq!(ed.cursor(), 4);
        ed.move_cursor(Left);
        ed.move_cursor(Left);
        assert_eq!(ed.cursor(), 0);
        ed.set_cursor(1);
        assert_eq!(
            ed.cursor(),
            0,
            "between base and mark snaps to the cluster start"
        );
        ed.set_cursor(3);
        ed.backspace();
        assert_eq!(text(&ed), "x", "backspace removes the whole cluster");
        ed.undo();
        ed.set_cursor(0);
        ed.delete_forward();
        assert_eq!(text(&ed), "x", "delete removes the whole cluster");
        ed.undo();
        ed.extend_selection(WordRight);
        assert_eq!(
            ed.selected_text().as_deref(),
            Some("e\u{301}x"),
            "clusters join words"
        );
    }

    #[test]
    fn emoji_sequences_are_one_character() {
        let family = "👨\u{200d}👩\u{200d}👧";
        let flag = "🇺🇸";
        let mut ed = editor(&format!("{family}{flag}!"));
        ed.move_cursor(Right);
        assert_eq!(ed.cursor(), family.len());
        assert_eq!(ed.cursor_position().column, 2);
        ed.move_cursor(Right);
        assert_eq!(ed.cursor(), family.len() + flag.len());
        assert_eq!(ed.cursor_position().column, 4);
        ed.move_cursor(Left);
        assert_eq!(ed.cursor(), family.len());
    }

    #[test]
    fn wide_characters_take_two_columns() {
        let mut ed = editor("한글ab\nabcdef");
        ed.set_cursor(9 + 3);
        assert_eq!(ed.cursor_position(), Position { line: 1, column: 3 });
        ed.move_cursor(Up);
        // Column 3 is the middle of "글" (columns 2..4): land at its start.
        assert_eq!(ed.cursor(), 3);
        assert_eq!(ed.cursor_position(), Position { line: 0, column: 2 });
        ed.move_cursor(Down);
        assert_eq!(
            ed.cursor_position(),
            Position { line: 1, column: 3 },
            "desired column kept"
        );
        assert_eq!(ed.offset_of_position(Position { line: 0, column: 4 }), 6);
        assert_eq!(ed.offset_of_position(Position { line: 0, column: 5 }), 7);
        assert_eq!(ed.position_of_offset(6), Position { line: 0, column: 4 });
    }

    #[test]
    fn tabs_advance_to_the_next_tab_stop() {
        let mut ed = editor("\tx\n123456789\na\tb");
        assert_eq!(ed.tab_width(), DEFAULT_TAB_WIDTH);
        ed.move_cursor(Right);
        assert_eq!(ed.cursor_position(), Position { line: 0, column: 4 });
        ed.move_cursor(Down);
        assert_eq!(ed.cursor(), 3 + 4);
        ed.move_cursor(Down);
        assert_eq!(ed.cursor_position(), Position { line: 2, column: 4 });
        assert_eq!(
            ed.cursor(),
            3 + 10 + 2,
            "column 4 is past the tab ending at column 4"
        );

        ed.set_tab_width(8);
        assert_eq!(ed.position_of_offset(1), Position { line: 0, column: 8 });
        assert_eq!(
            ed.position_of_offset(3 + 10 + 2),
            Position { line: 2, column: 8 }
        );
        ed.set_tab_width(0);
        assert_eq!(ed.tab_width(), 1, "tab width is at least one");
    }

    #[test]
    fn invalid_utf8_bytes_are_single_characters() {
        let mut ed = Editor::new(FileBuffer::from_bytes(b"a\xff\xfeb"));
        for expected in [1, 2, 3, 4, 4] {
            ed.move_cursor(Right);
            assert_eq!(ed.cursor(), expected);
        }
        assert_eq!(ed.cursor_position().column, 4);
        ed.backspace();
        ed.backspace();
        assert_eq!(ed.buffer().to_bytes(), b"a\xff");
    }

    #[test]
    fn line_cells_lay_out_a_line() {
        let mut ed = editor("a\t한\r\n");
        let cells = ed.line_cells(0);
        let summary: Vec<_> = cells
            .iter()
            .map(|c| (c.range.clone(), c.text.as_str(), c.column, c.width))
            .collect();
        assert_eq!(
            summary,
            vec![(0..1, "a", 0, 1), (1..2, "\t", 1, 3), (2..5, "한", 4, 2)]
        );
        assert!(ed.line_cells(1).is_empty());
        assert_eq!(ed.line_width(0), 6);
        assert_eq!(ed.line_width(1), 0);

        ed.set_tab_width(2);
        assert_eq!(ed.line_cells(0)[1].width, 1);
        assert_eq!(ed.line_width(0), 4);

        let invalid = Editor::new(FileBuffer::from_bytes(b"\xff"));
        assert_eq!(invalid.line_cells(0)[0].text, "\u{FFFD}");
    }

    #[test]
    fn positions_round_trip() {
        let ed = editor("aé\r\nb\nlast");
        assert_eq!(ed.position_of_offset(3), Position { line: 0, column: 2 });
        assert_eq!(ed.position_of_offset(5), Position { line: 1, column: 0 });
        assert_eq!(ed.offset_of_position(Position { line: 0, column: 2 }), 3);
        assert_eq!(
            ed.offset_of_position(Position {
                line: 0,
                column: 99
            }),
            3,
            "clamps column"
        );
        assert_eq!(
            ed.offset_of_position(Position {
                line: 99,
                column: 1
            }),
            8,
            "clamps line"
        );
        assert_eq!(ed.cursor_position(), Position::default());
    }

    #[test]
    fn set_cursor_snaps_to_character_boundaries() {
        let mut ed = editor("aé\r\nb");
        ed.set_cursor(2);
        assert_eq!(ed.cursor(), 1, "inside a multi-byte character");
        ed.set_cursor(4);
        assert_eq!(ed.cursor(), 3, "inside a CRLF pair");
        ed.set_cursor(100);
        assert_eq!(ed.cursor(), 6, "past the end");
        ed.set_selection(100, 2);
        assert_eq!(ed.selection(), Some(1..6));
    }

    #[test]
    fn word_movement() {
        let mut ed = editor("foo bar_baz  (qux)\nnext");
        for &offset in &[3, 11, 14, 17, 18, 19, 23, 23] {
            ed.move_cursor(WordRight);
            assert_eq!(ed.cursor(), offset);
        }
        for &offset in &[19, 18, 17, 14, 13, 4, 0, 0] {
            ed.move_cursor(WordLeft);
            assert_eq!(ed.cursor(), offset);
        }
    }

    #[test]
    fn select_word_at_picks_the_run_under_the_offset() {
        let mut ed = editor("foo bar_baz  (qux)\r\n\nend");
        ed.select_word_at(5);
        assert_eq!(ed.selection(), Some(4..11));
        assert_eq!(ed.cursor(), 11);
        ed.select_word_at(12);
        assert_eq!(ed.selection(), Some(11..13), "a run of spaces");
        ed.select_word_at(13);
        assert_eq!(ed.selection(), Some(13..14), "punctuation");
        ed.select_word_at(18);
        assert_eq!(
            ed.selection(),
            Some(17..18),
            "the end of a line: the last word"
        );
        ed.select_word_at(19);
        assert_eq!(ed.selection(), Some(17..18), "inside the line ending");
        ed.select_word_at(20);
        assert_eq!(ed.selection(), None, "an empty line");
        assert_eq!(ed.cursor(), 20);
        ed.select_word_at(22);
        assert_eq!(ed.selection(), Some(21..24));
        ed.select_word_at(99);
        assert_eq!(ed.selection(), Some(21..24));
    }

    #[test]
    fn line_and_document_movement() {
        let mut ed = editor("first\r\nsecond");
        ed.set_cursor(2);
        ed.move_cursor(LineEnd);
        assert_eq!(ed.cursor(), 5, "before the terminator");
        ed.move_cursor(LineStart);
        assert_eq!(ed.cursor(), 0);
        ed.move_cursor(DocumentEnd);
        assert_eq!(ed.cursor(), 13);
        ed.move_cursor(DocumentStart);
        assert_eq!(ed.cursor(), 0);
    }

    #[test]
    fn line_start_alternates_between_indent_and_column_zero() {
        let mut ed = editor("    indented\n\t\t\nplain\n");
        // From inside the content: to the first non-whitespace character,
        // then to the first column, then back again.
        ed.set_cursor(10);
        ed.move_cursor(LineStart);
        assert_eq!(ed.cursor(), 4, "first to the first non-blank character");
        ed.move_cursor(LineStart);
        assert_eq!(ed.cursor(), 0, "then to the first column");
        ed.move_cursor(LineStart);
        assert_eq!(ed.cursor(), 4, "and from the first column back again");
        // From inside the indentation, the first column comes first.
        ed.set_cursor(2);
        ed.move_cursor(LineStart);
        assert_eq!(ed.cursor(), 0, "within the indentation: the first column");
        // A whitespace-only line only has a first column to go to.
        ed.set_cursor(15);
        ed.move_cursor(LineStart);
        assert_eq!(ed.cursor(), 13, "a blank line: the first column");
        ed.move_cursor(LineStart);
        assert_eq!(ed.cursor(), 13, "and it stays there");
        // A line without indentation behaves like a plain Home.
        ed.set_cursor(19);
        ed.move_cursor(LineStart);
        assert_eq!(ed.cursor(), 16, "no indentation: the first column");
        ed.move_cursor(LineStart);
        assert_eq!(ed.cursor(), 16, "and it stays there");
        // Extending a selection follows the same rule.
        ed.set_cursor(10);
        ed.extend_selection(LineStart);
        assert_eq!(ed.selection(), Some(4..10));
        ed.extend_selection(LineStart);
        assert_eq!(ed.selection(), Some(0..10));
    }

    #[test]
    fn vertical_movement_keeps_desired_column() {
        let mut ed = editor("long line here\nab\nlonger line\n");
        ed.set_cursor(10);
        ed.move_cursor(Down);
        assert_eq!(ed.cursor_position(), Position { line: 1, column: 2 });
        ed.move_cursor(Down);
        assert_eq!(
            ed.cursor_position(),
            Position {
                line: 2,
                column: 10
            }
        );
        ed.move_cursor(Down);
        assert_eq!(ed.cursor_position(), Position { line: 3, column: 0 });
        ed.move_cursor(Up);
        assert_eq!(
            ed.cursor_position(),
            Position {
                line: 2,
                column: 10
            }
        );
        ed.move_cursor(Up);
        ed.move_cursor(Up);
        assert_eq!(
            ed.cursor_position(),
            Position {
                line: 0,
                column: 10
            }
        );
        ed.move_cursor(Up);
        assert_eq!(ed.cursor(), 0, "up on the first line goes to the start");
        ed.move_cursor(DocumentEnd);
        ed.move_cursor(Down);
        assert_eq!(
            ed.cursor(),
            ed.buffer().len(),
            "down on the last line goes to the end"
        );

        // Horizontal movement resets the remembered column.
        ed.set_cursor(10);
        ed.move_cursor(Down);
        ed.move_cursor(Left);
        ed.move_cursor(Down);
        assert_eq!(ed.cursor_position(), Position { line: 2, column: 1 });
    }

    #[test]
    fn page_movement() {
        let lines: Vec<String> = (0..10).map(|i| format!("line {i}")).collect();
        let mut ed = editor(&lines.join("\n"));
        ed.set_cursor(3);
        ed.move_cursor(PageDown(4));
        assert_eq!(ed.cursor_position(), Position { line: 4, column: 3 });
        ed.move_cursor(PageDown(4));
        assert_eq!(ed.cursor_position(), Position { line: 8, column: 3 });
        ed.move_cursor(PageDown(4));
        assert_eq!(ed.cursor_position(), Position { line: 9, column: 3 });
        ed.move_cursor(PageDown(4));
        assert_eq!(ed.cursor(), ed.buffer().len());
        ed.move_cursor(PageUp(3));
        assert_eq!(ed.cursor_position(), Position { line: 6, column: 6 });
        ed.move_cursor(PageUp(100));
        assert_eq!(ed.cursor_position(), Position { line: 0, column: 6 });
    }

    #[test]
    fn selection_extends_and_collapses() {
        let mut ed = editor("hello world");
        for _ in 0..5 {
            ed.extend_selection(Right);
        }
        assert_eq!(ed.selection(), Some(0..5));
        assert_eq!(ed.selected_text().as_deref(), Some("hello"));
        assert_eq!(ed.anchor(), Some(0));
        assert_eq!(ed.cursor(), 5);

        ed.move_cursor(Left);
        assert_eq!(ed.cursor(), 0, "left collapses to the selection start");
        assert_eq!(ed.selection(), None);

        ed.extend_selection(WordRight);
        ed.move_cursor(Right);
        assert_eq!(ed.cursor(), 5, "right collapses to the selection end");
        assert_eq!(ed.selection(), None);

        ed.extend_selection(WordLeft);
        assert_eq!(ed.selection(), Some(0..5));
        ed.extend_selection(WordRight);
        assert_eq!(ed.selection(), None, "selection shrunk back to empty");
        assert_eq!(ed.anchor(), Some(5));
        ed.move_cursor(Up);
        assert_eq!(ed.anchor(), None, "plain movement clears the selection");

        ed.select_all();
        assert_eq!(ed.selection(), Some(0..11));
        ed.clear_selection();
        assert_eq!(ed.selection(), None);
        assert_eq!(ed.cursor(), 11);
    }

    #[test]
    fn typing_replaces_selection_and_undo_restores_it() {
        let mut ed = editor("hello world");
        ed.set_selection(0, 5);
        type_str(&mut ed, "Jay");
        assert_eq!(text(&ed), "Jay world");
        assert_eq!(ed.cursor(), 3);

        assert!(ed.undo());
        assert_eq!(text(&ed), "hello world");
        assert_eq!(ed.cursor(), 5);
        assert_eq!(ed.selection(), Some(0..5), "selection restored");
        assert!(!ed.undo(), "nothing left to undo");

        assert!(ed.redo());
        assert_eq!(text(&ed), "Jay world");
        assert_eq!(ed.cursor(), 3);
        assert_eq!(ed.selection(), None);
        assert!(!ed.redo());
    }

    #[test]
    fn typing_groups_by_word() {
        let mut ed = editor("");
        type_str(&mut ed, "ab cd");
        assert!(ed.undo());
        assert_eq!(text(&ed), "ab ");
        assert!(ed.undo());
        assert_eq!(text(&ed), "");
        assert!(ed.redo());
        assert_eq!(text(&ed), "ab ");
        assert_eq!(ed.cursor(), 3);
    }

    #[test]
    fn movement_and_newlines_break_typing_groups() {
        let mut ed = editor("");
        type_str(&mut ed, "ab");
        ed.move_cursor(Left);
        ed.move_cursor(Right);
        type_str(&mut ed, "cd");
        ed.insert_char('\n');
        type_str(&mut ed, "ef");
        assert_eq!(text(&ed), "abcd\nef");
        ed.undo();
        assert_eq!(text(&ed), "abcd\n");
        ed.undo();
        assert_eq!(text(&ed), "abcd");
        ed.undo();
        assert_eq!(text(&ed), "ab");
        ed.undo();
        assert_eq!(text(&ed), "");
    }

    #[test]
    fn newline_uses_buffer_line_ending() {
        let mut ed = editor("a\r\nb");
        ed.move_cursor(Right);
        ed.insert_char('\n');
        assert_eq!(text(&ed), "a\r\n\r\nb");
        assert_eq!(ed.cursor(), 3);
        ed.backspace();
        assert_eq!(
            text(&ed),
            "a\r\nb",
            "backspace removes the whole terminator"
        );
        assert_eq!(ed.cursor(), 1);
        ed.delete_forward();
        assert_eq!(text(&ed), "ab", "delete removes the whole terminator");
        ed.undo();
        assert_eq!(text(&ed), "a\r\nb");
    }

    #[test]
    fn backspace_and_delete_group_and_stop_at_edges() {
        let mut ed = editor("héllo");
        ed.move_cursor(DocumentEnd);
        ed.backspace();
        ed.backspace();
        ed.backspace();
        assert_eq!(text(&ed), "hé");
        ed.backspace();
        assert_eq!(
            text(&ed),
            "h",
            "backspace removes a whole multi-byte character"
        );
        assert!(ed.undo());
        assert_eq!(text(&ed), "héllo", "one undo restores the whole run");
        assert_eq!(ed.cursor(), 6);

        ed.move_cursor(DocumentStart);
        ed.backspace();
        assert_eq!(text(&ed), "héllo", "backspace at the start does nothing");
        assert!(!ed.can_undo());

        ed.delete_forward();
        ed.delete_forward();
        assert_eq!(text(&ed), "llo");
        assert_eq!(ed.cursor(), 0);
        ed.move_cursor(DocumentEnd);
        ed.delete_forward();
        assert_eq!(text(&ed), "llo", "delete at the end does nothing");
        assert!(ed.undo());
        assert_eq!(text(&ed), "héllo");
        assert!(!ed.can_undo());
    }

    #[test]
    fn selection_deletion_is_one_undo_step() {
        let mut ed = editor("hello world");
        ed.set_selection(6, 11);
        ed.backspace();
        assert_eq!(text(&ed), "hello ");
        ed.set_selection(0, 5);
        ed.delete_forward();
        assert_eq!(text(&ed), " ");
        assert!(!ed.delete_selection());
        ed.undo();
        assert_eq!(text(&ed), "hello ");
        assert_eq!(ed.selection(), Some(0..5));
        ed.undo();
        assert_eq!(text(&ed), "hello world");
        assert_eq!(ed.selection(), Some(6..11));
        assert_eq!(ed.cursor(), 11);
    }

    #[test]
    fn clipboard_operations() {
        let mut ed = editor("hello world");
        assert_eq!(ed.copy(), None);
        assert_eq!(ed.cut(), None);
        assert!(!ed.can_undo());

        ed.set_selection(6, 11);
        assert_eq!(ed.copy().as_deref(), Some("world"));
        assert_eq!(text(&ed), "hello world", "copy doesn't change the buffer");
        assert!(!ed.can_undo(), "copy isn't an undoable edit");

        assert_eq!(ed.cut().as_deref(), Some("world"));
        assert_eq!(text(&ed), "hello ");
        assert_eq!(ed.cursor(), 6);

        ed.move_cursor(DocumentStart);
        ed.paste("world ");
        assert_eq!(text(&ed), "world hello ");
        assert_eq!(ed.cursor(), 6);

        // Undo is independent of the clipboard: the cut text comes back even
        // though the caller's clipboard would now hold something else.
        ed.undo();
        assert_eq!(text(&ed), "hello ");
        assert_eq!(ed.cursor(), 0);
        ed.undo();
        assert_eq!(text(&ed), "hello world");
        assert_eq!(ed.selection(), Some(6..11));
        ed.redo();
        ed.redo();
        assert_eq!(text(&ed), "world hello ");
    }

    #[test]
    fn paste_normalizes_line_endings() {
        let mut ed = editor("a\r\nb");
        ed.move_cursor(DocumentEnd);
        ed.paste("x\ny\r\nz\r");
        assert_eq!(text(&ed), "a\r\nbx\r\ny\r\nz\r\n");
        ed.paste("");
        assert!(ed.can_undo());
        ed.undo();
        assert_eq!(text(&ed), "a\r\nb", "an empty paste is not an undo step");
    }

    #[test]
    fn paste_replaces_selection() {
        let mut ed = editor("hello world");
        ed.select_all();
        ed.paste("bye");
        assert_eq!(text(&ed), "bye");
        ed.undo();
        assert_eq!(text(&ed), "hello world");
        assert_eq!(ed.selection(), Some(0..11));
    }

    #[test]
    fn undo_preserves_invalid_utf8_exactly() {
        let mut ed = Editor::new(FileBuffer::from_bytes(b"ab\xffcd"));
        ed.set_selection(1, 4);
        assert_eq!(ed.cut().as_deref(), Some("b\u{FFFD}c"));
        assert_eq!(ed.buffer().to_bytes(), b"ad");
        ed.undo();
        assert_eq!(ed.buffer().to_bytes(), b"ab\xffcd");
    }

    #[test]
    fn modified_tracks_the_save_point() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        let mut ed = editor("");
        assert!(!ed.is_modified());

        type_str(&mut ed, "one");
        assert!(ed.is_modified());
        assert!(ed.buffer().is_modified());
        ed.undo();
        assert!(!ed.is_modified(), "undone back to the loaded state");
        assert!(!ed.buffer().is_modified());
        ed.redo();
        assert!(ed.is_modified());

        ed.save_as(&path).unwrap();
        assert!(!ed.is_modified());
        assert_eq!(ed.buffer().path(), Some(path.as_path()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one");

        ed.undo();
        assert!(ed.is_modified(), "undone past the save");
        ed.redo();
        assert!(!ed.is_modified(), "redone back to the saved state");

        // Typing right after a save must not merge into the saved entry.
        type_str(&mut ed, "s");
        assert!(ed.is_modified());
        ed.undo();
        assert!(!ed.is_modified());
        assert_eq!(text(&ed), "one");

        // Editing after undoing past the save point discards the saved
        // state from the history, so no amount of undo reaches it.
        ed.undo();
        type_str(&mut ed, "x");
        assert!(ed.is_modified());
        ed.undo();
        assert!(ed.is_modified());
        assert_eq!(text(&ed), "");

        ed.save().unwrap();
        assert!(!ed.is_modified());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    }

    #[test]
    fn save_without_path_fails_and_stays_modified() {
        let mut ed = editor("x");
        type_str(&mut ed, "y");
        assert!(ed.save().is_err());
        assert!(ed.is_modified());
    }

    #[test]
    fn already_modified_buffer_starts_modified() {
        let mut buffer = FileBuffer::from_text("a");
        buffer.insert(1, "b");
        let mut ed = Editor::new(buffer);
        assert!(ed.is_modified());
        ed.undo();
        assert!(ed.is_modified(), "there is no saved state in the history");
    }

    #[test]
    fn undo_history_is_linear() {
        let mut ed = editor("");
        type_str(&mut ed, "a");
        ed.move_cursor(LineEnd);
        type_str(&mut ed, "b");
        ed.undo();
        assert!(ed.can_redo());
        ed.move_cursor(LineEnd);
        type_str(&mut ed, "c");
        assert!(!ed.can_redo(), "a new edit discards the redo history");
        assert_eq!(text(&ed), "ac");
        ed.undo();
        ed.undo();
        assert_eq!(text(&ed), "");
        assert!(!ed.can_undo());
    }

    // ----- Files changing on disk -----------------------------------------

    fn file_editor(dir: &tempfile::TempDir, text: &str) -> (std::path::PathBuf, Editor) {
        let path = dir.path().join("a.txt");
        std::fs::write(&path, text).unwrap();
        let editor = Editor::new(FileBuffer::open(&path).unwrap());
        (path, editor)
    }

    #[test]
    fn a_file_the_editor_wrote_itself_is_not_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut ed) = file_editor(&dir, "one\ntwo\n");
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::None);
        type_str(&mut ed, "x");
        ed.save().unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::None);
        // Nor is a rewrite with the same contents.
        std::fs::write(&path, "xone\ntwo\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::None);
        assert!(!ed.is_modified());
    }

    #[test]
    fn an_unmodified_buffer_reloads_and_undo_brings_the_old_contents_back() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut ed) = file_editor(&dir, "one\ntwo\nthree\n");
        ed.go_to_line(2);
        ed.move_cursor(Right);
        std::fs::write(&path, "zero\none\ntwo\nthree\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Reloaded);
        assert_eq!(text(&ed), "zero\none\ntwo\nthree\n");
        assert!(!ed.is_modified());
        // The cursor stays on the text it was on.
        assert_eq!(ed.cursor_position(), Position { line: 3, column: 1 });
        // Undo restores what was there, and saving puts it back on disk.
        assert!(ed.undo());
        assert_eq!(text(&ed), "one\ntwo\nthree\n");
        assert!(ed.is_modified());
        assert_eq!(ed.cursor_position(), Position { line: 2, column: 1 });
        ed.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\nthree\n");
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::None);
    }

    #[test]
    fn unsaved_edits_are_merged_with_changes_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut ed) = file_editor(&dir, "one\ntwo\nthree\nfour\n");
        type_str(&mut ed, "> ");
        std::fs::write(&path, "one\ntwo\nthree\nfour\nfive\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Merged);
        assert_eq!(text(&ed), "> one\ntwo\nthree\nfour\nfive\n");
        assert!(ed.is_modified());
        assert_eq!(ed.cursor_position(), Position { line: 0, column: 2 });
        // Undo takes out what came from disk and leaves the buffer's own
        // edit; typing afterwards doesn't join the merge's undo entry.
        assert!(ed.undo());
        assert_eq!(text(&ed), "> one\ntwo\nthree\nfour\n");
        assert!(ed.redo());
        type_str(&mut ed, "!");
        assert!(ed.undo());
        assert_eq!(text(&ed), "> one\ntwo\nthree\nfour\nfive\n");
        ed.save().unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "> one\ntwo\nthree\nfour\nfive\n"
        );
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::None);
    }

    #[test]
    fn overlapping_changes_leave_conflict_markers_with_the_cursor_on_them() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut ed) = file_editor(&dir, "one\ntwo\nthree\n");
        ed.go_to_line(1);
        type_str(&mut ed, "mine ");
        std::fs::write(&path, "one\ntheirs two\nthree\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Conflicted);
        let merged = text(&ed);
        assert!(
            merged.starts_with("one\n<<<<<<< editor\nmine two\n"),
            "{merged}"
        );
        assert!(
            merged.ends_with("=======\ntheirs two\n>>>>>>> disk\nthree\n"),
            "{merged}"
        );
        assert_eq!(ed.cursor_position(), Position { line: 1, column: 0 });
        assert!(ed.is_modified());
        assert!(ed.undo());
        assert_eq!(text(&ed), "one\nmine two\nthree\n");
        assert_eq!(ed.cursor_position(), Position { line: 1, column: 5 });
    }

    #[test]
    fn the_same_edit_on_both_sides_leaves_the_buffer_unmodified() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut ed) = file_editor(&dir, "one\ntwo\n");
        type_str(&mut ed, "x");
        std::fs::write(&path, "xone\ntwo\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Merged);
        assert_eq!(text(&ed), "xone\ntwo\n");
        assert!(!ed.is_modified());
    }

    #[test]
    fn a_binary_change_is_left_for_saving_to_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut ed) = file_editor(&dir, "one\n");
        type_str(&mut ed, "x");
        std::fs::write(&path, b"one\0\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Unmerged);
        assert_eq!(text(&ed), "xone\n");
        assert!(ed.is_modified());
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::None);
        ed.save().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"xone\n");
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::None);
    }

    #[test]
    fn a_deleted_file_leaves_the_buffer_modified_so_saving_recreates_it() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut ed) = file_editor(&dir, "one\n");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Deleted);
        assert!(ed.is_modified());
        assert_eq!(text(&ed), "one\n");
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::None);
        ed.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");
        assert!(!ed.is_modified());
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::None);
    }

    #[test]
    fn discard_changes_reloads_the_file_and_is_undoable() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut ed) = file_editor(&dir, "one\ntwo\n");
        assert!(!ed.discard_changes().unwrap());
        type_str(&mut ed, "edit ");
        assert!(ed.is_modified());
        assert!(ed.discard_changes().unwrap());
        assert_eq!(text(&ed), "one\ntwo\n");
        assert!(!ed.is_modified());
        assert!(ed.undo());
        assert_eq!(text(&ed), "edit one\ntwo\n");
        assert!(ed.is_modified());
        // Discarding picks up a newer version on disk.
        std::fs::write(&path, "new\n").unwrap();
        assert!(ed.discard_changes().unwrap());
        assert_eq!(text(&ed), "new\n");
        assert!(!ed.is_modified());
        // Without a file there is nothing to reload.
        assert!(editor("x").discard_changes().is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(ed.discard_changes().is_err());
        assert!(ed.is_modified());
    }

    #[test]
    fn a_second_change_from_disk_replaces_the_first_while_nothing_was_edited() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut ed) = file_editor(&dir, "one\ntwo\nthree\n");
        ed.go_to_line(1);
        type_str(&mut ed, "mine ");
        std::fs::write(&path, "one\nfirst two\nthree\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Conflicted);
        std::fs::write(&path, "one\nsecond two\nthree\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Conflicted);
        // One conflict block, showing the latest file, not a conflict
        // inside the earlier conflict.
        let merged = text(&ed);
        assert_eq!(merged.matches("<<<<<<<").count(), 1, "{merged}");
        assert!(!merged.contains("first"), "{merged}");
        assert!(
            merged.ends_with("=======\nsecond two\n>>>>>>> disk\nthree\n"),
            "{merged}"
        );
        assert_eq!(ed.cursor_position(), Position { line: 1, column: 0 });
        // One undo is back to the user's own version, and redo returns.
        assert!(ed.undo());
        assert_eq!(text(&ed), "one\nmine two\nthree\n");
        assert!(ed.redo());
        assert_eq!(text(&ed), merged);
    }

    #[test]
    fn repeated_reloads_of_an_unmodified_buffer_undo_in_one_step() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut ed) = file_editor(&dir, "one\n");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Reloaded);
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Reloaded);
        assert_eq!(text(&ed), "one\ntwo\nthree\n");
        assert!(!ed.is_modified());
        assert!(ed.undo());
        assert_eq!(text(&ed), "one\n");
        assert!(ed.is_modified());
        assert!(!ed.can_undo());
        ed.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::None);
    }

    #[test]
    fn a_clean_merge_followed_by_a_conflict_gives_one_conflict_block() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut ed) = file_editor(&dir, "one\ntwo\nthree\n");
        type_str(&mut ed, "> ");
        std::fs::write(&path, "one\ntwo\nthree\nfour\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Merged);
        std::fs::write(&path, "first one\ntwo\nthree\nfour\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Conflicted);
        let merged = text(&ed);
        assert_eq!(merged.matches("<<<<<<<").count(), 1, "{merged}");
        assert!(merged.starts_with("<<<<<<< editor\n> one\n"), "{merged}");
        assert!(
            merged.ends_with(">>>>>>> disk\ntwo\nthree\nfour\n"),
            "{merged}"
        );
        assert!(ed.undo());
        assert_eq!(text(&ed), "> one\ntwo\nthree\n");
    }

    #[test]
    fn an_undone_change_from_disk_counts_as_the_users_edit() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut ed) = file_editor(&dir, "a\nb\nc\nd\ne\n");
        ed.move_cursor(DocumentEnd);
        type_str(&mut ed, "user\n");
        std::fs::write(&path, "A\nb\nc\nd\ne\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Merged);
        assert_eq!(text(&ed), "A\nb\nc\nd\ne\nuser\n");
        // The user rejects the change from disk...
        assert!(ed.undo());
        assert_eq!(text(&ed), "a\nb\nc\nd\ne\nuser\n");
        // ...so the next change is merged with that rejection kept.
        std::fs::write(&path, "A\nb\nC\nd\ne\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Merged);
        assert_eq!(text(&ed), "a\nb\nC\nd\ne\nuser\n");
    }

    #[test]
    fn an_edit_after_a_change_from_disk_keeps_the_next_one_separate() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut ed) = file_editor(&dir, "one\ntwo\nthree\n");
        std::fs::write(&path, "one\ntwo\nthree\nfour\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Reloaded);
        type_str(&mut ed, "x");
        std::fs::write(&path, "one\ntwo\nthree\nfour\nfive\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Merged);
        assert_eq!(text(&ed), "xone\ntwo\nthree\nfour\nfive\n");
        // Each step is its own undo entry: the edit in between stops the
        // second change replacing the first.
        assert!(ed.undo());
        assert_eq!(text(&ed), "xone\ntwo\nthree\nfour\n");
        assert!(ed.undo());
        assert_eq!(text(&ed), "one\ntwo\nthree\nfour\n");
        assert!(ed.undo());
        assert_eq!(text(&ed), "one\ntwo\nthree\n");
        assert!(!ed.can_undo());
    }

    #[test]
    fn saving_between_changes_from_disk_settles_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut ed) = file_editor(&dir, "one\ntwo\nthree\n");
        type_str(&mut ed, "x");
        std::fs::write(&path, "one\ntwo\nthree\nfour\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Merged);
        ed.save().unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "xone\ntwo\nthree\nfour\n"
        );
        // A writer that didn't see the save overwrites it; the file is
        // what counts, and undo is how to get the saved version back.
        std::fs::write(&path, "one\ntwo\nthree\nfour\nfive\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Reloaded);
        assert_eq!(text(&ed), "one\ntwo\nthree\nfour\nfive\n");
        assert!(ed.undo());
        assert_eq!(text(&ed), "xone\ntwo\nthree\nfour\n");
        assert!(ed.is_modified());
    }

    #[test]
    fn an_edit_the_file_matched_is_still_the_users_when_the_file_moves_on() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut ed) = file_editor(&dir, "one\ntwo\nthree\nfour\n");
        type_str(&mut ed, "x");
        // The file gains the same edit plus another: the merge matches
        // the file exactly, so the buffer counts as unmodified.
        std::fs::write(&path, "xone\ntwo\nthree\nFOUR\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Merged);
        assert!(!ed.is_modified());
        // The file then loses that edit again. It was the user's too, so
        // it stays, merged with the file as it now is.
        std::fs::write(&path, "one\ntwo\nthree\nFOUR\n").unwrap();
        assert_eq!(ed.check_disk().unwrap(), ExternalChange::Merged);
        assert_eq!(text(&ed), "xone\ntwo\nthree\nFOUR\n");
        assert!(ed.is_modified());
        assert!(ed.undo());
        assert_eq!(text(&ed), "xone\ntwo\nthree\nfour\n");
    }

    // ----- Typing code ----------------------------------------------------

    /// An editor over `text` highlighted as `language`, with the cursor
    /// at the `‸` in it.
    fn code(language: Language, text: &str) -> Editor {
        let at = text.find('‸').expect("a cursor");
        let mut ed = editor(&text.replace('‸', ""));
        ed.set_language(language);
        ed.set_cursor(at);
        ed
    }

    /// The buffer with `‸` at the cursor, after any pending indentation,
    /// the way the cursor is shown.
    fn shown(ed: &Editor) -> String {
        let mut shown = text(ed);
        let pending = ed.pending_indentation().unwrap_or_default();
        shown.insert_str(ed.cursor(), &format!("{pending}‸"));
        shown
    }

    #[test]
    fn opening_brackets_are_paired_and_closing_ones_stepped_over() {
        let mut ed = code(Language::Rust, "‸");
        type_str(&mut ed, "fn main(");
        assert_eq!(shown(&ed), "fn main(‸)");
        type_str(&mut ed, ")");
        assert_eq!(shown(&ed), "fn main()‸");
        type_str(&mut ed, " { foo([a], b");
        assert_eq!(shown(&ed), "fn main() { foo([a], b‸)}");
        type_str(&mut ed, "); }");
        assert_eq!(shown(&ed), "fn main() { foo([a], b); }‸");

        ed.undo();
        assert_eq!(
            shown(&ed),
            "fn main() { foo([a], b)‸}",
            "stepping over a bracket isn't an edit"
        );
    }

    #[test]
    fn brackets_are_not_paired_before_text_or_in_strings_and_comments() {
        let mut ed = code(Language::Rust, "let x = ‸foo;");
        type_str(&mut ed, "(");
        assert_eq!(shown(&ed), "let x = (‸foo;");

        let mut ed = code(Language::Rust, "let s = \"a‸\";");
        type_str(&mut ed, "{");
        assert_eq!(shown(&ed), "let s = \"a{‸\";");
        type_str(&mut ed, "}");
        assert_eq!(shown(&ed), "let s = \"a{}‸\";");

        let mut ed = code(Language::Rust, "// see ‸");
        type_str(&mut ed, "(");
        assert_eq!(shown(&ed), "// see (‸");

        let mut ed = code(Language::Rust, "let s = \"‸)\";");
        type_str(&mut ed, ")");
        assert_eq!(
            shown(&ed),
            "let s = \")‸)\";",
            "a bracket in a string isn't stepped over"
        );

        let mut ed = code(Language::Rust, "‸");
        ed.insert_char('(');
        ed.move_cursor(Movement::Right);
        ed.set_selection(0, 2);
        ed.insert_char('[');
        assert_eq!(shown(&ed), "[‸", "a selection is replaced as usual");
    }

    #[test]
    fn backspace_deletes_an_empty_pair() {
        let mut ed = code(Language::Rust, "foo‸");
        type_str(&mut ed, "(");
        ed.backspace();
        assert_eq!(shown(&ed), "foo‸");
        type_str(&mut ed, "(a");
        ed.backspace();
        ed.backspace();
        assert_eq!(shown(&ed), "foo‸");
        ed.undo();
        assert_eq!(shown(&ed), "foo(‸)");
        ed.undo();
        assert_eq!(shown(&ed), "foo(a‸)");
    }

    #[test]
    fn newline_between_braces_puts_the_closing_one_on_its_own_line() {
        let mut ed = code(Language::Rust, "‸");
        type_str(&mut ed, "fn main() {\n");
        assert_eq!(shown(&ed), "fn main() {\n    ‸\n}");
        type_str(&mut ed, "if x {\n");
        assert_eq!(shown(&ed), "fn main() {\n    if x {\n        ‸\n    }\n}");
        ed.undo();
        assert_eq!(
            shown(&ed),
            "fn main() {\n    if x {‸}\n}",
            "the line break is one undo step"
        );

        // Parentheses and square brackets too, in any language.
        let mut ed = code(Language::Python, "‸");
        type_str(&mut ed, "x = foo(\n");
        assert_eq!(shown(&ed), "x = foo(\n    ‸\n)");
        type_str(&mut ed, "[\n");
        assert_eq!(shown(&ed), "x = foo(\n    [\n        ‸\n    ]\n)");

        // The closing brace goes level with the statement, not with a
        // continuation line.
        let mut ed = code(Language::C, "‸");
        type_str(&mut ed, "if (a &&\n");
        assert_eq!(shown(&ed), "if (a &&\n    ‸)");
        type_str(&mut ed, "b) {\n");
        assert_eq!(shown(&ed), "if (a &&\n    b) {\n    ‸\n}");

        let mut ed = code(Language::Rust, "‸");
        type_str(&mut ed, "items.iter().for_each(|x| {\n");
        assert_eq!(shown(&ed), "items.iter().for_each(|x| {\n    ‸\n})");
    }

    #[test]
    fn braces_on_their_own_line() {
        let mut ed = code(Language::C, "‸");
        type_str(&mut ed, "void f()\n");
        assert_eq!(shown(&ed), "void f()\n‸");
        type_str(&mut ed, "{\n");
        assert_eq!(shown(&ed), "void f()\n{\n    ‸\n}");
        type_str(&mut ed, "if (x)\n");
        assert_eq!(
            shown(&ed),
            "void f()\n{\n    if (x)\n        ‸\n}",
            "a brace-less body is indented"
        );
        type_str(&mut ed, "{");
        assert_eq!(
            shown(&ed),
            "void f()\n{\n    if (x)\n    {‸}\n}",
            "until a brace goes back level with the `if`"
        );
        type_str(&mut ed, "\n");
        assert_eq!(
            shown(&ed),
            "void f()\n{\n    if (x)\n    {\n        ‸\n    }\n}"
        );

        // Typed at the right level, a brace stays put.
        let mut ed = code(Language::C, "int x;\n‸");
        type_str(&mut ed, "{");
        assert_eq!(shown(&ed), "int x;\n{‸}");
    }

    #[test]
    fn brace_less_bodies_go_back_to_the_statement_level() {
        let mut ed = code(Language::C, "‸");
        type_str(&mut ed, "if (x)\n");
        assert_eq!(shown(&ed), "if (x)\n    ‸");
        type_str(&mut ed, "y();\n");
        assert_eq!(shown(&ed), "if (x)\n    y();\n‸");

        let mut ed = code(Language::C, "if (a &&\n    b)\n    y();‸");
        ed.insert_char('\n');
        assert_eq!(shown(&ed), "if (a &&\n    b)\n    y();\n‸");

        let mut ed = code(Language::C, "if (a) {\n} else\n    y();‸");
        ed.insert_char('\n');
        assert_eq!(shown(&ed), "if (a) {\n} else\n    y();\n‸");
    }

    #[test]
    fn multi_line_conditions_in_rustfmt_style() {
        let mut ed = code(Language::Rust, "fn f() {\n    ‸\n}");
        type_str(&mut ed, "if a\n");
        assert_eq!(shown(&ed), "fn f() {\n    if a\n        ‸\n}");
        type_str(&mut ed, "&& b\n");
        assert_eq!(shown(&ed), "fn f() {\n    if a\n        && b\n        ‸\n}");
        type_str(&mut ed, "{\n");
        assert_eq!(
            shown(&ed),
            "fn f() {\n    if a\n        && b\n    {\n        ‸\n    }\n}"
        );
    }

    #[test]
    fn indentation_follows_the_statement_not_its_continuations() {
        // Arguments spread over lines line up with the first.
        let mut ed = code(Language::Rust, "    let x = foo(a,‸");
        ed.insert_char('\n');
        assert_eq!(shown(&ed), "    let x = foo(a,\n        ‸");
        type_str(&mut ed, "b);\n");
        assert_eq!(
            shown(&ed),
            "    let x = foo(a,\n        b);\n    ‸",
            "and the statement after goes back to the level of the first line"
        );

        let mut ed = code(Language::Rust, "let x = foo\n    .bar()\n    .baz();‸");
        ed.insert_char('\n');
        assert_eq!(shown(&ed), "let x = foo\n    .bar()\n    .baz();\n‸");

        let mut ed = code(Language::Rust, "let x = foo\n    .bar()‸");
        ed.insert_char('\n');
        assert_eq!(
            shown(&ed),
            "let x = foo\n    .bar()\n    ‸",
            "an unfinished statement keeps its continuation level"
        );

        let mut ed = code(
            Language::Rust,
            "let v = vec![\n    Foo {\n        a: 1,\n    },‸",
        );
        ed.insert_char('\n');
        assert_eq!(
            shown(&ed),
            "let v = vec![\n    Foo {\n        a: 1,\n    },\n    ‸"
        );

        let mut ed = code(Language::Rust, "if x { // why‸");
        ed.insert_char('\n');
        assert_eq!(shown(&ed), "if x { // why\n    ‸", "comments don't count");
        let mut ed = code(Language::Rust, "foo(\"(\"‸");
        ed.insert_char('\n');
        assert_eq!(shown(&ed), "foo(\"(\"\n    ‸", "nor brackets in strings");
    }

    #[test]
    fn continuation_lines_can_be_aligned_instead_of_indented() {
        let indent = CodeStyle {
            continuation: auto_indent::ContinuationIndent::Align,
        };
        let mut ed = code(Language::Rust, "fn f() {\n    let x = foo(a,‸\n}");
        ed.set_code_style(indent);
        ed.insert_char('\n');
        assert_eq!(
            shown(&ed),
            "fn f() {\n    let x = foo(a,\n                ‸\n}"
        );
        type_str(&mut ed, "b,\n");
        assert_eq!(
            shown(&ed),
            "fn f() {\n    let x = foo(a,\n                b,\n                ‸\n}",
            "later lines keep the level"
        );
        type_str(&mut ed, "c);\n");
        assert_eq!(
            shown(&ed),
            "fn f() {\n    let x = foo(a,\n                b,\n                c);\n    ‸\n}"
        );
    }

    #[test]
    fn nested_continuation_with_indentation() {
        // Brackets nested in a continuation go a level past it.
        let mut ed = code(Language::C, "if (a &&\n    foo(b,‸");
        ed.insert_char('\n');
        assert_eq!(shown(&ed), "if (a &&\n    foo(b,\n        ‸");

        // A bracket ending its line is indented the same either way.
        let mut ed = code(Language::Rust, "let x = foo(\n    a,\n    bar(b,‸");
        ed.insert_char('\n');
        assert_eq!(shown(&ed), "let x = foo(\n    a,\n    bar(b,\n        ‸");
    }

    #[test]
    fn python_method_continuation_with_indentation() {
        let mut ed = code(Language::Python, "‸");
        type_str(&mut ed, "def f(a,\n");
        assert_eq!(shown(&ed), "def f(a,\n    ‸)");
        type_str(&mut ed, "b):\n");
        assert_eq!(shown(&ed), "def f(a,\n    b):\n    ‸");
    }

    #[test]
    fn closing_brackets_typed_at_line_start_go_to_the_statement_level() {
        let mut ed = code(Language::Rust, "let v = [\n    1,\n    2,‸");
        type_str(&mut ed, "\n]");
        assert_eq!(shown(&ed), "let v = [\n    1,\n    2,\n]‸");

        let mut ed = code(Language::C, "if (a) {\n    x();\n        ‸");
        type_str(&mut ed, "}");
        assert_eq!(shown(&ed), "if (a) {\n    x();\n}‸");
        ed.undo();
        assert_eq!(shown(&ed), "if (a) {\n    x();\n        ‸");

        let mut ed = code(Language::Rust, "x();\n    ‸");
        type_str(&mut ed, ")");
        assert_eq!(
            shown(&ed),
            "x();\n    )‸",
            "with nothing to close, left alone"
        );
    }

    #[test]
    fn colons_open_scopes() {
        let mut ed = code(Language::Python, "‸");
        type_str(&mut ed, "def f(a,\n");
        assert_eq!(shown(&ed), "def f(a,\n    ‸)");
        type_str(&mut ed, "b):\n");
        assert_eq!(shown(&ed), "def f(a,\n    b):\n    ‸");
        type_str(&mut ed, "if a:\nx = 1\n");
        assert_eq!(
            shown(&ed),
            "def f(a,\n    b):\n    if a:\n        x = 1\n        ‸"
        );
        type_str(&mut ed, "else:");
        assert_eq!(
            shown(&ed),
            "def f(a,\n    b):\n    if a:\n        x = 1\n    else:‸",
            "`else` goes level with its `if`"
        );
        type_str(&mut ed, "\nreturn b\n");
        assert_eq!(
            shown(&ed),
            "def f(a,\n    b):\n    if a:\n        x = 1\n    else:\n        return b\n    ‸",
            "a block ends after `return`"
        );

        let mut ed = code(Language::Python, "x = 1 + \\‸");
        type_str(&mut ed, "\n2\n");
        assert_eq!(shown(&ed), "x = 1 + \\\n    2\n‸");

        let mut ed = code(Language::Markdown, "Notes:‸");
        ed.insert_char('\n');
        assert_eq!(shown(&ed), "Notes:\n‸", "not in prose");
    }

    #[test]
    fn case_labels() {
        let mut ed = code(Language::C, "‸");
        type_str(&mut ed, "switch (x) {\ncase 1:\n");
        assert_eq!(shown(&ed), "switch (x) {\n    case 1:\n        ‸\n}");
        type_str(&mut ed, "break;\ncase 2:");
        assert_eq!(
            shown(&ed),
            "switch (x) {\n    case 1:\n        break;\n    case 2:‸\n}",
            "a label goes level with the one before"
        );

        // Labels of a nested switch don't count.
        let mut ed = code(
            Language::Cpp,
            "switch (a) {\ncase 1:\n    switch (b) {\n    case 2:\n        break;\n    }\n    ‸\n}",
        );
        type_str(&mut ed, "default:");
        assert_eq!(
            shown(&ed),
            "switch (a) {\ncase 1:\n    switch (b) {\n    case 2:\n        break;\n    }\ndefault:‸\n}"
        );
    }

    #[test]
    fn smart_indentation_uses_the_buffer_style() {
        let mut ed = code(Language::Rust, "fn f() {\n\tx();\n}\n‸");
        type_str(&mut ed, "fn g() {\n");
        assert_eq!(shown(&ed), "fn f() {\n\tx();\n}\nfn g() {\n\t‸\n}");

        let indent = CodeStyle {
            continuation: auto_indent::ContinuationIndent::Align,
        };
        let mut ed = code(Language::Rust, "fn f() {\n\tlet x = foo(a,‸");
        ed.set_code_style(indent);
        ed.insert_char('\n');
        assert_eq!(
            shown(&ed),
            "fn f() {\n\tlet x = foo(a,\n\t            ‸",
            "tabs to indent, spaces to align"
        );
    }

    // ----- Code completion ------------------------------------------------

    /// Type, take the request that typing makes, and answer it with
    /// `completion`.
    fn suggest(ed: &mut Editor, completion: &str) {
        let request = ed
            .take_completion_request()
            .expect("typing asks for a completion");
        assert!(ed.offer_completion(request.serial, completion));
        assert_eq!(ed.suggestion(), Some(completion.trim_end()));
    }

    #[test]
    fn typing_asks_for_a_completion_and_moving_does_not() {
        let mut ed = editor("fn main() {\n    \n}\n");
        assert!(!ed.completion_wanted());
        assert!(ed.take_completion_request().is_none());
        ed.move_cursor(Down);
        ed.move_cursor(LineEnd);
        assert!(!ed.completion_wanted(), "moving doesn't ask");
        ed.insert_char('l');
        assert!(ed.completion_wanted());
        let request = ed.take_completion_request().unwrap();
        assert!(!ed.completion_wanted(), "taken");
        assert_eq!(request.language, Language::Plain);
        assert_eq!(request.prefix, "fn main() {\n    l");
        assert_eq!(request.prefix_start, 0);
        assert_eq!(request.suffix, "\n}\n");
        assert_eq!(request.indentation, Indentation::Spaces(4));
        // The answer to that request is the suggestion; a stale one is
        // ignored.
        assert!(!ed.offer_completion(request.serial + 1, "nope"));
        assert!(ed.suggestion().is_none());
        assert!(ed.offer_completion(request.serial, "et x = 1;\n"));
        assert_eq!(ed.suggestion(), Some("et x = 1;"));
        // Moving drops it, and doesn't ask for another.
        ed.move_cursor(Left);
        assert!(ed.suggestion().is_none());
        assert!(!ed.completion_wanted());
        // An answer arriving after the move is stale too.
        assert!(!ed.offer_completion(request.serial, "et x = 1;"));
        // Backspace and delete are typing.
        ed.backspace();
        assert!(ed.completion_wanted());
        ed.take_completion_request();
        ed.delete_forward();
        assert!(ed.completion_wanted());
        // Nothing is asked for with a selection, and a whitespace answer
        // is no suggestion.
        ed.take_completion_request();
        ed.insert_char('x');
        ed.set_selection(0, 3);
        assert!(!ed.completion_wanted(), "selecting is a movement");
        ed.clear_selection();
        ed.insert_char('y');
        let serial = ed.take_completion_request().unwrap().serial;
        assert!(!ed.offer_completion(serial, "  \n"));
        assert!(ed.suggestion().is_none());
    }

    #[test]
    fn the_request_carries_a_window_around_the_cursor_with_pending_indentation() {
        let mut ed = editor("");
        ed.set_language(Language::Rust);
        for _ in 0..200 {
            type_str(&mut ed, &format!("{}\n", "x".repeat(200)));
        }
        type_str(&mut ed, "    fn f() {\n");
        assert_eq!(ed.pending_indentation(), Some("        "));
        let request = ed.take_completion_request().unwrap();
        assert_eq!(request.language, Language::Rust);
        // The prefix opens with a line break, holds at most the window,
        // and ends with the indentation the cursor is shown at.
        assert!(
            request.prefix.starts_with("\nxxx"),
            "{:?}",
            &request.prefix[..10]
        );
        assert!(request.prefix.len() <= COMPLETION_PREFIX_BYTES + 201 + 18);
        assert!(request.prefix.ends_with("\n    fn f() {\n        "));
        assert_eq!(
            ed.buffer().byte_at(request.prefix_start),
            b'\n',
            "starts at a line break"
        );
        assert!(request.prefix_start > 0);
        assert_eq!(request.suffix, "\n    }", "the closing brace typing paired");

        // CRLF files are sent with plain line breaks, and the suffix is a
        // bounded number of lines.
        let mut lines = String::new();
        for i in 0..100 {
            lines.push_str(&format!("line {i}\r\n"));
        }
        let mut ed = editor(&lines);
        assert_eq!(ed.buffer().eol().as_str(), "\r\n");
        ed.go_to_line(2);
        ed.insert_char('a');
        let request = ed.take_completion_request().unwrap();
        assert_eq!(request.prefix, "line 0\nline 1\na");
        assert_eq!(request.prefix_start, 0);
        assert_eq!(
            request.suffix.matches('\n').count(),
            COMPLETION_SUFFIX_LINES
        );
        assert!(request.suffix.starts_with("line 2\nline 3\n"));
        assert!(!request.suffix.contains('\r'));
    }

    #[test]
    fn typing_along_with_a_suggestion_keeps_it() {
        let mut ed = editor("");
        type_str(&mut ed, "let");
        suggest(&mut ed, " x = 1;\nlet y = 2;");
        // Each matching character is consumed; no new completion is
        // asked for.
        type_str(&mut ed, " x =");
        assert_eq!(ed.suggestion(), Some(" 1;\nlet y = 2;"));
        assert!(!ed.completion_wanted());
        assert_eq!(text(&ed), "let x =");
        // A different character drops it and asks again.
        ed.insert_char('2');
        assert!(ed.suggestion().is_none());
        assert!(ed.completion_wanted());
        assert_eq!(text(&ed), "let x =2");

        // Typing all of a suggestion asks for the next.
        let mut ed = editor("");
        type_str(&mut ed, "a");
        suggest(&mut ed, "bc");
        type_str(&mut ed, "bc");
        assert!(ed.suggestion().is_none());
        assert!(ed.completion_wanted());
    }

    /// Answer the request typing made with `completion`, returning the
    /// suggestion it becomes.
    fn offer(ed: &mut Editor, completion: &str) -> Option<String> {
        let serial = ed.take_completion_request().unwrap().serial;
        ed.offer_completion(serial, completion);
        ed.suggestion().map(str::to_owned)
    }

    #[test]
    fn brackets_pair_the_same_whether_or_not_a_suggestion_came_first() {
        // Typed before the suggestion arrives: paired, and the answer
        // stops short of the closing bracket the pair supplied.
        let mut ed = code(Language::Rust, "fn f() {\n    let v = foo‸\n}");
        ed.insert_char('(');
        assert_eq!(
            offer(&mut ed, "a, b);\n    bar();").as_deref(),
            Some("a, b")
        );
        assert_eq!(shown(&ed), "fn f() {\n    let v = foo(‸)\n}");

        // Typed along with a suggestion that has it: paired all the same,
        // and the rest of the suggestion stays inside.
        let mut ed = code(Language::Rust, "fn f() {\n    let v = fo‸\n}");
        ed.insert_char('o');
        assert_eq!(
            offer(&mut ed, "(a, [b]);\n    bar();").as_deref(),
            Some("(a, [b]);\n    bar();")
        );
        ed.insert_char('(');
        assert_eq!(shown(&ed), "fn f() {\n    let v = foo(‸)\n}");
        assert_eq!(ed.suggestion(), Some("a, [b]"));
        type_str(&mut ed, "a, [");
        assert_eq!(shown(&ed), "fn f() {\n    let v = foo(a, [‸])\n}");
        assert_eq!(ed.suggestion(), Some("b"));
        type_str(&mut ed, "b");
        assert!(ed.suggestion().is_none());
        assert!(ed.completion_wanted(), "typed through it");

        // A suggestion that is only the closing bracket is no suggestion.
        let mut ed = code(Language::Rust, "foo‸");
        ed.insert_char('(');
        assert_eq!(offer(&mut ed, ");"), None);
    }

    #[test]
    fn taking_a_line_that_opens_a_block_pairs_its_bracket() {
        // Tab, Tab: the first pairs the brace, the second takes the body
        // and puts the brace on a line of its own, as suggested.
        let mut ed = code(Language::Rust, "fn f() {\n    ‸\n}");
        ed.insert_char('i');
        assert!(offer(&mut ed, "f x {\n        y();\n    }\n    z();").is_some());
        ed.accept_suggestion();
        assert_eq!(shown(&ed), "fn f() {\n    if x {‸}\n}");
        assert_eq!(ed.suggestion(), Some("\n        y();"));
        assert!(ed.suggestion_continues());
        ed.accept_suggestion();
        assert_eq!(shown(&ed), "fn f() {\n    if x {\n        y();‸\n    }\n}");
        assert!(ed.completion_wanted());
        // One undo step each.
        ed.undo();
        assert_eq!(shown(&ed), "fn f() {\n    if x {‸}\n}");

        // Tab, Enter, Tab: Enter between the pair makes the lines, and
        // the suggestion goes on in between.
        let mut ed = code(Language::Rust, "fn f() {\n    ‸\n}");
        ed.insert_char('i');
        offer(&mut ed, "f x {\n        y();\n    }");
        ed.accept_suggestion();
        ed.insert_char('\n');
        assert_eq!(shown(&ed), "fn f() {\n    if x {\n        ‸\n    }\n}");
        assert_eq!(ed.suggestion(), Some("y();"));
        ed.accept_suggestion();
        assert_eq!(shown(&ed), "fn f() {\n    if x {\n        y();‸\n    }\n}");

        // An opening bracket in a string isn't paired.
        let mut ed = code(Language::Rust, "‸");
        ed.insert_char('f');
        offer(&mut ed, "(\"{\");");
        ed.dismiss_suggestion();
        ed.insert_char('(');
        ed.insert_char('"');
        offer(&mut ed, "{\n");
        ed.accept_suggestion();
        assert_eq!(shown(&ed), "f(\"{‸)");
    }

    #[test]
    fn a_suggestion_that_opens_a_block_ends_with_it() {
        // Taking the first line pairs the brace and confines the rest to
        // the block, so what follows the block isn't shown.
        let mut ed = code(Language::Rust, "fn f() {\n    ‸\n}");
        ed.insert_char('i');
        assert_eq!(
            offer(&mut ed, "f (a) {\n        return;\n    }\n    foo();\n}").as_deref(),
            Some("f (a) {\n        return;\n    }")
        );
        ed.accept_suggestion();
        ed.accept_suggestion();
        assert_eq!(
            shown(&ed),
            "fn f() {\n    if (a) {\n        return;‸\n    }\n}"
        );
        assert!(ed.completion_wanted(), "foo(); comes next");

        // A block the first line opens but that Tab wouldn't pair, before
        // other text, doesn't end it; nor does one on a later line.
        let mut ed = code(Language::Rust, "‸x");
        ed.insert_char('a');
        assert_eq!(
            offer(&mut ed, " {\n}\nb();").as_deref(),
            Some(" {\n}\nb();")
        );
        let mut ed = code(Language::Rust, "‸");
        ed.insert_char('a');
        assert_eq!(
            offer(&mut ed, "();\nif b {\n}\nc();").as_deref(),
            Some("();\nif b {\n}\nc();")
        );
    }

    #[test]
    fn a_block_typed_before_its_suggestion_gets_the_suggested_layout() {
        let mut ed = code(Language::Rust, "‸");
        type_str(&mut ed, "if x {");
        assert_eq!(shown(&ed), "if x {‸}");
        assert_eq!(
            offer(&mut ed, "\n    y();\n}\nz();").as_deref(),
            Some("\n    y();")
        );
        ed.accept_suggestion();
        assert_eq!(shown(&ed), "if x {\n    y();‸\n}");
    }

    #[test]
    fn suggestions_stay_within_the_scope_at_the_cursor() {
        // The block's closing brace is already there: the suggestion
        // stops before the model's.
        let mut ed = code(Language::Rust, "fn f() {\n    ‸\n}\n");
        ed.insert_char('x');
        assert_eq!(
            offer(&mut ed, " = 1;\n}\n\nfn g() {\n}").as_deref(),
            Some(" = 1;")
        );
        // Nested scopes in the suggestion are its own.
        ed.dismiss_suggestion();
        ed.insert_char(' ');
        assert_eq!(
            offer(&mut ed, "= if a { (1) } else { 2 };\n    y();\n}").as_deref(),
            Some("= if a { (1) } else { 2 };\n    y();")
        );

        // Nothing closes the bracket after the cursor, as when it was
        // typed before other text: the suggestion closes it, and stops.
        let mut ed = code(Language::Rust, "foo(‸x");
        ed.insert_char('a');
        assert_eq!(offer(&mut ed, ", b);\nbar();").as_deref(), Some(", b)"));

        // Brackets in strings and comments don't count.
        let mut ed = code(Language::Rust, "foo(‸)");
        ed.insert_char('"');
        assert_eq!(
            offer(&mut ed, ")\", 1 /* ) */);").as_deref(),
            Some(")\", 1 /* ) */")
        );
    }

    #[test]
    fn enter_along_with_a_suggestion_takes_its_indentation() {
        // The next line of the suggestion is indented more than the
        // current one: Enter goes on to it with that indentation pending.
        // Deleting the paired brace leaves nothing after the cursor.
        let mut ed = editor("");
        type_str(&mut ed, "fn f() {");
        ed.delete_forward();
        suggest(&mut ed, "\n    body;\n}");
        ed.insert_char('\n');
        assert_eq!(text(&ed), "fn f() {\n");
        assert_eq!(ed.pending_indentation(), Some("    "));
        assert_eq!(ed.suggestion(), Some("body;\n}"));
        assert_eq!(ed.cursor_position(), Position { line: 1, column: 4 });
        type_str(&mut ed, "bo");
        assert_eq!(text(&ed), "fn f() {\n    bo");
        assert_eq!(ed.suggestion(), Some("dy;\n}"));
        // Less indented: Enter inside the braces would indent the new
        // line, but the suggestion says none.
        type_str(&mut ed, "dy;");
        assert_eq!(ed.suggestion(), Some("\n}"));
        ed.insert_char('\n');
        assert_eq!(ed.pending_indentation(), None);
        assert_eq!(ed.suggestion(), Some("}"));
        ed.insert_char('}');
        assert_eq!(text(&ed), "fn f() {\n    body;\n}");
        assert!(ed.suggestion().is_none());
        assert!(ed.completion_wanted(), "typed through it all");

        // Enter in the middle of a line writes the indentation out; the
        // suggestion goes on if it agrees.
        let mut ed = editor("    a();b();\n");
        ed.set_cursor(8);
        ed.insert_char('c');
        assert_eq!(text(&ed), "    a();cb();\n");
        ed.backspace();
        suggest(&mut ed, "\n    d();");
        ed.insert_char('\n');
        assert_eq!(text(&ed), "    a();\n    b();\n");
        assert_eq!(ed.suggestion(), Some("d();"));
        // And is dropped if it doesn't.
        ed.set_cursor(7);
        ed.insert_char('x');
        ed.backspace();
        suggest(&mut ed, "\nd();");
        ed.insert_char('\n');
        assert_eq!(text(&ed), "    a()\n    ;\n    b();\n");
        assert!(ed.suggestion().is_none());
        assert!(ed.completion_wanted());
    }

    #[test]
    fn tab_takes_a_line_then_the_rest() {
        let mut ed = editor("fn f() {\n    \n}\n");
        ed.go_to_line(1);
        ed.move_cursor(LineEnd);
        ed.insert_char('l');
        suggest(&mut ed, "et a = 1;\n    let b = 2;\n    let c = 3;");
        assert!(!ed.suggestion_continues());
        assert!(ed.accept_suggestion());
        assert_eq!(text(&ed), "fn f() {\n    let a = 1;\n}\n");
        assert_eq!(ed.suggestion(), Some("\n    let b = 2;\n    let c = 3;"));
        assert!(ed.suggestion_continues());
        assert!(!ed.completion_wanted());
        assert!(ed.accept_suggestion());
        assert_eq!(
            text(&ed),
            "fn f() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n}\n"
        );
        assert!(ed.suggestion().is_none());
        assert!(ed.completion_wanted(), "all taken: ask for more");
        assert_eq!(
            ed.cursor_position(),
            Position {
                line: 3,
                column: 14
            }
        );
        // Each acceptance is one undo step.
        assert!(ed.undo());
        assert_eq!(text(&ed), "fn f() {\n    let a = 1;\n}\n");
        assert!(ed.undo());
        assert_eq!(text(&ed), "fn f() {\n    l\n}\n");

        // Typing after the first Tab puts the next one back to one line:
        // Tab, Enter, Tab takes two lines.
        let mut ed = editor("");
        type_str(&mut ed, "a");
        suggest(&mut ed, "1\nb2\nc3");
        ed.accept_suggestion();
        assert_eq!(text(&ed), "a1");
        ed.insert_char('\n');
        assert_eq!(ed.suggestion(), Some("b2\nc3"));
        assert!(!ed.suggestion_continues());
        ed.accept_suggestion();
        assert_eq!(text(&ed), "a1\nb2");
        assert_eq!(ed.suggestion(), Some("\nc3"));
        // A suggestion that opens with line breaks: the first Tab takes
        // through its first line of text.
        ed.dismiss_suggestion();
        assert!(!ed.completion_wanted(), "dismissing isn't typing");
        ed.insert_char(';');
        suggest(&mut ed, "\n\nfn g() {\n}");
        ed.accept_suggestion();
        assert_eq!(text(&ed), "a1\nb2;\n\nfn g() {}", "the brace paired");
        assert!(ed.suggestion().is_none(), "nothing left but the brace");
        // Pending indentation is written out with the accepted text.
        let mut ed = editor("");
        type_str(&mut ed, "{\n");
        assert_eq!(ed.pending_indentation(), Some("    "));
        suggest(&mut ed, "x");
        ed.accept_suggestion();
        assert_eq!(text(&ed), "{\n    x\n}");
    }

    #[test]
    fn suggestions_are_dropped_by_edits_and_movement() {
        let mut ed = editor("abc\n");
        ed.move_cursor(LineEnd);
        ed.insert_char('d');
        suggest(&mut ed, "ef");
        ed.undo();
        assert!(ed.suggestion().is_none());
        assert!(!ed.completion_wanted(), "undo isn't typing");
        ed.redo();
        assert!(ed.suggestion().is_none());
        ed.insert_char('e');
        suggest(&mut ed, "f");
        ed.paste("zz");
        assert!(ed.suggestion().is_none());
        assert!(ed.completion_wanted(), "pasting is typing");
        ed.take_completion_request();
        ed.insert_char('!');
        suggest(&mut ed, "?");
        ed.set_cursor(0);
        assert!(ed.suggestion().is_none());
        assert!(!ed.accept_suggestion());
        assert!(!ed.dismiss_suggestion());
    }
}
