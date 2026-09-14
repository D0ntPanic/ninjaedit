//! What a commit changed: its message and the people behind it, the
//! files it touched, and the diff of any one of them; and the pieces
//! the [`changes`](super::changes) module builds the same views of the
//! working tree from.
//!
//! A commit is compared with its first parent (a merge shows what it
//! brought in from the second, as `git show` does for the changes on
//! the first parent's side). The [`CommitDetail`] lists the files with
//! how many lines each gained and lost; a [`FileDiff`] has one file's
//! hunks, and both sides of the file in full, so that the context around
//! a hunk can be expanded line by line with [`FileDiff::expand_up`] and
//! [`FileDiff::expand_down`] until two hunks meet. Both sides are lexed
//! whole (see the [`syntax`] module) so that every line of the diff,
//! shown or expanded, is highlighted as it would be in the file.
//!
//! A [`FileDiff`] is built from a libgit2 patch and the [`Contents`] of
//! each side, wherever they come from: a commit's blobs, the index's, or
//! a file in the working directory.
//!
//! [`syntax`]: crate::syntax

use super::history::CommitTime;
use crate::syntax::{Language, Token, lex_text};
use git2::{Delta, DiffFindOptions, DiffLineType, DiffOptions, FileMode, Oid, Patch, Repository};
use std::path::Path;

/// Files bigger than this aren't shown line by line.
const MAX_FILE_SIZE: usize = 8 * 1024 * 1024;
/// With more files changed than this, the lines gained and lost by
/// each aren't counted: it would mean diffing them all up front.
pub(super) const STATS_LIMIT: usize = 500;
/// The lines of context around each change, as git shows them.
pub(super) const CONTEXT_LINES: u32 = 3;
/// How far into a file git looks for a NUL byte to call it binary.
const BINARY_PROBE: usize = 8000;

/// Who made a commit, and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub name: String,
    pub email: String,
    pub time: CommitTime,
}

/// How a commit, or the working tree, changed a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    /// A file became a symlink, or the other way round.
    TypeChanged,
    /// A file in the working directory that git doesn't track yet.
    Untracked,
    /// A file a merge left in conflict, to be resolved and staged.
    Conflicted,
    Other,
}

impl ChangeKind {
    /// The letter git's `--name-status` uses for the change (`?` for an
    /// untracked file and `U` for a conflict, as `git status` has them).
    pub fn letter(self) -> char {
        match self {
            ChangeKind::Added => 'A',
            ChangeKind::Modified => 'M',
            ChangeKind::Deleted => 'D',
            ChangeKind::Renamed => 'R',
            ChangeKind::Copied => 'C',
            ChangeKind::TypeChanged => 'T',
            ChangeKind::Untracked => '?',
            ChangeKind::Conflicted => 'U',
            ChangeKind::Other => '!',
        }
    }

    fn from_delta(delta: Delta) -> ChangeKind {
        match delta {
            Delta::Added => ChangeKind::Added,
            Delta::Deleted => ChangeKind::Deleted,
            Delta::Modified => ChangeKind::Modified,
            Delta::Renamed => ChangeKind::Renamed,
            Delta::Copied => ChangeKind::Copied,
            Delta::Typechange => ChangeKind::TypeChanged,
            Delta::Untracked => ChangeKind::Untracked,
            Delta::Conflicted => ChangeKind::Conflicted,
            _ => ChangeKind::Other,
        }
    }
}

/// One file a commit, or the working tree, changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub kind: ChangeKind,
    /// The file's path after the change, or before it for a deleted file.
    pub path: String,
    /// The path before the change, when it differs (a rename or copy).
    pub old_path: Option<String>,
    /// Lines added and removed, when counted (see [`STATS_LIMIT`]).
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
    /// Whether the "file" is a submodule: the change is to the commit
    /// it points at, and there are no lines to show.
    pub submodule: bool,
}

