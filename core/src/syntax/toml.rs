//! TOML. Keys, table headers, and values are told apart by position: a
//! line starts with a key (or a header), `=` switches to a value, and
//! inside an inline table `,` switches back to a key. Multi-line strings
//! and arrays carry across lines in the state.

use super::{Context, LexState, Lexer, Token, TokenKind};

pub(super) struct Toml;

fn is_bare_key_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

impl Lexer for Toml {
    fn lex_line(&self, state: LexState, line: &[u8], out: &mut Vec<Token>) -> LexState {
        let first = out.len();
        let state = lex_line(state, line, out);
        // Opening quotes are emitted separately from string bodies; join
        // adjacent string tokens so a plain string is one token.
        let mut i = first + 1;
        while i < out.len() {
            if out[i].kind == TokenKind::String
                && out[i - 1].kind == TokenKind::String
                && out[i - 1].range.end == out[i].range.start
            {
                let end = out[i].range.end;
                out[i - 1].range.end = end;
                out.remove(i);
            } else {
                i += 1;
            }
        }
        state
    }
}

fn lex_line(mut state: LexState, line: &[u8], out: &mut Vec<Token>) -> LexState {
    {
        let at = |i: usize| line.get(i).copied().unwrap_or(0);
        let mut pos = 0;
        let emit = |start: usize, end: usize, kind: TokenKind, out: &mut Vec<Token>| {
            if end > start {
                out.push(Token {
                    range: start..end,
                    kind,
                });
            }
        };

        // Whether the next bare word is a key rather than a value.
        let mut expect_key = match state.top() {
            None => true,
            Some(Context::Value { close }) => close == b'}',
            Some(_) => false,
        };
        // Inside a table header, names are namespaces.
        let mut header = false;

        loop {
            // A string continued from a previous line.
            if let Some(Context::String {
                quote,
                triple,
                escapes,
                ..
            }) = state.top()
            {
                let mut segment = pos;
                let mut i = pos;
                let mut closed = false;
                while i < line.len() {
                    let b = line[i];
                    if escapes && b == b'\\' {
                        emit(segment, i, TokenKind::String, out);
                        let end = match at(i + 1) {
                            b'u' => i + 6,
                            b'U' => i + 10,
                            _ => i + 2,
                        }
                        .min(line.len());
                        emit(i, end, TokenKind::StringEscape, out);
                        segment = end;
                        i = end;
                        continue;
                    }
                    if b == quote && (!triple || (at(i + 1) == quote && at(i + 2) == quote)) {
                        let mut end = i + if triple { 3 } else { 1 };
                        // Up to two extra quotes may end a triple string.
                        while triple && at(end) == quote && end - i < 5 {
                            end += 1;
                        }
                        emit(segment, end, TokenKind::String, out);
                        pos = end;
                        closed = true;
                        break;
                    }
                    i += 1;
                }
                if !closed {
                    emit(segment, line.len(), TokenKind::String, out);
                    if !triple {
                        // A single-line string that didn't close: don't
                        // let it swallow the file.
                        state.pop();
                    }
                    return state;
                }
                state.pop();
                expect_key = false;
            }

            while pos < line.len() && matches!(line[pos], b' ' | b'\t' | b'\r') {
                pos += 1;
            }
            if pos >= line.len() {
                return state;
            }
            let start = pos;
            let b = line[pos];
            match b {
                b'#' => {
                    emit(start, line.len(), TokenKind::Comment, out);
                    return state;
                }
                b'[' if expect_key && state.top().is_none() && pos == line_indent(line) => {
                    // Table header: [name] or [[name]].
                    let mut end = pos + 1;
                    if at(end) == b'[' {
                        end += 1;
                    }
                    emit(start, end, TokenKind::Punctuation, out);
                    pos = end;
                    header = true;
                    expect_key = true;
                }
                b']' if header => {
                    let mut end = pos + 1;
                    if at(end) == b']' {
                        end += 1;
                    }
                    emit(start, end, TokenKind::Punctuation, out);
                    pos = end;
                    header = false;
                    expect_key = false;
                }
                b'"' | b'\'' => {
                    let triple = at(pos + 1) == b && at(pos + 2) == b;
                    let body = pos + if triple { 3 } else { 1 };
                    let escapes = b == b'"';
                    // Quoted keys are single-line and never triple.
                    if (expect_key || header) && !triple {
                        let mut i = body;
                        while i < line.len() && line[i] != b {
                            if escapes && line[i] == b'\\' {
                                i += 1;
                            }
                            i += 1;
                        }
                        let end = (i + 1).min(line.len());
                        let kind = if header {
                            TokenKind::Namespace
                        } else {
                            TokenKind::Key
                        };
                        emit(start, end, kind, out);
                        pos = end;
                        continue;
                    }
                    emit(start, body, TokenKind::String, out);
                    pos = body;
                    state.push(Context::String {
                        quote: b,
                        triple,
                        escapes,
                        hashes: 0,
                    });
                    // The string branch at the top of the loop lexes the
                    // body.
                }
                b'=' => {
                    emit(start, start + 1, TokenKind::Operator, out);
                    pos += 1;
                    expect_key = false;
                }
                b'.' => {
                    emit(start, start + 1, TokenKind::Punctuation, out);
                    pos += 1;
                }
                b',' => {
                    emit(start, start + 1, TokenKind::Punctuation, out);
                    pos += 1;
                    if let Some(Context::Value { close: b'}' }) = state.top() {
                        expect_key = true;
                    }
                }
                b'[' => {
                    emit(start, start + 1, TokenKind::Punctuation, out);
                    pos += 1;
                    state.push(Context::Value { close: b']' });
                    expect_key = false;
                }
                b'{' => {
                    emit(start, start + 1, TokenKind::Punctuation, out);
                    pos += 1;
                    state.push(Context::Value { close: b'}' });
                    expect_key = true;
                }
                b']' | b'}' => {
                    emit(start, start + 1, TokenKind::Punctuation, out);
                    pos += 1;
                    if let Some(Context::Value { close }) = state.top()
                        && close == b
                    {
                        state.pop();
                    }
                    expect_key = false;
                }
                _ if is_bare_key_char(b) || b == b'+' => {
                    let mut end = pos + 1;
                    if expect_key || header {
                        while end < line.len() && is_bare_key_char(line[end]) {
                            end += 1;
                        }
                        let kind = if header {
                            TokenKind::Namespace
                        } else {
                            TokenKind::Key
                        };
                        emit(start, end, kind, out);
                    } else {
                        // A value: number, date, time, boolean, inf, nan.
                        while end < line.len()
                            && (line[end].is_ascii_alphanumeric()
                                || matches!(line[end], b'_' | b'-' | b'+' | b'.' | b':'))
                        {
                            end += 1;
                        }
                        // A date and a time may be separated by a space.
                        if end + 1 < line.len()
                            && line[end] == b' '
                            && line[end + 1].is_ascii_digit()
                            && line[start..end].len() == 10
                            && line[start + 4] == b'-'
                        {
                            end += 1;
                            while end < line.len()
                                && (line[end].is_ascii_alphanumeric()
                                    || matches!(line[end], b'-' | b'+' | b'.' | b':'))
                            {
                                end += 1;
                            }
                        }
                        let word = &line[start..end];
                        let kind = match word {
                            b"true" | b"false" => TokenKind::Constant,
                            b"inf" | b"nan" | b"+inf" | b"-inf" | b"+nan" | b"-nan" => {
                                TokenKind::Number
                            }
                            _ if word[0].is_ascii_digit()
                                || (matches!(word[0], b'+' | b'-') && word.len() > 1) =>
                            {
                                TokenKind::Number
                            }
                            _ => TokenKind::Invalid,
                        };
                        emit(start, end, kind, out);
                    }
                    pos = end;
                }
                _ => {
                    let mut end = pos + 1;
                    while end < line.len() && (line[end] & 0xC0) == 0x80 {
                        end += 1;
                    }
                    emit(start, end, TokenKind::Invalid, out);
                    pos = end;
                }
            }
        }
    }
}

