//! A single line of editable text: the model behind one-line inputs such
//! as the command palette's query and the search box.
//!
//! The line edit does what a text field in a graphical program does, in
//! a frontend-agnostic way: it holds the text, a cursor, and a selection
//! (an anchor at the fixed end, the cursor at the moving end, as in the
//! [`Editor`](crate::Editor)), and moves, selects, inserts, and deletes by
//! character and by word. The frontend maps keys and mouse positions onto
//! these actions and draws the result; the [`cells`](LineEdit::cells)
//! layout gives it the display column of every character for both.
//!
//! Line breaks can't be part of the text: inserting text drops them, so a
//! pasted paragraph becomes one line.

use crate::text::{self, Grapheme};
use std::ops::Range;

/// Tabs in the text are shown at this width.
const TAB_WIDTH: usize = 4;

/// A cursor movement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Movement {
    /// One character left.
    Left,
    /// One character right.
    Right,
    /// To the start of the previous word.
    WordLeft,
    /// To the end of the current (or next) word.
    WordRight,
    /// To the start of the text.
    Start,
    /// To the end of the text.
    End,
}

/// One character of the text laid out for display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell<'a> {
    /// The character's bytes within the text.
    pub range: Range<usize>,
    /// The display column it starts at.
    pub column: usize,
    /// Its width in columns.
    pub width: usize,
    /// The character; `"\t"` for a tab, whose width spans to the next
    /// tab stop.
    pub text: &'a str,
}

/// A single line of text with a cursor and a selection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineEdit {
    text: String,
    /// Byte offset of the cursor, always on a character boundary.
    cursor: usize,
    /// The fixed end of the selection; the cursor is the moving end.
    /// `None` when nothing is selected.
    anchor: Option<usize>,
}

impl LineEdit {
    /// An empty line edit.
    pub fn new() -> LineEdit {
        LineEdit::default()
    }

