//! Merging another branch into the one HEAD is on, as `git merge
//! --no-edit` does: up to date, a fast-forward, or a merge commit with
//! the message git prepares (`Merge branch 'feature'`). A merge whose
//! changes conflict stops with the conflicts in the working directory
//! and the index and the merge in progress, as git leaves it, for the
//! changes page to resolve and commit (see the
//! [`changes`](super::changes) module, whose commit finishes a merge),
//! or for [`abort_merge`] to throw away. See the
//! [`operation`](super::operation) module for what merging and
//! rebasing share.

use super::operation::{
    OperationError, Outcome, Target, checkout_options, conflicted_files, ensure_ready,
    fast_forward, follow_submodules, settle_part_way,
};
use git2::build::CheckoutBuilder;
use git2::{Repository, RepositoryState, ResetType};
use std::collections::BTreeSet;
use std::fs;

/// Merge `target` into HEAD's branch (or a detached HEAD), telling
/// `progress` how many files have been written of how many.
pub(super) fn merge(
    repo: &Repository,
    target: &Target,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Outcome, OperationError> {
    let head = ensure_ready(repo)?;
    let theirs = target.resolve(repo)?;
    let their_id = theirs.id();
    let their_commit = repo.find_commit(their_id)?;
    super::checkout::check_submodules(repo, &their_commit)?;

    let (analysis, _) = repo.merge_analysis(&[&theirs])?;
    if analysis.is_up_to_date() {
        return Ok(Outcome::UpToDate);
    }
    if analysis.is_fast_forward() {
        let why = format!("merge {}: Fast-forward", target.name());
        let submodules = fast_forward(repo, their_id, &why, progress)?;
        return Ok(Outcome::FastForwarded { submodules });
    }

    {
        let mut options = checkout_options(progress);
        repo.merge(&[&theirs], None, Some(&mut options))?;
    }
    name_remote_branch(repo, target);
    let mut index = repo.index()?;
    // Submodule conflicts that can be resolved are; with nothing else in
    // conflict, the merge is committed as a clean one would be.
    let mut moved = BTreeSet::new();
    let mut resolved = 0;
    if index.has_conflicts() {
        let settled = settle_part_way(repo, progress)?;
        if index.has_conflicts() {
            return Ok(Outcome::Conflicts {
                files: conflicted_files(&index)?,
                step: None,
                unresolved: settled.unresolved,
            });
        }
        moved.extend(settled.moved);
        resolved = settled.resolved;
    }
    // libgit2 lists the conflicts it met in the message as comments,
    // which git's command line would strip when committing.
    let message = fs::read_to_string(repo.path().join("MERGE_MSG"))
        .ok()
        .and_then(|text| git2::message_prettify(text, Some(b'#')).ok())
        .map(|text| text.trim_end().to_owned())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| format!("Merge {}", target.name()));
    let tree = repo.find_tree(index.write_tree()?)?;
    let signature = repo.signature()?;
    let ours = repo.find_commit(head)?;
    let commit = repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        &message,
        &tree,
        &[&ours, &their_commit],
    )?;
    repo.cleanup_state()?;
    let submodules = moved.len() + follow_submodules(repo, commit, progress)?;
    Ok(Outcome::Merged {
        commit,
        submodules,
        resolved,
    })
}

/// Name a remote's branch in the message libgit2 prepared as git's
/// command line does (`Merge remote-tracking branch 'origin/main'`),
/// rather than by its full reference name. The message is git's own
/// state, so the changes page shows it too when the merge stops at
/// conflicts.
fn name_remote_branch(repo: &Repository, target: &Target) {
    let Target::Branch { name, remote: true } = target else {
        return;
    };
    let path = repo.path().join("MERGE_MSG");
    if let Ok(text) = fs::read_to_string(&path) {
        let full = format!("'refs/remotes/{name}'");
        if text.contains(&full) {
            let _ = fs::write(&path, text.replacen(&full, &format!("'{name}'"), 1));
        }
    }
}

