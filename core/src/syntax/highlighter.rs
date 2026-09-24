//! Incremental highlighting of a buffer.
//!
//! A [`Highlighter`] keeps the [`LexState`] at the start of every line of
//! a buffer, and nothing else: tokens for a line are produced on demand
//! from its state (see [`Highlighter::tokens`]). The states are what make
//! highlighting a line cheap regardless of what precedes it.
//!
//! # Keeping the states current
//!
//! An edit is reported as a replacement of some lines by some others (see
//! [`Highlighter::lines_changed`]). The state at the start of the first
//! changed line is untouched, since it depends only on earlier text; the
//! states of newly inserted lines are unknown; and the states of every
//! later line are kept as *guesses*, since most edits don't change them.
//! Re-lexing then sweeps forward from the changed line, replacing guesses,
//! until a computed state equals the guess for a line. From there on the
//! guesses were right, and the sweep stops (or jumps to the next place an
//! edit happened while it was busy). Typing inside a comment converges on
//! the next line; opening a block comment at the top of a file sweeps to
//! the end.
//!
//! Sweeps run on a worker thread over a [`BufferSnapshot`], which is cheap
//! to take and never blocks editing. The worker merges its results in
//! batches, and every batch is checked against the edits that happened
//! since its snapshot: states for lines above the earliest such edit are
//! still correct and are kept, the rest are dropped and the worker starts
//! over from a fresh snapshot.
//!
//! So that the lines around the cursor never lag behind the worker, asking
//! for a line's tokens first lexes forward synchronously from the sweep
//! position if that line is within [`SYNC_LIMIT`] lines of it. After an
//! edit the viewport is right there, so what the user sees is always
//! current; only lines far below show their guesses until the worker
//! reaches them. Frontends poll [`Highlighter::generation`] to learn when
//! the worker has changed something worth redrawing.

use super::{ConflictSide, Context, Language, LexState, Lexer, Token};
use crate::buffer::{BufferSnapshot, FileBuffer};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

/// Lines the worker lexes between merges.
const BATCH: usize = 2048;
/// How far past the sweep position a line may be for its tokens to be
/// computed synchronously rather than left to the worker.
const SYNC_LIMIT: usize = 2000;

/// The states of a buffer's lines and the bookkeeping of what is stale.
struct Shared {
    /// The state at the start of each line; `states.len()` is the line
    /// count. [`LexState::UNKNOWN`] for lines not yet lexed.
    states: Vec<LexState>,
    /// Sorted lines from which re-lexing still has to sweep forward. The
    /// first is the current sweep's position, whose own state is valid;
    /// later entries are lines whose content changed while the sweep was
    /// elsewhere. Empty when everything is current.
    dirty: Vec<usize>,
    /// Bumped by every edit.
    version: u64,
    /// The version of the snapshot the worker was last given.
    job_version: u64,
    /// The lowest line edited since that snapshot was taken, or
    /// `usize::MAX`. Worker results for lines at or above it are valid.
    floor: usize,
}

/// What the worker gets to sweep.
struct Job {
    snapshot: BufferSnapshot,
    version: u64,
    /// The line to start from, whose state is `state`.
    from: usize,
    state: LexState,
}

/// The outcome of merging a batch of computed states.
enum Merge {
    /// Keep going from where the batch ended.
    Continue,
    /// The batch converged; the next sweep starts at this line and state.
    JumpTo(usize, LexState),
    /// Everything is current.
    Done,
    /// The batch was computed from text that has since changed.
    Stale,
}

/// Incremental syntax highlighting for one buffer. See the [module
/// documentation](self).
pub struct Highlighter {
    language: Language,
    lexer: &'static dyn Lexer,
    shared: Arc<Mutex<Shared>>,
    generation: Arc<AtomicU64>,
    jobs: Sender<Job>,
}

