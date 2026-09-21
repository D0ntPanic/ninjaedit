//! The changes page (Ctrl+U): a mode, like the git log, that stands in
//! for the editor in the upper part of the screen and shows what is
//! changed in the working tree and not yet committed, for reviewing
//! the changes, staging them, and committing them; and, after a merge,
//! for seeing which files are in conflict and committing the merge once
//! they are resolved.
//!
//! The left of the page is the file lists: `Unstaged changes` above
//! (the working directory against the index: modified, deleted, and
//! untracked files, and any in conflict, marked `U`) and `Staged
//! changes` below (the index against HEAD), each a tree of directories
//! as the git log lists a commit's files (see the core crate's
//! `git::tree` module), each file with the letter `git status` gives
//! its change and how many lines it gained and lost, and each directory
//! with the sums. The rest of the page is the diff of the selected
//! file, drawn as on the git log page (see the `diff_pane` module): an
//! unstaged file against the index, a staged one against HEAD, and a
//! conflicted one against our side of the merge, so the conflict
//! markers and the other side's lines show as additions. A selected
//! submodule shows the graph of the commits it moved over, and under
//! it a summary of the uncommitted changes inside it, if any, since a
//! move of its commit alone doesn't show that there is more to commit
//! on its tab. A selected directory shows what is under it, with the
//! counts. Under the diff
//! is the commit box: a line saying where the commit goes (`Commit to
//! main`, `Merge into main`, `Amend on main`, or the conflicts that
//! stand in the way) with the amend toggle at its right, and the
//! message in an editor of its own, with every line and key the file
//! editor has, since a message is a summary line and then as much as
//! the change needs explaining. The box sits under the diff rather
//! than under the lists so that it is as wide as the code being
//! reviewed: the lists are kept narrow to leave the diff room, and a
//! message's lines want the room too. On a page too narrow for a diff
//! the box goes under the lists.
//!
//! The keyboard is in one pane at a time; Tab and Shift+Tab move it
//! round (the lists, the diff, the commit box), and clicking a pane
//! moves it there. In a list ↑ and ↓ move
//! between rows, running on from the end of the unstaged list into
//! the staged one and from there into the commit box; ← and → fold and
//! unfold a directory (← on a file goes to its directory); Space
//! stages what is selected, a file or a whole directory (or unstages
//! it, in the staged list), and `a` stages or unstages everything;
//! Enter or → on a file moves into its diff, and Enter on a directory
//! folds or unfolds it; `o` opens the file in the editor, at its first
//! conflict if it has one; and `m` toggles amending. Ctrl+S, from any
//! pane, makes the commit with the message in the box, as the person
//! the repository's configuration names; a merge in progress starts
//! with the message git prepared for it. In the diff the arrows scroll,
//! Page Up and Page Down by a screenful, and ← goes back to the list
//! when nothing is scrolled sideways. The wheel scrolls whichever pane
//! it is over.
//!
//! Amending (the toggle beside the commit heading, `m` in a list, or
//! the command palette's "Toggle amend") makes the commit replace the
//! last one, as `git commit --amend` does: the staged list becomes
//! everything the amended commit will hold, the last commit's changes
//! among them, unstaging puts a file back to the version before that
//! commit (its change moves to the unstaged list), and the message box
//! starts with the last commit's message. The toggle is off again once
//! the commit is made.
//!
//! The rules between the panes drag to resize them: the file lists'
//! right edge, the rule between the two lists, and the rule between
//! the diff and the commit box. The sizes are kept per repository in
//! the project's storage (see the `git_layout` module) once a drag
//! ends. The diff is the larger side to start with.
//!
//! The working tree is scanned when the page opens and again after
//! every stage, unstage, and commit; the scan runs on a worker thread
//! (see the core crate's `git::changes` module), and the status bar
//! says so while it does. Ctrl+U on the page scans again, for changes
//! made elsewhere: a file saved in the editor, a `git` command in a
//! shell. A directory folded by hand stays folded across scans.
//!
//! A submodule with uncommitted changes of its own gets a tab, after
//! the main repository's, in the bar where a file's tab would be, so
//! that its changes can be committed first and the parent's then show
//! the new commit to stage; a nested submodule's changes make its
//! parent's a changed one too, so the tabs follow the changes down.
//! The tabs come and go with the changes: a submodule committed clean
//! loses its tab. Ctrl+T on the page picks a tab, as it picks a file
//! tab in the editor, and `o` on a submodule in a list goes to its tab
//! rather than opening it as a file; a submodule with no tab (its
//! commit moved, but nothing is changed inside it) has nothing to go
//! to, and the status bar says so.

use crate::clipboard::Clipboard;
use crate::diff_pane::{
    Button, HScroll, Piece, WHEEL_COLUMNS, WHEEL_LINES, clamp_between, content_background,
    diff_extent, display_width, draw_cells, fit_end, layout_cells, render_diff, share_for,
    share_of, text_extent,
};
use crate::editor_view::EditorView;
use crate::git_layout::{ChangesSizes, GitChangesLayout, MAIN_REPOSITORY};
use crate::palette::palette_background;
use crate::status::StatusLine;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ninjaedit_core::git::{ChangeKind, Changes, FileChange, FileDiff, FileTree, TreeRow, short_id};
use ninjaedit_core::{Editor, FileBuffer};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::{Color, Modifier, Style};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The name of the page, shown in its tab.
pub const TITLE: &str = "Git Changes";
/// The file lists' width, within these bounds, as a share of the page.
const MIN_FILES_WIDTH: u16 = 24;
const MAX_FILES_WIDTH: u16 = 60;
/// The least the diff keeps when the lists are widened.
const MIN_CONTENT_WIDTH: u16 = 16;
/// The least a list keeps: its heading and one row.
const MIN_LIST_HEIGHT: u16 = 2;
/// The least the diff keeps above the commit box.
const MIN_CONTENT_HEIGHT: u16 = 5;
/// The commit box's height: the rule above it, its heading, and the
/// message's rows, at least one; and how many rows it starts with.
const MIN_COMMIT_HEIGHT: u16 = 3;
const DEFAULT_COMMIT_HEIGHT: u16 = 9;
const UNSTAGED_HEADING: &str = "Unstaged changes";
const STAGED_HEADING: &str = "Staged changes";
const AMEND_ON: &str = "[x] amend";
const AMEND_OFF: &str = "[ ] amend";
const NOT_A_REPOSITORY: &str = "Not a git repository";
const NOT_INITIALIZED: &str = "Submodule not initialized";
const CLEAN: &str = "Nothing to commit: the working tree is clean";
const NO_UNSTAGED: &str = "No unstaged changes";
const NO_STAGED: &str = "No staged changes";
const SCANNING: &str = "scanning…";
/// The key bindings the status bar lists, pane by pane.
const UNSTAGED_HELP: &[(&str, &str)] = &[
    ("↑↓", "move"),
    ("←→", "fold"),
    ("Space", "stage"),
    ("a", "all"),
    ("Enter", "diff"),
    ("o", "open"),
    ("m", "amend"),
    ("Ctrl+S", "commit"),
    ("Tab", "pane"),
];
const STAGED_HELP: &[(&str, &str)] = &[
    ("↑↓", "move"),
    ("←→", "fold"),
    ("Space", "unstage"),
    ("a", "all"),
    ("Enter", "diff"),
    ("o", "open"),
    ("m", "amend"),
    ("Ctrl+S", "commit"),
    ("Tab", "pane"),
];
const COMMIT_HELP: &[(&str, &str)] = &[
    ("", "type the message"),
    ("Ctrl+S", "commit"),
    ("Tab", "pane"),
    ("Ctrl+E", "leave"),
];
const CONTENT_HELP: &[(&str, &str)] = &[
    ("↑↓", "scroll"),
    ("←→", "sideways"),
    ("▲▼", "expand context"),
    ("Tab", "pane"),
    ("Ctrl+E", "leave"),
];

/// Which pane has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Unstaged,
    Staged,
    Commit,
    Content,
}

impl Pane {
    /// The pane after this one: down the lists, then the diff, then
    /// the commit box under it.
    fn next(self) -> Pane {
        match self {
            Pane::Unstaged => Pane::Staged,
            Pane::Staged => Pane::Content,
            Pane::Content => Pane::Commit,
            Pane::Commit => Pane::Unstaged,
        }
    }

    fn previous(self) -> Pane {
        match self {
            Pane::Unstaged => Pane::Commit,
            Pane::Staged => Pane::Unstaged,
            Pane::Content => Pane::Staged,
            Pane::Commit => Pane::Content,
        }
    }
}

/// One of the two file lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum List {
    Unstaged,
    Staged,
}

impl List {
    fn pane(self) -> Pane {
        match self {
            List::Unstaged => Pane::Unstaged,
            List::Staged => Pane::Staged,
        }
    }
}

/// A rule between panes that can be dragged to resize them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Divider {
    /// The file lists' right edge.
    Files,
    /// Between the unstaged and the staged list.
    Lists,
    /// Between the diff and the commit box.
    Commit,
}

/// What the diff pane shows.
enum Content {
    /// A line saying why there is no diff: nothing selected.
    Message(String),
    /// What is under a selected directory.
    Directory(Vec<Piece>),
    Diff(Box<FileDiff>),
    Failed(String),
}

/// What a row of a list stands for, by path, for telling whether the
/// diff pane shows the selection and for keeping the selection across
/// a rebuild of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RowKey {
    Dir(String),
    File(String),
}

/// What the application should do after the page handled a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangesOutcome {
    Continue,
    /// Something to say in the status bar: a commit made, an action
    /// that failed.
    Notice(StatusLine),
    /// Open a file in the editor; `conflicted` asks for the cursor at
    /// its first merge conflict.
    OpenFile {
        path: PathBuf,
        conflicted: bool,
    },
}

/// One of the file lists: its files as a tree, which directories are
/// folded, the rows that leaves to show, and the selection among them.
#[derive(Default)]
struct FileList {
    tree: FileTree,
    dirs_collapsed: Vec<bool>,
    rows: Vec<TreeRow>,
    selected: usize,
    scroll: usize,
    reveal: bool,
    /// The paths the tree was built from, to notice when they change.
    paths: Vec<String>,
    /// The directories folded by hand, by path, kept across rebuilds
    /// (even through the list emptying and filling again).
    folded: BTreeSet<String>,
}

impl FileList {
    /// Build the tree afresh when the files changed, keeping the
    /// directories folded by hand folded and the selection on the same
    /// row when it is still there (else where it was, or the end).
    fn rebuild(&mut self, files: &[FileChange]) {
        let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        if paths == self.paths {
            return;
        }
        let selected = self.selected_key();
        self.tree = FileTree::new(paths.iter().map(String::as_str));
        self.dirs_collapsed = self
            .tree
            .dirs()
            .iter()
            .map(|dir| self.folded.contains(&dir.path))
            .collect();
        self.paths = paths;
        self.rows = self.tree.rows(&self.dirs_collapsed);
        let kept =
            selected.and_then(|key| self.rows.iter().position(|row| self.key_of(*row) == key));
        self.selected = kept
            .unwrap_or(self.selected)
            .min(self.rows.len().saturating_sub(1));
    }

    fn key_of(&self, row: TreeRow) -> RowKey {
        match row {
            TreeRow::Dir { dir, .. } => RowKey::Dir(self.tree.dirs()[dir].path.clone()),
            TreeRow::File { file, .. } => {
                RowKey::File(self.paths.get(file).cloned().unwrap_or_default())
            }
        }
    }

    fn selected_key(&self) -> Option<RowKey> {
        self.rows.get(self.selected).map(|row| self.key_of(*row))
    }

    fn selected_row(&self) -> Option<TreeRow> {
        self.rows.get(self.selected).copied()
    }

    fn select(&mut self, index: usize) {
        self.selected = index.min(self.rows.len().saturating_sub(1));
        self.reveal = true;
    }

    /// Select the row of a directory of the tree.
    fn select_dir(&mut self, dir: usize) {
        if let Some(index) = self
            .rows
            .iter()
            .position(|row| matches!(row, TreeRow::Dir { dir: d, .. } if *d == dir))
        {
            self.select(index);
        }
    }

    /// Fold or unfold a directory. A selected row folded out of sight
    /// leaves the selection on the directory.
    fn set_dir_collapsed(&mut self, dir: usize, collapsed: bool) {
        if self.dirs_collapsed.get(dir).copied() != Some(!collapsed) {
            return;
        }
        let selected = self.selected_key();
        self.dirs_collapsed[dir] = collapsed;
        let path = self.tree.dirs()[dir].path.clone();
        if collapsed {
            self.folded.insert(path);
        } else {
            self.folded.remove(&path);
        }
        self.rows = self.tree.rows(&self.dirs_collapsed);
        let kept =
            selected.and_then(|key| self.rows.iter().position(|row| self.key_of(*row) == key));
        match kept {
            Some(index) => self.select(index),
            None => self.select_dir(dir),
        }
    }

    /// The files a row stands for: one, or every file under a
    /// directory.
    fn files_of(&self, row: TreeRow) -> Vec<usize> {
        match row {
            TreeRow::Dir { dir, .. } => self.tree.files_under(dir),
            TreeRow::File { file, .. } => vec![file],
        }
    }
}

