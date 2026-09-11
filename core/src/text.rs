//! Unicode text segmentation and measurement, shared by the editor model and
//! the UI so that both agree on what a "character" is and how wide it is.
//!
//! A character, for cursor movement and column arithmetic, is a grapheme
//! cluster: a base code point together with any combining marks, joiners,
//! and variation selectors that display as a single unit. Bytes that are not
//! valid UTF-8 are treated as one character each, displayed as U+FFFD.
//!
//! Columns are terminal cells, as computed by the `unicode-width` crate:
//! most characters are one cell wide, East Asian wide characters and emoji
//! are two, and combining marks on their own are zero. A tab advances to the
//! next tab stop, so its width depends on the column it starts at.
//!
//! Words, for word-wise movement and double-click selection, are runs of
//! characters of one [`CharClass`]: letters, digits, and underscores make
//! a word, and so, separately, do stretches of spaces and of punctuation.

use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// One user-perceived character within a byte slice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grapheme<'a> {
    /// The byte range within the slice the grapheme was read from.
    pub range: Range<usize>,
    /// The grapheme's text. For a byte of invalid UTF-8 this is U+FFFD.
    pub text: &'a str,
}

impl Grapheme<'_> {
    /// Display width in cells when starting at `column`. Tabs extend to the
    /// next multiple of `tab_width`; everything else is measured by
    /// `unicode-width`.
    pub fn width(&self, column: usize, tab_width: usize) -> usize {
        width(self.text, column, tab_width)
    }
}

/// Display width in cells of a grapheme (or any text without tabs beyond
/// the first character) drawn starting at `column`. See [`Grapheme::width`].
pub fn width(text: &str, column: usize, tab_width: usize) -> usize {
    if text == "\t" {
        let tab_width = tab_width.max(1);
        tab_width - column % tab_width
    } else {
        text.width()
    }
}

/// How a character takes part in words, for word-wise movement and
/// selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharClass {
    /// Letters, digits, and underscores.
    Word,
    Space,
    /// Anything else.
    Punctuation,
}

/// Classify a grapheme by its first code point.
pub fn classify(grapheme: &str) -> CharClass {
    let c = grapheme.chars().next().unwrap_or('\u{FFFD}');
    if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else if c.is_whitespace() {
        CharClass::Space
    } else {
        CharClass::Punctuation
    }
}

/// The word at a byte offset within one line of text: the run of
/// characters of one class (a word, a stretch of spaces, a run of
/// punctuation) containing the character at `offset`, or, at the end of
/// the text, the run ending there. Empty text gives an empty range. This
/// is what a double-click selects.
pub fn word_at(bytes: &[u8], offset: usize) -> Range<usize> {
    let cells: Vec<(Range<usize>, CharClass)> = graphemes(bytes)
        .map(|g| (g.range, classify(g.text)))
        .collect();
    if cells.is_empty() {
        return 0..0;
    }
    let offset = offset.min(bytes.len());
    let i = cells
        .partition_point(|(range, _)| range.end <= offset)
        .min(cells.len() - 1);
    let class = cells[i].1;
    let mut start = i;
    while start > 0 && cells[start - 1].1 == class {
        start -= 1;
    }
    let mut end = i;
    while end + 1 < cells.len() && cells[end + 1].1 == class {
        end += 1;
    }
    cells[start].0.start..cells[end].0.end
}

/// The offset of the next word boundary after `offset` within one line
/// of text: past any spaces, then past the run of characters of one
/// class that follows. Returns the end of the text from its last word.
pub fn next_word_boundary(bytes: &[u8], offset: usize) -> usize {
    let cells: Vec<(Range<usize>, CharClass)> = graphemes(bytes)
        .map(|g| (g.range, classify(g.text)))
        .collect();
    // The character containing (or starting at) the offset.
    let mut i = cells.partition_point(|(range, _)| range.end <= offset);
    while i < cells.len() && cells[i].1 == CharClass::Space {
        i += 1;
    }
    if i < cells.len() {
        let run = cells[i].1;
        while i < cells.len() && cells[i].1 == run {
            i += 1;
        }
    }
    cells.get(i).map_or(bytes.len(), |(range, _)| range.start)
}

/// The offset of the previous word boundary before `offset` within one
/// line of text: back over any spaces, then back over the run of
/// characters of one class before them. Returns 0 from the first word.
pub fn prev_word_boundary(bytes: &[u8], offset: usize) -> usize {
    let cells: Vec<(Range<usize>, CharClass)> = graphemes(bytes)
        .map(|g| (g.range, classify(g.text)))
        .collect();
    // Number of characters strictly before the offset.
    let mut i = cells.partition_point(|(range, _)| range.start < offset);
    while i > 0 && cells[i - 1].1 == CharClass::Space {
        i -= 1;
    }
    if i > 0 {
        let run = cells[i - 1].1;
        while i > 0 && cells[i - 1].1 == run {
            i -= 1;
        }
    }
    cells.get(i).map_or(bytes.len(), |(range, _)| range.start)
}

