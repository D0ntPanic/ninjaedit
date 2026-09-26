//! Shared types for reading the corpora that the `corpus` binary builds: records (a crate, or
//! a Debian source package's files in one language), shard iteration, and the deterministic
//! train/valid/test split by record name.
//!
//! Every file names its language, so one record can hold files in several (a crate's Rust
//! source and its `Cargo.toml`), and the packer picks the languages a model trains on.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use twox_hash::XxHash3_64;

#[derive(Serialize, Deserialize)]
pub struct FileRecord {
    pub path: String,
    /// The file's language, as the corpus names it: the name of a Debian corpus's language
    /// directory (`rust`, `cargo`, `cpp`), which is also how models name the language.
    pub language: String,
    pub content: String,
    /// The file's own license, where files in one record can differ (Debian packages); crates
    /// have one license for the whole record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

/// A crate, or a Debian source package's files in one language.
#[derive(Serialize, Deserialize)]
pub struct PackageRecord {
    pub name: String,
    pub version: String,
    pub edition: String,
    pub license: Option<String>,
    pub pubtime: Option<String>,
    pub deps: Vec<String>,
    pub files: Vec<FileRecord>,
}

/// Which partition of the corpus a record belongs to. The assignment is a function of the
/// record name alone, so it is stable across corpus rebuilds and every stage agrees on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Split {
    Train,
    Valid,
    Test,
}

impl Split {
    pub fn of(name: &str) -> Split {
        match XxHash3_64::oneshot(name.as_bytes()) % 1000 {
            0..980 => Split::Train,
            980..990 => Split::Valid,
            _ => Split::Test,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Split::Train => "train",
            Split::Valid => "valid",
            Split::Test => "test",
        }
    }
}

impl std::str::FromStr for Split {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Split> {
        match s {
            "train" => Ok(Split::Train),
            "valid" => Ok(Split::Valid),
            "test" => Ok(Split::Test),
            other => anyhow::bail!("unknown split {other:?}, expected train, valid or test"),
        }
    }
}

/// Checks a list of languages given on the command line, as the corpus names them, and
/// returns it in the order given.
pub fn parse_languages(args: &[String]) -> Result<Vec<String>> {
    let mut languages: Vec<String> = Vec::new();
    for language in args {
        if language.is_empty() || language.contains(char::is_whitespace) {
            anyhow::bail!("{language:?} is not a language name");
        }
        if languages.contains(language) {
            anyhow::bail!("{language} is listed twice");
        }
        languages.push(language.clone());
    }
    if languages.is_empty() {
        anyhow::bail!("no languages given");
    }
    Ok(languages)
}

/// Expands a leading `~/` to the home directory.
pub fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return Path::new(&home).join(rest);
    }
    PathBuf::from(path)
}

/// The `shard-NNN.jsonl.zst` files in a corpus directory, in order.
pub fn shard_paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut shards: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("shard-") && n.ends_with(".jsonl.zst"))
        })
        .collect();
    shards.sort();
    Ok(shards)
}

/// Streams the records in one shard.
pub fn read_shard(path: &Path) -> Result<impl Iterator<Item = Result<PackageRecord>>> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = BufReader::new(zstd::stream::read::Decoder::new(file)?);
    Ok(reader.lines().map(|line| {
        let line = line?;
        Ok(serde_json::from_str(&line)?)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn languages() {
        assert_eq!(parse_languages(&args(&["rust"])).unwrap(), ["rust"]);
        assert_eq!(
            parse_languages(&args(&["cpp", "c", "cmake"])).unwrap(),
            ["cpp", "c", "cmake"]
        );
        assert!(parse_languages(&args(&[])).is_err());
        assert!(parse_languages(&args(&[""])).is_err());
        assert!(parse_languages(&args(&["c", "c"])).is_err());
        assert!(parse_languages(&args(&["emacs lisp"])).is_err());
    }
}
