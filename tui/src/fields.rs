//! Several one-line controls with the keyboard focus moving between
//! them: what a settings page, or any form, is made of.
//!
//! [`Fields`] owns a control per field and knows which one has the
//! focus. A field is one of two [kinds](FieldKind):
//!
//! * A text field, an [`Input`], which edits as a text field does.
//! * A choice among a few options, drawn side by side on the field's row
//!   with the chosen one marked. Left and Right choose the option before
//!   or after (stopping at the ends), Space the next (wrapping around),
//!   Home and End the first and last, and a click the option clicked.
//!
//! Tab and Down move the focus to the next field and Shift+Tab and Up to
//! the previous (Tab wraps around at the ends, the arrows stop); every
//! other key goes to the focused field. A click in a field's row focuses
//! it (and places a text field's cursor), and a drag that started in a
//! text field stays with it. The fields report when the focus moves, and
//! from where, so the caller can act on the field just left (a settings
//! page applies its value).
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
use ratatui::style::Style;
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

/// Symbols marking the chosen option of a choice and the others.
const CHOSEN: &str = "●";
const NOT_CHOSEN: &str = "○";
/// Columns before a choice's first option, where a text field has its
/// prompt, so the two line up.
const CHOICE_INDENT: u16 = 1;
/// Columns between the options of a choice.
const CHOICE_GAP: u16 = 2;

/// What a field edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// Free text.
    Text,
    /// One of these options, by their labels.
    Choice(Vec<String>),
}

/// One field's control.
enum Control {
    Text(Input),
    Choice(Choice),
}

/// A choice among options, shown side by side.
struct Choice {
    labels: Vec<String>,
    chosen: usize,
    /// Where each option was last drawn, marker and label, to hit-test
    /// clicks.
    areas: Vec<Rect>,
}

impl Choice {
    /// Handle a key meant for the choice.
    fn handle_key(&mut self, key: KeyEvent) -> FieldKey {
        let last = self.labels.len().saturating_sub(1);
        let chosen = match key.code {
            KeyCode::Left => self.chosen.saturating_sub(1),
            KeyCode::Right => (self.chosen + 1).min(last),
            KeyCode::Char(' ') if self.chosen == last => 0,
            KeyCode::Char(' ') => self.chosen + 1,
            KeyCode::Home => 0,
            KeyCode::End => last,
            _ => return FieldKey::Ignored,
        };
        self.choose(chosen)
    }

    fn choose(&mut self, option: usize) -> FieldKey {
        if option == self.chosen || option >= self.labels.len() {
            return FieldKey::Unchanged;
        }
        self.chosen = option;
        FieldKey::Changed
    }

    /// Draw the options into `row`, the chosen one highlighted. Returns
    /// where the terminal cursor goes if the field has the focus: on the
    /// chosen option's marker.
    fn render(&mut self, row: Rect, buf: &mut Buffer, theme: &Theme) -> Option<ScreenPosition> {
        let style = Style::default()
            .fg(theme.command_palette_placeholder_text)
            .bg(theme.command_palette_background);
        let chosen_style = Style::default()
            .fg(theme.command_palette_input_text)
            .bg(theme.selection_background);
        buf.set_style(row, style);
        self.areas.clear();
        let mut cursor = None;
        let mut x = row.x + CHOICE_INDENT.min(row.width);
        for (index, label) in self.labels.iter().enumerate() {
            let is_chosen = index == self.chosen;
            let text = format!(" {} {label} ", if is_chosen { CHOSEN } else { NOT_CHOSEN });
            let width = (Span::raw(&text).width() as u16).min(row.right().saturating_sub(x));
            let area = Rect::new(x, row.y, width, 1);
            buf.set_stringn(
                x,
                row.y,
                &text,
                width as usize,
                if is_chosen { chosen_style } else { style },
            );
            if is_chosen && width > 1 {
                cursor = Some(ScreenPosition::new(x + 1, row.y));
            }
            self.areas.push(area);
            x = (x + width + CHOICE_GAP).min(row.right());
        }
        cursor
    }
}

/// Whether a key meant something to the fields, and what.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKey {
    /// Not a key of the fields'; the caller decides what it does.
    Ignored,
    /// The focused field handled it without changing its text or
    /// choice.
    Unchanged,
    /// The focused field's text or choice changed.
    Changed,
    /// The focus moved from one field to another.
    Moved { from: usize, to: usize },
}

