//! The uncommitted changes of a working tree, as `git status` lists
//! them, and the steps of committing them: staging, unstaging, and the
//! commit itself. The changes page of a frontend is built on this.
//!
//! [`Changes::open`] opens the repository and starts a scan of it on a
//! worker thread, since a big working tree takes a while to compare
//! with the index and the index with HEAD; the result arrives through
//! [`Changes::poll`], which a frontend calls from its idle loop, showing
//! [`is_loading`](Changes::is_loading) meanwhile. A scan lists the
//! [`unstaged`](Changes::unstaged) changes (the working directory
//! against the index: modified, deleted, and untracked files, and any
//! the last merge left in conflict) and the [`staged`](Changes::staged)
//! ones (the index against HEAD), each with how many lines it gained
//! and lost, along with what a commit would be: the branch it goes on,
//! whether a merge is in progress and the message git prepared for it,
//! and the submodules with changes of their own (see the
//! [`submodules`](super::submodules) module).
//!
//! With [`amend`](Changes::set_amend) on, the commit will replace HEAD
//! rather than follow it, as `git commit --amend` does: the staged
//! list is then everything the amended commit would hold, the index
//! against HEAD's parent, and unstaging puts a file back to the
//! parent's version, so that a change HEAD made can be taken out of it
//! (it stays in the working directory, as an unstaged change).
//! [`head_message`](Changes::head_message) is HEAD's message, to start
//! the amended one from.
//!
//! Every action ([`stage`](Changes::stage), [`unstage`](Changes::unstage),
//! and [`commit`](Changes::commit)) changes the repository on the
//! caller's thread and updates the lists right away with what it did:
//! staging and unstaging compare just the files they touched again,
//! which is quick however big the tree, and a commit empties the
//! staged list. A scan then starts to confirm the lists and catch
//! anything else; while it runs, [`is_confirming`](Changes::is_confirming)
//! tells a frontend it needn't say it is waiting. [`refresh`](Changes::refresh)
//! starts a scan too, for when something else changed the working
//! tree: a file saved in the editor, a `git` command in a shell. A
//! scan started while one is running replaces it; the old one's result
//! is dropped.
//!
//! The diff of a change is read on demand, on the caller's thread, by
//! [`Changes::unstaged_diff`] and [`Changes::staged_diff`], as a
//! [`FileDiff`] like a commit's (see the [`diff`](super::diff) module).
//! A conflicted file is shown against our side of the merge, so the
//! diff is what the conflict markers and their side's lines add to it.

