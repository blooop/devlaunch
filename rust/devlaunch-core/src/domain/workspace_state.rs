//! What a workspace holds — the facts a cleanup decision is made from elsewhere.
//!
//! A workspace per branch means workspaces accumulate, and something has to
//! remove the finished ones. That something is **not devlaunch**: whether a piece
//! of work is finished is a fact about a ticket, a review or a person's intent,
//! and dl knows about none of those. It knows about clones and containers.
//!
//! So the split is mechanism here, policy in the caller:
//!
//! - `dl --ls --json` reports what exists and what each workspace holds, which is
//!   what a caller needs to decide anything at all.
//! - `dl <ws> rm` deletes one, and refuses when the clone holds work that exists
//!   nowhere else.
//!
//! The refusal is the one judgement dl does make, and it is not a policy about
//! finished work: it is dl declining to destroy the only copy of something. A
//! caller that means it says `--force`.
//!
//! The alternative — dl inferring "finished" from the branch (merged into the
//! default, or deleted from the remote) — was built first and thrown away. It
//! reads as a git fact but it is a guess at intent: a squash-merged branch and an
//! abandoned one are indistinguishable, a branch merged upstream may still have
//! work to do, and a repository whose flow does not delete branches gets nothing.
//! The caller that knows the answer should say the answer.
//!
//! # Three answers, not two (devlaunch#171)
//!
//! This module used to answer "a description of what would be lost, or nothing".
//! *Nothing* carried two meanings — "nothing would be lost" and "I could not find
//! out" — and the destructive caller read both as permission. That is not a
//! hypothetical conflation; it shipped, and it destroyed the wrong thing:
//!
//! git was run with a working directory and nothing else — no `--git-dir`, no
//! `--work-tree`, no ceiling — so git's repository **discovery walked up the
//! parent chain**. A clone whose `.git` was unusable — truncated, half-removed by
//! an interrupted delete, or never finished — did not make git refuse. It made
//! git find an *ancestor* repository and answer confidently about that one. With
//! dl's cache under `$XDG_CACHE_HOME` and a dotfiles repository in `$HOME`, that
//! ancestor is common; when it was clean and fully pushed, the guard was told
//! nothing would be lost for a clone holding untracked work, and `dl <ws> rm`
//! deleted it without so much as asking for `--force`. The failure needed a
//! *tidy* host to appear, because a dirty ancestor made the guard fire — for the
//! wrong reason, about the wrong repository — and hid it.
//!
//! Both halves are fixed, and neither is sufficient alone:
//!
//! 1. Every git command names its repository explicitly
//!    (`Git::about`), so discovery is switched off and an
//!    unusable `.git` produces a refusal.
//! 2. A refusal has somewhere to go: [`Unsaved`] is a total sum —
//!    [`Unsaved::NothingToLose`], [`Unsaved::WouldLose`],
//!    [`Unsaved::CouldNotTell`] — and every caller must name the arm it is
//!    handling. "Could not tell" refuses a delete exactly as "would lose" does.
//!
//! # What the port changed, and what it deliberately did not
//!
//! Python needed two runtime guards that Rust's types make unnecessary, and
//! neither is a behaviour that was dropped:
//!
//! - `unhandled_unsaved()` raised on an arm nobody handled, because Python's
//!   `match` falls off the end. A `match` on [`Unsaved`] here is exhaustive at
//!   compile time, so a fourth arm added later breaks every reader rather than
//!   being read as permission to delete — which is what the guard was for.
//! - `WouldLose.__post_init__` raised on an empty description, because a
//!   description was a string a caller could get wrong. Here a `WouldLose`
//!   carries `Losses`, which cannot be empty by construction, and the
//!   description is *derived* from it — so the illegal value has no
//!   representation rather than a check.
//!
//! Ported from `devlaunch/workspace_state.py`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::clients::git::{Git, GitAnswer, LocalBranch, REFS_HEADS, RemoteRef, TagRef, TextMerge};

/// The bare cache a clone was made from, when there is one to consult.
///
/// The one thing on the host that can say which of a clone's `refs/tags/*` came
/// off the remote: dl's mirror fetches `+refs/tags/*:refs/tags/*` forced and
/// pruned, `git clone` copies those tags into the workspace, so a tag in both at
/// the same object is a tag the remote had at the last sweep. A tag the bare has
/// not got is one somebody typed here.
///
/// Two arms rather than an `Option<&Path>`, because the absent case is an
/// *answer* with a direction and not a missing argument. [`Self::Unknown`] counts
/// every tag in the clone as local, which is the fail-towards-keeping side: a
/// clone kept costs disk, and the other way costs the only copy of somebody's
/// work. Every caller that cannot name a bare has to write the word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BareCache<'a> {
    /// dl's mirror of this clone's repository is at this path.
    At(&'a Path),
    /// There is no mirror to compare against — no record, an unusable one, or a
    /// clone dl did not make.
    Unknown,
}

impl<'a> BareCache<'a> {
    /// The one conversion from "a path, or not" — the shape every resolver
    /// answers in — so the direction the absent case fails in is decided here
    /// rather than at each call site.
    pub(crate) fn of(bare: Option<&'a Path>) -> Self {
        match bare {
            Some(path) => Self::At(path),
            None => Self::Unknown,
        }
    }
}

/// What deleting a clone would destroy, as far as git can be made to say.
///
/// Three arms and no fourth. "The directory is not there" is not a fourth: it is
/// [`Unsaved::NothingToLose`], because there is no work in it to lose — the same
/// answer `disk_usage` gives such a directory (`Measured(0)`), and what lets a
/// caller clear away a workspace whose clone was already removed by hand.
#[derive(Clone, Debug, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub enum Unsaved {
    /// Everything in the clone exists somewhere else. Deleting it costs nothing.
    ///
    /// Carries no payload on purpose: there is nothing to say. It is an arm of
    /// its own rather than an absent value so that a caller cannot reach it by
    /// accident — the whole of devlaunch#171 was a caller reaching it by
    /// accident.
    NothingToLose,
    /// Deleting the clone would destroy this.
    ///
    /// `Losses` rather than a flag, and that is load-bearing: what it
    /// [`describes`](Losses::describe) is printed to a person deciding whether to
    /// force the delete, and "3 uncommitted change(s) (pixi.lock, notes.md, …)
    /// and 2 unpushed commit(s)" is the thing that answers them. See
    /// [`name_a_few`] for why a count alone is not enough.
    WouldLose(Losses),
    /// git could not be asked about this clone, and this is what it said.
    ///
    /// Not a failure to report — a report. It is the answer for a directory that
    /// is there but is not a repository git can read, and it must stop a delete
    /// for the same reason [`Unsaved::WouldLose`] does: the files are still on
    /// disk, and nothing has established that they exist anywhere else.
    ///
    /// The cause is carried rather than reconstructed at the point of printing,
    /// because it is not guessable from the path: an interrupted delete, a
    /// `.git` written by a container as another user, a truncated gitfile and a
    /// directory that was never a clone all arrive here and read differently to
    /// the person who has to decide what to do about it. It names the directory
    /// it is about, which is the specific thing the shipped bug got wrong.
    CouldNotTell(CouldNotTell),
}

