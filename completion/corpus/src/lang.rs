//! Identifies the programming language of a source file from its name.

use crate::filter::CommentStyle;

pub struct Lang {
    pub name: &'static str,
    /// File extensions, without the dot. Matched case-sensitively first (`.C` is C++, `.c` is
    /// C), then lowercased.
    pub extensions: &'static [&'static str],
    /// Whole file names, for build files that have no extension.
    pub file_names: &'static [&'static str],
    pub comments: &'static CommentStyle,
}

impl Lang {
    /// How the corpus names the language, in each file's record and as the directory of a
    /// Debian corpus: `C++` is `cpp`, `C#` is `csharp`. Models name languages the same way.
    pub fn identifier(&self) -> String {
        self.name
            .to_ascii_lowercase()
            .replace('+', "p")
            .replace('#', "sharp")
            .replace(' ', "-")
    }
}

const DASH: CommentStyle = CommentStyle {
    line: &["--"],
    doc_line: &[],
    block: None,
    doc_block: &[],
    preamble: &["#!"],
};

const SEMICOLON: CommentStyle = CommentStyle {
    line: &[";"],
    doc_line: &[],
    block: None,
    doc_block: &[],
    preamble: &[],
};

const PERCENT: CommentStyle = CommentStyle {
    line: &["%"],
    doc_line: &[],
    block: None,
    doc_block: &[],
    preamble: &["#!"],
};

const FORTRAN: CommentStyle = CommentStyle {
    line: &["!", "c ", "C ", "*"],
    doc_line: &[],
    block: None,
    doc_block: &[],
    preamble: &[],
};

const ML: CommentStyle = CommentStyle {
    line: &[],
    doc_line: &[],
    block: Some(("(*", "*)")),
    doc_block: &['*'],
    preamble: &["#!"],
};

const PASCAL: CommentStyle = CommentStyle {
    line: &["//"],
    doc_line: &[],
    block: Some(("{", "}")),
    doc_block: &['$'],
    preamble: &[],
};

const PHP: CommentStyle = CommentStyle {
    line: &["//", "#"],
    doc_line: &[],
    block: Some(("/*", "*/")),
    doc_block: &['*'],
    preamble: &["#!", "<?php"],
};

/// C-family languages whose files may start with a shebang (scripts run by an interpreter).
const C_SCRIPT: CommentStyle = CommentStyle {
    preamble: &["#!"],
    ..CommentStyle::C
};

