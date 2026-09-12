//! Searching every file in a project for text, off the main thread.
//!
//! A [`ProjectSearch`] is given a query and looks for it in each
//! non-ignored file the project's [`FileIndex`](crate::FileIndex) knows
//! about, waiting for the index to finish its first scan if it hasn't. The
//! query takes the same form as the in-file search's (see the
//! [`search`](crate::search) module): plain text is matched literally,
//! and a leading `/` introduces a regular expression. A match never spans
//! a line break.
//!
//! Files are read from disk, except those a frontend has handed over the
//! contents of (see [`ProjectSearch::set_buffers`]): a file open in an
//! editor is searched as the editor has it, unsaved edits and all, and
//! its lines can be had the same way for showing the context of a match
//! (see [`ProjectSearch::file_lines`]). Files that look binary (a NUL byte
//! near the start) are skipped. The
//! work is spread over a few threads, but matches are published in a
//! fixed order: files sorted by path, then top to bottom within each file.
//! Matches are only ever appended, so an index into them stays valid as
//! more arrive, and a frontend can poll [`ProjectSearch::generation`] to
//! learn when it has more to show.
//!
//! A search stops at a limit, [`MAX_MATCHES`] unless the settings say
//! otherwise (a query like `/.` matches every character in the project,
//! and a half-typed query can easily be that broad): the workers stop as
//! soon as that many have been found, the search reports itself
//! [`truncated`](ProjectSearch::is_truncated), and what was found stays
//! available, in order. Far more matches than anyone could look through
//! fit under the default limit.
//!
//! Each [`ProjectMatch`] carries the text of its line (or, for a very long
//! line, a window of it around the match) so a list of results can be
//! drawn without touching the files again.

use crate::buffer::BufferSnapshot;
use crate::index::FileList;
use crate::search::compile;
use regex::bytes::Regex;
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use std::{fs, io};

/// The most matches a search keeps; see the [module documentation](self).
pub const MAX_MATCHES: usize = 10_000;
/// Files searched as one unit; matches are published a chunk at a time.
const CHUNK: usize = 32;
/// The most worker threads a search runs.
const MAX_THREADS: usize = 8;
/// How long to wait for the file index's first scan before searching
/// whatever it has.
const INDEX_WAIT: Duration = Duration::from_secs(60);
/// A file whose first bytes contain a NUL within this many is binary.
const BINARY_PROBE: usize = 8192;
/// Lines longer than this are stored as a window around the match.
const LONG_LINE: usize = 1024;
/// The window kept before and after a match on a long line.
const WINDOW_BEFORE: usize = 256;
const WINDOW_AFTER: usize = 512;

/// The contents of files as they are in editors, keyed by absolute path,
/// to search in place of what is on disk; see
/// [`ProjectSearch::set_buffers`].
pub type Buffers = HashMap<PathBuf, BufferSnapshot>;

/// One match in one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectMatch {
    /// Absolute path of the file.
    pub path: Arc<Path>,
    /// Zero-based line the match is on.
    pub line: usize,
    /// Byte offset of the match within the line.
    pub column: usize,
    /// Length of the match in bytes.
    pub len: usize,
    /// The line's text, lossily decoded, without its terminator. For a
    /// line longer than a kilobyte, a window of it around the match.
    /// Shared between the matches on one line.
    pub text: Arc<str>,
    /// The match within `text`.
    pub range: Range<usize>,
    /// The byte offset within the line that `text` starts at: zero unless
    /// the line was windowed and the window doesn't start at the line's
    /// start. When it is zero, `text` is the line or a prefix of it, and
    /// can be lexed from the line's starting state.
    pub text_offset: usize,
}

/// What stage the search is at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchPhase {
    /// The file index is still scanning the project.
    WaitingForIndex,
    Searching,
    Done,
}

struct Found {
    matches: Vec<ProjectMatch>,
    /// Files with at least one match, counted as their matches arrive.
    files_with_matches: usize,
    /// Chunks finished out of order, waiting for the ones before them.
    pending: BTreeMap<usize, Vec<ProjectMatch>>,
    next_chunk: usize,
    phase: SearchPhase,
    /// Whether the search stopped at the limit with more to find.
    truncated: bool,
}

