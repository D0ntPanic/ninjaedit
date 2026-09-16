//! Application state: the project, the open tabs, the command palette, the
//! settings, and the status bar, plus the routing of events between them.
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
//! Not every terminal can tell Ctrl+Shift+F from Ctrl+F, so Ctrl+F with
//! the search box already open opens the project search instead, taking
//! over a query typed into the box. With nothing typed (the box empty,
//! or still holding the seeded selection) Ctrl+F, Ctrl+F is exactly
//! Ctrl+Shift+F. With no file open there is nothing to search locally,
//! so Ctrl+F opens the project search right away.
//!
//! The go to line box (Ctrl+J) floats where the search box does, and
//! like it never shows at the same time as the palette. Enter moves the
//! active tab's cursor to the start of the line typed, clamped to the
//! first or last line when the number is out of range, and closes the box.
//!
//! The command palette (Ctrl+P) lists every command (see the `command`
//! module) and every view, with the key each is bound to, so it is
//! both a way to run what has no key and the place to learn the keys
//! that do. The commands run from it as their keys would; a view is
//! shown and focused as the modes palette would. Whatever it last ran
//! is listed first the next time it opens, so Ctrl+P, Enter repeats it:
//! the way to step through a file's merge conflicts with "Next
//! conflict", which has no key of its own. Typing a query ranks the
//! entries as usual.
//!
//! The modes palette (Ctrl+E) lists the editor, the settings page, and
//! every tool (the shell today; builds, debuggers and the like later) in
//! the same palette the tab and file searches use. Picking one shows it
//! and gives it the keyboard, starting a tool if it isn't running. Unlike
//! the other palettes it opens from the shell as well as from the editor,
//! and while it is open it has the keyboard whichever view is focused;
//! the view it is listing first (the current one) keeps the focus if it
//! is closed without a choice. The rest follow most recently used first,
//! so the view before this one is preselected and Ctrl+E, Enter goes
//! back to it, whichever way the user came.
//!
//! The settings page, the build configuration page, and the git log
//! page are the editor's modes: views that stand in for the editor and
//! its tab bar in the upper part of the screen (the tool pane, if
//! showing, stays below). The git log (Ctrl+L, from the editor or after
//! the prefix from a tool; see the `git_view` module) shows the
//! project's history: the branches, the graph of every commit, and what
//! the selected commit changed, with a tab for each submodule after
//! the main repository's. Left for the editor or another mode, the
//! page is kept as it was and Ctrl+L brings it back to the same
//! commit and file, reading the repository again in the background
//! (as Ctrl+L does while it is showing), since a commit may have been
//! made meanwhile; the fresh history takes the old one's place once it
//! is read, keeping the place wherever it still makes sense (see the
//! `git_view` module). On the page Ctrl+T picks one of its tabs rather
//! than a file's, since the editor isn't showing. How its panes are
//! sized, by dragging the rules between them, is kept per repository
//! in the project's storage (`tui-git-log.toml`, the TUI's own file;
//! see the `git_layout` module) and comes back next time. The changes page
//! (Ctrl+U; see the `changes_view` module) is the working tree's
//! uncommitted changes, unstaged and staged, with the diff of each,
//! for staging them (a file or a whole directory at once) and
//! committing them with Ctrl+S, amending the last commit if asked, and
//! for resolving a merge's conflicts: `o` on a file opens it in the
//! editor, at its first conflict. It scans the working tree when opened and after each of
//! its actions; Ctrl+U while it is showing scans again, since a file
//! saved in the editor or a `git` command in a shell may have changed
//! things. A submodule with changes of its own has a tab, as on the
//! git log, and its pane sizes are kept in `tui-git-changes.toml`. A
//! mode is left by choosing the editor in the modes palette, by closing
//! its tab, or by anything that brings a file to the front: opening one
//! with Ctrl+O, switching to one with Ctrl+T, going to a project search
//! match. Focusing a tool leaves the mode where it is above the pane.
//! The settings themselves live in `~/.ninjaedit/settings.toml` and are
//! saved whenever the page changes one, and applied at once to what is
//! running: a terminal's scrollback is trimmed, the project search takes
//! the new limit, the next shell to start is the one named, and the next
//! CMake build configures again if the generator changed.
//!
//! The build configuration (see the core crate's `build` module) lives
//! in the project's own storage directory, in `build.toml`, and is saved
//! whenever the page changes it or the current configuration or target
//! is changed. With no file yet, the project's top-level `Cargo.toml`
//! and `CMakeLists.txt` are found and set up with their defaults, and
//! every load looks at the project again for the targets it finds
//! there. The status bar shows the current configuration and target; clicking
//! either opens a palette to pick another. Ctrl+B builds the current
//! target with the current configuration and Ctrl+R builds and runs it,
//! from the editor, a mode, or (after the prefix, or straight away when
//! nothing is running in it) a tool. Each job runs in the output tool:
//! its commands one after another on one screen, with a heading before
//! each and a line saying how it ended. Either gives the output the
//! keyboard, so Ctrl+C stops the job (or the program) without switching
//! to it first, and once the job is over Ctrl+D dismisses the output the
//! way it ends a shell: the pane hides and the editor gets the keyboard
//! back. Nothing else dismisses it, so a program that ends sooner than
//! expected doesn't lose its output to the keys meant for it. Starting a
//! job while one is running stops the old one.
//! A CMake configuration's build directory is configured on its first
//! build of the run of the editor, and again when the options the
//! configure step depends on change. To start over, the command palette
//! has "Delete build directory" (the current configuration's) and
//! "Delete all build directories" (every root's); each runs as a job
//! in the output tool, so what it did and any failure are there to
//! read. Neither has a key: they are for debugging a build, not for a
//! slip of the fingers. The command palette also has "Run <target>"
//! for every target that isn't disabled: it builds and runs that one
//! with the configuration selecting it would bring (the current one in
//! its root, or that root's of the same name, or its first) and leaves
//! the current target and configuration alone, so a benchmark or a
//! test suite can be run now and then without losing the target being
//! worked on from Ctrl+R.
//!
//! With a tool focused its program gets the whole keyboard, since a shell
//! or a coding agent has uses for nearly every key and its line editing
//! (Ctrl+E for the end of the line, say) is muscle memory. The one key
//! the program never sees is the prefix, Ctrl+], telnet's escape
//! character: it holds the next key back, and that key goes to the
//! editor instead, meaning what it means there: Ctrl+] Ctrl+P opens the
//! command palette, Ctrl+E the modes palette, Ctrl+O the file search,
//! Ctrl+T the tab search, Ctrl+F
//! and Ctrl+Shift+F the searches, Ctrl+J the go to line box, Ctrl+L the
//! git log, Ctrl+U the changes page, Ctrl+Q
//! quits, and Ctrl+, and Ctrl+. move the focus. So a file named in a
//! build's output is a prefix and a few keys away without leaving the
//! shell first; the overlay takes the keyboard while it is open, and
//! whichever it ends in the editor (a file opened, a match or a line
//! gone to) moves the focus there, while closing it leaves the focus on
//! the shell. Ctrl+] Ctrl+] sends one Ctrl+] to the program, and any key
//! the editor has no use for after the prefix is sent along with the
//! prefix, so nothing is lost, only held for a keystroke. There is no
//! timeout; the status bar shows the prefix while it waits. The prefix
//! also takes the mouse from a program reading it: dragging in the
//! terminal after Ctrl+] selects text to copy, as it does without the
//! prefix when the program isn't reading the mouse (see the terminal
//! view); the press spends the prefix, as any click does. With nothing
//! running in the tool (the output tool between jobs) the editor's keys
//! work without the prefix, since there is no program to want them.
//! Terminals without the kitty keyboard protocol deliver Ctrl+] as
//! Ctrl+5, which is also how to type it on a layout that puts ] behind
//! AltGr, so both spellings are the prefix. Ctrl+` toggles the shell
//! without a prefix from either view: it only arrives at all on terminals
//! where it is unambiguous.
//!
//! Closing a modified tab or quitting with unsaved changes asks for the key
//! to be pressed a second time rather than popping up a dialog.

use crate::build_view::{self, BuildOutcome, BuildView, Node};
use crate::changes_view::{self, ChangesOutcome, ChangesTabs};
use crate::clipboard::Clipboard;
use crate::command::Command;
use crate::editor_view::EditorView;
use crate::git_layout;
use crate::git_view::{self, GitLogTabs};
use crate::goto_line::{GoToLineBox, GoToLineOutcome};
use crate::palette::{Palette, PaletteAction, PaletteItem, PaletteOutcome};
use crate::project_search::{ProjectSearchDialog, ProjectSearchOutcome};
use crate::search_box::{self, SearchBox, SearchOutcome};
use crate::settings_view::{self, SettingsOutcome, SettingsView};
use crate::tabs::{TabBar, TabHit, TabLabel};
use crate::theme::Theme;
use crate::tool::{Tool, ToolKind, ToolPane};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ninjaedit_core::build::root_candidates;
use ninjaedit_core::search::literal_query;
use ninjaedit_core::terminal::{ExitStatus, Output, Session, SessionId};
use ninjaedit_core::{
    BuildConfig, BuildRoot, ConflictStep, Discovery, DiscoveryResult, Editor, ExternalChange, Job,
    Project, ProjectKind, ProjectMatch, SearchStep, Selection, Settings, SourceLocation, Step,
    Storage,
};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// Everything the event loop reacts to: input from the terminal and the
/// output of programs running in tool panes. Both arrive on one channel so
/// the loop can wait on them together; see `main`.
pub enum AppEvent {
    /// A crossterm event: a key, a mouse action, a paste, a resize.
    Terminal(Event),
    /// Output (or the exit) of the program in a tool's session.
    Pty(SessionId, Output),
}

/// Which of the two on-screen views the keyboard drives: the upper one
/// (the editor, or the mode standing in for it) or the tool pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Editor,
    Tool,
}

/// What the upper part of the screen shows: the editor with its tabs,
/// or a mode standing in for it.
enum Mode {
    Editor,
    Settings(SettingsView),
    Build(BuildView),
    /// Boxed: the page is far bigger than the others.
    GitLog(Box<GitLogTabs>),
    Changes(Box<ChangesTabs>),
}

/// One of the places the keyboard can be, as the modes palette lists
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Editor,
    Settings,
    Build,
    GitLog,
    Changes,
    Tool(ToolKind),
}

impl View {
    /// Every view, in the order the modes palette lists the ones not
    /// used recently: the pages last, being the least often wanted.
    fn all() -> impl Iterator<Item = View> {
        std::iter::once(View::Editor)
            .chain(ToolKind::ALL.into_iter().map(View::Tool))
            .chain([View::GitLog, View::Changes, View::Build, View::Settings])
    }

    fn label(self) -> &'static str {
        match self {
            View::Editor => EDITOR_MODE_LABEL,
            View::Settings => settings_view::TITLE,
            View::Build => build_view::TITLE,
            View::GitLog => git_view::TITLE,
            View::Changes => changes_view::TITLE,
            View::Tool(kind) => kind.name(),
        }
    }

    fn description(self) -> &'static str {
        match self {
            View::Editor => "Edit the open files",
            View::Settings => "Change the editor's settings",
            View::Build => "Set up how the project is built and run",
            View::GitLog => {
                "Browse the project's git history: branches, commits, and their changes"
            }
            View::Changes => {
                "Review the uncommitted changes, stage them, and commit them; resolve merge conflicts"
            }
            View::Tool(kind) => kind.description(),
        }
    }

    /// The key that shows the view, if one does; the rest are reached
    /// through the modes palette.
    fn shortcut(self) -> Option<&'static str> {
        match self {
            View::Tool(ToolKind::Shell) => Some("Ctrl+`"),
            View::GitLog => Some("Ctrl+L"),
            View::Changes => Some("Ctrl+U"),
            _ => None,
        }
    }

    fn action(self) -> PaletteAction {
        match self {
            View::Editor => PaletteAction::FocusEditor,
            View::Settings => PaletteAction::OpenSettings,
            View::Build => PaletteAction::OpenBuildConfig,
            View::GitLog => PaletteAction::OpenGitLog,
            View::Changes => PaletteAction::OpenChanges,
            View::Tool(kind) => PaletteAction::OpenTool(kind),
        }
    }
}

/// The height of the bar dragged to resize the editor/tool split.
const DIVIDER_HEIGHT: u16 = 1;

/// A git page's tabs, for the tab palette: which is shown, their
/// titles, and the action that shows one by index.
type RepositoryTabs = (usize, Vec<String>, fn(usize) -> PaletteAction);

const TABS_PLACEHOLDER: &str = "Search open tabs";
const REPOSITORIES_PLACEHOLDER: &str = "The project's repository or a submodule";
const FILES_PLACEHOLDER: &str = "Search files in project";
const MODES_PLACEHOLDER: &str = "Editor, a page, or a tool";
const COMMANDS_PLACEHOLDER: &str = "Search commands and views";
/// The note in a palette that lists targets while some are still being
/// found.
const DISCOVERY_HINT: &str = "finding targets…";
/// How long a discovery thread waits for the file index before walking
/// the tree itself.
const DISCOVERY_INDEX_WAIT: Duration = Duration::from_secs(600);
const ADD_ROOT_PLACEHOLDER: &str = "Add a Cargo.toml or CMakeLists.txt as a build root";
const CONFIGURATION_PLACEHOLDER: &str = "Build configuration to use";
const TARGET_PLACEHOLDER: &str = "Target to build and run";
/// The modes palette's row for the editor.
const EDITOR_MODE_LABEL: &str = "Editor";
/// What the status bar shows on the settings page.
const SETTINGS_HINT: &str = "Tab/↑↓ next · Enter apply · Ctrl+D default · Ctrl+E leave";
/// What the status bar shows on the build configuration page.
const BUILD_HINT: &str =
    "↑↓ select · Enter options · Ctrl+N new · Ctrl+D duplicate · Del remove · Ctrl+E leave";
/// What the status bar shows with the output tool focused and idle.
const OUTPUT_IDLE_HINT: &str =
    "Ctrl+B build · Ctrl+R run · Ctrl+D dismiss · Ctrl+E mode · Ctrl+P commands";
/// What the status bar shows while the prefix waits for its key.
const PREFIX_HINT: &str = "Ctrl+]  then Ctrl+ P commands · E mode · O file · T tab · F search · J line · L log · B build · R run · Q quit · ] itself · drag to copy";
/// What the output tool shows before the first job.
const OUTPUT_WELCOME: &str = "Ctrl+B builds and Ctrl+R runs the current target\r\n";
/// How long a change to the search query waits for the search to finish,
/// so that in all but the largest files the matches show up in the same
/// frame as the keystroke rather than a tick later.
const SEARCH_GRACE: Duration = Duration::from_millis(15);
/// How long open files go unchecked for changes on disk when nothing
/// hints that they should be: the most a change made outside the
/// project's watched tree can take to show up.
const FILE_CHECK_INTERVAL: Duration = Duration::from_secs(2);
const SEARCH_WRAPPED: &str =
    "Search wrapped around to where it started; press Ctrl+G to go around again";

/// Which palette is open, as far as the application needs to know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaletteKind {
    /// The file search, which needs refreshing as the index fills in.
    Files,
    /// The command palette, which remembers what it ran, and lists a
    /// run entry per target, so it fills in as targets are found.
    Commands,
    /// The target search, which likewise fills in as targets are found.
    Targets,
    /// Tabs, modes, build roots, or configurations.
    Other,
}

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
    /// Where the settings (and the rest of what outlives a run) live.
    storage: Storage,
    /// Where the project's own state lives: its build configuration.
    project_storage: Storage,
    settings: Settings,
    /// How the project is built and run, and with what.
    build: BuildConfig,
    /// The CMake build directories configured during this run, with the
    /// signature of the configuration they were configured with, so a
    /// build configures only when that has changed.
    configured: HashMap<PathBuf, String>,
    /// The steps of the running job still to run, after the one running.
    pending_steps: Vec<Step>,
    /// The running job's title and how many of its steps have started.
    job_title: String,
    job_step: usize,
    /// The build directory and signature the running job's first step is
    /// configuring, to record once it succeeds.
    job_configure: Option<(PathBuf, String)>,
    /// The directories the last job's commands ran in (and built in),
    /// which the paths in their output are relative to.
    job_dirs: Vec<PathBuf>,
    /// The status bar's configuration and target segments from the last
    /// render, to hit-test clicks.
    status_config_area: Rect,
    status_target_area: Rect,
    /// What the threads finding roots' targets report back, taken in
    /// by [`tick`](Self::tick); see [`start_discoveries`](Self::start_discoveries).
    discoveries: Receiver<DiscoveryResult>,
    discovery_tx: Sender<DiscoveryResult>,
    theme: Theme,
    /// What the upper part of the screen shows.
    mode: Mode,
    /// The git log page while another mode or the editor is showing,
    /// kept as it was left so that Ctrl+L brings it back to the same
    /// place; see [`open_git_log`](Self::open_git_log).
    git_log: Option<Box<GitLogTabs>>,
    /// The tab bar drawn in place of the editor's while a mode is up,
    /// with the mode's one tab.
    mode_tab_bar: TabBar,
    /// The views the keyboard has been in, most recent last, without
    /// the current one: the order the modes palette lists them in.
    view_history: Vec<View>,
    /// The view the keyboard was in after the last event, to notice when
    /// it moves.
    last_view: View,
    tabs: Vec<Tab>,
    active: usize,
    /// Counts tab activations, to order tabs by most recently viewed.
    view_clock: u64,
    tab_bar: TabBar,
    palette: Option<Palette>,
    /// Which palette is open, for the two that need telling apart.
    palette_kind: PaletteKind,
    /// The entry last picked from the command palette, listed first the
    /// next time it opens so that Enter alone runs it again.
    last_command: Option<PaletteAction>,
    /// The search box, driving a search in the active tab. Never open at
    /// the same time as the palette.
    search_box: Option<SearchBox>,
    /// The last query searched for with Enter, for Ctrl+G to repeat.
    last_search: String,
    /// The go to line box, over the active tab. Never open at the same
    /// time as the palette, the search box, or the project search.
    goto_line: Option<GoToLineBox>,
    /// The project search dialog, kept (with its results) once opened so
    /// that it comes back as it was. Shown while `project_search_open`,
    /// never at the same time as the palette or the search box.
    project_search: Option<ProjectSearchDialog>,
    project_search_open: bool,
    index_generation: u64,
    /// The index's file activity counter as of the last check of open
    /// files for changes on disk, and when that check was; see
    /// [`check_open_files`](Self::check_open_files).
    file_activity: u64,
    last_file_check: Option<Instant>,
    /// A message shown in the status bar until the next key press.
    status: Option<String>,
    confirm: Option<Confirm>,
    /// Lives as long as the app: see the `clipboard` module for why.
    clipboard: Clipboard,
    quit: bool,
    editor_area: Rect,
    /// The tools shown below the editor.
    tool_pane: ToolPane,
    /// The tool pane's own tab bar, one tab per tool.
    tool_tab_bar: TabBar,
    /// Which view the keyboard drives.
    focus: Focus,
    /// Where programs' output is sent so it reaches the event loop: the
    /// sink each session is spawned with is built from a clone of this.
    events: Sender<AppEvent>,
    /// The screen regions of the tool pane from the last render, to
    /// hit-test the mouse.
    divider_area: Rect,
    tool_term_area: Rect,
    /// The editor-plus-divider-plus-tool region, for turning a divider
    /// drag into a split fraction.
    content_area: Rect,
    /// Whether a mouse drag is moving the editor/tool divider.
    dragging_divider: bool,
    /// The prefix key (Ctrl+]) pressed with a tool focused, waiting for
    /// the key it applies to. Kept as pressed so it can be sent on to the
    /// program in whichever spelling the terminal used.
    prefix: Option<KeyEvent>,
}

impl App {
    pub fn new(project: Project, storage: Storage, events: Sender<AppEvent>) -> App {
        // A settings file that can't be read is reported rather than
        // fatal: the defaults do until it is fixed, and saving from the
        // settings page replaces it.
        let (settings, mut status) = match storage.load_settings() {
            Ok(settings) => (settings, None),
            Err(err) => (Settings::default(), Some(err.to_string())),
        };
        // Likewise the build configuration; with none saved yet the
        // project's own root files are found.
        let project_storage = storage.project(&project);
        let mut build = match project_storage.load_build_config() {
            Ok(Some(build)) => build,
            Ok(None) => BuildConfig::find_roots(project.root()),
            Err(err) => {
                status = Some(err.to_string());
                BuildConfig::find_roots(project.root())
            }
        };
        // The file holds what the user changed; the project has the rest
        // of the targets, found on threads of their own since a big tree
        // takes seconds to look through, and the editor should be up
        // before then.
        let discoveries = build.request_discovery(project.root());
        let (discovery_tx, discovery_rx) = mpsc::channel();
        let app = App {
            index_generation: project.index().generation(),
            file_activity: 0,
            last_file_check: None,
            project,
            storage,
            project_storage,
            settings,
            build,
            configured: HashMap::new(),
            pending_steps: Vec::new(),
            job_title: String::new(),
            job_step: 0,
            job_configure: None,
            job_dirs: Vec::new(),
            status_config_area: Rect::default(),
            status_target_area: Rect::default(),
            discoveries: discovery_rx,
            discovery_tx,
            theme: Theme::default(),
            mode: Mode::Editor,
            git_log: None,
            mode_tab_bar: TabBar::default(),
            view_history: Vec::new(),
            last_view: View::Editor,
            tabs: Vec::new(),
            active: 0,
            view_clock: 0,
            tab_bar: TabBar::default(),
            palette: None,
            palette_kind: PaletteKind::Other,
            last_command: None,
            search_box: None,
            last_search: String::new(),
            goto_line: None,
            project_search: None,
            project_search_open: false,
            status,
            confirm: None,
            clipboard: Clipboard::new(),
            quit: false,
            editor_area: Rect::default(),
            tool_pane: ToolPane::default(),
            tool_tab_bar: TabBar::default(),
            focus: Focus::Editor,
            events,
            divider_area: Rect::default(),
            tool_term_area: Rect::default(),
            content_area: Rect::default(),
            dragging_divider: false,
            prefix: None,
        };
        app.start_discoveries(discoveries);
        app
    }

    // ----- Finding targets -------------------------------------------------

    /// Find the targets of roots pending discovery, each on a thread of
    /// its own; the results come back through `discoveries` and are
    /// taken in by [`tick`](Self::tick). A project's index has the
    /// project's files already, or will shortly, which saves walking
    /// the tree again; a directory opened on its own has no such list,
    /// and the tree under the root is walked.
    fn start_discoveries(&self, discoveries: Vec<Discovery>) {
        for discovery in discoveries {
            let index = self.project.index();
            let files = index.is_recursive().then(|| index.file_list());
            let tx = self.discovery_tx.clone();
            let name = format!("discovery {}", discovery.path().display());
            let _ = std::thread::Builder::new().name(name).spawn(move || {
                let files = files
                    .filter(|list| list.wait_for_primary(DISCOVERY_INDEX_WAIT))
                    .map(|list| list.files(false));
                let _ = tx.send(discovery.run(files.as_deref()));
            });
        }
    }

    /// Take in whatever the discovery threads have found since the last
    /// time. Returns whether anything came in.
    fn take_discoveries(&mut self) -> bool {
        let mut any = false;
        while let Ok(result) = self.discoveries.try_recv() {
            any |= self.take_discovery(result);
        }
        if any {
            self.refresh_target_palettes();
        }
        any
    }

