//! Fetching from a repository's remotes: what `git fetch --all` does,
//! and no more. Every remote is asked for its branches and tags, by
//! its configured refspecs, and the remote-tracking references
//! (`refs/remotes/<remote>/*`) and tags are brought up to date; whether
//! tags come along and whether gone branches are pruned follow the
//! repository's configuration, as they do for git. Nothing is merged,
//! and neither the working tree nor HEAD nor any local branch is
//! touched.
//!
//! Submodules are fetched after the repository as git's
//! `fetch.recurseSubmodules` says: never when it is `false`, all of
//! them when it is `true`, and otherwise (`on-demand`, the default) only
//! those with a commit missing. A submodule is missing a commit when a
//! reference this fetch brought in or moved points at a tree whose
//! gitlink for it names a commit the submodule's repository doesn't
//! have. Only the references that moved count, as for git: a branch or
//! tag pointing the submodule at a commit its remote no longer has
//! (a feature branch whose submodule commit was never pushed, say)
//! would otherwise be fetched for, in vain, every time. So a fetch
//! that brings nothing in costs nothing more, and one that brings in a
//! bump of a submodule fetches that submodule, once. The caller may
//! also name commits it wants ([`Fetch::start`]'s `wanted`): one a diff
//! couldn't show, say, whether it came in before or was never at a
//! tip. Submodules nest: one fetched has its own submodules checked
//! the same way. A submodule that isn't initialized is left alone.
//!
//! A fetch runs in the background (see [`Fetch`]) and can't ask for
//! anything, so it uses whatever credentials are at hand: for SSH the
//! agent, then the usual keys in `~/.ssh`, unencrypted; for HTTPS the
//! credential helpers git is configured with (the macOS keychain, say)
//! and the system's default mechanisms (Kerberos, NTLM). A remote that
//! can't be reached, or won't let us in, is reported and the others
//! are fetched all the same.

use git2::{
    Cred, CredentialType, FetchOptions, ObjectType, Oid, ProxyOptions, RemoteCallbacks, Repository,
};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

/// How a fetch went, remote by remote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FetchReport {
    /// The remotes fetched, in the order they were tried.
    pub fetched: Vec<String>,
    /// How many references changed across all of them: branches or
    /// tags that are new, moved, or (when pruning) gone.
    pub updated: usize,
    /// The remotes that couldn't be fetched, and why.
    pub failed: Vec<(String, String)>,
    /// The submodules fetched after the repository, by path (a nested
    /// one's joined with `/`), each with how its own fetch went.
    pub submodules: Vec<(String, FetchReport)>,
}

impl FetchReport {
    /// The report in a sentence, for the status bar: which remotes were
    /// fetched and what came of it, then any that failed.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.fetched.is_empty() {
            let what = match self.updated {
                0 => "up to date".to_owned(),
                1 => "1 ref updated".to_owned(),
                n => format!("{n} refs updated"),
            };
            parts.push(format!("Fetched {}: {what}", self.fetched.join(", ")));
        }
        for (remote, why) in &self.failed {
            parts.push(format!("Could not fetch {remote}: {why}"));
        }
        for (path, report) in &self.submodules {
            parts.push(format!("{path}: {}", report.summary()));
        }
        if parts.is_empty() {
            "No remotes to fetch from".to_owned()
        } else {
            parts.join(" · ")
        }
    }
}

/// Whether submodules are fetched after the repository: git's
/// `fetch.recurseSubmodules`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Recurse {
    No,
    /// Only those missing a commit the repository points at.
    OnDemand,
    Always,
}

/// The repository's `fetch.recurseSubmodules`: a boolean or
/// `on-demand`, which is the default.
fn recurse_mode(repo: &Repository) -> Recurse {
    let Ok(config) = repo.config() else {
        return Recurse::OnDemand;
    };
    match config.get_string("fetch.recurseSubmodules") {
        Ok(value) if value.eq_ignore_ascii_case("on-demand") => Recurse::OnDemand,
        Ok(value) => match git2::Config::parse_bool(&value) {
            Ok(true) => Recurse::Always,
            Ok(false) => Recurse::No,
            Err(_) => Recurse::OnDemand,
        },
        Err(_) => Recurse::OnDemand,
    }
}

