//! The settings page: the first of the editor's modes, views that stand
//! in for the editor in the upper part of the screen rather than sitting
//! below it in the tool pane. It is opened from the modes palette
//! (Ctrl+E) like a tool, and left the same way, or by anything that
//! brings a file to the front: Ctrl+O, Ctrl+T, a project search match.
//!
//! The page lists every [`SettingKey`] under its category as a text
//! field with the setting's name above it and a line about it below.
//! The fields are a [`Fields`], so Tab and the arrows move between them
//! and each edits like any text field. A value is applied when its
//! field is left (Tab, an arrow, a click elsewhere, leaving the page)
//! or with Enter; Ctrl+S applies every field. Text that isn't a valid
//! value is refused: the setting keeps its value and the reason shows
//! under the field until the text changes. Escape puts the field's text
//! back to the setting's value. A setting changed from its default is
//! tagged, and Ctrl+R puts the focused one back to its default.
//!
//! The page only edits the [`Settings`]; the application saves them and
//! applies them to what is running whenever the page reports a change.

use crate::clipboard::Clipboard;
use crate::fields::{FieldKey, Fields};
use crate::palette::palette_background;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ninjaedit_core::{Category, SettingKey, Settings};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::Modifier;
use ratatui::text::Span;

/// The name of the page, shown in its tab.
pub const TITLE: &str = "Settings";
/// The tag on a setting changed from its default.
const MODIFIED_TAG: &str = "modified";
/// The widest a field gets.
const MAX_FIELD_WIDTH: u16 = 76;
/// How far the category headers and the settings are indented.
const HEADER_INDENT: u16 = 1;
const INDENT: u16 = 3;
/// Rows scrolled per mouse wheel notch.
const WHEEL_LINES: usize = 3;

/// What the application should do after the page handled an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsOutcome {
    /// Nothing the application needs to act on.
    Continue,
    /// A setting changed: save the settings and apply them.
    Changed,
}

/// One row of the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Line {
    Blank,
    Header(Category),
    /// The name of a setting (by its index in [`SettingKey::ALL`]).
    Name(usize),
    Field(usize),
    /// The setting's description, or the reason its text was refused.
    Note(usize),
}

pub struct SettingsView {
    fields: Fields,
    /// For each field, why its text was refused, shown until the text
    /// changes.
    errors: Vec<Option<String>>,
    /// The rows of the page, in order.
    lines: Vec<Line>,
    /// How many rows are scrolled off the top.
    scroll: usize,
    /// Whether the next render should scroll to show the focused field,
    /// after the focus moved.
    reveal: bool,
    /// The page's area from the last render.
    area: Rect,
}

impl SettingsView {
    /// A page showing `settings`, its first field focused.
    pub fn new(settings: &Settings) -> SettingsView {
        let mut lines = Vec::new();
        for category in Category::ALL {
            lines.push(Line::Blank);
            lines.push(Line::Header(category));
            lines.push(Line::Blank);
            for key in category.keys() {
                let index = SettingKey::ALL
                    .iter()
                    .position(|k| *k == key)
                    .expect("every setting is listed");
                lines.push(Line::Name(index));
                lines.push(Line::Field(index));
                lines.push(Line::Note(index));
                lines.push(Line::Blank);
            }
        }
        let mut view = SettingsView {
            fields: Fields::new(SettingKey::ALL.len()),
            errors: vec![None; SettingKey::ALL.len()],
            lines,
            scroll: 0,
            reveal: true,
            area: Rect::default(),
        };
        view.refresh(settings);
        view
    }

    /// Show every setting's current value, dropping any text typed into
    /// the fields.
    pub fn refresh(&mut self, settings: &Settings) {
        for (index, key) in SettingKey::ALL.into_iter().enumerate() {
            self.fields.set_text(index, &settings.text(key));
            self.errors[index] = None;
        }
    }

    /// The setting whose field has the focus.
    #[cfg(test)]
    pub fn focused_key(&self) -> SettingKey {
        SettingKey::ALL[self.fields.focused()]
    }

    /// The text in a setting's field.
    #[cfg(test)]
    pub fn text(&self, key: SettingKey) -> &str {
        self.fields.text(index_of(key))
    }

    /// Why a setting's text was refused, if it was.
    #[cfg(test)]
    pub fn error(&self, key: SettingKey) -> Option<&str> {
        self.errors[index_of(key)].as_deref()
    }

    // ----- Applying values ------------------------------------------------

