//! Core logic backing the ninjaedit editor: project management, file
//! indexing, file buffers, fuzzy search, and the editor model. The TUI layer
//! in the `tui` crate builds on these types.

pub mod buffer;
pub mod editor;
pub mod fuzzy;
pub mod indent;
pub mod index;
pub mod line_edit;
pub mod project;
pub mod search;
pub mod syntax;
pub mod text;

pub use buffer::{FileBuffer, LineEnding};
pub use editor::{Cell, Editor, Movement, Position};
pub use indent::Indentation;
pub use index::{FileIndex, IndexEntry};
pub use line_edit::LineEdit;
pub use project::Project;
pub use search::{Search, SearchStep};
pub use syntax::{Highlighter, Language, Token, TokenKind};
