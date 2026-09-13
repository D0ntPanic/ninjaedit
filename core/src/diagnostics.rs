//! Source locations in the output of compilers and build tools, so a
//! frontend can turn the file and line of a warning or error into a
//! link that opens the file.
//!
//! [`find_source_links`] scans one line of output and returns every
//! location it names, as a [`SourceLink`]: where in the line the
//! location's text is (to underline it) and the [`SourceLocation`] it
//! names (to open). The formats recognized:
//!
//! * rustc's, where the location follows an arrow on its own line
//!   (`  --> src/main.rs:10:5`, or `:::` for a secondary one);
//! * gcc's and clang's, where the location begins the message
//!   (`src/main.c:10:5: error: ...`, and the `In file included from`
//!   and `from` lines above it), and the linker's
//!   (`main.c:10: undefined reference to ...`);
//! * a Rust panic (`thread 'main' panicked at src/main.rs:10:5:`) and
//!   the frames of a backtrace (`at ./src/main.rs:10:5`);
//! * CMake's (`CMake Error at CMakeLists.txt:12 (message):`).
//!
//! A location is a path, a line, and sometimes a column, all as the
//! tool printed them. Lines and columns count from one. The path is
//! usually relative to where the tool ran, which the output alone can't
//! say, so [`SourceLocation::resolve`] takes the directories it might
//! be relative to and finds the file among them.

use regex::Regex;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// A place in a source file, as a tool named it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceLocation {
    /// The path as printed, absolute or relative to wherever the tool
    /// ran.
    pub path: String,
    /// The line, counting from one.
    pub line: usize,
    /// The column, counting from one, when the tool gave one.
    pub column: Option<usize>,
}

impl SourceLocation {
    /// The file the location names, looked for as an absolute path or
    /// under each of `bases` in turn. `None` when it exists under none
    /// of them.
    pub fn resolve<'a>(&self, bases: impl IntoIterator<Item = &'a Path>) -> Option<PathBuf> {
        let path = Path::new(&self.path);
        if path.is_absolute() {
            return path.is_file().then(|| path.to_path_buf());
        }
        bases
            .into_iter()
            .map(|base| base.join(path))
            .find(|candidate| candidate.is_file())
    }
}

/// A location found in a line of output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLink {
    pub location: SourceLocation,
    /// The bytes of the line that spell the location, path through
    /// column.
    pub range: Range<usize>,
}

/// A location: a path (with a Windows drive letter, if any), a line, and
/// maybe a column. The path stops at whitespace, a colon, and the
/// quotes and brackets tools put around paths.
const LOCATION: &str =
    r"(?P<loc>(?P<path>(?:[A-Za-z]:)?[^\s:'`\x22()<>\[\]]+):(?P<line>[0-9]+)(?::(?P<col>[0-9]+))?)";

/// The contexts a location is recognized in. Each has one `loc` group.
static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // rustc: `  --> src/main.rs:10:5` and `  ::: src/lib.rs:3:1`.
        format!(r"(?:-->|:::)\s+{LOCATION}"),
        // gcc and clang: `src/main.c:10:5: error: ...`, and the linker's
        // `main.c:10: undefined reference to ...`.
        format!(r"{LOCATION}:\s*(?:fatal error|error|warning|note|remark|undefined reference)\b"),
        // gcc and clang: the chain of includes above a message.
        format!(r"(?:In file included from|^\s*from)\s+{LOCATION}[:,]"),
        // A Rust panic, and the frames of a backtrace.
        format!(r"\bat\s+{LOCATION}"),
        // CMake: `CMake Error at CMakeLists.txt:12 (message):`.
        format!(r"CMake (?:Error|Warning|Deprecation Warning)(?: \(dev\))? at {LOCATION}"),
    ]
    .iter()
    .map(|pattern| Regex::new(pattern).expect("a valid diagnostic pattern"))
    .collect()
});

/// Every source location named in one line of output, in the order
/// they appear.
pub fn find_source_links(text: &str) -> Vec<SourceLink> {
    let mut links: Vec<SourceLink> = Vec::new();
    for pattern in PATTERNS.iter() {
        for captures in pattern.captures_iter(text) {
            let (Some(loc), Some(path), Some(line)) = (
                captures.name("loc"),
                captures.name("path"),
                captures.name("line"),
            ) else {
                continue;
            };
            if !looks_like_path(path.as_str()) {
                continue;
            }
            let Ok(line) = line.as_str().parse::<usize>() else {
                continue;
            };
            let column = captures
                .name("col")
                .and_then(|col| col.as_str().parse::<usize>().ok());
            let range = loc.range();
            if links.iter().any(|l| overlaps(&l.range, &range)) {
                continue;
            }
            links.push(SourceLink {
                location: SourceLocation {
                    path: path.as_str().to_owned(),
                    line,
                    column,
                },
                range,
            });
        }
    }
    links.sort_by_key(|link| link.range.start);
    links
}

