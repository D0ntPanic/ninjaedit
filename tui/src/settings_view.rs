//! The settings page: the first of the editor's modes, views that stand
//! in for the editor in the upper part of the screen rather than sitting
//! below it in the tool pane. It is opened from the modes palette
//! (Ctrl+E) like a tool, and left the same way, or by anything that
//! brings a file to the front: Ctrl+O, Ctrl+T, a project search match.
//!
//! The page lists every [`SettingKey`] under its category as a field
//! with the setting's name above it and a line about it below (wrapped
//! to the page's width). The field is a control for the setting's
//! [kind](SettingKind): a text field, or for a setting with a fixed set
//! of options, a choice showing them all.
//! The fields are a [`Fields`], so Tab and Up and Down move between them.
//! A choice is applied as soon as another option is chosen. Text is
//! applied when its field is left (Tab, an arrow, a click elsewhere,
//! leaving the page) or with Enter; Ctrl+S applies every field. Text that
//! isn't a valid value is refused: the setting keeps its value and the
//! reason shows under the field until the text changes. Escape puts the
//! field's text back to the setting's value. A setting changed from its default is
//! tagged, and Ctrl+D puts the focused one back to its default.
//!
//! The page only edits the [`Settings`]; the application saves them and
//! applies them to what is running whenever the page reports a change.

use crate::clipboard::Clipboard;
use crate::fields::{FieldKey, FieldKind, Fields, wrap_words};
use crate::palette::palette_background;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ninjaedit_core::{Category, SettingKey, SettingKind, Settings};
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

/// One entry of the page, as laid out before the width is known; a
/// note wraps into as many rows as it needs.
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

/// One screen row of the page, after wrapping to a width.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    Blank,
    Header(Category),
    Name(usize),
    Field(usize),
    /// One line of a setting's note: the setting, whether it is the
    /// reason its value was refused, and the line.
    Note(usize, bool, String),
}

