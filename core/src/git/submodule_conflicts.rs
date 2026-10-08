//! Resolving submodule conflicts in a merge or rebase, in the common
//! case where the submodule's own branch has already been brought up to
//! date: the work `git sub-resolve` does.
//!
//! When both sides of a merge or rebase moved a submodule, git leaves
//! its gitlink in conflict, with three stages: the commit both started
//! from (the ancestor), ours (HEAD's side: in a rebase, the target with
//! the commits replayed so far), and theirs (the commit being merged or
//! replayed). Git won't look inside the submodule to settle it. But the
//! usual workflow has settled it already: before rebasing the
//! superproject, the submodule's feature branch was rebased onto the
//! submodule commit the target now uses, so somewhere in the submodule
//! there is a commit that is ours with exactly the incoming commits
//! replayed on top. That is the commit the gitlink should point at.
//!
//! It is found by the incoming commits' messages: those of the commits
//! from the ancestor to theirs, which are what the superproject commit
//! being applied brings into the submodule, less any that ours has
//! already. A feature branch based on a commit upstream has since taken
//! in has that commit among the incoming ones, and upstream may have
//! cherry-picked one under an id of its own; rebasing the submodule
//! leaves both out (as `git rebase` does: the first is upstream's
//! already, and the second is dropped as the same patch), so they are
//! left out of the messages to match too. A candidate is any commit
//! that descends from ours and is reachable from one of the submodule's
//! branches, its remotes' branches, or its HEAD; it matches when the
//! commits from ours to it carry the same messages (as a multiset).
//! Replaying keeps messages, so this finds the rebased branch's tip, or
//! the commit within the replayed chain that a given superproject
//! commit wants; and a whole list of messages rarely matches by
//! accident, even in a large repository with backports and several
//! copies of a feature branch about. An incoming side that didn't move
//! the submodule past the ancestor resolves to ours.
//!
//! With exactly one match, the submodule is checked out there (on a
//! local branch of its own that points there, if there is one; refused
//! if the submodule has changes it could lose) and then the gitlink is
//! staged at it, so a checkout that fails stages nothing. With none,
//! several, a commit missing from the submodule, or a gitlink that was
//! added or removed on one side, the conflict is left as it is, for the
//! user, with the reason. A submodule merged by a true merge commit, or
//! squashed, has no matching messages, and is left too.

use super::checkout::{CheckoutError, follow_gitlink, open_submodule};
use super::history::short_id;
use git2::{Index, IndexEntry, IndexTime, Oid, Repository};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::path::Path;

/// An index entry's mode for a gitlink: a submodule's commit.
const GITLINK: u32 = 0o160000;

/// A submodule left in conflict, and why it couldn't be resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmoduleConflict {
    /// The submodule's path from the working directory.
    pub path: String,
    pub why: String,
}

/// What [`resolve_submodule_conflicts`] did.
#[derive(Debug, Default)]
pub(super) struct Resolution {
    /// The submodules resolved, by path.
    pub resolved: Vec<String>,
    /// Those left in conflict.
    pub unresolved: Vec<SubmoduleConflict>,
}

/// Why a submodule's conflict couldn't be resolved.
#[derive(Debug)]
enum Unresolved {
    /// The gitlink was added or removed on one side, rather than moved
    /// on both.
    OneSided,
    NotInitialized,
    /// The submodule doesn't have one of the three commits.
    MissingCommit {
        which: &'static str,
        id: Oid,
    },
    /// Ours and theirs share no history.
    Unrelated,
    /// No commit is ours with the incoming commits replayed on it.
    NoMatch {
        ours: Oid,
        incoming: usize,
    },
    /// Several are.
    Ambiguous {
        ours: Oid,
        found: Vec<Oid>,
    },
    /// The one that is couldn't be checked out in the submodule.
    Checkout {
        id: Oid,
        why: CheckoutError,
    },
    Git(git2::Error),
}

impl From<git2::Error> for Unresolved {
    fn from(err: git2::Error) -> Unresolved {
        Unresolved::Git(err)
    }
}

