//! The editor's commands, as the command palette (Ctrl+P) lists them:
//! everything a key does that isn't editing the text or moving around
//! in it, plus a few things that shouldn't be a keystroke away.
//!
//! Each command has a name to search for, a line about what it does,
//! and the key bound to it when there is one, so that the palette is
//! also where to learn the keys. The commands without one are for now
//! and then: throwing away a build directory to configure from nothing
//! is wanted when debugging a build, not by accident from a key beside
//! another. Stepping between merge conflicts has no key either, but is
//! wanted several times in a row, so the palette puts the command it
//! last ran at the top when it opens: Ctrl+P, Enter runs it again.
//!
//! Not every command makes sense all the time: saving a file is
//! nothing to offer while the git log stands in the editor's place, and
//! committing is nothing to offer anywhere but the changes page. So
//! the palette lists only the commands that apply to what is showing
//! and what is going on, which each command decides for itself from a
//! [`Context`] the application fills in: see
//! [`Command::is_available`]. That is what lets the page's own keys be
//! commands too (fetching on the git log page, say, or committing on
//! the changes page): each is listed on its page and nowhere else, so
//! the list is never long, and the palette is the place to find every
//! key of the page one is on. A command's key is shown only when it
//! works wherever the command is listed; one that works in one pane of
//! a page but not another (Space in the log to check a commit out) is
//! named in the description instead, so the palette doesn't promise a
//! key that types a letter in the next pane over.
//!
//! The right-click menus (see the `context_menu` module) offer commands
//! from the same list, filtered by the same [`Command::is_available`],
//! so what the mouse is offered and what the palette lists never
//! disagree.
//!
//! The application runs the commands; this module only describes them.
//! The views the modes palette (Ctrl+E) lists sit alongside them in
//! the palette, as does "Run \<target\>" for each build target, but
//! those come from the application, which knows what is open and what
//! the project builds.

/// A command the palette can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    OpenFile,
    SwitchTab,
    Save,
    DiscardChanges,
    CloseTab,
    Find,
    FindNext,
    SearchProject,
    GoToLine,
    NextConflict,
    PreviousConflict,
    Undo,
    Redo,
    Cut,
    Copy,
    /// Copying a git page's diff selection as a patch.
    CopyAsPatch,
    /// Copying a git page's diff selection as the new side has it.
    CopyNewSide,
    Paste,
    SelectAll,
    Build,
    Run,
    StopJob,
    SelectConfiguration,
    SelectTarget,
    DeleteBuildDir,
    DeleteAllBuildDirs,
    /// The build page's Ctrl+N.
    AddBuildEntry,
    /// The build page's Ctrl+D.
    DuplicateBuildEntry,
    /// The build page's Delete.
    RemoveBuildEntry,
    /// The settings page's Ctrl+D.
    ResetSetting,
    /// Wherever the status bar shows a repository.
    NewBranch,
    /// The git log page's F5.
    Fetch,
    /// The git log page's Space.
    CheckoutCommit,
    /// The git log page's checking out of the branch selected in the
    /// sidebar.
    CheckoutBranch,
    /// The git log page's deleting of the local branch selected in the
    /// sidebar.
    DeleteBranch,
    /// The git log page's merging of the branch at the selected commit
    /// into HEAD's.
    MergeCommit,
    /// The git log page's rebasing of HEAD's branch onto the selected
    /// commit.
    RebaseOntoCommit,
    /// The git log page's resetting of HEAD's branch to the selected
    /// commit.
    ResetToCommit,
    /// The git log page's editing of the selected commit of HEAD's
    /// branch, by an interactive rebase.
    EditCommit,
    /// The git log page's merging of the branch selected in the sidebar
    /// into HEAD's.
    MergeBranch,
    /// The git log page's rebasing of HEAD's branch onto the branch
    /// selected in the sidebar.
    RebaseOntoBranch,
    /// The git log page's resetting of HEAD's branch to the branch
    /// selected in the sidebar.
    ResetToBranch,
    /// The changes page's Ctrl+S.
    Commit,
    /// The changes page's `a` in the unstaged list.
    StageAll,
    /// The changes page's `a` in the staged list.
    UnstageAll,
    /// The changes page's Space in the unstaged list.
    StageSelected,
    /// The changes page's Space in the staged list.
    UnstageSelected,
    /// The changes page's discarding of the selected unstaged changes,
    /// once confirmed.
    DiscardSelected,
    /// The changes page's Space in the diff of an unstaged file.
    StageLines,
    /// The changes page's Space in the diff of a staged file.
    UnstageLines,
    /// The changes page's reverting of the selected lines of an
    /// unstaged file, once confirmed.
    RevertLines,
    /// The changes page's resolving of the selected conflicts by taking
    /// our side, once confirmed.
    ResolveOurs,
    /// The changes page's resolving of the selected conflicts by taking
    /// their side, once confirmed.
    ResolveTheirs,
    /// The changes page's `o`, and the git log page's among a commit's
    /// files.
    OpenChange,
    /// `o` in the diff on either git page.
    OpenAtCursor,
    /// The git log page's restoring of the selected files as the
    /// selected commit left them.
    RestoreCommitVersion,
    /// The git log page's restoring of the selected files as they were
    /// before the selected commit.
    RestoreParentVersion,
    /// The changes page's `m`.
    ToggleAmend,
    /// The changes page's Ctrl+S while a rebase is in progress.
    ContinueRebase,
    /// The changes page's giving up of the merge in progress, once
    /// confirmed.
    AbortMerge,
    /// The changes page's giving up of the rebase in progress, once
    /// confirmed.
    AbortRebase,
    SwitchView,
    NextView,
    PreviousView,
    /// The output tool's Ctrl+D once its job is over.
    DismissOutput,
    Quit,
}

/// What the upper part of the screen shows, as far as the commands
/// care.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Editor,
    Settings,
    Build,
    GitLog,
    Changes,
    /// The coding agent's terminal.
    Agent,
}

