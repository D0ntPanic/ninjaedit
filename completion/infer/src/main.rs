//! Command-line front end for the CPU engine: sampling, and a throughput benchmark.

use anyhow::Result;
use clap::Parser;
use infer::{CompletionOptions, Model, Session, TokenSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokenizer::Tokenizer;
use tokenizer::pretok::{self, Indent};

#[derive(Parser)]
#[command(about = "CPU inference for the completion model")]
enum Command {
    /// Complete a prefix/suffix pair.
    Sample(SampleArgs),
    /// Measure prefill and decode throughput on synthetic tokens.
    Bench(BenchArgs),
}

#[derive(Parser)]
struct LogitsArgs {
    #[arg(long)]
    checkpoint: String,
    #[arg(long)]
    tokens: String,
    #[arg(long)]
    out: String,
    #[arg(long)]
    threads: Option<usize>,
}

#[derive(Parser)]
struct SampleArgs {
    #[arg(long)]
    checkpoint: String,
    #[arg(long, default_value = "~/corpus/tokenizers/rust-16384.json")]
    tokenizer: String,
    #[arg(long)]
    prefix: String,
    #[arg(long)]
    suffix: Option<String>,
    #[arg(long, default_value_t = 256)]
    tokens: usize,
    #[arg(long, default_value_t = 8)]
    max_lines: usize,
    /// End-of-middle probability at a line end that stops the completion.
    #[arg(long, default_value_t = 0.2)]
    stop_threshold: f32,
    #[arg(long, default_value_t = 0.0)]
    min_line_confidence: f32,
    /// Feed the prefix exactly as typed instead of backing up to the last pre-token boundary
    /// and constraining generation to cover what was typed past it.
    #[arg(long)]
    no_heal: bool,
    /// The module the text is from, for the header training documents open with. The header is
    /// only given with both this and `--file-name`.
    #[arg(long, requires = "file_name")]
    module_name: Option<String>,
    /// The path of the text's file within its module, such as `src/lib.rs`.
    #[arg(long, requires = "module_name")]
    file_name: Option<String>,
    #[arg(long)]
    threads: Option<usize>,
}

#[derive(Parser)]
struct BenchArgs {
    #[arg(long)]
    checkpoint: String,
    #[arg(long, default_value_t = 256)]
    prefill: usize,
    #[arg(long, default_value_t = 64)]
    decode: usize,
    #[arg(long)]
    threads: Option<usize>,
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return Path::new(&home).join(rest);
    }
    PathBuf::from(path)
}

/// Decode is bound by memory bandwidth, and on Apple silicon four threads already reach it;
/// more only add dispatch overhead and pull in the efficiency cores.
const DEFAULT_THREADS: usize = 4;

/// The tokenizer for a checkpoint: `tokenizer.json` inside it if present, else `fallback`.
/// A fingerprint recorded in the checkpoint's `state.json` must match, since a model is only
/// meaningful with the tokenizer it was trained with.
fn checkpoint_tokenizer(checkpoint: &Path, fallback: &str) -> Result<Tokenizer> {
    let bundled = checkpoint.join("tokenizer.json");
    let path = if bundled.exists() {
        bundled
    } else {
        expand_home(fallback)
    };
    let tok = Tokenizer::load(&path)?;
    if let Ok(text) = fs::read_to_string(checkpoint.join("state.json"))
        && let Ok(state) = serde_json::from_str::<serde_json::Value>(&text)
        && let Some(expected) = state.get("tokenizer_fingerprint").and_then(|v| v.as_str())
        && expected != tok.fingerprint()
    {
        anyhow::bail!(
            "tokenizer {} (fingerprint {}) is not the one this model was trained with ({expected}); copy the model's tokenizer to {}",
            path.display(),
            tok.fingerprint(),
            checkpoint.join("tokenizer.json").display()
        );
    }
    eprintln!("tokenizer {} ({})", path.display(), tok.fingerprint());
    Ok(tok)
}

fn set_threads(threads: Option<usize>) {
    let available = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let n = threads.unwrap_or(DEFAULT_THREADS.min(available));
    rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .build_global()
        .ok();
}

