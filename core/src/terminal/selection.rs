//! A stretch of a terminal's output picked out with the mouse, to be
//! copied.
//!
//! Positions are [`Point`]s: a column on a line, where lines are numbered
//! through the terminal's history (see [`Terminal::first_line`]) rather
//! than by screen row. That way a selection stays on the text it was made
//! over while the program's output pushes that text up into the
//! scrollback, and while the frontend scrolls the view to reach more of
//! it. The selection itself is an anchor, where the drag began, and a
//! head, where the pointer is now. As in the editor, each is a boundary
//! between cells rather than a cell: the selection runs from the one to
//! the other and stops short of the cell under the head, so dragging from
//! the start of one line to the start of the next selects exactly that
//! line, and dragging across one cell selects one character.
//!
//! What the selected cells say is the terminal's to work out, since it
//! holds the rows: see [`Terminal::text_between`].
//!
//! [`Terminal::first_line`]: super::Terminal::first_line
//! [`Terminal::text_between`]: super::Terminal::text_between

use std::ops::Range;

/// A boundary between cells of the terminal's history: the left edge of
/// a column on a numbered line, with `col` one past the last column for
/// the row's end. Points order by line and then by column, which is
/// reading order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    pub line: usize,
    pub col: usize,
}

impl Point {
    pub fn new(line: usize, col: usize) -> Point {
        Point { line, col }
    }
}

/// The cells from where a drag began up to where the pointer is, in
/// either order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    anchor: Point,
    head: Point,
}

impl Selection {
    /// An empty selection at the boundary a drag begins on.
    pub fn new(anchor: Point) -> Selection {
        Selection {
            anchor,
            head: anchor,
        }
    }

    /// Move the head to where the pointer is now.
    pub fn extend(&mut self, head: Point) {
        self.head = head;
    }

    pub fn anchor(&self) -> Point {
        self.anchor
    }

    pub fn head(&self) -> Point {
        self.head
    }

    /// Where the selection begins, in reading order.
    pub fn start(&self) -> Point {
        self.anchor.min(self.head)
    }

    /// Where the selection stops, in reading order: the cell there is
    /// not selected.
    pub fn end(&self) -> Point {
        self.anchor.max(self.head)
    }

    /// Whether nothing is selected: the head is back at the anchor, as
    /// after a click.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Whether the cell at `point` is selected.
    pub fn contains(&self, point: Point) -> bool {
        self.start() <= point && point < self.end()
    }

    /// The columns selected on `line` when rows are `cols` wide, for a
    /// frontend to highlight: the whole row of a line inside the
    /// selection, the tail of the first line and the head of the last, or
    /// `None` for a line outside it. The range never goes past the row.
    pub fn columns_on(&self, line: usize, cols: usize) -> Option<Range<usize>> {
        let (start, end) = (self.start(), self.end());
        if line < start.line || line > end.line {
            return None;
        }
        let from = if line == start.line { start.col } else { 0 };
        let to = if line == end.line { end.col } else { cols };
        let range = from.min(cols)..to.min(cols);
        (!range.is_empty()).then_some(range)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_its_ends_by_reading_order() {
        let mut selection = Selection::new(Point::new(5, 3));
        assert!(selection.is_empty());
        assert_eq!(selection.columns_on(5, 10), None);
        // Across one cell is one character.
        selection.extend(Point::new(5, 4));
        assert!(!selection.is_empty());
        assert_eq!(selection.columns_on(5, 10), Some(3..4));
        // Dragging up and left puts the head before the anchor.
        selection.extend(Point::new(2, 7));
        assert_eq!(selection.start(), Point::new(2, 7));
        assert_eq!(selection.end(), Point::new(5, 3));
        assert!(selection.contains(Point::new(2, 7)));
        assert!(selection.contains(Point::new(3, 0)));
        assert!(selection.contains(Point::new(5, 2)));
        assert!(!selection.contains(Point::new(5, 3)));
        assert!(!selection.contains(Point::new(2, 6)));
        // Back to the anchor is nothing again.
        selection.extend(Point::new(5, 3));
        assert!(selection.is_empty());
    }

    #[test]
    fn columns_cover_whole_middle_rows() {
        let mut selection = Selection::new(Point::new(2, 7));
        selection.extend(Point::new(4, 1));
        assert_eq!(selection.columns_on(1, 10), None);
        assert_eq!(selection.columns_on(2, 10), Some(7..10));
        assert_eq!(selection.columns_on(3, 10), Some(0..10));
        assert_eq!(selection.columns_on(4, 10), Some(0..1));
        assert_eq!(selection.columns_on(5, 10), None);
        // A narrower screen than the selection was made on cuts it.
        assert_eq!(selection.columns_on(2, 5), None);
        assert_eq!(selection.columns_on(4, 1), Some(0..1));
        // Whole lines: from the start of one to the start of another
        // highlights nothing on the latter.
        let mut lines = Selection::new(Point::new(1, 0));
        lines.extend(Point::new(3, 0));
        assert_eq!(lines.columns_on(2, 10), Some(0..10));
        assert_eq!(lines.columns_on(3, 10), None);
        // One line, in either direction.
        let mut one = Selection::new(Point::new(3, 6));
        one.extend(Point::new(3, 2));
        assert_eq!(one.columns_on(3, 10), Some(2..6));
    }
}
