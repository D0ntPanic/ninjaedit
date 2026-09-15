//! The history of a repository: its branches, its remotes and their
//! branches, and every commit reachable from any of them, laid out as a
//! graph, as `git log --graph --all` shows it.
//!
//! [`History::open`] reads the references at once, since there are few,
//! and starts walking the commits on a worker thread, since there may
//! be a great many and libgit2 has to see them all before it can order
//! them. The commits arrive in batches through [`History::poll`], newest
//! first, each with its place in the graph (see the [`graph`] module);
//! a frontend polls from its idle loop and redraws as they come, showing
//! [`is_loading`](History::is_loading) until the walk is done. The walk
//! is in topological order with time breaking ties, git's `--date-order`,
//! so that every commit comes before its parents and otherwise the newest
//! come first.
//!
//! What a commit changed is read on demand, on the caller's thread, by
//! [`History::detail`] and [`History::file_diff`]; see the [`diff`]
//! module.
//!
//! [`graph`]: super::graph
//! [`diff`]: super::diff

use super::diff::{self, CommitDetail, FileDiff};
use super::graph::{GraphLayout, GraphRow};
pub use git2::Oid;
use git2::{BranchType, Repository, Sort};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

/// How many commits the walker gathers before sending them on, unless
/// [`BATCH_INTERVAL`] passes first. Small enough that the first screen
/// shows at once.
const BATCH_SIZE: usize = 256;
const BATCH_INTERVAL: Duration = Duration::from_millis(50);

/// A local branch, or a branch of a remote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// The branch's name: `main`, or for a remote's branch the part
    /// after the remote, so `main` for `origin/main`.
    pub name: String,
    /// The commit the branch points at.
    pub target: Oid,
    /// Whether HEAD is this (local) branch.
    pub is_head: bool,
}

/// A remote and its branches, in name order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub branches: Vec<Branch>,
}

/// What kind of reference points at a commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    Branch,
    RemoteBranch,
    Tag,
}

/// A reference pointing at a commit, as shown beside its message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefLabel {
    /// `main`, `origin/main`, `v1.0`.
    pub name: String,
    pub kind: RefKind,
    /// Whether this is the branch HEAD is on.
    pub is_head: bool,
}

/// When something happened, as git records it: seconds since the epoch
/// and the offset of the time zone it happened in. Displays as
/// `YYYY-MM-DD HH:MM` in the user's own time zone (the system's, as
/// `jiff` finds it: `TZ`, then the system's setting), which is what a
/// reader wants to know; the zone it happened in is kept for whoever
/// wants it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommitTime {
    pub seconds: i64,
    pub offset_minutes: i32,
}

/// How a [`CommitTime`] is written out.
const TIME_FORMAT: &str = "%Y-%m-%d %H:%M";

impl From<git2::Time> for CommitTime {
    fn from(time: git2::Time) -> CommitTime {
        CommitTime {
            seconds: time.seconds(),
            offset_minutes: time.offset_minutes(),
        }
    }
}

impl CommitTime {
    /// The time written as `YYYY-MM-DD HH:MM` in `zone`. A time git
    /// couldn't have recorded (outside the years jiff represents)
    /// falls back to the seconds themselves.
    pub fn in_zone(&self, zone: &TimeZone) -> String {
        match Timestamp::from_second(self.seconds) {
            Ok(timestamp) => timestamp
                .to_zoned(zone.clone())
                .strftime(TIME_FORMAT)
                .to_string(),
            Err(_) => format!("@{}", self.seconds),
        }
    }

    /// The time in the zone it was recorded in, as `git log` shows it.
    pub fn in_own_zone(&self) -> String {
        let zone = TimeZone::fixed(
            jiff::tz::Offset::from_seconds(self.offset_minutes * 60)
                .unwrap_or(jiff::tz::Offset::UTC),
        );
        self.in_zone(&zone)
    }
}

impl fmt::Display for CommitTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.in_zone(&TimeZone::system()))
    }
}

