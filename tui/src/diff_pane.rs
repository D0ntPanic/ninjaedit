//! What the git pages share for showing text: the diff of a file, drawn
//! the same way on the git log page (a commit's file) and the changes
//! page (a file of the working tree or the index), and the pieces it
//! is built from, which the pages' other panes use too.
//!
//! A diff is highlighted as the file would be, over the theme's
//! `diff-added-` and `diff-removed-background` colors, with the old and
//! new line numbers in a gutter that stays put while the text scrolls
//! sideways. Where lines are hidden between changes a row says how
//! many, with buttons to reveal more: ▲ from the change below it
//! upward, ▼ from the change above it downward, or all of them (the
//! gap's [`GapAction`]s); the pane hit-tests clicks against the
//! [`Button`]s the render records. The diff's cursor stops on each
//! button in turn, which is drawn as a dialog's focused button is while
//! the pane has the keyboard, and Enter presses it.
//!
//! A submodule's diff has no lines: it is the graph of the submodule's
//! commits the change moved over, drawn as the log draws its commits
//! (see the `commit_row` module), the commit the submodule now points
//! at styled as HEAD is in the log, under a line saying which commit
//! it moved from and to. On the changes page the submodule may hold
//! uncommitted changes of its own as well, which are easy to miss
//! behind the graph, so they are listed under it, a file a row with
//! its status letter as the file lists have them, and where to go to
//! commit them.
//!
//! Both pages show the diff in a [`ContentPane`], which shows lines of
//! text (a commit's description, a directory's files) and notes in
//! place of content as well, and keeps the pane's scrolling, with a
//! scrollbar down its right edge ([`VScroll`], always shown, as the
//! editor's is) saying where in the content it is and how much of it
//! fits. A diff has a cursor and a selection (see the core crate's
//! `git::diff_model` module), which the keys and the mouse work as in
//! the editor, without the editing: the arrows, Home and End, and Page
//! Up and Page Down move the cursor, with Shift they select, a click
//! puts the cursor under the pointer and a drag selects, and a click
//! or drag in the line numbers selects whole lines. The view follows
//! the cursor; the wheel and the scrollbars scroll without moving it.
//! The selection is drawn in the theme's selection colors over the
//! text, and Ctrl+C copies it as shown, without the hidden lines. On a
//! row of hidden lines the cursor stops on each of its buttons, and
//! Enter presses the one it is on. Lines of text only scroll.
//!
//! [`HScroll`] is the sideways scrolling of a pane, with the editor's
//! rules (see `EditorView`): the limit is set by the longest line
//! visible now plus a little slack, not by the longest line there is,
//! and a scrollbar shows along the bottom while anything is out of
//! view. The rest is the layout of a line's characters with tabs
//! expanded ([`layout_cells`] and [`draw_cells`]), pieces of styled
//! text drawn end to end ([`draw_pieces`]), and the small sums the
//! pages' layouts are made of.

use crate::clicks::ClickTracker;
use crate::clipboard::Clipboard;
use crate::commit_row::{CommitLine, Highlight, commit_extent, draw_commit_line, lane_cap};
use crate::palette::palette_background;
use crate::status::StatusLine;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ninjaedit_core::git::{
    ChangeKind, Commit, DiffModel, DiffRow, EXPAND_LINES, FileDiff, GapAction, LineKind,
    RANGE_LIMIT, SubmoduleRange, UncommittedChange, Unshown, short_id,
};
use ninjaedit_core::{Movement, Token, TokenKind, text};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget};

/// Columns scrolled sideways by ← and →.
pub(crate) const ARROW_COLUMNS: usize = 4;
/// Columns a pane may scroll past the longest visible line, as in the
/// editor.
pub(crate) const HSCROLL_SLACK: usize = 2;
pub(crate) const TAB_WIDTH: usize = 4;
/// A button drawn in a diff's gap row: which row and gap, and what
/// pressing it reveals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Button {
    row: usize,
    gap: usize,
    action: GapAction,
}

impl Button {
    /// Press the button, as a click does: the cursor goes to it, and
    /// what it stands for is revealed.
    fn press(self, model: &mut DiffModel) {
        model.put_cursor_on_action(self.row, self.action);
        model.take_action(self.gap, self.action);
    }
}

/// Put a diff's selection on the clipboard, saying so; nothing without
/// one.
pub(crate) fn copy(content: Shown<'_>, clipboard: &mut Clipboard) -> Option<StatusLine> {
    let Shown::Diff(model) = content else {
        return None;
    };
    let text = model.selected_text()?;
    let notice = StatusLine::copied(&text);
    clipboard.set(text);
    Some(notice)
}

/// A drawn piece of text: what and in which style.
pub(crate) type Piece = (String, Style);

/// The sideways scrolling of a pane, with the editor's rules (see
/// `EditorView`): the limit is set by the longest line *visible now*,
/// plus [`HSCROLL_SLACK`], not by the longest line there is; scrolling
/// right stops at the limit, but a position already past it (left
/// there by a vertical scroll) stays put until scrolled back; and a
/// scrollbar shows while anything is out of view. The caller measures
/// the visible lines, since it knows what they are; the pane's
/// `extent` is the columns from the left edge of what scrolls to the
/// end of the longest visible line, and its `capacity` the columns it
/// has to show them in.
#[derive(Default)]
pub(crate) struct HScroll {
    pub(crate) col: usize,
    /// The capacity as of the last render.
    pub(crate) capacity: usize,
    /// The reachable range as of the last render: the visible extent
    /// with its slack, or what is scrolled to, whichever is more.
    pub(crate) total: usize,
    /// Where the scrollbar was drawn, empty while hidden.
    pub(crate) bar: Rect,
    pub(crate) dragging: bool,
}

impl HScroll {
    /// The furthest right the pane can scroll for the lines visible now.
    pub(crate) fn max_col(&self, extent: usize) -> usize {
        (extent + HSCROLL_SLACK).saturating_sub(self.capacity)
    }

    /// Scroll sideways. Scrolling right stops at the limit, but a
    /// position already past it stays where it is.
    pub(crate) fn scroll_by(&mut self, columns: isize, extent: usize) {
        let target = self.col.saturating_add_signed(columns);
        self.col = if columns > 0 {
            target.min(self.max_col(extent).max(self.col))
        } else {
            target
        };
    }

