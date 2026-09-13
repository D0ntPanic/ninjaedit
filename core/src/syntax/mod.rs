//! Syntax highlighting: frontend-agnostic token classification of source
//! text, kept up to date incrementally as a buffer is edited.
//!
//! Every language is a state machine over lines. A [`Lexer`] is given the
//! [`LexState`] at the start of a line and the line's bytes, and produces
//! the line's [`Token`]s and the state at its end. A state only ever
//! depends on the text *before* it, never after, which is what makes the
//! whole system incremental: an edit can only change the states of the
//! lines that follow it, and re-lexing can stop as soon as a freshly
//! computed state matches the one already known for a line.
//!
//! Tokens are never cached. The [`Highlighter`] remembers only the start
//! state of every line, which is small and fixed-size, and a frontend
//! re-lexes the handful of lines it is about to draw from their cached
//! states. Inside a huge block comment, the state of a visible line simply
//! says "in a block comment", so drawing costs the same wherever the
//! comment began. Re-lexing after an edit happens on a worker thread, with
//! a short synchronous pass at draw time so the lines around an edit are
//! never behind; see the [`highlighter`] module.
//!
//! Classification is done by the lexer alone, with a little lookahead
//! within the line, so it works without a language server and on partial
//! text such as diff hunks. `self.field` and `self.func()` are told apart
//! by whether a call follows, `Foo::bar` marks `Foo` as a type by its
//! capital and `foo::bar` marks `foo` as a namespace, and so on; see the
//! [`clike`] module for the rules. The [`Language`] of a file is chosen by
//! its extension; a file whose kind isn't known is [`Language::Plain`].
//!
//! Merge conflict markers belong to no language, so a file's lexer (see
//! [`Language::file_lexer`]) is its language's lexer wrapped in one that
//! recognizes them in any file; see the [`conflicts`] module.

mod clike;
mod cmake;
mod conflicts;
mod highlighter;
mod json;
mod markdown;
mod plain;
mod toml;

pub use highlighter::Highlighter;

pub(crate) use conflicts::is_conflict_start;

use std::ops::Range;
use std::path::Path;

/// What a piece of text is, for a frontend to map onto a style. The kinds
/// are deliberately more specific than most themes will care about; a
/// theme that doesn't set a kind can fall back to its [`parent`](Self::parent).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TokenKind {
    /// Anything not otherwise classified, including whitespace.
    Text,
    /// An identifier with no more specific classification.
    Identifier,
    Keyword,
    /// A keyword that directs control flow: `if`, `return`, `for`, ...
    ControlKeyword,
    Operator,
    /// Brackets, commas, semicolons, and the like.
    Punctuation,
    Comment,
    /// A documentation comment, such as `///` in Rust or `/** */` in C.
    DocComment,
    Number,
    String,
    /// An escape sequence inside a string or character literal.
    StringEscape,
    /// A character literal.
    Char,
    /// A regular expression literal, in languages that have them.
    Regex,
    /// A type name in use.
    Type,
    /// A type built into the language: `u32`, `int`, `bool`, `vec4`, ...
    PrimitiveType,
    /// A constant: `SCREAMING_CASE` names and language constants such as
    /// `true`, `null`, and `None`.
    Constant,
    /// A function or method being called or referred to.
    Function,
    /// The name of a function in its definition.
    FunctionDefinition,
    /// The name of a type in its definition.
    TypeDefinition,
    /// The name of a variable in its declaration.
    VariableDefinition,
    /// A field or property access, or a field in a struct or object literal.
    Field,
    /// A module, namespace, or crate name.
    Namespace,
    /// A macro invocation.
    Macro,
    /// An attribute or decorator: `#[derive]`, `@property`, `@location(0)`.
    Attribute,
    Lifetime,
    Label,
    /// A preprocessor directive.
    Preprocessor,
    /// A key in a data file such as TOML or JSON.
    Key,
    /// Text the lexer could tell is malformed.
    Invalid,
    /// A heading in prose formats such as Markdown.
    Heading,
    /// Strongly emphasized (bold) prose, including its markers.
    Strong,
    /// Emphasized (italic) prose, including its markers.
    Emphasis,
    /// A link: its text, reference, or URL.
    Link,
    /// A list bullet, number, or task checkbox.
    ListMarker,
    /// A block quote.
    Quote,
    /// A variable reference in languages that mark them, such as `${var}`
    /// in CMake.
    Variable,
    /// A merge conflict marker line: `<<<<<<<`, `|||||||`, `=======`, or
    /// `>>>>>>>` with its label. See the [`conflicts`] module.
    ConflictMarker,
}