    /// Take in one discovery's result. The build page follows its
    /// targets: discovered targets go ahead of the user's own, so the
    /// rows move, and the fields being edited must stay with their
    /// target.
    fn take_discovery(&mut self, result: DiscoveryResult) -> bool {
        let Some(root) = self.build.root_at(result.path()) else {
            return false;
        };
        let before: Vec<String> = self.build.roots()[root]
            .targets()
            .iter()
            .map(|t| t.name.clone())
            .collect();
        if self.build.apply_discovery(result).is_none() {
            return false;
        }
        let targets = self.build.roots()[root].targets();
        let moved: Vec<Option<usize>> = before
            .iter()
            .map(|name| targets.iter().position(|t| t.name == *name))
            .collect();
        if let Mode::Build(view) = &mut self.mode {
            view.targets_moved(root, &moved, &self.build);
        }
        true
    }

    /// Give a palette that lists targets the targets as they now are.
    fn refresh_target_palettes(&mut self) {
        let items = match self.palette_kind {
            PaletteKind::Commands => self.command_items(),
            PaletteKind::Targets => self.target_items().0,
            PaletteKind::Files | PaletteKind::Other => return,
        };
        if let Some(mut palette) = self.palette.take() {
            palette.set_items(items);
            self.refresh_discovery_hint(&mut palette);
            self.palette = Some(palette);
        }
    }

    /// A note in a palette that lists targets while some are still to
    /// be found.
    fn refresh_discovery_hint(&self, palette: &mut Palette) {
        let hint = self
            .build
            .is_discovering()
            .then(|| DISCOVERY_HINT.to_owned());
        palette.set_hint(hint);
    }