enum Message {
    /// Which remote is being fetched now.
    Fetching(String),
    Done(FetchReport),
    /// The repository couldn't be opened, or its remotes listed.
    Failed(String),
}

/// A fetch under way in the background. [`poll`](Self::poll) takes in
/// its progress; once it is done the report says how it went.
pub struct Fetch {
    receiver: Receiver<Message>,
    /// The remote being fetched now, once one is.
    current: Option<String>,
    done: Option<Result<FetchReport, String>>,
}

impl Fetch {
    /// Start fetching every remote of the repository whose git
    /// directory is `git_dir` (see [`Repository::path`]), and then of
    /// the submodules that need it (see the module's documentation).
    /// `wanted` names commits the caller knows to be missing from
    /// submodules, by the submodule's path from the repository's
    /// working directory: those submodules are fetched whatever else
    /// says.
    pub fn start(git_dir: &Path, wanted: Vec<(String, Oid)>) -> Result<Fetch, git2::Error> {
        let (sender, receiver) = mpsc::channel();
        let git_dir = git_dir.to_path_buf();
        thread::Builder::new()
            .name("git-fetch".to_owned())
            .spawn(move || {
                let outcome = fetch_all(&git_dir, &wanted, &mut |remote: &str| {
                    let _ = sender.send(Message::Fetching(remote.to_owned()));
                });
                let _ = sender.send(match outcome {
                    Ok(report) => Message::Done(report),
                    Err(err) => Message::Failed(err.message().to_owned()),
                });
            })
            .map_err(|err| git2::Error::from_str(&err.to_string()))?;
        Ok(Fetch {
            receiver,
            current: None,
            done: None,
        })
    }

    /// Take in what the fetch has reported so far. Returns whether
    /// anything changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            match self.receiver.try_recv() {
                Ok(Message::Fetching(remote)) => {
                    self.current = Some(remote);
                    changed = true;
                }
                Ok(Message::Done(report)) => {
                    self.done = Some(Ok(report));
                    changed = true;
                }
                Ok(Message::Failed(message)) => {
                    self.done = Some(Err(message));
                    changed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.done.is_none() {
                        self.done = Some(Err("the fetch stopped".to_owned()));
                        changed = true;
                    }
                    break;
                }
            }
        }
        changed
    }

    /// The remote being fetched now, if the fetch has got that far.
    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// Whether the fetch is finished (as of the last poll).
    pub fn is_done(&self) -> bool {
        self.done.is_some()
    }

    /// The outcome, once [`is_done`](Self::is_done): the report, or why
    /// nothing could be fetched at all.
    pub fn outcome(self) -> Option<Result<FetchReport, String>> {
        self.done
    }
}

/// Fetch every remote of the repository at `git_dir`, then of the
/// submodules that need it, telling `began` the name of each remote
/// as it starts (a submodule's prefixed with its path). Fails only
/// when the repository can't be opened or its remotes listed; a remote
/// that fails is in the report.
fn fetch_all(
    git_dir: &Path,
    wanted: &[(String, Oid)],
    began: &mut dyn FnMut(&str),
) -> Result<FetchReport, git2::Error> {
    let repo = Repository::open(git_dir)?;
    let (mut report, moved) = fetch_remotes(&repo, "", began)?;
    fetch_submodules(&repo, "", wanted, &moved, began, &mut report);
    Ok(report)
}

/// Fetch every remote of `repo`, telling `began` each remote's name
/// after `prefix`. Returns the report and the names of the references
/// the fetch brought in or moved. Fails only when the remotes can't be
/// listed.
fn fetch_remotes(
    repo: &Repository,
    prefix: &str,
    began: &mut dyn FnMut(&str),
) -> Result<(FetchReport, Vec<String>), git2::Error> {
    let names = repo.remotes()?;
    let mut report = FetchReport::default();
    let mut moved = Vec::new();
    for name in names.iter() {
        // A remote whose name isn't UTF-8 can't be shown; skip it.
        let Ok(Some(name)) = name else { continue };
        began(&format!("{prefix}{name}"));
        match fetch_remote(repo, name) {
            Ok(updated) => {
                report.fetched.push(name.to_owned());
                report.updated += updated.len();
                moved.extend(updated);
            }
            Err(err) => report
                .failed
                .push((name.to_owned(), err.message().trim().to_owned())),
        }
    }
    Ok((report, moved))
}