// There is deliberately no `may_delete()` here. Two of the three arms refuse a
// delete, and a bool saying so is the sentinel this module exists to not have:
// the guard has to name the arm anyway — it prints what would be lost, or what
// could not be told — so a `match` costs it nothing and a fourth arm added later
// breaks it rather than being read as permission.

/// Why git could not be asked, one arm per cause.
///
/// One `reason: String` held all four before, which collapsed causes that read
/// differently to the person deciding what to do — and nothing but convention
/// kept it non-empty or naming the directory it was about, which is the specific
/// thing the shipped bug got wrong. Each arm here carries the directory (or the
/// workspace, for the one cause with no directory to name) plus the words the
/// failure came with, and [`CouldNotTell::describe`] is derived from them.
#[derive(Clone, Debug, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub enum CouldNotTell {
    /// dl was stopped before it could look at the directory at all; `error` is
    /// the OS's words.
    CouldNotLook { clone: PathBuf, error: String },
    /// git could not read the directory as a repository; `reason` is git's words.
    GitCouldNotRead { clone: PathBuf, reason: String },
    /// The repository read fine, and listing unpushed commits then refused —
    /// broken remote-tracking refs, usually.
    ///
    /// Names no branch, because since #471 the question names none: it is asked of
    /// every ref in the clone at once. A branch here would be the checked-out one
    /// standing in for a question it was not about, which is the shape of thing
    /// this module keeps deleting.
    UnpushedNotListed { clone: PathBuf, reason: String },
    /// No directory could be named for this workspace's record at all: the
    /// recorded path is unusable and the derivation refused the record's own
    /// triple (the `dl <ws> rm` guard's arm — see
    /// [`crate::flows::listing::unsaved_work_in`]).
    DirectoryUnknown { workspace_id: String },
}

impl CouldNotTell {
    /// Python's phrasing, exactly: this text reaches a person through
    /// `dl <ws> rm`'s refusal and a tool through `--ls --json`'s `couldNotTell`,
    /// so it is a wire payload and not rendering — [`Losses::describe`]'s rule.
    /// Every arm opens with words of its own, so it cannot read as empty.
    ///
    /// binary surface — not part of the frozen wf API (#251 §7)
    pub fn describe(&self) -> String {
        match self {
            Self::CouldNotLook { clone, error } => {
                format!("could not look at {}: {error}", clone.display())
            }
            Self::GitCouldNotRead { clone, reason } => {
                format!("git could not read {}: {reason}", clone.display())
            }
            Self::UnpushedNotListed { clone, reason } => format!(
                "git could not list unpushed commits in {}: {reason}",
                clone.display()
            ),
            Self::DirectoryUnknown { workspace_id } => {
                format!("could not work out which directory {workspace_id}'s clone is in")
            }
        }
    }
}

/// What a clone holds that exists nowhere else: at least one thing, always.
pub(crate) type Losses = NonEmpty<Loss>;

/// One kind of loss.
///
/// Two arms because someone deciding whether to force a delete wants both, and
/// each carries the lines git printed rather than a count — the count is derived
/// from them, and so are the names, which is what makes a description
/// unforgeable.
#[derive(Clone, Debug, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub enum Loss {
    /// A dirty tree, **untracked files included** — an agent's scratch notes are
    /// not less lost for never having been added. One `git status --porcelain`
    /// line per changed path.
    Uncommitted(NonEmpty<String>),
    /// Commits no remote-tracking ref contains, one `git log --oneline` line
    /// each.
    Unpushed {
        commits: NonEmpty<String>,
        /// The share of those commits that nothing but a local tag reaches, when
        /// there is such a share.
        ///
        /// `None` is "no tag is why this clone is being kept", and it is the
        /// ordinary case: an unpushed commit on a branch needs no explaining,
        /// because the sentence the reader gets already tells them what to do
        /// about it. `Some` is the case where that sentence is wrong, so the
        /// answer carries what makes it right instead of the reader guessing.
        by_tags: Option<ByLocalTags>,
    },
}

/// The local tags an unpushed count is owed to, and the commits owed to them.
///
/// Both halves are [`NonEmpty`], so this value cannot say "some tags reached no
/// commits" or "some commits were reached by no tags": it exists exactly when a
/// tag this clone's mirror does not vouch for is the only thing naming a commit,
/// which is the one case where "push or commit it" is advice the reader cannot
/// act on. Where nothing is owed to a tag there is no value, not an empty one.
///
/// Two shapes reach it and the sentence does not distinguish them, deliberately.
/// The mirror has never heard of the tag (#487's own case: tagged here, never
/// pushed) and the mirror is simply behind the remote (the tag was pushed from
/// this clone and no sweep has run since). Naming the tag settles both without
/// this having to know which: a reader who recognises `v0.26.0` as a release they
/// pushed learns the cache is stale, and a reader who sees `backup-before-rebase`
/// learns the thing that saves their work.
#[derive(Clone, Debug, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub struct ByLocalTags {
    /// Short tag names, `backup` rather than `refs/tags/backup`, because this is
    /// what a person types and reads.
    tags: NonEmpty<String>,
    /// The `git log --oneline` lines only those tags reach.
    commits: NonEmpty<String>,
}

impl ByLocalTags {
    /// The attribution, or nothing when no commit is owed to a tag.
    ///
    /// Takes both sides through [`NonEmpty::of`], so the empty case is the absent
    /// value rather than a value that describes nothing. *tags* arrive as full
    /// refnames, which is what the query names them by, and are shortened here so
    /// there is one place that knows the prefix.
    pub(crate) fn of(
        tags: impl IntoIterator<Item = String>,
        commits: impl IntoIterator<Item = String>,
    ) -> Option<Self> {
        Some(Self {
            tags: NonEmpty::of(tags.into_iter().map(|reference| {
                reference
                    .strip_prefix(REFS_TAGS)
                    .unwrap_or(&reference)
                    .to_owned()
            }))?,
            commits: NonEmpty::of(commits)?,
        })
    }

