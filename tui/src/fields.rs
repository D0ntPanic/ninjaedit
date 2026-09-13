//! Several one-line inputs with the keyboard focus moving between them:
//! what a settings page, or any form, is made of.
//!
//! [`Fields`] owns an [`Input`] per field and knows which one has the
//! focus. Tab and Down move the focus to the next field and Shift+Tab
//! and Up to the previous (Tab wraps around at the ends, the arrows
//! stop); every other key goes to the focused field, which edits as a
//! text field does. A click in a field's row focuses it and places the
//! cursor, and a drag that started in a field stays with it. The fields
//! report when the focus moves, and from where, so the caller can act
//! on the field just left (a settings page applies its value).
//!
//! The caller lays the fields out, since what goes around them differs
//! from form to form, and draws each through [`render_field`], which
//! draws the terminal cursor only in the focused one.
//!
//! [`render_field`]: Fields::render_field

use crate::clipboard::Clipboard;
use crate::input::{Input, InputKey};
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::text::Span;

/// Break text into lines no wider than `width` columns at the spaces,
/// for a note under a field. A word wider than a line is cut where the
/// line ends. Blank text is one empty line, so a note keeps its row.
pub fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut line_width = 0;
    for word in text.split_whitespace() {
        let mut word = word;
        let mut word_width = Span::raw(word).width();
        // A word that doesn't fit on the line goes to the next; one
        // wider than a line is cut into pieces that fit.
        if line_width > 0 && line_width + 1 + word_width > width {
            lines.push(std::mem::take(&mut line));
            line_width = 0;
        }
        while word_width > width {
            let mut taken = 0;
            let mut cut = 0;
            for (offset, c) in word.char_indices() {
                let w = Span::raw(&*c.encode_utf8(&mut [0; 4])).width();
                if taken + w > width {
                    break;
                }
                taken += w;
                cut = offset + c.len_utf8();
            }
            if cut == 0 {
                break;
            }
            lines.push(word[..cut].to_owned());
            word = &word[cut..];
            word_width = Span::raw(word).width();
        }
        if line_width > 0 {
            line.push(' ');
            line_width += 1;
        }
        line.push_str(word);
        line_width += word_width;
    }
    lines.push(line);
    lines
}

/// Whether a key meant something to the fields, and what.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKey {
    /// Not a key of the fields'; the caller decides what it does.
    Ignored,
    /// The focused field handled it without changing its text.
    Unchanged,
    /// The focused field's text changed.
    Changed,
    /// The focus moved from one field to another.
    Moved { from: usize, to: usize },
}

pub struct Fields {
    inputs: Vec<Input>,
    focused: usize,
    /// The row each field was last drawn in (the whole row, prompt
    /// included), or an empty rectangle for one that wasn't drawn, to
    /// hit-test clicks.
    rows: Vec<Rect>,
}

impl Fields {
    /// `count` empty fields, the first focused.
    pub fn new(count: usize) -> Fields {
        Fields {
            inputs: (0..count).map(|_| Input::new()).collect(),
            focused: 0,
            rows: vec![Rect::default(); count],
        }
    }

    pub fn len(&self) -> usize {
        self.inputs.len()
    }

    /// The index of the focused field.
    pub fn focused(&self) -> usize {
        self.focused
    }

    #[cfg(test)]
    pub fn input(&self, index: usize) -> &Input {
        &self.inputs[index]
    }

    pub fn text(&self, index: usize) -> &str {
        self.inputs[index].text()
    }

    /// Replace a field's text, with the cursor at its end.
    pub fn set_text(&mut self, index: usize, text: &str) {
        self.inputs[index].set_text(text);
    }

    /// Move the focus to a field. Returns where it moved from, if it
    /// moved. The field losing the focus loses its selection too, as a
    /// text field in a graphical program does.
    pub fn focus(&mut self, index: usize) -> Option<usize> {
        if index >= self.inputs.len() || index == self.focused {
            return None;
        }
        let from = self.focused;
        self.inputs[from].clear_selection();
        self.focused = index;
        Some(from)
    }