    /// Scroll to where a click or drag at screen column `x` puts the
    /// scrollbar's thumb, with the same limit as [`scroll_by`].
    ///
    /// [`scroll_by`]: Self::scroll_by
    pub(crate) fn scroll_to(&mut self, x: u16, extent: usize) {
        let track = self.bar.width.max(1) as usize;
        let column = x.saturating_sub(self.bar.x) as usize;
        let max = self.total.saturating_sub(self.capacity);
        let target = (column * (max + 1) / track).min(max);
        if target > self.col {
            self.col = target.min(self.max_col(extent).max(self.col));
        } else {
            self.col = target;
        }
    }

    /// Whether the scrollbar is needed for this render: something is
    /// out of view to the right, or the pane is scrolled.
    pub(crate) fn needs_bar(&self, extent: usize, capacity: usize) -> bool {
        self.col > 0 || extent > capacity
    }

    /// Note the render's measurements and draw the scrollbar in `bar`,
    /// or none (an empty `bar`) when it isn't needed.
    pub(crate) fn render(
        &mut self,
        extent: usize,
        capacity: usize,
        bar: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        self.capacity = capacity;
        self.total = (extent + HSCROLL_SLACK).max(self.col + capacity);
        self.bar = bar;
        if bar.width == 0 || bar.height == 0 || capacity == 0 {
            self.bar = Rect::default();
            return;
        }
        let base = palette_background(theme);
        let mut state = ScrollbarState::new(self.total - capacity + 1)
            .position(self.col)
            .viewport_content_length(capacity);
        Scrollbar::new(ScrollbarOrientation::HorizontalBottom)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("─"))
            .thumb_symbol("█")
            .track_style(base.fg(theme.scroll_bar_track))
            .thumb_style(base.fg(theme.scroll_bar_color))
            .render(bar, buf, &mut state);
    }
}

/// The vertical scrollbar of a pane, which is always shown, as the
/// editor's is: the thumb's place says where in the content the pane
/// is, and its length how much of the content fits.
#[derive(Default)]
pub(crate) struct VScroll {
    /// Where the scrollbar was drawn, empty while there is none.
    pub(crate) bar: Rect,
    pub(crate) dragging: bool,
    /// The furthest down the content could scroll at the last render.
    max: usize,
}

impl VScroll {
    /// The scroll position that a click or drag at screen row `y` puts
    /// the thumb at.
    pub(crate) fn scroll_at(&self, y: u16) -> usize {
        let track = self.bar.height.max(1) as usize;
        let row = y.saturating_sub(self.bar.y) as usize;
        (row * (self.max + 1) / track).min(self.max)
    }

    /// Draw the scrollbar in `bar` for content of `rows` rows, `shown`
    /// of them from `scroll`; an empty `bar` draws none.
    pub(crate) fn render(
        &mut self,
        rows: usize,
        shown: usize,
        scroll: usize,
        bar: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        self.bar = bar;
        self.max = rows.saturating_sub(shown);
        if bar.width == 0 || bar.height == 0 {
            self.bar = Rect::default();
            return;
        }
        let base = content_background(theme);
        let mut state = ScrollbarState::new(self.max + 1)
            .position(scroll)
            .viewport_content_length(shown);
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .track_style(base.fg(theme.scroll_bar_track))
            .thumb_style(base.fg(theme.scroll_bar_color))
            .render(bar, buf, &mut state);
    }
}