    /// The attribution without the commits a remote already holds a copy of,
    /// or nothing when that leaves none.
    ///
    /// So the tag share stays a share of the count it is printed beside.
    fn leaving_out(self, copied: &[String]) -> Option<Self> {
        Some(Self {
            commits: NonEmpty::of(not_among(self.commits.iter(), copied))?,
            tags: self.tags,
        })
    }
}

/// The prefix every tag refname carries, stripped for display.
const REFS_TAGS: &str = "refs/tags/";

impl Loss {
    /// Python's phrasing, exactly: this text reaches a person through
    /// `dl <ws> rm`'s refusal and a tool through `--ls --json`'s `wouldLose`, so
    /// it is a wire payload and not rendering.
    fn describe(&self) -> String {
        match self {
            Self::Uncommitted(changed) => format!(
                "{} uncommitted change(s) ({})",
                changed.len(),
                name_a_few(changed, NAME_AT_MOST)
            ),
            Self::Unpushed {
                commits,
                by_tags: None,
            } => format!("{} unpushed commit(s)", commits.len()),
            // Both counts, because they are different facts and the smaller one is
            // the actionable half: it says how much of the refusal "push it" cannot
            // clear. Where every unpushed commit is a tag's, the two numbers agree
            // and the sentence is merely emphatic rather than wrong.
            Self::Unpushed {
                commits,
                by_tags: Some(by_tags),
            } => format!(
                "{} unpushed commit(s), {} reachable only from local tag(s) ({})",
                commits.len(),
                by_tags.commits.len(),
                first_few(&by_tags.tags, NAME_AT_MOST),
            ),
        }
    }
}

impl Losses {
    /// The whole loss in words, the two kinds joined as Python joins them.
    ///
    /// binary surface — not part of the frozen wf API (#251 §7)
    pub fn describe(&self) -> String {
        self.iter()
            .map(Loss::describe)
            .collect::<Vec<_>>()
            .join(" and ")
    }
}

/// How many changed paths are named before the list is cut short.
const NAME_AT_MOST: usize = 3;

/// A sequence that has at least one element, because the empty case is a
/// different answer rather than a degenerate one.
///
/// Built from the positive space: an element and the rest, so "at least one" is
/// the type rather than a check. [`NonEmpty::of`] is the only way in from a
/// `Vec`, and it answers `None` for the empty one — which is how the empty status
/// output becomes [`Unsaved::NothingToLose`] instead of a `WouldLose` with
/// nothing to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NonEmpty<T> {
    first: T,
    rest: Vec<T>,
}

impl<T> NonEmpty<T> {
    pub(crate) fn one(first: T) -> Self {
        Self {
            first,
            rest: Vec::new(),
        }
    }

    /// The sequence, or nothing when there was nothing in it.
    pub fn of(items: impl IntoIterator<Item = T>) -> Option<Self> {
        let mut items = items.into_iter();
        let first = items.next()?;
        Some(Self {
            first,
            rest: items.collect(),
        })
    }

    /// The item the sequence always holds.
    pub fn first(&self) -> &T {
        &self.first
    }

    pub(crate) fn len(&self) -> usize {
        1 + self.rest.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        std::iter::once(&self.first).chain(self.rest.iter())
    }

    pub(crate) fn map<U>(&self, mut f: impl FnMut(&T) -> U) -> NonEmpty<U> {
        NonEmpty {
            first: f(&self.first),
            rest: self.rest.iter().map(f).collect(),
        }
    }
}

/// What one workspace clone holds, as far as git can tell.
///
/// `branch` is what the clone has checked out, or `None` when git could not say —
/// an unusable `.git`, or an unborn HEAD.
///
/// The two fields are independent, and an earlier draft of this doc said
/// otherwise ("`None` in every case where `unsaved` is a `CouldNotTell`"). A
/// clone git *can* read as a repository but whose remote-tracking refs are broken
/// gives `branch: Some("feature")` with `unsaved: CouldNotTell(…)`:
/// `git status` answered, so the branch is known, and only the later
/// `git log … --not --remotes` refused. That is a shape the tests build, and the
/// behaviour is right — it was the invariant that was wrong.
///
/// `branch` is reported beside the *recorded* branch rather than instead of it
/// (`dl --ls --json` prints both), so a clone an agent moved off its branch is
/// visible as such.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CloneState {
    pub(crate) branch: Option<String>,
    pub(crate) unsaved: Unsaved,
    /// The unpushed commits the count's rules cleared, as full hashes
    /// ([`cleared_by_the_rules`]).
    ///
    /// For the agent worktree lines of the same verdict, which take these out
    /// of their own listings: they then clear what `unsaved` cleared, and the
    /// rules run once. Empty wherever the rules cleared nothing, and wherever
    /// git refused before they ran, so a worktree line then stays as git
    /// listed it.
    pub(crate) cleared: Vec<String>,
}

/// Report what *clone* holds. The only function here that talks to git.
///
/// A directory that is **not there** holds nothing: there is no work in it to
/// lose. That is the truth about it rather than a special case, and it is what
/// lets a caller clear away a workspace whose clone was already removed by hand.
///
/// A directory that *is* there and that git cannot read as a repository is a
/// different answer, and used to be given the same one. It holds whatever files
/// are in it, and with no repository to consult nothing has established that they
/// exist anywhere else — so it is a [`Unsaved::CouldNotTell`], and the delete
/// stops. See this module's docs for what that cost before it did.
///
/// A directory dl cannot even *look at* is a third answer, and this is why the
/// `metadata` call below is written out rather than left as an `is_dir()`-shaped
/// question. Python's `Path.is_dir()` collapses every failure into `False`, and
/// it did not even do that consistently across the versions it supported: up to
/// and including 3.13 it swallowed ENOENT, ENOTDIR, EBADF and ELOOP and
/// *re-raised* the rest, so a clone whose parent is mode `000` raised
/// `PermissionError` straight out of here (`dl <ws> rm` failed closed by crashing;
/// `dl --ls --json` became a traceback for the whole listing because of one
/// workspace). On 3.14 the same call returned `False` instead, which read as "not
/// there, so nothing to lose" — a clone that may be full of work, reported as
/// free to delete, because dl was not allowed to look. One sentinel each way,
/// from the same expression.
///
/// [`std::fs::metadata`] hands over the [`std::io::ErrorKind`] instead, and the
/// three arms are read from it: ENOENT and ENOTDIR mean there is no directory
/// there to hold anything, and everything else means dl was stopped before it
/// could find out, which is exactly a [`Unsaved::CouldNotTell`].
///
/// A path with a NUL byte in it — which a hand-edited or truncated
/// `metadata.json` can put in a record — lands in that same arm. In Python it was
/// a `ValueError` raised before the syscall and had to be caught alongside
/// `OSError`; here it is an `InvalidInput` error from the same call, which is one
/// fewer thing to remember. Uncaught, either takes down the whole of
/// `dl --ls --json` for one bad record, which is the exact harm this guard exists
/// to stop.
///
/// *bare* is dl's mirror of the repository this clone came from, and it is here
/// for one question only: which of the clone's tags arrived in a fetch (#487).
/// See [`BareCache`] for why not naming one is safe and what it costs.
pub(crate) fn read_clone(git: &Git<'_>, clone: &Path, bare: BareCache<'_>) -> CloneState {
    let present = match std::fs::metadata(clone) {
        Ok(metadata) => metadata.is_dir(),
        // No directory there, so nothing in it to lose. ENOTDIR is a parent
        // component that is a file, which is equally "no clone at that path".
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return CloneState {
                branch: None,
                unsaved: Unsaved::NothingToLose,
                cleared: Vec::new(),
            };
        }
        Err(error) => {
            return CloneState {
                branch: None,
                unsaved: Unsaved::CouldNotTell(CouldNotTell::CouldNotLook {
                    clone: clone.to_path_buf(),
                    error: error.to_string(),
                }),
                cleared: Vec::new(),
            };
        }
    };
    if !present {
        return CloneState {
            branch: None,
            unsaved: Unsaved::NothingToLose,
            cleared: Vec::new(),
        };
    }
    // Empty means git answered without naming a branch, which is not a branch;
    // a refusal is not one either. Both are `None`, and neither is the
    // ancestor's branch the shipped bug reported here.
    let branch = match git.head_branch(clone) {
        GitAnswer::Said(head) if !head.is_empty() => Some(head),
        _ => None,
    };
    let (unsaved, cleared) = unsaved(git, clone, bare);
    CloneState {
        branch,
        unsaved,
        cleared,
    }
}

