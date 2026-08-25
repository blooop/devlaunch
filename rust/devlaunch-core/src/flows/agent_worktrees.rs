//! The git worktrees an agent harness leaves inside a workspace clone.
//!
//! # What this is about, and what the word means here
//!
//! An agent harness working inside a devcontainer makes its own git worktrees
//! under `<clone>/.claude/worktrees/<name>/`, one per task, and nothing ever
//! collects them. Measured on one host (devlaunch#426): 72 such directories,
//! 104.5 GB, 18 of them carrying a whole `.pixi/envs/default` — about 82% of
//! everything under `repos/`. Every one of them was inside a clone belonging to a
//! **live** devpod workspace, so [`crate::flows::lifecycle`]'s orphan rule not
//! only missed them, it must never fire on them: firing would delete a live
//! workspace's checkout.
//!
//! **`WorktreeInfo` is not this.** [`crate::domain::model::WorktreeInfo`] is
//! devlaunch's own long-standing name for *a workspace clone of one branch*, and
//! has nothing to do with anything in this module. Here "worktree" means git's
//! own thing — a second checkout registered in a repository, which
//! `git worktree list` prints and `git worktree prune` forgets.
//!
//! # Four categories, four safety profiles
//!
//! These are *registered* worktrees rather than stray directories, and they were
//! registered from inside the container, so the path git holds for one is
//! `/workspaces/<id>/.claude/worktrees/<name>` — a path that does not resolve on
//! the host at all. On the reference host: 6 locked and 33 prunable registrations
//! against 72 directories on disk. So a directory here is one of four things, and
//! [`decide`] is the only place any of them becomes deletable:
//!
//! - **Forgotten.** No registration names it. git has already let go and the
//!   directory is the whole of what is left.
//! - **Prunable.** Registered, and git's own listing says the registration is
//!   collectable — which on a host is what a container-registered worktree looks
//!   like, because the path it names is not there.
//! - **Locked.** Registered and locked. Never removed implicitly.
//! - **Held.** Registered, and git calls it neither locked nor prunable, so git
//!   still believes in it. Always kept, and this is the arm that stops a run
//!   *inside* a container — where the registered paths do resolve — from
//!   collecting its own live worktrees.
//!
//! # What a lock is, and what it is not
//!
//! A lock is the harness's courtesy, so `locked` is neither necessary nor
//! sufficient for "somebody is working in here": a killed session leaves one
//! behind, and a live session that never took one is indistinguishable from an
//! abandoned directory. **Nothing on a host can prove a worktree is idle**, so
//! nothing here claims to. Every refusal names the fact it rests on — registered,
//! locked, dirty, holding commits nothing else reaches — and never idleness.
//!
//! That also means the race is real and cannot be closed from here. `--prune`
//! holds devlaunch's per-repo lock, and a container running `git worktree add` is
//! not a participant in it: the scan can say prunable and the container can
//! re-register before the removal lands. So every directory is classified **again,
//! immediately before it goes**, which is the same reasoning that put a
//! [`WorktreePromotion`] on each candidate rather than a run-wide boolean.
//!
//! # Why a path is never resolved, and a name is matched instead
//!
//! The registrations and the directories are two views of one set, and the only
//! thing they reliably share is the place inside the clone: a `.claude/worktrees/`
//! and a leaf, possibly nested. So a registration is joined to a directory by that
//! suffix, and the path git printed is never handed to the filesystem. The
//! container prefix is also why `git worktree remove` is no use from a host — it
//! resolves the registered path — so a removal here is a directory removal
//! followed by [`Git::worktree_prune`], in that order.
//!
//! `.claude/worktrees/` is the match rather than the `agent-` prefix the names
//! happen to carry: the prefix is the harness's business and can change, where
//! the containing directory is the thing devlaunch is reasoning about. And a
//! directory sitting in that place is confirmed to be a worktree by its own `.git`
//! gitfile naming a `…/.git/worktrees/<name>` admin directory, because a plain
//! directory that happens to be there is not devlaunch's to delete.
//!
//! # Nesting
//!
//! An agent session running *inside* an agent worktree makes its worktrees under
//! *that* directory, and that nesting is how one clone on the reference host
//! reached 55 GB. So the scan recurses — but only into the worktrees it is
//! **keeping**. A directory that is going takes everything inside it, so
//! descending into one would report the same bytes twice and offer to remove a
//! directory that will not be there.

