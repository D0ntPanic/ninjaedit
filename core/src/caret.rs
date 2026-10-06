//! Moving a cursor through lines of text, and selecting with it.
//!
//! The editor, and the views that show text without editing it, move a
//! cursor the same way: by character (a grapheme cluster; see the
//! [`text`] module), by word, by line keeping the column, by
//! page, and to either end of a line or of the text. [`Movement`] names
//! those moves for every model that has a cursor to move, and a [`Caret`]
//! makes them over any [`LineSource`]: lines of text it can read but
//! doesn't own, so a model keeps its text however suits it and lends it
//! to the caret for each move.
//!
//! Positions are [`TextPos`]es: a line, and a byte offset into it, which
//! the caret keeps on a character boundary. A column, for vertical moves
//! and for a frontend placing the cursor, is a display column in terminal
//! cells (see [`text::width`]): wide characters take two, tabs run to the
//! next tab stop. Moving up and down remembers the column the move began
//! at, so crossing a short line and coming back returns to it.
//!
//! The selection is an anchor, its fixed end, and the cursor, its moving
//! end, as in the editor: extending the selection moves the cursor and
//! leaves the anchor where the selection began. It runs from one to the
//! other, between characters, so a selection from the start of a line to
//! the start of the next is that line and its line break.

use crate::text::{self, LineLayout};
use std::borrow::Cow;
use std::ops::Range;

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

/// Lines of text a [`Caret`] moves through: each without its line break.
pub trait LineSource {
    fn line_count(&self) -> usize;

    /// The bytes of a line; empty for a line past the end.
    fn line(&self, index: usize) -> Cow<'_, [u8]>;
}

impl<S: AsRef<str>> LineSource for [S] {
    fn line_count(&self) -> usize {
        self.len()
    }

    fn line(&self, index: usize) -> Cow<'_, [u8]> {
        Cow::Borrowed(self.get(index).map_or(&[], |line| line.as_ref().as_bytes()))
    }
}

/// A place in lines of text: a line, counted from zero, and a byte
/// offset into it. Positions order by line and then by byte, which is
/// reading order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextPos {
    pub line: usize,
    pub byte: usize,
}

impl TextPos {
    pub fn new(line: usize, byte: usize) -> TextPos {
        TextPos { line, byte }
    }
}

/// What of one line a selection covers: see [`Caret::selection_on`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineSelection {
    /// The bytes of the line selected; empty when only its line break
    /// is.
    pub bytes: Range<usize>,
    /// Whether the selection runs on past the end of the line, taking
    /// its line break.
    pub past_end: bool,
}

/// A cursor and the selection it makes in lines of text; see the
/// [module documentation](self).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caret {
    cursor: TextPos,
    /// The fixed end of the selection; the cursor is the moving end.
    /// `None` when nothing is selected.
    anchor: Option<TextPos>,
    /// The column vertical movement aims for, remembered across lines
    /// that are too short to reach it. Cleared by anything that isn't a
    /// vertical movement.
    desired_column: Option<usize>,
    tab_width: usize,
}

impl Caret {
    /// A caret at the start of the text, with tab stops every
    /// `tab_width` columns.
    pub fn new(tab_width: usize) -> Caret {
        Caret {
            cursor: TextPos::default(),
            anchor: None,
            desired_column: None,
            tab_width,
        }
    }

    pub fn cursor(&self) -> TextPos {
        self.cursor
    }

    /// The selection anchor: the end of the selection that doesn't move.
    /// `None` when nothing is selected.
    pub fn anchor(&self) -> Option<TextPos> {
        self.anchor
    }

