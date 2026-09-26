//! Builds per-language corpora from a Debian or Ubuntu archive mirror, and reports how much
//! code there is in each language after the quality filters and deduplication, and under
//! which licenses.
//!
//! The output directory has one subdirectory per language, holding shards in the Rust
//! corpus's format: one record per source package with that package's files in the language,
//! each with its own license. Directories and files name the language by its
//! `Lang::identifier`. C and C++ are deduplicated together, since they train one model.
//! Files the license policy admits are staged as they are read; once every package is in,
//! exact and near-duplicates are removed and the staging shards are rewritten without them.
//!
//! Every file's license is taken, in order of preference, from an `SPDX-License-Identifier`
//! in its header, from the package's machine-readable `debian/copyright` (lowered to what the
//! file's own header comment says, when that is more restrictive), from its header comment
//! alone, or from the license texts in a free-form `debian/copyright`. Exact duplicates keep
//! the most permissive copy, and near-duplicate clustering visits permissive files first, so
//! the permissive totals are what a permissive-only corpus would keep.

mod copyright;
mod source;

use crate::filter::{self, CommentStyle, Reject};
use crate::lang::{self, Detected, LANGS, LangId};
use crate::license::{self, Policy, Tier};
use crate::minhash::{self, Signature};
use crate::{ShardWriter, rewrite};
use anyhow::{Context, Result};
use clap::Parser;
use copyright::Copyright;
use corpus::{FileRecord, PackageRecord, expand_home};
use rayon::prelude::*;
use serde_json::{Value, json};
use source::{ArchiveKind, COMPONENTS, Package};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Instant;
use twox_hash::XxHash3_64;

#[derive(Parser)]
pub struct DebianArgs {
    /// Root of the archive mirror (contains `pool/`).
    #[arg(long, default_value = "~/apt-mirror/mirror/archive.ubuntu.com/ubuntu")]
    mirror: String,
    /// Output directory: a shard directory per language, the report, statistics and file list.
    #[arg(long, default_value = "~/corpus/debian-packages")]
    out: String,
    /// Which licenses to write to the corpus. The report covers every license regardless.
    #[arg(long, value_enum, default_value_t = Policy::Permissive)]
    licenses: Policy,
    /// Archive components to include.
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "main,restricted,universe,multiverse"
    )]
    components: Vec<String>,
    /// Process only every Nth package, for quick experiments.
    #[arg(long)]
    sample: Option<usize>,
    /// Process only these source packages.
    #[arg(long, value_delimiter = ',')]
    packages: Vec<String>,
    /// Number of worker threads (defaults to the CPU count).
    #[arg(long)]
    jobs: Option<usize>,
    /// Estimated Jaccard similarity at or above which two files are near-duplicates.
    #[arg(long, default_value_t = 0.8)]
    near_dup_threshold: f64,
    /// Skip near-duplicate detection (exact duplicates are always removed).
    #[arg(long)]
    skip_near_dedup: bool,
    /// Bytes per token for estimating token counts. The default is what the Rust vocabulary
    /// achieves on the validation split of the Rust corpus.
    #[arg(long, default_value_t = 3.826)]
    bytes_per_token: f64,
    /// Training tokens to compare each language against. The default is the 70m shape's plan.
    #[arg(long, default_value_t = 2_000_000_000)]
    token_budget: u64,
}

/// Where a file's license came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Spdx,
    Dep5,
    Header,
    CopyrightText,
    None,
}

impl Source {
    const ALL: [Source; 5] = [
        Source::Spdx,
        Source::Dep5,
        Source::Header,
        Source::CopyrightText,
        Source::None,
    ];

