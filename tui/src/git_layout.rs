//! How the git log page's panes are sized, kept per repository in the
//! project's storage so that a layout dragged into shape comes back
//! the next time the page opens. This is the TUI's own affair, not the
//! core's: another frontend lays its page out its own way and keeps
//! its own file, so this one is named for the TUI.
//!
//! The sizes are shares rather than columns and rows: the sidebar's
//! share of the page's width, the log's share of its height, and the
//! file list's share of the width left beside the sidebar. A share
//! survives a change of terminal size, and a page with fewer columns
//! to give simply gives fewer. Each repository the page shows (the
//! project's own, under `.`, and each submodule under its path) has
//! sizes of its own, since a submodule's history is a page of its own.
//!
//! The file, `tui-git-log.toml`, has a table per repository:
//!
//! ```toml
//! ["."]
//! sidebar = 0.2
//! log = 0.5
//! files = 0.33
//!
//! ["libs/sub"]
//! log = 0.7
//! ```
//!
//! A share left out is the page's default. The file is written by the
//! editor, so it is read leniently: a share that isn't a number between
//! zero and one is dropped rather than refused, and keys it doesn't
//! know are ignored.

use ninjaedit_core::Storage;
use std::collections::BTreeMap;
use std::fmt;
use std::io;
use toml::{Table, Value};

/// The file's name within a project's storage.
pub const LAYOUT_FILE: &str = "tui-git-log.toml";
/// The key of the project's own repository.
pub const MAIN_REPOSITORY: &str = ".";

/// One repository's pane sizes, each a share of the space it divides;
/// `None` leaves the page's default.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PaneSizes {
    pub sidebar: Option<f32>,
    pub log: Option<f32>,
    pub files: Option<f32>,
}

impl PaneSizes {
    /// Whether every size is the default.
    pub fn is_default(&self) -> bool {
        *self == PaneSizes::default()
    }
}

/// Why the layout file could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutError(pub String);

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LayoutError {}

/// The pane sizes of every repository the page has been resized for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GitLogLayout {
    repositories: BTreeMap<String, PaneSizes>,
}

impl GitLogLayout {
    /// The sizes for a repository, by its key; the defaults when none
    /// were kept.
    pub fn get(&self, repository: &str) -> PaneSizes {
        self.repositories
            .get(repository)
            .copied()
            .unwrap_or_default()
    }

    /// Keep a repository's sizes; all defaults forgets it.
    pub fn set(&mut self, repository: &str, sizes: PaneSizes) {
        if sizes.is_default() {
            self.repositories.remove(repository);
        } else {
            self.repositories.insert(repository.to_owned(), sizes);
        }
    }

    /// Whether nothing has been kept.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.repositories.is_empty()
    }

    /// Parse the file's text. Only a file that isn't TOML at all, or
    /// whose entries aren't tables, is an error.
    pub fn parse(text: &str) -> Result<GitLogLayout, LayoutError> {
        let table: Table = text
            .parse()
            .map_err(|err: toml::de::Error| LayoutError(err.message().to_owned()))?;
        let mut layout = GitLogLayout::default();
        for (key, value) in table {
            let Some(entry) = value.as_table() else {
                return Err(LayoutError(format!(
                    "`{key}` must be a table of pane sizes"
                )));
            };
            let share = |name: &str| {
                entry
                    .get(name)
                    .and_then(share_of)
                    .filter(|share| (0.0..=1.0).contains(share))
            };
            layout.set(
                &key,
                PaneSizes {
                    sidebar: share("sidebar"),
                    log: share("log"),
                    files: share("files"),
                },
            );
        }
        Ok(layout)
    }

    /// The file's text.
    pub fn to_toml(&self) -> String {
        let mut table = Table::new();
        for (key, sizes) in &self.repositories {
            let mut entry = Table::new();
            for (name, share) in [
                ("sidebar", sizes.sidebar),
                ("log", sizes.log),
                ("files", sizes.files),
            ] {
                if let Some(share) = share {
                    entry.insert(name.to_owned(), Value::Float(f64::from(share)));
                }
            }
            table.insert(key.clone(), Value::Table(entry));
        }
        toml::to_string(&table).unwrap_or_default()
    }
}

