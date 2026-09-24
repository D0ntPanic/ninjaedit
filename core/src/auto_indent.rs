//! Indentation for lines as code is typed: where a new line should start,
//! and where a line should move to when its first character closes or
//! opens a scope.
//!
//! The heuristics read the code around the cursor rather than parse it,
//! and trust that the lines before the cursor are already indented the
//! way their author wanted. The indentation of a new line comes from the
//! *statement* the cursor is in, not simply the line: scanning backwards
//! from the cursor, any closing bracket sends the scan up to the line of
//! its opening bracket, so a new line after
//!
//! ```text
//! if (a &&
//!     b) {
//! ```
//!
//! is indented one level past the `if`, not past the continuation line.
//! Brackets inside strings and comments are ignored, going by the syntax
//! highlighter's tokens.
//!
//! The rules, for a line break at the cursor:
//!
//! * After an opening bracket that ends its line (ignoring comments), one
//!   level deeper than the statement the bracket belongs to. A bracket that
//!   starts its line, as with a brace on its own line, belongs to that
//!   line.
//! * Inside a bracket that has text after it on its line, as for the
//!   arguments of a call spread over lines, aligned with that text, or,
//!   by the [`CodeStyle`], one level deeper than the statement.
//! * After a line ending in `:`, in languages where that opens a scope
//!   (Python blocks, `case` labels), one level deeper than the statement.
//! * After a line ending in `;` or `}`, level with the start of the
//!   statement, walking up past continuation lines that start with an
//!   operator (`.`, `&&`, ...) or follow a line ending in `\`. In C-like
//!   languages, a statement that is the body of a brace-less `if`, `for`,
//!   ... goes back to the level of the `if`.
//! * After the header of a brace-less `if`, `for`, `while`, `else`, or
//!   `do`, one level deeper, unless the new line starts with `{`.
//! * In Python, after `return`, `pass`, `break`, `continue`, or `raise`,
//!   one level shallower.
//! * After a line ending in `\`, one level deeper than the statement on
//!   its first continuation line, and level with the line after that.
//! * Otherwise, level with the line (a statement may be continuing), or
//!   in Python, level with the statement.
//!
//! A line whose first character is a closing bracket goes level with the
//! statement of its opening bracket; the editor applies this when a
//! closing bracket is typed at the start of a line, and when a line break
//! is typed between a pair of brackets, which puts the closing one on a
//! line of its own. The editor also uses these rules to
//! move a line when `{` is typed at its start (so a brace on its own line
//! lines up with its `if`), and when `:` completes a `case` label or a
//! Python `else:`, `elif`, `except`, or `finally` (which go level with the
//! label or block they continue).

use crate::indent::{self, Indentation};
use crate::syntax::{Language, Token, TokenKind};
use std::collections::HashMap;
use std::rc::Rc;

/// How far scans go back through the buffer, in lines, before giving up
/// and settling for the indentation of the line they started from.
const MAX_SCAN_LINES: usize = 2000;

/// Operators that, starting a line, mark it as the continuation of the
/// statement on the line before in C-like languages.
const CONTINUATION_OPERATORS: [&[u8]; 7] = [b".", b"&&", b"||", b"?", b":", b"+", b"="];

/// Keywords that start a statement whose body may follow without braces.
const CONTROL_KEYWORDS: [&[u8]; 5] = [b"if", b"else", b"for", b"while", b"do"];

/// Python statements after which a block ends.
const PYTHON_BLOCK_ENDERS: [&[u8]; 5] = [b"return", b"pass", b"break", b"continue", b"raise"];

/// Python keywords that continue the compound statement above them.
const PYTHON_CONTINUATIONS: [&[u8]; 4] = [b"else", b"elif", b"except", b"finally"];

/// Python keywords that start a compound statement that can be continued.
const PYTHON_COMPOUNDS: [&[u8]; 7] = [b"if", b"elif", b"else", b"for", b"while", b"try", b"except"];

/// Labels within a C-like `switch`.
const CASE_LABELS: [&[u8]; 2] = [b"case", b"default"];

/// The closing bracket for an opening one.
pub fn closer_of(b: u8) -> Option<u8> {
    match b {
        b'(' => Some(b')'),
        b'[' => Some(b']'),
        b'{' => Some(b'}'),
        _ => None,
    }
}

