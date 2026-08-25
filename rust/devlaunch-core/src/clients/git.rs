//! Every `git` devlaunch runs, in one place.
//!
//! Python spread these across five modules — `worktree/repo_manager.py`,
//! `worktree/branch_manager.py`, `worktree/workspace_clone.py`,
//! `workspace_state.py` and `dl.py` — and devlaunch#54 is the observation that
//! they had drifted: the same verb built twice, one site trimming output that
//! its twin parsed raw, and one family of calls (workspace_state's) pinning the
//! repository while every other site left git's discovery switched on. Merging
//! them is structural rather than tidy: there is now one answer type, one place
//! that shapes a failed verb into something a caller can act on, and one place
//! where an argv is written down.
//!
//! # What this layer decides, and what it leaves alone
//!
//! It decides argv, cwd, environment, per-call timeout, and how a spawn outcome
//! becomes a [`GitAnswer`]. It decides nothing about *sequence*: clone-if-missing,
//! the fetch-then-create-branch dance, the recovery of a half-removed cache and
//! the LFS two-phase materialization are flows (M4b), and what they get from here
//! is verbs. A verb here never falls back to another verb, never retries, and
//! never logs.
//!
//! # Two families, and the difference is load-bearing
//!
//! - **[`Git::head_branch`], [`Git::status_porcelain`], [`Git::unpushed_commits`]**
//!   name their repository with `--git-dir` *and* `--work-tree`, so git's
//!   discovery is off and an unusable `.git` is a refusal rather than a confident
//!   answer about an ancestor repository. That is devlaunch#171, and
//!   [`Git::about`] carries the whole of the reasoning.
//! - **Everything else** operates on a repository this process just created or
//!   just cloned, and selects it with `cwd` exactly as Python did. Their argv is
//!   preserved to the letter — parity is judged on argv through the shim log — so
//!   no `--git-dir` was added to them here. The distinction is that the pinned
//!   family answers questions *about* a directory a caller found on disk, where
//!   these act on a directory this process is in the middle of building.
//!
//! # Timing spans are deliberately absent
//!
//! Python wraps eight of these calls in a `timing.span` (`git clone --bare`,
//! `git fetch`, `git clone`, `git ls-files`, `git lfs ls-files`,
//! `git lfs fetch (cache)`, `git lfs pull (cache)`, `git lfs pull`,
//! `git ls-remote`). A span names a round trip, which is this layer's idea rather
//! than the runner's — but `timing`'s registry lands in the same wave as this
//! module, so the spans are wired in M4b/M5 against the real registry rather than
//! guessed at here. The names above are the list to wire.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::runner::{
    CapturedText, EnvSpec, Exit, Invocation, OsFailure, Outcome, Runner, SpawnSpec,
};

/// The program, everywhere. `git lfs` is this program with a subcommand, not a
/// second program: `git-lfs` on PATH is what makes the subcommand work, which is
/// why [`lfs_is_installed`] asks about that name and nothing here spawns it.
pub(crate) const PROGRAM: &str = "git";

/// `refs/heads/` — `branch_manager.py`'s `REFS_HEADS_PREFIX`.
pub(crate) const REFS_HEADS: &str = "refs/heads/";

/// How long a question *about one repository* may take (`workspace_state._git`).
///
/// A bound rather than none because these run inside `dl --ls --json`, once per
/// workspace: one clone whose filesystem has gone away must cost one pass, not
/// the listing.
const ABOUT_ONE_REPO: Duration = Duration::from_secs(30);

/// `repo_manager.get_default_branch`'s bound on asking a remote for its HEAD.
const ASK_REMOTE_FOR_HEAD: Duration = Duration::from_secs(10);

/// `dl.py`'s bound on every `ls-remote` it issues for completion data.
const ASK_REMOTE_FOR_REFS: Duration = Duration::from_secs(5);

/// `dl.py`'s bound on reading local refs out of the bare cache.
const READ_LOCAL_REFS: Duration = Duration::from_secs(2);

/// Every git-lfs pointer file starts with this; see the git-lfs pointer spec.
const LFS_POINTER_PREFIX: &[u8] = b"version https://git-lfs";

/// What git answered, or that it did not.
///
/// `Said` carries the output shaped the way the verb that asked for it needs —
/// a string for a verb whose stdout *is* the answer, a parsed list for a verb
/// whose stdout is a format, `()` for a verb that was never captured. `T`
/// defaults to `String` because most verbs are asked a question.
///
/// This is Python's `GitSaid`/`GitRefused` pair, one layer down from
/// `workspace_state.py` where it was written, and it exists for the reason that
/// module gives at length: `""` and `None` are both falsey, so an
/// `Optional[str]` let a *refused* `git status` read as a clean tree. An empty
/// `Said` is an answer; a `Refused` is not an empty answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GitAnswer<T = String> {
    /// git ran and exited 0. The payload may be empty; empty is an answer.
    Said(T),
    /// git could not answer.
    Refused(GitRefused),
}

impl<T> GitAnswer<T> {
    /// The answer, or nothing when git refused.
    pub(crate) fn said(self) -> Option<T> {
        match self {
            Self::Said(output) => Some(output),
            Self::Refused(_) => None,
        }
    }

    /// The refusal, or nothing when git answered.
    pub(crate) fn refusal(&self) -> Option<&GitRefused> {
        match self {
            Self::Said(_) => None,
            Self::Refused(refused) => Some(refused),
        }
    }

    pub(crate) fn is_said(&self) -> bool {
        matches!(self, Self::Said(_))
    }

    /// Shape an answer, leaving a refusal exactly as it is.
    fn map<U>(self, shape: impl FnOnce(T) -> U) -> GitAnswer<U> {
        match self {
            Self::Said(output) => GitAnswer::Said(shape(output)),
            Self::Refused(refused) => GitAnswer::Refused(refused),
        }
    }
}

