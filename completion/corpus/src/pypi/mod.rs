//! Builds a Python corpus from a bandersnatch mirror of PyPI.
//!
//! Each project's latest release is one record, in the Rust corpus's format: its Python files
//! (language `python`) under their paths in the source tree, read from the source distribution,
//! or from a wheel for projects that publish no source distribution or one that cannot be
//! read. The corpus is one language, so the shards are directly in the output directory.
//!
//! A project's license is what its core metadata declares (see `metadata`): the wheel's
//! metadata file beside it, or the sdist's `PKG-INFO` where there is none, and failing both,
//! the license files the distribution ships. Projects whose metadata the license policy does
//! not admit are not read. Within a project, a file's own `SPDX-License-Identifier` overrides
//! the project's license, and a license in its header comment only ever makes it more
//! restrictive, as in the Debian corpus; in a project that declares nothing, the header is
//! all there is. Exact duplicates are dropped as projects come in, and near-duplicates once
//! all are in, by rewriting the staging shards without them.

mod metadata;
mod mirror;

use crate::archive::{self, TopDir};
use crate::filter::{self, Reject};
use crate::lang::{self, Detected, LANGS};
use crate::license::{self, Policy, Tier};
use crate::minhash::{self, Signature};
use crate::stats::{bytes, human, truncate};
use crate::{RejectLog, ShardWriter, remove_shards, rewrite};
use anyhow::{Context, Result};
use clap::Parser;
use corpus::{FileRecord, PackageRecord, expand_home};
use mirror::Project;
use rayon::prelude::*;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Instant;
use twox_hash::XxHash3_64;

#[derive(Parser)]
pub struct PypiArgs {
    /// Root of the bandersnatch mirror (contains `web/packages/`).
    #[arg(long, default_value = "~/pypi-mirror")]
    mirror: String,
    /// Output directory for the corpus shards, the report and statistics.
    #[arg(long, default_value = "~/corpus/pypi-packages")]
    out: String,
    /// Which licenses to accept.
    #[arg(long, value_enum, default_value_t = Policy::Permissive)]
    licenses: Policy,
    /// Process only every Nth project, for quick experiments.
    #[arg(long)]
    sample: Option<usize>,
    /// Process only these projects.
    #[arg(long, value_delimiter = ',')]
    projects: Vec<String>,
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
    /// Training tokens to compare the corpus against. The default is the 70m shape's plan.
    #[arg(long, default_value_t = 2_000_000_000)]
    token_budget: u64,
    /// Write one line per rejected file (project, path, reason) to this file, for tuning
    /// filters.
    #[arg(long)]
    reject_log: Option<String>,
}

/// Upper bounds on how much of one project is read, to keep pathological projects in check.
const MAX_PROJECT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PROJECT_FILES: usize = 5000;
/// Bytes read of a license file or `PKG-INFO`.
const MAX_TEXT_BYTES: u64 = 64 * 1024;
/// Bytes at the start of a file searched for a license header.
const HEADER_BYTES: usize = 4096;

/// Where a license came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The project's metadata: `License-Expression`, a trove classifier, or `License`.
    Expression,
    Classifier,
    LicenseField,
    /// The license files the project's distribution ships.
    LicenseFile,
    /// The file's own `SPDX-License-Identifier`.
    Spdx,
    /// The file's own header comment.
    Header,
    None,
}

impl Source {
    const ALL: [Source; 7] = [
        Source::Expression,
        Source::Classifier,
        Source::LicenseField,
        Source::LicenseFile,
        Source::Spdx,
        Source::Header,
        Source::None,
    ];

