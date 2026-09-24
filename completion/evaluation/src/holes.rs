//! Choosing the sections of a file to delete and write again: the
//! editing scenarios the model is measured on.
//!
//! Three kinds of hole stand for three things a user does:
//!
//! * [finishing a line](HoleKind::RestOfLine) they have begun: the hole
//!   runs from a word in the middle of a line to its end;
//! * [writing new lines](HoleKind::Lines): one to eight lines from the
//!   start of the first one's content, its indentation already in place
//!   as it is after Enter;
//! * [filling in a block](HoleKind::Block): the body under a line that
//!   opens one (ending in `{`, `(`, `[` or `:`), everything indented
//!   deeper than it, up to [`MAX_BLOCK_LINES`] lines.
//!
//! Every hole starts at the start of a word and ends at the end of a
//! line's content, or earlier to stay within the scope it starts in: a
//! closing bracket of a scope opened before the hole is left in place,
//! as pairing puts it in when the scope is opened, and nothing after it
//! is taken (see [`within_scope`]). The choice is random but repeatable: it comes only
//! from the document and an [`Rng`] seeded by the caller.

use crate::document::Document;
use ninjaedit_core::auto_indent::closer_of;

/// The most lines a block hole takes; the rest of a longer body is left
/// in place after it.
pub const MAX_BLOCK_LINES: usize = 30;
/// Lines longer than this (generated or minified code) aren't chosen to
/// start a hole in.
const MAX_LINE_BYTES: usize = 400;
/// How many lines a [`HoleKind::Lines`] hole takes, picked uniformly,
/// so that short stretches are the most common.
const LINE_COUNTS: [usize; 8] = [1, 1, 1, 2, 2, 3, 5, 8];

/// A small, fast, seedable generator (SplitMix64), so that the
/// evaluation is the same from one run and one machine to the next.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number below `n`, which must not be zero.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// Shuffle `items` in place.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.below(i + 1));
        }
    }
}

/// A stable hash of a string (FNV-1a), for seeding a file's generator
/// from its path, so a file gets the same holes whatever other files are
/// evaluated with it.
pub fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HoleKind {
    RestOfLine,
    Lines,
    Block,
}

impl HoleKind {
    pub const ALL: [HoleKind; 3] = [HoleKind::RestOfLine, HoleKind::Lines, HoleKind::Block];

    pub fn name(self) -> &'static str {
        match self {
            HoleKind::RestOfLine => "rest of line",
            HoleKind::Lines => "lines",
            HoleKind::Block => "block",
        }
    }

    /// The name in the JSON report.
    pub fn key(self) -> &'static str {
        match self {
            HoleKind::RestOfLine => "rest_of_line",
            HoleKind::Lines => "lines",
            HoleKind::Block => "block",
        }
    }
}

/// A section of a document to delete and write again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hole {
    pub kind: HoleKind,
    /// Offsets into the document's text.
    pub start: usize,
    pub end: usize,
}

