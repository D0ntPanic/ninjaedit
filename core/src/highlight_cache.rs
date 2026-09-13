//! Syntax highlighting of whole files on demand, in the background, for
//! showing lines of files that aren't open in an editor: project search
//! results and the context around them.
//!
//! A [`HighlightCache`] is asked for a file with
//! [`states`](HighlightCache::states). The first time a file is asked
//! for, the answer is `None` and the file is queued for a worker thread,
//! which reads it (or takes it as an editor has it, if the cache was made
//! with [`with_buffers`](HighlightCache::with_buffers)) and lexes every
//! line from the top; from then on the answer is at hand. Files whose
//! language isn't known from their name are remembered as such and never
//! queued again. A frontend polls [`generation`](HighlightCache::generation)
//! to learn when a file it asked for has been lexed and is worth
//! redrawing with.
//!
//! What is kept for a file is not its tokens but, as the editor's
//! [`Highlighter`](crate::Highlighter) keeps, the small fixed-size
//! [`LexState`] at the start of each line: the tokens of any one line
//! follow from its state and its text (see [`FileStates::tokens`]), and
//! a frontend has the text of the lines it is drawing. Since many search
//! results usually fall in the same file, lexing a file once serves them
//! all. The cache holds up to [`MAX_FILES`] files, dropping the least
//! recently asked for beyond that.

use crate::project_search::{Buffers, lines};
use crate::syntax::{Language, LexState, Token};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use std::{fs, thread};

/// The most files kept lexed at once.
pub const MAX_FILES: usize = 64;

/// The lexer states at the start of every line of a file, from which any
/// line's tokens can be had given its text.
pub struct FileStates {
    language: Language,
    states: Vec<LexState>,
}

impl FileStates {
    pub fn language(&self) -> Language {
        self.language
    }

    /// The number of lines the file had when lexed.
    pub fn line_count(&self) -> usize {
        self.states.len()
    }

    /// The tokens of `line`, given its content (without its terminator),
    /// lexed from the state the line starts in. Empty for a line past the
    /// end of the file as it was lexed. The content may be a prefix of
    /// the line, since lexing runs left to right, but not a later part of
    /// it.
    pub fn tokens(&self, line: usize, content: &[u8]) -> Vec<Token> {
        let mut tokens = Vec::new();
        if let Some(&state) = self.states.get(line) {
            self.language
                .file_lexer()
                .lex_line(state, content, &mut tokens);
        }
        tokens
    }
}

enum Entry {
    /// Queued for the worker.
    Pending,
    /// The file has no known language (or couldn't be read).
    Unsupported,
    Lexed(Arc<FileStates>),
}

struct State {
    files: HashMap<Arc<Path>, Entry>,
    /// Files waiting to be lexed, oldest request first.
    queue: VecDeque<Arc<Path>>,
    /// Whether the worker is lexing a file it has taken off the queue.
    busy: bool,
    /// Lexed files by the time they were last asked for, oldest first.
    recent: Vec<(Instant, Arc<Path>)>,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    /// Signalled when the queue gains a file, or on shutdown.
    wake: Condvar,
    /// Signalled when the queue empties and the worker has nothing in
    /// hand.
    idle: Condvar,
    generation: AtomicU64,
    /// Files to lex as they are in editors rather than on disk.
    buffers: Arc<Buffers>,
}

/// A background lexer with a cache of the results; see the [module
/// documentation](self).
pub struct HighlightCache {
    shared: Arc<Shared>,
}

impl HighlightCache {
    pub fn new() -> HighlightCache {
        Self::with_buffers(Arc::new(Buffers::new()))
    }