/// What would be lost by deleting *clone*, as far as git can be made to say.
///
/// The clone's own count, without the agent worktree lines that the guard
/// `dl <ws> rm` adds to it (`flows::agent_worktrees::clone_verdict`, which reads
/// [`read_clone`] so that both halves share [`CloneState::cleared`]). Kept for
/// the tests that pin the clone's own count. Total — every path returns one of
/// the three arms, and none of them means "go ahead" by default.
#[cfg(test)]
pub(crate) fn holds_unsaved_work(git: &Git<'_>, clone: &Path, bare: BareCache<'_>) -> Unsaved {
    read_clone(git, clone, bare).unsaved
}

/// Name the first few changed paths from `git status --porcelain` lines.
///
/// A count alone is not enough to decide anything with. A devcontainer that runs
/// `pixi install` in its `postCreateCommand` leaves the tracked lockfile modified
/// in *every* workspace it builds, so "1 uncommitted change(s)" is the permanent
/// state of an otherwise untouched clone — and a person told only the count has no
/// way to tell that from an hour of unsaved work. Told the name, they can.
///
/// The porcelain format is two status characters, a space, then the path, so the
/// path starts at offset 3; a rename reads `old -> new`, and the whole field is
/// kept rather than split, because both halves are the news.
fn name_a_few(changed: &NonEmpty<String>, limit: usize) -> String {
    let named = changed.iter().filter_map(|line| path_in(line));
    // Counted from the porcelain lines rather than from `named`, so a line too
    // short to hold a path still counts towards "there are more than these".
    cut_short(named, changed.len(), limit)
}

/// The first few of *names*, with an ellipsis when there were more.
///
/// The tag half of the same rule [`name_a_few`] applies to changed paths, sharing
/// its one implementation: two truncation rules that drifted would be two
/// sentences claiming to elide the same way.
fn first_few(names: &NonEmpty<String>, limit: usize) -> String {
    cut_short(names.iter().map(String::as_str), names.len(), limit)
}

/// *limit* of the names, then `…` when *total* is larger.
fn cut_short<'a>(names: impl Iterator<Item = &'a str>, total: usize, limit: usize) -> String {
    let mut kept: Vec<&str> = names.take(limit).collect();
    if total > limit {
        kept.push("…");
    }
    kept.join(", ")
}

/// The path field of one porcelain line, or nothing when the line is too short to
/// have one.
///
/// Counted in characters rather than bytes, as Python counts it: the status
/// columns are ASCII but a path need not be, and slicing a byte offset into the
/// middle of one would be a panic where Python had an answer.
fn path_in(line: &str) -> Option<&str> {
    let (offset, _) = line.char_indices().nth(3)?;
    Some(line[offset..].trim())
}

/// The directory devpod writes the build files into, for a devcontainer that has
/// `features`. It sits in the build context, which is the config's directory only
/// by default: `"context": ".."` puts it at the clone root. devpod removes it when
/// the build ends, so a clone keeps it only after a build that was interrupted.
const DEVPOD_INTERNAL: &str = ".devpod-internal";

/// Whether one `git status --porcelain` line is a file devpod wrote into the
/// clone, which is not anybody's work (devlaunch#707).
///
/// Two things, both untracked:
///
/// - `.devpod-internal/`, wherever the build context put it, where devpod writes
///   `Dockerfile-with-features` and the features' env files when it builds a
///   config with `features`. Also a directory git reports whole, when all it
///   holds is `.devpod-internal`.
/// - `.devcontainer.json` at the root, which devpod saves for its fallback image
///   in a repository with no devcontainer config. Only while it holds devpod's
///   one key, `{"image": "..."}`: a file that says anything more is an edit, and
///   kept. A person who wrote exactly that line loses one line anybody can retype.
fn written_by_devpod(clone: &Path, line: &str) -> bool {
    if !line.starts_with("?? ") {
        return false;
    }
    let Some(path) = path_in(line) else {
        return false;
    };
    if path.split('/').any(|part| part == DEVPOD_INTERNAL) {
        return true;
    }
    if let Some(dir) = path.strip_suffix('/') {
        return holds_only_devpod_internal(&clone.join(dir));
    }
    path == ".devcontainer.json" && is_devpods_fallback_config(&clone.join(".devcontainer.json"))
}

/// Whether *dir* holds something, and everything in it is a `.devpod-internal`
/// or a directory that in turn holds only those. Symlinks are not followed.
fn holds_only_devpod_internal(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut any = false;
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if !is_dir {
            return false;
        }
        if entry.file_name() != DEVPOD_INTERNAL && !holds_only_devpod_internal(&entry.path()) {
            return false;
        }
        any = true;
    }
    any
}

/// Whether *file* is the config devpod saves for a fallback image: a JSON object
/// whose one key is `image`, a string.
fn is_devpods_fallback_config(file: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(file) else {
        return false;
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Object(map)) => {
            map.len() == 1 && map.get("image").is_some_and(serde_json::Value::is_string)
        }
        _ => false,
    }
}

