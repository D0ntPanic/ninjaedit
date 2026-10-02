//! Rebasing the branch HEAD is on onto another commit, as `git rebase
//! <target>` does: each of HEAD's commits that the target doesn't have
//! is replayed on top of it, in order, keeping its author and message,
//! and the branch moved to the last. Merge commits are left out, as git
//! leaves them, and a commit whose change the target already has is
//! dropped. A target that already has every commit of HEAD is up to
//! date; one that HEAD is behind is a fast-forward.
//!
//! A commit whose replay conflicts stops the rebase there, in progress,
//! with HEAD detached at the commits replayed so far and the conflicts
//! in the working directory and the index, as git leaves it, for the
//! changes page to resolve. [`continue_rebase`] then commits the
//! resolved commit, with the message given or its own, and replays the
//! rest, which may stop again; [`abort_rebase`] puts the branch back
//! where it was. [`rebase_status`] says what a rebase in progress is
//! doing, for the changes page's heading. See the
//! [`operation`](super::operation) module for what merging and
//! rebasing share.

use super::history::short_id;
use super::operation::{
    Integration, OperationError, Outcome, Target, checkout_options, conflicted_files, ensure_ready,
    fast_forward, follow_submodules, head_commit, settle_part_way,
};
use git2::{ErrorCode, Oid, Rebase, RebaseOptions, Repository, RepositoryState, Signature};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fs;

/// What a rebase in progress is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebaseStatus {
    /// The branch being rebased, or `None` for a detached HEAD.
    pub branch: Option<String>,
    /// What it is being rebased onto, as named when it started: a
    /// branch, or a commit id.
    pub onto: String,
    /// The commit being replayed, counting from 1, and how many there
    /// are.
    pub step: usize,
    pub total: usize,
    /// The message of the commit being replayed, which it keeps unless
    /// the user writes another.
    pub message: Option<String>,
}

impl RebaseStatus {
    /// How continuing the rebase went, for the status bar (see
    /// [`Integration::summary`]).
    pub fn summary(&self, outcome: &Outcome) -> String {
        let onto = Target::Branch {
            name: self.onto.clone(),
            remote: false,
        };
        Integration::Rebase(onto).summary(self.branch.as_deref(), outcome)
    }
}

