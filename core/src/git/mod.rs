//! The project's git repository, as the editor shows it: for now its
//! history. The [`history`] module walks the commits of every branch
//! and lays them out as a graph (the [`graph`] module); the [`diff`]
//! module says what a commit changed, file by file, with the context
//! around each change expandable, and the [`tree`] module arranges
//! those files as a tree of directories. The [`submodules`] module
//! lists a repository's submodules, each of which has a history of its
//! own. The [`changes`] module is the working tree: what is changed and
//! not yet committed, staged or not, with staging, unstaging, and
//! committing. Everything goes through libgit2, by way of the `git2`
//! crate; nothing shells out to git.
//!
//! Only the [`changes`] module changes the repository, and only its
//! index and HEAD: nothing here touches the working directory's files.

pub mod changes;
pub mod diff;
pub mod graph;
pub mod history;
pub mod submodules;
pub mod tree;

pub use changes::Changes;
pub use diff::{
    ChangeKind, CommitDetail, DiffLine, DiffRow, FileChange, FileDiff, LineKind, Person, Side,
    Unshown,
};
pub use graph::{GraphCell, GraphRow, NODE, cells_for};
pub use history::{Branch, Commit, CommitTime, History, Oid, RefKind, RefLabel, Remote, short_id};
pub use submodules::{Submodule, changed_submodules, has_uncommitted_changes, submodules};
pub use tree::{Dir, Entry, FileTree, TreeRow};