/// What deleting *clone* would destroy, in words — or that git could not say.
///
/// `git status` is asked first and doubles as the repository probe: with the
/// repository named (see `Git::about`) it
/// succeeds on anything git can read — including a repository with no commits yet
/// — and refuses on every unusable `.git`. A refusal here is therefore not
/// "clean", it is [`Unsaved::CouldNotTell`], and so is a refusal from the
/// `git log` below: once the repository has been shown readable, a command that
/// then fails has failed for a reason nobody here can account for, and accounting
/// for it by saying "nothing to lose" is the bug this module exists to not have.
///
/// The `git log` is asked unconditionally, and since #471 it is asked about every
/// ref in the clone rather than about the checked-out branch. That drops the gate
/// this used to carry, which skipped the question when HEAD named no commit: on a
/// clone with no refs at all git exits 0 with no output, so the gate bought
/// nothing, and on a clone whose HEAD is unborn but which carries an orphan branch
/// it hid the one thing there was to find.
fn unsaved(git: &Git<'_>, clone: &Path, bare: BareCache<'_>) -> (Unsaved, Vec<String>) {
    let status = match git.status_porcelain(clone) {
        GitAnswer::Said(status) => status,
        GitAnswer::Refused(refused) => {
            return (
                Unsaved::CouldNotTell(CouldNotTell::GitCouldNotRead {
                    clone: clone.to_path_buf(),
                    reason: refused.reason().to_owned(),
                }),
                Vec::new(),
            );
        }
    };

    let mut losses = Vec::new();
    let mut cleared = Vec::new();
    if let Some(changed) = NonEmpty::of(
        status
            .lines()
            .filter(|line| !written_by_devpod(clone, line))
            .map(str::to_owned),
    ) {
        losses.push(Loss::Uncommitted(changed));
    }
    let local_tags = match local_tags(git, clone, bare) {
        GitAnswer::Said(tags) => tags,
        GitAnswer::Refused(refused) => {
            return (
                Unsaved::CouldNotTell(CouldNotTell::UnpushedNotListed {
                    clone: clone.to_path_buf(),
                    reason: refused.reason().to_owned(),
                }),
                Vec::new(),
            );
        }
    };
    match git.unpushed_commits(clone, &local_tags) {
        GitAnswer::Said(unpushed) => {
            if let Some(listed) = NonEmpty::of(unpushed.lines().map(str::to_owned)) {
                let copied = cleared_by_the_rules(git, clone, &listed, &local_tags);
                if let Some(commits) = NonEmpty::of(not_among(listed.iter(), &copied)) {
                    let by_tags = owed_to_tags(git, clone, &local_tags)
                        .and_then(|by_tags| by_tags.leaving_out(&copied));
                    losses.push(Loss::Unpushed { commits, by_tags });
                }
                cleared = copied;
            }
        }
        GitAnswer::Refused(refused) => {
            return (
                Unsaved::CouldNotTell(CouldNotTell::UnpushedNotListed {
                    clone: clone.to_path_buf(),
                    reason: refused.reason().to_owned(),
                }),
                Vec::new(),
            );
        }
    }
    let unsaved = match Losses::of(losses) {
        Some(losses) => Unsaved::WouldLose(losses),
        None => Unsaved::NothingToLose,
    };
    (unsaved, cleared)
}

/// Which of *listed* the rules show a remote already holds, as full hashes.
///
/// The one pipeline both counts go through: the merge and copy rules
/// ([`already_on_a_remote`]), then the squash rule ([`squashed_onto_a_remote`])
/// on what they left, then the revert rule ([`reverted_in_pairs`]) on what the
/// squash rule left. *listed* is the clone count's `git log --oneline` lines.
///
/// Each rule only adds to what is cleared, and a rule git refuses adds
/// nothing, so a failure anywhere leaves more counted, never less.
fn cleared_by_the_rules(
    git: &Git<'_>,
    clone: &Path,
    listed: &NonEmpty<String>,
    local_tags: &[String],
) -> Vec<String> {
    let mut copied = already_on_a_remote(git, clone, local_tags);
    let left = not_among(listed.iter(), &copied);
    if !left.is_empty() {
        copied.extend(squashed_onto_a_remote(git, clone, &left, local_tags));
    }
    let left = not_among(listed.iter(), &copied);
    if !left.is_empty() {
        copied.extend(reverted_in_pairs(git, clone, &left));
    }
    copied
}

/// The clone's unpushed commits that the rules clear, as full hashes: the
/// commits the clone count leaves out although no remote ref reaches them.
///
/// For a count that lists a part of the clone's unpushed commits, which is an
/// agent worktree's count of the commits under its HEAD (devlaunch#653,
/// devlaunch#659, devlaunch#664 were each written for the clone count alone).
/// Such a count takes these out of its own listing, so it clears a commit
/// exactly when the clone count clears it, and the two cannot disagree about
/// one commit.
///
/// **The rules are asked about the clone's whole listing, not the part.**
/// The worktree's commits are among the clone's, because `--all` reads every
/// worktree's HEAD, and the squash and revert rules weigh every ref that
/// reaches a commit, so the answer for a commit does not depend on which count
/// asks about it.
///
/// **Every failure clears nothing.** A refusal on the tags or on the listing
/// gives an empty answer, so the part's count stays as git listed it.
pub(crate) fn cleared_unpushed(git: &Git<'_>, clone: &Path, bare: BareCache<'_>) -> Vec<String> {
    let Some(local_tags) = local_tags(git, clone, bare).said() else {
        return Vec::new();
    };
    let Some(unpushed) = git.unpushed_commits(clone, &local_tags).said() else {
        return Vec::new();
    };
    match NonEmpty::of(unpushed.lines().map(str::to_owned)) {
        Some(listed) => cleared_by_the_rules(git, clone, &listed, &local_tags),
        None => Vec::new(),
    }
}

