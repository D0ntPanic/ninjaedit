//! Core logic backing the ninjaedit editor: project management, file
//! indexing, and file buffers. The TUI layer in the root crate builds on
//! these types.

pub mod buffer;
pub mod index;
pub mod project;

pub use buffer::{FileBuffer, LineEnding};
pub use index::{FileIndex, IndexEntry};
pub use project::Project;