struct Results {
    found: Mutex<Found>,
    changed: Condvar,
}

impl Results {
    fn new(phase: SearchPhase) -> Arc<Results> {
        Arc::new(Results {
            found: Mutex::new(Found {
                matches: Vec::new(),
                files_with_matches: 0,
                pending: BTreeMap::new(),
                next_chunk: 0,
                phase,
                truncated: false,
            }),
            changed: Condvar::new(),
        })
    }
}

/// A search of a project's files for one query; see the [module
/// documentation](self).
pub struct ProjectSearch {
    files: FileList,
    /// The most matches to find; [`MAX_MATCHES`] unless changed with
    /// [`set_limit`](Self::set_limit).
    limit: usize,
    query: String,
    pattern: Result<Option<Regex>, String>,
    buffers: Arc<Buffers>,
    results: Arc<Results>,
    cancel: Arc<AtomicBool>,
    /// Bumped whenever a worker publishes matches. Shared by every worker
    /// this search starts so it only ever grows.
    generation: Arc<AtomicU64>,
}

impl ProjectSearch {
    /// A search over the files of `files`, with an empty query.
    pub fn new(files: FileList) -> ProjectSearch {
        ProjectSearch {
            files,
            limit: MAX_MATCHES,
            query: String::new(),
            pattern: Ok(None),
            buffers: Arc::new(Buffers::new()),
            results: Results::new(SearchPhase::Done),
            cancel: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Whether the query is a regular expression (starts with `/`).
    pub fn is_regex(&self) -> bool {
        self.query.starts_with('/')
    }

    /// Why the query doesn't compile, briefly, if it doesn't.
    pub fn error(&self) -> Option<&str> {
        self.pattern.as_ref().err().map(String::as_str)
    }

    /// Replace the query and search for it afresh. Matches found so far
    /// for the old query are discarded at once.
    pub fn set_query(&mut self, query: &str) {
        if query == self.query {
            return;
        }
        self.query = query.to_owned();
        self.pattern = compile(query);
        self.restart();
    }

    /// Search files that are open in editors as the editors have them,
    /// rather than as they are on disk. Only files the project's index
    /// knows about are searched, whether or not they are in `buffers`. A
    /// search under way starts over with the new contents, discarding
    /// what it had found (and the buffers given before, so a file left
    /// out is read from disk again).
    pub fn set_buffers(&mut self, buffers: Arc<Buffers>) {
        self.buffers = buffers;
        if matches!(self.pattern, Ok(Some(_))) {
            self.restart();
        }
    }

    /// The lines of a file as the search sees them: from its buffer, if
    /// one was given with [`set_buffers`](Self::set_buffers), and
    /// otherwise from disk as [`read_lines`] reads them.
    pub fn file_lines(&self, path: &Path) -> io::Result<Vec<String>> {
        match self.buffers.get(path) {
            Some(snapshot) => Ok(snapshot_lines(snapshot)),
            None => read_lines(path),
        }
    }

    /// Stop any search under way and start one afresh for the query,
    /// unless there is nothing to search for.
    fn restart(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.cancel = Arc::new(AtomicBool::new(false));
        let Ok(Some(regex)) = &self.pattern else {
            self.results = Results::new(SearchPhase::Done);
            return;
        };
        self.results = Results::new(SearchPhase::WaitingForIndex);
        let job = Job {
            regex: regex.clone(),
            files: self.files.clone(),
            buffers: Arc::clone(&self.buffers),
            limit: self.limit,
            results: Arc::clone(&self.results),
            cancel: Arc::clone(&self.cancel),
            full: AtomicBool::new(false),
            counted: AtomicUsize::new(0),
            generation: Arc::clone(&self.generation),
        };
        std::thread::Builder::new()
            .name("project-search".to_owned())
            .spawn(move || job.run())
            .expect("spawn project search thread");
    }

    /// A counter that changes whenever the workers have found more
    /// matches or the search has finished, so a frontend knows to redraw.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn phase(&self) -> SearchPhase {
        self.results.found.lock().unwrap().phase
    }

    /// Whether every file has been searched (or the search has stopped at
    /// its limit).
    pub fn is_done(&self) -> bool {
        self.phase() == SearchPhase::Done
    }

    /// Whether the search stopped at its [limit](Self::limit) with more
    /// to find.
    pub fn is_truncated(&self) -> bool {
        self.results.found.lock().unwrap().truncated
    }

    /// The number of matches a search stops at.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Change the number of matches to stop at, for the queries set from
    /// now on; a search already running keeps its limit.
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
    }

