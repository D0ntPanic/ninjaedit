//! Streams the files of compressed tarballs, as source archives hold them.

use anyhow::{Result, bail};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

/// Opens a compressed file for streaming decompression, by extension.
pub fn decompress(path: &Path) -> Result<Box<dyn Read>> {
    let file = BufReader::with_capacity(1 << 16, File::open(path)?);
    let name = path.to_string_lossy();
    Ok(if name.ends_with(".gz") || name.ends_with(".tgz") {
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
pub struct TopDir {
    /// `None` before the first entry, then the candidate directory, or `Some(None)` once an
    /// entry outside it has been seen.
    state: Option<Option<String>>,
}

impl TopDir {
    pub fn observe(&mut self, path: &str, is_file: bool) {
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

    pub fn get(self) -> Option<String> {
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
}
