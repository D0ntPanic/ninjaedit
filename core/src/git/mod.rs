//! The project's git repository, as the editor shows it: for now its
//! history. The [`history`] module walks the commits of every branch
//! and lays them out as a graph (the [`graph`] module); the [`diff`]
//! module says what a commit changed, file by file, with the context
//! around each change expandable, and the [`tree`] module arranges
//! those files as a tree of directories. The [`submodules`] module
//! lists a repository's submodules, each of which has a history of its
//! own, and the commits of one that a change to it moved over. The [`changes`] module is the working tree: what is changed and
//! not yet committed, staged or not, with staging, unstaging,
//! discarding, and committing. The [`fetch`] module brings the remote-tracking
//! references up to date from the remotes, as `git fetch` does, and
//! those of the submodules that need it, as `git fetch`'s on-demand
//! recursion does. The [`checkout`] module moves HEAD, and the working
//! tree with it, to a commit picked from the log, on to whatever
//! branch points there. The [`head`] module says where HEAD is, the
//! branch or the commit, for the status bar, and the [`branch`] module
//! makes a new branch there and moves HEAD on to it. The [`restore`]
//! module writes files into the working directory as a commit (or the
//! one before it) has them, leaving the index alone.
//! Everything goes through libgit2, by way of the `git2` crate; nothing
//! shells out to git.
//!
//! Only the [`changes`], [`fetch`], [`checkout`], [`branch`], and
//! [`restore`] modules change the repository. The first two touch only its index,
//! HEAD, and remote-tracking references, except that discarding a
//! change puts a file of the working directory back to the index's
//! version (or deletes it, when untracked); a checkout also rewrites the
//! working directory's files and may make a local branch, and refuses
//! to while there are changes it could lose; a new branch is only a
//! reference and HEAD, never the files; and restoring rewrites files
//! of the working directory and nothing else.

pub mod branch;
pub mod changes;
pub mod checkout;
pub mod diff;
pub mod fetch;
pub mod graph;
pub mod head;
pub mod history;
pub mod restore;
pub mod submodules;
pub mod tree;

pub use branch::{BranchError, create_branch, create_branch_in};
pub use changes::Changes;
pub use checkout::{Checkout, CheckoutError, CheckoutJob, CheckoutOutcome, CheckoutPlan};
pub use diff::{
    ChangeKind, CommitDetail, DiffLine, DiffRow, FileChange, FileDiff, LineKind, Person, Side,
    Unshown,
};
pub use fetch::{Fetch, FetchReport};
pub use graph::{GraphCell, GraphRow, NODE, cells_for};
pub use head::{Head, head, head_of};
pub use history::{Branch, Commit, CommitTime, History, Oid, RefKind, RefLabel, Remote, short_id};
pub use restore::{Restored, restore, unstaged_among};
pub use submodules::{
    RANGE_LIMIT, Submodule, SubmoduleRange, UncommittedChange, changed_submodules,
    has_uncommitted_changes, submodule_range, submodules, uncommitted_changes,
};
pub use tree::{Dir, Entry, FileTree, TreeRow};
