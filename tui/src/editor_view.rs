//! The editor view: draws one [`Editor`] into a rectangle and turns keyboard
//! and mouse input within that rectangle into editor actions.
//!
//! The view owns what the model doesn't care about: the scroll position, the
//! screen regions from the last render (needed to hit-test the mouse), and
//! any drag in progress. Everything about the text itself, including the
//! cursor and selection, lives in the model.
//!
//! Scrolling follows the cursor after any action that moves it, but not
//! after plain scrolling (mouse wheel, scrollbar), so the user can look
//! around without losing their place. While a search is being previewed
//! the cursor stays put, so the view instead scrolls to the search's
//! current match whenever that changes, centering it (clear of the search
//! box, which covers the top rows) if it isn't already in view, so the
//! match comes with some context around it. The vertical scrollbar is always
//! shown; the horizontal one appears only when a visible line doesn't fit,
//! or the view is scrolled horizontally. Only the visible lines are measured
//! for that decision, so it stays cheap on files with huge lines.
//!
//! Horizontal scrolling stops at the longest visible line plus a couple of
//! columns, so a trackpad's stray sideways motion can't push the text out
//! of view. Scrolling vertically can lower that limit, and then the view
//! keeps its horizontal position rather than snapping back; the limit only
//! ever blocks further scrolling to the right.
//!
//! A click puts the cursor under the pointer and a drag selects; two
//! clicks in quick succession on one cell select the word there.
//!
//! The gutter to the left of the text is laid out as
//! `[breakpoint][line number][space][guide]`. The breakpoint column is
//! blank for now; a debugger can later mark it with a red circle. The
//! guide is a vertical line right against the text, showing where the
//! text's left edge is; git line status can later replace the guide glyph
//! on a line with a thin colored block. A view embedded in a page (the
//! changes page's commit message) can do without the gutter altogether
//! (see [`EditorView::set_gutter`]): the text then starts at the left
//! edge, and only the scrollbars remain around it.

use crate::clicks::ClickTracker;
use crate::clipboard::Clipboard;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ninjaedit_core::{Cell, ConflictStep, Editor, Movement, Position};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget};
use std::ops::Range;

/// Lines scrolled per mouse wheel notch.
const WHEEL_LINES: usize = 3;
/// Columns scrolled per horizontal wheel notch.
const WHEEL_COLUMNS: usize = 4;
/// Columns the view may scroll past the longest visible line: one for the
/// cursor at the end of the line, one for padding.
const HSCROLL_SLACK: usize = 2;

/// What a render scrolls to bring into view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reveal {
    /// The cursor, after an action moved it: scrolled just into view.
    Cursor(Position),
    /// A search match that has just become current, as the position of its
    /// start and the column of its end. Centered vertically if it was out
    /// of view; horizontally, scrolled as little as possible while showing
    /// its start and as much of the rest as fits.
    Match(Position, usize),
}

/// What the mouse is dragging, from a button press until its release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    None,
    /// Extending the selection through the text.
    Selection,
    VerticalScrollbar,
    HorizontalScrollbar,
}

pub struct EditorView {
    editor: Editor,
    /// First visible line.
    scroll_line: usize,
    /// First visible display column.
    scroll_col: usize,
    /// Whether the next render should scroll to bring the cursor into view.
    follow_cursor: bool,
    /// The search match the view last scrolled to show, so that it only
    /// scrolls when the match changes and not on every redraw.
    revealed_match: Option<Range<usize>>,
    /// Rows at the top of the view hidden under an overlay such as the
    /// search box, which a revealed match is kept clear of.
    covered_rows: u16,
    /// A range the next render centers on, after a jump from outside the
    /// view (a project search result), selected as a match would be.
    jump: Option<Range<usize>>,
    drag: Drag,
    /// Presses in the text, to notice a double-click.
    clicks: ClickTracker,
    /// Whether the gutter (line numbers and the guide) is drawn.
    show_gutter: bool,
    // Screen regions from the last render. The gutter is zero-sized
    // while hidden.
    gutter: Rect,
    text: Rect,
    vscroll: Rect,
    /// Zero-sized when the horizontal scrollbar is hidden.
    hscroll: Rect,
    /// The total width in columns the horizontal scrollbar represents.
    hscroll_total: usize,
}

