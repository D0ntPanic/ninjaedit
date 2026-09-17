//! Checking out a commit from the log: moving HEAD, and the working
//! tree with it, to a commit the user picked.
//!
//! Where HEAD ends up depends on what points at the commit, so that
//! commits made afterwards go where they naturally should:
//!
//! * A local branch pointing at the commit is checked out, as
//!   `git checkout <branch>` does; HEAD's own branch is preferred when
//!   it is one of several. Commits made from there go on the branch.
//! * With no local branch there but a remote's branch (`origin/feature`)
//!   that a local branch tracks from behind (the local branch's commits
//!   are all in the remote's, as after a fetch), the local branch is
//!   fast-forwarded to the commit and checked out. A local branch that
//!   is ahead of the remote's, or has diverged from it, isn't moved:
//!   the checkout is refused and says so, since bringing the two
//!   together needs a merge, which isn't done here yet.
//! * With no local branch tracking the remote's, a local branch of the
//!   same name is made at the commit, tracking the remote's, and
//!   checked out, as `git checkout feature` does when only
//!   `origin/feature` exists. If a local branch by that name already
//!   exists (pointing elsewhere, tracking something else or nothing),
//!   the caller is asked for another name ([`CheckoutPlan::NeedsName`]).
//! * With nothing pointing at the commit, HEAD is detached there.
//!
//! [`Checkout::plan`] says which of these a commit calls for, and
//! [`Checkout::run`] does it. Both refuse when the working tree has
//! changes to tracked files, staged or not, since the checkout could
//! lose them: the user is to commit or stash first. Untracked files
//! are left alone and don't get in the way.
//!
//! The commit's submodules follow, as `git checkout --recurse-submodules`
//! has them: each initialized submodule is brought to the commit the
//! checked-out commit points it at, and its own submodules likewise.
//! A submodule whose commit a local branch of its own points at is put
//! on that branch (its current branch when that is one of them),
//! rather than left with HEAD detached; with no branch there, HEAD is
//! detached. A submodule already at its commit is left as it is, HEAD
//! and all, so nothing in it is written; its own submodules are still
//! seen to. The submodules are checked before anything moves: one
//! with changes of its own, or without the commit (not fetched yet),
//! refuses the whole checkout and is named. A submodule that isn't
//! initialized is left alone, as git leaves it.
//!
//! On a large repository a checkout takes a while, so a frontend runs
//! it in the background as a [`CheckoutJob`], which plans and then
//! runs the checkout on a thread of its own, reporting how many files
//! it has written as it goes, and ends with a [`CheckoutOutcome`]: what
//! was done, a name wanted for a new branch, or why it couldn't be
//! done.

use super::history::short_id;
use git2::build::CheckoutBuilder;
use git2::{
    Branch, BranchType, ErrorCode, ObjectType, Oid, Repository, Status, StatusOptions,
    TreeWalkMode, TreeWalkResult,
};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

/// Where HEAD is to go when a commit is checked out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Checkout {
    /// On to a local branch that points at the commit.
    Branch(String),
    /// On to a local branch that tracks a remote's branch (`upstream`,
    /// as `origin/feature`) pointing at the commit, once the local
    /// branch is fast-forwarded to it.
    FastForward { name: String, upstream: String },
    /// On to a new local branch made at the commit, tracking a remote's
    /// branch (`upstream`, as `origin/feature`) that points at it.
    NewBranch { name: String, upstream: String },
    /// Detached at the commit: no branch points at it.
    Detached,
}

/// What checking out a commit calls for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckoutPlan {
    Ready(Checkout),
    /// A remote's branch (`upstream`) points at the commit, but the
    /// local branch that would track it is named for one that exists
    /// already (`taken`), pointing elsewhere: a name for the new
    /// branch is wanted, to make a [`Checkout::NewBranch`] with.
    NeedsName {
        upstream: String,
        taken: String,
    },
}

/// Why a commit couldn't be checked out.
#[derive(Debug)]
pub enum CheckoutError {
    /// The working tree has changes to tracked files that the checkout
    /// could lose.
    Dirty {
        unstaged: bool,
        staged: bool,
    },
    /// The name given for a new branch isn't one git accepts.
    InvalidName(String),
    /// A local branch by the name given for a new branch exists.
    NameTaken(String),
    /// The local branch tracking the remote's branch at the commit has
    /// commits of its own: `ahead` of them, and `behind` the remote's.
    /// Bringing them together is a merge, which isn't done here.
    NotFastForward {
        name: String,
        upstream: String,
        ahead: usize,
        behind: usize,
    },
    /// A submodule (at `path` from the working directory, nested ones
    /// joined with `/`) can't follow the commit, for the reason given.
    Submodule {
        path: String,
        why: Box<CheckoutError>,
    },
    /// A submodule doesn't have the commit it is to go to.
    MissingCommit(Oid),
    Git(git2::Error),
}

impl From<git2::Error> for CheckoutError {
    fn from(err: git2::Error) -> CheckoutError {
        CheckoutError::Git(err)
    }
}

impl fmt::Display for CheckoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CheckoutError::Dirty { unstaged, staged } => {
                let what = match (unstaged, staged) {
                    (true, false) => "unstaged changes",
                    (false, true) => "staged changes",
                    _ => "uncommitted changes",
                };
                write!(f, "{what} would be lost; commit or stash them first")
            }
            CheckoutError::InvalidName(name) => write!(f, "{name:?} is not a valid branch name"),
            CheckoutError::NameTaken(name) => write!(f, "branch {name} already exists"),
            CheckoutError::NotFastForward {
                name,
                upstream,
                ahead,
                behind,
            } => {
                let commits = |n: &usize| if *n == 1 { "commit" } else { "commits" };
                if *behind == 0 {
                    write!(
                        f,
                        "{name} is ahead of {upstream} by {ahead} {}; push or reset it first",
                        commits(ahead)
                    )
                } else {
                    write!(
                        f,
                        "{name} and {upstream} have diverged ({ahead} and {behind} {} apart); \
                         merging them isn't supported yet",
                        commits(&(ahead + behind))
                    )
                }
            }
            CheckoutError::Submodule { path, why } => write!(f, "in submodule {path}: {why}"),
            CheckoutError::MissingCommit(id) => {
                write!(f, "commit {} isn't here; fetch it first", short_id(*id))
            }
            CheckoutError::Git(err) => f.write_str(err.message()),
        }
    }
}

impl std::error::Error for CheckoutError {}

