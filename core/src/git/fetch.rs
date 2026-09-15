//! Fetching from a repository's remotes: what `git fetch --all` does,
//! and no more. Every remote is asked for its branches and tags, by
//! its configured refspecs, and the remote-tracking references
//! (`refs/remotes/<remote>/*`) and tags are brought up to date; whether
//! tags come along and whether gone branches are pruned follow the
//! repository's configuration, as they do for git. Nothing is merged,
//! and neither the working tree nor HEAD nor any local branch is
//! touched.
//!
//! A fetch runs in the background (see [`Fetch`]) and can't ask for
//! anything, so it uses whatever credentials are at hand: for SSH the
//! agent, then the usual keys in `~/.ssh`, unencrypted; for HTTPS the
//! credential helpers git is configured with (the macOS keychain, say)
//! and the system's default mechanisms (Kerberos, NTLM). A remote that
//! can't be reached, or won't let us in, is reported and the others
//! are fetched all the same.

use git2::{Cred, CredentialType, FetchOptions, ProxyOptions, RemoteCallbacks, Repository};
use std::cell::Cell;
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
        if parts.is_empty() {
            "No remotes to fetch from".to_owned()
        } else {
            parts.join(" · ")
        }
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
    /// directory is `git_dir` (see [`Repository::path`]).
    pub fn start(git_dir: &Path) -> Result<Fetch, git2::Error> {
        let (sender, receiver) = mpsc::channel();
        let git_dir = git_dir.to_path_buf();
        thread::Builder::new()
            .name("git-fetch".to_owned())
            .spawn(move || {
                let outcome = fetch_all(&git_dir, |remote| {
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

/// Fetch every remote of the repository at `git_dir`, telling `began`
/// the name of each as it starts. Fails only when the repository can't
/// be opened or its remotes listed; a remote that fails is in the
/// report.
fn fetch_all(git_dir: &Path, mut began: impl FnMut(&str)) -> Result<FetchReport, git2::Error> {
    let repo = Repository::open(git_dir)?;
    let names = repo.remotes()?;
    let mut report = FetchReport::default();
    for name in names.iter() {
        // A remote whose name isn't UTF-8 can't be shown; skip it.
        let Ok(Some(name)) = name else { continue };
        began(name);
        match fetch_remote(&repo, name) {
            Ok(updated) => {
                report.fetched.push(name.to_owned());
                report.updated += updated;
            }
            Err(err) => report
                .failed
                .push((name.to_owned(), err.message().trim().to_owned())),
        }
    }
    Ok(report)
}

/// Fetch one remote by its configured refspecs. Returns how many
/// references it updated.
fn fetch_remote(repo: &Repository, name: &str) -> Result<usize, git2::Error> {
    let mut remote = repo.find_remote(name)?;
    let updated = Rc::new(Cell::new(0));
    let mut callbacks = RemoteCallbacks::new();
    let mut credentials = Credentials::new(repo.config()?);
    callbacks.credentials(move |url, username, allowed| credentials.next(url, username, allowed));
    let counted = Rc::clone(&updated);
    callbacks.update_tips(move |_, _, _| {
        counted.set(counted.get() + 1);
        true
    });
    let mut proxy = ProxyOptions::new();
    proxy.auto();
    let mut options = FetchOptions::new();
    options.remote_callbacks(callbacks).proxy_options(proxy);
    // No refspecs given: the remote's configured ones are used, as
    // `git fetch <remote>` uses them.
    remote.fetch(&[] as &[&str], Some(&mut options), None)?;
    Ok(updated.get())
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
        let mut fetch = Fetch::start(repo.path()).unwrap();
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

    #[test]
    fn a_repository_without_remotes_says_so() {
        let mut local = TestRepo::new();
        local.commit(&[("l.txt", "l\n")], "Local", &[]);
        let report = run(&local.repo).unwrap();
        assert_eq!(report, FetchReport::default());
        assert_eq!(report.summary(), "No remotes to fetch from");
    }
}
