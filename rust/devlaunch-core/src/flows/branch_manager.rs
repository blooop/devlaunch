//! Making sure a branch exists, locally and — when asked — on the remote.
//!
//! Ported from `devlaunch/worktree/branch_manager.py`. Four git verbs and one
//! decision procedure over them: what to do about a branch given whether it is
//! there locally and whether it is there upstream.
//!
//! The one production caller is
//! [`crate::flows::workspace_clone::WorkspaceCloneManager::ensure_branch`], which
//! runs against the *bare cache* and asks for
//! [`RemoteRefs::InferFromLocal`] + [`CreateRemote::Never`] — no `ls-remote`, no
//! push. The other combinations are the general contract this module has always
//! offered, and they are kept: `dl` pushes no branch today, and the arm that does
//! is one flag away rather than one rewrite away.
//!
//! Nothing here prints. Python logged five lines; the two that carried a decision
//! are typed answers ([`Tracking`], [`BranchError`]) and the rest said what the
//! caller had just asked for.

// The callers are the storage flows in this wave and the launch path in M7.
#![allow(dead_code)] // consumed from M6/M7

use std::path::Path;

use crate::clients::git::{self, Git};

/// How the remote side of a branch is established.
///
/// A named pair rather than Python's `use_local_refs: bool`, because at a call
/// site `True` says nothing about what is being inferred from what.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteRefs {
    /// Ask the remote with `git ls-remote`.
    Ask,
    /// Infer remote existence from the local refs.
    ///
    /// Safe — and correct — in a bare cache whose refspec maps remote heads onto
    /// local heads, which is what `+refs/heads/*:refs/heads/*` does: a head that
    /// is there locally got there from the remote. It is also what keeps the
    /// launch path's network use to the one targeted fetch, since an `ls-remote`
    /// here would be a second round trip under the repo lock.
    InferFromLocal,
}

/// What to do when the remote has not got the branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreateRemote<'key> {
    /// Push it, optionally with a named ssh key.
    Push { ssh_key: Option<&'key Path> },
    /// Leave the remote alone. What the launch path asks for: a branch exists on
    /// the remote when somebody pushes it, not when somebody launches it.
    Never,
}

/// What ensuring a branch is being asked to do.
///
/// A struct because Python's five defaulted keyword arguments do not survive
/// translation as five positional ones — and because two of them
/// ([`RemoteRefs`], [`CreateRemote`]) are the decision the caller is really
/// making.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EnsureBranch<'a> {
    /// The repository the branch lives in — the bare cache, on the launch path.
    pub(crate) repo: &'a Path,
    pub(crate) branch: &'a str,
    /// The remote's name, `origin` everywhere in devlaunch.
    pub(crate) remote: &'a str,
    /// What a *new* branch is cut from. `HEAD` is the last resort, and the caller
    /// decides it: the launch path passes the default branch when it has one.
    pub(crate) start_point: &'a str,
    pub(crate) remote_refs: RemoteRefs,
    pub(crate) create_remote: CreateRemote<'a>,
}

impl<'a> EnsureBranch<'a> {
    /// The launch path's request: a bare cache, local refs, no push.
    pub(crate) fn in_cache(repo: &'a Path, branch: &'a str, start_point: &'a str) -> Self {
        Self {
            repo,
            branch,
            remote: "origin",
            start_point,
            remote_refs: RemoteRefs::InferFromLocal,
            create_remote: CreateRemote::Never,
        }
    }
}

/// Whether a branch's upstream could be set.
///
/// An answer rather than an error, because the failure is *expected* on the arm
/// that matters: setting the upstream of a branch the remote has not got yet is
/// the last step of creating a local branch, and git refuses it. The reason is
/// carried for a caller that wants to say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Tracking {
    Set,
    NotSet { reason: String },
}

/// Why a branch could not be ensured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BranchError {
    /// `git branch` refused for a reason that is not "it is already there".
    NotCreated { branch: String, reason: String },
    /// `git push` refused.
    NotPushed { branch: String, reason: String },
}

/// The four git verbs a branch needs, and the decision over them.
#[derive(Clone, Copy)]
pub(crate) struct BranchManager<'r> {
    git: Git<'r>,
}

impl<'r> BranchManager<'r> {
    pub(crate) fn new(git: Git<'r>) -> Self {
        Self { git }
    }