pub struct Fields {
    controls: Vec<Control>,
    focused: usize,
    /// The row each field was last drawn in (the whole row, prompt
    /// included), or an empty rectangle for one that wasn't drawn, to
    /// hit-test clicks.
    rows: Vec<Rect>,
}

impl Fields {
    /// `count` empty text fields, the first focused.
    pub fn new(count: usize) -> Fields {
        Fields::with_kinds(vec![FieldKind::Text; count])
    }

    /// A field of each kind, the first focused. Text fields start empty,
    /// and choices with their first option chosen.
    pub fn with_kinds(kinds: Vec<FieldKind>) -> Fields {
        let count = kinds.len();
        let controls = kinds
            .into_iter()
            .map(|kind| match kind {
                FieldKind::Text => Control::Text(Input::new()),
                FieldKind::Choice(labels) => Control::Choice(Choice {
                    labels,
                    chosen: 0,
                    areas: Vec::new(),
                }),
            })
            .collect();
        Fields {
            controls,
            focused: 0,
            rows: vec![Rect::default(); count],
        }
    }

    pub fn len(&self) -> usize {
        self.controls.len()
    }

    /// The index of the focused field.
    pub fn focused(&self) -> usize {
        self.focused
    }

    #[cfg(test)]
    pub fn input(&self, index: usize) -> &Input {
        match &self.controls[index] {
            Control::Text(input) => input,
            Control::Choice(_) => panic!("field {index} is a choice"),
        }
    }

    /// A text field's text; empty for a choice.
    pub fn text(&self, index: usize) -> &str {
        match &self.controls[index] {
            Control::Text(input) => input.text(),
            Control::Choice(_) => "",
        }
    }

    /// Replace a text field's text, with the cursor at its end. Does
    /// nothing to a choice.
    pub fn set_text(&mut self, index: usize, text: &str) {
        if let Control::Text(input) = &mut self.controls[index] {
            input.set_text(text);
        }
    }

    /// The option chosen in a choice, or `None` for a text field.
    pub fn chosen(&self, index: usize) -> Option<usize> {
        match &self.controls[index] {
            Control::Text(_) => None,
            Control::Choice(choice) => Some(choice.chosen),
        }
    }

    /// Choose an option of a choice. Does nothing to a text field, or
    /// with an option the choice doesn't have.
    pub fn choose(&mut self, index: usize, option: usize) {
        if let Control::Choice(choice) = &mut self.controls[index] {
            choice.choose(option);
        }
    }

    /// Move the focus to a field. Returns where it moved from, if it
    /// moved. The field losing the focus loses its selection too, as a
    /// text field in a graphical program does.
    pub fn focus(&mut self, index: usize) -> Option<usize> {
        if index >= self.controls.len() || index == self.focused {
            return None;
        }
        let from = self.focused;
        if let Control::Text(input) = &mut self.controls[from] {
            input.clear_selection();
        }
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
        self.inputs().any(|input| input.is_dragging())
    }