/// Rebase HEAD's branch (or a detached HEAD) onto `target`, telling
/// `progress` how many files have been written of how many.
pub(super) fn rebase(
    repo: &Repository,
    target: &Target,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Outcome, OperationError> {
    let head = ensure_ready(repo)?;
    let upstream = target.resolve(repo)?;
    let onto = upstream.id();
    super::checkout::check_submodules(repo, &repo.find_commit(onto)?)?;
    if head == onto || repo.graph_descendant_of(head, onto)? {
        return Ok(Outcome::UpToDate);
    }
    if repo.graph_descendant_of(onto, head)? {
        let why = format!("rebase: fast-forward to {}", target.name());
        let submodules = fast_forward(repo, onto, &why, progress)?;
        return Ok(Outcome::FastForwarded { submodules });
    }
    // Each step's checkout and the submodules following it report to
    // the one callback, never both at once.
    let progress = RefCell::new(progress);
    let mut checkout_progress = |completed, total| (*progress.borrow_mut())(completed, total);
    let mut submodule_progress = |completed, total| (*progress.borrow_mut())(completed, total);
    // The options must outlive the rebase: libgit2 keeps the checkout
    // progress callback they hold for every step.
    let mut options = RebaseOptions::new();
    options.checkout_options(checkout_options(&mut checkout_progress));
    let mut rebase = repo.rebase(None, Some(&upstream), None, Some(&mut options))?;
    let signature = repo.signature()?;
    replay(
        repo,
        &mut rebase,
        &signature,
        Tally::default(),
        &mut submodule_progress,
    )
}

/// Commit the commit a rebase stopped at, now that its conflicts are
/// resolved and staged, with `message` or, given none, its own; then
/// replay the rest, as `git rebase --continue` does. A commit that the
/// resolution leaves with no change of its own is dropped, as git
/// drops it. Stops again at the next conflict.
pub fn continue_rebase(
    repo: &Repository,
    message: Option<&str>,
) -> Result<Outcome, OperationError> {
    if repo.state() != RepositoryState::RebaseMerge {
        return Err(OperationError::NothingInProgress);
    }
    // Submodules as the resolution has them (a commit staged for one is
    // checked out in it), and any submodule conflict that has become
    // resolvable since the stop: one whose own branch the user has now
    // rebased, as the stop said to.
    let progress = &mut |_, _| {};
    let settled = settle_part_way(repo, progress)?;
    let index = repo.index()?;
    if index.has_conflicts() {
        return Err(OperationError::Unresolved {
            files: conflicted_files(&index)?,
            submodules: settled.unresolved,
        });
    }
    let mut options = RebaseOptions::new();
    options.checkout_options(git2::build::CheckoutBuilder::new());
    let mut rebase = repo.open_rebase(Some(&mut options))?;
    let signature = repo.signature()?;
    let mut tally = Tally {
        resolved: settled.resolved,
        ..Tally::default()
    };
    tally.moved.extend(settled.moved);
    let message = message.map(str::trim).filter(|text| !text.is_empty());
    if rebase.operation_current().is_some() {
        commit_step(&mut rebase, &signature, message, &mut tally)?;
    }
    replay(repo, &mut rebase, &signature, tally, progress)
}

/// Give up a rebase in progress, as `git rebase --abort` does: the
/// branch goes back to where it was, and the working tree and index
/// with it, and the submodules the rebase moved.
pub fn abort_rebase(repo: &Repository) -> Result<(), OperationError> {
    if repo.state() != RepositoryState::RebaseMerge {
        return Err(OperationError::NothingInProgress);
    }
    repo.open_rebase(None)?.abort()?;
    follow_submodules(repo, head_commit(repo)?, &mut |_, _| {})?;
    Ok(())
}

/// What a rebase in progress is doing, if one is (and is one the
/// editor can carry on with: not interactive).
pub fn rebase_status(repo: &Repository) -> Option<RebaseStatus> {
    if repo.state() != RepositoryState::RebaseMerge {
        return None;
    }
    let mut rebase = repo.open_rebase(None).ok()?;
    let total = rebase.len();
    let current = rebase.operation_current();
    let branch = rebase
        .orig_head_name()
        .ok()
        .flatten()
        .map(|name| name.strip_prefix("refs/heads/").unwrap_or(name).to_owned());
    let message = current
        .and_then(|index| rebase.nth(index).map(|operation| operation.id()))
        .and_then(|id| repo.find_commit(id).ok())
        .and_then(|commit| commit.message().ok().map(|text| text.trim_end().to_owned()));
    let onto = fs::read_to_string(repo.path().join("rebase-merge").join("onto_name"))
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        // A commit rebased onto by id is named by all of it.
        .map(|text| match Oid::from_str(&text) {
            Ok(id) if text.len() == 40 => short_id(id),
            _ => text,
        })
        .unwrap_or_else(|| "?".to_owned());
    Some(RebaseStatus {
        branch,
        onto,
        step: current.map_or(0, |index| index + 1),
        total,
        message,
    })
}

/// The commits a rebase has made and dropped as it goes, and the
/// submodules it has moved.
#[derive(Clone, Debug, Default)]
struct Tally {
    commits: usize,
    skipped: usize,
    moved: BTreeSet<String>,
    resolved: usize,
}

/// Replay the rest of a rebase's commits, stopping at one that
/// conflicts, and finish it once all are done. The submodules follow
/// each step before it is committed, as libgit2 refuses to commit one
/// while a submodule isn't where the index has it (the first step's
/// checkout of the target moves every submodule the target moved), and
/// follow a step that stops at conflicts as far as they aren't in
/// conflict themselves. A step whose only conflicts are submodules
/// whose own branches were brought up to date is resolved and goes on
/// (see the [`submodule_conflicts`](super::submodule_conflicts)
/// module).
fn replay(
    repo: &Repository,
    rebase: &mut Rebase<'_>,
    signature: &Signature<'_>,
    mut tally: Tally,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Outcome, OperationError> {
    let total = rebase.len();
    while let Some(operation) = rebase.next() {
        operation?;
        let settled = settle_part_way(repo, progress)?;
        tally.moved.extend(settled.moved);
        tally.resolved += settled.resolved;
        let index = repo.index()?;
        if index.has_conflicts() {
            return Ok(Outcome::Conflicts {
                files: conflicted_files(&index)?,
                step: rebase.operation_current().map(|index| (index + 1, total)),
                unresolved: settled.unresolved,
            });
        }
        commit_step(rebase, signature, None, &mut tally)?;
    }
    rebase.finish(Some(signature))?;
    let head = repo
        .head()?
        .target()
        .ok_or_else(|| git2::Error::from_str("HEAD points at nothing after the rebase"))?;
    // They are there already, unless the rebase had nothing to replay.
    let submodules = tally.moved.len() + follow_submodules(repo, head, progress)?;
    Ok(Outcome::Rebased {
        commits: tally.commits,
        skipped: tally.skipped,
        submodules,
        resolved: tally.resolved,
    })
}

/// Commit the rebase's current commit as replayed, keeping its author,
/// or drop it when it changes nothing any more.
fn commit_step(
    rebase: &mut Rebase<'_>,
    signature: &Signature<'_>,
    message: Option<&str>,
    tally: &mut Tally,
) -> Result<(), OperationError> {
    match rebase.commit(None, signature, message) {
        Ok(_) => tally.commits += 1,
        Err(err) if err.code() == ErrorCode::Applied => tally.skipped += 1,
        Err(err) => return Err(err.into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::operation::tests::{diverged, read, repo, submodule_moved_on_side, sync};
    use git2::Oid;
    use std::path::Path;

    fn run(repo: &Repository, target: &Target) -> Result<Outcome, OperationError> {
        rebase(repo, target, &mut |_, _| {})
    }

    fn side() -> Target {
        Target::Branch {
            name: "side".to_owned(),
            remote: false,
        }
    }

    fn head(repo: &Repository) -> Oid {
        repo.head().unwrap().target().unwrap()
    }

    fn stage(repo: &Repository, path: &str) {
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(path)).unwrap();
        index.write().unwrap();
    }

    #[test]
    fn heads_commits_are_replayed_on_the_target() {
        let mut t = repo();
        let (_, main, side_id) = diverged(&mut t, false);
        let outcome = run(&t.repo, &side()).unwrap();
        assert_eq!(
            outcome,
            Outcome::Rebased {
                commits: 1,
                skipped: 0,
                submodules: 0,
                resolved: 0,
            }
        );
        assert_eq!(t.repo.state(), RepositoryState::Clean);
        assert_eq!(t.repo.head().unwrap().shorthand().unwrap(), "master");
        let replayed = t.repo.find_commit(head(&t.repo)).unwrap();
        assert_ne!(replayed.id(), main);
        assert_eq!(replayed.parent_ids().collect::<Vec<_>>(), [side_id]);
        assert_eq!(replayed.message().unwrap(), "Main");
        assert_eq!(replayed.author().name().unwrap(), "Test Author");
        assert_eq!(read(&t, "f.txt"), "main\n");
        assert_eq!(read(&t, "g.txt"), "side\n");
        assert!(t.repo.statuses(None).unwrap().is_empty());
    }

    #[test]
    fn up_to_date_and_fast_forward_replay_nothing() {
        let mut t = repo();
        let base = t.commit(&[("f.txt", "base\n")], "Base", &[]);
        let ahead = t.commit(&[("f.txt", "ahead\n")], "Ahead", &[base]);
        assert_eq!(
            run(&t.repo, &Target::Commit(base)).unwrap(),
            Outcome::UpToDate
        );
        assert_eq!(
            run(&t.repo, &Target::Commit(ahead)).unwrap(),
            Outcome::UpToDate
        );
        t.repo.head().unwrap().set_target(base, "test").unwrap();
        sync(&t);
        assert_eq!(
            run(&t.repo, &Target::Commit(ahead)).unwrap(),
            Outcome::FastForwarded { submodules: 0 }
        );
        assert_eq!(head(&t.repo), ahead);
        assert_eq!(read(&t, "f.txt"), "ahead\n");
    }

    #[test]
    fn a_conflict_stops_the_rebase_which_continues_once_resolved() {
        let mut t = repo();
        let (_, main, side_id) = diverged(&mut t, true);
        // A second commit of main's, after the one that conflicts.
        let second = t.commit(&[("h.txt", "more\n")], "More", &[main]);
        let outcome = run(&t.repo, &side()).unwrap();
        assert_eq!(
            outcome,
            Outcome::Conflicts {
                files: 1,
                step: Some((1, 2)),
                unresolved: Vec::new(),
            }
        );
        assert_eq!(t.repo.state(), RepositoryState::RebaseMerge);
        assert!(read(&t, "f.txt").contains("<<<<<<<"));
        let status = rebase_status(&t.repo).unwrap();
        assert_eq!(
            status,
            RebaseStatus {
                branch: Some("master".to_owned()),
                onto: "side".to_owned(),
                step: 1,
                total: 2,
                message: Some("Main".to_owned()),
            }
        );

        // Continuing with the conflict unresolved is refused.
        let err = continue_rebase(&t.repo, None).unwrap_err();
        assert!(
            matches!(&err, OperationError::Unresolved { files: 1, submodules } if submodules.is_empty()),
            "{err:?}"
        );

        fs::write(t.path().join("f.txt"), "both\n").unwrap();
        stage(&t.repo, "f.txt");
        let outcome = continue_rebase(&t.repo, Some("Main, resolved\n")).unwrap();
        assert_eq!(
            outcome,
            Outcome::Rebased {
                commits: 2,
                skipped: 0,
                submodules: 0,
                resolved: 0,
            }
        );
        assert_eq!(t.repo.state(), RepositoryState::Clean);
        assert!(rebase_status(&t.repo).is_none());
        assert_eq!(t.repo.head().unwrap().shorthand().unwrap(), "master");
        let last = t.repo.find_commit(head(&t.repo)).unwrap();
        assert_ne!(last.id(), second);
        assert_eq!(last.message().unwrap(), "More");
        let resolved = last.parent(0).unwrap();
        assert_eq!(resolved.message().unwrap(), "Main, resolved");
        assert_eq!(resolved.parent_ids().collect::<Vec<_>>(), [side_id]);
        assert_eq!(read(&t, "f.txt"), "both\n");
        assert_eq!(read(&t, "h.txt"), "more\n");
    }

    #[test]
    fn a_resolution_to_the_targets_version_drops_the_commit() {
        let mut t = repo();
        let (_, _, side_id) = diverged(&mut t, true);
        run(&t.repo, &side()).unwrap();
        fs::write(t.path().join("f.txt"), "side\n").unwrap();
        stage(&t.repo, "f.txt");
        let outcome = continue_rebase(&t.repo, None).unwrap();
        assert_eq!(
            outcome,
            Outcome::Rebased {
                commits: 0,
                skipped: 1,
                submodules: 0,
                resolved: 0,
            }
        );
        assert_eq!(head(&t.repo), side_id);
    }

    #[test]
    fn a_submodule_the_target_moves_follows_the_rebase() {
        let mut t = repo();
        let (sub, [_, _, side, _, s2]) = submodule_moved_on_side(&mut t, false);
        let outcome = run(
            &t.repo,
            &Target::Branch {
                name: "side".to_owned(),
                remote: false,
            },
        )
        .unwrap();
        assert_eq!(
            outcome,
            Outcome::Rebased {
                commits: 1,
                skipped: 0,
                submodules: 1,
                resolved: 0,
            }
        );
        let replayed = t.repo.find_commit(head(&t.repo)).unwrap();
        assert_eq!(replayed.parent_ids().collect::<Vec<_>>(), [side]);
        assert_eq!(sub.head().unwrap().target(), Some(s2));
        assert!(t.repo.statuses(None).unwrap().is_empty());
    }

    #[test]
    fn a_rebase_stopped_at_a_conflict_has_the_submodules_the_target_moved() {
        let mut t = repo();
        let (sub, [_, _, side, _, s2]) = submodule_moved_on_side(&mut t, true);
        let outcome = run(&t.repo, &Target::Commit(side)).unwrap();
        assert_eq!(
            outcome,
            Outcome::Conflicts {
                files: 1,
                step: Some((1, 1)),
                unresolved: Vec::new(),
            }
        );
        // The submodule is where side has it, so it isn't a change to
        // stage by mistake, and the step can be continued.
        assert_eq!(sub.head().unwrap().target(), Some(s2));
        assert_eq!(
            t.repo.status_file(Path::new("sub")).unwrap(),
            git2::Status::CURRENT
        );
        fs::write(t.path().join("f.txt"), "both\n").unwrap();
        stage(&t.repo, "f.txt");
        let outcome = continue_rebase(&t.repo, None).unwrap();
        assert!(
            matches!(outcome, Outcome::Rebased { commits: 1, .. }),
            "{outcome:?}"
        );
        assert_eq!(sub.head().unwrap().target(), Some(s2));
        assert!(t.repo.statuses(None).unwrap().is_empty());
    }

    #[test]
    fn aborting_brings_the_submodules_back_too() {
        let mut t = repo();
        let (sub, [_, main, side, s1, s2]) = submodule_moved_on_side(&mut t, true);
        run(&t.repo, &Target::Commit(side)).unwrap();
        assert_eq!(sub.head().unwrap().target(), Some(s2));
        abort_rebase(&t.repo).unwrap();
        assert_eq!(head(&t.repo), main);
        assert_eq!(sub.head().unwrap().target(), Some(s1));
        assert!(t.repo.statuses(None).unwrap().is_empty());
    }

    #[test]
    fn abort_puts_the_branch_back() {
        let mut t = repo();
        let (_, main, _) = diverged(&mut t, true);
        run(&t.repo, &side()).unwrap();
        assert!(t.repo.head_detached().unwrap());
        abort_rebase(&t.repo).unwrap();
        assert_eq!(t.repo.state(), RepositoryState::Clean);
        assert_eq!(t.repo.head().unwrap().shorthand().unwrap(), "master");
        assert_eq!(head(&t.repo), main);
        assert_eq!(read(&t, "f.txt"), "main\n");
        assert!(t.repo.statuses(None).unwrap().is_empty());
        assert!(matches!(
            abort_rebase(&t.repo),
            Err(OperationError::NothingInProgress)
        ));
        assert!(matches!(
            continue_rebase(&t.repo, None),
            Err(OperationError::NothingInProgress)
        ));
    }
}