/// Give up a merge in progress, as `git merge --abort` does: the
/// working tree and index go back to HEAD, conflicts, resolutions, and
/// all, and the merge state is cleared. Only the merge's changes can
/// be there, since a merge doesn't start with changes to tracked
/// files; untracked files are left alone. The submodules the merge
/// moved go back too.
pub fn abort_merge(repo: &Repository) -> Result<(), OperationError> {
    if repo.state() != RepositoryState::Merge {
        return Err(OperationError::NothingInProgress);
    }
    let head = repo.head()?.peel_to_commit()?;
    let mut options = CheckoutBuilder::new();
    options.force();
    repo.reset(head.as_object(), ResetType::Hard, Some(&mut options))?;
    repo.cleanup_state()?;
    follow_submodules(repo, head.id(), &mut |_, _| {})?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::operation::tests::{diverged, read, repo, submodule_moved_on_side, sync};
    use crate::git::short_id;
    use git2::Oid;
    use std::fs;

    fn run(repo: &Repository, target: &Target) -> Result<Outcome, OperationError> {
        merge(repo, target, &mut |_, _| {})
    }

    fn branch(name: &str) -> Target {
        Target::Branch {
            name: name.to_owned(),
            remote: false,
        }
    }

    fn head(repo: &Repository) -> Oid {
        repo.head().unwrap().target().unwrap()
    }

    #[test]
    fn a_merge_commits_both_sides_with_gits_message() {
        let mut t = repo();
        let (_, main, side) = diverged(&mut t, false);
        let outcome = run(&t.repo, &branch("side")).unwrap();
        let Outcome::Merged {
            commit, submodules, ..
        } = outcome
        else {
            panic!("{outcome:?}");
        };
        assert_eq!(submodules, 0);
        assert_eq!(head(&t.repo), commit);
        assert_eq!(t.repo.head().unwrap().shorthand().unwrap(), "master");
        let merged = t.repo.find_commit(commit).unwrap();
        assert_eq!(merged.parent_ids().collect::<Vec<_>>(), [main, side]);
        assert_eq!(merged.message().unwrap(), "Merge branch 'side'");
        assert_eq!(read(&t, "f.txt"), "main\n");
        assert_eq!(read(&t, "g.txt"), "side\n");
        assert_eq!(t.repo.state(), RepositoryState::Clean);
        let statuses = t.repo.statuses(None).unwrap();
        assert!(statuses.is_empty(), "the tree is clean after");
    }

    #[test]
    fn up_to_date_and_fast_forward_make_no_merge_commit() {
        let mut t = repo();
        let base = t.commit(&[("f.txt", "base\n")], "Base", &[]);
        let ahead = t.commit(&[("f.txt", "ahead\n")], "Ahead", &[base]);
        t.branch("old", base);
        assert_eq!(run(&t.repo, &branch("old")).unwrap(), Outcome::UpToDate);
        assert_eq!(head(&t.repo), ahead);

        // HEAD back at base on its branch; merging a branch ahead of it
        // moves the branch there.
        t.branch("new", ahead);
        t.repo
            .head()
            .unwrap()
            .set_target(base, "test: back")
            .unwrap();
        sync(&t);
        assert_eq!(read(&t, "f.txt"), "base\n");
        assert_eq!(
            run(&t.repo, &branch("new")).unwrap(),
            Outcome::FastForwarded { submodules: 0 }
        );
        assert_eq!(head(&t.repo), ahead);
        assert_eq!(t.repo.head().unwrap().shorthand().unwrap(), "master");
        assert_eq!(read(&t, "f.txt"), "ahead\n");

        // A commit by id merges as well as a branch.
        t.repo.set_head_detached(base).unwrap();
        sync(&t);
        assert_eq!(
            run(&t.repo, &Target::Commit(ahead)).unwrap(),
            Outcome::FastForwarded { submodules: 0 }
        );
        assert!(t.repo.head_detached().unwrap());
        assert_eq!(head(&t.repo), ahead);
    }

    #[test]
    fn a_conflict_stops_the_merge_in_progress_and_abort_undoes_it() {
        let mut t = repo();
        let (_, main, side) = diverged(&mut t, true);
        let outcome = run(&t.repo, &branch("side")).unwrap();
        assert_eq!(
            outcome,
            Outcome::Conflicts {
                files: 1,
                step: None,
                unresolved: Vec::new(),
            }
        );
        assert_eq!(t.repo.state(), RepositoryState::Merge);
        assert_eq!(head(&t.repo), main);
        let text = read(&t, "f.txt");
        assert!(text.contains("<<<<<<<") && text.contains("side"), "{text}");
        let merge_head = fs::read_to_string(t.repo.path().join("MERGE_HEAD")).unwrap();
        assert_eq!(merge_head.trim(), side.to_string());

        // Another merge or a rebase is refused meanwhile.
        let err = run(&t.repo, &branch("side")).unwrap_err();
        assert!(
            err.to_string().starts_with("a merge is in progress"),
            "{err}"
        );

        abort_merge(&t.repo).unwrap();
        assert_eq!(t.repo.state(), RepositoryState::Clean);
        assert_eq!(head(&t.repo), main);
        assert_eq!(read(&t, "f.txt"), "main\n");
        assert!(t.repo.statuses(None).unwrap().is_empty());
        assert!(matches!(
            abort_merge(&t.repo),
            Err(OperationError::NothingInProgress)
        ));
    }

    #[test]
    fn submodules_follow_a_merge_whether_it_stops_at_conflicts_or_not() {
        // Clean: the merge commit records side's submodule commit, and
        // the submodule is checked out there.
        let mut t = repo();
        let (sub, [_, _, side, _, s2]) = submodule_moved_on_side(&mut t, false);
        let outcome = run(&t.repo, &Target::Commit(side)).unwrap();
        assert!(
            matches!(outcome, Outcome::Merged { submodules: 1, .. }),
            "{outcome:?}"
        );
        assert_eq!(sub.head().unwrap().target(), Some(s2));
        assert!(t.repo.statuses(None).unwrap().is_empty());

        // Stopped at a conflict elsewhere: the submodule is already where
        // the merge has it, so committing the resolution keeps it there.
        let mut t = repo();
        let (sub, [_, _, side, _, s2]) = submodule_moved_on_side(&mut t, true);
        let outcome = run(&t.repo, &Target::Commit(side)).unwrap();
        assert_eq!(
            outcome,
            Outcome::Conflicts {
                files: 1,
                step: None,
                unresolved: Vec::new(),
            }
        );
        assert_eq!(sub.head().unwrap().target(), Some(s2));
        assert_eq!(
            t.repo.status_file(std::path::Path::new("sub")).unwrap(),
            git2::Status::INDEX_MODIFIED
        );
        // Aborting takes it back.
        abort_merge(&t.repo).unwrap();
        assert_ne!(sub.head().unwrap().target(), Some(s2));
        assert!(t.repo.statuses(None).unwrap().is_empty());
    }

    #[test]
    fn a_remote_branch_merges_by_its_name() {
        let mut t = repo();
        let (_, _, side) = diverged(&mut t, false);
        t.remote_branch("origin", "side", side);
        let target = Target::Branch {
            name: "origin/side".to_owned(),
            remote: true,
        };
        let Outcome::Merged { commit, .. } = run(&t.repo, &target).unwrap() else {
            panic!("a merge commit");
        };
        let message = t
            .repo
            .find_commit(commit)
            .unwrap()
            .message()
            .unwrap()
            .to_owned();
        assert_eq!(message, "Merge remote-tracking branch 'origin/side'");
        assert_ne!(short_id(commit), short_id(side));
    }
}
