//! Code completion: running the completion model for the editor.
//!
//! The models are trained per language (see the `completion` directory
//! of the repository), and the user gives the checkpoint directory of
//! each in the settings. A [`Completer`] keeps one worker thread per
//! configured language, which loads its model on the first request and
//! then answers requests one at a time. Inference is slow next to
//! typing, so a request supersedes any earlier one that hasn't started,
//! and one that is running is cancelled as soon as a newer one arrives;
//! only the newest request's answer is ever reported.
//!
//! The editor asks for a completion with a [`CompletionRequest`] it
//! builds from the text around the cursor (see
//! [`Editor::take_completion_request`](crate::Editor::take_completion_request)),
//! and the frontend passes it to [`Completer::request`] along with a
//! number naming which editor it came from. The answer comes back as a
//! [`CompletionOutcome`] from [`Completer::take_outcomes`], which the
//! frontend hands to that editor as
//! [`Editor::offer_completion`](crate::Editor::offer_completion). A
//! frontend that wants to be woken when an answer is ready rather than
//! polling gives the completer a waker.
//!
//! The model's context is limited, and prefilling it is the expensive
//! part of a completion, so the worker keeps a session whose cache is
//! reused across requests: only the tokens that differ from the last
//! prompt are run. The prompt is laid out so the text before the cursor
//! comes last, where typing only appends to it. The window of text
//! before the cursor that goes into the prompt is anchored at a line
//! break and the anchor is kept from one request to the next, moving
//! only when the text has outgrown the budget (and then far enough ahead
//! to last a while), so the prompt doesn't change at its start with
//! every line typed.

use crate::indent::Indentation;
use crate::syntax::Language;
use anyhow::{Context, Result, anyhow};
use infer::{CompletionOptions, Model, Session, StopReason, TokenSet};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use tokenizer::Tokenizer;
use tokenizer::pretok::{self, Indent};

/// What the editor sends to have a completion made at the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionRequest {
    /// The editor's number for this request; the outcome carries it
    /// back so an answer to an earlier request can be told apart.
    pub serial: u64,
    pub language: Language,
    /// The text before the cursor, from a line break some way back (or
    /// the start of the buffer) to the cursor, with `\n` line breaks.
    /// Includes indentation the cursor is shown at but that isn't in the
    /// buffer yet.
    pub prefix: String,
    /// The buffer offset the prefix starts at, so the worker can keep
    /// its context window anchored at the same place across requests.
    pub prefix_start: usize,
    /// The text after the cursor: the rest of its line and some lines
    /// after, with `\n` line breaks.
    pub suffix: String,
    /// The buffer's indentation style, which the model's output is
    /// rendered in.
    pub indentation: Indentation,
}

/// What a worker reports back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionOutcome {
    /// A completion for the request `serial` from the editor `owner`. The
    /// text is what the model proposes inserting at the cursor, with
    /// `\n` line breaks; it may be empty when the model had nothing to
    /// add.
    Completed {
        owner: u64,
        serial: u64,
        text: String,
    },
    /// A language's model could not be loaded. Reported once per
    /// configured path; requests for the language are then dropped until
    /// its path is changed.
    Failed { language: Language, error: String },
}

/// A job for a worker.
struct Job {
    owner: u64,
    request: CompletionRequest,
}

struct Worker {
    path: PathBuf,
    jobs: Sender<Job>,
}

type Waker = Arc<dyn Fn() + Send + Sync>;

/// The completion models, one worker per configured language. See the
/// [module documentation](self).
pub struct Completer {
    workers: HashMap<Language, Worker>,
    outcomes: Receiver<CompletionOutcome>,
    outcome_tx: Sender<CompletionOutcome>,
    waker: Option<Waker>,
}

impl Default for Completer {
    fn default() -> Self {
        Self::new()
    }
}

impl Completer {
    /// A completer with no models configured.
    pub fn new() -> Completer {
        let (outcome_tx, outcomes) = mpsc::channel();
        Completer {
            workers: HashMap::new(),
            outcomes,
            outcome_tx,
            waker: None,
        }
    }