/// What a page's content pane shows, for drawing and measuring it.
#[derive(Clone, Copy)]
pub(crate) enum Shown<'a> {
    Nothing,
    /// Lines of text, each in a style of its own.
    Lines(&'a [Piece]),
    Diff(&'a DiffModel),
    /// A note in place of content, such as why there is none.
    Note(&'a str),
}

/// What a page's content pane shows, for input that may move the
/// cursor in a diff or expand it.
pub(crate) enum ShownMut<'a> {
    Nothing,
    Lines(&'a [Piece]),
    Diff(&'a mut DiffModel),
    Note(&'a str),
}

impl ShownMut<'_> {
    fn shown(&self) -> Shown<'_> {
        match self {
            ShownMut::Nothing => Shown::Nothing,
            ShownMut::Lines(lines) => Shown::Lines(lines),
            ShownMut::Diff(model) => Shown::Diff(model),
            ShownMut::Note(note) => Shown::Note(note),
        }
    }
}

/// A drag in a diff in progress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Drag {
    #[default]
    None,
    /// Selecting text from where the button went down.
    Text,
    /// Selecting whole rows, in the line numbers, from this row.
    Rows(usize),
}

/// The pane the git pages show the selection's content in, a diff or
/// lines of text: how it is scrolled, both ways, with a scrollbar for
/// each, the expand buttons a diff's gaps drew, and a drag in progress.
/// The page keeps what is shown and hands it over for each render and
/// input, as a [`Shown`] (or a [`ShownMut`], for input that may change a
/// diff); the pane keeps the rest.
///
/// A diff has a cursor, kept by its [`DiffModel`], which the keys move
/// as the editor's do and the view follows; the wheel and the
/// scrollbars scroll without moving it. Lines of text only scroll.
#[derive(Default)]
pub(crate) struct ContentPane {
    /// The rows scrolled off the top.
    scroll: usize,
    h: HScroll,
    v: VScroll,
    /// How many rows the content had at the last render, and how many
    /// of them were shown.
    rows: usize,
    shown: usize,
    /// Where the pane was drawn.
    area: Rect,
    /// Where a diff's text was drawn, right of its line numbers: empty
    /// while a diff has none.
    text: Rect,
    /// The expand buttons drawn in a diff, to hit-test clicks.
    buttons: Vec<(Rect, Button)>,
    /// Whether the next render is to scroll the diff's cursor into
    /// view: it has moved since.
    follow: bool,
    drag: Drag,
    clicks: ClickTracker,
}

impl ContentPane {
    /// Back to the top and the left edge, for new content.
    pub(crate) fn reset(&mut self) {
        self.scroll = 0;
        self.h.col = 0;
        self.follow = false;
    }

    /// Whether a scrollbar is being dragged, or a selection, in which
    /// case the pane wants drag and release events wherever they happen.
    pub(crate) fn is_dragging(&self) -> bool {
        self.h.dragging || self.v.dragging || self.drag != Drag::None
    }

    /// The columns the visible rows of the content reach.
    pub(crate) fn extent(&self, content: Shown<'_>) -> usize {
        match content {
            Shown::Lines(lines) => text_extent(lines, self.scroll, self.shown),
            Shown::Diff(model) => diff_extent(model.diff(), model.rows(), self.scroll, self.shown),
            Shown::Nothing | Shown::Note(_) => 0,
        }
    }

    /// The content's row `from` is now row `to`, the rows above it having
    /// changed: scroll as far, so it stays where it was on the screen.
    pub(crate) fn keep_on_screen(&mut self, from: usize, to: usize) {
        self.scroll = (self.scroll + to).saturating_sub(from);
    }

    /// Scroll by some rows, down for positive. The render clamps the
    /// position to the content.
    pub(crate) fn scroll_by(&mut self, rows: isize) {
        self.scroll = self.scroll.saturating_add_signed(rows);
    }

    /// Scroll sideways by some columns, right for positive, with the
    /// editor's limits (see [`HScroll`]).
    pub(crate) fn scroll_sideways(&mut self, columns: isize, content: Shown<'_>) {
        let extent = self.extent(content);
        self.h.scroll_by(columns, extent);
    }

    /// Handle a key while the pane has the keyboard. In a diff with
    /// lines the keys work as in the editor, without the editing: the
    /// arrows, Home and End, and Page Up and Page Down move the cursor
    /// (by word and to either end of the diff with Ctrl), Shift with any
    /// of them selects, Ctrl+↑ and Ctrl+↓ scroll without moving it,
    /// Ctrl+A selects everything, Ctrl+C copies the selection, Escape
    /// clears it, and Enter on a gap presses the button the cursor is
    /// on, the cursor staying on it while the gap is left. Anything
    /// else only scrolls: the arrows (sideways too), Page Up and Page
    /// Down by a page, Home and End to either end. Returns what to say
    /// in the status bar, if anything: what was copied.
    pub(crate) fn handle_key(
        &mut self,
        key: KeyEvent,
        content: ShownMut<'_>,
        clipboard: &mut Clipboard,
    ) -> Option<StatusLine> {
        match content {
            ShownMut::Diff(model) if !model.rows().is_empty() => {
                self.handle_diff_key(key, model, clipboard)
            }
            content => {
                self.scroll_key(key, content.shown());
                None
            }
        }
    }

    fn scroll_key(&mut self, key: KeyEvent, content: Shown<'_>) {
        let page = (self.area.height as usize).saturating_sub(1).max(1);
        match key.code {
            KeyCode::Up => self.scroll_by(-1),
            KeyCode::Down => self.scroll_by(1),
            KeyCode::PageUp => self.scroll_by(-(page as isize)),
            KeyCode::PageDown => self.scroll_by(page as isize),
            KeyCode::Home => self.scroll = 0,
            KeyCode::End => self.scroll = usize::MAX,
            KeyCode::Right => self.scroll_sideways(ARROW_COLUMNS as isize, content),
            KeyCode::Left => self.scroll_sideways(-(ARROW_COLUMNS as isize), content),
            _ => {}
        }
    }

    fn handle_diff_key(
        &mut self,
        key: KeyEvent,
        model: &mut DiffModel,
        clipboard: &mut Clipboard,
    ) -> Option<StatusLine> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        // Page Up and Down scroll by a page and move the cursor as far,
        // so it stays on its screen row; the page is a row short of the
        // pane's, as in the editor, to keep some context across it.
        let page = self.shown.saturating_sub(1).max(1);
        let movement = match key.code {
            KeyCode::Left if ctrl => Movement::WordLeft,
            KeyCode::Right if ctrl => Movement::WordRight,
            KeyCode::Left => Movement::Left,
            KeyCode::Right => Movement::Right,
            KeyCode::Up if ctrl => {
                self.scroll_by(-1);
                return None;
            }
            KeyCode::Down if ctrl => {
                self.scroll_by(1);
                return None;
            }
            KeyCode::Up => Movement::Up,
            KeyCode::Down => Movement::Down,
            KeyCode::Home if ctrl => Movement::DocumentStart,
            KeyCode::End if ctrl => Movement::DocumentEnd,
            KeyCode::Home => Movement::LineStart,
            KeyCode::End => Movement::LineEnd,
            KeyCode::PageUp => {
                self.scroll_by(-(page as isize));
                Movement::PageUp(page)
            }
            KeyCode::PageDown => {
                self.scroll_by(page as isize);
                Movement::PageDown(page)
            }
            KeyCode::Char(c) if ctrl => {
                match c.to_ascii_lowercase() {
                    'a' => model.select_all(),
                    'c' => return copy(Shown::Diff(model), clipboard),
                    _ => {}
                }
                return None;
            }
            KeyCode::Esc => {
                model.clear_selection();
                return None;
            }
            KeyCode::Enter => {
                if let Some((gap, action)) = model.action_at_cursor() {
                    model.take_action(gap, action);
                    self.follow = true;
                }
                return None;
            }
            _ => return None,
        };
        if shift {
            model.extend_selection(movement);
        } else {
            model.move_cursor(movement);
        }
        self.follow = true;
        None
    }

    /// Handle a mouse event over the pane, or any drag or release while
    /// [dragging](Self::is_dragging). `wheel` is how many rows (or
    /// columns, sideways) the wheel scrolls. In a diff a click puts the
    /// cursor under the pointer and a drag selects, a double-click
    /// selects a word and a Shift+click selects to the pointer; in the
    /// line numbers, they select whole rows. A right press puts the
    /// cursor under the pointer too, unless it is in the selection, which
    /// is what a menu there is for. A click on an expand button reveals
    /// what it says.
    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent, wheel: usize, content: ShownMut<'_>) {
        let at = ScreenPosition::new(mouse.column, mouse.row);
        let wheel = wheel as isize;
        match mouse.kind {
            MouseEventKind::Drag(_) if self.h.dragging => {
                let extent = self.extent(content.shown());
                self.h.scroll_to(mouse.column, extent);
            }
            MouseEventKind::Drag(_) if self.v.dragging => self.scroll = self.v.scroll_at(mouse.row),
            MouseEventKind::Drag(_) => {
                if let ShownMut::Diff(model) = content {
                    match self.drag {
                        Drag::Text => {
                            let (row, column) = self.position_at(mouse.column, mouse.row);
                            model.extend_to(row, column);
                            self.follow = true;
                        }
                        Drag::Rows(anchor) => {
                            model.select_rows(anchor, self.row_at(mouse.row));
                            self.follow = true;
                        }
                        Drag::None => {}
                    }
                }
            }
            MouseEventKind::Up(_) => {
                self.h.dragging = false;
                self.v.dragging = false;
                self.drag = Drag::None;
            }
            MouseEventKind::ScrollUp => self.scroll_by(-wheel),
            MouseEventKind::ScrollDown => self.scroll_by(wheel),
            MouseEventKind::ScrollRight => self.scroll_sideways(wheel, content.shown()),
            MouseEventKind::ScrollLeft => self.scroll_sideways(-wheel, content.shown()),
            MouseEventKind::Down(MouseButton::Left) if self.h.bar.contains(at) => {
                self.h.dragging = true;
                let extent = self.extent(content.shown());
                self.h.scroll_to(mouse.column, extent);
            }
            MouseEventKind::Down(MouseButton::Left) if self.v.bar.contains(at) => {
                self.v.dragging = true;
                self.scroll = self.v.scroll_at(mouse.row);
            }
            MouseEventKind::Down(button @ (MouseButton::Left | MouseButton::Right)) => {
                let ShownMut::Diff(model) = content else {
                    return;
                };
                if button == MouseButton::Left
                    && let Some((_, pressed)) =
                        self.buttons.iter().find(|(area, _)| area.contains(at))
                {
                    pressed.press(model);
                    return;
                }
                let rows = Rect::new(self.area.x, self.text.y, self.area.width, self.text.height);
                if !rows.contains(at) || self.text.width == 0 {
                    return;
                }
                let (row, mut column) = self.position_at(mouse.column, mouse.row);
                // A gap's row, off its buttons, is its first button.
                if !model.gap_actions(row).is_empty() {
                    column = 0;
                }
                if button == MouseButton::Right {
                    let pos = model.position_at(row, column);
                    if !model.selection().is_some_and(|range| range.contains(&pos)) {
                        model.set_cursor_at(row, column);
                    }
                } else if mouse.column < self.text.x {
                    model.select_rows(row, row);
                    self.drag = Drag::Rows(row);
                } else {
                    if self.clicks.press(mouse.column, mouse.row) == 2 {
                        model.select_word_at(row, column);
                    } else if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                        model.extend_to(row, column);
                    } else {
                        model.set_cursor_at(row, column);
                    }
                    self.drag = Drag::Text;
                }
                self.follow = true;
            }
            _ => {}
        }
    }

    /// The row of the diff at screen row `y`. Above the rows shown it is
    /// the one before them, and below them the one after, so a drag past
    /// an edge scrolls that way.
    fn row_at(&self, y: u16) -> usize {
        if y < self.text.y {
            self.scroll.saturating_sub(1)
        } else if y >= self.text.bottom() {
            self.scroll + self.text.height as usize
        } else {
            self.scroll + (y - self.text.y) as usize
        }
    }

    /// The row and display column of the diff at a screen position; past
    /// the text's edges, one beyond what shows, as for [`row_at`].
    ///
    /// [`row_at`]: Self::row_at
    fn position_at(&self, x: u16, y: u16) -> (usize, usize) {
        let column = if x < self.text.x {
            self.h.col.saturating_sub(1)
        } else if x >= self.text.right() {
            self.h.col + self.text.width as usize
        } else {
            self.h.col + (x - self.text.x) as usize
        };
        (self.row_at(y), column)
    }

    /// Draw the content into `area`, with the vertical scrollbar down
    /// its right edge, and the horizontal one along the bottom of the
    /// rest when anything is out of view sideways. A diff's cursor is
    /// scrolled into view if it has moved since the last render. Returns
    /// where the terminal's cursor goes, while the pane is `focused`: on
    /// a diff's cursor when it is in view and nothing is selected.
    pub(crate) fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        content: Shown<'_>,
        focused: bool,
    ) -> Option<ScreenPosition> {
        self.area = area;
        self.text = Rect::default();
        self.buttons.clear();
        let follow = std::mem::take(&mut self.follow);
        let background = content_background(theme);
        buf.set_style(area, background);
        if area.width < 4 || area.height == 0 {
            self.h.bar = Rect::default();
            self.v.bar = Rect::default();
            return None;
        }
        let dim = background.fg(theme.command_palette_result_context_text);
        let inner = Rect::new(area.x, area.y, area.width - 1, area.height);
        let mut cursor = None;
        match content {
            Shown::Nothing => {
                self.h.render(0, 0, Rect::default(), buf, theme);
                self.v.bar = Rect::default();
                return None;
            }
            Shown::Note(note) => {
                buf.set_stringn(inner.x + 1, inner.y, note, inner.width as usize - 1, dim);
                self.h
                    .render(0, inner.width as usize, Rect::default(), buf, theme);
                self.rows = 1;
                self.shown = 1;
                self.scroll = 0;
            }
            Shown::Lines(lines) => self.render_lines(inner, buf, theme, lines),
            Shown::Diff(model) => {
                let drawn = render_diff(
                    model,
                    inner,
                    buf,
                    theme,
                    self.scroll,
                    &mut self.h,
                    &mut self.buttons,
                    follow,
                    focused,
                );
                self.rows = drawn.rows;
                self.shown = drawn.shown;
                self.scroll = drawn.scroll;
                self.text = drawn.text;
                if focused && model.selection().is_none() {
                    cursor = self.cursor_position(model);
                }
            }
        }
        // The bar runs down beside the rows shown, leaving the corner
        // by the horizontal scrollbar empty, as in the editor.
        let bar = Rect::new(
            area.right() - 1,
            area.y,
            1,
            self.shown.min(area.height as usize) as u16,
        );
        self.v
            .render(self.rows, self.shown, self.scroll, bar, buf, theme);
        cursor
    }

    /// Where a diff's cursor is on the screen, if it is in view: in the
    /// text, or at the start of a gap's row.
    fn cursor_position(&self, model: &DiffModel) -> Option<ScreenPosition> {
        let row = model.cursor().line;
        if self.text.width == 0
            || row < self.scroll
            || row >= self.scroll + self.text.height as usize
        {
            return None;
        }
        let y = self.text.y + (row - self.scroll) as u16;
        // On a gap's row, on the arrow of the button it is on (or at the
        // row's start, if that didn't fit).
        if let Some((_, action)) = model.action_at_cursor() {
            let button = self
                .buttons
                .iter()
                .find(|(_, button)| button.row == row && button.action == action);
            let x = button.map_or(self.area.x + 1, |(area, _)| area.x + 1);
            return Some(ScreenPosition::new(x, y));
        }
        let column = model.cursor_column();
        let visible = self.h.col..self.h.col + self.text.width as usize;
        visible
            .contains(&column)
            .then(|| ScreenPosition::new(self.text.x + (column - self.h.col) as u16, y))
    }

    /// Draw lines of text, each in its own style over the pane's
    /// background, scrolled both ways.
    fn render_lines(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme, lines: &[Piece]) {
        let background = content_background(theme);
        let height = area.height as usize;
        self.rows = lines.len();
        let capacity = area.width as usize - 1;
        // The scrollbar takes the last row; see `render_diff`. Clamp the
        // scroll asked for afresh on each pass, as `render_diff` does,
        // so the last row is reachable once the bar takes a row.
        let wanted = self.scroll;
        let mut show_bar = false;
        let mut shown;
        let mut extent;
        loop {
            shown = height - usize::from(show_bar);
            self.scroll = wanted.min(lines.len().saturating_sub(shown));
            extent = text_extent(lines, self.scroll, shown);
            let needed = self.h.needs_bar(extent, capacity) && height > 1;
            if needed && !show_bar {
                show_bar = true;
                continue;
            }
            break;
        }
        self.shown = shown;
        for (row, (line, style)) in lines.iter().skip(self.scroll).take(shown).enumerate() {
            let y = area.y + row as u16;
            let cells = layout_cells(line, &[]);
            let row_area = Rect::new(area.x + 1, y, area.width - 1, 1);
            draw_cells(buf, row_area, &cells, self.h.col, |_| {
                background.patch(*style)
            });
        }
        let bar = if show_bar {
            Rect::new(area.x + 1, area.bottom() - 1, area.width - 1, 1)
        } else {
            Rect::default()
        };
        self.h.render(extent, capacity, bar, buf, theme);
    }
}

