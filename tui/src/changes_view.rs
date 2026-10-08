//! The changes page (Ctrl+U): a mode, like the git log, that stands in
//! for the editor in the upper part of the screen and shows what is
//! changed in the working tree and not yet committed, for reviewing
//! the changes, staging them, and committing them; and, after a merge
//! or rebase that stopped at conflicts, for seeing which files are in
//! conflict and committing the merge, or continuing the rebase, once
//! they are resolved. The git log page brings it up when one of its
//! merges or rebases stops so.
//!
//! The page is kept when it is left (for the editor, or another mode)
//! and comes back as it was, as the git log page does, so that a review
//! can go to the editor to look something up and carry on where it
//! was: the same file selected in the same list, the diff as it was
//! read (its place kept even when the file was edited meanwhile; see
//! below), and the commit message as far as it was written. Coming back
//! scans the working tree again, keeping the selection by path.
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
//! round (the lists, the diff, the commit box; Shift+Tab from the diff
//! goes back to the list it shows a file of), and clicking a pane
//! moves it there. The arrows never leave a pane, as in a graphical
//! program: in a list ↑ and ↓ move between its rows and stop at its
//! ends; ← and → fold and unfold a directory (← on a file goes to its
//! directory); Space stages what is selected, a file
//! or a whole directory (or unstages it, in the staged list), and `a`
//! stages or unstages everything; Enter on a file moves into its diff,
//! and Enter on a directory folds or unfolds it; `o` opens the file in
//! the editor, at its first conflict if it has one; and `m` toggles
//! amending. Ctrl+S, from any pane, makes the commit with the message
//! in the box, as the person the repository's configuration names; a
//! merge in progress starts with the message git prepared for it. The
//! diff has a cursor and a selection, as the editor has, to read it by
//! and copy from it (see the `diff_pane` module), with a scrollbar down
//! the side saying where in the diff the pane is and how much of it
//! shows; `o` there opens the file at the line the cursor is on.
//! Space in the diff stages the changes on the selected lines (or on
//! the line under the cursor, with nothing selected), as Space on a file
//! in its list stages the file, or unstages them in a staged file's
//! diff: the core crate's `Changes::apply_lines` writes what the diff
//! works out for them, and refuses if the file changed since the diff
//! was made. A right click in the diff opens a menu of all of that, with
//! copying the selection (as shown, as a patch, or as the new side has
//! it), selecting it all, and, set apart since it
//! loses work, reverting the selected lines of an unstaged file to what
//! is staged, after asking. A
//! scan builds the diff again, since the file may have changed, and the
//! new diff keeps the reader's place in the old one (see the core
//! crate's `git::diff_model` module): the context revealed, and the
//! cursor and selection on the same lines, which stay where they were
//! on the screen when lines come or go above them. The wheel scrolls
//! whichever pane it is over.
//!
//! A right click on a file or directory in a list selects it and opens
//! a menu of what can be done to it (the application draws the menu;
//! see the `context_menu` module): open it, stage or unstage it, and,
//! set apart below those in the unstaged list, resolve its conflicts
//! and discard its changes.
//!
//! "Resolve using ours" and "Resolve using theirs" (from the menu or
//! the command palette) settle a file in conflict, or every one under a
//! directory, by taking one side's version whole, as `git checkout
//! --ours` (or `--theirs`) and `git add` do: the file is written as
//! that side has it, or deleted if that side deleted it, and staged.
//! Files under the directory that aren't in conflict are left alone.
//! Ours and theirs are git's names, which in a rebase are easy to have
//! backwards (ours is what is being rebased onto), so the box that asks
//! first names whose version each is. It asks because what the file
//! has now, conflict markers and any resolving done in it by hand, is
//! lost; `y` or its Resolve button goes ahead.
//!
//! Discarding (from the menu or the command palette) throws away the
//! unstaged changes of a file, or of every file under a directory: each
//! goes back to what is staged, or with nothing staged to what was last
//! committed, and an untracked file is deleted. There is no undoing
//! that, so a box asks first, saying what will happen, and `y` or its
//! Discard button goes ahead (Enter alone presses Cancel, so an Enter
//! pressed once too often loses nothing). A file in conflict has no one
//! version to go back to, and a submodule's changes are its own to
//! discard on its tab, so both are left alone, and the box says so.
//!
//! While a rebase is in progress the commit box is where it goes on
//! from: its heading says which of the rebase's commits it stopped at
//! (`Rebase main onto origin/main: 2 of 5`), the box starts with that
//! commit's message, and Ctrl+S (or "Continue rebase") commits it,
//! keeping its author, and replays the rest, as `git rebase --continue`
//! does; that may stop at conflicts again. Continuing (or committing a
//! merge) first tries again to resolve submodule conflicts by
//! themselves, so that one whose own branch has been rebased since the
//! stop, as the status bar advised, needs nothing more. "Abort merge" and "Abort
//! rebase" in the command palette give up the one in progress, putting
//! the branch and its files back as they were before it began; that
//! loses whatever was resolved, so a box asks first.
//!
//! An interactive rebase (the git log's "Edit commit", or one planned
//! in its rebase dialog; see the core crate's `git::interactive`
//! module) stops here to edit a commit, with
//! HEAD at what it is replayed onto and the commit's changes staged,
//! the heading saying so (`Edit 1a2b3c4d on main: 1 of 3`) and the box
//! starting with its message. Ctrl+S commits what is staged, keeping
//! the commit's author, and the rebase goes on by itself once nothing
//! is left unstaged: so the commit is reworded by changing the message,
//! changed by staging more, and split by unstaging some of it, then
//! committing what is left staged and the rest after. With nothing
//! staged Ctrl+S commits nothing, never an empty commit, and goes on if
//! nothing is unstaged either (the commit's changes all discarded,
//! say), or says what is left to stage or discard. A squash stops the
//! same way (`Squash 1a2b3c4d on main: 3 of 5`), with the commits it
//! folds together staged and their messages in the box, to be made
//! one. An untracked file holds it up only when the commit added it
//! (unstaged from it, to split it off); others are left alone, as git
//! leaves them. A step
//! that conflicts stops as a plain rebase's does, and its resolution is
//! committed the same way.
//!
//! On arriving at a merge or rebase's stop, and again after each commit
//! made there, the keyboard goes where the next step is done: to the
//! first file in conflict, if any; else, with everything staged, to the
//! commit message; else to the first unstaged file. Staging and the
//! like at the same stop, or coming back to the page, leave it where the
//! user put it.
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
//! every stage, unstage, resolve, discard, and commit; the scan runs on a worker thread
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
//!
//! A commit runs the repository's hooks as `git commit` does (see the
//! core crate's `git::hooks` module). With any to run, it is made in
//! the background, the status bar naming the hook running, which
//! Escape cancels; the page's other actions are refused until it is
//! done. A hook is never seen unless it fails: then the hook box (see
//! the `hook_box` module) shows what it wrote. One that stopped the
//! commit leaves the message and what is staged as they were, and
//! offers to commit anyway, as `git commit --no-verify` does; one run
//! after the commit (`post-commit`) stopped nothing, and only says so.

use crate::clipboard::Clipboard;
use crate::confirm_box::{ConfirmBox, ConfirmOutcome};
use crate::diff_pane::{
    self, ContentPane, CopyAs, Piece, Shown, ShownMut, TAB_WIDTH, clamp_between, display_width,
    fit_end, share_for, share_of,
};
use crate::editor_view::EditorView;
use crate::git_layout::{ChangesSizes, GitChangesLayout, MAIN_REPOSITORY};
use crate::hook_box::{HookBox, HookOutcome};
use crate::palette::palette_background;
use crate::status::StatusLine;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ninjaedit_core::git::{
    ChangeKind, Changes, CommitError, Committed, ConflictSide, Continued, DiffLine, DiffModel,
    FileChange, FileDiff, FileTree, HookError, Hooks, InProgress, LinesChange, LinesTarget, Oid,
    RebaseAction, TreeRow, short_id,
};
use ninjaedit_core::{Editor, FileBuffer};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::{Color, Modifier, Style};
use std::collections::{BTreeSet, HashSet};
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
/// The discard box's button.
const DISCARD: &str = "Discard";
/// The revert box's button.
const REVERT: &str = "Revert";
/// The resolve box's button.
const RESOLVE: &str = "Resolve";
/// The abort box's button.
const ABORT: &str = "Abort";
/// The key bindings the status bar lists, pane by pane.
const UNSTAGED_HELP: &[(&str, &str)] = &[
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
    ("", "Type the message"),
    ("Ctrl+S", "commit"),
    ("Tab", "pane"),
    ("Ctrl+E", "leave"),
];
const DISCARD_HELP: &[(&str, &str)] = &[("y", "discard"), ("n/Esc", "cancel")];
const RESOLVE_HELP: &[(&str, &str)] = &[("y", "resolve"), ("n/Esc", "cancel")];
const ABORT_HELP: &[(&str, &str)] = &[("y", "abort"), ("n/Esc", "cancel")];
const REVERT_HELP: &[(&str, &str)] = &[("y", "revert"), ("n/Esc", "cancel")];
const HOOK_HELP: &[(&str, &str)] = &[("↑↓", "scroll"), ("Tab", "button"), ("Esc", "dismiss")];
/// The hook box's button, for a hook that stopped a commit.
const COMMIT_ANYWAY: &str = "Commit anyway";
/// The diff of an unstaged file, and of a staged one.
const CONTENT_HELP: &[(&str, &str)] = &[
    ("Space", "stage lines"),
    ("Enter", "expand"),
    ("o", "open"),
    ("Tab", "pane"),
    ("Ctrl+E", "leave"),
];
const STAGED_CONTENT_HELP: &[(&str, &str)] = &[
    ("Space", "unstage lines"),
    ("Enter", "expand"),
    ("o", "open"),
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
    Diff(Box<DiffModel>),
    Failed(String),
}

/// What the diff pane is to draw for `content`.
fn shown(content: Option<&Content>) -> Shown<'_> {
    match content {
        None => Shown::Nothing,
        Some(Content::Message(message) | Content::Failed(message)) => Shown::Note(message),
        Some(Content::Directory(lines)) => Shown::Lines(lines),
        Some(Content::Diff(model)) => Shown::Diff(model),
    }
}

/// How the status bar and the revert box name a file's lines: `3 lines
/// of src/a.rs`.
fn lines_of(change: &LinesChange) -> String {
    format!(
        "{} of {}",
        count_of(change.lines, "line", "lines"),
        change.path
    )
}

/// What staging, unstaging, or reverting lines says when there are none
/// to act on: in the diff of a file of the right list, that the
/// selection (or the cursor's line) has no changes; elsewhere, nothing.
fn nothing_to(in_diff: bool, verb: &str) -> ChangesOutcome {
    if !in_diff {
        return ChangesOutcome::Continue;
    }
    ChangesOutcome::Notice(StatusLine::info(format!(
        "Nothing to {verb}: select changed lines, or put the cursor on one"
    )))
}

/// Drop what the diff pane shows, for it to be built again on the next
/// draw, keeping a diff as `stale`, to put the new one's cursor where
/// its was.
fn drop_content(content: &mut Option<Content>, stale: &mut Option<Box<DiffModel>>) {
    if let Some(Content::Diff(model)) = content.take() {
        *stale = Some(model);
    }
}

/// [`shown`], for input that may move the cursor in a diff, or expand
/// it.
fn shown_mut(content: Option<&mut Content>) -> ShownMut<'_> {
    match content {
        None => ShownMut::Nothing,
        Some(Content::Message(message) | Content::Failed(message)) => ShownMut::Note(message),
        Some(Content::Directory(lines)) => ShownMut::Lines(lines),
        Some(Content::Diff(model)) => ShownMut::Diff(model),
    }
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
    /// its first merge conflict, and a `line` (from zero) for it there.
    OpenFile {
        path: PathBuf,
        conflicted: bool,
        line: Option<usize>,
    },
}

/// What the application should do after the page handled a mouse
/// event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangesMouseOutcome {
    /// The pane sizes changed (a drag of a rule ended), so the layout
    /// is worth keeping.
    pub resized: bool,
    /// A right press on a row of a list, which it selected: open the
    /// page's menu at the pointer, for that row.
    pub menu: bool,
    /// A right press in a diff: open the menu for its selection at the
    /// pointer.
    pub diff_menu: bool,
    /// Something to say in the status bar: a discard confirmed with a
    /// click, an action that failed.
    pub notice: Option<StatusLine>,
}

/// A commit being made on its own thread while its hooks run (see the
/// core crate's `git::hooks` module): the hooks, to say which is
/// running and to cancel it, and the message it was given, to clear
/// from the box once made unless the user has changed it meanwhile.
struct Committing {
    hooks: Hooks,
    message: String,
}

/// What a box asking about files of the unstaged list will do to them
/// once answered yes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FilesAction {
    Discard,
    /// Resolve their conflicts by taking a side.
    Resolve(ConflictSide),
    /// Revert some of a file's lines.
    RevertLines,
}

/// Unstaged files the discard or resolve box is asking about: what the
/// box asks, and what to do to which paths once it is answered yes.
struct PendingFiles {
    dialog: ConfirmBox,
    action: FilesAction,
    paths: Vec<String>,
    /// What is being acted on, for the status bar: a file's path, or a
    /// directory's with a slash, or a file's lines.
    what: String,
    /// The lines to revert, worked out when the box was opened.
    lines: Option<LinesChange>,
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