/// Why git could not answer, and what it said about that.
///
/// Two things, because they answer different questions and Python needed both:
///
/// - [`GitRefused::reason`] is the text — git's own stderr, or, when git was
///   silent, the command and its exit status. It is `git_errors.py`'s
///   `git_failure_reason`, and it is *data*: `workspace_state` puts it in front
///   of a person deciding whether to force a delete, `--ls --json` puts it on
///   the wire under `couldNotTell`, and two flows classify it by substring
///   (`"couldn't find remote ref"` for a ref the remote has not got,
///   `"already exists"` for a branch that is already there — both of which is
///   why the C locale is pinned on those two verbs).
/// - [`GitRefused::how`] is the fact the text cannot be classified from: a
///   non-zero exit, a git that is not installed, a bound that elapsed, an OS
///   refusal. `repo_manager.fetch_repo` reports a timeout differently from a
///   failure, and "git is not installed" is a diagnostic of its own; recovering
///   either by looking for words in a message would be reading tea leaves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitRefused {
    /// Never empty: every constructor falls back to naming the command.
    reason: String,
    pub(crate) how: Failure,
}

impl GitRefused {
    /// What a caller prints, classifies or carries. Never empty.
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// git ran and refused. `git_failure_reason`, exactly: stderr, trimmed; and
    /// when git said nothing, the command and its status — because "…: " with
    /// nothing after the colon tells the reader only that something went wrong,
    /// which they already knew.
    fn exited(named: &str, exit: Exit, stderr: &str) -> Self {
        let said = stderr.trim();
        let reason = if said.is_empty() {
            format!("git {named} exited {}", returncode(exit))
        } else {
            said.to_owned()
        };
        Self {
            reason,
            how: Failure::Exited(exit),
        }
    }

    /// Nothing was captured, so there is no stderr to quote — only what ran and
    /// how it ended. The uncaptured verbs are the LFS ones, whose output goes to
    /// the user's terminal as it happens (a multi-gigabyte fetch has to look like
    /// progress rather than a hang), and Python's message for them is the
    /// `CalledProcessError`'s own `str`, which is this same pair of facts.
    fn exited_unseen(named: &str, exit: Exit) -> Self {
        Self {
            reason: format!("git {named} exited {}", returncode(exit)),
            how: Failure::Exited(exit),
        }
    }

    /// git is not on PATH. Python reaches this as the `OSError` from `spawn` and
    /// carries its `str()`; this carries the same fact in this layer's own words,
    /// since the runner reports "not on PATH" as an arm rather than as an errno
    /// (a missing program and a missing working directory are one ENOENT).
    fn not_installed() -> Self {
        Self {
            reason: "git is not on PATH".to_owned(),
            how: Failure::GitNotInstalled,
        }
    }

    fn timed_out(named: &str, limit: Duration) -> Self {
        Self {
            reason: format!("git {named} timed out after {}s", limit.as_secs()),
            how: Failure::TimedOut,
        }
    }

    /// The OS refused for some other reason — an unreadable working directory,
    /// a process limit. The message is the OS's own, which is what Python's
    /// `str(OSError)` carries.
    fn not_started(failure: OsFailure) -> Self {
        let reason = match failure.errno {
            Some(errno) => std::io::Error::from_raw_os_error(errno).to_string(),
            None => format!("git could not be started ({:?})", failure.kind),
        };
        Self {
            reason,
            how: Failure::NotStarted(failure),
        }
    }
}

/// The four ways a verb does not answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    /// git ran to completion and refused.
    Exited(Exit),
    /// git is not installed.
    GitNotInstalled,
    /// The per-call bound elapsed and the child was killed.
    TimedOut,
    /// The OS would not start it at all.
    NotStarted(OsFailure),
}

/// Python's `returncode`: the exit status, or the negated signal number.
///
/// Spelled Python's way because it is interpolated into [`GitRefused::reason`],
/// which is compared as text; `subprocess` reports a child killed by SIGTERM as
/// `-15`, so a fallback message reads "exited -15".
fn returncode(exit: Exit) -> i32 {
    match exit {
        Exit::Code(code) => code,
        Exit::Signal(signal) => -signal,
    }
}

/// The one git client. Holds the runner and nothing else — no cache, no state,
/// no configuration: every verb is told which repository it is about.
#[derive(Clone, Copy)]
pub struct Git<'r> {
    runner: &'r dyn Runner,
}

impl<'r> Git<'r> {
    pub fn new(runner: &'r dyn Runner) -> Self {
        Self { runner }
    }

    // ----------------------------------------------- one repository, pinned

