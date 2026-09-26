//! Packs documents into fixed-length rows with best-fit placement.

use anyhow::Result;
use std::io::Write;
use std::ops::Range;

/// Set on tokens the trainer should not compute loss for: padding, headers, and the header and
/// frame repeated at the start of a continued plain document.
pub const MASK_BIT: u16 = 0x8000;

/// Fewest content tokens worth starting a plain document with at the end of a row; a smaller
/// gap is padded.
const MIN_FRAGMENT: usize = 16;

pub struct Doc {
    pub tokens: Vec<u16>,
    /// For a plain document, its first `head.len()` tokens, masked: the header and the frame.
    /// Where the document is split to fill the end of a row, the rest continues in another row
    /// behind a copy of them, so every piece opens as a prompt does. Empty for FIM documents,
    /// which are always placed whole.
    pub head: Vec<u16>,
    pub fim: bool,
}

impl Doc {
    pub fn fim(tokens: Vec<u16>) -> Doc {
        Doc {
            tokens,
            head: Vec::new(),
            fim: true,
        }
    }

    /// A plain document whose tokens begin with `head_len` tokens of header and frame.
    pub fn plain(tokens: Vec<u16>, head_len: usize) -> Doc {
        let head = tokens[..head_len].iter().map(|&t| t | MASK_BIT).collect();
        Doc {
            tokens,
            head,
            fim: false,
        }
    }
}

/// A pending document with where it may be split: before any of its line breaks after the
/// head, so the rest starts with a line break as the editor's context window does.
struct Pending {
    doc: Doc,
    breaks: Vec<usize>,
}

#[derive(Default, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct PackStats {
    pub docs: u64,
    pub fim_docs: u64,
    pub split_docs: u64,
    pub dropped_docs: u64,
    /// FIM documents per span kind, in `SpanKind::ALL` order: line, lines, block, uniform.
    pub spans: [u64; 4],
    /// FIM documents in the PSM layout; the rest are SPM.
    #[serde(default)]
    pub psm_docs: u64,
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
        self.psm_docs += other.psm_docs;
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
    /// The line-break token ids.
    newlines: Range<u16>,
    pending: Vec<Pending>,
    row: Vec<u16>,
    out: W,
    pub stats: PackStats,
}

