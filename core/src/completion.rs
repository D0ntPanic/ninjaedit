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
//! polling gives the completer a waker, and one that has nothing else to
//! do, like the completion evaluation, can block on
//! [`Completer::wait_for_outcomes`].
//!
//! Which model a language uses comes from the settings, given to
//! [`Completer::apply_settings`]; a model set with
//! [`Completer::override_model`] takes precedence over them, for tools
//! that run a model of their choosing through the editor.
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
//!
//! The model is always asked for its whole completion, but only as much
//! of it as the model is sure enough of is offered: the lines from the
//! start up to the first it isn't, or when that is the first, as much of
//! it as it is sure of, by the [`ConfidenceThresholds`] the settings
//! give. The outcome carries both, and how sure the model was of each
//! line and token, so that a tool measuring the model can see all it
//! said and how much of it the user would see at any thresholds.

use crate::indent::Indentation;
use crate::settings::Settings;
use crate::syntax::Language;
use anyhow::{Context, Result, anyhow};
use infer::{CompletionOptions, Model, Session, StopReason, TokenSet};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;
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
    /// The file the buffer was loaded from, if any. The model was trained
    /// with each file opening with the name of its crate and its path in
    /// the crate, so the prompt opens with them too when the file is in
    /// a Cargo package.
    pub path: Option<PathBuf>,
}

/// What a worker reports back.
#[derive(Clone, Debug, PartialEq)]
pub enum CompletionOutcome {
    /// A completion for the request `serial` from the editor `owner`. The
    /// text is what the model proposes inserting at the cursor, with
    /// `\n` line breaks; it may be empty when the model had nothing to
    /// add. `offered` is how many bytes of it, from the start, the model
    /// is sure enough of to offer (see [`ConfidenceThresholds`]); the
    /// rest is what it went on to say, which only tools measuring the
    /// model look at. `lines` is how sure it was of each line of the
    /// text, for such a tool to see what other thresholds would offer.
    Completed {
        owner: u64,
        serial: u64,
        text: String,
        offered: usize,
        lines: Vec<LineConfidence>,
    },
    /// A language's model could not be loaded. Reported once per
    /// configured path; requests for the language are then dropped until
    /// its path is changed.
    Failed { language: Language, error: String },
}

/// How sure the model was of one line of a completion.
#[derive(Clone, Debug, PartialEq)]
pub struct LineConfidence {
    /// Where the line ends in the completion's text, in bytes, before its
    /// line break. Zero for a line that ends in text the user had
    /// already typed.
    pub end: usize,
    /// The geometric mean of the probabilities of its tokens, 0 to 1.
    pub confidence: f32,
    /// The probability of its least likely token, 0 to 1.
    pub min_prob: f32,
    /// Whether it has only whitespace, which the model can't be said to
    /// be sure of or not (its confidence is zero).
    pub blank: bool,
    /// How sure it was of each of the line's tokens, in order.
    pub tokens: Vec<TokenConfidence>,
}

/// How sure the model was of one token of a completion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TokenConfidence {
    /// Where the token ends in the completion's text, in bytes. Zero for
    /// a token that ends in text the user had already typed.
    pub end: usize,
    /// Its probability, 0 to 1.
    pub prob: f32,
    /// Whether the text may be cut after it: it doesn't end partway
    /// through a word that the text goes on with.
    pub word_end: bool,
}

/// How sure the model must be of a line of a completion for the line to
/// be offered, from the settings: lower for a more eager completer,
/// higher for a more cautious one. A completion is offered from its
/// start up to the first line that falls short of either threshold.
/// Blank lines have no tokens to be sure of, so they don't count either
/// way: they are offered only on the way to a line that is.
///
/// When the first line with text falls short, the start of it may be
/// offered instead, as when the model is sure which function is called
/// but not what with: the tokens before the first that falls short of
/// the token threshold, as far as they meet the line threshold together
/// and end a word.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConfidenceThresholds {
    /// The least geometric mean of the probabilities of a line's tokens,
    /// 0 to 1.
    pub line: f64,
    /// The least probability of any one token of a line, 0 to 1.
    pub token: f64,
}

impl ConfidenceThresholds {
    /// A middle course, from the completion evaluation's `--sweep` of
    /// the first Rust model: it offers more than half the completions, a
    /// third of them right in full, where offering everything is right in
    /// full one time in fifteen or so.
    pub const DEFAULT: ConfidenceThresholds = ConfidenceThresholds {
        line: 0.7,
        token: 0.4,
    };

