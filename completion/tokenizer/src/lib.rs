//! Byte-level BPE tokenizer for Rust source.
//!
//! Token ids are laid out as: special tokens, then one line-break token per indent level,
//! then the 256 byte values, then one id per learned merge.

pub mod bpe;
pub mod pretok;

use anyhow::{Context, Result, bail};
use bpe::Pair;
use pretok::{Indent, MAX_INDENT, Piece};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Names of the special tokens, in id order.
pub const SPECIALS: &[&str] = &[
    "<pad>",
    "<lang>",
    "<eos>",
    "<fim_prefix>",
    "<fim_middle>",
    "<fim_suffix>",
    "<eom>",
    "<context>",
    "<file_name>",
    "<module_name>",
];

const NEWLINE_COUNT: u32 = MAX_INDENT as u32 + 1;

#[derive(Serialize, Deserialize)]
struct Saved {
    specials: Vec<String>,
    max_indent: usize,
    merges: Vec<Pair>,
}

pub struct Tokenizer {
    /// Byte content of every id; empty for specials and line breaks.
    vocab: Vec<Vec<u8>>,
    merges: Vec<Pair>,
    /// Pair to the id it merges into.
    ranks: FxHashMap<Pair, u32>,
}

impl Tokenizer {
    pub const NEWLINE_BASE: u32 = SPECIALS.len() as u32;
    pub const BYTE_BASE: u32 = Self::NEWLINE_BASE + NEWLINE_COUNT;
    pub const MERGE_BASE: u32 = Self::BYTE_BASE + 256;

    pub fn from_merges(merges: Vec<Pair>) -> Tokenizer {
        let mut vocab: Vec<Vec<u8>> = vec![Vec::new(); Self::BYTE_BASE as usize];
        vocab.extend((0..=255u8).map(|b| vec![b]));
        let mut ranks = FxHashMap::default();
        for (i, &(a, b)) in merges.iter().enumerate() {
            let mut bytes = vocab[a as usize].clone();
            bytes.extend_from_slice(&vocab[b as usize]);
            vocab.push(bytes);
            ranks.insert((a, b), Self::MERGE_BASE + i as u32);
        }
        Tokenizer {
            vocab,
            merges,
            ranks,
        }
    }

    pub fn vocab_size(&self) -> usize {
        self.vocab.len()
    }

    /// A hash of the merge list. Two tokenizers with the same fingerprint assign the same ids;
    /// a model is only usable with the tokenizer it was trained with, so checkpoints record it.
    pub fn fingerprint(&self) -> String {
        let mut bytes = Vec::with_capacity(self.merges.len() * 8);
        for &(a, b) in &self.merges {
            bytes.extend_from_slice(&a.to_le_bytes());
            bytes.extend_from_slice(&b.to_le_bytes());
        }
        format!("{:016x}", twox_hash::XxHash3_64::oneshot(&bytes))
    }

    pub fn merges(&self) -> &[Pair] {
        &self.merges
    }

    pub fn special(&self, name: &str) -> u32 {
        SPECIALS
            .iter()
            .position(|&s| s == name)
            .unwrap_or_else(|| panic!("no special token {name}")) as u32
    }

    pub fn newline(&self, level: usize) -> u32 {
        Self::NEWLINE_BASE + level.min(MAX_INDENT) as u32
    }

    /// Indent level if `id` is a line-break token.
    pub fn newline_level(&self, id: u32) -> Option<usize> {
        (Self::NEWLINE_BASE..Self::BYTE_BASE)
            .contains(&id)
            .then(|| (id - Self::NEWLINE_BASE) as usize)
    }

    pub fn is_special(&self, id: u32) -> bool {
        id < Self::NEWLINE_BASE
    }

    /// Bytes for a BPE token, or `None` for specials and line breaks.
    pub fn token_bytes(&self, id: u32) -> Option<&[u8]> {
        (id >= Self::BYTE_BASE).then(|| self.vocab[id as usize].as_slice())
    }

    /// A readable rendering of one token, for debugging output.
    pub fn token_text(&self, id: u32) -> String {
        if let Some(level) = self.newline_level(id) {
            format!("<nl:{level}>")
        } else if self.is_special(id) {
            SPECIALS[id as usize].to_owned()
        } else {
            String::from_utf8_lossy(&self.vocab[id as usize]).into_owned()
        }
    }

    /// The bytes each id renders to when line breaks use `indent` per indent level: the bytes of
    /// a BPE token, the line break and its indentation, or nothing for a special token. Indexed
    /// by id; `decode` produces the concatenation of these.
    pub fn render_table(&self, indent: &str) -> Vec<Vec<u8>> {
        (0..self.vocab.len() as u32)
            .map(|id| {
                if let Some(level) = self.newline_level(id) {
                    let mut bytes = vec![b'\n'];
                    for _ in 0..level {
                        bytes.extend_from_slice(indent.as_bytes());
                    }
                    bytes
                } else {
                    self.token_bytes(id).map(<[u8]>::to_vec).unwrap_or_default()
                }
            })
            .collect()
    }

