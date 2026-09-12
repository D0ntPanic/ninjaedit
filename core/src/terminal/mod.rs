//! Terminal emulation: running programs in a pseudo-terminal and keeping
//! the screen their output describes, for a frontend to draw.
//!
//! The pieces, from the program outward:
//!
//! * [`Session`] runs a program in a pty and reports what it writes.
//! * [`Terminal`] is the emulator: it takes those bytes and maintains a
//!   screen of [`Cell`]s, with scrollback, and encodes the user's keys
//!   and mouse for the program.
//! * A frontend owns both, feeds the session's output to the terminal,
//!   draws the terminal's rows, and writes the encoded input back to the
//!   session.
//!
//! Every tool that shows a process (a shell, a build, a debugger, a
//! coding agent) is a session and a terminal; what differs is the
//! command and what the frontend does around it.

pub mod cell;
pub mod emulator;
pub mod grid;
pub mod keys;
pub mod pty;

pub use cell::{Cell, Color, Style, Underline};
pub use emulator::{CursorStyle, Event, Modes, MouseMode, Terminal};
pub use grid::Row;
pub use keys::{Key, Modifiers, MouseButton, MouseEvent, MouseEventKind};
pub use pty::{Command, ExitStatus, Output, Session, SessionId};
