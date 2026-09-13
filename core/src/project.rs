//! Projects: the root of a tree of files being worked on.
//!
//! A [`Project`] is fundamentally a wrapper around the path of its root
//! directory — that path alone is its identity (two `Project` values compare
//! equal iff their roots are the same directory). Everything else it carries,
//! such as the file index, is transient state that is rebuilt on open and
//! maintained in the background so the project itself opens instantly.
//!
//! Not every directory the editor is started in is a project. One that
//! isn't inside a git repository is opened as a plain
//! [directory](ProjectKind::Directory): its own files are indexed so they
//! can be opened quickly, but nothing beneath it is, since a user who
//! starts the editor in, say, their home directory or the root of the
//! filesystem to edit one file hasn't asked for all of that to be crawled.

use crate::buffer::FileBuffer;
use crate::index::FileIndex;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// What a [`Project`] is: a real project, or a lone directory being
/// edited in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectKind {
    /// A project proper, indexed in full: the working tree of a git
    /// repository, or any directory opened as one with
    /// [`Project::open`].
    Project,
    /// A directory that isn't part of a project, opened with
    /// [`Project::open_directory`]. Only its own files are indexed; its
    /// subdirectories are listed but never descended into.
    Directory,
}

/// A project rooted at a directory.
pub struct Project {
    root: PathBuf,
    kind: ProjectKind,
    index: FileIndex,
}

impl Project {
    /// Open the project rooted at `root`. This returns immediately: the file
    /// index starts filling in on a background thread (see [`FileIndex`]),
    /// so callers that need a complete file list should go through the
    /// index's wait methods.
    pub fn open(root: impl AsRef<Path>) -> io::Result<Project> {
        Project::open_as(root.as_ref(), ProjectKind::Project)
    }

    /// Open `root` as a lone [directory](ProjectKind::Directory) rather
    /// than a project: only its own files are indexed, and only it is
    /// watched for changes. Returns immediately like [`open`](Self::open).
    pub fn open_directory(root: impl AsRef<Path>) -> io::Result<Project> {
        Project::open_as(root.as_ref(), ProjectKind::Directory)
    }

    fn open_as(root: &Path, kind: ProjectKind) -> io::Result<Project> {
        let root = fs::canonicalize(root)?;
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} is not a directory", root.display()),
            ));
        }
        let index = match kind {
            ProjectKind::Project => FileIndex::new(root.clone()),
            ProjectKind::Directory => FileIndex::shallow(root.clone()),
        };
        Ok(Project { root, kind, index })
    }

    /// Open the project that contains `path`: the working tree of the git
    /// repository `path` belongs to, indexed in full. If `path` is not
    /// inside a repository (or the repository is bare), it isn't in a
    /// project at all, and `path` itself is opened as a lone
    /// [directory](ProjectKind::Directory) instead. `path` must be a
    /// directory.
    pub fn discover(path: impl AsRef<Path>) -> io::Result<Project> {
        let path = path.as_ref();
        let workdir = git2::Repository::discover(path)
            .ok()
            .and_then(|repo| repo.workdir().map(Path::to_path_buf));
        match workdir {
            Some(root) => Project::open(root),
            None => Project::open_directory(path),
        }
    }

    /// The project's root directory (canonicalized).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether this is a project proper or a lone directory.
    pub fn kind(&self) -> ProjectKind {
        self.kind
    }

    /// The project's display name: the root directory's name (or the
    /// whole root path for a directory without one, like the root of the
    /// filesystem).
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

        // Not a repository: the directory itself is opened, as a lone
        // directory rather than a project.
        let lone = Project::discover(&nested).unwrap();
        assert_eq!(lone.root(), nested);
        assert_eq!(lone.kind(), ProjectKind::Directory);
        assert!(!lone.index().is_recursive());

        git2::Repository::init(&root).unwrap();
        let found = Project::discover(&nested).unwrap();
        assert_eq!(found.root(), root);
        assert_eq!(found.kind(), ProjectKind::Project);
        assert!(found.index().is_recursive());
        assert_eq!(Project::discover(&root).unwrap().root(), root);
    }

    #[test]
    fn directory_indexes_only_its_own_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("top.txt"), "top").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub").join("deep.txt"), "deep").unwrap();

        let project = Project::open_directory(dir.path()).unwrap();
        assert!(project.index().wait_for_full(Duration::from_secs(10)));
        let files = project.index().files(true);
        assert_eq!(files, vec![project.root().join("top.txt")]);

        let project = Project::open(dir.path()).unwrap();
        assert!(project.index().wait_for_full(Duration::from_secs(10)));
        assert_eq!(project.index().files(true).len(), 2);
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
