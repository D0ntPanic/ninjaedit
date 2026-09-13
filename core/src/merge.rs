//! Three-way merging of file contents, for bringing changes made to a file
//! on disk into a buffer that has unsaved edits of its own.
//!
//! The merge is the one git does for files: given the contents both sides
//! started from, the buffer's edits and the on-disk changes are combined
//! line by line, and where they overlap the result carries git's conflict
//! markers for the user to resolve. libgit2 does the work through
//! [`git2::merge_file`], which needs no repository.
//!
//! [`LineMap`] is the other half of replacing a buffer's contents: it says
//! where each line of the old contents ended up in the new, so that the
//! cursor can stay on the text it was on.

use git2::{DiffOptions, MergeFileInput, MergeFileOptions, Patch};
use std::sync::Once;

/// How many leading bytes to look at for a NUL when deciding whether
/// contents are binary; the same as git's heuristic.
const BINARY_PROBE: usize = 8000;

/// The result of a [`merge`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merged {
    /// The merged contents. With `conflicts`, they include git-style
    /// conflict markers around each overlapping change.
    pub content: Vec<u8>,
    /// Whether any changes overlapped and could not be merged cleanly.
    pub conflicts: bool,
}

/// Whether contents look binary: a NUL byte near the start, as git decides.
/// Binary contents can't be merged line by line.
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_PROBE)].contains(&0)
}

/// Merge `ours` and `theirs`, both derived from `base`, as git would merge
/// two branches' versions of a file. Overlapping changes are kept as
/// conflicts with markers, labelled "editor" for our side and "disk" for
/// theirs. Returns `None` when any of the three is binary.
pub fn merge(base: &[u8], ours: &[u8], theirs: &[u8]) -> Option<Merged> {
    if is_binary(base) || is_binary(ours) || is_binary(theirs) {
        return None;
    }
    init_libgit2();
    let input = |content| {
        let mut input = MergeFileInput::new();
        input.content(content);
        input
    };
    let mut opts = MergeFileOptions::new();
    opts.ancestor_label("original")
        .our_label("editor")
        .their_label("disk")
        .style_zdiff3(true);
    let result =
        git2::merge_file(&input(base), &input(ours), &input(theirs), Some(&mut opts)).ok()?;
    Some(Merged {
        content: result.content().to_vec(),
        conflicts: !result.is_automergeable(),
    })
}

/// The byte offset of the first conflict marker line in merged contents, if
/// any: the start of a line beginning `<<<<<<<`.
pub fn first_conflict(content: &[u8]) -> Option<usize> {
    const MARKER: &[u8] = b"<<<<<<<";
    let mut start = 0;
    loop {
        if content[start..].starts_with(MARKER) {
            return Some(start);
        }
        let rest = &content[start..];
        let next = rest.iter().position(|&b| b == b'\n')?;
        start += next + 1;
    }
}

/// Where the lines of one version of some contents ended up in another,
/// computed from the line diff between the two.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineMap {
    hunks: Vec<Hunk>,
}

/// One changed region of a diff, as zero-based line ranges: `old_lines` of
/// the old contents starting at `old_start` became `new_lines` of the new
/// starting at `new_start`. Either count may be zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Hunk {
    old_start: usize,
    old_lines: usize,
    new_start: usize,
    new_lines: usize,
}

impl LineMap {
    /// Diff `old` against `new`. Binary contents, or a failed diff, give an
    /// identity map.
    pub fn new(old: &[u8], new: &[u8]) -> LineMap {
        if is_binary(old) || is_binary(new) {
            return LineMap::default();
        }
        init_libgit2();
        let mut opts = DiffOptions::new();
        opts.context_lines(0);
        let Ok(patch) = Patch::from_buffers(old, None, new, None, Some(&mut opts)) else {
            return LineMap::default();
        };
        let hunks = (0..patch.num_hunks())
            .filter_map(|i| patch.hunk(i).ok())
            .map(|(hunk, _)| Hunk {
                old_start: zero_based(hunk.old_start(), hunk.old_lines()),
                old_lines: hunk.old_lines() as usize,
                new_start: zero_based(hunk.new_start(), hunk.new_lines()),
                new_lines: hunk.new_lines() as usize,
            })
            .collect();
        LineMap { hunks }
    }

