//! The git log page (Ctrl+L): a mode, like the settings and build
//! configuration pages, that stands in for the editor in the upper part
//! of the screen and shows the project's history.
//!
//! The page has four panes. Down the left is the sidebar: the local
//! branches under a `Branches` heading and, under `Remotes`, each
//! remote with its branches beneath it, folded up until opened. The
//! rest of the page is the log above and the selected commit below. The
//! log is `git log --graph --all`: every commit reachable from a branch
//! or tag, newest first, each drawn on two lines beside its place in
//! the graph. The first line has the branches and tags pointing at the
//! commit, then its message; the second its author, its abbreviated id,
//! and when it was made. Each starts right after its own part of the
//! graph rather than in one column for the whole log, so that a few
//! wide rows don't push every message right. The commit HEAD is on is bold and in the
//! theme's `git-head-text`, and is where the log opens, once the walk
//! (see the core crate's `git::history` module) reaches it.
//!
//! Below the log, the left pane lists `Description` and then every file
//! the selected commit changed, as a tree of directories (see the core
//! crate's `git::tree` module) with how many lines each file gained and
//! lost. The tree starts fully open; a directory folds and unfolds with
//! ← and →, Enter, Space, or a click. The right pane shows whichever
//! is selected: the commit's message with who made it and when, a
//! directory's files, or a file's diff. A diff is
//! highlighted as the file would be, over the theme's `diff-added-` and
//! `diff-removed-background` colors, and shows three lines of context
//! around each change. Where lines are hidden between changes a row
//! says how many, with buttons to reveal more: ▲ from the change below
//! it upward, ▼ from the change above it downward, or all of them.
//!
//! The rules between the panes can be dragged to resize them: the
//! sidebar's right edge, the rule under the log, and the rule between
//! the files and the content. The sizes are shares of the space each
//! divides (see the `git_layout` module), within what
//! leaves every pane its minimum, and are kept per repository in the
//! project's storage once a drag ends, so the page comes back as it
//! was left.
//!
//! The keyboard is in one pane at a time; Tab and Shift+Tab move it
//! round, and clicking a pane moves it there. In the sidebar ↑ and ↓
//! move between branches, ← and → fold and unfold a remote (Space too),
//! and Enter goes to the branch's commit in the log. In the log ↑ and ↓
//! move between commits (Page Up and Page Down by a screenful, Home and
//! End to the ends), ← and → scroll the messages sideways (← at the
//! left edge goes back to the sidebar), and Enter moves on to the
//! files. In the files ↑ and ↓ choose what the right pane shows,
//! Enter or → on a file moves into it, ← on a file goes to its
//! directory, and ← at the top of the tree goes back to the log. In the
//! diff the arrows scroll, Page Up and Page Down by a screenful, and ←
//! goes back to the files when nothing is scrolled sideways. The wheel
//! scrolls whichever pane it is over, sideways too.
//!
//! The log and the content pane scroll sideways as the editor does
//! (see `EditorView`): only as far as the longest line on screen now
//! needs, plus a little slack, never by the longest line in the whole
//! history or file; a position further right than the lines now shown
//! reach, left there by scrolling down, stays until scrolled back. A
//! scrollbar appears along the bottom of the pane while anything is
//! out of view, and can be clicked and dragged.
//!
//! A repository with submodules gets a tab for each, after the main
//! repository's, in the bar where a file's tab would be; each tab is a
//! page of its own, opened the first time it is shown. Ctrl+T on the
//! page picks a tab, as it picks a file tab in the editor. A submodule
//! that hasn't been initialized has a tab too, which says so.
//!
//! The page only looks at the repository: checking out a branch or
//! commit will come later.

use crate::git_layout::{GitLogLayout, MAIN_REPOSITORY, PaneSizes};
use crate::palette::palette_background;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ninjaedit_core::git::{
    ChangeKind, CommitDetail, DiffRow, FileDiff, FileTree, History, LineKind, NODE, Oid, RefKind,
    TreeRow, Unshown, cells_for, submodules,
};
use ninjaedit_core::{Token, TokenKind, text};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget};
use std::path::{Path, PathBuf};

/// The name of the page, shown in its tab.
pub const TITLE: &str = "Git Log";
/// The sidebar's width, within these bounds, as a share of the page.
const MIN_SIDEBAR_WIDTH: u16 = 20;
const MAX_SIDEBAR_WIDTH: u16 = 32;
/// The page is all log and detail below this width.
const MIN_WIDTH_FOR_SIDEBAR: u16 = 64;
/// The least the main part of the page (the log and the detail) keeps
/// when the sidebar is widened.
const MIN_MAIN_WIDTH: u16 = 40;
/// The least the log keeps (one commit) and the detail keeps when the
/// rule between them is dragged.
const MIN_LOG_HEIGHT: u16 = 2;
const MIN_DETAIL_HEIGHT: u16 = 2;
/// The least the content pane keeps when the file list is widened.
const MIN_CONTENT_WIDTH: u16 = 16;
/// The file list's width, within these bounds, as a share of the main
/// part of the page.
const MIN_FILES_WIDTH: u16 = 24;
const MAX_FILES_WIDTH: u16 = 56;
/// Rows scrolled per mouse wheel notch.
const WHEEL_LINES: usize = 3;
/// Columns scrolled per horizontal wheel notch, or by ← and →.
const WHEEL_COLUMNS: usize = 4;
/// Columns a pane may scroll past the longest visible line, as in the
/// editor.
const HSCROLL_SLACK: usize = 2;
/// Lines of context one press of an expand button reveals.
const EXPAND_LINES: usize = 10;
const TAB_WIDTH: usize = 4;
/// The graph takes at most this share of the log's width.
const MAX_GRAPH_SHARE: usize = 3;
/// The node of the commit HEAD is on.
const HEAD_NODE: &str = "◉";
const DESCRIPTION: &str = "Description";
const NOT_A_REPOSITORY: &str = "Not a git repository";
const NOT_INITIALIZED: &str = "Submodule not initialized";
const NO_COMMITS: &str = "No commits yet";
const NO_BRANCHES: &str = "none";
const SIDEBAR_HINT: &str = "↑↓ branch · Enter go to · ←→ fold remote · Tab pane · Ctrl+E leave";
const LOG_HINT: &str = "↑↓ commit · Enter files · ←→ sideways · Tab pane · Ctrl+E leave";
const FILES_HINT: &str = "↑↓ file · Enter view · ←→ fold/unfold · Tab pane · Ctrl+E leave";
const CONTENT_HINT: &str =
    "↑↓ scroll · ←→ sideways · ▲▼ buttons expand context · Tab pane · Ctrl+E leave";

/// Which pane has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Sidebar,
    Log,
    Files,
    Content,
}

impl Pane {
    fn next(self) -> Pane {
        match self {
            Pane::Sidebar => Pane::Log,
            Pane::Log => Pane::Files,
            Pane::Files => Pane::Content,
            Pane::Content => Pane::Sidebar,
        }
    }

    fn previous(self) -> Pane {
        match self {
            Pane::Sidebar => Pane::Content,
            Pane::Log => Pane::Sidebar,
            Pane::Files => Pane::Log,
            Pane::Content => Pane::Files,
        }
    }
}

/// One row of the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SideRow {
    BranchesHeading,
    Branch(usize),
    /// Under a heading with nothing beneath it.
    None,
    RemotesHeading,
    Remote(usize),
    RemoteBranch(usize, usize),
}

impl SideRow {
    fn selectable(self) -> bool {
        matches!(
            self,
            SideRow::Branch(_) | SideRow::Remote(_) | SideRow::RemoteBranch(..)
        )
    }
}

/// A button drawn in a diff's gap row, and what pressing it reveals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    /// Reveal lines above the change below the gap.
    Up(usize),
    /// Reveal lines below the change above the gap.
    Down(usize),
    /// Reveal the whole gap.
    All(usize),
}

/// A rule between panes that can be dragged to resize them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Divider {
    /// The sidebar's right edge.
    Sidebar,
    /// Between the log and the detail.
    Log,
    /// Between the file list and the content.
    Files,
}

/// What the right-hand pane below the log shows.
enum Content {
    Description,
    /// The files under a directory of the tree, by its index.
    Directory(usize),
    Diff(Box<FileDiff>),
    Failed(String),
}

/// A drawn piece of text: what and in which style.
type Piece = (String, Style);

/// The sideways scrolling of a pane, with the editor's rules (see
/// `EditorView`): the limit is set by the longest line *visible now*,
/// plus [`HSCROLL_SLACK`], not by the longest line there is; scrolling
/// right stops at the limit, but a position already past it (left
/// there by a vertical scroll) stays put until scrolled back; and a
/// scrollbar shows while anything is out of view. The caller measures
/// the visible lines, since it knows what they are; the pane's
/// `extent` is the columns from the left edge of what scrolls to the
/// end of the longest visible line, and its `capacity` the columns it
/// has to show them in.
#[derive(Default)]
struct HScroll {
    col: usize,
    /// The capacity as of the last render.
    capacity: usize,
    /// The reachable range as of the last render: the visible extent
    /// with its slack, or what is scrolled to, whichever is more.
    total: usize,
    /// Where the scrollbar was drawn, empty while hidden.
    bar: Rect,
    dragging: bool,
}

impl HScroll {
    /// The furthest right the pane can scroll for the lines visible now.
    fn max_col(&self, extent: usize) -> usize {
        (extent + HSCROLL_SLACK).saturating_sub(self.capacity)
    }

    /// Scroll sideways. Scrolling right stops at the limit, but a
    /// position already past it stays where it is.
    fn scroll_by(&mut self, columns: isize, extent: usize) {
        let target = self.col.saturating_add_signed(columns);
        self.col = if columns > 0 {
            target.min(self.max_col(extent).max(self.col))
        } else {
            target
        };
    }

    /// Scroll to where a click or drag at screen column `x` puts the
    /// scrollbar's thumb, with the same limit as [`scroll_by`].
    ///
    /// [`scroll_by`]: Self::scroll_by
    fn scroll_to(&mut self, x: u16, extent: usize) {
        let track = self.bar.width.max(1) as usize;
        let column = x.saturating_sub(self.bar.x) as usize;
        let max = self.total.saturating_sub(self.capacity);
        let target = (column * (max + 1) / track).min(max);
        if target > self.col {
            self.col = target.min(self.max_col(extent).max(self.col));
        } else {
            self.col = target;
        }
    }

    /// Whether the scrollbar is needed for this render: something is
    /// out of view to the right, or the pane is scrolled.
    fn needs_bar(&self, extent: usize, capacity: usize) -> bool {
        self.col > 0 || extent > capacity
    }

    /// Note the render's measurements and draw the scrollbar in `bar`,
    /// or none (an empty `bar`) when it isn't needed.
    fn render(
        &mut self,
        extent: usize,
        capacity: usize,
        bar: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        self.capacity = capacity;
        self.total = (extent + HSCROLL_SLACK).max(self.col + capacity);
        self.bar = bar;
        if bar.width == 0 || bar.height == 0 || capacity == 0 {
            self.bar = Rect::default();
            return;
        }
        let base = palette_background(theme);
        let mut state = ScrollbarState::new(self.total - capacity + 1)
            .position(self.col)
            .viewport_content_length(capacity);
        Scrollbar::new(ScrollbarOrientation::HorizontalBottom)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("─"))
            .thumb_symbol("█")
            .track_style(base.fg(theme.scroll_bar_track))
            .thumb_style(base.fg(theme.scroll_bar_color))
            .render(bar, buf, &mut state);
    }
}

/// One repository's tab of the git log: the main repository or a
/// submodule, with its page once it has been shown.
struct GitTab {
    title: String,
    workdir: PathBuf,
    /// Whether the tab is a submodule, opened as exactly that directory
    /// rather than whatever repository contains it.
    submodule: bool,
    view: Option<GitLogView>,
}

/// The git log page's tabs: the project's repository first, then each
/// of its submodules, nested ones included, by path. A submodule's page
/// is opened the first time its tab is shown, with the pane sizes kept
/// for it.
pub struct GitLogTabs {
    tabs: Vec<GitTab>,
    active: usize,
    /// The pane sizes of every repository, as loaded and as dragged.
    layout: GitLogLayout,
}

impl GitTab {
    /// The tab's key in the layout.
    fn key(&self) -> &str {
        if self.submodule {
            &self.title
        } else {
            MAIN_REPOSITORY
        }
    }
}

impl GitLogTabs {
    /// The tabs for the repository containing `root`, with the main
    /// repository's page open and sized as `layout` has it.
    pub fn new(root: &Path, layout: GitLogLayout) -> GitLogTabs {
        let mut main = GitLogView::new(root);
        main.set_sizes(layout.get(MAIN_REPOSITORY));
        let mut tabs = vec![GitTab {
            title: TITLE.to_owned(),
            workdir: root.to_path_buf(),
            submodule: false,
            view: Some(main),
        }];
        tabs.extend(submodules(root).into_iter().map(|submodule| GitTab {
            title: submodule.path,
            workdir: submodule.workdir,
            submodule: true,
            view: None,
        }));
        GitLogTabs {
            tabs,
            active: 0,
            layout,
        }
    }

