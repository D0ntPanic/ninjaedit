//! A one-line text input: the command palette's and the search box's
//! query field.
//!
//! The text itself, with its cursor and selection, is a
//! [`LineEdit`] from the core crate. This widget maps keys and the mouse
//! onto it, with the bindings a text field in a graphical program has:
//! the arrow keys move by character and with Ctrl by word, Shift with
//! any of them selects, Home and End go to the ends, Ctrl+A selects
//! everything, Ctrl+C, Ctrl+X, and Ctrl+V go through the system
//! clipboard, and Ctrl+Backspace and Ctrl+Delete take out a word (Ctrl+U,
//! the whole line). Clicking places the cursor, dragging selects, and a
//! double-click selects a word.
//!
//! The widget also owns how the text is shown: a prompt, then the text
//! (or a dimmed placeholder while it is empty), scrolled sideways as
//! needed to keep the cursor in view.

use crate::clicks::ClickTracker;
use crate::clipboard::Clipboard;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ninjaedit_core::LineEdit;
use ninjaedit_core::line_edit::Movement;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::Style;

const PROMPT: &str = "> ";

/// Whether a key meant something to the input, and if so whether it
/// changed the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKey {
    /// Not an editing key; the caller decides what it does.
    Ignored,
    /// The input handled it but the text is as it was (a movement, a
    /// copy).
    Unchanged,
    Changed,
}

#[derive(Default)]
pub struct Input {
    edit: LineEdit,
    /// First visible display column of the text.
    scroll: usize,
    /// Where the text (after the prompt) was drawn at the last render.
    area: Rect,
    /// Whether a mouse drag is extending the selection.
    dragging: bool,
    clicks: ClickTracker,
}

impl Input {
    pub fn new() -> Input {
        Input::default()
    }

    pub fn text(&self) -> &str {
        self.edit.text()
    }

    /// The text, cursor, and selection.
    #[cfg(test)]
    pub fn edit(&self) -> &LineEdit {
        &self.edit
    }

    /// Replace the text, leaving it all selected so that typing replaces
    /// it and Backspace clears it.
    pub fn set_text_selected(&mut self, text: &str) {
        self.edit.set_text(text);
        self.edit.select_all();
    }

    /// Select all of the text, so that typing replaces it.
    pub fn select_all(&mut self) {
        self.edit.select_all();
    }