/// Fetch the submodules of `repo` that need it, as its configuration
/// says (see [`Recurse`]), and their submodules in turn, adding each
/// one fetched to `report` under its path from the top repository
/// (`prefix` is the way down to `repo`). `wanted` is relative to
/// `repo`, and `moved` names the references the fetch of `repo`
/// brought in or moved.
fn fetch_submodules(
    repo: &Repository,
    prefix: &str,
    wanted: &[(String, Oid)],
    moved: &[String],
    began: &mut dyn FnMut(&str),
    report: &mut FetchReport,
) {
    let mode = recurse_mode(repo);
    if mode == Recurse::No {
        return;
    }
    if mode == Recurse::OnDemand && moved.is_empty() && wanted.is_empty() {
        return;
    }
    let Ok(submodules) = repo.submodules() else {
        return;
    };
    if submodules.is_empty() {
        return;
    }
    let tips = if mode == Recurse::OnDemand {
        moved_trees(repo, moved)
    } else {
        Vec::new()
    };
    for submodule in submodules {
        let Ok(sub) = submodule.open() else {
            // Not initialized: nothing to fetch into.
            continue;
        };
        let relative = submodule.path().to_string_lossy().replace('\\', "/");
        let path = format!("{prefix}{relative}");
        let wanted_here: Vec<Oid> = wanted
            .iter()
            .filter(|(at, _)| *at == relative)
            .map(|(_, id)| *id)
            .collect();
        let wanted_below: Vec<(String, Oid)> = wanted
            .iter()
            .filter_map(|(at, id)| {
                at.strip_prefix(&format!("{relative}/"))
                    .map(|rest| (rest.to_owned(), *id))
            })
            .collect();
        let needed =
            mode == Recurse::Always || needs_fetch(&sub, submodule.path(), &wanted_here, &tips);
        let mut moved_below = Vec::new();
        if needed {
            let outcome = match fetch_remotes(&sub, &format!("{path}: "), began) {
                Ok((outcome, moved)) => {
                    moved_below = moved;
                    outcome
                }
                Err(err) => FetchReport {
                    failed: vec![("remotes".to_owned(), err.message().trim().to_owned())],
                    ..FetchReport::default()
                },
            };
            report.submodules.push((path.clone(), outcome));
        }
        fetch_submodules(
            &sub,
            &format!("{path}/"),
            &wanted_below,
            &moved_below,
            began,
            report,
        );
    }
}

/// Whether the submodule `sub` at `path` is missing a commit: one of
/// `wanted`, or one a tip's tree points it at.
fn needs_fetch(sub: &Repository, path: &Path, wanted: &[Oid], tips: &[git2::Tree<'_>]) -> bool {
    let Ok(odb) = sub.odb() else {
        return false;
    };
    let missing = |id: Oid| !odb.exists(id);
    if wanted.iter().any(|id| missing(*id)) {
        return true;
    }
    tips.iter().any(|tree| match tree.get_path(path) {
        Ok(entry) => entry.kind() == Some(ObjectType::Commit) && missing(entry.id()),
        Err(_) => false,
    })
}

/// The trees the references named point at, each once.
fn moved_trees<'repo>(repo: &'repo Repository, moved: &[String]) -> Vec<git2::Tree<'repo>> {
    let mut seen = HashSet::new();
    let mut trees = Vec::new();
    for name in moved {
        if let Ok(reference) = repo.find_reference(name)
            && let Ok(commit) = reference.peel_to_commit()
            && let Ok(tree) = commit.tree()
            && seen.insert(tree.id())
        {
            trees.push(tree);
        }
    }
    trees
}