/// How many holes to make in `doc` at `rate` holes per 100 lines of code
/// (lines with words in them; blank lines don't count), so each file is
/// sampled in proportion to its size. The fraction left over is a draw:
/// at 1 per 100, a 30-line file gets a hole 30% of the time, so small
/// files are sampled as much as large ones on average.
pub fn count(doc: &Document, rate: f64, rng: &mut Rng) -> usize {
    let lines = doc.lines.iter().filter(|l| !l.words.is_empty()).count();
    let expected = lines as f64 * rate / 100.0;
    let whole = expected.floor();
    // 53 random bits make a uniform fraction in [0, 1).
    let draw = (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
    whole as usize + usize::from(draw < expected - whole)
}

/// Up to `count` different holes in `doc`, in the order they come in the
/// file. Fewer when the file hasn't room for that many.
pub fn choose(doc: &Document, count: usize, rng: &mut Rng) -> Vec<Hole> {
    let candidates = |keep: &dyn Fn(usize) -> bool| -> Vec<usize> {
        (0..doc.lines.len())
            .filter(|&i| {
                let line = &doc.lines[i];
                !line.words.is_empty() && line.content_end - line.start <= MAX_LINE_BYTES && keep(i)
            })
            .collect()
    };
    let rest_of_line = candidates(&|i| doc.lines[i].words.len() >= 2);
    let lines = candidates(&|_| true);
    let blocks = candidates(&|i| block_body(doc, i).is_some());

    let mut holes: Vec<Hole> = Vec::new();
    // Duplicates are drawn again, a bounded number of times, since a
    // small file may not have `count` different holes at all.
    for _ in 0..count * 8 {
        if holes.len() == count {
            break;
        }
        // Rest of line 35%, lines 40%, block 25%, falling back to lines
        // when the file has none of the kind drawn.
        let roll = rng.below(20);
        let kind = match roll {
            0..7 if !rest_of_line.is_empty() => HoleKind::RestOfLine,
            15..20 if !blocks.is_empty() => HoleKind::Block,
            _ => HoleKind::Lines,
        };
        let mut hole = match kind {
            HoleKind::RestOfLine => {
                let line = &doc.lines[rest_of_line[rng.below(rest_of_line.len())]];
                let word = 1 + rng.below(line.words.len() - 1);
                Hole {
                    kind,
                    start: line.words[word].start,
                    end: line.content_end,
                }
            }
            HoleKind::Lines => {
                if lines.is_empty() {
                    break;
                }
                let first = lines[rng.below(lines.len())];
                let count = LINE_COUNTS[rng.below(LINE_COUNTS.len())];
                let last = (first..doc.lines.len())
                    .filter(|&i| !doc.lines[i].words.is_empty())
                    .take(count)
                    .last()
                    .unwrap_or(first);
                Hole {
                    kind,
                    start: doc.lines[first].words[0].start,
                    end: doc.lines[last].content_end,
                }
            }
            HoleKind::Block => {
                let opener = blocks[rng.below(blocks.len())];
                let (first, last) = block_body(doc, opener).expect("a block candidate");
                Hole {
                    kind,
                    start: doc.lines[first].words[0].start,
                    end: doc.lines[last].content_end,
                }
            }
        };
        let Some(end) = within_scope(doc, hole.start, hole.end) else {
            continue;
        };
        hole.end = end;
        if !holes.contains(&hole) {
            holes.push(hole);
        }
    }
    holes.sort_by_key(|hole| (hole.start, hole.end));
    holes
}

/// Where a hole from `start` to `end` ends when kept within the scope it
/// starts in: before the first closing bracket in it of a bracket opened
/// before it, and the whitespace before that. That bracket is already
/// there when a user writes code in the scope, as pairing put it in when
/// the scope was opened, and the editor never suggests past it. `None`
/// if nothing is left.
fn within_scope(doc: &Document, start: usize, end: usize) -> Option<usize> {
    let first = doc.brackets.partition_point(|&at| at < start);
    let mut depth = 0usize;
    let mut cut = end;
    for &at in doc.brackets[first..].iter().take_while(|&&at| at < end) {
        if closer_of(doc.text.as_bytes()[at]).is_some() {
            depth += 1;
        } else if depth == 0 {
            cut = at;
            break;
        } else {
            depth -= 1;
        }
    }
    let kept = doc.text[start..cut].trim_end().len();
    (kept > 0).then_some(start + kept)
}

/// The body of the block the line `opener` opens, if it opens one: the
/// first and last lines with content among those after it that are
/// indented deeper, at most [`MAX_BLOCK_LINES`] of them.
fn block_body(doc: &Document, opener: usize) -> Option<(usize, usize)> {
    let line = &doc.lines[opener];
    let content = &doc.text[line.start..line.content_end];
    if !content.ends_with(['{', '(', '[', ':']) {
        return None;
    }
    let mut body = (opener + 1..doc.lines.len())
        .filter(|&i| !doc.lines[i].words.is_empty())
        .take_while(|&i| doc.lines[i].indent > line.indent)
        .take(MAX_BLOCK_LINES);
    let first = body.next()?;
    Some((first, body.last().unwrap_or(first)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ninjaedit_core::Language;

    const SOURCE: &str = "\
use std::io;

fn main() {
    let name = read_name();
    if name.is_empty() {
        println!(\"nobody\");
        return;
    }

    greet(&name);
}

fn greet(name: &str) {
    println!(\"hello, {name}\");
}
";

    #[test]
    fn the_generator_is_repeatable() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        let drawn: Vec<u64> = (0..4).map(|_| a.next_u64()).collect();
        assert_eq!(drawn, (0..4).map(|_| b.next_u64()).collect::<Vec<_>>());
        assert_ne!(drawn[0], Rng::new(43).next_u64());
        let mut items = [1, 2, 3, 4, 5, 6];
        Rng::new(7).shuffle(&mut items);
        let mut again = [1, 2, 3, 4, 5, 6];
        Rng::new(7).shuffle(&mut again);
        assert_eq!(items, again);
        assert_eq!(hash("src/main.rs"), hash("src/main.rs"));
        assert_ne!(hash("src/main.rs"), hash("src/lib.rs"));
    }

    #[test]
    fn holes_start_at_a_word_and_end_at_a_line_or_scope_end() {
        let doc = Document::new(SOURCE.to_owned(), Language::Rust);
        for seed in 0..50 {
            let holes = choose(&doc, 6, &mut Rng::new(seed));
            assert_eq!(holes, choose(&doc, 6, &mut Rng::new(seed)), "repeatable");
            assert!(!holes.is_empty());
            for hole in &holes {
                let (line, _) = doc.line_and_column(hole.start);
                assert!(
                    doc.lines[line].words.iter().any(|w| w.start == hole.start),
                    "{hole:?}"
                );
                let (last, _) = doc.line_and_column(hole.end);
                if doc.lines[last].content_end != hole.end {
                    // Cut short, before the bracket closing its scope.
                    let next = doc.text[hole.end..].trim_start();
                    assert!(next.starts_with([')', ']', '}']), "{hole:?}");
                }
                assert_eq!(within_scope(&doc, hole.start, hole.end), Some(hole.end));
                let text = &doc.text[hole.start..hole.end];
                match hole.kind {
                    HoleKind::RestOfLine => {
                        assert!(!text.contains('\n'));
                        assert!(hole.start > doc.lines[line].words[0].start);
                    }
                    HoleKind::Lines => {
                        assert_eq!(hole.start, doc.lines[line].words[0].start);
                        assert!(text.matches('\n').count() < 8 + 2, "{text:?}");
                    }
                    HoleKind::Block => {
                        let opener = &doc.text[..hole.start].trim_end();
                        assert!(opener.ends_with(['{', '(', '[', ':']), "{text:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn holes_stay_within_the_scope_they_start_in() {
        const SCOPES: &str = "\
fn f() {
    let x = compute(a, b);
    let v = vec![
        1,
        2,
    ];
    go(v);
}

fn g() {
    h(\")\");
}
";
        let doc = Document::new(SCOPES.to_owned(), Language::Rust);
        let cut = |hole: &str| {
            let start = SCOPES.find(hole).unwrap();
            within_scope(&doc, start, start + hole.len()).map(|end| &SCOPES[start..end])
        };
        // Brackets opened and closed in the hole are its own.
        assert_eq!(
            cut("let x = compute(a, b);"),
            Some("let x = compute(a, b);")
        );
        // One opened before it is closed after it, as pairing has it.
        assert_eq!(cut("b);"), Some("b"));
        assert_eq!(cut("2,\n    ];\n    go(v);"), Some("2,"));
        assert_eq!(cut("go(v);\n}\n\nfn g() {"), Some("go(v);"));
        // Nothing is left of a hole that starts by closing the scope.
        assert_eq!(cut(");"), None);
        // A bracket in a string doesn't count.
        assert_eq!(cut("h(\")\");"), Some("h(\")\");"));
        assert_eq!(cut("\")\");"), Some("\")\""));

        // Chosen holes are cut the same way.
        for seed in 0..50 {
            for hole in choose(&doc, 6, &mut Rng::new(seed)) {
                assert_eq!(within_scope(&doc, hole.start, hole.end), Some(hole.end));
                assert!(!doc.text[hole.start..hole.end].ends_with(char::is_whitespace));
            }
        }
    }

    #[test]
    fn holes_are_made_at_a_rate_per_line_of_code() {
        // Ten lines of code, and blank lines that don't count.
        let doc = Document::new("x;\n\n".repeat(10), Language::Rust);
        let mut rng = Rng::new(1);
        assert_eq!(count(&doc, 50.0, &mut rng), 5);
        assert_eq!(count(&doc, 0.0, &mut rng), 0);
        // A fraction of a hole is made that often, on average.
        let made: usize = (0..10_000).map(|_| count(&doc, 5.0, &mut rng)).sum();
        assert!((4_800..5_200).contains(&made), "{made}");
        let made: usize = (0..10_000).map(|_| count(&doc, 25.0, &mut rng)).sum();
        assert!((24_800..25_200).contains(&made), "{made}");
    }

    #[test]
    fn a_block_is_the_deeper_indented_body() {
        let doc = Document::new(SOURCE.to_owned(), Language::Rust);
        // `fn main() {` is line 2; its body runs to `greet(&name);`,
        // over the blank line.
        let (first, last) = block_body(&doc, 2).unwrap();
        assert_eq!((first, last), (3, 9));
        assert_eq!(block_body(&doc, 4), Some((5, 6)));
        assert_eq!(block_body(&doc, 3), None, "doesn't open a block");
        assert_eq!(block_body(&doc, 0), None);
    }
}