/// The guide glyph drawn between the gutter and the text.
const GUIDE: &str = "│";
/// Gutter columns besides the line number: the breakpoint column before
/// it, and the space and guide after it.
const GUTTER_EXTRA: u16 = 3;

impl EditorView {
    pub fn new(editor: Editor) -> EditorView {
        EditorView {
            editor,
            scroll_line: 0,
            scroll_col: 0,
            follow_cursor: true,
            revealed_match: None,
            covered_rows: 0,
            jump: None,
            drag: Drag::None,
            clicks: ClickTracker::default(),
            show_gutter: true,
            gutter: Rect::default(),
            text: Rect::default(),
            vscroll: Rect::default(),
            hscroll: Rect::default(),
            hscroll_total: 0,
        }
    }

    /// Draw the gutter, with its line numbers and guide, or leave it
    /// out so the text starts at the view's left edge, as an editor
    /// embedded in a page wants.
    pub fn set_gutter(&mut self, shown: bool) {
        self.show_gutter = shown;
    }

    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    /// The editor, for an action that may move the cursor: the view
    /// scrolls to show it at the next render.
    pub fn editor_mut(&mut self) -> &mut Editor {
        self.follow_cursor = true;
        &mut self.editor
    }

    /// The editor, for an action that leaves the cursor where it is, such
    /// as a search preview, so the view doesn't scroll back to it.
    pub fn editor_mut_in_place(&mut self) -> &mut Editor {
        &mut self.editor
    }

    /// Tell the view how many of its top rows an overlay hides, so that a
    /// search match it scrolls to isn't put under the overlay.
    pub fn set_covered_rows(&mut self, rows: u16) {
        self.covered_rows = rows;
    }

    /// Select `start..end` and, at the next render, bring it into view the
    /// way a search match is: centered vertically unless it's already on
    /// screen, and scrolled sideways as little as will show it. For
    /// jumping to a place found outside the view, such as a project
    /// search result.
    pub fn select_and_center(&mut self, start: usize, end: usize) {
        self.editor.set_selection(start, end);
        let range = self.editor.selection().unwrap_or(start..end);
        self.jump = Some(range);
        self.follow_cursor = true;
    }

    /// Move the cursor to the start of a line (counted from zero, clamped
    /// to the buffer) and, at the next render, bring it into view the way
    /// a jump does: centered vertically unless it's already on screen.
    pub fn go_to_line(&mut self, line: usize) {
        self.editor.go_to_line(line);
        let cursor = self.editor.cursor();
        self.jump = Some(cursor..cursor);
        self.follow_cursor = true;
    }

    /// Move the cursor to the start of the next (or previous) merge
    /// conflict, wrapping around the buffer, and bring it into view the
    /// way a jump does. See [`Editor::next_conflict`].
    pub fn step_conflict(&mut self, forward: bool) -> ConflictStep {
        let step = if forward {
            self.editor.next_conflict()
        } else {
            self.editor.previous_conflict()
        };
        if step != ConflictStep::NoConflicts {
            let cursor = self.editor.cursor();
            self.jump = Some(cursor..cursor);
            self.follow_cursor = true;
        }
        step
    }

    /// Move the cursor to a line and a character within it (both counted
    /// from zero and clamped to what's there) and, at the next render,
    /// bring it into view the way a jump does. For following a location
    /// named outside the editor, such as a compiler error's.
    pub fn go_to_line_column(&mut self, line: usize, column: usize) {
        let buffer = self.editor.buffer();
        let line = line.min(buffer.line_count().saturating_sub(1));
        let content = buffer.line_content_range(line);
        let text = buffer.line_text(line);
        let byte = text
            .char_indices()
            .nth(column)
            .map_or(text.len(), |(i, _)| i);
        let offset = (content.start + byte).min(content.end);
        self.editor.set_cursor(offset);
        self.jump = Some(offset..offset);
        self.follow_cursor = true;
    }

