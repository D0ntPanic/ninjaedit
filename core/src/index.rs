//! Background file index for a project.
//!
//! The index is built on a background thread so opening a project is instant;
//! anything that needs a complete file list (such as search) can block on
//! [`FileIndex::wait_for_primary`] until the files the user cares about are
//! indexed. Indexing runs in two phases: non-ignored directories are scanned
//! first so the interesting files are available as quickly as possible, then
//! ignored directories are filled in so the full tree can be browsed and an
//! "all files" search is possible.
//!
//! `.gitignore` files are honored at every level of the tree (ignored entries
//! are still indexed, just flagged), and a `.git` directory is treated as
//! ignored and never descended into — its object store is not useful to index.
//!
//! After the initial scan, the index keeps itself up to date by watching the
//! filesystem with the `notify` crate, so changes made outside the process
//! (other tools, agents in another terminal, git operations) are picked up
//! automatically. UI code can poll [`FileIndex::generation`] to learn when the
//! tree has changed and needs redrawing.
//!
//! An index can also be [shallow](FileIndex::shallow): it lists the root
//! directory's own entries and stops there, never descending into
//! subdirectories (and watching only the root). That's for editing files
//! in a directory that isn't a project, where crawling everything beneath
//! it, say the whole filesystem, would be far more work than was asked for.

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::event::ModifyKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const GIT_DIR: &str = ".git";
const GITIGNORE: &str = ".gitignore";

/// One entry in the indexed directory tree, as exposed to the UI.
#[derive(Clone, Debug)]
pub struct IndexEntry {
    /// The entry's file name, lossily converted for display.
    pub name: String,
    /// Absolute path of the entry.
    pub path: PathBuf,
    pub is_dir: bool,
    /// Whether the entry is excluded by gitignore rules (or is `.git`).
    pub ignored: bool,
}

struct FileNode {
    name: OsString,
    ignored: bool,
}

struct DirNode {
    name: OsString,
    ignored: bool,
    /// Whether this directory's contents have been scanned yet. A `.git`
    /// directory is marked scanned but never actually descended into.
    scanned: bool,
    dirs: Vec<DirNode>,
    files: Vec<FileNode>,
}

impl DirNode {
    fn new(name: OsString, ignored: bool) -> DirNode {
        DirNode {
            name,
            ignored,
            scanned: false,
            dirs: Vec::new(),
            files: Vec::new(),
        }
    }
}

fn find_node<'a>(mut node: &'a DirNode, rel: &Path) -> Option<&'a DirNode> {
    for component in rel.components() {
        let name = component.as_os_str();
        node = node.dirs.iter().find(|d| d.name == name)?;
    }
    Some(node)
}

fn find_node_mut<'a>(mut node: &'a mut DirNode, rel: &Path) -> Option<&'a mut DirNode> {
    for component in rel.components() {
        let name = component.as_os_str();
        node = node.dirs.iter_mut().find(|d| d.name == name)?;
    }
    Some(node)
}

struct State {
    root: DirNode,
    /// All non-ignored directories have been scanned.
    primary_done: bool,
    /// All directories, including ignored ones, have been scanned.
    full_done: bool,
}

struct Shared {
    root_path: PathBuf,
    /// Whether subdirectories are scanned at all. A shallow index only
    /// ever holds the root's own entries.
    recursive: bool,
    state: Mutex<State>,
    cond: Condvar,
    generation: AtomicU64,
}

enum Msg {
    Fs(notify::Result<notify::Event>),
    Shutdown,
}

/// A live index of the files in a project, maintained on a background thread.
pub struct FileIndex {
    shared: Arc<Shared>,
    tx: Sender<Msg>,
    // Held so the filesystem watch stays active for the index's lifetime.
    watcher: Option<RecommendedWatcher>,
    worker: Option<JoinHandle<()>>,
}

impl FileIndex {
    /// Start indexing `root` in the background and watching it for changes.
    pub fn new(root: impl Into<PathBuf>) -> FileIndex {
        FileIndex::start(root.into(), true)
    }

