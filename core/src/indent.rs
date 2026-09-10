//! Indentation style: whether lines are indented with tabs or spaces, and
//! by how many spaces, guessed from a file's contents.
//!
//! The guess looks at the leading whitespace of each line. A file is
//! tab-indented when more indented lines start with a tab than with a
//! space. For a space-indented file, the width of one level is the most
//! common change in indentation between consecutive non-blank lines.
//! Changes of a single column are ignored, since they come from aligning
//! continuation lines far more often than from one-space indentation. Only
//! the first [`MAX_LINES`] lines are examined, so opening a huge file stays
//! fast.

use crate::buffer::BufferSnapshot;
use std::cmp::Reverse;

/// How lines are indented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indentation {
    /// One tab character per level.
    Tabs,
    /// The given number of spaces per level (at least one).
    Spaces(usize),
}

impl Indentation {
    /// The style used when a file's contents don't reveal one.
    pub const DEFAULT: Indentation = Indentation::Spaces(4);

    /// The text of one level of indentation.
    pub fn unit(self) -> String {
        match self {
            Indentation::Tabs => "\t".to_owned(),
            Indentation::Spaces(n) => " ".repeat(n.max(1)),
        }
    }

    /// The width in columns of one level, given the width of a tab.
    pub fn width(self, tab_width: usize) -> usize {
        match self {
            Indentation::Tabs => tab_width.max(1),
            Indentation::Spaces(n) => n.max(1),
        }
    }
}

impl Default for Indentation {
    fn default() -> Indentation {
        Indentation::DEFAULT
    }
}

/// The number of lines [`detect`] examines at most.
pub const MAX_LINES: usize = 10_000;

/// The widest space indentation that can be detected.
const MAX_WIDTH: usize = 8;

/// Guess the indentation style of a buffer, or `None` if it has no indented
/// lines to go by.
pub fn detect(snapshot: &BufferSnapshot) -> Option<Indentation> {
    let mut detector = Detector::default();
    let mut lines = snapshot.lines_from(0);
    let mut count = 0;
    while count < MAX_LINES
        && let Some(line) = lines.next_line()
    {
        detector.feed(line);
        count += 1;
    }
    detector.finish()
}

/// Incremental indentation detection: feed it lines in order, then ask for
/// the result. See the [module documentation](self) for the heuristic.
#[derive(Debug, Default)]
pub struct Detector {
    /// Lines whose indentation starts with a tab.
    tab_lines: usize,
    /// Lines indented with spaces only.
    space_lines: usize,
    /// `deltas[d]` counts the space-indented lines whose indentation
    /// differs by `d` columns from the non-blank line before them.
    deltas: [usize; MAX_WIDTH + 1],
    /// The indentation of the previous non-blank line, when it was made of
    /// spaces (or nothing).
    previous: Option<usize>,
}

impl Detector {
    /// Examine one line, given without its terminator.
    pub fn feed(&mut self, line: &[u8]) {
        let indent = line
            .iter()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count();
        if indent == line.len() {
            // Blank lines say nothing, and don't interrupt the comparison
            // of the lines around them.
            return;
        }
        let leading = &line[..indent];
        if leading.first() == Some(&b'\t') {
            self.tab_lines += 1;
            self.previous = None;
            return;
        }
        if leading.contains(&b'\t') {
            // Spaces then tabs: mixed, so no vote either way.
            self.previous = None;
            return;
        }
        if indent > 0 {
            self.space_lines += 1;
        }
        if let Some(previous) = self.previous {
            let delta = indent.abs_diff(previous);
            if (2..=MAX_WIDTH).contains(&delta) {
                self.deltas[delta] += 1;
            }
        }
        self.previous = Some(indent);
    }

    /// The style the lines so far suggest, or `None` if they suggest
    /// nothing: no line is indented, or lines are indented with spaces but
    /// never by a consistent amount.
    pub fn finish(&self) -> Option<Indentation> {
        if self.tab_lines == 0 && self.space_lines == 0 {
            return None;
        }
        if self.tab_lines > self.space_lines {
            return Some(Indentation::Tabs);
        }
        // The most common delta wins; on a tie, the narrower one, since a
        // wide delta is often two levels of a narrow one.
        let (width, count) = (2..=MAX_WIDTH)
            .map(|d| (d, self.deltas[d]))
            .max_by_key(|&(d, count)| (count, Reverse(d)))?;
        (count > 0).then_some(Indentation::Spaces(width))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::FileBuffer;

    fn detect_text(text: &str) -> Option<Indentation> {
        detect(&FileBuffer::from_text(text).snapshot())
    }

    #[test]
    fn unit_and_width() {
        assert_eq!(Indentation::Tabs.unit(), "\t");
        assert_eq!(Indentation::Spaces(2).unit(), "  ");
        assert_eq!(Indentation::Spaces(0).unit(), " ");
        assert_eq!(Indentation::Tabs.width(8), 8);
        assert_eq!(Indentation::Spaces(3).width(8), 3);
        assert_eq!(Indentation::default(), Indentation::Spaces(4));
    }

    #[test]
    fn nothing_to_go_by() {
        assert_eq!(detect_text(""), None);
        assert_eq!(detect_text("fn main() {}\n"), None);
        assert_eq!(
            detect_text("a\n\n   \nb\n"),
            None,
            "blank lines don't count"
        );
        assert_eq!(
            detect_text("    a\n    b\n"),
            None,
            "no width without a change in indentation"
        );
    }

    #[test]
    fn four_spaces() {
        let text = "fn main() {\n    if x {\n        y();\n    }\n}\n";
        assert_eq!(detect_text(text), Some(Indentation::Spaces(4)));
    }

    #[test]
    fn two_spaces_with_aligned_continuations() {
        let text = "function f() {\n  return g(a,\n           b);\n  if (x) {\n    y();\n  }\n}\n";
        assert_eq!(detect_text(text), Some(Indentation::Spaces(2)));
    }

    #[test]
    fn tabs() {
        let text = "fn main() {\n\tif x {\n\t\ty();\n\t}\n}\n";
        assert_eq!(detect_text(text), Some(Indentation::Tabs));
    }

    #[test]
    fn mostly_tabs_with_some_spaces() {
        let text = "a\n\tb\n\tc\n    d\n\te\n";
        assert_eq!(detect_text(text), Some(Indentation::Tabs));
        let text = "a\n\tb\n  c\n  d\n    e\n";
        assert_eq!(detect_text(text), Some(Indentation::Spaces(2)));
    }

    #[test]
    fn mixed_lines_do_not_vote() {
        let text = "a\n  \tb\n  \tc\n";
        assert_eq!(detect_text(text), None);
    }

    #[test]
    fn ties_prefer_the_narrower_width() {
        let text = "a\n  b\n      c\n";
        assert_eq!(detect_text(text), Some(Indentation::Spaces(2)));
    }

    #[test]
    fn deltas_are_measured_across_blank_lines() {
        let text = "a\n\n    b\n\n\n        c\n";
        assert_eq!(detect_text(text), Some(Indentation::Spaces(4)));
    }
}
