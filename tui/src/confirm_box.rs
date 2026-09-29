//! A box asking whether to go ahead with something that can't be
//! undone, floating over the page that asked: a title saying what is
//! about to happen, a few lines saying what that means, and two
//! buttons, the action and Cancel.
//!
//! The box is the page's until it is answered. `y` goes ahead and `n`
//! or Escape doesn't; ← → and Tab move between the buttons, and Enter
//! presses the one that has the keyboard, which is Cancel to begin
//! with, so that an Enter meant for whatever opened the box (a menu, a
//! palette), pressed once too often, loses nothing. A click on a button
//! presses it, the pointer over one gives it the keyboard, and a press
//! anywhere outside the box is Cancel.
//!
//! The box only asks: the page that opened it keeps what the answer is
//! about, and acts on [`ConfirmOutcome::Confirm`].

use crate::diff_pane::display_width;
use crate::fields::wrap_words;
use crate::palette::{palette_background, render_frame};
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::{Modifier, Style};

/// The widest the box gets, border included.
const MAX_WIDTH: u16 = 64;
const CANCEL: &str = "Cancel";
/// What the bottom border says.
const HINT: &str = "y yes · n/Esc no";

/// What the page should do after the box handled an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmOutcome {
    /// Keep the box open.
    Continue,
    /// Close the box, doing nothing.
    Cancel,
    /// Close the box and go ahead.
    Confirm,
}

/// One of the box's two buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    Confirm,
    Cancel,
}

pub struct ConfirmBox {
    title: String,
    body: String,
    /// The action's button: what it does, in a word or two.
    action: String,
    /// The button with the keyboard, which Enter presses.
    focused: Button,
    /// The whole box, including its border, and its buttons, from the
    /// last render.
    area: Rect,
    confirm_area: Rect,
    cancel_area: Rect,
}

impl ConfirmBox {
    pub fn new(
        title: impl Into<String>,
        body: impl Into<String>,
        action: impl Into<String>,
    ) -> ConfirmBox {
        ConfirmBox {
            title: title.into(),
            body: body.into(),
            action: action.into(),
            focused: Button::Cancel,
            area: Rect::default(),
            confirm_area: Rect::default(),
            cancel_area: Rect::default(),
        }
    }

    #[cfg(test)]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[cfg(test)]
    pub fn body(&self) -> &str {
        &self.body
    }

    fn press(&self, button: Button) -> ConfirmOutcome {
        match button {
            Button::Confirm => ConfirmOutcome::Confirm,
            Button::Cancel => ConfirmOutcome::Cancel,
        }
    }