    /// Start indexing only the entries of `root` itself, in the
    /// background, and watching that directory (not its subdirectories)
    /// for changes. Subdirectories are listed but never scanned, so
    /// [`children`](Self::children) of one is `None` and
    /// [`files`](Self::files) holds only the root's own files.
    pub fn shallow(root: impl Into<PathBuf>) -> FileIndex {
        FileIndex::start(root.into(), false)
    }

    fn start(root: PathBuf, recursive: bool) -> FileIndex {
        let root = fs::canonicalize(&root).unwrap_or(root);
        let shared = Arc::new(Shared {
            root_path: root.clone(),
            recursive,
            state: Mutex::new(State {
                root: DirNode::new(OsString::new(), false),
                primary_done: false,
                full_done: false,
            }),
            cond: Condvar::new(),
            generation: AtomicU64::new(0),
        });
        let (tx, rx) = mpsc::channel();

        // Start watching before the initial scan so no changes are missed in
        // between; events for directories not yet scanned are handled by
        // rescanning the nearest indexed ancestor.
        let watcher_tx = tx.clone();
        let watcher = notify::recommended_watcher(move |event| {
            let _ = watcher_tx.send(Msg::Fs(event));
        })
        .and_then(|mut watcher| {
            let mode = if recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            watcher.watch(&root, mode)?;
            Ok(watcher)
        })
        .ok();

        let worker_shared = shared.clone();
        let worker = std::thread::Builder::new()
            .name("file-index".into())
            .spawn(move || Worker::new(worker_shared, rx).run())
            .ok();

        FileIndex {
            shared,
            tx,
            watcher,
            worker,
        }
    }

    /// The project root being indexed (canonicalized).
    pub fn root(&self) -> &Path {
        &self.shared.root_path
    }

    /// Whether the index descends into subdirectories, as opposed to a
    /// [shallow](Self::shallow) index of the root's own entries only.
    pub fn is_recursive(&self) -> bool {
        self.shared.recursive
    }

    /// Whether every non-ignored directory has been scanned. Searches over
    /// project files are complete once this is true.
    pub fn is_primary_complete(&self) -> bool {
        self.shared.state.lock().unwrap().primary_done
    }

    /// Whether every directory, including ignored ones, has been scanned.
    pub fn is_fully_complete(&self) -> bool {
        self.shared.state.lock().unwrap().full_done
    }

    /// Block until all non-ignored directories have been scanned. Returns
    /// false if the timeout elapsed first.
    pub fn wait_for_primary(&self, timeout: Duration) -> bool {
        self.wait(timeout, |state| state.primary_done)
    }

    /// Block until the entire tree, including ignored directories, has been
    /// scanned. Returns false if the timeout elapsed first.
    pub fn wait_for_full(&self, timeout: Duration) -> bool {
        self.wait(timeout, |state| state.full_done)
    }

    fn wait(&self, timeout: Duration, done: impl Fn(&State) -> bool) -> bool {
        self.shared.wait(timeout, done)
    }

