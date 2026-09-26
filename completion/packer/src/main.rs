//! Packs the corpus into fixed-length rows of tokens for training.
//!
//! Output layout, per split: `<split>.bin` holds rows of `seq_len` little-endian u16 tokens.
//! The high bit marks tokens excluded from the loss (padding and the crate/file header).
//! `meta.json` records the layout and counts.
//!
//! Every document opens as the editor's prompts do: the header, then `<fim_prefix>`. FIM
//! documents are in the SPM layout the editor uses, or with probability `--psm-rate` in PSM;
//! the rest are framed as SPM documents with an empty suffix, which is the prompt for a cursor
//! at the end of a file. A plain document split to fill the end of a row is cut at a line
//! break, and the rest opens with a masked copy of its header and frame, as a prompt whose
//! context window starts partway through the file does.
//!
//! The corpus can be several directories, and every file in it names its language. Only the
//! files in the languages given by `--languages` are packed, so a Rust model can leave out the
//! `Cargo.toml` files of the Rust corpus. `meta.json` lists the languages, which the trainer
//! records in the model's config: the editor picks a model for a file by them, and prompts open
//! as the training documents did. With more than one language, every document's header opens
//! with `<lang>` and the language; a single-language model is trained without it.

mod fim;
mod pack;
mod rng;

use anyhow::{Context, Result, bail};
use clap::Parser;
use corpus::{PackageRecord, Split, expand_home, parse_languages, read_shard, shard_paths};
use pack::{Doc, MASK_BIT, PackStats, Packer};
use rayon::prelude::*;
use rng::Rng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokenizer::pretok;
use tokenizer::{Encoder, Tokenizer};
use twox_hash::XxHash3_64;

#[derive(Parser)]
#[command(about = "Pack the corpus into training rows")]
enum Command {
    /// Tokenize and pack the train and valid splits.
    Build(BuildArgs),
    /// Decode one row of a packed split for inspection.
    Show(ShowArgs),
}

#[derive(Parser)]
struct BuildArgs {
    /// Corpus directories, comma-separated.
    #[arg(long, value_delimiter = ',', required = true)]
    corpus: Vec<String>,
    /// Languages to train on, comma-separated, as the corpus names them (`rust`, `cargo`, `c`,
    /// `cpp`). Files in other languages are left out.
    #[arg(long, value_delimiter = ',', required = true)]
    languages: Vec<String>,
    #[arg(long)]
    tokenizer: String,
    #[arg(long)]
    out: String,
    #[arg(long, default_value_t = 2048)]
    seq_len: usize,
    /// Use one of every N training records. The valid split is always packed in full.
    #[arg(long, default_value_t = 1)]
    sample: u64,
    /// Fraction of documents given the fill-in-the-middle transformation. The rest are
    /// framed as SPM documents with an empty suffix (see `fim::frame`).
    #[arg(long, default_value_t = 0.5)]
    fim_rate: f64,
    /// Fraction of FIM documents in the PSM layout; the rest are SPM, the layout the editor's
    /// prompts use. None by default: at 70m and 500M tokens of C/C++, PSM documents made no
    /// difference to completions and slightly worsened loss on the SPM layout.
    #[arg(long, default_value_t = 0.0)]
    psm_rate: f64,
    #[arg(long, default_value_t = 1)]
    seed: u64,
}

#[derive(Parser)]
struct ShowArgs {
    /// The packed dataset directory.
    dir: String,
    #[arg(long, default_value = "valid")]
    split: Split,
    #[arg(long, default_value_t = 0)]
    row: u64,
    #[arg(long, required = true)]
    tokenizer: String,
}

#[derive(Serialize, Deserialize)]
struct Meta {
    seq_len: usize,
    vocab_size: usize,
    mask_bit: u16,
    tokenizer: String,
    tokenizer_fingerprint: String,
    /// The languages packed, in the order given.
    #[serde(default)]
    languages: Vec<String>,
    fim_rate: f64,
    #[serde(default)]
    psm_rate: f64,
    sample: u64,
    seed: u64,
    train: PackStats,
    valid: PackStats,
}

