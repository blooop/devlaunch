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
//!   directory is the whole of what is left. This is the arm that deletes without
//!   git's help, so it is asked what it holds wherever there is still an admin
//!   directory to ask through, and only takes git's word when there is not.
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
use crate::flows::repo_manager::{Refusal, Removal, remove_tree_as_far_as_it_goes};

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
    /// key, and the only thing a directory is ever matched on.
    inside: String,
    /// The path git printed, kept for one question and never handed to the
    /// filesystem: whether it is a path *in this clone* or a container's. That is
    /// the whole of what tells a registration whose directory somebody deleted
    /// apart from one that never resolved on this host.
    registered_at: PathBuf,
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
        let mut registered_at = None;
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
                ("worktree", Some(path)) => {
                    inside = inside_a_worktrees_dir(Path::new(path));
                    registered_at = Some(PathBuf::from(path));
                }
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
        let (Some(inside), Some(registered_at), Some(commit)) = (inside, registered_at, commit)
        else {
            continue;
        };
        let head = match reference {
            Some(reference) => WorktreeHead::Branch { reference, commit },
            None => WorktreeHead::Detached { commit },
        };
        found.push(Registration {
            inside,
            registered_at,
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
    // `..` would name the clone's own `.git` and `.` its `.git/worktrees`, so a
    // gitfile carrying either would have this module probe, and then remove,
    // something that is not one worktree -- and without this guard it *did*
    // remove it, as a directory git had forgotten. The tail is file content and
    // file content is not trusted to be a name (devlaunch#442 review, S6).
    if name == "." || name == ".." || name.is_empty() {
        return None;
    }
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
    /// No registration names it: git has let go of the name.
    ///
    /// devlaunch#426 calls this category safe to delete outright, and it is the
    /// one arm that takes git's word rather than checking — **but only where there
    /// is nothing left to check with**. With the admin directory gone there is no
    /// index and no HEAD, so no question can be put at all, and `holds` is
    /// [`Unsaved::NothingToLose`] because that is the honest answer rather than a
    /// shortcut. Where the admin directory *is* here the directory can still be
    /// asked what it holds, and it is asked: reaching this arm with one present
    /// means git dropped the name or the suffix join missed, and no wrong
    /// classification should be able to cost somebody work (devlaunch#442 review,
    /// S1). This is the arm that deletes, so it is the arm that has to be hardest
    /// to reach by accident.
    Forgotten { holds: Unsaved, usage: DiskUsage },
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
/// - **Nested worktrees are dropped from the answer, and nothing else is.** A
///   worktree holding a nested one would otherwise always read dirty — the nested
///   directory is untracked — and would be kept forever while the bytes that
///   matter sat inside it. [`dirt_in`] carries which entries go and why the
///   exclusion is per-entry rather than per-path.
/// - **Reachability asks the cache first.** See [`InTheCache`].
fn unsaved_in(
    git: &Git<'_>,
    clone: &Path,
    bare: Option<&Path>,
    admin: &Path,
    directory: &Path,
    head: &WorktreeHead,
) -> Unsaved {
    let mut losses = Vec::new();
    match dirt_in(git, admin, directory) {
        Err(could_not_tell) => return could_not_tell,
        Ok(None) => {}
        Ok(Some(changed)) => losses.push(changed),
    }
    match unreachable_commits_in(git, clone, bare, directory, head) {
        Err(could_not_tell) => return could_not_tell,
        Ok(None) => {}
        Ok(Some(commits)) => losses.push(commits),
    }
    match Losses::of(losses) {
        Some(losses) => Unsaved::WouldLose(losses),
        None => Unsaved::NothingToLose,
    }
}

/// The commits `head` holds that neither the sibling bare cache nor the clone can
/// reach, or that neither could be asked.
///
/// The cache goes first, for the stale-ref reason [`InTheCache`] carries.
fn unreachable_commits_in(
    git: &Git<'_>,
    clone: &Path,
    bare: Option<&Path>,
    directory: &Path,
    head: &WorktreeHead,
) -> Result<Option<Loss>, Unsaved> {
    match in_the_cache(git, bare, head.commit()) {
        InTheCache::Reached => Ok(None),
        InTheCache::Beyond | InTheCache::CouldNotSay => {
            match git.unpushed_commits(clone, head.revision()).said() {
                None => Err(Unsaved::CouldNotTell(CouldNotTell::UnpushedNotListed {
                    clone: directory.to_path_buf(),
                    branch: head.named(),
                    reason: "neither the repository cache nor the clone could say whether these \
                             commits are anywhere else"
                        .to_owned(),
                })),
                Some(unpushed) => {
                    Ok(NonEmpty::of(unpushed.lines().map(str::to_owned)).map(Loss::Unpushed))
                }
            }
        }
    }
}

/// What a registration can still be asked when its admin directory has gone.
///
/// A narrow window -- `git worktree list` reads the admin directories, so a
/// registration means one was there a moment ago, and only a concurrent prune
/// takes it away between the listing and the look. But the arm this lands on is
/// the arm that deletes, and the registration still names a head, so the commits
/// can be asked about even though the working tree cannot. Fails towards keeping,
/// like everything else here.
fn unsaved_without_an_admin_dir(
    git: &Git<'_>,
    clone: &Path,
    bare: Option<&Path>,
    directory: &Path,
    head: &WorktreeHead,
) -> Unsaved {
    match unreachable_commits_in(git, clone, bare, directory, head) {
        Err(could_not_tell) => could_not_tell,
        Ok(None) => Unsaved::NothingToLose,
        Ok(Some(commits)) => match Losses::of([commits]) {
            Some(losses) => Unsaved::WouldLose(losses),
            None => Unsaved::NothingToLose,
        },
    }
}

/// What one worktree directory holds that is not committed, asked through the
/// admin directory, or that git would not say.
///
/// **The nested-worktree exclusion lives here rather than in a pathspec, and that
/// is the whole point** (devlaunch#442 review, S4). `:!.claude/worktrees` kept the
/// motivating case working — a worktree holding a nested one would otherwise read
/// dirty forever, and be kept forever while the bytes that matter sat inside it —
/// but it excluded the place rather than the thing, so it also hid two kinds of
/// real work: a *tracked* file modified under that path, and plain content
/// somebody put under a `.claude/worktrees/` that is not a worktree at all. The
/// second is the dangerous one, because the sweep skips it too (it is not a linked
/// worktree), so nothing reported it and nothing protected it, and it went when
/// its parent did.
///
/// So git is asked without a pathspec and the answer is filtered by what an entry
/// *is*: an untracked entry is dropped only when everything under it is a
/// confirmed linked worktree, which is exactly the set this sweep reasons about
/// separately. A tracked change is never dropped, whatever its path.
fn dirt_in(git: &Git<'_>, admin: &Path, directory: &Path) -> Result<Option<Loss>, Unsaved> {
    let dirt = match git.worktree_dirt(admin, directory).said() {
        None => {
            return Err(Unsaved::CouldNotTell(CouldNotTell::GitCouldNotRead {
                clone: directory.to_path_buf(),
                reason: "git could not read this worktree through the clone's admin directory"
                    .to_owned(),
            }));
        }
        Some(dirt) => dirt,
    };
    let lines = dirt
        .lines()
        .filter(|line| !is_only_nested_worktrees(directory, line))
        .map(str::to_owned);
    Ok(NonEmpty::of(lines).map(Loss::Uncommitted))
}

/// The dirt half alone, for a directory with no registration to ask about
/// reachability with.
fn dirt_only(git: &Git<'_>, admin: &Path, directory: &Path) -> Unsaved {
    match dirt_in(git, admin, directory) {
        Err(could_not_tell) => could_not_tell,
        Ok(None) => Unsaved::NothingToLose,
        Ok(Some(loss)) => match Losses::of([loss]) {
            Some(losses) => Unsaved::WouldLose(losses),
            None => Unsaved::NothingToLose,
        },
    }
}

/// Whether one `git status --porcelain` line is nothing but agent worktrees the
/// sweep is reasoning about separately.
///
/// Only ever true of an **untracked** entry on the `.claude/worktrees/` spine.
/// Both halves of that matter: a tracked change under the same path is somebody's
/// edit to a file the repository knows about, and an untracked entry anywhere else
/// is not this module's business and must not cost a walk to find out.
fn is_only_nested_worktrees(root: &Path, line: &str) -> bool {
    let Some(entry) = line.strip_prefix("?? ") else {
        return false;
    };
    // git quotes a path holding anything unusual. Quoted means unparsed here,
    // which reads as work, which is the direction that keeps the directory.
    if entry.starts_with('"') {
        return false;
    }
    let entry = Path::new(entry.trim_end_matches('/'));
    on_the_worktrees_spine(entry) && holds_only_worktrees(&root.join(entry))
}

/// Whether `entry` is the `.claude/worktrees/` spine or something under it.
///
/// The cheap guard in front of [`holds_only_worktrees`], which walks: without it
/// an untracked `build/` would be walked in full to establish what everyone
/// already knows, which is what the pathspec was buying.
fn on_the_worktrees_spine(entry: &Path) -> bool {
    let parts: Vec<String> = entry
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.first().is_some_and(|first| first == WORKTREES_DIR[0])
        && parts.get(1).is_none_or(|second| second == WORKTREES_DIR[1])
}

/// Whether every leaf under `path` is inside a confirmed linked worktree.
///
/// Stops at each worktree rather than descending into it, which is what bounds
/// the walk: the multi-gigabyte `.pixi/` that makes these directories worth
/// reclaiming is always inside one, so the only thing walked is content that is
/// *not* a worktree — which is precisely the content this is looking for.
///
/// A symlink is not descended into, which is what makes the recursion terminate:
/// a link back up its own tree would otherwise be walked forever. It reads as
/// something to keep, like anything else here that is not a confirmed worktree.
fn holds_only_worktrees(path: &Path) -> bool {
    if std::fs::symlink_metadata(path).is_ok_and(|it| it.is_symlink()) {
        return false;
    }
    if linked_worktree_name(path).is_some() {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        // A file, or a directory that will not be read. Neither is a worktree, so
        // neither is something to drop from the answer.
        return false;
    };
    entries
        .filter_map(Result::ok)
        .all(|entry| holds_only_worktrees(&entry.path()))
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
    let (registration, admin) = match (registered, admin) {
        (Some(registration), Some(admin)) => (registration, admin),
        // An admin directory with nothing joined to it. git has dropped the name,
        // or this module's suffix join missed one -- and either way the index and
        // HEAD are here, so the directory is asked what it holds before it is
        // treated as an empty leftover.
        (None, Some(admin)) => {
            return WorktreeStatus::Forgotten {
                holds: dirt_only(git, admin, directory),
                usage: disk_usage::exclusive_usage(directory),
            };
        }
        // A registration whose admin directory has gone: a concurrent prune, in
        // the window between the listing and this look. No index and no HEAD, so
        // the working tree cannot be asked -- but the registration still names a
        // head, and the commits can be.
        (Some(registration), None) => {
            return WorktreeStatus::Forgotten {
                holds: unsaved_without_an_admin_dir(
                    git,
                    clone,
                    bare,
                    directory,
                    &registration.head,
                ),
                usage: disk_usage::exclusive_usage(directory),
            };
        }
        // Neither. Nothing can be asked at all, and that is devlaunch#426's
        // category 1: git has let go and the directory is the whole of what is
        // left.
        (None, None) => {
            return WorktreeStatus::Forgotten {
                holds: Unsaved::NothingToLose,
                usage: disk_usage::exclusive_usage(directory),
            };
        }
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
        WorktreeStatus::Forgotten { holds, usage } => {
            (SeenAs::Forgotten, usage, objections_of(None, &holds))
        }
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
    registrations_with_nothing_here: RegistrationsWithNothingHere,
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
    /// it**: there is nothing to free, and [`Git::worktree_prune`] is the whole of
    /// the work.
    pub fn registrations_with_nothing_here(&self) -> RegistrationsWithNothingHere {
        self.registrations_with_nothing_here
    }

    /// How the plan expects `git worktree prune` to go in this clone.
    ///
    /// **A forecast, and it says so.** It is a fold over the worktrees the *plan*
    /// is keeping, which is everything the plan can know — and the acting pass
    /// re-classifies every candidate, so it can withhold one the plan meant to
    /// remove. That pass therefore builds its own gate from this one and folds its
    /// own outcomes in, rather than reading a prediction: see [`reclaim`].
    pub fn metadata_gate(&self) -> MetadataGate {
        self.keeping
            .iter()
            .fold(MetadataGate::default(), |gate, kept| {
                gate.and_keeping(&kept.because)
            })
    }

    /// Whether this clone's share of the sweep would change anything.
    ///
    /// **A registration with nothing behind it is not by itself work.** It frees
    /// no bytes, and the only thing that clears it is the metadata prune — which
    /// runs only where the gate is open. Where the gate is closed the registration
    /// is being kept on purpose, so counting it as work makes `--prune` print a
    /// section, ask the question and do nothing, every run, for as long as the
    /// worktree it is protecting stays protected (devlaunch#442 review, S3).
    fn nothing_to_do(&self) -> bool {
        self.removing.is_empty()
            && (self.registrations_with_nothing_here.none() || !self.metadata_gate().open())
    }

    fn nothing_to_say(&self) -> bool {
        self.removing.is_empty()
            && self.keeping.is_empty()
            && self.registrations_with_nothing_here.none()
    }
}

/// Whether `git worktree prune` may still run in one clone.
///
/// **A data-loss guard, not tidiness, and it answers to outcomes rather than to
/// predictions.** `git worktree prune` is all-or-nothing across a clone, and on a
/// host it drops the registration of *every* container-registered worktree —
/// including one being kept because it is dirty or holds commits nothing else
/// reaches. That registration is the only reason a later run can tell the
/// directory apart from a forgotten one, and [`WorktreeStatus::Forgotten`] is the
/// arm that deletes. So pruning there protects a worktree once and hands it over
/// the second time.
///
/// Which is why this is a value that gets folded, and not a method on
/// [`CloneWorktrees`]. A gate read off the plan's keeps alone misses the candidate
/// the acting pass re-classified and withheld — the prune then ran, took that
/// worktree's registration with it, and the next run removed the directory and an
/// afternoon's uncommitted work outright, with no flag typed at either run
/// (devlaunch#442 review, S1). The plan and the act disagree by design; the fold
/// is what stops that disagreement being spendable.
///
/// A lock survives a prune by git's own rule, so a worktree kept only for being
/// locked does not close the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MetadataGate {
    closed: bool,
}

impl MetadataGate {
    /// Fold in one worktree that is staying, whichever pass decided it.
    pub(crate) fn and_keeping(mut self, because: &WorktreeKept) -> Self {
        self.closed |= registration_goes_on_protecting(because);
        self
    }

    /// Whether the prune may run.
    pub fn open(self) -> bool {
        !self.closed
    }
}

/// Whether the registration is what goes on protecting a worktree that is
/// staying. The single rule, which both passes reach through [`MetadataGate`].
fn registration_goes_on_protecting(because: &WorktreeKept) -> bool {
    match because {
        // git holds this one, and git skips what it holds when it prunes.
        WorktreeKept::StillHeld { .. } => false,
        WorktreeKept::Objected(objections) => objections
            .iter()
            .any(|objection| matches!(objection, WorktreeObjection::Holds(_))),
    }
}

/// Registrations under a `.claude/worktrees/` with no directory in the clone,
/// told apart by *why* there is nothing there.
///
/// **Two counts rather than one, because the sharpened spec on devlaunch#426 asks
/// the two apart and they are different facts.** Neither has host bytes behind it,
/// so `git worktree prune` is the whole of the work either way — but a registered
/// container path that never resolved here is the ordinary shape of every worktree
/// an agent made inside a devcontainer, where a registration naming a directory in
/// *this* clone that is not there is either somebody's own `rm -rf` or a previous
/// run interrupted between the removal and the prune. One is routine and one is
/// worth reading, and a single number said neither.
///
/// The registered path is compared as a prefix and never resolved, which is the
/// module's whole discipline about these paths: on a host a container path names
/// nothing, and asking the filesystem about it is how that fact gets lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RegistrationsWithNothingHere {
    container_paths: usize,
    deleted: usize,
}

impl RegistrationsWithNothingHere {
    /// Registered at a path outside this clone — a container's, which is what
    /// every worktree an agent made inside a devcontainer carries.
    pub fn container_paths(self) -> usize {
        self.container_paths
    }

    /// Registered at a path inside this clone, with nothing at it.
    pub fn deleted(self) -> usize {
        self.deleted
    }

    /// Whether there are none of either.
    pub fn none(self) -> bool {
        self.container_paths == 0 && self.deleted == 0
    }

    fn count(&mut self, clone: &Path, registered_at: &Path) {
        if registered_at.starts_with(clone) {
            self.deleted += 1;
        } else {
            self.container_paths += 1;
        }
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
    ///
    /// The counts are what the tests assert against; `dl` reads the per-clone
    /// lists, so nothing outside this crate has ever wanted either of these.
    #[cfg(test)]
    pub(crate) fn removing(&self) -> usize {
        self.clones.iter().map(|it| it.removing.len()).sum()
    }

    /// How many it would leave, whatever the reason.
    #[cfg(test)]
    pub(crate) fn keeping(&self) -> usize {
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
        self.clones.iter().all(CloneWorktrees::nothing_to_do)
    }

    /// Whether there is nothing to say about it either.
    pub fn nothing_to_say(&self) -> bool {
        self.clones.is_empty()
    }

    pub(crate) fn record(&mut self, found: Option<CloneWorktrees>) {
        if let Some(found) = found.filter(|it| !it.nothing_to_say()) {
            self.clones.push(found);
        }
    }
}

/// What git says about one clone's worktrees, read once.
///
/// One `git worktree list` per clone rather than one per candidate, and the
/// reason it is a value rather than a parameter list: the acting pass has to
/// classify each directory *again* immediately before removing it, and it must
/// re-read git to do that. Sharing this type is what keeps the two passes asking
/// the same question in the same words.
pub(crate) struct ClonePicture {
    registered: Vec<Registration>,
}

impl ClonePicture {
    /// What git says about `clone`, or nothing when git will not say.
    ///
    /// A refusal takes the whole clone out of the sweep. Reading one as "git named
    /// no registrations" would classify every directory as forgotten, and
    /// forgotten is the arm that deletes.
    pub(crate) fn of(git: &Git<'_>, clone: &Path) -> Option<Self> {
        let listing = git.worktree_listing(clone).said()?;
        Some(Self {
            registered: registrations(&listing),
        })
    }

    /// Which arm `directory` is, or nothing when it is not a linked worktree of
    /// this clone at all.
    pub(crate) fn status_of(
        &self,
        git: &Git<'_>,
        clone: &Path,
        bare: Option<&Path>,
        directory: &Path,
    ) -> Option<WorktreeStatus> {
        let inside = inside_the_clone(clone, directory)?;
        let name = linked_worktree_name(directory)?;
        let registration = self.registered.iter().find(|it| it.inside == inside);
        let admin = admin_dir(clone, &name);
        Some(worktree_status(
            git,
            clone,
            bare,
            directory,
            admin.as_deref(),
            registration,
        ))
    }

    /// Registrations under a `.claude/worktrees/` with no directory in `clone`,
    /// counted by why there is nothing there.
    fn registrations_with_nothing_here(&self, clone: &Path) -> RegistrationsWithNothingHere {
        let mut counted = RegistrationsWithNothingHere::default();
        for registration in &self.registered {
            if !clone.join(&registration.inside).exists() {
                counted.count(clone, &registration.registered_at);
            }
        }
        counted
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
    let picture = ClonePicture::of(git, clone)?;
    let mut removing = Vec::new();
    let mut keeping = Vec::new();
    while let Some(directory) = pending.pop() {
        let Some(status) = picture.status_of(git, clone, bare, &directory) else {
            // Not a linked worktree: a plain directory that happens to sit here,
            // or one whose `.git` says something else. Not devlaunch's to remove
            // and not descended into either.
            continue;
        };
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
    let registrations_with_nothing_here = picture.registrations_with_nothing_here(clone);
    Some(CloneWorktrees {
        clone: clone.to_path_buf(),
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        removing,
        keeping,
        registrations_with_nothing_here,
    })
}

/// How much of a clone's bytes are agent git worktrees, or nothing when it has
/// none.
///
/// **Attribution, not an addition.** These bytes are inside the clone, so they are
/// already in what `dl --ls --size` says the clone would free; this says how much
/// of that figure is worktrees. It reached 82% of a whole cache on the reference
/// host while being invisible in `--ls --size`, which is how it got to a full disk
/// (devlaunch#426).
///
/// One walk of the whole `.claude/worktrees/` tree rather than one per worktree,
/// which also means nesting is counted once and counted right. The object store is
/// not in it: a linked worktree shares the clone's, and the clone's objects are
/// hardlinked out of the `.bare` next door, so billing them here would count them
/// two or three times — which [`disk_usage::exclusive_usage`] already refuses to
/// do, because a file's bytes are a tree's only when every link to it is inside
/// that tree.
///
/// `None` rather than a zero, because "this clone has never had an agent worktree
/// in it" and "it has some and they cost nothing" are different facts, and the
/// first is what nearly every clone is.
pub(crate) fn bytes_in(clone: &Path) -> Option<DiskUsage> {
    let root = worktrees_dir(clone);
    root.is_dir().then(|| disk_usage::exclusive_usage(&root))
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

// ===========================================================================
// the acting pass
// ===========================================================================

/// One worktree directory the plan meant to remove that the acting pass would
/// not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithheldWorktree {
    pub path: PathBuf,
    /// Why it is staying — and it is worth saying this was not so when the plan
    /// was printed.
    pub because: WorktreeKept,
}

/// What the acting pass did about the agent worktrees.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeReport {
    pub removed: Vec<ReclaimableWorktree>,
    pub withheld: Vec<WithheldWorktree>,
    /// Directories that would not come away. Not empty means the run is
    /// unfinished, and what did go is still gone.
    pub refused: Vec<Refusal>,
    /// Clones whose `git worktree prune` was held back, because something in
    /// them is being kept for what it holds and the registration is what keeps on
    /// protecting it. Named rather than counted: it is the one line that explains
    /// why git still lists a worktree whose directory is not there.
    pub metadata_held_back: Vec<PathBuf>,
    /// Clones where the prune ran and would not.
    pub metadata_refused: Vec<PathBuf>,
}

impl WorktreeReport {
    /// What this run actually freed, with the figures the plan measured, so what
    /// somebody is told they got back is what they said yes to.
    pub fn freed(&self) -> DiskUsage {
        disk_usage::total_usage(self.removed.iter().map(|it| it.usage.clone()))
    }

    pub fn nothing_to_say(&self) -> bool {
        self.removed.is_empty()
            && self.withheld.is_empty()
            && self.refused.is_empty()
            && self.metadata_held_back.is_empty()
            && self.metadata_refused.is_empty()
    }
}

/// Carry out one clone's share of the sweep, and add what happened to `report`.
///
/// The caller holds the repository lock. **Every directory is classified again,
/// under that lock, immediately before it goes**, and only what this pass *also*
/// finds removable is removed. The lock is not enough on its own and cannot be
/// made enough: a container running `git worktree add` is not a participant in
/// it, so the plan a person answered can have been overtaken by a worktree that
/// is now registered and live. The approved set can therefore shrink between the
/// report and the act and can never grow, which is the direction that costs a
/// command rather than somebody's afternoon.
///
/// **The directory goes first and `git worktree prune` follows.** Interrupted
/// between the two, git is left holding a registration whose directory is gone —
/// which is exactly the prunable state the next run already handles, so the run
/// heals itself. The other order leaves a registered, present worktree with its
/// metadata dropped, which nothing recognises and the next run removes outright.
pub(crate) fn reclaim(
    git: &Git<'_>,
    clone: &CloneWorktrees,
    bare: Option<&Path>,
    report: &mut WorktreeReport,
) {
    let Some(picture) = ClonePicture::of(git, &clone.clone) else {
        // git will not say what it holds any more, so nothing here is removable:
        // the classification the plan rests on cannot be re-taken.
        report
            .withheld
            .extend(clone.removing.iter().map(|worktree| WithheldWorktree {
                path: worktree.path.clone(),
                because: WorktreeKept::Objected(NonEmpty::one(WorktreeObjection::Holds(
                    Objection::CouldNotTell(CouldNotTell::GitCouldNotRead {
                        clone: clone.clone.clone(),
                        reason:
                            "git would not list this clone's worktrees a second time".to_owned(),
                    }),
                ))),
            }));
        return;
    };
    // Seeded from the plan's keeps and then fed every outcome this pass reaches,
    // so the thing the prune is gated on is what happened rather than what was
    // foreseen. See [`MetadataGate`] for what reading the forecast here cost.
    let mut gate = clone.metadata_gate();
    let mut removed_anything = false;
    for worktree in &clone.removing {
        let status = picture.status_of(git, &clone.clone, bare, &worktree.path);
        let decision = match status {
            // The directory is no longer a linked worktree of this clone — it was
            // removed by hand, or something else is there now. Either way this
            // pass has nothing it can say is safe to delete.
            None => WorktreeDecision::Keep(WorktreeKept::Objected(NonEmpty::one(
                WorktreeObjection::Holds(Objection::CouldNotTell(CouldNotTell::CouldNotLook {
                    clone: worktree.path.clone(),
                    error: "this is no longer a linked worktree of the clone".to_owned(),
                })),
            ))),
            Some(status) => decide(status, worktree.promotion.insistence()),
        };
        match decision {
            WorktreeDecision::Keep(because) => {
                gate = gate.and_keeping(&because);
                report.withheld.push(WithheldWorktree {
                    path: worktree.path.clone(),
                    because,
                });
            }
            WorktreeDecision::Remove { .. } => {
                match remove_tree_as_far_as_it_goes(&worktree.path) {
                    Removal::Everything => {
                        removed_anything = true;
                        report.removed.push(worktree.clone());
                    }
                    Removal::WhatItCould(refused) | Removal::Nothing(refused) => {
                        report.refused.extend(refused.iter().cloned());
                    }
                }
            }
        }
    }
    // A clone whose only outstanding work is a registration with nothing behind it
    // still has that work done, or the plan would go on offering it forever
    // (devlaunch#442 review, S3).
    if !removed_anything && clone.registrations_with_nothing_here.none() {
        return;
    }
    if !gate.open() {
        report.metadata_held_back.push(clone.clone.clone());
        return;
    }
    if git.worktree_prune(&clone.clone).said().is_none() {
        report.metadata_refused.push(clone.clone.clone());
    }
}

#[cfg(test)]
mod tests;
