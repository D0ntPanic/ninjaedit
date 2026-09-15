//! The submodules of a repository, for showing each one's history on
//! its own tab of the git log page, and each one's uncommitted changes
//! on its own tab of the changes page.
//!
//! [`submodules`] lists every submodule the repository declares, and
//! within each one that has been initialized (its working directory
//! holds a repository), its own submodules, so nested submodules are
//! listed with their paths joined. A submodule that hasn't been
//! initialized is listed too, so the user can see that it exists; its
//! working directory is not a repository, and opening it with
//! [`History::open_repository`](super::History::open_repository) fails
//! and says so.
//!
//! [`changed_submodules`] lists only the initialized ones with
//! uncommitted changes of their own, staged or not, which are the ones
//! there is something to do in. Changes nest: a submodule whose only
//! change is that a submodule of its own has changes counts, since its
//! own working tree shows that submodule as modified, and the changes
//! have to be committed from the bottom up.
//!
//! A change to a submodule, in a commit or the working tree, is a
//! change of the commit it points at, and what a reader wants to know
//! is which of the submodule's commits that moved over.
//! [`submodule_range`] lists them from the submodule's own repository,
//! laid out as a graph like the log's: the commits reachable from the
//! new commit but not the old one, and, when the submodule went
//! backwards or sideways, those reachable from the old but not the
//! new, down to and including the commit(s) they have in common, so the
//! graph joins up. A submodule added or removed shows just the one
//! commit it points at.

use super::graph::GraphLayout;
use super::history::{Commit, Oid, PendingCommit, collect_refs, short_id};
use git2::{ErrorCode, Repository, Sort, StatusOptions};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// How many of a submodule's commits a change lists, at most: a
/// submodule bumped by years of history is shown from the top.
pub const RANGE_LIMIT: usize = 1000;

/// A submodule of a repository, or of one of its submodules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submodule {
    /// The submodule's path from the top repository's working
    /// directory, with `/` between the parts.
    pub path: String,
    /// Where its working directory is.
    pub workdir: PathBuf,
    /// Whether the working directory holds a repository.
    pub initialized: bool,
}