fn main() -> Result<()> {
    match Command::parse() {
        Command::Build(args) => build(args),
        Command::Show(args) => show(args),
    }
}

fn build(args: BuildArgs) -> Result<()> {
    let started = Instant::now();
    let languages = parse_languages(&args.languages)?;
    let tok = Tokenizer::load(&expand_home(&args.tokenizer))?;
    if tok.vocab_size() > MASK_BIT as usize {
        bail!(
            "vocabulary of {} does not leave the mask bit free",
            tok.vocab_size()
        );
    }
    let out = expand_home(&args.out);
    let mut shards = Vec::new();
    for dir in &args.corpus {
        let found = shard_paths(&expand_home(dir))?;
        if found.is_empty() {
            bail!("no shards in {dir}");
        }
        eprintln!("{dir}: {} shards", found.len());
        shards.extend(found);
    }
    eprintln!("languages: {}", languages.join(", "));
    fs::create_dir_all(&out)?;
    let mut stats = [PackStats::default(), PackStats::default()];
    for (i, split) in [Split::Train, Split::Valid].into_iter().enumerate() {
        let sample = if split == Split::Train {
            args.sample
        } else {
            1
        };
        eprintln!("packing {} (1/{sample} of records)...", split.name());
        let (split_stats, files) =
            pack_split(&tok, &shards, split, sample, &languages, &args, &out)?;
        stats[i] = split_stats;
        let s = &stats[i];
        let packed: Vec<String> = languages
            .iter()
            .map(|l| format!("{l} {}", files.packed.get(l).copied().unwrap_or(0)))
            .collect();
        eprintln!("  files: {}", packed.join(", "));
        if !files.skipped.is_empty() {
            let skipped: Vec<String> = files
                .skipped
                .iter()
                .map(|(l, n)| format!("{l} {n}"))
                .collect();
            eprintln!("  skipped: {}", skipped.join(", "));
        }
        if split == Split::Train
            && let Some(missing) = languages.iter().find(|l| !files.packed.contains_key(*l))
        {
            bail!("the corpus has no files in {missing}");
        }
        let spans: Vec<String> = fim::SpanKind::ALL
            .iter()
            .zip(s.spans)
            .map(|(k, n)| format!("{} {n}", k.name()))
            .collect();
        eprintln!("  spans: {}", spans.join(", "));
        eprintln!(
            "  {} rows, {} tokens, {} docs ({} fim, {} split), {:.2}% pad, {:.2}% masked ({:.0}s)",
            s.rows,
            s.tokens,
            s.docs,
            s.fim_docs,
            s.split_docs,
            100.0 * s.pad_tokens as f64 / s.tokens.max(1) as f64,
            100.0 * s.masked_tokens as f64 / s.tokens.max(1) as f64,
            started.elapsed().as_secs_f64()
        );
    }
    let meta = Meta {
        seq_len: args.seq_len,
        vocab_size: tok.vocab_size(),
        mask_bit: MASK_BIT,
        tokenizer: args.tokenizer.clone(),
        tokenizer_fingerprint: tok.fingerprint(),
        languages,
        fim_rate: args.fim_rate,
        psm_rate: args.psm_rate,
        sample: args.sample,
        seed: args.seed,
        train: stats[0],
        valid: stats[1],
    };
    fs::write(out.join("meta.json"), serde_json::to_string_pretty(&meta)?)?;
    eprintln!(
        "wrote {} ({:.0}s)",
        out.display(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Files counted by language: those packed, and those left out.
#[derive(Default)]
struct FileCounts {
    packed: BTreeMap<String, u64>,
    skipped: BTreeMap<String, u64>,
}

impl FileCounts {
    fn add(&mut self, other: FileCounts) {
        for (mine, theirs) in [
            (&mut self.packed, other.packed),
            (&mut self.skipped, other.skipped),
        ] {
            for (language, n) in theirs {
                *mine.entry(language).or_default() += n;
            }
        }
    }
}

/// Packs one split. Shards are processed in parallel into temporary files that are then
/// concatenated in shard order, so the output is deterministic for a given seed.
fn pack_split(
    tok: &Tokenizer,
    shards: &[PathBuf],
    split: Split,
    sample: u64,
    languages: &[String],
    args: &BuildArgs,
    out: &Path,
) -> Result<(PackStats, FileCounts)> {
    let parts: Vec<Result<(PathBuf, PackStats, FileCounts)>> = shards
        .par_iter()
        .enumerate()
        .map(|(i, shard)| {
            let part = out.join(format!("{}.part{i:03}", split.name()));
            let mut files = FileCounts::default();
            let file = BufWriter::new(File::create(&part)?);
            let newlines = Tokenizer::NEWLINE_BASE as u16..Tokenizer::BYTE_BASE as u16;
            let mut packer = Packer::new(args.seq_len, tok.special("<pad>") as u16, newlines, file);
            let mut encoder = tok.encoder();
            let mut rng =
                Rng::new(args.seed ^ (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ split as u64);
            for record in read_shard(shard)? {
                let mut record = record?;
                if Split::of(&record.name) != split
                    || !XxHash3_64::oneshot(record.name.as_bytes()).is_multiple_of(sample)
                {
                    continue;
                }
                record.files.retain(|file| {
                    let keep = languages.contains(&file.language);
                    let counts = if keep {
                        &mut files.packed
                    } else {
                        &mut files.skipped
                    };
                    *counts.entry(file.language.clone()).or_default() += 1;
                    keep
                });
                pack_record(
                    tok,
                    &mut encoder,
                    &record,
                    languages,
                    args,
                    &mut rng,
                    &mut packer,
                )?;
            }
            let (_, stats) = packer.finish()?;
            Ok((part, stats, files))
        })
        .collect();
    let mut total = PackStats::default();
    let mut files = FileCounts::default();
    let mut output = BufWriter::new(File::create(out.join(format!("{}.bin", split.name())))?);
    for part in parts {
        let (path, stats, counts) = part?;
        total.add(&stats);
        files.add(counts);
        let mut file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
        std::io::copy(&mut file, &mut output)?;
        fs::remove_file(&path)?;
    }
    output.flush()?;
    Ok((total, files))
}

/// Room reserved beyond a document's content: the four FIM specials plus a margin for the
/// extra tokens that re-tokenizing at the split points can introduce.
const FIM_OVERHEAD: usize = 4 + 8;

fn pack_record<W: Write>(
    tok: &Tokenizer,
    encoder: &mut Encoder,
    record: &PackageRecord,
    languages: &[String],
    args: &BuildArgs,
    rng: &mut Rng,
    packer: &mut Packer<W>,
) -> Result<()> {
    let eos = tok.special("<eos>") as u16;
    let mut ids = Vec::new();
    for file in &record.files {
        // The language is named in headers only when there are several to tell apart.
        let language = (languages.len() > 1).then_some(file.language.as_str());
        // Header: the language, when the model has several, and the crate and file name,
        // excluded from the loss.
        ids.clear();
        encoder.encode_header(language, Some((&record.name, &file.path)), &mut ids);
        let header: Vec<u16> = ids.iter().map(|&id| id as u16 | MASK_BIT).collect();
        let max_content = match args.seq_len.checked_sub(header.len() + FIM_OVERHEAD + 2) {
            Some(n) if n >= 16 => n,
            _ => continue,
        };

        let indent = pretok::detect_indent(&file.content);
        for piece in pieces(tok, encoder, &file.content, indent, max_content) {
            ids.clear();
            let made = if rng.chance(args.fim_rate) {
                fim::transform(tok, encoder, piece, indent, args.psm_rate, rng, &mut ids)
            } else {
                None
            };
            if let Some(made) = made {
                packer.stats.spans[made.kind as usize] += 1;
                packer.stats.psm_docs += u64::from(made.psm);
            } else {
                ids.clear();
                fim::frame(tok, encoder, piece, indent, &mut ids);
            }
            let mut tokens = header.clone();
            tokens.extend(ids.iter().map(|&id| id as u16));
            tokens.push(eos);
            if tokens.len() > args.seq_len {
                // Rare: re-tokenizing at the split points exceeded the reserved margin.
                packer.stats.dropped_docs += 1;
                continue;
            }
            packer.push(match made {
                Some(_) => Doc::fim(tokens),
                None => Doc::plain(tokens, header.len() + fim::FRAME_TOKENS),
            })?;
        }
    }
    Ok(())
}

/// Splits a file into runs of whole lines that each tokenize to at most `max_tokens`.
/// Lines too long on their own are dropped. Every piece after the first starts with the
/// line break that precedes it, so its first line's indentation is a line-break token
/// rather than literal whitespace.
fn pieces<'a>(
    tok: &Tokenizer,
    encoder: &mut Encoder,
    text: &'a str,
    indent: pretok::Indent,
    max_tokens: usize,
) -> Vec<&'a str> {
    let mut ids = Vec::new();
    encoder.encode_with(text, indent, &mut ids);
    if ids.len() <= max_tokens {
        return vec![text];
    }
    // Token count of each line, taken from the line-break tokens in the full encoding.
    let mut line_tokens = Vec::new();
    let mut count = 0usize;
    for &id in &ids {
        count += 1;
        if tok.newline_level(id).is_some() {
            line_tokens.push(count);
            count = 0;
        }
    }
    if count > 0 {
        line_tokens.push(count);
    }
    let mut out = Vec::new();
    let mut offset = 0;
    let mut start = 0;
    let mut acc = 0;
    // Pieces begin one byte early to include the previous line's newline.
    let piece = |start: usize, end: usize| &text[start.saturating_sub(1)..end];
    for (line, &n) in text.split_inclusive('\n').zip(&line_tokens) {
        if n > max_tokens {
            if offset > start {
                out.push(piece(start, offset));
            }
            offset += line.len();
            start = offset;
            acc = 0;
            continue;
        }
        if acc + n > max_tokens {
            out.push(piece(start, offset));
            start = offset;
            acc = 0;
        }
        // The leading newline of a later piece costs one token.
        acc += n + usize::from(acc == 0 && start > 0);
        offset += line.len();
    }
    if offset > start {
        out.push(piece(start, offset));
    }
    out
}

fn show(args: ShowArgs) -> Result<()> {
    let dir = expand_home(&args.dir);
    let meta: Meta = serde_json::from_str(&fs::read_to_string(dir.join("meta.json"))?)?;
    let tok = Tokenizer::load(&expand_home(&args.tokenizer))?;
    let mut file = File::open(dir.join(format!("{}.bin", args.split.name())))?;
    file.seek(SeekFrom::Start(args.row * meta.seq_len as u64 * 2))?;
    let mut bytes = vec![0u8; meta.seq_len * 2];
    file.read_exact(&mut bytes).context("row out of range")?;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut masked = 0;
    for pair in bytes.chunks(2) {
        let raw = u16::from_le_bytes([pair[0], pair[1]]);
        let id = (raw & !MASK_BIT) as u32;
        if raw & MASK_BIT != 0 {
            masked += 1;
        }
        let text = tok.token_text(id);
        if tok.newline_level(id).is_some() {
            writeln!(out, "{text}")?;
        } else if tok.is_special(id) {
            write!(
                out,
                "{}{text}{}",
                if raw & MASK_BIT != 0 { "{" } else { "" },
                if raw & MASK_BIT != 0 { "}" } else { "" }
            )?;
            if id == tok.special("<eos>") {
                writeln!(out)?;
            }
        } else {
            write!(
                out,
                "[{}{text}]",
                if raw & MASK_BIT != 0 { "~" } else { "" }
            )?;
        }
    }
    writeln!(out)?;
    eprintln!(
        "row {} of {}: {} tokens, {masked} masked",
        args.row,
        args.split.name(),
        meta.seq_len
    );
    Ok(())
}