use std::path::{Path, PathBuf};

use crate::clients::git::Git;
use crate::domain::workspace_state::{CouldNotTell, Loss, Losses, NonEmpty, Unsaved};
use crate::flows::disk_usage::{self, DiskUsage};
use crate::flows::lifecycle::{Insistence, Objection, objection};

/// The directory an agent harness puts its worktrees in, relative to a clone.
const WORKTREES_DIR: [&str; 2] = [".claude", "worktrees"];

/// The `.git` gitfile's prefix, and the admin directory it names inside a clone.
const GITFILE_PREFIX: &str = "gitdir:";
const ADMIN_DIR: [&str; 2] = [".git", "worktrees"];

// ===========================================================================
// what git says about one registration
// ===========================================================================

/// What one worktree has checked out, as `worktree list --porcelain` says it.
///
/// Two arms rather than an optional branch, because a detached worktree's commits
/// are as losable as a branch's and the probe needs *something* to ask git about
/// either way. An absent branch that meant "ask nothing" would be the answer that
/// deletes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeHead {
    /// `branch refs/heads/<name>`, with the commit git printed beside it.
    Branch { reference: String, commit: String },
    /// `detached`, and the commit `HEAD` named.
    Detached { commit: String },
}

impl WorktreeHead {
    /// What a report calls it.
    pub fn named(&self) -> String {
        match self {
            Self::Branch { reference, .. } => reference
                .strip_prefix("refs/heads/")
                .unwrap_or(reference)
                .to_owned(),
            Self::Detached { commit } => format!("a detached HEAD at {}", short(commit)),
        }
    }

    /// What the clone is asked about it.
    ///
    /// The full `refs/heads/…` spelling rather than the short name, so a branch
    /// and a tag of one name cannot be taken for each other.
    fn revision(&self) -> &str {
        match self {
            Self::Branch { reference, .. } => reference,
            Self::Detached { commit } => commit,
        }
    }

    /// The commit itself, which is what the sibling bare cache is asked about:
    /// the branch *name* means nothing over there.
    fn commit(&self) -> &str {
        match self {
            Self::Branch { commit, .. } | Self::Detached { commit } => commit,
        }
    }
}

fn short(commit: &str) -> String {
    commit.chars().take(8).collect()
}

/// git is holding this worktree, and what it says about why.
///
/// The reason is genuinely absent for a `git worktree lock` with no `--reason`,
/// which is what a harness does, so this is an absence and not a stand-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    pub reason: Option<String>,
}

/// One paragraph of `git worktree list --porcelain`, for a worktree registered
/// somewhere under a `.claude/worktrees/`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Registration {
    /// Where inside a clone the registration sits, as
    /// `.claude/worktrees/<leaf>[/.claude/worktrees/<leaf>…]`. This is the join
    /// key; the path git printed is deliberately not kept, because on a host it
    /// names nothing.
    inside: String,
    head: WorktreeHead,
    locked: Option<Lock>,
    prunable: bool,
}