/// What tests look at.
#[cfg(test)]
impl ContentPane {
    pub(crate) fn h(&self) -> &HScroll {
        &self.h
    }

    pub(crate) fn v(&self) -> &VScroll {
        &self.v
    }

    pub(crate) fn scroll(&self) -> usize {
        self.scroll
    }

    /// How many rows the content had at the last render, and how many
    /// of them were shown.
    pub(crate) fn rows(&self) -> (usize, usize) {
        (self.rows, self.shown)
    }
}

/// The display width of a line of pieces.
pub(crate) fn pieces_width(pieces: &[Piece]) -> usize {
    pieces.iter().map(|(text, _)| display_width(text)).sum()
}

/// The widest of `rows` lines of text from `scroll`.
pub(crate) fn text_extent(lines: &[Piece], scroll: usize, rows: usize) -> usize {
    lines
        .iter()
        .skip(scroll)
        .take(rows)
        .map(|(text, _)| display_width(text))
        .max()
        .unwrap_or(0)
}

/// The widest of `rows` rows of a diff from `scroll`; the gap rows
/// don't scroll and don't count. A submodule's rows are its commits'
/// (see [`render_submodule`]), measured with the graph at full width.
pub(crate) fn diff_extent(diff: &FileDiff, rows: &[DiffRow], scroll: usize, count: usize) -> usize {
    if let Some(range) = &diff.submodule {
        return submodule_rows(range)
            .iter()
            .skip(scroll)
            .take(count)
            .map(|row| submodule_row_extent(diff, range, row, usize::MAX))
            .max()
            .unwrap_or(0);
    }
    rows.iter()
        .skip(scroll)
        .take(count)
        .filter_map(|row| match row {
            DiffRow::Line(line) => Some(display_width(diff.text(line))),
            DiffRow::Gap { .. } => None,
        })
        .max()
        .unwrap_or(0)
}