/// One repository's tab of the page: the main repository or a
/// submodule with changes, with its page once it has been shown.
struct ChangesTab {
    title: String,
    workdir: PathBuf,
    /// Whether the tab is a submodule, opened as exactly that directory
    /// rather than whatever repository contains it.
    submodule: bool,
    view: Option<ChangesView>,
}

impl ChangesTab {
    /// The tab's key in the layout.
    fn key(&self) -> &str {
        if self.submodule {
            &self.title
        } else {
            MAIN_REPOSITORY
        }
    }
}

/// The page's tabs: the project's repository first, then each
/// submodule with uncommitted changes, nested ones included, by path,
/// as the main repository's scan finds them. A submodule's page is
/// opened the first time its tab is shown, with the pane sizes kept
/// for it.
pub struct ChangesTabs {
    tabs: Vec<ChangesTab>,
    active: usize,
    /// The pane sizes of every repository, as loaded and as dragged.
    layout: GitChangesLayout,
}

impl ChangesTabs {
    /// The tabs for the repository containing `root`, with the main
    /// repository's page open and sized as `layout` has it. The
    /// submodules' tabs appear once the scan finds them.
    pub fn new(root: &Path, layout: GitChangesLayout) -> ChangesTabs {
        let mut main = ChangesView::new(root);
        main.set_sizes(layout.get(MAIN_REPOSITORY));
        ChangesTabs {
            tabs: vec![ChangesTab {
                title: TITLE.to_owned(),
                workdir: root.to_path_buf(),
                submodule: false,
                view: Some(main),
            }],
            active: 0,
            layout,
        }
    }

    /// The pane sizes of every repository, to keep.
    pub fn layout(&self) -> &GitChangesLayout {
        &self.layout
    }

    /// The tabs' titles, in order: the page's name, then each
    /// submodule's path.
    pub fn titles(&self) -> impl Iterator<Item = &str> {
        self.tabs.iter().map(|tab| tab.title.as_str())
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    /// Show a tab, opening its page if this is its first showing and
    /// scanning again if not, since the tree may have changed since.
    pub fn set_active(&mut self, index: usize) {
        if index < self.tabs.len() && index != self.active {
            self.active = index;
            let opened = self.tabs[index].view.is_some();
            let view = self.active();
            if opened {
                view.refresh();
            }
        }
    }

    /// The shown tab's page.
    pub fn active(&mut self) -> &mut ChangesView {
        let tab = &mut self.tabs[self.active];
        let sizes = self.layout.get(tab.key());
        tab.view.get_or_insert_with(|| {
            let mut view = if tab.submodule {
                ChangesView::for_repository(&tab.workdir)
            } else {
                ChangesView::new(&tab.workdir)
            };
            view.set_sizes(sizes);
            view
        })
    }

    /// The shown tab's page, if opened (it is, once shown).
    pub fn active_view(&self) -> Option<&ChangesView> {
        self.tabs[self.active].view.as_ref()
    }

    /// The shown tab's repository as its page opens it: the working
    /// directory, and whether as exactly that directory (a
    /// submodule's) rather than whatever repository contains it.
    pub fn active_repository(&self) -> (PathBuf, bool) {
        let tab = &self.tabs[self.active];
        (tab.workdir.clone(), tab.submodule)
    }

    /// Scan every opened page's working tree again.
    pub fn refresh(&mut self) {
        for tab in &mut self.tabs {
            if let Some(view) = &mut tab.view {
                view.refresh();
            }
        }
    }

    /// Take in the scans the pages' workers have finished, and follow
    /// the main repository's for the tabs. Returns whether the page
    /// needs redrawing.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        let mut main_changed = false;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            if let Some(view) = &mut tab.view
                && view.poll()
            {
                changed = true;
                main_changed |= index == 0;
            }
        }
        if main_changed {
            self.sync_tabs();
        }
        changed
    }

    /// Make the tabs the main repository's changed submodules, keeping
    /// the pages of those still listed. A shown tab that is gone (its
    /// submodule committed clean) gives way to the main repository's.
    fn sync_tabs(&mut self) {
        let Some(main) = &self.tabs[0].view else {
            return;
        };
        let wanted = main.changed_submodules();
        let shown = self.tabs[self.active].title.clone();
        let mut old: Vec<ChangesTab> = self.tabs.drain(1..).collect();
        for submodule in wanted {
            let view = old
                .iter()
                .position(|tab| tab.title == submodule.path)
                .and_then(|index| old.remove(index).view);
            self.tabs.push(ChangesTab {
                title: submodule.path,
                workdir: submodule.workdir,
                submodule: true,
                view,
            });
        }
        self.active = self
            .tabs
            .iter()
            .position(|tab| tab.title == shown)
            .unwrap_or(0);
    }

    /// Scan the other opened pages again after an action on the shown
    /// one: a submodule's commit changes what its parent shows.
    fn refresh_others(&mut self) {
        let active = self.active;
        if self.tabs[active]
            .view
            .as_mut()
            .is_some_and(ChangesView::take_acted)
        {
            for (index, tab) in self.tabs.iter_mut().enumerate() {
                if index != active
                    && let Some(view) = &mut tab.view
                {
                    view.refresh();
                }
            }
        }
    }

    /// Give the shown page a key.
    pub fn handle_key(&mut self, key: KeyEvent, clipboard: &mut Clipboard) -> ChangesOutcome {
        let outcome = self.active().handle_key(key, clipboard);
        self.refresh_others();
        self.follow_submodule(outcome)
    }

    /// Show the tab of the submodule the shown page asks for, when `o`
    /// or "Open changed file" was on a submodule in a list. The tab is
    /// named by the submodule's path from the main repository, which
    /// for a submodule of a submodule is the shown tab's path and then
    /// its own. A submodule with no tab has no uncommitted changes of
    /// its own (its commit moved, or it isn't initialized), so there
    /// is nothing to show, and `outcome` becomes a notice saying so.
    fn follow_submodule(&mut self, outcome: ChangesOutcome) -> ChangesOutcome {
        let Some(path) = self.active().take_submodule_to_show() else {
            return outcome;
        };
        let tab = &self.tabs[self.active];
        let title = if tab.submodule {
            format!("{}/{}", tab.title, path)
        } else {
            path.clone()
        };
        match self.tabs.iter().position(|tab| tab.title == title) {
            Some(index) => {
                self.set_active(index);
                outcome
            }
            None => ChangesOutcome::Notice(StatusLine::info(format!(
                "{path} has no uncommitted changes of its own"
            ))),
        }
    }

    /// Turn amending on or off on the shown page.
    pub fn toggle_amend(&mut self) -> ChangesOutcome {
        let outcome = self.active().toggle_amend();
        self.refresh_others();
        outcome
    }

    /// Commit what is staged on the shown page, as its Ctrl+S does.
    pub fn commit(&mut self) -> ChangesOutcome {
        let outcome = self.active().commit();
        self.refresh_others();
        outcome
    }

    /// Stage everything on the shown page.
    pub fn stage_all(&mut self) -> ChangesOutcome {
        let outcome = self.active().stage_all();
        self.refresh_others();
        outcome
    }

    /// Unstage everything on the shown page.
    pub fn unstage_all(&mut self) -> ChangesOutcome {
        let outcome = self.active().unstage_all();
        self.refresh_others();
        outcome
    }

    /// Open the file selected on the shown page, as its `o` does, or
    /// show the selected submodule's tab.
    pub fn open_selected(&mut self) -> ChangesOutcome {
        let outcome = self.active().open_selected();
        self.follow_submodule(outcome)
    }

    /// Give the shown page a mouse event. Returns whether the page's
    /// pane sizes changed (a drag of a rule ended), in which case the
    /// layout is worth keeping.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> bool {
        let active = self.active;
        let resized = self.active().handle_mouse(mouse);
        if resized {
            let sizes = self.tabs[active].view.as_ref().map(ChangesView::sizes);
            if let Some(sizes) = sizes {
                let key = self.tabs[active].key().to_owned();
                self.layout.set(&key, sizes);
            }
        }
        self.refresh_others();
        resized
    }

    /// Add pasted text to the commit message, if that is where the
    /// keyboard is.
    pub fn paste(&mut self, text: &str) {
        self.active().paste(text);
    }

    /// Draw the shown page. Returns where the terminal cursor belongs,
    /// if in the commit message.
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) -> Option<ScreenPosition> {
        self.active().render(area, buf, theme)
    }

    /// What the status bar shows.
    pub fn hint(&self) -> StatusLine {
        self.active_view()
            .map(ChangesView::hint)
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub fn is_loading(&self) -> bool {
        self.tabs
            .iter()
            .filter_map(|tab| tab.view.as_ref())
            .any(ChangesView::is_loading)
    }
}

pub struct ChangesView {
    changes: Option<Changes>,
    /// Why there are no changes: the project isn't in a repository.
    error: Option<String>,
    pane: Pane,
    /// The list the diff follows: the one last moved in.
    list: List,
    unstaged: FileList,
    staged: FileList,
    /// What the diff pane shows, and what it was built for.
    shown: Option<(List, RowKey)>,
    content: Option<Content>,
    content_scroll: usize,
    content_h: HScroll,
    /// How many rows the content had when last drawn, and how many of
    /// them were shown.
    content_rows: usize,
    content_shown: usize,
    /// The commit message, in an editor of its own.
    message: EditorView,
    /// Text the page put in the message itself (a merge's prepared
    /// message, the amended commit's), which it may replace or clear
    /// again while the user hasn't changed it.
    auto_message: Option<String>,
    /// Sizes set by dragging the rules between panes, or kept from
    /// last time, as shares of the space each divides.
    files_share: Option<f32>,
    unstaged_share: Option<f32>,
    commit_share: Option<f32>,
    /// The rule being dragged, if any.
    divider_drag: Option<Divider>,
    /// Whether an action changed the repository since last asked, so
    /// that the other repositories' pages can scan again.
    acted: bool,
    /// The submodule whose tab the page asks to have shown, by its
    /// path from this repository, when `o` was on one (see
    /// [`ChangesTabs::follow_submodule`]) and not yet taken.
    submodule_to_show: Option<String>,
    /// The page, its panes, and the rules between them from the last
    /// render.
    area: Rect,
    unstaged_area: Rect,
    staged_area: Rect,
    /// The commit box's heading and message rows.
    commit_area: Rect,
    /// The amend toggle on the heading row.
    amend_area: Rect,
    content_area: Rect,
    files_rule: Rect,
    lists_rule: Rect,
    commit_rule: Rect,
    /// The expand buttons drawn in the diff, to hit-test clicks.
    buttons: Vec<(Rect, Button)>,
}

impl ChangesView {
    /// A page for the repository containing `root`, with its scan
    /// started.
    pub fn new(root: &Path) -> ChangesView {
        ChangesView::from_changes(Changes::open(root).map_err(|err| err.message().to_owned()))
    }

    /// A page for the repository whose working directory is `root`
    /// itself: a submodule's. One that isn't initialized says so.
    pub fn for_repository(root: &Path) -> ChangesView {
        ChangesView::from_changes(
            Changes::open_repository(root).map_err(|_| NOT_INITIALIZED.to_owned()),
        )
    }

    fn from_changes(changes: Result<Changes, String>) -> ChangesView {
        let (changes, error) = match changes {
            Ok(changes) => (Some(changes), None),
            Err(message) => (None, Some(message)),
        };
        ChangesView {
            changes,
            error,
            pane: Pane::Unstaged,
            list: List::Unstaged,
            unstaged: FileList::default(),
            staged: FileList::default(),
            shown: None,
            content: None,
            content_scroll: 0,
            content_h: HScroll::default(),
            content_rows: 0,
            content_shown: 0,
            message: message_editor(""),
            auto_message: None,
            files_share: None,
            unstaged_share: None,
            commit_share: None,
            divider_drag: None,
            acted: false,
            submodule_to_show: None,
            area: Rect::default(),
            unstaged_area: Rect::default(),
            staged_area: Rect::default(),
            commit_area: Rect::default(),
            amend_area: Rect::default(),
            content_area: Rect::default(),
            files_rule: Rect::default(),
            lists_rule: Rect::default(),
            commit_rule: Rect::default(),
            buttons: Vec::new(),
        }
    }

    /// Scan the working tree again.
    pub fn refresh(&mut self) {
        if let Some(changes) = &mut self.changes {
            changes.refresh();
        }
    }

    /// Take in the scan if it has finished. Returns whether the page
    /// needs redrawing.
    pub fn poll(&mut self) -> bool {
        let Some(changes) = &mut self.changes else {
            return false;
        };
        if !changes.poll() {
            return false;
        }
        self.follow_lists();
        true
    }

    /// The lists are new, from a scan or an action: rebuild the trees,
    /// and show the selected file afresh, since it may have changed
    /// too.
    fn follow_lists(&mut self) {
        let Some(changes) = &self.changes else {
            return;
        };
        self.unstaged.rebuild(changes.unstaged());
        self.staged.rebuild(changes.staged());
        self.content = None;
        let merge_message = match changes.merge_message() {
            Some(message) if !changes.is_amending() => Some(message.to_owned()),
            _ => None,
        };
        if let Some(message) = merge_message {
            self.offer_message(message);
        }
    }