/// Every registration under a `.claude/worktrees/`, from `worktree list
/// --porcelain`.
///
/// The clone's own entry, and any worktree registered somewhere else entirely,
/// are dropped here: this module reasons about directories inside one clone, and
/// a registration outside has no directory here to be joined to.
fn registrations(listing: &str) -> Vec<Registration> {
    let mut found = Vec::new();
    for paragraph in listing.split("\n\n") {
        let mut inside = None;
        let mut reference = None;
        let mut commit = None;
        let mut locked = None;
        let mut prunable = false;
        for line in paragraph.lines() {
            let (key, rest) = match line.split_once(' ') {
                Some((key, rest)) => (key, Some(rest)),
                None => (line, None),
            };
            match (key, rest) {
                ("worktree", Some(path)) => inside = inside_a_worktrees_dir(Path::new(path)),
                ("HEAD", Some(sha)) => commit = Some(sha.to_owned()),
                ("branch", Some(name)) => reference = Some(name.to_owned()),
                ("locked", reason) => {
                    locked = Some(Lock {
                        reason: reason.map(str::to_owned).filter(|it| !it.is_empty()),
                    });
                }
                ("prunable", _) => prunable = true,
                _ => {}
            }
        }
        let (Some(inside), Some(commit)) = (inside, commit) else {
            continue;
        };
        let head = match reference {
            Some(reference) => WorktreeHead::Branch { reference, commit },
            None => WorktreeHead::Detached { commit },
        };
        found.push(Registration {
            inside,
            head,
            locked,
            prunable,
        });
    }
    found
}

/// Where in a clone `path` sits, as the join key, when it sits under a
/// `.claude/worktrees/` at all.
///
/// The suffix from the *first* `.claude/worktrees` onwards, so a nested worktree
/// keeps its whole path inside the clone and cannot be confused with a
/// same-named one at the top. Read off the components git printed rather than off
/// the filesystem, because the point of this module is that the path is not a
/// path here.
fn inside_a_worktrees_dir(path: &Path) -> Option<String> {
    let parts: Vec<String> = path
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    let at = (0..parts.len().saturating_sub(2)).find(|&at| {
        parts[at] == WORKTREES_DIR[0] && parts[at + 1] == WORKTREES_DIR[1] && at + 2 < parts.len()
    })?;
    Some(parts[at..].join("/"))
}

/// Where `directory` sits inside `clone`, as the same join key.
fn inside_the_clone(clone: &Path, directory: &Path) -> Option<String> {
    let relative = directory.strip_prefix(clone).ok()?;
    inside_a_worktrees_dir(relative)
}

/// The admin directory name `directory`'s own `.git` gitfile claims, when
/// `directory` really is a linked git worktree.
///
/// **This is the confirmation, and it is not the path shape.** A plain directory
/// sitting under `.claude/worktrees/`, or a file, or a symlink, is not
/// devlaunch's to remove; a linked worktree has a `.git` *file* reading
/// `gitdir: …/.git/worktrees/<name>`. The prefix of that path is the container's
/// and is ignored — only the `…/.git/worktrees/<name>` tail is read, which is
/// what says "a git worktree, registered under some repository's admin
/// directory".
fn linked_worktree_name(directory: &Path) -> Option<String> {
    let gitfile = directory.join(".git");
    if !std::fs::symlink_metadata(&gitfile).ok()?.is_file() {
        return None;
    }
    let content = std::fs::read_to_string(&gitfile).ok()?;
    let named = content.trim().strip_prefix(GITFILE_PREFIX)?.trim();
    let parts: Vec<String> = Path::new(named)
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    let (name, rest) = parts.split_last()?;
    let (worktrees, rest) = rest.split_last()?;
    let dot_git = rest.last()?;
    (dot_git == ADMIN_DIR[0] && worktrees == ADMIN_DIR[1]).then(|| name.clone())
}

// ===========================================================================
// what one directory is
// ===========================================================================