    /// Ensure the branch exists locally, and optionally on the remote.
    ///
    /// Four states, and each gets exactly what it needs:
    ///
    /// - **local and remote**: nothing to do.
    /// - **remote only**: create the local branch tracking `<remote>/<branch>`,
    ///   then set its upstream.
    /// - **local only**: push it if asked, then set its upstream.
    /// - **neither**: create it from `start_point`, push it if asked, then set its
    ///   upstream.
    ///
    /// The upstream is set at the end of every arm that changed anything, and its
    /// failure is not this call's failure — see [`Tracking`].
    pub(crate) fn ensure_branch_exists(
        &self,
        request: EnsureBranch<'_>,
    ) -> Result<Tracking, BranchError> {
        let EnsureBranch {
            repo,
            branch,
            remote,
            start_point,
            remote_refs,
            create_remote,
        } = request;

        let local_exists = self.local_branch_exists(repo, branch);
        let remote_exists = match remote_refs {
            RemoteRefs::InferFromLocal => local_exists,
            RemoteRefs::Ask => self.remote_branch_exists(repo, branch, remote),
        };

        if local_exists && remote_exists {
            return Ok(Tracking::Set);
        }

        if !local_exists && remote_exists {
            // Track the remote branch that is already there, so the new local
            // branch starts where the remote is rather than at `start_point`.
            self.create_local_branch(repo, branch, &format!("{remote}/{branch}"))?;
            return Ok(self.track_remote_branch(repo, branch, remote));
        }

        if !local_exists {
            self.create_local_branch(repo, branch, start_point)?;
        }

        if !remote_exists && let CreateRemote::Push { ssh_key } = create_remote {
            self.push_branch_to_remote(repo, branch, remote, ssh_key)?;
        }

        Ok(self.track_remote_branch(repo, branch, remote))
    }

    /// Create a local branch at `start_point`.
    ///
    /// A branch that is already there is not a failure, and is told apart from a
    /// real one by reading git's stderr — which is why the client pins the C
    /// locale on this verb. A failure git said nothing about falls through to the
    /// error, carrying the exit status: "Failed to create branch: " with nothing
    /// after the colon tells the reader only that something went wrong, which the
    /// error already said.
    pub(crate) fn create_local_branch(
        &self,
        repo: &Path,
        branch: &str,
        start_point: &str,
    ) -> Result<(), BranchError> {
        match self.git.create_branch(repo, branch, start_point).refusal() {
            None => Ok(()),
            Some(refused) if refused.reason().contains("already exists") => Ok(()),
            Some(refused) => Err(BranchError::NotCreated {
                branch: branch.to_owned(),
                reason: refused.reason().to_owned(),
            }),
        }
    }

    /// Point the branch's upstream at `<remote>/<branch>`.
    pub(crate) fn track_remote_branch(&self, repo: &Path, branch: &str, remote: &str) -> Tracking {
        match self.git.set_upstream(repo, branch, remote).refusal() {
            None => Tracking::Set,
            Some(refused) => Tracking::NotSet {
                reason: refused.reason().to_owned(),
            },
        }
    }

    /// Whether the branch is there locally.
    ///
    /// `show-ref --verify` exits non-zero both for a ref that is absent and for a
    /// directory that is not a repository, and this collapses the two — as Python
    /// did. The collapse is sound for its callers: a branch in a directory that is
    /// not a repository is not a branch anybody can check out, and the step after
    /// this one is the one that reports what git says about the repository.
    pub(crate) fn local_branch_exists(&self, repo: &Path, branch: &str) -> bool {
        self.git
            .verify_ref(repo, &git::refs_heads(branch))
            .is_said()
    }

    /// Whether the remote has the branch.
    ///
    /// A refusal is `false`, for the same reason as above: the question is being
    /// asked to decide whether to create the branch, and a remote that cannot be
    /// asked has not got it as far as this decision goes.
    pub(crate) fn remote_branch_exists(&self, repo: &Path, branch: &str, remote: &str) -> bool {
        self.git
            .ls_remote_heads(repo, remote, Some(branch))
            .said()
            .is_some_and(|branches| !branches.is_empty())
    }

    /// The branch names the remote has.
    ///
    /// An empty list for a refusal, which is what Python answered after logging
    /// the reason. Nothing in devlaunch calls this today; it is the readable half
    /// of the `ls-remote` pair and the completion flows (M5) may want it.
    pub(crate) fn get_remote_branches(&self, repo: &Path, remote: &str) -> Vec<String> {
        self.git
            .ls_remote_heads(repo, remote, None)
            .said()
            .unwrap_or_default()
    }

