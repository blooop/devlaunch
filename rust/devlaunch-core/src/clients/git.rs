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
//! becomes a [`GitAnswer`] — including what git's stderr *means*, which is the
//! one place devlaunch reads git's English. Three of [`Failure`]'s arms are that
//! reading; every caller matches an arm, so a reworded git message moves one
//! function in this file and nothing else. It decides nothing about *sequence*:
//! clone-if-missing, the fetch-then-create-branch dance, the recovery of a
//! half-removed cache and the LFS two-phase materialization are flows (M4b), and
//! what they get from here is verbs. A verb here never falls back to another verb, never retries, and
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

/// The one tag query, asked of a clone and of a bare in turn.
///
/// Written once because the answers are compared to each other: two spellings of
/// the same question would be two formats to keep in step, and a mismatch reads
/// as "every tag is local" rather than as a bug.
const TAG_REFS_QUERY: [&str; 3] = [
    "for-each-ref",
    "--format=%(objectname) %(refname)",
    "refs/tags/",
];

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
/// Two things, because they answer different questions:
///
/// - [`GitRefused::how`] is the fact, and it is what every decision is made on:
///   a non-zero exit, a branch that is already there, a ref the remote has not
///   got, a repository the host says it has not got, a git that is not
///   installed, a bound that elapsed, an OS refusal. The three of those that git
///   only ever says in words are read into [`Failure`] *here*, in the one module
///   that already reads git's stderr — see [`Failure`] for why the words are the
///   only signal available, and what was tried instead.
/// - [`GitRefused::reason`] is the text — git's own stderr, or, when git was
///   silent, the command and its exit status. It is `git_errors.py`'s
///   `git_failure_reason`, and it is for *reading*: `workspace_state` puts it in
///   front of a person deciding whether to force a delete, and `--ls --json`
///   puts it on the wire under `couldNotTell`. Nothing branches on it. That is
///   the invariant worth keeping: git may reword a message, and the only thing
///   that has to move is a reader below.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitRefused {
    /// Never empty: every constructor falls back to naming the command.
    reason: String,
    how: Failure,
}

impl GitRefused {
    /// What a caller prints or carries. Never empty, and never branched on —
    /// [`GitRefused::how`] is what a decision reads.
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// What a caller decides on.
    pub fn how(&self) -> Failure {
        self.how
    }

