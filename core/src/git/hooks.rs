//! Running the repository's hooks around what the editor does to it, as
//! git's command line runs them: libgit2 runs none, so the operations
//! that git would run one for (a commit, a merge, a rebase, a checkout)
//! run it here.
//!
//! A hook is the executable of its name in the hooks directory: the one
//! `core.hooksPath` names (relative to the working tree), else `hooks`
//! in the git directory (the common one, for a linked worktree). It
//! runs in the working tree with git's arguments, its input from
//! nowhere (as git gives most hooks), and its output going to a pty,
//! so that a program in it that colors its output for a terminal does
//! so. Nobody sees that terminal: what the hook writes is kept in a
//! [`Terminal`] of the size [`Hooks::new`] was given, and only a hook
//! that fails has it shown, in its [`HookFailure`]. A hook that passes
//! is never seen at all.
//!
//! Some hooks can stop what they run before ([`Hook::blocks`]): a
//! failing `pre-commit` stops the commit. The operation then returns a
//! [`HookError::Failed`] and leaves the repository as it was (but for
//! whatever the hook itself did), so that it can be done again with
//! [`Hooks::without_verify`], git's `--no-verify`, which skips the
//! hooks that flag skips ([`Hook::skippable`]) and nothing else. Hooks
//! run after the fact (`post-commit`) can't stop anything, and git
//! ignores how they end; one that fails is kept for the frontend to
//! show (see [`Hooks::take_failures`]) and the operation goes on.
//!
//! An operation runs its hooks on its own thread, since they may take a
//! while. A [`Hooks`] is shared with it, cloned: the frontend's copy
//! says which hook is running ([`Hooks::running`]) and can stop it
//! ([`Hooks::cancel`]), which kills the hook and ends the operation
//! with [`HookError::Cancelled`] where it would have failed.

use crate::terminal::{Command, ExitStatus, Output, Session, Terminal};
use git2::Repository;
use std::ffi::OsString;
use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often a running hook is checked for having been cancelled.
const CANCEL_CHECK: Duration = Duration::from_millis(50);

/// The hooks the editor runs, by when git runs them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hook {
    /// Before a commit is made; may stop it.
    PreCommit,
    /// Before a commit's message is settled, to change it; may stop it.
    PrepareCommitMsg,
    /// Once a commit's message is settled, to check or change it; may
    /// stop the commit.
    CommitMsg,
    /// After a commit is made.
    PostCommit,
    /// Before a merge commits the merge it made; may stop it.
    PreMergeCommit,
    /// After a merge or fast-forward.
    PostMerge,
    /// Before a rebase starts; may stop it.
    PreRebase,
    /// After a checkout.
    PostCheckout,
}

impl Hook {
    /// The hook's file name, as git names it.
    pub fn name(self) -> &'static str {
        match self {
            Hook::PreCommit => "pre-commit",
            Hook::PrepareCommitMsg => "prepare-commit-msg",
            Hook::CommitMsg => "commit-msg",
            Hook::PostCommit => "post-commit",
            Hook::PreMergeCommit => "pre-merge-commit",
            Hook::PostMerge => "post-merge",
            Hook::PreRebase => "pre-rebase",
            Hook::PostCheckout => "post-checkout",
        }
    }

    /// Whether the hook failing stops what it runs before.
    pub fn blocks(self) -> bool {
        !matches!(
            self,
            Hook::PostCommit | Hook::PostMerge | Hook::PostCheckout
        )
    }

    /// Whether git's `--no-verify` skips the hook, so that what it
    /// stopped can be done anyway. `prepare-commit-msg` is the one that
    /// stops things that the flag doesn't skip.
    pub fn skippable(self) -> bool {
        matches!(
            self,
            Hook::PreCommit | Hook::CommitMsg | Hook::PreMergeCommit | Hook::PreRebase
        )
    }
}

impl fmt::Display for Hook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A hook that failed, with what it wrote.
pub struct HookFailure {
    pub hook: Hook,
    pub status: ExitStatus,
    /// What the hook wrote, as its terminal showed it.
    pub output: Box<Terminal>,
}

impl HookFailure {
    /// How the hook ended, in words: `exit code 1`, `killed by SIGKILL`.
    pub fn how(&self) -> String {
        match &self.status.signal {
            Some(signal) => format!("killed by {signal}"),
            None => format!("exit code {}", self.status.code),
        }
    }

