//! Evaluates license expressions: SPDX expressions from Cargo manifests against an allow-list,
//! and the SPDX and Debian short names found in source headers and `debian/copyright` files
//! against license tiers. Also recognizes common licenses from their text.

use clap::ValueEnum;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Policy {
    /// Only permissive licenses (MIT, Apache-2.0, BSD, ISC, Zlib, Unlicense, CC0, ...).
    Permissive,
    /// Accept everything regardless of license, including code with none declared.
    Any,
}

impl Policy {
    /// Whether a file whose license was rated `tier` is accepted.
    pub fn admits(self, tier: Tier) -> bool {
        match self {
            Policy::Permissive => tier == Tier::Permissive,
            Policy::Any => true,
        }
    }
}

const PERMISSIVE: &[&str] = &[
    "mit",
    "mit-0",
    "apache-2.0",
    "bsd-2-clause",
    "bsd-3-clause",
    "bsd-2-clause-patent",
    "isc",
    "zlib",
    "unlicense",
    "cc0-1.0",
    "0bsd",
    "bsl-1.0",
    "unicode-dfs-2016",
    "unicode-3.0",
    "wtfpl",
    "ncsa",
    "x11",
    "python-2.0",
    "openssl",
];

const ALLOWED_EXCEPTIONS: &[&str] = &["llvm-exception"];

/// Returns whether a manifest license expression satisfies the policy.
/// `None` means the manifest declared no `license` field.
pub fn allowed(expr: Option<&str>, policy: Policy) -> bool {
    match policy {
        Policy::Any => true,
        Policy::Permissive => expr.is_some_and(|e| {
            evaluate(e, |id, exception| {
                PERMISSIVE.contains(&id)
                    && exception.is_none_or(|x| ALLOWED_EXCEPTIONS.contains(&x))
            })
            .unwrap_or(false)
        }),
    }
}

/// Evaluates a license expression over an ordering of outcomes, better ones greater: `OR`
/// takes the best of its alternatives and `AND` the worst of its requirements. `leaf` rates
/// one license id, lowercased and without a trailing `+`, along with the exception named after
/// `WITH`, lowercased and with multi-word names joined by `-`. Returns `None` if the
/// expression does not parse.
pub fn evaluate<T: Ord>(expr: &str, leaf: impl Fn(&str, Option<&str>) -> T) -> Option<T> {
    let mut parser = Parser::new(expr);
    let result = parser.or_expr(&leaf)?;
    (parser.pos == parser.tokens.len()).then_some(result)
}

struct Parser {
    tokens: Vec<String>,
    pos: usize,
}

impl Parser {
    fn new(expr: &str) -> Parser {
        // Old manifests use `MIT/Apache-2.0` to mean either license.
        let normalized = expr
            .replace('/', " OR ")
            .replace('(', " ( ")
            .replace(')', " ) ");
        let tokens = normalized
            .split_whitespace()
            .map(|t| t.to_ascii_lowercase())
            .collect();
        Parser { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.pos).map(String::as_str)
    }

    fn next(&mut self) -> Option<&str> {
        let token = self.tokens.get(self.pos).map(String::as_str);
        self.pos += 1;
        token
    }

    fn or_expr<T: Ord>(&mut self, leaf: &impl Fn(&str, Option<&str>) -> T) -> Option<T> {
        let mut result = self.and_expr(leaf)?;
        while self.peek() == Some("or") {
            self.pos += 1;
            result = result.max(self.and_expr(leaf)?);
        }
        Some(result)
    }

    fn and_expr<T: Ord>(&mut self, leaf: &impl Fn(&str, Option<&str>) -> T) -> Option<T> {
        let mut result = self.primary(leaf)?;
        while self.peek() == Some("and") {
            self.pos += 1;
            result = result.min(self.primary(leaf)?);
        }
        Some(result)
    }

