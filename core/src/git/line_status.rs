//! What has happened to each line of an editor's buffer since the last
//! commit, for marking changed lines beside the text.
//!
//! A [`LineStatuses`] keeps a [`LineStatus`] for every line of a buffer:
//! unchanged, added, modified, or the line just after some that were
//! deleted (the buffer only has the lines that are there, so a deletion
//! is marked where it happened). The comparison is between the file as
//! HEAD has it and the buffer as it is in memory, unsaved edits and all.
//! A file in no repository, one git ignores, and a binary one have no
//! statuses; a file HEAD doesn't have, untracked or only staged, is all
//! added.
//!
//! # Keeping the statuses current
//!
//! Diffing a large file takes too long to do on every keystroke, so it is
//! done on a worker thread, over a [`BufferSnapshot`], with one diff out
//! at a time. A status is `None` until the first diff comes back, and a
//! frontend draws nothing for it. Asking for statuses
//! ([`LineStatuses::statuses`]) sends the worker the buffer as it is if
//! it has changed since the last diff and the worker is free; the worker
//! bumps [`LineStatuses::generation`] when it has something new, so a
//! frontend knows to redraw (and so to ask again).
//!
//! An edit is reported as some lines replaced by others (see
//! [`LinesEdit`], whose [`between`](LinesEdit::between) leaves out the
//! lines at either end that came out as they were). The statuses of later
//! lines move with them, and the replaced lines get a guess at once:
//! lines in place of nothing are added, lines in place of only added ones
//! are still added, other replacements are modified, and lines replaced
//! by nothing leave a deletion on the line after. Edits made while a diff
//! is out are kept and made again over its result when it comes back, so
//! the result fits the buffer as it is by then and the guesses for the
//! newest edits stay until the next diff settles them.
//!
//! Lines are numbered as the buffer numbers them, with any of `\n`,
//! `\r\n`, or a lone `\r` ending one; the HEAD side is split the same way,
//! so a change of line endings alone is not a change. The empty line the
//! buffer has after a final line break is not a line to git: it is only
//! ever unchanged or marked for a deletion at the end of the file.

use crate::buffer::{BufferSnapshot, FileBuffer};
use crate::merge;
use git2::{ErrorCode, ObjectType, Oid, Repository};
use std::borrow::Cow;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
use std::{fs, thread};

/// Files bigger than this, on either side, have no statuses.
const MAX_FILE_SIZE: usize = 16 * 1024 * 1024;

/// What happened to a line since the last commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineStatus {
    Unchanged,
    /// The line is new.
    Added,
    /// The line replaces one or more that were there.
    Modified,
    /// The line is as it was, but lines just above it were deleted.
    DeletedAbove,
}

/// A change to a buffer's lines: `old_count` lines starting at `start`
/// were replaced by `new_count` lines. Either count may be zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinesEdit {
    pub start: usize,
    pub old_count: usize,
    pub new_count: usize,
}

impl LinesEdit {
    /// The edit that replaced the lines `old` with `new`, both starting at
    /// line `start` and running through the end of their last line's
    /// terminator (or the end of the buffer), with the lines both begin
    /// and end with left out. Lines are compared without regard to which
    /// terminator ends them, but a line with none (the last of a file
    /// without a final line break) differs from the same line with one.
    pub fn between(start: usize, old: &[u8], new: &[u8]) -> LinesEdit {
        let old = git_lines(old);
        let new = git_lines(new);
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        LinesEdit {
            start: start + prefix,
            old_count: old.len() - prefix - suffix,
            new_count: new.len() - prefix - suffix,
        }
    }
}

/// The lines of `bytes` as git counts them: each line's content and
/// whether a terminator ends it. Text after the last terminator is a line
/// only if there is some.
fn git_lines(bytes: &[u8]) -> Vec<(&[u8], bool)> {
    let mut lines = Vec::new();
    let mut begin = 0;
    let mut i = 0;
    while i < bytes.len() {
        match break_len(bytes, i) {
            Some(len) => {
                lines.push((&bytes[begin..i], true));
                i += len;
                begin = i;
            }
            None => i += 1,
        }
    }
    if begin < bytes.len() {
        lines.push((&bytes[begin..], false));
    }
    lines
}

