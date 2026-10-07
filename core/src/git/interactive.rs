//! Interactive rebasing, as `git rebase -i` does it: the commits of
//! HEAD's branch from some point on are replayed onto a commit as a
//! plan, a [`RebasePlan`], says, step by step. For now a step picks its
//! commit (replays it as it is), edits it (stops with its changes
//! staged, for the user to change, split, or reword before going on),
//! or rewords it (stops the same way, for a new message), and the one
//! plan made here is [`RebasePlan::edit`]'s: edit one commit of HEAD's
//! branch, and pick each one after it. A frontend that lets the user
//! write the plan builds on the same steps.
//!
//! libgit2's rebase does no interactive steps, so the replaying is the
//! editor's own, by libgit2's three-way merge as a cherry-pick does it;
//! but what it keeps in the repository while under way is git's own
//! (`.git/rebase-merge` with its `interactive` marker, its
//! `git-rebase-todo` of the steps to come and `done` of those taken,
//! and at a stop `stopped-sha`, `message`, and `author-script`), so
//! `git status` in a shell says an interactive rebase is in progress,
//! and `git rebase --continue` or `--abort` there carries on from where
//! the editor stopped. One that git's command line began is carried on
//! here too, as long as its remaining steps are ones the editor knows.
//! Git stops to edit a commit differently, though: with the commit
//! made, HEAD on it, and nothing staged, for `git commit --amend`;
//! [`adopt_stop`] makes such a stop the editor's kind.
//!
//! A pick replays its commit onto HEAD, keeping its author and message:
//! as the very commit when HEAD is its parent already (a fast-forward,
//! as git does), else by merging its change in. A replay that leaves
//! nothing changed is dropped, as git drops it. One that conflicts
//! stops the rebase there, with the conflicts in the working directory
//! and the index. An edit replays its commit the same way but stops
//! without committing it: HEAD is left at what it is replayed onto, and
//! the commit's changes are staged, so that the user can change them,
//! unstage some of them to split the commit, or just reword it. A
//! reword stops just as an edit does, git's own reword being an edit
//! that only changes the message: the message box is where it is
//! changed, and the commit then made goes on as an edit's does.
//!
//! At a stop, [`proceed`] commits what is staged, with the message
//! given, as the stopped commit's author, and goes on with the rest of
//! the plan once nothing is left unstaged; what is still unstaged is
//! for another commit of the same stop (the next part of a split), or
//! to be discarded. With nothing staged it commits nothing (a rebase
//! never makes an empty commit) and goes on if nothing is unstaged
//! either. An untracked file counts as unstaged when the stopped
//! commit added it, since unstaging a file the commit added leaves it
//! untracked, and going on would leave it out; other untracked files
//! have nothing to do with the rebase, and are left alone, as git
//! leaves them. [`abort`] puts the branch back as it was before the
//! rebase.
//!
//! The submodules follow each step as they follow a plain rebase's (see
//! the [`operation`](super::operation) module).

use super::checkout::check_submodules;
use super::history::short_id;
use super::operation::{
    InProgress, OperationError, Outcome, checkout_options, conflicted_files, ensure_ready,
    follow_submodules, head_commit, settle_part_way, with_resolved, with_submodules,
};
use super::rebase::RebaseStatus;
use git2::{Commit, Oid, Repository, RepositoryState, ResetType, Signature, Status, StatusOptions};
use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The directory git keeps a rebase's state in, under the git
/// directory.
const STATE_DIR: &str = "rebase-merge";
/// What `head-name` holds for a rebase of a detached HEAD.
const DETACHED: &str = "detached HEAD";
/// Where git notes the commit it stopped on to be amended, at a stop
/// of its own to edit a commit.
const AMEND: &str = "amend";

/// What a step of an interactive rebase does with its commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebaseAction {
    /// Replay the commit as it is.
    Pick,
    /// Replay the commit's changes, staged, and stop for the user to
    /// change, split, or reword them, and commit.
    Edit,
    /// Stop as for an edit, for the user to give the commit a new
    /// message.
    Reword,
}

impl RebaseAction {
    /// The word for it in git's list of steps.
    pub fn word(self) -> &'static str {
        match self {
            RebaseAction::Pick => "pick",
            RebaseAction::Edit => "edit",
            RebaseAction::Reword => "reword",
        }
    }

    /// Whether the rebase stops at a step that does this, for the user.
    pub fn stops(self) -> bool {
        match self {
            RebaseAction::Pick => false,
            RebaseAction::Edit | RebaseAction::Reword => true,
        }
    }

    /// The action a word of git's list of steps names, in full or
    /// abbreviated, if the editor knows it.
    fn parse(word: &str) -> Option<RebaseAction> {
        match word {
            "pick" | "p" => Some(RebaseAction::Pick),
            "edit" | "e" => Some(RebaseAction::Edit),
            "reword" | "r" => Some(RebaseAction::Reword),
            _ => None,
        }
    }
}

/// One step of a plan: what to do with which commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebaseStep {
    pub action: RebaseAction,
    pub commit: Oid,
    /// The commit's summary line, as the list of steps shows it.
    pub summary: String,
}

impl RebaseStep {
    /// The step as a line of git's list of steps.
    fn line(&self) -> String {
        format!("{} {} {}", self.action.word(), self.commit, self.summary)
    }
}

/// What an interactive rebase is to do: replay its steps' commits, in
/// order, onto `onto`, and move the branch to the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebasePlan {
    /// The branch being rebased, as its full reference name
    /// (`refs/heads/main`), or `None` for a detached HEAD.
    pub branch: Option<String>,
    /// The commit HEAD was at when the plan was made. The rebase is
    /// refused if HEAD has moved since, as the plan no longer fits.
    pub head: Oid,
    /// The commit the steps are replayed onto.
    pub onto: Oid,
    /// The steps, oldest commit first.
    pub steps: Vec<RebaseStep>,
}

/// Why a commit can't be edited on HEAD's branch.
#[derive(Debug)]
pub enum PlanError {
    /// HEAD has no commit yet.
    Unborn,
    /// The commit isn't one of the branch's own: HEAD's, or one its
    /// first parents lead back to.
    NotOnBranch,
    /// A commit from the one to edit up to HEAD (perhaps that one) is a
    /// merge, which replaying the branch would flatten.
    Merge(Oid),
    /// The commit is the first of the history, with no parent to
    /// replay it onto.
    Root,
    Git(git2::Error),
}

impl From<git2::Error> for PlanError {
    fn from(err: git2::Error) -> PlanError {
        PlanError::Git(err)
    }
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::Unborn => f.write_str("HEAD has no commits yet"),
            PlanError::NotOnBranch => f.write_str("the commit isn't on the branch HEAD is on"),
            PlanError::Merge(id) => write!(
                f,
                "{} is a merge, which replaying the branch would flatten",
                short_id(*id)
            ),
            PlanError::Root => f.write_str("the first commit has no parent to replay it onto"),
            PlanError::Git(err) => f.write_str(err.message()),
        }
    }
}