    /// Apply a field's text to its setting. Text that isn't a valid
    /// value is refused and the reason kept for the note under the field.
    fn commit(&mut self, index: usize, settings: &mut Settings) -> SettingsOutcome {
        let key = SettingKey::ALL[index];
        match settings.set_text(key, self.fields.text(index)) {
            Ok(changed) => {
                // The text as the setting has it: trimmed, and so on.
                self.fields.set_text(index, &settings.text(key));
                self.errors[index] = None;
                if changed {
                    SettingsOutcome::Changed
                } else {
                    SettingsOutcome::Continue
                }
            }
            Err(reason) => {
                self.errors[index] = Some(reason);
                SettingsOutcome::Continue
            }
        }
    }

    /// Apply every field's text, as Ctrl+S and leaving the page do.
    pub fn commit_all(&mut self, settings: &mut Settings) -> SettingsOutcome {
        let mut outcome = SettingsOutcome::Continue;
        for index in 0..self.fields.len() {
            if self.commit(index, settings) == SettingsOutcome::Changed {
                outcome = SettingsOutcome::Changed;
            }
        }
        outcome
    }

    /// Ctrl+R: put the focused setting back to its default.
    fn reset_focused(&mut self, settings: &mut Settings) -> SettingsOutcome {
        let index = self.fields.focused();
        let key = SettingKey::ALL[index];
        let changed = settings.reset(key);
        self.fields.set_text(index, &settings.text(key));
        self.errors[index] = None;
        if changed {
            SettingsOutcome::Changed
        } else {
            SettingsOutcome::Continue
        }
    }

    /// Escape: put the focused field's text back to the setting's value.
    fn revert_focused(&mut self, settings: &Settings) {
        let index = self.fields.focused();
        self.fields
            .set_text(index, &settings.text(SettingKey::ALL[index]));
        self.errors[index] = None;
    }

