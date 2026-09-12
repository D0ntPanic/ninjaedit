//! The go to line box (Ctrl+L): a one-line input floating over the top of
//! the editor, like the search box, into which a line number is typed.
//!
//! The box only owns the number's text and its look. Enter hands the
//! number to the application, which moves the active editor's cursor to
//! that line; a number out of range is clamped there to the first or last
//! line. With something that isn't a number typed, Enter keeps the box
//! open, its hint saying what it wants.

use crate::clipboard::Clipboard;
use crate::input::{Input, InputKey};
use crate::palette::{MAX_WIDTH, render_frame};
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};

const PLACEHOLDER: &str = "Go to line";
const INVALID_HINT: &str = "enter a line number";
/// The box's height, border included.
pub const HEIGHT: u16 = 3;

/// What the application should do after the box handled an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GoToLineOutcome {
    /// Keep the box open.
    Continue,
    /// Escape: close the box.
    Close,
    /// Enter with a number typed: go to that line, counted from one, and
    /// close the box. Zero means the first line.
    Accept(usize),
}

pub struct GoToLineBox {
    input: Input,
    /// How many lines the file has, shown in the bottom border so that
    /// the range is known.
    line_count: usize,
    /// Whether Enter was last pressed with something other than a number
    /// typed, in which case the hint says so until the text changes.
    invalid: bool,
    /// The whole box, including its border, from the last render.
    area: Rect,
}

impl GoToLineBox {
    pub fn new(line_count: usize) -> GoToLineBox {
        GoToLineBox {
            input: Input::new(),
            line_count,
            invalid: false,
            area: Rect::default(),
        }
    }

    /// The text typed so far.
    #[cfg(test)]
    pub fn text(&self) -> &str {
        self.input.text()
    }

    /// The line number typed so far, counted from one, if the text is
    /// one. Surrounding spaces are allowed; a number too long for the
    /// machine is treated as one past the end.
    pub fn line_number(&self) -> Option<usize> {
        let text = self.input.text().trim();
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some(text.parse().unwrap_or(usize::MAX))
    }

    pub fn handle_key(&mut self, key: KeyEvent, clipboard: &mut Clipboard) -> GoToLineOutcome {
        match key.code {
            KeyCode::Esc => GoToLineOutcome::Close,
            KeyCode::Enter => match self.line_number() {
                Some(line) => GoToLineOutcome::Accept(line),
                None => {
                    self.invalid = true;
                    GoToLineOutcome::Continue
                }
            },
            _ => {
                if self.input.handle_key(key, clipboard) == InputKey::Changed {
                    self.invalid = false;
                }
                GoToLineOutcome::Continue
            }
        }
    }

    /// Add pasted text to the number.
    pub fn paste(&mut self, text: &str) {
        if self.input.paste(text) {
            self.invalid = false;
        }
    }

    /// Whether the mouse position is over the box.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.area.contains(ScreenPosition::new(x, y))
    }

    /// Whether a drag that started in the text is going on, in which case
    /// the box wants drag and release events wherever they happen.
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
        let hint = if self.invalid {
            INVALID_HINT.to_owned()
        } else {
            format!("{} lines", self.line_count)
        };
        let inner = render_frame(self.area, Some(&hint), buf, theme);
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
