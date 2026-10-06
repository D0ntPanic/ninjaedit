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
//! wide rows don't push every message right. The commit HEAD is on is
//! bold and in the theme's `git-head-text`, says `HEAD →` before the
//! branch HEAD is on (or `HEAD` alone when HEAD is detached), and is
//! where the log opens, once the walk (see the core crate's
//! `git::history` module) reaches it.
//!
//! Below the log, the left pane lists `Description` and then every file
//! the selected commit changed, as a tree of directories (see the core
//! crate's `git::tree` module) with how many lines each file gained and
//! lost. The tree starts fully open; a directory folds and unfolds with
//! ← and →, Enter, Space, or a click. The right pane shows whichever
//! is selected: the commit's message with who made it and when, a
//! directory's files, or a file's diff. A diff is drawn as the changes
//! page draws one (see the `diff_pane` module): highlighted as the
//! file would be, over the theme's `diff-added-` and
//! `diff-removed-background` colors, with three lines of context
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
//! round, and clicking a pane moves it there. The arrows never leave a
//! pane: they act within it. In the sidebar ↑ and ↓ move between
//! branches, ← and → fold and unfold a remote (Space too), and Enter
//! goes to the branch's commit in the log. In the log ↑ and ↓ move
//! between commits (Page Up and Page Down by a screenful, Home and End
//! to the ends), ← and → scroll the messages sideways, Enter moves on
//! to the files, and Space (or a double-click) checks the commit out.
//! In the files ↑ and ↓ choose what the right pane shows, Enter or a
//! double-click on a file moves into it, and ← on a file goes to its
//! directory. The diff has a cursor and a selection, as the editor has,
//! to read it by and copy from it (see the `diff_pane` module), with a
//! scrollbar down the side saying where in the diff the pane is and
//! how much of it shows. The wheel scrolls whichever pane it is over,
//! sideways too. In the files and the diff, `o` opens the selected file
//! in the editor as it is in the working directory now, from the diff
//! at the line its cursor is on (one that isn't there any more has
//! nothing to open, and the status bar says so). A right click in the
//! diff opens a menu to open the file there, copy the selection (as
//! shown, as a patch, or as the new side has it), or select it all.
//!
//! A right click on a file or directory of the selected commit selects
//! it and opens a menu of what can be done to it (the application
//! draws the menu; see the `context_menu` module): open it, and, set
//! apart below, restore it. "Restore this version" puts the file in the
//! working directory as the commit left it, and "Restore previous
//! version" as it was before the commit (as the first parent has it,
//! the side the diff is against), undoing what the commit did to it; a
//! file that version hasn't is deleted. On a directory, each file the
//! commit changed under it is restored, and nothing else in it. Only
//! the working directory changes (see the core crate's `git::restore`
//! module), so what was restored shows on the changes page as an
//! unstaged change, to stage or discard. Restoring would lose changes
//! to the files that aren't committed or staged, so with any a box asks
//! first, naming them (see the `confirm_box` module); otherwise it is
//! done at once. Submodules are left alone, and the status bar says so.
//!
//! A right click on a branch in the sidebar selects it (without going
//! to its commit) and opens a menu for it: check it out; merge it into
//! the branch HEAD is on, or rebase that branch onto it; and, set apart
//! below, reset HEAD's branch to it, and delete it. All are in the
//! command palette too while the sidebar has the keyboard. Merging,
//! rebasing, and resetting are for a branch HEAD isn't on or at, and do
//! as they do for a commit in the log (below). Checking out a local branch puts HEAD on
//! it; a remote's branch is checked out as its commit would be from
//! the log were it the only branch there (see below): on the local
//! branch tracking it, fast-forwarded if behind, or on a new one made
//! to track it. Deleting is for a local branch HEAD isn't on, and asks
//! first, in a box, only when the branch has commits that neither HEAD
//! nor its upstream has, which would be left on no branch; otherwise
//! it is done at once, the status bar saying which commit the branch
//! was at. A remote's branch isn't deleted here.
//!
//! A right click on a commit in the log selects it and opens a menu of
//! what can be done with it, each in the command palette too: check it
//! out (as Space does, below); merge the branch there into the branch
//! HEAD is on, or rebase HEAD's branch onto the commit; and, set apart
//! below, reset HEAD's branch to it. Only a commit HEAD isn't at offers
//! more than checking out, and merging needs a branch there other than
//! HEAD's: a local one, or failing that a remote's (see the core
//! crate's `git::merge` and `git::rebase` modules). A merge or rebase
//! does what `git merge` and `git rebase` do, in the background as a
//! checkout does, with the status bar saying how it goes: nothing when
//! HEAD has the commits already, a fast-forward when HEAD has nothing
//! of its own, and otherwise a merge commit or the branch's commits
//! replayed on the target. Changes to tracked files refuse either, as
//! they refuse a checkout. One whose changes conflict stops part way,
//! leaving the conflicts in the files as git does, and brings up the
//! changes page, where they are resolved and the merge committed or
//! the rebase continued, or either aborted. A submodule in conflict is
//! resolved without stopping when its own branch was rebased first onto
//! the commit the target uses, as `git sub-resolve` resolves it (see
//! the core crate's `git::submodule_conflicts` module), so a rebase in
//! that workflow goes through by itself; the status bar says how many
//! were, and names any submodule left in conflict with why; a merge, rebase, or reset
//! is refused until then. A reset is `git reset` in its mixed mode
//! (see the core crate's `git::reset` module): HEAD's branch moves to
//! the commit and the index with it, but the working directory stays
//! as it is, so what differs shows on the changes page as unstaged
//! changes. Commits it would leave on no branch (none other, nor a
//! tag, has them) are named in a box that asks first; otherwise it is
//! done at once. The page refreshes after each, so the log shows where
//! HEAD is now.
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
//! A commit that moves a submodule on lists it among its files, and
//! Enter or a double-click on it goes to the commit the submodule now
//! points at, on the submodule's tab, so that its commits can be
//! reviewed there; the tab's log walks to it if it hasn't yet. A
//! submodule the commit removed has no commit to go to, one with no
//! tab (no longer a submodule, or inside one that isn't initialized)
//! can't be shown, and one whose commit no branch or tag reaches any
//! more isn't in the log: the status bar says which.
//!
//! The page is kept when it is left (for the editor, or another
//! mode) and comes back as it was: the same commit selected, the same
//! file open in it, the log scrolled to the same place. Coming back
//! (or Ctrl+L while it is showing) reads the repository again, since
//! a commit may have been made or a branch fetched meanwhile, but in
//! the background: a second history is opened and walked while the
//! page goes on showing the first, and takes its place only once the
//! walk is done, keeping the place wherever it makes sense. The
//! selected commit is found again by its id, and the log scrolled so
//! it stays on the same row; the sidebar's selection and which remotes
//! are unfolded are kept by name. A commit that is gone (its branch
//! reset, or deleted and the commit unreachable) gives way to HEAD,
//! as the page opens. A repository that wasn't one when the page was
//! opened (`git init` since) shows its history once refreshed.
//!
//! F5 fetches: what `git fetch --all` does (see the core crate's
//! `git::fetch` module), bringing every remote's branches and tags up
//! to date, in the background, with the status bar saying which remote
//! is being fetched meanwhile. Nothing is merged or checked out. Once
//! the fetch is done the page refreshes as it does when coming back,
//! keeping its place, and the status bar says what came of it: how
//! many references changed, and any remote that couldn't be fetched.
//! A fetch is for the shown tab's repository (a submodule's, on its
//! tab), and then for its submodules as git's on-demand recursion
//! does: those a reference that came in points at a commit they don't
//! have, and the one whose diff is on show if it couldn't find its
//! commits. F5 during one does nothing.
//!
//! Space or a double-click on a commit in the log checks it out: HEAD
//! moves there, and the working tree with it, as the core crate's
//! `git::checkout` module does it. A branch pointing at the commit is
//! checked out, so that commits made from there go on the branch. When
//! only a remote's branch points there, the local branch tracking it
//! is fast-forwarded to the commit and checked out if it is merely
//! behind (one that is ahead, or has diverged, refuses: that needs a
//! merge or a rebase, below); with no branch tracking it, a local
//! branch of the same name is made tracking it, and if a local branch
//! by that name already exists elsewhere a box over the log asks for
//! another name (see the `branch_prompt` module). With nothing
//! pointing at the commit, HEAD is detached there. The commit's
//! submodules follow, each on to a local branch of its own that points
//! at its new commit if there is one. Changes to tracked files, staged
//! or not, in the repository or a submodule, refuse the checkout,
//! since it could lose them; the status bar says so, as it says what
//! was checked out and how many submodules came along. The checkout
//! runs in the background, as a fetch does, with the status bar saying
//! which commit is being checked out and how many files are written
//! so far; Space or a double-click meanwhile says one is under way. The
//! page then refreshes, as it does when coming back, so the HEAD marker
//! moves to the commit; the submodules' tabs refresh when next shown.

use crate::branch_prompt::{BranchPrompt, BranchPromptOutcome};
use crate::clicks::ClickTracker;
use crate::clipboard::Clipboard;
use crate::commit_row::{CommitLine, Highlight, commit_extent, draw_commit_line, lane_cap};
use crate::confirm_box::{ConfirmBox, ConfirmOutcome};
use crate::diff_pane::{
    self, ARROW_COLUMNS, ContentPane, CopyAs, HScroll, Piece, Shown, ShownMut, TAB_WIDTH,
    clamp_between, display_width, fit_end, share_for, share_of,
};
use crate::git_layout::{GitLogLayout, MAIN_REPOSITORY, PaneSizes};
use crate::palette::palette_background;
use crate::status::StatusLine;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ninjaedit_core::git::{
    ChangeKind, Checkout, CheckoutError, CheckoutJob, CheckoutOutcome, CommitDetail,
    DeleteBranchError, DiffModel, Fetch, FileChange, FileTree, History, Integration,
    IntegrationJob, Oid, OperationError, Outcome, RefKind, Restored, Target, TreeRow,
    delete_branch, left_behind, reset, restore, short_id, submodules, unstaged_among,
};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::{Color, Modifier, Style};
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
const DESCRIPTION: &str = "Description";
const NOT_A_REPOSITORY: &str = "Not a git repository";
const NOT_INITIALIZED: &str = "Submodule not initialized";
const NO_COMMITS: &str = "No commits yet";
const NO_BRANCHES: &str = "none";
/// The key bindings the status bar lists, pane by pane.
const SIDEBAR_HELP: &[(&str, &str)] = &[
    ("Enter", "go to"),
    ("←→", "fold remote"),
    ("F5", "fetch"),
    ("Tab", "pane"),
    ("Ctrl+E", "leave"),
];
const LOG_HELP: &[(&str, &str)] = &[
    ("Enter", "files"),
    ("Space", "checkout"),
    ("F5", "fetch"),
    ("Tab", "pane"),
    ("Ctrl+E", "leave"),
];
const PROMPT_HELP: &[(&str, &str)] = &[("Enter", "create branch and check out"), ("Esc", "cancel")];
const CONFIRM_HELP: &[(&str, &str)] = &[("y", "restore"), ("n/Esc", "cancel")];
const DELETE_HELP: &[(&str, &str)] = &[("y", "delete"), ("n/Esc", "cancel")];
const RESET_HELP: &[(&str, &str)] = &[("y", "reset"), ("n/Esc", "cancel")];
/// The restore box's button.
const RESTORE: &str = "Restore";
/// The delete branch box's button.
const DELETE: &str = "Delete";
/// The reset box's button.
const RESET: &str = "Reset";
const FILES_HELP: &[(&str, &str)] = &[
    ("Enter", "view"),
    ("←→", "fold/unfold"),
    ("o", "open"),
    ("F5", "fetch"),
    ("Tab", "pane"),
    ("Ctrl+E", "leave"),
];
const CONTENT_HELP: &[(&str, &str)] = &[
    ("Enter", "expand"),
    ("o", "open"),
    ("F5", "fetch"),
    ("Tab", "pane"),
    ("Ctrl+E", "leave"),
];

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

/// A selectable sidebar row by name, to find it again once the
/// branches have changed under it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SideKey {
    Branch(String),
    Remote(String),
    /// The remote's name, then the branch's.
    RemoteBranch(String, String),
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
    Diff(Box<DiffModel>),
    Failed(String),
}

/// What the content pane is to draw for `content`, with the lines of
/// text it shows when it isn't a diff (see
/// [`GitLogView::content_lines`]).
fn shown<'a>(content: Option<&'a Content>, lines: Option<&'a [Piece]>) -> Shown<'a> {
    match (content, lines) {
        (Some(Content::Diff(model)), _) => Shown::Diff(model),
        (Some(Content::Failed(why)), _) => Shown::Note(why),
        (Some(_), Some(lines)) => Shown::Lines(lines),
        _ => Shown::Nothing,
    }
}

/// [`shown`], for input that may move the cursor in a diff, or expand
/// it.
fn shown_mut<'a>(content: Option<&'a mut Content>, lines: Option<&'a [Piece]>) -> ShownMut<'a> {
    match (content, lines) {
        (Some(Content::Diff(model)), _) => ShownMut::Diff(model),
        (Some(Content::Failed(why)), _) => ShownMut::Note(why),
        (Some(_), Some(lines)) => ShownMut::Lines(lines),
        _ => ShownMut::Nothing,
    }
}

/// Which version of the selected files a restore puts in the working
/// directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    /// As the selected commit left them.
    Commit,
    /// As they were before it: as its first parent has them, the side
    /// its diff is against.
    Parent,
}

/// A restore of some of a commit's files: which, and from where.
struct RestoreRequest {
    /// The commit whose files are restored, and which version of them.
    commit: Oid,
    version: Version,
    /// The commit that version is (none, for the version before a root
    /// commit: the files didn't exist).
    source: Option<Oid>,
    /// The files' paths, and their paths before a rename.
    paths: Vec<String>,
    /// What is restored, for the status bar: a file's path, or a
    /// directory's with a slash.
    what: String,
    /// How many submodules under a directory are left alone.
    kept: usize,
}

impl RestoreRequest {
    /// Which version, in words: `as of 1a2b3c4d`, `as before 1a2b3c4d`.
    fn version_words(&self) -> String {
        match self.version {
            Version::Commit => format!("as of {}", short_id(self.commit)),
            Version::Parent => format!("as before {}", short_id(self.commit)),
        }
    }
}

/// A restore waiting on the restore box, which asks because it would
/// overwrite changes that aren't committed or staged.
struct PendingRestore {
    dialog: ConfirmBox,
    request: RestoreRequest,
}

/// A deletion of a branch waiting on the delete branch box, which asks
/// because the branch has commits no other branch has.
struct PendingDelete {
    dialog: ConfirmBox,
    /// The local branch to delete.
    name: String,
}

/// A reset waiting on the reset box, which asks because commits of
/// HEAD's would be left on no branch.
struct PendingReset {
    dialog: ConfirmBox,
    /// The commit to reset to, and what to call it in the status bar:
    /// a branch's name or the commit's short id.
    id: Oid,
    what: String,
}

/// A merge or rebase under way, and what of.
struct RunningIntegration {
    what: Integration,
    job: IntegrationJob,
}

/// A checkout under way, and what of.
struct RunningCheckout {
    /// What is being checked out, for the status bar: a commit's short
    /// id, or a branch's name.
    what: String,
    /// The commit being checked out: the one picked, or the one a
    /// branch was at when picked until the job has looked it up again.
    id: Oid,
    job: CheckoutJob,
}

/// A branch selected in the sidebar.
struct SideBranch {
    /// Its name as git has it: `main`, or `origin/main` for a remote's.
    name: String,
    remote: bool,
    /// The commit it is at.
    target: Oid,
    is_head: bool,
}

/// Which of the page's right-click menus to open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitLogMenu {
    /// For a commit in the log.
    Commit,
    /// For a file or directory of the selected commit.
    Files,
    /// For a branch in the sidebar.
    Branch,
    /// For the selection in a file's diff.
    Diff,
}

/// What the application should do after the page handled a mouse
/// event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GitLogMouseOutcome {
    /// The pane sizes changed (a drag of a rule ended), so the layout
    /// is worth keeping.
    pub resized: bool,
    /// A right press on a commit in the log, on a file or directory of
    /// the selected commit, or on a branch in the sidebar, which it
    /// selected: open that menu of the page at the pointer, for it.
    pub menu: Option<GitLogMenu>,
}

/// What a page asks of its tabs when Enter or a double-click lands on
/// a submodule among a commit's files: to show a commit of the
/// submodule on the submodule's own tab.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SubmoduleJump {
    /// The submodule's path, from the page's repository.
    path: String,
    /// The commit to go to: the one the submodule points at after the
    /// change.
    id: Oid,
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
    /// Whether the page, if opened, is to read its repository again
    /// the next time it is shown: the tabs were refreshed while it
    /// was hidden.
    stale: bool,
}