/// The file at `index` of a diff, as a [`FileChange`]; its lines are
/// counted when `count_stats`, which means loading the patch.
pub(super) fn file_change(
    diff: &git2::Diff<'_>,
    index: usize,
    count_stats: bool,
) -> Result<FileChange, git2::Error> {
    let delta = diff.get_delta(index).expect("a delta of the diff");
    let path = |file: git2::DiffFile<'_>| {
        file.path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let new_path = path(delta.new_file());
    let old_path = path(delta.old_file());
    let kind = ChangeKind::from_delta(delta.status());
    let (path, old_path) = match kind {
        ChangeKind::Deleted => (old_path, None),
        _ if old_path != new_path && !old_path.is_empty() => (new_path, Some(old_path)),
        _ => (new_path, None),
    };
    let submodule = is_submodule(&delta);
    let (additions, deletions, binary) = if count_stats && !submodule {
        match Patch::from_diff(diff, index)? {
            Some(patch) => {
                let (_, additions, deletions) = patch.line_stats()?;
                (additions, deletions, patch.delta().flags().is_binary())
            }
            None => (0, 0, delta.flags().is_binary()),
        }
    } else {
        (0, 0, false)
    };
    Ok(FileChange {
        kind,
        path,
        old_path,
        additions,
        deletions,
        binary,
        submodule,
    })
}

/// Whether either side of a delta is a submodule's commit.
fn is_submodule(delta: &git2::DiffDelta<'_>) -> bool {
    delta.old_file().mode() == FileMode::Commit || delta.new_file().mode() == FileMode::Commit
}

/// Where the delta for `path` is in a diff limited to it: the pathspec
/// may match more than the one file (a directory of the same name,
/// say), so the delta wanted is the one at `path`.
pub(super) fn delta_at(diff: &git2::Diff<'_>, path: &str) -> Result<usize, git2::Error> {
    diff.deltas()
        .position(|delta| {
            let at = |file: git2::DiffFile<'_>| file.path() == Some(Path::new(path));
            at(delta.new_file()) || (delta.status() == Delta::Deleted && at(delta.old_file()))
        })
        .ok_or_else(|| git2::Error::from_str("that file did not change"))
}

/// The bytes of one side of a file, with why they can't be shown line
/// by line if they can't.
pub(super) struct Contents {
    pub bytes: Vec<u8>,
    pub unshown: Option<Unshown>,
}

impl Contents {
    /// Nothing: the side of an added or deleted file that isn't there.
    pub(super) fn empty() -> Contents {
        Contents {
            bytes: Vec::new(),
            unshown: None,
        }
    }

    /// The bytes as read, called binary by git's rule: a NUL byte near
    /// the start.
    pub(super) fn from_bytes(bytes: Vec<u8>) -> Contents {
        let probe = &bytes[..bytes.len().min(BINARY_PROBE)];
        let unshown = if probe.contains(&0) {
            Some(Unshown::Binary)
        } else if bytes.len() > MAX_FILE_SIZE {
            Some(Unshown::TooLarge)
        } else {
            None
        };
        Contents { bytes, unshown }
    }
}

/// The contents of a diff's file as a blob of the repository, or
/// nothing when the side doesn't exist.
pub(super) fn blob_contents(
    repo: &Repository,
    file: git2::DiffFile<'_>,
) -> Result<Contents, git2::Error> {
    if !file.exists() || file.mode() == FileMode::Commit {
        return Ok(Contents::empty());
    }
    let blob = repo.find_blob(file.id())?;
    let unshown = if blob.is_binary() {
        Some(Unshown::Binary)
    } else if blob.size() > MAX_FILE_SIZE {
        Some(Unshown::TooLarge)
    } else {
        None
    };
    Ok(Contents {
        bytes: blob.content().to_vec(),
        unshown,
    })
}

/// The contents of a file in the working directory, or nothing when it
/// isn't there (deleted, or a directory).
pub(super) fn workdir_contents(path: &Path) -> Contents {
    match std::fs::read(path) {
        Ok(bytes) => Contents::from_bytes(bytes),
        Err(_) => Contents::empty(),
    }
}

/// A commit in full, with the files it changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitDetail {
    pub id: Oid,
    pub summary: String,
    /// The whole message, trailing blank lines dropped.
    pub message: String,
    pub author: Person,
    pub committer: Person,
    pub parents: Vec<Oid>,
    pub files: Vec<FileChange>,
}