impl TokenKind {
    /// Every kind, for building tables indexed by kind.
    pub const ALL: [TokenKind; TokenKind::COUNT] = [
        TokenKind::Text,
        TokenKind::Identifier,
        TokenKind::Keyword,
        TokenKind::ControlKeyword,
        TokenKind::Operator,
        TokenKind::Punctuation,
        TokenKind::Comment,
        TokenKind::DocComment,
        TokenKind::Number,
        TokenKind::String,
        TokenKind::StringEscape,
        TokenKind::Char,
        TokenKind::Regex,
        TokenKind::Type,
        TokenKind::PrimitiveType,
        TokenKind::Constant,
        TokenKind::Function,
        TokenKind::FunctionDefinition,
        TokenKind::TypeDefinition,
        TokenKind::VariableDefinition,
        TokenKind::Field,
        TokenKind::Namespace,
        TokenKind::Macro,
        TokenKind::Attribute,
        TokenKind::Lifetime,
        TokenKind::Label,
        TokenKind::Preprocessor,
        TokenKind::Key,
        TokenKind::Invalid,
        TokenKind::Heading,
        TokenKind::Strong,
        TokenKind::Emphasis,
        TokenKind::Link,
        TokenKind::ListMarker,
        TokenKind::Quote,
        TokenKind::Variable,
        TokenKind::ConflictMarker,
    ];

    /// The number of kinds; `index` is always below it.
    pub const COUNT: usize = 37;

    /// A stable index for tables, `0..COUNT`.
    pub fn index(self) -> usize {
        self as usize
    }

    /// A short, stable, kebab-case name, as used in theme files.
    pub fn name(self) -> &'static str {
        match self {
            TokenKind::Text => "text",
            TokenKind::Identifier => "identifier",
            TokenKind::Keyword => "keyword",
            TokenKind::ControlKeyword => "control-keyword",
            TokenKind::Operator => "operator",
            TokenKind::Punctuation => "punctuation",
            TokenKind::Comment => "comment",
            TokenKind::DocComment => "doc-comment",
            TokenKind::Number => "number",
            TokenKind::String => "string",
            TokenKind::StringEscape => "string-escape",
            TokenKind::Char => "char",
            TokenKind::Regex => "regex",
            TokenKind::Type => "type",
            TokenKind::PrimitiveType => "primitive-type",
            TokenKind::Constant => "constant",
            TokenKind::Function => "function",
            TokenKind::FunctionDefinition => "function-definition",
            TokenKind::TypeDefinition => "type-definition",
            TokenKind::VariableDefinition => "variable-definition",
            TokenKind::Field => "field",
            TokenKind::Namespace => "namespace",
            TokenKind::Macro => "macro",
            TokenKind::Attribute => "attribute",
            TokenKind::Lifetime => "lifetime",
            TokenKind::Label => "label",
            TokenKind::Preprocessor => "preprocessor",
            TokenKind::Key => "key",
            TokenKind::Invalid => "invalid",
            TokenKind::Heading => "heading",
            TokenKind::Strong => "strong",
            TokenKind::Emphasis => "emphasis",
            TokenKind::Link => "link",
            TokenKind::ListMarker => "list-marker",
            TokenKind::Quote => "quote",
            TokenKind::Variable => "variable",
            TokenKind::ConflictMarker => "conflict-marker",
        }
    }

    /// The kind whose style this one should borrow when a theme doesn't
    /// style it directly. `None` for the root kinds, which fall back to
    /// plain text.
    pub fn parent(self) -> Option<TokenKind> {
        Some(match self {
            TokenKind::Text | TokenKind::Identifier => return None,
            TokenKind::ControlKeyword | TokenKind::Label => TokenKind::Keyword,
            TokenKind::DocComment => TokenKind::Comment,
            TokenKind::StringEscape | TokenKind::Char | TokenKind::Regex => TokenKind::String,
            TokenKind::PrimitiveType | TokenKind::TypeDefinition => TokenKind::Type,
            TokenKind::FunctionDefinition | TokenKind::Macro => TokenKind::Function,
            TokenKind::VariableDefinition | TokenKind::Variable => TokenKind::Identifier,
            TokenKind::Constant => TokenKind::Number,
            TokenKind::Key => TokenKind::Field,
            TokenKind::Namespace | TokenKind::Field => TokenKind::Identifier,
            TokenKind::Attribute | TokenKind::Preprocessor => TokenKind::Keyword,
            TokenKind::Lifetime => TokenKind::Type,
            TokenKind::Heading | TokenKind::ListMarker => TokenKind::Keyword,
            TokenKind::Link => TokenKind::String,
            TokenKind::Quote => TokenKind::Comment,
            TokenKind::ConflictMarker => TokenKind::Invalid,
            TokenKind::Strong | TokenKind::Emphasis => return None,
            TokenKind::Keyword
            | TokenKind::Operator
            | TokenKind::Punctuation
            | TokenKind::Comment
            | TokenKind::Number
            | TokenKind::String
            | TokenKind::Type
            | TokenKind::Function
            | TokenKind::Invalid => return None,
        })
    }
}

