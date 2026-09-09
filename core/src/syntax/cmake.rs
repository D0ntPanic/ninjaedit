//! CMake: `CMakeLists.txt` and `*.cmake` files.
//!
//! A CMake file is a sequence of commands, `name(arguments...)`, which
//! may span lines. Command names are matched case-insensitively: control
//! flow (`if`, `foreach`, ...) is a control keyword, `function` and
//! `macro` define the function named by their first argument, `set` and
//! friends define the variable named by theirs, and everything else is a
//! function. Arguments are quoted strings (which may span lines), bracket
//! arguments `[[...]]` (likewise), or unquoted words: all-capital words
//! such as `PRIVATE` or `REQUIRED` are constants, and the comparison
//! and logic words of `if` are keywords. `${VAR}`, `$ENV{VAR}`, and
//! `$CACHE{VAR}` are variables, inside strings as well as outside.
//! Comments are `#` to end of line or `#[[ ... ]]` blocks.

use super::{Context, LexState, Lexer, Token, TokenKind};

pub(super) struct CMake;

const CONTROL: &[&str] = &[
    "if",
    "elseif",
    "else",
    "endif",
    "foreach",
    "endforeach",
    "while",
    "endwhile",
    "break",
    "continue",
    "return",
    "block",
    "endblock",
];

/// Commands that close a definition.
const KEYWORDS: &[&str] = &["endfunction", "endmacro"];

/// Commands whose first argument names what they define.
const DEFINERS: &[(&str, TokenKind)] = &[
    ("function", TokenKind::FunctionDefinition),
    ("macro", TokenKind::FunctionDefinition),
    ("set", TokenKind::VariableDefinition),
    ("unset", TokenKind::VariableDefinition),
    ("option", TokenKind::VariableDefinition),
    ("set_property", TokenKind::Identifier),
];

/// Words with special meaning in `if` conditions and similar.
const OPERATORS: &[&str] = &[
    "NOT",
    "AND",
    "OR",
    "COMMAND",
    "POLICY",
    "TARGET",
    "TEST",
    "DEFINED",
    "EXISTS",
    "IS_NEWER_THAN",
    "IS_DIRECTORY",
    "IS_SYMLINK",
    "IS_ABSOLUTE",
    "IS_READABLE",
    "IS_WRITABLE",
    "IS_EXECUTABLE",
    "MATCHES",
    "LESS",
    "GREATER",
    "EQUAL",
    "LESS_EQUAL",
    "GREATER_EQUAL",
    "STRLESS",
    "STRGREATER",
    "STREQUAL",
    "STRLESS_EQUAL",
    "STRGREATER_EQUAL",
    "VERSION_LESS",
    "VERSION_GREATER",
    "VERSION_EQUAL",
    "VERSION_LESS_EQUAL",
    "VERSION_GREATER_EQUAL",
    "IN_LIST",
    "PATH_EQUAL",
];

impl Lexer for CMake {
    fn lex_line(&self, mut state: LexState, line: &[u8], out: &mut Vec<Token>) -> LexState {
        let mut lx = Line {
            line,
            pos: 0,
            out,
            pending: None,
        };
        loop {
            let more = match state.top() {
                Some(Context::Bracket { equals, comment }) => {
                    lx.continue_bracket(&mut state, equals, comment)
                }
                Some(Context::String { .. }) => lx.continue_string(&mut state),
                _ => lx.next_token(&mut state),
            };
            if !more {
                return state;
            }
        }
    }
}

struct Line<'a> {
    line: &'a [u8],
    pos: usize,
    out: &'a mut Vec<Token>,
    /// The kind of the next unquoted argument, set by a defining command.
    pending: Option<TokenKind>,
}

/// The nesting depth of parentheses, which is how many `Value` contexts
/// are on the stack. Command names appear only at depth zero.
fn paren_depth(state: &LexState) -> usize {
    let mut depth = 0;
    let mut probe = *state;
    while let Some(context) = probe.pop() {
        if matches!(context, Context::Value { close: b')' }) {
            depth += 1;
        }
    }
    depth
}