    /// Push a branch to the remote, setting its upstream as it goes.
    ///
    /// `ssh_key` names a key to use instead of whatever the agent offers; the
    /// client is where it is quoted into `GIT_SSH_COMMAND` and layered onto the
    /// inherited environment, and why.
    pub(crate) fn push_branch_to_remote(
        &self,
        repo: &Path,
        branch: &str,
        remote: &str,
        ssh_key: Option<&Path>,
    ) -> Result<(), BranchError> {
        match self
            .git
            .push_branch(repo, remote, branch, ssh_key)
            .refusal()
        {
            None => Ok(()),
            Some(refused) => Err(BranchError::NotPushed {
                branch: branch.to_owned(),
                reason: refused.reason().to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    //! `test/test_worktree_branch_manager.py`, re-pinned at the argv seam.
    //!
    //! Python's `TestEnsureBranchExists` classes patch the four verbs on the class
    //! and assert which of them were called with what. There is nothing to patch
    //! here — the verbs are calls on the git client — so the same decision table is
    //! pinned one layer down, as the argv sequence each state produces. That is
    //! strictly more than Python asserted: it pins the argv as well as the choice.

    use std::path::PathBuf;

    use super::*;
    use crate::clients::git::Git;
    use crate::flows::repo_manager::tests::{FakeGit, as_strs};
    use devlaunch_test_support::Response;

    /// A repository to be about. Never touched: every verb here is argv.
    fn a_repo() -> PathBuf {
        PathBuf::from("/cache/repos/owner/repo/.bare")
    }

    /// A branch manager over `fake`.
    fn manager<'r>(fake: &'r FakeGit) -> BranchManager<'r> {
        BranchManager::new(Git::new(fake))
    }

    /// The C locale the two classified verbs are addressed in.
    fn c_locale(call: &devlaunch_test_support::Call) -> Vec<(&str, &str)> {
        call.invocation()
            .env
            .entries
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect()
    }

    // ----------------------------------------------------------- the probes

    #[test]
    fn a_local_branch_is_asked_after_by_its_exact_ref() {
        let fake = FakeGit::new();

        assert!(manager(&fake).local_branch_exists(&a_repo(), "main"));

        let call = fake.only_call();
        assert_eq!(
            as_strs(&[call.argv()])[0],
            ["git", "show-ref", "--verify", "refs/heads/main"]
        );
        assert_eq!(call.invocation().cwd.as_deref(), Some(a_repo().as_path()));
    }

    #[test]
    fn a_local_branch_that_show_ref_refused_is_not_there() {
        // `show-ref --verify` exits non-zero both for a ref that is absent and for a
        // directory that is not a repository, and both are collapsed: a branch in a
        // directory that is not a repository is not a branch anybody can check out.
        for refusal in [Response::exited(1), Response::ProgramNotFound] {
            let fake = FakeGit::new().with_script(["git", "show-ref"], refusal);

            assert!(!manager(&fake).local_branch_exists(&a_repo(), "nonexistent"));
        }
    }

    #[test]
    fn a_remote_branch_is_asked_after_by_name_and_read_from_the_answer() {
        let fake = FakeGit::new().with_script(
            ["git", "ls-remote"],
            Response::stdout("abc123\trefs/heads/main\n"),
        );

        assert!(manager(&fake).remote_branch_exists(&a_repo(), "main", "origin"));

        assert_eq!(
            as_strs(&fake.argvs())[0],
            ["git", "ls-remote", "--heads", "origin", "main"]
        );
    }

    #[test]
    fn a_remote_that_answered_nothing_or_refused_has_not_got_the_branch() {
        for answer in [Response::stdout(""), Response::exited(1)] {
            let fake = FakeGit::new().with_script(["git", "ls-remote"], answer);

            assert!(!manager(&fake).remote_branch_exists(&a_repo(), "nosuch", "origin"));
        }
    }

    #[test]
    fn the_remotes_branches_are_the_names_in_its_listing() {
        let fake = FakeGit::new().with_script(
            ["git", "ls-remote"],
            Response::stdout("abc123\trefs/heads/main\ndef456\trefs/heads/develop\n"),
        );

        assert_eq!(
            manager(&fake).get_remote_branches(&a_repo(), "origin"),
            ["main", "develop"]
        );
        assert_eq!(
            as_strs(&fake.argvs())[0],
            ["git", "ls-remote", "--heads", "origin"],
            "no branch argument: this asks for all of them"
        );
    }

    #[test]
    fn a_remote_that_could_not_be_listed_is_an_empty_list() {
        for answer in [Response::stdout(""), Response::exited(1)] {
            let fake = FakeGit::new().with_script(["git", "ls-remote"], answer);

            assert_eq!(
                manager(&fake).get_remote_branches(&a_repo(), "origin"),
                Vec::<String>::new()
            );
        }
    }

    // ------------------------------------------------------------ creating

    #[test]
    fn creating_a_branch_names_its_start_point_and_pins_gits_locale() {
        let fake = FakeGit::new();

        manager(&fake)
            .create_local_branch(&a_repo(), "new-branch", "origin/main")
            .expect("created");

        let call = fake.only_call();
        assert_eq!(
            as_strs(&[call.argv()])[0],
            ["git", "branch", "new-branch", "origin/main"]
        );
        // The already-exists arm is classified from git's stderr text, which git
        // translates — so on a German host an ordinary re-launch of a branch that is
        // already there would raise instead of being swallowed. `LANGUAGE` is pinned
        // too: under gettext it outranks a non-C `LC_ALL`.
        assert_eq!(c_locale(&call), [("LANGUAGE", "C"), ("LC_ALL", "C")]);
        assert_eq!(
            call.invocation().env.base,
            crate::runner::EnvBase::Parent,
            "the rest of the environment must survive"
        );
    }

    #[test]
    fn a_branch_that_is_already_there_is_not_a_failure() {
        let fake = FakeGit::new().with_script(
            ["git", "branch"],
            Response::failed(128, "fatal: a branch named 'x' already exists\n"),
        );

        manager(&fake)
            .create_local_branch(&a_repo(), "x", "HEAD")
            .expect("an existing branch is what a re-launch finds");
    }

    #[test]
    fn a_branch_git_refused_for_any_other_reason_is_a_failure_with_its_words() {
        let fake =
            FakeGit::new().with_script(["git", "branch"], Response::failed(128, "fatal: boom\n"));

        let failed = manager(&fake)
            .create_local_branch(&a_repo(), "x", "HEAD")
            .expect_err("refused");

        assert_eq!(
            failed,
            BranchError::NotCreated {
                branch: "x".to_owned(),
                // Trimmed: git's messages end in a newline, and this ends up
                // interpolated mid-sentence.
                reason: "fatal: boom".to_owned(),
            }
        );
    }

    #[test]
    fn a_branch_failure_git_said_nothing_about_carries_its_exit_status() {
        // An unguarded membership test on an absent stderr turned that failure into
        // a type error naming neither the branch nor the cause; quoting it raw read
        // "Failed to create branch: " and stopped.
        let fake = FakeGit::new().with_script(["git", "branch"], Response::exited(128));

        let failed = manager(&fake)
            .create_local_branch(&a_repo(), "x", "HEAD")
            .expect_err("refused");

        assert_eq!(
            failed,
            BranchError::NotCreated {
                branch: "x".to_owned(),
                reason: "git branch exited 128".to_owned(),
            }
        );
    }

    // ------------------------------------------------------------ tracking

    #[test]
    fn tracking_points_the_branch_at_its_remote_counterpart() {
        for (remote, expected) in [
            ("origin", "--set-upstream-to=origin/main"),
            ("upstream", "--set-upstream-to=upstream/main"),
        ] {
            let fake = FakeGit::new();

            assert_eq!(
                manager(&fake).track_remote_branch(&a_repo(), "main", remote),
                Tracking::Set
            );

            assert_eq!(
                as_strs(&fake.argvs())[0],
                ["git", "branch", expected, "main"]
            );
        }
    }

    #[test]
    fn tracking_that_git_refused_is_an_answer_rather_than_a_failure() {
        // Expected on the arm that matters: setting the upstream of a branch the
        // remote has not got yet is the last step of creating a local branch, and
        // git refuses it.
        let fake = FakeGit::new().with_script(
            ["git", "branch"],
            Response::failed(128, "fatal: the requested upstream branch does not exist\n"),
        );

        assert_eq!(
            manager(&fake).track_remote_branch(&a_repo(), "main", "origin"),
            Tracking::NotSet {
                reason: "fatal: the requested upstream branch does not exist".to_owned()
            }
        );
    }

    // ------------------------------------------------------------- pushing

    #[test]
    fn pushing_sets_the_upstream_as_it_goes() {
        let fake = FakeGit::new();

        manager(&fake)
            .push_branch_to_remote(&a_repo(), "new-branch", "origin", None)
            .expect("pushed");

        let call = fake.only_call();
        assert_eq!(
            as_strs(&[call.argv()])[0],
            ["git", "push", "-u", "origin", "new-branch"]
        );
        assert!(
            call.invocation().env.entries.is_empty(),
            "no key named, so nothing is added to the environment"
        );
    }

    #[test]
    fn a_named_key_reaches_ssh_whole_however_it_is_spelled() {
        // `GIT_SSH_COMMAND` is a shell string, not argv, so an unquoted path
        // containing a space is split by the shell — ssh is handed a truncated `-i`
        // and the rest of the path as a hostname, and the push fails on the one
        // setup naming a key was supposed to guarantee. The second path carries the
        // shell's other weapons, because the guarantee is "one argument, whatever is
        // in it".
        for key in ["/tmp/dl keys/id ed25519", "/tmp/dl$(rm x); it's/id"] {
            let fake = FakeGit::new();

            manager(&fake)
                .push_branch_to_remote(&a_repo(), "b", "origin", Some(Path::new(key)))
                .expect("pushed");

            let call = fake.only_call();
            let command = call
                .invocation()
                .env
                .entries
                .get("GIT_SSH_COMMAND")
                .expect("a key reaches git through the environment");
            assert_eq!(
                shlex::split(command).expect("a shell string"),
                ["ssh", "-i", key, "-o", "IdentitiesOnly=yes"]
            );
            // Layered on the inherited environment, not substituted for it: a push
            // with no PATH cannot find the ssh it was just told to run, and one with
            // no HOME or SSH_AUTH_SOCK cannot read known_hosts or reach the agent.
            assert_eq!(call.invocation().env.base, crate::runner::EnvBase::Parent);
        }
    }

    #[test]
    fn a_push_git_refused_carries_its_words_or_its_exit_status() {
        for (answer, reason) in [
            (Response::failed(1, "Push failed\n"), "Push failed"),
            (Response::exited(128), "git push exited 128"),
        ] {
            let fake = FakeGit::new().with_script(["git", "push"], answer);

            let failed = manager(&fake)
                .push_branch_to_remote(&a_repo(), "b", "origin", None)
                .expect_err("refused");

            assert_eq!(
                failed,
                BranchError::NotPushed {
                    branch: "b".to_owned(),
                    reason: reason.to_owned(),
                }
            );
        }
    }

    // ------------------------------------------------- the decision table

    /// `ensure_branch_exists` with the remote asked and pushes allowed — Python's
    /// defaults.
    fn asking<'a>(repo: &'a Path, branch: &'a str, start_point: &'a str) -> EnsureBranch<'a> {
        EnsureBranch {
            repo,
            branch,
            remote: "origin",
            start_point,
            remote_refs: RemoteRefs::Ask,
            create_remote: CreateRemote::Push { ssh_key: None },
        }
    }