impl Highlighter {
    /// Start highlighting a buffer of `line_count` lines. Nothing is lexed
    /// until [`tokens`](Self::tokens) is first asked for.
    pub fn new(language: Language, line_count: usize) -> Highlighter {
        let mut states = vec![LexState::UNKNOWN; line_count.max(1)];
        states[0] = LexState::default();
        let shared = Arc::new(Mutex::new(Shared {
            states,
            dirty: vec![0],
            version: 0,
            job_version: u64::MAX,
            floor: usize::MAX,
        }));
        let generation = Arc::new(AtomicU64::new(0));
        let (jobs, rx) = mpsc::channel();
        let lexer = language.file_lexer();
        let worker_shared = Arc::clone(&shared);
        let worker_generation = Arc::clone(&generation);
        std::thread::Builder::new()
            .name(format!("highlight-{}", language.name()))
            .spawn(move || run_worker(rx, worker_shared, worker_generation, lexer))
            .expect("spawn highlighter thread");
        Highlighter {
            language,
            lexer,
            shared,
            generation,
            jobs,
        }
    }

    pub fn language(&self) -> Language {
        self.language
    }

    /// A counter that changes whenever the worker updates the states of
    /// lines, so a frontend knows to redraw.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// The number of lines being tracked. Always equals the line count of
    /// the buffer the highlighter is kept in step with.
    pub fn line_count(&self) -> usize {
        self.shared.lock().unwrap().states.len()
    }

    /// Whether every line's state is current.
    pub fn is_current(&self) -> bool {
        self.shared.lock().unwrap().dirty.is_empty()
    }

    /// Record that `old_count` lines starting at `start` were replaced by
    /// `new_count` lines (both at least 1: an edit within a line replaces
    /// that one line with itself). `line_count` is the buffer's new line
    /// count, as a check that the two haven't drifted apart.
    pub fn lines_changed(
        &self,
        start: usize,
        old_count: usize,
        new_count: usize,
        line_count: usize,
    ) {
        let mut shared = self.shared.lock().unwrap();
        shared.lines_changed(start, old_count, new_count);
        if shared.states.len() != line_count {
            // Shouldn't happen; start over rather than index out of bounds.
            shared.reset(line_count);
        }
    }

    /// The tokens of a line, lexed from its cached state. `buffer` must be
    /// the buffer the highlighter has been kept in step with.
    ///
    /// If the line is a little way past the current sweep position, the
    /// states up to it are computed here and now; otherwise its cached
    /// state (possibly a guess, possibly unknown) is used and the worker is
    /// left to catch up.
    pub fn tokens(&self, buffer: &FileBuffer, line: usize) -> Vec<Token> {
        let range = buffer.line_content_range(line);
        let content = buffer.bytes_in_range(range);
        self.tokens_of(buffer, line, &content)
    }

    /// As [`tokens`](Self::tokens), for a caller that already has the
    /// line's content (without its terminator) in hand.
    pub fn tokens_of(&self, buffer: &FileBuffer, line: usize, content: &[u8]) -> Vec<Token> {
        self.tokens_of_lines(buffer, line, &[content])
            .pop()
            .unwrap_or_default()
    }

    /// The tokens of lines not (yet) in the buffer: the first of
    /// `contents` lexed from `line`'s cached state as in
    /// [`tokens_of`](Self::tokens_of), and each one after it from the
    /// state the one before leaves, as if they followed it in the buffer.
    pub fn tokens_of_lines(
        &self,
        buffer: &FileBuffer,
        line: usize,
        contents: &[&[u8]],
    ) -> Vec<Vec<Token>> {
        self.catch_up(buffer, line);
        let state = {
            let shared = self.shared.lock().unwrap();
            shared
                .states
                .get(line)
                .copied()
                .unwrap_or(LexState::UNKNOWN)
        };
        let mut state = if state.is_unknown() {
            LexState::default()
        } else {
            state
        };
        contents
            .iter()
            .map(|content| {
                let mut tokens = Vec::new();
                state = self.lexer.lex_line(state, content, &mut tokens);
                tokens
            })
            .collect()
    }