/// The submodules of the repository containing `path`, nested ones
/// included, in path order. Empty when there is no repository.
pub fn submodules(path: &Path) -> Vec<Submodule> {
    let mut found = Vec::new();
    if let Ok(repo) = Repository::discover(path) {
        collect(&repo, "", &mut found, false);
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// The submodules of `repo` with uncommitted changes, nested ones
/// included, in path order.
pub fn changed_submodules(repo: &Repository) -> Vec<Submodule> {
    let mut found = Vec::new();
    collect(repo, "", &mut found, true);
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

fn collect(repo: &Repository, prefix: &str, found: &mut Vec<Submodule>, only_changed: bool) {
    let Some(workdir) = repo.workdir() else {
        return;
    };
    let Ok(submodules) = repo.submodules() else {
        return;
    };
    for submodule in submodules {
        let relative = submodule.path().to_string_lossy().replace('\\', "/");
        let path = format!("{prefix}{relative}");
        let workdir = workdir.join(submodule.path());
        let inner = Repository::open(&workdir).ok();
        let wanted = !only_changed || inner.as_ref().is_some_and(has_uncommitted_changes);
        if wanted {
            found.push(Submodule {
                path: path.clone(),
                workdir,
                initialized: inner.is_some(),
            });
        }
        if let Some(inner) = inner {
            collect(&inner, &format!("{path}/"), found, only_changed);
        }
    }
}

/// Whether anything in a repository's working tree or index differs
/// from HEAD: a change to stage or commit, an untracked file, or a
/// conflict. A submodule of it with changes of its own counts, as it
/// does for `git status`.
pub fn has_uncommitted_changes(repo: &Repository) -> bool {
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(false)
        .include_ignored(false)
        .exclude_submodules(false);
    match repo.statuses(Some(&mut options)) {
        Ok(statuses) => statuses
            .iter()
            .any(|entry| !entry.status().is_empty() && !entry.status().is_ignored()),
        Err(_) => false,
    }
}

/// The commits a change to a submodule spans: from the commit it
/// pointed at before to the one it points at now, laid out as a graph
/// (see the [module's](self) documentation for which commits).
#[derive(Debug)]
pub struct SubmoduleRange {
    /// The commit the submodule pointed at before the change, or `None`
    /// when it was added (or wasn't a submodule).
    pub old: Option<Oid>,
    /// The commit it points at after, or `None` when it was removed.
    pub new: Option<Oid>,
    /// The commits in the range, newest first, each with its place in
    /// the graph; empty when they couldn't be listed (see `error`) or
    /// when `old` and `new` are the same commit (the submodule's own
    /// working tree changed, not the commit it is at).
    pub commits: Vec<Commit>,
    /// The widest graph row, in lanes.
    pub max_lanes: usize,
    /// Whether the range was cut short at [`RANGE_LIMIT`] commits.
    pub truncated: bool,
    /// Why the commits couldn't be listed, when they couldn't: the
    /// submodule isn't initialized, or one of the commits isn't in it.
    pub error: Option<String>,
    /// The commits (of `old` and `new`) the submodule's repository
    /// doesn't have, which a fetch of it may bring.
    pub missing: Vec<Oid>,
}

/// The commits the submodule at `path` (in `parent`'s working
/// directory) moved over from `old` to `new`, from the submodule's own
/// repository. Never fails: what went wrong is in the range's `error`.
pub fn submodule_range(
    parent: &Repository,
    path: &str,
    old: Option<Oid>,
    new: Option<Oid>,
) -> SubmoduleRange {
    let mut range = SubmoduleRange {
        old,
        new,
        commits: Vec::new(),
        max_lanes: 0,
        truncated: false,
        error: None,
        missing: Vec::new(),
    };
    if old == new {
        return range;
    }
    let mut missing = Vec::new();
    let outcome = open_submodule(parent, path).and_then(|repo| {
        missing.extend(
            [old, new]
                .into_iter()
                .flatten()
                .filter(|id| repo.find_commit(*id).is_err()),
        );
        if !missing.is_empty() {
            let names: Vec<String> = missing.iter().map(|id| short_id(*id)).collect();
            let (noun, verb) = if names.len() == 1 {
                ("Commit", "is")
            } else {
                ("Commits", "are")
            };
            return Err(git2::Error::from_str(&format!(
                "{noun} {} {verb} not in the submodule's repository (not fetched?)",
                names.join(" and ")
            )));
        }
        walk_range(&repo, old, new, RANGE_LIMIT)
    });
    range.missing = missing;
    match outcome {
        Ok((commits, truncated)) => {
            range.max_lanes = commits
                .iter()
                .map(|commit| commit.graph.width())
                .max()
                .unwrap_or(0);
            range.commits = commits;
            range.truncated = truncated;
        }
        Err(err) => range.error = Some(err.message().to_owned()),
    }
    range
}

/// The repository of the submodule at `path`: as the parent knows it,
/// or failing that whatever repository is at that path (a submodule the
/// parent no longer declares).
fn open_submodule(parent: &Repository, path: &str) -> Result<Repository, git2::Error> {
    if let Ok(submodule) = parent.find_submodule(path)
        && let Ok(repo) = submodule.open()
    {
        return Ok(repo);
    }
    let workdir = parent
        .workdir()
        .ok_or_else(|| git2::Error::from_str("the repository has no working tree"))?;
    Repository::open(workdir.join(path))
        .map_err(|_| git2::Error::from_str("Submodule not initialized: its commits can't be shown"))
}

/// Walk the commits between `old` and `new` in `repo`, newest first,
/// at most `limit` of them, and lay them out. The graph is of the range
/// alone: a parent outside it isn't drawn.
fn walk_range(
    repo: &Repository,
    old: Option<Oid>,
    new: Option<Oid>,
    limit: usize,
) -> Result<(Vec<Commit>, bool), git2::Error> {
    let mut revwalk = repo.revwalk()?;
    revwalk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
    match (old, new) {
        (Some(old), Some(new)) => {
            revwalk.push(new)?;
            revwalk.push(old)?;
            // Down to the commits they have in common, which stay in;
            // unrelated histories have none, and both are walked whole.
            match repo.merge_bases(old, new) {
                Ok(bases) => {
                    for base in bases.iter() {
                        for parent in repo.find_commit(*base)?.parent_ids() {
                            revwalk.hide(parent)?;
                        }
                    }
                }
                Err(err) if err.code() == ErrorCode::NotFound => {}
                Err(err) => return Err(err),
            }
        }
        (Some(only), None) | (None, Some(only)) => {
            let commit = repo.find_commit(only)?;
            revwalk.push(only)?;
            for parent in commit.parent_ids() {
                revwalk.hide(parent)?;
            }
        }
        (None, None) => return Ok((Vec::new(), false)),
    }

    // Every commit first, so that each one's parents can be limited to
    // the range before it is laid out.
    let mut ids = Vec::new();
    let mut truncated = false;
    for id in revwalk {
        if ids.len() >= limit {
            truncated = true;
            break;
        }
        ids.push(id?);
    }
    let in_range: HashSet<Oid> = ids.iter().copied().collect();
    let (_, _, mut labels) = collect_refs(repo, None)?;
    let mut layout = GraphLayout::new();
    let mut pending: Option<PendingCommit> = None;
    let mut commits = Vec::with_capacity(ids.len());
    for id in ids {
        let commit = repo.find_commit(id)?;
        let mut info = PendingCommit::read(&commit, labels.remove(&id).unwrap_or_default());
        info.parents.retain(|parent| in_range.contains(parent));
        if let Some(row) = layout.push(id, &info.parents) {
            let done = pending.take().expect("a row finishes a pushed commit");
            commits.push(done.finish(row));
        }
        pending = Some(info);
    }
    if let (Some(row), Some(done)) = (layout.finish(), pending.take()) {
        commits.push(done.finish(row));
    }
    Ok((commits, truncated))
}

#[cfg(test)]
mod tests {
    use super::super::history::tests::{TestRepo, add_submodule_to};
    use super::*;

    #[test]
    fn submodules_are_listed_with_nesting_and_initialization() {
        let mut repo = TestRepo::new();
        repo.commit(&[("a.txt", "one\n")], "Base", &[]);
        let sub = repo.add_submodule("libs/sub");
        // A submodule of the submodule, and one never initialized.
        add_submodule_to(&sub, "deep");
        repo.add_uninitialized_submodule("vendor/missing");
        let listed = submodules(repo.path());
        let names: Vec<(&str, bool)> = listed
            .iter()
            .map(|s| (s.path.as_str(), s.initialized))
            .collect();
        assert_eq!(
            names,
            [
                ("libs/sub", true),
                ("libs/sub/deep", true),
                ("vendor/missing", false)
            ]
        );
        // Canonicalized: libgit2 resolves symlinks in the working
        // directory's path (macOS's `/var` is one).
        assert_eq!(
            std::fs::canonicalize(&listed[0].workdir).unwrap(),
            std::fs::canonicalize(repo.path().join("libs/sub")).unwrap()
        );
        assert!(submodules(&std::env::temp_dir().join("no-such-repo-here")).is_empty());
    }

    /// Commit a file in `repo` on top of its HEAD, on HEAD's branch.
    fn commit_in(repo: &Repository, name: &str, content: &str, message: &str) -> Oid {
        std::fs::write(repo.workdir().unwrap().join(name), content).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(name)).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("Sub Author", "sub@example.com").unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&head])
            .unwrap()
    }

    /// Stage the submodule at `path` as it is and commit that on the
    /// parent's HEAD.
    fn commit_gitlink(parent: &Repository, path: &str, message: &str) -> Oid {
        let mut index = parent.index().unwrap();
        index.add_path(Path::new(path)).unwrap();
        index.write().unwrap();
        let tree = parent.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("Test Author", "test@example.com").unwrap();
        let head = parent.head().unwrap().peel_to_commit().unwrap();
        parent
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &[&head])
            .unwrap()
    }

    fn missing2() -> Oid {
        Oid::from_str("fedcba9876543210fedcba9876543210fedcba98").unwrap()
    }

    fn summaries(range: &SubmoduleRange) -> Vec<&str> {
        range.commits.iter().map(|c| c.summary.as_str()).collect()
    }

    #[test]
    fn a_submodule_change_lists_the_commits_it_moved_over_as_a_graph() {
        let mut repo = TestRepo::new();
        repo.commit(&[("a.txt", "one\n")], "Base", &[]);
        let sub = repo.add_submodule("libs/sub");
        let s1 = sub.head().unwrap().target().unwrap();
        let s2 = commit_in(&sub, "inner.txt", "two\n", "Inner two");
        let s3 = commit_in(&sub, "inner.txt", "three\n", "Inner three");
        sub.branch("topic", &sub.find_commit(s1).unwrap(), false)
            .unwrap();

        // Forward: the new commit down to the old one, which stays in.
        let range = submodule_range(&repo.repo, "libs/sub", Some(s1), Some(s3));
        assert_eq!(range.error, None);
        assert_eq!(
            summaries(&range),
            ["Inner three", "Inner two", "Inner commit"]
        );
        assert_eq!((range.old, range.new), (Some(s1), Some(s3)));
        assert_eq!(range.max_lanes, 1);
        assert!(!range.truncated);
        // The submodule's own references label its commits.
        let labels: Vec<&str> = range.commits[2]
            .refs
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(labels, ["topic"]);
        assert!(range.commits[0].refs.iter().all(|r| !r.is_head));
        // Backward: the same commits.
        let range = submodule_range(&repo.repo, "libs/sub", Some(s3), Some(s1));
        assert_eq!(summaries(&range).len(), 3);

        // Added or removed: just the one commit, its parents outside
        // the range and so not drawn.
        let range = submodule_range(&repo.repo, "libs/sub", None, Some(s3));
        assert_eq!(summaries(&range), ["Inner three"]);
        assert!(range.commits[0].parents.is_empty());
        assert!(range.commits[0].graph.edges.is_empty());
        let range = submodule_range(&repo.repo, "libs/sub", Some(s2), None);
        assert_eq!(summaries(&range), ["Inner two"]);

        // Sideways: both sides down to the commit they share, in two
        // lanes.
        sub.set_head("refs/heads/topic").unwrap();
        sub.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        let t1 = commit_in(&sub, "topic.txt", "t\n", "Topic");
        let range = submodule_range(&repo.repo, "libs/sub", Some(s3), Some(t1));
        let mut names = summaries(&range);
        assert_eq!(names[3], "Inner commit");
        names.sort_unstable();
        assert_eq!(names, ["Inner commit", "Inner three", "Inner two", "Topic"]);
        assert_eq!(range.max_lanes, 2);

        // The same commit on both sides is no range at all.
        let range = submodule_range(&repo.repo, "libs/sub", Some(s1), Some(s1));
        assert!(range.commits.is_empty());
        assert_eq!(range.error, None);

        // A commit the submodule doesn't have, and a submodule that
        // isn't initialized, say so.
        let missing = Oid::from_str("0123456789abcdef0123456789abcdef01234567").unwrap();
        let range = submodule_range(&repo.repo, "libs/sub", Some(s1), Some(missing));
        assert!(range.commits.is_empty());
        assert_eq!(
            range.error.as_deref(),
            Some("Commit 01234567 is not in the submodule's repository (not fetched?)")
        );
        assert_eq!(range.missing, [missing]);
        let range = submodule_range(&repo.repo, "libs/sub", Some(missing), Some(missing2()));
        assert_eq!(range.missing.len(), 2);
        assert!(
            range
                .error
                .as_deref()
                .unwrap()
                .starts_with("Commits 01234567 and "),
            "{range:?}"
        );
        repo.add_uninitialized_submodule("vendor/missing");
        let range = submodule_range(&repo.repo, "vendor/missing", Some(s1), Some(s3));
        assert_eq!(
            range.error.as_deref(),
            Some("Submodule not initialized: its commits can't be shown")
        );

        // A long range is cut short from the top.
        let (commits, truncated) = walk_range(&sub, Some(s1), Some(s3), 2).unwrap();
        assert!(truncated);
        let names: Vec<&str> = commits.iter().map(|c| c.summary.as_str()).collect();
        assert_eq!(names, ["Inner three", "Inner two"]);
        assert!(
            commits[1].parents.is_empty(),
            "the cut-off parent isn't drawn"
        );

        // A commit's diff of the submodule carries the range.
        sub.set_head_detached(s3).unwrap();
        sub.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        let p2 = commit_gitlink(&repo.repo, "libs/sub", "Bump submodule");
        let history = super::super::history::tests::open(&repo);
        let diff = history.file_diff(p2, "libs/sub", None).unwrap();
        assert_eq!(diff.unshown, Some(super::super::diff::Unshown::Submodule));
        let range = diff.submodule.as_ref().unwrap();
        assert_eq!((range.old, range.new), (Some(s1), Some(s3)));
        assert_eq!(
            summaries(range),
            ["Inner three", "Inner two", "Inner commit"]
        );
        // And of the commit that added it: the one commit.
        let added = history
            .commits()
            .iter()
            .find(|c| c.summary == "Add submodule libs/sub")
            .unwrap()
            .id;
        let diff = history.file_diff(added, "libs/sub", None).unwrap();
        let range = diff.submodule.as_ref().unwrap();
        assert_eq!((range.old, range.new), (None, Some(s1)));
        assert_eq!(summaries(range), ["Inner commit"]);
    }
}
