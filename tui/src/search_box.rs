//! The search box: the command palette's input row without the results,
//! floating over the top of the editor while a search is typed.
//!
//! The box only owns the query text and its look; what the query finds
//! belongs to the editor's [`ninjaedit_core::Search`], which the
//! application updates as the box reports changes. The hint in the bottom
//! border (the match count, an error in a regular expression) is set by
//! the application for the same reason.

use crate::palette::{MAX_WIDTH, render_frame, render_input};
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};

const PLACEHOLDER: &str = "Search in file (start with / for a regular expression)";
/// The box's height, border included.
pub const HEIGHT: u16 = 3;

/// What the application should do after the box handled an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchOutcome {
    /// Nothing changed.
    Continue,
    /// The query changed.
    Changed,
    /// Escape: close the box and forget the search.
    Close,
    /// Enter: select the current match.
    Accept,
    /// Ctrl+G: move on to the next match.
    Next,
}

#[derive(Default)]
pub struct SearchBox {
    query: String,
    hint: Option<String>,
    /// The whole box, including its border, from the last render.
    area: Rect,
}

impl SearchBox {
    pub fn new() -> SearchBox {
        SearchBox::default()
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn set_hint(&mut self, hint: Option<String>) {
        self.hint = hint;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SearchOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => SearchOutcome::Close,
            KeyCode::Enter => SearchOutcome::Accept,
            KeyCode::Char('g') if ctrl => SearchOutcome::Next,
            KeyCode::Backspace => match self.query.pop() {
                Some(_) => SearchOutcome::Changed,
                None => SearchOutcome::Continue,
            },
            KeyCode::Char('u') if ctrl => {
                if self.query.is_empty() {
                    SearchOutcome::Continue
                } else {
                    self.query.clear();
                    SearchOutcome::Changed
                }
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.query.push(c);
                SearchOutcome::Changed
            }
            _ => SearchOutcome::Continue,
        }
    }

    /// Add pasted text to the query. A match can't span lines, so line
    /// breaks are dropped. Returns whether the query changed.
    pub fn paste(&mut self, text: &str) -> bool {
        let before = self.query.len();
        self.query
            .extend(text.chars().filter(|c| *c != '\n' && *c != '\r'));
        self.query.len() != before
    }

    /// Whether the mouse position is over the box.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.area.contains(ScreenPosition::new(x, y))
    }

    /// Draw the box over the top center of `screen`. Returns where the
    /// terminal cursor belongs.
    pub fn render(
        &mut self,
        screen: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) -> Option<ScreenPosition> {
        let width = MAX_WIDTH.min(screen.width.saturating_sub(2));
        if width < 10 || screen.height < 3 {
            return None;
        }
        let x = screen.x + (screen.width - width) / 2;
        self.area = Rect::new(x, screen.y + 1, width, HEIGHT);
        let inner = render_frame(self.area, self.hint.as_deref(), buf, theme);
        if inner.height == 0 {
            return None;
        }
        Some(render_input(
            Rect::new(inner.x, inner.y, inner.width, 1),
            &self.query,
            PLACEHOLDER,
            buf,
            theme,
        ))
    }
}
