//! The project's git repository, as the editor shows it: for now its
//! history. The [`history`] module walks the commits of every branch
//! and lays them out as a graph (the [`graph`] module); the [`diff`]
//! module says what a commit changed, file by file, with the context
//! around each change expandable, and the [`tree`] module arranges
//! those files as a tree of directories. The [`submodules`] module
//! lists a repository's submodules, each of which has a history of its
//! own. Everything goes through libgit2, by way of the `git2` crate;
//! nothing shells out to git.
//!
//! Nothing here changes the repository. Checking out, committing, and
//! the rest of a git workflow will build on these views.

pub mod diff;
pub mod graph;
pub mod history;
pub mod submodules;
pub mod tree;

pub use diff::{
    ChangeKind, CommitDetail, DiffLine, DiffRow, FileChange, FileDiff, LineKind, Person, Side,
    Unshown,
};
pub use graph::{GraphCell, GraphRow, NODE, cells_for};
pub use history::{Branch, Commit, CommitTime, History, Oid, RefKind, RefLabel, Remote, short_id};
pub use submodules::{Submodule, submodules};
pub use tree::{Dir, Entry, FileTree, TreeRow};
