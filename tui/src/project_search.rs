//! The project search dialog (Ctrl+Shift+F): a query over every file in
//! the project, with the matches listed under it and the surroundings of
//! the highlighted match under those.
//!
//! The dialog takes up most of the screen. Its top row is the query,
//! which drives a [`ProjectSearch`] from the core crate as it is typed;
//! the matches stream in from the background. Each row of the results
//! list shows the matched line (trimmed to keep the match itself in view
//! if the line is too long) and, on the right, the file and line number.
//! Both are syntax highlighted, as the editor would show the file, once
//! a background thread has lexed the file (see [`HighlightCache`]); a
//! file is lexed once however many matches fall in it, and until then
//! its lines show plain. The cache is dropped whenever the dialog is
//! hidden, since it is easily made again, unlike the results.
//!
//! Files open with unsaved changes are searched, and shown, as their
//! editors have them rather than as they are on disk: the application
//! hands their contents over each time it shows the dialog (see
//! [`set_buffers`](ProjectSearchDialog::set_buffers)), and the results
//! are made afresh when those have changed since.
//!
//! The pane below shows the highlighted match's file around the match,
//! laid out as the editor would show it: the match is brought into view
//! the way a jump to it in the editor is, and from there Ctrl+Up,
//! Ctrl+Down, Ctrl+PageUp, and Ctrl+PageDown scroll the pane, as does
//! the mouse wheel over it, as far as either end of the file; a sideways
//! wheel scrolls it horizontally, as far as the editor would allow.
//!
//! The arrow keys, PageUp and PageDown, Ctrl+Home and Ctrl+End, the wheel
//! over the list, and a click choose a match; Enter, or a double-click,
//! goes to it. The dialog keeps its query, matches, and highlighted row
//! when it closes, so that reopening it picks up where it left off: with
//! Enter closing the dialog, the list works as a worklist, one match per
//! Ctrl+Shift+F, Down, Enter. The query comes back selected, though, so
//! that typing starts a new search just as naturally.

use crate::clicks::ClickTracker;
use crate::clipboard::Clipboard;
use crate::input::{Input, InputKey};
use crate::palette::{palette_background, render_frame};
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ninjaedit_core::highlight_cache::FileStates;
use ninjaedit_core::project_search::{Buffers, SearchPhase};
use ninjaedit_core::search::literal_query;
use ninjaedit_core::text;
use ninjaedit_core::{
    BufferSnapshot, FileList, HighlightCache, ProjectMatch, ProjectSearch, Token, TokenKind,
};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const PLACEHOLDER: &str = "Search in project (start with / for a regular expression)";
const KEYS_HINT: &str = "Enter opens the match; Ctrl+Up and Ctrl+Down scroll the context";
/// How long a change to the query waits for the search to finish, so a
/// small project's matches show up in the same frame as the keystroke.
const SEARCH_GRACE: Duration = Duration::from_millis(15);
const TAB_WIDTH: usize = 4;
/// Lines the context pane scrolls per mouse wheel notch.
const WHEEL_LINES: usize = 3;
/// Columns the context pane scrolls per horizontal wheel notch.
const WHEEL_COLUMNS: usize = 4;
/// Columns the context pane may scroll past its longest visible line.
const HSCROLL_SLACK: usize = 2;
const ELLIPSIS: &str = "…";
const GUIDE: &str = "│";

/// What the application should do after the dialog handled an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectSearchOutcome {
    Continue,
    /// Escape: hide the dialog, keeping its state.
    Close,
    /// Enter or a double-click: hide the dialog and go to this match.
    Navigate(ProjectMatch),
}

pub struct ProjectSearchDialog {
    /// The project root, which paths are shown relative to.
    root: PathBuf,
    input: Input,
    search: ProjectSearch,
    /// The search's generation as of the last poll.
    generation: u64,
    /// Syntax highlighting of the files on show, lexed in the background.
    /// Made when the dialog is drawn and dropped when it is hidden.
    highlights: Option<HighlightCache>,
    /// The contents of files open with unsaved changes, as last handed
    /// over, which the search and the highlight cache use in place of
    /// the files on disk.
    buffers: Arc<Buffers>,
    /// The version of each of those buffers, sorted by path, to tell
    /// whether what is handed over next differs.
    pub(crate) buffer_versions: Vec<(PathBuf, u64)>,
    /// The cache's generation as of the last poll.
    highlight_generation: u64,
    /// Index of the highlighted match. Clamped to the matches at render,
    /// since they arrive over time.
    selected: usize,
    /// Index of the first match shown in the list.
    first_visible: usize,
    /// The lines of the highlighted match's file, for the context pane.
    context: Option<(Arc<Path>, Vec<String>)>,
    /// First visible line of the context pane.
    context_scroll: usize,
    /// First visible display column of the context pane.
    context_col: usize,
    /// Whether the next render should scroll the context pane to show
    /// the highlighted match, after it changed: centered vertically, and
    /// scrolled sideways as little as will show it.
    center_context: bool,
    /// The context pane's text columns from the last render, for
    /// limiting horizontal scrolling.
    context_text: Rect,
    clicks: ClickTracker,
    // Screen regions from the last render.
    area: Rect,
    rows: Rect,
    context_area: Rect,
}

