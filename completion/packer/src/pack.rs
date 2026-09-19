//! Packs documents into fixed-length rows with best-fit placement.

use anyhow::Result;
use std::io::Write;

/// Set on tokens the trainer should not compute loss for: padding and header tokens.
pub const MASK_BIT: u16 = 0x8000;

pub struct Doc {
    pub tokens: Vec<u16>,
    /// FIM documents are always placed whole; plain documents may be split across rows.
    pub fim: bool,
}

#[derive(Default, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct PackStats {
    pub docs: u64,
    pub fim_docs: u64,
    pub split_docs: u64,
    pub dropped_docs: u64,
    /// FIM documents per span kind, in `SpanKind::ALL` order: line, lines, block, uniform.
    pub spans: [u64; 4],
    pub rows: u64,
    pub tokens: u64,
    pub pad_tokens: u64,
    pub masked_tokens: u64,
}

impl PackStats {
    pub fn add(&mut self, other: &PackStats) {
        self.docs += other.docs;
        self.fim_docs += other.fim_docs;
        self.split_docs += other.split_docs;
        self.dropped_docs += other.dropped_docs;
        for (a, b) in self.spans.iter_mut().zip(other.spans) {
            *a += b;
        }
        self.rows += other.rows;
        self.tokens += other.tokens;
        self.pad_tokens += other.pad_tokens;
        self.masked_tokens += other.masked_tokens;
    }
}

/// Pending documents kept for best-fit selection before rows are forced out.
const BUFFER_HIGH: usize = 512;
const BUFFER_LOW: usize = 256;

pub struct Packer<W: Write> {
    seq_len: usize,
    pad: u16,
    pending: Vec<Doc>,
    row: Vec<u16>,
    out: W,
    pub stats: PackStats,
}

impl<W: Write> Packer<W> {
    pub fn new(seq_len: usize, pad: u16, out: W) -> Packer<W> {
        Packer {
            seq_len,
            pad: pad | MASK_BIT,
            pending: Vec::new(),
            row: Vec::with_capacity(seq_len),
            out,
            stats: PackStats::default(),
        }
    }

    pub fn push(&mut self, doc: Doc) -> Result<()> {
        assert!(
            doc.tokens.len() <= self.seq_len,
            "document longer than a row"
        );
        self.stats.docs += 1;
        if doc.fim {
            self.stats.fim_docs += 1;
        }
        self.pending.push(doc);
        if self.pending.len() >= BUFFER_HIGH {
            self.drain(BUFFER_LOW)?;
        }
        Ok(())
    }

    /// Emits rows until at most `keep` documents remain pending.
    fn drain(&mut self, keep: usize) -> Result<()> {
        while self.pending.len() > keep {
            let remaining = self.seq_len - self.row.len();
            // Largest pending document that fits.
            let fit = self
                .pending
                .iter()
                .enumerate()
                .filter(|(_, d)| d.tokens.len() <= remaining)
                .max_by_key(|(_, d)| d.tokens.len())
                .map(|(i, _)| i);
            if let Some(i) = fit {
                let doc = self.pending.swap_remove(i);
                self.row.extend_from_slice(&doc.tokens);
            } else {
                // Nothing fits: fill the gap with the start of a plain document, or pad.
                let plain = self
                    .pending
                    .iter()
                    .enumerate()
                    .filter(|(_, d)| !d.fim)
                    .max_by_key(|(_, d)| d.tokens.len())
                    .map(|(i, _)| i);
                match plain {
                    Some(i) => {
                        let mut doc = self.pending.swap_remove(i);
                        let rest = doc.tokens.split_off(remaining);
                        self.row.extend_from_slice(&doc.tokens);
                        self.pending.push(Doc {
                            tokens: rest,
                            fim: false,
                        });
                        self.stats.split_docs += 1;
                    }
                    None => self.pad_row(),
                }
            }
            if self.row.len() == self.seq_len {
                self.flush_row()?;
            }
        }
        Ok(())
    }

    fn pad_row(&mut self) {
        let pad = self.seq_len - self.row.len();
        self.row.resize(self.seq_len, self.pad);
        self.stats.pad_tokens += pad as u64;
    }

    fn flush_row(&mut self) -> Result<()> {
        debug_assert_eq!(self.row.len(), self.seq_len);
        let mut bytes = Vec::with_capacity(self.seq_len * 2);
        for &t in &self.row {
            bytes.extend_from_slice(&t.to_le_bytes());
            if t & MASK_BIT != 0 {
                self.stats.masked_tokens += 1;
            }
        }
        self.out.write_all(&bytes)?;
        self.stats.rows += 1;
        self.stats.tokens += self.seq_len as u64;
        self.row.clear();
        Ok(())
    }

    /// Places every pending document and pads the final row.
    pub fn finish(mut self) -> Result<(W, PackStats)> {
        self.drain(0)?;
        if !self.row.is_empty() {
            self.pad_row();
            self.flush_row()?;
        }
        self.out.flush()?;
        Ok((self.out, self.stats))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(n: usize, fim: bool) -> Doc {
        Doc {
            tokens: vec![1; n],
            fim,
        }
    }

    #[test]
    fn packs_whole_fim_docs_and_splits_plain_ones() {
        let mut p = Packer::new(10, 0, Vec::new());
        p.push(doc(8, true)).unwrap();
        p.push(doc(7, false)).unwrap();
        let (bytes, stats) = p.finish().unwrap();
        assert_eq!(stats.rows, 2);
        assert_eq!(stats.split_docs, 1);
        // Row 1: fim(8) + 2 tokens of the plain doc. Row 2: the remaining 5 + 5 pad.
        assert_eq!(stats.pad_tokens, 5);
        assert_eq!(stats.masked_tokens, 5);
        assert_eq!(bytes.len(), 40);
        let last = u16::from_le_bytes([bytes[38], bytes[39]]);
        assert_eq!(last, MASK_BIT);
    }
}
