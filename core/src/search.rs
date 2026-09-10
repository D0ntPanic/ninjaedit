//! Searching a buffer for text, off the main thread.
//!
//! A [`Search`] is started at an *origin* offset (the cursor) over a
//! [`BufferSnapshot`], and given a query. Matches are found on a worker
//! thread so that typing a query never waits on a large file; the caller
//! polls [`Search::generation`] to learn when new matches have arrived, or
//! waits briefly with [`Search::wait_for`] so a small file shows its
//! results in the same frame.
//!
//! The query is plain text, matched literally and case-sensitively, unless
//! it starts with a `/`, in which case the rest is a regular expression
//! (the [`regex`] crate's syntax). Either way a match never spans a line
//! break: the worker walks the snapshot a line at a time, so `^` and `$`
//! anchor to lines and a pattern containing `\n` finds nothing. Empty
//! matches are ignored.
//!
//! Matches are ordered from the origin: the first match at or after the
//! origin comes first, then the rest of the file, then the matches before
//! the origin, wrapping around. The worker searches in that order too, so
//! the first match is known almost at once even in a huge file. One match
//! is *current*, starting with the first; [`Search::advance`] steps
//! through them and reports when it comes back around to the start.

use crate::buffer::{BufferSnapshot, FileBuffer};
use regex::bytes::Regex;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Lines the worker searches between publishing what it found.
const BATCH: usize = 4096;

/// The outcome of [`Search::advance`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchStep {
    /// The current match moved on to this one.
    Moved(Range<usize>),
    /// The current match is the last one before the search wraps around
    /// to where it started. Nothing moved; advancing again wraps.
    ReachedStart,
    NoMatches,
}

/// The matches found so far for one query.
struct Found {
    /// Matches before the origin, ascending. Filled in after `tail`.
    head: Vec<Range<usize>>,
    /// Matches at or after the origin, ascending.
    tail: Vec<Range<usize>>,
    done: bool,
}

struct Results {
    found: Mutex<Found>,
    changed: Condvar,
}

impl Results {
    /// Empty results, `done` if nothing is going to fill them in.
    fn new(done: bool) -> Arc<Results> {
        Arc::new(Results {
            found: Mutex::new(Found {
                head: Vec::new(),
                tail: Vec::new(),
                done,
            }),
            changed: Condvar::new(),
        })
    }
}

impl Found {
    fn len(&self) -> usize {
        self.head.len() + self.tail.len()
    }

    /// The match `index` steps from the origin.
    fn nth(&self, index: usize) -> Option<Range<usize>> {
        if index < self.tail.len() {
            self.tail.get(index).cloned()
        } else {
            self.head.get(index - self.tail.len()).cloned()
        }
    }
}

/// A search of one buffer for one query; see the [module
/// documentation](self).
pub struct Search {
    snapshot: BufferSnapshot,
    origin: usize,
    origin_line: usize,
    query: String,
    /// The compiled query, or why it didn't compile. `Ok(None)` for an
    /// empty query, which matches nothing.
    pattern: Result<Option<Regex>, String>,
    results: Arc<Results>,
    /// Tells the worker for the current query to stop.
    cancel: Arc<AtomicBool>,
    /// Bumped whenever a worker publishes matches. Shared by every worker
    /// this search starts so it only ever grows.
    generation: Arc<AtomicU64>,
    /// Steps from the origin to the current match.
    current: usize,
    /// Whether the last advance stopped at the end rather than wrapping.
    at_end: bool,
    accepted: bool,
}

impl Search {
    /// Start a search of `buffer` from `origin`, with an empty query.
    pub fn new(buffer: &FileBuffer, origin: usize) -> Search {
        let origin = origin.min(buffer.len());
        Search {
            snapshot: buffer.snapshot(),
            origin,
            origin_line: buffer.line_of_offset(origin),
            query: String::new(),
            pattern: Ok(None),
            results: Results::new(true),
            cancel: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
            current: 0,
            at_end: false,
            accepted: false,
        }
    }