    /// Ask git about *repo* — and only about *repo*.
    ///
    /// `--git-dir` and `--work-tree` are the fix for devlaunch#171 and are not
    /// decoration. A `cwd` alone leaves git's repository discovery switched on,
    /// and discovery walks up the parent chain: on a clone whose `.git` is
    /// unusable git does not refuse, it finds an **ancestor** repository and
    /// answers about that one. With dl's cache under `$XDG_CACHE_HOME` and a
    /// dotfiles repository in `$HOME`, that ancestor is common; when it was clean
    /// and fully pushed, the delete guard read "nothing to report" and destroyed
    /// a clone holding an hour of untracked work. Naming the git directory
    /// switches discovery off, so the only repository git can reach is this one
    /// and an unusable `.git` becomes a refusal — which is what the caller needs.
    ///
    /// Verified against real git 2.55.0 on each shape a broken clone takes: a
    /// `.git` directory holding garbage, an empty `.git`, a `.git` with HEAD and
    /// nothing else, a real clone with its object store deleted, and a truncated
    /// gitfile. All five refuse. A healthy clone and a *linked worktree* (whose
    /// `.git` is a gitfile, which git follows) both still answer normally, so
    /// pinning the clone down costs nothing.
    ///
    /// `--work-tree` earns its place separately from `--git-dir`.
    /// `core.worktree` in the clone's own config points the work tree at another
    /// directory and `--git-dir` alone honours it, so `git status --porcelain`
    /// would compare this clone's index against *that* directory. Where the other
    /// directory mirrors HEAD — a second checkout of the same commit, which is
    /// what `core.worktree` is normally pointed at — git prints nothing at rc 0,
    /// and the clone below, holding work that exists nowhere else, reads as
    /// clean. That is devlaunch#171's failure class reached by a second route,
    /// and it is the fail-*open* one. `core.bare = true` is the neighbouring
    /// shape; with `--work-tree` given it answers about the real clone too.
    ///
    /// `GIT_CEILING_DIRECTORIES` was the other candidate and is not used: it
    /// bounds discovery instead of switching it off, so it has to match what git
    /// resolved the clone's parent to, and when it does not match it fails
    /// *open* — back to the ancestor, silently.
    ///
    /// `cwd = repo` is kept even though it no longer selects the repository, so
    /// that `git status` keeps printing paths relative to the clone root.
    ///
    /// **Only trailing newlines are trimmed, never leading whitespace.** A full
    /// trim here was wrong in a way that took real use to notice: the porcelain
    /// line for a *modified tracked* file begins with a space (`" M pixi.lock"`),
    /// so trimming ate the status column and the path was reported one character
    /// short. Untracked entries start `??` and were unharmed, which is exactly
    /// why the tests missed it.
    fn about(&self, repo: &Path, args: &[&str]) -> GitAnswer<String> {
        let root = pinned_root(repo);
        let mut argv = vec![
            format!("--git-dir={}", root.join(".git").display()),
            format!("--work-tree={}", root.display()),
        ];
        argv.extend(args.iter().map(|arg| (*arg).to_owned()));
        let spec = SpawnSpec::new(
            Invocation::new(PROGRAM)
                .with_args(argv)
                .with_cwd(repo.to_path_buf()),
        )
        .with_timeout(ABOUT_ONE_REPO);
        // Named by the whole argument list rather than by a verb: this family's
        // fallback message is `workspace_state._git`'s, which spells out what was
        // asked ("git status --porcelain exited 128"), where every other site
        // names the subcommand alone.
        self.captured(&args.join(" "), &spec)
            .map(|stdout| stdout.trim_end_matches('\n').to_owned())
    }

    /// The branch *clone* has checked out, as `rev-parse --abbrev-ref HEAD`.
    ///
    /// Refuses on an unusable `.git`, and *also* on a repository with an unborn
    /// HEAD — a clone of an empty repository, where git exits 128 saying `HEAD`
    /// is an ambiguous argument. Both mean "git could not say which branch", and
    /// the caller reads them the same way.
    pub(crate) fn head_branch(&self, clone: &Path) -> GitAnswer<String> {
        self.about(clone, &["rev-parse", "--abbrev-ref", "HEAD"])
    }

    /// The porcelain status of *clone*: one line per changed path, untracked
    /// included, with the two status columns and their space intact.
    ///
    /// Doubles as the repository probe. With the repository named it succeeds on
    /// anything git can read — including a repository with no commits yet — and
    /// refuses on every unusable `.git`.
    pub(crate) fn status_porcelain(&self, clone: &Path) -> GitAnswer<String> {
        self.about(clone, &["status", "--porcelain"])
    }

    /// Commits on *branch* that no remote-tracking ref contains.
    ///
    /// **Argument order is load-bearing.** `--not` flips the sense of every ref
    /// *after* it, so the branch has to be named before it:
    /// `log --not --remotes <branch>` excludes the branch as well and is silently
    /// always empty — which would report every clone as safe to delete.
    ///
    /// `--remotes` rather than the branch's upstream, so work pushed under
    /// another name, or merged and fetched back, is correctly not counted as
    /// lost.
    pub(crate) fn unpushed_commits(&self, clone: &Path, branch: &str) -> GitAnswer<String> {
        self.about(clone, &["log", "--oneline", branch, "--not", "--remotes"])
    }

    /// Every worktree registered in *clone*, as `worktree list --porcelain`
    /// writes it: the clone's own entry first, then one paragraph per linked
    /// worktree carrying its `branch` or `detached`, and `locked` or `prunable`
    /// where git says so.
    ///
    /// **The registered paths are not necessarily paths on this machine.** An
    /// agent harness running inside a devcontainer registers its worktrees at the
    /// container's `/workspaces/<id>/…`, and the very same directories are
    /// reached from the host through the clone. So this output is read for what
    /// git *thinks* about each registration, and a registration is matched to a
    /// directory by its place inside the clone rather than by resolving the path
    /// git prints, which on a host resolves to nothing (devlaunch#426).
    pub(crate) fn worktree_listing(&self, clone: &Path) -> GitAnswer<String> {
        self.about(clone, &["worktree", "list", "--porcelain"])
    }

    /// Drop the registrations whose worktree directory git can no longer find.
    ///
    /// Locked registrations are skipped by git itself, which is what makes a lock
    /// survive this and go on protecting a worktree somebody may be working in.
    ///
    /// All-or-nothing across the clone: git offers no way to drop one
    /// registration and keep another, which is why the caller decides whether a
    /// clone may be pruned at all rather than which of its registrations goes.
    pub(crate) fn worktree_prune(&self, clone: &Path) -> GitAnswer<String> {
        self.about(clone, &["worktree", "prune"])
    }

