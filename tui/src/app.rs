//! Application state: the project, the open tabs, the command palette, and
//! the status bar, plus the routing of events between them.
//!
//! Layout, top to bottom: the tab bar, the editor for the active tab (or a
//! hint when nothing is open), and the status bar. The command palette,
//! when open, floats over the top of the editor and takes all keyboard
//! input until it is closed. The search box (Ctrl+F) floats there too,
//! and never at the same time as the palette; it drives a search in the
//! active tab's editor, which highlights what it finds as the query is
//! typed. Opened while text within one line is selected, the box starts
//! with that text as its query (selected, so typing replaces it) and the
//! search already run, which with a double-click to select a word makes
//! looking for the other uses of a name a two-step affair. Enter selects
//! the current match and closes the box, leaving the
//! other matches highlighted; Ctrl+G, with the box open or closed, steps
//! to the next match, and with no search going repeats the last query
//! from the cursor. Escape closes the box and clears the search.
//!
//! The project search dialog (Ctrl+Shift+F) searches every file in the
//! project; see the `project_search` module. It is seeded from a
//! single-line selection like the search box, but only when that text
//! isn't what it is already searching for: otherwise it comes back just
//! as it was closed, so that Enter (go to the match, closing the dialog)
//! and Ctrl+Shift+F, Down, Enter work through the matches one by one.
//! Either way the query is selected, so typing starts a new search.
//! Going to a match selects it in its file, opening the file if need be.
//!
//! Closing a modified tab or quitting with unsaved changes asks for the key
//! to be pressed a second time rather than popping up a dialog.

use crate::clipboard::Clipboard;
use crate::editor_view::EditorView;
use crate::palette::{Palette, PaletteAction, PaletteItem, PaletteOutcome};
use crate::project_search::{ProjectSearchDialog, ProjectSearchOutcome};
use crate::search_box::{self, SearchBox, SearchOutcome};
use crate::tabs::{TabBar, TabHit, TabLabel};
use crate::theme::Theme;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ninjaedit_core::search::literal_query;
use ninjaedit_core::{Editor, Project, ProjectMatch, SearchStep};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use std::path::Path;
use std::time::Duration;

const TABS_PLACEHOLDER: &str = "Search open tabs";
const FILES_PLACEHOLDER: &str = "Search files in project";
/// How long a change to the search query waits for the search to finish,
/// so that in all but the largest files the matches show up in the same
/// frame as the keystroke rather than a tick later.
const SEARCH_GRACE: Duration = Duration::from_millis(15);
const SEARCH_WRAPPED: &str =
    "Search wrapped around to where it started; press Ctrl+G to go around again";

/// An action that discards unsaved changes and so needs confirming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Confirm {
    CloseTab(usize),
    Quit,
}

struct Tab {
    view: EditorView,
    /// When the tab was last active, as a tick of [`App::view_clock`].
    /// Higher is more recent.
    last_viewed: u64,
    /// The editor's highlight generation as of the last redraw, to notice
    /// when background highlighting has changed what's on screen.
    highlight_generation: u64,
    /// Likewise for the editor's search, which finds matches in the
    /// background.
    search_generation: u64,
}

impl Tab {
    fn path(&self) -> Option<&Path> {
        self.view.editor().buffer().path()
    }

    fn title(&self) -> String {
        self.path()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_owned())
    }
}

pub struct App {
    project: Project,
    theme: Theme,
    tabs: Vec<Tab>,
    active: usize,
    /// Counts tab activations, to order tabs by most recently viewed.
    view_clock: u64,
    tab_bar: TabBar,
    palette: Option<Palette>,
    /// Whether the open palette searches project files, and so needs
    /// refreshing as the index fills in.
    palette_is_files: bool,
    /// The search box, driving a search in the active tab. Never open at
    /// the same time as the palette.
    search_box: Option<SearchBox>,
    /// The last query searched for with Enter, for Ctrl+G to repeat.
    last_search: String,
    /// The project search dialog, kept (with its results) once opened so
    /// that it comes back as it was. Shown while `project_search_open`,
    /// never at the same time as the palette or the search box.
    project_search: Option<ProjectSearchDialog>,
    project_search_open: bool,
    index_generation: u64,
    /// A message shown in the status bar until the next key press.
    status: Option<String>,
    confirm: Option<Confirm>,
    /// Lives as long as the app: see the `clipboard` module for why.
    clipboard: Clipboard,
    quit: bool,
    editor_area: Rect,
}

impl App {
    pub fn new(project: Project) -> App {
        App {
            index_generation: project.index().generation(),
            project,
            theme: Theme::default(),
            tabs: Vec::new(),
            active: 0,
            view_clock: 0,
            tab_bar: TabBar::default(),
            palette: None,
            palette_is_files: false,
            search_box: None,
            last_search: String::new(),
            project_search: None,
            project_search_open: false,
            status: None,
            confirm: None,
            clipboard: Clipboard::new(),
            quit: false,
            editor_area: Rect::default(),
        }
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Open a file in a new tab, or switch to its tab if it is already
    /// open. Failures are reported in the status bar.
    pub fn open_file(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref();
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(index) = self.tabs.iter().position(|tab| tab.path() == Some(&path)) {
            self.activate(index);
            return;
        }
        match self.project.open_file(&path) {
            Ok(buffer) => {
                let editor = Editor::new(buffer);
                self.tabs.push(Tab {
                    highlight_generation: editor.highlight_generation(),
                    search_generation: 0,
                    view: EditorView::new(editor),
                    last_viewed: 0,
                });
                self.activate(self.tabs.len() - 1);
            }
            Err(err) => {
                self.status = Some(format!("Could not open {}: {err}", path.display()));
            }
        }
    }

    /// Make a tab the active one, recording it as the most recently viewed.
    fn activate(&mut self, index: usize) {
        if let Some(tab) = self.tabs.get_mut(index) {
            self.active = index;
            self.view_clock += 1;
            tab.last_viewed = self.view_clock;
        }
    }

    fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        if self.active == index {
            // Closing the active tab goes back to the most recently viewed
            // of the rest, as the user most likely came from there.
            if let Some(recent) = self.most_recent_tab() {
                self.activate(recent);
            }
        } else if self.active > index {
            self.active -= 1;
        }
    }

    /// The most recently viewed tab, ignoring the active one.
    fn most_recent_tab(&self) -> Option<usize> {
        self.tabs
            .iter()
            .enumerate()
            .max_by_key(|(_, tab)| tab.last_viewed)
            .map(|(index, _)| index)
    }