    /// A counter that increases every time the indexed tree changes. The UI
    /// can poll this cheaply to know when to refresh.
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }

    /// The entries of one indexed directory (directories first, then files,
    /// each sorted by name). `dir` may be absolute or relative to the root.
    /// Returns None if the directory is unknown or not yet scanned.
    pub fn children(&self, dir: impl AsRef<Path>) -> Option<Vec<IndexEntry>> {
        let dir = dir.as_ref();
        let rel = if dir.is_absolute() {
            dir.strip_prefix(&self.shared.root_path).ok()?
        } else {
            dir
        };
        let abs = self.shared.root_path.join(rel);
        let state = self.shared.state.lock().unwrap();
        let node = find_node(&state.root, rel)?;
        if !node.scanned {
            return None;
        }
        let mut entries = Vec::with_capacity(node.dirs.len() + node.files.len());
        for child in &node.dirs {
            entries.push(IndexEntry {
                name: child.name.to_string_lossy().into_owned(),
                path: abs.join(&child.name),
                is_dir: true,
                ignored: child.ignored,
            });
        }
        for child in &node.files {
            entries.push(IndexEntry {
                name: child.name.to_string_lossy().into_owned(),
                path: abs.join(&child.name),
                is_dir: false,
                ignored: child.ignored,
            });
        }
        Some(entries)
    }

    /// Absolute paths of all indexed files. With `include_ignored` set, files
    /// covered by gitignore rules are included as well (though never the
    /// contents of `.git`, which is not descended into).
    pub fn files(&self, include_ignored: bool) -> Vec<PathBuf> {
        self.shared.files(include_ignored)
    }

    /// A handle to the index's file list that can be sent to another
    /// thread, for work that waits on the index in the background.
    pub fn file_list(&self) -> FileList {
        FileList {
            shared: Arc::clone(&self.shared),
        }
    }
}

/// A handle to a [`FileIndex`]'s file list, cheap to clone and to send to
/// another thread. It lives independently of the index (a background job
/// holding one keeps the indexed tree alive, though not the watcher or
/// the indexing thread).
#[derive(Clone)]
pub struct FileList {
    shared: Arc<Shared>,
}

impl FileList {
    /// See [`FileIndex::wait_for_primary`].
    pub fn wait_for_primary(&self, timeout: Duration) -> bool {
        self.shared.wait(timeout, |state| state.primary_done)
    }

    /// See [`FileIndex::files`].
    pub fn files(&self, include_ignored: bool) -> Vec<PathBuf> {
        self.shared.files(include_ignored)
    }
}

impl Shared {
    fn wait(&self, timeout: Duration, done: impl Fn(&State) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock().unwrap();
        loop {
            if done(&state) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (guard, _) = self.cond.wait_timeout(state, deadline - now).unwrap();
            state = guard;
        }
    }

    fn files(&self, include_ignored: bool) -> Vec<PathBuf> {
        fn walk(node: &DirNode, path: &Path, include_ignored: bool, out: &mut Vec<PathBuf>) {
            for file in &node.files {
                if include_ignored || !file.ignored {
                    out.push(path.join(&file.name));
                }
            }
            for dir in &node.dirs {
                if include_ignored || !dir.ignored {
                    walk(dir, &path.join(&dir.name), include_ignored, out);
                }
            }
        }
        let state = self.state.lock().unwrap();
        let mut out = Vec::new();
        walk(&state.root, &self.root_path, include_ignored, &mut out);
        out
    }
}

impl Drop for FileIndex {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        self.watcher = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Worker {
    shared: Arc<Shared>,
    rx: Receiver<Msg>,
    /// Non-ignored directories, scanned before anything in `deferred`.
    priority: VecDeque<PathBuf>,
    /// Ignored directories, scanned only once the priority queue is empty.
    deferred: VecDeque<PathBuf>,
    queued: HashSet<PathBuf>,
    /// Cached matcher for each directory that contains a `.gitignore`.
    gitignores: HashMap<PathBuf, Option<Arc<Gitignore>>>,
}

impl Worker {
    fn new(shared: Arc<Shared>, rx: Receiver<Msg>) -> Worker {
        Worker {
            shared,
            rx,
            priority: VecDeque::new(),
            deferred: VecDeque::new(),
            queued: HashSet::new(),
            gitignores: HashMap::new(),
        }
    }

    fn run(mut self) {
        let root = self.shared.root_path.clone();
        self.enqueue(root, false);
        loop {
            while let Ok(msg) = self.rx.try_recv() {
                if !self.handle(msg) {
                    return;
                }
            }
            let next = self
                .priority
                .pop_front()
                .or_else(|| self.deferred.pop_front());
            match next {
                Some(dir) => {
                    self.queued.remove(&dir);
                    self.scan_dir(&dir);
                    self.update_flags();
                }
                None => {
                    self.update_flags();
                    match self.rx.recv() {
                        Ok(msg) => {
                            if !self.handle(msg) {
                                return;
                            }
                        }
                        Err(_) => return,
                    }
                }
            }
        }
    }

