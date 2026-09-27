//! Finds the projects in a bandersnatch mirror of PyPI and picks the distributions to read.
//!
//! Release files are stored as `web/packages/<xx>/<yy>/<hash>/<file>`, so nothing groups a
//! project's files but their names. A wheel's name and version are the first two of its
//! `-`-separated fields; a source distribution's name ends at the last `-` before a digit.
//! Projects are keyed by the name normalized as PyPI compares names (PEP 503). A wheel's core
//! metadata is beside it as `<wheel>.metadata`. The `web/simple` index is not read: it is one
//! enormous directory.

use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Source distribution archive extensions.
const SDIST_EXTENSIONS: &[&str] = &[".tar.gz", ".tgz", ".tar.bz2", ".tar.xz", ".zip"];

/// Platform names that `bdist_dumb` appends to a built distribution's version
/// (`name-1.0.linux-x86_64.tar.gz`), which is not a source distribution.
const PLATFORMS: &[&str] = &[
    ".linux", ".macosx", ".win32", ".win-", ".cygwin", ".freebsd", ".openbsd", ".netbsd",
    ".solaris", ".sunos", ".darwin", ".aix", ".mingw",
];

pub struct Dist {
    pub path: PathBuf,
    pub size: u64,
}

pub struct Project {
    /// The name as the file names spell it, until the metadata gives the real one.
    pub name: String,
    pub version: String,
    /// The source distribution, which is what is read when there is one: its layout is the
    /// project's source tree, tests and all.
    pub sdist: Option<Dist>,
    /// A wheel of the same version, read when there is no source distribution or it cannot
    /// be read.
    pub wheel: Option<Dist>,
    /// The wheel's core metadata.
    pub metadata: Option<PathBuf>,
}

impl Project {
    /// Compressed size of the distributions that may be read.
    pub fn size(&self) -> u64 {
        self.sdist.iter().chain(&self.wheel).map(|d| d.size).sum()
    }
}

/// What the mirror holds, by kind of file.
#[derive(Default)]
pub struct MirrorCounts {
    pub wheels: u64,
    pub sdists: u64,
    pub metadata: u64,
    /// Built distributions that are not wheels: eggs, installers, `bdist_dumb` tarballs.
    pub other_builds: u64,
    /// Files whose names could not be parsed.
    pub unparsed: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Sdist,
    Wheel,
}

struct Found {
    key: String,
    name: String,
    version: String,
    kind: Kind,
    path: PathBuf,
    size: u64,
    mtime: SystemTime,
    metadata: bool,
}

/// Finds every project in the mirror, sorted by normalized name.
pub fn find_projects(mirror: &Path) -> Result<(Vec<Project>, MirrorCounts)> {
    let root = mirror.join("web").join("packages");
    let tops = subdirs(&root)?;
    let found: Vec<Result<(Vec<Found>, MirrorCounts)>> =
        tops.par_iter().map(|dir| scan_top(dir)).collect();
    let mut counts = MirrorCounts::default();
    let mut groups: HashMap<String, Vec<Found>> = HashMap::new();
    for result in found {
        let (files, c) = result?;
        counts.wheels += c.wheels;
        counts.sdists += c.sdists;
        counts.metadata += c.metadata;
        counts.other_builds += c.other_builds;
        counts.unparsed += c.unparsed;
        for f in files {
            groups.entry(f.key.clone()).or_default().push(f);
        }
    }
    let mut projects: Vec<(String, Project)> = groups
        .into_iter()
        .map(|(key, files)| (key, choose(files)))
        .collect();
    projects.sort_by(|a, b| a.0.cmp(&b.0));
    let projects: Vec<Project> = projects.into_iter().map(|(_, p)| p).collect();
    Ok((projects, counts))
}

fn subdirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    Ok(dirs)
}

/// Lists the release files under one `web/packages/<xx>` directory.
fn scan_top(top: &Path) -> Result<(Vec<Found>, MirrorCounts)> {
    let mut found = Vec::new();
    let mut counts = MirrorCounts::default();
    for mid in subdirs(top)? {
        for hash in subdirs(&mid)? {
            let mut files: Vec<(String, fs::Metadata)> = Vec::new();
            for entry in fs::read_dir(&hash)? {
                let entry = entry?;
                let Ok(name) = entry.file_name().into_string() else {
                    counts.unparsed += 1;
                    continue;
                };
                files.push((name, entry.metadata()?));
            }
            for (name, meta) in &files {
                if name.ends_with(".metadata") {
                    counts.metadata += 1;
                    continue;
                }
                let Some((project, version, kind)) = parse_file_name(name) else {
                    if name.ends_with(".egg")
                        || name.ends_with(".exe")
                        || name.ends_with(".msi")
                        || name.ends_with(".rpm")
                        || name.ends_with(".dmg")
                        || name.ends_with(".deb")
                        || SDIST_EXTENSIONS.iter().any(|e| name.ends_with(e))
                    {
                        counts.other_builds += 1;
                    } else {
                        counts.unparsed += 1;
                    }
                    continue;
                };
                match kind {
                    Kind::Wheel => counts.wheels += 1,
                    Kind::Sdist => counts.sdists += 1,
                }
                let metadata_name = format!("{name}.metadata");
                found.push(Found {
                    key: normalize(project),
                    name: project.to_owned(),
                    version: version.to_owned(),
                    kind,
                    path: hash.join(name),
                    size: meta.len(),
                    mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    metadata: files.iter().any(|(n, _)| *n == metadata_name),
                });
            }
        }
    }
    Ok((found, counts))
}