    /// The pane sizes of every repository, to keep.
    pub fn layout(&self) -> &GitLogLayout {
        &self.layout
    }

    /// Give the shown page a mouse event. Returns whether the page's
    /// pane sizes changed (a drag of a rule ended), in which case the
    /// layout is worth keeping.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> bool {
        let active = self.active;
        let resized = self.active().handle_mouse(mouse);
        if resized {
            let sizes = self.tabs[active].view.as_ref().map(GitLogView::sizes);
            if let Some(sizes) = sizes {
                let key = self.tabs[active].key().to_owned();
                self.layout.set(&key, sizes);
            }
        }
        resized
    }

    /// The tabs' titles, in order: the page's name, then each
    /// submodule's path.
    pub fn titles(&self) -> impl Iterator<Item = &str> {
        self.tabs.iter().map(|tab| tab.title.as_str())
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    /// Show a tab, opening its page if this is its first showing.
    pub fn set_active(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
            self.active();
        }
    }

    /// The shown tab's page.
    pub fn active(&mut self) -> &mut GitLogView {
        let tab = &mut self.tabs[self.active];
        let sizes = self.layout.get(tab.key());
        tab.view.get_or_insert_with(|| {
            let mut view = if tab.submodule {
                GitLogView::for_repository(&tab.workdir)
            } else {
                GitLogView::new(&tab.workdir)
            };
            view.set_sizes(sizes);
            view
        })
    }

    /// The shown tab's page, if opened (it is, once shown).
    pub fn active_view(&self) -> Option<&GitLogView> {
        self.tabs[self.active].view.as_ref()
    }

    /// Take in commits the shown page's walk has produced. Returns
    /// whether the page needs redrawing.
    pub fn poll(&mut self) -> bool {
        self.active().poll()
    }

    /// What the status bar shows.
    pub fn hint(&self) -> String {
        self.active_view().map(GitLogView::hint).unwrap_or_default()
    }

    #[cfg(test)]
    pub fn is_loading(&self) -> bool {
        self.active_view().is_some_and(GitLogView::is_loading)
    }
}

pub struct GitLogView {
    history: Option<History>,
    /// Why there is no history: the project isn't in a repository.
    error: Option<String>,
    pane: Pane,
    side_rows: Vec<SideRow>,
    /// Which remotes are folded up; all of them to start with.
    collapsed: Vec<bool>,
    side_selected: usize,
    side_scroll: usize,
    reveal_side: bool,
    /// The selected commit, an index into the history's commits.
    selected: usize,
    /// How many commits are scrolled off the top of the log, and how
    /// many rows of it were shown at the last render.
    log_scroll: usize,
    log_rows: usize,
    log_h: HScroll,
    reveal_log: bool,
    /// Whether the log has been put at HEAD's commit yet, which waits
    /// for the walk to reach it (unless the user has moved first).
    head_shown: bool,
    /// A commit to select once the walk reaches it.
    pending_jump: Option<Oid>,
    /// The selected commit's detail, and which commit it is for.
    detail: Option<CommitDetail>,
    detail_of: Option<Oid>,
    /// The selected commit's files as a tree, which directories of it
    /// are folded, and the rows that leaves to show.
    file_tree: FileTree,
    dirs_collapsed: Vec<bool>,
    file_rows: Vec<TreeRow>,
    /// The selected row of the file list: 0 for the description, then
    /// `file_rows` from 1.
    file_selected: usize,
    files_scroll: usize,
    reveal_files: bool,
    /// What the content pane shows, for `file_selected`; built when
    /// first drawn.
    content: Option<Content>,
    content_scroll: usize,
    content_h: HScroll,
    /// How many rows the content had when last drawn, and how many of
    /// them were shown.
    content_rows: usize,
    content_shown: usize,
    /// Sizes set by dragging the rules between panes, or kept from
    /// last time, as shares of the space each divides; the layout
    /// picks defaults where these are unset, and clamps them to what
    /// leaves every pane its minimum.
    sidebar_share: Option<f32>,
    log_share: Option<f32>,
    files_share: Option<f32>,
    /// The rule being dragged, if any.
    divider_drag: Option<Divider>,
    /// The page, its panes, and the rules between them from the last
    /// render.
    area: Rect,
    sidebar_area: Rect,
    log_area: Rect,
    files_area: Rect,
    content_area: Rect,
    sidebar_rule: Rect,
    log_rule: Rect,
    files_rule: Rect,
    /// The expand buttons drawn in the content pane, to hit-test clicks.
    buttons: Vec<(Rect, Button)>,
}

impl GitLogView {
    /// A page for the repository containing `root`, with its walk
    /// started.
    pub fn new(root: &Path) -> GitLogView {
        GitLogView::from_history(History::open(root).map_err(|err| err.message().to_owned()))
    }

    /// A page for the repository whose working directory is `root`
    /// itself: a submodule's. One that isn't initialized says so.
    pub fn for_repository(root: &Path) -> GitLogView {
        GitLogView::from_history(
            History::open_repository(root).map_err(|_| NOT_INITIALIZED.to_owned()),
        )
    }

    fn from_history(history: Result<History, String>) -> GitLogView {
        let (history, error) = match history {
            Ok(history) => (Some(history), None),
            Err(message) => (None, Some(message)),
        };
        let mut view = GitLogView {
            history,
            error,
            pane: Pane::Log,
            side_rows: Vec::new(),
            collapsed: Vec::new(),
            side_selected: 0,
            side_scroll: 0,
            reveal_side: true,
            selected: 0,
            log_scroll: 0,
            log_rows: 0,
            log_h: HScroll::default(),
            reveal_log: true,
            head_shown: false,
            pending_jump: None,
            detail: None,
            detail_of: None,
            file_tree: FileTree::default(),
            dirs_collapsed: Vec::new(),
            file_rows: Vec::new(),
            file_selected: 0,
            files_scroll: 0,
            reveal_files: false,
            content: None,
            content_scroll: 0,
            content_h: HScroll::default(),
            content_rows: 0,
            content_shown: 0,
            sidebar_share: None,
            log_share: None,
            files_share: None,
            divider_drag: None,
            area: Rect::default(),
            sidebar_area: Rect::default(),
            log_area: Rect::default(),
            files_area: Rect::default(),
            content_area: Rect::default(),
            sidebar_rule: Rect::default(),
            log_rule: Rect::default(),
            files_rule: Rect::default(),
            buttons: Vec::new(),
        };
        if let Some(history) = &view.history {
            view.collapsed = vec![true; history.remotes().len()];
        }
        view.rebuild_side_rows();
        view.side_selected = view
            .side_rows
            .iter()
            .position(|row| row.selectable())
            .unwrap_or(0);
        view.poll();
        view
    }

    /// Take in commits the walk has produced. Returns whether the page
    /// needs redrawing.
    pub fn poll(&mut self) -> bool {
        let Some(history) = &mut self.history else {
            return false;
        };
        if !history.poll() {
            return false;
        }
        // The log opens at HEAD: select it as soon as it is walked,
        // unless the user has already moved.
        let head = history.head();
        let head_position = head.and_then(|head| history.position(head));
        let jump_position = self
            .pending_jump
            .and_then(|target| history.position(target));
        if !self.head_shown {
            match (head, head_position) {
                (Some(_), Some(position)) => {
                    self.head_shown = true;
                    self.select_commit(position);
                }
                (None, _) => self.head_shown = true,
                _ => {}
            }
        }
        if let Some(position) = jump_position {
            self.pending_jump = None;
            self.select_commit(position);
        }
        true
    }

    /// What the status bar shows for the page.
    pub fn hint(&self) -> String {
        let pane = match self.pane {
            Pane::Sidebar => SIDEBAR_HINT,
            Pane::Log => LOG_HINT,
            Pane::Files => FILES_HINT,
            Pane::Content => CONTENT_HINT,
        };
        match &self.history {
            Some(history) if history.is_loading() => {
                format!("loading {} commits… · {pane}", history.commits().len())
            }
            _ => pane.to_owned(),
        }
    }

    /// The selected commit's id, if any.
    #[cfg(test)]
    pub fn selected_commit(&self) -> Option<Oid> {
        self.history
            .as_ref()
            .and_then(|h| h.commits().get(self.selected))
            .map(|c| c.id)
    }

    #[cfg(test)]
    pub fn is_loading(&self) -> bool {
        self.history.as_ref().is_some_and(History::is_loading)
    }

    /// Where the rule under the log was drawn.
    #[cfg(test)]
    pub fn log_rule(&self) -> Rect {
        self.log_rule
    }

    // ----- Selection ------------------------------------------------------

    fn commit_count(&self) -> usize {
        self.history.as_ref().map_or(0, |h| h.commits().len())
    }

    /// Select a commit of the log and show its detail.
    fn select_commit(&mut self, index: usize) {
        let count = self.commit_count();
        if count == 0 {
            return;
        }
        let index = index.min(count - 1);
        if index != self.selected {
            self.selected = index;
            self.file_selected = 0;
            self.files_scroll = 0;
            self.content = None;
            self.content_scroll = 0;
            self.content_h.col = 0;
        }
        self.reveal_log = true;
    }

    /// How many rows the file list has: the description and the tree.
    fn file_row_count(&self) -> usize {
        self.file_rows.len() + 1
    }

    /// The tree row selected in the file list, if not the description.
    fn selected_file_row(&self) -> Option<TreeRow> {
        self.file_selected
            .checked_sub(1)
            .and_then(|index| self.file_rows.get(index).copied())
    }

    /// Select a row of the file list, changing what the content pane
    /// shows.
    fn select_file(&mut self, index: usize) {
        let index = index.min(self.file_row_count() - 1);
        if index != self.file_selected {
            self.file_selected = index;
            self.content = None;
            self.content_scroll = 0;
            self.content_h.col = 0;
        }
        self.reveal_files = true;
    }

    fn rebuild_file_rows(&mut self) {
        self.file_rows = self.file_tree.rows(&self.dirs_collapsed);
    }

    /// Select the row of a directory of the tree.
    fn select_dir(&mut self, dir: usize) {
        if let Some(index) = self
            .file_rows
            .iter()
            .position(|row| matches!(row, TreeRow::Dir { dir: d, .. } if *d == dir))
        {
            self.select_file(index + 1);
        }
    }

    /// Fold or unfold a directory of the tree. A selected file folded
    /// out of sight leaves the selection on the directory.
    fn set_dir_collapsed(&mut self, dir: usize, collapsed: bool) {
        if self.dirs_collapsed.get(dir).copied() != Some(!collapsed) {
            return;
        }
        let selected = self.selected_file_row();
        self.dirs_collapsed[dir] = collapsed;
        self.rebuild_file_rows();
        let kept = match selected {
            Some(TreeRow::Dir { dir: d, .. }) => self
                .file_rows
                .iter()
                .position(|row| matches!(row, TreeRow::Dir { dir, .. } if *dir == d)),
            Some(TreeRow::File { file: f, .. }) => self
                .file_rows
                .iter()
                .position(|row| matches!(row, TreeRow::File { file, .. } if *file == f)),
            None => return,
        };
        match kept {
            Some(index) => {
                self.file_selected = index + 1;
                self.reveal_files = true;
            }
            None => self.select_dir(dir),
        }
    }

    /// Select a commit by id, now if it has been walked and otherwise
    /// when it is.
    fn jump_to(&mut self, id: Oid) {
        match self.history.as_ref().and_then(|h| h.position(id)) {
            Some(position) => {
                self.head_shown = true;
                self.select_commit(position);
            }
            None => self.pending_jump = Some(id),
        }
    }

    /// Move the sidebar's selection to the next selectable row in a
    /// direction, staying put at the ends.
    fn move_side(&mut self, forward: bool) {
        let mut index = self.side_selected;
        loop {
            let next = if forward {
                index + 1
            } else {
                match index.checked_sub(1) {
                    Some(next) => next,
                    None => return,
                }
            };
            let Some(row) = self.side_rows.get(next) else {
                return;
            };
            index = next;
            if row.selectable() {
                self.side_selected = index;
                self.reveal_side = true;
                return;
            }
        }
    }

    /// Fold or unfold a remote, keeping the selection on its row.
    fn set_collapsed(&mut self, remote: usize, collapsed: bool) {
        if self.collapsed.get(remote) == Some(&collapsed) {
            return;
        }
        self.collapsed[remote] = collapsed;
        self.rebuild_side_rows();
        if let Some(index) = self
            .side_rows
            .iter()
            .position(|row| *row == SideRow::Remote(remote))
        {
            self.side_selected = index;
        }
        self.reveal_side = true;
    }