    /// A line edit holding `text` (less any line breaks), with the cursor
    /// at the end and nothing selected.
    pub fn from_text(text: &str) -> LineEdit {
        let mut edit = LineEdit::new();
        edit.set_text(text);
        edit
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The cursor's byte offset.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The selection anchor, `None` when nothing is selected.
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

    /// The selected text, or `None` if the selection is empty.
    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|range| &self.text[range])
    }

    // ----- Cursor and selection -------------------------------------------

    /// Place the cursor and clear the selection. The offset is clamped to
    /// the text and snapped back to a character boundary.
    pub fn set_cursor(&mut self, offset: usize) {
        self.cursor = self.snap(offset);
        self.anchor = None;
    }

    /// Select from `anchor` to `cursor`, leaving the cursor at `cursor`.
    /// Both are clamped and snapped like [`set_cursor`](Self::set_cursor).
    pub fn set_selection(&mut self, anchor: usize, cursor: usize) {
        self.anchor = Some(self.snap(anchor));
        self.cursor = self.snap(cursor);
    }

    /// Select everything, leaving the cursor at the end.
    pub fn select_all(&mut self) {
        self.set_selection(0, self.text.len());
    }

    /// Clear the selection, leaving the cursor where it is.
    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// Select the word at a byte offset, as a double-click does; see
    /// [`text::word_at`]. The cursor ends up at the end of the word.
    pub fn select_word_at(&mut self, offset: usize) {
        let word = text::word_at(self.text.as_bytes(), self.snap(offset));
        self.set_selection(word.start, word.end);
    }

    /// Move the cursor. With `extend` the selection grows (or starts) from
    /// where the cursor was; without, the selection is cleared, and
    /// moving left or right out of one only collapses it to its start or
    /// end.
    pub fn move_cursor(&mut self, movement: Movement, extend: bool) {
        if !extend && let Some(range) = self.selection() {
            let collapsed = match movement {
                Movement::Left => Some(range.start),
                Movement::Right => Some(range.end),
                _ => None,
            };
            if let Some(offset) = collapsed {
                self.set_cursor(offset);
                return;
            }
        }
        let bytes = self.text.as_bytes();
        let target = match movement {
            Movement::Left => self.prev_char(self.cursor),
            Movement::Right => self.next_char(self.cursor),
            Movement::WordLeft => text::prev_word_boundary(bytes, self.cursor),
            Movement::WordRight => text::next_word_boundary(bytes, self.cursor),
            Movement::Start => 0,
            Movement::End => self.text.len(),
        };
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = target;
    }

    // ----- Editing --------------------------------------------------------

    /// Replace the text, dropping any line breaks, with the cursor at the
    /// end and nothing selected.
    pub fn set_text(&mut self, text: &str) {
        self.text = text.chars().filter(|c| !is_line_break(*c)).collect();
        self.cursor = self.text.len();
        self.anchor = None;
    }

    /// Remove all the text.
    pub fn clear(&mut self) {
        self.set_text("");
    }

    /// Insert text at the cursor, replacing the selection if there is one.
    /// Line breaks are dropped. Returns whether the text changed.
    pub fn insert(&mut self, text: &str) -> bool {
        let text: String = text.chars().filter(|c| !is_line_break(*c)).collect();
        if text.is_empty() {
            return self.delete_selection();
        }
        self.delete_selection();
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
        true
    }

    /// Delete the selection, or the character before the cursor. Returns
    /// whether the text changed.
    pub fn backspace(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let start = self.prev_char(self.cursor);
        self.remove(start..self.cursor)
    }

    /// Delete the selection, or the character after the cursor. Returns
    /// whether the text changed.
    pub fn delete_forward(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let end = self.next_char(self.cursor);
        self.remove(self.cursor..end)
    }

    /// Delete the selection, or back to the previous word boundary.
    /// Returns whether the text changed.
    pub fn delete_word_left(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let start = text::prev_word_boundary(self.text.as_bytes(), self.cursor);
        self.remove(start..self.cursor)
    }

    /// Delete the selection, or forward to the next word boundary.
    /// Returns whether the text changed.
    pub fn delete_word_right(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let end = text::next_word_boundary(self.text.as_bytes(), self.cursor);
        self.remove(self.cursor..end)
    }

    /// Delete the selected text, leaving the cursor where it was. Returns
    /// whether there was one.
    pub fn delete_selection(&mut self) -> bool {
        match self.selection() {
            Some(range) => self.remove(range),
            None => {
                self.anchor = None;
                false
            }
        }
    }

    /// The selected text, for the clipboard.
    pub fn copy(&self) -> Option<String> {
        self.selected_text().map(str::to_owned)
    }

    /// Remove and return the selected text, for the clipboard.
    pub fn cut(&mut self) -> Option<String> {
        let text = self.copy()?;
        self.delete_selection();
        Some(text)
    }

    /// Remove a byte range, leaving the cursor at its start. Returns
    /// whether anything was removed.
    fn remove(&mut self, range: Range<usize>) -> bool {
        self.anchor = None;
        if range.is_empty() {
            return false;
        }
        self.text.replace_range(range.clone(), "");
        self.cursor = range.start;
        true
    }

    // ----- Layout ---------------------------------------------------------

    /// The characters of the text with their display columns.
    pub fn cells(&self) -> Vec<Cell<'_>> {
        let mut column = 0;
        text::graphemes(self.text.as_bytes())
            .map(|Grapheme { range, text }| {
                let width = text::width(text, column, TAB_WIDTH);
                let cell = Cell {
                    range,
                    column,
                    width,
                    text,
                };
                column += width;
                cell
            })
            .collect()
    }

    /// The display width of the whole text.
    pub fn width(&self) -> usize {
        self.cells().last().map_or(0, |c| c.column + c.width)
    }

    /// The display column of a byte offset, snapped back to the start of
    /// the character containing it.
    pub fn column_of(&self, offset: usize) -> usize {
        let cells = self.cells();
        match cells.iter().find(|c| c.range.end > offset) {
            Some(cell) => cell.column,
            None => cells.last().map_or(0, |c| c.column + c.width),
        }
    }

    /// The byte offset of the character covering a display column, or the
    /// end of the text past its last character. A column in the middle of
    /// a wide character maps to that character's start.
    pub fn offset_at_column(&self, column: usize) -> usize {
        self.cells()
            .iter()
            .find(|c| c.column + c.width > column)
            .map_or(self.text.len(), |c| c.range.start)
    }

    /// Clamp an offset to the text and move it back to a character
    /// boundary.
    fn snap(&self, offset: usize) -> usize {
        let offset = offset.min(self.text.len());
        text::graphemes(self.text.as_bytes())
            .find(|g| g.range.end > offset)
            .map_or(self.text.len(), |g| g.range.start)
    }

    /// The offset one character right of `offset`, or the end of the text.
    fn next_char(&self, offset: usize) -> usize {
        text::graphemes(self.text.as_bytes())
            .find(|g| g.range.end > offset)
            .map_or(self.text.len(), |g| g.range.end)
    }

    /// The offset one character left of `offset`, or 0.
    fn prev_char(&self, offset: usize) -> usize {
        text::graphemes(self.text.as_bytes())
            .take_while(|g| g.range.start < offset)
            .last()
            .map_or(0, |g| g.range.start)
    }
}

