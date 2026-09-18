//! Builds a filtered, deduplicated Rust source corpus from a local Panamax mirror of crates.io.
//!
//! The output is a directory of zstd-compressed JSONL shards. Each line is one crate with its
//! surviving `.rs` files, edition, license and dependency list, so later stages can build
//! repository-aware training examples from whole crates rather than isolated files.

mod filter;
mod index;
mod license;
mod minhash;
mod stats;

use anyhow::{Context, Result, bail};
use clap::Parser;
use corpus::{CrateRecord, FileRecord, expand_home, read_shard};
use flate2::read::GzDecoder;
use index::CrateEntry;
use license::Policy;
use minhash::Signature;
use rayon::prelude::*;
use stats::Stats;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Instant;
use twox_hash::XxHash3_64;

#[derive(Parser)]
#[command(about = "Build and inspect a filtered Rust source corpus from a local crates.io mirror")]
enum Command {
    /// Build the corpus from the mirror.
    Build(BuildArgs),
    /// List the crates in a shard, or show one crate's files.
    Inspect(InspectArgs),
}

#[derive(Parser)]
struct InspectArgs {
    /// A `shard-NNN.jsonl.zst` file.
    shard: String,
    /// Show the files of this crate instead of listing crates.
    #[arg(long = "crate")]
    krate: Option<String>,
    /// With `--crate`, print the content of this file.
    #[arg(long)]
    file: Option<String>,
    /// Stop after this many crates when listing.
    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Parser)]
struct BuildArgs {
    /// Root of the Panamax mirror (contains `crates/` and `crates.io-index/`).
    #[arg(long, default_value = "~/crates-mirror")]
    mirror: String,
    /// Output directory for the corpus shards and stats.
    #[arg(long, default_value = "~/corpus/rust-crates")]
    out: String,
    /// Which crate licenses to accept.
    #[arg(long, value_enum, default_value_t = Policy::Permissive)]
    licenses: Policy,
    /// Process only every Nth crate, for quick experiments.
    #[arg(long)]
    sample: Option<usize>,
    /// Number of worker threads (defaults to the CPU count).
    #[arg(long)]
    jobs: Option<usize>,
    /// Estimated Jaccard similarity at or above which two files are near-duplicates.
    #[arg(long, default_value_t = 0.8)]
    near_dup_threshold: f64,
    /// Skip near-duplicate detection (exact duplicates are always removed).
    #[arg(long)]
    skip_near_dedup: bool,
    /// Write one line per rejected file (crate, path, reason) to this file, for tuning filters.
    #[arg(long)]
    reject_log: Option<String>,
}

/// Optional log of rejected files, shared across worker threads.
struct RejectLog(Mutex<BufWriter<File>>);

impl RejectLog {
    fn open(path: Option<&str>) -> Result<Option<RejectLog>> {
        let Some(path) = path else { return Ok(None) };
        let file = File::create(expand_home(path))?;
        Ok(Some(RejectLog(Mutex::new(BufWriter::new(file)))))
    }

    fn log(&self, krate: &str, path: &str, reason: &str, detail: &str) {
        let mut w = self.0.lock().unwrap();
        let _ = writeln!(w, "{reason}\t{krate}\t{path}\t{detail}");
    }
}

/// Upper bounds on how much of a single crate is read, to keep pathological crates in check.
const MAX_CRATE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CRATE_FILES: usize = 5000;
/// Uncompressed size at which an output shard is rotated.
const SHARD_BYTES: u64 = 1024 * 1024 * 1024;
const ZSTD_LEVEL: i32 = 3;

struct Processed {
    record: CrateRecord,
    /// One signature per file in `record.files`; `None` for files too short to compare.
    sigs: Vec<Option<Signature>>,
}

/// Sharded set of content hashes for exact deduplication across worker threads.
struct ExactSet {
    shards: Vec<Mutex<HashSet<u64>>>,
}

impl ExactSet {
    fn new() -> ExactSet {
        ExactSet {
            shards: (0..256).map(|_| Mutex::new(HashSet::new())).collect(),
        }
    }

    /// Returns true if the hash was already present.
    fn insert(&self, hash: u64) -> bool {
        !self.shards[(hash & 0xff) as usize]
            .lock()
            .unwrap()
            .insert(hash)
    }
}

fn main() -> Result<()> {
    match Command::parse() {
        Command::Build(args) => build(args),
        Command::Inspect(args) => inspect(args),
    }
}

fn inspect(args: InspectArgs) -> Result<()> {
    // Piping into `head` closes stdout early; that is not an error worth reporting.
    match inspect_inner(args) {
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe) =>
        {
            Ok(())
        }
        other => other,
    }
}

