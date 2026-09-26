//! Finds source packages in a Debian or Ubuntu archive mirror and streams their files.
//!
//! Every package is described by a `.dsc` file in `pool/<component>/<prefix>/<name>/`, which
//! lists its archives: the upstream tarball and any component tarballs, and the packaging as
//! a `.debian.tar` (format 3.0) or `.diff.gz` (format 1.0), or one tarball for a native
//! package.

use anyhow::{Context, Result, bail};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

pub const COMPONENTS: &[&str] = &["main", "restricted", "universe", "multiverse"];

pub struct Package {
    pub name: String,
    pub version: String,
    /// Index into `COMPONENTS`.
    pub component: u8,
    pub dir: PathBuf,
    pub archives: Vec<Archive>,
}

impl Package {
    /// Compressed size of every archive.
    pub fn size(&self) -> u64 {
        self.archives.iter().map(|a| a.size).sum()
    }
}

pub struct Archive {
    pub file: String,
    pub kind: ArchiveKind,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveKind {
    /// The upstream tarball.
    Orig,
    /// An additional upstream tarball, unpacked into a directory of this name.
    Component(String),
    /// The `debian/` directory of a format 3.0 package.
    Debian,
    /// The packaging of a format 1.0 package, as a patch.
    Diff,
    /// The single tarball of a native package, `debian/` included.
    Native,
}

/// Finds every `.dsc` under `pool/<component>/` for the given components, sorted by name.
pub fn find_packages(mirror: &Path, components: &[String]) -> Result<Vec<Package>> {
    let mut packages = Vec::new();
    for component in components {
        let Some(index) = COMPONENTS.iter().position(|c| c == component) else {
            bail!("unknown component {component:?}, expected one of {COMPONENTS:?}");
        };
        let root = mirror.join("pool").join(component);
        for prefix in read_dirs(&root)? {
            for dir in read_dirs(&prefix)? {
                for entry in fs::read_dir(&dir)? {
                    let path = entry?.path();
                    if path.extension().is_some_and(|e| e == "dsc") {
                        packages.push(
                            parse_dsc(&path, index as u8)
                                .with_context(|| format!("reading {}", path.display()))?,
                        );
                    }
                }
            }
        }
    }
    packages.sort_by(|a, b| a.name.cmp(&b.name).then(a.component.cmp(&b.component)));
    Ok(packages)
}

fn read_dirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    Ok(dirs)
}

fn parse_dsc(path: &Path, component: u8) -> Result<Package> {
    let text = String::from_utf8_lossy(&fs::read(path)?).into_owned();
    let mut name = None;
    let mut version = None;
    let mut archives = Vec::new();
    let mut in_files = false;
    for line in text.lines() {
        if in_files {
            if let Some(entry) = line.strip_prefix(' ') {
                // ` <md5> <size> <file>`
                let mut fields = entry.split_whitespace();
                let (Some(_), Some(size), Some(file)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    continue;
                };
                if let Some(kind) = archive_kind(file) {
                    archives.push(Archive {
                        file: file.to_owned(),
                        kind,
                        size: size.parse().unwrap_or(0),
                    });
                }
                continue;
            }
            in_files = false;
        }
        if let Some(v) = line.strip_prefix("Source:") {
            name = Some(v.trim().to_owned());
        } else if let Some(v) = line.strip_prefix("Version:") {
            version = Some(v.trim().to_owned());
        } else if line.starts_with("Files:") {
            in_files = true;
        }
    }
    let Some(name) = name else {
        bail!("no Source field");
    };
    Ok(Package {
        name,
        version: version.unwrap_or_default(),
        component,
        dir: path.parent().unwrap().to_path_buf(),
        archives,
    })
}

fn archive_kind(file: &str) -> Option<ArchiveKind> {
    if file.ends_with(".diff.gz") {
        return Some(ArchiveKind::Diff);
    }
    let stem = [".tar.gz", ".tar.xz", ".tar.bz2"]
        .iter()
        .find_map(|ext| file.strip_suffix(ext))?;
    if stem.ends_with(".debian") {
        return Some(ArchiveKind::Debian);
    }
    if stem.ends_with(".orig") {
        return Some(ArchiveKind::Orig);
    }
    if let Some((_, component)) = stem.rsplit_once(".orig-") {
        return Some(ArchiveKind::Component(component.to_owned()));
    }
    Some(ArchiveKind::Native)
}

/// Opens a compressed file for streaming decompression, by extension.
pub fn decompress(path: &Path) -> Result<Box<dyn Read>> {
    let file = BufReader::with_capacity(1 << 16, File::open(path)?);
    let name = path.to_string_lossy();
    Ok(if name.ends_with(".gz") {
        Box::new(flate2::read::MultiGzDecoder::new(file))
    } else if name.ends_with(".xz") {
        Box::new(liblzma::read::XzDecoder::new_multi_decoder(file))
    } else if name.ends_with(".bz2") {
        Box::new(bzip2::read::MultiBzDecoder::new(file))
    } else {
        bail!("unknown compression: {}", path.display())
    })
}

