//! The project's git repository, as the editor shows it: for now its
//! history. The [`history`] module walks the commits of every branch
//! and lays them out as a graph (the [`graph`] module); the [`diff`]
//! module says what a commit changed, file by file, with the context
//! around each change expandable, the [`diff_model`] module puts a
//! cursor and a selection in one file's diff for reading it, and the
//! [`tree`] module arranges those files as a tree of directories. The [`submodules`](mod@submodules) module
//! lists a repository's submodules, each of which has a history of its
//! own, and the commits of one that a change to it moved over. The [`changes`] module is the working tree: what is changed and
//! not yet committed, staged or not, with staging, unstaging,
//! discarding, and committing. The [`fetch`] module brings the remote-tracking
//! references up to date from the remotes, as `git fetch` does, and
//! those of the submodules that need it, as `git fetch`'s on-demand
//! recursion does. The [`checkout`] module moves HEAD, and the working
//! tree with it, to a commit picked from the log, on to whatever
//! branch points there. The [`head`](mod@head) module says where HEAD is, the
//! branch or the commit, for the status bar, and the [`branch`] module
//! makes a new branch there and moves HEAD on to it, or deletes one.
//! The [`restore`](mod@restore) module writes files into the working directory as a commit (or the
//! one before it) has them, leaving the index alone. The [`merge`] and
//! [`rebase`] modules bring another branch's commits into HEAD's, as
//! `git merge` and `git rebase` do, stopping at conflicts for the
//! changes page to resolve, and the [`interactive`] module rewrites
//! HEAD's own commits by a plan, as `git rebase -i` does, stopping for
//! the changes page to edit one, with what they share (and the job that
//! runs any of them in the background) in the [`operation`] module; the
//! [`reset`](mod@reset) module moves HEAD's branch to a commit as `git reset`
//! does, keeping the working directory.
//! Everything goes through libgit2, by way of the `git2` crate; nothing
//! shells out to git.
//!
//! Only the [`changes`], [`fetch`], [`checkout`], [`branch`],
//! [`restore`](mod@restore), [`merge`], [`rebase`], [`interactive`], and [`reset`](mod@reset) modules change the
//! repository. The first two touch only its index,
//! HEAD, and remote-tracking references, except that discarding a
//! change puts a file of the working directory back to the index's
//! version (or deletes it, when untracked); a checkout also rewrites the
//! working directory's files and may make a local branch, and refuses
//! to while there are changes it could lose; a new or deleted branch is
//! only a reference (with its configuration) and HEAD, never the files;
//! and restoring rewrites files
//! of the working directory and nothing else. A merge or rebase
//! (interactive or not) rewrites the working directory and index as a checkout does, and
//! refuses to start while there are changes it could lose; a reset
//! moves HEAD and the index, never the working directory.

pub mod branch;
pub mod changes;
pub mod checkout;
pub mod diff;
pub mod diff_model;
pub mod fetch;
pub mod graph;
pub mod head;
pub mod history;
pub mod interactive;
pub mod merge;
pub mod operation;
pub mod rebase;
pub mod reset;
pub mod restore;
pub mod submodule_conflicts;
pub mod submodules;
pub mod tree;

pub use branch::{BranchError, DeleteBranchError, create_branch, create_branch_in, delete_branch};
pub use changes::{Changes, ConflictSide};
pub use checkout::{Checkout, CheckoutError, CheckoutJob, CheckoutOutcome, CheckoutPlan};
pub use diff::{
    ChangeKind, CommitDetail, DiffLine, DiffRow, FileChange, FileDiff, LineKind, LinesChange,
    LinesTarget, Person, Side, Unshown,
};
pub use diff_model::{DiffModel, EXPAND_LINES, GapAction};
pub use fetch::{Fetch, FetchReport};
pub use graph::{GraphCell, GraphRow, NODE, cells_for};
pub use head::{Head, head, head_of};
pub use history::{Branch, Commit, CommitTime, History, Oid, RefKind, RefLabel, Remote, short_id};
pub use interactive::{Continued, PlanError, RebaseAction, RebasePlan, RebaseStep};
pub use merge::abort_merge;
pub use operation::{
    InProgress, Integration, IntegrationJob, OperationError, Outcome, Target, in_progress,
};
pub use rebase::{RebaseStatus, abort_rebase, continue_rebase, rebase_status};
pub use reset::{left_behind, reset};
pub use restore::{Restored, restore, unstaged_among};
pub use submodule_conflicts::SubmoduleConflict;
pub use submodules::{
    RANGE_LIMIT, Submodule, SubmoduleRange, UncommittedChange, changed_submodules,
    has_uncommitted_changes, submodule_range, submodules, uncommitted_changes,
};
pub use tree::{Dir, Entry, FileTree, TreeRow};