    /// Wait for every root's targets to be found and take them in, as
    /// tests that look at targets must.
    #[cfg(test)]
    pub fn wait_for_discovery(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.build.is_discovering() {
            let timeout = deadline.saturating_duration_since(Instant::now());
            match self.discoveries.recv_timeout(timeout) {
                Ok(result) => {
                    self.take_discovery(result);
                }
                Err(_) => panic!("discovery did not finish"),
            }
        }
        self.refresh_target_palettes();
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

    /// Reload the active file from disk over its unsaved edits. Undoable,
    /// so no confirmation is asked.
    fn discard_active(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let title = tab.title();
        self.status = Some(match tab.view.editor_mut().discard_changes() {
            Ok(true) => format!("Discarded unsaved changes to {title} (undo brings them back)"),
            Ok(false) => format!("{title} has no unsaved changes"),
            Err(err) => format!("Could not reload {title}: {err}"),
        });
    }

    /// See whether any open file has changed on disk and bring the changes
    /// in; see [`Editor::check_disk`]. Says what happened in the status
    /// bar and returns whether anything did. Done when the index reports
    /// filesystem activity, when the terminal regains focus, and every
    /// [`FILE_CHECK_INTERVAL`] regardless; see
    /// [`check_open_files_if_due`](Self::check_open_files_if_due).
    fn check_open_files(&mut self) -> bool {
        self.file_activity = self.project.index().activity();
        self.last_file_check = Some(Instant::now());
        let mut changed = false;
        for tab in &mut self.tabs {
            let title = tab.title();
            let message = match tab.view.editor_mut_in_place().check_disk() {
                Ok(ExternalChange::None) => continue,
                Ok(ExternalChange::Reloaded) => format!("{title} changed on disk: reloaded"),
                Ok(ExternalChange::Merged) => {
                    format!(
                        "{title} changed on disk: merged with unsaved changes (undo to keep yours)"
                    )
                }
                Ok(ExternalChange::Conflicted) => {
                    tab.view.reveal_cursor();
                    format!("{title} changed on disk: conflicts with unsaved changes are marked")
                }
                Ok(ExternalChange::Unmerged) => {
                    format!(
                        "{title} changed on disk but can't be merged with unsaved changes: saving will overwrite it"
                    )
                }
                Ok(ExternalChange::Deleted) => {
                    format!("{title} was deleted on disk: save to recreate it")
                }
                Err(err) => format!("Could not read {title}: {err}"),
            };
            self.status = Some(message);
            changed = true;
        }
        changed
    }

    /// [`check_open_files`](Self::check_open_files) if there has been
    /// filesystem activity since the last check or it was long enough
    /// ago. Stats every open file, so not for every idle tick.
    fn check_open_files_if_due(&mut self) -> bool {
        let hinted = self.project.index().activity() != self.file_activity;
        let due = self
            .last_file_check
            .is_none_or(|last| last.elapsed() >= FILE_CHECK_INTERVAL);
        if hinted || due {
            self.check_open_files()
        } else {
            false
        }
    }

    /// The title for the terminal window: the editor's name and the
    /// project's, or the directory's path when there isn't a project.
    pub fn window_title(&self) -> String {
        let location = match self.project.kind() {
            ProjectKind::Project => self.project.name(),
            ProjectKind::Directory => self.project.root().display().to_string(),
        };
        format!("{EDITOR_NAME} — {location}")
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
        self.goto_line = None;
        self.hide_project_search();
        // On the git pages the tabs are their repositories, the shown
        // one first so that Enter alone flips to the next.
        let repositories: Option<RepositoryTabs> = match &self.mode {
            Mode::GitLog(tabs) => Some((
                tabs.active_index(),
                tabs.titles().map(str::to_owned).collect(),
                PaletteAction::GitLogTab,
            )),
            Mode::Changes(tabs) => Some((
                tabs.active_index(),
                tabs.titles().map(str::to_owned).collect(),
                PaletteAction::ChangesTab,
            )),
            _ => None,
        };
        if let Some((active, titles, action)) = repositories {
            let order = std::iter::once(active).chain((0..titles.len()).filter(|i| *i != active));
            let items = order
                .map(|index| PaletteItem {
                    label: titles[index].clone(),
                    detail: if index == 0 {
                        "The project's repository".to_owned()
                    } else {
                        "Submodule".to_owned()
                    },
                    search: titles[index].clone(),
                    shortcut: None,
                    action: action(index),
                })
                .collect();
            let mut palette = Palette::new(REPOSITORIES_PLACEHOLDER, items);
            palette.select(1);
            self.palette = Some(palette);
            self.palette_kind = PaletteKind::Other;
            return;
        }
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
                    shortcut: None,
                    action: PaletteAction::SwitchTab(index),
                }
            })
            .collect();
        let mut palette = Palette::new(TABS_PLACEHOLDER, items);
        palette.select(1);
        self.palette = Some(palette);
        self.palette_kind = PaletteKind::Other;
    }

    fn open_files_palette(&mut self) {
        self.close_search_box(true);
        self.goto_line = None;
        self.hide_project_search();
        let mut palette = Palette::new(FILES_PLACEHOLDER, self.file_items());
        self.refresh_index_hint(&mut palette);
        self.palette = Some(palette);
        self.palette_kind = PaletteKind::Files;
    }

    /// Every file in the project, in the index's order: each directory's
    /// files by name, then its subdirectories likewise. (The index is
    /// asked for the list afresh, and this runs again whenever it
    /// changes while the palette is up, so on a project of hundreds of
    /// thousands of files it is worth keeping cheap: no sorting, and
    /// the three strings each entry needs and no more.)
    fn file_items(&self) -> Vec<PaletteItem> {
        self.project
            .index()
            .files(false)
            .into_iter()
            .map(|path| {
                let relative = self.display_path(&path);
                let label = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let detail = relative
                    .strip_suffix(label.as_str())
                    .map(|dir| dir.trim_end_matches(std::path::MAIN_SEPARATOR))
                    .unwrap_or_default()
                    .to_owned();
                PaletteItem {
                    label,
                    detail,
                    search: relative,
                    shortcut: None,
                    action: PaletteAction::OpenFile(path),
                }
            })
            .collect()
    }

    fn refresh_index_hint(&self, palette: &mut Palette) {
        let hint = (!self.project.index().is_primary_complete()).then(|| "indexing…".to_owned());
        palette.set_hint(hint);
    }

    /// Ctrl+E: open the modes palette, listing the editor, the pages,
    /// and every kind of tool. The current view comes first, then
    /// the others most recently used first, so the one before this is
    /// preselected and Enter alone goes back to it, as the tab search
    /// does.
    fn open_modes_palette(&mut self) {
        self.close_search_box(true);
        self.goto_line = None;
        self.hide_project_search();
        self.track_view();
        let current = self.current_view();
        let mut views = vec![current];
        views.extend(self.view_history.iter().rev().copied());
        let rest: Vec<View> = View::all().filter(|view| !views.contains(view)).collect();
        views.extend(rest);
        let items = views
            .into_iter()
            .map(|view| PaletteItem {
                label: view.label().to_owned(),
                detail: view.description().to_owned(),
                search: view.label().to_owned(),
                shortcut: view.shortcut(),
                action: view.action(),
            })
            .collect();
        let mut palette = Palette::new(MODES_PLACEHOLDER, items);
        palette.select(1);
        self.palette = Some(palette);
        self.palette_kind = PaletteKind::Other;
    }

    /// Ctrl+P: open the command palette, listing every command, then
    /// every view with the key bound to each, then "Run <target>" for
    /// every target that isn't disabled, with whatever it last ran ahead
    /// of them all. The description is searched too, so "clean" finds
    /// the commands that delete build directories.
    fn open_command_palette(&mut self) {
        self.close_search_box(true);
        self.goto_line = None;
        self.hide_project_search();
        let mut palette = Palette::new(COMMANDS_PLACEHOLDER, self.command_items());
        self.refresh_discovery_hint(&mut palette);
        self.palette = Some(palette);
        self.palette_kind = PaletteKind::Commands;
    }

    /// The command palette's entries; see
    /// [`open_command_palette`](Self::open_command_palette).
    fn command_items(&self) -> Vec<PaletteItem> {
        let commands = Command::ALL.into_iter().map(|command| PaletteItem {
            label: command.label().to_owned(),
            detail: command.description().to_owned(),
            search: format!("{} {}", command.label(), command.description()),
            shortcut: command.shortcut(),
            action: PaletteAction::Command(command),
        });
        let views = View::all().map(|view| PaletteItem {
            label: view.label().to_owned(),
            detail: view.description().to_owned(),
            search: format!("{} {}", view.label(), view.description()),
            shortcut: view.shortcut(),
            action: view.action(),
        });
        let mut items: Vec<PaletteItem> = commands
            .chain(views)
            .chain(self.run_target_items())
            .collect();
        // The entry last run comes first, so Enter alone repeats it. A
        // "Run <target>" whose target is gone has no entry to move.
        if let Some(last) = &self.last_command
            && let Some(index) = items.iter().position(|item| item.action == *last)
        {
            let item = items.remove(index);
            items.insert(0, item);
        }
        items
    }

    /// The command palette's "Run <target>" entries: one for each
    /// target that isn't disabled, in name order within each root, each
    /// saying what configuration it would run with.
    fn run_target_items(&self) -> Vec<PaletteItem> {
        let mut items = Vec::new();
        for (r, root) in self.build.roots().iter().enumerate() {
            for i in root.targets_by_name() {
                let target = &root.targets()[i];
                if target.disabled {
                    continue;
                }
                let configuration = self
                    .build
                    .selection_with_target(r, i)
                    .and_then(|selection| self.build.configuration_for(selection))
                    .map_or_else(String::new, |(_, c)| format!(" with {}", c.name));
                let detail = format!(
                    "Build and run it{} in {}, without making it the current target",
                    configuration,
                    root.label()
                );
                items.push(PaletteItem {
                    label: format!("Run {}", target.name),
                    search: format!("Run {} {}", target.name, detail),
                    detail,
                    shortcut: None,
                    action: PaletteAction::RunTarget { root: r, index: i },
                });
            }
        }
        items
    }

    /// Run a command picked from the command palette, as its key would.
    fn run_command(&mut self, command: Command) {
        match command {
            Command::OpenFile => self.open_files_palette(),
            Command::SwitchTab => self.open_tabs_palette(),
            Command::Save => self.save_active(),
            Command::DiscardChanges => self.discard_active(),
            Command::CloseTab => self.request_close_tab(self.active),
            Command::Find => self.open_search_box(),
            Command::FindNext => self.find_next(),
            Command::SearchProject => self.open_project_search(),
            Command::GoToLine => self.open_goto_line(),
            Command::NextConflict => self.step_conflict(true),
            Command::PreviousConflict => self.step_conflict(false),
            Command::Undo => self.press_editor_key('z'),
            Command::Redo => self.press_editor_key('y'),
            Command::Cut => self.press_editor_key('x'),
            Command::Copy => self.press_editor_key('c'),
            Command::Paste => self.press_editor_key('v'),
            Command::SelectAll => self.press_editor_key('a'),
            Command::Build => self.build(),
            Command::Run => self.run(),
            Command::SelectConfiguration => self.open_configuration_palette(),
            Command::SelectTarget => self.open_target_palette(),
            Command::DeleteBuildDir => self.delete_build_dir(),
            Command::DeleteAllBuildDirs => self.delete_all_build_dirs(),
            Command::ToggleAmend => self.toggle_amend(),
            Command::SwitchView => self.open_modes_palette(),
            Command::NextView => self.focus_next(),
            Command::PreviousView => self.focus_previous(),
            Command::Quit => self.request_quit(),
        }
    }

    /// Do what Ctrl and a letter do in the active tab's editor (undo,
    /// say), for a command that is that key's: the key is the one
    /// source of what it does. With a mode in the editor's place there
    /// is no editor to press it in. An edit made this way from a tool
    /// brings the editor into focus, so it can be seen.
    fn press_editor_key(&mut self, letter: char) {
        if !matches!(self.mode, Mode::Editor) {
            return;
        }
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let key = KeyEvent::new(KeyCode::Char(letter), KeyModifiers::CONTROL);
        tab.view.handle_key(key, &mut self.clipboard);
        self.focus = Focus::Editor;
    }

    /// Where the keyboard is: a tool when one is focused, else whatever
    /// the upper part of the screen shows.
    fn current_view(&self) -> View {
        if self.focus == Focus::Tool
            && self.tool_pane.is_visible()
            && let Some(tool) = self.tool_pane.active()
        {
            return View::Tool(tool.kind());
        }
        match self.mode {
            Mode::Editor => View::Editor,
            Mode::Settings(_) => View::Settings,
            Mode::Build(_) => View::Build,
            Mode::GitLog(_) => View::GitLog,
            Mode::Changes(_) => View::Changes,
        }
    }

    /// Note where the keyboard is now, so the modes palette can list the
    /// views most recently used first.
    fn track_view(&mut self) {
        let current = self.current_view();
        if current == self.last_view {
            return;
        }
        let previous = std::mem::replace(&mut self.last_view, current);
        self.view_history
            .retain(|view| *view != previous && *view != current);
        self.view_history.push(previous);
    }

    fn run_palette_action(&mut self, action: PaletteAction) {
        self.palette = None;
        if self.palette_kind == PaletteKind::Commands {
            self.last_command = Some(action.clone());
        }
        match action {
            PaletteAction::SwitchTab(index) => {
                self.enter_editor();
                self.activate(index);
            }
            PaletteAction::OpenFile(path) => {
                self.enter_editor();
                self.open_file(path);
            }
            PaletteAction::FocusEditor => self.enter_editor(),
            PaletteAction::OpenSettings => self.open_settings(),
            PaletteAction::OpenBuildConfig => self.open_build_config(),
            PaletteAction::OpenGitLog => self.open_git_log(),
            PaletteAction::GitLogTab(index) => {
                if let Mode::GitLog(tabs) = &mut self.mode {
                    tabs.set_active(index);
                    self.focus = Focus::Editor;
                }
            }
            PaletteAction::OpenChanges => self.open_changes(),
            PaletteAction::ChangesTab(index) => {
                if let Mode::Changes(tabs) = &mut self.mode {
                    tabs.set_active(index);
                    self.focus = Focus::Editor;
                }
            }
            PaletteAction::OpenTool(kind) => self.open_tool(kind),
            PaletteAction::AddBuildRoot(path) => self.add_build_root(path),
            PaletteAction::SelectConfiguration { root, index } => {
                if self.build.select_configuration(root, index) {
                    self.selection_changed();
                }
            }
            PaletteAction::SelectTarget { root, index } => {
                if self.build.select_target(root, index) {
                    self.selection_changed();
                }
            }
            PaletteAction::RunTarget { root, index } => self.run_target(root, index),
            PaletteAction::Command(command) => self.run_command(command),
        }
    }

    // ----- Modes and settings ---------------------------------------------

    /// Show the settings page in the editor's place and give it the
    /// keyboard.
    fn open_settings(&mut self) {
        self.close_editor_overlays();
        if !matches!(self.mode, Mode::Settings(_)) {
            self.mode = Mode::Settings(SettingsView::new(&self.settings));
        }
        self.focus = Focus::Editor;
    }

    /// Show the editor in place of any mode, and give it the keyboard.
    /// Values typed into the settings page but not yet applied are
    /// applied on the way out.
    fn enter_editor(&mut self) {
        self.leave_mode();
        self.focus = Focus::Editor;
    }

    /// Put the editor back in the upper part of the screen.
    fn leave_mode(&mut self) {
        match std::mem::replace(&mut self.mode, Mode::Editor) {
            Mode::Editor => {}
            Mode::Settings(mut view) => {
                let outcome = view.commit_all(&mut self.settings);
                self.handle_settings_outcome(outcome);
            }
            Mode::Build(mut view) => {
                let outcome = view.commit_all(&mut self.build);
                self.handle_build_outcome(outcome);
            }
            Mode::GitLog(tabs) => self.git_log = Some(tabs),
            Mode::Changes(_) => {}
        }
    }

    fn handle_settings_outcome(&mut self, outcome: SettingsOutcome) {
        match outcome {
            SettingsOutcome::Continue => {}
            SettingsOutcome::Changed => self.settings_changed(),
        }
    }

    /// The settings changed: keep them, and apply them to what is
    /// running.
    fn settings_changed(&mut self) {
        self.apply_settings();
        if let Err(err) = self.storage.save_settings(&self.settings) {
            self.status = Some(format!(
                "Could not save {}: {err}",
                self.storage
                    .path(ninjaedit_core::storage::SETTINGS_FILE)
                    .display()
            ));
        }
    }

    /// Apply the settings to what is running: every terminal's scrollback
    /// and the project search's limit. The shell setting applies to the
    /// next shell started.
    fn apply_settings(&mut self) {
        let scrollback = self.settings.terminal_scrollback();
        for tool in self.tool_pane.tools_mut() {
            tool.set_scrollback_limit(scrollback);
        }
        if let Some(dialog) = &mut self.project_search {
            dialog.set_limit(self.settings.search_max_results());
        }
    }

    // ----- Build configuration --------------------------------------------

    /// Show the build configuration page in the editor's place and give
    /// it the keyboard.
    fn open_build_config(&mut self) {
        self.close_editor_overlays();
        if !matches!(self.mode, Mode::Build(_)) {
            self.mode = Mode::Build(BuildView::new(&self.build));
        }
        self.focus = Focus::Editor;
    }

    // ----- Git log --------------------------------------------------------

    /// Ctrl+L: show the git log page in the editor's place and give it
    /// the keyboard. The page is kept when it is left, and comes back
    /// as it was: the same commit and file selected. Coming back, or
    /// Ctrl+L while it is up, reads the repository again in the
    /// background, since a commit may have been made or a branch
    /// fetched meanwhile; what the page shows changes only once the
    /// history is read, and keeps its place where it can.
    fn open_git_log(&mut self) {
        self.close_editor_overlays();
        match &mut self.mode {
            Mode::GitLog(tabs) => tabs.refresh(),
            _ => {
                self.leave_mode();
                let tabs = match self.git_log.take() {
                    Some(mut tabs) => {
                        tabs.refresh();
                        tabs
                    }
                    None => {
                        // A layout file that can't be read is reported,
                        // and the defaults do; the next resize replaces
                        // it.
                        let layout = match git_layout::load(
                            &self.project_storage,
                            git_layout::LAYOUT_FILE,
                        ) {
                            Ok(layout) => layout,
                            Err(err) => {
                                self.status = Some(err.to_string());
                                Default::default()
                            }
                        };
                        Box::new(GitLogTabs::new(self.project.root(), layout))
                    }
                };
                self.mode = Mode::GitLog(tabs);
            }
        }
        self.focus = Focus::Editor;
    }

    // ----- Changes --------------------------------------------------------

    /// Ctrl+U: show the changes page in the editor's place and give it
    /// the keyboard. With the page already up, scan the working tree
    /// again: the user may have saved a file, or run git in a shell.
    fn open_changes(&mut self) {
        self.close_editor_overlays();
        match &mut self.mode {
            Mode::Changes(tabs) => tabs.refresh(),
            _ => {
                self.leave_mode();
                let layout = match git_layout::load(
                    &self.project_storage,
                    git_layout::CHANGES_LAYOUT_FILE,
                ) {
                    Ok(layout) => layout,
                    Err(err) => {
                        self.status = Some(err.to_string());
                        Default::default()
                    }
                };
                self.mode = Mode::Changes(Box::new(ChangesTabs::new(self.project.root(), layout)));
            }
        }
        self.focus = Focus::Editor;
    }

    /// The command palette's "Toggle amend": on the changes page, make
    /// the commit replace the last one, or follow it again.
    fn toggle_amend(&mut self) {
        if let Mode::Changes(tabs) = &mut self.mode {
            let outcome = tabs.toggle_amend();
            self.handle_changes_outcome(outcome);
        } else {
            self.status = Some("Amending is a choice on the changes page (Ctrl+U)".to_owned());
        }
    }

    /// Act on what the changes page asked for after a key.
    fn handle_changes_outcome(&mut self, outcome: ChangesOutcome) {
        match outcome {
            ChangesOutcome::Continue => {}
            ChangesOutcome::Notice(message) => self.status = Some(message),
            ChangesOutcome::OpenFile { path, conflicted } => {
                self.enter_editor();
                self.open_file(&path);
                // A failure to open is in the status bar; the cursor
                // goes to the first conflict of the file that opened.
                if conflicted && self.status.is_none() {
                    self.step_conflict(true);
                }
            }
        }
    }

    fn handle_build_outcome(&mut self, outcome: BuildOutcome) {
        match outcome {
            BuildOutcome::Continue => {}
            BuildOutcome::Changed => self.build_changed(),
            BuildOutcome::AddRoot => self.open_add_root_palette(),
            BuildOutcome::Notice(message) => self.status = Some(message),
        }
    }

    /// The build configuration changed: keep it in the project's
    /// storage.
    fn build_changed(&mut self) {
        if let Err(err) = self.project_storage.save_build_config(&self.build) {
            self.status = Some(format!(
                "Could not save {}: {err}",
                self.project_storage
                    .path(ninjaedit_core::build::BUILD_FILE)
                    .display()
            ));
        }
    }

    /// The current configuration or target changed from outside the
    /// page: save, and show the page the change if it is up.
    fn selection_changed(&mut self) {
        if let Mode::Build(view) = &mut self.mode {
            view.refresh(&self.build);
        }
        self.build_changed();
    }

    /// Offer the project's `Cargo.toml` and `CMakeLists.txt` files that
    /// aren't build roots yet, to add one.
    fn open_add_root_palette(&mut self) {
        self.close_search_box(true);
        self.goto_line = None;
        self.hide_project_search();
        let root = self.project.root();
        let files = self.project.index().files(false);
        let relative: Vec<PathBuf> = files
            .iter()
            .map(|path| path.strip_prefix(root).unwrap_or(path).to_path_buf())
            .collect();
        let existing: Vec<&Path> = self.build.roots().iter().map(BuildRoot::path).collect();
        let items = root_candidates(relative.iter())
            .into_iter()
            .filter(|path| !existing.contains(&path.as_path()))
            .map(|path| {
                let display = path.display().to_string();
                PaletteItem {
                    label: path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    detail: parent_of(&display),
                    search: display,
                    shortcut: None,
                    action: PaletteAction::AddBuildRoot(path),
                }
            })
            .collect();
        let mut palette = Palette::new(ADD_ROOT_PLACEHOLDER, items);
        self.refresh_index_hint(&mut palette);
        self.palette = Some(palette);
        self.palette_kind = PaletteKind::Other;
    }

    /// Add a root file (relative to the project) with its defaults and
    /// the targets found under it, and select it on the page.
    fn add_build_root(&mut self, path: PathBuf) {
        let Some(root) = BuildRoot::pending(&path) else {
            self.status = Some(format!("{} is not a build root file", path.display()));
            return;
        };
        let discovery = root.discovery(self.project.root());
        match self.build.add_root(root) {
            Ok(index) => {
                self.start_discoveries(vec![discovery]);
                if let Mode::Build(view) = &mut self.mode {
                    view.select(Node::Root(index), &self.build);
                }
                self.build_changed();
            }
            Err(err) => self.status = Some(err),
        }
    }

    /// A palette of every root's configurations in name order, the
    /// current one preselected, to pick the current one.
    fn open_configuration_palette(&mut self) {
        self.close_search_box(true);
        self.goto_line = None;
        self.hide_project_search();
        let current = self.build.current();
        let mut items = Vec::new();
        let mut selected = 0;
        for (r, root) in self.build.roots().iter().enumerate() {
            for i in root.configurations_by_name() {
                let configuration = &root.configurations()[i];
                if current.is_some_and(|c| c.root == r && c.configuration == i) {
                    selected = items.len();
                }
                items.push(PaletteItem {
                    label: configuration.name.clone(),
                    detail: root.label(),
                    search: format!("{} {}", configuration.name, root.label()),
                    shortcut: None,
                    action: PaletteAction::SelectConfiguration { root: r, index: i },
                });
            }
        }
        let mut palette = Palette::new(CONFIGURATION_PLACEHOLDER, items);
        palette.select(selected);
        self.palette = Some(palette);
        self.palette_kind = PaletteKind::Other;
    }

    /// A palette of every root's targets, likewise.
    fn open_target_palette(&mut self) {
        self.close_search_box(true);
        self.goto_line = None;
        self.hide_project_search();
        let (items, selected) = self.target_items();
        let mut palette = Palette::new(TARGET_PLACEHOLDER, items);
        palette.select(selected);
        self.refresh_discovery_hint(&mut palette);
        self.palette = Some(palette);
        self.palette_kind = PaletteKind::Targets;
    }

    /// The target palette's entries, and which of them is the current
    /// target.
    fn target_items(&self) -> (Vec<PaletteItem>, usize) {
        let current = self.build.current();
        let mut items = Vec::new();
        let mut selected = 0;
        for (r, root) in self.build.roots().iter().enumerate() {
            for i in root.targets_by_name() {
                let target = &root.targets()[i];
                if target.disabled {
                    continue;
                }
                if current.is_some_and(|c| c.root == r && c.target == i) {
                    selected = items.len();
                }
                items.push(PaletteItem {
                    label: target.name.clone(),
                    detail: root.label(),
                    search: format!("{} {}", target.name, root.label()),
                    shortcut: None,
                    action: PaletteAction::SelectTarget { root: r, index: i },
                });
            }
        }
        (items, selected)
    }

    // ----- Building and running -------------------------------------------

    /// Ctrl+B: build the current target with the current configuration
    /// in the output tool.
    fn build(&mut self) {
        self.start_current_job(false);
    }

    /// Ctrl+R: build and run the current target in the output tool.
    fn run(&mut self) {
        self.start_current_job(true);
    }

    /// Values typed into the build configuration page but not yet
    /// applied count for a job started while it is up.
    fn commit_build_page(&mut self) {
        if let Mode::Build(view) = &mut self.mode {
            let outcome = view.commit_all(&mut self.build);
            self.handle_build_outcome(outcome);
        }
    }

    fn start_current_job(&mut self, run: bool) {
        self.commit_build_page();
        // There is no selection only with no roots; and one whose target
        // is still being looked for would build the wrong thing.
        match self.build.current_for_job() {
            Ok(selection) => self.start_selected_job(selection, run),
            Err(message) => self.status = Some(message),
        }
    }

    /// "Run <target>" from the command palette: build and run a target
    /// that needn't be the current one, with the configuration selecting
    /// it would bring, leaving the current target and configuration as
    /// they are.
    fn run_target(&mut self, root: usize, index: usize) {
        self.commit_build_page();
        if let Some(selection) = self.build.selection_with_target(root, index) {
            self.start_selected_job(selection, true);
        }
    }

    /// Build, or build and run, a selection's target with its
    /// configuration in the output tool.
    fn start_selected_job(&mut self, selection: Selection, run: bool) {
        let configure = self.cmake_configure_needed(selection);
        let root = self.project.root().to_path_buf();
        let job = if run {
            self.build
                .run_job_for(selection, &root, configure.is_some(), &self.settings)
        } else {
            self.build
                .build_job_for(selection, &root, configure.is_some(), &self.settings)
        };
        match job {
            Ok(job) => {
                self.job_configure = configure;
                self.start_job(job);
                // A compiler run by CMake's build tool may name files
                // relative to the build directory.
                if let Some(dir) = self.build.cmake_build_dir_for(&root, selection) {
                    self.job_dirs.push(dir);
                }
            }
            Err(err) => self.status = Some(err),
        }
    }

    /// "Delete build directory": throw away what the current
    /// configuration has built, in the output tool like a build. A
    /// CMake directory deleted this way is configured again by the
    /// next build.
    fn delete_build_dir(&mut self) {
        self.commit_build_page();
        let root = self.project.root().to_path_buf();
        match self.build.clean_job(&root) {
            Ok(job) => {
                if let Some(dir) = self.build.cmake_build_dir(&root) {
                    self.configured.remove(&dir);
                }
                self.job_configure = None;
                self.start_job(job);
            }
            Err(err) => self.status = Some(err),
        }
    }

    /// "Delete all build directories": likewise for every configuration
    /// of every root.
    fn delete_all_build_dirs(&mut self) {
        self.commit_build_page();
        match self.build.clean_all_job(self.project.root()) {
            Ok(job) => {
                self.configured.clear();
                self.job_configure = None;
                self.start_job(job);
            }
            Err(err) => self.status = Some(err),
        }
    }

    /// Whether a selection's CMake configuration's build directory needs
    /// configuring before a build, and with what: it has no cache yet,
    /// or was last configured with different options. `None` for a
    /// Cargo configuration, or one configured as it now is.
    fn cmake_configure_needed(&self, selection: Selection) -> Option<(PathBuf, String)> {
        let dir = self
            .build
            .cmake_build_dir_for(self.project.root(), selection)?;
        let (_, configuration) = self.build.configuration_for(selection)?;
        let signature = configuration.cmake_configure_signature(&self.settings);
        let configured =
            dir.join("CMakeCache.txt").is_file() && self.configured.get(&dir) == Some(&signature);
        (!configured).then_some((dir, signature))
    }

    /// Run a job's steps in the output tool, one after another, stopping
    /// whatever was running there. Shows the tool and gives it the
    /// keyboard, so that Ctrl+C reaches the job.
    fn start_job(&mut self, job: Job) {
        self.ensure_tool(ToolKind::Output);
        self.tool_pane.set_visible(true);
        self.close_editor_overlays();
        self.focus = Focus::Tool;
        let Some(tool) = self.tool_pane.active_mut() else {
            return;
        };
        tool.stop();
        tool.clear_screen();
        self.job_title = job.title;
        self.job_step = 0;
        self.job_dirs.clear();
        for step in &job.steps {
            if let Some(dir) = step.command.current_dir_path()
                && !self.job_dirs.iter().any(|d| d == dir)
            {
                self.job_dirs.push(dir.to_path_buf());
            }
        }
        self.pending_steps = job.steps;
        self.start_next_step();
    }

    /// Open the file a link in a tool's output names, at its line and
    /// column, in the editor. The path is looked for where the last job's
    /// commands ran and then in the project; a file found nowhere is
    /// reported in the status bar.
    fn open_source_location(&mut self, location: &SourceLocation) {
        let root = self.project.root().to_path_buf();
        let bases = self
            .job_dirs
            .iter()
            .map(PathBuf::as_path)
            .chain(std::iter::once(root.as_path()));
        let Some(path) = location.resolve(bases) else {
            self.status = Some(format!("Could not find {}", location.path));
            return;
        };
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        self.enter_editor();
        self.open_file(&path);
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        if tab.path() != Some(&*path) {
            return; // it couldn't be opened; the status bar says why
        }
        let column = location.column.unwrap_or(1);
        tab.view
            .go_to_line_column(location.line.saturating_sub(1), column.saturating_sub(1));
    }

    /// Start the next of the running job's steps, with a heading saying
    /// what it does and the command it runs.
    fn start_next_step(&mut self) {
        if self.pending_steps.is_empty() {
            return;
        }
        let step = self.pending_steps.remove(0);
        self.job_step += 1;
        let events = self.events.clone();
        let Some(index) = self.tool_pane.index_of_kind(ToolKind::Output) else {
            return;
        };
        let tool = &mut self.tool_pane.tools_mut()[index];
        let heading = format!(
            "\x1b[1m{}\x1b[0m\r\n\x1b[2m$ {}\x1b[0m\r\n",
            step.description,
            step.command.display()
        );
        tool.process(heading.as_bytes());
        let command = step.command;
        if let Err(err) = tool.start(|cols, rows| spawn_session(&command, cols, rows, events)) {
            let message = format!(
                "Could not start {}: {err}",
                command.program().to_string_lossy()
            );
            tool.process(format!("\x1b[1;31m{message}\x1b[0m\r\n").as_bytes());
            self.pending_steps.clear();
            self.status = Some(message);
        }
    }

    /// A step of the running job exited: go on to the next when it
    /// succeeded, or stop the job when it failed, saying so either way.
    fn step_exited(&mut self, index: usize, status: ExitStatus) {
        let tool = &mut self.tool_pane.tools_mut()[index];
        if status.success() {
            if self.job_step == 1
                && let Some((dir, signature)) = self.job_configure.take()
            {
                self.configured.insert(dir, signature);
            }
            if self.pending_steps.is_empty() {
                tool.process(b"\x1b[1;32mFinished\x1b[0m\r\n");
                self.status = Some(format!("{}: finished", self.job_title));
            } else {
                tool.process(b"\r\n");
                self.start_next_step();
            }
        } else {
            let why = match &status.signal {
                Some(signal) => format!("killed by {signal}"),
                None => format!("exit code {}", status.code),
            };
            tool.process(format!("\x1b[1;31mFailed: {why}\x1b[0m\r\n").as_bytes());
            self.pending_steps.clear();
            self.job_configure = None;
            self.status = Some(format!("{}: failed with {why}", self.job_title));
        }
    }

    // ----- Search ---------------------------------------------------------

    /// Open the search box over the active tab, starting a search from its
    /// cursor, or with the selected text as the query when the selection
    /// lies within one line. Does nothing with the box already open; with
    /// no tab there is nothing to search, so the project search opens
    /// instead.
    fn open_search_box(&mut self) {
        if self.search_box.is_some() {
            return;
        }
        // A mode in the editor's place has nothing to search either.
        if self.tabs.get(self.active).is_none() || !matches!(self.mode, Mode::Editor) {
            self.open_project_search();
            return;
        }
        self.palette = None;
        self.goto_line = None;
        self.hide_project_search();
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let editor = tab.view.editor_mut_in_place();
        let seed = single_line_selection(editor);
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
                    self.focus = Focus::Editor;
                }
            }
            SearchOutcome::Next => self.step_search(),
        }
    }

    // ----- Go to line -----------------------------------------------------

    /// Ctrl+J: open the go to line box over the active tab. Does nothing
    /// with the box already open, or with no tab to move around in.
    fn open_goto_line(&mut self) {
        if self.goto_line.is_some() || !matches!(self.mode, Mode::Editor) {
            return;
        }
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let line_count = tab.view.editor().buffer().line_count();
        self.palette = None;
        self.close_search_box(true);
        self.hide_project_search();
        self.goto_line = Some(GoToLineBox::new(line_count));
    }

    fn handle_goto_line_outcome(&mut self, outcome: GoToLineOutcome) {
        match outcome {
            GoToLineOutcome::Continue => {}
            GoToLineOutcome::Close => self.goto_line = None,
            GoToLineOutcome::Accept(line) => {
                self.goto_line = None;
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    // Line numbers are typed counting from one; zero and
                    // anything past the end clamp to the first and last.
                    tab.view.go_to_line(line.saturating_sub(1));
                    self.focus = Focus::Editor;
                }
            }
        }
    }

    /// Ctrl+G: move the active tab's search on to its next match, or with
    /// no search going, search again for the last query from the cursor.
    fn find_next(&mut self) {
        if !matches!(self.mode, Mode::Editor) {
            return;
        }
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

    /// "Next conflict" and "Previous conflict": move the active tab's
    /// cursor to the start of the next or previous merge conflict,
    /// wrapping around the file, and give the editor the keyboard so the
    /// jump can be seen. The status bar says when it wrapped, and when
    /// there is nothing to go to.
    fn step_conflict(&mut self, forward: bool) {
        if !matches!(self.mode, Mode::Editor) {
            return;
        }
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        match tab.view.step_conflict(forward) {
            ConflictStep::Moved => {}
            ConflictStep::Wrapped => {
                let end = if forward { "first" } else { "last" };
                self.status = Some(format!("Wrapped around to the {end} conflict in the file"));
            }
            ConflictStep::NoConflicts => {
                self.status = Some("No merge conflicts in this file".to_owned());
                return;
            }
        }
        self.focus = Focus::Editor;
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

    /// Ctrl+Shift+F: show the project search dialog, seeded with the
    /// active tab's selection when that lies within one line and isn't
    /// what the dialog is already searching for; otherwise the dialog
    /// comes back as it was, its query selected so that typing replaces
    /// it.
    fn open_project_search(&mut self) {
        self.show_project_search(None);
    }

    /// Ctrl+F with the search box open: move the search to the whole
    /// project. A query typed into the box is what the dialog searches
    /// for; with nothing typed this is Ctrl+Shift+F.
    fn upgrade_search_to_project(&mut self) {
        let Some(search_box) = &self.search_box else {
            return;
        };
        let query = search_box.query().to_owned();
        let seed = self
            .tabs
            .get(self.active)
            .and_then(|tab| single_line_selection(tab.view.editor()))
            .map(|seed| literal_query(&seed));
        let typed = !query.is_empty() && seed.as_deref() != Some(query.as_str());
        self.show_project_search(typed.then_some(query));
    }

    /// Show the project search dialog, searching for `query` when given,
    /// and otherwise seeded from the selection as
    /// [`open_project_search`](Self::open_project_search) describes.
    fn show_project_search(&mut self, query: Option<String>) {
        self.palette = None;
        self.goto_line = None;
        let seed = self
            .tabs
            .get(self.active)
            .and_then(|tab| single_line_selection(tab.view.editor()));
        self.close_search_box(true);
        // Files with unsaved changes are searched as their editors have
        // them; the dialog notices when these differ from last time.
        let buffers = self
            .tabs
            .iter()
            .filter_map(|tab| {
                let editor = tab.view.editor();
                if !editor.is_modified() {
                    return None;
                }
                let buffer = editor.buffer();
                let path = buffer.path()?.to_path_buf();
                Some((path, buffer.version(), buffer.snapshot()))
            })
            .collect();
        let dialog = self.project_search.get_or_insert_with(|| {
            ProjectSearchDialog::new(
                self.project.root().to_path_buf(),
                self.project.index().file_list(),
            )
        });
        dialog.set_limit(self.settings.search_max_results());
        dialog.set_buffers(buffers);
        match (query, seed) {
            (Some(query), _) => dialog.set_query_selected(&query),
            (None, Some(seed)) if !dialog.is_searching_for(&seed) => {
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
                self.enter_editor();
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

    // ----- Tool pane ------------------------------------------------------

    /// Ctrl+`: show the shell and focus it; with it showing but the editor
    /// focused, just focus it; and with it focused, hide the pane and go
    /// back to the editor. Hiding leaves the program running; showing
    /// again returns to it, or starts a new shell when the last one has
    /// exited.
    fn toggle_shell(&mut self) {
        let shell_showing = self.tool_pane.is_visible()
            && self.tool_pane.active().map(Tool::kind) == Some(ToolKind::Shell);
        if shell_showing && self.focus == Focus::Tool {
            self.tool_pane.set_visible(false);
            self.focus = Focus::Editor;
        } else {
            self.open_tool(ToolKind::Shell);
        }
    }

    /// Show a tool in the pane and give it the keyboard, starting or
    /// restarting its program as needed. A failure to start leaves a
    /// message in the status bar instead, and the focus where it was.
    /// The output tool shows whether or not a job is running in it.
    fn open_tool(&mut self, kind: ToolKind) {
        self.close_editor_overlays();
        self.ensure_tool(kind);
        if self
            .tool_pane
            .active()
            .is_some_and(|tool| tool.is_running() || tool.kind().shows_when_idle())
        {
            self.tool_pane.set_visible(true);
            self.focus = Focus::Tool;
        }
    }

    /// Make sure there is a tool of `kind`, active in the pane, and that
    /// it is running, when it is the kind that runs a program of its
    /// own. Reports a failure to start in the status bar.
    fn ensure_tool(&mut self, kind: ToolKind) {
        match self.tool_pane.index_of_kind(kind) {
            Some(index) => self.tool_pane.set_active(index),
            None => {
                // Sized to the terminal area the last render measured, or a
                // sensible default before the first one; the next render
                // fits it exactly.
                let (cols, rows) = self.default_tool_size();
                let scrollback = self.settings.terminal_scrollback();
                let mut tool = Tool::of_kind(kind, cols, rows, scrollback);
                if kind == ToolKind::Output {
                    tool.process(OUTPUT_WELCOME.as_bytes());
                }
                self.tool_pane.add(tool);
            }
        }
        self.start_active_tool();
    }

    /// Start the active tool's program if it isn't already running, with
    /// the command the settings now call for.
    fn start_active_tool(&mut self) {
        let events = self.events.clone();
        let Some(tool) = self.tool_pane.active_mut() else {
            return;
        };
        if tool.is_running() {
            return;
        }
        let name = tool.kind().name().to_lowercase();
        let Some(command) = tool.kind().command(self.project.root(), &self.settings) else {
            return;
        };
        if let Err(err) = tool.start(|cols, rows| spawn_session(&command, cols, rows, events)) {
            self.status = Some(format!("Could not start {name}: {err}"));
        }
    }

    /// The terminal size to start a tool at before a render has measured
    /// the pane: the last tool area if there is one, else a common default.
    fn default_tool_size(&self) -> (u16, u16) {
        if self.tool_term_area.width > 0 && self.tool_term_area.height > 0 {
            (self.tool_term_area.width, self.tool_term_area.height)
        } else {
            (80, 24)
        }
    }

    /// Ctrl+. and Ctrl+,: move the keyboard focus to the next or previous
    /// view. With the tool pane hidden there is only the editor, so this
    /// does nothing.
    fn focus_next(&mut self) {
        self.switch_focus();
    }

    fn focus_previous(&mut self) {
        // With two views, previous and next are the same flip.
        self.switch_focus();
    }

    fn switch_focus(&mut self) {
        if !self.tool_pane.is_visible() {
            self.focus = Focus::Editor;
            return;
        }
        self.focus = match self.focus {
            Focus::Editor => Focus::Tool,
            Focus::Tool => Focus::Editor,
        };
        if self.focus == Focus::Tool {
            self.close_editor_overlays();
        }
    }

    /// Whether a floating overlay (a palette, the search or go to line
    /// box, the project search) is open, and so has the keyboard whichever
    /// view is focused.
    fn overlay_open(&self) -> bool {
        self.palette.is_some()
            || self.search_box.is_some()
            || self.goto_line.is_some()
            || self.project_search_open
    }

    /// Close any floating editor overlay, so it doesn't linger over the
    /// screen once the keyboard has moved to the tool pane.
    fn close_editor_overlays(&mut self) {
        self.palette = None;
        self.close_search_box(true);
        self.goto_line = None;
        self.hide_project_search();
    }

    /// Send a key press to the focused tool's program, and write back any
    /// reply the terminal makes to it (a cursor report, say).
    fn send_key_to_tool(&mut self, key: KeyEvent) {
        if let Some(tool) = self.tool_pane.active_mut() {
            let bytes = tool.view_mut().handle_key(key);
            tool.write(bytes);
        }
    }

    /// Whether a key is the prefix, Ctrl+], in either of its spellings:
    /// a terminal without the kitty keyboard protocol delivers the byte
    /// it sends as Ctrl+5.
    fn is_prefix(key: KeyEvent) -> bool {
        key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char(']') | KeyCode::Char('5'))
    }

    /// A key pressed with a tool focused: the prefix holds the next key
    /// for the editor, Ctrl+` toggles the shell, and everything else goes
    /// to the program. With no program running (the output tool between
    /// jobs) the editor's keys need no prefix, and Ctrl+D dismisses the
    /// tool as it would end a shell. `confirm` is the confirmation
    /// pending before this key, kept alive through the prefix so that
    /// Ctrl+] Ctrl+Q twice quits with unsaved changes.
    fn handle_tool_key(&mut self, key: KeyEvent, confirm: Option<Confirm>) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if let Some(prefix) = self.prefix.take() {
            if Self::is_prefix(key) {
                self.send_key_to_tool(prefix);
            } else if !(ctrl && self.handle_shared_key(key, confirm)) {
                // Not a key of the editor's: the program gets both.
                self.send_key_to_tool(prefix);
                self.send_key_to_tool(key);
            }
            return;
        }
        if Self::is_prefix(key) {
            self.prefix = Some(key);
            self.confirm = confirm;
            return;
        }
        if ctrl && key.code == KeyCode::Char('`') {
            self.toggle_shell();
            return;
        }
        let running = self.tool_pane.active().is_some_and(Tool::is_running);
        if !running && ctrl {
            if key.code == KeyCode::Char('d') {
                self.dismiss_tool();
                return;
            }
            if self.handle_shared_key(key, confirm) {
                return;
            }
        }
        self.confirm = confirm;
        self.send_key_to_tool(key);
    }

    /// Hide the pane and give the editor the keyboard, as a shell that
    /// exits does; the tool's output is kept for next time.
    fn dismiss_tool(&mut self) {
        self.tool_pane.set_visible(false);
        self.focus = Focus::Editor;
        self.prefix = None;
    }

    /// The Ctrl bindings the editor and, after the prefix, the tool pane
    /// share: everything that opens something or moves between views, as
    /// against the keys that edit the active tab. Returns whether the key
    /// was one of them.
    fn handle_shared_key(&mut self, key: KeyEvent, confirm: Option<Confirm>) -> bool {
        match key.code {
            KeyCode::Char('q') => {
                self.confirm = confirm;
                self.request_quit();
            }
            KeyCode::Char('`') => self.toggle_shell(),
            KeyCode::Char('p') => self.open_command_palette(),
            KeyCode::Char('e') => self.open_modes_palette(),
            KeyCode::Char(',') => self.focus_previous(),
            KeyCode::Char('.') => self.focus_next(),
            KeyCode::Char('t') => self.open_tabs_palette(),
            KeyCode::Char('o') => self.open_files_palette(),
            // With the kitty keyboard protocol Ctrl+Shift+F arrives as a
            // shifted 'F' (or as 'f' with the shift modifier); a plain
            // terminal can't tell it from Ctrl+F.
            KeyCode::Char('F') => self.open_project_search(),
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.open_project_search()
            }
            KeyCode::Char('f') if self.search_box.is_some() => self.upgrade_search_to_project(),
            KeyCode::Char('f') => self.open_search_box(),
            KeyCode::Char('j') => self.open_goto_line(),
            KeyCode::Char('l') => self.open_git_log(),
            KeyCode::Char('u') => self.open_changes(),
            KeyCode::Char('b') => self.build(),
            KeyCode::Char('r') => self.run(),
            _ => return false,
        }
        true
    }

    /// Handle output or the exit of a tool's program.
    fn handle_pty(&mut self, id: SessionId, output: Output) {
        match output {
            Output::Bytes(bytes) => {
                let mut clipboard = Vec::new();
                if let Some(tool) = self.tool_pane.tool_of_session(id) {
                    tool.process(&bytes);
                    let responses = tool.view_mut().terminal_mut().take_responses();
                    tool.write(responses);
                    // Collect the program's requests before dropping the
                    // borrow, since acting on them needs the app.
                    for event in tool.view_mut().terminal_mut().take_events() {
                        if let ninjaedit_core::terminal::Event::Clipboard(text) = event {
                            clipboard.push(text);
                        }
                    }
                }
                for text in clipboard {
                    self.clipboard.set(text);
                }
            }
            Output::Exited(status) => {
                let Some(index) = self.tool_pane.index_of_session(id) else {
                    return;
                };
                let was_active = index == self.tool_pane.active_index();
                let tool = &mut self.tool_pane.tools_mut()[index];
                tool.note_exit(status.clone());
                if tool.kind() == ToolKind::Output {
                    // A job's step ended: on to the next, or say how it
                    // ended. The output stays showing.
                    self.step_exited(index, status);
                } else if was_active && self.tool_pane.is_visible() {
                    // The user quit the shell: give the whole screen back
                    // to the editor. Showing it again starts a fresh
                    // shell.
                    self.tool_pane.set_visible(false);
                    self.focus = Focus::Editor;
                    self.prefix = None;
                }
            }
        }
        self.track_view();
    }

    /// Periodic housekeeping while idle. Returns whether the screen needs
    /// redrawing.
    pub fn tick(&mut self) -> bool {
        let mut redraw = self.check_open_files_if_due();
        redraw |= self.take_discoveries();
        // A selection dragged past the tool's edge keeps scrolling.
        if let Some(tool) = self.tool_pane.active_mut()
            && tool.view_mut().tick()
        {
            redraw = true;
        }
        let polled = match &mut self.mode {
            Mode::GitLog(tabs) => {
                let polled = tabs.poll();
                // A fetch that finished says how it went.
                if let Some(notice) = tabs.take_notice() {
                    self.status = Some(notice);
                }
                polled
            }
            Mode::Changes(tabs) => tabs.poll(),
            _ => false,
        };
        redraw |= polled;
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
        if self.palette_kind == PaletteKind::Files {
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

    /// Handle one loop event, from the terminal or from a program, and
    /// say whether the screen needs redrawing. Focus changes alone don't:
    /// a terminal that reports focus doesn't cost a redraw each time the
    /// window is clicked away and back. Regaining focus is when files
    /// changed elsewhere are most expected to be current, though, so it
    /// checks them.
    pub fn handle_app_event(&mut self, event: AppEvent) -> bool {
        match event {
            AppEvent::Terminal(Event::FocusGained) => self.check_open_files(),
            AppEvent::Terminal(Event::FocusLost) => false,
            AppEvent::Terminal(event) => {
                self.handle_event(event);
                true
            }
            AppEvent::Pty(id, output) => {
                self.handle_pty(id, output);
                true
            }
        }
    }

    pub fn handle_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.handle_key(key),
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            Event::Paste(text) => {
                self.handle_paste(&text);
            }
            _ => {}
        }
        self.track_view();
    }

    fn handle_paste(&mut self, text: &str) {
        if let Some(palette) = &mut self.palette {
            palette.paste(text);
        } else if let Some(search_box) = &mut self.search_box {
            if search_box.paste(text) {
                self.update_search_query();
            }
        } else if let Some(goto_line) = &mut self.goto_line {
            goto_line.paste(text);
        } else if self.project_search_open
            && let Some(dialog) = &mut self.project_search
        {
            dialog.paste(text);
        } else if self.focus == Focus::Tool {
            // Pasting into the shell sends the text as the program
            // reads it, wrapped for bracketed paste if it asked. A
            // prefix waiting for a key goes first, like any other
            // key it isn't followed by one of the editor's.
            if let Some(prefix) = self.prefix.take() {
                self.send_key_to_tool(prefix);
            }
            if let Some(tool) = self.tool_pane.active_mut() {
                let bytes = tool.view_mut().paste(text);
                tool.write(bytes);
            }
        } else if let Mode::Settings(view) = &mut self.mode {
            view.paste(text);
        } else if let Mode::Build(view) = &mut self.mode {
            view.paste(text);
        } else if let Mode::Changes(tabs) = &mut self.mode {
            tabs.paste(text);
        } else if matches!(self.mode, Mode::GitLog(_)) {
            // Nothing on the page takes text.
        } else if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.view.editor_mut().paste(text);
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        self.status = None;
        let confirm = self.confirm.take();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // With the tool pane focused the program gets every key but the
        // prefix and Ctrl+`, so a shell or coding agent has the full
        // keyboard; only an overlay opened after the prefix takes the
        // keyboard from it.
        if self.focus == Focus::Tool && !self.overlay_open() {
            self.handle_tool_key(key, confirm);
            return;
        }

        self.prefix = None;

        // Bindings shared with the tool pane, where they follow the prefix.
        if ctrl && self.handle_shared_key(key, confirm) {
            return;
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
        if let Some(goto_line) = &mut self.goto_line {
            let outcome = goto_line.handle_key(key, &mut self.clipboard);
            self.handle_goto_line_outcome(outcome);
            return;
        }
        if self.project_search_open
            && let Some(dialog) = &mut self.project_search
        {
            let outcome = dialog.handle_key(key, &mut self.clipboard);
            self.handle_project_search_outcome(outcome);
            return;
        }

        // A mode in the editor's place gets the editor's keys.
        if let Mode::Settings(view) = &mut self.mode {
            let outcome = view.handle_key(key, &mut self.clipboard, &mut self.settings);
            self.handle_settings_outcome(outcome);
            return;
        }
        if let Mode::Build(view) = &mut self.mode {
            let outcome = view.handle_key(key, &mut self.clipboard, &mut self.build);
            self.handle_build_outcome(outcome);
            return;
        }
        if let Mode::GitLog(tabs) = &mut self.mode {
            tabs.handle_key(key);
            return;
        }
        if let Mode::Changes(tabs) = &mut self.mode {
            let outcome = tabs.handle_key(key, &mut self.clipboard);
            self.handle_changes_outcome(outcome);
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
        // A click with the prefix waiting is the gesture it was held for:
        // in the tool's terminal it takes the mouse from a program
        // reading it, so a drag selects text, and anywhere else it
        // decides where the next key goes by itself. Either way the
        // prefix is spent rather than left dangling.
        let escaped = self.prefix.is_some();
        if matches!(mouse.kind, MouseEventKind::Down(_)) {
            self.prefix = None;
        }

        // A divider drag owns the mouse until the button comes up.
        if self.dragging_divider {
            match mouse.kind {
                MouseEventKind::Drag(_) => self.resize_split(y),
                MouseEventKind::Up(_) => self.dragging_divider = false,
                _ => {}
            }
            return;
        }

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
        // The go to line box works like the search box.
        if let Some(goto_line) = &mut self.goto_line {
            if goto_line.contains(x, y) || goto_line.is_dragging() {
                goto_line.handle_mouse(mouse);
                return;
            }
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.goto_line = None;
                return;
            }
        }

        // ----- The status bar's configuration and target -----
        let at = ScreenPosition::new(x, y);
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            if self.status_config_area.contains(at) {
                self.open_configuration_palette();
                return;
            }
            if self.status_target_area.contains(at) {
                self.open_target_palette();
                return;
            }
        }

        // ----- The tool pane, when it's showing -----
        if self.tool_pane.is_visible() {
            // Press the divider to start dragging it.
            if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                && self.divider_area.contains(at)
            {
                self.dragging_divider = true;
                self.resize_split(y);
                return;
            }
            // A drag that started in the tool (a selection, or a program
            // tracking the mouse) follows the pointer outside it.
            let tool_dragging = self
                .tool_pane
                .active()
                .is_some_and(|tool| tool.view().is_dragging());
            if tool_dragging
                && matches!(mouse.kind, MouseEventKind::Drag(_) | MouseEventKind::Up(_))
            {
                self.route_tool_mouse(mouse, escaped);
                return;
            }
            // The tool pane's tab bar: switch tools, or close (kill) one.
            if let Some(hit) = self.tool_tab_bar.hit(x, y) {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    self.focus = Focus::Tool;
                    match hit {
                        TabHit::Tab(index) => self.tool_pane.set_active(index),
                        TabHit::Close(index) => self.kill_tool(index),
                    }
                }
                return;
            }
            // The tool's terminal. A click on a source location in its
            // output opens the file instead of going to the program.
            if self.tool_term_area.contains(at) {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                    && let Some(location) = self
                        .tool_pane
                        .active()
                        .and_then(|tool| tool.view().link_at(x, y))
                        .cloned()
                {
                    self.open_source_location(&location);
                    return;
                }
                if matches!(mouse.kind, MouseEventKind::Down(_)) {
                    self.focus = Focus::Tool;
                }
                self.route_tool_mouse(mouse, escaped);
                return;
            }
        }

        // ----- A mode in the editor's place -----
        if !matches!(self.mode, Mode::Editor) {
            // The mode's tab: its close button leaves the mode.
            if let Some(hit) = self.mode_tab_bar.hit(x, y) {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    match hit {
                        TabHit::Close(_) => self.enter_editor(),
                        TabHit::Tab(index) => {
                            // The git pages' tabs are their repositories.
                            match &mut self.mode {
                                Mode::GitLog(tabs) => tabs.set_active(index),
                                Mode::Changes(tabs) => tabs.set_active(index),
                                _ => {}
                            }
                            self.focus = Focus::Editor;
                        }
                    }
                }
                return;
            }
            let pressed = matches!(mouse.kind, MouseEventKind::Down(_));
            match &mut self.mode {
                Mode::Settings(view) if view.contains(x, y) || view.is_dragging() => {
                    if pressed {
                        self.focus = Focus::Editor;
                    }
                    let outcome = view.handle_mouse(mouse, &mut self.settings);
                    self.handle_settings_outcome(outcome);
                }
                Mode::Build(view) if view.contains(x, y) || view.is_dragging() => {
                    if pressed {
                        self.focus = Focus::Editor;
                    }
                    let outcome = view.handle_mouse(mouse, &mut self.build);
                    self.handle_build_outcome(outcome);
                }
                Mode::GitLog(tabs)
                    if tabs
                        .active_view()
                        .is_some_and(|view| view.contains(x, y) || view.is_dragging()) =>
                {
                    if pressed {
                        self.focus = Focus::Editor;
                    }
                    // A resize is kept in the project's storage.
                    if tabs.handle_mouse(mouse)
                        && let Err(err) = git_layout::save(
                            &self.project_storage,
                            git_layout::LAYOUT_FILE,
                            tabs.layout(),
                        )
                    {
                        self.status = Some(format!(
                            "Could not save {}: {err}",
                            self.project_storage.path(git_layout::LAYOUT_FILE).display()
                        ));
                    }
                }
                Mode::Changes(tabs)
                    if tabs
                        .active_view()
                        .is_some_and(|view| view.contains(x, y) || view.is_dragging()) =>
                {
                    if pressed {
                        self.focus = Focus::Editor;
                    }
                    if tabs.handle_mouse(mouse)
                        && let Err(err) = git_layout::save(
                            &self.project_storage,
                            git_layout::CHANGES_LAYOUT_FILE,
                            tabs.layout(),
                        )
                    {
                        self.status = Some(format!(
                            "Could not save {}: {err}",
                            self.project_storage
                                .path(git_layout::CHANGES_LAYOUT_FILE)
                                .display()
                        ));
                    }
                }
                _ => {}
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
                self.focus = Focus::Editor;
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
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.focus = Focus::Editor;
            }
            tab.view.handle_mouse(mouse);
        }
    }

    /// Send a mouse event to the focused tool's view, writing any report
    /// it makes to the program and copying any text a selection ended
    /// with. `escaped` says the prefix was waiting when the event came,
    /// which takes the mouse from a program reading it.
    fn route_tool_mouse(&mut self, mouse: MouseEvent, escaped: bool) {
        let Some(tool) = self.tool_pane.active_mut() else {
            return;
        };
        let bytes = tool.view_mut().handle_mouse(mouse, escaped);
        let copied = tool.view_mut().take_copied();
        tool.write(bytes);
        if let Some(text) = copied {
            let lines = text.lines().count();
            self.status = Some(if lines == 1 {
                "Copied 1 line to the clipboard".to_owned()
            } else {
                format!("Copied {lines} lines to the clipboard")
            });
            self.clipboard.set(text);
        }
    }

    /// Kill a tool's program. Its exit, reported through the event
    /// channel, hides the pane if it was the active tool.
    fn kill_tool(&mut self, index: usize) {
        if let Some(tool) = self.tool_pane.tools_mut().get_mut(index) {
            tool.kill();
        }
    }

    /// Move the editor/tool divider so it sits at screen row `y`, giving
    /// the tool pane the space below it.
    fn resize_split(&mut self, y: u16) {
        let content = self.content_area;
        if content.height == 0 {
            return;
        }
        let y = y.clamp(content.y, content.bottom());
        let tool_rows = content.bottom().saturating_sub(y);
        self.tool_pane
            .set_split(tool_rows as f32 / content.height as f32);
    }

    // ----- Rendering ------------------------------------------------------

    pub fn render(&mut self, frame: &mut Frame) {
        let screen = frame.area();
        if screen.height < 2 {
            return;
        }
        let tab_area = Rect::new(screen.x, screen.y, screen.width, 1);
        let status_area = Rect::new(screen.x, screen.bottom() - 1, screen.width, 1);
        // Between the tab bar and the status bar sit the editor and, when
        // it's showing, the tool pane below it, split by a draggable
        // divider.
        let content = Rect::new(screen.x, screen.y + 1, screen.width, screen.height - 2);
        self.content_area = content;
        self.layout_content(content);

        let buf = frame.buffer_mut();
        let theme = &self.theme;
        let editor_focused = self.focus == Focus::Editor;

        let mut cursor = None;
        match &mut self.mode {
            Mode::Editor => {
                let labels: Vec<TabLabel> = self
                    .tabs
                    .iter()
                    .map(|tab| TabLabel {
                        title: tab.title(),
                        modified: tab.view.editor().is_modified(),
                    })
                    .collect();
                self.tab_bar
                    .render(tab_area, buf, theme, &labels, self.active, editor_focused);
                match self.tabs.get_mut(self.active) {
                    Some(tab) => {
                        // The search box, drawn later, sits over the top
                        // of the editor, and the editor is showing search
                        // matches.
                        let covered = if self.search_box.is_some() {
                            search_box::HEIGHT
                        } else {
                            0
                        };
                        tab.view.set_covered_rows(covered);
                        cursor = tab.view.render(self.editor_area, buf, theme);
                    }
                    None => render_empty(self.editor_area, buf, theme, &self.project),
                }
            }
            Mode::Settings(view) => {
                let label = TabLabel {
                    title: settings_view::TITLE.to_owned(),
                    modified: false,
                };
                self.mode_tab_bar
                    .render(tab_area, buf, theme, &[label], 0, editor_focused);
                cursor = view.render(self.editor_area, buf, theme, &self.settings);
            }
            Mode::Build(view) => {
                let label = TabLabel {
                    title: build_view::TITLE.to_owned(),
                    modified: false,
                };
                self.mode_tab_bar
                    .render(tab_area, buf, theme, &[label], 0, editor_focused);
                cursor = view.render(self.editor_area, buf, theme, &self.build);
            }
            Mode::GitLog(tabs) => {
                let labels: Vec<TabLabel> = tabs
                    .titles()
                    .map(|title| TabLabel {
                        title: title.to_owned(),
                        modified: false,
                    })
                    .collect();
                let active = tabs.active_index();
                self.mode_tab_bar
                    .render(tab_area, buf, theme, &labels, active, editor_focused);
                tabs.active().render(self.editor_area, buf, theme);
            }
            Mode::Changes(tabs) => {
                let labels: Vec<TabLabel> = tabs
                    .titles()
                    .map(|title| TabLabel {
                        title: title.to_owned(),
                        modified: false,
                    })
                    .collect();
                let active = tabs.active_index();
                self.mode_tab_bar
                    .render(tab_area, buf, theme, &labels, active, editor_focused);
                cursor = tabs.render(self.editor_area, buf, theme);
            }
        }

        // The tool pane: the divider, the tool tab bar, and the active
        // tool's terminal.
        let mut tool_cursor = None;
        if self.tool_pane.is_visible() && self.tool_term_area.height > 0 {
            render_divider(self.divider_area, buf, theme);
            let tool_labels: Vec<TabLabel> = self
                .tool_pane
                .tools()
                .iter()
                .map(|tool| TabLabel {
                    title: tool.title(),
                    modified: false,
                })
                .collect();
            let tool_tab_area = Rect::new(
                self.divider_area.x,
                self.divider_area.bottom(),
                self.divider_area.width,
                1,
            );
            let active = self.tool_pane.active_index();
            self.tool_tab_bar.render(
                tool_tab_area,
                buf,
                theme,
                &tool_labels,
                active,
                !editor_focused,
            );
            let area = self.tool_term_area;
            if let Some(tool) = self.tool_pane.active_mut() {
                tool_cursor = tool.view_mut().render(area, buf, theme);
                // The pty follows the size the render settled on.
                tool.sync_size();
            }
        }

        self.render_status(status_area, buf);
        let theme = &self.theme;

        if let Some(palette) = &mut self.palette {
            cursor = palette.render(screen, buf, theme);
        }
        let hint = self.search_hint();
        if let Some(search_box) = &mut self.search_box {
            search_box.set_hint(hint);
            cursor = search_box.render(screen, buf, theme);
        }
        if let Some(goto_line) = &mut self.goto_line {
            cursor = goto_line.render(screen, buf, theme);
        }
        if self.project_search_open
            && let Some(dialog) = &mut self.project_search
        {
            cursor = dialog.render(screen, buf, theme);
        }
        // With the tool focused the tool's cursor is the one to show,
        // unless an overlay is open over it; otherwise the editor's or an
        // overlay's.
        let show = if self.focus == Focus::Tool && !self.overlay_open() {
            tool_cursor
        } else {
            cursor
        };
        if let Some(position) = show {
            frame.set_cursor_position(position);
        }
    }

    /// Place the editor and the tool pane within the content region,
    /// setting the areas the render draws into and the mouse hit-tests
    /// against. With the tool pane hidden the editor takes it all.
    fn layout_content(&mut self, content: Rect) {
        // The tool pane needs at least a tab row and a terminal row, plus
        // the divider, and the editor at least one row.
        let fits = self.tool_pane.is_visible() && content.height >= 4;
        if !fits {
            self.editor_area = content;
            self.divider_area = Rect::default();
            self.tool_term_area = Rect::default();
            return;
        }
        let total = content.height;
        let tool_rows =
            ((self.tool_pane.split() * total as f32).round() as u16).clamp(2, total - 2);
        let editor_height = total - DIVIDER_HEIGHT - tool_rows;
        self.editor_area = Rect::new(content.x, content.y, content.width, editor_height);
        self.divider_area = Rect::new(
            content.x,
            content.y + editor_height,
            content.width,
            DIVIDER_HEIGHT,
        );
        let tool_top = content.y + editor_height + DIVIDER_HEIGHT;
        self.tool_term_area = Rect::new(content.x, tool_top + 1, content.width, tool_rows - 1);
    }

    fn render_status(&mut self, area: Rect, buf: &mut Buffer) {
        let theme = &self.theme;
        let base = Style::default().bg(theme.status_bar_background);
        buf.set_style(area, base);
        self.status_config_area = Rect::default();
        self.status_target_area = Rect::default();
        let tab = self.tabs.get(self.active);
        // With the tool pane focused the position report is about the
        // tool, not the editor: nothing to show, unless the user has
        // scrolled back through the shell's output.
        let tool_focused = self.focus == Focus::Tool && self.tool_pane.is_visible();
        let in_editor = matches!(self.mode, Mode::Editor);
        let position = if tool_focused {
            self.tool_pane.active().and_then(|tool| {
                let back = tool.view().scrollback_offset();
                (back > 0).then(|| format!(" scrollback −{back} "))
            })
        } else if !in_editor {
            None
        } else {
            tab.map(|tab| {
                let position = tab.view.editor().cursor_position();
                format!(" Ln {}, Col {} ", position.line + 1, position.column + 1)
            })
        };
        // The current build configuration and target, each a segment
        // the mouse can click to pick another.
        let mut configuration = self
            .build
            .current_configuration()
            .map(|(_, c)| format!(" {} ", c.name))
            .unwrap_or_default();
        // While the current root's targets are still being found, the
        // target is the name the file had, or whatever stands in, with
        // an ellipsis to say so.
        let finding = self
            .build
            .current_configuration()
            .is_some_and(|(root, _)| root.is_pending());
        let mut target = self
            .build
            .pending_target()
            .map(str::to_owned)
            .or_else(|| self.build.current_target().map(|(_, t)| t.name.clone()))
            .map(|name| {
                if finding {
                    format!("▸ {name}… ")
                } else {
                    format!("▸ {name} ")
                }
            })
            .unwrap_or_else(|| {
                if finding {
                    "▸ … ".to_owned()
                } else {
                    String::new()
                }
            });
        let width_of = |text: &str| Span::raw(text).width() as u16;
        let position_width = position.as_deref().map_or(0, width_of);
        // Drop the build segments when there's no room for them beside
        // the position.
        if position_width + width_of(&configuration) + width_of(&target) + 20 > area.width {
            configuration.clear();
            target.clear();
        }
        let right = [
            (position.unwrap_or_default(), theme.status_bar_position_text),
            (configuration, theme.status_bar_position_text),
            (target, theme.status_bar_position_text),
        ];
        let mode_hint = match &self.mode {
            Mode::Editor => String::new(),
            Mode::Settings(_) => SETTINGS_HINT.to_owned(),
            Mode::Build(_) => BUILD_HINT.to_owned(),
            Mode::GitLog(tabs) => tabs.hint(),
            Mode::Changes(tabs) => tabs.hint(),
        };
        let left = match &self.status {
            _ if self.prefix.is_some() => PREFIX_HINT.to_owned(),
            Some(message) => message.clone(),
            None if tool_focused => match self.tool_pane.active() {
                Some(tool) if tool.kind() == ToolKind::Output && !tool.is_running() => {
                    OUTPUT_IDLE_HINT.to_owned()
                }
                Some(tool) => tool.title(),
                None => String::new(),
            },
            None if !in_editor => mode_hint,
            None => match tab.and_then(Tab::path) {
                Some(path) => self.display_path(path),
                None => "Ctrl+O to open a file, Ctrl+Q to quit".to_owned(),
            },
        };
        let right_width: u16 = right.iter().map(|(text, _)| width_of(text)).sum();
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
            for (index, (text, color)) in right.iter().enumerate() {
                let width = width_of(text);
                buf.set_string(x, area.y, text, base.fg(*color));
                let segment = Rect::new(x, area.y, width, 1);
                match index {
                    1 => self.status_config_area = segment,
                    2 => self.status_target_area = segment,
                    _ => {}
                }
                x += width;
            }
        }
    }
}