/// A classified span of a line. Ranges are byte offsets relative to the
/// start of the line's content; the gaps between tokens are plain text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub range: Range<usize>,
    pub kind: TokenKind,
}

/// A language with a lexer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    /// Text of no known kind, which gets no highlighting of its own.
    Plain,
    Rust,
    C,
    Cpp,
    JavaScript,
    TypeScript,
    Wgsl,
    Toml,
    Json,
    Python,
    Markdown,
    CMake,
}

impl Language {
    /// Every language, in a stable order; see [`index`](Self::index).
    pub const ALL: [Language; 12] = [
        Language::Plain,
        Language::Rust,
        Language::C,
        Language::Cpp,
        Language::JavaScript,
        Language::TypeScript,
        Language::Wgsl,
        Language::Toml,
        Language::Json,
        Language::Python,
        Language::Markdown,
        Language::CMake,
    ];

    /// A stable small integer for the language, the inverse of
    /// [`from_index`](Self::from_index). Used to name the language of a
    /// Markdown code fence inside a [`LexState`].
    pub fn index(self) -> u8 {
        self as u8
    }

    pub fn from_index(index: u8) -> Option<Language> {
        Language::ALL.get(index as usize).copied()
    }

    /// The language a Markdown code fence's info string names, such as
    /// `rust` or `js`.
    pub fn from_fence_info(info: &str) -> Option<Language> {
        let name = info
            .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
            .next()?;
        Some(match name.to_ascii_lowercase().as_str() {
            "rust" | "rs" => Language::Rust,
            "c" | "h" => Language::C,
            "cpp" | "c++" | "cxx" | "cc" | "hpp" => Language::Cpp,
            "js" | "javascript" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
            "ts" | "typescript" | "tsx" => Language::TypeScript,
            "wgsl" => Language::Wgsl,
            "toml" => Language::Toml,
            "json" | "jsonc" | "json5" => Language::Json,
            "py" | "python" | "python3" => Language::Python,
            "md" | "markdown" => Language::Markdown,
            "cmake" => Language::CMake,
            _ => return None,
        })
    }