    /// Something to call, from a worker thread, whenever an outcome is
    /// ready to be taken, so a frontend's event loop can wake up for it.
    pub fn set_waker(&mut self, waker: impl Fn() + Send + Sync + 'static) {
        self.waker = Some(Arc::new(waker));
    }

    /// Use the model in the checkpoint directory `path` for `language`,
    /// or none. A path as the user typed it, with `~` for the home
    /// directory. Changing a language's path replaces its worker;
    /// setting the same path again does nothing, so this can be called
    /// whenever the settings change.
    pub fn set_model(&mut self, language: Language, path: Option<&str>) {
        let path = path.map(str::trim).filter(|p| !p.is_empty());
        let Some(path) = path else {
            self.workers.remove(&language);
            return;
        };
        let path = expand_home(path);
        if self
            .workers
            .get(&language)
            .is_some_and(|worker| worker.path == path)
        {
            return;
        }
        let checkpoint = path.clone();
        self.spawn(language, path, move || load_checkpoint(&checkpoint));
    }

    /// Whether completions can be asked for in `language`.
    pub fn has_model(&self, language: Language) -> bool {
        self.workers.contains_key(&language)
    }

    /// Start a worker for `language` whose model comes from `load`, run
    /// on the worker's thread at its first request. Any worker the
    /// language had is dropped; it finishes what it is doing and exits.
    fn spawn(
        &mut self,
        language: Language,
        path: PathBuf,
        load: impl FnOnce() -> Result<(Model, Tokenizer)> + Send + 'static,
    ) {
        let (jobs_tx, jobs) = mpsc::channel();
        let outcomes = self.outcome_tx.clone();
        let waker = self.waker.clone();
        let spawned = thread::Builder::new()
            .name(format!("completion-{}", language.name()))
            .spawn(move || run_worker(language, load, jobs, outcomes, waker));
        if spawned.is_err() {
            return;
        }
        self.workers.insert(
            language,
            Worker {
                path,
                jobs: jobs_tx,
            },
        );
    }

    /// Ask for a completion on behalf of the editor `owner`. Does nothing
    /// when the request's language has no model. The answer, if the
    /// request isn't superseded first, arrives through
    /// [`take_outcomes`](Self::take_outcomes).
    pub fn request(&mut self, owner: u64, request: CompletionRequest) {
        let language = request.language;
        let Some(worker) = self.workers.get(&language) else {
            return;
        };
        if worker.jobs.send(Job { owner, request }).is_err() {
            // The worker's thread is gone, which only happens if it
            // panicked; don't keep sending into the void.
            self.workers.remove(&language);
        }
    }

    /// Report an outcome as a worker would, for a frontend to test what
    /// it does with one without a model.
    pub fn inject(&self, outcome: CompletionOutcome) {
        let _ = self.outcome_tx.send(outcome);
        if let Some(waker) = &self.waker {
            waker();
        }
    }

    /// Every outcome reported since the last call, oldest first.
    pub fn take_outcomes(&mut self) -> Vec<CompletionOutcome> {
        let mut outcomes = Vec::new();
        while let Ok(outcome) = self.outcomes.try_recv() {
            outcomes.push(outcome);
        }
        outcomes
    }
}

/// A path as the user typed it, with a leading `~` standing for the
/// home directory.
pub fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        return std::env::home_dir().unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

/// Load a checkpoint directory: the model, and the tokenizer bundled
/// with it, which must be the one it was trained with.
fn load_checkpoint(dir: &Path) -> Result<(Model, Tokenizer)> {
    if !dir.is_dir() {
        return Err(anyhow!("{} is not a directory", dir.display()));
    }
    let tokenizer_path = dir.join("tokenizer.json");
    let tokenizer = Tokenizer::load(&tokenizer_path)
        .with_context(|| format!("loading the tokenizer in {}", dir.display()))?;
    if let Ok(text) = std::fs::read_to_string(dir.join("state.json"))
        && let Ok(state) = serde_json::from_str::<serde_json::Value>(&text)
        && let Some(expected) = state.get("tokenizer_fingerprint").and_then(|v| v.as_str())
        && expected != tokenizer.fingerprint()
    {
        return Err(anyhow!(
            "{} is not the tokenizer the model was trained with",
            tokenizer_path.display()
        ));
    }
    let model =
        Model::load(dir).with_context(|| format!("loading the model in {}", dir.display()))?;
    Ok((model, tokenizer))
}

