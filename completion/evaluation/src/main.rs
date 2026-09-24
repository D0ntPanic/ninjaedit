//! Measures how well a completion model does, end to end: through the
//! editor's own model of a buffer and its completion requests, with the
//! editor's completer running the model, on files of a real project.
//!
//! The directory given is where the files to evaluate on are, and needn't
//! be the project's root: the project is found from it as the editor
//! finds it, and every file the project's index lists under the directory
//! in the given language is evaluated (so files the project ignores,
//! like build output, are left out). In each file, sections are deleted
//! and written back with the model's help as a user would type them (see
//! [`holes`] for which, and [`replay`] for how), and what the model wrote
//! is counted (see [`stats`]). Everything random is seeded, so the same
//! arguments give the same evaluation.

mod document;
mod holes;
mod replay;
mod stats;

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use document::Document;
use holes::{HoleKind, Rng};
use ninjaedit_core::completion::expand_home;
use ninjaedit_core::{
    Completer, CompletionOutcome, Editor, Language, Project, ProjectKind, Settings,
};
use replay::Source;
use stats::Stats;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The seed everything random in the evaluation comes from, unless
/// another is given.
const DEFAULT_SEED: u64 = 0x6e69_6e6a_6165_6474;
/// How long to wait for the project's index.
const INDEX_TIMEOUT: Duration = Duration::from_secs(600);
/// How long to wait for a completion, the first of which loads the
/// model, before deciding the worker is stuck.
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Parser, Debug)]
#[command(about = "Evaluate a code completion model on a project, through the editor")]
struct Args {
    /// The directory whose files to evaluate on, in a project or the
    /// root of one.
    dir: PathBuf,

    /// The checkpoint directory of the model (config.json,
    /// model.safetensors, tokenizer.json).
    #[arg(long)]
    model: String,

    /// The language the model is for, such as `rust`; only files in it
    /// are evaluated.
    #[arg(long, value_parser = parse_language)]
    language: Language,

    /// How many sections to delete and write again per 100 lines of
    /// code, so that files are sampled in proportion to their size.
    #[arg(long, default_value_t = 2.0, value_parser = parse_rate)]
    hole_rate: f64,

    /// Evaluate on at most this many of the files, chosen at random
    /// (with the seed).
    #[arg(long)]
    max_files: Option<usize>,

    /// The seed for everything random.
    #[arg(long, default_value_t = DEFAULT_SEED)]
    seed: u64,

    /// Also write the results, per file too, as JSON to this file.
    #[arg(long)]
    json: Option<PathBuf>,

    /// Print every completion next to what was expected.
    #[arg(long)]
    trace: bool,
}

fn parse_rate(text: &str) -> Result<f64, String> {
    match text.parse::<f64>() {
        Ok(rate) if rate.is_finite() && rate >= 0.0 => Ok(rate),
        _ => Err(format!("`{text}` isn't a rate of zero or more")),
    }
}

fn parse_language(name: &str) -> Result<Language, String> {
    Language::from_fence_info(name)
        .or_else(|| {
            Language::ALL
                .into_iter()
                .find(|l| l.name().eq_ignore_ascii_case(name))
        })
        .ok_or_else(|| format!("unknown language `{name}`"))
}