use super::diff::{
    self, CONTEXT_LINES, ChangeKind, Contents, FileChange, FileDiff, STATS_LIMIT, Unshown,
};
use super::submodules::{Submodule, changed_submodules};
use git2::{
    Delta, DiffFindOptions, DiffOptions, ErrorCode, Oid, Patch, Repository, RepositoryState, Tree,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

/// What a scan of the working tree found.
struct Snapshot {
    unstaged: Vec<FileChange>,
    staged: Vec<FileChange>,
    submodules: Vec<Submodule>,
    head_branch: Option<String>,
    detached: bool,
    unborn: bool,
    merging: bool,
    merge_message: Option<String>,
    head_message: Option<String>,
    conflicts: usize,
}

struct Scanned {
    /// Which scan this is the result of; an older one is dropped.
    generation: u64,
    result: Result<Snapshot, String>,
}

/// A working tree's uncommitted changes, kept up to date by scans.
pub struct Changes {
    repo: Repository,
    workdir: PathBuf,
    unstaged: Vec<FileChange>,
    staged: Vec<FileChange>,
    submodules: Vec<Submodule>,
    head_branch: Option<String>,
    detached: bool,
    unborn: bool,
    merging: bool,
    merge_message: Option<String>,
    head_message: Option<String>,
    conflicts: usize,
    /// Whether the commit will replace HEAD.
    amend: bool,
    receiver: Option<Receiver<Scanned>>,
    generation: u64,
    loading: bool,
    /// Whether the running scan follows an action the lists already
    /// show the result of.
    confirming: bool,
    error: Option<String>,
}

impl Changes {
    /// Open the repository containing `path` and start scanning its
    /// working tree. Fails when `path` isn't inside a repository, or
    /// the repository is bare.
    pub fn open(path: impl AsRef<Path>) -> Result<Changes, git2::Error> {
        Changes::from_repository(Repository::discover(path)?)
    }

    /// Open the repository whose working directory is `path` itself,
    /// not one containing it: a submodule's directory.
    pub fn open_repository(path: impl AsRef<Path>) -> Result<Changes, git2::Error> {
        Changes::from_repository(Repository::open(path)?)
    }

    fn from_repository(repo: Repository) -> Result<Changes, git2::Error> {
        let workdir = repo
            .workdir()
            .ok_or_else(|| git2::Error::from_str("the repository has no working tree"))?
            .to_path_buf();
        let mut changes = Changes {
            repo,
            workdir,
            unstaged: Vec::new(),
            staged: Vec::new(),
            submodules: Vec::new(),
            head_branch: None,
            detached: false,
            unborn: false,
            merging: false,
            merge_message: None,
            head_message: None,
            conflicts: 0,
            amend: false,
            receiver: None,
            generation: 0,
            loading: false,
            confirming: false,
            error: None,
        };
        changes.refresh();
        Ok(changes)
    }

    /// The working directory the paths of the changes are relative to.
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// Scan the working tree again. A scan already running is
    /// superseded: its result is dropped when it arrives.
    pub fn refresh(&mut self) {
        self.scan(false);
    }

    /// Scan the working tree to confirm what an action left the lists
    /// showing.
    fn confirm(&mut self) {
        self.scan(true);
    }

    fn scan(&mut self, confirming: bool) {
        self.generation += 1;
        let generation = self.generation;
        let (sender, receiver) = mpsc::channel();
        let workdir = self.workdir.clone();
        let amend = self.amend;
        let spawned = thread::Builder::new()
            .name("git-status".to_owned())
            .spawn(move || {
                let result = snapshot(&workdir, amend).map_err(|err| err.message().to_owned());
                let _ = sender.send(Scanned { generation, result });
            });
        match spawned {
            Ok(_) => {
                self.receiver = Some(receiver);
                self.loading = true;
                self.confirming = confirming;
            }
            Err(err) => {
                self.receiver = None;
                self.loading = false;
                self.confirming = false;
                self.error = Some(err.to_string());
            }
        }
    }

    /// Take in the result of a scan if one has finished. Returns whether
    /// anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(receiver) = &self.receiver else {
            return false;
        };
        let mut changed = false;
        let mut disconnected = false;
        let mut latest = None;
        loop {
            match receiver.try_recv() {
                Ok(scanned) if scanned.generation == self.generation => latest = Some(scanned),
                Ok(_) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if let Some(scanned) = latest {
            self.apply(scanned.result);
            changed = true;
        }
        if disconnected {
            if self.loading {
                self.loading = false;
                self.confirming = false;
                self.error = Some("the scan of the working tree stopped".to_owned());
                changed = true;
            }
            self.receiver = None;
        }
        changed
    }

    fn apply(&mut self, result: Result<Snapshot, String>) {
        self.loading = false;
        self.confirming = false;
        match result {
            Ok(snapshot) => {
                self.unstaged = snapshot.unstaged;
                self.staged = snapshot.staged;
                self.submodules = snapshot.submodules;
                self.head_branch = snapshot.head_branch;
                self.detached = snapshot.detached;
                self.unborn = snapshot.unborn;
                self.merging = snapshot.merging;
                self.merge_message = snapshot.merge_message;
                self.head_message = snapshot.head_message;
                self.conflicts = snapshot.conflicts;
                self.error = None;
            }
            Err(message) => self.error = Some(message),
        }
    }

    /// Poll until the scan is done or `timeout` passes. Returns whether
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

    /// Whether a scan is running.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Whether the running scan only confirms what the lists already
    /// show after an action, so that nothing is being waited for.
    pub fn is_confirming(&self) -> bool {
        self.loading && self.confirming
    }

    /// Why the last scan failed, if it did.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The changes in the working directory not yet staged: modified,
    /// deleted, and untracked files, and conflicts, in path order.
    pub fn unstaged(&self) -> &[FileChange] {
        &self.unstaged
    }

    /// The changes staged for the next commit, in path order: the index
    /// against HEAD, or when amending against HEAD's parent, so that
    /// the list is what the commit will hold.
    pub fn staged(&self) -> &[FileChange] {
        &self.staged
    }

    /// Whether the commit will replace HEAD, as `git commit --amend`
    /// does.
    pub fn is_amending(&self) -> bool {
        self.amend
    }

    /// Make the commit replace HEAD, or follow it again. Refused with
    /// no commit to amend or a merge in progress, as git refuses it.
    /// The lists scan again to follow.
    pub fn set_amend(&mut self, amend: bool) -> Result<(), git2::Error> {
        if amend == self.amend {
            return Ok(());
        }
        if amend {
            if self
                .repo
                .head()
                .and_then(|head| head.peel_to_commit())
                .is_err()
            {
                return Err(git2::Error::from_str("there is no commit to amend"));
            }
            if self.repo.state() == RepositoryState::Merge {
                return Err(git2::Error::from_str(
                    "a merge in progress can't amend: commit it first",
                ));
            }
        }
        self.amend = amend;
        self.refresh();
        Ok(())
    }

    /// HEAD's message, as the amended commit starts from it.
    pub fn head_message(&self) -> Option<&str> {
        self.head_message.as_deref()
    }

    /// The submodules with uncommitted changes of their own, nested ones
    /// included, in path order.
    pub fn changed_submodules(&self) -> &[Submodule] {
        &self.submodules
    }

    /// The branch a commit would go on, or `None` with HEAD detached.
    /// An unborn branch (no commits yet) is named too.
    pub fn head_branch(&self) -> Option<&str> {
        self.head_branch.as_deref()
    }

    /// Whether HEAD is detached: a commit would go on no branch.
    pub fn is_detached(&self) -> bool {
        self.detached
    }

    /// Whether the repository has no commits yet.
    pub fn is_unborn(&self) -> bool {
        self.unborn
    }

    /// Whether a merge is in progress: a commit would finish it, with
    /// the merged commit as its second parent.
    pub fn is_merging(&self) -> bool {
        self.merging
    }

    /// The message git prepared for the merge in progress, if any.
    pub fn merge_message(&self) -> Option<&str> {
        self.merge_message.as_deref()
    }

    /// How many files are in conflict, which must be resolved and
    /// staged before a commit.
    pub fn conflict_count(&self) -> usize {
        self.conflicts
    }

    /// Whether nothing is changed anywhere: the tree is clean.
    pub fn is_clean(&self) -> bool {
        self.unstaged.is_empty() && self.staged.is_empty()
    }

    // ----- Diffs -----------------------------------------------------------

    /// The diff of an unstaged change: the file in the working directory
    /// against the index (or nothing, for an untracked file). A
    /// conflicted file is shown against our side of the merge.
    pub fn unstaged_diff(&self, change: &FileChange) -> Result<FileDiff, git2::Error> {
        if change.kind == ChangeKind::Conflicted {
            return self.conflict_diff(change);
        }
        let mut options = workdir_options();
        options.pathspec(&change.path).disable_pathspec_match(true);
        let diff = self.repo.diff_index_to_workdir(None, Some(&mut options))?;
        let index = diff::delta_at(&diff, &change.path)?;
        let delta = diff.get_delta(index).expect("found above");
        if change.submodule {
            // The index's commit against the submodule's HEAD, which
            // libgit2 puts on the working directory's side.
            return Ok(diff::submodule_diff(
                &self.repo,
                change.kind,
                &change.path,
                None,
                &delta,
            ));
        }
        let old = diff::blob_contents(&self.repo, delta.old_file())?;
        let new = diff::workdir_contents(&self.workdir.join(&change.path));
        let patch = Patch::from_diff(&diff, index)?;
        FileDiff::build(change.kind, &change.path, None, &old, &new, patch.as_ref())
    }

    /// The commit the staged changes are against: HEAD, or its parent
    /// when amending (none for a root commit, or an unborn branch).
    fn base_commit(&self) -> Option<git2::Commit<'_>> {
        let head = self.repo.head().ok()?.peel_to_commit().ok()?;
        if self.amend {
            head.parent(0).ok()
        } else {
            Some(head)
        }
    }

    /// The diff of a staged change: the index against HEAD (or HEAD's
    /// parent, when amending).
    pub fn staged_diff(&self, change: &FileChange) -> Result<FileDiff, git2::Error> {
        let base_tree = self.base_commit().and_then(|commit| commit.tree().ok());
        let mut options = DiffOptions::new();
        options
            .context_lines(CONTEXT_LINES)
            .disable_pathspec_match(true)
            .pathspec(&change.path);
        if let Some(old_path) = &change.old_path {
            options.pathspec(old_path);
        }
        let mut diff =
            self.repo
                .diff_tree_to_index(base_tree.as_ref(), None, Some(&mut options))?;
        find_renames(&mut diff)?;
        let index = diff::delta_at(&diff, &change.path)?;
        let delta = diff.get_delta(index).expect("found above");
        if change.submodule {
            return Ok(diff::submodule_diff(
                &self.repo,
                change.kind,
                &change.path,
                change.old_path.as_deref(),
                &delta,
            ));
        }
        let old = diff::blob_contents(&self.repo, delta.old_file())?;
        let new = diff::blob_contents(&self.repo, delta.new_file())?;
        let patch = Patch::from_diff(&diff, index)?;
        FileDiff::build(
            change.kind,
            &change.path,
            change.old_path.as_deref(),
            &old,
            &new,
            patch.as_ref(),
        )
    }

    /// A conflicted file in the working directory against our side of
    /// the merge (or nothing, when our side deleted it).
    fn conflict_diff(&self, change: &FileChange) -> Result<FileDiff, git2::Error> {
        let index = self.repo.index()?;
        let ours = index
            .conflicts()?
            .filter_map(Result::ok)
            .find(|conflict| {
                [&conflict.our, &conflict.their, &conflict.ancestor]
                    .into_iter()
                    .flatten()
                    .any(|entry| entry.path == change.path.as_bytes())
            })
            .and_then(|conflict| conflict.our);
        let old = match ours {
            Some(entry) => {
                let blob = self.repo.find_blob(entry.id)?;
                let mut contents = Contents::from_bytes(blob.content().to_vec());
                if blob.is_binary() {
                    contents.unshown = Some(Unshown::Binary);
                }
                contents
            }
            None => Contents::empty(),
        };
        let new = diff::workdir_contents(&self.workdir.join(&change.path));
        let mut options = DiffOptions::new();
        options.context_lines(CONTEXT_LINES);
        let path = Path::new(&change.path);
        let patch = Patch::from_buffers(
            &old.bytes,
            Some(path),
            &new.bytes,
            Some(path),
            Some(&mut options),
        )?;
        FileDiff::build(change.kind, &change.path, None, &old, &new, Some(&patch))
    }

    // ----- Actions ---------------------------------------------------------

    /// Stage changes by path: a file's contents as they are in the
    /// working directory, or its deletion. Staging a conflicted file
    /// marks the conflict resolved. Staging a submodule stages the
    /// commit it is on.
    pub fn stage<'a>(
        &mut self,
        paths: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), git2::Error> {
        let paths: Vec<&str> = paths.into_iter().collect();
        let mut index = self.repo.index()?;
        for &path in &paths {
            if fs::symlink_metadata(self.workdir.join(path)).is_ok() {
                index.add_path(Path::new(path))?;
            } else {
                index.remove_path(Path::new(path))?;
            }
        }
        index.write()?;
        self.rescan(&paths);
        self.confirm();
        Ok(())
    }

    /// Stage every unstaged change of the last scan.
    pub fn stage_all(&mut self) -> Result<(), git2::Error> {
        let paths: Vec<String> = self.unstaged.iter().map(|c| c.path.clone()).collect();
        self.stage(paths.iter().map(String::as_str))
    }

    /// Unstage changes by path, putting the index's entries back to
    /// HEAD's (`git reset -- path`), or to HEAD's parent's when
    /// amending; the working directory is left alone.
    pub fn unstage<'a>(
        &mut self,
        paths: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), git2::Error> {
        let paths: Vec<&str> = paths.into_iter().collect();
        {
            let base = self.base_commit().map(|commit| commit.into_object());
            self.repo.reset_default(base.as_ref(), &paths)?;
        }
        self.rescan(&paths);
        self.confirm();
        Ok(())
    }

    /// Compare just `paths` again, on this thread, and put what they
    /// are now in place of what the lists had for them: the outcome of
    /// staging or unstaging them, before a scan of the whole tree
    /// confirms it. Failing that, the lists wait for the scan.
    fn rescan(&mut self, paths: &[&str]) {
        let Ok((staged, unstaged, icase)) = self.compare(paths) else {
            return;
        };
        let touched: HashSet<&str> = paths.iter().copied().collect();
        let untouched = |change: &FileChange| {
            !touched.contains(change.path.as_str())
                && !change
                    .old_path
                    .as_deref()
                    .is_some_and(|path| touched.contains(path))
        };
        // Keep the order a scan lists in, which follows the case rule
        // of the index, so that the confirming scan changes nothing.
        let key = |change: &FileChange| -> Vec<u8> {
            if icase {
                change.path.to_ascii_lowercase().into_bytes()
            } else {
                change.path.clone().into_bytes()
            }
        };
        for (list, fresh) in [(&mut self.staged, staged), (&mut self.unstaged, unstaged)] {
            list.retain(untouched);
            list.extend(fresh);
            list.sort_by_cached_key(key);
        }
        self.conflicts = count_conflicts(&self.unstaged);
    }

    /// The staged and unstaged changes of `paths` as they are now, with
    /// their lines counted unless the lists are too long for that, and
    /// whether the lists are ordered ignoring case.
    fn compare(
        &self,
        paths: &[&str],
    ) -> Result<(Vec<FileChange>, Vec<FileChange>, bool), git2::Error> {
        let base_tree = self.base_commit().and_then(|commit| commit.tree().ok());
        let diff = index_diff(&self.repo, base_tree.as_ref(), paths)?;
        let staged = list_changes(&diff, staged_delta, self.staged.len() <= STATS_LIMIT)?;
        let diff = workdir_diff(&self.repo, paths)?;
        let unstaged = list_changes(&diff, |_| true, self.unstaged.len() <= STATS_LIMIT)?;
        Ok((staged, unstaged, diff.is_sorted_icase()))
    }

    /// Unstage every staged change of the last scan.
    pub fn unstage_all(&mut self) -> Result<(), git2::Error> {
        let paths: Vec<String> = self
            .staged
            .iter()
            .flat_map(|c| std::iter::once(c.path.clone()).chain(c.old_path.clone()))
            .collect();
        self.unstage(paths.iter().map(String::as_str))
    }

    /// Commit what is staged with `message`, on the current branch (or
    /// as a detached commit), as the person the repository's
    /// configuration names. A merge in progress is finished: the merged
    /// commit is the second parent, and the merge state is cleared.
    /// When amending, HEAD is replaced by a commit with its parents,
    /// the staged tree, and the message. Refuses an empty message,
    /// unresolved conflicts, and a commit that would change nothing
    /// (unless it finishes a merge or amends).
    pub fn commit(&mut self, message: &str) -> Result<Oid, git2::Error> {
        let message = message.trim();
        if message.is_empty() {
            return Err(git2::Error::from_str("a commit needs a message"));
        }
        let merging = self.repo.state() == RepositoryState::Merge;
        let mut merged = Vec::new();
        if merging {
            self.repo.mergehead_foreach(|id| {
                merged.push(*id);
                true
            })?;
        }
        let id = {
            let mut index = self.repo.index()?;
            if index.has_conflicts() {
                return Err(git2::Error::from_str(
                    "resolve the conflicts and stage the files first",
                ));
            }
            let head = self
                .repo
                .head()
                .ok()
                .and_then(|head| head.peel_to_commit().ok());
            let tree_id = index.write_tree()?;
            let unchanged = match &head {
                Some(head) => head.tree_id() == tree_id,
                None => index.is_empty(),
            };
            if unchanged && !merging && !self.amend {
                return Err(git2::Error::from_str("nothing is staged to commit"));
            }
            let tree = self.repo.find_tree(tree_id)?;
            let signature = self.repo.signature()?;
            if self.amend {
                let head =
                    head.ok_or_else(|| git2::Error::from_str("there is no commit to amend"))?;
                head.amend(
                    Some("HEAD"),
                    None,
                    Some(&signature),
                    None,
                    Some(message),
                    Some(&tree),
                )?
            } else {
                let mut parents: Vec<git2::Commit<'_>> = head.into_iter().collect();
                for id in merged {
                    parents.push(self.repo.find_commit(id)?);
                }
                let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
                self.repo.commit(
                    Some("HEAD"),
                    &signature,
                    &signature,
                    message,
                    &tree,
                    &parent_refs,
                )?
            }
        };
        if merging {
            self.repo.cleanup_state()?;
        }
        // An amended commit is made; the next one follows it.
        self.amend = false;
        // The commit holds what was staged, and is now what a commit
        // would replace.
        self.staged.clear();
        self.unborn = false;
        self.merging = false;
        self.merge_message = None;
        self.head_message = Some(message.to_owned());
        self.confirm();
        Ok(id)
    }
}