/// One commit of the history, with its place in the graph.
#[derive(Clone, Debug)]
pub struct Commit {
    pub id: Oid,
    /// The first line of the message.
    pub summary: String,
    pub author: String,
    /// When the author made it.
    pub time: CommitTime,
    pub parents: Vec<Oid>,
    /// The references pointing at it: the branch HEAD is on first, then
    /// the other local branches, remote branches, and tags, each in name
    /// order.
    pub refs: Vec<RefLabel>,
    pub graph: GraphRow,
}

impl Commit {
    /// The first characters of the id, as shown beside the commit.
    pub fn short_id(&self) -> String {
        short_id(self.id)
    }
}

/// The first characters of a commit id, enough to tell commits apart
/// in all but the largest histories.
pub fn short_id(id: Oid) -> String {
    id.to_string()[..8].to_owned()
}

enum Message {
    Commits(Vec<Commit>),
    Done,
    Failed(String),
}

/// A repository's history, filling in as it is walked.
pub struct History {
    repo: Repository,
    branches: Vec<Branch>,
    remotes: Vec<Remote>,
    head: Option<Oid>,
    /// The branch HEAD is on, unless detached.
    head_branch: Option<String>,
    commits: Vec<Commit>,
    /// Each commit's index in `commits`.
    positions: HashMap<Oid, usize>,
    /// The widest graph row so far, in lanes.
    max_lanes: usize,
    receiver: Receiver<Message>,
    loading: bool,
    error: Option<String>,
}

impl History {
    /// Open the repository containing `path` and start walking its
    /// history. Fails when `path` isn't inside a repository.
    pub fn open(path: impl AsRef<Path>) -> Result<History, git2::Error> {
        History::from_repository(Repository::discover(path)?)
    }

    /// Open the repository whose working directory is `path` itself,
    /// not one containing it: a submodule's directory that hasn't been
    /// initialized lies inside its parent's repository, and mustn't
    /// show the parent's history as its own.
    pub fn open_repository(path: impl AsRef<Path>) -> Result<History, git2::Error> {
        History::from_repository(Repository::open(path)?)
    }

    fn from_repository(repo: Repository) -> Result<History, git2::Error> {
        let head = repo.head().ok();
        let head_id = head.as_ref().and_then(|head| head.target());
        let detached = repo.head_detached().unwrap_or(false);
        let head_branch = match &head {
            Some(head) if !detached => head.shorthand().map(str::to_owned).ok(),
            _ => None,
        };
        let (branches, remotes, labels) = collect_refs(&repo, head_branch.as_deref())?;
        drop(head);

        let (sender, receiver) = mpsc::channel();
        let git_dir = repo.path().to_path_buf();
        thread::Builder::new()
            .name("git-log".to_owned())
            .spawn(move || walk(git_dir, labels, sender))
            .map_err(|err| git2::Error::from_str(&err.to_string()))?;

        Ok(History {
            repo,
            branches,
            remotes,
            head: head_id,
            head_branch,
            commits: Vec::new(),
            positions: HashMap::new(),
            max_lanes: 0,
            receiver,
            loading: true,
            error: None,
        })
    }

    /// The repository's git directory (`.git`, or the directory of a
    /// bare repository), to open it again elsewhere.
    pub fn git_dir(&self) -> &Path {
        self.repo.path()
    }

    /// The local branches, in name order.
    pub fn branches(&self) -> &[Branch] {
        &self.branches
    }

    /// The remotes, in name order, each with its branches.
    pub fn remotes(&self) -> &[Remote] {
        &self.remotes
    }

    /// The commit HEAD points at, if the repository has any commits.
    pub fn head(&self) -> Option<Oid> {
        self.head
    }

    /// The branch HEAD is on, or `None` when detached (or unborn).
    pub fn head_branch(&self) -> Option<&str> {
        self.head_branch.as_deref()
    }

    /// The commits walked so far, newest first.
    pub fn commits(&self) -> &[Commit] {
        &self.commits
    }

    /// Where a commit is in [`commits`](Self::commits), if walked yet.
    pub fn position(&self, id: Oid) -> Option<usize> {
        self.positions.get(&id).copied()
    }