    /// The side of a merge conflict that `line` is on, if any. A marker
    /// line counts with the side it opens, and the closing `>>>>>>>` with
    /// the side it closes, so a whole conflict is covered from its first
    /// marker to its last. Read from the cached states like
    /// [`tokens`](Self::tokens), with the same catching up.
    pub fn conflict_side(&self, buffer: &FileBuffer, line: usize) -> Option<ConflictSide> {
        // The state at the end of the line is the state at the start of
        // the next, which is what tells a marker line's own side.
        self.catch_up(buffer, line + 1);
        let shared = self.shared.lock().unwrap();
        let side_at = |line: usize| match shared.states.get(line) {
            Some(state) if !state.is_unknown() => match state.bottom() {
                Some(Context::Conflict { side }) => Some(side),
                _ => None,
            },
            _ => None,
        };
        side_at(line + 1).or_else(|| side_at(line))
    }

    /// Bring the states up to `line` current if the sweep position is
    /// within [`SYNC_LIMIT`] lines of it, then make sure the worker has a
    /// job if anything is still stale.
    fn catch_up(&self, buffer: &FileBuffer, line: usize) {
        let (start, needs_job) = {
            let mut shared = self.shared.lock().unwrap();
            match shared.sweep_start() {
                None => return,
                Some((from, state)) => {
                    let start = (from < line && line - from <= SYNC_LIMIT).then_some((from, state));
                    (start, shared.job_version != shared.version)
                }
            }
        };
        if start.is_none() && !needs_job {
            return;
        }
        let snapshot = buffer.snapshot();
        if let Some((from, mut state)) = start {
            let line_count = snapshot.line_count();
            let mut lines = snapshot.lines_from(from);
            let mut batch = Vec::with_capacity(line - from);
            let mut scratch = Vec::new();
            let mut current = from;
            let mut finished = false;
            while current < line {
                let Some(text) = lines.next_line() else {
                    finished = true;
                    break;
                };
                scratch.clear();
                state = self.lexer.lex_line(state, text, &mut scratch);
                current += 1;
                if current < line_count {
                    batch.push(state);
                } else {
                    finished = true;
                    break;
                }
            }
            let mut shared = self.shared.lock().unwrap();
            shared.merge(None, from + 1, &batch, finished);
        }
        let mut shared = self.shared.lock().unwrap();
        if let Some((from, state)) = shared.sweep_start()
            && shared.job_version != shared.version
        {
            shared.job_version = shared.version;
            shared.floor = usize::MAX;
            let _ = self.jobs.send(Job {
                snapshot,
                version: shared.version,
                from,
                state,
            });
        }
    }
}

impl Shared {
    fn reset(&mut self, line_count: usize) {
        self.states.clear();
        self.states.resize(line_count.max(1), LexState::UNKNOWN);
        self.states[0] = LexState::default();
        self.dirty = vec![0];
        self.version += 1;
        self.floor = 0;
    }

    fn lines_changed(&mut self, start: usize, old_count: usize, new_count: usize) {
        let old_count = old_count.max(1);
        let new_count = new_count.max(1);
        let start = start.min(self.states.len() - 1);
        let old_end = (start + old_count).min(self.states.len());
        // The changed line keeps its start state; inserted lines are
        // unknown; later lines keep their states as guesses.
        self.states.splice(
            start + 1..old_end,
            std::iter::repeat_n(LexState::UNKNOWN, new_count - 1),
        );
        for d in &mut self.dirty {
            if *d > start && *d < old_end {
                *d = start;
            } else if *d >= old_end {
                *d = *d + new_count - old_count;
            }
        }
        self.dirty.push(start);
        self.dirty.sort_unstable();
        self.dirty.dedup();
        self.version += 1;
        self.floor = self.floor.min(start);
    }

    /// Where the next sweep starts and with what state, or `None` when
    /// everything is current. The start is moved back over unknown states
    /// (which shouldn't be there, but would be unrecoverable otherwise).
    fn sweep_start(&mut self) -> Option<(usize, LexState)> {
        let mut from = *self.dirty.first()?;
        from = from.min(self.states.len() - 1);
        while self.states[from].is_unknown() && from > 0 {
            from -= 1;
        }
        self.dirty[0] = from;
        if self.states[from].is_unknown() {
            self.states[from] = LexState::default();
        }
        Some((from, self.states[from]))
    }