/// The options for comparing the working directory with the index:
/// untracked files listed one by one with their contents, so they can
/// be staged and shown like any other change.
fn workdir_options() -> DiffOptions {
    let mut options = DiffOptions::new();
    options
        .context_lines(CONTEXT_LINES)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true)
        .include_typechange(true);
    options
}

/// Limit a diff to `paths`, taken literally; none means every path.
fn limit_to(options: &mut DiffOptions, paths: &[&str]) {
    if paths.is_empty() {
        return;
    }
    options.disable_pathspec_match(true);
    for path in paths {
        options.pathspec(path);
    }
}

/// The diff the staged list is built from: the index against
/// `base_tree` (HEAD's, or its parent's when amending; none on an
/// unborn branch), with renames found.
fn index_diff<'r>(
    repo: &'r Repository,
    base_tree: Option<&Tree<'_>>,
    paths: &[&str],
) -> Result<git2::Diff<'r>, git2::Error> {
    let mut options = DiffOptions::new();
    options.context_lines(CONTEXT_LINES);
    limit_to(&mut options, paths);
    let mut diff = repo.diff_tree_to_index(base_tree, None, Some(&mut options))?;
    find_renames(&mut diff)?;
    Ok(diff)
}

/// The diff the unstaged list is built from: the working directory
/// against the index.
fn workdir_diff<'r>(repo: &'r Repository, paths: &[&str]) -> Result<git2::Diff<'r>, git2::Error> {
    let mut options = workdir_options();
    limit_to(&mut options, paths);
    repo.diff_index_to_workdir(None, Some(&mut options))
}