    /// What is selected, in reading order, or `None` when the selection
    /// is empty.
    pub fn selection(&self) -> Option<Range<TextPos>> {
        let anchor = self.anchor?;
        if anchor == self.cursor {
            return None;
        }
        Some(anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    pub fn tab_width(&self) -> usize {
        self.tab_width
    }

    pub fn set_tab_width(&mut self, tab_width: usize) {
        self.tab_width = tab_width;
        self.desired_column = None;
    }

    /// The display column of a position.
    pub fn column_of(&self, lines: &(impl LineSource + ?Sized), pos: TextPos) -> usize {
        self.layout(lines, pos.line).column_of(pos.byte)
    }

    /// The position of the character covering a display column of a
    /// line, or the end of the line past its last character. A line past
    /// the end is the last line.
    pub fn position_at_column(
        &self,
        lines: &(impl LineSource + ?Sized),
        line: usize,
        column: usize,
    ) -> TextPos {
        let line = line.min(lines.line_count().saturating_sub(1));
        TextPos::new(line, self.layout(lines, line).offset_at_column(column))
    }

    /// Place the cursor, clearing the selection. The position is clamped
    /// to the text and moved back to a character boundary.
    pub fn set_cursor(&mut self, lines: &(impl LineSource + ?Sized), pos: TextPos) {
        self.cursor = self.snap(lines, pos);
        self.anchor = None;
        self.desired_column = None;
    }

    /// Select from `anchor` to `cursor`, leaving the cursor at `cursor`.
    /// Both are clamped and snapped as by [`set_cursor`](Self::set_cursor).
    pub fn set_selection(
        &mut self,
        lines: &(impl LineSource + ?Sized),
        anchor: TextPos,
        cursor: TextPos,
    ) {
        self.anchor = Some(self.snap(lines, anchor));
        self.cursor = self.snap(lines, cursor);
        self.desired_column = None;
    }

    /// Select all of the text, leaving the cursor at its end.
    pub fn select_all(&mut self, lines: &(impl LineSource + ?Sized)) {
        let end = self.end_of_text(lines);
        self.set_selection(lines, TextPos::default(), end);
    }

    /// Clear the selection, leaving the cursor where it is.
    pub fn clear_selection(&mut self) {
        self.anchor = None;
        self.desired_column = None;
    }

    /// Select the word at a position, as a double-click does: the run of
    /// word characters (or of spaces, or of punctuation) containing it,
    /// or at the end of a line the run ending there; see
    /// [`text::word_at`]. The cursor ends up at the end of the word. On an
    /// empty line nothing is selected and the cursor moves there.
    pub fn select_word_at(&mut self, lines: &(impl LineSource + ?Sized), pos: TextPos) {
        let pos = self.snap(lines, pos);
        let word = text::word_at(&lines.line(pos.line), pos.byte);
        self.set_selection(
            lines,
            TextPos::new(pos.line, word.start),
            TextPos::new(pos.line, word.end),
        );
    }

    /// Move the cursor, clearing any selection. Moving left or right out
    /// of a selection collapses it to its start or end.
    pub fn move_cursor(&mut self, lines: &(impl LineSource + ?Sized), movement: Movement) {
        if let Some(range) = self.selection() {
            let collapsed = match movement {
                Movement::Left => Some(range.start),
                Movement::Right => Some(range.end),
                _ => None,
            };
            if let Some(pos) = collapsed {
                self.cursor = pos;
                self.anchor = None;
                self.desired_column = None;
                return;
            }
        }
        self.apply(lines, movement, false);
    }

    /// Move the cursor while keeping (or starting) a selection from where
    /// the cursor was.
    pub fn extend_selection(&mut self, lines: &(impl LineSource + ?Sized), movement: Movement) {
        self.apply(lines, movement, true);
    }

    /// What of a line the selection covers, for a frontend to highlight:
    /// `None` for a line outside it, and for the line it ends at the
    /// start of.
    pub fn selection_on(
        &self,
        lines: &(impl LineSource + ?Sized),
        line: usize,
    ) -> Option<LineSelection> {
        let range = self.selection()?;
        if line < range.start.line || line > range.end.line {
            return None;
        }
        if line == range.end.line && range.end.byte == 0 && line != range.start.line {
            return None;
        }
        let len = lines.line(line).len();
        let from = if line == range.start.line {
            range.start.byte
        } else {
            0
        };
        let past_end = line < range.end.line;
        let to = if past_end { len } else { range.end.byte };
        Some(LineSelection {
            bytes: from.min(len)..to.min(len),
            past_end,
        })
    }

    /// The selected text, its lines joined by `\n`, or `None` when the
    /// selection is empty. Invalid UTF-8 is replaced.
    pub fn selected_text(&self, lines: &(impl LineSource + ?Sized)) -> Option<String> {
        let range = self.selection()?;
        let mut text = Vec::new();
        for line in range.start.line..=range.end.line {
            if let Some(selected) = self.selection_on(lines, line) {
                text.extend_from_slice(&lines.line(line)[selected.bytes]);
                if selected.past_end {
                    text.push(b'\n');
                }
            }
        }
        Some(String::from_utf8_lossy(&text).into_owned())
    }

    fn apply(&mut self, lines: &(impl LineSource + ?Sized), movement: Movement, extend: bool) {
        if lines.line_count() == 0 {
            return;
        }
        let vertical = matches!(
            movement,
            Movement::Up | Movement::Down | Movement::PageUp(_) | Movement::PageDown(_)
        );
        let mut column = vertical.then(|| {
            self.desired_column
                .unwrap_or_else(|| self.column_of(lines, self.cursor))
        });
        let line = self.cursor.line;
        let mut vertical_target = |delta: isize| {
            match self.vertical_target(lines, line, column.unwrap_or(0), delta) {
                Some(pos) => pos,
                // Ran off the first or last line: to that end of the
                // text, which is a horizontal move, so forget the column.
                None => {
                    column = None;
                    if delta < 0 {
                        TextPos::default()
                    } else {
                        self.end_of_text(lines)
                    }
                }
            }
        };
        let target = match movement {
            Movement::Left => self.prev_char(lines, self.cursor),
            Movement::Right => self.next_char(lines, self.cursor),
            Movement::Up => vertical_target(-1),
            Movement::Down => vertical_target(1),
            Movement::PageUp(height) => vertical_target(-(height.max(1) as isize)),
            Movement::PageDown(height) => vertical_target(height.max(1) as isize),
            Movement::WordLeft => self.word_left(lines, self.cursor),
            Movement::WordRight => self.word_right(lines, self.cursor),
            Movement::LineStart => self.line_start_target(lines, self.cursor),
            Movement::LineEnd => TextPos::new(line, lines.line(line).len()),
            Movement::DocumentStart => TextPos::default(),
            Movement::DocumentEnd => self.end_of_text(lines),
        };
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = target;
        self.desired_column = column;
    }

    // ----- Position arithmetic -------------------------------------------

    fn layout(&self, lines: &(impl LineSource + ?Sized), line: usize) -> LineLayout {
        LineLayout::new(&lines.line(line), self.tab_width)
    }

    fn end_of_text(&self, lines: &(impl LineSource + ?Sized)) -> TextPos {
        let last = lines.line_count().saturating_sub(1);
        TextPos::new(last, lines.line(last).len())
    }

    /// Clamp a position to the text and move it back to the nearest
    /// character boundary.
    fn snap(&self, lines: &(impl LineSource + ?Sized), pos: TextPos) -> TextPos {
        let line = pos.line.min(lines.line_count().saturating_sub(1));
        let layout = self.layout(lines, line);
        let byte = match layout.cell_at(pos.byte) {
            Some(i) => layout.cells()[i].range.start,
            None => layout.len(),
        };
        TextPos::new(line, byte)
    }

    /// The position one character to the right, crossing to the next
    /// line at the end of a line. The position itself at the end of the
    /// text.
    fn next_char(&self, lines: &(impl LineSource + ?Sized), pos: TextPos) -> TextPos {
        let layout = self.layout(lines, pos.line);
        match layout.cell_at(pos.byte) {
            Some(i) => TextPos::new(pos.line, layout.cells()[i].range.end),
            None if pos.line + 1 < lines.line_count() => TextPos::new(pos.line + 1, 0),
            None => pos,
        }
    }

    /// The position one character to the left, crossing to the end of
    /// the previous line at the start of a line. The position itself at
    /// the start of the text.
    fn prev_char(&self, lines: &(impl LineSource + ?Sized), pos: TextPos) -> TextPos {
        if pos.byte == 0 {
            return match pos.line.checked_sub(1) {
                Some(line) => TextPos::new(line, lines.line(line).len()),
                None => pos,
            };
        }
        // The character ending at (or containing) the position.
        let layout = self.layout(lines, pos.line);
        let cells = layout.cells();
        let i = cells.partition_point(|c| c.range.end < pos.byte);
        TextPos::new(pos.line, cells[i.min(cells.len() - 1)].range.start)
    }

    /// The next word boundary, crossing to the next line from the end of
    /// a line.
    fn word_right(&self, lines: &(impl LineSource + ?Sized), pos: TextPos) -> TextPos {
        let bytes = lines.line(pos.line);
        if pos.byte >= bytes.len() {
            return self.next_char(lines, pos);
        }
        TextPos::new(pos.line, text::next_word_boundary(&bytes, pos.byte))
    }

    /// The previous word boundary, crossing to the previous line from the
    /// start of a line.
    fn word_left(&self, lines: &(impl LineSource + ?Sized), pos: TextPos) -> TextPos {
        if pos.byte == 0 {
            return self.prev_char(lines, pos);
        }
        let bytes = lines.line(pos.line);
        TextPos::new(pos.line, text::prev_word_boundary(&bytes, pos.byte))
    }

    /// Where [`Movement::LineStart`] goes from a position: the end of the
    /// line's leading whitespace, unless the position is already at or
    /// before it (but not at the very start of the line), in which case
    /// the start of the line. A line that is all whitespace has no first
    /// character to go to, so its start is the only target.
    fn line_start_target(&self, lines: &(impl LineSource + ?Sized), pos: TextPos) -> TextPos {
        let bytes = lines.line(pos.line);
        let indent = bytes
            .iter()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count();
        let byte = if indent == bytes.len() || (pos.byte > 0 && pos.byte <= indent) {
            0
        } else {
            indent
        };
        TextPos::new(pos.line, byte)
    }

    /// The target of moving `delta` lines from `line` aiming for
    /// `column`, or `None` if `line` is already the first (or last) line.
    fn vertical_target(
        &self,
        lines: &(impl LineSource + ?Sized),
        line: usize,
        column: usize,
        delta: isize,
    ) -> Option<TextPos> {
        let last = lines.line_count().saturating_sub(1);
        let target = if delta < 0 {
            line.saturating_sub(delta.unsigned_abs())
        } else {
            (line + delta as usize).min(last)
        };
        (target != line).then(|| self.position_at_column(lines, target, column))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caret_at(lines: &[&str], line: usize, byte: usize) -> Caret {
        let mut caret = Caret::new(4);
        caret.set_cursor(lines, TextPos::new(line, byte));
        caret
    }

    fn moved(caret: &mut Caret, lines: &[&str], movement: Movement) -> (usize, usize) {
        caret.move_cursor(lines, movement);
        (caret.cursor().line, caret.cursor().byte)
    }

    #[test]
    fn characters_are_graphemes_and_lines_wrap() {
        let lines: &[&str] = &["ae\u{301}", "", "한b"];
        let mut caret = caret_at(lines, 0, 0);
        assert_eq!(moved(&mut caret, lines, Movement::Right), (0, 1));
        assert_eq!(
            moved(&mut caret, lines, Movement::Right),
            (0, 4),
            "e and its mark"
        );
        assert_eq!(
            moved(&mut caret, lines, Movement::Right),
            (1, 0),
            "on to the next line"
        );
        assert_eq!(
            moved(&mut caret, lines, Movement::Right),
            (2, 0),
            "over an empty one"
        );
        assert_eq!(moved(&mut caret, lines, Movement::Right), (2, 3));
        assert_eq!(moved(&mut caret, lines, Movement::Right), (2, 4));
        assert_eq!(
            moved(&mut caret, lines, Movement::Right),
            (2, 4),
            "the end stays"
        );
        assert_eq!(moved(&mut caret, lines, Movement::Left), (2, 3));
        assert_eq!(moved(&mut caret, lines, Movement::Left), (2, 0));
        assert_eq!(moved(&mut caret, lines, Movement::Left), (1, 0));
        assert_eq!(
            moved(&mut caret, lines, Movement::Left),
            (0, 4),
            "the previous end"
        );
        assert_eq!(moved(&mut caret, lines, Movement::Left), (0, 1));
        assert_eq!(moved(&mut caret, lines, Movement::Left), (0, 0));
        assert_eq!(
            moved(&mut caret, lines, Movement::Left),
            (0, 0),
            "the start stays"
        );
    }

    #[test]
    fn positions_snap_to_characters_and_clamp_to_the_text() {
        let lines: &[&str] = &["ae\u{301}x", "y"];
        let caret = caret_at(lines, 0, 2);
        assert_eq!(caret.cursor(), TextPos::new(0, 1), "inside e's mark");
        let caret = caret_at(lines, 9, 9);
        assert_eq!(caret.cursor(), TextPos::new(1, 1));
        let caret = caret_at(lines, 0, 99);
        assert_eq!(caret.cursor(), TextPos::new(0, 5));
    }

    #[test]
    fn words_cross_line_ends() {
        let lines: &[&str] = &["foo bar", "  baz"];
        let mut caret = caret_at(lines, 0, 0);
        assert_eq!(moved(&mut caret, lines, Movement::WordRight), (0, 3));
        assert_eq!(moved(&mut caret, lines, Movement::WordRight), (0, 7));
        assert_eq!(moved(&mut caret, lines, Movement::WordRight), (1, 0));
        assert_eq!(moved(&mut caret, lines, Movement::WordRight), (1, 5));
        assert_eq!(moved(&mut caret, lines, Movement::WordLeft), (1, 2));
        assert_eq!(moved(&mut caret, lines, Movement::WordLeft), (1, 0));
        assert_eq!(moved(&mut caret, lines, Movement::WordLeft), (0, 7));
        assert_eq!(moved(&mut caret, lines, Movement::WordLeft), (0, 4));
    }

    #[test]
    fn vertical_moves_keep_the_column_they_began_at() {
        // Columns, not bytes: a tab runs to column 4, and the wide
        // character takes columns 2 and 3.
        let lines: &[&str] = &["0123456789", "ab", "\txyz", "a한b", "end"];
        let mut caret = caret_at(lines, 0, 6);
        assert_eq!(
            moved(&mut caret, lines, Movement::Down),
            (1, 2),
            "short: its end"
        );
        assert_eq!(
            moved(&mut caret, lines, Movement::Down),
            (2, 3),
            "column 6 is the z"
        );
        assert_eq!(moved(&mut caret, lines, Movement::Up), (1, 2));
        assert_eq!(
            moved(&mut caret, lines, Movement::Up),
            (0, 6),
            "back to column 6"
        );
        let mut caret = caret_at(lines, 0, 2);
        caret.move_cursor(lines, Movement::PageDown(3));
        assert_eq!(
            caret.cursor(),
            TextPos::new(3, 1),
            "column 2 is in the wide character"
        );
        assert_eq!(caret.column_of(lines, caret.cursor()), 1);
        // A horizontal move forgets the column.
        caret.move_cursor(lines, Movement::Right);
        assert_eq!(moved(&mut caret, lines, Movement::Down), (4, 3));
        // Off either end: to that end of the text.
        assert_eq!(moved(&mut caret, lines, Movement::Down), (4, 3));
        assert_eq!(moved(&mut caret, lines, Movement::PageUp(10)), (0, 3));
        assert_eq!(moved(&mut caret, lines, Movement::Up), (0, 0));
        assert_eq!(moved(&mut caret, lines, Movement::DocumentEnd), (4, 3));
        assert_eq!(moved(&mut caret, lines, Movement::DocumentStart), (0, 0));
    }

    #[test]
    fn line_start_alternates_between_the_indent_and_the_start() {
        let lines: &[&str] = &["    code();", "   ", "flush"];
        let mut caret = caret_at(lines, 0, 8);
        assert_eq!(moved(&mut caret, lines, Movement::LineStart), (0, 4));
        assert_eq!(moved(&mut caret, lines, Movement::LineStart), (0, 0));
        assert_eq!(moved(&mut caret, lines, Movement::LineStart), (0, 4));
        assert_eq!(moved(&mut caret, lines, Movement::LineEnd), (0, 11));
        let mut caret = caret_at(lines, 1, 2);
        assert_eq!(
            moved(&mut caret, lines, Movement::LineStart),
            (1, 0),
            "all spaces"
        );
        let mut caret = caret_at(lines, 2, 3);
        assert_eq!(moved(&mut caret, lines, Movement::LineStart), (2, 0));
    }

    #[test]
    fn a_selection_extends_collapses_and_reads_back() {
        let lines: &[&str] = &["one", "two", "three"];
        let mut caret = caret_at(lines, 0, 1);
        assert_eq!(caret.selection(), None);
        caret.extend_selection(lines, Movement::Down);
        caret.extend_selection(lines, Movement::Right);
        assert_eq!(caret.anchor(), Some(TextPos::new(0, 1)));
        assert_eq!(
            caret.selection(),
            Some(TextPos::new(0, 1)..TextPos::new(1, 2))
        );
        assert_eq!(caret.selected_text(lines).as_deref(), Some("ne\ntw"));
        let on = |caret: &Caret, line| caret.selection_on(lines, line);
        assert_eq!(
            on(&caret, 0),
            Some(LineSelection {
                bytes: 1..3,
                past_end: true
            })
        );
        assert_eq!(
            on(&caret, 1),
            Some(LineSelection {
                bytes: 0..2,
                past_end: false
            })
        );
        assert_eq!(on(&caret, 2), None);
        // Left collapses to the start, Right to the end.
        let mut left = caret.clone();
        left.move_cursor(lines, Movement::Left);
        assert_eq!(
            (left.cursor(), left.selection()),
            (TextPos::new(0, 1), None)
        );
        caret.move_cursor(lines, Movement::Right);
        assert_eq!(
            (caret.cursor(), caret.selection()),
            (TextPos::new(1, 2), None)
        );
        // Backwards from the anchor, and whole lines: from the start
        // of one to the start of another takes its line break and
        // nothing of the line it stops at.
        caret.set_selection(lines, TextPos::new(2, 0), TextPos::new(0, 0));
        assert_eq!(caret.selected_text(lines).as_deref(), Some("one\ntwo\n"));
        assert_eq!(on(&caret, 2), None);
        // Back to the anchor is nothing.
        caret.set_selection(lines, TextPos::new(1, 1), TextPos::new(1, 1));
        assert_eq!(caret.selected_text(lines), None);
    }

    #[test]
    fn words_and_all_are_selected_whole() {
        let lines: &[&str] = &["let foo_bar = 1;", "", "x"];
        let mut caret = Caret::new(4);
        caret.select_word_at(lines, TextPos::new(0, 6));
        assert_eq!(caret.selected_text(lines).as_deref(), Some("foo_bar"));
        assert_eq!(caret.cursor(), TextPos::new(0, 11));
        caret.select_word_at(lines, TextPos::new(1, 0));
        assert_eq!(caret.selection(), None, "nothing on an empty line");
        assert_eq!(caret.cursor(), TextPos::new(1, 0));
        caret.select_all(lines);
        assert_eq!(
            caret.selected_text(lines).as_deref(),
            Some("let foo_bar = 1;\n\nx")
        );
        assert_eq!(caret.cursor(), TextPos::new(2, 1));
        caret.clear_selection();
        assert_eq!(caret.selection(), None);
        assert_eq!(caret.cursor(), TextPos::new(2, 1));
    }

    #[test]
    fn no_lines_leave_the_caret_at_the_start() {
        let lines: &[&str] = &[];
        let mut caret = Caret::new(4);
        for movement in [Movement::Right, Movement::Down, Movement::DocumentEnd] {
            caret.move_cursor(lines, movement);
            caret.extend_selection(lines, movement);
        }
        assert_eq!(caret.cursor(), TextPos::default());
        assert_eq!(caret.selection(), None);
        caret.select_all(lines);
        assert_eq!(caret.selected_text(lines), None);
    }
}
