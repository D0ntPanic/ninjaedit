//! Reads `debian/copyright` files to find the license of each file in a source package.
//!
//! Machine-readable files (DEP-5) have `Files:` stanzas of glob patterns, each with a
//! `License:` expression; the last stanza that matches a path applies. Stand-alone `License:`
//! stanzas give the text behind custom short names. Free-form files are only searched for
//! license texts, which gives one conservative answer for the whole package.

use crate::license::{self, Tier};
use std::collections::HashMap;

pub struct Copyright {
    kind: Kind,
    stanzas: Vec<Stanza>,
    /// For free-form files: the most restrictive license found in the text.
    fallback: Option<(Tier, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dep5,
    FreeForm,
    Missing,
}

struct Stanza {
    patterns: Vec<Pattern>,
    license: String,
    tier: Tier,
}

struct Pattern {
    glob: Vec<u8>,
    /// The literal bytes before the first wildcard, checked before the full match.
    prefix_len: usize,
}

impl Copyright {
    pub fn missing() -> Copyright {
        Copyright {
            kind: Kind::Missing,
            stanzas: Vec::new(),
            fallback: None,
        }
    }

    pub fn parse(text: &str) -> Copyright {
        let paragraphs = paragraphs(text);
        let dep5 = paragraphs.first().is_some_and(|p| {
            field(p, "format").is_some_and(|f| {
                let f = f.to_ascii_lowercase();
                f.contains("copyright-format") || f.contains("dep5") || f.contains("dep-5")
            })
        });
        if !dep5 {
            let found = license::from_text(text);
            let fallback = found.iter().map(|&(tier, _)| tier).min().map(|tier| {
                let names: Vec<&str> = found.iter().map(|&(_, n)| n).collect();
                (tier, names.join(" and "))
            });
            return Copyright {
                kind: Kind::FreeForm,
                stanzas: Vec::new(),
                fallback,
            };
        }

        // Stand-alone license paragraphs: short name to text.
        let mut texts: HashMap<String, String> = HashMap::new();
        for p in &paragraphs {
            if field(p, "files").is_none()
                && let Some(value) = field(p, "license")
            {
                let (name, body) = value.split_once('\n').unwrap_or((value, ""));
                texts.insert(name.trim().to_ascii_lowercase(), body.to_owned());
            }
        }
        let mut tiers: HashMap<String, Tier> = HashMap::new();
        let mut stanzas = Vec::new();
        for p in &paragraphs {
            let (Some(files), Some(value)) = (field(p, "files"), field(p, "license")) else {
                continue;
            };
            let (expr, inline_text) = value.split_once('\n').unwrap_or((value, ""));
            let expr = expr.trim().to_owned();
            let tier = *tiers
                .entry(expr.clone())
                .or_insert_with(|| rate(&expr, inline_text, &texts));
            stanzas.push(Stanza {
                patterns: files.split_whitespace().map(Pattern::new).collect(),
                license: expr,
                tier,
            });
        }
        Copyright {
            kind: Kind::Dep5,
            stanzas,
            fallback: None,
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The license of a file at `path`, relative to the source root: its tier and the
    /// license expression or names it was rated from.
    pub fn lookup(&self, path: &str) -> Option<(Tier, &str)> {
        if self.kind != Kind::Dep5 {
            return self.fallback.as_ref().map(|(t, n)| (*t, n.as_str()));
        }
        let path = path.as_bytes();
        self.stanzas
            .iter()
            .rev()
            .find(|s| s.patterns.iter().any(|p| p.matches(path)))
            .map(|s| (s.tier, s.license.as_str()))
    }
}

/// Rates a license expression. A name that is not a known license is rated by its text, from
/// the stanza itself or a stand-alone paragraph.
fn rate(expr: &str, inline_text: &str, texts: &HashMap<String, String>) -> Tier {
    // Debian sometimes separates licenses that both apply with commas, or puts a comma
    // before the operator.
    let normalized = expr
        .to_ascii_lowercase()
        .replace(", and ", " and ")
        .replace(", or ", " or ")
        .replace(',', " and ");
    license::evaluate(&normalized, |id, exception| {
        let tier = license::tier_of(id, exception);
        if tier != Tier::Other {
            return tier;
        }
        let text = texts.get(id).map(String::as_str).unwrap_or(inline_text);
        // A single license's text: any alternatives it mentions are offered, not imposed.
        license::from_text(text)
            .iter()
            .map(|&(t, _)| t)
            .max()
            .unwrap_or(Tier::Other)
    })
    .unwrap_or(Tier::Other)
}

impl Pattern {
    fn new(raw: &str) -> Pattern {
        let raw = raw.strip_prefix("./").unwrap_or(raw);
        let mut glob = raw.as_bytes().to_vec();
        // A directory, written as `dir/` or just `dir`, covers everything below it. `dir`
        // gets both forms through the alternative in `matches`.
        if glob.ends_with(b"/") {
            glob.push(b'*');
        }
        let prefix_len = glob
            .iter()
            .position(|&c| matches!(c, b'*' | b'?' | b'\\'))
            .unwrap_or(glob.len());
        Pattern { glob, prefix_len }
    }

    fn matches(&self, path: &[u8]) -> bool {
        if !path.starts_with(&self.glob[..self.prefix_len]) {
            return false;
        }
        if self.prefix_len == self.glob.len() {
            // No wildcards: the file itself, or a directory containing it.
            return path.len() == self.glob.len()
                || (path[self.glob.len()] == b'/' && !self.glob.is_empty());
        }
        glob_match(&self.glob, path)
    }
}

/// DEP-5 globs: `*` matches any run of characters including `/`, `?` any one character, and
/// a backslash escapes the next character.
fn glob_match(glob: &[u8], text: &[u8]) -> bool {
    let (mut g, mut t) = (0, 0);
    // Where to resume after the last `*`: the glob position after it, and the text position
    // it currently matches up to.
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match glob.get(g) {
            Some(b'*') => {
                star = Some((g + 1, t));
                g += 1;
                continue;
            }
            Some(b'?') => {
                g += 1;
                t += 1;
                continue;
            }
            Some(&c) => {
                let (lit, width) = if c == b'\\' && g + 1 < glob.len() {
                    (glob[g + 1], 2)
                } else {
                    (c, 1)
                };
                if lit == text[t] {
                    g += width;
                    t += 1;
                    continue;
                }
            }
            None => {}
        }
        match star {
            Some((sg, st)) => {
                g = sg;
                t = st + 1;
                star = Some((sg, st + 1));
            }
            None => return false,
        }
    }
    glob[g..].iter().all(|&c| c == b'*')
}

/// Splits a control file into paragraphs of (lowercased field name, value) pairs.
/// Continuation lines join the value with newlines; a lone `.` stands for a blank line.
fn paragraphs(text: &str) -> Vec<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut current: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = current.last_mut() {
                value.push('\n');
                let cont = line.trim();
                if cont != "." {
                    value.push_str(cont);
                }
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            current.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn field<'a>(paragraph: &'a [(String, String)], name: &str) -> Option<&'a str> {
    paragraph
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEP5: &str = "\
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: example

Files: *
Copyright: 2020 Someone
License: GPL-2+

Files: lib/*.c
 compat/getopt.c
Copyright: 2019 Other
License: Expat

Files: lib/strange.c
Copyright: 2019 Other
License: Strange
 Redistribution and use in source and binary forms, with or without
 modification, are permitted.

Files: vendor
Copyright: 2019 Other
License: Custom-BSD

Files: mixed/*
License: MPL-1.1 or LGPL-2.1+, and Unknown-Thing

Files: debian/*
License: GPL-2+

License: Custom-BSD
 Permission is hereby granted, free of charge, to any person obtaining a copy
";

    #[test]
    fn dep5_lookup() {
        let c = Copyright::parse(DEP5);
        assert_eq!(c.kind(), Kind::Dep5);
        let tier = |p: &str| c.lookup(p).map(|(t, _)| t);
        assert_eq!(c.lookup("src/main.c"), Some((Tier::Copyleft, "GPL-2+")));
        assert_eq!(tier("lib/a.c"), Some(Tier::Permissive));
        assert_eq!(tier("lib/sub/b.c"), Some(Tier::Permissive));
        assert_eq!(tier("lib/a.h"), Some(Tier::Copyleft));
        assert_eq!(tier("compat/getopt.c"), Some(Tier::Permissive));
        assert_eq!(tier("lib/strange.c"), Some(Tier::Permissive));
        assert_eq!(tier("vendor/x/y.c"), Some(Tier::Permissive));
        assert_eq!(tier("vendorx/y.c"), Some(Tier::Copyleft));
        assert_eq!(tier("mixed/a.c"), Some(Tier::WeakCopyleft));
    }

    #[test]
    fn free_form() {
        let c = Copyright::parse(
            "This package was debianized by someone.\n\nIt is licensed under the GNU Lesser General Public License, and parts under the MIT license: Permission is hereby granted, free of charge, to any person\n",
        );
        assert_eq!(c.kind(), Kind::FreeForm);
        let (tier, name) = c.lookup("anything.c").unwrap();
        assert_eq!(tier, Tier::WeakCopyleft);
        assert_eq!(name, "LGPL and MIT");
    }

    #[test]
    fn globs() {
        assert!(glob_match(b"*", b"a/b/c"));
        assert!(glob_match(b"src/*.c", b"src/x/y.c"));
        assert!(!glob_match(b"src/*.c", b"src/x/y.h"));
        assert!(glob_match(b"a?c", b"abc"));
        assert!(glob_match(b"*foo*bar", b"xfooybar"));
        assert!(!glob_match(b"*foo*bar", b"xfooybarz"));
        assert!(glob_match(b"a\\*b", b"a*b"));
        assert!(!glob_match(b"a\\*b", b"axb"));
    }
}
