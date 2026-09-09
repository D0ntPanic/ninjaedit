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
