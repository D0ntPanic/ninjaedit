//! Markdown (CommonMark with the usual GitHub extensions: tables, task
//! lists, strikethrough, bare URLs).
//!
//! Block structure is decided from the start of each line: headings,
//! block quotes, list items, thematic breaks, table rows, indented code,
//! and code fences. Fenced code whose info string names a language we
//! know is lexed by that language's lexer, whose state is kept above the
//! fence on the state stack; other fenced code and indented code is a
//! string. Everything else is prose, lexed inline for code spans,
//! emphasis, links, escapes, and HTML.
//!
//! Only two things carry between lines: whether the previous line was
//! prose (so an indented line continues it rather than starting a code
//! block, and `===` underlines it as a heading), and any open code fence
//! or HTML comment.

use super::{Context, Language, LexState, Lexer, Token, TokenKind};

pub(super) struct Markdown;

impl Lexer for Markdown {
    fn lex_line(&self, mut state: LexState, line: &[u8], out: &mut Vec<Token>) -> LexState {
        // Fenced code: close, or hand the line to the fenced language.
        if let Some(Context::Fence {
            fence,
            len,
            language,
        }) = state.bottom()
        {
            let indent = indent_of(line);
            let run = run_len(&line[indent..], fence);
            if indent < 4 && run >= len as usize && is_blank(&line[indent + run..]) {
                out.push(Token {
                    range: indent..indent + run,
                    kind: TokenKind::Punctuation,
                });
                return LexState::default();
            }
            return match language.and_then(Language::from_index) {
                Some(inner) if inner != Language::Markdown => {
                    let inner_state = inner.lexer().lex_line(state.without_bottom(), line, out);
                    inner_state.with_bottom(Context::Fence {
                        fence,
                        len,
                        language,
                    })
                }
                _ => {
                    if !line.is_empty() {
                        out.push(Token {
                            range: 0..line.len(),
                            kind: TokenKind::String,
                        });
                    }
                    state
                }
            };
        }

        let mut lx = Inline { line, out };
        let mut pos = 0;

        // An HTML comment continued from the previous line.
        if let Some(Context::BlockComment { .. }) = state.top() {
            match find(line, 0, b"-->") {
                Some(end) => {
                    lx.emit(0, end + 3, TokenKind::Comment);
                    state.pop();
                    pos = end + 3;
                }
                None => {
                    lx.emit(0, line.len(), TokenKind::Comment);
                    return state;
                }
            }
        }

        let paragraph = state.top() == Some(Context::Paragraph);
        let indent = indent_of(&line[pos..]) + pos;
        let rest = &line[indent..];

        if is_blank(rest) {
            // A blank line ends a paragraph.
            if pos == 0 {
                return LexState::default();
            }
            return state;
        }

        // Indented code, unless it continues a paragraph or list item.
        if indent - pos >= 4 && !paragraph && pos == 0 {
            lx.emit(0, line.len(), TokenKind::String);
            return LexState::default();
        }

        // Code fence.
        let fence_char = rest.first().copied();
        if let Some(fence) = fence_char
            && (fence == b'`' || fence == b'~')
            && run_len(rest, fence) >= 3
        {
            let run = run_len(rest, fence);
            let info = &rest[run..];
            // Backtick fences can't have backticks in their info string.
            if fence == b'~' || !info.contains(&b'`') {
                lx.emit(indent, indent + run, TokenKind::Punctuation);
                let info_start = indent + run + indent_of(info);
                let info_end = line.len() - trailing_blank(line);
                lx.emit(info_start, info_end.max(info_start), TokenKind::Attribute);
                let language = std::str::from_utf8(&line[info_start..info_end.max(info_start)])
                    .ok()
                    .and_then(Language::from_fence_info)
                    .map(Language::index);
                let mut state = LexState::default();
                state.push(Context::Fence {
                    fence,
                    len: run.min(255) as u8,
                    language,
                });
                return state;
            }
        }

        // ATX heading.
        let hashes = run_len(rest, b'#');
        if (1..=6).contains(&hashes) && matches!(rest.get(hashes), None | Some(b' ') | Some(b'\t'))
        {
            lx.lex_inline(&mut state, indent, TokenKind::Heading);
            return end_block(state);
        }

        // Thematic break, or a setext heading underline.
        if let Some(c) = fence_char
            && matches!(c, b'-' | b'*' | b'_')
            && rest.iter().all(|&b| b == c || b == b' ' || b == b'\t')
            && rest.iter().filter(|&&b| b == c).count() >= 3
        {
            let kind = if c == b'-' && paragraph {
                TokenKind::Heading
            } else {
                TokenKind::Punctuation
            };
            lx.emit(indent, line.len(), kind);
            return end_block(state);
        }
        if fence_char == Some(b'=') && paragraph && rest.iter().all(|&b| b == b'=' || b == b' ') {
            lx.emit(indent, line.len(), TokenKind::Heading);
            return end_block(state);
        }

        // Block quote: the markers, then the rest as quoted prose.
        if fence_char == Some(b'>') {
            let mut end = indent;
            while end < line.len() && matches!(line[end], b'>' | b' ' | b'\t') {
                end += 1;
            }
            lx.emit(indent, end, TokenKind::Quote);
            lx.lex_inline(&mut state, end, TokenKind::Quote);
            return start_paragraph(state);
        }

        // List item: bullet or number, then an optional task checkbox.
        if let Some(marker_end) = list_marker_end(rest) {
            let mut end = indent + marker_end;
            lx.emit(indent, end, TokenKind::ListMarker);
            let after = &line[end..];
            let spaces = indent_of(after);
            let task = &after[spaces..];
            if task.len() >= 3
                && task[0] == b'['
                && matches!(task[1], b' ' | b'x' | b'X')
                && task[2] == b']'
                && matches!(task.get(3), None | Some(b' ') | Some(b'\t'))
            {
                lx.emit(end + spaces, end + spaces + 3, TokenKind::ListMarker);
                end += spaces + 3;
            }
            lx.lex_inline(&mut state, end, TokenKind::Text);
            return start_paragraph(state);
        }

        // Table row.
        if fence_char == Some(b'|') {
            let delimiter_row = rest
                .iter()
                .all(|&b| matches!(b, b'|' | b'-' | b':' | b' ' | b'\t'));
            if delimiter_row {
                lx.emit(indent, line.len(), TokenKind::Punctuation);
            } else {
                let mut cell_start = indent;
                let mut i = indent;
                while i < line.len() {
                    match line[i] {
                        b'\\' => i += 2,
                        b'|' => {
                            lx.lex_inline_range(&mut state, cell_start, i, TokenKind::Text);
                            lx.emit(i, i + 1, TokenKind::Punctuation);
                            i += 1;
                            cell_start = i;
                        }
                        _ => i += 1,
                    }
                }
                lx.lex_inline_range(&mut state, cell_start, line.len(), TokenKind::Text);
            }
            return start_paragraph(state);
        }

        // Plain prose.
        lx.lex_inline(&mut state, pos, TokenKind::Text);
        start_paragraph(state)
    }
}