/// What a commit changed, against its first parent.
pub fn commit_detail(repo: &Repository, id: Oid) -> Result<CommitDetail, git2::Error> {
    let commit = repo.find_commit(id)?;
    let person = |sig: git2::Signature<'_>| Person {
        name: sig.name().unwrap_or("").to_owned(),
        email: sig.email().unwrap_or("").to_owned(),
        time: sig.when().into(),
    };
    let diff = tree_diff(repo, &commit, &[])?;
    let count = diff.deltas().len();
    let mut files = Vec::with_capacity(count);
    for index in 0..count {
        files.push(file_change(&diff, index, count <= STATS_LIMIT)?);
    }
    let message = String::from_utf8_lossy(commit.message_bytes())
        .trim_end()
        .to_owned();
    Ok(CommitDetail {
        id,
        summary: commit
            .summary_bytes()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .unwrap_or_default(),
        message,
        author: person(commit.author()),
        committer: person(commit.committer()),
        parents: commit.parent_ids().collect(),
        files,
    })
}

/// The diff of a commit against its first parent, limited to `paths`
/// when any are given, with renames found.
fn tree_diff<'repo>(
    repo: &'repo Repository,
    commit: &git2::Commit<'repo>,
    paths: &[&str],
) -> Result<git2::Diff<'repo>, git2::Error> {
    let tree = commit.tree()?;
    let parent_tree = match commit.parent(0) {
        Ok(parent) => Some(parent.tree()?),
        Err(_) => None,
    };
    let mut options = DiffOptions::new();
    options.context_lines(CONTEXT_LINES);
    for path in paths {
        options.pathspec(*path);
    }
    let mut diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut options))?;
    let mut find = DiffFindOptions::new();
    find.renames(true);
    diff.find_similar(Some(&mut find))?;
    Ok(diff)
}

/// Which side of a diff a line is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Old,
    New,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// One line of a diff: which side(s) it is on, by line index (from
/// zero) into the old and new contents. A context line is on both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old: Option<usize>,
    pub new: Option<usize>,
}

/// One row of a diff as shown: a line, or the lines hidden between two
/// hunks (or before the first, or after the last) with the ways to
/// reveal them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffRow {
    Line(DiffLine),
    Gap {
        /// Which gap, for [`FileDiff::expand_up`] and friends.
        gap: usize,
        /// How many lines are hidden.
        hidden: usize,
        /// Whether lines can be revealed upward from the hunk below:
        /// there is one.
        up: bool,
        /// Whether lines can be revealed downward from the hunk above.
        down: bool,
    },
}

/// Why a file's lines aren't shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unshown {
    Binary,
    TooLarge,
    /// The "file" is a submodule: what changed is the commit it points
    /// at.
    Submodule,
}

struct Hunk {
    /// The first line of the hunk on each side, from zero, and how many
    /// lines of that side it covers.
    old_first: usize,
    old_count: usize,
    new_first: usize,
    new_count: usize,
    lines: Vec<DiffLine>,
}

/// The hidden lines between two hunks, and how many have been revealed
/// at each end.
#[derive(Clone, Copy, Default)]
struct Gap {
    /// Revealed at the top, continuing the hunk above.
    top: usize,
    /// Revealed at the bottom, leading into the hunk below.
    bottom: usize,
}

/// The diff of one file, with both sides in full.
pub struct FileDiff {
    pub kind: ChangeKind,
    pub path: String,
    pub old_path: Option<String>,
    /// Set when the lines can't be shown.
    pub unshown: Option<Unshown>,
    language: Language,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
    old_tokens: Vec<Vec<Token>>,
    new_tokens: Vec<Vec<Token>>,
    hunks: Vec<Hunk>,
    /// One more than the hunks: before the first, between each pair,
    /// after the last.
    gaps: Vec<Gap>,
}