impl std::error::Error for PlanError {}

/// The commits an edit of `commit` replays, oldest first (`commit`
/// itself, and each one after it up to `head`), and the commit they go
/// onto, `commit`'s parent; `parents` gives a commit's parents, or
/// `None` for one it doesn't know, which ends the search. Only a
/// commit `head`'s first parents lead back to, through no merge, can
/// be edited: one beyond a merge, or reached through a merge's other
/// parents, would have the merge flattened by the replay.
pub(super) fn commits_to_edit(
    head: Oid,
    commit: Oid,
    mut parents: impl FnMut(Oid) -> Option<Vec<Oid>>,
) -> Result<(Oid, Vec<Oid>), PlanError> {
    let mut path = Vec::new();
    let mut at = head;
    loop {
        let Some(of) = parents(at) else {
            return Err(PlanError::NotOnBranch);
        };
        if of.len() > 1 {
            return Err(PlanError::Merge(at));
        }
        path.push(at);
        if at == commit {
            let onto = *of.first().ok_or(PlanError::Root)?;
            path.reverse();
            return Ok((onto, path));
        }
        at = *of.first().ok_or(PlanError::NotOnBranch)?;
    }
}

impl RebasePlan {
    /// The plan that edits `commit` on HEAD's branch: replay its
    /// changes onto its parent and stop there for the user, then pick
    /// each commit after it. Only a commit of the branch's own can be
    /// edited: HEAD's, or one HEAD's first parents lead back to through
    /// no merge (one beyond a merge, or reached through a merge's other
    /// parents, would have the merge flattened by the replay), and that
    /// has a parent to replay it onto.
    pub fn edit(repo: &Repository, commit: Oid) -> Result<RebasePlan, PlanError> {
        let head = head_commit(repo).map_err(|_| PlanError::Unborn)?;
        if head != commit && !repo.graph_descendant_of(head, commit)? {
            return Err(PlanError::NotOnBranch);
        }
        let (onto, commits) = commits_to_edit(head, commit, |id| {
            repo.find_commit(id)
                .ok()
                .map(|commit| commit.parent_ids().collect())
        })?;
        let steps = commits
            .into_iter()
            .map(|id| {
                let action = if id == commit {
                    RebaseAction::Edit
                } else {
                    RebaseAction::Pick
                };
                Ok(RebaseStep {
                    action,
                    commit: id,
                    summary: summary_of(&repo.find_commit(id)?),
                })
            })
            .collect::<Result<_, git2::Error>>()?;
        let branch = if repo.head_detached()? {
            None
        } else {
            repo.head()?.name().ok().map(str::to_owned)
        };
        Ok(RebasePlan {
            branch,
            head,
            onto,
            steps,
        })
    }
}

/// A commit's summary line, for a step.
fn summary_of(commit: &Commit<'_>) -> String {
    commit.summary().ok().flatten().unwrap_or("").to_owned()
}

/// How [`proceed`] went, when it didn't fail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Continued {
    /// The commit made of what was staged, if anything was.
    pub committed: Option<Oid>,
    /// How the rest of the rebase went, or `None` when it didn't go on
    /// since changes are left unstaged: the next commit of the stop
    /// (see the module's documentation).
    pub outcome: Option<Outcome>,
}

/// An interactive rebase under way, as kept in the repository.
#[derive(Clone, Debug)]
struct State {
    branch: Option<String>,
    orig_head: Oid,
    onto: Oid,
    /// The lines of the steps taken, as git's `done` has them; the last
    /// is the one stopped at, while stopped.
    done: Vec<String>,
    todo: Vec<RebaseStep>,
    /// The commit of the step stopped at, while stopped.
    stop: Option<Oid>,
}

impl State {
    fn total(&self) -> usize {
        self.done.len() + self.todo.len()
    }

    /// What the step stopped at does, as its line in `done` says.
    fn stopped_action(&self) -> Option<RebaseAction> {
        self.stop?;
        let line = self.done.last()?;
        RebaseAction::parse(line.split_whitespace().next()?)
    }
}

fn state_dir(repo: &Repository) -> PathBuf {
    repo.path().join(STATE_DIR)
}

/// A failure to read or write the rebase's state, as git's errors are
/// reported.
fn io_error(err: io::Error) -> OperationError {
    OperationError::Git(git2::Error::from_str(&err.to_string()))
}

/// The refusal for an interactive rebase the editor can't carry on
/// with: one git's command line began with steps the editor doesn't
/// know.
fn unsupported() -> OperationError {
    OperationError::InProgress(InProgress::Other("interactive rebase"))
}

/// Write the state of a rebase under way for git's command line and
/// the editor alike.
fn write_state(repo: &Repository, state: &State) -> Result<(), OperationError> {
    let dir = state_dir(repo);
    let write = |name: &str, text: String| fs::write(dir.join(name), text).map_err(io_error);
    fs::create_dir_all(&dir).map_err(io_error)?;
    write("interactive", String::new())?;
    write(
        "head-name",
        format!("{}\n", state.branch.as_deref().unwrap_or(DETACHED)),
    )?;
    write("onto", format!("{}\n", state.onto))?;
    write("orig-head", format!("{}\n", state.orig_head))?;
    write(
        "git-rebase-todo",
        lines(state.todo.iter().map(RebaseStep::line)),
    )?;
    write("done", lines(state.done.iter().cloned()))?;
    write("msgnum", format!("{}\n", state.done.len()))?;
    write("end", format!("{}\n", state.total()))?;
    match &state.stop {
        Some(stop) => {
            let commit = repo.find_commit(*stop)?;
            write("stopped-sha", format!("{stop}\n"))?;
            write("message", commit.message_raw().unwrap_or("").to_owned())?;
            write("author-script", author_script(&commit.author()))?;
        }
        None => {
            for name in ["stopped-sha", "message", "author-script", AMEND] {
                remove_file(&dir.join(name))?;
            }
        }
    }
    Ok(())
}

fn lines(lines: impl Iterator<Item = String>) -> String {
    lines.map(|line| line + "\n").collect()
}

/// Clear the state of the rebase, which is then no longer in progress.
fn remove_state(repo: &Repository) -> Result<(), OperationError> {
    match fs::remove_dir_all(state_dir(repo)) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(io_error(err)),
        _ => Ok(()),
    }
}

fn remove_file(path: &Path) -> Result<(), OperationError> {
    match fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(io_error(err)),
        _ => Ok(()),
    }
}