/// Fetch one remote by its configured refspecs. Returns the names of
/// the references it updated: new, moved, or (when pruning) gone.
fn fetch_remote(repo: &Repository, name: &str) -> Result<Vec<String>, git2::Error> {
    let mut remote = repo.find_remote(name)?;
    let updated = Rc::new(RefCell::new(Vec::new()));
    let mut callbacks = RemoteCallbacks::new();
    let mut credentials = Credentials::new(repo.config()?);
    callbacks.credentials(move |url, username, allowed| credentials.next(url, username, allowed));
    let noted = Rc::clone(&updated);
    callbacks.update_tips(move |name, _, _| {
        noted.borrow_mut().push(name.to_owned());
        true
    });
    let mut proxy = ProxyOptions::new();
    proxy.auto();
    let mut options = FetchOptions::new();
    options.remote_callbacks(callbacks).proxy_options(proxy);
    // No refspecs given: the remote's configured ones are used, as
    // `git fetch <remote>` uses them.
    remote.fetch(&[] as &[&str], Some(&mut options), None)?;
    drop(options);
    Ok(Rc::try_unwrap(updated)
        .map(RefCell::into_inner)
        .unwrap_or_default())
}

/// The credentials to try for a remote, each once. libgit2 asks again
/// after each it rejects, and would ask forever if given the same one
/// every time; so after the last there are none, and the fetch fails
/// with the remote's refusal.
struct Credentials {
    config: git2::Config,
    username_given: bool,
    agent_tried: bool,
    /// The SSH keys in `~/.ssh` not yet tried, unencrypted.
    keys: Vec<PathBuf>,
    helper_tried: bool,
    default_tried: bool,
}

/// The private keys ssh itself tries by default, in its order.
const DEFAULT_KEYS: [&str; 4] = ["id_ed25519", "id_ecdsa", "id_rsa", "id_dsa"];