    /// Enter or a click on a sidebar row: go to a branch's commit, or
    /// fold or unfold a remote.
    fn activate_side_row(&mut self, index: usize) {
        let Some(row) = self.side_rows.get(index).copied() else {
            return;
        };
        let Some(history) = &self.history else {
            return;
        };
        match row {
            SideRow::Branch(b) => {
                if let Some(branch) = history.branches().get(b) {
                    let target = branch.target;
                    self.jump_to(target);
                }
            }
            SideRow::RemoteBranch(r, b) => {
                if let Some(branch) = history
                    .remotes()
                    .get(r)
                    .and_then(|remote| remote.branches.get(b))
                {
                    let target = branch.target;
                    self.jump_to(target);
                }
            }
            SideRow::Remote(r) => {
                let collapsed = self.collapsed.get(r).copied().unwrap_or(true);
                self.set_collapsed(r, !collapsed);
            }
            _ => {}
        }
    }

    fn rebuild_side_rows(&mut self) {
        self.side_rows.clear();
        let Some(history) = &self.history else {
            return;
        };
        self.side_rows.push(SideRow::BranchesHeading);
        if history.branches().is_empty() {
            self.side_rows.push(SideRow::None);
        }
        for b in 0..history.branches().len() {
            self.side_rows.push(SideRow::Branch(b));
        }
        self.side_rows.push(SideRow::RemotesHeading);
        if history.remotes().is_empty() {
            self.side_rows.push(SideRow::None);
        }
        for (r, remote) in history.remotes().iter().enumerate() {
            self.side_rows.push(SideRow::Remote(r));
            if !self.collapsed.get(r).copied().unwrap_or(true) {
                for b in 0..remote.branches.len() {
                    self.side_rows.push(SideRow::RemoteBranch(r, b));
                }
            }
        }
    }

    // ----- The detail ------------------------------------------------------

    /// Read the selected commit's detail, if it isn't the one held.
    fn ensure_detail(&mut self) {
        let Some(history) = &self.history else {
            return;
        };
        let Some(commit) = history.commits().get(self.selected) else {
            self.detail = None;
            self.detail_of = None;
            return;
        };
        if self.detail_of == Some(commit.id) {
            return;
        }
        self.detail_of = Some(commit.id);
        self.detail = history.detail(commit.id).ok();
        self.file_tree = match &self.detail {
            Some(detail) => FileTree::new(detail.files.iter().map(|f| f.path.as_str())),
            None => FileTree::default(),
        };
        self.dirs_collapsed = vec![false; self.file_tree.dirs().len()];
        self.rebuild_file_rows();
        self.file_selected = 0;
        self.files_scroll = 0;
        self.content = None;
        self.content_scroll = 0;
        self.content_h.col = 0;
    }

    /// Build what the content pane shows for the selected file, if it
    /// isn't built.
    fn ensure_content(&mut self) {
        if self.content.is_some() {
            return;
        }
        let (Some(history), Some(detail)) = (&self.history, &self.detail) else {
            return;
        };
        self.content = Some(match self.selected_file_row() {
            None => Content::Description,
            Some(TreeRow::Dir { dir, .. }) => Content::Directory(dir),
            Some(TreeRow::File { file, .. }) => match detail.files.get(file) {
                Some(file) => {
                    match history.file_diff(detail.id, &file.path, file.old_path.as_deref()) {
                        Ok(diff) => Content::Diff(Box::new(diff)),
                        Err(err) => Content::Failed(err.message().to_owned()),
                    }
                }
                None => Content::Description,
            },
        });
    }

    /// The content pane's lines for a directory of the tree: what is
    /// under it, with the counts.
    fn directory_lines(&self, dir: usize) -> Vec<Piece> {
        let (Some(detail), Some(entry)) = (&self.detail, self.file_tree.dirs().get(dir)) else {
            return Vec::new();
        };
        let plain = Style::default();
        let files = self.file_tree.files_under(dir);
        let (additions, deletions) = files
            .iter()
            .filter_map(|f| detail.files.get(*f))
            .fold((0, 0), |(a, d), f| (a + f.additions, d + f.deletions));
        let mut lines: Vec<Piece> = vec![
            (
                format!("{}/", entry.path),
                plain.add_modifier(Modifier::BOLD),
            ),
            (
                format!(
                    "{} {}, +{additions} −{deletions}",
                    files.len(),
                    if files.len() == 1 { "file" } else { "files" }
                ),
                plain,
            ),
            (String::new(), plain),
        ];
        let prefix = format!("{}/", entry.path);
        for file in files.iter().filter_map(|f| detail.files.get(*f)) {
            let path = file.path.strip_prefix(&prefix).unwrap_or(&file.path);
            let counts = if file.binary {
                "binary".to_owned()
            } else {
                format!("+{} −{}", file.additions, file.deletions)
            };
            lines.push((format!("{} {path}  {counts}", file.kind.letter()), plain));
        }
        lines
    }

    /// The description pane's lines for the selected commit.
    fn description_lines(&self) -> Vec<Piece> {
        let Some(detail) = &self.detail else {
            return Vec::new();
        };
        let plain = Style::default();
        let mut lines: Vec<Piece> = Vec::new();
        lines.push((format!("commit {}", detail.id), plain));
        let parents = if detail.parents.is_empty() {
            "none (root commit)".to_owned()
        } else {
            detail
                .parents
                .iter()
                .map(|id| ninjaedit_core::git::short_id(*id))
                .collect::<Vec<_>>()
                .join(" ")
        };
        lines.push((format!("Parents:   {parents}"), plain));
        lines.push((
            format!(
                "Author:    {} <{}>",
                detail.author.name, detail.author.email
            ),
            plain,
        ));
        lines.push((date_line("Date:      ", detail.author.time), plain));
        if detail.committer.name != detail.author.name
            || detail.committer.email != detail.author.email
        {
            lines.push((
                format!(
                    "Committer: {} <{}>",
                    detail.committer.name, detail.committer.email
                ),
                plain,
            ));
        }
        if detail.committer.time != detail.author.time {
            lines.push((date_line("Committed: ", detail.committer.time), plain));
        }
        if let Some(commit) = self
            .history
            .as_ref()
            .and_then(|h| h.commits().get(self.selected))
            && !commit.refs.is_empty()
        {
            let refs: Vec<&str> = commit.refs.iter().map(|r| r.name.as_str()).collect();
            lines.push((format!("Refs:      {}", refs.join(", ")), plain));
        }
        lines.push((String::new(), plain));
        for line in detail.message.lines() {
            lines.push((line.to_owned(), plain));
        }
        lines
    }

    // ----- Sideways extents ------------------------------------------------

    /// How many lanes of the graph the log draws, at most.
    fn lane_cap(&self) -> usize {
        (self.log_area.width as usize / MAX_GRAPH_SHARE)
            .div_ceil(2)
            .max(1)
    }

    /// The columns the log's visible commits reach, from the first
    /// column after its left edge to the end of the longest of their
    /// lines: `rows` rows from `scroll`, two a commit.
    fn log_extent(&self, scroll: usize, rows: usize) -> usize {
        let Some(history) = &self.history else {
            return 0;
        };
        let cap = self.lane_cap();
        let head = history.head();
        history
            .commits()
            .iter()
            .skip(scroll)
            .take(rows / 2)
            .map(|commit| {
                let lanes = commit.graph.width().clamp(1, cap);
                let (first, second) = commit_lines(commit, head == Some(commit.id), None);
                cells_for(lanes) + 1 + pieces_width(&first).max(pieces_width(&second))
            })
            .max()
            .unwrap_or(0)
    }

    /// The columns the content pane's visible lines reach: `rows` rows
    /// from `scroll`.
    fn content_extent(&self, scroll: usize, rows: usize) -> usize {
        match &self.content {
            Some(Content::Description) => text_extent(&self.description_lines(), scroll, rows),
            Some(Content::Directory(dir)) => text_extent(&self.directory_lines(*dir), scroll, rows),
            Some(Content::Diff(diff)) => diff_extent(diff, &diff.rows(), scroll, rows),
            _ => 0,
        }
    }

    /// Scroll the log sideways by `columns`, within the editor's limits.
    fn scroll_log_sideways(&mut self, columns: isize) {
        let extent = self.log_extent(self.log_scroll, self.log_rows);
        self.log_h.scroll_by(columns, extent);
    }

    /// Scroll the content pane sideways by `columns`.
    fn scroll_content_sideways(&mut self, columns: isize) {
        let extent = self.content_extent(self.content_scroll, self.content_shown);
        self.content_h.scroll_by(columns, extent);
    }