    /// The porcelain status of one linked worktree, asked through the clone's
    /// admin directory for it.
    ///
    /// **Not [`Git::about`], and the difference is what makes this answerable at
    /// all.** A linked worktree's own `.git` is a gitfile, and for an agent
    /// worktree that gitfile names a path inside a container. Pointing
    /// `--git-dir` at it on a host resolves nothing and git refuses — so a dirty
    /// check that went the ordinary way would report every one of these as
    /// unreadable, which is a refusal to reclaim anything.
    /// `--git-dir=<clone>/.git/worktrees/<name>` is the same repository reached
    /// from the side that does resolve here, and `--work-tree` is the directory
    /// on this host.
    ///
    /// **Every line git prints comes back, including the nested agent worktrees
    /// the caller reasons about separately.** This used to carry a
    /// `:!.claude/worktrees` pathspec, which excluded the *place* rather than the
    /// thing and so also hid a tracked file modified under that path and plain
    /// content sitting there that is not a worktree at all (devlaunch#442 review,
    /// S4). Which entries to disregard is a question about what a directory *is*,
    /// which is `flows::agent_worktrees`'s question and not a git client's, so the
    /// filtering is done there and the name of the directory stays in the one
    /// module that reasons about it.
    pub(crate) fn worktree_dirt(&self, admin: &Path, work_tree: &Path) -> GitAnswer<String> {
        let args = [
            format!("--git-dir={}", admin.display()),
            format!("--work-tree={}", work_tree.display()),
            "status".to_owned(),
            "--porcelain".to_owned(),
        ];
        self.captured(
            "status --porcelain",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args(args)
                    .with_cwd(work_tree.to_path_buf()),
            )
            .with_timeout(ABOUT_ONE_REPO),
        )
        .map(|stdout| stdout.trim_end_matches('\n').to_owned())
    }

    /// How many commits are reachable from *rev* and from no ref in *repo*.
    ///
    /// Asked of the sibling bare cache, which is the repository devlaunch
    /// actually fetches into, so `--all` there means "everything the forge had at
    /// the last fetch". `0` is therefore the one answer that says a commit is
    /// safely somewhere else.
    ///
    /// A refusal is usually `bad object`: the cache has never seen the commit,
    /// which is what an unpushed branch looks like from over there. The caller
    /// reads it that way and asks the clone as well rather than treating it as an
    /// error.
    pub(crate) fn commits_beyond_every_ref(&self, repo: &Path, rev: &str) -> GitAnswer<String> {
        self.captured(
            "rev-list",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args(["rev-list", "--count", rev, "--not", "--all"])
                    .with_cwd(repo.to_path_buf()),
            )
            .with_timeout(ABOUT_ONE_REPO),
        )
        .map(trimmed)
    }

    // ------------------------------------------------------- the bare cache

    /// `git clone --bare <remote_url> <bare>` — the cache for one repository.
    ///
    /// Bare so that no branch is checked out and every branch can be cloned from
    /// without conflict. No cwd: the destination is absolute, and Python passed
    /// none.
    pub(crate) fn clone_bare(&self, remote_url: &str, bare: &Path) -> GitAnswer<String> {
        self.captured(
            "clone",
            &SpawnSpec::new(Invocation::new(PROGRAM).with_args([
                "clone".to_owned(),
                "--bare".to_owned(),
                remote_url.to_owned(),
                bare.display().to_string(),
            ])),
        )
    }

    /// Sweep every head and tag into the bare cache.
    ///
    /// *limit* bounds it, and matters because this runs under the repo lock:
    /// whoever holds that lock for the length of a fetch is somebody every other
    /// dl run wanting the same repository waits for. A launch is watched and
    /// interruptible and passes `None`; the detached background sweep is neither
    /// and passes a bound.
    pub(crate) fn fetch_all(&self, bare: &Path, limit: Option<Duration>) -> GitAnswer<String> {
        let mut spec = SpawnSpec::new(
            Invocation::new(PROGRAM)
                .with_args([
                    "fetch",
                    "origin",
                    "+refs/heads/*:refs/heads/*",
                    "--tags",
                    "--prune",
                ])
                .with_cwd(bare.to_path_buf()),
        );
        spec.timeout = limit;
        self.captured("fetch", &spec)
    }

    /// Fetch exactly one branch into the bare cache.
    ///
    /// The launch path's entire network budget, so the time it can hold the repo
    /// lock is bounded by one branch's objects rather than by the repository's
    /// whole history of branches.
    ///
    /// The C locale is pinned because the caller classifies the failure from
    /// git's stderr text (`"couldn't find remote ref"` is the one non-zero exit
    /// that is an *answer*), and git translates that text — so a German host
    /// would collapse a three-way outcome to two. `LANGUAGE` is pinned as well:
    /// under gettext it outranks a non-C `LC_ALL`, and the guarantee should not
    /// hang on the one glibc rule that exempts C.
    pub(crate) fn fetch_ref(&self, bare: &Path, branch: &str) -> GitAnswer<String> {
        self.captured(
            "fetch",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args([
                        "fetch".to_owned(),
                        "origin".to_owned(),
                        format!("+{REFS_HEADS}{branch}:{REFS_HEADS}{branch}"),
                    ])
                    .with_cwd(bare.to_path_buf())
                    .with_env(c_locale()),
            ),
        )
    }

    /// What a symbolic ref points at, e.g. `HEAD` in a bare clone →
    /// `refs/heads/main`. Trimmed, as Python trims it.
    pub(crate) fn symbolic_ref(&self, repo: &Path, name: &str) -> GitAnswer<String> {
        self.captured(
            "symbolic-ref",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args(["symbolic-ref", name])
                    .with_cwd(repo.to_path_buf()),
            ),
        )
        .map(trimmed)
    }

    /// `git branch -r`, as text.
    ///
    /// Text rather than a parsed list because its one caller searches it as text
    /// (`"origin/main" in branches`) on the way to a last-resort default branch,
    /// and `branch -r` prints things that are not branch names
    /// (`origin/HEAD -> origin/main`).
    pub(crate) fn remote_branch_listing(&self, repo: &Path) -> GitAnswer<String> {
        self.captured(
            "branch",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args(["branch", "-r"])
                    .with_cwd(repo.to_path_buf()),
            ),
        )
        .map(trimmed)
    }

    /// Ask a remote what its HEAD points at, without a clone.
    ///
    /// Bounded at ten seconds: it is the fallback in front of "call it `main`",
    /// so a remote that hangs must cost a pause rather than the launch.
    pub(crate) fn ls_remote_symref_head(&self, remote_url: &str) -> GitAnswer<String> {
        self.captured(
            "ls-remote",
            &SpawnSpec::new(Invocation::new(PROGRAM).with_args([
                "ls-remote",
                "--symref",
                remote_url,
                "HEAD",
            ]))
            .with_timeout(ASK_REMOTE_FOR_HEAD),
        )
    }

    /// The branch names the bare cache holds locally.
    ///
    /// Reads refs off the disk, so the bound is two seconds: it feeds shell
    /// completion, where a pause is worse than a short list.
    pub(crate) fn local_branches(&self, bare: &Path) -> GitAnswer<Vec<String>> {
        self.captured(
            "for-each-ref",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args(["for-each-ref", "--format=%(refname:short)", REFS_HEADS])
                    .with_cwd(bare.to_path_buf()),
            )
            .with_timeout(READ_LOCAL_REFS),
        )
        .map(|stdout| lines(&stdout))
    }

    // ----------------------------------------------------------- branches

    /// `git branch <branch> <start_point>` — create a local branch.
    ///
    /// The C locale is pinned for the same reason as [`Git::fetch_ref`]'s: the
    /// caller swallows this failure when the reason says `"already exists"`, and
    /// on a translated host an ordinary re-launch of a branch that is already
    /// there would raise instead.
    pub(crate) fn create_branch(
        &self,
        repo: &Path,
        branch: &str,
        start_point: &str,
    ) -> GitAnswer<String> {
        self.captured(
            "branch",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args(["branch", branch, start_point])
                    .with_cwd(repo.to_path_buf())
                    .with_env(c_locale()),
            ),
        )
    }

    /// Point *branch*'s upstream at `<remote>/<branch>`.
    pub(crate) fn set_upstream(
        &self,
        repo: &Path,
        branch: &str,
        remote: &str,
    ) -> GitAnswer<String> {
        self.captured(
            "branch",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args([
                        "branch".to_owned(),
                        format!("--set-upstream-to={remote}/{branch}"),
                        branch.to_owned(),
                    ])
                    .with_cwd(repo.to_path_buf()),
            ),
        )
    }

    /// `git show-ref --verify <ref>` — whether one exact ref is there.
    ///
    /// Answers with git's own outcome rather than a bool: `show-ref --verify`
    /// exits non-zero both for a ref that is absent and for a directory that is
    /// not a repository, and Python's two callers collapse those to `False`. The
    /// collapse is theirs to keep or to reconsider; the client does not decide it
    /// for them.
    ///
    /// Build the ref with [`refs_heads`] or [`refs_remotes`] rather than by hand.
    pub(crate) fn verify_ref(&self, repo: &Path, reference: &str) -> GitAnswer<String> {
        self.captured(
            "show-ref",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args(["show-ref", "--verify", reference])
                    .with_cwd(repo.to_path_buf()),
            ),
        )
    }

    /// The branch names a remote has, asked from inside *repo* — one named
    /// branch, or all of them.
    ///
    /// Bounded at five seconds, `dl.py`'s cap on every remote `ls-remote` it
    /// issued (`get_remote_branches`/`_git_ls_remote`, both `timeout=5`): this is
    /// a network round trip on the refresh and completion paths, so an
    /// unreachable remote must cost a pause, not an unbounded hang.
    pub(crate) fn ls_remote_heads(
        &self,
        repo: &Path,
        remote: &str,
        branch: Option<&str>,
    ) -> GitAnswer<Vec<String>> {
        let mut argv = vec![
            "ls-remote".to_owned(),
            "--heads".to_owned(),
            remote.to_owned(),
        ];
        argv.extend(branch.map(str::to_owned));
        self.captured(
            "ls-remote",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args(argv)
                    .with_cwd(repo.to_path_buf()),
            )
            .with_timeout(ASK_REMOTE_FOR_REFS),
        )
        .map(|stdout| branches_in_ls_remote(&stdout))
    }

    /// `git push -u <remote> <branch>`.
    ///
    /// *ssh_key* names a key to use instead of whatever the agent offers, and it
    /// reaches git through `GIT_SSH_COMMAND`, which is a **shell string** rather
    /// than argv — so the path is quoted: a key under a directory with a space in
    /// it would otherwise be split, and ssh would get a truncated `-i` and the
    /// remainder as a hostname. The variable is layered on the inherited
    /// environment, not substituted for it: a push with no PATH cannot find the
    /// ssh it was just told to run, and one with no HOME or `SSH_AUTH_SOCK`
    /// cannot read `known_hosts` or reach the agent — so naming a key would be
    /// what breaks the authentication it sets up.
    pub(crate) fn push_branch(
        &self,
        repo: &Path,
        remote: &str,
        branch: &str,
        ssh_key: Option<&Path>,
    ) -> GitAnswer<String> {
        let mut invocation = Invocation::new(PROGRAM)
            .with_args(["push", "-u", remote, branch])
            .with_cwd(repo.to_path_buf());
        if let Some(key) = ssh_key {
            invocation = invocation.with_var("GIT_SSH_COMMAND", ssh_command(key));
        }
        self.captured("push", &SpawnSpec::new(invocation))
    }

    // --------------------------------------------------- workspace clones

    /// Clone a workspace out of the bare cache.
    ///
    /// **The flags this does *not* pass are the load-bearing part.**
    /// `git clone <path> <path>` hardlinks the pack files instead of copying
    /// them, which is the whole reason a workspace per branch is affordable:
    /// measured on this repository, each further workspace's `.git` costs 196 KB
    /// instead of 2268 KB. A `file://` source, an intermediate copy or an
    /// explicit `--no-hardlinks` each lose that silently, and no flag here
    /// guards it — `--local` is already the default and does not even error on a
    /// `file://` source, and `--shared`/`--reference` were measured to leave an
    /// fsck-broken workspace once the cache's force-refspec fetch and gc have
    /// run. An integration test asserts the shared inode instead.
    ///
    /// `GIT_LFS_SKIP_SMUDGE=1` because the bare cache is the repository's LFS
    /// store only for refs some earlier launch already materialized: it arrives
    /// empty and is filled one ref at a time *after* this clone, so a smudge here
    /// has nothing to fetch on a repository's first workspace and fails with
    /// "remote missing object" whenever it comes up short.
    pub(crate) fn clone_from_cache(&self, bare: &Path, workspace: &Path) -> GitAnswer<String> {
        self.captured(
            "clone",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args([
                        "clone".to_owned(),
                        bare.display().to_string(),
                        workspace.display().to_string(),
                    ])
                    .with_var("GIT_LFS_SKIP_SMUDGE", "1"),
            ),
        )
    }

    /// Point a clone's remote at the real forge, away from the bare cache it was
    /// cloned from.
    pub(crate) fn set_remote_url(
        &self,
        workspace: &Path,
        remote: &str,
        url: &str,
    ) -> GitAnswer<String> {
        self.captured(
            "remote set-url",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args(["remote", "set-url", remote, url])
                    .with_cwd(workspace.to_path_buf()),
            ),
        )
    }

    /// `git checkout <branch>` — the existing-workspace path, which preserves
    /// local work.
    pub(crate) fn checkout(&self, workspace: &Path, branch: &str) -> GitAnswer<String> {
        self.captured(
            "checkout",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args(["checkout", branch])
                    .with_cwd(workspace.to_path_buf()),
            ),
        )
    }

    /// `git checkout -B <branch> <start_point>` — the new-workspace path, which
    /// resets the branch to the ref it was cut from so a launch starts from the
    /// latest commit rather than from a stale clone.
    pub(crate) fn checkout_reset(
        &self,
        workspace: &Path,
        branch: &str,
        start_point: &str,
    ) -> GitAnswer<String> {
        self.captured(
            "checkout",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args(["checkout", "-B", branch, start_point])
                    .with_cwd(workspace.to_path_buf()),
            ),
        )
    }

    /// Every path in HEAD's tree *or* the index.
    ///
    /// The union is load-bearing, which is what `--with-tree=HEAD` buys: the
    /// index alone is a strictly smaller set than what git-lfs can name, and the
    /// gap is reachable with no user action — a clone left with no `.git/index`
    /// makes `git ls-files` exit *zero with empty output*, and reading that as
    /// "nothing tracked, so no pointers" would strand the workspace on stub files
    /// on every later launch.
    ///
    /// `-z` because a path may contain a newline. Python decodes each name with
    /// `os.fsdecode`, which round-trips a name that is not UTF-8; the runner
    /// decodes captured output lossily, so such a name arrives here with
    /// replacement characters and will not open — [`is_lfs_pointer`] then answers
    /// `false` for it where Python could have answered `true`. Noted rather than
    /// papered over: it needs a non-UTF-8 path *holding an LFS pointer*, and the
    /// runner captures text by design.
    pub(crate) fn tracked_files(&self, workspace: &Path) -> GitAnswer<Vec<String>> {
        self.captured(
            "ls-files",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args(["ls-files", "-z", "--with-tree=HEAD"])
                    .with_cwd(workspace.to_path_buf()),
            ),
        )
        .map(|stdout| nul_separated(&stdout))
    }

    // --------------------------------------------------------------- LFS

    /// The paths in the tree that git-lfs tracks.
    pub(crate) fn lfs_tracked_files(&self, workspace: &Path) -> GitAnswer<Vec<String>> {
        self.captured(
            "lfs ls-files",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args(["lfs", "ls-files", "--name-only"])
                    .with_cwd(workspace.to_path_buf()),
            ),
        )
        .map(|stdout| lines(&stdout))
    }

    /// Fetch one ref's LFS objects into the bare cache's own store.
    ///
    /// A bare clone arrives with an **empty** `lfs/` directory — git-lfs has no
    /// bare-clone hook — so this call is what makes the cache the repository's
    /// store. The bare as cwd is the only thing that decides where the objects
    /// land: git-lfs writes them under the git directory it was invoked in, so no
    /// `lfs.storage` override is needed, which is the point.
    ///
    /// The two `fetchrecent` knobs bound what it costs: `git lfs fetch` otherwise
    /// also walks recent refs and recent commits, so a repository with many
    /// branches would download several branches' payloads to launch one. Passed
    /// with `-c` rather than written into the cache's config, because this is a
    /// property of *this fetch* and not of the repository, whose config every
    /// workspace of it shares.
    ///
    /// Uncaptured: a multi-gigabyte fetch has to show progress rather than look
    /// like a hang.
    pub(crate) fn lfs_fetch_into_cache(&self, bare: &Path, reference: &str) -> GitAnswer<()> {
        self.uncaptured(
            "lfs fetch",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args([
                        "-c",
                        "lfs.fetchrecentrefsdays=0",
                        "-c",
                        "lfs.fetchrecentcommitsdays=0",
                        "lfs",
                        "fetch",
                        "origin",
                        reference,
                    ])
                    .with_cwd(bare.to_path_buf()),
            ),
        )
    }

    /// Materialize a workspace's LFS content out of the bare cache.
    ///
    /// `file://<bare>` **as a URL on the command line**, and each half of that is
    /// load-bearing. As an argument rather than a configured remote, because the
    /// clone directory is bind-mounted into the devcontainer and `.bare` is not:
    /// a host path persisted into the clone names a directory that does not exist
    /// inside the container, and git-lfs reads it on every checkout. And
    /// `file://` rather than `-c lfs.storage=<bare>/lfs`, which was measured to
    /// break against local-path remotes outright — `-c` reaches children through
    /// `GIT_CONFIG_PARAMETERS`, and the remote-side git-lfs then reads the *local*
    /// store as its own.
    ///
    /// Measured (git-lfs 3.7.1, ext4): this hardlinks, so the workspace's object
    /// is the same `(st_dev, st_ino)` as the cache's and costs zero bytes, and it
    /// succeeds with `origin` pointing at a URL that does not resolve — zero
    /// network.
    pub(crate) fn lfs_pull_from_cache(&self, workspace: &Path, bare: &Path) -> GitAnswer<()> {
        self.uncaptured(
            "lfs pull",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args(["lfs".to_owned(), "pull".to_owned(), file_url(bare)])
                    .with_cwd(workspace.to_path_buf()),
            ),
        )
    }

    /// `git lfs pull origin` — the network phase, entered only when a pointer
    /// survived the cache phase.
    pub(crate) fn lfs_pull_origin(&self, workspace: &Path) -> GitAnswer<()> {
        self.uncaptured(
            "lfs pull",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args(["lfs", "pull", "origin"])
                    .with_cwd(workspace.to_path_buf()),
            ),
        )
    }

    // ------------------------------------------------- remotes, from outside

    /// The origin URL of the repository at *path*.
    ///
    /// `-C <path>` rather than a cwd, which is what Python passed: the difference
    /// is invisible to git here, and argv is what parity is judged on.
    pub(crate) fn origin_url_at(&self, path: &Path) -> GitAnswer<String> {
        self.captured(
            "remote get-url",
            &SpawnSpec::new(Invocation::new(PROGRAM).with_args([
                "-C".to_owned(),
                path.display().to_string(),
                "remote".to_owned(),
                "get-url".to_owned(),
                "origin".to_owned(),
            ])),
        )
        .map(trimmed)
    }

    /// The branches a remote URL has, asked from nowhere in particular.
    ///
    /// Distinct from [`Git::ls_remote_heads`] in argv as well as in cwd — this is
    /// `ls-remote --heads <url>`, that is `ls-remote --heads <remote> [branch]`
    /// from inside a clone — because both spellings exist in Python and parity is
    /// judged on argv.
    pub(crate) fn ls_remote_heads_of(&self, remote_url: &str) -> GitAnswer<Vec<String>> {
        self.captured(
            "ls-remote",
            &SpawnSpec::new(Invocation::new(PROGRAM).with_args([
                "ls-remote",
                "--heads",
                remote_url,
            ]))
            .with_timeout(ASK_REMOTE_FOR_REFS),
        )
        .map(|stdout| branches_in_ls_remote(&stdout))
    }

    /// `git ls-remote <url> <args…>` — the URL before the options, which is the
    /// order `dl.py`'s helper built and therefore the order the shim log
    /// compares.
    /// Only this module's tests ask for the raw form; the completion flows go
    /// through [`Git::ls_remote_heads`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn ls_remote(&self, remote_url: &str, args: &[&str]) -> GitAnswer<String> {
        let mut argv = vec!["ls-remote".to_owned(), remote_url.to_owned()];
        argv.extend(args.iter().map(|arg| (*arg).to_owned()));
        self.captured(
            "ls-remote",
            &SpawnSpec::new(Invocation::new(PROGRAM).with_args(argv))
                .with_timeout(ASK_REMOTE_FOR_REFS),
        )
    }

    // ------------------------------------------------------------ spawning

    /// Run a verb whose output is the answer.
    fn captured(&self, named: &str, spec: &SpawnSpec) -> GitAnswer<String> {
        answered(named, spec.timeout, self.runner.capture(spec))
    }

    /// Run a verb whose output belongs on the user's terminal.
    fn uncaptured(&self, named: &str, spec: &SpawnSpec) -> GitAnswer<()> {
        match self.runner.passthrough(spec) {
            Outcome::Ran { exit, .. } if exit.is_success() => GitAnswer::Said(()),
            Outcome::Ran { exit, .. } => GitAnswer::Refused(GitRefused::exited_unseen(named, exit)),
            Outcome::ProgramNotFound => GitAnswer::Refused(GitRefused::not_installed()),
            Outcome::TimedOut => GitAnswer::Refused(GitRefused::timed_out(
                named,
                spec.timeout.unwrap_or_default(),
            )),
            Outcome::NotStarted(failure) => GitAnswer::Refused(GitRefused::not_started(failure)),
        }
    }
}

