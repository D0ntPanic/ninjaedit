//! Persistent storage: the directory where the editor keeps everything
//! that outlives a run. Today that is the settings and each project's
//! build configuration; worktrees, autosaves, and the like will live
//! there too. A frontend keeps its own files there as well, through
//! [`read`](Storage::read) and [`write`](Storage::write), for what is
//! its own affair: how it lays out a page, say.
//!
//! The directory is `~/.ninjaedit` unless a frontend (or a test) chooses
//! another. [`Storage`] is little more than that path: each kind of thing
//! kept there has a fixed file name under it, and the storage reads and
//! writes such files whole. The directory is made on the first write, so
//! only an editor that has something to keep leaves anything behind, and
//! a write replaces the file in one step, so a crash mid-way leaves the
//! old one intact rather than a half-written new one.
//!
//! State that belongs to one project lives under `projects/` in a
//! directory of its own, named after the project and the hash of its
//! root path (see [`Storage::project`]). The name keeps the directory
//! recognizable when browsing; the hash keeps two projects with the same
//! name apart.

use crate::build::{BUILD_FILE, BuildConfig, BuildError};
use crate::project::Project;
use crate::settings::{Settings, SettingsError};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use twox_hash::XxHash64;

/// The storage directory's name within the home directory.
pub const DIRECTORY_NAME: &str = ".ninjaedit";
/// The settings file's name within the storage directory.
pub const SETTINGS_FILE: &str = "settings.toml";
/// The directory within the storage that holds the per-project
/// directories.
pub const PROJECTS_DIRECTORY: &str = "projects";

/// The seed for every xxhash64 hash the editor takes. It is fixed so
/// hashes that name things on disk, like project directories, come out
/// the same from one run to the next.
const HASH_SEED: u64 = 0;

/// The xxhash64 hash of `data`. This is fast rather than cryptographic:
/// use it to name and look things up, not to protect them.
pub fn hash_bytes(data: &[u8]) -> u64 {
    XxHash64::oneshot(HASH_SEED, data)
}

/// The name of the directory a project's state is kept in, within the
/// projects directory: the project's name, a dash, and the xxhash64 hash
/// of its root path as sixteen hex digits. A name that is itself a path,
/// as the root of the filesystem's is, has its separators replaced so
/// the result stays one directory name.
pub fn project_directory_name(project: &Project) -> String {
    let hash = hash_bytes(project.root().as_os_str().as_encoded_bytes());
    let name: String = project
        .name()
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '_'
            } else {
                c
            }
        })
        .collect();
    format!("{name}-{hash:016x}")
}

/// The directory the editor keeps its persistent state in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Storage {
    dir: PathBuf,
}

impl Storage {
    /// Storage in `dir`, which need not exist yet.
    pub fn new(dir: impl Into<PathBuf>) -> Storage {
        Storage { dir: dir.into() }
    }