    /// The widest graph row so far, in lanes.
    pub fn max_lanes(&self) -> usize {
        self.max_lanes
    }

    /// Whether the walk is still going.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Why the walk stopped short, if it did.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Take in the commits the walk has produced since the last poll.
    /// Returns whether anything changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            match self.receiver.try_recv() {
                Ok(Message::Commits(batch)) => {
                    for commit in batch {
                        self.positions.insert(commit.id, self.commits.len());
                        self.max_lanes = self.max_lanes.max(commit.graph.width());
                        self.commits.push(commit);
                    }
                    changed = true;
                }
                Ok(Message::Done) => {
                    self.loading = false;
                    changed = true;
                }
                Ok(Message::Failed(message)) => {
                    self.error = Some(message);
                    self.loading = false;
                    changed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.loading {
                        self.loading = false;
                        changed = true;
                    }
                    break;
                }
            }
        }
        changed
    }

    /// Poll until the walk is done or `timeout` passes. Returns whether
    /// it finished.
    pub fn wait(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.poll();
            if !self.loading {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// What a commit changed: its full message, who made it, and the
    /// files it touched, against its first parent.
    pub fn detail(&self, id: Oid) -> Result<CommitDetail, git2::Error> {
        diff::commit_detail(&self.repo, id)
    }

    /// The diff of one of the files a commit changed, with the whole of
    /// the file on both sides so the context around the changes can be
    /// expanded. `path` is the file's path after the commit (or before
    /// it, for a deleted file) and `old_path` its path before a rename.
    pub fn file_diff(
        &self,
        id: Oid,
        path: &str,
        old_path: Option<&str>,
    ) -> Result<FileDiff, git2::Error> {
        diff::file_diff(&self.repo, id, path, old_path)
    }
}

/// The references of the repository: the local branches, the remotes
/// with their branches, and for every commit the labels to show beside
/// it.
#[allow(clippy::type_complexity)]
pub(super) fn collect_refs(
    repo: &Repository,
    head_branch: Option<&str>,
) -> Result<(Vec<Branch>, Vec<Remote>, HashMap<Oid, Vec<RefLabel>>), git2::Error> {
    let mut labels: HashMap<Oid, Vec<RefLabel>> = HashMap::new();
    let mut label = |target: Oid, name: String, kind: RefKind, is_head: bool| {
        labels.entry(target).or_default().push(RefLabel {
            name,
            kind,
            is_head,
        });
    };

    let mut branches = Vec::new();
    for entry in repo.branches(Some(BranchType::Local))? {
        let (branch, _) = entry?;
        let Some(target) = branch.get().target() else {
            continue;
        };
        let name = branch.name()?.unwrap_or("?").to_owned();
        let is_head = head_branch == Some(name.as_str());
        label(target, name.clone(), RefKind::Branch, is_head);
        branches.push(Branch {
            name,
            target,
            is_head,
        });
    }
    branches.sort_by(|a, b| a.name.cmp(&b.name));

    let mut remotes: Vec<Remote> = repo
        .remotes()?
        .iter()
        .flatten()
        .map(|name| Remote {
            name: name.unwrap_or("?").to_owned(),
            branches: Vec::new(),
        })
        .collect();
    remotes.sort_by(|a, b| a.name.cmp(&b.name));
    for entry in repo.branches(Some(BranchType::Remote))? {
        let (branch, _) = entry?;
        // A remote's HEAD is a symbolic reference to one of its
        // branches, not a branch of its own.
        let Some(target) = branch.get().target() else {
            continue;
        };
        let full = branch.name()?.unwrap_or("?").to_owned();
        // The longest remote name that prefixes the branch's name owns
        // it; a remote not listed (its configuration gone) is added.
        let owner = remotes
            .iter()
            .filter(|remote| {
                full.len() > remote.name.len() + 1
                    && full.starts_with(&remote.name)
                    && full.as_bytes()[remote.name.len()] == b'/'
            })
            .map(|remote| remote.name.len())
            .max();
        let split = match owner {
            Some(len) => len,
            None => match full.find('/') {
                Some(at) => {
                    remotes.push(Remote {
                        name: full[..at].to_owned(),
                        branches: Vec::new(),
                    });
                    remotes.sort_by(|a, b| a.name.cmp(&b.name));
                    at
                }
                None => continue,
            },
        };
        let (remote_name, name) = (&full[..split], &full[split + 1..]);
        label(target, full.clone(), RefKind::RemoteBranch, false);
        if let Some(remote) = remotes.iter_mut().find(|r| r.name == remote_name) {
            remote.branches.push(Branch {
                name: name.to_owned(),
                target,
                is_head: false,
            });
        }
    }
    for remote in &mut remotes {
        remote.branches.sort_by(|a, b| a.name.cmp(&b.name));
    }

    // A tag may point at a tag object rather than a commit; either way
    // it labels the commit it comes down to.
    repo.tag_foreach(|id, name| {
        let name = String::from_utf8_lossy(name);
        let name = name.strip_prefix("refs/tags/").unwrap_or(&name).to_owned();
        if let Ok(object) = repo.find_object(id, None)
            && let Ok(commit) = object.peel_to_commit()
        {
            label(commit.id(), name, RefKind::Tag, false);
        }
        true
    })?;

    for labels in labels.values_mut() {
        labels.sort_by(|a, b| (a.kind, !a.is_head, &a.name).cmp(&(b.kind, !b.is_head, &b.name)));
    }
    Ok((branches, remotes, labels))
}

/// The worker: walk every commit reachable from a reference, lay each
/// out in the graph, and send them on in batches.
fn walk(git_dir: PathBuf, mut labels: HashMap<Oid, Vec<RefLabel>>, sender: Sender<Message>) {
    let result = (|| -> Result<(), git2::Error> {
        let repo = Repository::open(&git_dir)?;
        let mut revwalk = repo.revwalk()?;
        revwalk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
        revwalk.push_glob("refs/heads/*")?;
        revwalk.push_glob("refs/remotes/*")?;
        revwalk.push_glob("refs/tags/*")?;
        // An unborn HEAD has nothing to push.
        let _ = revwalk.push_head();

        let mut layout = GraphLayout::new();
        let mut pending: Option<PendingCommit> = None;
        let mut batch = Vec::new();
        let mut last_sent = Instant::now();
        for id in revwalk {
            let id = id?;
            let commit = repo.find_commit(id)?;
            let info = PendingCommit::read(&commit, labels.remove(&id).unwrap_or_default());
            if let Some(row) = layout.push(id, &info.parents) {
                let done = pending.take().expect("a row finishes a pushed commit");
                batch.push(done.finish(row));
            }
            pending = Some(info);
            if batch.len() >= BATCH_SIZE || last_sent.elapsed() >= BATCH_INTERVAL {
                if !batch.is_empty() && sender.send(Message::Commits(batch)).is_err() {
                    return Ok(());
                }
                batch = Vec::new();
                last_sent = Instant::now();
            }
        }
        if let (Some(row), Some(done)) = (layout.finish(), pending.take()) {
            batch.push(done.finish(row));
        }
        if !batch.is_empty() {
            let _ = sender.send(Message::Commits(batch));
        }
        Ok(())
    })();
    let _ = sender.send(match result {
        Ok(()) => Message::Done,
        Err(err) => Message::Failed(err.message().to_owned()),
    });
}

/// A commit read from the repository, waiting for its graph row.
pub(super) struct PendingCommit {
    id: Oid,
    summary: String,
    author: String,
    time: CommitTime,
    pub(super) parents: Vec<Oid>,
    refs: Vec<RefLabel>,
}

impl PendingCommit {
    /// Read what the log shows of a commit, with the references that
    /// label it.
    pub(super) fn read(commit: &git2::Commit<'_>, refs: Vec<RefLabel>) -> PendingCommit {
        PendingCommit {
            id: commit.id(),
            summary: commit
                .summary_bytes()
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                .unwrap_or_default(),
            author: commit.author().name().unwrap_or("").to_owned(),
            time: commit.author().when().into(),
            parents: commit.parent_ids().collect(),
            refs,
        }
    }

    pub(super) fn finish(self, graph: GraphRow) -> Commit {
        Commit {
            id: self.id,
            summary: self.summary,
            author: self.author,
            time: self.time,
            parents: self.parents,
            refs: self.refs,
            graph,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use git2::Signature;
    use std::fs;

    /// A repository in a temporary directory, with helpers to build a
    /// history in it.
    pub struct TestRepo {
        pub dir: tempfile::TempDir,
        pub repo: Repository,
        clock: i64,
    }

    impl TestRepo {
        pub fn new() -> TestRepo {
            let dir = tempfile::tempdir().unwrap();
            let repo = Repository::init(dir.path()).unwrap();
            TestRepo {
                dir,
                repo,
                clock: 1_700_000_000,
            }
        }

        pub fn path(&self) -> &Path {
            self.dir.path()
        }

        fn signature(&mut self) -> Signature<'static> {
            self.clock += 60;
            Signature::new(
                "Test Author",
                "test@example.com",
                &git2::Time::new(self.clock, 0),
            )
            .unwrap()
        }

        /// Write files and commit them on top of `parents` (HEAD's
        /// branch moves to the commit).
        pub fn commit(&mut self, files: &[(&str, &str)], message: &str, parents: &[Oid]) -> Oid {
            for (name, content) in files {
                let path = self.dir.path().join(name);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, content).unwrap();
            }
            let mut index = self.repo.index().unwrap();
            for (name, _) in files {
                index.add_path(Path::new(name)).unwrap();
            }
            index.write().unwrap();
            let tree_id = index.write_tree().unwrap();
            let sig = self.signature();
            let tree = self.repo.find_tree(tree_id).unwrap();
            let parents: Vec<git2::Commit<'_>> = parents
                .iter()
                .map(|id| self.repo.find_commit(*id).unwrap())
                .collect();
            let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
            self.repo
                .commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
                .unwrap()
        }

        /// Remove a file and commit that.
        pub fn remove(&mut self, name: &str, message: &str, parent: Oid) -> Oid {
            fs::remove_file(self.dir.path().join(name)).unwrap();
            let mut index = self.repo.index().unwrap();
            index.remove_path(Path::new(name)).unwrap();
            index.write().unwrap();
            let tree_id = index.write_tree().unwrap();
            let sig = self.signature();
            let tree = self.repo.find_tree(tree_id).unwrap();
            let parent = self.repo.find_commit(parent).unwrap();
            self.repo
                .commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])
                .unwrap()
        }

        pub fn branch(&self, name: &str, at: Oid) {
            let commit = self.repo.find_commit(at).unwrap();
            self.repo.branch(name, &commit, false).unwrap();
        }

        pub fn checkout(&self, name: &str) {
            self.repo.set_head(&format!("refs/heads/{name}")).unwrap();
        }

        /// Add an initialized submodule at `path`, with one commit of
        /// its own, and commit the gitlink on HEAD. Returns the
        /// submodule's repository.
        pub fn add_submodule(&mut self, path: &str) -> Repository {
            add_submodule_to(&self.repo, path)
        }

        /// Declare a submodule at `path` and leave it uninitialized: an
        /// empty directory.
        pub fn add_uninitialized_submodule(&mut self, path: &str) {
            self.repo
                .submodule("https://example.com/sub.git", Path::new(path), true)
                .unwrap();
            let dir = self.dir.path().join(path);
            fs::remove_dir_all(&dir).unwrap();
            fs::create_dir_all(&dir).unwrap();
        }

        pub fn tag(&self, name: &str, at: Oid) {
            let object = self.repo.find_object(at, None).unwrap();
            self.repo.tag_lightweight(name, &object, false).unwrap();
        }

        /// Pretend `name` is a remote with a branch at `at`.
        pub fn remote_branch(&self, remote: &str, branch: &str, at: Oid) {
            if self.repo.find_remote(remote).is_err() {
                self.repo
                    .remote(remote, "https://example.com/repo.git")
                    .unwrap();
            }
            self.repo
                .reference(
                    &format!("refs/remotes/{remote}/{branch}"),
                    at,
                    false,
                    "test",
                )
                .unwrap();
        }
    }

    /// Add an initialized submodule to `parent` at `path`, with one
    /// commit of its own, and commit the gitlink on the parent's HEAD.
    pub fn add_submodule_to(parent: &Repository, path: &str) -> Repository {
        let mut submodule = parent
            .submodule("https://example.com/sub.git", Path::new(path), true)
            .unwrap();
        let sub = submodule.open().unwrap();
        let sig = Signature::now("Sub Author", "sub@example.com").unwrap();
        {
            fs::write(sub.workdir().unwrap().join("inner.txt"), "inner\n").unwrap();
            let mut index = sub.index().unwrap();
            index.add_path(Path::new("inner.txt")).unwrap();
            index.write().unwrap();
            let tree = sub.find_tree(index.write_tree().unwrap()).unwrap();
            sub.commit(Some("HEAD"), &sig, &sig, "Inner commit", &tree, &[])
                .unwrap();
        }
        submodule.add_finalize().unwrap();
        let mut index = parent.index().unwrap();
        let tree = parent.find_tree(index.write_tree().unwrap()).unwrap();
        let head = parent.head().ok().and_then(|h| h.target());
        let parents: Vec<git2::Commit<'_>> = head
            .into_iter()
            .map(|id| parent.find_commit(id).unwrap())
            .collect();
        let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        parent
            .commit(
                Some("HEAD"),
                &sig,
                &sig,
                &format!("Add submodule {path}"),
                &tree,
                &refs,
            )
            .unwrap();
        sub
    }

    pub fn open(repo: &TestRepo) -> History {
        let mut history = History::open(repo.path()).unwrap();
        assert!(history.wait(Duration::from_secs(10)), "the walk finished");
        history
    }

    #[test]
    fn times_display_in_a_zone() {
        let time = |seconds, offset_minutes| CommitTime {
            seconds,
            offset_minutes,
        };
        let utc = TimeZone::UTC;
        assert_eq!(time(0, 0).in_zone(&utc), "1970-01-01 00:00");
        assert_eq!(time(1_700_000_000, 0).in_zone(&utc), "2023-11-14 22:13");
        assert_eq!(time(951_782_400, 0).in_zone(&utc), "2000-02-29 00:00");
        // A named zone, with its daylight saving: New York is five
        // hours behind in November and four in July.
        let new_york = TimeZone::get("America/New_York").unwrap();
        assert_eq!(
            time(1_700_000_000, 0).in_zone(&new_york),
            "2023-11-14 17:13"
        );
        assert_eq!(
            time(1_720_000_000, 0).in_zone(&new_york),
            "2024-07-03 05:46"
        );
        // The zone the commit was made in.
        assert_eq!(time(0, 90).in_own_zone(), "1970-01-01 01:30");
        assert_eq!(time(0, -60).in_own_zone(), "1969-12-31 23:00");
        // The display is the system's zone: the same time as in that
        // zone, whatever it is here.
        let now = time(1_700_000_000, 0);
        assert_eq!(now.to_string(), now.in_zone(&TimeZone::system()));
        assert_eq!(now.to_string().len(), "2023-11-14 22:13".len());
    }

    #[test]
    fn a_linear_history_lists_newest_first_with_head_labelled() {
        let mut repo = TestRepo::new();
        let a = repo.commit(&[("a.txt", "one\n")], "First commit\n\nWith a body.\n", &[]);
        let b = repo.commit(&[("a.txt", "one\ntwo\n")], "Second commit", &[a]);
        let history = open(&repo);
        assert!(!history.is_loading());
        assert_eq!(history.error(), None);
        assert_eq!(history.head(), Some(b));
        // Whatever the default branch is called, HEAD is on it.
        assert_eq!(
            history.head_branch(),
            Some(history.branches()[0].name.as_str())
        );
        let ids: Vec<Oid> = history.commits().iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![b, a]);
        assert_eq!(history.position(a), Some(1));
        let first = &history.commits()[0];
        assert_eq!(first.summary, "Second commit");
        assert_eq!(first.author, "Test Author");
        assert_eq!(first.parents, vec![a]);
        assert_eq!(first.refs.len(), 1);
        assert_eq!(first.refs[0].kind, RefKind::Branch);
        assert!(first.refs[0].is_head);
        assert_eq!(first.short_id(), &b.to_string()[..8]);
        assert_eq!(history.commits()[1].summary, "First commit");
        assert!(history.commits()[1].refs.is_empty());
        assert_eq!(history.max_lanes(), 1);
        assert_eq!(history.branches().len(), 1);
        assert!(history.branches()[0].is_head);
        assert!(history.remotes().is_empty());
    }

    #[test]
    fn branches_remotes_and_tags_are_listed_and_label_their_commits() {
        let mut repo = TestRepo::new();
        let a = repo.commit(&[("a.txt", "one\n")], "Base", &[]);
        repo.branch("feature", a);
        repo.checkout("feature");
        let f = repo.commit(&[("f.txt", "feature\n")], "Feature work", &[a]);
        repo.tag("v1.0", a);
        repo.remote_branch("origin", "feature", f);
        repo.remote_branch("origin", "main", a);
        repo.remote_branch("upstream", "main", a);
        let history = open(&repo);

        let names: Vec<&str> = history.branches().iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"feature"), "{names:?}");
        assert_eq!(names.len(), 2);
        assert_eq!(history.head_branch(), Some("feature"));
        let remotes: Vec<(&str, Vec<&str>)> = history
            .remotes()
            .iter()
            .map(|r| {
                (
                    r.name.as_str(),
                    r.branches.iter().map(|b| b.name.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            remotes,
            vec![
                ("origin", vec!["feature", "main"]),
                ("upstream", vec!["main"])
            ]
        );

        let top = &history.commits()[0];
        assert_eq!(top.id, f);
        let labels: Vec<(&str, RefKind, bool)> = top
            .refs
            .iter()
            .map(|r| (r.name.as_str(), r.kind, r.is_head))
            .collect();
        assert_eq!(
            labels,
            vec![
                ("feature", RefKind::Branch, true),
                ("origin/feature", RefKind::RemoteBranch, false)
            ]
        );
        let base = &history.commits()[1];
        assert_eq!(base.id, a);
        let labels: Vec<(&str, RefKind)> = base
            .refs
            .iter()
            .map(|r| (r.name.as_str(), r.kind))
            .collect();
        assert_eq!(labels.last(), Some(&("v1.0", RefKind::Tag)));
        assert_eq!(labels.len(), 4, "{labels:?}");
        assert_eq!(labels[1], ("origin/main", RefKind::RemoteBranch));
    }

    #[test]
    fn a_merge_is_laid_out_with_two_lanes() {
        let mut repo = TestRepo::new();
        let a = repo.commit(&[("a.txt", "one\n")], "Base", &[]);
        let main = repo.repo.head().unwrap().shorthand().unwrap().to_owned();
        let b = repo.commit(&[("b.txt", "two\n")], "On main", &[a]);
        repo.branch("side", a);
        repo.checkout("side");
        let s = repo.commit(&[("s.txt", "side\n")], "On side", &[a]);
        repo.checkout(&main);
        let m = repo.commit(&[("m.txt", "merge\n")], "Merge side", &[b, s]);
        let history = open(&repo);
        let ids: Vec<Oid> = history.commits().iter().map(|c| c.id).collect();
        assert_eq!(ids[0], m);
        assert_eq!(ids[3], a);
        assert_eq!(history.max_lanes(), 2);
        let merge = &history.commits()[0];
        assert_eq!(merge.graph.column, 0);
        assert_eq!(merge.graph.edges.len(), 2);
        assert_eq!(history.commits()[3].graph.lanes, vec![Some(0)]);
    }

    #[test]
    fn an_empty_repository_has_no_commits_and_a_directory_is_not_one() {
        let repo = TestRepo::new();
        let history = open(&repo);
        assert!(history.commits().is_empty());
        assert_eq!(history.head(), None);
        assert_eq!(history.error(), None);
        let dir = tempfile::tempdir().unwrap();
        assert!(History::open(dir.path()).is_err());
    }
}