/// A worker's thread: load the model at the first request, then answer
/// requests until the completer drops the job channel.
fn run_worker(
    language: Language,
    load: impl FnOnce() -> Result<(Model, Tokenizer)>,
    jobs: Receiver<Job>,
    outcomes: Sender<CompletionOutcome>,
    waker: Option<Waker>,
) {
    let wake = || {
        if let Some(waker) = &waker {
            waker();
        }
    };
    let Some(mut job) = latest_job(&jobs, None) else {
        return;
    };
    let (model, tokenizer) = match load() {
        Ok(loaded) => loaded,
        Err(err) => {
            let _ = outcomes.send(CompletionOutcome::Failed {
                language,
                error: format!("{err:#}"),
            });
            wake();
            // Drain requests until the completer gives up on this
            // worker, so the channel doesn't fill.
            while jobs.recv().is_ok() {}
            return;
        }
    };
    let mut engine = Engine::new(&model, &tokenizer);
    loop {
        // A newer request arriving while this one runs cancels it and
        // is kept to run next.
        let mut next: Option<Job> = None;
        let mut cancel = || match jobs.try_recv() {
            Ok(newer) => {
                next = Some(newer);
                true
            }
            Err(_) => false,
        };
        let text = engine.complete(&job.request, &mut cancel);
        if let Some(text) = text {
            let _ = outcomes.send(CompletionOutcome::Completed {
                owner: job.owner,
                serial: job.request.serial,
                text,
            });
            wake();
        }
        match latest_job(&jobs, next) {
            Some(newer) => job = newer,
            None => return,
        }
    }
}

/// The newest job waiting, starting from `first` if there is one, else
/// blocking for one. `None` once the completer has hung up.
fn latest_job(jobs: &Receiver<Job>, first: Option<Job>) -> Option<Job> {
    let mut job = match first {
        Some(job) => job,
        None => jobs.recv().ok()?,
    };
    loop {
        match jobs.try_recv() {
            Ok(newer) => job = newer,
            Err(TryRecvError::Empty) => return Some(job),
            Err(TryRecvError::Disconnected) => return None,
        }
    }
}

/// Room left in the model's context for what it generates.
const GENERATION_BUDGET: usize = 256;
/// The most tokens of the text after the cursor that go in the prompt.
const SUFFIX_BUDGET: usize = 512;
/// The special tokens the prompt is framed with.
const FRAME_TOKENS: usize = 3;

/// A loaded model with the session that keeps its cache between
/// requests, and the anchor of the context window.
struct Engine<'a> {
    tokenizer: &'a Tokenizer,
    session: Session<'a>,
    max_seq_len: usize,
    /// The buffer offset of the line break the context window starts at
    /// (or of the start of the buffer), kept across requests while it
    /// still fits; see the [module documentation](self).
    anchor: Option<usize>,
}

