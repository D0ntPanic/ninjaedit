//! The files a commit changed, arranged as a tree of directories, for
//! listing them with the long shared prefixes of their paths folded
//! into directory rows that can be collapsed.
//!
//! A [`FileTree`] is built from the paths in the order given (git lists
//! them sorted) and keeps that order within each directory, directories
//! first. A run of directories with nothing in them but the next
//! directory is one row, `core/src/git`, as file browsers show them.
//! [`FileTree::rows`] flattens the tree for display given which
//! directories are collapsed; the caller keeps that state, one flag per
//! directory by its index in [`dirs`](FileTree::dirs), and the tree
//! numbers its directories in the order they are first shown, parents
//! before children, so the flags stay put as directories fold and
//! unfold.

use std::collections::BTreeMap;

/// A directory of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dir {
    /// The directory's name, or the names of the run of directories it
    /// stands for joined with `/`.
    pub label: String,
    /// The directory's path from the root of the tree.
    pub path: String,
    /// The directory holding it, by index, if any.
    pub parent: Option<usize>,
    /// What it holds, directories first.
    pub entries: Vec<Entry>,
}

/// One thing a directory holds: a directory or a file, each by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Dir(usize),
    /// A file, by its index in the paths the tree was built from.
    File(usize),
}

/// One row of the flattened tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeRow {
    Dir {
        dir: usize,
        depth: usize,
        collapsed: bool,
    },
    File {
        file: usize,
        depth: usize,
        /// The directory the file is in, if not the root.
        parent: Option<usize>,
    },
}

impl TreeRow {
    /// How deep the row is nested.
    pub fn depth(self) -> usize {
        match self {
            TreeRow::Dir { depth, .. } | TreeRow::File { depth, .. } => depth,
        }
    }
}

#[derive(Default)]
struct Node {
    dirs: BTreeMap<String, Node>,
    files: Vec<(String, usize)>,
}

/// The paths of a commit's files as a tree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileTree {
    dirs: Vec<Dir>,
    /// The entries at the root.
    roots: Vec<Entry>,
}

