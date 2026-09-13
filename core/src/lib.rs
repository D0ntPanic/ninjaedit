//! Core logic backing the ninjaedit editor: project management, file
//! indexing, file buffers, fuzzy search, the editor model, the build
//! configuration, and the settings with the persistent storage they live
//! in. The TUI layer in
//! the `tui` crate builds on these types.

pub mod buffer;
pub mod build;
pub mod editor;
pub mod fuzzy;
pub mod highlight_cache;
pub mod indent;
pub mod index;
pub mod line_edit;
pub mod project;
pub mod project_search;
pub mod search;
pub mod settings;
pub mod storage;
pub mod syntax;
pub mod terminal;
pub mod text;

pub use buffer::{BufferSnapshot, FileBuffer, LineEnding};
pub use build::{
    BuildConfig, BuildRoot, BuildSystem, ConfigurationKey, Job, Selection, Step, TargetKey,
};
pub use editor::{Cell, Editor, Movement, Position};
pub use highlight_cache::HighlightCache;
pub use indent::Indentation;
pub use index::{FileIndex, FileList, IndexEntry};
pub use line_edit::LineEdit;
pub use project::Project;
pub use project_search::{ProjectMatch, ProjectSearch};
pub use search::{Search, SearchStep};
pub use settings::{Category, SettingKey, Settings, SettingsError};
pub use storage::Storage;
pub use syntax::{Highlighter, Language, Token, TokenKind};
pub use terminal::Terminal;