/// Which arm one directory under a `.claude/worktrees/` is.
///
/// The module docs carry what the four arms mean. `usage` rides on the three
/// removable arms and not on [`Self::Held`], for the reason
/// `lifecycle::CloneStatus` does the same: the walk behind it is O(files) with no
/// ceiling, and an arm nobody can reclaim is an arm nobody should pay to weigh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorktreeStatus {
    /// No registration names it, so there is no admin directory to ask git
    /// anything through either.
    ///
    /// Nothing is probed here and that is a real limit, not an oversight: with
    /// the admin directory gone there is no index and no HEAD, so no `git status`
    /// can be run against the directory at all. devlaunch#426 calls this category
    /// safe to delete outright, and it is the one category where devlaunch takes
    /// git's word for it rather than checking.
    Forgotten { usage: DiskUsage },
    /// Registered, and git's own listing calls the registration prunable.
    Prunable {
        head: WorktreeHead,
        holds: Unsaved,
        usage: DiskUsage,
    },
    /// Registered and locked.
    Locked {
        lock: Lock,
        head: WorktreeHead,
        holds: Unsaved,
        usage: DiskUsage,
    },
    /// Registered, and git calls it neither locked nor prunable.
    Held { head: WorktreeHead },
}

/// Whether the sibling bare cache's refs already reach a commit.
///
/// **This is the fix for a stale-ref trap and not a nicety.** A workspace clone
/// is cut from the sibling `.bare` and then has its remote repointed at the
/// forge, with no fetch of its own (see `flows::workspace_clone`'s module
/// header), so the clone's `refs/remotes/origin/*` is as of clone time and can be
/// arbitrarily old or absent. The `.bare` next door is the thing that gets
/// fetched. Ask the clone alone and branches that were pushed and merged months
/// ago read as unpushed, which keeps every byte forever and makes the flag the
/// only way to reclaim anything.
///
/// Both answers are **as of the last fetch**, which is what the report says. No
/// network call is added here: `--prune` is a local cleanup, and one that failed
/// offline would be a worse command than one that is sometimes out of date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InTheCache {
    /// Every commit is reachable from a ref the cache holds.
    Reached,
    /// The cache does not reach it — including the case where it has never seen
    /// the commit at all, which is what an unpushed branch looks like from there.
    Beyond,
    /// The cache could not be asked, or there is none.
    CouldNotSay,
}

/// Whether `commit` is inside what the bare cache already has.
fn in_the_cache(git: &Git<'_>, bare: Option<&Path>, commit: &str) -> InTheCache {
    let Some(bare) = bare else {
        return InTheCache::CouldNotSay;
    };
    match git.commits_beyond_every_ref(bare, commit).said() {
        // A refusal here is overwhelmingly `bad object` — the cache has never
        // seen this commit, which is exactly what an unpushed branch looks like
        // from the cache and is a fact, not a failure. A genuinely broken cache
        // lands here too and reads the same way, which is the conservative
        // direction: the clone is asked next, and only both of them failing to
        // find the work anywhere keeps the directory.
        None => InTheCache::Beyond,
        Some(count) => match count.trim().parse::<u64>() {
            Ok(0) => InTheCache::Reached,
            Ok(_) => InTheCache::Beyond,
            Err(_) => InTheCache::CouldNotSay,
        },
    }
}