    fn name(self) -> &'static str {
        match self {
            Source::Spdx => "spdx",
            Source::Dep5 => "dep5",
            Source::Header => "header",
            Source::CopyrightText => "copyright_text",
            Source::None => "none",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Per-language counts of the files in a package, before deduplication.
#[derive(Clone, Default)]
struct RawCounts {
    files: u64,
    bytes: u64,
    oversized: u64,
    misidentified: u64,
    rejected: [u64; Reject::ALL.len()],
    passed: Tally,
}

impl RawCounts {
    fn add(&mut self, other: &RawCounts) {
        self.files += other.files;
        self.bytes += other.bytes;
        self.oversized += other.oversized;
        self.misidentified += other.misidentified;
        for (a, b) in self.rejected.iter_mut().zip(&other.rejected) {
            *a += b;
        }
        self.passed.add(&other.passed);
    }
}

#[derive(Clone, Copy, Default)]
struct Tally {
    files: u64,
    bytes: u64,
    lines: u64,
}

impl Tally {
    fn add(&mut self, other: &Tally) {
        self.files += other.files;
        self.bytes += other.bytes;
        self.lines += other.lines;
    }

    fn count(&mut self, bytes: u32, lines: u32) {
        self.files += 1;
        self.bytes += bytes as u64;
        self.lines += lines as u64;
    }

    fn json(&self, bytes_per_token: f64) -> Value {
        json!({
            "files": self.files,
            "bytes": self.bytes,
            "lines": self.lines,
            "tokens": (self.bytes as f64 / bytes_per_token).round() as u64,
        })
    }
}

/// A file that passed the quality filters, before its license is known.
struct Candidate {
    /// A `LANGS` index, or `LANGS.len()` for a `.h` header.
    slot: usize,
    path: String,
    content: String,
}

/// A file that passed the quality filters, as a worker reports it.
struct ScannedFile {
    lang: LangId,
    path: String,
    hash: u64,
    bytes: u32,
    lines: u32,
    sig: Option<Box<Signature>>,
    tier: Tier,
    source: Source,
    license: String,
    /// The content, for files the license policy admits.
    content: Option<String>,
}

struct PackageScan {
    package: u32,
    copyright: copyright::Kind,
    raw: Vec<RawCounts>,
    files: Vec<ScannedFile>,
    unpacked_bytes: u64,
    error: Option<String>,
}

pub fn build(args: DebianArgs) -> Result<()> {
    let mirror = expand_home(&args.mirror);
    let out = expand_home(&args.out);
    if let Some(jobs) = args.jobs {
        rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build_global()?;
    }
    fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
    for l in LANGS {
        remove_shards(&out.join(l.identifier()))?;
    }
    let started = Instant::now();

    let mut packages = source::find_packages(&mirror, &args.components)?;
    if !args.packages.is_empty() {
        packages.retain(|p| args.packages.contains(&p.name));
    }
    if let Some(n) = args.sample.filter(|&n| n > 1) {
        packages = packages.into_iter().step_by(n).collect();
    }
    let compressed: u64 = packages.iter().map(Package::size).sum();
    eprintln!(
        "{} packages, {:.1} GB compressed ({:.1}s)",
        packages.len(),
        compressed as f64 / 1e9,
        started.elapsed().as_secs_f64()
    );

    // Largest packages first, so the longest scans do not start last.
    let mut order: Vec<u32> = (0..packages.len() as u32).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(packages[i as usize].size()));

    let collector = std::thread::scope(|scope| -> Result<Collector> {
        let (tx, rx) = mpsc::sync_channel::<PackageScan>(256);
        let collector = scope.spawn(|| -> Result<Collector> {
            let mut collector = Collector::new(&packages, &out);
            for scan in rx {
                collector.add(scan)?;
            }
            Ok(collector)
        });
        let done = AtomicU64::new(0);
        let read = AtomicU64::new(0);
        let total = packages.len() as u64;
        // `par_bridge` hands packages out in order as threads free up, so the largest start
        // first; splitting the slice would leave most of them to one thread.
        order.iter().par_bridge().for_each_with(tx, |tx, &index| {
            let package = &packages[index as usize];
            let scan = scan_package(package, index, args.licenses);
            read.fetch_add(package.size(), Ordering::Relaxed);
            tx.send(scan).expect("collector stopped");
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_multiple_of(1000) || n == total {
                eprintln!(
                    "  {n}/{total} packages, {:.1}/{:.1} GB ({:.0}s)",
                    read.load(Ordering::Relaxed) as f64 / 1e9,
                    compressed as f64 / 1e9,
                    started.elapsed().as_secs_f64()
                );
            }
        });
        collector.join().expect("collector panicked")
    })?;

    let report = collector.finish(&args, started)?;
    print!("{report}");
    Ok(())
}

/// Reads every archive of a package and reports the files that pass the quality filters,
/// with their licenses.
fn scan_package(package: &Package, index: u32, policy: Policy) -> PackageScan {
    let header_slot = LANGS.len();
    // Archives in parallel: the largest packages have several multi-gigabyte tarballs.
    let archives: Vec<ArchiveScan> = package
        .archives
        .par_iter()
        .map(|archive| scan_archive(package, archive))
        .collect();
    let mut raw = vec![RawCounts::default(); LANGS.len() + 1];
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut copyright_text: Option<String> = None;
    let mut unpacked_bytes = 0;
    let mut error = None;
    for scan in archives {
        for (total, counts) in raw.iter_mut().zip(&scan.raw) {
            total.add(counts);
        }
        candidates.extend(scan.candidates);
        copyright_text = copyright_text.or(scan.copyright_text);
        unpacked_bytes += scan.unpacked_bytes;
        error = error.or(scan.error);
    }

    // A `.h` header is C++ if the package has more C++ than C source.
    let header_lang = if raw[lang::CPP as usize].passed.bytes > raw[lang::C as usize].passed.bytes {
        lang::CPP
    } else {
        lang::C
    };
    let headers = raw.pop().unwrap();
    raw[header_lang as usize].add(&headers);

    let copyright = match &copyright_text {
        Some(text) => Copyright::parse(text),
        None => Copyright::missing(),
    };
    let files = candidates
        .into_par_iter()
        .map(|c| {
            let lang = if c.slot == header_slot {
                header_lang
            } else {
                c.slot as LangId
            };
            let (tier, source, license) = file_license(&copyright, &c.path, &c.content);
            ScannedFile {
                lang,
                hash: XxHash3_64::oneshot(c.content.as_bytes()),
                bytes: c.content.len() as u32,
                lines: c.content.lines().count() as u32,
                sig: minhash::signature(&c.content).map(Box::new),
                tier,
                source,
                license,
                content: policy.admits(tier).then_some(c.content),
                path: c.path,
            }
        })
        .collect();
    PackageScan {
        package: index,
        copyright: copyright.kind(),
        raw,
        files,
        unpacked_bytes,
        error,
    }
}

/// What one archive of a package holds.
struct ArchiveScan {
    /// Indexed like `LANGS`, then one more for `.h` headers.
    raw: Vec<RawCounts>,
    candidates: Vec<Candidate>,
    copyright_text: Option<String>,
    unpacked_bytes: u64,
    error: Option<String>,
}

/// What became of one file in a recognized language, decided while the tarball is read.
enum Outcome {
    Oversized,
    Misidentified,
    Rejected(Reject),
    Passed(String),
}

/// Reads one archive, keeping the files that pass the quality filters and the packaging's
/// `debian/copyright`.
fn scan_archive(package: &Package, archive: &source::Archive) -> ArchiveScan {
    let header_slot = LANGS.len();
    let mut scan = ArchiveScan {
        raw: vec![RawCounts::default(); LANGS.len() + 1],
        candidates: Vec::new(),
        copyright_text: None,
        unpacked_bytes: 0,
        error: None,
    };
    let path = package.dir.join(&archive.file);
    let (strip_top, prefix) = match &archive.kind {
        ArchiveKind::Diff => {
            match source::copyright_from_diff(&path) {
                Ok(text) => scan.copyright_text = text,
                Err(e) => scan.error = Some(format!("{}: {e:#}", archive.file)),
            }
            return scan;
        }
        ArchiveKind::Debian => (false, None),
        ArchiveKind::Orig | ArchiveKind::Native => (true, None),
        ArchiveKind::Component(name) => (true, Some(name.as_str())),
    };
    let packaging = matches!(archive.kind, ArchiveKind::Debian | ArchiveKind::Native);
    // Paths are final only once the whole tarball has shown whether it has a top-level
    // directory, so files are kept under their stored paths until then.
    let mut seen: Vec<(String, usize, u64, Outcome)> = Vec::new();
    let mut copyrights: Vec<(String, String)> = Vec::new();
    let result = source::walk_tar(&path, |stored, size, reader| {
        if packaging
            && (stored == "debian/copyright"
                || stored
                    .strip_suffix("/debian/copyright")
                    .is_some_and(|d| !d.contains('/')))
        {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes)?;
            copyrights.push((
                stored.to_owned(),
                String::from_utf8_lossy(&bytes).into_owned(),
            ));
            return Ok(());
        }
        if stored.split('/').any(|c| c.starts_with('.')) {
            return Ok(());
        }
        let Some(detected) = lang::detect(stored) else {
            return Ok(());
        };
        let slot = match detected {
            Detected::Lang(id) => id as usize,
            Detected::Header => header_slot,
        };
        let outcome = if size > filter::MAX_FILE_BYTES {
            Outcome::Oversized
        } else {
            let mut bytes = Vec::with_capacity(size as usize);
            reader.read_to_end(&mut bytes)?;
            match filter::normalize(&bytes) {
                None => Outcome::Rejected(Reject::NotUtf8),
                Some(content) => match detected {
                    Detected::Lang(id) if lang::misidentified(id, &content) => {
                        Outcome::Misidentified
                    }
                    _ => {
                        let style: &CommentStyle = match detected {
                            Detected::Lang(id) => LANGS[id as usize].comments,
                            Detected::Header => &CommentStyle::C,
                        };
                        match filter::check_with(&content, style) {
                            Ok(()) => Outcome::Passed(content),
                            Err(reason) => Outcome::Rejected(reason),
                        }
                    }
                },
            }
        };
        seen.push((stored.to_owned(), slot, size, outcome));
        Ok(())
    });
    let top = match result {
        Ok((bytes, top)) => {
            scan.unpacked_bytes = bytes;
            top.filter(|_| strip_top)
        }
        Err(e) => {
            // Without the whole tarball the paths are uncertain; it contributes nothing.
            scan.error = Some(format!("{}: {e:#}", archive.file));
            return scan;
        }
    };
    for (stored, text) in copyrights {
        if source::source_path(&stored, top.as_deref(), None) == "debian/copyright" {
            scan.copyright_text = Some(text);
        }
    }
    for (stored, slot, size, outcome) in seen {
        let path = source::source_path(&stored, top.as_deref(), prefix);
        if excluded(&path) {
            continue;
        }
        let counts = &mut scan.raw[slot];
        counts.files += 1;
        counts.bytes += size;
        match outcome {
            Outcome::Oversized => counts.oversized += 1,
            Outcome::Misidentified => counts.misidentified += 1,
            Outcome::Rejected(reason) => counts.rejected[reason.index()] += 1,
            Outcome::Passed(content) => {
                counts
                    .passed
                    .count(content.len() as u32, content.lines().count() as u32);
                scan.candidates.push(Candidate {
                    slot,
                    path,
                    content,
                });
            }
        }
    }
    scan
}