/// The diff of one file a commit changed; see [`super::History::file_diff`].
pub fn file_diff(
    repo: &Repository,
    id: Oid,
    path: &str,
    old_path: Option<&str>,
) -> Result<FileDiff, git2::Error> {
    let commit = repo.find_commit(id)?;
    let mut paths = vec![path];
    paths.extend(old_path);
    let diff = tree_diff(repo, &commit, &paths)?;
    let index = delta_at(&diff, path)
        .map_err(|_| git2::Error::from_str("the commit did not change that file"))?;
    let delta = diff.get_delta(index).expect("found above");
    let kind = ChangeKind::from_delta(delta.status());
    if is_submodule(&delta) {
        return Ok(FileDiff::unshown(kind, path, old_path, Unshown::Submodule));
    }
    let old = blob_contents(repo, delta.old_file())?;
    let new = blob_contents(repo, delta.new_file())?;
    let patch = Patch::from_diff(&diff, index)?;
    FileDiff::build(kind, path, old_path, &old, &new, patch.as_ref())
}

impl FileDiff {
    /// A diff whose lines can't be shown, for the reason given.
    pub(super) fn unshown(
        kind: ChangeKind,
        path: &str,
        old_path: Option<&str>,
        why: Unshown,
    ) -> FileDiff {
        FileDiff {
            kind,
            path: path.to_owned(),
            old_path: old_path.map(str::to_owned),
            unshown: Some(why),
            language: Language::from_path(Path::new(path)).unwrap_or(Language::Plain),
            old_lines: Vec::new(),
            new_lines: Vec::new(),
            old_tokens: Vec::new(),
            new_tokens: Vec::new(),
            hunks: Vec::new(),
            gaps: vec![Gap::default()],
        }
    }

    /// The diff of a file from the contents of both sides and the patch
    /// between them (none when nothing changed line by line). Both
    /// sides are lexed whole in the language of `path`.
    pub(super) fn build(
        kind: ChangeKind,
        path: &str,
        old_path: Option<&str>,
        old: &Contents,
        new: &Contents,
        patch: Option<&Patch<'_>>,
    ) -> Result<FileDiff, git2::Error> {
        // A binary side rules: nothing of it can be shown at all.
        let unshown = match (old.unshown, new.unshown) {
            (Some(Unshown::Binary), _) | (_, Some(Unshown::Binary)) => Some(Unshown::Binary),
            (Some(why), _) | (_, Some(why)) => Some(why),
            (None, None) => None,
        };
        let language = Language::from_path(Path::new(path)).unwrap_or(Language::Plain);
        let mut file = FileDiff {
            kind,
            path: path.to_owned(),
            old_path: old_path.map(str::to_owned),
            unshown,
            language,
            old_lines: Vec::new(),
            new_lines: Vec::new(),
            old_tokens: Vec::new(),
            new_tokens: Vec::new(),
            hunks: Vec::new(),
            gaps: vec![Gap::default()],
        };
        if unshown.is_some() {
            return Ok(file);
        }
        let old_text = String::from_utf8_lossy(&old.bytes);
        let new_text = String::from_utf8_lossy(&new.bytes);
        file.old_lines = split_lines(&old_text);
        file.new_lines = split_lines(&new_text);
        if language != Language::Plain {
            file.old_tokens = lex_text(language, &old_text);
            file.new_tokens = lex_text(language, &new_text);
        }

        let Some(patch) = patch else {
            return Ok(file);
        };
        for h in 0..patch.num_hunks() {
            let (hunk, count) = patch.hunk(h)?;
            let mut lines = Vec::with_capacity(count);
            for l in 0..count {
                let line = patch.line_in_hunk(h, l)?;
                let kind = match line.origin_value() {
                    DiffLineType::Context => LineKind::Context,
                    DiffLineType::Addition => LineKind::Added,
                    DiffLineType::Deletion => LineKind::Removed,
                    _ => continue,
                };
                lines.push(DiffLine {
                    kind,
                    old: line.old_lineno().map(|n| n as usize - 1),
                    new: line.new_lineno().map(|n| n as usize - 1),
                });
            }
            // A hunk with no lines on a side names the line before it.
            let first = |start: u32, count: u32| {
                if count == 0 {
                    start as usize
                } else {
                    start as usize - 1
                }
            };
            file.hunks.push(Hunk {
                old_first: first(hunk.old_start(), hunk.old_lines()),
                old_count: hunk.old_lines() as usize,
                new_first: first(hunk.new_start(), hunk.new_lines()),
                new_count: hunk.new_lines() as usize,
                lines,
            });
            file.gaps.push(Gap::default());
        }
        Ok(file)
    }
}

/// The lines of a text, without their terminators; a final terminator
/// doesn't start an empty line.
fn split_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
        .collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