impl ProjectSearchDialog {
    pub fn new(root: PathBuf, files: FileList) -> ProjectSearchDialog {
        let search = ProjectSearch::new(files);
        ProjectSearchDialog {
            root,
            input: Input::new(),
            generation: search.generation(),
            search,
            highlight_generation: 0,
            highlights: None,
            buffers: Arc::new(Buffers::new()),
            buffer_versions: Vec::new(),
            selected: 0,
            first_visible: 0,
            context: None,
            context_scroll: 0,
            context_col: 0,
            center_context: true,
            context_text: Rect::default(),
            clicks: ClickTracker::default(),
            area: Rect::default(),
            rows: Rect::default(),
            context_area: Rect::default(),
        }
    }

    pub fn query(&self) -> &str {
        self.input.text()
    }

    /// The highlighted match, if there is one.
    pub fn selected(&self) -> Option<ProjectMatch> {
        self.search.get(self.selected)
    }

    /// Whether the dialog is already showing a search for `text`: it is
    /// the query, or it is what the highlighted match found (as after
    /// going to a match, which leaves it selected in the editor).
    pub fn is_searching_for(&self, text: &str) -> bool {
        literal_query(text) == self.query()
            || self
                .selected()
                .is_some_and(|m| m.text.get(m.range.clone()) == Some(text))
    }

    /// Start over with a query, left selected so that typing replaces it.
    pub fn set_query_selected(&mut self, query: &str) {
        self.input.set_text_selected(query);
        self.query_changed();
    }

    /// Select the whole query, so that typing replaces it, without
    /// disturbing the search or its results.
    pub fn select_query(&mut self) {
        self.input.select_all();
    }

    /// Stop searches at `limit` matches, as the settings say. When that
    /// differs from the limit the results were found under, the search
    /// runs again, since they may be cut short (or short of the cut).
    pub fn set_limit(&mut self, limit: usize) {
        if self.search.limit() == limit {
            return;
        }
        self.search.set_limit(limit);
        if !self.query().is_empty() {
            self.query_changed();
        }
    }

    /// Search the files open with unsaved changes as their editors have
    /// them: `buffers` holds each one's path, its
    /// [version](ninjaedit_core::FileBuffer::version), and its contents.
    /// Nothing changes unless the set of files or any of their versions
    /// differs from last time; when it does, the results are made afresh
    /// and the highlighted row goes back to the top, since the old rows
    /// no longer mean anything.
    pub fn set_buffers(&mut self, mut buffers: Vec<(PathBuf, u64, BufferSnapshot)>) {
        buffers.sort_by(|a, b| a.0.cmp(&b.0));
        let versions: Vec<(PathBuf, u64)> = buffers
            .iter()
            .map(|(path, version, _)| (path.clone(), *version))
            .collect();
        if versions == self.buffer_versions {
            return;
        }
        self.buffer_versions = versions;
        self.buffers = Arc::new(
            buffers
                .into_iter()
                .map(|(path, _, snapshot)| (path, snapshot))
                .collect(),
        );
        self.highlights = None;
        self.highlight_generation = 0;
        self.context = None;
        self.search.set_buffers(Arc::clone(&self.buffers));
        self.search.wait_for(SEARCH_GRACE);
        self.selected = 0;
        self.first_visible = 0;
        self.center_context = true;
    }

    /// Search for what the input now holds, giving a quick search a
    /// moment to finish so its matches are drawn right away.
    fn query_changed(&mut self) {
        self.search.set_query(self.input.text());
        self.search.wait_for(SEARCH_GRACE);
        self.selected = 0;
        self.first_visible = 0;
        self.center_context = true;
    }

    /// Block until every file has been searched.
    #[cfg(test)]
    pub fn wait(&self) {
        self.search.wait();
    }

    /// The query's text, cursor, and selection.
    #[cfg(test)]
    pub fn input(&self) -> &Input {
        &self.input
    }

    /// The context pane's first visible display column.
    #[cfg(test)]
    pub fn context_col(&self) -> usize {
        self.context_col
    }

