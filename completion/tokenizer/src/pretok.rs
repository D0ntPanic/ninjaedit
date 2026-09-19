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
        let rest = if i > 0 {
            let (level, consumed) = line_indent(bytes, indent);
            out.push(Piece::Newline(level));
            consumed
        } else {
            // The first line has no preceding line break; keep any indentation literal.
            0
        };
        line_pieces(&bytes[rest..], &mut out);
    }
    out
}

/// The indent level of a line and the number of leading bytes that level accounts for.
/// Indentation beyond the unit or the maximum level stays literal.
fn line_indent(line: &[u8], indent: Indent) -> (usize, usize) {
    let tabs = line.iter().take_while(|&&c| c == b'\t').count();
    let spaces = line[tabs..].iter().take_while(|&&c| c == b' ').count();
    let (level, remainder) = match indent {
        Indent::Spaces(unit) => (tabs + spaces / unit, spaces % unit),
        Indent::Tabs => (tabs, spaces),
    };
    let extra = level.saturating_sub(MAX_INDENT);
    let literal = remainder
        + match indent {
            Indent::Spaces(unit) => extra * unit,
            Indent::Tabs => extra,
        };
    (level - extra, tabs + spaces - literal)
}

/// Byte offsets where every pre-token of `text` starts: the `\n` of each line break and the first
/// byte of each run. These are the cursor positions at which the text so far encodes exactly as
/// it does within the whole, so a document split at one of them tokenizes as the concatenation
/// of its parts. Positions inside indentation are not included.
pub fn piece_starts(text: &str, indent: Indent) -> Vec<usize> {
    let mut out = Vec::with_capacity(text.len() / 4);
    let mut offset = 0;
    for (i, line) in text.split('\n').enumerate() {
        let bytes = line.as_bytes();
        let rest = if i > 0 {
            out.push(offset - 1);
            line_indent(bytes, indent).1
        } else {
            0
        };
        line_spans(&bytes[rest..], |start, _| out.push(offset + rest + start));
        offset += line.len() + 1;
    }
    out
}

/// Start of the last pre-token of `text`, which may still be incomplete: more typing could
/// extend it and change its encoding. Everything before this offset encodes exactly as it will
/// once the text is finished, because BPE merges never cross pre-token boundaries. When the
/// last line holds nothing but indentation, the last pre-token is its line break, so the offset
/// is that of the `\n`; an editor completing from here treats the line break and the typed
/// indentation as one partial token.
pub fn last_piece_start(text: &str, indent: Indent) -> usize {
    let line_start = text.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line = &text.as_bytes()[line_start..];
    if line_start > 0 {
        let (_, consumed) = line_indent(line, indent);
        let body = &line[consumed..];
        if body.iter().all(|&c| c == b' ' || c == b'\t') {
            return line_start - 1;
        }
        let mut last = 0;
        line_spans(body, |start, _| last = start);
        line_start + consumed + last
    } else {
        let mut last = 0;
        line_spans(line, |start, _| last = start);
        last
    }
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
    line_spans(line, |start, end| out.push(Piece::Bytes(&line[start..end])));
}

/// Calls `emit` with the byte range of each run of a line, in order.
fn line_spans(line: &[u8], mut emit: impl FnMut(usize, usize)) {
    let mut i = 0;
    while i < line.len() {
        let c = line[i];
        if c == b' ' {
            let run = line[i..].iter().take_while(|&&c| c == b' ').count();
            if i + run == line.len() {
                emit(i, line.len());
                break;
            }
            if run > 1 {
                emit(i, i + run - 1);
            }
            let start = i + run - 1;
            i += run;
            let end = run_end(line, i);
            emit(start, end);
            i = end;
        } else {
            let end = run_end(line, i);
            emit(i, end);
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

    #[test]
    fn piece_starts_split_the_text_into_its_pieces() {
        let text = "fn a() {\n    let x = a::b(1,\n          2);\n\n}\n";
        let indent = Indent::Spaces(4);
        let starts = piece_starts(text, indent);
        assert_eq!(
            starts,
            [
                0, 2, 4, 6, 8, 13, 16, 18, 20, 22, 24, 25, 26, 27, 28, 37, 38, 40, 42, 43, 44, 45
            ]
        );
        // Splitting at any piece start leaves both parts tokenizing as they do in the whole.
        let whole = pretokenize(text, indent);
        for &at in &starts {
            let mut parts = pretokenize(&text[..at], indent);
            parts.extend(pretokenize(&text[at..], indent));
            assert_eq!(parts, whole, "split at {at}");
        }
        // Anywhere else changes the pieces (the end of the text trivially does not).
        for at in 0..text.len() {
            if !starts.contains(&at) {
                let mut parts = pretokenize(&text[..at], indent);
                parts.extend(pretokenize(&text[at..], indent));
                assert_ne!(parts, whole, "split at {at}");
            }
        }
        // The last start agrees with `last_piece_start` when the text ends in a run.
        for prefix in ["fn a() {\n    let x", "fn a() {\n    let x =", "fn a() {\n"] {
            assert_eq!(
                piece_starts(prefix, indent).last().copied(),
                Some(last_piece_start(prefix, indent))
            );
        }
    }

    #[test]
    fn last_piece_start_backs_up_to_the_pre_token() {
        let four = Indent::Spaces(4);
        assert_eq!(last_piece_start("", four), 0);
        assert_eq!(last_piece_start("fn", four), 0);
        assert_eq!(last_piece_start("fn main", four), 2);
        assert_eq!(last_piece_start("fn main(", four), 7);
        // A trailing space is the start of the next run.
        assert_eq!(last_piece_start("let x = ", four), 7);
        assert_eq!(last_piece_start("a  ", four), 1);
        assert_eq!(last_piece_start("fn a() {\n    b", four), 13);
        // Aligned continuation: the extra space is its own run, `2` carries one space.
        assert_eq!(last_piece_start("x(1,\n      2", four), 10);
        // A line holding only indentation backs up to the line break.
        assert_eq!(last_piece_start("fn a() {\n    ", four), 8);
        assert_eq!(last_piece_start("fn a() {\n  ", four), 8);
        assert_eq!(last_piece_start("fn a() {\n", four), 8);
        assert_eq!(last_piece_start("fn a() {\n\t", Indent::Tabs), 8);
        // Everything before the boundary encodes as it will once the text is finished.
        let text = "fn a() {\n    let x = Option";
        let at = last_piece_start(text, four);
        assert_eq!(&text[at..], " Option");
        let full = pretokenize(text, four);
        let head = pretokenize(&text[..at], four);
        assert_eq!(&full[..full.len() - 1], &head[..]);
    }
}