impl FileDiff {
    pub fn language(&self) -> Language {
        self.language
    }

    /// How many lines each side of the file has.
    pub fn old_line_count(&self) -> usize {
        self.old_lines.len()
    }

    pub fn new_line_count(&self) -> usize {
        self.new_lines.len()
    }

    /// Whether the diff has any hunks: none for a file whose contents
    /// didn't change (a mode change, an empty file added).
    pub fn has_hunks(&self) -> bool {
        !self.hunks.is_empty()
    }

    /// The text of a line, from whichever side has it.
    pub fn text(&self, line: &DiffLine) -> &str {
        match (line.new, line.old) {
            (Some(new), _) => self.new_lines.get(new).map_or("", String::as_str),
            (None, Some(old)) => self.old_lines.get(old).map_or("", String::as_str),
            (None, None) => "",
        }
    }

    /// The syntax tokens of a line, from whichever side has it; empty
    /// when the file's language isn't known.
    pub fn tokens(&self, line: &DiffLine) -> &[Token] {
        match (line.new, line.old) {
            (Some(new), _) => self.new_tokens.get(new).map_or(&[], Vec::as_slice),
            (None, Some(old)) => self.old_tokens.get(old).map_or(&[], Vec::as_slice),
            (None, None) => &[],
        }
    }

    /// The lines of a side, for showing a file whole.
    pub fn lines(&self, side: Side) -> &[String] {
        match side {
            Side::Old => &self.old_lines,
            Side::New => &self.new_lines,
        }
    }

    /// Where gap `gap` starts on each side and how many lines it holds.
    fn gap_extent(&self, gap: usize) -> (usize, usize, usize) {
        let (old_start, new_start) = match gap.checked_sub(1).and_then(|h| self.hunks.get(h)) {
            Some(hunk) => (
                hunk.old_first + hunk.old_count,
                hunk.new_first + hunk.new_count,
            ),
            None => (0, 0),
        };
        let (old_end, new_end) = match self.hunks.get(gap) {
            Some(hunk) => (hunk.old_first, hunk.new_first),
            None => (self.old_lines.len(), self.new_lines.len()),
        };
        let size = new_end
            .saturating_sub(new_start)
            .min(old_end.saturating_sub(old_start));
        (old_start, new_start, size)
    }

    /// The rows to show: every hunk's lines, with the context revealed
    /// around them and a [`DiffRow::Gap`] wherever lines are hidden.
    pub fn rows(&self) -> Vec<DiffRow> {
        let mut rows = Vec::new();
        let context = |rows: &mut Vec<DiffRow>, old: usize, new: usize, count: usize| {
            for i in 0..count {
                rows.push(DiffRow::Line(DiffLine {
                    kind: LineKind::Context,
                    old: Some(old + i),
                    new: Some(new + i),
                }));
            }
        };
        for (g, gap) in self.gaps.iter().enumerate() {
            let (old_start, new_start, size) = self.gap_extent(g);
            let top = gap.top.min(size);
            let bottom = gap.bottom.min(size - top);
            let hidden = size - top - bottom;
            context(&mut rows, old_start, new_start, top);
            if hidden > 0 {
                rows.push(DiffRow::Gap {
                    gap: g,
                    hidden,
                    up: g < self.hunks.len(),
                    down: g > 0,
                });
            }
            let from = size - bottom;
            context(&mut rows, old_start + from, new_start + from, bottom);
            if let Some(hunk) = self.hunks.get(g) {
                rows.extend(hunk.lines.iter().copied().map(DiffRow::Line));
            }
        }
        rows
    }

    /// Reveal up to `count` more hidden lines of a gap above the hunk
    /// below it.
    pub fn expand_up(&mut self, gap: usize, count: usize) {
        if gap < self.hunks.len() {
            self.reveal(gap, 0, count);
        }
    }

    /// Reveal up to `count` more hidden lines of a gap below the hunk
    /// above it.
    pub fn expand_down(&mut self, gap: usize, count: usize) {
        if gap > 0 {
            self.reveal(gap, count, 0);
        }
    }