    /// Whether a mouse drag is in progress, in which case the input wants
    /// drag and release events even outside its area.
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// Whether the screen position is over the text field.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.area.contains(ScreenPosition::new(x, y))
    }

    // ----- Keyboard -------------------------------------------------------

    /// Handle an editing key. `clipboard` is read by paste and replaced
    /// by copy and cut.
    pub fn handle_key(&mut self, key: KeyEvent, clipboard: &mut Clipboard) -> InputKey {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let movement = match key.code {
            KeyCode::Left if ctrl => Some(Movement::WordLeft),
            KeyCode::Right if ctrl => Some(Movement::WordRight),
            KeyCode::Left => Some(Movement::Left),
            KeyCode::Right => Some(Movement::Right),
            KeyCode::Home => Some(Movement::Start),
            KeyCode::End => Some(Movement::End),
            _ => None,
        };
        if let Some(movement) = movement {
            self.edit.move_cursor(movement, shift);
            return InputKey::Unchanged;
        }
        let changed = match key.code {
            KeyCode::Backspace if ctrl || alt => self.edit.delete_word_left(),
            KeyCode::Delete if ctrl || alt => self.edit.delete_word_right(),
            KeyCode::Backspace => self.edit.backspace(),
            KeyCode::Delete => self.edit.delete_forward(),
            KeyCode::Char(c) if ctrl => match c.to_ascii_lowercase() {
                'a' => {
                    self.edit.select_all();
                    false
                }
                'u' => {
                    let was_empty = self.edit.is_empty();
                    self.edit.clear();
                    !was_empty
                }
                'c' => {
                    if let Some(text) = self.edit.copy() {
                        clipboard.set(text);
                    }
                    false
                }
                'x' => match self.edit.cut() {
                    Some(text) => {
                        clipboard.set(text);
                        true
                    }
                    None => false,
                },
                'v' => match clipboard.get() {
                    Some(text) => self.edit.insert(&text),
                    None => false,
                },
                _ => return InputKey::Ignored,
            },
            KeyCode::Char(c) if !alt => self.edit.insert(c.encode_utf8(&mut [0; 4])),
            _ => return InputKey::Ignored,
        };
        if changed {
            InputKey::Changed
        } else {
            InputKey::Unchanged
        }
    }

    /// Add pasted text, replacing the selection. Returns whether the text
    /// changed.
    pub fn paste(&mut self, text: &str) -> bool {
        self.edit.insert(text)
    }

    // ----- Mouse ----------------------------------------------------------

    /// Handle a mouse event over the input, or a drag that started in it.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        let (x, y) = (mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if self.contains(x, y) => {
                let offset = self.offset_at(x);
                self.dragging = true;
                if self.clicks.press(x, y) == 2 {
                    self.edit.select_word_at(offset);
                } else if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                    let anchor = self.edit.anchor().unwrap_or(self.edit.cursor());
                    self.edit.set_selection(anchor, offset);
                } else {
                    self.edit.set_cursor(offset);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                let anchor = self.edit.anchor().unwrap_or(self.edit.cursor());
                self.edit.set_selection(anchor, self.offset_at(x));
            }
            MouseEventKind::Up(MouseButton::Left) => self.dragging = false,
            _ => {}
        }
    }

    /// The text offset under a screen column. Columns past either edge
    /// map to one column beyond it, so dragging past an edge scrolls.
    fn offset_at(&self, x: u16) -> usize {
        let column = if x < self.area.x {
            self.scroll.saturating_sub(1)
        } else if x >= self.area.right() {
            self.scroll + self.area.width as usize
        } else {
            self.scroll + (x - self.area.x) as usize
        };
        self.edit.offset_at_column(column)
    }

    // ----- Rendering ------------------------------------------------------

    /// Draw the input into `row`: the prompt, then the text or, while it
    /// is empty, the dimmed placeholder. Returns where the terminal cursor
    /// belongs, or `None` with a selection, whose highlight marks the
    /// cursor's end well enough on its own.
    pub fn render(
        &mut self,
        row: Rect,
        placeholder: &str,
        buf: &mut Buffer,
        theme: &Theme,
    ) -> Option<ScreenPosition> {
        let style = Style::default()
            .fg(theme.command_palette_input_text)
            .bg(theme.command_palette_input_background);
        buf.set_style(row, style);
        buf.set_stringn(
            row.x,
            row.y,
            PROMPT,
            row.width as usize,
            style.fg(theme.command_palette_box_color),
        );
        let prompt_width = (PROMPT.len() as u16).min(row.width);
        self.area = Rect::new(
            row.x + prompt_width,
            row.y,
            row.width - prompt_width,
            row.height.min(1),
        );
        let width = self.area.width as usize;
        if width == 0 {
            return None;
        }
        if self.edit.is_empty() {
            buf.set_stringn(
                self.area.x,
                self.area.y,
                placeholder,
                width,
                style.fg(theme.command_palette_placeholder_text),
            );
            self.scroll = 0;
            return Some(ScreenPosition::new(self.area.x, self.area.y));
        }

        // Scroll no further than needed to show the end of the text and a
        // cell for the cursor after it, then as needed to show the cursor.
        let cursor = self.edit.column_of(self.edit.cursor());
        self.scroll = self
            .scroll
            .min((self.edit.width() + 1).saturating_sub(width));
        if cursor < self.scroll {
            self.scroll = cursor;
        } else if cursor >= self.scroll + width {
            self.scroll = cursor + 1 - width;
        }

        let selection = self.edit.selection();
        let mut selected = Style::default().bg(theme.selection_background);
        if let Some(color) = theme.selection_text {
            selected = selected.fg(color);
        }
        let visible = self.scroll..self.scroll + width;
        for cell in self.edit.cells() {
            if cell.width == 0 || cell.column + cell.width <= visible.start {
                continue;
            }
            if cell.column >= visible.end {
                break;
            }
            let style = match &selection {
                Some(range) if range.contains(&cell.range.start) => style.patch(selected),
                _ => style,
            };
            let fits = cell.column >= visible.start && cell.column + cell.width <= visible.end;
            let start = cell.column.max(visible.start);
            let end = (cell.column + cell.width).min(visible.end);
            let x = self.area.x + (start - visible.start) as u16;
            if cell.text == "\t" || !fits {
                // Tabs are blank; a wide character cut off by the edge
                // shows as blank for the part that fits.
                for x in x..x + (end - start) as u16 {
                    buf[(x, self.area.y)].set_symbol(" ").set_style(style);
                }
            } else {
                buf.set_string(x, self.area.y, cell.text, style);
            }
        }
        (selection.is_none())
            .then(|| ScreenPosition::new(self.area.x + (cursor - self.scroll) as u16, self.area.y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    /// A clipboard with no system backing, so tests don't touch the real
    /// one.
    fn clipboard() -> Clipboard {
        Clipboard::local_only()
    }

    fn type_str(input: &mut Input, clipboard: &mut Clipboard, s: &str) {
        for c in s.chars() {
            input.handle_key(key(KeyCode::Char(c), KeyModifiers::NONE), clipboard);
        }
    }

    fn row(input: &mut Input, width: u16) -> (String, Option<ScreenPosition>, Buffer) {
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        let cursor = input.render(area, "type here", &mut buf, &Theme::default());
        let text: String = (0..width)
            .map(|x| buf[(x, 0)].symbol().to_owned())
            .collect();
        (text, cursor, buf)
    }

    #[test]
    fn keys_edit_move_select_and_use_the_clipboard() {
        let mut clipboard = clipboard();
        let mut input = Input::new();
        type_str(&mut input, &mut clipboard, "hello world");
        assert_eq!(
            input.handle_key(key(KeyCode::Left, KeyModifiers::CONTROL), &mut clipboard),
            InputKey::Unchanged
        );
        assert_eq!(input.edit().cursor(), 6);
        input.handle_key(key(KeyCode::End, KeyModifiers::SHIFT), &mut clipboard);
        assert_eq!(input.edit().selected_text(), Some("world"));
        assert_eq!(
            input.handle_key(
                key(KeyCode::Char('c'), KeyModifiers::CONTROL),
                &mut clipboard
            ),
            InputKey::Unchanged
        );
        assert_eq!(clipboard.get().as_deref(), Some("world"));
        assert_eq!(
            input.handle_key(
                key(KeyCode::Char('x'), KeyModifiers::CONTROL),
                &mut clipboard
            ),
            InputKey::Changed
        );
        assert_eq!(input.text(), "hello ");
        input.handle_key(key(KeyCode::Home, KeyModifiers::NONE), &mut clipboard);
        assert_eq!(
            input.handle_key(
                key(KeyCode::Char('v'), KeyModifiers::CONTROL),
                &mut clipboard
            ),
            InputKey::Changed
        );
        assert_eq!(input.text(), "worldhello ");
        assert_eq!(input.edit().cursor(), 5);
        // Ctrl+A then a typed character replaces everything.
        input.handle_key(
            key(KeyCode::Char('a'), KeyModifiers::CONTROL),
            &mut clipboard,
        );
        assert_eq!(input.edit().selection(), Some(0..11));
        assert_eq!(
            input.handle_key(key(KeyCode::Char('z'), KeyModifiers::NONE), &mut clipboard),
            InputKey::Changed
        );
        assert_eq!(input.text(), "z");
        // Word deletion, and Ctrl+U for the lot.
        type_str(&mut input, &mut clipboard, " one two");
        input.handle_key(
            key(KeyCode::Backspace, KeyModifiers::CONTROL),
            &mut clipboard,
        );
        assert_eq!(input.text(), "z one ");
        input.handle_key(key(KeyCode::Backspace, KeyModifiers::ALT), &mut clipboard);
        assert_eq!(input.text(), "z ");
        assert_eq!(
            input.handle_key(
                key(KeyCode::Char('u'), KeyModifiers::CONTROL),
                &mut clipboard
            ),
            InputKey::Changed
        );
        assert_eq!(input.text(), "");
        assert_eq!(
            input.handle_key(
                key(KeyCode::Char('u'), KeyModifiers::CONTROL),
                &mut clipboard
            ),
            InputKey::Unchanged
        );
        assert_eq!(
            input.handle_key(key(KeyCode::Backspace, KeyModifiers::NONE), &mut clipboard),
            InputKey::Unchanged
        );
        // Keys the input doesn't use are left to the caller.
        assert_eq!(
            input.handle_key(key(KeyCode::Enter, KeyModifiers::NONE), &mut clipboard),
            InputKey::Ignored
        );
        assert_eq!(
            input.handle_key(
                key(KeyCode::Char('g'), KeyModifiers::CONTROL),
                &mut clipboard
            ),
            InputKey::Ignored
        );
        assert_eq!(
            input.handle_key(key(KeyCode::Up, KeyModifiers::NONE), &mut clipboard),
            InputKey::Ignored
        );
    }

    #[test]
    fn renders_placeholder_text_selection_and_scrolls_to_the_cursor() {
        let theme = Theme::default();
        let mut clipboard = clipboard();
        let mut input = Input::new();
        let (text, cursor, _) = row(&mut input, 10);
        assert_eq!(text, "> type her");
        assert_eq!(cursor, Some(ScreenPosition::new(2, 0)));

        type_str(&mut input, &mut clipboard, "abcdefghij");
        let (text, cursor, _) = row(&mut input, 10);
        // Eight text columns; the cursor at the end needs one of them.
        assert_eq!(text, "> defghij ");
        assert_eq!(cursor, Some(ScreenPosition::new(9, 0)));
        input.handle_key(key(KeyCode::Home, KeyModifiers::NONE), &mut clipboard);
        let (text, cursor, _) = row(&mut input, 10);
        assert_eq!(text, "> abcdefgh");
        assert_eq!(cursor, Some(ScreenPosition::new(2, 0)));
        // Moving right within the view doesn't scroll; past its edge, it
        // scrolls just enough.
        for _ in 0..8 {
            input.handle_key(key(KeyCode::Right, KeyModifiers::NONE), &mut clipboard);
        }
        let (text, cursor, _) = row(&mut input, 10);
        assert_eq!(text, "> bcdefghi");
        assert_eq!(cursor, Some(ScreenPosition::new(9, 0)));
        // Deleting from the end scrolls back so no room is wasted.
        input.handle_key(key(KeyCode::End, KeyModifiers::NONE), &mut clipboard);
        for _ in 0..5 {
            input.handle_key(key(KeyCode::Backspace, KeyModifiers::NONE), &mut clipboard);
        }
        let (text, _, _) = row(&mut input, 10);
        assert_eq!(text, "> abcde   ");

        // A selection is highlighted and hides the cursor.
        input.handle_key(key(KeyCode::Left, KeyModifiers::SHIFT), &mut clipboard);
        input.handle_key(key(KeyCode::Left, KeyModifiers::SHIFT), &mut clipboard);
        let (text, cursor, buf) = row(&mut input, 10);
        assert_eq!(text, "> abcde   ");
        assert_eq!(cursor, None);
        assert_eq!(buf[(4, 0)].bg, theme.command_palette_input_background);
        assert_eq!(buf[(5, 0)].bg, theme.selection_background);
        assert_eq!(buf[(6, 0)].bg, theme.selection_background);
        assert_eq!(buf[(7, 0)].bg, theme.command_palette_input_background);
    }

    fn mouse(kind: MouseEventKind, x: u16, modifiers: KeyModifiers) -> MouseEvent {
        MouseEvent {
            kind,
            column: x,
            row: 0,
            modifiers,
        }
    }

    #[test]
    fn mouse_places_the_cursor_drags_and_double_clicks() {
        let mut clipboard = clipboard();
        let mut input = Input::new();
        type_str(&mut input, &mut clipboard, "foo bar");
        row(&mut input, 20);
        assert!(input.contains(2, 0) && !input.contains(1, 0));
        let down = MouseEventKind::Down(MouseButton::Left);
        let drag = MouseEventKind::Drag(MouseButton::Left);
        let up = MouseEventKind::Up(MouseButton::Left);
        input.handle_mouse(mouse(down, 3, KeyModifiers::NONE));
        assert_eq!(input.edit().cursor(), 1);
        assert!(input.is_dragging());
        input.handle_mouse(mouse(drag, 6, KeyModifiers::NONE));
        assert_eq!(input.edit().selection(), Some(1..4));
        // Dragging past the far edge selects to the end.
        input.handle_mouse(mouse(drag, 40, KeyModifiers::NONE));
        assert_eq!(input.edit().selection(), Some(1..7));
        input.handle_mouse(mouse(up, 40, KeyModifiers::NONE));
        assert!(!input.is_dragging());
        // Shift+click extends from the anchor.
        input.handle_mouse(mouse(down, 2, KeyModifiers::SHIFT));
        assert_eq!(input.edit().selection(), Some(0..1));
        input.handle_mouse(mouse(up, 2, KeyModifiers::NONE));
        // A click past the end of the text goes to the end.
        input.handle_mouse(mouse(down, 15, KeyModifiers::NONE));
        assert_eq!(input.edit().cursor(), 7);
        assert_eq!(input.edit().selection(), None);
        input.handle_mouse(mouse(up, 15, KeyModifiers::NONE));
        // Two quick presses on one cell select the word there.
        input.handle_mouse(mouse(down, 7, KeyModifiers::NONE));
        input.handle_mouse(mouse(up, 7, KeyModifiers::NONE));
        input.handle_mouse(mouse(down, 7, KeyModifiers::NONE));
        assert_eq!(input.edit().selected_text(), Some("bar"));
        // A press outside the field is ignored.
        input.handle_mouse(mouse(up, 7, KeyModifiers::NONE));
        input.handle_mouse(mouse(down, 0, KeyModifiers::NONE));
        assert_eq!(input.edit().selected_text(), Some("bar"));
    }
}
