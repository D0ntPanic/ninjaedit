//! Splits source text into pre-tokens: line breaks that carry the indentation level of the
//! next line, and byte runs that BPE merges never cross.
//!
//! Indentation is encoded structurally rather than as literal whitespace. The indent unit of
//! each file is detected from the code itself, so a two-space file, a four-space file and a
//! tab-indented file produce identical tokens. Whatever renders the tokens back to text chooses
//! the indentation style, which is how user formatting preferences apply to model output.

/// Highest indent level that has its own line-break token. Deeper lines keep the excess as
/// literal whitespace.
pub const MAX_INDENT: usize = 31;

/// The indentation unit of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indent {
    Spaces(usize),
    Tabs,
}

/// One pre-token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    /// A line break followed by a line at this indent level.
    Newline(usize),
    /// A run of bytes to be encoded with BPE.
    Bytes(&'a [u8]),
}

/// Detects a file's indent unit from the most common indentation increase after a line that
/// opens a block. Falls back to four spaces for files without blocks.
pub fn detect_indent(text: &str) -> Indent {
    let mut deltas = [0usize; 9];
    let mut tab_lines = 0usize;
    let mut prev: Option<(usize, bool)> = None;
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let bytes = line.as_bytes();
        let leading_tabs = bytes.iter().take_while(|&&c| c == b'\t').count();
        let leading = bytes[leading_tabs..]
            .iter()
            .take_while(|&&c| c == b' ')
            .count();
        if leading_tabs > 0 {
            tab_lines += 1;
        }
        if let Some((prev_indent, opens)) = prev
            && opens
            && leading_tabs == 0
        {
            let delta = leading.saturating_sub(prev_indent);
            if (1..=8).contains(&delta) {
                deltas[delta] += 1;
            }
        }
        let opens = line.trim_end().ends_with(['{', '(', '[']);
        prev = Some((leading, opens));
    }
    let (best, count) = deltas
        .iter()
        .enumerate()
        .skip(1)
        .max_by_key(|&(delta, &count)| (count, std::cmp::Reverse(delta)))
        .map(|(d, &c)| (d, c))
        .unwrap_or((4, 0));
    if tab_lines > count {
        Indent::Tabs
    } else if count == 0 {
        Indent::Spaces(4)
    } else {
        Indent::Spaces(best)
    }
}

/// Splits text into pieces. The text is expected to be normalized (no trailing whitespace on
/// lines, `\n` line endings).
pub fn pretokenize(text: &str, indent: Indent) -> Vec<Piece<'_>> {
    let mut out = Vec::with_capacity(text.len() / 4);
    for (i, line) in text.split('\n').enumerate() {
        let bytes = line.as_bytes();
        let tabs = bytes.iter().take_while(|&&c| c == b'\t').count();
        let spaces = bytes[tabs..].iter().take_while(|&&c| c == b' ').count();
        let (mut level, remainder) = match indent {
            Indent::Spaces(unit) => (tabs + spaces / unit, spaces % unit),
            Indent::Tabs => (tabs, spaces),
        };
        let mut rest = tabs + spaces;
        let mut extra = 0;
        if level > MAX_INDENT {
            extra = level - MAX_INDENT;
            level = MAX_INDENT;
        }
        if i > 0 {
            out.push(Piece::Newline(level));
            // Excess indentation beyond the unit or the maximum level stays literal.
            let literal = remainder
                + match indent {
                    Indent::Spaces(unit) => extra * unit,
                    Indent::Tabs => extra,
                };
            rest -= literal;
        } else {
            // The first line has no preceding line break; keep any indentation literal.
            rest = 0;
        }
        line_pieces(&bytes[rest..], &mut out);
    }
    out
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

fn is_punct(c: u8) -> bool {
    c.is_ascii_punctuation()
}

/// Splits one line's bytes into word, punctuation and whitespace runs. A single space before a
/// run is attached to it, so `let x` becomes `let` and ` x`.
fn line_pieces<'a>(line: &'a [u8], out: &mut Vec<Piece<'a>>) {
    let mut i = 0;
    while i < line.len() {
        let c = line[i];
        if c == b' ' {
            let run = line[i..].iter().take_while(|&&c| c == b' ').count();
            if i + run == line.len() {
                out.push(Piece::Bytes(&line[i..]));
                break;
            }
            if run > 1 {
                out.push(Piece::Bytes(&line[i..i + run - 1]));
            }
            let start = i + run - 1;
            i += run;
            let end = run_end(line, i);
            out.push(Piece::Bytes(&line[start..end]));
            i = end;
        } else {
            let end = run_end(line, i);
            out.push(Piece::Bytes(&line[i..end]));
            i = end;
        }
    }
}

/// End of the run starting at `i`: a word, a punctuation run, or a single other byte.
fn run_end(line: &[u8], i: usize) -> usize {
    let c = line[i];
    let mut end = i + 1;
    if is_word(c) {
        while end < line.len() && is_word(line[end]) {
            end += 1;
        }
    } else if is_punct(c) {
        while end < line.len() && is_punct(line[end]) {
            end += 1;
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(text: &str) -> Vec<String> {
        pretokenize(text, detect_indent(text))
            .into_iter()
            .map(|p| match p {
                Piece::Newline(l) => format!("<nl:{l}>"),
                Piece::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            })
            .collect()
    }

    #[test]
    fn detects_units() {
        assert_eq!(detect_indent("fn a() {\n    b();\n}\n"), Indent::Spaces(4));
        assert_eq!(
            detect_indent("fn a() {\n  b();\n  if c {\n    d();\n  }\n}\n"),
            Indent::Spaces(2)
        );
        assert_eq!(detect_indent("fn a() {\n\tb();\n}\n"), Indent::Tabs);
        assert_eq!(detect_indent("fn a();\n"), Indent::Spaces(4));
    }

    #[test]
    fn splits_lines_and_words() {
        assert_eq!(
            pieces("fn main() {\n    let x = a::b(1);\n}\n"),
            [
                "fn", " main", "()", " {", "<nl:1>", "let", " x", " =", " a", "::", "b", "(", "1",
                ");", "<nl:0>", "}", "<nl:0>"
            ]
        );
    }

    #[test]
    fn same_tokens_for_tabs_and_spaces() {
        let spaces = "fn a() {\n    if b {\n        c();\n    }\n}\n";
        let tabs = "fn a() {\n\tif b {\n\t\tc();\n\t}\n}\n";
        let two = "fn a() {\n  if b {\n    c();\n  }\n}\n";
        assert_eq!(pieces(spaces), pieces(tabs));
        assert_eq!(pieces(spaces), pieces(two));
    }

    #[test]
    fn keeps_alignment_and_multiple_spaces() {
        assert_eq!(
            pieces("fn a() {\n    x(1,\n      2);\n}\n"),
            [
                "fn", " a", "()", " {", "<nl:1>", "x", "(", "1", ",", "<nl:1>", " ", " 2", ");",
                "<nl:0>", "}", "<nl:0>"
            ]
        );
        assert_eq!(pieces("a   b\n"), ["a", "  ", " b", "<nl:0>"]);
    }
}
