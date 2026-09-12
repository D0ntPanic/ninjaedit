//! The rows of cells that make up a terminal screen, and the operations on
//! whole rows: scrolling a region, clearing, resizing.
//!
//! Everything that moves the cursor or writes text lives in the emulator;
//! the grid only knows about rows. It is also what the scrollback holds,
//! one [`Row`] per line that has scrolled off the top.

use super::cell::{Cell, Style};

/// One row of the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// Whether the row was cut by automatic wrapping, so that it and the
    /// next row are really one line of text. Lets a copy join them back
    /// together.
    pub wrapped: bool,
}

impl Row {
    /// A row of blank cells in a style.
    pub fn blank(cols: usize, style: Style) -> Row {
        Row {
            cells: vec![Cell::blank(style); cols],
            wrapped: false,
        }
    }

    /// Cut or pad the row to `cols` cells. A wide character cut in half
    /// by the new edge becomes a blank.
    pub fn resize(&mut self, cols: usize, style: Style) {
        self.cells.resize_with(cols, || Cell::blank(style));
        if let Some(last) = self.cells.last_mut()
            && last.wide
        {
            *last = Cell::blank(last.style);
        }
    }

    /// Drop the blank, unstyled cells at the end of the row, which a row
    /// in the scrollback has no need to keep.
    pub fn trim(&mut self) {
        let keep = self
            .cells
            .iter()
            .rposition(|cell| !cell.is_default())
            .map_or(0, |i| i + 1);
        self.cells.truncate(keep);
    }

    /// The row's text with trailing spaces removed, as a copy would give
    /// it. Spacers contribute nothing, so a wide character appears once.
    pub fn text(&self) -> String {
        let mut text: String = self.cells.iter().map(|cell| cell.text.as_str()).collect();
        let end = text.trim_end_matches(' ').len();
        text.truncate(end);
        text
    }

    /// Make the row consistent after cells were inserted or removed:
    /// a spacer that lost its wide character, or a wide character that
    /// lost its spacer, becomes a blank.
    pub fn normalize(&mut self) {
        let cols = self.cells.len();
        for col in 0..cols {
            let cell = &self.cells[col];
            // A spacer with no wide character before it, or a wide
            // character with no spacer after it, is left over from an
            // insertion or deletion and becomes a blank.
            let orphan_spacer = cell.spacer && (col == 0 || !self.cells[col - 1].wide);
            let orphan_wide = cell.wide && (col + 1 >= cols || !self.cells[col + 1].spacer);
            if orphan_spacer || orphan_wide {
                self.cells[col] = Cell::blank(cell.style);
            }
        }
    }
}

/// The rows of one screen (the primary or the alternate).
#[derive(Clone, Debug)]
pub struct Grid {
    cols: usize,
    rows: Vec<Row>,
}