/// The layout kept in a project's storage; nothing kept when no pane
/// has been resized yet.
pub fn load(storage: &Storage) -> Result<GitLogLayout, LayoutError> {
    let path = storage.path(LAYOUT_FILE);
    let text = storage
        .read(LAYOUT_FILE)
        .map_err(|err| LayoutError(format!("could not read {}: {err}", path.display())))?;
    match text {
        Some(text) => GitLogLayout::parse(&text)
            .map_err(|err| LayoutError(format!("{}: {err}", path.display()))),
        None => Ok(GitLogLayout::default()),
    }
}

/// Keep the layout in a project's storage.
pub fn save(storage: &Storage, layout: &GitLogLayout) -> io::Result<()> {
    storage.write(LAYOUT_FILE, &layout.to_toml())
}

/// A share from a TOML number, integer or float.
fn share_of(value: &Value) -> Option<f32> {
    match value {
        Value::Float(f) => Some(*f as f32),
        Value::Integer(i) => Some(*i as f32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_round_trip_through_the_file_by_repository() {
        let mut layout = GitLogLayout::default();
        assert!(layout.is_empty());
        assert_eq!(layout.get(MAIN_REPOSITORY), PaneSizes::default());
        layout.set(
            MAIN_REPOSITORY,
            PaneSizes {
                sidebar: Some(0.25),
                log: Some(0.5),
                files: None,
            },
        );
        layout.set(
            "libs/sub",
            PaneSizes {
                sidebar: None,
                log: Some(0.75),
                files: Some(0.4),
            },
        );
        let text = layout.to_toml();
        assert!(text.contains("[\".\"]"), "{text}");
        assert!(text.contains("[\"libs/sub\"]"), "{text}");
        assert!(text.contains("sidebar = 0.25"), "{text}");
        assert!(
            !text.contains("files = 0\n"),
            "unset shares are left out: {text}"
        );
        let parsed = GitLogLayout::parse(&text).unwrap();
        assert_eq!(parsed, layout);
        assert_eq!(parsed.get("libs/sub").log, Some(0.75));
        // All defaults forgets the repository.
        layout.set("libs/sub", PaneSizes::default());
        assert!(!layout.to_toml().contains("libs/sub"));
    }

    #[test]
    fn the_file_is_read_leniently() {
        let layout = GitLogLayout::parse(
            "[\".\"]\nsidebar = 1\nlog = 1.5\nfiles = \"wide\"\ncolor = \"red\"\n\n[\"other\"]\n",
        )
        .unwrap();
        assert_eq!(
            layout.get(MAIN_REPOSITORY),
            PaneSizes {
                sidebar: Some(1.0),
                log: None,
                files: None,
            }
        );
        assert!(layout.get("other").is_default());
        assert!(GitLogLayout::parse("").unwrap().is_empty());
        assert!(GitLogLayout::parse("[\".\"\n").is_err());
        let err = GitLogLayout::parse("main = 3\n").unwrap_err().to_string();
        assert!(err.contains("must be a table"), "{err}");
    }

    #[test]
    fn the_layout_is_kept_in_a_projects_storage() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().join("p"));
        assert!(load(&storage).unwrap().is_empty());
        let mut layout = GitLogLayout::default();
        layout.set(
            "libs/sub",
            PaneSizes {
                sidebar: None,
                log: Some(0.7),
                files: None,
            },
        );
        save(&storage, &layout).unwrap();
        assert_eq!(load(&storage).unwrap(), layout);
        storage.write(LAYOUT_FILE, "[\".\"\n").unwrap();
        let err = load(&storage).unwrap_err().to_string();
        assert!(err.contains(LAYOUT_FILE), "{err}");
    }
}