/// Paths that are not upstream source: hidden files and directories (`.git`, the `.pc`
/// directory of quilt patches), and the packaging.
fn excluded(path: &str) -> bool {
    path.starts_with("debian/") || path.split('/').any(|c| c.starts_with('.'))
}

/// Bytes at the start of a file searched for a license header.
const HEADER_BYTES: usize = 4096;

/// Decides a file's license tier, where that came from, and the license's name.
fn file_license(copyright: &Copyright, path: &str, content: &str) -> (Tier, Source, String) {
    if let Some(expr) = license::spdx_identifier(content)
        && let Some(tier) = license::evaluate(expr, license::tier_of)
    {
        return (tier, Source::Spdx, expr.to_owned());
    }
    let header = license::from_header(&content[..content.floor_char_boundary(HEADER_BYTES)]);
    match (header, copyright.lookup(path)) {
        (Some((ht, hname)), Some((tier, name))) if copyright.kind() == copyright::Kind::Dep5 => {
            if ht < tier {
                (ht, Source::Header, hname)
            } else {
                (tier, Source::Dep5, name.to_owned())
            }
        }
        (Some((ht, hname)), _) => (ht, Source::Header, hname),
        (None, Some((tier, name))) => {
            let source = match copyright.kind() {
                copyright::Kind::Dep5 => Source::Dep5,
                _ => Source::CopyrightText,
            };
            (tier, source, name.to_owned())
        }
        (None, None) => (Tier::Other, Source::None, "none".to_owned()),
    }
}