/// A commit's author as git's `author-script` has it, for `git rebase
/// --continue` to commit a stop's changes as them.
fn author_script(author: &Signature<'_>) -> String {
    let when = author.when();
    let offset = when.offset_minutes();
    let sign = if offset < 0 { '-' } else { '+' };
    let date = format!(
        "@{} {sign}{:02}{:02}",
        when.seconds(),
        offset.abs() / 60,
        offset.abs() % 60
    );
    format!(
        "GIT_AUTHOR_NAME={}\nGIT_AUTHOR_EMAIL={}\nGIT_AUTHOR_DATE={}\n",
        quote(&String::from_utf8_lossy(author.name_bytes())),
        quote(&String::from_utf8_lossy(author.email_bytes())),
        quote(&date)
    )
}

/// Quote text for a shell, as git's `author-script` is written.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The interactive rebase in progress, as kept in the repository.
fn read_state(repo: &Repository) -> Result<State, OperationError> {
    if repo.state() != RepositoryState::RebaseInteractive {
        return Err(OperationError::NothingInProgress);
    }
    let dir = state_dir(repo);
    let read = |name: &str| fs::read_to_string(dir.join(name)).map_err(|_| unsupported());
    let commit = |text: &str| -> Result<Oid, OperationError> {
        let text = text.trim();
        repo.revparse_single(text)
            .and_then(|object| object.peel_to_commit())
            .map(|commit| commit.id())
            .map_err(|_| unsupported())
    };
    let head_name = read("head-name")?.trim().to_owned();
    let branch = (head_name != DETACHED).then_some(head_name);
    let orig_head = commit(&read("orig-head")?)?;
    let onto = commit(&read("onto")?)?;
    let mut todo = Vec::new();
    for line in steps(&read("git-rebase-todo")?) {
        let mut words = line.splitn(3, char::is_whitespace);
        let action = words
            .next()
            .and_then(RebaseAction::parse)
            .ok_or_else(unsupported)?;
        let id = commit(words.next().ok_or_else(unsupported)?)?;
        // Newer gits write the summary as a comment: `pick 1a2b3c4 # Fix`.
        let summary = words.next().unwrap_or("").trim();
        let summary = summary.strip_prefix("# ").unwrap_or(summary).to_owned();
        todo.push(RebaseStep {
            action,
            commit: id,
            summary,
        });
    }
    let done = fs::read_to_string(dir.join("done")).unwrap_or_default();
    let done: Vec<String> = steps(&done).map(str::to_owned).collect();
    let stop = match fs::read_to_string(dir.join("stopped-sha")) {
        Ok(text) => Some(commit(&text)?),
        Err(_) => None,
    };
    Ok(State {
        branch,
        orig_head,
        onto,
        done,
        todo,
        stop,
    })
}

