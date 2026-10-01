//! A table-driven lexer for languages with C-like lexical structure:
//! identifiers, keywords, brackets, C or shell comments, quoted strings,
//! and numbers. Rust, C, C++, JavaScript, TypeScript, WGSL, Python, C#,
//! Java, Kotlin, Go, and Ruby are all instances of it, differing in their
//! tables and in a handful of feature flags (nested comments, raw strings,
//! template literals, ...).
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
//!   types. In C# and Go, where methods and exported functions are
//!   capitalized too, `Name(` is a function.
//! * A soft keyword (`var`, `data`, `record`, ...) is a keyword only
//!   where a name couldn't be: before another name or an opening bracket,
//!   and not where a definition expects a name.
//! * A dotted path after `import` or `namespace` is all namespaces, up to
//!   a capitalized name in languages where that is the class imported.
//! * A Kotlin extension's receiver (`fun Foo.bar`) is a type, and so is a
//!   Go method's (`func (f *Foo) Bar`), the name after either being the
//!   definition; so is the name after Ruby's `def self.`. Go's `a, b :=`
//!   defines its names, and so does a Ruby block's `|a, b|`.

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
    /// `[Name(...)]` at the start of a line, taken whole, as in C#.
    Bracket,
}

/// What a backtick starts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Backticks {
    None,
    /// A JavaScript template literal, with `${ }` expressions.
    Template,
    /// A raw string that may span lines, as in Go.
    RawString,
    /// A quoted identifier, such as Kotlin's ``fun `does a thing`()``.
    Identifier,
}

/// Which family of strings with embedded expressions, raw strings, and
/// verbatim strings a language has, beyond the ordinary ones. See
/// [`Context::RichString`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum RichStrings {
    None,
    /// C#: `@"verbatim"`, `"""raw"""`, and `$"{interpolated}"`, which
    /// combine as `$@"..."` and `$$"""...{{x}}..."""`.
    CSharp,
    /// Kotlin: every string has `$name` and `${expr}` templates, and
    /// `"""raw"""` strings have no escapes.
    Kotlin,
    /// Ruby: strings that span lines, with `#{expr}` in `"..."` but not
    /// `'...'`, `%w[...]` and other `%` literals, and heredocs. See
    /// [`Context::Delimited`] and [`Context::Heredoc`].
    Ruby,
}

/// How a language writes the receiver of a method definition.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Receivers {
    None,
    /// In parentheses before the name, after a space: Go's
    /// `func (s *Server) Run()`. A function literal, `func(`, has no
    /// space.
    Parenthesized,
    /// A type and a dot before the name: Kotlin's `fun Foo.bar()` and
    /// `val List<T>.second`.
    Dotted,
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
    /// Keywords that are also common names, and are only taken as
    /// keywords where a name couldn't be; see the module documentation.
    soft_keywords: &'static [&'static str],
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
    backticks: Backticks,
    rich_strings: RichStrings,
    regex_literals: bool,
    preprocessor: bool,
    attributes: Attributes,
    /// `name!` is a macro invocation.
    macros: bool,
    /// `::` separates namespaces.
    scope_operator: bool,
    /// What a lowercase name after `::` is when it isn't called: a
    /// namespace in Rust (`use std::fmt`), a type in C++ (`std::string`),
    /// a method in a Java method reference (`String::valueOf`).
    scope_member: TokenKind,
    /// Whether a lowercase name before `::` is a namespace. In Java and
    /// Kotlin it is the object of a method reference (`list::add`).
    scope_namespaces: bool,
    /// `$` may appear in identifiers.
    dollar_identifiers: bool,
    /// `name:` marks a field.
    colon_fields: bool,
    /// Names ending in `_t` are types.
    underscore_t_types: bool,
    /// `->` is a member access (C), rather than a return type arrow.
    arrow_member: bool,
    /// Capitalized names are as often functions, properties, and
    /// variables as types (C# methods, Go exports): `Name(` is a function
    /// call, and `var Name` defines a variable rather than naming a
    /// pattern's type as Rust's `let Some(x)` does.
    capitalized_values: bool,
    /// In a dotted path after a keyword that defines a namespace, a
    /// capitalized name is a type, the class being imported, as in Java's
    /// `import java.util.List`. Otherwise the whole path is namespaces,
    /// as in C#'s `using System.Text`.
    path_types: bool,
    receivers: Receivers,
    /// `name :=` and `a, b :=` define variables, as in Go.
    short_declarations: bool,
    /// `name@` is a label and `return@name` refers to one, as in Kotlin.
    at_labels: bool,
    /// `:name` is a symbol, as in Ruby.
    symbols: bool,
    /// `@name`, `@@name`, and `$name` are variables, as in Ruby.
    sigils: bool,
    /// A method name may end in `?` or `!`, and a setter's definition in
    /// `=`, as in Ruby.
    method_suffixes: bool,
    /// `=begin` and `=end` lines enclose a block comment, as in Ruby.
    begin_end_comments: bool,
    /// `|a, b|` after `do` or `{` defines a block's parameters, as in
    /// Ruby.
    block_parameters: bool,
    /// The keyword tables merged into one map, built on first use, with
    /// whether each word is a soft keyword.
    words: OnceLock<HashMap<&'static [u8], (TokenKind, bool)>>,
}