/// The state after a block that ends any paragraph.
fn end_block(mut state: LexState) -> LexState {
    if state.top() == Some(Context::Paragraph) {
        state.pop();
    }
    state
}

/// The state after a line of prose: in a paragraph, unless the line
/// opened an HTML comment (which then sits above the paragraph).
fn start_paragraph(mut state: LexState) -> LexState {
    match state.top() {
        Some(Context::Paragraph) => {}
        Some(Context::BlockComment { .. }) => {
            let comment = state.pop().unwrap();
            state.push(Context::Paragraph);
            state.push(comment);
        }
        _ => {
            state.push(Context::Paragraph);
        }
    }
    state
}

/// The width of leading blanks, counting a tab as reaching the next
/// multiple of four columns.
fn indent_of(bytes: &[u8]) -> usize {
    let mut i = 0;
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t') {
        i += 1;
    }
    i
}

fn trailing_blank(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .rev()
        .take_while(|&&b| matches!(b, b' ' | b'\t' | b'\r'))
        .count()
}

fn is_blank(bytes: &[u8]) -> bool {
    bytes.iter().all(|&b| matches!(b, b' ' | b'\t' | b'\r'))
}

fn run_len(bytes: &[u8], c: u8) -> usize {
    bytes.iter().take_while(|&&b| b == c).count()
}