/// What removing one registered worktree directory would destroy, or that git
/// could not say.
///
/// Mirrors `workspace_state`'s clone-level probe and answers in the same type, so
/// the report reads in the same words — but every question is asked differently,
/// because a host cannot ask this worktree anything the ordinary way:
///
/// - **The dirty check goes through the admin directory.** The worktree's own
///   `.git` gitfile names a container path, so pointing git at the directory
///   refuses. `--git-dir=<clone>/.git/worktrees/<name>` with
///   `--work-tree=<directory>` is the same repository reached from the side that
///   does resolve here.
/// - **`.claude/worktrees/` is excluded from it.** A worktree holding a nested
///   worktree would otherwise always read dirty — the nested directory is
///   untracked — and would be kept forever while the bytes that matter sat inside
///   it. Those nested directories are what this sweep reasons about separately,
///   not somebody's unsaved work.
/// - **Reachability asks the cache first.** See [`InTheCache`].
fn unsaved_in(
    git: &Git<'_>,
    clone: &Path,
    bare: Option<&Path>,
    admin: &Path,
    directory: &Path,
    head: &WorktreeHead,
) -> Unsaved {
    let dirt = match git.worktree_dirt(admin, directory).said() {
        None => {
            return Unsaved::CouldNotTell(CouldNotTell::GitCouldNotRead {
                clone: directory.to_path_buf(),
                reason: "git could not read this worktree through the clone's admin directory"
                    .to_owned(),
            });
        }
        Some(dirt) => dirt,
    };
    let mut losses = Vec::new();
    if let Some(changed) = NonEmpty::of(dirt.lines().map(str::to_owned)) {
        losses.push(Loss::Uncommitted(changed));
    }
    match in_the_cache(git, bare, head.commit()) {
        InTheCache::Reached => {}
        InTheCache::Beyond | InTheCache::CouldNotSay => {
            match git.unpushed_commits(clone, head.revision()).said() {
                None => {
                    return Unsaved::CouldNotTell(CouldNotTell::UnpushedNotListed {
                        clone: directory.to_path_buf(),
                        branch: head.named(),
                        reason: "neither the repository cache nor the clone could say whether \
                                 these commits are anywhere else"
                            .to_owned(),
                    });
                }
                Some(unpushed) => {
                    if let Some(commits) = NonEmpty::of(unpushed.lines().map(str::to_owned)) {
                        losses.push(Loss::Unpushed(commits));
                    }
                }
            }
        }
    }
    match Losses::of(losses) {
        Some(losses) => Unsaved::WouldLose(losses),
        None => Unsaved::NothingToLose,
    }
}

/// Which arm `directory` is, asked in the order that fails towards keeping it.
fn worktree_status(
    git: &Git<'_>,
    clone: &Path,
    bare: Option<&Path>,
    directory: &Path,
    admin: Option<&Path>,
    registered: Option<&Registration>,
) -> WorktreeStatus {
    let (Some(registration), Some(admin)) = (registered, admin) else {
        return WorktreeStatus::Forgotten {
            usage: disk_usage::exclusive_usage(directory),
        };
    };
    if !registration.prunable && registration.locked.is_none() {
        return WorktreeStatus::Held {
            head: registration.head.clone(),
        };
    }
    let holds = unsaved_in(git, clone, bare, admin, directory, &registration.head);
    let usage = disk_usage::exclusive_usage(directory);
    match registration.locked.clone() {
        Some(lock) => WorktreeStatus::Locked {
            lock,
            head: registration.head.clone(),
            holds,
            usage,
        },
        None => WorktreeStatus::Prunable {
            head: registration.head.clone(),
            holds,
            usage,
        },
    }
}

// ===========================================================================
// what is done about it
// ===========================================================================

/// One thing arguing against removing a worktree directory.
///
/// A list of these rather than one, because a locked worktree that is also dirty
/// has two things wrong with it and a report naming one would be telling half the
/// truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeObjection {
    /// git is holding it. Which says the harness asked for it to be left alone,
    /// and says nothing whatever about whether anyone is working in it.
    Locked { lock: Lock },
    /// Removing it would destroy this, or git could not say.
    Holds(Objection),
}

/// Why one worktree directory is staying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeKept {
    /// git still holds the registration and calls it neither locked nor
    /// prunable, so as far as git is concerned this worktree is live.
    StillHeld { head: WorktreeHead },
    /// At least one thing objected and `--force-worktrees` was not typed.
    Objected(NonEmpty<WorktreeObjection>),
}

/// Nothing objected, or `--force-worktrees` carried this directory past what did.
///
/// Carried per directory rather than read from a run-wide flag, for the reason
/// `lifecycle::Promotion` spells out at length: a plan-wide boolean says "the
/// user insisted" about every directory in the plan, including the ones nothing
/// objected to, and the acting pass then skips its re-check for all of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreePromotion {
    Unopposed,
    Insisted {
        despite: NonEmpty<WorktreeObjection>,
    },
}