    /// Guess a file's language from its name, or `None` when nothing is
    /// known about it (the file is then [`Plain`](Self::Plain)). Headers
    /// are treated as C++, which highlights C correctly as well.
    pub fn from_path(path: &Path) -> Option<Language> {
        let name = path.file_name()?.to_str()?;
        match name {
            "Cargo.lock" => return Some(Language::Toml),
            "CMakeLists.txt" => return Some(Language::CMake),
            "README" | "CHANGELOG" | "CONTRIBUTING" | "LICENSE.md" => {
                return Some(Language::Markdown);
            }
            ".babelrc" | ".eslintrc" | ".prettierrc" | "composer.lock" => {
                return Some(Language::Json);
            }
            _ => {}
        }
        let extension = path.extension()?.to_str()?;
        Some(match extension.to_ascii_lowercase().as_str() {
            "rs" => Language::Rust,
            "c" => Language::C,
            "h" | "hh" | "hpp" | "hxx" | "h++" | "cc" | "cpp" | "cxx" | "c++" | "inl" | "ipp" => {
                Language::Cpp
            }
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            "ts" | "mts" | "cts" | "tsx" => Language::TypeScript,
            "wgsl" => Language::Wgsl,
            "toml" => Language::Toml,
            "json" | "jsonc" | "json5" | "webmanifest" => Language::Json,
            "py" | "pyi" | "pyw" => Language::Python,
            "md" | "markdown" | "mdown" | "mkd" | "mkdn" => Language::Markdown,
            "cmake" => Language::CMake,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Language::Plain => "Plain Text",
            Language::Rust => "Rust",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::JavaScript => "JavaScript",
            Language::TypeScript => "TypeScript",
            Language::Wgsl => "WGSL",
            Language::Toml => "TOML",
            Language::Json => "JSON",
            Language::Python => "Python",
            Language::Markdown => "Markdown",
            Language::CMake => "CMake",
        }
    }

    /// The lexer for this language alone. Highlighting a file should use
    /// [`file_lexer`](Self::file_lexer) instead; this is for lexers that
    /// embed one language in another, such as Markdown's code fences.
    pub fn lexer(self) -> &'static dyn Lexer {
        match self {
            Language::Plain => &plain::Plain,
            Language::Rust => &clike::RUST,
            Language::C => &clike::C,
            Language::Cpp => &clike::CPP,
            Language::JavaScript => &clike::JAVASCRIPT,
            Language::TypeScript => &clike::TYPESCRIPT,
            Language::Wgsl => &clike::WGSL,
            Language::Python => &clike::PYTHON,
            Language::Toml => &toml::Toml,
            Language::Json => &json::Json,
            Language::Markdown => &markdown::Markdown,
            Language::CMake => &cmake::CMake,
        }
    }

    /// The lexer for a file in this language: the language's own lexer,
    /// with merge conflict markers recognized throughout (see the
    /// [`conflicts`] module).
    pub fn file_lexer(self) -> &'static dyn Lexer {
        &FILE_LEXERS[self.index() as usize]
    }
}

/// One [`Conflicts`](conflicts::Conflicts) wrapper per language, indexed
/// by [`Language::index`].
static FILE_LEXERS: [conflicts::Conflicts; Language::ALL.len()] = {
    let mut lexers = [conflicts::Conflicts {
        inner: Language::Plain,
    }; Language::ALL.len()];
    let mut i = 0;
    while i < Language::ALL.len() {
        lexers[i] = conflicts::Conflicts {
            inner: Language::ALL[i],
        };
        i += 1;
    }
    lexers
};

/// Which side of a merge conflict a line is on; see the [`conflicts`]
/// module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConflictSide {
    /// Between `<<<<<<<` and the next marker: the lines as this side had
    /// them. For the editor's own merges, the buffer's unsaved edits.
    Ours,
    /// Between `|||||||` and `=======`: the lines both sides started from.
    Base,
    /// Between `=======` and `>>>>>>>`: the lines as the other side has
    /// them. For the editor's own merges, the file on disk.
    Theirs,
}

/// A lexer for one language. Implementations are stateless; everything
/// that carries between lines is in the [`LexState`].
pub trait Lexer: Sync {
    /// Lex one line's content (without its terminator), starting in
    /// `state`. Tokens are appended to `out` in order, without overlaps,
    /// and the state at the end of the line is returned.
    ///
    /// The returned state must depend only on `state` and `line`; the
    /// tokens may not depend on anything else either.
    fn lex_line(&self, state: LexState, line: &[u8], out: &mut Vec<Token>) -> LexState;
}