    fn primary<T: Ord>(&mut self, leaf: &impl Fn(&str, Option<&str>) -> T) -> Option<T> {
        match self.next()? {
            "(" => {
                let result = self.or_expr(leaf)?;
                if self.next()? != ")" {
                    return None;
                }
                Some(result)
            }
            "or" | "and" | "with" | ")" => None,
            id => {
                let id = id.trim_end_matches('+').to_owned();
                if self.peek() != Some("with") {
                    return Some(leaf(&id, None));
                }
                self.pos += 1;
                // Debian writes exceptions as several words (`GPL-2+ with OpenSSL exception`).
                let mut words = Vec::new();
                while let Some(word) = self.peek()
                    && !matches!(word, "or" | "and" | "with" | "(" | ")")
                {
                    words.push(word.to_owned());
                    self.pos += 1;
                }
                if words.is_empty() {
                    return None;
                }
                Some(leaf(&id, Some(&words.join("-"))))
            }
        }
    }
}

/// How freely code under a license can be used, from least to most free. A `Permissive` file
/// can be trained on and its completions used anywhere, as the Rust corpus requires.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// Not recognized, or not open source: unknown names, custom terms, non-free licenses,
    /// advertising clauses, and files without any license information.
    Other,
    /// Strong copyleft: GPL, AGPL, EUPL and the like.
    Copyleft,
    /// File- or library-scoped copyleft: LGPL, MPL, EPL, CDDL, Artistic, and GPL with a
    /// linking exception.
    WeakCopyleft,
    Permissive,
}

impl Tier {
    pub const ALL: [Tier; 4] = [
        Tier::Permissive,
        Tier::WeakCopyleft,
        Tier::Copyleft,
        Tier::Other,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tier::Other => "other",
            Tier::Copyleft => "copyleft",
            Tier::WeakCopyleft => "weak_copyleft",
            Tier::Permissive => "permissive",
        }
    }
}

/// Permissive ids and families beyond `PERMISSIVE`, in the spellings Debian uses. Matched as
/// prefixes, so `bsd-3-clause` also covers `bsd-3-clause-lbnl`.
const PERMISSIVE_PREFIXES: &[&str] = &[
    "0bsd",
    "afl",
    "apache",
    "boost",
    "bsd-2",
    "bsd-3",
    "bsl-1",
    "cc0",
    "cecill-b",
    "curl",
    "ecl-2",
    "expat",
    "fsfap",
    "fsful",
    "hpnd",
    "icu",
    "ijg",
    "isc",
    "libpng",
    "mit",
    "ms-pl",
    "ncsa",
    "openssl",
    "permissive",
    "all-permissive",
    "psf",
    "public-domain",
    "publicdomain",
    "python",
    "ruby",
    "sgi-b",
    "ssleay",
    "tcl",
    "unicode",
    "unlicense",
    "upl",
    "w3c",
    "wtfpl",
    "x11",
    "zlib",
    "zpl",
];

const WEAK_COPYLEFT_PREFIXES: &[&str] = &[
    "apsl", "artistic", "cddl", "cecill-c", "cpl", "ecos", "epl", "ipl", "lgpl", "lppl", "mpl",
    "ms-rl", "perl",
];

const COPYLEFT_PREFIXES: &[&str] = &["agpl", "cecill", "eupl", "gfdl", "gpl", "osl", "sspl"];

/// Exceptions that let GPL code be linked into anything, which makes it behave like weak
/// copyleft. Matched as substrings of the exception name.
const LINKING_EXCEPTIONS: &[&str] = &["classpath", "gcc", "runtime", "link", "font"];