impl WorktreePromotion {
    /// The insistence the acting pass re-applies to this directory alone.
    pub(crate) fn insistence(&self) -> Insistence {
        match self {
            Self::Unopposed => Insistence::NotInsisted,
            Self::Insisted { .. } => Insistence::Insisted,
        }
    }
}

/// How git saw a directory this run is removing.
///
/// Reported because the categories are different sentences: "git had already
/// forgotten these" needs nothing else done, where "git will let go of these once
/// it is asked" is what [`Git::worktree_prune`] follows for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeenAs {
    Forgotten,
    Prunable,
    Locked,
}

/// What `--prune` does about one directory under a `.claude/worktrees/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorktreeDecision {
    Remove {
        seen_as: SeenAs,
        usage: DiskUsage,
        promotion: WorktreePromotion,
    },
    Keep(WorktreeKept),
}

/// What `--prune` does about one worktree directory. The only such place.
///
/// Total over [`WorktreeStatus`]'s arms, so a fifth arm added later stops the
/// build rather than falling through into a deletion.
///
/// `--force-worktrees` promotes exactly the arms that carry objections. It is not
/// a general override: [`WorktreeStatus::Held`] is not a refusal to be insisted
/// past, it is git saying this worktree is live, and there is nothing for a person
/// to mean by insisting on it.
pub(crate) fn decide(status: WorktreeStatus, insistence: Insistence) -> WorktreeDecision {
    let (seen_as, usage, objections) = match status {
        WorktreeStatus::Held { head } => {
            return WorktreeDecision::Keep(WorktreeKept::StillHeld { head });
        }
        WorktreeStatus::Forgotten { usage } => (SeenAs::Forgotten, usage, Vec::new()),
        WorktreeStatus::Prunable { holds, usage, .. } => {
            (SeenAs::Prunable, usage, objections_of(None, &holds))
        }
        WorktreeStatus::Locked {
            lock, holds, usage, ..
        } => (SeenAs::Locked, usage, objections_of(Some(lock), &holds)),
    };
    match NonEmpty::of(objections) {
        None => WorktreeDecision::Remove {
            seen_as,
            usage,
            promotion: WorktreePromotion::Unopposed,
        },
        Some(objected) => match insistence {
            Insistence::Insisted => WorktreeDecision::Remove {
                seen_as,
                usage,
                promotion: WorktreePromotion::Insisted { despite: objected },
            },
            Insistence::NotInsisted => WorktreeDecision::Keep(WorktreeKept::Objected(objected)),
        },
    }
}

/// Everything arguing against removing one registered worktree, in the order a
/// report reads best in: what git is doing about it, then what it holds.
fn objections_of(lock: Option<Lock>, holds: &Unsaved) -> Vec<WorktreeObjection> {
    let mut objections = Vec::new();
    if let Some(lock) = lock {
        objections.push(WorktreeObjection::Locked { lock });
    }
    if let Some(objected) = objection(holds) {
        objections.push(WorktreeObjection::Holds(objected));
    }
    objections
}

// ===========================================================================
// the sweep
// ===========================================================================

/// One worktree directory this run will remove, what it frees, and why it may.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimableWorktree {
    pub path: PathBuf,
    pub seen_as: SeenAs,
    pub usage: DiskUsage,
    pub promotion: WorktreePromotion,
}

/// One worktree directory this run will leave standing, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptWorktree {
    pub path: PathBuf,
    pub because: WorktreeKept,
}

/// One clone's share of the sweep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneWorktrees {
    // Private, all of them, for the reason `PrunePlan`'s fields are: this is an
    // answer, and a caller that could fill it could pair one clone with a
    // classification of another clone's directories.
    clone: PathBuf,
    owner: String,
    repo: String,
    removing: Vec<ReclaimableWorktree>,
    keeping: Vec<KeptWorktree>,
    registrations_with_nothing_here: usize,
}