    #[test]
    fn a_branch_that_is_there_at_both_ends_is_left_alone() {
        let repo = a_repo();
        let fake = FakeGit::new().with_script(
            ["git", "ls-remote"],
            Response::stdout("abc\trefs/heads/main\n"),
        );

        let tracked = manager(&fake)
            .ensure_branch_exists(asking(&repo, "main", "HEAD"))
            .expect("nothing to do");

        assert_eq!(tracked, Tracking::Set);
        assert_eq!(
            as_strs(&fake.argvs()),
            [
                vec!["git", "show-ref", "--verify", "refs/heads/main"],
                vec!["git", "ls-remote", "--heads", "origin", "main"],
            ],
            "the two probes and nothing else"
        );
    }

    #[test]
    fn a_branch_only_the_remote_has_is_created_tracking_it() {
        let repo = a_repo();
        let fake = FakeGit::new()
            .with_script(["git", "show-ref"], Response::exited(1))
            .with_script(
                ["git", "ls-remote"],
                Response::stdout("abc\trefs/heads/main\n"),
            );

        manager(&fake)
            .ensure_branch_exists(asking(&repo, "main", "HEAD"))
            .expect("created");

        assert_eq!(
            as_strs(&fake.argvs()),
            [
                vec!["git", "show-ref", "--verify", "refs/heads/main"],
                vec!["git", "ls-remote", "--heads", "origin", "main"],
                // From `origin/main`, not from the start point: the branch is
                // already somewhere, and that somewhere is where it starts.
                vec!["git", "branch", "main", "origin/main"],
                vec!["git", "branch", "--set-upstream-to=origin/main", "main"],
            ]
        );
    }