fn find(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from > bytes.len() {
        return None;
    }
    bytes[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

/// The length of a list marker at the start of `rest` (`-`, `+`, `*`,
/// or a number followed by `.` or `)`), including the blank after it.
fn list_marker_end(rest: &[u8]) -> Option<usize> {
    let after_marker = match rest.first()? {
        b'-' | b'+' | b'*' => 1,
        b'0'..=b'9' => {
            let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
            if digits > 9 || !matches!(rest.get(digits), Some(b'.') | Some(b')')) {
                return None;
            }
            digits + 1
        }
        _ => return None,
    };
    match rest.get(after_marker) {
        None => Some(after_marker),
        Some(b' ') | Some(b'\t') => Some(after_marker + 1),
        _ => None,
    }
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// Inline lexing of prose. Gaps between the constructs found are given
/// the `base` kind (unless it is plain text), so a whole heading or quote
/// is styled as one.
struct Inline<'a> {
    line: &'a [u8],
    out: &'a mut Vec<Token>,
}

impl Inline<'_> {
    fn at(&self, i: usize) -> u8 {
        self.line.get(i).copied().unwrap_or(0)
    }

    fn emit(&mut self, start: usize, end: usize, kind: TokenKind) {
        if end > start {
            self.out.push(Token {
                range: start..end,
                kind,
            });
        }
    }

    fn lex_inline(&mut self, state: &mut LexState, from: usize, base: TokenKind) {
        self.lex_inline_range(state, from, self.line.len(), base);
    }

    fn lex_inline_range(&mut self, state: &mut LexState, from: usize, end: usize, base: TokenKind) {
        let mut gap = from;
        let mut i = from;
        while i < end {
            let Some((start, stop, kind)) = self.construct_at(state, i, end) else {
                i += 1;
                continue;
            };
            if base != TokenKind::Text {
                self.emit(gap, start, base);
            }
            self.emit(start, stop, kind);
            i = stop;
            gap = stop;
            if let Some(Context::BlockComment { .. }) = state.top() {
                // An unclosed HTML comment takes the rest of the line.
                return;
            }
        }
        if base != TokenKind::Text {
            self.emit(gap, end, base);
        }
    }

    /// The inline construct starting at `i`, if any, as its range and
    /// kind. May open an HTML comment in `state`.
    fn construct_at(
        &mut self,
        state: &mut LexState,
        i: usize,
        end: usize,
    ) -> Option<(usize, usize, TokenKind)> {
        let line = &self.line[..end];
        let b = line[i];
        match b {
            b'\\' if i + 1 < end && line[i + 1].is_ascii_punctuation() => {
                Some((i, i + 2, TokenKind::StringEscape))
            }
            b'`' => {
                let run = run_len(&line[i..], b'`');
                let mut j = i + run;
                while j < end {
                    if line[j] == b'`' {
                        let close = run_len(&line[j..], b'`');
                        if close == run {
                            return Some((i, j + run, TokenKind::String));
                        }
                        j += close;
                    } else {
                        j += 1;
                    }
                }
                None
            }
            b'<' => {
                if line[i..].starts_with(b"<!--") {
                    return Some(match find(line, i + 4, b"-->") {
                        Some(close) => (i, close + 3, TokenKind::Comment),
                        None => {
                            state.push(Context::BlockComment {
                                doc: false,
                                depth: 1,
                            });
                            (i, end, TokenKind::Comment)
                        }
                    });
                }
                let close = find(line, i, b">")?;
                let inner = &line[i + 1..close];
                if inner.is_empty()
                    || inner.iter().any(|b| b.is_ascii_whitespace())
                        && !inner[0].is_ascii_alphabetic()
                {
                    return None;
                }
                let autolink = inner.contains(&b':')
                    && !inner.iter().any(|b| b.is_ascii_whitespace())
                    && !inner.contains(&b'=');
                let tag = matches!(inner[0], b'/' | b'!') || inner[0].is_ascii_alphabetic();
                if autolink {
                    Some((i, close + 1, TokenKind::Link))
                } else if tag {
                    Some((i, close + 1, TokenKind::Attribute))
                } else {
                    None
                }
            }
            b'!' if self.at(i + 1) == b'[' => self.link_at(i, i + 1, end),
            b'[' => self.link_at(i, i, end),
            b'*' | b'_' => {
                let run = run_len(&line[i..], b).min(3);
                // Try the strongest delimiter that closes: `***`, `**`, `*`.
                for len in (1..=run).rev() {
                    if let Some(close) = self.emphasis_close(i, len, b, end) {
                        let kind = if len == 1 {
                            TokenKind::Emphasis
                        } else {
                            TokenKind::Strong
                        };
                        return Some((i, close + len, kind));
                    }
                }
                None
            }
            b'~' if line[i..].starts_with(b"~~") && !line[i..].starts_with(b"~~~") => {
                let close = find(line, i + 2, b"~~")?;
                (close > i + 2).then_some((i, close + 2, TokenKind::Comment))
            }
            b'h' if line[i..].starts_with(b"http://") || line[i..].starts_with(b"https://") => {
                let mut j = i;
                while j < end
                    && !line[j].is_ascii_whitespace()
                    && !matches!(line[j], b'<' | b'>' | b'"' | b'\'')
                {
                    j += 1;
                }
                while j > i
                    && matches!(
                        line[j - 1],
                        b'.' | b',' | b';' | b':' | b'!' | b'?' | b')' | b']'
                    )
                {
                    j -= 1;
                }
                (j > i + 8).then_some((i, j, TokenKind::Link))
            }
            _ => None,
        }
    }

    /// A link or image whose `[` is at `bracket`, with the construct
    /// starting at `start`.
    fn link_at(
        &mut self,
        start: usize,
        bracket: usize,
        end: usize,
    ) -> Option<(usize, usize, TokenKind)> {
        let line = &self.line[..end];
        // Find the matching close bracket.
        let mut depth = 0;
        let mut j = bracket;
        let close = loop {
            if j >= end {
                return None;
            }
            match line[j] {
                b'\\' => j += 1,
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        break j;
                    }
                }
                _ => {}
            }
            j += 1;
        };
        match line.get(close + 1) {
            Some(b'(') => {
                // Inline link: the destination is a string.
                let mut depth = 0;
                let mut k = close + 1;
                while k < end {
                    match line[k] {
                        b'\\' => k += 1,
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    k += 1;
                }
                if k >= end {
                    return None;
                }
                self.emit(start, close + 1, TokenKind::Link);
                Some((close + 1, k + 1, TokenKind::String))
            }
            Some(b'[') => {
                let ref_close = find(line, close + 1, b"]")?;
                Some((start, ref_close + 1, TokenKind::Link))
            }
            Some(b':') if indent_of(self.line) == start => {
                // Reference definition: `[label]: destination`.
                self.emit(start, close + 1, TokenKind::Link);
                self.emit(close + 1, close + 2, TokenKind::Punctuation);
                let dest = close + 2 + indent_of(&line[close + 2..]);
                Some((dest.min(end), end, TokenKind::String))
            }
            _ => None,
        }
    }

    /// Where a run of `len` delimiter characters `c` opened at `i` closes,
    /// if it can open emphasis and something closes it on the line.
    fn emphasis_close(&self, i: usize, len: usize, c: u8, end: usize) -> Option<usize> {
        let line = &self.line[..end];
        let after = *line.get(i + len)?;
        if after.is_ascii_whitespace() || after == c {
            return None;
        }
        let before = if i > 0 { line[i - 1] } else { b' ' };
        // Underscores inside words are literal (snake_case).
        if c == b'_' && is_word_byte(before) {
            return None;
        }
        let mut j = i + len + 1;
        while j + len <= end {
            if line[j..].starts_with(&line[i..i + len])
                && !line[j - 1].is_ascii_whitespace()
                && (j + len >= end || line[j + len] != c)
                && (c != b'_' || j + len >= end || !is_word_byte(line[j + len]))
            {
                return Some(j);
            }
            if line[j] == b'\\' {
                j += 1;
            }
            j += 1;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::check;
    use super::super::{Language, lex_text};
    use super::*;

    #[test]
    fn blocks() {
        check(
            Language::Markdown,
            "\
# Title with `code` and *em*
HHHHHHHHHHHHHssssssHHHHHIIII
Setext heading

==============
HHHHHHHHHHHHHH
---
ppp
> quoted **bold** text
QQQQQQQQQBBBBBBBBQQQQQ
- item with [link](http://x.y \"t\")
--          AAAAAAssssssssssssssss
1. numbered
---
- [x] done and [ ] not
-----
| a | b |
p   p   p
|---|:-:|
ppppppppp
| **c** | d |
p BBBBB p   p


    indented code
sssssssssssssssss
text

para

    not code, continues

```rust
pppaaaa
let x = \"s\"; // c
kkk v o sssp cccc
```
ppp
plain <b>tag</b> https://example.com/a. <!-- note
      aaa   aaaa AAAAAAAAAAAAAAAAAAAAA  ccccccccc
still --> after ~~gone~~ snake_case _em_ \\* ![img](a.png)
ccccccccc       cccccccc            IIII ee AAAAAAsssssss
[ref]: dest
AAAAAp ssss
[text][ref] and <https://auto.link> and __strong__ ***both***
AAAAAAAAAAA     AAAAAAAAAAAAAAAAAAA     BBBBBBBBBB BBBBBBBBBB
",
        );
    }

    #[test]
    fn fenced_code_uses_the_named_language() {
        let lines = lex_text(Language::Markdown, "```py\ndef f(): pass\n```\nafter\n");
        assert_eq!(lines[1][0].kind, TokenKind::Keyword);
        assert_eq!(lines[1][1].kind, TokenKind::FunctionDefinition);
        assert_eq!(
            lines[2],
            vec![Token {
                range: 0..3,
                kind: TokenKind::Punctuation
            }]
        );
        assert_eq!(lines[3], vec![]);

        // A fence in an unknown language is a string; a shorter fence
        // doesn't close it; the fenced language's own state carries
        // across lines.
        let lines = lex_text(
            Language::Markdown,
            "````text\n```\nx\n````\n```rust\n/* a\nb */ c\n```\n",
        );
        assert_eq!(
            lines[1],
            vec![Token {
                range: 0..3,
                kind: TokenKind::String
            }]
        );
        assert_eq!(lines[2][0].kind, TokenKind::String);
        assert_eq!(lines[3][0].kind, TokenKind::Punctuation);
        assert_eq!(
            lines[5],
            vec![Token {
                range: 0..4,
                kind: TokenKind::Comment
            }]
        );
        assert_eq!(
            lines[6][0],
            Token {
                range: 0..4,
                kind: TokenKind::Comment
            }
        );
        assert_eq!(lines[6][1].kind, TokenKind::Identifier);
        assert_eq!(lines[7][0].kind, TokenKind::Punctuation);
    }

    #[test]
    fn no_panics_on_odd_input() {
        let inputs: &[&[u8]] = &[
            b"", b"[", b"[]", b"[](", b"![", b"<", b"<!--", b"`", b"``", b"*", b"**", b"_", b"~~",
            b"http://", b"\\", b"|", b"1.", b"-", b"#", b"```", b"[a]:", b"    ", b"\xff",
        ];
        for input in inputs {
            for state in [LexState::default(), {
                let mut s = LexState::default();
                s.push(Context::Paragraph);
                s
            }] {
                let mut out = Vec::new();
                Markdown.lex_line(state, input, &mut out);
            }
        }
    }
}
