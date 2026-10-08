//! The uncommitted changes of a working tree, as `git status` lists
//! them, and the steps of committing them: staging, unstaging, and the
//! commit itself, and throwing away what isn't wanted. The changes page
//! of a frontend is built on this.
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
//! [`submodules`](mod@super::submodules) module).
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
//! A merge or rebase that stopped at conflicts (see the
//! [`merge`](super::merge) and [`rebase`](super::rebase) modules) is
//! finished here once they are resolved and staged: a merge by
//! [`commit`](Changes::commit), a rebase by
//! [`continue_rebase`](Changes::continue_rebase), which commits the
//! commit it stopped at and replays the rest. An interactive rebase
//! (see the [`interactive`] module) is carried on
//! by [`continue_rebase`](Changes::continue_rebase) too, which there
//! commits what is staged at the stop and goes on once nothing is left
//! unstaged. [`rebase`](Changes::rebase) says what a rebase in progress
//! is doing. Any of them is given up with [`abort`](Changes::abort),
//! which puts things back as they were before it started.
//!
//! A conflict can also be settled wholesale by
//! [`resolve`](Changes::resolve), which takes one side's version of the
//! file, ours or theirs (see [`ConflictSide`]), as `git checkout
//! --ours` and then `git add` do.
//!
//! Some of a file's changed lines, rather than the whole file, are
//! staged, unstaged, or reverted by [`apply_lines`](Changes::apply_lines),
//! with what the file's diff works out for them (see
//! [`FileDiff::stage_lines`] and the two after it), as `git add -p`,
//! `git reset -p`, and `git checkout -p` do. The diff may be out of date
//! by then (the file edited since, say), so nothing is written unless
//! the index or the working directory still holds what the diff saw.
//!
//! [`discard`](Changes::discard) throws away unstaged changes, putting
//! files back to the index's version (or deleting them, when untracked):
//! it touches the working directory and nothing else, and can't be
//! undone. It, reverting lines, and [`resolve`](Changes::resolve), which
//! writes over what the working directory has for a conflicted file,
//! are the actions here that lose work.
//!
//! A commit runs the repository's hooks as `git commit` does (see the
//! [`hooks`](super::hooks) module). Since they may take a while, a
//! commit with any to run is made on a thread of its own, which
//! [`Changes::poll`] takes the outcome of, and every other action is
//! refused meanwhile.
//!
//! Every action ([`stage`](Changes::stage), [`unstage`](Changes::unstage),
//! [`discard`](Changes::discard), [`resolve`](Changes::resolve), and
//! [`commit`](Changes::commit), when it has no hooks to run)
//! changes the repository on the caller's thread and updates the lists
//! right away with what it did: staging, unstaging, discarding, and
//! resolving compare just the files they touched again, which is quick however
//! big the tree, and a commit empties the
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
//! [`FileDiff`] like a commit's (see the [`diff`] module).
//! A conflicted file is shown against our side of the merge, so the
//! diff is what the conflict markers and their side's lines add to it.

use super::checkout::follow_gitlink;
use super::diff::{
    self, CONTEXT_LINES, ChangeKind, Contents, FileChange, FileDiff, LinesChange, LinesTarget,
    STATS_LIMIT, Unshown,
};
use super::hooks::{Hook, HookError, Hooks, commit_env};
use super::interactive::{self, Continued};
use super::operation::{InProgress, OperationError};
use super::rebase::{RebaseStatus, rebase_status};
use super::submodule_conflicts::resolve_submodule_conflicts;
use super::submodules::{Submodule, changed_submodules, submodule_range, uncommitted_changes};
use git2::{
    Delta, DiffFindOptions, DiffOptions, ErrorCode, FileMode, Index, IndexEntry, Oid, Patch,
    Repository, RepositoryState, Tree,
};
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

/// The bits of an index entry's flags that hold its stage: 0 for a
/// path not in conflict, and 1 to 3 for the ancestor, ours, and theirs
/// of one that is.
const STAGE_MASK: u16 = 0x3000;