pub fn is_closer(b: u8) -> bool {
    matches!(b, b')' | b']' | b'}')
}

fn is_blank(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// How a line that continues inside an open bracket is indented, when the
/// bracket has text after it on its own line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContinuationIndent {
    /// Aligned with the text after the bracket:
    ///
    /// ```text
    /// let x = foo(a,
    ///             b);
    /// ```
    Align,
    /// One level deeper than the statement the bracket belongs to:
    ///
    /// ```text
    /// let x = foo(a,
    ///     b);
    /// ```
    #[default]
    Indent,
}

/// The user's preferences for how code is laid out, where the code itself
/// can't tell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CodeStyle {
    pub continuation: ContinuationIndent,
}

/// Which of the rules apply to a language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rules {
    /// A `:` ending a line opens a scope.
    pub colon_scopes: bool,
    /// Statements end in `;` and control statements may omit braces.
    pub c_like: bool,
    pub python: bool,
}

impl Rules {
    pub fn for_language(language: Language) -> Rules {
        use Language::*;
        let c_like = matches!(language, Rust | C | Cpp | JavaScript | TypeScript | Wgsl);
        let python = language == Python;
        Rules {
            colon_scopes: python || (c_like && language != Rust),
            c_like,
            python,
        }
    }
}

/// A line's content, with which of its bytes are code rather than part
/// of a comment or a string, character, or regular expression literal.
pub(crate) struct CodeLine {
    pub text: Vec<u8>,
    pub code: Vec<bool>,
}

impl CodeLine {
    pub fn new(text: Vec<u8>, tokens: &[Token]) -> CodeLine {
        let mut code = vec![true; text.len()];
        for token in tokens {
            if !is_code(token.kind) {
                let range = token.range.start.min(text.len())..token.range.end.min(text.len());
                code[range].fill(false);
            }
        }
        CodeLine { text, code }
    }

    /// The leading spaces and tabs.
    pub fn leading(&self) -> &[u8] {
        let len = self.text.iter().take_while(|&&b| is_blank(b)).count();
        &self.text[..len]
    }

    /// Whether the byte at `i` is code other than whitespace.
    fn is_code_at(&self, i: usize) -> bool {
        self.code[i] && !self.text[i].is_ascii_whitespace()
    }

    fn first_code(&self) -> Option<usize> {
        (0..self.text.len()).find(|&i| self.is_code_at(i))
    }

    /// The last code byte before `end` that isn't whitespace.
    fn last_code(&self, end: usize) -> Option<usize> {
        (0..end.min(self.text.len()))
            .rev()
            .find(|&i| self.is_code_at(i))
    }

    fn has_code(&self) -> bool {
        self.first_code().is_some()
    }

    /// The word the line's code starts with, after any closing braces (so
    /// that `} else` starts with `else`). Empty when it starts with
    /// something else.
    fn first_word(&self) -> &[u8] {
        let Some(start) = self.first_code() else {
            return &[];
        };
        let start = start
            + self.text[start..]
                .iter()
                .take_while(|&&b| b == b'}' || is_blank(b))
                .count();
        let len = self.text[start..]
            .iter()
            .take_while(|&&b| b.is_ascii_alphanumeric() || b == b'_')
            .count();
        &self.text[start..start + len]
    }
}

/// Whether a token is code, as opposed to a comment or a literal.
pub fn is_code(kind: TokenKind) -> bool {
    !matches!(
        kind,
        TokenKind::Comment
            | TokenKind::DocComment
            | TokenKind::String
            | TokenKind::StringEscape
            | TokenKind::Char
            | TokenKind::Regex
    )
}

/// How far [`Indenter::scan_back`] goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Until {
    /// To the start of a line with every closing bracket matched, passing
    /// over unmatched openers: the start of a statement, as far as
    /// brackets go.
    Balanced,
    /// As `Balanced`, but stopping at an unmatched opener on the way.
    BalancedOrOpener,
    /// To an unmatched opener, however many balanced lines come first.
    Opener,
}