    /// At the next render, bring the cursor into view the way a jump
    /// does: centered vertically unless it's already on screen. For when
    /// something other than the user has moved it, such as a merge of
    /// changes from disk leaving it on a conflict.
    pub fn reveal_cursor(&mut self) {
        let cursor = self.editor.cursor();
        self.jump = Some(cursor..cursor);
        self.follow_cursor = true;
    }

    /// First visible display column.
    #[cfg(test)]
    pub fn scroll_col(&self) -> usize {
        self.scroll_col
    }

    // ----- Rendering ------------------------------------------------------

    /// Draw the view into `area`. Returns the screen position of the
    /// terminal cursor if it should be shown: when the model's cursor is in
    /// view and nothing is selected. With a selection, the highlight alone
    /// marks the cursor's end; a blinking cursor there only obscures it.
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) -> Option<ScreenPosition> {
        let text_style = Style::default()
            .fg(theme.view_text)
            .bg(theme.view_background);
        buf.set_style(area, text_style);
        let line_count = self.editor.buffer().line_count();
        let number_width = digits(line_count) as u16;
        let gutter_width = if self.show_gutter {
            number_width + GUTTER_EXTRA
        } else {
            0
        };
        // Gutter, at least one text column, and the vertical scrollbar.
        if area.width < gutter_width + 2 || area.height == 0 {
            return None;
        }
        let text_width = (area.width - gutter_width - 1) as usize;
        let cursor = self.editor.cursor_position();
        // Where to scroll to, if anywhere: a search match that has become
        // current since the last render (whether previewed or selected,
        // since a selection's cursor at the match's end says nothing
        // about where its start is), or else the cursor after it moved.
        let current_match = self.editor.search().and_then(|search| search.current());
        let reveal = match (self.jump.take(), &current_match) {
            (Some(range), _) => {
                let start = self.editor.position_of_offset(range.start);
                let end = self.editor.position_of_offset(range.end);
                Some(Reveal::Match(start, end.column))
            }
            (None, Some(range)) if current_match != self.revealed_match => {
                let start = self.editor.position_of_offset(range.start);
                let end = self.editor.position_of_offset(range.end);
                Some(Reveal::Match(start, end.column))
            }
            _ if self.follow_cursor => Some(Reveal::Cursor(cursor)),
            _ => None,
        };
        self.revealed_match = current_match.clone();

        // Settle the vertical scroll position and decide whether the
        // horizontal scrollbar is needed. Showing it costs a row, which can
        // hide the cursor line, so lay out a second time with the row taken
        // away. The second layout is final even if the lines it shows would
        // fit without the scrollbar, since going back and forth would never
        // settle.
        // Each layout starts from the scroll position as it was, since
        // the first one clamps it for the full height and the second
        // has to be able to reach the last line with a row fewer.
        let full_height = area.height as usize;
        let wanted_scroll_line = self.scroll_line;
        let mut show_hscroll = false;
        let mut height;
        let mut lines: Vec<Vec<Cell>>;
        let mut max_width;
        loop {
            height = full_height - usize::from(show_hscroll);
            self.scroll_line = wanted_scroll_line;
            match reveal {
                Some(Reveal::Cursor(target)) => self.scroll_to_line(target.line, height),
                Some(Reveal::Match(target, _)) => {
                    self.center_line_if_hidden(target.line, height);
                }
                None => {}
            }
            self.scroll_line = self.scroll_line.min(line_count.saturating_sub(height));
            lines = (self.scroll_line..(self.scroll_line + height).min(line_count))
                .map(|line| self.editor.line_cells(line))
                .collect();
            max_width = lines
                .iter()
                .map(|cells| line_width(cells))
                .max()
                .unwrap_or(0);
            let needed = (self.scroll_col > 0 || max_width > text_width) && full_height > 1;
            if needed && !show_hscroll {
                show_hscroll = true;
                continue;
            }
            break;
        }
        match reveal {
            Some(Reveal::Cursor(target)) => {
                if target.column < self.scroll_col {
                    self.scroll_col = target.column;
                } else if target.column >= self.scroll_col + text_width {
                    self.scroll_col = target.column + 1 - text_width;
                }
            }
            Some(Reveal::Match(start, end)) => {
                self.scroll_to_columns(start.column, end, text_width);
            }
            None => {}
        }
        self.follow_cursor = false;

        let height = height as u16;
        self.gutter = Rect::new(area.x, area.y, gutter_width, height);
        self.text = Rect::new(area.x + gutter_width, area.y, text_width as u16, height);
        self.vscroll = Rect::new(area.right() - 1, area.y, 1, height);
        self.hscroll = if show_hscroll {
            Rect::new(self.text.x, area.bottom() - 1, self.text.width, 1)
        } else {
            Rect::default()
        };

        // The guide runs the full height of the view, past the end of the
        // file, since it marks the edge of the text area rather than a line.
        if self.show_gutter {
            let guide_x = self.text.x - 1;
            let guide_style = text_style.fg(theme.gutter_guide);
            for y in area.y..area.y + height {
                buf.set_string(guide_x, y, GUIDE, guide_style);
            }
        }

        let selection = self.editor.selection();
        // A theme can force one text color over the selection for contrast;
        // otherwise the text keeps its syntax color. Bold and italic stay
        // either way.
        let selected = Style::default().bg(theme.selection_background);
        // Search matches on the visible lines, ascending. Cells are walked
        // in the same order, so one index keeps up with them.
        let matches = match self.editor.search() {
            Some(search) if !lines.is_empty() => {
                let first = self.editor.buffer().offset_of_line(self.scroll_line);
                let last = self
                    .editor
                    .buffer()
                    .line_range(self.scroll_line + lines.len() - 1)
                    .end;
                search.matches_in(first..last)
            }
            _ => Vec::new(),
        };
        let mut next_match = 0;
        let found = Style::default().bg(theme.find_result_background);
        let found_current = Style::default().bg(theme.highlighted_find_result_background);
        for (row, cells) in lines.iter().enumerate() {
            let line = self.scroll_line + row;
            let y = area.y + row as u16;

            // Line number, right-aligned after the breakpoint column.
            if self.show_gutter {
                let number = format!("{:>width$}", line + 1, width = number_width as usize);
                let color = if line == cursor.line {
                    theme.active_line_number
                } else {
                    theme.inactive_line_number
                };
                buf.set_string(area.x + 1, y, &number, text_style.fg(color));
            }

            // The lines of a merge conflict are tinted by side, across
            // the whole text area; the text keeps its syntax colors.
            let row_style = match self
                .editor
                .conflict_side(line)
                .and_then(|side| theme.conflict_background(side))
            {
                Some(background) => {
                    let row_style = text_style.bg(background);
                    buf.set_style(Rect::new(self.text.x, y, self.text.width, 1), row_style);
                    row_style
                }
                None => text_style,
            };

            let visible = self.scroll_col..self.scroll_col + text_width;
            for cell in cells {
                if cell.width == 0 || cell.column + cell.width <= visible.start {
                    continue;
                }
                if cell.column >= visible.end {
                    break;
                }
                let syntax = theme.syntax(cell.kind);
                while next_match < matches.len() && matches[next_match].end <= cell.range.start {
                    next_match += 1;
                }
                let in_match = matches
                    .get(next_match)
                    .is_some_and(|m| m.start <= cell.range.start);
                let in_current = current_match
                    .as_ref()
                    .is_some_and(|m| m.contains(&cell.range.start));
                // The selection wins over a match, and the current match
                // over the others.
                let style = match &selection {
                    Some(range) if range.contains(&cell.range.start) => {
                        let style = syntax.apply(selected);
                        match theme.selection_text {
                            Some(color) => style.fg(color),
                            None => style,
                        }
                    }
                    _ if in_current => syntax.apply(found_current),
                    _ if in_match => syntax.apply(found),
                    _ => syntax.apply(row_style),
                };
                let fits = cell.column >= visible.start && cell.column + cell.width <= visible.end;
                let start = cell.column.max(visible.start);
                let end = (cell.column + cell.width).min(visible.end);
                let x = self.text.x + (start - visible.start) as u16;
                if cell.text == "\t" || !fits {
                    // Tabs are blank; a wide character cut off by the edge
                    // of the view shows as blank for the part that fits.
                    for x in x..x + (end - start) as u16 {
                        buf[(x, y)].set_symbol(" ").set_style(style);
                    }
                } else {
                    buf.set_string(x, y, &cell.text, style);
                }
            }

            // A selection that continues onto the next line highlights one
            // cell past the end of this one, standing in for the line break.
            if let Some(range) = &selection {
                let end = self.editor.buffer().line_content_range(line).end;
                if line + 1 < line_count && range.start <= end && end < range.end {
                    let column = line_width(cells);
                    if visible.contains(&column) {
                        let x = self.text.x + (column - visible.start) as u16;
                        buf[(x, y)].set_symbol(" ").set_style(selected);
                    }
                }
            }
        }

        let track = text_style.fg(theme.scroll_bar_track);
        let thumb = text_style.fg(theme.scroll_bar_color);
        let mut state = ScrollbarState::new(self.max_scroll_line() + 1)
            .position(self.scroll_line)
            .viewport_content_length(height as usize);
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .track_style(track)
            .thumb_style(thumb)
            .render(self.vscroll, buf, &mut state);

        if show_hscroll {
            // The bar covers the reachable range, whichever is larger: the
            // longest line and its slack, or what is already scrolled to.
            self.hscroll_total = (max_width + HSCROLL_SLACK).max(self.scroll_col + text_width);
            let mut state = ScrollbarState::new(self.hscroll_total - text_width + 1)
                .position(self.scroll_col)
                .viewport_content_length(text_width);
            Scrollbar::new(ScrollbarOrientation::HorizontalBottom)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("─"))
                .thumb_symbol("█")
                .track_style(track)
                .thumb_style(thumb)
                .render(self.hscroll, buf, &mut state);
        }

        let visible_line =
            cursor.line >= self.scroll_line && cursor.line < self.scroll_line + height as usize;
        let visible_col =
            cursor.column >= self.scroll_col && cursor.column < self.scroll_col + text_width;
        (visible_line && visible_col && selection.is_none()).then(|| {
            ScreenPosition::new(
                self.text.x + (cursor.column - self.scroll_col) as u16,
                self.text.y + (cursor.line - self.scroll_line) as u16,
            )
        })
    }

    /// Scroll vertically so that `line` is visible in a view `height` rows
    /// tall.
    fn scroll_to_line(&mut self, line: usize, height: usize) {
        let height = height.max(1);
        if line < self.scroll_line {
            self.scroll_line = line;
        } else if line >= self.scroll_line + height {
            self.scroll_line = line + 1 - height;
        }
    }

    /// Scroll vertically so that `line` is in the middle of the part of a
    /// view `height` rows tall that isn't covered by an overlay, unless it
    /// is already visible there.
    fn center_line_if_hidden(&mut self, line: usize, height: usize) {
        let covered = (self.covered_rows as usize).min(height.saturating_sub(1));
        let visible = self.scroll_line + covered..self.scroll_line + height.max(1);
        if !visible.contains(&line) {
            self.scroll_line = line.saturating_sub(covered + (height - covered) / 2);
        }
    }

    /// Scroll horizontally so that the columns `start..end` are visible in
    /// a view `text_width` columns wide, scrolling as little to the right
    /// as will do: a range already in view stays put, one that fits is
    /// brought in with its end at the right edge (or not scrolled at all
    /// if it fits from the left margin), and one wider than the view has
    /// its start at the left edge and shows as much as fits.
    fn scroll_to_columns(&mut self, start: usize, end: usize, text_width: usize) {
        let text_width = text_width.max(1);
        if start >= self.scroll_col && end <= self.scroll_col + text_width {
            return;
        }
        self.scroll_col = if end - start > text_width {
            start
        } else {
            end.saturating_sub(text_width)
        };
    }

    /// The furthest the view can scroll down: the last line at the bottom.
    /// Measured for the view's full height, and then, when the lines at
    /// the bottom would need the horizontal scrollbar, for the row fewer
    /// it leaves; the last render's height alone would stop a row short
    /// of the end when the bar appears only once the end is in view.
    fn max_scroll_line(&self) -> usize {
        let line_count = self.editor.buffer().line_count();
        let full_height = (self.text.height + self.hscroll.height).max(1) as usize;
        let max = line_count.saturating_sub(full_height);
        let text_width = self.text.width as usize;
        let needs_bar = self.scroll_col > 0
            || (max..line_count).any(|line| self.editor.line_width(line) > text_width);
        if needs_bar && full_height > 1 {
            line_count.saturating_sub(full_height - 1)
        } else {
            max
        }
    }

    fn scroll_by(&mut self, lines: isize) {
        self.scroll_line = self
            .scroll_line
            .saturating_add_signed(lines)
            .min(self.max_scroll_line());
    }

    /// Scroll sideways. Scrolling right stops at the limit, but a position
    /// already past it, left there by a vertical scroll, stays where it is.
    fn scroll_horizontally_by(&mut self, columns: isize) {
        let target = self.scroll_col.saturating_add_signed(columns);
        self.scroll_col = if columns > 0 {
            target.min(self.max_scroll_col().max(self.scroll_col))
        } else {
            target
        };
    }

    /// The furthest the view can scroll right: the longest visible line
    /// plus its slack at the right edge. Measured from the lines visible
    /// now, not at the last render, since a burst of wheel events can mix
    /// vertical and horizontal motion between redraws.
    fn max_scroll_col(&self) -> usize {
        let text_width = self.text.width as usize;
        let last = (self.scroll_line + self.text.height.max(1) as usize)
            .min(self.editor.buffer().line_count());
        let max_width = (self.scroll_line..last)
            .map(|line| self.editor.line_width(line))
            .max()
            .unwrap_or(0);
        (max_width + HSCROLL_SLACK).saturating_sub(text_width)
    }

    // ----- Keyboard -------------------------------------------------------

    /// Handle a key press. Returns whether the key meant something to the
    /// editor. `clipboard` is the application's clipboard, read by paste and
    /// replaced by copy and cut.
    pub fn handle_key(&mut self, key: KeyEvent, clipboard: &mut Clipboard) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        // Page up and down scroll the view by a page and move the cursor
        // by the same amount, so the cursor stays on its screen row. The
        // page is one line short of the view, so the line at the edge of
        // the old page is still visible at the opposite edge of the new
        // one, as vim does, to keep some context across the jump.
        let page = self.text.height.saturating_sub(1).max(1) as usize;
        let paging = match key.code {
            KeyCode::PageUp => Some((-(page as isize), Movement::PageUp(page))),
            KeyCode::PageDown => Some((page as isize, Movement::PageDown(page))),
            _ => None,
        };
        if let Some((lines, movement)) = paging {
            self.scroll_by(lines);
            if shift {
                self.editor.extend_selection(movement);
            } else {
                self.editor.move_cursor(movement);
            }
            // The scroll may have been clamped at either end of the buffer,
            // so still make sure the cursor ends up in view.
            self.follow_cursor = true;
            return true;
        }

        let movement = match key.code {
            KeyCode::Left if ctrl => Some(Movement::WordLeft),
            KeyCode::Right if ctrl => Some(Movement::WordRight),
            KeyCode::Left => Some(Movement::Left),
            KeyCode::Right => Some(Movement::Right),
            KeyCode::Up if !ctrl => Some(Movement::Up),
            KeyCode::Down if !ctrl => Some(Movement::Down),
            KeyCode::Home if ctrl => Some(Movement::DocumentStart),
            KeyCode::End if ctrl => Some(Movement::DocumentEnd),
            KeyCode::Home => Some(Movement::LineStart),
            KeyCode::End => Some(Movement::LineEnd),
            _ => None,
        };
        if let Some(movement) = movement {
            if shift {
                self.editor.extend_selection(movement);
            } else {
                self.editor.move_cursor(movement);
            }
            self.follow_cursor = true;
            return true;
        }

        match key.code {
            // Ctrl+Up/Down scroll the view without moving the cursor.
            KeyCode::Up => self.scroll_by(-1),
            KeyCode::Down => self.scroll_by(1),
            KeyCode::Char(c) if ctrl => match c.to_ascii_lowercase() {
                'a' => self.editor.select_all(),
                'z' if shift => {
                    self.editor.redo();
                }
                'z' => {
                    self.editor.undo();
                }
                'y' => {
                    self.editor.redo();
                }
                'c' => {
                    if let Some(text) = self.editor.copy() {
                        clipboard.set(text);
                    }
                }
                'x' => {
                    if let Some(text) = self.editor.cut() {
                        clipboard.set(text);
                    }
                }
                'v' => {
                    if let Some(text) = clipboard.get() {
                        self.editor.paste(&text);
                    }
                }
                _ => return false,
            },
            KeyCode::Char(c) if !alt => self.editor.insert_char(c),
            KeyCode::Enter => self.editor.insert_char('\n'),
            // Shift+Tab arrives as BackTab from most terminals, or as Tab
            // with the shift modifier under enhanced keyboard protocols.
            KeyCode::BackTab => self.editor.outdent(),
            KeyCode::Tab if shift => self.editor.outdent(),
            KeyCode::Tab => self.editor.indent(),
            KeyCode::Backspace => self.editor.backspace(),
            KeyCode::Delete => self.editor.delete_forward(),
            KeyCode::Esc => self.editor.clear_selection(),
            _ => return false,
        }
        if !matches!(key.code, KeyCode::Up | KeyCode::Down) {
            self.follow_cursor = true;
        }
        true
    }

    // ----- Mouse ----------------------------------------------------------

    /// Whether the mouse position is over any part of the view.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        let p = ScreenPosition::new(x, y);
        self.gutter.contains(p)
            || self.text.contains(p)
            || self.vscroll.contains(p)
            || self.hscroll.contains(p)
    }

    /// Whether a mouse drag is in progress, in which case the view wants
    /// drag and release events even outside its area.
    pub fn is_dragging(&self) -> bool {
        self.drag != Drag::None
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        let (x, y) = (mouse.column, mouse.row);
        let at = ScreenPosition::new(x, y);
        match mouse.kind {
            MouseEventKind::ScrollDown => self.scroll_by(WHEEL_LINES as isize),
            MouseEventKind::ScrollUp => self.scroll_by(-(WHEEL_LINES as isize)),
            MouseEventKind::ScrollRight => self.scroll_horizontally_by(WHEEL_COLUMNS as isize),
            MouseEventKind::ScrollLeft => self.scroll_horizontally_by(-(WHEEL_COLUMNS as isize)),
            MouseEventKind::Down(MouseButton::Left) => {
                if self.vscroll.contains(at) {
                    self.drag = Drag::VerticalScrollbar;
                    self.scroll_vertically_to(y);
                } else if self.hscroll.contains(at) {
                    self.drag = Drag::HorizontalScrollbar;
                    self.scroll_horizontally_to(x);
                } else if self.text.contains(at) || self.gutter.contains(at) {
                    self.drag = Drag::Selection;
                    let offset = self.offset_at(x, y);
                    if self.clicks.press(x, y) == 2 {
                        self.editor.select_word_at(offset);
                    } else if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                        let anchor = self.editor.anchor().unwrap_or(self.editor.cursor());
                        self.editor.set_selection(anchor, offset);
                    } else {
                        self.editor.set_cursor(offset);
                    }
                    self.follow_cursor = true;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => match self.drag {
                Drag::Selection => {
                    let anchor = self.editor.anchor().unwrap_or(self.editor.cursor());
                    self.editor.set_selection(anchor, self.offset_at(x, y));
                    self.follow_cursor = true;
                }
                Drag::VerticalScrollbar => self.scroll_vertically_to(y),
                Drag::HorizontalScrollbar => self.scroll_horizontally_to(x),
                Drag::None => {}
            },
            MouseEventKind::Up(MouseButton::Left) => self.drag = Drag::None,
            _ => {}
        }
    }

    /// The buffer offset under a screen position. Positions outside the text
    /// area map to the nearest edge, one line or column beyond the view, so
    /// dragging past an edge scrolls in that direction.
    fn offset_at(&self, x: u16, y: u16) -> usize {
        let line = if y < self.text.y {
            self.scroll_line.saturating_sub(1)
        } else if y >= self.text.bottom() {
            self.scroll_line + self.text.height as usize
        } else {
            self.scroll_line + (y - self.text.y) as usize
        };
        let column = if x < self.text.x {
            self.scroll_col.saturating_sub(1)
        } else if x >= self.text.right() {
            self.scroll_col + self.text.width as usize
        } else {
            self.scroll_col + (x - self.text.x) as usize
        };
        self.editor.offset_of_position(Position { line, column })
    }

    /// Scroll so the vertical scrollbar's thumb is around row `y`.
    fn scroll_vertically_to(&mut self, y: u16) {
        let track = self.vscroll.height.max(1) as usize;
        let row = y.saturating_sub(self.vscroll.y) as usize;
        let max = self.max_scroll_line();
        self.scroll_line = (row * (max + 1) / track).min(max);
    }

    /// Scroll so the horizontal scrollbar's thumb is around column `x`. The
    /// bar's range is the reachable one, so dragging can't go past the
    /// limit either, and dragging right from a position beyond the limit
    /// stays put.
    fn scroll_horizontally_to(&mut self, x: u16) {
        let track = self.hscroll.width.max(1) as usize;
        let column = x.saturating_sub(self.hscroll.x) as usize;
        let max = self.hscroll_total.saturating_sub(self.text.width as usize);
        let target = (column * (max + 1) / track).min(max);
        if target > self.scroll_col {
            self.scroll_col = target.min(self.max_scroll_col().max(self.scroll_col));
        } else {
            self.scroll_col = target;
        }
    }
}

