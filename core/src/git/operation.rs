//! Merging and rebasing: bringing another branch's commits into the one
//! HEAD is on, as `git merge` and `git rebase` do, either of which may
//! stop part way at conflicts for the user to resolve; and rebasing
//! interactively, as `git rebase -i` does, rewriting HEAD's own
//! commits by a plan, which may stop for the user too. The work itself
//! is in the [`merge`](super::merge), [`rebase`](super::rebase), and
//! [`interactive`] modules; this one has what they
//! share: what to merge or rebase onto
//! (a [`Target`]), how it went (an [`Outcome`]), why it couldn't be
//! done (an [`OperationError`]), and the job that runs one in the
//! background ([`IntegrationJob`]), since on a large repository either
//! may write a great many files.
//!
//! Neither starts while the working tree has changes to tracked files,
//! staged or not, since a conflict would mix them in with the merge's,
//! nor while another merge, rebase, or the like is under way (see
//! [`in_progress`]). When HEAD already has every commit of the target
//! there is nothing to do; when the target has every commit of HEAD,
//! HEAD's branch is fast-forwarded to it, as git does by default. The
//! submodules follow the result as they follow a checkout (see the
//! [`checkout`](super::checkout) module), each brought to the commit it
//! now points at, and they follow as the operation goes too: a rebase
//! brings them to each commit it replays before committing it (libgit2
//! won't commit a step while a submodule differs from what the index
//! records), and a merge or rebase that stops at conflicts leaves them
//! where its result so far has them, so that staging a resolution
//! doesn't record a submodule's old commit by mistake.
//!
//! A submodule in conflict (moved on both sides) is resolved where its
//! own branch has already been brought up to date, as `git sub-resolve`
//! does (see the [`submodule_conflicts`](super::submodule_conflicts)
//! module), so that a rebase whose only conflicts are such submodules
//! goes on by itself, and a merge commits. One that can't be resolved
//! so is left in conflict for the user, with the reason.
//!
//! A merge or rebase that stops at conflicts is left in progress, just
//! as git's command line leaves it, with the conflicting files marked
//! in the working directory and the index: the changes page is where
//! they are resolved and staged, and the merge committed or the rebase
//! continued (or either aborted). What is left in the repository is
//! git's own state, so `git merge --continue` or `git rebase
//! --continue` in a shell goes on from there just as well.

use super::checkout::{
    CheckoutError, check_submodules, ensure_clean, follow_index, update_submodules,
};
use super::history::short_id;
use super::hooks::{HookError, Hooks};
use super::interactive::{self, RebaseAction, RebasePlan};
use super::submodule_conflicts::{SubmoduleConflict, resolve_submodule_conflicts};
use git2::build::CheckoutBuilder;
use git2::{AnnotatedCommit, BranchType, Index, Oid, Repository, RepositoryState};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

/// What to merge, or to rebase onto.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A branch, by its name as git has it: `main`, or `origin/main`
    /// for a remote's (`remote`). It is looked up when the operation
    /// runs, so a branch that moved since it was picked is taken where
    /// it is now; a merge's message names it.
    Branch { name: String, remote: bool },
    /// A commit, by id.
    Commit(Oid),
}

impl Target {
    /// The target as the status bar names it: the branch's name, or the
    /// commit's short id.
    pub fn name(&self) -> String {
        match self {
            Target::Branch { name, .. } => name.clone(),
            Target::Commit(id) => short_id(*id),
        }
    }

    /// The target as libgit2 wants it, branch name and all.
    pub(super) fn resolve<'r>(
        &self,
        repo: &'r Repository,
    ) -> Result<AnnotatedCommit<'r>, git2::Error> {
        match self {
            Target::Branch { name, remote } => {
                let kind = if *remote {
                    BranchType::Remote
                } else {
                    BranchType::Local
                };
                let branch = repo.find_branch(name, kind)?;
                repo.reference_to_annotated_commit(branch.get())
            }
            Target::Commit(id) => repo.find_annotated_commit(*id),
        }
    }
}