/// A side of a merge or rebase stopped at conflicts, whose version of a
/// conflicted file [`Changes::resolve`] takes. These are git's own
/// names, which mean what HEAD is on at the time: in a rebase, that is
/// the commit being replayed onto, not the branch being rebased.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictSide {
    /// HEAD's side: in a merge, the branch being merged into; in a
    /// rebase, what it is rebasing onto, with the commits replayed so
    /// far.
    Ours,
    /// The incoming side: in a merge, the commit being merged in; in a
    /// rebase, the commit being replayed.
    Theirs,
}

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
    rebase: Option<RebaseStatus>,
    head_id: Option<Oid>,
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
    rebase: Option<RebaseStatus>,
    head_id: Option<Oid>,
    head_message: Option<String>,
    conflicts: usize,
    /// Whether the commit will replace HEAD.
    amend: bool,
    /// The commit being made on its thread, while its hooks run, and
    /// how it went, until taken.
    committing: Option<Receiver<Result<Committed, CommitError>>>,
    commit_result: Option<Result<Committed, CommitError>>,
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
            rebase: None,
            head_id: None,
            head_message: None,
            conflicts: 0,
            amend: false,
            committing: None,
            commit_result: None,
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
        // A stop git's command line made to edit a commit is made the
        // editor's kind first, so that the scan finds the commit's
        // changes staged. One that can't be is shown as git left it,
        // and continuing tries again, saying why if it fails.
        let _ = interactive::adopt_stop(&self.repo);
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
        let mut changed = self.poll_commit();
        let Some(receiver) = &self.receiver else {
            return changed;
        };
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
                self.rebase = snapshot.rebase;
                self.head_id = snapshot.head_id;
                self.head_message = snapshot.head_message;
                self.conflicts = snapshot.conflicts;
                self.error = None;
            }
            Err(message) => self.error = Some(message),
        }
    }

    /// Poll until the scan is done, and any commit being made, or
    /// `timeout` passes. Returns whether they finished.
    pub fn wait(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.poll();
            if !self.loading && self.committing.is_none() {
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
        self.ensure_idle()?;
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
            if matches!(
                self.repo.state(),
                RepositoryState::RebaseMerge | RepositoryState::RebaseInteractive
            ) {
                return Err(git2::Error::from_str(
                    "a rebase in progress can't amend: continue it first",
                ));
            }
        }
        self.amend = amend;
        self.refresh();
        Ok(())
    }

    /// The commit HEAD is at, if any.
    pub fn head_id(&self) -> Option<Oid> {
        self.head_id
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

    /// What the rebase in progress is doing, if one is: its commit
    /// that stopped at conflicts is committed and the rest replayed by
    /// [`continue_rebase`](Self::continue_rebase), not by a commit; as
    /// is what is staged at an interactive rebase's stop.
    pub fn rebase(&self) -> Option<&RebaseStatus> {
        self.rebase.as_ref()
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

    /// The repository's index as it is on disk now. libgit2 keeps the
    /// one it last read, which something else may have written since
    /// (a merge or rebase run in the background, a `git` command in a
    /// shell), and an action on that one would write it back over what
    /// is there.
    fn index(&self) -> Result<Index, git2::Error> {
        let mut index = self.repo.index()?;
        index.read(false)?;
        Ok(index)
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
            return Ok(self.submodule_diff(change, None, &delta));
        }
        let old = diff::blob_contents(&self.repo, delta.old_file())?;
        let new = diff::workdir_contents(&self.workdir.join(&change.path));
        let patch = Patch::from_diff(&diff, index)?;
        FileDiff::build(change.kind, &change.path, None, &old, &new, patch.as_ref())
    }

    /// The diff of a submodule change: the commits it moved over, and
    /// the uncommitted changes inside the submodule, which are as much
    /// a part of what the working tree holds as the move.
    fn submodule_diff(
        &self,
        change: &FileChange,
        old_path: Option<&str>,
        delta: &git2::DiffDelta<'_>,
    ) -> FileDiff {
        let mut diff = diff::submodule_diff(&self.repo, change.kind, &change.path, old_path, delta);
        if let Some(range) = &mut diff.submodule {
            range.uncommitted = uncommitted_changes(&self.workdir.join(&change.path));
        }
        diff
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
            return Ok(self.submodule_diff(change, change.old_path.as_deref(), &delta));
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
    /// the merge (or nothing, when our side deleted it). A submodule in
    /// conflict has no file to compare: its diff is the commits of the
    /// two sides, from ours to theirs, which the submodule's own
    /// repository has.
    fn conflict_diff(&self, change: &FileChange) -> Result<FileDiff, git2::Error> {
        let index = self.index()?;
        let conflict = index.conflicts()?.filter_map(Result::ok).find(|conflict| {
            [&conflict.our, &conflict.their, &conflict.ancestor]
                .into_iter()
                .flatten()
                .any(|entry| entry.path == change.path.as_bytes())
        });
        let gitlink = |entry: &Option<git2::IndexEntry>| {
            entry
                .as_ref()
                .filter(|entry| entry.mode == 0o160000)
                .map(|entry| entry.id)
        };
        if let Some(conflict) = &conflict {
            let (ours, theirs) = (gitlink(&conflict.our), gitlink(&conflict.their));
            if ours.is_some() || theirs.is_some() {
                let mut diff =
                    FileDiff::unshown(change.kind, &change.path, None, Unshown::Submodule);
                diff.submodule = Some(submodule_range(&self.repo, &change.path, ours, theirs));
                return Ok(diff);
            }
        }
        let ours = conflict.and_then(|conflict| conflict.our);
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
        self.ensure_idle()?;
        let paths: Vec<&str> = paths.into_iter().collect();
        let mut index = self.index()?;
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
        self.ensure_idle()?;
        let paths: Vec<&str> = paths.into_iter().collect();
        {
            // Which libgit2 changes as it last read it.
            self.index()?;
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
        let unstaged = unstaged_changes(&diff, self.unstaged.len() <= STATS_LIMIT)?;
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

    /// Discard the unstaged changes of `paths`, putting the files in
    /// the working directory back to the index's version (`git restore
    /// -- path`): what is staged, or with nothing staged, what is
    /// committed. A deleted file comes back; an untracked file has no
    /// version to go back to, so it is deleted, along with any
    /// directories that leaves empty. A path with no unstaged change is
    /// left alone. The index is untouched, so what is staged stays
    /// staged.
    ///
    /// Refused, with nothing changed, when a path is in conflict (there
    /// is no one version to go back to: resolve it, or stage it) or is
    /// a submodule (its change is the commit checked out in it, or the
    /// changes inside it, which are the submodule's own to discard).
    /// There is no undoing a discard: a frontend should confirm it.
    pub fn discard<'a>(
        &mut self,
        paths: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), git2::Error> {
        self.ensure_idle()?;
        let paths: Vec<&str> = paths.into_iter().collect();
        if paths.is_empty() {
            return Ok(());
        }
        // What the paths are now, not what the last scan said: the
        // working directory may have changed since.
        let mut restore = Vec::new();
        let mut remove = Vec::new();
        let diff = workdir_diff(&self.repo, &paths)?;
        for change in unstaged_changes(&diff, false)? {
            if change.kind == ChangeKind::Conflicted {
                return Err(git2::Error::from_str(&format!(
                    "{} is in conflict: resolve it instead",
                    change.path
                )));
            }
            if change.submodule {
                return Err(git2::Error::from_str(&format!(
                    "{} is a submodule: discard its changes on its own page",
                    change.path
                )));
            }
            if change.kind == ChangeKind::Untracked {
                remove.push(change.path);
            } else {
                restore.push(change.path);
            }
        }
        drop(diff);
        let result = self.restore_and_remove(&restore, &remove);
        // Whatever was done before a failure shows in the lists too.
        self.rescan(&paths);
        self.confirm();
        result
    }

    /// Stage, unstage, or revert some of a file's changed lines, as its
    /// diff worked out (see [`FileDiff::stage_lines`] and the two after
    /// it): write what the change holds for the file to the index, or to
    /// the working directory, and update the lists as staging does.
    ///
    /// Refused, with nothing changed, when the index or the working
    /// directory no longer holds what the diff saw: the file was edited,
    /// staged, or unstaged since. A file the index doesn't have (an
    /// untracked one being staged, or one whose deletion is staged
    /// being unstaged) gets an entry, executable or not as the working
    /// directory's file is, or as HEAD has it. Reverting can't be
    /// undone: a frontend should confirm it.
    pub fn apply_lines(&mut self, change: &LinesChange) -> Result<(), git2::Error> {
        self.ensure_idle()?;
        let path = change.path.as_str();
        let stale = || {
            git2::Error::from_str(&format!(
                "{path} changed since its diff was shown: look again"
            ))
        };
        match change.target {
            LinesTarget::Index => {
                // As the index is on disk now, which the diff may be older
                // than.
                let mut index = self.index()?;
                let entry = index.get_path(Path::new(path), 0);
                let now = match &entry {
                    Some(entry) => self.repo.find_blob(entry.id)?.content().to_vec(),
                    None => Vec::new(),
                };
                if now != change.expected {
                    return Err(stale());
                }
                let entry = match entry {
                    Some(entry) => entry,
                    None => self.new_index_entry(&mut index, path)?,
                };
                index.add_frombuffer(&entry, &change.contents)?;
                index.write()?;
            }
            LinesTarget::WorkingTree => {
                let file = self.workdir.join(path);
                let now = match fs::read(&file) {
                    Ok(bytes) => bytes,
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                    Err(err) => {
                        return Err(git2::Error::from_str(&format!(
                            "could not read {path}: {err}"
                        )));
                    }
                };
                if now != change.expected {
                    return Err(stale());
                }
                if let Some(parent) = file.parent() {
                    fs::create_dir_all(parent).ok();
                }
                fs::write(&file, &change.contents).map_err(|err| {
                    git2::Error::from_str(&format!("could not write {path}: {err}"))
                })?;
            }
        }
        self.rescan(&[path]);
        self.confirm();
        Ok(())
    }

    /// An index entry for a file the index doesn't have, to put lines
    /// of it in: as staging the working directory's file would make it,
    /// or else as HEAD (or the commit amended) has the file.
    fn new_index_entry(&self, index: &mut Index, path: &str) -> Result<IndexEntry, git2::Error> {
        if self.workdir.join(path).is_file() {
            index.add_path(Path::new(path))?;
            if let Some(entry) = index.get_path(Path::new(path), 0) {
                return Ok(entry);
            }
        }
        let mode = self
            .base_commit()
            .and_then(|commit| commit.tree().ok())
            .and_then(|tree| tree.get_path(Path::new(path)).ok())
            .map_or(0o100644, |entry| entry.filemode() as u32);
        let time = git2::IndexTime::new(0, 0);
        Ok(IndexEntry {
            ctime: time,
            mtime: time,
            dev: 0,
            ino: 0,
            mode,
            uid: 0,
            gid: 0,
            file_size: 0,
            id: Oid::ZERO_SHA1,
            flags: path.len().min(0xfff) as u16,
            flags_extended: 0,
            path: path.as_bytes().to_vec(),
        })
    }

    /// The work of a discard: check `restore` out of the index over
    /// what the working directory has, and delete `remove`.
    fn restore_and_remove(&self, restore: &[String], remove: &[String]) -> Result<(), git2::Error> {
        // An empty list of paths would check out every file.
        if !restore.is_empty() {
            let mut checkout = git2::build::CheckoutBuilder::new();
            checkout.force().disable_pathspec_match(true);
            for path in restore {
                checkout.path(path);
            }
            self.repo.checkout_index(None, Some(&mut checkout))?;
        }
        for path in remove {
            let file = self.workdir.join(path);
            fs::remove_file(&file)
                .map_err(|err| git2::Error::from_str(&format!("could not delete {path}: {err}")))?;
            remove_empty_parents(&self.workdir, &file);
        }
        Ok(())
    }

    /// Resolve the conflicts of `paths` by taking `side`'s version of
    /// each, as `git checkout --ours -- path` (or `--theirs`) and then
    /// `git add` do: the file is written as that side has it (its
    /// contents, and whether it is executable or a symbolic link) and
    /// staged, which marks it resolved. A file that side deleted is
    /// deleted, and the deletion staged. A submodule is checked out at
    /// that side's commit and then staged there, so a checkout that
    /// fails (one that would lose changes in the submodule) stages
    /// nothing; one that side deleted is unstaged and its directory
    /// left alone. A path not in conflict is left alone.
    ///
    /// What the working directory had for a resolved file, conflict
    /// markers and any resolving done by hand, is lost: a frontend
    /// should confirm it.
    pub fn resolve<'a>(
        &mut self,
        paths: impl IntoIterator<Item = &'a str>,
        side: ConflictSide,
    ) -> Result<(), git2::Error> {
        self.ensure_idle()?;
        let paths: Vec<&str> = paths.into_iter().collect();
        if paths.is_empty() {
            return Ok(());
        }
        let result = self.take_side(&paths, side);
        // Whatever was resolved before a failure shows in the lists too.
        self.rescan(&paths);
        self.confirm();
        result
    }

    /// The work of a resolve: stage `side`'s entry of each conflict
    /// among `paths`, then bring the working directory to match. A
    /// failure partway still writes what was resolved before it, so
    /// that the index and the working directory agree.
    fn take_side(&self, paths: &[&str], side: ConflictSide) -> Result<(), git2::Error> {
        let gitlink = u32::from(FileMode::Commit);
        let mut index = self.index()?;
        let mut write = Vec::new();
        let mut remove = Vec::new();
        let mut failure = None;
        for &path in paths {
            let conflict = match index.conflict_get(Path::new(path)) {
                Ok(conflict) => conflict,
                Err(err) if err.code() == ErrorCode::NotFound => continue,
                Err(err) => return Err(err),
            };
            let submodule = [&conflict.ancestor, &conflict.our, &conflict.their]
                .into_iter()
                .flatten()
                .any(|entry| entry.mode == gitlink);
            let taken = match side {
                ConflictSide::Ours => conflict.our,
                ConflictSide::Theirs => conflict.their,
            };
            match taken {
                Some(entry) if entry.mode == gitlink => {
                    if let Err(why) = follow_gitlink(&self.repo, path, entry.id, &mut |_, _| {}) {
                        failure = Some(git2::Error::from_str(&why.to_string()));
                        break;
                    }
                    stage_side(&mut index, path, entry)?;
                }
                Some(entry) => {
                    stage_side(&mut index, path, entry)?;
                    write.push(path.to_owned());
                }
                None => {
                    index.remove_path(Path::new(path))?;
                    let file = fs::symlink_metadata(self.workdir.join(path));
                    if !submodule && file.is_ok_and(|file| !file.is_dir()) {
                        remove.push(path.to_owned());
                    }
                }
            }
        }
        index.write()?;
        self.restore_and_remove(&write, &remove)?;
        failure.map_or(Ok(()), Err)
    }

    /// Commit what is staged with `message`, on the current branch (or
    /// as a detached commit), as the person the repository's
    /// configuration names. A merge in progress is finished: the merged
    /// commit is the second parent, and the merge state is cleared.
    /// When amending, HEAD is replaced by a commit with its parents,
    /// the staged tree, and the message. Refuses an empty message,
    /// unresolved conflicts, and a commit that would change nothing
    /// (unless it finishes a merge or amends).
    ///
    /// The repository's hooks run around it as around `git commit`
    /// (`pre-commit`, `prepare-commit-msg`, `commit-msg`, then
    /// `post-commit`), with `hooks`. With none to run, the commit
    /// is made at once and returned. Otherwise it is made on a thread
    /// of its own, since hooks may take a while, and `None` returned:
    /// [`poll`](Self::poll) takes in how it went, for
    /// [`take_commit`](Self::take_commit); until then
    /// [`is_committing`](Self::is_committing) says so, and the other
    /// actions are refused. Either way the lists follow as a commit made
    /// here does.
    pub fn commit(
        &mut self,
        message: &str,
        hooks: &Hooks,
    ) -> Result<Option<Committed>, CommitError> {
        self.ensure_idle()?;
        if !COMMIT_HOOKS
            .iter()
            .any(|&hook| hooks.will_run(&self.repo, hook))
        {
            let committed = make_commit(&self.repo, message, self.amend, hooks)?;
            self.follow_commit(&committed);
            return Ok(Some(committed));
        }
        // Whatever would refuse the commit does so now, rather than
        // after a hook has run for nothing.
        check_commit(&self.repo, message, self.amend)?;
        let git_dir = self.repo.path().to_path_buf();
        let message = message.to_owned();
        let amend = self.amend;
        let hooks = hooks.clone();
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("git-commit".to_owned())
            .spawn(move || {
                let result = Repository::open(&git_dir)
                    .map_err(CommitError::from)
                    .and_then(|repo| make_commit(&repo, &message, amend, &hooks));
                let _ = sender.send(result);
            })
            .map_err(|err| git2::Error::from_str(&err.to_string()))?;
        self.committing = Some(receiver);
        Ok(None)
    }

    /// Whether a commit is being made on its thread (see
    /// [`commit`](Self::commit)), as of the last poll.
    pub fn is_committing(&self) -> bool {
        self.committing.is_some()
    }

    /// How the commit made on its thread went, once
    /// [`poll`](Self::poll) has taken it in. Taking it clears it.
    pub fn take_commit(&mut self) -> Option<Result<Committed, CommitError>> {
        self.commit_result.take()
    }

    /// Take in how the commit on its thread went, if it is done.
    /// Returns whether it is.
    fn poll_commit(&mut self) -> bool {
        let Some(receiver) = &self.committing else {
            return false;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                Err(git2::Error::from_str("the commit stopped").into())
            }
        };
        self.committing = None;
        match &result {
            Ok(committed) => self.follow_commit(committed),
            // The hooks may have changed files or staged them before
            // one failed.
            Err(_) => self.refresh(),
        }
        self.commit_result = Some(result);
        true
    }

    /// Refuse an action while a commit is being made on its thread.
    fn ensure_idle(&self) -> Result<(), git2::Error> {
        if self.committing.is_some() {
            return Err(git2::Error::from_str(
                "a commit is being made; wait for its hooks to finish",
            ));
        }
        Ok(())
    }

    /// Show what a commit made: the commit holds what was staged, and
    /// is now what a commit would replace.
    fn follow_commit(&mut self, committed: &Committed) {
        // An amended commit is made; the next one follows it.
        self.amend = false;
        self.staged.clear();
        self.unborn = false;
        self.merging = false;
        self.merge_message = None;
        self.head_message = Some(committed.message.clone());
        self.confirm();
    }

    /// Go on with the rebase in progress, now that the conflicts of the
    /// commit it stopped at are resolved and staged: commit that commit
    /// with `message` (its own when empty), keeping its author, and
    /// replay the rest, which may stop at conflicts again (see the
    /// [`rebase`](super::rebase) module). An interactive rebase commits
    /// what is staged instead, if anything, as the stopped commit's
    /// author, and goes on only once nothing is left unstaged (see
    /// [`interactive::proceed`]). The lists scan again after.
    pub fn continue_rebase(&mut self, message: &str) -> Result<Continued, OperationError> {
        self.ensure_idle()?;
        let continued = self.go_on(message);
        self.refresh();
        continued
    }

    /// The work of [`continue_rebase`](Self::continue_rebase).
    fn go_on(&self, message: &str) -> Result<Continued, OperationError> {
        self.index()?;
        if self.repo.state() == RepositoryState::RebaseInteractive {
            return interactive::proceed(&self.repo, message);
        }
        let outcome = super::rebase::continue_rebase(&self.repo, Some(message))?;
        Ok(Continued {
            committed: None,
            outcome: Some(outcome),
        })
    }

    /// Give up the merge or rebase in progress, putting the branch, the
    /// index, and the working tree back as they were before it began.
    /// Returns which it was. The lists scan again after.
    pub fn abort(&mut self) -> Result<InProgress, OperationError> {
        self.ensure_idle()?;
        self.index()?;
        let aborted = match self.repo.state() {
            RepositoryState::Merge => {
                super::merge::abort_merge(&self.repo).map(|()| InProgress::Merge)
            }
            RepositoryState::RebaseMerge => {
                super::rebase::abort_rebase(&self.repo).map(|()| InProgress::Rebase)
            }
            RepositoryState::RebaseInteractive => {
                interactive::abort(&self.repo).map(|()| InProgress::Rebase)
            }
            _ => Err(OperationError::NothingInProgress),
        };
        self.amend = false;
        self.refresh();
        aborted
    }
}