/// The length of the line break at `bytes[i]`, as the buffer counts line
/// breaks, if there is one.
fn break_len(bytes: &[u8], i: usize) -> Option<usize> {
    match bytes[i] {
        b'\n' => Some(1),
        b'\r' if bytes.get(i + 1) == Some(&b'\n') => Some(2),
        b'\r' => Some(1),
        _ => None,
    }
}

/// The statuses and the bookkeeping of what is out with the worker.
struct Shared {
    /// One per line of the buffer; `None` until a diff says.
    statuses: Vec<Option<LineStatus>>,
    /// Whether the last diff found something to compare with, so that
    /// edits are guessed at rather than left unknown.
    tracking: bool,
    /// Bumped by every edit.
    version: u64,
    /// The version the last diff merged was of.
    diffed: Option<u64>,
    /// Whether the worker has a diff to do.
    in_flight: bool,
    /// The edits made since the snapshot the worker has, with the
    /// buffer's line count after each, to make again over its result.
    edits: Vec<(LinesEdit, usize)>,
    /// Whether HEAD should be looked at again even if the buffer hasn't
    /// changed.
    recheck: bool,
}

/// What the worker found.
enum Outcome {
    /// Nothing changed since the last diff it did.
    Unchanged,
    /// The statuses of the snapshot's lines, or `None` with nothing to
    /// compare it with.
    Statuses(Option<Vec<LineStatus>>),
}

struct Inner {
    shared: Mutex<Shared>,
    /// Signalled whenever the worker merges a result.
    merged: Condvar,
    generation: AtomicU64,
}

/// What the worker gets to diff.
struct Job {
    snapshot: BufferSnapshot,
    version: u64,
    path: PathBuf,
}

/// The line statuses of one buffer; see the [module documentation](self).
pub struct LineStatuses {
    inner: Arc<Inner>,
    /// The worker's queue, made with the worker on the first diff, so a
    /// buffer never compared with anything costs no thread.
    jobs: OnceLock<Sender<Job>>,
}

impl LineStatuses {
    /// Statuses for a buffer of `line_count` lines, all unknown until
    /// the first diff.
    pub fn new(line_count: usize) -> LineStatuses {
        LineStatuses {
            inner: Arc::new(Inner {
                shared: Mutex::new(Shared {
                    statuses: vec![None; line_count.max(1)],
                    tracking: false,
                    version: 0,
                    diffed: None,
                    in_flight: false,
                    edits: Vec::new(),
                    recheck: false,
                }),
                merged: Condvar::new(),
                generation: AtomicU64::new(0),
            }),
            jobs: OnceLock::new(),
        }
    }