/// The background of a page's content pane: the theme's preview
/// background, so that code can be shown over the editor's colors.
pub(crate) fn content_background(theme: &Theme) -> Style {
    palette_background(theme).bg(theme
        .search_preview_background
        .unwrap_or(theme.command_palette_background))
}

/// What a render of a diff settled on.
pub(crate) struct DiffDrawn {
    /// How many rows the diff has.
    pub(crate) rows: usize,
    /// How many of them were shown.
    pub(crate) shown: usize,
    /// The vertical scroll position.
    pub(crate) scroll: usize,
    /// Where the rows' text was drawn, right of the line numbers, one
    /// screen row a row shown; empty for a diff without lines.
    pub(crate) text: Rect,
}

/// Draw a diff into `area`, scrolled by `scroll` rows and sideways by
/// `h`, with its selection, recording the expand buttons drawn. With
/// `follow`, the scroll is first changed as little as brings the
/// cursor into view. The button the cursor is on is drawn as focused
/// while the pane is.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_diff(
    model: &DiffModel,
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    scroll: usize,
    h: &mut HScroll,
    buttons: &mut Vec<(Rect, Button)>,
    follow: bool,
    focused: bool,
) -> DiffDrawn {
    let diff = model.diff();
    let background = content_background(theme);
    let dim = background.fg(theme.command_palette_result_context_text);
    let height = area.height as usize;
    if let Some(range) = &diff.submodule {
        return render_submodule(diff, range, area, buf, theme, scroll, h);
    }
    if let Some(unshown) = diff.unshown {
        let message = match unshown {
            Unshown::Binary => "Binary file",
            Unshown::TooLarge => "File too large to show",
            Unshown::Submodule => "Submodule: the commit it points at changed",
        };
        buf.set_stringn(area.x + 1, area.y, message, area.width as usize - 1, dim);
        h.render(0, area.width as usize, Rect::default(), buf, theme);
        return DiffDrawn {
            rows: 1,
            shown: 1,
            scroll: 0,
            text: Rect::default(),
        };
    }
    let rows = model.rows();
    if rows.is_empty() {
        let message = match diff.kind {
            ChangeKind::Renamed => "Renamed without changes",
            ChangeKind::Copied => "Copied without changes",
            _ => "No changes to the contents",
        };
        buf.set_stringn(area.x + 1, area.y, message, area.width as usize - 1, dim);
        h.render(0, area.width as usize, Rect::default(), buf, theme);
        return DiffDrawn {
            rows: 1,
            shown: 1,
            scroll: 0,
            text: Rect::default(),
        };
    }
    // The gutter: the old and new line numbers, then the marker.
    let old_digits = digits(diff.old_line_count());
    let new_digits = digits(diff.new_line_count());
    let gutter = old_digits + 1 + new_digits + 1;
    let marker_x = area.x + gutter as u16;
    let text_x = marker_x + 2;
    if text_x >= area.right() {
        h.render(0, 0, Rect::default(), buf, theme);
        return DiffDrawn {
            rows: rows.len(),
            shown: height,
            scroll: scroll.min(rows.len().saturating_sub(height)),
            text: Rect::default(),
        };
    }
    let text_width = area.right() - text_x;
    // The sideways scrollbar takes the last row; see `render_log`.
    let capacity = text_width as usize;
    let cursor = model.cursor().line;
    if follow {
        let column = model.cursor_column();
        if column < h.col {
            h.col = column;
        } else if column >= h.col + capacity {
            h.col = column + 1 - capacity;
        }
    }
    // Clamp the scroll asked for afresh on each pass: the first pass
    // clamps it for the full height, and once the bar takes a row the
    // last row of the diff has to be reachable again (and the cursor
    // may be on it).
    let wanted = scroll;
    let mut show_bar = false;
    let mut shown;
    let mut scroll;
    let mut extent;
    loop {
        shown = height - usize::from(show_bar);
        scroll = wanted.min(rows.len().saturating_sub(shown));
        if follow {
            if cursor < scroll {
                scroll = cursor;
            } else if cursor >= scroll + shown {
                scroll = cursor + 1 - shown;
            }
        }
        extent = diff_extent(diff, rows, scroll, shown);
        let needed = h.needs_bar(extent, capacity) && height > 1;
        if needed && !show_bar {
            show_bar = true;
            continue;
        }
        break;
    }
    let col = h.col;
    let added_bg = background.bg(theme.diff_added_background);
    let removed_bg = background.bg(theme.diff_removed_background);
    let number = |base: Style| base.fg(theme.inactive_line_number);
    let button_style = background
        .fg(theme.active_tab_text)
        .bg(theme.command_palette_background);
    // As a dialog draws its focused button.
    let focused_button_style = background
        .fg(theme.command_palette_selection_text)
        .bg(theme.command_palette_selection_background);

    for (row, entry) in rows.iter().enumerate().skip(scroll).take(shown) {
        let y = area.y + (row - scroll) as u16;
        match entry {
            DiffRow::Line(line) => {
                let (row_style, marker, marker_style) = match line.kind {
                    LineKind::Context => (background, " ", background),
                    LineKind::Added => (added_bg, "+", added_bg.fg(theme.diff_added_text)),
                    LineKind::Removed => (removed_bg, "-", removed_bg.fg(theme.diff_removed_text)),
                };
                buf.set_style(Rect::new(area.x, y, area.width, 1), row_style);
                let old = line.old.map_or(String::new(), |n| (n + 1).to_string());
                let new = line.new.map_or(String::new(), |n| (n + 1).to_string());
                buf.set_string(
                    area.x,
                    y,
                    format!("{old:>old_digits$} {new:>new_digits$}"),
                    number(row_style),
                );
                buf.set_string(
                    marker_x,
                    y,
                    marker,
                    marker_style.add_modifier(Modifier::BOLD),
                );
                let text = diff.text(line);
                let tokens = diff.tokens(line);
                let cells = layout_cells(text, tokens);
                let text_area = Rect::new(text_x, y, text_width, 1);
                draw_cells(buf, text_area, &cells, col, |kind| {
                    if kind == TokenKind::Text {
                        row_style
                    } else {
                        theme.syntax(kind).apply(row_style)
                    }
                });
                // The selection over the text drawn: its background, and
                // the theme's text color for it if it has one, else the
                // syntax colors. The gaps' rows are left out of it, as
                // they are out of what it copies.
                if let Some(selected) = model.selected_columns(row) {
                    let from = selected.start.max(col);
                    let to = selected.end.min(col + capacity);
                    for column in from..to {
                        let cell = &mut buf[(text_x + (column - col) as u16, y)];
                        cell.set_bg(theme.selection_background);
                        if let Some(color) = theme.selection_text {
                            cell.set_fg(color);
                        }
                    }
                }
            }
            DiffRow::Gap { gap, hidden, .. } => {
                let mut x = area.x + 1;
                let mut place = |text: String, style: Style, button: Option<Button>| {
                    let width = display_width(&text) as u16;
                    if x + width > area.right() {
                        return;
                    }
                    buf.set_string(x, y, &text, style);
                    if let Some(button) = button {
                        buttons.push((Rect::new(x, y, width, 1), button));
                    }
                    x += width + 1;
                };
                let step = (*hidden).min(EXPAND_LINES);
                let chosen = match model.action_at_cursor() {
                    Some((_, action)) if focused && cursor == row => Some(action),
                    _ => None,
                };
                for action in model.gap_actions(row) {
                    let label = match action {
                        GapAction::Up => format!(" ▲ {step} "),
                        GapAction::Down => format!(" ▼ {step} "),
                        GapAction::All => " all ".to_owned(),
                    };
                    let style = if chosen == Some(action) {
                        focused_button_style
                    } else {
                        button_style
                    };
                    let button = Button {
                        row,
                        gap: *gap,
                        action,
                    };
                    place(label, style, Some(button));
                }
                let noun = if *hidden == 1 { "line" } else { "lines" };
                place(format!("⋯ {hidden} {noun} hidden"), dim, None);
            }
        }
    }
    let bar = if show_bar {
        Rect::new(text_x, area.bottom() - 1, text_width, 1)
    } else {
        Rect::default()
    };
    h.render(extent, capacity, bar, buf, theme);
    DiffDrawn {
        rows: rows.len(),
        shown,
        scroll,
        text: Rect::new(text_x, area.y, text_width, shown as u16),
    }
}

