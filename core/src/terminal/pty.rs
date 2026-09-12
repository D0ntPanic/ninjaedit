//! Running a program in a pseudo-terminal.
//!
//! A [`Session`] spawns a program with a pty as its controlling terminal
//! and reads whatever it writes on a background thread, handing each
//! chunk to a callback as [`Output::Bytes`]. When the program exits the
//! callback gets [`Output::Exited`] once, after all its output. The
//! callback runs on the reader thread, so its job is to wake whoever
//! owns the [`Terminal`](super::Terminal) and let them feed the bytes
//! in; the emulator itself stays on one thread, like the rest of the
//! editor's models.
//!
//! Input goes the other way through [`Session::write`], which queues it
//! for a writer thread so that a program slow to read never holds up the
//! caller. Dropping the session kills the program.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Reads from the pty are this big at most.
const READ_SIZE: usize = 64 * 1024;

/// Identifies a session across threads, so that output arriving after
/// the session is gone can be told apart from a newer session's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(u64);

/// How a program ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitStatus {
    /// The exit code; 1 for a program killed by a signal.
    pub code: u32,
    /// The signal that killed it, if one did.
    pub signal: Option<String>,
}

impl ExitStatus {
    pub fn success(&self) -> bool {
        self.code == 0 && self.signal.is_none()
    }
}

/// What a session reports to its callback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// The program wrote to its terminal.
    Bytes(Vec<u8>),
    /// The program has exited, and there is no more output.
    Exited(ExitStatus),
}

/// A program to run in a session.
#[derive(Clone, Debug)]
pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
}