impl FileTree {
    /// Build the tree of `paths`, each with `/` between its parts.
    pub fn new<'a>(paths: impl IntoIterator<Item = &'a str>) -> FileTree {
        let mut root = Node::default();
        for (index, path) in paths.into_iter().enumerate() {
            let mut parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
            let Some(name) = parts.pop() else {
                continue;
            };
            let mut node = &mut root;
            for part in parts {
                node = node.dirs.entry(part.to_owned()).or_default();
            }
            node.files.push((name.to_owned(), index));
        }
        let mut tree = FileTree::default();
        tree.roots = tree.build(&root, "", None);
        tree
    }

    /// The entries of `node`, adding its directories to the tree.
    fn build(&mut self, node: &Node, prefix: &str, parent: Option<usize>) -> Vec<Entry> {
        let mut entries = Vec::new();
        for (name, child) in &node.dirs {
            // A directory holding only one directory is shown with it.
            let mut label = name.clone();
            let mut current = child;
            while current.files.is_empty() && current.dirs.len() == 1 {
                let (name, next) = current.dirs.iter().next().expect("one entry");
                label.push('/');
                label.push_str(name);
                current = next;
            }
            let path = format!("{prefix}{label}");
            let index = self.dirs.len();
            self.dirs.push(Dir {
                label,
                path: path.clone(),
                parent,
                entries: Vec::new(),
            });
            let children = self.build(current, &format!("{path}/"), Some(index));
            self.dirs[index].entries = children;
            entries.push(Entry::Dir(index));
        }
        entries.extend(node.files.iter().map(|(_, index)| Entry::File(*index)));
        entries
    }

    /// The directories, parents before children.
    pub fn dirs(&self) -> &[Dir] {
        &self.dirs
    }

    /// The rows to show, skipping what is inside a collapsed directory.
    /// `collapsed` has one flag per directory; a directory it doesn't
    /// cover is open.
    pub fn rows(&self, collapsed: &[bool]) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        self.push_rows(&self.roots, 0, None, collapsed, &mut rows);
        rows
    }

    fn push_rows(
        &self,
        entries: &[Entry],
        depth: usize,
        parent: Option<usize>,
        collapsed: &[bool],
        rows: &mut Vec<TreeRow>,
    ) {
        for entry in entries {
            match *entry {
                Entry::Dir(dir) => {
                    let is_collapsed = collapsed.get(dir).copied().unwrap_or(false);
                    rows.push(TreeRow::Dir {
                        dir,
                        depth,
                        collapsed: is_collapsed,
                    });
                    if !is_collapsed {
                        self.push_rows(
                            &self.dirs[dir].entries,
                            depth + 1,
                            Some(dir),
                            collapsed,
                            rows,
                        );
                    }
                }
                Entry::File(file) => rows.push(TreeRow::File {
                    file,
                    depth,
                    parent,
                }),
            }
        }
    }

    /// Every file under a directory, at any depth, in tree order.
    pub fn files_under(&self, dir: usize) -> Vec<usize> {
        let mut files = Vec::new();
        self.collect_files(&self.dirs[dir].entries, &mut files);
        files
    }

    fn collect_files(&self, entries: &[Entry], files: &mut Vec<usize>) {
        for entry in entries {
            match *entry {
                Entry::Dir(dir) => self.collect_files(&self.dirs[dir].entries, files),
                Entry::File(file) => files.push(file),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn describe(tree: &FileTree, collapsed: &[bool], names: &[&str]) -> Vec<String> {
        tree.rows(collapsed)
            .iter()
            .map(|row| {
                let indent = "  ".repeat(row.depth());
                match *row {
                    TreeRow::Dir { dir, collapsed, .. } => format!(
                        "{indent}{} {}",
                        if collapsed { ">" } else { "v" },
                        tree.dirs()[dir].label
                    ),
                    TreeRow::File { file, .. } => {
                        format!("{indent}{}", names[file].rsplit('/').next().unwrap())
                    }
                }
            })
            .collect()
    }

    #[test]
    fn directories_come_first_and_lone_chains_fold_into_one_row() {
        let paths = [
            "Cargo.toml",
            "core/src/git/diff.rs",
            "core/src/git/mod.rs",
            "core/src/lib.rs",
            "tui/src/app.rs",
        ];
        let tree = FileTree::new(paths);
        assert_eq!(
            describe(&tree, &[], &paths),
            [
                "v core/src",
                "  v git",
                "    diff.rs",
                "    mod.rs",
                "  lib.rs",
                "v tui/src",
                "  app.rs",
                "Cargo.toml",
            ]
        );
        let labels: Vec<(&str, &str, Option<usize>)> = tree
            .dirs()
            .iter()
            .map(|d| (d.label.as_str(), d.path.as_str(), d.parent))
            .collect();
        assert_eq!(
            labels,
            [
                ("core/src", "core/src", None),
                ("git", "core/src/git", Some(0)),
                ("tui/src", "tui/src", None),
            ]
        );
        assert_eq!(tree.files_under(0), vec![1, 2, 3]);
        assert_eq!(tree.files_under(1), vec![1, 2]);
        let rows = tree.rows(&[]);
        assert_eq!(
            rows[2],
            TreeRow::File {
                file: 1,
                depth: 2,
                parent: Some(1)
            }
        );
        assert_eq!(
            rows[7],
            TreeRow::File {
                file: 0,
                depth: 0,
                parent: None
            }
        );
    }

    #[test]
    fn collapsed_directories_hide_their_contents() {
        let paths = ["a/b/c.rs", "a/d.rs", "e.rs"];
        let tree = FileTree::new(paths);
        assert_eq!(
            describe(&tree, &[false, true], &paths),
            ["v a", "  > b", "  d.rs", "e.rs"]
        );
        assert_eq!(describe(&tree, &[true], &paths), ["> a", "e.rs"]);
        // Flags beyond the directories, or missing, are harmless.
        assert_eq!(
            describe(&tree, &[false, false, true, true], &paths).len(),
            5
        );
    }

    #[test]
    fn odd_paths() {
        let tree = FileTree::new(["", "/leading.rs", "dir//double.rs"]);
        assert_eq!(tree.rows(&[]).len(), 3);
        assert_eq!(tree.dirs().len(), 1);
        assert_eq!(tree.dirs()[0].path, "dir");
    }
}
