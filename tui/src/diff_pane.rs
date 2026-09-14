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
//! upward, ▼ from the change above it downward, or all of them; the
//! page hit-tests clicks against the [`Button`]s the render records.
//!
//! [`HScroll`] is the sideways scrolling of a pane, with the editor's
//! rules (see `EditorView`): the limit is set by the longest line
//! visible now plus a little slack, not by the longest line there is,
//! and a scrollbar shows along the bottom while anything is out of
//! view. The rest is the layout of a line's characters with tabs
//! expanded ([`layout_cells`] and [`draw_cells`]), pieces of styled
//! text drawn end to end ([`draw_pieces`]), and the small sums the
//! pages' layouts are made of.

use crate::palette::palette_background;
use crate::theme::Theme;
use ninjaedit_core::git::{ChangeKind, DiffRow, FileDiff, LineKind, Unshown};
use ninjaedit_core::{Token, TokenKind, text};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget};

/// Rows scrolled per mouse wheel notch.
pub(crate) const WHEEL_LINES: usize = 3;
/// Columns scrolled per horizontal wheel notch, or by ← and →.
pub(crate) const WHEEL_COLUMNS: usize = 4;
/// Columns a pane may scroll past the longest visible line, as in the
/// editor.
pub(crate) const HSCROLL_SLACK: usize = 2;
/// Lines of context one press of an expand button reveals.
pub(crate) const EXPAND_LINES: usize = 10;
pub(crate) const TAB_WIDTH: usize = 4;
/// A button drawn in a diff's gap row, and what pressing it reveals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Button {
    /// Reveal lines above the change below the gap.
    Up(usize),
    /// Reveal lines below the change above the gap.
    Down(usize),
    /// Reveal the whole gap.
    All(usize),
}

impl Button {
    /// Press the button: reveal what it stands for.
    pub(crate) fn press(self, diff: &mut FileDiff) {
        match self {
            Button::Up(gap) => diff.expand_up(gap, EXPAND_LINES),
            Button::Down(gap) => diff.expand_down(gap, EXPAND_LINES),
            Button::All(gap) => diff.expand_all(gap),
        }
    }
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
/// don't scroll and don't count.
pub(crate) fn diff_extent(diff: &FileDiff, rows: &[DiffRow], scroll: usize, count: usize) -> usize {
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
}

/// Draw a diff into `area`, scrolled by `scroll` rows and sideways by
/// `h`, recording the expand buttons drawn.
pub(crate) fn render_diff(
    diff: &FileDiff,
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    scroll: usize,
    h: &mut HScroll,
    buttons: &mut Vec<(Rect, Button)>,
) -> DiffDrawn {
    let background = content_background(theme);
    let dim = background.fg(theme.command_palette_result_context_text);
    let height = area.height as usize;
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
        };
    }
    let rows = diff.rows();
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
        };
    }
    let text_width = area.right() - text_x;
    // The sideways scrollbar takes the last row; see `render_log`.
    let capacity = text_width as usize;
    let mut show_bar = false;
    let mut shown;
    let mut scroll = scroll;
    let mut extent;
    loop {
        shown = height - usize::from(show_bar);
        scroll = scroll.min(rows.len().saturating_sub(shown));
        extent = diff_extent(diff, &rows, scroll, shown);
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

    for (row, entry) in rows.iter().skip(scroll).take(shown).enumerate() {
        let y = area.y + row as u16;
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
            }
            DiffRow::Gap {
                gap,
                hidden,
                up,
                down,
            } => {
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
                if *up {
                    place(format!(" ▲ {step} "), button_style, Some(Button::Up(*gap)));
                }
                if *down {
                    place(
                        format!(" ▼ {step} "),
                        button_style,
                        Some(Button::Down(*gap)),
                    );
                }
                if *hidden > EXPAND_LINES {
                    place(" all ".to_owned(), button_style, Some(Button::All(*gap)));
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
