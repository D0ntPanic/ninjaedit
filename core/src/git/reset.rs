//! Resetting HEAD to a commit, as `git reset <commit>` does in its
//! default, mixed mode: the branch HEAD is on (or a detached HEAD) is
//! moved to the commit and the index made to match it, while the
//! working directory is left exactly as it is, so that whatever differs
//! from the commit (the work of the commits left behind, and any
//! changes that were there already) shows as unstaged changes. Nothing
//! in the working directory is lost.
//!
//! What can be lost track of is commits: those HEAD had that the commit
//! doesn't, and that no other branch, remote branch, or tag has,
//! are left on no branch (git's reflog keeps them for a while).
//! [`left_behind`] counts them, for a frontend to ask before going
//! ahead. A reset waits for a merge or rebase in progress to be
//! finished or aborted, since it would end it half done.

use super::operation::{OperationError, head_commit, in_progress};
use git2::{Oid, Repository, ResetType};
use std::path::Path;

/// How many of HEAD's commits a reset to `id` would leave on no branch
/// of the repository whose git directory is `git_dir`: those no other
/// reference (a local or remote branch, or a tag) has.
pub fn left_behind(git_dir: &Path, id: Oid) -> Result<usize, OperationError> {
    left_behind_in(&Repository::open(git_dir)?, id)
}

/// [`left_behind`] in an opened repository.
pub fn left_behind_in(repo: &Repository, id: Oid) -> Result<usize, OperationError> {
    let head = head_commit(repo)?;
    let own = repo
        .head()
        .ok()
        .filter(|head| head.is_branch())
        .and_then(|head| head.name().ok().map(str::to_owned));
    let mut walk = repo.revwalk()?;
    walk.push(head)?;
    walk.hide(id)?;
    for reference in repo.references()? {
        let reference = reference?;
        let Ok(name) = reference.name() else {
            continue;
        };
        let other = name.starts_with("refs/heads/")
            || name.starts_with("refs/remotes/")
            || name.starts_with("refs/tags/");
        if !other || own.as_deref() == Some(name) {
            continue;
        }
        // A remote's symbolic HEAD, or a tag of something other than a
        // commit, hides nothing.
        if let Ok(commit) = reference.peel_to_commit() {
            walk.hide(commit.id())?;
        }
    }
    let mut commits = 0;
    for id in walk {
        id?;
        commits += 1;
    }
    Ok(commits)
}

/// Reset HEAD to `id` in the repository whose git directory is
/// `git_dir`, keeping the working directory as it is (see the
/// [module](self) documentation).
pub fn reset(git_dir: &Path, id: Oid) -> Result<(), OperationError> {
    reset_in(&Repository::open(git_dir)?, id)
}

/// [`reset`] in an opened repository.
pub fn reset_in(repo: &Repository, id: Oid) -> Result<(), OperationError> {
    if let Some(what) = in_progress(repo) {
        return Err(OperationError::InProgress(what));
    }
    head_commit(repo)?;
    let commit = repo.find_commit(id)?;
    repo.reset(commit.as_object(), ResetType::Mixed, None)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::operation::tests::{diverged, read, repo};
    use git2::Status;
    use std::fs;

    #[test]
    fn the_branch_moves_and_the_files_stay_as_unstaged_changes() {
        let mut t = repo();
        let base = t.commit(&[("f.txt", "base\n")], "Base", &[]);
        let next = t.commit(&[("f.txt", "next\n"), ("g.txt", "new\n")], "Next", &[base]);
        fs::write(t.path().join("f.txt"), "edited\n").unwrap();

        // The one commit left behind is on no other branch.
        assert_eq!(left_behind_in(&t.repo, base).unwrap(), 1);
        reset_in(&t.repo, base).unwrap();
        assert_eq!(t.repo.head().unwrap().shorthand().unwrap(), "master");
        assert_eq!(t.repo.head().unwrap().target(), Some(base));
        assert_eq!(read(&t, "f.txt"), "edited\n");
        assert_eq!(read(&t, "g.txt"), "new\n");
        assert_eq!(
            t.repo.status_file(Path::new("f.txt")).unwrap(),
            Status::WT_MODIFIED
        );
        assert_eq!(
            t.repo.status_file(Path::new("g.txt")).unwrap(),
            Status::WT_NEW
        );

        // Back again: nothing is left behind going forward.
        assert_eq!(left_behind_in(&t.repo, next).unwrap(), 0);
        reset_in(&t.repo, next).unwrap();
        assert_eq!(t.repo.head().unwrap().target(), Some(next));
        assert_eq!(
            t.repo.status_file(Path::new("g.txt")).unwrap(),
            Status::CURRENT
        );
    }

    #[test]
    fn commits_another_reference_has_are_not_left_behind() {
        let mut t = repo();
        let (base, main, side) = diverged(&mut t, false);
        // Main's own commit is on no other branch; side's commits are
        // side's.
        assert_eq!(left_behind_in(&t.repo, side).unwrap(), 1);
        t.tag("kept", main);
        assert_eq!(left_behind_in(&t.repo, side).unwrap(), 0);
        assert_eq!(left_behind_in(&t.repo, base).unwrap(), 0);
        t.repo.tag_delete("kept").unwrap();
        t.remote_branch("origin", "master", main);
        assert_eq!(left_behind_in(&t.repo, side).unwrap(), 0);
        // A detached HEAD's own commit counts too.
        t.repo.set_head_detached(main).unwrap();
        let more = t.commit(&[("h.txt", "h\n")], "More", &[main]);
        assert_eq!(t.repo.head().unwrap().target(), Some(more));
        assert_eq!(left_behind_in(&t.repo, main).unwrap(), 1);
        reset_in(&t.repo, main).unwrap();
        assert!(t.repo.head_detached().unwrap());
        assert_eq!(t.repo.head().unwrap().target(), Some(main));
    }

    #[test]
    fn a_merge_in_progress_refuses_a_reset() {
        let mut t = repo();
        let (base, _, _) = diverged(&mut t, false);
        fs::write(t.repo.path().join("MERGE_HEAD"), format!("{base}\n")).unwrap();
        let err = reset_in(&t.repo, base).unwrap_err();
        assert!(
            err.to_string().starts_with("a merge is in progress"),
            "{err}"
        );
    }
}