    /// Block until every file has been searched.
    pub fn wait(&self) {
        let mut found = self.results.found.lock().unwrap();
        while found.phase != SearchPhase::Done {
            found = self.results.changed.wait(found).unwrap();
        }
    }

    /// Wait up to `timeout` for the search to finish. Returns whether it
    /// did.
    pub fn wait_for(&self, timeout: Duration) -> bool {
        let found = self.results.found.lock().unwrap();
        let (found, _) = self
            .results
            .changed
            .wait_timeout_while(found, timeout, |found| found.phase != SearchPhase::Done)
            .unwrap();
        found.phase == SearchPhase::Done
    }

    /// The number of matches found so far.
    pub fn match_count(&self) -> usize {
        self.results.found.lock().unwrap().matches.len()
    }

    /// The number of files with a match found so far.
    pub fn file_count(&self) -> usize {
        self.results.found.lock().unwrap().files_with_matches
    }

    /// The match at an index into the matches found so far.
    pub fn get(&self, index: usize) -> Option<ProjectMatch> {
        self.results
            .found
            .lock()
            .unwrap()
            .matches
            .get(index)
            .cloned()
    }

    /// The matches found so far with indexes in `range`, which is clamped
    /// to what there is.
    pub fn slice(&self, range: Range<usize>) -> Vec<ProjectMatch> {
        let found = self.results.found.lock().unwrap();
        let end = range.end.min(found.matches.len());
        let start = range.start.min(end);
        found.matches[start..end].to_vec()
    }
}