    /// Returns false when the worker should shut down.
    fn handle(&mut self, msg: Msg) -> bool {
        let event = match msg {
            Msg::Shutdown => return false,
            Msg::Fs(Err(_)) => return true,
            Msg::Fs(Ok(event)) => event,
        };
        // Content and metadata changes don't affect the index; name changes
        // (renames) and creations/removals do.
        match event.kind {
            EventKind::Access(_)
            | EventKind::Modify(ModifyKind::Data(_))
            | EventKind::Modify(ModifyKind::Metadata(_)) => return true,
            _ => {}
        }
        for path in &event.paths {
            let Ok(rel) = path.strip_prefix(&self.shared.root_path) else {
                continue;
            };
            // Activity inside .git (constant during git operations) doesn't
            // change the indexed tree, which never descends into it.
            if rel.components().any(|c| c.as_os_str() == GIT_DIR) {
                continue;
            }
            if path.file_name() == Some(OsStr::new(GITIGNORE)) {
                // A changed .gitignore can flip the ignored state of anything
                // beneath its directory: drop the cached matcher and rescan
                // the whole subtree.
                if let Some(dir) = path.parent() {
                    self.gitignores.remove(dir);
                    self.enqueue_subtree(dir);
                }
            } else if let Some(parent) = path.parent() {
                // Rescanning the parent picks up creations, deletions, and
                // renames regardless of the exact event kind.
                self.enqueue_nearest(parent);
            }
        }
        true
    }

    fn enqueue(&mut self, dir: PathBuf, ignored: bool) {
        if !self.queued.insert(dir.clone()) {
            return;
        }
        let mut state = self.shared.state.lock().unwrap();
        state.full_done = false;
        if ignored {
            drop(state);
            self.deferred.push_back(dir);
        } else {
            state.primary_done = false;
            drop(state);
            self.priority.push_back(dir);
        }
    }

    /// Enqueue a rescan of `dir`, or of its nearest indexed ancestor if the
    /// directory isn't in the tree yet (its pending ancestor scan will reach
    /// it anyway).
    fn enqueue_nearest(&mut self, dir: &Path) {
        let mut current = dir;
        loop {
            let Ok(rel) = current.strip_prefix(&self.shared.root_path) else {
                return;
            };
            let ignored = {
                let state = self.shared.state.lock().unwrap();
                find_node(&state.root, rel).map(|node| node.ignored)
            };
            if let Some(ignored) = ignored {
                self.enqueue(current.to_path_buf(), ignored);
                return;
            }
            match current.parent() {
                Some(parent) => current = parent,
                None => return,
            }
        }
    }

    /// Enqueue a rescan of `dir` and every already-scanned directory below it.
    fn enqueue_subtree(&mut self, dir: &Path) {
        let Ok(rel) = dir.strip_prefix(&self.shared.root_path) else {
            return;
        };
        let mut to_scan = Vec::new();
        {
            let state = self.shared.state.lock().unwrap();
            let Some(node) = find_node(&state.root, rel) else {
                drop(state);
                self.enqueue_nearest(dir);
                return;
            };
            fn collect(node: &DirNode, path: PathBuf, out: &mut Vec<(PathBuf, bool)>) {
                if node.name == GIT_DIR {
                    return;
                }
                out.push((path.clone(), node.ignored));
                for child in &node.dirs {
                    if child.scanned {
                        collect(child, path.join(&child.name), out);
                    }
                }
            }
            collect(node, dir.to_path_buf(), &mut to_scan);
        }
        for (path, ignored) in to_scan {
            self.enqueue(path, ignored);
        }
    }

