//! Persistent storage: the directory where the editor keeps everything
//! that outlives a run. Today that is the settings; worktrees, autosaves,
//! and the like will live there too.
//!
//! The directory is `~/.ninjaedit` unless a frontend (or a test) chooses
//! another. [`Storage`] is little more than that path: each kind of thing
//! kept there has a fixed file name under it, and the storage reads and
//! writes such files whole. The directory is made on the first write, so
//! only an editor that has something to keep leaves anything behind, and
//! a write replaces the file in one step, so a crash mid-way leaves the
//! old one intact rather than a half-written new one.

use crate::settings::{Settings, SettingsError};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The storage directory's name within the home directory.
pub const DIRECTORY_NAME: &str = ".ninjaedit";
/// The settings file's name within the storage directory.
pub const SETTINGS_FILE: &str = "settings.toml";

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
}