    /// Block until every file asked to be highlighted has been.
    #[cfg(test)]
    pub fn wait_for_highlighting(&self) {
        if let Some(highlights) = &self.highlights {
            assert!(highlights.wait_idle(Duration::from_secs(10)));
        }
    }

    /// Whether the syntax highlighting cache exists.
    #[cfg(test)]
    pub fn has_highlights(&self) -> bool {
        self.highlights.is_some()
    }

    /// The dialog is being hidden: let go of what can be made again when
    /// it is shown next, keeping the search and its results.
    pub fn hide(&mut self) {
        self.highlights = None;
        // The next cache counts from zero again.
        self.highlight_generation = 0;
    }

    /// Whether the search has found more, or a file on show has been
    /// highlighted, since the last call, so the application knows to
    /// redraw.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        let generation = self.search.generation();
        if generation != self.generation {
            self.generation = generation;
            changed = true;
        }
        if let Some(highlights) = &self.highlights {
            let generation = highlights.generation();
            if generation != self.highlight_generation {
                self.highlight_generation = generation;
                changed = true;
            }
        }
        changed
    }

    /// Highlight a match, clamped to the matches found so far.
    fn select(&mut self, index: usize) {
        let count = self.search.match_count();
        let index = index.min(count.saturating_sub(1));
        if index != self.selected {
            self.selected = index;
            self.center_context = true;
        }
    }

    fn scroll_context(&mut self, lines: isize) {
        let max = self
            .context
            .as_ref()
            .map_or(0, |(_, lines)| lines.len())
            .saturating_sub(self.context_area.height.max(1) as usize);
        self.context_scroll = self.context_scroll.saturating_add_signed(lines).min(max);
        self.center_context = false;
    }

    /// Scroll the context pane sideways. Scrolling right stops at the
    /// longest visible line plus a little slack, as the editor does, but
    /// a position already past that (left there by a vertical scroll)
    /// stays put.
    fn scroll_context_horizontally(&mut self, columns: isize) {
        let target = self.context_col.saturating_add_signed(columns);
        self.context_col = if columns > 0 {
            target.min(self.max_context_col().max(self.context_col))
        } else {
            target
        };
        self.center_context = false;
    }

    /// The furthest the context pane can scroll right, measured from the
    /// lines visible now.
    fn max_context_col(&self) -> usize {
        let Some((_, lines)) = &self.context else {
            return 0;
        };
        let height = self.context_area.height.max(1) as usize;
        let last = (self.context_scroll + height).min(lines.len());
        let widest = lines[self.context_scroll.min(last)..last]
            .iter()
            .map(|line| line_width(&cells(line, &[], 0)))
            .max()
            .unwrap_or(0);
        (widest + HSCROLL_SLACK).saturating_sub(self.context_text.width as usize)
    }

    fn navigate(&self) -> ProjectSearchOutcome {
        match self.selected() {
            Some(m) => ProjectSearchOutcome::Navigate(m),
            None => ProjectSearchOutcome::Continue,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent, clipboard: &mut Clipboard) -> ProjectSearchOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.rows.height.max(1) as usize;
        let context_page = self.context_area.height.max(1) as usize;
        match key.code {
            KeyCode::Esc => return ProjectSearchOutcome::Close,
            KeyCode::Enter => return self.navigate(),
            KeyCode::Up if ctrl => self.scroll_context(-1),
            KeyCode::Down if ctrl => self.scroll_context(1),
            KeyCode::PageUp if ctrl => self.scroll_context(-(context_page as isize)),
            KeyCode::PageDown if ctrl => self.scroll_context(context_page as isize),
            KeyCode::Up => self.select(self.selected.saturating_sub(1)),
            KeyCode::Down => self.select(self.selected + 1),
            KeyCode::PageUp => self.select(self.selected.saturating_sub(page)),
            KeyCode::PageDown => self.select(self.selected + page),
            KeyCode::Home if ctrl => self.select(0),
            KeyCode::End if ctrl => self.select(usize::MAX),
            _ => {
                if self.input.handle_key(key, clipboard) == InputKey::Changed {
                    self.query_changed();
                }
            }
        }
        ProjectSearchOutcome::Continue
    }

    /// Add pasted text to the query. A match can't span lines, so line
    /// breaks are dropped.
    pub fn paste(&mut self, text: &str) {
        if self.input.paste(text) {
            self.query_changed();
        }
    }

    /// Whether the mouse position is over the dialog.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.area.contains(ScreenPosition::new(x, y))
    }

    /// Whether a drag that started in the query is going on, in which
    /// case the dialog wants drag and release events wherever they
    /// happen.
    pub fn is_dragging(&self) -> bool {
        self.input.is_dragging()
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> ProjectSearchOutcome {
        let (x, y) = (mouse.column, mouse.row);
        let at = ScreenPosition::new(x, y);
        if self.input.is_dragging() || self.input.contains(x, y) {
            self.input.handle_mouse(mouse);
            return ProjectSearchOutcome::Continue;
        }
        let in_context = self.context_area.contains(at);
        match mouse.kind {
            MouseEventKind::ScrollUp if in_context => {
                self.scroll_context(-(WHEEL_LINES as isize));
            }
            MouseEventKind::ScrollDown if in_context => {
                self.scroll_context(WHEEL_LINES as isize);
            }
            MouseEventKind::ScrollLeft if in_context => {
                self.scroll_context_horizontally(-(WHEEL_COLUMNS as isize));
            }
            MouseEventKind::ScrollRight if in_context => {
                self.scroll_context_horizontally(WHEEL_COLUMNS as isize);
            }
            MouseEventKind::ScrollUp => self.select(self.selected.saturating_sub(1)),
            MouseEventKind::ScrollDown => self.select(self.selected + 1),
            MouseEventKind::Down(MouseButton::Left) if self.rows.contains(at) => {
                let row = self.first_visible + (y - self.rows.y) as usize;
                if row < self.search.match_count() {
                    self.select(row);
                    if self.clicks.press(x, y) == 2 {
                        return self.navigate();
                    }
                }
            }
            _ => {}
        }
        ProjectSearchOutcome::Continue
    }

    /// What the bottom border says about the search.
    fn hint(&self) -> Option<String> {
        if self.query().is_empty() {
            return Some(KEYS_HINT.to_owned());
        }
        if let Some(error) = self.search.error() {
            return Some(format!("invalid regex: {error}"));
        }
        let kind = if self.search.is_regex() {
            "regex, "
        } else {
            ""
        };
        let count = self.search.match_count();
        let found = match self.search.phase() {
            SearchPhase::WaitingForIndex => "waiting for the file index…".to_owned(),
            SearchPhase::Searching => format!("searching… {count} so far"),
            SearchPhase::Done if self.search.is_truncated() => {
                format!("first {count} matches; narrow the search to see the rest")
            }
            SearchPhase::Done => {
                let files = self.search.file_count();
                match (count, files) {
                    (0, _) => "no matches".to_owned(),
                    (1, _) => "1 match".to_owned(),
                    (n, 1) => format!("{n} matches in 1 file"),
                    (n, f) => format!("{n} matches in {f} files"),
                }
            }
        };
        Some(format!("{kind}{found}"))
    }

    // ----- Rendering ------------------------------------------------------

    /// Draw the dialog over most of `screen`. Returns where the terminal
    /// cursor belongs.
    pub fn render(
        &mut self,
        screen: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) -> Option<ScreenPosition> {
        if screen.width < 12 || screen.height < 5 {
            return None;
        }
        self.area = Rect::new(
            screen.x + 2,
            screen.y + 1,
            screen.width - 4,
            screen.height - 2,
        );
        let hint = self.hint();
        let inner = render_frame(self.area, hint.as_deref(), buf, theme);
        if inner.height == 0 {
            return None;
        }
        let cursor = self.input.render(
            Rect::new(inner.x, inner.y, inner.width, 1),
            PLACEHOLDER,
            buf,
            theme,
        );

        // The rows under the query are split between the list and the
        // context pane, with a rule between them; a very short dialog is
        // all list.
        let rest = inner.height - 1;
        let (list_height, context_height) = if rest < 4 {
            (rest, 0)
        } else {
            let list = (rest - 1) / 2;
            (list, rest - 1 - list)
        };
        self.rows = Rect::new(inner.x, inner.y + 1, inner.width, list_height);
        if context_height > 0 {
            let y = self.rows.bottom();
            let style = palette_background(theme).fg(theme.command_palette_box_color);
            buf.set_string(self.area.x, y, "├", style);
            buf.set_string(self.area.right() - 1, y, "┤", style);
            for x in inner.x..inner.right() {
                buf.set_string(x, y, "─", style);
            }
            self.context_area = Rect::new(inner.x, y + 1, inner.width, context_height);
        } else {
            self.context_area = Rect::default();
        }
        self.render_results(buf, theme);
        self.render_context(buf, theme);
        cursor
    }

    fn render_results(&mut self, buf: &mut Buffer, theme: &Theme) {
        let background = palette_background(theme);
        let rows = self.rows;
        if rows.height == 0 || rows.width < 4 {
            return;
        }
        let count = self.search.match_count();
        if count == 0 {
            let message = if self.query().is_empty() {
                "Type to search every file in the project"
            } else if self.search.is_done() || self.search.error().is_some() {
                "No matches"
            } else {
                "Searching…"
            };
            buf.set_stringn(
                rows.x + 1,
                rows.y,
                message,
                rows.width.saturating_sub(1) as usize,
                background.fg(theme.command_palette_result_context_text),
            );
            self.selected = 0;
            self.first_visible = 0;
            return;
        }
        self.selected = self.selected.min(count - 1);
        let height = rows.height as usize;
        if self.selected < self.first_visible {
            self.first_visible = self.selected;
        } else if self.selected >= self.first_visible + height {
            self.first_visible = self.selected + 1 - height;
        }
        self.first_visible = self.first_visible.min(count.saturating_sub(height));
        let matches = self
            .search
            .slice(self.first_visible..self.first_visible + height);
        let width = rows.width as usize;
        let selected = self.selected;
        let first_visible = self.first_visible;
        let root = &self.root;
        let highlights = self
            .highlights
            .get_or_insert_with(|| HighlightCache::with_buffers(Arc::clone(&self.buffers)));
        // Consecutive rows are often in one file: look its states up once.
        let mut file_states: Option<(Arc<Path>, Option<Arc<FileStates>>)> = None;
        for (row, m) in matches.iter().enumerate() {
            let y = rows.y + row as u16;
            let highlighted = first_visible + row == selected;
            if file_states.as_ref().map(|(path, _)| path) != Some(&m.path) {
                file_states = Some((Arc::clone(&m.path), highlights.states(&m.path)));
            }
            // The row's text is lexed from its line's starting state,
            // which only works for the line itself or a prefix of it, not
            // a window cut from later in a very long line. The highlighted
            // row keeps the selection colors alone, as the editor does
            // with a selection text color set.
            let tokens: Vec<Token> = match &file_states {
                Some((_, Some(states))) if !highlighted && m.text_offset == 0 => {
                    states.tokens(m.line, m.text.as_bytes())
                }
                _ => Vec::new(),
            };
            let (base, location_style, found) = if highlighted {
                let base = Style::default()
                    .fg(theme.command_palette_selection_text)
                    .bg(theme.command_palette_selection_background);
                buf.set_style(Rect::new(rows.x, y, rows.width, 1), base);
                (base, base, base.add_modifier(Modifier::BOLD))
            } else {
                (
                    background,
                    background.fg(theme.project_search_location_text),
                    background
                        .bg(theme.find_result_background)
                        .add_modifier(Modifier::BOLD),
                )
            };
            // The location on the right, cut from the left if it must be,
            // so the file name and line number always show; the matched
            // line on the left, with a space before it and two between it
            // and the location.
            let location = format!("{}:{}", display_path(root, &m.path), m.line + 1);
            let trimmed = (m.text.len() - m.text.trim_start().len()).min(m.range.start);
            let content_needed = line_width(&cells(&m.text[trimmed..], &[], 0));
            let (content_width, location_width) = split_row(
                width.saturating_sub(3),
                content_needed,
                Span::raw(&location).width(),
            );
            let location = fit_end(&location, location_width);
            buf.set_string(
                rows.right() - location_width as u16,
                y,
                &location,
                location_style,
            );
            let content = Rect::new(rows.x + 1, y, content_width as u16, 1);
            let line = Highlighted {
                text: &m.text,
                tokens: &tokens,
                offset: 0,
                theme,
            };
            render_excerpt(buf, content, line, m.range.clone(), base, found);
        }
    }

    fn render_context(&mut self, buf: &mut Buffer, theme: &Theme) {
        let area = self.context_area;
        if area.height == 0 {
            return;
        }
        let Some(m) = self.search.get(self.selected) else {
            self.context = None;
            return;
        };
        if self.context.as_ref().map(|(path, _)| &**path) != Some(&*m.path) {
            let lines = self.search.file_lines(&m.path).unwrap_or_default();
            self.context = Some((Arc::clone(&m.path), lines));
            self.context_col = 0;
            self.center_context = true;
        }
        let Some((_, lines)) = &self.context else {
            return;
        };
        let background = palette_background(theme).bg(theme
            .search_preview_background
            .unwrap_or(theme.command_palette_background));
        buf.set_style(area, background);
        let states = self
            .highlights
            .get_or_insert_with(|| HighlightCache::with_buffers(Arc::clone(&self.buffers)))
            .states(&m.path);
        let number_width = digits(lines.len().max(1));
        // Number, space, guide, then the text.
        let gutter = number_width + 2;
        if (area.width as usize) < gutter + 2 {
            return;
        }
        let height = area.height as usize;
        let text_width = area.width as usize - gutter;
        self.context_text = Rect::new(
            area.x + gutter as u16,
            area.y,
            text_width as u16,
            area.height,
        );

        // Bring the match into view as the editor does when jumping to
        // it: centered vertically, and sideways scrolled as little as
        // will do (see `EditorView::scroll_to_columns`).
        let match_cells = lines.get(m.line).map(|text| cells(text, &[], 0));
        let match_range =
            m.column..(m.column + m.len).min(lines.get(m.line).map_or(0, String::len));
        if self.center_context {
            self.context_scroll = m.line.saturating_sub(height / 2);
            if let Some(cells) = &match_cells {
                let (start, end) = match_columns(cells, &match_range);
                if start < self.context_col || end > self.context_col + text_width {
                    self.context_col = if end - start > text_width {
                        start
                    } else {
                        end.saturating_sub(text_width)
                    };
                }
            }
            self.center_context = false;
        }
        self.context_scroll = self.context_scroll.min(lines.len().saturating_sub(height));

        let found = background
            .bg(theme.find_result_background)
            .add_modifier(Modifier::BOLD);
        for row in 0..height {
            let line = self.context_scroll + row;
            let Some(text) = lines.get(line) else {
                break;
            };
            let y = area.y + row as u16;
            let color = if line == m.line {
                theme.active_line_number
            } else {
                theme.inactive_line_number
            };
            buf.set_string(
                area.x,
                y,
                format!("{:>width$}", line + 1, width = number_width),
                background.fg(color),
            );
            buf.set_string(
                area.x + number_width as u16 + 1,
                y,
                GUIDE,
                background.fg(theme.gutter_guide),
            );
            let text_area = Rect::new(self.context_text.x, y, self.context_text.width, 1);
            let tokens = states
                .as_ref()
                .map_or(Vec::new(), |states| states.tokens(line, text.as_bytes()));
            let cells = cells(text, &tokens, 0);
            draw_cells(buf, text_area, &cells, self.context_col, None, |cell| {
                let base = if line == m.line && match_range.contains(&cell.range.start) {
                    found
                } else {
                    background
                };
                styled(theme, cell.kind, base)
            });
        }
    }
}

