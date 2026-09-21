//! Where a repository's HEAD is, for the status bar: the branch it is
//! on, or, when detached, the commit it points at. A submodule's is
//! asked for as exactly its directory, as the pages open one, so that
//! an uninitialized submodule (whose directory lies inside its
//! parent's repository) doesn't answer with the parent's branch.

use super::history::short_id;
use git2::Repository;
use std::path::Path;

/// What HEAD points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Head {
    /// The local branch HEAD is on, by its short name. A branch with no
    /// commits yet (a fresh repository's) counts: HEAD names it.
    Branch(String),
    /// HEAD is detached: the first characters of the commit it is on,
    /// as [`short_id`] has them.
    Commit(String),
}

impl Head {
    /// The text to show.
    pub fn label(&self) -> &str {
        match self {
            Head::Branch(text) | Head::Commit(text) => text,
        }
    }
}

/// HEAD of the repository containing `path`, or, with `exact`, of the
/// repository whose working directory is `path` itself. `None` when
/// there is no such repository, or it has no HEAD to speak of.
pub fn head(path: &Path, exact: bool) -> Option<Head> {
    let repo = if exact {
        Repository::open(path).ok()?
    } else {
        Repository::discover(path).ok()?
    };
    head_of(&repo)
}

/// HEAD of an opened repository.
pub fn head_of(repo: &Repository) -> Option<Head> {
    if repo.head_detached().unwrap_or(false) {
        let id = repo.head().ok()?.target()?;
        return Some(Head::Commit(short_id(id)));
    }
    // HEAD is symbolic: a branch, born or not. `Repository::head`
    // fails on an unborn one, so read the reference itself.
    let reference = repo.find_reference("HEAD").ok()?;
    let target = reference.symbolic_target().ok()??;
    let name = target.strip_prefix("refs/heads/").unwrap_or(target);
    Some(Head::Branch(name.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::history::tests::{TestRepo, add_submodule_to};
    use crate::git::short_id;

    #[test]
    fn a_branch_by_name_and_a_detached_head_by_commit() {
        let mut repo = TestRepo::new();
        // A fresh repository is on its unborn default branch.
        let unborn = head(repo.path(), false).unwrap();
        assert!(matches!(unborn, Head::Branch(_)), "{unborn:?}");
        let first = repo.commit(&[("a.txt", "a\n")], "first", &[]);
        repo.branch("feature", first);
        repo.checkout("feature");
        assert_eq!(
            head(repo.path(), false),
            Some(Head::Branch("feature".to_owned()))
        );
        assert_eq!(head(repo.path(), false).unwrap().label(), "feature");
        repo.repo.set_head_detached(first).unwrap();
        assert_eq!(
            head(repo.path(), false),
            Some(Head::Commit(short_id(first)))
        );
        assert_eq!(short_id(first).len(), 8);
    }

    #[test]
    fn a_file_inside_the_repository_finds_it_and_an_exact_open_does_not_look_up() {
        let mut repo = TestRepo::new();
        repo.commit(&[("dir/a.txt", "a\n")], "first", &[]);
        assert_eq!(
            head(&repo.path().join("dir"), false),
            Some(Head::Branch("master".to_owned()))
        );
        let sub = add_submodule_to(&repo.repo, "sub");
        let sub_head = head(sub.workdir().unwrap(), true).unwrap();
        assert!(matches!(sub_head, Head::Branch(_)), "{sub_head:?}");
        // A directory that isn't a repository of its own answers
        // nothing when asked exactly, though its parent contains it.
        assert_eq!(head(&repo.path().join("dir"), true), None);
        assert_eq!(head(Path::new("/nonexistent/nowhere"), false), None);
    }
}