/// Every recognized language. Indexes into this table identify languages elsewhere.
pub const LANGS: &[Lang] = &[
    Lang {
        name: "C",
        extensions: &["c"],
        file_names: &[],
        comments: &CommentStyle::C,
    },
    Lang {
        name: "C++",
        extensions: &[
            "cc", "cpp", "cxx", "c++", "C", "hh", "hpp", "hxx", "h++", "H", "ipp", "tcc", "tpp",
            "txx", "inl",
        ],
        file_names: &[],
        comments: &CommentStyle::C,
    },
    Lang {
        name: "Python",
        extensions: &["py", "pyi", "pyw"],
        file_names: &[],
        comments: &CommentStyle::HASH,
    },
    Lang {
        name: "Cython",
        extensions: &["pyx", "pxd", "pxi"],
        file_names: &[],
        comments: &CommentStyle::HASH,
    },
    Lang {
        name: "Go",
        extensions: &["go"],
        file_names: &[],
        comments: &CommentStyle::C,
    },
    Lang {
        name: "Rust",
        extensions: &["rs"],
        file_names: &[],
        comments: &CommentStyle::C,
    },
    Lang {
        name: "Java",
        extensions: &["java"],
        file_names: &[],
        comments: &CommentStyle::C,
    },
    Lang {
        name: "Kotlin",
        extensions: &["kt", "kts"],
        file_names: &[],
        comments: &CommentStyle::C,
    },
    Lang {
        name: "Scala",
        extensions: &["scala", "sc"],
        file_names: &[],
        comments: &CommentStyle::C,
    },
    Lang {
        name: "Groovy",
        extensions: &["groovy", "gradle"],
        file_names: &[],
        comments: &CommentStyle::C,
    },
    Lang {
        name: "C#",
        extensions: &["cs"],
        file_names: &[],
        comments: &CommentStyle::C,
    },
    Lang {
        name: "JavaScript",
        extensions: &["js", "mjs", "cjs", "jsx"],
        file_names: &[],
        comments: &C_SCRIPT,
    },
    Lang {
        name: "TypeScript",
        extensions: &["ts", "tsx", "mts", "cts"],
        file_names: &[],
        comments: &C_SCRIPT,
    },
    Lang {
        name: "Swift",
        extensions: &["swift"],
        file_names: &[],
        comments: &CommentStyle::C,
    },
    Lang {
        name: "Dart",
        extensions: &["dart"],
        file_names: &[],
        comments: &CommentStyle::C,
    },
    Lang {
        name: "D",
        extensions: &["d", "di"],
        file_names: &[],
        comments: &CommentStyle::C,
    },
    Lang {
        name: "Zig",
        extensions: &["zig"],
        file_names: &[],
        comments: &CommentStyle::C,
    },
    Lang {
        name: "Vala",
        extensions: &["vala", "vapi"],
        file_names: &[],
        comments: &CommentStyle::C,
    },
    Lang {
        name: "PHP",
        extensions: &["php"],
        file_names: &[],
        comments: &PHP,
    },
    Lang {
        name: "Shell",
        extensions: &["sh", "bash", "zsh", "ksh"],
        file_names: &[],
        comments: &CommentStyle::HASH,
    },
    Lang {
        name: "Perl",
        extensions: &["pl", "pm"],
        file_names: &[],
        comments: &CommentStyle::HASH,
    },
    Lang {
        name: "Ruby",
        extensions: &["rb", "rake", "gemspec"],
        file_names: &["Rakefile", "Gemfile"],
        comments: &CommentStyle::HASH,
    },
    Lang {
        name: "Lua",
        extensions: &["lua"],
        file_names: &[],
        comments: &DASH,
    },
    Lang {
        name: "Tcl",
        extensions: &["tcl"],
        file_names: &[],
        comments: &CommentStyle::HASH,
    },
    Lang {
        name: "R",
        extensions: &["R", "r"],
        file_names: &[],
        comments: &CommentStyle::HASH,
    },
    Lang {
        name: "Julia",
        extensions: &["jl"],
        file_names: &[],
        comments: &CommentStyle::HASH,
    },
    Lang {
        name: "Nim",
        extensions: &["nim"],
        file_names: &[],
        comments: &CommentStyle::HASH,
    },
    Lang {
        name: "Elixir",
        extensions: &["ex", "exs"],
        file_names: &[],
        comments: &CommentStyle::HASH,
    },
    Lang {
        name: "Erlang",
        extensions: &["erl", "hrl"],
        file_names: &[],
        comments: &PERCENT,
    },
    Lang {
        name: "Haskell",
        extensions: &["hs"],
        file_names: &[],
        comments: &DASH,
    },
    Lang {
        name: "OCaml",
        extensions: &["ml", "mli"],
        file_names: &[],
        comments: &ML,
    },
    Lang {
        name: "Emacs Lisp",
        extensions: &["el"],
        file_names: &[],
        comments: &SEMICOLON,
    },
    Lang {
        name: "Scheme",
        extensions: &["scm", "ss", "sld", "sls", "rkt"],
        file_names: &[],
        comments: &SEMICOLON,
    },
    Lang {
        name: "Common Lisp",
        extensions: &["lisp", "lsp", "asd"],
        file_names: &[],
        comments: &SEMICOLON,
    },
    Lang {
        name: "Clojure",
        extensions: &["clj", "cljs", "cljc"],
        file_names: &[],
        comments: &SEMICOLON,
    },
    Lang {
        name: "Fortran",
        extensions: &["f", "for", "f77", "f90", "f95", "f03", "f08", "F", "F90"],
        file_names: &[],
        comments: &FORTRAN,
    },
    Lang {
        name: "Pascal",
        extensions: &["pas", "pp", "lpr", "dpr"],
        file_names: &[],
        comments: &PASCAL,
    },
    Lang {
        name: "Ada",
        extensions: &["adb", "ads"],
        file_names: &[],
        comments: &DASH,
    },
    Lang {
        name: "SQL",
        extensions: &["sql"],
        file_names: &[],
        comments: &DASH,
    },
    Lang {
        name: "Assembly",
        extensions: &["s", "S", "asm"],
        file_names: &[],
        comments: &CommentStyle::C,
    },
    Lang {
        name: "CMake",
        extensions: &["cmake"],
        file_names: &["CMakeLists.txt"],
        comments: &CommentStyle::HASH,
    },
    Lang {
        name: "Make",
        extensions: &["mk", "mak"],
        file_names: &["Makefile", "makefile", "GNUmakefile", "Makefile.am"],
        comments: &CommentStyle::HASH,
    },
    Lang {
        name: "Meson",
        extensions: &[],
        file_names: &["meson.build", "meson_options.txt", "meson.options"],
        comments: &CommentStyle::HASH,
    },
    Lang {
        name: "Cargo",
        extensions: &[],
        file_names: &["Cargo.toml"],
        comments: &CommentStyle::HASH,
    },
];