    /// The line of the new contents holding what was on `line` of the old.
    /// A line that was changed maps to the corresponding line of its
    /// replacement, or to the line after a deletion.
    pub fn map(&self, line: usize) -> usize {
        let mut delta = 0isize;
        for hunk in &self.hunks {
            if line < hunk.old_start {
                break;
            }
            if line < hunk.old_start + hunk.old_lines {
                let within = (line - hunk.old_start).min(hunk.new_lines.saturating_sub(1));
                return hunk.new_start + within;
            }
            delta = (hunk.new_start + hunk.new_lines) as isize
                - (hunk.old_start + hunk.old_lines) as isize;
        }
        (line as isize + delta).max(0) as usize
    }
}

/// A hunk's one-based start line as a zero-based one. A hunk with no lines
/// on a side names the line *before* the change on that side instead, so
/// its one-based number is already the zero-based index of the change.
fn zero_based(start: u32, lines: u32) -> usize {
    if lines == 0 {
        start as usize
    } else {
        start as usize - 1
    }
}

/// libgit2 needs its global setup done once before use. Repository calls
/// do it themselves, but the merge and diff entry points used here don't.
fn init_libgit2() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = git2::Config::new();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_changes_to_different_lines() {
        let base = b"a\nb\nc\nd\n";
        let merged = merge(base, b"A\nb\nc\nd\n", b"a\nb\nc\nD\n").unwrap();
        assert!(!merged.conflicts);
        assert_eq!(merged.content, b"A\nb\nc\nD\n");
    }

    #[test]
    fn overlapping_changes_conflict_with_markers() {
        let base = b"a\nb\nc\n";
        let merged = merge(base, b"a\nours\nc\n", b"a\ntheirs\nc\n").unwrap();
        assert!(merged.conflicts);
        let text = String::from_utf8(merged.content.clone()).unwrap();
        assert!(text.starts_with("a\n<<<<<<< editor\nours\n"), "{text}");
        assert!(
            text.contains("=======\ntheirs\n>>>>>>> disk\nc\n"),
            "{text}"
        );
        assert_eq!(first_conflict(&merged.content), Some(2));
    }

    #[test]
    fn binary_contents_are_not_merged() {
        assert!(merge(b"a\n", b"a\n\0", b"b\n").is_none());
        assert!(!is_binary(b"plain\ntext\n"));
        assert_eq!(first_conflict(b"no markers\n"), None);
    }

    #[test]
    fn line_map_follows_insertions_and_deletions() {
        let old = b"a\nb\nc\nd\ne\n";
        // Two lines inserted before "c"; "e" deleted.
        let map = LineMap::new(old, b"a\nb\nx\ny\nc\nd\n");
        assert_eq!(map.map(0), 0);
        assert_eq!(map.map(1), 1);
        assert_eq!(map.map(2), 4);
        assert_eq!(map.map(3), 5);
        // The deleted line maps to where it was, now past the end.
        assert_eq!(map.map(4), 6);
        // Past the end of the old contents still shifts by the net change.
        assert_eq!(map.map(5), 6);
    }

    #[test]
    fn line_map_of_a_replacement_stays_within_it() {
        let old = b"a\nb\nc\nd\n";
        // "b" and "c" replaced by one line.
        let map = LineMap::new(old, b"a\nz\nd\n");
        assert_eq!(map.map(1), 1);
        assert_eq!(map.map(2), 1);
        assert_eq!(map.map(3), 2);
        // Identical contents map every line to itself.
        assert_eq!(LineMap::new(old, old).map(3), 3);
        // Insertion at the very start.
        assert_eq!(LineMap::new(b"a\n", b"x\na\n").map(0), 1);
    }
}