    fn update_flags(&self) {
        let mut state = self.shared.state.lock().unwrap();
        state.primary_done = self.priority.is_empty();
        state.full_done = state.primary_done && self.deferred.is_empty();
        if state.primary_done {
            self.shared.cond.notify_all();
        }
    }

    /// The stack of gitignore matchers governing entries of `dir`, ordered
    /// from the root down so that deeper files override shallower ones.
    fn ignore_chain(&mut self, dir: &Path) -> Vec<Arc<Gitignore>> {
        let mut ancestors: Vec<&Path> = dir
            .ancestors()
            .take_while(|a| a.starts_with(&self.shared.root_path))
            .collect();
        ancestors.reverse();
        let mut chain = Vec::new();
        for ancestor in ancestors {
            let matcher = self
                .gitignores
                .entry(ancestor.to_path_buf())
                .or_insert_with(|| {
                    let file = ancestor.join(GITIGNORE);
                    if !file.is_file() {
                        return None;
                    }
                    let mut builder = GitignoreBuilder::new(ancestor);
                    builder.add(file);
                    builder.build().ok().map(Arc::new)
                });
            if let Some(matcher) = matcher {
                chain.push(matcher.clone());
            }
        }
        chain
    }

    fn is_ignored(chain: &[Arc<Gitignore>], path: &Path, is_dir: bool) -> bool {
        let mut ignored = false;
        for matcher in chain {
            match matcher.matched(path, is_dir) {
                Match::Ignore(_) => ignored = true,
                Match::Whitelist(_) => ignored = false,
                Match::None => {}
            }
        }
        ignored
    }

    fn scan_dir(&mut self, dir: &Path) {
        let Ok(rel) = dir.strip_prefix(&self.shared.root_path) else {
            return;
        };
        let rel = rel.to_path_buf();

        // A directory ignored by its parent stays ignored throughout (matching
        // git, which never re-includes files inside an excluded directory).
        let inherited = {
            let state = self.shared.state.lock().unwrap();
            match find_node(&state.root, &rel) {
                Some(node) => node.ignored,
                None => return, // removed from the tree since being queued
            }
        };

        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => {
                self.remove_node(&rel);
                return;
            }
        };

        let chain = self.ignore_chain(dir);
        let mut dirs: Vec<(OsString, bool)> = Vec::new();
        let mut files: Vec<(OsString, bool)> = Vec::new();
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            let child_path = dir.join(&name);
            // Symlinks are listed as files and never followed, so a symlink
            // cycle can't trap the indexer.
            if file_type.is_dir() {
                let ignored =
                    name == GIT_DIR || inherited || Self::is_ignored(&chain, &child_path, true);
                dirs.push((name, ignored));
            } else {
                let ignored = inherited || Self::is_ignored(&chain, &child_path, false);
                files.push((name, ignored));
            }
        }
        dirs.sort_by(|a, b| a.0.cmp(&b.0));
        files.sort_by(|a, b| a.0.cmp(&b.0));