    /// Reveal all of a gap's lines.
    pub fn expand_all(&mut self, gap: usize) {
        let (_, _, size) = self.gap_extent(gap);
        self.reveal(gap, size, 0);
    }

    fn reveal(&mut self, gap: usize, top: usize, bottom: usize) {
        let (_, _, size) = self.gap_extent(gap);
        let Some(entry) = self.gaps.get_mut(gap) else {
            return;
        };
        entry.top = (entry.top + top).min(size);
        entry.bottom = (entry.bottom + bottom).min(size - entry.top);
    }
}

#[cfg(test)]
mod tests {
    use super::super::history::tests::{TestRepo, open};
    use super::*;
    use crate::syntax::TokenKind;

    fn lines(file: &FileDiff) -> Vec<String> {
        file.rows()
            .iter()
            .map(|row| match row {
                DiffRow::Line(line) => {
                    let marker = match line.kind {
                        LineKind::Context => ' ',
                        LineKind::Added => '+',
                        LineKind::Removed => '-',
                    };
                    format!("{marker}{}", file.text(line))
                }
                DiffRow::Gap {
                    hidden, up, down, ..
                } => format!(
                    "~{hidden}{}{}",
                    if *up { "^" } else { "" },
                    if *down { "v" } else { "" }
                ),
            })
            .collect()
    }

    #[test]
    fn a_commit_lists_its_files_with_counts_and_kinds() {
        let mut repo = TestRepo::new();
        let a = repo.commit(
            &[
                ("keep.rs", "fn a() {}\n"),
                ("old.txt", "x\n"),
                ("gone.txt", "bye\n"),
            ],
            "Base",
            &[],
        );
        std::fs::rename(repo.path().join("old.txt"), repo.path().join("new.txt")).unwrap();
        let mut index = repo.repo.index().unwrap();
        index.remove_path(Path::new("old.txt")).unwrap();
        index.remove_path(Path::new("gone.txt")).unwrap();
        std::fs::remove_file(repo.path().join("gone.txt")).unwrap();
        index.write().unwrap();
        let b = repo.commit(
            &[
                ("keep.rs", "fn a() {}\nfn b() {}\n"),
                ("new.txt", "x\n"),
                ("added.md", "# Hi\n"),
            ],
            "Change things\n\nMore about it.\n\n",
            &[a],
        );
        let history = open(&repo);
        let detail = history.detail(b).unwrap();
        assert_eq!(detail.summary, "Change things");
        assert_eq!(detail.message, "Change things\n\nMore about it.");
        assert_eq!(detail.author.name, "Test Author");
        assert_eq!(detail.author.email, "test@example.com");
        assert_eq!(detail.parents, vec![a]);
        let files: Vec<(char, &str, Option<&str>, usize, usize)> = detail
            .files
            .iter()
            .map(|f| {
                (
                    f.kind.letter(),
                    f.path.as_str(),
                    f.old_path.as_deref(),
                    f.additions,
                    f.deletions,
                )
            })
            .collect();
        assert_eq!(
            files,
            vec![
                ('A', "added.md", None, 1, 0),
                ('D', "gone.txt", None, 0, 1),
                ('M', "keep.rs", None, 1, 0),
                ('R', "new.txt", Some("old.txt"), 0, 0),
            ]
        );
        assert!(!detail.files[0].binary);

        // The first commit is against nothing: everything is added.
        let detail = history.detail(a).unwrap();
        assert_eq!(detail.parents, vec![]);
        assert!(detail.files.iter().all(|f| f.kind == ChangeKind::Added));
        assert_eq!(detail.files.len(), 3);
    }