/// A captured outcome, as an answer.
fn answered(
    named: &str,
    limit: Option<Duration>,
    outcome: Outcome<CapturedText>,
) -> GitAnswer<String> {
    match outcome {
        Outcome::Ran { exit, io } if exit.is_success() => GitAnswer::Said(io.stdout),
        Outcome::Ran { exit, io } => {
            GitAnswer::Refused(GitRefused::exited(named, exit, &io.stderr))
        }
        Outcome::ProgramNotFound => GitAnswer::Refused(GitRefused::not_installed()),
        Outcome::TimedOut => {
            GitAnswer::Refused(GitRefused::timed_out(named, limit.unwrap_or_default()))
        }
        Outcome::NotStarted(failure) => GitAnswer::Refused(GitRefused::not_started(failure)),
    }
}

/// `refs/heads/<branch>` — the ref [`Git::verify_ref`] is asked for a local
/// branch.
pub(crate) fn refs_heads(branch: &str) -> String {
    format!("{REFS_HEADS}{branch}")
}

/// `refs/remotes/<remote>/<branch>` — the ref [`Git::verify_ref`] is asked for a
/// remote-tracking branch.
pub(crate) fn refs_remotes(remote: &str, branch: &str) -> String {
    format!("refs/remotes/{remote}/{branch}")
}