impl Checkout {
    /// What checking out `id` calls for (see the [module](self)
    /// documentation), or why it can't be done now.
    pub fn plan(repo: &Repository, id: Oid) -> Result<CheckoutPlan, CheckoutError> {
        ensure_clean(repo)?;
        check_submodules(repo, &repo.find_commit(id)?)?;

        if let Some(name) = local_branch_at(repo, id)? {
            return Ok(CheckoutPlan::Ready(Checkout::Branch(name)));
        }

        let mut remote = branches_at(repo, BranchType::Remote, id)?;
        remote.sort();
        let named: Vec<(String, String)> = remote
            .into_iter()
            .map(|full| {
                let name = branch_part(repo, &full);
                (full, name)
            })
            .collect();
        // A local branch tracking one of the remote's branches is
        // brought up to it, if that is a fast-forward; if not, nothing
        // else is tried, since that branch is the one the user means.
        for (upstream, name) in &named {
            if let Some(local) = tracking_branch(repo, upstream, name)? {
                let target = local.get().target().ok_or_else(|| {
                    git2::Error::from_str("the tracking branch points at nothing")
                })?;
                let (ahead, behind) = repo.graph_ahead_behind(target, id)?;
                let name = local.name()?.unwrap_or(name).to_owned();
                if ahead > 0 {
                    return Err(CheckoutError::NotFastForward {
                        name,
                        upstream: upstream.clone(),
                        ahead,
                        behind,
                    });
                }
                return Ok(CheckoutPlan::Ready(Checkout::FastForward {
                    name,
                    upstream: upstream.clone(),
                }));
            }
        }
        // A remote's branch whose name is free makes a local branch of
        // that name; only when every one is taken is a name asked for.
        for (upstream, name) in &named {
            if !branch_exists(repo, name)? {
                return Ok(CheckoutPlan::Ready(Checkout::NewBranch {
                    name: name.clone(),
                    upstream: upstream.clone(),
                }));
            }
        }
        if let Some((upstream, taken)) = named.into_iter().next() {
            return Ok(CheckoutPlan::NeedsName { upstream, taken });
        }
        Ok(CheckoutPlan::Ready(Checkout::Detached))
    }

    /// Check out `id` this way: the working tree and index are brought
    /// to the commit, HEAD moved, and the submodules brought along.
    /// Returns how many submodules were, nested ones included. A new
    /// branch's name is checked first, so that a bad one changes
    /// nothing; so is that a fast-forward still is one, and that the
    /// submodules can follow.
    pub fn run(&self, repo: &Repository, id: Oid) -> Result<usize, CheckoutError> {
        self.run_with(repo, id, &mut |_, _| {})
    }

    /// [`run`](Self::run), telling `progress` how many files of the
    /// working tree have been written and how many there are to write,
    /// as libgit2 reports it, for the repository and then for each
    /// submodule in turn.
    pub fn run_with(
        &self,
        repo: &Repository,
        id: Oid,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<usize, CheckoutError> {
        ensure_clean(repo)?;
        let commit = repo.find_commit(id)?;
        check_submodules(repo, &commit)?;
        match self {
            Checkout::Branch(name) => {
                repo.find_branch(name, BranchType::Local)?;
                checkout_tree(repo, &commit, progress)?;
                repo.set_head(&format!("refs/heads/{name}"))?;
            }
            Checkout::FastForward { name, upstream } => {
                let mut branch = repo.find_branch(name, BranchType::Local)?;
                let target = branch.get().target().ok_or_else(|| {
                    git2::Error::from_str("the tracking branch points at nothing")
                })?;
                let (ahead, behind) = repo.graph_ahead_behind(target, id)?;
                if ahead > 0 {
                    return Err(CheckoutError::NotFastForward {
                        name: name.clone(),
                        upstream: upstream.clone(),
                        ahead,
                        behind,
                    });
                }
                checkout_tree(repo, &commit, progress)?;
                branch
                    .get_mut()
                    .set_target(id, &format!("fast-forward to {upstream}"))?;
                repo.set_head(&format!("refs/heads/{name}"))?;
            }
            Checkout::NewBranch { name, upstream } => {
                if !Branch::name_is_valid(name)? {
                    return Err(CheckoutError::InvalidName(name.clone()));
                }
                if branch_exists(repo, name)? {
                    return Err(CheckoutError::NameTaken(name.clone()));
                }
                repo.find_branch(upstream, BranchType::Remote)?;
                checkout_tree(repo, &commit, progress)?;
                let mut branch = repo.branch(name, &commit, false)?;
                branch.set_upstream(Some(upstream))?;
                repo.set_head(&format!("refs/heads/{name}"))?;
            }
            Checkout::Detached => {
                checkout_tree(repo, &commit, progress)?;
                repo.set_head_detached(id)?;
            }
        }
        update_submodules(repo, &commit, progress)
    }

    /// What was done, for the status bar, with how many submodules
    /// came along (see [`run`](Self::run)).
    pub fn summary(&self, id: Oid, submodules: usize) -> String {
        let what = match self {
            Checkout::Branch(name) => format!("Checked out {name}"),
            Checkout::FastForward { name, upstream } => {
                format!("Fast-forwarded {name} to {upstream} and checked it out")
            }
            Checkout::NewBranch { name, upstream } => {
                format!("Checked out new branch {name} tracking {upstream}")
            }
            Checkout::Detached => format!("HEAD detached at {}", short_id(id)),
        };
        match submodules {
            0 => what,
            1 => format!("{what}; 1 submodule updated"),
            n => format!("{what}; {n} submodules updated"),
        }
    }
}

/// Bring the working tree and index to a commit, refusing to overwrite
/// anything that differs (which [`ensure_clean`] has already ruled
/// out for tracked files), and telling `progress` how it is going.
fn checkout_tree(
    repo: &Repository,
    commit: &git2::Commit<'_>,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<(), git2::Error> {
    let mut options = CheckoutBuilder::new();
    options
        .safe()
        .progress(|_, completed, total| progress(completed, total));
    repo.checkout_tree(commit.as_object(), Some(&mut options))
}

/// The local branch to check out at a commit, if any points at it:
/// HEAD's own when it does, otherwise the first by name.
fn local_branch_at(repo: &Repository, id: Oid) -> Result<Option<String>, git2::Error> {
    let mut local = branches_at(repo, BranchType::Local, id)?;
    local.sort();
    if let Some(head) = head_branch(repo)
        && local.contains(&head)
    {
        return Ok(Some(head));
    }
    Ok(local.into_iter().next())
}

/// The submodules a commit has, as its tree records them: each one's
/// path from the working directory and the commit it points at, in
/// tree order.
fn gitlinks(commit: &git2::Commit<'_>) -> Result<Vec<(String, Oid)>, git2::Error> {
    let mut found = Vec::new();
    commit.tree()?.walk(TreeWalkMode::PreOrder, |root, entry| {
        if entry.kind() == Some(ObjectType::Commit) {
            let name = entry.name().unwrap_or("");
            found.push((format!("{root}{name}"), entry.id()));
        }
        TreeWalkResult::Ok
    })?;
    Ok(found)
}

/// A submodule's repository, at `path` from the working directory, if
/// the submodule is initialized: its directory holds a repository.
fn open_submodule(repo: &Repository, path: &str) -> Option<Repository> {
    let workdir = repo.workdir()?;
    Repository::open(workdir.join(path)).ok()
}

/// Name the submodule in whatever goes wrong inside it.
fn in_submodule<T>(
    path: &str,
    inner: impl FnOnce() -> Result<T, CheckoutError>,
) -> Result<T, CheckoutError> {
    inner().map_err(|why| CheckoutError::Submodule {
        path: path.to_owned(),
        why: Box::new(why),
    })
}

/// See that every initialized submodule of a commit, nested ones
/// included, can follow it: none has changes it could lose, and each
/// has the commit it is to go to.
fn check_submodules(repo: &Repository, commit: &git2::Commit<'_>) -> Result<(), CheckoutError> {
    for (path, id) in gitlinks(commit)? {
        let Some(sub) = open_submodule(repo, &path) else {
            continue;
        };
        in_submodule(&path, || {
            ensure_clean(&sub)?;
            let target = sub
                .find_commit(id)
                .map_err(|_| CheckoutError::MissingCommit(id))?;
            check_submodules(&sub, &target)
        })?;
    }
    Ok(())
}

/// Bring every initialized submodule of a commit to the commit it
/// points it at, on a local branch of its own that points there if
/// there is one, and its own submodules likewise. One already at its
/// commit is left as it is. Returns how many were moved, nested ones
/// included.
fn update_submodules(
    repo: &Repository,
    commit: &git2::Commit<'_>,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<usize, CheckoutError> {
    let mut updated = 0;
    for (path, id) in gitlinks(commit)? {
        let Some(sub) = open_submodule(repo, &path) else {
            continue;
        };
        updated += in_submodule(&path, || {
            if head_is_at(&sub, id) {
                // Already there: its index and working tree are left
                // untouched (a checkout would rewrite the index even
                // with nothing to do), but its own submodules may
                // still be behind.
                return update_submodules(&sub, &sub.find_commit(id)?, progress);
            }
            let how = match local_branch_at(&sub, id)? {
                Some(name) => Checkout::Branch(name),
                None => Checkout::Detached,
            };
            Ok(how.run_with(&sub, id, progress)? + 1)
        })?;
    }
    Ok(updated)
}

/// Whether a repository's HEAD is at `id`: on a branch there, or
/// detached there. An unborn HEAD is not.
fn head_is_at(repo: &Repository, id: Oid) -> bool {
    repo.head().ok().and_then(|head| head.target()) == Some(id)
}

/// How a [`CheckoutJob`] ended.
#[derive(Debug)]
pub enum CheckoutOutcome {
    /// The commit was checked out this way, with this many submodules
    /// brought along (see [`Checkout::run`]).
    Done {
        how: Checkout,
        submodules: usize,
    },
    /// A name is wanted for the new branch (see
    /// [`CheckoutPlan::NeedsName`]); nothing was done. A job started
    /// with the name given ([`CheckoutJob::start_with`]) ends with the
    /// checkout instead.
    NeedsName {
        upstream: String,
        taken: String,
    },
    Failed(CheckoutError),
}

enum Message {
    /// Files written so far, and files to write.
    Progress(usize, usize),
    Outcome(CheckoutOutcome),
}

/// A checkout under way in the background. [`poll`](Self::poll) takes
/// in its progress; once it is done the outcome says how it went.
pub struct CheckoutJob {
    receiver: Receiver<Message>,
    progress: Option<(usize, usize)>,
    done: Option<CheckoutOutcome>,
}

impl CheckoutJob {
    /// Start checking out `id` in the repository whose git directory is
    /// `git_dir` (see [`Repository::path`]), however [`Checkout::plan`]
    /// says to; a plan wanting a name ends the job with
    /// [`CheckoutOutcome::NeedsName`].
    pub fn start(git_dir: &Path, id: Oid) -> Result<CheckoutJob, CheckoutError> {
        CheckoutJob::spawn(git_dir.to_path_buf(), id, None)
    }