/// Rates one SPDX or Debian license id, as `evaluate` passes it.
pub fn tier_of(id: &str, exception: Option<&str>) -> Tier {
    let id = id
        .strip_suffix("-or-later")
        .or_else(|| id.strip_suffix("-only"))
        .unwrap_or(id);
    let prefixed = |list: &[&str]| list.iter().any(|p| id.starts_with(p));
    let cc_by = id.starts_with("cc-by")
        && !["-sa", "-nc", "-nd"]
            .iter()
            .any(|clause| id.contains(clause));
    let tier = if PERMISSIVE.contains(&id) || prefixed(PERMISSIVE_PREFIXES) || id == "pd" || cc_by {
        Tier::Permissive
    } else if prefixed(WEAK_COPYLEFT_PREFIXES) {
        Tier::WeakCopyleft
    } else if prefixed(COPYLEFT_PREFIXES) {
        Tier::Copyleft
    } else {
        Tier::Other
    };
    match exception {
        Some(e) if tier == Tier::Copyleft && LINKING_EXCEPTIONS.iter().any(|x| e.contains(x)) => {
            Tier::WeakCopyleft
        }
        _ => tier,
    }
}

/// Phrases that identify a license in a header comment or license text, after `words`
/// normalization. The GPL is found separately, since its name is inside the LGPL's and AGPL's.
const TEXT_FINGERPRINTS: &[(&str, Tier, &str)] = &[
    ("gnu affero general public license", Tier::Copyleft, "AGPL"),
    ("lesser general public license", Tier::WeakCopyleft, "LGPL"),
    ("library general public license", Tier::WeakCopyleft, "LGPL"),
    ("mozilla public license", Tier::WeakCopyleft, "MPL"),
    ("eclipse public license", Tier::WeakCopyleft, "EPL"),
    (
        "common development and distribution license",
        Tier::WeakCopyleft,
        "CDDL",
    ),
    ("artistic license", Tier::WeakCopyleft, "Artistic"),
    ("same terms as perl itself", Tier::WeakCopyleft, "Perl"),
    ("apache license", Tier::Permissive, "Apache"),
    (
        "permission is hereby granted free of charge",
        Tier::Permissive,
        "MIT",
    ),
    (
        "redistribution and use in source and binary forms",
        Tier::Permissive,
        "BSD",
    ),
    (
        "permission to use copy modify and or distribute this software for any purpose",
        Tier::Permissive,
        "ISC",
    ),
    (
        "permission to use copy modify and distribute this software",
        Tier::Permissive,
        "HPND",
    ),
    (
        "permission to use copy modify distribute and sell this software",
        Tier::Permissive,
        "HPND",
    ),
    (
        "altered source versions must be plainly marked",
        Tier::Permissive,
        "Zlib",
    ),
    ("boost software license", Tier::Permissive, "BSL-1.0"),
    (
        "python software foundation license",
        Tier::Permissive,
        "PSF",
    ),
    ("into the public domain", Tier::Permissive, "public-domain"),
    (
        "placed in the public domain",
        Tier::Permissive,
        "public-domain",
    ),
    ("is in the public domain", Tier::Permissive, "public-domain"),
    (
        "dedicated to the public domain",
        Tier::Permissive,
        "public-domain",
    ),
    (
        "are permitted in any medium without royalty",
        Tier::Permissive,
        "FSFAP",
    ),
    (
        "gives unlimited permission to copy and or distribute",
        Tier::Permissive,
        "FSFUL",
    ),
];