/// The state of the editor's active file, for the commands that act on
/// it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileContext {
    /// Whether it has unsaved changes.
    pub modified: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub has_selection: bool,
    /// Whether it holds merge conflict markers.
    pub has_conflicts: bool,
    /// Whether Ctrl+G has a search to go on with: one going in the
    /// file, or a query searched before to search for again.
    pub can_find_next: bool,
}

/// The state of the build configuration, for the commands that build.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildContext {
    pub has_roots: bool,
    pub has_configurations: bool,
    /// Whether any root has a target that isn't disabled.
    pub has_targets: bool,
    /// Whether there is a current target to build: Ctrl+B has something
    /// to do.
    pub has_current: bool,
    /// On the build page, whether a configuration or a target is
    /// selected, for Ctrl+D to duplicate.
    pub can_duplicate: bool,
    /// On the build page, whether a root, a configuration, or a target
    /// is selected, for Delete to remove.
    pub can_remove: bool,
}

/// What is showing and what is going on, as far as which commands make
/// sense depends on it. The application fills one in each time the
/// palette opens or its list is refreshed. The default is an editor
/// with nothing open and nothing running: the few commands that always
/// apply are all it lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Context {
    pub page: Page,
    /// The editor's active file, when the editor is showing and has
    /// one; `None` with no file open, and while a page stands in for
    /// the editor, since the editor's keys don't reach it then.
    pub file: Option<FileContext>,
    /// Whether the tab search would have anything to list: a file's
    /// tab, or a git page's repositories.
    pub has_tabs: bool,
    pub build: BuildContext,
    /// Whether a build or run is under way in the output tool.
    pub job_running: bool,
    /// Whether the tool pane shows, so there is a view to move the
    /// keyboard to.
    pub tool_pane_visible: bool,
    /// Whether the output tool shows with its job over, so Ctrl+D would
    /// dismiss it.
    pub output_idle: bool,
    /// On the git log page, whether a commit is selected to check out.
    pub commit_selected: bool,
    /// On the changes page, whether there is anything to stage.
    pub has_unstaged: bool,
    /// On the changes page, whether there is anything to unstage.
    pub has_staged: bool,
    /// On the changes page, or among the selected commit's files on the
    /// git log page, whether a file (not a directory) is selected to
    /// open.
    pub change_selected: bool,
    /// On the changes page, whether a file or directory is selected in
    /// the unstaged list, to stage.
    pub can_stage_selected: bool,
    /// On the changes page, whether a file or directory is selected in
    /// the staged list, to unstage.
    pub can_unstage_selected: bool,
    /// On the changes page, whether what is selected in the unstaged
    /// list has changes that can be discarded: not only conflicts and
    /// submodules.
    pub can_discard_selected: bool,
    /// On the changes page, whether what is selected in the unstaged
    /// list has files in conflict, to resolve by taking a side.
    pub can_resolve_selected: bool,
    /// On the changes page, whether changed lines are selected (or under
    /// the cursor) in the diff of an unstaged file, to stage or revert.
    pub can_stage_lines: bool,
    pub can_revert_lines: bool,
    /// On the changes page, whether changed lines are selected (or under
    /// the cursor) in the diff of a staged file, to unstage.
    pub can_unstage_lines: bool,
    /// On either git page, whether the diff of a file to open is shown,
    /// for its cursor to say where.
    pub can_open_at_cursor: bool,
    /// On the git log page, whether a file or directory is selected
    /// among the selected commit's files, with files to restore: not
    /// only submodules.
    pub can_restore_selected: bool,
    /// On the git log page, with the keyboard in the sidebar, whether a
    /// branch (local or a remote's) is selected there, to check out.
    pub can_checkout_branch: bool,
    /// On the git log page, with the keyboard in the sidebar, whether a
    /// local branch HEAD isn't on is selected there, to delete.
    pub can_delete_branch: bool,
    /// On the git log page, whether the selected commit is one HEAD
    /// isn't at, to rebase onto or reset to.
    pub other_commit_selected: bool,
    /// On the git log page, whether the selected commit is one HEAD
    /// isn't at with a branch at it other than HEAD's, to merge.
    pub can_merge_commit: bool,
    /// On the git log page, whether the selected commit is one of
    /// HEAD's branch that an interactive rebase can edit.
    pub can_edit_commit: bool,
    /// On the git log page, with the keyboard in the sidebar, whether a
    /// branch (local or a remote's) HEAD isn't on is selected there, to
    /// merge, rebase onto, or reset to.
    pub other_branch_selected: bool,
    /// On the changes page, whether a merge is in progress, to commit
    /// or abort.
    pub merging: bool,
    /// On the changes page, whether a rebase is in progress, to
    /// continue or abort.
    pub rebasing: bool,
    /// Whether the status bar shows a repository to make a branch in:
    /// the one the file being edited is in, the shown git page's, or
    /// the project's own. A property of where the user is in the
    /// project, not of what the editor is doing.
    pub has_repository: bool,
    /// On the git pages, whether a diff with lines is shown, to select
    /// in.
    pub shows_diff: bool,
    /// On the git pages, whether anything is selected in the diff shown,
    /// to copy.
    pub diff_has_selection: bool,
    /// On the git pages, whether the diff's selection takes in changed
    /// lines, to copy as a patch.
    pub diff_selection_has_changes: bool,
}