    /// Start checking out `id` a given way: the plan, or a
    /// [`Checkout::NewBranch`] with the name the user gave.
    pub fn start_with(
        git_dir: &Path,
        id: Oid,
        how: Checkout,
    ) -> Result<CheckoutJob, CheckoutError> {
        CheckoutJob::spawn(git_dir.to_path_buf(), id, Some(how))
    }

    fn spawn(
        git_dir: PathBuf,
        id: Oid,
        how: Option<Checkout>,
    ) -> Result<CheckoutJob, CheckoutError> {
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("git-checkout".to_owned())
            .spawn(move || {
                let outcome = (|| {
                    let repo = match Repository::open(&git_dir) {
                        Ok(repo) => repo,
                        Err(err) => return CheckoutOutcome::Failed(err.into()),
                    };
                    let how = match how {
                        Some(how) => how,
                        None => match Checkout::plan(&repo, id) {
                            Ok(CheckoutPlan::Ready(how)) => how,
                            Ok(CheckoutPlan::NeedsName { upstream, taken }) => {
                                return CheckoutOutcome::NeedsName { upstream, taken };
                            }
                            Err(err) => return CheckoutOutcome::Failed(err),
                        },
                    };
                    let mut progress = |completed, total| {
                        let _ = sender.send(Message::Progress(completed, total));
                    };
                    match how.run_with(&repo, id, &mut progress) {
                        Ok(submodules) => CheckoutOutcome::Done { how, submodules },
                        Err(err) => CheckoutOutcome::Failed(err),
                    }
                })();
                let _ = sender.send(Message::Outcome(outcome));
            })
            .map_err(|err| CheckoutError::Git(git2::Error::from_str(&err.to_string())))?;
        Ok(CheckoutJob {
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
                        self.done = Some(CheckoutOutcome::Failed(CheckoutError::Git(
                            git2::Error::from_str("the checkout stopped"),
                        )));
                        changed = true;
                    }
                    break;
                }
            }
        }
        changed
    }

    /// How many files have been written, and how many there are to
    /// write, once the checkout has got that far.
    pub fn progress(&self) -> Option<(usize, usize)> {
        self.progress
    }

    /// Whether the job is finished (as of the last poll).
    pub fn is_done(&self) -> bool {
        self.done.is_some()
    }

    /// The outcome, once [`is_done`](Self::is_done).
    pub fn outcome(self) -> Option<CheckoutOutcome> {
        self.done
    }
}

/// The branch HEAD is on, unless detached or unborn.
fn head_branch(repo: &Repository) -> Option<String> {
    if repo.head_detached().unwrap_or(false) {
        return None;
    }
    repo.head().ok()?.shorthand().ok().map(str::to_owned)
}

