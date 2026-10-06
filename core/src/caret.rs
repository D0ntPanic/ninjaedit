//! Moving a cursor through lines of text, and selecting with it.
//!
//! The editor, and the views that show text without editing it, move a
//! cursor the same way: by character (a grapheme cluster; see the
//! [`text`](crate::text) module), by word, by line keeping the column, by
//! page, and to either end of a line or of the text. [`Movement`] names
//! those moves for every model that has a cursor to move.

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
