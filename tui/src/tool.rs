//! Tools shown in the pane below the editor, and the pane that holds
//! them.
//!
//! A [`Tool`] is one running (or finished) program shown in a
//! [`TerminalView`]: a shell for now, later a build, a program's output, a
//! debugger, a coding agent. Each keeps the command that started it so it
//! can be started again after it exits, and the [`Session`] driving the
//! pty while it runs. The session's output reaches the tool from the
//! application, which owns the channel the pty reader threads write to.
//!
//! The [`ToolPane`] is the bottom half of the screen: a stack of tools
//! with one active, shown or hidden as a whole, and a split with the
//! editor that the mouse can drag. Hiding the pane leaves the programs
//! running, so coming back shows the same shell where it was left; only
//! quitting the editor, which drops the sessions, ends them.
//!
//! Several tools are planned, so the pane is a list from the start even
//! though the shell is the only one wired up today. [`ToolKind`] names
//! each kind of tool the editor can open, so the modes palette (Ctrl+E)
//! can list them all and the pane can find the one already running.

use crate::terminal_view::TerminalView;
use ninjaedit_core::terminal::{Command, ExitStatus, Session, SessionId};
use std::io;
use std::path::Path;

/// A kind of tool the editor can open in the pane. Each new tool gets a
/// variant here, and with it a row in the modes palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    Shell,
}

impl ToolKind {
    /// Every kind, in the order the modes palette lists them.
    pub const ALL: [ToolKind; 1] = [ToolKind::Shell];

    /// The short name shown in the tool's tab and the palette.
    pub fn name(self) -> &'static str {
        match self {
            ToolKind::Shell => "Shell",
        }
    }

    /// A line about what the tool does, shown after the name in the
    /// palette.
    pub fn description(self) -> &'static str {
        match self {
            ToolKind::Shell => "Run commands in a shell below the editor",
        }
    }

    /// The command that runs this tool in `directory`.
    fn command(self, directory: &Path) -> Command {
        match self {
            ToolKind::Shell => Command::shell().current_dir(directory),
        }
    }
}

/// The starting fraction of the editor-plus-tool area given to the tool
/// pane.
const DEFAULT_SPLIT: f32 = 0.4;
/// The tool pane's split is kept within these fractions so neither view
/// vanishes entirely to a drag.
const MIN_SPLIT: f32 = 0.1;
const MAX_SPLIT: f32 = 0.9;

/// One program in the tool pane.
pub struct Tool {
    kind: ToolKind,
    /// A short name for the tab (a program that sets a title replaces it).
    name: String,
    /// The view onto the running (or last) program's screen.
    view: TerminalView,
    /// The command, kept so the tool can be restarted after it exits.
    command: Command,
    /// The pty session while the program runs; `None` before it starts or
    /// after it exits.
    session: Option<Session>,
    /// How the program exited, if it has.
    exit: Option<ExitStatus>,
    /// The pty size last sent, to resize it only when the view changes.
    sent_size: (u16, u16),
}

impl Tool {
    /// A tool of `kind` that will run `command`, named `name`, its
    /// terminal starting at `cols` by `rows`.
    pub fn new(
        kind: ToolKind,
        name: impl Into<String>,
        command: Command,
        cols: u16,
        rows: u16,
    ) -> Tool {
        Tool {
            kind,
            name: name.into(),
            view: TerminalView::new(cols, rows),
            command,
            session: None,
            exit: None,
            sent_size: (cols, rows),
        }
    }

    /// A tool of `kind` working in `directory` (for the shell, running the
    /// user's `$SHELL` there).
    pub fn of_kind(kind: ToolKind, directory: &Path, cols: u16, rows: u16) -> Tool {
        Tool::new(kind, kind.name(), kind.command(directory), cols, rows)
    }

    pub fn kind(&self) -> ToolKind {
        self.kind
    }

    pub fn view(&self) -> &TerminalView {
        &self.view
    }

