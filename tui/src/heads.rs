//! The status bar's branch: where HEAD is in the repository the user
//! is working in, read from the repository as needed and read again
//! every so often, since a checkout, a commit, or git run in a shell
//! moves it.
//!
//! Which repository that is depends on where the user is (the
//! application decides; see `App::status_repository`): the deepest
//! one containing the file being edited, which is the submodule the
//! file is in if it is in one; the repository of the shown tab of the
//! git log or changes page; or otherwise, on a page that isn't about
//! the files or the repository, with a tool focused, or with nothing
//! open, the project's own. Each is read once and remembered until the
//! next look, so a redraw doesn't open a repository.

use ninjaedit_core::git::{Head, head};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long between looks at the repository.
const LOOK_INTERVAL: Duration = Duration::from_secs(2);

/// A repository as the pages open one: a directory, and whether the
/// repository is exactly that directory (a submodule's) rather than
/// whatever repository contains it.
pub type Repository = (PathBuf, bool);

/// Where HEAD is in each repository asked about since the last look.
pub struct Heads {
    known: HashMap<Repository, Option<Head>>,
    looked: Instant,
}

impl Heads {
    pub fn new() -> Heads {
        Heads {
            known: HashMap::new(),
            looked: Instant::now(),
        }
    }

    /// Where HEAD is in a repository: as remembered, or read now.
    pub fn head(&mut self, repository: Repository) -> Option<&Head> {
        self.known
            .entry(repository)
            .or_insert_with_key(|(path, exact)| head(path, *exact))
            .as_ref()
    }

    /// Look again when it's time; see [`look`](Self::look).
    pub fn look_if_due(&mut self, current: Repository) -> bool {
        if self.looked.elapsed() < LOOK_INTERVAL {
            return false;
        }
        self.look(current)
    }

    /// Forget what was remembered and read the repository the status
    /// bar is showing again. Returns whether its HEAD moved, which
    /// calls for a redraw.
    pub fn look(&mut self, current: Repository) -> bool {
        self.looked = Instant::now();
        let before = self.known.remove(&current).flatten();
        self.known.clear();
        self.head(current) != before.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{Repository as GitRepository, Signature};
    use std::path::Path;

    #[test]
    fn remembers_until_a_look_and_notices_a_move() {
        let dir = tempfile::tempdir().unwrap();
        let repo = GitRepository::init(dir.path()).unwrap();
        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("a.txt")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = Signature::now("Ann", "ann@example.com").unwrap();
        let first = repo
            .commit(Some("HEAD"), &sig, &sig, "first", &tree, &[])
            .unwrap();
        let key = (dir.path().to_path_buf(), false);
        let mut heads = Heads::new();
        let branch = heads.head(key.clone()).cloned();
        assert!(matches!(branch, Some(Head::Branch(_))), "{branch:?}");

        // Detaching HEAD isn't seen until a look, which reports it.
        repo.set_head_detached(first).unwrap();
        assert_eq!(heads.head(key.clone()).cloned(), branch);
        assert!(!heads.look_if_due(key.clone()), "not due yet");
        assert!(heads.look(key.clone()));
        assert!(matches!(heads.head(key.clone()), Some(Head::Commit(_))));
        // Nothing moved: no redraw.
        assert!(!heads.look(key.clone()));
        // A directory that isn't in a repository has no HEAD.
        assert_eq!(
            heads.head((PathBuf::from("/nonexistent/nowhere"), false)),
            None
        );
    }
}