    /// A cache that lexes the files in `buffers` as they are there, and
    /// the rest from disk. The buffers are fixed for the cache's life: a
    /// frontend makes a new cache when they change, as the project search
    /// dialog does whenever it is shown.
    pub fn with_buffers(buffers: Arc<Buffers>) -> HighlightCache {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                files: HashMap::new(),
                queue: VecDeque::new(),
                busy: false,
                recent: Vec::new(),
                shutdown: false,
            }),
            wake: Condvar::new(),
            idle: Condvar::new(),
            generation: AtomicU64::new(0),
            buffers,
        });
        let worker_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("highlight-cache".to_owned())
            .spawn(move || run_worker(worker_shared))
            .expect("spawn highlight cache thread");
        HighlightCache { shared }
    }

    /// The line states of `path`, if it has been lexed. Otherwise `None`,
    /// and the file is queued to be lexed if its language is known; ask
    /// again after [`generation`](Self::generation) changes.
    pub fn states(&self, path: &Arc<Path>) -> Option<Arc<FileStates>> {
        let mut state = self.shared.state.lock().unwrap();
        match state.files.get(path) {
            Some(Entry::Lexed(states)) => {
                let states = Arc::clone(states);
                state.touch(path);
                Some(states)
            }
            Some(Entry::Pending | Entry::Unsupported) => None,
            None => {
                if Language::from_path(path).is_none() {
                    state.files.insert(Arc::clone(path), Entry::Unsupported);
                } else {
                    state.files.insert(Arc::clone(path), Entry::Pending);
                    state.queue.push_back(Arc::clone(path));
                    drop(state);
                    self.shared.wake.notify_one();
                }
                None
            }
        }
    }

    /// A counter that changes whenever a file has been lexed, so a
    /// frontend knows to redraw.
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }

    /// Wait up to `timeout` for every queued file to be lexed. Returns
    /// whether the queue emptied.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let state = self.shared.state.lock().unwrap();
        let (state, _) = self
            .shared
            .idle
            .wait_timeout_while(state, timeout, |state| !state.is_idle())
            .unwrap();
        state.is_idle()
    }
}

impl Default for HighlightCache {
    fn default() -> HighlightCache {
        HighlightCache::new()
    }
}

impl Drop for HighlightCache {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().shutdown = true;
        self.shared.wake.notify_all();
    }
}

impl State {
    fn is_idle(&self) -> bool {
        self.queue.is_empty() && !self.busy
    }

    /// Note that a lexed file was just asked for.
    fn touch(&mut self, path: &Arc<Path>) {
        self.recent.retain(|(_, p)| p != path);
        self.recent.push((Instant::now(), Arc::clone(path)));
    }

    /// Forget the least recently asked for lexed files beyond the limit.
    fn evict(&mut self) {
        while self.recent.len() > MAX_FILES {
            let (_, path) = self.recent.remove(0);
            self.files.remove(&path);
        }
    }
}

fn run_worker(shared: Arc<Shared>) {
    loop {
        let path = {
            let mut state = shared.state.lock().unwrap();
            loop {
                if state.shutdown {
                    return;
                }
                if let Some(path) = state.queue.pop_front() {
                    state.busy = true;
                    break path;
                }
                state = shared.wake.wait(state).unwrap();
            }
        };
        let entry = match lex_file(&path, &shared.buffers) {
            Some(states) => Entry::Lexed(Arc::new(states)),
            None => Entry::Unsupported,
        };
        let mut state = shared.state.lock().unwrap();
        if let Entry::Lexed(_) = entry {
            state.touch(&path);
        }
        state.files.insert(path, entry);
        state.evict();
        state.busy = false;
        let idle = state.is_idle();
        drop(state);
        shared.generation.fetch_add(1, Ordering::Release);
        if idle {
            shared.idle.notify_all();
        }
    }
}