    pub fn view_mut(&mut self) -> &mut TerminalView {
        &mut self.view
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    /// Whether the program is running, so input goes to it and hiding the
    /// pane should keep it alive.
    pub fn is_running(&self) -> bool {
        self.session.is_some()
    }

    /// The tab title: the tool's name, followed by the window title the
    /// program set, if any, so the tab always identifies the tool.
    pub fn title(&self) -> String {
        let title = self.view.title();
        if title.is_empty() {
            self.name.clone()
        } else {
            format!("{} \u{2014} {}", self.name, title)
        }
    }

    /// Start the program if it isn't running, wiping any previous screen so
    /// a restarted shell begins clean. `spawn` runs the command and returns
    /// its session; the application supplies it so the pty output is routed
    /// to its event channel.
    pub fn start(
        &mut self,
        spawn: impl FnOnce(&Command, u16, u16) -> io::Result<Session>,
    ) -> io::Result<()> {
        if self.session.is_some() {
            return Ok(());
        }
        let (cols, rows) = self.view.size();
        if self.exit.is_some() {
            // A restart begins on a fresh screen.
            *self.view.terminal_mut() =
                ninjaedit_core::terminal::Terminal::new(cols as usize, rows as usize);
        }
        let session = spawn(&self.command, cols, rows)?;
        self.sent_size = (cols, rows);
        self.session = Some(session);
        self.exit = None;
        Ok(())
    }

    /// Feed program output to the terminal.
    pub fn process(&mut self, bytes: &[u8]) {
        self.view.process(bytes);
    }

    /// Note that the program exited: drop the session (so a later start
    /// makes a new one) and keep the status.
    pub fn note_exit(&mut self, status: ExitStatus) {
        self.session = None;
        self.exit = Some(status);
    }

    /// Send bytes to the program, if it's running.
    pub fn write(&self, bytes: Vec<u8>) {
        if let Some(session) = &self.session {
            session.write(bytes);
        }
    }

    /// Kill the program. Its exit is reported through the session's sink,
    /// as a normal exit is.
    pub fn kill(&mut self) {
        if let Some(session) = &mut self.session {
            session.kill();
        }
    }

    /// Match the pty's size to the view after a render changed it.
    pub fn sync_size(&mut self) {
        let size = self.view.size();
        if size != self.sent_size
            && let Some(session) = &self.session
        {
            session.resize(size.0 as usize, size.1 as usize);
            self.sent_size = size;
        }
    }
}

/// The pane of tools below the editor.
pub struct ToolPane {
    tools: Vec<Tool>,
    active: usize,
    visible: bool,
    /// The fraction of the editor-and-tool area the pane takes, dragged by
    /// the divider.
    split: f32,
}

impl Default for ToolPane {
    fn default() -> ToolPane {
        ToolPane {
            tools: Vec::new(),
            active: 0,
            visible: false,
            split: DEFAULT_SPLIT,
        }
    }
}

impl ToolPane {
    pub fn is_visible(&self) -> bool {
        self.visible && !self.tools.is_empty()
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn active(&self) -> Option<&Tool> {
        self.tools.get(self.active)
    }

    pub fn active_mut(&mut self) -> Option<&mut Tool> {
        self.tools.get_mut(self.active)
    }

    pub fn set_active(&mut self, index: usize) {
        if index < self.tools.len() {
            self.active = index;
        }
    }

    /// Add a tool and make it active, returning its index.
    pub fn add(&mut self, tool: Tool) -> usize {
        self.tools.push(tool);
        self.active = self.tools.len() - 1;
        self.active
    }

    pub fn tools_mut(&mut self) -> &mut [Tool] {
        &mut self.tools
    }

    /// The tool a session belongs to, for routing its output.
    pub fn tool_of_session(&mut self, id: SessionId) -> Option<&mut Tool> {
        self.tools
            .iter_mut()
            .find(|tool| tool.session().map(Session::id) == Some(id))
    }

    /// The index of the tool a session belongs to.
    pub fn index_of_session(&self, id: SessionId) -> Option<usize> {
        self.tools
            .iter()
            .position(|tool| tool.session().map(Session::id) == Some(id))
    }

    /// The index of the (first) tool of a kind, running or not.
    pub fn index_of_kind(&self, kind: ToolKind) -> Option<usize> {
        self.tools.iter().position(|tool| tool.kind() == kind)
    }

    /// The fraction of the shared area the pane takes.
    pub fn split(&self) -> f32 {
        self.split
    }

    /// Set the split fraction, clamped so neither view disappears.
    pub fn set_split(&mut self, fraction: f32) {
        self.split = fraction.clamp(MIN_SPLIT, MAX_SPLIT);
    }
}