        let mut to_enqueue: Vec<(PathBuf, bool)> = Vec::new();
        {
            let mut state = self.shared.state.lock().unwrap();
            let Some(node) = find_node_mut(&mut state.root, &rel) else {
                return;
            };
            node.scanned = true;
            node.files = files
                .into_iter()
                .map(|(name, ignored)| FileNode { name, ignored })
                .collect();
            let mut old: HashMap<OsString, DirNode> = std::mem::take(&mut node.dirs)
                .into_iter()
                .map(|d| (d.name.clone(), d))
                .collect();
            node.dirs = dirs
                .into_iter()
                .map(|(name, ignored)| match old.remove(&name) {
                    Some(mut existing) => {
                        // An ignored-state flip changes what the children
                        // inherit, so the subtree needs rescanning.
                        let needs_scan = existing.ignored != ignored || !existing.scanned;
                        existing.ignored = ignored;
                        if needs_scan && name != GIT_DIR {
                            to_enqueue.push((dir.join(&name), ignored));
                        }
                        existing
                    }
                    None => {
                        let mut new = DirNode::new(name.clone(), ignored);
                        if name == GIT_DIR {
                            // Deliberately left unscanned-but-complete: the
                            // tree shows .git but never its contents.
                            new.scanned = true;
                        } else {
                            to_enqueue.push((dir.join(&name), ignored));
                        }
                        new
                    }
                })
                .collect();
        }
        if self.shared.recursive {
            for (path, ignored) in to_enqueue {
                self.enqueue(path, ignored);
            }
        }
        self.shared.generation.fetch_add(1, Ordering::Release);
    }

    fn remove_node(&self, rel: &Path) {
        let Some(parent_rel) = rel.parent() else {
            return;
        };
        let Some(name) = rel.file_name() else { return };
        let mut state = self.shared.state.lock().unwrap();
        if let Some(parent) = find_node_mut(&mut state.root, parent_rel) {
            parent.dirs.retain(|d| d.name != name);
        }
        drop(state);
        self.shared.generation.fetch_add(1, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(10);

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn build_tree(root: &Path) {
        write(&root.join(".gitignore"), "target/\n*.log\n");
        write(&root.join("README.md"), "readme");
        write(&root.join("notes.log"), "log");
        write(&root.join("src/main.rs"), "fn main() {}");
        write(&root.join("target/debug/app"), "binary");
        write(&root.join("sub/.gitignore"), "generated.rs\n");
        write(&root.join("sub/lib.rs"), "lib");
        write(&root.join("sub/generated.rs"), "generated");
        write(&root.join(".git/config"), "[core]");
    }

    fn relative(paths: Vec<PathBuf>, root: &Path) -> Vec<String> {
        let mut out: Vec<String> = paths
            .into_iter()
            .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().into_owned())
            .collect();
        out.sort();
        out
    }

    /// Poll until `check` passes or the timeout elapses; used for conditions
    /// driven by filesystem watcher events.
    fn eventually(check: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if check() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn index_respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        build_tree(dir.path());
        let index = FileIndex::new(dir.path());
        assert!(index.wait_for_full(WAIT), "indexing did not finish");
        let root = index.root().to_path_buf();

        assert_eq!(
            relative(index.files(false), &root),
            vec![
                ".gitignore",
                "README.md",
                "src/main.rs",
                "sub/.gitignore",
                "sub/lib.rs"
            ]
        );
        assert_eq!(
            relative(index.files(true), &root),
            vec![
                ".gitignore",
                "README.md",
                "notes.log",
                "src/main.rs",
                "sub/.gitignore",
                "sub/generated.rs",
                "sub/lib.rs",
                "target/debug/app",
            ],
            "ignored files are indexed too, but .git contents never are"
        );
    }

    #[test]
    fn hierarchy_and_ignored_flags() {
        let dir = tempfile::tempdir().unwrap();
        build_tree(dir.path());
        let index = FileIndex::new(dir.path());
        assert!(index.wait_for_full(WAIT));

        let entries = index.children("").unwrap();
        let flag = |name: &str| {
            let entry = entries.iter().find(|e| e.name == name).unwrap_or_else(|| {
                panic!("missing entry {name}");
            });
            entry.ignored
        };
        assert!(flag(".git"), ".git must be marked ignored");
        assert!(
            flag("target"),
            "gitignored directory must be marked ignored"
        );
        assert!(flag("notes.log"), "gitignored file must be marked ignored");
        assert!(!flag("src"));
        assert!(!flag("README.md"));

        // Directories come first, and everything is sorted by name.
        let dirs: Vec<_> = entries
            .iter()
            .take_while(|e| e.is_dir)
            .map(|e| e.name.clone())
            .collect();
        assert_eq!(dirs, vec![".git", "src", "sub", "target"]);

        // .git is present in the tree but its contents are not.
        let git = index.children(".git").unwrap();
        assert!(git.is_empty());

        // Nested .gitignore applies to its own directory.
        let sub = index.children("sub").unwrap();
        assert!(
            sub.iter()
                .find(|e| e.name == "generated.rs")
                .unwrap()
                .ignored
        );
        assert!(!sub.iter().find(|e| e.name == "lib.rs").unwrap().ignored);

        // Files inside an ignored directory are ignored by inheritance.
        let target_debug = index.children(Path::new("target").join("debug")).unwrap();
        assert!(target_debug.iter().all(|e| e.ignored));
    }

    #[test]
    fn shallow_index_stops_at_the_root() {
        let dir = tempfile::tempdir().unwrap();
        build_tree(dir.path());
        let index = FileIndex::shallow(dir.path());
        assert!(!index.is_recursive());
        assert!(index.wait_for_full(WAIT), "indexing did not finish");
        let root = index.root().to_path_buf();

        assert_eq!(
            relative(index.files(false), &root),
            vec![".gitignore", "README.md"]
        );
        assert_eq!(
            relative(index.files(true), &root),
            vec![".gitignore", "README.md", "notes.log"]
        );
        // Subdirectories are listed, but their contents are never looked at.
        let dirs: Vec<_> = index
            .children("")
            .unwrap()
            .into_iter()
            .filter(|e| e.is_dir)
            .map(|e| e.name)
            .collect();
        assert_eq!(dirs, vec![".git", "src", "sub", "target"]);
        assert!(index.children("src").is_none());

        // Changes in the root are still picked up; those below it are not.
        write(&root.join("new.rs"), "new");
        assert!(
            eventually(|| relative(index.files(false), &root).contains(&"new.rs".to_string())),
            "new root file was not picked up by the watcher"
        );
        write(&root.join("src/new_file.rs"), "new");
        std::thread::sleep(Duration::from_millis(300));
        assert!(index.children("src").is_none());
        assert_eq!(
            relative(index.files(false), &root),
            vec![".gitignore", "README.md", "new.rs"]
        );
    }

    #[test]
    fn watcher_picks_up_new_files() {
        let dir = tempfile::tempdir().unwrap();
        build_tree(dir.path());
        let index = FileIndex::new(dir.path());
        assert!(index.wait_for_full(WAIT));
        let root = index.root().to_path_buf();

        write(&root.join("src/new_file.rs"), "new");
        assert!(
            eventually(
                || relative(index.files(false), &root).contains(&"src/new_file.rs".to_string())
            ),
            "new file was not picked up by the watcher"
        );

        write(&root.join("brand_new_dir/inner/deep.rs"), "deep");
        assert!(
            eventually(|| relative(index.files(false), &root)
                .contains(&"brand_new_dir/inner/deep.rs".to_string())),
            "file in new directory was not picked up by the watcher"
        );

        fs::remove_file(root.join("src/new_file.rs")).unwrap();
        assert!(
            eventually(
                || !relative(index.files(false), &root).contains(&"src/new_file.rs".to_string())
            ),
            "deleted file was not removed from the index"
        );
    }

    #[test]
    fn gitignore_changes_reflag_files() {
        let dir = tempfile::tempdir().unwrap();
        build_tree(dir.path());
        let index = FileIndex::new(dir.path());
        assert!(index.wait_for_full(WAIT));
        let root = index.root().to_path_buf();

        assert!(!relative(index.files(false), &root).contains(&"notes.log".to_string()));

        // Stop ignoring *.log: notes.log should become a project file.
        write(&root.join(".gitignore"), "target/\n");
        assert!(
            eventually(|| relative(index.files(false), &root).contains(&"notes.log".to_string())),
            "gitignore change was not applied to the index"
        );

        // Start ignoring src: its files should drop out of the project set.
        write(&root.join(".gitignore"), "target/\nsrc/\n");
        assert!(
            eventually(|| !relative(index.files(false), &root).contains(&"src/main.rs".to_string())),
            "newly ignored directory still appears in project files"
        );
        // But they remain in the full index, flagged as ignored.
        assert!(relative(index.files(true), &root).contains(&"src/main.rs".to_string()));
    }
}