/// How many of a submodule's uncommitted changes are listed, at most.
pub(crate) const UNCOMMITTED_LIMIT: usize = 50;
/// The heading over a submodule's uncommitted changes.
pub(crate) const UNCOMMITTED_HEADING: &str =
    "Uncommitted changes inside the submodule, on its tab:";

/// One row of a submodule's diff.
enum SubmoduleRow<'a> {
    /// The line saying from which commit to which.
    Heading,
    Note(String),
    Commit(&'a Commit, CommitLine),
    /// An uncommitted change inside the submodule.
    Change(&'a UncommittedChange),
}

/// The rows of a submodule's diff: the heading, a note when there are
/// no commits to show, two rows a commit, and then, after a blank
/// row, the uncommitted changes inside the submodule under a heading
/// of their own, when there are any.
fn submodule_rows(range: &SubmoduleRange) -> Vec<SubmoduleRow<'_>> {
    let mut rows = vec![SubmoduleRow::Heading];
    if let Some(error) = &range.error {
        rows.push(SubmoduleRow::Note(error.clone()));
    } else if range.commits.is_empty() && range.old.is_some() && range.old == range.new {
        let note = if range.uncommitted.is_empty() {
            "Still at this commit: the changes are inside the submodule, on its tab"
        } else {
            "Still at this commit"
        };
        rows.push(SubmoduleRow::Note(note.to_owned()));
    }
    for commit in &range.commits {
        rows.push(SubmoduleRow::Commit(commit, CommitLine::Node));
        rows.push(SubmoduleRow::Commit(commit, CommitLine::Transition));
    }
    if range.truncated {
        rows.push(SubmoduleRow::Note(format!(
            "⋯ only the newest {RANGE_LIMIT} commits are shown"
        )));
    }
    if !range.uncommitted.is_empty() {
        rows.push(SubmoduleRow::Note(String::new()));
        rows.push(SubmoduleRow::Note(UNCOMMITTED_HEADING.to_owned()));
        rows.extend(
            range
                .uncommitted
                .iter()
                .take(UNCOMMITTED_LIMIT)
                .map(SubmoduleRow::Change),
        );
        let more = range.uncommitted.len().saturating_sub(UNCOMMITTED_LIMIT);
        if more > 0 {
            rows.push(SubmoduleRow::Note(format!("⋯ and {more} more")));
        }
    }
    rows
}

