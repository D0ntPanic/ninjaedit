//! Trains and evaluates the tokenizer on the corpus.

use anyhow::{Result, bail};
use clap::Parser;
use corpus::{Split, expand_home, read_shard, shard_paths};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokenizer::pretok::{self, Piece};
use tokenizer::{Tokenizer, bpe};
use twox_hash::XxHash3_64;

#[derive(Parser)]
#[command(about = "Train, evaluate and inspect the Rust source tokenizer")]
enum Command {
    /// Learn BPE merges from a sample of the training split.
    Train(TrainArgs),
    /// Measure compression on a corpus split.
    Eval(EvalArgs),
    /// Show how a file is tokenized.
    Encode(EncodeArgs),
    /// List the vocabulary.
    Vocab(VocabArgs),
}

#[derive(Parser)]
struct TrainArgs {
    #[arg(long, default_value = "~/corpus/rust-crates")]
    corpus: String,
    #[arg(long)]
    out: String,
    /// Total vocabulary size including special, line-break and byte tokens.
    #[arg(long, default_value_t = 32768)]
    vocab_size: usize,
    /// Use one of every N training crates.
    #[arg(long, default_value_t = 16)]
    sample: u64,
}

#[derive(Parser)]
struct EvalArgs {
    #[arg(long, default_value = "~/corpus/rust-crates")]
    corpus: String,
    #[arg(long)]
    model: String,
    #[arg(long, default_value = "valid")]
    split: Split,
}

#[derive(Parser)]
struct EncodeArgs {
    #[arg(long)]
    model: String,
    path: String,
    /// Print one token per line with its id instead of the inline rendering.
    #[arg(long)]
    ids: bool,
}

#[derive(Parser)]
struct VocabArgs {
    #[arg(long)]
    model: String,
}

fn main() -> Result<()> {
    match Command::parse() {
        Command::Train(args) => train(args),
        Command::Eval(args) => eval(args),
        Command::Encode(args) => encode(args),
        Command::Vocab(args) => vocab(args),
    }
}

/// Whether a training crate is part of the sample used to learn merges.
fn sampled(name: &str, every: u64) -> bool {
    Split::of(name) == Split::Train && XxHash3_64::oneshot(name.as_bytes()).is_multiple_of(every)
}

