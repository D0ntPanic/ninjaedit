//! Restoring files of the working directory as a commit has them, as
//! `git restore --source=<commit> -- <paths>` does: for going back to
//! a version of a file seen in the log, or undoing what a commit did to
//! it by restoring it as the commit's parent has it.
//!
//! Only the working directory changes. The index is left alone, so a
//! restored file shows as an unstaged change on the changes page, to be
//! reviewed and then staged, or discarded to put the file back as it
//! was; HEAD doesn't move. A path the source commit has is written as
//! the commit has it (its contents, and whether it is executable or a
//! symbolic link); a path it doesn't have is deleted, along with any
//! directories that leaves empty, since that is how the commit has it.
//! With no source at all (restoring as before a root commit) every path
//! is deleted.
//!
//! Restoring overwrites what the working directory has, which is only
//! lost if it isn't in the index: [`unstaged_among`] says which paths
//! have unstaged changes (or are untracked) that restoring them would
//! lose, for a frontend to ask about first.
//!
//! A submodule isn't restored here: its "contents" are the commit
//! checked out in it, which is the submodule's own business, and
//! restoring refuses one, changing nothing.

use super::changes::remove_empty_parents;
use git2::build::CheckoutBuilder;
use git2::{DiffOptions, FileMode, ObjectType, Oid, Repository, Tree};
use std::fs;
use std::path::Path;

/// What [`restore`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Restored {
    /// How many paths were written as the source has them.
    pub written: usize,
    /// How many paths the source doesn't have were deleted (those
    /// already missing aren't counted).
    pub deleted: usize,
}

/// Open the repository whose git directory is `git_dir`, which must
/// have a working directory.
fn open(git_dir: &Path) -> Result<Repository, git2::Error> {
    let repo = Repository::open(git_dir)?;
    if repo.workdir().is_none() {
        return Err(git2::Error::from_str("the repository has no working tree"));
    }
    Ok(repo)
}

/// The paths among `paths` whose working-directory contents aren't in
/// the index, and so would be lost to a restore: files with unstaged
/// changes, and untracked files. A file deleted from the working
/// directory has nothing to lose, so it isn't one. In path order.
pub fn unstaged_among(git_dir: &Path, paths: &[&str]) -> Result<Vec<String>, git2::Error> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let repo = open(git_dir)?;
    let mut options = DiffOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_typechange(true)
        .disable_pathspec_match(true);
    for path in paths {
        options.pathspec(path);
    }
    let diff = repo.diff_index_to_workdir(None, Some(&mut options))?;
    Ok(diff
        .deltas()
        .filter(|delta| delta.status() != git2::Delta::Deleted)
        .filter_map(|delta| {
            let file = if delta.new_file().path().is_some() {
                delta.new_file()
            } else {
                delta.old_file()
            };
            file.path().map(|p| p.to_string_lossy().into_owned())
        })
        .collect())
}