impl<W: Write> Packer<W> {
    pub fn new(seq_len: usize, pad: u16, newlines: Range<u16>, out: W) -> Packer<W> {
        Packer {
            seq_len,
            pad: pad | MASK_BIT,
            newlines,
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
        self.queue(doc);
        if self.pending.len() >= BUFFER_HIGH {
            self.drain(BUFFER_LOW)?;
        }
        Ok(())
    }

    fn queue(&mut self, doc: Doc) {
        let breaks = if doc.fim {
            Vec::new()
        } else {
            (doc.head.len() + 1..doc.tokens.len())
                .filter(|&i| self.newlines.contains(&(doc.tokens[i] & !MASK_BIT)))
                .collect()
        };
        self.pending.push(Pending { doc, breaks });
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
                .filter(|(_, p)| p.doc.tokens.len() <= remaining)
                .max_by_key(|(_, p)| p.doc.tokens.len())
                .map(|(i, _)| i);
            if let Some(i) = fit {
                let p = self.pending.swap_remove(i);
                self.row.extend_from_slice(&p.doc.tokens);
            } else {
                // Nothing fits: fill the gap with the start of the plain document that fills
                // the most of it when cut at a line break, or pad.
                let cut = self
                    .pending
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| !p.doc.fim)
                    .filter_map(|(i, p)| {
                        let at = p.breaks[..p.breaks.partition_point(|&b| b <= remaining)]
                            .last()
                            .copied()?;
                        (at >= p.doc.head.len() + MIN_FRAGMENT).then_some((i, at))
                    })
                    .max_by_key(|&(_, at)| at);
                match cut {
                    Some((i, at)) => {
                        let p = self.pending.swap_remove(i);
                        self.row.extend_from_slice(&p.doc.tokens[..at]);
                        let mut tokens = p.doc.head.clone();
                        tokens.extend_from_slice(&p.doc.tokens[at..]);
                        let head_len = p.doc.head.len();
                        self.queue(Doc {
                            tokens,
                            head: p.doc.head,
                            fim: false,
                        });
                        debug_assert!(
                            self.pending.last().unwrap().doc.tokens.len() <= self.seq_len
                        );
                        debug_assert_eq!(self.pending.last().unwrap().doc.head.len(), head_len);
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

    const NL: u16 = 5;

    fn rows(bytes: &[u8], seq_len: usize) -> Vec<Vec<u16>> {
        let tokens: Vec<u16> = bytes
            .chunks(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        tokens.chunks(seq_len).map(|r| r.to_vec()).collect()
    }

    /// A plain document: a two-token head (9, 8), then lines of `line` tokens each ending in a
    /// line break.
    fn plain(lines: usize, line: usize) -> Doc {
        let mut tokens = vec![9, 8];
        for i in 0..lines {
            tokens.extend(std::iter::repeat_n(10 + i as u16, line - 1));
            tokens.push(NL);
        }
        Doc::plain(tokens, 2)
    }

    #[test]
    fn places_fim_docs_whole() {
        let mut p = Packer::new(10, 0, NL..NL + 1, Vec::new());
        p.push(Doc::fim(vec![1; 8])).unwrap();
        p.push(Doc::fim(vec![2; 4])).unwrap();
        let (bytes, stats) = p.finish().unwrap();
        assert_eq!((stats.rows, stats.split_docs, stats.pad_tokens), (2, 0, 8));
        assert_eq!(rows(&bytes, 10)[1][..4], [2; 4]);
    }

    #[test]
    fn splits_plain_docs_at_line_breaks_behind_their_head() {
        let seq_len = 100;
        let mut p = Packer::new(seq_len, 0, NL..NL + 1, Vec::new());
        // Best fit places the FIM document first, leaving 40 tokens: too few for the plain
        // document of 2 + 8 lines of 6 = 50 tokens.
        p.push(Doc::fim(vec![1; 60])).unwrap();
        p.push(plain(8, 6)).unwrap();
        let (bytes, stats) = p.finish().unwrap();
        assert_eq!(stats.split_docs, 1);
        let rows = rows(&bytes, seq_len);
        // The cut is at the last line break that fits, offset 37: the row gets the head, five
        // whole lines and the sixth line's content, and pads the 3 tokens left.
        assert_eq!(rows[0][60..62], [9, 8]);
        assert_eq!(rows[0][96], 15);
        assert_eq!(rows[0][97..], [MASK_BIT; 3]);
        // The rest opens with a masked copy of the head, then the line break it was cut at.
        assert_eq!(rows[1][..3], [9 | MASK_BIT, 8 | MASK_BIT, NL]);
        assert_eq!(rows[1][3..8], [16; 5]);
        assert_eq!(stats.masked_tokens, 2 + stats.pad_tokens);
    }

    #[test]
    fn pads_rather_than_leave_a_scrap_of_a_document() {
        let seq_len = 100;
        let mut p = Packer::new(seq_len, 0, NL..NL + 1, Vec::new());
        // 17 tokens left: the last line break that fits leaves fewer than `MIN_FRAGMENT`
        // tokens of content after the head.
        p.push(Doc::fim(vec![1; 83])).unwrap();
        p.push(plain(8, 6)).unwrap();
        let (bytes, stats) = p.finish().unwrap();
        assert_eq!(stats.split_docs, 0);
        let rows = rows(&bytes, seq_len);
        assert_eq!(rows[0][83..], [MASK_BIT; 17]);
        assert_eq!(rows[1][..2], [9, 8]);
    }
}