    fn inputs(&self) -> impl Iterator<Item = &Input> {
        self.controls.iter().filter_map(|control| match control {
            Control::Text(input) => Some(input),
            Control::Choice(_) => None,
        })
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
        if self.controls.is_empty() {
            return FieldKey::Ignored;
        }
        let last = self.controls.len() - 1;
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
            _ => match &mut self.controls[self.focused] {
                Control::Text(input) => match input.handle_key(key, clipboard) {
                    InputKey::Ignored => FieldKey::Ignored,
                    InputKey::Unchanged => FieldKey::Unchanged,
                    InputKey::Changed => FieldKey::Changed,
                },
                Control::Choice(choice) => choice.handle_key(key),
            },
        }
    }

    /// Add pasted text to the focused field, if it is a text field.
    /// Returns whether its text changed.
    pub fn paste(&mut self, text: &str) -> bool {
        match self.controls.get_mut(self.focused) {
            Some(Control::Text(input)) => input.paste(text),
            _ => false,
        }
    }

    // ----- Mouse ----------------------------------------------------------

    /// Handle a mouse event: a press in a field's row focuses that field
    /// (returning where the focus came from, if it moved) and goes to it,
    /// choosing the option clicked in a choice; a drag or release goes to
    /// the text field being dragged in.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Option<usize> {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let index = self.field_at(mouse.column, mouse.row)?;
                let from = self.focus(index);
                match &mut self.controls[index] {
                    Control::Text(input) => {
                        input.handle_mouse(mouse);
                    }
                    Control::Choice(choice) => {
                        let at = ScreenPosition::new(mouse.column, mouse.row);
                        if let Some(option) = choice.areas.iter().position(|a| a.contains(at)) {
                            choice.choose(option);
                        }
                    }
                }
                from
            }
            MouseEventKind::Drag(_) | MouseEventKind::Up(_) => {
                let dragging = self.controls.iter_mut().find_map(|control| match control {
                    Control::Text(input) if input.is_dragging() => Some(input),
                    _ => None,
                });
                if let Some(input) = dragging {
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

    /// Draw one field into `row`: a text field with `placeholder` while
    /// it is empty, a choice with its options. Returns where the terminal
    /// cursor belongs when the field is the focused one (and has no
    /// selection), else `None`.
    pub fn render_field(
        &mut self,
        index: usize,
        row: Rect,
        placeholder: &str,
        buf: &mut Buffer,
        theme: &Theme,
    ) -> Option<ScreenPosition> {
        self.rows[index] = row;
        let cursor = match &mut self.controls[index] {
            Control::Text(input) => input.render(row, placeholder, buf, theme),
            Control::Choice(choice) => choice.render(row, buf, theme),
        };
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

    #[test]
    fn a_choice_moves_with_the_arrows_and_space_and_clicks() {
        let mut clipboard = Clipboard::local_only();
        let mut fields = Fields::with_kinds(vec![
            FieldKind::Choice(vec!["One".into(), "Two".into(), "Three".into()]),
            FieldKind::Text,
        ]);
        assert_eq!(fields.chosen(0), Some(0));
        assert_eq!(fields.chosen(1), None);
        let mut press = |fields: &mut Fields, code| fields.handle_key(key(code), &mut clipboard);
        assert_eq!(press(&mut fields, KeyCode::Left), FieldKey::Unchanged);
        assert_eq!(press(&mut fields, KeyCode::Right), FieldKey::Changed);
        assert_eq!(fields.chosen(0), Some(1));
        assert_eq!(press(&mut fields, KeyCode::End), FieldKey::Changed);
        assert_eq!(press(&mut fields, KeyCode::Right), FieldKey::Unchanged);
        assert_eq!(
            press(&mut fields, KeyCode::Char(' ')),
            FieldKey::Changed,
            "Space wraps around"
        );
        assert_eq!(fields.chosen(0), Some(0));
        assert_eq!(press(&mut fields, KeyCode::Char('x')), FieldKey::Ignored);
        assert!(!fields.paste("text"), "a choice takes no text");
        assert_eq!(fields.text(0), "");
        fields.set_text(0, "ignored");
        assert_eq!(fields.chosen(0), Some(0));
        assert_eq!(
            press(&mut fields, KeyCode::Down),
            FieldKey::Moved { from: 0, to: 1 },
            "the focus keys still move between fields"
        );
        fields.choose(0, 2);
        assert_eq!(fields.chosen(0), Some(2));
        fields.choose(0, 7);
        assert_eq!(fields.chosen(0), Some(2), "no such option");

        // Drawn side by side, the chosen one marked, with the cursor on
        // its marker once focused.
        let (buf, cursors) = render(&mut fields);
        let row: String = (0..20).map(|x| buf[(x, 0)].symbol().to_owned()).collect();
        assert_eq!(row, "    ○ One    ○ Two  ");
        assert_eq!(cursors[0], None);
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        fields.focus(0);
        let cursor =
            fields.render_field(0, Rect::new(0, 0, 40, 1), "", &mut buf, &Theme::default());
        let row: String = (0..40).map(|x| buf[(x, 0)].symbol().to_owned()).collect();
        assert_eq!(row.trim_end(), "  ○ One    ○ Two    ● Three");
        assert_eq!(cursor, Some(ScreenPosition::new(20, 0)));

        // A click on an option chooses it.
        fields.focus(1);
        let click = |column| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(fields.handle_mouse(click(12)), Some(1));
        assert_eq!(fields.focused(), 0);
        assert_eq!(fields.chosen(0), Some(1));
        fields.handle_mouse(click(8));
        assert_eq!(fields.chosen(0), Some(1), "between options, nothing");
        assert!(!fields.is_dragging());
    }
}