    /// Merge computed states for lines `first..first + batch.len()`. With
    /// `job_version`, the batch came from the worker's snapshot of that
    /// version and is checked against edits made since; without, it was
    /// computed from the live buffer. `finished` says the batch reached
    /// the end of the text.
    fn merge(
        &mut self,
        job_version: Option<u64>,
        first: usize,
        batch: &[LexState],
        finished: bool,
    ) -> Merge {
        let valid_through = match job_version {
            Some(version) if version != self.job_version => return Merge::Stale,
            Some(_) => self.floor,
            None => usize::MAX,
        };
        let Some(&position) = self.dirty.first() else {
            return Merge::Done;
        };
        for (k, &state) in batch.iter().enumerate() {
            let line = first + k;
            if line > valid_through {
                return Merge::Stale;
            }
            if line >= self.states.len() {
                break;
            }
            if line <= position {
                // Already current, by an earlier merge.
                continue;
            }
            if self.states[line] == state {
                // Converged: the guesses from here to the next dirty line
                // were right.
                self.dirty.retain(|&d| d >= line);
                return match self.dirty.first() {
                    Some(&next) => Merge::JumpTo(next, self.states[next]),
                    None => Merge::Done,
                };
            }
            self.states[line] = state;
            self.dirty.retain(|&d| d > line);
            self.dirty.insert(0, line);
        }
        if finished {
            // Swept to the end; only lines edited since the snapshot (which
            // the batch stopped short of) can still be stale.
            self.dirty.retain(|&d| d >= valid_through);
            return match self.dirty.first() {
                Some(&next) => Merge::JumpTo(next, self.states[next]),
                None => Merge::Done,
            };
        }
        Merge::Continue
    }
}