/// Split bytes into graphemes, tolerating invalid UTF-8.
pub fn graphemes(bytes: &[u8]) -> Graphemes<'_> {
    Graphemes {
        chunks: bytes.utf8_chunks(),
        valid: None,
        invalid_remaining: 0,
        pos: 0,
    }
}

/// Iterator returned by [`graphemes`].
pub struct Graphemes<'a> {
    chunks: std::str::Utf8Chunks<'a>,
    /// Graphemes of the valid part of the current chunk, with their offsets
    /// relative to that part.
    valid: Option<(usize, unicode_segmentation::GraphemeIndices<'a>)>,
    /// Invalid bytes still to emit after the current chunk's valid part.
    invalid_remaining: usize,
    /// Byte position of the next grapheme.
    pos: usize,
}

impl<'a> Iterator for Graphemes<'a> {
    type Item = Grapheme<'a>;

    fn next(&mut self) -> Option<Grapheme<'a>> {
        loop {
            if let Some((base, iter)) = &mut self.valid {
                if let Some((start, text)) = iter.next() {
                    let start = *base + start;
                    self.pos = start + text.len();
                    return Some(Grapheme {
                        range: start..self.pos,
                        text,
                    });
                }
                self.valid = None;
            }
            if self.invalid_remaining > 0 {
                self.invalid_remaining -= 1;
                let start = self.pos;
                self.pos += 1;
                return Some(Grapheme {
                    range: start..self.pos,
                    text: "\u{FFFD}",
                });
            }
            let chunk = self.chunks.next()?;
            self.valid = Some((self.pos, chunk.valid().grapheme_indices(true)));
            self.invalid_remaining = chunk.invalid().len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(bytes: &[u8]) -> Vec<(Range<usize>, &str)> {
        graphemes(bytes).map(|g| (g.range, g.text)).collect()
    }

    #[test]
    fn splits_clusters_and_invalid_bytes() {
        assert_eq!(split(b""), vec![]);
        assert_eq!(
            split("ae\u{301}b".as_bytes()),
            vec![(0..1, "a"), (1..4, "e\u{301}"), (4..5, "b")]
        );
        assert_eq!(
            split(b"a\xff\xfe\xe2\x82b"),
            vec![
                (0..1, "a"),
                (1..2, "\u{FFFD}"),
                (2..3, "\u{FFFD}"),
                (3..4, "\u{FFFD}"),
                (4..5, "\u{FFFD}"),
                (5..6, "b"),
            ],
            "each invalid byte, including a truncated sequence, is one character"
        );
        assert_eq!(split(b"\xff"), vec![(0..1, "\u{FFFD}")]);
        let family = "👨\u{200d}👩\u{200d}👧";
        assert_eq!(split(family.as_bytes()), vec![(0..family.len(), family)]);
    }

    #[test]
    fn words() {
        let line = b"foo_bar  ++baz";
        assert_eq!(word_at(line, 0), 0..7);
        assert_eq!(word_at(line, 6), 0..7);
        assert_eq!(word_at(line, 7), 7..9, "a run of spaces is a word");
        assert_eq!(word_at(line, 9), 9..11);
        assert_eq!(word_at(line, 12), 11..14);
        assert_eq!(word_at(line, 14), 11..14, "at the end: the last word");
        assert_eq!(word_at(line, 99), 11..14);
        assert_eq!(word_at(b"", 0), 0..0);
        assert_eq!(word_at("e\u{301}a b".as_bytes(), 1), 0..4);

        assert_eq!(next_word_boundary(line, 0), 7);
        assert_eq!(next_word_boundary(line, 3), 7);
        assert_eq!(next_word_boundary(line, 7), 11);
        assert_eq!(next_word_boundary(line, 11), 14);
        assert_eq!(next_word_boundary(line, 14), 14);
        assert_eq!(prev_word_boundary(line, 14), 11);
        assert_eq!(prev_word_boundary(line, 11), 9);
        assert_eq!(prev_word_boundary(line, 9), 0);
        assert_eq!(prev_word_boundary(line, 3), 0);
        assert_eq!(prev_word_boundary(line, 0), 0);
    }

    #[test]
    fn widths() {
        assert_eq!(width("a", 0, 4), 1);
        assert_eq!(width("한", 0, 4), 2);
        assert_eq!(width("e\u{301}", 0, 4), 1);
        assert_eq!(width("\u{301}", 0, 4), 0);
        assert_eq!(width("\t", 0, 4), 4);
        assert_eq!(width("\t", 3, 4), 1);
        assert_eq!(width("\t", 4, 4), 4);
        assert_eq!(width("\t", 5, 0), 1, "a zero tab width behaves as one");
        assert_eq!(width("\u{FFFD}", 0, 4), 1);
    }
}
