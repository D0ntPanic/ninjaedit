//! The tab bar: one tab per open file, with a close button on each.
//!
//! The bar keeps the screen extents of the tabs it drew last so mouse clicks
//! can be resolved to a tab or its close button. When the tabs don't all
//! fit, the bar scrolls horizontally just enough to keep the active tab in
//! view.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

const CLOSE: &str = "×";
const SEPARATOR: &str = "│";

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
    pub fn render(&mut self, area: Rect, buf: &mut Buffer, tabs: &[TabLabel], active: usize) {
        self.area = area;
        self.extents.clear();
        if area.height == 0 || tabs.is_empty() {
            return;
        }

        // Lay the tabs out end to end in bar coordinates, then choose an
        // offset that keeps the active one visible.
        let labels: Vec<String> = tabs
            .iter()
            .map(|tab| {
                let dot = if tab.modified { "•" } else { "" };
                format!(" {}{} ", dot, tab.title)
            })
            .collect();
        let mut positions = Vec::with_capacity(tabs.len());
        let mut x = 0usize;
        for label in &labels {
            let width = Span::raw(label).width() + 2; // label, ×, space
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

        let inactive = Style::default().fg(Color::DarkGray);
        let active_style = Style::default().add_modifier(Modifier::BOLD);
        let visible = self.offset..self.offset + width;
        for (index, (label, &(start, end))) in labels.iter().zip(&positions).enumerate() {
            if end <= visible.start || start >= visible.end {
                continue;
            }
            let style = if index == active {
                active_style
            } else {
                inactive
            };
            let mut column = start;
            let mut close = None;
            // Draw the label one cell at a time so it can be cropped at the
            // edges of the bar.
            for grapheme in Span::styled(label, style).styled_graphemes(style) {
                let w = Span::raw(grapheme.symbol).width();
                if visible.contains(&column) && column + w <= visible.end {
                    let x = area.x + (column - visible.start) as u16;
                    buf.set_string(x, area.y, grapheme.symbol, style);
                }
                column += w;
            }
            if visible.contains(&column) {
                let x = area.x + (column - visible.start) as u16;
                buf.set_string(x, area.y, CLOSE, style);
                close = Some(x);
            }
            column += 2;
            if index + 1 < labels.len() && visible.contains(&column) {
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
