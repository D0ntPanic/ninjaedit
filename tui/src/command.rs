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
//! The application runs the commands; this module only describes them.
//! The views the modes palette (Ctrl+E) lists sit alongside them in
//! the palette, as does "Run <target>" for each build target, but
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
    /// The git log page's F5.
    Fetch,
    /// The git log page's Space.
    CheckoutCommit,
    /// The changes page's Ctrl+S.
    Commit,
    /// The changes page's `a` in the unstaged list.
    StageAll,
    /// The changes page's `a` in the staged list.
    UnstageAll,
    /// The changes page's `o`.
    OpenChange,
    /// The changes page's `m`.
    ToggleAmend,
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
    /// On the changes page, whether a file (not a directory) is
    /// selected to open.
    pub change_selected: bool,
}

impl Command {
    /// Every command, in the order the palette lists them before
    /// anything is typed: files, searching, editing, building, the
    /// pages, views, and quitting last.
    pub const ALL: [Command; 40] = [
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
        Command::Fetch,
        Command::CheckoutCommit,
        Command::Commit,
        Command::StageAll,
        Command::UnstageAll,
        Command::OpenChange,
        Command::ToggleAmend,
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
            | Command::Paste
            | Command::SelectAll => file.is_some(),
            Command::DiscardChanges => file.is_some_and(|f| f.modified),
            Command::FindNext => file.is_some_and(|f| f.can_find_next),
            Command::NextConflict | Command::PreviousConflict => {
                file.is_some_and(|f| f.has_conflicts)
            }
            Command::Undo => file.is_some_and(|f| f.can_undo),
            Command::Redo => file.is_some_and(|f| f.can_redo),
            Command::Cut | Command::Copy => file.is_some_and(|f| f.has_selection),
            Command::Build | Command::Run | Command::DeleteBuildDir => build.has_current,
            Command::StopJob => context.job_running,
            Command::SelectConfiguration => build.has_configurations,
            Command::SelectTarget => build.has_targets,
            Command::DeleteAllBuildDirs => build.has_roots,
            Command::AddBuildEntry => on(Page::Build),
            Command::DuplicateBuildEntry => on(Page::Build) && build.can_duplicate,
            Command::RemoveBuildEntry => on(Page::Build) && build.can_remove,
            Command::ResetSetting => on(Page::Settings),
            Command::Fetch => on(Page::GitLog),
            Command::CheckoutCommit => on(Page::GitLog) && context.commit_selected,
            Command::Commit | Command::ToggleAmend => on(Page::Changes),
            Command::StageAll => on(Page::Changes) && context.has_unstaged,
            Command::UnstageAll => on(Page::Changes) && context.has_staged,
            Command::OpenChange => on(Page::Changes) && context.change_selected,
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
            Command::Fetch => "Fetch from remotes",
            Command::CheckoutCommit => "Check out commit",
            Command::Commit => "Commit",
            Command::StageAll => "Stage all changes",
            Command::UnstageAll => "Unstage all changes",
            Command::OpenChange => "Open changed file",
            Command::ToggleAmend => "Toggle amend",
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
            Command::Paste => "Paste from the clipboard",
            Command::SelectAll => "Select the whole file",
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
            Command::Fetch => {
                "On the git log page, fetch every remote of the repository in the background and refresh the page"
            }
            Command::CheckoutCommit => {
                "On the git log page, check out the selected commit (Space in the log): its branch, a new one tracking a remote's, or HEAD detached"
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
                "On the changes page, open the selected file in the editor (o in a list), at its first conflict if it has one"
            }
            Command::ToggleAmend => {
                "On the changes page, make the commit replace the last one (git commit --amend), or follow it (m in a list)"
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
            Command::Commit => "Ctrl+S",
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
            | Command::CheckoutCommit
            | Command::StageAll
            | Command::UnstageAll
            | Command::OpenChange
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
