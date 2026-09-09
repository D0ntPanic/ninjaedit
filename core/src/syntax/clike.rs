//! A table-driven lexer for languages with C-like lexical structure:
//! identifiers, keywords, brackets, C or shell comments, quoted strings,
//! and numbers. Rust, C, C++, JavaScript, TypeScript, WGSL, and Python are
//! all instances of it, differing in their tables and in a handful of
//! feature flags (nested comments, raw strings, template literals, ...).
//!
//! Identifiers are classified by their surroundings, using only the
//! current line:
//!
//! * A definition keyword (`fn`, `struct`, `let`, `class`, `def`, ...)
//!   makes the next identifier a definition of the matching kind.
//! * `name!` is a macro; `name::` is a namespace, or a type if capitalized.
//! * After `.`, `->`, or `?.`: `name(` is a function, otherwise a field.
//! * Elsewhere, `name(` is a function, or a type if it is capitalized
//!   (`Some(x)`, `Foo(1)`), or a function again if it is all capitals
//!   (`ASSERT(x)`).
//! * `name:` is a field (struct literals, object literals, declarations),
//!   except after `case` and on lines with a `?`, where it is more likely
//!   a label or a ternary.
//! * `SCREAMING_CASE` is a constant, and other capitalized names are
//!   types.

use super::{Context, LexState, Lexer, Token, TokenKind};
use std::collections::HashMap;
use std::sync::OnceLock;

/// How a language spells its attributes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Attributes {
    None,
    /// `#[...]` and `#![...]`, taken whole.
    Hash,
    /// `@name`, optionally dotted, with any arguments lexed normally.
    At,
}

/// What a single quote starts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Quote {
    /// A string, like a double quote.
    String,
    /// A character literal of any length, as in C.
    Char,
    /// A one-character literal, or a lifetime or label, as in Rust.
    CharOrLifetime,
}

/// A language's lexical tables.
pub(super) struct Spec {
    keywords: &'static [&'static str],
    control: &'static [&'static str],
    primitives: &'static [&'static str],
    constants: &'static [&'static str],
    /// Keywords after which the next identifier is a definition of the
    /// given kind.
    definitions: &'static [(&'static str, TokenKind)],
    line_comment: &'static str,
    /// Prefixes that make a line comment a documentation comment. Checked
    /// as a longer match than `line_comment`.
    doc_line_comment: &'static [&'static str],
    /// Prefixes that start a documentation block comment.
    doc_block_comment: &'static [&'static str],
    block_comments: bool,
    nested_comments: bool,
    /// What a single quote introduces.
    single_quote: Quote,
    /// Whether three quotes in a row delimit a multi-line string.
    triple_quotes: bool,
    /// Whether an ordinary string may continue onto the next line without
    /// a trailing backslash.
    multiline_strings: bool,
    /// Identifiers that may directly precede a quote to prefix a string.
    string_prefixes: &'static [&'static str],
    /// Whether a prefix containing `r` makes a string raw (no escapes).
    raw_strings: bool,
    /// Whether raw strings take the Rust `r#"..."#` form, and `r#name` is
    /// a raw identifier.
    raw_hashes: bool,
    template_literals: bool,
    regex_literals: bool,
    preprocessor: bool,
    attributes: Attributes,
    /// `name!` is a macro invocation.
    macros: bool,
    /// `::` separates namespaces.
    scope_operator: bool,
    /// What a lowercase name after `::` is when it isn't called: a
    /// namespace in Rust (`use std::fmt`), a type in C++ (`std::string`).
    scope_member: TokenKind,
    /// `$` may appear in identifiers.
    dollar_identifiers: bool,
    /// `name:` marks a field.
    colon_fields: bool,
    /// Names ending in `_t` are types.
    underscore_t_types: bool,
    /// `->` is a member access (C), rather than a return type arrow.
    arrow_member: bool,
    /// The keyword tables merged into one map, built on first use.
    words: OnceLock<HashMap<&'static [u8], TokenKind>>,
}

impl Spec {
    /// The kind of a reserved word, if `word` is one.
    fn word_kind(&self, word: &[u8]) -> Option<TokenKind> {
        let words = self.words.get_or_init(|| {
            let mut map = HashMap::new();
            for (list, kind) in [
                (self.primitives, TokenKind::PrimitiveType),
                (self.keywords, TokenKind::Keyword),
                (self.control, TokenKind::ControlKeyword),
                (self.constants, TokenKind::Constant),
            ] {
                for word in list {
                    map.insert(word.as_bytes(), kind);
                }
            }
            map
        });
        words.get(word).copied()
    }
}

const RUST_DEFINITIONS: &[(&str, TokenKind)] = &[
    ("fn", TokenKind::FunctionDefinition),
    ("struct", TokenKind::TypeDefinition),
    ("enum", TokenKind::TypeDefinition),
    ("union", TokenKind::TypeDefinition),
    ("trait", TokenKind::TypeDefinition),
    ("type", TokenKind::TypeDefinition),
    ("impl", TokenKind::Type),
    ("dyn", TokenKind::Type),
    ("mod", TokenKind::Namespace),
    ("crate", TokenKind::Namespace),
    ("let", TokenKind::VariableDefinition),
    ("const", TokenKind::VariableDefinition),
    ("static", TokenKind::VariableDefinition),
    ("for", TokenKind::VariableDefinition),
];

pub(super) static RUST: Spec = Spec {
    keywords: &[
        "as",
        "async",
        "const",
        "crate",
        "dyn",
        "enum",
        "extern",
        "fn",
        "impl",
        "let",
        "macro_rules",
        "mod",
        "move",
        "mut",
        "pub",
        "ref",
        "self",
        "Self",
        "static",
        "struct",
        "super",
        "trait",
        "type",
        "union",
        "unsafe",
        "use",
        "where",
    ],
    control: &[
        "await", "break", "continue", "else", "for", "if", "in", "loop", "match", "return",
        "while", "yield",
    ],
    primitives: &[
        "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "str", "u8",
        "u16", "u32", "u64", "u128", "usize",
    ],
    constants: &["true", "false"],
    definitions: RUST_DEFINITIONS,
    line_comment: "//",
    doc_line_comment: &["///", "//!"],
    doc_block_comment: &["/**", "/*!"],
    block_comments: true,
    nested_comments: true,
    single_quote: Quote::CharOrLifetime,
    triple_quotes: false,
    multiline_strings: true,
    string_prefixes: &["b", "r", "br", "c", "cr"],
    raw_strings: true,
    raw_hashes: true,
    template_literals: false,
    regex_literals: false,
    preprocessor: false,
    attributes: Attributes::Hash,
    macros: true,
    scope_operator: true,
    scope_member: TokenKind::Namespace,
    dollar_identifiers: false,
    colon_fields: true,
    underscore_t_types: false,
    arrow_member: false,
    words: OnceLock::new(),
};

const C_KEYWORDS: &[&str] = &[
    "auto",
    "const",
    "enum",
    "extern",
    "inline",
    "register",
    "restrict",
    "sizeof",
    "static",
    "struct",
    "typedef",
    "union",
    "volatile",
    "_Alignas",
    "_Alignof",
    "_Atomic",
    "_Generic",
    "_Noreturn",
    "_Static_assert",
    "_Thread_local",
    "alignas",
    "alignof",
    "static_assert",
    "thread_local",
    "typeof",
];