impl CloneWorktrees {
    /// The clone these worktrees live inside.
    pub fn clone_path(&self) -> &Path {
        &self.clone
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// Biggest first, path breaking ties, so two runs over an unchanged cache
    /// read alike.
    pub fn removing(&self) -> &[ReclaimableWorktree] {
        &self.removing
    }

    pub fn keeping(&self) -> &[KeptWorktree] {
        &self.keeping
    }

    /// Registrations under a `.claude/worktrees/` with no directory here at all.
    ///
    /// Worth its own count because it is the one category with **no bytes behind
    /// it**: the registration is either a container path that never resolved on
    /// this host or a directory somebody removed by hand, and either way there is
    /// nothing to free. `git worktree prune` is the whole of the work.
    pub fn registrations_with_nothing_here(&self) -> usize {
        self.registrations_with_nothing_here
    }

    /// What removing this clone's share would free.
    pub fn freed(&self) -> DiskUsage {
        disk_usage::total_usage(self.removing.iter().map(|it| it.usage.clone()))
    }

    /// Whether `git worktree prune` may run in this clone once the removals are
    /// done.
    ///
    /// **A data-loss guard, not tidiness.** `git worktree prune` is
    /// all-or-nothing across a clone, and on a host it drops the registration of
    /// *every* container-registered worktree — including one being kept because
    /// it is dirty or holds commits nothing else reaches. That registration is the
    /// only reason a later run can tell the directory apart from a forgotten one,
    /// and a forgotten one is removed outright. So pruning here would protect a
    /// worktree once and hand it over the second time.
    ///
    /// A lock survives a prune by git's own rule, so a worktree kept only for
    /// being locked does not hold the prune back.
    pub fn metadata_may_be_pruned(&self) -> bool {
        !self.keeping.iter().any(|kept| match &kept.because {
            WorktreeKept::StillHeld { .. } => false,
            WorktreeKept::Objected(objections) => objections
                .iter()
                .any(|objection| matches!(objection, WorktreeObjection::Holds(_))),
        })
    }

    fn nothing_to_say(&self) -> bool {
        self.removing.is_empty()
            && self.keeping.is_empty()
            && self.registrations_with_nothing_here == 0
    }
}

/// Every agent worktree inside the clones one `--prune` is keeping.
///
/// Empty on the overwhelming majority of hosts, and cheap to find out: a clone
/// with no `.claude/worktrees/` costs one failed `read_dir` and no git at all.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeSweep {
    clones: Vec<CloneWorktrees>,
}

impl WorktreeSweep {
    /// One entry per clone that has anything to say, in scan order.
    pub fn clones(&self) -> &[CloneWorktrees] {
        &self.clones
    }

    /// How many directories this run would remove.
    pub fn removing(&self) -> usize {
        self.clones.iter().map(|it| it.removing.len()).sum()
    }

    /// How many it would leave, whatever the reason.
    pub fn keeping(&self) -> usize {
        self.clones.iter().map(|it| it.keeping.len()).sum()
    }

    /// What the whole sweep would free.
    pub fn freed(&self) -> DiskUsage {
        disk_usage::total_usage(
            self.clones
                .iter()
                .flat_map(|it| it.removing.iter().map(|worktree| worktree.usage.clone())),
        )
    }

    /// Whether this sweep would change anything on disk or in git.
    pub fn nothing_to_do(&self) -> bool {
        self.clones
            .iter()
            .all(|it| it.removing.is_empty() && it.registrations_with_nothing_here == 0)
    }

    /// Whether there is nothing to say about it either.
    pub fn nothing_to_say(&self) -> bool {
        self.clones.is_empty()
    }

    fn record(&mut self, found: Option<CloneWorktrees>) {
        if let Some(found) = found.filter(|it| !it.nothing_to_say()) {
            self.clones.push(found);
        }
    }
}

