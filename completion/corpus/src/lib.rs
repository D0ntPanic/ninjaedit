//! Shared types for reading the corpus that the `corpus` binary builds: crate records,
//! shard iteration, and the deterministic train/valid/test split by crate name.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use twox_hash::XxHash3_64;

#[derive(Serialize, Deserialize)]
pub struct FileRecord {
    pub path: String,
    pub content: String,
}

#[derive(Serialize, Deserialize)]
pub struct CrateRecord {
    pub name: String,
    pub version: String,
    pub edition: String,
    pub license: Option<String>,
    pub pubtime: Option<String>,
    pub deps: Vec<String>,
    pub files: Vec<FileRecord>,
}

/// Which partition of the corpus a crate belongs to. The assignment is a function of the
/// crate name alone, so it is stable across corpus rebuilds and every stage agrees on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Split {
    Train,
    Valid,
    Test,
}

impl Split {
    pub fn of(crate_name: &str) -> Split {
        match XxHash3_64::oneshot(crate_name.as_bytes()) % 1000 {
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

/// Streams the crate records in one shard.
pub fn read_shard(path: &Path) -> Result<impl Iterator<Item = Result<CrateRecord>>> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = BufReader::new(zstd::stream::read::Decoder::new(file)?);
    Ok(reader.lines().map(|line| {
        let line = line?;
        Ok(serde_json::from_str(&line)?)
    }))
}