impl Spec {
    /// The kind of a reserved word, if `word` is one, and whether it is a
    /// soft keyword.
    fn word_kind(&self, word: &[u8]) -> Option<(TokenKind, bool)> {
        let words = self.words.get_or_init(|| {
            let mut map = HashMap::new();
            for (list, kind, soft) in [
                (self.primitives, TokenKind::PrimitiveType, false),
                (self.keywords, TokenKind::Keyword, false),
                (self.control, TokenKind::ControlKeyword, false),
                (self.constants, TokenKind::Constant, false),
                (self.soft_keywords, TokenKind::Keyword, true),
            ] {
                for word in list {
                    map.insert(word.as_bytes(), (kind, soft));
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
    soft_keywords: &[],
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
    backticks: Backticks::None,
    rich_strings: RichStrings::None,
    regex_literals: false,
    preprocessor: false,
    attributes: Attributes::Hash,
    macros: true,
    scope_operator: true,
    scope_member: TokenKind::Namespace,
    scope_namespaces: true,
    dollar_identifiers: false,
    colon_fields: true,
    underscore_t_types: false,
    arrow_member: false,
    capitalized_values: false,
    path_types: false,
    receivers: Receivers::None,
    short_declarations: false,
    at_labels: false,
    symbols: false,
    sigils: false,
    method_suffixes: false,
    begin_end_comments: false,
    block_parameters: false,
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
    soft_keywords: &[],
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
    backticks: Backticks::None,
    rich_strings: RichStrings::None,
    regex_literals: false,
    preprocessor: true,
    attributes: Attributes::None,
    macros: false,
    scope_operator: false,
    scope_member: TokenKind::Namespace,
    scope_namespaces: true,
    dollar_identifiers: false,
    colon_fields: true,
    underscore_t_types: true,
    arrow_member: true,
    capitalized_values: false,
    path_types: false,
    receivers: Receivers::None,
    short_declarations: false,
    at_labels: false,
    symbols: false,
    sigils: false,
    method_suffixes: false,
    begin_end_comments: false,
    block_parameters: false,
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
    soft_keywords: &[],
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
    backticks: Backticks::None,
    rich_strings: RichStrings::None,
    regex_literals: false,
    preprocessor: true,
    attributes: Attributes::None,
    macros: false,
    scope_operator: true,
    scope_member: TokenKind::Type,
    scope_namespaces: true,
    dollar_identifiers: false,
    colon_fields: true,
    underscore_t_types: true,
    arrow_member: true,
    capitalized_values: false,
    path_types: false,
    receivers: Receivers::None,
    short_declarations: false,
    at_labels: false,
    symbols: false,
    sigils: false,
    method_suffixes: false,
    begin_end_comments: false,
    block_parameters: false,
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
    soft_keywords: &[],
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
    backticks: Backticks::Template,
    rich_strings: RichStrings::None,
    regex_literals: true,
    preprocessor: false,
    attributes: Attributes::At,
    macros: false,
    scope_operator: false,
    scope_member: TokenKind::Namespace,
    scope_namespaces: true,
    dollar_identifiers: true,
    colon_fields: true,
    underscore_t_types: false,
    arrow_member: false,
    capitalized_values: false,
    path_types: false,
    receivers: Receivers::None,
    short_declarations: false,
    at_labels: false,
    symbols: false,
    sigils: false,
    method_suffixes: false,
    begin_end_comments: false,
    block_parameters: false,
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
    soft_keywords: &[],
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
    backticks: Backticks::Template,
    rich_strings: RichStrings::None,
    regex_literals: true,
    preprocessor: false,
    attributes: Attributes::At,
    macros: false,
    scope_operator: false,
    scope_member: TokenKind::Namespace,
    scope_namespaces: true,
    dollar_identifiers: true,
    colon_fields: true,
    underscore_t_types: false,
    arrow_member: false,
    capitalized_values: false,
    path_types: false,
    receivers: Receivers::None,
    short_declarations: false,
    at_labels: false,
    symbols: false,
    sigils: false,
    method_suffixes: false,
    begin_end_comments: false,
    block_parameters: false,
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
    soft_keywords: &[],
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
    backticks: Backticks::None,
    rich_strings: RichStrings::None,
    regex_literals: false,
    preprocessor: false,
    attributes: Attributes::At,
    macros: false,
    scope_operator: false,
    scope_member: TokenKind::Namespace,
    scope_namespaces: true,
    dollar_identifiers: false,
    colon_fields: true,
    underscore_t_types: false,
    arrow_member: false,
    capitalized_values: false,
    path_types: false,
    receivers: Receivers::None,
    short_declarations: false,
    at_labels: false,
    symbols: false,
    sigils: false,
    method_suffixes: false,
    begin_end_comments: false,
    block_parameters: false,
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
    soft_keywords: &[],
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
    backticks: Backticks::None,
    rich_strings: RichStrings::None,
    regex_literals: false,
    preprocessor: false,
    attributes: Attributes::At,
    macros: false,
    scope_operator: false,
    scope_member: TokenKind::Namespace,
    scope_namespaces: true,
    dollar_identifiers: false,
    colon_fields: false,
    underscore_t_types: false,
    arrow_member: false,
    capitalized_values: false,
    path_types: false,
    receivers: Receivers::None,
    short_declarations: false,
    at_labels: false,
    symbols: false,
    sigils: false,
    method_suffixes: false,
    begin_end_comments: false,
    block_parameters: false,
    words: OnceLock::new(),
};

pub(super) static CSHARP: Spec = Spec {
    keywords: &[
        "abstract",
        "and",
        "as",
        "base",
        "checked",
        "class",
        "const",
        "delegate",
        "event",
        "explicit",
        "extern",
        "fixed",
        "get",
        "implicit",
        "in",
        "init",
        "interface",
        "internal",
        "is",
        "lock",
        "namespace",
        "new",
        "not",
        "operator",
        "or",
        "out",
        "override",
        "params",
        "private",
        "protected",
        "public",
        "readonly",
        "ref",
        "sealed",
        "set",
        "sizeof",
        "stackalloc",
        "static",
        "struct",
        "this",
        "typeof",
        "unchecked",
        "unsafe",
        "using",
        "virtual",
        "volatile",
    ],
    control: &[
        "await", "break", "case", "catch", "continue", "default", "do", "else", "finally", "for",
        "foreach", "goto", "if", "return", "switch", "throw", "try", "when", "while", "yield",
    ],
    primitives: &[
        "bool", "byte", "char", "decimal", "double", "dynamic", "float", "int", "long", "nint",
        "nuint", "object", "sbyte", "short", "string", "uint", "ulong", "ushort", "void",
    ],
    constants: &["true", "false", "null"],
    soft_keywords: &[
        "ascending",
        "async",
        "by",
        "descending",
        "equals",
        "file",
        "from",
        "group",
        "into",
        "join",
        "let",
        "nameof",
        "on",
        "orderby",
        "partial",
        "record",
        "required",
        "scoped",
        "select",
        "var",
        "where",
        "with",
    ],
    definitions: &[
        ("class", TokenKind::TypeDefinition),
        ("struct", TokenKind::TypeDefinition),
        ("interface", TokenKind::TypeDefinition),
        ("enum", TokenKind::TypeDefinition),
        ("record", TokenKind::TypeDefinition),
        ("namespace", TokenKind::Namespace),
        ("using", TokenKind::Namespace),
        ("new", TokenKind::Type),
        ("var", TokenKind::VariableDefinition),
    ],
    line_comment: "//",
    doc_line_comment: &["///"],
    doc_block_comment: &["/**"],
    block_comments: true,
    nested_comments: false,
    single_quote: Quote::Char,
    triple_quotes: false,
    multiline_strings: false,
    string_prefixes: &[],
    raw_strings: false,
    raw_hashes: false,
    backticks: Backticks::None,
    rich_strings: RichStrings::CSharp,
    regex_literals: false,
    preprocessor: true,
    attributes: Attributes::Bracket,
    macros: false,
    scope_operator: true,
    scope_member: TokenKind::Namespace,
    scope_namespaces: true,
    dollar_identifiers: false,
    colon_fields: true,
    underscore_t_types: false,
    arrow_member: true,
    capitalized_values: true,
    path_types: false,
    receivers: Receivers::None,
    short_declarations: false,
    at_labels: false,
    symbols: false,
    sigils: false,
    method_suffixes: false,
    begin_end_comments: false,
    block_parameters: false,
    words: OnceLock::new(),
};

pub(super) static JAVA: Spec = Spec {
    keywords: &[
        "abstract",
        "assert",
        "class",
        "const",
        "enum",
        "extends",
        "final",
        "implements",
        "import",
        "instanceof",
        "interface",
        "native",
        "new",
        "package",
        "private",
        "protected",
        "public",
        "static",
        "strictfp",
        "super",
        "synchronized",
        "this",
        "throws",
        "transient",
        "volatile",
    ],
    control: &[
        "break", "case", "catch", "continue", "default", "do", "else", "finally", "for", "if",
        "return", "switch", "throw", "try", "while", "yield",
    ],
    primitives: &[
        "boolean", "byte", "char", "double", "float", "int", "long", "short", "void",
    ],
    constants: &["true", "false", "null"],
    soft_keywords: &[
        "exports",
        "module",
        "opens",
        "permits",
        "provides",
        "record",
        "requires",
        "sealed",
        "transitive",
        "uses",
        "var",
    ],
    definitions: &[
        ("class", TokenKind::TypeDefinition),
        ("interface", TokenKind::TypeDefinition),
        ("enum", TokenKind::TypeDefinition),
        ("record", TokenKind::TypeDefinition),
        ("package", TokenKind::Namespace),
        ("import", TokenKind::Namespace),
        ("var", TokenKind::VariableDefinition),
    ],
    line_comment: "//",
    doc_line_comment: &[],
    doc_block_comment: &["/**"],
    block_comments: true,
    nested_comments: false,
    single_quote: Quote::Char,
    triple_quotes: true,
    multiline_strings: false,
    string_prefixes: &[],
    raw_strings: false,
    raw_hashes: false,
    backticks: Backticks::None,
    rich_strings: RichStrings::None,
    regex_literals: false,
    preprocessor: false,
    attributes: Attributes::At,
    macros: false,
    scope_operator: true,
    scope_member: TokenKind::Function,
    scope_namespaces: false,
    dollar_identifiers: true,
    // `name:` is a label, or the variable of an enhanced `for`, rather
    // than a field.
    colon_fields: false,
    underscore_t_types: false,
    arrow_member: false,
    capitalized_values: false,
    path_types: true,
    receivers: Receivers::None,
    short_declarations: false,
    at_labels: false,
    symbols: false,
    sigils: false,
    method_suffixes: false,
    begin_end_comments: false,
    block_parameters: false,
    words: OnceLock::new(),
};

pub(super) static KOTLIN: Spec = Spec {
    keywords: &[
        "as",
        "class",
        "constructor",
        "fun",
        "import",
        "in",
        "interface",
        "is",
        "object",
        "package",
        "super",
        "this",
        "typealias",
        "typeof",
        "val",
        "var",
    ],
    control: &[
        "break", "catch", "continue", "do", "else", "finally", "for", "if", "return", "throw",
        "try", "when", "while",
    ],
    primitives: &[],
    constants: &["true", "false", "null"],
    soft_keywords: &[
        "abstract",
        "actual",
        "annotation",
        "by",
        "companion",
        "const",
        "crossinline",
        "data",
        "enum",
        "expect",
        "external",
        "final",
        "get",
        "infix",
        "init",
        "inline",
        "inner",
        "internal",
        "lateinit",
        "noinline",
        "open",
        "operator",
        "out",
        "override",
        "private",
        "protected",
        "public",
        "reified",
        "sealed",
        "set",
        "suspend",
        "tailrec",
        "value",
        "vararg",
        "where",
    ],
    definitions: &[
        ("fun", TokenKind::FunctionDefinition),
        ("class", TokenKind::TypeDefinition),
        ("interface", TokenKind::TypeDefinition),
        ("object", TokenKind::TypeDefinition),
        ("typealias", TokenKind::TypeDefinition),
        ("package", TokenKind::Namespace),
        ("import", TokenKind::Namespace),
        ("val", TokenKind::VariableDefinition),
        ("var", TokenKind::VariableDefinition),
    ],
    line_comment: "//",
    doc_line_comment: &[],
    doc_block_comment: &["/**"],
    block_comments: true,
    nested_comments: true,
    single_quote: Quote::Char,
    triple_quotes: false,
    multiline_strings: false,
    string_prefixes: &[],
    raw_strings: false,
    raw_hashes: false,
    backticks: Backticks::Identifier,
    rich_strings: RichStrings::Kotlin,
    regex_literals: false,
    preprocessor: false,
    attributes: Attributes::At,
    macros: false,
    scope_operator: true,
    scope_member: TokenKind::Function,
    scope_namespaces: false,
    dollar_identifiers: false,
    colon_fields: true,
    underscore_t_types: false,
    arrow_member: false,
    capitalized_values: false,
    path_types: true,
    receivers: Receivers::Dotted,
    short_declarations: false,
    at_labels: true,
    symbols: false,
    sigils: false,
    method_suffixes: false,
    begin_end_comments: false,
    block_parameters: false,
    words: OnceLock::new(),
};

pub(super) static GO: Spec = Spec {
    keywords: &[
        "chan",
        "const",
        "func",
        "import",
        "interface",
        "map",
        "package",
        "struct",
        "type",
        "var",
    ],
    control: &[
        "break",
        "case",
        "continue",
        "default",
        "defer",
        "else",
        "fallthrough",
        "for",
        "go",
        "goto",
        "if",
        "range",
        "return",
        "select",
        "switch",
    ],
    primitives: &[
        "any",
        "bool",
        "byte",
        "comparable",
        "complex64",
        "complex128",
        "error",
        "float32",
        "float64",
        "int",
        "int8",
        "int16",
        "int32",
        "int64",
        "rune",
        "string",
        "uint",
        "uint8",
        "uint16",
        "uint32",
        "uint64",
        "uintptr",
    ],
    constants: &["true", "false", "nil", "iota"],
    soft_keywords: &[],
    definitions: &[
        ("func", TokenKind::FunctionDefinition),
        ("type", TokenKind::TypeDefinition),
        ("var", TokenKind::VariableDefinition),
        ("const", TokenKind::VariableDefinition),
        ("package", TokenKind::Namespace),
    ],
    line_comment: "//",
    doc_line_comment: &[],
    doc_block_comment: &[],
    block_comments: true,
    nested_comments: false,
    single_quote: Quote::Char,
    triple_quotes: false,
    multiline_strings: false,
    string_prefixes: &[],
    raw_strings: false,
    raw_hashes: false,
    backticks: Backticks::RawString,
    rich_strings: RichStrings::None,
    regex_literals: false,
    preprocessor: false,
    attributes: Attributes::None,
    macros: false,
    scope_operator: false,
    scope_member: TokenKind::Namespace,
    scope_namespaces: false,
    dollar_identifiers: false,
    colon_fields: true,
    underscore_t_types: false,
    arrow_member: false,
    capitalized_values: true,
    path_types: false,
    receivers: Receivers::Parenthesized,
    short_declarations: true,
    at_labels: false,
    symbols: false,
    sigils: false,
    method_suffixes: false,
    begin_end_comments: false,
    block_parameters: false,
    words: OnceLock::new(),
};

pub(super) static RUBY: Spec = Spec {
    keywords: &[
        "alias",
        "and",
        "attr_accessor",
        "attr_reader",
        "attr_writer",
        "begin",
        "BEGIN",
        "class",
        "def",
        "defined?",
        "do",
        "end",
        "END",
        "extend",
        "include",
        "module",
        "module_function",
        "not",
        "or",
        "prepend",
        "private",
        "private_constant",
        "protected",
        "public",
        "refine",
        "require",
        "require_relative",
        "self",
        "super",
        "undef",
        "using",
    ],
    control: &[
        "break", "case", "else", "elsif", "ensure", "for", "if", "in", "next", "raise", "redo",
        "rescue", "retry", "return", "then", "unless", "until", "when", "while", "yield",
    ],
    primitives: &[],
    constants: &[
        "true",
        "false",
        "nil",
        "__FILE__",
        "__LINE__",
        "__dir__",
        "__method__",
        "__ENCODING__",
    ],
    soft_keywords: &[],
    definitions: &[
        ("def", TokenKind::FunctionDefinition),
        ("alias", TokenKind::FunctionDefinition),
        ("class", TokenKind::TypeDefinition),
        ("module", TokenKind::TypeDefinition),
        ("for", TokenKind::VariableDefinition),
    ],
    line_comment: "#",
    doc_line_comment: &[],
    doc_block_comment: &[],
    block_comments: false,
    nested_comments: false,
    single_quote: Quote::String,
    triple_quotes: false,
    multiline_strings: true,
    string_prefixes: &[],
    raw_strings: false,
    raw_hashes: false,
    backticks: Backticks::None,
    rich_strings: RichStrings::Ruby,
    regex_literals: true,
    preprocessor: false,
    attributes: Attributes::None,
    macros: false,
    scope_operator: true,
    scope_member: TokenKind::Function,
    scope_namespaces: false,
    dollar_identifiers: false,
    colon_fields: true,
    underscore_t_types: false,
    arrow_member: false,
    capitalized_values: false,
    path_types: false,
    receivers: Receivers::Dotted,
    short_declarations: false,
    at_labels: false,
    symbols: true,
    sigils: true,
    method_suffixes: true,
    begin_end_comments: true,
    block_parameters: true,
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
    /// Whether the next `.` continues the pending definition rather than
    /// ending it: a dotted `import` path, or a Kotlin receiver type.
    keep_pending: bool,
    /// How many parentheses deep a Go method receiver is, while in one.
    receiver_depth: u8,
    /// Whether the current token is directly inside a C# interpolation
    /// hole, where a `:` starts a format string.
    format_hole: bool,
    /// The first Ruby heredoc opened on this line, whose body starts on
    /// the next.
    heredoc: Option<Context>,
    /// Whether this is inside a Ruby block's `|parameters|`.
    block_params: bool,
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
            keep_pending: false,
            receiver_depth: 0,
            format_hole: false,
            heredoc: None,
            block_params: false,
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
                Some(Context::RichString {
                    quotes,
                    escapes,
                    dollars,
                }) => lx.continue_rich_string(&mut state, quotes, escapes, dollars),
                Some(Context::Delimited {
                    close,
                    depth,
                    interpolates,
                    regex,
                }) => lx.continue_delimited(&mut state, close, depth, interpolates, regex),
                Some(Context::Heredoc { tag, interpolates }) => {
                    lx.continue_heredoc(&mut state, tag, interpolates)
                }
                Some(Context::TemplateExpression { .. })
                | Some(Context::Value { .. })
                | Some(Context::Fence { .. })
                | Some(Context::Paragraph)
                | Some(Context::Bracket { .. })
                | Some(Context::Conflict { .. })
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
        if let Some(heredoc) = lx.heredoc {
            state.push(heredoc);
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

/// Whether `line` starts with `word` followed by a space or nothing, as
/// Ruby's `=begin` and `=end` lines do.
fn is_line_keyword(line: &[u8], word: &[u8]) -> bool {
    line.starts_with(word) && line.get(word.len()).is_none_or(|b| b.is_ascii_whitespace())
}

/// A hash of a Ruby heredoc's terminator, which is all of it a
/// [`Context::Heredoc`] has room for: FNV-1a, folded to 24 bits.
fn tag_hash(tag: &[u8]) -> [u8; 3] {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in tag {
        hash = (hash ^ b as u32).wrapping_mul(0x0100_0193);
    }
    let folded = (hash >> 24) ^ (hash & 0x00ff_ffff);
    [folded as u8, (folded >> 8) as u8, (folded >> 16) as u8]
}

/// The bracket that closes `open`, or `open` itself if it isn't one.
fn closing_bracket(open: u8) -> u8 {
    match open {
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        b'<' => b'>',
        _ => open,
    }
}

/// The bracket that `close` closes, or `close` itself if it isn't one.
fn opening_bracket(close: u8) -> u8 {
    match close {
        b')' => b'(',
        b']' => b'[',
        b'}' => b'{',
        b'>' => b'<',
        _ => close,
    }
}

/// Whether a token of `kind` is a definition of a new name.
fn is_definition(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::FunctionDefinition | TokenKind::TypeDefinition | TokenKind::VariableDefinition
    )
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
        if self.spec.begin_end_comments {
            // The whole line is comment, through the `=end` line.
            if is_line_keyword(self.line, b"=end") {
                state.pop();
            }
            self.emit(self.pos, self.line.len(), TokenKind::Comment);
            self.pos = self.line.len();
            return false;
        }
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
            if self.spec.backticks == Backticks::Template
                && quote == b'`'
                && self.line[i..].starts_with(b"${")
            {
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
        // continues any string that has escapes. Backtick strings
        // (template literals and Go raw strings) always continue.
        let continued = triple
            || self.spec.multiline_strings
            || quote == b'`'
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

    /// Lex the rest of a C# or Kotlin string; see
    /// [`Context::RichString`]. Returns whether the line goes on after it.
    fn continue_rich_string(
        &mut self,
        state: &mut LexState,
        quotes: u8,
        escapes: bool,
        dollars: u8,
    ) -> bool {
        let line = self.line;
        let mut segment = self.pos;
        let mut i = self.pos;
        while i < line.len() {
            let b = line[i];
            if escapes && b == b'\\' {
                let end = self.escape_end(i);
                self.emit(segment, i, TokenKind::String);
                self.emit(i, end, TokenKind::StringEscape);
                i = end;
                segment = end;
                continue;
            }
            if b == b'"' {
                let run = self.run_of(i, b'"');
                if quotes == 1 && !escapes && run >= 2 {
                    // A doubled quote in a verbatim string.
                    self.emit(segment, i, TokenKind::String);
                    self.emit(i, i + 2, TokenKind::StringEscape);
                    i += 2;
                    segment = i;
                    continue;
                }
                if run >= quotes as usize {
                    let end = if quotes == 1 { i + 1 } else { i + run };
                    self.emit(segment, end, TokenKind::String);
                    self.pos = end;
                    state.pop();
                    self.prev = Prev::Value;
                    self.pending = None;
                    return true;
                }
                i += run;
                continue;
            }
            if dollars > 0 {
                match self.spec.rich_strings {
                    RichStrings::CSharp if b == b'{' || b == b'}' => {
                        let run = self.run_of(i, b);
                        if dollars == 1 && run >= 2 {
                            // `{{` and `}}` stand for a brace.
                            self.emit(segment, i, TokenKind::String);
                            self.emit(i, i + 2, TokenKind::StringEscape);
                            i += 2;
                            segment = i;
                            continue;
                        }
                        if b == b'{' && run >= dollars as usize {
                            // With `$$`, `{{{x}}}` is a brace and then an
                            // expression in two.
                            let open = i + run - dollars as usize;
                            self.emit(segment, open, TokenKind::String);
                            return self.open_embedded(state, open, open + dollars as usize);
                        }
                        i += run;
                        continue;
                    }
                    RichStrings::Kotlin if b == b'$' => {
                        let next = self.at(i + 1);
                        if next == b'{' {
                            self.emit(segment, i, TokenKind::String);
                            return self.open_embedded(state, i, i + 2);
                        }
                        if is_identifier_start(next, false) {
                            let mut end = i + 2;
                            while end < line.len() && is_identifier_char(line[end], false) {
                                end += 1;
                            }
                            self.emit(segment, i, TokenKind::String);
                            self.emit(i, end, TokenKind::Variable);
                            i = end;
                            segment = end;
                            continue;
                        }
                    }
                    _ => {}
                }
            }
            i += 1;
        }
        self.emit(segment, line.len(), TokenKind::String);
        self.pos = line.len();
        // Only raw and verbatim strings continue on the next line.
        if quotes == 1 && escapes {
            state.pop();
        }
        false
    }

    /// Enter an expression embedded in a string, whose opening delimiter
    /// spans `start..end`. Returns whether the line goes on after it.
    fn open_embedded(&mut self, state: &mut LexState, start: usize, end: usize) -> bool {
        self.emit(start, end, TokenKind::Punctuation);
        self.pos = end;
        state.push(Context::TemplateExpression { braces: 0 });
        self.prev = Prev::Start;
        self.pending = None;
        true
    }

    /// Lex the rest of the string that an embedded expression just closed
    /// in, which is now the innermost context. Returns whether the line
    /// goes on after it.
    fn continue_embedding(&mut self, state: &mut LexState) -> bool {
        match state.top() {
            Some(Context::RichString {
                quotes,
                escapes,
                dollars,
            }) => self.continue_rich_string(state, quotes, escapes, dollars),
            Some(Context::Delimited {
                close,
                depth,
                interpolates,
                regex,
            }) => self.continue_delimited(state, close, depth, interpolates, regex),
            Some(Context::Heredoc { tag, interpolates }) => {
                self.continue_heredoc(state, tag, interpolates)
            }
            _ => self.continue_template(state),
        }
    }

    /// The length of the run of `b` starting at `i`.
    fn run_of(&self, i: usize, b: u8) -> usize {
        self.line[i..].iter().take_while(|&&c| c == b).count()
    }

    /// Lex the rest of a Ruby literal; see [`Context::Delimited`]. Returns
    /// whether the line goes on after it.
    fn continue_delimited(
        &mut self,
        state: &mut LexState,
        close: u8,
        mut depth: u8,
        interpolates: bool,
        regex: bool,
    ) -> bool {
        let line = self.line;
        let open = opening_bracket(close);
        let kind = if regex {
            TokenKind::Regex
        } else {
            TokenKind::String
        };
        let mut segment = self.pos;
        let mut i = self.pos;
        while i < line.len() {
            let b = line[i];
            if b == b'\\' {
                // Without interpolation, only the delimiters and the
                // backslash itself can be escaped.
                let next = self.at(i + 1);
                let end = if interpolates {
                    self.escape_end(i)
                } else if next == b'\\' || next == close || next == open {
                    i + 2
                } else {
                    i += 1;
                    continue;
                };
                self.emit(segment, i, kind);
                self.emit(i, end, TokenKind::StringEscape);
                i = end;
                segment = end;
                continue;
            }
            if b == close && (depth == 0 || open == close) {
                let mut end = i + 1;
                if regex {
                    while end < line.len() && line[end].is_ascii_alphabetic() {
                        end += 1;
                    }
                }
                self.emit(segment, end, kind);
                self.pos = end;
                state.pop();
                self.prev = Prev::Value;
                self.pending = None;
                return true;
            }
            if b == close {
                depth -= 1;
            } else if b == open && open != close {
                depth = depth.saturating_add(1);
            } else if interpolates && b == b'#' && self.at(i + 1) == b'{' {
                self.emit(segment, i, kind);
                state.replace(Context::Delimited {
                    close,
                    depth,
                    interpolates,
                    regex,
                });
                return self.open_embedded(state, i, i + 2);
            }
            i += 1;
        }
        self.emit(segment, line.len(), kind);
        self.pos = line.len();
        state.replace(Context::Delimited {
            close,
            depth,
            interpolates,
            regex,
        });
        false
    }

    /// Lex a line of a Ruby heredoc's body, or its terminator; see
    /// [`Context::Heredoc`]. Returns whether the line goes on after it.
    fn continue_heredoc(&mut self, state: &mut LexState, tag: [u8; 3], interpolates: bool) -> bool {
        let line = self.line;
        let trimmed = line.trim_ascii();
        if self.pos == 0 && !trimmed.is_empty() && tag_hash(trimmed) == tag {
            self.emit(0, line.len(), TokenKind::String);
            self.pos = line.len();
            state.pop();
            return false;
        }
        let mut segment = self.pos;
        let mut i = self.pos;
        while interpolates && i < line.len() {
            if line[i] == b'\\' {
                let end = self.escape_end(i);
                self.emit(segment, i, TokenKind::String);
                self.emit(i, end, TokenKind::StringEscape);
                i = end;
                segment = end;
            } else if line[i..].starts_with(b"#{") {
                self.emit(segment, i, TokenKind::String);
                return self.open_embedded(state, i, i + 2);
            } else {
                i += 1;
            }
        }
        self.emit(segment, line.len(), TokenKind::String);
        self.pos = line.len();
        false
    }

    /// Where a Ruby method name ending at `end` really ends, with a `?` or
    /// `!` (`empty?`, `save!`, but not `a != b` or `x ?y : z`), or the
    /// `=` of a setter's definition (`def name=(value)`).
    fn method_suffix_end(&self, end: usize) -> usize {
        let next = self.at(end + 1);
        match self.at(end) {
            b'?' if !is_identifier_char(next, false) && next != b':' => end + 1,
            b'!' if next != b'=' => end + 1,
            b'=' if next == b'(' && self.pending == Some(TokenKind::FunctionDefinition) => end + 1,
            _ => end,
        }
    }

    /// If a Ruby string or `%` literal starts at `start`, where its body
    /// starts and the context it is lexed in.
    fn ruby_literal_start(&self, start: usize) -> Option<(usize, Context)> {
        let delimited = |close, interpolates, regex| Context::Delimited {
            close,
            depth: 0,
            interpolates,
            regex,
        };
        match self.at(start) {
            b @ (b'"' | b'`') => Some((start + 1, delimited(b, true, false))),
            b'\'' => Some((start + 1, delimited(b'\'', false, false))),
            b'%' => {
                let letter = self.at(start + 1);
                let (letter, at) = if letter.is_ascii_alphabetic() {
                    (letter, start + 2)
                } else {
                    (0, start + 1)
                };
                if !matches!(
                    letter,
                    0 | b'q' | b'Q' | b'w' | b'W' | b'i' | b'I' | b'r' | b's' | b'x'
                ) {
                    return None;
                }
                let open = self.at(at);
                if !open.is_ascii_punctuation() {
                    return None;
                }
                // After a value, `%` is the modulo operator: `x % 2`,
                // `count %(n)`. A method name can take a literal argument
                // only with a letter, as in `puts %w[a b]`.
                if self.prev == Prev::Value || (letter == 0 && self.prev == Prev::Identifier) {
                    return None;
                }
                let interpolates = matches!(letter, 0 | b'Q' | b'W' | b'I' | b'r' | b'x');
                Some((
                    at + 1,
                    delimited(closing_bracket(open), interpolates, letter == b'r'),
                ))
            }
            _ => None,
        }
    }

    /// If a Ruby heredoc opener such as `<<~SQL` or `<<-'EOS'` starts at
    /// `start`, where it ends and the context its body is lexed in.
    fn heredoc_start(&self, start: usize) -> Option<(usize, Context)> {
        let line = self.line;
        if !self.starts_with("<<")
            || (start > 0 && !matches!(line[start - 1], b' ' | b'\t' | b'(' | b',' | b'[' | b'='))
        {
            return None;
        }
        let mut i = start + 2;
        let indented = matches!(self.at(i), b'~' | b'-');
        if indented {
            i += 1;
        }
        let quote = self.at(i);
        let quoted = matches!(quote, b'\'' | b'"' | b'`');
        let tag_start = if quoted { i + 1 } else { i };
        // A bare `<<NAME` needs a constant's name, to tell it from a shift.
        let first = self.at(tag_start);
        if !(quoted || indented || first.is_ascii_uppercase() || first == b'_') {
            return None;
        }
        let mut end = tag_start;
        while end < line.len() && is_identifier_char(line[end], false) {
            end += 1;
        }
        if end == tag_start || (quoted && self.at(end) != quote) {
            return None;
        }
        let context = Context::Heredoc {
            tag: tag_hash(&line[tag_start..end]),
            interpolates: quote != b'\'',
        };
        Some((if quoted { end + 1 } else { end }, context))
    }

    /// If a string that needs a [`Context::RichString`] starts at
    /// `start`, where its body starts and the context's `quotes`,
    /// `escapes`, and `dollars`.
    fn rich_string_start(&self, start: usize) -> Option<(usize, u8, bool, u8)> {
        let count = |n: usize| n.min(u8::MAX as usize) as u8;
        match self.spec.rich_strings {
            RichStrings::None | RichStrings::Ruby => None,
            RichStrings::Kotlin => {
                if self.at(start) != b'"' {
                    return None;
                }
                Some(if self.run_of(start, b'"') >= 3 {
                    (start + 3, 3, false, 1)
                } else {
                    (start + 1, 1, true, 1)
                })
            }
            RichStrings::CSharp => {
                let mut i = start;
                let mut dollars = 0;
                let mut verbatim = false;
                loop {
                    match self.at(i) {
                        b'$' => dollars += 1,
                        b'@' if !verbatim => verbatim = true,
                        _ => break,
                    }
                    i += 1;
                }
                if self.at(i) != b'"' {
                    return None;
                }
                let run = self.run_of(i, b'"');
                if verbatim {
                    Some((i + 1, 1, false, count(dollars)))
                } else if run >= 3 {
                    Some((i + run, count(run), false, count(dollars)))
                } else if dollars > 0 {
                    Some((i + 1, 1, true, count(dollars)))
                } else {
                    // An ordinary string.
                    None
                }
            }
        }
    }

    /// The end of a C# attribute such as `[Test]` or `[assembly: Foo(1)]`
    /// starting at the `[` at `start`, if that is what it is: it starts
    /// with a capitalized name or a target, closes on this line, and isn't
    /// followed by what would make it an expression.
    fn bracket_attribute_end(&self, start: usize) -> Option<usize> {
        let line = self.line;
        let name = start + 1;
        if !is_identifier_start(self.at(name), false) {
            return None;
        }
        let mut i = name;
        while i < line.len() && is_identifier_char(line[i], false) {
            i += 1;
        }
        let target = self.at(i) == b':' && self.at(i + 1) != b':';
        if !is_capitalized(&line[name..i]) && !target {
            return None;
        }
        let mut depth = 0;
        let mut i = start;
        while i < line.len() {
            match line[i] {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        let end = i + 1;
                        let (after, _) = self.peek_significant(end);
                        return (!matches!(after, b';' | b',' | b')' | b'.' | b'=')).then_some(end);
                    }
                }
                b'"' => {
                    // Skip a string argument, which may hold brackets.
                    i += 1;
                    while i < line.len() && line[i] != b'"' {
                        i += if line[i] == b'\\' { 2 } else { 1 };
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// Whether a Kotlin receiver type ending at `end` is followed by the
    /// dot before the name being defined, as in `fun Foo.bar`,
    /// `fun List<T>.bar`, or `fun Foo?.bar`.
    fn receiver_follows(&self, end: usize) -> bool {
        let (mut b, mut i) = self.peek_significant(end);
        if b == b'<' {
            let mut depth = 0;
            while i < self.line.len() {
                match self.line[i] {
                    b'<' => depth += 1,
                    b'>' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    b'(' | b')' | b'{' | b'}' | b';' | b'=' => return false,
                    _ => {}
                }
                i += 1;
            }
            (b, i) = self.peek_significant(i + 1);
        }
        if b == b'?' {
            i += 1;
            b = self.at(i);
        }
        b == b'.' && self.at(i + 1) != b'.'
    }

    /// Whether the identifier ending at `end` is one of the names a Go
    /// short variable declaration defines: `x :=` or `a, b :=`.
    fn declares(&self, end: usize) -> bool {
        let mut i = end;
        loop {
            let (b, j) = self.peek_significant(i);
            if b == b':' && self.at(j + 1) == b'=' {
                return true;
            }
            if b != b',' {
                return false;
            }
            let (b, mut k) = self.peek_significant(j + 1);
            if !is_identifier_start(b, false) {
                return false;
            }
            while k < self.line.len() && is_identifier_char(self.line[k], false) {
                k += 1;
            }
            i = k;
        }
    }

    /// The kind of `word` if it is a keyword or other reserved word here.
    /// A soft keyword is one only before a name (other than an operator
    /// such as `in`) or an opening bracket, or at the end of the line
    /// after another soft keyword (Kotlin's `private set`), and not where
    /// a definition expects a name; `next` is the significant byte after
    /// it, at `next_pos`.
    fn reserved_kind(&self, word: &[u8], next: u8, next_pos: usize) -> Option<TokenKind> {
        let (kind, soft) = self.spec.word_kind(word)?;
        if !soft {
            return Some(kind);
        }
        if self.pending.is_some_and(is_definition) {
            return None;
        }
        let keyword = if is_identifier_start(next, self.spec.dollar_identifiers) {
            let mut end = next_pos;
            while end < self.line.len() && is_identifier_char(self.line[end], false) {
                end += 1;
            }
            !matches!(&self.line[next_pos..end], b"in" | b"is" | b"as")
        } else if next_pos >= self.line.len() {
            self.out.last().is_some_and(|t| {
                t.kind == TokenKind::Keyword
                    && self.spec.word_kind(&self.line[t.range.clone()]) == Some((kind, true))
            })
        } else {
            matches!(next, b'(' | b'{')
        };
        keyword.then_some(kind)
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
        self.format_hole = spec.rich_strings == RichStrings::CSharp
            && state.top() == Some(Context::TemplateExpression { braces: 0 })
            && !self.saw_question;

        // Comments.
        if spec.begin_end_comments && start == 0 && is_line_keyword(line, b"=begin") {
            state.push(Context::BlockComment {
                doc: false,
                depth: 1,
            });
            self.emit(start, line.len(), TokenKind::Comment);
            self.pos = line.len();
            return false;
        }
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
                // `return@outer` refers to a label.
                if spec.at_labels && start > 0 && is_identifier_char(line[start - 1], false) {
                    let mut i = start + 1;
                    while i < line.len() && is_identifier_char(line[i], false) {
                        i += 1;
                    }
                    self.emit(start, i, TokenKind::Label);
                    self.pos = i;
                    self.prev = Prev::Operator;
                    return true;
                }
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
            Attributes::Bracket if b == b'[' && self.prev == Prev::Start => {
                if let Some(end) = self.bracket_attribute_end(start) {
                    self.emit(start, end, TokenKind::Attribute);
                    self.pos = end;
                    return true;
                }
            }
            _ => {}
        }

        // Ruby's literals, variables, and symbols.
        if spec.rich_strings == RichStrings::Ruby {
            if let Some((body, context)) = self.ruby_literal_start(start) {
                let kind = if matches!(context, Context::Delimited { regex: true, .. }) {
                    TokenKind::Regex
                } else {
                    TokenKind::String
                };
                self.emit(start, body, kind);
                self.pos = body;
                state.push(context);
                let more = self.continue_embedding(state);
                self.merge_string_start(start);
                return more;
            }
            if let Some((end, context)) = self.heredoc_start(start) {
                self.emit(start, end, TokenKind::String);
                self.pos = end;
                self.heredoc.get_or_insert(context);
                self.prev = Prev::Value;
                self.pending = None;
                return true;
            }
        }
        if spec.sigils && matches!(b, b'@' | b'$') {
            let mut i = start + 1;
            if b == b'@' && self.at(i) == b'@' {
                i += 1;
            }
            let name = i;
            while i < line.len() && is_identifier_char(line[i], false) {
                i += 1;
            }
            // Special globals: `$!`, `$0`, `$~`, ...
            if i == name && b == b'$' && self.at(i).is_ascii_punctuation() {
                i += 1;
            }
            if i > name && (b == b'$' || !self.at(name).is_ascii_digit()) {
                self.emit(start, i, TokenKind::Variable);
                self.pos = i;
                self.prev = Prev::Value;
                self.pending = None;
                return true;
            }
        }
        if spec.symbols
            && b == b':'
            && is_identifier_start(self.at(start + 1), false)
            && (start == 0
                || !is_identifier_char(line[start - 1], false) && line[start - 1] != b':')
        {
            let mut end = start + 2;
            while end < line.len() && is_identifier_char(line[end], false) {
                end += 1;
            }
            if spec.method_suffixes && matches!(self.at(end), b'?' | b'!' | b'=') {
                end += 1;
            }
            self.emit(start, end, TokenKind::Constant);
            self.pos = end;
            self.prev = Prev::Value;
            self.pending = None;
            return true;
        }

        // C# and Kotlin strings beyond the ordinary ones, and C# verbatim
        // identifiers such as `@class`.
        if spec.rich_strings != RichStrings::None {
            if let Some((body, quotes, escapes, dollars)) = self.rich_string_start(start) {
                self.emit(start, body, TokenKind::String);
                self.pos = body;
                state.push(Context::RichString {
                    quotes,
                    escapes,
                    dollars,
                });
                let more = self.continue_rich_string(state, quotes, escapes, dollars);
                self.merge_string_start(start);
                return more;
            }
            if spec.rich_strings == RichStrings::CSharp
                && b == b'@'
                && is_identifier_start(self.at(start + 1), false)
            {
                let mut end = start + 2;
                while end < line.len() && is_identifier_char(line[end], false) {
                    end += 1;
                }
                return self.identifier_token(start, end, &line[start..end]);
            }
        }

        // Identifiers, and strings with prefixes.
        if is_identifier_start(b, spec.dollar_identifiers) {
            let mut end = start + 1;
            while end < line.len() && is_identifier_char(line[end], spec.dollar_identifiers) {
                end += 1;
            }
            if spec.method_suffixes {
                end = self.method_suffix_end(end);
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
        if b == b'`' {
            match spec.backticks {
                Backticks::None => {}
                Backticks::Template => {
                    self.emit(start, start + 1, TokenKind::String);
                    self.pos = start + 1;
                    state.push(Context::Template);
                    let more = self.continue_template(state);
                    self.merge_string_start(start);
                    return more;
                }
                Backticks::RawString => {
                    return self.string_token(state, start, start, b, false, 0);
                }
                Backticks::Identifier => {
                    if let Some(n) = line[start + 1..].iter().position(|&c| c == b'`') {
                        let end = start + n + 2;
                        return self.identifier_token(start, end, &line[start..end]);
                    }
                }
            }
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
                if braces == 0 {
                    state.pop();
                    // A C# `$$` string's expressions close with `}}`.
                    let close = match state.top() {
                        Some(Context::RichString { dollars, .. }) => {
                            self.run_of(start, b'}').clamp(1, dollars.max(1) as usize)
                        }
                        _ => 1,
                    };
                    self.emit(start, start + close, TokenKind::Punctuation);
                    self.pos = start + close;
                    return self.continue_embedding(state);
                }
                self.emit(start, start + 1, TokenKind::Punctuation);
                self.pos = start + 1;
                state.replace(Context::TemplateExpression { braces: braces - 1 });
                self.prev = Prev::Value;
                return true;
            } else if self.format_hole && b == b':' && self.at(start + 1) != b':' {
                // A format string, as in `{value:N2}`, runs to the brace.
                let end = line[start..]
                    .iter()
                    .position(|&c| c == b'}')
                    .map_or(line.len(), |n| start + n);
                self.emit(start, end, TokenKind::String);
                self.pos = end;
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
            && matches!(self.out[n - 2].kind, TokenKind::String | TokenKind::Regex)
            && self.out[n - 1].kind == self.out[n - 2].kind
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

        let reserved = self.reserved_kind(word, next, next_pos);
        if spec.at_labels && self.at(end) == b'@' && reserved.is_none() {
            // A Kotlin label: `outer@ for (...)`.
            self.emit(start, end + 1, TokenKind::Label);
            self.pos = end + 1;
            self.prev = Prev::Operator;
            self.pending = None;
            return true;
        }

        let mut prev = Prev::Identifier;
        // Whether the pending definition carries on past this name.
        let mut keep = false;
        // After a dot, a name is a member, unless the dot is part of the
        // definition still pending.
        let kind = if self.prev == Prev::Dot && self.pending.is_none() && word != b"await" {
            if called {
                TokenKind::Function
            } else {
                TokenKind::Field
            }
        } else if let Some(kind) = reserved {
            // Ruby's `def self.name`.
            keep = spec.receivers == Receivers::Dotted
                && self.pending == Some(TokenKind::FunctionDefinition)
                && self.receiver_follows(end);
            prev = match kind {
                TokenKind::Constant => Prev::Value,
                TokenKind::Keyword | TokenKind::ControlKeyword => {
                    if matches!(
                        word,
                        b"this" | b"super" | b"self" | b"Self" | b"cls" | b"base"
                    ) {
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
            if spec.receivers == Receivers::Dotted
                && matches!(
                    pending,
                    TokenKind::FunctionDefinition | TokenKind::VariableDefinition
                )
                && self.receiver_follows(end)
            {
                keep = true;
                TokenKind::Type
            } else if pending == TokenKind::VariableDefinition && is_screaming(word) {
                prev = Prev::Value;
                TokenKind::Constant
            } else if pending == TokenKind::VariableDefinition
                && is_capitalized(word)
                && !spec.capitalized_values
            {
                TokenKind::Type
            } else if pending == TokenKind::Namespace && spec.path_types && is_capitalized(word) {
                // `import java.util.List` ends with the class.
                TokenKind::Type
            } else {
                keep = pending == TokenKind::Namespace && next == b'.';
                pending
            }
        } else if spec.short_declarations && self.declares(end) {
            TokenKind::VariableDefinition
        } else if scoped && (is_capitalized(word) || spec.scope_namespaces) {
            if is_capitalized(word) {
                TokenKind::Type
            } else {
                TokenKind::Namespace
            }
        } else if called {
            if is_screaming(word) || !is_capitalized(word) || spec.capitalized_values {
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
            && !self.format_hole
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
                self.keep_pending = false;
            } else if keep {
                self.keep_pending = true;
            }
        } else if keep {
            self.keep_pending = true;
        } else {
            if self.pending.is_some() {
                self.keep_pending = false;
            }
            self.pending = None;
        }
        self.prev = prev;
        true
    }

    fn operator_token(&mut self, start: usize) -> bool {
        const THREE: &[&str] = &[
            "<<=", ">>=", "...", "===", "!==", "**=", "..=", ">>>", "<=>", "->*", "??=",
        ];
        const TWO: &[&str] = &[
            "->", "=>", "::", "==", "!=", "<=", ">=", "&&", "||", "+=", "-=", "*=", "/=", "%=",
            "&=", "|=", "^=", "<<", ">>", "..", "++", "--", "**", "?.", "??", "//", ".*", "|>",
            ":=", "?:",
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
        // A Ruby block's parameters: `do |a, b|`, `{ |x| ... }`.
        let mut in_params = false;
        if self.spec.block_parameters && op == b"|" {
            if self.block_params {
                self.block_params = false;
            } else {
                in_params = self.out.last().is_some_and(|t| {
                    let text = &self.line[t.range.clone()];
                    (t.kind == TokenKind::Keyword && text == b"do")
                        || (t.kind == TokenKind::Punctuation && text == b"{")
                });
            }
        } else {
            in_params = self.block_params;
        }
        self.block_params = in_params;
        self.emit(start, start + len, kind);
        self.pos = start + len;
        self.prev = prev;
        // A Go method's receiver comes between `func` and the name.
        let mut receiver_closed = false;
        if self.spec.receivers == Receivers::Parenthesized {
            match op {
                b"(" if self.receiver_depth > 0 => self.receiver_depth += 1,
                b"(" if self.pending == Some(TokenKind::FunctionDefinition)
                    && start > 0
                    && matches!(self.line[start - 1], b' ' | b'\t') =>
                {
                    self.receiver_depth = 1;
                }
                b")" if self.receiver_depth > 0 => {
                    self.receiver_depth -= 1;
                    receiver_closed = self.receiver_depth == 0;
                }
                _ => {}
            }
        }
        // Generic parameters and address spaces don't interrupt a
        // definition: `impl<T> Foo`, `var<private> name`. Nor does the
        // dot in a dotted path or after a Kotlin receiver.
        match op {
            b"<" if self.pending.is_some() => self.stashed = self.pending.take(),
            b">" if self.stashed.is_some() => self.pending = self.stashed.take(),
            b"." | b"?." if self.keep_pending && self.pending.is_some() => {
                self.keep_pending = false;
            }
            _ => {
                self.pending = None;
                self.stashed = None;
                self.keep_pending = false;
            }
        }
        if receiver_closed {
            self.pending = Some(TokenKind::FunctionDefinition);
        }
        if in_params {
            self.pending = Some(TokenKind::VariableDefinition);
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

    #[test]
    fn csharp() {
        check(
            Language::CSharp,
            "\
using System.Text; using var s = File.OpenRead(path);
kkkkk mmmmmmpmmmmp kkkkk kkk v o ttttpffffffffpiiiipp
[Serializable, Obsolete(\"[x]\")] public partial record Point(int X);
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa kkkkkk kkkkkkk kkkkkk DDDDDpTTT tpp
public string Name { get; init; } = $\"{first} {{x}} {n:N2}\" + @\"a\"\"b\";
kkkkkk TTTTTT tttt p kkkp kkkkp p o sspiiiiipseeseespisssps o ssseessp
var q = from x in items where x > 1 select DoThing(x, @class);
kkk v o kkkk i kk iiiii kkkkk i o n kkkkkk fffffffpip iiiiiipp
/// <summary>Docs.</summary>
CCCCCCCCCCCCCCCCCCCCCCCCCCCC
#if DEBUG
PPP NNNNN
return list?.Count ?? new List<int>().Count;
KKKKKK iiiippddddd oo kkk ttttoTTTopppdddddp

",
        );
    }

    #[test]
    fn java() {
        check(
            Language::Java,
            "\
import static org.junit.Assert.assertEquals;
kkkkkk kkkkkk mmmpmmmmmpttttttpddddddddddddp
@Override public sealed interface Shape permits Circle {}
aaaaaaaaa kkkkkk kkkkkk kkkkkkkkk DDDDD kkkkkkk tttttt pp
public record Circle(double r) { static final int MAX = 0x1FL; }
kkkkkk kkkkkk DDDDDDpTTTTTT ip p kkkkkk kkkkk TTT NNN o nnnnnp p
var text = \"\"\"
kkk vvvv o sss
    a \"quoted\" \\n line
ssssssssssssssseesssss
    \"\"\";
sssssssp
names.forEach(System.out::println); int record = 'c';
iiiiipfffffffpttttttpdddppfffffffpp TTT iiiiii o hhhp

",
        );
    }

    #[test]
    fn kotlin() {
        check(
            Language::Kotlin,
            "\
import java.util.List
kkkkkk mmmmpmmmmptttt
data class User(val name: String) : Base() { private set }
kkkk kkkkk DDDDpkkk vvvvo ttttttp o ttttpp p kkkkkkk iii p
fun <T> List<T>.second(): T = this[1] ?: error(\"$name ${x + 1}\")
kkk oto ttttotopFFFFFFppo t o kkkkpnp oo fffffps$$$$$sppi o npsp
val open = data.filter { it > 1 }; fun `a test`() {}
kkk vvvv o iiiipdddddd p ii o n pp kkk FFFFFFFFpp pp
loop@ for (x in xs) { break@loop }
LLLLL KKK pi kk iip p KKKKKLLLLL p
val s = \"\"\"raw $x \\n
kkk v o sssssss$$sss
${y}\"\"\"
ppipsss

",
        );
    }

    #[test]
    fn go() {
        check(
            Language::Go,
            "\
package main
kkkkkkk mmmm
func (s *Server) Run(ctx context.Context) error {
kkkk pi ottttttp FFFpiii iiiiiiipdddddddp TTTTT p
	x, err := s.start(`raw
 vp vvv oo ipfffffpssss
string`)
sssssssp
	f := func(a int) int { return NewThing(a) }
 v oo kkkkpi TTTp TTT p KKKKKK ffffffffpip p
	srv := &Server{Name: \"x\", port: 80, ch: 'a'}
 vvv oo ottttttptttto sssp ddddo nnp ddo hhhp
type List[T any] struct { next *List[T] }
kkkk DDDDpt TTTp kkkkkk p iiii ottttptp p

",
        );
    }

    #[test]
    fn ruby() {
        check(
            Language::Ruby,
            "\
require_relative \"lib/#{name}\" # comment
kkkkkkkkkkkkkkkk sssssppiiiips ccccccccc
def self.build(items = [], discount: 0) = new(**opts)
kkk kkkkpFFFFFpiiiii o ppp ddddddddo np o fffpooiiiip
def empty? = @items.empty? && $stdout && @@count != 1
kkk FFFFFF o $$$$$$pdddddd oo $$$$$$$ oo $$$$$$$ oo n
attr_accessor :items; ok = x =~ /ab+c/ ? :yes : :no
kkkkkkkkkkkkk NNNNNNp ii o i oo rrrrrr o NNNN o NNN
words = %w[a b] + %r{^/x/(\\d+)$}i + %(a (b) c); y % 3
iiiii o sssssss o rrrrrrrreerrrrr o ssssssssssp i o n
@items.each { |item, i| save! unless item.valid? }
$$$$$$pdddd p ovvvvp vo iiiii KKKKKK iiiipdddddd p
Shop::Order.find(id)&.update(total: 0, 'k' => 'it\\'s \\n')
ttttpptttttpffffpiipopffffffpdddddo np sss oo ssseesssssp

",
        );
    }

    #[test]
    fn ruby_literals_carry_across_lines() {
        // A heredoc's body starts on the next line, and ends at its
        // terminator however it is indented.
        let lines = lex_text(
            Language::Ruby,
            "x = <<~SQL.strip + 'a\n  SELECT #{id}\n  SQL\nb' + y\n=begin\nc\n=end\nd\n",
        );
        assert_eq!(lines[0][4].kind, TokenKind::Field);
        assert_eq!(lines[0][6].kind, TokenKind::String);
        assert_eq!(lines[1][1].kind, TokenKind::Punctuation);
        assert_eq!(lines[1][2].kind, TokenKind::Identifier);
        assert_eq!(lines[2][0].kind, TokenKind::String);
        // The string opened on the heredoc's line resumes after it.
        assert_eq!(lines[3][0].kind, TokenKind::String);
        assert_eq!(lines[3].last().unwrap().kind, TokenKind::Identifier);
        assert_eq!(lines[5][0].kind, TokenKind::Comment);
        assert_eq!(lines[6][0].kind, TokenKind::Comment);
        assert_eq!(lines[7][0].kind, TokenKind::Identifier);

        // A  literal nests its brackets across lines.
        let lines = lex_text(Language::Ruby, "%w[a [\nb] c] d\n");
        assert_eq!(lines[1][0].kind, TokenKind::String);
        assert_eq!(lines[1][1].kind, TokenKind::Identifier);
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
    fn rich_strings_carry_across_lines() {
        // A C# verbatim string continues, an interpolated one doesn't.
        let lines = lex_text(Language::CSharp, "@\"a\nb\"\" c\" d\n$\"e\nf\n");
        assert_eq!(lines[1][0].kind, TokenKind::String);
        assert_eq!(lines[1][1].kind, TokenKind::StringEscape);
        assert_eq!(lines[1].last().unwrap().kind, TokenKind::Identifier);
        assert_eq!(lines[3][0].kind, TokenKind::Identifier);

        // An expression in a raw string spans lines, and closes with as
        // many braces as the string has dollars.
        let lines = lex_text(Language::CSharp, "$$\"\"\"\n{{a +\nb}} c\n\"\"\" d\n");
        assert_eq!(lines[1][0].kind, TokenKind::Punctuation);
        assert_eq!(lines[2][0].kind, TokenKind::Identifier);
        assert_eq!(
            lines[2][1],
            Token {
                range: 1..3,
                kind: TokenKind::Punctuation
            }
        );
        assert_eq!(lines[2][2].kind, TokenKind::String);
        assert_eq!(lines[3][0].kind, TokenKind::String);
        assert_eq!(lines[3][1].kind, TokenKind::Identifier);

        // A Kotlin raw string has templates on every line.
        let lines = lex_text(Language::Kotlin, "\"\"\"a\n$b ${\nc\n}\"\"\"\n");
        assert_eq!(lines[1][0].kind, TokenKind::Variable);
        assert_eq!(lines[2][0].kind, TokenKind::Identifier);
        assert_eq!(lines[3][0].kind, TokenKind::Punctuation);
        assert_eq!(lines[3][1].kind, TokenKind::String);

        // A Go raw string spans lines without escapes.
        let lines = lex_text(Language::Go, "`a\\\nb` c\n");
        assert_eq!(
            lines[0],
            vec![Token {
                range: 0..3,
                kind: TokenKind::String
            }]
        );
        assert_eq!(lines[1][1].kind, TokenKind::Identifier);
    }

    #[test]
    fn soft_keywords() {
        let kinds = |language, text| {
            lex_text(language, text)[0]
                .iter()
                .map(|t| t.kind)
                .collect::<Vec<_>>()
        };
        use TokenKind::*;
        // Before a name, a soft keyword is one; elsewhere it is a name.
        assert_eq!(
            kinds(Language::Kotlin, "data class A"),
            [Keyword, Keyword, TypeDefinition]
        );
        assert_eq!(
            kinds(Language::Kotlin, "f(data, data.x)"),
            [
                Function,
                Punctuation,
                Identifier,
                Punctuation,
                Identifier,
                Punctuation,
                Field,
                Punctuation
            ]
        );
        assert_eq!(
            kinds(Language::Kotlin, "if (value in xs) return data"),
            [
                ControlKeyword,
                Punctuation,
                Identifier,
                Keyword,
                Identifier,
                Punctuation,
                ControlKeyword,
                Identifier
            ]
        );
        // Where a definition expects a name, it is the name.
        assert_eq!(
            kinds(Language::Kotlin, "operator fun get(i: Int)")[..3],
            [Keyword, Keyword, FunctionDefinition]
        );
        assert_eq!(
            kinds(Language::Java, "var record = 1;")[..2],
            [Keyword, VariableDefinition]
        );
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
            Language::CSharp,
            Language::Java,
            Language::Kotlin,
            Language::Go,
            Language::Ruby,
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
            b"$\"{",
            b"$$\"\"\"{{",
            b"@\"",
            b"[A",
            b"`a",
            b"func (",
            b"fun a.",
            b"x@",
            b"%w[",
            b"<<~",
            b"<<~'A",
            b":",
            b"$",
            b"@@",
            b"=begin",
        ];
        for language in [
            Language::Rust,
            Language::C,
            Language::Cpp,
            Language::JavaScript,
            Language::TypeScript,
            Language::Wgsl,
            Language::Python,
            Language::CSharp,
            Language::Java,
            Language::Kotlin,
            Language::Go,
            Language::Ruby,
        ] {
            let lexer = language.lexer();
            for input in inputs {
                let mut out = Vec::new();
                lexer.lex_line(LexState::default(), input, &mut out);
            }
        }
    }
}