impl fmt::Display for Unresolved {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unresolved::OneSided => f.write_str("added or removed on one side"),
            Unresolved::NotInitialized => f.write_str("not initialized"),
            Unresolved::MissingCommit { which, id } => write!(
                f,
                "it lacks the {which} commit {}; fetch in it first",
                short_id(*id)
            ),
            Unresolved::Unrelated => f.write_str("the two sides share no history"),
            Unresolved::NoMatch { ours, incoming } => {
                let noun = if *incoming == 1 { "commit" } else { "commits" };
                write!(
                    f,
                    "no commit in it is {} with the {incoming} incoming {noun} replayed on top; rebase the submodule onto {} first",
                    short_id(*ours),
                    short_id(*ours)
                )
            }
            Unresolved::Ambiguous { ours, found } => {
                let found: Vec<String> = found.iter().map(|id| short_id(*id)).collect();
                write!(
                    f,
                    "several commits in it are {} with the incoming commits replayed on top ({})",
                    short_id(*ours),
                    found.join(", ")
                )
            }
            Unresolved::Checkout { id, why } => {
                write!(f, "it couldn't be checked out at {}: {why}", short_id(*id))
            }
            Unresolved::Git(err) => f.write_str(err.message()),
        }
    }
}

/// A gitlink in conflict: its path and the commits of its stages.
struct GitlinkConflict {
    path: String,
    ancestor: Option<Oid>,
    ours: Option<Oid>,
    theirs: Option<Oid>,
}