    /// A counter that changes whenever the worker has updated the
    /// statuses, or finished while there is more to do, so a frontend
    /// knows to redraw.
    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::Acquire)
    }

    /// Whether an edit's lines are worth comparing to make a guess with
    /// (see [`LinesEdit::between`]): when there are statuses to guess
    /// at, or soon will be.
    pub fn wants_edit_text(&self) -> bool {
        let shared = self.inner.shared.lock().unwrap();
        shared.tracking || shared.in_flight
    }

    /// Record an edit, leaving `line_count` lines in the buffer.
    pub fn lines_changed(&self, edit: LinesEdit, line_count: usize) {
        let mut shared = self.inner.shared.lock().unwrap();
        let tracking = shared.tracking;
        apply_edit(&mut shared.statuses, edit, tracking, line_count);
        shared.version += 1;
        if shared.in_flight {
            shared.edits.push((edit, line_count));
        }
    }

    /// The statuses of `lines`, `None` for any not known. `buffer` must
    /// be the buffer the statuses have been kept in step with; if it has
    /// changed since the last diff, a new one is started when the worker
    /// is free.
    pub fn statuses(&self, buffer: &FileBuffer, lines: Range<usize>) -> Vec<Option<LineStatus>> {
        self.request(buffer);
        let shared = self.inner.shared.lock().unwrap();
        let end = lines.end.min(shared.statuses.len());
        shared.statuses[lines.start.min(end)..end].to_vec()
    }

    /// Diff again even if the buffer hasn't changed, for when HEAD may
    /// have moved (a commit, a checkout) or the buffer's file has a new
    /// name. Cheap when nothing has: the worker only looks up the file in
    /// HEAD's tree and finds the same blob.
    pub fn refresh(&self, buffer: &FileBuffer) {
        self.inner.shared.lock().unwrap().recheck = true;
        self.request(buffer);
    }

    /// Wait up to `timeout` for the statuses to be of `buffer` as it is
    /// now. Returns whether they are. For tests, and anything else that
    /// can't redraw when the worker is done.
    pub fn wait(&self, buffer: &FileBuffer, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.request(buffer);
            let shared = self.inner.shared.lock().unwrap();
            if buffer.path().is_none() || (!shared.in_flight && shared.is_settled()) {
                return true;
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            let _ = self.inner.merged.wait_timeout(shared, left).unwrap();
        }
    }

    /// Send the worker the buffer if it has changed (or HEAD should be
    /// looked at again) and the worker is free.
    fn request(&self, buffer: &FileBuffer) {
        let Some(path) = buffer.path() else {
            return;
        };
        let mut shared = self.inner.shared.lock().unwrap();
        if shared.in_flight || shared.is_settled() {
            return;
        }
        let jobs = self.jobs.get_or_init(|| {
            let (jobs, rx) = mpsc::channel();
            let inner = Arc::clone(&self.inner);
            thread::Builder::new()
                .name("line-status".to_owned())
                .spawn(move || run_worker(rx, inner))
                .expect("spawn line status thread");
            jobs
        });
        let job = Job {
            snapshot: buffer.snapshot(),
            version: shared.version,
            path: path.to_owned(),
        };
        if jobs.send(job).is_ok() {
            shared.in_flight = true;
            shared.recheck = false;
            shared.edits.clear();
        }
    }
}

impl Shared {
    /// Whether the statuses are of the buffer as it is, and nothing asked
    /// for them to be looked at again.
    fn is_settled(&self) -> bool {
        !self.recheck && self.diffed == Some(self.version)
    }

    /// Take in the worker's outcome for the snapshot of `version`. Returns
    /// whether a frontend should redraw: the statuses changed, or the
    /// buffer has since, and drawing asks for another diff.
    fn merge(&mut self, version: u64, outcome: Outcome) -> bool {
        self.in_flight = false;
        self.diffed = Some(version);
        let edits = std::mem::take(&mut self.edits);
        let changed = match outcome {
            Outcome::Unchanged => false,
            Outcome::Statuses(None) => {
                let had = self.tracking || self.statuses.iter().any(Option::is_some);
                self.statuses.fill(None);
                self.tracking = false;
                had
            }
            Outcome::Statuses(Some(statuses)) => {
                let mut statuses: Vec<Option<LineStatus>> =
                    statuses.into_iter().map(Some).collect();
                for (edit, line_count) in edits {
                    apply_edit(&mut statuses, edit, true, line_count);
                }
                if statuses.len() == self.statuses.len() {
                    self.statuses = statuses;
                    self.tracking = true;
                } else {
                    // Shouldn't happen; diff again rather than show lines
                    // out of place.
                    self.diffed = None;
                }
                true
            }
        };
        changed || self.version != version
    }
}