/// The names of the branches of a kind that point at `id`: `main`, or
/// `origin/main` for a remote's.
fn branches_at(repo: &Repository, kind: BranchType, id: Oid) -> Result<Vec<String>, git2::Error> {
    let mut names = Vec::new();
    for entry in repo.branches(Some(kind))? {
        let (branch, _) = entry?;
        // A remote's HEAD is symbolic: not a branch of its own.
        if branch.get().target() != Some(id) {
            continue;
        }
        if let Some(name) = branch.name()? {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

/// The branch's own part of a remote branch's name: `feature` of
/// `origin/feature`, whatever the remote is called (a remote's name may
/// itself have slashes in it).
fn branch_part(repo: &Repository, full: &str) -> String {
    let remote = repo
        .branch_remote_name(&format!("refs/remotes/{full}"))
        .ok()
        .and_then(|buf| buf.as_str().ok().map(str::to_owned));
    match remote {
        Some(remote) if full.len() > remote.len() + 1 && full.starts_with(&remote) => {
            full[remote.len() + 1..].to_owned()
        }
        _ => full
            .split_once('/')
            .map_or(full, |(_, name)| name)
            .to_owned(),
    }
}

/// The local branch tracking a remote's branch (`upstream`, as
/// `origin/feature`), if any: the one named as the remote's branch is
/// (`name`) when it tracks it, otherwise the first by name that does.
fn tracking_branch<'r>(
    repo: &'r Repository,
    upstream: &str,
    name: &str,
) -> Result<Option<Branch<'r>>, git2::Error> {
    let tracks = |branch: &Branch<'_>| -> bool {
        branch
            .upstream()
            .ok()
            .and_then(|up| up.name().ok().flatten().map(str::to_owned))
            .is_some_and(|up| up == upstream)
    };
    if let Ok(branch) = repo.find_branch(name, BranchType::Local)
        && tracks(&branch)
    {
        return Ok(Some(branch));
    }
    let mut found: Option<(String, Branch<'r>)> = None;
    for entry in repo.branches(Some(BranchType::Local))? {
        let (branch, _) = entry?;
        if !tracks(&branch) {
            continue;
        }
        let branch_name = branch.name()?.unwrap_or("").to_owned();
        if found.as_ref().is_none_or(|(name, _)| branch_name < *name) {
            found = Some((branch_name, branch));
        }
    }
    Ok(found.map(|(_, branch)| branch))
}

fn branch_exists(repo: &Repository, name: &str) -> Result<bool, git2::Error> {
    match repo.find_branch(name, BranchType::Local) {
        Ok(_) => Ok(true),
        Err(err) if err.code() == ErrorCode::NotFound => Ok(false),
        // An invalid name isn't a branch that exists; `run` says so.
        Err(err) if err.code() == ErrorCode::InvalidSpec => Ok(false),
        Err(err) => Err(err),
    }
}

/// Refuse while tracked files have changes, staged or not, that a
/// checkout could lose.
fn ensure_clean(repo: &Repository) -> Result<(), CheckoutError> {
    let mut options = StatusOptions::new();
    options
        .include_untracked(false)
        .include_ignored(false)
        .exclude_submodules(true);
    let statuses = repo.statuses(Some(&mut options))?;
    let mut unstaged = false;
    let mut staged = false;
    for entry in statuses.iter() {
        let status = entry.status();
        if status.intersects(
            Status::WT_MODIFIED
                | Status::WT_DELETED
                | Status::WT_TYPECHANGE
                | Status::WT_RENAMED
                | Status::CONFLICTED,
        ) {
            unstaged = true;
        }
        if status.intersects(
            Status::INDEX_NEW
                | Status::INDEX_MODIFIED
                | Status::INDEX_DELETED
                | Status::INDEX_RENAMED
                | Status::INDEX_TYPECHANGE,
        ) {
            staged = true;
        }
    }
    if unstaged || staged {
        return Err(CheckoutError::Dirty { unstaged, staged });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::history::tests::TestRepo;
    use std::fs;
    use std::path::Path;

    fn head_of(repo: &Repository) -> (Option<String>, Oid, bool) {
        let head = repo.head().unwrap();
        let detached = repo.head_detached().unwrap();
        let name = if detached {
            None
        } else {
            head.shorthand().ok().map(str::to_owned)
        };
        (name, head.target().unwrap(), detached)
    }

    fn checkout(repo: &Repository, id: Oid) -> Result<Checkout, CheckoutError> {
        match Checkout::plan(repo, id)? {
            CheckoutPlan::Ready(how) => {
                how.run(repo, id)?;
                Ok(how)
            }
            CheckoutPlan::NeedsName { upstream, taken } => {
                panic!("a name was asked for: {upstream} vs {taken}")
            }
        }
    }

    #[test]
    fn a_commit_on_a_branch_checks_out_the_branch_and_its_files() {
        let mut t = TestRepo::new();
        let a = t.commit(&[("f.txt", "one\n")], "One", &[]);
        let b = t.commit(&[("f.txt", "two\n")], "Two", &[a]);
        t.branch("side", a);
        let main = head_of(&t.repo).0.unwrap();

        let how = checkout(&t.repo, a).unwrap();
        assert_eq!(how, Checkout::Branch("side".to_owned()));
        assert_eq!(how.summary(a, 0), "Checked out side");
        assert_eq!(head_of(&t.repo), (Some("side".to_owned()), a, false));
        assert_eq!(fs::read_to_string(t.path().join("f.txt")).unwrap(), "one\n");

        // Back to main, and its files.
        let how = checkout(&t.repo, b).unwrap();
        assert_eq!(how, Checkout::Branch(main.clone()));
        assert_eq!(head_of(&t.repo), (Some(main), b, false));
        assert_eq!(fs::read_to_string(t.path().join("f.txt")).unwrap(), "two\n");
    }

    #[test]
    fn heads_own_branch_wins_among_several_at_a_commit() {
        let mut t = TestRepo::new();
        let a = t.commit(&[("f.txt", "one\n")], "One", &[]);
        let main = head_of(&t.repo).0.unwrap();
        // Sorted before main, but main is HEAD's.
        t.branch("aaa", a);
        assert_eq!(
            Checkout::plan(&t.repo, a).unwrap(),
            CheckoutPlan::Ready(Checkout::Branch(main))
        );
        // Detached elsewhere, the first by name is taken.
        let b = t.commit(&[("f.txt", "two\n")], "Two", &[a]);
        t.repo.set_head_detached(b).unwrap();
        assert_eq!(
            Checkout::plan(&t.repo, a).unwrap(),
            CheckoutPlan::Ready(Checkout::Branch("aaa".to_owned()))
        );
    }

    #[test]
    fn a_commit_on_no_branch_detaches_head() {
        let mut t = TestRepo::new();
        let a = t.commit(&[("f.txt", "one\n")], "One", &[]);
        let b = t.commit(&[("f.txt", "two\n")], "Two", &[a]);
        let c = t.commit(&[("f.txt", "three\n")], "Three", &[b]);
        let how = checkout(&t.repo, a).unwrap();
        assert_eq!(how, Checkout::Detached);
        assert_eq!(
            how.summary(a, 0),
            format!("HEAD detached at {}", short_id(a))
        );
        assert_eq!(head_of(&t.repo), (None, a, true));
        assert_eq!(fs::read_to_string(t.path().join("f.txt")).unwrap(), "one\n");
        // A tag doesn't count as a branch.
        t.tag("v1", b);
        let how = checkout(&t.repo, b).unwrap();
        assert_eq!(how, Checkout::Detached);
        assert_eq!(head_of(&t.repo), (None, b, true));
        // The branch's own commit puts HEAD back on the branch.
        let how = checkout(&t.repo, c).unwrap();
        assert!(matches!(how, Checkout::Branch(_)));
        assert!(!head_of(&t.repo).2);
        assert_eq!(
            fs::read_to_string(t.path().join("f.txt")).unwrap(),
            "three\n"
        );
    }

    #[test]
    fn a_remote_branch_gets_a_local_branch_tracking_it() {
        let mut t = TestRepo::new();
        let a = t.commit(&[("f.txt", "one\n")], "One", &[]);
        let b = t.commit(&[("f.txt", "two\n")], "Two", &[a]);
        t.remote_branch("origin", "feature", a);
        // The remote's HEAD is symbolic and doesn't count.
        t.repo
            .reference_symbolic(
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/feature",
                false,
                "t",
            )
            .unwrap();

        let how = checkout(&t.repo, a).unwrap();
        assert_eq!(
            how,
            Checkout::NewBranch {
                name: "feature".to_owned(),
                upstream: "origin/feature".to_owned(),
            }
        );
        assert_eq!(
            how.summary(a, 0),
            "Checked out new branch feature tracking origin/feature"
        );
        assert_eq!(head_of(&t.repo), (Some("feature".to_owned()), a, false));
        let branch = t.repo.find_branch("feature", BranchType::Local).unwrap();
        let upstream = branch.upstream().unwrap();
        assert_eq!(upstream.name().unwrap(), Some("origin/feature"));
        let config = t.repo.config().unwrap();
        assert_eq!(
            config.get_string("branch.feature.remote").unwrap(),
            "origin"
        );
        assert_eq!(
            config.get_string("branch.feature.merge").unwrap(),
            "refs/heads/feature"
        );

        // Now that the local branch exists, it is what gets checked out.
        checkout(&t.repo, b).unwrap();
        assert_eq!(
            Checkout::plan(&t.repo, a).unwrap(),
            CheckoutPlan::Ready(Checkout::Branch("feature".to_owned()))
        );
    }

    #[test]
    fn a_remote_branch_whose_name_is_taken_asks_for_one() {
        let mut t = TestRepo::new();
        let a = t.commit(&[("f.txt", "one\n")], "One", &[]);
        let b = t.commit(&[("f.txt", "two\n")], "Two", &[a]);
        t.branch("feature", b);
        t.remote_branch("origin", "feature", a);
        assert_eq!(
            Checkout::plan(&t.repo, a).unwrap(),
            CheckoutPlan::NeedsName {
                upstream: "origin/feature".to_owned(),
                taken: "feature".to_owned(),
            }
        );
        // Another remote's branch of a free name is taken instead.
        t.remote_branch("fork", "topic", a);
        assert_eq!(
            Checkout::plan(&t.repo, a).unwrap(),
            CheckoutPlan::Ready(Checkout::NewBranch {
                name: "topic".to_owned(),
                upstream: "fork/topic".to_owned(),
            })
        );

        // The name given is checked before anything changes.
        let with = |name: &str| Checkout::NewBranch {
            name: name.to_owned(),
            upstream: "origin/feature".to_owned(),
        };
        let err = with("feature").run(&t.repo, a).unwrap_err();
        assert!(matches!(err, CheckoutError::NameTaken(name) if name == "feature"));
        let err = with("bad name").run(&t.repo, a).unwrap_err();
        assert!(matches!(&err, CheckoutError::InvalidName(name) if name == "bad name"));
        assert_eq!(err.to_string(), "\"bad name\" is not a valid branch name");
        let err = with("").run(&t.repo, a).unwrap_err();
        assert!(matches!(err, CheckoutError::InvalidName(_)));
        assert_eq!(head_of(&t.repo).1, b);
        assert_eq!(fs::read_to_string(t.path().join("f.txt")).unwrap(), "two\n");

        with("feature-upstream").run(&t.repo, a).unwrap();
        assert_eq!(
            head_of(&t.repo),
            (Some("feature-upstream".to_owned()), a, false)
        );
        let branch = t
            .repo
            .find_branch("feature-upstream", BranchType::Local)
            .unwrap();
        assert_eq!(
            branch.upstream().unwrap().name().unwrap(),
            Some("origin/feature")
        );
        assert_eq!(fs::read_to_string(t.path().join("f.txt")).unwrap(), "one\n");
    }

    #[test]
    fn a_tracking_branch_behind_the_remote_is_fast_forwarded() {
        let mut t = TestRepo::new();
        let a = t.commit(&[("f.txt", "one\n")], "One", &[]);
        let b = t.commit(&[("f.txt", "two\n")], "Two", &[a]);
        let c = t.commit(&[("f.txt", "three\n")], "Three", &[b]);
        let main = head_of(&t.repo).0.unwrap();
        // feature tracks origin/feature from one commit behind, under
        // a name of its own: the remote's branch is called topic.
        t.branch("feature", b);
        t.remote_branch("origin", "topic", c);
        t.repo
            .find_branch("feature", BranchType::Local)
            .unwrap()
            .set_upstream(Some("origin/topic"))
            .unwrap();
        // A branch merely named topic, tracking nothing, doesn't count.
        t.branch("topic", a);
        // HEAD elsewhere, with no local branch at the commit: the
        // tracking branch is moved and checked out.
        t.repo
            .reference(&format!("refs/heads/{main}"), a, true, "t")
            .unwrap();
        t.repo
            .checkout_tree(
                t.repo.find_commit(a).unwrap().as_object(),
                Some(CheckoutBuilder::new().force()),
            )
            .unwrap();
        t.repo.set_head("refs/heads/topic").unwrap();

        let plan = Checkout::plan(&t.repo, c).unwrap();
        assert_eq!(
            plan,
            CheckoutPlan::Ready(Checkout::FastForward {
                name: "feature".to_owned(),
                upstream: "origin/topic".to_owned(),
            })
        );
        let how = checkout(&t.repo, c).unwrap();
        assert_eq!(
            how.summary(c, 0),
            "Fast-forwarded feature to origin/topic and checked it out"
        );
        assert_eq!(head_of(&t.repo), (Some("feature".to_owned()), c, false));
        assert_eq!(
            t.repo
                .find_branch("feature", BranchType::Local)
                .unwrap()
                .get()
                .target(),
            Some(c)
        );
        assert_eq!(
            fs::read_to_string(t.path().join("f.txt")).unwrap(),
            "three\n"
        );
        // topic stayed where it was.
        assert_eq!(
            t.repo
                .find_branch("topic", BranchType::Local)
                .unwrap()
                .get()
                .target(),
            Some(a)
        );

        // From the branch itself, behind again after a "fetch".
        let d = t.commit(&[("f.txt", "four\n")], "Four", &[c]);
        t.repo
            .reference("refs/heads/feature", c, true, "t")
            .unwrap();
        t.repo
            .reference("refs/remotes/origin/topic", d, true, "t")
            .unwrap();
        t.repo
            .checkout_tree(
                t.repo.find_commit(c).unwrap().as_object(),
                Some(CheckoutBuilder::new().force()),
            )
            .unwrap();
        assert_eq!(head_of(&t.repo), (Some("feature".to_owned()), c, false));
        let how = checkout(&t.repo, d).unwrap();
        assert!(matches!(how, Checkout::FastForward { .. }));
        assert_eq!(head_of(&t.repo), (Some("feature".to_owned()), d, false));
        assert_eq!(
            fs::read_to_string(t.path().join("f.txt")).unwrap(),
            "four\n"
        );
    }

    #[test]
    fn a_tracking_branch_ahead_or_diverged_refuses_the_checkout() {
        let mut t = TestRepo::new();
        let a = t.commit(&[("f.txt", "one\n")], "One", &[]);
        let b = t.commit(&[("f.txt", "two\n")], "Two", &[a]);
        let c = t.commit(&[("f.txt", "three\n")], "Three", &[b]);
        let main = head_of(&t.repo).0.unwrap();
        t.remote_branch("origin", main.as_str(), a);
        t.repo
            .find_branch(&main, BranchType::Local)
            .unwrap()
            .set_upstream(Some(&format!("origin/{main}")))
            .unwrap();

        // Ahead: the local branch has commits the remote's hasn't.
        let err = Checkout::plan(&t.repo, a).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("{main} is ahead of origin/{main} by 2 commits; push or reset it first")
        );
        assert!(matches!(
            err,
            CheckoutError::NotFastForward {
                ahead: 2,
                behind: 0,
                ..
            }
        ));
        assert_eq!(head_of(&t.repo), (Some(main.clone()), c, false));

        // Diverged: a commit of its own on the remote's side too.
        t.repo.set_head_detached(a).unwrap();
        let r = t.commit(&[("g.txt", "remote\n")], "Remote", &[a]);
        t.repo
            .reference(&format!("refs/remotes/origin/{main}"), r, true, "t")
            .unwrap();
        t.repo.set_head(&format!("refs/heads/{main}")).unwrap();
        t.repo
            .checkout_tree(
                t.repo.find_commit(c).unwrap().as_object(),
                Some(CheckoutBuilder::new().force()),
            )
            .unwrap();
        let err = Checkout::plan(&t.repo, r).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "{main} and origin/{main} have diverged (2 and 1 commits apart); \
                 merging them isn't supported yet"
            )
        );
        // A plan made before the divergence doesn't go through either.
        let stale = Checkout::FastForward {
            name: main.clone(),
            upstream: format!("origin/{main}"),
        };
        let err = stale.run(&t.repo, r).unwrap_err();
        assert!(matches!(err, CheckoutError::NotFastForward { .. }));
        assert_eq!(head_of(&t.repo), (Some(main), c, false));
        assert_eq!(
            fs::read_to_string(t.path().join("f.txt")).unwrap(),
            "three\n"
        );
        assert!(!t.path().join("g.txt").exists());
    }

    /// Commit the parent's index as it is (a submodule's gitlink
    /// staged), on HEAD.
    fn commit_index(repo: &Repository, message: &str) -> Oid {
        let sig = git2::Signature::now("Test Author", "test@example.com").unwrap();
        let mut index = repo.index().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let head = repo.head().unwrap().target().unwrap();
        let parent = repo.find_commit(head).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])
            .unwrap()
    }

    /// Commit a file in a submodule on its HEAD.
    fn commit_in(sub: &Repository, name: &str, content: &str, message: &str) -> Oid {
        let sig = git2::Signature::now("Sub Author", "sub@example.com").unwrap();
        fs::write(sub.workdir().unwrap().join(name), content).unwrap();
        let mut index = sub.index().unwrap();
        index.add_path(Path::new(name)).unwrap();
        index.write().unwrap();
        let tree = sub.find_tree(index.write_tree().unwrap()).unwrap();
        let parents: Vec<git2::Commit<'_>> = sub
            .head()
            .ok()
            .and_then(|h| h.target())
            .map(|id| sub.find_commit(id).unwrap())
            .into_iter()
            .collect();
        let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        sub.commit(Some("HEAD"), &sig, &sig, message, &tree, &refs)
            .unwrap()
    }

    /// Stage the submodule at `path` as it now is in the parent.
    fn stage_submodule(repo: &Repository, path: &str) {
        repo.find_submodule(path)
            .unwrap()
            .add_to_index(true)
            .unwrap();
    }

    #[test]
    fn submodules_follow_the_commit_on_to_their_branches_or_detached() {
        let mut t = TestRepo::new();
        t.commit(&[("f.txt", "one\n")], "One", &[]);
        let sub = t.add_submodule("sub");
        let s1 = sub.head().unwrap().target().unwrap();
        let p1 = t.repo.head().unwrap().target().unwrap();
        let sub_main = sub.head().unwrap().shorthand().unwrap().to_owned();
        // The submodule moves on, and the parent follows it; the
        // submodule's branch stays at its new commit.
        let s2 = commit_in(&sub, "inner.txt", "two\n", "Inner two");
        stage_submodule(&t.repo, "sub");
        let p2 = commit_index(&t.repo, "Bump sub");
        // A branch of the submodule at its old commit.
        sub.branch("old", &sub.find_commit(s1).unwrap(), false)
            .unwrap();
        let main = head_of(&t.repo).0.unwrap();

        // Back to p1: the submodule goes to s1, on its branch there.
        let updated = Checkout::Detached.run(&t.repo, p1).unwrap();
        assert_eq!(updated, 1);
        assert_eq!(head_of(&sub), (Some("old".to_owned()), s1, false));
        assert_eq!(
            fs::read_to_string(sub.workdir().unwrap().join("inner.txt")).unwrap(),
            "inner\n"
        );
        assert_eq!(
            Checkout::Detached.summary(p1, updated),
            format!("HEAD detached at {}; 1 submodule updated", short_id(p1))
        );

        // Forward to p2: the submodule's own main branch is there.
        let updated = Checkout::Branch(main.clone()).run(&t.repo, p2).unwrap();
        assert_eq!(updated, 1);
        assert_eq!(head_of(&sub), (Some(sub_main.clone()), s2, false));
        assert_eq!(
            fs::read_to_string(sub.workdir().unwrap().join("inner.txt")).unwrap(),
            "two\n"
        );
        assert_eq!(
            Checkout::Branch(main.clone()).summary(p2, 3),
            format!("Checked out {main}; 3 submodules updated")
        );

        // With no branch at the old commit, HEAD is detached there.
        sub.find_branch("old", BranchType::Local)
            .unwrap()
            .delete()
            .unwrap();
        Checkout::Detached.run(&t.repo, p1).unwrap();
        assert_eq!(head_of(&sub), (None, s1, true));
        // Its main branch is where it was.
        assert_eq!(
            sub.find_branch(&sub_main, BranchType::Local)
                .unwrap()
                .get()
                .target(),
            Some(s2)
        );
    }

    #[test]
    fn nested_submodules_follow_too() {
        let mut t = TestRepo::new();
        t.commit(&[("f.txt", "one\n")], "One", &[]);
        let sub = t.add_submodule("sub");
        let inner = crate::git::history::tests::add_submodule_to(&sub, "inner");
        let i1 = inner.head().unwrap().target().unwrap();
        stage_submodule(&t.repo, "sub");
        let p1 = commit_index(&t.repo, "Sub with inner");
        let i2 = commit_in(&inner, "deep.txt", "two\n", "Deep two");
        stage_submodule(&sub, "inner");
        commit_index(&sub, "Bump inner");
        stage_submodule(&t.repo, "sub");
        let p2 = commit_index(&t.repo, "Bump sub");
        let main = head_of(&t.repo).0.unwrap();

        let updated = Checkout::Detached.run(&t.repo, p1).unwrap();
        assert_eq!(updated, 2);
        assert_eq!(head_of(&inner), (None, i1, true));
        let updated = Checkout::Branch(main).run(&t.repo, p2).unwrap();
        assert_eq!(updated, 2);
        assert_eq!(head_of(&inner).1, i2);
        assert!(!head_of(&inner).2);
    }

    #[test]
    fn a_submodule_already_at_its_commit_is_left_alone() {
        let mut t = TestRepo::new();
        t.commit(&[("f.txt", "one\n")], "One", &[]);
        let sub = t.add_submodule("sub");
        let inner = crate::git::history::tests::add_submodule_to(&sub, "inner");
        let i1 = inner.head().unwrap().target().unwrap();
        stage_submodule(&t.repo, "sub");
        let p1 = commit_index(&t.repo, "Sub with inner");
        let s1 = sub.head().unwrap().target().unwrap();
        // A commit that changes a file but not the submodule.
        let p2 = t.commit(&[("f.txt", "two\n")], "Two", &[p1]);
        // And one that moves the nested submodule on, and so the
        // submodule, but leaves the working tree at the old commits.
        let i2 = commit_in(&inner, "deep.txt", "two\n", "Deep two");
        stage_submodule(&sub, "inner");
        let s2 = commit_index(&sub, "Bump inner");
        stage_submodule(&t.repo, "sub");
        let p3 = commit_index(&t.repo, "Bump sub");
        let main = head_of(&t.repo).0.unwrap();
        Checkout::Detached.run(&t.repo, p1).unwrap();
        assert_eq!((head_of(&sub).1, head_of(&inner).1), (s1, i1));

        // The submodule's HEAD detached at its commit, with a branch
        // there that a checkout would put it on, and its index locked
        // as a crashed process leaves it: a checkout in it would fail,
        // so leaving it alone is what makes these go through.
        sub.set_head_detached(s1).unwrap();
        let lock = sub.path().join("index.lock");
        fs::write(&lock, "").unwrap();

        let updated = Checkout::Detached.run(&t.repo, p2).unwrap();
        assert_eq!(updated, 0);
        assert_eq!(head_of(&sub), (None, s1, true));
        assert_eq!(fs::read_to_string(t.path().join("f.txt")).unwrap(), "two\n");
        let updated = Checkout::Detached.run(&t.repo, p1).unwrap();
        assert_eq!(updated, 0);
        assert_eq!(head_of(&sub), (None, s1, true));
        assert!(lock.exists());

        // A nested submodule that fell behind is still brought along,
        // though its parent is left alone.
        fs::remove_file(&lock).unwrap();
        sub.set_head_detached(s2).unwrap();
        sub.checkout_head(Some(CheckoutBuilder::new().force()))
            .unwrap();
        assert_eq!(head_of(&inner).1, i1, "the checkout doesn't recurse");
        fs::write(&lock, "").unwrap();
        let updated = Checkout::Branch(main).run(&t.repo, p3).unwrap();
        assert_eq!(updated, 1, "only the nested submodule moved");
        assert_eq!(head_of(&sub), (None, s2, true));
        assert_eq!(head_of(&inner).1, i2);
        assert!(lock.exists());
    }

    #[test]
    fn a_submodule_that_cannot_follow_refuses_the_checkout() {
        let mut t = TestRepo::new();
        t.commit(&[("f.txt", "one\n")], "One", &[]);
        let sub = t.add_submodule("sub");
        let p1 = t.repo.head().unwrap().target().unwrap();
        let s2 = commit_in(&sub, "inner.txt", "two\n", "Inner two");
        stage_submodule(&t.repo, "sub");
        let p2 = commit_index(&t.repo, "Bump sub");

        // Changes in the submodule.
        fs::write(sub.workdir().unwrap().join("inner.txt"), "edited\n").unwrap();
        let err = Checkout::plan(&t.repo, p1).unwrap_err();
        assert_eq!(
            err.to_string(),
            "in submodule sub: unstaged changes would be lost; commit or stash them first"
        );
        let err = Checkout::Detached.run(&t.repo, p1).unwrap_err();
        assert!(matches!(err, CheckoutError::Submodule { .. }));
        assert_eq!(head_of(&t.repo).1, p2);
        assert_eq!(head_of(&sub).1, s2);
        fs::write(sub.workdir().unwrap().join("inner.txt"), "two\n").unwrap();

        // A commit the submodule doesn't have: the parent points at
        // one made up.
        let fake = Oid::from_str("1234567890123456789012345678901234567890").unwrap();
        let p3 = {
            let tree = t.repo.head().unwrap().peel_to_tree().unwrap();
            let mut builder = t.repo.treebuilder(Some(&tree)).unwrap();
            builder.insert("sub", fake, 0o160000).unwrap();
            let tree = t.repo.find_tree(builder.write().unwrap()).unwrap();
            let sig = git2::Signature::now("Test Author", "test@example.com").unwrap();
            let parent = t.repo.find_commit(p2).unwrap();
            t.repo
                .commit(None, &sig, &sig, "Sub elsewhere", &tree, &[&parent])
                .unwrap()
        };
        let err = Checkout::plan(&t.repo, p3).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "in submodule sub: commit {} isn't here; fetch it first",
                short_id(fake)
            )
        );
        assert_eq!(head_of(&t.repo).1, p2);
        assert_eq!(head_of(&sub).1, s2);

        // An uninitialized submodule (its directory empty) is left
        // alone, whatever commit it is meant to be at.
        t.add_uninitialized_submodule("bare");
        let p4 = {
            // Declaring it changed .gitmodules: commit that too.
            let mut index = t.repo.index().unwrap();
            index.add_path(Path::new(".gitmodules")).unwrap();
            index.write().unwrap();
            let tree = t.repo.find_tree(index.write_tree().unwrap()).unwrap();
            let mut builder = t.repo.treebuilder(Some(&tree)).unwrap();
            builder.insert("bare", fake, 0o160000).unwrap();
            let tree = t.repo.find_tree(builder.write().unwrap()).unwrap();
            let sig = git2::Signature::now("Test Author", "test@example.com").unwrap();
            let parent = t.repo.find_commit(p2).unwrap();
            t.repo
                .commit(Some("HEAD"), &sig, &sig, "Add bare", &tree, &[&parent])
                .unwrap()
        };
        let updated = Checkout::Detached.run(&t.repo, p1).unwrap();
        assert_eq!(updated, 1);
        assert_eq!(head_of(&t.repo).1, p1);
        let updated = Checkout::Detached.run(&t.repo, p4).unwrap();
        assert_eq!(updated, 1);
        assert_eq!(head_of(&sub).1, s2);
    }

    fn finish(job: &mut CheckoutJob) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !job.is_done() && std::time::Instant::now() < deadline {
            job.poll();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(job.is_done());
    }

    #[test]
    fn a_job_checks_out_in_the_background_and_reports_progress() {
        let mut t = TestRepo::new();
        let a = t.commit(&[("f.txt", "one\n"), ("g.txt", "g\n")], "One", &[]);
        let b = t.commit(&[("f.txt", "two\n")], "Two", &[a]);
        t.remote_branch("origin", "feature", a);
        t.branch("feature", b);

        // The plan wants a name: nothing is done.
        let mut job = CheckoutJob::start(t.repo.path(), a).unwrap();
        finish(&mut job);
        assert!(matches!(
            job.outcome(),
            Some(CheckoutOutcome::NeedsName { upstream, taken })
                if upstream == "origin/feature" && taken == "feature"
        ));
        assert_eq!(head_of(&t.repo).1, b);

        // Given a name, the checkout happens, with progress along the
        // way, and what was done comes back.
        let how = Checkout::NewBranch {
            name: "mine".to_owned(),
            upstream: "origin/feature".to_owned(),
        };
        let mut job = CheckoutJob::start_with(t.repo.path(), a, how.clone()).unwrap();
        finish(&mut job);
        let (completed, total) = job.progress().expect("progress was reported");
        assert_eq!(completed, total);
        assert!(total > 0);
        assert!(matches!(
            job.outcome(),
            Some(CheckoutOutcome::Done { how: done, submodules: 0 }) if done == how
        ));
        assert_eq!(head_of(&t.repo), (Some("mine".to_owned()), a, false));

        // A name that won't do fails, and says why.
        let taken = Checkout::NewBranch {
            name: "feature".to_owned(),
            upstream: "origin/feature".to_owned(),
        };
        let mut job = CheckoutJob::start_with(t.repo.path(), b, taken).unwrap();
        finish(&mut job);
        assert!(matches!(
            job.outcome(),
            Some(CheckoutOutcome::Failed(CheckoutError::NameTaken(name))) if name == "feature"
        ));
        // A plain plan runs through to the end.
        let mut job = CheckoutJob::start(t.repo.path(), b).unwrap();
        finish(&mut job);
        assert!(matches!(
            job.outcome(),
            Some(CheckoutOutcome::Done { how: Checkout::Branch(name), .. }) if name == "feature"
        ));
        assert_eq!(head_of(&t.repo), (Some("feature".to_owned()), b, false));
    }

    #[test]
    fn uncommitted_changes_refuse_the_checkout() {
        let mut t = TestRepo::new();
        let a = t.commit(&[("f.txt", "one\n")], "One", &[]);
        let b = t.commit(&[("f.txt", "two\n"), ("g.txt", "g\n")], "Two", &[a]);

        // Unstaged.
        fs::write(t.path().join("g.txt"), "changed\n").unwrap();
        let err = Checkout::plan(&t.repo, a).unwrap_err();
        assert_eq!(
            err.to_string(),
            "unstaged changes would be lost; commit or stash them first"
        );
        let err = Checkout::Detached.run(&t.repo, a).unwrap_err();
        assert!(matches!(
            err,
            CheckoutError::Dirty {
                unstaged: true,
                staged: false
            }
        ));
        assert_eq!(head_of(&t.repo).1, b);

        // Staged.
        let mut index = t.repo.index().unwrap();
        index.add_path(Path::new("g.txt")).unwrap();
        index.write().unwrap();
        let err = Checkout::plan(&t.repo, a).unwrap_err();
        assert_eq!(
            err.to_string(),
            "staged changes would be lost; commit or stash them first"
        );
        // Both.
        fs::write(t.path().join("g.txt"), "changed again\n").unwrap();
        let err = Checkout::plan(&t.repo, a).unwrap_err();
        assert_eq!(
            err.to_string(),
            "uncommitted changes would be lost; commit or stash them first"
        );

        // An untracked file is neither lost nor in the way.
        fs::write(t.path().join("g.txt"), "g\n").unwrap();
        index.add_path(Path::new("g.txt")).unwrap();
        index.write().unwrap();
        fs::write(t.path().join("new.txt"), "new\n").unwrap();
        checkout(&t.repo, a).unwrap();
        assert_eq!(head_of(&t.repo).1, a);
        assert_eq!(
            fs::read_to_string(t.path().join("new.txt")).unwrap(),
            "new\n"
        );
        assert!(!t.path().join("g.txt").exists());
    }
}