/// The branch a symbolic ref names, with its namespace prefix removed.
///
/// **Not the last path segment.** `release/1.0` and `feature/auth` are ordinary
/// branch names, and taking the segment after the final slash renames them to
/// `1.0` and `auth` — refs the repository does not have, recorded as the one every
/// later operation targets. The last-segment fallback survives for a ref in
/// neither namespace, which is where Python left it.
pub(crate) fn branch_in_symbolic_ref(reference: &str) -> &str {
    for prefix in ["refs/remotes/origin/", REFS_HEADS] {
        if let Some(branch) = reference.strip_prefix(prefix) {
            return branch;
        }
    }
    reference.rsplit('/').next().unwrap_or(reference)
}

/// The absolute, symlink-free directory `--git-dir` and `--work-tree` name.
///
/// Total where Python's `Path.resolve()` is total: a path that cannot be
/// canonicalized (a component removed under us) falls back to an absolute path
/// built without touching the filesystem, and then to the path as given — git
/// resolves a relative `--git-dir` against the cwd this call also sets, so the
/// fallback still names the same directory.
fn pinned_root(repo: &Path) -> PathBuf {
    std::fs::canonicalize(repo)
        .or_else(|_| std::path::absolute(repo))
        .unwrap_or_else(|_| repo.to_path_buf())
}

