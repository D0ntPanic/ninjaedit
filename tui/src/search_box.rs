//! The search box: the command palette's input row without the results,
//! floating over the top of the editor while a search is typed.
//!
//! The box only owns the query text and its look; what the query finds
//! belongs to the editor's [`ninjaedit_core::Search`], which the
//! application updates as the box reports changes. The hint in the bottom
//! border (the match count, an error in a regular expression) is set by
//! the application for the same reason.

use crate::clipboard::Clipboard;
use crate::input::{Input, InputKey};
use crate::palette::{MAX_WIDTH, render_frame};
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
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
    input: Input,
    hint: Option<String>,
    /// The whole box, including its border, from the last render.
    area: Rect,
}

impl SearchBox {
    pub fn new() -> SearchBox {
        SearchBox::default()
    }

    pub fn query(&self) -> &str {
        self.input.text()
    }

    /// Start with a query, left selected so that typing replaces it.
    pub fn set_query_selected(&mut self, query: &str) {
        self.input.set_text_selected(query);
    }

    pub fn set_hint(&mut self, hint: Option<String>) {
        self.hint = hint;
    }

    /// The query's text, cursor, and selection.
    #[cfg(test)]
    pub fn input(&self) -> &Input {
        &self.input
    }

    pub fn handle_key(&mut self, key: KeyEvent, clipboard: &mut Clipboard) -> SearchOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => SearchOutcome::Close,
            KeyCode::Enter => SearchOutcome::Accept,
            KeyCode::Char('g') if ctrl => SearchOutcome::Next,
            _ => match self.input.handle_key(key, clipboard) {
                InputKey::Changed => SearchOutcome::Changed,
                InputKey::Unchanged | InputKey::Ignored => SearchOutcome::Continue,
            },
        }
    }

    /// Add pasted text to the query. A match can't span lines, so line
    /// breaks are dropped. Returns whether the query changed.
    pub fn paste(&mut self, text: &str) -> bool {
        self.input.paste(text)
    }

    /// Whether the mouse position is over the box.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.area.contains(ScreenPosition::new(x, y))
    }

    /// Whether a drag that started in the query is going on, in which
    /// case the box wants drag and release events wherever they happen.
    pub fn is_dragging(&self) -> bool {
        self.input.is_dragging()
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        self.input.handle_mouse(mouse);
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
        self.input.render(
            Rect::new(inner.x, inner.y, inner.width, 1),
            PLACEHOLDER,
            buf,
            theme,
        )
    }
}