/// Whether `bytes` starts a bracket, `[`, `=`s, `[`; returns the number
/// of equals signs.
fn bracket_open(bytes: &[u8]) -> Option<usize> {
    if bytes.first() != Some(&b'[') {
        return None;
    }
    let equals = bytes[1..].iter().take_while(|&&b| b == b'=').count();
    (bytes.get(1 + equals) == Some(&b'[')).then_some(equals)
}

impl Line<'_> {
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

    /// The rest of a bracket argument or comment. Returns whether the
    /// line goes on after it.
    fn continue_bracket(&mut self, state: &mut LexState, equals: u8, comment: bool) -> bool {
        let kind = if comment {
            TokenKind::Comment
        } else {
            TokenKind::String
        };
        let start = self.pos;
        let mut i = self.pos;
        while i < self.line.len() {
            if self.line[i] == b']'
                && (1..=equals as usize).all(|k| self.at(i + k) == b'=')
                && self.at(i + 1 + equals as usize) == b']'
            {
                let end = i + 2 + equals as usize;
                self.emit(start, end, kind);
                self.pos = end;
                state.pop();
                return true;
            }
            i += 1;
        }
        self.emit(start, self.line.len(), kind);
        self.pos = self.line.len();
        false
    }

    /// The rest of a quoted argument, with escapes and variable
    /// references picked out. Returns whether the line goes on after it.
    fn continue_string(&mut self, state: &mut LexState) -> bool {
        let mut segment = self.pos;
        let mut i = self.pos;
        while i < self.line.len() {
            match self.line[i] {
                b'\\' => {
                    let end = (i + 2).min(self.line.len());
                    self.emit(segment, i, TokenKind::String);
                    self.emit(i, end, TokenKind::StringEscape);
                    segment = end;
                    i = end;
                }
                b'$' => match self.variable_end(i) {
                    Some(end) => {
                        self.emit(segment, i, TokenKind::String);
                        self.emit(i, end, TokenKind::Variable);
                        segment = end;
                        i = end;
                    }
                    None => i += 1,
                },
                b'"' => {
                    self.emit(segment, i + 1, TokenKind::String);
                    self.pos = i + 1;
                    state.pop();
                    self.pending = None;
                    return true;
                }
                _ => i += 1,
            }
        }
        self.emit(segment, self.line.len(), TokenKind::String);
        self.pos = self.line.len();
        false
    }

    /// The end of a variable reference (`${...}`, `$ENV{...}`,
    /// `$CACHE{...}`) starting at the `$` at `i`, allowing nested
    /// references.
    fn variable_end(&self, i: usize) -> Option<usize> {
        let rest = &self.line[i..];
        let open = if rest.starts_with(b"${") {
            1
        } else if rest.starts_with(b"$ENV{") {
            4
        } else if rest.starts_with(b"$CACHE{") {
            6
        } else {
            return None;
        };
        let mut depth = 0;
        let mut j = i + open;
        while j < self.line.len() {
            match self.line[j] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j + 1);
                    }
                }
                b' ' | b'\t' | b'"' | b'(' | b')' => return None,
                _ => {}
            }
            j += 1;
        }
        None
    }

    /// One token outside strings and brackets. Returns whether there is
    /// more of the line.
    fn next_token(&mut self, state: &mut LexState) -> bool {
        while self.pos < self.line.len() && matches!(self.line[self.pos], b' ' | b'\t' | b'\r') {
            self.pos += 1;
        }
        if self.pos >= self.line.len() {
            return false;
        }
        let start = self.pos;
        let rest = &self.line[start..];
        match rest[0] {
            b'#' => {
                if let Some(equals) = bracket_open(&rest[1..]) {
                    self.pos = start + 3 + equals;
                    state.push(Context::Bracket {
                        equals: equals.min(255) as u8,
                        comment: true,
                    });
                    let more = self.continue_bracket(state, equals.min(255) as u8, true);
                    self.stretch_last(start);
                    return more;
                }
                self.emit(start, self.line.len(), TokenKind::Comment);
                self.pos = self.line.len();
                false
            }
            b'"' => {
                self.pos = start + 1;
                state.push(Context::String {
                    quote: b'"',
                    triple: false,
                    escapes: true,
                    hashes: 0,
                });
                let more = self.continue_string(state);
                // Join the opening quote onto the first body token, or put
                // it before the body if that starts with an escape or a
                // variable.
                let mut first = self.out.len();
                while first > 0 && self.out[first - 1].range.start > start {
                    first -= 1;
                }
                match self.out.get_mut(first) {
                    Some(body) if body.kind == TokenKind::String => body.range.start = start,
                    _ => self.out.insert(
                        first,
                        Token {
                            range: start..start + 1,
                            kind: TokenKind::String,
                        },
                    ),
                }
                self.pending = None;
                more
            }
            b'[' if bracket_open(rest).is_some() => {
                let equals = bracket_open(rest).unwrap().min(255) as u8;
                self.pos = start + 2 + equals as usize;
                state.push(Context::Bracket {
                    equals,
                    comment: false,
                });
                let more = self.continue_bracket(state, equals, false);
                self.stretch_last(start);
                self.pending = None;
                more
            }
            b'(' => {
                self.emit(start, start + 1, TokenKind::Punctuation);
                self.pos = start + 1;
                state.push(Context::Value { close: b')' });
                true
            }
            b')' => {
                self.emit(start, start + 1, TokenKind::Punctuation);
                self.pos = start + 1;
                if let Some(Context::Value { close: b')' }) = state.top() {
                    state.pop();
                }
                self.pending = None;
                true
            }
            b'$' => {
                if let Some(end) = self.variable_end(start) {
                    self.emit(start, end, TokenKind::Variable);
                    self.pos = end;
                    self.pending = None;
                    return true;
                }
                if rest.starts_with(b"$<") {
                    self.emit(start, start + 2, TokenKind::Punctuation);
                    self.pos = start + 2;
                    return true;
                }
                self.unquoted_argument(state, start)
            }
            b'>' => {
                self.emit(start, start + 1, TokenKind::Punctuation);
                self.pos = start + 1;
                true
            }
            _ => self.unquoted_argument(state, start),
        }
    }

    /// Extend the last token back to `start`, so a bracket's opening
    /// delimiter is part of it.
    fn stretch_last(&mut self, start: usize) {
        match self.out.last_mut() {
            Some(last) if last.range.start > start => last.range.start = start,
            _ => {}
        }
    }

    fn unquoted_argument(&mut self, state: &mut LexState, start: usize) -> bool {
        let mut end = start;
        while end < self.line.len() {
            let b = self.line[end];
            if matches!(b, b' ' | b'\t' | b'\r' | b'(' | b')' | b'"' | b'#' | b'>') {
                break;
            }
            if b == b'$' && (self.variable_end(end).is_some() || self.at(end + 1) == b'<') {
                break;
            }
            if b == b'\\' {
                end += 2;
                continue;
            }
            end += 1;
        }
        let end = end.min(self.line.len());
        if end == start {
            self.emit(start, start + 1, TokenKind::Invalid);
            self.pos = start + 1;
            return true;
        }
        let word = &self.line[start..end];
        let mut next = end;
        while next < self.line.len() && matches!(self.line[next], b' ' | b'\t') {
            next += 1;
        }
        let command = self.at(next) == b'(' && paren_depth(state) == 0;
        let kind = if command {
            let lower: Vec<u8> = word.to_ascii_lowercase();
            if CONTROL.iter().any(|c| c.as_bytes() == lower) {
                TokenKind::ControlKeyword
            } else if KEYWORDS.iter().any(|c| c.as_bytes() == lower) {
                TokenKind::Keyword
            } else if let Some((_, defines)) = DEFINERS.iter().find(|(c, _)| c.as_bytes() == lower)
            {
                self.pending = Some(*defines);
                if *defines == TokenKind::FunctionDefinition {
                    TokenKind::Keyword
                } else {
                    TokenKind::Function
                }
            } else {
                TokenKind::Function
            }
        } else if let Some(pending) = self.pending.take() {
            pending
        } else if OPERATORS.iter().any(|o| o.as_bytes() == word) {
            TokenKind::Keyword
        } else if word.iter().all(|b| b.is_ascii_digit() || *b == b'.') && word[0].is_ascii_digit()
        {
            TokenKind::Number
        } else if word.len() > 1
            && word.iter().any(|b| b.is_ascii_uppercase())
            && word
                .iter()
                .all(|&b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        {
            TokenKind::Constant
        } else {
            TokenKind::Identifier
        };
        if !command && kind != TokenKind::Keyword {
            // Only the first argument after a defining command is special.
            self.pending = None;
        }
        self.emit(start, end, kind);
        self.pos = end;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::check;
    use super::super::{Language, lex_text};
    use super::*;

    #[test]
    fn commands_and_arguments() {
        check(
            Language::CMake,
            "\
cmake_minimum_required(VERSION 3.16) # comment
ffffffffffffffffffffffpNNNNNNN nnnnp ccccccccc
project(Demo LANGUAGES CXX)
fffffffpiiii NNNNNNNNN NNNp
set(SOURCES src/main.cpp \"${CMAKE_SOURCE_DIR}/x.cpp\" \"a\\;b\")
fffpvvvvvvv iiiiiiiiiiii s$$$$$$$$$$$$$$$$$$$sssssss sseessp
IF(NOT DEFINED FOO AND ${FOO} STREQUAL \"x\")
KKpkkk kkkkkkk NNN kkk $$$$$$ kkkkkkkk sssp
  target_link_libraries(demo PRIVATE $<$<CONFIG:Debug>:dbg> $ENV{HOME}/lib)
  fffffffffffffffffffffpiiii NNNNNNN ppppiiiiiiiiiiiipiiiip $$$$$$$$$$iiiip
endif()
KKKKKpp
function(my_func arg)
kkkkkkkkpFFFFFFF iiip
  message(STATUS \"got ${arg}\")
  fffffffpNNNNNN sssss$$$$$$sp
endfunction()
kkkkkkkkkkkpp
#[[ block
ccccccccc
comment ]] set(x [=[ raw ]] ]=])
cccccccccc fffpv ssssssssssssssp
",
        );
    }

    #[test]
    fn multi_line_commands_strings_and_brackets() {
        let lines = lex_text(
            Language::CMake,
            "set(LIST\n  a\n  \"two\n  lines\"\n  [[b\nc]]\n)\nfoo(x)\n",
        );
        // Arguments on continuation lines are not commands.
        assert_eq!(
            lines[1],
            vec![Token {
                range: 2..3,
                kind: TokenKind::Identifier
            }]
        );
        assert_eq!(
            lines[2][0],
            Token {
                range: 2..6,
                kind: TokenKind::String
            }
        );
        assert_eq!(
            lines[3],
            vec![Token {
                range: 0..8,
                kind: TokenKind::String
            }]
        );
        assert_eq!(
            lines[4][0],
            Token {
                range: 2..5,
                kind: TokenKind::String
            }
        );
        assert_eq!(
            lines[5],
            vec![Token {
                range: 0..3,
                kind: TokenKind::String
            }]
        );
        assert_eq!(
            lines[6],
            vec![Token {
                range: 0..1,
                kind: TokenKind::Punctuation
            }]
        );
        assert_eq!(lines[7][0].kind, TokenKind::Function);
    }

    #[test]
    fn no_panics_on_odd_input() {
        let inputs: &[&[u8]] = &[
            b"", b"#", b"#[[", b"#[=[", b"[[", b"[=", b"\"", b"\"\\", b"$", b"${", b"$ENV{", b"$<",
            b"(", b")", b"\\", b">", b"\xff",
        ];
        for input in inputs {
            let mut out = Vec::new();
            CMake.lex_line(LexState::default(), input, &mut out);
        }
    }
}