    /// Handle a key. The box is the page's until answered, so a key it
    /// has no use for does nothing.
    pub fn handle_key(&mut self, key: KeyEvent) -> ConfirmOutcome {
        match key.code {
            KeyCode::Char('y' | 'Y') => ConfirmOutcome::Confirm,
            KeyCode::Char('n' | 'N') | KeyCode::Esc => ConfirmOutcome::Cancel,
            KeyCode::Enter => self.press(self.focused),
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                self.focused = match self.focused {
                    Button::Confirm => Button::Cancel,
                    Button::Cancel => Button::Confirm,
                };
                ConfirmOutcome::Continue
            }
            _ => ConfirmOutcome::Continue,
        }
    }

    /// Whether the mouse position is over the box.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.area.contains(ScreenPosition::new(x, y))
    }

    fn button_at(&self, x: u16, y: u16) -> Option<Button> {
        let at = ScreenPosition::new(x, y);
        if self.confirm_area.contains(at) {
            Some(Button::Confirm)
        } else if self.cancel_area.contains(at) {
            Some(Button::Cancel)
        } else {
            None
        }
    }

    /// Handle a mouse event anywhere on the screen.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> ConfirmOutcome {
        let over = self.button_at(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Moved => {
                if let Some(button) = over {
                    self.focused = button;
                }
                ConfirmOutcome::Continue
            }
            MouseEventKind::Down(MouseButton::Left) if over.is_some() => {
                self.press(over.expect("checked"))
            }
            MouseEventKind::Down(_) if !self.contains(mouse.column, mouse.row) => {
                ConfirmOutcome::Cancel
            }
            _ => ConfirmOutcome::Continue,
        }
    }

    /// Draw the box across the middle of `page`, a little above center.
    pub fn render(&mut self, page: Rect, buf: &mut Buffer, theme: &Theme) {
        self.area = Rect::default();
        self.confirm_area = Rect::default();
        self.cancel_area = Rect::default();
        let width = MAX_WIDTH.min(page.width.saturating_sub(2));
        if width < 20 {
            return;
        }
        // Inside the border, a column of space either side of the text.
        let text_width = (width - 4) as usize;
        let title = wrap_words(&self.title, text_width);
        let body = wrap_words(&self.body, text_width);
        // The border, the title, the body, a blank row, and the buttons.
        let wanted = 2 + title.len() + body.len() + 2;
        let height = (wanted as u16).min(page.height);
        if height < 5 {
            return;
        }
        let x = page.x + (page.width - width) / 2;
        let y = page.y + (page.height - height) / 3;
        self.area = Rect::new(x, y, width, height);
        let inner = render_frame(self.area, Some(HINT), buf, theme);

        let background = palette_background(theme);
        let bold = background.add_modifier(Modifier::BOLD);
        // The buttons take the last row; the text what is above it,
        // less the blank row, cut short if the page is too.
        let text_rows = inner.height.saturating_sub(2) as usize;
        let lines = title
            .iter()
            .map(|line| (line, bold))
            .chain(body.iter().map(|line| (line, background)));
        for (row, (line, style)) in lines.take(text_rows).enumerate() {
            buf.set_stringn(inner.x + 1, inner.y + row as u16, line, text_width, style);
        }

        // The buttons at the right of the last row, the action first.
        let focused = Style::default()
            .fg(theme.command_palette_selection_text)
            .bg(theme.command_palette_selection_background);
        let button_y = inner.bottom() - 1;
        let label = |text: &str| format!(" {text} ");
        let action = label(&self.action);
        let cancel = label(CANCEL);
        let cancel_width = display_width(&cancel) as u16;
        let action_width = display_width(&action) as u16;
        let cancel_x = inner.right().saturating_sub(cancel_width + 1);
        let action_x = cancel_x.saturating_sub(action_width + 2);
        if action_x <= inner.x {
            return;
        }
        let action_style = if self.focused == Button::Confirm {
            focused
        } else {
            background.fg(theme.diff_removed_text)
        };
        let cancel_style = if self.focused == Button::Cancel {
            focused
        } else {
            background
        };
        buf.set_string(action_x, button_y, &action, action_style);
        buf.set_string(cancel_x, button_y, &cancel, cancel_style);
        self.confirm_area = Rect::new(action_x, button_y, action_width, 1);
        self.cancel_area = Rect::new(cancel_x, button_y, cancel_width, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn screen_rows(buf: &Buffer) -> Vec<String> {
        let area = buf.area;
        (area.y..area.bottom())
            .map(|y| {
                (area.x..area.right())
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect()
            })
            .collect()
    }

    fn rendered(width: u16, height: u16) -> (ConfirmBox, Vec<String>) {
        let mut dialog = ConfirmBox::new(
            "Discard changes to src/?",
            "2 files go back to what is staged or committed. This can't be undone.",
            "Discard",
        );
        let page = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(page);
        dialog.render(page, &mut buf, &Theme::default());
        let rows = screen_rows(&buf);
        (dialog, rows)
    }

    #[test]
    fn keys_answer_and_enter_presses_cancel_to_begin_with() {
        let mut dialog = ConfirmBox::new("Title", "Body", "Go");
        assert_eq!(
            dialog.handle_key(key(KeyCode::Enter)),
            ConfirmOutcome::Cancel
        );
        assert_eq!(
            dialog.handle_key(key(KeyCode::Char('x'))),
            ConfirmOutcome::Continue
        );
        assert_eq!(
            dialog.handle_key(key(KeyCode::Left)),
            ConfirmOutcome::Continue
        );
        assert_eq!(
            dialog.handle_key(key(KeyCode::Enter)),
            ConfirmOutcome::Confirm
        );
        assert_eq!(
            dialog.handle_key(key(KeyCode::Tab)),
            ConfirmOutcome::Continue
        );
        assert_eq!(
            dialog.handle_key(key(KeyCode::Enter)),
            ConfirmOutcome::Cancel
        );
        assert_eq!(
            dialog.handle_key(key(KeyCode::Char('y'))),
            ConfirmOutcome::Confirm
        );
        assert_eq!(
            dialog.handle_key(key(KeyCode::Char('n'))),
            ConfirmOutcome::Cancel
        );
        assert_eq!(dialog.handle_key(key(KeyCode::Esc)), ConfirmOutcome::Cancel);
    }

    #[test]
    fn renders_the_text_wrapped_with_the_buttons_under_it() {
        let (dialog, rows) = rendered(50, 20);
        let text: Vec<&str> = rows
            .iter()
            .map(|row| row.trim_matches(|c| c == ' ' || c == '│'))
            .filter(|row| !row.is_empty())
            .collect();
        assert!(text[0].starts_with('╭'), "{rows:#?}");
        assert_eq!(text[1], "Discard changes to src/?");
        assert_eq!(text[2], "2 files go back to what is staged or");
        assert_eq!(text[3], "committed. This can't be undone.");
        assert_eq!(text[4], "Discard    Cancel");
        assert!(text[5].contains(HINT), "{rows:#?}");
        // Centered across the page, a little above the middle.
        assert_eq!(dialog.area, Rect::new(1, 4, 48, 7));
        assert!(dialog.contains(1, 4));
        assert!(!dialog.contains(0, 4));
    }

    #[test]
    fn clicks_press_buttons_and_a_press_outside_cancels() {
        let (mut dialog, _) = rendered(50, 20);
        let action = dialog.confirm_area;
        let cancel = dialog.cancel_area;
        // Hovering gives a button the keyboard, for Enter.
        dialog.handle_mouse(mouse(MouseEventKind::Moved, action.x, action.y));
        assert_eq!(
            dialog.handle_key(key(KeyCode::Enter)),
            ConfirmOutcome::Confirm
        );
        assert_eq!(
            dialog.handle_mouse(mouse(
                MouseEventKind::Down(MouseButton::Left),
                cancel.x,
                cancel.y
            )),
            ConfirmOutcome::Cancel
        );
        assert_eq!(
            dialog.handle_mouse(mouse(
                MouseEventKind::Down(MouseButton::Left),
                action.right() - 1,
                action.y
            )),
            ConfirmOutcome::Confirm
        );
        // Inside the box but on no button: nothing.
        assert_eq!(
            dialog.handle_mouse(mouse(
                MouseEventKind::Down(MouseButton::Left),
                dialog.area.x + 2,
                dialog.area.y + 1
            )),
            ConfirmOutcome::Continue
        );
        assert_eq!(
            dialog.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), 0, 0)),
            ConfirmOutcome::Cancel
        );
    }

    #[test]
    fn a_page_too_small_draws_nothing_and_takes_no_clicks_on_buttons() {
        let (dialog, rows) = rendered(15, 20);
        assert_eq!(dialog.area, Rect::default());
        assert!(rows.iter().all(|row| row.trim().is_empty()));
    }
}
