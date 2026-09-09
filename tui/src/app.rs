//! Application state: the project, the open tabs, the command palette, and
//! the status bar, plus the routing of events between them.
//!
//! Layout, top to bottom: the tab bar, the editor for the active tab (or a
//! hint when nothing is open), and the status bar. The command palette,
//! when open, floats over the top of the editor and takes all keyboard
//! input until it is closed.
//!
//! Closing a modified tab or quitting with unsaved changes asks for the key
//! to be pressed a second time rather than popping up a dialog.

use crate::editor_view::EditorView;
use crate::palette::{Palette, PaletteAction, PaletteItem, PaletteOutcome};
use crate::tabs::{TabBar, TabHit, TabLabel};
use crate::theme::Theme;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ninjaedit_core::{Editor, Project};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use std::path::Path;

const TABS_PLACEHOLDER: &str = "Search open tabs";
const FILES_PLACEHOLDER: &str = "Search files in project";

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
    index_generation: u64,
    /// A message shown in the status bar until the next key press.
    status: Option<String>,
    confirm: Option<Confirm>,
    clipboard: Option<String>,
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
            status: None,
            confirm: None,
            clipboard: None,
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
                self.tabs.push(Tab {
                    view: EditorView::new(Editor::new(buffer)),
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

    /// Periodic housekeeping while idle. Returns whether the screen needs
    /// redrawing.
    pub fn tick(&mut self) -> bool {
        let generation = self.project.index().generation();
        if generation == self.index_generation {
            return false;
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
        false
    }

    // ----- Events ---------------------------------------------------------

    pub fn handle_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.handle_key(key),
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            Event::Paste(text) if self.palette.is_none() => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
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
                _ => {}
            }
        }

        if let Some(palette) = &mut self.palette {
            match palette.handle_key(key) {
                PaletteOutcome::Continue => {}
                PaletteOutcome::Close => self.palette = None,
                PaletteOutcome::Activate(action) => self.run_palette_action(action),
            }
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
                _ => {}
            }
        }

        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.view.handle_key(key, &mut self.clipboard);
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        let (x, y) = (mouse.column, mouse.row);

        if let Some(palette) = &mut self.palette {
            if palette.contains(x, y) {
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
            Some(tab) => cursor = tab.view.render(self.editor_area, buf, theme),
            None => render_empty(self.editor_area, buf, theme),
        }

        self.render_status(status_area, buf);

        if let Some(palette) = &mut self.palette {
            cursor = palette.render(screen, buf, theme);
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
        "Ctrl+O   open a project file",
        "Ctrl+T   switch between open tabs",
        "Ctrl+Q   quit",
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
            std::fs::write(dir.path().join(name), contents).unwrap();
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
        assert_eq!(screen[1], " 1 one                       █");
        assert_eq!(screen[2], " 2 two                       █");
        assert_eq!(screen[3], " 3 three                     █");
        assert_eq!(screen[4], " 4                           █");
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
        // Gutter is 3 wide: text starts at column 3. Row 2 is line 2.
        click(&mut app, 5, 2);
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
        // Gutter is 3 wide, so "he" is selected at columns 3 and 4.
        let theme = Theme::default();
        let selected = cell(&mut app, 4);
        assert_eq!(selected.symbol(), "e");
        assert_eq!(selected.bg, theme.selection_background);
        assert_eq!(selected.fg, theme.view_text);
        let plain = cell(&mut app, 5);
        assert_eq!(plain.symbol(), "l");
        assert_eq!(plain.bg, theme.view_background);

        // An explicit selection text color takes over.
        app.set_theme(Theme::parse("selection-text = \"#010203\"").unwrap());
        let selected = cell(&mut app, 4);
        assert_eq!(selected.fg, Color::Rgb(1, 2, 3));
        assert_eq!(selected.bg, theme.selection_background);
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
        assert_eq!(screen[0].trim_end(), " a.txt × │ b.txt ×  ◢c.txt ×◣");

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
        let (_dir, mut app) = app_with_files(&[("a.rs", ""), ("b.rs", ""), ("c.rs", "")]);
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