    /// How many bytes of a completion whose lines are `lines` to offer:
    /// up to the end of the last line before the first with text that
    /// isn't confident enough, so blank lines are offered only before a
    /// line that is, or when that is the first, the start of it the
    /// model is sure of.
    pub fn offered(self, lines: &[LineConfidence]) -> usize {
        let mut offered = 0;
        for (i, line) in lines.iter().filter(|line| !line.blank).enumerate() {
            if f64::from(line.confidence) < self.line || f64::from(line.min_prob) < self.token {
                if i == 0 {
                    offered = self.confident_start(line);
                }
                break;
            }
            offered = line.end;
        }
        offered
    }

    /// Where the start of `line` that the model is sure enough of ends:
    /// at the last word end among the tokens before the first that falls
    /// short of the token threshold where the tokens up to it meet the
    /// line threshold together. Zero if there is none.
    fn confident_start(self, line: &LineConfidence) -> usize {
        let mut end = 0;
        let mut logprob_sum = 0.0;
        for (i, token) in line.tokens.iter().enumerate() {
            let prob = f64::from(token.prob);
            if prob < self.token {
                break;
            }
            logprob_sum += prob.max(1e-30).ln();
            let confidence = (logprob_sum / (i + 1) as f64).exp();
            if confidence >= self.line && token.word_end {
                end = token.end;
            }
        }
        end
    }
}

impl Default for ConfidenceThresholds {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A job for a worker.
struct Job {
    owner: u64,
    request: CompletionRequest,
    thresholds: ConfidenceThresholds,
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
    /// Model paths set with [`override_model`](Self::override_model),
    /// which the settings don't replace.
    overrides: HashMap<Language, String>,
    /// How much of each completion to offer, from the settings.
    thresholds: ConfidenceThresholds,
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
            overrides: HashMap::new(),
            thresholds: ConfidenceThresholds::default(),
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

    /// Use the model the settings name for each language, or none where
    /// they name none, except for languages whose model has been
    /// [overridden](Self::override_model), and the confidence thresholds
    /// they give for the requests from now on. Models that haven't
    /// changed are kept, so this can be called whenever the settings
    /// change.
    pub fn apply_settings(&mut self, settings: &Settings) {
        self.thresholds = settings.completion_thresholds();
        for language in Language::ALL {
            if !self.overrides.contains_key(&language) {
                self.set_model(language, settings.completion_model(language));
            }
        }
    }

