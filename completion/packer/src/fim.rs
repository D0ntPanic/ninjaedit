//! Fill-in-the-middle transformation of a document.

use crate::rng::Rng;
use tokenizer::pretok::Indent;
use tokenizer::{Encoder, Tokenizer};

/// Cursor positions where a split may happen: pre-token boundaries, which include the end of
/// every line and the end of the text. Inference backs the context up to the last pre-token
/// boundary and constrains generation over whatever was typed past it, so the model only ever
/// sees prefixes that end on a boundary; splitting there also keeps the prefix, middle and
/// suffix tokenizing exactly as the whole document does.
pub fn cursor_positions(text: &str, indent: Indent) -> Vec<usize> {
    let mut positions = tokenizer::pretok::piece_starts(text, indent);
    if positions.last() != Some(&text.len()) {
        positions.push(text.len());
    }
    positions
}

/// How the middle of a document is chosen. The mix favours what an editor asks for: the
/// rest of the current line, a few lines, or the rest of the enclosing block, with a share of
/// arbitrary spans for variety.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    /// From the cursor to the end of its line.
    Line,
    /// From the cursor to the end of a following line, mostly two to four lines.
    Lines,
    /// From the cursor to the end of the enclosing block: up to the line before the next
    /// line indented less than the cursor's line.
    Block,
    /// Two uniformly random cursor positions.
    Uniform,
}

impl SpanKind {
    pub const ALL: [SpanKind; 4] = [
        SpanKind::Line,
        SpanKind::Lines,
        SpanKind::Block,
        SpanKind::Uniform,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SpanKind::Line => "line",
            SpanKind::Lines => "lines",
            SpanKind::Block => "block",
            SpanKind::Uniform => "uniform",
        }
    }
}

/// Percent of FIM documents given each span kind, in `SpanKind::ALL` order.
const SPAN_WEIGHTS: [usize; 4] = [35, 25, 25, 15];
/// Longest `Lines` span; the count decays geometrically from two.
const MAX_LINES: usize = 12;

fn pick_kind(rng: &mut Rng) -> SpanKind {
    let mut roll = rng.below(100);
    for (kind, weight) in SpanKind::ALL.iter().zip(SPAN_WEIGHTS) {
        if roll < weight {
            return *kind;
        }
        roll -= weight;
    }
    SpanKind::Uniform
}

/// Offset of the newline ending the line that contains `pos`, or the text length.
fn line_end(text: &str, pos: usize) -> usize {
    text[pos..]
        .find('\n')
        .map(|i| pos + i)
        .unwrap_or(text.len())
}

/// Offset of the `n`th newline at or after `pos`, or the text length.
fn nth_line_end(text: &str, pos: usize, n: usize) -> usize {
    text[pos..]
        .match_indices('\n')
        .nth(n - 1)
        .map(|(i, _)| pos + i)
        .unwrap_or(text.len())
}