fn train(args: TrainArgs) -> Result<()> {
    let started = Instant::now();
    let num_merges = args
        .vocab_size
        .checked_sub(Tokenizer::MERGE_BASE as usize)
        .filter(|&n| n > 0)
        .unwrap_or_else(|| panic!("vocab size must exceed {}", Tokenizer::MERGE_BASE));
    let shards = shard_paths(&expand_home(&args.corpus))?;
    eprintln!(
        "counting pre-tokens in 1/{} of training crates across {} shards...",
        args.sample,
        shards.len()
    );

    /// Pre-token counts of one shard's sample, with its byte and line-break totals.
    type ShardCounts = (FxHashMap<Vec<u8>, u64>, u64, u64);
    let counted: Vec<Result<ShardCounts>> = shards
        .par_iter()
        .map(|shard| {
            let mut counts: FxHashMap<Vec<u8>, u64> = FxHashMap::default();
            let mut bytes = 0u64;
            let mut newlines = 0u64;
            for record in read_shard(shard)? {
                let record = record?;
                if !sampled(&record.name, args.sample) {
                    continue;
                }
                for file in &record.files {
                    bytes += file.content.len() as u64;
                    let indent = pretok::detect_indent(&file.content);
                    for piece in pretok::pretokenize(&file.content, indent) {
                        match piece {
                            Piece::Newline(_) => newlines += 1,
                            Piece::Bytes(b) => match counts.get_mut(b) {
                                Some(c) => *c += 1,
                                None => {
                                    counts.insert(b.to_vec(), 1);
                                }
                            },
                        }
                    }
                }
            }
            Ok((counts, bytes, newlines))
        })
        .collect();
    let mut counts: FxHashMap<Vec<u8>, u64> = FxHashMap::default();
    let mut sample_bytes = 0u64;
    let mut sample_newlines = 0u64;
    for result in counted {
        let (c, b, n) = result?;
        sample_bytes += b;
        sample_newlines += n;
        for (k, v) in c {
            *counts.entry(k).or_default() += v;
        }
    }
    let pre_tokens: u64 = counts.values().sum();
    eprintln!(
        "{} distinct pre-tokens, {} occurrences, {} bytes, {} line breaks ({:.0}s)",
        counts.len(),
        pre_tokens,
        sample_bytes,
        sample_newlines,
        started.elapsed().as_secs_f64()
    );

    let mut words: Vec<(Vec<u32>, u64)> = counts
        .into_iter()
        .filter(|(w, _)| w.len() > 1)
        .map(|(w, c)| {
            (
                w.iter().map(|&b| Tokenizer::BYTE_BASE + b as u32).collect(),
                c,
            )
        })
        .collect();
    words.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    eprintln!("learning {num_merges} merges...");
    let mut vocab: Vec<Vec<u8>> = (0..Tokenizer::MERGE_BASE)
        .map(|id| {
            id.checked_sub(Tokenizer::BYTE_BASE)
                .map(|b| vec![b as u8])
                .unwrap_or_default()
        })
        .collect();
    let merges = bpe::train(
        words.clone(),
        Tokenizer::MERGE_BASE,
        num_merges,
        |n, (a, b), count| {
            let mut text = vocab[a as usize].clone();
            text.extend_from_slice(&vocab[b as usize]);
            if n.is_multiple_of(2000) || n == num_merges {
                eprintln!(
                    "  {n} merges: last {:?} x{count} ({:.0}s)",
                    String::from_utf8_lossy(&text),
                    started.elapsed().as_secs_f64()
                );
            }
            vocab.push(text);
        },
    );
    let tok = Tokenizer::from_merges(merges);
    let out = expand_home(&args.out);
    tok.save(&out)?;

    // Compression on the training sample itself, computed by encoding the pre-token table.
    let mut encoder = tok.encoder();
    let mut buf = Vec::new();
    let mut tokens = sample_newlines;
    for (word, count) in &words {
        let bytes: Vec<u8> = word
            .iter()
            .map(|&id| (id - Tokenizer::BYTE_BASE) as u8)
            .collect();
        buf.clear();
        encoder.encode_bytes(&bytes, &mut buf);
        tokens += buf.len() as u64 * count;
    }
    // Single-byte pre-tokens were filtered out of `words`; they are one token each.
    tokens += pre_tokens - words.iter().map(|(_, c)| c).sum::<u64>();
    println!("vocab size          {}", tok.vocab_size());
    println!("sample bytes        {sample_bytes}");
    println!("sample tokens       {tokens}");
    println!(
        "bytes per token     {:.3}",
        sample_bytes as f64 / tokens as f64
    );
    println!(
        "wrote {} ({:.0}s)",
        out.display(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn eval(args: EvalArgs) -> Result<()> {
    let started = Instant::now();
    let tok = Tokenizer::load(&expand_home(&args.model))?;
    let shards = shard_paths(&expand_home(&args.corpus))?;
    let files = AtomicU64::new(0);
    let bytes = AtomicU64::new(0);
    let lines = AtomicU64::new(0);
    let tokens = AtomicU64::new(0);
    let crates = AtomicU64::new(0);
    let hist = std::sync::Mutex::new(vec![0u64; tok.vocab_size()]);
    shards.par_iter().try_for_each(|shard| -> Result<()> {
        let mut encoder = tok.encoder();
        let mut buf = Vec::new();
        let mut local_hist = vec![0u64; tok.vocab_size()];
        for record in read_shard(shard)? {
            let record = record?;
            if Split::of(&record.name) != args.split {
                continue;
            }
            crates.fetch_add(1, Ordering::Relaxed);
            for file in &record.files {
                buf.clear();
                encoder.encode(&file.content, &mut buf);
                files.fetch_add(1, Ordering::Relaxed);
                bytes.fetch_add(file.content.len() as u64, Ordering::Relaxed);
                lines.fetch_add(file.content.lines().count() as u64, Ordering::Relaxed);
                tokens.fetch_add(buf.len() as u64, Ordering::Relaxed);
                for &id in &buf {
                    local_hist[id as usize] += 1;
                }
            }
        }
        let mut hist = hist.lock().unwrap();
        for (h, l) in hist.iter_mut().zip(&local_hist) {
            *h += l;
        }
        Ok(())
    })?;
    let hist = hist.into_inner().unwrap();
    let tokens = tokens.load(Ordering::Relaxed);
    let bytes = bytes.load(Ordering::Relaxed);
    let newline_tokens: u64 = (Tokenizer::NEWLINE_BASE..Tokenizer::BYTE_BASE)
        .map(|id| hist[id as usize])
        .sum();
    let byte_tokens: u64 = (Tokenizer::BYTE_BASE..Tokenizer::MERGE_BASE)
        .map(|id| hist[id as usize])
        .sum();
    let unused = hist[Tokenizer::MERGE_BASE as usize..]
        .iter()
        .filter(|&&c| c == 0)
        .count();
    println!("split               {}", args.split.name());
    println!("crates              {}", crates.load(Ordering::Relaxed));
    println!("files               {}", files.load(Ordering::Relaxed));
    println!("bytes               {bytes}");
    println!("lines               {}", lines.load(Ordering::Relaxed));
    println!("tokens              {tokens}");
    println!("bytes per token     {:.3}", bytes as f64 / tokens as f64);
    println!(
        "tokens per line     {:.3}",
        tokens as f64 / lines.load(Ordering::Relaxed) as f64
    );
    println!(
        "line-break tokens   {:.1}%",
        100.0 * newline_tokens as f64 / tokens as f64
    );
    println!(
        "single-byte tokens  {:.1}%",
        100.0 * byte_tokens as f64 / tokens as f64
    );
    println!("unused merges       {unused}");
    let mut top: Vec<(u64, u32)> = hist
        .iter()
        .enumerate()
        .map(|(id, &c)| (c, id as u32))
        .collect();
    top.sort_unstable_by(|a, b| b.cmp(a));
    println!("most common tokens:");
    for (c, id) in top.iter().take(40) {
        println!("  {:>10}  {:?}", c, tok.token_text(*id));
    }
    eprintln!("({:.0}s)", started.elapsed().as_secs_f64());
    Ok(())
}

fn encode(args: EncodeArgs) -> Result<()> {
    let tok = Tokenizer::load(&expand_home(&args.model))?;
    let path = PathBuf::from(&args.path);
    let text = std::fs::read_to_string(&path)?;
    let mut ids = Vec::new();
    tok.encoder().encode(&text, &mut ids);
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if args.ids {
        for id in &ids {
            writeln!(out, "{id}\t{:?}", tok.token_text(*id))?;
        }
    } else {
        for id in &ids {
            if tok.newline_level(*id).is_some() {
                writeln!(out, "{}", tok.token_text(*id))?;
            } else {
                write!(out, "[{}]", tok.token_text(*id))?;
            }
        }
        writeln!(out)?;
    }
    eprintln!(
        "{} bytes, {} tokens, {:.3} bytes/token, indent {:?}",
        text.len(),
        ids.len(),
        text.len() as f64 / ids.len() as f64,
        pretok::detect_indent(&text)
    );
    if tok.decode(&ids, "    ") != text && pretok::detect_indent(&text) == pretok::Indent::Spaces(4)
    {
        bail!("decode did not round-trip");
    }
    Ok(())
}

fn vocab(args: VocabArgs) -> Result<()> {
    let tok = Tokenizer::load(&expand_home(&args.model))?;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for id in 0..tok.vocab_size() as u32 {
        writeln!(out, "{id}\t{:?}", tok.token_text(id))?;
    }
    Ok(())
}