    /// git ran and refused. `git_failure_reason`, exactly: stderr, trimmed; and
    /// when git said nothing, the command and its status — because "…: " with
    /// nothing after the colon tells the reader only that something went wrong,
    /// which they already knew.
    ///
    /// `reading` is the verb's own reader — the single place git's wording is
    /// turned into a fact. A verb nothing branches on passes [`reads_nothing`]
    /// and lands on [`Failure::Exited`].
    fn exited(named: &str, exit: Exit, stderr: &str, reading: Reading) -> Self {
        let said = stderr.trim();
        let reason = if said.is_empty() {
            format!("git {named} exited {}", returncode(exit))
        } else {
            said.to_owned()
        };
        Self {
            how: reading(said).unwrap_or(Failure::Exited(exit)),
            reason,
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

/// The ways a verb does not answer.
///
/// Four of them are how the process ended. The other three are what git *said*,
/// read here rather than by a caller: they are all exits, and git exits 128 for
/// a missing ref, a refused key, a DNS failure and a branch that is already
/// there alike, so the status cannot tell them apart and the words are the only
/// signal there is. Each reader below records what was tried instead of the
/// words, and why it does not work.
///
/// Reading them here is the point of the arms. A caller matches one, so git can
/// reword a message and the only thing that moves is a reader in this file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// git ran to completion and refused, and devlaunch has no name for why.
    Exited(Exit),
    /// `git branch` refused because the branch is already there. Not a failure
    /// to its caller: two dl runs racing to start the same branch both succeed.
    BranchAlreadyExists,
    /// `git fetch` reached the remote and was told it has not got the ref. The
    /// one non-zero exit that is an *answer*, and its caller does something
    /// different with it: it bases the new branch on the default branch.
    RefMissingOnRemote,
    /// The host answered a clone by saying it has no such repository — as
    /// against refusing the key, failing to resolve the name, or any other
    /// reason a clone does not happen.
    RepositoryNotFound,
    /// git is not installed.
    GitNotInstalled,
    /// The per-call bound elapsed and the child was killed.
    TimedOut,
    /// The OS would not start it at all.
    NotStarted(OsFailure),
}

/// What one verb's stderr is read for. `None` leaves the refusal as
/// [`Failure::Exited`], which is what all but three of the verbs get.
type Reading = fn(&str) -> Option<Failure>;

/// The default: this verb's words are for a person, not for a decision.
fn reads_nothing(_stderr: &str) -> Option<Failure> {
    None
}

/// `fatal: a branch named 'x' already exists`.
///
/// The tail of the sentence, so git's capitalisation does not move it: up to
/// v2.34.0 git wrote `A branch named '%s' already exists.` (`branch.c:208`), and
/// from v2.35.0 it writes the same sentence lowercase and unstopped
/// (`branch.c:307`). [`c_locale`] is pinned on the verb because the sentence goes
/// through `die(_())` and is translated.
///
/// Not `exists` alone: git's other refusal here reads `cannot lock ref
/// 'refs/heads/a': 'refs/heads/a/b' exists; cannot create 'refs/heads/a'`, which
/// is a name colliding with a ref namespace rather than the branch being there,
/// and is a failure its caller must not swallow.
///
/// Asking `show-ref` afterwards instead of reading this would cost a second
/// spawn on the launch path and answer a different question — whether the branch
/// is there *now*, which a concurrent dl could have made true in between.
fn branch_already_exists(stderr: &str) -> Option<Failure> {
    stderr
        .contains("already exists")
        .then_some(Failure::BranchAlreadyExists)
}

/// `fatal: couldn't find remote ref refs/heads/x`.
///
/// Case-insensitive, because [`c_locale`] does not cover this one on an old git:
/// up to v2.20.0 git wrote it `Couldn't find remote ref %s` through a bare
/// `die()` rather than `die(_())` (`remote.c:1785`), so it was neither lowercase
/// nor translatable and the pinned locale could not reach it. From v2.21.0 it is
/// `die(_("couldn't find remote ref %s"))` (`remote.c:1840`) — lowercase and
/// translated, which is what the pinned locale is for.
fn ref_missing_on_remote(stderr: &str) -> Option<Failure> {
    stderr
        .to_lowercase()
        .contains("couldn't find remote ref")
        .then_some(Failure::RefMissingOnRemote)
}

/// A host saying it has not got the repository, told from every other way a
/// clone fails.
///
/// Four wordings — three hosts' own, and one of git's. GitHub's `Repository not
/// found` (ssh and https both), GitLab's `The project you were looking for could
/// not be found`, Bitbucket's `conq: repository does not exist`, and git's own
/// `repository '<url>' not found`, which `git-remote-http` writes for an HTTP 404
/// and is all a host whose 404 body says something else gets — Codeberg
/// (Forgejo) answers `remote: Not found.`, which carries none of the other three.
///
/// Each host phrase is matched whole, and each shorter form was tried and
/// rejected. `not exist` alone also catches git's *local* complaint, `repository
/// '/some/path' does not exist`, which is a missing directory rather than a
/// host's answer. `could not be found` alone is generic English rather than
/// anything GitLab specifically said. And `and the repository exists` rides along
/// with every ssh failure git reports, refused keys included, one word from the
/// wording above.
///
/// git's own line is matched as a whole line ending in `' not found`, for the
/// same reason: it keeps `repository '/some/path' does not exist` out, and it
/// keeps out every other `'%s' not found` git has — `branch '%s' not found`,
/// `tag '%s' not found` — that could otherwise ride along on a line of its own.
///
/// # Why [`Git::clone_bare`] pins no locale, when the other two reading verbs do
///
/// Three of the four phrases are the **remote's** bytes, relayed over the wire by
/// the host's own git-upload-pack and never passed through git's gettext
/// catalogue, so a French locale does not move them. The fourth is git's own and
/// is translated — but pinning C to catch it would put the whole clone failure in
/// front of a French reader in English, to gain a hint. A non-English locale
/// loses that one wording instead: a candidate not offered, never a wrong one
/// offered, which is the safe direction for this to be wrong in. A host that
/// words it some fifth way loses the hint the same way, and keeps git's own
/// message.
fn repository_not_found(stderr: &str) -> Option<Failure> {
    let said = stderr.to_lowercase();
    let host_said = [
        "repository not found",
        "project you were looking for could not be found",
        "repository does not exist",
    ]
    .iter()
    .any(|phrase| said.contains(phrase));
    let git_said = said
        .lines()
        .any(|line| line.contains("repository '") && line.trim_end().ends_with("' not found"));
    (host_said || git_said).then_some(Failure::RepositoryNotFound)
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
/// Whether a `worktree remove` may carry `--force --force` — the spelling that
/// takes a forget past a lock, and nothing else: the refusal that matters (a
/// recorded path resolving to something unrelated) survives it. Its own two-arm
/// type rather than a boolean so a call site reads as the decision it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForgetForce {
    AsAsked,
    PastALock,
}

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

    /// Commits in *clone* that no remote-tracking ref contains, whichever ref
    /// they are on.
    ///
    /// **`--all`, not the checked-out branch, and that is the whole of #471.** A
    /// clone is asked what *it* holds, and it holds every ref in it: commit on
    /// `wip`, switch back to `main`, and a probe that named one branch reported the
    /// clone as safe to delete while holding the only copy of that commit.
    /// `--all` reaches every local branch, every worktree's HEAD including
    /// detached ones, and `refs/stash`.
    ///
    /// The stash is deliberate rather than incidental. It is one ref per clone,
    /// written to the clone's own `refs/stash` even from inside a linked worktree,
    /// and what it holds exists nowhere else, so it belongs on the same side of
    /// the answer as an unpushed commit.
    ///
    /// **Argument order is load-bearing.** `--not` flips the sense of every ref
    /// *after* it, so the refs being asked about have to be named before it:
    /// `log --not --remotes --all` excludes them as well and is silently always
    /// empty — which would report every clone as safe to delete.
    ///
    /// `--remotes` rather than any branch's upstream, so work pushed under another
    /// name, or merged and fetched back, is correctly not counted as lost.
    ///
    /// **`refs/tags` is the one thing `--all` reaches that is excluded, and #485
    /// is why.** A tag the remote carries but no remote *branch* reaches any more
    /// reads as unpushed, and that is the ordinary state of a repository which
    /// tags releases on branches it then deletes: one such repository carries 265
    /// commits reachable only from its tags, so six of the eight workspaces on a
    /// host refused to be deleted, reporting between 265 and 269 unpushed commits
    /// apiece, and not one commit of it was real. The first reading of that was a clone kept when it could have gone,
    /// paid for in disk against the other direction's cost of the work — but a
    /// guard that fires on every clone of a repository forever is not paying disk
    /// for safety. It teaches `--force` as the ordinary way to delete a workspace,
    /// and a habit of `--force` is exactly the clone with real work in it going.
    ///
    /// `--exclude` binds to the `--all` that follows it and drops the tags out of
    /// it alone, so every other ref `--all` reaches is still asked about: local
    /// branches, every worktree's HEAD including detached ones, and `refs/stash`.
    ///
    /// **The tags that are local come back in by name, and #487 is why.** A
    /// blanket exclusion gives up one shape of work: a commit reachable *only*
    /// from a tag typed in this clone, with no branch, worktree HEAD or stash
    /// naming it as well. Tag before a rewrite, move the branch off it, and that
    /// clone read as nothing to lose. The two cases are not distinguishable from
    /// inside the clone, because remote-tracking refs carry no tags: there is no
    /// local mark saying which of `refs/tags/*` arrived in a fetch. What knows is
    /// the bare cache the clone came from, so *local_tags* is the answer to that
    /// comparison — see [`Git::tags_in_clone`] — and each one is named as an
    /// ordinary positive ref.
    ///
    /// Naming them *after* `--all` and *before* `--not` is the same argument-order
    /// rule as above: they are refs being asked about, so they belong on the
    /// positive side. Every name is a full `refs/tags/…`, which is unambiguous
    /// against a branch of the same name and cannot be read as an option.
    ///
    /// Answers on a clone with no refs at all, where there is nothing to be
    /// unpushed: git exits 0 with no output rather than refusing, so a clone of an
    /// empty repository needs no gate here and does not get one.
    pub(crate) fn unpushed_commits(
        &self,
        clone: &Path,
        local_tags: &[String],
    ) -> GitAnswer<String> {
        let mut args: Vec<&str> = vec!["log", "--oneline", "--exclude=refs/tags/*", "--all"];
        args.extend(local_tags.iter().map(String::as_str));
        args.extend(["--not", "--remotes"]);
        self.about(clone, &args)
    }

    /// Every tag in *clone*, with the object each one names.
    ///
    /// Half of #487's comparison. The other half is [`Git::tags_in_bare`], and a
    /// tag that appears in both at the same object is a tag the remote had at the
    /// last sweep — [`Git::fetch_all`] copies `refs/tags/*` into the bare forced
    /// and pruned, and `git clone` copies them from there into the workspace, so
    /// the two agree by construction for everything that came off the remote.
    ///
    /// `%(objectname)` rather than a peeled commit, so an annotated tag is
    /// compared as the tag object it is. A fetch copies that object whole, so the
    /// two sides match; a tag *retyped* here at the same commit does not, and
    /// counts as local — which is the safe direction.
    ///
    /// Pinned to the clone by [`Git::about`] like every other question asked of a
    /// workspace, so an unusable `.git` refuses rather than answering about an
    /// ancestor repository (devlaunch#171).
    pub(crate) fn tags_in_clone(&self, clone: &Path) -> GitAnswer<Vec<TagRef>> {
        self.about(clone, &TAG_REFS_QUERY).map(tag_refs_in)
    }

    /// Of the commits [`Git::unpushed_commits`] counted, the ones *only* these
    /// local tags reach.
    ///
    /// The same ref algebra as that query with the sides swapped: the tags are the
    /// whole positive set, and everything else — the remotes and every local ref
    /// that is not a tag — is subtracted. So a commit here is on no remote, on no
    /// branch, in no worktree HEAD and in no stash, and the only thing naming it
    /// is a tag this clone's mirror does not vouch for.
    ///
    /// It is asked for the *sentence*, not for the decision: the refusal is
    /// already decided by the query above, and this says which tags to name in it.
    /// "1 unpushed commit(s)" tells someone to push or commit something they have
    /// already committed, and where the mirror is merely behind, already pushed
    /// too; "reachable only from local tag(s) (backup)" is the same fact said in
    /// a way that can be acted on.
    ///
    /// The result is a subset of the other query's by construction, which is what
    /// lets the two counts be reported in one sentence without either being a
    /// second measurement of the same thing.
    pub(crate) fn commits_only_tags_reach(
        &self,
        clone: &Path,
        local_tags: &[String],
    ) -> GitAnswer<String> {
        let mut args: Vec<&str> = vec!["log", "--oneline"];
        args.extend(local_tags.iter().map(String::as_str));
        args.extend(["--not", "--remotes", "--exclude=refs/tags/*", "--all"]);
        self.about(clone, &args)
    }

    /// Every tag in the bare cache at *bare*, with the object each one names.
    ///
    /// `--git-dir` and no work tree, because a bare has none, and no cwd, because
    /// the directory may not be there: a bare that is missing, half-removed or not
    /// a repository is `fatal: not a git repository` — a refusal — rather than git
    /// discovering some other repository from wherever dl happens to be standing.
    /// The caller reads that refusal as "nothing to compare against", which counts
    /// every tag in the clone as local.
    pub(crate) fn tags_in_bare(&self, bare: &Path) -> GitAnswer<Vec<TagRef>> {
        let mut argv = vec![format!("--git-dir={}", bare.display())];
        argv.extend(TAG_REFS_QUERY.iter().map(|arg| (*arg).to_owned()));
        let spec =
            SpawnSpec::new(Invocation::new(PROGRAM).with_args(argv)).with_timeout(ABOUT_ONE_REPO);
        self.captured("for-each-ref", &spec)
            .map(|stdout| tag_refs_in(stdout.trim_end_matches('\n').to_owned()))
    }

    // --------------------------------------------- agent worktrees (#426, #454)

    /// Every worktree registered in *clone*, as `worktree list --porcelain`
    /// writes it: the clone's own entry first, then one paragraph per linked
    /// worktree carrying its `branch` or `detached`, and `locked` where git says
    /// so.
    ///
    /// **The registered paths are not necessarily paths on this machine.** An
    /// agent harness running inside a devcontainer registers its worktrees at the
    /// container's `/workspaces/<id>/…`, and the very same directories are
    /// reached from the host through the clone. So this output is read for what
    /// git records about each registration, and a registration is matched to a
    /// directory by its place inside the clone rather than by resolving the path
    /// git prints, which on a host resolves to nothing (devlaunch#426).
    pub(crate) fn worktree_listing(&self, clone: &Path) -> GitAnswer<String> {
        self.about(clone, &["worktree", "list", "--porcelain"])
    }

    /// Drop exactly one registration, by the path the listing printed.
    ///
    /// This is a supported single-registration prune, and the reason it works
    /// from a host is the opposite of what an earlier build wrote here: the
    /// recorded path **does not resolve**, so git skips its working-tree
    /// validation and its dirty check and drops the one name. Measured on git
    /// 2.51.1 and 2.43.0 (devlaunch#448, re-measured on #445): a recorded path
    /// that resolves to something unrelated is refused, and `-f -f` does not get
    /// past that refusal — the one direction git holds on its own.
    ///
    /// Because the dirty check sits inside git's own `file_exists(wt->path)`, a
    /// non-resolving path is dropped with **no safety check at all** — exit 0,
    /// silent, whatever the host directory holds. The caller's verdict is the
    /// entirety of the protection, which is why the caller only ever forgets a
    /// name after the directory it accounted for is gone.
    ///
    /// `--force` twice is what carries a forget past a lock, and it is passed
    /// only when the caller's plan carried that insistence for this unit.
    ///
    /// There is deliberately no `worktree prune` beside this. Its domain is a
    /// readdir over `$GIT_DIR/worktrees` at act time — registrations created
    /// after a plan was printed included — so no plan can name its blast radius,
    /// and devlaunch deleted it rather than gating it (devlaunch#445).
    pub(crate) fn worktree_remove(
        &self,
        clone: &Path,
        recorded: &Path,
        force: ForgetForce,
    ) -> GitAnswer<String> {
        let recorded = recorded.display().to_string();
        let mut args = vec!["worktree", "remove"];
        if let ForgetForce::PastALock = force {
            args.extend(["--force", "--force"]);
        }
        args.push(&recorded);
        self.about(clone, &args)
    }

    /// The porcelain status of one linked worktree, asked through the clone's
    /// admin directory for it.
    ///
    /// **Not [`Git::about`], and the difference is what makes this answerable at
    /// all.** A linked worktree's own `.git` is a gitfile, and for an agent
    /// worktree that gitfile names a path inside a container. Pointing
    /// `--git-dir` at it on a host resolves nothing and git refuses — so a dirty
    /// check that went the ordinary way would report every one of these as
    /// unreadable. `--git-dir=<clone>/.git/worktrees/<name>` is the same
    /// repository reached from the side that does resolve here, and
    /// `--work-tree` is the directory on this host.
    ///
    /// **`--ignored` is deliberately not passed, and that is a decision rather
    /// than an omission.** This asks the same question [`Git::status_porcelain`]
    /// asks of a whole clone, in the same words, so there is one definition of
    /// what makes a tree dirty rather than two that disagree about the same
    /// bytes. A clone's own ignored content has never counted — `dl <ws> rm` and
    /// `--prune`'s orphan arm both `rm -rf` past it — and counting it one level
    /// in but not at the root is the same rule written twice.
    ///
    /// It was passed once, and the cost is worth recording so nobody re-adds it
    /// as thoroughness: an installed `.pixi/envs/default` is ignored content, it
    /// is what makes an agent worktree worth reclaiming at all, and weighing it
    /// put every such directory behind `--force-worktrees` — the one flag that
    /// also carries past a lock and past another repository's worktree. That
    /// trades the whole of this sweep's yield for a habit of typing the flag
    /// that switches its protections off. Whether ignored bytes should be
    /// weighed is a real question, and it is **one question for both scopes**;
    /// `docs/cleanup.md` records it as a limit with its reason.
    ///
    /// Every line git prints comes back, nested agent worktrees included. Which
    /// entries to disregard is a question about what a directory *is*, which is
    /// `flows::agent_worktrees`'s question and not a git client's.
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

    /// Commits reachable from *rev* in *clone* that no remote-tracking ref
    /// contains — the per-revision spelling of [`Git::unpushed_commits`], for a
    /// question that has to attribute its answer to one worktree's checkout
    /// rather than to the clone as a whole.
    pub(crate) fn unpushed_commits_from(&self, clone: &Path, rev: &str) -> GitAnswer<String> {
        self.about(clone, &["log", "--oneline", rev, "--not", "--remotes"])
    }

    /// How many commits are reachable from *rev* and from no ref in *repo*.
    ///
    /// Asked of the sibling bare cache, which is the repository devlaunch
    /// actually fetches into, so `--all` there means "everything the forge had
    /// at the last fetch". `0` is therefore the one answer that says a commit is
    /// safely somewhere else.
    ///
    /// A refusal is usually `bad object`: the cache has never seen the commit,
    /// which is what an unpushed branch looks like from over there. The caller
    /// reads it that way and asks the clone as well rather than treating it as
    /// an error.
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
        self.captured_reading(
            "clone",
            repository_not_found,
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
    ///
    /// Both refspecs are written out and both are forced, and the tags one is
    /// spelled rather than left to `--tags`, which is the same refspec *unforced*.
    /// `--prune` prunes per refspec, so that one word cost two things at once: a
    /// tag the remote had retracted was never pruned, leaving `refs/tags` monotone
    /// in every tag the remote ever advertised and pinning every object those tags
    /// reach for the life of the cache; and a tag the remote *moved* was rejected
    /// with `would clobber existing tag`, which fails the whole fetch — the heads
    /// in the same push included — and keeps failing, since nothing here ever
    /// resolves it. That made one moved tag upstream a permanently `Refused`
    /// freshness fetch for that repository until a human deleted the local tag by
    /// hand.
    ///
    /// Pruning a tag is a deletion, so it is worth saying which reading licenses
    /// it: a tag in the bare is a *copy* of an upstream ref and holds no work that
    /// exists nowhere else, which is the argument that already licenses pruning
    /// heads. Nothing here is a place work is authored.
    ///
    /// One consequence of pruning heads is older than this and is recorded rather
    /// than handled: when the remote's own default branch disappears, `--prune`
    /// takes `refs/heads/main` with it and the bare's `HEAD` symref is left
    /// dangling. `symbolic-ref HEAD` still answers `refs/heads/main`, so
    /// default-branch detection still returns a name and does not notice. Nor
    /// does a bare `rev-parse HEAD`, which exits 0 printing the literal `HEAD`;
    /// it takes `rev-parse --verify HEAD` to get a failure out of it, which is
    /// why nothing here reports one. What an eye actually meets is `git clone`
    /// warning `remote HEAD refers to nonexistent ref, unable to checkout`. The
    /// symptom is a launch aimed at a branch the cache no longer has, on a
    /// repository whose default branch was deleted upstream.
    pub(crate) fn fetch_all(&self, bare: &Path, limit: Option<Duration>) -> GitAnswer<String> {
        let mut spec = SpawnSpec::new(
            Invocation::new(PROGRAM)
                .with_args([
                    "fetch",
                    "origin",
                    "+refs/heads/*:refs/heads/*",
                    "+refs/tags/*:refs/tags/*",
                    "--prune",
                ])
                .with_cwd(bare.to_path_buf()),
        );
        spec.timeout = limit;
        self.captured("fetch", &spec)
    }

    /// Collapse every loose ref in the bare into `packed-refs`.
    ///
    /// A ref git writes as a file costs a whole filesystem block, typically 4096
    /// bytes against the ~81 the packed line takes, and [`Git::fetch_all`] writes
    /// one per ref it updates. Nothing else in devlaunch ever packed them:
    /// `pack-refs --auto` is a documented no-op on the `files` backend, and no
    /// `gc` is run on the bare, so `gc.auto` never gets the chance either.
    /// Measured on git 2.51.1 over a bare with 301 loose refs of 551, the loose
    /// files held 1204 KiB of blocks against a 30 KiB `packed-refs` for all 551,
    /// and the pack took 23 ms.
    ///
    /// `--all` rather than the default, and the difference is the whole verb here
    /// rather than a nicety: measured on 2.51.1 against a bare holding 301 loose
    /// heads and 101 loose tags, a bare `pack-refs` took the tags to zero and left
    /// all 301 heads exactly where they were. Heads are the population a broad
    /// sweep of a real repository mostly makes.
    ///
    /// **Pure, in the sense that decides where this is allowed to live.** Packing
    /// changes how a ref is stored and not which refs exist, so it can lose no
    /// work — and it does not change what a later `--prune` may delete either.
    /// Measured on 2.51.1: a prune of a packed ref rewrites `packed-refs` through
    /// the same ref transaction, deletes nothing else, and a ref that was loose
    /// over a stale packed line loses *both*, so nothing is resurrected at the old
    /// sha. The only difference is cost, and it falls on the prune: removing a
    /// packed ref rewrites the whole file where removing a loose one unlinks
    /// a single path.
    ///
    /// Bounded at [`ABOUT_ONE_REPO`] rather than left unbounded like the sweep's
    /// fetch: this touches no network, so a pack that has not finished in thirty
    /// seconds is a stuck filesystem rather than a slow remote, and the caller it
    /// runs under is a detached child whose whole point is that nobody is watching
    /// it.
    pub(crate) fn pack_refs(&self, bare: &Path) -> GitAnswer<String> {
        self.captured(
            "pack-refs",
            &SpawnSpec::new(
                Invocation::new(PROGRAM)
                    .with_args(["pack-refs", "--all"])
                    .with_cwd(bare.to_path_buf()),
            )
            .with_timeout(ABOUT_ONE_REPO),
        )
    }

    /// Fetch exactly one branch into the bare cache.
    ///
    /// The launch path's entire network budget, so the time it can hold the repo
    /// lock is bounded by one branch's objects rather than by the repository's
    /// whole history of branches.
    ///
    /// The C locale is pinned because [`ref_missing_on_remote`] reads git's stderr
    /// here — `couldn't find remote ref` is the one non-zero exit that is an
    /// *answer* — and git translates that text, so a German host would collapse a
    /// three-way outcome to two. `LANGUAGE` is pinned as well: under gettext it
    /// outranks a non-C `LC_ALL`, and the guarantee should not hang on the one
    /// glibc rule that exempts C.
    pub(crate) fn fetch_ref(&self, bare: &Path, branch: &str) -> GitAnswer<String> {
        self.captured_reading(
            "fetch",
            ref_missing_on_remote,
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
    /// The C locale is pinned for the same reason as [`Git::fetch_ref`]'s:
    /// [`branch_already_exists`] reads git's stderr here and the caller swallows
    /// that arm, so on a translated host an ordinary re-launch of a branch that
    /// is already there would raise instead.
    pub(crate) fn create_branch(
        &self,
        repo: &Path,
        branch: &str,
        start_point: &str,
    ) -> GitAnswer<String> {
        self.captured_reading(
            "branch",
            branch_already_exists,
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

    /// Run a verb whose output is the answer, and whose words nothing decides on.
    fn captured(&self, named: &str, spec: &SpawnSpec) -> GitAnswer<String> {
        self.captured_reading(named, reads_nothing, spec)
    }

    /// Run a verb whose stderr carries a fact devlaunch acts on, and read it.
    ///
    /// The three callers are the three verbs a decision hangs off. Naming the
    /// reader at the call site is what keeps a phrase scoped to the verb that
    /// says it: `already exists` means the branch to `git branch`, and the
    /// destination directory to `git clone`.
    fn captured_reading(
        &self,
        named: &str,
        reading: Reading,
        spec: &SpawnSpec,
    ) -> GitAnswer<String> {
        answered(named, spec.timeout, reading, self.runner.capture(spec))
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
    reading: Reading,
    outcome: Outcome<CapturedText>,
) -> GitAnswer<String> {
    match outcome {
        Outcome::Ran { exit, io } if exit.is_success() => GitAnswer::Said(io.stdout),
        Outcome::Ran { exit, io } => {
            GitAnswer::Refused(GitRefused::exited(named, exit, &io.stderr, reading))
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

/// The environment for the two verbs whose failure is read from words git
/// *translates*. See [`Git::fetch_ref`], and [`repository_not_found`] for why the
/// third reading verb inherits the environment instead.
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

/// One tag, as the ref it is: the full refname and the object that ref names.
///
/// A pair rather than a name alone, because #487's question is not "does the bare
/// have a tag of this name" but "does it have *this* tag": a name the two sides
/// hold at different objects is a tag that was moved or retyped here, and what it
/// used to reach may exist nowhere else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TagRef {
    /// The full refname, `refs/tags/v1` — never the short name, so it is
    /// unambiguous as a revision argument and can never read as an option.
    pub(crate) name: String,
    pub(crate) object: String,
}

/// The tags in [`TAG_REFS_QUERY`] output, one per `<object> <refname>` line.
///
/// A line in any other shape is dropped rather than guessed at, and a dropped
/// line costs nothing here: the tag it named is then absent from *both* readings
/// where the sides agree, and absent from the bare's alone where they do not,
/// which counts the clone's tag as local. Both are the safe direction.
fn tag_refs_in(output: String) -> Vec<TagRef> {
    output
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(object, name)| !object.is_empty() && name.starts_with("refs/tags/"))
        .map(|(object, name)| TagRef {
            name: name.to_owned(),
            object: object.to_owned(),
        })
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
