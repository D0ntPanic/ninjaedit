//! Packs the corpus into fixed-length rows of tokens for training.
//!
//! Output layout, per split: `<split>.bin` holds rows of `seq_len` little-endian u16 tokens.
//! The high bit marks tokens excluded from the loss (padding and the crate/file header).
//! `meta.json` records the layout and counts.

mod fim;
mod pack;
mod rng;

use anyhow::{Context, Result, bail};
use clap::Parser;
use corpus::{CrateRecord, Split, expand_home, read_shard, shard_paths};
use pack::{Doc, MASK_BIT, PackStats, Packer};
use rayon::prelude::*;
use rng::Rng;
use serde::{Deserialize, Serialize};
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
    #[arg(long, default_value = "~/corpus/rust-crates")]
    corpus: String,
    #[arg(long, default_value = "~/corpus/tokenizers/rust-16384.json")]
    tokenizer: String,
    #[arg(long)]
    out: String,
    #[arg(long, default_value_t = 2048)]
    seq_len: usize,
    /// Use one of every N training crates. The valid split is always packed in full.
    #[arg(long, default_value_t = 1)]
    sample: u64,
    /// Fraction of documents given the fill-in-the-middle transformation.
    #[arg(long, default_value_t = 0.5)]
    fim_rate: f64,
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
    #[arg(long, default_value = "~/corpus/tokenizers/rust-16384.json")]
    tokenizer: String,
}

#[derive(Serialize, Deserialize)]
struct Meta {
    seq_len: usize,
    vocab_size: usize,
    mask_bit: u16,
    tokenizer: String,
    fim_rate: f64,
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
    let tok = Tokenizer::load(&expand_home(&args.tokenizer))?;
    if tok.vocab_size() > MASK_BIT as usize {
        bail!(
            "vocabulary of {} does not leave the mask bit free",
            tok.vocab_size()
        );
    }
    let out = expand_home(&args.out);
    fs::create_dir_all(&out)?;
    let shards = shard_paths(&expand_home(&args.corpus))?;
    let mut stats = [PackStats::default(), PackStats::default()];
    for (i, split) in [Split::Train, Split::Valid].into_iter().enumerate() {
        let sample = if split == Split::Train {
            args.sample
        } else {
            1
        };
        eprintln!("packing {} (1/{sample} of crates)...", split.name());
        stats[i] = pack_split(&tok, &shards, split, sample, &args, &out)?;
        let s = &stats[i];
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
        fim_rate: args.fim_rate,
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

/// Packs one split. Shards are processed in parallel into temporary files that are then
/// concatenated in shard order, so the output is deterministic for a given seed.
fn pack_split(
    tok: &Tokenizer,
    shards: &[PathBuf],
    split: Split,
    sample: u64,
    args: &BuildArgs,
    out: &Path,
) -> Result<PackStats> {
    let parts: Vec<Result<(PathBuf, PackStats)>> = shards
        .par_iter()
        .enumerate()
        .map(|(i, shard)| {
            let part = out.join(format!("{}.part{i:03}", split.name()));
            let file = BufWriter::new(File::create(&part)?);
            let mut packer = Packer::new(args.seq_len, tok.special("<pad>") as u16, file);
            let mut encoder = tok.encoder();
            let mut rng =
                Rng::new(args.seed ^ (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ split as u64);
            for record in read_shard(shard)? {
                let record = record?;
                if Split::of(&record.name) != split
                    || !XxHash3_64::oneshot(record.name.as_bytes()).is_multiple_of(sample)
                {
                    continue;
                }
                pack_crate(tok, &mut encoder, &record, args, &mut rng, &mut packer)?;
            }
            let (_, stats) = packer.finish()?;
            Ok((part, stats))
        })
        .collect();
    let mut total = PackStats::default();
    let mut output = BufWriter::new(File::create(out.join(format!("{}.bin", split.name())))?);
    for part in parts {
        let (path, stats) = part?;
        total.add(&stats);
        let mut file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
        std::io::copy(&mut file, &mut output)?;
        fs::remove_file(&path)?;
    }
    output.flush()?;
    Ok(total)
}

/// Room reserved beyond a document's content: the four FIM specials plus a margin for the
/// extra tokens that re-tokenizing at the split points can introduce.
const FIM_OVERHEAD: usize = 4 + 8;

fn pack_crate<W: Write>(
    tok: &Tokenizer,
    encoder: &mut Encoder,
    record: &CrateRecord,
    args: &BuildArgs,
    rng: &mut Rng,
    packer: &mut Packer<W>,
) -> Result<()> {
    let eos = tok.special("<eos>") as u16;
    let mut ids = Vec::new();
    for file in &record.files {
        // Header: crate and file name, excluded from the loss.
        ids.clear();
        ids.push(tok.special("<crate_name>"));
        encoder.encode_with(&record.name, pretok::Indent::Spaces(4), &mut ids);
        ids.push(tok.special("<file_name>"));
        encoder.encode_with(&file.path, pretok::Indent::Spaces(4), &mut ids);
        let header: Vec<u16> = ids.iter().map(|&id| id as u16 | MASK_BIT).collect();
        let max_content = match args.seq_len.checked_sub(header.len() + FIM_OVERHEAD + 2) {
            Some(n) if n >= 16 => n,
            _ => continue,
        };

        let indent = pretok::detect_indent(&file.content);
        for piece in pieces(tok, encoder, &file.content, indent, max_content) {
            ids.clear();
            let fim = rng.chance(args.fim_rate)
                && fim::transform(tok, encoder, piece, indent, rng, &mut ids);
            if !fim {
                ids.clear();
                encoder.encode_with(piece, indent, &mut ids);
            }
            let mut tokens = header.clone();
            tokens.extend(ids.iter().map(|&id| id as u16));
            tokens.push(eos);
            if tokens.len() > args.seq_len {
                // Rare: re-tokenizing at the split points exceeded the reserved margin.
                packer.stats.dropped_docs += 1;
                continue;
            }
            packer.push(Doc { tokens, fim })?;
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