/// The result of scanning backwards from a position.
enum Scan {
    /// An opening bracket, at this line and byte, with nothing between it
    /// and the start of the scan to close it.
    Opener(usize, usize),
    /// The scan reached the start of this line with every closing bracket
    /// it passed matched.
    Balanced(usize),
}

/// Computes indentation over a buffer's lines, which it reads through
/// `lines` (given a line number, its [`CodeLine`]) and caches, since scans
/// revisit them.
pub(crate) struct Indenter<F> {
    lines: F,
    cache: HashMap<usize, Rc<CodeLine>>,
    rules: Rules,
    style: CodeStyle,
    indentation: Indentation,
    tab_width: usize,
}

impl<F: FnMut(usize) -> CodeLine> Indenter<F> {
    pub fn new(
        rules: Rules,
        style: CodeStyle,
        indentation: Indentation,
        tab_width: usize,
        lines: F,
    ) -> Self {
        Indenter {
            lines,
            cache: HashMap::new(),
            rules,
            style,
            indentation,
            tab_width,
        }
    }

    pub fn line(&mut self, line: usize) -> Rc<CodeLine> {
        if let Some(cached) = self.cache.get(&line) {
            return cached.clone();
        }
        let code_line = Rc::new((self.lines)(line));
        self.cache.insert(line, code_line.clone());
        code_line
    }

    fn leading(&mut self, line: usize) -> Vec<u8> {
        self.line(line).leading().to_vec()
    }

    /// One level deeper than `indent`.
    fn deeper(&self, mut indent: Vec<u8>) -> Vec<u8> {
        indent.extend_from_slice(self.indentation.unit().as_bytes());
        indent
    }

    /// One level shallower than `indent`.
    fn shallower(&self, mut indent: Vec<u8>) -> Vec<u8> {
        let len = indent.len() - self.indentation.outdent_len(&indent, self.tab_width);
        indent.truncate(len);
        indent
    }

    fn columns(&self, text: &[u8]) -> usize {
        indent::columns(text, self.tab_width)
    }

    /// Scan backwards from byte `end` of `line`, matching closing brackets
    /// with their opening ones, as far as `until` says. A scan that goes
    /// too far is abandoned as balanced at the line it started from.
    fn scan_back(&mut self, line: usize, end: usize, until: Until) -> Scan {
        let mut depth = 0usize;
        let mut current = line;
        let mut end = end;
        for _ in 0..MAX_SCAN_LINES {
            let l = self.line(current);
            for i in (0..end.min(l.text.len())).rev() {
                if !l.code[i] {
                    continue;
                }
                let b = l.text[i];
                if is_closer(b) {
                    depth += 1;
                } else if closer_of(b).is_some() {
                    if depth > 0 {
                        depth -= 1;
                    } else if until != Until::Balanced {
                        return Scan::Opener(current, i);
                    }
                }
            }
            if (depth == 0 && until != Until::Opener) || current == 0 {
                return Scan::Balanced(current);
            }
            current -= 1;
            end = usize::MAX;
        }
        Scan::Balanced(line)
    }

    fn balanced_start(&mut self, line: usize, end: usize) -> usize {
        match self.scan_back(line, end, Until::Balanced) {
            Scan::Balanced(start) => start,
            Scan::Opener(..) => unreachable!("openers are passed over"),
        }
    }

    /// The nearest line before `line` with any code on it.
    fn previous_code_line(&mut self, line: usize) -> Option<usize> {
        (line.saturating_sub(MAX_SCAN_LINES)..line)
            .rev()
            .find(|&n| self.line(n).has_code())
    }

    /// The line a statement continued on `line` started on, when `line`
    /// is a continuation: it follows a line ending in `\`, or (in C-like
    /// languages) starts with an operator.
    fn continued_from(&mut self, line: usize) -> Option<usize> {
        if line > 0 && self.line(line - 1).text.trim_ascii_end().ends_with(b"\\") {
            return Some(line - 1);
        }
        if !self.rules.c_like {
            return None;
        }
        let l = self.line(line);
        let start = l.first_code()?;
        let rest = &l.text[start..];
        let continues = CONTINUATION_OPERATORS.iter().any(|op| rest.starts_with(op))
            && !rest.starts_with(b"::");
        if continues {
            self.previous_code_line(line)
        } else {
            None
        }
    }