/// The lines of a list of steps that are steps: not blank, and not
/// comments.
fn steps(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

/// Whether the interactive rebase in progress is one the editor can
/// carry on with: one whose steps to come it knows.
pub(super) fn is_supported(repo: &Repository) -> bool {
    read_state(repo).is_ok()
}

/// Begin the rebase a plan describes, telling `progress` how many files
/// have been written of how many: put HEAD at the commit the plan goes
/// onto, detached, and take its steps until one stops (or all are
/// taken, when the branch is moved to the result). Refused, as a plain
/// rebase is, while another operation is under way or the working
/// tree has changes; and when HEAD has moved since the plan was made.
pub(super) fn start(
    repo: &Repository,
    plan: &RebasePlan,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Outcome, OperationError> {
    let head = ensure_ready(repo)?;
    let branch = if repo.head_detached()? {
        None
    } else {
        repo.head()?.name().ok().map(str::to_owned)
    };
    if head != plan.head || branch != plan.branch {
        return Err(OperationError::Git(git2::Error::from_str(
            "HEAD has moved since the rebase was planned",
        )));
    }
    let onto = repo.find_commit(plan.onto)?;
    check_submodules(repo, &onto)?;
    {
        let mut options = checkout_options(progress);
        repo.checkout_tree(onto.as_object(), Some(&mut options))?;
    }
    repo.set_head_detached(onto.id())?;
    let mut state = State {
        branch: plan.branch.clone(),
        orig_head: head,
        onto: onto.id(),
        done: Vec::new(),
        todo: plan.steps.clone(),
        stop: None,
    };
    write_state(repo, &state)?;
    replay(repo, &mut state, Tally::default(), progress)
}

/// Go on from where the interactive rebase in progress stopped:
/// commit what is staged, with `message` (which may not be empty, if
/// there is anything to commit) and the stopped commit's author; then,
/// unless changes are left unstaged for another commit, take the rest
/// of the steps, which may stop again. Refused while files are in
/// conflict, and with nothing staged and changes left unstaged, which
/// are to be staged and committed, or discarded, first. See the
/// module's documentation.
pub fn proceed(repo: &Repository, message: &str) -> Result<Continued, OperationError> {
    adopt_stop(repo)?;
    let mut state = read_state(repo)?;
    let progress = &mut |_, _| {};
    let mut tally = Tally::default();
    if repo.index()?.has_conflicts() {
        // A submodule conflict that has become resolvable since the
        // stop (its own branch rebased since, as the stop said to) is
        // resolved now.
        let settled = settle_part_way(repo, progress)?;
        tally.resolved = settled.resolved;
        tally.moved.extend(settled.moved);
        let index = repo.index()?;
        if index.has_conflicts() {
            return Err(OperationError::Unresolved {
                files: conflicted_files(&index)?,
                submodules: settled.unresolved,
            });
        }
    }
    let mut committed = None;
    if let Some(stop) = state.stop {
        let stopped = repo.find_commit(stop)?;
        if has_staged(repo)? {
            let message = message.trim();
            if message.is_empty() {
                return Err(OperationError::Git(git2::Error::from_str(
                    "a commit needs a message",
                )));
            }
            let signature = repo.signature()?;
            committed = commit_index(repo, &stopped.author(), &signature, message)?;
            tally.commits += usize::from(committed.is_some());
        }
        if let Some(untracked_only) = unstaged(repo, &stopped)? {
            if committed.is_some() {
                return Ok(Continued {
                    committed,
                    outcome: None,
                });
            }
            return Err(OperationError::Unstaged { untracked_only });
        }
        state.stop = None;
        write_state(repo, &state)?;
    }
    let outcome = replay(repo, &mut state, tally, progress)?;
    Ok(Continued {
        committed,
        outcome: Some(outcome),
    })
}

/// Make a stop git's command line made to edit a commit the editor's
/// kind. Git commits the commit before stopping, leaving HEAD on it and
/// nothing staged, to be amended (`git commit --amend`), and notes it
/// in `amend`; the editor stops with HEAD on the commit's parent and
/// its changes staged (see the module's documentation). So HEAD goes
/// back to the parent, the index and the working directory staying as
/// they are, as `git reset --soft HEAD~` does, and `amend` goes, after
/// which `git rebase --continue` commits what is staged, as the editor
/// does. Nothing is lost: the commit's changes are all staged, and
/// `stopped-sha` still names it. A stop where HEAD has moved on since
/// is left as it is, the commits made there being the user's. Returns
/// whether it made the stop over.
pub fn adopt_stop(repo: &Repository) -> Result<bool, OperationError> {
    if repo.state() != RepositoryState::RebaseInteractive {
        return Ok(false);
    }
    let path = state_dir(repo).join(AMEND);
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(false);
    };
    let Ok(amend) = Oid::from_str(text.trim()) else {
        return Ok(false);
    };
    let head = repo.head()?.peel_to_commit()?;
    if head.id() != amend || !repo.head_detached()? {
        return Ok(false);
    }
    // The first commit of the history has no parent to stage it on.
    let Some(parent) = head.parent_ids().next() else {
        return Ok(false);
    };
    repo.set_head_detached(parent)?;
    remove_file(&path)?;
    Ok(true)
}

/// Give up the interactive rebase in progress, as `git rebase --abort`
/// does: HEAD goes back on to the branch, the branch to the commit it
/// was at, and the index and working tree with it, as do the
/// submodules the rebase moved. Changes made since it began are lost.
pub fn abort(repo: &Repository) -> Result<(), OperationError> {
    if repo.state() != RepositoryState::RebaseInteractive {
        return Err(OperationError::NothingInProgress);
    }
    // Only what it takes to go back, so that one begun by git with
    // steps the editor doesn't know can be given up too.
    let dir = state_dir(repo);
    let read = |name: &str| fs::read_to_string(dir.join(name)).map_err(|_| unsupported());
    let orig_head = Oid::from_str(read("orig-head")?.trim()).map_err(|_| unsupported())?;
    let head_name = read("head-name")?.trim().to_owned();
    let orig = repo.find_commit(orig_head)?;
    if head_name == DETACHED {
        repo.set_head_detached(orig_head)?;
    } else {
        repo.set_head(&head_name)?;
    }
    repo.reset(orig.as_object(), ResetType::Hard, None)?;
    // A hard reset clears the rebase's state itself, as it clears any
    // operation's.
    remove_state(repo)?;
    follow_submodules(repo, orig_head, &mut |_, _| {})?;
    Ok(())
}

/// What the interactive rebase in progress is doing, if one is that
/// the editor can carry on with.
pub(super) fn status(repo: &Repository) -> Option<RebaseStatus> {
    let state = read_state(repo).ok()?;
    let branch = state
        .branch
        .as_deref()
        .map(|name| name.strip_prefix("refs/heads/").unwrap_or(name).to_owned());
    let message = state
        .stop
        .and_then(|stop| repo.find_commit(stop).ok())
        .and_then(|commit| commit.message().ok().map(|text| text.trim_end().to_owned()));
    let stop = state
        .stop
        .map(|stop| (state.stopped_action().unwrap_or(RebaseAction::Pick), stop));
    Some(RebaseStatus {
        branch,
        onto: short_id(state.onto),
        step: state.done.len(),
        total: state.total(),
        message,
        interactive: true,
        stop,
    })
}

/// How an interactive rebase went, for the status bar: `head` names the
/// branch rebased.
pub(super) fn summary(head: &str, outcome: &Outcome) -> String {
    match outcome {
        Outcome::Rebased {
            commits,
            skipped,
            submodules,
            resolved,
        } => {
            let noun = if *commits == 1 { "commit" } else { "commits" };
            let mut text = format!("Rebased {head}: {commits} {noun}");
            if *skipped > 0 {
                text.push_str(&format!(" ({skipped} left empty, dropped)"));
            }
            with_submodules(with_resolved(text, *resolved), *submodules)
        }
        Outcome::Editing {
            step,
            commit,
            action,
        } => {
            let then = match action {
                RebaseAction::Reword => "change its message and commit it",
                _ => "change, stage, and commit it",
            };
            format!(
                "Stopped to {} {} ({} of {}): {then}; the rebase goes on once nothing is left unstaged",
                action.word(),
                short_id(*commit),
                step.0,
                step.1
            )
        }
        Outcome::Conflicts {
            files,
            step,
            unresolved,
        } => {
            let noun = if *files == 1 { "file" } else { "files" };
            let at = match step {
                Some((step, total)) => format!(" at commit {step} of {total}"),
                None => String::new(),
            };
            let mut text = format!(
                "Rebasing {head} stopped{at} with conflicts in {files} {noun}: resolve and stage them, then commit"
            );
            for conflict in unresolved {
                text.push_str(&format!(
                    "; submodule {} left in conflict: {}",
                    conflict.path, conflict.why
                ));
            }
            text
        }
        // Not outcomes an interactive rebase has, but said plainly all
        // the same.
        Outcome::UpToDate | Outcome::FastForwarded { .. } | Outcome::Merged { .. } => {
            format!("Rebased {head}")
        }
    }
}

/// The commits a rebase has made and dropped as it goes, and the
/// submodules it has moved.
#[derive(Clone, Debug, Default)]
struct Tally {
    commits: usize,
    skipped: usize,
    moved: BTreeSet<String>,
    resolved: usize,
}

/// Take the steps left of a rebase, until one stops, and finish it once
/// all are taken. A step whose commit can't be written (its checkout
/// refused, say) is left to do, so that going on again tries it again.
fn replay(
    repo: &Repository,
    state: &mut State,
    mut tally: Tally,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Outcome, OperationError> {
    let signature = repo.signature()?;
    while !state.todo.is_empty() {
        let step = state.todo.remove(0);
        let commit = repo.find_commit(step.commit)?;
        let head = repo.find_commit(head_commit(repo)?)?;
        let fast_forward = commit.parent_ids().next() == Some(head.id());
        let written = if fast_forward {
            let mut options = checkout_options(progress);
            repo.checkout_tree(commit.as_object(), Some(&mut options))
        } else {
            merge_in(repo, &commit, &head, progress)
        };
        if let Err(err) = written {
            state.todo.insert(0, step);
            write_state(repo, state)?;
            return Err(err.into());
        }
        state.done.push(step.line());
        state.stop = Some(commit.id());
        write_state(repo, state)?;
        if fast_forward && step.action == RebaseAction::Pick {
            // The very commit: HEAD moves on to it.
            repo.set_head_detached(commit.id())?;
        }
        let settled = settle_part_way(repo, progress)?;
        tally.moved.extend(settled.moved);
        tally.resolved += settled.resolved;
        let index = repo.index()?;
        let at = (state.done.len(), state.total());
        if index.has_conflicts() {
            return Ok(Outcome::Conflicts {
                files: conflicted_files(&index)?,
                step: Some(at),
                unresolved: settled.unresolved,
            });
        }
        match step.action {
            action @ (RebaseAction::Edit | RebaseAction::Reword) => {
                return Ok(Outcome::Editing {
                    step: at,
                    commit: commit.id(),
                    action,
                });
            }
            RebaseAction::Pick if fast_forward => tally.commits += 1,
            RebaseAction::Pick => {
                match commit_index(repo, &commit.author(), &signature, &message_of(&commit))? {
                    Some(_) => tally.commits += 1,
                    None => tally.skipped += 1,
                }
            }
        }
        state.stop = None;
        write_state(repo, state)?;
    }
    finish(repo, state, tally, progress)
}

/// A commit's message, whole, to replay it with.
fn message_of(commit: &Commit<'_>) -> String {
    String::from_utf8_lossy(commit.message_raw_bytes()).into_owned()
}

/// Write `commit`'s change, merged onto `head`, into the index and the
/// working directory, conflicts and all, as `git cherry-pick` does.
fn merge_in(
    repo: &Repository,
    commit: &Commit<'_>,
    head: &Commit<'_>,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<(), git2::Error> {
    let mut index = repo.cherrypick_commit(commit, head, 0, None)?;
    let mut options = checkout_options(progress);
    repo.checkout_index(Some(&mut index), Some(&mut options))
}

/// Commit the index on HEAD as `author`, with `message`, unless it
/// changes nothing from HEAD. Returns the commit made, if any.
fn commit_index(
    repo: &Repository,
    author: &Signature<'_>,
    committer: &Signature<'_>,
    message: &str,
) -> Result<Option<Oid>, git2::Error> {
    let tree_id = repo.index()?.write_tree()?;
    let head = repo.head()?.peel_to_commit()?;
    if head.tree_id() == tree_id {
        return Ok(None);
    }
    let tree = repo.find_tree(tree_id)?;
    repo.commit(Some("HEAD"), author, committer, message, &tree, &[&head])
        .map(Some)
}

/// Move the branch to where the replay left HEAD, put HEAD back on it,
/// and clear the rebase's state, as `git rebase` does when it is done.
fn finish(
    repo: &Repository,
    state: &State,
    tally: Tally,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Outcome, OperationError> {
    let head = head_commit(repo)?;
    if let Some(branch) = &state.branch {
        let why = format!("rebase (finish): {branch} onto {}", state.onto);
        repo.reference_matching(branch, head, true, state.orig_head, &why)?;
        repo.set_head(branch)?;
    }
    remove_state(repo)?;
    // They are there already, unless the rebase had nothing to replay.
    let submodules = tally.moved.len() + follow_submodules(repo, head, progress)?;
    Ok(Outcome::Rebased {
        commits: tally.commits,
        skipped: tally.skipped,
        submodules,
        resolved: tally.resolved,
    })
}

/// Whether the index has anything staged: differs from HEAD.
fn has_staged(repo: &Repository) -> Result<bool, git2::Error> {
    let tree_id = repo.index()?.write_tree()?;
    Ok(repo.head()?.peel_to_commit()?.tree_id() != tree_id)
}

/// The untracked files of the working directory, each by path.
fn untracked_files(repo: &Repository) -> Result<Vec<String>, git2::Error> {
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .exclude_submodules(true);
    let statuses = repo.statuses(Some(&mut options))?;
    Ok(statuses
        .iter()
        .filter(|entry| entry.status().contains(Status::WT_NEW))
        .filter_map(|entry| entry.path().ok().map(str::to_owned))
        .collect())
}

/// Whether changes are left unstaged that hold up the rebase stopped at
/// `stopped`: changes to tracked files (a submodule moved counts,
/// changes inside one don't), or untracked files that `stopped` has,
/// which were unstaged from its changes. Says whether those are only
/// untracked files, if there are any.
fn unstaged(repo: &Repository, stopped: &Commit<'_>) -> Result<Option<bool>, OperationError> {
    let mut options = StatusOptions::new();
    options
        .include_untracked(false)
        .include_ignored(false)
        .exclude_submodules(true);
    let statuses = repo.statuses(Some(&mut options))?;
    let tracked = statuses.iter().any(|entry| {
        entry.status().intersects(
            Status::WT_MODIFIED | Status::WT_DELETED | Status::WT_TYPECHANGE | Status::WT_RENAMED,
        )
    });
    let moved = repo.submodules()?.iter().any(|submodule| {
        matches!(
            (submodule.index_id(), submodule.workdir_id()),
            (Some(index), Some(workdir)) if index != workdir
        )
    });
    if tracked || moved {
        return Ok(Some(false));
    }
    let tree = stopped.tree()?;
    let unstaged_from = untracked_files(repo)?
        .iter()
        .any(|path| tree.get_path(Path::new(path)).is_ok());
    Ok(unstaged_from.then_some(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::history::tests::TestRepo;
    use crate::git::operation::tests::{read, repo, sync};
    use crate::git::operation::{Integration, in_progress};
    use crate::git::rebase::rebase_status;

    fn head(repo: &Repository) -> Oid {
        repo.head().unwrap().target().unwrap()
    }

    fn stage(repo: &Repository, path: &str) {
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(path)).unwrap();
        index.write().unwrap();
    }

    /// Put a path of the index back to HEAD's version, leaving the
    /// working directory alone.
    fn unstage(repo: &Repository, path: &str) {
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.reset_default(Some(head.as_object()), [path]).unwrap();
    }

    /// The messages of HEAD's commits, newest first.
    fn log(repo: &Repository) -> Vec<String> {
        let mut walk = repo.revwalk().unwrap();
        walk.push_head().unwrap();
        walk.map(|id| {
            let commit = repo.find_commit(id.unwrap()).unwrap();
            commit.message().unwrap().trim_end().to_owned()
        })
        .collect()
    }

    /// A branch of four commits: `Base` adding `a.txt`, `Two` adding
    /// `b.txt` and changing `a.txt`, `Three` changing `b.txt`, and
    /// `Four` adding `c.txt`, with HEAD on it, by someone other than
    /// the one the configuration names.
    fn branch(t: &mut TestRepo) -> [Oid; 4] {
        let base = t.commit(&[("a.txt", "a\n")], "Base", &[]);
        let two = t.commit(&[("a.txt", "a2\n"), ("b.txt", "b\n")], "Two", &[base]);
        let three = t.commit(&[("b.txt", "b3\n")], "Three", &[two]);
        let four = t.commit(&[("c.txt", "c\n")], "Four", &[three]);
        sync(t);
        [base, two, three, four]
    }

    fn run(repo: &Repository, plan: &RebasePlan) -> Result<Outcome, OperationError> {
        Integration::Interactive(plan.clone()).run_with(repo, &mut |_, _| {})
    }

    /// Begin editing `commit`, which must stop there.
    fn edit(repo: &Repository, commit: Oid) -> RebasePlan {
        let plan = RebasePlan::edit(repo, commit).unwrap();
        let outcome = run(repo, &plan).unwrap();
        assert!(
            matches!(outcome, Outcome::Editing { commit: c, .. } if c == commit),
            "{outcome:?}"
        );
        plan
    }

    #[test]
    fn the_plan_edits_the_commit_and_picks_the_ones_after_it() {
        let mut t = repo();
        let [base, two, three, four] = branch(&mut t);
        let plan = RebasePlan::edit(&t.repo, two).unwrap();
        assert_eq!(plan.onto, base);
        assert_eq!(plan.head, four);
        assert_eq!(plan.branch.as_deref(), Some("refs/heads/master"));
        let steps: Vec<_> = plan
            .steps
            .iter()
            .map(|step| (step.action, step.commit, step.summary.as_str()))
            .collect();
        assert_eq!(
            steps,
            [
                (RebaseAction::Edit, two, "Two"),
                (RebaseAction::Pick, three, "Three"),
                (RebaseAction::Pick, four, "Four"),
            ]
        );
        // HEAD's own commit is a plan of one step.
        let plan = RebasePlan::edit(&t.repo, four).unwrap();
        assert_eq!(plan.onto, three);
        assert_eq!(plan.steps.len(), 1);
    }

    #[test]
    fn only_a_commit_of_the_branch_short_of_a_merge_can_be_edited() {
        let mut t = repo();
        let [base, two, _, four] = branch(&mut t);
        assert!(matches!(
            RebasePlan::edit(&t.repo, base),
            Err(PlanError::Root)
        ));
        // Another branch's commit.
        t.branch("side", two);
        t.checkout("side");
        let side = t.commit(&[("s.txt", "s\n")], "Side", &[two]);
        t.checkout("master");
        sync(&t);
        assert!(matches!(
            RebasePlan::edit(&t.repo, side),
            Err(PlanError::NotOnBranch)
        ));
        // A merge between HEAD and the commit, or the commit itself.
        let merge = t.commit(&[("m.txt", "m\n")], "Merge", &[four, side]);
        let after = t.commit(&[("n.txt", "n\n")], "After", &[merge]);
        sync(&t);
        assert!(matches!(
            RebasePlan::edit(&t.repo, two),
            Err(PlanError::Merge(id)) if id == merge
        ));
        assert!(matches!(
            RebasePlan::edit(&t.repo, side),
            Err(PlanError::Merge(id)) if id == merge
        ));
        assert!(matches!(
            RebasePlan::edit(&t.repo, merge),
            Err(PlanError::Merge(id)) if id == merge
        ));
        assert!(RebasePlan::edit(&t.repo, after).is_ok());
    }

    #[test]
    fn an_edit_stops_with_the_commits_changes_staged_on_its_parent() {
        let mut t = repo();
        let [base, two, ..] = branch(&mut t);
        edit(&t.repo, two);
        assert_eq!(t.repo.state(), RepositoryState::RebaseInteractive);
        assert_eq!(in_progress(&t.repo), Some(InProgress::Rebase));
        assert_eq!(head(&t.repo), base);
        assert!(t.repo.head_detached().unwrap());
        assert_eq!(read(&t, "a.txt"), "a2\n");
        assert_eq!(read(&t, "b.txt"), "b\n");
        let status = t.repo.statuses(None).unwrap();
        let staged: Vec<_> = status
            .iter()
            .map(|entry| (entry.path().unwrap().to_owned(), entry.status()))
            .collect();
        assert_eq!(
            staged,
            [
                ("a.txt".to_owned(), Status::INDEX_MODIFIED),
                ("b.txt".to_owned(), Status::INDEX_NEW),
            ]
        );
        assert_eq!(
            rebase_status(&t.repo).unwrap(),
            RebaseStatus {
                branch: Some("master".to_owned()),
                onto: short_id(base),
                step: 1,
                total: 3,
                message: Some("Two".to_owned()),
                interactive: true,
                stop: Some((RebaseAction::Edit, two)),
            }
        );
    }

    #[test]
    fn rewording_commits_the_stop_and_replays_the_rest() {
        let mut t = repo();
        let [base, two, three, four] = branch(&mut t);
        edit(&t.repo, two);
        let continued = proceed(&t.repo, "Two, reworded\n").unwrap();
        assert_eq!(
            continued.outcome,
            Some(Outcome::Rebased {
                commits: 3,
                skipped: 0,
                submodules: 0,
                resolved: 0,
            })
        );
        assert_eq!(t.repo.state(), RepositoryState::Clean);
        assert!(!state_dir(&t.repo).exists());
        assert_eq!(t.repo.head().unwrap().shorthand().unwrap(), "master");
        assert_eq!(log(&t.repo), ["Four", "Three", "Two, reworded", "Base"]);
        let new_two = t.repo.find_commit(continued.committed.unwrap()).unwrap();
        assert_eq!(new_two.parent_ids().collect::<Vec<_>>(), [base]);
        // The author is the edited commit's; the committer is whoever
        // the configuration names.
        let old_two = t.repo.find_commit(two).unwrap();
        assert_eq!(new_two.author().when(), old_two.author().when());
        assert_eq!(new_two.author().name(), old_two.author().name());
        assert_eq!(new_two.tree_id(), old_two.tree_id());
        let new_four = t.repo.find_commit(head(&t.repo)).unwrap();
        assert_ne!(new_four.id(), four);
        assert_ne!(new_four.parent_id(0).unwrap(), three);
        assert_eq!(
            new_four.tree_id(),
            t.repo.find_commit(four).unwrap().tree_id()
        );
        assert!(t.repo.statuses(None).unwrap().is_empty());
    }

    /// Make an edit stop of the editor's into git's kind: the commit
    /// made (the very one, as git fast-forwards to it), HEAD on it, and
    /// `amend` naming it.
    fn as_git_stops(repo: &Repository, commit: Oid) {
        repo.set_head_detached(commit).unwrap();
        fs::write(state_dir(repo).join(AMEND), format!("{commit}\n")).unwrap();
        assert!(repo.statuses(None).unwrap().is_empty());
    }

    #[test]
    fn gits_stop_to_edit_is_made_one_with_the_changes_staged() {
        let mut t = repo();
        let [base, two, _, four] = branch(&mut t);
        edit(&t.repo, two);
        as_git_stops(&t.repo, two);
        assert!(adopt_stop(&t.repo).unwrap());
        assert!(!state_dir(&t.repo).join(AMEND).exists());
        assert_eq!(head(&t.repo), base);
        let staged: Vec<_> = t
            .repo
            .statuses(None)
            .unwrap()
            .iter()
            .map(|entry| (entry.path().unwrap().to_owned(), entry.status()))
            .collect();
        assert_eq!(
            staged,
            [
                ("a.txt".to_owned(), Status::INDEX_MODIFIED),
                ("b.txt".to_owned(), Status::INDEX_NEW),
            ]
        );
        assert_eq!(
            rebase_status(&t.repo).unwrap().message.as_deref(),
            Some("Two")
        );
        // Once only.
        assert!(!adopt_stop(&t.repo).unwrap());
        let continued = proceed(&t.repo, "Two, reworded").unwrap();
        assert!(
            matches!(continued.outcome, Some(Outcome::Rebased { commits: 3, .. })),
            "{continued:?}"
        );
        assert_eq!(log(&t.repo), ["Four", "Three", "Two, reworded", "Base"]);

        // Going on from git's stop takes it over too: committing what
        // was the commit, as it was, and the rest after it.
        let three_now = t
            .repo
            .find_commit(head(&t.repo))
            .unwrap()
            .parent_id(0)
            .unwrap();
        edit(&t.repo, three_now);
        as_git_stops(&t.repo, three_now);
        let continued = proceed(&t.repo, "Three").unwrap();
        assert!(
            matches!(continued.outcome, Some(Outcome::Rebased { commits: 2, .. })),
            "{continued:?}"
        );
        assert_eq!(log(&t.repo), ["Four", "Three", "Two, reworded", "Base"]);
        assert_eq!(
            t.repo.find_commit(head(&t.repo)).unwrap().tree_id(),
            t.repo.find_commit(four).unwrap().tree_id()
        );
        assert!(t.repo.statuses(None).unwrap().is_empty());
    }

    #[test]
    fn gits_stop_is_left_alone_once_commits_are_made_there() {
        let mut t = repo();
        let [_, two, ..] = branch(&mut t);
        edit(&t.repo, two);
        as_git_stops(&t.repo, two);
        // As `git commit --amend` would leave it.
        let amended = t
            .repo
            .find_commit(two)
            .unwrap()
            .amend(Some("HEAD"), None, None, None, Some("Two, amended"), None)
            .unwrap();
        assert!(!adopt_stop(&t.repo).unwrap());
        assert_eq!(head(&t.repo), amended);
        assert!(state_dir(&t.repo).join(AMEND).exists());
        // Nothing to commit, nothing unstaged: it goes on.
        let continued = proceed(&t.repo, "").unwrap();
        assert_eq!(continued.committed, None);
        assert!(
            matches!(continued.outcome, Some(Outcome::Rebased { commits: 2, .. })),
            "{continued:?}"
        );
        assert_eq!(log(&t.repo), ["Four", "Three", "Two, amended", "Base"]);
    }

    #[test]
    fn a_reword_stops_as_an_edit_does_for_a_new_message() {
        let mut t = repo();
        let [base, two, three, four] = branch(&mut t);
        let mut plan = RebasePlan::edit(&t.repo, two).unwrap();
        plan.steps[0].action = RebaseAction::Reword;
        let outcome = run(&t.repo, &plan).unwrap();
        assert_eq!(
            outcome,
            Outcome::Editing {
                step: (1, 3),
                commit: two,
                action: RebaseAction::Reword,
            }
        );
        assert_eq!(
            summary("master", &outcome),
            format!(
                "Stopped to reword {} (1 of 3): change its message and commit it; the rebase goes on once nothing is left unstaged",
                short_id(two)
            )
        );
        let done = fs::read_to_string(state_dir(&t.repo).join("done")).unwrap();
        assert_eq!(done, format!("reword {two} Two\n"));
        let status = rebase_status(&t.repo).unwrap();
        assert_eq!(status.stop, Some((RebaseAction::Reword, two)));
        assert_eq!(status.message.as_deref(), Some("Two"));
        assert_eq!(head(&t.repo), base);

        // A reword git's way of writing it, abbreviated, among the steps
        // to come (as one git's command line began would have).
        let todo = state_dir(&t.repo).join("git-rebase-todo");
        let text = fs::read_to_string(&todo).unwrap();
        let short = &three.to_string()[..7];
        let text = text.replace(
            &format!("pick {three} Three"),
            &format!("r {short} # Three"),
        );
        fs::write(&todo, text).unwrap();
        assert_eq!(in_progress(&t.repo), Some(InProgress::Rebase));
        let step = &read_state(&t.repo).unwrap().todo[0];
        assert_eq!(
            (step.action, step.commit, step.summary.as_str()),
            (RebaseAction::Reword, three, "Three")
        );

        let continued = proceed(&t.repo, "Two, reworded").unwrap();
        assert_eq!(
            continued.outcome,
            Some(Outcome::Editing {
                step: (2, 3),
                commit: three,
                action: RebaseAction::Reword,
            })
        );
        assert_eq!(
            rebase_status(&t.repo).unwrap().stop,
            Some((RebaseAction::Reword, three))
        );
        let continued = proceed(&t.repo, "Three, reworded").unwrap();
        assert!(
            matches!(continued.outcome, Some(Outcome::Rebased { commits: 2, .. })),
            "{continued:?}"
        );
        assert_eq!(
            log(&t.repo),
            ["Four", "Three, reworded", "Two, reworded", "Base"]
        );
        // Only the messages changed.
        let new_four = t.repo.find_commit(head(&t.repo)).unwrap();
        let old_four = t.repo.find_commit(four).unwrap();
        assert_eq!(new_four.tree_id(), old_four.tree_id());
        assert!(t.repo.statuses(None).unwrap().is_empty());
    }

    #[test]
    fn a_split_commits_part_waits_for_the_rest_and_then_goes_on() {
        let mut t = repo();
        let [base, two, ..] = branch(&mut t);
        edit(&t.repo, two);
        unstage(&t.repo, "b.txt");
        let continued = proceed(&t.repo, "Two: a").unwrap();
        // b.txt is left, unstaged (untracked, as two added it): the
        // rebase waits for it.
        assert_eq!(continued.outcome, None);
        let first = continued.committed.unwrap();
        assert_eq!(head(&t.repo), first);
        assert_eq!(t.repo.state(), RepositoryState::RebaseInteractive);
        // Still stopped at Two, whose message a frontend offers for the
        // first commit made there.
        assert_eq!(
            rebase_status(&t.repo).unwrap().message.as_deref(),
            Some("Two")
        );
        // Nothing staged, something unstaged: nothing to do yet.
        let err = proceed(&t.repo, "").unwrap_err();
        assert!(
            matches!(
                err,
                OperationError::Unstaged {
                    untracked_only: true
                }
            ),
            "{err:?}"
        );
        stage(&t.repo, "b.txt");
        let continued = proceed(&t.repo, "Two: b").unwrap();
        // Two: b, and the two replayed after it.
        assert!(
            matches!(continued.outcome, Some(Outcome::Rebased { commits: 3, .. })),
            "{continued:?}"
        );
        assert_eq!(log(&t.repo), ["Four", "Three", "Two: b", "Two: a", "Base"]);
        let first = t.repo.find_commit(first).unwrap();
        assert_eq!(first.parent_ids().collect::<Vec<_>>(), [base]);
        assert_eq!(read(&t, "b.txt"), "b3\n");
        assert!(t.repo.statuses(None).unwrap().is_empty());
    }

    #[test]
    fn a_stop_left_with_nothing_goes_on_without_an_empty_commit() {
        let mut t = repo();
        let [_, two, three, _] = branch(&mut t);
        // Edit three, then throw its change away.
        edit(&t.repo, three);
        unstage(&t.repo, "b.txt");
        let err = proceed(&t.repo, "Three").unwrap_err();
        assert!(
            matches!(
                err,
                OperationError::Unstaged {
                    untracked_only: false
                }
            ),
            "{err:?}"
        );
        assert_eq!(head(&t.repo), two);
        t.repo
            .checkout_index(None, Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        let continued = proceed(&t.repo, "Three").unwrap();
        assert_eq!(continued.committed, None);
        assert_eq!(
            continued.outcome,
            Some(Outcome::Rebased {
                commits: 1,
                skipped: 0,
                submodules: 0,
                resolved: 0,
            })
        );
        assert_eq!(log(&t.repo), ["Four", "Two", "Base"]);
        assert_eq!(read(&t, "b.txt"), "b\n");
    }

    #[test]
    fn an_untracked_file_the_commit_did_not_add_holds_nothing_up() {
        let mut t = repo();
        let [_, two, ..] = branch(&mut t);
        fs::write(t.path().join("notes.txt"), "mine\n").unwrap();
        edit(&t.repo, two);
        // Changed and staged; the file from before, and one made since,
        // are left alone.
        fs::write(t.path().join("scratch.txt"), "new\n").unwrap();
        fs::write(t.path().join("a.txt"), "a2, edited\n").unwrap();
        stage(&t.repo, "a.txt");
        let continued = proceed(&t.repo, "Two, edited").unwrap();
        assert!(
            matches!(continued.outcome, Some(Outcome::Rebased { commits: 3, .. })),
            "{continued:?}"
        );
        assert_eq!(read(&t, "a.txt"), "a2, edited\n");
        assert_eq!(read(&t, "notes.txt"), "mine\n");
        assert_eq!(read(&t, "scratch.txt"), "new\n");
    }

    #[test]
    fn an_edit_that_conflicts_later_stops_there_to_be_resolved() {
        let mut t = repo();
        let [_, two, ..] = branch(&mut t);
        edit(&t.repo, two);
        // Three changes b.txt, which this edit changes too.
        fs::write(t.path().join("b.txt"), "b, edited\n").unwrap();
        stage(&t.repo, "b.txt");
        let continued = proceed(&t.repo, "Two").unwrap();
        assert_eq!(
            continued.outcome,
            Some(Outcome::Conflicts {
                files: 1,
                step: Some((2, 3)),
                unresolved: Vec::new(),
            })
        );
        assert!(read(&t, "b.txt").contains("<<<<<<<"));
        let status = rebase_status(&t.repo).unwrap();
        assert_eq!(status.step, 2);
        assert_eq!(status.message.as_deref(), Some("Three"));
        assert!(matches!(status.stop, Some((RebaseAction::Pick, _))));
        assert!(matches!(
            proceed(&t.repo, "Three"),
            Err(OperationError::Unresolved { files: 1, .. })
        ));
        fs::write(t.path().join("b.txt"), "b3, edited\n").unwrap();
        stage(&t.repo, "b.txt");
        let continued = proceed(&t.repo, "Three, resolved").unwrap();
        assert!(
            matches!(continued.outcome, Some(Outcome::Rebased { commits: 2, .. })),
            "{continued:?}"
        );
        assert_eq!(log(&t.repo), ["Four", "Three, resolved", "Two", "Base"]);
        assert_eq!(read(&t, "b.txt"), "b3, edited\n");
    }

    #[test]
    fn aborting_puts_the_branch_back() {
        let mut t = repo();
        let [_, two, _, four] = branch(&mut t);
        edit(&t.repo, two);
        fs::write(t.path().join("a.txt"), "scribbled\n").unwrap();
        abort(&t.repo).unwrap();
        assert_eq!(t.repo.state(), RepositoryState::Clean);
        assert_eq!(t.repo.head().unwrap().shorthand().unwrap(), "master");
        assert_eq!(head(&t.repo), four);
        assert_eq!(read(&t, "a.txt"), "a2\n");
        assert!(t.repo.statuses(None).unwrap().is_empty());
        assert!(matches!(
            abort(&t.repo),
            Err(OperationError::NothingInProgress)
        ));
        assert!(matches!(
            proceed(&t.repo, ""),
            Err(OperationError::NothingInProgress)
        ));
    }

    #[test]
    fn a_plan_refuses_a_dirty_tree_and_a_head_that_moved() {
        let mut t = repo();
        let [_, two, three, _] = branch(&mut t);
        let plan = RebasePlan::edit(&t.repo, two).unwrap();
        fs::write(t.path().join("a.txt"), "dirty\n").unwrap();
        assert!(matches!(
            run(&t.repo, &plan),
            Err(OperationError::Checkout(_))
        ));
        sync(&t);
        t.repo.head().unwrap().set_target(three, "test").unwrap();
        sync(&t);
        assert_eq!(
            run(&t.repo, &plan).unwrap_err().to_string(),
            "HEAD has moved since the rebase was planned"
        );
    }

    #[test]
    fn steps_the_editor_does_not_know_leave_the_rebase_to_git() {
        let mut t = repo();
        let [_, two, ..] = branch(&mut t);
        edit(&t.repo, two);
        let todo = state_dir(&t.repo).join("git-rebase-todo");
        let text = fs::read_to_string(&todo).unwrap();
        fs::write(&todo, format!("exec make\n{text}")).unwrap();
        assert_eq!(
            in_progress(&t.repo),
            Some(InProgress::Other("interactive rebase"))
        );
        assert!(rebase_status(&t.repo).is_none());
        // It can still be given up.
        abort(&t.repo).unwrap();
        assert_eq!(t.repo.state(), RepositoryState::Clean);
    }

    #[test]
    fn the_author_script_is_quoted_for_a_shell() {
        let author = Signature::new("O'Neil", "o@example.com", &git2::Time::new(100, -90)).unwrap();
        assert_eq!(
            author_script(&author),
            "GIT_AUTHOR_NAME='O'\\''Neil'\nGIT_AUTHOR_EMAIL='o@example.com'\nGIT_AUTHOR_DATE='@100 -0130'\n"
        );
    }
}