/// Lex every line of a file from the top, keeping the state each starts
/// in.
fn lex_file(path: &Path, buffers: &Buffers) -> Option<FileStates> {
    let language = Language::from_path(path)?;
    let lexer = language.file_lexer();
    let mut state = LexState::default();
    let mut states = Vec::new();
    let mut scratch = Vec::new();
    let mut lex = |line: &[u8]| {
        states.push(state);
        scratch.clear();
        state = lexer.lex_line(state, line, &mut scratch);
    };
    if let Some(snapshot) = buffers.get(path) {
        let mut lines = snapshot.lines_from(0);
        while let Some(line) = lines.next_line() {
            lex(line);
        }
    } else {
        let bytes = fs::read(path).ok()?;
        for (_, line) in lines(&bytes) {
            lex(line);
        }
    }
    Some(FileStates { language, states })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::TokenKind;

    const WAIT: Duration = Duration::from_secs(10);

    #[test]
    fn lexes_files_in_the_background_once() {
        let dir = tempfile::tempdir().unwrap();
        let rust = dir.path().join("a.rs");
        fs::write(&rust, "fn main() {} /* open\nstill */ let x = \"s\";\n").unwrap();
        let plain = dir.path().join("notes.unknownext");
        fs::write(&plain, "fn main() {}\n").unwrap();
        let rust: Arc<Path> = Arc::from(rust);
        let plain: Arc<Path> = Arc::from(plain);

        let cache = HighlightCache::new();
        let generation = cache.generation();
        assert!(cache.states(&rust).is_none(), "not lexed yet");
        assert!(cache.wait_idle(WAIT));
        assert!(cache.generation() > generation);
        let states = cache.states(&rust).expect("lexed");
        assert_eq!(states.language(), Language::Rust);
        assert_eq!(states.line_count(), 2);
        let tokens = states.tokens(0, b"fn main() {} /* open");
        assert_eq!(tokens[0].kind, TokenKind::Keyword);
        assert_eq!(tokens[0].range, 0..2);
        // The second line starts inside the comment: its state says so.
        let tokens = states.tokens(1, b"still */ let x = \"s\";");
        assert_eq!(tokens[0].kind, TokenKind::Comment);
        assert_eq!(tokens[0].range, 0..8);
        assert!(tokens.iter().any(|t| t.kind == TokenKind::String));
        // A prefix of a line lexes the same way; a line past the end has
        // no tokens.
        assert_eq!(states.tokens(1, b"still */ let").len(), 2);
        assert!(states.tokens(2, b"fn").is_empty());
        // The same states come back without another lexing.
        let generation = cache.generation();
        assert!(Arc::ptr_eq(&states, &cache.states(&rust).unwrap()));
        assert_eq!(cache.generation(), generation);

        // A file of no known language is never lexed.
        assert!(cache.states(&plain).is_none());
        assert!(cache.wait_idle(WAIT));
        assert!(cache.states(&plain).is_none());
        assert_eq!(cache.generation(), generation);

        // A file that can't be read is likewise given up on.
        let missing: Arc<Path> = Arc::from(dir.path().join("missing.rs"));
        assert!(cache.states(&missing).is_none());
        assert!(cache.wait_idle(WAIT));
        assert!(cache.states(&missing).is_none());
    }

    #[test]
    fn open_buffers_are_lexed_in_place_of_disk() {
        use crate::buffer::FileBuffer;
        let dir = tempfile::tempdir().unwrap();
        let rust = dir.path().join("a.rs");
        fs::write(&rust, "fn main() {}\n").unwrap();
        let mut buffers = Buffers::new();
        let buffer = FileBuffer::from_text("/* open\nstill */ fn\n");
        buffers.insert(rust.clone(), buffer.snapshot());
        let rust: Arc<Path> = Arc::from(rust);
        let cache = HighlightCache::with_buffers(Arc::new(buffers));
        assert!(cache.states(&rust).is_none());
        assert!(cache.wait_idle(WAIT));
        let states = cache.states(&rust).expect("lexed");
        // Three lines, counting the empty one after the trailing newline,
        // and the second starts inside the comment opened on the first.
        assert_eq!(states.line_count(), 3);
        let tokens = states.tokens(1, b"still */ fn");
        assert_eq!(tokens[0].kind, TokenKind::Comment);
        assert_eq!(tokens[0].range, 0..8);
        assert_eq!(tokens.last().unwrap().kind, TokenKind::Keyword);
    }

    #[test]
    fn old_files_are_evicted() {
        let dir = tempfile::tempdir().unwrap();
        let cache = HighlightCache::new();
        let paths: Vec<Arc<Path>> = (0..MAX_FILES + 2)
            .map(|i| {
                let path = dir.path().join(format!("f{i}.rs"));
                fs::write(&path, "fn f() {}\n").unwrap();
                Arc::from(path)
            })
            .collect();
        for path in &paths {
            cache.states(path);
        }
        assert!(cache.wait_idle(WAIT));
        let state = cache.shared.state.lock().unwrap();
        assert_eq!(state.recent.len(), MAX_FILES);
        assert!(!state.files.contains_key(&paths[0]));
        assert!(!state.files.contains_key(&paths[1]));
        assert!(state.files.contains_key(&paths[2]));
        drop(state);
        // Asking again lexes the evicted file afresh.
        assert!(cache.states(&paths[0]).is_none());
        assert!(cache.wait_idle(WAIT));
        assert!(cache.states(&paths[0]).is_some());
    }
}