/// The offset of the first non-blank byte of a line.
fn line_indent(line: &[u8]) -> usize {
    line.iter()
        .position(|b| !matches!(b, b' ' | b'\t'))
        .unwrap_or(line.len())
}

#[cfg(test)]
mod tests {
    use super::super::{Language, lex_text};
    use super::*;

    fn kinds(text: &str) -> Vec<Vec<(TokenKind, &str)>> {
        lex_text(Language::Toml, text)
            .into_iter()
            .zip(text.lines())
            .map(|(tokens, line)| {
                tokens
                    .into_iter()
                    .map(|t| (t.kind, &line[t.range]))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn tables_keys_and_values() {
        let lines = kinds(
            "[package.\"sub\"] # c\nname = \"x\\ty\" # done\n[[bin]]\na.b = [1, -2.5, 0xff, true, inf, 1979-05-27 07:32:00Z]\nt = { k = 'v', 'q k' = 3 }\n",
        );
        assert_eq!(
            lines[0],
            vec![
                (TokenKind::Punctuation, "["),
                (TokenKind::Namespace, "package"),
                (TokenKind::Punctuation, "."),
                (TokenKind::Namespace, "\"sub\""),
                (TokenKind::Punctuation, "]"),
                (TokenKind::Comment, "# c"),
            ]
        );
        assert_eq!(
            lines[1],
            vec![
                (TokenKind::Key, "name"),
                (TokenKind::Operator, "="),
                (TokenKind::String, "\"x"),
                (TokenKind::StringEscape, "\\t"),
                (TokenKind::String, "y\""),
                (TokenKind::Comment, "# done"),
            ]
        );
        assert_eq!(lines[2][0], (TokenKind::Punctuation, "[["));
        assert_eq!(lines[2][1], (TokenKind::Namespace, "bin"));
        assert_eq!(lines[2][2], (TokenKind::Punctuation, "]]"));
        assert_eq!(lines[3][0], (TokenKind::Key, "a"));
        assert_eq!(lines[3][2], (TokenKind::Key, "b"));
        assert_eq!(lines[3][5], (TokenKind::Number, "1"));
        assert_eq!(lines[3][7], (TokenKind::Number, "-2.5"));
        assert_eq!(lines[3][9], (TokenKind::Number, "0xff"));
        assert_eq!(lines[3][11], (TokenKind::Constant, "true"));
        assert_eq!(lines[3][13], (TokenKind::Number, "inf"));
        assert_eq!(lines[3][15], (TokenKind::Number, "1979-05-27 07:32:00Z"));
        assert_eq!(lines[4][3], (TokenKind::Key, "k"));
        assert_eq!(lines[4][5], (TokenKind::String, "'v'"));
        assert_eq!(lines[4][7], (TokenKind::Key, "'q k'"));
        assert_eq!(lines[4][9], (TokenKind::Number, "3"));
    }

    #[test]
    fn multi_line_strings_and_arrays() {
        let lines = kinds("s = \"\"\"one\ntwo\"\"\" # c\na = [\n  1,\n  \"x\",\n]\nb = 2\n");
        assert_eq!(lines[0][2], (TokenKind::String, "\"\"\"one"));
        assert_eq!(lines[1][0], (TokenKind::String, "two\"\"\""));
        assert_eq!(lines[1][1], (TokenKind::Comment, "# c"));
        assert_eq!(lines[3][0], (TokenKind::Number, "1"));
        assert_eq!(lines[4][0], (TokenKind::String, "\"x\""));
        assert_eq!(lines[5][0], (TokenKind::Punctuation, "]"));
        assert_eq!(lines[6][0], (TokenKind::Key, "b"));

        // An unterminated single-line string doesn't leak into the next
        // line.
        let lines = kinds("s = \"open\nb = 2\n");
        assert_eq!(lines[1][0], (TokenKind::Key, "b"));
    }
}