    /// Close a tab, asking for confirmation first if it has unsaved changes.
    fn request_close_tab(&mut self, index: usize) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        if tab.view.editor().is_modified() && self.confirm != Some(Confirm::CloseTab(index)) {
            self.confirm = Some(Confirm::CloseTab(index));
            self.status = Some(format!(
                "{} has unsaved changes: press Ctrl+W again to close without saving, Ctrl+S to save",
                tab.title()
            ));
            return;
        }
        self.close_tab(index);
    }

    fn request_quit(&mut self) {
        let unsaved = self
            .tabs
            .iter()
            .filter(|t| t.view.editor().is_modified())
            .count();
        if unsaved > 0 && self.confirm != Some(Confirm::Quit) {
            self.confirm = Some(Confirm::Quit);
            self.status = Some(format!(
                "{unsaved} file(s) have unsaved changes: press Ctrl+Q again to quit without saving"
            ));
            return;
        }
        self.quit = true;
    }

    fn save_active(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let title = tab.title();
        self.status = Some(match tab.view.editor_mut().save() {
            Ok(()) => format!("Saved {title}"),
            Err(err) => format!("Could not save {title}: {err}"),
        });
    }

    /// A path for display: relative to the project when inside it.
    fn display_path(&self, path: &Path) -> String {
        path.strip_prefix(self.project.root())
            .unwrap_or(path)
            .display()
            .to_string()
    }

    // ----- Command palette ------------------------------------------------

    /// Open the tab search, listing tabs most recently viewed first. The
    /// active tab is always at the top, so the previously viewed one is
    /// preselected: Enter alone flips back to it.
    fn open_tabs_palette(&mut self) {
        self.close_search_box(true);
        self.hide_project_search();
        let mut order: Vec<usize> = (0..self.tabs.len()).collect();
        order.sort_by_key(|&index| std::cmp::Reverse(self.tabs[index].last_viewed));
        let items = order
            .into_iter()
            .map(|index| {
                let tab = &self.tabs[index];
                let path = tab.path().map(|p| self.display_path(p)).unwrap_or_default();
                PaletteItem {
                    label: tab.title(),
                    detail: parent_of(&path),
                    search: path,
                    action: PaletteAction::SwitchTab(index),
                }
            })
            .collect();
        let mut palette = Palette::new(TABS_PLACEHOLDER, items);
        palette.select(1);
        self.palette = Some(palette);
        self.palette_is_files = false;
    }

    fn open_files_palette(&mut self) {
        self.close_search_box(true);
        self.hide_project_search();
        let mut palette = Palette::new(FILES_PLACEHOLDER, self.file_items());
        self.refresh_index_hint(&mut palette);
        self.palette = Some(palette);
        self.palette_is_files = true;
    }

    fn file_items(&self) -> Vec<PaletteItem> {
        let mut paths = self.project.index().files(false);
        paths.sort();
        paths
            .into_iter()
            .map(|path| {
                let relative = self.display_path(&path);
                PaletteItem {
                    label: path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    detail: parent_of(&relative),
                    search: relative,
                    action: PaletteAction::OpenFile(path),
                }
            })
            .collect()
    }

    fn refresh_index_hint(&self, palette: &mut Palette) {
        let hint = (!self.project.index().is_primary_complete()).then(|| "indexing…".to_owned());
        palette.set_hint(hint);
    }

    fn run_palette_action(&mut self, action: PaletteAction) {
        self.palette = None;
        match action {
            PaletteAction::SwitchTab(index) => self.activate(index),
            PaletteAction::OpenFile(path) => self.open_file(path),
        }
    }

    // ----- Search ---------------------------------------------------------

    /// Open the search box over the active tab, starting a search from its
    /// cursor, or with the selected text as the query when the selection
    /// lies within one line. Does nothing with the box already open, or
    /// with no tab.
    fn open_search_box(&mut self) {
        if self.search_box.is_some() {
            return;
        }
        if self.tabs.get(self.active).is_none() {
            return;
        }
        self.palette = None;
        self.hide_project_search();
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let editor = tab.view.editor_mut_in_place();
        let seed = editor
            .selected_text()
            .filter(|text| !text.contains(['\n', '\r']));
        editor.start_search();
        let mut search_box = SearchBox::new();
        let seeded = seed.is_some();
        if let Some(seed) = seed {
            search_box.set_query_selected(&literal_query(&seed));
        }
        self.search_box = Some(search_box);
        if seeded {
            self.update_search_query();
        }
    }

    /// Close the search box, and with `clear` drop the search along with
    /// its highlights; otherwise the accepted search stays.
    fn close_search_box(&mut self, clear: bool) {
        if self.search_box.take().is_some()
            && clear
            && let Some(tab) = self.tabs.get_mut(self.active)
        {
            tab.view.editor_mut_in_place().clear_search();
        }
    }

    /// Search for what the box now holds, giving a quick search a moment
    /// to finish so its matches are drawn right away.
    fn update_search_query(&mut self) {
        let (Some(search_box), Some(tab)) = (&self.search_box, self.tabs.get_mut(self.active))
        else {
            return;
        };
        let editor = tab.view.editor_mut_in_place();
        editor.set_search_query(search_box.query());
        if let Some(search) = editor.search() {
            search.wait_for(SEARCH_GRACE);
        }
    }

    fn handle_search_outcome(&mut self, outcome: SearchOutcome) {
        match outcome {
            SearchOutcome::Continue => {}
            SearchOutcome::Changed => self.update_search_query(),
            SearchOutcome::Close => self.close_search_box(true),
            SearchOutcome::Accept => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return;
                };
                // With nothing to select the box stays open, its hint
                // saying why.
                if tab.view.editor_mut().accept_search() {
                    if let Some(search_box) = &self.search_box {
                        self.last_search = search_box.query().to_owned();
                    }
                    self.close_search_box(false);
                }
            }
            SearchOutcome::Next => self.step_search(),
        }
    }

    /// Ctrl+G: move the active tab's search on to its next match, or with
    /// no search going, search again for the last query from the cursor.
    fn find_next(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        if tab.view.editor().search().is_some() {
            self.step_search();
        } else if self.last_search.is_empty() {
            self.open_search_box();
        } else {
            let editor = tab.view.editor_mut();
            editor.start_search();
            editor.set_search_query(&self.last_search);
            if !editor.accept_search() {
                editor.clear_search();
                self.status = Some(format!("No matches for {}", self.last_search));
            }
        }
    }

    fn step_search(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let step = if self.search_box.is_some() {
            tab.view.editor_mut_in_place().next_match()
        } else {
            tab.view.editor_mut().next_match()
        };
        if step == SearchStep::ReachedStart {
            self.status = Some(SEARCH_WRAPPED.to_owned());
        }
    }

    /// What the search box's bottom border says about the search.
    fn search_hint(&self) -> Option<String> {
        let search = self.tabs.get(self.active)?.view.editor().search()?;
        if search.query().is_empty() {
            return None;
        }
        if let Some(error) = search.error() {
            return Some(format!("invalid regex: {error}"));
        }
        let kind = if search.is_regex() { "regex, " } else { "" };
        let count = search.match_count();
        let found = if !search.is_done() {
            format!("searching… {count} so far")
        } else {
            match count {
                0 => "no matches".to_owned(),
                1 => "1 match".to_owned(),
                n => format!("{n} matches"),
            }
        };
        Some(format!("{kind}{found}"))
    }

    // ----- Project search -------------------------------------------------

    /// Show the project search dialog, seeded with the active tab's
    /// selection when that lies within one line and isn't what the dialog
    /// is already searching for; otherwise the dialog comes back as it
    /// was, its query selected so that typing replaces it.
    fn open_project_search(&mut self) {
        self.palette = None;
        self.close_search_box(true);
        let seed = self
            .tabs
            .get(self.active)
            .and_then(|tab| tab.view.editor().selected_text())
            .filter(|text| !text.contains(['\n', '\r']));
        let dialog = self.project_search.get_or_insert_with(|| {
            ProjectSearchDialog::new(
                self.project.root().to_path_buf(),
                self.project.index().file_list(),
            )
        });
        match seed {
            Some(seed) if !dialog.is_searching_for(&seed) => {
                dialog.set_query_selected(&literal_query(&seed));
            }
            _ => dialog.select_query(),
        }
        self.project_search_open = true;
    }

    /// Hide the project search dialog, keeping it (and its results) for
    /// next time, less what it can make again.
    fn hide_project_search(&mut self) {
        if self.project_search_open
            && let Some(dialog) = &mut self.project_search
        {
            dialog.hide();
        }
        self.project_search_open = false;
    }

    fn handle_project_search_outcome(&mut self, outcome: ProjectSearchOutcome) {
        match outcome {
            ProjectSearchOutcome::Continue => {}
            ProjectSearchOutcome::Close => self.hide_project_search(),
            ProjectSearchOutcome::Navigate(found) => {
                self.hide_project_search();
                self.go_to_match(&found);
            }
        }
    }

    /// Select a project search match in its file, opening the file if it
    /// isn't open. The match is placed by line and column, so if the file
    /// has changed since the search the selection lands where the match
    /// was rather than on it.
    fn go_to_match(&mut self, found: &ProjectMatch) {
        self.open_file(&found.path);
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        if tab.path() != Some(&*found.path) {
            return; // it couldn't be opened; the status bar says why
        }
        let buffer = tab.view.editor().buffer();
        let line = found.line.min(buffer.line_count().saturating_sub(1));
        let content = buffer.line_content_range(line);
        let start = (content.start + found.column).min(content.end);
        let end = (start + found.len).min(content.end);
        tab.view.select_and_center(start, end);
    }

    /// Periodic housekeeping while idle. Returns whether the screen needs
    /// redrawing.
    pub fn tick(&mut self) -> bool {
        let mut redraw = false;
        if self.project_search_open
            && let Some(dialog) = &mut self.project_search
            && dialog.poll()
        {
            redraw = true;
        }
        if let Some(tab) = self.tabs.get_mut(self.active) {
            let generation = tab.view.editor().highlight_generation();
            if generation != tab.highlight_generation {
                tab.highlight_generation = generation;
                redraw = true;
            }
            let generation = tab.view.editor().search().map_or(0, |s| s.generation());
            if generation != tab.search_generation {
                tab.search_generation = generation;
                redraw = true;
            }
        }
        let generation = self.project.index().generation();
        if generation == self.index_generation {
            return redraw;
        }
        self.index_generation = generation;
        if self.palette_is_files {
            if let Some(mut palette) = self.palette.take() {
                palette.set_items(self.file_items());
                self.refresh_index_hint(&mut palette);
                self.palette = Some(palette);
            }
            return true;
        }
        redraw
    }

    // ----- Events ---------------------------------------------------------

    pub fn handle_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.handle_key(key),
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            Event::Paste(text) => {
                if let Some(palette) = &mut self.palette {
                    palette.paste(&text);
                } else if let Some(search_box) = &mut self.search_box {
                    if search_box.paste(&text) {
                        self.update_search_query();
                    }
                } else if self.project_search_open
                    && let Some(dialog) = &mut self.project_search
                {
                    dialog.paste(&text);
                } else if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.view.editor_mut().paste(&text);
                }
            }
            _ => {}
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        self.status = None;
        let confirm = self.confirm.take();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // Global bindings.
        if ctrl {
            match key.code {
                KeyCode::Char('q') => {
                    self.confirm = confirm;
                    self.request_quit();
                    return;
                }
                KeyCode::Char('t') => {
                    self.open_tabs_palette();
                    return;
                }
                KeyCode::Char('o') => {
                    self.open_files_palette();
                    return;
                }
                // With the kitty keyboard protocol Ctrl+Shift+F arrives as
                // a shifted 'F' (or as 'f' with the shift modifier); a
                // plain terminal can't tell it from Ctrl+F.
                KeyCode::Char('F') => {
                    self.open_project_search();
                    return;
                }
                KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.open_project_search();
                    return;
                }
                KeyCode::Char('f') => {
                    self.open_search_box();
                    return;
                }
                _ => {}
            }
        }

        if let Some(palette) = &mut self.palette {
            match palette.handle_key(key, &mut self.clipboard) {
                PaletteOutcome::Continue => {}
                PaletteOutcome::Close => self.palette = None,
                PaletteOutcome::Activate(action) => self.run_palette_action(action),
            }
            return;
        }
        if let Some(search_box) = &mut self.search_box {
            let outcome = search_box.handle_key(key, &mut self.clipboard);
            self.handle_search_outcome(outcome);
            return;
        }
        if self.project_search_open
            && let Some(dialog) = &mut self.project_search
        {
            let outcome = dialog.handle_key(key, &mut self.clipboard);
            self.handle_project_search_outcome(outcome);
            return;
        }

        if ctrl {
            match key.code {
                KeyCode::Char('w') => {
                    self.confirm = confirm;
                    self.request_close_tab(self.active);
                    return;
                }
                KeyCode::Char('s') => {
                    self.save_active();
                    return;
                }
                KeyCode::Char('g') => {
                    self.find_next();
                    return;
                }
                _ => {}
            }
        }

        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.view.handle_key(key, &mut self.clipboard);
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        let (x, y) = (mouse.column, mouse.row);

        // A drag that started in the palette's or the search box's query
        // stays with it wherever the pointer goes.
        if let Some(palette) = &mut self.palette {
            if palette.contains(x, y) || palette.is_dragging() {
                match palette.handle_mouse(mouse) {
                    PaletteOutcome::Continue => {}
                    PaletteOutcome::Close => self.palette = None,
                    PaletteOutcome::Activate(action) => self.run_palette_action(action),
                }
            } else if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.palette = None;
            }
            return;
        }
        // The project search dialog works like the palette.
        if self.project_search_open
            && let Some(dialog) = &mut self.project_search
        {
            if dialog.contains(x, y) || dialog.is_dragging() {
                let outcome = dialog.handle_mouse(mouse);
                self.handle_project_search_outcome(outcome);
            } else if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.hide_project_search();
            }
            return;
        }
        // A click outside the search box closes it, like the palette, but
        // the wheel still scrolls the editor under it so the matches can
        // be looked over.
        if let Some(search_box) = &mut self.search_box {
            if search_box.contains(x, y) || search_box.is_dragging() {
                search_box.handle_mouse(mouse);
                return;
            }
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.close_search_box(true);
                return;
            }
        }

        // A drag that started in the editor stays with it wherever the
        // pointer goes, so selections can extend past the edge of the view.
        if let Some(tab) = self.tabs.get_mut(self.active)
            && tab.view.is_dragging()
            && matches!(mouse.kind, MouseEventKind::Drag(_) | MouseEventKind::Up(_))
        {
            tab.view.handle_mouse(mouse);
            return;
        }

        if let Some(hit) = self.tab_bar.hit(x, y) {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                self.confirm = None;
                match hit {
                    TabHit::Tab(index) => self.activate(index),
                    TabHit::Close(index) => self.request_close_tab(index),
                }
            }
            return;
        }

        if let Some(tab) = self.tabs.get_mut(self.active)
            && tab.view.contains(x, y)
        {
            tab.view.handle_mouse(mouse);
        }
    }

    // ----- Rendering ------------------------------------------------------

    pub fn render(&mut self, frame: &mut Frame) {
        let screen = frame.area();
        if screen.height < 2 {
            return;
        }
        let tab_area = Rect::new(screen.x, screen.y, screen.width, 1);
        let status_area = Rect::new(screen.x, screen.bottom() - 1, screen.width, 1);
        self.editor_area = Rect::new(screen.x, screen.y + 1, screen.width, screen.height - 2);
        let buf = frame.buffer_mut();
        let theme = &self.theme;

        let labels: Vec<TabLabel> = self
            .tabs
            .iter()
            .map(|tab| TabLabel {
                title: tab.title(),
                modified: tab.view.editor().is_modified(),
            })
            .collect();
        self.tab_bar
            .render(tab_area, buf, theme, &labels, self.active);

        let mut cursor = None;
        match self.tabs.get_mut(self.active) {
            Some(tab) => {
                // The search box, drawn later, sits over the top of the
                // editor, and the editor is showing search matches.
                let covered = if self.search_box.is_some() {
                    search_box::HEIGHT
                } else {
                    0
                };
                tab.view.set_covered_rows(covered);
                cursor = tab.view.render(self.editor_area, buf, theme);
            }
            None => render_empty(self.editor_area, buf, theme),
        }

        self.render_status(status_area, buf);

        if let Some(palette) = &mut self.palette {
            cursor = palette.render(screen, buf, theme);
        }
        let hint = self.search_hint();
        if let Some(search_box) = &mut self.search_box {
            search_box.set_hint(hint);
            cursor = search_box.render(screen, buf, theme);
        }
        if self.project_search_open
            && let Some(dialog) = &mut self.project_search
        {
            cursor = dialog.render(screen, buf, theme);
        }
        if let Some(cursor) = cursor {
            frame.set_cursor_position(cursor);
        }
    }

    fn render_status(&self, area: Rect, buf: &mut Buffer) {
        let theme = &self.theme;
        let base = Style::default().bg(theme.status_bar_background);
        buf.set_style(area, base);
        let tab = self.tabs.get(self.active);
        let position = tab.map(|tab| {
            let position = tab.view.editor().cursor_position();
            format!(" Ln {}, Col {} ", position.line + 1, position.column + 1)
        });
        let mut project = format!(" {} ", self.project.name());
        if let Some(position) = &position {
            // Drop the project name when there's no room for it.
            if Span::raw(position).width() as u16 + Span::raw(&project).width() as u16 + 20
                > area.width
            {
                project.clear();
            }
        }
        let right = [
            (position.unwrap_or_default(), theme.status_bar_position_text),
            (project, theme.status_bar_project_text),
        ];
        let left = match &self.status {
            Some(message) => message.clone(),
            None => match tab.and_then(Tab::path) {
                Some(path) => self.display_path(path),
                None => "Ctrl+O to open a file, Ctrl+Q to quit".to_owned(),
            },
        };
        let right_width: u16 = right
            .iter()
            .map(|(text, _)| Span::raw(text).width() as u16)
            .sum();
        let left_width = area.width.saturating_sub(right_width + 1) as usize;
        buf.set_stringn(
            area.x + 1,
            area.y,
            &left,
            left_width,
            base.fg(theme.status_bar_filename_text),
        );
        if right_width <= area.width {
            let mut x = area.right() - right_width;
            for (text, color) in &right {
                buf.set_string(x, area.y, text, base.fg(*color));
                x += Span::raw(text).width() as u16;
            }
        }
    }
}