    // ----- Input ----------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        // The file list follows the selected commit's detail, which is
        // read on the first draw after the selection moves; a key that
        // comes first (from a test, or a burst of input) needs it too.
        self.ensure_detail();
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Tab if !shift => {
                self.pane = self.pane.next();
                return;
            }
            KeyCode::BackTab => {
                self.pane = self.pane.previous();
                return;
            }
            KeyCode::Tab => {
                self.pane = self.pane.previous();
                return;
            }
            _ => {}
        }
        match self.pane {
            Pane::Sidebar => self.handle_sidebar_key(key),
            Pane::Log => self.handle_log_key(key),
            Pane::Files => self.handle_files_key(key),
            Pane::Content => self.handle_content_key(key),
        }
    }

    fn handle_sidebar_key(&mut self, key: KeyEvent) {
        let selected = self.side_rows.get(self.side_selected).copied();
        match key.code {
            KeyCode::Up => self.move_side(false),
            KeyCode::Down => self.move_side(true),
            KeyCode::Enter => {
                self.activate_side_row(self.side_selected);
                if matches!(
                    selected,
                    Some(SideRow::Branch(_) | SideRow::RemoteBranch(..))
                ) {
                    self.pane = Pane::Log;
                }
            }
            KeyCode::Char(' ') => self.activate_side_row(self.side_selected),
            KeyCode::Left => match selected {
                Some(SideRow::Remote(r)) => self.set_collapsed(r, true),
                Some(SideRow::RemoteBranch(r, _)) => self.set_collapsed(r, true),
                _ => {}
            },
            KeyCode::Right => match selected {
                Some(SideRow::Remote(r)) => self.set_collapsed(r, false),
                Some(SideRow::Branch(_) | SideRow::RemoteBranch(..)) => self.pane = Pane::Log,
                _ => {}
            },
            _ => {}
        }
    }

    fn handle_log_key(&mut self, key: KeyEvent) {
        let page = (self.log_area.height as usize / 2).max(1);
        let moved = match key.code {
            KeyCode::Up => Some(self.selected.saturating_sub(1)),
            KeyCode::Down => Some(self.selected + 1),
            KeyCode::PageUp => Some(self.selected.saturating_sub(page)),
            KeyCode::PageDown => Some(self.selected + page),
            KeyCode::Home => Some(0),
            KeyCode::End => Some(usize::MAX),
            KeyCode::Enter => {
                self.pane = Pane::Files;
                None
            }
            KeyCode::Right => {
                self.scroll_log_sideways(WHEEL_COLUMNS as isize);
                None
            }
            KeyCode::Left if self.log_h.col > 0 => {
                self.scroll_log_sideways(-(WHEEL_COLUMNS as isize));
                None
            }
            KeyCode::Left => {
                if self.sidebar_area.width > 0 {
                    self.pane = Pane::Sidebar;
                }
                None
            }
            _ => None,
        };
        if let Some(index) = moved {
            self.head_shown = true;
            self.pending_jump = None;
            self.select_commit(index);
        }
    }

    fn handle_files_key(&mut self, key: KeyEvent) {
        let page = (self.files_area.height as usize).saturating_sub(1).max(1);
        let row = self.selected_file_row();
        match key.code {
            KeyCode::Up => self.select_file(self.file_selected.saturating_sub(1)),
            KeyCode::Down => self.select_file(self.file_selected + 1),
            KeyCode::PageUp => self.select_file(self.file_selected.saturating_sub(page)),
            KeyCode::PageDown => self.select_file(self.file_selected + page),
            KeyCode::Home => self.select_file(0),
            KeyCode::End => self.select_file(usize::MAX),
            KeyCode::Enter | KeyCode::Char(' ') => match row {
                Some(TreeRow::Dir { dir, collapsed, .. }) => {
                    self.set_dir_collapsed(dir, !collapsed);
                }
                _ if key.code == KeyCode::Enter => self.pane = Pane::Content,
                _ => {}
            },
            KeyCode::Right => match row {
                Some(TreeRow::Dir {
                    dir,
                    collapsed: true,
                    ..
                }) => self.set_dir_collapsed(dir, false),
                // An open directory: on to its first entry.
                Some(TreeRow::Dir { .. }) => self.select_file(self.file_selected + 1),
                _ => self.pane = Pane::Content,
            },
            KeyCode::Left => match row {
                Some(TreeRow::Dir {
                    dir,
                    collapsed: false,
                    ..
                }) => self.set_dir_collapsed(dir, true),
                Some(TreeRow::Dir { dir, .. }) => match self.file_tree.dirs()[dir].parent {
                    Some(parent) => self.select_dir(parent),
                    None => self.pane = Pane::Log,
                },
                Some(TreeRow::File {
                    parent: Some(parent),
                    ..
                }) => self.select_dir(parent),
                _ => self.pane = Pane::Log,
            },
            _ => {}
        }
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
            KeyCode::Left => self.pane = Pane::Files,
            _ => {}
        }
    }

    /// Whether the screen position is over the page.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.area.contains(ScreenPosition::new(x, y))
    }

    /// Whether a scrollbar or a rule between panes is being dragged,
    /// in which case the page wants drag and release events wherever
    /// they happen.
    pub fn is_dragging(&self) -> bool {
        self.log_h.dragging || self.content_h.dragging || self.divider_drag.is_some()
    }

    // ----- Resizing the panes -----------------------------------------------

    /// The sidebar's width for the page's width: what was dragged to,
    /// or a share of the page, within what leaves the rest its minimum.
    fn sidebar_width_for(&self, width: u16) -> u16 {
        if width < MIN_WIDTH_FOR_SIDEBAR {
            return 0;
        }
        let wanted = match self.sidebar_share {
            Some(share) => share_of(width, share),
            None => (width / 5).clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH),
        };
        let most = width.saturating_sub(MIN_MAIN_WIDTH + 1);
        clamp_between(wanted, MIN_SIDEBAR_WIDTH, most)
    }

    /// The pane sizes as shares, to keep for next time.
    pub fn sizes(&self) -> PaneSizes {
        PaneSizes {
            sidebar: self.sidebar_share,
            log: self.log_share,
            files: self.files_share,
        }
    }

    /// Size the panes as kept from last time.
    pub fn set_sizes(&mut self, sizes: PaneSizes) {
        self.sidebar_share = sizes.sidebar;
        self.log_share = sizes.log;
        self.files_share = sizes.files;
    }

    /// The log's height for the page's height: its share of it, within
    /// what leaves the detail its minimum; a short page is all log.
    fn log_height_for(&self, height: u16) -> u16 {
        if height < MIN_LOG_HEIGHT + 1 + MIN_DETAIL_HEIGHT + 1 {
            return height;
        }
        let wanted = share_of(height, self.log_share.unwrap_or(0.5));
        clamp_between(
            wanted,
            MIN_LOG_HEIGHT,
            height.saturating_sub(MIN_DETAIL_HEIGHT + 1),
        )
    }

    /// The file list's width for the main part's width.
    fn files_width_for(&self, main_width: u16) -> u16 {
        let wanted = match self.files_share {
            Some(share) => share_of(main_width, share),
            None => (main_width / 3)
                .clamp(MIN_FILES_WIDTH, MAX_FILES_WIDTH)
                .min(main_width / 2),
        };
        let most = main_width.saturating_sub(MIN_CONTENT_WIDTH + 1);
        clamp_between(wanted, MIN_FILES_WIDTH.min(main_width / 2), most)
    }

    /// Move a rule to where the mouse is, as far as the panes allow,
    /// and keep the share that settles on.
    fn drag_divider(&mut self, divider: Divider, x: u16, y: u16) {
        match divider {
            Divider::Sidebar => {
                let width = self.area.width.max(1);
                self.sidebar_share = Some(share_for(x.saturating_sub(self.area.x), width));
                let settled = self.sidebar_width_for(width);
                self.sidebar_share = Some(share_for(settled, width));
            }
            Divider::Log => {
                let height = self.area.height.max(1);
                self.log_share = Some(share_for(y.saturating_sub(self.area.y), height));
                let settled = self.log_height_for(height);
                self.log_share = Some(share_for(settled, height));
            }
            Divider::Files => {
                let main_width = self.area.right().saturating_sub(self.files_area.x).max(1);
                self.files_share = Some(share_for(x.saturating_sub(self.files_area.x), main_width));
                let settled = self.files_width_for(main_width);
                self.files_share = Some(share_for(settled, main_width));
            }
        }
    }

    /// Handle a mouse event. Returns whether the pane sizes changed:
    /// a drag of a rule ended.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> bool {
        self.ensure_detail();
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
            let divider = if self.sidebar_rule.contains(at) {
                Some(Divider::Sidebar)
            } else if self.log_rule.contains(at) {
                Some(Divider::Log)
            } else if self.files_rule.contains(at) {
                Some(Divider::Files)
            } else {
                None
            };
            if let Some(divider) = divider {
                self.divider_drag = Some(divider);
                self.drag_divider(divider, mouse.column, mouse.row);
                return false;
            }
        }
        // A scrollbar drag owns the mouse likewise.
        if self.is_dragging() {
            match mouse.kind {
                MouseEventKind::Drag(_) => {
                    if self.log_h.dragging {
                        let extent = self.log_extent(self.log_scroll, self.log_rows);
                        self.log_h.scroll_to(mouse.column, extent);
                    } else {
                        let extent = self.content_extent(self.content_scroll, self.content_shown);
                        self.content_h.scroll_to(mouse.column, extent);
                    }
                }
                MouseEventKind::Up(_) => {
                    self.log_h.dragging = false;
                    self.content_h.dragging = false;
                }
                _ => {}
            }
            return false;
        }
        let pane = if self.sidebar_area.contains(at) {
            Some(Pane::Sidebar)
        } else if self.log_area.contains(at) {
            Some(Pane::Log)
        } else if self.files_area.contains(at) {
            Some(Pane::Files)
        } else if self.content_area.contains(at) {
            Some(Pane::Content)
        } else {
            None
        };
        let Some(pane) = pane else {
            return false;
        };
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_pane(pane, false),
            MouseEventKind::ScrollDown => self.scroll_pane(pane, true),
            MouseEventKind::ScrollRight | MouseEventKind::ScrollLeft => {
                let columns = if mouse.kind == MouseEventKind::ScrollRight {
                    WHEEL_COLUMNS as isize
                } else {
                    -(WHEEL_COLUMNS as isize)
                };
                match pane {
                    Pane::Log => self.scroll_log_sideways(columns),
                    Pane::Content => self.scroll_content_sideways(columns),
                    _ => {}
                }
            }
            MouseEventKind::Down(MouseButton::Left) if self.log_h.bar.contains(at) => {
                self.pane = Pane::Log;
                self.log_h.dragging = true;
                let extent = self.log_extent(self.log_scroll, self.log_rows);
                self.log_h.scroll_to(mouse.column, extent);
            }
            MouseEventKind::Down(MouseButton::Left) if self.content_h.bar.contains(at) => {
                self.pane = Pane::Content;
                self.content_h.dragging = true;
                let extent = self.content_extent(self.content_scroll, self.content_shown);
                self.content_h.scroll_to(mouse.column, extent);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.pane = pane;
                match pane {
                    Pane::Sidebar => {
                        let row = self.side_scroll + (mouse.row - self.sidebar_area.y) as usize;
                        if self.side_rows.get(row).is_some_and(|r| r.selectable()) {
                            self.side_selected = row;
                            self.activate_side_row(row);
                        }
                    }
                    Pane::Log => {
                        let index = self.log_scroll + (mouse.row - self.log_area.y) as usize / 2;
                        if index < self.commit_count() {
                            self.head_shown = true;
                            self.pending_jump = None;
                            self.select_commit(index);
                        }
                    }
                    Pane::Files => {
                        let row = self.files_scroll + (mouse.row - self.files_area.y) as usize;
                        if row < self.file_row_count() {
                            self.select_file(row);
                            // A directory folds or unfolds when clicked.
                            if let Some(TreeRow::Dir { dir, collapsed, .. }) =
                                self.selected_file_row()
                            {
                                self.set_dir_collapsed(dir, !collapsed);
                            }
                        }
                    }
                    Pane::Content => {
                        if let Some((_, button)) =
                            self.buttons.iter().find(|(area, _)| area.contains(at))
                        {
                            self.press(*button);
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
            Pane::Sidebar => self.side_scroll = step(self.side_scroll),
            Pane::Log => {
                // Two rows a commit: a notch moves fewer commits.
                let commits = WHEEL_LINES.div_ceil(2);
                self.log_scroll = if down {
                    self.log_scroll + commits
                } else {
                    self.log_scroll.saturating_sub(commits)
                };
            }
            Pane::Files => self.files_scroll = step(self.files_scroll),
            Pane::Content => self.content_scroll = step(self.content_scroll),
        }
    }

    /// Press one of a diff's expand buttons.
    fn press(&mut self, button: Button) {
        let Some(Content::Diff(diff)) = &mut self.content else {
            return;
        };
        match button {
            Button::Up(gap) => diff.expand_up(gap, EXPAND_LINES),
            Button::Down(gap) => diff.expand_down(gap, EXPAND_LINES),
            Button::All(gap) => diff.expand_all(gap),
        }
    }

    // ----- Rendering ------------------------------------------------------

    /// Draw the page into `area`. The page never wants the terminal
    /// cursor.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        self.area = area;
        let background = palette_background(theme);
        buf.set_style(area, background);
        self.buttons.clear();
        if area.height == 0 || area.width < 8 {
            self.sidebar_area = Rect::default();
            self.log_area = Rect::default();
            self.files_area = Rect::default();
            self.content_area = Rect::default();
            return;
        }
        if self.history.is_none() {
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
            return;
        }
        self.ensure_detail();
        self.ensure_content();

        // The sidebar on the left, then the log over the detail, which
        // is the file list beside the content; rules between them.
        let rule = background.fg(theme.gutter_guide);
        let sidebar_width = self.sidebar_width_for(area.width);
        let main_x = area.x + sidebar_width + u16::from(sidebar_width > 0);
        let main_width = area.right() - main_x;
        self.sidebar_area = Rect::new(area.x, area.y, sidebar_width, area.height);
        self.sidebar_rule = Rect::default();
        if sidebar_width > 0 {
            let x = area.x + sidebar_width;
            self.sidebar_rule = Rect::new(x, area.y, 1, area.height);
            for y in area.y..area.bottom() {
                buf[(x, y)].set_symbol("│").set_style(rule);
            }
        }
        let log_height = self.log_height_for(area.height);
        self.log_area = Rect::new(main_x, area.y, main_width, log_height);
        let detail_y = area.y + log_height + 1;
        let detail_height = area.bottom().saturating_sub(detail_y);
        self.log_rule = Rect::default();
        self.files_rule = Rect::default();
        if detail_height > 0 {
            let y = area.y + log_height;
            self.log_rule = Rect::new(main_x, y, main_width, 1);
            for x in main_x..area.right() {
                buf[(x, y)].set_symbol("─").set_style(rule);
            }
            if sidebar_width > 0 {
                buf[(main_x - 1, y)].set_symbol("├").set_style(rule);
            }
            let files_width = self.files_width_for(main_width);
            self.files_area = Rect::new(main_x, detail_y, files_width, detail_height);
            self.files_rule = Rect::new(main_x + files_width, detail_y, 1, detail_height);
            let content_x = main_x + files_width + 1;
            self.content_area = Rect::new(
                content_x,
                detail_y,
                area.right().saturating_sub(content_x),
                detail_height,
            );
            buf[(main_x + files_width, y)]
                .set_symbol("┬")
                .set_style(rule);
            for y in detail_y..area.bottom() {
                buf[(main_x + files_width, y)]
                    .set_symbol("│")
                    .set_style(rule);
            }
        } else {
            self.files_area = Rect::default();
            self.content_area = Rect::default();
        }

        if sidebar_width > 0 {
            self.render_sidebar(buf, theme);
        }
        self.render_log(buf, theme);
        if detail_height > 0 {
            self.render_files(buf, theme);
            self.render_content(buf, theme);
        }
    }

    /// The background of a list's selected row: brighter when the pane
    /// has the keyboard than when it doesn't, so the selection says
    /// where the keyboard is. Only the background marks the selection:
    /// the text keeps its colors, selected or not.
    fn selection_background(&self, pane: Pane, theme: &Theme) -> Color {
        if self.pane == pane {
            theme.list_selection_background
        } else {
            theme.unfocused_list_selection_background
        }
    }

    fn render_sidebar(&mut self, buf: &mut Buffer, theme: &Theme) {
        let area = self.sidebar_area;
        let height = area.height as usize;
        let width = area.width as usize;
        let Some(history) = &self.history else {
            return;
        };
        self.side_scroll = self
            .side_scroll
            .min(self.side_rows.len().saturating_sub(height));
        if self.reveal_side {
            self.reveal_side = false;
            if self.side_selected < self.side_scroll {
                self.side_scroll = self.side_selected;
            } else if self.side_selected >= self.side_scroll + height {
                self.side_scroll = self.side_selected + 1 - height;
            }
        }
        let background = palette_background(theme);
        let heading = background
            .fg(theme.active_tab_text)
            .add_modifier(Modifier::BOLD);
        let dim = background.fg(theme.command_palette_result_context_text);
        let head = background
            .fg(theme.git_head_text)
            .add_modifier(Modifier::BOLD);
        let selected_background = self.selection_background(Pane::Sidebar, theme);
        for (row, entry) in self
            .side_rows
            .iter()
            .enumerate()
            .skip(self.side_scroll)
            .take(height)
        {
            let y = area.y + (row - self.side_scroll) as u16;
            let is_selected = row == self.side_selected && entry.selectable();
            let (indent, label, style) = match *entry {
                SideRow::BranchesHeading => (1, "Branches".to_owned(), heading),
                SideRow::RemotesHeading => (1, "Remotes".to_owned(), heading),
                SideRow::None => (3, NO_BRANCHES.to_owned(), dim),
                SideRow::Branch(b) => {
                    let branch = &history.branches()[b];
                    let style = if branch.is_head { head } else { background };
                    (3, branch.name.clone(), style)
                }
                SideRow::Remote(r) => {
                    let arrow = if self.collapsed.get(r).copied().unwrap_or(true) {
                        "▸"
                    } else {
                        "▾"
                    };
                    (
                        2,
                        format!("{arrow} {}", history.remotes()[r].name),
                        background,
                    )
                }
                SideRow::RemoteBranch(r, b) => {
                    (5, history.remotes()[r].branches[b].name.clone(), background)
                }
            };
            let style = if is_selected {
                let style = style.bg(selected_background);
                buf.set_style(Rect::new(area.x, y, area.width, 1), style);
                style
            } else {
                style
            };
            buf.set_stringn(
                area.x + indent,
                y,
                &label,
                width.saturating_sub(indent as usize),
                style,
            );
        }
    }

    fn render_log(&mut self, buf: &mut Buffer, theme: &Theme) {
        let area = self.log_area;
        let Some(history) = &self.history else {
            return;
        };
        let background = palette_background(theme);
        let dim = background.fg(theme.command_palette_result_context_text);
        let commits = history.commits();
        if commits.is_empty() {
            let message = if history.is_loading() {
                "loading…"
            } else {
                NO_COMMITS
            };
            buf.set_stringn(area.x + 1, area.y, message, area.width as usize, dim);
            return;
        }
        // Two rows a commit; an odd last row stays blank. The sideways
        // scrollbar, when needed, takes the last row, which can change
        // which commits are visible and so whether it is needed: lay
        // out again with the row taken, and settle there.
        let full_height = area.height as usize;
        let capacity = area.width as usize - 1;
        let mut show_bar = false;
        let mut visible;
        let mut extent;
        loop {
            let rows = full_height - usize::from(show_bar);
            visible = (rows / 2).max(1);
            self.log_scroll = self.log_scroll.min(commits.len().saturating_sub(visible));
            if self.reveal_log {
                if self.selected < self.log_scroll {
                    self.log_scroll = self.selected;
                } else if self.selected >= self.log_scroll + visible {
                    self.log_scroll = self.selected + 1 - visible;
                }
            }
            extent = self.log_extent(self.log_scroll, visible * 2);
            let needed = self.log_h.needs_bar(extent, capacity) && full_height > 2;
            if needed && !show_bar {
                show_bar = true;
                continue;
            }
            break;
        }
        self.reveal_log = false;
        self.log_rows = visible * 2;
        let scroll_col = self.log_h.col;
        let bar = if show_bar {
            Rect::new(area.x + 1, area.bottom() - 1, area.width - 1, 1)
        } else {
            Rect::default()
        };
        self.log_h.render(extent, capacity, bar, buf, theme);
        let Some(history) = &self.history else {
            return;
        };
        let commits = history.commits();
        // Each row's text starts right after its own part of the graph,
        // rather than after the widest row in the history: aligning
        // them all would push every message right for the sake of a
        // few wide rows. The graph gets at most a share of the width.
        let max_lanes = self.lane_cap();
        let head = history.head();
        let selected_style = background.bg(self.selection_background(Pane::Log, theme));
        let lane_colors = lane_palette(theme);

        for (index, commit) in commits
            .iter()
            .enumerate()
            .skip(self.log_scroll)
            .take(visible)
        {
            let row = index - self.log_scroll;
            let y0 = area.y + (row * 2) as u16;
            let y1 = y0 + 1;
            if y1 >= area.bottom() {
                break;
            }
            let is_head = head == Some(commit.id);
            let is_selected = index == self.selected;
            let row_style = if is_selected {
                buf.set_style(Rect::new(area.x, y0, area.width, 2), selected_style);
                selected_style
            } else {
                background
            };
            // The graph, in the lanes' colors over the row's background.
            let lanes = commit.graph.width().clamp(1, max_lanes);
            let text_x = area.x + 1 + cells_for(lanes) as u16 + 1;
            let text_width = area.right().saturating_sub(text_x) as usize;
            let node_cells = commit.graph.node_cells(lanes);
            let transition_cells = commit.graph.transition_cells(lanes);
            for (k, cell) in node_cells.iter().enumerate() {
                let x = area.x + 1 + k as u16;
                let mut style = row_style;
                if let Some(color) = cell.color {
                    style = style.fg(lane_colors[color % lane_colors.len()]);
                }
                let glyph = if cell.node && is_head {
                    style = style.add_modifier(Modifier::BOLD);
                    HEAD_NODE
                } else if cell.node {
                    NODE
                } else {
                    cell.glyph
                };
                buf[(x, y0)].set_symbol(glyph).set_style(style);
            }
            for (k, cell) in transition_cells.iter().enumerate() {
                let x = area.x + 1 + k as u16;
                let mut style = row_style;
                if let Some(color) = cell.color {
                    style = style.fg(lane_colors[color % lane_colors.len()]);
                }
                buf[(x, y1)].set_symbol(cell.glyph).set_style(style);
            }
            if text_width == 0 {
                continue;
            }

            // Line one: the references, then the message. Line two: the
            // author, the id, and the time. Both scroll sideways
            // together, the graph staying put.
            let (first, second) = commit_lines(commit, is_head, Some((theme, row_style)));
            draw_pieces(buf, text_x, y0, text_width, &first, scroll_col);
            draw_pieces(buf, text_x, y1, text_width, &second, scroll_col);
        }
    }

    fn render_files(&mut self, buf: &mut Buffer, theme: &Theme) {
        let area = self.files_area;
        let height = area.height as usize;
        let width = area.width as usize;
        let background = palette_background(theme);
        let Some(detail) = &self.detail else {
            if self.commit_count() > 0 {
                let dim = background.fg(theme.command_palette_result_context_text);
                buf.set_stringn(area.x + 1, area.y, "…", width, dim);
            }
            return;
        };
        let count = self.file_row_count();
        self.files_scroll = self.files_scroll.min(count.saturating_sub(height));
        if self.reveal_files {
            self.reveal_files = false;
            if self.file_selected < self.files_scroll {
                self.files_scroll = self.file_selected;
            } else if self.file_selected >= self.files_scroll + height {
                self.files_scroll = self.file_selected + 1 - height;
            }
        }
        let selected_style = background.bg(self.selection_background(Pane::Files, theme));
        let added = theme.diff_added_text;
        let removed = theme.diff_removed_text;
        for row in self.files_scroll..(self.files_scroll + height).min(count) {
            let y = area.y + (row - self.files_scroll) as u16;
            let is_selected = row == self.file_selected;
            let row_style = if is_selected {
                buf.set_style(Rect::new(area.x, y, area.width, 1), selected_style);
                selected_style
            } else {
                background
            };
            let Some(entry) = row.checked_sub(1).and_then(|r| self.file_rows.get(r)) else {
                buf.set_stringn(
                    area.x + 1,
                    y,
                    DESCRIPTION,
                    width.saturating_sub(1),
                    row_style.add_modifier(Modifier::BOLD),
                );
                continue;
            };
            let indent = 1 + entry.depth() * 2;
            let x = area.x + indent as u16;
            let file = match *entry {
                TreeRow::Dir { dir, collapsed, .. } => {
                    let arrow = if collapsed { "▸" } else { "▾" };
                    let label = format!("{arrow} {}", self.file_tree.dirs()[dir].label);
                    buf.set_stringn(
                        x,
                        y,
                        &label,
                        width.saturating_sub(indent),
                        row_style.fg(theme.active_tab_text),
                    );
                    continue;
                }
                TreeRow::File { file, .. } => file,
            };
            let Some(file) = detail.files.get(file) else {
                continue;
            };
            // The kind's letter, the file's name (its end kept when
            // cut), and the counts at the right edge.
            let counts = if file.binary {
                " bin".to_owned()
            } else if file.additions + file.deletions > 0 {
                format!(" +{} −{}", file.additions, file.deletions)
            } else {
                String::new()
            };
            let counts_width = display_width(&counts);
            let letter_color = match file.kind {
                ChangeKind::Added | ChangeKind::Copied => theme.diff_added_text,
                ChangeKind::Deleted => theme.diff_removed_text,
                _ => theme.git_tag_text,
            };
            buf.set_stringn(
                x,
                y,
                file.kind.letter().to_string(),
                1,
                row_style.fg(letter_color),
            );
            let name = file.path.rsplit('/').next().unwrap_or(&file.path);
            let name = match &file.old_path {
                Some(old) => format!("{name} (from {old})"),
                None => name.to_owned(),
            };
            let name_x = x + 2;
            let name_width = width.saturating_sub(indent + 2 + counts_width);
            buf.set_stringn(name_x, y, fit_end(&name, name_width), name_width, row_style);
            if counts_width > 0 && counts_width <= width {
                let x = area.right() - counts_width as u16;
                if file.binary {
                    buf.set_string(x, y, &counts, row_style);
                } else {
                    let (plus, minus) = counts.split_once(" −").expect("both counts");
                    let (x, _) = buf.set_stringn(x, y, plus, counts_width, row_style.fg(added));
                    buf.set_stringn(
                        x,
                        y,
                        format!(" −{minus}"),
                        counts_width,
                        row_style.fg(removed),
                    );
                }
            }
        }
    }

    fn render_content(&mut self, buf: &mut Buffer, theme: &Theme) {
        let area = self.content_area;
        if area.width < 4 || area.height == 0 {
            return;
        }
        let background = palette_background(theme).bg(theme
            .search_preview_background
            .unwrap_or(theme.command_palette_background));
        buf.set_style(area, background);
        let dim = background.fg(theme.command_palette_result_context_text);
        let height = area.height as usize;
        let content = match &self.content {
            Some(content) => content,
            None => return,
        };
        let text = match content {
            Content::Description => Some(self.description_lines()),
            Content::Directory(dir) => Some(self.directory_lines(*dir)),
            _ => None,
        };
        match content {
            Content::Description | Content::Directory(_) => {
                let lines = text.unwrap_or_default();
                self.content_rows = lines.len();
                let capacity = area.width as usize - 1;
                // The scrollbar takes the last row; see `render_log`.
                let mut show_bar = false;
                let mut shown;
                let mut extent;
                loop {
                    shown = height - usize::from(show_bar);
                    self.content_scroll =
                        self.content_scroll.min(lines.len().saturating_sub(shown));
                    extent = text_extent(&lines, self.content_scroll, shown);
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
            Content::Failed(message) => {
                buf.set_stringn(area.x + 1, area.y, message, area.width as usize - 1, dim);
                self.content_h
                    .render(0, area.width as usize, Rect::default(), buf, theme);
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

/// A description line for a time: in the user's zone, and in the zone
/// it was recorded in when that reads differently.
fn date_line(label: &str, time: ninjaedit_core::git::CommitTime) -> String {
    let local = time.to_string();
    let own = time.in_own_zone();
    if own == local {
        format!("{label}{local}")
    } else {
        format!("{label}{local}  ({own} where committed)")
    }
}

/// The colors the graph's lanes cycle through.
fn lane_palette(theme: &Theme) -> [Color; 8] {
    [
        theme.terminal_blue,
        theme.terminal_green,
        theme.terminal_yellow,
        theme.terminal_magenta,
        theme.terminal_cyan,
        theme.terminal_red,
        theme.terminal_bright_blue,
        theme.terminal_bright_magenta,
    ]
}

/// A commit's two lines of text: the references and the message, then
/// the author, the id, and the time. Styled over `row_style` with the
/// theme's colors when given (HEAD's row bold and in its color),
/// otherwise plain, for measuring.
fn commit_lines(
    commit: &ninjaedit_core::git::Commit,
    is_head: bool,
    styles: Option<(&Theme, Style)>,
) -> (Vec<Piece>, Vec<Piece>) {
    let row_style = styles.map_or_else(Style::default, |(_, style)| style);
    let colored = |pick: fn(&Theme) -> Color| match styles {
        Some((theme, style)) => style.fg(pick(theme)),
        None => row_style,
    };
    let mut first: Vec<Piece> = Vec::new();
    for label in &commit.refs {
        if label.is_head {
            first.push(("HEAD → ".to_owned(), colored(|t| t.git_head_text)));
        }
        let color: fn(&Theme) -> Color = match label.kind {
            RefKind::Branch => |t| t.git_branch_text,
            RefKind::RemoteBranch => |t| t.git_remote_text,
            RefKind::Tag => |t| t.git_tag_text,
        };
        let name = if label.kind == RefKind::Tag {
            format!("tag: {}", label.name)
        } else {
            label.name.clone()
        };
        first.push((
            format!("{name} "),
            colored(color).add_modifier(Modifier::BOLD),
        ));
    }
    let summary_style = if is_head {
        colored(|t| t.git_head_text).add_modifier(Modifier::BOLD)
    } else {
        row_style
    };
    if !first.is_empty() {
        first.push((" ".to_owned(), row_style));
    }
    first.push((commit.summary.clone(), summary_style));
    let second = vec![
        (commit.author.clone(), colored(|t| t.git_author_text)),
        ("  ".to_owned(), row_style),
        (commit.short_id(), colored(|t| t.git_hash_text)),
        ("  ".to_owned(), row_style),
        (commit.time.to_string(), colored(|t| t.git_date_text)),
    ];
    (first, second)
}

/// The display width of a line of pieces.
fn pieces_width(pieces: &[Piece]) -> usize {
    pieces.iter().map(|(text, _)| display_width(text)).sum()
}

/// The widest of `rows` lines of text from `scroll`.
fn text_extent(lines: &[Piece], scroll: usize, rows: usize) -> usize {
    lines
        .iter()
        .skip(scroll)
        .take(rows)
        .map(|(text, _)| display_width(text))
        .max()
        .unwrap_or(0)
}

/// The widest of `rows` rows of a diff from `scroll`; the gap rows
/// don't scroll and don't count.
fn diff_extent(diff: &FileDiff, rows: &[DiffRow], scroll: usize, count: usize) -> usize {
    rows.iter()
        .skip(scroll)
        .take(count)
        .filter_map(|row| match row {
            DiffRow::Line(line) => Some(display_width(diff.text(line))),
            DiffRow::Gap { .. } => None,
        })
        .max()
        .unwrap_or(0)
}

/// What a render of a diff settled on.
struct DiffDrawn {
    /// How many rows the diff has.
    rows: usize,
    /// How many of them were shown.
    shown: usize,
    /// The vertical scroll position.
    scroll: usize,
}

/// Draw a diff into `area`, scrolled by `scroll` rows and sideways by
/// `h`, recording the expand buttons drawn.
fn render_diff(
    diff: &FileDiff,
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    scroll: usize,
    h: &mut HScroll,
    buttons: &mut Vec<(Rect, Button)>,
) -> DiffDrawn {
    let background = palette_background(theme).bg(theme
        .search_preview_background
        .unwrap_or(theme.command_palette_background));
    let dim = background.fg(theme.command_palette_result_context_text);
    let height = area.height as usize;
    if let Some(unshown) = diff.unshown {
        let message = match unshown {
            Unshown::Binary => "Binary file",
            Unshown::TooLarge => "File too large to show",
        };
        buf.set_stringn(area.x + 1, area.y, message, area.width as usize - 1, dim);
        h.render(0, area.width as usize, Rect::default(), buf, theme);
        return DiffDrawn {
            rows: 1,
            shown: 1,
            scroll: 0,
        };
    }
    let rows = diff.rows();
    if rows.is_empty() {
        let message = match diff.kind {
            ChangeKind::Renamed => "Renamed without changes",
            ChangeKind::Copied => "Copied without changes",
            _ => "No changes to the contents",
        };
        buf.set_stringn(area.x + 1, area.y, message, area.width as usize - 1, dim);
        h.render(0, area.width as usize, Rect::default(), buf, theme);
        return DiffDrawn {
            rows: 1,
            shown: 1,
            scroll: 0,
        };
    }
    // The gutter: the old and new line numbers, then the marker.
    let old_digits = digits(diff.old_line_count());
    let new_digits = digits(diff.new_line_count());
    let gutter = old_digits + 1 + new_digits + 1;
    let marker_x = area.x + gutter as u16;
    let text_x = marker_x + 2;
    if text_x >= area.right() {
        h.render(0, 0, Rect::default(), buf, theme);
        return DiffDrawn {
            rows: rows.len(),
            shown: height,
            scroll: scroll.min(rows.len().saturating_sub(height)),
        };
    }
    let text_width = area.right() - text_x;
    // The sideways scrollbar takes the last row; see `render_log`.
    let capacity = text_width as usize;
    let mut show_bar = false;
    let mut shown;
    let mut scroll = scroll;
    let mut extent;
    loop {
        shown = height - usize::from(show_bar);
        scroll = scroll.min(rows.len().saturating_sub(shown));
        extent = diff_extent(diff, &rows, scroll, shown);
        let needed = h.needs_bar(extent, capacity) && height > 1;
        if needed && !show_bar {
            show_bar = true;
            continue;
        }
        break;
    }
    let col = h.col;
    let added_bg = background.bg(theme.diff_added_background);
    let removed_bg = background.bg(theme.diff_removed_background);
    let number = |base: Style| base.fg(theme.inactive_line_number);
    let button_style = background
        .fg(theme.active_tab_text)
        .bg(theme.command_palette_background);

    for (row, entry) in rows.iter().skip(scroll).take(shown).enumerate() {
        let y = area.y + row as u16;
        match entry {
            DiffRow::Line(line) => {
                let (row_style, marker, marker_style) = match line.kind {
                    LineKind::Context => (background, " ", background),
                    LineKind::Added => (added_bg, "+", added_bg.fg(theme.diff_added_text)),
                    LineKind::Removed => (removed_bg, "-", removed_bg.fg(theme.diff_removed_text)),
                };
                buf.set_style(Rect::new(area.x, y, area.width, 1), row_style);
                let old = line.old.map_or(String::new(), |n| (n + 1).to_string());
                let new = line.new.map_or(String::new(), |n| (n + 1).to_string());
                buf.set_string(
                    area.x,
                    y,
                    format!("{old:>old_digits$} {new:>new_digits$}"),
                    number(row_style),
                );
                buf.set_string(
                    marker_x,
                    y,
                    marker,
                    marker_style.add_modifier(Modifier::BOLD),
                );
                let text = diff.text(line);
                let tokens = diff.tokens(line);
                let cells = layout_cells(text, tokens);
                let text_area = Rect::new(text_x, y, text_width, 1);
                draw_cells(buf, text_area, &cells, col, |kind| {
                    if kind == TokenKind::Text {
                        row_style
                    } else {
                        theme.syntax(kind).apply(row_style)
                    }
                });
            }
            DiffRow::Gap {
                gap,
                hidden,
                up,
                down,
            } => {
                let mut x = area.x + 1;
                let mut place = |text: String, style: Style, button: Option<Button>| {
                    let width = display_width(&text) as u16;
                    if x + width > area.right() {
                        return;
                    }
                    buf.set_string(x, y, &text, style);
                    if let Some(button) = button {
                        buttons.push((Rect::new(x, y, width, 1), button));
                    }
                    x += width + 1;
                };
                let step = (*hidden).min(EXPAND_LINES);
                if *up {
                    place(format!(" ▲ {step} "), button_style, Some(Button::Up(*gap)));
                }
                if *down {
                    place(
                        format!(" ▼ {step} "),
                        button_style,
                        Some(Button::Down(*gap)),
                    );
                }
                if *hidden > EXPAND_LINES {
                    place(" all ".to_owned(), button_style, Some(Button::All(*gap)));
                }
                let noun = if *hidden == 1 { "line" } else { "lines" };
                place(format!("⋯ {hidden} {noun} hidden"), dim, None);
            }
        }
    }
    let bar = if show_bar {
        Rect::new(text_x, area.bottom() - 1, text_width, 1)
    } else {
        Rect::default()
    };
    h.render(extent, capacity, bar, buf, theme);
    DiffDrawn {
        rows: rows.len(),
        shown,
        scroll,
    }
}

/// Draw pieces of text one after another on a row, cut at `width`,
/// starting `start` display columns into them.
fn draw_pieces(buf: &mut Buffer, x: u16, y: u16, width: usize, pieces: &[Piece], start: usize) {
    let mut x = x;
    let end = x + width as u16;
    let mut skipped = 0;
    for (text, style) in pieces {
        if x >= end {
            break;
        }
        let piece_width = display_width(text);
        let text = if skipped + piece_width <= start {
            skipped += piece_width;
            continue;
        } else if skipped < start {
            let rest = skip_columns(text, start - skipped);
            skipped = start;
            rest
        } else {
            text.as_str()
        };
        let (next, _) = buf.set_stringn(x, y, text, (end - x) as usize, *style);
        x = next;
    }
}

/// `text` without its first `columns` display columns; a wide
/// character straddling the cut goes with the part cut off.
fn skip_columns(text: &str, columns: usize) -> &str {
    let mut column = 0;
    for g in text::graphemes(text.as_bytes()) {
        if column >= columns {
            return &text[g.range.start..];
        }
        column += text::width(g.text, column, TAB_WIDTH);
    }
    ""
}

/// One character of a line laid out for display.
struct Cell<'a> {
    text: &'a str,
    column: usize,
    width: usize,
    kind: TokenKind,
}

/// Lay out a line's characters, expanding tabs, each classified by the
/// token containing its first byte.
fn layout_cells<'a>(text: &'a str, tokens: &[Token]) -> Vec<Cell<'a>> {
    let mut column = 0;
    let mut next_token = 0;
    text::graphemes(text.as_bytes())
        .map(|g| {
            let width = text::width(g.text, column, TAB_WIDTH);
            let start = g.range.start;
            while next_token < tokens.len() && tokens[next_token].range.end <= start {
                next_token += 1;
            }
            let kind = match tokens.get(next_token) {
                Some(token) if token.range.start <= start => token.kind,
                _ => TokenKind::Text,
            };
            let cell = Cell {
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

/// Draw a line's cells into a one-row `area` from display column
/// `start`, each in the style `style_of` gives its kind.
fn draw_cells(
    buf: &mut Buffer,
    area: Rect,
    cells: &[Cell<'_>],
    start: usize,
    style_of: impl Fn(TokenKind) -> Style,
) {
    let width = area.width as usize;
    if width == 0 {
        return;
    }
    let visible = start..start + width;
    for cell in cells {
        if cell.width == 0 || cell.column + cell.width <= visible.start {
            continue;
        }
        if cell.column >= visible.end {
            break;
        }
        let style = style_of(cell.kind);
        let fits = cell.column >= visible.start && cell.column + cell.width <= visible.end;
        let from = cell.column.max(visible.start);
        let to = (cell.column + cell.width).min(visible.end);
        let x = area.x + (from - start) as u16;
        if cell.text == "\t" || !fits {
            for x in x..x + (to - from) as u16 {
                buf[(x, area.y)].set_symbol(" ").set_style(style);
            }
        } else {
            buf.set_string(x, area.y, cell.text, style);
        }
    }
}

/// The display width of a piece of text.
fn display_width(text: &str) -> usize {
    text::graphemes(text.as_bytes())
        .map(|g| text::width(g.text, 0, TAB_WIDTH))
        .sum()
}

/// `text` if it fits in `width` columns, else its end with an ellipsis
/// before it: for a path, the file name is the part that matters.
fn fit_end(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_owned();
    }
    let room = width.saturating_sub(1);
    let graphemes: Vec<_> = text::graphemes(text.as_bytes()).collect();
    let mut kept = 0;
    let mut cut = text.len();
    for g in graphemes.iter().rev() {
        let w = text::width(g.text, 0, TAB_WIDTH);
        if kept + w > room {
            break;
        }
        kept += w;
        cut = g.range.start;
    }
    format!("…{}", &text[cut..])
}

/// `value` within `low..=high`, or `high` when the bounds cross (a pane
/// too small for both minimums gets what there is).
fn clamp_between(value: u16, low: u16, high: u16) -> u16 {
    value.min(high).max(low.min(high))
}

/// The cells a share of `total` comes to.
fn share_of(total: u16, share: f32) -> u16 {
    (f32::from(total) * share).round() as u16
}

/// The share of `total` that `cells` are.
fn share_for(cells: u16, total: u16) -> f32 {
    f32::from(cells) / f32::from(total.max(1))
}

/// The number of decimal digits needed to show `n`.
fn digits(n: usize) -> usize {
    n.max(1).ilog10() as usize + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};
    use git2::{Repository, Signature};
    use std::fs;
    use std::time::Duration;

    /// A repository with a merge in it: `main` has Base and On main,
    /// `side` has On side, and `main` has merged `side`; plus a tag on
    /// Base and a remote branch on the merge.
    fn repo_with_history() -> (tempfile::TempDir, Vec<Oid>) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let mut clock = 1_700_000_000;
        let mut commit = |files: &[(&str, &str)], message: &str, parents: &[Oid]| {
            for (name, content) in files {
                fs::write(dir.path().join(name), content).unwrap();
            }
            let mut index = repo.index().unwrap();
            for (name, _) in files {
                index.add_path(Path::new(name)).unwrap();
            }
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            clock += 60;
            let sig = Signature::new("Ann Author", "ann@example.com", &git2::Time::new(clock, 0))
                .unwrap();
            let parents: Vec<git2::Commit<'_>> = parents
                .iter()
                .map(|id| repo.find_commit(*id).unwrap())
                .collect();
            let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
            repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &refs)
                .unwrap()
        };
        let a = commit(
            &[("a.rs", "fn main() {\n    one();\n}\n")],
            "Base commit",
            &[],
        );
        let b = commit(
            &[("a.rs", "fn main() {\n    one();\n    two();\n}\n")],
            "On main",
            &[a],
        );
        let main = repo.head().unwrap().shorthand().unwrap().to_owned();
        repo.branch("side", &repo.find_commit(a).unwrap(), false)
            .unwrap();
        repo.set_head("refs/heads/side").unwrap();
        let s = commit(&[("s.txt", "side\n")], "On side", &[a]);
        repo.set_head(&format!("refs/heads/{main}")).unwrap();
        let m = commit(&[("s.txt", "side\n")], "Merge side into main", &[b, s]);
        repo.tag_lightweight("v1.0", &repo.find_object(a, None).unwrap(), false)
            .unwrap();
        repo.remote("origin", "https://example.com/r.git").unwrap();
        repo.reference(&format!("refs/remotes/origin/{main}"), m, false, "t")
            .unwrap();
        (dir, vec![a, b, s, m])
    }

    fn view(dir: &tempfile::TempDir) -> GitLogView {
        let mut view = GitLogView::new(dir.path());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while view.is_loading() && std::time::Instant::now() < deadline {
            view.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!view.is_loading());
        view
    }

    fn draw(view: &mut GitLogView, width: u16, height: u16) -> Vec<String> {
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

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn click(view: &mut GitLogView, column: u16, row: u16) {
        view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }

    /// The screen column `needle` starts at in `row`, which may hold
    /// characters wider than a byte.
    fn column_of(row: &str, needle: &str) -> u16 {
        let at = row
            .find(needle)
            .unwrap_or_else(|| panic!("no {needle:?} in {row:?}"));
        row[..at].chars().count() as u16
    }

    fn row_with<'a>(screen: &'a [String], text: &str) -> &'a str {
        screen
            .iter()
            .find(|row| row.contains(text))
            .unwrap_or_else(|| panic!("no row with {text:?} in {screen:#?}"))
    }

    #[test]
    fn the_log_opens_at_head_with_the_graph_refs_and_details() {
        let (dir, ids) = repo_with_history();
        let mut view = view(&dir);
        let [a, _b, _s, m] = ids[..] else { panic!() };
        assert_eq!(view.selected_commit(), Some(m));
        let screen = draw(&mut view, 110, 24);
        // The sidebar: branches, and the remote folded.
        assert!(screen[0].starts_with(" Branches"), "{screen:#?}");
        assert!(!row_with(&screen, "   side").is_empty());
        let origin = screen.iter().position(|r| r.contains("▸ origin")).unwrap();
        assert!(
            screen[origin + 1][..20].trim().is_empty(),
            "folded: {screen:#?}"
        );
        // The log: the merge first, with HEAD's node, its labels, and
        // two lines each.
        let merge = row_with(&screen, "Merge side into main");
        assert!(merge.contains(HEAD_NODE), "{merge}");
        assert!(merge.contains("HEAD → "), "{merge}");
        assert!(merge.contains("origin/"), "{merge}");
        let merge_index = screen
            .iter()
            .position(|r| r.contains("Merge side"))
            .unwrap();
        let second = &screen[merge_index + 1];
        assert!(second.contains("Ann Author"), "{second}");
        assert!(second.contains(&m.to_string()[..8]), "{second}");
        // The time is shown in the user's zone: the date is the 14th
        // or, east of UTC+1, the 15th.
        assert!(second.contains("2023-11-1"), "{second}");
        assert!(
            second.contains('├') && second.contains('╮'),
            "the fork: {second}"
        );
        let base = row_with(&screen, "Base commit");
        assert!(base.contains("tag: v1.0"), "{base}");
        // Text follows each row's own graph: the base commit's, one lane
        // wide, starts two columns left of the two-lane rows'.
        let side = row_with(&screen, "On side");
        assert_eq!(
            column_of(base, "tag:"),
            column_of(side, "side  On side") - 2
        );
        assert_eq!(column_of(merge, "HEAD"), column_of(side, "side  On side"));
        // The detail: the description and the file list.
        assert!(
            screen.iter().any(|r| r.contains("Description")),
            "{screen:#?}"
        );
        assert!(screen.iter().any(|r| r.contains("A s.txt")), "{screen:#?}");
        assert!(
            screen.iter().any(|r| r.contains(&format!("commit {m}"))),
            "{screen:#?}"
        );
        assert!(row_with(&screen, "Parents:").contains(&ids[1].to_string()[..8]));
        assert!(row_with(&screen, "Author:").contains("ann@example.com"));
        // The selected rows keep their colors over the selection
        // backgrounds: the focused log's brighter than the sidebar's.
        let theme = Theme::default();
        let area = Rect::new(0, 0, 110, 24);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf, &theme);
        let head_cell = &buf[(column_of(merge, "HEAD"), merge_index as u16)];
        assert_eq!(head_cell.fg, theme.git_head_text);
        assert_eq!(head_cell.bg, theme.list_selection_background);
        let branch = screen
            .iter()
            .position(|r| r.contains("   master") || r.contains("   main"))
            .unwrap();
        let branch_cell = &buf[(3, branch as u16)];
        assert_eq!(branch_cell.fg, theme.git_head_text);
        assert_eq!(branch_cell.bg, theme.unfocused_list_selection_background);
        assert_ne!(
            theme.list_selection_background,
            theme.unfocused_list_selection_background
        );
        // Base is the root.
        view.handle_key(key(KeyCode::End));
        assert_eq!(view.selected_commit(), Some(a));
        let screen = draw(&mut view, 110, 24);
        assert!(row_with(&screen, "Parents:").contains("root"));
        assert!(screen.iter().any(|r| r.contains("A a.rs")), "{screen:#?}");
    }

    #[test]
    fn a_file_diff_shows_highlighted_lines_and_expands_context() {
        let (dir, ids) = repo_with_history();
        let mut view = view(&dir);
        let b = ids[1];
        view.jump_to(b);
        // Down in the files pane picks a.rs; Enter moves to the diff.
        view.handle_key(key(KeyCode::Enter));
        view.handle_key(key(KeyCode::Down));
        let screen = draw(&mut view, 100, 24);
        assert!(row_with(&screen, "M a.rs").contains("+1 −0"), "{screen:#?}");
        let plus = row_with(&screen, "+     two();");
        assert!(plus.contains(" 3 "), "the new line number: {plus}");
        assert!(
            screen.iter().any(|r| r.contains("fn main() {")),
            "{screen:#?}"
        );
        // The whole file is within three lines, so nothing is hidden.
        assert!(!screen.iter().any(|r| r.contains("hidden")), "{screen:#?}");
        let area = Rect::new(0, 0, 100, 24);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf, &Theme::default());
        let theme = Theme::default();
        let y = screen
            .iter()
            .position(|r| r.contains("+     two();"))
            .unwrap() as u16;
        let x = column_of(&screen[y as usize], "two");
        assert_eq!(buf[(x, y)].bg, theme.diff_added_background);
        assert_eq!(buf[(x, y)].fg, theme.syntax(TokenKind::Function).color);
    }

    #[test]
    fn gap_buttons_reveal_hidden_lines() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let sig = Signature::now("T", "t@example.com").unwrap();
        let commit = |content: &str, parents: &[Oid]| {
            fs::write(dir.path().join("f.txt"), content).unwrap();
            let mut index = repo.index().unwrap();
            index.add_path(Path::new("f.txt")).unwrap();
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let parents: Vec<git2::Commit<'_>> = parents
                .iter()
                .map(|id| repo.find_commit(*id).unwrap())
                .collect();
            let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
            repo.commit(Some("HEAD"), &sig, &sig, "c", &tree, &refs)
                .unwrap()
        };
        let old: String = (1..=40).map(|n| format!("line {n}\n")).collect();
        let a = commit(&old, &[]);
        let new = old.replace("line 20\n", "line twenty\n");
        let _b = commit(&new, &[a]);
        let mut view = view(&dir);
        view.handle_key(key(KeyCode::Enter));
        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Enter));
        let screen = draw(&mut view, 90, 30);
        let gap = screen
            .iter()
            .position(|r| r.contains("16 lines hidden"))
            .unwrap_or_else(|| panic!("{screen:#?}"));
        assert!(screen[gap].contains("▲ 10"), "{screen:#?}");
        assert!(
            !screen[gap].contains("▼"),
            "nothing above the first gap: {screen:#?}"
        );
        assert!(screen[gap].contains("all"), "{screen:#?}");
        let last = screen
            .iter()
            .position(|r| r.contains("17 lines hidden"))
            .unwrap();
        assert!(
            screen[last].contains("▼ 10") && !screen[last].contains("▲"),
            "{screen:#?}"
        );
        // Press ▲ on the first gap: ten lines above the hunk appear.
        let x = column_of(&screen[gap], "▲");
        click(&mut view, x, gap as u16);
        let screen = draw(&mut view, 90, 30);
        assert!(
            screen.iter().any(|r| r.contains("6 lines hidden")),
            "{screen:#?}"
        );
        assert!(screen.iter().any(|r| r.contains(" line 7")), "{screen:#?}");
        // "all" on the last gap (End scrolls down to it) reveals the
        // rest of the file.
        view.handle_key(key(KeyCode::End));
        let screen = draw(&mut view, 90, 30);
        let last = screen
            .iter()
            .position(|r| r.contains("17 lines hidden"))
            .unwrap_or_else(|| panic!("{screen:#?}"));
        let x = column_of(&screen[last], "all");
        click(&mut view, x, last as u16);
        view.handle_key(key(KeyCode::End));
        let screen = draw(&mut view, 90, 30);
        assert!(screen.iter().any(|r| r.contains("line 40")), "{screen:#?}");
        assert!(
            !screen.iter().any(|r| r.contains("17 lines hidden")),
            "{screen:#?}"
        );
    }

    #[test]
    fn the_sidebar_folds_remotes_and_goes_to_branches() {
        let (dir, ids) = repo_with_history();
        let mut view = view(&dir);
        let [_a, _b, s, m] = ids[..] else { panic!() };
        draw(&mut view, 110, 24);
        view.handle_key(key(KeyCode::BackTab));
        assert_eq!(view.pane, Pane::Sidebar);
        // Down to `side`, Enter goes to its commit and to the log.
        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Enter));
        assert_eq!(view.selected_commit(), Some(s));
        assert_eq!(view.pane, Pane::Log);
        // Back in the sidebar, the remote unfolds with → and its branch
        // goes to the merge.
        view.handle_key(key(KeyCode::Left));
        view.handle_key(key(KeyCode::Down));
        assert_eq!(view.side_rows[view.side_selected], SideRow::Remote(0));
        view.handle_key(key(KeyCode::Right));
        let screen = draw(&mut view, 110, 24);
        assert!(screen.iter().any(|r| r.contains("▾ origin")), "{screen:#?}");
        view.handle_key(key(KeyCode::Down));
        assert!(matches!(
            view.side_rows[view.side_selected],
            SideRow::RemoteBranch(0, 0)
        ));
        view.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(view.selected_commit(), Some(m));
        assert_eq!(view.pane, Pane::Sidebar);
        view.handle_key(key(KeyCode::Left));
        let screen = draw(&mut view, 110, 24);
        assert!(screen.iter().any(|r| r.contains("▸ origin")), "{screen:#?}");
        assert_eq!(view.side_rows[view.side_selected], SideRow::Remote(0));
        // Clicking a commit in the log selects it and focuses the log.
        let row = screen.iter().position(|r| r.contains("On side")).unwrap() as u16;
        click(&mut view, 60, row);
        assert_eq!(view.selected_commit(), Some(s));
        assert_eq!(view.pane, Pane::Log);
    }

    #[test]
    fn files_are_listed_as_a_tree_that_folds() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let sig = Signature::now("T", "t@example.com").unwrap();
        let files = [
            ("Cargo.toml", "[package]\n"),
            ("core/src/git/diff.rs", "fn d() {}\n"),
            ("core/src/git/mod.rs", "mod diff;\n"),
            ("core/src/lib.rs", "mod git;\n"),
            ("tui/src/app.rs", "fn a() {}\n"),
        ];
        let mut index = repo.index().unwrap();
        for (name, content) in files {
            let path = dir.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
            index.add_path(Path::new(name)).unwrap();
        }
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "Lay out the crates", &tree, &[])
            .unwrap();
        let mut view = view(&dir);
        let screen = draw(&mut view, 100, 30);
        // The file pane's columns, between its two rules.
        let listed: Vec<String> = screen
            .iter()
            .map(|row| row.chars().skip(21).take(26).collect::<String>())
            .map(|row| row.trim_end().to_owned())
            .skip_while(|row| !row.contains("Description"))
            .take_while(|row| !row.is_empty())
            .collect();
        assert_eq!(
            listed,
            [
                " Description",
                " ▾ core/src",
                "   ▾ git",
                "     A diff.rs       +1 −0",
                "     A mod.rs        +1 −0",
                "   A lib.rs          +1 −0",
                " ▾ tui/src",
                "   A app.rs          +1 −0",
                " A Cargo.toml        +1 −0",
            ],
            "{screen:#?}"
        );

        // Into the files: Down to core/src, ← folds it and keeps the
        // selection on it; → unfolds it again.
        view.handle_key(key(KeyCode::Enter));
        view.handle_key(key(KeyCode::Down));
        assert!(matches!(
            view.selected_file_row(),
            Some(TreeRow::Dir { dir: 0, .. })
        ));
        view.handle_key(key(KeyCode::Left));
        let screen = draw(&mut view, 100, 30);
        assert!(
            screen.iter().any(|r| r.contains("▸ core/src")),
            "{screen:#?}"
        );
        let file_pane = |screen: &[String]| -> Vec<String> {
            screen
                .iter()
                .map(|row| row.chars().skip(21).take(26).collect())
                .collect()
        };
        assert!(
            !file_pane(&screen).iter().any(|r| r.contains("diff.rs")),
            "{screen:#?}"
        );
        assert!(matches!(
            view.selected_file_row(),
            Some(TreeRow::Dir {
                dir: 0,
                collapsed: true,
                ..
            })
        ));
        // A folded directory's content pane sums up what is under it.
        assert!(
            screen.iter().any(|r| r.contains("3 files, +3 −0")),
            "{screen:#?}"
        );
        assert!(
            screen.iter().any(|r| r.contains("A git/diff.rs  +1 −0")),
            "{screen:#?}"
        );
        view.handle_key(key(KeyCode::Right));
        assert!(matches!(
            view.selected_file_row(),
            Some(TreeRow::Dir {
                dir: 0,
                collapsed: false,
                ..
            })
        ));
        // Down twice to diff.rs; folding git from there (a click on its
        // row) leaves the selection on git, and ← from an open file
        // goes to its directory.
        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Down));
        assert!(matches!(
            view.selected_file_row(),
            Some(TreeRow::File { file: 1, .. })
        ));
        view.handle_key(key(KeyCode::Left));
        assert!(matches!(
            view.selected_file_row(),
            Some(TreeRow::Dir { dir: 1, .. })
        ));
        let screen = draw(&mut view, 100, 30);
        let git_row = screen.iter().position(|r| r.contains("▾ git")).unwrap() as u16;
        click(&mut view, 26, git_row);
        assert!(matches!(
            view.selected_file_row(),
            Some(TreeRow::Dir {
                dir: 1,
                collapsed: true,
                ..
            })
        ));
        let screen = draw(&mut view, 100, 30);
        assert!(screen.iter().any(|r| r.contains("▸ git")), "{screen:#?}");
        assert!(screen.iter().any(|r| r.contains("A lib.rs")), "{screen:#?}");
        // Enter on a file row shows its diff.
        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Enter));
        assert_eq!(view.pane, Pane::Content);
        let screen = draw(&mut view, 100, 30);
        assert!(
            screen.iter().any(|r| r.contains("+ mod git;")),
            "{screen:#?}"
        );
    }

    /// A repository with a commit whose message and file both have a
    /// long line, and a short commit after it.
    fn repo_with_long_lines() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let sig = Signature::now("T", "t@example.com").unwrap();
        let commit = |files: &[(&str, &str)], message: &str, parents: &[Oid]| {
            for (name, content) in files {
                fs::write(dir.path().join(name), content).unwrap();
            }
            let mut index = repo.index().unwrap();
            for (name, _) in files {
                index.add_path(Path::new(name)).unwrap();
            }
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let parents: Vec<git2::Commit<'_>> = parents
                .iter()
                .map(|id| repo.find_commit(*id).unwrap())
                .collect();
            let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
            repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &refs)
                .unwrap()
        };
        let long_line = format!("let long = \"{}\"; // TAIL", "a".repeat(120));
        let message = format!("Long: {}END", "word ".repeat(30));
        let a = commit(&[("l.rs", &format!("{long_line}\n"))], &message, &[]);
        commit(&[("s.rs", "fn s() {}\n")], "Short", &[a]);
        (dir, message)
    }

    #[test]
    fn the_log_scrolls_sideways_as_the_editor_does() {
        let (dir, message) = repo_with_long_lines();
        let mut view = view(&dir);
        let screen = draw(&mut view, 90, 30);
        // The long message is cut off, so the log has a scrollbar in its
        // last row, and Right scrolls the messages but not the graph.
        assert!(!screen.iter().any(|r| r.contains("END")), "{screen:#?}");
        let log_bottom = view.log_area.bottom() as usize - 1;
        assert!(screen[log_bottom].contains('█'), "{screen:#?}");
        assert_eq!(view.log_h.bar.y as usize, log_bottom);
        let graph_column = column_of(&screen[0], "◉");
        for _ in 0..60 {
            view.handle_key(key(KeyCode::Right));
        }
        let col = view.log_h.col;
        let screen = draw(&mut view, 90, 30);
        assert!(screen.iter().any(|r| r.contains("END")), "{screen:#?}");
        assert_eq!(column_of(&screen[0], "◉"), graph_column, "{screen:#?}");
        // Scrolling right stops at the longest visible line plus its
        // slack, and no further.
        let capacity = view.log_area.width as usize - 1;
        let extent = view.log_extent(view.log_scroll, view.log_rows);
        assert!(display_width(&message) < extent, "the graph is part of it");
        assert_eq!(col, extent + HSCROLL_SLACK - capacity);
        view.handle_key(key(KeyCode::Right));
        assert_eq!(view.log_h.col, col);
        // Moving to the short commit keeps the position, since the long
        // one is still on screen; the horizontal wheel scrolls back.
        view.handle_key(key(KeyCode::Down));
        assert_eq!(view.log_h.col, col);
        view.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollLeft,
            column: 40,
            row: 2,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(view.log_h.col, col - WHEEL_COLUMNS);
        // Dragging the scrollbar's thumb to the left edge scrolls back to
        // the start; then ← at the edge goes to the sidebar.
        let bar = view.log_h.bar;
        click(&mut view, bar.right() - 1, bar.y);
        // The last cell of the track maps just short of the limit, as
        // the editor's does.
        assert!(
            (col - 1..=col).contains(&view.log_h.col),
            "{}",
            view.log_h.col
        );
        assert!(view.is_dragging());
        view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: bar.x,
            row: bar.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(view.log_h.col, 0);
        view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: bar.x,
            row: bar.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(!view.is_dragging());
        // The long message is still cut off, so the bar stays.
        let screen = draw(&mut view, 90, 30);
        assert!(screen[log_bottom].contains('█'), "{screen:#?}");
        view.handle_key(key(KeyCode::Left));
        assert_eq!(view.pane, Pane::Sidebar);
    }

    #[test]
    fn a_diff_scrolls_sideways_with_its_gutter_fixed() {
        let (dir, _) = repo_with_long_lines();
        let mut view = view(&dir);
        view.handle_key(key(KeyCode::End));
        view.handle_key(key(KeyCode::Enter));
        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Enter));
        assert_eq!(view.pane, Pane::Content);
        let screen = draw(&mut view, 100, 30);
        let line = screen
            .iter()
            .position(|r| r.contains("+ let long"))
            .unwrap();
        assert!(!screen[line].contains("TAIL"), "{screen:#?}");
        let content_bottom = view.content_area.bottom() as usize - 1;
        assert!(screen[content_bottom].contains('█'), "{screen:#?}");
        let number_column = column_of(&screen[line], "1 +");
        for _ in 0..50 {
            view.handle_key(key(KeyCode::Right));
        }
        let col = view.content_h.col;
        let screen = draw(&mut view, 100, 30);
        assert!(screen[line].contains("TAIL"), "{screen:#?}");
        assert_eq!(
            column_of(&screen[line], "1 +"),
            number_column,
            "{screen:#?}"
        );
        let extent = view.content_extent(view.content_scroll, view.content_shown);
        assert_eq!(col, extent + HSCROLL_SLACK - view.content_h.capacity);
        // ← scrolls back to the edge and then leaves for the files.
        // Coming back to the file starts at the left again, and the
        // description, with nothing out of view, has no scrollbar.
        for _ in 0..100 {
            if view.pane == Pane::Files {
                break;
            }
            view.handle_key(key(KeyCode::Left));
        }
        assert_eq!(view.pane, Pane::Files);
        assert_eq!(view.content_h.col, 0);
        view.handle_key(key(KeyCode::Right));
        assert_eq!(view.pane, Pane::Content);
        view.handle_key(key(KeyCode::Right));
        assert_eq!(view.content_h.col, WHEEL_COLUMNS);
        view.handle_key(key(KeyCode::Left));
        assert_eq!(view.content_h.col, 0);
        view.handle_key(key(KeyCode::Left));
        assert_eq!(view.pane, Pane::Files);
        view.handle_key(key(KeyCode::Up));
        view.handle_key(key(KeyCode::Up));
        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Down));
        let screen = draw(&mut view, 100, 30);
        assert!(
            screen.iter().any(|r| r.contains("+ let long")),
            "{screen:#?}"
        );
        assert_eq!(view.content_h.col, 0);
        // The description scrolls too: this commit's long message needs
        // the bar, the short commit's doesn't.
        view.handle_key(key(KeyCode::Up));
        let screen = draw(&mut view, 100, 30);
        assert!(screen.iter().any(|r| r.contains("Parents:")), "{screen:#?}");
        assert!(screen[content_bottom].contains('█'), "{screen:#?}");
        view.handle_key(key(KeyCode::Left));
        assert_eq!(view.pane, Pane::Log);
        view.handle_key(key(KeyCode::Home));
        let screen = draw(&mut view, 100, 30);
        assert!(screen.iter().any(|r| r.contains("Parents:")), "{screen:#?}");
        assert!(!screen[content_bottom].contains('█'), "{screen:#?}");
    }

    #[test]
    fn the_rules_between_panes_drag_to_resize_them() {
        let (dir, _) = repo_with_history();
        let mut view = view(&dir);
        draw(&mut view, 120, 40);
        let drag = |view: &mut GitLogView, from: (u16, u16), to: (u16, u16)| {
            click(view, from.0, from.1);
            assert!(view.is_dragging());
            view.handle_mouse(MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: to.0,
                row: to.1,
                modifiers: KeyModifiers::NONE,
            });
            view.handle_mouse(MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: to.0,
                row: to.1,
                modifiers: KeyModifiers::NONE,
            });
            assert!(!view.is_dragging());
        };
        // The sidebar's edge moves right, and no further left than the
        // sidebar's minimum.
        let rule = view.sidebar_rule;
        assert_eq!(rule.width, 1);
        drag(&mut view, (rule.x, 5), (rule.x + 10, 5));
        let screen = draw(&mut view, 120, 40);
        assert_eq!(view.sidebar_area.width, rule.width + rule.x + 10 - 1);
        assert_eq!(column_of(&screen[0], "│"), rule.x + 10);
        drag(&mut view, (rule.x + 10, 5), (2, 5));
        draw(&mut view, 120, 40);
        assert_eq!(view.sidebar_area.width, MIN_SIDEBAR_WIDTH);
        // The log grows downward and the detail keeps its minimum.
        let rule = view.log_rule;
        let log_height = view.log_area.height;
        drag(&mut view, (rule.x + 5, rule.y), (rule.x + 5, rule.y + 6));
        draw(&mut view, 120, 40);
        assert_eq!(view.log_area.height, log_height + 6);
        assert_eq!(view.log_rule.y, rule.y + 6);
        drag(&mut view, (rule.x + 5, rule.y + 6), (rule.x + 5, 200));
        draw(&mut view, 120, 40);
        assert_eq!(view.files_area.height, MIN_DETAIL_HEIGHT);
        // The file list widens, keeping the content its minimum, and
        // the log's share holds when the page grows.
        let rule = view.files_rule;
        drag(&mut view, (rule.x, rule.y), (rule.x + 8, rule.y));
        draw(&mut view, 120, 40);
        assert_eq!(view.files_rule.x, rule.x + 8);
        drag(&mut view, (rule.x + 8, rule.y), (300, rule.y));
        draw(&mut view, 120, 40);
        assert_eq!(view.content_area.width, MIN_CONTENT_WIDTH);
        draw(&mut view, 120, 60);
        assert!(
            (55..=57).contains(&view.log_area.height),
            "the log keeps its share of a taller page: {}",
            view.log_area.height
        );
        // Clicking a rule without moving changes nothing.
        let files = view.files_area;
        let at = (view.files_rule.x, view.files_rule.y);
        drag(&mut view, at, at);
        draw(&mut view, 120, 60);
        assert_eq!(view.files_area, files);
        // The sizes are shares, and a page given them comes out the
        // same; the release of a drag says the sizes changed.
        let sizes = view.sizes();
        assert!(sizes.sidebar.is_some() && sizes.log.is_some() && sizes.files.is_some());
        assert!((0.0..=1.0).contains(&sizes.log.unwrap()));
        let mut again = GitLogView::new(dir.path());
        again.set_sizes(sizes);
        draw(&mut again, 120, 60);
        assert_eq!(again.sidebar_area, view.sidebar_area);
        assert_eq!(again.log_area, view.log_area);
        assert_eq!(again.files_area, view.files_area);
        click(&mut view, at.0, at.1);
        let released = view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: at.0,
            row: at.1,
            modifiers: KeyModifiers::NONE,
        });
        assert!(released);
    }

    #[test]
    fn a_directory_that_is_not_a_repository_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut view = GitLogView::new(dir.path());
        let screen = draw(&mut view, 60, 10);
        assert!(screen[1].contains(NOT_A_REPOSITORY), "{screen:#?}");
        assert!(!view.poll());
    }
}