/// Restore `paths` in the working directory as the commit `source` has
/// them, or with no source, delete them. Refused, with nothing changed,
/// for a path that is a submodule (in the source or the index) or a
/// directory in the source: the paths are files.
pub fn restore(
    git_dir: &Path,
    source: Option<Oid>,
    paths: &[&str],
) -> Result<Restored, git2::Error> {
    let repo = open(git_dir)?;
    let workdir = repo.workdir().expect("checked on opening").to_path_buf();
    let tree: Option<Tree<'_>> = match source {
        Some(id) => Some(repo.find_commit(id)?.tree()?),
        None => None,
    };
    let index = repo.index()?;
    let mut write = Vec::new();
    let mut delete = Vec::new();
    for &path in paths {
        let in_index = index
            .get_path(Path::new(path), 0)
            .is_some_and(|entry| entry.mode == u32::from(FileMode::Commit));
        let entry = match &tree {
            Some(tree) => match tree.get_path(Path::new(path)) {
                Ok(entry) => Some(entry),
                Err(err) if err.code() == git2::ErrorCode::NotFound => None,
                Err(err) => return Err(err),
            },
            None => None,
        };
        let submodule = in_index
            || entry
                .as_ref()
                .is_some_and(|e| e.kind() == Some(ObjectType::Commit));
        if submodule {
            return Err(git2::Error::from_str(&format!(
                "{path} is a submodule: check out its commit on its own tab"
            )));
        }
        match entry {
            Some(entry) if entry.kind() == Some(ObjectType::Tree) => {
                return Err(git2::Error::from_str(&format!(
                    "{path} is a directory in that commit"
                )));
            }
            Some(_) => write.push(path),
            None => delete.push(path),
        }
    }
    let mut restored = Restored::default();
    // An empty list of paths would check out every file.
    if let (Some(tree), false) = (&tree, write.is_empty()) {
        let mut checkout = CheckoutBuilder::new();
        // Written over whatever is there, and the index left as it is.
        checkout
            .force()
            .update_index(false)
            .disable_pathspec_match(true);
        for path in &write {
            checkout.path(path);
        }
        repo.checkout_tree(tree.as_object(), Some(&mut checkout))?;
        restored.written = write.len();
    }
    for path in delete {
        let file = workdir.join(path);
        match fs::symlink_metadata(&file) {
            Ok(meta) if !meta.is_dir() => {
                fs::remove_file(&file).map_err(|err| {
                    git2::Error::from_str(&format!("could not delete {path}: {err}"))
                })?;
                remove_empty_parents(&workdir, &file);
                restored.deleted += 1;
            }
            _ => {}
        }
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::super::history::tests::TestRepo;
    use super::*;

    fn read(repo: &TestRepo, path: &str) -> String {
        fs::read_to_string(repo.path().join(path)).unwrap()
    }

    fn staged(repo: &TestRepo, path: &str) -> Option<String> {
        let index = repo.repo.index().unwrap();
        index.get_path(Path::new(path), 0).map(|entry| {
            let blob = repo.repo.find_blob(entry.id).unwrap();
            String::from_utf8_lossy(blob.content()).into_owned()
        })
    }

    #[test]
    fn files_are_written_as_a_commit_has_them_leaving_the_index() {
        let mut repo = TestRepo::new();
        let first = repo.commit(&[("a.rs", "one\n"), ("dir/b.rs", "b\n")], "First", &[]);
        let second = repo.commit(
            &[("a.rs", "two\n"), ("dir/c.rs", "c\n")],
            "Second",
            &[first],
        );
        repo.commit(&[("a.rs", "three\n")], "Third", &[second]);
        let git_dir = repo.repo.path().to_owned();

        // a.rs as the second commit has it, and dir/c.rs as the first
        // (which hadn't it: deleted, and dir kept for b.rs).
        let restored = restore(&git_dir, Some(second), &["a.rs"]).unwrap();
        assert_eq!(
            restored,
            Restored {
                written: 1,
                deleted: 0
            }
        );
        assert_eq!(read(&repo, "a.rs"), "two\n");
        let restored = restore(&git_dir, Some(first), &["dir/c.rs"]).unwrap();
        assert_eq!(
            restored,
            Restored {
                written: 0,
                deleted: 1
            }
        );
        assert!(!repo.path().join("dir/c.rs").exists());
        assert_eq!(read(&repo, "dir/b.rs"), "b\n");
        // The index still has HEAD's, so both are unstaged changes.
        assert_eq!(staged(&repo, "a.rs").as_deref(), Some("three\n"));
        assert_eq!(staged(&repo, "dir/c.rs").as_deref(), Some("c\n"));
        // dir/c.rs is deleted: writing it back loses nothing.
        assert_eq!(
            unstaged_among(&git_dir, &["a.rs", "dir/b.rs", "dir/c.rs"]).unwrap(),
            ["a.rs"]
        );
        // Back again, as the last commit has them: nothing unstaged.
        let head = repo.repo.head().unwrap().target().unwrap();
        let restored = restore(&git_dir, Some(head), &["a.rs", "dir/c.rs"]).unwrap();
        assert_eq!(
            restored,
            Restored {
                written: 2,
                deleted: 0
            }
        );
        assert_eq!(read(&repo, "a.rs"), "three\n");
        assert_eq!(read(&repo, "dir/c.rs"), "c\n");
        assert!(
            unstaged_among(&git_dir, &["a.rs", "dir/c.rs"])
                .unwrap()
                .is_empty()
        );
        // Nothing to restore from deletes: as before the first commit.
        restore(&git_dir, None, &["dir/b.rs", "dir/c.rs"]).unwrap();
        assert!(!repo.path().join("dir").exists());
        // Nothing given, nothing done.
        assert_eq!(
            restore(&git_dir, Some(first), &[]).unwrap(),
            Restored::default()
        );
        assert_eq!(read(&repo, "a.rs"), "three\n");
    }

    #[test]
    fn unstaged_and_untracked_files_are_what_a_restore_would_lose() {
        let mut repo = TestRepo::new();
        let first = repo.commit(&[("a.rs", "one\n"), ("b.rs", "b\n")], "First", &[]);
        let git_dir = repo.repo.path().to_owned();
        fs::write(repo.path().join("a.rs"), "edited\n").unwrap();
        fs::write(repo.path().join("new.rs"), "new\n").unwrap();
        // A staged change isn't lost: the index keeps it.
        fs::write(repo.path().join("b.rs"), "staged\n").unwrap();
        let mut index = repo.repo.index().unwrap();
        index.add_path(Path::new("b.rs")).unwrap();
        index.write().unwrap();
        assert_eq!(
            unstaged_among(&git_dir, &["a.rs", "b.rs", "new.rs"]).unwrap(),
            ["a.rs", "new.rs"]
        );
        assert!(unstaged_among(&git_dir, &[]).unwrap().is_empty());
        // Restoring overwrites them all the same; the staged version of
        // b.rs stays staged.
        restore(&git_dir, Some(first), &["a.rs", "b.rs", "new.rs"]).unwrap();
        assert_eq!(read(&repo, "a.rs"), "one\n");
        assert_eq!(read(&repo, "b.rs"), "b\n");
        assert!(!repo.path().join("new.rs").exists());
        assert_eq!(staged(&repo, "b.rs").as_deref(), Some("staged\n"));
    }

    #[cfg(unix)]
    #[test]
    fn the_executable_bit_comes_back_with_the_contents() {
        use std::os::unix::fs::PermissionsExt;
        let mut repo = TestRepo::new();
        fs::write(repo.path().join("run.sh"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(
            repo.path().join("run.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let first = repo.commit(&[], "Empty", &[]);
        let mut index = repo.repo.index().unwrap();
        index.add_path(Path::new("run.sh")).unwrap();
        index.write().unwrap();
        let tree = repo.repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("T", "t@example.com").unwrap();
        let parent = repo.repo.find_commit(first).unwrap();
        let second = repo
            .repo
            .commit(Some("HEAD"), &sig, &sig, "Script", &tree, &[&parent])
            .unwrap();
        fs::remove_file(repo.path().join("run.sh")).unwrap();
        restore(repo.repo.path(), Some(second), &["run.sh"]).unwrap();
        let mode = fs::metadata(repo.path().join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "{mode:o}");
    }

    #[test]
    fn a_submodule_is_refused_changing_nothing() {
        let mut repo = TestRepo::new();
        let first = repo.commit(&[("a.rs", "one\n")], "First", &[]);
        repo.add_submodule("libs/sub");
        fs::write(repo.path().join("a.rs"), "edited\n").unwrap();
        let err = restore(repo.repo.path(), Some(first), &["a.rs", "libs/sub"]).unwrap_err();
        assert!(err.message().contains("libs/sub"), "{}", err.message());
        assert_eq!(read(&repo, "a.rs"), "edited\n");
    }
}