fn is_line_break(c: char) -> bool {
    c == '\n' || c == '\r'
}

#[cfg(test)]
mod tests {
    use super::*;
    use Movement::*;

    #[test]
    fn inserts_and_deletes_by_character() {
        let mut edit = LineEdit::new();
        assert!(edit.insert("ab"));
        assert!(edit.insert("c"));
        assert_eq!(edit.text(), "abc");
        assert_eq!(edit.cursor(), 3);
        edit.move_cursor(Left, false);
        assert!(edit.insert("x"));
        assert_eq!(edit.text(), "abxc");
        assert!(edit.backspace());
        assert_eq!(edit.text(), "abc");
        assert!(edit.delete_forward());
        assert_eq!(edit.text(), "ab");
        assert!(!edit.delete_forward(), "nothing after the cursor");
        edit.move_cursor(Start, false);
        assert!(!edit.backspace(), "nothing before the cursor");
        assert!(!edit.insert(""));
        assert!(!edit.insert("\n"), "line breaks are dropped");
        assert_eq!(edit.text(), "ab");
    }

    #[test]
    fn line_breaks_are_dropped() {
        let edit = LineEdit::from_text("one\ntwo\r\nthree");
        assert_eq!(edit.text(), "onetwothree");
        assert_eq!(edit.cursor(), 11);
        let mut edit = LineEdit::new();
        edit.insert("a\r\nb");
        assert_eq!(edit.text(), "ab");
    }

    #[test]
    fn moves_by_character_and_word_over_graphemes() {
        let mut edit = LineEdit::from_text("e\u{301}x  foo");
        assert_eq!(edit.cursor(), 9);
        edit.move_cursor(WordLeft, false);
        assert_eq!(edit.cursor(), 6);
        edit.move_cursor(WordLeft, false);
        assert_eq!(edit.cursor(), 0);
        edit.move_cursor(Right, false);
        assert_eq!(edit.cursor(), 3, "a grapheme cluster is one character");
        edit.move_cursor(WordRight, false);
        assert_eq!(edit.cursor(), 4);
        edit.move_cursor(WordRight, false);
        assert_eq!(edit.cursor(), 9);
        edit.move_cursor(WordRight, false);
        assert_eq!(edit.cursor(), 9);
        edit.move_cursor(Start, false);
        assert_eq!(edit.cursor(), 0);
        edit.move_cursor(Left, false);
        assert_eq!(edit.cursor(), 0);
        edit.move_cursor(End, false);
        assert_eq!(edit.cursor(), 9);
        // Snapping lands on a character boundary.
        edit.set_cursor(1);
        assert_eq!(edit.cursor(), 0);
        edit.set_cursor(99);
        assert_eq!(edit.cursor(), 9);
    }