/// Whether a file of the index diff belongs in the staged list: a
/// conflict is shown unstaged, to be resolved.
fn staged_delta(delta: Delta) -> bool {
    delta != Delta::Conflicted
}

fn count_conflicts(unstaged: &[FileChange]) -> usize {
    unstaged
        .iter()
        .filter(|change| change.kind == ChangeKind::Conflicted)
        .count()
}

fn find_renames(diff: &mut git2::Diff<'_>) -> Result<(), git2::Error> {
    let mut find = DiffFindOptions::new();
    find.renames(true);
    diff.find_similar(Some(&mut find))
}

/// The worker: scan a working tree. With `amend`, the staged changes
/// are against HEAD's parent rather than HEAD.
fn snapshot(workdir: &Path, amend: bool) -> Result<Snapshot, git2::Error> {
    let repo = Repository::open(workdir)?;
    let (head, head_branch, detached, unborn) = match repo.head() {
        Ok(head) => {
            let detached = repo.head_detached().unwrap_or(false);
            let name = if detached {
                None
            } else {
                head.shorthand().ok().map(str::to_owned)
            };
            (Some(head), name, detached, false)
        }
        Err(err) if err.code() == ErrorCode::UnbornBranch => {
            let name = repo.find_reference("HEAD").ok().and_then(|head| {
                head.symbolic_target().ok().flatten().map(|target| {
                    target
                        .strip_prefix("refs/heads/")
                        .unwrap_or(target)
                        .to_owned()
                })
            });
            (None, name, false, true)
        }
        Err(err) => return Err(err),
    };
    let head_commit = head.and_then(|head| head.peel_to_commit().ok());
    let head_message = head_commit.as_ref().and_then(|commit| {
        commit
            .message()
            .ok()
            .map(|message| message.trim_end().to_owned())
    });
    let base_tree = match &head_commit {
        Some(commit) if amend => commit.parent(0).ok().and_then(|parent| parent.tree().ok()),
        Some(commit) => commit.tree().ok(),
        None => None,
    };

    let diff = index_diff(&repo, base_tree.as_ref(), &[])?;
    let staged = list_changes(&diff, staged_delta, diff.deltas().len() <= STATS_LIMIT)?;
    let diff = workdir_diff(&repo, &[])?;
    let unstaged = list_changes(&diff, |_| true, diff.deltas().len() <= STATS_LIMIT)?;
    let conflicts = count_conflicts(&unstaged);

    let merging = repo.state() == RepositoryState::Merge;
    let merge_message = if merging {
        fs::read_to_string(repo.path().join("MERGE_MSG"))
            .ok()
            .map(|text| text.trim_end().to_owned())
            .filter(|text| !text.is_empty())
    } else {
        None
    };
    let submodules = changed_submodules(&repo);
    Ok(Snapshot {
        unstaged,
        staged,
        submodules,
        head_branch,
        detached,
        unborn,
        merging,
        merge_message,
        head_message,
        conflicts,
    })
}

