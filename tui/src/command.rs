//! The editor's commands, as the command palette (Ctrl+P) lists them:
//! everything a key does that isn't editing the text or moving around
//! in it, plus a few things that shouldn't be a keystroke away.
//!
//! Each command has a name to search for, a line about what it does,
//! and the key bound to it when there is one, so that the palette is
//! also where to learn the keys. The commands without one are for now
//! and then: throwing away a build directory to configure from nothing
//! is wanted when debugging a build, not by accident from a key beside
//! another. The application runs them; this module only describes them.
//! The views the modes palette (Ctrl+E) lists sit alongside them in
//! the palette, but those come from the application, which knows what
//! is open.

/// A command the palette can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    OpenFile,
    SwitchTab,
    Save,
    CloseTab,
    Find,
    FindNext,
    SearchProject,
    GoToLine,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Build,
    Run,
    SelectConfiguration,
    SelectTarget,
    DeleteBuildDir,
    DeleteAllBuildDirs,
    SwitchView,
    NextView,
    PreviousView,
    Quit,
}

impl Command {
    /// Every command, in the order the palette lists them before
    /// anything is typed: files, searching, editing, building, views,
    /// and quitting last.
    pub const ALL: [Command; 24] = [
        Command::OpenFile,
        Command::SwitchTab,
        Command::Save,
        Command::CloseTab,
        Command::Find,
        Command::FindNext,
        Command::SearchProject,
        Command::GoToLine,
        Command::Undo,
        Command::Redo,
        Command::Cut,
        Command::Copy,
        Command::Paste,
        Command::SelectAll,
        Command::Build,
        Command::Run,
        Command::SelectConfiguration,
        Command::SelectTarget,
        Command::DeleteBuildDir,
        Command::DeleteAllBuildDirs,
        Command::SwitchView,
        Command::NextView,
        Command::PreviousView,
        Command::Quit,
    ];

    /// The name the palette shows and matches first.
    pub fn label(self) -> &'static str {
        match self {
            Command::OpenFile => "Open file",
            Command::SwitchTab => "Switch tab",
            Command::Save => "Save file",
            Command::CloseTab => "Close tab",
            Command::Find => "Find in file",
            Command::FindNext => "Find next",
            Command::SearchProject => "Search in project",
            Command::GoToLine => "Go to line",
            Command::Undo => "Undo",
            Command::Redo => "Redo",
            Command::Cut => "Cut",
            Command::Copy => "Copy",
            Command::Paste => "Paste",
            Command::SelectAll => "Select all",
            Command::Build => "Build",
            Command::Run => "Run",
            Command::SelectConfiguration => "Select build configuration",
            Command::SelectTarget => "Select build target",
            Command::DeleteBuildDir => "Delete build directory",
            Command::DeleteAllBuildDirs => "Delete all build directories",
            Command::SwitchView => "Switch view",
            Command::NextView => "Focus next view",
            Command::PreviousView => "Focus previous view",
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
            Command::CloseTab => "Close the active file's tab",
            Command::Find => "Search the active file",
            Command::FindNext => "Go to the next match of the last search",
            Command::SearchProject => "Search every file in the project",
            Command::GoToLine => "Move the cursor to a line by number",
            Command::Undo => "Undo the last edit",
            Command::Redo => "Redo the last undone edit",
            Command::Cut => "Cut the selection to the clipboard",
            Command::Copy => "Copy the selection to the clipboard",
            Command::Paste => "Paste from the clipboard",
            Command::SelectAll => "Select the whole file",
            Command::Build => "Build the current target with the current configuration",
            Command::Run => "Build and run the current target",
            Command::SelectConfiguration => "Pick the build configuration to build with",
            Command::SelectTarget => "Pick the target to build and run",
            Command::DeleteBuildDir => {
                "Clean: delete what the current configuration has built, to build again from nothing"
            }
            Command::DeleteAllBuildDirs => {
                "Clean everything: delete what every configuration of every build root has built"
            }
            Command::SwitchView => "Editor, a page, or a tool",
            Command::NextView => "Move the keyboard to the next view",
            Command::PreviousView => "Move the keyboard to the previous view",
            Command::Quit => "Quit the editor",
        }
    }

    /// The key the command is bound to, if any, as the palette shows it.
    pub fn shortcut(self) -> Option<&'static str> {
        Some(match self {
            Command::OpenFile => "Ctrl+O",
            Command::SwitchTab => "Ctrl+T",
            Command::Save => "Ctrl+S",
            Command::CloseTab => "Ctrl+W",
            Command::Find => "Ctrl+F",
            Command::FindNext => "Ctrl+G",
            Command::SearchProject => "Ctrl+Shift+F",
            Command::GoToLine => "Ctrl+L",
            Command::Undo => "Ctrl+Z",
            Command::Redo => "Ctrl+Y",
            Command::Cut => "Ctrl+X",
            Command::Copy => "Ctrl+C",
            Command::Paste => "Ctrl+V",
            Command::SelectAll => "Ctrl+A",
            Command::Build => "Ctrl+B",
            Command::Run => "Ctrl+R",
            Command::SwitchView => "Ctrl+E",
            Command::NextView => "Ctrl+.",
            Command::PreviousView => "Ctrl+,",
            Command::Quit => "Ctrl+Q",
            Command::SelectConfiguration
            | Command::SelectTarget
            | Command::DeleteBuildDir
            | Command::DeleteAllBuildDirs => return None,
        })
    }
}
