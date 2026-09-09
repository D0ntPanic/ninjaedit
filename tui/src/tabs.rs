//! The tab bar: one tab per open file, with a close button on each. The
//! active tab is drawn with sloping sides, ◢ and ◣, so it stands out from
//! the bar like a physical tab.
//!
//! The bar keeps the screen extents of the tabs it drew last so mouse clicks
//! can be resolved to a tab or its close button. When the tabs don't all
//! fit, the bar scrolls horizontally just enough to keep the active tab in
//! view.

use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

const CLOSE: &str = "×";
const SEPARATOR: &str = "│";
/// The sides of the active tab. Drawn with the tab's background as the
/// foreground over the bar's background, so the tab appears to slope out
/// of the bar.
const LEFT_EDGE: &str = "◢";
const RIGHT_EDGE: &str = "◣";

/// What a tab looks like, as supplied by the application.
pub struct TabLabel {
    pub title: String,
    pub modified: bool,
}

/// What the mouse hit in the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabHit {
    Tab(usize),
    Close(usize),
}

/// Screen extents of one drawn tab.
struct Extent {
    index: usize,
    /// Columns covered by the tab, in screen coordinates.
    start: u16,
    end: u16,
    /// Column of the close button, if it was drawn.
    close: Option<u16>,
}

#[derive(Default)]
pub struct TabBar {
    /// How many columns of the bar are scrolled out of view to the left.
    offset: usize,
    extents: Vec<Extent>,
    area: Rect,
}

impl TabBar {
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        tabs: &[TabLabel],
        active: usize,
    ) {
        self.area = area;
        self.extents.clear();
        let inactive = Style::default()
            .fg(theme.inactive_tab_text)
            .bg(theme.inactive_tab_background);
        buf.set_style(area, inactive);
        if area.height == 0 || tabs.is_empty() {
            return;
        }

        // Lay the tabs out end to end in bar coordinates, then choose an
        // offset that keeps the active one visible.
        let labels: Vec<String> = tabs
            .iter()
            .map(|tab| {
                let dot = if tab.modified { "•" } else { "" };
                format!("{}{}", dot, tab.title)
            })
            .collect();
        let mut positions = Vec::with_capacity(tabs.len());
        let mut x = 0usize;
        for label in &labels {
            // Left edge, label, space, ×, right edge.
            let width = Span::raw(label).width() + 4;
            positions.push((x, x + width));
            x += width + 1; // separator
        }
        let width = area.width as usize;
        let (start, end) = positions[active.min(positions.len() - 1)];
        if end > self.offset + width {
            self.offset = end - width;
        }
        if start < self.offset {
            self.offset = start;
        }

        let active_style = Style::default()
            .fg(theme.active_tab_text)
            .bg(theme.active_tab_background)
            .add_modifier(Modifier::BOLD);
        let edge = Style::default()
            .fg(theme.active_tab_background)
            .bg(theme.inactive_tab_background);
        let visible = self.offset..self.offset + width;
        for (index, (label, &(start, end))) in labels.iter().zip(&positions).enumerate() {
            if end <= visible.start || start >= visible.end {
                continue;
            }
            let is_active = index == active;
            let (style, close_style) = if is_active {
                (active_style, active_style.fg(theme.active_tab_close))
            } else {
                (inactive, inactive.fg(theme.inactive_tab_close))
            };

            // The tab as a row of cells, so it can be cropped at the edges
            // of the bar.
            let span = Span::raw(label);
            let mut cells: Vec<(&str, Style)> = Vec::new();
            cells.push(if is_active {
                (LEFT_EDGE, edge)
            } else {
                (" ", style)
            });
            for grapheme in span.styled_graphemes(style) {
                cells.push((grapheme.symbol, style));
            }
            cells.push((" ", style));
            let close_cell = cells.len();
            cells.push((CLOSE, close_style));
            cells.push(if is_active {
                (RIGHT_EDGE, edge)
            } else {
                (" ", style)
            });

            let mut column = start;
            let mut close = None;
            for (i, (symbol, style)) in cells.iter().enumerate() {
                let w = Span::raw(*symbol).width();
                if visible.contains(&column) && column + w <= visible.end {
                    let x = area.x + (column - visible.start) as u16;
                    buf.set_string(x, area.y, symbol, *style);
                    if i == close_cell {
                        close = Some(x);
                    }
                }
                column += w;
            }
            // The active tab's sloping sides do the separating on their
            // own; a bar next to them only clutters.
            let beside_active = is_active || index + 1 == active;
            if index + 1 < labels.len() && !beside_active && visible.contains(&column) {
                let x = area.x + (column - visible.start) as u16;
                buf.set_string(x, area.y, SEPARATOR, inactive);
            }
            self.extents.push(Extent {
                index,
                start: area.x + start.saturating_sub(visible.start) as u16,
                end: area.x + (end.min(visible.end) - visible.start) as u16,
                close,
            });
        }
    }

    /// Resolve a mouse position to a tab or close button.
    pub fn hit(&self, x: u16, y: u16) -> Option<TabHit> {
        if y != self.area.y || y >= self.area.bottom() {
            return None;
        }
        let extent = self.extents.iter().find(|e| x >= e.start && x < e.end)?;
        if extent.close == Some(x) {
            Some(TabHit::Close(extent.index))
        } else {
            Some(TabHit::Tab(extent.index))
        }
    }
}