impl Credentials {
    fn new(config: git2::Config) -> Credentials {
        let ssh_dir = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".ssh"));
        let keys = ssh_dir
            .map(|dir| {
                DEFAULT_KEYS
                    .iter()
                    .map(|name| dir.join(name))
                    .filter(|key| key.is_file())
                    .collect()
            })
            .unwrap_or_default();
        Credentials {
            config,
            username_given: false,
            agent_tried: false,
            keys,
            helper_tried: false,
            default_tried: false,
        }
    }

    fn next(
        &mut self,
        url: &str,
        username: Option<&str>,
        allowed: CredentialType,
    ) -> Result<Cred, git2::Error> {
        // An SSH URL without a user in it: libgit2 asks for one first,
        // and git's default for SSH remotes is `git`.
        if allowed.contains(CredentialType::USERNAME) && !self.username_given {
            self.username_given = true;
            return Cred::username(username.unwrap_or("git"));
        }
        if allowed.contains(CredentialType::SSH_KEY) {
            let user = username.unwrap_or("git");
            if !self.agent_tried {
                self.agent_tried = true;
                if let Ok(cred) = Cred::ssh_key_from_agent(user) {
                    return Ok(cred);
                }
            }
            while !self.keys.is_empty() {
                let key = self.keys.remove(0);
                if let Ok(cred) = Cred::ssh_key(user, None, &key, None) {
                    return Ok(cred);
                }
            }
        }
        if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) && !self.helper_tried {
            self.helper_tried = true;
            if let Ok(cred) = Cred::credential_helper(&self.config, url, username) {
                return Ok(cred);
            }
        }
        if allowed.contains(CredentialType::DEFAULT) && !self.default_tried {
            self.default_tried = true;
            if let Ok(cred) = Cred::default() {
                return Ok(cred);
            }
        }
        Err(git2::Error::from_str(
            "no credentials for the remote were accepted",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::history::tests::TestRepo;
    use std::time::{Duration, Instant};

    fn run(repo: &Repository) -> Result<FetchReport, String> {
        run_wanting(repo, Vec::new())
    }

    fn run_wanting(repo: &Repository, wanted: Vec<(String, Oid)>) -> Result<FetchReport, String> {
        let mut fetch = Fetch::start(repo.path(), wanted).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !fetch.is_done() && Instant::now() < deadline {
            fetch.poll();
            thread::sleep(Duration::from_millis(5));
        }
        assert!(fetch.is_done());
        fetch.outcome().unwrap()
    }

    #[test]
    fn a_fetch_brings_the_remote_branches_and_tags_up_to_date() {
        let mut upstream = TestRepo::new();
        let a = upstream.commit(&[("a.txt", "a\n")], "First", &[]);
        upstream.tag("v1", a);
        let main = upstream
            .repo
            .head()
            .unwrap()
            .shorthand()
            .unwrap()
            .to_owned();

        let mut local = TestRepo::new();
        local.commit(&[("l.txt", "l\n")], "Local", &[]);
        local
            .repo
            .remote("origin", upstream.path().to_str().unwrap())
            .unwrap();

        let report = run(&local.repo).unwrap();
        assert_eq!(report.fetched, vec!["origin".to_owned()]);
        assert!(report.failed.is_empty(), "{report:?}");
        // The branch and the tag.
        assert_eq!(report.updated, 2);
        assert_eq!(
            local
                .repo
                .refname_to_id(&format!("refs/remotes/origin/{main}"))
                .unwrap(),
            a
        );
        assert_eq!(local.repo.refname_to_id("refs/tags/v1").unwrap(), a);
        assert_eq!(report.summary(), "Fetched origin: 2 refs updated");

        // Nothing new: up to date. Then a commit upstream moves the
        // branch. Neither touches HEAD or the local branch.
        let report = run(&local.repo).unwrap();
        assert_eq!(report.updated, 0);
        assert_eq!(report.summary(), "Fetched origin: up to date");
        let head_before = local.repo.head().unwrap().target();
        let b = upstream.commit(&[("b.txt", "b\n")], "Second", &[a]);
        let report = run(&local.repo).unwrap();
        assert_eq!(report.summary(), "Fetched origin: 1 ref updated");
        assert_eq!(
            local
                .repo
                .refname_to_id(&format!("refs/remotes/origin/{main}"))
                .unwrap(),
            b
        );
        assert_eq!(local.repo.head().unwrap().target(), head_before);
    }

    #[test]
    fn a_remote_that_cannot_be_reached_is_reported_and_the_rest_fetched() {
        let mut upstream = TestRepo::new();
        upstream.commit(&[("a.txt", "a\n")], "First", &[]);
        let mut local = TestRepo::new();
        local.commit(&[("l.txt", "l\n")], "Local", &[]);
        let missing = tempfile::tempdir().unwrap();
        let gone = missing.path().join("gone");
        local.repo.remote("broken", gone.to_str().unwrap()).unwrap();
        local
            .repo
            .remote("origin", upstream.path().to_str().unwrap())
            .unwrap();

        let report = run(&local.repo).unwrap();
        assert_eq!(report.fetched, vec!["origin".to_owned()]);
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert_eq!(report.failed[0].0, "broken");
        let summary = report.summary();
        assert!(
            summary.starts_with("Fetched origin: 1 ref updated · Could not fetch broken: "),
            "{summary}"
        );
    }

    /// Commit a file in `repo` on top of its HEAD.
    fn commit_in(repo: &Repository, name: &str, content: &str, message: &str) -> Oid {
        std::fs::write(repo.workdir().unwrap().join(name), content).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(name)).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("Sub Author", "sub@example.com").unwrap();
        let head = repo.head().ok().and_then(|h| h.target());
        let parents: Vec<git2::Commit<'_>> = head
            .into_iter()
            .map(|id| repo.find_commit(id).unwrap())
            .collect();
        let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &refs)
            .unwrap()
    }

    /// Stage the submodule at `path` as it is and commit that.
    fn commit_gitlink(repo: &Repository, path: &str, message: &str) -> Oid {
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(path)).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("Test Author", "test@example.com").unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&head])
            .unwrap()
    }

    fn has_commit(repo: &Repository, id: Oid) -> bool {
        repo.find_commit(id).is_ok()
    }

    #[test]
    fn submodules_are_fetched_on_demand_when_a_commit_is_missing() {
        // Upstream: a repository with a submodule, whose URL is its
        // own directory so that a clone can fetch from it.
        let mut upstream = TestRepo::new();
        upstream.commit(&[("a.txt", "a\n")], "First", &[]);
        let sub_url = upstream.path().join("sub");
        let mut declared = upstream
            .repo
            .submodule(sub_url.to_str().unwrap(), Path::new("sub"), true)
            .unwrap();
        let up_sub = declared.open().unwrap();
        let s1 = commit_in(&up_sub, "s.txt", "one\n", "Sub one");
        declared.add_finalize().unwrap();
        commit_gitlink(&upstream.repo, "sub", "Add sub");

        // A clone, with the submodule cloned into it.
        let dir = tempfile::tempdir().unwrap();
        let local = Repository::clone(upstream.path().to_str().unwrap(), dir.path()).unwrap();
        let mut cloned = local.find_submodule("sub").unwrap();
        cloned.update(true, None).unwrap();
        let local_sub = Repository::open(dir.path().join("sub")).unwrap();
        assert!(has_commit(&local_sub, s1));

        // Upstream moves the submodule on; a fetch brings the parent's
        // branch, sees the commit it points at is missing, and fetches
        // the submodule too.
        let s2 = commit_in(&up_sub, "s.txt", "two\n", "Sub two");
        commit_gitlink(&upstream.repo, "sub", "Bump sub");
        let report = run(&local).unwrap();
        assert_eq!(report.fetched, vec!["origin".to_owned()]);
        assert_eq!(report.submodules.len(), 1, "{report:?}");
        let (path, sub_report) = &report.submodules[0];
        assert_eq!(path, "sub");
        assert_eq!(sub_report.fetched, vec!["origin".to_owned()]);
        assert!(sub_report.failed.is_empty(), "{report:?}");
        assert!(has_commit(&local_sub, s2));
        assert!(
            report
                .summary()
                .starts_with("Fetched origin: 1 ref updated · sub: Fetched origin: "),
            "{}",
            report.summary()
        );

        // Nothing missing: the submodule is left alone.
        let report = run(&local).unwrap();
        assert!(report.submodules.is_empty(), "{report:?}");
        assert_eq!(report.summary(), "Fetched origin: up to date");

        // With recursion off, a missing commit stays missing.
        local
            .config()
            .unwrap()
            .set_str("fetch.recurseSubmodules", "false")
            .unwrap();
        let s3 = commit_in(&up_sub, "s.txt", "three\n", "Sub three");
        commit_gitlink(&upstream.repo, "sub", "Bump sub again");
        let report = run(&local).unwrap();
        assert_eq!(report.updated, 1);
        assert!(report.submodules.is_empty(), "{report:?}");
        assert!(!has_commit(&local_sub, s3));

        // Back on demand, a tip that already came in doesn't count,
        // however much is missing at it: a commit the submodule's
        // remote hasn't got would otherwise be fetched for every time.
        local
            .config()
            .unwrap()
            .set_str("fetch.recurseSubmodules", "on-demand")
            .unwrap();
        let report = run(&local).unwrap();
        assert!(report.submodules.is_empty(), "{report:?}");
        assert!(!has_commit(&local_sub, s3));
        // But a commit the caller wants is enough, even one nothing
        // points at.
        let s4 = commit_in(&up_sub, "s.txt", "four\n", "Sub four");
        let report = run_wanting(&local, vec![("sub".to_owned(), s4)]).unwrap();
        assert_eq!(report.submodules.len(), 1, "{report:?}");
        assert!(has_commit(&local_sub, s3));
        assert!(has_commit(&local_sub, s4));

        // Always: fetched whether or not anything is missing.
        local
            .config()
            .unwrap()
            .set_str("fetch.recurseSubmodules", "true")
            .unwrap();
        let report = run(&local).unwrap();
        assert_eq!(report.submodules.len(), 1, "{report:?}");
        assert_eq!(
            report.summary(),
            "Fetched origin: up to date · sub: Fetched origin: up to date"
        );
    }

    #[test]
    fn a_repository_without_remotes_says_so() {
        let mut local = TestRepo::new();
        local.commit(&[("l.txt", "l\n")], "Local", &[]);
        let report = run(&local.repo).unwrap();
        assert_eq!(report, FetchReport::default());
        assert_eq!(report.summary(), "No remotes to fetch from");
    }
}