/// The deepest nesting of contexts a state can hold. Deeper nesting (say,
/// template literals inside template literals five levels down) stays in
/// the innermost context that fit, which only affects highlighting of
/// text nobody should write.
const MAX_DEPTH: usize = 6;

/// A construct that continues across line breaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Context {
    /// Inside a block comment, `depth` levels deep for languages whose
    /// comments nest (1 for the rest).
    BlockComment { doc: bool, depth: u8 },
    /// Inside a string literal that can span lines.
    String {
        /// The closing quote character.
        quote: u8,
        /// Whether the string is delimited by three quotes.
        triple: bool,
        /// Whether backslash escapes are recognized.
        escapes: bool,
        /// For Rust raw strings, the number of `#` after the closing quote.
        hashes: u8,
    },
    /// Inside a JavaScript template literal, outside any `${ }`.
    Template,
    /// Inside a `${ }` in a template literal, `braces` levels of plain
    /// braces deep.
    TemplateExpression { braces: u8 },
    /// Inside a bracketed value in a data file (a TOML array or inline
    /// table), where keys are not expected.
    Value { close: u8 },
    /// A preprocessor directive continued from the previous line.
    Preprocessor,
    /// Inside a Markdown code fence opened with `len` of `fence` (`` ` ``
    /// or `~`), whose contents are lexed as the language with index
    /// `language` (see [`Language::index`]) if it is one we know. The
    /// fenced language's own state sits above this on the stack.
    Fence {
        fence: u8,
        len: u8,
        language: Option<u8>,
    },
    /// The previous Markdown line was prose (not blank), so this one may
    /// continue it.
    Paragraph,
    /// Inside a CMake bracket argument or comment, `[=[ ... ]=]` with
    /// `equals` equals signs.
    Bracket { equals: u8, comment: bool },
    /// Inside one side of a merge conflict. Always the outermost context;
    /// see the [`conflicts`] module.
    Conflict { side: ConflictSide },
}

/// The lexer state at a line boundary: a small stack of the
/// [`Context`]s the line starts inside. Fixed-size, so a highlighter can
/// keep one per line of a large file, and comparable, so re-lexing knows
/// when it has caught up with the previous results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LexState {
    stack: [Option<Context>; MAX_DEPTH],
    depth: u8,
}

impl Default for LexState {
    /// The state at the start of a file: not inside anything.
    fn default() -> LexState {
        LexState {
            stack: [None; MAX_DEPTH],
            depth: 0,
        }
    }
}

impl LexState {
    /// A marker for a line whose state hasn't been computed. Never equal
    /// to a real state, so re-lexing can't converge on it.
    pub(super) const UNKNOWN: LexState = LexState {
        stack: [None; MAX_DEPTH],
        depth: u8::MAX,
    };

    pub(super) fn is_unknown(&self) -> bool {
        self.depth == u8::MAX
    }

    /// The innermost context, if any.
    pub fn top(&self) -> Option<Context> {
        if self.depth == 0 {
            None
        } else {
            self.stack[self.depth as usize - 1]
        }
    }

    /// Enter a context. Returns whether it fit; if not, the state is
    /// unchanged and the caller should carry on as if it had entered.
    pub fn push(&mut self, context: Context) -> bool {
        if self.depth as usize >= MAX_DEPTH {
            return false;
        }
        self.stack[self.depth as usize] = Some(context);
        self.depth += 1;
        true
    }

    /// Leave the innermost context.
    pub fn pop(&mut self) -> Option<Context> {
        if self.depth == 0 {
            return None;
        }
        self.depth -= 1;
        self.stack[self.depth as usize].take()
    }

    /// Replace the innermost context.
    pub fn replace(&mut self, context: Context) {
        if self.depth == 0 {
            self.push(context);
        } else {
            self.stack[self.depth as usize - 1] = Some(context);
        }
    }

    pub fn is_default(&self) -> bool {
        self.depth == 0
    }

    /// The outermost context, if any.
    pub fn bottom(&self) -> Option<Context> {
        self.stack[0]
    }