/// Run a command in a pty, routing its output to the application's event
/// channel so the loop wakes when the program writes or exits.
fn spawn_session(
    command: &ninjaedit_core::terminal::Command,
    cols: u16,
    rows: u16,
    events: Sender<AppEvent>,
) -> std::io::Result<Session> {
    Session::spawn(command, cols as usize, rows as usize, move |id, output| {
        // The loop has ended once the receiver is gone; nothing to do.
        let _ = events.send(AppEvent::Pty(id, output));
    })
}

/// Draw the draggable divider between the editor and the tool pane.
fn render_divider(area: Rect, buf: &mut Buffer, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let style = Style::default()
        .fg(theme.scroll_bar_track)
        .bg(theme.view_background);
    for x in area.x..area.right() {
        buf[(x, area.y)].set_symbol("─").set_style(style);
    }
}

/// The name of the editor, as shown on the empty page and in the
/// terminal's title.
pub const EDITOR_NAME: &str = "Ninja Edit";

/// What's shown on the empty page under the editor's name when the
/// directory the editor was started in isn't part of a project.
const NOT_A_PROJECT: &str = "not a project: only this directory's own files are indexed";

/// The page shown where the editor would be when no file is open: the
/// editor's name, where it was started (the project, or the directory
/// when there isn't a project), and the keys to get going with.
fn render_empty(area: Rect, buf: &mut Buffer, theme: &Theme, project: &Project) {
    buf.set_style(area, Style::default().bg(theme.view_background));
    if area.height == 0 {
        return;
    }
    // A path too wide for the view keeps its end, the part that says
    // where this is, behind an ellipsis.
    let mut location = project.root().display().to_string();
    let width = area.width as usize;
    let chars = location.chars().count();
    if chars > width && width > 0 {
        location = std::iter::once('…')
            .chain(location.chars().skip(chars + 1 - width))
            .collect();
    }
    let (open_hint, note) = match project.kind() {
        ProjectKind::Project => ("open a project file", None),
        ProjectKind::Directory => ("open a file in this directory", Some(NOT_A_PROJECT)),
    };
    let keys: [(&str, &str); 12] = [
        ("Ctrl+O", open_hint),
        ("Ctrl+Shift+F", "search in project files"),
        ("Ctrl+T", "switch between open tabs"),
        ("Ctrl+L", "browse the git history"),
        ("Ctrl+U", "review, stage, and commit changes"),
        ("Ctrl+`", "open a shell below the editor"),
        ("Ctrl+B", "build the current target"),
        ("Ctrl+R", "run the current target"),
        ("Ctrl+E", "switch views: editor, pages, tools"),
        ("Ctrl+P", "search every command and its key"),
        ("Ctrl+]", "in a tool, prefix for the editor's keys"),
        ("Ctrl+Q", "quit"),
    ];
    // The keys read as a table: every key is padded to the widest one so
    // the descriptions line up in a column, and the whole table is
    // centered as one block rather than row by row.
    let key_column = keys
        .iter()
        .map(|(key, _)| Span::raw(*key).width())
        .max()
        .unwrap_or(0)
        + 3;
    let table: Vec<String> = keys
        .iter()
        .map(|(key, description)| format!("{key:<key_column$}{description}"))
        .collect();
    let table_width = table
        .iter()
        .map(|row| Span::raw(row.as_str()).width())
        .max()
        .unwrap_or(0) as u16;

    enum Line<'a> {
        Heading(&'a str),
        Centered(&'a str),
        Row(&'a str),
    }
    let mut lines = vec![Line::Heading(EDITOR_NAME), Line::Centered(&location)];
    lines.extend(note.map(Line::Centered));
    lines.extend([
        Line::Centered(""),
        Line::Centered("No files open"),
        Line::Centered(""),
    ]);
    lines.extend(table.iter().map(|row| Line::Row(row)));

    let top = area.y + area.height.saturating_sub(lines.len() as u16) / 2;
    let base = Style::default()
        .fg(theme.view_text)
        .bg(theme.view_background);
    let table_x = area.x + area.width.saturating_sub(table_width) / 2;
    for (i, line) in lines.iter().enumerate() {
        let y = top + i as u16;
        if y >= area.bottom() {
            break;
        }
        let (text, x, style) = match line {
            Line::Heading(text) => {
                let width = Span::raw(*text).width() as u16;
                let x = area.x + area.width.saturating_sub(width) / 2;
                (*text, x, base.add_modifier(Modifier::BOLD))
            }
            Line::Centered(text) => {
                let width = Span::raw(*text).width() as u16;
                let x = area.x + area.width.saturating_sub(width) / 2;
                (*text, x, base.add_modifier(Modifier::DIM))
            }
            Line::Row(text) => (*text, table_x, base.add_modifier(Modifier::DIM)),
        };
        buf.set_stringn(x, y, text, area.width as usize, style);
    }
}

/// The selected text, when the selection lies within one line: what a
/// search opened over it starts out looking for.
fn single_line_selection(editor: &Editor) -> Option<String> {
    editor
        .selected_text()
        .filter(|text| !text.contains(['\n', '\r']))
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
    use ninjaedit_core::Position;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use std::time::Duration;

    fn app_with_files(files: &[(&str, &str)]) -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        for (name, contents) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        let (events, _events_rx) = std::sync::mpsc::channel();
        let storage = Storage::new(dir.path().join(".storage"));
        let mut app = App::new(Project::open(dir.path()).unwrap(), storage, events);
        app.wait_for_discovery();
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

    fn mouse_at(app: &mut App, kind: MouseEventKind, column: u16, row: u16) {
        app.handle_event(Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }));
    }

    fn settings_file(app: &App) -> Option<String> {
        app.storage
            .read(ninjaedit_core::storage::SETTINGS_FILE)
            .unwrap()
    }

    #[test]
    fn settings_page_opens_from_the_modes_palette_and_saves_changes() {
        let (dir, mut app) = app_with_files(&[("a.txt", "hi\n")]);
        draw(&mut app, 80, 20);

        // The settings come last in a fresh palette, so Ctrl+E, Enter
        // still opens the shell; typing finds them.
        ctrl(&mut app, 'e');
        let screen = draw(&mut app, 80, 20);
        assert!(screen[3].contains(EDITOR_MODE_LABEL), "{screen:#?}");
        assert!(screen[4].contains("Shell"), "{screen:#?}");
        assert!(screen[5].contains("Output"), "{screen:#?}");
        assert!(screen[6].contains(git_view::TITLE), "{screen:#?}");
        assert!(screen[7].contains(changes_view::TITLE), "{screen:#?}");
        assert!(screen[8].contains(build_view::TITLE), "{screen:#?}");
        assert!(screen[9].contains(settings_view::TITLE), "{screen:#?}");
        type_str(&mut app, "sett");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Settings(_)));
        assert_eq!(app.focus, Focus::Editor);
        let screen = draw(&mut app, 80, 20);
        assert!(screen[0].contains(settings_view::TITLE), "{screen:#?}");
        assert!(
            !screen[0].contains("a.txt"),
            "the mode's tab replaces the editor's"
        );
        assert!(
            screen.iter().any(|r| r.contains("Shell executable")),
            "{screen:#?}"
        );
        assert!(screen[19].contains("Ctrl+D default"), "{screen:#?}");
        assert!(!screen[19].contains("Ln 1"), "{screen:#?}");

        // Down to the scrollback field, a new value, Enter: applied and
        // saved to the storage, where nothing was before.
        assert_eq!(settings_file(&app), None);
        press(&mut app, KeyCode::Down);
        ctrl(&mut app, 'a');
        type_str(&mut app, "250");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.terminal_scrollback(), 250);
        let file = settings_file(&app).expect("saved");
        assert!(file.contains("scrollback = 250"), "{file}");
        let screen = draw(&mut app, 80, 20);
        assert!(
            screen
                .iter()
                .any(|r| r.contains("Scrollback lines") && r.contains("modified")),
            "{screen:#?}"
        );

        // Ctrl+E now lists the settings first and the editor, where the
        // user came from, second: Enter goes back to it. A value typed but
        // not applied is applied on the way out.
        press(&mut app, KeyCode::Down);
        ctrl(&mut app, 'a');
        type_str(&mut app, "77");
        ctrl(&mut app, 'e');
        let screen = draw(&mut app, 80, 20);
        assert!(screen[3].contains(settings_view::TITLE), "{screen:#?}");
        assert!(screen[4].contains(EDITOR_MODE_LABEL), "{screen:#?}");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Editor));
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(app.settings.search_max_results(), 77);
        let screen = draw(&mut app, 80, 20);
        assert!(screen[0].contains("a.txt"), "{screen:#?}");

        // A new app on the same storage reads the settings back.
        let (events, _events_rx) = std::sync::mpsc::channel();
        let storage = Storage::new(dir.path().join(".storage"));
        let again = App::new(Project::open(dir.path()).unwrap(), storage, events);
        assert_eq!(again.settings.terminal_scrollback(), 250);
        assert_eq!(again.settings.search_max_results(), 77);
        assert_eq!(again.status, None);
    }

    #[test]
    fn ctrl_d_resets_and_ctrl_o_or_ctrl_t_leave_the_settings_page() {
        let (dir, mut app) = app_with_files(&[("a.txt", "hi\n"), ("b.txt", "yo\n")]);
        app.open_settings();
        type_str(&mut app, "/bin/dash");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.shell(), Some("/bin/dash"));
        assert!(settings_file(&app).unwrap().contains("/bin/dash"));
        ctrl(&mut app, 'd');
        assert_eq!(app.settings.shell(), None);
        assert!(!settings_file(&app).unwrap().contains("shell"));

        // Ctrl+T's tab switch brings the editor back.
        ctrl(&mut app, 't');
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Editor));
        assert_eq!(app.tabs[app.active].title(), "a.txt");

        // As does opening a file with Ctrl+O.
        app.open_settings();
        assert!(matches!(app.mode, Mode::Settings(_)));
        assert!(
            app.project
                .index()
                .wait_for_primary(Duration::from_secs(10))
        );
        ctrl(&mut app, 'o');
        type_str(&mut app, "b.txt");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Editor));
        assert_eq!(app.tabs[app.active].title(), "b.txt");

        // The editor's own keys do nothing to the hidden tabs from the
        // page: Ctrl+W closes no tab, Ctrl+J opens no box.
        app.open_settings();
        ctrl(&mut app, 'w');
        assert_eq!(app.tabs.len(), 2);
        ctrl(&mut app, 'j');
        assert!(app.goto_line.is_none());
        // Closing the page's tab with the mouse leaves it too.
        let screen = draw(&mut app, 60, 20);
        let close = screen[0].chars().position(|c| c == '×').unwrap() as u16;
        click(&mut app, close, 0);
        assert!(matches!(app.mode, Mode::Editor));
        drop(dir);
    }

    #[test]
    fn ctrl_u_opens_the_changes_page_with_a_tab_per_changed_submodule() {
        let (dir, mut app) = app_with_files(&[("a.txt", "hi\n")]);
        // A repository with one commit (of the open file and another),
        // a submodule with a commit of its own, and then a change in
        // each: to the file that isn't open, so the editor stays quiet.
        let repo = git2::Repository::init(dir.path()).unwrap();
        {
            let mut config = repo.config().unwrap();
            config.set_str("user.name", "Ann").unwrap();
            config.set_str("user.email", "ann@example.com").unwrap();
        }
        std::fs::write(dir.path().join("b.txt"), "hi\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("a.txt")).unwrap();
        index.add_path(Path::new("b.txt")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("Ann", "ann@example.com").unwrap();
        let first = repo
            .commit(Some("HEAD"), &sig, &sig, "Say hi", &tree, &[])
            .unwrap();
        let mut submodule = repo
            .submodule("https://example.com/sub.git", Path::new("libs/sub"), true)
            .unwrap();
        let sub_workdir = {
            let sub = submodule.open().unwrap();
            {
                let mut config = sub.config().unwrap();
                config.set_str("user.name", "Ann").unwrap();
                config.set_str("user.email", "ann@example.com").unwrap();
            }
            std::fs::write(sub.workdir().unwrap().join("inner.txt"), "inner\n").unwrap();
            let mut index = sub.index().unwrap();
            index.add_path(Path::new("inner.txt")).unwrap();
            index.write().unwrap();
            let tree = sub.find_tree(index.write_tree().unwrap()).unwrap();
            sub.commit(Some("HEAD"), &sig, &sig, "Inner commit", &tree, &[])
                .unwrap();
            sub.workdir().unwrap().to_path_buf()
        };
        submodule.add_finalize().unwrap();
        let mut index = repo.index().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = repo.find_commit(first).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "Add submodule", &tree, &[&parent])
            .unwrap();
        std::fs::write(dir.path().join("b.txt"), "hi there\n").unwrap();
        std::fs::write(sub_workdir.join("inner.txt"), "inner changed\n").unwrap();

        let settle = |app: &mut App| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while let Mode::Changes(tabs) = &app.mode
                && tabs.is_loading()
                && Instant::now() < deadline
            {
                app.tick();
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(matches!(&app.mode, Mode::Changes(tabs) if !tabs.is_loading()));
        };
        ctrl(&mut app, 'u');
        assert!(matches!(app.mode, Mode::Changes(_)));
        assert_eq!(app.focus, Focus::Editor);
        settle(&mut app);
        let screen = draw(&mut app, 100, 24);
        assert!(screen[0].contains(changes_view::TITLE), "{screen:#?}");
        assert!(
            screen[0].contains("libs/sub"),
            "a tab for the changed submodule: {screen:#?}"
        );
        assert!(screen.iter().any(|r| r.contains("M b.txt")), "{screen:#?}");
        assert!(
            screen
                .iter()
                .any(|r| r.contains("M sub") && r.contains("submodule")),
            "{screen:#?}"
        );
        assert!(screen[23].contains("Space stage"), "{screen:#?}");
        // The editor's keys don't reach the hidden tab; Ctrl+U again
        // scans afresh and leaves the page up.
        ctrl(&mut app, 'j');
        assert!(app.goto_line.is_none());
        ctrl(&mut app, 'u');
        assert!(matches!(app.mode, Mode::Changes(_)));
        settle(&mut app);
        // Ctrl+T lists the repositories with the submodule preselected;
        // on its tab, `a` stages its change and Enter in the commit box
        // commits it, after which its tab is gone and the main
        // repository has the new commit to stage.
        ctrl(&mut app, 't');
        let screen = draw(&mut app, 100, 24);
        assert!(screen[3].contains(changes_view::TITLE), "{screen:#?}");
        assert!(screen[4].contains("libs/sub"), "{screen:#?}");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(&app.mode, Mode::Changes(tabs) if tabs.active_index() == 1));
        settle(&mut app);
        let screen = draw(&mut app, 100, 24);
        assert!(
            screen.iter().any(|r| r.contains("M inner.txt")),
            "{screen:#?}"
        );
        press(&mut app, KeyCode::Char('a'));
        settle(&mut app);
        // Tab round the staged list and the diff to the commit box.
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "Inner change");
        ctrl(&mut app, 's');
        assert!(
            app.status
                .as_deref()
                .is_some_and(|s| s.starts_with("Committed")),
            "{:?}",
            app.status
        );
        settle(&mut app);
        assert!(matches!(&app.mode, Mode::Changes(tabs) if tabs.active_index() == 0));
        let screen = draw(&mut app, 100, 24);
        assert!(
            !screen[0].contains("libs/sub"),
            "the submodule is clean: {screen:#?}"
        );
        assert!(
            screen
                .iter()
                .any(|r| r.contains("M sub") && r.contains("submodule")),
            "{screen:#?}"
        );
        // Dragging the rule between the lists keeps the sizes in the
        // project's storage, and they come back when the page is
        // reopened.
        let rule = match &app.mode {
            Mode::Changes(tabs) => tabs.active_view().unwrap().lists_rule(),
            _ => unreachable!(),
        };
        click(&mut app, rule.x + 3, rule.y);
        mouse_at(
            &mut app,
            MouseEventKind::Drag(MouseButton::Left),
            rule.x + 3,
            rule.y + 3,
        );
        mouse_at(
            &mut app,
            MouseEventKind::Up(MouseButton::Left),
            rule.x + 3,
            rule.y + 3,
        );
        let file = app
            .project_storage
            .read(git_layout::CHANGES_LAYOUT_FILE)
            .unwrap()
            .expect("kept");
        assert!(
            file.contains("[\".\"]") && file.contains("unstaged = "),
            "{file}"
        );
        draw(&mut app, 100, 24);
        // `o` on b.txt (the last row: the `libs` directory and its
        // submodule come first) opens it in the editor, in the editor's
        // place; Ctrl+U comes back to the page, sized as it was left.
        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Char('o'));
        assert!(matches!(app.mode, Mode::Editor));
        assert!(
            app.tabs[app.active]
                .path()
                .is_some_and(|p| p.ends_with("b.txt")),
            "the file is the active tab"
        );
        ctrl(&mut app, 'u');
        settle(&mut app);
        draw(&mut app, 100, 24);
        let restored = match &app.mode {
            Mode::Changes(tabs) => tabs.active_view().unwrap().lists_rule(),
            _ => unreachable!(),
        };
        assert_eq!(restored.y, rule.y + 3);
        // The page is in the modes palette and the command palette with
        // its key, and Ctrl+] Ctrl+U opens it from the shell too.
        ctrl(&mut app, 'p');
        type_str(&mut app, "git changes");
        let screen = draw(&mut app, 100, 24);
        assert!(
            screen[3].contains(changes_view::TITLE) && screen[3].contains("Ctrl+U"),
            "{screen:#?}"
        );
        press(&mut app, KeyCode::Esc);
        ctrl(&mut app, '`');
        assert_eq!(app.focus, Focus::Tool);
        ctrl(&mut app, ']');
        ctrl(&mut app, 'u');
        assert!(matches!(app.mode, Mode::Changes(_)));
        assert_eq!(app.focus, Focus::Editor);
        drop(dir);
    }

    #[test]
    fn ctrl_l_opens_the_git_log_page_and_leaving_brings_the_editor_back() {
        let (dir, mut app) = app_with_files(&[("a.txt", "hi\n")]);
        // Make the project a repository with one commit.
        let repo = git2::Repository::init(dir.path()).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("a.txt")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("Ann", "ann@example.com").unwrap();
        let first = repo
            .commit(Some("HEAD"), &sig, &sig, "Say hi", &tree, &[])
            .unwrap();
        // And a submodule with a commit of its own.
        let mut submodule = repo
            .submodule("https://example.com/sub.git", Path::new("libs/sub"), true)
            .unwrap();
        {
            let sub = submodule.open().unwrap();
            std::fs::write(sub.workdir().unwrap().join("inner.txt"), "inner\n").unwrap();
            let mut index = sub.index().unwrap();
            index.add_path(Path::new("inner.txt")).unwrap();
            index.write().unwrap();
            let tree = sub.find_tree(index.write_tree().unwrap()).unwrap();
            sub.commit(Some("HEAD"), &sig, &sig, "Inner commit", &tree, &[])
                .unwrap();
        }
        submodule.add_finalize().unwrap();
        let mut index = repo.index().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = repo.find_commit(first).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "Add submodule", &tree, &[&parent])
            .unwrap();

        ctrl(&mut app, 'l');
        assert!(matches!(app.mode, Mode::GitLog(_)));
        assert_eq!(app.focus, Focus::Editor);
        let deadline = Instant::now() + Duration::from_secs(10);
        while let Mode::GitLog(tabs) = &app.mode
            && tabs.is_loading()
            && Instant::now() < deadline
        {
            app.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
        let screen = draw(&mut app, 100, 24);
        assert!(screen[0].contains(git_view::TITLE), "{screen:#?}");
        assert!(
            screen[0].contains("libs/sub"),
            "a tab per submodule: {screen:#?}"
        );
        assert!(screen.iter().any(|r| r.contains("Say hi")), "{screen:#?}");
        assert!(
            screen.iter().any(|r| r.contains("Description")),
            "{screen:#?}"
        );
        assert!(screen[23].contains("Tab pane"), "{screen:#?}");
        // Ctrl+L again leaves the page as it is; the editor's keys don't
        // reach the hidden tab.
        ctrl(&mut app, 'l');
        assert!(matches!(app.mode, Mode::GitLog(_)));
        ctrl(&mut app, 'j');
        assert!(app.goto_line.is_none());
        // Ctrl+T on the page lists its repositories, not the editor's
        // tabs, with the submodule preselected; picking it shows the
        // submodule's history on its tab.
        ctrl(&mut app, 't');
        let screen = draw(&mut app, 100, 24);
        assert!(screen[3].contains(git_view::TITLE), "{screen:#?}");
        assert!(screen[4].contains("libs/sub"), "{screen:#?}");
        assert!(!screen.iter().any(|r| r.contains("a.txt")), "{screen:#?}");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(&app.mode, Mode::GitLog(tabs) if tabs.active_index() == 1));
        let deadline = Instant::now() + Duration::from_secs(10);
        while let Mode::GitLog(tabs) = &app.mode
            && tabs.is_loading()
            && Instant::now() < deadline
        {
            app.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
        let screen = draw(&mut app, 100, 24);
        assert!(
            screen.iter().any(|r| r.contains("Inner commit")),
            "{screen:#?}"
        );
        assert!(!screen.iter().any(|r| r.contains("Say hi")), "{screen:#?}");
        // Clicking the main tab goes back to the project's history.
        let x = screen[0].chars().position(|c| c == 'G').unwrap() as u16;
        click(&mut app, x, 0);
        assert!(matches!(&app.mode, Mode::GitLog(tabs) if tabs.active_index() == 0));
        let screen = draw(&mut app, 100, 24);
        assert!(screen.iter().any(|r| r.contains("Say hi")), "{screen:#?}");
        // Dragging the rule under the log keeps the sizes in the
        // project's storage, and they come back when the page is
        // reopened.
        let rule = match &app.mode {
            Mode::GitLog(tabs) => tabs.active_view().unwrap().log_rule(),
            _ => unreachable!(),
        };
        click(&mut app, rule.x + 3, rule.y);
        mouse_at(
            &mut app,
            MouseEventKind::Drag(MouseButton::Left),
            rule.x + 3,
            rule.y + 4,
        );
        mouse_at(
            &mut app,
            MouseEventKind::Up(MouseButton::Left),
            rule.x + 3,
            rule.y + 4,
        );
        let file = app
            .project_storage
            .read(git_layout::LAYOUT_FILE)
            .unwrap()
            .expect("kept");
        assert!(
            file.contains("[\".\"]") && file.contains("log = "),
            "{file}"
        );
        assert!(
            !file.contains("libs/sub"),
            "only the resized repository: {file}"
        );
        let screen = draw(&mut app, 100, 24);
        let moved = match &app.mode {
            Mode::GitLog(tabs) => tabs.active_view().unwrap().log_rule(),
            _ => unreachable!(),
        };
        assert_eq!(moved.y, rule.y + 4, "{screen:#?}");
        // The modes palette brings the editor back.
        ctrl(&mut app, 'e');
        type_str(&mut app, EDITOR_MODE_LABEL);
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Editor));
        ctrl(&mut app, 'l');
        draw(&mut app, 100, 24);
        let restored = match &app.mode {
            Mode::GitLog(tabs) => tabs.active_view().unwrap().log_rule(),
            _ => unreachable!(),
        };
        assert_eq!(restored.y, rule.y + 4);
        // The page is in the modes palette and the command palette with
        // its key.
        ctrl(&mut app, 'p');
        type_str(&mut app, "git log");
        let screen = draw(&mut app, 100, 24);
        assert!(
            screen[3].contains(git_view::TITLE) && screen[3].contains("Ctrl+L"),
            "{screen:#?}"
        );
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::GitLog(_)));
        // Ctrl+] Ctrl+L opens it from the shell too.
        ctrl(&mut app, '`');
        assert_eq!(app.focus, Focus::Tool);
        ctrl(&mut app, ']');
        ctrl(&mut app, 'l');
        assert!(matches!(app.mode, Mode::GitLog(_)));
        assert_eq!(app.focus, Focus::Editor);
        drop(dir);
    }

    #[test]
    fn the_git_log_comes_back_as_it_was_left_and_refreshes_behind_it() {
        let (dir, mut app) = app_with_files(&[("a.txt", "hello\n")]);
        let repo = git2::Repository::init(dir.path()).unwrap();
        let sig = git2::Signature::now("Ann", "ann@example.com").unwrap();
        let commit = |name: &str, content: &str, message: &str| {
            std::fs::write(dir.path().join(name), content).unwrap();
            let mut index = repo.index().unwrap();
            index.add_path(Path::new(name)).unwrap();
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let parents: Vec<git2::Commit<'_>> = repo
                .head()
                .ok()
                .and_then(|head| head.target())
                .map(|id| repo.find_commit(id).unwrap())
                .into_iter()
                .collect();
            let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
            repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &refs)
                .unwrap()
        };
        let first = commit("a.txt", "hello\n", "Say hello");
        commit("b.txt", "there\n", "Say there");
        let wait = |app: &mut App| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while let Mode::GitLog(tabs) = &app.mode
                && tabs.is_loading()
                && Instant::now() < deadline
            {
                app.tick();
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        let selected = |app: &App| match &app.mode {
            Mode::GitLog(tabs) => tabs.active_view().unwrap().selected_commit(),
            _ => unreachable!(),
        };
        ctrl(&mut app, 'l');
        wait(&mut app);
        // Down to the first commit, into its files, and its one file.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Down);
        let screen = draw(&mut app, 100, 24);
        assert_eq!(selected(&app), Some(first));
        assert!(screen.iter().any(|r| r.contains("a.txt")), "{screen:#?}");
        assert!(screen.iter().any(|r| r.contains("hello")), "{screen:#?}");
        // Leave for the editor; a commit is made meanwhile; come back.
        ctrl(&mut app, 'e');
        type_str(&mut app, EDITOR_MODE_LABEL);
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Editor));
        commit("c.txt", "more\n", "Say more");
        ctrl(&mut app, 'l');
        assert!(matches!(&app.mode, Mode::GitLog(tabs) if tabs.is_loading()));
        // The page is as it was left while the repository is read
        // again behind it, and says so.
        let screen = draw(&mut app, 100, 24);
        assert_eq!(selected(&app), Some(first));
        assert!(screen.iter().any(|r| r.contains("hello")), "{screen:#?}");
        assert!(
            !screen.iter().any(|r| r.contains("Say more")),
            "{screen:#?}"
        );
        assert!(screen[23].contains("refreshing"), "{screen:#?}");
        // Read: the new commit is in the log, the place kept.
        wait(&mut app);
        let screen = draw(&mut app, 100, 24);
        assert_eq!(selected(&app), Some(first));
        assert!(screen.iter().any(|r| r.contains("Say more")), "{screen:#?}");
        assert!(screen.iter().any(|r| r.contains("hello")), "{screen:#?}");
        assert!(!screen[23].contains("refreshing"), "{screen:#?}");
        // Ctrl+L on the page reads the repository again too.
        ctrl(&mut app, 'l');
        assert!(matches!(&app.mode, Mode::GitLog(tabs) if tabs.is_loading()));
        assert_eq!(selected(&app), Some(first));
        wait(&mut app);
        assert_eq!(selected(&app), Some(first));
    }

    #[test]
    fn a_bad_settings_file_is_reported_and_replaced_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().join(".storage"));
        storage
            .write(
                ninjaedit_core::storage::SETTINGS_FILE,
                "[search]\nmax-results = -5\n",
            )
            .unwrap();
        let (events, _events_rx) = std::sync::mpsc::channel();
        let mut app = App::new(Project::open(dir.path()).unwrap(), storage, events);
        let status = app.status.clone().expect("reported");
        assert!(status.contains("settings.toml"), "{status}");
        assert!(status.contains("search.max-results"), "{status}");
        assert_eq!(app.settings, Settings::default());
        let screen = draw(&mut app, 200, 10);
        assert!(screen[9].contains("search.max-results"), "{screen:#?}");

        app.open_settings();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        ctrl(&mut app, 'a');
        type_str(&mut app, "5");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.search_max_results(), 5);
        assert!(settings_file(&app).unwrap().contains("max-results = 5"));
    }

    fn build_file(app: &App) -> Option<String> {
        app.project_storage
            .read(ninjaedit_core::build::BUILD_FILE)
            .unwrap()
    }

    #[test]
    fn build_page_edits_and_saves_the_projects_build_configuration() {
        let (dir, mut app) = app_with_files(&[
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            ("src/main.rs", "fn main() {}\n"),
        ]);
        // The top-level Cargo.toml was found; the status bar shows its
        // first configuration and target. Nothing is saved until
        // something changes.
        let screen = draw(&mut app, 80, 20);
        assert!(screen[19].contains(" dev ▸ app "), "{screen:#?}");
        assert_eq!(build_file(&app), None);

        ctrl(&mut app, 'e');
        type_str(&mut app, "build");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Build(_)));
        assert_eq!(app.focus, Focus::Editor);
        let screen = draw(&mut app, 80, 20);
        assert!(screen[0].contains(build_view::TITLE), "{screen:#?}");
        assert!(screen[1].contains("Cargo.toml"), "{screen:#?}");
        assert!(
            screen[3].contains("dev") && screen[3].contains("current"),
            "{screen:#?}"
        );
        assert!(screen[19].contains("Ctrl+N new"), "{screen:#?}");

        // Rename the dev configuration: saved to the project's storage,
        // and the status bar follows.
        press(&mut app, KeyCode::Enter);
        ctrl(&mut app, 'a');
        type_str(&mut app, "debug");
        press(&mut app, KeyCode::Enter);
        let file = build_file(&app).expect("saved");
        assert!(file.contains("name = \"debug\""), "{file}");
        assert!(file.contains("configuration = \"debug\""), "{file}");
        assert!(file.contains("target = \"app\""), "{file}");
        let screen = draw(&mut app, 80, 20);
        assert!(screen[19].contains(" debug ▸ app "), "{screen:#?}");

        // A value typed but not applied is applied on the way out.
        press(&mut app, KeyCode::Tab);
        ctrl(&mut app, 'a');
        type_str(&mut app, "bench");
        ctrl(&mut app, 'e');
        let screen = draw(&mut app, 80, 20);
        assert!(screen[3].contains(build_view::TITLE), "{screen:#?}");
        assert!(screen[4].contains(EDITOR_MODE_LABEL), "{screen:#?}");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Editor));
        assert_eq!(app.build.roots()[0].configurations()[0].profile, "bench");

        // A new app on the same storage reads it back.
        let (events, _events_rx) = std::sync::mpsc::channel();
        let storage = Storage::new(dir.path().join(".storage"));
        let again = App::new(Project::open(dir.path()).unwrap(), storage, events);
        assert_eq!(again.build.current_configuration().unwrap().1.name, "debug");
        assert_eq!(again.build.roots()[0].configurations()[0].profile, "bench");
        assert_eq!(again.status, None);
    }

    #[test]
    fn discovered_targets_stay_out_of_the_file_and_follow_the_project() {
        let (dir, mut app) = app_with_files(&[
            ("Cargo.toml", "[workspace]\nmembers = [\"a\", \"b\"]\n"),
            ("a/Cargo.toml", "[package]\nname = \"a\"\n"),
            ("b/Cargo.toml", "[package]\nname = \"b\"\n"),
        ]);
        // Disable `b` from the page: saved with its origin, while `a`,
        // untouched, isn't written at all.
        app.open_build_config();
        press(&mut app, KeyCode::End);
        assert_eq!(app.build.roots()[0].targets()[1].name, "b");
        press(&mut app, KeyCode::Delete);
        assert!(app.build.roots()[0].targets()[1].disabled);
        let file = build_file(&app).expect("saved");
        assert!(file.contains("auto = \"b\""), "{file}");
        assert!(file.contains("disabled = true"), "{file}");
        assert!(!file.contains("name = \"a\""), "{file}");
        // The selector leaves the disabled target out.
        app.enter_editor();
        let screen = draw(&mut app, 80, 20);
        let column = column_of(&screen[19], "▸ a");
        click(&mut app, column, 19);
        let screen = draw(&mut app, 80, 20);
        assert!(screen[3].contains(" a "), "{screen:#?}");
        assert!(screen[4].contains('╰'), "no `b`: {screen:#?}");
        press(&mut app, KeyCode::Esc);

        // The workspace gains a package: the next launch lists it,
        // still with `b` disabled and `a` as found.
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"a\", \"b\", \"c\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("c")).unwrap();
        std::fs::write(dir.path().join("c/Cargo.toml"), "[package]\nname = \"c\"\n").unwrap();
        let (events, _events_rx) = std::sync::mpsc::channel();
        let storage = Storage::new(dir.path().join(".storage"));
        let mut again = App::new(Project::open(dir.path()).unwrap(), storage, events);
        // Until the project has been looked through again, the targets
        // are the file's, the one it named as current is shown with a
        // mark, the palettes say so, and a build waits rather than
        // building whatever stands in.
        assert!(again.build.is_discovering());
        let screen = draw(&mut again, 80, 20);
        assert!(screen[19].contains("▸ a… "), "{screen:#?}");
        ctrl(&mut again, 'b');
        let status = again.status.take().unwrap_or_default();
        assert!(
            status.contains("Still finding") && status.contains('a'),
            "{status}"
        );
        assert!(again.pending_steps.is_empty());
        let column = column_of(&screen[19], "▸ a");
        click(&mut again, column, 19);
        let screen = draw(&mut again, 80, 20);
        assert!(screen[2].contains(TARGET_PLACEHOLDER), "{screen:#?}");
        assert!(
            screen.iter().any(|r| r.contains(DISCOVERY_HINT)),
            "{screen:#?}"
        );
        again.wait_for_discovery();
        assert!(!again.build.is_discovering());
        let screen = draw(&mut again, 80, 20);
        assert!(
            !screen.iter().any(|r| r.contains(DISCOVERY_HINT)),
            "{screen:#?}"
        );
        assert!(
            screen[3].contains(" a ") && screen[4].contains(" c "),
            "{screen:#?}"
        );
        press(&mut again, KeyCode::Esc);
        let screen = draw(&mut again, 80, 20);
        assert!(screen[19].contains("▸ a "), "{screen:#?}");
        let names: Vec<(String, bool)> = again.build.roots()[0]
            .targets()
            .iter()
            .map(|t| (t.name.clone(), t.disabled))
            .collect();
        assert_eq!(
            names,
            vec![
                ("a".to_owned(), false),
                ("b".to_owned(), true),
                ("c".to_owned(), false)
            ]
        );
        assert_eq!(again.build.current_target().unwrap().1.name, "a");
    }

    #[test]
    fn clicking_the_status_bar_picks_a_configuration_or_target() {
        let (_dir, mut app) = app_with_files(&[
            ("Cargo.toml", "[workspace]\nmembers = [\"a\", \"b\"]\n"),
            ("a/Cargo.toml", "[package]\nname = \"a\"\n"),
            ("b/Cargo.toml", "[package]\nname = \"b\"\n"),
        ]);
        let screen = draw(&mut app, 80, 20);
        assert!(screen[19].contains(" dev ▸ a "), "{screen:#?}");

        // The target segment: a palette of targets, the current one
        // selected; picking another saves the choice.
        let column = column_of(&screen[19], "▸ a");
        click(&mut app, column + 2, 19);
        let screen = draw(&mut app, 80, 20);
        assert!(screen[2].contains(TARGET_PLACEHOLDER), "{screen:#?}");
        assert!(
            screen[3].contains("a") && screen[3].contains("Cargo.toml"),
            "{screen:#?}"
        );
        assert!(screen[4].contains("b"), "{screen:#?}");
        type_str(&mut app, "b");
        press(&mut app, KeyCode::Enter);
        assert!(app.palette.is_none());
        assert_eq!(app.build.current_target().unwrap().1.name, "b");
        let file = build_file(&app).expect("saved");
        assert!(file.contains("target = \"b\""), "{file}");
        let screen = draw(&mut app, 80, 20);
        assert!(screen[19].contains(" dev ▸ b "), "{screen:#?}");

        // The configuration segment likewise; Down, Enter takes the
        // next one.
        let column = column_of(&screen[19], " dev ");
        click(&mut app, column + 1, 19);
        let screen = draw(&mut app, 80, 20);
        assert!(screen[2].contains(CONFIGURATION_PLACEHOLDER), "{screen:#?}");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.build.current_configuration().unwrap().1.name, "release");
        let screen = draw(&mut app, 80, 20);
        assert!(screen[19].contains(" release ▸ b "), "{screen:#?}");

        // With the page open, it shows the change.
        app.open_build_config();
        let screen = draw(&mut app, 80, 20);
        assert!(
            screen[4].contains("release") && screen[4].contains("current"),
            "{screen:#?}"
        );
        let column = column_of(&screen[19], "▸ b");
        click(&mut app, column, 19);
        type_str(&mut app, "a");
        press(&mut app, KeyCode::Enter);
        let screen = draw(&mut app, 80, 20);
        let a_row = screen.iter().position(|r| r.contains("  a ")).unwrap();
        assert!(screen[a_row].contains("current"), "{screen:#?}");
        assert!(matches!(app.mode, Mode::Build(_)), "the page stays up");
    }

    #[test]
    fn ctrl_n_on_a_root_offers_the_projects_other_root_files() {
        let (_dir, mut app) = app_with_files(&[
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            ("native/CMakeLists.txt", "add_executable(demo demo.c)\n"),
            ("sub/Cargo.toml", "[package]\nname = \"sub\"\n"),
        ]);
        assert!(
            app.project
                .index()
                .wait_for_primary(Duration::from_secs(10))
        );
        app.open_build_config();
        press(&mut app, KeyCode::Home);
        ctrl(&mut app, 'n');
        let screen = draw(&mut app, 80, 20);
        assert!(screen[2].contains(ADD_ROOT_PLACEHOLDER), "{screen:#?}");
        assert!(
            screen[3].contains("CMakeLists.txt") && screen[3].contains("native"),
            "{screen:#?}"
        );
        assert!(
            screen[4].contains("Cargo.toml") && screen[4].contains("sub"),
            "{screen:#?}"
        );
        assert!(
            screen[5].contains('╰'),
            "the top Cargo.toml is a root already: {screen:#?}"
        );
        type_str(&mut app, "cmake");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.build.roots().len(), 2);
        app.wait_for_discovery();
        let root = &app.build.roots()[1];
        assert_eq!(root.label(), "native/CMakeLists.txt");
        assert_eq!(root.targets()[1].name, "demo");
        assert!(build_file(&app).unwrap().contains("native/CMakeLists.txt"));
        let screen = draw(&mut app, 80, 20);
        assert!(screen[7].contains("native/CMakeLists.txt"), "{screen:#?}");
        assert!(
            screen.iter().any(|r| r.contains("CMake root")),
            "{screen:#?}"
        );
        // Ctrl+B with a Cargo target current from the page applies the
        // fields first; with no roots at all there is nothing to build.
        app.build = BuildConfig::default();
        ctrl(&mut app, 'b');
        assert!(
            app.status
                .as_deref()
                .unwrap_or("")
                .contains("No build root"),
            "{:?}",
            app.status
        );
    }

    #[test]
    fn command_palette_lists_commands_with_their_keys_and_runs_them() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "hi\n")]);
        draw(&mut app, 80, 20);

        // Every command with its key at the right end of its row; the
        // ones without a key show none.
        ctrl(&mut app, 'p');
        let screen = draw(&mut app, 80, 20);
        assert!(screen[2].contains(COMMANDS_PLACEHOLDER), "{screen:#?}");
        let open = screen
            .iter()
            .find(|r| r.contains("Open file"))
            .expect("the open file command");
        assert!(open.contains("Search the project's files"), "{open}");
        assert!(open.contains("Ctrl+O │"), "at the right end: {open}");
        type_str(&mut app, "delete build");
        let screen = draw(&mut app, 80, 20);
        let delete = screen
            .iter()
            .find(|r| r.contains("Delete build directory"))
            .expect("the delete command");
        assert!(!delete.contains("Ctrl"), "{delete}");

        // A view is listed too, and picking it shows it.
        ctrl(&mut app, 'a');
        type_str(&mut app, "settings");
        press(&mut app, KeyCode::Enter);
        assert!(app.palette.is_none());
        assert!(matches!(app.mode, Mode::Settings(_)));

        // A command runs as its key would: back to the editor, an edit,
        // and undo from the palette takes it back.
        ctrl(&mut app, 'p');
        type_str(&mut app, "editor");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Editor));
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.tabs[0].view.editor().buffer().to_text(), "xhi\n");
        ctrl(&mut app, 'p');
        type_str(&mut app, "undo");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tabs[0].view.editor().buffer().to_text(), "hi\n");

        // The description is searched too: "clean" finds the commands
        // that delete build directories, and with no roots there is
        // nothing to delete.
        ctrl(&mut app, 'p');
        type_str(&mut app, "clean everything");
        let screen = draw(&mut app, 80, 20);
        assert!(
            screen[3].contains("Delete all build directories"),
            "{screen:#?}"
        );
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.status.as_deref(), Some("No build roots to clean"));
        assert!(!app.tool_pane.is_visible());

        // Quit is a command as well.
        ctrl(&mut app, 'p');
        type_str(&mut app, "quit");
        press(&mut app, KeyCode::Enter);
        assert!(app.should_quit());
    }

    #[test]
    fn conflict_commands_step_between_conflicts_and_the_palette_repeats_them() {
        let (_dir, mut app) = app_with_files(&[(
            "a.txt",
            "a\n<<<<<<< ours\nb\n=======\nc\n>>>>>>> theirs\nd\n<<<<<<< ours\ne\n=======\nf\n>>>>>>> theirs\ng\n",
        )]);
        draw(&mut app, 80, 20);
        let line = |app: &App| app.tabs[0].view.editor().cursor_position().line;

        // Before anything has run, the palette lists the commands in
        // their own order.
        ctrl(&mut app, 'p');
        let screen = draw(&mut app, 80, 20);
        assert!(screen[3].contains("Open file"), "{screen:#?}");
        type_str(&mut app, "next conflict");
        press(&mut app, KeyCode::Enter);
        assert!(app.palette.is_none());
        assert_eq!(line(&app), 1);
        assert_eq!(app.status, None);

        // Now "Next conflict" is first, so Ctrl+P, Enter repeats it; the
        // second time it wraps around and says so.
        ctrl(&mut app, 'p');
        let screen = draw(&mut app, 80, 20);
        assert!(screen[3].contains("Next conflict"), "{screen:#?}");
        assert!(screen[4].contains("Open file"), "{screen:#?}");
        press(&mut app, KeyCode::Enter);
        assert_eq!(line(&app), 7);
        ctrl(&mut app, 'p');
        press(&mut app, KeyCode::Enter);
        assert_eq!(line(&app), 1);
        assert_eq!(
            app.status.as_deref(),
            Some("Wrapped around to the first conflict in the file")
        );

        // A query ranks the entries as usual.
        ctrl(&mut app, 'p');
        type_str(&mut app, "open file");
        let screen = draw(&mut app, 80, 20);
        assert!(screen[3].contains("Open file"), "{screen:#?}");
        press(&mut app, KeyCode::Esc);

        // Previous wraps the other way, and becomes the one to repeat.
        ctrl(&mut app, 'p');
        type_str(&mut app, "previous conflict");
        press(&mut app, KeyCode::Enter);
        assert_eq!(line(&app), 7);
        assert_eq!(
            app.status.as_deref(),
            Some("Wrapped around to the last conflict in the file")
        );
        ctrl(&mut app, 'p');
        let screen = draw(&mut app, 80, 20);
        assert!(screen[3].contains("Previous conflict"), "{screen:#?}");
        press(&mut app, KeyCode::Enter);
        assert_eq!(line(&app), 1);
        assert_eq!(app.status, None);

        // Picking from another palette (a view from the modes palette)
        // doesn't change what the command palette repeats.
        ctrl(&mut app, 'e');
        type_str(&mut app, "editor");
        press(&mut app, KeyCode::Enter);
        ctrl(&mut app, 'p');
        let screen = draw(&mut app, 80, 20);
        assert!(screen[3].contains("Previous conflict"), "{screen:#?}");
        press(&mut app, KeyCode::Esc);

        // With the markers gone there is nothing to go to.
        app.tabs[0].view.editor_mut().select_all();
        app.tabs[0].view.editor_mut().insert_text("plain\n");
        ctrl(&mut app, 'p');
        type_str(&mut app, "next conflict");
        press(&mut app, KeyCode::Enter);
        assert_eq!(line(&app), 1);
        assert_eq!(
            app.status.as_deref(),
            Some("No merge conflicts in this file")
        );
    }

    #[test]
    fn deleting_build_directories_runs_as_a_job_and_forgets_what_was_configured() {
        let (_dir, mut app) = app_with_files(&[("native/CMakeLists.txt", "project(x)\n")]);
        draw(&mut app, 80, 20);
        let root = app.project.root().to_path_buf();
        app.build = BuildConfig::default();
        app.build
            .add_root(BuildRoot::discover(&root, "native/CMakeLists.txt").unwrap())
            .unwrap();
        let build_dir = app.build.cmake_build_dir(&root).unwrap();
        assert!(build_dir.ends_with("native/cmake-build-debug"));
        app.configured
            .insert(build_dir.clone(), "signature".to_owned());
        app.configured.insert(
            build_dir.with_file_name("cmake-build-release"),
            "x".to_owned(),
        );

        ctrl(&mut app, 'p');
        type_str(&mut app, "delete build directory");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.job_title, "Delete build directory (Debug)");
        assert!(app.tool_pane.is_visible());
        assert_eq!(app.tool_pane.active().unwrap().kind(), ToolKind::Output);
        assert!(!app.configured.contains_key(&build_dir));
        assert_eq!(app.configured.len(), 1, "the other configuration's stays");
        let text = output_text(&app);
        assert!(text.contains("Deleting"), "{text}");
        assert!(text.contains("cmake-build-debug"), "{text}");

        // The output tool has the keyboard now, so the palette comes
        // after the prefix.
        assert_eq!(app.focus, Focus::Tool);
        ctrl(&mut app, ']');
        ctrl(&mut app, 'p');
        type_str(&mut app, "delete all");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.job_title, "Delete all build directories");
        assert!(app.configured.is_empty());
    }

    #[test]
    fn a_target_runs_from_the_command_palette_without_becoming_current() {
        let (_dir, mut app) = app_with_files(&[
            ("Cargo.toml", "[workspace]\nmembers = [\"a\", \"b\"]\n"),
            ("a/Cargo.toml", "[package]\nname = \"a\"\n"),
            ("b/Cargo.toml", "[package]\nname = \"b\"\n"),
            (
                "native/CMakeLists.txt",
                "project(x)\nadd_executable(tool tool.c)\n",
            ),
        ]);
        draw(&mut app, 80, 20);
        let root = app.project.root().to_path_buf();
        app.build
            .add_root(BuildRoot::discover(&root, "native/CMakeLists.txt").unwrap())
            .unwrap();
        assert!(app.build.select_configuration(0, 1), "release");
        let before = app.build.current().unwrap();
        assert_eq!(app.build.current_target().unwrap().1.name, "a");

        // Every target has a "Run" entry saying what it runs with; a
        // disabled one has none.
        app.build.set_target_disabled(1, 0, true);
        ctrl(&mut app, 'p');
        type_str(&mut app, "run tool");
        let screen = draw(&mut app, 80, 20);
        let row = screen
            .iter()
            .find(|r| r.contains("Run tool"))
            .expect("the CMake target");
        assert!(row.contains("with Debug"), "{row}");
        assert!(row.contains("native/CMakeLists.txt"), "{row}");
        assert!(
            !screen.iter().any(|r| r.contains("Run All")),
            "disabled: {screen:#?}"
        );
        press(&mut app, KeyCode::Esc);

        // Running one leaves the selection alone and runs it with the
        // current configuration.
        ctrl(&mut app, 'p');
        type_str(&mut app, "run b");
        let screen = draw(&mut app, 80, 20);
        assert!(screen[3].contains("Run b"), "{screen:#?}");
        assert!(screen[3].contains("with release"), "{screen:#?}");
        press(&mut app, KeyCode::Enter);
        assert!(app.palette.is_none());
        assert_eq!(app.job_title, "Run b (release)");
        assert!(app.tool_pane.is_visible());
        assert_eq!(app.tool_pane.active().unwrap().kind(), ToolKind::Output);
        assert_eq!(app.build.current(), Some(before));
        assert_eq!(app.build.current_target().unwrap().1.name, "a");
        let screen = draw(&mut app, 80, 20);
        assert!(screen[19].contains(" release ▸ a "), "{screen:#?}");
        assert!(build_file(&app).is_none(), "nothing to save");

        // One from the other root configures its build directory and
        // looks there for the files its compiler names.
        ctrl(&mut app, ']');
        ctrl(&mut app, 'p');
        type_str(&mut app, "run tool");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.job_title, "Run tool (Debug)");
        let build_dir = root.join("native/cmake-build-debug");
        assert_eq!(
            app.job_configure.as_ref().map(|(dir, _)| dir),
            Some(&build_dir)
        );
        assert!(app.job_dirs.contains(&build_dir), "{:?}", app.job_dirs);
        assert_eq!(app.build.current(), Some(before));
    }

    /// Feed the app events from the channel until `done`, or fail after
    /// ten seconds.
    #[cfg(unix)]
    fn pump(
        app: &mut App,
        events: &std::sync::mpsc::Receiver<AppEvent>,
        done: impl Fn(&App) -> bool,
    ) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !done(app) {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let event = events
                .recv_timeout(remaining)
                .expect("the job should get there");
            app.handle_app_event(event);
        }
    }

    fn output_text(app: &App) -> String {
        let tool = app
            .tool_pane
            .tools()
            .iter()
            .find(|tool| tool.kind() == ToolKind::Output)
            .expect("an output tool");
        let terminal = tool.view().terminal_mut_for_test();
        let rows = tool.view().size().1 as usize;
        (0..rows)
            .map(|row| terminal.row_text(row))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[cfg(unix)]
    #[test]
    fn a_job_runs_its_steps_in_the_output_tool_and_says_how_it_ended() {
        let dir = tempfile::tempdir().unwrap();
        let (events, events_rx) = std::sync::mpsc::channel();
        let storage = Storage::new(dir.path().join(".storage"));
        let mut app = App::new(Project::open(dir.path()).unwrap(), storage, events);
        draw(&mut app, 60, 20);
        let step = |description: &str, script: &str| Step {
            description: description.to_owned(),
            command: ninjaedit_core::terminal::Command::new("sh")
                .arg("-c")
                .arg(script),
        };
        let job = Job {
            title: "Test job".to_owned(),
            steps: vec![
                step("Step one", "echo one"),
                step("Step two", "echo two; exit 3"),
                step("Step three", "echo three"),
            ],
        };
        app.start_job(job);
        assert!(app.tool_pane.is_visible());
        assert_eq!(app.focus, Focus::Tool, "so Ctrl+C reaches the job");
        assert_eq!(app.tool_pane.active().unwrap().kind(), ToolKind::Output);
        pump(&mut app, &events_rx, |app| {
            app.status.as_deref().is_some_and(|s| s.contains("failed"))
        });
        let text = output_text(&app);
        for expected in [
            "Step one",
            "$ sh -c 'echo one'",
            "one",
            "Step two",
            "two",
            "Failed: exit code 3",
        ] {
            assert!(text.contains(expected), "{expected:?} in {text}");
        }
        assert!(!text.contains("three"), "stopped at the failure: {text}");
        assert!(app.pending_steps.is_empty());
        assert!(!app.tool_pane.active().unwrap().is_running());
        assert_eq!(
            app.status.as_deref(),
            Some("Test job: failed with exit code 3")
        );
        let screen = draw(&mut app, 60, 20);
        assert!(screen.iter().any(|r| r.contains("Output")), "{screen:#?}");

        // With the output idle and focused, the editor's keys need no
        // prefix, and Ctrl+D dismisses it: the pane hides and the
        // editor has the keyboard.
        ctrl(&mut app, 'e');
        assert!(app.palette.is_some());
        press(&mut app, KeyCode::Esc);
        let screen = draw(&mut app, 100, 20);
        assert!(screen[19].contains(OUTPUT_IDLE_HINT), "{screen:#?}");
        press(&mut app, KeyCode::Char('x'));
        assert!(
            app.tool_pane.is_visible(),
            "an ordinary key doesn't dismiss it"
        );
        ctrl(&mut app, 'd');
        assert!(!app.tool_pane.is_visible());
        assert_eq!(app.focus, Focus::Editor);

        // Another job brings it back with the keyboard; success says so,
        // on a fresh screen.
        let job = Job {
            title: "Quick".to_owned(),
            steps: vec![step("Only step", "echo done")],
        };
        app.start_job(job);
        assert!(app.tool_pane.is_visible());
        assert_eq!(app.focus, Focus::Tool);
        pump(&mut app, &events_rx, |app| {
            app.status
                .as_deref()
                .is_some_and(|s| s.contains("finished"))
        });
        let text = output_text(&app);
        assert!(text.contains("done") && text.contains("Finished"), "{text}");
        assert!(
            !text.contains("Step one"),
            "a new job starts on a fresh screen: {text}"
        );
        assert!(app.tool_pane.is_visible(), "the output stays showing");
    }

    #[test]
    fn clicking_a_location_in_the_output_opens_the_file_there() {
        let (dir, mut app) = app_with_files(&[("src/main.rs", "fn main() {}\n")]);
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn f() {\n    let unused = 1;\n}\n",
        )
        .unwrap();
        app.open_tool(ToolKind::Output);
        assert_eq!(app.focus, Focus::Tool);
        let tool = app.tool_pane.active_mut().unwrap();
        tool.process(b"warning: unused variable: `unused`\r\n  --> src/lib.rs:2:9\r\n   |\r\n");
        tool.process(b"  --> src/nope.rs:1:1\r\n");
        let screen = draw(&mut app, 60, 20);
        let (row, text) = screen
            .iter()
            .enumerate()
            .find(|(_, row)| row.contains("src/lib.rs:2:9"))
            .expect("the location on screen");
        let column = text.find("src/lib.rs").unwrap() as u16;

        // Clicking beside the link is an ordinary click in the tool.
        click(&mut app, column.saturating_sub(3), row as u16);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.focus, Focus::Tool);

        // Clicking the link opens the file at the line and column.
        click(&mut app, column + 4, row as u16);
        assert_eq!(app.tabs.len(), 2, "src/lib.rs opened in a new tab");
        let tab = &app.tabs[app.active];
        assert_eq!(
            tab.path()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str()),
            Some("lib.rs")
        );
        assert_eq!(
            tab.view.editor().cursor_position(),
            ninjaedit_core::Position { line: 1, column: 8 }
        );
        assert_eq!(app.focus, Focus::Editor, "ready to edit");
        assert!(app.tool_pane.is_visible(), "the output stays showing");
        let screen = draw(&mut app, 60, 20);
        assert!(screen[2].contains("let unused = 1;"), "{screen:#?}");

        // A location that exists nowhere is reported, not opened.
        let (row, text) = screen
            .iter()
            .enumerate()
            .find(|(_, row)| row.contains("src/nope.rs:1:1"))
            .expect("the missing location on screen");
        let column = text.find("src/nope.rs").unwrap() as u16;
        click(&mut app, column, row as u16);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.status.as_deref(), Some("Could not find src/nope.rs"));
    }

    // The shell tool spawns a real process, so these are unix-only and
    // never rely on its output (the event receiver is dropped by the test
    // helper), only on the pane's structure and focus.
    #[cfg(unix)]
    #[test]
    fn scrollback_setting_applies_to_a_running_shell() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "hi\n")]);
        draw(&mut app, 40, 20);
        ctrl(&mut app, '`');
        assert_eq!(app.focus, Focus::Tool);
        // Forty lines of output into a 24-row terminal leave scrollback.
        let output: String = (0..40).map(|i| format!("line {i}\r\n")).collect();
        app.tool_pane.tools_mut()[0].process(output.as_bytes());
        let before = app.tool_pane.tools()[0]
            .view()
            .terminal_mut_for_test()
            .scrollback_len();
        assert!(before > 3, "{before}");

        // Settings from the shell go through the prefix; the shell stays
        // below the page and is listed as where the user came from.
        ctrl(&mut app, ']');
        ctrl(&mut app, 'e');
        type_str(&mut app, "sett");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Settings(_)));
        assert_eq!(app.focus, Focus::Editor);
        assert!(app.tool_pane.is_visible());
        press(&mut app, KeyCode::Down);
        ctrl(&mut app, 'a');
        type_str(&mut app, "3");
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.tool_pane.tools()[0]
                .view()
                .terminal_mut_for_test()
                .scrollback_len(),
            3
        );
        ctrl(&mut app, 'e');
        let screen = draw(&mut app, 40, 20);
        assert!(screen[3].contains(settings_view::TITLE), "{screen:#?}");
        assert!(screen[4].contains("Shell"), "{screen:#?}");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.focus, Focus::Tool);
        assert!(
            matches!(app.mode, Mode::Settings(_)),
            "focusing a tool keeps the page"
        );
        // From the shell, Ctrl+E, Enter goes back to the page; the editor
        // is one further down.
        ctrl(&mut app, ']');
        ctrl(&mut app, 'e');
        let screen = draw(&mut app, 40, 20);
        assert!(screen[3].contains("Shell"), "{screen:#?}");
        assert!(screen[4].contains(settings_view::TITLE), "{screen:#?}");
        assert!(screen[5].contains(EDITOR_MODE_LABEL), "{screen:#?}");
        press(&mut app, KeyCode::Esc);
    }

    #[cfg(unix)]
    #[test]
    fn shell_toggles_split_and_focus() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "hi\n")]);
        draw(&mut app, 40, 20);
        assert!(!app.tool_pane.is_visible());
        assert_eq!(app.focus, Focus::Editor);

        // Ctrl+` shows the shell, focuses it, and splits the screen.
        ctrl(&mut app, '`');
        assert!(app.tool_pane.is_visible());
        assert_eq!(app.focus, Focus::Tool);
        let screen = draw(&mut app, 40, 20);
        let divider = screen.iter().position(|r| r.contains('─')).unwrap();
        assert!(divider > 1 && divider < 18, "divider row {divider}");
        assert!(screen[divider + 1].contains("Shell"), "{screen:#?}");

        // From the shell, Ctrl+. needs the prefix; from the editor, Ctrl+,
        // doesn't.
        ctrl(&mut app, '.');
        assert_eq!(app.focus, Focus::Tool);
        ctrl(&mut app, ']');
        ctrl(&mut app, '.');
        assert_eq!(app.focus, Focus::Editor);
        ctrl(&mut app, ',');
        assert_eq!(app.focus, Focus::Tool);

        // From the editor, Ctrl+` with the shell showing focuses it rather
        // than hiding it; from the shell it hides it and returns to the
        // editor.
        ctrl(&mut app, ']');
        ctrl(&mut app, '.');
        assert_eq!(app.focus, Focus::Editor);
        ctrl(&mut app, '`');
        assert!(app.tool_pane.is_visible());
        assert_eq!(app.focus, Focus::Tool);
        ctrl(&mut app, '`');
        assert!(!app.tool_pane.is_visible());
        assert_eq!(app.focus, Focus::Editor);
    }

    #[cfg(unix)]
    #[test]
    fn prefix_holds_one_key_for_the_editor() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "hi\n")]);
        draw(&mut app, 40, 20);
        ctrl(&mut app, '`');
        assert_eq!(app.focus, Focus::Tool);

        // The prefix waits, and says so in the status bar.
        ctrl(&mut app, ']');
        assert!(app.prefix.is_some());
        let screen = draw(&mut app, 40, 20);
        assert!(screen[19].trim_start().starts_with("Ctrl+]"), "{screen:#?}");

        // A key the editor has no binding for goes through to the shell,
        // prefix and all, and the wait is over.
        press(&mut app, KeyCode::Char('x'));
        assert!(app.prefix.is_none());
        assert_eq!(app.focus, Focus::Tool);
        assert!(app.palette.is_none());

        // So does a second prefix.
        ctrl(&mut app, ']');
        ctrl(&mut app, ']');
        assert!(app.prefix.is_none());
        assert_eq!(app.focus, Focus::Tool);

        // Ctrl+5 is the same key, as terminals without the kitty protocol
        // deliver it.
        ctrl(&mut app, '5');
        assert!(app.prefix.is_some());
        ctrl(&mut app, 'e');
        assert!(app.palette.is_some());
        press(&mut app, KeyCode::Esc);

        // A click drops a waiting prefix rather than holding it for the
        // editor.
        ctrl(&mut app, ']');
        click(&mut app, 2, 2);
        assert_eq!(app.focus, Focus::Editor);
        assert!(app.prefix.is_none());

        // In the editor the prefix isn't special, so Ctrl+] Ctrl+E works
        // there too, out of habit.
        ctrl(&mut app, ']');
        ctrl(&mut app, 'e');
        assert!(app.palette.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn prefixed_editor_keys_work_from_the_shell() {
        let (_dir, mut app) = app_with_files(&[("alpha.rs", "one\ntwo\nthree\n"), ("beta.rs", "")]);
        app.project
            .index()
            .wait_for_primary(std::time::Duration::from_secs(10));
        draw(&mut app, 40, 20);
        ctrl(&mut app, '`');
        assert_eq!(app.focus, Focus::Tool);

        // Ctrl+] Ctrl+T: the tab search opens over the shell and takes the
        // keyboard; picking a tab focuses the editor.
        ctrl(&mut app, ']');
        ctrl(&mut app, 't');
        assert!(app.palette.is_some());
        type_str(&mut app, "alpha");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(app.tabs[app.active].title(), "alpha.rs");
        assert!(app.tool_pane.is_visible());

        // Ctrl+] Ctrl+O: likewise for the file search, while Escape leaves
        // the shell focused.
        ctrl(&mut app, ',');
        assert_eq!(app.focus, Focus::Tool);
        ctrl(&mut app, ']');
        ctrl(&mut app, 'o');
        assert!(app.palette.is_some());
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.focus, Focus::Tool);
        ctrl(&mut app, ']');
        ctrl(&mut app, 'o');
        type_str(&mut app, "beta");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(app.tabs[app.active].title(), "beta.rs");

        // Ctrl+] Ctrl+J: the go to line box takes the keyboard from the
        // shell, and Enter lands in the editor.
        ctrl(&mut app, ']');
        ctrl(&mut app, 't');
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tabs[app.active].title(), "alpha.rs");
        ctrl(&mut app, ',');
        assert_eq!(app.focus, Focus::Tool);
        ctrl(&mut app, ']');
        ctrl(&mut app, 'j');
        assert!(app.goto_line.is_some());
        let screen = draw(&mut app, 40, 20);
        assert!(screen[1].contains('╭'), "{screen:#?}");
        type_str(&mut app, "3");
        press(&mut app, KeyCode::Enter);
        assert!(app.goto_line.is_none());
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(app.tabs[app.active].view.editor().cursor_position().line, 2);

        // Ctrl+] Ctrl+F: the search box, and Ctrl+F again upgrades it to
        // the project search as in the editor.
        ctrl(&mut app, ',');
        ctrl(&mut app, ']');
        ctrl(&mut app, 'f');
        assert!(app.search_box.is_some());
        ctrl(&mut app, 'f');
        assert!(app.search_box.is_none() && app.project_search_open);
        press(&mut app, KeyCode::Esc);
        assert!(!app.project_search_open);
        assert_eq!(app.focus, Focus::Tool);
    }

    #[cfg(unix)]
    #[test]
    fn prefixed_quit_confirms_unsaved_changes() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "hi\n")]);
        press(&mut app, KeyCode::Char('z'));
        draw(&mut app, 40, 20);
        ctrl(&mut app, '`');
        assert_eq!(app.focus, Focus::Tool);

        // Ctrl+Q alone is the shell's; after the prefix it asks first.
        ctrl(&mut app, 'q');
        assert!(!app.should_quit());
        ctrl(&mut app, ']');
        ctrl(&mut app, 'q');
        assert!(!app.should_quit());
        assert_eq!(app.confirm, Some(Confirm::Quit));
        // The confirmation survives the second prefix.
        ctrl(&mut app, ']');
        ctrl(&mut app, 'q');
        assert!(app.should_quit());
    }

    #[cfg(unix)]
    #[test]
    fn modes_palette_switches_between_editor_and_shell() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "hi\n")]);
        draw(&mut app, 40, 20);

        // Ctrl+E from the editor lists the editor first and preselects the
        // shell, so Enter alone opens it.
        ctrl(&mut app, 'e');
        let screen = draw(&mut app, 40, 20);
        assert!(screen[2].contains(MODES_PLACEHOLDER), "{screen:#?}");
        assert!(screen[3].contains(EDITOR_MODE_LABEL), "{screen:#?}");
        assert!(screen[4].contains("Shell"), "{screen:#?}");
        press(&mut app, KeyCode::Enter);
        assert!(app.palette.is_none());
        assert!(app.tool_pane.is_visible());
        assert_eq!(app.focus, Focus::Tool);

        // From the shell Ctrl+E is the shell's; after the prefix the
        // palette opens, takes the keyboard, and lists the shell first.
        // Escape leaves the shell focused.
        ctrl(&mut app, 'e');
        assert!(app.palette.is_none());
        ctrl(&mut app, ']');
        ctrl(&mut app, 'e');
        assert!(app.palette.is_some());
        let screen = draw(&mut app, 40, 20);
        assert!(screen[3].contains("Shell"), "{screen:#?}");
        assert!(screen[4].contains(EDITOR_MODE_LABEL), "{screen:#?}");
        press(&mut app, KeyCode::Esc);
        assert!(app.palette.is_none());
        assert_eq!(app.focus, Focus::Tool);

        // Typing narrows it like any palette; picking the editor focuses
        // it and leaves the shell showing.
        ctrl(&mut app, ']');
        ctrl(&mut app, 'e');
        type_str(&mut app, "edit");
        let screen = draw(&mut app, 40, 20);
        assert!(screen[3].contains(EDITOR_MODE_LABEL), "{screen:#?}");
        // The shell row is filtered out: the frame closes right under the
        // editor row.
        assert!(screen[4].contains('╰'), "{screen:#?}");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.focus, Focus::Editor);
        assert!(app.tool_pane.is_visible());
        assert_eq!(app.tool_pane.tools().len(), 1);

        // Picking the shell again reuses the running one rather than
        // adding another.
        ctrl(&mut app, 'e');
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.focus, Focus::Tool);
        assert_eq!(app.tool_pane.tools().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn dragging_the_divider_resizes_the_split() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "hi\n")]);
        ctrl(&mut app, '`');
        draw(&mut app, 40, 20);
        let start = app.divider_area.y;
        let tall_editor = app.editor_area.height;
        // Drag the divider up: the editor shrinks, the tool grows.
        mouse_at(&mut app, MouseEventKind::Down(MouseButton::Left), 5, start);
        mouse_at(
            &mut app,
            MouseEventKind::Drag(MouseButton::Left),
            5,
            start - 4,
        );
        mouse_at(
            &mut app,
            MouseEventKind::Up(MouseButton::Left),
            5,
            start - 4,
        );
        draw(&mut app, 40, 20);
        assert!(
            app.editor_area.height < tall_editor,
            "editor did not shrink"
        );
        assert!(app.divider_area.y < start, "divider did not move up");
    }

    #[cfg(unix)]
    #[test]
    fn clicking_a_view_focuses_it() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "hi\n")]);
        ctrl(&mut app, '`');
        draw(&mut app, 40, 20);
        assert_eq!(app.focus, Focus::Tool);
        // Click in the editor: focus moves there.
        click(&mut app, 6, 2);
        assert_eq!(app.focus, Focus::Editor);
        // Click in the tool's terminal: focus moves back.
        let row = app.tool_term_area.y;
        click(&mut app, 6, row);
        assert_eq!(app.focus, Focus::Tool);
    }

    #[cfg(unix)]
    #[test]
    fn dragging_in_a_tool_copies_its_text_and_the_prefix_takes_the_mouse_from_a_program() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "hi\n")]);
        app.clipboard = Clipboard::local_only();
        ctrl(&mut app, '`');
        draw(&mut app, 40, 20);
        app.tool_pane.tools_mut()[0].process(b"hello world\r\nsecond line\r\n");
        let area = app.tool_term_area;
        let (x, y) = (area.x, area.y);
        let drag = |app: &mut App, from: (u16, u16), to: (u16, u16)| {
            mouse_at(app, MouseEventKind::Down(MouseButton::Left), from.0, from.1);
            mouse_at(app, MouseEventKind::Drag(MouseButton::Left), to.0, to.1);
            mouse_at(app, MouseEventKind::Up(MouseButton::Left), to.0, to.1);
        };
        // The shell isn't reading the mouse, so a drag selects and the
        // release copies, says so, and leaves nothing selected.
        drag(&mut app, (x, y), (x + 6, y + 1));
        assert_eq!(app.clipboard.get().as_deref(), Some("hello world\nsecond"));
        assert_eq!(
            app.status.as_deref(),
            Some("Copied 2 lines to the clipboard")
        );
        let screen = draw(&mut app, 40, 20);
        assert!(screen[19].contains("Copied 2 lines"), "{screen:#?}");
        assert!(!app.tool_pane.tools()[0].view().is_selecting());

        // A program reading the mouse gets the drag instead.
        app.status = None;
        app.tool_pane.tools_mut()[0].process(b"\x1b[?1000h");
        drag(&mut app, (x + 6, y), (x + 11, y));
        assert_eq!(
            app.clipboard.get().as_deref(),
            Some("hello world\nsecond"),
            "the program's drag copied nothing"
        );
        assert_eq!(app.status, None);

        // After the prefix the mouse is the editor's again: the drag
        // selects, and the press spends the prefix.
        ctrl(&mut app, ']');
        assert!(app.prefix.is_some());
        drag(&mut app, (x + 6, y), (x + 11, y));
        assert_eq!(app.prefix, None);
        assert_eq!(app.clipboard.get().as_deref(), Some("world"));
        assert_eq!(
            app.status.as_deref(),
            Some("Copied 1 line to the clipboard")
        );
        assert_eq!(app.focus, Focus::Tool);
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
    fn empty_page_names_the_editor_and_the_project() {
        let (dir, mut app) = project_with_files(&[("a.txt", "one\n")]);
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let screen = draw(&mut app, 80, 20);
        let text = screen.join("\n");
        assert!(text.contains(EDITOR_NAME), "{screen:#?}");
        assert!(text.contains(&root.display().to_string()), "{screen:#?}");
        assert!(text.contains("No files open"), "{screen:#?}");
        assert!(text.contains("open a project file"), "{screen:#?}");
        assert!(!text.contains("not a project"), "{screen:#?}");
        // The status bar no longer carries the project's name.
        let name = root.file_name().unwrap().to_string_lossy();
        assert!(!screen[19].contains(name.as_ref()), "{screen:#?}");
        assert_eq!(app.window_title(), format!("{EDITOR_NAME} — {name}"));
    }

    #[test]
    fn empty_page_lays_the_keys_out_as_a_table() {
        let (_dir, mut app) = project_with_files(&[("a.txt", "one\n")]);
        let screen = draw(&mut app, 80, 24);
        // The last line is the status bar, which mentions keys of its own.
        let rows: Vec<&String> = screen[..screen.len() - 1]
            .iter()
            .filter(|l| l.contains("Ctrl+"))
            .collect();
        assert_eq!(rows.len(), 12, "{screen:#?}");
        // Every key starts in the same column, and so does every description.
        let key_column = rows[0].find("Ctrl+").unwrap();
        let description_column = rows[0].find("open a project file").unwrap();
        for row in &rows {
            assert_eq!(row.find("Ctrl+"), Some(key_column), "{row:?}");
            let description = row[key_column..].trim_start_matches(|c: char| !c.is_whitespace());
            let description_start = row.len() - description.trim_start().len();
            assert_eq!(description_start, description_column, "{row:?}");
        }
        // The table is centered as a block: its widest row has as much
        // room on the left as on the right.
        let widest = rows.iter().map(|r| r.trim_end().len()).max().unwrap();
        let right = 80 - widest;
        assert!(key_column.abs_diff(right) <= 1, "{screen:#?}");
    }

    #[test]
    fn empty_page_says_when_the_directory_is_not_a_project() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        let (events, _events_rx) = std::sync::mpsc::channel();
        let storage = Storage::new(dir.path().join(".storage"));
        let mut app = App::new(
            Project::open_directory(dir.path()).unwrap(),
            storage,
            events,
        );
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let screen = draw(&mut app, 80, 20);
        let text = screen.join("\n");
        assert!(text.contains(EDITOR_NAME), "{screen:#?}");
        assert!(text.contains(&root.display().to_string()), "{screen:#?}");
        assert!(text.contains(NOT_A_PROJECT), "{screen:#?}");
        assert!(
            text.contains("open a file in this directory"),
            "{screen:#?}"
        );
        assert_eq!(
            app.window_title(),
            format!("{EDITOR_NAME} — {}", root.display())
        );
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
    fn the_last_line_shows_above_the_horizontal_scrollbar() {
        // A file taller than the view whose last line is too long to
        // fit: scrolled to the end without moving the cursor, the
        // scrollbar takes the last row and the last line sits above it.
        let mut text: String = (1..=40).map(|i| format!("line {i}\n")).collect();
        text.push_str(&format!("last {}", "x".repeat(60)));
        let (_dir, mut app) = app_with_files(&[("a.txt", &text)]);
        draw(&mut app, 30, 12);
        let wheel = |app: &mut App, kind| {
            app.handle_event(Event::Mouse(MouseEvent {
                kind,
                column: 8,
                row: 2,
                modifiers: KeyModifiers::NONE,
            }));
        };
        for _ in 0..30 {
            wheel(&mut app, MouseEventKind::ScrollDown);
        }
        let screen = draw(&mut app, 30, 12);
        // Rows 1..=9 are text, row 10 the horizontal scrollbar.
        assert!(
            screen[10].contains('─') || screen[10].contains('█'),
            "{screen:#?}"
        );
        assert!(screen[9].contains("last"), "{screen:#?}");
        // The view can't be scrolled any further, and the rows stay put
        // on a redraw.
        wheel(&mut app, MouseEventKind::ScrollDown);
        let again = draw(&mut app, 30, 12);
        assert_eq!(again, screen);
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
    fn conflict_tint_reaches_the_screen() {
        // A file of unknown kind still gets its conflict highlighted.
        let (_dir, mut app) = app_with_files(&[(
            "a.txt",
            "top\n<<<<<<< editor\nours\n=======\ntheirs\n>>>>>>> disk\nend\n",
        )]);
        app.set_theme(
            Theme::parse(
                "conflict-ours-background = \"#010203\"\n\
                 conflict-theirs-background = \"#040506\"\n\
                 syntax-conflict-marker = \"*#070809\"\n",
            )
            .unwrap(),
        );
        let mut terminal = Terminal::new(TestBackend::new(20, 9)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let default = Theme::default();
        // Rows 1..=7 are the file's lines; the gutter is 4 wide.
        assert_eq!(buffer[(4, 1)].bg, default.view_background);
        let marker = &buffer[(4, 2)];
        assert_eq!(marker.symbol(), "<");
        assert_eq!(marker.fg, Color::Rgb(7, 8, 9));
        assert!(marker.modifier.contains(Modifier::BOLD));
        assert_eq!(marker.bg, Color::Rgb(1, 2, 3));
        // The tint runs past the end of the text, but not into the gutter.
        assert_eq!(buffer[(4, 3)].bg, Color::Rgb(1, 2, 3));
        assert_eq!(buffer[(15, 3)].bg, Color::Rgb(1, 2, 3));
        assert_eq!(buffer[(1, 3)].bg, default.view_background);
        assert_eq!(buffer[(4, 4)].bg, Color::Rgb(4, 5, 6));
        assert_eq!(buffer[(4, 5)].bg, Color::Rgb(4, 5, 6));
        assert_eq!(buffer[(4, 6)].bg, Color::Rgb(4, 5, 6));
        assert_eq!(buffer[(4, 7)].bg, default.view_background);
        // Text inside keeps the view's text color over the tint.
        assert_eq!(buffer[(4, 5)].fg, default.view_text);
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
    fn ctrl_j_goes_to_a_line_clamped_to_the_file() {
        let text: String = (1..=50).map(|n| format!("line {n}\n")).collect();
        let (_dir, mut app) = app_with_files(&[("a.txt", &text)]);
        ctrl(&mut app, 'j');
        assert!(app.goto_line.is_some());
        let screen = draw(&mut app, 60, 12);
        assert!(screen[2].contains("Go to line"), "{screen:#?}");
        assert!(screen[3].contains("51 lines"), "{screen:#?}");
        type_str(&mut app, "30");
        press(&mut app, KeyCode::Enter);
        assert!(app.goto_line.is_none());
        let editor = app.tabs[0].view.editor();
        assert_eq!(
            editor.cursor_position(),
            Position {
                line: 29,
                column: 0
            }
        );
        // The line is brought into view, centered.
        let screen = draw(&mut app, 60, 12);
        let shown: Vec<&str> = screen[1..11].iter().map(|row| row.trim_start()).collect();
        assert!(shown[5].starts_with("30 │line 30"), "{screen:#?}");
        let status = &screen[11];
        assert!(status.contains("Ln 30, Col 1"), "{screen:#?}");

        // Out of range numbers clamp to the ends of the file.
        ctrl(&mut app, 'j');
        type_str(&mut app, "9999");
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.tabs[0].view.editor().cursor_position(),
            Position {
                line: 50,
                column: 0
            }
        );
        ctrl(&mut app, 'j');
        type_str(&mut app, "0");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tabs[0].view.editor().cursor(), 0);
        ctrl(&mut app, 'j');
        type_str(&mut app, "99999999999999999999999999");
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.tabs[0].view.editor().cursor_position(),
            Position {
                line: 50,
                column: 0
            }
        );

        // Going to a line drops the selection.
        app.tabs[0].view.editor_mut().set_selection(0, 5);
        ctrl(&mut app, 'j');
        type_str(&mut app, " 2 ");
        press(&mut app, KeyCode::Enter);
        let editor = app.tabs[0].view.editor();
        assert_eq!(editor.selection(), None);
        assert_eq!(editor.cursor_position(), Position { line: 1, column: 0 });
    }

    #[test]
    fn go_to_line_box_wants_a_number_and_closes_like_the_search_box() {
        let (_dir, mut app) = app_with_files(&[("a.txt", "one\ntwo\nthree\n")]);
        ctrl(&mut app, 'j');
        // Enter with nothing, or with something other than a number,
        // keeps the box open and says why.
        press(&mut app, KeyCode::Enter);
        assert!(app.goto_line.is_some());
        let screen = draw(&mut app, 60, 8);
        assert!(screen[3].contains("enter a line number"), "{screen:#?}");
        type_str(&mut app, "2x");
        press(&mut app, KeyCode::Enter);
        assert!(app.goto_line.is_some());
        let screen = draw(&mut app, 60, 8);
        assert!(screen[3].contains("enter a line number"), "{screen:#?}");
        press(&mut app, KeyCode::Backspace);
        let screen = draw(&mut app, 60, 8);
        assert!(screen[3].contains("4 lines"), "{screen:#?}");
        assert_eq!(app.tabs[0].view.editor().cursor(), 0);
        // Escape closes the box without moving.
        press(&mut app, KeyCode::Esc);
        assert!(app.goto_line.is_none());
        assert_eq!(app.tabs[0].view.editor().cursor(), 0);

        // A click outside the box closes it; the palette and the search
        // box replace it, and it replaces them.
        ctrl(&mut app, 'j');
        click(&mut app, 5, 6);
        assert!(app.goto_line.is_none());
        ctrl(&mut app, 'j');
        ctrl(&mut app, 't');
        assert!(app.goto_line.is_none() && app.palette.is_some());
        ctrl(&mut app, 'j');
        assert!(app.goto_line.is_some() && app.palette.is_none());
        ctrl(&mut app, 'f');
        assert!(app.goto_line.is_none() && app.search_box.is_some());
        ctrl(&mut app, 'j');
        assert!(app.goto_line.is_some() && app.search_box.is_none());
        assert!(app.tabs[0].view.editor().search().is_none());
        // Ctrl+J with the box open leaves it as it is.
        type_str(&mut app, "3");
        ctrl(&mut app, 'j');
        assert_eq!(app.goto_line.as_ref().unwrap().text(), "3");
        // Pasting goes into the box.
        app.handle_event(Event::Paste("1".to_owned()));
        assert_eq!(app.goto_line.as_ref().unwrap().text(), "31");

        // With no file open there is nothing to go to.
        let (_dir, mut app) = app_with_files(&[]);
        ctrl(&mut app, 'j');
        assert!(app.goto_line.is_none());
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
        // The plain text form of a pattern is literal. (Ctrl+U is the
        // changes page's, so the text is cleared by selecting it all.)
        ctrl(&mut app, 'a');
        press(&mut app, KeyCode::Backspace);
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
        let (events, _events_rx) = std::sync::mpsc::channel();
        let storage = Storage::new(dir.path().join(".storage"));
        let mut app = App::new(Project::open(dir.path()).unwrap(), storage, events);
        app.wait_for_discovery();
        (dir, app)
    }

    fn ctrl_shift_f(app: &mut App) {
        app.handle_event(key(
            KeyCode::Char('F'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
    }

    /// A screen row up to the project search dialog's right border,
    /// leaving out whatever the empty page behind it shows past the box
    /// (its location line can be wider than the box).
    fn boxed(row: &str) -> String {
        match row.rfind('│') {
            Some(end) => row[..end + '│'.len_utf8()].to_owned(),
            None => row.trim_end().to_owned(),
        }
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
        assert!(boxed(&screen[3]).ends_with("a.rs:2│"), "{screen:#?}");
        assert!(screen[4].contains("needle"), "{screen:#?}");
        assert!(boxed(&screen[4]).ends_with("b/c.rs:1│"), "{screen:#?}");
        assert!(screen[5].contains("needle again"), "{screen:#?}");
        assert!(boxed(&screen[5]).ends_with("b/c.rs:3│"), "{screen:#?}");
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
    fn project_search_sees_unsaved_edits() {
        let (dir, mut app) = project_with_files(&[("a.rs", "one\ntwo\n"), ("b.rs", "needle\n")]);
        app.open_file(dir.path().join("a.rs"));
        // An unmodified file is searched from disk.
        ctrl_shift_f(&mut app);
        type_str(&mut app, "needle");
        wait_for_project_search(&app);
        let screen = draw(&mut app, 60, 20);
        assert!(screen[18].contains("1 match"), "{screen:#?}");
        press(&mut app, KeyCode::Esc);
        // Edit the first line of a.rs without saving. Reopening the dialog
        // searches the edit and shows it in the results and the context.
        type_str(&mut app, "needle ");
        assert!(app.tabs[app.active].view.editor().is_modified());
        ctrl_shift_f(&mut app);
        wait_for_project_search(&app);
        let screen = draw(&mut app, 60, 20);
        assert!(screen[18].contains("2 matches"), "{screen:#?}");
        assert!(screen[3].contains("needle one"), "{screen:#?}");
        assert!(screen[3].contains("a.rs:1│"), "{screen:#?}");
        assert!(
            screen[11..18].iter().any(|row| row.contains("needle one")),
            "{screen:#?}"
        );
        // Going to the match selects it in the edited buffer.
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tabs[app.active].title(), "a.rs");
        assert_eq!(app.tabs[app.active].view.editor().selection(), Some(0..6));
        // A further edit is picked up on the next showing; an unchanged
        // buffer leaves the results as they were.
        let dialog = app.project_search.as_ref().unwrap();
        let versions = dialog.buffer_versions.clone();
        ctrl_shift_f(&mut app);
        assert_eq!(
            app.project_search.as_ref().unwrap().buffer_versions,
            versions
        );
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::End);
        type_str(&mut app, " needle");
        ctrl_shift_f(&mut app);
        wait_for_project_search(&app);
        assert_ne!(
            app.project_search.as_ref().unwrap().buffer_versions,
            versions
        );
        let screen = draw(&mut app, 60, 20);
        assert!(screen[18].contains("3 matches"), "{screen:#?}");
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
    fn ctrl_f_twice_upgrades_to_project_search() {
        // b.rs is the active tab; three blank lines keep its text clear
        // of the search box.
        let (_dir, mut app) = app_with_files(&[
            ("a.rs", "needle\nother\n"),
            ("b.rs", "\n\n\nneedle other\n"),
        ]);
        assert_eq!(app.tabs[app.active].title(), "b.rs");
        // Ctrl+F, Ctrl+F with nothing typed is Ctrl+Shift+F: an empty
        // project search.
        ctrl(&mut app, 'f');
        assert!(app.search_box.is_some());
        ctrl(&mut app, 'f');
        assert!(app.search_box.is_none());
        assert!(app.project_search_open);
        assert_eq!(app.project_search.as_ref().unwrap().query(), "");
        // The search box's query carries over, selected so that typing
        // replaces it, and the file's own search is dropped.
        press(&mut app, KeyCode::Esc);
        assert!(!app.project_search_open);
        ctrl(&mut app, 'f');
        type_str(&mut app, "needle");
        wait_for_search(&app);
        ctrl(&mut app, 'f');
        assert!(app.search_box.is_none());
        assert!(app.project_search_open);
        assert!(app.tabs[app.active].view.editor().search().is_none());
        let dialog = app.project_search.as_ref().unwrap();
        assert_eq!(dialog.query(), "needle");
        assert_eq!(dialog.input().edit().selection(), Some(0..6));
        wait_for_project_search(&app);
        let screen = draw(&mut app, 60, 20);
        assert!(screen[18].contains("2 matches in 2 files"), "{screen:#?}");
        // A regular expression carries over as one.
        press(&mut app, KeyCode::Esc);
        ctrl(&mut app, 'f');
        type_str(&mut app, "/oth.r");
        ctrl(&mut app, 'f');
        assert_eq!(app.project_search.as_ref().unwrap().query(), "/oth.r");
        wait_for_project_search(&app);
        let screen = draw(&mut app, 60, 20);
        assert!(screen[18].contains("2 matches in 2 files"), "{screen:#?}");
        // Ctrl+F, Ctrl+F over a selection, with nothing typed in between,
        // is Ctrl+Shift+F over that selection: the dialog is seeded from
        // it when it isn't already searching for it...
        press(&mut app, KeyCode::Esc);
        app.tabs[app.active].view.editor_mut().set_selection(3, 9);
        ctrl(&mut app, 'f');
        assert_eq!(app.search_box.as_ref().unwrap().query(), "needle");
        ctrl(&mut app, 'f');
        let dialog = app.project_search.as_ref().unwrap();
        assert_eq!(dialog.query(), "needle");
        assert_eq!(dialog.input().edit().selection(), Some(0..6));
        wait_for_project_search(&app);
        press(&mut app, KeyCode::Enter);
        // ...and otherwise comes back as it was, so that the matches can
        // be walked through one by one.
        assert_eq!(app.tabs[app.active].title(), "a.rs");
        assert_eq!(app.tabs[app.active].view.editor().selection(), Some(0..6));
        ctrl(&mut app, 'f');
        ctrl(&mut app, 'f');
        let dialog = app.project_search.as_ref().unwrap();
        assert_eq!(dialog.query(), "needle");
        assert_eq!(dialog.input().edit().selection(), Some(0..6));
        assert_eq!(dialog.selected().unwrap().line, 0);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tabs[app.active].title(), "b.rs");
        assert_eq!(app.tabs[app.active].view.editor().selection(), Some(3..9));
    }

    #[test]
    fn ctrl_f_with_no_file_open_is_project_search() {
        let (_dir, mut app) = project_with_files(&[("a.rs", "needle\n")]);
        assert!(app.tabs.is_empty());
        ctrl(&mut app, 'f');
        assert!(app.search_box.is_none());
        assert!(app.project_search_open);
        type_str(&mut app, "needle");
        wait_for_project_search(&app);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tabs[app.active].title(), "a.rs");
        assert_eq!(app.tabs[app.active].view.editor().selection(), Some(0..6));
        // With a file open, Ctrl+F is the local search again.
        ctrl(&mut app, 'f');
        assert!(app.search_box.is_some());
        assert!(!app.project_search_open);
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
        let row = boxed(&screen[3]);
        assert!(row.contains("…") && row.contains("needle"), "{row:?}");
        assert!(row.ends_with("yyy…  …/dir/name/f.rs:1│"), "{row:?}");
        assert!(
            !row.contains("xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"),
            "{row:?}"
        );
        let row = boxed(&screen[4]);
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

    #[test]
    fn tick_reloads_files_changed_on_disk_and_says_so() {
        let (dir, mut app) = app_with_files(&[("a.rs", "one\ntwo\n")]);
        std::fs::write(dir.path().join("a.rs"), "one\ntwo\nthree\n").unwrap();
        assert!(app.tick());
        assert_eq!(
            app.tabs[app.active].view.editor().buffer().to_text(),
            "one\ntwo\nthree\n"
        );
        let status = app.status.clone().unwrap_or_default();
        assert!(
            status.contains("a.rs changed on disk: reloaded"),
            "{status}"
        );
    }

    #[test]
    fn discard_changes_command_reloads_the_active_file() {
        let (_dir, mut app) = app_with_files(&[("a.rs", "one\ntwo\n")]);
        type_str(&mut app, "x");
        assert!(app.tabs[app.active].view.editor().is_modified());
        app.run_command(Command::DiscardChanges);
        assert!(!app.tabs[app.active].view.editor().is_modified());
        assert_eq!(
            app.tabs[app.active].view.editor().buffer().to_text(),
            "one\ntwo\n"
        );
        let status = app.status.clone().unwrap_or_default();
        assert!(
            status.starts_with("Discarded unsaved changes to a.rs"),
            "{status}"
        );
        // It is an ordinary edit to undo.
        app.run_command(Command::Undo);
        assert_eq!(
            app.tabs[app.active].view.editor().buffer().to_text(),
            "xone\ntwo\n"
        );
    }

    #[test]
    fn regaining_focus_checks_open_files_right_away() {
        let (dir, mut app) = app_with_files(&[("a.rs", "one\ntwo\n")]);
        // The first idle tick checks; after that, idle ticks wait for a
        // hint or the interval.
        app.tick();
        std::fs::write(dir.path().join("a.rs"), "one\ntwo\nthree\n").unwrap();
        assert!(app.handle_app_event(AppEvent::Terminal(Event::FocusGained)));
        assert_eq!(
            app.tabs[app.active].view.editor().buffer().to_text(),
            "one\ntwo\nthree\n"
        );
        // Losing focus, and regaining it with nothing changed, need no
        // redraw.
        assert!(!app.handle_app_event(AppEvent::Terminal(Event::FocusLost)));
        assert!(!app.handle_app_event(AppEvent::Terminal(Event::FocusGained)));
    }

    #[test]
    fn idle_ticks_pick_up_a_change_from_the_watcher_or_the_interval() {
        let (dir, mut app) = app_with_files(&[("a.rs", "one\ntwo\n")]);
        app.tick();
        std::fs::write(dir.path().join("a.rs"), "one\ntwo\nthree\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.tabs[app.active].view.editor().buffer().to_text() != "one\ntwo\nthree\n" {
            assert!(Instant::now() < deadline, "change on disk never noticed");
            std::thread::sleep(Duration::from_millis(50));
            app.tick();
        }
        let status = app.status.clone().unwrap_or_default();
        assert!(
            status.contains("a.rs changed on disk: reloaded"),
            "{status}"
        );
    }
}
