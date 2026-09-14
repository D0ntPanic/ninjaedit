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

use git2::{Repository, StatusOptions};
use std::path::{Path, PathBuf};

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
}