const C_CONTROL: &[&str] = &[
    "break", "case", "continue", "default", "do", "else", "for", "goto", "if", "return", "switch",
    "while",
];

const C_PRIMITIVES: &[&str] = &[
    "bool",
    "char",
    "double",
    "float",
    "int",
    "long",
    "short",
    "signed",
    "unsigned",
    "void",
    "_Bool",
    "_Complex",
    "_Imaginary",
    "int8_t",
    "int16_t",
    "int32_t",
    "int64_t",
    "uint8_t",
    "uint16_t",
    "uint32_t",
    "uint64_t",
    "size_t",
    "ssize_t",
    "ptrdiff_t",
    "intptr_t",
    "uintptr_t",
    "wchar_t",
    "char16_t",
    "char32_t",
];

const C_DEFINITIONS: &[(&str, TokenKind)] = &[
    ("struct", TokenKind::Type),
    ("enum", TokenKind::Type),
    ("union", TokenKind::Type),
];

pub(super) static C: Spec = Spec {
    keywords: C_KEYWORDS,
    control: C_CONTROL,
    primitives: C_PRIMITIVES,
    constants: &["true", "false", "NULL"],
    definitions: C_DEFINITIONS,
    line_comment: "//",
    doc_line_comment: &["///", "//!"],
    doc_block_comment: &["/**", "/*!"],
    block_comments: true,
    nested_comments: false,
    single_quote: Quote::Char,
    triple_quotes: false,
    multiline_strings: false,
    string_prefixes: &["L", "u", "U", "u8"],
    raw_strings: false,
    raw_hashes: false,
    template_literals: false,
    regex_literals: false,
    preprocessor: true,
    attributes: Attributes::None,
    macros: false,
    scope_operator: false,
    scope_member: TokenKind::Namespace,
    dollar_identifiers: false,
    colon_fields: true,
    underscore_t_types: true,
    arrow_member: true,
    words: OnceLock::new(),
};

pub(super) static CPP: Spec = Spec {
    keywords: &[
        "alignas",
        "alignof",
        "asm",
        "auto",
        "class",
        "concept",
        "const",
        "consteval",
        "constexpr",
        "constinit",
        "const_cast",
        "decltype",
        "delete",
        "dynamic_cast",
        "enum",
        "explicit",
        "export",
        "extern",
        "final",
        "friend",
        "inline",
        "module",
        "mutable",
        "namespace",
        "new",
        "noexcept",
        "operator",
        "override",
        "private",
        "protected",
        "public",
        "register",
        "reinterpret_cast",
        "requires",
        "sizeof",
        "static",
        "static_assert",
        "static_cast",
        "struct",
        "template",
        "this",
        "thread_local",
        "typedef",
        "typeid",
        "typename",
        "union",
        "using",
        "virtual",
        "volatile",
        "import",
    ],
    control: &[
        "break",
        "case",
        "catch",
        "continue",
        "co_await",
        "co_return",
        "co_yield",
        "default",
        "do",
        "else",
        "for",
        "goto",
        "if",
        "return",
        "switch",
        "throw",
        "try",
        "while",
    ],
    primitives: C_PRIMITIVES,
    constants: &["true", "false", "NULL", "nullptr"],
    definitions: &[
        ("struct", TokenKind::Type),
        ("class", TokenKind::Type),
        ("enum", TokenKind::Type),
        ("union", TokenKind::Type),
        ("typename", TokenKind::Type),
        ("namespace", TokenKind::Namespace),
        ("using", TokenKind::Type),
    ],
    line_comment: "//",
    doc_line_comment: &["///", "//!"],
    doc_block_comment: &["/**", "/*!"],
    block_comments: true,
    nested_comments: false,
    single_quote: Quote::Char,
    triple_quotes: false,
    multiline_strings: false,
    string_prefixes: &["L", "u", "U", "u8", "R", "LR", "uR", "UR", "u8R"],
    raw_strings: false,
    raw_hashes: false,
    template_literals: false,
    regex_literals: false,
    preprocessor: true,
    attributes: Attributes::None,
    macros: false,
    scope_operator: true,
    scope_member: TokenKind::Type,
    dollar_identifiers: false,
    colon_fields: true,
    underscore_t_types: true,
    arrow_member: true,
    words: OnceLock::new(),
};

const JS_KEYWORDS: &[&str] = &[
    "as",
    "async",
    "class",
    "const",
    "debugger",
    "delete",
    "export",
    "extends",
    "from",
    "function",
    "get",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "of",
    "set",
    "static",
    "super",
    "this",
    "typeof",
    "var",
    "void",
    "with",
];

const JS_CONTROL: &[&str] = &[
    "await", "break", "case", "catch", "continue", "default", "do", "else", "finally", "for", "if",
    "return", "switch", "throw", "try", "while", "yield",
];

const JS_CONSTANTS: &[&str] = &["true", "false", "null", "undefined", "NaN", "Infinity"];

const JS_DEFINITIONS: &[(&str, TokenKind)] = &[
    ("function", TokenKind::FunctionDefinition),
    ("class", TokenKind::TypeDefinition),
    ("let", TokenKind::VariableDefinition),
    ("const", TokenKind::VariableDefinition),
    ("var", TokenKind::VariableDefinition),
];

pub(super) static JAVASCRIPT: Spec = Spec {
    keywords: JS_KEYWORDS,
    control: JS_CONTROL,
    primitives: &[],
    constants: JS_CONSTANTS,
    definitions: JS_DEFINITIONS,
    line_comment: "//",
    doc_line_comment: &[],
    doc_block_comment: &["/**"],
    block_comments: true,
    nested_comments: false,
    single_quote: Quote::String,
    triple_quotes: false,
    multiline_strings: false,
    string_prefixes: &[],
    raw_strings: false,
    raw_hashes: false,
    template_literals: true,
    regex_literals: true,
    preprocessor: false,
    attributes: Attributes::At,
    macros: false,
    scope_operator: false,
    scope_member: TokenKind::Namespace,
    dollar_identifiers: true,
    colon_fields: true,
    underscore_t_types: false,
    arrow_member: false,
    words: OnceLock::new(),
};