    // ----- Input ----------------------------------------------------------

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        clipboard: &mut Clipboard,
        settings: &mut Settings,
    ) -> SettingsOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => self.commit(self.fields.focused(), settings),
            KeyCode::Esc => {
                self.revert_focused(settings);
                SettingsOutcome::Continue
            }
            KeyCode::Char('r') if ctrl => self.reset_focused(settings),
            KeyCode::Char('s') if ctrl => self.commit_all(settings),
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(self.page());
                SettingsOutcome::Continue
            }
            KeyCode::PageDown => {
                self.scroll += self.page();
                SettingsOutcome::Continue
            }
            _ => match self.fields.handle_key(key, clipboard) {
                FieldKey::Moved { from, .. } => {
                    self.reveal = true;
                    self.commit(from, settings)
                }
                FieldKey::Changed => {
                    self.errors[self.fields.focused()] = None;
                    SettingsOutcome::Continue
                }
                FieldKey::Unchanged | FieldKey::Ignored => SettingsOutcome::Continue,
            },
        }
    }

    /// Add pasted text to the focused field.
    pub fn paste(&mut self, text: &str) {
        if self.fields.paste(text) {
            self.errors[self.fields.focused()] = None;
        }
    }

    /// Whether the screen position is over the page.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.area.contains(ScreenPosition::new(x, y))
    }

    /// Whether a drag that started in a field is going on, in which case
    /// the page wants drag and release events wherever they happen.
    pub fn is_dragging(&self) -> bool {
        self.fields.is_dragging()
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent, settings: &mut Settings) -> SettingsOutcome {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll = self.scroll.saturating_sub(WHEEL_LINES);
                SettingsOutcome::Continue
            }
            MouseEventKind::ScrollDown => {
                self.scroll += WHEEL_LINES;
                SettingsOutcome::Continue
            }
            _ => match self.fields.handle_mouse(mouse) {
                Some(from) => self.commit(from, settings),
                None => SettingsOutcome::Continue,
            },
        }
    }

    /// Rows scrolled by Page Up and Page Down.
    fn page(&self) -> usize {
        (self.area.height as usize).saturating_sub(1).max(1)
    }

    // ----- Rendering ------------------------------------------------------

    /// Draw the page into `area`. Returns where the terminal cursor
    /// belongs, if the focused field is on screen.
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        settings: &Settings,
    ) -> Option<ScreenPosition> {
        self.area = area;
        let background = palette_background(theme);
        buf.set_style(area, background);
        let height = area.height as usize;
        if height == 0 || area.width < INDENT + 4 {
            return None;
        }

        // Scroll no further than needed to show the last row, then as
        // needed to show the focused field with its name and note.
        self.scroll = self.scroll.min(self.lines.len().saturating_sub(height));
        if self.reveal {
            self.reveal = false;
            let focused = self.fields.focused();
            let first = self
                .lines
                .iter()
                .position(|line| *line == Line::Name(focused));
            let last = self
                .lines
                .iter()
                .position(|line| *line == Line::Note(focused));
            if let (Some(first), Some(last)) = (first, last) {
                if first < self.scroll {
                    self.scroll = first;
                } else if last >= self.scroll + height {
                    self.scroll = last + 1 - height;
                }
            }
        }

        let header = background
            .fg(theme.active_tab_text)
            .add_modifier(Modifier::BOLD);
        let rule = background.fg(theme.gutter_guide);
        let name = background.add_modifier(Modifier::BOLD);
        let tag = background.fg(theme.status_bar_project_text);
        let note = background.fg(theme.command_palette_result_context_text);
        // The page has no color of its own for a refused value; the
        // terminal palette's red says it as well as anything.
        let error = background.fg(theme.terminal_red);
        let width = area.width as usize;
        let field_width = area.width.saturating_sub(INDENT * 2).min(MAX_FIELD_WIDTH);

        self.fields.clear_layout();
        let mut cursor = None;
        for (row, line) in self.lines.iter().skip(self.scroll).take(height).enumerate() {
            let y = area.y + row as u16;
            match *line {
                Line::Blank => {}
                Line::Header(category) => {
                    let title = format!(" {} ", category.name());
                    let title_width = Span::raw(&title).width();
                    let x = area.x + HEADER_INDENT;
                    buf.set_stringn(x, y, &title, width, header);
                    let rule_from = (HEADER_INDENT as usize + title_width) as u16;
                    for x in area.x + rule_from..area.right().saturating_sub(HEADER_INDENT) {
                        buf[(x, y)].set_symbol("─").set_style(rule);
                    }
                }
                Line::Name(index) => {
                    let key = SettingKey::ALL[index];
                    let x = area.x + INDENT;
                    let name_width = Span::raw(key.name()).width();
                    buf.set_stringn(x, y, key.name(), width, name);
                    if !settings.is_default(key) {
                        let x = x + name_width as u16 + 2;
                        if x < area.right() {
                            buf.set_stringn(x, y, MODIFIED_TAG, (area.right() - x) as usize, tag);
                        }
                    }
                }
                Line::Field(index) => {
                    let key = SettingKey::ALL[index];
                    let row = Rect::new(area.x + INDENT, y, field_width, 1);
                    let placeholder = key.placeholder();
                    if let Some(at) = self
                        .fields
                        .render_field(index, row, &placeholder, buf, theme)
                    {
                        cursor = Some(at);
                    }
                }
                Line::Note(index) => {
                    let key = SettingKey::ALL[index];
                    let x = area.x + INDENT;
                    let (text, style) = match &self.errors[index] {
                        Some(reason) => (reason.clone(), error),
                        None if settings.is_default(key) => (key.description().to_owned(), note),
                        None => {
                            let default = match key.default_text() {
                                text if text.is_empty() => "blank".to_owned(),
                                text => text,
                            };
                            (format!("{} (default: {default})", key.description()), note)
                        }
                    };
                    buf.set_stringn(x, y, &text, width.saturating_sub(INDENT as usize), style);
                }
            }
        }
        cursor
    }
}