fn inspect_inner(args: InspectArgs) -> Result<()> {
    let path = expand_home(&args.shard);
    let mut out = BufWriter::new(std::io::stdout().lock());
    let mut shown = 0usize;
    for record in read_shard(&path)? {
        let record = record?;
        match &args.krate {
            None => {
                let bytes: usize = record.files.iter().map(|f| f.content.len()).sum();
                writeln!(
                    out,
                    "{:<40} {:<14} {:<5} {:>5} files {:>10} bytes  {}",
                    record.name,
                    record.version,
                    record.edition,
                    record.files.len(),
                    bytes,
                    record.license.as_deref().unwrap_or("-")
                )?;
                shown += 1;
                if args.limit.is_some_and(|l| shown >= l) {
                    break;
                }
            }
            Some(name) if *name == record.name => {
                if let Some(file) = &args.file {
                    match record.files.iter().find(|f| f.path == *file) {
                        Some(f) => write!(out, "{}", f.content)?,
                        None => bail!("{name} has no file {file}"),
                    }
                } else {
                    writeln!(
                        out,
                        "{} {} edition={} license={} deps={}",
                        record.name,
                        record.version,
                        record.edition,
                        record.license.as_deref().unwrap_or("-"),
                        record.deps.join(",")
                    )?;
                    for f in &record.files {
                        writeln!(out, "  {:<60} {:>8} bytes", f.path, f.content.len())?;
                    }
                }
                out.flush()?;
                return Ok(());
            }
            Some(_) => {}
        }
    }
    out.flush()?;
    if let Some(name) = &args.krate {
        bail!("crate {name} not found in {}", path.display());
    }
    Ok(())
}