impl Command {
    /// Every command, in the order the palette lists them before
    /// anything is typed: files, searching, editing, building, the
    /// pages, views, and quitting last.
    pub const ALL: [Command; 66] = [
        Command::OpenFile,
        Command::SwitchTab,
        Command::Save,
        Command::DiscardChanges,
        Command::CloseTab,
        Command::Find,
        Command::FindNext,
        Command::SearchProject,
        Command::GoToLine,
        Command::NextConflict,
        Command::PreviousConflict,
        Command::Undo,
        Command::Redo,
        Command::Cut,
        Command::Copy,
        Command::CopyAsPatch,
        Command::CopyNewSide,
        Command::Paste,
        Command::SelectAll,
        Command::Build,
        Command::Run,
        Command::StopJob,
        Command::SelectConfiguration,
        Command::SelectTarget,
        Command::DeleteBuildDir,
        Command::DeleteAllBuildDirs,
        Command::AddBuildEntry,
        Command::DuplicateBuildEntry,
        Command::RemoveBuildEntry,
        Command::ResetSetting,
        Command::NewBranch,
        Command::Fetch,
        Command::CheckoutCommit,
        Command::CheckoutBranch,
        Command::DeleteBranch,
        Command::MergeCommit,
        Command::RebaseOntoCommit,
        Command::EditCommit,
        Command::ResetToCommit,
        Command::MergeBranch,
        Command::RebaseOntoBranch,
        Command::ResetToBranch,
        Command::RestoreCommitVersion,
        Command::RestoreParentVersion,
        Command::Commit,
        Command::StageSelected,
        Command::UnstageSelected,
        Command::StageLines,
        Command::UnstageLines,
        Command::StageAll,
        Command::UnstageAll,
        Command::DiscardSelected,
        Command::RevertLines,
        Command::ResolveOurs,
        Command::ResolveTheirs,
        Command::OpenChange,
        Command::OpenAtCursor,
        Command::ToggleAmend,
        Command::ContinueRebase,
        Command::AbortMerge,
        Command::AbortRebase,
        Command::SwitchView,
        Command::NextView,
        Command::PreviousView,
        Command::DismissOutput,
        Command::Quit,
    ];

    /// Whether the command makes sense now, so the palette lists it.
    /// A command that would only report that there is nothing for it
    /// to do (undo with nothing to undo, a commit to check out with
    /// none selected) is left out, as is one that belongs to a page
    /// that isn't showing; the ones that open something (a file, the
    /// project search, a view) apply everywhere.
    pub fn is_available(self, context: &Context) -> bool {
        let file = context.file;
        let build = context.build;
        let on = |page: Page| context.page == page;
        match self {
            Command::OpenFile | Command::SearchProject | Command::SwitchView | Command::Quit => {
                true
            }
            Command::SwitchTab => context.has_tabs,
            Command::Save
            | Command::CloseTab
            | Command::Find
            | Command::GoToLine
            | Command::Paste => file.is_some(),
            Command::SelectAll => file.is_some() || context.shows_diff,
            Command::DiscardChanges => file.is_some_and(|f| f.modified),
            Command::FindNext => file.is_some_and(|f| f.can_find_next),
            Command::NextConflict | Command::PreviousConflict => {
                file.is_some_and(|f| f.has_conflicts)
            }
            Command::Undo => file.is_some_and(|f| f.can_undo),
            Command::Redo => file.is_some_and(|f| f.can_redo),
            Command::Cut => file.is_some_and(|f| f.has_selection),
            Command::Copy => file.is_some_and(|f| f.has_selection) || context.diff_has_selection,
            Command::CopyNewSide => context.diff_has_selection,
            Command::CopyAsPatch => context.diff_selection_has_changes,
            Command::Build | Command::Run | Command::DeleteBuildDir => build.has_current,
            Command::StopJob => context.job_running,
            Command::SelectConfiguration => build.has_configurations,
            Command::SelectTarget => build.has_targets,
            Command::DeleteAllBuildDirs => build.has_roots,
            Command::AddBuildEntry => on(Page::Build),
            Command::DuplicateBuildEntry => on(Page::Build) && build.can_duplicate,
            Command::RemoveBuildEntry => on(Page::Build) && build.can_remove,
            Command::ResetSetting => on(Page::Settings),
            Command::NewBranch => context.has_repository,
            Command::Fetch => on(Page::GitLog),
            Command::CheckoutCommit => on(Page::GitLog) && context.commit_selected,
            Command::CheckoutBranch => on(Page::GitLog) && context.can_checkout_branch,
            Command::DeleteBranch => on(Page::GitLog) && context.can_delete_branch,
            Command::MergeCommit => on(Page::GitLog) && context.can_merge_commit,
            Command::EditCommit => on(Page::GitLog) && context.can_edit_commit,
            Command::RebaseOntoCommit | Command::ResetToCommit => {
                on(Page::GitLog) && context.other_commit_selected
            }
            Command::MergeBranch | Command::RebaseOntoBranch | Command::ResetToBranch => {
                on(Page::GitLog) && context.other_branch_selected
            }
            Command::Commit => on(Page::Changes) && !context.rebasing,
            Command::ToggleAmend => on(Page::Changes) && !context.merging && !context.rebasing,
            Command::ContinueRebase | Command::AbortRebase => on(Page::Changes) && context.rebasing,
            Command::AbortMerge => on(Page::Changes) && context.merging,
            Command::StageAll => on(Page::Changes) && context.has_unstaged,
            Command::UnstageAll => on(Page::Changes) && context.has_staged,
            Command::OpenChange => {
                (on(Page::Changes) || on(Page::GitLog)) && context.change_selected
            }
            Command::RestoreCommitVersion | Command::RestoreParentVersion => {
                on(Page::GitLog) && context.can_restore_selected
            }
            Command::StageSelected => on(Page::Changes) && context.can_stage_selected,
            Command::UnstageSelected => on(Page::Changes) && context.can_unstage_selected,
            Command::StageLines => on(Page::Changes) && context.can_stage_lines,
            Command::UnstageLines => on(Page::Changes) && context.can_unstage_lines,
            Command::RevertLines => on(Page::Changes) && context.can_revert_lines,
            Command::OpenAtCursor => {
                (on(Page::Changes) || on(Page::GitLog)) && context.can_open_at_cursor
            }
            Command::DiscardSelected => on(Page::Changes) && context.can_discard_selected,
            Command::ResolveOurs | Command::ResolveTheirs => {
                on(Page::Changes) && context.can_resolve_selected
            }
            Command::NextView | Command::PreviousView => context.tool_pane_visible,
            Command::DismissOutput => context.output_idle,
        }
    }