/// The worker: sweeps snapshots forward from a job's start line, merging
/// a batch at a time, until the sweep converges or is overtaken by an
/// edit. Exits when the highlighter is dropped.
fn run_worker(
    jobs: Receiver<Job>,
    shared: Arc<Mutex<Shared>>,
    generation: Arc<AtomicU64>,
    lexer: &'static dyn Lexer,
) {
    let mut scratch = Vec::new();
    let mut batch = Vec::with_capacity(BATCH);
    let mut pending = None;
    loop {
        let mut job = match pending.take() {
            Some(job) => job,
            None => match jobs.recv() {
                Ok(job) => job,
                Err(_) => return,
            },
        };
        // Only the newest job matters.
        while let Ok(newer) = jobs.try_recv() {
            job = newer;
        }
        let line_count = job.snapshot.line_count();
        let mut lines = job.snapshot.lines_from(job.from);
        let mut state = job.state;
        let mut line = job.from;
        loop {
            batch.clear();
            let first = line + 1;
            let mut finished = false;
            while batch.len() < BATCH {
                let Some(text) = lines.next_line() else {
                    finished = true;
                    break;
                };
                scratch.clear();
                state = lexer.lex_line(state, text, &mut scratch);
                line += 1;
                if line < line_count {
                    batch.push(state);
                } else {
                    finished = true;
                    break;
                }
            }
            let outcome = shared
                .lock()
                .unwrap()
                .merge(Some(job.version), first, &batch, finished);
            generation.fetch_add(1, Ordering::Release);
            match outcome {
                Merge::Continue if !finished => {}
                Merge::Continue | Merge::Done | Merge::Stale => break,
                Merge::JumpTo(from, from_state) => {
                    lines = job.snapshot.lines_from(from);
                    state = from_state;
                    line = from;
                }
            }
            // Let a newer job preempt a long sweep.
            match jobs.try_recv() {
                Ok(newer) => {
                    pending = Some(newer);
                    break;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::TokenKind;
    use super::*;
    use crate::buffer::FileBuffer;
    use std::time::{Duration, Instant};

    /// Lex the whole buffer from scratch and return every line's state,
    /// the reference the incremental states must match.
    fn reference_states(language: Language, buffer: &FileBuffer) -> Vec<LexState> {
        let lexer = language.file_lexer();
        let snapshot = buffer.snapshot();
        let mut lines = snapshot.lines_from(0);
        let mut states = vec![LexState::default()];
        let mut state = LexState::default();
        let mut scratch = Vec::new();
        while let Some(text) = lines.next_line() {
            scratch.clear();
            state = lexer.lex_line(state, text, &mut scratch);
            states.push(state);
        }
        states.pop();
        states
    }

    fn wait_until_current(highlighter: &Highlighter) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !highlighter.is_current() {
            assert!(Instant::now() < deadline, "highlighter never caught up");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn states_of(highlighter: &Highlighter) -> Vec<LexState> {
        highlighter.shared.lock().unwrap().states.clone()
    }

    /// Apply a replacement to both the buffer and the highlighter the way
    /// the editor does.
    fn replace(
        buffer: &mut FileBuffer,
        highlighter: &Highlighter,
        offset: usize,
        len: usize,
        text: &str,
    ) {
        let start = buffer.line_of_offset(offset);
        let old_end = buffer.line_of_offset(offset + len);
        buffer.delete(offset..offset + len);
        buffer.insert(offset, text);
        let new_end = buffer.line_of_offset(offset + text.len());
        highlighter.lines_changed(
            start,
            old_end - start + 1,
            new_end - start + 1,
            buffer.line_count(),
        );
    }

    #[test]
    fn tokens_of_lines_in_a_comment() {
        let text = "fn a() {}\n/* start\nmiddle\nend */ fn b() {}\n";
        let buffer = FileBuffer::from_text(text);
        let highlighter = Highlighter::new(Language::Rust, buffer.line_count());
        let tokens = highlighter.tokens(&buffer, 2);
        assert_eq!(
            tokens,
            vec![Token {
                range: 0..6,
                kind: TokenKind::Comment
            }]
        );
        let tokens = highlighter.tokens(&buffer, 3);
        assert_eq!(
            tokens[0],
            Token {
                range: 0..6,
                kind: TokenKind::Comment
            }
        );
        assert_eq!(tokens[1].kind, TokenKind::Keyword);
        let tokens = highlighter.tokens(&buffer, 0);
        assert_eq!(tokens[0].kind, TokenKind::Keyword);
        assert_eq!(tokens[1].kind, TokenKind::FunctionDefinition);
        wait_until_current(&highlighter);
        assert_eq!(
            states_of(&highlighter),
            reference_states(Language::Rust, &buffer)
        );
    }

    #[test]
    fn worker_sweeps_large_files() {
        let mut text = String::new();
        for i in 0..20_000 {
            text.push_str(&format!("let x{i} = {i}; // line\n"));
        }
        let mut buffer = FileBuffer::from_text(&text);
        let highlighter = Highlighter::new(Language::Rust, buffer.line_count());
        // Asking for a line far from the start leaves the rest to the
        // worker.
        let generation = highlighter.generation();
        let _ = highlighter.tokens(&buffer, 15_000);
        wait_until_current(&highlighter);
        assert!(highlighter.generation() > generation);
        assert_eq!(
            states_of(&highlighter),
            reference_states(Language::Rust, &buffer)
        );

        // Open a comment at the top: every state after it changes.
        replace(&mut buffer, &highlighter, 0, 0, "/* ");
        assert!(!highlighter.is_current());
        let tokens = highlighter.tokens(&buffer, 1);
        assert_eq!(tokens[0].kind, TokenKind::Comment);
        wait_until_current(&highlighter);
        assert_eq!(
            states_of(&highlighter),
            reference_states(Language::Rust, &buffer)
        );

        // Close it again near the bottom while editing near the top: the
        // worker's sweep is overtaken by the edit and recovers.
        let offset = buffer.offset_of_line(19_000);
        replace(&mut buffer, &highlighter, offset, 0, "*/ ");
        let _ = highlighter.tokens(&buffer, 19_001);
        replace(&mut buffer, &highlighter, 3, 0, "x");
        let _ = highlighter.tokens(&buffer, 0);
        wait_until_current(&highlighter);
        assert_eq!(
            states_of(&highlighter),
            reference_states(Language::Rust, &buffer)
        );
    }

    #[test]
    fn random_edits_match_a_fresh_lex() {
        // A small deterministic generator, so the test needs no crates.
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        let pieces = [
            "fn ",
            "/* ",
            " */",
            "\"",
            "'",
            "\n",
            "x",
            "{",
            "}",
            "// c\n",
            "r#\"",
            "\"#",
            "1",
            ";",
            "\n\n",
            "let ",
            "'a",
            "\\",
            "b\"",
            "`",
            "${",
            "```rust\n",
            "```\n",
            "<!--",
            "-->",
            "# ",
            "    ",
        ];
        for language in [
            Language::Rust,
            Language::JavaScript,
            Language::Toml,
            Language::Python,
        ] {
            let mut buffer = FileBuffer::from_text("fn main() {\n    // start\n}\n");
            let highlighter = Highlighter::new(language, buffer.line_count());
            for round in 0..300 {
                let len = buffer.len();
                let offset = next(len + 1);
                let remove = next(4).min(len - offset);
                let insert = if next(3) == 0 {
                    ""
                } else {
                    pieces[next(pieces.len())]
                };
                replace(&mut buffer, &highlighter, offset, remove, insert);
                // Look at a few lines, sometimes letting the worker do the
                // rest.
                let line_count = buffer.line_count();
                let _ = highlighter.tokens(&buffer, next(line_count));
                if round % 7 == 0 {
                    wait_until_current(&highlighter);
                    assert_eq!(
                        states_of(&highlighter),
                        reference_states(language, &buffer),
                        "{language:?} round {round}: {:?}",
                        buffer.to_text()
                    );
                }
            }
            wait_until_current(&highlighter);
            assert_eq!(states_of(&highlighter), reference_states(language, &buffer));
        }
    }

    /// Timing of a full sweep over a large file; run by hand with
    /// `cargo test --release -p ninjaedit-core sweep_timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn sweep_timing() {
        let mut text = String::new();
        for i in 0..500_000 {
            text.push_str(&format!(
                "    let value_{i} = self.items.get({i}).map(|x| x.name.len()) + 0x1f; // note\n"
            ));
        }
        let mut buffer = FileBuffer::from_text(&text);
        let highlighter = Highlighter::new(Language::Rust, buffer.line_count());
        let start = Instant::now();
        let _ = highlighter.tokens(&buffer, 250_000);
        wait_until_current(&highlighter);
        eprintln!(
            "full sweep of {} lines: {:?}",
            buffer.line_count(),
            start.elapsed()
        );

        let start = Instant::now();
        replace(&mut buffer, &highlighter, 0, 0, "/* ");
        let edit = start.elapsed();
        let start = Instant::now();
        let tokens: Vec<_> = (0..60)
            .map(|line| highlighter.tokens(&buffer, line))
            .collect();
        let draw = start.elapsed();
        assert_eq!(tokens[30][0].kind, TokenKind::Comment);
        wait_until_current(&highlighter);
        eprintln!(
            "edit bookkeeping {edit:?}, drawing 60 lines {draw:?}, resweep {:?}",
            start.elapsed()
        );
        let states = highlighter.shared.lock().unwrap();
        eprintln!(
            "state cache: {} bytes per line, {} MB total",
            std::mem::size_of::<LexState>(),
            states.states.len() * std::mem::size_of::<LexState>() / 1_000_000
        );
    }

    #[test]
    fn line_count_drift_resets() {
        let buffer = FileBuffer::from_text("a\nb\nc\n");
        let highlighter = Highlighter::new(Language::Rust, buffer.line_count());
        highlighter.lines_changed(1, 1, 1, 10);
        assert_eq!(highlighter.line_count(), 10);
    }
}