/// A commit made by [`Changes::commit`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Committed {
    pub id: Oid,
    /// The message it was made with, which the hooks may have changed.
    pub message: String,
    /// Whether it replaced HEAD.
    pub amended: bool,
}

/// Why a commit wasn't made.
#[derive(Debug)]
pub enum CommitError {
    /// A hook stopped it.
    Hook(HookError),
    Git(git2::Error),
}

impl From<git2::Error> for CommitError {
    fn from(err: git2::Error) -> CommitError {
        CommitError::Git(err)
    }
}

impl From<HookError> for CommitError {
    fn from(err: HookError) -> CommitError {
        CommitError::Hook(err)
    }
}

impl fmt::Display for CommitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommitError::Hook(err) => err.fmt(f),
            CommitError::Git(err) => f.write_str(err.message()),
        }
    }
}

impl std::error::Error for CommitError {}

/// The hooks a commit runs, any of which sends it to its thread.
const COMMIT_HOOKS: [Hook; 4] = [
    Hook::PreCommit,
    Hook::PrepareCommitMsg,
    Hook::CommitMsg,
    Hook::PostCommit,
];

/// What a commit's checks found to commit.
struct CommitPlan {
    /// Whether it finishes a merge in progress, and the commits merged,
    /// the parents after HEAD.
    merging: bool,
    merged: Vec<Oid>,
}