pub type LangId = u8;

pub const C: LangId = 0;
pub const CPP: LangId = 1;
pub const RUST: LangId = lang_id("Rust");
pub const CARGO: LangId = lang_id("Cargo");
pub const TYPESCRIPT: LangId = lang_id("TypeScript");

const fn lang_id(name: &str) -> LangId {
    let mut i = 0;
    while i < LANGS.len() {
        if LANGS[i].name.eq_ignore_ascii_case(name) {
            return i as LangId;
        }
        i += 1;
    }
    panic!("unknown language");
}

/// The language whose files a language's files are deduplicated with. C and C++ train one
/// model, and a header vendored into a C++ project is the same file as in the C library it
/// came from.
pub fn dedup_group(lang: LangId) -> LangId {
    if lang == CPP { C } else { lang }
}

/// What a file name says about its language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detected {
    Lang(LangId),
    /// A `.h` header, which is C or C++ depending on the rest of the package.
    Header,
}

/// Identifies a file's language from its path, or `None` if it is not a recognized language.
pub fn detect(path: &str) -> Option<Detected> {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    if let Some(i) = LANGS.iter().position(|l| l.file_names.contains(&file_name)) {
        return Some(Detected::Lang(i as LangId));
    }
    let (_, ext) = file_name.rsplit_once('.')?;
    if ext == "h" {
        return Some(Detected::Header);
    }
    let find = |ext: &str| LANGS.iter().position(|l| l.extensions.contains(&ext));
    find(ext)
        .or_else(|| find(&ext.to_ascii_lowercase()))
        .map(|i| Detected::Lang(i as LangId))
}

/// Content that the extension claims but that is not source in the language, such as the
/// XML translation files Qt also names `.ts`.
pub fn misidentified(lang: LangId, content: &str) -> bool {
    lang == TYPESCRIPT && content.trim_start().starts_with('<')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(path: &str) -> Option<&'static str> {
        match detect(path)? {
            Detected::Lang(id) => Some(LANGS[id as usize].name),
            Detected::Header => Some("header"),
        }
    }

    #[test]
    fn detects() {
        assert_eq!(name("src/main.c"), Some("C"));
        assert_eq!(name("src/Main.C"), Some("C++"));
        assert_eq!(name("lib/foo.hpp"), Some("C++"));
        assert_eq!(name("include/foo.h"), Some("header"));
        assert_eq!(name("setup.py"), Some("Python"));
        assert_eq!(name("a/b/CMakeLists.txt"), Some("CMake"));
        assert_eq!(name("Makefile"), Some("Make"));
        assert_eq!(name("Makefile.in"), None);
        assert_eq!(name("README"), None);
        assert_eq!(name("x.PY"), Some("Python"));
        assert_eq!(name("x.S"), Some("Assembly"));
    }

    #[test]
    fn language_ids() {
        assert_eq!(LANGS[C as usize].name, "C");
        assert_eq!(LANGS[CPP as usize].name, "C++");
        assert_eq!(LANGS[RUST as usize].name, "Rust");
        assert_eq!(LANGS[CARGO as usize].name, "Cargo");
        assert_eq!(LANGS[TYPESCRIPT as usize].name, "TypeScript");
    }

    #[test]
    fn identifiers() {
        assert_eq!(LANGS[CPP as usize].identifier(), "cpp");
        assert_eq!(LANGS[CARGO as usize].identifier(), "cargo");
        assert_eq!(LANGS[lang_id("C#") as usize].identifier(), "csharp");
        assert_eq!(
            LANGS[lang_id("Emacs Lisp") as usize].identifier(),
            "emacs-lisp"
        );
    }
}