    #[test]
    fn a_file_diff_has_hunks_with_expandable_context() {
        let mut repo = TestRepo::new();
        let old: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        let a = repo.commit(&[("f.txt", &old)], "Base", &[]);
        let new: String = (1..=30)
            .map(|n| match n {
                5 => "line five\n".to_owned(),
                20 => "line 20\nline 20b\n".to_owned(),
                _ => format!("line {n}\n"),
            })
            .collect();
        let b = repo.commit(&[("f.txt", &new)], "Edit", &[a]);
        let history = open(&repo);
        let mut file = history.file_diff(b, "f.txt", None).unwrap();
        assert_eq!(file.kind, ChangeKind::Modified);
        assert_eq!(file.unshown, None);
        assert_eq!(file.old_line_count(), 30);
        assert_eq!(file.new_line_count(), 31);
        assert!(file.has_hunks());
        assert_eq!(
            lines(&file),
            [
                "~1^",
                " line 2",
                " line 3",
                " line 4",
                "-line 5",
                "+line five",
                " line 6",
                " line 7",
                " line 8",
                "~9^v",
                " line 18",
                " line 19",
                " line 20",
                "+line 20b",
                " line 21",
                " line 22",
                " line 23",
                "~7v",
            ]
        );
        // Revealing from the top of the middle gap continues the first
        // hunk; from the bottom, leads into the second.
        file.expand_down(1, 3);
        let rows = lines(&file);
        assert_eq!(
            &rows[9..14],
            [" line 9", " line 10", " line 11", "~6^v", " line 18"]
        );
        file.expand_up(1, 3);
        let rows = lines(&file);
        assert_eq!(&rows[12..16], ["~3^v", " line 15", " line 16", " line 17"]);
        // Revealing more than is left reveals what is left, and the gap
        // is gone.
        file.expand_up(1, 10);
        let rows = lines(&file);
        assert_eq!(&rows[12..15], [" line 12", " line 13", " line 14"]);
        assert!(!rows.iter().any(|r| r.starts_with("~") && r.contains("^v")));
        // The first gap only expands upward and the last only downward;
        // the wrong direction does nothing.
        file.expand_down(0, 5);
        assert_eq!(lines(&file)[0], "~1^");
        file.expand_up(0, 5);
        assert_eq!(lines(&file)[0], " line 1");
        file.expand_up(2, 5);
        assert_eq!(lines(&file).last().unwrap(), "~7v");
        file.expand_all(2);
        assert_eq!(lines(&file).last().unwrap(), " line 30");
        assert_eq!(
            lines(&file).len(),
            32,
            "every new line once, plus the removed one"
        );
    }

    #[test]
    fn added_deleted_renamed_and_binary_files() {
        let mut repo = TestRepo::new();
        let a = repo.commit(
            &[
                ("a.rs", "fn main() {}\n"),
                ("old.txt", "same\n"),
                ("bin", "\0\x01\x02"),
            ],
            "Base",
            &[],
        );
        let history = open(&repo);
        let file = history.file_diff(a, "a.rs", None).unwrap();
        assert_eq!(file.kind, ChangeKind::Added);
        assert_eq!(lines(&file), ["+fn main() {}"]);
        assert_eq!(file.language(), Language::Rust);
        let DiffRow::Line(line) = file.rows()[0] else {
            panic!()
        };
        assert_eq!(file.tokens(&line)[0].kind, TokenKind::Keyword);
        let file = history.file_diff(a, "bin", None).unwrap();
        assert_eq!(file.unshown, Some(Unshown::Binary));
        assert!(!file.has_hunks());

        let b = repo.remove("a.rs", "Remove", a);
        let history = open(&repo);
        let file = history.file_diff(b, "a.rs", None).unwrap();
        assert_eq!(file.kind, ChangeKind::Deleted);
        assert_eq!(lines(&file), ["-fn main() {}"]);
        assert_eq!(
            file.tokens(&match file.rows()[0] {
                DiffRow::Line(line) => line,
                _ => panic!(),
            })[0]
                .kind,
            TokenKind::Keyword
        );
        assert!(history.file_diff(b, "nope.txt", None).is_err());

        std::fs::rename(repo.path().join("old.txt"), repo.path().join("new.txt")).unwrap();
        let mut index = repo.repo.index().unwrap();
        index.remove_path(Path::new("old.txt")).unwrap();
        index.write().unwrap();
        let c = repo.commit(&[("new.txt", "same\n")], "Rename", &[b]);
        let history = open(&repo);
        let detail = history.detail(c).unwrap();
        assert_eq!(detail.files[0].kind, ChangeKind::Renamed);
        let file = history.file_diff(c, "new.txt", Some("old.txt")).unwrap();
        assert_eq!(file.kind, ChangeKind::Renamed);
        assert!(!file.has_hunks(), "the contents didn't change");
        assert_eq!(file.lines(Side::New), ["same"]);
    }
}