/// Whether a commit of what is staged, with `message`, can be made (see
/// [`Changes::commit`]), before any hook runs. A merge's submodule
/// conflicts that have become resolvable are resolved.
fn check_commit(repo: &Repository, message: &str, amend: bool) -> Result<CommitPlan, git2::Error> {
    if message.trim().is_empty() {
        return Err(git2::Error::from_str("a commit needs a message"));
    }
    if matches!(
        repo.state(),
        RepositoryState::RebaseMerge | RepositoryState::RebaseInteractive
    ) {
        return Err(git2::Error::from_str(
            "a rebase is in progress: continue it rather than commit",
        ));
    }
    let merging = repo.state() == RepositoryState::Merge;
    let mut merged = Vec::new();
    if merging {
        // As `mergehead_foreach` would list them, which wants the
        // repository to itself.
        let heads = fs::read_to_string(repo.path().join("MERGE_HEAD"))
            .map_err(|err| git2::Error::from_str(&format!("MERGE_HEAD: {err}")))?;
        for line in heads.lines().map(str::trim).filter(|line| !line.is_empty()) {
            merged.push(Oid::from_str(line)?);
        }
    }
    let mut index = repo.index()?;
    index.read(false)?;
    if index.has_conflicts() && merging {
        // A submodule conflict that has become resolvable since the
        // merge stopped (its own branch rebased since, as the stop said
        // to) is resolved now.
        resolve_submodule_conflicts(repo, &mut |_, _| {})?;
        index.read(true)?;
    }
    if index.has_conflicts() {
        return Err(git2::Error::from_str(
            "resolve the conflicts and stage the files first",
        ));
    }
    let tree_id = index.write_tree()?;
    let unchanged = match head_commit(repo) {
        Some(head) => head.tree_id() == tree_id,
        None => index.is_empty(),
    };
    if unchanged && !merging && !amend {
        return Err(git2::Error::from_str("nothing is staged to commit"));
    }
    Ok(CommitPlan { merging, merged })
}

fn head_commit(repo: &Repository) -> Option<git2::Commit<'_>> {
    repo.head().ok().and_then(|head| head.peel_to_commit().ok())
}

/// Make the commit [`Changes::commit`] describes, with the hooks `git
/// commit -m` runs: `pre-commit`, which may stage more; then
/// `prepare-commit-msg` and `commit-msg` on the message, in
/// `COMMIT_EDITMSG`, which may change it; any of which may stop the
/// commit; and `post-commit` once it is made.
fn make_commit(
    repo: &Repository,
    message: &str,
    amend: bool,
    hooks: &Hooks,
) -> Result<Committed, CommitError> {
    let CommitPlan { merging, merged } = check_commit(repo, message, amend)?;
    let mut message = message.trim().to_owned();
    let env = commit_env(repo);
    hooks.run(repo, Hook::PreCommit, &[], &env)?;
    let mut index = repo.index()?;
    index.read(true)?;
    if index.has_conflicts() {
        return Err(
            git2::Error::from_str("resolve the conflicts and stage the files first").into(),
        );
    }
    // The message as git keeps it, for the hooks and after.
    let path = repo.path().join("COMMIT_EDITMSG");
    let io = |err: std::io::Error| git2::Error::from_str(&format!("COMMIT_EDITMSG: {err}"));
    fs::write(&path, format!("{message}\n")).map_err(io)?;
    if hooks.will_run(repo, Hook::PrepareCommitMsg) || hooks.will_run(repo, Hook::CommitMsg) {
        let file = path.to_string_lossy();
        hooks.run(repo, Hook::PrepareCommitMsg, &[&file, "message"], &env)?;
        hooks.run(repo, Hook::CommitMsg, &[&file], &env)?;
        let text = fs::read_to_string(&path).map_err(io)?;
        message = git2::message_prettify(text, None)?.trim().to_owned();
        if message.is_empty() {
            return Err(git2::Error::from_str("the hooks left the commit's message empty").into());
        }
    }
    let tree = repo.find_tree(index.write_tree()?)?;
    let signature = repo.signature()?;
    let head = head_commit(repo);
    let id = if amend {
        let head = head.ok_or_else(|| git2::Error::from_str("there is no commit to amend"))?;
        head.amend(
            Some("HEAD"),
            None,
            Some(&signature),
            None,
            Some(&message),
            Some(&tree),
        )?
    } else {
        let mut parents: Vec<git2::Commit<'_>> = head.into_iter().collect();
        for id in &merged {
            parents.push(repo.find_commit(*id)?);
        }
        let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            &message,
            &tree,
            &parent_refs,
        )?
    };
    if merging {
        repo.cleanup_state()?;
    }
    hooks.run(repo, Hook::PostCommit, &[], &env)?;
    Ok(Committed {
        id,
        message,
        amended: amend,
    })
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

/// Put `entry`, one side of the conflict at `path`, in the index as the
/// path's only entry, clearing the conflict.
fn stage_side(index: &mut Index, path: &str, mut entry: IndexEntry) -> Result<(), git2::Error> {
    index.remove_path(Path::new(path))?;
    entry.flags &= !STAGE_MASK;
    index.add(&entry)
}

fn count_conflicts(unstaged: &[FileChange]) -> usize {
    unstaged
        .iter()
        .filter(|change| change.kind == ChangeKind::Conflicted)
        .count()
}

/// Delete the directories above `file` that are left empty, up to but
/// not including `workdir`: those an untracked file discarded was the
/// last thing in. One that isn't empty stops it.
pub(super) fn remove_empty_parents(workdir: &Path, file: &Path) {
    let mut dir = file.parent();
    while let Some(current) = dir {
        if current == workdir || !current.starts_with(workdir) || fs::remove_dir(current).is_err() {
            break;
        }
        dir = current.parent();
    }
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
    let head_id = head_commit.as_ref().map(git2::Commit::id);
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
    let unstaged = unstaged_changes(&diff, diff.deltas().len() <= STATS_LIMIT)?;
    let conflicts = count_conflicts(&unstaged);

    let merging = repo.state() == RepositoryState::Merge;
    let merge_message = if merging {
        // Without the comments libgit2 adds listing the conflicts, which
        // git's command line would strip when committing.
        fs::read_to_string(repo.path().join("MERGE_MSG"))
            .ok()
            .and_then(|text| git2::message_prettify(text, Some(b'#')).ok())
            .map(|text| text.trim_end().to_owned())
            .filter(|text| !text.is_empty())
    } else {
        None
    };
    let rebase = rebase_status(&repo);
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
        rebase,
        head_id,
        head_message,
        conflicts,
    })
}