/// The text of an uncommitted change's row: its letter, its path, and
/// whether it is staged. Styled with the theme's colors over `(plain,
/// dim)` when given, its letter colored as the file lists color
/// theirs; plain for measuring.
fn uncommitted_pieces(
    change: &UncommittedChange,
    styles: Option<(&Theme, Style, Style)>,
) -> Vec<Piece> {
    let (plain, dim) = styles
        .map(|(_, plain, dim)| (plain, dim))
        .unwrap_or_default();
    let letter = match styles {
        None => plain,
        Some((theme, _, _)) => match change.kind {
            ChangeKind::Added | ChangeKind::Copied | ChangeKind::Untracked => {
                plain.fg(theme.diff_added_text)
            }
            ChangeKind::Deleted => plain.fg(theme.diff_removed_text),
            ChangeKind::Conflicted => plain
                .fg(theme.diff_removed_text)
                .add_modifier(Modifier::BOLD),
            _ => plain.fg(theme.git_tag_text),
        },
    };
    let mut pieces = vec![
        ("  ".to_owned(), plain),
        (change.kind.letter().to_string(), letter),
        (format!(" {}", change.path), plain),
    ];
    if change.staged {
        pieces.push(("  staged".to_owned(), dim));
    }
    pieces
}

/// The heading of a submodule's diff: which commit it moved from and
/// to. Styled as `(dim, hash)` when given, plain for measuring.
fn submodule_heading(
    diff: &FileDiff,
    range: &SubmoduleRange,
    styles: Option<(Style, Style)>,
) -> Vec<Piece> {
    let (dim, hash) = styles.unwrap_or_default();
    let mut pieces = vec![(format!("Submodule {}: ", diff.path), dim)];
    match (range.old, range.new) {
        (Some(old), Some(new)) => {
            pieces.push((short_id(old), hash));
            pieces.push((" → ".to_owned(), dim));
            pieces.push((short_id(new), hash));
        }
        (None, Some(new)) => {
            pieces.push(("added at ".to_owned(), dim));
            pieces.push((short_id(new), hash));
        }
        (Some(old), None) => {
            pieces.push(("removed at ".to_owned(), dim));
            pieces.push((short_id(old), hash));
        }
        (None, None) => pieces.push(("the commit it points at changed".to_owned(), dim)),
    }
    pieces
}

/// The columns a row of a submodule's diff reaches, its graph drawn
/// at most `max_lanes` wide.
fn submodule_row_extent(
    diff: &FileDiff,
    range: &SubmoduleRange,
    row: &SubmoduleRow<'_>,
    max_lanes: usize,
) -> usize {
    match row {
        SubmoduleRow::Heading => pieces_width(&submodule_heading(diff, range, None)),
        SubmoduleRow::Note(note) => display_width(note),
        SubmoduleRow::Change(change) => pieces_width(&uncommitted_pieces(change, None)),
        SubmoduleRow::Commit(commit, _) => commit_extent(
            commit,
            Highlight::target_if(range.new == Some(commit.id)),
            commit.graph.width().clamp(1, max_lanes),
        ),
    }
}