/// Calls `f` with the path and reader of every regular file in a tarball, the path as stored
/// but for a leading `./`. Returns the total uncompressed size of the regular files, and the
/// directory that every entry is inside, if there is one: unpacking an upstream tarball
/// removes it (see `source_path`), but that is only known once the whole tarball is read.
pub fn walk_tar(
    path: &Path,
    mut f: impl FnMut(&str, u64, &mut dyn Read) -> Result<()>,
) -> Result<(u64, Option<String>)> {
    let mut archive = tar::Archive::new(decompress(path)?);
    let mut top = TopDir::default();
    let mut total = 0u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            continue;
        }
        let full = entry.path()?.to_string_lossy().into_owned();
        let full = full.trim_start_matches("./").trim_end_matches('/');
        if full.is_empty() {
            continue;
        }
        top.observe(full, kind.is_file());
        if !kind.is_file() {
            continue;
        }
        let size = entry.header().size()?;
        total += size;
        f(full, size, &mut entry)?;
    }
    Ok((total, top.get()))
}

/// Tracks whether every entry of a tarball is inside one top-level directory.
#[derive(Default)]
struct TopDir {
    /// `None` before the first entry, then the candidate directory, or `Some(None)` once an
    /// entry outside it has been seen.
    state: Option<Option<String>>,
}

impl TopDir {
    fn observe(&mut self, path: &str, is_file: bool) {
        let (first, nested) = match path.split_once('/') {
            Some((first, _)) => (first, true),
            None => (path, false),
        };
        // A file at the root means there is no top-level directory.
        let inside = nested || !is_file;
        match &self.state {
            None => self.state = Some(inside.then(|| first.to_owned())),
            Some(Some(top)) if !inside || top != first => self.state = Some(None),
            _ => {}
        }
    }

    fn get(self) -> Option<String> {
        self.state.flatten()
    }
}

/// A file's path relative to the source root, as unpacking places it: without the tarball's
/// top-level directory, if it has one, and under `prefix` (the directory a component tarball
/// unpacks into).
pub fn source_path(path: &str, top: Option<&str>, prefix: Option<&str>) -> String {
    let inner = top
        .and_then(|t| path.strip_prefix(t))
        .and_then(|r| r.strip_prefix('/'))
        .unwrap_or(path);
    match prefix {
        Some(prefix) => format!("{prefix}/{inner}"),
        None => inner.to_owned(),
    }
}

/// Extracts `debian/copyright` from a format 1.0 `.diff.gz`, where it appears as a new file.
pub fn copyright_from_diff(path: &Path) -> Result<Option<String>> {
    let reader = BufReader::new(decompress(path)?);
    let mut out: Option<String> = None;
    let mut in_file = false;
    for line in reader.split(b'\n') {
        let line = line?;
        let line = String::from_utf8_lossy(&line);
        if line.starts_with("+++ ") {
            in_file = line
                .split_whitespace()
                .nth(1)
                .is_some_and(|p| p.ends_with("/debian/copyright"));
            if in_file {
                out = Some(String::new());
            }
            continue;
        }
        if line.starts_with("--- ") || line.starts_with("diff ") {
            in_file = false;
            continue;
        }
        if in_file && let (Some(text), Some(added)) = (out.as_mut(), line.strip_prefix('+')) {
            text.push_str(added);
            text.push('\n');
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(entries: &[(&str, bool)]) -> Option<String> {
        let mut top = TopDir::default();
        for &(path, is_file) in entries {
            top.observe(path, is_file);
        }
        top.get()
    }

    #[test]
    fn top_level_directory() {
        let tree = [
            ("pkg-1.0", false),
            ("pkg-1.0/Makefile", true),
            ("pkg-1.0/src/a.c", true),
        ];
        assert_eq!(top(&tree), Some("pkg-1.0".to_owned()));
        // Directories first, then a file at the root: nothing to remove.
        let flat = [("lib", false), ("lib/Makefile", true), ("Makefile", true)];
        assert_eq!(top(&flat), None);
        assert_eq!(top(&[("a/x.c", true), ("b/y.c", true)]), None);
        assert_eq!(top(&[("Makefile", true)]), None);
        assert_eq!(
            source_path("pkg-1.0/src/a.c", Some("pkg-1.0"), None),
            "src/a.c"
        );
        assert_eq!(source_path("lib/a.c", None, Some("extra")), "extra/lib/a.c");
    }

    #[test]
    fn archive_kinds() {
        assert_eq!(
            archive_kind("gawk_5.3.2.orig.tar.xz"),
            Some(ArchiveKind::Orig)
        );
        assert_eq!(
            archive_kind("gawk_5.3.2-1ubuntu1.debian.tar.xz"),
            Some(ArchiveKind::Debian)
        );
        assert_eq!(
            archive_kind("node-foo_1.0.orig-bar-baz.tar.gz"),
            Some(ArchiveKind::Component("bar-baz".to_owned()))
        );
        assert_eq!(
            archive_kind("linux_7.0.0-14.14.diff.gz"),
            Some(ArchiveKind::Diff)
        );
        assert_eq!(
            archive_kind("ubuntu-meta_1.2.tar.xz"),
            Some(ArchiveKind::Native)
        );
        assert_eq!(archive_kind("gawk_5.3.2.orig.tar.xz.asc"), None);
    }
}
