//! The project's git repository, as the editor shows it: for now its
//! history. The [`history`] module walks the commits of every branch
//! and lays them out as a graph (the [`graph`] module); the [`diff`]
//! module says what a commit changed, file by file, with the context
//! around each change expandable, and the [`tree`] module arranges
//! those files as a tree of directories. The [`submodules`] module
//! lists a repository's submodules, each of which has a history of its
//! own, and the commits of one that a change to it moved over. The [`changes`] module is the working tree: what is changed and
//! not yet committed, staged or not, with staging, unstaging, and
//! committing. The [`fetch`] module brings the remote-tracking
//! references up to date from the remotes, as `git fetch` does, and
//! those of the submodules that need it, as `git fetch`'s on-demand
//! recursion does. The [`checkout`] module moves HEAD, and the working
//! tree with it, to a commit picked from the log, on to whatever
//! branch points there.
//! Everything goes through libgit2, by way of the `git2` crate; nothing
//! shells out to git.
//!
//! Only the [`changes`], [`fetch`], and [`checkout`] modules change the
//! repository. The first two touch only its index, HEAD, and
//! remote-tracking references; a checkout also rewrites the working
//! directory's files and may make a local branch, and refuses to while
//! there are changes it could lose.

pub mod changes;
pub mod checkout;
pub mod diff;
pub mod fetch;
pub mod graph;
pub mod history;
pub mod submodules;
pub mod tree;

pub use changes::Changes;
pub use checkout::{Checkout, CheckoutError, CheckoutJob, CheckoutOutcome, CheckoutPlan};
pub use diff::{
    ChangeKind, CommitDetail, DiffLine, DiffRow, FileChange, FileDiff, LineKind, Person, Side,
    Unshown,
};
pub use fetch::{Fetch, FetchReport};
pub use graph::{GraphCell, GraphRow, NODE, cells_for};
pub use history::{Branch, Commit, CommitTime, History, Oid, RefKind, RefLabel, Remote, short_id};
pub use submodules::{
    RANGE_LIMIT, Submodule, SubmoduleRange, UncommittedChange, changed_submodules,
    has_uncommitted_changes, submodule_range, submodules, uncommitted_changes,
};
pub use tree::{Dir, Entry, FileTree, TreeRow};