    fn moved(&mut self, to: usize) -> FieldKey {
        match self.focus(to) {
            Some(from) => FieldKey::Moved {
                from,
                to: self.focused,
            },
            None => FieldKey::Unchanged,
        }
    }

    /// Whether a mouse drag is going on in a field, in which case the
    /// fields want drag and release events wherever they happen.
    pub fn is_dragging(&self) -> bool {
        self.inputs.iter().any(Input::is_dragging)
    }

    /// The field whose row is at a screen position.
    pub fn field_at(&self, x: u16, y: u16) -> Option<usize> {
        let at = ScreenPosition::new(x, y);
        self.rows.iter().position(|row| row.contains(at))
    }

    // ----- Keyboard -------------------------------------------------------

    /// Handle a key: the focus keys move between fields, anything else
    /// goes to the focused field.
    pub fn handle_key(&mut self, key: KeyEvent, clipboard: &mut Clipboard) -> FieldKey {
        if self.inputs.is_empty() {
            return FieldKey::Ignored;
        }
        let last = self.inputs.len() - 1;
        match key.code {
            KeyCode::Tab => {
                let next = if self.focused == last {
                    0
                } else {
                    self.focused + 1
                };
                self.moved(next)
            }
            KeyCode::BackTab => {
                let previous = if self.focused == 0 {
                    last
                } else {
                    self.focused - 1
                };
                self.moved(previous)
            }
            KeyCode::Down => self.moved((self.focused + 1).min(last)),
            KeyCode::Up => self.moved(self.focused.saturating_sub(1)),
            _ => match self.inputs[self.focused].handle_key(key, clipboard) {
                InputKey::Ignored => FieldKey::Ignored,
                InputKey::Unchanged => FieldKey::Unchanged,
                InputKey::Changed => FieldKey::Changed,
            },
        }
    }

    /// Add pasted text to the focused field. Returns whether its text
    /// changed.
    pub fn paste(&mut self, text: &str) -> bool {
        match self.inputs.get_mut(self.focused) {
            Some(input) => input.paste(text),
            None => false,
        }
    }

    // ----- Mouse ----------------------------------------------------------

