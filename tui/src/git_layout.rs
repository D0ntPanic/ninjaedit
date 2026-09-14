//! How the git pages' panes are sized, kept per repository in the
//! project's storage so that a layout dragged into shape comes back
//! the next time a page opens. This is the TUI's own affair, not the
//! core's: another frontend lays its pages out its own way and keeps
//! its own files, so these are named for the TUI.
//!
//! The sizes are shares rather than columns and rows: on the git log
//! page the sidebar's share of the page's width, the log's share of its
//! height, and the file list's share of the width left beside the
//! sidebar; on the changes page the file lists' share of the page's
//! width, the commit box's share of its height, and the unstaged
//! list's share of the height left to the lists. A share
//! survives a change of terminal size, and a page with fewer columns
//! to give simply gives fewer. Each repository a page shows (the
//! project's own, under `.`, and each submodule under its path) has
//! sizes of its own, since a submodule's page is a page of its own.
//!
//! Each page has a file, `tui-git-log.toml` and `tui-git-changes.toml`,
//! with a table per repository:
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
//! A share left out is the page's default. The files are written by
//! the editor, so they are read leniently: a share that isn't a number
//! between zero and one is dropped rather than refused, and keys they
//! don't know are ignored.

use ninjaedit_core::Storage;
use std::collections::BTreeMap;
use std::fmt;
use std::io;
use toml::{Table, Value};

/// The git log page's file within a project's storage.
pub const LAYOUT_FILE: &str = "tui-git-log.toml";
/// The changes page's file within a project's storage.
pub const CHANGES_LAYOUT_FILE: &str = "tui-git-changes.toml";
/// The key of the project's own repository.
pub const MAIN_REPOSITORY: &str = ".";

/// One repository's pane sizes on a page: a share, or `None` for the
/// page's default, under each of the page's names for them.
pub trait Shares: Copy + Default + PartialEq {
    /// The names of the shares, as the file has them.
    const NAMES: &'static [&'static str];

    /// The share by name.
    fn get(&self, name: &str) -> Option<f32>;

    /// The sizes from a share for each name.
    fn from_fn(share: impl FnMut(&str) -> Option<f32>) -> Self;

    /// Whether every size is the default.
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// The git log page's pane sizes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PaneSizes {
    pub sidebar: Option<f32>,
    pub log: Option<f32>,
    pub files: Option<f32>,
}

impl Shares for PaneSizes {
    const NAMES: &'static [&'static str] = &["sidebar", "log", "files"];

    fn get(&self, name: &str) -> Option<f32> {
        match name {
            "sidebar" => self.sidebar,
            "log" => self.log,
            "files" => self.files,
            _ => None,
        }
    }

    fn from_fn(mut share: impl FnMut(&str) -> Option<f32>) -> PaneSizes {
        PaneSizes {
            sidebar: share("sidebar"),
            log: share("log"),
            files: share("files"),
        }
    }
}

/// The changes page's pane sizes: the file lists' share of the page's
/// width, the commit box's share of the page's height, and the
/// unstaged list's share of the lists' height.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChangesSizes {
    pub files: Option<f32>,
    pub unstaged: Option<f32>,
    pub commit: Option<f32>,
}

impl Shares for ChangesSizes {
    const NAMES: &'static [&'static str] = &["files", "unstaged", "commit"];

    fn get(&self, name: &str) -> Option<f32> {
        match name {
            "files" => self.files,
            "unstaged" => self.unstaged,
            "commit" => self.commit,
            _ => None,
        }
    }

    fn from_fn(mut share: impl FnMut(&str) -> Option<f32>) -> ChangesSizes {
        ChangesSizes {
            files: share("files"),
            unstaged: share("unstaged"),
            commit: share("commit"),
        }
    }
}

/// Why a layout file could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutError(pub String);

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LayoutError {}

/// The pane sizes of every repository a page has been resized for.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout<S> {
    repositories: BTreeMap<String, S>,
}

/// The git log page's layout.
pub type GitLogLayout = Layout<PaneSizes>;
/// The changes page's layout.
pub type GitChangesLayout = Layout<ChangesSizes>;

impl<S> Default for Layout<S> {
    fn default() -> Layout<S> {
        Layout {
            repositories: BTreeMap::new(),
        }
    }
}

impl<S: Shares> Layout<S> {
    /// The sizes for a repository, by its key; the defaults when none
    /// were kept.
    pub fn get(&self, repository: &str) -> S {
        self.repositories
            .get(repository)
            .copied()
            .unwrap_or_default()
    }