impl Grid {
    pub fn new(cols: usize, rows: usize) -> Grid {
        Grid {
            cols,
            rows: (0..rows)
                .map(|_| Row::blank(cols, Style::default()))
                .collect(),
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows.len()
    }

    pub fn row(&self, index: usize) -> &Row {
        &self.rows[index]
    }

    pub fn row_mut(&mut self, index: usize) -> &mut Row {
        &mut self.rows[index]
    }

    pub fn iter(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter()
    }

    /// Replace every row with blanks in a style.
    pub fn clear(&mut self, style: Style) {
        for row in &mut self.rows {
            *row = Row::blank(self.cols, style);
        }
    }

    /// Scroll the rows `top..=bottom` up by `count`, filling in blank rows
    /// at the bottom. Returns the rows that scrolled off the top, oldest
    /// first, for the caller to keep as scrollback.
    pub fn scroll_up(&mut self, top: usize, bottom: usize, count: usize, style: Style) -> Vec<Row> {
        let count = count.min(bottom + 1 - top);
        let evicted: Vec<Row> = self.rows.drain(top..top + count).collect();
        let cols = self.cols;
        self.rows.splice(
            bottom + 1 - count..bottom + 1 - count,
            (0..count).map(|_| Row::blank(cols, style)),
        );
        evicted
    }

    /// Scroll the rows `top..=bottom` down by `count`, filling in blank
    /// rows at the top. The rows pushed off the bottom are lost.
    pub fn scroll_down(&mut self, top: usize, bottom: usize, count: usize, style: Style) {
        let count = count.min(bottom + 1 - top);
        self.rows.drain(bottom + 1 - count..bottom + 1);
        let cols = self.cols;
        self.rows
            .splice(top..top, (0..count).map(|_| Row::blank(cols, style)));
    }

    /// Change the width, cutting or padding every row.
    pub fn set_cols(&mut self, cols: usize) {
        self.cols = cols;
        for row in &mut self.rows {
            row.resize(cols, Style::default());
        }
    }

    /// Take `count` rows off the top, oldest first.
    pub fn remove_top(&mut self, count: usize) -> Vec<Row> {
        self.rows.drain(..count.min(self.rows.len())).collect()
    }

    /// Take `count` rows off the bottom.
    pub fn remove_bottom(&mut self, count: usize) {
        let keep = self.rows.len().saturating_sub(count);
        self.rows.truncate(keep);
    }

    /// Put rows back on top, in the order given, resized to fit.
    pub fn insert_top(&mut self, rows: Vec<Row>) {
        let cols = self.cols;
        self.rows.splice(
            0..0,
            rows.into_iter().map(|mut row| {
                row.resize(cols, Style::default());
                row
            }),
        );
    }

    /// Add blank rows at the bottom.
    pub fn pad_bottom(&mut self, count: usize) {
        let cols = self.cols;
        self.rows
            .extend((0..count).map(|_| Row::blank(cols, Style::default())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_row(text: &str) -> Row {
        let mut row = Row::blank(text.chars().count(), Style::default());
        for (cell, c) in row.cells.iter_mut().zip(text.chars()) {
            cell.text = c.to_string().into();
        }
        row
    }

    fn texts(grid: &Grid) -> Vec<String> {
        grid.iter().map(Row::text).collect()
    }

    #[test]
    fn scrolls_regions() {
        let mut grid = Grid::new(3, 5);
        for (i, row) in grid.rows.iter_mut().enumerate() {
            *row = text_row(&format!("r{i} "));
        }
        let evicted = grid.scroll_up(1, 3, 1, Style::default());
        assert_eq!(evicted.iter().map(Row::text).collect::<Vec<_>>(), ["r1"]);
        assert_eq!(texts(&grid), ["r0", "r2", "r3", "", "r4"]);
        grid.scroll_down(0, 4, 2, Style::default());
        assert_eq!(texts(&grid), ["", "", "r0", "r2", "r3"]);
        // More than the region holds just clears it.
        let evicted = grid.scroll_up(2, 3, 10, Style::default());
        assert_eq!(evicted.len(), 2);
        assert_eq!(texts(&grid), ["", "", "", "", "r3"]);
    }

    #[test]
    fn trims_and_normalizes_rows() {
        let mut row = text_row("ab   ");
        row.trim();
        assert_eq!(row.cells.len(), 2);
        assert_eq!(row.text(), "ab");
        let mut row = Row::blank(4, Style::default());
        row.cells[0].text = "한".into();
        row.cells[0].wide = true;
        row.cells[1] = Cell::spacer(Style::default());
        row.cells[3] = Cell::spacer(Style::default());
        row.normalize();
        assert!(row.cells[0].wide && row.cells[1].spacer);
        assert!(!row.cells[3].spacer, "a spacer without its wide character");
        row.cells[1] = Cell::blank(Style::default());
        row.normalize();
        assert!(!row.cells[0].wide, "a wide character without its spacer");
        assert_eq!(row.cells[0].text, " ");
        // Cutting a wide character in half blanks it.
        let mut row = Row::blank(4, Style::default());
        row.cells[2].text = "한".into();
        row.cells[2].wide = true;
        row.cells[3] = Cell::spacer(Style::default());
        row.resize(3, Style::default());
        assert_eq!(row.cells.len(), 3);
        assert!(!row.cells[2].wide);
        assert_eq!(row.text(), "");
    }
}