/// A path for display: relative to the project `root` when inside it.
fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// A line of text with the syntax tokens that classify it, if it has
/// been lexed; see [`cells`].
#[derive(Clone, Copy)]
struct Highlighted<'a> {
    text: &'a str,
    tokens: &'a [Token],
    /// The byte offset of `text` within the line the tokens are for.
    offset: usize,
    theme: &'a Theme,
}

/// A cell's style: `base` with the theme's color for what the cell is
/// part of. Plain text keeps `base` as it is, so that a line looks the
/// same before and after its file is lexed apart from what the lexer
/// picks out.
fn styled(theme: &Theme, kind: TokenKind, base: Style) -> Style {
    if kind == TokenKind::Text {
        base
    } else {
        theme.syntax(kind).apply(base)
    }
}

/// One character of a line laid out for display.
struct Cell<'a> {
    range: Range<usize>,
    text: &'a str,
    column: usize,
    width: usize,
    /// What the character is part of; [`TokenKind::Text`] where the
    /// tokens don't say.
    kind: TokenKind,
}

/// Lay out a line's characters, expanding tabs to the next tab stop, and
/// classify each by `tokens`, whose ranges are relative to a line that
/// `text` starts `offset` bytes into. A character is classified by the
/// token containing its first byte, as in the editor.
fn cells<'a>(text: &'a str, tokens: &[Token], offset: usize) -> Vec<Cell<'a>> {
    let mut column = 0;
    let mut next_token = 0;
    text::graphemes(text.as_bytes())
        .map(|g| {
            let width = text::width(g.text, column, TAB_WIDTH);
            let start = offset + g.range.start;
            while next_token < tokens.len() && tokens[next_token].range.end <= start {
                next_token += 1;
            }
            let kind = match tokens.get(next_token) {
                Some(token) if token.range.start <= start => token.kind,
                _ => TokenKind::Text,
            };
            let cell = Cell {
                range: g.range,
                text: g.text,
                column,
                width,
                kind,
            };
            column += width;
            cell
        })
        .collect()
}