fn indent_width(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// End of the block containing `pos`: the end of the last non-blank line before the next
/// non-blank line indented less than `pos`'s line.
fn block_end(text: &str, pos: usize) -> usize {
    let line_start = text[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let base = indent_width(&text[line_start..line_end(text, line_start)]);
    let mut end = line_end(text, pos);
    let mut offset = end;
    while offset < text.len() {
        let next_start = offset + 1;
        let next_end = line_end(text, next_start);
        let line = &text[next_start..next_end];
        if !line.trim().is_empty() {
            if indent_width(line) < base {
                break;
            }
            end = next_end;
        }
        offset = next_end;
    }
    end
}

/// Chooses the middle span `[start, end)` of a document and reports its kind.
fn choose_span(text: &str, positions: &[usize], rng: &mut Rng) -> (usize, usize, SpanKind) {
    for _ in 0..4 {
        let kind = pick_kind(rng);
        let start = positions[rng.below(positions.len())];
        let end = match kind {
            SpanKind::Line => line_end(text, start),
            SpanKind::Lines => {
                let mut n = 2;
                while n < MAX_LINES && rng.chance(0.5) {
                    n += 1;
                }
                nth_line_end(text, start, n)
            }
            SpanKind::Block => block_end(text, start),
            SpanKind::Uniform => {
                let b = positions[rng.below(positions.len())];
                let (a, b) = (start.min(b), start.max(b));
                if a < b {
                    return (a, b, kind);
                }
                continue;
            }
        };
        // A cursor at the end of a line gives an empty middle; draw again.
        if end > start {
            return (start, end, kind);
        }
    }
    let a = positions[rng.below(positions.len())];
    let b = positions[rng.below(positions.len())];
    (
        a.min(b),
        a.max(b).max(a.min(b) + 1).min(text.len()),
        SpanKind::Uniform,
    )
}

/// Tokens `frame` puts before the text.
pub const FRAME_TOKENS: usize = 3;

/// Appends a plain document in the SPM layout with an empty suffix and the whole text as the
/// prefix: `<fim_prefix> <fim_suffix> <fim_middle> T <eom>`. That is the prompt the editor
/// sends with the cursor at the end of a file, so plain documents train on the format the
/// model is used in: the header is closed by `<fim_prefix>` as in every prompt, the text is
/// predicted as a continuation after `<fim_middle>`, and the end of the file teaches the model
/// where a completion stops.
pub fn frame(
    tok: &Tokenizer,
    encoder: &mut Encoder,
    text: &str,
    indent: Indent,
    out: &mut Vec<u32>,
) {
    out.extend([
        tok.special("<fim_prefix>"),
        tok.special("<fim_suffix>"),
        tok.special("<fim_middle>"),
    ]);
    encoder.encode_with(text, indent, out);
    out.push(tok.special("<eom>"));
}

/// How a FIM document was made.
#[derive(Clone, Copy, Debug)]
pub struct Fim {
    pub kind: SpanKind,
    /// The PSM layout rather than SPM.
    pub psm: bool,
}

/// Appends the FIM form of `text` to `out`, in the PSM layout with probability `psm_rate` and
/// otherwise SPM. Returns how it was made, or `None` if the text has no usable split.
pub fn transform(
    tok: &Tokenizer,
    encoder: &mut Encoder,
    text: &str,
    indent: Indent,
    psm_rate: f64,
    rng: &mut Rng,
    out: &mut Vec<u32>,
) -> Option<Fim> {
    let positions = cursor_positions(text, indent);
    if positions.len() < 2 {
        return None;
    }
    let (start, end, kind) = choose_span(text, &positions, rng);
    // The fallback span may end one byte past a position, inside a multibyte character.
    if !text.is_char_boundary(end) {
        return None;
    }
    let (prefix, middle, suffix) = (&text[..start], &text[start..end], &text[end..]);
    let mut p = Vec::new();
    let mut m = Vec::new();
    let mut s = Vec::new();
    encoder.encode_with(prefix, indent, &mut p);
    encoder.encode_with(middle, indent, &mut m);
    encoder.encode_with(suffix, indent, &mut s);
    let fim_prefix = tok.special("<fim_prefix>");
    let fim_suffix = tok.special("<fim_suffix>");
    let fim_middle = tok.special("<fim_middle>");
    let eom = tok.special("<eom>");
    let psm = rng.chance(psm_rate);
    if psm {
        // PSM: <fim_prefix> P <fim_suffix> S <fim_middle> M <eom>
        out.push(fim_prefix);
        out.extend_from_slice(&p);
        out.push(fim_suffix);
        out.extend_from_slice(&s);
        out.push(fim_middle);
        out.extend_from_slice(&m);
    } else {
        // SPM: <fim_prefix> <fim_suffix> S <fim_middle> P M <eom>
        // The prefix comes last so the model continues straight from it into the middle,
        // which is the layout inference uses to keep the prefix cache warm.
        out.push(fim_prefix);
        out.push(fim_suffix);
        out.extend_from_slice(&s);
        out.push(fim_middle);
        out.extend_from_slice(&p);
        out.extend_from_slice(&m);
    }
    out.push(eom);
    Some(Fim { kind, psm })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_spans_end_before_the_dedent() {
        let text = "fn a() {\n    let x = 1;\n    if x > 0 {\n        y();\n    }\n\n    z();\n}\n";
        // Cursor at `let`: the block runs through `z();`, before the closing brace.
        let start = text.find("let").unwrap();
        assert_eq!(
            &text[start..block_end(text, start)],
            "let x = 1;\n    if x > 0 {\n        y();\n    }\n\n    z();"
        );
        // Cursor inside the `if`: the block is just `y();`.
        let start = text.find("y()").unwrap();
        assert_eq!(&text[start..block_end(text, start)], "y();");
        // Line and lines spans stop before the newline.
        assert_eq!(&text[start..line_end(text, start)], "y();");
        assert_eq!(&text[start..nth_line_end(text, start, 2)], "y();\n    }");
    }

    #[test]
    fn positions_are_pre_token_boundaries() {
        let text = "fn a() {\n    b();\n\n}\n";
        let positions = cursor_positions(text, Indent::Spaces(4));
        // Line 2 starts at offset 9; its indentation occupies 9..13.
        assert!(!positions.contains(&10));
        assert!(positions.contains(&13));
        assert!(!positions.contains(&15)); // inside `();`
        assert!(positions.contains(&17)); // end of "    b();"
        assert!(positions.contains(&18)); // the blank line
        assert!(positions.contains(&0));
        assert_eq!(positions.last(), Some(&text.len()));
    }
}