    /// The first line of the statement that `line`, balanced as far as
    /// its brackets go, belongs to.
    fn walk_continuations(&mut self, mut line: usize) -> usize {
        for _ in 0..MAX_SCAN_LINES {
            let Some(previous) = self.continued_from(line) else {
                break;
            };
            let len = self.line(previous).text.len();
            line = self.balanced_start(previous, len);
        }
        line
    }

    /// The first line of the statement containing byte `end` of `line`.
    fn statement_start(&mut self, line: usize, end: usize) -> usize {
        let start = self.balanced_start(line, end);
        self.walk_continuations(start)
    }

    /// The indentation of the statement an opening bracket belongs to:
    /// its own line when the bracket starts the line, else the statement
    /// containing it.
    fn opener_base(&mut self, line: usize, col: usize) -> Vec<u8> {
        if self.line(line).first_code() == Some(col) {
            return self.leading(line);
        }
        let start = self.statement_start(line, col);
        self.leading(start)
    }

    fn is_control_header(&mut self, line: usize) -> bool {
        let l = self.line(line);
        CONTROL_KEYWORDS.contains(&l.first_word())
    }

    /// When `line` is the last line of the header of a brace-less control
    /// statement, the indentation of that statement.
    fn header_indent(&mut self, line: usize) -> Option<Vec<u8>> {
        let l = self.line(line);
        let last = l.last_code(l.text.len())?;
        if matches!(l.text[last], b';' | b'{' | b'}' | b',' | b':') {
            return None;
        }
        let Scan::Balanced(start) = self.scan_back(line, l.text.len(), Until::BalancedOrOpener)
        else {
            return None;
        };
        let start = self.walk_continuations(start);
        self.is_control_header(start).then(|| self.leading(start))
    }

    /// The indentation after a complete statement starting on `start`.
    fn after_statement(&mut self, start: usize) -> Vec<u8> {
        let indent = self.leading(start);
        if !self.rules.c_like {
            return indent;
        }
        // The body of a brace-less `if` goes back to the level of the `if`.
        if let Some(previous) = self.previous_code_line(start)
            && let Some(header) = self.header_indent(previous)
            && self.columns(&header) < self.columns(&indent)
        {
            return header;
        }
        indent
    }

    /// The indentation for a new line made by breaking `line` at byte
    /// `col`, whose content will start with `next`. See the [module
    /// documentation](self) for the rules.
    pub fn newline_indent(&mut self, line: usize, col: usize, next: Option<u8>) -> Vec<u8> {
        let current = self.line(line);
        match self.scan_back(line, col, Until::BalancedOrOpener) {
            Scan::Opener(opener_line, opener_col) => {
                let l = self.line(opener_line);
                let limit = if opener_line == line {
                    col
                } else {
                    l.text.len()
                };
                let after = opener_col + 1..limit;
                let block = !after.clone().any(|i| l.is_code_at(i));
                if block || self.style.continuation == ContinuationIndent::Indent {
                    let base = self.opener_base(opener_line, opener_col);
                    return self.deeper(base);
                }
                // Align with the text after the bracket.
                let target = after
                    .clone()
                    .find(|&i| !is_blank(l.text[i]))
                    .unwrap_or(opener_col + 1);
                let mut indent = l.leading().to_vec();
                let pad = self.columns(&l.text[..target]) - self.columns(&indent);
                indent.resize(indent.len() + pad, b' ');
                indent
            }
            Scan::Balanced(start) => {
                let Some(last) = current.last_code(col) else {
                    // Nothing but comments: keep their indentation.
                    let len = current.leading().len().min(col);
                    return current.text[..len].to_vec();
                };
                let c = current.text[last];
                if c == b',' {
                    return self.leading(start);
                }
                if current.text[..col].trim_ascii_end().ends_with(b"\\") {
                    // A line continued with `\`: indent the first
                    // continuation line, keep the level of later ones.
                    let base = self.leading(start);
                    return if self.continued_from(start).is_some() {
                        base
                    } else {
                        self.deeper(base)
                    };
                }
                let statement = self.walk_continuations(start);
                let is_scope_colon = c == b':'
                    && self.rules.colon_scopes
                    && !(last > 0 && current.text[last - 1] == b':');
                if is_scope_colon {
                    let base = self.leading(statement);
                    return self.deeper(base);
                }
                if self.rules.python
                    && PYTHON_BLOCK_ENDERS.contains(&self.line(statement).first_word())
                {
                    let base = self.leading(statement);
                    return self.shallower(base);
                }
                if matches!(c, b';' | b'}') {
                    return self.after_statement(statement);
                }
                if self.rules.c_like && self.is_control_header(statement) {
                    let base = self.leading(statement);
                    return if next == Some(b'{') {
                        base
                    } else {
                        self.deeper(base)
                    };
                }
                if self.rules.c_like && next == Some(b'{') {
                    return self.leading(statement);
                }
                if self.rules.python {
                    // Outside brackets, a Python statement ends with its
                    // line.
                    return self.leading(statement);
                }
                self.leading(start)
            }
        }
    }