/// Make an edit to `statuses`, guessing at the replaced lines when
/// `tracking` (see the [module documentation](self)) and leaving them
/// unknown otherwise, then fit the statuses to `line_count` lines. The
/// edit's counts are git's, which leave out the buffer's empty last line
/// after a final line break, so an edit that makes or removes that line
/// leaves the statuses a line off: one is added or taken at the end.
fn apply_edit(
    statuses: &mut Vec<Option<LineStatus>>,
    edit: LinesEdit,
    tracking: bool,
    line_count: usize,
) {
    let start = edit.start.min(statuses.len());
    let end = (start + edit.old_count).min(statuses.len());
    let unchanged = tracking.then_some(LineStatus::Unchanged);
    let replaced: Vec<Option<LineStatus>> = statuses[start..end].to_vec();
    // Lines that were only ever added are still added when replaced, and
    // deleting them leaves nothing to mark.
    let only_added = replaced.iter().all(|s| *s == Some(LineStatus::Added));
    let fill = if !tracking {
        None
    } else if only_added {
        Some(LineStatus::Added)
    } else {
        Some(LineStatus::Modified)
    };
    statuses.splice(start..end, std::iter::repeat_n(fill, edit.new_count));
    if tracking && edit.new_count == 0 && !replaced.is_empty() && !only_added {
        let after = start.min(statuses.len().saturating_sub(1));
        if let Some(status) = statuses.get_mut(after)
            && *status == unchanged
        {
            *status = Some(LineStatus::DeletedAbove);
        }
    }
    statuses.resize(line_count.max(1), unchanged);
}

fn run_worker(rx: Receiver<Job>, inner: Arc<Inner>) {
    let mut worker = Worker::default();
    while let Ok(job) = rx.recv() {
        let outcome = worker.diff(&job);
        let redraw = inner.shared.lock().unwrap().merge(job.version, outcome);
        if redraw {
            inner.generation.fetch_add(1, Ordering::Release);
        }
        inner.merged.notify_all();
    }
}

/// What the buffer is compared with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Base {
    /// Nothing: no repository, an ignored or binary file, and so on.
    None,
    /// HEAD doesn't have the file, so all of it is new.
    Empty,
    /// HEAD's blob of the file.
    Blob(Oid),
}

/// The worker's memory between diffs.
#[derive(Default)]
struct Worker {
    /// The file last diffed, and its repository with its path there,
    /// if it is in one.
    file: Option<(PathBuf, Option<(Repository, PathBuf)>)>,
    /// The contents of the last blob read, with line breaks made `\n`.
    blob: Option<(Oid, Vec<u8>)>,
    /// The version and base of the last diff, to skip one that would
    /// come out the same.
    last: Option<(u64, Base)>,
}

impl Worker {
    fn diff(&mut self, job: &Job) -> Outcome {
        let base = self.base(&job.path);
        if self.last == Some((job.version, base)) {
            return Outcome::Unchanged;
        }
        self.last = Some((job.version, base));
        let statuses = match base {
            Base::None => None,
            Base::Empty => statuses(&[], &job.snapshot),
            Base::Blob(_) => {
                let blob = self.blob.as_ref().map_or(&[][..], |(_, bytes)| bytes);
                statuses(blob, &job.snapshot)
            }
        };
        Outcome::Statuses(statuses)
    }

    /// What to compare the file at `path` with, reading HEAD as it is now
    /// and the blob if it isn't the one read last.
    fn base(&mut self, path: &Path) -> Base {
        if self.file.as_ref().is_none_or(|(file, _)| file != path) {
            self.file = Some((path.to_owned(), open_repository(path)));
        }
        let Some((_, Some((repo, relative)))) = &self.file else {
            return Base::None;
        };
        let tree = match repo.head() {
            Ok(head) => match head.peel_to_tree() {
                Ok(tree) => Some(tree),
                Err(_) => return Base::None,
            },
            Err(err) if matches!(err.code(), ErrorCode::UnbornBranch | ErrorCode::NotFound) => None,
            Err(_) => return Base::None,
        };
        let entry = tree.and_then(|tree| tree.get_path(relative).ok());
        let Some(entry) = entry else {
            return if repo.is_path_ignored(relative).unwrap_or(true) {
                Base::None
            } else {
                Base::Empty
            };
        };
        if entry.kind() != Some(ObjectType::Blob) {
            return Base::None;
        }
        let id = entry.id();
        if self.blob.as_ref().is_none_or(|(blob, _)| *blob != id) {
            let Ok(blob) = repo.find_blob(id) else {
                return Base::None;
            };
            let content = blob.content();
            if content.len() > MAX_FILE_SIZE {
                return Base::None;
            }
            self.blob = Some((id, normalized(content).into_owned()));
        }
        Base::Blob(id)
    }
}