/// A unique file, kept until near-duplicate detection.
struct Entry {
    hash: u64,
    bytes: u32,
    lines: u32,
    /// Of the copy kept. In a deduplication group of several languages, copies can differ.
    lang: LangId,
    tier: Tier,
    source: Source,
    license: u32,
    package: u32,
    path: Box<str>,
    /// The kept copy's id in its language's staging shards, if the policy admits it.
    staged: Option<u32>,
    sig: Option<Box<Signature>>,
}

impl Entry {
    /// Which of several copies of the same content to keep; smaller is better. Permissive
    /// copies win, then the most supported component, then a fixed order.
    fn rank(&self, packages: &[Package]) -> (std::cmp::Reverse<Tier>, u8, u32, &str) {
        (
            std::cmp::Reverse(self.tier),
            packages[self.package as usize].component,
            self.package,
            &self.path,
        )
    }
}

#[derive(Default)]
struct LangData {
    raw: RawCounts,
    exact_dups: Tally,
    staging: Option<ShardWriter>,
    /// Files written to the staging shards so far.
    staged: u32,
}

/// Files deduplicated together, indexed by the group's `lang::dedup_group`.
#[derive(Default)]
struct Group {
    entries: Vec<Entry>,
    by_hash: HashMap<u64, u32>,
}

struct Collector<'a> {
    packages: &'a [Package],
    out: &'a Path,
    langs: Vec<LangData>,
    groups: Vec<Group>,
    licenses: Vec<String>,
    license_ids: HashMap<String, u32>,
    copyright_kinds: [u64; 3],
    unpacked_bytes: u64,
    errors: Vec<String>,
}