    #[test]
    fn selection_extends_collapses_and_is_replaced() {
        let mut edit = LineEdit::from_text("hello world");
        edit.move_cursor(WordLeft, true);
        assert_eq!(edit.selection(), Some(6..11));
        assert_eq!(edit.selected_text(), Some("world"));
        edit.move_cursor(Left, true);
        assert_eq!(edit.selection(), Some(5..11));
        // Moving without extending collapses to the near end.
        edit.move_cursor(Left, false);
        assert_eq!(edit.selection(), None);
        assert_eq!(edit.cursor(), 5);
        edit.move_cursor(WordRight, true);
        edit.move_cursor(Right, false);
        assert_eq!(edit.cursor(), 11);
        // Other movements just clear it.
        edit.move_cursor(Start, true);
        edit.move_cursor(WordRight, false);
        assert_eq!(edit.selection(), None);
        assert_eq!(edit.cursor(), 5);

        // Typing over a selection replaces it; deleting removes it.
        edit.set_selection(0, 5);
        assert!(edit.insert("bye"));
        assert_eq!(edit.text(), "bye world");
        assert_eq!(edit.cursor(), 3);
        edit.set_selection(9, 4);
        assert!(edit.backspace());
        assert_eq!(edit.text(), "bye ");
        assert_eq!(edit.cursor(), 4);
        edit.select_all();
        assert_eq!(edit.selection(), Some(0..4));
        assert!(edit.delete_forward());
        assert_eq!(edit.text(), "");
        assert!(!edit.delete_selection());
    }

    #[test]
    fn word_deletion_and_selection() {
        let mut edit = LineEdit::from_text("foo bar  baz");
        assert!(edit.delete_word_left());
        assert_eq!(edit.text(), "foo bar  ");
        edit.set_cursor(4);
        assert!(edit.delete_word_right());
        assert_eq!(edit.text(), "foo   ");
        assert!(edit.delete_word_right());
        assert_eq!(edit.text(), "foo ");
        assert!(!edit.delete_word_right());
        edit.select_word_at(1);
        assert_eq!(edit.selected_text(), Some("foo"));
        assert_eq!(edit.cursor(), 3);
        edit.select_word_at(4);
        assert_eq!(edit.selected_text(), Some(" "));
        assert!(edit.delete_word_left(), "deletes the selection first");
        assert_eq!(edit.text(), "foo");
    }

    #[test]
    fn cut_and_copy() {
        let mut edit = LineEdit::from_text("abc");
        assert_eq!(edit.copy(), None);
        assert_eq!(edit.cut(), None);
        edit.set_selection(1, 3);
        assert_eq!(edit.copy().as_deref(), Some("bc"));
        assert_eq!(edit.text(), "abc");
        assert_eq!(edit.cut().as_deref(), Some("bc"));
        assert_eq!(edit.text(), "a");
        assert_eq!(edit.cursor(), 1);
    }

    #[test]
    fn layout_columns() {
        let edit = LineEdit::from_text("a\t한b");
        let cells = edit.cells();
        let columns: Vec<(usize, usize)> = cells.iter().map(|c| (c.column, c.width)).collect();
        assert_eq!(columns, vec![(0, 1), (1, 3), (4, 2), (6, 1)]);
        assert_eq!(edit.width(), 7);
        assert_eq!(edit.column_of(0), 0);
        assert_eq!(edit.column_of(2), 4);
        assert_eq!(edit.column_of(3), 4, "inside a character: its start");
        assert_eq!(edit.column_of(6), 7);
        assert_eq!(edit.offset_at_column(0), 0);
        assert_eq!(edit.offset_at_column(2), 1, "inside the tab");
        assert_eq!(
            edit.offset_at_column(5),
            2,
            "the second cell of a wide character"
        );
        assert_eq!(edit.offset_at_column(6), 5);
        assert_eq!(edit.offset_at_column(7), 6);
        assert_eq!(edit.offset_at_column(99), 6);
    }
}