/// The display width of a laid-out line.
fn line_width(cells: &[Cell]) -> usize {
    cells.last().map(|c| c.column + c.width).unwrap_or(0)
}

/// The number of decimal digits needed to show `n`.
fn digits(n: usize) -> usize {
    n.max(1).ilog10() as usize + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use ninjaedit_core::FileBuffer;

    fn draw(
        view: &mut EditorView,
        width: u16,
        height: u16,
    ) -> (Vec<String>, Option<ScreenPosition>) {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let cursor = view.render(area, &mut buf, &Theme::default());
        let rows = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect();
        (rows, cursor)
    }

    #[test]
    fn the_gutter_can_be_left_out() {
        let mut view = EditorView::new(Editor::new(FileBuffer::from_text("one\ntwo\n")));
        let (rows, cursor) = draw(&mut view, 20, 3);
        // With the gutter: the breakpoint column, the number, a space,
        // and the guide before the text.
        assert!(rows[0].starts_with(" 1 │one"), "{rows:#?}");
        assert_eq!(cursor, Some(ScreenPosition::new(4, 0)));
        // Without it the text starts at the edge, and a click there
        // still lands in the text.
        view.set_gutter(false);
        let (rows, cursor) = draw(&mut view, 20, 3);
        assert!(rows[0].starts_with("one "), "{rows:#?}");
        assert!(rows[1].starts_with("two "), "{rows:#?}");
        assert_eq!(cursor, Some(ScreenPosition::new(0, 0)));
        view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            view.editor.cursor_position(),
            Position { line: 1, column: 2 }
        );
        assert!(view.contains(0, 0));
    }
}