impl Drop for ProjectSearch {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

/// The lines of a file as the editor counts them: split at `\n`, `\r\n`,
/// or a lone `\r`, without the terminators. A trailing terminator ends
/// the last line rather than starting an empty one, but the empty line
/// after it matches nothing anyway, so it is left out.
pub fn read_lines(path: &Path) -> io::Result<Vec<String>> {
    let bytes = fs::read(path)?;
    Ok(lines(&bytes)
        .map(|(_, line)| String::from_utf8_lossy(line).into_owned())
        .collect())
}

/// The lines of a buffer, without their terminators, as its editor counts
/// them: unlike [`read_lines`], a trailing terminator is followed by an
/// empty last line, since the editor shows one.
pub fn snapshot_lines(snapshot: &BufferSnapshot) -> Vec<String> {
    let mut out = Vec::with_capacity(snapshot.line_count());
    let mut lines = snapshot.lines_from(0);
    while let Some(line) = lines.next_line() {
        out.push(String::from_utf8_lossy(line).into_owned());
    }
    out
}

/// The lines of `bytes` with the byte offset each starts at.
pub(crate) fn lines(bytes: &[u8]) -> impl Iterator<Item = (usize, &[u8])> {
    let mut start = 0;
    std::iter::from_fn(move || {
        if start >= bytes.len() {
            return None;
        }
        let rest = &bytes[start..];
        let (len, terminator) = match rest.iter().position(|&b| b == b'\n' || b == b'\r') {
            Some(i) if rest[i] == b'\r' && rest.get(i + 1) == Some(&b'\n') => (i, 2),
            Some(i) => (i, 1),
            None => (rest.len(), 0),
        };
        let line = (start, &rest[..len]);
        start += len + terminator;
        Some(line)
    })
}

/// What the coordinator thread searches.
struct Job {
    regex: Regex,
    files: FileList,
    /// Files to search as they are in editors rather than on disk.
    buffers: Arc<Buffers>,
    limit: usize,
    results: Arc<Results>,
    cancel: Arc<AtomicBool>,
    /// Set once the limit is reached: workers stop looking.
    full: AtomicBool,
    /// Matches found so far by every worker, published or not, so the
    /// workers stop looking once there are enough rather than filling
    /// memory with more than will be kept.
    counted: AtomicUsize,
    generation: Arc<AtomicU64>,
}

impl Job {
    fn run(self) {
        self.files.wait_for_primary(INDEX_WAIT);
        if self.cancel.load(Ordering::Acquire) {
            return;
        }
        let mut paths = self.files.files(false);
        paths.sort();
        let paths: Vec<Arc<Path>> = paths.into_iter().map(Arc::from).collect();
        {
            let mut found = self.results.found.lock().unwrap();
            found.phase = SearchPhase::Searching;
        }
        self.notify();

        let chunks = paths.chunks(CHUNK).collect::<Vec<_>>();
        let next = AtomicUsize::new(0);
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .clamp(1, MAX_THREADS)
            .min(chunks.len().max(1));
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(chunk) = chunks.get(index) else {
                            break;
                        };
                        let mut matches = Vec::new();
                        for path in chunk.iter() {
                            if self.cancel.load(Ordering::Acquire) {
                                return;
                            }
                            if self.full.load(Ordering::Acquire) {
                                break;
                            }
                            self.search_file(path, &mut matches);
                        }
                        if !self.publish(index, matches) {
                            return;
                        }
                        if self.full.load(Ordering::Acquire) {
                            return;
                        }
                    }
                });
            }
        });
        if self.cancel.load(Ordering::Acquire) {
            return;
        }
        let mut found = self.results.found.lock().unwrap();
        found.phase = SearchPhase::Done;
        found.truncated = self.full.load(Ordering::Acquire);
        drop(found);
        self.notify();
    }

    fn notify(&self) {
        self.generation.fetch_add(1, Ordering::Release);
        self.results.changed.notify_all();
    }

    /// Hand in a finished chunk's matches, appending them to the results
    /// once every chunk before it is in, up to the limit. Returns false
    /// when cancelled.
    fn publish(&self, index: usize, matches: Vec<ProjectMatch>) -> bool {
        if self.cancel.load(Ordering::Acquire) {
            return false;
        }
        let mut guard = self.results.found.lock().unwrap();
        let found = &mut *guard;
        found.pending.insert(index, matches);
        let mut appended = false;
        while let Some(matches) = found.pending.remove(&found.next_chunk) {
            found.next_chunk += 1;
            if matches.is_empty() {
                continue;
            }
            appended = true;
            let mut last = found.matches.last().map(|m| Arc::clone(&m.path));
            for m in matches {
                if found.matches.len() >= self.limit {
                    self.full.store(true, Ordering::Release);
                    found.pending.clear();
                    break;
                }
                if last.as_deref() != Some(&*m.path) {
                    found.files_with_matches += 1;
                    last = Some(Arc::clone(&m.path));
                }
                found.matches.push(m);
            }
        }
        drop(guard);
        if appended {
            self.notify();
        }
        true
    }

    fn search_file(&self, path: &Arc<Path>, out: &mut Vec<ProjectMatch>) {
        if let Some(snapshot) = self.buffers.get(&**path) {
            // An editor's contents are text by definition, whatever the
            // file on disk holds.
            let mut lines = snapshot.lines_from(0);
            let mut number = 0;
            while let Some(line) = lines.next_line() {
                if !self.search_line(path, number, line, out) {
                    return;
                }
                number += 1;
            }
            return;
        }
        let Ok(bytes) = fs::read(path) else {
            return;
        };
        if bytes[..bytes.len().min(BINARY_PROBE)].contains(&0) {
            return;
        }
        for (number, (_, line)) in lines(&bytes).enumerate() {
            if !self.search_line(path, number, line, out) {
                return;
            }
        }
    }

    /// Find the matches on one line. Returns false once the limit is
    /// reached, when there is no point going on.
    fn search_line(
        &self,
        path: &Arc<Path>,
        number: usize,
        line: &[u8],
        out: &mut Vec<ProjectMatch>,
    ) -> bool {
        // The matches on a short line share one copy of its text.
        let mut whole: Option<Arc<str>> = None;
        for m in self.regex.find_iter(line) {
            if m.is_empty() {
                continue;
            }
            if self.counted.fetch_add(1, Ordering::Relaxed) >= self.limit {
                self.full.store(true, Ordering::Release);
                return false;
            }
            let (text, range, text_offset) = if line.len() <= LONG_LINE {
                let text = whole
                    .get_or_insert_with(|| String::from_utf8_lossy(line).into())
                    .clone();
                (text, decoded_range(line, m.range()), 0)
            } else {
                let (text, range, offset) = excerpt(line, m.range());
                (Arc::from(text), range, offset)
            };
            out.push(ProjectMatch {
                path: Arc::clone(path),
                line: number,
                column: m.start(),
                len: m.len(),
                text,
                range,
                text_offset,
            });
        }
        true
    }
}