    /// Handle a mouse event: a press in a field's row focuses that field
    /// (returning where the focus came from, if it moved) and goes to it;
    /// a drag or release goes to the field being dragged in.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Option<usize> {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let index = self.field_at(mouse.column, mouse.row)?;
                let from = self.focus(index);
                self.inputs[index].handle_mouse(mouse);
                from
            }
            MouseEventKind::Drag(_) | MouseEventKind::Up(_) => {
                if let Some(input) = self.inputs.iter_mut().find(|input| input.is_dragging()) {
                    input.handle_mouse(mouse);
                }
                None
            }
            _ => None,
        }
    }

    // ----- Rendering ------------------------------------------------------

    /// Forget where the fields were drawn, before a render that may leave
    /// some of them out (scrolled off the page), so a click where one
    /// used to be doesn't reach it.
    pub fn clear_layout(&mut self) {
        self.rows.fill(Rect::default());
    }

    /// Draw one field into `row`, with `placeholder` while it is empty.
    /// Returns where the terminal cursor belongs when the field is the
    /// focused one (and has no selection), else `None`.
    pub fn render_field(
        &mut self,
        index: usize,
        row: Rect,
        placeholder: &str,
        buf: &mut Buffer,
        theme: &Theme,
    ) -> Option<ScreenPosition> {
        self.rows[index] = row;
        let cursor = self.inputs[index].render(row, placeholder, buf, theme);
        (index == self.focused).then_some(cursor).flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn render(fields: &mut Fields) -> (Buffer, Vec<Option<ScreenPosition>>) {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 6));
        fields.clear_layout();
        let cursors = (0..fields.len())
            .map(|i| {
                fields.render_field(
                    i,
                    Rect::new(2, 2 * i as u16, 16, 1),
                    "empty",
                    &mut buf,
                    &Theme::default(),
                )
            })
            .collect();
        (buf, cursors)
    }

    #[test]
    fn wraps_at_spaces_and_cuts_long_words() {
        assert_eq!(wrap_words("", 10), vec![""]);
        assert_eq!(wrap_words("short", 10), vec!["short"]);
        assert_eq!(
            wrap_words("the quick brown fox jumps", 10),
            vec!["the quick", "brown fox", "jumps"]
        );
        assert_eq!(
            wrap_words("a exactly-10 b", 10),
            vec!["a", "exactly-10", "b"]
        );
        assert_eq!(
            wrap_words("ab abcdefghijklmno cd", 6),
            vec!["ab", "abcdef", "ghijkl", "mno cd"]
        );
        assert_eq!(wrap_words("  spaced   out  ", 20), vec!["spaced out"]);
        assert_eq!(wrap_words("x", 0), vec!["x"]);
    }

    #[test]
    fn focus_moves_with_tab_and_the_arrows() {
        let mut clipboard = Clipboard::local_only();
        let mut fields = Fields::new(3);
        assert_eq!(fields.focused(), 0);
        assert_eq!(
            fields.handle_key(key(KeyCode::Up), &mut clipboard),
            FieldKey::Unchanged,
            "the arrows stop at the ends"
        );
        assert_eq!(
            fields.handle_key(key(KeyCode::Down), &mut clipboard),
            FieldKey::Moved { from: 0, to: 1 }
        );
        assert_eq!(
            fields.handle_key(key(KeyCode::Tab), &mut clipboard),
            FieldKey::Moved { from: 1, to: 2 }
        );
        assert_eq!(
            fields.handle_key(key(KeyCode::Down), &mut clipboard),
            FieldKey::Unchanged
        );
        assert_eq!(
            fields.handle_key(key(KeyCode::Tab), &mut clipboard),
            FieldKey::Moved { from: 2, to: 0 },
            "Tab wraps around"
        );
        assert_eq!(
            fields.handle_key(key(KeyCode::BackTab), &mut clipboard),
            FieldKey::Moved { from: 0, to: 2 }
        );
        assert_eq!(
            fields.handle_key(key(KeyCode::Up), &mut clipboard),
            FieldKey::Moved { from: 2, to: 1 }
        );
        // Typing goes to the focused field only.
        assert_eq!(
            fields.handle_key(key(KeyCode::Char('x')), &mut clipboard),
            FieldKey::Changed
        );
        assert_eq!(fields.text(1), "x");
        assert_eq!(fields.text(0), "");
        assert_eq!(
            fields.handle_key(key(KeyCode::Enter), &mut clipboard),
            FieldKey::Ignored
        );
        assert!(fields.paste("yz"));
        assert_eq!(fields.text(1), "xyz");
    }

    #[test]
    fn only_the_focused_field_shows_the_cursor_and_clicks_move_it() {
        let mut fields = Fields::new(2);
        fields.set_text(0, "one");
        fields.set_text(1, "two");
        let (buf, cursors) = render(&mut fields);
        assert_eq!(cursors[0], Some(ScreenPosition::new(7, 0)));
        assert_eq!(cursors[1], None);
        let row: String = (0..20).map(|x| buf[(x, 2)].symbol().to_owned()).collect();
        assert_eq!(row.trim_end(), "  > two");

        // A click in the second row focuses its field and places the
        // cursor; the first field's selection goes away.
        let mut clipboard = Clipboard::local_only();
        fields.handle_key(
            KeyEvent {
                code: KeyCode::Char('a'),
                modifiers: KeyModifiers::CONTROL,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            },
            &mut clipboard,
        );
        assert!(fields.input(0).edit().selection().is_some());
        let press = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(fields.handle_mouse(press), Some(0));
        assert_eq!(fields.focused(), 1);
        assert_eq!(fields.input(1).edit().cursor(), 1);
        assert_eq!(fields.input(0).edit().selection(), None);
        assert!(fields.is_dragging());
        let (_, cursors) = render(&mut fields);
        assert_eq!(cursors[0], None);
        assert_eq!(cursors[1], Some(ScreenPosition::new(5, 2)));
        // A click on a row that wasn't drawn hits nothing.
        fields.clear_layout();
        assert_eq!(fields.field_at(5, 2), None);
    }
}