    pub fn encoder(&self) -> Encoder<'_> {
        Encoder {
            tok: self,
            cache: FxHashMap::default(),
        }
    }

    /// Renders tokens back to text, using `indent` for each indent level.
    pub fn decode(&self, ids: &[u32], indent: &str) -> String {
        let mut bytes = Vec::new();
        for &id in ids {
            if let Some(level) = self.newline_level(id) {
                bytes.push(b'\n');
                for _ in 0..level {
                    bytes.extend_from_slice(indent.as_bytes());
                }
            } else if let Some(b) = self.token_bytes(id) {
                bytes.extend_from_slice(b);
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let saved = Saved {
            specials: SPECIALS.iter().map(|s| s.to_string()).collect(),
            max_indent: MAX_INDENT,
            merges: self.merges.clone(),
        };
        std::fs::write(path, serde_json::to_string(&saved)?)
            .with_context(|| format!("writing {}", path.display()))
    }

    pub fn load(path: &Path) -> Result<Tokenizer> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let saved: Saved = serde_json::from_str(&text)?;
        if saved.specials != SPECIALS || saved.max_indent != MAX_INDENT {
            bail!("{} was built for a different token layout", path.display());
        }
        Ok(Tokenizer::from_merges(saved.merges))
    }
}

/// Encodes text, caching the BPE result of each distinct pre-token.
pub struct Encoder<'a> {
    tok: &'a Tokenizer,
    cache: FxHashMap<Vec<u8>, Vec<u32>>,
}

const CACHE_LIMIT: usize = 1 << 20;

impl Encoder<'_> {
    /// Appends the tokens of `text` to `out`. The indent unit is detected from the text.
    pub fn encode(&mut self, text: &str, out: &mut Vec<u32>) {
        self.encode_with(text, pretok::detect_indent(text), out)
    }

    pub fn encode_with(&mut self, text: &str, indent: Indent, out: &mut Vec<u32>) {
        for piece in pretok::pretokenize(text, indent) {
            match piece {
                Piece::Newline(level) => out.push(self.tok.newline(level)),
                Piece::Bytes(bytes) => self.encode_piece(bytes, out),
            }
        }
    }

    /// Appends the header each training document opens with: the module's name and the file's
    /// path within the module, each after its special token. Prompts open with the same header
    /// so the model sees what it was trained on.
    pub fn encode_header(&mut self, module_name: &str, file_path: &str, out: &mut Vec<u32>) {
        out.push(self.tok.special("<module_name>"));
        self.encode_with(module_name, Indent::Spaces(4), out);
        out.push(self.tok.special("<file_name>"));
        self.encode_with(file_path, Indent::Spaces(4), out);
    }

    /// Encodes one pre-token's bytes with BPE, without line or indentation handling.
    pub fn encode_bytes(&mut self, bytes: &[u8], out: &mut Vec<u32>) {
        if bytes.is_empty() {
            return;
        }
        self.encode_piece(bytes, out)
    }

    fn encode_piece(&mut self, bytes: &[u8], out: &mut Vec<u32>) {
        if bytes.len() == 1 {
            out.push(Tokenizer::BYTE_BASE + bytes[0] as u32);
            return;
        }
        if let Some(ids) = self.cache.get(bytes) {
            out.extend_from_slice(ids);
            return;
        }
        let ids = self.merge(bytes);
        out.extend_from_slice(&ids);
        if self.cache.len() >= CACHE_LIMIT {
            self.cache.clear();
        }
        self.cache.insert(bytes.to_vec(), ids);
    }

    /// Applies merges in rank order until none apply.
    fn merge(&self, bytes: &[u8]) -> Vec<u32> {
        let mut ids: Vec<u32> = bytes
            .iter()
            .map(|&b| Tokenizer::BYTE_BASE + b as u32)
            .collect();
        loop {
            let mut best: Option<(u32, usize)> = None;
            for i in 0..ids.len() - 1 {
                if let Some(&id) = self.tok.ranks.get(&(ids[i], ids[i + 1]))
                    && best.is_none_or(|(b, _)| id < b)
                {
                    best = Some((id, i));
                }
            }
            let Some((new_id, _)) = best else { break };
            let (a, b) = self.tok.merges[(new_id - Tokenizer::MERGE_BASE) as usize];
            let mut merged = Vec::with_capacity(ids.len());
            let mut i = 0;
            while i < ids.len() {
                if i + 1 < ids.len() && ids[i] == a && ids[i + 1] == b {
                    merged.push(new_id);
                    i += 2;
                } else {
                    merged.push(ids[i]);
                    i += 1;
                }
            }
            ids = merged;
            if ids.len() == 1 {
                break;
            }
        }
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_chosen_indent() {
        let b = |c: u8| Tokenizer::BYTE_BASE + c as u32;
        let merges = vec![(b(b'f'), b(b'n')), (b(b' '), b(b'm')), (b(b'a'), b(b'i'))];
        let tok = Tokenizer::from_merges(merges);
        let src = "fn main() {\n\tif x {\n\t\ty();\n\t}\n}\n";
        let mut ids = Vec::new();
        tok.encoder().encode(src, &mut ids);
        assert_eq!(tok.token_text(ids[0]), "fn");
        assert_eq!(tok.token_text(ids[1]), " m");
        assert_eq!(tok.token_text(ids[2]), "ai");
        assert_eq!(tok.decode(&ids, "\t"), src);
        assert_eq!(
            tok.decode(&ids, "    "),
            "fn main() {\n    if x {\n        y();\n    }\n}\n"
        );
    }
}