/// Resolve every submodule conflict of the repository's index that can
/// be (see the [module](self) documentation), checking each resolved
/// submodule out at its commit and staging it, and leaving the rest.
/// Conflicts in files are left alone. `progress` hears how the
/// submodules' checkouts go.
pub(super) fn resolve_submodule_conflicts(
    repo: &Repository,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Resolution, git2::Error> {
    let mut resolution = Resolution::default();
    for conflict in gitlink_conflicts(&repo.index()?)? {
        match resolve(repo, &conflict, progress) {
            Ok(()) => resolution.resolved.push(conflict.path),
            Err(why) => resolution.unresolved.push(SubmoduleConflict {
                path: conflict.path,
                why: why.to_string(),
            }),
        }
    }
    Ok(resolution)
}

/// The index's conflicts that are a submodule's: those with a gitlink
/// among their stages.
fn gitlink_conflicts(index: &Index) -> Result<Vec<GitlinkConflict>, git2::Error> {
    let mut found = Vec::new();
    for conflict in index.conflicts()? {
        let conflict = conflict?;
        let stages = [&conflict.ancestor, &conflict.our, &conflict.their];
        let Some(entry) = stages
            .iter()
            .filter_map(|stage| stage.as_ref())
            .find(|entry| entry.mode == GITLINK)
        else {
            continue;
        };
        // A stage that is a file, not a submodule, isn't a commit to
        // match against.
        let commit = |stage: &Option<IndexEntry>| {
            stage
                .as_ref()
                .filter(|entry| entry.mode == GITLINK)
                .map(|entry| entry.id)
        };
        found.push(GitlinkConflict {
            path: String::from_utf8_lossy(&entry.path).into_owned(),
            ancestor: commit(&conflict.ancestor),
            ours: commit(&conflict.our),
            theirs: commit(&conflict.their),
        });
    }
    Ok(found)
}

/// Resolve one submodule's conflict, or say why it can't be.
fn resolve(
    repo: &Repository,
    conflict: &GitlinkConflict,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<(), Unresolved> {
    let (Some(ancestor), Some(ours), Some(theirs)) =
        (conflict.ancestor, conflict.ours, conflict.theirs)
    else {
        return Err(Unresolved::OneSided);
    };
    let sub = open_submodule(repo, &conflict.path).ok_or(Unresolved::NotInitialized)?;
    for (which, id) in [("base", ancestor), ("ours", ours), ("theirs", theirs)] {
        if sub.find_commit(id).is_err() {
            return Err(Unresolved::MissingCommit { which, id });
        }
    }
    sub.merge_base(ours, theirs)
        .map_err(|_| Unresolved::Unrelated)?;
    let id = matching_commit(&sub, ancestor, ours, theirs)?;
    // The submodule first: a checkout that fails stages nothing.
    follow_gitlink(repo, &conflict.path, id, progress)
        .map_err(|why| Unresolved::Checkout { id, why })?;
    stage(repo, &conflict.path, id)?;
    Ok(())
}

/// The submodule commit that is `ours` with the commits `ancestor..theirs`
/// replayed on top, found by their messages.
fn matching_commit(
    sub: &Repository,
    ancestor: Oid,
    ours: Oid,
    theirs: Oid,
) -> Result<Oid, Unresolved> {
    let mut incoming = incoming_messages(sub, ancestor, ours, theirs)?;
    if incoming.is_empty() {
        // Theirs brings nothing ours hasn't got.
        return Ok(ours);
    }
    incoming.sort();
    let wanted: HashSet<&[u8]> = incoming.iter().map(Vec::as_slice).collect();

    let mut walk = sub.revwalk()?;
    for tip in ref_tips(sub)? {
        walk.push(tip)?;
    }
    walk.hide(ours)?;
    let mut found = BTreeSet::new();
    for id in walk {
        let id = id?;
        // Cheap first: a match's own message is one of the incoming
        // commits'.
        if !wanted.contains(sub.find_commit(id)?.message_raw_bytes()) {
            continue;
        }
        if !sub.graph_descendant_of(id, ours)? {
            continue;
        }
        let mut since = messages_between(sub, ours, id)?;
        since.sort();
        if since == incoming {
            found.insert(id);
        }
    }
    let mut found = found.into_iter();
    match (found.next(), found.next()) {
        (None, _) => Err(Unresolved::NoMatch {
            ours,
            incoming: incoming.len(),
        }),
        (Some(id), None) => Ok(id),
        (Some(first), Some(second)) => {
            let mut all = vec![first, second];
            all.extend(found);
            Err(Unresolved::Ambiguous { ours, found: all })
        }
    }
}

/// The raw messages of the commits theirs brings into the submodule
/// that ours hasn't got: those from the ancestor to theirs, less those
/// ours has (theirs was based on a commit upstream has since taken in)
/// and less those upstream took in as commits of its own with the same
/// message and patch (cherry-picked). Those are what a rebase of theirs
/// onto ours replays.
fn incoming_messages(
    repo: &Repository,
    ancestor: Oid,
    ours: Oid,
    theirs: Oid,
) -> Result<Vec<Vec<u8>>, git2::Error> {
    let mut walk = repo.revwalk()?;
    walk.push(theirs)?;
    walk.hide(ancestor)?;
    walk.hide(ours)?;
    let mut incoming = Vec::new();
    for id in walk {
        let id = id?;
        incoming.push((id, repo.find_commit(id)?.message_raw_bytes().to_vec()));
    }
    if incoming.is_empty() {
        return Ok(Vec::new());
    }
    // Upstream's commits since the ancestor that carry one of those
    // messages, with their patches: few, so few patches are worked out.
    let messages: HashSet<&[u8]> = incoming
        .iter()
        .map(|(_, message)| message.as_slice())
        .collect();
    let mut upstream: HashMap<Vec<u8>, Vec<Oid>> = HashMap::new();
    let mut walk = repo.revwalk()?;
    walk.push(ours)?;
    walk.hide(ancestor)?;
    for id in walk {
        let id = id?;
        let message = repo.find_commit(id)?.message_raw_bytes().to_vec();
        if messages.contains(message.as_slice()) {
            let patch = patch_id(repo, id)?;
            upstream.entry(message).or_default().push(patch);
        }
    }
    let mut kept = Vec::new();
    for (id, message) in incoming {
        if let Some(patches) = upstream.get(&message)
            && patches.contains(&patch_id(repo, id)?)
        {
            continue;
        }
        kept.push(message);
    }
    Ok(kept)
}

/// A commit's patch id: its change against its first parent, as an id
/// that commits making the same change share.
fn patch_id(repo: &Repository, id: Oid) -> Result<Oid, git2::Error> {
    let commit = repo.find_commit(id)?;
    let parent = match commit.parent(0) {
        Ok(parent) => Some(parent.tree()?),
        Err(_) => None,
    };
    repo.diff_tree_to_tree(parent.as_ref(), Some(&commit.tree()?), None)?
        .patchid(None)
}

/// The raw messages of the commits reachable from `include` but not
/// from `exclude`. Raw bytes, so that a message in no encoding in
/// particular still compares.
fn messages_between(
    repo: &Repository,
    exclude: Oid,
    include: Oid,
) -> Result<Vec<Vec<u8>>, git2::Error> {
    let mut walk = repo.revwalk()?;
    walk.push(include)?;
    walk.hide(exclude)?;
    let mut messages = Vec::new();
    for id in walk {
        messages.push(repo.find_commit(id?)?.message_raw_bytes().to_vec());
    }
    Ok(messages)
}

/// The commits the submodule's branches, its remotes' branches, and its
/// HEAD are at: where a brought-up-to-date branch would be. Tags are
/// left out, as they mark releases rather than work under way.
fn ref_tips(repo: &Repository) -> Result<Vec<Oid>, git2::Error> {
    let mut tips = BTreeSet::new();
    for reference in repo.references()? {
        let reference = reference?;
        let branch = reference
            .name()
            .is_ok_and(|name| name.starts_with("refs/heads/") || name.starts_with("refs/remotes/"));
        if branch && let Ok(commit) = reference.peel_to_commit() {
            tips.insert(commit.id());
        }
    }
    if let Ok(commit) = repo.head().and_then(|head| head.peel_to_commit()) {
        tips.insert(commit.id());
    }
    Ok(tips.into_iter().collect())
}

/// Stage the submodule at `path` at the commit `id`, clearing its
/// conflict.
fn stage(repo: &Repository, path: &str, id: Oid) -> Result<(), git2::Error> {
    let mut index = repo.index()?;
    index.remove_path(Path::new(path))?;
    index.add(&IndexEntry {
        ctime: IndexTime::new(0, 0),
        mtime: IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode: GITLINK,
        uid: 0,
        gid: 0,
        file_size: 0,
        id,
        flags: 0,
        flags_extended: 0,
        path: path.as_bytes().to_vec(),
    })?;
    index.write()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::history::tests::TestRepo;
    use crate::git::operation::tests::repo;
    use crate::git::{Integration, Outcome, Target};
    use git2::build::CheckoutBuilder;
    use git2::{RepositoryState, Signature, Time};
    use std::fs;

    /// A commit in `repo` on `parent` with `files` (top-level) written,
    /// and the submodule `sub` at `gitlink` when given, moving no
    /// reference. `time` tells apart commits that are otherwise alike.
    fn commit_on(
        repo: &Repository,
        parent: Oid,
        files: &[(&str, &str)],
        gitlink: Option<Oid>,
        message: &str,
        time: i64,
    ) -> Oid {
        let parent = repo.find_commit(parent).unwrap();
        let mut tree = repo.treebuilder(Some(&parent.tree().unwrap())).unwrap();
        for (name, content) in files {
            let blob = repo.blob(content.as_bytes()).unwrap();
            tree.insert(name, blob, 0o100644).unwrap();
        }
        if let Some(id) = gitlink {
            tree.insert("sub", id, GITLINK as i32).unwrap();
        }
        let tree = repo.find_tree(tree.write().unwrap()).unwrap();
        let sig = Signature::new("Ann Author", "ann@example.com", &Time::new(time, 0)).unwrap();
        repo.commit(None, &sig, &sig, message, &tree, &[&parent])
            .unwrap()
    }

    /// The workflow: the superproject's upstream (`side`) moved the
    /// submodule from A to B; the feature branch (HEAD's, `master`)
    /// moved it from A through each of `features` in turn (F1, F2, …),
    /// a superproject commit for each, changing `f.txt` to `feature`
    /// too when `clash` (as side does, so they conflict there too).
    /// HEAD and the submodule are where the feature branch has them.
    /// Returns the repository, the submodule, side's commit, B, and the
    /// submodule's feature commits.
    struct Workflow {
        t: TestRepo,
        sub: Repository,
        side: Oid,
        b: Oid,
        features: Vec<Oid>,
        /// How many of the feature's first submodule commits upstream
        /// has already, which rebasing the submodule leaves out.
        taken: usize,
    }

    fn workflow(features: &[&str], clash: bool) -> Workflow {
        workflow_with(features, clash, Upstream::Apart)
    }

    /// Whether upstream's submodule commit B has the first feature
    /// commit of the submodule already.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Upstream {
        /// No: B is on A.
        Apart,
        /// The very commit: B is on it, upstream having merged it.
        Merged,
        /// The same change by a commit of upstream's own, cherry-picked
        /// on A, that B is on.
        CherryPicked,
    }

    fn workflow_with(features: &[&str], clash: bool, upstream: Upstream) -> Workflow {
        let mut t = repo();
        t.commit(&[("f.txt", "base\n")], "Base", &[]);
        let sub = t.add_submodule("sub");
        let a = sub.head().unwrap().target().unwrap();
        let base = t.repo.head().unwrap().target().unwrap();

        let mut sub_tip = a;
        let mut ids = Vec::new();
        for (i, message) in features.iter().enumerate() {
            let file = format!("feature{i}.txt");
            sub_tip = commit_on(&sub, sub_tip, &[(&file, "feature\n")], None, message, 10);
            ids.push(sub_tip);
        }
        let below_b = match upstream {
            Upstream::Apart => a,
            Upstream::Merged => ids[0],
            Upstream::CherryPicked => commit_on(
                &sub,
                a,
                &[("feature0.txt", "feature\n")],
                None,
                features[0],
                5,
            ),
        };
        let b = commit_on(
            &sub,
            below_b,
            &[("up.txt", "up\n")],
            None,
            "Upstream work",
            1,
        );
        let side_files: &[(&str, &str)] = if clash { &[("f.txt", "side\n")] } else { &[] };
        let side = commit_on(&t.repo, base, side_files, Some(b), "Upstream bump", 1);
        t.repo
            .reference("refs/heads/side", side, true, "test")
            .unwrap();

        let mut tip = base;
        for (i, (message, sub_tip)) in features.iter().zip(&ids).enumerate() {
            let sub_tip = *sub_tip;
            let files: &[(&str, &str)] = if clash && i == 0 {
                &[("f.txt", "feature\n")]
            } else {
                &[]
            };
            tip = commit_on(
                &t.repo,
                tip,
                files,
                Some(sub_tip),
                &format!("Use {message}"),
                10,
            );
        }
        let head = t.repo.head().unwrap().name().unwrap().to_owned();
        t.repo.reference(&head, tip, true, "test").unwrap();
        t.repo
            .checkout_head(Some(CheckoutBuilder::new().force()))
            .unwrap();
        sub.reference("refs/heads/feature", sub_tip, true, "test")
            .unwrap();
        sub.set_head_detached(sub_tip).unwrap();
        sub.checkout_head(Some(CheckoutBuilder::new().force()))
            .unwrap();
        assert!(t.repo.statuses(None).unwrap().is_empty(), "clean to start");
        Workflow {
            t,
            sub,
            side,
            b,
            features: ids,
            taken: usize::from(upstream != Upstream::Apart),
        }
    }

    impl Workflow {
        /// Rebase the submodule's feature commits onto B, as the user
        /// does before rebasing the superproject, moving its `feature`
        /// branch to the last; `time` sets them apart from another such
        /// copy. Returns the rebased commits.
        fn rebase_submodule(&self, time: i64, branch: &str) -> Vec<Oid> {
            let mut tip = self.b;
            let mut ids = Vec::new();
            for (i, id) in self.features.iter().enumerate().skip(self.taken) {
                let message = self
                    .sub
                    .find_commit(*id)
                    .unwrap()
                    .message()
                    .unwrap()
                    .to_owned();
                let file = format!("feature{i}.txt");
                tip = commit_on(
                    &self.sub,
                    tip,
                    &[(&file, "feature\n")],
                    None,
                    &message,
                    time,
                );
                ids.push(tip);
            }
            self.sub
                .reference(&format!("refs/heads/{branch}"), tip, true, "test")
                .unwrap();
            ids
        }

        fn rebase(&self) -> Outcome {
            Integration::Rebase(Target::Commit(self.side))
                .run_with(
                    &self.t.repo,
                    &crate::git::hooks::tests::hooks(),
                    &mut |_, _| {},
                )
                .unwrap()
        }

        /// The submodule's commit as HEAD's commit records it.
        fn recorded(&self) -> Oid {
            let head = self.t.repo.head().unwrap().peel_to_tree().unwrap();
            head.get_name("sub").unwrap().id()
        }

        fn sub_head(&self) -> (Option<String>, Oid) {
            let head = self.sub.head().unwrap();
            let name = head
                .is_branch()
                .then(|| head.shorthand().unwrap().to_owned());
            (name, head.target().unwrap())
        }
    }

    #[test]
    fn a_rebase_whose_submodule_was_rebased_first_goes_through() {
        let w = workflow(&["Feature work"], false);
        let rebased = w.rebase_submodule(20, "feature");
        let outcome = w.rebase();
        assert_eq!(
            outcome,
            Outcome::Rebased {
                commits: 1,
                skipped: 0,
                submodules: 1,
                resolved: 1,
            }
        );
        assert_eq!(w.t.repo.state(), RepositoryState::Clean);
        let head = w.t.repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.parent_ids().collect::<Vec<_>>(), [w.side]);
        assert_eq!(w.recorded(), rebased[0]);
        // The submodule is checked out there, on its branch.
        assert_eq!(w.sub_head(), (Some("feature".to_owned()), rebased[0]));
        assert!(w.t.repo.statuses(None).unwrap().is_empty());
    }

    #[test]
    fn each_superproject_commit_gets_its_place_in_the_rebased_chain() {
        let w = workflow(&["Feature one", "Feature two"], false);
        let rebased = w.rebase_submodule(20, "feature");
        let outcome = w.rebase();
        assert!(
            matches!(
                outcome,
                Outcome::Rebased {
                    commits: 2,
                    resolved: 2,
                    ..
                }
            ),
            "{outcome:?}"
        );
        let head = w.t.repo.head().unwrap().peel_to_commit().unwrap();
        let first = head.parent(0).unwrap();
        assert_eq!(
            first.tree().unwrap().get_name("sub").unwrap().id(),
            rebased[0]
        );
        assert_eq!(w.recorded(), rebased[1]);
        assert_eq!(w.sub_head(), (Some("feature".to_owned()), rebased[1]));
    }

    #[test]
    fn incoming_commits_upstream_has_already_are_not_looked_for() {
        // The feature's first submodule commit is upstream's already, by
        // the very commit or cherry-picked, so rebasing the submodule
        // replays only the second, and the resolution is that.
        for upstream in [Upstream::Merged, Upstream::CherryPicked] {
            let w = workflow_with(&["Feature one", "Feature two"], false, upstream);
            let rebased = w.rebase_submodule(20, "feature");
            assert_eq!(rebased.len(), 1);
            let outcome = w.rebase();
            assert!(
                matches!(outcome, Outcome::Rebased { resolved, .. } if resolved >= 1),
                "{outcome:?}"
            );
            assert_eq!(w.recorded(), rebased[0]);
            assert_eq!(w.sub_head(), (Some("feature".to_owned()), rebased[0]));
        }
    }

    #[test]
    fn a_submodule_in_conflict_is_listed_once_with_both_sides_commits() {
        let w = workflow(&["Feature work"], false);
        assert!(matches!(w.rebase(), Outcome::Conflicts { .. }));
        let mut changes = crate::git::Changes::open(w.t.path()).unwrap();
        assert!(changes.wait(std::time::Duration::from_secs(10)));
        // Not also as an untracked directory, which the index has no
        // entry of its own for while it is in conflict.
        let listed: Vec<(char, &str)> = changes
            .unstaged()
            .iter()
            .map(|change| (change.kind.letter(), change.path.as_str()))
            .collect();
        assert_eq!(listed, [('U', "sub")]);
        // Its diff is the commits of the two sides, not a file.
        let diff = changes.unstaged_diff(&changes.unstaged()[0]).unwrap();
        let range = diff.submodule.unwrap();
        assert_eq!((range.old, range.new), (Some(w.b), Some(w.features[0])));
        assert!(range.error.is_none(), "{:?}", range.error);
        assert_eq!(range.commits.len(), 3, "B, the feature commit, and A");
    }

    #[test]
    fn a_submodule_not_rebased_first_is_left_in_conflict_saying_why() {
        let w = workflow(&["Feature work"], false);
        let outcome = w.rebase();
        let Outcome::Conflicts {
            files: 1,
            step: Some((1, 1)),
            unresolved,
        } = &outcome
        else {
            panic!("{outcome:?}");
        };
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].path, "sub");
        assert_eq!(
            unresolved[0].why,
            format!(
                "no commit in it is {} with the 1 incoming commit replayed on top; rebase the submodule onto {} first",
                short_id(w.b),
                short_id(w.b)
            )
        );
        assert_eq!(w.t.repo.state(), RepositoryState::RebaseMerge);
        assert!(w.t.repo.index().unwrap().has_conflicts());
        let summary = Integration::Rebase(Target::Branch {
            name: "side".to_owned(),
            remote: false,
        })
        .summary(Some("master"), &outcome);
        assert!(
            summary.ends_with(&format!(
                "; submodule sub left in conflict: {}",
                unresolved[0].why
            )),
            "{summary}"
        );
        // Continuing as it is says why again.
        let err = crate::git::continue_rebase(&w.t.repo, None).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "1 file is still in conflict; resolve and stage them first; submodule sub: {}",
                unresolved[0].why
            )
        );
        // Rebasing the submodule then, as it says, and continuing goes
        // through.
        let rebased = w.rebase_submodule(20, "feature");
        let outcome = crate::git::continue_rebase(&w.t.repo, None).unwrap();
        assert_eq!(
            outcome,
            Outcome::Rebased {
                commits: 1,
                skipped: 0,
                submodules: 1,
                resolved: 1,
            }
        );
        assert_eq!(w.recorded(), rebased[0]);
        assert_eq!(w.t.repo.state(), RepositoryState::Clean);
    }

    #[test]
    fn a_submodule_left_in_conflict_is_resolved_by_taking_a_side() {
        use crate::git::{Changes, ConflictSide};
        let w = workflow(&["Feature work"], false);
        let Outcome::Conflicts { unresolved, .. } = w.rebase() else {
            panic!("a conflict");
        };
        assert_eq!(unresolved.len(), 1);
        let mut changes = Changes::open(w.t.path()).unwrap();
        assert!(changes.wait(std::time::Duration::from_secs(10)));
        assert_eq!(changes.conflict_count(), 1);
        // Ours, in a rebase, is upstream's B: the submodule is checked
        // out there and staged.
        changes.resolve(["sub"], ConflictSide::Ours).unwrap();
        assert_eq!(changes.conflict_count(), 0);
        // Written by the page's own handle on the repository.
        let mut index = w.t.repo.index().unwrap();
        index.read(true).unwrap();
        assert!(!index.has_conflicts());
        assert_eq!(index.get_path(Path::new("sub"), 0).unwrap().id, w.b);
        assert_eq!(w.sub_head().1, w.b);
        // The commit being replayed changed nothing else, so taking
        // ours leaves it empty, and it is dropped as git drops one.
        let outcome = crate::git::continue_rebase(&w.t.repo, None).unwrap();
        assert!(
            matches!(
                outcome,
                Outcome::Rebased {
                    commits: 0,
                    skipped: 1,
                    ..
                }
            ),
            "{outcome:?}"
        );
        assert_eq!(w.recorded(), w.b);
    }

    #[test]
    fn two_rebased_copies_are_ambiguous() {
        let w = workflow(&["Feature work"], false);
        let one = w.rebase_submodule(20, "feature");
        let two = w.rebase_submodule(30, "feature-again");
        let Outcome::Conflicts { unresolved, .. } = w.rebase() else {
            panic!("a conflict");
        };
        let mut both = [short_id(one[0]), short_id(two[0])];
        both.sort();
        assert_eq!(
            unresolved[0].why,
            format!(
                "several commits in it are {} with the incoming commits replayed on top ({}, {})",
                short_id(w.b),
                both[0],
                both[1]
            )
        );
        // The submodule wasn't moved for it.
        assert_eq!(w.sub_head().1, w.features[0]);
    }

    #[test]
    fn a_file_conflict_alongside_still_has_the_submodule_resolved() {
        let w = workflow(&["Feature work"], true);
        let rebased = w.rebase_submodule(20, "feature");
        let outcome = w.rebase();
        assert_eq!(
            outcome,
            Outcome::Conflicts {
                files: 1,
                step: Some((1, 1)),
                unresolved: Vec::new(),
            }
        );
        let index = w.t.repo.index().unwrap();
        let staged = index.get_path(Path::new("sub"), 0).unwrap();
        assert_eq!(staged.id, rebased[0]);
        assert_eq!(w.sub_head().1, rebased[0]);
        // Resolving the file and continuing finishes with it.
        fs::write(w.t.path().join("f.txt"), "both\n").unwrap();
        let mut index = w.t.repo.index().unwrap();
        index.add_path(Path::new("f.txt")).unwrap();
        index.write().unwrap();
        let outcome = crate::git::continue_rebase(&w.t.repo, None).unwrap();
        assert!(
            matches!(outcome, Outcome::Rebased { commits: 1, .. }),
            "{outcome:?}"
        );
        assert_eq!(w.recorded(), rebased[0]);
    }

    #[test]
    fn a_merge_of_the_feature_whose_submodule_was_rebased_first_commits() {
        // Merging the feature into upstream: ours is upstream's side, as
        // in a rebase, and theirs brings the feature work.
        let w = workflow(&["Feature work"], false);
        let rebased = w.rebase_submodule(20, "feature");
        let feature = w.t.repo.head().unwrap().target().unwrap();
        w.t.repo.set_head("refs/heads/side").unwrap();
        w.t.repo
            .checkout_head(Some(CheckoutBuilder::new().force()))
            .unwrap();
        w.sub.set_head_detached(w.b).unwrap();
        w.sub
            .checkout_head(Some(CheckoutBuilder::new().force()))
            .unwrap();
        let outcome = Integration::Merge(Target::Commit(feature))
            .run_with(
                &w.t.repo,
                &crate::git::hooks::tests::hooks(),
                &mut |_, _| {},
            )
            .unwrap();
        let Outcome::Merged {
            commit,
            resolved: 1,
            ..
        } = outcome
        else {
            panic!("{outcome:?}");
        };
        assert_eq!(w.t.repo.state(), RepositoryState::Clean);
        let merged = w.t.repo.find_commit(commit).unwrap();
        assert_eq!(merged.parent_count(), 2);
        // No conflict notes left in the message.
        let message = merged.message().unwrap();
        assert!(!message.contains('#'), "{message:?}");
        assert!(message.starts_with("Merge commit"), "{message:?}");
        assert_eq!(w.recorded(), rebased[0]);
        assert!(w.t.repo.statuses(None).unwrap().is_empty());
    }
}
