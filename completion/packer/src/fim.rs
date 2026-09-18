//! Fill-in-the-middle transformation of a document.

use crate::rng::Rng;
use tokenizer::pretok::Indent;
use tokenizer::{Encoder, Tokenizer};

/// Longest completion-shaped middle, in lines.
const MAX_SHORT_LINES: usize = 12;

/// Cursor positions where a split may happen: character boundaries that are not inside a
/// line's leading indentation. The end of every line is included.
pub fn cursor_positions(text: &str) -> Vec<usize> {
    let mut positions = Vec::with_capacity(text.len() / 2);
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let ws = body.len() - body.trim_start().len();
        positions.extend(
            body.char_indices()
                .filter(|&(i, _)| i >= ws)
                .map(|(i, _)| offset + i),
        );
        positions.push(offset + body.len());
        offset += line.len();
    }
    positions
}

/// Chooses the middle span `[start, end)` of a document.
fn choose_span(text: &str, positions: &[usize], rng: &mut Rng) -> (usize, usize) {
    if rng.chance(0.5) {
        // Uniform random span, as in document-level FIM training.
        let a = positions[rng.below(positions.len())];
        let b = positions[rng.below(positions.len())];
        (a.min(b), a.max(b))
    } else {
        // Completion-shaped span: from a cursor to the end of one of the next few lines.
        let start = positions[rng.below(positions.len())];
        let lines = 1 + rng.below(MAX_SHORT_LINES);
        let mut end = text.len();
        let mut seen = 0;
        for (i, _) in text[start..].match_indices('\n') {
            seen += 1;
            if seen == lines {
                end = start + i;
                break;
            }
        }
        (start, end)
    }
}

/// Appends the FIM form of `text` to `out`. Returns false if the text has no usable split.
pub fn transform(
    tok: &Tokenizer,
    encoder: &mut Encoder,
    text: &str,
    indent: Indent,
    rng: &mut Rng,
    out: &mut Vec<u32>,
) -> bool {
    let positions = cursor_positions(text);
    if positions.len() < 2 {
        return false;
    }
    let (start, end) = choose_span(text, &positions, rng);
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
    if rng.chance(0.5) {
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
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_skip_indentation() {
        let text = "fn a() {\n    b();\n\n}\n";
        let positions = cursor_positions(text);
        // Line 2 starts at offset 9; its indentation occupies 9..13.
        assert!(!positions.contains(&10));
        assert!(positions.contains(&13));
        assert!(positions.contains(&17)); // end of "    b();"
        assert!(positions.contains(&18)); // the blank line
        assert!(positions.contains(&0));
    }
}