/// Whether a path-shaped token is plausibly a file: it has an extension
/// or a directory separator, which keeps `12:30:45` and `host:8080`
/// from becoming links.
fn looks_like_path(path: &str) -> bool {
    path.chars().any(|c| matches!(c, '.' | '/' | '\\')) && path.chars().any(char::is_alphabetic)
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(text: &str) -> Vec<(&str, &str, usize, Option<usize>)> {
        find_source_links(text)
            .into_iter()
            .map(|link| {
                let location = link.location;
                let path: &str = &text[link.range.clone()];
                // The text under the link is the whole location.
                assert!(path.starts_with(&location.path), "{path:?} {location:?}");
                let path_str = &text[link.range.start..link.range.start + location.path.len()];
                (path, path_str, location.line, location.column)
            })
            .collect()
    }

    #[test]
    fn rustc_arrows() {
        assert_eq!(
            links("  --> src/main.rs:10:5"),
            [("src/main.rs:10:5", "src/main.rs", 10, Some(5))]
        );
        assert_eq!(
            links("  ::: /home/me/proj/src/lib.rs:3:1"),
            [(
                "/home/me/proj/src/lib.rs:3:1",
                "/home/me/proj/src/lib.rs",
                3,
                Some(1)
            )]
        );
        // The rest of a rustc diagnostic has no locations in it.
        assert!(links("warning: unused variable: `x`").is_empty());
        assert!(links("   |").is_empty());
        assert!(links("10 |     let x = 1;").is_empty());
        assert!(links("   = note: `#[warn(unused_variables)]` on by default").is_empty());
    }

    #[test]
    fn gcc_and_clang_messages() {
        assert_eq!(
            links("src/main.c:12:5: error: expected ';' before 'return'"),
            [("src/main.c:12:5", "src/main.c", 12, Some(5))]
        );
        assert_eq!(
            links("main.cpp:7:10: warning: unused variable 'x' [-Wunused-variable]"),
            [("main.cpp:7:10", "main.cpp", 7, Some(10))]
        );
        assert_eq!(
            links("/abs/path/foo.h:3:1: fatal error: 'bar.h' file not found"),
            [("/abs/path/foo.h:3:1", "/abs/path/foo.h", 3, Some(1))]
        );
        assert_eq!(
            links("foo.c:20: note: declared here"),
            [("foo.c:20", "foo.c", 20, None)]
        );
        assert_eq!(
            links("In file included from src/main.c:2:"),
            [("src/main.c:2", "src/main.c", 2, None)]
        );
        assert_eq!(
            links("                 from include/util.h:5,"),
            [("include/util.h:5", "include/util.h", 5, None)]
        );
        assert_eq!(
            links("/usr/bin/ld: main.c:14: undefined reference to `helper'"),
            [("main.c:14", "main.c", 14, None)]
        );
        assert_eq!(
            links(r"C:\src\main.c:12:5: error: oops"),
            [(r"C:\src\main.c:12:5", r"C:\src\main.c", 12, Some(5))]
        );
        // gcc's function context line names no line.
        assert!(links("src/main.c: In function 'main':").is_empty());
        // Neither does the link failure's summary.
        assert!(links("collect2: error: ld returned 1 exit status").is_empty());
    }

    #[test]
    fn panics_and_backtraces() {
        assert_eq!(
            links("thread 'main' panicked at src/main.rs:4:5:"),
            [("src/main.rs:4:5", "src/main.rs", 4, Some(5))]
        );
        assert_eq!(
            links("             at ./src/main.rs:4:5"),
            [("./src/main.rs:4:5", "./src/main.rs", 4, Some(5))]
        );
        assert_eq!(links("   1: core::panicking::panic_fmt"), []);
    }

    #[test]
    fn cmake_messages() {
        assert_eq!(
            links("CMake Error at CMakeLists.txt:12 (message):"),
            [("CMakeLists.txt:12", "CMakeLists.txt", 12, None)]
        );
        assert_eq!(
            links("CMake Warning (dev) at cmake/Foo.cmake:3 (add_library):"),
            [("cmake/Foo.cmake:3", "cmake/Foo.cmake", 3, None)]
        );
    }

    #[test]
    fn times_hosts_and_prose_are_not_links() {
        assert!(links("[12:30:45] building").is_empty());
        assert!(links("listening at localhost:8080").is_empty());
        assert!(links("error: aborting due to 2 previous errors").is_empty());
        assert!(links("$ cargo build").is_empty());
        assert!(links("   Compiling ninjaedit v0.1.0 (/home/me/proj)").is_empty());
    }

    #[test]
    fn locations_resolve_against_bases() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("crate/src")).unwrap();
        std::fs::write(root.join("crate/src/main.rs"), "fn main() {}\n").unwrap();
        let location = SourceLocation {
            path: "src/main.rs".to_owned(),
            line: 1,
            column: None,
        };
        // Found under the second base, not the first.
        assert_eq!(
            location.resolve([root, &*root.join("crate")]),
            Some(root.join("crate/src/main.rs"))
        );
        assert_eq!(location.resolve([root]), None);
        let absolute = SourceLocation {
            path: root
                .join("crate/src/main.rs")
                .to_string_lossy()
                .into_owned(),
            line: 1,
            column: None,
        };
        assert_eq!(absolute.resolve([]), Some(root.join("crate/src/main.rs")));
        let missing = SourceLocation {
            path: root.join("nope.rs").to_string_lossy().into_owned(),
            line: 1,
            column: None,
        };
        assert_eq!(missing.resolve([root]), None);
    }
}