    /// Whether a scan is running whose result is being waited for: not
    /// one that only confirms what an action left the lists showing.
    fn scanning(&self) -> bool {
        self.changes
            .as_ref()
            .is_some_and(|changes| changes.is_loading() && !changes.is_confirming())
    }

    /// Put text in the message box on the page's own account, unless
    /// the user has written something else there.
    fn offer_message(&mut self, text: String) {
        let current = self.message_text();
        let untouched = current.trim().is_empty() || Some(current) == self.auto_message;
        if untouched && self.auto_message.as_deref() != Some(text.as_str()) {
            self.message = message_editor(&text);
            self.auto_message = Some(text);
        }
    }

    /// Take back text the page put in the message box, if it is still
    /// as offered.
    fn withdraw_message(&mut self) {
        if self.auto_message.take() == Some(self.message_text()) {
            self.message = message_editor("");
        }
    }

    /// Whether an action changed the repository since last asked.
    fn take_acted(&mut self) -> bool {
        std::mem::take(&mut self.acted)
    }

    /// The submodule whose tab the page asks to have shown, when `o`
    /// was on one and this wasn't asked since.
    fn take_submodule_to_show(&mut self) -> Option<String> {
        self.submodule_to_show.take()
    }

    /// The submodules with uncommitted changes, as the last scan found
    /// them.
    fn changed_submodules(&self) -> Vec<ninjaedit_core::git::Submodule> {
        self.changes
            .as_ref()
            .map(|c| c.changed_submodules().to_vec())
            .unwrap_or_default()
    }

    /// What the status bar shows for the page: the scan while one is
    /// under way, otherwise the pane's key bindings.
    pub fn hint(&self) -> StatusLine {
        if self.scanning() {
            return StatusLine::progress(SCANNING);
        }
        StatusLine::help(match self.pane {
            Pane::Unstaged => UNSTAGED_HELP,
            Pane::Staged => STAGED_HELP,
            Pane::Commit => COMMIT_HELP,
            Pane::Content => CONTENT_HELP,
        })
    }

    #[cfg(test)]
    pub fn is_loading(&self) -> bool {
        self.changes.as_ref().is_some_and(Changes::is_loading)
    }

    /// Where the rule between the lists was drawn.
    #[cfg(test)]
    pub fn lists_rule(&self) -> Rect {
        self.lists_rule
    }

    /// The commit message typed so far.
    pub fn message_text(&self) -> String {
        String::from_utf8_lossy(&self.message.editor().buffer().to_bytes()).into_owned()
    }

    // ----- Selection ------------------------------------------------------

    fn files(&self, list: List) -> &[FileChange] {
        match (&self.changes, list) {
            (Some(changes), List::Unstaged) => changes.unstaged(),
            (Some(changes), List::Staged) => changes.staged(),
            (None, _) => &[],
        }
    }

    fn file_list(&self, list: List) -> &FileList {
        match list {
            List::Unstaged => &self.unstaged,
            List::Staged => &self.staged,
        }
    }

    fn file_list_mut(&mut self, list: List) -> &mut FileList {
        match list {
            List::Unstaged => &mut self.unstaged,
            List::Staged => &mut self.staged,
        }
    }

    /// The row selected in the list the diff follows, if it has any.
    fn selected_row(&self) -> Option<(List, TreeRow)> {
        let list = self.list;
        self.file_list(list).selected_row().map(|row| (list, row))
    }

    /// The file selected in the list the diff follows, if a file is.
    fn selected_change(&self) -> Option<(List, &FileChange)> {
        match self.selected_row()? {
            (list, TreeRow::File { file, .. }) => {
                self.files(list).get(file).map(|change| (list, change))
            }
            _ => None,
        }
    }

    /// Select a row of a list, and make it the list the diff follows.
    fn select(&mut self, list: List, index: usize) {
        self.list = list;
        self.file_list_mut(list).select(index);
    }

    /// Move the keyboard to a list, selecting in it.
    fn enter_list(&mut self, list: List, index: usize) {
        self.pane = list.pane();
        self.select(list, index);
    }

    /// Whether the commit box is drawn: the page is tall enough.
    fn has_commit_box(&self) -> bool {
        self.commit_area.height > 0
    }

    // ----- The diff --------------------------------------------------------

    /// Build what the diff pane shows for the selection, if it isn't
    /// built.
    fn ensure_content(&mut self) {
        let wanted = self
            .selected_row()
            .map(|(list, row)| (list, self.file_list(list).key_of(row)));
        if self.content.is_some() && self.shown == wanted {
            return;
        }
        if self.shown != wanted {
            self.content_scroll = 0;
            self.content_h.col = 0;
        }
        self.shown = wanted;
        let Some(changes) = &self.changes else {
            return;
        };
        let scanning = self.scanning();
        self.content = Some(match self.selected_row() {
            None => Content::Message(
                if scanning && changes.is_clean() {
                    SCANNING
                } else if changes.is_clean() {
                    CLEAN
                } else if self.list == List::Unstaged {
                    NO_UNSTAGED
                } else {
                    NO_STAGED
                }
                .to_owned(),
            ),
            Some((list, TreeRow::Dir { dir, .. })) => {
                Content::Directory(self.directory_lines(list, dir))
            }
            Some((list, TreeRow::File { file, .. })) => match self.files(list).get(file) {
                Some(change) => {
                    let diff = match list {
                        List::Unstaged => changes.unstaged_diff(change),
                        List::Staged => changes.staged_diff(change),
                    };
                    match diff {
                        Ok(diff) => Content::Diff(Box::new(diff)),
                        Err(err) => Content::Failed(err.message().to_owned()),
                    }
                }
                None => Content::Message(String::new()),
            },
        });
    }

    /// The lines for a directory of a list: what is under it, with the
    /// counts.
    fn directory_lines(&self, list: List, dir: usize) -> Vec<Piece> {
        let files = self.files(list);
        let file_list = self.file_list(list);
        let Some(entry) = file_list.tree.dirs().get(dir) else {
            return Vec::new();
        };
        let plain = Style::default();
        let under = file_list.tree.files_under(dir);
        let (additions, deletions) = under
            .iter()
            .filter_map(|f| files.get(*f))
            .fold((0, 0), |(a, d), f| (a + f.additions, d + f.deletions));
        let mut lines: Vec<Piece> = vec![
            (
                format!("{}/", entry.path),
                plain.add_modifier(Modifier::BOLD),
            ),
            (
                format!(
                    "{} {}, +{additions} −{deletions}",
                    under.len(),
                    if under.len() == 1 { "file" } else { "files" }
                ),
                plain,
            ),
            (String::new(), plain),
        ];
        let prefix = format!("{}/", entry.path);
        for file in under.iter().filter_map(|f| files.get(*f)) {
            let path = file.path.strip_prefix(&prefix).unwrap_or(&file.path);
            lines.push((
                format!("{} {path}  {}", file.kind.letter(), counts_of(file).trim()),
                plain,
            ));
        }
        lines
    }

    /// The columns the diff pane's visible lines reach: `rows` rows from
    /// `scroll`.
    fn content_extent(&self, scroll: usize, rows: usize) -> usize {
        match &self.content {
            Some(Content::Diff(diff)) => diff_extent(diff, &diff.rows(), scroll, rows),
            Some(Content::Directory(lines)) => text_extent(lines, scroll, rows),
            _ => 0,
        }
    }

    /// Scroll the diff sideways by `columns`.
    fn scroll_content_sideways(&mut self, columns: isize) {
        let extent = self.content_extent(self.content_scroll, self.content_shown);
        self.content_h.scroll_by(columns, extent);
    }

    // ----- Actions ---------------------------------------------------------

    /// Stage what is selected in the unstaged list, or unstage what is
    /// selected in the staged list: a file, or every file under a
    /// directory.
    fn toggle_selected(&mut self, list: List) -> ChangesOutcome {
        let Some(row) = self.file_list(list).selected_row() else {
            return ChangesOutcome::Continue;
        };
        let files: Vec<FileChange> = self
            .file_list(list)
            .files_of(row)
            .into_iter()
            .filter_map(|f| self.files(list).get(f).cloned())
            .collect();
        if files.is_empty() {
            return ChangesOutcome::Continue;
        }
        let what = match row {
            TreeRow::Dir { dir, .. } => {
                format!("{}/", self.file_list(list).tree.dirs()[dir].path)
            }
            TreeRow::File { .. } => files[0].path.clone(),
        };
        self.apply(list, &files, &what)
    }

    /// The command palette's "Stage all changes": `a` in the unstaged
    /// list.
    pub fn stage_all(&mut self) -> ChangesOutcome {
        self.toggle_all(List::Unstaged)
    }

    /// The command palette's "Unstage all changes": `a` in the staged
    /// list.
    pub fn unstage_all(&mut self) -> ChangesOutcome {
        self.toggle_all(List::Staged)
    }

    /// Whether there is anything to stage.
    pub fn has_unstaged(&self) -> bool {
        !self.files(List::Unstaged).is_empty()
    }

    /// Whether there is anything to unstage.
    pub fn has_staged(&self) -> bool {
        !self.files(List::Staged).is_empty()
    }

    /// Whether a file is selected in the list the diff follows, for
    /// `o` to open.
    pub fn has_selected_change(&self) -> bool {
        self.selected_change().is_some()
    }

    /// Stage every unstaged file, or unstage every staged one.
    fn toggle_all(&mut self, list: List) -> ChangesOutcome {
        let files = self.files(list).to_vec();
        if files.is_empty() {
            return ChangesOutcome::Continue;
        }
        self.apply(list, &files, "everything")
    }

    /// Stage files (from the unstaged list) or unstage them (from the
    /// staged list).
    fn apply(&mut self, list: List, files: &[FileChange], what: &str) -> ChangesOutcome {
        let Some(changes) = &mut self.changes else {
            return ChangesOutcome::Continue;
        };
        let result = match list {
            List::Unstaged => changes.stage(files.iter().map(|f| f.path.as_str())),
            List::Staged => changes.unstage(
                files
                    .iter()
                    .flat_map(|f| std::iter::once(f.path.as_str()).chain(f.old_path.as_deref())),
            ),
        };
        self.acted = true;
        self.content = None;
        match result {
            Ok(()) => {
                self.follow_lists();
                ChangesOutcome::Continue
            }
            Err(err) => ChangesOutcome::Notice(StatusLine::error(format!(
                "Could not {} {what}: {}",
                if list == List::Unstaged {
                    "stage"
                } else {
                    "unstage"
                },
                err.message()
            ))),
        }
    }

    /// Turn amending on or off: the commit replaces the last one, or
    /// follows it. The message box takes the last commit's message
    /// while amending, unless the user has written one.
    pub fn toggle_amend(&mut self) -> ChangesOutcome {
        let Some(changes) = &mut self.changes else {
            return ChangesOutcome::Continue;
        };
        let amend = !changes.is_amending();
        if let Err(err) = changes.set_amend(amend) {
            return ChangesOutcome::Notice(StatusLine::error(format!(
                "Could not amend: {}",
                err.message()
            )));
        }
        self.acted = true;
        self.content = None;
        if amend {
            if let Some(message) = changes.head_message().map(str::to_owned) {
                self.offer_message(message);
            }
        } else {
            self.withdraw_message();
        }
        ChangesOutcome::Continue
    }

    /// Commit what is staged with the message in the box.
    pub fn commit(&mut self) -> ChangesOutcome {
        let message = self.message_text();
        let Some(changes) = &mut self.changes else {
            return ChangesOutcome::Continue;
        };
        let amended = changes.is_amending();
        match changes.commit(&message) {
            Ok(id) => {
                self.acted = true;
                self.message = message_editor("");
                self.auto_message = None;
                self.follow_lists();
                let summary = message.trim().lines().next().unwrap_or("").to_owned();
                let verb = if amended { "Amended" } else { "Committed" };
                ChangesOutcome::Notice(StatusLine::info(format!(
                    "{verb} {} {summary}",
                    short_id(id)
                )))
            }
            Err(err) => ChangesOutcome::Notice(StatusLine::error(format!(
                "Could not commit: {}",
                err.message()
            ))),
        }
    }

    /// Open the selected file in the editor. A submodule isn't a file
    /// to open: its changes are on its own tab, which the page asks to
    /// have shown (see [`ChangesTabs::follow_submodule`]).
    pub fn open_selected(&mut self) -> ChangesOutcome {
        let Some((_, change)) = self.selected_change() else {
            return ChangesOutcome::Continue;
        };
        let Some(changes) = &self.changes else {
            return ChangesOutcome::Continue;
        };
        if change.submodule {
            self.submodule_to_show = Some(change.path.clone());
            return ChangesOutcome::Continue;
        }
        if change.kind == ChangeKind::Deleted {
            return ChangesOutcome::Notice(StatusLine::info(format!("{} is deleted", change.path)));
        }
        ChangesOutcome::OpenFile {
            path: changes.workdir().join(&change.path),
            conflicted: change.kind == ChangeKind::Conflicted,
        }
    }