    /// The state without its outermost context: what a lexer nested
    /// inside that context sees as its own state.
    pub(super) fn without_bottom(&self) -> LexState {
        let mut inner = LexState::default();
        if self.depth > 0 {
            inner.stack[..MAX_DEPTH - 1].copy_from_slice(&self.stack[1..]);
            inner.depth = self.depth - 1;
        }
        inner
    }

    /// A nested lexer's state wrapped back inside `outer`. If the nested
    /// state is at full depth its innermost context is dropped.
    pub(super) fn with_bottom(&self, outer: Context) -> LexState {
        let mut state = LexState::default();
        state.stack[0] = Some(outer);
        state.stack[1..].copy_from_slice(&self.stack[..MAX_DEPTH - 1]);
        state.depth = (self.depth + 1).min(MAX_DEPTH as u8);
        state
    }
}

/// Lex a whole text from the default state, returning the tokens of every
/// line. For tests and for highlighting text that isn't in a buffer, such
/// as a diff hunk.
pub fn lex_text(language: Language, text: &str) -> Vec<Vec<Token>> {
    let lexer = language.file_lexer();
    let mut state = LexState::default();
    let mut lines = Vec::new();
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        let mut tokens = Vec::new();
        state = lexer.lex_line(state, content.as_bytes(), &mut tokens);
        lines.push(tokens);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_indexed_in_order() {
        for (i, kind) in TokenKind::ALL.iter().enumerate() {
            assert_eq!(kind.index(), i, "{kind:?}");
        }
        let mut names: Vec<_> = TokenKind::ALL.iter().map(|k| k.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TokenKind::COUNT);
    }

    #[test]
    fn parents_terminate() {
        for kind in TokenKind::ALL {
            let mut current = kind;
            for _ in 0..10 {
                match current.parent() {
                    Some(parent) => current = parent,
                    None => break,
                }
            }
            assert!(
                current.parent().is_none() || current == kind,
                "{kind:?} loops"
            );
        }
    }

    #[test]
    fn language_from_path() {
        let of = |name: &str| Language::from_path(Path::new(name));
        assert_eq!(of("src/main.rs"), Some(Language::Rust));
        assert_eq!(of("a.H"), Some(Language::Cpp));
        assert_eq!(of("a.c"), Some(Language::C));
        assert_eq!(of("Cargo.lock"), Some(Language::Toml));
        assert_eq!(of("x.tsx"), Some(Language::TypeScript));
        assert_eq!(of("x.mjs"), Some(Language::JavaScript));
        assert_eq!(of("shader.wgsl"), Some(Language::Wgsl));
        assert_eq!(of("tsconfig.json"), Some(Language::Json));
        assert_eq!(of("setup.py"), Some(Language::Python));
        assert_eq!(of("README.md"), Some(Language::Markdown));
        assert_eq!(of("README"), Some(Language::Markdown));
        assert_eq!(of("src/CMakeLists.txt"), Some(Language::CMake));
        assert_eq!(of("Find.cmake"), Some(Language::CMake));
        assert_eq!(of("Makefile"), None);
        for language in Language::ALL {
            assert_eq!(Language::from_index(language.index()), Some(language));
        }
        assert_eq!(
            Language::from_fence_info("rust,ignore"),
            Some(Language::Rust)
        );
        assert_eq!(
            Language::from_fence_info("Python {.x}"),
            Some(Language::Python)
        );
        assert_eq!(Language::from_fence_info(""), None);
        assert_eq!(Language::from_fence_info("text"), None);
    }

    #[test]
    fn state_stack() {
        let mut state = LexState::default();
        assert!(state.is_default());
        assert_eq!(state.top(), None);
        for i in 0..MAX_DEPTH {
            assert!(state.push(Context::BlockComment {
                doc: false,
                depth: i as u8
            }));
        }
        assert!(!state.push(Context::Template));
        assert_eq!(
            state.top(),
            Some(Context::BlockComment {
                doc: false,
                depth: MAX_DEPTH as u8 - 1
            })
        );
        state.replace(Context::Template);
        assert_eq!(state.pop(), Some(Context::Template));
        assert_eq!(state.depth as usize, MAX_DEPTH - 1);
        while state.pop().is_some() {}
        assert_eq!(state, LexState::default());

        // Nesting a state inside an outer context and back.
        let fence = Context::Fence {
            fence: b'`',
            len: 3,
            language: Some(0),
        };
        let mut inner = LexState::default();
        inner.push(Context::Template);
        let outer = inner.with_bottom(fence);
        assert_eq!(outer.bottom(), Some(fence));
        assert_eq!(outer.top(), Some(Context::Template));
        assert_eq!(outer.without_bottom(), inner);
        assert_eq!(
            LexState::default().with_bottom(fence).without_bottom(),
            LexState::default()
        );
        let mut full = LexState::default();
        while full.push(Context::Template) {}
        let wrapped = full.with_bottom(fence);
        assert_eq!(wrapped.depth as usize, MAX_DEPTH);
        assert_eq!(wrapped.bottom(), Some(fence));
    }
}