    fn name(self) -> &'static str {
        match self {
            Source::Expression => "expression",
            Source::Classifier => "classifier",
            Source::LicenseField => "license_field",
            Source::LicenseFile => "license_file",
            Source::Spdx => "spdx",
            Source::Header => "header",
            Source::None => "none",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// A license, rated, with where it came from and its name.
#[derive(Clone, Debug)]
pub struct Declared {
    pub tier: Tier,
    pub source: Source,
    pub name: String,
}

impl Declared {
    fn none() -> Declared {
        Declared {
            tier: Tier::Other,
            source: Source::None,
            name: "none".to_owned(),
        }
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

/// What became of the Python files of the projects read, before deduplication.
#[derive(Default)]
struct FileCounts {
    seen: u64,
    bytes: u64,
    excluded: u64,
    oversized: u64,
    rejected: [u64; Reject::ALL.len()],
    /// Files whose license the policy does not admit: in a project that declares none, all
    /// but those whose headers name an admitted license.
    license: u64,
    passed: Tally,
}

impl FileCounts {
    fn add(&mut self, other: &FileCounts) {
        self.seen += other.seen;
        self.bytes += other.bytes;
        self.excluded += other.excluded;
        self.oversized += other.oversized;
        for (a, b) in self.rejected.iter_mut().zip(&other.rejected) {
            *a += b;
        }
        self.license += other.license;
        self.passed.add(&other.passed);
    }
}

/// What was done with a project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Sdist,
    Wheel,
    /// Its license is one the policy does not admit.
    Unlicensed,
    /// None of its distributions could be read.
    Unreadable,
}

impl Outcome {
    const ALL: [Outcome; 4] = [
        Outcome::Sdist,
        Outcome::Wheel,
        Outcome::Unlicensed,
        Outcome::Unreadable,
    ];

    fn name(self) -> &'static str {
        match self {
            Outcome::Sdist => "sdist",
            Outcome::Wheel => "wheel",
            Outcome::Unlicensed => "unlicensed",
            Outcome::Unreadable => "unreadable",
        }
    }
}

/// A file that passed the quality filters and the license policy.
struct ScannedFile {
    path: String,
    content: String,
    hash: u64,
    lines: u32,
    sig: Option<Box<Signature>>,
    license: Declared,
}

struct ProjectScan {
    project: u32,
    name: String,
    version: String,
    deps: Vec<String>,
    license: Declared,
    outcome: Outcome,
    truncated: bool,
    counts: FileCounts,
    files: Vec<ScannedFile>,
    error: Option<String>,
}

impl ProjectScan {
    /// Takes the project's name, version and dependencies from its metadata.
    fn apply(&mut self, meta: &metadata::Metadata) {
        if let Some(name) = &meta.name {
            self.name = name.clone();
        }
        if let Some(version) = &meta.version {
            self.version = version.clone();
        }
        self.deps = meta.requires.clone();
    }
}

pub fn build(args: PypiArgs) -> Result<()> {
    let mirror = expand_home(&args.mirror);
    let out = expand_home(&args.out);
    if let Some(jobs) = args.jobs {
        rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build_global()?;
    }
    fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
    remove_shards(&out)?;
    let started = Instant::now();

    eprintln!("listing {}...", mirror.join("web/packages").display());
    let (mut projects, counts) = mirror::find_projects(&mirror)?;
    if !args.projects.is_empty() {
        let wanted: Vec<String> = args.projects.iter().map(|p| mirror::normalize(p)).collect();
        projects.retain(|p| wanted.contains(&mirror::normalize(&p.name)));
    }
    if let Some(n) = args.sample.filter(|&n| n > 1) {
        projects = projects.into_iter().step_by(n).collect();
    }
    let compressed: u64 = projects.iter().map(Project::size).sum();
    eprintln!(
        "{} projects, {:.1} GB compressed ({:.1}s)",
        projects.len(),
        compressed as f64 / 1e9,
        started.elapsed().as_secs_f64()
    );

    // Largest projects first, so the longest reads do not start last.
    let mut order: Vec<u32> = (0..projects.len() as u32).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(projects[i as usize].size()));
    let reject_log = RejectLog::open(args.reject_log.as_deref())?;

    let collector = std::thread::scope(|scope| -> Result<Collector> {
        let (tx, rx) = mpsc::sync_channel::<ProjectScan>(256);
        let collector = scope.spawn(|| -> Result<Collector> {
            let mut collector = Collector::new(&projects, &out, reject_log.as_ref());
            for scan in rx {
                collector.add(scan)?;
            }
            Ok(collector)
        });
        let done = AtomicU64::new(0);
        let read = AtomicU64::new(0);
        let total = projects.len() as u64;
        // `par_bridge` hands projects out in order as threads free up, so the largest start
        // first; splitting the slice would leave most of them to one thread.
        order.iter().par_bridge().for_each_with(tx, |tx, &index| {
            let project = &projects[index as usize];
            let scan = scan_project(project, index, args.licenses, reject_log.as_ref());
            read.fetch_add(project.size(), Ordering::Relaxed);
            tx.send(scan).expect("collector stopped");
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_multiple_of(10000) || n == total {
                eprintln!(
                    "  {n}/{total} projects, {:.1}/{:.1} GB ({:.0}s)",
                    read.load(Ordering::Relaxed) as f64 / 1e9,
                    compressed as f64 / 1e9,
                    started.elapsed().as_secs_f64()
                );
            }
        });
        collector.join().expect("collector panicked")
    })?;

    let report = collector.finish(&args, &counts, started)?;
    print!("{report}");
    Ok(())
}

/// Decides a project's license, reads its Python files, and applies the quality filters and
/// the license policy to each.
fn scan_project(
    project: &Project,
    index: u32,
    policy: Policy,
    reject_log: Option<&RejectLog>,
) -> ProjectScan {
    let mut scan = ProjectScan {
        project: index,
        name: project.name.clone(),
        version: project.version.clone(),
        deps: Vec::new(),
        license: Declared::none(),
        outcome: Outcome::Unreadable,
        truncated: false,
        counts: FileCounts::default(),
        files: Vec::new(),
        error: None,
    };
    let wheel_meta = project
        .metadata
        .as_ref()
        .and_then(|path| fs::read(path).ok())
        .map(|bytes| metadata::parse(&String::from_utf8_lossy(&bytes)));
    let mut declared = wheel_meta.as_ref().and_then(metadata::declared);
    if let Some(meta) = &wheel_meta {
        scan.apply(meta);
    }
    if let Some(license) = &declared
        && !policy.admits(license.tier)
    {
        scan.license = license.clone();
        scan.outcome = Outcome::Unlicensed;
        return scan;
    }

    let mut read = None;
    for (dist, kind) in [
        (&project.sdist, DistKind::Sdist),
        (&project.wheel, DistKind::Wheel),
    ] {
        let Some(dist) = dist else { continue };
        match read_dist(&dist.path, kind) {
            Ok(contents) => {
                read = Some((contents, kind));
                break;
            }
            Err(e) => {
                let file = dist.path.file_name().unwrap_or_default().to_string_lossy();
                scan.error.get_or_insert(format!("{file}: {e:#}"));
            }
        }
    }
    let Some((contents, kind)) = read else {
        return scan;
    };
    if let Some(text) = &contents.pkg_info {
        let meta = metadata::parse(text);
        declared = declared.or_else(|| metadata::declared(&meta));
        if wheel_meta.is_none() {
            scan.apply(&meta);
        }
    }
    scan.license = declared
        .or_else(|| from_license_files(&contents.license_texts))
        .unwrap_or_else(Declared::none);
    // Without a declared license, files can still name their own.
    if scan.license.source != Source::None && !policy.admits(scan.license.tier) {
        scan.outcome = Outcome::Unlicensed;
        return scan;
    }
    scan.outcome = match kind {
        DistKind::Sdist => Outcome::Sdist,
        DistKind::Wheel => Outcome::Wheel,
    };
    scan.truncated = contents.truncated;
    let counts = &mut scan.counts;
    counts.excluded = contents.excluded;
    counts.oversized = contents.oversized;
    let style = LANGS[lang::PYTHON as usize].comments;
    let log = |path: &str, reason: &str, detail: &str| {
        if let Some(log) = reject_log {
            log.log(&scan.name, path, reason, detail);
        }
    };
    for (path, bytes) in contents.files {
        counts.seen += 1;
        counts.bytes += bytes.len() as u64;
        let Some(content) = filter::normalize(&bytes) else {
            counts.rejected[Reject::NotUtf8.index()] += 1;
            log(&path, Reject::NotUtf8.name(), "");
            continue;
        };
        if let Err(reason) = filter::check_with(&content, style) {
            counts.rejected[reason.index()] += 1;
            let detail = match reason {
                Reject::Generated => filter::generated_marker_with(&content, style).unwrap_or(""),
                _ => "",
            };
            log(&path, reason.name(), detail);
            continue;
        }
        let lines = content.lines().count() as u32;
        counts.passed.count(content.len() as u32, lines);
        let license = file_license(&scan.license, &content);
        if !policy.admits(license.tier) {
            counts.license += 1;
            log(&path, "license", &license.name);
            continue;
        }
        scan.files.push(ScannedFile {
            hash: XxHash3_64::oneshot(content.as_bytes()),
            lines,
            sig: minhash::signature(&content).map(Box::new),
            license,
            path,
            content,
        });
    }
    scan
}

/// Decides a file's license: its own SPDX identifier, or the project's, unless its header
/// comment names a more restrictive one or the project declares none.
fn file_license(project: &Declared, content: &str) -> Declared {
    if let Some(expr) = license::spdx_identifier(content)
        && let Some(tier) = license::evaluate(expr, license::tier_of)
    {
        return Declared {
            tier,
            source: Source::Spdx,
            name: expr.to_owned(),
        };
    }
    match license::from_header(&content[..content.floor_char_boundary(HEADER_BYTES)]) {
        Some((tier, name)) if tier < project.tier || project.source == Source::None => Declared {
            tier,
            source: Source::Header,
            name,
        },
        _ => project.clone(),
    }
}

/// Rates the license files a distribution ships. When there are several, all apply.
fn from_license_files(texts: &[String]) -> Option<Declared> {
    let found: Vec<(Tier, String)> = texts
        .iter()
        .filter_map(|t| license::from_header(t))
        .collect();
    let tier = found.iter().map(|(t, _)| *t).min()?;
    let mut names: Vec<&str> = Vec::new();
    for (_, name) in &found {
        if !names.contains(&name.as_str()) {
            names.push(name);
        }
    }
    Some(Declared {
        tier,
        source: Source::LicenseFile,
        name: names.join(", "),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DistKind {
    Sdist,
    Wheel,
}

/// What a distribution holds that the corpus wants.
#[derive(Default)]
struct Contents {
    /// Python files, by path in the project, and their bytes.
    files: Vec<(String, Vec<u8>)>,
    license_texts: Vec<String>,
    pkg_info: Option<String>,
    excluded: u64,
    oversized: u64,
    truncated: bool,
}

fn read_dist(path: &Path, kind: DistKind) -> Result<Contents> {
    let name = path.to_string_lossy();
    if name.ends_with(".whl") || name.ends_with(".zip") {
        read_zip(path, kind)
    } else {
        let mut reader = DistReader::default();
        let (_, top) = archive::walk_tar(path, |stored, size, r| reader.offer(stored, size, r))?;
        Ok(reader.finish(top.as_deref(), kind))
    }
}

fn read_zip(path: &Path, kind: DistKind) -> Result<Contents> {
    let mut zip = zip::ZipArchive::new(BufReader::new(File::open(path)?))?;
    let mut top = TopDir::default();
    let mut reader = DistReader::default();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        let stored = file.name().replace('\\', "/");
        let stored = stored.trim_start_matches("./").trim_end_matches('/');
        if stored.is_empty() {
            continue;
        }
        let is_file = !file.is_dir();
        top.observe(stored, is_file);
        if !is_file {
            continue;
        }
        let path = match kind {
            DistKind::Sdist => stored.to_owned(),
            DistKind::Wheel => wheel_path(stored),
        };
        let size = file.size();
        reader.offer(&path, size, &mut file)?;
    }
    // A wheel unpacks into `site-packages` as it is; only an sdist has a top directory.
    let top = top.get().filter(|_| kind == DistKind::Sdist);
    Ok(reader.finish(top.as_deref(), kind))
}

/// A file in a wheel, where it installs relative to `site-packages`: the `purelib` and
/// `platlib` directories of `<name>.data/` install there too.
fn wheel_path(stored: &str) -> String {
    if let Some((first, rest)) = stored.split_once('/')
        && first.ends_with(".data")
        && let Some(inner) = rest
            .strip_prefix("purelib/")
            .or_else(|| rest.strip_prefix("platlib/"))
    {
        return inner.to_owned();
    }
    stored.to_owned()
}

enum Entry {
    Python(Vec<u8>),
    Oversized,
    /// A license file or `PKG-INFO`, perhaps.
    Text(Vec<u8>),
}

/// Collects a distribution's files as an archive is read. The paths are final only once all
/// of a tarball has shown whether it has a top-level directory, so files are kept under their
/// stored paths until then.
#[derive(Default)]
struct DistReader {
    entries: Vec<(String, Entry)>,
    python_bytes: u64,
    python_files: usize,
    excluded: u64,
    truncated: bool,
}

impl DistReader {
    fn offer(&mut self, stored: &str, size: u64, reader: &mut dyn Read) -> Result<()> {
        let name = stored.rsplit('/').next().unwrap_or(stored);
        if matches!(lang::detect(stored), Some(Detected::Lang(id)) if id == lang::PYTHON) {
            // Excluded directories can hold thousands of files (a virtualenv), which must not
            // count against the project's limits. The top directory is not known yet, so the
            // path is checked both with and without its first component.
            if excluded(stored)
                || stored
                    .split_once('/')
                    .is_some_and(|(_, rest)| excluded(rest))
            {
                self.excluded += 1;
                return Ok(());
            }
            if size > filter::MAX_FILE_BYTES {
                self.entries.push((stored.to_owned(), Entry::Oversized));
                return Ok(());
            }
            if self.python_bytes + size > MAX_PROJECT_BYTES
                || self.python_files >= MAX_PROJECT_FILES
            {
                self.truncated = true;
                return Ok(());
            }
            let mut bytes = Vec::with_capacity(size as usize);
            reader.read_to_end(&mut bytes)?;
            self.python_bytes += size;
            self.python_files += 1;
            self.entries.push((stored.to_owned(), Entry::Python(bytes)));
        } else if name == "PKG-INFO"
            || license_file_name(name)
            || stored.contains("LICENSES/")
            || stored.contains(".dist-info/licenses/")
        {
            let mut bytes = Vec::new();
            reader.take(MAX_TEXT_BYTES).read_to_end(&mut bytes)?;
            self.entries.push((stored.to_owned(), Entry::Text(bytes)));
        }
        Ok(())
    }

    fn finish(self, top: Option<&str>, kind: DistKind) -> Contents {
        let mut contents = Contents {
            excluded: self.excluded,
            truncated: self.truncated,
            ..Contents::default()
        };
        for (stored, entry) in self.entries {
            let path = archive::source_path(&stored, top, None);
            match entry {
                Entry::Python(_) | Entry::Oversized if excluded(&path) => contents.excluded += 1,
                Entry::Python(bytes) => contents.files.push((path, bytes)),
                Entry::Oversized => contents.oversized += 1,
                Entry::Text(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    if kind == DistKind::Sdist && path == "PKG-INFO" {
                        contents.pkg_info = Some(text);
                    } else if license_path(&path, kind) {
                        contents.license_texts.push(text);
                    }
                }
            }
        }
        contents
    }
}

/// Paths in a project that are not its own source: hidden and vendored directories, build
/// output, installed packages (a virtualenv packaged by mistake), and a wheel's metadata.
fn excluded(path: &str) -> bool {
    let first = path.split('/').next().unwrap_or(path);
    filter::excluded_path(path)
        || path.starts_with("build/lib")
        || path.starts_with("build/bdist")
        || first.ends_with(".dist-info")
        || first.ends_with(".data")
        || path.split('/').any(|c| {
            matches!(c, "_vendor" | "site-packages" | "dist-packages") || c.ends_with(".egg-info")
        })
}

fn license_file_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    ["LICENSE", "LICENCE", "COPYING", "UNLICENSE", "COPYRIGHT"]
        .iter()
        .any(|p| upper.starts_with(p))
        && !upper.ends_with(".PY")
}

/// Whether a file is one of the project's own license files: at the top of an sdist or in its
/// `LICENSES/` directory, or in a wheel's `.dist-info` or its `licenses/` directory. License
/// files deeper in the tree are those of vendored code.
fn license_path(path: &str, kind: DistKind) -> bool {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    match kind {
        DistKind::Sdist => (dir.is_empty() && license_file_name(name)) || dir == "LICENSES",
        DistKind::Wheel => {
            (dir.ends_with(".dist-info") && !dir.contains('/') && license_file_name(name))
                || dir
                    .strip_suffix("/licenses")
                    .is_some_and(|d| d.ends_with(".dist-info") && !d.contains('/'))
        }
    }
}

/// A file staged for the corpus, as the report counts it.
struct Staged {
    project: u32,
    bytes: u32,
    lines: u32,
    tier: Tier,
    source: Source,
}

struct Collector<'a> {
    projects: &'a [Project],
    out: &'a Path,
    reject_log: Option<&'a RejectLog>,
    /// Taken when every project is in.
    staging: Option<ShardWriter>,
    hashes: HashSet<u64>,
    /// Indexed by id in the staging shards.
    staged: Vec<Staged>,
    sigs: Vec<Option<Signature>>,
    /// Record names of the projects with staged files.
    names: HashMap<u32, String>,
    counts: FileCounts,
    exact_dups: Tally,
    outcomes: [u64; Outcome::ALL.len()],
    truncated: u64,
    /// Projects and their compressed bytes by license tier, and projects by license source,
    /// of every project whose distributions could be read or whose metadata ruled it out.
    tiers: [(u64, u64); 4],
    sources: [u64; Source::ALL.len()],
    /// Projects by license name and tier.
    licenses: HashMap<(String, usize), u64>,
    errors: Vec<String>,
}

impl<'a> Collector<'a> {
    fn new(
        projects: &'a [Project],
        out: &'a Path,
        reject_log: Option<&'a RejectLog>,
    ) -> Collector<'a> {
        Collector {
            projects,
            out,
            reject_log,
            staging: Some(ShardWriter::new(out, "staging")),
            hashes: HashSet::new(),
            staged: Vec::new(),
            sigs: Vec::new(),
            names: HashMap::new(),
            counts: FileCounts::default(),
            exact_dups: Tally::default(),
            outcomes: [0; Outcome::ALL.len()],
            truncated: 0,
            tiers: [(0, 0); 4],
            sources: [0; Source::ALL.len()],
            licenses: HashMap::new(),
            errors: Vec::new(),
        }
    }