/// A setting's index in [`SettingKey::ALL`], which is its field's.
#[cfg(test)]
fn index_of(key: SettingKey) -> usize {
    SettingKey::ALL
        .iter()
        .position(|k| *k == key)
        .expect("every setting is listed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState, MouseButton};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn press(view: &mut SettingsView, settings: &mut Settings, code: KeyCode) -> SettingsOutcome {
        view.handle_key(
            key(code, KeyModifiers::NONE),
            &mut Clipboard::local_only(),
            settings,
        )
    }

    fn ctrl(view: &mut SettingsView, settings: &mut Settings, c: char) -> SettingsOutcome {
        view.handle_key(
            key(KeyCode::Char(c), KeyModifiers::CONTROL),
            &mut Clipboard::local_only(),
            settings,
        )
    }

    fn type_str(view: &mut SettingsView, settings: &mut Settings, text: &str) {
        for c in text.chars() {
            press(view, settings, KeyCode::Char(c));
        }
    }

    fn draw(view: &mut SettingsView, settings: &Settings, width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf, &Theme::default(), settings);
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect()
    }

    fn row_with<'a>(screen: &'a [String], text: &str) -> &'a str {
        screen
            .iter()
            .find(|row| row.contains(text))
            .unwrap_or_else(|| panic!("no row with {text:?} in {screen:#?}"))
    }

    #[test]
    fn lists_settings_by_category_with_placeholders_and_values() {
        let settings = Settings::default();
        let mut view = SettingsView::new(&settings);
        let screen = draw(&mut view, &settings, 80, 24);
        let terminal = screen
            .iter()
            .position(|r| r.contains(" Terminal "))
            .unwrap();
        let search = screen.iter().position(|r| r.contains(" Search ")).unwrap();
        let shell = screen
            .iter()
            .position(|r| r.contains("Shell executable"))
            .unwrap();
        let scrollback = screen
            .iter()
            .position(|r| r.contains("Scrollback lines"))
            .unwrap();
        let results = screen
            .iter()
            .position(|r| r.contains("Maximum search results"))
            .unwrap();
        assert!(terminal < shell && shell < scrollback && scrollback < search && search < results);
        assert!(screen[terminal].contains('─'), "{screen:#?}");
        // The shell field is blank, so its placeholder names the detected
        // shell; the numbers show their defaults.
        assert!(
            screen[shell + 1].contains("> autodetected: "),
            "{screen:#?}"
        );
        assert!(screen[shell + 2].contains("login shell"), "{screen:#?}");
        assert!(
            screen[scrollback + 1].contains(&format!("> {}", settings.terminal_scrollback())),
            "{screen:#?}"
        );
        assert!(
            !screen.iter().any(|r| r.contains(MODIFIED_TAG)),
            "{screen:#?}"
        );
    }

    #[test]
    fn enter_applies_a_value_and_moving_on_applies_the_field_left() {
        let mut settings = Settings::default();
        let mut view = SettingsView::new(&settings);
        // Down to the scrollback field; the shell field, left blank, is
        // applied without changing anything.
        assert_eq!(
            press(&mut view, &mut settings, KeyCode::Down),
            SettingsOutcome::Continue
        );
        assert_eq!(view.focused_key(), SettingKey::TerminalScrollback);
        ctrl(&mut view, &mut settings, 'a');
        type_str(&mut view, &mut settings, " 250 ");
        assert_eq!(
            settings.terminal_scrollback(),
            100_000,
            "not applied until Enter"
        );
        assert_eq!(
            press(&mut view, &mut settings, KeyCode::Enter),
            SettingsOutcome::Changed
        );
        assert_eq!(settings.terminal_scrollback(), 250);
        assert_eq!(view.text(SettingKey::TerminalScrollback), "250", "trimmed");
        assert_eq!(
            press(&mut view, &mut settings, KeyCode::Enter),
            SettingsOutcome::Continue
        );

        let screen = draw(&mut view, &settings, 80, 24);
        let row = row_with(&screen, "Scrollback lines");
        assert!(row.contains(MODIFIED_TAG), "{screen:#?}");
        assert!(
            row_with(&screen, "scroll back through").contains("(default: 100000)"),
            "{screen:#?}"
        );

        // Typing into the results field and leaving it with Tab applies
        // it; Shift+Tab back applies the (unchanged) results field.
        press(&mut view, &mut settings, KeyCode::Tab);
        ctrl(&mut view, &mut settings, 'a');
        type_str(&mut view, &mut settings, "42");
        assert_eq!(
            press(&mut view, &mut settings, KeyCode::Tab),
            SettingsOutcome::Changed
        );
        assert_eq!(settings.search_max_results(), 42);
        assert_eq!(view.focused_key(), SettingKey::Shell, "Tab wraps");
        assert_eq!(
            press(&mut view, &mut settings, KeyCode::BackTab),
            SettingsOutcome::Continue
        );
        assert_eq!(view.focused_key(), SettingKey::SearchMaxResults);
    }

    #[test]
    fn bad_text_is_refused_with_a_note_and_escape_reverts() {
        let mut settings = Settings::default();
        let mut view = SettingsView::new(&settings);
        press(&mut view, &mut settings, KeyCode::Down);
        ctrl(&mut view, &mut settings, 'a');
        type_str(&mut view, &mut settings, "many");
        assert_eq!(
            press(&mut view, &mut settings, KeyCode::Enter),
            SettingsOutcome::Continue
        );
        assert_eq!(settings.terminal_scrollback(), 100_000);
        assert_eq!(
            view.error(SettingKey::TerminalScrollback),
            Some("must be a whole number")
        );
        assert_eq!(
            view.text(SettingKey::TerminalScrollback),
            "many",
            "the text stays"
        );
        let screen = draw(&mut view, &settings, 80, 24);
        assert!(
            screen.iter().any(|r| r.contains("must be a whole number")),
            "{screen:#?}"
        );
        // Typing clears the note; Escape puts the value back.
        press(&mut view, &mut settings, KeyCode::Backspace);
        assert_eq!(view.error(SettingKey::TerminalScrollback), None);
        press(&mut view, &mut settings, KeyCode::Esc);
        assert_eq!(view.text(SettingKey::TerminalScrollback), "100000");
        // Leaving with bad text applies nothing for that field but
        // still applies the others.
        ctrl(&mut view, &mut settings, 'a');
        type_str(&mut view, &mut settings, "x");
        press(&mut view, &mut settings, KeyCode::Up);
        type_str(&mut view, &mut settings, "/bin/dash");
        assert_eq!(view.commit_all(&mut settings), SettingsOutcome::Changed);
        assert_eq!(settings.shell(), Some("/bin/dash"));
        assert_eq!(settings.terminal_scrollback(), 100_000);
        assert!(view.error(SettingKey::TerminalScrollback).is_some());
    }

    #[test]
    fn ctrl_r_resets_the_focused_setting() {
        let mut settings = Settings::default();
        settings.set_text(SettingKey::Shell, "fish").unwrap();
        settings
            .set_text(SettingKey::SearchMaxResults, "5")
            .unwrap();
        let mut view = SettingsView::new(&settings);
        assert_eq!(view.text(SettingKey::Shell), "fish");
        assert_eq!(
            ctrl(&mut view, &mut settings, 'r'),
            SettingsOutcome::Changed
        );
        assert_eq!(settings.shell(), None);
        assert_eq!(view.text(SettingKey::Shell), "");
        assert_eq!(
            ctrl(&mut view, &mut settings, 'r'),
            SettingsOutcome::Continue
        );
        assert_eq!(settings.search_max_results(), 5, "only the focused one");
        // Ctrl+R with bad text typed drops the text along with the note.
        press(&mut view, &mut settings, KeyCode::Tab);
        press(&mut view, &mut settings, KeyCode::Tab);
        type_str(&mut view, &mut settings, "x");
        press(&mut view, &mut settings, KeyCode::Enter);
        assert!(view.error(SettingKey::SearchMaxResults).is_some());
        assert_eq!(
            ctrl(&mut view, &mut settings, 'r'),
            SettingsOutcome::Changed
        );
        assert!(settings.is_default(SettingKey::SearchMaxResults));
        assert_eq!(view.error(SettingKey::SearchMaxResults), None);
        assert_eq!(view.text(SettingKey::SearchMaxResults), "10000");
    }

    #[test]
    fn clicking_a_field_focuses_it_and_applies_the_one_left() {
        let mut settings = Settings::default();
        let mut view = SettingsView::new(&settings);
        type_str(&mut view, &mut settings, "/bin/sh");
        let screen = draw(&mut view, &settings, 80, 24);
        let row = screen
            .iter()
            .position(|r| r.contains("Maximum search results"))
            .unwrap() as u16
            + 1;
        assert!(screen[row as usize].contains("> 10000"), "{screen:#?}");
        let outcome = view.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 8,
                row,
                modifiers: KeyModifiers::NONE,
            },
            &mut settings,
        );
        assert_eq!(outcome, SettingsOutcome::Changed);
        assert_eq!(settings.shell(), Some("/bin/sh"));
        assert_eq!(view.focused_key(), SettingKey::SearchMaxResults);
        assert!(view.contains(8, row));
    }

    #[test]
    fn a_short_screen_scrolls_to_the_focused_field() {
        let mut settings = Settings::default();
        let mut view = SettingsView::new(&settings);
        let screen = draw(&mut view, &settings, 60, 6);
        assert!(
            screen.iter().any(|r| r.contains("Shell executable")),
            "{screen:#?}"
        );
        assert!(
            !screen.iter().any(|r| r.contains("Maximum search")),
            "{screen:#?}"
        );
        press(&mut view, &mut settings, KeyCode::Down);
        press(&mut view, &mut settings, KeyCode::Down);
        let screen = draw(&mut view, &settings, 60, 6);
        assert!(
            screen.iter().any(|r| r.contains("Maximum search")),
            "{screen:#?}"
        );
        assert!(
            screen.iter().any(|r| r.contains("this many matches")),
            "the note is shown with the field: {screen:#?}"
        );
        // The wheel scrolls back up without moving the focus.
        for _ in 0..5 {
            view.handle_mouse(
                MouseEvent {
                    kind: MouseEventKind::ScrollUp,
                    column: 0,
                    row: 0,
                    modifiers: KeyModifiers::NONE,
                },
                &mut settings,
            );
        }
        let screen = draw(&mut view, &settings, 60, 6);
        assert!(
            screen.iter().any(|r| r.contains("Shell executable")),
            "{screen:#?}"
        );
        assert_eq!(view.focused_key(), SettingKey::SearchMaxResults);
    }
}