pub(super) static TYPESCRIPT: Spec = Spec {
    keywords: &[
        "abstract",
        "as",
        "asserts",
        "async",
        "class",
        "const",
        "constructor",
        "debugger",
        "declare",
        "delete",
        "enum",
        "export",
        "extends",
        "from",
        "function",
        "get",
        "implements",
        "import",
        "in",
        "infer",
        "instanceof",
        "interface",
        "is",
        "keyof",
        "let",
        "module",
        "namespace",
        "new",
        "of",
        "out",
        "override",
        "private",
        "protected",
        "public",
        "readonly",
        "require",
        "satisfies",
        "set",
        "static",
        "super",
        "this",
        "type",
        "typeof",
        "var",
        "with",
    ],
    control: JS_CONTROL,
    primitives: &[
        "any", "bigint", "boolean", "never", "number", "object", "string", "symbol", "unknown",
        "void",
    ],
    constants: JS_CONSTANTS,
    definitions: &[
        ("function", TokenKind::FunctionDefinition),
        ("class", TokenKind::TypeDefinition),
        ("interface", TokenKind::TypeDefinition),
        ("enum", TokenKind::TypeDefinition),
        ("type", TokenKind::TypeDefinition),
        ("namespace", TokenKind::Namespace),
        ("module", TokenKind::Namespace),
        ("let", TokenKind::VariableDefinition),
        ("const", TokenKind::VariableDefinition),
        ("var", TokenKind::VariableDefinition),
    ],
    line_comment: "//",
    doc_line_comment: &[],
    doc_block_comment: &["/**"],
    block_comments: true,
    nested_comments: false,
    single_quote: Quote::String,
    triple_quotes: false,
    multiline_strings: false,
    string_prefixes: &[],
    raw_strings: false,
    raw_hashes: false,
    template_literals: true,
    regex_literals: true,
    preprocessor: false,
    attributes: Attributes::At,
    macros: false,
    scope_operator: false,
    scope_member: TokenKind::Namespace,
    dollar_identifiers: true,
    colon_fields: true,
    underscore_t_types: false,
    arrow_member: false,
    words: OnceLock::new(),
};

pub(super) static WGSL: Spec = Spec {
    keywords: &[
        "alias",
        "const",
        "const_assert",
        "diagnostic",
        "enable",
        "fn",
        "let",
        "override",
        "requires",
        "struct",
        "var",
    ],
    control: &[
        "break",
        "case",
        "continue",
        "continuing",
        "default",
        "discard",
        "else",
        "for",
        "if",
        "loop",
        "return",
        "switch",
        "while",
    ],
    primitives: &[
        "bool",
        "f16",
        "f32",
        "i32",
        "u32",
        "vec2",
        "vec3",
        "vec4",
        "vec2f",
        "vec3f",
        "vec4f",
        "vec2i",
        "vec3i",
        "vec4i",
        "vec2u",
        "vec3u",
        "vec4u",
        "vec2h",
        "vec3h",
        "vec4h",
        "mat2x2",
        "mat2x3",
        "mat2x4",
        "mat3x2",
        "mat3x3",
        "mat3x4",
        "mat4x2",
        "mat4x3",
        "mat4x4",
        "mat2x2f",
        "mat3x3f",
        "mat4x4f",
        "array",
        "atomic",
        "ptr",
        "sampler",
        "sampler_comparison",
        "texture_1d",
        "texture_2d",
        "texture_2d_array",
        "texture_3d",
        "texture_cube",
        "texture_cube_array",
        "texture_multisampled_2d",
        "texture_storage_1d",
        "texture_storage_2d",
        "texture_storage_2d_array",
        "texture_storage_3d",
        "texture_depth_2d",
        "texture_depth_2d_array",
        "texture_depth_cube",
        "texture_depth_cube_array",
        "texture_depth_multisampled_2d",
        "texture_external",
    ],
    constants: &["true", "false"],
    definitions: &[
        ("fn", TokenKind::FunctionDefinition),
        ("struct", TokenKind::TypeDefinition),
        ("alias", TokenKind::TypeDefinition),
        ("let", TokenKind::VariableDefinition),
        ("var", TokenKind::VariableDefinition),
        ("const", TokenKind::VariableDefinition),
        ("override", TokenKind::VariableDefinition),
    ],
    line_comment: "//",
    doc_line_comment: &[],
    doc_block_comment: &[],
    block_comments: true,
    nested_comments: true,
    single_quote: Quote::Char,
    triple_quotes: false,
    multiline_strings: false,
    string_prefixes: &[],
    raw_strings: false,
    raw_hashes: false,
    template_literals: false,
    regex_literals: false,
    preprocessor: false,
    attributes: Attributes::At,
    macros: false,
    scope_operator: false,
    scope_member: TokenKind::Namespace,
    dollar_identifiers: false,
    colon_fields: true,
    underscore_t_types: false,
    arrow_member: false,
    words: OnceLock::new(),
};

pub(super) static PYTHON: Spec = Spec {
    keywords: &[
        "and", "as", "assert", "async", "class", "cls", "def", "del", "from", "global", "import",
        "in", "is", "lambda", "nonlocal", "not", "or", "pass", "self", "with",
    ],
    control: &[
        "await", "break", "case", "continue", "elif", "else", "except", "finally", "for", "if",
        "match", "raise", "return", "try", "while", "yield",
    ],
    primitives: &[
        "bool",
        "bytes",
        "complex",
        "dict",
        "float",
        "frozenset",
        "int",
        "list",
        "object",
        "set",
        "str",
        "tuple",
    ],
    constants: &["True", "False", "None", "NotImplemented", "Ellipsis"],
    definitions: &[
        ("def", TokenKind::FunctionDefinition),
        ("class", TokenKind::TypeDefinition),
        ("for", TokenKind::VariableDefinition),
        ("as", TokenKind::VariableDefinition),
        ("global", TokenKind::VariableDefinition),
        ("nonlocal", TokenKind::VariableDefinition),
    ],
    line_comment: "#",
    doc_line_comment: &[],
    doc_block_comment: &[],
    block_comments: false,
    nested_comments: false,
    single_quote: Quote::String,
    triple_quotes: true,
    multiline_strings: false,
    string_prefixes: &[
        "r", "u", "b", "f", "t", "rb", "br", "rf", "fr", "rt", "tr", "R", "U", "B", "F", "T", "Rb",
        "rB", "RB", "bR", "Br", "BR", "Rf", "rF", "RF", "fR", "Fr", "FR",
    ],
    raw_strings: true,
    raw_hashes: false,
    template_literals: false,
    regex_literals: false,
    preprocessor: false,
    attributes: Attributes::At,
    macros: false,
    scope_operator: false,
    scope_member: TokenKind::Namespace,
    dollar_identifiers: false,
    colon_fields: false,
    underscore_t_types: false,
    arrow_member: false,
    words: OnceLock::new(),
};

/// The previous significant token on the line, as far as classification
/// cares.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Prev {
    /// Nothing yet on this line.
    Start,
    Identifier,
    /// A keyword other than one that stands for a value.
    Keyword,
    /// A literal, a closing bracket, or a keyword such as `this` that
    /// stands for a value. A `/` after one of these is division.
    Value,
    /// A member access operator: `.`, `->`, `?.`.
    Dot,
    /// The scope operator `::`.
    Scope,
    Operator,
}

/// The lexing of one line.
struct Line<'a> {
    spec: &'a Spec,
    line: &'a [u8],
    pos: usize,
    out: &'a mut Vec<Token>,
    prev: Prev,
    /// The last keyword seen, for context checks such as `case x:`.
    prev_keyword: &'a [u8],
    /// The kind the next identifier gets, set by a definition keyword.
    pending: Option<TokenKind>,
    /// A pending kind set aside while inside `<...>` right after the
    /// keyword, as in `var<private> name` or `impl<T> Name`.
    stashed: Option<TokenKind>,
    /// Whether a `?` operator appeared earlier on the line, which makes a
    /// later `name:` more likely a ternary branch than a field.
    saw_question: bool,
    /// Whether this line continues a preprocessor directive.
    directive: bool,
    /// Whether the last `::` followed a type name.
    scope_after_type: bool,
}