/// The text of a line to keep with a match on it, where the match is
/// within that text, and where the text starts within the line. Short
/// lines are kept whole; long ones as a window around the match, cut at
/// character boundaries. Each part is decoded on its own so that invalid
/// bytes before the match, which decode to a wider replacement
/// character, don't throw off the range.
fn excerpt(line: &[u8], m: Range<usize>) -> (String, Range<usize>, usize) {
    let (start, end) = if line.len() <= LONG_LINE {
        (0, line.len())
    } else {
        let mut start = m.start.saturating_sub(WINDOW_BEFORE);
        while start < m.start && is_continuation(line[start]) {
            start += 1;
        }
        let mut end = (m.end + WINDOW_AFTER).min(line.len());
        while end < line.len() && is_continuation(line[end]) {
            end += 1;
        }
        (start, end)
    };
    let before = String::from_utf8_lossy(&line[start..m.start]);
    let matched = String::from_utf8_lossy(&line[m.clone()]);
    let after = String::from_utf8_lossy(&line[m.end..end]);
    let range = before.len()..before.len() + matched.len();
    let mut text = String::with_capacity(before.len() + matched.len() + after.len());
    text.push_str(&before);
    text.push_str(&matched);
    text.push_str(&after);
    (text, range, start)
}

/// Where a byte range of a line lands in the line's lossily decoded
/// text, which invalid bytes before it widen.
fn decoded_range(line: &[u8], m: Range<usize>) -> Range<usize> {
    let start = String::from_utf8_lossy(&line[..m.start]).len();
    let matched = String::from_utf8_lossy(&line[m]).len();
    start..start + matched
}

fn is_continuation(byte: u8) -> bool {
    byte & 0xC0 == 0x80
}