    /// The numbers of the lines of output that have anything on them
    /// (see [`Terminal::line`]): the scrollback and the screen, less the
    /// blank rows below the last line written. Empty when the hook
    /// wrote nothing.
    pub fn output_lines(&self) -> Range<usize> {
        let first = self.output.first_line();
        let mut end = first + self.output.line_count();
        while end > first
            && self
                .output
                .line(end - 1)
                .is_none_or(|row| row.text().trim().is_empty())
        {
            end -= 1;
        }
        first..end
    }

    /// The output as plain text, a line per row, for tests and logs.
    pub fn output_text(&self) -> String {
        self.output_lines()
            .filter_map(|line| self.output.line(line))
            .map(|row| row.text())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl fmt::Debug for HookFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HookFailure")
            .field("hook", &self.hook)
            .field("status", &self.status)
            .field("output", &self.output_text())
            .finish()
    }
}

impl fmt::Display for HookFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the {} hook failed ({})", self.hook, self.how())
    }
}

/// Why a hook stopped an operation.
#[derive(Debug)]
pub enum HookError {
    /// It failed.
    Failed(HookFailure),
    /// It was cancelled while running.
    Cancelled(Hook),
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HookError::Failed(failure) => failure.fmt(f),
            HookError::Cancelled(hook) => write!(f, "the {hook} hook was cancelled"),
        }
    }
}

impl std::error::Error for HookError {}

/// What an operation's hooks and the frontend share.
#[derive(Default)]
struct Shared {
    cancelled: AtomicBool,
    running: Mutex<Option<Hook>>,
    failures: Mutex<Vec<HookFailure>>,
}

/// How one operation runs the repository's hooks, shared between the
/// operation and the frontend waiting on it. Make one for each
/// operation: once cancelled, it stays cancelled.
#[derive(Clone)]
pub struct Hooks {
    cols: usize,
    rows: usize,
    verify: bool,
    shared: Arc<Shared>,
}

impl Hooks {
    /// Hooks whose output is kept in a terminal `cols` by `rows` (with
    /// scrollback), the size it will be shown at if a hook fails.
    pub fn new(cols: usize, rows: usize) -> Hooks {
        Hooks {
            cols: cols.max(1),
            rows: rows.max(1),
            verify: true,
            shared: Arc::default(),
        }
    }

    /// Skip the hooks git's `--no-verify` skips (see
    /// [`Hook::skippable`]), to do what one of them stopped anyway.
    pub fn without_verify(mut self) -> Hooks {
        self.verify = false;
        self
    }

    /// Kill the hook running, if any, and run no more: the operation
    /// stops where the hook could have stopped it, or goes on without it
    /// where it couldn't.
    pub fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::Relaxed);
    }

    fn is_cancelled(&self) -> bool {
        self.shared.cancelled.load(Ordering::Relaxed)
    }

    /// The hook running now, if any.
    pub fn running(&self) -> Option<Hook> {
        *self
            .shared
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// The hooks that failed after the fact (see [`Hook::blocks`]),
    /// which stopped nothing, since last taken.
    pub fn take_failures(&self) -> Vec<HookFailure> {
        std::mem::take(
            &mut *self
                .shared
                .failures
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        )
    }

    /// Whether `hook` would run in `repo`: it is there, and isn't one
    /// being skipped.
    pub fn will_run(&self, repo: &Repository, hook: Hook) -> bool {
        (self.verify || !hook.skippable()) && find(repo, hook).is_some()
    }

    /// Run `hook` in `repo`, if it is there, with `args` and `env` on
    /// top of the editor's environment, waiting for it to finish. A
    /// hook that [blocks](Hook::blocks) and fails, or is cancelled,
    /// is an error; one that doesn't is kept among the
    /// [failures](Self::take_failures) and isn't.
    pub(super) fn run(
        &self,
        repo: &Repository,
        hook: Hook,
        args: &[&str],
        env: &[(&str, OsString)],
    ) -> Result<(), HookError> {
        if !self.verify && hook.skippable() {
            return Ok(());
        }
        let Some(path) = find(repo, hook) else {
            return Ok(());
        };
        if self.is_cancelled() {
            return if hook.blocks() {
                Err(HookError::Cancelled(hook))
            } else {
                Ok(())
            };
        }
        let mut command = runner(&path).current_dir(hook_cwd(repo));
        for arg in args {
            command = command.arg(arg);
        }
        for (key, value) in env {
            command = command.env(key, value);
        }
        *self
            .shared
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(hook);
        let ran = self.wait_for(&command);
        *self
            .shared
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        let (status, output, cancelled) = ran;
        if status.success() {
            return Ok(());
        }
        if !hook.blocks() {
            if !cancelled {
                let failure = HookFailure {
                    hook,
                    status,
                    output,
                };
                self.shared
                    .failures
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(failure);
            }
            return Ok(());
        }
        if cancelled {
            return Err(HookError::Cancelled(hook));
        }
        Err(HookError::Failed(HookFailure {
            hook,
            status,
            output,
        }))
    }

    /// Run `command` in a pty until it exits, keeping what it writes in
    /// a terminal. Returns how it ended, the terminal, and whether it
    /// was cancelled. A command that can't be started at all is a
    /// failure saying why.
    fn wait_for(&self, command: &Command) -> (ExitStatus, Box<Terminal>, bool) {
        let mut terminal = Box::new(Terminal::new(self.cols, self.rows));
        let (sender, receiver) = mpsc::channel();
        let session = Session::spawn(command, self.cols, self.rows, move |_, output| {
            let _ = sender.send(output);
        });
        let mut session = match session {
            Ok(session) => session,
            Err(err) => {
                terminal.process(format!("could not run the hook: {err}\r\n").as_bytes());
                let status = ExitStatus {
                    code: 127,
                    signal: None,
                };
                return (status, terminal, false);
            }
        };
        let mut cancelled = false;
        let status = loop {
            if !cancelled && self.is_cancelled() {
                cancelled = true;
                session.kill();
            }
            match receiver.recv_timeout(CANCEL_CHECK) {
                Ok(Output::Bytes(bytes)) => {
                    terminal.process(&bytes);
                    // A program asking the terminal something (where the
                    // cursor is) gets its answer, as from any terminal.
                    session.write(terminal.take_responses());
                }
                Ok(Output::Exited(status)) => break status,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    break ExitStatus {
                        code: 1,
                        signal: None,
                    };
                }
            }
        };
        (status, terminal, cancelled)
    }
}