    fn add(&mut self, scan: ProjectScan) -> Result<()> {
        let project = &self.projects[scan.project as usize];
        self.outcomes[Outcome::ALL
            .iter()
            .position(|&o| o == scan.outcome)
            .unwrap()] += 1;
        self.truncated += scan.truncated as u64;
        if let Some(e) = scan.error {
            self.errors.push(format!("{}: {e}", scan.name));
        }
        if scan.outcome != Outcome::Unreadable {
            let t = tier_index(scan.license.tier);
            self.tiers[t].0 += 1;
            self.tiers[t].1 += project.size();
            self.sources[scan.license.source.index()] += 1;
            *self
                .licenses
                .entry((scan.license.name.clone(), t))
                .or_default() += 1;
        }
        self.counts.add(&scan.counts);
        let language = LANGS[lang::PYTHON as usize].identifier();
        let mut files = Vec::new();
        for file in scan.files {
            let bytes = file.content.len() as u32;
            if !self.hashes.insert(file.hash) {
                self.exact_dups.count(bytes, file.lines);
                if let Some(log) = self.reject_log {
                    log.log(&scan.name, &file.path, "exact_dup", "");
                }
                continue;
            }
            self.staged.push(Staged {
                project: scan.project,
                bytes,
                lines: file.lines,
                tier: file.license.tier,
                source: file.license.source,
            });
            self.sigs.push(file.sig.map(|s| *s));
            // A file names its license only where it differs from the project's.
            let own = matches!(file.license.source, Source::Spdx | Source::Header);
            files.push(FileRecord {
                path: file.path,
                language: language.clone(),
                content: file.content,
                license: own.then_some(file.license.name),
            });
        }
        if files.is_empty() {
            return Ok(());
        }
        self.names.insert(scan.project, scan.name.clone());
        let staging = self.staging.as_mut().expect("staging is open");
        staging.write(&PackageRecord {
            name: scan.name,
            version: scan.version,
            edition: String::new(),
            license: (scan.license.source != Source::None).then_some(scan.license.name),
            pubtime: None,
            deps: scan.deps,
            files,
        })
    }