/// The environment for the two verbs whose failure is classified from git's own
/// words. See [`Git::fetch_ref`].
fn c_locale() -> EnvSpec {
    EnvSpec::inherited().and("LC_ALL", "C").and("LANGUAGE", "C")
}

/// The `GIT_SSH_COMMAND` shell string for a named key.
fn ssh_command(key: &Path) -> String {
    format!(
        "ssh -i {} -o IdentitiesOnly=yes",
        shlex::try_quote(&key.display().to_string())
            .map(|quoted| quoted.into_owned())
            // A path with a NUL in it cannot be quoted for a shell, and cannot
            // name a key either; the unquoted spelling is what git would have
            // been handed anyway, and ssh will refuse it.
            .unwrap_or_else(|_| key.display().to_string())
    )
}

/// `file://<path>` — the way the bare cache is named as a git-lfs remote.
fn file_url(path: &Path) -> String {
    format!("file://{}", path.display())
}

fn trimmed(output: String) -> String {
    output.trim().to_owned()
}

/// The non-empty lines of *output*, trimmed of the trailing newline every git
/// verb ends with.
fn lines(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The non-empty entries of a `-z` listing.
fn nul_separated(output: &str) -> Vec<String> {
    output
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The branch names in `git ls-remote --heads` output.
///
/// Each line is `<hash>\trefs/heads/<branch>`. Python has two readings of this —
/// `branch_manager` splits on the tab and strips the prefix, `dl.py` splits on
/// the last occurrence of `refs/heads/` — which agree on every line git emits;
/// this is the tab-based one, and a line that is not that shape is dropped rather
/// than guessed at.
pub(crate) fn branches_in_ls_remote(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter_map(|(_hash, reference)| reference.trim().strip_prefix(REFS_HEADS))
        .filter(|branch| !branch.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The branch a remote's HEAD points at, from `ls-remote --symref <url> HEAD`.
///
/// The output's first line is `ref: refs/heads/<branch>\tHEAD`. `None` means the
/// remote answered without one — a repository with no HEAD, or output in a shape
/// this does not recognise, which the caller treats as "no default branch named"
/// rather than as a failure.
pub(crate) fn head_branch_in_symref(output: &str) -> Option<String> {
    output
        .lines()
        .filter(|line| line.starts_with("ref:") && line.contains("HEAD"))
        .filter_map(|line| line.split_whitespace().nth(1))
        .filter_map(|reference| reference.strip_prefix(REFS_HEADS))
        .find(|branch| !branch.is_empty())
        .map(str::to_owned)
}

/// Whether git-lfs is installed.
///
/// A PATH lookup rather than a spawn, which is what Python's
/// `shutil.which("git-lfs")` is: the answer gates a fork, so paying a fork to
/// learn it would defeat the point. `git-lfs` is the name to look for — `git lfs`
/// is this program dispatching to that one.
pub(crate) fn lfs_is_installed() -> bool {
    std::env::var_os("PATH")
        .as_deref()
        .is_some_and(lfs_is_installed_along)
}

/// [`lfs_is_installed`] against a PATH given rather than read, so it can be
/// asserted without a test mutating this process's environment.
fn lfs_is_installed_along(path: &std::ffi::OsStr) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::env::split_paths(path).any(|dir| {
        // An empty PATH entry means the current directory, as it does for the
        // shell.
        let dir = if dir.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            dir
        };
        std::fs::metadata(dir.join("git-lfs"))
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    })
}

/// Whether *path* holds an unmaterialized git-lfs pointer.
///
/// A path that will not open is **not** a pointer. Every ordinary workspace has
/// several — a deleted file, a dangling symlink, a submodule's directory, a path
/// a sparse checkout leaves off disk — and none of them says anything about LFS.
/// Answering `true` instead is not a harmless over-estimate: it reinstates the
/// git-lfs fork at the gate, and at the materialization call site it drives
/// `git lfs pull origin` — unbounded and uncaptured — on every launch of such a
/// workspace, forever, since the pull cannot put a path the checkout excludes
/// back on disk.
pub(crate) fn is_lfs_pointer(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = vec![0u8; LFS_POINTER_PREFIX.len()];
    // Python reads *up to* the prefix length and compares, so a file shorter
    // than the prefix is simply not a pointer; `read_exact` is the same test,
    // with a short read reported rather than silently compared.
    file.read_exact(&mut head).is_ok() && head == LFS_POINTER_PREFIX
}

// Private again: the fake runner this module used to define for
// `domain::workspace_state`'s tests is now `crate::testing`'s, so nothing outside
// reaches in here.
#[cfg(test)]
mod tests;