/// Draw a submodule's diff: the commits it moved over, as a graph like
/// the log's with the commit it now points at styled as HEAD, under a
/// line saying from which commit to which. The graph gets at most a
/// share of the width, as in the log, and stays put while the text
/// scrolls sideways.
fn render_submodule(
    diff: &FileDiff,
    range: &SubmoduleRange,
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    scroll: usize,
    h: &mut HScroll,
) -> DiffDrawn {
    let background = content_background(theme);
    let dim = background.fg(theme.command_palette_result_context_text);
    let hash = background.fg(theme.git_hash_text);
    let height = area.height as usize;
    let rows = submodule_rows(range);
    let max_lanes = lane_cap(area.width - 1);
    // The sideways scrollbar takes the last row; see `render_diff`.
    let capacity = area.width as usize - 1;
    // Clamp the scroll asked for afresh on each pass: the first pass
    // clamps it for the full height, and once the bar takes a row the
    // last row of the diff has to be reachable again.
    let wanted = scroll;
    let mut show_bar = false;
    let mut shown;
    let mut scroll;
    let mut extent;
    loop {
        shown = height - usize::from(show_bar);
        scroll = wanted.min(rows.len().saturating_sub(shown));
        extent = rows
            .iter()
            .skip(scroll)
            .take(shown)
            .map(|row| submodule_row_extent(diff, range, row, max_lanes))
            .max()
            .unwrap_or(0);
        let needed = h.needs_bar(extent, capacity) && height > 1;
        if needed && !show_bar {
            show_bar = true;
            continue;
        }
        break;
    }
    for (index, row) in rows.iter().skip(scroll).take(shown).enumerate() {
        let row_area = Rect::new(area.x + 1, area.y + index as u16, area.width - 1, 1);
        match row {
            SubmoduleRow::Heading => {
                let pieces = submodule_heading(diff, range, Some((dim, hash)));
                draw_pieces(buf, row_area.x, row_area.y, capacity, &pieces, h.col);
            }
            SubmoduleRow::Note(note) => {
                let pieces = [(note.clone(), dim)];
                draw_pieces(buf, row_area.x, row_area.y, capacity, &pieces, h.col);
            }
            SubmoduleRow::Change(change) => {
                let pieces = uncommitted_pieces(change, Some((theme, background, dim)));
                draw_pieces(buf, row_area.x, row_area.y, capacity, &pieces, h.col);
            }
            SubmoduleRow::Commit(commit, line) => draw_commit_line(
                buf,
                row_area,
                commit,
                *line,
                Highlight::target_if(range.new == Some(commit.id)),
                commit.graph.width().clamp(1, max_lanes),
                background,
                theme,
                h.col,
            ),
        }
    }
    let bar = if show_bar {
        Rect::new(area.x + 1, area.bottom() - 1, area.width - 1, 1)
    } else {
        Rect::default()
    };
    h.render(extent, capacity, bar, buf, theme);
    DiffDrawn {
        rows: rows.len(),
        shown,
        scroll,
        text: Rect::default(),
    }
}

/// Draw pieces of text one after another on a row, cut at `width`,
/// starting `start` display columns into them.
pub(crate) fn draw_pieces(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: usize,
    pieces: &[Piece],
    start: usize,
) {
    let mut x = x;
    let end = x + width as u16;
    let mut skipped = 0;
    for (text, style) in pieces {
        if x >= end {
            break;
        }
        let piece_width = display_width(text);
        let text = if skipped + piece_width <= start {
            skipped += piece_width;
            continue;
        } else if skipped < start {
            let rest = skip_columns(text, start - skipped);
            skipped = start;
            rest
        } else {
            text.as_str()
        };
        let (next, _) = buf.set_stringn(x, y, text, (end - x) as usize, *style);
        x = next;
    }
}

/// `text` without its first `columns` display columns; a wide
/// character straddling the cut goes with the part cut off.
pub(crate) fn skip_columns(text: &str, columns: usize) -> &str {
    let mut column = 0;
    for g in text::graphemes(text.as_bytes()) {
        if column >= columns {
            return &text[g.range.start..];
        }
        column += text::width(g.text, column, TAB_WIDTH);
    }
    ""
}

/// One character of a line laid out for display.
pub(crate) struct Cell<'a> {
    pub(crate) text: &'a str,
    pub(crate) column: usize,
    pub(crate) width: usize,
    pub(crate) kind: TokenKind,
}

/// Lay out a line's characters, expanding tabs, each classified by the
/// token containing its first byte.
pub(crate) fn layout_cells<'a>(text: &'a str, tokens: &[Token]) -> Vec<Cell<'a>> {
    let mut column = 0;
    let mut next_token = 0;
    text::graphemes(text.as_bytes())
        .map(|g| {
            let width = text::width(g.text, column, TAB_WIDTH);
            let start = g.range.start;
            while next_token < tokens.len() && tokens[next_token].range.end <= start {
                next_token += 1;
            }
            let kind = match tokens.get(next_token) {
                Some(token) if token.range.start <= start => token.kind,
                _ => TokenKind::Text,
            };
            let cell = Cell {
                text: g.text,
                column,
                width,
                kind,
            };
            column += width;
            cell
        })
        .collect()
}

/// Draw a line's cells into a one-row `area` from display column
/// `start`, each in the style `style_of` gives its kind.
pub(crate) fn draw_cells(
    buf: &mut Buffer,
    area: Rect,
    cells: &[Cell<'_>],
    start: usize,
    style_of: impl Fn(TokenKind) -> Style,
) {
    let width = area.width as usize;
    if width == 0 {
        return;
    }
    let visible = start..start + width;
    for cell in cells {
        if cell.width == 0 || cell.column + cell.width <= visible.start {
            continue;
        }
        if cell.column >= visible.end {
            break;
        }
        let style = style_of(cell.kind);
        let fits = cell.column >= visible.start && cell.column + cell.width <= visible.end;
        let from = cell.column.max(visible.start);
        let to = (cell.column + cell.width).min(visible.end);
        let x = area.x + (from - start) as u16;
        if cell.text == "\t" || !fits {
            for x in x..x + (to - from) as u16 {
                buf[(x, area.y)].set_symbol(" ").set_style(style);
            }
        } else {
            buf.set_string(x, area.y, cell.text, style);
        }
    }
}

/// The display width of a piece of text.
pub(crate) fn display_width(text: &str) -> usize {
    text::graphemes(text.as_bytes())
        .map(|g| text::width(g.text, 0, TAB_WIDTH))
        .sum()
}

/// `text` if it fits in `width` columns, else its end with an ellipsis
/// before it: for a path, the file name is the part that matters.
pub(crate) fn fit_end(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_owned();
    }
    let room = width.saturating_sub(1);
    let graphemes: Vec<_> = text::graphemes(text.as_bytes()).collect();
    let mut kept = 0;
    let mut cut = text.len();
    for g in graphemes.iter().rev() {
        let w = text::width(g.text, 0, TAB_WIDTH);
        if kept + w > room {
            break;
        }
        kept += w;
        cut = g.range.start;
    }
    format!("…{}", &text[cut..])
}

/// `value` within `low..=high`, or `high` when the bounds cross (a pane
/// too small for both minimums gets what there is).
pub(crate) fn clamp_between(value: u16, low: u16, high: u16) -> u16 {
    value.min(high).max(low.min(high))
}

/// The cells a share of `total` comes to.
pub(crate) fn share_of(total: u16, share: f32) -> u16 {
    (f32::from(total) * share).round() as u16
}

/// The share of `total` that `cells` are.
pub(crate) fn share_for(cells: u16, total: u16) -> f32 {
    f32::from(cells) / f32::from(total.max(1))
}

/// The number of decimal digits needed to show `n`.
pub(crate) fn digits(n: usize) -> usize {
    n.max(1).ilog10() as usize + 1
}