    /// Removes near-duplicates, writes the corpus shards and statistics, and returns the
    /// report.
    fn finish(
        mut self,
        args: &PypiArgs,
        mirror: &mirror::MirrorCounts,
        started: Instant,
    ) -> Result<String> {
        if let Some(staging) = self.staging.take() {
            staging.finish()?;
        }
        self.hashes = HashSet::new();
        let sigs = std::mem::take(&mut self.sigs);
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

        let mut result = Kept::default();
        for (file, &dropped) in self.staged.iter().zip(&dropped) {
            if dropped {
                result.near_dups.count(file.bytes, file.lines);
                continue;
            }
            let t = tier_index(file.tier);
            result.kept[t].count(file.bytes, file.lines);
            result.sources[t][file.source.index()] += file.bytes as u64;
            *result.projects.entry(file.project).or_default() += file.bytes as u64;
        }
        result.shards = rewrite(self.out, &dropped, |record, _| {
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
        let kept_files: u64 = result.kept.iter().map(|t| t.files).sum();
        if result.written.files != kept_files {
            anyhow::bail!(
                "wrote {} files but {kept_files} were kept",
                result.written.files
            );
        }

        let stats = self.stats_json(args, mirror, &result, started);
        fs::write(
            self.out.join("stats.json"),
            serde_json::to_string_pretty(&stats)?,
        )?;
        let report = self.report(args, mirror, &result, started);
        fs::write(self.out.join("report.txt"), &report)?;
        eprintln!(
            "wrote stats.json and report.txt in {} ({:.0}s)",
            self.out.display(),
            started.elapsed().as_secs_f64()
        );
        Ok(report)
    }

    /// Projects with an sdist and a wheel of its version, with only an sdist, and with only
    /// wheels.
    fn kinds(&self) -> [u64; 3] {
        let mut kinds = [0; 3];
        for p in self.projects {
            kinds[match (&p.sdist, &p.wheel) {
                (Some(_), Some(_)) => 0,
                (Some(_), None) => 1,
                _ => 2,
            }] += 1;
        }
        kinds
    }

    /// Projects by kept bytes, largest first.
    fn top_projects(&self, result: &Kept) -> Vec<(&str, u64)> {
        let mut list: Vec<(&str, u64)> = result
            .projects
            .iter()
            .map(|(p, &b)| (self.names[p].as_str(), b))
            .collect();
        list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        list
    }

    fn stats_json(
        &self,
        args: &PypiArgs,
        mirror: &mirror::MirrorCounts,
        result: &Kept,
        started: Instant,
    ) -> Value {
        let bpt = args.bytes_per_token;
        let tokens = |bytes: u64| (bytes as f64 / bpt).round() as u64;
        let kinds = self.kinds();
        let by_tier = |f: &dyn Fn(usize) -> Value| -> Value {
            Tier::ALL
                .iter()
                .enumerate()
                .map(|(t, tier)| (tier.name().to_owned(), f(t)))
                .collect::<serde_json::Map<_, _>>()
                .into()
        };
        let mut licenses: Vec<(&(String, usize), &u64)> = self.licenses.iter().collect();
        licenses.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        json!({
            "mirror": args.mirror,
            "licenses": format!("{:?}", args.licenses).to_lowercase(),
            "bytes_per_token": bpt,
            "token_budget": args.token_budget,
            "near_dup_threshold": if args.skip_near_dedup { None } else { Some(args.near_dup_threshold) },
            "sample": args.sample,
            "seconds": started.elapsed().as_secs(),
            "mirror_files": {
                "wheels": mirror.wheels,
                "sdists": mirror.sdists,
                "metadata": mirror.metadata,
                "other_builds": mirror.other_builds,
                "unparsed": mirror.unparsed,
            },
            "projects": {
                "total": self.projects.len(),
                "sdist_and_wheel": kinds[0],
                "sdist_only": kinds[1],
                "wheel_only": kinds[2],
                "compressed_bytes": self.projects.iter().map(Project::size).sum::<u64>(),
                "outcomes": Outcome::ALL.iter().zip(self.outcomes).map(|(o, n)| (o.name().to_owned(), json!(n))).collect::<serde_json::Map<_, _>>(),
                "truncated": self.truncated,
                "by_tier": by_tier(&|t| json!({"projects": self.tiers[t].0, "compressed_bytes": self.tiers[t].1})),
                "by_license_source": Source::ALL.iter().map(|s| (s.name().to_owned(), json!(self.sources[s.index()]))).collect::<serde_json::Map<_, _>>(),
                "errors": self.errors,
            },
            "files": {
                "seen": self.counts.seen,
                "bytes": self.counts.bytes,
                "excluded_path": self.counts.excluded,
                "oversized": self.counts.oversized,
                "rejected": Reject::ALL.iter().map(|r| (r.name().to_owned(), json!(self.counts.rejected[r.index()]))).collect::<serde_json::Map<_, _>>(),
                "passed_filters": self.counts.passed.json(bpt),
                "license_rejected": self.counts.license,
                "exact_dups": self.exact_dups.json(bpt),
                "near_dups": result.near_dups.json(bpt),
                "kept": by_tier(&|t| result.kept[t].json(bpt)),
                "kept_tokens_by_source": by_tier(&|t| Source::ALL.iter().map(|s| (s.name().to_owned(), json!(tokens(result.sources[t][s.index()])))).collect::<serde_json::Map<_, _>>().into()),
            },
            "corpus": {
                "records": result.records,
                "shards": result.shards,
                "written": result.written.json(bpt),
            },
            "top_projects": self.top_projects(result).iter().take(100).map(|(name, b)| json!({
                "project": name,
                "tokens": tokens(*b),
            })).collect::<Vec<_>>(),
            "top_licenses": licenses.iter().take(200).map(|((name, t), n)| json!({
                "license": name,
                "tier": Tier::ALL[*t].name(),
                "projects": n,
            })).collect::<Vec<_>>(),
        })
    }

    fn report(
        &self,
        args: &PypiArgs,
        mirror: &mirror::MirrorCounts,
        result: &Kept,
        started: Instant,
    ) -> String {
        let bpt = args.bytes_per_token;
        let tokens = |bytes: u64| bytes as f64 / bpt;
        let kinds = self.kinds();
        let outcome =
            |o: Outcome| self.outcomes[Outcome::ALL.iter().position(|&x| x == o).unwrap()];
        let mut s = String::new();
        let _ = writeln!(
            s,
            "{} projects ({} with an sdist and a wheel of its version, {} sdist only, {} wheel only), {} compressed, {:.0} s",
            self.projects.len(),
            kinds[0],
            kinds[1],
            kinds[2],
            bytes(self.projects.iter().map(Project::size).sum()),
            started.elapsed().as_secs_f64()
        );
        let _ = writeln!(
            s,
            "mirror files: {} wheels ({} with metadata), {} sdists; skipped {} other built distributions, {} unrecognized",
            mirror.wheels, mirror.metadata, mirror.sdists, mirror.other_builds, mirror.unparsed
        );
        let _ = writeln!(
            s,
            "read {} sdists and {} wheels; {} projects not admitted by license, {} unreadable ({} with read errors), {} truncated",
            outcome(Outcome::Sdist),
            outcome(Outcome::Wheel),
            outcome(Outcome::Unlicensed),
            outcome(Outcome::Unreadable),
            self.errors.len(),
            self.truncated
        );
        let _ = writeln!(
            s,
            "tokens estimated at {bpt} bytes/token; budget {} tokens\n",
            human(args.token_budget as f64)
        );

        let _ = writeln!(s, "Projects by license (readable projects)");
        let projects: u64 = self.tiers.iter().map(|t| t.0).sum();
        let _ = writeln!(
            s,
            "{:<16}{:>10}{:>8}{:>12}",
            "tier", "projects", "%", "compressed"
        );
        for (t, tier) in Tier::ALL.iter().enumerate() {
            let _ = writeln!(
                s,
                "{:<16}{:>10}{:>7.1}%{:>12}",
                tier.name(),
                human(self.tiers[t].0 as f64),
                100.0 * self.tiers[t].0 as f64 / projects.max(1) as f64,
                bytes(self.tiers[t].1)
            );
        }
        let _ = writeln!(s, "\nWhere the projects' licenses came from");
        for src in &Source::ALL[..4] {
            let _ = write!(s, "{:>15}", src.name());
        }
        let _ = writeln!(s, "{:>15}", Source::None.name());
        for src in Source::ALL[..4].iter().chain([&Source::None]) {
            let _ = write!(
                s,
                "{:>14.1}%",
                100.0 * self.sources[src.index()] as f64 / projects.max(1) as f64
            );
        }
        let _ = writeln!(s);

        let c = &self.counts;
        let kept = result.kept.iter().fold(Tally::default(), |mut a, t| {
            a.add(t);
            a
        });
        let _ = writeln!(s, "\nPython files of the projects read");
        let row = |s: &mut String, label: &str, files: u64, b: Option<u64>| {
            let _ = writeln!(
                s,
                "  {label:<22}{:>10}{:>10}",
                human(files as f64),
                b.map(bytes).unwrap_or_default()
            );
        };
        row(&mut s, "seen", c.seen, Some(c.bytes));
        row(&mut s, "excluded path", c.excluded, None);
        row(&mut s, "oversized", c.oversized, None);
        for r in Reject::ALL {
            row(
                &mut s,
                &format!("rejected {}", r.name()),
                c.rejected[r.index()],
                None,
            );
        }
        row(
            &mut s,
            "passed filters",
            c.passed.files,
            Some(c.passed.bytes),
        );
        row(&mut s, "license not admitted", c.license, None);
        row(
            &mut s,
            "exact duplicates",
            self.exact_dups.files,
            Some(self.exact_dups.bytes),
        );
        row(
            &mut s,
            "near duplicates",
            result.near_dups.files,
            Some(result.near_dups.bytes),
        );
        row(&mut s, "kept", kept.files, Some(kept.bytes));

        let _ = writeln!(
            s,
            "\nCorpus written to {} ({} licenses)",
            self.out.display(),
            format!("{:?}", args.licenses).to_lowercase()
        );
        let _ = writeln!(
            s,
            "{:>10}{:>10}{:>10}{:>10}{:>10}{:>8}{:>9}",
            "projects", "files", "lines", "bytes", "tokens", "shards", "budget"
        );
        let _ = writeln!(
            s,
            "{:>10}{:>10}{:>10}{:>10}{:>10}{:>8}{:>8.2}x",
            human(result.records as f64),
            human(result.written.files as f64),
            human(result.written.lines as f64),
            bytes(result.written.bytes),
            human(tokens(result.written.bytes)),
            result.shards,
            tokens(result.written.bytes) / args.token_budget as f64,
        );

        let _ = writeln!(
            s,
            "\nKept tokens by license tier and where the license came from"
        );
        let _ = write!(s, "{:<16}{:>10}", "tier", "tokens");
        for src in Source::ALL {
            let _ = write!(s, "{:>14}", src.name());
        }
        let _ = writeln!(s);
        for (t, tier) in Tier::ALL.iter().enumerate() {
            if result.kept[t].files == 0 {
                continue;
            }
            let _ = write!(
                s,
                "{:<16}{:>10}",
                tier.name(),
                human(tokens(result.kept[t].bytes))
            );
            for src in Source::ALL {
                let _ = write!(
                    s,
                    "{:>13.1}%",
                    100.0 * result.sources[t][src.index()] as f64
                        / result.kept[t].bytes.max(1) as f64
                );
            }
            let _ = writeln!(s);
        }

        let _ = writeln!(s, "\nLargest projects by kept tokens");
        for (name, b) in self.top_projects(result).iter().take(25) {
            let _ = writeln!(
                s,
                "  {:<40}{:>9} {:>5.2}%",
                truncate(name, 39),
                human(tokens(*b)),
                100.0 * *b as f64 / kept.bytes.max(1) as f64
            );
        }

        for (title, t) in [
            ("Most common licenses rated \"other\" (projects)", 3),
            ("Most common licenses rated permissive (projects)", 0),
        ] {
            let mut list: Vec<(&str, u64)> = self
                .licenses
                .iter()
                .filter(|((_, tier), _)| *tier == t)
                .map(|((name, _), &n)| (name.as_str(), n))
                .collect();
            list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            let _ = writeln!(s, "\n{title}");
            for (name, n) in list.iter().take(30) {
                let _ = writeln!(
                    s,
                    "  {:>8}  {}",
                    human(*n as f64),
                    truncate(&name.replace('\n', " "), 100)
                );
            }
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

/// What deduplication leaves, and what was written.
#[derive(Default)]
struct Kept {
    near_dups: Tally,
    /// Indexed like `Tier::ALL`, permissive first.
    kept: [Tally; 4],
    /// Bytes by tier and license source.
    sources: [[u64; Source::ALL.len()]; 4],
    /// Bytes by project.
    projects: HashMap<u32, u64>,
    written: Tally,
    records: u64,
    shards: usize,
}

fn tier_index(tier: Tier) -> usize {
    Tier::ALL.iter().position(|&t| t == tier).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(tier: Tier, source: Source) -> Declared {
        Declared {
            tier,
            source,
            name: "x".to_owned(),
        }
    }

    #[test]
    fn licenses_by_preference() {
        let mit = declared(Tier::Permissive, Source::Classifier);
        let code = "import os\n\nprint(os.getcwd())\n";
        assert_eq!(file_license(&mit, code).source, Source::Classifier);
        // An SPDX tag overrides the project's license, in either direction.
        let gpl = declared(Tier::Copyleft, Source::Expression);
        let spdx = format!("# SPDX-License-Identifier: MIT\n{code}");
        let own = file_license(&gpl, &spdx);
        assert_eq!((own.tier, own.source), (Tier::Permissive, Source::Spdx));
        // A header only ever makes the project's license more restrictive.
        let header = format!("# under the terms of the GNU General Public License\n{code}");
        assert_eq!(file_license(&mit, &header).tier, Tier::Copyleft);
        let header = format!("# Permission is hereby granted, free of charge\n{code}");
        assert_eq!(file_license(&gpl, &header).tier, Tier::Copyleft);
        // Unless the project declares nothing.
        let own = file_license(&Declared::none(), &header);
        assert_eq!((own.tier, own.source), (Tier::Permissive, Source::Header));
    }

    #[test]
    fn exclusions() {
        assert!(excluded(".tox/py3/x.py"));
        assert!(excluded("venv/lib/python3.12/site-packages/six.py"));
        assert!(excluded("build/lib/pkg/x.py"));
        assert!(excluded("pip/_vendor/six.py"));
        assert!(excluded("pkg-1.0.dist-info/x.py"));
        assert!(excluded("pkg-1.0.data/scripts/x.py"));
        assert!(!excluded("src/pkg/build/x.py"));
        assert!(!excluded("tests/test_x.py"));
        assert_eq!(wheel_path("pkg-1.0.data/purelib/pkg/x.py"), "pkg/x.py");
        assert_eq!(wheel_path("pkg/x.py"), "pkg/x.py");
    }

    #[test]
    fn license_files() {
        assert!(license_path("LICENSE", DistKind::Sdist));
        assert!(license_path("LICENSE.txt", DistKind::Sdist));
        assert!(license_path("COPYING", DistKind::Sdist));
        assert!(license_path("LICENSES/MIT.txt", DistKind::Sdist));
        assert!(!license_path("vendor/LICENSE", DistKind::Sdist));
        assert!(!license_path("license.py", DistKind::Sdist));
        assert!(license_path("pkg-1.0.dist-info/LICENSE", DistKind::Wheel));
        assert!(license_path(
            "pkg-1.0.dist-info/licenses/LICENSE.md",
            DistKind::Wheel
        ));
        assert!(!license_path("pkg/LICENSE", DistKind::Wheel));
        let mit = "Permission is hereby granted, free of charge, to any person".to_owned();
        let d = from_license_files(&[mit]).unwrap();
        assert_eq!((d.tier, d.source), (Tier::Permissive, Source::LicenseFile));
        assert!(from_license_files(&["Copyright 2020 Someone".to_owned()]).is_none());
    }
}
