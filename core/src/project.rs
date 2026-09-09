//! Projects: the root of a tree of files being worked on.
//!
//! A [`Project`] is fundamentally a wrapper around the path of its root
//! directory — that path alone is its identity (two `Project` values compare
//! equal iff their roots are the same directory). Everything else it carries,
//! such as the file index, is transient state that is rebuilt on open and
//! maintained in the background so the project itself opens instantly.

use crate::buffer::FileBuffer;
use crate::index::FileIndex;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// A project rooted at a directory.
pub struct Project {
    root: PathBuf,
    index: FileIndex,
}

impl Project {
    /// Open the project rooted at `root`. This returns immediately: the file
    /// index starts filling in on a background thread (see [`FileIndex`]),
    /// so callers that need a complete file list should go through the
    /// index's wait methods.
    pub fn open(root: impl AsRef<Path>) -> io::Result<Project> {
        let root = fs::canonicalize(root)?;
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} is not a directory", root.display()),
            ));
        }
        let index = FileIndex::new(root.clone());
        Ok(Project { root, index })
    }

    /// Open the project that contains `path`: the working tree of the git
    /// repository `path` belongs to, or `path` itself if it is not inside a
    /// repository (or the repository is bare). `path` must be a directory.
    pub fn discover(path: impl AsRef<Path>) -> io::Result<Project> {
        let path = path.as_ref();
        let root = git2::Repository::discover(path)
            .ok()
            .and_then(|repo| repo.workdir().map(Path::to_path_buf))
            .unwrap_or_else(|| path.to_path_buf());
        Project::open(root)
    }

    /// The project's root directory (canonicalized).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The project's display name: the root directory's name.
    pub fn name(&self) -> String {
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.to_string_lossy().into_owned())
    }

    /// The live index of the project's files.
    pub fn index(&self) -> &FileIndex {
        &self.index
    }

    /// Load a file into a buffer. Relative paths are resolved against the
    /// project root.
    pub fn open_file(&self, path: impl AsRef<Path>) -> io::Result<FileBuffer> {
        let path = path.as_ref();
        if path.is_absolute() {
            FileBuffer::open(path)
        } else {
            FileBuffer::open(self.root.join(path))
        }
    }
}

impl PartialEq for Project {
    fn eq(&self, other: &Project) -> bool {
        self.root == other.root
    }
}

impl Eq for Project {}

impl std::fmt::Debug for Project {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Project")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn open_and_edit_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("hello.txt"), "hello\nworld\n").unwrap();

        let project = Project::open(dir.path()).unwrap();
        assert!(project.index().wait_for_primary(Duration::from_secs(10)));
        assert_eq!(project.index().files(false).len(), 1);

        let mut buffer = project.open_file("hello.txt").unwrap();
        assert_eq!(buffer.line_count(), 3);
        buffer.set_line(1, "there");
        buffer.save().unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
            "hello\nthere\n"
        );
    }

    #[test]
    fn identity_is_the_root_path() {
        let dir = tempfile::tempdir().unwrap();
        let a = Project::open(dir.path()).unwrap();
        let b = Project::open(dir.path()).unwrap();
        let other = tempfile::tempdir().unwrap();
        let c = Project::open(other.path()).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn discover_finds_git_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let nested = root.join("a").join("b");
        fs::create_dir_all(&nested).unwrap();

        // Not a repository: the directory itself is the project.
        assert_eq!(Project::discover(&nested).unwrap().root(), nested);

        git2::Repository::init(&root).unwrap();
        assert_eq!(Project::discover(&nested).unwrap().root(), root);
        assert_eq!(Project::discover(&root).unwrap().root(), root);
    }

    #[test]
    fn open_rejects_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file.txt");
        fs::write(&file, "x").unwrap();
        assert!(Project::open(&file).is_err());
        assert!(Project::open(dir.path().join("missing")).is_err());
    }
}