/// The unpushed commits whose content a remote already holds, as full hashes.
///
/// The SHA count in [`Git::unpushed_commits`] asks whether a remote holds *this
/// commit*. A rebase, a cherry-pick and a squash merge each put the same change
/// on the remote as a *new* commit, so the old one counts as unpushed although
/// nothing in it is lost. A kinisi_ros workspace refused `rm` over 23 commits
/// that way: its remote branch had been rebased, a merged PR had been squashed,
/// and five of the commits were merges of `origin/main`. So two more things
/// count as already on a remote:
///
/// - A commit with a copy on a remote ref, the same patch byte for byte and
///   whitespace included ([`Git::patches_already_on`]).
/// - A merge that adds nothing of its own to its parents
///   ([`Git::merges_with_nothing_of_their_own`]). Its parents are asked about
///   in their own right.
///
/// **Each local branch is compared with a handful of remote refs, not with all
/// of them.** They are the upstream of *every* local branch, the remote branch
/// of the same name, and `origin/HEAD`, which catches the squash of a one-commit
/// PR. Every upstream rather than the branch's own, because the branch that
/// holds the old commits is often a backup with no upstream at all: on the host
/// this was measured on, `backup/modex-pre-rebase-pull` and `sim-gate-backup`
/// held 28 old commits whose copies were all on another local branch's
/// upstream. A clone can hold thousands of remote refs, and the listing asks this
/// of every workspace, so a copy on a remote branch nothing local tracks is not
/// found, and its commit stays counted, which is the safe side.
///
/// A branch stops being compared once every one of its unpushed commits is
/// accounted for, so the usual clone pays one comparison per unpushed branch.
///
/// **Every failure here counts less as copied, never more.** A question git
/// refuses adds nothing to the answer, so the commits it was about stay
/// counted. The same holds for a commit that no local branch reaches (a stash,
/// a detached worktree HEAD, a local tag): no branch is compared for it, so
/// only the merge rule can clear it.
fn already_on_a_remote(git: &Git<'_>, clone: &Path, local_tags: &[String]) -> Vec<String> {
    let mut copied = git
        .merges_with_nothing_of_their_own(clone, local_tags)
        .said()
        .unwrap_or_default();
    let branches = git
        .branches_with_upstreams(clone)
        .said()
        .unwrap_or_default();
    let upstreams: Vec<&RemoteRef> = branches
        .iter()
        .filter_map(|branch| branch.upstream.as_ref())
        .collect();
    for branch in &branches {
        let Some(unpushed) = git.unpushed_commits_from(clone, &branch.name).said() else {
            continue;
        };
        for other in remote_refs_to_compare(branch, &upstreams) {
            if unpushed.lines().all(|line| is_among(line, &copied)) {
                break;
            }
            if let Some(hashes) = git.patches_already_on(clone, &branch.name, &other).said() {
                copied.extend(hashes);
            }
        }
    }
    copied
}

/// The counted commits whose branch's whole change a remote ref already holds,
/// as full hashes.
///
/// What [`already_on_a_remote`] leaves: a squash of several commits. The
/// squash's patch is none of theirs, so no copy is found, and each of them
/// counts. A kinisi_ros workspace refused `rm` over 3 commits that way, with
/// kinisi_ros#12035 squashed into `main` byte for byte.
///
/// So a branch is asked a question of its own: does a remote ref already hold
/// the change the branch makes since it left that ref
/// ([`Git::holds_the_change_of`])? The tip is asked first, then the commits
/// under it on its first-parent line, because a branch that carried on after
/// its squash holds new work at the tip and the squashed work under it. The
/// first commit that passes clears the commits on its first-parent line that no
/// remote ref has, and the commits above it stay counted. A commit a merge
/// brought in through its second parent is not cleared: the merge can drop
/// that commit's change (`merge -s ours`), and then the squash holds none of
/// it. The remote refs are the
/// ones the copy rule compares with ([`remote_refs_to_compare`]).
///
/// **A commit is cleared only when every ref that reaches it is a branch that
/// passed at or above it.** A second branch that grew from a squashed commit
/// and does not pass holds that commit's own state, which the squash may have
/// overwritten, so the commit stays counted with it. The stash, a local tag and
/// a detached HEAD never pass, so what they reach stays counted too.
///
/// It runs only when the copy rule left a commit counted, so a clone the copy
/// rule accounted for pays nothing here. When it runs it pays three spawns for
/// the clone and one `rev-list` per local branch, and only a branch that still
/// reaches a counted commit is merged: one more `rev-list` and one
/// `merge-tree --stdin` of up to [`LOOK_BACK`] points times its remote refs,
/// and three spawns more for each pair whose merge gives its ref's tree. A
/// pass costs one more read of the commits off every branch. The
/// merges in one clone are bounded by [`MERGE_BUDGET`].
///
/// **Every failure clears less, never more.** A refusal on any branch's commits
/// or on the refs that are not branches clears nothing at all, because what
/// was not read may hold a commit back, and so does a refusal on the remote
/// refs' trees or on how the clone merges. A refusal on one branch's batch of
/// merges fails that branch, and one on a single merge fails that pair.
fn squashed_onto_a_remote(
    git: &Git<'_>,
    clone: &Path,
    counted: &[String],
    local_tags: &[String],
) -> Vec<String> {
    let Some(branches) = git.branches_with_upstreams(clone).said() else {
        return Vec::new();
    };
    let Some(Some(merge)) = git.text_merge(clone).said() else {
        return Vec::new();
    };
    let upstreams: Vec<&RemoteRef> = branches
        .iter()
        .filter_map(|branch| branch.upstream.as_ref())
        .collect();
    let mut every_ref: Vec<RemoteRef> = Vec::new();
    for branch in &branches {
        for other in remote_refs_to_compare(branch, &upstreams) {
            if !every_ref.contains(&other) {
                every_ref.push(other);
            }
        }
    }
    let Some(ref_trees) = git.remote_ref_trees(clone, &every_ref).said() else {
        return Vec::new();
    };
    let mut asked = Asked {
        git,
        clone,
        merge: &merge,
        ref_trees: &ref_trees,
        merges_left: MERGE_BUDGET,
    };
    let mut cleared: HashSet<String> = HashSet::new();
    let mut held_back: HashSet<String> = HashSet::new();
    for branch in &branches {
        let Some(reached) = git.unpushed_hashes_from(clone, &branch.name).said() else {
            return Vec::new();
        };
        let still_counted = counted.iter().any(|line| is_among(line, &reached));
        let passed = if still_counted {
            asked.passed_at(branch, &upstreams)
        } else {
            HashSet::new()
        };
        held_back.extend(reached.into_iter().filter(|hash| !passed.contains(hash)));
        cleared.extend(passed);
    }
    if cleared.is_empty() {
        return Vec::new();
    }
    let Some(elsewhere) = git.unpushed_off_every_branch(clone, local_tags).said() else {
        return Vec::new();
    };
    held_back.extend(elsewhere);
    let mut cleared: Vec<String> = cleared
        .into_iter()
        .filter(|hash| !held_back.contains(hash))
        .collect();
    cleared.sort();
    cleared
}