impl Command {
    pub fn new(program: impl AsRef<OsStr>) -> Command {
        Command {
            program: program.as_ref().to_owned(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
        }
    }

    /// The user's interactive shell: `$SHELL`, or the system's default
    /// when that isn't set.
    pub fn shell() -> Command {
        let program = std::env::var_os("SHELL")
            .filter(|shell| !shell.is_empty())
            .unwrap_or_else(default_shell);
        Command::new(program)
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Command {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn current_dir(mut self, dir: impl AsRef<Path>) -> Command {
        self.cwd = Some(dir.as_ref().to_owned());
        self
    }

    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Command {
        self.env
            .push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    pub fn program(&self) -> &OsStr {
        &self.program
    }

    /// The command as the pty library wants it, with the environment a
    /// terminal program expects: `TERM` naming what this emulator
    /// imitates and `COLORTERM` promising direct color.
    fn builder(&self) -> CommandBuilder {
        let mut builder = CommandBuilder::new(&self.program);
        builder.args(&self.args);
        if let Some(cwd) = &self.cwd {
            builder.cwd(cwd);
        }
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");
        builder.env("TERM_PROGRAM", "ninjaedit");
        for (key, value) in &self.env {
            builder.env(key, value);
        }
        builder
    }
}

#[cfg(windows)]
fn default_shell() -> OsString {
    std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into())
}

#[cfg(not(windows))]
fn default_shell() -> OsString {
    "/bin/sh".into()
}

/// A program running in a pty.
pub struct Session {
    id: SessionId,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    input: mpsc::Sender<Vec<u8>>,
}

impl Session {
    /// Start `command` in a pty `cols` by `rows`, reporting its output to
    /// `sink` from a background thread. `sink` is called with the
    /// session's id so one callback can serve several sessions.
    pub fn spawn(
        command: &Command,
        cols: usize,
        rows: usize,
        mut sink: impl FnMut(SessionId, Output) + Send + 'static,
    ) -> io::Result<Session> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = SessionId(NEXT_ID.fetch_add(1, Ordering::Relaxed));

        let pty = native_pty_system();
        let pair = pty.openpty(size(cols, rows)).map_err(io::Error::other)?;
        let mut child = pair
            .slave
            .spawn_command(command.builder())
            .map_err(io::Error::other)?;
        // The slave end is the child's now. Keeping it open here would
        // mean never reading end of file when the child exits.
        drop(pair.slave);
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader().map_err(io::Error::other)?;
        let mut writer = pair.master.take_writer().map_err(io::Error::other)?;

        let (input, input_rx) = mpsc::channel::<Vec<u8>>();
        thread::Builder::new()
            .name(format!("pty-writer-{}", id.0))
            .spawn(move || {
                use std::io::Write;
                for bytes in input_rx {
                    if writer
                        .write_all(&bytes)
                        .and_then(|()| writer.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        thread::Builder::new()
            .name(format!("pty-reader-{}", id.0))
            .spawn(move || {
                use std::io::Read;
                let mut buf = vec![0; READ_SIZE];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => sink(id, Output::Bytes(buf[..n].to_vec())),
                        // Some systems report a closed pty as an error
                        // rather than end of file.
                        Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
                let status = child
                    .wait()
                    .map(|status| ExitStatus {
                        code: status.exit_code(),
                        signal: status.signal().map(str::to_owned),
                    })
                    .unwrap_or(ExitStatus {
                        code: 1,
                        signal: None,
                    });
                sink(id, Output::Exited(status));
            })?;

        Ok(Session {
            id,
            master: pair.master,
            killer,
            input,
        })
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    /// Send bytes to the program's input. Queued; never blocks.
    pub fn write(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let _ = self.input.send(bytes);
    }

    /// Tell the program its terminal is now `cols` by `rows`.
    pub fn resize(&self, cols: usize, rows: usize) {
        let _ = self.master.resize(size(cols, rows));
    }

    /// Kill the program. Its exit is still reported through the sink.
    pub fn kill(&mut self) {
        let _ = self.killer.kill();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.kill();
    }
}

fn size(cols: usize, rows: usize) -> PtySize {
    PtySize {
        rows: rows.clamp(1, u16::MAX as usize) as u16,
        cols: cols.clamp(1, u16::MAX as usize) as u16,
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::terminal::Terminal;
    use std::time::{Duration, Instant};

    /// Run a command to completion, feeding its output to a terminal.
    fn run(command: &Command, input: &[u8]) -> (Terminal, ExitStatus) {
        let (tx, rx) = mpsc::channel();
        let session = Session::spawn(command, 20, 4, move |id, output| {
            let _ = tx.send((id, output));
        })
        .unwrap();
        session.write(input.to_vec());
        let mut terminal = Terminal::new(20, 4);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (id, output) = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("the program should finish");
            assert_eq!(id, session.id());
            match output {
                Output::Bytes(bytes) => terminal.process(&bytes),
                Output::Exited(status) => return (terminal, status),
            }
        }
    }

    #[test]
    fn runs_a_program_and_reports_its_exit() {
        let command = Command::new("sh")
            .arg("-c")
            .arg("printf 'hi %s' \"$TERM\"; exit 3");
        let (terminal, status) = run(&command, b"");
        assert_eq!(terminal.row_text(0), "hi xterm-256color");
        assert_eq!(status.code, 3);
        assert!(!status.success());
    }

    #[test]
    fn input_reaches_the_program() {
        let command = Command::new("sh")
            .arg("-c")
            .arg("read line; echo \"got $line\"");
        let (terminal, status) = run(&command, b"hello\r");
        assert!(status.success());
        // The pty echoes the typed line, then the program prints its own.
        assert_eq!(terminal.row_text(0), "hello");
        assert_eq!(terminal.row_text(1), "got hello");
    }

    #[test]
    fn dropping_kills_the_program() {
        let (tx, rx) = mpsc::channel();
        let command = Command::new("sh").arg("-c").arg("sleep 30");
        let session = Session::spawn(&command, 10, 2, move |_, output| {
            let _ = tx.send(output);
        })
        .unwrap();
        drop(session);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let output = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("the program should be killed");
            if let Output::Exited(status) = output {
                assert!(!status.success(), "{status:?}");
                break;
            }
        }
    }
}
