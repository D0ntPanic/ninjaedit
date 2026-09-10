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
//! around without losing their place. The vertical scrollbar is always
//! shown; the horizontal one appears only when a visible line doesn't fit,
//! or the view is scrolled horizontally. Only the visible lines are measured
//! for that decision, so it stays cheap on files with huge lines.
//!
//! The gutter to the left of the text is laid out as
//! `[breakpoint][line number][space][guide]`. The breakpoint column is
//! blank for now; a debugger can later mark it with a red circle. The
//! guide is a vertical line right against the text, showing where the
//! text's left edge is; git line status can later replace the guide glyph
//! on a line with a thin colored block.

use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ninjaedit_core::{Cell, Editor, Movement, Position};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget};

/// Lines scrolled per mouse wheel notch.
const WHEEL_LINES: usize = 3;
/// Columns scrolled per horizontal wheel notch.
const WHEEL_COLUMNS: usize = 4;

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
    drag: Drag,
    // Screen regions from the last render.
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
            drag: Drag::None,
            gutter: Rect::default(),
            text: Rect::default(),
            vscroll: Rect::default(),
            hscroll: Rect::default(),
            hscroll_total: 0,
        }
    }

    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    pub fn editor_mut(&mut self) -> &mut Editor {
        self.follow_cursor = true;
        &mut self.editor
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
        let gutter_width = number_width + GUTTER_EXTRA;
        // Gutter, at least one text column, and the vertical scrollbar.
        if area.width < gutter_width + 2 || area.height == 0 {
            return None;
        }
        let text_width = (area.width - gutter_width - 1) as usize;
        let cursor = self.editor.cursor_position();

        // Settle the vertical scroll position and decide whether the
        // horizontal scrollbar is needed. Showing it costs a row, which can
        // hide the cursor line, so lay out a second time with the row taken
        // away. The second layout is final even if the lines it shows would
        // fit without the scrollbar, since going back and forth would never
        // settle.
        let full_height = area.height as usize;
        let mut show_hscroll = false;
        let mut height;
        let mut lines: Vec<Vec<Cell>>;
        let mut max_width;
        loop {
            height = full_height - usize::from(show_hscroll);
            if self.follow_cursor {
                self.scroll_to_line(cursor.line, height);
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
        if self.follow_cursor {
            if cursor.column < self.scroll_col {
                self.scroll_col = cursor.column;
            } else if cursor.column >= self.scroll_col + text_width {
                self.scroll_col = cursor.column + 1 - text_width;
            }
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
        let guide_x = self.text.x - 1;
        let guide_style = text_style.fg(theme.gutter_guide);
        for y in area.y..area.y + height {
            buf.set_string(guide_x, y, GUIDE, guide_style);
        }

        let selection = self.editor.selection();
        // A theme can force one text color over the selection for contrast;
        // otherwise the text keeps its syntax color. Bold and italic stay
        // either way.
        let selected = Style::default().bg(theme.selection_background);
        for (row, cells) in lines.iter().enumerate() {
            let line = self.scroll_line + row;
            let y = area.y + row as u16;

            // Line number, right-aligned after the breakpoint column.
            let number = format!("{:>width$}", line + 1, width = number_width as usize);
            let color = if line == cursor.line {
                theme.active_line_number
            } else {
                theme.inactive_line_number
            };
            buf.set_string(area.x + 1, y, &number, text_style.fg(color));

            let visible = self.scroll_col..self.scroll_col + text_width;
            for cell in cells {
                if cell.width == 0 || cell.column + cell.width <= visible.start {
                    continue;
                }
                if cell.column >= visible.end {
                    break;
                }
                let syntax = theme.syntax(cell.kind);
                let style = match &selection {
                    Some(range) if range.contains(&cell.range.start) => {
                        let style = syntax.apply(selected);
                        match theme.selection_text {
                            Some(color) => style.fg(color),
                            None => style,
                        }
                    }
                    _ => syntax.apply(text_style),
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
            self.hscroll_total = max_width.max(self.scroll_col + text_width);
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

    /// The furthest the view can scroll down: the last line at the bottom.
    fn max_scroll_line(&self) -> usize {
        self.editor
            .buffer()
            .line_count()
            .saturating_sub(self.text.height.max(1) as usize)
    }

    fn scroll_by(&mut self, lines: isize) {
        self.scroll_line = self
            .scroll_line
            .saturating_add_signed(lines)
            .min(self.max_scroll_line());
    }

    fn scroll_horizontally_by(&mut self, columns: isize) {
        self.scroll_col = self.scroll_col.saturating_add_signed(columns);
    }

    // ----- Keyboard -------------------------------------------------------

    /// Handle a key press. Returns whether the key meant something to the
    /// editor. `clipboard` is the application's clipboard, read by paste and
    /// replaced by copy and cut.
    pub fn handle_key(&mut self, key: KeyEvent, clipboard: &mut Option<String>) -> bool {
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
                        *clipboard = Some(text);
                    }
                }
                'x' => {
                    if let Some(text) = self.editor.cut() {
                        *clipboard = Some(text);
                    }
                }
                'v' => {
                    if let Some(text) = clipboard {
                        self.editor.paste(text);
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
                    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
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

    /// Scroll so the horizontal scrollbar's thumb is around column `x`.
    fn scroll_horizontally_to(&mut self, x: u16) {
        let track = self.hscroll.width.max(1) as usize;
        let column = x.saturating_sub(self.hscroll.x) as usize;
        let max = self.hscroll_total.saturating_sub(self.text.width as usize);
        self.scroll_col = (column * (max + 1) / track).min(max);
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