fn main() -> Result<()> {
    let args = Args::parse();
    let dir = std::fs::canonicalize(&args.dir)
        .with_context(|| format!("can't open {}", args.dir.display()))?;
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    if !expand_home(&args.model).is_dir() {
        bail!("{} is not a model checkpoint directory", args.model);
    }

    let project = open_project(&dir)?;
    let files = choose_files(&project, &dir, &args)?;
    if files.is_empty() {
        bail!(
            "no {} files in the project's index under {}",
            args.language.name(),
            dir.display()
        );
    }
    eprintln!(
        "project {}: {} {} files to evaluate on",
        project.root().display(),
        files.len(),
        args.language.name()
    );

    // The completer as the editor has it, with its settings (the
    // defaults, so the user's own can't change the results), and the
    // model under evaluation over them.
    let mut completer = Completer::new();
    completer.apply_settings(&Settings::default());
    completer.override_model(args.language, &args.model);
    let mut source = ModelSource {
        completer,
        latencies: Vec::new(),
    };

    let started = Instant::now();
    let mut total = Stats::default();
    let mut by_kind: HashMap<HoleKind, Stats> = HashMap::new();
    let mut per_file = Vec::new();
    for (i, path) in files.iter().enumerate() {
        let name = path.strip_prefix(project.root()).unwrap_or(path);
        let name = name.to_string_lossy();
        let Some((stats, kinds)) = evaluate_file(&project, path, &name, &args, &mut source)? else {
            eprintln!("[{}/{}] {name}: skipped, not UTF-8", i + 1, files.len());
            continue;
        };
        eprintln!(
            "[{}/{}] {name}: {} holes, next word {}, whole line {}",
            i + 1,
            files.len(),
            stats.holes,
            stats.next_word().percent(),
            stats.whole_line().percent()
        );
        total += stats;
        for (kind, stats) in kinds {
            *by_kind.entry(kind).or_default() += stats;
        }
        per_file.push((name.into_owned(), stats));
    }

    print_report(
        &total,
        &by_kind,
        per_file.len(),
        &source.latencies,
        started.elapsed(),
    );
    if let Some(path) = &args.json {
        let json = serde_json::json!({
            "model": args.model,
            "language": args.language.name(),
            "dir": dir,
            "project": project.root(),
            "seed": args.seed,
            "hole_rate": args.hole_rate,
            "files": per_file.len(),
            "latency_ms": latency_summary(&source.latencies),
            "total": total.to_json(),
            "by_kind": HoleKind::ALL
                .iter()
                .map(|k| (k.key().to_owned(), by_kind.get(k).copied().unwrap_or_default().to_json()))
                .collect::<serde_json::Map<_, _>>(),
            "per_file": per_file
                .iter()
                .map(|(name, stats)| (name.clone(), stats.to_json()))
                .collect::<serde_json::Map<_, _>>(),
        });
        std::fs::write(path, serde_json::to_string_pretty(&json)? + "\n")
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// The project `dir` is in, found as the editor finds it. A directory
/// that isn't in a git repository is evaluated as a project of its own,
/// indexed in full, where the editor would only list its own files.
fn open_project(dir: &Path) -> Result<Project> {
    let project = Project::discover(dir)?;
    let project = match project.kind() {
        ProjectKind::Project => project,
        ProjectKind::Directory => Project::open(dir)?,
    };
    if !project.index().wait_for_primary(INDEX_TIMEOUT) {
        bail!("the project's index took too long to build");
    }
    Ok(project)
}

/// The files to evaluate on: those in the project's index (not ignored)
/// under `dir` and in the language, in order of path, or a random
/// choice of them.
fn choose_files(project: &Project, dir: &Path, args: &Args) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = project
        .index()
        .files(false)
        .into_iter()
        .filter(|path| path.starts_with(dir) && Language::from_path(path) == Some(args.language))
        .collect();
    files.sort();
    if let Some(max) = args.max_files {
        Rng::new(args.seed).shuffle(&mut files);
        files.truncate(max);
        files.sort();
    }
    Ok(files)
}

/// Evaluate one file: open it as the editor does, and delete and write
/// back each of its holes in turn. `None` if it isn't text.
fn evaluate_file(
    project: &Project,
    path: &Path,
    name: &str,
    args: &Args,
    source: &mut ModelSource,
) -> Result<Option<(Stats, HashMap<HoleKind, Stats>)>> {
    let buffer = project
        .open_file(path)
        .with_context(|| format!("opening {name}"))?;
    let original = buffer.to_bytes();
    let mut editor = Editor::new(buffer);
    if editor.language() != args.language {
        return Err(anyhow!("{name} opened as {}", editor.language().name()));
    }
    let Some(doc) = Document::from_buffer(editor.buffer(), editor.language()) else {
        return Ok(None);
    };
    // Seeded by the file's path in the project too, so a file gets the
    // same holes whichever other files are evaluated with it.
    let mut rng = Rng::new(args.seed ^ holes::hash(name));
    let mut stats = Stats::default();
    let mut kinds: HashMap<HoleKind, Stats> = HashMap::new();
    let count = holes::count(&doc, args.hole_rate, &mut rng);
    for hole in holes::choose(&doc, count, &mut rng) {
        if args.trace {
            let (line, _) = doc.line_and_column(hole.start);
            eprintln!("{name}:{} {} hole", line + 1, hole.kind.name());
        }
        let hole_stats = replay::reimplement(&mut editor, &doc, &hole, source, args.trace)?;
        if editor.buffer().to_bytes() != original {
            bail!("{name} wasn't written back as it was after a hole: {hole:?}");
        }
        stats += hole_stats;
        *kinds.entry(hole.kind).or_default() += hole_stats;
    }
    Ok(Some((stats, kinds)))
}

/// Completions from the model, through the editor's completer.
struct ModelSource {
    completer: Completer,
    /// How long each completion took, from request to answer.
    latencies: Vec<Duration>,
}

impl Source for ModelSource {
    fn complete(&mut self, editor: &mut Editor) -> Result<()> {
        let Some(request) = editor.take_completion_request() else {
            return Ok(());
        };
        let serial = request.serial;
        let started = Instant::now();
        self.completer.request(0, request);
        loop {
            let outcomes = self.completer.wait_for_outcomes(COMPLETION_TIMEOUT);
            if outcomes.is_empty() {
                bail!("no completion after {COMPLETION_TIMEOUT:?}");
            }
            for outcome in outcomes {
                match outcome {
                    CompletionOutcome::Completed {
                        serial: answered,
                        text,
                        ..
                    } if answered == serial => {
                        self.latencies.push(started.elapsed());
                        editor.offer_completion(serial, &text);
                        return Ok(());
                    }
                    CompletionOutcome::Completed { .. } => {}
                    CompletionOutcome::Failed { error, .. } => {
                        bail!("the model couldn't be loaded: {error}")
                    }
                }
            }
        }
    }
}

fn latency_summary(latencies: &[Duration]) -> serde_json::Value {
    let mut ms: Vec<f64> = latencies.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    if ms.is_empty() {
        return serde_json::Value::Null;
    }
    ms.sort_by(f64::total_cmp);
    let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    serde_json::json!({ "median": at(0.5), "p90": at(0.9), "max": at(1.0) })
}

fn print_report(
    total: &Stats,
    by_kind: &HashMap<HoleKind, Stats>,
    files: usize,
    latencies: &[Duration],
    elapsed: Duration,
) {
    let latency = latency_summary(latencies);
    println!();
    println!(
        "{files} files, {} holes, {} completions in {:.0?}",
        total.holes, total.completions, elapsed
    );
    if !latency.is_null() {
        println!(
            "completion latency: median {:.0} ms, p90 {:.0} ms",
            latency["median"].as_f64().unwrap_or(0.0),
            latency["p90"].as_f64().unwrap_or(0.0)
        );
    }
    println!();
    print!("{:<14}{:>16}", "", "all");
    for kind in HoleKind::ALL {
        print!("{:>14}", kind.name());
    }
    println!();
    let kinds: Vec<Stats> = HoleKind::ALL
        .iter()
        .map(|k| by_kind.get(k).copied().unwrap_or_default())
        .collect();
    for (row, (name, _, rate)) in total.rates().into_iter().enumerate() {
        let all = format!("{} {:>7}", rate.percent(), format!("({})", rate.total));
        print!("{name:<14}{all:>16}");
        for stats in &kinds {
            print!("{:>14}", stats.rates()[row].2.percent());
        }
        println!();
    }
    println!();
    println!("next word:    words accepted from completions, of all words written");
    println!(
        "partial line: lines finished by a completion begun partway, of lines not predicted whole"
    );
    println!(
        "whole line:   lines predicted start to end by one completion, of lines the holes took whole"
    );
    println!("multi-line:   completions right about 2+ lines, of those asked with 2+ lines left");
    println!("characters:   non-whitespace characters accepted, of all written");
}
