//! Core logic backing the ninjaedit editor: project management, file
//! indexing, file buffers, fuzzy search, and the editor model. The TUI layer
//! in the `tui` crate builds on these types.

pub mod buffer;
pub mod editor;
pub mod fuzzy;
pub mod index;
pub mod project;
pub mod text;

pub use buffer::{FileBuffer, LineEnding};
pub use editor::{Cell, Editor, Movement, Position};
pub use index::{FileIndex, IndexEntry};
pub use project::Project;
