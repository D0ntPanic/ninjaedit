//! A stretch of a terminal's output picked out with the mouse, to be
//! copied.
//!
//! Positions are [`Point`]s: a column on a line, where lines are numbered
//! through the terminal's history (see [`Terminal::first_line`]) rather
//! than by screen row. That way a selection stays on the text it was made
//! over while the program's output pushes that text up into the
//! scrollback, and while the frontend scrolls the view to reach more of
//! it. The selection itself is an anchor, where the drag began, and a
//! head, where the pointer is now; both cells are part of it, as in any
//! terminal.
//!
//! What the selected cells say is the terminal's to work out, since it
//! holds the rows: see [`Terminal::text_between`].
//!
//! [`Terminal::first_line`]: super::Terminal::first_line
//! [`Terminal::text_between`]: super::Terminal::text_between

use std::ops::Range;

/// A cell of the terminal's history: a column on a numbered line.
/// Points order by line and then by column, which is reading order.
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

/// The cells from where a drag began to where the pointer is, in either
/// order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    anchor: Point,
    head: Point,
}

impl Selection {
    /// A selection of the one cell a drag begins on.
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

    /// The first selected cell in reading order.
    pub fn start(&self) -> Point {
        self.anchor.min(self.head)
    }

    /// The last selected cell in reading order, itself selected.
    pub fn end(&self) -> Point {
        self.anchor.max(self.head)
    }

    /// Whether the head has left the anchor: a press that never moved
    /// selects one cell, which is a click and not worth a copy.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    pub fn contains(&self, point: Point) -> bool {
        self.start() <= point && point <= self.end()
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
        let to = if line == end.line { end.col + 1 } else { cols };
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
        assert_eq!(selection.columns_on(5, 10), Some(3..4));
        // Dragging up and left puts the head before the anchor.
        selection.extend(Point::new(2, 7));
        assert!(!selection.is_empty());
        assert_eq!(selection.start(), Point::new(2, 7));
        assert_eq!(selection.end(), Point::new(5, 3));
        assert!(selection.contains(Point::new(2, 7)));
        assert!(selection.contains(Point::new(3, 0)));
        assert!(selection.contains(Point::new(5, 3)));
        assert!(!selection.contains(Point::new(5, 4)));
        assert!(!selection.contains(Point::new(2, 6)));
    }

    #[test]
    fn columns_cover_whole_middle_rows() {
        let mut selection = Selection::new(Point::new(2, 7));
        selection.extend(Point::new(4, 1));
        assert_eq!(selection.columns_on(1, 10), None);
        assert_eq!(selection.columns_on(2, 10), Some(7..10));
        assert_eq!(selection.columns_on(3, 10), Some(0..10));
        assert_eq!(selection.columns_on(4, 10), Some(0..2));
        assert_eq!(selection.columns_on(5, 10), None);
        // A narrower screen than the selection was made on cuts it.
        assert_eq!(selection.columns_on(2, 5), None);
        assert_eq!(selection.columns_on(4, 1), Some(0..1));
        // One line, in either direction.
        let mut one = Selection::new(Point::new(3, 6));
        one.extend(Point::new(3, 2));
        assert_eq!(one.columns_on(3, 10), Some(2..7));
    }
}