/// An operation a repository is in the middle of, which has to be
/// finished or aborted before another starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InProgress {
    /// A merge stopped at conflicts, to be committed once they are
    /// resolved.
    Merge,
    /// A rebase stopped at conflicts, to be continued once they are
    /// resolved; or an interactive one stopped there, or for the user
    /// to edit a commit.
    Rebase,
    /// Something begun with git's command line that the editor doesn't
    /// carry on with: a cherry-pick, a revert, a bisect, an interactive
    /// rebase with steps the editor doesn't know, or a `git am` style
    /// rebase. Named as git names it.
    Other(&'static str),
}

/// What the repository is in the middle of, if anything.
pub fn in_progress(repo: &Repository) -> Option<InProgress> {
    match repo.state() {
        RepositoryState::Clean => None,
        RepositoryState::Merge => Some(InProgress::Merge),
        RepositoryState::RebaseMerge => Some(InProgress::Rebase),
        RepositoryState::RebaseInteractive if interactive::is_supported(repo) => {
            Some(InProgress::Rebase)
        }
        RepositoryState::RebaseInteractive => Some(InProgress::Other("interactive rebase")),
        RepositoryState::Rebase | RepositoryState::ApplyMailboxOrRebase => {
            Some(InProgress::Other("rebase"))
        }
        RepositoryState::ApplyMailbox => Some(InProgress::Other("git am")),
        RepositoryState::CherryPick | RepositoryState::CherryPickSequence => {
            Some(InProgress::Other("cherry-pick"))
        }
        RepositoryState::Revert | RepositoryState::RevertSequence => {
            Some(InProgress::Other("revert"))
        }
        RepositoryState::Bisect => Some(InProgress::Other("bisect")),
    }
}

/// Why a merge, rebase, or reset couldn't be done, or carried on.
#[derive(Debug)]
pub enum OperationError {
    /// Another operation is under way.
    InProgress(InProgress),
    /// There is nothing under way to continue or abort.
    NothingInProgress,
    /// HEAD has no commit yet.
    Unborn,
    /// Files are still in conflict: resolve and stage them first. The
    /// submodules among them are named with why they couldn't be
    /// resolved by themselves.
    Unresolved {
        files: usize,
        submodules: Vec<SubmoduleConflict>,
    },
    /// An interactive rebase's stop has nothing staged to commit, but
    /// changes left unstaged (only untracked files, if
    /// `untracked_only`): stage and commit them, or discard them, first.
    Unstaged {
        untracked_only: bool,
    },
    /// The working tree, or a submodule, isn't fit to start: it has
    /// changes that could be lost, or a submodule lacks a commit.
    Checkout(CheckoutError),
    /// The operation was done, but the submodules couldn't all follow.
    SubmodulesBehind(CheckoutError),
    /// The operation stopped part way, left in progress, since the
    /// submodules couldn't follow it there.
    Stopped(CheckoutError),
    /// An interactive rebase's plan can't be carried out as it stands.
    Plan(interactive::PlanError),
    /// A hook stopped it (see the [`hooks`](super::hooks) module),
    /// leaving the repository as it was before, for it to be done again
    /// without the hook if the hook can be skipped.
    Hook(HookError),
    Git(git2::Error),
}

impl From<git2::Error> for OperationError {
    fn from(err: git2::Error) -> OperationError {
        OperationError::Git(err)
    }
}

impl From<HookError> for OperationError {
    fn from(err: HookError) -> OperationError {
        OperationError::Hook(err)
    }
}

impl From<CheckoutError> for OperationError {
    fn from(err: CheckoutError) -> OperationError {
        match err {
            CheckoutError::Git(err) => OperationError::Git(err),
            err => OperationError::Checkout(err),
        }
    }
}