pub struct SettingsView {
    fields: Fields,
    /// For each field, why its text was refused, shown until the text
    /// changes.
    errors: Vec<Option<String>>,
    /// The entries of the page, in order.
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
            fields: Fields::with_kinds(
                SettingKey::ALL
                    .into_iter()
                    .map(|key| match key.kind() {
                        SettingKind::Text => FieldKind::Text,
                        SettingKind::Choice(choices) => {
                            FieldKind::Choice(choices.iter().map(|c| c.label.to_owned()).collect())
                        }
                    })
                    .collect(),
            ),
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
        for index in 0..SettingKey::ALL.len() {
            self.show(index, settings);
        }
    }

    /// Show a setting's current value in its field, dropping any text
    /// typed into it and the note about refused text.
    fn show(&mut self, index: usize, settings: &Settings) {
        let key = SettingKey::ALL[index];
        let value = settings.text(key);
        match key.kind() {
            SettingKind::Text => self.fields.set_text(index, &value),
            SettingKind::Choice(choices) => {
                if let Some(option) = choices.iter().position(|c| c.value == value) {
                    self.fields.choose(index, option);
                }
            }
        }
        self.errors[index] = None;
    }

    /// A field's value in the setting's text form: the text typed, or
    /// the chosen option's value.
    fn value(&self, index: usize) -> String {
        match SettingKey::ALL[index].kind() {
            SettingKind::Text => self.fields.text(index).to_owned(),
            SettingKind::Choice(choices) => self
                .fields
                .chosen(index)
                .map(|option| choices[option].value.to_owned())
                .unwrap_or_default(),
        }
    }

    /// Whether a field is a choice, which applies as soon as it changes.
    fn is_choice(index: usize) -> bool {
        matches!(SettingKey::ALL[index].kind(), SettingKind::Choice(_))
    }

    /// The setting whose field has the focus.
    #[cfg(test)]
    pub fn focused_key(&self) -> SettingKey {
        SettingKey::ALL[self.fields.focused()]
    }

    /// The value in a setting's field, in the setting's text form.
    #[cfg(test)]
    pub fn text(&self, key: SettingKey) -> String {
        self.value(index_of(key))
    }

    /// Why a setting's text was refused, if it was.
    #[cfg(test)]
    pub fn error(&self, key: SettingKey) -> Option<&str> {
        self.errors[index_of(key)].as_deref()
    }

    // ----- Applying values ------------------------------------------------

    /// Apply a field's value to its setting. Text that isn't a valid
    /// value is refused and the reason kept for the note under the field.
    fn commit(&mut self, index: usize, settings: &mut Settings) -> SettingsOutcome {
        let key = SettingKey::ALL[index];
        match settings.set_text(key, &self.value(index)) {
            Ok(changed) => {
                // The value as the setting has it: trimmed, and so on.
                self.show(index, settings);
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

    /// Ctrl+D: put the focused setting back to its default.
    pub fn reset_focused(&mut self, settings: &mut Settings) -> SettingsOutcome {
        let index = self.fields.focused();
        let changed = settings.reset(SettingKey::ALL[index]);
        self.show(index, settings);
        if changed {
            SettingsOutcome::Changed
        } else {
            SettingsOutcome::Continue
        }
    }

    /// Escape: put the focused field's text back to the setting's value.
    fn revert_focused(&mut self, settings: &Settings) {
        self.show(self.fields.focused(), settings);
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
            KeyCode::Char('d') if ctrl => self.reset_focused(settings),
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
                FieldKey::Changed if Self::is_choice(self.fields.focused()) => {
                    self.commit(self.fields.focused(), settings)
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
            _ => {
                let mut outcome = match self.fields.handle_mouse(mouse) {
                    Some(from) => self.commit(from, settings),
                    None => SettingsOutcome::Continue,
                };
                // A click on a choice's option chooses it.
                let focused = self.fields.focused();
                if Self::is_choice(focused)
                    && self.commit(focused, settings) == SettingsOutcome::Changed
                {
                    outcome = SettingsOutcome::Changed;
                }
                outcome
            }
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

        let width = area.width as usize;
        let field_width = area.width.saturating_sub(INDENT * 2).min(MAX_FIELD_WIDTH);
        let text_width = width.saturating_sub(INDENT as usize);

        // Lay the entries out as rows for this width: a note wraps, and
        // is in the error color when its value was refused.
        let rows: Vec<Row> = self
            .lines
            .iter()
            .flat_map(|line| match *line {
                Line::Blank => vec![Row::Blank],
                Line::Header(category) => vec![Row::Header(category)],
                Line::Name(index) => vec![Row::Name(index)],
                Line::Field(index) => vec![Row::Field(index)],
                Line::Note(index) => {
                    let key = SettingKey::ALL[index];
                    let (text, is_error) = match &self.errors[index] {
                        Some(reason) => (reason.clone(), true),
                        None if settings.is_default(key) => (key.description().to_owned(), false),
                        None => {
                            let default = match (key.kind(), key.default_text()) {
                                (SettingKind::Choice(choices), text) => choices
                                    .iter()
                                    .find(|c| c.value == text)
                                    .map_or(text, |c| c.label.to_owned()),
                                (_, text) if text.is_empty() => "blank".to_owned(),
                                (_, text) => text,
                            };
                            (format!("{} (default: {default})", key.description()), false)
                        }
                    };
                    wrap_words(&text, text_width)
                        .into_iter()
                        .map(|text| Row::Note(index, is_error, text))
                        .collect()
                }
            })
            .collect();

        // Scroll no further than needed to show the last row, then as
        // needed to show the focused field with its name and note.
        self.scroll = self.scroll.min(rows.len().saturating_sub(height));
        if self.reveal {
            self.reveal = false;
            let focused = self.fields.focused();
            let first = rows.iter().position(|row| *row == Row::Name(focused));
            let last = rows
                .iter()
                .rposition(|row| matches!(row, Row::Note(index, ..) if *index == focused));
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

        self.fields.clear_layout();
        let mut cursor = None;
        for (row, entry) in rows.into_iter().skip(self.scroll).take(height).enumerate() {
            let y = area.y + row as u16;
            match entry {
                Row::Blank => {}
                Row::Header(category) => {
                    let title = format!(" {} ", category.name());
                    let title_width = Span::raw(&title).width();
                    let x = area.x + HEADER_INDENT;
                    buf.set_stringn(x, y, &title, width, header);
                    let rule_from = (HEADER_INDENT as usize + title_width) as u16;
                    for x in area.x + rule_from..area.right().saturating_sub(HEADER_INDENT) {
                        buf[(x, y)].set_symbol("─").set_style(rule);
                    }
                }
                Row::Name(index) => {
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
                Row::Field(index) => {
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
                Row::Note(_, is_error, text) => {
                    let style = if is_error { error } else { note };
                    buf.set_stringn(area.x + INDENT, y, &text, text_width, style);
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
    use ninjaedit_core::ContinuationIndent;

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
        // Down past the continuation indent and shell fields to the
        // scrollback field; the fields left as they were are applied
        // without changing anything.
        assert_eq!(
            press(&mut view, &mut settings, KeyCode::Down),
            SettingsOutcome::Continue
        );
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
        assert_eq!(view.focused_key(), SettingKey::CMakeGenerator);
        assert_eq!(
            press(&mut view, &mut settings, KeyCode::Tab),
            SettingsOutcome::Continue
        );
        assert_eq!(view.focused_key(), SettingKey::RustCompletionModel);
        for _ in 0..2 {
            press(&mut view, &mut settings, KeyCode::Tab);
        }
        assert_eq!(view.focused_key(), SettingKey::CompletionTokenConfidence);
        assert_eq!(
            press(&mut view, &mut settings, KeyCode::Tab),
            SettingsOutcome::Continue
        );
        assert_eq!(
            view.focused_key(),
            SettingKey::ContinuationIndent,
            "Tab wraps"
        );
        assert_eq!(
            press(&mut view, &mut settings, KeyCode::BackTab),
            SettingsOutcome::Continue
        );
        assert_eq!(view.focused_key(), SettingKey::CompletionTokenConfidence);
    }

    #[test]
    fn a_choice_shows_its_options_and_applies_as_soon_as_it_changes() {
        let mut settings = Settings::default();
        let mut view = SettingsView::new(&settings);
        assert_eq!(view.focused_key(), SettingKey::ContinuationIndent);
        let screen = draw(&mut view, &settings, 80, 24);
        let row = row_with(&screen, "Align with the bracket");
        assert!(row.contains("○ Align with the bracket"), "{screen:#?}");
        assert!(row.contains("● Indent one level"), "{screen:#?}");

        assert_eq!(
            press(&mut view, &mut settings, KeyCode::Left),
            SettingsOutcome::Changed
        );
        assert_eq!(settings.continuation_indent(), ContinuationIndent::Align);
        let screen = draw(&mut view, &settings, 80, 24);
        assert!(
            row_with(&screen, "Continuation indent").contains(MODIFIED_TAG),
            "{screen:#?}"
        );
        assert!(
            row_with(&screen, "(default:").contains("(default: Indent one level)"),
            "the default by its label: {screen:#?}"
        );
        assert_eq!(
            press(&mut view, &mut settings, KeyCode::Left),
            SettingsOutcome::Continue,
            "already the last"
        );
        assert_eq!(
            press(&mut view, &mut settings, KeyCode::Down),
            SettingsOutcome::Continue,
            "leaving it applies nothing more"
        );

        // A click on an option chooses it, focusing the field.
        let screen = draw(&mut view, &settings, 80, 24);
        let y = screen
            .iter()
            .position(|r| r.contains("Indent one level"))
            .unwrap();
        let x = screen[y].find("Indent").unwrap() as u16;
        let outcome = view.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x,
                row: y as u16,
                modifiers: KeyModifiers::NONE,
            },
            &mut settings,
        );
        assert_eq!(outcome, SettingsOutcome::Changed);
        assert_eq!(view.focused_key(), SettingKey::ContinuationIndent);
        assert_eq!(settings.continuation_indent(), ContinuationIndent::Indent);

        // Ctrl+D and a refresh show the setting's option.
        press(&mut view, &mut settings, KeyCode::Char(' '));
        assert_eq!(settings.continuation_indent(), ContinuationIndent::Align);
        assert_eq!(
            ctrl(&mut view, &mut settings, 'd'),
            SettingsOutcome::Changed
        );
        assert_eq!(view.text(SettingKey::ContinuationIndent), "indent");
        settings
            .set_text(SettingKey::ContinuationIndent, "align")
            .unwrap();
        view.refresh(&settings);
        assert_eq!(view.text(SettingKey::ContinuationIndent), "align");
    }

    #[test]
    fn bad_text_is_refused_with_a_note_and_escape_reverts() {
        let mut settings = Settings::default();
        let mut view = SettingsView::new(&settings);
        press(&mut view, &mut settings, KeyCode::Down);
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
    fn ctrl_d_resets_the_focused_setting() {
        let mut settings = Settings::default();
        settings.set_text(SettingKey::Shell, "fish").unwrap();
        settings
            .set_text(SettingKey::SearchMaxResults, "5")
            .unwrap();
        let mut view = SettingsView::new(&settings);
        assert_eq!(view.text(SettingKey::Shell), "fish");
        press(&mut view, &mut settings, KeyCode::Down);
        assert_eq!(
            ctrl(&mut view, &mut settings, 'd'),
            SettingsOutcome::Changed
        );
        assert_eq!(settings.shell(), None);
        assert_eq!(view.text(SettingKey::Shell), "");
        assert_eq!(
            ctrl(&mut view, &mut settings, 'd'),
            SettingsOutcome::Continue
        );
        assert_eq!(settings.search_max_results(), 5, "only the focused one");
        // Ctrl+D with bad text typed drops the text along with the note.
        press(&mut view, &mut settings, KeyCode::Tab);
        press(&mut view, &mut settings, KeyCode::Tab);
        type_str(&mut view, &mut settings, "x");
        press(&mut view, &mut settings, KeyCode::Enter);
        assert!(view.error(SettingKey::SearchMaxResults).is_some());
        assert_eq!(
            ctrl(&mut view, &mut settings, 'd'),
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
        press(&mut view, &mut settings, KeyCode::Down);
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
    fn a_narrow_page_wraps_a_note_and_reveals_all_of_it() {
        let mut settings = Settings::default();
        let mut view = SettingsView::new(&settings);
        // Wide enough for the note in one row, then not.
        let screen = draw(&mut view, &settings, 80, 40);
        let name = screen
            .iter()
            .position(|r| r.contains("Maximum search results"))
            .unwrap();
        assert!(
            screen[name + 2].contains("A project search stops after finding this many matches"),
            "{screen:#?}"
        );
        assert!(screen[name + 3].trim().is_empty(), "{screen:#?}");

        let screen = draw(&mut view, &settings, 40, 40);
        let name = screen
            .iter()
            .position(|r| r.contains("Maximum search results"))
            .unwrap();
        let note = format!("{} {}", screen[name + 2].trim(), screen[name + 3].trim());
        assert_eq!(
            note, "A project search stops after finding this many matches",
            "two rows, broken at a space: {screen:#?}"
        );
        assert!(screen[name + 4].trim().is_empty(), "{screen:#?}");
        assert!(
            screen[name + 3].starts_with("   ") && !screen[name + 3].starts_with("    "),
            "continuation rows keep the indent: {screen:#?}"
        );

        // Moving to the field on a short screen scrolls so the whole
        // note shows, not just its first row.
        for _ in 0..3 {
            press(&mut view, &mut settings, KeyCode::Down);
        }
        let screen = draw(&mut view, &settings, 40, 5);
        assert!(
            screen.iter().any(|r| r.contains("this many matches")),
            "the note's last row: {screen:#?}"
        );
        assert!(
            screen.iter().any(|r| r.contains("Maximum search results")),
            "{screen:#?}"
        );

        // A refused value's reason wraps the same way.
        ctrl(&mut view, &mut settings, 'a');
        type_str(&mut view, &mut settings, "many");
        press(&mut view, &mut settings, KeyCode::Enter);
        let screen = draw(&mut view, &settings, 40, 5);
        assert!(
            screen.iter().any(|r| r.contains("must be a whole number")),
            "{screen:#?}"
        );
    }

    #[test]
    fn a_short_screen_scrolls_to_the_focused_field() {
        let mut settings = Settings::default();
        let mut view = SettingsView::new(&settings);
        let screen = draw(&mut view, &settings, 60, 6);
        assert!(
            screen.iter().any(|r| r.contains("Continuation indent")),
            "{screen:#?}"
        );
        assert!(
            !screen.iter().any(|r| r.contains("Maximum search")),
            "{screen:#?}"
        );
        for _ in 0..3 {
            press(&mut view, &mut settings, KeyCode::Down);
        }
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
        for _ in 0..10 {
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
            screen.iter().any(|r| r.contains("Continuation indent")),
            "{screen:#?}"
        );
        assert_eq!(view.focused_key(), SettingKey::SearchMaxResults);
    }
}