fn build(args: BuildArgs) -> Result<()> {
    let mirror = expand_home(&args.mirror);
    let out = expand_home(&args.out);
    if let Some(jobs) = args.jobs {
        rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build_global()?;
    }
    fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;

    let started = Instant::now();
    eprintln!("reading index...");
    let mut crates = index::load(&mirror.join("crates.io-index"))?;
    if let Some(n) = args.sample.filter(|&n| n > 1) {
        crates = crates.into_iter().step_by(n).collect();
    }
    eprintln!(
        "{} crates to process ({:.1}s)",
        crates.len(),
        started.elapsed().as_secs_f64()
    );

    let stats = Stats::default();
    stats
        .crates_in_index
        .store(crates.len() as u64, Ordering::Relaxed);
    let exact = ExactSet::new();
    let reject_log = RejectLog::open(args.reject_log.as_deref())?;
    let progress = AtomicU64::new(0);

    // Pass 1: filter every crate in parallel and stream the survivors into staging shards.
    let (tx, rx) = mpsc::sync_channel::<Processed>(32);
    let writer_out = out.clone();
    let writer = std::thread::spawn(move || -> Result<Vec<Option<Signature>>> {
        let mut shards = ShardWriter::new(&writer_out, "staging");
        let mut sigs = Vec::new();
        for processed in rx {
            shards.write(&processed.record)?;
            sigs.extend(processed.sigs);
        }
        shards.finish()?;
        Ok(sigs)
    });
    let total = crates.len() as u64;
    crates.par_iter().for_each_with(tx, |tx, entry| {
        if let Some(processed) = process_crate(
            entry,
            &mirror,
            args.licenses,
            &stats,
            &exact,
            reject_log.as_ref(),
        ) {
            tx.send(processed).expect("writer thread stopped");
        }
        let done = progress.fetch_add(1, Ordering::Relaxed) + 1;
        if done.is_multiple_of(10000) || done == total {
            eprintln!(
                "  {done}/{total} crates ({:.0}s)",
                started.elapsed().as_secs_f64()
            );
        }
    });
    let sigs = writer.join().expect("writer thread panicked")?;
    drop(exact);
    eprintln!(
        "pass 1 done: {} candidate files ({:.0}s)",
        sigs.len(),
        started.elapsed().as_secs_f64()
    );

    // Pass 2: cluster near-duplicates and rewrite the staging shards without them.
    let dropped = if args.skip_near_dedup {
        vec![false; sigs.len()]
    } else {
        let clusters = minhash::cluster(&sigs, args.near_dup_threshold);
        eprintln!(
            "near-dedup: {} candidate pairs, {} confirmed, {} files dropped ({:.0}s)",
            clusters.candidate_pairs,
            clusters.confirmed_pairs,
            clusters.dropped.iter().filter(|&&d| d).count(),
            started.elapsed().as_secs_f64()
        );
        clusters.dropped
    };
    drop(sigs);
    rewrite(&out, &dropped, &stats)?;

    let snapshot = stats.snapshot();
    snapshot.print();
    let stats_path = out.join("stats.json");
    fs::write(&stats_path, serde_json::to_string_pretty(&snapshot)?)?;
    eprintln!(
        "wrote {} ({:.0}s total)",
        stats_path.display(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Reads one crate's tarball and applies the crate-level and file-level filters.
fn process_crate(
    entry: &CrateEntry,
    mirror: &Path,
    policy: Policy,
    stats: &Stats,
    exact: &ExactSet,
    reject_log: Option<&RejectLog>,
) -> Option<Processed> {
    let bump = |counter: &AtomicU64| {
        counter.fetch_add(1, Ordering::Relaxed);
    };
    if entry.versions.is_empty() {
        bump(&stats.crates_no_versions);
        return None;
    }
    let Some((version, path)) = entry.select(mirror) else {
        bump(&stats.crates_missing_on_disk);
        return None;
    };

    let (manifest, raw_files) = match read_tarball(&path, stats) {
        Ok(contents) => contents,
        Err(_) => {
            bump(&stats.crates_tar_error);
            return None;
        }
    };

    let (license, edition) = match manifest.as_deref().map(parse_manifest) {
        Some(Some(parsed)) => parsed,
        _ => {
            bump(&stats.crates_manifest_error);
            (None, "2015".to_owned())
        }
    };
    if !license::allowed(license.as_deref(), policy) {
        match &license {
            None => bump(&stats.crates_no_license),
            Some(l) => {
                bump(&stats.crates_license_rejected);
                stats.count(&stats.rejected_licenses, l);
            }
        }
        return None;
    }
    if raw_files.is_empty() {
        bump(&stats.crates_no_rust_files);
        return None;
    }

    let mut files = Vec::new();
    let mut sigs = Vec::new();
    for (path, bytes) in raw_files {
        bump(&stats.files_seen);
        stats
            .bytes_seen
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        let Some(content) = filter::normalize(&bytes) else {
            stats.reject(filter::Reject::NotUtf8);
            if let Some(log) = reject_log {
                log.log(&entry.name, &path, "not_utf8", "");
            }
            continue;
        };
        if let Err(reason) = filter::check(&content) {
            stats.reject(reason);
            if let Some(log) = reject_log {
                let detail = match reason {
                    filter::Reject::Generated => filter::generated_marker(&content).unwrap_or(""),
                    _ => "",
                };
                log.log(&entry.name, &path, reason.name(), detail);
            }
            continue;
        }
        if exact.insert(XxHash3_64::oneshot(content.as_bytes())) {
            bump(&stats.files_exact_dup);
            if let Some(log) = reject_log {
                log.log(&entry.name, &path, "exact_dup", "");
            }
            continue;
        }
        sigs.push(minhash::signature(&content));
        files.push(FileRecord { path, content });
    }
    if files.is_empty() {
        bump(&stats.crates_no_rust_files);
        return None;
    }

    Some(Processed {
        record: CrateRecord {
            name: entry.name.clone(),
            version: version.version.to_string(),
            edition,
            license,
            pubtime: version.pubtime.clone(),
            deps: version.deps.clone(),
            files,
        },
        sigs,
    })
}

/// Relative path and raw bytes of a source file inside a tarball.
type RawFile = (String, Vec<u8>);

/// Streams a `.crate` tarball, returning the manifest text and every candidate `.rs` file.
fn read_tarball(path: &Path, stats: &Stats) -> Result<(Option<String>, Vec<RawFile>)> {
    let file = File::open(path)?;
    let mut archive = tar::Archive::new(GzDecoder::new(BufReader::with_capacity(1 << 16, file)));
    let mut manifest = None;
    let mut files = Vec::new();
    let mut total_bytes = 0u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let full = entry.path()?.to_string_lossy().into_owned();
        // Strip the leading `name-version/` directory.
        let Some((_, rel)) = full.split_once('/') else {
            continue;
        };
        if rel == "Cargo.toml" {
            let mut text = String::new();
            entry.read_to_string(&mut text)?;
            manifest = Some(text);
            continue;
        }
        if !rel.ends_with(".rs") {
            continue;
        }
        if filter::excluded_path(rel) {
            stats.files_excluded_path.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let size = entry.header().size()?;
        if size > filter::MAX_FILE_BYTES {
            stats.files_oversized_entry.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        if total_bytes + size > MAX_CRATE_BYTES || files.len() >= MAX_CRATE_FILES {
            stats.crates_truncated.fetch_add(1, Ordering::Relaxed);
            break;
        }
        let mut bytes = Vec::with_capacity(size as usize);
        entry.read_to_end(&mut bytes)?;
        total_bytes += size;
        files.push((rel.to_owned(), bytes));
    }
    Ok((manifest, files))
}

/// Extracts the license expression and edition from a normalized Cargo manifest.
fn parse_manifest(text: &str) -> Option<(Option<String>, String)> {
    let value: toml::Value = toml::from_str(text).ok()?;
    let package = value.get("package")?;
    let license = package
        .get("license")
        .and_then(|l| l.as_str())
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty());
    let edition = package
        .get("edition")
        .and_then(|e| e.as_str())
        .unwrap_or("2015")
        .to_owned();
    Some((license, edition))
}

/// Writes crate records as JSON lines into zstd-compressed shards of bounded size.
struct ShardWriter {
    dir: PathBuf,
    prefix: &'static str,
    index: usize,
    current: Option<zstd::stream::write::Encoder<'static, BufWriter<File>>>,
    written: u64,
    paths: Vec<PathBuf>,
}

impl ShardWriter {
    fn new(dir: &Path, prefix: &'static str) -> ShardWriter {
        ShardWriter {
            dir: dir.to_path_buf(),
            prefix,
            index: 0,
            current: None,
            written: 0,
            paths: Vec::new(),
        }
    }

    fn write(&mut self, record: &CrateRecord) -> Result<()> {
        if self.current.is_none() {
            let path = self
                .dir
                .join(format!("{}-{:03}.jsonl.zst", self.prefix, self.index));
            let file =
                File::create(&path).with_context(|| format!("creating {}", path.display()))?;
            self.current = Some(zstd::stream::write::Encoder::new(
                BufWriter::new(file),
                ZSTD_LEVEL,
            )?);
            self.paths.push(path);
            self.index += 1;
            self.written = 0;
        }
        let encoder = self.current.as_mut().unwrap();
        let line = serde_json::to_vec(record)?;
        encoder.write_all(&line)?;
        encoder.write_all(b"\n")?;
        self.written += line.len() as u64 + 1;
        if self.written >= SHARD_BYTES {
            self.rotate()?;
        }
        Ok(())
    }

    fn rotate(&mut self) -> Result<()> {
        if let Some(encoder) = self.current.take() {
            encoder.finish()?.flush()?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<PathBuf>> {
        self.rotate()?;
        Ok(self.paths)
    }
}

/// Rewrites the staging shards into final shards, dropping near-duplicate files and
/// collecting the statistics of what survives.
fn rewrite(out: &Path, dropped: &[bool], stats: &Stats) -> Result<()> {
    let mut staging: Vec<PathBuf> = fs::read_dir(out)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("staging-") && n.ends_with(".jsonl.zst"))
        })
        .collect();
    staging.sort();
    let mut writer = ShardWriter::new(out, "shard");
    let mut next_id = 0usize;
    for path in &staging {
        let reader = BufReader::new(zstd::stream::read::Decoder::new(File::open(path)?)?);
        for line in reader.lines() {
            let line = line?;
            let mut record: CrateRecord = serde_json::from_str(&line)?;
            let mut kept = Vec::with_capacity(record.files.len());
            for file in record.files.drain(..) {
                let id = next_id;
                next_id += 1;
                if *dropped.get(id).unwrap_or(&false) {
                    stats.files_near_dup.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                stats.files_kept.fetch_add(1, Ordering::Relaxed);
                stats
                    .bytes_kept
                    .fetch_add(file.content.len() as u64, Ordering::Relaxed);
                stats
                    .lines_kept
                    .fetch_add(file.content.lines().count() as u64, Ordering::Relaxed);
                stats.tokens_kept.fetch_add(
                    minhash::token_count(&file.content) as u64,
                    Ordering::Relaxed,
                );
                kept.push(file);
            }
            if kept.is_empty() {
                continue;
            }
            record.files = kept;
            stats.crates_kept.fetch_add(1, Ordering::Relaxed);
            stats.count(&stats.editions, &record.edition);
            writer.write(&record)?;
        }
    }
    if next_id != dropped.len() {
        bail!(
            "staging shards held {next_id} files but {} were expected",
            dropped.len()
        );
    }
    let shards = writer.finish()?;
    for path in staging {
        fs::remove_file(path)?;
    }
    eprintln!("wrote {} shards", shards.len());
    Ok(())
}