    #[test]
    fn a_branch_that_exists_nowhere_is_created_from_the_start_point_and_pushed() {
        let repo = a_repo();
        let fake = FakeGit::new()
            .with_script(["git", "show-ref"], Response::exited(1))
            .with_script(["git", "ls-remote"], Response::stdout(""));

        manager(&fake)
            .ensure_branch_exists(asking(&repo, "new-branch", "origin/main"))
            .expect("created");

        assert_eq!(
            as_strs(&fake.argvs()),
            [
                vec!["git", "show-ref", "--verify", "refs/heads/new-branch"],
                vec!["git", "ls-remote", "--heads", "origin", "new-branch"],
                vec!["git", "branch", "new-branch", "origin/main"],
                vec!["git", "push", "-u", "origin", "new-branch"],
                vec![
                    "git",
                    "branch",
                    "--set-upstream-to=origin/new-branch",
                    "new-branch"
                ],
            ]
        );
    }

    #[test]
    fn a_caller_that_will_not_create_the_remote_branch_pushes_nothing() {
        let repo = a_repo();
        let fake = FakeGit::new()
            .with_script(["git", "show-ref"], Response::exited(1))
            .with_script(["git", "ls-remote"], Response::stdout(""));

        manager(&fake)
            .ensure_branch_exists(EnsureBranch {
                create_remote: CreateRemote::Never,
                ..asking(&repo, "new-branch", "HEAD")
            })
            .expect("created");

        let argvs = fake.argvs();
        let issued = as_strs(&argvs);
        assert!(issued.contains(&vec!["git", "branch", "new-branch", "HEAD"]));
        assert!(
            !issued.iter().any(|argv| argv.contains(&"push")),
            "a branch exists on the remote when somebody pushes it, not when \
             somebody launches it: {issued:?}"
        );
    }

