//! JSON, leniently: comments and trailing commas are accepted as in JSONC.
//! A string followed by a colon is a key.

use super::{Context, LexState, Lexer, Token, TokenKind};

pub(super) struct Json;

impl Lexer for Json {
    fn lex_line(&self, mut state: LexState, line: &[u8], out: &mut Vec<Token>) -> LexState {
        let mut pos = 0;
        let at = |i: usize| line.get(i).copied().unwrap_or(0);
        // Where the current block comment's token starts, when it opened
        // on this line.
        let mut comment_start = None;
        loop {
            if let Some(Context::BlockComment { .. }) = state.top() {
                let start = comment_start.take().unwrap_or(pos);
                match line[pos..].windows(2).position(|w| w == b"*/") {
                    Some(i) => {
                        pos += i + 2;
                        out.push(Token {
                            range: start..pos,
                            kind: TokenKind::Comment,
                        });
                        state.pop();
                    }
                    None => {
                        if start < line.len() {
                            out.push(Token {
                                range: start..line.len(),
                                kind: TokenKind::Comment,
                            });
                        }
                        return state;
                    }
                }
            }
            while pos < line.len() && line[pos].is_ascii_whitespace() {
                pos += 1;
            }
            if pos >= line.len() {
                return state;
            }
            let start = pos;
            let b = line[pos];
            match b {
                b'/' if at(pos + 1) == b'/' => {
                    out.push(Token {
                        range: start..line.len(),
                        kind: TokenKind::Comment,
                    });
                    return state;
                }
                b'/' if at(pos + 1) == b'*' => {
                    pos += 2;
                    comment_start = Some(start);
                    state.push(Context::BlockComment {
                        doc: false,
                        depth: 1,
                    });
                }
                b'"' => {
                    let mut segment = start;
                    let mut i = pos + 1;
                    let mut closed = false;
                    let mut pieces: Vec<Token> = Vec::new();
                    while i < line.len() {
                        match line[i] {
                            b'\\' => {
                                let end = if at(i + 1) == b'u' {
                                    (i + 6).min(line.len())
                                } else {
                                    (i + 2).min(line.len())
                                };
                                pieces.push(Token {
                                    range: segment..i,
                                    kind: TokenKind::String,
                                });
                                pieces.push(Token {
                                    range: i..end,
                                    kind: TokenKind::StringEscape,
                                });
                                segment = end;
                                i = end;
                            }
                            b'"' => {
                                i += 1;
                                closed = true;
                                break;
                            }
                            _ => i += 1,
                        }
                    }
                    pieces.push(Token {
                        range: segment..i,
                        kind: TokenKind::String,
                    });
                    pos = i;
                    // A key is a string followed by a colon.
                    let mut j = pos;
                    while j < line.len() && line[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    let kind = if closed && at(j) == b':' {
                        TokenKind::Key
                    } else {
                        TokenKind::String
                    };
                    for mut piece in pieces {
                        if piece.kind == TokenKind::String {
                            piece.kind = kind;
                        }
                        if !piece.range.is_empty() {
                            out.push(piece);
                        }
                    }
                }
                b'-' | b'0'..=b'9' => {
                    let mut i = pos + 1;
                    while i < line.len()
                        && (line[i].is_ascii_alphanumeric()
                            || matches!(line[i], b'.' | b'+' | b'-'))
                    {
                        i += 1;
                    }
                    out.push(Token {
                        range: start..i,
                        kind: TokenKind::Number,
                    });
                    pos = i;
                }
                b'{' | b'}' | b'[' | b']' | b',' | b':' => {
                    out.push(Token {
                        range: start..start + 1,
                        kind: TokenKind::Punctuation,
                    });
                    pos += 1;
                }
                _ if b.is_ascii_alphabetic() => {
                    let mut i = pos + 1;
                    while i < line.len() && (line[i].is_ascii_alphanumeric() || line[i] == b'_') {
                        i += 1;
                    }
                    let kind = match &line[start..i] {
                        b"true" | b"false" | b"null" | b"NaN" | b"Infinity" => TokenKind::Constant,
                        _ => TokenKind::Invalid,
                    };
                    out.push(Token {
                        range: start..i,
                        kind,
                    });
                    pos = i;
                }
                _ => {
                    let mut i = pos + 1;
                    while i < line.len() && (line[i] & 0xC0) == 0x80 {
                        i += 1;
                    }
                    out.push(Token {
                        range: start..i,
                        kind: TokenKind::Invalid,
                    });
                    pos = i;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Language, lex_text};
    use super::*;

    fn kinds(text: &str) -> Vec<Vec<(TokenKind, &str)>> {
        lex_text(Language::Json, text)
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
    fn keys_values_and_comments() {
        let lines = kinds(
            "{ // hi\n  \"a\\n\": [1, -2.5e3, true, null], /* multi\nline */ \"b\": \"s\", bad }\n",
        );
        assert_eq!(
            lines[0],
            vec![(TokenKind::Punctuation, "{"), (TokenKind::Comment, "// hi")]
        );
        assert_eq!(lines[1][0], (TokenKind::Key, "\"a"));
        assert_eq!(lines[1][1], (TokenKind::StringEscape, "\\n"));
        assert_eq!(lines[1][2], (TokenKind::Key, "\""));
        assert_eq!(lines[1][3], (TokenKind::Punctuation, ":"));
        assert_eq!(lines[1][5], (TokenKind::Number, "1"));
        assert_eq!(lines[1][7], (TokenKind::Number, "-2.5e3"));
        assert_eq!(lines[1][9], (TokenKind::Constant, "true"));
        assert_eq!(lines[1][11], (TokenKind::Constant, "null"));
        assert_eq!(lines[1].last().unwrap(), &(TokenKind::Comment, "/* multi"));
        assert_eq!(lines[2][0], (TokenKind::Comment, "line */"));
        assert_eq!(lines[2][1], (TokenKind::Key, "\"b\""));
        assert_eq!(lines[2][3], (TokenKind::String, "\"s\""));
        assert_eq!(lines[2][5], (TokenKind::Invalid, "bad"));
    }

    #[test]
    fn unterminated_string_and_comment_at_line_end() {
        let lines = kinds("\"open\n/*\nx */ 1\n");
        assert_eq!(lines[0], vec![(TokenKind::String, "\"open")]);
        assert_eq!(lines[1], vec![(TokenKind::Comment, "/*")]);
        assert_eq!(lines[2][0], (TokenKind::Comment, "x */"));
        assert_eq!(lines[2][1], (TokenKind::Number, "1"));
    }
}