impl Lexer for Spec {
    fn lex_line(&self, mut state: LexState, line: &[u8], out: &mut Vec<Token>) -> LexState {
        let mut lx = Line {
            spec: self,
            line,
            pos: 0,
            out,
            prev: Prev::Start,
            prev_keyword: b"",
            pending: None,
            stashed: None,
            saw_question: false,
            directive: false,
            scope_after_type: false,
        };
        if state.top() == Some(Context::Preprocessor) {
            state.pop();
            lx.directive = true;
        }
        loop {
            let more = match state.top() {
                Some(Context::BlockComment { doc, depth }) => {
                    lx.continue_block_comment(&mut state, doc, depth)
                }
                Some(Context::String {
                    quote,
                    triple,
                    escapes,
                    hashes,
                }) => lx.continue_string(&mut state, quote, triple, escapes, hashes),
                Some(Context::Template) => lx.continue_template(&mut state),
                Some(Context::TemplateExpression { .. })
                | Some(Context::Value { .. })
                | Some(Context::Fence { .. })
                | Some(Context::Paragraph)
                | Some(Context::Bracket { .. })
                | None => lx.next_token(&mut state),
                Some(Context::Preprocessor) => unreachable!("popped at line start"),
            };
            if !more {
                break;
            }
        }
        if self.preprocessor && lx.directive && line.ends_with(b"\\") {
            state.push(Context::Preprocessor);
        }
        state
    }
}

fn is_identifier_start(b: u8, dollar: bool) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80 || (dollar && b == b'$')
}

fn is_identifier_char(b: u8, dollar: bool) -> bool {
    is_identifier_start(b, dollar) || b.is_ascii_digit()
}