impl<'a> Engine<'a> {
    fn new(model: &'a Model, tokenizer: &'a Tokenizer) -> Engine<'a> {
        Engine {
            tokenizer,
            session: Session::new(model),
            max_seq_len: model.config.max_seq_len,
            anchor: None,
        }
    }

    /// Run one request. `None` if it was cancelled.
    fn complete(
        &mut self,
        request: &CompletionRequest,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Option<String> {
        let tok = self.tokenizer;
        let (indent, indent_str) = match request.indentation {
            Indentation::Spaces(n) => (Indent::Spaces(n.max(1)), " ".repeat(n.max(1))),
            Indentation::Tabs => (Indent::Tabs, "\t".to_owned()),
        };
        // The context ends at the last pre-token boundary; what the user
        // typed past it is the partial token the completion must cover.
        let boundary = pretok::last_piece_start(&request.prefix, indent);
        let (context, partial) = request.prefix.split_at(boundary);

        let mut encoder = tok.encoder();
        let mut suffix_ids = Vec::new();
        encoder.encode_with(&request.suffix, indent, &mut suffix_ids);
        suffix_ids.truncate(SUFFIX_BUDGET);
        let token_set = TokenSet {
            eom: tok.special("<eom>"),
            eos: tok.special("<eos>"),
            newline: Tokenizer::NEWLINE_BASE..Tokenizer::BYTE_BASE,
            render: tok.render_table(&indent_str),
        };
        // The suffix's first content line, which a completion must not
        // repeat.
        let suffix_line: Vec<u32> = suffix_ids
            .iter()
            .copied()
            .skip_while(|t| token_set.newline.contains(t))
            .take_while(|t| !token_set.newline.contains(t))
            .collect();

        let prefix_budget = self
            .max_seq_len
            .saturating_sub(GENERATION_BUDGET + FRAME_TOKENS + suffix_ids.len())
            .max(1);
        let window = self.window(context, request.prefix_start, prefix_budget, &mut encoder);

        let mut ids = vec![tok.special("<fim_prefix>"), tok.special("<fim_suffix>")];
        ids.extend_from_slice(&suffix_ids);
        ids.push(tok.special("<fim_middle>"));
        encoder.encode_with(&context[window..], indent, &mut ids);
        if cancel() {
            return None;
        }
        self.session.sync(&ids);
        let opts = CompletionOptions {
            max_tokens: GENERATION_BUDGET,
            ..CompletionOptions::default()
        };
        let completion =
            self.session
                .complete_with(&token_set, &opts, &suffix_line, partial.as_bytes(), cancel);
        if completion.reason == StopReason::Cancelled {
            return None;
        }
        // The generated text begins with the partial the user typed.
        let text = tok.decode(&completion.tokens(), &indent_str);
        Some(text.get(partial.len()..).unwrap_or("").to_owned())
    }

    /// Where in `context` the window that goes into the prompt starts:
    /// at the anchor kept from the last request while it is still in the
    /// text and fits the budget, else at a new anchor chosen to leave
    /// room to type before the next move. Returns the offset within
    /// `context`; `context` starts at buffer offset `start`.
    fn window(
        &mut self,
        context: &str,
        start: usize,
        budget: usize,
        encoder: &mut tokenizer::Encoder,
    ) -> usize {
        let count = |encoder: &mut tokenizer::Encoder, from: usize| {
            let mut ids = Vec::new();
            encoder.encode_with(&context[from..], Indent::Spaces(4), &mut ids);
            ids.len()
        };
        // Places the window may start: the start of the text, or a line
        // break, so the first line's indentation is a line-break token
        // as in training.
        let candidates: Vec<usize> = std::iter::once(0)
            .chain(
                context
                    .bytes()
                    .enumerate()
                    .filter(|&(_, b)| b == b'\n')
                    .map(|(i, _)| i)
                    .filter(|&i| i > 0),
            )
            .collect();
        if let Some(anchor) = self.anchor
            && anchor >= start
            && anchor - start <= context.len()
            && candidates.binary_search(&(anchor - start)).is_ok()
        {
            let from = anchor - start;
            let tokens = count(encoder, from);
            // Kept while it fits, unless there is much more text before
            // it than it lets the model see.
            if tokens <= budget && (tokens * 2 >= budget || from == 0) {
                return from;
            }
        }
        // The earliest candidate that fits three quarters of the budget,
        // by binary search: the count only falls as the start moves on.
        let target = (budget * 3 / 4).max(1);
        let (mut low, mut high) = (0, candidates.len() - 1);
        if count(encoder, candidates[high]) > target {
            // Even the last line alone is over budget; the model gets a
            // window that the session will truncate.
            self.anchor = Some(start + candidates[high]);
            return candidates[high];
        }
        while low < high {
            let mid = (low + high) / 2;
            if count(encoder, candidates[mid]) <= target {
                high = mid;
            } else {
                low = mid + 1;
            }
        }
        self.anchor = Some(start + candidates[low]);
        candidates[low]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use infer::Config;
    use std::time::{Duration, Instant};

    /// A tiny random model over the byte-level tokenizer, for the
    /// plumbing; what it says is noise.
    fn tiny_model() -> (Model, Tokenizer) {
        let tokenizer = Tokenizer::from_merges(Vec::new());
        let config = Config {
            vocab_size: tokenizer.vocab_size(),
            d_model: 16,
            n_layers: 1,
            n_heads: 2,
            d_ff: 32,
            max_seq_len: 512,
            rope_theta: 10000.0,
        };
        (Model::random(config, 7), tokenizer)
    }

    fn wait_for(
        completer: &mut Completer,
        mut done: impl FnMut(&CompletionOutcome) -> bool,
    ) -> Vec<CompletionOutcome> {
        let started = Instant::now();
        let mut all = Vec::new();
        while started.elapsed() < Duration::from_secs(30) {
            let outcomes = completer.take_outcomes();
            let finished = outcomes.iter().any(&mut done);
            all.extend(outcomes);
            if finished {
                return all;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("no outcome in time: {all:?}");
    }

    fn request(serial: u64, prefix: &str, suffix: &str) -> CompletionRequest {
        CompletionRequest {
            serial,
            language: Language::Rust,
            prefix: prefix.to_owned(),
            prefix_start: 0,
            suffix: suffix.to_owned(),
            indentation: Indentation::Spaces(4),
        }
    }

    #[test]
    fn answers_the_newest_request_and_names_its_owner() {
        let mut completer = Completer::new();
        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&woken);
        completer.set_waker(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        assert!(!completer.has_model(Language::Rust));
        completer.spawn(Language::Rust, PathBuf::from("tiny"), || Ok(tiny_model()));
        assert!(completer.has_model(Language::Rust));
        assert!(!completer.has_model(Language::C));

        // A request for a language without a model goes nowhere.
        let mut other = request(1, "int main() {\n", "}\n");
        other.language = Language::C;
        completer.request(9, other);

        completer.request(3, request(1, "fn main() {\n    let x = ", "\n}\n"));
        completer.request(3, request(2, "fn main() {\n    let x = 1", "\n}\n"));
        let outcomes = wait_for(&mut completer, |o| {
            matches!(o, CompletionOutcome::Completed { serial: 2, .. })
        });
        for outcome in &outcomes {
            match outcome {
                CompletionOutcome::Completed { owner, serial, .. } => {
                    assert_eq!(*owner, 3);
                    assert!(*serial <= 2);
                }
                other => panic!("{other:?}"),
            }
        }
        assert!(woken.load(std::sync::atomic::Ordering::SeqCst) >= 1);
    }

    #[test]
    fn a_model_that_fails_to_load_is_reported_once() {
        let mut completer = Completer::new();
        completer.spawn(Language::Rust, PathBuf::from("bad"), || {
            Err(anyhow!("no such checkpoint"))
        });
        completer.request(1, request(1, "fn a() {\n", ""));
        completer.request(1, request(2, "fn ab() {\n", ""));
        let outcomes = wait_for(&mut completer, |o| {
            matches!(o, CompletionOutcome::Failed { .. })
        });
        assert_eq!(
            outcomes,
            vec![CompletionOutcome::Failed {
                language: Language::Rust,
                error: "no such checkpoint".to_owned()
            }]
        );
        // Later requests are dropped quietly.
        completer.request(1, request(3, "fn abc() {\n", ""));
        thread::sleep(Duration::from_millis(50));
        assert!(completer.take_outcomes().is_empty());
    }

    #[test]
    fn a_missing_checkpoint_directory_fails_to_load() {
        let mut completer = Completer::new();
        completer.set_model(Language::Rust, Some("/nonexistent/checkpoint"));
        assert!(completer.has_model(Language::Rust));
        // The same path again keeps the worker; a blank drops it.
        completer.set_model(Language::Rust, Some(" /nonexistent/checkpoint "));
        completer.request(1, request(1, "fn a() {\n", ""));
        let outcomes = wait_for(&mut completer, |o| {
            matches!(o, CompletionOutcome::Failed { .. })
        });
        match &outcomes[0] {
            CompletionOutcome::Failed { language, error } => {
                assert_eq!(*language, Language::Rust);
                assert!(error.contains("not a directory"), "{error}");
            }
            other => panic!("{other:?}"),
        }
        completer.set_model(Language::Rust, Some(""));
        assert!(!completer.has_model(Language::Rust));
    }

    /// Runs a real checkpoint, named by `NINJAEDIT_COMPLETION_MODEL`,
    /// through the worker and prints what it says; for trying a model
    /// by hand.
    #[test]
    #[ignore = "needs a checkpoint in NINJAEDIT_COMPLETION_MODEL"]
    fn completes_with_a_real_checkpoint() {
        let path = std::env::var("NINJAEDIT_COMPLETION_MODEL").expect("checkpoint path");
        let mut completer = Completer::new();
        completer.set_model(Language::Rust, Some(&path));
        let prefix = "use std::collections::HashMap;\n\n/// Counts how often each word occurs in `text`.\nfn word_counts(text: &str) -> HashMap<&str, usize> {\n    let mut counts = HashMap::new();\n    for word in text.split_whitespace() {\n        *counts.en";
        let suffix = "\n    }\n    counts\n}\n";
        let started = Instant::now();
        completer.request(1, request(1, prefix, suffix));
        let outcomes = wait_for(&mut completer, |o| {
            matches!(
                o,
                CompletionOutcome::Completed { .. } | CompletionOutcome::Failed { .. }
            )
        });
        eprintln!("first answer after {:?}", started.elapsed());
        match &outcomes[0] {
            CompletionOutcome::Completed { text, .. } => {
                eprintln!("completion: {text:?}");
                assert!(!text.is_empty());
            }
            CompletionOutcome::Failed { error, .. } => panic!("{error}"),
        }
        // A second request continuing the text reuses the session.
        let prefix2 = format!(
            "{prefix}try(word).or_insert(0) += 1;\n    }}\n    counts\n}}\n\nfn main() {{\n    let text = "
        );
        let started = Instant::now();
        completer.request(1, request(2, &prefix2, ""));
        let outcomes = wait_for(&mut completer, |o| {
            matches!(o, CompletionOutcome::Completed { serial: 2, .. })
        });
        eprintln!("second answer after {:?}", started.elapsed());
        if let Some(CompletionOutcome::Completed { text, .. }) = outcomes.last() {
            eprintln!("completion: {text:?}");
        }
    }

    #[test]
    fn expands_a_leading_tilde() {
        let home = std::env::home_dir().unwrap();
        assert_eq!(expand_home("~/models/rust"), home.join("models/rust"));
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(expand_home("~user/x"), PathBuf::from("~user/x"));
    }

    #[test]
    fn the_window_keeps_its_anchor_until_the_text_outgrows_the_budget() {
        let (model, tokenizer) = tiny_model();
        let mut engine = Engine::new(&model, &tokenizer);
        let mut encoder = tokenizer.encoder();
        // Byte-level tokens: each line of "abcd\n" is five tokens (four
        // bytes and a line break).
        let lines = |n: usize| "abcd\n".repeat(n);
        let text = lines(20);
        // A budget of 60 tokens: the first fit is three quarters of
        // that, 45 tokens, so eight lines after the opening line break.
        let from = engine.window(&text, 0, 60, &mut encoder);
        assert_eq!(engine.anchor, Some(from));
        assert_eq!(text[from..].matches('\n').count(), 1 + 8, "{from}");
        // Typing three more lines still fits the budget, so the anchor
        // stays where it was.
        let text = lines(23);
        assert_eq!(engine.window(&text, 0, 60, &mut encoder), from);
        // Three more again outgrows it (15 lines, 75 tokens), and the
        // anchor moves on to leave room again.
        let text = lines(26);
        let moved = engine.window(&text, 0, 60, &mut encoder);
        assert!(moved > from);
        assert_eq!(text[moved..].matches('\n').count(), 1 + 8);
        // Text that starts later in the buffer (the window sent by the
        // editor moved) keeps the anchor by its buffer offset.
        let start = 5 * 3;
        assert_eq!(
            engine.window(&text[start..], start, 60, &mut encoder),
            moved - start
        );
        // A cursor moved far back, so that the anchor is past the text,
        // gets a fresh window from the start.
        let short = lines(4);
        assert_eq!(engine.window(&short, 0, 60, &mut encoder), 0);
        assert_eq!(engine.anchor, Some(0));
    }
}