    // ----- Input ----------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent, clipboard: &mut Clipboard) -> ChangesOutcome {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Tab if !shift => {
                self.pane = self.pane.next();
                if self.pane == Pane::Commit && !self.has_commit_box() {
                    self.pane = self.pane.next();
                }
                return ChangesOutcome::Continue;
            }
            KeyCode::BackTab | KeyCode::Tab => {
                self.pane = self.pane.previous();
                if self.pane == Pane::Commit && !self.has_commit_box() {
                    self.pane = self.pane.previous();
                }
                return ChangesOutcome::Continue;
            }
            // The save key makes the commit, from any pane: there is
            // nothing else on the page to save.
            KeyCode::Char('s') if ctrl => return self.commit(),
            _ => {}
        }
        match self.pane {
            Pane::Unstaged => self.handle_list_key(List::Unstaged, key),
            Pane::Staged => self.handle_list_key(List::Staged, key),
            Pane::Commit => {
                self.message.handle_key(key, clipboard);
                ChangesOutcome::Continue
            }
            Pane::Content => {
                self.handle_content_key(key);
                ChangesOutcome::Continue
            }
        }
    }

    fn handle_list_key(&mut self, list: List, key: KeyEvent) -> ChangesOutcome {
        let area = match list {
            List::Unstaged => self.unstaged_area,
            List::Staged => self.staged_area,
        };
        let page = (area.height as usize).saturating_sub(2).max(1);
        let count = self.file_list(list).rows.len();
        let selected = self.file_list(list).selected;
        let row = self.file_list(list).selected_row();
        // Moving in a list makes it the one the diff follows, even at
        // its ends.
        self.list = list;
        match key.code {
            KeyCode::Up if selected == 0 || count == 0 => {
                if list == List::Staged && !self.unstaged.rows.is_empty() {
                    self.enter_list(List::Unstaged, usize::MAX);
                }
            }
            KeyCode::Up => self.select(list, selected - 1),
            KeyCode::Down if selected + 1 >= count => {
                if list == List::Unstaged && !self.staged.rows.is_empty() {
                    self.enter_list(List::Staged, 0);
                } else if self.has_commit_box() {
                    self.pane = Pane::Commit;
                }
            }
            KeyCode::Down => self.select(list, selected + 1),
            KeyCode::PageUp => self.select(list, selected.saturating_sub(page)),
            KeyCode::PageDown => self.select(list, selected + page),
            KeyCode::Home => self.select(list, 0),
            KeyCode::End => self.select(list, usize::MAX),
            KeyCode::Enter => match row {
                Some(TreeRow::Dir { dir, collapsed, .. }) => {
                    self.file_list_mut(list).set_dir_collapsed(dir, !collapsed);
                }
                Some(TreeRow::File { .. }) if self.content_area.width > 0 => {
                    self.pane = Pane::Content;
                }
                _ => {}
            },
            KeyCode::Right => match row {
                Some(TreeRow::Dir {
                    dir,
                    collapsed: true,
                    ..
                }) => self.file_list_mut(list).set_dir_collapsed(dir, false),
                // An open directory: on to its first entry.
                Some(TreeRow::Dir { .. }) => self.select(list, selected + 1),
                Some(TreeRow::File { .. }) if self.content_area.width > 0 => {
                    self.pane = Pane::Content;
                }
                _ => {}
            },
            KeyCode::Left => match row {
                Some(TreeRow::Dir {
                    dir,
                    collapsed: false,
                    ..
                }) => self.file_list_mut(list).set_dir_collapsed(dir, true),
                Some(TreeRow::Dir { dir, .. }) => {
                    if let Some(parent) = self.file_list(list).tree.dirs()[dir].parent {
                        self.file_list_mut(list).select_dir(parent);
                    }
                }
                Some(TreeRow::File {
                    parent: Some(parent),
                    ..
                }) => self.file_list_mut(list).select_dir(parent),
                _ => {}
            },
            KeyCode::Char(' ') => return self.toggle_selected(list),
            KeyCode::Char('a') => return self.toggle_all(list),
            KeyCode::Char('o') => return self.open_selected(),
            KeyCode::Char('m') => return self.toggle_amend(),
            _ => {}
        }
        ChangesOutcome::Continue
    }

    fn handle_content_key(&mut self, key: KeyEvent) {
        let page = (self.content_area.height as usize).saturating_sub(1).max(1);
        match key.code {
            KeyCode::Up => self.content_scroll = self.content_scroll.saturating_sub(1),
            KeyCode::Down => self.content_scroll += 1,
            KeyCode::PageUp => self.content_scroll = self.content_scroll.saturating_sub(page),
            KeyCode::PageDown => self.content_scroll += page,
            KeyCode::Home => self.content_scroll = 0,
            KeyCode::End => self.content_scroll = usize::MAX,
            KeyCode::Right => self.scroll_content_sideways(WHEEL_COLUMNS as isize),
            KeyCode::Left if self.content_h.col > 0 => {
                self.scroll_content_sideways(-(WHEEL_COLUMNS as isize));
            }
            KeyCode::Left => self.pane = self.list.pane(),
            _ => {}
        }
    }

    /// Add pasted text to the commit message, if that is where the
    /// keyboard is.
    pub fn paste(&mut self, text: &str) {
        if self.pane == Pane::Commit {
            self.message.editor_mut().paste(text);
        }
    }

    /// Whether the screen position is over the page.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.area.contains(ScreenPosition::new(x, y))
    }

    /// Whether a scrollbar, a rule between panes, or a selection in the
    /// message is being dragged, in which case the page wants drag and
    /// release events wherever they happen.
    pub fn is_dragging(&self) -> bool {
        self.content_h.dragging || self.divider_drag.is_some() || self.message.is_dragging()
    }

    // ----- Resizing the panes -----------------------------------------------

    /// The pane sizes as shares, to keep for next time.
    pub fn sizes(&self) -> ChangesSizes {
        ChangesSizes {
            files: self.files_share,
            unstaged: self.unstaged_share,
            commit: self.commit_share,
        }
    }

    /// Size the panes as kept from last time.
    pub fn set_sizes(&mut self, sizes: ChangesSizes) {
        self.files_share = sizes.files;
        self.unstaged_share = sizes.unstaged;
        self.commit_share = sizes.commit;
    }

    /// The file lists' width for the page's width: what was dragged to,
    /// or a share of the page, within what leaves the diff its minimum.
    /// A page too narrow for both is all lists.
    fn files_width_for(&self, width: u16) -> u16 {
        if width < MIN_FILES_WIDTH + 1 + MIN_CONTENT_WIDTH {
            return width;
        }
        let wanted = match self.files_share {
            Some(share) => share_of(width, share),
            None => (width / 3).clamp(MIN_FILES_WIDTH, MAX_FILES_WIDTH),
        };
        let most = width.saturating_sub(MIN_CONTENT_WIDTH + 1);
        clamp_between(wanted, MIN_FILES_WIDTH, most)
    }

    /// The commit box's height (its rule included) for the page's
    /// height: its share, within what leaves what is above it (the
    /// diff, or on a narrow page the lists) its minimum; none on a
    /// page too short for both.
    fn commit_height_for(&self, height: u16) -> u16 {
        let lists_min = MIN_LIST_HEIGHT + 1 + MIN_LIST_HEIGHT;
        let above_min = MIN_CONTENT_HEIGHT.max(lists_min);
        if height < above_min + MIN_COMMIT_HEIGHT {
            return 0;
        }
        let wanted = match self.commit_share {
            Some(share) => share_of(height, share),
            None => DEFAULT_COMMIT_HEIGHT,
        };
        clamp_between(wanted, MIN_COMMIT_HEIGHT, height - above_min)
    }

    /// The file lists' height for the page's size: the whole height,
    /// unless the page is too narrow for a diff, in which case the
    /// commit box is under the lists and takes its share of it.
    fn lists_height_for(&self, width: u16, height: u16) -> u16 {
        if self.files_width_for(width) < width {
            height
        } else {
            height - self.commit_height_for(height)
        }
    }

    /// The unstaged list's height for the lists' height: its share,
    /// within what leaves the staged list its minimum; too short for
    /// both, it is all unstaged.
    fn unstaged_height_for(&self, height: u16) -> u16 {
        if height < MIN_LIST_HEIGHT + 1 + MIN_LIST_HEIGHT {
            return height;
        }
        let wanted = share_of(height, self.unstaged_share.unwrap_or(0.5));
        clamp_between(
            wanted,
            MIN_LIST_HEIGHT,
            height.saturating_sub(MIN_LIST_HEIGHT + 1),
        )
    }

    /// Move a rule to where the mouse is, as far as the panes allow,
    /// and keep the share that settles on.
    fn drag_divider(&mut self, divider: Divider, x: u16, y: u16) {
        match divider {
            Divider::Files => {
                let width = self.area.width.max(1);
                self.files_share = Some(share_for(x.saturating_sub(self.area.x), width));
                let settled = self.files_width_for(width);
                self.files_share = Some(share_for(settled, width));
            }
            Divider::Lists => {
                let height = self
                    .lists_height_for(self.area.width, self.area.height)
                    .max(1);
                self.unstaged_share = Some(share_for(y.saturating_sub(self.area.y), height));
                let settled = self.unstaged_height_for(height);
                self.unstaged_share = Some(share_for(settled, height));
            }
            Divider::Commit => {
                let height = self.area.height.max(1);
                self.commit_share = Some(share_for(self.area.bottom().saturating_sub(y), height));
                let settled = self.commit_height_for(height);
                self.commit_share = Some(share_for(settled, height));
            }
        }
    }

    /// Handle a mouse event. Returns whether the pane sizes changed:
    /// a drag of a rule ended.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> bool {
        let at = ScreenPosition::new(mouse.column, mouse.row);
        // A rule being dragged owns the mouse until the button comes up.
        if let Some(divider) = self.divider_drag {
            match mouse.kind {
                MouseEventKind::Drag(_) => self.drag_divider(divider, mouse.column, mouse.row),
                MouseEventKind::Up(_) => {
                    self.divider_drag = None;
                    return true;
                }
                _ => {}
            }
            return false;
        }
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            let divider = if self.files_rule.contains(at) {
                Some(Divider::Files)
            } else if self.lists_rule.contains(at) {
                Some(Divider::Lists)
            } else if self.commit_rule.contains(at) {
                Some(Divider::Commit)
            } else {
                None
            };
            if let Some(divider) = divider {
                self.divider_drag = Some(divider);
                self.drag_divider(divider, mouse.column, mouse.row);
                return false;
            }
        }
        // A scrollbar drag, or a drag in the message, owns the mouse
        // likewise until the button comes up.
        if self.is_dragging()
            && matches!(mouse.kind, MouseEventKind::Drag(_) | MouseEventKind::Up(_))
        {
            match mouse.kind {
                MouseEventKind::Drag(_) if self.content_h.dragging => {
                    let extent = self.content_extent(self.content_scroll, self.content_shown);
                    self.content_h.scroll_to(mouse.column, extent);
                }
                MouseEventKind::Up(_) if self.content_h.dragging => {
                    self.content_h.dragging = false;
                }
                _ => self.message.handle_mouse(mouse),
            }
            return false;
        }
        let pane = if self.unstaged_area.contains(at) {
            Some(Pane::Unstaged)
        } else if self.staged_area.contains(at) {
            Some(Pane::Staged)
        } else if self.commit_area.contains(at) {
            Some(Pane::Commit)
        } else if self.content_area.contains(at) {
            Some(Pane::Content)
        } else {
            None
        };
        let Some(pane) = pane else {
            return false;
        };
        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if pane == Pane::Commit => {
                self.message.handle_mouse(mouse);
            }
            MouseEventKind::ScrollUp => self.scroll_pane(pane, false),
            MouseEventKind::ScrollDown => self.scroll_pane(pane, true),
            MouseEventKind::ScrollRight | MouseEventKind::ScrollLeft if pane == Pane::Content => {
                let columns = if mouse.kind == MouseEventKind::ScrollRight {
                    WHEEL_COLUMNS as isize
                } else {
                    -(WHEEL_COLUMNS as isize)
                };
                self.scroll_content_sideways(columns);
            }
            MouseEventKind::Down(MouseButton::Left) if self.content_h.bar.contains(at) => {
                self.pane = Pane::Content;
                self.content_h.dragging = true;
                let extent = self.content_extent(self.content_scroll, self.content_shown);
                self.content_h.scroll_to(mouse.column, extent);
            }
            MouseEventKind::Down(MouseButton::Left) if self.amend_area.contains(at) => {
                self.toggle_amend();
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.pane = pane;
                match pane {
                    Pane::Unstaged | Pane::Staged => {
                        let (list, area) = if pane == Pane::Unstaged {
                            (List::Unstaged, self.unstaged_area)
                        } else {
                            (List::Staged, self.staged_area)
                        };
                        self.list = list;
                        // The heading row selects nothing; a directory
                        // folds or unfolds when clicked.
                        if mouse.row > area.y {
                            let row =
                                self.file_list(list).scroll + (mouse.row - area.y - 1) as usize;
                            if row < self.file_list(list).rows.len() {
                                self.select(list, row);
                                if let Some(TreeRow::Dir { dir, collapsed, .. }) =
                                    self.file_list(list).selected_row()
                                {
                                    self.file_list_mut(list).set_dir_collapsed(dir, !collapsed);
                                }
                            }
                        }
                    }
                    Pane::Commit => {
                        if mouse.row > self.commit_area.y {
                            self.message.handle_mouse(mouse);
                        }
                    }
                    Pane::Content => {
                        if let Some((_, button)) =
                            self.buttons.iter().find(|(area, _)| area.contains(at))
                            && let Some(Content::Diff(diff)) = &mut self.content
                        {
                            button.press(diff);
                        }
                    }
                }
            }
            _ => {}
        }
        false
    }

    fn scroll_pane(&mut self, pane: Pane, down: bool) {
        let step = |value: usize| {
            if down {
                value + WHEEL_LINES
            } else {
                value.saturating_sub(WHEEL_LINES)
            }
        };
        match pane {
            Pane::Unstaged => self.unstaged.scroll = step(self.unstaged.scroll),
            Pane::Staged => self.staged.scroll = step(self.staged.scroll),
            Pane::Commit => {}
            Pane::Content => self.content_scroll = step(self.content_scroll),
        }
    }

    // ----- Rendering ------------------------------------------------------

    /// Draw the page into `area`. Returns where the terminal cursor
    /// belongs: in the commit message, while that has the keyboard.
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) -> Option<ScreenPosition> {
        self.area = area;
        let background = palette_background(theme);
        buf.set_style(area, background);
        self.buttons.clear();
        self.unstaged_area = Rect::default();
        self.staged_area = Rect::default();
        self.commit_area = Rect::default();
        self.amend_area = Rect::default();
        self.content_area = Rect::default();
        self.files_rule = Rect::default();
        self.lists_rule = Rect::default();
        self.commit_rule = Rect::default();
        if area.height == 0 || area.width < 8 {
            return None;
        }
        if self.changes.is_none() {
            let message = match &self.error {
                Some(error) if error == NOT_INITIALIZED => error.clone(),
                Some(error) if !error.is_empty() => format!("{NOT_A_REPOSITORY}: {error}"),
                _ => NOT_A_REPOSITORY.to_owned(),
            };
            let dim = background.fg(theme.command_palette_result_context_text);
            buf.set_stringn(
                area.x + 2,
                area.y + 1,
                &message,
                area.width as usize - 2,
                dim,
            );
            return None;
        }
        self.ensure_content();

        // The lists down the left, the diff and the commit box under
        // it down the right; rules between them. A page too narrow for
        // a diff is all lists, with the commit box under them.
        let rule = background.fg(theme.gutter_guide);
        let files_width = self.files_width_for(area.width);
        let has_content = files_width < area.width;
        let commit_height = self.commit_height_for(area.height);
        let lists_height = self.lists_height_for(area.width, area.height);
        let (commit_x, commit_width) = if has_content {
            let x = area.x + files_width + 1;
            (x, area.right() - x)
        } else {
            (area.x, files_width)
        };
        if has_content {
            let x = area.x + files_width;
            self.files_rule = Rect::new(x, area.y, 1, area.height);
            for y in area.y..area.bottom() {
                buf[(x, y)].set_symbol("│").set_style(rule);
            }
            self.content_area =
                Rect::new(commit_x, area.y, commit_width, area.height - commit_height);
        }
        // A horizontal rule across a column, joined to the vertical
        // rule beside it (crossing it when a rule from the other side
        // meets it on the same row).
        let horizontal_rule = |buf: &mut Buffer, y: u16, x: u16, width: u16, junction: &str| {
            for x in x..x + width {
                buf[(x, y)].set_symbol("─").set_style(rule);
            }
            if has_content {
                let cell = &mut buf[(area.x + files_width, y)];
                let symbol = if cell.symbol() == "│" {
                    junction
                } else {
                    "┼"
                };
                cell.set_symbol(symbol).set_style(rule);
            }
        };
        let unstaged_height = self.unstaged_height_for(lists_height);
        self.unstaged_area = Rect::new(area.x, area.y, files_width, unstaged_height);
        if unstaged_height < lists_height {
            let y = area.y + unstaged_height;
            self.lists_rule = Rect::new(area.x, y, files_width, 1);
            horizontal_rule(buf, y, area.x, files_width, "┤");
            self.staged_area =
                Rect::new(area.x, y + 1, files_width, area.y + lists_height - (y + 1));
        }
        if commit_height > 0 {
            let y = area.bottom() - commit_height;
            self.commit_rule = Rect::new(commit_x, y, commit_width, 1);
            horizontal_rule(buf, y, commit_x, commit_width, "├");
            self.commit_area = Rect::new(commit_x, y + 1, commit_width, commit_height - 1);
        }

        self.render_list(List::Unstaged, buf, theme);
        self.render_list(List::Staged, buf, theme);
        let cursor = self.render_commit_box(buf, theme);
        self.render_content(buf, theme);
        cursor
    }

    /// The background of a list's selected row: brighter when the pane
    /// has the keyboard than when it doesn't.
    fn selection_background(&self, pane: Pane, theme: &Theme) -> Color {
        if self.pane == pane {
            theme.list_selection_background
        } else {
            theme.unfocused_list_selection_background
        }
    }

    fn render_list(&mut self, list: List, buf: &mut Buffer, theme: &Theme) {
        let (area, heading_text) = match list {
            List::Unstaged => (self.unstaged_area, UNSTAGED_HEADING),
            List::Staged => (self.staged_area, STAGED_HEADING),
        };
        if area.height == 0 || area.width == 0 {
            return;
        }
        let width = area.width as usize;
        let background = palette_background(theme);
        let heading = background
            .fg(theme.active_tab_text)
            .add_modifier(Modifier::BOLD);
        let dim = background.fg(theme.command_palette_result_context_text);
        let count = self.files(list).len();
        let loading = self.scanning();
        let title = if count > 0 {
            format!("{heading_text} ({count})")
        } else {
            heading_text.to_owned()
        };
        buf.set_stringn(area.x + 1, area.y, &title, width.saturating_sub(1), heading);
        let rows = area.height as usize - 1;
        if rows == 0 {
            return;
        }
        if count == 0 {
            let note = if loading { SCANNING } else { "none" };
            buf.set_stringn(area.x + 3, area.y + 1, note, width.saturating_sub(3), dim);
            return;
        }
        let selected_style = background.bg(self.selection_background(list.pane(), theme));
        let added = theme.diff_added_text;
        let removed = theme.diff_removed_text;
        let dir_style = theme.active_tab_text;
        // Keep the selection in view, and the scroll within the list.
        let file_list = self.file_list_mut(list);
        let total = file_list.rows.len();
        file_list.scroll = file_list.scroll.min(total.saturating_sub(rows));
        if file_list.reveal {
            file_list.reveal = false;
            if file_list.selected < file_list.scroll {
                file_list.scroll = file_list.selected;
            } else if file_list.selected >= file_list.scroll + rows {
                file_list.scroll = file_list.selected + 1 - rows;
            }
        }
        let scroll = file_list.scroll;
        let selected = file_list.selected;
        let files = self.files(list);
        let file_list = self.file_list(list);
        for (row, entry) in file_list.rows.iter().enumerate().skip(scroll).take(rows) {
            let y = area.y + 1 + (row - scroll) as u16;
            let row_style = if row == selected {
                buf.set_style(Rect::new(area.x, y, area.width, 1), selected_style);
                selected_style
            } else {
                background
            };
            let indent = 1 + entry.depth() * 2;
            let x = area.x + indent as u16;
            // A directory: its arrow and label, and the sums of what is
            // under it. A file: the kind's letter, the file's name (its
            // end kept when cut), and its counts at the right edge.
            let (label, label_x, label_style, counts) = match *entry {
                TreeRow::Dir { dir, collapsed, .. } => {
                    let arrow = if collapsed { "▸" } else { "▾" };
                    let (additions, deletions) = file_list
                        .tree
                        .files_under(dir)
                        .into_iter()
                        .filter_map(|f| files.get(f))
                        .fold((0, 0), |(a, d), f| (a + f.additions, d + f.deletions));
                    let counts = if additions + deletions > 0 {
                        format!(" +{additions} −{deletions}")
                    } else {
                        String::new()
                    };
                    (
                        format!("{arrow} {}", file_list.tree.dirs()[dir].label),
                        x,
                        row_style.fg(dir_style),
                        counts,
                    )
                }
                TreeRow::File { file, .. } => {
                    let Some(file) = files.get(file) else {
                        continue;
                    };
                    let letter_style = match file.kind {
                        ChangeKind::Added | ChangeKind::Copied | ChangeKind::Untracked => {
                            row_style.fg(added)
                        }
                        ChangeKind::Deleted => row_style.fg(removed),
                        ChangeKind::Conflicted => {
                            row_style.fg(removed).add_modifier(Modifier::BOLD)
                        }
                        _ => row_style.fg(theme.git_tag_text),
                    };
                    buf.set_stringn(x, y, file.kind.letter().to_string(), 1, letter_style);
                    let name = file.path.rsplit('/').next().unwrap_or(&file.path);
                    let name = match &file.old_path {
                        Some(old) => format!("{name} (from {old})"),
                        None => name.to_owned(),
                    };
                    (name, x + 2, row_style, counts_of(file))
                }
            };
            let counts_width = display_width(&counts);
            let label_width = width.saturating_sub((label_x - area.x) as usize + counts_width);
            buf.set_stringn(
                label_x,
                y,
                fit_end(&label, label_width),
                label_width,
                label_style,
            );
            if counts_width > 0 && counts_width <= width {
                let x = area.right() - counts_width as u16;
                if let Some((plus, minus)) = counts.split_once(" −") {
                    let (x, _) = buf.set_stringn(x, y, plus, counts_width, row_style.fg(added));
                    buf.set_stringn(
                        x,
                        y,
                        format!(" −{minus}"),
                        counts_width,
                        row_style.fg(removed),
                    );
                } else if counts.trim() == "conflict" {
                    buf.set_string(x, y, &counts, row_style.fg(removed));
                } else {
                    buf.set_string(
                        x,
                        y,
                        &counts,
                        row_style.fg(theme.command_palette_result_context_text),
                    );
                }
            }
        }
    }

    /// The commit box's heading: where the commit goes, or what stands
    /// in its way.
    fn commit_heading(&self, theme: &Theme) -> (String, Style) {
        let background = palette_background(theme);
        let heading = background
            .fg(theme.active_tab_text)
            .add_modifier(Modifier::BOLD);
        let Some(changes) = &self.changes else {
            return (String::new(), heading);
        };
        let conflicts = changes.conflict_count();
        if conflicts > 0 {
            let noun = if conflicts == 1 {
                "conflict"
            } else {
                "conflicts"
            };
            return (
                format!("Resolve {conflicts} {noun}"),
                background
                    .fg(theme.diff_removed_text)
                    .add_modifier(Modifier::BOLD),
            );
        }
        let branch = changes.head_branch();
        let text = if changes.is_amending() {
            match branch {
                Some(branch) => format!("Amend on {branch}"),
                None => "Amend (detached HEAD)".to_owned(),
            }
        } else {
            match (branch, changes.is_merging()) {
                (Some(branch), true) => format!("Merge into {branch}"),
                (None, true) => "Merge (detached HEAD)".to_owned(),
                (Some(branch), false) if changes.is_unborn() => {
                    format!("First commit on {branch}")
                }
                (Some(branch), false) => format!("Commit to {branch}"),
                (None, false) => "Commit (detached HEAD)".to_owned(),
            }
        };
        (text, heading)
    }

    fn render_commit_box(&mut self, buf: &mut Buffer, theme: &Theme) -> Option<ScreenPosition> {
        let area = self.commit_area;
        if area.height < 2 || area.width < 4 {
            return None;
        }
        let (text, style) = self.commit_heading(theme);
        let amending = self.changes.as_ref().is_some_and(Changes::is_amending);
        let toggle = if amending { AMEND_ON } else { AMEND_OFF };
        let toggle_width = display_width(toggle) as u16;
        // The toggle at the right of the heading row, the heading cut
        // short of it.
        let toggle_x = area.right().saturating_sub(toggle_width + 1);
        let heading_width = toggle_x.saturating_sub(area.x + 2) as usize;
        buf.set_stringn(area.x + 1, area.y, &text, heading_width, style);
        if toggle_x > area.x {
            self.amend_area = Rect::new(toggle_x, area.y, toggle_width, 1);
            let toggle_style = palette_background(theme).fg(if amending {
                theme.git_head_text
            } else {
                theme.command_palette_result_context_text
            });
            buf.set_string(toggle_x, area.y, toggle, toggle_style);
        }
        let editor_area = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
        let cursor = self.message.render(editor_area, buf, theme);
        if self.pane == Pane::Commit {
            cursor
        } else {
            None
        }
    }

    fn render_content(&mut self, buf: &mut Buffer, theme: &Theme) {
        let area = self.content_area;
        if area.width < 4 || area.height == 0 {
            return;
        }
        let background = content_background(theme);
        buf.set_style(area, background);
        let dim = background.fg(theme.command_palette_result_context_text);
        let height = area.height as usize;
        let Some(content) = &self.content else {
            return;
        };
        match content {
            Content::Message(message) | Content::Failed(message) => {
                buf.set_stringn(area.x + 1, area.y, message, area.width as usize - 1, dim);
                self.content_h
                    .render(0, area.width as usize, Rect::default(), buf, theme);
                self.content_rows = 1;
                self.content_shown = 1;
                self.content_scroll = 0;
            }
            Content::Directory(lines) => {
                self.content_rows = lines.len();
                let capacity = area.width as usize - 1;
                // The scrollbar takes the last row; see `render_diff`.
                // Clamp the scroll asked for afresh on each pass, as
                // `render_diff` does, so the last row is reachable once
                // the bar takes a row.
                let wanted = self.content_scroll;
                let mut show_bar = false;
                let mut shown;
                let mut extent;
                loop {
                    shown = height - usize::from(show_bar);
                    self.content_scroll = wanted.min(lines.len().saturating_sub(shown));
                    extent = text_extent(lines, self.content_scroll, shown);
                    let needed = self.content_h.needs_bar(extent, capacity) && height > 1;
                    if needed && !show_bar {
                        show_bar = true;
                        continue;
                    }
                    break;
                }
                self.content_shown = shown;
                for (row, (line, style)) in lines
                    .iter()
                    .skip(self.content_scroll)
                    .take(shown)
                    .enumerate()
                {
                    let y = area.y + row as u16;
                    let cells = layout_cells(line, &[]);
                    let row_area = Rect::new(area.x + 1, y, area.width - 1, 1);
                    draw_cells(buf, row_area, &cells, self.content_h.col, |_| {
                        background.patch(*style)
                    });
                }
                let bar = if show_bar {
                    Rect::new(area.x + 1, area.bottom() - 1, area.width - 1, 1)
                } else {
                    Rect::default()
                };
                self.content_h.render(extent, capacity, bar, buf, theme);
            }
            Content::Diff(diff) => {
                let mut buttons = Vec::new();
                let drawn = render_diff(
                    diff,
                    area,
                    buf,
                    theme,
                    self.content_scroll,
                    &mut self.content_h,
                    &mut buttons,
                );
                self.content_rows = drawn.rows;
                self.content_shown = drawn.shown;
                self.content_scroll = drawn.scroll;
                self.buttons = buttons;
            }
        }
    }
}