/// A path for tests and display: relative to `root` when inside it.
pub fn relative_to(path: &Path, root: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::FileIndex;

    const WAIT: Duration = Duration::from_secs(10);

    fn write(root: &Path, name: &str, contents: &[u8]) {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    /// `(path, line, column, text, matched)` for every match.
    fn summarize(
        search: &ProjectSearch,
        root: &Path,
    ) -> Vec<(String, usize, usize, String, String)> {
        search
            .slice(0..usize::MAX)
            .into_iter()
            .map(|m| {
                (
                    relative_to(&m.path, root)
                        .to_string_lossy()
                        .replace('\\', "/"),
                    m.line,
                    m.column,
                    m.text.to_string(),
                    m.text[m.range.clone()].to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn finds_matches_across_files_in_path_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        write(&root, ".gitignore", b"ignored/\n");
        write(&root, "b.txt", b"needle\nhay\nneedle needle\n");
        write(&root, "a/one.txt", b"hay\r\nneedle at 2\r\n");
        write(&root, "a/two.txt", b"nothing here\n");
        write(&root, "ignored/x.txt", b"needle\n");
        write(&root, "bin.dat", b"needle\0\x01\x02");
        let index = FileIndex::new(&root);
        let mut search = ProjectSearch::new(index.file_list());
        assert!(search.is_done());
        assert_eq!(search.match_count(), 0);

        search.set_query("needle");
        assert!(search.wait_for(WAIT));
        assert_eq!(
            summarize(&search, &root),
            vec![
                (
                    "a/one.txt".into(),
                    1,
                    0,
                    "needle at 2".into(),
                    "needle".into()
                ),
                ("b.txt".into(), 0, 0, "needle".into(), "needle".into()),
                (
                    "b.txt".into(),
                    2,
                    0,
                    "needle needle".into(),
                    "needle".into()
                ),
                (
                    "b.txt".into(),
                    2,
                    7,
                    "needle needle".into(),
                    "needle".into()
                ),
            ]
        );
        assert_eq!(search.file_count(), 2);
        assert_eq!(search.get(1).unwrap().line, 0);
        assert!(search.get(4).is_none());
        assert_eq!(search.slice(3..10).len(), 1);

        // A regex, an error, and clearing the query.
        search.set_query("/n[a-z]+ing");
        assert!(search.wait_for(WAIT));
        assert!(search.is_regex());
        assert_eq!(search.match_count(), 1);
        assert_eq!(&*search.get(0).unwrap().text, "nothing here");
        search.set_query("/(");
        assert_eq!(search.error(), Some("unclosed group"));
        assert!(search.is_done());
        assert_eq!(search.match_count(), 0);
        search.set_query("");
        assert!(search.is_done());
        assert_eq!(search.match_count(), 0);
    }

    #[test]
    fn many_files_come_back_in_order_and_generation_moves() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        for i in 0..200 {
            write(
                &root,
                &format!("f{i:03}.txt"),
                format!("x\nneedle {i}\n").as_bytes(),
            );
        }
        let index = FileIndex::new(&root);
        let mut search = ProjectSearch::new(index.file_list());
        let generation = search.generation();
        search.set_query("needle");
        assert!(search.wait_for(WAIT));
        assert!(search.generation() > generation);
        let paths: Vec<PathBuf> = search
            .slice(0..usize::MAX)
            .iter()
            .map(|m| relative_to(&m.path, &root))
            .collect();
        assert_eq!(paths.len(), 200);
        assert!(paths.windows(2).all(|w| w[0] < w[1]), "{paths:?}");
        assert_eq!(search.file_count(), 200);
    }

    #[test]
    fn open_buffers_are_searched_in_place_of_disk() {
        use crate::buffer::FileBuffer;
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        write(&root, "a.txt", b"needle on disk\n");
        write(&root, "b.txt", b"needle\n");
        let index = FileIndex::new(&root);
        let mut search = ProjectSearch::new(index.file_list());
        search.set_query("needle");
        assert!(search.wait_for(WAIT));
        assert_eq!(search.match_count(), 2);
        assert_eq!(
            search.file_lines(&root.join("a.txt")).unwrap(),
            vec!["needle on disk"]
        );

        // Handing over a buffer restarts the search with its contents.
        let mut buffers = Buffers::new();
        let edited = FileBuffer::from_text("edited\r\nneedle needle\n");
        buffers.insert(root.join("a.txt"), edited.snapshot());
        // A buffer for a file the index doesn't know is never searched.
        let elsewhere = fs::canonicalize(tempfile::tempdir().unwrap().path())
            .unwrap()
            .join("c.txt");
        buffers.insert(elsewhere, FileBuffer::from_text("needle\n").snapshot());
        search.set_buffers(Arc::new(buffers));
        assert!(search.wait_for(WAIT));
        assert_eq!(
            summarize(&search, &root),
            vec![
                (
                    "a.txt".into(),
                    1,
                    0,
                    "needle needle".into(),
                    "needle".into()
                ),
                (
                    "a.txt".into(),
                    1,
                    7,
                    "needle needle".into(),
                    "needle".into()
                ),
                ("b.txt".into(), 0, 0, "needle".into(), "needle".into()),
            ]
        );
        assert_eq!(
            search.file_lines(&root.join("a.txt")).unwrap(),
            vec!["edited", "needle needle", ""]
        );
        assert_eq!(
            search.file_lines(&root.join("b.txt")).unwrap(),
            vec!["needle"]
        );

        // Taking the buffers away goes back to the disk.
        search.set_buffers(Arc::new(Buffers::new()));
        assert!(search.wait_for(WAIT));
        assert_eq!(search.match_count(), 2);
        assert_eq!(search.slice(0..1)[0].text.as_ref(), "needle on disk");
    }

    #[test]
    fn matches_on_one_line_share_its_text_and_bad_bytes_are_placed() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        write(&root, "a.txt", b"ab \xffab ab\n");
        let index = FileIndex::new(&root);
        let mut search = ProjectSearch::new(index.file_list());
        search.set_query("ab");
        assert!(search.wait_for(WAIT));
        let matches = search.slice(0..usize::MAX);
        assert_eq!(matches.len(), 3);
        assert!(Arc::ptr_eq(&matches[0].text, &matches[1].text));
        assert!(Arc::ptr_eq(&matches[1].text, &matches[2].text));
        assert_eq!(&*matches[1].text, "ab \u{FFFD}ab ab");
        assert_eq!(&matches[1].text[matches[1].range.clone()], "ab");
        assert_eq!(matches[1].column, 4);
        assert_eq!(&matches[2].text[matches[2].range.clone()], "ab");
    }

    #[test]
    fn a_search_stops_at_its_limit() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        for i in 0..50 {
            write(&root, &format!("f{i:02}.txt"), &b"x".repeat(1000));
        }
        let index = FileIndex::new(&root);
        let mut search = ProjectSearch::new(index.file_list());
        search.set_limit(500);
        search.set_query("/.");
        assert!(search.wait_for(WAIT));
        assert!(search.is_done());
        assert!(search.is_truncated());
        assert_eq!(search.match_count(), 500);
        // What was kept is in order and complete up to the cut.
        let matches = search.slice(0..usize::MAX);
        assert!(
            matches
                .windows(2)
                .all(|w| (&w[0].path, w[0].column) < (&w[1].path, w[1].column))
        );
        assert_eq!(matches[0].column, 0);
        // A search within the limit isn't truncated.
        search.set_query(&"x".repeat(200));
        assert!(search.wait_for(WAIT));
        assert!(!search.is_truncated());
        assert_eq!(search.match_count(), 50 * 5);
    }

    #[test]
    fn lines_split_like_the_editor() {
        let split = |bytes: &[u8]| {
            lines(bytes)
                .map(|(start, line)| (start, String::from_utf8_lossy(line).into_owned()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            split(b"a\nb\r\nc\rd"),
            vec![
                (0, "a".into()),
                (2, "b".into()),
                (5, "c".into()),
                (7, "d".into())
            ]
        );
        assert_eq!(split(b"a\n"), vec![(0, "a".into())]);
        assert_eq!(
            split(b"\n\nx"),
            vec![(0, "".into()), (1, "".into()), (2, "x".into())]
        );
        assert_eq!(split(b""), vec![]);
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "f.txt", b"one\r\ntwo\n");
        assert_eq!(
            read_lines(&dir.path().join("f.txt")).unwrap(),
            vec!["one", "two"]
        );
    }

    #[test]
    fn long_lines_are_excerpted_and_bad_bytes_tolerated() {
        let (text, range, offset) = excerpt(b"ab\xffcd needle", 6..12);
        assert_eq!(text, "ab\u{FFFD}cd needle");
        assert_eq!(&text[range], "needle");
        assert_eq!(offset, 0);

        let mut line = vec![b'x'; 2000];
        line.extend_from_slice("é needle é".as_bytes());
        line.extend(vec![b'y'; 2000]);
        let start = 2000 + 3;
        let (text, range, offset) = excerpt(&line, start..start + 6);
        assert_eq!(&text[range.clone()], "needle");
        assert_eq!(offset, start - WINDOW_BEFORE);
        assert_eq!(offset + range.start, start);
        assert!(text.starts_with("xxx"), "{text}");
        assert!(text.ends_with("yyy"), "{text}");
        assert_eq!(text.len(), WINDOW_BEFORE + 6 + WINDOW_AFTER);
        assert!(text.contains("é needle é"));
        // A window edge inside a multi-byte character moves past it.
        let mut line = vec![b'x'; 2000];
        line.extend_from_slice("é".as_bytes());
        line.extend_from_slice(b"needle");
        let start = 2000 + 2;
        let window_start = start - WINDOW_BEFORE;
        let mut line2 = line.clone();
        // Put a multi-byte character right across the window's start.
        line2[window_start - 1] = 0xC3;
        line2[window_start] = 0xA9;
        let (text, range, offset) = excerpt(&line2, start..start + 6);
        assert_eq!(&text[range], "needle");
        assert_eq!(offset, start - WINDOW_BEFORE + 1);
        assert!(!text.starts_with('\u{FFFD}'), "{text}");
    }
}