fn is_screaming(word: &[u8]) -> bool {
    word.len() > 1
        && word.iter().any(|b| b.is_ascii_uppercase())
        && word
            .iter()
            .all(|&b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

fn is_capitalized(word: &[u8]) -> bool {
    word.first().is_some_and(|b| b.is_ascii_uppercase())
}

impl<'a> Line<'a> {
    fn at(&self, i: usize) -> u8 {
        self.line.get(i).copied().unwrap_or(0)
    }

    fn rest(&self) -> &'a [u8] {
        &self.line[self.pos..]
    }

    fn starts_with(&self, prefix: &str) -> bool {
        self.rest().starts_with(prefix.as_bytes())
    }

    fn emit(&mut self, start: usize, end: usize, kind: TokenKind) {
        if end > start {
            self.out.push(Token {
                range: start..end,
                kind,
            });
        }
    }

    /// The next non-space byte at or after `i`, and where it is.
    fn peek_significant(&self, mut i: usize) -> (u8, usize) {
        while i < self.line.len() && (self.line[i] == b' ' || self.line[i] == b'\t') {
            i += 1;
        }
        (self.at(i), i)
    }

    // ----- Contexts continued from previous lines ---------------------------

    /// Lex the rest of a block comment. Returns whether the line goes on
    /// after it.
    fn continue_block_comment(&mut self, state: &mut LexState, doc: bool, mut depth: u8) -> bool {
        let start = self.pos;
        let kind = if doc {
            TokenKind::DocComment
        } else {
            TokenKind::Comment
        };
        let mut i = self.pos;
        while i < self.line.len() {
            if self.line[i..].starts_with(b"*/") {
                i += 2;
                depth -= 1;
                if depth == 0 {
                    self.emit(start, i, kind);
                    self.pos = i;
                    state.pop();
                    return true;
                }
            } else if self.spec.nested_comments && self.line[i..].starts_with(b"/*") {
                i += 2;
                depth = depth.saturating_add(1);
            } else {
                i += 1;
            }
        }
        self.emit(start, self.line.len(), kind);
        self.pos = self.line.len();
        state.replace(Context::BlockComment { doc, depth });
        false
    }

    /// Lex the rest of a string. Returns whether the line goes on after it.
    fn continue_string(
        &mut self,
        state: &mut LexState,
        quote: u8,
        triple: bool,
        escapes: bool,
        hashes: u8,
    ) -> bool {
        let mut segment = self.pos;
        let mut i = self.pos;
        while i < self.line.len() {
            let b = self.line[i];
            if escapes && b == b'\\' {
                let end = self.escape_end(i);
                self.emit(segment, i, TokenKind::String);
                self.emit(i, end, TokenKind::StringEscape);
                i = end;
                segment = end;
                continue;
            }
            if b == quote {
                let closes = if triple {
                    self.line[i..].starts_with(&[quote, quote, quote])
                } else {
                    (0..hashes as usize).all(|h| self.at(i + 1 + h) == b'#')
                };
                if closes {
                    let end = i + if triple { 3 } else { 1 + hashes as usize };
                    self.emit(segment, end, TokenKind::String);
                    self.pos = end;
                    state.pop();
                    self.prev = Prev::Value;
                    self.pending = None;
                    return true;
                }
            }
            if self.spec.template_literals && quote == b'`' && self.line[i..].starts_with(b"${") {
                self.emit(segment, i, TokenKind::String);
                self.emit(i, i + 2, TokenKind::Punctuation);
                self.pos = i + 2;
                state.replace(Context::Template);
                state.push(Context::TemplateExpression { braces: 0 });
                self.prev = Prev::Start;
                return true;
            }
            i += 1;
        }
        self.emit(segment, self.line.len(), TokenKind::String);
        self.pos = self.line.len();
        // Does the string continue on the next line? A trailing backslash
        // continues any string that has escapes.
        let continued = triple
            || self.spec.multiline_strings
            || (quote == b'`' && self.spec.template_literals)
            || (escapes && self.line.ends_with(b"\\") && !self.line.ends_with(b"\\\\"));
        if !continued {
            state.pop();
        }
        false
    }

    /// Lex the body of a template literal. Returns whether the line goes
    /// on after it.
    fn continue_template(&mut self, state: &mut LexState) -> bool {
        self.continue_string(state, b'`', false, true, 0)
    }

    /// The end of the escape sequence starting at the backslash at `i`.
    fn escape_end(&self, i: usize) -> usize {
        let next = self.at(i + 1);
        let hex_run = |from: usize, max: usize| {
            let mut j = from;
            while j < self.line.len() && j - from < max && self.line[j].is_ascii_hexdigit() {
                j += 1;
            }
            j
        };
        match next {
            0 => i + 1,
            b'x' => hex_run(i + 2, 2),
            b'u' if self.at(i + 2) == b'{' => {
                let mut j = i + 3;
                while j < self.line.len() && self.line[j] != b'}' {
                    j += 1;
                }
                (j + 1).min(self.line.len())
            }
            b'u' => hex_run(i + 2, 4),
            b'U' => hex_run(i + 2, 8),
            b'N' if self.at(i + 2) == b'{' => {
                let mut j = i + 3;
                while j < self.line.len() && self.line[j] != b'}' {
                    j += 1;
                }
                (j + 1).min(self.line.len())
            }
            _ if next < 0x80 => i + 2,
            _ => {
                // A backslash before a multi-byte character escapes the
                // whole character.
                let mut j = i + 2;
                while j < self.line.len() && (self.line[j] & 0xC0) == 0x80 {
                    j += 1;
                }
                j
            }
        }
    }

    // ----- Normal lexing ----------------------------------------------------

    /// Lex one token in normal (or template expression) context. Returns
    /// whether there is more of the line to lex.
    fn next_token(&mut self, state: &mut LexState) -> bool {
        // Skip whitespace.
        while self.pos < self.line.len() && matches!(self.line[self.pos], b' ' | b'\t' | b'\r') {
            self.pos += 1;
        }
        if self.pos >= self.line.len() {
            return false;
        }
        let start = self.pos;
        let line = self.line;
        let b = line[start];
        let spec = self.spec;

        // Comments.
        if self.starts_with(spec.line_comment) {
            let doc = spec
                .doc_line_comment
                .iter()
                .any(|d| self.starts_with(d) && self.at(start + d.len()) != b'/');
            let kind = if doc {
                TokenKind::DocComment
            } else {
                TokenKind::Comment
            };
            self.emit(start, self.line.len(), kind);
            self.pos = self.line.len();
            return false;
        }
        if spec.block_comments && self.starts_with("/*") {
            let doc = spec
                .doc_block_comment
                .iter()
                .any(|d| self.starts_with(d) && !self.starts_with("/**/"));
            self.pos += 2;
            state.push(Context::BlockComment { doc, depth: 1 });
            // Emit the comment from its opening delimiter.
            let opened = self.pos;
            let more = self.continue_block_comment(state, doc, 1);
            // Stretch the token back over the delimiter.
            if let Some(last) = self.out.last_mut()
                && last.range.start == opened
            {
                last.range.start = start;
            } else {
                self.emit(
                    start,
                    opened,
                    if doc {
                        TokenKind::DocComment
                    } else {
                        TokenKind::Comment
                    },
                );
            }
            return more;
        }

        // Preprocessor directives.
        if spec.preprocessor && b == b'#' && self.prev == Prev::Start && !self.directive {
            return self.directive_token(state);
        }

        // Attributes.
        match spec.attributes {
            Attributes::Hash
                if b == b'#' && (self.at(start + 1) == b'[' || self.starts_with("#![")) =>
            {
                let mut depth = 0i32;
                let mut i = start;
                while i < self.line.len() {
                    match self.line[i] {
                        b'[' => depth += 1,
                        b']' => {
                            depth -= 1;
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                self.emit(start, i, TokenKind::Attribute);
                self.pos = i;
                self.prev = Prev::Start;
                return true;
            }
            Attributes::At if b == b'@' && is_identifier_start(self.at(start + 1), false) => {
                let mut i = start + 1;
                while i < self.line.len()
                    && (is_identifier_char(self.line[i], false)
                        || (self.line[i] == b'.' && is_identifier_start(self.at(i + 1), false)))
                {
                    i += 1;
                }
                self.emit(start, i, TokenKind::Attribute);
                self.pos = i;
                self.prev = Prev::Operator;
                return true;
            }
            _ => {}
        }

        // Identifiers, and strings with prefixes.
        if is_identifier_start(b, spec.dollar_identifiers) {
            let mut end = start + 1;
            while end < line.len() && is_identifier_char(line[end], spec.dollar_identifiers) {
                end += 1;
            }
            let word = &line[start..end];
            if !spec.string_prefixes.is_empty()
                && spec.string_prefixes.iter().any(|p| p.as_bytes() == word)
            {
                let raw = spec.raw_strings && word.iter().any(|&c| c == b'r' || c == b'R');
                let mut hashes = 0;
                let mut q = end;
                if raw && spec.raw_hashes {
                    while self.at(q) == b'#' {
                        hashes += 1;
                        q += 1;
                    }
                }
                let quote = self.at(q);
                if quote == b'"' || (quote == b'\'' && spec.single_quote == Quote::String) {
                    return self.string_token(state, start, q, quote, !raw, hashes);
                }
                if quote == b'\'' && hashes == 0 {
                    return self.single_quote_token(start, q);
                }
                if hashes == 1 && is_identifier_start(quote, false) {
                    // A raw identifier: r#name.
                    let mut e = q;
                    while e < line.len() && is_identifier_char(line[e], false) {
                        e += 1;
                    }
                    return self.identifier_token(start, e, &line[q..e]);
                }
            }
            return self.identifier_token(start, end, word);
        }
        if b == b'"' || (b == b'\'' && spec.single_quote == Quote::String) {
            return self.string_token(state, start, start, b, true, 0);
        }
        if b == b'`' && spec.template_literals {
            self.emit(start, start + 1, TokenKind::String);
            self.pos = start + 1;
            state.push(Context::Template);
            let more = self.continue_template(state);
            self.merge_string_start(start);
            return more;
        }
        if b == b'\'' {
            return self.single_quote_token(start, start);
        }

        // Numbers.
        if b.is_ascii_digit()
            || (b == b'.'
                && self.at(start + 1).is_ascii_digit()
                && !matches!(self.prev, Prev::Identifier | Prev::Value | Prev::Dot))
        {
            let end = self.number_end(start);
            self.emit(start, end, TokenKind::Number);
            self.pos = end;
            self.prev = Prev::Value;
            self.pending = None;
            return true;
        }

        // Regular expressions.
        if spec.regex_literals
            && b == b'/'
            && !matches!(self.prev, Prev::Identifier | Prev::Value)
            && let Some(end) = self.regex_end(start)
        {
            self.emit(start, end, TokenKind::Regex);
            self.pos = end;
            self.prev = Prev::Value;
            self.pending = None;
            return true;
        }

        // Template expression braces.
        if let Some(Context::TemplateExpression { braces }) = state.top() {
            if b == b'{' {
                state.replace(Context::TemplateExpression {
                    braces: braces.saturating_add(1),
                });
            } else if b == b'}' {
                self.emit(start, start + 1, TokenKind::Punctuation);
                self.pos = start + 1;
                if braces == 0 {
                    state.pop();
                    let more = self.continue_template(state);
                    return more;
                }
                state.replace(Context::TemplateExpression { braces: braces - 1 });
                self.prev = Prev::Value;
                return true;
            }
        }

        // Operators and punctuation.
        self.operator_token(start)
    }

    fn directive_token(&mut self, _state: &mut LexState) -> bool {
        let start = self.pos;
        let (_, mut i) = self.peek_significant(start + 1);
        let name_start = i;
        while i < self.line.len() && self.line[i].is_ascii_alphabetic() {
            i += 1;
        }
        self.emit(start, i, TokenKind::Preprocessor);
        self.pos = i;
        self.directive = true;
        self.prev = Prev::Keyword;
        let name = &self.line[name_start..i];
        if name == b"include" || name == b"import" || name == b"include_next" {
            let (b, j) = self.peek_significant(i);
            if b == b'<' {
                let mut e = j + 1;
                while e < self.line.len() && self.line[e] != b'>' {
                    e += 1;
                }
                let e = (e + 1).min(self.line.len());
                self.emit(j, e, TokenKind::String);
                self.pos = e;
                self.prev = Prev::Value;
            }
        }
        true
    }

    /// Lex a string starting with its opening quote at `quote_pos` (the
    /// token starts at `start`, which may include a prefix).
    fn string_token(
        &mut self,
        state: &mut LexState,
        start: usize,
        quote_pos: usize,
        quote: u8,
        escapes: bool,
        hashes: u8,
    ) -> bool {
        let triple = self.spec.triple_quotes
            && self.at(quote_pos + 1) == quote
            && self.at(quote_pos + 2) == quote;
        let body = quote_pos + if triple { 3 } else { 1 };
        self.emit(start, body, TokenKind::String);
        self.pos = body;
        state.push(Context::String {
            quote,
            triple,
            escapes,
            hashes,
        });
        let more = self.continue_string(state, quote, triple, escapes, hashes);
        self.merge_string_start(start);
        more
    }

    /// After lexing a string body, join the opening-quote token with the
    /// first body token so a string without escapes is a single token.
    fn merge_string_start(&mut self, start: usize) {
        let n = self.out.len();
        if n >= 2
            && self.out[n - 2].range.start == start
            && self.out[n - 2].kind == TokenKind::String
            && self.out[n - 1].kind == TokenKind::String
            && self.out[n - 1].range.start == self.out[n - 2].range.end
        {
            let end = self.out[n - 1].range.end;
            self.out.pop();
            self.out[n - 2].range.end = end;
        }
    }

    /// A single quote in a language where it is not a string quote: a
    /// character literal, or in Rust possibly a lifetime or label.
    fn single_quote_token(&mut self, start: usize, quote_pos: usize) -> bool {
        let next = self.at(quote_pos + 1);
        if self.spec.single_quote == Quote::CharOrLifetime && next != b'\\' && start == quote_pos {
            // Skip one whole character and see if a quote follows.
            let mut i = quote_pos + 1;
            if i < self.line.len() {
                i += 1;
                while i < self.line.len() && (self.line[i] & 0xC0) == 0x80 {
                    i += 1;
                }
            }
            if self.at(i) != b'\'' && is_identifier_start(next, false) {
                let mut end = quote_pos + 1;
                while end < self.line.len() && is_identifier_char(self.line[end], false) {
                    end += 1;
                }
                let label = (self.at(end) == b':' && self.at(end + 1) != b':')
                    || self.prev_keyword == b"break"
                    || self.prev_keyword == b"continue";
                let kind = if label {
                    TokenKind::Label
                } else {
                    TokenKind::Lifetime
                };
                self.emit(start, end, kind);
                self.pos = end;
                self.prev = Prev::Operator;
                self.pending = None;
                return true;
            }
        }
        // A character literal, to the closing quote on this line.
        let mut segment = start;
        let mut i = quote_pos + 1;
        let mut end = self.line.len();
        while i < self.line.len() {
            match self.line[i] {
                b'\\' => {
                    let e = self.escape_end(i);
                    self.emit(segment, i, TokenKind::Char);
                    self.emit(i, e, TokenKind::StringEscape);
                    segment = e;
                    i = e;
                }
                b'\'' => {
                    end = i + 1;
                    break;
                }
                _ => i += 1,
            }
        }
        self.emit(segment, end, TokenKind::Char);
        self.pos = end;
        self.prev = Prev::Value;
        self.pending = None;
        true
    }

    fn number_end(&self, start: usize) -> usize {
        let mut i = start;
        let hex = self.at(i) == b'0' && matches!(self.at(i + 1), b'x' | b'X');
        if self.at(i) == b'0' && matches!(self.at(i + 1), b'x' | b'X' | b'b' | b'B' | b'o' | b'O') {
            i += 2;
        }
        let digit = |b: u8| if hex { b.is_ascii_hexdigit() } else { b.is_ascii_digit() } || b == b'_';
        while i < self.line.len() && digit(self.line[i]) {
            i += 1;
        }
        if !hex && self.at(i) == b'.' {
            let next = self.at(i + 1);
            if next.is_ascii_digit() {
                i += 1;
                while i < self.line.len() && digit(self.line[i]) {
                    i += 1;
                }
            } else if next != b'.' && !is_identifier_start(next, false) && i > start {
                i += 1;
            }
        }
        if !hex && matches!(self.at(i), b'e' | b'E') {
            let mut j = i + 1;
            if matches!(self.at(j), b'+' | b'-') {
                j += 1;
            }
            if self.at(j).is_ascii_digit() {
                i = j;
                while i < self.line.len() && self.line[i].is_ascii_digit() {
                    i += 1;
                }
            }
        }
        // Suffixes: u32, f64, ULL, n, and so on.
        while i < self.line.len() && is_identifier_char(self.line[i], false) {
            i += 1;
        }
        i
    }

    /// The end of a regex literal starting at `start`, if the rest of the
    /// line contains a closing `/`.
    fn regex_end(&self, start: usize) -> Option<usize> {
        let mut i = start + 1;
        let mut in_class = false;
        while i < self.line.len() {
            match self.line[i] {
                b'\\' => i += 2,
                b'[' => {
                    in_class = true;
                    i += 1;
                }
                b']' => {
                    in_class = false;
                    i += 1;
                }
                b'/' if !in_class => {
                    i += 1;
                    while i < self.line.len() && self.line[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    return Some(i);
                }
                _ => i += 1,
            }
        }
        None
    }

    fn identifier_token(&mut self, start: usize, end: usize, word: &[u8]) -> bool {
        let spec = self.spec;
        let line = self.line;
        let (next, next_pos) = self.peek_significant(end);
        let next2 = self.at(next_pos + 1);
        let called = next == b'(';
        let scoped = spec.scope_operator && next == b':' && next2 == b':';

        if spec.macros && next == b'!' && next2 != b'=' && next_pos == end {
            self.emit(start, end + 1, TokenKind::Macro);
            self.pos = end + 1;
            self.prev = Prev::Identifier;
            self.pending = None;
            return true;
        }

        let mut prev = Prev::Identifier;
        let reserved = spec.word_kind(word);
        let kind = if self.prev == Prev::Dot && word != b"await" {
            if called {
                TokenKind::Function
            } else {
                TokenKind::Field
            }
        } else if let Some(kind) = reserved {
            prev = match kind {
                TokenKind::Constant => Prev::Value,
                TokenKind::Keyword | TokenKind::ControlKeyword => {
                    if matches!(word, b"this" | b"super" | b"self" | b"Self" | b"cls") {
                        Prev::Value
                    } else {
                        Prev::Keyword
                    }
                }
                _ => Prev::Identifier,
            };
            kind
        } else if let Some(pending) = self.pending {
            // `const MAX: u32` defines a constant, `let Some(x)` names a
            // type; a definition keyword only tells us the rest.
            if pending == TokenKind::VariableDefinition && is_screaming(word) {
                prev = Prev::Value;
                TokenKind::Constant
            } else if pending == TokenKind::VariableDefinition && is_capitalized(word) {
                TokenKind::Type
            } else {
                pending
            }
        } else if scoped {
            if is_capitalized(word) {
                TokenKind::Type
            } else {
                TokenKind::Namespace
            }
        } else if called {
            if is_screaming(word) || !is_capitalized(word) {
                TokenKind::Function
            } else {
                TokenKind::Type
            }
        } else if is_screaming(word) {
            prev = Prev::Value;
            TokenKind::Constant
        } else if is_capitalized(word) {
            TokenKind::Type
        } else if spec.colon_fields
            && next == b':'
            && next2 != b':'
            && !self.saw_question
            && !matches!(self.prev_keyword, b"case" | b"default")
            && !(spec.preprocessor && self.prev == Prev::Start)
        {
            TokenKind::Field
        } else if spec.underscore_t_types && word.ends_with(b"_t") {
            TokenKind::Type
        } else if self.prev == Prev::Scope {
            // `Type::name` is an associated function (or constant, caught
            // above); `module::name` is whatever the language uses paths
            // for most.
            if self.scope_after_type {
                TokenKind::Function
            } else {
                spec.scope_member
            }
        } else {
            TokenKind::Identifier
        };

        self.emit(start, end, kind);
        self.pos = end;
        if matches!(
            reserved,
            Some(TokenKind::Keyword | TokenKind::ControlKeyword)
        ) && kind == reserved.unwrap()
        {
            self.prev_keyword = &line[start..end];
            // A definition keyword sets what the next identifier is; other
            // keywords (`mut`, `pub`, `async`) leave a pending kind alone.
            if let Some((_, def)) = spec.definitions.iter().find(|(k, _)| k.as_bytes() == word) {
                self.pending = Some(*def);
            }
        } else {
            self.pending = None;
        }
        self.prev = prev;
        true
    }

    fn operator_token(&mut self, start: usize) -> bool {
        const THREE: &[&str] = &[
            "<<=", ">>=", "...", "===", "!==", "**=", "..=", ">>>", "<=>", "->*",
        ];
        const TWO: &[&str] = &[
            "->", "=>", "::", "==", "!=", "<=", ">=", "&&", "||", "+=", "-=", "*=", "/=", "%=",
            "&=", "|=", "^=", "<<", ">>", "..", "++", "--", "**", "?.", "??", "//", ".*", "|>",
            ":=",
        ];
        let rest = self.rest();
        let len = if THREE.iter().any(|op| rest.starts_with(op.as_bytes())) {
            3
        } else if TWO.iter().any(|op| rest.starts_with(op.as_bytes())) {
            2
        } else if rest[0] < 0x80 {
            1
        } else {
            // A stray non-ASCII byte sequence: take the whole character.
            let mut i = 1;
            while i < rest.len() && (rest[i] & 0xC0) == 0x80 {
                i += 1;
            }
            i
        };
        let op = &rest[..len];
        let (kind, prev) = match op {
            b"(" | b"[" | b"{" | b"," | b";" => (TokenKind::Punctuation, Prev::Operator),
            b")" | b"]" | b"}" => (TokenKind::Punctuation, Prev::Value),
            b"." | b"?." => (TokenKind::Punctuation, Prev::Dot),
            b"->" if self.spec.arrow_member => (TokenKind::Punctuation, Prev::Dot),
            b"::" => (TokenKind::Punctuation, Prev::Scope),
            _ if op[0] >= 0x80 => (TokenKind::Invalid, Prev::Operator),
            _ => (TokenKind::Operator, Prev::Operator),
        };
        if op == b"?" {
            self.saw_question = true;
        }
        if op == b"::" {
            self.scope_after_type = matches!(
                self.out.last().map(|t| t.kind),
                Some(TokenKind::Type | TokenKind::PrimitiveType)
            ) && self.out.last().is_some_and(|t| t.range.end == start);
        }
        self.emit(start, start + len, kind);
        self.pos = start + len;
        self.prev = prev;
        // Generic parameters and address spaces don't interrupt a
        // definition: `impl<T> Foo`, `var<private> name`.
        match op {
            b"<" if self.pending.is_some() => self.stashed = self.pending.take(),
            b">" if self.stashed.is_some() => self.pending = self.stashed.take(),
            _ => {
                self.pending = None;
                self.stashed = None;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{annotate, check};
    use super::super::{Language, lex_text};
    use super::*;

    #[test]
    fn rust() {
        check(
            Language::Rust,
            "\
use std::collections::HashMap;
kkk mmmppmmmmmmmmmmmpptttttttp
/// Docs for `Foo`.
CCCCCCCCCCCCCCCCCCC
#[derive(Debug, Clone)]
aaaaaaaaaaaaaaaaaaaaaaa
pub struct Foo<'a, T: Copy> {
kkk kkkkkk DDDollp to tttto p
    name: &'a str,
    ddddo oll TTTp
}
p
impl<'a> Foo<'a> {
kkkkollo tttollo p
    pub fn new(name: &'a str) -> Self {
    kkk kk FFFpddddo oll TTTp oo kkkk p
        let mut x = self.name.len() + 0x1f_u32 as usize + 1.5e3;
        kkk kkk v o kkkkpddddpfffpp o nnnnnnnn kk TTTTT o nnnnnp
        Foo { name }
        ttt p iiii p
    }
    p
    fn go(&self) -> Option<u8> { self.name.chars().next().map(|c| c as u8) }
    kk FFpokkkkp oo ttttttoTTo p kkkkpddddpfffffpppffffpppfffpoio i kk TTp p
}
p
fn main() {
kk FFFFpp p
    println!(\"{} {:?}\", r#\"raw \"str\"\"#, b'\\n');
    MMMMMMMMpsssssssssp ssssssssssssssp hheehpp
    'outer: loop { break 'outer; }
    LLLLLLo KKKK p KKKKK LLLLLLp p
    let s = \"multi
    kkk v o ssssss
line\"; /* block /* nested */ still */ x
sssssp cccccccccccccccccccccccccccccc i
    let c = 'x'; let q = Vec::<u32>::new(); MAX_LEN
    kkk v o hhhp kkk v o tttppoTTToppfffppp NNNNNNN
}
p

",
        );
    }

    #[test]
    fn c() {
        check(
            Language::C,
            "\
#include <stdio.h>
PPPPPPPP sssssssss
#define MAX(a, b) ((a) > (b) ? (a) : (b)) \\
PPPPPPP fffpip ip ppip o pip o pip o pipp o
    + 1
    o n
static const uint32_t table[] = { 0x10ULL, 1.5f, 'a', '\\0' };
kkkkkk kkkkk TTTTTTTT iiiiipp o p nnnnnnnp nnnnp hhhp heeh pp
struct point *p = s->x.y ? foo(1) : NULL; // done
kkkkkk ttttt oi o ippdpd o fffpnp o NNNNp ccccccc
label:
iiiiio
    case FOO: default: return sizeof(int);
    KKKK NNNo KKKKKKKo KKKKKK kkkkkkpTTTpp
/** doc */ int f(void);
CCCCCCCCCC TTT fpTTTTpp

",
        );
    }

    #[test]
    fn cpp() {
        check(
            Language::Cpp,
            "\
namespace foo { class Bar : public Baz<int> {
kkkkkkkkk mmm p kkkkk ttt o kkkkkk tttoTTTo p
public:
kkkkkko
    std::vector<std::string> items = std::move(other);
    mmmppttttttommmpptttttto iiiii o mmmppffffpiiiiipp
    auto x = static_cast<Foo*>(nullptr)->get();
    kkkk i o kkkkkkkkkkkotttoopNNNNNNNpppfffppp
}; }
pp p

",
        );
    }

    #[test]
    fn javascript() {
        check(
            Language::JavaScript,
            "\
import { foo } from './foo.js';
kkkkkk p iii p kkkk ssssssssssp
const re = /ab[/]c/gi, x = a / b / c;
kkkkk vv o rrrrrrrrrrp i o i o i o ip
let obj = { key: 1, [k]: `tpl ${x + `in${y}`} end`, 'q': null };
kkk vvv o p dddo np pipo sssssppi o sssppipspsssssp ssso NNNN pp
class Foo extends Bar { @dec method() { return this.x?.y ?? super.z(); } }
kkkkk DDD kkkkkkk ttt p aaaa ffffffpp p KKKKKK kkkkpdppd oo kkkkkpfppp p p
x = cond ? a : b; // comment
i o iiii o i o ip cccccccccc
/** doc */ function f($a, b) { return $a.b(c); }
CCCCCCCCCC kkkkkkkk Fpiip ip p KKKKKK iipfpipp p

",
        );
    }

    #[test]
    fn typescript() {
        check(
            Language::TypeScript,
            "\
export interface Props<T> { name: string; count?: number }
kkkkkk kkkkkkkkk DDDDDoto p ddddo TTTTTTp iiiiioo TTTTTT p
type Id = string | number; enum Color { Red = 1 }
kkkk DD o TTTTTT o TTTTTTp kkkk DDDDD p ttt o n p

",
        );
    }

    #[test]
    fn wgsl() {
        check(
            Language::Wgsl,
            "\
@vertex fn vs_main(@builtin(vertex_index) i: u32) -> VertexOut {
aaaaaaa kk FFFFFFFpaaaaaaaapiiiiiiiiiiiip do TTTp oo ttttttttt p
    var<private> pos = vec4<f32>(1.0, 0.5, 0u, 1i);
    kkkoiiiiiiio vvv o TTTToTTTopnnnp nnnp nnp nnpp
    let s = textureSample(tex, samp, uv).rgb; // done
    kkk v o fffffffffffffpiiip iiiip iippdddp ccccccc

",
        );
    }

    #[test]
    fn python() {
        check(
            Language::Python,
            "\
import os.path as osp  # comment
kkkkkk iipdddd kk vvv  ccccccccc
@dataclass(frozen=True)
aaaaaaaaaapiiiiiioNNNNp
class Foo(Base):
kkkkk DDDpttttpo
    def method(self, x: int = 0x10) -> None:
    kkk FFFFFFpkkkkp io TTT o nnnnp oo NNNNo
        \"\"\"Doc
        ssssss
        string\"\"\"
sssssssssssssssss
        return f\"{self.x!r}\" + rb'\\d+' + 'it\\'s' + self.go()
        KKKKKK sssssssssssss o sssssss o ssseess o kkkkpffpp
    for i in range(10): print(i)
    KKK v kk fffffpnnpo fffffpip

",
        );
    }

    /// Print the annotated lexing of a file, for eyeballing the rules on
    /// real code: `SYNTAX_FILE=src/foo.rs cargo test -p ninjaedit-core
    /// annotate_file -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn annotate_file() {
        let Ok(path) = std::env::var("SYNTAX_FILE") else {
            return;
        };
        let text = std::fs::read_to_string(&path).unwrap();
        let language = Language::from_path(std::path::Path::new(&path)).expect("known language");
        eprintln!("{}", annotate(language, &text));
    }

    #[test]
    fn states_carry_across_lines() {
        let lines = lex_text(Language::Rust, "/* a\nb\nc */ d\n\"x\ny\" z\n");
        assert_eq!(
            lines[1],
            vec![Token {
                range: 0..1,
                kind: TokenKind::Comment
            }]
        );
        assert_eq!(
            lines[2][0],
            Token {
                range: 0..4,
                kind: TokenKind::Comment
            }
        );
        assert_eq!(lines[2][1].kind, TokenKind::Identifier);
        assert_eq!(
            lines[3],
            vec![Token {
                range: 0..2,
                kind: TokenKind::String
            }]
        );
        assert_eq!(
            lines[4][0],
            Token {
                range: 0..2,
                kind: TokenKind::String
            }
        );
        assert_eq!(lines[4][1].kind, TokenKind::Identifier);

        // A C string doesn't continue without a trailing backslash; with
        // one it does.
        let lines = lex_text(Language::C, "\"open\nx\n\"cont\\\ny\" z\n");
        assert_eq!(lines[1][0].kind, TokenKind::Identifier);
        assert_eq!(lines[3][0].kind, TokenKind::String);
        assert_eq!(lines[3][1].kind, TokenKind::Identifier);
    }

    #[test]
    fn preprocessor_continuation() {
        let lines = lex_text(Language::C, "#define X \\\n  #y\n#if 1\n");
        assert_eq!(lines[0][0].kind, TokenKind::Preprocessor);
        // The continuation line's `#` is not a directive.
        assert_eq!(lines[1][0].kind, TokenKind::Operator);
        assert_eq!(lines[2][0].kind, TokenKind::Preprocessor);
    }

    #[test]
    fn template_literals_nest_across_lines() {
        let lines = lex_text(Language::JavaScript, "`a ${\n  b + `c ${d}`\n} e` f\n");
        assert_eq!(lines[0][0].kind, TokenKind::String);
        assert_eq!(lines[1][0].kind, TokenKind::Identifier);
        assert_eq!(lines[1][2].kind, TokenKind::String);
        assert_eq!(lines[2][0].kind, TokenKind::Punctuation);
        assert_eq!(lines[2][1].kind, TokenKind::String);
        assert_eq!(lines[2][2].kind, TokenKind::Identifier);
    }

    #[test]
    fn tokens_are_ordered_and_disjoint() {
        for language in [
            Language::Rust,
            Language::C,
            Language::Cpp,
            Language::JavaScript,
            Language::TypeScript,
            Language::Wgsl,
            Language::Python,
        ] {
            let text = "a \"b\\\"c\" 'd' /* e */ f(g) # h @i #[j] `k ${l}` /m/ 0.5e3 x::y z.w ->\n";
            for tokens in lex_text(language, text) {
                let mut end = 0;
                for token in tokens {
                    assert!(token.range.start >= end, "{language:?}: {token:?}");
                    assert!(
                        token.range.end > token.range.start,
                        "{language:?}: {token:?}"
                    );
                    end = token.range.end;
                }
                assert!(end <= text.len());
            }
        }
    }

    #[test]
    fn no_panics_on_odd_input() {
        let inputs: &[&[u8]] = &[
            b"",
            b"\\",
            b"\"",
            b"'",
            b"'\\",
            b"/*",
            b"*/",
            b"#[",
            b"@",
            b"`${",
            b"r#",
            b"0x",
            b"1.",
            b".",
            b"\xff\xfe",
            b"'\xe2\x9c\x93'",
            b"\"\\u{",
            b"/[",
        ];
        for language in [
            Language::Rust,
            Language::C,
            Language::Cpp,
            Language::JavaScript,
            Language::TypeScript,
            Language::Wgsl,
            Language::Python,
        ] {
            let lexer = language.lexer();
            for input in inputs {
                let mut out = Vec::new();
                lexer.lex_line(LexState::default(), input, &mut out);
            }
        }
    }
}