/// Helpers shared by the lexer tests.
#[cfg(test)]
pub(super) mod test_support {
    use super::*;

    /// Render lexed lines as `text` and a parallel annotation line where
    /// each byte of a token is replaced by a letter for its kind, for
    /// compact golden tests.
    pub fn annotate(language: Language, text: &str) -> String {
        let lines = lex_text(language, text);
        let mut out = String::new();
        for (line, tokens) in text.lines().zip(lines) {
            let mut marks = vec![b' '; line.len()];
            for token in tokens {
                let c = letter(token.kind);
                for m in &mut marks[token.range.clone()] {
                    *m = c;
                }
            }
            out.push_str(line);
            out.push('\n');
            out.push_str(std::str::from_utf8(&marks).unwrap().trim_end());
            out.push('\n');
        }
        out
    }

    pub fn letter(kind: TokenKind) -> u8 {
        match kind {
            TokenKind::Text => b' ',
            TokenKind::Identifier => b'i',
            TokenKind::Keyword => b'k',
            TokenKind::ControlKeyword => b'K',
            TokenKind::Operator => b'o',
            TokenKind::Punctuation => b'p',
            TokenKind::Comment => b'c',
            TokenKind::DocComment => b'C',
            TokenKind::Number => b'n',
            TokenKind::String => b's',
            TokenKind::StringEscape => b'e',
            TokenKind::Char => b'h',
            TokenKind::Regex => b'r',
            TokenKind::Type => b't',
            TokenKind::PrimitiveType => b'T',
            TokenKind::Constant => b'N',
            TokenKind::Function => b'f',
            TokenKind::FunctionDefinition => b'F',
            TokenKind::TypeDefinition => b'D',
            TokenKind::VariableDefinition => b'v',
            TokenKind::Field => b'd',
            TokenKind::Namespace => b'm',
            TokenKind::Macro => b'M',
            TokenKind::Attribute => b'a',
            TokenKind::Lifetime => b'l',
            TokenKind::Label => b'L',
            TokenKind::Preprocessor => b'P',
            TokenKind::Key => b'y',
            TokenKind::Invalid => b'!',
            TokenKind::Heading => b'H',
            TokenKind::Strong => b'B',
            TokenKind::Emphasis => b'I',
            TokenKind::Link => b'A',
            TokenKind::ListMarker => b'-',
            TokenKind::Quote => b'Q',
            TokenKind::Variable => b'$',
            TokenKind::ConflictMarker => b'X',
        }
    }

    /// Check a golden: alternating source and annotation lines.
    #[track_caller]
    pub fn check(language: Language, golden: &str) {
        let lines: Vec<&str> = golden.lines().collect();
        let source: String = lines.iter().step_by(2).map(|l| format!("{l}\n")).collect();
        let expected: String = lines
            .chunks(2)
            .map(|pair| format!("{}\n{}\n", pair[0], pair.get(1).unwrap_or(&"").trim_end()))
            .collect();
        let actual = annotate(language, &source);
        assert_eq!(
            actual, expected,
            "\n--- actual ---\n{actual}\n--- expected ---\n{expected}"
        );
    }
}
