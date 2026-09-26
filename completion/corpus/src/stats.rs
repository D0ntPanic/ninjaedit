//! Counters collected while building the corpus.

use crate::filter::Reject;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct Stats {
    pub crates_in_index: AtomicU64,
    pub crates_no_versions: AtomicU64,
    pub crates_missing_on_disk: AtomicU64,
    pub crates_tar_error: AtomicU64,
    pub crates_manifest_error: AtomicU64,
    pub crates_no_license: AtomicU64,
    pub crates_license_rejected: AtomicU64,
    pub crates_no_rust_files: AtomicU64,
    pub crates_truncated: AtomicU64,
    pub crates_kept: AtomicU64,

    pub files_seen: AtomicU64,
    pub files_excluded_path: AtomicU64,
    pub files_oversized_entry: AtomicU64,
    pub files_rejected: [AtomicU64; Reject::ALL.len()],
    pub files_exact_dup: AtomicU64,
    pub files_near_dup: AtomicU64,
    pub files_kept: AtomicU64,

    pub bytes_seen: AtomicU64,
    pub bytes_kept: AtomicU64,
    pub lines_kept: AtomicU64,
    pub tokens_kept: AtomicU64,

    pub rejected_licenses: Mutex<HashMap<String, u64>>,
    pub editions: Mutex<HashMap<String, u64>>,
    /// Files and bytes kept, by language.
    pub languages: Mutex<HashMap<String, (u64, u64)>>,
}

impl Stats {
    pub fn reject(&self, reason: Reject) {
        self.files_rejected[reason.index()].fetch_add(1, Ordering::Relaxed);
    }

    pub fn count(&self, map: &Mutex<HashMap<String, u64>>, key: &str) {
        *map.lock().unwrap().entry(key.to_owned()).or_default() += 1;
    }

    pub fn count_language(&self, language: &str, bytes: u64) {
        let mut languages = self.languages.lock().unwrap();
        let entry = languages.entry(language.to_owned()).or_default();
        entry.0 += 1;
        entry.1 += bytes;
    }

    pub fn snapshot(&self) -> Snapshot {
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let mut rejected_licenses: Vec<(String, u64)> = self
            .rejected_licenses
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        rejected_licenses.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        rejected_licenses.truncate(40);
        let mut editions: Vec<(String, u64)> = self
            .editions
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        editions.sort();
        let mut languages: Vec<(String, u64, u64)> = self
            .languages
            .lock()
            .unwrap()
            .iter()
            .map(|(k, &(files, bytes))| (k.clone(), files, bytes))
            .collect();
        languages.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
        Snapshot {
            crates: [
                ("in_index", g(&self.crates_in_index)),
                ("no_versions", g(&self.crates_no_versions)),
                ("missing_on_disk", g(&self.crates_missing_on_disk)),
                ("tar_error", g(&self.crates_tar_error)),
                ("manifest_error", g(&self.crates_manifest_error)),
                ("no_license", g(&self.crates_no_license)),
                ("license_rejected", g(&self.crates_license_rejected)),
                ("no_rust_files", g(&self.crates_no_rust_files)),
                ("truncated", g(&self.crates_truncated)),
                ("kept", g(&self.crates_kept)),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
            files: [
                ("seen", g(&self.files_seen)),
                ("excluded_path", g(&self.files_excluded_path)),
                ("oversized_entry", g(&self.files_oversized_entry)),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .chain(
                Reject::ALL
                    .iter()
                    .zip(&self.files_rejected)
                    .map(|(r, c)| (format!("rejected_{}", r.name()), g(c))),
            )
            .chain([
                ("exact_dup".to_owned(), g(&self.files_exact_dup)),
                ("near_dup".to_owned(), g(&self.files_near_dup)),
                ("kept".to_owned(), g(&self.files_kept)),
            ])
            .collect(),
            bytes_seen: g(&self.bytes_seen),
            bytes_kept: g(&self.bytes_kept),
            lines_kept: g(&self.lines_kept),
            tokens_kept: g(&self.tokens_kept),
            rejected_licenses,
            editions,
            languages,
        }
    }
}

#[derive(Serialize)]
pub struct Snapshot {
    pub crates: Vec<(String, u64)>,
    pub files: Vec<(String, u64)>,
    pub bytes_seen: u64,
    pub bytes_kept: u64,
    pub lines_kept: u64,
    /// Rough token count: identifier runs plus punctuation characters. A trained BPE
    /// vocabulary will land within a factor of two of this.
    pub tokens_kept: u64,
    pub rejected_licenses: Vec<(String, u64)>,
    pub editions: Vec<(String, u64)>,
    /// Language, files and bytes kept.
    pub languages: Vec<(String, u64, u64)>,
}

impl Snapshot {
    pub fn print(&self) {
        fn table(title: &str, rows: &[(String, u64)]) {
            println!("{title}");
            for (k, v) in rows {
                println!("  {k:<28}{v:>14}");
            }
        }
        table("crates", &self.crates);
        table("files", &self.files);
        println!("sizes");
        println!("  {:<28}{:>14}", "bytes_seen", self.bytes_seen);
        println!("  {:<28}{:>14}", "bytes_kept", self.bytes_kept);
        println!("  {:<28}{:>14}", "lines_kept", self.lines_kept);
        println!("  {:<28}{:>14}", "tokens_kept (rough)", self.tokens_kept);
        println!("languages (kept files, bytes)");
        for (language, files, bytes) in &self.languages {
            println!("  {language:<16}{files:>12}{bytes:>14}");
        }
        table("editions (kept crates)", &self.editions);
        table("rejected licenses (top)", &self.rejected_licenses);
    }
}
