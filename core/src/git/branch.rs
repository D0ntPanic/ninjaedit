//! Making a new local branch where HEAD is, and moving HEAD on to it,
//! as `git checkout -b <name>` does: a reference to the commit HEAD is
//! on, and HEAD pointing at it. Neither the working tree nor the index
//! is touched, since the commit is the one checked out already; files
//! being edited, changed or not, stay as they are. A HEAD that has no
//! commit yet (a fresh repository's) is moved on to the new name,
//! unborn as it was, as git does.
//!
//! The repository is named as the status bar names one (see the
//! [`head`](super::head) module): a path inside it, or, with `exact`,
//! exactly its working directory, so that a submodule's branch is made
//! in the submodule and not in whatever contains it.
//!
//! Deleting a local branch, as `git branch -d` does, is here too: the
//! reference and its configuration (its upstream) go, and nothing else.
//! The branch HEAD is on can't be deleted. Nor, unless forced (as
//! `git branch -D`), can one with commits that neither HEAD nor the
//! branch's upstream has, since deleting it would leave them on no
//! branch; [`DeleteBranchError::Unmerged`] says how many, for the
//! caller to ask before forcing.

use super::checkout::branch_exists;
use git2::{Branch, BranchType, ErrorCode, Oid, Repository};
use std::fmt;
use std::path::Path;

/// Why a branch couldn't be made.
#[derive(Debug)]
pub enum BranchError {
    /// The name isn't one git accepts.
    InvalidName(String),
    /// A local branch by the name exists.
    NameTaken(String),
    Git(git2::Error),
}

impl From<git2::Error> for BranchError {
    fn from(err: git2::Error) -> BranchError {
        BranchError::Git(err)
    }
}

impl fmt::Display for BranchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BranchError::InvalidName(name) => write!(f, "{name:?} is not a valid branch name"),
            BranchError::NameTaken(name) => write!(f, "branch {name} already exists"),
            BranchError::Git(err) => f.write_str(err.message()),
        }
    }
}

impl std::error::Error for BranchError {}

/// Make a local branch `name` at HEAD of the repository containing
/// `path`, or, with `exact`, of the repository whose working directory
/// is `path` itself, and put HEAD on it.
pub fn create_branch(path: &Path, exact: bool, name: &str) -> Result<(), BranchError> {
    let repo = if exact {
        Repository::open(path)?
    } else {
        Repository::discover(path)?
    };
    create_branch_in(&repo, name)
}

/// Make a local branch `name` at HEAD of an opened repository and put
/// HEAD on it. The name is checked before anything is done, so a bad
/// one changes nothing.
pub fn create_branch_in(repo: &Repository, name: &str) -> Result<(), BranchError> {
    if !Branch::name_is_valid(name)? {
        return Err(BranchError::InvalidName(name.to_owned()));
    }
    if branch_exists(repo, name)? {
        return Err(BranchError::NameTaken(name.to_owned()));
    }
    match repo.head() {
        Ok(head) => {
            let commit = head.peel_to_commit()?;
            repo.branch(name, &commit, false)?;
        }
        // No commit to point the branch at: HEAD just takes the new
        // name, and the first commit will make the branch.
        Err(err) if err.code() == ErrorCode::UnbornBranch => {}
        Err(err) => return Err(err.into()),
    }
    repo.set_head(&format!("refs/heads/{name}"))?;
    Ok(())
}

/// Why a branch couldn't be deleted.
#[derive(Debug)]
pub enum DeleteBranchError {
    /// HEAD is on the branch.
    Head(String),
    /// The branch has `commits` that neither HEAD nor its upstream has,
    /// which deleting it would leave on no branch.
    Unmerged {
        name: String,
        commits: usize,
    },
    Git(git2::Error),
}

impl From<git2::Error> for DeleteBranchError {
    fn from(err: git2::Error) -> DeleteBranchError {
        DeleteBranchError::Git(err)
    }
}

impl fmt::Display for DeleteBranchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeleteBranchError::Head(name) => write!(f, "{name} is checked out"),
            DeleteBranchError::Unmerged { name, commits } => {
                let noun = if *commits == 1 { "commit" } else { "commits" };
                write!(
                    f,
                    "{name} has {commits} {noun} not merged into HEAD or its upstream"
                )
            }
            DeleteBranchError::Git(err) => f.write_str(err.message()),
        }
    }
}