/// The files of a diff whose status `keep` accepts, with their lines
/// counted when `stats` (see [`STATS_LIMIT`]).
fn list_changes(
    diff: &git2::Diff<'_>,
    keep: impl Fn(Delta) -> bool,
    stats: bool,
) -> Result<Vec<FileChange>, git2::Error> {
    let mut changes = Vec::new();
    for (index, delta) in diff.deltas().enumerate() {
        if !keep(delta.status()) {
            continue;
        }
        let stats = stats && delta.status() != Delta::Conflicted;
        changes.push(diff::file_change(diff, index, stats)?);
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::super::history::tests::TestRepo;
    use super::*;
    use crate::git::{DiffRow, LineKind};

    fn open(repo: &TestRepo) -> Changes {
        let mut changes = Changes::open(repo.path()).unwrap();
        assert!(changes.wait(Duration::from_secs(10)), "the scan finished");
        changes
    }

    fn settle(changes: &mut Changes) {
        assert!(changes.wait(Duration::from_secs(10)), "the scan finished");
    }

    /// Settle the scan that follows an action, checking that the lists
    /// showed its outcome before the scan and that the scan only
    /// confirms them.
    fn confirmed(changes: &mut Changes) {
        assert!(changes.is_confirming(), "the scan confirms an action");
        let owned = |changes: &[FileChange]| -> Vec<(char, String, usize, usize)> {
            listed(changes)
                .into_iter()
                .map(|(kind, path, added, removed)| (kind, path.to_owned(), added, removed))
                .collect()
        };
        let before = (
            owned(changes.staged()),
            owned(changes.unstaged()),
            changes.conflict_count(),
            changes.is_merging(),
            changes.head_message().map(str::to_owned),
        );
        settle(changes);
        assert!(!changes.is_confirming());
        let after = (
            owned(changes.staged()),
            owned(changes.unstaged()),
            changes.conflict_count(),
            changes.is_merging(),
            changes.head_message().map(str::to_owned),
        );
        assert_eq!(before, after, "the scan confirmed what the action showed");
    }

    fn listed(changes: &[FileChange]) -> Vec<(char, &str, usize, usize)> {
        changes
            .iter()
            .map(|c| (c.kind.letter(), c.path.as_str(), c.additions, c.deletions))
            .collect()
    }

    fn lines(file: &FileDiff) -> Vec<String> {
        file.rows()
            .iter()
            .filter_map(|row| match row {
                DiffRow::Line(line) => {
                    let marker = match line.kind {
                        LineKind::Context => ' ',
                        LineKind::Added => '+',
                        LineKind::Removed => '-',
                    };
                    Some(format!("{marker}{}", file.text(line)))
                }
                DiffRow::Gap { .. } => None,
            })
            .collect()
    }

    fn configure_user(repo: &TestRepo) {
        let mut config = repo.repo.config().unwrap();
        config.set_str("user.name", "Test Author").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();
    }

    #[test]
    fn changes_are_listed_staged_unstaged_and_committed() {
        let mut repo = TestRepo::new();
        configure_user(&repo);
        let base = repo.commit(
            &[
                ("a.rs", "fn a() {}\n"),
                ("gone.txt", "bye\n"),
                ("keep.txt", "k\n"),
            ],
            "Base",
            &[],
        );
        fs::write(repo.path().join("a.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        fs::remove_file(repo.path().join("gone.txt")).unwrap();
        fs::write(repo.path().join("new.txt"), "new\n").unwrap();
        fs::create_dir_all(repo.path().join("dir")).unwrap();
        fs::write(repo.path().join("dir/inner.txt"), "in\n").unwrap();
        let mut changes = open(&repo);
        assert!(!changes.is_loading());
        assert_eq!(changes.error(), None);
        assert_eq!(
            changes.head_branch(),
            Some(repo.repo.head().unwrap().shorthand().unwrap())
        );
        assert!(!changes.is_merging());
        assert!(!changes.is_detached());
        assert!(!changes.is_unborn());
        assert_eq!(
            listed(changes.unstaged()),
            [
                ('M', "a.rs", 1, 0),
                ('?', "dir/inner.txt", 1, 0),
                ('D', "gone.txt", 0, 1),
                ('?', "new.txt", 1, 0),
            ]
        );
        assert!(changes.staged().is_empty());
        assert!(!changes.is_clean());

        // The unstaged diff of a modified file is against the index; of
        // an untracked file, against nothing.
        let diff = changes.unstaged_diff(&changes.unstaged()[0]).unwrap();
        assert_eq!(diff.kind, ChangeKind::Modified);
        assert_eq!(lines(&diff), [" fn a() {}", "+fn b() {}"]);
        let diff = changes.unstaged_diff(&changes.unstaged()[3]).unwrap();
        assert_eq!(diff.kind, ChangeKind::Untracked);
        assert_eq!(lines(&diff), ["+new"]);
        let diff = changes.unstaged_diff(&changes.unstaged()[2]).unwrap();
        assert_eq!(lines(&diff), ["-bye"]);

        // Stage two of them: they move lists, and the staged diff is
        // against HEAD.
        changes.stage(["a.rs", "gone.txt"]).unwrap();
        confirmed(&mut changes);
        assert_eq!(
            listed(changes.unstaged()),
            [('?', "dir/inner.txt", 1, 0), ('?', "new.txt", 1, 0)]
        );
        assert_eq!(
            listed(changes.staged()),
            [('M', "a.rs", 1, 0), ('D', "gone.txt", 0, 1)]
        );
        let diff = changes.staged_diff(&changes.staged()[0]).unwrap();
        assert_eq!(lines(&diff), [" fn a() {}", "+fn b() {}"]);
        // A further edit shows up unstaged while the staged part stays.
        fs::write(
            repo.path().join("a.rs"),
            "fn a() {}\nfn b() {}\nfn c() {}\n",
        )
        .unwrap();
        changes.refresh();
        settle(&mut changes);
        assert_eq!(listed(changes.unstaged())[0], ('M', "a.rs", 1, 0));
        assert_eq!(listed(changes.staged())[0], ('M', "a.rs", 1, 0));
        let diff = changes.unstaged_diff(&changes.unstaged()[0]).unwrap();
        assert_eq!(lines(&diff), [" fn a() {}", " fn b() {}", "+fn c() {}"]);

        // Unstage one; stage all; unstage all; stage all again.
        changes.unstage(["gone.txt"]).unwrap();
        confirmed(&mut changes);
        assert_eq!(listed(changes.staged()), [('M', "a.rs", 1, 0)]);
        assert!(changes.unstaged().iter().any(|c| c.path == "gone.txt"));
        changes.stage_all().unwrap();
        confirmed(&mut changes);
        assert!(changes.unstaged().is_empty());
        assert_eq!(changes.staged().len(), 4);
        changes.unstage_all().unwrap();
        confirmed(&mut changes);
        assert!(changes.staged().is_empty());
        assert_eq!(changes.unstaged().len(), 4);
        changes.stage_all().unwrap();
        confirmed(&mut changes);

        // Commit: the tree is clean, and HEAD moved on from the base.
        assert!(changes.commit("   ").is_err());
        let id = changes.commit("Change things\n").unwrap();
        confirmed(&mut changes);
        assert!(changes.is_clean());
        let commit = repo.repo.find_commit(id).unwrap();
        assert_eq!(commit.message().unwrap(), "Change things");
        assert_eq!(commit.parent_id(0).unwrap(), base);
        assert_eq!(repo.repo.head().unwrap().target(), Some(id));
        assert!(
            changes.commit("Nothing").is_err(),
            "nothing staged to commit"
        );
    }

    #[test]
    fn renames_are_found_among_staged_changes() {
        let mut repo = TestRepo::new();
        repo.commit(&[("old.txt", "same\nlines\nhere\n")], "Base", &[]);
        fs::rename(repo.path().join("old.txt"), repo.path().join("new.txt")).unwrap();
        let mut changes = open(&repo);
        assert_eq!(
            listed(changes.unstaged()),
            [('?', "new.txt", 3, 0), ('D', "old.txt", 0, 3)]
        );
        changes.stage(["new.txt", "old.txt"]).unwrap();
        confirmed(&mut changes);
        assert_eq!(listed(changes.staged()), [('R', "new.txt", 0, 0)]);
        assert_eq!(changes.staged()[0].old_path.as_deref(), Some("old.txt"));
        let diff = changes.staged_diff(&changes.staged()[0]).unwrap();
        assert_eq!(diff.kind, ChangeKind::Renamed);
        assert!(!diff.has_hunks());
        changes.unstage_all().unwrap();
        confirmed(&mut changes);
        assert!(changes.staged().is_empty());
        assert_eq!(changes.unstaged().len(), 2);
    }

    #[test]
    fn a_merge_conflict_is_listed_resolved_and_committed() {
        let mut repo = TestRepo::new();
        configure_user(&repo);
        let base = repo.commit(&[("f.txt", "one\ntwo\nthree\n")], "Base", &[]);
        let main = repo.repo.head().unwrap().shorthand().unwrap().to_owned();
        let ours = repo.commit(&[("f.txt", "one\nours\nthree\n")], "Ours", &[base]);
        repo.branch("side", base);
        repo.checkout("side");
        let theirs = repo.commit(&[("f.txt", "one\ntheirs\nthree\n")], "Theirs", &[base]);
        repo.checkout(&main);
        // The test repository's checkout only moves HEAD: bring the
        // working directory along.
        repo.repo
            .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        // Merge `side` into `main`, as `git merge side` does, leaving the
        // conflict in the index and the working directory.
        let annotated = repo.repo.find_annotated_commit(theirs).unwrap();
        repo.repo.merge(&[&annotated], None, None).unwrap();
        assert_eq!(repo.repo.state(), RepositoryState::Merge);
        fs::write(repo.path().join(".git/MERGE_MSG"), "Merge branch 'side'\n").unwrap();

        let mut changes = open(&repo);
        assert!(changes.is_merging());
        assert_eq!(changes.merge_message(), Some("Merge branch 'side'"));
        assert_eq!(changes.conflict_count(), 1);
        assert_eq!(listed(changes.unstaged()), [('U', "f.txt", 0, 0)]);
        assert!(changes.staged().is_empty());
        let diff = changes.unstaged_diff(&changes.unstaged()[0]).unwrap();
        assert_eq!(diff.kind, ChangeKind::Conflicted);
        let shown = lines(&diff);
        assert!(shown.contains(&" one".to_owned()), "{shown:?}");
        assert!(shown.contains(&"+<<<<<<< HEAD".to_owned()), "{shown:?}");
        assert!(shown.contains(&" ours".to_owned()), "{shown:?}");
        assert!(shown.contains(&"+theirs".to_owned()), "{shown:?}");
        assert!(changes.commit("Merge").is_err(), "conflicts remain");

        // Resolve it, stage it, and commit the merge.
        fs::write(repo.path().join("f.txt"), "one\nours and theirs\nthree\n").unwrap();
        changes.refresh();
        settle(&mut changes);
        assert_eq!(listed(changes.unstaged()), [('U', "f.txt", 0, 0)]);
        changes.stage(["f.txt"]).unwrap();
        confirmed(&mut changes);
        assert_eq!(changes.conflict_count(), 0);
        assert_eq!(listed(changes.staged()), [('M', "f.txt", 1, 1)]);
        assert!(changes.unstaged().is_empty());
        let id = changes.commit("Merge branch 'side'").unwrap();
        confirmed(&mut changes);
        let commit = repo.repo.find_commit(id).unwrap();
        let parents: Vec<Oid> = commit.parent_ids().collect();
        assert_eq!(parents, vec![ours, theirs]);
        assert_eq!(repo.repo.state(), RepositoryState::Clean);
        assert!(!changes.is_merging());
        assert!(changes.is_clean());
    }

    #[test]
    fn submodules_with_changes_are_listed_and_staged_as_commits() {
        let mut repo = TestRepo::new();
        configure_user(&repo);
        repo.commit(&[("a.txt", "one\n")], "Base", &[]);
        let sub = repo.add_submodule("libs/sub");
        repo.add_submodule("libs/clean");
        let mut changes = open(&repo);
        assert!(changes.is_clean());
        assert!(changes.changed_submodules().is_empty());

        // A change inside the submodule makes it a changed submodule,
        // and the parent sees it as modified.
        fs::write(sub.workdir().unwrap().join("inner.txt"), "changed\n").unwrap();
        changes.refresh();
        settle(&mut changes);
        let paths: Vec<&str> = changes
            .changed_submodules()
            .iter()
            .map(|s| s.path.as_str())
            .collect();
        assert_eq!(paths, ["libs/sub"]);
        assert_eq!(listed(changes.unstaged()), [('M', "libs/sub", 0, 0)]);
        assert!(changes.unstaged()[0].submodule);
        let diff = changes.unstaged_diff(&changes.unstaged()[0]).unwrap();
        assert_eq!(diff.unshown, Some(Unshown::Submodule));
        // Still at the same commit: the change is inside the submodule,
        // so there are no commits to list.
        let first_inner = sub.head().unwrap().target().unwrap();
        let range = diff.submodule.as_ref().unwrap();
        assert_eq!(
            (range.old, range.new),
            (Some(first_inner), Some(first_inner))
        );
        assert!(range.commits.is_empty());

        // Commit inside the submodule, from its own page.
        let mut inner = Changes::open_repository(sub.workdir().unwrap()).unwrap();
        settle(&mut inner);
        assert_eq!(listed(inner.unstaged()), [('M', "inner.txt", 1, 1)]);
        inner.stage_all().unwrap();
        settle(&mut inner);
        {
            let mut config = sub.config().unwrap();
            config.set_str("user.name", "Sub Author").unwrap();
            config.set_str("user.email", "sub@example.com").unwrap();
        }
        let inner_commit = inner.commit("Inner change").unwrap();
        settle(&mut inner);
        assert!(inner.is_clean());

        // The parent now has the submodule's new commit to stage and
        // commit; the submodule itself is clean, so it is no longer a
        // changed one.
        changes.refresh();
        settle(&mut changes);
        assert!(changes.changed_submodules().is_empty());
        assert_eq!(listed(changes.unstaged()), [('M', "libs/sub", 0, 0)]);
        // Unstaged: the index's commit to the submodule's HEAD.
        let diff = changes.unstaged_diff(&changes.unstaged()[0]).unwrap();
        let range = diff.submodule.as_ref().unwrap();
        assert_eq!(
            (range.old, range.new),
            (Some(first_inner), Some(inner_commit))
        );
        let names: Vec<&str> = range.commits.iter().map(|c| c.summary.as_str()).collect();
        assert_eq!(names, ["Inner change", "Inner commit"]);
        changes.stage(["libs/sub"]).unwrap();
        confirmed(&mut changes);
        assert_eq!(listed(changes.staged()), [('M', "libs/sub", 0, 0)]);
        assert!(changes.unstaged().is_empty());
        // Staged: HEAD's commit to the index's.
        let diff = changes.staged_diff(&changes.staged()[0]).unwrap();
        assert_eq!(diff.unshown, Some(Unshown::Submodule));
        let range = diff.submodule.as_ref().unwrap();
        assert_eq!(
            (range.old, range.new),
            (Some(first_inner), Some(inner_commit))
        );
        assert_eq!(range.commits.len(), 2);
        let id = changes.commit("Update submodule").unwrap();
        confirmed(&mut changes);
        assert!(changes.is_clean());
        let tree = repo.repo.find_commit(id).unwrap().tree().unwrap();
        let entry = tree.get_path(Path::new("libs/sub")).unwrap();
        assert_eq!(entry.id(), inner_commit);
    }

    #[test]
    fn amending_shows_heads_changes_as_staged_and_replaces_it() {
        let mut repo = TestRepo::new();
        configure_user(&repo);
        let base = repo.commit(&[("a.txt", "one\n"), ("b.txt", "b\n")], "Base", &[]);
        let head = repo.commit(
            &[("a.txt", "one\ntwo\n"), ("c.txt", "c\n")],
            "Add two\n\nWith a body.\n",
            &[base],
        );
        let mut changes = open(&repo);
        assert!(!changes.is_amending());
        assert_eq!(changes.head_message(), Some("Add two\n\nWith a body."));
        assert!(changes.is_clean());
        // Amending: HEAD's changes are the staged list, against its
        // parent, and its diff reads that way.
        changes.set_amend(true).unwrap();
        settle(&mut changes);
        assert!(changes.is_amending());
        assert_eq!(
            listed(changes.staged()),
            [('M', "a.txt", 1, 0), ('A', "c.txt", 1, 0)]
        );
        let diff = changes.staged_diff(&changes.staged()[0]).unwrap();
        assert_eq!(lines(&diff), [" one", "+two"]);
        // A new staged change joins them; unstaging c.txt puts it back
        // to the parent's version (none), so it turns up unstaged.
        fs::write(repo.path().join("b.txt"), "bee\n").unwrap();
        changes.stage(["b.txt"]).unwrap();
        confirmed(&mut changes);
        assert_eq!(
            listed(changes.staged()),
            [
                ('M', "a.txt", 1, 0),
                ('M', "b.txt", 1, 1),
                ('A', "c.txt", 1, 0)
            ]
        );
        changes.unstage(["c.txt"]).unwrap();
        confirmed(&mut changes);
        assert_eq!(
            listed(changes.staged()),
            [('M', "a.txt", 1, 0), ('M', "b.txt", 1, 1)]
        );
        assert_eq!(listed(changes.unstaged()), [('?', "c.txt", 1, 0)]);
        // The commit replaces HEAD: same parent, new tree and message,
        // and amending is off again.
        let id = changes.commit("Add two and bee").unwrap();
        confirmed(&mut changes);
        assert_ne!(id, head);
        assert!(!changes.is_amending());
        let commit = repo.repo.find_commit(id).unwrap();
        assert_eq!(commit.parent_id(0).unwrap(), base);
        assert_eq!(commit.message().unwrap(), "Add two and bee");
        assert!(commit.tree().unwrap().get_path(Path::new("c.txt")).is_err());
        assert_eq!(repo.repo.head().unwrap().target(), Some(id));
        assert_eq!(listed(changes.unstaged()), [('?', "c.txt", 1, 0)]);
        assert_eq!(changes.head_message(), Some("Add two and bee"));
        // Amending with nothing staged is allowed: the message alone
        // can change. Turning amend off restores the plain view.
        changes.set_amend(true).unwrap();
        settle(&mut changes);
        assert_eq!(listed(changes.staged()).len(), 2);
        changes.set_amend(false).unwrap();
        settle(&mut changes);
        assert!(changes.staged().is_empty());
    }

    #[test]
    fn an_empty_repository_and_a_directory_that_is_not_one() {
        let repo = TestRepo::new();
        let mut changes = open(&repo);
        assert!(changes.is_unborn());
        assert!(changes.head_branch().is_some());
        assert!(changes.is_clean());
        fs::write(repo.path().join("first.txt"), "hi\n").unwrap();
        changes.refresh();
        settle(&mut changes);
        assert_eq!(listed(changes.unstaged()), [('?', "first.txt", 1, 0)]);
        assert!(changes.commit("Nothing staged").is_err());
        changes.stage_all().unwrap();
        confirmed(&mut changes);
        assert_eq!(listed(changes.staged()), [('A', "first.txt", 1, 0)]);
        // Unstaging from an unborn branch empties the index again.
        changes.unstage_all().unwrap();
        confirmed(&mut changes);
        assert!(changes.staged().is_empty());
        assert_eq!(listed(changes.unstaged()), [('?', "first.txt", 1, 0)]);
        let dir = tempfile::tempdir().unwrap();
        assert!(Changes::open(dir.path()).is_err());
        // Nothing to amend on an unborn branch.
        assert!(changes.set_amend(true).is_err());
        assert!(!changes.is_amending());
    }
}