/// An editor for the commit message, holding `text`: the file editor
/// without its gutter, since line numbers mean nothing in a message.
fn message_editor(text: &str) -> EditorView {
    let mut view = EditorView::new(Editor::new(FileBuffer::from_text(text)));
    view.set_gutter(false);
    view
}

/// What a file's row shows at its right edge: its counts, or what it
/// is instead of a file with lines.
fn counts_of(file: &FileChange) -> String {
    if file.submodule {
        " submodule".to_owned()
    } else if file.kind == ChangeKind::Conflicted {
        " conflict".to_owned()
    } else if file.binary {
        " bin".to_owned()
    } else if file.additions + file.deletions > 0 {
        format!(" +{} −{}", file.additions, file.deletions)
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commit_row::HEAD_NODE;
    use crate::diff_pane::UNCOMMITTED_HEADING;
    use crossterm::event::{KeyEventKind, KeyEventState};
    use git2::{Repository, Signature};
    use ninjaedit_core::git::NODE;
    use std::fs;
    use std::time::Duration;

    fn configure_user(repo: &Repository) {
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Ann Author").unwrap();
        config.set_str("user.email", "ann@example.com").unwrap();
    }

    /// Commit `files` on HEAD.
    fn commit_files(repo: &Repository, files: &[(&str, &str)], message: &str) -> git2::Oid {
        let workdir = repo.workdir().unwrap();
        let mut index = repo.index().unwrap();
        for (name, content) in files {
            let path = workdir.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
            index.add_path(Path::new(name)).unwrap();
        }
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = Signature::now("Ann Author", "ann@example.com").unwrap();
        let head = repo.head().ok().and_then(|h| h.target());
        let parents: Vec<git2::Commit<'_>> = head
            .into_iter()
            .map(|id| repo.find_commit(id).unwrap())
            .collect();
        let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &refs)
            .unwrap()
    }

    /// A repository with a commit of `a.rs` and `b.txt`, and then `a.rs`
    /// edited, `b.txt` deleted, and `c.txt` new, all unstaged.
    fn repo_with_changes() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        configure_user(&repo);
        commit_files(
            &repo,
            &[("a.rs", "fn main() {\n    one();\n}\n"), ("b.txt", "b\n")],
            "Base",
        );
        fs::write(
            dir.path().join("a.rs"),
            "fn main() {\n    one();\n    two();\n}\n",
        )
        .unwrap();
        fs::remove_file(dir.path().join("b.txt")).unwrap();
        fs::write(dir.path().join("c.txt"), "c\n").unwrap();
        dir
    }

    fn settle(view: &mut ChangesView) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while view.is_loading() && std::time::Instant::now() < deadline {
            view.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!view.is_loading());
    }

    fn view(dir: &tempfile::TempDir) -> ChangesView {
        let mut view = ChangesView::new(dir.path());
        settle(&mut view);
        view
    }

    fn draw(view: &mut ChangesView, width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf, &Theme::default());
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect()
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn press(view: &mut ChangesView, code: KeyCode) -> ChangesOutcome {
        let mut clipboard = Clipboard::local_only();
        view.handle_key(key(code, KeyModifiers::NONE), &mut clipboard)
    }

    fn ctrl(view: &mut ChangesView, c: char) -> ChangesOutcome {
        let mut clipboard = Clipboard::local_only();
        view.handle_key(key(KeyCode::Char(c), KeyModifiers::CONTROL), &mut clipboard)
    }

    fn type_str(view: &mut ChangesView, s: &str) {
        for c in s.chars() {
            if c == '\n' {
                press(view, KeyCode::Enter);
            } else {
                press(view, KeyCode::Char(c));
            }
        }
    }

    fn click(view: &mut ChangesView, column: u16, row: u16) {
        view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
        view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }

    fn row_with<'a>(screen: &'a [String], text: &str) -> &'a str {
        screen
            .iter()
            .find(|row| row.contains(text))
            .unwrap_or_else(|| panic!("no row with {text:?} in {screen:#?}"))
    }

    /// The left column of the screen, trimmed, from the first row on.
    fn left_column(screen: &[String], width: usize) -> Vec<String> {
        screen
            .iter()
            .map(|row| row.chars().take(width).collect::<String>())
            .map(|row| row.trim_end().to_owned())
            .collect()
    }

    /// The right column of the screen, past the rule at `x`, trimmed.
    fn right_column(screen: &[String], x: usize) -> Vec<String> {
        screen
            .iter()
            .map(|row| row.chars().skip(x + 1).collect::<String>())
            .map(|row| row.trim_end().to_owned())
            .collect()
    }

    fn head_message(dir: &tempfile::TempDir) -> String {
        let repo = Repository::open(dir.path()).unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        head.message().unwrap().to_owned()
    }

    #[test]
    fn changes_are_listed_staged_and_committed() {
        let dir = repo_with_changes();
        let mut view = view(&dir);
        let screen = draw(&mut view, 100, 30);
        let width = view.files_rule.x as usize;
        let left = left_column(&screen, width);
        assert_eq!(left[0], " Unstaged changes (3)");
        assert!(left[1].starts_with(" M a.rs"), "{left:#?}");
        assert!(left[1].ends_with("+1 −0"), "{left:#?}");
        assert!(left[2].starts_with(" D b.txt"), "{left:#?}");
        assert!(left[3].starts_with(" ? c.txt"), "{left:#?}");
        let staged = left.iter().position(|r| r == " Staged changes").unwrap();
        assert_eq!(left[staged + 1].trim(), "none");
        // The commit box is under the diff, as wide as it, so that a
        // message has the room the code does; its heading has the
        // amend toggle at the right.
        let right = right_column(&screen, width);
        let heading = right
            .iter()
            .position(|r| r.starts_with(" Commit to "))
            .unwrap_or_else(|| panic!("{right:#?}"));
        assert!(right[heading].ends_with(AMEND_OFF), "{right:#?}");
        assert!(right[heading - 1].starts_with('─'), "{right:#?}");
        assert!(
            !left.iter().any(|r| r.starts_with(" Commit to ")),
            "{left:#?}"
        );
        assert_eq!(view.commit_area.x, view.content_area.x);
        assert_eq!(view.commit_area.width, view.content_area.width);
        assert_eq!(view.commit_area.bottom(), 30);
        // The message editor under it, with no gutter: no line number,
        // only its scrollbar at the right edge.
        assert!(
            right[heading + 1].trim_end_matches('█').trim().is_empty(),
            "{right:#?}"
        );
        // The diff on the right is the larger side, and follows the
        // selection: a.rs's unstaged change against the index.
        assert!(view.content_area.width > view.unstaged_area.width);
        assert!(row_with(&screen, "+     two();").len() > width);
        assert!(view.hint().text().contains("Space stage"));

        // Down to c.txt shows its whole contents as added; Space stages
        // it, and the selection stays put on the next file.
        press(&mut view, KeyCode::Down);
        press(&mut view, KeyCode::Down);
        let screen = draw(&mut view, 100, 30);
        assert!(screen.iter().any(|r| r.contains("+ c")), "{screen:#?}");
        assert_eq!(
            press(&mut view, KeyCode::Char(' ')),
            ChangesOutcome::Continue
        );
        settle(&mut view);
        let screen = draw(&mut view, 100, 30);
        let left = left_column(&screen, width);
        assert_eq!(left[0], " Unstaged changes (2)");
        let staged = left
            .iter()
            .position(|r| r == " Staged changes (1)")
            .unwrap_or_else(|| panic!("{left:#?}"));
        assert!(left[staged + 1].starts_with(" A c.txt"), "{left:#?}");
        assert_eq!(view.unstaged.selected, 1);
        // Down past the end of the unstaged list goes on into the
        // staged one, whose diff is against HEAD.
        press(&mut view, KeyCode::Down);
        assert_eq!(view.pane, Pane::Staged);
        assert_eq!(view.list, List::Staged);
        let screen = draw(&mut view, 100, 30);
        assert!(screen.iter().any(|r| r.contains("+ c")), "{screen:#?}");
        assert!(view.hint().text().contains("Space unstage"));
        // Space unstages it again; `a` in the unstaged list stages all.
        // The lists show the outcome before the scan confirms it, with
        // no word of waiting.
        press(&mut view, KeyCode::Char(' '));
        assert!(view.is_loading());
        assert!(view.files(List::Staged).is_empty());
        assert_eq!(view.files(List::Unstaged).len(), 3);
        assert!(!view.hint().text().contains(SCANNING), "{}", view.hint());
        settle(&mut view);
        assert!(view.files(List::Staged).is_empty());
        assert_eq!(view.files(List::Unstaged).len(), 3);
        press(&mut view, KeyCode::Up);
        assert_eq!(view.pane, Pane::Unstaged);
        press(&mut view, KeyCode::Char('a'));
        assert!(view.files(List::Unstaged).is_empty());
        assert_eq!(view.files(List::Staged).len(), 3);
        let screen = draw(&mut view, 100, 30);
        assert!(row_with(&screen, NO_UNSTAGED).len() > width, "{screen:#?}");
        assert!(!screen.iter().any(|r| r.contains(SCANNING)), "{screen:#?}");
        settle(&mut view);
        assert!(view.files(List::Unstaged).is_empty());
        assert_eq!(view.files(List::Staged).len(), 3);
        let screen = draw(&mut view, 100, 30);
        assert!(row_with(&screen, NO_UNSTAGED).len() > width);

        // Down from the empty unstaged list goes to the staged one, and
        // from its end to the commit box; a two-paragraph message and
        // Ctrl+S commit.
        press(&mut view, KeyCode::Down);
        assert_eq!(view.pane, Pane::Staged);
        press(&mut view, KeyCode::End);
        press(&mut view, KeyCode::Down);
        assert_eq!(view.pane, Pane::Commit);
        let outcome = ctrl(&mut view, 's');
        assert!(
            matches!(&outcome, ChangesOutcome::Notice(m) if m.text().contains("needs a message")),
            "{outcome:?}"
        );
        type_str(&mut view, "Change things\n\nA body, with the why.");
        assert_eq!(
            view.message_text(),
            "Change things\n\nA body, with the why."
        );
        let screen = draw(&mut view, 100, 30);
        assert!(
            screen.iter().any(|r| r.contains("A body, with the why.")),
            "{screen:#?}"
        );
        let outcome = ctrl(&mut view, 's');
        assert!(
            matches!(&outcome, ChangesOutcome::Notice(m) if m.text().starts_with("Committed ") && m.text().ends_with("Change things")),
            "{outcome:?}"
        );
        assert!(view.take_acted());
        assert_eq!(view.message_text(), "");
        let screen = draw(&mut view, 100, 30);
        assert!(row_with(&screen, CLEAN).len() > width, "{screen:#?}");
        settle(&mut view);
        let screen = draw(&mut view, 100, 30);
        assert!(row_with(&screen, CLEAN).len() > width);
        assert_eq!(head_message(&dir), "Change things\n\nA body, with the why.");
    }

    /// Wait for every opened page's scan.
    fn settle_tabs(tabs: &mut ChangesTabs) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while tabs.is_loading() && std::time::Instant::now() < deadline {
            tabs.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!tabs.is_loading());
        tabs.poll();
    }

    fn press_tabs(tabs: &mut ChangesTabs, code: KeyCode) -> ChangesOutcome {
        let mut clipboard = Clipboard::local_only();
        tabs.handle_key(key(code, KeyModifiers::NONE), &mut clipboard)
    }

    #[test]
    fn opening_a_submodule_goes_to_its_tab() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        configure_user(&repo);
        commit_files(&repo, &[("a.rs", "fn main() {}\n")], "Base");
        let mut submodule = repo
            .submodule("https://example.com/sub.git", Path::new("sub"), true)
            .unwrap();
        let sub = submodule.open().unwrap();
        configure_user(&sub);
        commit_files(&sub, &[("inner.txt", "one\n")], "Inner commit");
        submodule.add_finalize().unwrap();
        commit_files(&repo, &[], "Add submodule");

        // A change inside the submodule gives it a tab; `o` on it in
        // the parent's list goes there rather than opening it.
        fs::write(dir.path().join("sub").join("inner.txt"), "dirty\n").unwrap();
        let mut tabs = ChangesTabs::new(dir.path(), GitChangesLayout::default());
        settle_tabs(&mut tabs);
        assert_eq!(tabs.titles().collect::<Vec<_>>(), vec![TITLE, "sub"]);
        let outcome = press_tabs(&mut tabs, KeyCode::Char('o'));
        assert_eq!(outcome, ChangesOutcome::Continue);
        assert_eq!(tabs.active_index(), 1);
        settle_tabs(&mut tabs);
        let screen = draw(tabs.active(), 100, 30);
        assert!(
            screen.iter().any(|r| r.contains("M inner.txt")),
            "{screen:#?}"
        );
        // The command palette's "Open changed file" does the same.
        tabs.set_active(0);
        let outcome = tabs.open_selected();
        assert_eq!(outcome, ChangesOutcome::Continue);
        assert_eq!(tabs.active_index(), 1);

        // Committed inside the submodule, it has no tab: the parent's
        // change is the move to the new commit, and `o` says there is
        // nothing to go to.
        commit_files(&sub, &[("inner.txt", "two\n")], "Inner two");
        tabs.set_active(0);
        tabs.refresh();
        settle_tabs(&mut tabs);
        assert_eq!(tabs.titles().collect::<Vec<_>>(), vec![TITLE]);
        let outcome = press_tabs(&mut tabs, KeyCode::Char('o'));
        assert!(
            matches!(&outcome, ChangesOutcome::Notice(m) if m.text().contains("sub has no uncommitted changes")),
            "{outcome:?}"
        );
        assert_eq!(tabs.active_index(), 0);
    }

    #[test]
    fn a_submodule_change_shows_its_commits_as_a_graph() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        configure_user(&repo);
        commit_files(&repo, &[("a.rs", "fn main() {}\n")], "Base");
        let mut submodule = repo
            .submodule("https://example.com/sub.git", Path::new("sub"), true)
            .unwrap();
        let sub = submodule.open().unwrap();
        configure_user(&sub);
        let s1 = commit_files(&sub, &[("inner.txt", "one\n")], "Inner commit");
        submodule.add_finalize().unwrap();
        commit_files(&repo, &[], "Add submodule");

        // A change inside the submodule leaves the parent at the same
        // commit: nothing to graph, a note saying so, and the change
        // inside listed under it.
        fs::write(dir.path().join("sub").join("inner.txt"), "dirty\n").unwrap();
        let mut view = view(&dir);
        let screen = draw(&mut view, 100, 30);
        assert!(
            row_with(&screen, "M sub").contains("submodule"),
            "{screen:#?}"
        );
        let heading = row_with(&screen, "Submodule sub: ");
        assert!(
            heading.contains(&format!("{} → {}", short_id(s1), short_id(s1))),
            "{heading}"
        );
        row_with(&screen, "Still at this commit");
        let heading_row = screen
            .iter()
            .position(|r| r.contains(UNCOMMITTED_HEADING))
            .unwrap_or_else(|| panic!("{screen:#?}"));
        assert!(
            screen[heading_row + 1].contains("M inner.txt"),
            "{screen:#?}"
        );

        // Committed inside the submodule, the parent's unstaged change
        // is the move to the new commit, drawn as HEAD; the submodule
        // is clean, so there is no summary.
        let s2 = commit_files(&sub, &[("inner.txt", "two\n")], "Inner two");
        view.refresh();
        settle(&mut view);
        let screen = draw(&mut view, 100, 30);
        let heading = row_with(&screen, "Submodule sub: ");
        assert!(
            heading.contains(&format!("{} → {}", short_id(s1), short_id(s2))),
            "{heading}"
        );
        let two = row_with(&screen, "Inner two");
        assert!(two.contains(HEAD_NODE), "{two}");
        let one = row_with(&screen, "Inner commit");
        assert!(one.contains(NODE) && !one.contains(HEAD_NODE), "{one}");
        assert!(
            screen.iter().any(|r| r.contains(&short_id(s2))),
            "{screen:#?}"
        );
        assert!(!screen.iter().any(|r| r.contains("Still at this commit")));
        assert!(!screen.iter().any(|r| r.contains(UNCOMMITTED_HEADING)));

        // A new file inside on top of the move: the graph stays, and
        // the summary follows it.
        fs::write(dir.path().join("sub").join("extra.txt"), "extra\n").unwrap();
        view.refresh();
        settle(&mut view);
        let screen = draw(&mut view, 100, 30);
        let graph_row = screen
            .iter()
            .position(|r| r.contains("Inner commit"))
            .unwrap();
        let heading_row = screen
            .iter()
            .position(|r| r.contains(UNCOMMITTED_HEADING))
            .unwrap_or_else(|| panic!("{screen:#?}"));
        assert!(heading_row > graph_row, "{screen:#?}");
        assert!(
            screen[heading_row + 1].contains("? extra.txt"),
            "{screen:#?}"
        );
        fs::remove_file(dir.path().join("sub").join("extra.txt")).unwrap();
        view.refresh();
        settle(&mut view);

        // Staged, the same range shows against HEAD; Down from the
        // now empty unstaged list goes to it.
        press(&mut view, KeyCode::Char(' '));
        settle(&mut view);
        press(&mut view, KeyCode::Down);
        let screen = draw(&mut view, 100, 30);
        assert!(
            row_with(&screen, "Submodule sub: ").contains(&short_id(s2)),
            "{screen:#?}"
        );
        assert!(
            row_with(&screen, "Inner two").contains(HEAD_NODE),
            "{screen:#?}"
        );
    }

    #[test]
    fn directories_fold_and_stage_as_one() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        configure_user(&repo);
        commit_files(
            &repo,
            &[
                ("tests/oracles/one.txt", "1\n"),
                ("tests/oracles/two.txt", "2\n"),
                ("tests/other.txt", "o\n"),
                ("src/main.rs", "fn main() {}\n"),
            ],
            "Base",
        );
        for name in [
            "tests/oracles/one.txt",
            "tests/oracles/two.txt",
            "src/main.rs",
        ] {
            fs::write(dir.path().join(name), "changed\n").unwrap();
        }
        fs::write(dir.path().join("tests/oracles/three.txt"), "3\n").unwrap();
        let mut view = view(&dir);
        let screen = draw(&mut view, 100, 30);
        let width = view.files_rule.x as usize;
        let left = left_column(&screen, width);
        assert_eq!(left[0], " Unstaged changes (4)");
        assert!(left[1].starts_with(" ▾ src"), "{left:#?}");
        assert!(left[2].starts_with("   M main.rs"), "{left:#?}");
        assert!(left[3].starts_with(" ▾ tests/oracles"), "{left:#?}");
        assert!(
            left[3].ends_with("+3 −2"),
            "the directory's sums: {left:#?}"
        );
        assert!(left[4].starts_with("   M one.txt"), "{left:#?}");
        assert!(left[5].starts_with("   ? three.txt"), "{left:#?}");
        assert!(left[6].starts_with("   M two.txt"), "{left:#?}");
        // A selected directory's diff pane sums it up; ← folds it and
        // → unfolds it.
        press(&mut view, KeyCode::Down);
        press(&mut view, KeyCode::Down);
        assert!(matches!(
            view.unstaged.selected_row(),
            Some(TreeRow::Dir { dir: 1, .. })
        ));
        let screen = draw(&mut view, 100, 30);
        assert!(
            screen.iter().any(|r| r.contains("3 files, +3 −2")),
            "{screen:#?}"
        );
        assert!(
            screen.iter().any(|r| r.contains("? three.txt  +1 −0")),
            "{screen:#?}"
        );
        press(&mut view, KeyCode::Left);
        let screen = draw(&mut view, 100, 30);
        assert!(
            screen.iter().any(|r| r.contains("▸ tests/oracles")),
            "{screen:#?}"
        );
        assert!(
            !left_column(&screen, width)
                .iter()
                .any(|r| r.contains("one.txt"))
        );
        press(&mut view, KeyCode::Right);
        assert!(matches!(
            view.unstaged.selected_row(),
            Some(TreeRow::Dir {
                collapsed: false,
                ..
            })
        ));
        // Space on the directory stages the three files under it and
        // nothing else; main.rs stays unstaged, and the selection moves
        // on to it.
        press(&mut view, KeyCode::Char(' '));
        settle(&mut view);
        let screen = draw(&mut view, 100, 30);
        let left = left_column(&screen, width);
        assert_eq!(left[0], " Unstaged changes (1)");
        assert!(left[2].starts_with("   M main.rs"), "{left:#?}");
        let staged = left
            .iter()
            .position(|r| r == " Staged changes (3)")
            .unwrap_or_else(|| panic!("{left:#?}"));
        assert!(
            left[staged + 1].starts_with(" ▾ tests/oracles"),
            "{left:#?}"
        );
        assert!(left[staged + 3].starts_with("   A three.txt"), "{left:#?}");
        // In the staged list, ← from a file goes to its directory, and
        // Space there unstages the lot. A directory folded by hand
        // stays folded across the rescan.
        press(&mut view, KeyCode::Tab);
        assert_eq!(view.pane, Pane::Staged);
        press(&mut view, KeyCode::Down);
        assert!(matches!(
            view.staged.selected_row(),
            Some(TreeRow::File { .. })
        ));
        press(&mut view, KeyCode::Left);
        assert!(matches!(
            view.staged.selected_row(),
            Some(TreeRow::Dir { .. })
        ));
        press(&mut view, KeyCode::Left);
        press(&mut view, KeyCode::Char(' '));
        settle(&mut view);
        assert!(view.files(List::Staged).is_empty());
        assert_eq!(view.files(List::Unstaged).len(), 4);
        press(&mut view, KeyCode::BackTab);
        press(&mut view, KeyCode::Char('a'));
        settle(&mut view);
        let screen = draw(&mut view, 100, 30);
        let left = left_column(&screen, width);
        assert!(
            left.iter().any(|r| r.starts_with(" ▸ tests/oracles")),
            "{left:#?}"
        );
        // Clicking a directory row folds or unfolds it.
        let y = screen
            .iter()
            .position(|r| r.contains("▸ tests/oracles"))
            .unwrap() as u16;
        click(&mut view, 4, y);
        assert_eq!(view.pane, Pane::Staged);
        let screen = draw(&mut view, 100, 30);
        assert!(
            screen.iter().any(|r| r.contains("▾ tests/oracles")),
            "{screen:#?}"
        );
    }

    #[test]
    fn amending_folds_the_last_commit_into_the_staged_list() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        configure_user(&repo);
        commit_files(&repo, &[("a.txt", "one\n"), ("b.txt", "b\n")], "Base");
        let head = commit_files(
            &repo,
            &[("a.txt", "one\ntwo\n"), ("c.txt", "c\n")],
            "Add two\n\nWith a body.\n",
        );
        fs::write(dir.path().join("b.txt"), "bee\n").unwrap();
        let mut view = view(&dir);
        let screen = draw(&mut view, 100, 30);
        let width = view.files_rule.x as usize;
        let left = left_column(&screen, width);
        assert!(left.iter().any(|r| r.starts_with(" M b.txt")), "{left:#?}");
        let right = right_column(&screen, width);
        assert!(
            right
                .iter()
                .any(|r| r.starts_with(" Commit to ") && r.ends_with(AMEND_OFF)),
            "{right:#?}"
        );
        // `m` turns amending on: the last commit's files are staged
        // against its parent, the heading says so, and its message is
        // in the box.
        assert_eq!(
            press(&mut view, KeyCode::Char('m')),
            ChangesOutcome::Continue
        );
        settle(&mut view);
        let screen = draw(&mut view, 100, 30);
        let left = left_column(&screen, width);
        let right = right_column(&screen, width);
        assert!(
            right
                .iter()
                .any(|r| r.starts_with(" Amend on ") && r.ends_with(AMEND_ON)),
            "{right:#?}"
        );
        let staged = left
            .iter()
            .position(|r| r == " Staged changes (2)")
            .unwrap_or_else(|| panic!("{left:#?}"));
        assert!(left[staged + 1].starts_with(" M a.txt"), "{left:#?}");
        assert!(left[staged + 2].starts_with(" A c.txt"), "{left:#?}");
        assert_eq!(view.message_text(), "Add two\n\nWith a body.");
        assert!(
            screen.iter().any(|r| r.contains("With a body.")),
            "{screen:#?}"
        );
        // Stage b.txt too, unstage c.txt (back to before the commit:
        // it turns up untracked), and amend with a changed message.
        press(&mut view, KeyCode::Char(' '));
        settle(&mut view);
        press(&mut view, KeyCode::Tab);
        press(&mut view, KeyCode::End);
        assert!(matches!(
            view.staged.selected_row(),
            Some(TreeRow::File { file: 2, .. })
        ));
        press(&mut view, KeyCode::Char(' '));
        settle(&mut view);
        assert_eq!(view.files(List::Staged).len(), 2);
        assert_eq!(view.files(List::Unstaged)[0].kind, ChangeKind::Untracked);
        // Tab goes round the diff to the commit box.
        press(&mut view, KeyCode::Tab);
        assert_eq!(view.pane, Pane::Content);
        press(&mut view, KeyCode::Tab);
        assert_eq!(view.pane, Pane::Commit);
        press(&mut view, KeyCode::End);
        type_str(&mut view, " and bee");
        let outcome = ctrl(&mut view, 's');
        assert!(
            matches!(&outcome, ChangesOutcome::Notice(m) if m.text().starts_with("Amended ")),
            "{outcome:?}"
        );
        settle(&mut view);
        let new_head = repo.head().unwrap().peel_to_commit().unwrap();
        assert_ne!(new_head.id(), head);
        assert_eq!(
            new_head.message().unwrap(),
            "Add two and bee\n\nWith a body."
        );
        assert_eq!(new_head.parent_count(), 1);
        assert!(
            new_head
                .tree()
                .unwrap()
                .get_path(Path::new("c.txt"))
                .is_err()
        );
        // Amending is off again, and the box is empty.
        assert_eq!(view.message_text(), "");
        let right = right_column(&draw(&mut view, 100, 30), width);
        assert!(
            right
                .iter()
                .any(|r| r.starts_with(" Commit to ") && r.ends_with(AMEND_OFF)),
            "{right:#?}"
        );
        // Clicking the toggle turns it on; a message the user has typed
        // is left alone by the toggle, and off again drops the offered
        // one only.
        assert_eq!(view.pane, Pane::Commit);
        type_str(&mut view, "Mine");
        let toggle = view.amend_area;
        click(&mut view, toggle.x + 1, toggle.y);
        settle(&mut view);
        assert!(view.changes.as_ref().unwrap().is_amending());
        assert_eq!(view.message_text(), "Mine");
        click(&mut view, toggle.x + 1, toggle.y);
        settle(&mut view);
        assert!(!view.changes.as_ref().unwrap().is_amending());
        assert_eq!(view.message_text(), "Mine");
        ctrl(&mut view, 'a');
        press(&mut view, KeyCode::Backspace);
        press(&mut view, KeyCode::Tab);
        assert_eq!(view.pane, Pane::Unstaged);
        press(&mut view, KeyCode::Char('m'));
        settle(&mut view);
        assert_eq!(view.message_text(), "Add two and bee\n\nWith a body.");
        press(&mut view, KeyCode::Char('m'));
        settle(&mut view);
        assert_eq!(view.message_text(), "");
    }

    #[test]
    fn a_conflicted_file_is_marked_shown_and_opened() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        configure_user(&repo);
        let base = commit_files(&repo, &[("f.txt", "one\ntwo\nthree\n")], "Base");
        let main = repo.head().unwrap().shorthand().unwrap().to_owned();
        commit_files(&repo, &[("f.txt", "one\nours\nthree\n")], "Ours");
        repo.branch("side", &repo.find_commit(base).unwrap(), false)
            .unwrap();
        repo.set_head("refs/heads/side").unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        let theirs = commit_files(&repo, &[("f.txt", "one\ntheirs\nthree\n")], "Theirs");
        repo.set_head(&format!("refs/heads/{main}")).unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        let annotated = repo.find_annotated_commit(theirs).unwrap();
        repo.merge(&[&annotated], None, None).unwrap();
        fs::write(
            dir.path().join(".git/MERGE_MSG"),
            "Merge branch 'side'\n\nDetails.\n",
        )
        .unwrap();

        let mut view = view(&dir);
        let screen = draw(&mut view, 100, 30);
        let width = view.files_rule.x as usize;
        let left = left_column(&screen, width);
        assert!(left[1].starts_with(" U f.txt"), "{left:#?}");
        assert!(left[1].ends_with("conflict"), "{left:#?}");
        let right = right_column(&screen, width);
        assert!(
            right.iter().any(|r| r.starts_with(" Resolve 1 conflict")),
            "{right:#?}"
        );
        // The merge's whole message is put in the box; the diff shows
        // the markers against our side.
        assert_eq!(view.message_text(), "Merge branch 'side'\n\nDetails.");
        assert!(
            screen.iter().any(|r| r.contains("+ <<<<<<< HEAD")),
            "{screen:#?}"
        );
        assert!(screen.iter().any(|r| r.contains("+ theirs")), "{screen:#?}");
        // `o` opens the file at its conflict. (libgit2 canonicalizes
        // the working directory's path: macOS's `/var` is a symlink.)
        let outcome = press(&mut view, KeyCode::Char('o'));
        let expected = fs::canonicalize(dir.path().join("f.txt")).unwrap();
        assert!(
            matches!(&outcome, ChangesOutcome::OpenFile { path, conflicted: true }
                if fs::canonicalize(path).unwrap() == expected),
            "{outcome:?}"
        );
        // Amending is refused while merging; committing is refused
        // while the conflict stands. Resolving and staging it, then
        // Ctrl+S, makes the merge commit.
        let outcome = press(&mut view, KeyCode::Char('m'));
        assert!(
            matches!(&outcome, ChangesOutcome::Notice(m) if m.text().contains("merge")),
            "{outcome:?}"
        );
        let outcome = ctrl(&mut view, 's');
        assert!(
            matches!(&outcome, ChangesOutcome::Notice(m) if m.text().contains("conflicts")),
            "{outcome:?}"
        );
        fs::write(dir.path().join("f.txt"), "one\nboth\nthree\n").unwrap();
        press(&mut view, KeyCode::Char(' '));
        settle(&mut view);
        let screen = draw(&mut view, 100, 30);
        let left = left_column(&screen, width);
        assert!(left.iter().any(|r| r.starts_with(" M f.txt")), "{left:#?}");
        let right = right_column(&screen, width);
        assert!(
            right.iter().any(|r| r.starts_with(" Merge into ")),
            "{right:#?}"
        );
        let outcome = ctrl(&mut view, 's');
        assert!(
            matches!(&outcome, ChangesOutcome::Notice(m) if m.text().starts_with("Committed")),
            "{outcome:?}"
        );
        settle(&mut view);
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.parent_count(), 2);
        assert_eq!(head.message().unwrap(), "Merge branch 'side'\n\nDetails.");
        assert_eq!(repo.state(), git2::RepositoryState::Clean);
        let right = right_column(&draw(&mut view, 100, 30), width);
        assert!(
            right.iter().any(|r| r.starts_with(" Commit to ")),
            "{right:#?}"
        );
    }

    #[test]
    fn the_rules_drag_to_resize_and_the_panes_take_clicks() {
        let dir = repo_with_changes();
        let mut view = view(&dir);
        draw(&mut view, 120, 40);
        let drag = |view: &mut ChangesView, from: (u16, u16), to: (u16, u16)| {
            view.handle_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: from.0,
                row: from.1,
                modifiers: KeyModifiers::NONE,
            });
            assert!(view.is_dragging());
            view.handle_mouse(MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: to.0,
                row: to.1,
                modifiers: KeyModifiers::NONE,
            });
            let released = view.handle_mouse(MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: to.0,
                row: to.1,
                modifiers: KeyModifiers::NONE,
            });
            assert!(released);
            assert!(!view.is_dragging());
        };
        // The lists widen, keeping the diff its minimum.
        let rule = view.files_rule;
        drag(&mut view, (rule.x, 5), (rule.x + 10, 5));
        draw(&mut view, 120, 40);
        assert_eq!(view.files_rule.x, rule.x + 10);
        drag(&mut view, (rule.x + 10, 5), (300, 5));
        draw(&mut view, 120, 40);
        assert_eq!(view.content_area.width, MIN_CONTENT_WIDTH);
        let widened = view.files_rule.x;
        drag(&mut view, (widened, 5), (rule.x, 5));
        // The unstaged list grows downward, the staged one keeping its
        // minimum; the lists have the whole height, the commit box
        // being under the diff.
        let lists = view.lists_rule;
        let unstaged = view.unstaged_area.height;
        drag(
            &mut view,
            (lists.x + 3, lists.y),
            (lists.x + 3, lists.y + 5),
        );
        draw(&mut view, 120, 40);
        assert_eq!(view.unstaged_area.height, unstaged + 5);
        drag(&mut view, (lists.x + 3, lists.y + 5), (lists.x + 3, 200));
        draw(&mut view, 120, 40);
        assert_eq!(view.staged_area.height, MIN_LIST_HEIGHT);
        assert_eq!(view.staged_area.bottom(), 40);
        assert_eq!(view.commit_area.bottom(), 40);
        assert_eq!(view.commit_rule.x, view.content_area.x);
        assert_eq!(view.commit_rule.right(), 120);
        // The commit box grows upward at the diff's expense, and no
        // further than leaves the diff its minimum; the lists are not
        // touched.
        let commit = view.commit_rule;
        let box_height = view.commit_area.height;
        let content_height = view.content_area.height;
        drag(
            &mut view,
            (commit.x + 3, commit.y),
            (commit.x + 3, commit.y - 4),
        );
        draw(&mut view, 120, 40);
        assert_eq!(view.commit_area.height, box_height + 4);
        assert_eq!(view.commit_rule.y, commit.y - 4);
        assert_eq!(view.content_area.height, content_height - 4);
        drag(&mut view, (commit.x + 3, commit.y - 4), (commit.x + 3, 0));
        draw(&mut view, 120, 40);
        assert_eq!(view.content_area.height, MIN_CONTENT_HEIGHT);
        assert_eq!(view.staged_area.height, MIN_LIST_HEIGHT);
        assert_eq!(view.unstaged_area.bottom(), 40 - 1 - MIN_LIST_HEIGHT);
        let top = view.commit_rule.y;
        drag(&mut view, (commit.x + 3, top), (commit.x + 3, commit.y));
        draw(&mut view, 120, 40);
        assert_eq!(view.commit_rule.y, commit.y);
        // The sizes are shares, and a page given them comes out the same.
        let sizes = view.sizes();
        assert!(sizes.files.is_some() && sizes.unstaged.is_some() && sizes.commit.is_some());
        let mut again = ChangesView::new(dir.path());
        again.set_sizes(sizes);
        settle(&mut again);
        draw(&mut again, 120, 40);
        assert_eq!(again.unstaged_area, view.unstaged_area);
        assert_eq!(again.commit_area, view.commit_area);
        assert_eq!(again.content_area, view.content_area);

        // Clicking a file selects it and focuses its list; clicking the
        // message focuses it, and the cursor goes there; clicking the
        // diff focuses it.
        let screen = draw(&mut view, 120, 40);
        let row = screen.iter().position(|r| r.contains("? c.txt")).unwrap() as u16;
        press(&mut view, KeyCode::Tab);
        assert_eq!(view.pane, Pane::Staged);
        click(&mut view, 5, row);
        assert_eq!(view.pane, Pane::Unstaged);
        assert_eq!(view.unstaged.selected, 2);
        let message_row = view.commit_area.y + 1;
        let message_x = view.commit_area.x + 8;
        click(&mut view, message_x, message_row);
        assert_eq!(view.pane, Pane::Commit);
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        let cursor = view.render(area, &mut buf, &Theme::default());
        assert_eq!(cursor.map(|c| c.y), Some(message_row));
        let content_x = view.content_area.x;
        click(&mut view, content_x + 5, 3);
        assert_eq!(view.pane, Pane::Content);
        let mut buf = Buffer::empty(area);
        assert_eq!(view.render(area, &mut buf, &Theme::default()), None);
        // ← in the diff goes back to the list the diff follows.
        press(&mut view, KeyCode::Left);
        assert_eq!(view.pane, Pane::Unstaged);
    }

    #[test]
    fn a_directory_that_is_not_a_repository_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut view = ChangesView::new(dir.path());
        let screen = draw(&mut view, 60, 10);
        assert!(screen[1].contains(NOT_A_REPOSITORY), "{screen:#?}");
        assert!(!view.poll());
    }
}