impl std::error::Error for DeleteBranchError {}

/// Delete the local branch `name` of the repository whose git
/// directory is `git_dir` (see [`Repository::path`]), unless HEAD is on
/// it or, without `force`, it has commits that would be left on no
/// branch. Returns the commit it pointed at.
pub fn delete_branch(git_dir: &Path, name: &str, force: bool) -> Result<Oid, DeleteBranchError> {
    let repo = Repository::open(git_dir)?;
    delete_branch_in(&repo, name, force)
}

/// [`delete_branch`] in an opened repository.
pub fn delete_branch_in(
    repo: &Repository,
    name: &str,
    force: bool,
) -> Result<Oid, DeleteBranchError> {
    let mut branch = repo.find_branch(name, BranchType::Local)?;
    if branch.is_head() {
        return Err(DeleteBranchError::Head(name.to_owned()));
    }
    let target = branch.get().peel_to_commit()?.id();
    if !force {
        let commits = unmerged_commits(repo, &branch, target)?;
        if commits > 0 {
            return Err(DeleteBranchError::Unmerged {
                name: name.to_owned(),
                commits,
            });
        }
    }
    branch.delete()?;
    Ok(target)
}

/// How many commits of a branch (at `target`) neither HEAD nor the
/// branch's upstream has.
fn unmerged_commits(
    repo: &Repository,
    branch: &Branch<'_>,
    target: Oid,
) -> Result<usize, git2::Error> {
    let mut walk = repo.revwalk()?;
    walk.push(target)?;
    // An unborn HEAD, or an upstream that is gone, has nothing.
    if let Some(head) = repo.head().ok().and_then(|head| head.target()) {
        walk.hide(head)?;
    }
    if let Some(upstream) = branch
        .upstream()
        .ok()
        .and_then(|upstream| upstream.get().target())
    {
        walk.hide(upstream)?;
    }
    let mut commits = 0;
    for id in walk {
        id?;
        commits += 1;
    }
    Ok(commits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::head::{Head, head_of};
    use crate::git::history::tests::{TestRepo, add_submodule_to};
    use std::fs;

    #[test]
    fn a_branch_is_made_at_head_and_checked_out_without_touching_files() {
        let mut repo = TestRepo::new();
        let first = repo.commit(&[("a.txt", "a\n")], "first", &[]);
        let second = repo.commit(&[("a.txt", "a\nb\n")], "second", &[first]);
        // A change in the working tree, unstaged, comes along untouched.
        fs::write(repo.path().join("a.txt"), "edited\n").unwrap();

        create_branch(repo.path(), false, "feature").unwrap();
        assert_eq!(
            head_of(&repo.repo),
            Some(Head::Branch("feature".to_owned()))
        );
        assert_eq!(repo.repo.head().unwrap().target(), Some(second));
        let master = repo.repo.find_reference("refs/heads/master").unwrap();
        assert_eq!(master.target(), Some(second));
        assert_eq!(
            fs::read_to_string(repo.path().join("a.txt")).unwrap(),
            "edited\n"
        );

        // From a detached HEAD likewise: the branch is at the commit.
        repo.repo.set_head_detached(first).unwrap();
        create_branch(&repo.path().join("a.txt"), false, "old").unwrap();
        assert_eq!(head_of(&repo.repo), Some(Head::Branch("old".to_owned())));
        assert_eq!(repo.repo.head().unwrap().target(), Some(first));
        assert_eq!(
            fs::read_to_string(repo.path().join("a.txt")).unwrap(),
            "edited\n"
        );
    }

    #[test]
    fn a_bad_or_taken_name_is_refused_and_nothing_changes() {
        let mut repo = TestRepo::new();
        let first = repo.commit(&[("a.txt", "a\n")], "first", &[]);
        repo.branch("feature", first);
        for (name, expected) in [
            ("", "\"\" is not a valid branch name"),
            ("bad name", "\"bad name\" is not a valid branch name"),
            ("feature", "branch feature already exists"),
        ] {
            let err = create_branch(repo.path(), false, name).unwrap_err();
            assert_eq!(err.to_string(), expected);
        }
        assert_eq!(head_of(&repo.repo), Some(Head::Branch("master".to_owned())));
        assert!(matches!(
            create_branch(Path::new("/nonexistent/nowhere"), false, "x"),
            Err(BranchError::Git(_))
        ));
    }

    #[test]
    fn an_unborn_head_takes_the_new_name() {
        let repo = TestRepo::new();
        create_branch(repo.path(), false, "start").unwrap();
        assert_eq!(head_of(&repo.repo), Some(Head::Branch("start".to_owned())));
        assert!(repo.repo.find_reference("refs/heads/start").is_err());
    }

    #[test]
    fn an_exact_repository_is_the_submodule_and_not_its_parent() {
        let mut repo = TestRepo::new();
        repo.commit(&[("a.txt", "a\n")], "first", &[]);
        let sub = add_submodule_to(&repo.repo, "sub");
        create_branch(sub.workdir().unwrap(), true, "subfeature").unwrap();
        assert_eq!(head_of(&sub), Some(Head::Branch("subfeature".to_owned())));
        assert_eq!(head_of(&repo.repo), Some(Head::Branch("master".to_owned())));
        assert!(
            repo.repo
                .find_branch("subfeature", git2::BranchType::Local)
                .is_err()
        );
    }

    #[test]
    fn a_merged_branch_is_deleted_and_an_unmerged_one_only_when_forced() {
        let mut repo = TestRepo::new();
        let first = repo.commit(&[("a.txt", "a\n")], "first", &[]);
        let second = repo.commit(&[("a.txt", "b\n")], "second", &[first]);
        // merged: behind HEAD. ahead: two commits of its own.
        repo.branch("merged", first);
        let own = repo.commit(&[("a.txt", "c\n")], "own", &[second]);
        let own2 = repo.commit(&[("a.txt", "d\n")], "own 2", &[own]);
        repo.branch("ahead", own2);
        repo.repo
            .reference("refs/heads/master", second, true, "back")
            .unwrap();
        repo.repo
            .config()
            .unwrap()
            .set_str("branch.merged.remote", "origin")
            .unwrap();

        assert_eq!(
            delete_branch(repo.repo.path(), "merged", false).unwrap(),
            first
        );
        assert!(repo.repo.find_branch("merged", BranchType::Local).is_err());
        // Its configuration goes with it.
        assert!(
            repo.repo
                .config()
                .unwrap()
                .snapshot()
                .unwrap()
                .get_str("branch.merged.remote")
                .is_err()
        );

        let err = delete_branch(repo.repo.path(), "ahead", false).unwrap_err();
        assert!(matches!(
            err,
            DeleteBranchError::Unmerged { commits: 2, .. }
        ));
        assert_eq!(
            err.to_string(),
            "ahead has 2 commits not merged into HEAD or its upstream"
        );
        assert!(repo.repo.find_branch("ahead", BranchType::Local).is_ok());

        // Its upstream having the commits is as good as HEAD having them.
        repo.remote_branch("origin", "ahead", own);
        repo.repo
            .find_branch("ahead", BranchType::Local)
            .unwrap()
            .set_upstream(Some("origin/ahead"))
            .unwrap();
        let err = delete_branch(repo.repo.path(), "ahead", false).unwrap_err();
        assert!(matches!(
            err,
            DeleteBranchError::Unmerged { commits: 1, .. }
        ));
        assert_eq!(
            delete_branch(repo.repo.path(), "ahead", true).unwrap(),
            own2
        );
        assert!(repo.repo.find_branch("ahead", BranchType::Local).is_err());
        // The remote's branch is left alone.
        assert!(
            repo.repo
                .find_branch("origin/ahead", BranchType::Remote)
                .is_ok()
        );
    }

    #[test]
    fn heads_branch_or_one_that_is_not_there_is_not_deleted() {
        let mut repo = TestRepo::new();
        repo.commit(&[("a.txt", "a\n")], "first", &[]);
        let err = delete_branch(repo.repo.path(), "master", true).unwrap_err();
        assert_eq!(err.to_string(), "master is checked out");
        assert!(matches!(
            delete_branch(repo.repo.path(), "nothing", false),
            Err(DeleteBranchError::Git(_))
        ));
    }
}