impl fmt::Display for OperationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OperationError::InProgress(InProgress::Merge) => {
                f.write_str("a merge is in progress; commit or abort it on the changes page first")
            }
            OperationError::InProgress(InProgress::Rebase) => f.write_str(
                "a rebase is in progress; continue or abort it on the changes page first",
            ),
            OperationError::InProgress(InProgress::Other(what)) => {
                write!(f, "a {what} is in progress; finish it with git first")
            }
            OperationError::NothingInProgress => f.write_str("no merge or rebase is in progress"),
            OperationError::Unborn => f.write_str("HEAD has no commits yet"),
            OperationError::Unresolved { files, submodules } => {
                let noun = if *files == 1 { "file is" } else { "files are" };
                write!(
                    f,
                    "{files} {noun} still in conflict; resolve and stage them first"
                )?;
                for conflict in submodules {
                    write!(f, "; submodule {}: {}", conflict.path, conflict.why)?;
                }
                Ok(())
            }
            OperationError::Unstaged { untracked_only } => {
                let what = if *untracked_only {
                    "untracked files are"
                } else {
                    "changes are"
                };
                write!(
                    f,
                    "{what} left unstaged; stage and commit them, or discard them, to go on"
                )
            }
            OperationError::Checkout(err) => err.fmt(f),
            OperationError::SubmodulesBehind(err) => {
                write!(f, "done, but the submodules couldn't follow: {err}")
            }
            OperationError::Stopped(err) => write!(
                f,
                "stopped part way, as the submodules couldn't follow: {err}; fix that, then continue or abort on the changes page"
            ),
            OperationError::Plan(err) => err.fmt(f),
            OperationError::Hook(err) => err.fmt(f),
            OperationError::Git(err) => f.write_str(err.message()),
        }
    }
}

impl std::error::Error for OperationError {}

/// How a merge or rebase went, when it didn't fail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// HEAD already has every commit of the target: nothing was done.
    UpToDate,
    /// The target has every commit of HEAD, so HEAD's branch (or a
    /// detached HEAD) was moved forward to it, with this many
    /// submodules brought along.
    FastForwarded { submodules: usize },
    /// A merge commit was made, with `resolved` submodule conflicts
    /// resolved on the way.
    Merged {
        commit: Oid,
        submodules: usize,
        resolved: usize,
    },
    /// An interactive rebase stopped at a step that edits or rewords
    /// `commit` (counting from 1, of how many), as `action` says, with
    /// its changes staged for the user to change and commit.
    Editing {
        step: (usize, usize),
        commit: Oid,
        action: RebaseAction,
    },
    /// HEAD's commits were replayed on the target: `commits` of them,
    /// of which `skipped` were dropped as already there, with
    /// `resolved` submodule conflicts resolved on the way.
    Rebased {
        commits: usize,
        skipped: usize,
        submodules: usize,
        resolved: usize,
    },
    /// Stopped at conflicts in this many files (submodules included),
    /// and left in progress for them to be resolved; a rebase says
    /// which of its commits it stopped at (counting from 1) and how many
    /// it has. The submodules in conflict are named with why they
    /// couldn't be resolved.
    Conflicts {
        files: usize,
        step: Option<(usize, usize)>,
        unresolved: Vec<SubmoduleConflict>,
    },
}

/// A merge or a rebase, as a job runs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Integration {
    /// Merge the target into HEAD's branch.
    Merge(Target),
    /// Rebase HEAD's branch onto the target.
    Rebase(Target),
    /// Rewrite HEAD's own commits as the plan says.
    Interactive(RebasePlan),
}