/// Classify the agent worktrees inside one clone.
///
/// `None` when there is no `.claude/worktrees/` at all, which is the answer for
/// nearly every clone and what makes this affordable on the prune path: no
/// `git worktree list`, no disk walk, one `read_dir` that fails.
///
/// `bare` is the sibling repository cache, consulted for reachability; see
/// [`InTheCache`] for why the clone alone is not enough.
pub(crate) fn sweep_clone(
    git: &Git<'_>,
    clone: &Path,
    owner: &str,
    repo: &str,
    bare: Option<&Path>,
    insistence: Insistence,
) -> Option<CloneWorktrees> {
    let mut pending = children_of(&worktrees_dir(clone))?;
    // A git that will not answer takes the whole clone out of the sweep. Reading
    // a refusal as "git named no registrations" would classify every directory as
    // forgotten, and forgotten is the arm that deletes.
    let listing = git.worktree_listing(clone).said()?;
    let registered = registrations(&listing);
    let mut removing = Vec::new();
    let mut keeping = Vec::new();
    while let Some(directory) = pending.pop() {
        let (Some(inside), Some(name)) = (
            inside_the_clone(clone, &directory),
            linked_worktree_name(&directory),
        ) else {
            // Not a linked worktree: a plain directory that happens to sit here,
            // or one whose `.git` says something else. Not devlaunch's to remove
            // and not descended into either.
            continue;
        };
        let registration = registered.iter().find(|it| it.inside == inside);
        let admin = admin_dir(clone, &name);
        let status = worktree_status(git, clone, bare, &directory, admin.as_deref(), registration);
        match decide(status, insistence) {
            WorktreeDecision::Remove {
                seen_as,
                usage,
                promotion,
            } => removing.push(ReclaimableWorktree {
                path: directory,
                seen_as,
                usage,
                promotion,
            }),
            WorktreeDecision::Keep(because) => {
                // Only into the ones that are staying. A directory that is going
                // takes everything inside it, so descending into one would count
                // the same bytes twice and offer a directory that will not be
                // there.
                pending.extend(children_of(&worktrees_dir(&directory)).unwrap_or_default());
                keeping.push(KeptWorktree {
                    path: directory,
                    because,
                });
            }
        }
    }
    removing.sort_by(|left, right| {
        right
            .usage
            .known_bytes()
            .cmp(&left.usage.known_bytes())
            .then_with(|| left.path.cmp(&right.path))
    });
    keeping.sort_by(|left, right| left.path.cmp(&right.path));
    let registrations_with_nothing_here = registered
        .iter()
        .filter(|registration| !clone.join(&registration.inside).exists())
        .count();
    Some(CloneWorktrees {
        clone: clone.to_path_buf(),
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        removing,
        keeping,
        registrations_with_nothing_here,
    })
}

/// The `.claude/worktrees/` inside one directory.
fn worktrees_dir(directory: &Path) -> PathBuf {
    directory.join(WORKTREES_DIR[0]).join(WORKTREES_DIR[1])
}

/// The admin directory `name` names inside `clone`, when it is there.
fn admin_dir(clone: &Path, name: &str) -> Option<PathBuf> {
    let admin = clone.join(ADMIN_DIR[0]).join(ADMIN_DIR[1]).join(name);
    admin.is_dir().then_some(admin)
}

/// The directories directly inside `root`, or nothing when there is no `root`.
///
/// Symlinks are skipped rather than followed, for the reason the clone scan skips
/// them: following one walks a removal out of the cache directory `--prune` is
/// scoped to, and that scoping is what makes a scratch-cache run harmless.
fn children_of(root: &Path) -> Option<Vec<PathBuf>> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_dir() && !kind.is_symlink())
                .unwrap_or(false)
        })
        .map(|entry| entry.path())
        .collect();
    found.sort();
    Some(found)
}

#[cfg(test)]
mod tests;