    /// Select the first file, in the order the tree shows them, that
    /// `wanted` accepts (by its index in the files the tree was built
    /// from), unfolding the directories it is in. Returns whether there
    /// was one.
    fn select_first(&mut self, wanted: impl Fn(usize) -> bool) -> bool {
        let found = self.tree.rows(&[]).into_iter().find_map(|row| match row {
            TreeRow::File { file, parent, .. } if wanted(file) => Some((file, parent)),
            _ => None,
        });
        let Some((file, mut dir)) = found else {
            return false;
        };
        while let Some(at) = dir {
            self.set_dir_collapsed(at, false);
            dir = self.tree.dirs()[at].parent;
        }
        let row = self
            .rows
            .iter()
            .position(|row| matches!(row, TreeRow::File { file: f, .. } if *f == file));
        if let Some(row) = row {
            self.select(row);
        }
        row.is_some()
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
    /// The working directory of a submodule whose tab is to be shown
    /// once the main repository's scan lists it (see
    /// [`show_repository`](Self::show_repository)).
    wanted: Option<PathBuf>,
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
            wanted: None,
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
        let mut acted = None;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            if let Some(view) = &mut tab.view
                && view.poll()
            {
                changed = true;
                main_changed |= index == 0;
                // A commit whose hooks ran has been made.
                if view.take_acted() {
                    acted = Some(index);
                }
            }
        }
        if let Some(acted) = acted {
            for (index, tab) in self.tabs.iter_mut().enumerate() {
                if index != acted
                    && let Some(view) = &mut tab.view
                {
                    view.refresh();
                }
            }
        }
        if main_changed {
            self.sync_tabs();
        }
        changed
    }

    /// What the status bar is to say about something a page finished
    /// between events (a commit whose hooks ran), once.
    pub fn take_notice(&mut self) -> Option<StatusLine> {
        self.tabs
            .iter_mut()
            .filter_map(|tab| tab.view.as_mut())
            .find_map(ChangesView::take_notice)
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
        if let Some(wanted) = self.wanted.take()
            && let Some(index) = self.tabs.iter().position(|tab| tab.workdir == wanted)
        {
            self.set_active(index);
        }
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

    /// Stage what is selected in the shown page's unstaged list, as its
    /// Space does.
    pub fn stage_selected(&mut self) -> ChangesOutcome {
        let outcome = self.active().stage_selected();
        self.refresh_others();
        outcome
    }

    /// Unstage what is selected in the shown page's staged list, as its
    /// Space does.
    pub fn unstage_selected(&mut self) -> ChangesOutcome {
        let outcome = self.active().unstage_selected();
        self.refresh_others();
        outcome
    }

    /// Ask whether to discard the unstaged changes selected on the
    /// shown page; the page discards them once answered yes.
    pub fn discard_selected(&mut self) -> ChangesOutcome {
        self.active().discard_selected()
    }

    /// Ask whether to resolve the conflicts selected on the shown page
    /// by taking `side`; the page resolves them once answered yes.
    pub fn resolve_selected(&mut self, side: ConflictSide) -> ChangesOutcome {
        self.active().resolve_selected(side)
    }

    /// Ask whether to give up the shown page's merge or rebase in
    /// progress; the page does once answered yes.
    pub fn abort(&mut self) -> ChangesOutcome {
        self.active().abort()
    }

    /// Show the tab of the repository whose working directory is
    /// `workdir`: now if it has one, otherwise once the main
    /// repository's next scan lists it among the submodules with
    /// changes (a merge stopped at conflicts in one has them).
    pub fn show_repository(&mut self, workdir: &Path) {
        match self.tabs.iter().position(|tab| tab.workdir == workdir) {
            Some(index) => self.set_active(index),
            None => self.wanted = Some(workdir.to_path_buf()),
        }
    }

    /// Open the file selected on the shown page, as its `o` does, or
    /// show the selected submodule's tab.
    pub fn open_selected(&mut self) -> ChangesOutcome {
        let outcome = self.active().open_selected();
        self.follow_submodule(outcome)
    }

    /// Open the file of the shown page's diff at the line its cursor is
    /// on, as `o` in the diff does.
    pub fn open_at_cursor(&mut self) -> ChangesOutcome {
        self.active().open_at_cursor()
    }

    /// Stage the lines selected in the shown page's diff of an unstaged
    /// file, as Space there does.
    pub fn stage_lines(&mut self) -> ChangesOutcome {
        let outcome = self.active().stage_lines();
        self.refresh_others();
        outcome
    }

    /// Unstage the lines selected in the shown page's diff of a staged
    /// file, as Space there does.
    pub fn unstage_lines(&mut self) -> ChangesOutcome {
        let outcome = self.active().unstage_lines();
        self.refresh_others();
        outcome
    }

    /// Ask whether to revert the lines selected in the shown page's diff
    /// of an unstaged file; the page reverts them once answered yes.
    pub fn revert_lines(&mut self) -> ChangesOutcome {
        self.active().revert_lines()
    }

    /// Give the shown page a mouse event. A change to its pane sizes (a
    /// drag of a rule ended) goes into the layout, which is then worth
    /// keeping. `wheel` is how many rows (or columns, sideways) a wheel
    /// event scrolls.
    pub fn handle_mouse(&mut self, mouse: MouseEvent, wheel: usize) -> ChangesMouseOutcome {
        let active = self.active;
        let outcome = self.active().handle_mouse(mouse, wheel);
        if outcome.resized {
            let sizes = self.tabs[active].view.as_ref().map(ChangesView::sizes);
            if let Some(sizes) = sizes {
                let key = self.tabs[active].key().to_owned();
                self.layout.set(&key, sizes);
            }
        }
        self.refresh_others();
        outcome
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
    /// The diff the content was before it was dropped to be built
    /// again, for the new one to put its cursor where this one's was
    /// when it is of the same file.
    stale: Option<Box<DiffModel>>,
    content_pane: ContentPane,
    /// The commit message, in an editor of its own.
    message: EditorView,
    /// Text the page put in the message itself (a merge's prepared
    /// message, the amended commit's), which it may replace or clear
    /// again while the user hasn't changed it.
    auto_message: Option<String>,
    /// The commit an interactive rebase stopped at that a commit has
    /// been made at already, the first part of a split: the stopped
    /// commit's message was for that one, and isn't offered again.
    committed_at: Option<Oid>,
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
    /// The discard or resolve box, while it asks whether to go ahead.
    pending: Option<PendingFiles>,
    /// The abort box, while it asks whether to give up the merge or
    /// rebase in progress.
    abort: Option<ConfirmBox>,
    /// The commit being made while its hooks run, if any.
    committing: Option<Committing>,
    /// The hook box, while it shows a hook that failed.
    hook_box: Option<HookBox>,
    /// What the status bar is to say about something that finished
    /// between events (a commit whose hooks ran), until taken.
    notice: Option<StatusLine>,
    /// The stop of the merge or rebase in progress that the keyboard
    /// was last put where it is next needed for (see
    /// [`place_for_stop`](Self::place_for_stop)): HEAD's commit then,
    /// and the rebase's step; `None` with neither in progress.
    placed_for: Option<(Option<Oid>, usize)>,
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
            stale: None,
            content_pane: ContentPane::default(),
            message: message_editor(""),
            auto_message: None,
            committed_at: None,
            files_share: None,
            unstaged_share: None,
            commit_share: None,
            divider_drag: None,
            acted: false,
            submodule_to_show: None,
            pending: None,
            abort: None,
            committing: None,
            hook_box: None,
            notice: None,
            placed_for: None,
            area: Rect::default(),
            unstaged_area: Rect::default(),
            staged_area: Rect::default(),
            commit_area: Rect::default(),
            amend_area: Rect::default(),
            content_area: Rect::default(),
            files_rule: Rect::default(),
            lists_rule: Rect::default(),
            commit_rule: Rect::default(),
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
        if let Some(result) = changes.take_commit()
            && let Some(Committing { hooks, message }) = self.committing.take()
        {
            let outcome = match result {
                Ok(committed) => self.committed(committed, &message, &hooks),
                Err(err) => self.commit_failed(err),
            };
            if let ChangesOutcome::Notice(notice) = outcome {
                self.notice = Some(notice);
            }
        }
        self.follow_lists();
        self.place_for_stop();
        true
    }

    /// What the status bar is to say about something that finished
    /// between events, once.
    pub fn take_notice(&mut self) -> Option<StatusLine> {
        self.notice.take()
    }

    /// Put the keyboard where the next step of a merge or rebase in
    /// progress is done, the first time a scan shows where it has
    /// stopped: on arriving at the page with one stopped, and after
    /// each commit made there (HEAD has moved, or the rebase has gone
    /// on to another step). A file in conflict is the first thing to
    /// do, so the first one is selected; with none, and everything
    /// staged, the commit message is next; otherwise the first of the
    /// unstaged files, to stage or discard. A scan of the same stop
    /// (after staging a file, or coming back to the page) leaves the
    /// keyboard where the user put it.
    fn place_for_stop(&mut self) {
        let Some(changes) = &self.changes else {
            return;
        };
        let stop = (changes.is_merging() || changes.rebase().is_some()).then(|| {
            (
                changes.head_id(),
                changes.rebase().map_or(0, |rebase| rebase.step),
            )
        });
        if stop == self.placed_for {
            return;
        }
        self.placed_for = stop;
        if stop.is_none() {
            return;
        }
        let conflicted: Vec<bool> = changes
            .unstaged()
            .iter()
            .map(|change| change.kind == ChangeKind::Conflicted)
            .collect();
        let any_conflict = conflicted.contains(&true);
        let selected = !conflicted.is_empty()
            && self
                .unstaged
                .select_first(|file| !any_conflict || conflicted[file]);
        if selected {
            self.list = List::Unstaged;
            self.pane = Pane::Unstaged;
        } else {
            self.pane = Pane::Commit;
        }
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
        drop_content(&mut self.content, &mut self.stale);
        // A rebase's commit keeps its message unless another is written,
        // but a later commit at the same stop is another one.
        let rebase_message = changes
            .rebase()
            .filter(|rebase| {
                rebase
                    .stop
                    .is_none_or(|(_, at)| Some(at) != self.committed_at)
            })
            .and_then(|rebase| rebase.message.as_deref());
        let merge_message = match changes.merge_message().or(rebase_message) {
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
        if let Some(committing) = &self.committing {
            return StatusLine::progress(match committing.hooks.running() {
                Some(hook) => format!("committing: running the {hook} hook… (Esc cancels)"),
                None => "committing…".to_owned(),
            });
        }
        if self.hook_box.is_some() {
            return StatusLine::help(HOOK_HELP);
        }
        if self.scanning() {
            return StatusLine::progress(SCANNING);
        }
        if let Some(pending) = &self.pending {
            return StatusLine::help(match pending.action {
                FilesAction::Discard => DISCARD_HELP,
                FilesAction::Resolve(_) => RESOLVE_HELP,
                FilesAction::RevertLines => REVERT_HELP,
            });
        }
        if self.abort.is_some() {
            return StatusLine::help(ABORT_HELP);
        }
        StatusLine::help(match self.pane {
            Pane::Unstaged => UNSTAGED_HELP,
            Pane::Staged => STAGED_HELP,
            Pane::Commit => COMMIT_HELP,
            Pane::Content if self.shown_list() == Some(List::Staged) => STAGED_CONTENT_HELP,
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
        let stale = self.stale.take();
        if self.shown != wanted {
            self.content_pane.reset();
        }
        let same = self.shown == wanted;
        self.shown = wanted;
        let Some(changes) = &self.changes else {
            return;
        };
        let scanning = self.scanning();
        // Where the cursor of a diff built again was, and is now.
        let mut moved = None;
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
                        Ok(diff) => {
                            let mut model = DiffModel::new(diff, TAB_WIDTH);
                            // The same file's diff built again keeps the
                            // reader's place: see the core crate's
                            // `git::diff_model` module.
                            if let Some(stale) = stale.filter(|_| same) {
                                model.carry_place_from(&stale);
                                moved = Some((stale.cursor().line, model.cursor().line));
                            }
                            Content::Diff(Box::new(model))
                        }
                        Err(err) => Content::Failed(err.message().to_owned()),
                    }
                }
                None => Content::Message(String::new()),
            },
        });
        // Rows added or gone above the cursor move the view as far, so
        // its line stays where it was on the screen.
        if let Some((from, to)) = moved {
            self.content_pane.keep_on_screen(from, to);
        }
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
        let what = self.describe_row(list, row);
        self.apply(list, &files, &what)
    }

    /// How the status bar names what a row stands for: a file's path,
    /// or a directory's with a slash.
    fn describe_row(&self, list: List, row: TreeRow) -> String {
        match self.file_list(list).key_of(row) {
            RowKey::Dir(path) => format!("{path}/"),
            RowKey::File(path) => path,
        }
    }

    /// The command palette's and the menu's "Stage changes": Space in
    /// the unstaged list.
    pub fn stage_selected(&mut self) -> ChangesOutcome {
        if self.list != List::Unstaged {
            return ChangesOutcome::Continue;
        }
        self.toggle_selected(List::Unstaged)
    }

    /// The command palette's and the menu's "Unstage changes": Space in
    /// the staged list.
    pub fn unstage_selected(&mut self) -> ChangesOutcome {
        if self.list != List::Staged {
            return ChangesOutcome::Continue;
        }
        self.toggle_selected(List::Staged)
    }

    /// Whether a row of the unstaged list is selected, in the list the
    /// diff follows, to stage.
    pub fn can_stage_selected(&self) -> bool {
        self.list == List::Unstaged && self.unstaged.selected_row().is_some()
    }

    /// Whether a row of the staged list is selected, in the list the
    /// diff follows, to unstage.
    pub fn can_unstage_selected(&self) -> bool {
        self.list == List::Staged && self.staged.selected_row().is_some()
    }

    /// Whether what is selected in the unstaged list, in the list the
    /// diff follows, has changes to discard.
    pub fn can_discard_selected(&self) -> bool {
        self.list == List::Unstaged && !self.discardable_selection().0.is_empty()
    }

    /// The files the unstaged list's selection stands for, split into
    /// those whose changes can be discarded and those that can't: files
    /// in conflict, which have no one version to go back to, and
    /// submodules, whose changes are theirs to discard.
    fn discardable_selection(&self) -> (Vec<FileChange>, Vec<FileChange>) {
        let Some(row) = self.unstaged.selected_row() else {
            return (Vec::new(), Vec::new());
        };
        let files = self.files(List::Unstaged);
        self.unstaged
            .files_of(row)
            .into_iter()
            .filter_map(|f| files.get(f).cloned())
            .partition(|change| change.kind != ChangeKind::Conflicted && !change.submodule)
    }

    /// The command palette's and the menu's "Discard changes": ask, in
    /// the discard box, whether to throw away the unstaged changes of
    /// what is selected in the unstaged list, a file or every file
    /// under a directory. The box's yes discards them (see
    /// [`finish_pending`](Self::finish_pending)). Conflicts and
    /// submodules under a directory are left alone, and the box says so.
    pub fn discard_selected(&mut self) -> ChangesOutcome {
        if self.list != List::Unstaged {
            return ChangesOutcome::Continue;
        }
        let Some(row) = self.unstaged.selected_row() else {
            return ChangesOutcome::Continue;
        };
        let what = self.describe_row(List::Unstaged, row);
        let (files, kept) = self.discardable_selection();
        if files.is_empty() {
            return ChangesOutcome::Notice(StatusLine::info(format!(
                "Nothing to discard in {what}: conflicts are resolved, and submodules discarded on their own tabs"
            )));
        }
        let staged: HashSet<&str> = self
            .files(List::Staged)
            .iter()
            .map(|change| change.path.as_str())
            .collect();
        let body = discard_body(&files, &kept, &staged);
        let dialog = ConfirmBox::new(format!("Discard changes to {what}?"), body, DISCARD);
        self.pending = Some(PendingFiles {
            dialog,
            action: FilesAction::Discard,
            paths: files.into_iter().map(|change| change.path).collect(),
            what,
            lines: None,
        });
        ChangesOutcome::Continue
    }

    /// Whether what is selected in the unstaged list, in the list the
    /// diff follows, has files in conflict to resolve.
    pub fn can_resolve_selected(&self) -> bool {
        self.list == List::Unstaged && !self.conflicted_selection().is_empty()
    }

    /// The files in conflict among those the unstaged list's selection
    /// stands for, submodules included.
    fn conflicted_selection(&self) -> Vec<FileChange> {
        let Some(row) = self.unstaged.selected_row() else {
            return Vec::new();
        };
        let files = self.files(List::Unstaged);
        self.unstaged
            .files_of(row)
            .into_iter()
            .filter_map(|f| files.get(f))
            .filter(|change| change.kind == ChangeKind::Conflicted)
            .cloned()
            .collect()
    }

    /// The command palette's and the menu's "Resolve using ours" and
    /// "Resolve using theirs": ask, in the resolve box, whether to
    /// resolve the conflicts of what is selected in the unstaged list,
    /// a file or every conflicted file under a directory, by taking
    /// `side`'s version. The box's yes resolves them (see
    /// [`finish_pending`](Self::finish_pending)). Files under a
    /// directory that aren't in conflict are left alone.
    pub fn resolve_selected(&mut self, side: ConflictSide) -> ChangesOutcome {
        if self.list != List::Unstaged {
            return ChangesOutcome::Continue;
        }
        let Some(row) = self.unstaged.selected_row() else {
            return ChangesOutcome::Continue;
        };
        let Some(changes) = &self.changes else {
            return ChangesOutcome::Continue;
        };
        let what = self.describe_row(List::Unstaged, row);
        let files = self.conflicted_selection();
        if files.is_empty() {
            return ChangesOutcome::Notice(StatusLine::info(format!(
                "Nothing to resolve in {what}: it has no conflicts"
            )));
        }
        let title = match &files[..] {
            [file] => format!("Resolve {} using {}?", file.path, side_name(side)),
            _ => format!("Resolve conflicts in {what} using {}?", side_name(side)),
        };
        let body = resolve_body(changes, side, &files);
        let dialog = ConfirmBox::new(title, body, RESOLVE);
        self.pending = Some(PendingFiles {
            dialog,
            action: FilesAction::Resolve(side),
            paths: files.into_iter().map(|change| change.path).collect(),
            what,
            lines: None,
        });
        ChangesOutcome::Continue
    }

    /// The discard or resolve box was answered yes: do what it asked
    /// about.
    fn finish_pending(&mut self) -> ChangesOutcome {
        let Some(pending) = self.pending.take() else {
            return ChangesOutcome::Continue;
        };
        if let Some(change) = &pending.lines {
            return self.apply_lines(change);
        }
        let Some(changes) = &mut self.changes else {
            return ChangesOutcome::Continue;
        };
        let paths = pending.paths.iter().map(String::as_str);
        let result = match pending.action {
            FilesAction::Discard => changes.discard(paths),
            FilesAction::Resolve(side) => changes.resolve(paths, side),
            // Its lines were worked out when the box opened, and are
            // written above.
            FilesAction::RevertLines => return ChangesOutcome::Continue,
        };
        self.acted = true;
        drop_content(&mut self.content, &mut self.stale);
        self.follow_lists();
        let what = &pending.what;
        match (pending.action, result) {
            (FilesAction::Discard, Ok(())) => {
                ChangesOutcome::Notice(StatusLine::info(format!("Discarded changes to {what}")))
            }
            (FilesAction::Discard, Err(err)) => ChangesOutcome::Notice(StatusLine::error(format!(
                "Could not discard {what}: {}",
                err.message()
            ))),
            (FilesAction::Resolve(side), Ok(())) => {
                let what = match pending.paths.len() {
                    1 => what.clone(),
                    n => format!("{n} conflicts in {what}"),
                };
                ChangesOutcome::Notice(StatusLine::info(format!(
                    "Resolved {what} using {}",
                    side_name(side)
                )))
            }
            (FilesAction::Resolve(_), Err(err)) => ChangesOutcome::Notice(StatusLine::error(
                format!("Could not resolve {what}: {}", err.message()),
            )),
            (FilesAction::RevertLines, _) => ChangesOutcome::Continue,
        }
    }

    /// Whether the discard or resolve box is asking.
    #[cfg(test)]
    fn files_box(&self) -> Option<&ConfirmBox> {
        self.pending.as_ref().map(|pending| &pending.dialog)
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
        drop_content(&mut self.content, &mut self.stale);
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
        drop_content(&mut self.content, &mut self.stale);
        if amend {
            if let Some(message) = changes.head_message().map(str::to_owned) {
                self.offer_message(message);
            }
        } else {
            self.withdraw_message();
        }
        ChangesOutcome::Continue
    }

    /// Commit what is staged with the message in the box; while a
    /// rebase is in progress, continue it instead. With hooks to run,
    /// the commit is made in the background, the status bar saying
    /// which hook is running (Escape cancels it), and how it went comes
    /// with a later [`poll`](Self::poll). A hook that stops the commit
    /// is shown in the hook box, which can make it anyway.
    pub fn commit(&mut self) -> ChangesOutcome {
        let (cols, rows) = HookBox::terminal_size(self.area);
        self.commit_with(Hooks::new(cols, rows))
    }

    fn commit_with(&mut self, hooks: Hooks) -> ChangesOutcome {
        if self.is_rebasing() {
            return self.continue_rebase();
        }
        let message = self.message_text();
        let Some(changes) = &mut self.changes else {
            return ChangesOutcome::Continue;
        };
        match changes.commit(&message, &hooks) {
            Ok(Some(committed)) => self.committed(committed, &message, &hooks),
            Ok(None) => {
                self.committing = Some(Committing { hooks, message });
                ChangesOutcome::Continue
            }
            Err(err) => self.commit_failed(err),
        }
    }

    /// A commit was made with `message` from the box: clear the box,
    /// unless the user has written in it since, follow the lists, and
    /// say so; and show a hook run after it that failed.
    fn committed(&mut self, committed: Committed, message: &str, hooks: &Hooks) -> ChangesOutcome {
        self.acted = true;
        if self.message_text() == message {
            self.message = message_editor("");
            self.auto_message = None;
        }
        self.follow_lists();
        let summary = committed.message.lines().next().unwrap_or("");
        let verb = if committed.amended {
            "Amended"
        } else {
            "Committed"
        };
        let id = short_id(committed.id);
        if let Some(failure) = hooks.take_failures().into_iter().next() {
            let body = format!("{verb} {id} all the same: the hook runs after the commit is made.");
            self.hook_box = Some(HookBox::new(failure, body, None));
        }
        ChangesOutcome::Notice(StatusLine::info(format!("{verb} {id} {summary}")))
    }

    /// A commit wasn't made: say why, showing a hook that stopped it in
    /// the hook box.
    fn commit_failed(&mut self, err: CommitError) -> ChangesOutcome {
        match err {
            CommitError::Hook(HookError::Failed(failure)) => {
                let text = format!("Could not commit: {failure}");
                let body = "Nothing was committed. What is staged stays staged.";
                self.hook_box = Some(HookBox::new(failure, body, Some(COMMIT_ANYWAY.to_owned())));
                ChangesOutcome::Notice(StatusLine::error(text))
            }
            CommitError::Hook(err @ HookError::Cancelled(_)) => {
                ChangesOutcome::Notice(StatusLine::info(format!("Commit cancelled: {err}")))
            }
            CommitError::Git(err) => ChangesOutcome::Notice(StatusLine::error(format!(
                "Could not commit: {}",
                err.message()
            ))),
        }
    }

    /// Act on the hook box's answer: close it, or commit again without
    /// the hooks git's `--no-verify` skips.
    fn finish_hook_box(&mut self, outcome: HookOutcome) -> ChangesOutcome {
        match outcome {
            HookOutcome::Continue => ChangesOutcome::Continue,
            HookOutcome::Dismiss => {
                self.hook_box = None;
                ChangesOutcome::Continue
            }
            HookOutcome::Override => {
                self.hook_box = None;
                let (cols, rows) = HookBox::terminal_size(self.area);
                self.commit_with(Hooks::new(cols, rows).without_verify())
            }
        }
    }

    /// The hook box, while it shows a failed hook.
    #[cfg(test)]
    fn hook_box(&self) -> Option<&HookBox> {
        self.hook_box.as_ref()
    }

    /// Whether a merge is in progress, to commit or abort.
    pub fn is_merging(&self) -> bool {
        self.changes.as_ref().is_some_and(Changes::is_merging)
    }

    /// Whether a rebase is in progress, to continue or abort.
    pub fn is_rebasing(&self) -> bool {
        self.changes
            .as_ref()
            .is_some_and(|changes| changes.rebase().is_some())
    }

    /// "Continue rebase", and Ctrl+S while a rebase is in progress:
    /// commit the commit it stopped at, its conflicts resolved and
    /// staged, with the message in the box, and replay the rest (see
    /// the core crate's `git::rebase` module). In an interactive rebase,
    /// commit what is staged with the message, if anything is, and go
    /// on once nothing is left unstaged (see the core crate's
    /// `git::interactive` module). The status bar says how that went:
    /// committed and waiting for the rest, finished, or stopped again.
    pub fn continue_rebase(&mut self) -> ChangesOutcome {
        let message = self.message_text();
        let Some(changes) = &mut self.changes else {
            return ChangesOutcome::Continue;
        };
        let Some(status) = changes.rebase().cloned() else {
            return ChangesOutcome::Notice(StatusLine::info("No rebase is in progress"));
        };
        let result = changes.continue_rebase(&message);
        self.acted = true;
        drop_content(&mut self.content, &mut self.stale);
        match result {
            Ok(Continued { committed, outcome }) => {
                self.message = message_editor("");
                self.auto_message = None;
                let committed = committed.map(|id| {
                    let summary = message.trim().lines().next().unwrap_or("");
                    format!("Committed {} {summary}", short_id(id))
                });
                if committed.is_some() && outcome.is_none() {
                    self.committed_at = status.stop.map(|(_, at)| at);
                }
                let text = match (committed, outcome) {
                    (Some(committed), None) => format!(
                        "{committed}; stage and commit, or discard, what is left unstaged to go on with the rebase"
                    ),
                    (Some(committed), Some(outcome)) => {
                        format!("{committed}; {}", status.summary(&outcome))
                    }
                    (None, Some(outcome)) => status.summary(&outcome),
                    (None, None) => String::new(),
                };
                ChangesOutcome::Notice(StatusLine::info(text))
            }
            Err(err) => ChangesOutcome::Notice(StatusLine::error(format!(
                "Could not continue the rebase: {err}"
            ))),
        }
    }

    /// "Abort merge" and "Abort rebase": ask whether to give up the one
    /// in progress, which goes ahead once answered yes (see
    /// [`finish_abort`](Self::finish_abort)).
    pub fn abort(&mut self) -> ChangesOutcome {
        let Some(changes) = &self.changes else {
            return ChangesOutcome::Continue;
        };
        let (title, body) = if let Some(rebase) = changes.rebase() {
            let branch = rebase.branch.as_deref().unwrap_or("HEAD");
            let title = if rebase.interactive {
                format!("Abort rebasing {branch}?")
            } else {
                format!("Abort rebasing {branch} onto {}?", rebase.onto)
            };
            (
                title,
                format!(
                    "The commits replayed so far and any conflicts resolved are thrown away: {branch} and its files go back to how they were before the rebase. This can't be undone."
                ),
            )
        } else if changes.is_merging() {
            let branch = changes.head_branch().unwrap_or("HEAD");
            (
                "Abort the merge?".to_owned(),
                format!(
                    "The merge's changes and any conflicts resolved are thrown away: {branch} and its files go back to how they were before the merge. Untracked files are left alone. This can't be undone."
                ),
            )
        } else {
            return ChangesOutcome::Notice(StatusLine::info("No merge or rebase is in progress"));
        };
        self.abort = Some(ConfirmBox::new(title, body, ABORT));
        ChangesOutcome::Continue
    }

    /// The abort box was answered yes: give up the merge or rebase.
    fn finish_abort(&mut self) -> ChangesOutcome {
        self.abort = None;
        let Some(changes) = &mut self.changes else {
            return ChangesOutcome::Continue;
        };
        let result = changes.abort();
        self.acted = true;
        drop_content(&mut self.content, &mut self.stale);
        self.withdraw_message();
        match result {
            Ok(InProgress::Rebase) => ChangesOutcome::Notice(StatusLine::info("Rebase aborted")),
            Ok(_) => ChangesOutcome::Notice(StatusLine::info("Merge aborted")),
            Err(err) => {
                ChangesOutcome::Notice(StatusLine::error(format!("Could not abort: {err}")))
            }
        }
    }

    /// Whether the abort box is asking.
    #[cfg(test)]
    fn abort_box(&self) -> Option<&ConfirmBox> {
        self.abort.as_ref()
    }

    /// Open the selected file in the editor, at its first conflict if it
    /// has one. A submodule isn't a file to open: its changes are on its
    /// own tab, which the page asks to have shown (see
    /// [`ChangesTabs::follow_submodule`]).
    pub fn open_selected(&mut self) -> ChangesOutcome {
        self.open(false)
    }

    /// Open the file of the diff shown in the editor, at the line the
    /// diff's cursor is on: `o` in the diff.
    pub fn open_at_cursor(&mut self) -> ChangesOutcome {
        self.open(true)
    }

    /// Whether the diff of a file to open is shown, for its cursor to say
    /// where to open it.
    pub fn can_open_at_cursor(&self) -> bool {
        self.shows_diff_lines()
            && self
                .selected_change()
                .is_some_and(|(_, change)| !change.submodule && change.kind != ChangeKind::Deleted)
    }

    fn open(&mut self, at_cursor: bool) -> ChangesOutcome {
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
        let line = match &self.content {
            Some(Content::Diff(model)) if at_cursor => model.new_line_at_cursor(),
            _ => None,
        };
        ChangesOutcome::OpenFile {
            path: changes.workdir().join(&change.path),
            conflicted: change.kind == ChangeKind::Conflicted && line.is_none(),
            line,
        }
    }

    // ----- Input ----------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent, clipboard: &mut Clipboard) -> ChangesOutcome {
        // The hook box, while it shows, has every key.
        if let Some(dialog) = &mut self.hook_box {
            let outcome = dialog.handle_key(key);
            return self.finish_hook_box(outcome);
        }
        // Escape stops the hook a commit is waiting on.
        if key.code == KeyCode::Esc
            && let Some(committing) = &self.committing
        {
            committing.hooks.cancel();
            return ChangesOutcome::Continue;
        }
        // The discard or resolve box, while it asks, has every key.
        if let Some(pending) = &mut self.pending {
            return match pending.dialog.handle_key(key) {
                ConfirmOutcome::Continue => ChangesOutcome::Continue,
                ConfirmOutcome::Cancel => {
                    self.pending = None;
                    ChangesOutcome::Continue
                }
                ConfirmOutcome::Confirm => self.finish_pending(),
            };
        }
        // So has the abort box.
        if let Some(dialog) = &mut self.abort {
            return match dialog.handle_key(key) {
                ConfirmOutcome::Continue => ChangesOutcome::Continue,
                ConfirmOutcome::Cancel => {
                    self.abort = None;
                    ChangesOutcome::Continue
                }
                ConfirmOutcome::Confirm => self.finish_abort(),
            };
        }
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
            // From the diff, back to the list it shows a file of.
            KeyCode::BackTab | KeyCode::Tab if self.pane == Pane::Content => {
                self.pane = self.list.pane();
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
            Pane::Content => self.handle_content_key(key, clipboard),
        }
    }

    fn handle_list_key(&mut self, list: List, key: KeyEvent) -> ChangesOutcome {
        let area = match list {
            List::Unstaged => self.unstaged_area,
            List::Staged => self.staged_area,
        };
        let page = (area.height as usize).saturating_sub(2).max(1);
        let selected = self.file_list(list).selected;
        let row = self.file_list(list).selected_row();
        // Moving in a list makes it the one the diff follows, even at
        // its ends.
        self.list = list;
        match key.code {
            // The ends of a list are as far as the arrows go: Tab moves
            // on to the other panes.
            KeyCode::Up => self.select(list, selected.saturating_sub(1)),
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

    fn handle_content_key(&mut self, key: KeyEvent, clipboard: &mut Clipboard) -> ChangesOutcome {
        if key.modifiers.is_empty() {
            match key.code {
                KeyCode::Char('o') => return self.open_at_cursor(),
                // As Space on a file in its list.
                KeyCode::Char(' ') => {
                    return match self.shown_list() {
                        Some(List::Unstaged) => self.stage_lines(),
                        Some(List::Staged) => self.unstage_lines(),
                        None => ChangesOutcome::Continue,
                    };
                }
                _ => {}
            }
        }
        let content = shown_mut(self.content.as_mut());
        match self.content_pane.handle_key(key, content, clipboard) {
            Some(notice) => ChangesOutcome::Notice(notice),
            None => ChangesOutcome::Continue,
        }
    }

    // ----- The diff's selection ---------------------------------------------

    /// Whether the diff pane shows a diff with lines, for the cursor to
    /// move through and select in.
    pub fn shows_diff_lines(&self) -> bool {
        matches!(&self.content, Some(Content::Diff(model)) if !model.rows().is_empty())
    }

    /// What is selected on the page, for a search to start from: the
    /// text selected in the commit message while that has the keyboard,
    /// and otherwise in the diff shown.
    pub fn selected_text(&self) -> Option<String> {
        if self.pane == Pane::Commit {
            return self.message.editor().selected_text();
        }
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
        diff_pane::copy(shown(self.content.as_ref()), clipboard, how)
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

    // ----- Staging, unstaging, and reverting lines --------------------------

    /// The list whose file the diff shows.
    fn shown_list(&self) -> Option<List> {
        self.shown.as_ref().map(|(list, _)| *list)
    }

    /// Whether the diff shown is of a file of `list` with changed lines
    /// selected (or under the cursor, with nothing selected), to stage,
    /// unstage, or revert.
    fn has_lines_in(&self, list: List) -> bool {
        self.shown_list() == Some(list)
            && matches!(&self.content, Some(Content::Diff(model))
                if model.diff().has_changes_among(&model.selected_lines()))
    }

    /// Whether lines of an unstaged file's diff are selected to stage.
    pub fn can_stage_lines(&self) -> bool {
        self.has_lines_in(List::Unstaged)
    }

    /// Whether lines of a staged file's diff are selected to unstage.
    pub fn can_unstage_lines(&self) -> bool {
        self.has_lines_in(List::Staged)
    }

    /// Whether lines of an unstaged file's diff are selected to revert.
    pub fn can_revert_lines(&self) -> bool {
        self.has_lines_in(List::Unstaged)
    }

    /// What staging, unstaging, or reverting the selected lines of the
    /// diff of a file of `list` makes of the file, as `make` works it out.
    fn lines_change(
        &self,
        list: List,
        make: fn(&FileDiff, &[DiffLine]) -> Option<LinesChange>,
    ) -> Option<LinesChange> {
        if self.shown_list() != Some(list) {
            return None;
        }
        match &self.content {
            Some(Content::Diff(model)) => make(model.diff(), &model.selected_lines()),
            _ => None,
        }
    }

    /// The command palette's and the menu's "Stage lines", and Space in
    /// the diff of an unstaged file: stage the changes on the selected
    /// lines, or the line under the cursor with nothing selected.
    pub fn stage_lines(&mut self) -> ChangesOutcome {
        match self.lines_change(List::Unstaged, FileDiff::stage_lines) {
            Some(change) => self.apply_lines(&change),
            None => nothing_to(self.shown_list() == Some(List::Unstaged), "stage"),
        }
    }

    /// The command palette's and the menu's "Unstage lines", and Space in
    /// the diff of a staged file: unstage the changes on the selected
    /// lines, or the line under the cursor with nothing selected.
    pub fn unstage_lines(&mut self) -> ChangesOutcome {
        match self.lines_change(List::Staged, FileDiff::unstage_lines) {
            Some(change) => self.apply_lines(&change),
            None => nothing_to(self.shown_list() == Some(List::Staged), "unstage"),
        }
    }

    /// The command palette's and the menu's "Revert lines": ask, in the
    /// revert box, whether to put the selected lines of an unstaged
    /// file's diff (or the line under the cursor) back as the index has
    /// them, throwing away the working directory's changes to them. The
    /// box's yes reverts them (see [`finish_pending`](Self::finish_pending)).
    pub fn revert_lines(&mut self) -> ChangesOutcome {
        let Some(change) = self.lines_change(List::Unstaged, FileDiff::revert_lines) else {
            return nothing_to(self.shown_list() == Some(List::Unstaged), "revert");
        };
        let what = lines_of(&change);
        let dialog = ConfirmBox::new(
            format!("Revert {what}?"),
            "The working directory's changes to them are lost: they go back to what is staged, or else what was last committed. This can't be undone.",
            REVERT,
        );
        self.pending = Some(PendingFiles {
            dialog,
            action: FilesAction::RevertLines,
            paths: vec![change.path.clone()],
            what,
            lines: Some(change),
        });
        ChangesOutcome::Continue
    }

    /// Write lines staged, unstaged, or reverted, and show the outcome:
    /// the lists and the diff built again, the diff's place kept but its
    /// selection gone with the lines it was on.
    fn apply_lines(&mut self, change: &LinesChange) -> ChangesOutcome {
        let Some(changes) = &mut self.changes else {
            return ChangesOutcome::Continue;
        };
        let result = changes.apply_lines(change);
        if let Some(Content::Diff(model)) = &mut self.content {
            model.clear_selection();
        }
        self.acted = true;
        drop_content(&mut self.content, &mut self.stale);
        self.follow_lists();
        let what = lines_of(change);
        let (done, verb) = match (change.target, self.shown_list()) {
            (LinesTarget::WorkingTree, _) => ("Reverted", "revert"),
            (LinesTarget::Index, Some(List::Staged)) => ("Unstaged", "unstage"),
            (LinesTarget::Index, _) => ("Staged", "stage"),
        };
        ChangesOutcome::Notice(match result {
            Ok(()) => StatusLine::info(format!("{done} {what}")),
            Err(err) => StatusLine::error(format!("Could not {verb} {what}: {}", err.message())),
        })
    }

    /// Add pasted text to the commit message, if that is where the
    /// keyboard is.
    pub fn paste(&mut self, text: &str) {
        if self.pane == Pane::Commit && self.pending.is_none() && self.abort.is_none() {
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
        self.content_pane.is_dragging() || self.divider_drag.is_some() || self.message.is_dragging()
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

    /// The row of a list at screen row `y`, if there is one there: not
    /// the heading, nor below the last row.
    fn list_row_at(&self, list: List, y: u16) -> Option<usize> {
        let area = match list {
            List::Unstaged => self.unstaged_area,
            List::Staged => self.staged_area,
        };
        if y <= area.y || y >= area.bottom() {
            return None;
        }
        let file_list = self.file_list(list);
        let row = file_list.scroll + (y - area.y - 1) as usize;
        (row < file_list.rows.len()).then_some(row)
    }

    /// Handle a mouse event. `wheel` is how many rows (or columns,
    /// sideways) a wheel event scrolls.
    pub fn handle_mouse(&mut self, mouse: MouseEvent, wheel: usize) -> ChangesMouseOutcome {
        let mut outcome = ChangesMouseOutcome::default();
        // The hook box, while it shows, has the mouse; a press outside
        // it dismisses it.
        if let Some(dialog) = &mut self.hook_box {
            let answer = dialog.handle_mouse(mouse, wheel);
            if let ChangesOutcome::Notice(notice) = self.finish_hook_box(answer) {
                outcome.notice = Some(notice);
            }
            return outcome;
        }
        // The discard or resolve box, while it asks, has the mouse; a
        // press outside it is no.
        if let Some(pending) = &mut self.pending {
            match pending.dialog.handle_mouse(mouse) {
                ConfirmOutcome::Continue => {}
                ConfirmOutcome::Cancel => self.pending = None,
                ConfirmOutcome::Confirm => {
                    if let ChangesOutcome::Notice(notice) = self.finish_pending() {
                        outcome.notice = Some(notice);
                    }
                }
            }
            return outcome;
        }
        if let Some(dialog) = &mut self.abort {
            match dialog.handle_mouse(mouse) {
                ConfirmOutcome::Continue => {}
                ConfirmOutcome::Cancel => self.abort = None,
                ConfirmOutcome::Confirm => {
                    if let ChangesOutcome::Notice(notice) = self.finish_abort() {
                        outcome.notice = Some(notice);
                    }
                }
            }
            return outcome;
        }
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
                return outcome;
            }
        }
        // A scrollbar drag, or a drag in the message, owns the mouse
        // likewise until the button comes up.
        if self.is_dragging()
            && matches!(mouse.kind, MouseEventKind::Drag(_) | MouseEventKind::Up(_))
        {
            if self.content_pane.is_dragging() {
                self.content_pane
                    .handle_mouse(mouse, wheel, shown_mut(self.content.as_mut()));
            } else {
                self.message.handle_mouse(mouse, wheel);
            }
            return outcome;
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
            return outcome;
        };
        // The diff pane scrolls itself, and moves the diff's cursor and
        // selects in it; a right press in a diff with lines asks for the
        // menu of what can be done with the selection.
        if pane == Pane::Content {
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.pane = Pane::Content;
            }
            self.content_pane
                .handle_mouse(mouse, wheel, shown_mut(self.content.as_mut()));
            outcome.diff_menu =
                mouse.kind == MouseEventKind::Down(MouseButton::Right) && self.shows_diff_lines();
            return outcome;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if pane == Pane::Commit => {
                self.message.handle_mouse(mouse, wheel);
            }
            MouseEventKind::ScrollUp => self.scroll_pane(pane, -(wheel as isize)),
            MouseEventKind::ScrollDown => self.scroll_pane(pane, wheel as isize),
            MouseEventKind::Down(MouseButton::Left) if self.amend_area.contains(at) => {
                if let ChangesOutcome::Notice(notice) = self.toggle_amend() {
                    outcome.notice = Some(notice);
                }
            }
            // A right press on a row of a list selects it, without
            // folding a directory, and asks for the menu of what can be
            // done to it.
            MouseEventKind::Down(MouseButton::Right)
                if matches!(pane, Pane::Unstaged | Pane::Staged) =>
            {
                let list = if pane == Pane::Unstaged {
                    List::Unstaged
                } else {
                    List::Staged
                };
                if let Some(row) = self.list_row_at(list, mouse.row) {
                    self.enter_list(list, row);
                    outcome.menu = true;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.pane = pane;
                match pane {
                    Pane::Unstaged | Pane::Staged => {
                        let list = if pane == Pane::Unstaged {
                            List::Unstaged
                        } else {
                            List::Staged
                        };
                        self.list = list;
                        // The heading row selects nothing; a directory
                        // folds or unfolds when clicked.
                        if let Some(row) = self.list_row_at(list, mouse.row) {
                            self.select(list, row);
                            if let Some(TreeRow::Dir { dir, collapsed, .. }) =
                                self.file_list(list).selected_row()
                            {
                                self.file_list_mut(list).set_dir_collapsed(dir, !collapsed);
                            }
                        }
                    }
                    Pane::Commit => {
                        if mouse.row > self.commit_area.y {
                            self.message.handle_mouse(mouse, wheel);
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
            Pane::Unstaged => self.unstaged.scroll = step(self.unstaged.scroll),
            Pane::Staged => self.staged.scroll = step(self.staged.scroll),
            Pane::Commit => {}
            Pane::Content => self.content_pane.scroll_by(rows),
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
        let message_cursor = self.render_commit_box(buf, theme);
        let content_cursor = self.render_content(buf, theme);
        let cursor = message_cursor.or(content_cursor);
        // The discard or resolve box over it all, while it asks; the
        // message's cursor doesn't show through it.
        if let Some(pending) = &mut self.pending {
            pending.dialog.render(area, buf, theme);
            return None;
        }
        if let Some(dialog) = &mut self.abort {
            dialog.render(area, buf, theme);
            return None;
        }
        if let Some(dialog) = &mut self.hook_box {
            dialog.render(area, buf, theme);
            return None;
        }
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
        let rebasing = changes.rebase().map(|rebase| {
            let branch = rebase.branch.as_deref().unwrap_or("HEAD");
            match rebase.stop {
                // What the step stopped at does, to which commit.
                Some((action, commit)) => {
                    let action = match action {
                        RebaseAction::Pick => "Pick",
                        RebaseAction::Edit => "Edit",
                        RebaseAction::Reword => "Reword",
                        RebaseAction::Drop => "Drop",
                        RebaseAction::Squash => "Squash",
                        RebaseAction::Fixup => "Fixup",
                    };
                    format!(
                        "{action} {} on {branch}: {} of {}",
                        short_id(commit),
                        rebase.step,
                        rebase.total
                    )
                }
                None if rebase.interactive => {
                    format!("Rebase {branch}: {} of {}", rebase.step, rebase.total)
                }
                None => format!(
                    "Rebase {branch} onto {}: {} of {}",
                    rebase.onto, rebase.step, rebase.total
                ),
            }
        });
        let conflicts = changes.conflict_count();
        if conflicts > 0 {
            let noun = if conflicts == 1 {
                "conflict"
            } else {
                "conflicts"
            };
            let doing = match &rebasing {
                Some(rebasing) => format!(" · {rebasing}"),
                None => String::new(),
            };
            return (
                format!("Resolve {conflicts} {noun}{doing}"),
                background
                    .fg(theme.diff_removed_text)
                    .add_modifier(Modifier::BOLD),
            );
        }
        if let Some(rebasing) = rebasing {
            return (rebasing, heading);
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

    /// Draw the diff pane. Returns where the terminal cursor goes while
    /// the pane has the keyboard: on a diff's cursor.
    fn render_content(&mut self, buf: &mut Buffer, theme: &Theme) -> Option<ScreenPosition> {
        let focused = self.pane == Pane::Content;
        self.content_pane.render(
            self.content_area,
            buf,
            theme,
            shown(self.content.as_ref()),
            focused,
        )
    }
}

/// An editor for the commit message, holding `text`: the file editor
/// without its gutter, since line numbers mean nothing in a message.
fn message_editor(text: &str) -> EditorView {
    let mut view = EditorView::new(Editor::new(FileBuffer::from_text(text)));
    view.set_gutter(false);
    view
}

/// `count` and the noun for it, singular or plural.
fn count_of(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// How the resolve box and the status bar name a side, as git does.
fn side_name(side: ConflictSide) -> &'static str {
    match side {
        ConflictSide::Ours => "ours",
        ConflictSide::Theirs => "theirs",
    }
}

/// What the resolve box says resolving `files` by taking `side` will
/// do: whose version that is in the merge or rebase in progress (in a
/// rebase, ours is what it is rebasing onto, which is easy to have
/// backwards), that the files are staged, and that what they have now
/// is lost for good.
fn resolve_body(changes: &Changes, side: ConflictSide, files: &[FileChange]) -> String {
    let whose = match (side, changes.rebase()) {
        (ConflictSide::Ours, Some(rebase)) => {
            format!("{} with the commits replayed so far", rebase.onto)
        }
        (ConflictSide::Theirs, Some(rebase)) => match rebase
            .message
            .as_deref()
            .and_then(|message| message.lines().next())
        {
            Some(summary) => format!("the commit being replayed (\"{summary}\")"),
            None => "the commit being replayed".to_owned(),
        },
        (ConflictSide::Ours, None) => changes.head_branch().unwrap_or("HEAD").to_owned(),
        (ConflictSide::Theirs, None) => "the branch being merged in".to_owned(),
    };
    let side = side_name(side);
    let mut body = if files.len() == 1 {
        format!(
            "The file is written as {whose} has it ({side}), or deleted if {side} deleted it, and staged, marking it resolved. What it has now, conflict markers and any resolving done by hand, is lost."
        )
    } else {
        format!(
            "The {} files in conflict are written as {whose} has them ({side}), or deleted where {side} deleted them, and staged, marking them resolved. What they have now, conflict markers and any resolving done by hand, is lost.",
            files.len()
        )
    };
    if files.iter().any(|file| file.submodule) {
        body.push_str(" A submodule is checked out at that side's commit.");
    }
    body.push_str(" This can't be undone.");
    body
}

/// What the discard box says discarding `files` will do: which go back
/// to what version, and which are deleted; that `kept` (conflicts and
/// submodules under a directory) are left alone; and that there is no
/// undoing it. `staged` are the paths with staged changes, which a file
/// goes back to rather than to what was committed.
fn discard_body(files: &[FileChange], kept: &[FileChange], staged: &HashSet<&str>) -> String {
    let (untracked, restored): (Vec<&FileChange>, Vec<&FileChange>) = files
        .iter()
        .partition(|change| change.kind == ChangeKind::Untracked);
    let mut body = match (files, restored.first()) {
        ([_], Some(change)) => {
            let version = if staged.contains(change.path.as_str()) {
                "what is staged"
            } else {
                "what was last committed"
            };
            if change.kind == ChangeKind::Deleted {
                format!("It is deleted: it comes back as {version}.")
            } else {
                format!("Its unstaged edits are lost: it goes back to {version}.")
            }
        }
        ([_], None) => "It isn't tracked by git, so it is deleted.".to_owned(),
        _ => {
            let mut parts = Vec::new();
            if !restored.is_empty() {
                parts.push(format!(
                    "{} back to what is staged, or else what was last committed",
                    if restored.len() == 1 {
                        "1 file goes".to_owned()
                    } else {
                        format!("{} files go", restored.len())
                    }
                ));
            }
            if !untracked.is_empty() {
                parts.push(format!(
                    "{} deleted",
                    if untracked.len() == 1 {
                        "1 untracked file is".to_owned()
                    } else {
                        format!("{} untracked files are", untracked.len())
                    }
                ));
            }
            format!("{}.", parts.join("; "))
        }
    };
    let conflicts = kept
        .iter()
        .filter(|change| change.kind == ChangeKind::Conflicted)
        .count();
    let submodules = kept.len() - conflicts;
    let mut left = Vec::new();
    if conflicts > 0 {
        left.push(count_of(conflicts, "file in conflict", "files in conflict"));
    }
    if submodules > 0 {
        left.push(count_of(submodules, "submodule", "submodules"));
    }
    if !left.is_empty() {
        body.push_str(&format!(
            " {} {} left alone.",
            left.join(" and "),
            if kept.len() == 1 { "is" } else { "are" }
        ));
    }
    body.push_str(" This can't be undone.");
    body
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
    use ninjaedit_core::TextPos;
    use ninjaedit_core::git::{DiffRow, NODE};
    use std::fs;
    use std::time::Duration;

    /// Rows a wheel event scrolls in these tests.
    const WHEEL: usize = 3;

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
        view.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            },
            WHEEL,
        );
        view.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            },
            WHEEL,
        );
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
        // Down at the end of the unstaged list stays there; Tab goes
        // on to the staged one, whose diff, against HEAD, shows once
        // its selection moves.
        press(&mut view, KeyCode::Down);
        assert_eq!(view.pane, Pane::Unstaged);
        assert_eq!(view.unstaged.selected, 1);
        press(&mut view, KeyCode::Tab);
        assert_eq!(view.pane, Pane::Staged);
        press(&mut view, KeyCode::Home);
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
        assert_eq!(view.pane, Pane::Staged, "↑ stays in the emptied list");
        press(&mut view, KeyCode::BackTab);
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

        // Down in the empty unstaged list stays there; Tab goes to the
        // staged one, and on past the diff to the commit box, where a
        // two-paragraph message and Ctrl+S commit.
        press(&mut view, KeyCode::Down);
        assert_eq!(view.pane, Pane::Unstaged);
        press(&mut view, KeyCode::Tab);
        assert_eq!(view.pane, Pane::Staged);
        press(&mut view, KeyCode::End);
        press(&mut view, KeyCode::Down);
        assert_eq!(view.pane, Pane::Staged);
        press(&mut view, KeyCode::Tab);
        press(&mut view, KeyCode::Tab);
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

        // Staged, the same range shows against HEAD, in the staged
        // list.
        press(&mut view, KeyCode::Char(' '));
        settle(&mut view);
        press(&mut view, KeyCode::Tab);
        press(&mut view, KeyCode::Home);
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
        // → on a file stays in the list: Enter or Tab goes to the diff.
        press(&mut view, KeyCode::Right);
        assert_eq!(view.pane, Pane::Staged);
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
            matches!(&outcome, ChangesOutcome::OpenFile { path, conflicted: true, .. }
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
    fn conflicts_are_resolved_by_taking_a_side_after_asking() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        configure_user(&repo);
        let base = commit_files(
            &repo,
            &[
                ("f.txt", "f\n"),
                ("src/a.txt", "a\n"),
                ("src/b.txt", "b\n"),
                ("src/c.txt", "c\n"),
            ],
            "Base",
        );
        let main = repo.head().unwrap().shorthand().unwrap().to_owned();
        let ours = [
            ("f.txt", "f ours\n"),
            ("src/a.txt", "a ours\n"),
            ("src/b.txt", "b ours\n"),
        ];
        commit_files(&repo, &ours, "Ours");
        repo.branch("side", &repo.find_commit(base).unwrap(), false)
            .unwrap();
        repo.set_head("refs/heads/side").unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        let theirs = [
            ("f.txt", "f theirs\n"),
            ("src/a.txt", "a theirs\n"),
            ("src/b.txt", "b theirs\n"),
        ];
        let theirs = commit_files(&repo, &theirs, "Theirs");
        repo.set_head(&format!("refs/heads/{main}")).unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        let annotated = repo.find_annotated_commit(theirs).unwrap();
        repo.merge(&[&annotated], None, None).unwrap();
        fs::write(dir.path().join("src/c.txt"), "c edited\n").unwrap();

        let mut view = view(&dir);
        let row_of = |view: &ChangesView, key: RowKey| {
            let list = &view.unstaged;
            list.rows
                .iter()
                .position(|row| list.key_of(*row) == key)
                .unwrap()
        };

        // A conflicted file can be resolved, but not discarded. The box
        // asks first, naming whose version ours is; Escape cancels.
        let f = row_of(&view, RowKey::File("f.txt".into()));
        view.select(List::Unstaged, f);
        assert!(view.can_resolve_selected());
        assert!(!view.can_discard_selected());
        let conflicted = read(&dir, "f.txt");
        view.resolve_selected(ConflictSide::Ours);
        let dialog = view.files_box().unwrap();
        assert_eq!(dialog.title(), "Resolve f.txt using ours?");
        assert_eq!(
            dialog.body(),
            format!(
                "The file is written as {main} has it (ours), or deleted if ours deleted it, and staged, marking it resolved. What it has now, conflict markers and any resolving done by hand, is lost. This can't be undone."
            )
        );
        assert!(view.hint().text().contains("y resolve"), "{}", view.hint());
        press(&mut view, KeyCode::Esc);
        assert!(view.files_box().is_none());
        assert_eq!(read(&dir, "f.txt"), conflicted);

        // `y` goes ahead: f.txt is as main has it, so it leaves the
        // lists altogether.
        view.resolve_selected(ConflictSide::Ours);
        let outcome = press(&mut view, KeyCode::Char('y'));
        assert_eq!(
            outcome,
            ChangesOutcome::Notice(StatusLine::info("Resolved f.txt using ours"))
        );
        assert!(view.take_acted());
        assert_eq!(read(&dir, "f.txt"), "f ours\n");
        settle(&mut view);
        assert_eq!(
            unstaged_paths(&view),
            ["src/a.txt", "src/b.txt", "src/c.txt"]
        );

        // A directory resolves every conflict under it, and leaves its
        // other changes alone.
        let src = row_of(&view, RowKey::Dir("src".into()));
        view.select(List::Unstaged, src);
        assert!(view.can_resolve_selected());
        view.resolve_selected(ConflictSide::Theirs);
        let dialog = view.files_box().unwrap();
        assert_eq!(dialog.title(), "Resolve conflicts in src/ using theirs?");
        assert!(
            dialog
                .body()
                .starts_with("The 2 files in conflict are written as the branch being merged in has them (theirs)"),
            "{}",
            dialog.body()
        );
        let outcome = press(&mut view, KeyCode::Char('y'));
        assert_eq!(
            outcome,
            ChangesOutcome::Notice(StatusLine::info(
                "Resolved 2 conflicts in src/ using theirs"
            ))
        );
        settle(&mut view);
        assert_eq!(read(&dir, "src/a.txt"), "a theirs\n");
        assert_eq!(read(&dir, "src/b.txt"), "b theirs\n");
        assert_eq!(read(&dir, "src/c.txt"), "c edited\n");
        assert_eq!(unstaged_paths(&view), ["src/c.txt"]);
        let staged: Vec<&str> = view
            .files(List::Staged)
            .iter()
            .map(|change| change.path.as_str())
            .collect();
        assert_eq!(staged, ["src/a.txt", "src/b.txt"]);
        // Nothing is left in conflict to resolve.
        assert!(!view.can_resolve_selected());
        let outcome = view.resolve_selected(ConflictSide::Ours);
        assert!(
            matches!(&outcome, ChangesOutcome::Notice(m) if m.text().contains("no conflicts")),
            "{outcome:?}"
        );
        let screen = draw(&mut view, 100, 30);
        let right = right_column(&screen, view.files_rule.x as usize);
        assert!(
            right.iter().any(|r| r.starts_with(" Merge into ")),
            "{right:#?}"
        );
    }

    /// A repository whose main branch has two commits on a base, the
    /// first changing `f.txt` as a `side` branch from the base does too,
    /// rebased onto side as far as that conflict. Returns the
    /// repository's directory, main's branch, and side's commit.
    fn rebase_stopped_at_a_conflict() -> (tempfile::TempDir, String, git2::Oid) {
        use ninjaedit_core::git::{Integration, Target};
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        configure_user(&repo);
        let base = commit_files(&repo, &[("f.txt", "one\ntwo\nthree\n")], "Base");
        let main = repo.head().unwrap().shorthand().unwrap().to_owned();
        commit_files(&repo, &[("f.txt", "one\nours\nthree\n")], "Ours");
        commit_files(&repo, &[("g.txt", "more\n")], "More");
        repo.branch("side", &repo.find_commit(base).unwrap(), false)
            .unwrap();
        repo.set_head("refs/heads/side").unwrap();
        repo.checkout_head(Some(
            git2::build::CheckoutBuilder::new()
                .force()
                .remove_untracked(true),
        ))
        .unwrap();
        let side = commit_files(&repo, &[("f.txt", "one\ntheirs\nthree\n")], "Theirs");
        repo.set_head(&format!("refs/heads/{main}")).unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        let rebase = Integration::Rebase(Target::Branch {
            name: "side".to_owned(),
            remote: false,
        });
        rebase
            .run_with(&repo, &Hooks::new(80, 24), &mut |_, _| {})
            .unwrap();
        assert_eq!(repo.state(), git2::RepositoryState::RebaseMerge);
        (dir, main, side)
    }

    #[test]
    fn a_rebase_stopped_at_a_conflict_is_resolved_and_continued() {
        let (dir, main, side) = rebase_stopped_at_a_conflict();
        let repo = Repository::open(dir.path()).unwrap();
        let mut view = view(&dir);
        assert!(view.is_rebasing());
        assert!(!view.is_merging());
        let screen = draw(&mut view, 100, 30);
        let width = view.files_rule.x as usize;
        let right = right_column(&screen, width);
        let heading = format!(" Resolve 1 conflict · Rebase {main} onto side: 1 of 2");
        assert!(right.iter().any(|r| r.starts_with(&heading)), "{right:#?}");
        // The box starts with the message of the commit being replayed.
        assert_eq!(view.message_text(), "Ours");
        // Amending is refused, and so is going on with the conflict.
        let outcome = press(&mut view, KeyCode::Char('m'));
        assert!(
            matches!(&outcome, ChangesOutcome::Notice(m) if m.text().contains("rebase")),
            "{outcome:?}"
        );
        let outcome = ctrl(&mut view, 's');
        assert!(
            matches!(&outcome, ChangesOutcome::Notice(m)
                if m.text() == "Could not continue the rebase: 1 file is still in conflict; resolve and stage them first"),
            "{outcome:?}"
        );
        settle(&mut view);

        // Resolved and staged, the heading says what Ctrl+S goes on with.
        fs::write(dir.path().join("f.txt"), "one\nboth\nthree\n").unwrap();
        press(&mut view, KeyCode::Char(' '));
        settle(&mut view);
        let right = right_column(&draw(&mut view, 100, 30), width);
        let heading = format!(" Rebase {main} onto side: 1 of 2");
        assert!(right.iter().any(|r| r.starts_with(&heading)), "{right:#?}");
        let outcome = ctrl(&mut view, 's');
        assert!(
            matches!(&outcome, ChangesOutcome::Notice(m)
                if m.text() == format!("Rebased {main} onto side: 2 commits")),
            "{outcome:?}"
        );
        settle(&mut view);
        assert!(!view.is_rebasing());
        assert_eq!(view.message_text(), "");
        assert_eq!(repo.state(), git2::RepositoryState::Clean);
        let head = repo.head().unwrap();
        assert_eq!(head.shorthand().unwrap(), main);
        let head = head.peel_to_commit().unwrap();
        assert_eq!(head.message().unwrap(), "More");
        let resolved = head.parent(0).unwrap();
        assert_eq!(resolved.message().unwrap(), "Ours");
        assert_eq!(resolved.parent_ids().collect::<Vec<_>>(), [side]);
        let right = right_column(&draw(&mut view, 100, 30), width);
        assert!(
            right.iter().any(|r| r.starts_with(" Commit to ")),
            "{right:#?}"
        );
    }

    #[test]
    fn a_conflict_is_selected_on_arriving_and_left_alone_at_the_same_stop() {
        let (dir, main, _) = rebase_stopped_at_a_conflict();
        // An untracked file listed before the one in conflict.
        fs::write(dir.path().join("a_notes.txt"), "notes\n").unwrap();
        let mut view = view(&dir);
        assert_eq!(unstaged_paths(&view), ["a_notes.txt", "f.txt"]);
        assert_eq!(view.pane, Pane::Unstaged);
        assert_eq!(view.selected_change().unwrap().1.path, "f.txt");
        // Resolving it is the same stop: the keyboard stays put.
        fs::write(dir.path().join("f.txt"), "one\nboth\nthree\n").unwrap();
        press(&mut view, KeyCode::Char(' '));
        settle(&mut view);
        press(&mut view, KeyCode::Up);
        assert_eq!(view.selected_change().unwrap().1.path, "a_notes.txt");
        view.refresh();
        settle(&mut view);
        assert_eq!(view.selected_change().unwrap().1.path, "a_notes.txt");
        // Going on finishes the rebase; nothing more to place.
        let outcome = ctrl(&mut view, 's');
        assert!(
            matches!(&outcome, ChangesOutcome::Notice(m)
                if m.text() == format!("Rebased {main} onto side: 2 commits")),
            "{outcome:?}"
        );
        settle(&mut view);
        assert_eq!(view.placed_for, None);
    }

    /// A repository whose main branch has `Base`, then `Two` changing
    /// `a.txt` and adding `dir/b.txt`, then `Three` adding `c.txt`, with
    /// an interactive rebase stopped to edit Two. Returns the
    /// repository's directory, main's branch, and Two.
    fn editing_a_commit() -> (tempfile::TempDir, String, git2::Oid) {
        use ninjaedit_core::git::{Integration, RebasePlan};
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        configure_user(&repo);
        commit_files(&repo, &[("a.txt", "a\n")], "Base");
        let main = repo.head().unwrap().shorthand().unwrap().to_owned();
        let two = commit_files(&repo, &[("a.txt", "a2\n"), ("dir/b.txt", "b\n")], "Two");
        commit_files(&repo, &[("c.txt", "c\n")], "Three");
        let edit = Integration::Interactive(RebasePlan::edit(&repo, two).unwrap());
        edit.run_with(&repo, &Hooks::new(80, 24), &mut |_, _| {})
            .unwrap();
        assert_eq!(repo.state(), git2::RepositoryState::RebaseInteractive);
        (dir, main, two)
    }

    #[test]
    fn an_edited_commit_is_split_with_the_keyboard_where_it_is_next_needed() {
        let (dir, main, two) = editing_a_commit();
        let repo = Repository::open(dir.path()).unwrap();
        let mut view = view(&dir);
        assert!(view.is_rebasing());
        // Everything is staged: the message is next, Two's to start.
        assert_eq!(view.pane, Pane::Commit);
        assert_eq!(view.message_text(), "Two");
        let screen = draw(&mut view, 100, 30);
        let right = right_column(&screen, view.files_rule.x as usize);
        let heading = format!(" Edit {} on {main}: 1 of 2", short_id(two));
        assert!(right.iter().any(|r| r.starts_with(&heading)), "{right:#?}");

        // Split dir/b.txt off into a commit of its own, its directory
        // folded to show it is unfolded to select it.
        view.unstaged.folded.insert("dir".to_owned());
        view.changes
            .as_mut()
            .unwrap()
            .unstage(["dir/b.txt"])
            .unwrap();
        settle(&mut view);
        view.message = message_editor("Two: a");
        let outcome = ctrl(&mut view, 's');
        let ChangesOutcome::Notice(notice) = &outcome else {
            panic!("{outcome:?}");
        };
        assert!(
            notice.text().ends_with(
                " Two: a; stage and commit, or discard, what is left unstaged to go on with the rebase"
            ),
            "{outcome:?}"
        );
        settle(&mut view);
        assert!(view.is_rebasing());
        assert_eq!(view.pane, Pane::Unstaged);
        assert_eq!(view.selected_change().unwrap().1.path, "dir/b.txt");
        // Two's message was for the first commit only, even when the
        // page scans again.
        assert_eq!(view.message_text(), "");
        view.refresh();
        settle(&mut view);
        assert_eq!(view.message_text(), "");

        // Nothing staged: nothing to go on with yet.
        let outcome = ctrl(&mut view, 's');
        assert!(
            matches!(&outcome, ChangesOutcome::Notice(m)
                if m.text() == "Could not continue the rebase: untracked files are left unstaged; stage and commit them, or discard them, to go on"),
            "{outcome:?}"
        );
        press(&mut view, KeyCode::Char(' '));
        settle(&mut view);
        view.message = message_editor("Two: b");
        let outcome = ctrl(&mut view, 's');
        let ChangesOutcome::Notice(notice) = &outcome else {
            panic!("{outcome:?}");
        };
        assert!(
            notice
                .text()
                .ends_with(&format!(" Two: b; Rebased {main}: 2 commits")),
            "{outcome:?}"
        );
        settle(&mut view);
        assert!(!view.is_rebasing());
        assert_eq!(repo.state(), git2::RepositoryState::Clean);
        let head = repo.head().unwrap();
        assert_eq!(head.shorthand().unwrap(), main);
        let messages: Vec<String> = {
            let mut walk = repo.revwalk().unwrap();
            walk.push_head().unwrap();
            walk.map(|id| {
                let commit = repo.find_commit(id.unwrap()).unwrap();
                commit.message().unwrap().to_owned()
            })
            .collect()
        };
        assert_eq!(messages, ["Three", "Two: b", "Two: a", "Base"]);
    }

    #[test]
    fn a_stop_git_made_to_edit_opens_with_the_commits_changes_staged() {
        let (dir, main, two) = editing_a_commit();
        // As git's command line stops: the commit made, HEAD on it,
        // nothing staged, and `amend` naming it.
        let repo = Repository::open(dir.path()).unwrap();
        repo.set_head_detached(two).unwrap();
        fs::write(
            repo.path().join("rebase-merge").join("amend"),
            format!("{two}\n"),
        )
        .unwrap();
        let mut view = view(&dir);
        assert!(view.is_rebasing());
        assert_eq!(view.pane, Pane::Commit);
        assert_eq!(view.message_text(), "Two");
        let staged: Vec<&str> = view
            .changes
            .as_ref()
            .unwrap()
            .staged()
            .iter()
            .map(|change| change.path.as_str())
            .collect();
        assert_eq!(staged, ["a.txt", "dir/b.txt"]);
        view.message = message_editor("Two, reworded");
        let outcome = ctrl(&mut view, 's');
        let ChangesOutcome::Notice(notice) = &outcome else {
            panic!("{outcome:?}");
        };
        assert!(
            notice
                .text()
                .ends_with(&format!(" Two, reworded; Rebased {main}: 2 commits")),
            "{outcome:?}"
        );
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.parent(0).unwrap().message().unwrap(), "Two, reworded");
    }

    #[test]
    fn an_edited_commit_left_with_nothing_goes_on_without_a_commit() {
        let (dir, main, two) = editing_a_commit();
        let mut view = view(&dir);
        let mut changes = Changes::open(dir.path()).unwrap();
        changes.wait(Duration::from_secs(10));
        changes.unstage(["a.txt", "dir/b.txt"]).unwrap();
        changes.discard(["a.txt", "dir/b.txt"]).unwrap();
        view.refresh();
        settle(&mut view);
        let outcome = ctrl(&mut view, 's');
        assert!(
            matches!(&outcome, ChangesOutcome::Notice(m)
                if m.text() == format!("Rebased {main}: 1 commit")),
            "{outcome:?}"
        );
        let repo = Repository::open(dir.path()).unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.message().unwrap(), "Three");
        assert_ne!(head.parent_id(0).unwrap(), two);
        assert_eq!(head.parent(0).unwrap().message().unwrap(), "Base");
    }

    #[test]
    fn an_interactive_rebase_is_aborted_once_the_box_says_yes() {
        let (dir, main, _) = editing_a_commit();
        let mut view = view(&dir);
        view.abort();
        let title = format!("Abort rebasing {main}?");
        assert_eq!(view.abort_box().unwrap().title(), title);
        let outcome = press(&mut view, KeyCode::Char('y'));
        assert_eq!(
            outcome,
            ChangesOutcome::Notice(StatusLine::info("Rebase aborted"))
        );
        settle(&mut view);
        assert!(!view.is_rebasing());
        let repo = Repository::open(dir.path()).unwrap();
        assert_eq!(repo.state(), git2::RepositoryState::Clean);
        let head = repo.head().unwrap();
        assert_eq!(head.shorthand().unwrap(), main);
        assert_eq!(head.peel_to_commit().unwrap().message().unwrap(), "Three");
    }

    #[test]
    fn a_rebase_is_aborted_once_the_box_says_yes() {
        let (dir, main, _) = rebase_stopped_at_a_conflict();
        let repo = Repository::open(dir.path()).unwrap();
        let before = repo.find_branch(&main, git2::BranchType::Local).unwrap();
        let before = before.get().target().unwrap();
        let mut view = view(&dir);
        assert_eq!(view.abort(), ChangesOutcome::Continue);
        let title = format!("Abort rebasing {main} onto side?");
        assert_eq!(view.abort_box().unwrap().title(), title);
        assert_eq!(view.hint(), StatusLine::help(ABORT_HELP));
        // No keeps it going.
        press(&mut view, KeyCode::Char('n'));
        assert!(view.abort_box().is_none());
        assert!(view.is_rebasing());
        view.abort();
        let screen = draw(&mut view, 100, 30);
        assert!(screen.iter().any(|r| r.contains(&title)), "{screen:#?}");
        let outcome = press(&mut view, KeyCode::Char('y'));
        assert_eq!(
            outcome,
            ChangesOutcome::Notice(StatusLine::info("Rebase aborted"))
        );
        settle(&mut view);
        assert!(!view.is_rebasing());
        assert_eq!(repo.state(), git2::RepositoryState::Clean);
        assert_eq!(repo.head().unwrap().shorthand().unwrap(), main);
        assert_eq!(repo.head().unwrap().target(), Some(before));
        assert_eq!(
            fs::read_to_string(dir.path().join("f.txt")).unwrap(),
            "one\nours\nthree\n"
        );
        // Nothing is left to abort.
        assert_eq!(
            view.abort(),
            ChangesOutcome::Notice(StatusLine::info("No merge or rebase is in progress"))
        );
    }

    #[test]
    fn the_rules_drag_to_resize_and_the_panes_take_clicks() {
        let dir = repo_with_changes();
        let mut view = view(&dir);
        draw(&mut view, 120, 40);
        let drag = |view: &mut ChangesView, from: (u16, u16), to: (u16, u16)| {
            view.handle_mouse(
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: from.0,
                    row: from.1,
                    modifiers: KeyModifiers::NONE,
                },
                WHEEL,
            );
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
            let released = view.handle_mouse(
                MouseEvent {
                    kind: MouseEventKind::Up(MouseButton::Left),
                    column: to.0,
                    row: to.1,
                    modifiers: KeyModifiers::NONE,
                },
                WHEEL,
            );
            assert!(released.resized);
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
        // ← stays in the diff; Shift+Tab goes back to the list the
        // diff follows, which needn't be the pane before it.
        press(&mut view, KeyCode::Left);
        assert_eq!(view.pane, Pane::Content);
        press(&mut view, KeyCode::BackTab);
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

    #[test]
    fn the_diffs_cursor_stays_through_a_scan_and_opens_the_file_there() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        configure_user(&repo);
        let old: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        commit_files(&repo, &[("f.txt", &old)], "Base");
        fs::write(
            dir.path().join("f.txt"),
            old.replace("line 10\n", "line ten\n"),
        )
        .unwrap();
        let mut view = view(&dir);
        draw(&mut view, 100, 30);
        press(&mut view, KeyCode::Enter);
        assert_eq!(view.pane, Pane::Content);
        // Down past the gap and the context to the removed line.
        for _ in 0..4 {
            press(&mut view, KeyCode::Down);
        }
        let cursor = |view: &ChangesView| match &view.content {
            Some(Content::Diff(model)) => Some(model.cursor()),
            _ => None,
        };
        let removed = Some(TextPos::new(4, 0));
        assert_eq!(cursor(&view), removed);
        // A scan builds the diff again, with the cursor where it was.
        view.refresh();
        settle(&mut view);
        draw(&mut view, 100, 30);
        assert_eq!(cursor(&view), removed);
        // `o` opens the file at the line that replaced the removed one.
        assert!(matches!(
            press(&mut view, KeyCode::Char('o')),
            ChangesOutcome::OpenFile { line: Some(9), .. }
        ));
        // A right press in the diff asks for the selection's menu, and
        // the menu's commands work on the diff.
        let content = view.content_area;
        let outcome = right_click(&mut view, content.x + 10, content.y + 2);
        assert!(outcome.diff_menu && !outcome.menu);
        assert!(view.shows_diff_lines() && !view.has_diff_selection());
        view.select_all_in_diff();
        assert!(view.has_diff_selection());
        let mut clipboard = Clipboard::local_only();
        assert!(
            view.copy_diff_selection(&mut clipboard, CopyAs::Text)
                .is_some()
        );
        assert_eq!(
            clipboard.get().as_deref(),
            Some("line 7\nline 8\nline 9\nline 10\nline ten\nline 11\nline 12\nline 13\n")
        );
    }

    #[test]
    fn a_scan_keeps_the_diffs_place_when_lines_are_added_above_it() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        configure_user(&repo);
        let old: String = (1..=80).map(|n| format!("line {n}\n")).collect();
        commit_files(&repo, &[("f.txt", &old)], "Base");
        let edited = old.replace("line 60\n", "line sixty\n");
        fs::write(dir.path().join("f.txt"), &edited).unwrap();
        let mut view = view(&dir);
        draw(&mut view, 100, 30);
        press(&mut view, KeyCode::Enter);
        // Everything revealed, from the gap above the change (its "all"
        // button) and the gap below; then up to line 62.
        press(&mut view, KeyCode::Right);
        press(&mut view, KeyCode::Enter);
        let mut clipboard = Clipboard::local_only();
        view.handle_key(key(KeyCode::End, KeyModifiers::CONTROL), &mut clipboard);
        press(&mut view, KeyCode::Enter);
        press(&mut view, KeyCode::Up);
        press(&mut view, KeyCode::Up);
        let cursor_text = |view: &ChangesView| match &view.content {
            Some(Content::Diff(model)) => match model.rows()[model.cursor().line] {
                DiffRow::Line(line) => model.diff().text(&line).to_owned(),
                DiffRow::Gap { .. } => "…".to_owned(),
            },
            _ => String::new(),
        };
        let hidden = |view: &ChangesView| match &view.content {
            Some(Content::Diff(model)) => model
                .rows()
                .iter()
                .filter(|row| matches!(row, DiffRow::Gap { .. }))
                .count(),
            _ => usize::MAX,
        };
        let area = Rect::new(0, 0, 100, 30);
        let render = |view: &mut ChangesView| {
            let mut buf = Buffer::empty(area);
            view.render(area, &mut buf, &Theme::default())
        };
        let before = render(&mut view);
        assert_eq!(cursor_text(&view), "line 62");
        assert_eq!(hidden(&view), 0);
        assert!(before.is_some());
        // Three lines added at the top of the file, and a scan: the same
        // line, on the same row of the screen, with all still revealed.
        fs::write(dir.path().join("f.txt"), format!("a\nb\nc\n{edited}")).unwrap();
        view.refresh();
        settle(&mut view);
        let after = render(&mut view);
        assert_eq!(cursor_text(&view), "line 62");
        assert_eq!(hidden(&view), 0);
        assert_eq!(after, before);
    }

    #[test]
    fn lines_of_a_diff_are_staged_unstaged_and_reverted() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        configure_user(&repo);
        let base: String = (1..=10).map(|n| format!("{n}\n")).collect();
        commit_files(&repo, &[("a.txt", &base)], "Base");
        let edited = base.replace("3\n", "three\n").replace("8\n", "eight\n");
        fs::write(dir.path().join("a.txt"), &edited).unwrap();
        let mut view = view(&dir);
        draw(&mut view, 100, 30);
        press(&mut view, KeyCode::Enter);
        assert_eq!(view.pane, Pane::Content);
        let index = |dir: &tempfile::TempDir| {
            let repo = Repository::open(dir.path()).unwrap();
            let index = repo.index().unwrap();
            let entry = index.get_path(Path::new("a.txt"), 0).unwrap();
            String::from_utf8(repo.find_blob(entry.id).unwrap().content().to_vec()).unwrap()
        };
        // The rows: 1, 2, -3, +three, 4, 5, 6, 7, -8, +eight, 9, 10. On
        // a context line there is nothing to stage, and Space says so.
        assert!(!view.can_stage_lines());
        assert!(matches!(
            press(&mut view, KeyCode::Char(' ')),
            ChangesOutcome::Notice(notice) if notice.text().contains("Nothing to stage")
        ));
        // Down to -3, and Shift+Down, Shift+End over +three: Space
        // stages that change.
        press(&mut view, KeyCode::Down);
        press(&mut view, KeyCode::Down);
        for code in [KeyCode::Down, KeyCode::End] {
            view.handle_key(key(code, KeyModifiers::SHIFT), &mut Clipboard::local_only());
        }
        assert!(view.can_stage_lines() && view.can_revert_lines());
        assert!(!view.can_unstage_lines());
        let outcome = press(&mut view, KeyCode::Char(' '));
        assert!(
            matches!(&outcome, ChangesOutcome::Notice(notice) if notice.text() == "Staged 2 lines of a.txt"),
            "{outcome:?}"
        );
        settle(&mut view);
        assert_eq!(index(&dir), base.replace("3\n", "three\n"));
        assert!(
            !view.has_diff_selection(),
            "the selection goes with the lines"
        );
        // Revert the line under the cursor, +eight, after asking: it goes
        // from the working directory, while the removal of 8, a change of
        // its own, stays, as does three.
        draw(&mut view, 100, 30);
        let row = |view: &ChangesView, text: &str| match &view.content {
            Some(Content::Diff(model)) => model
                .rows()
                .iter()
                .position(
                    |row| matches!(row, DiffRow::Line(line) if model.diff().text(line) == text),
                )
                .unwrap(),
            _ => panic!("no diff"),
        };
        let eight = row(&view, "eight");
        if let Some(Content::Diff(model)) = &mut view.content {
            model.set_cursor_at(eight, 0);
        }
        assert_eq!(view.revert_lines(), ChangesOutcome::Continue);
        let dialog = view.files_box().expect("the revert box asks");
        assert!(
            dialog.title().contains("Revert 1 line of a.txt"),
            "{}",
            dialog.title()
        );
        assert!(matches!(
            press(&mut view, KeyCode::Char('y')),
            ChangesOutcome::Notice(notice) if notice.text() == "Reverted 1 line of a.txt"
        ));
        settle(&mut view);
        assert_eq!(
            read(&dir, "a.txt"),
            base.replace("3\n", "three\n").replace("8\n", "")
        );
        // In the staged list's diff, Space on -3 unstages it, and the
        // +three left staged alone.
        press(&mut view, KeyCode::BackTab);
        press(&mut view, KeyCode::Tab);
        assert_eq!(view.pane, Pane::Staged);
        press(&mut view, KeyCode::Home);
        press(&mut view, KeyCode::Enter);
        draw(&mut view, 100, 30);
        assert_eq!(view.pane, Pane::Content);
        let removed = row(&view, "3");
        if let Some(Content::Diff(model)) = &mut view.content {
            model.set_cursor_at(removed, 0);
        }
        assert!(view.can_unstage_lines() && !view.can_stage_lines());
        assert!(matches!(
            press(&mut view, KeyCode::Char(' ')),
            ChangesOutcome::Notice(notice) if notice.text() == "Unstaged 1 line of a.txt"
        ));
        settle(&mut view);
        assert_eq!(index(&dir), base.replace("3\n", "3\nthree\n"));
    }

    fn right_click(view: &mut ChangesView, column: u16, row: u16) -> ChangesMouseOutcome {
        let outcome = view.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Right),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            },
            WHEEL,
        );
        view.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Right),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            },
            WHEEL,
        );
        outcome
    }

    fn read(dir: &tempfile::TempDir, path: &str) -> String {
        fs::read_to_string(dir.path().join(path)).unwrap()
    }

    fn unstaged_paths(view: &ChangesView) -> Vec<&str> {
        view.files(List::Unstaged)
            .iter()
            .map(|change| change.path.as_str())
            .collect()
    }

    #[test]
    fn discarding_asks_first_then_puts_files_back_or_deletes_them() {
        let dir = repo_with_changes();
        let mut view = view(&dir);
        draw(&mut view, 100, 30);
        // a.rs is selected in the unstaged list: it can be staged and
        // discarded, and there is nothing staged to unstage.
        assert!(view.can_stage_selected());
        assert!(view.can_discard_selected());
        assert!(!view.can_unstage_selected());

        // Discarding asks first, saying what will happen; Enter alone
        // presses Cancel, and nothing is lost.
        let edited = read(&dir, "a.rs");
        assert_eq!(view.discard_selected(), ChangesOutcome::Continue);
        let dialog = view.files_box().unwrap();
        assert_eq!(dialog.title(), "Discard changes to a.rs?");
        assert_eq!(
            dialog.body(),
            "Its unstaged edits are lost: it goes back to what was last committed. This can't be undone."
        );
        let screen = draw(&mut view, 100, 30);
        row_with(&screen, "Discard changes to a.rs?");
        let buttons = row_with(&screen, " Cancel ");
        assert!(buttons.contains(" Discard "), "{screen:#?}");
        assert!(view.hint().text().contains("y discard"), "{}", view.hint());
        // The box has the keys while it asks: Space stages nothing.
        press(&mut view, KeyCode::Char(' '));
        assert!(view.files(List::Staged).is_empty());
        assert_eq!(press(&mut view, KeyCode::Enter), ChangesOutcome::Continue);
        assert!(view.files_box().is_none());
        assert_eq!(read(&dir, "a.rs"), edited);
        assert!(!view.take_acted());

        // `y` goes ahead: a.rs is as committed, and gone from the list
        // before the scan confirms it.
        view.discard_selected();
        let outcome = press(&mut view, KeyCode::Char('y'));
        assert_eq!(
            outcome,
            ChangesOutcome::Notice(StatusLine::info("Discarded changes to a.rs"))
        );
        assert!(view.take_acted());
        assert_eq!(read(&dir, "a.rs"), "fn main() {\n    one();\n}\n");
        assert_eq!(unstaged_paths(&view), ["b.txt", "c.txt"]);
        settle(&mut view);
        assert_eq!(unstaged_paths(&view), ["b.txt", "c.txt"]);

        // A deleted file comes back, here by a click on the box's
        // Discard button, whose notice goes to the status bar.
        assert_eq!(
            view.unstaged.selected_key(),
            Some(RowKey::File("b.txt".into()))
        );
        view.discard_selected();
        assert_eq!(
            view.files_box().unwrap().body(),
            "It is deleted: it comes back as what was last committed. This can't be undone."
        );
        let screen = draw(&mut view, 100, 30);
        let y = screen
            .iter()
            .position(|row| row.contains(" Cancel "))
            .unwrap();
        let x = screen[y].chars().collect::<Vec<_>>();
        let x = (0..x.len())
            .find(|&i| x[i..].iter().collect::<String>().starts_with(" Discard "))
            .unwrap();
        let outcome = view.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x as u16 + 1,
                row: y as u16,
                modifiers: KeyModifiers::NONE,
            },
            WHEEL,
        );
        assert_eq!(
            outcome.notice,
            Some(StatusLine::info("Discarded changes to b.txt"))
        );
        assert_eq!(read(&dir, "b.txt"), "b\n");
        settle(&mut view);

        // An untracked file is deleted. A press outside the box is no.
        assert_eq!(unstaged_paths(&view), ["c.txt"]);
        view.discard_selected();
        assert_eq!(
            view.files_box().unwrap().body(),
            "It isn't tracked by git, so it is deleted. This can't be undone."
        );
        draw(&mut view, 100, 30);
        let outcome = view.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 0,
                row: 29,
                modifiers: KeyModifiers::NONE,
            },
            WHEEL,
        );
        assert_eq!(outcome, ChangesMouseOutcome::default());
        assert!(view.files_box().is_none());
        assert!(dir.path().join("c.txt").exists());
        view.discard_selected();
        press(&mut view, KeyCode::Char('y'));
        assert!(!dir.path().join("c.txt").exists());
        settle(&mut view);
        assert!(view.files(List::Unstaged).is_empty());
        assert!(!view.can_discard_selected());
        assert!(!view.can_stage_selected());
        let screen = draw(&mut view, 100, 30);
        row_with(&screen, CLEAN);
    }

    #[test]
    fn discarding_a_directory_discards_every_file_under_it() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        configure_user(&repo);
        commit_files(
            &repo,
            &[
                ("src/a.rs", "a\n"),
                ("src/b.rs", "b\n"),
                ("top.txt", "top\n"),
            ],
            "Base",
        );
        // src/a.rs: staged, then edited again; src/b.rs: edited; and a
        // new file in a directory of its own under src.
        fs::write(dir.path().join("src/a.rs"), "a staged\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("src/a.rs")).unwrap();
        index.write().unwrap();
        fs::write(dir.path().join("src/a.rs"), "a staged and edited\n").unwrap();
        fs::write(dir.path().join("src/b.rs"), "b edited\n").unwrap();
        fs::create_dir_all(dir.path().join("src/new")).unwrap();
        fs::write(dir.path().join("src/new/n.rs"), "n\n").unwrap();
        fs::write(dir.path().join("top.txt"), "top edited\n").unwrap();
        let mut view = view(&dir);
        draw(&mut view, 100, 30);
        assert_eq!(
            unstaged_paths(&view),
            ["src/a.rs", "src/b.rs", "src/new/n.rs", "top.txt"]
        );
        // The first row is the src directory.
        assert_eq!(
            view.unstaged.selected_key(),
            Some(RowKey::Dir("src".into()))
        );
        assert!(view.can_discard_selected());
        view.discard_selected();
        let dialog = view.files_box().unwrap();
        assert_eq!(dialog.title(), "Discard changes to src/?");
        assert_eq!(
            dialog.body(),
            "2 files go back to what is staged, or else what was last committed; 1 untracked file is deleted. This can't be undone."
        );
        let outcome = press(&mut view, KeyCode::Char('y'));
        assert_eq!(
            outcome,
            ChangesOutcome::Notice(StatusLine::info("Discarded changes to src/"))
        );
        // Each file went back to its own version: a.rs to what is
        // staged, which stays staged, and b.rs to what was committed.
        assert_eq!(read(&dir, "src/a.rs"), "a staged\n");
        assert_eq!(read(&dir, "src/b.rs"), "b\n");
        assert!(!dir.path().join("src/new").exists());
        assert_eq!(read(&dir, "top.txt"), "top edited\n");
        assert_eq!(unstaged_paths(&view), ["top.txt"]);
        let staged: Vec<&str> = view
            .files(List::Staged)
            .iter()
            .map(|change| change.path.as_str())
            .collect();
        assert_eq!(staged, ["src/a.rs"]);
        settle(&mut view);
        assert_eq!(unstaged_paths(&view), ["top.txt"]);

        // Nothing to discard in the staged list: the command does
        // nothing there.
        press(&mut view, KeyCode::Tab);
        press(&mut view, KeyCode::Home);
        press(&mut view, KeyCode::Down);
        assert_eq!(view.list, List::Staged);
        assert!(!view.can_discard_selected());
        assert!(view.can_unstage_selected());
        assert_eq!(view.discard_selected(), ChangesOutcome::Continue);
        assert!(view.files_box().is_none());
    }

    #[test]
    fn a_right_click_on_a_row_selects_it_and_asks_for_the_menu() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        configure_user(&repo);
        commit_files(&repo, &[("src/a.rs", "a\n"), ("top.txt", "top\n")], "Base");
        fs::write(dir.path().join("src/a.rs"), "a edited\n").unwrap();
        fs::write(dir.path().join("top.txt"), "top edited\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("top.txt")).unwrap();
        index.write().unwrap();
        let mut view = view(&dir);
        let screen = draw(&mut view, 100, 30);
        let width = view.files_rule.x as usize;
        let left = left_column(&screen, width);
        let src = left.iter().position(|r| r.contains("▾ src")).unwrap() as u16;
        let a = left.iter().position(|r| r.contains("M a.rs")).unwrap() as u16;
        let top = left.iter().position(|r| r.contains("M top.txt")).unwrap() as u16;

        // A file of the unstaged list.
        view.pane = Pane::Commit;
        let outcome = right_click(&mut view, 3, a);
        assert!(outcome.menu);
        assert_eq!(view.pane, Pane::Unstaged);
        assert_eq!(
            view.unstaged.selected_key(),
            Some(RowKey::File("src/a.rs".into()))
        );
        assert!(view.has_selected_change());
        // A directory is selected, and not folded as a left click would.
        let outcome = right_click(&mut view, 3, src);
        assert!(outcome.menu);
        assert_eq!(
            view.unstaged.selected_key(),
            Some(RowKey::Dir("src".into()))
        );
        assert!(!view.unstaged.dirs_collapsed[0]);
        assert!(!view.has_selected_change());
        assert!(view.can_stage_selected());
        // A file of the staged list makes it the list the diff follows.
        let outcome = right_click(&mut view, 3, top);
        assert!(outcome.menu);
        assert_eq!(view.list, List::Staged);
        assert!(view.can_unstage_selected());
        assert!(!view.can_stage_selected());
        assert!(!view.can_discard_selected());
        // The heading, the rows below the last, and the diff ask for no
        // menu.
        assert!(!right_click(&mut view, 3, 0).menu);
        assert!(!right_click(&mut view, 3, top + 3).menu);
        let content_x = view.content_area.x;
        assert!(!right_click(&mut view, content_x + 3, 3).menu);
        assert_eq!(view.list, List::Staged);
        // The menu's Stage and Unstage do what Space does.
        assert_eq!(view.unstage_selected(), ChangesOutcome::Continue);
        assert!(view.files(List::Staged).is_empty());
        settle(&mut view);
        right_click(&mut view, 3, src);
        assert_eq!(view.stage_selected(), ChangesOutcome::Continue);
        assert!(
            view.files(List::Unstaged)
                .iter()
                .all(|c| c.path == "top.txt")
        );
        settle(&mut view);
    }

    #[test]
    fn the_discard_box_says_what_goes_back_what_is_deleted_and_what_is_left() {
        let change = |kind: ChangeKind, path: &str, submodule: bool| FileChange {
            kind,
            path: path.to_owned(),
            old_path: None,
            additions: 0,
            deletions: 0,
            binary: false,
            submodule,
        };
        let staged: HashSet<&str> = ["a.rs"].into_iter().collect();
        let body = |files: &[FileChange], kept: &[FileChange]| discard_body(files, kept, &staged);
        let modified = change(ChangeKind::Modified, "a.rs", false);
        assert_eq!(
            body(std::slice::from_ref(&modified), &[]),
            "Its unstaged edits are lost: it goes back to what is staged. This can't be undone."
        );
        let deleted = change(ChangeKind::Deleted, "d.rs", false);
        let untracked = change(ChangeKind::Untracked, "u.rs", false);
        let conflict = change(ChangeKind::Conflicted, "c.rs", false);
        let submodule = change(ChangeKind::Modified, "libs/sub", true);
        assert_eq!(
            body(
                &[modified, deleted.clone()],
                std::slice::from_ref(&conflict)
            ),
            "2 files go back to what is staged, or else what was last committed. 1 file in conflict is left alone. This can't be undone."
        );
        assert_eq!(
            body(
                &[untracked.clone(), untracked.clone()],
                &[conflict.clone(), conflict.clone(), submodule.clone()]
            ),
            "2 untracked files are deleted. 2 files in conflict and 1 submodule are left alone. This can't be undone."
        );
        assert_eq!(
            body(&[deleted, untracked], &[submodule]),
            "1 file goes back to what is staged, or else what was last committed; 1 untracked file is deleted. 1 submodule is left alone. This can't be undone."
        );
    }

    /// Install `script` as the repository's hook `name`.
    fn install_hook(dir: &tempfile::TempDir, name: &str, script: &str) {
        let hooks = dir.path().join(".git/hooks");
        fs::create_dir_all(&hooks).unwrap();
        let path = hooks.join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// Wait for the commit whose hooks are running, and the scan after.
    fn finish_commit(view: &mut ChangesView) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while view.committing.is_some() && std::time::Instant::now() < deadline {
            view.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(view.committing.is_none());
        settle(view);
    }

    /// The changes of [`repo_with_changes`] staged, with a message to
    /// commit them with.
    fn ready_to_commit(dir: &tempfile::TempDir) -> ChangesView {
        let mut view = view(dir);
        view.stage_all();
        settle(&mut view);
        view.message = message_editor("Add two");
        view
    }

    #[test]
    fn a_hook_that_stops_the_commit_is_shown_and_can_be_overridden() {
        let dir = repo_with_changes();
        install_hook(
            &dir,
            "pre-commit",
            "echo 'lint: a.rs is not formatted'; exit 1",
        );
        let mut view = ready_to_commit(&dir);
        assert_eq!(ctrl(&mut view, 's'), ChangesOutcome::Continue);
        assert!(matches!(view.hint(), StatusLine::Progress(_)));
        finish_commit(&mut view);
        assert_eq!(
            view.take_notice(),
            Some(StatusLine::error(
                "Could not commit: the pre-commit hook failed (exit code 1)"
            ))
        );
        let dialog = view.hook_box().expect("the hook box shows");
        assert_eq!(dialog.title(), "The pre-commit hook failed (exit code 1)");
        assert_eq!(dialog.action(), Some(COMMIT_ANYWAY));
        assert_eq!(view.hint(), StatusLine::help(HOOK_HELP));
        let screen = draw(&mut view, 100, 30);
        assert!(
            screen
                .iter()
                .any(|r| r.contains("lint: a.rs is not formatted")),
            "{screen:#?}"
        );
        assert_eq!(head_message(&dir), "Base");

        // Enter dismisses, as Dismiss has the keyboard; the message and
        // what is staged stay for another go.
        assert_eq!(press(&mut view, KeyCode::Enter), ChangesOutcome::Continue);
        assert!(view.hook_box().is_none());
        assert_eq!(view.message_text(), "Add two");
        assert_eq!(view.changes.as_ref().unwrap().staged().len(), 3);

        // Again, and this time commit anyway: no hook stands in the way,
        // so the commit is made at once.
        ctrl(&mut view, 's');
        finish_commit(&mut view);
        assert!(view.hook_box().is_some());
        press(&mut view, KeyCode::Tab);
        let ChangesOutcome::Notice(notice) = press(&mut view, KeyCode::Enter) else {
            panic!("the commit says so");
        };
        assert!(notice.text().starts_with("Committed "), "{notice}");
        assert!(view.hook_box().is_none());
        assert_eq!(head_message(&dir), "Add two");
        assert_eq!(view.message_text(), "");
    }

    #[test]
    fn escape_cancels_the_hook_a_commit_waits_on() {
        let dir = repo_with_changes();
        install_hook(&dir, "pre-commit", "sleep 30");
        let mut view = ready_to_commit(&dir);
        ctrl(&mut view, 's');
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while view.committing.as_ref().unwrap().hooks.running().is_none() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            view.hint(),
            StatusLine::progress("committing: running the pre-commit hook… (Esc cancels)")
        );
        // Meanwhile nothing else changes the repository.
        let ChangesOutcome::Notice(notice) = view.unstage_all() else {
            panic!("refused");
        };
        assert!(notice.text().contains("wait for its hooks"), "{notice}");
        press(&mut view, KeyCode::Esc);
        finish_commit(&mut view);
        assert_eq!(
            view.take_notice(),
            Some(StatusLine::info(
                "Commit cancelled: the pre-commit hook was cancelled"
            ))
        );
        assert!(view.hook_box().is_none());
        assert_eq!(head_message(&dir), "Base");
        assert_eq!(view.message_text(), "Add two");
    }

    #[test]
    fn a_failing_post_commit_hook_is_shown_after_the_commit() {
        let dir = repo_with_changes();
        install_hook(&dir, "post-commit", "echo 'could not notify'; exit 1");
        let mut view = ready_to_commit(&dir);
        ctrl(&mut view, 's');
        finish_commit(&mut view);
        let notice = view.take_notice().unwrap();
        assert!(notice.text().starts_with("Committed "), "{notice}");
        assert!(notice.text().ends_with(" Add two"), "{notice}");
        assert_eq!(head_message(&dir), "Add two");
        assert_eq!(view.message_text(), "");
        let dialog = view.hook_box().expect("the hook box shows");
        assert_eq!(dialog.title(), "The post-commit hook failed (exit code 1)");
        assert_eq!(dialog.action(), None);
        assert_eq!(dialog.failure().output_text(), "could not notify");
        press(&mut view, KeyCode::Esc);
        assert!(view.hook_box().is_none());
    }
}