fn render_empty(area: Rect, buf: &mut Buffer, theme: &Theme) {
    buf.set_style(area, Style::default().bg(theme.view_background));
    if area.height == 0 {
        return;
    }
    let lines = [
        "No files open",
        "",
        "Ctrl+O         open a project file",
        "Ctrl+Shift+F   search in project files",
        "Ctrl+T         switch between open tabs",
        "Ctrl+Q         quit",
    ];
    let top = area.y + area.height.saturating_sub(lines.len() as u16) / 2;
    for (i, line) in lines.iter().enumerate() {
        let y = top + i as u16;
        if y >= area.bottom() {
            break;
        }
        let width = Span::raw(*line).width() as u16;
        let x = area.x + area.width.saturating_sub(width) / 2;
        buf.set_stringn(
            x,
            y,
            line,
            area.width as usize,
            Style::default()
                .fg(theme.view_text)
                .bg(theme.view_background)
                .add_modifier(Modifier::DIM),
        );
    }
}

/// The directory part of a displayed path, or empty for a bare file name.
fn parent_of(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|p| p.display().to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    fn app_with_files(files: &[(&str, &str)]) -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        for (name, contents) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        let mut app = App::new(Project::open(dir.path()).unwrap());
        for (name, _) in files {
            app.open_file(dir.path().join(name));
        }
        (dir, app)
    }

    fn draw(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn theme_colors_reach_the_screen() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "hello\n")]);
        let theme = Theme::parse(
            "view-background = \"#010203\"\nstatus-bar-background = \"#040506\"\n\
             active-tab-text = \"#070809\"\nstatus-bar-position-text = \"#0a0b0c\"\n",
        )
        .unwrap();
        app.set_theme(theme);
        let mut terminal = Terminal::new(TestBackend::new(40, 5)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        // Tab bar: the active tab's title.
        assert_eq!(buffer[(1, 0)].symbol(), "a");
        assert_eq!(buffer[(1, 0)].fg, Color::Rgb(7, 8, 9));
        // Editor text and the blank area past it share the background.
        assert_eq!(buffer[(3, 1)].bg, Color::Rgb(1, 2, 3));
        assert_eq!(buffer[(20, 3)].bg, Color::Rgb(1, 2, 3));
        // Status bar: the row and the cursor position on its right.
        assert_eq!(buffer[(0, 4)].bg, Color::Rgb(4, 5, 6));
        let row: String = (0..40).map(|x| buffer[(x, 4)].symbol()).collect();
        let ln = row.find("Ln 1").unwrap() as u16;
        assert_eq!(buffer[(ln, 4)].fg, Color::Rgb(10, 11, 12));
        assert_eq!(buffer[(ln, 4)].bg, Color::Rgb(4, 5, 6));
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_event(key(code, KeyModifiers::NONE));
    }

    fn ctrl(app: &mut App, c: char) {
        app.handle_event(key(KeyCode::Char(c), KeyModifiers::CONTROL));
    }

    fn click(app: &mut App, column: u16, row: u16) {
        app.handle_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }));
    }

    #[test]
    fn renders_tabs_gutter_scrollbar_and_status() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "one\ntwo\nthree\n")]);
        let screen = draw(&mut app, 30, 6);
        assert_eq!(screen[0].trim_end(), "◢a.txt ×◣");
        // Gutter: breakpoint column, line number, space, guide.
        assert_eq!(screen[1], " 1 │one                      █");
        assert_eq!(screen[2], " 2 │two                      █");
        assert_eq!(screen[3], " 3 │three                    █");
        assert_eq!(screen[4], " 4 │                         █");
        assert_eq!(screen[5], " a.txt            Ln 1, Col 1 ");
    }

    #[test]
    fn vertical_scrollbar_tracks_scrolling() {
        let text: String = (1..=40).map(|i| format!("line {i}\n")).collect();
        let (_dir, mut app) = app_with_files(&[("a.txt", &text)]);
        let screen = draw(&mut app, 20, 12);
        let bar: String = screen[1..11]
            .iter()
            .map(|r| r.chars().last().unwrap())
            .collect();
        assert_eq!(bar, "██││││││││", "{screen:#?}");
        app.handle_event(key(KeyCode::End, KeyModifiers::CONTROL));
        let screen = draw(&mut app, 20, 12);
        let bar: String = screen[1..11]
            .iter()
            .map(|r| r.chars().last().unwrap())
            .collect();
        assert_eq!(bar, "││││││││██", "{screen:#?}");
        assert!(screen[10].starts_with(" 41 "), "{screen:#?}");
        // Wheel scrolling moves the view without moving the cursor.
        app.handle_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        }));
        let screen = draw(&mut app, 20, 12);
        assert!(screen[1].starts_with(" 29 "), "{screen:#?}");
        assert!(screen[11].contains("Ln 41"), "{screen:#?}");
    }

    #[test]
    fn paging_scrolls_the_view_with_one_line_of_overlap() {
        let text: String = (1..=40).map(|i| format!("line {i}\n")).collect();
        let (_dir, mut app) = app_with_files(&[("a.txt", &text)]);
        // Ten text rows, so a page is nine lines.
        draw(&mut app, 20, 12);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::PageDown);
        let screen = draw(&mut app, 20, 12);
        // The old bottom line (10) is now at the top, and the cursor stayed
        // on its row: line 3 was on row 3, line 12 is on row 3.
        assert!(screen[1].starts_with(" 10 "), "{screen:#?}");
        assert!(screen[11].contains("Ln 12"), "{screen:#?}");
        press(&mut app, KeyCode::PageDown);
        let screen = draw(&mut app, 20, 12);
        assert!(screen[1].starts_with(" 19 "), "{screen:#?}");
        assert!(screen[11].contains("Ln 21"), "{screen:#?}");
        // Near the end the view stops at the last page while the cursor
        // keeps moving a full page.
        press(&mut app, KeyCode::PageDown);
        press(&mut app, KeyCode::PageDown);
        let screen = draw(&mut app, 20, 12);
        assert!(screen[1].starts_with(" 32 "), "{screen:#?}");
        assert!(screen[11].contains("Ln 39"), "{screen:#?}");
        // Paging back up scrolls the old top line to the bottom, cursor row
        // unchanged again.
        press(&mut app, KeyCode::PageUp);
        let screen = draw(&mut app, 20, 12);
        assert!(screen[1].starts_with(" 23 "), "{screen:#?}");
        assert!(screen[10].starts_with(" 32 "), "{screen:#?}");
        assert!(screen[11].contains("Ln 30"), "{screen:#?}");
        // Shift+PageUp extends the selection while scrolling.
        app.handle_event(key(KeyCode::PageUp, KeyModifiers::SHIFT));
        draw(&mut app, 20, 12);
        assert!(app.tabs[0].view.editor().selection().is_some());
    }

    #[test]
    fn horizontal_scrollbar_appears_only_when_needed() {
        let long = "x".repeat(50);
        let (_dir, mut app) = app_with_files(&[("a.txt", &format!("short\n{long}\nshort\n"))]);
        let screen = draw(&mut app, 20, 6);
        assert!(screen[4].contains('─'), "scrollbar row: {:?}", screen[4]);

        // Scroll the long line out of view and the scrollbar goes away.
        for _ in 0..8 {
            press(&mut app, KeyCode::Down);
        }
        let screen = draw(&mut app, 20, 4);
        assert!(!screen.iter().any(|row| row.contains('─')), "{screen:#?}");
    }

    #[test]
    fn horizontal_scrolling_stops_past_the_longest_visible_line() {
        // A long line among short ones; the view is 14 text columns wide.
        let long = "x".repeat(50);
        let short: String = "short\n".repeat(30);
        let (_dir, mut app) = app_with_files(&[("a.txt", &format!("short\n{long}\n{short}"))]);
        draw(&mut app, 20, 6);
        let wheel = |app: &mut App, kind| {
            app.handle_event(Event::Mouse(MouseEvent {
                kind,
                column: 8,
                row: 2,
                modifiers: KeyModifiers::NONE,
            }));
        };
        // Scrolling far right stops with the end of the long line and two
        // spare columns in view: 50 + 2 - 14.
        for _ in 0..20 {
            wheel(&mut app, MouseEventKind::ScrollRight);
        }
        let screen = draw(&mut app, 20, 6);
        assert_eq!(app.tabs[0].view.scroll_col(), 38);
        assert_eq!(screen[2], "  2 │xxxxxxxxxxxx  │");
        // Scrolling down to where only short lines are visible keeps the
        // horizontal position rather than snapping back...
        wheel(&mut app, MouseEventKind::ScrollDown);
        draw(&mut app, 20, 6);
        assert_eq!(app.tabs[0].view.scroll_col(), 38);
        // ...but it can't go any further right, only left.
        wheel(&mut app, MouseEventKind::ScrollRight);
        assert_eq!(app.tabs[0].view.scroll_col(), 38);
        wheel(&mut app, MouseEventKind::ScrollLeft);
        assert_eq!(app.tabs[0].view.scroll_col(), 34);
        wheel(&mut app, MouseEventKind::ScrollRight);
        assert_eq!(app.tabs[0].view.scroll_col(), 34);
        // Back at the top, the short first line doesn't limit anything
        // until the long line scrolls out of view, and even a burst of
        // vertical and horizontal motion between redraws respects the
        // lines visible at the time.
        wheel(&mut app, MouseEventKind::ScrollUp);
        wheel(&mut app, MouseEventKind::ScrollRight);
        wheel(&mut app, MouseEventKind::ScrollRight);
        assert_eq!(app.tabs[0].view.scroll_col(), 38);
        // Clicking the far end of the scrollbar track can't get past the
        // limit either; the bar's range ends at the limit.
        wheel(&mut app, MouseEventKind::ScrollLeft);
        wheel(&mut app, MouseEventKind::ScrollLeft);
        let screen = draw(&mut app, 20, 6);
        assert!(screen[4].contains('─'), "{screen:#?}");
        app.handle_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 18,
            row: 4,
            modifiers: KeyModifiers::NONE,
        }));
        let col = app.tabs[0].view.scroll_col();
        assert!(col > 30 && col <= 38, "scroll_col {col}");
        // Short lines only: no limit to scroll into from the left edge.
        for _ in 0..10 {
            wheel(&mut app, MouseEventKind::ScrollLeft);
        }
        wheel(&mut app, MouseEventKind::ScrollDown);
        draw(&mut app, 20, 6);
        assert_eq!(app.tabs[0].view.scroll_col(), 0);
        wheel(&mut app, MouseEventKind::ScrollRight);
        assert_eq!(app.tabs[0].view.scroll_col(), 0);
    }

    #[test]
    fn paging_through_long_lines_keeps_the_full_height() {
        // Every line overflows a 30-column view, so the horizontal scrollbar
        // is always needed. Paging down must not shrink the editor.
        let text: String = (1..=200)
            .map(|i| format!("line {i} {}\n", "x".repeat(60)))
            .collect();
        let (_dir, mut app) = app_with_files(&[("a.txt", &text)]);
        draw(&mut app, 30, 20);
        for _ in 0..6 {
            press(&mut app, KeyCode::PageDown);
            let screen = draw(&mut app, 30, 20);
            // Rows 1..=17 are text, row 18 the horizontal scrollbar.
            for row in &screen[1..18] {
                assert!(row.starts_with(" ") && row.contains("line "), "{screen:#?}");
                assert!(row.ends_with('█') || row.ends_with('│'), "{screen:#?}");
            }
            assert!(
                screen[18].contains('─') || screen[18].contains('█'),
                "{screen:#?}"
            );
            assert!(!screen[18].contains("line "), "{screen:#?}");
        }
        let position = app.tabs[0].view.editor().cursor_position();
        assert!(
            position.line > 90,
            "paging should have moved far: {position:?}"
        );
    }

    #[test]
    fn keyboard_navigation_and_selection() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "hello world\nsecond\n")]);
        app.handle_event(key(KeyCode::Right, KeyModifiers::CONTROL));
        app.handle_event(key(
            KeyCode::Right,
            KeyModifiers::SHIFT | KeyModifiers::CONTROL,
        ));
        let editor = app.tabs[0].view.editor();
        assert_eq!(editor.selected_text().as_deref(), Some(" world"));
        assert!(draw(&mut app, 30, 5)[4].contains("Ln 1, Col 12"));

        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.tabs[0].view.editor().cursor_position().line, 1);
        app.handle_event(key(KeyCode::Home, KeyModifiers::CONTROL));
        assert_eq!(app.tabs[0].view.editor().cursor(), 0);
        app.handle_event(key(KeyCode::End, KeyModifiers::CONTROL));
        assert_eq!(app.tabs[0].view.editor().cursor(), 19);
    }

    #[test]
    fn cursor_is_hidden_while_selecting() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "hello\n")]);
        let mut terminal = Terminal::new(TestBackend::new(30, 5)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        assert!(terminal.backend().cursor_visible());
        app.handle_event(key(KeyCode::Right, KeyModifiers::SHIFT));
        terminal.draw(|frame| app.render(frame)).unwrap();
        assert!(!terminal.backend().cursor_visible());
        press(&mut app, KeyCode::Esc);
        terminal.draw(|frame| app.render(frame)).unwrap();
        assert!(terminal.backend().cursor_visible());
    }

    #[test]
    fn mouse_click_moves_cursor_and_switches_tabs() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "abc\ndef\n"), ("b.txt", "x\n")]);
        assert_eq!(app.active, 1);
        draw(&mut app, 30, 6);
        // Tab bar: " a.txt ×  ◢b.txt ×◣"
        click(&mut app, 1, 0);
        assert_eq!(app.active, 0);
        draw(&mut app, 30, 6);
        // Gutter is 4 wide: text starts at column 4. Row 2 is line 2.
        click(&mut app, 6, 2);
        let position = app.tabs[0].view.editor().cursor_position();
        assert_eq!((position.line, position.column), (1, 2));
        // The close button on the second tab.
        click(&mut app, 17, 0);
        assert_eq!(app.tabs.len(), 1);
    }

    #[test]
    fn selection_uses_theme_colors() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "hello\n")]);
        app.handle_event(key(KeyCode::Right, KeyModifiers::SHIFT));
        app.handle_event(key(KeyCode::Right, KeyModifiers::SHIFT));
        let cell = |app: &mut App, x: u16| {
            let mut terminal = Terminal::new(TestBackend::new(30, 4)).unwrap();
            terminal.draw(|frame| app.render(frame)).unwrap();
            terminal.backend().buffer()[(x, 1)].clone()
        };
        // Gutter is 4 wide, so "he" is selected at columns 4 and 5.
        let theme = Theme::default();
        let selected = cell(&mut app, 5);
        assert_eq!(selected.symbol(), "e");
        assert_eq!(selected.bg, theme.selection_background);
        assert_eq!(selected.fg, theme.view_text);
        let plain = cell(&mut app, 6);
        assert_eq!(plain.symbol(), "l");
        assert_eq!(plain.bg, theme.view_background);

        // An explicit selection text color takes over.
        app.set_theme(Theme::parse("selection-text = \"#010203\"").unwrap());
        let selected = cell(&mut app, 5);
        assert_eq!(selected.fg, Color::Rgb(1, 2, 3));
        assert_eq!(selected.bg, theme.selection_background);
    }

    #[test]
    fn syntax_highlighting_reaches_the_screen() {
        let (_dir, mut app) = app_with_files(&[("a.rs", "fn main() {} // hi\n")]);
        app.set_theme(
            Theme::parse(
                "syntax-keyword = \"*#010203\"\nsyntax-function-definition = \"#040506\"\n\
                 syntax-comment = \"/#070809\"\n",
            )
            .unwrap(),
        );
        let mut terminal = Terminal::new(TestBackend::new(30, 4)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        // Gutter is 4 wide, ending in the guide.
        let guide = &buffer[(3, 1)];
        assert_eq!(guide.symbol(), "│");
        assert_eq!(guide.fg, Theme::default().gutter_guide);
        let keyword = &buffer[(4, 1)];
        assert_eq!(keyword.symbol(), "f");
        assert_eq!(keyword.fg, Color::Rgb(1, 2, 3));
        assert!(keyword.modifier.contains(Modifier::BOLD));
        let name = &buffer[(7, 1)];
        assert_eq!(name.symbol(), "m");
        assert_eq!(name.fg, Color::Rgb(4, 5, 6));
        assert!(!name.modifier.contains(Modifier::BOLD));
        let comment = &buffer[(17, 1)];
        assert_eq!(comment.symbol(), "/");
        assert_eq!(comment.fg, Color::Rgb(7, 8, 9));
        assert!(comment.modifier.contains(Modifier::ITALIC));
        let space = &buffer[(6, 1)];
        assert_eq!(space.fg, Theme::default().view_text);

        // Selected text keeps its syntax color and modifiers unless the
        // theme forces a selection text color.
        app.handle_event(key(KeyCode::Right, KeyModifiers::SHIFT));
        terminal.draw(|frame| app.render(frame)).unwrap();
        let selected = &terminal.backend().buffer()[(4, 1)];
        assert_eq!(selected.fg, Color::Rgb(1, 2, 3));
        assert!(selected.modifier.contains(Modifier::BOLD));
        assert_eq!(selected.bg, Theme::default().selection_background);
    }

    #[test]
    fn active_tab_has_sloped_edges() {
        let (_dir, mut app) = app_with_files(&[("a.txt", ""), ("b.txt", ""), ("c.txt", "")]);
        app.activate(1);
        let screen = draw(&mut app, 40, 4);
        // Separators only stand between two inactive tabs.
        assert_eq!(screen[0].trim_end(), " a.txt ×  ◢b.txt ×◣  c.txt ×");
        app.activate(2);
        let screen = draw(&mut app, 40, 4);
        assert_eq!(screen[0].trim_end(), " a.txt ×   b.txt ×  ◢c.txt ×◣");

        // The edges are the tab's background drawn over the bar's.
        let theme = Theme::default();
        let mut terminal = Terminal::new(TestBackend::new(40, 4)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let row: String = (0..40).map(|x| buffer[(x, 0)].symbol()).collect();
        // Every glyph in the bar is one column wide.
        let left = row.chars().position(|c| c == '◢').unwrap() as u16;
        let right = row.chars().position(|c| c == '◣').unwrap() as u16;
        for x in [left, right] {
            assert_eq!(buffer[(x, 0)].fg, theme.active_tab_background);
            assert_eq!(buffer[(x, 0)].bg, theme.inactive_tab_background);
        }
        assert_eq!(buffer[(left + 1, 0)].bg, theme.active_tab_background);
        assert_eq!(buffer[(left + 1, 0)].fg, theme.active_tab_text);
    }

    #[test]
    fn tab_palette_lists_most_recently_viewed_first() {
        // Paths of different lengths, so ranking by length rather than by
        // recency would show.
        let (_dir, mut app) =
            app_with_files(&[("a.rs", ""), ("bb/b.rs", ""), ("longer/name/c.rs", "")]);
        // Viewed in order a, b, c on open; now view a again.
        app.activate(0);
        ctrl(&mut app, 't');
        let screen = draw(&mut app, 40, 12);
        assert!(screen[3].contains("a.rs"), "{screen:#?}");
        assert!(screen[4].contains("c.rs"), "{screen:#?}");
        assert!(screen[5].contains("b.rs"), "{screen:#?}");
        // The previously viewed tab is preselected, so Enter flips back.
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.active, 2);
        ctrl(&mut app, 't');
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.active, 0);
        // Closing the active tab returns to the most recently viewed one.
        ctrl(&mut app, 'w');
        assert_eq!(app.tabs[app.active].title(), "c.rs");
    }

    #[test]
    fn palette_searches_tabs_and_files() {
        let (_dir, mut app) =
            app_with_files(&[("alpha.rs", ""), ("beta.rs", ""), ("gamma.rs", "")]);
        assert_eq!(app.active, 2);
        ctrl(&mut app, 't');
        let screen = draw(&mut app, 40, 12);
        assert!(screen[2].contains(TABS_PLACEHOLDER), "{screen:#?}");
        for c in "bet".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        let screen = draw(&mut app, 40, 12);
        assert!(screen[3].contains("beta.rs"), "{screen:#?}");
        assert!(
            !screen[1..].iter().any(|r| r.contains("alpha.rs")),
            "{screen:#?}"
        );
        press(&mut app, KeyCode::Enter);
        assert!(app.palette.is_none());
        assert_eq!(app.active, 1);

        // Close everything, then reopen a file through the file search.
        ctrl(&mut app, 'w');
        ctrl(&mut app, 'w');
        ctrl(&mut app, 'w');
        assert!(app.tabs.is_empty());
        app.project
            .index()
            .wait_for_primary(std::time::Duration::from_secs(10));
        ctrl(&mut app, 'o');
        for c in "gam".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].title(), "gamma.rs");
        press(&mut app, KeyCode::Esc);
        assert!(app.palette.is_none());
    }

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    /// Wait for the active tab's search to finish, since it runs in the
    /// background.
    fn wait_for_search(app: &App) {
        if let Some(search) = app.tabs[app.active].view.editor().search() {
            search.wait();
        }
    }

    fn cell_bg(app: &mut App, width: u16, height: u16, x: u16, y: u16) -> Color {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        terminal.backend().buffer()[(x, y)].bg
    }

    #[test]
    fn search_previews_selects_and_steps_through_matches() {
        // Three blank lines keep the text clear of the search box, which
        // covers rows 1 to 3. The gutter is 4 wide.
        let (_dir, mut app) = app_with_files(&[("a.txt", "\n\n\nfoo bar\nbaz foo\nfoo\n")]);
        let theme = Theme::default();
        ctrl(&mut app, 'f');
        let screen = draw(&mut app, 60, 10);
        assert!(screen[2].contains("Search in file"), "{screen:#?}");
        type_str(&mut app, "foo");
        wait_for_search(&app);
        let screen = draw(&mut app, 60, 10);
        assert!(screen[3].contains("3 matches"), "{screen:#?}");
        // Previewing: the first match from the cursor is highlighted, the
        // others marked, and the cursor hasn't moved.
        assert_eq!(
            cell_bg(&mut app, 60, 10, 4, 4),
            theme.highlighted_find_result_background
        );
        assert_eq!(
            cell_bg(&mut app, 60, 10, 6, 4),
            theme.highlighted_find_result_background
        );
        assert_eq!(cell_bg(&mut app, 60, 10, 7, 4), theme.view_background);
        assert_eq!(
            cell_bg(&mut app, 60, 10, 8, 5),
            theme.find_result_background
        );
        assert_eq!(
            cell_bg(&mut app, 60, 10, 4, 6),
            theme.find_result_background
        );
        assert!(screen[9].contains("Ln 1, Col 1"), "{screen:#?}");
        assert!(app.tabs[0].view.editor().selection().is_none());

        // Enter selects it and closes the box; the rest stay marked.
        press(&mut app, KeyCode::Enter);
        assert!(app.search_box.is_none());
        let editor = app.tabs[0].view.editor();
        assert_eq!(editor.selection(), Some(3..6));
        assert!(editor.search().is_some());
        assert_eq!(cell_bg(&mut app, 60, 10, 4, 4), theme.selection_background);
        assert_eq!(
            cell_bg(&mut app, 60, 10, 8, 5),
            theme.find_result_background
        );
        assert_eq!(
            cell_bg(&mut app, 60, 10, 4, 6),
            theme.find_result_background
        );

        // Ctrl+G steps on, wrapping with a message before going around.
        ctrl(&mut app, 'g');
        assert_eq!(app.tabs[0].view.editor().selection(), Some(15..18));
        ctrl(&mut app, 'g');
        assert_eq!(app.tabs[0].view.editor().selection(), Some(19..22));
        assert!(app.status.is_none());
        ctrl(&mut app, 'g');
        assert_eq!(app.tabs[0].view.editor().selection(), Some(19..22));
        assert!(app.status.as_deref().unwrap().contains("wrapped"));
        ctrl(&mut app, 'g');
        assert_eq!(app.tabs[0].view.editor().selection(), Some(3..6));
        assert!(app.status.is_none());

        // Any other movement drops the highlights.
        press(&mut app, KeyCode::Right);
        assert!(app.tabs[0].view.editor().search().is_none());
        assert_eq!(cell_bg(&mut app, 60, 10, 8, 5), theme.view_background);

        // Ctrl+G with no search going repeats the last query from the
        // cursor, which is now at the end of the first match.
        ctrl(&mut app, 'g');
        assert_eq!(app.tabs[0].view.editor().selection(), Some(15..18));
        assert_eq!(
            cell_bg(&mut app, 60, 10, 4, 6),
            theme.find_result_background
        );
    }

    #[test]
    fn search_starts_at_the_cursor_and_reveals_the_match() {
        let text: String = (1..=60)
            .map(|i| {
                if i % 20 == 10 {
                    format!("needle {i}\n")
                } else {
                    format!("line {i}\n")
                }
            })
            .collect();
        let (_dir, mut app) = app_with_files(&[("a.txt", &text)]);
        draw(&mut app, 30, 12);
        // Cursor onto line 12, past the first needle.
        for _ in 0..11 {
            press(&mut app, KeyCode::Down);
        }
        draw(&mut app, 30, 12);
        ctrl(&mut app, 'f');
        type_str(&mut app, "needle");
        wait_for_search(&app);
        let screen = draw(&mut app, 30, 12);
        // The view scrolled to the first match after the cursor, on line
        // 30, centered in the rows the box leaves clear, while the cursor
        // stayed on line 12.
        assert_eq!(screen[7], " 30 │needle 30               │", "{screen:#?}");
        assert!(screen[11].contains("Ln 12"), "{screen:#?}");
        assert!(screen[3].contains("3 matches"), "{screen:#?}");
        // Ctrl+G in the box previews the next one.
        ctrl(&mut app, 'g');
        let screen = draw(&mut app, 30, 12);
        assert!(
            screen[1..11].iter().any(|r| r.contains("needle 50")),
            "{screen:#?}"
        );
        assert!(screen[11].contains("Ln 12"), "{screen:#?}");
        ctrl(&mut app, 'g');
        let screen = draw(&mut app, 30, 12);
        assert_eq!(screen[7], " 10 │needle 10               │", "{screen:#?}");
        // Enter selects the previewed match.
        press(&mut app, KeyCode::Enter);
        let editor = app.tabs[0].view.editor();
        assert_eq!(editor.selected_text().as_deref(), Some("needle"));
        assert_eq!(editor.cursor_position().line, 9);
    }

    #[test]
    fn stepping_between_matches_scrolls_sideways_as_little_as_possible() {
        // The view is 30 wide with a 4-column gutter and a scrollbar: 25
        // text columns. Three blank lines keep the text clear of the
        // search box.
        let x = "x".repeat(50);
        let z = "z".repeat(10);
        let w = "w".repeat(40);
        let text = format!("\n\n\n{x}needle\nneedle {x}\n{z}{w}\n");
        let (_dir, mut app) = app_with_files(&[("a.txt", &text)]);
        draw(&mut app, 30, 10);
        ctrl(&mut app, 'f');
        type_str(&mut app, "needle");
        wait_for_search(&app);
        // The first match ends at column 56: scrolled just enough to fit.
        let screen = draw(&mut app, 30, 10);
        assert_eq!(app.tabs[0].view.scroll_col(), 31);
        assert_eq!(screen[4], " 4 │xxxxxxxxxxxxxxxxxxxneedle█", "{screen:#?}");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tabs[0].view.editor().selection(), Some(53..59));
        // The next match is at the start of its line, so the view scrolls
        // all the way back rather than leaving its start off the left.
        ctrl(&mut app, 'g');
        let screen = draw(&mut app, 30, 10);
        assert_eq!(app.tabs[0].view.scroll_col(), 0);
        assert_eq!(app.tabs[0].view.editor().selection(), Some(60..66));
        assert!(screen[5].starts_with(" 5 │needle xxx"), "{screen:#?}");
        // Back to the first: it doesn't fit from the margin, so the view
        // scrolls right again.
        ctrl(&mut app, 'g');
        ctrl(&mut app, 'g');
        draw(&mut app, 30, 10);
        assert_eq!(app.tabs[0].view.scroll_col(), 31);
        // A match already in view leaves the scroll alone.
        press(&mut app, KeyCode::Esc);
        for _ in 0..6 {
            press(&mut app, KeyCode::Left);
        }
        draw(&mut app, 30, 10);
        assert_eq!(app.tabs[0].view.editor().cursor(), 53);
        ctrl(&mut app, 'f');
        type_str(&mut app, "eedle");
        wait_for_search(&app);
        draw(&mut app, 30, 10);
        assert_eq!(
            app.tabs[0].view.editor().search().unwrap().current(),
            Some(54..59)
        );
        assert_eq!(app.tabs[0].view.scroll_col(), 31);
        // A match wider than the view starts at the left edge.
        press(&mut app, KeyCode::Esc);
        ctrl(&mut app, 'f');
        type_str(&mut app, "/w+");
        wait_for_search(&app);
        let screen = draw(&mut app, 30, 10);
        assert_eq!(app.tabs[0].view.scroll_col(), 10);
        assert_eq!(screen[6], " 6 │wwwwwwwwwwwwwwwwwwwwwwwww█", "{screen:#?}");
    }

    #[test]
    fn escape_clears_the_search() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "foo foo\n")]);
        let theme = Theme::default();
        ctrl(&mut app, 'f');
        type_str(&mut app, "foo");
        wait_for_search(&app);
        assert!(app.tabs[0].view.editor().search().is_some());
        press(&mut app, KeyCode::Esc);
        assert!(app.search_box.is_none());
        assert!(app.tabs[0].view.editor().search().is_none());
        assert_eq!(cell_bg(&mut app, 30, 4, 4, 1), theme.view_background);

        // After Enter too: Escape drops both the selection and the marks.
        ctrl(&mut app, 'f');
        type_str(&mut app, "foo");
        press(&mut app, KeyCode::Enter);
        assert!(app.tabs[0].view.editor().selection().is_some());
        press(&mut app, KeyCode::Esc);
        let editor = app.tabs[0].view.editor();
        assert!(editor.selection().is_none());
        assert!(editor.search().is_none());

        // Typing after Enter drops the marks as well.
        ctrl(&mut app, 'f');
        type_str(&mut app, "foo");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('x'));
        assert!(app.tabs[0].view.editor().search().is_none());
        assert_eq!(app.tabs[0].view.editor().buffer().to_text(), "foo x\n");

        // A click outside the box closes it and clears the search; the
        // palette replaces it.
        ctrl(&mut app, 'f');
        type_str(&mut app, "foo");
        click(&mut app, 5, 6);
        assert!(app.search_box.is_none());
        assert!(app.tabs[0].view.editor().search().is_none());
        ctrl(&mut app, 'f');
        ctrl(&mut app, 't');
        assert!(app.search_box.is_none() && app.palette.is_some());
        assert!(app.tabs[0].view.editor().search().is_none());
        ctrl(&mut app, 'f');
        assert!(app.search_box.is_some() && app.palette.is_none());
    }

    #[test]
    fn regex_search_and_hints() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "\n\n\nfoo fooo f\n")]);
        ctrl(&mut app, 'f');
        type_str(&mut app, "/fo+");
        wait_for_search(&app);
        let screen = draw(&mut app, 60, 8);
        assert!(screen[3].contains("regex, 2 matches"), "{screen:#?}");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tabs[0].view.editor().selection(), Some(3..6));
        ctrl(&mut app, 'g');
        assert_eq!(app.tabs[0].view.editor().selection(), Some(7..11));

        press(&mut app, KeyCode::Right);
        ctrl(&mut app, 'f');
        type_str(&mut app, "/(");
        let screen = draw(&mut app, 60, 8);
        assert!(
            screen[3].contains("invalid regex: unclosed group"),
            "{screen:#?}"
        );
        // Enter with nothing to select keeps the box open.
        press(&mut app, KeyCode::Enter);
        assert!(app.search_box.is_some());
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Backspace);
        type_str(&mut app, "zzz");
        wait_for_search(&app);
        let screen = draw(&mut app, 60, 8);
        assert!(screen[3].contains("no matches"), "{screen:#?}");
        press(&mut app, KeyCode::Enter);
        assert!(app.search_box.is_some());
        // The plain text form of a pattern is literal.
        ctrl(&mut app, 'u');
        type_str(&mut app, "fo+");
        wait_for_search(&app);
        let screen = draw(&mut app, 60, 8);
        assert!(screen[3].contains("no matches"), "{screen:#?}");
    }

    fn mouse(app: &mut App, kind: MouseEventKind, column: u16, row: u16) {
        app.handle_event(Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }));
    }

    #[test]
    fn search_starts_with_a_single_line_selection() {
        // Three blank lines keep the text clear of the search box. The
        // gutter is 4 wide.
        let (_dir, mut app) = app_with_files(&[("a.txt", "\n\n\nfoo bar\nbaz foo\nfoo\n")]);
        let theme = Theme::default();
        draw(&mut app, 60, 10);
        // Double-click "foo" on line 5 (row 5, columns 8 to 10).
        click(&mut app, 9, 5);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 9, 5);
        click(&mut app, 9, 5);
        assert_eq!(app.tabs[0].view.editor().selection(), Some(15..18));
        // Ctrl+F searches for it right away, with the query selected.
        ctrl(&mut app, 'f');
        let search_box = app.search_box.as_ref().unwrap();
        assert_eq!(search_box.query(), "foo");
        assert_eq!(search_box.input().edit().selection(), Some(0..3));
        wait_for_search(&app);
        let screen = draw(&mut app, 60, 10);
        assert!(screen[3].contains("3 matches"), "{screen:#?}");
        // The selected occurrence is the current match (still drawn as
        // the selection, which wins over a match), so Enter keeps it and
        // Ctrl+G goes on from there.
        let editor = app.tabs[0].view.editor();
        assert_eq!(editor.search().unwrap().current(), Some(15..18));
        assert_eq!(editor.selection(), Some(15..18));
        assert_eq!(cell_bg(&mut app, 60, 10, 8, 5), theme.selection_background);
        assert_eq!(
            cell_bg(&mut app, 60, 10, 4, 6),
            theme.find_result_background
        );
        press(&mut app, KeyCode::Enter);
        assert!(app.search_box.is_none());
        assert_eq!(app.tabs[0].view.editor().selection(), Some(15..18));
        ctrl(&mut app, 'g');
        assert_eq!(app.tabs[0].view.editor().selection(), Some(19..22));

        // Typing replaces the seeded query rather than adding to it.
        ctrl(&mut app, 'f');
        assert_eq!(app.search_box.as_ref().unwrap().query(), "foo");
        type_str(&mut app, "ba");
        assert_eq!(app.search_box.as_ref().unwrap().query(), "ba");
        wait_for_search(&app);
        let screen = draw(&mut app, 60, 10);
        assert!(screen[3].contains("2 matches"), "{screen:#?}");
        press(&mut app, KeyCode::Esc);

        // A selection spanning lines doesn't seed the box.
        app.tabs[0].view.editor_mut().set_selection(4, 12);
        ctrl(&mut app, 'f');
        assert_eq!(app.search_box.as_ref().unwrap().query(), "");
        press(&mut app, KeyCode::Esc);
        // Nor does no selection.
        ctrl(&mut app, 'f');
        assert_eq!(app.search_box.as_ref().unwrap().query(), "");
        press(&mut app, KeyCode::Esc);
    }

    #[test]
    fn search_seeded_with_a_leading_slash_stays_literal() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "\n\n\n/a.b /axb /a.b\n")]);
        app.tabs[0].view.editor_mut().set_selection(3, 7);
        ctrl(&mut app, 'f');
        assert_eq!(app.search_box.as_ref().unwrap().query(), "//a\\.b");
        wait_for_search(&app);
        let screen = draw(&mut app, 60, 10);
        assert!(screen[3].contains("regex, 2 matches"), "{screen:#?}");
    }

    #[test]
    fn search_query_edits_like_a_text_field() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "\n\n\nfoo bar\n")]);
        ctrl(&mut app, 'f');
        type_str(&mut app, "bar");
        press(&mut app, KeyCode::Home);
        type_str(&mut app, "foo ");
        assert_eq!(app.search_box.as_ref().unwrap().query(), "foo bar");
        wait_for_search(&app);
        let screen = draw(&mut app, 60, 10);
        assert!(screen[3].contains("1 match"), "{screen:#?}");
        // Select "foo " with Shift+Home and cut it; paste it back at the
        // end.
        press(&mut app, KeyCode::End);
        app.handle_event(key(KeyCode::Left, KeyModifiers::CONTROL));
        app.handle_event(key(KeyCode::Home, KeyModifiers::SHIFT));
        ctrl(&mut app, 'x');
        assert_eq!(app.search_box.as_ref().unwrap().query(), "bar");
        press(&mut app, KeyCode::End);
        ctrl(&mut app, 'v');
        assert_eq!(app.search_box.as_ref().unwrap().query(), "barfoo ");
        wait_for_search(&app);
        let screen = draw(&mut app, 60, 10);
        assert!(screen[3].contains("no matches"), "{screen:#?}");

        // The mouse places the cursor and drags a selection in the query,
        // even when the drag leaves the box; a click outside closes it.
        // The box spans columns 1 to 58 on rows 1 to 3; the query starts
        // at column 4.
        click(&mut app, 5, 2);
        assert_eq!(app.search_box.as_ref().unwrap().input().edit().cursor(), 1);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 7, 2);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 7, 6);
        assert_eq!(
            app.search_box.as_ref().unwrap().input().edit().selection(),
            Some(1..3)
        );
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 7, 6);
        assert!(app.search_box.is_some());
        type_str(&mut app, "o");
        assert_eq!(app.search_box.as_ref().unwrap().query(), "bofoo ");
        click(&mut app, 7, 6);
        assert!(app.search_box.is_none());
    }

    #[test]
    fn palette_query_edits_and_pastes() {
        let (_dir, mut app) = app_with_files(&[("alpha.rs", ""), ("beta.rs", "")]);
        ctrl(&mut app, 't');
        app.handle_event(Event::Paste("et\n".to_owned()));
        press(&mut app, KeyCode::Home);
        type_str(&mut app, "b");
        let screen = draw(&mut app, 40, 12);
        assert!(screen[2].contains("> bet"), "{screen:#?}");
        assert!(screen[3].contains("beta.rs"), "{screen:#?}");
        assert!(
            !screen[1..].iter().any(|r| r.contains("alpha.rs")),
            "{screen:#?}"
        );
        // Ctrl+Home still picks the first result; Ctrl+A selects the query
        // and typing replaces it.
        app.handle_event(key(KeyCode::Home, KeyModifiers::CONTROL));
        ctrl(&mut app, 'a');
        type_str(&mut app, "alp");
        let screen = draw(&mut app, 40, 12);
        assert!(screen[2].contains("> alp"), "{screen:#?}");
        assert!(screen[3].contains("alpha.rs"), "{screen:#?}");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.active, 0);
    }

    /// An app on a project with these files, none of them open.
    fn project_with_files(files: &[(&str, &str)]) -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        for (name, contents) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        let app = App::new(Project::open(dir.path()).unwrap());
        (dir, app)
    }

    fn ctrl_shift_f(app: &mut App) {
        app.handle_event(key(
            KeyCode::Char('F'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
    }

    /// Wait for the project search to finish, since it runs in the
    /// background.
    fn wait_for_project_search(app: &App) {
        app.project_search.as_ref().unwrap().wait();
    }

    /// The screen column `needle` starts at in a drawn row.
    fn column_of(row: &str, needle: &str) -> u16 {
        let index = row.find(needle).unwrap();
        row[..index].chars().count() as u16
    }

    fn cell(app: &mut App, width: u16, height: u16, x: u16, y: u16) -> ratatui::buffer::Cell {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        terminal.backend().buffer()[(x, y)].clone()
    }

    #[test]
    fn project_search_lists_matches_and_walks_through_them() {
        let (_dir, mut app) = project_with_files(&[
            ("a.rs", "fn alpha() {}\nlet needle = 1;\n"),
            ("b/c.rs", "needle\nx\nneedle again\n"),
        ]);
        let theme = Theme::default();
        assert!(app.tabs.is_empty());
        ctrl_shift_f(&mut app);
        assert!(app.project_search_open);
        // On a 60x20 screen the dialog spans rows 1 to 18: the query on
        // row 2, seven result rows from row 3, a rule on row 10, and
        // seven context rows from row 11.
        let screen = draw(&mut app, 60, 20);
        assert!(screen[2].contains("Search in project"), "{screen:#?}");
        assert!(screen[3].contains("Type to search"), "{screen:#?}");
        type_str(&mut app, "needle");
        wait_for_project_search(&app);
        let screen = draw(&mut app, 60, 20);
        assert!(screen[3].contains("let needle = 1;"), "{screen:#?}");
        assert!(screen[3].trim_end().ends_with("a.rs:2│"), "{screen:#?}");
        assert!(screen[4].contains("needle"), "{screen:#?}");
        assert!(screen[4].trim_end().ends_with("b/c.rs:1│"), "{screen:#?}");
        assert!(screen[5].contains("needle again"), "{screen:#?}");
        assert!(screen[5].trim_end().ends_with("b/c.rs:3│"), "{screen:#?}");
        assert!(screen[10].starts_with("  ├"), "{screen:#?}");
        assert!(screen[11].contains("1 │fn alpha() {}"), "{screen:#?}");
        assert!(screen[12].contains("2 │let needle = 1;"), "{screen:#?}");
        assert!(screen[18].contains("3 matches in 2 files"), "{screen:#?}");
        // The first row is highlighted; the location of the second is in
        // its own color.
        assert_eq!(
            cell(&mut app, 60, 20, 10, 3).bg,
            theme.command_palette_selection_background
        );
        let location = column_of(&screen[4], "b/c.rs");
        assert_eq!(
            cell(&mut app, 60, 20, location, 4).fg,
            theme.project_search_location_text
        );
        // The match itself is bold, in the find result color.
        let x = column_of(&screen[4], "needle");
        let found = cell(&mut app, 60, 20, x, 4);
        assert!(found.modifier.contains(Modifier::BOLD));
        assert_eq!(found.bg, theme.find_result_background);

        // Enter opens the file and selects the match.
        press(&mut app, KeyCode::Enter);
        assert!(!app.project_search_open);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[app.active].title(), "a.rs");
        assert_eq!(app.tabs[0].view.editor().selection(), Some(18..24));
        assert_eq!(
            app.tabs[0].view.editor().selected_text().as_deref(),
            Some("needle")
        );

        // Reopened, the dialog is as it was: the match just visited is
        // still the highlighted one, so Down and Enter go to the next.
        ctrl_shift_f(&mut app);
        assert_eq!(app.project_search.as_ref().unwrap().query(), "needle");
        let screen = draw(&mut app, 60, 20);
        assert!(screen[3].contains("let needle = 1;"), "{screen:#?}");
        press(&mut app, KeyCode::Down);
        assert_eq!(
            cell(&mut app, 60, 20, 10, 4).bg,
            theme.command_palette_selection_background
        );
        let screen = draw(&mut app, 60, 20);
        assert!(screen[11].contains("1 │needle"), "{screen:#?}");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.tabs[app.active].title(), "c.rs");
        assert_eq!(app.tabs[app.active].view.editor().selection(), Some(0..6));
        ctrl_shift_f(&mut app);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.tabs[app.active].title(), "c.rs");
        assert_eq!(app.tabs[app.active].view.editor().selection(), Some(9..15));
        // Past the last match, Down stays put.
        ctrl_shift_f(&mut app);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tabs[app.active].view.editor().selection(), Some(9..15));

        // A different selection seeds a new search, with the query
        // selected so typing replaces it.
        app.tabs[app.active].view.editor_mut().set_selection(16, 21);
        ctrl_shift_f(&mut app);
        let dialog = app.project_search.as_ref().unwrap();
        assert_eq!(dialog.query(), "again");
        assert_eq!(dialog.input().edit().selection(), Some(0..5));
        wait_for_project_search(&app);
        let screen = draw(&mut app, 60, 20);
        assert!(screen[18].contains("1 match"), "{screen:#?}");
        assert!(!screen[4].contains("needle"), "{screen:#?}");
        type_str(&mut app, "zzz");
        assert_eq!(app.project_search.as_ref().unwrap().query(), "zzz");
        wait_for_project_search(&app);
        let screen = draw(&mut app, 60, 20);
        assert!(screen[3].contains("No matches"), "{screen:#?}");
        assert!(screen[18].contains("no matches"), "{screen:#?}");

        // Escape hides the dialog; Ctrl+F replaces it with the search
        // box; a click outside hides it too.
        press(&mut app, KeyCode::Esc);
        assert!(!app.project_search_open);
        ctrl_shift_f(&mut app);
        ctrl(&mut app, 'f');
        assert!(!app.project_search_open && app.search_box.is_some());
        ctrl_shift_f(&mut app);
        assert!(app.project_search_open && app.search_box.is_none());
        draw(&mut app, 60, 20);
        click(&mut app, 0, 5);
        assert!(!app.project_search_open);
    }

    #[test]
    fn reopened_project_search_has_its_query_selected() {
        let (_dir, mut app) =
            project_with_files(&[("a.rs", "needle\n"), ("b.rs", "needle\nother\n")]);
        ctrl_shift_f(&mut app);
        type_str(&mut app, "needle");
        wait_for_project_search(&app);
        assert_eq!(
            app.project_search
                .as_ref()
                .unwrap()
                .input()
                .edit()
                .selection(),
            None
        );
        press(&mut app, KeyCode::Enter);
        // Reopened after a jump: the state is kept, the query selected,
        // and the results untouched until something is typed.
        ctrl_shift_f(&mut app);
        let dialog = app.project_search.as_ref().unwrap();
        assert_eq!(dialog.query(), "needle");
        assert_eq!(dialog.input().edit().selection(), Some(0..6));
        assert_eq!(dialog.selected().unwrap().line, 0);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tabs[app.active].title(), "b.rs");
        // Typing over the selected query starts a new search.
        ctrl_shift_f(&mut app);
        type_str(&mut app, "other");
        assert_eq!(app.project_search.as_ref().unwrap().query(), "other");
        wait_for_project_search(&app);
        let screen = draw(&mut app, 60, 20);
        assert!(screen[18].contains("1 match"), "{screen:#?}");
        // Reopened with nothing selected in the editor, and after Escape,
        // the query is selected too.
        press(&mut app, KeyCode::Esc);
        app.tabs[app.active].view.editor_mut().clear_selection();
        ctrl_shift_f(&mut app);
        assert_eq!(
            app.project_search
                .as_ref()
                .unwrap()
                .input()
                .edit()
                .selection(),
            Some(0..5)
        );
    }

    #[test]
    fn project_search_context_pane_scrolls_through_the_file() {
        let text: String = (1..=30)
            .map(|i| {
                if i == 20 {
                    "needle here\n".to_owned()
                } else {
                    format!("line {i}\n")
                }
            })
            .collect();
        let (_dir, mut app) = project_with_files(&[("long.txt", &text)]);
        ctrl_shift_f(&mut app);
        type_str(&mut app, "needle");
        wait_for_project_search(&app);
        // Seven context rows, the match centered: lines 17 to 23.
        let screen = draw(&mut app, 60, 20);
        assert!(screen[11].contains("17 │line 17"), "{screen:#?}");
        assert!(screen[14].contains("20 │needle here"), "{screen:#?}");
        assert!(screen[17].contains("23 │line 23"), "{screen:#?}");
        app.handle_event(key(KeyCode::Down, KeyModifiers::CONTROL));
        let screen = draw(&mut app, 60, 20);
        assert!(screen[11].contains("18 │line 18"), "{screen:#?}");
        app.handle_event(key(KeyCode::PageUp, KeyModifiers::CONTROL));
        let screen = draw(&mut app, 60, 20);
        assert!(screen[11].contains("11 │line 11"), "{screen:#?}");
        // The wheel over the pane scrolls it too, and stops at the ends.
        mouse(&mut app, MouseEventKind::ScrollDown, 20, 14);
        let screen = draw(&mut app, 60, 20);
        assert!(screen[11].contains("14 │line 14"), "{screen:#?}");
        for _ in 0..20 {
            mouse(&mut app, MouseEventKind::ScrollDown, 20, 14);
        }
        let screen = draw(&mut app, 60, 20);
        assert!(screen[11].contains("24 │line 24"), "{screen:#?}");
        assert!(screen[17].contains("30 │line 30"), "{screen:#?}");
        // Closing and reopening keeps the scroll position.
        press(&mut app, KeyCode::Esc);
        ctrl_shift_f(&mut app);
        let screen = draw(&mut app, 60, 20);
        assert!(screen[11].contains("24 │line 24"), "{screen:#?}");
        // Enter selects the match and centers it in the editor.
        press(&mut app, KeyCode::Enter);
        let screen = draw(&mut app, 60, 20);
        assert_eq!(
            app.tabs[0].view.editor().selected_text().as_deref(),
            Some("needle")
        );
        assert!(screen[10].contains("20 │needle here"), "{screen:#?}");
    }

    #[test]
    fn project_search_context_pane_keeps_indentation_and_scrolls_sideways() {
        // The context pane's text is 51 columns wide on a 60-column
        // screen with a one-digit gutter. The match on line 2 sits past
        // column 60; line 3 is the longest.
        let text = format!(
            "fn a() {{\n    let x = {}needle {};\n    {}\n}}\n",
            "-".repeat(50),
            "+".repeat(10),
            "=".repeat(100)
        );
        let (_dir, mut app) = project_with_files(&[("a.rs", &text)]);
        ctrl_shift_f(&mut app);
        type_str(&mut app, "needle");
        wait_for_project_search(&app);
        // Scrolled just far enough to show the end of the match, every
        // line by the same amount, the indentation intact.
        let screen = draw(&mut app, 60, 20);
        // The text after the gutter's guide (the dialog's border is the
        // first bar on the row).
        let text = |row: &str| row.splitn(3, '│').nth(2).unwrap().to_owned();
        assert_eq!(
            text(&screen[11]),
            "                                                   │  "
        );
        assert!(
            text(&screen[12]).starts_with("-------------------"),
            "{screen:#?}"
        );
        assert!(screen[12].contains("-needle│"), "{screen:#?}");
        assert!(text(&screen[13]).starts_with("=========="), "{screen:#?}");
        assert!(text(&screen[14]).starts_with(' '), "{screen:#?}");
        // The wheel scrolls left, back to the indentation...
        for _ in 0..30 {
            mouse(&mut app, MouseEventKind::ScrollLeft, 20, 12);
        }
        let screen = draw(&mut app, 60, 20);
        assert!(screen[11].contains("1 │fn a() {"), "{screen:#?}");
        assert!(screen[12].contains("2 │    let x = ---"), "{screen:#?}");
        assert!(screen[13].contains("3 │    ====="), "{screen:#?}");
        assert!(screen[14].contains("4 │}"), "{screen:#?}");
        // ...and right, as far as the longest visible line and two spare
        // columns: 104 + 2 - 51.
        for _ in 0..40 {
            mouse(&mut app, MouseEventKind::ScrollRight, 20, 12);
        }
        assert_eq!(app.project_search.as_ref().unwrap().context_col(), 55);
        let screen = draw(&mut app, 60, 20);
        assert!(screen[13].trim_end().ends_with("=  │"), "{screen:#?}");
        // Closing and reopening keeps the position.
        press(&mut app, KeyCode::Esc);
        ctrl_shift_f(&mut app);
        draw(&mut app, 60, 20);
        assert_eq!(app.project_search.as_ref().unwrap().context_col(), 55);
    }

    #[test]
    fn project_search_highlights_syntax_in_rows_and_preview() {
        let (_dir, mut app) = project_with_files(&[
            ("a.rs", "let x = 1;\nfn needle() {}\n"),
            ("b.txt", "needle\n"),
        ]);
        let theme = Theme::parse(
            "syntax-keyword = \"*#010203\"\nsearch-preview-background = \"#040506\"\n",
        )
        .unwrap();
        app.set_theme(theme);
        ctrl_shift_f(&mut app);
        type_str(&mut app, "needle");
        wait_for_project_search(&app);
        // Row 3 is a.rs's match (highlighted), row 4 b.txt's. Drawing
        // asks for a.rs to be lexed in the background.
        press(&mut app, KeyCode::Down);
        let screen = draw(&mut app, 60, 20);
        assert!(screen[3].contains("fn needle() {}"), "{screen:#?}");
        let x = column_of(&screen[3], "fn");
        app.project_search.as_ref().unwrap().wait_for_highlighting();
        // The tick notices and asks for a redraw, which colors the keyword.
        assert!(app.tick());
        let keyword = cell(&mut app, 60, 20, x, 3);
        assert_eq!(keyword.fg, Color::Rgb(1, 2, 3));
        assert!(keyword.modifier.contains(Modifier::BOLD));
        assert_eq!(keyword.bg, theme.command_palette_background);
        // The preview of b.txt, a file of no known language, is plain
        // text over the preview background, the match marked as in the
        // editor.
        let screen = draw(&mut app, 60, 20);
        assert!(screen[11].contains("1 │needle"), "{screen:#?}");
        let px = column_of(&screen[11], "needle");
        let plain = cell(&mut app, 60, 20, px, 11);
        assert_eq!(plain.fg, theme.command_palette_result_text);
        assert_eq!(plain.bg, theme.find_result_background);
        assert_eq!(cell(&mut app, 60, 20, px + 6, 11).bg, Color::Rgb(4, 5, 6));
        assert_eq!(cell(&mut app, 60, 20, 30, 16).bg, Color::Rgb(4, 5, 6));
        assert_eq!(
            cell(&mut app, 60, 20, 30, 8).bg,
            theme.command_palette_background
        );

        // The highlighted row keeps the selection colors; its preview,
        // of a.rs, is highlighted.
        press(&mut app, KeyCode::Up);
        let selected = cell(&mut app, 60, 20, x, 3);
        assert_eq!(selected.fg, theme.command_palette_selection_text);
        assert_eq!(selected.bg, theme.command_palette_selection_background);
        let screen = draw(&mut app, 60, 20);
        assert!(screen[12].contains("2 │fn needle() {}"), "{screen:#?}");
        let px = column_of(&screen[12], "fn");
        let keyword = cell(&mut app, 60, 20, px, 12);
        assert_eq!(keyword.fg, Color::Rgb(1, 2, 3));
        assert!(keyword.modifier.contains(Modifier::BOLD));
        assert_eq!(keyword.bg, Color::Rgb(4, 5, 6));
        // The match on that line keeps its find color under the syntax
        // color.
        let found = cell(&mut app, 60, 20, px + 3, 12);
        assert_eq!(found.bg, theme.find_result_background);
        assert!(found.modifier.contains(Modifier::BOLD));

        // Without the optional color the preview shares the palette's
        // background.
        app.set_theme(Theme::parse("search-preview-background = \"\"\n").unwrap());
        assert_eq!(
            cell(&mut app, 60, 20, 30, 16).bg,
            theme.command_palette_background
        );

        // Hiding the dialog drops the highlighting cache; showing it
        // again makes a new one, and the keyword colors once more.
        app.set_theme(theme);
        press(&mut app, KeyCode::Esc);
        assert!(!app.project_search.as_ref().unwrap().has_highlights());
        ctrl_shift_f(&mut app);
        draw(&mut app, 60, 20);
        assert!(app.project_search.as_ref().unwrap().has_highlights());
        app.project_search.as_ref().unwrap().wait_for_highlighting();
        assert!(app.tick());
        let keyword = cell(&mut app, 60, 20, px, 12);
        assert_eq!(keyword.fg, Color::Rgb(1, 2, 3));
    }

    #[test]
    fn project_search_rows_keep_the_match_in_view() {
        // The rows are 54 wide: 51 columns to share. The long line and
        // the long path both overflow, so the path gets a third, cut from
        // the left; the short line leaves its long path all it needs.
        let long = format!("{} needle {}", "x".repeat(80), "y".repeat(80));
        let deep = format!("{}/f.rs", "z".repeat(38));
        let (_dir, mut app) =
            project_with_files(&[("deep/dir/name/f.rs", &long), (&deep, "needle\n")]);
        ctrl_shift_f(&mut app);
        type_str(&mut app, "needle");
        wait_for_project_search(&app);
        let screen = draw(&mut app, 60, 20);
        let row = screen[3].trim_end();
        assert!(row.contains("…") && row.contains("needle"), "{row:?}");
        assert!(row.ends_with("yyy…  …/dir/name/f.rs:1│"), "{row:?}");
        assert!(
            !row.contains("xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"),
            "{row:?}"
        );
        let row = screen[4].trim_end();
        assert!(row.ends_with(&format!("{deep}:1│")), "{row:?}");
        assert!(row.contains(" needle "), "{row:?}");
    }

    #[test]
    fn closing_and_quitting_confirm_unsaved_changes() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "abc\n")]);
        press(&mut app, KeyCode::Char('z'));
        ctrl(&mut app, 'w');
        assert_eq!(app.tabs.len(), 1);
        assert!(app.status.as_deref().unwrap().contains("unsaved"));
        ctrl(&mut app, 'w');
        assert!(app.tabs.is_empty());

        app.open_file(_dir.path().join("a.txt"));
        press(&mut app, KeyCode::Char('z'));
        ctrl(&mut app, 'q');
        assert!(!app.should_quit());
        ctrl(&mut app, 'q');
        assert!(app.should_quit());
    }
}
