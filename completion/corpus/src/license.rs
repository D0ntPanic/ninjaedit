//! Evaluates SPDX license expressions from Cargo manifests against an allow-list.

use clap::ValueEnum;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Policy {
    /// Only permissive licenses (MIT, Apache-2.0, BSD, ISC, Zlib, Unlicense, CC0, ...).
    Permissive,
    /// Accept every crate regardless of license, including crates with none declared.
    Any,
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
        Policy::Permissive => expr.is_some_and(|e| Parser::new(e).evaluate()),
    }
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

    fn evaluate(mut self) -> bool {
        let Some(result) = self.or_expr() else {
            return false;
        };
        result && self.pos == self.tokens.len()
    }

    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.pos).map(String::as_str)
    }

    fn next(&mut self) -> Option<&str> {
        let token = self.tokens.get(self.pos).map(String::as_str);
        self.pos += 1;
        token
    }

    fn or_expr(&mut self) -> Option<bool> {
        let mut result = self.and_expr()?;
        while self.peek() == Some("or") {
            self.pos += 1;
            result |= self.and_expr()?;
        }
        Some(result)
    }

    fn and_expr(&mut self) -> Option<bool> {
        let mut result = self.primary()?;
        while self.peek() == Some("and") {
            self.pos += 1;
            result &= self.primary()?;
        }
        Some(result)
    }

    fn primary(&mut self) -> Option<bool> {
        match self.next()? {
            "(" => {
                let result = self.or_expr()?;
                if self.next()? != ")" {
                    return None;
                }
                Some(result)
            }
            "or" | "and" | "with" | ")" => None,
            id => {
                let id = id.trim_end_matches('+').to_owned();
                let mut result = PERMISSIVE.contains(&id.as_str());
                if self.peek() == Some("with") {
                    self.pos += 1;
                    let exception = self.next()?;
                    result &= ALLOWED_EXCEPTIONS.contains(&exception);
                }
                Some(result)
            }
        }
    }
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
    }
}