/// The project name and version in a release file's name, and which kind of distribution it
/// is, or `None` for anything but a wheel or a source distribution.
fn parse_file_name(file: &str) -> Option<(&str, &str, Kind)> {
    if let Some(stem) = file.strip_suffix(".whl") {
        let mut fields = stem.split('-');
        let (name, version) = (fields.next()?, fields.next()?);
        return Some((name, version, Kind::Wheel));
    }
    let stem = SDIST_EXTENSIONS
        .iter()
        .find_map(|ext| file.strip_suffix(ext))?;
    let split = stem.rmatch_indices('-').map(|(i, _)| i).find(|&i| {
        let rest = &stem[i + 1..];
        let rest = rest.strip_prefix(['v', 'V']).unwrap_or(rest);
        rest.starts_with(|c: char| c.is_ascii_digit())
    })?;
    let (name, version) = (&stem[..split], &stem[split + 1..]);
    if name.is_empty() || PLATFORMS.iter().any(|p| version.contains(p)) {
        return None;
    }
    Some((name, version, Kind::Sdist))
}

/// A project name as PyPI compares names: lowercase, with runs of `-`, `_` and `.` as one `-`.
pub fn normalize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !out.ends_with('-') {
                out.push('-');
            }
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// Picks one project's distributions: its newest source distribution and a wheel of the same
/// version, or its newest wheel if it has no source distribution. Mirrors that keep only the
/// latest release have one version, but a project's files are not always named alike.
fn choose(mut files: Vec<Found>) -> Project {
    // Newest first, then by name so that the choice does not depend on listing order.
    files.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.path.cmp(&b.path)));
    let sdist = files.iter().find(|f| f.kind == Kind::Sdist);
    let wheels = files
        .iter()
        .filter(|f| f.kind == Kind::Wheel && sdist.is_none_or(|s| s.version == f.version));
    // A pure-Python wheel over one built for a particular interpreter.
    let wheel = wheels
        .clone()
        .find(|f| f.path.to_string_lossy().ends_with("-none-any.whl"))
        .or_else(|| wheels.clone().next());
    let first = sdist.or(wheel).expect("a project has at least one file");
    let dist = |f: &Found| Dist {
        path: f.path.clone(),
        size: f.size,
    };
    Project {
        name: first.name.clone(),
        version: first.version.clone(),
        sdist: sdist.map(dist),
        wheel: wheel.map(dist),
        metadata: wheel.filter(|w| w.metadata).map(|w| {
            let mut name = w.path.clone().into_os_string();
            name.push(".metadata");
            PathBuf::from(name)
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(file: &str) -> Option<(&str, &str, bool)> {
        parse_file_name(file).map(|(n, v, k)| (n, v, k == Kind::Wheel))
    }

    #[test]
    fn file_names() {
        assert_eq!(
            parse("chopro_epub-1.1.1-py3-none-any.whl"),
            Some(("chopro_epub", "1.1.1", true))
        );
        assert_eq!(
            parse("python-speck-0.0.1.tar.gz"),
            Some(("python-speck", "0.0.1", false))
        );
        assert_eq!(
            parse("python-3parclient-4.2.zip"),
            Some(("python-3parclient", "4.2", false))
        );
        assert_eq!(
            parse("foo-1.0-beta.tar.gz"),
            Some(("foo", "1.0-beta", false))
        );
        assert_eq!(parse("foo-v2.1.tgz"), Some(("foo", "v2.1", false)));
        assert_eq!(parse("swot-1.0.1.linux-x86_64.tar.gz"), None);
        assert_eq!(parse("zbar-0.10.win32-py2.6.exe"), None);
        assert_eq!(parse("pgpu-2.0.0-py2.7.egg"), None);
        assert_eq!(parse("noversion.tar.gz"), None);
    }

    #[test]
    fn normalized_names() {
        assert_eq!(normalize("Chopro_EPUB"), "chopro-epub");
        assert_eq!(normalize("zope.interface"), "zope-interface");
        assert_eq!(normalize("a-_.b"), "a-b");
    }
}