    /// Storage in the usual place, `~/.ninjaedit`. Fails only when the
    /// home directory can't be found.
    pub fn in_home() -> io::Result<Storage> {
        let home = std::env::home_dir().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "the home directory is unknown, so ~/.ninjaedit can't be used",
            )
        })?;
        Ok(Storage::new(home.join(DIRECTORY_NAME)))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The path of a file kept in the storage.
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// The directory that holds every project's storage.
    pub fn projects_dir(&self) -> PathBuf {
        self.dir.join(PROJECTS_DIRECTORY)
    }

    /// The storage for `project`'s own state: a directory under
    /// `projects/` named after the project and the hash of its root
    /// path. Like any storage, it need not exist yet; it is made on the
    /// first write into it.
    pub fn project(&self, project: &Project) -> Storage {
        Storage::new(self.projects_dir().join(project_directory_name(project)))
    }

    /// Read a file kept in the storage, or `None` if there isn't one.
    pub fn read(&self, name: &str) -> io::Result<Option<String>> {
        match fs::read_to_string(self.path(name)) {
            Ok(text) => Ok(Some(text)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Write a file into the storage, making the directory if need be.
    /// The file is replaced in one step: the contents go to a temporary
    /// file beside it first.
    pub fn write(&self, name: &str, contents: &str) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.path(name);
        let mut temp = path.clone();
        temp.set_file_name(format!(".{name}.tmp"));
        fs::write(&temp, contents)?;
        fs::rename(&temp, &path).inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })
    }

    /// The settings kept in the storage; the defaults when there are
    /// none yet.
    pub fn load_settings(&self) -> Result<Settings, SettingsError> {
        let path = self.path(SETTINGS_FILE);
        let text = self
            .read(SETTINGS_FILE)
            .map_err(|err| SettingsError(format!("could not read {}: {err}", path.display())))?;
        match text {
            Some(text) => Settings::parse(&text)
                .map_err(|err| SettingsError(format!("{}: {err}", path.display()))),
            None => Ok(Settings::default()),
        }
    }

    /// Keep the settings in the storage.
    pub fn save_settings(&self, settings: &Settings) -> io::Result<()> {
        self.write(SETTINGS_FILE, &settings.to_toml())
    }

    /// The build configuration kept in a project's storage, or `None`
    /// when none has been saved yet (in which case the frontend finds
    /// the project's roots itself).
    pub fn load_build_config(&self) -> Result<Option<BuildConfig>, BuildError> {
        let path = self.path(BUILD_FILE);
        let text = self
            .read(BUILD_FILE)
            .map_err(|err| BuildError(format!("could not read {}: {err}", path.display())))?;
        text.map(|text| {
            BuildConfig::parse(&text)
                .map_err(|err| BuildError(format!("{}: {err}", path.display())))
        })
        .transpose()
    }

    /// Keep a project's build configuration in its storage.
    pub fn save_build_config(&self, config: &BuildConfig) -> io::Result<()> {
        self.write(BUILD_FILE, &config.to_toml())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::SettingKey;

    #[test]
    fn reads_nothing_until_something_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().join("store"));
        assert_eq!(storage.read("a.txt").unwrap(), None);
        assert!(!storage.dir().exists(), "reading makes nothing");
        storage.write("a.txt", "one").unwrap();
        assert_eq!(storage.read("a.txt").unwrap().as_deref(), Some("one"));
        storage.write("a.txt", "two").unwrap();
        assert_eq!(storage.read("a.txt").unwrap().as_deref(), Some("two"));
        // No temporary file is left behind.
        let names: Vec<String> = fs::read_dir(storage.dir())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a.txt"]);
    }

    #[test]
    fn settings_round_trip_through_the_storage() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().join("store"));
        assert_eq!(storage.load_settings().unwrap(), Settings::default());

        let mut settings = Settings::default();
        settings
            .set_text(SettingKey::TerminalScrollback, "500")
            .unwrap();
        storage.save_settings(&settings).unwrap();
        assert_eq!(storage.load_settings().unwrap(), settings);
        assert_eq!(storage.load_settings().unwrap().terminal_scrollback(), 500);

        // A file that can't be parsed is reported with its path.
        storage.write(SETTINGS_FILE, "[terminal\n").unwrap();
        let err = storage.load_settings().unwrap_err().to_string();
        assert!(err.contains(SETTINGS_FILE), "{err}");
    }

    #[test]
    fn build_config_round_trips_through_a_project_storage() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().join("store"));
        let project_dir = dir.path().join("app");
        fs::create_dir_all(&project_dir).unwrap();
        let project = Project::open(&project_dir).unwrap();
        let project_storage = storage.project(&project);
        assert_eq!(project_storage.load_build_config().unwrap(), None);

        let mut config = BuildConfig::default();
        config
            .add_root(crate::build::BuildRoot::new(
                crate::build::BuildSystem::Cargo,
                "Cargo.toml",
            ))
            .unwrap();
        project_storage.save_build_config(&config).unwrap();
        assert_eq!(project_storage.load_build_config().unwrap(), Some(config));
        assert!(project_storage.path(BUILD_FILE).is_file());

        project_storage.write(BUILD_FILE, "roots = 1\n").unwrap();
        let err = project_storage.load_build_config().unwrap_err().to_string();
        assert!(err.contains(BUILD_FILE), "{err}");
    }

    #[test]
    fn hash_is_xxhash64() {
        // Reference values from the xxHash specification.
        assert_eq!(hash_bytes(b""), 0xef46db3751d8e999);
        assert_eq!(hash_bytes(b"a"), 0xd24ec4f1a98c6e5b);
    }

    #[test]
    fn project_storage_is_named_by_name_and_path_hash() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().join("store"));

        // Two projects with the same name but different roots get
        // different directories; the same project always gets the same.
        let a = dir.path().join("a").join("app");
        let b = dir.path().join("b").join("app");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let project_a = Project::open(&a).unwrap();
        let project_b = Project::open(&b).unwrap();

        let name_a = project_directory_name(&project_a);
        let name_b = project_directory_name(&project_b);
        assert_ne!(name_a, name_b);
        assert_eq!(name_a, project_directory_name(&Project::open(&a).unwrap()));
        let expected_hash = hash_bytes(project_a.root().as_os_str().as_encoded_bytes());
        assert_eq!(name_a, format!("app-{expected_hash:016x}"));
        assert_eq!(name_a.len(), "app-".len() + 16);

        let project_storage = storage.project(&project_a);
        assert_eq!(project_storage.dir(), storage.projects_dir().join(&name_a));
        assert_ne!(storage.project(&project_b).dir(), project_storage.dir());

        // Nothing exists until something is written, and a write goes to
        // the project's own directory rather than the top-level one.
        assert!(!storage.projects_dir().exists());
        project_storage.write("state.txt", "kept").unwrap();
        assert_eq!(
            project_storage.read("state.txt").unwrap().as_deref(),
            Some("kept")
        );
        assert_eq!(storage.read("state.txt").unwrap(), None);
        assert!(
            storage
                .projects_dir()
                .join(&name_a)
                .join("state.txt")
                .is_file()
        );
    }
}