/// Draw a line's cells into a one-row `area`, starting from display
/// column `start`. With `ellipsis`, an ellipsis in that style, dimmed,
/// marks either edge where the line continues past it; without, the line
/// is simply cut off, as in the editor. `style_of` gives each cell's
/// style.
fn draw_cells(
    buf: &mut Buffer,
    area: Rect,
    cells: &[Cell<'_>],
    start: usize,
    ellipsis: Option<Style>,
    style_of: impl Fn(&Cell<'_>) -> Style,
) {
    let width = area.width as usize;
    if width == 0 {
        return;
    }
    let total = line_width(cells);
    let left_cut = ellipsis.is_some() && start > 0;
    let right_cut = ellipsis.is_some() && total > start + width;
    let visible = (start + usize::from(left_cut))..(start + width - usize::from(right_cut));
    for cell in cells {
        if cell.width == 0 || cell.column + cell.width <= visible.start {
            continue;
        }
        if cell.column >= visible.end {
            break;
        }
        let style = style_of(cell);
        let fits = cell.column >= visible.start && cell.column + cell.width <= visible.end;
        let from = cell.column.max(visible.start);
        let to = (cell.column + cell.width).min(visible.end);
        let x = area.x + (from - start) as u16;
        if cell.text == "\t" || !fits {
            // Tabs are blank; a wide character cut off by the edge shows
            // as blank for the part that fits.
            for x in x..x + (to - from) as u16 {
                buf[(x, area.y)].set_symbol(" ").set_style(style);
            }
        } else {
            buf.set_string(x, area.y, cell.text, style);
        }
    }
    let dim = ellipsis.unwrap_or_default().add_modifier(Modifier::DIM);
    if left_cut {
        buf.set_string(area.x, area.y, ELLIPSIS, dim);
    }
    if right_cut {
        buf.set_string(area.right() - 1, area.y, ELLIPSIS, dim);
    }
}

/// Draw a line with a match in it into a one-row `area`, scrolled so the
/// match is in view: a line that fits is shown whole; otherwise the match
/// is centered in the row, or, if it is wider than the row, starts at the
/// left edge so as much of it shows as can. Leading whitespace is
/// dropped. Text is drawn in `base` and the match in `found`, each
/// colored by the line's tokens.
fn render_excerpt(
    buf: &mut Buffer,
    area: Rect,
    line: Highlighted<'_>,
    range: Range<usize>,
    base: Style,
    found: Style,
) {
    let width = area.width as usize;
    if width == 0 {
        return;
    }
    let text = line.text;
    let trimmed = (text.len() - text.trim_start().len()).min(range.start);
    let text = &text[trimmed..];
    let range = range.start - trimmed..range.end.max(range.start) - trimmed;
    let cells = cells(text, line.tokens, line.offset + trimmed);
    let total = line_width(&cells);
    let (match_start, match_end) = match_columns(&cells, &range);
    let start = if total <= width {
        0
    } else if match_end - match_start >= width {
        match_start
    } else {
        match_start
            .saturating_sub((width - (match_end - match_start)) / 2)
            .min(total - width)
    };
    draw_cells(buf, area, &cells, start, Some(base), |cell| {
        let base = if range.contains(&cell.range.start) {
            found
        } else {
            base
        };
        styled(line.theme, cell.kind, base)
    });
}

/// The display width of a laid-out line.
fn line_width(cells: &[Cell<'_>]) -> usize {
    cells.last().map_or(0, |c| c.column + c.width)
}

/// The display columns a byte range of a laid-out line starts and ends
/// at.
fn match_columns(cells: &[Cell<'_>], range: &Range<usize>) -> (usize, usize) {
    let total = line_width(cells);
    let start = cells
        .iter()
        .find(|c| c.range.end > range.start)
        .map_or(total, |c| c.column);
    let end = cells
        .iter()
        .rfind(|c| c.range.start < range.end)
        .map_or(start, |c| c.column + c.width);
    (start, end)
}

/// How to share a result row's `available` columns between its content
/// and its location, given the columns each would like: both get what
/// they need when that fits; otherwise the location gets what the
/// content leaves it, but at least a third of the row, and the content
/// the rest.
fn split_row(available: usize, content: usize, location: usize) -> (usize, usize) {
    if content + location <= available {
        return (content, location);
    }
    let location = location.min((available - content.min(available)).max(available / 3));
    (available - location, location)
}

/// `text` if it fits in `width` columns, else its end with an ellipsis
/// before it.
fn fit_end(text: &str, width: usize) -> String {
    if Span::raw(text).width() <= width {
        return text.to_owned();
    }
    let room = width.saturating_sub(1);
    let mut kept = 0;
    let mut cut = text.len();
    for g in text::graphemes(text.as_bytes())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let w = text::width(g.text, 0, TAB_WIDTH);
        if kept + w > room {
            break;
        }
        kept += w;
        cut = g.range.start;
    }
    format!("{ELLIPSIS}{}", &text[cut..])
}

/// The number of decimal digits needed to show `n`.
fn digits(n: usize) -> usize {
    n.max(1).ilog10() as usize + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn row(buf: &Buffer, area: Rect) -> String {
        (area.x..area.right())
            .map(|x| buf[(x, area.y)].symbol().to_owned())
            .collect()
    }

    #[test]
    fn excerpts_keep_the_match_in_view() {
        let area = Rect::new(0, 0, 10, 1);
        let base = Style::default();
        let found = Style::default().add_modifier(Modifier::BOLD);
        let theme = Theme::default();
        let draw = |text: &str, range: Range<usize>| {
            let mut buf = Buffer::empty(area);
            let line = Highlighted {
                text,
                tokens: &[],
                offset: 0,
                theme: &theme,
            };
            render_excerpt(&mut buf, area, line, range, base, found);
            let bold: String = (0..10)
                .map(|x| {
                    if buf[(x, 0)].modifier.contains(Modifier::BOLD) {
                        '^'
                    } else {
                        ' '
                    }
                })
                .collect();
            (row(&buf, area), bold)
        };
        // Short lines are shown whole, less their indentation.
        assert_eq!(
            draw("  ab cd", 5..7),
            ("ab cd     ".into(), "   ^^     ".into())
        );
        // A match near the end of a long line is centered.
        let (text, bold) = draw("0123456789abcdefghij", 12..14);
        assert_eq!(text, "…9abcdefg…");
        assert_eq!(bold, "    ^^    ");
        // A match at the start shows the start.
        assert_eq!(
            draw("0123456789abcdefghij", 0..2),
            ("012345678…".into(), "^^        ".into())
        );
        // A match wider than the row starts at the left edge.
        let (text, bold) = draw("xx0123456789abcdefghij", 2..22);
        assert_eq!(text, "…12345678…");
        assert_eq!(bold, " ^^^^^^^^ ");
        // Tabs expand; a match inside the line isn't cut by trimming.
        assert_eq!(
            draw("\tab", 1..3),
            ("ab        ".into(), "^^        ".into())
        );
        assert_eq!(
            draw("a\tb", 2..3),
            ("a   b     ".into(), "    ^     ".into())
        );
    }

    #[test]
    fn tokens_color_the_cells() {
        let theme = Theme::parse("syntax-keyword = \"*#010203\"\n").unwrap();
        let area = Rect::new(0, 0, 12, 1);
        let base = Style::default().fg(Color::White);
        let found = base.bg(Color::Blue);
        // Tokens are for the whole line; the text is a window of it, and
        // its indentation is trimmed on top of that.
        let tokens = [
            Token {
                range: 4..6,
                kind: TokenKind::Keyword,
            },
            Token {
                range: 7..11,
                kind: TokenKind::Function,
            },
        ];
        let mut buf = Buffer::empty(area);
        let line = Highlighted {
            text: "  fn main",
            tokens: &tokens,
            offset: 2,
            theme: &theme,
        };
        render_excerpt(&mut buf, area, line, 5..9, base, found);
        assert_eq!(row(&buf, area), "fn main     ");
        let keyword = &buf[(0, 0)];
        assert_eq!(keyword.fg, Color::Rgb(1, 2, 3));
        assert!(keyword.modifier.contains(Modifier::BOLD));
        assert_eq!(buf[(1, 0)].fg, Color::Rgb(1, 2, 3));
        // The space between is plain text in the base style.
        assert_eq!(buf[(2, 0)].fg, Color::White);
        assert_eq!(buf[(2, 0)].bg, Color::Reset);
        // The match keeps its token color over the match background.
        let name = &buf[(3, 0)];
        assert_eq!(name.fg, theme.syntax(TokenKind::Function).color);
        assert_eq!(name.bg, Color::Blue);
        assert_eq!(buf[(7, 0)].bg, Color::Reset);
    }

    #[test]
    fn rows_favor_the_content_over_the_location() {
        // Both fit: each gets what it needs.
        assert_eq!(split_row(60, 20, 10), (20, 10));
        // A long location takes only what a short content leaves.
        assert_eq!(split_row(60, 20, 50), (20, 40));
        // A long content leaves the location a third of the row.
        assert_eq!(split_row(60, 100, 50), (40, 20));
        assert_eq!(split_row(60, 100, 10), (50, 10));
        // Both long: two thirds to the content.
        assert_eq!(split_row(60, 100, 100), (40, 20));
        assert_eq!(split_row(0, 5, 5), (0, 0));
    }

    #[test]
    fn locations_are_cut_from_the_left() {
        assert_eq!(fit_end("src/main.rs:12", 20), "src/main.rs:12");
        assert_eq!(fit_end("src/main.rs:12", 8), "…n.rs:12");
        assert_eq!(fit_end("src/main.rs:12", 1), "…");
        assert_eq!(fit_end("한글.rs:1", 5), "…rs:1");
    }
}