/// The counted commits that come in pairs of a commit and its revert, as full
/// hashes.
///
/// What [`already_on_a_remote`] and [`squashed_onto_a_remote`] leave: a commit
/// and the commit that reverts it. Neither has a copy on a remote, and a branch
/// that changes nothing proves nothing to the squash rule. A kinisi_ros
/// workspace refused `rm` over a probe commit and its revert that way, with
/// the PR (kinisi_ros#11898) squashed into `main`. Together the two commits
/// change nothing, so a push of them would change nothing either.
///
/// **A pair** is a counted commit *R* whose one parent is a counted commit *C*
/// with one parent of its own, where *R*'s tree is the tree under *C* and
/// *C*'s tree is not. Trees, not patches, so the revert must take back the
/// whole change, byte for byte. So:
///
/// - an empty *C* is no pair, because an empty commit holds only its message;
/// - a root *C* is no pair, because there is no tree under it;
/// - a merge is no pair on either side, because its change is not one
///   commit's;
/// - a *C* that another rule cleared is no pair, because then *R* is the one
///   record of taking a change on a remote out again.
///
/// **A commit is in one pair at most, and the pairs are taken from the
/// bottom.** A revert of a revert puts the change back, so the third commit
/// holds what the first did and stays counted while the first two drop out.
///
/// **A pair drops out only when nothing else holds the state of *C*.** A ref or
/// a worktree's HEAD on *C* holds it, and so does a second commit that grew
/// from *C*: a branch, or the stash. Then both commits stay counted. Every ref
/// that reaches *C* then reaches it through *R*. "Every ref" is every ref
/// `--all` reads, as for the count: a linked worktree's own `refs/worktree/*`
/// and `refs/bisect/*` are not among them.
///
/// A cycle in the graph, which only `refs/replace` or grafts can make, holds
/// no pair.
///
/// It runs only when the other rules left a commit counted, and it costs one
/// `git log` of the unpushed graph, and one `rev-list` of the ref tips only
/// when a pair is found. **Every failure clears nothing**: a refusal on either,
/// or a graph in a shape this cannot read.
fn reverted_in_pairs(git: &Git<'_>, clone: &Path, counted: &[String]) -> Vec<String> {
    let Some(Some(graph)) = git.unpushed_graph(clone).said() else {
        return Vec::new();
    };
    let is_counted = |hash: &str| {
        graph.commit(hash).is_some()
            && counted
                .iter()
                .any(|line| is_among(line, std::slice::from_ref(&hash.to_owned())))
    };
    let reverts = |hash: &str| -> Option<String> {
        let revert = graph.commit(hash)?;
        let [reverted] = revert.parents.as_slice() else {
            return None;
        };
        let [under] = graph.commit(reverted)?.parents.as_slice() else {
            return None;
        };
        let under_tree = graph.tree_of(under)?;
        let changed = graph.tree_of(reverted)? != under_tree;
        (changed && revert.tree == under_tree && is_counted(hash) && is_counted(reverted))
            .then(|| reverted.clone())
    };
    struct Pair {
        reverted: String,
        revert: String,
    }
    let mut taken: HashMap<String, Option<Pair>> = HashMap::new();
    for (hash, _) in graph.unpushed() {
        let mut chain: Vec<Pair> = Vec::new();
        let mut at = hash.to_owned();
        let mut walked: HashSet<String> = HashSet::new();
        while !taken.contains_key(&at) {
            if !walked.insert(at.clone()) {
                // A cycle, which only `refs/replace` or grafts can make: git
                // shows the replaced parents. Nothing on it is a pair.
                for pair in chain.drain(..) {
                    taken.insert(pair.revert, None);
                }
                break;
            }
            match reverts(&at) {
                Some(reverted) => {
                    chain.push(Pair {
                        reverted: reverted.clone(),
                        revert: at,
                    });
                    at = reverted;
                }
                None => {
                    taken.insert(at.clone(), None);
                }
            }
        }
        for pair in chain.into_iter().rev() {
            let under_a_pair = matches!(taken.get(&pair.reverted), Some(Some(_)));
            taken.insert(pair.revert.clone(), (!under_a_pair).then_some(pair));
        }
    }
    let pairs: Vec<Pair> = taken.into_values().flatten().collect();
    if pairs.is_empty() {
        return Vec::new();
    }
    let Some(tips) = git.ref_tips(clone).said() else {
        return Vec::new();
    };
    let mut children: HashMap<&str, usize> = HashMap::new();
    for (_, commit) in graph.unpushed() {
        for parent in &commit.parents {
            *children.entry(parent.as_str()).or_default() += 1;
        }
    }
    let mut cleared: Vec<String> = pairs
        .into_iter()
        .filter(|pair| {
            !tips.contains(&pair.reverted) && children.get(pair.reverted.as_str()) == Some(&1)
        })
        .flat_map(|pair| [pair.reverted, pair.revert])
        .collect();
    cleared.sort();
    cleared
}

/// How far down a branch [`squashed_onto_a_remote`] looks for a commit that
/// passes.
///
/// A bound because `dl --ls --json` asks it of every workspace, and a branch of
/// work that was never pushed passes at no commit, so the walk costs one merge
/// per commit per remote ref and finds nothing. Eight covers a branch that
/// carried on for seven commits after its squash. A commit further down stays
/// counted, which is the safe side.
const LOOK_BACK: usize = 8;

/// How many merges [`squashed_onto_a_remote`] makes in one clone, at most.
///
/// Each branch is compared with every other branch's upstream, so the merges
/// grow with the square of the branches: 15 branches of local work make 1,920.
/// They run in one git per branch ([`Git::trees_of_clean_merges`]), and this
/// bounds what those gits are asked. A branch whose merges no longer fit is not
/// merged at all, and its commits stay counted, which is the safe side.
const MERGE_BUDGET: usize = 4096;

/// What [`squashed_onto_a_remote`] asks every branch with, and how many
/// merges it has left.
struct Asked<'g, 'a> {
    git: &'a Git<'g>,
    clone: &'a Path,
    merge: &'a TextMerge,
    ref_trees: &'a HashMap<String, String>,
    merges_left: usize,
}