    /// Use the model in the checkpoint directory `path` for `language`
    /// whatever the settings say: [`apply_settings`](Self::apply_settings)
    /// leaves it in place. For tools that run a model of their choosing,
    /// such as the completion evaluation.
    pub fn override_model(&mut self, language: Language, path: &str) {
        self.overrides.insert(language, path.to_owned());
        self.set_model(language, Some(path));
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
        let job = Job {
            owner,
            request,
            thresholds: self.thresholds,
        };
        if worker.jobs.send(job).is_err() {
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
        self.drain_outcomes(&mut outcomes);
        outcomes
    }

    /// Like [`take_outcomes`](Self::take_outcomes), but blocks until
    /// there is at least one outcome or `timeout` has passed; empty only
    /// on a timeout. For callers with nothing to do but wait.
    pub fn wait_for_outcomes(&mut self, timeout: Duration) -> Vec<CompletionOutcome> {
        let mut outcomes = Vec::new();
        if let Ok(outcome) = self.outcomes.recv_timeout(timeout) {
            outcomes.push(outcome);
            self.drain_outcomes(&mut outcomes);
        }
        outcomes
    }

    fn drain_outcomes(&mut self, outcomes: &mut Vec<CompletionOutcome>) {
        while let Ok(outcome) = self.outcomes.try_recv() {
            outcomes.push(outcome);
        }
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
        let answer = engine.complete(&job.request, &mut cancel);
        if let Some((text, lines)) = answer {
            let _ = outcomes.send(CompletionOutcome::Completed {
                owner: job.owner,
                serial: job.request.serial,
                text,
                offered: job.thresholds.offered(&lines),
                lines,
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
/// The special tokens the prompt is framed with, besides the header.
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
    /// The header tokens for each file asked about, empty for one in no
    /// Cargo package; see [`crate_of`].
    headers: HashMap<PathBuf, Vec<u32>>,
}

impl<'a> Engine<'a> {
    fn new(model: &'a Model, tokenizer: &'a Tokenizer) -> Engine<'a> {
        Engine {
            tokenizer,
            session: Session::new(model),
            max_seq_len: model.config.max_seq_len,
            anchor: None,
            headers: HashMap::new(),
        }
    }

    /// The tokens naming the crate and file the request is in, which the
    /// prompt opens with as training documents do. Read from the
    /// package's manifest once per file.
    fn header(&mut self, request: &CompletionRequest) -> Vec<u32> {
        let Some(path) = &request.path else {
            return Vec::new();
        };
        if request.language != Language::Rust {
            return Vec::new();
        }
        let tok = self.tokenizer;
        self.headers
            .entry(path.clone())
            .or_insert_with(|| {
                let mut ids = Vec::new();
                if let Some((name, file)) = crate_of(path) {
                    tok.encoder().encode_header(&name, &file, &mut ids);
                }
                ids
            })
            .clone()
    }

    /// Run one request: the text of the completion and how sure the
    /// model was of each of its lines. `None` if it was cancelled.
    fn complete(
        &mut self,
        request: &CompletionRequest,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Option<(String, Vec<LineConfidence>)> {
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

        let mut ids = self.header(request);
        let prefix_budget = self
            .max_seq_len
            .saturating_sub(GENERATION_BUDGET + FRAME_TOKENS + ids.len() + suffix_ids.len())
            .max(1);
        let window = self.window(context, request.prefix_start, prefix_budget, &mut encoder);

        ids.extend([tok.special("<fim_prefix>"), tok.special("<fim_suffix>")]);
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
        let text = text.get(partial.len()..).unwrap_or("").to_owned();
        // Each token's end, and each line's, is where the text up to it
        // ends.
        let mut ids = Vec::new();
        let end_of = |ids: &[u32]| {
            let mut end = tok
                .decode(ids, &indent_str)
                .len()
                .saturating_sub(partial.len())
                .min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            end
        };
        let lines = completion
            .lines
            .iter()
            .map(|line| {
                let tokens = line
                    .tokens
                    .iter()
                    .zip(&line.probs)
                    .map(|(&id, &prob)| {
                        ids.push(id);
                        let end = end_of(&ids);
                        TokenConfidence {
                            end,
                            prob,
                            word_end: !splits_word(&text, end),
                        }
                    })
                    .collect();
                let end = end_of(&ids);
                ids.extend(line.newline);
                LineConfidence {
                    end,
                    confidence: line.confidence,
                    min_prob: line.min_prob,
                    blank: line.tokens.iter().all(|&t| {
                        token_set.render[t as usize]
                            .iter()
                            .all(u8::is_ascii_whitespace)
                    }),
                    tokens,
                }
            })
            .collect();
        Some((text, lines))
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

/// The crate a file is in and its path within it, as the training data
/// names them: the package name in the nearest `Cargo.toml` above the
/// file, and the path from that manifest's directory with `/` between
/// its parts. `None` when the nearest manifest has no package (a virtual
/// workspace) or there is none.
fn crate_of(path: &Path) -> Option<(String, String)> {
    let path = std::path::absolute(path).ok()?;
    let (dir, manifest) = path.ancestors().skip(1).find_map(|dir| {
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
        Some((dir, manifest))
    })?;
    let manifest: toml::Table = toml::from_str(&manifest).ok()?;
    let name = manifest.get("package")?.get("name")?.as_str()?.to_owned();
    let parts: Vec<_> = path
        .strip_prefix(dir)
        .ok()?
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect();
    Some((name, parts.join("/")))
}

/// Whether cutting `text` at `at` would split a word: the characters on
/// either side are both of an identifier or a number.
fn splits_word(text: &str, at: usize) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    text[..at].chars().next_back().is_some_and(word) && text[at..].chars().next().is_some_and(word)
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
            path: None,
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
            CompletionOutcome::Completed { text, offered, .. } => {
                eprintln!("completion: {text:?}, offering {:?}", &text[..*offered]);
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
    fn an_overridden_model_outlasts_the_settings() {
        use crate::settings::SettingKey;
        let mut settings = Settings::default();
        let mut completer = Completer::new();
        completer.apply_settings(&settings);
        assert!(!completer.has_model(Language::Rust));
        settings
            .set_text(SettingKey::RustCompletionModel, "/settings/model")
            .unwrap();
        completer.apply_settings(&settings);
        assert_eq!(
            completer.workers[&Language::Rust].path,
            PathBuf::from("/settings/model")
        );

        completer.override_model(Language::Rust, "/override/model");
        assert_eq!(
            completer.workers[&Language::Rust].path,
            PathBuf::from("/override/model")
        );
        // Neither the settings naming another model nor naming none
        // replaces the override.
        completer.apply_settings(&settings);
        settings.reset(SettingKey::RustCompletionModel);
        completer.apply_settings(&settings);
        assert_eq!(
            completer.workers[&Language::Rust].path,
            PathBuf::from("/override/model")
        );
        // A language the settings have no model for can be given one.
        completer.override_model(Language::C, "/override/c");
        completer.apply_settings(&settings);
        assert!(completer.has_model(Language::C));
    }

    #[test]
    fn waiting_for_outcomes_blocks_until_one_arrives() {
        let mut completer = Completer::new();
        assert!(
            completer
                .wait_for_outcomes(Duration::from_millis(10))
                .is_empty()
        );
        completer.spawn(Language::Rust, PathBuf::from("tiny"), || Ok(tiny_model()));
        completer.request(1, request(1, "fn main() {\n    let x = ", "\n}\n"));
        let outcomes = completer.wait_for_outcomes(Duration::from_secs(30));
        assert!(
            matches!(
                outcomes.as_slice(),
                [CompletionOutcome::Completed { serial: 1, .. }]
            ),
            "{outcomes:?}"
        );
    }

    #[test]
    fn lines_are_offered_up_to_the_first_the_model_is_unsure_of() {
        // Each line two bytes long and followed by a line break, so the
        // n-th (from 1) ends at 3n - 1.
        let lines = |lines: &[(f32, f32)]| -> Vec<LineConfidence> {
            lines
                .iter()
                .enumerate()
                .map(|(i, &(confidence, min_prob))| LineConfidence {
                    end: 3 * i + 2,
                    confidence,
                    min_prob,
                    blank: confidence == 0.0,
                    tokens: Vec::new(),
                })
                .collect()
        };
        let offered = |sure: &[(f32, f32)], line: f64, token: f64| {
            ConfidenceThresholds { line, token }.offered(&lines(sure))
        };
        let (sure, unsure, blank) = ((0.9, 0.6), (0.4, 0.1), (0.0, 0.0));
        assert_eq!(offered(&[sure, sure, unsure, sure], 0.5, 0.0), 5);
        // Either threshold can stop it.
        assert_eq!(offered(&[sure, (0.9, 0.2)], 0.5, 0.0), 5);
        assert_eq!(offered(&[sure, (0.9, 0.2)], 0.5, 0.3), 2);
        assert_eq!(offered(&[unsure, sure], 0.5, 0.0), 0);
        assert_eq!(offered(&[unsure, sure], 0.0, 0.0), 5);
        // Blank lines, whose confidence is zero, are passed over on the
        // way to a line the model is sure of, but never end what is
        // offered, nor make up all of it.
        assert_eq!(offered(&[blank, sure], 0.5, 0.5), 5);
        assert_eq!(offered(&[sure, blank, sure], 0.5, 0.5), 8);
        assert_eq!(offered(&[sure, blank, unsure], 0.5, 0.0), 2);
        assert_eq!(offered(&[sure, blank], 0.0, 0.0), 2);
        assert_eq!(offered(&[blank, blank], 0.0, 0.0), 0);
        assert_eq!(offered(&[], 0.0, 0.0), 0);
    }

    #[test]
    fn the_start_of_a_first_line_the_model_is_sure_of_is_offered() {
        // `    foo_bar(x, y);` after a blank line, a token at a time with
        // how sure the model was of each, and a line after it.
        let text = "\n    foo_bar(x, y);\n    done();";
        let probs = [
            ("foo", 0.99),
            ("_bar", 0.95),
            ("(", 0.9),
            ("x", 0.3),
            (",", 0.9),
            (" y", 0.9),
            (");", 0.9),
        ];
        let mut end = "\n    ".len();
        let tokens: Vec<TokenConfidence> = probs
            .iter()
            .map(|&(token, prob)| {
                end += token.len();
                TokenConfidence {
                    end,
                    prob,
                    word_end: !splits_word(text, end),
                }
            })
            .collect();
        let line = |tokens: &[TokenConfidence]| {
            let logprob_sum: f32 = tokens.iter().map(|t| t.prob.ln()).sum();
            LineConfidence {
                end: tokens.last().unwrap().end,
                confidence: (logprob_sum / tokens.len() as f32).exp(),
                min_prob: tokens.iter().map(|t| t.prob).fold(1.0, f32::min),
                blank: false,
                tokens: tokens.to_vec(),
            }
        };
        let blank = LineConfidence {
            end: 0,
            confidence: 0.0,
            min_prob: 0.0,
            blank: true,
            tokens: Vec::new(),
        };
        let lines = [blank, line(&tokens)];
        let at = |line, token| ConfidenceThresholds { line, token };
        let offered = |t: ConfidenceThresholds| &text[..t.offered(&lines)];
        // Up to the token that falls short, with the blank line before.
        assert_eq!(offered(at(0.7, 0.4)), "\n    foo_bar(");
        // Never partway through a word: `foo` alone would be.
        assert!(!tokens[0].word_end && tokens[1].word_end);
        assert_eq!(offered(at(0.7, 0.96)), "");
        // Only as far as the tokens meet the line threshold together.
        assert_eq!(offered(at(0.96, 0.4)), "\n    foo_bar");
        assert_eq!(offered(at(0.99, 0.4)), "");
        // Only the first line with text is offered in part: after a line
        // it is sure of, one it isn't is left out whole.
        let (first, second) = (line(&tokens[..3]), line(&tokens[3..]));
        let lines = [first, second];
        assert_eq!(at(0.7, 0.4).offered(&lines), "\n    foo_bar(".len());
        // The second averages (0.3 * 0.9^3)^(1/4), about 0.68.
        assert_eq!(at(0.6, 0.2).offered(&lines), text.find(';').unwrap() + 1);
    }

    #[test]
    fn a_word_is_not_split() {
        assert!(splits_word("foo_bar", 3));
        assert!(splits_word("x1", 1));
        assert!(!splits_word("foo(", 3));
        assert!(!splits_word("foo bar", 3));
        assert!(!splits_word("foo", 3));
        assert!(!splits_word("foo", 0));
        assert!(splits_word("héllo", 1));
    }

    #[test]
    fn the_offered_part_is_the_start_of_the_text() {
        let (model, tokenizer) = tiny_model();
        let mut engine = Engine::new(&model, &tokenizer);
        let everything = ConfidenceThresholds {
            line: 0.0,
            token: 0.0,
        };
        let nothing = ConfidenceThresholds {
            line: 1.0,
            token: 1.0,
        };
        // Ending at a token boundary, partway through one, and with a
        // line break and indentation still to be covered.
        for prefix in [
            "fn main() {\n    let x = ",
            "fn main() {\n    let xy",
            "fn main() {\n    ",
        ] {
            let request = request(1, prefix, "\n}\n");
            let (text, lines) = engine.complete(&request, &mut || false).unwrap();
            let mut last = 0;
            for line in &lines {
                assert!(
                    line.end >= last && text.is_char_boundary(line.end),
                    "{prefix:?}"
                );
                // The line break before it, then the line.
                let content = &text[last..line.end];
                assert!(
                    content.matches('\n').count() <= 1,
                    "{prefix:?}: {content:?}"
                );
                assert!(
                    !content.trim_start().contains('\n'),
                    "{prefix:?}: {content:?}"
                );
                assert_eq!(
                    line.blank,
                    content.trim().is_empty(),
                    "{prefix:?}: {content:?}"
                );
                // Its tokens end in order through it, and at its end.
                let mut token_end = last;
                for token in &line.tokens {
                    assert!(
                        token.end >= token_end && token.end <= line.end,
                        "{prefix:?}"
                    );
                    assert_eq!(token.word_end, !splits_word(&text, token.end));
                    token_end = token.end;
                }
                if !line.tokens.is_empty() {
                    assert_eq!(token_end, line.end, "{prefix:?}");
                }
                last = line.end;
            }
            // All but blank lines at the end are offered.
            let offered = everything.offered(&lines);
            assert!(
                text[offered..].trim().is_empty(),
                "{prefix:?}: {text:?} {offered}"
            );
            assert_eq!(nothing.offered(&lines), 0, "{prefix:?}");
        }
    }

    #[test]
    fn the_thresholds_come_from_the_settings_with_each_request() {
        use crate::settings::SettingKey;
        let mut settings = Settings::default();
        let mut completer = Completer::new();
        completer.apply_settings(&settings);
        assert_eq!(completer.thresholds, ConfidenceThresholds::DEFAULT);
        settings
            .set_text(SettingKey::CompletionLineConfidence, "0.25")
            .unwrap();
        settings
            .set_text(SettingKey::CompletionTokenConfidence, "0.125")
            .unwrap();
        completer.apply_settings(&settings);
        assert_eq!(
            completer.thresholds,
            ConfidenceThresholds {
                line: 0.25,
                token: 0.125,
            }
        );
    }

    #[test]
    fn a_file_is_named_by_its_nearest_package() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"app\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("app/src/ui")).unwrap();
        std::fs::write(
            root.join("app/Cargo.toml"),
            "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        assert_eq!(
            crate_of(&root.join("app/src/ui/view.rs")),
            Some(("my-app".to_owned(), "src/ui/view.rs".to_owned()))
        );
        // A file under only a virtual workspace is in no crate.
        assert_eq!(crate_of(&root.join("build.rs")), None);
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