    /// Keep a repository's sizes; all defaults forgets it.
    pub fn set(&mut self, repository: &str, sizes: S) {
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

    /// Parse a file's text. Only a file that isn't TOML at all, or
    /// whose entries aren't tables, is an error.
    pub fn parse(text: &str) -> Result<Layout<S>, LayoutError> {
        let table: Table = text
            .parse()
            .map_err(|err: toml::de::Error| LayoutError(err.message().to_owned()))?;
        let mut layout = Layout::default();
        for (key, value) in table {
            let Some(entry) = value.as_table() else {
                return Err(LayoutError(format!(
                    "`{key}` must be a table of pane sizes"
                )));
            };
            let sizes = S::from_fn(|name| {
                entry
                    .get(name)
                    .and_then(share_of)
                    .filter(|share| (0.0..=1.0).contains(share))
            });
            layout.set(&key, sizes);
        }
        Ok(layout)
    }

    /// The file's text.
    pub fn to_toml(&self) -> String {
        let mut table = Table::new();
        for (key, sizes) in &self.repositories {
            let mut entry = Table::new();
            for name in S::NAMES {
                if let Some(share) = sizes.get(name) {
                    entry.insert((*name).to_owned(), Value::Float(f64::from(share)));
                }
            }
            table.insert(key.clone(), Value::Table(entry));
        }
        toml::to_string(&table).unwrap_or_default()
    }
}

/// A page's layout as kept in a project's storage, in `file`; nothing
/// kept when no pane has been resized yet.
pub fn load<S: Shares>(storage: &Storage, file: &str) -> Result<Layout<S>, LayoutError> {
    let path = storage.path(file);
    let text = storage
        .read(file)
        .map_err(|err| LayoutError(format!("could not read {}: {err}", path.display())))?;
    match text {
        Some(text) => {
            Layout::parse(&text).map_err(|err| LayoutError(format!("{}: {err}", path.display())))
        }
        None => Ok(Layout::default()),
    }
}

/// Keep a page's layout in a project's storage, in `file`.
pub fn save<S: Shares>(storage: &Storage, file: &str, layout: &Layout<S>) -> io::Result<()> {
    storage.write(file, &layout.to_toml())
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
    fn the_changes_page_has_sizes_of_its_own() {
        let mut layout = GitChangesLayout::default();
        layout.set(
            MAIN_REPOSITORY,
            ChangesSizes {
                files: Some(0.4),
                unstaged: None,
                commit: Some(0.2),
            },
        );
        let text = layout.to_toml();
        assert!(text.contains("files = 0.4"), "{text}");
        assert!(text.contains("commit = 0.2"), "{text}");
        assert!(!text.contains("unstaged"), "{text}");
        let parsed = GitChangesLayout::parse(&text).unwrap();
        assert_eq!(parsed, layout);
        // The log page's names mean nothing to it.
        let parsed = GitChangesLayout::parse("[\".\"]\nlog = 0.5\nunstaged = 0.3\n").unwrap();
        assert_eq!(
            parsed.get(MAIN_REPOSITORY),
            ChangesSizes {
                files: None,
                unstaged: Some(0.3),
                commit: None,
            }
        );
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
        assert!(load::<PaneSizes>(&storage, LAYOUT_FILE).unwrap().is_empty());
        let mut layout = GitLogLayout::default();
        layout.set(
            "libs/sub",
            PaneSizes {
                sidebar: None,
                log: Some(0.7),
                files: None,
            },
        );
        save(&storage, LAYOUT_FILE, &layout).unwrap();
        assert_eq!(load(&storage, LAYOUT_FILE).unwrap(), layout);
        // The changes page's file is another, so neither disturbs the
        // other.
        let mut changes = GitChangesLayout::default();
        changes.set(
            MAIN_REPOSITORY,
            ChangesSizes {
                files: Some(0.3),
                unstaged: Some(0.6),
                commit: None,
            },
        );
        save(&storage, CHANGES_LAYOUT_FILE, &changes).unwrap();
        assert_eq!(load(&storage, CHANGES_LAYOUT_FILE).unwrap(), changes);
        assert_eq!(load(&storage, LAYOUT_FILE).unwrap(), layout);
        storage.write(LAYOUT_FILE, "[\".\"\n").unwrap();
        let err = load::<PaneSizes>(&storage, LAYOUT_FILE)
            .unwrap_err()
            .to_string();
        assert!(err.contains(LAYOUT_FILE), "{err}");
    }
}