/// The unstaged changes of the working directory's diff against the
/// index. A submodule in conflict has no entry of its own in the index
/// for its directory to be compared with, so the diff has the directory
/// as untracked too: that is the conflict, already listed, and not
/// something new to stage (or, discarded, to delete), so it is left out.
fn unstaged_changes(diff: &git2::Diff<'_>, stats: bool) -> Result<Vec<FileChange>, git2::Error> {
    let mut changes = list_changes(diff, |_| true, stats)?;
    let conflicted: HashSet<String> = changes
        .iter()
        .filter(|change| change.kind == ChangeKind::Conflicted)
        .map(|change| change.path.clone())
        .collect();
    if !conflicted.is_empty() {
        changes.retain(|change| {
            change.kind != ChangeKind::Untracked
                || !conflicted.contains(change.path.trim_end_matches('/'))
        });
    }
    Ok(changes)
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
    use crate::git::hooks::{self, Hook, HookFailure};
    use crate::git::{DiffLine, DiffRow, Integration, LineKind, Outcome, RebasePlan};

    fn open(repo: &TestRepo) -> Changes {
        let mut changes = Changes::open(repo.path()).unwrap();
        assert!(changes.wait(Duration::from_secs(10)), "the scan finished");
        changes
    }

    /// Commit as the changes page does, without hooks to run.
    fn make(changes: &mut Changes, message: &str) -> Result<Oid, CommitError> {
        let committed = changes.commit(message, &hooks::tests::hooks())?;
        Ok(committed.expect("made at once, with no hooks").id)
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
        assert!(make(&mut changes, "   ").is_err());
        let id = make(&mut changes, "Change things\n").unwrap();
        confirmed(&mut changes);
        assert!(changes.is_clean());
        let commit = repo.repo.find_commit(id).unwrap();
        assert_eq!(commit.message().unwrap(), "Change things");
        assert_eq!(commit.parent_id(0).unwrap(), base);
        assert_eq!(repo.repo.head().unwrap().target(), Some(id));
        assert!(
            make(&mut changes, "Nothing").is_err(),
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
        assert!(make(&mut changes, "Merge").is_err(), "conflicts remain");

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
        let id = make(&mut changes, "Merge branch 'side'").unwrap();
        confirmed(&mut changes);
        let commit = repo.repo.find_commit(id).unwrap();
        let parents: Vec<Oid> = commit.parent_ids().collect();
        assert_eq!(parents, vec![ours, theirs]);
        assert_eq!(repo.repo.state(), RepositoryState::Clean);
        assert!(!changes.is_merging());
        assert!(changes.is_clean());
    }

    #[test]
    fn conflicts_are_resolved_by_taking_one_side() {
        let mut repo = TestRepo::new();
        configure_user(&repo);
        // The test repository's checkout only moves HEAD: bring the
        // working directory and the index along.
        let follow_head = |repo: &TestRepo| {
            let mut checkout = git2::build::CheckoutBuilder::new();
            repo.repo.checkout_head(Some(checkout.force())).unwrap();
        };
        let base = repo.commit(
            &[
                ("a/f.txt", "one\ntwo\nthree\n"),
                ("a/g.txt", "g\n"),
                ("d/gone.txt", "d\n"),
            ],
            "Base",
            &[],
        );
        let main = repo.repo.head().unwrap().shorthand().unwrap().to_owned();
        let ours = repo.commit(
            &[
                ("a/f.txt", "one\nours\nthree\n"),
                ("a/g.txt", "g ours\n"),
                ("d/gone.txt", "d ours\n"),
            ],
            "Ours",
            &[base],
        );
        repo.branch("side", base);
        repo.checkout("side");
        follow_head(&repo);
        let edited = repo.commit(
            &[
                ("a/f.txt", "one\ntheirs\nthree\n"),
                ("a/g.txt", "g theirs\n"),
            ],
            "Theirs",
            &[base],
        );
        let theirs = repo.remove("d/gone.txt", "Remove", edited);
        repo.checkout(&main);
        follow_head(&repo);
        let annotated = repo.repo.find_annotated_commit(theirs).unwrap();
        repo.repo.merge(&[&annotated], None, None).unwrap();

        let mut changes = open(&repo);
        assert_eq!(changes.conflict_count(), 3);
        // Ours: the file is as HEAD has it, so it is in neither list. A
        // path not in conflict is left alone.
        changes
            .resolve(["a/f.txt", "nothing.txt"], ConflictSide::Ours)
            .unwrap();
        confirmed(&mut changes);
        assert_eq!(read(&repo, "a/f.txt"), "one\nours\nthree\n");
        assert_eq!(changes.conflict_count(), 2);
        assert_eq!(
            listed(changes.unstaged()),
            [('U', "a/g.txt", 0, 0), ('U', "d/gone.txt", 0, 0)]
        );
        assert!(changes.staged().is_empty());
        // Theirs: an edit is staged as theirs, and a deletion deletes,
        // taking the directory it leaves empty.
        changes
            .resolve(["a/g.txt", "d/gone.txt"], ConflictSide::Theirs)
            .unwrap();
        confirmed(&mut changes);
        assert_eq!(read(&repo, "a/g.txt"), "g theirs\n");
        assert!(!repo.path().join("d").exists());
        assert_eq!(changes.conflict_count(), 0);
        assert!(changes.unstaged().is_empty());
        assert_eq!(
            listed(changes.staged()),
            [('M', "a/g.txt", 1, 1), ('D', "d/gone.txt", 0, 1)]
        );
        let id = make(&mut changes, "Merge branch 'side'").unwrap();
        confirmed(&mut changes);
        let commit = repo.repo.find_commit(id).unwrap();
        assert_eq!(commit.parent_ids().collect::<Vec<_>>(), [ours, theirs]);
        assert!(changes.is_clean());
    }

    #[test]
    fn a_rebase_conflict_is_resolved_and_continued_or_aborted() {
        use crate::git::operation::tests::diverged;
        use crate::git::{Integration, Target};
        let mut repo = TestRepo::new();
        configure_user(&repo);
        let (_, main, side) = diverged(&mut repo, true);
        let rebase = Integration::Rebase(Target::Branch {
            name: "side".to_owned(),
            remote: false,
        });
        let stopped = rebase
            .run_with(&repo.repo, &hooks::tests::hooks(), &mut |_, _| {})
            .unwrap();
        assert!(matches!(stopped, Outcome::Conflicts { files: 1, .. }));

        let mut changes = open(&repo);
        let status = changes.rebase().unwrap();
        assert_eq!((status.step, status.total), (1, 1));
        assert_eq!(status.onto, "side");
        assert_eq!(status.message.as_deref(), Some("Main"));
        assert!(!changes.is_merging());
        assert_eq!(changes.conflict_count(), 1);
        assert!(changes.set_amend(true).is_err(), "no amending mid-rebase");
        assert!(make(&mut changes, "Main").is_err(), "a rebase continues");
        assert!(matches!(
            changes.continue_rebase(""),
            Err(OperationError::Unresolved { files: 1, .. })
        ));
        settle(&mut changes);

        // Abort: back on main as it was.
        assert_eq!(changes.abort().unwrap(), InProgress::Rebase);
        settle(&mut changes);
        assert!(changes.rebase().is_none());
        assert!(changes.is_clean());
        assert_eq!(repo.repo.head().unwrap().target(), Some(main));

        // Again, resolved and continued this time, with a new message.
        rebase
            .run_with(&repo.repo, &hooks::tests::hooks(), &mut |_, _| {})
            .unwrap();
        changes.refresh();
        settle(&mut changes);
        fs::write(repo.path().join("f.txt"), "main and side\n").unwrap();
        changes.stage(["f.txt"]).unwrap();
        settle(&mut changes);
        let continued = changes.continue_rebase("Main, on side").unwrap();
        assert_eq!(continued.committed, None);
        assert!(matches!(
            continued.outcome,
            Some(Outcome::Rebased { commits: 1, .. })
        ));
        settle(&mut changes);
        assert!(changes.rebase().is_none());
        assert!(changes.is_clean());
        assert_eq!(changes.head_message(), Some("Main, on side"));
        let head = repo.repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.parent_ids().collect::<Vec<_>>(), [side]);
        assert_eq!(repo.repo.head().unwrap().shorthand().unwrap(), "master");
    }

    #[test]
    fn staging_keeps_what_something_else_staged_since() {
        let mut repo = TestRepo::new();
        repo.commit(&[("a.txt", "a\n"), ("b.txt", "b\n")], "Base", &[]);
        let mut changes = open(&repo);
        // Read the index once, as an action does.
        changes.unstage(["a.txt"]).unwrap();
        // Something else (a `git add` in a shell) stages b.txt.
        fs::write(repo.path().join("a.txt"), "a2\n").unwrap();
        fs::write(repo.path().join("b.txt"), "b2\n").unwrap();
        let other = Repository::open(repo.path()).unwrap();
        let mut index = other.index().unwrap();
        index.add_path(Path::new("b.txt")).unwrap();
        index.write().unwrap();
        // Staging a.txt here mustn't write the index back as it was.
        changes.stage(["a.txt"]).unwrap();
        settle(&mut changes);
        assert_eq!(
            listed(changes.staged()),
            [('M', "a.txt", 1, 1), ('M', "b.txt", 1, 1)]
        );
    }

    #[test]
    fn an_interactive_rebases_stop_is_committed_and_continued_or_aborted() {
        let mut repo = TestRepo::new();
        configure_user(&repo);
        let base = repo.commit(&[("a.txt", "a\n")], "Base", &[]);
        let two = repo.commit(&[("a.txt", "a2\n"), ("b.txt", "b\n")], "Two", &[base]);
        let three = repo.commit(&[("c.txt", "c\n")], "Three", &[two]);
        let edit = Integration::Interactive(RebasePlan::edit(&repo.repo, two).unwrap());
        edit.run_with(&repo.repo, &hooks::tests::hooks(), &mut |_, _| {})
            .unwrap();
        let mut changes = open(&repo);
        // Two's changes, staged on its parent.
        assert_eq!(changes.head_id(), Some(base));
        assert_eq!(
            listed(changes.staged()),
            [('M', "a.txt", 1, 1), ('A', "b.txt", 1, 0)]
        );
        assert!(changes.unstaged().is_empty());
        let status = changes.rebase().unwrap();
        assert!(status.interactive);
        assert_eq!((status.step, status.total), (1, 2));
        assert_eq!(status.message.as_deref(), Some("Two"));
        assert!(changes.set_amend(true).is_err(), "no amending mid-rebase");
        assert!(make(&mut changes, "Two").is_err(), "a rebase continues");

        // Split off b.txt: committing the rest waits for it.
        changes.unstage(["b.txt"]).unwrap();
        settle(&mut changes);
        let continued = changes.continue_rebase("Two: a").unwrap();
        assert_eq!(continued.outcome, None);
        settle(&mut changes);
        assert_eq!(changes.head_id(), continued.committed);
        assert_eq!(listed(changes.unstaged()), [('?', "b.txt", 1, 0)]);
        assert_eq!(changes.rebase().unwrap().message.as_deref(), Some("Two"));

        // Abort: back where the branch was.
        assert_eq!(changes.abort().unwrap(), InProgress::Rebase);
        settle(&mut changes);
        assert!(changes.rebase().is_none());
        assert!(changes.is_clean());
        assert_eq!(changes.head_id(), Some(three));

        // Again, with b.txt committed too this time.
        edit.run_with(&repo.repo, &hooks::tests::hooks(), &mut |_, _| {})
            .unwrap();
        changes.refresh();
        settle(&mut changes);
        changes.unstage(["b.txt"]).unwrap();
        changes.continue_rebase("Two: a").unwrap();
        settle(&mut changes);
        changes.stage(["b.txt"]).unwrap();
        let continued = changes.continue_rebase("Two: b").unwrap();
        assert!(matches!(
            continued.outcome,
            Some(Outcome::Rebased { commits: 2, .. })
        ));
        settle(&mut changes);
        assert!(changes.rebase().is_none());
        assert!(changes.is_clean());
        assert_eq!(changes.head_message(), Some("Three"));
        assert_eq!(repo.repo.head().unwrap().shorthand().unwrap(), "master");
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
        // The uncommitted change inside is listed, so that it isn't
        // missed behind the graph.
        let inner_changes: Vec<(char, &str, bool)> = range
            .uncommitted
            .iter()
            .map(|c| (c.kind.letter(), c.path.as_str(), c.staged))
            .collect();
        assert_eq!(inner_changes, [('M', "inner.txt", false)]);

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
        let inner_commit = make(&mut inner, "Inner change").unwrap();
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
        assert!(range.uncommitted.is_empty());
        // A new file inside, on top of the move, is listed under the
        // same commits.
        fs::write(sub.workdir().unwrap().join("extra.txt"), "extra\n").unwrap();
        let diff = changes.unstaged_diff(&changes.unstaged()[0]).unwrap();
        let range = diff.submodule.as_ref().unwrap();
        assert_eq!(range.commits.len(), 2);
        let inner_changes: Vec<(char, &str)> = range
            .uncommitted
            .iter()
            .map(|c| (c.kind.letter(), c.path.as_str()))
            .collect();
        assert_eq!(inner_changes, [('?', "extra.txt")]);
        fs::remove_file(sub.workdir().unwrap().join("extra.txt")).unwrap();
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
        let id = make(&mut changes, "Update submodule").unwrap();
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
        let id = make(&mut changes, "Add two and bee").unwrap();
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

    fn read(repo: &TestRepo, path: &str) -> String {
        fs::read_to_string(repo.path().join(path)).unwrap()
    }

    /// The lines of a diff with these texts as `lines` writes them
    /// (`-3`, `+three`), in the order asked for.
    fn pick(file: &FileDiff, texts: &[&str]) -> Vec<DiffLine> {
        texts
            .iter()
            .map(|text| {
                file.rows()
                    .into_iter()
                    .find_map(|row| match row {
                        DiffRow::Line(line) => {
                            let marker = match line.kind {
                                LineKind::Context => ' ',
                                LineKind::Added => '+',
                                LineKind::Removed => '-',
                            };
                            (format!("{marker}{}", file.text(&line)) == *text).then_some(line)
                        }
                        DiffRow::Gap { .. } => None,
                    })
                    .unwrap_or_else(|| panic!("no {text:?} in {:?}", lines(file)))
            })
            .collect()
    }

    /// What the index holds for a file.
    fn index_text(repo: &TestRepo, path: &str) -> String {
        let mut index = repo.repo.index().unwrap();
        index.read(true).unwrap();
        let entry = index.get_path(Path::new(path), 0).unwrap();
        let blob = repo.repo.find_blob(entry.id).unwrap();
        String::from_utf8(blob.content().to_vec()).unwrap()
    }

    #[test]
    fn lines_are_staged_unstaged_and_reverted() {
        let mut repo = TestRepo::new();
        let base: String = (1..=10).map(|n| format!("{n}\n")).collect();
        repo.commit(&[("a.txt", &base)], "Base", &[]);
        let edited = base
            .replace("3\n", "three\n")
            .replace("5\n", "5\nnew\n")
            .replace("8\n", "eight\n");
        fs::write(repo.path().join("a.txt"), &edited).unwrap();
        let mut changes = open(&repo);
        let unstaged = |changes: &Changes| {
            let change = changes.unstaged()[0].clone();
            changes.unstaged_diff(&change).unwrap()
        };
        let staged = |changes: &Changes| {
            let change = changes.staged()[0].clone();
            changes.staged_diff(&change).unwrap()
        };

        // Stage the change of line 3 alone: the index has it, and the
        // rest are still unstaged.
        let diff = unstaged(&changes);
        let stage = diff.stage_lines(&pick(&diff, &["-3", "+three"])).unwrap();
        assert_eq!((stage.lines, stage.target), (2, LinesTarget::Index));
        changes.apply_lines(&stage).unwrap();
        confirmed(&mut changes);
        assert_eq!(index_text(&repo, "a.txt"), base.replace("3\n", "three\n"));
        assert_eq!(listed(changes.staged()), [('M', "a.txt", 1, 1)]);
        assert_eq!(listed(changes.unstaged()), [('M', "a.txt", 2, 1)]);
        // Context lines, and lines that aren't the diff's, take nothing.
        let diff = unstaged(&changes);
        assert_eq!(diff.stage_lines(&pick(&diff, &[" 4"])), None);
        assert_eq!(diff.stage_lines(&[]), None);

        // Stage the added line alone, then unstage the change of line 3
        // again: the index has the base with just the new line.
        let stage = diff.stage_lines(&pick(&diff, &["+new"])).unwrap();
        changes.apply_lines(&stage).unwrap();
        confirmed(&mut changes);
        let diff = staged(&changes);
        let unstage = diff.unstage_lines(&pick(&diff, &["-3", "+three"])).unwrap();
        changes.apply_lines(&unstage).unwrap();
        confirmed(&mut changes);
        assert_eq!(index_text(&repo, "a.txt"), base.replace("5\n", "5\nnew\n"));

        // Revert the change of line 8 in the working directory: the rest
        // of the edits stay.
        let diff = unstaged(&changes);
        let revert = diff.revert_lines(&pick(&diff, &["-8", "+eight"])).unwrap();
        assert_eq!(revert.target, LinesTarget::WorkingTree);
        changes.apply_lines(&revert).unwrap();
        confirmed(&mut changes);
        assert_eq!(
            read(&repo, "a.txt"),
            base.replace("3\n", "three\n").replace("5\n", "5\nnew\n")
        );

        // A diff the file has moved on from writes nothing.
        let diff = unstaged(&changes);
        let revert = diff.revert_lines(&pick(&diff, &["+three"])).unwrap();
        fs::write(repo.path().join("a.txt"), "edited since\n").unwrap();
        let err = changes.apply_lines(&revert).unwrap_err();
        assert!(err.message().contains("changed since"), "{}", err.message());
        assert_eq!(read(&repo, "a.txt"), "edited since\n");
    }

    /// Apply a patch to the working directory as `git apply` does,
    /// with libgit2's reader of git's patches.
    fn apply_patch(repo: &TestRepo, patch: &str) {
        let diff = git2::Diff::from_buffer(patch.as_bytes())
            .unwrap_or_else(|err| panic!("{err}: {patch}"));
        repo.repo
            .apply(&diff, git2::ApplyLocation::WorkDir, None)
            .unwrap_or_else(|err| panic!("{err}: {patch}"));
    }

    #[test]
    fn selected_lines_make_a_patch_git_applies() {
        let mut repo = TestRepo::new();
        let base: String = (1..=10).map(|n| format!("{n}\n")).collect();
        repo.commit(
            &[("a.txt", &base), ("gone.txt", "x\ny\n"), ("end.txt", "e")],
            "Base",
            &[],
        );
        let edited = base.replace("3\n", "three\n").replace("8\n", "eight\n");
        fs::write(repo.path().join("a.txt"), &edited).unwrap();
        fs::write(repo.path().join("new.txt"), "n1\nn2\n").unwrap();
        fs::remove_file(repo.path().join("gone.txt")).unwrap();
        fs::write(repo.path().join("end.txt"), "e\nf").unwrap();
        let changes = open(&repo);
        let diff_of = |path: &str| {
            let change = changes
                .unstaged()
                .iter()
                .find(|change| change.path == path)
                .unwrap()
                .clone();
            changes.unstaged_diff(&change).unwrap()
        };

        // The change of line 3 alone, against the file as committed.
        let diff = diff_of("a.txt");
        let patch = diff
            .patch_of_lines(&pick(&diff, &["-3", "+three"]))
            .unwrap();
        assert!(
            patch.starts_with("diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ "),
            "{patch}"
        );
        assert!(!patch.contains("eight"), "{patch}");
        fs::write(repo.path().join("a.txt"), &base).unwrap();
        apply_patch(&repo, &patch);
        assert_eq!(read(&repo, "a.txt"), base.replace("3\n", "three\n"));
        assert_eq!(diff.patch_of_lines(&pick(&diff, &[" 4"])), None);

        // A new file is created, with the lines chosen.
        let diff = diff_of("new.txt");
        let patch = diff.patch_of_lines(&pick(&diff, &["+n2"])).unwrap();
        assert!(
            patch.contains("new file mode 100644\n--- /dev/null\n"),
            "{patch}"
        );
        fs::remove_file(repo.path().join("new.txt")).unwrap();
        apply_patch(&repo, &patch);
        assert_eq!(read(&repo, "new.txt"), "n2\n");

        // A file with all its lines removed is deleted; with some, kept.
        let diff = diff_of("gone.txt");
        let all = diff.patch_of_lines(&pick(&diff, &["-x", "-y"])).unwrap();
        assert!(all.contains("deleted file mode 100644\n"), "{all}");
        assert!(all.contains("+++ /dev/null\n"), "{all}");
        let some = diff.patch_of_lines(&pick(&diff, &["-x"])).unwrap();
        fs::write(repo.path().join("gone.txt"), "x\ny\n").unwrap();
        apply_patch(&repo, &some);
        assert_eq!(read(&repo, "gone.txt"), "y\n");
        fs::write(repo.path().join("gone.txt"), "x\ny\n").unwrap();
        apply_patch(&repo, &all);
        assert!(!repo.path().join("gone.txt").exists());

        // A last line without a line break says so, and keeps it so.
        let diff = diff_of("end.txt");
        let patch = diff
            .patch_of_lines(&pick(&diff, &["-e", "+e", "+f"]))
            .unwrap();
        assert!(patch.contains("\\ No newline at end of file"), "{patch}");
        fs::write(repo.path().join("end.txt"), "e").unwrap();
        apply_patch(&repo, &patch);
        assert_eq!(read(&repo, "end.txt"), "e\nf");
    }

    #[test]
    fn lines_of_an_untracked_file_are_staged_into_a_new_entry() {
        let mut repo = TestRepo::new();
        repo.commit(&[("a.txt", "a\n")], "Base", &[]);
        fs::write(repo.path().join("b.txt"), "x\ny\n").unwrap();
        let mut changes = open(&repo);
        let change = changes.unstaged()[0].clone();
        assert_eq!(change.kind, ChangeKind::Untracked);
        let diff = changes.unstaged_diff(&change).unwrap();
        let stage = diff.stage_lines(&pick(&diff, &["+x"])).unwrap();
        changes.apply_lines(&stage).unwrap();
        confirmed(&mut changes);
        assert_eq!(index_text(&repo, "b.txt"), "x\n");
        assert_eq!(listed(changes.staged()), [('A', "b.txt", 1, 0)]);
        assert_eq!(listed(changes.unstaged()), [('M', "b.txt", 1, 0)]);
    }

    #[test]
    fn discarding_puts_files_back_to_the_index_and_deletes_untracked_ones() {
        let mut repo = TestRepo::new();
        repo.commit(
            &[
                ("a.rs", "fn a() {}\n"),
                ("b.txt", "b\n"),
                ("dir/c.txt", "c\n"),
                ("dir/d.txt", "d\n"),
                ("keep.txt", "k\n"),
            ],
            "Base",
            &[],
        );
        // a.rs: a staged change, and an unstaged one on top of it.
        fs::write(repo.path().join("a.rs"), "fn a() {}\nfn staged() {}\n").unwrap();
        let mut changes = open(&repo);
        changes.stage(["a.rs"]).unwrap();
        confirmed(&mut changes);
        fs::write(
            repo.path().join("a.rs"),
            "fn a() {}\nfn staged() {}\nfn unstaged() {}\n",
        )
        .unwrap();
        // b.txt: modified, nothing staged. dir/c.txt: deleted.
        fs::write(repo.path().join("b.txt"), "bee\n").unwrap();
        fs::remove_file(repo.path().join("dir/c.txt")).unwrap();
        // Untracked: one beside tracked files, one alone in directories
        // of its own.
        fs::write(repo.path().join("dir/e.txt"), "e\n").unwrap();
        fs::create_dir_all(repo.path().join("new/deep")).unwrap();
        fs::write(repo.path().join("new/deep/x.txt"), "x\n").unwrap();
        // keep.txt: modified, and not discarded.
        fs::write(repo.path().join("keep.txt"), "kept\n").unwrap();
        changes.refresh();
        settle(&mut changes);
        assert_eq!(
            listed(changes.unstaged()),
            [
                ('M', "a.rs", 1, 0),
                ('M', "b.txt", 1, 1),
                ('D', "dir/c.txt", 0, 1),
                ('?', "dir/e.txt", 1, 0),
                ('M', "keep.txt", 1, 1),
                ('?', "new/deep/x.txt", 1, 0),
            ]
        );
        assert_eq!(listed(changes.staged()), [('M', "a.rs", 1, 0)]);

        changes
            .discard(["a.rs", "b.txt", "dir/c.txt", "dir/e.txt", "new/deep/x.txt"])
            .unwrap();
        confirmed(&mut changes);
        // a.rs goes back to what is staged, which stays staged; b.txt
        // to what is committed; dir/c.txt comes back.
        assert_eq!(read(&repo, "a.rs"), "fn a() {}\nfn staged() {}\n");
        assert_eq!(listed(changes.staged()), [('M', "a.rs", 1, 0)]);
        assert_eq!(read(&repo, "b.txt"), "b\n");
        assert_eq!(read(&repo, "dir/c.txt"), "c\n");
        // The untracked files are gone, and the directories only the
        // one was in with them; dir still has its tracked files.
        assert!(!repo.path().join("dir/e.txt").exists());
        assert!(!repo.path().join("new").exists());
        assert_eq!(read(&repo, "dir/d.txt"), "d\n");
        // Only what was named: keep.txt is still changed.
        assert_eq!(listed(changes.unstaged()), [('M', "keep.txt", 1, 1)]);
        assert_eq!(read(&repo, "keep.txt"), "kept\n");

        // A path with nothing to discard is left alone, as is an empty
        // list (which mustn't mean every file).
        changes.discard(["b.txt"]).unwrap();
        confirmed(&mut changes);
        changes.discard([]).unwrap();
        assert_eq!(read(&repo, "keep.txt"), "kept\n");
        assert_eq!(listed(changes.unstaged()), [('M', "keep.txt", 1, 1)]);
    }

    #[test]
    fn discarding_refuses_a_conflict_changing_nothing() {
        let mut repo = TestRepo::new();
        let base = repo.commit(
            &[("f.txt", "one\ntwo\nthree\n"), ("g.txt", "g\n")],
            "Base",
            &[],
        );
        let main = repo.repo.head().unwrap().shorthand().unwrap().to_owned();
        repo.commit(&[("f.txt", "one\nours\nthree\n")], "Ours", &[base]);
        repo.branch("side", base);
        repo.checkout("side");
        let theirs = repo.commit(&[("f.txt", "one\ntheirs\nthree\n")], "Theirs", &[base]);
        repo.checkout(&main);
        repo.repo
            .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        let annotated = repo.repo.find_annotated_commit(theirs).unwrap();
        repo.repo.merge(&[&annotated], None, None).unwrap();
        fs::write(repo.path().join("g.txt"), "gee\n").unwrap();

        let mut changes = open(&repo);
        assert_eq!(
            listed(changes.unstaged()),
            [('U', "f.txt", 0, 0), ('M', "g.txt", 1, 1)]
        );
        let conflicted = read(&repo, "f.txt");
        let err = changes.discard(["g.txt", "f.txt"]).unwrap_err();
        assert!(err.message().contains("f.txt"), "{}", err.message());
        // Nothing was discarded, not even the file that could have been.
        assert_eq!(read(&repo, "g.txt"), "gee\n");
        assert_eq!(read(&repo, "f.txt"), conflicted);
        assert_eq!(changes.conflict_count(), 1);
        changes.discard(["g.txt"]).unwrap();
        confirmed(&mut changes);
        assert_eq!(read(&repo, "g.txt"), "g\n");
        assert_eq!(listed(changes.unstaged()), [('U', "f.txt", 0, 0)]);
    }

    #[test]
    fn discarding_refuses_a_submodule() {
        let mut repo = TestRepo::new();
        repo.commit(&[("a.txt", "one\n")], "Base", &[]);
        let sub = repo.add_submodule("libs/sub");
        fs::write(sub.workdir().unwrap().join("inner.txt"), "changed\n").unwrap();
        let mut changes = open(&repo);
        assert_eq!(listed(changes.unstaged()), [('M', "libs/sub", 0, 0)]);
        let err = changes.discard(["libs/sub"]).unwrap_err();
        assert!(err.message().contains("submodule"), "{}", err.message());
        assert_eq!(
            fs::read_to_string(sub.workdir().unwrap().join("inner.txt")).unwrap(),
            "changed\n"
        );
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
        assert!(make(&mut changes, "Nothing staged").is_err());
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

    /// Commit with hooks to run, waiting for the commit's thread.
    fn make_with(
        changes: &mut Changes,
        message: &str,
        hooks: &Hooks,
    ) -> Result<Committed, CommitError> {
        assert!(
            changes.commit(message, hooks)?.is_none(),
            "made on its thread"
        );
        assert!(changes.is_committing());
        assert!(changes.wait(Duration::from_secs(10)), "the commit finished");
        changes.take_commit().expect("the commit's outcome")
    }

    /// A repository with a commit, and a change staged on top of it.
    fn staged_change() -> (TestRepo, Changes, Oid) {
        let mut repo = TestRepo::new();
        let base = repo.commit(&[("a.txt", "one\n")], "Base", &[]);
        fs::write(repo.path().join("a.txt"), "two\n").unwrap();
        let mut changes = open(&repo);
        changes.stage(["a.txt"]).unwrap();
        settle(&mut changes);
        (repo, changes, base)
    }

    #[test]
    fn a_failing_pre_commit_hook_stops_the_commit_until_skipped() {
        let (repo, mut changes, base) = staged_change();
        hooks::tests::install(&repo.repo, Hook::PreCommit, "echo \"lint: bad\"; exit 1");
        let Err(CommitError::Hook(HookError::Failed(failure))) =
            make_with(&mut changes, "Change", &hooks::tests::hooks())
        else {
            panic!("the hook should stop the commit");
        };
        assert_eq!(failure.hook, Hook::PreCommit);
        assert_eq!(failure.output_text(), "lint: bad");
        settle(&mut changes);
        assert_eq!(repo.repo.head().unwrap().target(), Some(base));
        assert_eq!(listed(changes.staged()), [('M', "a.txt", 1, 1)]);

        // --no-verify: done anyway.
        let hooks = hooks::tests::hooks().without_verify();
        let committed = changes.commit("Change", &hooks).unwrap().unwrap();
        assert_eq!(committed.message, "Change");
        assert_eq!(repo.repo.head().unwrap().target(), Some(committed.id));
        assert!(changes.staged().is_empty());
    }

    #[test]
    fn message_hooks_change_the_message_and_post_commit_failing_stops_nothing() {
        let (repo, mut changes, base) = staged_change();
        hooks::tests::install(
            &repo.repo,
            Hook::PrepareCommitMsg,
            "[ \"$2\" = message ] || exit 1; printf '[%s] ' \"$(basename \"$1\")\" | cat - \"$1\" > \"$1.new\" && mv \"$1.new\" \"$1\"",
        );
        hooks::tests::install(
            &repo.repo,
            Hook::CommitMsg,
            "[ \"$GIT_EDITOR\" = : ] || exit 1; printf '\\nSigned-off-by: Test\\n\\n\\n' >> \"$1\"",
        );
        hooks::tests::install(&repo.repo, Hook::PostCommit, "echo done; exit 2");
        let hooks = hooks::tests::hooks();
        let committed = make_with(&mut changes, "Change\n", &hooks).unwrap();
        let message = "[COMMIT_EDITMSG] Change\n\nSigned-off-by: Test";
        assert_eq!(committed.message, message);
        let commit = repo.repo.find_commit(committed.id).unwrap();
        assert_eq!(commit.message().unwrap(), message);
        assert_eq!(commit.parent_id(0).unwrap(), base);
        assert_eq!(changes.head_message(), Some(message));
        let failures = hooks.take_failures();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].hook, Hook::PostCommit);
        assert_eq!(failures[0].output_text(), "done");

        // A commit-msg hook that rejects a message stops the amend.
        hooks::tests::install(
            &repo.repo,
            Hook::CommitMsg,
            "grep -q WIP \"$1\" && exit 1; exit 0",
        );
        changes.set_amend(true).unwrap();
        settle(&mut changes);
        let result = make_with(&mut changes, "WIP", &hooks::tests::hooks());
        assert!(matches!(
            result,
            Err(CommitError::Hook(HookError::Failed(HookFailure {
                hook: Hook::CommitMsg,
                ..
            })))
        ));
        assert_eq!(repo.repo.head().unwrap().target(), Some(committed.id));
    }

    #[test]
    fn what_the_pre_commit_hook_stages_is_committed() {
        let (repo, mut changes, _) = staged_change();
        // As a formatter run by the hook would.
        hooks::tests::install(
            &repo.repo,
            Hook::PreCommit,
            "echo three > a.txt && git add a.txt",
        );
        let committed = make_with(&mut changes, "Change", &hooks::tests::hooks()).unwrap();
        let commit = repo.repo.find_commit(committed.id).unwrap();
        let entry = commit.tree().unwrap().get_name("a.txt").unwrap().id();
        let blob = repo.repo.find_blob(entry).unwrap();
        assert_eq!(blob.content(), b"three\n");
    }

    #[test]
    fn nothing_else_is_done_while_the_hooks_of_a_commit_run() {
        let (repo, mut changes, _) = staged_change();
        let go = repo.path().join("go");
        hooks::tests::install(
            &repo.repo,
            Hook::PreCommit,
            "while [ ! -f go ]; do sleep 0.02; done; rm go",
        );
        let hooks = hooks::tests::hooks();
        assert!(changes.commit("Change", &hooks).unwrap().is_none());
        assert!(changes.is_committing());
        assert!(changes.stage(["a.txt"]).is_err());
        assert!(changes.set_amend(true).is_err());
        assert!(changes.commit("Again", &hooks).is_err());
        assert!(changes.abort().is_err());
        fs::write(&go, "").unwrap();
        assert!(changes.wait(Duration::from_secs(10)));
        assert!(!changes.is_committing());
        let committed = changes.take_commit().unwrap().unwrap();
        assert_eq!(repo.repo.head().unwrap().target(), Some(committed.id));
        assert!(changes.take_commit().is_none());

        // A refusal that needs no hook comes at once, before any runs.
        assert!(changes.commit("Nothing staged", &hooks).is_err());
        assert!(!changes.is_committing());
    }

    #[test]
    fn cancelling_a_commit_s_hook_makes_no_commit() {
        let (repo, mut changes, base) = staged_change();
        hooks::tests::install(&repo.repo, Hook::PreCommit, "sleep 30");
        let hooks = hooks::tests::hooks();
        assert!(changes.commit("Change", &hooks).unwrap().is_none());
        let start = Instant::now();
        while hooks.running().is_none() {
            assert!(start.elapsed() < Duration::from_secs(10));
            thread::sleep(Duration::from_millis(5));
        }
        hooks.cancel();
        assert!(changes.wait(Duration::from_secs(10)));
        assert!(matches!(
            changes.take_commit(),
            Some(Err(CommitError::Hook(HookError::Cancelled(
                Hook::PreCommit
            ))))
        ));
        assert_eq!(repo.repo.head().unwrap().target(), Some(base));
    }
}