/// The hook's executable in `repo`, if there is one. A file there that
/// isn't executable is no hook, as git (with a warning) takes it.
pub fn find(repo: &Repository, hook: Hook) -> Option<PathBuf> {
    let path = hooks_dir(repo).join(hook.name());
    is_executable(&path).then_some(path)
}

/// Where `repo`'s hooks are: `core.hooksPath`, taken from the working
/// tree when relative, or the git directory's `hooks`.
fn hooks_dir(repo: &Repository) -> PathBuf {
    let configured = repo
        .config()
        .ok()
        .and_then(|config| config.get_path("core.hooksPath").ok());
    match configured {
        Some(path) if path.is_absolute() => path,
        Some(path) => hook_cwd(repo).join(path),
        None => repo.commondir().join("hooks"),
    }
}

/// Where hooks run: the top of the working tree, or the git directory
/// of a bare repository.
fn hook_cwd(repo: &Repository) -> &Path {
    repo.workdir().unwrap_or_else(|| repo.path())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// The command that runs the hook at `path`: by way of the shell, so
/// that its input can come from nowhere, as git gives it, rather than
/// from the pty, where a hook reading it would wait for ever; and so
/// that a script runs by its `#!` line on Windows, as Git for Windows
/// runs it.
fn runner(path: &Path) -> Command {
    #[cfg(windows)]
    let shell = "sh";
    #[cfg(not(windows))]
    let shell = "/bin/sh";
    Command::new(shell)
        .arg("-c")
        .arg("exec \"$0\" \"$@\" </dev/null")
        .arg(path)
}

/// The environment git gives the hooks around a commit: the index being
/// committed, and no editor, since the message is already written.
pub(super) fn commit_env(repo: &Repository) -> Vec<(&'static str, OsString)> {
    vec![
        ("GIT_INDEX_FILE", repo.path().join("index").into_os_string()),
        ("GIT_EDITOR", ":".into()),
    ]
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::git::history::tests::TestRepo;
    use std::fs;

    /// Install `script` (a shell script's body) as `hook` in `repo`.
    pub fn install(repo: &Repository, hook: Hook, script: &str) {
        let dir = repo.commondir().join("hooks");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(hook.name());
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    pub fn hooks() -> Hooks {
        Hooks::new(80, 24)
    }

    #[test]
    fn a_hook_that_passes_is_quiet_and_one_that_fails_says_what_it_wrote() {
        let t = TestRepo::new();
        install(&t.repo, Hook::PreCommit, "echo checking \"$@\"; exit 0");
        let hooks = hooks();
        assert!(hooks.will_run(&t.repo, Hook::PreCommit));
        hooks.run(&t.repo, Hook::PreCommit, &["a"], &[]).unwrap();

        install(
            &t.repo,
            Hook::PreCommit,
            "echo \"in $(pwd -P)\"; echo \"args $1 $2\"; echo \"var $THING\"; exit 3",
        );
        let env = [("THING", OsString::from("set"))];
        let Err(HookError::Failed(failure)) =
            hooks.run(&t.repo, Hook::PreCommit, &["one", "two"], &env)
        else {
            panic!("the hook should fail");
        };
        assert_eq!(failure.hook, Hook::PreCommit);
        assert_eq!(failure.how(), "exit code 3");
        let workdir = t.path().canonicalize().unwrap();
        assert_eq!(
            failure.output_text(),
            format!("in {}\nargs one two\nvar set", workdir.display())
        );
        assert_eq!(failure.output_lines().len(), 3);
    }

    #[test]
    fn missing_and_unexecutable_hooks_and_skipped_ones_do_not_run() {
        let t = TestRepo::new();
        let hooks = hooks();
        assert!(!hooks.will_run(&t.repo, Hook::PreCommit));
        hooks.run(&t.repo, Hook::PreCommit, &[], &[]).unwrap();

        install(&t.repo, Hook::PreCommit, "exit 1");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = t.repo.path().join("hooks/pre-commit");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(!hooks.will_run(&t.repo, Hook::PreCommit));
            hooks.run(&t.repo, Hook::PreCommit, &[], &[]).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(hooks.run(&t.repo, Hook::PreCommit, &[], &[]).is_err());

        // --no-verify skips pre-commit, but not prepare-commit-msg.
        let skipping = Hooks::new(80, 24).without_verify();
        assert!(!skipping.will_run(&t.repo, Hook::PreCommit));
        skipping.run(&t.repo, Hook::PreCommit, &[], &[]).unwrap();
        install(&t.repo, Hook::PrepareCommitMsg, "exit 1");
        assert!(skipping.will_run(&t.repo, Hook::PrepareCommitMsg));
        assert!(
            skipping
                .run(&t.repo, Hook::PrepareCommitMsg, &[], &[])
                .is_err()
        );
    }

    #[test]
    fn hooks_path_is_taken_from_the_working_tree() {
        let t = TestRepo::new();
        let dir = t.path().join("tools/hooks");
        fs::create_dir_all(&dir).unwrap();
        t.repo
            .config()
            .unwrap()
            .set_str("core.hooksPath", "tools/hooks")
            .unwrap();
        let path = dir.join("pre-rebase");
        fs::write(&path, "#!/bin/sh\necho no; exit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let found = find(&t.repo, Hook::PreRebase).unwrap();
        assert_eq!(found.canonicalize().unwrap(), path.canonicalize().unwrap());
        // The usual place is no longer looked at.
        install(&t.repo, Hook::PreCommit, "exit 1");
        assert_eq!(find(&t.repo, Hook::PreCommit), None);
    }

    #[test]
    fn a_failing_hook_after_the_fact_is_kept_rather_than_returned() {
        let t = TestRepo::new();
        install(&t.repo, Hook::PostCommit, "echo oops; exit 1");
        let hooks = hooks();
        hooks.run(&t.repo, Hook::PostCommit, &[], &[]).unwrap();
        let failures = hooks.take_failures();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].hook, Hook::PostCommit);
        assert_eq!(failures[0].output_text(), "oops");
        assert!(hooks.take_failures().is_empty());
    }

    #[test]
    fn a_hook_reading_its_input_gets_none_rather_than_waiting() {
        let t = TestRepo::new();
        install(
            &t.repo,
            Hook::PreCommit,
            "read line; echo \"got [$line]\"; exit 1",
        );
        let Err(HookError::Failed(failure)) = hooks().run(&t.repo, Hook::PreCommit, &[], &[])
        else {
            panic!("the hook should fail");
        };
        assert_eq!(failure.output_text(), "got []");
    }

    #[test]
    fn cancelling_kills_the_hook_and_stops_the_operation() {
        let t = TestRepo::new();
        install(&t.repo, Hook::PreCommit, "sleep 30");
        let hooks = hooks();
        let canceller = hooks.clone();
        let repo_path = t.repo.path().to_path_buf();
        let handle = std::thread::spawn(move || {
            let repo = Repository::open(repo_path).unwrap();
            hooks.run(&repo, Hook::PreCommit, &[], &[])
        });
        let start = std::time::Instant::now();
        while canceller.running().is_none() {
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(canceller.running(), Some(Hook::PreCommit));
        canceller.cancel();
        let result = handle.join().unwrap();
        assert!(matches!(result, Err(HookError::Cancelled(Hook::PreCommit))));
        assert!(start.elapsed() < Duration::from_secs(10));
        assert_eq!(canceller.running(), None);
    }
}