/// The git log page's tabs: the project's repository first, then each
/// of its submodules, nested ones included, by path. A submodule's page
/// is opened the first time its tab is shown, with the pane sizes kept
/// for it. A refresh reads the submodules again and refreshes the shown
/// page; the other opened pages refresh when next shown.
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
            stale: false,
        }];
        tabs.extend(submodules(root).into_iter().map(|submodule| GitTab {
            title: submodule.path,
            workdir: submodule.workdir,
            submodule: true,
            view: None,
            stale: false,
        }));
        GitLogTabs {
            tabs,
            active: 0,
            layout,
        }
    }

    /// Read the repository again, in the background: the shown page
    /// refreshes now (see [`GitLogView::refresh`]), the other opened
    /// pages when next shown. The tabs follow the submodules as they
    /// are now, keeping the pages of those still there; a shown tab
    /// whose submodule is gone gives way to the main repository's.
    pub fn refresh(&mut self) {
        self.rescan();
        self.active();
    }

    /// Read the submodules again and mark every page as due a refresh
    /// when next shown (the shown one included, until [`active`](Self::active)
    /// is next asked for it).
    fn rescan(&mut self) {
        let root = self.tabs[0].workdir.clone();
        let shown = self.tabs[self.active].title.clone();
        let mut old: Vec<GitTab> = self.tabs.drain(1..).collect();
        for submodule in submodules(&root) {
            let view = old
                .iter()
                .position(|tab| tab.title == submodule.path)
                .and_then(|index| old.remove(index).view);
            self.tabs.push(GitTab {
                title: submodule.path,
                workdir: submodule.workdir,
                submodule: true,
                view,
                stale: true,
            });
        }
        self.tabs[0].stale = true;
        self.active = self
            .tabs
            .iter()
            .position(|tab| tab.title == shown)
            .unwrap_or(0);
    }

    /// After a checkout on the shown page (which finishes in a poll)
    /// that has refreshed itself:
    /// the submodules may have moved with it, or come and gone, so the
    /// tabs are read again and the other pages made due a refresh.
    fn follow_checkout(&mut self) {
        if !self.active().take_checked_out() {
            return;
        }
        self.rescan();
        self.tabs[self.active].stale = false;
    }

    /// The pane sizes of every repository, to keep.
    pub fn layout(&self) -> &GitLogLayout {
        &self.layout
    }

    /// Give the shown page a key, and go where it asks (see
    /// [`follow_submodule`](Self::follow_submodule)).
    pub fn handle_key(&mut self, key: KeyEvent, clipboard: &mut Clipboard) {
        self.active().handle_key(key, clipboard);
        self.follow_checkout();
        self.follow_submodule();
    }

    /// Add pasted text to the shown page's branch name box, if open.
    pub fn paste(&mut self, text: &str) {
        self.active().paste(text);
    }

    /// The command palette's "Fetch from remotes": the shown page's F5.
    pub fn fetch(&mut self) {
        self.active().fetch();
    }

    /// The command palette's "Check out commit": the shown page's Space
    /// in the log.
    pub fn checkout_selected(&mut self) {
        self.active().checkout_selected();
        self.follow_checkout();
    }

    /// The command palette's "Check out branch": the shown page's
    /// branch selected in the sidebar.
    pub fn checkout_selected_branch(&mut self) {
        self.active().checkout_selected_branch();
        self.follow_checkout();
    }

    /// The command palette's "Delete branch": the shown page's local
    /// branch selected in the sidebar, asking first if it has commits
    /// of its own.
    pub fn delete_selected_branch(&mut self) {
        self.active().delete_selected_branch();
    }

    /// The command palette's "Merge into current branch": the branch at
    /// the shown page's selected commit, in the background.
    pub fn merge_selected_commit(&mut self) {
        self.active().merge_selected_commit();
    }

    /// The command palette's "Rebase current branch onto commit": the
    /// shown page's selected commit, in the background.
    pub fn rebase_onto_selected_commit(&mut self) {
        self.active().rebase_onto_selected_commit();
    }

    /// The command palette's "Reset current branch to commit": the
    /// shown page's selected commit, asking first if commits would be
    /// left on no branch.
    pub fn reset_to_selected_commit(&mut self) {
        self.active().reset_to_selected_commit();
        self.follow_checkout();
    }

    /// The command palette's "Merge branch into current branch": the
    /// branch selected in the shown page's sidebar, in the background.
    pub fn merge_selected_branch(&mut self) {
        self.active().merge_selected_branch();
    }

    /// The command palette's "Rebase current branch onto branch": the
    /// branch selected in the shown page's sidebar, in the background.
    pub fn rebase_onto_selected_branch(&mut self) {
        self.active().rebase_onto_selected_branch();
    }

    /// The command palette's "Reset current branch to branch": the
    /// branch selected in the shown page's sidebar, asking first if
    /// commits would be left on no branch.
    pub fn reset_to_selected_branch(&mut self) {
        self.active().reset_to_selected_branch();
        self.follow_checkout();
    }

    /// The repository whose changes page is to be shown, as the tabs
    /// open it (see [`active_repository`](Self::active_repository)),
    /// when a merge or rebase on the shown page stopped at conflicts
    /// since this was last asked: they are resolved there.
    pub fn take_show_changes(&mut self) -> Option<(PathBuf, bool)> {
        let shown = self.tabs[self.active]
            .view
            .as_mut()
            .is_some_and(GitLogView::take_show_changes);
        shown.then(|| self.active_repository())
    }

    /// Open the selected file of the shown page's commit in the editor,
    /// as its `o` does; see [`take_file_to_open`](Self::take_file_to_open).
    pub fn open_selected_change(&mut self) {
        self.active().open_selected_change();
    }

    /// Open the file of the shown page's diff in the editor at the line
    /// its cursor is on, as `o` in the diff does.
    pub fn open_change_at_cursor(&mut self) {
        self.active().open_change_at_cursor();
    }

    /// Restore the selected files of the shown page's commit in the
    /// working directory, as the commit left them or as they were
    /// before it, asking first if that would overwrite changes that
    /// aren't committed or staged.
    pub fn restore_selected(&mut self, version: Version) {
        self.active().restore_selected(version);
    }

    /// The file of the working directory the shown page asks to have
    /// opened in the editor, if it has asked since this was last asked,
    /// with the line (from zero) to open it at, if any.
    pub fn take_file_to_open(&mut self) -> Option<(PathBuf, Option<usize>)> {
        self.tabs[self.active]
            .view
            .as_mut()
            .and_then(|view| view.file_to_open.take())
    }

    /// Give the shown page a mouse event, and go where it asks (see
    /// [`follow_submodule`](Self::follow_submodule)). A change to its
    /// pane sizes (a drag of a rule ended) goes into the layout, which
    /// is then worth keeping. `wheel` is how many rows (or columns,
    /// sideways) a wheel event scrolls.
    pub fn handle_mouse(&mut self, mouse: MouseEvent, wheel: usize) -> GitLogMouseOutcome {
        let active = self.active;
        let outcome = self.active().handle_mouse(mouse, wheel);
        if outcome.resized {
            let sizes = self.tabs[active].view.as_ref().map(GitLogView::sizes);
            if let Some(sizes) = sizes {
                let key = self.tabs[active].key().to_owned();
                self.layout.set(&key, sizes);
            }
        }
        self.follow_checkout();
        self.follow_submodule();
        outcome
    }

    /// Show the commit of a submodule the shown page asks for, on the
    /// submodule's tab: Enter or a double-click on a submodule among a
    /// commit's files goes to the commit it now points at. The tab is
    /// named by the submodule's path from the main repository, which
    /// for a submodule of a submodule is the shown tab's path and then
    /// its own. A submodule with no tab (no longer one, or inside one
    /// that isn't initialized) can't be shown, and the status bar says
    /// so.
    fn follow_submodule(&mut self) {
        let Some(jump) = self.active().take_submodule_jump() else {
            return;
        };
        let tab = &self.tabs[self.active];
        let title = if tab.submodule {
            format!("{}/{}", tab.title, jump.path)
        } else {
            jump.path.clone()
        };
        match self.tabs.iter().position(|tab| tab.title == title) {
            Some(index) => {
                self.active = index;
                self.active().go_to(jump.id);
            }
            None => self.active().set_notice(StatusLine::error(format!(
                "No tab for submodule {}",
                jump.path
            ))),
        }
    }

    /// The tabs' titles, in order: the page's name, then each
    /// submodule's path.
    pub fn titles(&self) -> impl Iterator<Item = &str> {
        self.tabs.iter().map(|tab| tab.title.as_str())
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

    /// The shown tab's page, opened if this is its first showing and
    /// refreshed if it is due one.
    pub fn active(&mut self) -> &mut GitLogView {
        let tab = &mut self.tabs[self.active];
        let sizes = self.layout.get(tab.key());
        if std::mem::take(&mut tab.stale)
            && let Some(view) = &mut tab.view
        {
            view.refresh();
        }
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

    /// The shown tab's repository as its page opens it: the working
    /// directory, and whether as exactly that directory (a
    /// submodule's) rather than whatever repository contains it.
    pub fn active_repository(&self) -> (PathBuf, bool) {
        let tab = &self.tabs[self.active];
        (tab.workdir.clone(), tab.submodule)
    }

    /// Take in commits the shown page's walk has produced. Returns
    /// whether the page needs redrawing.
    pub fn poll(&mut self) -> bool {
        let polled = self.active().poll();
        self.follow_checkout();
        polled
    }

    /// What the status bar shows.
    pub fn hint(&self) -> StatusLine {
        self.active_view().map(GitLogView::hint).unwrap_or_default()
    }

    /// What the status bar is to say, once: how the shown page's fetch
    /// or checkout went, when one has finished since the last time this
    /// was asked.
    pub fn take_notice(&mut self) -> Option<StatusLine> {
        self.tabs[self.active]
            .view
            .as_mut()
            .and_then(GitLogView::take_notice)
    }

    #[cfg(test)]
    pub fn is_loading(&self) -> bool {
        self.active_view().is_some_and(GitLogView::is_loading)
    }
}

pub struct GitLogView {
    /// Where the repository was opened from, and whether as exactly
    /// that directory (a submodule's) rather than whatever repository
    /// contains it, so a refresh opens it the same way.
    root: PathBuf,
    exact: bool,
    history: Option<History>,
    /// Why there is no history: the project isn't in a repository.
    error: Option<String>,
    /// The history being read again in the background, which replaces
    /// `history` once its walk is done; see [`refresh`](Self::refresh).
    pending: Option<History>,
    /// A fetch under way (F5); the page refreshes once it is done.
    fetch: Option<Fetch>,
    /// A checkout under way (Space or a double-click in the log, or a
    /// branch from the sidebar); the page refreshes once it is done.
    checkout: Option<RunningCheckout>,
    /// A merge or rebase under way; the page refreshes once it is
    /// done.
    integration: Option<RunningIntegration>,
    /// What the status bar is to say, once: how a fetch or a checkout
    /// went.
    notice: Option<StatusLine>,
    pane: Pane,
    side_rows: Vec<SideRow>,
    /// Which remotes are folded up; all of them to start with.
    collapsed: Vec<bool>,
    side_selected: usize,
    side_scroll: usize,
    reveal_side: bool,
    /// The selected commit, an index into the history's commits.
    selected: usize,
    /// How many rows of the log are scrolled off its top, two a commit
    /// (an odd count leaves the top commit's second line showing), and
    /// how many rows of it were shown at the last render.
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
    content_pane: ContentPane,
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
    /// Presses in the log and the file list, to notice a double-click.
    clicks: ClickTracker,
    /// A commit of a submodule to show on the submodule's tab, asked
    /// for and not yet taken (see [`take_submodule_jump`](Self::take_submodule_jump)).
    submodule_jump: Option<SubmoduleJump>,
    /// The box asking for a new branch's name, while a checkout waits
    /// on one.
    branch_prompt: Option<BranchPrompt>,
    /// Whether a checkout has been done and not yet noticed by the
    /// tabs (see [`take_checked_out`](Self::take_checked_out)).
    checked_out: bool,
    /// A file of the working directory to open in the editor, asked
    /// for with `o` or "Open changed file" and not yet taken (see
    /// [`GitLogTabs::take_file_to_open`]).
    file_to_open: Option<(PathBuf, Option<usize>)>,
    /// The restore box, while it asks whether to go ahead.
    restore: Option<PendingRestore>,
    /// The delete branch box, while it asks whether to go ahead.
    branch_delete: Option<PendingDelete>,
    /// The reset box, while it asks whether to go ahead.
    reset_box: Option<PendingReset>,
    /// Whether a merge or rebase stopped at conflicts and the changes
    /// page, where they are resolved, is to be shown, and that hasn't
    /// been asked since (see [`take_show_changes`](Self::take_show_changes)).
    show_changes: bool,
}

impl GitLogView {
    /// A page for the repository containing `root`, with its walk
    /// started.
    pub fn new(root: &Path) -> GitLogView {
        GitLogView::from_root(root, false)
    }

    /// A page for the repository whose working directory is `root`
    /// itself: a submodule's. One that isn't initialized says so.
    pub fn for_repository(root: &Path) -> GitLogView {
        GitLogView::from_root(root, true)
    }

    /// Open the repository as the page was opened, and start its walk.
    fn open_history(root: &Path, exact: bool) -> Result<History, String> {
        if exact {
            History::open_repository(root).map_err(|_| NOT_INITIALIZED.to_owned())
        } else {
            History::open(root).map_err(|err| err.message().to_owned())
        }
    }

    fn from_root(root: &Path, exact: bool) -> GitLogView {
        let mut view = GitLogView {
            root: root.to_path_buf(),
            exact,
            history: None,
            error: None,
            pending: None,
            fetch: None,
            checkout: None,
            integration: None,
            notice: None,
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
            content_pane: ContentPane::default(),
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
            clicks: ClickTracker::default(),
            submodule_jump: None,
            branch_prompt: None,
            checked_out: false,
            file_to_open: None,
            restore: None,
            branch_delete: None,
            reset_box: None,
            show_changes: false,
        };
        view.install(GitLogView::open_history(root, exact));
        view
    }

    /// Show a history (or the lack of one) from the start, as the page
    /// opens: the sidebar's remotes folded, its first branch selected,
    /// the log at its top until the walk reaches HEAD.
    fn install(&mut self, history: Result<History, String>) {
        let (history, error) = match history {
            Ok(history) => (Some(history), None),
            Err(message) => (None, Some(message)),
        };
        self.history = history;
        self.error = error;
        self.pending = None;
        self.collapsed = self
            .history
            .as_ref()
            .map_or_else(Vec::new, |history| vec![true; history.remotes().len()]);
        self.rebuild_side_rows();
        self.side_selected = self
            .side_rows
            .iter()
            .position(|row| row.selectable())
            .unwrap_or(0);
        self.side_scroll = 0;
        self.reveal_side = true;
        self.selected = 0;
        self.log_scroll = 0;
        self.log_h.col = 0;
        self.head_shown = false;
        self.pending_jump = None;
        self.reset_detail();
        self.poll();
    }

    /// Forget the selected commit's detail and everything shown of it,
    /// to be read again for whatever commit is selected.
    fn reset_detail(&mut self) {
        self.detail = None;
        self.detail_of = None;
        self.file_tree = FileTree::default();
        self.dirs_collapsed.clear();
        self.file_rows.clear();
        self.file_selected = 0;
        self.files_scroll = 0;
        self.content = None;
        self.content_pane.reset();
    }

    /// Read the repository again, without disturbing what the page
    /// shows: a fresh history is opened and walked in the background,
    /// and replaces the one shown once the walk is done (see
    /// [`poll`](Self::poll)), keeping the place where it makes sense.
    /// A refresh started while one is under way supersedes it. With no
    /// history to show (the directory wasn't a repository, or a
    /// submodule wasn't initialized) there is no place to keep, so
    /// whatever opens now shows from the start; and a repository that
    /// can't be opened any more says so in place of the history.
    pub fn refresh(&mut self) {
        let fresh = GitLogView::open_history(&self.root, self.exact);
        match (&self.history, fresh) {
            (Some(_), Ok(fresh)) => self.pending = Some(fresh),
            (None, fresh @ Ok(_)) | (_, fresh @ Err(_)) => self.install(fresh),
        }
    }

    /// Take in commits the walk has produced, and the refresh's once
    /// it is done. Returns whether the page needs redrawing.
    pub fn poll(&mut self) -> bool {
        let checked_out = self.poll_checkout() | self.poll_integration();
        let fetched = self.poll_fetch() || checked_out;
        let refreshed = self.poll_refresh() || fetched;
        let Some(history) = &mut self.history else {
            return refreshed;
        };
        if !history.poll() && !refreshed {
            return self.settle_jump();
        }
        let history = self.history.as_ref().expect("polled just above");
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
        self.settle_jump();
        true
    }

    /// Give up a commit to go to that the walk, now done, didn't reach:
    /// no branch or tag reaches it, so it isn't in the log, and the
    /// status bar says so. Returns whether that happened.
    fn settle_jump(&mut self) -> bool {
        let Some(id) = self.pending_jump else {
            return false;
        };
        let walked = self.pending.is_none()
            && self
                .history
                .as_ref()
                .is_some_and(|history| !history.is_loading());
        if !walked {
            return false;
        }
        self.pending_jump = None;
        self.notice = Some(StatusLine::error(format!(
            "Commit {} is not in the log",
            short_id(id)
        )));
        true
    }

    /// F5: fetch every remote of the repository, in the background;
    /// see the core crate's `git::fetch` module. The page refreshes
    /// once the fetch is done, and the status bar says how it went. A
    /// fetch already under way is left to finish.
    pub fn fetch(&mut self) {
        if self.fetch.is_some() {
            return;
        }
        let Some(history) = &self.history else {
            self.notice = Some(StatusLine::error(self.no_history_message()));
            return;
        };
        // A submodule diff on show that couldn't find its commits
        // names them, so that the fetch brings them whatever else says.
        let wanted = match &self.content {
            Some(Content::Diff(model)) => model
                .diff()
                .submodule
                .as_ref()
                .map(|range| {
                    range
                        .missing
                        .iter()
                        .map(|id| (model.diff().path.clone(), *id))
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        match Fetch::start(history.git_dir(), wanted) {
            Ok(fetch) => self.fetch = Some(fetch),
            Err(err) => {
                self.notice = Some(StatusLine::error(format!(
                    "Could not fetch: {}",
                    err.message()
                )));
            }
        }
    }

    /// Why there is no history, as the page says it in place of one.
    fn no_history_message(&self) -> String {
        match &self.error {
            Some(error) if error == NOT_INITIALIZED => error.clone(),
            Some(error) if !error.is_empty() => format!("{NOT_A_REPOSITORY}: {error}"),
            _ => NOT_A_REPOSITORY.to_owned(),
        }
    }

    /// Take in what the fetch has reported, and once it is done say
    /// how it went and read the repository again. Returns whether
    /// anything changed.
    fn poll_fetch(&mut self) -> bool {
        let Some(fetch) = &mut self.fetch else {
            return false;
        };
        let changed = fetch.poll();
        if !fetch.is_done() {
            return changed;
        }
        self.notice = Some(match self.fetch.take().and_then(Fetch::outcome) {
            // A remote that couldn't be fetched makes the report an
            // error, whatever else went well.
            Some(Ok(report)) => {
                let failed = !report.failed.is_empty()
                    || report.submodules.iter().any(|(_, r)| !r.failed.is_empty());
                if failed {
                    StatusLine::error(report.summary())
                } else {
                    StatusLine::info(report.summary())
                }
            }
            Some(Err(why)) => StatusLine::error(format!("Could not fetch: {why}")),
            None => StatusLine::error("Could not fetch: the fetch stopped"),
        });
        // A submodule's diff is read from the submodule's repository,
        // which the fetch may have filled in: build it again. A file's
        // diff keeps its revealed context.
        if matches!(&self.content, Some(Content::Diff(model)) if model.diff().submodule.is_some()) {
            self.content = None;
        }
        self.refresh();
        true
    }

    /// Take in what the refresh's walk has produced, and once it is
    /// done put its history in place of the one shown. Returns whether
    /// anything changed.
    fn poll_refresh(&mut self) -> bool {
        let Some(fresh) = &mut self.pending else {
            return false;
        };
        let changed = fresh.poll();
        if fresh.is_loading() {
            return changed;
        }
        if let Some(fresh) = self.pending.take() {
            self.replace_history(fresh);
        }
        true
    }

    /// Show a refreshed history in place of the one shown, keeping the
    /// place: the selected commit, found again by its id, on the same
    /// row of the log, with its detail (a commit's content doesn't
    /// change) and whatever is shown of it; the sidebar's selection and
    /// unfolded remotes, by name. A selected commit that is gone gives
    /// way to HEAD, as the page opens; a sidebar selection that is gone
    /// to the first branch.
    fn replace_history(&mut self, fresh: History) {
        let selected = self.selected_id();
        let offset = (self.selected * 2).saturating_sub(self.log_scroll);
        let side_selected = self.side_key(self.side_selected);
        let unfolded: Vec<String> = self
            .history
            .as_ref()
            .map(|history| {
                history
                    .remotes()
                    .iter()
                    .enumerate()
                    .filter(|(r, _)| !self.collapsed.get(*r).copied().unwrap_or(true))
                    .map(|(_, remote)| remote.name.clone())
                    .collect()
            })
            .unwrap_or_default();

        let position = selected.and_then(|id| fresh.position(id));
        self.collapsed = fresh
            .remotes()
            .iter()
            .map(|remote| !unfolded.contains(&remote.name))
            .collect();
        self.history = Some(fresh);
        self.error = None;
        self.rebuild_side_rows();
        self.side_selected = (0..self.side_rows.len())
            .find(|index| side_selected.is_some() && self.side_key(*index) == side_selected)
            .or_else(|| self.side_rows.iter().position(|row| row.selectable()))
            .unwrap_or(0);
        self.reveal_side = true;

        match position {
            Some(position) => {
                self.selected = position;
                self.log_scroll = (position * 2).saturating_sub(offset);
                self.head_shown = true;
            }
            None => {
                self.selected = 0;
                self.log_scroll = 0;
                self.head_shown = false;
                self.reset_detail();
            }
        }
        self.reveal_log = true;
    }

    /// The selected commit's id, if any.
    fn selected_id(&self) -> Option<Oid> {
        self.history
            .as_ref()
            .and_then(|h| h.commits().get(self.selected))
            .map(|c| c.id)
    }

    /// What a sidebar row is, by name rather than by index, so it can
    /// be found again after the branches change: a local branch, a
    /// remote, or a remote's branch.
    fn side_key(&self, index: usize) -> Option<SideKey> {
        let history = self.history.as_ref()?;
        match self.side_rows.get(index)? {
            SideRow::Branch(b) => Some(SideKey::Branch(history.branches().get(*b)?.name.clone())),
            SideRow::Remote(r) => Some(SideKey::Remote(history.remotes().get(*r)?.name.clone())),
            SideRow::RemoteBranch(r, b) => {
                let remote = history.remotes().get(*r)?;
                Some(SideKey::RemoteBranch(
                    remote.name.clone(),
                    remote.branches.get(*b)?.name.clone(),
                ))
            }
            _ => None,
        }
    }

    /// What the status bar shows for the page.
    pub fn hint(&self) -> StatusLine {
        // Something under way has the bar to itself: it is what the
        // user is waiting on, and the bindings have been seen.
        if let Some(RunningCheckout { what, job, .. }) = &self.checkout {
            return StatusLine::progress(match job.progress() {
                Some((written, total)) => {
                    format!("checking out {what}… {written}/{total} files")
                }
                None => format!("checking out {what}…"),
            });
        }
        if let Some(RunningIntegration { what, job }) = &self.integration {
            let doing = match what {
                Integration::Merge(target) => format!("merging {}", target.name()),
                Integration::Rebase(target) => format!("rebasing onto {}", target.name()),
            };
            return StatusLine::progress(match job.progress() {
                Some((written, total)) => format!("{doing}… {written}/{total} files"),
                None => format!("{doing}…"),
            });
        }
        if let Some(history) = &self.history
            && history.is_loading()
        {
            return StatusLine::progress(format!("loading {} commits…", history.commits().len()));
        }
        if let Some(fresh) = &self.pending {
            return StatusLine::progress(format!("refreshing {} commits…", fresh.commits().len()));
        }
        if let Some(fetch) = &self.fetch {
            return StatusLine::progress(match fetch.current() {
                Some(remote) => format!("fetching {remote}…"),
                None => "fetching…".to_owned(),
            });
        }
        if self.branch_prompt.is_some() {
            return StatusLine::help(PROMPT_HELP);
        }
        if self.restore.is_some() {
            return StatusLine::help(CONFIRM_HELP);
        }
        if self.branch_delete.is_some() {
            return StatusLine::help(DELETE_HELP);
        }
        if self.reset_box.is_some() {
            return StatusLine::help(RESET_HELP);
        }
        StatusLine::help(match self.pane {
            Pane::Sidebar => SIDEBAR_HELP,
            Pane::Log => LOG_HELP,
            Pane::Files => FILES_HELP,
            Pane::Content => CONTENT_HELP,
        })
    }

    /// What the status bar is to say, once: how a fetch or a checkout
    /// went, or why a commit couldn't be gone to, when any has happened
    /// since the last time this was asked.
    pub fn take_notice(&mut self) -> Option<StatusLine> {
        self.notice.take()
    }

    fn set_notice(&mut self, notice: StatusLine) {
        self.notice = Some(notice);
    }

    /// The commit of a submodule the page asks to have shown on the
    /// submodule's tab, when Enter or a double-click has asked for one
    /// since the last time this was asked.
    fn take_submodule_jump(&mut self) -> Option<SubmoduleJump> {
        self.submodule_jump.take()
    }

    /// Go to a commit in the log, with the keyboard there: now if the
    /// walk has reached it, otherwise when it does; if the walk ends
    /// without it, the status bar says so.
    fn go_to(&mut self, id: Oid) {
        self.pane = Pane::Log;
        self.jump_to(id);
        self.settle_jump();
    }

    /// The selected commit's id, if any.
    pub fn selected_commit(&self) -> Option<Oid> {
        self.selected_id()
    }

    /// Whether the walk, or a refresh's, or a fetch, or a checkout is
    /// still going.
    #[cfg(test)]
    pub fn is_loading(&self) -> bool {
        self.history.as_ref().is_some_and(History::is_loading)
            || self.pending.is_some()
            || self.fetch.is_some()
            || self.checkout.is_some()
            || self.integration.is_some()
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
            self.content_pane.reset();
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
            self.content_pane.reset();
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

    /// Enter or a double-click on a file of the list: move into its
    /// diff, or for a submodule ask for the commit it now points at to
    /// be shown on the submodule's tab (see
    /// [`GitLogTabs::follow_submodule`]). A submodule the commit
    /// removed points at nothing after it, and the status bar says so.
    fn open_selected_file(&mut self) {
        let Some(TreeRow::File { file, .. }) = self.selected_file_row() else {
            return;
        };
        let is_submodule = self
            .detail
            .as_ref()
            .and_then(|detail| detail.files.get(file))
            .is_some_and(|file| file.submodule);
        if !is_submodule {
            self.pane = Pane::Content;
            return;
        }
        // The submodule's diff carries the commits it moved over.
        self.ensure_content();
        match &self.content {
            Some(Content::Diff(model)) => {
                let diff = model.diff();
                match diff.submodule.as_ref().and_then(|r| r.new) {
                    Some(id) => {
                        self.submodule_jump = Some(SubmoduleJump {
                            path: diff.path.clone(),
                            id,
                        });
                    }
                    None => {
                        self.notice = Some(StatusLine::error(format!(
                            "Submodule {} was removed: no commit to go to",
                            diff.path
                        )));
                    }
                }
            }
            Some(Content::Failed(why)) => self.notice = Some(StatusLine::error(why.clone())),
            _ => {}
        }
    }

    // ----- Opening and restoring files -----------------------------------

    /// The file selected in the file list, if a file (not a directory,
    /// nor the description) is.
    fn selected_change(&self) -> Option<&FileChange> {
        match self.selected_file_row()? {
            TreeRow::File { file, .. } => self.detail.as_ref()?.files.get(file),
            TreeRow::Dir { .. } => None,
        }
    }

    /// Whether a file (not a submodule) is selected, for `o` to open.
    pub fn can_open_selected_change(&self) -> bool {
        self.selected_change()
            .is_some_and(|change| !change.submodule)
    }

    /// `o` in the files, or "Open changed file": ask for the selected
    /// file to be opened in the editor as it is in the working directory
    /// now (see [`GitLogTabs::take_file_to_open`]). A file that isn't
    /// there (the commit deleted it, or a later one did) has nothing to
    /// open, and the status bar says so.
    pub fn open_selected_change(&mut self) {
        self.open_change(false);
    }

    /// `o` in the diff, or "Open file at cursor": the same, at the line
    /// the diff's cursor is on, as near as the commit's lines say.
    pub fn open_change_at_cursor(&mut self) {
        self.open_change(true);
    }

    /// Whether the selected file's diff is shown, with lines for its
    /// cursor to say where to open the file.
    pub fn can_open_at_cursor(&self) -> bool {
        self.shows_diff_lines() && self.can_open_selected_change()
    }

    fn open_change(&mut self, at_cursor: bool) {
        self.ensure_detail();
        let Some(change) = self.selected_change() else {
            return;
        };
        if change.submodule {
            self.notice = Some(StatusLine::info(format!(
                "{} is a submodule: Enter goes to its commit",
                change.path
            )));
            return;
        }
        let Some(workdir) = self.history.as_ref().and_then(History::workdir) else {
            return;
        };
        let path = workdir.join(&change.path);
        if path.is_file() {
            self.ensure_content();
            let line = match &self.content {
                Some(Content::Diff(model)) if at_cursor => model.new_line_at_cursor(),
                _ => None,
            };
            self.file_to_open = Some((path, line));
        } else {
            self.notice = Some(StatusLine::info(format!(
                "{} isn't in the working directory",
                change.path
            )));
        }
    }

    /// How the status bar names what a row of the file list stands
    /// for: a file's path, or a directory's with a slash.
    fn describe_file_row(&self, row: TreeRow) -> String {
        match row {
            TreeRow::Dir { dir, .. } => format!("{}/", self.file_tree.dirs()[dir].path),
            TreeRow::File { file, .. } => self
                .detail
                .as_ref()
                .and_then(|detail| detail.files.get(file))
                .map(|change| change.path.clone())
                .unwrap_or_default(),
        }
    }

    /// The paths a restore of the selected row writes or deletes: a
    /// file's, or those of the commit's files under a directory (not
    /// the rest of the directory: what the commit changed is what is
    /// restored), each with its path before a rename, since the file
    /// was there on one side. Submodules are left out, and counted.
    fn restorable_selection(&self) -> (Vec<String>, usize) {
        let (Some(detail), Some(row)) = (&self.detail, self.selected_file_row()) else {
            return (Vec::new(), 0);
        };
        let files = match row {
            TreeRow::Dir { dir, .. } => self.file_tree.files_under(dir),
            TreeRow::File { file, .. } => vec![file],
        };
        let mut paths = Vec::new();
        let mut kept = 0;
        for change in files.iter().filter_map(|f| detail.files.get(*f)) {
            if change.submodule {
                kept += 1;
                continue;
            }
            paths.push(change.path.clone());
            paths.extend(change.old_path.clone());
        }
        (paths, kept)
    }

    /// Whether the selected file or directory has files to restore.
    pub fn can_restore_selected(&self) -> bool {
        !self.restorable_selection().0.is_empty()
    }

    /// "Restore this version" or "Restore previous version": put the
    /// selected file in the working directory as the commit left it, or
    /// as it was before the commit (as the first parent, which the diff
    /// is against, has it), or likewise every file the commit changed
    /// under the selected directory. A file the version hasn't is
    /// deleted. Only the working directory changes, so the outcome is
    /// an unstaged change to review on the changes page. Changes to the
    /// files that aren't committed or staged would be lost, so with any
    /// the restore box asks first (see [`handle_restore_outcome`](Self::handle_restore_outcome));
    /// otherwise it is done now. The status bar says how it went.
    pub fn restore_selected(&mut self, version: Version) {
        self.ensure_detail();
        if self.busy() {
            return;
        }
        let (Some(detail), Some(row), Some(history)) =
            (&self.detail, self.selected_file_row(), &self.history)
        else {
            return;
        };
        let (paths, kept) = self.restorable_selection();
        let what = self.describe_file_row(row);
        if paths.is_empty() {
            self.notice = Some(StatusLine::info(format!(
                "Nothing to restore in {what}: a submodule's commit is checked out on its own tab"
            )));
            return;
        }
        let source = match version {
            Version::Commit => Some(detail.id),
            Version::Parent => detail.parents.first().copied(),
        };
        let request = RestoreRequest {
            commit: detail.id,
            version,
            source,
            paths,
            what,
            kept,
        };
        let refs: Vec<&str> = request.paths.iter().map(String::as_str).collect();
        match unstaged_among(history.git_dir(), &refs) {
            Ok(lost) if lost.is_empty() => self.finish_restore(request),
            Ok(lost) => {
                let dialog = ConfirmBox::new(
                    format!("Restore {} {}?", request.what, request.version_words()),
                    format!(
                        "Changes that aren't committed or staged are overwritten in {}. This can't be undone.",
                        name_some(&lost)
                    ),
                    RESTORE,
                );
                self.restore = Some(PendingRestore { dialog, request });
            }
            Err(err) => {
                self.notice = Some(StatusLine::error(format!(
                    "Could not restore {}: {}",
                    request.what,
                    err.message()
                )));
            }
        }
    }

    /// Act on the restore box's answer: go ahead with the restore it
    /// asked about, or drop it.
    fn handle_restore_outcome(&mut self, outcome: ConfirmOutcome) {
        match outcome {
            ConfirmOutcome::Continue => {}
            ConfirmOutcome::Cancel => self.restore = None,
            ConfirmOutcome::Confirm => {
                if let Some(pending) = self.restore.take() {
                    self.finish_restore(pending.request);
                }
            }
        }
    }

    /// Do a restore, and say how it went.
    fn finish_restore(&mut self, request: RestoreRequest) {
        let Some(history) = &self.history else {
            return;
        };
        let refs: Vec<&str> = request.paths.iter().map(String::as_str).collect();
        self.notice = Some(match restore(history.git_dir(), request.source, &refs) {
            Ok(done) => StatusLine::info(restored_summary(&request, done)),
            Err(err) => StatusLine::error(format!(
                "Could not restore {}: {}",
                request.what,
                err.message()
            )),
        });
    }

    /// Whether the restore box is asking.
    #[cfg(test)]
    fn restore_box(&self) -> Option<&ConfirmBox> {
        self.restore.as_ref().map(|pending| &pending.dialog)
    }

    // ----- Branches in the sidebar ---------------------------------------

    /// The branch selected in the sidebar, if a branch (not a remote)
    /// is.
    fn selected_branch(&self) -> Option<SideBranch> {
        let history = self.history.as_ref()?;
        match self.side_rows.get(self.side_selected)? {
            SideRow::Branch(b) => {
                let branch = history.branches().get(*b)?;
                Some(SideBranch {
                    name: branch.name.clone(),
                    remote: false,
                    target: branch.target,
                    is_head: branch.is_head,
                })
            }
            SideRow::RemoteBranch(r, b) => {
                let remote = history.remotes().get(*r)?;
                let branch = remote.branches.get(*b)?;
                Some(SideBranch {
                    name: format!("{}/{}", remote.name, branch.name),
                    remote: true,
                    target: branch.target,
                    is_head: false,
                })
            }
            _ => None,
        }
    }

    /// Whether the keyboard is in the sidebar with a branch selected
    /// there, for "Check out branch".
    pub fn can_checkout_branch(&self) -> bool {
        self.pane == Pane::Sidebar && self.selected_branch().is_some()
    }

    /// Whether the keyboard is in the sidebar with a local branch HEAD
    /// isn't on selected there, for "Delete branch".
    pub fn can_delete_branch(&self) -> bool {
        self.pane == Pane::Sidebar
            && self
                .selected_branch()
                .is_some_and(|branch| !branch.remote && !branch.is_head)
    }

    /// "Delete branch": delete the local branch selected in the
    /// sidebar (see the core crate's `git::branch` module). One whose
    /// commits HEAD or its upstream has is deleted at once; one with
    /// commits of its own, which would be left on no branch, has the
    /// delete branch box ask first. The status bar says how it went,
    /// and the page refreshes.
    pub fn delete_selected_branch(&mut self) {
        if self.busy() {
            return;
        }
        let Some(branch) = self.selected_branch() else {
            return;
        };
        if branch.remote {
            return;
        }
        if branch.is_head {
            self.notice = Some(StatusLine::error(format!(
                "Could not delete {}: it is checked out",
                branch.name
            )));
            return;
        }
        self.delete_branch_named(branch.name, false);
    }

    /// Delete a local branch, forced or not, and say how it went; or,
    /// when unforced and it has commits of its own, open the box asking
    /// whether to force it.
    fn delete_branch_named(&mut self, name: String, force: bool) {
        let Some(history) = &self.history else {
            return;
        };
        match delete_branch(history.git_dir(), &name, force) {
            Ok(id) => {
                self.notice = Some(StatusLine::info(format!(
                    "Deleted branch {name} (was {})",
                    short_id(id)
                )));
                self.refresh();
            }
            Err(DeleteBranchError::Unmerged { commits, .. }) => {
                let noun = if commits == 1 { "commit" } else { "commits" };
                let dialog = ConfirmBox::new(
                    format!("Delete branch {name}?"),
                    format!(
                        "{name} has {commits} {noun} that neither HEAD nor its upstream has, which would be left on no branch. This can't be undone."
                    ),
                    DELETE,
                );
                self.branch_delete = Some(PendingDelete { dialog, name });
            }
            Err(err) => {
                self.notice = Some(StatusLine::error(format!("Could not delete {name}: {err}")));
            }
        }
    }

    /// Act on the delete branch box's answer: delete the branch it
    /// asked about, forced, or drop it.
    fn handle_delete_outcome(&mut self, outcome: ConfirmOutcome) {
        match outcome {
            ConfirmOutcome::Continue => {}
            ConfirmOutcome::Cancel => self.branch_delete = None,
            ConfirmOutcome::Confirm => {
                if let Some(pending) = self.branch_delete.take() {
                    self.delete_branch_named(pending.name, true);
                }
            }
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
        self.content_pane.reset();
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
                        Ok(diff) => Content::Diff(Box::new(DiffModel::new(diff, TAB_WIDTH))),
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
        lane_cap(self.log_area.width)
    }

    /// The columns the log's visible commits reach, from the first
    /// column after its left edge to the end of the longest of their
    /// lines: `rows` rows from row `scroll`, two a commit, counting a
    /// commit with one line showing at either end.
    fn log_extent(&self, scroll: usize, rows: usize) -> usize {
        let Some(history) = &self.history else {
            return 0;
        };
        let cap = self.lane_cap();
        let head = history.head();
        history
            .commits()
            .iter()
            .skip(scroll / 2)
            .take((scroll + rows).div_ceil(2) - scroll / 2)
            .map(|commit| {
                let lanes = commit.graph.width().clamp(1, cap);
                commit_extent(commit, Highlight::head_if(head == Some(commit.id)), lanes)
            })
            .max()
            .unwrap_or(0)
    }

    /// The lines of text the content pane shows, when it shows text
    /// rather than a diff; see [`shown`].
    fn content_lines(&self) -> Option<Vec<Piece>> {
        match &self.content {
            Some(Content::Description) => Some(self.description_lines()),
            Some(Content::Directory(dir)) => Some(self.directory_lines(*dir)),
            _ => None,
        }
    }

    /// Scroll the log sideways by `columns`, within the editor's limits.
    fn scroll_log_sideways(&mut self, columns: isize) {
        let extent = self.log_extent(self.log_scroll, self.log_rows);
        self.log_h.scroll_by(columns, extent);
    }

    // ----- Input ----------------------------------------------------------

    // ----- Merging, rebasing, and resetting -------------------------------

    /// Whether a checkout, merge, or rebase is under way, beside which
    /// nothing else that changes the repository starts; the status bar
    /// says so.
    fn busy(&mut self) -> bool {
        let what = if self.checkout.is_some() {
            "A checkout"
        } else if let Some(running) = &self.integration {
            match running.what {
                Integration::Merge(_) => "A merge",
                Integration::Rebase(_) => "A rebase",
            }
        } else {
            return false;
        };
        self.notice = Some(StatusLine::error(format!("{what} is under way")));
        true
    }

    /// The selected commit's id, unless HEAD is at it.
    fn other_selected_id(&self) -> Option<Oid> {
        let id = self.selected_id()?;
        let head = self.history.as_ref()?.head()?;
        (id != head).then_some(id)
    }

    /// Whether the selected commit is one HEAD isn't at, for "Rebase
    /// current branch onto commit" and "Reset current branch to
    /// commit".
    pub fn other_commit_selected(&self) -> bool {
        self.other_selected_id().is_some()
    }

    /// The branch to merge for the selected commit, unless HEAD is at
    /// it: a local branch there other than HEAD's, or failing that a
    /// remote's, the first as the log lists them.
    fn branch_at_selected(&self) -> Option<Target> {
        self.other_selected_id()?;
        let commit = self.history.as_ref()?.commits().get(self.selected)?;
        let local = commit
            .refs
            .iter()
            .find(|label| label.kind == RefKind::Branch && !label.is_head);
        let label = local.or_else(|| {
            commit
                .refs
                .iter()
                .find(|label| label.kind == RefKind::RemoteBranch)
        })?;
        Some(Target::Branch {
            name: label.name.clone(),
            remote: label.kind == RefKind::RemoteBranch,
        })
    }

    /// Whether the selected commit has a branch to merge, for "Merge
    /// into current branch".
    pub fn can_merge_commit(&self) -> bool {
        self.branch_at_selected().is_some()
    }

    /// The branch selected in the sidebar, with the keyboard there,
    /// unless HEAD is on it or at its commit.
    fn other_selected_branch(&self) -> Option<SideBranch> {
        if self.pane != Pane::Sidebar {
            return None;
        }
        let branch = self.selected_branch()?;
        let head = self.history.as_ref()?.head()?;
        (!branch.is_head && branch.target != head).then_some(branch)
    }

    /// Whether the keyboard is in the sidebar with a branch selected
    /// there that HEAD isn't on or at, for merging it, rebasing onto
    /// it, or resetting to it.
    pub fn other_branch_selected(&self) -> bool {
        self.other_selected_branch().is_some()
    }

    /// "Merge into current branch": merge the branch at the selected
    /// commit into HEAD's (see [`start_integration`](Self::start_integration)).
    pub fn merge_selected_commit(&mut self) {
        if let Some(target) = self.branch_at_selected() {
            self.start_integration(Integration::Merge(target));
        }
    }

    /// "Rebase current branch onto commit": rebase HEAD's branch onto
    /// the selected commit.
    pub fn rebase_onto_selected_commit(&mut self) {
        if let Some(id) = self.other_selected_id() {
            self.start_integration(Integration::Rebase(Target::Commit(id)));
        }
    }

    /// "Reset current branch to commit": reset HEAD's branch to the
    /// selected commit (see [`request_reset`](Self::request_reset)).
    pub fn reset_to_selected_commit(&mut self) {
        if let Some(id) = self.other_selected_id() {
            self.request_reset(id, short_id(id));
        }
    }

    /// "Merge branch into current branch": merge the branch selected in
    /// the sidebar into HEAD's.
    pub fn merge_selected_branch(&mut self) {
        if let Some(branch) = self.other_selected_branch() {
            self.start_integration(Integration::Merge(Target::Branch {
                name: branch.name,
                remote: branch.remote,
            }));
        }
    }

    /// "Rebase current branch onto branch": rebase HEAD's branch onto
    /// the branch selected in the sidebar.
    pub fn rebase_onto_selected_branch(&mut self) {
        if let Some(branch) = self.other_selected_branch() {
            self.start_integration(Integration::Rebase(Target::Branch {
                name: branch.name,
                remote: branch.remote,
            }));
        }
    }

    /// "Reset current branch to branch": reset HEAD's branch to where
    /// the branch selected in the sidebar is.
    pub fn reset_to_selected_branch(&mut self) {
        if let Some(branch) = self.other_selected_branch() {
            self.request_reset(branch.target, branch.name);
        }
    }

    /// Start merging or rebasing in the background (see the core
    /// crate's `git::operation` module), unless something else is under
    /// way. The status bar says how it is going, and then how it went.
    fn start_integration(&mut self, what: Integration) {
        if self.busy() {
            return;
        }
        let Some(history) = &self.history else {
            return;
        };
        match IntegrationJob::start(history.git_dir(), what.clone()) {
            Ok(job) => self.integration = Some(RunningIntegration { what, job }),
            Err(err) => {
                self.notice = Some(StatusLine::error(format!(
                    "Could not {}: {err}",
                    integration_words(&what)
                )));
            }
        }
    }

    /// Take in what the merge or rebase has reported, and once it is
    /// done say how it went and read the repository again so the log
    /// shows where HEAD is now. One that stopped at conflicts asks for
    /// the changes page, where they are resolved (see
    /// [`take_show_changes`](Self::take_show_changes)). Returns
    /// whether anything changed.
    fn poll_integration(&mut self) -> bool {
        let Some(running) = &mut self.integration else {
            return false;
        };
        let changed = running.job.poll();
        if !running.job.is_done() {
            return changed;
        }
        let Some(RunningIntegration { what, job }) = self.integration.take() else {
            return changed;
        };
        let head = self
            .history
            .as_ref()
            .and_then(History::head_branch)
            .map(str::to_owned);
        let moved = match job.outcome() {
            Some(Ok(outcome)) => {
                self.notice = Some(StatusLine::info(what.summary(head.as_deref(), &outcome)));
                if matches!(outcome, Outcome::Conflicts { .. }) {
                    self.show_changes = true;
                }
                outcome != Outcome::UpToDate
            }
            // Left in progress part way: the changes page is where it
            // is continued or aborted.
            Some(Err(err @ OperationError::Stopped(_))) => {
                let doing = match &what {
                    Integration::Merge(target) => format!("Merging {}", target.name()),
                    Integration::Rebase(target) => format!("Rebasing onto {}", target.name()),
                };
                self.notice = Some(StatusLine::error(format!("{doing} {err}")));
                self.show_changes = true;
                true
            }
            Some(Err(err @ OperationError::SubmodulesBehind(_))) => {
                let done = match &what {
                    Integration::Merge(target) => format!("Merged {}", target.name()),
                    Integration::Rebase(target) => format!("Rebased onto {}", target.name()),
                };
                self.notice = Some(StatusLine::error(format!("{done}: {err}")));
                true
            }
            Some(Err(err)) => {
                self.notice = Some(StatusLine::error(format!(
                    "Could not {}: {err}",
                    integration_words(&what)
                )));
                false
            }
            None => {
                self.notice = Some(StatusLine::error(format!(
                    "Could not {}: it stopped",
                    integration_words(&what)
                )));
                false
            }
        };
        if moved {
            self.checked_out = true;
            self.refresh();
        }
        true
    }

    /// Whether a merge or rebase stopped at conflicts since the last
    /// time this was asked, for the changes page to be shown, where
    /// they are resolved.
    fn take_show_changes(&mut self) -> bool {
        std::mem::take(&mut self.show_changes)
    }

    /// Reset HEAD's branch to `id` (called `what` in the status bar),
    /// keeping the working directory (see the core crate's
    /// `git::reset` module): at once if every commit it leaves behind
    /// is on another branch or tag, otherwise once the reset box has
    /// asked.
    fn request_reset(&mut self, id: Oid, what: String) {
        if self.busy() {
            return;
        }
        let Some(history) = &self.history else {
            return;
        };
        match left_behind(history.git_dir(), id) {
            Ok(0) => self.finish_reset(id, what),
            Ok(commits) => {
                let head = history.head_branch().unwrap_or("HEAD");
                let noun = if commits == 1 { "commit" } else { "commits" };
                let dialog = ConfirmBox::new(
                    format!("Reset {head} to {what}?"),
                    format!(
                        "{commits} {noun} of {head} that no other branch or tag has would be left on no branch (git's reflog keeps them for a while). The working directory stays as it is."
                    ),
                    RESET,
                );
                self.reset_box = Some(PendingReset { dialog, id, what });
            }
            Err(err) => {
                self.notice = Some(StatusLine::error(format!(
                    "Could not reset to {what}: {err}"
                )));
            }
        }
    }

    /// Act on the reset box's answer: reset as it asked, or drop it.
    fn handle_reset_outcome(&mut self, outcome: ConfirmOutcome) {
        match outcome {
            ConfirmOutcome::Continue => {}
            ConfirmOutcome::Cancel => self.reset_box = None,
            ConfirmOutcome::Confirm => {
                if let Some(pending) = self.reset_box.take() {
                    self.finish_reset(pending.id, pending.what);
                }
            }
        }
    }

    /// Do a reset, say how it went, and read the repository again.
    fn finish_reset(&mut self, id: Oid, what: String) {
        let Some(history) = &self.history else {
            return;
        };
        let head = history.head_branch().unwrap_or("HEAD").to_owned();
        match reset(history.git_dir(), id) {
            Ok(()) => {
                self.notice = Some(StatusLine::info(format!(
                    "Reset {head} to {what}; the working directory is unchanged"
                )));
                self.checked_out = true;
                self.refresh();
            }
            Err(err) => {
                self.notice = Some(StatusLine::error(format!(
                    "Could not reset to {what}: {err}"
                )));
            }
        }
    }

    // ----- Checking out --------------------------------------------------

    /// Space or a double-click in the log: check the selected commit
    /// out (see the core crate's `git::checkout` module), or open the
    /// box asking for a new branch's name when one is wanted. The
    /// status bar says what was done, or why it couldn't be.
    pub fn checkout_selected(&mut self) {
        if self.busy() {
            return;
        }
        let Some(id) = self.selected_id() else {
            return;
        };
        let Some(history) = &self.history else {
            return;
        };
        let job = CheckoutJob::start(history.git_dir(), id);
        self.start_checkout(short_id(id), id, job);
    }

    /// "Check out branch": check out the branch selected in the
    /// sidebar (see the core crate's `git::checkout` module): a local
    /// branch as it is, a remote's on the local branch tracking it,
    /// fast-forwarded to it, or on a new one made to track it, with
    /// the box asking for a name if its own is taken. The status bar
    /// says what was done, or why it couldn't be.
    pub fn checkout_selected_branch(&mut self) {
        if self.busy() {
            return;
        }
        let (Some(branch), Some(history)) = (self.selected_branch(), &self.history) else {
            return;
        };
        let job = CheckoutJob::start_branch(history.git_dir(), &branch.name, branch.remote);
        self.start_checkout(branch.name, branch.target, job);
    }

    /// Keep a checkout job just started, of `what` (for the status
    /// bar) at `id`, or say why it couldn't be.
    fn start_checkout(&mut self, what: String, id: Oid, job: Result<CheckoutJob, CheckoutError>) {
        match job {
            Ok(job) => self.checkout = Some(RunningCheckout { what, id, job }),
            Err(err) => {
                self.notice = Some(StatusLine::error(format!("Could not check out: {err}")))
            }
        }
    }

    /// Take in what the checkout has reported, and once it is done act
    /// on how it went: say so and read the repository again so the
    /// HEAD marker moves; open the box asking for a branch's name; or
    /// say why it couldn't be done (in the box, when it is the name
    /// that won't do). Returns whether anything changed.
    fn poll_checkout(&mut self) -> bool {
        let Some(RunningCheckout { job, .. }) = &mut self.checkout else {
            return false;
        };
        let changed = job.poll();
        if !job.is_done() {
            return changed;
        }
        let Some(RunningCheckout { id, job, .. }) = self.checkout.take() else {
            return changed;
        };
        // A branch's commit as the job found it, which may have moved
        // since the page was read.
        let id = job.commit().unwrap_or(id);
        match job.outcome() {
            Some(CheckoutOutcome::Done { how, submodules }) => {
                let already = self.history.as_ref().is_some_and(|history| {
                    history.head() == Some(id)
                        && matches!(&how, Checkout::Branch(name) if history.head_branch() == Some(name))
                });
                self.notice = Some(StatusLine::info(match &how {
                    Checkout::Branch(name) if already && submodules == 0 => {
                        format!("Already on {name}")
                    }
                    _ => how.summary(id, submodules),
                }));
                self.branch_prompt = None;
                self.checked_out = true;
                self.refresh();
            }
            Some(CheckoutOutcome::NeedsName { upstream, taken }) => {
                self.branch_prompt = Some(BranchPrompt::new(id, upstream, taken));
            }
            Some(CheckoutOutcome::Failed(
                err @ (CheckoutError::InvalidName(_) | CheckoutError::NameTaken(_)),
            )) if self.branch_prompt.is_some() => {
                if let Some(prompt) = &mut self.branch_prompt {
                    prompt.refuse(err.to_string());
                }
            }
            Some(CheckoutOutcome::Failed(err)) => {
                self.branch_prompt = None;
                self.notice = Some(StatusLine::error(format!("Could not check out: {err}")));
            }
            None => {
                self.notice = Some(StatusLine::error(
                    "Could not check out: the checkout stopped",
                ));
            }
        }
        true
    }

    /// Whether a checkout has been done since the last time this was
    /// asked: the submodules may have moved with it, so their tabs are
    /// due a refresh.
    fn take_checked_out(&mut self) -> bool {
        std::mem::take(&mut self.checked_out)
    }

    /// Do what the branch name box asks: start making the branch by
    /// the name given and checking the commit out on it. The box stays
    /// open until that is done, so that a name that won't do can be
    /// refused in it; Enter again meanwhile does nothing.
    fn handle_prompt_outcome(&mut self, outcome: BranchPromptOutcome) {
        match outcome {
            BranchPromptOutcome::Continue => {}
            BranchPromptOutcome::Close => self.branch_prompt = None,
            BranchPromptOutcome::Accept(name) => {
                if self.checkout.is_some() || self.integration.is_some() {
                    return;
                }
                let (Some(prompt), Some(history)) = (&self.branch_prompt, &self.history) else {
                    return;
                };
                let (id, upstream) = (prompt.id, prompt.upstream.clone());
                let what = name.clone();
                let how = Checkout::NewBranch { name, upstream };
                let job = CheckoutJob::start_with(history.git_dir(), id, how);
                if job.is_err() {
                    self.branch_prompt = None;
                }
                self.start_checkout(what, id, job);
            }
        }
    }

    /// Add pasted text to the branch name box, if open; nothing else
    /// on the page takes text.
    pub fn paste(&mut self, text: &str) {
        if let Some(prompt) = &mut self.branch_prompt {
            prompt.paste(text);
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent, clipboard: &mut Clipboard) {
        if let Some(prompt) = &mut self.branch_prompt {
            let outcome = prompt.handle_key(key, clipboard);
            self.handle_prompt_outcome(outcome);
            return;
        }
        // The restore box, while it asks, has every key.
        if let Some(pending) = &mut self.restore {
            let outcome = pending.dialog.handle_key(key);
            self.handle_restore_outcome(outcome);
            return;
        }
        // So has the delete branch box, and the reset box.
        if let Some(pending) = &mut self.branch_delete {
            let outcome = pending.dialog.handle_key(key);
            self.handle_delete_outcome(outcome);
            return;
        }
        if let Some(pending) = &mut self.reset_box {
            let outcome = pending.dialog.handle_key(key);
            self.handle_reset_outcome(outcome);
            return;
        }
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
            KeyCode::F(5) => {
                self.fetch();
                return;
            }
            KeyCode::Char('o') if self.pane == Pane::Files => {
                self.open_selected_change();
                return;
            }
            KeyCode::Char('o') if self.pane == Pane::Content => {
                self.open_change_at_cursor();
                return;
            }
            _ => {}
        }
        match self.pane {
            Pane::Sidebar => self.handle_sidebar_key(key),
            Pane::Log => self.handle_log_key(key),
            Pane::Files => self.handle_files_key(key),
            Pane::Content => self.handle_content_key(key, clipboard),
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
            KeyCode::Right => {
                if let Some(SideRow::Remote(r)) = selected {
                    self.set_collapsed(r, false);
                }
            }
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
            KeyCode::Char(' ') => {
                self.checkout_selected();
                None
            }
            KeyCode::Right => {
                self.scroll_log_sideways(ARROW_COLUMNS as isize);
                None
            }
            KeyCode::Left => {
                self.scroll_log_sideways(-(ARROW_COLUMNS as isize));
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
                Some(TreeRow::File { .. }) if key.code == KeyCode::Enter => {
                    self.open_selected_file();
                }
                None if key.code == KeyCode::Enter => self.pane = Pane::Content,
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
                _ => {}
            },
            KeyCode::Left => match row {
                Some(TreeRow::Dir {
                    dir,
                    collapsed: false,
                    ..
                }) => self.set_dir_collapsed(dir, true),
                Some(TreeRow::Dir { dir, .. }) => {
                    if let Some(parent) = self.file_tree.dirs()[dir].parent {
                        self.select_dir(parent);
                    }
                }
                Some(TreeRow::File {
                    parent: Some(parent),
                    ..
                }) => self.select_dir(parent),
                _ => {}
            },
            _ => {}
        }
    }

    fn handle_content_key(&mut self, key: KeyEvent, clipboard: &mut Clipboard) {
        let lines = self.content_lines();
        let content = shown_mut(self.content.as_mut(), lines.as_deref());
        if let Some(notice) = self.content_pane.handle_key(key, content, clipboard) {
            self.notice = Some(notice);
        }
    }

    // ----- The diff's selection ---------------------------------------------

    /// Whether the content pane shows a diff with lines, for the cursor
    /// to move through and select in.
    pub fn shows_diff_lines(&self) -> bool {
        matches!(&self.content, Some(Content::Diff(model)) if !model.rows().is_empty())
    }

    /// What is selected on the page, for a search to start from: the
    /// text selected in the diff shown.
    pub fn selected_text(&self) -> Option<String> {
        match &self.content {
            Some(Content::Diff(model)) => model.selected_text(),
            _ => None,
        }
    }

    /// Whether anything is selected in the diff shown.
    pub fn has_diff_selection(&self) -> bool {
        matches!(&self.content, Some(Content::Diff(model)) if model.selection().is_some())
    }

    /// "Copy", "Copy as patch", and "Copy new side": put the diff's
    /// selection on the clipboard, copied `how`. Returns what to say
    /// about it, nothing without a selection.
    pub fn copy_diff_selection(
        &self,
        clipboard: &mut Clipboard,
        how: CopyAs,
    ) -> Option<StatusLine> {
        let lines = self.content_lines();
        diff_pane::copy(
            shown(self.content.as_ref(), lines.as_deref()),
            clipboard,
            how,
        )
    }

    /// Whether the diff's selection has changes in it, to copy as a
    /// patch.
    pub fn has_diff_selection_changes(&self) -> bool {
        matches!(&self.content, Some(Content::Diff(model))
            if model.selection().is_some()
                && model.diff().has_changes_among(&model.selected_lines()))
    }

    /// "Select all": select the whole diff shown, and give the keyboard
    /// to it.
    pub fn select_all_in_diff(&mut self) {
        if let Some(Content::Diff(model)) = &mut self.content
            && !model.rows().is_empty()
        {
            model.select_all();
            self.pane = Pane::Content;
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
        self.log_h.dragging || self.content_pane.is_dragging() || self.divider_drag.is_some()
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

    /// Handle a mouse event. `wheel` is how many rows (or columns,
    /// sideways) a wheel event scrolls.
    pub fn handle_mouse(&mut self, mouse: MouseEvent, wheel: usize) -> GitLogMouseOutcome {
        let mut outcome = GitLogMouseOutcome::default();
        // The branch name box, while open, takes the mouse over it, and
        // a press anywhere else closes it, as the go to line box does.
        if let Some(prompt) = &mut self.branch_prompt {
            if prompt.contains(mouse.column, mouse.row) || prompt.is_dragging() {
                prompt.handle_mouse(mouse);
            } else if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.branch_prompt = None;
            }
            return outcome;
        }
        // The restore box, while it asks, has the mouse; a press outside
        // it is no.
        if let Some(pending) = &mut self.restore {
            let answer = pending.dialog.handle_mouse(mouse);
            self.handle_restore_outcome(answer);
            return outcome;
        }
        if let Some(pending) = &mut self.branch_delete {
            let answer = pending.dialog.handle_mouse(mouse);
            self.handle_delete_outcome(answer);
            return outcome;
        }
        if let Some(pending) = &mut self.reset_box {
            let answer = pending.dialog.handle_mouse(mouse);
            self.handle_reset_outcome(answer);
            return outcome;
        }
        self.ensure_detail();
        let at = ScreenPosition::new(mouse.column, mouse.row);
        // A rule being dragged owns the mouse until the button comes up.
        if let Some(divider) = self.divider_drag {
            match mouse.kind {
                MouseEventKind::Drag(_) => self.drag_divider(divider, mouse.column, mouse.row),
                MouseEventKind::Up(_) => {
                    self.divider_drag = None;
                    outcome.resized = true;
                }
                _ => {}
            }
            return outcome;
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
                return outcome;
            }
        }
        // A scrollbar drag owns the mouse likewise.
        if self.is_dragging() {
            match mouse.kind {
                MouseEventKind::Drag(_) if self.log_h.dragging => {
                    let extent = self.log_extent(self.log_scroll, self.log_rows);
                    self.log_h.scroll_to(mouse.column, extent);
                }
                MouseEventKind::Up(_) if self.log_h.dragging => self.log_h.dragging = false,
                MouseEventKind::Drag(_) | MouseEventKind::Up(_) => {
                    let lines = self.content_lines();
                    let content = shown_mut(self.content.as_mut(), lines.as_deref());
                    self.content_pane.handle_mouse(mouse, wheel, content);
                }
                _ => {}
            }
            return outcome;
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
            return outcome;
        };
        // The content pane scrolls itself, and moves a diff's cursor
        // and selects in it; a right press in a diff with lines asks for
        // the menu of what can be done with the selection.
        if pane == Pane::Content {
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.pane = Pane::Content;
            }
            let lines = self.content_lines();
            let content = shown_mut(self.content.as_mut(), lines.as_deref());
            self.content_pane.handle_mouse(mouse, wheel, content);
            if mouse.kind == MouseEventKind::Down(MouseButton::Right) && self.shows_diff_lines() {
                outcome.menu = Some(GitLogMenu::Diff);
            }
            return outcome;
        }
        match mouse.kind {
            // A right press on a commit in the log selects it, and asks
            // for the menu of what can be done with it.
            MouseEventKind::Down(MouseButton::Right) if pane == Pane::Log => {
                let index = (self.log_scroll + (mouse.row - self.log_area.y) as usize) / 2;
                if index < self.commit_count() {
                    self.pane = Pane::Log;
                    self.head_shown = true;
                    self.pending_jump = None;
                    self.select_commit(index);
                    outcome.menu = Some(GitLogMenu::Commit);
                }
            }
            // A right press on a file or directory of the commit selects
            // it, without folding a directory, and asks for the menu of
            // what can be done to it.
            MouseEventKind::Down(MouseButton::Right) if pane == Pane::Files => {
                let row = self.files_scroll + (mouse.row - self.files_area.y) as usize;
                if row > 0 && row < self.file_row_count() {
                    self.pane = Pane::Files;
                    self.select_file(row);
                    outcome.menu = Some(GitLogMenu::Files);
                }
            }
            // A right press on a branch in the sidebar selects it,
            // without going to its commit, and asks for the menu of
            // what can be done to it.
            MouseEventKind::Down(MouseButton::Right) if pane == Pane::Sidebar => {
                let row = self.side_scroll + (mouse.row - self.sidebar_area.y) as usize;
                if matches!(
                    self.side_rows.get(row),
                    Some(SideRow::Branch(_) | SideRow::RemoteBranch(..))
                ) {
                    self.pane = Pane::Sidebar;
                    self.side_selected = row;
                    outcome.menu = Some(GitLogMenu::Branch);
                }
            }
            MouseEventKind::ScrollUp => self.scroll_pane(pane, -(wheel as isize)),
            MouseEventKind::ScrollDown => self.scroll_pane(pane, wheel as isize),
            MouseEventKind::ScrollRight | MouseEventKind::ScrollLeft => {
                let columns = if mouse.kind == MouseEventKind::ScrollRight {
                    wheel as isize
                } else {
                    -(wheel as isize)
                };
                if pane == Pane::Log {
                    self.scroll_log_sideways(columns);
                }
            }
            MouseEventKind::Down(MouseButton::Left) if self.log_h.bar.contains(at) => {
                self.pane = Pane::Log;
                self.log_h.dragging = true;
                let extent = self.log_extent(self.log_scroll, self.log_rows);
                self.log_h.scroll_to(mouse.column, extent);
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
                        let index = (self.log_scroll + (mouse.row - self.log_area.y) as usize) / 2;
                        let presses = self.clicks.press(mouse.column, mouse.row);
                        if index < self.commit_count() {
                            self.head_shown = true;
                            self.pending_jump = None;
                            self.select_commit(index);
                            // A commit is checked out on a double-click,
                            // as on Space.
                            if presses == 2 {
                                self.checkout_selected();
                            }
                        }
                    }
                    Pane::Files => {
                        let row = self.files_scroll + (mouse.row - self.files_area.y) as usize;
                        let presses = self.clicks.press(mouse.column, mouse.row);
                        if row < self.file_row_count() {
                            self.select_file(row);
                            match self.selected_file_row() {
                                // A directory folds or unfolds when clicked.
                                Some(TreeRow::Dir { dir, collapsed, .. }) => {
                                    self.set_dir_collapsed(dir, !collapsed);
                                }
                                // A file opens on a double-click, as on Enter.
                                Some(TreeRow::File { .. }) if presses == 2 => {
                                    self.open_selected_file();
                                }
                                _ => {}
                            }
                        }
                    }
                    Pane::Content => {}
                }
            }
            _ => {}
        }
        outcome
    }

    /// Scroll a pane by some rows, down for positive.
    fn scroll_pane(&mut self, pane: Pane, rows: isize) {
        let step = |value: usize| value.saturating_add_signed(rows);
        match pane {
            Pane::Sidebar => self.side_scroll = step(self.side_scroll),
            Pane::Log => self.log_scroll = step(self.log_scroll),
            Pane::Files => self.files_scroll = step(self.files_scroll),
            Pane::Content => self.content_pane.scroll_by(rows),
        }
    }

    // ----- Rendering ------------------------------------------------------

    /// Draw the page into `area`. Returns where the terminal cursor
    /// belongs: in the branch name box while it is open, on a diff's
    /// cursor while the diff has the keyboard and no box is open over
    /// it, and nowhere otherwise.
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) -> Option<ScreenPosition> {
        let mut cursor = self.render_panes(area, buf, theme);
        if let Some(pending) = &mut self.restore {
            pending.dialog.render(area, buf, theme);
            cursor = None;
        }
        if let Some(pending) = &mut self.branch_delete {
            pending.dialog.render(area, buf, theme);
            cursor = None;
        }
        if let Some(pending) = &mut self.reset_box {
            pending.dialog.render(area, buf, theme);
            cursor = None;
        }
        match &mut self.branch_prompt {
            Some(prompt) => prompt.render(area, buf, theme),
            None => cursor,
        }
    }

    /// Draw the panes. Returns where the terminal cursor goes: see
    /// [`render_content`](Self::render_content).
    fn render_panes(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) -> Option<ScreenPosition> {
        self.area = area;
        let background = palette_background(theme);
        buf.set_style(area, background);
        if area.height == 0 || area.width < 8 {
            self.sidebar_area = Rect::default();
            self.log_area = Rect::default();
            self.files_area = Rect::default();
            self.content_area = Rect::default();
            return None;
        }
        if self.history.is_none() {
            let message = self.no_history_message();
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
            return self.render_content(buf, theme);
        }
        None
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
        // Two rows a commit, scrolled a row at a time, so either end
        // of the log can show one line of a commit, as a scroll area
        // would. Revealing the selected commit puts the top back on a
        // commit, so the keys move whole commits, and the end of the
        // log is reached the same way: the last commit whole at the
        // top of its pair of rows, an odd last row left blank. The
        // sideways scrollbar, when needed, takes the last row, which
        // can change which commits are visible and so whether it is
        // needed: lay out again with the row taken, and settle there.
        let full_height = area.height as usize;
        let capacity = area.width as usize - 1;
        let mut show_bar = false;
        let mut rows;
        let mut visible;
        let mut extent;
        loop {
            rows = full_height - usize::from(show_bar);
            visible = (rows / 2).max(1);
            let last = commits.len().saturating_sub(visible);
            self.log_scroll = self.log_scroll.min(last * 2);
            if self.reveal_log {
                let top = self.selected * 2;
                if top < self.log_scroll {
                    self.log_scroll = top;
                } else if top + 2 > self.log_scroll + rows {
                    self.log_scroll = (self.selected + 1 - visible) * 2;
                }
            }
            extent = self.log_extent(self.log_scroll, rows);
            let needed = self.log_h.needs_bar(extent, capacity) && full_height > 2;
            if needed && !show_bar {
                show_bar = true;
                continue;
            }
            break;
        }
        self.reveal_log = false;
        self.log_rows = rows;
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

        // Row by row, none under the scrollbar: each is a line of a
        // commit, the graph in the lanes' colors over the row's
        // background, then line one: the references and the message,
        // or line two: the author, the id, and the time. Both scroll
        // sideways together, the graph staying put.
        for offset in 0..rows {
            let row = self.log_scroll + offset;
            let index = row / 2;
            let Some(commit) = commits.get(index) else {
                break;
            };
            let line = if row.is_multiple_of(2) {
                CommitLine::Node
            } else {
                CommitLine::Transition
            };
            let y = area.y + offset as u16;
            let row_style = if index == self.selected {
                buf.set_style(Rect::new(area.x, y, area.width, 1), selected_style);
                selected_style
            } else {
                background
            };
            let highlight = Highlight::head_if(head == Some(commit.id));
            let lanes = commit.graph.width().clamp(1, max_lanes);
            let rect = Rect::new(area.x + 1, y, area.width - 1, 1);
            draw_commit_line(
                buf, rect, commit, line, highlight, lanes, row_style, theme, scroll_col,
            );
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

    /// Draw the content pane. Returns where the terminal cursor goes
    /// while the pane has the keyboard: on a diff's cursor.
    fn render_content(&mut self, buf: &mut Buffer, theme: &Theme) -> Option<ScreenPosition> {
        let lines = self.content_lines();
        let content = shown(self.content.as_ref(), lines.as_deref());
        let focused = self.pane == Pane::Content;
        self.content_pane
            .render(self.content_area, buf, theme, content, focused)
    }
}

/// A description line for a time: in the user's zone, and in the zone
/// it was recorded in when that reads differently.
/// What a merge or rebase does, in words for "Could not …": `merge
/// feature`, `rebase onto origin/main`.
fn integration_words(what: &Integration) -> String {
    match what {
        Integration::Merge(target) => format!("merge {}", target.name()),
        Integration::Rebase(target) => format!("rebase onto {}", target.name()),
    }
}

fn date_line(label: &str, time: ninjaedit_core::git::CommitTime) -> String {
    let local = time.to_string();
    let own = time.in_own_zone();
    if own == local {
        format!("{label}{local}")
    } else {
        format!("{label}{local}  ({own} where committed)")
    }
}

/// A few paths by name, and how many more there are: `a.rs`, `a.rs
/// and b.rs`, `a.rs, b.rs, c.rs and 2 more`.
fn name_some(paths: &[String]) -> String {
    const NAMED: usize = 3;
    let named = &paths[..paths.len().min(NAMED)];
    let more = paths.len() - named.len();
    match (named, more) {
        ([only], 0) => only.clone(),
        ([first @ .., last], 0) => format!("{} and {last}", first.join(", ")),
        _ => format!("{} and {more} more", named.join(", ")),
    }
}

/// What the status bar says once a restore is done: what was restored
/// and as which version, and anything the version hasn't that was
/// deleted or any submodules left alone.
fn restored_summary(request: &RestoreRequest, done: Restored) -> String {
    let mut text = format!("Restored {} {}", request.what, request.version_words());
    let plural = |count: usize, one: &str, many: &str| {
        format!("{count} {}", if count == 1 { one } else { many })
    };
    let mut details = Vec::new();
    if done.deleted > 0 {
        details.push(format!(
            "{} as that version hasn't it",
            plural(done.deleted, "file deleted,", "files deleted,")
        ));
    }
    if request.kept > 0 {
        details.push(plural(
            request.kept,
            "submodule left alone",
            "submodules left alone",
        ));
    }
    if !details.is_empty() {
        text.push_str(": ");
        text.push_str(&details.join("; "));
    }
    text
}

#[cfg(test)]
mod tests {
    /// The page's notice, as text.
    fn notice_of(view: &mut GitLogView) -> Option<String> {
        view.take_notice().map(|notice| notice.text())
    }

    fn notice_of_tabs(tabs: &mut GitLogTabs) -> Option<String> {
        tabs.take_notice().map(|notice| notice.text())
    }

    use super::*;
    use crate::commit_row::HEAD_NODE;
    use crate::diff_pane::HSCROLL_SLACK;
    use crossterm::event::{KeyEventKind, KeyEventState};
    use git2::BranchType;
    use git2::{Repository, Signature};
    use ninjaedit_core::TokenKind;
    use ninjaedit_core::git::NODE;
    use std::fs;
    use std::time::Duration;

    /// Rows a wheel event scrolls in these tests.
    const WHEEL: usize = 3;

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

    fn ctrl_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
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
        view.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            },
            WHEEL,
        );
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
        view.handle_key(key(KeyCode::End), &mut Clipboard::new());
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
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
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

    /// A repository with a submodule at `sub` added at its first commit
    /// and then bumped by two more: returns the bump and the
    /// submodule's three commits, oldest first.
    fn repo_with_submodule_bump() -> (tempfile::TempDir, Oid, Vec<Oid>) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let sig = Signature::now("Ann Author", "ann@example.com").unwrap();
        let commit_parent = |message: &str| {
            let mut index = repo.index().unwrap();
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let head = repo.head().ok().and_then(|h| h.target());
            let parents: Vec<git2::Commit<'_>> = head
                .into_iter()
                .map(|id| repo.find_commit(id).unwrap())
                .collect();
            let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
            repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &refs)
                .unwrap()
        };
        fs::write(dir.path().join("a.rs"), "fn main() {}\n").unwrap();
        repo.index().unwrap().add_path(Path::new("a.rs")).unwrap();
        commit_parent("Base commit");
        let mut submodule = repo
            .submodule("https://example.com/sub.git", Path::new("sub"), true)
            .unwrap();
        let sub = submodule.open().unwrap();
        let commit_sub = |name: &str, content: &str, message: &str| {
            fs::write(sub.workdir().unwrap().join(name), content).unwrap();
            let mut index = sub.index().unwrap();
            index.add_path(Path::new(name)).unwrap();
            index.write().unwrap();
            let tree = sub.find_tree(index.write_tree().unwrap()).unwrap();
            let head = sub.head().ok().and_then(|h| h.target());
            let parents: Vec<git2::Commit<'_>> = head
                .into_iter()
                .map(|id| sub.find_commit(id).unwrap())
                .collect();
            let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
            sub.commit(Some("HEAD"), &sig, &sig, message, &tree, &refs)
                .unwrap()
        };
        let s1 = commit_sub("inner.txt", "one\n", "Inner commit");
        submodule.add_finalize().unwrap();
        commit_parent("Add submodule");
        let s2 = commit_sub("inner.txt", "two\n", "Inner two");
        let s3 = commit_sub("inner.txt", "three\n", "Inner three");
        repo.index().unwrap().add_path(Path::new("sub")).unwrap();
        let bump = commit_parent("Bump submodule");
        (dir, bump, vec![s1, s2, s3])
    }

    #[test]
    fn a_submodule_change_shows_its_commits_as_a_graph() {
        let (dir, bump, subs) = repo_with_submodule_bump();
        let mut view = view(&dir);
        view.jump_to(bump);
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        let screen = draw(&mut view, 100, 30);
        row_with(&screen, "M sub");
        // The content pane: the heading, then the submodule's commits
        // from the new one down to the old, the new one drawn as HEAD.
        let content_x = view.content_area.x as usize;
        let content_y = view.content_area.y as usize;
        // Short of the vertical scrollbar down the right edge.
        let content_width = view.content_area.width as usize - 1;
        let content = move |row: &str| {
            row.chars()
                .skip(content_x)
                .take(content_width)
                .collect::<String>()
        };
        let heading = content(row_with(&screen, "Submodule sub: "));
        assert_eq!(
            heading.trim(),
            format!(
                "Submodule sub: {} → {}",
                ninjaedit_core::git::short_id(subs[0]),
                ninjaedit_core::git::short_id(subs[2])
            )
        );
        let rows: Vec<String> = screen.iter().map(|r| content(r)).collect();
        let at = |rows: &[String], text: &str| {
            rows.iter()
                .position(|r| r.contains(text))
                .unwrap_or_else(|| panic!("no {text:?} in {rows:#?}"))
        };
        let (three, two, one) = (
            at(&rows, "Inner three"),
            at(&rows, "Inner two"),
            at(&rows, "Inner commit"),
        );
        assert!(three < two && two < one, "{rows:#?}");
        assert_eq!(three, at(&rows, "Submodule sub: ") + 1);
        assert_eq!(at(&rows, "Submodule sub: "), content_y);
        assert!(rows[three].contains(HEAD_NODE), "{rows:#?}");
        // Drawn as HEAD is, but not called HEAD: it is where the
        // submodule now points, not where the user is.
        assert!(!rows[three].contains("HEAD"), "{rows:#?}");
        assert!(
            rows[two].contains(NODE) && !rows[two].contains(HEAD_NODE),
            "{rows:#?}"
        );
        assert!(rows[one].contains(NODE), "{rows:#?}");
        assert!(rows[three + 1].contains("Ann Author"), "{rows:#?}");
        assert!(
            rows[three + 1].contains(&ninjaedit_core::git::short_id(subs[2])),
            "{rows:#?}"
        );
        // The new commit's message is in HEAD's color, bold, as in
        // the log; the old one's is plain.
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        view.render(area, &mut buf, &theme);
        let cell_at = |row: usize, text: &str| {
            let x = column_of(&screen[row], text);
            buf[(x, row as u16)].clone()
        };
        let new = cell_at(three, "Inner three");
        assert_eq!(new.fg, theme.git_head_text);
        assert!(new.modifier.contains(Modifier::BOLD));
        let old = cell_at(one, "Inner commit");
        assert_ne!(old.fg, theme.git_head_text);
        assert!(!old.modifier.contains(Modifier::BOLD));
        // In a pane too small for all of it, the pane scrolls by rows:
        // one down puts the new commit's node line at the top; and
        // sideways, the text moving and the graph staying put.
        view.handle_key(key(KeyCode::Tab), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        let screen = draw(&mut view, 60, 10);
        assert!(view.content_area.height < 7, "{:?}", view.content_area);
        let small_x = view.content_area.x as usize;
        let small_y = view.content_area.y as usize;
        let top = screen[small_y].chars().skip(small_x).collect::<String>();
        assert!(
            top.contains(HEAD_NODE) && top.contains("Inner three"),
            "{screen:#?}"
        );
        view.handle_key(key(KeyCode::Right), &mut Clipboard::new());
        let screen = draw(&mut view, 60, 10);
        let scrolled = screen[small_y].chars().skip(small_x).collect::<String>();
        assert_eq!(column_of(&scrolled, HEAD_NODE), column_of(&top, HEAD_NODE));
        assert_eq!(
            column_of(&scrolled, "Inner three") + ARROW_COLUMNS as u16,
            column_of(&top, "Inner three"),
            "{screen:#?}"
        );

        // The commit that added the submodule shows just the commit
        // it was added at.
        let added = view
            .history
            .as_ref()
            .unwrap()
            .commits()
            .iter()
            .find(|c| c.summary == "Add submodule")
            .unwrap()
            .id;
        view.jump_to(added);
        // Back from the content pane to the files pane, and down past
        // .gitmodules to it.
        view.handle_key(key(KeyCode::BackTab), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        let screen = draw(&mut view, 100, 30);
        let rows: Vec<String> = screen.iter().map(|r| content(r)).collect();
        assert!(rows.iter().any(|r| r.contains("added at ")), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains("Inner commit")), "{rows:#?}");
        assert!(!rows.iter().any(|r| r.contains("Inner two")), "{rows:#?}");
    }

    /// Poll the shown tab until its walk, and any refresh, is done.
    fn wait_tabs(tabs: &mut GitLogTabs) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while tabs.is_loading() && std::time::Instant::now() < deadline {
            tabs.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!tabs.is_loading());
    }

    /// The row of the file list that lists the file at `path` of the
    /// selected commit, as drawn: the description is row 0.
    fn file_row_of(view: &GitLogView, path: &str) -> usize {
        let files = &view.detail.as_ref().unwrap().files;
        view.file_rows
            .iter()
            .position(|row| matches!(row, TreeRow::File { file, .. } if files[*file].path == path))
            .unwrap_or_else(|| panic!("no row for {path}"))
            + 1
    }

    #[test]
    fn enter_or_a_double_click_on_a_submodule_goes_to_its_commit_on_its_tab() {
        let (dir, bump, subs) = repo_with_submodule_bump();
        let mut tabs = GitLogTabs::new(dir.path(), GitLogLayout::default());
        wait_tabs(&mut tabs);
        assert_eq!(tabs.active().selected_commit(), Some(bump));
        // The bump's files are Description and then `sub`: Enter on
        // the submodule shows the submodule's tab, at the commit the
        // bump moved it to, with the keyboard in the log.
        tabs.handle_key(key(KeyCode::Tab), &mut Clipboard::new());
        tabs.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        assert_eq!(
            file_row_of(tabs.active(), "sub"),
            tabs.active().file_selected
        );
        tabs.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        assert_eq!(tabs.active_index(), 1);
        wait_tabs(&mut tabs);
        let view = tabs.active();
        assert_eq!(view.selected_commit(), Some(subs[2]));
        assert_eq!(view.pane, Pane::Log);
        let screen = draw(view, 100, 24);
        assert!(
            screen.iter().any(|r| r.contains("Inner three")),
            "{screen:#?}"
        );
        assert!(notice_of_tabs(&mut tabs).is_none());

        // Back on the main tab, the commit that added the submodule
        // (and `.gitmodules` with it): a double-click on `sub` goes to
        // the commit it was added at.
        tabs.set_active(0);
        tabs.handle_key(key(KeyCode::BackTab), &mut Clipboard::new());
        assert_eq!(tabs.active().pane, Pane::Log);
        tabs.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        let view = tabs.active();
        assert_eq!(
            view.history.as_ref().unwrap().commits()[view.selected].summary,
            "Add submodule"
        );
        draw(view, 100, 24);
        let files = view.files_area;
        let (x, y) = (files.x + 2, files.y + file_row_of(view, "sub") as u16);
        tabs.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            },
            WHEEL,
        );
        // One click only selects it.
        assert_eq!(tabs.active_index(), 0);
        assert_eq!(
            file_row_of(tabs.active(), "sub"),
            tabs.active().file_selected
        );
        tabs.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            },
            WHEEL,
        );
        assert_eq!(tabs.active_index(), 1);
        wait_tabs(&mut tabs);
        assert_eq!(tabs.active().selected_commit(), Some(subs[0]));
        assert_eq!(tabs.active().pane, Pane::Log);

        // A double-click on an ordinary file moves into its diff, as
        // Enter does.
        tabs.set_active(0);
        tabs.handle_key(key(KeyCode::BackTab), &mut Clipboard::new());
        tabs.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        let view = tabs.active();
        assert_eq!(
            view.history.as_ref().unwrap().commits()[view.selected].summary,
            "Base commit"
        );
        draw(view, 100, 24);
        let files = view.files_area;
        let y = files.y + file_row_of(view, "a.rs") as u16;
        for _ in 0..2 {
            tabs.handle_mouse(
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: files.x + 2,
                    row: y,
                    modifiers: KeyModifiers::NONE,
                },
                WHEEL,
            );
        }
        assert_eq!(tabs.active_index(), 0);
        assert_eq!(tabs.active().pane, Pane::Content);

        // The submodule's branch moved back so that the bump's commit
        // is reachable from nothing: once the tab has read the
        // repository again, going to it says it isn't in the log.
        let sub = Repository::open(dir.path().join("sub")).unwrap();
        let branch = sub.head().unwrap().name().unwrap().to_owned();
        sub.reference(&branch, subs[1], true, "back").unwrap();
        tabs.set_active(1);
        tabs.refresh();
        wait_tabs(&mut tabs);
        tabs.set_active(0);
        wait_tabs(&mut tabs);
        tabs.handle_key(key(KeyCode::Tab), &mut Clipboard::new());
        tabs.handle_key(key(KeyCode::Tab), &mut Clipboard::new());
        assert_eq!(tabs.active().pane, Pane::Log);
        tabs.handle_key(key(KeyCode::Home), &mut Clipboard::new());
        tabs.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        tabs.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        tabs.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        assert_eq!(tabs.active_index(), 1);
        wait_tabs(&mut tabs);
        assert_eq!(
            notice_of_tabs(&mut tabs).as_deref(),
            Some(format!("Commit {} is not in the log", short_id(subs[2])).as_str())
        );
        assert_ne!(tabs.active().selected_commit(), Some(subs[2]));
    }

    /// A repository whose last commit changes `line 20` of a forty-line
    /// `f.txt` to `line twenty`, with a page showing that file's diff,
    /// the keyboard in it. Its rows: a gap of 16 lines, lines 17 to 19,
    /// the change, lines 21 to 23, and a gap of 17.
    fn diff_of_a_changed_line() -> (tempfile::TempDir, GitLogView) {
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
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        assert_eq!(view.pane, Pane::Content);
        (dir, view)
    }

    #[test]
    fn the_diff_has_a_cursor_that_moves_selects_and_copies() {
        let (_dir, mut view) = diff_of_a_changed_line();
        let mut clipboard = Clipboard::local_only();
        let theme = Theme::default();
        let area = Rect::new(0, 0, 90, 30);
        // The screen, and where the terminal's cursor is.
        let render = |view: &mut GitLogView| {
            let mut buf = Buffer::empty(area);
            let cursor = view.render(area, &mut buf, &theme);
            (buf, cursor)
        };
        let screen = draw(&mut view, 90, 30);
        let row_of = |text: &str| screen.iter().position(|r| r.contains(text)).unwrap() as u16;
        let (line_17, line_19) = (row_of("line 17"), row_of("line 19"));
        let text_x = column_of(&screen[line_17 as usize], "line 17");
        // The cursor starts on the first row, the gap above the change,
        // on its first button, which is drawn as focused.
        let gap_row = line_17 - 1;
        let up = column_of(&screen[gap_row as usize], "▲");
        let (buf, cursor) = render(&mut view);
        assert_eq!(cursor, Some(ScreenPosition::new(up, gap_row)));
        assert_eq!(
            buf[(up, gap_row)].bg,
            theme.command_palette_selection_background
        );
        // → goes on to its next button, and → again to the next row.
        view.handle_key(key(KeyCode::Right), &mut clipboard);
        let all = column_of(&screen[gap_row as usize], "all");
        let (buf, cursor) = render(&mut view);
        assert_eq!(cursor, Some(ScreenPosition::new(all, gap_row)));
        assert_ne!(
            buf[(up, gap_row)].bg,
            theme.command_palette_selection_background
        );
        view.handle_key(key(KeyCode::Left), &mut clipboard);
        // Down to line 17 and along it, past "line ".
        view.handle_key(key(KeyCode::Down), &mut clipboard);
        for _ in 0..5 {
            view.handle_key(key(KeyCode::Right), &mut clipboard);
        }
        let (_, cursor) = render(&mut view);
        assert_eq!(cursor, Some(ScreenPosition::new(text_x + 5, line_17)));
        // Shift+End selects to the end of the line, drawn in the
        // selection's colors, and the terminal's cursor hides.
        view.handle_key(
            KeyEvent::new(KeyCode::End, KeyModifiers::SHIFT),
            &mut clipboard,
        );
        let (buf, cursor) = render(&mut view);
        assert_eq!(cursor, None);
        assert_eq!(buf[(text_x + 5, line_17)].bg, theme.selection_background);
        assert_eq!(buf[(text_x + 6, line_17)].bg, theme.selection_background);
        assert_ne!(buf[(text_x + 4, line_17)].bg, theme.selection_background);
        // Ctrl+C copies it, and says so.
        view.handle_key(ctrl_key(KeyCode::Char('c')), &mut clipboard);
        assert_eq!(clipboard.get().as_deref(), Some("17"));
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some("Copied 1 line to the clipboard")
        );
        // Down over the removed line: the text as shown, without the
        // diff's markers.
        for _ in 0..3 {
            view.handle_key(
                KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
                &mut clipboard,
            );
        }
        view.handle_key(ctrl_key(KeyCode::Char('c')), &mut clipboard);
        assert_eq!(
            clipboard.get().as_deref(),
            Some("17\nline 18\nline 19\nline 20")
        );
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some("Copied 4 lines to the clipboard")
        );
        // Escape clears the selection, and the cursor shows again.
        view.handle_key(key(KeyCode::Esc), &mut clipboard);
        let (_, cursor) = render(&mut view);
        assert_eq!(cursor, Some(ScreenPosition::new(text_x + 7, line_19 + 1)));
        // Ctrl+End goes to the gap at the end, onto its last button,
        // all; ← goes back to ▼, and Enter presses it: ten lines below
        // the change appear, the cursor still on ▼.
        view.handle_key(ctrl_key(KeyCode::End), &mut clipboard);
        view.handle_key(key(KeyCode::Left), &mut clipboard);
        view.handle_key(key(KeyCode::Enter), &mut clipboard);
        let (_, cursor) = render(&mut view);
        let screen = draw(&mut view, 90, 30);
        let gap_row = screen
            .iter()
            .position(|r| r.contains("7 lines hidden"))
            .unwrap_or_else(|| panic!("{screen:#?}"));
        assert!(screen.iter().any(|r| r.contains("line 33")), "{screen:#?}");
        let down = column_of(&screen[gap_row], "▼");
        assert_eq!(cursor, Some(ScreenPosition::new(down, gap_row as u16)));
        // Again, and the rest appear, the cursor on the first of them.
        view.handle_key(key(KeyCode::Enter), &mut clipboard);
        let screen = draw(&mut view, 90, 30);
        assert!(
            !screen.iter().any(|r| r.contains("7 lines hidden")),
            "{screen:#?}"
        );
        let line_34 = screen.iter().position(|r| r.contains("line 34")).unwrap() as u16;
        let (_, cursor) = render(&mut view);
        assert_eq!(cursor, Some(ScreenPosition::new(text_x, line_34)));
    }

    #[test]
    fn the_diffs_selection_copies_as_a_patch_or_the_new_side() {
        let (_dir, mut view) = diff_of_a_changed_line();
        draw(&mut view, 90, 30);
        let mut clipboard = Clipboard::local_only();
        // Nothing selected: nothing to copy, in any form.
        assert!(!view.has_diff_selection_changes());
        assert_eq!(
            view.copy_diff_selection(&mut clipboard, CopyAs::Patch),
            None
        );
        // Down to line 19, and over the change below it.
        for _ in 0..3 {
            view.handle_key(key(KeyCode::Down), &mut clipboard);
        }
        for _ in 0..3 {
            view.handle_key(
                KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
                &mut clipboard,
            );
        }
        assert!(view.has_diff_selection_changes());
        let notice = view.copy_diff_selection(&mut clipboard, CopyAs::NewSide);
        assert_eq!(clipboard.get().as_deref(), Some("line 19\nline twenty\n"));
        assert_eq!(
            notice.map(|n| n.text()).as_deref(),
            Some("Copied 2 lines to the clipboard")
        );
        let notice = view.copy_diff_selection(&mut clipboard, CopyAs::Patch);
        assert_eq!(
            notice.map(|n| n.text()).as_deref(),
            Some("Copied a patch of f.txt to the clipboard")
        );
        let patch = clipboard.get().unwrap();
        assert!(
            patch.starts_with("diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n"),
            "{patch}"
        );
        assert!(patch.contains("\n-line 20\n+line twenty\n"), "{patch}");
    }

    #[test]
    fn the_mouse_puts_the_cursor_in_the_diff_and_selects() {
        let (_dir, mut view) = diff_of_a_changed_line();
        let screen = draw(&mut view, 90, 30);
        let row_of = |text: &str| screen.iter().position(|r| r.contains(text)).unwrap() as u16;
        let (line_18, twenty, line_22) =
            (row_of("line 18"), row_of("line twenty"), row_of("line 22"));
        let text_x = column_of(&screen[line_18 as usize], "line 18");
        let selected = |view: &GitLogView| match &view.content {
            Some(Content::Diff(model)) => model.selected_text(),
            _ => None,
        };
        let mouse = |view: &mut GitLogView, kind, column, row| {
            view.handle_mouse(
                MouseEvent {
                    kind,
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                },
                WHEEL,
            )
        };
        // A press puts the cursor there, and a drag selects from it.
        mouse(
            &mut view,
            MouseEventKind::Down(MouseButton::Left),
            text_x + 5,
            twenty,
        );
        mouse(
            &mut view,
            MouseEventKind::Drag(MouseButton::Left),
            text_x + 4,
            line_22,
        );
        assert!(view.is_dragging());
        mouse(
            &mut view,
            MouseEventKind::Up(MouseButton::Left),
            text_x + 4,
            line_22,
        );
        assert!(!view.is_dragging());
        assert_eq!(selected(&view).as_deref(), Some("twenty\nline 21\nline"));
        // A double-click selects a word.
        for _ in 0..2 {
            mouse(
                &mut view,
                MouseEventKind::Down(MouseButton::Left),
                text_x + 7,
                twenty,
            );
            mouse(
                &mut view,
                MouseEventKind::Up(MouseButton::Left),
                text_x + 7,
                twenty,
            );
        }
        assert_eq!(selected(&view).as_deref(), Some("twenty"));
        // A press in the line numbers selects the whole line.
        let numbers_x = view.content_area.x + 1;
        mouse(
            &mut view,
            MouseEventKind::Down(MouseButton::Left),
            numbers_x,
            line_18,
        );
        mouse(
            &mut view,
            MouseEventKind::Up(MouseButton::Left),
            numbers_x,
            line_18,
        );
        assert_eq!(selected(&view).as_deref(), Some("line 18\n"));
        // A right press in the selection keeps it, and asks for the menu.
        let outcome = mouse(
            &mut view,
            MouseEventKind::Down(MouseButton::Right),
            text_x + 2,
            line_18,
        );
        assert_eq!(outcome.menu, Some(GitLogMenu::Diff));
        assert_eq!(selected(&view).as_deref(), Some("line 18\n"));
        assert!(view.has_diff_selection());
        let mut clipboard = Clipboard::local_only();
        assert!(
            view.copy_diff_selection(&mut clipboard, CopyAs::Text)
                .is_some()
        );
        assert_eq!(clipboard.get().as_deref(), Some("line 18\n"));
        // Outside it, the press moves the cursor there first.
        mouse(
            &mut view,
            MouseEventKind::Down(MouseButton::Right),
            text_x + 2,
            line_22,
        );
        assert!(!view.has_diff_selection());
    }

    #[test]
    fn gap_buttons_reveal_hidden_lines() {
        let (_dir, mut view) = diff_of_a_changed_line();
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
        // Press ▲ on the first gap: ten lines above the hunk appear, and
        // the cursor goes to the button, for Enter to press it again.
        let x = column_of(&screen[gap], "▲");
        click(&mut view, x, gap as u16);
        let Some(Content::Diff(model)) = &view.content else {
            panic!("the file's diff is shown");
        };
        assert_eq!(
            model.action_at_cursor(),
            Some((0, ninjaedit_core::git::GapAction::Up))
        );
        let screen = draw(&mut view, 90, 30);
        assert!(
            screen.iter().any(|r| r.contains("6 lines hidden")),
            "{screen:#?}"
        );
        assert!(screen.iter().any(|r| r.contains(" line 7")), "{screen:#?}");
        // "all" on the last gap (Ctrl+End takes the cursor down to it)
        // reveals the rest of the file.
        view.handle_key(ctrl_key(KeyCode::End), &mut Clipboard::new());
        let screen = draw(&mut view, 90, 30);
        let last = screen
            .iter()
            .position(|r| r.contains("17 lines hidden"))
            .unwrap_or_else(|| panic!("{screen:#?}"));
        let x = column_of(&screen[last], "all");
        click(&mut view, x, last as u16);
        view.handle_key(ctrl_key(KeyCode::End), &mut Clipboard::new());
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
        view.handle_key(key(KeyCode::BackTab), &mut Clipboard::new());
        assert_eq!(view.pane, Pane::Sidebar);
        // Down to `side`, Enter goes to its commit and to the log.
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        assert_eq!(view.selected_commit(), Some(s));
        assert_eq!(view.pane, Pane::Log);
        // Back in the sidebar, → on a branch stays there; the remote
        // unfolds with → and its branch goes to the merge.
        view.handle_key(key(KeyCode::BackTab), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Right), &mut Clipboard::new());
        assert_eq!(view.pane, Pane::Sidebar);
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        assert_eq!(view.side_rows[view.side_selected], SideRow::Remote(0));
        view.handle_key(key(KeyCode::Right), &mut Clipboard::new());
        let screen = draw(&mut view, 110, 24);
        assert!(screen.iter().any(|r| r.contains("▾ origin")), "{screen:#?}");
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        assert!(matches!(
            view.side_rows[view.side_selected],
            SideRow::RemoteBranch(0, 0)
        ));
        view.handle_key(key(KeyCode::Char(' ')), &mut Clipboard::new());
        assert_eq!(view.selected_commit(), Some(m));
        assert_eq!(view.pane, Pane::Sidebar);
        view.handle_key(key(KeyCode::Left), &mut Clipboard::new());
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
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        assert!(matches!(
            view.selected_file_row(),
            Some(TreeRow::Dir { dir: 0, .. })
        ));
        view.handle_key(key(KeyCode::Left), &mut Clipboard::new());
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
        view.handle_key(key(KeyCode::Right), &mut Clipboard::new());
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
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        assert!(matches!(
            view.selected_file_row(),
            Some(TreeRow::File { file: 1, .. })
        ));
        view.handle_key(key(KeyCode::Left), &mut Clipboard::new());
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
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
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
    fn an_odd_last_row_of_the_log_shows_the_next_commits_first_line() {
        // Five rows: two whole commits and the first line of a third,
        // as a scroll area would show it, rather than a blank row.
        let (dir, _) = repo_with_history();
        let mut view = view(&dir);
        view.log_share = Some(share_for(5, 40));
        let screen = draw(&mut view, 100, 40);
        assert_eq!(view.log_area.height, 5, "{screen:#?}");
        assert!(screen[0].contains("Merge side into main"), "{screen:#?}");
        assert!(screen[1].contains("Ann Author"), "{screen:#?}");
        assert!(screen[2].contains("On "), "{screen:#?}");
        assert!(screen[3].contains("Ann Author"), "{screen:#?}");
        assert!(screen[4].contains("On "), "{screen:#?}");
        assert!(!screen[4].contains("Ann Author"), "{screen:#?}");
        // The rule below the log is untouched.
        assert!(
            screen[5].starts_with("─") || screen[5].contains("├"),
            "{screen:#?}"
        );
        // Scrolling to the end shows the last commit whole, the odd
        // row blank below it, as the keys leave the top on a commit.
        view.handle_key(key(KeyCode::End), &mut Clipboard::new());
        let screen = draw(&mut view, 100, 40);
        assert_eq!(view.log_scroll, 4, "{screen:#?}");
        assert!(screen[2].contains("Base commit"), "{screen:#?}");
        assert!(screen[3].contains("Ann Author"), "{screen:#?}");
        let log_x = view.log_area.x as usize;
        let odd_row: String = screen[4].chars().skip(log_x).collect();
        assert_eq!(odd_row.trim(), "", "{screen:#?}");
        // The partly shown commit can be clicked, which reveals it whole.
        view.handle_key(key(KeyCode::Home), &mut Clipboard::new());
        draw(&mut view, 100, 40);
        let log = view.log_area;
        click(&mut view, log.x + 5, log.y + 4);
        let screen = draw(&mut view, 100, 40);
        assert_eq!(view.selected, 2, "{screen:#?}");
        assert_eq!(view.log_scroll, 2, "{screen:#?}");
        assert!(screen[2].contains("On "), "{screen:#?}");
        assert!(screen[4].contains("Base commit"), "{screen:#?}");
    }

    #[test]
    fn the_wheel_scrolls_the_log_a_row_at_a_time() {
        // Five rows of a four-commit log, scrolled a row an event.
        let (dir, _) = repo_with_history();
        let mut view = view(&dir);
        view.log_share = Some(share_for(5, 40));
        draw(&mut view, 100, 40);
        let log = view.log_area;
        let wheel = |view: &mut GitLogView, kind| {
            view.handle_mouse(
                MouseEvent {
                    kind,
                    column: log.x + 5,
                    row: log.y + 1,
                    modifiers: KeyModifiers::NONE,
                },
                1,
            );
            draw(view, 100, 40)
        };
        // One row down leaves the second line of the first commit at
        // the top, the next commit whole below it.
        let screen = wheel(&mut view, MouseEventKind::ScrollDown);
        assert_eq!(view.log_scroll, 1);
        let top = log.y as usize;
        assert!(screen[top].contains("Ann Author"), "{screen:#?}");
        assert!(!screen[top].contains("Merge"), "{screen:#?}");
        assert!(screen[top + 1].contains("On "), "{screen:#?}");
        // The wheel stops where the keys would end up: the last commit
        // whole, the odd row blank.
        for _ in 0..10 {
            wheel(&mut view, MouseEventKind::ScrollDown);
        }
        assert_eq!(view.log_scroll, 4);
        let screen = wheel(&mut view, MouseEventKind::ScrollUp);
        assert_eq!(view.log_scroll, 3);
        assert!(screen[top + 3].contains("Base commit"), "{screen:#?}");
        // A click on the commit half shown at the top selects it and
        // shows it whole.
        click(&mut view, log.x + 5, log.y);
        let screen = draw(&mut view, 100, 40);
        assert_eq!(view.selected, 1, "{screen:#?}");
        assert_eq!(view.log_scroll, 2, "{screen:#?}");
        // A key moving the selection off screen puts the top back on a
        // commit too.
        wheel(&mut view, MouseEventKind::ScrollDown);
        assert_eq!(view.log_scroll, 3);
        view.handle_key(key(KeyCode::Up), &mut Clipboard::new());
        draw(&mut view, 100, 40);
        assert_eq!(view.selected, 0);
        assert_eq!(view.log_scroll, 0);
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
            view.handle_key(key(KeyCode::Right), &mut Clipboard::new());
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
        view.handle_key(key(KeyCode::Right), &mut Clipboard::new());
        assert_eq!(view.log_h.col, col);
        // Moving to the short commit keeps the position, since the long
        // one is still on screen; the horizontal wheel scrolls back.
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        assert_eq!(view.log_h.col, col);
        view.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::ScrollLeft,
                column: 40,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
            WHEEL,
        );
        assert_eq!(view.log_h.col, col - WHEEL);
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
        view.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: bar.x,
                row: bar.y,
                modifiers: KeyModifiers::NONE,
            },
            WHEEL,
        );
        assert_eq!(view.log_h.col, 0);
        view.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: bar.x,
                row: bar.y,
                modifiers: KeyModifiers::NONE,
            },
            WHEEL,
        );
        assert!(!view.is_dragging());
        // The long message is still cut off, so the bar stays.
        let screen = draw(&mut view, 90, 30);
        assert!(screen[log_bottom].contains('█'), "{screen:#?}");
        view.handle_key(key(KeyCode::Left), &mut Clipboard::new());
        assert_eq!(view.pane, Pane::Log, "← at the left edge stays in the log");
    }

    #[test]
    fn a_diff_scrolls_sideways_with_its_gutter_fixed() {
        let (dir, _) = repo_with_long_lines();
        let mut view = view(&dir);
        view.handle_key(key(KeyCode::End), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        assert_eq!(view.pane, Pane::Content);
        let screen = draw(&mut view, 100, 30);
        let line = screen
            .iter()
            .position(|r| r.contains("+ let long"))
            .unwrap();
        assert!(!screen[line].contains("TAIL"), "{screen:#?}");
        assert!(
            sideways_bar_row(&view, &screen).contains('█'),
            "{screen:#?}"
        );
        let number_column = column_of(&screen[line], "1 +");
        // End takes the cursor to the end of the line, and the view
        // follows it there, the line numbers staying put.
        view.handle_key(key(KeyCode::End), &mut Clipboard::new());
        let screen = draw(&mut view, 100, 30);
        assert!(screen[line].contains("TAIL"), "{screen:#?}");
        assert_eq!(
            column_of(&screen[line], "1 +"),
            number_column,
            "{screen:#?}"
        );
        assert!(view.content_pane.h().col > 0);
        // The wheel scrolls sideways as far as the longest line shown
        // and a little more, as in the editor.
        let wheel = |view: &mut GitLogView, kind| {
            let area = view.content_area;
            view.handle_mouse(
                MouseEvent {
                    kind,
                    column: area.x + 10,
                    row: area.y + 1,
                    modifiers: KeyModifiers::NONE,
                },
                WHEEL,
            );
        };
        for _ in 0..50 {
            wheel(&mut view, MouseEventKind::ScrollRight);
        }
        let col = view.content_pane.h().col;
        let Some(Content::Diff(diff)) = &view.content else {
            panic!("the file's diff is shown");
        };
        let extent = view.content_pane.extent(Shown::Diff(diff));
        assert_eq!(col, extent + HSCROLL_SLACK - view.content_pane.h().capacity);
        // Home takes the cursor back to the start and the view with it;
        // ← there stays in the diff.
        view.handle_key(key(KeyCode::Home), &mut Clipboard::new());
        draw(&mut view, 100, 30);
        assert_eq!(view.content_pane.h().col, 0);
        view.handle_key(key(KeyCode::Left), &mut Clipboard::new());
        assert_eq!(view.pane, Pane::Content);
        wheel(&mut view, MouseEventKind::ScrollRight);
        assert_eq!(view.content_pane.h().col, WHEEL);
        wheel(&mut view, MouseEventKind::ScrollLeft);
        assert_eq!(view.content_pane.h().col, 0);
        // Coming back to the file starts at the left again, and the
        // description, with nothing out of view, has no scrollbar.
        view.handle_key(key(KeyCode::BackTab), &mut Clipboard::new());
        assert_eq!(view.pane, Pane::Files);
        view.handle_key(key(KeyCode::Right), &mut Clipboard::new());
        assert_eq!(view.pane, Pane::Files, "→ on a file stays in the files");
        view.handle_key(key(KeyCode::Up), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Up), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        let screen = draw(&mut view, 100, 30);
        assert!(
            screen.iter().any(|r| r.contains("+ let long")),
            "{screen:#?}"
        );
        assert_eq!(view.content_pane.h().col, 0);
        // The description scrolls too: this commit's long message needs
        // the bar, the short commit's doesn't.
        view.handle_key(key(KeyCode::Up), &mut Clipboard::new());
        let screen = draw(&mut view, 100, 30);
        assert!(screen.iter().any(|r| r.contains("Parents:")), "{screen:#?}");
        assert!(
            sideways_bar_row(&view, &screen).contains('█'),
            "{screen:#?}"
        );
        view.handle_key(key(KeyCode::Left), &mut Clipboard::new());
        assert_eq!(view.pane, Pane::Files, "← on the description stays");
        view.handle_key(key(KeyCode::BackTab), &mut Clipboard::new());
        assert_eq!(view.pane, Pane::Log);
        view.handle_key(key(KeyCode::Home), &mut Clipboard::new());
        let screen = draw(&mut view, 100, 30);
        assert!(screen.iter().any(|r| r.contains("Parents:")), "{screen:#?}");
        assert!(
            !sideways_bar_row(&view, &screen).contains('█'),
            "{screen:#?}"
        );
    }

    /// The content pane's bottom row, short of the vertical
    /// scrollbar's column: where the sideways scrollbar shows.
    fn sideways_bar_row(view: &GitLogView, screen: &[String]) -> String {
        let area = view.content_area;
        screen[area.bottom() as usize - 1]
            .chars()
            .skip(area.x as usize)
            .take(area.width as usize - 1)
            .collect()
    }

    #[test]
    fn a_diff_has_a_scrollbar_down_its_side_that_scrolls_it() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let sig = Signature::now("T", "t@example.com").unwrap();
        let body: String = (0..100).map(|i| format!("line {i}\n")).collect();
        fs::write(dir.path().join("t.rs"), body).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("t.rs")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "Tall", &tree, &[])
            .unwrap();
        let mut view = view(&dir);
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        assert_eq!(view.pane, Pane::Content);
        // The bar runs down the pane's right edge: at the top of the
        // diff the thumb is at the top, a short one, since a hundred
        // lines don't fit.
        let screen = draw(&mut view, 100, 30);
        let bar = view.content_pane.v().bar;
        assert_eq!(bar.x, view.content_area.right() - 1);
        assert_eq!(bar.height, view.content_area.height);
        let column = |screen: &[String]| -> String {
            (bar.y..bar.bottom())
                .map(|y| screen[y as usize].chars().nth(bar.x as usize).unwrap())
                .collect()
        };
        let track = column(&screen);
        assert!(track.starts_with('█') && track.ends_with('│'), "{track:?}");
        let thumb = track.chars().filter(|c| *c == '█').count();
        assert!(thumb < bar.height as usize / 2, "{track:?}");
        // At the end of the diff it is at the bottom.
        view.handle_key(ctrl_key(KeyCode::End), &mut Clipboard::new());
        let track = column(&draw(&mut view, 100, 30));
        assert!(track.starts_with('│') && track.ends_with('█'), "{track:?}");
        // A click halfway down the bar scrolls halfway through the
        // diff, and dragging to its top scrolls back to the start.
        click(&mut view, bar.x, bar.y + bar.height / 2);
        assert!(view.is_dragging());
        let (rows, shown) = view.content_pane.rows();
        let max = rows - shown;
        let halfway = view.content_pane.scroll();
        assert!(
            halfway > max / 3 && halfway < max * 2 / 3,
            "{halfway} of {max}"
        );
        let mouse = |kind| MouseEvent {
            kind,
            column: bar.x,
            row: bar.y,
            modifiers: KeyModifiers::NONE,
        };
        view.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left)), WHEEL);
        assert_eq!(view.content_pane.scroll(), 0);
        view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left)), WHEEL);
        assert!(!view.is_dragging());
        let screen = draw(&mut view, 100, 30);
        assert!(column(&screen).starts_with('█'), "{screen:#?}");
        assert!(screen[bar.y as usize].contains("line 0"), "{screen:#?}");
    }

    #[test]
    fn the_last_line_of_a_diff_shows_above_its_scrollbar() {
        // A diff taller than the pane whose last line is too long to
        // fit: at the end, the scrollbar takes the pane's last row and
        // the diff's last line is in the row above it.
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let sig = Signature::now("T", "t@example.com").unwrap();
        let mut body: String = (0..60).map(|i| format!("line {i}\n")).collect();
        body.push_str(&format!("let last = \"{}\"; // LAST\n", "a".repeat(120)));
        fs::write(dir.path().join("t.rs"), body).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("t.rs")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "Tall", &tree, &[])
            .unwrap();
        let mut view = view(&dir);
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        assert_eq!(view.pane, Pane::Content);
        view.handle_key(ctrl_key(KeyCode::End), &mut Clipboard::new());
        let screen = draw(&mut view, 100, 30);
        let content_bottom = view.content_area.bottom() as usize - 1;
        assert!(
            sideways_bar_row(&view, &screen).contains('█'),
            "{screen:#?}"
        );
        assert!(
            screen[content_bottom - 1].contains("+ let last"),
            "{screen:#?}"
        );
        let (rows, shown) = view.content_pane.rows();
        assert_eq!(view.content_pane.scroll() + shown, rows);
        // Scrolling down a line at a time (Ctrl+↓, which leaves the
        // cursor at the top) gets there too.
        view.handle_key(ctrl_key(KeyCode::Home), &mut Clipboard::new());
        draw(&mut view, 100, 30);
        for _ in 0..100 {
            view.handle_key(ctrl_key(KeyCode::Down), &mut Clipboard::new());
        }
        let screen = draw(&mut view, 100, 30);
        assert!(
            screen[content_bottom - 1].contains("+ let last"),
            "{screen:#?}"
        );
    }

    #[test]
    fn the_rules_between_panes_drag_to_resize_them() {
        let (dir, _) = repo_with_history();
        let mut view = view(&dir);
        draw(&mut view, 120, 40);
        let drag = |view: &mut GitLogView, from: (u16, u16), to: (u16, u16)| {
            click(view, from.0, from.1);
            assert!(view.is_dragging());
            view.handle_mouse(
                MouseEvent {
                    kind: MouseEventKind::Drag(MouseButton::Left),
                    column: to.0,
                    row: to.1,
                    modifiers: KeyModifiers::NONE,
                },
                WHEEL,
            );
            view.handle_mouse(
                MouseEvent {
                    kind: MouseEventKind::Up(MouseButton::Left),
                    column: to.0,
                    row: to.1,
                    modifiers: KeyModifiers::NONE,
                },
                WHEEL,
            );
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
        let released = view.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: at.0,
                row: at.1,
                modifiers: KeyModifiers::NONE,
            },
            WHEEL,
        );
        assert!(released.resized);
    }

    #[test]
    fn a_directory_that_is_not_a_repository_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut view = GitLogView::new(dir.path());
        let screen = draw(&mut view, 60, 10);
        assert!(screen[1].contains(NOT_A_REPOSITORY), "{screen:#?}");
        assert!(!view.poll());
    }

    /// Poll until the walk, and any refresh, is done.
    fn wait(view: &mut GitLogView) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while view.is_loading() && std::time::Instant::now() < deadline {
            view.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!view.is_loading());
    }

    /// Commit a file on top of HEAD, as `repo_with_history` commits.
    fn commit_on_head(dir: &tempfile::TempDir, name: &str, content: &str, message: &str) -> Oid {
        let repo = Repository::open(dir.path()).unwrap();
        fs::write(dir.path().join(name), content).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(name)).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = Signature::new(
            "Ann Author",
            "ann@example.com",
            &git2::Time::new(1_700_001_000, 0),
        )
        .unwrap();
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
    }

    #[test]
    fn a_refresh_keeps_the_place_and_takes_in_new_commits() {
        let (dir, ids) = repo_with_history();
        let mut view = view(&dir);
        let [_a, b, _s, _m] = ids[..] else { panic!() };
        // A log two commits tall, so that where it is scrolled to
        // matters.
        view.set_sizes(PaneSizes {
            sidebar: None,
            log: Some(0.2),
            files: None,
        });
        draw(&mut view, 110, 24);
        // Down twice to On main, scrolled onto the log's second row;
        // into its files, and the diff of its one file.
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        assert_eq!(view.selected_commit(), Some(b));
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        let screen = draw(&mut view, 110, 24);
        let row = screen.iter().position(|r| r.contains("On main")).unwrap();
        assert!(row_with(&screen, "a.rs").contains("+1"), "{screen:#?}");
        assert!(screen.iter().any(|r| r.contains("two();")), "{screen:#?}");
        assert!(!view.hint().text().contains("refreshing"));

        // A commit made meanwhile. Until the refresh is done, the page
        // is as it was, and says it is refreshing.
        commit_on_head(&dir, "n.txt", "new\n", "After the merge");
        view.refresh();
        assert!(view.is_loading());
        assert!(view.hint().text().contains("refreshing"), "{}", view.hint());
        let screen = draw(&mut view, 110, 24);
        assert_eq!(view.selected_commit(), Some(b));
        assert!(
            !screen.iter().any(|r| r.contains("After the merge")),
            "{screen:#?}"
        );
        assert!(screen[row].contains("On main"), "{screen:#?}");

        // Done: the same commit on the same row, its file still shown.
        wait(&mut view);
        assert!(
            !view.hint().text().contains("refreshing"),
            "{}",
            view.hint()
        );
        let screen = draw(&mut view, 110, 24);
        assert_eq!(view.selected_commit(), Some(b));
        assert_eq!(view.history.as_ref().unwrap().commits().len(), 5);
        assert!(screen[row].contains("On main"), "{screen:#?}");
        assert_eq!(view.pane, Pane::Files);
        assert!(row_with(&screen, "a.rs").contains("+1"), "{screen:#?}");
        assert!(screen.iter().any(|r| r.contains("two();")), "{screen:#?}");
        // The new commit is at the top of the log.
        view.handle_key(key(KeyCode::BackTab), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Home), &mut Clipboard::new());
        let screen = draw(&mut view, 110, 24);
        assert!(
            screen.iter().any(|r| r.contains("After the merge")),
            "{screen:#?}"
        );
    }

    #[test]
    fn a_refresh_whose_selected_commit_is_gone_goes_to_head() {
        let (dir, ids) = repo_with_history();
        let mut view = view(&dir);
        let [_a, b, _s, m] = ids[..] else { panic!() };
        draw(&mut view, 110, 24);
        assert_eq!(view.selected_commit(), Some(m));
        // Reset main to On main and drop the remote branch: the merge
        // is unreachable.
        let repo = Repository::open(dir.path()).unwrap();
        let main = repo.head().unwrap().shorthand().unwrap().to_owned();
        repo.reference(&format!("refs/heads/{main}"), b, true, "reset")
            .unwrap();
        repo.find_reference(&format!("refs/remotes/origin/{main}"))
            .unwrap()
            .delete()
            .unwrap();
        view.refresh();
        wait(&mut view);
        let screen = draw(&mut view, 110, 24);
        assert_eq!(view.selected_commit(), Some(b));
        assert!(
            !screen.iter().any(|r| r.contains("Merge side into main")),
            "{screen:#?}"
        );
        // The detail is the new selection's.
        assert!(row_with(&screen, "a.rs").contains("+1"), "{screen:#?}");
        assert!(!screen.iter().any(|r| r.contains("s.txt")), "{screen:#?}");
    }

    #[test]
    fn a_refresh_keeps_the_sidebar_by_name() {
        let (dir, ids) = repo_with_history();
        let mut view = view(&dir);
        let [a, ..] = ids[..] else { panic!() };
        draw(&mut view, 110, 24);
        // In the sidebar: unfold origin, and select `side`.
        view.handle_key(key(KeyCode::BackTab), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        assert_eq!(view.side_rows[view.side_selected], SideRow::Remote(0));
        view.handle_key(key(KeyCode::Right), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Up), &mut Clipboard::new());
        assert_eq!(view.side_rows[view.side_selected], SideRow::Branch(1));
        // A branch sorting before `side` and a remote sorting before
        // `origin` appear.
        let repo = Repository::open(dir.path()).unwrap();
        repo.branch("aaa", &repo.find_commit(a).unwrap(), false)
            .unwrap();
        repo.remote("backup", "https://example.com/b.git").unwrap();
        view.refresh();
        wait(&mut view);
        let screen = draw(&mut view, 110, 24);
        assert_eq!(view.side_rows[view.side_selected], SideRow::Branch(2));
        assert_eq!(view.collapsed, vec![true, false]);
        assert!(screen.iter().any(|r| r.contains("▸ backup")), "{screen:#?}");
        assert!(screen.iter().any(|r| r.contains("▾ origin")), "{screen:#?}");
        // The selected branch gone, the first is selected instead.
        repo.find_branch("side", git2::BranchType::Local)
            .unwrap()
            .delete()
            .unwrap();
        view.refresh();
        wait(&mut view);
        assert_eq!(view.side_rows[view.side_selected], SideRow::Branch(0));
    }

    /// A repository of one commit, `Local`, with `repo_with_history`'s
    /// as its `origin` (by path, so a fetch needs no network).
    fn repo_with_upstream() -> (tempfile::TempDir, tempfile::TempDir, Vec<Oid>) {
        let (upstream, ids) = repo_with_history();
        let dir = tempfile::tempdir().unwrap();
        Repository::init(dir.path()).unwrap();
        commit_on_head(&dir, "l.txt", "local\n", "Local");
        Repository::open(dir.path())
            .unwrap()
            .remote("origin", upstream.path().to_str().unwrap())
            .unwrap();
        (dir, upstream, ids)
    }

    #[test]
    fn f5_fetches_the_remotes_and_refreshes_keeping_the_place() {
        let (dir, upstream, ids) = repo_with_upstream();
        let [_a, _b, _s, m] = ids[..] else { panic!() };
        let mut view = view(&dir);
        let local = view.selected_commit().unwrap();
        draw(&mut view, 110, 24);
        // Into the files pane, so that there is a place to keep.
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        assert_eq!(view.pane, Pane::Files);
        let screen = draw(&mut view, 110, 24);
        assert!(
            !screen.iter().any(|r| r.contains("Merge side into main")),
            "{screen:#?}"
        );

        // F5: the fetch is under way, and the page says so; the
        // upstream's branches and tags come in, and the page refreshes
        // where it was, with what came of it for the status bar.
        view.handle_key(key(KeyCode::F(5)), &mut Clipboard::new());
        assert!(view.hint().text().contains("fetching"), "{}", view.hint());
        wait(&mut view);
        assert!(!view.hint().text().contains("fetching"), "{}", view.hint());
        let notice = notice_of(&mut view).unwrap();
        // origin's two branches and its tag.
        assert_eq!(notice, "Fetched origin: 3 refs updated");
        assert_eq!(notice_of(&mut view), None);
        assert_eq!(view.selected_commit(), Some(local));
        assert_eq!(view.pane, Pane::Files);
        let screen = draw(&mut view, 110, 24);
        assert!(screen.iter().any(|r| r.contains("▸ origin")), "{screen:#?}");
        assert!(
            screen.iter().any(|r| r.contains("Merge side into main")),
            "{screen:#?}"
        );
        assert!(view.history.as_ref().unwrap().position(m).is_some());
        // The local branch and HEAD are where they were.
        assert_eq!(view.history.as_ref().unwrap().head(), Some(local));

        // Again, with nothing new upstream: up to date.
        view.handle_key(key(KeyCode::F(5)), &mut Clipboard::new());
        wait(&mut view);
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some("Fetched origin: up to date")
        );
        // A commit upstream is fetched and shown.
        commit_on_head(&upstream, "u.txt", "up\n", "Upstream since");
        view.handle_key(key(KeyCode::F(5)), &mut Clipboard::new());
        wait(&mut view);
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some("Fetched origin: 1 ref updated")
        );
        view.handle_key(key(KeyCode::Left), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Home), &mut Clipboard::new());
        let screen = draw(&mut view, 110, 24);
        assert!(
            screen.iter().any(|r| r.contains("Upstream since")),
            "{screen:#?}"
        );
    }

    #[test]
    fn f5_fetches_the_submodule_for_a_diff_missing_its_commit() {
        // Upstream: a repository with a submodule whose URL is its own
        // directory, and a clone of it with the submodule cloned too.
        let upstream = tempfile::tempdir().unwrap();
        let up = Repository::init(upstream.path()).unwrap();
        commit_on_head(&upstream, "a.txt", "a\n", "First");
        let sub_url = upstream.path().join("sub");
        let mut declared = up
            .submodule(sub_url.to_str().unwrap(), Path::new("sub"), true)
            .unwrap();
        let up_sub = declared.open().unwrap();
        let commit_sub = |name: &str, content: &str, message: &str| {
            fs::write(up_sub.workdir().unwrap().join(name), content).unwrap();
            let mut index = up_sub.index().unwrap();
            index.add_path(Path::new(name)).unwrap();
            index.write().unwrap();
            let tree = up_sub.find_tree(index.write_tree().unwrap()).unwrap();
            let sig = Signature::now("Sub Author", "sub@example.com").unwrap();
            let head = up_sub.head().ok().and_then(|h| h.target());
            let parents: Vec<git2::Commit<'_>> = head
                .into_iter()
                .map(|id| up_sub.find_commit(id).unwrap())
                .collect();
            let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
            up_sub
                .commit(Some("HEAD"), &sig, &sig, message, &tree, &refs)
                .unwrap()
        };
        commit_sub("s.txt", "one\n", "Sub one");
        declared.add_finalize().unwrap();
        let commit_gitlink = |message: &str| {
            let mut index = up.index().unwrap();
            index.add_path(Path::new("sub")).unwrap();
            index.write().unwrap();
            let tree = up.find_tree(index.write_tree().unwrap()).unwrap();
            let sig = Signature::now("Ann Author", "ann@example.com").unwrap();
            let head = up.head().unwrap().peel_to_commit().unwrap();
            up.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&head])
                .unwrap()
        };
        commit_gitlink("Add sub");
        let dir = tempfile::tempdir().unwrap();
        let local = Repository::clone(upstream.path().to_str().unwrap(), dir.path()).unwrap();
        local
            .find_submodule("sub")
            .unwrap()
            .update(true, None)
            .unwrap();

        // Upstream moves the submodule on. Fetched without recursing,
        // the clone has the bump but its submodule doesn't have the
        // commit, and the bump's diff of the submodule says so.
        let s2 = commit_sub("s.txt", "two\n", "Sub two");
        let bump = commit_gitlink("Bump sub");
        local
            .config()
            .unwrap()
            .set_str("fetch.recurseSubmodules", "false")
            .unwrap();
        let mut view = view(&dir);
        view.handle_key(key(KeyCode::F(5)), &mut Clipboard::new());
        wait(&mut view);
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some("Fetched origin: 1 ref updated")
        );
        view.jump_to(bump);
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        let screen = draw(&mut view, 110, 30);
        row_with(&screen, "M sub");
        let row = row_with(&screen, "is not in the submodule's repository");
        assert!(
            row.contains(&format!("Commit {}", ninjaedit_core::git::short_id(s2))),
            "{row}"
        );
        assert!(!screen.iter().any(|r| r.contains("Sub two")), "{screen:#?}");

        // On demand again, F5 fetches the submodule as well, and the
        // diff on show fills in with the commits.
        local
            .config()
            .unwrap()
            .set_str("fetch.recurseSubmodules", "on-demand")
            .unwrap();
        view.handle_key(key(KeyCode::F(5)), &mut Clipboard::new());
        wait(&mut view);
        let notice = notice_of(&mut view).unwrap();
        assert!(
            notice.starts_with("Fetched origin: up to date · sub: Fetched origin: "),
            "{notice}"
        );
        let screen = draw(&mut view, 110, 30);
        assert!(
            !screen.iter().any(|r| r.contains("not in the submodule")),
            "{screen:#?}"
        );
        assert!(row_with(&screen, "Sub two").contains(HEAD_NODE));
        assert!(row_with(&screen, "Sub one").contains(NODE));
        assert_eq!(view.selected_commit(), Some(bump));
    }

    #[test]
    fn f5_without_remotes_or_a_repository_says_so() {
        let dir = tempfile::tempdir().unwrap();
        Repository::init(dir.path()).unwrap();
        commit_on_head(&dir, "l.txt", "local\n", "Local");
        let mut view = view(&dir);
        view.handle_key(key(KeyCode::F(5)), &mut Clipboard::new());
        wait(&mut view);
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some("No remotes to fetch from")
        );

        let dir = tempfile::tempdir().unwrap();
        let mut view = GitLogView::new(dir.path());
        view.handle_key(key(KeyCode::F(5)), &mut Clipboard::new());
        assert!(!view.is_loading());
        let notice = notice_of(&mut view).unwrap();
        assert!(notice.starts_with(NOT_A_REPOSITORY), "{notice}");
    }

    #[test]
    fn a_directory_made_a_repository_shows_its_history_when_refreshed() {
        let dir = tempfile::tempdir().unwrap();
        let mut view = GitLogView::new(dir.path());
        let screen = draw(&mut view, 60, 10);
        assert!(screen[1].contains(NOT_A_REPOSITORY), "{screen:#?}");
        Repository::init(dir.path()).unwrap();
        let first = commit_on_head(&dir, "f.txt", "first\n", "First commit");
        view.refresh();
        wait(&mut view);
        let screen = draw(&mut view, 60, 10);
        assert_eq!(view.selected_commit(), Some(first));
        assert!(
            screen.iter().any(|r| r.contains("First commit")),
            "{screen:#?}"
        );
    }

    /// Where HEAD is: its branch (`None` when detached) and its commit.
    fn head_of(dir: &tempfile::TempDir) -> (Option<String>, Oid) {
        head_of_repo(&Repository::open(dir.path()).unwrap())
    }

    fn type_text(view: &mut GitLogView, text: &str) {
        for c in text.chars() {
            view.handle_key(key(KeyCode::Char(c)), &mut Clipboard::new());
        }
    }

    #[test]
    fn space_checks_out_the_selected_commit_on_its_branch_or_detached() {
        let (dir, ids) = repo_with_history();
        let [a, b, s, m] = ids[..] else { panic!() };
        let main = head_of(&dir).0.unwrap();
        let mut view = view(&dir);
        draw(&mut view, 110, 24);
        assert_eq!(view.selected_commit(), Some(m));

        // On side: its branch is checked out, with its files, and the
        // page refreshes with HEAD there.
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        assert_eq!(view.selected_commit(), Some(s));
        view.handle_key(key(KeyCode::Char(' ')), &mut Clipboard::new());
        // Under way: a second Space is refused, and the status bar
        // says what is going on until it is done.
        assert!(
            view.hint()
                .text()
                .starts_with(&format!("checking out {}…", short_id(s))),
            "{}",
            view.hint()
        );
        view.handle_key(key(KeyCode::Char(' ')), &mut Clipboard::new());
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some("A checkout is under way")
        );
        wait(&mut view);
        assert_eq!(view.hint(), StatusLine::help(LOG_HELP));
        assert_eq!(notice_of(&mut view).as_deref(), Some("Checked out side"));
        assert_eq!(head_of(&dir), (Some("side".to_owned()), s));
        assert_eq!(
            fs::read_to_string(dir.path().join("s.txt")).unwrap(),
            "side\n"
        );
        wait(&mut view);
        let history = view.history.as_ref().unwrap();
        assert_eq!(history.head_branch(), Some("side"));
        assert_eq!(history.head(), Some(s));
        assert_eq!(view.selected_commit(), Some(s));

        // On main: no branch points at it, so HEAD is detached there.
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        assert_eq!(view.selected_commit(), Some(b));
        view.handle_key(key(KeyCode::Char(' ')), &mut Clipboard::new());
        wait(&mut view);
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some(format!("HEAD detached at {}", short_id(b)).as_str())
        );
        assert_eq!(head_of(&dir), (None, b));
        assert!(!dir.path().join("s.txt").exists());
        wait(&mut view);
        assert_eq!(view.history.as_ref().unwrap().head_branch(), None);
        // Detached, the commit still says HEAD, with no branch to point
        // at; the branch's commit no longer does.
        let screen = draw(&mut view, 110, 24);
        let on_main = row_with(&screen, "On main");
        assert!(on_main.contains("HEAD  On main"), "{on_main}");
        assert!(on_main.contains(HEAD_NODE), "{on_main}");
        let merge = row_with(&screen, "Merge side into main");
        assert!(!merge.contains("HEAD"), "{merge}");
        assert!(!merge.contains(HEAD_NODE), "{merge}");

        // The merge: main and origin/main point at it; the local
        // branch wins. Once there, Space again just says so.
        view.handle_key(key(KeyCode::Home), &mut Clipboard::new());
        assert_eq!(view.selected_commit(), Some(m));
        view.handle_key(key(KeyCode::Char(' ')), &mut Clipboard::new());
        wait(&mut view);
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some(format!("Checked out {main}").as_str())
        );
        assert_eq!(head_of(&dir), (Some(main.clone()), m));
        wait(&mut view);
        view.handle_key(key(KeyCode::Char(' ')), &mut Clipboard::new());
        wait(&mut view);
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some(format!("Already on {main}").as_str())
        );
        assert_eq!(head_of(&dir), (Some(main), m));
        let _ = a;
    }

    #[test]
    fn a_checkout_brings_the_submodules_along_and_refreshes_their_tabs() {
        let (dir, bump, subs) = repo_with_submodule_bump();
        let [s1, _s2, s3] = subs[..] else { panic!() };
        let sub = Repository::open(dir.path().join("sub")).unwrap();
        let sub_main = sub.head().unwrap().shorthand().unwrap().to_owned();
        sub.branch("old", &sub.find_commit(s1).unwrap(), false)
            .unwrap();
        let mut tabs = GitLogTabs::new(dir.path(), GitLogLayout::default());
        wait_tabs(&mut tabs);
        // Open the submodule's tab, so that it has a page to refresh.
        tabs.set_active(1);
        wait_tabs(&mut tabs);
        assert_eq!(tabs.active().history.as_ref().unwrap().head(), Some(s3));
        tabs.set_active(0);
        assert_eq!(tabs.active().selected_commit(), Some(bump));

        // Add submodule: the submodule goes back to its first commit,
        // on to the branch there.
        tabs.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        tabs.handle_key(key(KeyCode::Char(' ')), &mut Clipboard::new());
        wait_tabs(&mut tabs);
        let notice = notice_of_tabs(&mut tabs).unwrap();
        assert!(notice.ends_with("; 1 submodule updated"), "{notice}");
        assert_eq!(head_of_repo(&sub), (Some("old".to_owned()), s1));
        assert_eq!(
            fs::read_to_string(dir.path().join("sub/inner.txt")).unwrap(),
            "one\n"
        );
        wait_tabs(&mut tabs);
        // The submodule's tab, shown again, refreshes to where it is now.
        tabs.set_active(1);
        wait_tabs(&mut tabs);
        let history = tabs.active().history.as_ref().unwrap();
        assert_eq!(history.head(), Some(s1));
        assert_eq!(history.head_branch(), Some("old"));

        // And forward again, on to the submodule's own branch.
        tabs.set_active(0);
        tabs.handle_key(key(KeyCode::Up), &mut Clipboard::new());
        tabs.handle_key(key(KeyCode::Char(' ')), &mut Clipboard::new());
        wait_tabs(&mut tabs);
        let notice = notice_of_tabs(&mut tabs).unwrap();
        assert!(notice.ends_with("; 1 submodule updated"), "{notice}");
        assert_eq!(head_of_repo(&sub), (Some(sub_main), s3));
        wait_tabs(&mut tabs);

        // Changes in the submodule refuse the checkout and are named.
        fs::write(dir.path().join("sub/inner.txt"), "edited\n").unwrap();
        tabs.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        tabs.handle_key(key(KeyCode::Char(' ')), &mut Clipboard::new());
        wait_tabs(&mut tabs);
        assert_eq!(
            notice_of_tabs(&mut tabs).as_deref(),
            Some(
                "Could not check out: in submodule sub: unstaged changes would be lost; \
                 commit or stash them first"
            )
        );
        assert_eq!(head_of_repo(&sub).1, s3);
    }

    /// Where a repository's HEAD is: its branch (`None` when detached)
    /// and its commit.
    fn head_of_repo(repo: &Repository) -> (Option<String>, Oid) {
        let head = repo.head().unwrap();
        let name = if repo.head_detached().unwrap() {
            None
        } else {
            Some(head.shorthand().unwrap().to_owned())
        };
        (name, head.target().unwrap())
    }

    #[test]
    fn a_double_click_on_a_commit_checks_it_out() {
        let (dir, ids) = repo_with_history();
        let [_a, _b, s, m] = ids[..] else { panic!() };
        let mut view = view(&dir);
        let screen = draw(&mut view, 110, 24);
        let row = screen.iter().position(|r| r.contains("On side")).unwrap() as u16;
        let column = column_of(&screen[row as usize], "On side");
        // One click only selects.
        click(&mut view, column, row);
        assert_eq!(view.selected_commit(), Some(s));
        assert_eq!(head_of(&dir).1, m);
        assert_eq!(notice_of(&mut view), None);
        click(&mut view, column, row);
        // The checkout is under way, and the status bar says so.
        assert!(
            view.hint().text().starts_with("checking out "),
            "{}",
            view.hint()
        );
        wait(&mut view);
        assert_eq!(notice_of(&mut view).as_deref(), Some("Checked out side"));
        assert_eq!(head_of(&dir), (Some("side".to_owned()), s));
        wait(&mut view);
    }

    #[test]
    fn a_checkout_refuses_while_tracked_files_are_changed() {
        let (dir, ids) = repo_with_history();
        let [_a, _b, s, m] = ids[..] else { panic!() };
        let main = head_of(&dir).0.unwrap();
        let mut view = view(&dir);
        draw(&mut view, 110, 24);
        fs::write(dir.path().join("a.rs"), "changed\n").unwrap();
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        assert_eq!(view.selected_commit(), Some(s));
        view.handle_key(key(KeyCode::Char(' ')), &mut Clipboard::new());
        wait(&mut view);
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some("Could not check out: unstaged changes would be lost; commit or stash them first")
        );
        assert_eq!(head_of(&dir), (Some(main), m));
        assert_eq!(
            fs::read_to_string(dir.path().join("a.rs")).unwrap(),
            "changed\n"
        );
        assert!(!view.is_loading());
    }

    #[test]
    fn a_remote_branch_gets_a_local_branch_tracking_it() {
        let (dir, ids) = repo_with_history();
        let [_a, b, _s, _m] = ids[..] else { panic!() };
        Repository::open(dir.path())
            .unwrap()
            .reference("refs/remotes/origin/feature", b, false, "t")
            .unwrap();
        let mut view = view(&dir);
        draw(&mut view, 110, 24);
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        assert_eq!(view.selected_commit(), Some(b));
        view.handle_key(key(KeyCode::Char(' ')), &mut Clipboard::new());
        wait(&mut view);
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some("Checked out new branch feature tracking origin/feature")
        );
        assert_eq!(head_of(&dir), (Some("feature".to_owned()), b));
        let repo = Repository::open(dir.path()).unwrap();
        let branch = repo.find_branch("feature", BranchType::Local).unwrap();
        assert_eq!(
            branch.upstream().unwrap().name().unwrap(),
            Some("origin/feature")
        );
        wait(&mut view);
        let screen = draw(&mut view, 110, 24);
        assert!(
            screen
                .iter()
                .any(|r| r.contains("feature") && r.contains("On main")),
            "{screen:#?}"
        );
    }

    #[test]
    fn a_tracking_branch_is_fast_forwarded_or_the_checkout_refused() {
        let (dir, ids) = repo_with_history();
        let [_a, b, s, m] = ids[..] else { panic!() };
        let main = head_of(&dir).0.unwrap();
        {
            // origin/side is a commit past side, which tracks it;
            // origin/main is behind main, which tracks it.
            let repo = Repository::open(dir.path()).unwrap();
            repo.set_head("refs/heads/side").unwrap();
            repo.checkout_tree(
                repo.find_commit(s).unwrap().as_object(),
                Some(git2::build::CheckoutBuilder::new().force()),
            )
            .unwrap();
            let past = commit_on_head(&dir, "t.txt", "t\n", "Past side");
            repo.reference("refs/heads/side", s, true, "t").unwrap();
            repo.reference("refs/remotes/origin/side", past, false, "t")
                .unwrap();
            repo.find_branch("side", BranchType::Local)
                .unwrap()
                .set_upstream(Some("origin/side"))
                .unwrap();
            repo.reference(&format!("refs/remotes/origin/{main}"), b, true, "t")
                .unwrap();
            repo.find_branch(&main, BranchType::Local)
                .unwrap()
                .set_upstream(Some(&format!("origin/{main}")))
                .unwrap();
            repo.set_head(&format!("refs/heads/{main}")).unwrap();
            repo.checkout_tree(
                repo.find_commit(m).unwrap().as_object(),
                Some(git2::build::CheckoutBuilder::new().force()),
            )
            .unwrap();
        }
        let mut view = view(&dir);
        draw(&mut view, 110, 24);
        assert_eq!(view.selected_commit(), Some(m));

        // Past side: side is fast-forwarded to it and checked out.
        view.handle_key(key(KeyCode::Home), &mut Clipboard::new());
        let screen = draw(&mut view, 110, 24);
        let row = screen.iter().position(|r| r.contains("Past side")).unwrap() as u16;
        click(
            &mut view,
            column_of(&screen[row as usize], "Past side"),
            row,
        );
        view.handle_key(key(KeyCode::Char(' ')), &mut Clipboard::new());
        wait(&mut view);
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some("Fast-forwarded side to origin/side and checked it out")
        );
        let (branch, at) = head_of(&dir);
        assert_eq!(branch.as_deref(), Some("side"));
        assert_eq!(fs::read_to_string(dir.path().join("t.txt")).unwrap(), "t\n");
        wait(&mut view);
        let history = view.history.as_ref().unwrap();
        assert_eq!(history.head(), Some(at));
        assert_eq!(history.head_branch(), Some("side"));
        let screen = draw(&mut view, 110, 24);
        assert!(
            screen
                .iter()
                .any(|r| r.contains("side") && r.contains("Past side")),
            "{screen:#?}"
        );

        // On main: main tracks origin/main from ahead, so nothing moves.
        let row = screen.iter().position(|r| r.contains("On main")).unwrap() as u16;
        click(&mut view, column_of(&screen[row as usize], "On main"), row);
        assert_eq!(view.selected_commit(), Some(b));
        view.handle_key(key(KeyCode::Char(' ')), &mut Clipboard::new());
        wait(&mut view);
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some(
                format!(
                    "Could not check out: {main} is ahead of origin/{main} by 2 commits; \
                     push or reset it first"
                )
                .as_str()
            )
        );
        assert_eq!(head_of(&dir).0.as_deref(), Some("side"));
        assert!(!view.is_loading());
    }

    #[test]
    fn a_taken_branch_name_is_asked_for_in_a_box_over_the_log() {
        let (dir, ids) = repo_with_history();
        let [a, _b, _s, m] = ids[..] else { panic!() };
        let main = head_of(&dir).0.unwrap();
        {
            let repo = Repository::open(dir.path()).unwrap();
            repo.reference("refs/remotes/origin/other", a, false, "t")
                .unwrap();
            repo.branch("other", &repo.find_commit(m).unwrap(), false)
                .unwrap();
        }
        let mut view = view(&dir);
        draw(&mut view, 110, 24);
        view.handle_key(key(KeyCode::End), &mut Clipboard::new());
        assert_eq!(view.selected_commit(), Some(a));

        // The box opens, saying why, and takes the keyboard; Esc closes
        // it with nothing done.
        view.handle_key(key(KeyCode::Char(' ')), &mut Clipboard::new());
        wait(&mut view);
        assert!(view.branch_prompt.is_some());
        assert_eq!(view.hint(), StatusLine::help(PROMPT_HELP));
        let screen = draw(&mut view, 110, 24);
        assert!(
            screen.iter().any(|r| r.contains("Name for the new branch")),
            "{screen:#?}"
        );
        assert!(
            screen
                .iter()
                .any(|r| r.contains("branch other exists · new branch will track origin/other")),
            "{screen:#?}"
        );
        view.handle_key(key(KeyCode::Down), &mut Clipboard::new());
        assert_eq!(view.selected_commit(), Some(a));
        view.handle_key(key(KeyCode::Esc), &mut Clipboard::new());
        assert!(view.branch_prompt.is_none());
        assert_eq!(head_of(&dir), (Some(main.clone()), m));
        assert_eq!(notice_of(&mut view), None);

        // A name that won't do keeps the box open and says why.
        view.handle_key(key(KeyCode::Char(' ')), &mut Clipboard::new());
        wait(&mut view);
        type_text(&mut view, "other");
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        wait(&mut view);
        assert!(view.branch_prompt.is_some());
        let screen = draw(&mut view, 110, 24);
        assert!(
            screen
                .iter()
                .any(|r| r.contains("branch other already exists")),
            "{screen:#?}"
        );
        assert_eq!(head_of(&dir), (Some(main.clone()), m));
        for _ in 0..5 {
            view.handle_key(key(KeyCode::Backspace), &mut Clipboard::new());
        }
        type_text(&mut view, "bad name");
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        wait(&mut view);
        let screen = draw(&mut view, 110, 24);
        assert!(
            screen
                .iter()
                .any(|r| r.contains("\"bad name\" is not a valid branch name")),
            "{screen:#?}"
        );
        for _ in 0..8 {
            view.handle_key(key(KeyCode::Backspace), &mut Clipboard::new());
        }

        // A free name makes the branch, tracking the remote's, and
        // checks the commit out on it.
        type_text(&mut view, "mine");
        view.handle_key(key(KeyCode::Enter), &mut Clipboard::new());
        wait(&mut view);
        assert!(view.branch_prompt.is_none());
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some("Checked out new branch mine tracking origin/other")
        );
        assert_eq!(head_of(&dir), (Some("mine".to_owned()), a));
        let repo = Repository::open(dir.path()).unwrap();
        let branch = repo.find_branch("mine", BranchType::Local).unwrap();
        assert_eq!(
            branch.upstream().unwrap().name().unwrap(),
            Some("origin/other")
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("a.rs")).unwrap(),
            "fn main() {\n    one();\n}\n"
        );
        wait(&mut view);
        assert_eq!(view.history.as_ref().unwrap().head_branch(), Some("mine"));
        assert_eq!(view.hint(), StatusLine::help(LOG_HELP));

        // A click outside the box closes it too.
        view.handle_key(key(KeyCode::Home), &mut Clipboard::new());
        wait(&mut view);
        Repository::open(dir.path())
            .unwrap()
            .reference("refs/remotes/origin/other", m, true, "t")
            .unwrap();
        view.handle_key(key(KeyCode::Char(' ')), &mut Clipboard::new());
        wait(&mut view);
        // main points at the merge too, so it wins: no box.
        assert!(view.branch_prompt.is_none());
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some(format!("Checked out {main}").as_str())
        );
    }

    /// A repository of three commits: Base (src/a.rs, src/b.rs,
    /// top.txt), Change (src/a.rs and top.txt edited, src/new.rs added),
    /// and Later (src/a.rs and src/b.rs edited). Returns their ids.
    fn repo_to_restore() -> (tempfile::TempDir, [Oid; 3]) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let mut clock = 1_700_000_000;
        let mut commit = |files: &[(&str, &str)], message: &str| {
            let mut index = repo.index().unwrap();
            for (name, content) in files {
                let path = dir.path().join(name);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, content).unwrap();
                index.add_path(Path::new(name)).unwrap();
            }
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            clock += 60;
            let sig = Signature::new("Ann Author", "ann@example.com", &git2::Time::new(clock, 0))
                .unwrap();
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
        let base = commit(
            &[
                ("src/a.rs", "a1\n"),
                ("src/b.rs", "b1\n"),
                ("top.txt", "t1\n"),
            ],
            "Base",
        );
        let change = commit(
            &[
                ("src/a.rs", "a2\n"),
                ("src/new.rs", "n\n"),
                ("top.txt", "t2\n"),
            ],
            "Change",
        );
        let later = commit(&[("src/a.rs", "a3\n"), ("src/b.rs", "b3\n")], "Later");
        (dir, [base, change, later])
    }

    fn read(dir: &tempfile::TempDir, path: &str) -> String {
        fs::read_to_string(dir.path().join(path)).unwrap()
    }

    /// Select a commit of the log by id, and a row of its file list.
    fn select_file_of(view: &mut GitLogView, id: Oid, row: usize) {
        let position = view.history.as_ref().unwrap().position(id).unwrap();
        view.select_commit(position);
        view.ensure_detail();
        view.select_file(row);
    }

    /// The row of the file list for the directory at `path`.
    fn dir_row_of(view: &GitLogView, path: &str) -> usize {
        view.file_rows
            .iter()
            .position(|row| {
                matches!(row, TreeRow::Dir { dir, .. } if view.file_tree.dirs()[*dir].path == path)
            })
            .unwrap_or_else(|| panic!("no row for {path}/"))
            + 1
    }

    #[test]
    fn restoring_a_directory_restores_the_commits_files_under_it() {
        let (dir, [_base, change, _later]) = repo_to_restore();
        let mut view = view(&dir);
        draw(&mut view, 120, 40);
        select_file_of(&mut view, change, 0);
        let src = dir_row_of(&view, "src");
        view.select_file(src);
        assert!(view.can_restore_selected());
        assert!(!view.can_open_selected_change());
        let id = short_id(change);

        // As before the commit: a.rs as Base had it, and new.rs, which
        // Base hadn't, deleted. b.rs, which the commit didn't change, is
        // left as it is, as is top.txt, outside the directory. Nothing
        // unstaged was in the way, so nothing asked.
        view.restore_selected(Version::Parent);
        assert!(view.restore_box().is_none());
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some(
                format!("Restored src/ as before {id}: 1 file deleted, as that version hasn't it")
                    .as_str()
            )
        );
        assert_eq!(read(&dir, "src/a.rs"), "a1\n");
        assert!(!dir.path().join("src/new.rs").exists());
        assert_eq!(read(&dir, "src/b.rs"), "b3\n");
        assert_eq!(read(&dir, "top.txt"), "t2\n");
        // Only the working directory changed: the index has HEAD's.
        let repo = Repository::open(dir.path()).unwrap();
        let status = repo.status_file(Path::new("src/a.rs")).unwrap();
        assert!(status.contains(git2::Status::WT_MODIFIED), "{status:?}");
        assert!(!status.intersects(git2::Status::INDEX_MODIFIED));

        // As of the commit, those restored changes are unstaged and
        // would be overwritten, so the box asks; Enter alone cancels.
        view.restore_selected(Version::Commit);
        let dialog = view.restore_box().unwrap();
        assert_eq!(dialog.title(), format!("Restore src/ as of {id}?"));
        assert_eq!(
            dialog.body(),
            "Changes that aren't committed or staged are overwritten in src/a.rs. This can't be undone."
        );
        let screen = draw(&mut view, 120, 40);
        row_with(&screen, &format!("Restore src/ as of {id}?"));
        assert!(view.hint().text().contains("y restore"), "{}", view.hint());
        let mut clipboard = Clipboard::local_only();
        view.handle_key(key(KeyCode::Enter), &mut clipboard);
        assert!(view.restore_box().is_none());
        assert_eq!(read(&dir, "src/a.rs"), "a1\n");
        assert_eq!(notice_of(&mut view), None);
        view.restore_selected(Version::Commit);
        view.handle_key(key(KeyCode::Char('y')), &mut clipboard);
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some(format!("Restored src/ as of {id}").as_str())
        );
        assert_eq!(read(&dir, "src/a.rs"), "a2\n");
        assert_eq!(read(&dir, "src/new.rs"), "n\n");
        assert_eq!(read(&dir, "src/b.rs"), "b3\n");
    }

    #[test]
    fn restoring_a_file_as_before_a_root_commit_deletes_it() {
        let (dir, [base, _change, _later]) = repo_to_restore();
        let mut view = view(&dir);
        draw(&mut view, 120, 40);
        select_file_of(&mut view, base, 0);
        let top = file_row_of(&view, "top.txt");
        view.select_file(top);
        view.restore_selected(Version::Parent);
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some(
                format!(
                    "Restored top.txt as before {}: 1 file deleted, as that version hasn't it",
                    short_id(base)
                )
                .as_str()
            )
        );
        assert!(!dir.path().join("top.txt").exists());
        // As of it, it comes back. Its deletion is an unstaged change,
        // but writing it back loses nothing, so nothing asks.
        view.restore_selected(Version::Commit);
        assert!(view.restore_box().is_none());
        assert_eq!(read(&dir, "top.txt"), "t1\n");
    }

    #[test]
    fn o_opens_the_selected_file_as_it_is_in_the_working_directory() {
        let (dir, [_base, change, _later]) = repo_to_restore();
        let mut tabs = GitLogTabs::new(dir.path(), GitLogLayout::default());
        wait_tabs(&mut tabs);
        draw(tabs.active(), 120, 40);
        select_file_of(tabs.active(), change, 0);
        let top = file_row_of(tabs.active(), "top.txt");
        tabs.active().select_file(top);
        assert!(tabs.active().can_open_selected_change());
        // `o` from the file list, or from the diff, which opens it at
        // the line of the diff's cursor.
        let mut clipboard = Clipboard::local_only();
        tabs.active().pane = Pane::Files;
        tabs.handle_key(key(KeyCode::Char('o')), &mut clipboard);
        let (path, line) = tabs.take_file_to_open().unwrap();
        assert_eq!(
            fs::canonicalize(path).unwrap(),
            fs::canonicalize(dir.path().join("top.txt")).unwrap()
        );
        assert_eq!(line, None);
        assert_eq!(tabs.take_file_to_open(), None);
        tabs.active().pane = Pane::Content;
        tabs.handle_key(key(KeyCode::Char('o')), &mut clipboard);
        assert!(matches!(tabs.take_file_to_open(), Some((_, Some(_)))));
        // The palette's and the menu's "Open changed file" does the same.
        tabs.open_selected_change();
        assert!(tabs.take_file_to_open().is_some());
        // A file that isn't in the working directory has nothing to
        // open, and the status bar says so.
        fs::remove_file(dir.path().join("src/new.rs")).unwrap();
        let new = file_row_of(tabs.active(), "src/new.rs");
        tabs.active().select_file(new);
        tabs.open_selected_change();
        assert_eq!(tabs.take_file_to_open(), None);
        assert_eq!(
            notice_of_tabs(&mut tabs).as_deref(),
            Some("src/new.rs isn't in the working directory")
        );
        // Nor is there anything to open for the description.
        tabs.active().select_file(0);
        assert!(!tabs.active().can_open_selected_change());
        assert!(!tabs.active().can_restore_selected());
    }

    #[test]
    fn a_right_click_on_a_file_selects_it_and_asks_for_the_menu() {
        let (dir, [_base, change, _later]) = repo_to_restore();
        let mut view = view(&dir);
        draw(&mut view, 120, 40);
        select_file_of(&mut view, change, 0);
        let screen = draw(&mut view, 120, 40);
        let right_press = |view: &mut GitLogView, column: u16, row: u16| {
            view.handle_mouse(
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Right),
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                },
                WHEEL,
            )
        };
        let x = view.files_area.x + 2;
        let y_of = |text: &str| {
            screen
                .iter()
                .position(|row| {
                    row.chars()
                        .skip(view.files_area.x as usize)
                        .take(view.files_area.width as usize)
                        .collect::<String>()
                        .contains(text)
                })
                .unwrap_or_else(|| panic!("no {text:?} in {screen:#?}")) as u16
        };
        let (src, top, description) = (y_of("src"), y_of("top.txt"), y_of(DESCRIPTION));
        view.pane = Pane::Log;
        let outcome = right_press(&mut view, x, top);
        assert_eq!(outcome.menu, Some(GitLogMenu::Files));
        assert_eq!(view.pane, Pane::Files);
        assert!(view.can_open_selected_change());
        // A directory is selected, not folded as a left click would.
        let outcome = right_press(&mut view, x, src);
        assert_eq!(outcome.menu, Some(GitLogMenu::Files));
        assert!(!view.can_open_selected_change());
        assert!(view.can_restore_selected());
        assert!(view.dirs_collapsed.iter().all(|folded| !folded));
        // The description asks for no menu.
        assert_eq!(right_press(&mut view, x, description).menu, None);
        // A commit in the log selects it and asks for the commit's.
        let log = view.log_area;
        let outcome = right_press(&mut view, log.x + 4, log.y + 2);
        assert_eq!(outcome.menu, Some(GitLogMenu::Commit));
        assert_eq!(view.pane, Pane::Log);
        assert_eq!(view.selected, 1);
    }

    /// A repository with `main` at a base and a commit changing `f.txt`,
    /// and `side` from the base with a commit changing `g.txt`, or
    /// `f.txt` too when `clash`, so that the two conflict. HEAD is on
    /// main, with its files, and the configuration names someone to
    /// commit as. Returns the base, main's commit, and side's.
    fn diverged_repo(clash: bool) -> (tempfile::TempDir, [Oid; 3]) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Ann Author").unwrap();
        config.set_str("user.email", "ann@example.com").unwrap();
        let sig = Signature::new(
            "Ann Author",
            "ann@example.com",
            &git2::Time::new(1_700_000_000, 0),
        )
        .unwrap();
        let commit = |files: &[(&str, &str)], message: &str, parent: Option<Oid>| {
            let mut index = repo.index().unwrap();
            for (name, content) in files {
                fs::write(dir.path().join(name), content).unwrap();
                index.add_path(Path::new(name)).unwrap();
            }
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let parents: Vec<git2::Commit<'_>> = parent
                .map(|id| repo.find_commit(id).unwrap())
                .into_iter()
                .collect();
            let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
            repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &refs)
                .unwrap()
        };
        let base = commit(&[("f.txt", "base\n")], "Base", None);
        let main = commit(&[("f.txt", "main\n")], "Main", Some(base));
        let head = repo.head().unwrap().name().unwrap().to_owned();
        repo.branch("side", &repo.find_commit(base).unwrap(), false)
            .unwrap();
        repo.set_head("refs/heads/side").unwrap();
        let force = || {
            let mut options = git2::build::CheckoutBuilder::new();
            options.force().remove_untracked(true);
            options
        };
        repo.checkout_head(Some(&mut force())).unwrap();
        let side_file = if clash { "f.txt" } else { "g.txt" };
        let side = commit(&[(side_file, "side\n")], "Side", Some(base));
        repo.set_head(&head).unwrap();
        repo.checkout_head(Some(&mut force())).unwrap();
        (dir, [base, main, side])
    }

    #[test]
    fn a_commit_in_the_log_is_merged_or_reset_to() {
        let (dir, [base, main, side]) = diverged_repo(false);
        let head = head_of(&dir).0.unwrap();
        let mut view = view(&dir);
        draw(&mut view, 110, 24);
        let at = |view: &GitLogView, id: Oid| view.history.as_ref().unwrap().position(id).unwrap();

        // HEAD's own commit has nothing to merge, rebase onto, or reset
        // to; side's has a branch to merge; the base has no branch.
        view.select_commit(at(&view, main));
        assert!(!view.other_commit_selected());
        assert!(!view.can_merge_commit());
        view.select_commit(at(&view, base));
        assert!(view.other_commit_selected());
        assert!(!view.can_merge_commit());
        view.select_commit(at(&view, side));
        assert!(view.can_merge_commit());

        view.merge_selected_commit();
        assert!(
            view.hint().text().starts_with("merging side…"),
            "{}",
            view.hint()
        );
        wait(&mut view);
        let merged = head_of(&dir).1;
        assert_eq!(
            notice_of(&mut view),
            Some(format!("Merged side into {head} as {}", short_id(merged)))
        );
        assert!(!view.take_show_changes());
        let repo = Repository::open(dir.path()).unwrap();
        let parents: Vec<Oid> = repo.find_commit(merged).unwrap().parent_ids().collect();
        assert_eq!(parents, [main, side]);
        assert_eq!(
            fs::read_to_string(dir.path().join("g.txt")).unwrap(),
            "side\n"
        );
        // The log follows: HEAD is the merge now.
        assert_eq!(view.history.as_ref().unwrap().head(), Some(merged));

        // Resetting back to main's commit leaves the merge on no branch,
        // so the box asks; No keeps it, Yes resets, keeping the files.
        view.select_commit(at(&view, main));
        view.reset_to_selected_commit();
        assert_eq!(view.hint(), StatusLine::help(RESET_HELP));
        let screen = draw(&mut view, 110, 24);
        let asked = format!("Reset {head} to {}?", short_id(main));
        assert!(screen.iter().any(|r| r.contains(&asked)), "{screen:#?}");
        view.handle_key(key(KeyCode::Char('n')), &mut Clipboard::new());
        assert!(view.reset_box.is_none());
        assert_eq!(head_of(&dir).1, merged);
        view.reset_to_selected_commit();
        view.handle_key(key(KeyCode::Char('y')), &mut Clipboard::new());
        assert_eq!(
            notice_of(&mut view),
            Some(format!(
                "Reset {head} to {}; the working directory is unchanged",
                short_id(main)
            ))
        );
        assert_eq!(head_of(&dir), (Some(head.clone()), main));
        assert_eq!(
            fs::read_to_string(dir.path().join("g.txt")).unwrap(),
            "side\n"
        );
        assert_eq!(
            repo.status_file(Path::new("g.txt")).unwrap(),
            git2::Status::WT_NEW
        );
        wait(&mut view);

        // Back to side's commit: main's commit is on no other branch
        // either, but a tag keeps it, so nothing is asked.
        repo.tag_lightweight("kept", &repo.find_object(main, None).unwrap(), false)
            .unwrap();
        view.select_commit(at(&view, side));
        view.reset_to_selected_commit();
        assert!(view.reset_box.is_none());
        assert_eq!(head_of(&dir), (Some(head), side));
    }

    #[test]
    fn a_rebase_onto_a_branch_that_conflicts_asks_for_the_changes_page() {
        let (dir, [_base, main, side]) = diverged_repo(true);
        let head = head_of(&dir).0.unwrap();
        let mut view = view(&dir);
        let screen = draw(&mut view, 110, 24);
        let row_of = |screen: &[String], name: &str| {
            screen
                .iter()
                .position(|row| row.starts_with(&format!("   {name} ")))
                .unwrap_or_else(|| panic!("no {name} in {screen:#?}")) as u16
        };
        let right_press = |view: &mut GitLogView, row: u16| {
            view.handle_mouse(
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Right),
                    column: 4,
                    row,
                    modifiers: KeyModifiers::NONE,
                },
                WHEEL,
            )
        };
        // HEAD's branch is nothing to rebase onto; side is.
        right_press(&mut view, row_of(&screen, &head));
        assert!(!view.other_branch_selected());
        let outcome = right_press(&mut view, row_of(&screen, "side"));
        assert_eq!(outcome.menu, Some(GitLogMenu::Branch));
        assert!(view.other_branch_selected());

        view.rebase_onto_selected_branch();
        // Nothing else starts meanwhile.
        view.checkout_selected_branch();
        assert_eq!(
            notice_of(&mut view).as_deref(),
            Some("A rebase is under way")
        );
        wait(&mut view);
        assert_eq!(
            notice_of(&mut view),
            Some(format!(
                "Rebasing {head} onto side stopped at commit 1 of 1 with conflicts in 1 file: resolve and stage them, then continue"
            ))
        );
        assert!(view.take_show_changes());
        assert!(!view.take_show_changes(), "asked for once");
        let repo = Repository::open(dir.path()).unwrap();
        assert_eq!(repo.state(), git2::RepositoryState::RebaseMerge);
        assert_eq!(repo.head().unwrap().target(), Some(side));

        // Another rebase, or a merge or reset, waits for it. HEAD is
        // detached at side meanwhile, so it is main's commit that is
        // another.
        wait(&mut view);
        let position = view.history.as_ref().unwrap().position(main).unwrap();
        view.select_commit(position);
        view.rebase_onto_selected_commit();
        wait(&mut view);
        let notice = notice_of(&mut view).unwrap();
        let refused = format!(
            "Could not rebase onto {}: a rebase is in progress",
            short_id(main)
        );
        assert!(notice.starts_with(&refused), "{notice}");
        view.reset_to_selected_commit();
        let notice = notice_of(&mut view).unwrap();
        assert!(notice.contains("a rebase is in progress"), "{notice}");
    }

    #[test]
    fn a_branch_in_the_sidebar_is_checked_out_or_deleted() {
        let (dir, ids) = repo_with_history();
        let [_a, b, s, m] = ids[..] else { panic!() };
        let main = head_of(&dir).0.unwrap();
        let mut view = view(&dir);
        let screen = draw(&mut view, 110, 24);
        let right_press = |view: &mut GitLogView, row: u16| {
            view.handle_mouse(
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Right),
                    column: 4,
                    row,
                    modifiers: KeyModifiers::NONE,
                },
                WHEEL,
            )
        };
        let side_row = |screen: &[String], name: &str| {
            screen
                .iter()
                .position(|row| row.starts_with(&format!("   {name} ")))
                .unwrap_or_else(|| panic!("no {name} in {screen:#?}")) as u16
        };
        // With the keyboard in the log, the sidebar's branch is nothing
        // to act on.
        assert_eq!(view.pane, Pane::Log);
        assert!(!view.can_checkout_branch());
        // A right press on a heading asks for nothing; on a branch it
        // selects it, without going to its commit.
        assert_eq!(right_press(&mut view, 0).menu, None);
        let outcome = right_press(&mut view, side_row(&screen, "side"));
        assert_eq!(outcome.menu, Some(GitLogMenu::Branch));
        assert_eq!(view.pane, Pane::Sidebar);
        assert_eq!(view.selected_commit(), Some(m));
        assert!(view.can_checkout_branch());
        assert!(view.can_delete_branch());

        // Checking it out puts HEAD on it.
        view.checkout_selected_branch();
        assert!(
            view.hint().text().starts_with("checking out side…"),
            "{}",
            view.hint()
        );
        wait(&mut view);
        assert_eq!(notice_of(&mut view).as_deref(), Some("Checked out side"));
        assert_eq!(head_of(&dir), (Some("side".to_owned()), s));
        wait(&mut view);
        // HEAD's branch can't be deleted.
        assert!(view.can_checkout_branch());
        assert!(!view.can_delete_branch());

        // main has commits side hasn't: the box asks first, and No
        // keeps it.
        view.handle_key(key(KeyCode::Up), &mut Clipboard::new());
        assert!(view.can_delete_branch());
        view.delete_selected_branch();
        assert_eq!(view.hint(), StatusLine::help(DELETE_HELP));
        let screen = draw(&mut view, 110, 24);
        let asked = format!("Delete branch {main}?");
        assert!(screen.iter().any(|r| r.contains(&asked)), "{screen:#?}");
        view.handle_key(key(KeyCode::Char('n')), &mut Clipboard::new());
        assert!(view.branch_delete.is_none());
        let repo = Repository::open(dir.path()).unwrap();
        assert!(repo.find_branch(&main, BranchType::Local).is_ok());
        // Yes deletes it.
        view.delete_selected_branch();
        view.handle_key(key(KeyCode::Char('y')), &mut Clipboard::new());
        assert_eq!(
            notice_of(&mut view),
            Some(format!("Deleted branch {main} (was {})", short_id(m)))
        );
        assert!(repo.find_branch(&main, BranchType::Local).is_err());
        wait(&mut view);

        // A remote's branch can be checked out, not deleted: with no
        // local branch of its name, one is made tracking it.
        let screen = draw(&mut view, 110, 24);
        let origin = screen
            .iter()
            .position(|row| row.contains("▸ origin"))
            .unwrap() as u16;
        click(&mut view, 4, origin);
        let screen = draw(&mut view, 110, 24);
        let outcome = right_press(&mut view, side_row(&screen, &format!("  {main}")));
        assert_eq!(outcome.menu, Some(GitLogMenu::Branch));
        assert!(view.can_checkout_branch());
        assert!(!view.can_delete_branch());
        view.delete_selected_branch();
        assert!(view.branch_delete.is_none());
        assert!(notice_of(&mut view).is_none());
        view.checkout_selected_branch();
        wait(&mut view);
        assert_eq!(
            notice_of(&mut view),
            Some(format!(
                "Checked out new branch {main} tracking origin/{main}"
            ))
        );
        assert_eq!(head_of(&dir), (Some(main.clone()), m));
        wait(&mut view);

        // side is merged into main now: deleted at once.
        let screen = draw(&mut view, 110, 24);
        right_press(&mut view, side_row(&screen, "side"));
        view.delete_selected_branch();
        assert!(view.branch_delete.is_none());
        assert_eq!(
            notice_of(&mut view),
            Some(format!("Deleted branch side (was {})", short_id(s)))
        );
        assert!(repo.find_branch("side", BranchType::Local).is_err());
        let _ = b;
    }

    #[test]
    fn a_few_paths_are_named_and_the_rest_counted() {
        let paths = |names: &[&str]| -> Vec<String> {
            names.iter().map(|name| (*name).to_owned()).collect()
        };
        assert_eq!(name_some(&paths(&["a"])), "a");
        assert_eq!(name_some(&paths(&["a", "b"])), "a and b");
        assert_eq!(name_some(&paths(&["a", "b", "c"])), "a, b and c");
        assert_eq!(
            name_some(&paths(&["a", "b", "c", "d", "e"])),
            "a, b, c and 2 more"
        );
        let request = RestoreRequest {
            commit: Oid::ZERO_SHA1,
            version: Version::Commit,
            source: None,
            paths: Vec::new(),
            what: "src/".to_owned(),
            kept: 2,
        };
        let done = Restored {
            written: 1,
            deleted: 2,
        };
        assert_eq!(
            restored_summary(&request, done),
            "Restored src/ as of 00000000: 2 files deleted, as that version hasn't it; 2 submodules left alone"
        );
    }
}
