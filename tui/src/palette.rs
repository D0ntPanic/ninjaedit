//! The command palette: a search box at the top center of the screen with a
//! ranked list of results under it.
//!
//! The palette is generic over what it searches. The application builds a
//! list of [`PaletteItem`]s (open tabs, project files, ...) and the palette
//! ranks them against whatever is typed with the fuzzy matcher from the
//! core crate. Enter activates the selected result, which starts out as the
//! best match; the arrow keys and the mouse choose another. The query is
//! an [`Input`], so it edits like a text field: Home and End and the
//! arrow keys work on it, and Ctrl+Home and Ctrl+End pick the first and
//! last result instead.

use crate::clipboard::Clipboard;
use crate::input::{Input, InputKey};
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ninjaedit_core::fuzzy;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Clear, Widget};
use std::path::PathBuf;

/// The most results the palette keeps after ranking.
const MAX_RESULTS: usize = 200;
/// The most result rows shown at once.
const MAX_VISIBLE: usize = 12;
/// The widest the palette (and the search box, which shares its look)
/// gets.
pub const MAX_WIDTH: u16 = 80;

/// The palette's text over its background.
pub fn palette_background(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.command_palette_result_text)
        .bg(theme.command_palette_background)
}

/// Draw the palette's rounded frame over `area`, clearing what's under
/// it, with `hint` in the bottom border. Returns the area inside the
/// frame.
pub fn render_frame(area: Rect, hint: Option<&str>, buf: &mut Buffer, theme: &Theme) -> Rect {
    Clear.render(area, buf);
    let background = palette_background(theme);
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .style(background)
        .border_style(background.fg(theme.command_palette_box_color));
    if let Some(hint) = hint {
        block = block.title_bottom(Span::styled(
            format!(" {hint} "),
            background.fg(theme.command_palette_placeholder_text),
        ));
    }
    let inner = block.inner(area);
    block.render(area, buf);
    inner
}

/// What activating a result does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    SwitchTab(usize),
    OpenFile(PathBuf),
}

/// One searchable entry.
pub struct PaletteItem {
    /// The main text of the result (a file name). Matches here rank above
    /// matches only found in `search`.
    pub label: String,
    /// Dimmed text after the label (a directory).
    pub detail: String,
    /// The full text the query is matched against when the label doesn't
    /// match (a path).
    pub search: String,
    pub action: PaletteAction,
}

/// What the application should do after the palette handled an event.
pub enum PaletteOutcome {
    /// Keep the palette open.
    Continue,
    Close,
    Activate(PaletteAction),
}

pub struct Palette {
    /// Dimmed text shown in the search box before anything is typed.
    placeholder: &'static str,
    /// A note shown in the bottom border, such as indexing progress.
    hint: Option<String>,
    query: Input,
    items: Vec<PaletteItem>,
    /// Indices into `items`, best match first.
    results: Vec<usize>,
    /// Index into `results` of the highlighted row.
    selected: usize,
    /// Index into `results` of the first visible row.
    first_visible: usize,
    /// The whole palette, including its border, from the last render.
    area: Rect,
    /// The result rows from the last render.
    rows: Rect,
}

impl Palette {
    pub fn new(placeholder: &'static str, items: Vec<PaletteItem>) -> Palette {
        let mut palette = Palette {
            placeholder,
            hint: None,
            query: Input::new(),
            items,
            results: Vec::new(),
            selected: 0,
            first_visible: 0,
            area: Rect::default(),
            rows: Rect::default(),
        };
        palette.search();
        palette
    }

    /// Replace the searchable items, keeping the query.
    pub fn set_items(&mut self, items: Vec<PaletteItem>) {
        self.items = items;
        self.search();
    }

    pub fn set_hint(&mut self, hint: Option<String>) {
        self.hint = hint;
    }

    fn search(&mut self) {
        let candidates = self.items.iter().map(|item| (&item.label, &item.search));
        self.results = fuzzy::rank_labeled(candidates, self.query.text())
            .into_iter()
            .take(MAX_RESULTS)
            .map(|ranked| ranked.index)
            .collect();
        self.selected = 0;
        self.first_visible = 0;
    }

    /// Highlight a result row, clamped to the results, scrolling the list
    /// as needed.
    pub fn select(&mut self, index: usize) {
        if self.results.is_empty() {
            self.selected = 0;
            return;
        }
        self.selected = index.min(self.results.len() - 1);
        let visible = self.visible_rows();
        if self.selected < self.first_visible {
            self.first_visible = self.selected;
        } else if self.selected >= self.first_visible + visible {
            self.first_visible = self.selected + 1 - visible;
        }
    }

    fn visible_rows(&self) -> usize {
        self.results.len().clamp(1, MAX_VISIBLE)
    }