    #[test]
    fn inferring_from_local_refs_asks_no_remote_at_all() {
        // What the launch path asks for, and the reason it does: an `ls-remote` here
        // would be a second network round trip under the repo lock, and in a bare
        // cache whose refspec maps remote heads onto local heads a head that is
        // there locally got there from the remote.
        let repo = a_repo();

        let present = FakeGit::new();
        manager(&present)
            .ensure_branch_exists(EnsureBranch::in_cache(&repo, "main", "main"))
            .expect("nothing to do");
        assert_eq!(
            as_strs(&present.argvs()),
            [vec!["git", "show-ref", "--verify", "refs/heads/main"]],
            "one probe, and no branch created for a branch that is there"
        );

        let absent = FakeGit::new().with_script(["git", "show-ref"], Response::exited(1));
        manager(&absent)
            .ensure_branch_exists(EnsureBranch::in_cache(&repo, "newbranch", "main"))
            .expect("created");
        let argvs = absent.argvs();
        let issued = as_strs(&argvs);
        assert!(
            !issued.iter().any(|argv| argv.contains(&"ls-remote")),
            "{issued:?}"
        );
        assert_eq!(
            issued[1],
            ["git", "branch", "newbranch", "main"],
            "cut from the start point the caller named"
        );
        assert!(
            !issued.iter().any(|argv| argv.contains(&"push")),
            "and never pushed: {issued:?}"
        );
    }

    #[test]
    fn a_creation_that_failed_stops_the_sequence_before_the_push() {
        let repo = a_repo();
        let fake = FakeGit::new()
            .with_script(["git", "show-ref"], Response::exited(1))
            .with_script(["git", "ls-remote"], Response::stdout(""))
            .with_script(["git", "branch"], Response::failed(128, "fatal: boom\n"));

        let failed = manager(&fake)
            .ensure_branch_exists(asking(&repo, "new-branch", "HEAD"))
            .expect_err("refused");

        assert!(matches!(failed, BranchError::NotCreated { .. }));
        let argvs = fake.argvs();
        assert!(
            !as_strs(&argvs).iter().any(|argv| argv.contains(&"push")),
            "a branch that was not created has nothing to push"
        );
    }
}