    /// The name the palette shows and matches first.
    pub fn label(self) -> &'static str {
        match self {
            Command::OpenFile => "Open file",
            Command::SwitchTab => "Switch tab",
            Command::Save => "Save file",
            Command::DiscardChanges => "Discard unsaved changes",
            Command::CloseTab => "Close tab",
            Command::Find => "Find in file",
            Command::FindNext => "Find next",
            Command::SearchProject => "Search in project",
            Command::GoToLine => "Go to line",
            Command::NextConflict => "Next conflict",
            Command::PreviousConflict => "Previous conflict",
            Command::Undo => "Undo",
            Command::Redo => "Redo",
            Command::Cut => "Cut",
            Command::Copy => "Copy",
            Command::CopyAsPatch => "Copy as patch",
            Command::CopyNewSide => "Copy new side",
            Command::Paste => "Paste",
            Command::SelectAll => "Select all",
            Command::Build => "Build",
            Command::Run => "Run",
            Command::StopJob => "Stop build or run",
            Command::SelectConfiguration => "Select build configuration",
            Command::SelectTarget => "Select build target",
            Command::DeleteBuildDir => "Delete build directory",
            Command::DeleteAllBuildDirs => "Delete all build directories",
            Command::AddBuildEntry => "Add build configuration or target",
            Command::DuplicateBuildEntry => "Duplicate build configuration or target",
            Command::RemoveBuildEntry => "Remove build configuration, target, or root",
            Command::ResetSetting => "Reset setting to default",
            Command::NewBranch => "Create branch",
            Command::Fetch => "Fetch from remotes",
            Command::CheckoutCommit => "Check out commit",
            Command::CheckoutBranch => "Check out branch",
            Command::DeleteBranch => "Delete branch",
            Command::MergeCommit => "Merge into current branch",
            Command::RebaseOntoCommit => "Rebase current branch onto commit",
            Command::ResetToCommit => "Reset current branch to commit",
            Command::EditCommit => "Edit commit",
            Command::MergeBranch => "Merge branch into current branch",
            Command::RebaseOntoBranch => "Rebase current branch onto branch",
            Command::ResetToBranch => "Reset current branch to branch",
            Command::Commit => "Commit",
            Command::StageAll => "Stage all changes",
            Command::UnstageAll => "Unstage all changes",
            Command::OpenChange => "Open changed file",
            Command::RestoreCommitVersion => "Restore this version",
            Command::RestoreParentVersion => "Restore previous version",
            Command::StageSelected => "Stage changes",
            Command::UnstageSelected => "Unstage changes",
            Command::StageLines => "Stage lines",
            Command::UnstageLines => "Unstage lines",
            Command::RevertLines => "Revert lines",
            Command::OpenAtCursor => "Open file at cursor",
            Command::DiscardSelected => "Discard changes",
            Command::ResolveOurs => "Resolve using ours",
            Command::ResolveTheirs => "Resolve using theirs",
            Command::ToggleAmend => "Toggle amend",
            Command::ContinueRebase => "Continue rebase",
            Command::AbortMerge => "Abort merge",
            Command::AbortRebase => "Abort rebase",
            Command::SwitchView => "Switch view",
            Command::NextView => "Focus next view",
            Command::PreviousView => "Focus previous view",
            Command::DismissOutput => "Dismiss output",
            Command::Quit => "Quit",
        }
    }

    /// A line about what the command does, shown after the name and
    /// matched when the name doesn't.
    pub fn description(self) -> &'static str {
        match self {
            Command::OpenFile => "Search the project's files and open one",
            Command::SwitchTab => "Search the open tabs and switch to one",
            Command::Save => "Save the active file",
            Command::DiscardChanges => {
                "Reload the active file from disk, dropping its unsaved edits (undo brings them back)"
            }
            Command::CloseTab => "Close the active file's tab",
            Command::Find => "Search the active file",
            Command::FindNext => "Go to the next match of the last search",
            Command::SearchProject => "Search every file in the project",
            Command::GoToLine => "Move the cursor to a line by number",
            Command::NextConflict => {
                "Move the cursor to the start of the next merge conflict, wrapping around the end of the file"
            }
            Command::PreviousConflict => {
                "Move the cursor to the start of the previous merge conflict, wrapping around the start of the file"
            }
            Command::Undo => "Undo the last edit",
            Command::Redo => "Redo the last undone edit",
            Command::Cut => "Cut the selection to the clipboard",
            Command::Copy => "Copy the selection to the clipboard",
            Command::CopyAsPatch => {
                "On a git page, copy the changes on the lines selected in the diff as a patch in git's format, to apply with git apply to the file as it was before them"
            }
            Command::CopyNewSide => {
                "On a git page, copy the text selected in the diff as it is after the change: the added and unchanged lines, without the removed ones"
            }
            Command::Paste => "Paste from the clipboard",
            Command::SelectAll => "Select the whole file, or the whole diff on a git page",
            Command::Build => "Build the current target with the current configuration",
            Command::Run => "Build and run the current target",
            Command::StopJob => {
                "End the build or run under way in the output tool, as Ctrl+C in it does"
            }
            Command::SelectConfiguration => "Pick the build configuration to build with",
            Command::SelectTarget => "Pick the target to build and run",
            Command::DeleteBuildDir => {
                "Clean: delete what the current configuration has built, to build again from nothing"
            }
            Command::DeleteAllBuildDirs => {
                "Clean everything: delete what every configuration of every build root has built"
            }
            Command::AddBuildEntry => {
                "On the build page, add to the list the selection is in: a configuration or a target, or a build root with a root or nothing selected"
            }
            Command::DuplicateBuildEntry => {
                "On the build page, copy the selected configuration or target and select the copy"
            }
            Command::RemoveBuildEntry => {
                "On the build page, remove what is selected (Delete in the tree), on a second press of Delete; a discovered target is disabled instead, or enabled again"
            }
            Command::ResetSetting => {
                "On the settings page, put the focused setting back to its default"
            }
            Command::NewBranch => {
                "Make a new branch at the current commit of the repository the status bar shows (a submodule's when in one) and switch to it, leaving every file as it is"
            }
            Command::Fetch => {
                "On the git log page, fetch every remote of the repository in the background and refresh the page"
            }
            Command::CheckoutCommit => {
                "On the git log page, check out the selected commit (Space in the log): its branch, a new one tracking a remote's, or HEAD detached"
            }
            Command::CheckoutBranch => {
                "On the git log page, check out the branch selected in the sidebar: a local branch as it is, a remote's on the local branch tracking it (fast-forwarded) or a new one made to track it"
            }
            Command::DeleteBranch => {
                "On the git log page, delete the local branch selected in the sidebar, asking first if it has commits that neither HEAD nor its upstream has"
            }
            Command::MergeCommit => {
                "On the git log page, merge the branch at the selected commit into the branch HEAD is on (git merge): a fast-forward when it can be, else a merge commit; conflicts stop it on the changes page, to resolve and commit"
            }
            Command::RebaseOntoCommit => {
                "On the git log page, replay the commits of the branch HEAD is on onto the selected commit (git rebase); conflicts stop it on the changes page, to resolve and continue"
            }
            Command::EditCommit => {
                "On the git log page, rewrite the selected commit of the branch HEAD is on (git rebase -i, edit): the changes page opens with its changes staged, to reword, change, or split it into several commits; the commits after it are replayed once nothing is left unstaged"
            }
            Command::ResetToCommit => {
                "On the git log page, move the branch HEAD is on to the selected commit (git reset, mixed): the index follows, the working directory stays as it is, so what differs shows as unstaged changes; asks first if commits would be left on no branch"
            }
            Command::MergeBranch => {
                "On the git log page, merge the branch selected in the sidebar into the branch HEAD is on (git merge): a fast-forward when it can be, else a merge commit; conflicts stop it on the changes page, to resolve and commit"
            }
            Command::RebaseOntoBranch => {
                "On the git log page, replay the commits of the branch HEAD is on onto the branch selected in the sidebar (git rebase); conflicts stop it on the changes page, to resolve and continue"
            }
            Command::ResetToBranch => {
                "On the git log page, move the branch HEAD is on to where the branch selected in the sidebar is (git reset, mixed), keeping the working directory as it is; asks first if commits would be left on no branch"
            }
            Command::Commit => {
                "On the changes page, commit what is staged with the message in the box"
            }
            Command::StageAll => {
                "On the changes page, stage every unstaged file (a in the unstaged list)"
            }
            Command::UnstageAll => {
                "On the changes page, unstage every staged file (a in the staged list)"
            }
            Command::OpenChange => {
                "Open the selected changed file in the editor (o): on the changes page at its first conflict if it has one, a submodule going to its tab; on the git log page as it is in the working directory now"
            }
            Command::RestoreCommitVersion => {
                "On the git log page, put the selected file in the working directory as the selected commit left it, or every file the commit changed under the selected directory; the index is untouched, so it shows as an unstaged change"
            }
            Command::RestoreParentVersion => {
                "On the git log page, put the selected file in the working directory as it was before the selected commit, undoing the commit's change to it, or every file the commit changed under the selected directory; the index is untouched, so it shows as an unstaged change"
            }
            Command::StageSelected => {
                "On the changes page, stage the selected file, or every file under the selected directory (Space in the unstaged list)"
            }
            Command::StageLines => {
                "On the changes page, stage the changes on the lines selected in an unstaged file's diff, or on the line under the cursor (Space in the diff)"
            }
            Command::UnstageLines => {
                "On the changes page, unstage the changes on the lines selected in a staged file's diff, or on the line under the cursor (Space in the diff)"
            }
            Command::RevertLines => {
                "On the changes page, throw away the working directory's changes on the lines selected in an unstaged file's diff, or on the line under the cursor, after asking: they go back to what is staged, or else committed"
            }
            Command::OpenAtCursor => {
                "Open the file of the diff shown in the editor, at the line the diff's cursor is on (o in the diff)"
            }
            Command::UnstageSelected => {
                "On the changes page, unstage the selected file, or every file under the selected directory (Space in the staged list)"
            }
            Command::DiscardSelected => {
                "On the changes page, throw away the unstaged changes of the selected file or directory, after asking: files go back to what is staged, or else committed, and untracked ones are deleted"
            }
            Command::ResolveOurs => {
                "On the changes page, resolve the selected conflicted file, or every one under the selected directory, by taking our side's version and staging it, after asking (git checkout --ours): in a merge the branch merged into, in a rebase what it is rebasing onto"
            }
            Command::ResolveTheirs => {
                "On the changes page, resolve the selected conflicted file, or every one under the selected directory, by taking their side's version and staging it, after asking (git checkout --theirs): in a merge the branch merged in, in a rebase the commit being replayed"
            }
            Command::ToggleAmend => {
                "On the changes page, make the commit replace the last one (git commit --amend), or follow it (m in a list)"
            }
            Command::ContinueRebase => {
                "On the changes page, commit the commit the rebase stopped at, its conflicts resolved and staged, with the message in the box, and replay the rest (git rebase --continue); in an interactive rebase, commit what is staged, and go on once nothing is left unstaged"
            }
            Command::AbortMerge => {
                "On the changes page, give up the merge in progress after asking, putting the branch and its files back as they were before it (git merge --abort)"
            }
            Command::AbortRebase => {
                "On the changes page, give up the rebase in progress after asking, putting the branch and its files back as they were before it (git rebase --abort)"
            }
            Command::SwitchView => "Editor, a page, or a tool",
            Command::NextView => "Move the keyboard to the next view",
            Command::PreviousView => "Move the keyboard to the previous view",
            Command::DismissOutput => {
                "Hide the output tool now that its job is over, giving the editor the keyboard (Ctrl+D in it)"
            }
            Command::Quit => "Quit the editor",
        }
    }

    /// The key the command is bound to, if any, as the palette shows
    /// it: a key that does the command wherever the command is listed.
    /// A page's key that works in only one of its panes is named in
    /// the description instead.
    pub fn shortcut(self) -> Option<&'static str> {
        Some(match self {
            Command::OpenFile => "Ctrl+O",
            Command::SwitchTab => "Ctrl+T",
            Command::Save => "Ctrl+S",
            Command::CloseTab => "Ctrl+W",
            Command::Find => "Ctrl+F",
            Command::FindNext => "Ctrl+G",
            Command::SearchProject => "Ctrl+Shift+F",
            Command::GoToLine => "Ctrl+J",
            Command::Undo => "Ctrl+Z",
            Command::Redo => "Ctrl+Y",
            Command::Cut => "Ctrl+X",
            Command::Copy => "Ctrl+C",
            Command::Paste => "Ctrl+V",
            Command::SelectAll => "Ctrl+A",
            Command::Build => "Ctrl+B",
            Command::Run => "Ctrl+R",
            Command::AddBuildEntry => "Ctrl+N",
            Command::DuplicateBuildEntry => "Ctrl+D",
            Command::ResetSetting => "Ctrl+D",
            Command::Fetch => "F5",
            Command::Commit | Command::ContinueRebase => "Ctrl+S",
            Command::SwitchView => "Ctrl+E",
            Command::NextView => "Ctrl+.",
            Command::PreviousView => "Ctrl+,",
            Command::Quit => "Ctrl+Q",
            Command::DiscardChanges
            | Command::NextConflict
            | Command::PreviousConflict
            | Command::StopJob
            | Command::SelectConfiguration
            | Command::SelectTarget
            | Command::DeleteBuildDir
            | Command::DeleteAllBuildDirs
            | Command::RemoveBuildEntry
            | Command::NewBranch
            | Command::CheckoutCommit
            | Command::CheckoutBranch
            | Command::DeleteBranch
            | Command::MergeCommit
            | Command::RebaseOntoCommit
            | Command::EditCommit
            | Command::ResetToCommit
            | Command::MergeBranch
            | Command::RebaseOntoBranch
            | Command::ResetToBranch
            | Command::AbortMerge
            | Command::AbortRebase
            | Command::StageAll
            | Command::UnstageAll
            | Command::OpenChange
            | Command::RestoreCommitVersion
            | Command::RestoreParentVersion
            | Command::StageSelected
            | Command::UnstageSelected
            | Command::CopyAsPatch
            | Command::CopyNewSide
            | Command::StageLines
            | Command::UnstageLines
            | Command::RevertLines
            | Command::OpenAtCursor
            | Command::DiscardSelected
            | Command::ResolveOurs
            | Command::ResolveTheirs
            | Command::ToggleAmend
            | Command::DismissOutput => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available(context: &Context) -> Vec<Command> {
        Command::ALL
            .into_iter()
            .filter(|command| command.is_available(context))
            .collect()
    }

    #[test]
    fn nothing_open_lists_only_what_applies_everywhere() {
        assert_eq!(
            available(&Context::default()),
            [
                Command::OpenFile,
                Command::SearchProject,
                Command::SwitchView,
                Command::Quit
            ]
        );
    }

    #[test]
    fn a_file_brings_the_editor_commands_as_its_state_allows() {
        let mut context = Context {
            file: Some(FileContext::default()),
            has_tabs: true,
            ..Context::default()
        };
        let listed = available(&context);
        for command in [
            Command::SwitchTab,
            Command::Save,
            Command::CloseTab,
            Command::Find,
            Command::GoToLine,
            Command::Paste,
            Command::SelectAll,
        ] {
            assert!(listed.contains(&command), "{command:?} in {listed:?}");
        }
        // Nothing to undo, discard, cut, or step to yet.
        for command in [
            Command::DiscardChanges,
            Command::FindNext,
            Command::NextConflict,
            Command::PreviousConflict,
            Command::Undo,
            Command::Redo,
            Command::Cut,
            Command::Copy,
        ] {
            assert!(!listed.contains(&command), "{command:?} in {listed:?}");
        }
        context.file = Some(FileContext {
            modified: true,
            can_undo: true,
            can_redo: true,
            has_selection: true,
            has_conflicts: true,
            can_find_next: true,
        });
        let listed = available(&context);
        for command in [
            Command::DiscardChanges,
            Command::FindNext,
            Command::NextConflict,
            Command::PreviousConflict,
            Command::Undo,
            Command::Redo,
            Command::Cut,
            Command::Copy,
        ] {
            assert!(listed.contains(&command), "{command:?} in {listed:?}");
        }
    }

    #[test]
    fn a_page_lists_its_own_commands_and_not_the_editors() {
        let context = Context {
            page: Page::Changes,
            has_tabs: true,
            has_unstaged: true,
            ..Context::default()
        };
        let listed = available(&context);
        for command in [
            Command::Commit,
            Command::StageAll,
            Command::ToggleAmend,
            Command::SwitchTab,
            Command::SearchProject,
        ] {
            assert!(listed.contains(&command), "{command:?} in {listed:?}");
        }
        for command in [
            Command::Save,
            Command::UnstageAll,
            Command::OpenChange,
            Command::Fetch,
            Command::ResetSetting,
            Command::AddBuildEntry,
        ] {
            assert!(!listed.contains(&command), "{command:?} in {listed:?}");
        }
        let context = Context {
            page: Page::GitLog,
            commit_selected: true,
            ..Context::default()
        };
        let listed = available(&context);
        assert!(listed.contains(&Command::Fetch));
        assert!(listed.contains(&Command::CheckoutCommit));
        assert!(!listed.contains(&Command::Commit));
    }

    #[test]
    fn the_selected_changes_commands_follow_the_selection() {
        let selection = [
            Command::StageSelected,
            Command::UnstageSelected,
            Command::DiscardSelected,
            Command::ResolveOurs,
            Command::ResolveTheirs,
        ];
        let nothing = Context {
            page: Page::Changes,
            ..Context::default()
        };
        let listed = available(&nothing);
        assert!(selection.iter().all(|c| !listed.contains(c)), "{listed:?}");
        // A row of the unstaged list: stage it, and discard it when it
        // has something to discard.
        let unstaged = Context {
            can_stage_selected: true,
            can_discard_selected: true,
            ..nothing
        };
        let listed = available(&unstaged);
        assert!(listed.contains(&Command::StageSelected));
        assert!(listed.contains(&Command::DiscardSelected));
        assert!(!listed.contains(&Command::UnstageSelected));
        assert!(!listed.contains(&Command::ResolveOurs));
        assert!(!listed.contains(&Command::ResolveTheirs));
        // A conflict: resolve it by taking a side, but not discard it.
        let conflict = Context {
            can_discard_selected: false,
            can_resolve_selected: true,
            ..unstaged
        };
        let listed = available(&conflict);
        assert!(!listed.contains(&Command::DiscardSelected));
        assert!(listed.contains(&Command::ResolveOurs));
        assert!(listed.contains(&Command::ResolveTheirs));
        let staged = Context {
            can_unstage_selected: true,
            ..nothing
        };
        let listed = available(&staged);
        assert!(listed.contains(&Command::UnstageSelected));
        assert!(!listed.contains(&Command::StageSelected));
        // Off the page, none of them, whatever the context says.
        let elsewhere = Context {
            page: Page::GitLog,
            can_resolve_selected: true,
            ..unstaged
        };
        let listed = available(&elsewhere);
        assert!(selection.iter().all(|c| !listed.contains(c)), "{listed:?}");
    }

    #[test]
    fn the_git_log_offers_opening_and_restoring_its_files() {
        let restore = [Command::RestoreCommitVersion, Command::RestoreParentVersion];
        let log = Context {
            page: Page::GitLog,
            ..Context::default()
        };
        let listed = available(&log);
        assert!(!listed.contains(&Command::OpenChange));
        assert!(restore.iter().all(|c| !listed.contains(c)), "{listed:?}");
        // A file: open it and restore it. A directory: restore it.
        let file = Context {
            change_selected: true,
            can_restore_selected: true,
            ..log
        };
        let listed = available(&file);
        assert!(listed.contains(&Command::OpenChange));
        assert!(restore.iter().all(|c| listed.contains(c)), "{listed:?}");
        let dir = Context {
            change_selected: false,
            ..file
        };
        let listed = available(&dir);
        assert!(!listed.contains(&Command::OpenChange));
        assert!(restore.iter().all(|c| listed.contains(c)), "{listed:?}");
        // Restoring is the git log's alone.
        let changes = Context {
            page: Page::Changes,
            ..file
        };
        let listed = available(&changes);
        assert!(listed.contains(&Command::OpenChange));
        assert!(restore.iter().all(|c| !listed.contains(c)), "{listed:?}");
    }

    #[test]
    fn a_changes_diff_offers_its_lines_and_either_page_opening_at_the_cursor() {
        let lines = [
            Command::StageLines,
            Command::UnstageLines,
            Command::RevertLines,
        ];
        let changes = Context {
            page: Page::Changes,
            ..Context::default()
        };
        let listed = available(&changes);
        assert!(lines.iter().all(|c| !listed.contains(c)), "{listed:?}");
        assert!(!listed.contains(&Command::OpenAtCursor));
        // An unstaged file's lines: stage or revert them.
        let unstaged = Context {
            can_stage_lines: true,
            can_revert_lines: true,
            can_open_at_cursor: true,
            ..changes
        };
        let listed = available(&unstaged);
        assert!(listed.contains(&Command::StageLines), "{listed:?}");
        assert!(listed.contains(&Command::RevertLines), "{listed:?}");
        assert!(!listed.contains(&Command::UnstageLines), "{listed:?}");
        assert!(listed.contains(&Command::OpenAtCursor), "{listed:?}");
        // A staged file's lines: unstage them.
        let staged = Context {
            can_unstage_lines: true,
            ..changes
        };
        let listed = available(&staged);
        assert_eq!(
            lines
                .iter()
                .filter(|c| listed.contains(c))
                .collect::<Vec<_>>(),
            [&Command::UnstageLines]
        );
        // The git log opens at the cursor, and has no lines to stage.
        let log = Context {
            page: Page::GitLog,
            ..unstaged
        };
        let listed = available(&log);
        assert!(listed.contains(&Command::OpenAtCursor), "{listed:?}");
        assert!(lines.iter().all(|c| !listed.contains(c)), "{listed:?}");
    }

    #[test]
    fn a_diff_on_a_git_page_offers_selecting_and_copying() {
        for page in [Page::GitLog, Page::Changes] {
            let none = Context {
                page,
                ..Context::default()
            };
            let listed = available(&none);
            assert!(!listed.contains(&Command::SelectAll), "{listed:?}");
            assert!(!listed.contains(&Command::Copy), "{listed:?}");
            // A diff can be selected in, and copied from once it is.
            let diff = Context {
                shows_diff: true,
                ..none
            };
            let listed = available(&diff);
            assert!(listed.contains(&Command::SelectAll), "{listed:?}");
            assert!(!listed.contains(&Command::Copy), "{listed:?}");
            let selected = Context {
                diff_has_selection: true,
                ..diff
            };
            let listed = available(&selected);
            assert!(listed.contains(&Command::Copy), "{listed:?}");
            assert!(listed.contains(&Command::CopyNewSide), "{listed:?}");
            // A patch needs changes among what is selected.
            assert!(!listed.contains(&Command::CopyAsPatch), "{listed:?}");
            let changed = Context {
                diff_selection_has_changes: true,
                ..selected
            };
            assert!(available(&changed).contains(&Command::CopyAsPatch));
            // The editor's other edits stay the editor's.
            assert!(!listed.contains(&Command::Cut), "{listed:?}");
            assert!(!listed.contains(&Command::Paste), "{listed:?}");
        }
    }

    #[test]
    fn the_git_log_offers_merging_rebasing_and_resetting_as_the_selection_allows() {
        let commit_commands = [
            Command::MergeCommit,
            Command::RebaseOntoCommit,
            Command::ResetToCommit,
        ];
        let branch_commands = [
            Command::MergeBranch,
            Command::RebaseOntoBranch,
            Command::ResetToBranch,
        ];
        // HEAD's own commit: nothing to merge, rebase onto, or reset to.
        let log = Context {
            page: Page::GitLog,
            commit_selected: true,
            ..Context::default()
        };
        let listed = available(&log);
        assert!(listed.contains(&Command::CheckoutCommit));
        assert!(
            commit_commands.iter().all(|c| !listed.contains(c)),
            "{listed:?}"
        );
        // Another commit, with no branch to merge.
        let other = Context {
            other_commit_selected: true,
            ..log
        };
        let listed = available(&other);
        assert!(!listed.contains(&Command::MergeCommit));
        assert!(listed.contains(&Command::RebaseOntoCommit));
        assert!(listed.contains(&Command::ResetToCommit));
        let branch = Context {
            can_merge_commit: true,
            ..other
        };
        assert!(available(&branch).contains(&Command::MergeCommit));
        assert!(
            branch_commands
                .iter()
                .all(|c| !available(&branch).contains(c))
        );
        // A branch in the sidebar that HEAD isn't on.
        let sidebar = Context {
            other_branch_selected: true,
            ..log
        };
        let listed = available(&sidebar);
        assert!(
            branch_commands.iter().all(|c| listed.contains(c)),
            "{listed:?}"
        );
        // None of them off the page.
        let elsewhere = Context {
            page: Page::Changes,
            ..branch
        };
        let listed = available(&elsewhere);
        assert!(
            commit_commands.iter().all(|c| !listed.contains(c)),
            "{listed:?}"
        );
    }

    #[test]
    fn the_git_log_offers_editing_a_commit_of_heads_branch() {
        // HEAD's own commit can be edited, though nothing else is
        // offered for it.
        let log = Context {
            page: Page::GitLog,
            commit_selected: true,
            ..Context::default()
        };
        assert!(!available(&log).contains(&Command::EditCommit));
        let editable = Context {
            can_edit_commit: true,
            ..log
        };
        assert!(available(&editable).contains(&Command::EditCommit));
        let elsewhere = Context {
            page: Page::Changes,
            ..editable
        };
        assert!(!available(&elsewhere).contains(&Command::EditCommit));
    }

    #[test]
    fn the_changes_page_continues_or_aborts_what_is_in_progress() {
        let changes = Context {
            page: Page::Changes,
            ..Context::default()
        };
        let listed = available(&changes);
        assert!(listed.contains(&Command::Commit));
        assert!(listed.contains(&Command::ToggleAmend));
        for command in [
            Command::ContinueRebase,
            Command::AbortMerge,
            Command::AbortRebase,
        ] {
            assert!(!listed.contains(&command), "{command:?}");
        }
        let merging = Context {
            merging: true,
            ..changes
        };
        let listed = available(&merging);
        assert!(listed.contains(&Command::Commit));
        assert!(listed.contains(&Command::AbortMerge));
        assert!(!listed.contains(&Command::ToggleAmend));
        assert!(!listed.contains(&Command::AbortRebase));
        let rebasing = Context {
            rebasing: true,
            ..changes
        };
        let listed = available(&rebasing);
        assert!(!listed.contains(&Command::Commit));
        assert!(listed.contains(&Command::ContinueRebase));
        assert!(listed.contains(&Command::AbortRebase));
        assert!(!listed.contains(&Command::AbortMerge));
        assert!(!listed.contains(&Command::ToggleAmend));
    }

    #[test]
    fn a_branch_can_be_made_wherever_there_is_a_repository() {
        assert!(!Command::NewBranch.is_available(&Context::default()));
        for page in [
            Page::Editor,
            Page::Settings,
            Page::Build,
            Page::GitLog,
            Page::Changes,
            Page::Agent,
        ] {
            let context = Context {
                page,
                has_repository: true,
                ..Context::default()
            };
            assert!(
                available(&context).contains(&Command::NewBranch),
                "{page:?}"
            );
        }
    }

    #[test]
    fn building_needs_something_to_build_and_stopping_a_job_running() {
        let context = Context {
            build: BuildContext {
                has_roots: true,
                has_configurations: true,
                has_targets: true,
                has_current: true,
                ..BuildContext::default()
            },
            job_running: true,
            tool_pane_visible: true,
            ..Context::default()
        };
        let listed = available(&context);
        for command in [
            Command::Build,
            Command::Run,
            Command::StopJob,
            Command::SelectConfiguration,
            Command::SelectTarget,
            Command::DeleteBuildDir,
            Command::DeleteAllBuildDirs,
            Command::NextView,
        ] {
            assert!(listed.contains(&command), "{command:?} in {listed:?}");
        }
        assert!(!listed.contains(&Command::DismissOutput));
        let context = Context {
            output_idle: true,
            ..Context::default()
        };
        let listed = available(&context);
        assert!(listed.contains(&Command::DismissOutput));
        assert!(!listed.contains(&Command::StopJob));
        assert!(!listed.contains(&Command::Build));
    }

    #[test]
    fn every_command_has_a_label_and_a_description() {
        for command in Command::ALL {
            assert!(!command.label().is_empty(), "{command:?}");
            assert!(!command.description().is_empty(), "{command:?}");
        }
    }
}