fn main() -> Result<()> {
    match Command::parse() {
        Command::Sample(args) => {
            set_threads(args.threads);
            let checkpoint = expand_home(&args.checkpoint);
            let tok = checkpoint_tokenizer(&checkpoint, &args.tokenizer)?;
            let started = Instant::now();
            let model = Model::load(&checkpoint)?;
            eprintln!(
                "loaded {:.1} MB of weights in {:.0} ms",
                model.weight_bytes() as f64 / 1e6,
                started.elapsed().as_secs_f64() * 1000.0
            );
            let prefix = fs::read_to_string(expand_home(&args.prefix))?;
            let suffix = match &args.suffix {
                Some(path) => fs::read_to_string(expand_home(path))?,
                None => String::new(),
            };
            let indent = pretok::detect_indent(&format!("{prefix}{suffix}"));
            let indent_str = match indent {
                Indent::Spaces(n) => " ".repeat(n),
                Indent::Tabs => "\t".to_owned(),
            };
            // The context ends at the last pre-token boundary; the rest of the prefix is the
            // partial token the completion must cover.
            let boundary = if args.no_heal {
                prefix.len()
            } else {
                pretok::last_piece_start(&prefix, indent)
            };
            let (context, partial) = prefix.split_at(boundary);
            let mut encoder = tok.encoder();
            let mut ids = Vec::new();
            if let (Some(module_name), Some(file_name)) = (&args.module_name, &args.file_name) {
                encoder.encode_header(module_name, file_name, &mut ids);
            }
            ids.extend([tok.special("<fim_prefix>"), tok.special("<fim_suffix>")]);
            encoder.encode_with(&suffix, indent, &mut ids);
            ids.push(tok.special("<fim_middle>"));
            encoder.encode_with(context, indent, &mut ids);
            let token_set = TokenSet {
                eom: tok.special("<eom>"),
                eos: tok.special("<eos>"),
                newline: Tokenizer::NEWLINE_BASE..Tokenizer::BYTE_BASE,
                render: tok.render_table(&indent_str),
            };
            let opts = CompletionOptions {
                max_lines: args.max_lines,
                max_tokens: args.tokens,
                stop_threshold: args.stop_threshold,
                min_line_confidence: args.min_line_confidence,
            };
            // The suffix's first content line, for the duplicate check.
            let mut suffix_ids = Vec::new();
            encoder.encode_with(&suffix, indent, &mut suffix_ids);
            let suffix_line: Vec<u32> = suffix_ids
                .iter()
                .copied()
                .skip_while(|t| token_set.newline.contains(t))
                .take_while(|t| !token_set.newline.contains(t))
                .collect();
            let mut session = Session::new(&model);
            let started = Instant::now();
            session.feed(&ids);
            let prefill = started.elapsed();
            let started = Instant::now();
            let completion = session.complete(&token_set, &opts, &suffix_line, partial.as_bytes());
            let decode = started.elapsed();
            let generated = completion.tokens();
            eprintln!(
                "prefill {} tokens in {:.1} ms; generated {} tokens in {:.1} ms ({:.2} ms/token); stopped: {:?}",
                ids.len(),
                prefill.as_secs_f64() * 1000.0,
                generated.len(),
                decode.as_secs_f64() * 1000.0,
                decode.as_secs_f64() * 1000.0 / generated.len().max(1) as f64,
                completion.reason
            );
            if !partial.is_empty() {
                let covered: Vec<String> = generated
                    .iter()
                    .scan(0usize, |seen, &t| {
                        let len = token_set.render[t as usize].len();
                        let done = *seen >= partial.len();
                        *seen += len;
                        Some((done, t))
                    })
                    .take_while(|&(done, _)| !done)
                    .map(|(_, t)| tok.token_text(t))
                    .collect();
                eprintln!("healed partial {partial:?} as {covered:?}");
            }
            for (i, line) in completion.lines.iter().enumerate() {
                eprintln!(
                    "  line {:>2}  confidence {:.2}  min {:.2}  stop {:.3}",
                    i + 1,
                    line.confidence,
                    line.min_prob,
                    line.stop_prob
                );
            }
            // The completion's text begins with the partial the user already typed.
            let text = tok.decode(&generated, &indent_str);
            println!("{}", text.get(partial.len()..).unwrap_or(""));
        }
        Command::Bench(args) => {
            set_threads(args.threads);
            let model = Model::load(&expand_home(&args.checkpoint))?;
            let vocab = model.config.vocab_size as u32;
            let prompt: Vec<u32> = (0..args.prefill as u32)
                .map(|i| (i * 7919 + 13) % vocab)
                .collect();
            // Warm up thread pool and caches.
            let mut cache = model.new_cache();
            model.forward(&prompt[..8.min(prompt.len())], &mut cache, false);
            let mut cache = model.new_cache();
            let started = Instant::now();
            model.forward(&prompt, &mut cache, false);
            let prefill = started.elapsed().as_secs_f64();
            let started = Instant::now();
            for i in 0..args.decode {
                model.forward(&[(i as u32 * 31) % vocab], &mut cache, false);
            }
            let decode = started.elapsed().as_secs_f64();
            let per_token = decode / args.decode as f64;
            let (decode_threads, prefill_threads) = model.threads();
            println!(
                "threads {decode_threads}/{prefill_threads}: prefill {} tokens {:.1} ms ({:.0} tok/s); decode {:.2} ms/token ({:.1} GB/s of weights)",
                args.prefill,
                prefill * 1000.0,
                args.prefill as f64 / prefill,
                per_token * 1000.0,
                model.weight_bytes() as f64 / per_token / 1e9
            );
        }
    }
    Ok(())
}