    fn activate(&self) -> PaletteOutcome {
        match self.results.get(self.selected) {
            Some(&index) => PaletteOutcome::Activate(self.items[index].action.clone()),
            None => PaletteOutcome::Continue,
        }
    }

    /// Handle a key press. `clipboard` is the application's clipboard, for
    /// copy, cut, and paste in the query.
    pub fn handle_key(&mut self, key: KeyEvent, clipboard: &mut Clipboard) -> PaletteOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return PaletteOutcome::Close,
            KeyCode::Enter => return self.activate(),
            KeyCode::Up => self.select(self.selected.saturating_sub(1)),
            KeyCode::Down => self.select(self.selected + 1),
            KeyCode::PageUp => self.select(self.selected.saturating_sub(self.visible_rows())),
            KeyCode::PageDown => self.select(self.selected + self.visible_rows()),
            KeyCode::Home if ctrl => self.select(0),
            KeyCode::End if ctrl => self.select(usize::MAX),
            _ => {
                if self.query.handle_key(key, clipboard) == InputKey::Changed {
                    self.search();
                }
            }
        }
        PaletteOutcome::Continue
    }

    /// Add pasted text to the query. Returns whether the query changed.
    pub fn paste(&mut self, text: &str) -> bool {
        let changed = self.query.paste(text);
        if changed {
            self.search();
        }
        changed
    }

    /// Whether the mouse position is over the palette.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.area.contains(ScreenPosition::new(x, y))
    }

    /// Whether a drag that started in the query is going on, in which
    /// case the palette wants drag and release events wherever they
    /// happen.
    pub fn is_dragging(&self) -> bool {
        self.query.is_dragging()
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> PaletteOutcome {
        let at = ScreenPosition::new(mouse.column, mouse.row);
        if self.query.is_dragging() || self.query.contains(mouse.column, mouse.row) {
            self.query.handle_mouse(mouse);
            return PaletteOutcome::Continue;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => self.select(self.selected.saturating_sub(1)),
            MouseEventKind::ScrollDown => self.select(self.selected + 1),
            MouseEventKind::Down(MouseButton::Left) if self.rows.contains(at) => {
                let row = self.first_visible + (mouse.row - self.rows.y) as usize;
                if row < self.results.len() {
                    self.select(row);
                    return self.activate();
                }
            }
            _ => {}
        }
        PaletteOutcome::Continue
    }

    /// Draw the palette over the top center of `screen`. Returns where the
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
        let visible = self.visible_rows();
        // Border, search box, results (or one row for "no matches").
        let height = (visible as u16 + 3).min(screen.height.saturating_sub(1));
        let x = screen.x + (screen.width - width) / 2;
        let y = screen.y + 1.min(screen.height.saturating_sub(height));
        self.area = Rect::new(x, y, width, height);

        let background = palette_background(theme);
        let inner = render_frame(self.area, self.hint.as_deref(), buf, theme);
        if inner.height == 0 {
            return None;
        }
        let cursor = self.query.render(
            Rect::new(inner.x, inner.y, inner.width, 1),
            self.placeholder,
            buf,
            theme,
        );

        // Results.
        self.rows = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
        if self.results.is_empty() {
            let message = if self.items.is_empty() {
                "Nothing to show"
            } else {
                "No matches"
            };
            buf.set_stringn(
                self.rows.x + 1,
                self.rows.y,
                message,
                self.rows.width.saturating_sub(1) as usize,
                background.fg(theme.command_palette_result_context_text),
            );
            return cursor;
        }
        let rows = self.rows.height as usize;
        if self.selected >= self.first_visible + rows {
            self.first_visible = self.selected + 1 - rows;
        }
        for (row, &index) in self
            .results
            .iter()
            .enumerate()
            .skip(self.first_visible)
            .take(rows)
        {
            let item = &self.items[index];
            let y = self.rows.y + (row - self.first_visible) as u16;
            let highlighted = row == self.selected;
            let (base, context) = if highlighted {
                let base = Style::default()
                    .fg(theme.command_palette_selection_text)
                    .bg(theme.command_palette_selection_background);
                buf.set_style(Rect::new(self.rows.x, y, self.rows.width, 1), base);
                (base, base)
            } else {
                (
                    background,
                    background.fg(theme.command_palette_result_context_text),
                )
            };
            let width = self.rows.width as usize;
            let label_width = Span::raw(&item.label).width().min(width.saturating_sub(1));
            buf.set_stringn(self.rows.x + 1, y, &item.label, label_width, base);
            let used = 1 + label_width + 2;
            if !item.detail.is_empty() && used < width {
                buf.set_stringn(
                    self.rows.x + used as u16,
                    y,
                    &item.detail,
                    width - used,
                    context,
                );
            }
        }
        cursor
    }
}