impl Integration {
    /// Merge or rebase, telling `progress` how many files of the
    /// working tree have been written and how many there are to write.
    /// The repository's hooks run as git's would (see the
    /// [`hooks`](super::hooks) module).
    pub fn run_with(
        &self,
        repo: &Repository,
        hooks: &Hooks,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<Outcome, OperationError> {
        match self {
            Integration::Merge(target) => super::merge::merge(repo, target, hooks, progress),
            Integration::Rebase(target) => super::rebase::rebase(repo, target, hooks, progress),
            Integration::Interactive(plan) => interactive::start(repo, plan, hooks, progress),
        }
    }

    /// What was done, for the status bar, with HEAD's branch named as
    /// `head` (none when detached).
    pub fn summary(&self, head: Option<&str>, outcome: &Outcome) -> String {
        let head = head.unwrap_or("HEAD");
        let target = match self {
            Integration::Merge(target) | Integration::Rebase(target) => target.name(),
            Integration::Interactive(_) => return interactive::summary(head, outcome),
        };
        let what = match outcome {
            Outcome::UpToDate => return format!("{head} is already up to date with {target}"),
            Outcome::FastForwarded { .. } => format!("Fast-forwarded {head} to {target}"),
            Outcome::Merged {
                commit, resolved, ..
            } => with_resolved(
                format!("Merged {target} into {head} as {}", short_id(*commit)),
                *resolved,
            ),
            Outcome::Rebased {
                commits,
                skipped,
                resolved,
                ..
            } => with_resolved(
                rebased_summary(head, &target, *commits, *skipped),
                *resolved,
            ),
            Outcome::Editing { .. } => return interactive::summary(head, outcome),
            Outcome::Conflicts {
                files,
                step,
                unresolved,
            } => {
                let noun = if *files == 1 { "file" } else { "files" };
                let at = match step {
                    Some((step, total)) => format!(" at commit {step} of {total}"),
                    None => String::new(),
                };
                let (doing, then) = match self {
                    Integration::Merge(_) => (format!("Merging {target} into {head}"), "commit"),
                    // An interactive rebase is summarized above.
                    Integration::Rebase(_) | Integration::Interactive(_) => {
                        (format!("Rebasing {head} onto {target}"), "continue")
                    }
                };
                let mut text = format!(
                    "{doing} stopped{at} with conflicts in {files} {noun}: resolve and stage them, then {then}"
                );
                for conflict in unresolved {
                    text.push_str(&format!(
                        "; submodule {} left in conflict: {}",
                        conflict.path, conflict.why
                    ));
                }
                return text;
            }
        };
        with_submodules(what, outcome.submodules())
    }
}

impl Outcome {
    /// How many submodules were brought along.
    fn submodules(&self) -> usize {
        match self {
            Outcome::FastForwarded { submodules }
            | Outcome::Merged { submodules, .. }
            | Outcome::Rebased { submodules, .. } => *submodules,
            Outcome::UpToDate | Outcome::Editing { .. } | Outcome::Conflicts { .. } => 0,
        }
    }
}

/// `Rebased main onto feature: 3 commits`, with any skipped.
pub(super) fn rebased_summary(head: &str, onto: &str, commits: usize, skipped: usize) -> String {
    let noun = if commits == 1 { "commit" } else { "commits" };
    let mut text = format!("Rebased {head} onto {onto}: {commits} {noun}");
    if skipped > 0 {
        text.push_str(&format!(" ({skipped} already there, dropped)"));
    }
    text
}

/// A summary with how many submodule conflicts were resolved, if any.
pub(super) fn with_resolved(what: String, resolved: usize) -> String {
    match resolved {
        0 => what,
        1 => format!("{what}; 1 submodule conflict resolved"),
        n => format!("{what}; {n} submodule conflicts resolved"),
    }
}

/// A summary with how many submodules came along, if any.
pub(super) fn with_submodules(what: String, submodules: usize) -> String {
    match submodules {
        0 => what,
        1 => format!("{what}; 1 submodule updated"),
        n => format!("{what}; {n} submodules updated"),
    }
}

/// Refuse to start while another operation is under way, before HEAD
/// has a commit, or while the working tree or a submodule has changes
/// that could be lost. Returns HEAD's commit.
pub(super) fn ensure_ready(repo: &Repository) -> Result<Oid, OperationError> {
    if let Some(what) = in_progress(repo) {
        return Err(OperationError::InProgress(what));
    }
    let head = head_commit(repo)?;
    ensure_clean(repo)?;
    check_submodules(repo, &repo.find_commit(head)?)?;
    Ok(head)
}

/// HEAD's commit, or [`OperationError::Unborn`] when it has none.
pub(super) fn head_commit(repo: &Repository) -> Result<Oid, OperationError> {
    repo.head()
        .ok()
        .and_then(|head| head.target())
        .ok_or(OperationError::Unborn)
}

/// Checkout options that write what changed and nothing else, refusing
/// to overwrite what differs, and report how it is going.
pub(super) fn checkout_options<'a>(
    progress: &'a mut dyn FnMut(usize, usize),
) -> CheckoutBuilder<'a> {
    let mut options = CheckoutBuilder::new();
    options
        .safe()
        .progress(move |_, completed, total| progress(completed, total));
    options
}