    /// The offset the search started from.
    pub fn origin(&self) -> usize {
        self.origin
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

    /// Whether the current match has been selected in the editor, as
    /// opposed to only being previewed.
    pub fn is_accepted(&self) -> bool {
        self.accepted
    }

    pub(crate) fn set_accepted(&mut self) {
        self.accepted = true;
    }

    /// Replace the query and search for it afresh from the origin. Matches
    /// found so far for the old query are discarded at once.
    pub fn set_query(&mut self, query: &str) {
        if query == self.query {
            return;
        }
        self.cancel.store(true, Ordering::Release);
        self.query = query.to_owned();
        self.pattern = compile(query);
        self.results = Results::new(!matches!(self.pattern, Ok(Some(_))));
        self.cancel = Arc::new(AtomicBool::new(false));
        self.current = 0;
        self.at_end = false;
        if let Ok(Some(regex)) = &self.pattern {
            let job = Job {
                regex: regex.clone(),
                snapshot: self.snapshot.clone(),
                origin: self.origin,
                origin_line: self.origin_line,
                results: Arc::clone(&self.results),
                cancel: Arc::clone(&self.cancel),
                generation: Arc::clone(&self.generation),
            };
            std::thread::Builder::new()
                .name("search".to_owned())
                .spawn(move || job.run())
                .expect("spawn search thread");
        }
    }

    /// A counter that changes whenever the worker has found more matches,
    /// so a frontend knows to redraw.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Whether the whole buffer has been searched.
    pub fn is_done(&self) -> bool {
        self.results.found.lock().unwrap().done
    }

    /// Block until the whole buffer has been searched.
    pub fn wait(&self) {
        let mut found = self.results.found.lock().unwrap();
        while !found.done {
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
            .wait_timeout_while(found, timeout, |found| !found.done)
            .unwrap();
        found.done
    }

    /// The number of matches found so far.
    pub fn match_count(&self) -> usize {
        self.results.found.lock().unwrap().len()
    }

    /// The current match, if it is known yet. The first match is known as
    /// soon as the worker reaches it; until the search is done, `None`
    /// only means it hasn't been found yet.
    pub fn current(&self) -> Option<Range<usize>> {
        self.results.found.lock().unwrap().nth(self.current)
    }

    /// Make the next match current, wrapping around the end of the buffer.
    /// Coming back around to the first match takes two steps: the first
    /// reports [`SearchStep::ReachedStart`] without moving, the second
    /// wraps. Waits for the search to finish if it hasn't.
    pub fn advance(&mut self) -> SearchStep {
        self.wait();
        let found = self.results.found.lock().unwrap();
        if found.len() == 0 {
            return SearchStep::NoMatches;
        }
        let next = self.current + 1;
        if next < found.len() {
            self.current = next;
            self.at_end = false;
        } else if self.at_end {
            self.current = 0;
            self.at_end = false;
        } else {
            self.at_end = true;
            return SearchStep::ReachedStart;
        }
        SearchStep::Moved(found.nth(self.current).expect("index in range"))
    }

    /// The matches found so far that overlap `range`, ascending.
    pub fn matches_in(&self, range: Range<usize>) -> Vec<Range<usize>> {
        let found = self.results.found.lock().unwrap();
        let mut out = Vec::new();
        for list in [&found.head, &found.tail] {
            let first = list.partition_point(|m| m.end <= range.start);
            out.extend(
                list[first..]
                    .iter()
                    .take_while(|m| m.start < range.end)
                    .cloned(),
            );
        }
        out
    }
}

impl Drop for Search {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

/// Compile a query: a literal, or a regular expression after a `/`.
fn compile(query: &str) -> Result<Option<Regex>, String> {
    let pattern = match query.strip_prefix('/') {
        Some("") => return Ok(None),
        Some(pattern) => pattern.to_owned(),
        None if query.is_empty() => return Ok(None),
        None => regex::escape(query),
    };
    Regex::new(&pattern)
        .map(Some)
        .map_err(|err| brief_error(&err))
}

/// The regex crate's errors span several lines, pointing into the
/// pattern; keep just the last line's description.
fn brief_error(err: &regex::Error) -> String {
    let text = err.to_string();
    let last = text.lines().last().unwrap_or(&text);
    last.trim_start_matches("error: ").to_owned()
}

/// What a worker searches.
struct Job {
    regex: Regex,
    snapshot: BufferSnapshot,
    origin: usize,
    origin_line: usize,
    results: Arc<Results>,
    cancel: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
}

impl Job {
    fn run(self) {
        // The origin's own line is searched first, but its matches before
        // the origin belong at the very end of the order.
        let mut origin_line_before = Vec::new();
        let mut batch = Vec::new();
        let mut lines = self.snapshot.lines_from(self.origin_line);
        let mut line = self.origin_line;
        let publish = |batch: &mut Vec<Range<usize>>, to_head: bool, done: bool| -> bool {
            if self.cancel.load(Ordering::Acquire) {
                return false;
            }
            if batch.is_empty() && !done {
                return true;
            }
            let mut found = self.results.found.lock().unwrap();
            if to_head {
                found.head.append(batch);
            } else {
                found.tail.append(batch);
            }
            found.done = done;
            drop(found);
            self.generation.fetch_add(1, Ordering::Release);
            self.results.changed.notify_all();
            true
        };

        // From the origin to the end of the buffer.
        loop {
            let mut finished = false;
            for _ in 0..BATCH {
                let start = lines.offset();
                let Some(text) = lines.next_line() else {
                    finished = true;
                    break;
                };
                for m in self.regex.find_iter(text) {
                    if m.is_empty() {
                        continue;
                    }
                    let range = start + m.start()..start + m.end();
                    if line == self.origin_line && range.start < self.origin {
                        origin_line_before.push(range);
                    } else {
                        batch.push(range);
                    }
                }
                line += 1;
            }
            if !publish(&mut batch, false, false) {
                return;
            }
            if finished {
                break;
            }
        }

        // Then from the start of the buffer up to the origin's line.
        let mut lines = self.snapshot.lines_from(0);
        let mut line = 0;
        while line < self.origin_line {
            for _ in 0..BATCH {
                if line >= self.origin_line {
                    break;
                }
                let start = lines.offset();
                let Some(text) = lines.next_line() else {
                    break;
                };
                batch.extend(
                    self.regex
                        .find_iter(text)
                        .filter(|m| !m.is_empty())
                        .map(|m| start + m.start()..start + m.end()),
                );
                line += 1;
            }
            if !publish(&mut batch, true, false) {
                return;
            }
        }
        batch.append(&mut origin_line_before);
        publish(&mut batch, true, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search(text: &str, origin: usize, query: &str) -> Search {
        let buffer = FileBuffer::from_text(text);
        let mut search = Search::new(&buffer, origin);
        search.set_query(query);
        search.wait();
        search
    }

    fn all(search: &Search) -> Vec<Range<usize>> {
        search.matches_in(0..usize::MAX)
    }

    #[test]
    fn plain_text_matches_are_ordered_from_the_origin() {
        //                    0    5    10   15
        let text = "ab ab\nab ab\nab";
        let mut s = search(text, 7, "ab");
        assert_eq!(all(&s), vec![0..2, 3..5, 6..8, 9..11, 12..14]);
        assert_eq!(s.match_count(), 5);
        // The origin is inside the third match, so the first match at or
        // after it is the fourth.
        assert_eq!(s.current(), Some(9..11));
        assert_eq!(s.advance(), SearchStep::Moved(12..14));
        assert_eq!(s.advance(), SearchStep::Moved(0..2));
        assert_eq!(s.advance(), SearchStep::Moved(3..5));
        assert_eq!(s.advance(), SearchStep::Moved(6..8));
        assert_eq!(s.advance(), SearchStep::ReachedStart);
        assert_eq!(s.current(), Some(6..8));
        assert_eq!(s.advance(), SearchStep::Moved(9..11));
        assert_eq!(s.advance(), SearchStep::Moved(12..14));
    }

    #[test]
    fn origin_at_a_match_start_makes_it_first() {
        let mut s = search("ab ab ab", 3, "ab");
        assert_eq!(s.current(), Some(3..5));
        assert_eq!(s.advance(), SearchStep::Moved(6..8));
        assert_eq!(s.advance(), SearchStep::Moved(0..2));
        assert_eq!(s.advance(), SearchStep::ReachedStart);
        // A lone match: advancing reports the start, then stays put.
        let mut s = search("xx ab xx", 6, "ab");
        assert_eq!(s.current(), Some(3..5));
        assert_eq!(s.advance(), SearchStep::ReachedStart);
        assert_eq!(s.advance(), SearchStep::Moved(3..5));
        assert_eq!(s.advance(), SearchStep::ReachedStart);
    }

    #[test]
    fn regex_queries_and_errors() {
        let s = search("foo1 bar22 baz333\n", 0, "/[a-z]+(\\d+)");
        assert!(s.is_regex());
        assert_eq!(all(&s), vec![0..4, 5..10, 11..17]);
        // Plain text is literal: the same query without the slash finds
        // nothing, and metacharacters match themselves.
        assert!(all(&search("a+b a.b", 0, "a+b")).len() == 1);
        assert_eq!(all(&search("a+b a.b", 0, "a.b")), vec![4..7]);
        let mut s = search("abc", 0, "/(");
        assert_eq!(s.error(), Some("unclosed group"));
        assert!(all(&s).is_empty());
        assert!(s.is_done());
        assert_eq!(s.advance(), SearchStep::NoMatches);
        // A bare slash matches nothing, as does the empty query.
        assert!(all(&search("abc", 0, "/")).is_empty());
        assert!(all(&search("abc", 0, "")).is_empty());
        // Empty matches are dropped rather than highlighted or stepped to.
        assert_eq!(all(&search("aa b aa", 0, "/a*")), vec![0..2, 5..7]);
    }

    #[test]
    fn matches_stay_within_lines() {
        let s = search("a\nb\r\nab\n", 0, "/a.b");
        assert!(all(&s).is_empty());
        let s = search("ab\r\nxab\r\nab", 0, "/^ab$");
        assert_eq!(all(&s), vec![0..2, 9..11]);
        // The end of a line is the end of the content, before the CRLF.
        let s = search("ab\r\nab", 0, "/b$");
        assert_eq!(all(&s), vec![1..2, 5..6]);
    }

    #[test]
    fn matches_in_a_range() {
        let s = search("ab ab ab ab", 4, "ab");
        assert_eq!(s.matches_in(0..3), vec![0..2]);
        assert_eq!(s.matches_in(1..8), vec![0..2, 3..5, 6..8]);
        assert_eq!(s.matches_in(4..7), vec![3..5, 6..8]);
        assert_eq!(s.matches_in(2..3), Vec::<Range<usize>>::new());
        assert_eq!(s.matches_in(9..11), vec![9..11]);
    }

    #[test]
    fn large_files_are_searched_in_order_from_the_origin() {
        let text: String = (0..50_000)
            .map(|i| {
                if i % 1000 == 500 {
                    format!("needle {i}\n")
                } else {
                    format!("line {i}\n")
                }
            })
            .collect();
        let buffer = FileBuffer::from_text(&text);
        let origin = buffer.offset_of_line(25_000);
        let mut search = Search::new(&buffer, origin);
        let generation = search.generation();
        search.set_query("needle");
        search.wait();
        assert!(search.generation() > generation);
        let matches = all(&search);
        assert_eq!(matches.len(), 50);
        assert!(matches.windows(2).all(|w| w[0].end <= w[1].start));
        assert_eq!(
            search.current(),
            Some(buffer.offset_of_line(25_500)..buffer.offset_of_line(25_500) + 6)
        );
        // Stepping through all fifty comes back around.
        for _ in 0..49 {
            assert!(matches!(search.advance(), SearchStep::Moved(_)));
        }
        assert_eq!(search.advance(), SearchStep::ReachedStart);
        assert_eq!(
            search.advance(),
            SearchStep::Moved(buffer.offset_of_line(25_500)..buffer.offset_of_line(25_500) + 6)
        );

        // Changing the query drops the old results at once and the new
        // ones arrive in time.
        search.set_query("line 4999");
        assert!(search.wait_for(Duration::from_secs(10)));
        assert_eq!(all(&search).len(), 11, "4999 and 49990..=49999");
        assert_eq!(
            search.current(),
            Some(buffer.offset_of_line(49_990)..buffer.offset_of_line(49_990) + 9)
        );
    }

    #[test]
    fn setting_the_same_query_keeps_the_position() {
        let mut s = search("a a a", 0, "a");
        s.advance();
        s.set_query("a");
        assert_eq!(s.current(), Some(2..3));
        s.set_query("a ");
        s.wait();
        assert_eq!(s.current(), Some(0..2));
    }
}