    /// The indentation for a line starting with a closing bracket, placed
    /// after byte `col` of `line`: level with the statement of the opening
    /// bracket it closes. `None` when there is no such bracket.
    pub fn closer_indent(&mut self, line: usize, col: usize) -> Option<Vec<u8>> {
        match self.scan_back(line, col, Until::Opener) {
            Scan::Opener(opener_line, opener_col) => {
                Some(self.opener_base(opener_line, opener_col))
            }
            Scan::Balanced(_) => None,
        }
    }

    /// The indentation for `line` when a `{` is typed at its start: where
    /// a new line after the code above it would go if it started with
    /// `{`. `None` when there is no code above.
    pub fn brace_line_indent(&mut self, line: usize) -> Option<Vec<u8>> {
        let previous = self.previous_code_line(line)?;
        let len = self.line(previous).text.len();
        Some(self.newline_indent(previous, len, Some(b'{')))
    }

    /// The indentation for `line` when a `:` typed at its end completes a
    /// label that continues a scope above it: a `case` level with the
    /// previous `case` of the same `switch`, or a Python `else` level with
    /// its `if`. `None` when the line isn't such a label, or there is
    /// nothing above to line it up with.
    pub fn label_indent(&mut self, line: usize) -> Option<Vec<u8>> {
        let l = self.line(line);
        let word = l.first_word();
        if self.rules.python && PYTHON_CONTINUATIONS.contains(&word) {
            return self.python_label_indent(line);
        }
        if self.rules.c_like && self.rules.colon_scopes && CASE_LABELS.contains(&word) {
            return self.case_label_indent(line);
        }
        None
    }

    /// The indentation of the compound statement a Python `else:` (or
    /// similar) on `line` continues: the nearest line above at the same
    /// or a shallower level that starts one, passing over deeper lines
    /// (the body) and same-level ones that don't.
    fn python_label_indent(&mut self, line: usize) -> Option<Vec<u8>> {
        let l = self.line(line);
        let columns = self.columns(l.leading());
        let mut current = line;
        for _ in 0..MAX_SCAN_LINES {
            current = self.previous_code_line(current)?;
            let l = self.line(current);
            let level = self.columns(l.leading());
            if level > columns {
                continue;
            }
            if PYTHON_COMPOUNDS.contains(&l.first_word()) {
                return Some(l.leading().to_vec());
            }
            if level < columns {
                return None;
            }
        }
        None
    }

    /// The indentation of the previous `case` or `default` label in the
    /// same block as `line`, passing over nested blocks.
    fn case_label_indent(&mut self, line: usize) -> Option<Vec<u8>> {
        let mut depth = 0usize;
        for current in (line.saturating_sub(MAX_SCAN_LINES)..line).rev() {
            let l = self.line(current);
            for i in (0..l.text.len()).rev() {
                if !l.code[i] {
                    continue;
                }
                let b = l.text[i];
                if is_closer(b) {
                    depth += 1;
                } else if closer_of(b).is_some() {
                    // An unmatched opener is the start of the `switch`.
                    depth = depth.checked_sub(1)?;
                }
            }
            if depth == 0 && CASE_LABELS.contains(&l.first_word()) {
                return Some(l.leading().to_vec());
            }
        }
        None
    }
}