/// Move HEAD's branch (or a detached HEAD) forward to `id`, the working
/// tree with it, and bring the submodules along. Returns how many came.
pub(super) fn fast_forward(
    repo: &Repository,
    id: Oid,
    why: &str,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<usize, OperationError> {
    let commit = repo.find_commit(id)?;
    {
        let mut options = checkout_options(progress);
        repo.checkout_tree(commit.as_object(), Some(&mut options))?;
    }
    repo.head()?.set_target(id, why)?;
    follow_submodules(repo, id, progress)
}

/// Bring the submodules to where the commit `id` points them, now that
/// HEAD is there. Returns how many moved.
pub(super) fn follow_submodules(
    repo: &Repository,
    id: Oid,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<usize, OperationError> {
    update_submodules(repo, &repo.find_commit(id)?, progress)
        .map_err(OperationError::SubmodulesBehind)
}

/// Bring the submodules to where the index has them, part way through
/// a merge or rebase. Returns the paths of those moved.
pub(super) fn follow_part_way(
    repo: &Repository,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Vec<String>, OperationError> {
    follow_index(repo, progress).map_err(OperationError::Stopped)
}

/// What [`settle_part_way`] left.
pub(super) struct Settled {
    /// The submodules moved, by path: those the index has elsewhere,
    /// and those whose conflicts were resolved.
    pub moved: Vec<String>,
    /// How many submodule conflicts were resolved.
    pub resolved: usize,
    /// The submodule conflicts that couldn't be.
    pub unresolved: Vec<SubmoduleConflict>,
}

/// Settle what can be of a merge or rebase step before it is committed
/// or stops at conflicts: bring the submodules to where the index has
/// them, and resolve the submodule conflicts that can be.
pub(super) fn settle_part_way(
    repo: &Repository,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Settled, OperationError> {
    let mut moved = follow_part_way(repo, progress)?;
    if !repo.index()?.has_conflicts() {
        return Ok(Settled {
            moved,
            resolved: 0,
            unresolved: Vec::new(),
        });
    }
    let resolution = resolve_submodule_conflicts(repo, progress)
        .map_err(|err| OperationError::Stopped(CheckoutError::Git(err)))?;
    let resolved = resolution.resolved.len();
    moved.extend(resolution.resolved);
    Ok(Settled {
        moved,
        resolved,
        unresolved: resolution.unresolved,
    })
}

/// How many files of an index are in conflict.
pub(super) fn conflicted_files(index: &Index) -> Result<usize, git2::Error> {
    let mut files = 0;
    for conflict in index.conflicts()? {
        conflict?;
        files += 1;
    }
    Ok(files)
}

enum Message {
    Progress(usize, usize),
    Outcome(Result<Outcome, OperationError>),
}

/// A merge or rebase under way in the background. [`poll`](Self::poll)
/// takes in its progress; once it is done the outcome says how it went.
pub struct IntegrationJob {
    receiver: Receiver<Message>,
    progress: Option<(usize, usize)>,
    done: Option<Result<Outcome, OperationError>>,
}

impl IntegrationJob {
    /// Start merging or rebasing in the repository whose git directory
    /// is `git_dir` (see [`Repository::path`]), running its hooks with
    /// `hooks`.
    pub fn start(
        git_dir: &Path,
        what: Integration,
        hooks: Hooks,
    ) -> Result<IntegrationJob, OperationError> {
        let git_dir: PathBuf = git_dir.to_path_buf();
        let (sender, receiver) = mpsc::channel();
        let name = match what {
            Integration::Merge(_) => "git-merge",
            Integration::Rebase(_) => "git-rebase",
            Integration::Interactive(_) => "git-rebase-interactive",
        };
        thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let outcome = Repository::open(&git_dir)
                    .map_err(OperationError::from)
                    .and_then(|repo| {
                        let mut progress = |completed, total| {
                            let _ = sender.send(Message::Progress(completed, total));
                        };
                        what.run_with(&repo, &hooks, &mut progress)
                    });
                let _ = sender.send(Message::Outcome(outcome));
            })
            .map_err(|err| OperationError::Git(git2::Error::from_str(&err.to_string())))?;
        Ok(IntegrationJob {
            receiver,
            progress: None,
            done: None,
        })
    }

    /// Take in what the job has reported so far. Returns whether
    /// anything changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            match self.receiver.try_recv() {
                Ok(Message::Progress(completed, total)) => {
                    self.progress = Some((completed, total));
                    changed = true;
                }
                Ok(Message::Outcome(outcome)) => {
                    self.done = Some(outcome);
                    changed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.done.is_none() {
                        self.done = Some(Err(OperationError::Git(git2::Error::from_str(
                            "the operation stopped",
                        ))));
                        changed = true;
                    }
                    break;
                }
            }
        }
        changed
    }

    /// How many files have been written, and how many there are to
    /// write, once the job has got that far.
    pub fn progress(&self) -> Option<(usize, usize)> {
        self.progress
    }

    /// Whether the job is finished (as of the last poll).
    pub fn is_done(&self) -> bool {
        self.done.is_some()
    }

    /// The outcome, once [`is_done`](Self::is_done).
    pub fn outcome(self) -> Option<Result<Outcome, OperationError>> {
        self.done
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::git::history::tests::TestRepo;
    use git2::Signature;
    use git2::build::CheckoutBuilder;
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, Instant};

    /// A test repository whose configuration names someone to commit
    /// as, as merging and rebasing need.
    pub fn repo() -> TestRepo {
        let t = TestRepo::new();
        let mut config = t.repo.config().unwrap();
        config.set_str("user.name", "Test Author").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();
        t
    }

    /// Put the working tree and index where HEAD is, as the helpers
    /// that build a history leave them elsewhere.
    pub fn sync(t: &TestRepo) {
        t.repo
            .checkout_head(Some(CheckoutBuilder::new().force().remove_untracked(true)))
            .unwrap();
    }

    /// `main` at a base and a commit of its own changing `f.txt`, and
    /// `side` from the base with a commit changing `side_file`, or
    /// `f.txt` too when `clash` (so the two conflict). HEAD on main.
    pub fn diverged(t: &mut TestRepo, clash: bool) -> (Oid, Oid, Oid) {
        let base = t.commit(&[("f.txt", "base\n")], "Base", &[]);
        let main = t.commit(&[("f.txt", "main\n")], "Main", &[base]);
        let side_files: &[(&str, &str)] = if clash {
            &[("f.txt", "side\n")]
        } else {
            &[("g.txt", "side\n")]
        };
        let head = t.repo.head().unwrap().name().unwrap().to_owned();
        t.branch("side", base);
        t.checkout("side");
        sync(t);
        let side = t.commit(side_files, "Side", &[base]);
        t.repo.set_head(&head).unwrap();
        sync(t);
        (base, main, side)
    }

    /// `main` with a commit of its own after the base, and `side` from
    /// the base moving the submodule `sub` on, from `S1` to `S2`. HEAD
    /// is on main, with the submodule at S1. With `clash`, side's commit
    /// changes `f.txt` too, so that it conflicts with main's. Returns the submodule's
    /// repository, the base, main's commit, side's, and S1 and S2.
    pub fn submodule_moved_on_side(t: &mut TestRepo, clash: bool) -> (Repository, [Oid; 5]) {
        use git2::build::CheckoutBuilder;
        t.commit(&[("f.txt", "base\n")], "Base", &[]);
        let sub = t.add_submodule("sub");
        let s1 = sub.head().unwrap().target().unwrap();
        let base = t.repo.head().unwrap().target().unwrap();
        let main = t.commit(&[("f.txt", "main\n")], "Main", &[base]);
        let head_name = t.repo.head().unwrap().name().unwrap().to_owned();
        t.branch("side", base);
        t.checkout("side");
        sync(t);
        // The submodule moves on, and side records it.
        fs::write(sub.workdir().unwrap().join("inner.txt"), "two\n").unwrap();
        let mut index = sub.index().unwrap();
        index.add_path(Path::new("inner.txt")).unwrap();
        index.write().unwrap();
        let s2 = {
            let tree = sub.find_tree(index.write_tree().unwrap()).unwrap();
            let sig = Signature::now("Sub Author", "sub@example.com").unwrap();
            let parent = sub.find_commit(s1).unwrap();
            sub.commit(Some("HEAD"), &sig, &sig, "Inner two", &tree, &[&parent])
                .unwrap()
        };
        t.repo
            .find_submodule("sub")
            .unwrap()
            .add_to_index(true)
            .unwrap();
        let files: &[(&str, &str)] = if clash { &[("f.txt", "side\n")] } else { &[] };
        let side = t.commit(files, "Bump sub", &[base]);
        // Back on main, the submodule where main has it.
        t.repo.set_head(&head_name).unwrap();
        sync(t);
        sub.set_head_detached(s1).unwrap();
        sub.checkout_head(Some(CheckoutBuilder::new().force()))
            .unwrap();
        assert!(t.repo.statuses(None).unwrap().is_empty(), "clean to start");
        (sub, [base, main, side, s1, s2])
    }

    pub fn read(t: &TestRepo, path: &str) -> String {
        fs::read_to_string(t.path().join(path)).unwrap()
    }

    fn side() -> Target {
        Target::Branch {
            name: "side".to_owned(),
            remote: false,
        }
    }

    #[test]
    fn a_job_merges_in_the_background_and_reports_its_outcome() {
        let mut t = repo();
        let (_, main, side_id) = diverged(&mut t, false);
        let what = Integration::Merge(side());
        let mut job = IntegrationJob::start(
            t.repo.path(),
            what.clone(),
            crate::git::hooks::tests::hooks(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !job.is_done() && Instant::now() < deadline {
            job.poll();
            thread::sleep(Duration::from_millis(5));
        }
        let outcome = job.outcome().unwrap().unwrap();
        let Outcome::Merged { commit, .. } = outcome.clone() else {
            panic!("{outcome:?}");
        };
        let merged = t.repo.find_commit(commit).unwrap();
        assert_eq!(merged.parent_ids().collect::<Vec<_>>(), [main, side_id]);
        assert_eq!(
            what.summary(Some("main"), &outcome),
            format!("Merged side into main as {}", short_id(commit))
        );
        assert_eq!(read(&t, "g.txt"), "side\n");
    }

    #[test]
    fn summaries_say_what_was_done_and_what_is_left_to_do() {
        let merge = Integration::Merge(side());
        let rebase = Integration::Rebase(Target::Commit(Oid::from_str("1a2b3c4d5e").unwrap()));
        assert_eq!(
            merge.summary(Some("main"), &Outcome::UpToDate),
            "main is already up to date with side"
        );
        assert_eq!(
            merge.summary(None, &Outcome::FastForwarded { submodules: 2 }),
            "Fast-forwarded HEAD to side; 2 submodules updated"
        );
        assert_eq!(
            merge.summary(
                Some("main"),
                &Outcome::Conflicts {
                    files: 1,
                    step: None,
                    unresolved: Vec::new(),
                }
            ),
            "Merging side into main stopped with conflicts in 1 file: resolve and stage them, then commit"
        );
        assert_eq!(
            rebase.summary(
                Some("main"),
                &Outcome::Rebased {
                    commits: 3,
                    skipped: 1,
                    submodules: 0,
                    resolved: 2,
                }
            ),
            "Rebased main onto 1a2b3c4d: 3 commits (1 already there, dropped); 2 submodule conflicts resolved"
        );
        assert_eq!(
            rebase.summary(
                Some("main"),
                &Outcome::Conflicts {
                    files: 2,
                    step: Some((2, 5)),
                    unresolved: vec![SubmoduleConflict {
                        path: "api".to_owned(),
                        why: "not initialized".to_owned(),
                    }],
                }
            ),
            "Rebasing main onto 1a2b3c4d stopped at commit 2 of 5 with conflicts in 2 files: resolve and stage them, then continue; submodule api left in conflict: not initialized"
        );
    }

    #[test]
    fn nothing_starts_while_something_is_in_progress_or_the_tree_is_dirty() {
        let mut t = repo();
        diverged(&mut t, false);
        fs::write(t.path().join("f.txt"), "edited\n").unwrap();
        let err = super::super::merge::merge(
            &t.repo,
            &side(),
            &crate::git::hooks::tests::hooks(),
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                OperationError::Checkout(CheckoutError::Dirty {
                    unstaged: true,
                    staged: false
                })
            ),
            "{err:?}"
        );
        sync(&t);
        fs::write(t.repo.path().join("CHERRY_PICK_HEAD"), "x\n").unwrap();
        let err = super::super::rebase::rebase(
            &t.repo,
            &side(),
            &crate::git::hooks::tests::hooks(),
            &mut |_, _| {},
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "a cherry-pick is in progress; finish it with git first"
        );
    }
}