impl<'a> Collector<'a> {
    fn new(packages: &'a [Package], out: &'a Path) -> Collector<'a> {
        Collector {
            packages,
            out,
            langs: LANGS.iter().map(|_| LangData::default()).collect(),
            groups: LANGS.iter().map(|_| Group::default()).collect(),
            licenses: Vec::new(),
            license_ids: HashMap::new(),
            copyright_kinds: [0; 3],
            unpacked_bytes: 0,
            errors: Vec::new(),
        }
    }

    fn add(&mut self, scan: PackageScan) -> Result<()> {
        let packages = self.packages;
        let package = &packages[scan.package as usize];
        self.copyright_kinds[match scan.copyright {
            copyright::Kind::Dep5 => 0,
            copyright::Kind::FreeForm => 1,
            copyright::Kind::Missing => 2,
        }] += 1;
        self.unpacked_bytes += scan.unpacked_bytes;
        if let Some(e) = scan.error {
            self.errors.push(format!("{}: {e}", package.name));
        }
        for (data, raw) in self.langs.iter_mut().zip(&scan.raw) {
            data.raw.add(raw);
        }
        // Files to stage, by language, in the order of their staging ids.
        let mut staged: Vec<Vec<FileRecord>> = LANGS.iter().map(|_| Vec::new()).collect();
        for file in scan.files {
            let license = match self.license_ids.get(&file.license) {
                Some(&id) => id,
                None => {
                    let id = self.licenses.len() as u32;
                    self.licenses.push(file.license.clone());
                    self.license_ids.insert(file.license, id);
                    id
                }
            };
            let mut entry = Entry {
                hash: file.hash,
                bytes: file.bytes,
                lines: file.lines,
                lang: file.lang,
                tier: file.tier,
                source: file.source,
                license,
                package: scan.package,
                path: file.path.into_boxed_str(),
                staged: None,
                sig: file.sig,
            };
            let group = &mut self.groups[lang::dedup_group(file.lang) as usize];
            let existing = group.by_hash.get(&entry.hash).copied();
            if let Some(i) = existing {
                self.langs[file.lang as usize]
                    .exact_dups
                    .count(entry.bytes, entry.lines);
                // Only a copy that would replace the kept one is worth staging.
                if entry.rank(packages) >= group.entries[i as usize].rank(packages) {
                    continue;
                }
            }
            if let Some(content) = file.content {
                let data = &mut self.langs[file.lang as usize];
                entry.staged = Some(data.staged);
                data.staged += 1;
                staged[file.lang as usize].push(FileRecord {
                    path: entry.path.to_string(),
                    language: LANGS[file.lang as usize].identifier(),
                    content,
                    license: Some(self.licenses[license as usize].clone()),
                });
            }
            match existing {
                Some(i) => {
                    let kept = &mut group.entries[i as usize];
                    kept.lang = entry.lang;
                    kept.tier = entry.tier;
                    kept.source = entry.source;
                    kept.license = entry.license;
                    kept.package = entry.package;
                    kept.path = entry.path;
                    kept.staged = entry.staged;
                }
                None => {
                    group.by_hash.insert(entry.hash, group.entries.len() as u32);
                    group.entries.push(entry);
                }
            }
        }
        for (lang, files) in staged.into_iter().enumerate() {
            if files.is_empty() {
                continue;
            }
            let data = &mut self.langs[lang];
            let writer = match &mut data.staging {
                Some(w) => w,
                None => {
                    let dir = self.out.join(LANGS[lang].identifier());
                    fs::create_dir_all(&dir)?;
                    data.staging.insert(ShardWriter::new(&dir, "staging"))
                }
            };
            writer.write(&PackageRecord {
                name: package.name.clone(),
                version: package.version.clone(),
                edition: String::new(),
                license: None,
                pubtime: None,
                deps: Vec::new(),
                files,
            })?;
        }
        Ok(())
    }

    /// Removes near-duplicates, writes the corpus shards, the file list and statistics, and
    /// returns the report.
    fn finish(mut self, args: &DebianArgs, started: Instant) -> Result<String> {
        let packages = self.packages;
        let out = self.out;
        let files_path = out.join("files.tsv.zst");
        let mut files_out = zstd::stream::write::Encoder::new(
            BufWriter::new(
                File::create(&files_path)
                    .with_context(|| format!("creating {}", files_path.display()))?,
            ),
            3,
        )?;
        writeln!(
            files_out,
            "language\tpackage\tcomponent\tpath\tbytes\tlines\ttier\tlicense_source\tlicense\tnear_dup"
        )?;

        let mut results: Vec<LangResult> = LANGS.iter().map(|_| LangResult::default()).collect();
        // Per language, whether each staged file is in the corpus.
        let mut keep: Vec<Vec<bool>> = self
            .langs
            .iter()
            .map(|d| vec![false; d.staged as usize])
            .collect();
        for (group_lang, group) in self.groups.iter_mut().enumerate() {
            group.by_hash = HashMap::new();
            if group.entries.is_empty() {
                continue;
            }
            // Permissive files first: clustering keeps the earliest file of each cluster.
            group
                .entries
                .sort_by(|a, b| a.rank(packages).cmp(&b.rank(packages)));
            let dropped = if args.skip_near_dedup {
                vec![false; group.entries.len()]
            } else {
                let sigs: Vec<Option<Signature>> = group
                    .entries
                    .iter_mut()
                    .map(|e| e.sig.take().map(|s| *s))
                    .collect();
                minhash::cluster(&sigs, args.near_dup_threshold).dropped
            };

            for (entry, &dropped) in group.entries.iter().zip(&dropped) {
                let package = &packages[entry.package as usize];
                writeln!(
                    files_out,
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    LANGS[entry.lang as usize].name,
                    package.name,
                    COMPONENTS[package.component as usize],
                    entry.path.replace(['\t', '\n'], " "),
                    entry.bytes,
                    entry.lines,
                    entry.tier.name(),
                    entry.source.name(),
                    self.licenses[entry.license as usize].replace(['\t', '\n'], " "),
                    dropped as u8
                )?;
                let result = &mut results[entry.lang as usize];
                if dropped {
                    result.near_dups.count(entry.bytes, entry.lines);
                    continue;
                }
                if let Some(id) = entry.staged {
                    keep[entry.lang as usize][id as usize] = true;
                }
                let t = tier_index(entry.tier);
                result.kept[t].count(entry.bytes, entry.lines);
                result.sources[t][entry.source.index()] += entry.bytes as u64;
                result.components[package.component as usize][t] += entry.bytes as u64;
                result.packages.entry(entry.package).or_insert([0; 4])[t] += entry.bytes as u64;
                *result.licenses.entry((entry.license, t)).or_default() += entry.bytes as u64;
            }
            group.entries = Vec::new();
            eprintln!(
                "  deduplicated {} ({:.0}s)",
                LANGS[group_lang].name,
                started.elapsed().as_secs_f64()
            );
        }
        files_out.finish()?.flush()?;

        // Rewrite each language's staging shards without the duplicates.
        for (lang, data) in self.langs.iter_mut().enumerate() {
            let Some(writer) = data.staging.take() else {
                continue;
            };
            writer.finish()?;
            let result = &mut results[lang];
            let dropped: Vec<bool> = keep[lang].iter().map(|&k| !k).collect();
            let dir = out.join(LANGS[lang].identifier());
            result.shards = rewrite(&dir, &dropped, |record, _| {
                if record.files.is_empty() {
                    return;
                }
                result.records += 1;
                for file in &record.files {
                    result.written.count(
                        file.content.len() as u32,
                        file.content.lines().count() as u32,
                    );
                }
            })?;
            // Every kept file the policy admits was staged, so the two counts must agree.
            let admitted = Tier::ALL
                .iter()
                .enumerate()
                .filter(|&(_, &tier)| args.licenses.admits(tier))
                .map(|(t, _)| result.kept[t].files)
                .sum::<u64>();
            if result.written.files != admitted {
                anyhow::bail!(
                    "{}: wrote {} files but {admitted} were kept",
                    LANGS[lang].name,
                    result.written.files
                );
            }
        }

        let mut results: Vec<(LangId, RawCounts, Tally, LangResult)> = results
            .into_iter()
            .zip(&mut self.langs)
            .enumerate()
            .filter(|(_, (_, data))| data.raw.files > 0)
            .map(|(lang, (r, data))| {
                (
                    lang as LangId,
                    std::mem::take(&mut data.raw),
                    data.exact_dups,
                    r,
                )
            })
            .collect();
        let kept_total = |r: &LangResult| r.kept.iter().map(|t| t.bytes).sum::<u64>();
        results.sort_by_key(|(_, _, _, r)| std::cmp::Reverse(kept_total(r)));

        let stats = self.stats_json(args, &results, started);
        fs::write(
            out.join("stats.json"),
            serde_json::to_string_pretty(&stats)?,
        )?;
        let report = self.report(args, &results, started);
        fs::write(out.join("report.txt"), &report)?;
        eprintln!(
            "wrote {}, stats.json and report.txt ({:.0}s)",
            files_path.display(),
            started.elapsed().as_secs_f64()
        );
        Ok(report)
    }

    fn stats_json(
        &self,
        args: &DebianArgs,
        results: &[(LangId, RawCounts, Tally, LangResult)],
        started: Instant,
    ) -> Value {
        let bpt = args.bytes_per_token;
        let tokens = |bytes: u64| (bytes as f64 / bpt).round() as u64;
        let mut components = [0u64; 4];
        for p in self.packages {
            components[p.component as usize] += 1;
        }
        let languages: Vec<Value> = results
            .iter()
            .map(|(lang, raw, exact_dups, r)| {
                let by_tier = |f: &dyn Fn(usize) -> Value| -> Value {
                    Tier::ALL
                        .iter()
                        .enumerate()
                        .map(|(t, tier)| (tier.name().to_owned(), f(t)))
                        .collect::<serde_json::Map<_, _>>()
                        .into()
                };
                let mut top_packages: Vec<(&u32, &[u64; 4])> = r.packages.iter().collect();
                top_packages.sort_by_key(|(_, b)| std::cmp::Reverse(b.iter().sum::<u64>()));
                let mut top_permissive = top_packages.clone();
                top_permissive.sort_by_key(|(_, b)| std::cmp::Reverse(b[0]));
                let package_json = |list: &[(&u32, &[u64; 4])]| -> Vec<Value> {
                    list.iter()
                        .take(50)
                        .map(|(p, b)| {
                            json!({
                                "package": self.packages[**p as usize].name,
                                "tokens": tokens(b.iter().sum()),
                                "permissive_tokens": tokens(b[0]),
                            })
                        })
                        .collect()
                };
                let mut licenses: Vec<(&(u32, usize), &u64)> = r.licenses.iter().collect();
                licenses.sort_by_key(|(_, b)| std::cmp::Reverse(**b));
                json!({
                    "language": LANGS[*lang as usize].name,
                    "raw": {
                        "files": raw.files,
                        "bytes": raw.bytes,
                        "oversized": raw.oversized,
                        "misidentified": raw.misidentified,
                        "rejected": Reject::ALL.iter().map(|r| (r.name().to_owned(), json!(raw.rejected[r.index()]))).collect::<serde_json::Map<_, _>>(),
                    },
                    "passed_filters": raw.passed.json(bpt),
                    "exact_dups": exact_dups.json(bpt),
                    "near_dups": r.near_dups.json(bpt),
                    "kept": by_tier(&|t| r.kept[t].json(bpt)),
                    "kept_tokens_by_source": by_tier(&|t| Source::ALL.iter().map(|s| (s.name().to_owned(), json!(tokens(r.sources[t][s.index()])))).collect::<serde_json::Map<_, _>>().into()),
                    "kept_tokens_by_component": COMPONENTS.iter().enumerate().map(|(c, name)| (name.to_string(), by_tier(&|t| json!(tokens(r.components[c][t]))))).collect::<serde_json::Map<_, _>>(),
                    "corpus": {
                        "directory": LANGS[*lang as usize].identifier(),
                        "records": r.records,
                        "shards": r.shards,
                        "written": r.written.json(bpt),
                    },
                    "top_packages": package_json(&top_packages),
                    "top_permissive_packages": package_json(&top_permissive),
                    "top_licenses": licenses.iter().take(100).map(|((l, t), b)| json!({
                        "license": self.licenses[*l as usize],
                        "tier": Tier::ALL[*t].name(),
                        "tokens": tokens(**b),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        json!({
            "mirror": args.mirror,
            "licenses": format!("{:?}", args.licenses).to_lowercase(),
            "bytes_per_token": bpt,
            "token_budget": args.token_budget,
            "near_dup_threshold": if args.skip_near_dedup { None } else { Some(args.near_dup_threshold) },
            "sample": args.sample,
            "packages": {
                "total": self.packages.len(),
                "by_component": COMPONENTS.iter().zip(components).map(|(n, c)| (n.to_string(), json!(c))).collect::<serde_json::Map<_, _>>(),
                "copyright_dep5": self.copyright_kinds[0],
                "copyright_free_form": self.copyright_kinds[1],
                "copyright_missing": self.copyright_kinds[2],
                "errors": self.errors,
            },
            "unpacked_bytes": self.unpacked_bytes,
            "seconds": started.elapsed().as_secs(),
            "languages": languages,
        })
    }

    fn report(
        &self,
        args: &DebianArgs,
        results: &[(LangId, RawCounts, Tally, LangResult)],
        started: Instant,
    ) -> String {
        let bpt = args.bytes_per_token;
        let tokens = |bytes: u64| bytes as f64 / bpt;
        let mut s = String::new();
        let mut components = [0u64; 4];
        for p in self.packages {
            components[p.component as usize] += 1;
        }
        let _ = writeln!(
            s,
            "{} source packages ({}), {:.0} GB unpacked, {:.0} s",
            self.packages.len(),
            COMPONENTS
                .iter()
                .zip(components)
                .map(|(n, c)| format!("{n} {c}"))
                .collect::<Vec<_>>()
                .join(", "),
            self.unpacked_bytes as f64 / 1e9,
            started.elapsed().as_secs_f64()
        );
        let _ = writeln!(
            s,
            "debian/copyright: {} machine-readable, {} free-form, {} missing; {} packages with read errors",
            self.copyright_kinds[0],
            self.copyright_kinds[1],
            self.copyright_kinds[2],
            self.errors.len()
        );
        let _ = writeln!(
            s,
            "tokens estimated at {bpt} bytes/token; budget {} tokens\n",
            human(args.token_budget as f64)
        );

        let _ = writeln!(s, "Volume, all licenses (bytes after normalization)");
        let _ = writeln!(
            s,
            "{:<12}{:>10}{:>10}{:>10}{:>10}{:>10}{:>11}{:>10}{:>10}",
            "language",
            "files",
            "raw",
            "filtered",
            "unique",
            "near-uniq",
            "kept files",
            "lines",
            "tokens"
        );
        for (lang, raw, exact, r) in results {
            let kept = r.kept.iter().fold(Tally::default(), |mut a, t| {
                a.add(t);
                a
            });
            let unique = raw.passed.bytes - exact.bytes;
            let _ = writeln!(
                s,
                "{:<12}{:>10}{:>10}{:>10}{:>10}{:>10}{:>11}{:>10}{:>10}",
                LANGS[*lang as usize].name,
                human(raw.files as f64),
                bytes(raw.bytes),
                bytes(raw.passed.bytes),
                bytes(unique),
                bytes(kept.bytes),
                human(kept.files as f64),
                human(kept.lines as f64),
                human(tokens(kept.bytes)),
            );
        }

        let _ = writeln!(s, "\nEstimated tokens after deduplication, by license tier");
        let _ = writeln!(
            s,
            "{:<12}{:>11}{:>11}{:>11}{:>11}{:>9}{:>12}{:>9}",
            "language",
            "permissive",
            "weak-copy",
            "copyleft",
            "other",
            "perm %",
            "perm lines",
            "budget"
        );
        for (lang, _, _, r) in results {
            let total: u64 = r.kept.iter().map(|t| t.bytes).sum();
            let perm = r.kept[0].bytes;
            let _ = writeln!(
                s,
                "{:<12}{:>11}{:>11}{:>11}{:>11}{:>8.0}%{:>12}{:>8.2}x",
                LANGS[*lang as usize].name,
                human(tokens(perm)),
                human(tokens(r.kept[1].bytes)),
                human(tokens(r.kept[2].bytes)),
                human(tokens(r.kept[3].bytes)),
                100.0 * perm as f64 / total.max(1) as f64,
                human(r.kept[0].lines as f64),
                tokens(perm) / args.token_budget as f64,
            );
        }

        let _ = writeln!(
            s,
            "\nCorpus written to {} ({} licenses)",
            self.out.display(),
            format!("{:?}", args.licenses).to_lowercase()
        );
        let _ = writeln!(
            s,
            "{:<12}{:<13}{:>10}{:>10}{:>10}{:>10}{:>10}{:>8}",
            "language", "directory", "packages", "files", "lines", "bytes", "tokens", "shards"
        );
        for (lang, _, _, r) in results {
            if r.written.files == 0 {
                continue;
            }
            let lang = &LANGS[*lang as usize];
            let _ = writeln!(
                s,
                "{:<12}{:<13}{:>10}{:>10}{:>10}{:>10}{:>10}{:>8}",
                lang.name,
                lang.identifier(),
                human(r.records as f64),
                human(r.written.files as f64),
                human(r.written.lines as f64),
                bytes(r.written.bytes),
                human(tokens(r.written.bytes)),
                r.shards,
            );
        }

        let _ = writeln!(
            s,
            "\nWhere the permissive files' licenses came from (share of tokens)"
        );
        let _ = write!(s, "{:<12}", "language");
        for src in Source::ALL {
            let _ = write!(s, "{:>15}", src.name());
        }
        let _ = writeln!(s);
        for (lang, _, _, r) in results {
            let total: u64 = r.sources[0].iter().sum();
            let _ = write!(s, "{:<12}", LANGS[*lang as usize].name);
            for src in Source::ALL {
                let _ = write!(
                    s,
                    "{:>14.1}%",
                    100.0 * r.sources[0][src.index()] as f64 / total.max(1) as f64
                );
            }
            let _ = writeln!(s);
        }

        for (lang, _, _, r) in results.iter().take(8) {
            let total: u64 = r.kept.iter().map(|t| t.bytes).sum();
            let perm = r.kept[0].bytes;
            let _ = writeln!(
                s,
                "\n{}: largest packages by kept tokens (all / permissive)",
                LANGS[*lang as usize].name
            );
            let mut list: Vec<(&u32, &[u64; 4])> = r.packages.iter().collect();
            list.sort_by_key(|(_, b)| std::cmp::Reverse(b.iter().sum::<u64>()));
            let mut by_perm = list.clone();
            by_perm.sort_by_key(|(_, b)| std::cmp::Reverse(b[0]));
            by_perm.retain(|(_, b)| b[0] > 0);
            for i in 0..list.len().min(10) {
                let cell = |list: &[(&u32, &[u64; 4])], perm_only: bool, total: u64| -> String {
                    list.get(i)
                        .map(|(p, b)| {
                            let v = if perm_only { b[0] } else { b.iter().sum() };
                            format!(
                                "{:<28}{:>8} {:>4.1}%",
                                truncate(&self.packages[**p as usize].name, 27),
                                human(tokens(v)),
                                100.0 * v as f64 / total.max(1) as f64
                            )
                        })
                        .unwrap_or_default()
                };
                let _ = writeln!(
                    s,
                    "  {:<46}  {}",
                    cell(&list, false, total),
                    cell(&by_perm, true, perm)
                );
            }
        }

        // What the classifier could not place, to tune it.
        let mut other: HashMap<&str, u64> = HashMap::new();
        for (_, _, _, r) in results {
            for (&(l, t), &b) in &r.licenses {
                if t == 3 {
                    *other.entry(&self.licenses[l as usize]).or_default() += b;
                }
            }
        }
        let mut other: Vec<_> = other.into_iter().collect();
        other.sort_by_key(|(_, b)| std::cmp::Reverse(*b));
        let _ = writeln!(
            s,
            "\nLargest licenses rated \"other\" (tokens, all languages)"
        );
        for (name, b) in other.iter().take(30) {
            let _ = writeln!(s, "  {:>8}  {}", human(tokens(*b)), truncate(name, 100));
        }
        if !self.errors.is_empty() {
            let _ = writeln!(s, "\nRead errors");
            for e in self.errors.iter().take(20) {
                let _ = writeln!(s, "  {}", truncate(e, 160));
            }
        }
        s
    }
}

#[derive(Default)]
struct LangResult {
    near_dups: Tally,
    /// Indexed like `Tier::ALL`, permissive first.
    kept: [Tally; 4],
    /// Bytes by tier and license source.
    sources: [[u64; 5]; 4],
    /// Bytes by component and tier.
    components: [[u64; 4]; 4],
    /// Bytes by package and tier.
    packages: HashMap<u32, [u64; 4]>,
    /// Bytes by license name and tier.
    licenses: HashMap<(u32, usize), u64>,
    /// What was written to the corpus: files, records (packages) and shards.
    written: Tally,
    records: u64,
    shards: usize,
}

fn tier_index(tier: Tier) -> usize {
    Tier::ALL.iter().position(|&t| t == tier).unwrap()
}

/// Removes the shards and staging shards of an earlier build from a language directory.
fn remove_shards(dir: &Path) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries {
        let path = entry?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if (name.starts_with("shard-") || name.starts_with("staging-"))
            && name.ends_with(".jsonl.zst")
        {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

fn human(n: f64) -> String {
    match n {
        n if n >= 1e9 => format!("{:.2}B", n / 1e9),
        n if n >= 1e6 => format!("{:.1}M", n / 1e6),
        n if n >= 1e3 => format!("{:.1}K", n / 1e3),
        n => format!("{n:.0}"),
    }
}

fn bytes(n: u64) -> String {
    let n = n as f64;
    match n {
        n if n >= 1e9 => format!("{:.2}GB", n / 1e9),
        n if n >= 1e6 => format!("{:.1}MB", n / 1e6),
        n => format!("{:.0}KB", n / 1e3),
    }
}

fn truncate(s: &str, max: usize) -> &str {
    &s[..s.floor_char_boundary(max)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn licenses_by_preference() {
        let dep5 = Copyright::parse(
            "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n\nFiles: *\nLicense: Expat\n\nFiles: gpl/*\nLicense: GPL-2+\n",
        );
        let code = "int main(void) { return 0; }\n";
        assert_eq!(
            file_license(&dep5, "a.c", code),
            (Tier::Permissive, Source::Dep5, "Expat".to_owned())
        );
        // An SPDX tag overrides the copyright file, in either direction.
        let spdx = format!("// SPDX-License-Identifier: MIT\n{code}");
        assert_eq!(
            file_license(&dep5, "gpl/a.c", &spdx),
            (Tier::Permissive, Source::Spdx, "MIT".to_owned())
        );
        // A header comment only ever makes the copyright file's answer more restrictive.
        let gpl = format!("/* under the terms of the GNU General Public License */\n{code}");
        assert_eq!(file_license(&dep5, "a.c", &gpl).0, Tier::Copyleft);
        let mit = format!("/* Permission is hereby granted, free of charge */\n{code}");
        assert_eq!(file_license(&dep5, "gpl/a.c", &mit).0, Tier::Copyleft);
        // Without a machine-readable file, the header is all there is.
        let missing = Copyright::missing();
        assert_eq!(
            file_license(&missing, "a.c", &mit),
            (Tier::Permissive, Source::Header, "MIT".to_owned())
        );
        assert_eq!(file_license(&missing, "a.c", code).1, Source::None);
    }

    #[test]
    fn exclusions() {
        assert!(excluded("debian/rules"));
        assert!(excluded("src/.hidden/x.c"));
        assert!(excluded(".pc/patch/x.c"));
        assert!(!excluded("src/debian/x.c"));
    }
}