impl Asked<'_, '_> {
    /// The commits *branch* passes at: every unpushed commit on the highest
    /// passing point's first-parent line, or none.
    ///
    /// Every point is merged into every remote ref the clone has in one
    /// [`Git::trees_of_clean_merges`], and only a pair whose merge gives its
    /// ref's tree is asked [`Git::holds_the_change_of`], the question that
    /// decides.
    fn passed_at(&mut self, branch: &LocalBranch, upstreams: &[&RemoteRef]) -> HashSet<String> {
        let Some(points) = self
            .git
            .unpushed_first_parents(self.clone, &branch.name, Some(LOOK_BACK))
            .said()
        else {
            return HashSet::new();
        };
        let others: Vec<RemoteRef> = remote_refs_to_compare(branch, upstreams)
            .into_iter()
            .filter(|other| self.ref_trees.contains_key(other.as_str()))
            .collect();
        let pairs: Vec<(&str, &RemoteRef)> = points
            .iter()
            .flat_map(|point| others.iter().map(move |other| (point.as_str(), other)))
            .collect();
        if pairs.len() > self.merges_left {
            return HashSet::new();
        }
        self.merges_left -= pairs.len();
        let Some(merged) = self
            .git
            .trees_of_clean_merges(self.clone, self.merge, &pairs)
            .said()
        else {
            return HashSet::new();
        };
        let passing = pairs
            .iter()
            .zip(merged)
            .filter(|((_, other), tree)| {
                tree.is_some() && tree.as_ref() == self.ref_trees.get(other.as_str())
            })
            .find(|((point, other), _)| {
                matches!(
                    self.git
                        .holds_the_change_of(self.clone, self.merge, point, other),
                    GitAnswer::Said(true)
                )
            });
        passing
            .and_then(|((point, _), _)| {
                self.git
                    .unpushed_first_parents(self.clone, point, None)
                    .said()
            })
            .map(|reached| reached.into_iter().collect())
            .unwrap_or_default()
    }
}

/// The remote refs [`already_on_a_remote`] compares *branch* with, its own
/// upstream first because that is where a rebased copy usually is.
///
/// `origin` by name, as [`Git::fetch_origin`] names it. A ref the clone has not
/// got is still listed: git refuses the comparison, and a refusal adds nothing.
///
/// **Only a [`RemoteRef`] is ever listed.** An upstream can be a local branch
/// (`branch -u feature other`), and a copy there is one more local copy: comparing
/// with it would clear the only commit that holds the change. The type is what
/// keeps it out, since [`LocalBranch::upstream`] never holds a local one.
fn remote_refs_to_compare(branch: &LocalBranch, upstreams: &[&RemoteRef]) -> Vec<RemoteRef> {
    let short = branch.name.strip_prefix(REFS_HEADS).unwrap_or(&branch.name);
    let candidates = branch
        .upstream
        .iter()
        .cloned()
        .chain([RemoteRef::of("origin", short)])
        .chain(upstreams.iter().map(|upstream| (*upstream).clone()))
        .chain([RemoteRef::of("origin", "HEAD")]);
    let mut refs: Vec<RemoteRef> = Vec::new();
    for candidate in candidates {
        if !refs.contains(&candidate) {
            refs.push(candidate);
        }
    }
    refs
}

/// The `git log --oneline` lines whose commit is not among *hashes*.
///
/// A line starts with an abbreviated hash, and git makes that abbreviation
/// unique in the repository it printed it for, so a full hash that starts with
/// it is that commit.
pub(crate) fn not_among<'a>(
    lines: impl Iterator<Item = &'a String>,
    hashes: &[String],
) -> Vec<String> {
    lines
        .filter(|line| !is_among(line, hashes))
        .cloned()
        .collect()
}

fn is_among(line: &str, hashes: &[String]) -> bool {
    let abbreviated = line.split(' ').next().unwrap_or_default();
    !abbreviated.is_empty() && hashes.iter().any(|hash| hash.starts_with(abbreviated))
}

/// Which of the unpushed commits nothing but a local tag reaches, if any.
///
/// Asked only once there is a refusal to explain, and only when a local tag was
/// named in the question that produced it, so a clone with nothing to lose pays
/// nothing and a clone with no tags pays nothing.
///
/// **A refusal here is not a [`Unsaved::CouldNotTell`], and that is the one place
/// this module bends its own rule.** Everywhere else a git command that fails
/// after the repository has been shown readable takes the whole answer with it,
/// because the alternative is accounting for the failure as "nothing to lose" —
/// permission, granted by a question that did not work. Nothing of the sort is
/// available here: the loss is already established and the clone is already being
/// kept. All that is lost is the half of the sentence that says which tag, so the
/// answer degrades to the sentence it had before (#487) rather than to a worse
/// arm. Failing the whole reading would trade a plainer refusal for a vaguer one.
fn owed_to_tags(git: &Git<'_>, clone: &Path, local_tags: &[String]) -> Option<ByLocalTags> {
    if local_tags.is_empty() {
        return None;
    }
    let GitAnswer::Said(only_tags) = git.commits_only_tags_reach(clone, local_tags) else {
        return None;
    };
    ByLocalTags::of(
        local_tags.iter().cloned(),
        only_tags.lines().map(str::to_owned),
    )
}

/// The clone's tags that the bare cache does not vouch for, as full refnames.
///
/// #487's whole answer. The unpushed question excludes `refs/tags/*` wholesale
/// because a tag the remote carries is not work in danger (#485/#486); these are
/// the tags it should not have excluded, and they go back into the question by
/// name.
///
/// Three things decide a tag is local, and all three fail towards keeping:
///
/// - the bare has not got a tag by that name;
/// - it has one by that name pointing at a different object, which means this one
///   was moved or retyped here and what it used to reach may be nowhere else;
/// - there is no bare to ask — [`BareCache::Unknown`], or a bare that refused —
///   in which case every tag in the clone is local as far as anything here has
///   established.
///
/// The bare is asked only when the clone has a tag to ask about, so a repository
/// with no tags pays one `for-each-ref` and a missing bare costs it nothing.
///
/// A refusal from the *clone* is a refusal of the whole answer, for the reason
/// [`unsaved`] gives: the repository has already been shown readable by
/// `git status`, so a question it then refuses has failed for a reason nobody
/// here can account for, and "no local tags" would be accounting for it.
fn local_tags(git: &Git<'_>, clone: &Path, bare: BareCache<'_>) -> GitAnswer<Vec<String>> {
    let here = match git.tags_in_clone(clone) {
        GitAnswer::Said(tags) => tags,
        GitAnswer::Refused(refused) => return GitAnswer::Refused(refused),
    };
    if here.is_empty() {
        return GitAnswer::Said(Vec::new());
    }
    let fetched = match bare {
        BareCache::At(bare) => match git.tags_in_bare(bare) {
            GitAnswer::Said(tags) => tags,
            // The bare is gone, half-removed, or not a repository. Nothing has
            // established that any of these tags came off a remote.
            GitAnswer::Refused(_) => Vec::new(),
        },
        BareCache::Unknown => Vec::new(),
    };
    GitAnswer::Said(
        here.into_iter()
            .filter(|tag| !vouched_for(tag, &fetched))
            .map(|tag| tag.name)
            .collect(),
    )
}

/// Whether the bare holds *tag* under the same name at the same object.
fn vouched_for(tag: &TagRef, fetched: &[TagRef]) -> bool {
    fetched.iter().any(|cached| cached == tag)
}

#[cfg(test)]
mod tests;