/// The repository `path` is in, deepest first (a submodule's for a file
/// in one), and the path of the file there. `None` for a file in no
/// repository's working directory, or in the `.git` directory itself.
fn open_repository(path: &Path) -> Option<(Repository, PathBuf)> {
    let path = canonical(path);
    let repo = Repository::discover(path.parent()?).ok()?;
    let workdir = canonical(repo.workdir()?);
    let relative = path.strip_prefix(&workdir).ok()?.to_owned();
    if relative.components().next() == Some(Component::Normal(".git".as_ref())) {
        return None;
    }
    Some((repo, relative))
}

/// `path` with its directories resolved, so it can be compared with a
/// repository's working directory; the file itself needn't exist yet.
fn canonical(path: &Path) -> PathBuf {
    if let Ok(path) = fs::canonicalize(path) {
        return path;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => fs::canonicalize(parent)
            .map(|parent| parent.join(name))
            .unwrap_or_else(|_| path.to_owned()),
        _ => path.to_owned(),
    }
}

/// `bytes` with every line break, of whichever kind, made `\n`.
fn normalized(bytes: &[u8]) -> Cow<'_, [u8]> {
    if !bytes.contains(&b'\r') {
        return Cow::Borrowed(bytes);
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match break_len(bytes, i) {
            Some(len) => {
                out.push(b'\n');
                i += len;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    Cow::Owned(out)
}

/// The status of each line of `snapshot` compared with `base` (with its
/// line breaks already made `\n`). `None` if either is too big or binary.
fn statuses(base: &[u8], snapshot: &BufferSnapshot) -> Option<Vec<LineStatus>> {
    if snapshot.len() > MAX_FILE_SIZE {
        return None;
    }
    let mut current = Vec::with_capacity(snapshot.len());
    let mut lines = snapshot.lines_from(0);
    let mut first = true;
    while let Some(line) = lines.next_line() {
        if !first {
            current.push(b'\n');
        }
        first = false;
        current.extend_from_slice(line);
    }
    let hunks = merge::line_hunks(base, &current)?;
    let line_count = snapshot.line_count();
    let mut statuses = vec![LineStatus::Unchanged; line_count];
    for hunk in hunks {
        if hunk.new_lines == 0 {
            let after = hunk.new_start.min(line_count - 1);
            if statuses[after] == LineStatus::Unchanged {
                statuses[after] = LineStatus::DeletedAbove;
            }
        } else {
            let status = if hunk.old_lines == 0 {
                LineStatus::Added
            } else {
                LineStatus::Modified
            };
            let end = (hunk.new_start + hunk.new_lines).min(line_count);
            statuses[hunk.new_start.min(end)..end].fill(status);
        }
    }
    Some(statuses)
}

#[cfg(test)]
mod tests {
    use super::LineStatus::{Added as A, DeletedAbove as D, Modified as M, Unchanged as U};
    use super::*;
    use crate::git::history::tests::TestRepo;

    const WAIT: Duration = Duration::from_secs(10);

    fn edit(start: usize, old_count: usize, new_count: usize) -> LinesEdit {
        LinesEdit {
            start,
            old_count,
            new_count,
        }
    }

    #[test]
    fn edits_leave_out_the_lines_that_came_out_the_same() {
        // Typing within a line.
        assert_eq!(LinesEdit::between(4, b"abc\n", b"abxc\n"), edit(4, 1, 1));
        // A line break at the end of a line adds the line after it, and
        // one at the start adds the line before.
        assert_eq!(LinesEdit::between(4, b"abc\n", b"abc\n\n"), edit(5, 0, 1));
        assert_eq!(LinesEdit::between(4, b"abc\n", b"\nabc\n"), edit(4, 0, 1));
        // Splitting a line changes it and adds one.
        assert_eq!(LinesEdit::between(4, b"abc\n", b"ab\nc\n"), edit(4, 1, 2));
        // Deleting a whole line.
        assert_eq!(LinesEdit::between(4, b"a\nb\n", b"b\n"), edit(4, 1, 0));
        // Only the kind of line break changed.
        assert_eq!(LinesEdit::between(4, b"a\r\nb\n", b"a\nb\r"), edit(6, 0, 0));
        // At the end of a file that ends with a line break, the empty
        // line after it isn't one: typing there adds a line, and ending
        // that line with a break only changes it.
        assert_eq!(LinesEdit::between(1, b"", b"x"), edit(1, 0, 1));
        assert_eq!(LinesEdit::between(1, b"x", b"x\n"), edit(1, 1, 1));
    }

    fn known(statuses: &[LineStatus]) -> Vec<Option<LineStatus>> {
        statuses.iter().copied().map(Some).collect()
    }

    #[test]
    fn edits_move_statuses_and_guess_at_the_lines_they_touch() {
        let mut statuses = known(&[U, U, M, U, U]);
        // Two lines added before the modified one push it down.
        apply_edit(&mut statuses, edit(1, 0, 2), true, 7);
        assert_eq!(statuses, known(&[U, A, A, U, M, U, U]));
        // Typing on an added line leaves it added; on an unchanged one,
        // modified.
        apply_edit(&mut statuses, edit(2, 1, 1), true, 7);
        apply_edit(&mut statuses, edit(5, 1, 1), true, 7);
        assert_eq!(statuses, known(&[U, A, A, U, M, M, U]));
        // Deleting the added lines leaves no trace; deleting the
        // unchanged one after them marks the line after it.
        apply_edit(&mut statuses, edit(1, 2, 0), true, 5);
        assert_eq!(statuses, known(&[U, U, M, M, U]));
        apply_edit(&mut statuses, edit(0, 1, 0), true, 4);
        assert_eq!(statuses, known(&[D, M, M, U]));
        // Without statuses to go on, lines stay unknown.
        let mut statuses = vec![None; 3];
        apply_edit(&mut statuses, edit(1, 1, 3), false, 5);
        assert_eq!(statuses, vec![None; 5]);
    }

    #[test]
    fn edits_at_the_end_keep_the_empty_last_line_as_it_was() {
        // "a\n" has lines "a" and the empty one after; typing on the empty
        // one adds a line in git's terms but not in the buffer's.
        let mut statuses = known(&[U, U]);
        apply_edit(&mut statuses, edit(1, 0, 1), true, 2);
        assert_eq!(statuses, known(&[U, A]));
        // A line break after it adds the empty line back, unchanged.
        apply_edit(&mut statuses, edit(1, 1, 1), true, 3);
        assert_eq!(statuses, known(&[U, A, U]));
    }

    #[test]
    fn a_late_result_is_brought_up_to_date_with_the_edits_since() {
        let mut shared = Shared {
            statuses: known(&[U, U, U]),
            tracking: true,
            version: 0,
            diffed: Some(0),
            in_flight: true,
            edits: Vec::new(),
            recheck: false,
        };
        // The snapshot sent had line 1 modified; meanwhile a line was
        // added at the top.
        let added = edit(0, 0, 1);
        apply_edit(&mut shared.statuses, added, true, 4);
        shared.version = 1;
        shared.edits.push((added, 4));
        assert!(shared.merge(0, Outcome::Statuses(Some(vec![U, M, U]))));
        assert_eq!(shared.statuses, known(&[A, U, M, U]));
        assert!(!shared.is_settled(), "the buffer has changed since");
        assert!(shared.edits.is_empty());
    }

    /// An editor's view of a file: a buffer and its statuses, edited as
    /// the editor does.
    struct Open {
        buffer: FileBuffer,
        statuses: LineStatuses,
    }

    impl Open {
        fn new(path: &Path) -> Open {
            let buffer = FileBuffer::open(path).unwrap();
            let statuses = LineStatuses::new(buffer.line_count());
            Open { buffer, statuses }
        }

        fn replace(&mut self, offset: usize, len: usize, text: &str) {
            let start = self.buffer.line_of_offset(offset);
            let old_end = self.buffer.line_of_offset(offset + len);
            let lines = |buffer: &FileBuffer, end: usize| {
                buffer.bytes_in_range(buffer.offset_of_line(start)..buffer.line_range(end).end)
            };
            let old = lines(&self.buffer, old_end);
            self.buffer.delete(offset..offset + len);
            self.buffer.insert(offset, text);
            let new_end = self.buffer.line_of_offset(offset + text.len());
            let edit = LinesEdit::between(start, &old, &lines(&self.buffer, new_end));
            self.statuses.lines_changed(edit, self.buffer.line_count());
        }

        fn settled(&self) -> Vec<Option<LineStatus>> {
            assert!(self.statuses.wait(&self.buffer, WAIT));
            self.statuses
                .statuses(&self.buffer, 0..self.buffer.line_count())
        }
    }

    #[test]
    fn statuses_compare_the_buffer_with_head() {
        let mut repo = TestRepo::new();
        let text = "one\ntwo\nthree\nfour\nfive\n";
        let first = repo.commit(&[("a.txt", text)], "first", &[]);
        let path = repo.path().join("a.txt");
        let mut open = Open::new(&path);
        // Unknown until the first diff, then all unchanged.
        let unknown = open.statuses.inner.shared.lock().unwrap().statuses.clone();
        assert_eq!(unknown, vec![None; 6]);
        assert_eq!(open.settled(), known(&[U, U, U, U, U, U]));

        // Unsaved edits count: "two" changed, a line after "three", and
        // "five" deleted.
        open.replace(4, 3, "TWO");
        open.replace(14, 0, "new\n");
        open.replace(23, 5, "");
        let edited = "one\nTWO\nthree\nnew\nfour\n";
        assert_eq!(open.buffer.to_text(), edited);
        // The guesses are there at once, and the diff agrees.
        let expected = known(&[U, M, U, A, U, D]);
        assert_eq!(open.statuses.statuses(&open.buffer, 0..6), expected);
        assert_eq!(open.settled(), expected);

        // A commit of the file as the buffer has it settles everything,
        // once HEAD is looked at again.
        repo.commit(&[("a.txt", edited)], "second", &[first]);
        assert_eq!(open.settled(), expected);
        open.statuses.refresh(&open.buffer);
        assert_eq!(open.settled(), known(&[U, U, U, U, U, U]));

        // A change of line endings alone is no change.
        let mut crlf = Open::new(&path);
        crlf.replace(0, crlf.buffer.len(), &edited.replace('\n', "\r\n"));
        assert_eq!(crlf.settled(), known(&[U, U, U, U, U, U]));
    }

    #[test]
    fn files_head_lacks_are_new_unless_ignored() {
        let mut repo = TestRepo::new();
        repo.commit(&[(".gitignore", "*.log\n")], "first", &[]);
        fs::write(repo.path().join("new.txt"), "a\nb\n").unwrap();
        fs::write(repo.path().join("out.log"), "a\nb\n").unwrap();
        let new = Open::new(&repo.path().join("new.txt"));
        assert_eq!(new.settled(), known(&[A, A, U]));
        let ignored = Open::new(&repo.path().join("out.log"));
        assert_eq!(ignored.settled(), vec![None; 3]);

        // Nor does a file outside any repository have statuses.
        let elsewhere = tempfile::tempdir().unwrap();
        let path = elsewhere.path().join("x.txt");
        fs::write(&path, "x\n").unwrap();
        let outside = Open::new(&path);
        assert_eq!(outside.settled(), vec![None; 2]);
    }

    #[test]
    fn an_unborn_branch_has_everything_new() {
        let repo = TestRepo::new();
        fs::write(repo.path().join("a.txt"), "a\n").unwrap();
        let open = Open::new(&repo.path().join("a.txt"));
        assert_eq!(open.settled(), known(&[A, U]));
    }
}