/// Lowercases text and collapses everything but letters and digits into single spaces, so
/// phrases match across comment markers and line wrapping.
pub fn words(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push(' ');
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

/// The licenses whose text or name appears in `text`, each once, in `TEXT_FINGERPRINTS`
/// order with the GPL last.
pub fn from_text(text: &str) -> Vec<(Tier, &'static str)> {
    fingerprints(&words(text))
}

/// Rates the license a file's header comment states, with the names found. A header that
/// names several licenses offers them as alternatives when it says so ("alternatively",
/// "dual licensed", or Perl's "the GNU General Public License or the Artistic License"), and
/// is otherwise rated by the most restrictive.
pub fn from_header(head: &str) -> Option<(Tier, String)> {
    let text = words(head);
    let found = fingerprints(&text);
    let tiers = found.iter().map(|&(t, _)| t);
    let alternatives = text.contains(" alternatively ")
        || text.contains(" dual licensed ")
        || text.contains(" dual license ")
        || text.contains(" general public license or the artistic license ");
    let tier = if alternatives {
        tiers.max()
    } else {
        tiers.min()
    }?;
    let names: Vec<&str> = found.iter().map(|&(_, n)| n).collect();
    Some((tier, names.join(", ")))
}

fn fingerprints(text: &str) -> Vec<(Tier, &'static str)> {
    let mut found = Vec::new();
    for &(phrase, tier, name) in TEXT_FINGERPRINTS {
        if !text.contains(&format!(" {phrase} ")) {
            continue;
        }
        let found_one = if name == "BSD" && text.contains(" all advertising materials mentioning ")
        {
            // The four-clause BSD license's advertising clause.
            (Tier::Other, "BSD-4-clause")
        } else {
            (tier, name)
        };
        if !found.contains(&found_one) {
            found.push(found_one);
        }
    }
    let gpl = text
        .match_indices(" general public license ")
        .any(|(i, _)| {
            let before = &text[..i];
            !["lesser", "library", "affero"]
                .iter()
                .any(|w| before.ends_with(w))
        });
    if gpl {
        // Exceptions that allow linking the code into anything (OpenJDK, libstdc++, libgcc).
        let linking = text.contains(" classpath exception ")
            || text.contains(" runtime library exception ")
            || (text.contains(" special exception ") && text.contains(" link"));
        found.push(if linking {
            (Tier::WeakCopyleft, "GPL with linking exception")
        } else {
            (Tier::Copyleft, "GPL")
        });
    }
    found
}

/// The expression after `SPDX-License-Identifier:` in the first lines of a file.
pub fn spdx_identifier(content: &str) -> Option<&str> {
    const TAG: &str = "SPDX-License-Identifier:";
    let head = &content[..content.floor_char_boundary(content.len().min(4096))];
    let start = head.find(TAG)? + TAG.len();
    let line = head[start..].lines().next()?;
    let line = line.split("*/").next()?.split("-->").next()?;
    let expr = line
        .trim()
        .trim_matches(|c: char| matches!(c, '"' | '\'' | ';' | ','));
    (!expr.is_empty()).then_some(expr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(expr: &str) -> bool {
        allowed(Some(expr), Policy::Permissive)
    }

    #[test]
    fn expressions() {
        assert!(ok("MIT"));
        assert!(ok("MIT OR Apache-2.0"));
        assert!(ok("MIT/Apache-2.0"));
        assert!(ok("Apache-2.0 WITH LLVM-exception"));
        assert!(ok("(MIT OR Apache-2.0) AND Unicode-DFS-2016"));
        assert!(ok("GPL-3.0 OR MIT"));
        assert!(!ok("GPL-3.0"));
        assert!(!ok("MIT AND GPL-3.0"));
        assert!(!ok("MPL-2.0"));
        assert!(!ok("Apache 2.0"));
        assert!(!ok(""));
        assert!(!allowed(None, Policy::Permissive));
        assert!(allowed(None, Policy::Any));
        assert!(!ok("MIT WITH"));
        assert!(!ok("Apache-2.0 WITH OR MIT"));
    }

    fn tier(expr: &str) -> Option<Tier> {
        evaluate(expr, tier_of)
    }

    #[test]
    fn debian_tiers() {
        assert_eq!(tier("Expat"), Some(Tier::Permissive));
        assert_eq!(tier("BSD-3-clause"), Some(Tier::Permissive));
        assert_eq!(tier("public-domain"), Some(Tier::Permissive));
        assert_eq!(tier("GPL-2+"), Some(Tier::Copyleft));
        assert_eq!(tier("GPL-2.0-or-later"), Some(Tier::Copyleft));
        assert_eq!(tier("LGPL-2.1+"), Some(Tier::WeakCopyleft));
        assert_eq!(tier("GPL-1+ or Artistic"), Some(Tier::WeakCopyleft));
        assert_eq!(
            tier("MPL-1.1 or GPL-2+ or LGPL-2.1+"),
            Some(Tier::WeakCopyleft)
        );
        assert_eq!(tier("BSD-3-clause and GPL-2+"), Some(Tier::Copyleft));
        assert_eq!(
            tier("Apache-2.0 with LLVM exception"),
            Some(Tier::Permissive)
        );
        assert_eq!(
            tier("GPL-3+ with GCC-Runtime-Library exception"),
            Some(Tier::WeakCopyleft)
        );
        assert_eq!(tier("GPL-2+ with OpenSSL exception"), Some(Tier::Copyleft));
        assert_eq!(tier("BSD-4-clause"), Some(Tier::Other));
        assert_eq!(tier("CC-BY-SA-3.0"), Some(Tier::Other));
        assert_eq!(tier("MIT and custom-thing"), Some(Tier::Other));
        assert_eq!(tier("GPL-2+ with"), None);
    }

    #[test]
    fn license_text() {
        let lgpl = "/* This library is free software; you can redistribute it and/or\n * modify it under the terms of the GNU Lesser General Public\n * License. You should have received a copy of the GNU Lesser General\n * Public License along with this library. */";
        assert_eq!(from_text(lgpl), vec![(Tier::WeakCopyleft, "LGPL")]);
        let gpl = "# under the terms of the GNU General Public License as published by";
        assert_eq!(from_text(gpl), vec![(Tier::Copyleft, "GPL")]);
        let mit = "// Permission is hereby granted, free of charge, to any person obtaining";
        assert_eq!(from_text(mit), vec![(Tier::Permissive, "MIT")]);
        let bsd4 = " * Redistribution and use in source and binary forms, with or without\n * 3. All advertising materials mentioning features";
        assert_eq!(from_text(bsd4), vec![(Tier::Other, "BSD-4-clause")]);
        assert!(from_text("int main() { return 0; }").is_empty());
        let classpath = " * under the terms of the GNU General Public License version 2 only\n * Oracle designates this particular file as subject to the \"Classpath\"\n * exception as provided by Oracle in the LICENSE file";
        assert_eq!(
            from_text(classpath),
            vec![(Tier::WeakCopyleft, "GPL with linking exception")]
        );
    }

    #[test]
    fn headers() {
        let qt = "** GNU Lesser General Public License Usage\n** Alternatively, this file may be used under the terms of the GNU Lesser\n** General Public License version 3 ...\n** GNU General Public License Usage\n** Alternatively, this file may be used under the terms of the GNU\n** General Public License version 2.0 or (at your option) the GNU General\n** Public license version 3";
        assert_eq!(from_header(qt).unwrap().0, Tier::WeakCopyleft);
        let perl = "# You may distribute under the terms of either the GNU General Public\n# License or the Artistic License, as specified in the README file.";
        assert_eq!(from_header(perl).unwrap().0, Tier::WeakCopyleft);
        // Without a statement that they are alternatives, the most restrictive applies.
        let mixed = "/* Based on public domain code. This program is free software under\n * the terms of the GNU General Public License. */";
        assert_eq!(from_header(mixed).unwrap().0, Tier::Copyleft);
        assert_eq!(from_header("int x;"), None);
    }

    #[test]
    fn spdx_headers() {
        assert_eq!(
            spdx_identifier("// SPDX-License-Identifier: GPL-2.0-only\nint x;\n"),
            Some("GPL-2.0-only")
        );
        assert_eq!(
            spdx_identifier("/* SPDX-License-Identifier: MIT OR Apache-2.0 */\n"),
            Some("MIT OR Apache-2.0")
        );
        assert_eq!(spdx_identifier("int x;\n"), None);
    }
}
