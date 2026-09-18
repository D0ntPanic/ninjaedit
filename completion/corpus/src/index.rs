//! Reads the crates.io index from the mirror and selects one version per crate.

use anyhow::{Context, Result};
use rayon::prelude::*;
use semver::Version;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct IndexLine {
    name: String,
    vers: String,
    #[serde(default)]
    yanked: bool,
    #[serde(default)]
    deps: Vec<IndexDep>,
    #[serde(default)]
    pubtime: Option<String>,
}

#[derive(Deserialize)]
struct IndexDep {
    name: String,
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    optional: bool,
}

/// One published version of a crate that has not been yanked.
pub struct VersionEntry {
    pub version: Version,
    pub deps: Vec<String>,
    pub pubtime: Option<String>,
}

/// A crate from the index along with its non-yanked versions, newest first.
pub struct CrateEntry {
    pub name: String,
    pub versions: Vec<VersionEntry>,
}

impl CrateEntry {
    /// Directory in the mirror holding this crate's tarballs, one subdirectory per version.
    pub fn mirror_dir(&self, mirror: &Path) -> PathBuf {
        let name = &self.name;
        let lower = name.to_ascii_lowercase();
        let mut dir = mirror.join("crates");
        match lower.len() {
            1 => dir.push("1"),
            2 => dir.push("2"),
            3 => {
                dir.push("3");
                dir.push(&lower[..1]);
            }
            _ => {
                dir.push(&lower[..2]);
                dir.push(&lower[2..4]);
            }
        }
        dir.push(name);
        dir
    }

    /// Picks the newest non-yanked version that is present on disk, preferring stable
    /// releases over prereleases. Returns the version and the path to its tarball.
    pub fn select(&self, mirror: &Path) -> Option<(&VersionEntry, PathBuf)> {
        let dir = self.mirror_dir(mirror);
        let stable = self.versions.iter().filter(|v| v.version.pre.is_empty());
        let pre = self.versions.iter().filter(|v| !v.version.pre.is_empty());
        for entry in stable.chain(pre) {
            let path = dir
                .join(entry.version.to_string())
                .join(format!("{}-{}.crate", self.name, entry.version));
            if path.is_file() {
                return Some((entry, path));
            }
        }
        None
    }
}

/// Loads every crate in the index. Entries are sorted by name so runs are deterministic.
pub fn load(index_root: &Path) -> Result<Vec<CrateEntry>> {
    let mut files = Vec::new();
    collect_files(index_root, &mut files)?;
    files.sort();
    let crates: Vec<CrateEntry> = files
        .par_iter()
        .map(|path| parse_file(path))
        .collect::<Result<Vec<Option<CrateEntry>>>>()?
        .into_iter()
        .flatten()
        .collect();
    Ok(crates)
}

fn parse_file(path: &Path) -> Result<Option<CrateEntry>> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut name = None;
    let mut versions = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<IndexLine>(line) else {
            continue;
        };
        if entry.yanked {
            continue;
        }
        let Ok(version) = Version::parse(&entry.vers) else {
            continue;
        };
        let deps: BTreeSet<String> = entry
            .deps
            .into_iter()
            .filter(|d| !d.optional && matches!(d.kind.as_deref(), None | Some("normal")))
            .map(|d| d.package.unwrap_or(d.name))
            .collect();
        name.get_or_insert(entry.name);
        versions.push(VersionEntry {
            version,
            deps: deps.into_iter().collect(),
            pubtime: entry.pubtime,
        });
    }
    let Some(name) = name else { return Ok(None) };
    versions.sort_by(|a, b| b.version.cmp(&a.version));
    Ok(Some(CrateEntry { name, versions }))
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if file_name.starts_with('.') || file_name == "config.json" {
            continue;
        }
        if entry.file_type()?.is_dir() {
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}
