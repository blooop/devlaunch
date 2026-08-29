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
//! `git worktree list` prints.
//!
//! # The unit is a site subtree (devlaunch#445)
//!
//! The sweep reasons about **sites**: places inside the clone of the shape
//! `.claude/worktrees/<leaf>[/.claude/worktrees/<leaf>…]`, together with
//! everything nested inside them. A site's verdict is decided bottom-up and
//! conjunctively — a site is collectable only when it *and every site nested in
//! it* are — because the operation that removes bytes is a subtree removal, and a
//! unit narrower than the operation's blast radius is how a nested worktree's
//! uncommitted work, unpushed commits or lock got deleted with its parent, with
//! no flag typed (devlaunch#442 review, T1). That state has no representation
//! here: the collectable arm of a verdict is derived by a recursion that visits
//! every child itself, so a parent whose subtree holds a standing site cannot be
//! handed a collectable verdict by anyone.
//!
//! Containment edges come from the **filesystem walk**, never from
//! prefix-comparing recorded path strings: on a host every recorded path is a
//! string about another machine, and a worktree of a different repository has no
//! edge in this clone's listing at all. Recorded paths are used for exactly two
//! things, neither of which resolves them — the suffix join, and the argument to
//! `git worktree remove`.
//!
//! # The two operations, and their two radii
//!
//! - **Removing bytes** is [`remove_tree_as_far_as_it_goes`], whose radius is a
//!   subtree. Its unit is the site subtree above.
//! - **Forgetting a registration** is `git worktree remove <the path git
//!   printed>`, per registration, by name. Measured on git 2.51.1 and 2.43.0: it
//!   drops exactly one registration when the recorded path does not resolve —
//!   which is every container-registered worktree seen from a host — and it
//!   *works because the path does not resolve*, not despite it. When the recorded
//!   path resolves to something unrelated, git refuses, and `-f -f` does not get
//!   past that refusal. Its unit is one name.
//!
//! There is deliberately no `git worktree prune` here. Its domain is a `readdir`
//! over `$GIT_DIR/worktrees` **at act time**, so no plan-time unit can equal it:
//! a registration created after the plan was printed is in its blast radius, and
//! three registration states it reaches never appear in any listing. An operation
//! whose domain devlaunch cannot enumerate is an operation devlaunch cannot fail
//! towards keeping with, so it is deleted rather than gated (devlaunch#445, and
//! review T2 on devlaunch#442 is what the gate kept failing to hold). The
//! registrations git's own `gc` reclaims under `gc.worktreePruneExpire` are a
//! named third party's, not stragglers.
//!
//! `git worktree remove` also applies **no dirty check at all** when the recorded
//! path does not resolve — the cleanliness check sits inside git's own
//! `file_exists(wt->path)` — so the metadata operation contributes no safety of
//! its own. The verdict here is the entirety of what stands between a
//! registration and somebody's afternoon, and nothing in this module leans on git
//! refusing.
//!
//! # Ownership is a registration join, never a gitfile tail (devlaunch#463)
//!
//! A directory's own `.git` gitfile tail (`…/.git/worktrees/<name>`) says only
//! that the directory is *a* linked worktree, registered under **some**
//! repository's admin directory — and reading that as "one of ours" is how a live
//! worktree of a different repository, nested inside one of our clones and
//! holding uncommitted work, was offered for removal unopposed under the printed
//! reason "git has already forgotten it", which was false. So the tail is the
//! test for *is this a worktree at all*, never for *whose*: a site is ours only
//! when a registration **from this clone's own listing** joins it by its place
//! inside the clone, and the admin directory used to probe it is derived from
//! that joined registration, never taken from a name. A worktree this clone's
//! listing does not account for stands, is reported, and is never probed.
//!
//! git contributes nothing to that case. Its one unforceable refusal fires only
//! on a recorded path handed to `worktree remove`, which is an invocation this
//! module never makes for a foreign worktree; it says nothing about the byte
//! removal, which destroys a nested foreign tree exit 0 and silent; and `is not a
//! working tree` is this repository declining to recognise, not git protecting a
//! foreign owner.
//!
//! # What protects a run inside a container
//!
//! Nothing in this module detects containers, and no arm of it exists to protect
//! one. Two properties carry that instead (devlaunch#462):
//!
//! - **P1, locality.** The domain `--prune` sweeps is enumerated from the cache
//!   directory alone, and devlaunch never mounts a cache clone into a container
//!   at a path inside *that* container's own cache. So every registration in an
//!   enumerated clone was recorded one namespace in from the one enumerating it,
//!   and the container's own clone — bind-mounted at `/workspaces/<id>` — is
//!   never in the domain at all.
//! - **P2, ordering.** The directory goes first and the forget follows, and
//!   nothing is forgotten on a partial removal. So the recorded path does not
//!   resolve at the moment the forget runs, even where it resolved a moment
//!   earlier.
//!
//! git contributes none of this either: see the note on the missing dirty check
//! above.
//!
//! # What a lock is, and what it is not
//!
//! git documents a lock as saying the worktree may be on a portable device, and
//! nothing else. It is a claim over the site by a party this pass cannot
//! interrogate — which is precisely *could not be proved*, so it lands in
//! [`Blank::ThirdPartyClaim`] and never in [`Reason::Holds`]: reporting a lock as
//! a loss would be inventing work that may not exist. Nothing on a host can prove
//! a worktree idle either, so nothing here claims it; every refusal names the
//! fact it rests on.
//!
//! The race with a container running `git worktree add` is real and cannot be
//! closed from here — a container is not a participant in devlaunch's repository
//! lock. What this module bounds is its radius: the acting pass re-derives every
//! verdict immediately before acting, a site the plan did not approve cannot be
//! collectable then, and the only thing left between the re-check and the act is
//! one subtree the pass walked microseconds earlier or one registration it just
//! looked at.
//!
//! # Stashes are not a question
//!
//! Measured: a `git stash push` from inside a linked worktree writes the clone's
//! own `refs/stash`, survives the worktree directory, and is reached by
//! `rev-list --all`. It is in the shared ref store, which nothing here removes.
//! No probe asks about it, and none should be added.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::clients::git::{ForgetForce, Git};
use crate::domain::workspace_state::{
    self, BareCache, CouldNotTell, Loss, Losses, NonEmpty, Unsaved,
};
use crate::flows::disk_usage::{self, DiskUsage};
use crate::flows::lifecycle::Insistence;
use crate::flows::repo_manager::{Refusal, TreeSweep, remove_tree_as_far_as_it_goes};

mod derivatives;

pub use derivatives::{
    Derivative, NoRecipe, NotDerivableNow, Recipe, ReclaimedDerivative, Tagged, WithheldDerivative,
};
use derivatives::{Derivatives, claims_over, tagged_in};

/// The directory an agent harness puts its worktrees in, relative to a clone.
const WORKTREES_DIR: [&str; 2] = [".claude", "worktrees"];

/// The `.git` gitfile's prefix, and the admin directory tail it names.
const GITFILE_PREFIX: &str = "gitdir:";
const ADMIN_DIR: [&str; 2] = [".git", "worktrees"];

// ===========================================================================
// two newtypes that carry the module's discipline
// ===========================================================================

/// The path git printed for one registration, kept whole.
///
/// The only constructor parses `git worktree list --porcelain`, so a `Recorded`
/// in hand *is* the fact that a listing named it — which is what makes "no git
/// invocation ever names a registration the pass did not read from a listing"
/// structural rather than a rule: the forget's argument is a `Recorded`, and a
/// registration created after the plan was printed has never been one.
///
/// It is never resolved and never assembled from parts. It is used for two
/// things: the suffix join against the walk, and the argument to
/// `git worktree remove`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    path: PathBuf,
}

impl Recorded {
    /// The path as git printed it, for the report and for the forget. Never a
    /// path to hand to the filesystem.
    pub fn as_path(&self) -> &Path {
        &self.path
    }
}

/// Where inside a clone a site sits:
/// `.claude/worktrees/<leaf>[/.claude/worktrees/<leaf>…]`.
///
/// The join key between the two views of one set — the directories the walk
/// finds and the places the listing's registrations name — and the only thing a
/// directory is ever matched on. Built from a recorded path's components or from
/// a walked path relative to the clone; never from anything else.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Inside {
    place: String,
}

impl Inside {
    pub fn as_str(&self) -> &str {
        &self.place
    }
}

/// What a standing reason is about: the clone itself, or one site inside it.
///
/// Two arms rather than an optional site, because "no site" is a different fact
/// from "the clone", and a report that interpolates the wrong one misattributes
/// the loss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    TheCloneItself,
    ASite(Inside),
}

// ===========================================================================
// what git's listing says
// ===========================================================================

/// What one worktree has checked out, as `worktree list --porcelain` says it.
///
/// Two arms rather than an optional branch, because a detached worktree's
/// commits are as losable as a branch's and the reachability question is asked
/// about the *commit* either way. An absent branch that meant "ask nothing"
/// would be the answer that deletes.
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

    /// What the clone is asked about it: the full `refs/heads/…` spelling for a
    /// branch, so a branch and a tag of one name cannot be taken for each other,
    /// and the commit itself for a detached head.
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
///
/// There is deliberately no `prunable` field. `prunable` is git's claim about
/// whether `<recorded>/.git` exists as a filesystem entry — a fact about a path
/// this module's whole discipline is to not resolve — and reading it is
/// resolving by proxy (devlaunch#446 §5c). Nothing here reads it, and the
/// `Held` arm that used to hang off it is gone with it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Registration {
    /// Where inside a clone the registration sits — the join key.
    inside: Inside,
    /// The path git printed. See [`Recorded`].
    recorded: Recorded,
    head: WorktreeHead,
    locked: Option<Lock>,
}

/// Every registration under a `.claude/worktrees/`, from
/// `worktree list --porcelain`.
///
/// The clone's own entry, and any worktree registered somewhere else entirely,
/// are dropped here: this module reasons about sites inside one clone, and a
/// registration outside has no site here to be joined to.
fn registrations(listing: &str) -> Vec<Registration> {
    let mut found = Vec::new();
    for paragraph in listing.split("\n\n") {
        let mut inside = None;
        let mut recorded = None;
        let mut reference = None;
        let mut commit = None;
        let mut locked = None;
        for line in paragraph.lines() {
            let (key, rest) = match line.split_once(' ') {
                Some((key, rest)) => (key, Some(rest)),
                None => (line, None),
            };
            match (key, rest) {
                ("worktree", Some(path)) => {
                    inside = inside_a_worktrees_dir(Path::new(path));
                    recorded = Some(Recorded {
                        path: PathBuf::from(path),
                    });
                }
                ("HEAD", Some(sha)) => commit = Some(sha.to_owned()),
                ("branch", Some(name)) => reference = Some(name.to_owned()),
                ("locked", reason) => {
                    locked = Some(Lock {
                        reason: reason.map(str::to_owned).filter(|it| !it.is_empty()),
                    });
                }
                _ => {}
            }
        }
        let (Some(inside), Some(recorded), Some(commit)) = (inside, recorded, commit) else {
            continue;
        };
        let head = match reference {
            Some(reference) => WorktreeHead::Branch { reference, commit },
            None => WorktreeHead::Detached { commit },
        };
        found.push(Registration {
            inside,
            recorded,
            head,
            locked,
        });
    }
    found
}

/// Where in a clone `path` sits, as the join key, when it sits under a
/// `.claude/worktrees/` at all.
///
/// The suffix from the *first* `.claude/worktrees` onwards, so a nested worktree
/// keeps its whole path inside the clone and cannot be confused with a
/// same-named one at the top. Read off the components git printed rather than
/// off the filesystem, because the point of this module is that the path is not
/// a path here.
fn inside_a_worktrees_dir(path: &Path) -> Option<Inside> {
    let parts: Vec<String> = path
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    let at = (0..parts.len().saturating_sub(2)).find(|&at| {
        parts[at] == WORKTREES_DIR[0] && parts[at + 1] == WORKTREES_DIR[1] && at + 2 < parts.len()
    })?;
    Some(Inside {
        place: parts[at..].join("/"),
    })
}

/// Where `directory` sits inside `clone`, as the same join key.
fn inside_the_clone(clone: &Path, directory: &Path) -> Option<Inside> {
    let relative = directory.strip_prefix(clone).ok()?;
    inside_a_worktrees_dir(relative)
}

/// Whether `directory`'s own `.git` gitfile names a `…/.git/worktrees/<name>`
/// admin directory — the test for *is this a linked worktree at all*.
///
/// **Never the test for whose.** The tail says "a git worktree, registered under
/// some repository's admin directory", and *some* is the whole of what it says:
/// reading it as *this clone's* is the defect devlaunch#463 measured, a live
/// foreign worktree destroyed unopposed. Ownership is [`ClonePicture`]'s join.
///
/// A `..` tail would name the clone's own `.git` and `.` its `.git/worktrees`,
/// and the tail is file content, which is not trusted to be a name
/// (devlaunch#442 review, S6) — those shapes read as *not a worktree*, which
/// stands the site.
fn is_a_linked_worktree(directory: &Path) -> Option<bool> {
    let gitfile = directory.join(".git");
    let Ok(stat) = std::fs::symlink_metadata(&gitfile) else {
        // No `.git` at all: a plain directory, which the caller tells apart
        // from a gitfile that would not read.
        return Some(false);
    };
    if !stat.is_file() {
        return Some(false);
    }
    let content = std::fs::read_to_string(&gitfile).ok()?;
    let Some(named) = content.trim().strip_prefix(GITFILE_PREFIX) else {
        return Some(false);
    };
    let parts: Vec<String> = Path::new(named.trim())
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    let Some((name, rest)) = parts.split_last() else {
        return Some(false);
    };
    let Some((worktrees, rest)) = rest.split_last() else {
        return Some(false);
    };
    let Some(dot_git) = rest.last() else {
        return Some(false);
    };
    if name == "." || name == ".." || name.is_empty() {
        return Some(false);
    }
    Some(dot_git == ADMIN_DIR[0] && worktrees == ADMIN_DIR[1])
}

// ===========================================================================
// the picture: one listing, one admin readdir, taken together
// ===========================================================================

/// What this clone's own records say, read once per pass.
///
/// The acting pass has to classify every site *again* immediately before acting,
/// and it must re-read git to do that. Sharing this type is what keeps the two
/// passes asking the same question in the same words.
pub(crate) struct ClonePicture {
    registered: Vec<Registration>,
    /// Each admin directory under `<clone>/.git/worktrees/`, with the recorded
    /// path its own `gitdir` file names. The *path* half comes from a readdir of
    /// the clone's own `.git`, never from file content; the content half is used
    /// as a comparison key against the listing and nothing else.
    admins: Vec<AdminDir>,
}

struct AdminDir {
    path: PathBuf,
    /// The `gitdir` file's content, trimmed: `<recorded path>/.git`.
    records: String,
}

impl ClonePicture {
    /// What git says about `clone`, or nothing when git will not say.
    ///
    /// A refusal takes the whole clone out of the sweep. Reading one as "git
    /// named no registrations" would classify every site as unaccounted-for,
    /// and while that direction now *stands* sites rather than deleting them, a
    /// plan built on a refusal is still a plan that is not about the clone.
    pub(crate) fn of(git: &Git<'_>, clone: &Path) -> Option<Self> {
        let listing = git.worktree_listing(clone).said()?;
        Some(Self {
            registered: registrations(&listing),
            admins: admin_dirs(clone),
        })
    }

    /// The admin directory recording exactly `recorded`, when there is one.
    ///
    /// The join is the whole recorded path as one string against the whole
    /// `gitdir` content — `git worktree list` itself is generated from those
    /// same files, so the two spell the path identically — and it is what makes
    /// an admin directory a *channel* derived from the join rather than
    /// ownership evidence: a foreign worktree whose leaf name collides with a
    /// name this clone has an admin directory for can never be handed that
    /// admin directory, because no registration of this clone records its path
    /// (devlaunch#463).
    fn admin_for(&self, recorded: &Recorded) -> Option<&Path> {
        let names = format!("{}/.git", recorded.as_path().display());
        self.admins
            .iter()
            .find(|admin| admin.records == names)
            .map(|admin| admin.path.as_path())
    }

    fn joined_to(&self, inside: &Inside) -> Vec<&Registration> {
        self.registered
            .iter()
            .filter(|it| &it.inside == inside)
            .collect()
    }

    /// Registrations under a `.claude/worktrees/` with nothing at their place in
    /// this clone. The place is a join of the clone root and the suffix, built
    /// for one existence check; the recorded path itself is never resolved.
    ///
    /// **`symlink_metadata` rather than `exists`, to agree with the walk.**
    /// `Path::exists` follows links, so a *dangling* symlink at a registered
    /// place reads as absent here and as a present entry to [`walk_sites`] —
    /// and the two answers together produce two sites for one `Inside`, one of
    /// them offering a forget for a registration whose place is occupied. The
    /// walk is the authority on what is there, so this asks the walk's question.
    fn nothing_at_their_place(&self, clone: &Path) -> Vec<&Registration> {
        self.registered
            .iter()
            .filter(|it| std::fs::symlink_metadata(clone.join(&it.inside.place)).is_err())
            .collect()
    }
}

/// Every admin directory under `<clone>/.git/worktrees/`, with what its `gitdir`
/// file records.
fn admin_dirs(clone: &Path) -> Vec<AdminDir> {
    let root = clone.join(ADMIN_DIR[0]).join(ADMIN_DIR[1]);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let path = entry.path();
            let records = std::fs::read_to_string(path.join("gitdir")).ok()?;
            Some(AdminDir {
                path,
                records: records.trim().to_owned(),
            })
        })
        .collect()
}

// ===========================================================================
// the forest of sites
// ===========================================================================

/// What occupies one site. Three arms, no catch-all.
#[derive(Debug)]
enum SiteKind {
    /// A linked worktree of this clone: a registration from this clone's own
    /// listing joins the place. `admin` is derived from the joined registration
    /// inside [`ours_here`] and never taken from a gitfile name, so "ours on the
    /// strength of a gitfile" has no representation.
    OursHere {
        at: PathBuf,
        joined: NonEmpty<Joined>,
        admin: Option<PathBuf>,
    },
    /// Registered in this clone's listing, with nothing at its place. No bytes;
    /// the forget is the whole of the work — and it still needs the reachability
    /// question answered, because a registration can be the last ref reaching a
    /// detached worktree's commits (devlaunch#446 §5a).
    OursGone { joined: NonEmpty<Joined> },
    /// A directory in the worktrees place this clone's listing cannot account
    /// for. Never provable safe from here, so never collectable.
    NotOurs { at: PathBuf, why: Unaccountable },
}

/// One registration joined to a site.
#[derive(Debug, Clone)]
struct Joined {
    recorded: Recorded,
    head: WorktreeHead,
    locked: Option<Lock>,
}

impl From<&Registration> for Joined {
    fn from(registration: &Registration) -> Self {
        Self {
            recorded: registration.recorded.clone(),
            head: registration.head.clone(),
            locked: registration.locked.clone(),
        }
    }
}

/// Why a site is not this clone's to account for.
///
/// Named arms rather than one, because the report's words differ and only
/// [`Unaccountable::RegisteredElsewhere`] has a named reclaimer: the repository
/// that registered it, which is the only party that can complete the removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unaccountable {
    /// A linked worktree — its gitfile names some repository's admin directory —
    /// that this clone's listing does not account for. A worktree of a different
    /// repository, or one whose registration a routine `git gc` expired; the two
    /// are indistinguishable from here, and neither is this module's to remove.
    RegisteredElsewhere,
    /// A `.git` that could not be read as a worktree gitfile: unreadable, or a
    /// tail that does not normalise to `…/.git/worktrees/<name>`.
    GitfileUnreadable,
    /// No gitfile at all: a plain directory somebody put here.
    PlainDirectory,
    /// A symbolic link. Never followed — following one walks a removal out of
    /// the tree `--prune` is scoped to.
    SymlinkInThePlace,
}

/// One site: a place inside the clone, what occupies it, and every site nested
/// inside it. `nested` comes from the walk, which is the one source of
/// containment edges.
#[derive(Debug)]
struct Site {
    inside: Inside,
    kind: SiteKind,
    nested: Vec<Site>,
}

impl Site {
    /// Where the site sits on this host, for the report and for the removal.
    /// The clone root joined with the walk's own suffix — never a recorded path.
    fn at(&self, clone: &Path) -> PathBuf {
        clone.join(&self.inside.place)
    }

    /// Every path in this subtree that is a site, this one included — what the
    /// dirt filter uses to tell "accounted for by the forest" apart from plain
    /// content.
    fn paths_into(&self, clone: &Path, into: &mut Vec<PathBuf>) {
        into.push(self.at(clone));
        for child in &self.nested {
            child.paths_into(clone, into);
        }
    }
}

/// Build one clone's forest: nodes from the union of the walk and the listing,
/// edges from the walk alone.
fn forest_of(clone: &Path, picture: &ClonePicture) -> Vec<Site> {
    let mut joined_somewhere: Vec<&Inside> = Vec::new();
    let mut roots = walk_sites(clone, clone, picture, &mut joined_somewhere);
    // Registrations with nothing at their place become sites of their own. They
    // have no filesystem presence, so the walk gives them no edges: they stand
    // beside the walked roots, and their whole work is the forget.
    let mut gone: Vec<&Registration> = picture
        .nothing_at_their_place(clone)
        .into_iter()
        .filter(|registration| !joined_somewhere.contains(&&registration.inside))
        .collect();
    gone.sort_by(|left, right| left.inside.cmp(&right.inside));
    let mut by_place: Vec<(Inside, Vec<Joined>)> = Vec::new();
    for registration in gone {
        match by_place
            .iter_mut()
            .find(|(place, _)| place == &registration.inside)
        {
            Some((_, list)) => list.push(Joined::from(registration)),
            None => by_place.push((
                registration.inside.clone(),
                vec![Joined::from(registration)],
            )),
        }
    }
    for (inside, list) in by_place {
        let joined = NonEmpty::of(list).expect("grouped from at least one registration");
        roots.push(Site {
            inside,
            kind: SiteKind::OursGone { joined },
            nested: Vec::new(),
        });
    }
    roots
}

/// The sites directly under `holder`'s `.claude/worktrees/`, each with its own
/// subtree. `holder` is the clone itself at the top and a site's directory below.
fn walk_sites<'p>(
    clone: &Path,
    holder: &Path,
    picture: &'p ClonePicture,
    joined_somewhere: &mut Vec<&'p Inside>,
) -> Vec<Site> {
    let root = worktrees_dir(holder);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut children: Vec<(PathBuf, bool)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let kind = entry.file_type().ok()?;
            if kind.is_symlink() {
                Some((entry.path(), true))
            } else if kind.is_dir() {
                Some((entry.path(), false))
            } else {
                // A file directly in the worktrees place is not a site. It is
                // untracked content, which the holder's own dirt question keeps.
                None
            }
        })
        .collect();
    children.sort();
    let mut sites = Vec::new();
    for (at, is_symlink) in children {
        let Some(inside) = inside_the_clone(clone, &at) else {
            continue;
        };
        if is_symlink {
            sites.push(Site {
                inside,
                kind: SiteKind::NotOurs {
                    at,
                    why: Unaccountable::SymlinkInThePlace,
                },
                nested: Vec::new(),
            });
            continue;
        }
        let joined = picture.joined_to(&inside);
        let kind = if joined.is_empty() {
            SiteKind::NotOurs {
                at: at.clone(),
                why: match is_a_linked_worktree(&at) {
                    Some(true) => Unaccountable::RegisteredElsewhere,
                    Some(false) => match std::fs::symlink_metadata(at.join(".git")) {
                        Ok(_) => Unaccountable::GitfileUnreadable,
                        Err(_) => Unaccountable::PlainDirectory,
                    },
                    None => Unaccountable::GitfileUnreadable,
                },
            }
        } else {
            joined_somewhere.extend(joined.iter().map(|it| &it.inside));
            ours_here(&at, &joined, picture)
        };
        // Descend whatever the kind. The verdict recursion covers everything —
        // a worktree of ours nested inside a foreign site is still ours and
        // still collectable on its own proof — where the byte recursion stops at
        // the outermost thing that goes. The two recursions are not the same
        // recursion, and conflating them was the T1 hole.
        let nested = walk_sites(clone, &at, picture, joined_somewhere);
        sites.push(Site {
            inside,
            kind,
            nested,
        });
    }
    sites
}

/// The one constructor for "this site is ours": takes the joined registrations
/// and derives the admin channel from them.
fn ours_here(at: &Path, joined: &[&Registration], picture: &ClonePicture) -> SiteKind {
    let admin = joined
        .iter()
        .find_map(|registration| picture.admin_for(&registration.recorded))
        .map(Path::to_path_buf);
    let joined = NonEmpty::of(joined.iter().map(|it| Joined::from(*it)))
        .expect("ours_here is called only with a join in hand");
    SiteKind::OursHere {
        at: at.to_path_buf(),
        joined,
        admin,
    }
}

// ===========================================================================
// the verdict (devlaunch#446)
// ===========================================================================

/// What this pass established about one site, or about a clone as the root of
/// the same forest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Every question that bears on this subtree was put, and answered clear.
    Collectable(Proof),
    /// At least one reason it stands, and reasons accumulate up the subtree.
    Stands(Standing),
}

/// The witness that every question was asked and answered clear.
///
/// Private fields, no `Default`, no public constructor: "nothing objected" and
/// "nothing was asked" are different values, and `Safe` is not the fallthrough
/// of a filter — it is a thing you have to be handed. The `public-api.rest.txt`
/// snapshot is where the absent constructor is pinned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proof {
    how: ProofHow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProofHow {
    /// A present worktree of ours: dirt, reachability and the lock all answered.
    EverythingAsked {
        _clean: Clean,
        _elsewhere: Elsewhere,
        _unclaimed: Unclaimed,
    },
    /// A registered site with nothing at it: no bytes, and the commits the
    /// registration still names were found somewhere else. Emptiness alone never
    /// carries a registered site.
    HoldsNothing {
        _empty: NoBytes,
        _elsewhere: Elsewhere,
    },
    /// The clone root's own probes answered clear. Minted from
    /// [`Unsaved::NothingToLose`], whose own discipline (devlaunch#171) is that
    /// it is produced only by probes that answered.
    CloneProbesAnsweredClear,
    /// devlaunch has no clone of its own here, so there is nothing of ours to
    /// protect and no business inspecting somebody's checkout to find something.
    NothingOfOurs,
}

/// Q2 answered: the working tree holds nothing that exists nowhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Clean(());

/// Q3 answered: every commit reachable from here is reachable from a ref in a
/// repository this pass does not remove — as of the last fetch, which is what
/// the report says.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Elsewhere(());

/// Q4 answered: no third party asserts a claim on the site.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Unclaimed(());

/// The bounded walk found nothing at the site.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NoBytes(());

/// At least one reason a subtree stands. Non-empty by construction, and the
/// fields are private so "stands, and here is the empty list" has no
/// representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
    first: Reason,
    rest: Vec<Reason>,
}

impl Standing {
    fn one(reason: Reason) -> Self {
        Self {
            first: reason,
            rest: Vec::new(),
        }
    }

    fn of(reasons: Vec<Reason>) -> Option<Self> {
        let mut reasons = reasons.into_iter();
        let first = reasons.next()?;
        Some(Self {
            first,
            rest: reasons.collect(),
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = &Reason> {
        std::iter::once(&self.first).chain(self.rest.iter())
    }
}

/// One thing standing a subtree: proved unsafe, or could not be proved. Each
/// names the place it came from, which is what makes a parent's refusal report
/// the *child* that caused it rather than the parent it pinned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// A probe answered, and the answer was work that exists nowhere else.
    ///
    /// Boxed, and the reason is the shape of everything around it: a `Reason`
    /// rides inside a `Standing`, which rides inside a `Verdict`, which is the
    /// answer every clone and every site is carried as — most of them
    /// collectable, holding nothing. Inline, one arm's losses set the size of
    /// all of them. The indirection is paid only where there is a loss to
    /// describe.
    Holds { at: Place, losses: Box<Losses> },
    /// A question could not be put, or a party this pass cannot interrogate has
    /// a claim. An unproved is never reported as a loss: that would be inventing
    /// work that may not exist.
    CouldNotProve { at: Place, blank: Blank },
}

impl Reason {
    /// The same reason, said about `at` instead.
    ///
    /// Only for a memoised reachability answer, which is a fact about a commit
    /// rather than about a place: the site it was first asked from is not the
    /// site a later reader is deciding about, and a report that named the first
    /// one would send somebody to the wrong directory.
    fn about(mut self, at: &Place) -> Self {
        match &mut self {
            Self::Holds { at: was, .. } | Self::CouldNotProve { at: was, .. } => {
                *was = at.clone();
            }
        }
        self
    }

    pub fn at(&self) -> &Place {
        match self {
            Self::Holds { at, .. } | Self::CouldNotProve { at, .. } => at,
        }
    }

    /// Whose assertion this reason is — a claim by a party that may be using
    /// the site, or an account of what content is provable.
    ///
    /// Wildcard-free on purpose: this is the fold devlaunch#468's derivative
    /// reclaim reads (a tagged environment inside a standing site is still
    /// derivable unless a *claimant* stands the site), so a new arm added here
    /// must answer it explicitly rather than inheriting a default.
    pub fn subject(&self) -> Subject {
        match self {
            Self::Holds { .. } => Subject::GitsAccountOfContent,
            Self::CouldNotProve { blank, .. } => match blank {
                Blank::ThirdPartyClaim(_) => Subject::AClaim,
                // Somebody may be working in it right now; that is the whole
                // reason it was not in the plan. A claimant, so #468's
                // derivative reclaim does not reach into it either.
                Blank::AppearedAfterThePlan => Subject::AClaim,
                // A worktree, which is a claim on the directory holding it by
                // something the tag does not speak for.
                Blank::ASiteSitsInside => Subject::AClaim,
                Blank::NothingToAskThrough
                | Blank::GitWouldNotSay(_)
                // Decided on devlaunch#468: another repository's env, tagged, is
                // still derivable, and whose repository it is was never part of
                // that argument.
                | Blank::NotThisClonesToAccountFor(_) => Subject::GitsAccountOfContent,
            },
        }
    }
}

impl Verdict {
    /// The `unsaved` value `dl --ls --json` prints — the one place the verdict
    /// is flattened to the wire, and the flattening is additive rather than
    /// lossy: a standing that holds both kinds of reason emits **both**
    /// `wouldLose` and `couldNotTell`, so no reader keyed on key presence breaks
    /// and no answer is dropped to fit the shape (devlaunch#446 §6). A clone
    /// with no agent worktrees reads exactly as it always did.
    pub fn unsaved_json(&self) -> serde_json::Value {
        match self {
            Self::Collectable(_) => serde_json::json!({ "nothingToLose": true }),
            Self::Stands(standing) => {
                let mut value = serde_json::json!({});
                if let Some(holds) = standing.would_lose() {
                    value["wouldLose"] = serde_json::Value::String(holds);
                }
                if let Some(blanks) = standing.could_not_tell() {
                    value["couldNotTell"] = serde_json::Value::String(blanks);
                }
                value
            }
        }
    }
}

impl Standing {
    /// Every proved loss in here, in words, or nothing when every reason is an
    /// unproved. The wire payload behind `wouldLose`.
    pub fn would_lose(&self) -> Option<String> {
        let parts: Vec<String> = self
            .iter()
            .filter_map(|reason| match reason {
                Reason::Holds { at, losses } => Some(located(&losses.describe(), at)),
                Reason::CouldNotProve { .. } => None,
            })
            .collect();
        joined(parts)
    }

    /// Every unproved in here, in words, or nothing when every reason is a
    /// proved loss. The wire payload behind `couldNotTell`. An unproved is never
    /// folded into [`Standing::would_lose`]: reporting a lock or a refused probe
    /// as a loss would be inventing work that may not exist.
    pub fn could_not_tell(&self) -> Option<String> {
        let parts: Vec<String> = self
            .iter()
            .filter_map(|reason| match reason {
                Reason::Holds { .. } => None,
                Reason::CouldNotProve { at, blank } => Some(located(&blank.describe(), at)),
            })
            .collect();
        joined(parts)
    }

    /// The whole standing in words, both kinds, for a refusal a person reads.
    pub fn describe(&self) -> String {
        let parts: Vec<String> = self
            .iter()
            .map(|reason| match reason {
                Reason::Holds { at, losses } => located(&losses.describe(), at),
                Reason::CouldNotProve { at, blank } => located(&blank.describe(), at),
            })
            .collect();
        parts.join(" and ")
    }

    // `any_unproved` lived here and is deleted rather than kept for symmetry.
    // It was written for the refusal's wording and never called: the render
    // sites match on which words exist, which answers the same question without
    // asking a second one that could disagree. `Standing` is in the binary
    // surface residual, so an uncalled reader there is rows a consumer can bind
    // to for nothing (devlaunch#531).
}

fn located(words: &str, at: &Place) -> String {
    match at {
        Place::TheCloneItself => words.to_owned(),
        Place::ASite(inside) => format!("{words} (in {})", inside.as_str()),
    }
}

fn joined(parts: Vec<String>) -> Option<String> {
    (!parts.is_empty()).then(|| parts.join(" and "))
}

impl Blank {
    /// The words a report interpolates for one unproved. Like
    /// `CouldNotTell::describe`, this is a wire payload rather than rendering:
    /// it reaches a tool through `--ls --json`'s `couldNotTell`.
    pub fn describe(&self) -> String {
        match self {
            Self::NothingToAskThrough => {
                "there is nothing to ask git through about what it holds".to_owned()
            }
            Self::GitWouldNotSay(cause) => cause.describe(),
            Self::ThirdPartyClaim(None) => "git is holding it locked".to_owned(),
            Self::ThirdPartyClaim(Some(reason)) => {
                format!("git is holding it locked ({reason})")
            }
            Self::AppearedAfterThePlan => {
                "it appeared inside this directory after the plan was printed, so nobody has \
                 said yes to removing it"
                    .to_owned()
            }
            Self::ASiteSitsInside => {
                "a git worktree sits inside it, and whatever declared it regenerable was not \
                 speaking for that"
                    .to_owned()
            }
            Self::NotThisClonesToAccountFor(why) => why.describe().to_owned(),
        }
    }
}

impl Unaccountable {
    /// One sentence per arm, each naming the fact it rests on — and for
    /// [`Unaccountable::RegisteredElsewhere`], the only party that can end it.
    pub fn describe(&self) -> &'static str {
        match self {
            Self::RegisteredElsewhere => {
                "a git worktree this clone's listing does not account for; devlaunch will \
                 never reclaim it, and only the repository that registered it can"
            }
            Self::GitfileUnreadable => {
                "its .git could not be read as a worktree gitfile, so it is not devlaunch's \
                 to remove"
            }
            Self::PlainDirectory => {
                "a plain directory, not a git worktree, so it is not devlaunch's to remove"
            }
            Self::SymlinkInThePlace => {
                "a symbolic link, which devlaunch never follows and never removes"
            }
        }
    }
}

/// What kind of thing asserts a standing reason. See [`Reason::subject`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    /// A party that may be using the site: a lock, or anything else that speaks
    /// for itself rather than about bytes.
    AClaim,
    /// An account of what content could or could not be proved safe.
    GitsAccountOfContent,
}

#[cfg(test)]
impl Verdict {
    /// A collectable for unit tests with no probe to run. Test-only: the
    /// production constructors all take witnesses a probe minted, and this
    /// crate's `public-api.rest.txt` pins that no public constructor exists.
    pub(crate) fn test_collectable() -> Self {
        Self::Collectable(Proof {
            how: ProofHow::NothingOfOurs,
        })
    }

    /// A standing for unit tests, from at least one reason.
    pub(crate) fn test_stands(reasons: Vec<Reason>) -> Self {
        Self::Stands(Standing::of(reasons).expect("test_stands takes at least one reason"))
    }
}

#[cfg(test)]
impl Standing {
    pub(crate) fn test_of(reasons: Vec<Reason>) -> Self {
        Self::of(reasons).expect("test_of takes at least one reason")
    }
}

/// Why a question could not be put, one arm per cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blank {
    /// No admin directory and no registration: the working tree cannot be asked
    /// anything at all, and there are bytes here.
    NothingToAskThrough,
    /// A probe ran and refused. Carries git's words.
    GitWouldNotSay(CouldNotTell),
    /// `locked`, with its reason where git printed one.
    ThirdPartyClaim(Option<String>),
    /// This site is not this clone's to account for; see [`Unaccountable`].
    NotThisClonesToAccountFor(Unaccountable),
    /// A site inside an approved subtree that the plan did not name, found by
    /// the acting pass. Nothing establishes that the person who said yes to the
    /// plan meant this one, so the whole unit is withheld and offered again next
    /// run — by which time it is in the plan they read.
    AppearedAfterThePlan,
    /// A site sits inside a tagged directory, so whatever declared that
    /// directory regenerable was not speaking for what is in it (devlaunch#468
    /// §6). Produced by the derivative fold alone and never by a site's own
    /// verdict: a site is never *this* to itself.
    ASiteSitsInside,
}

// ===========================================================================
// the probes
// ===========================================================================

/// Whether the sibling bare cache's refs already reach a commit.
///
/// **This is the fix for a stale-ref trap and not a nicety.** A workspace clone
/// is cut from the sibling `.bare` and then has its remote repointed at the
/// forge, with no fetch of its own (see `flows::workspace_clone`'s module
/// header), so the clone's `refs/remotes/origin/*` is as of clone time and can
/// be arbitrarily old or absent. The `.bare` next door is the thing that gets
/// fetched. Ask the clone alone and branches that were pushed and merged months
/// ago read as unpushed, which keeps every byte forever and makes the flag the
/// only way to reclaim anything.
///
/// Both answers are **as of the last fetch**, which is what the report says. No
/// network call is added here: `--prune` is a local cleanup, and one that failed
/// offline would be a worse command than one that is sometimes out of date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InTheCache {
    Reached,
    /// The cache does not reach it — including the case where it has never seen
    /// the commit at all, which is what an unpushed branch looks like from
    /// there.
    Beyond,
    CouldNotSay,
}

fn in_the_cache(git: &Git<'_>, bare: Option<&Path>, commit: &str) -> InTheCache {
    let Some(bare) = bare else {
        return InTheCache::CouldNotSay;
    };
    match git.commits_beyond_every_ref(bare, commit).said() {
        // A refusal here is overwhelmingly `bad object` — the cache has never
        // seen this commit, which is exactly what an unpushed branch looks like
        // from the cache and is a fact, not a failure. The clone is asked next,
        // and only both failing to find the work anywhere stands the site.
        None => InTheCache::Beyond,
        Some(count) => match count.trim().parse::<u64>() {
            Ok(0) => InTheCache::Reached,
            Ok(_) => InTheCache::Beyond,
            Err(_) => InTheCache::CouldNotSay,
        },
    }
}

/// Everything the probes need to weigh one clone's forest.
struct Weigher<'a, 'r> {
    git: &'a Git<'r>,
    clone: &'a Path,
    bare: Option<&'a Path>,
    /// What Q3 already answered, keyed on the revision it was asked about.
    ///
    /// The question is about a commit and a repository, and both are fixed for
    /// the whole of one clone's weighing, so two sites cut from the same commit
    /// asked it twice and got the same answer twice. On the reference host's
    /// shape that is most of the git this pass runs. Keyed on the revision
    /// rather than on the site, because the revision is the whole of what the
    /// answer depends on — a cache keyed on anything wider would be a second
    /// definition of what the question is.
    ///
    /// Held for one `weigh_clone` call and dropped with it, so nothing here
    /// survives into a later pass with a staler answer than the pass that took
    /// it. `Result` rather than the witness alone: a refusal is an answer and
    /// re-asking it would not make it a different one.
    reachability: RefCell<HashMap<String, Result<Elsewhere, Reason>>>,
    /// Whether this pass costs the tagged derivatives inside the sites it
    /// stands. See [`Derivatives`].
    derivatives: Derivatives,
}

impl Weigher<'_, '_> {
    /// Q3 for one head: the commits it reaches exist somewhere else, or the
    /// reason they could not be shown to.
    ///
    /// The cache goes first, for [`InTheCache`]'s reason. The clone's probe is
    /// keyed on this site's own revision rather than on `--all`, because a loss
    /// has to be attributed to the directory holding it — the clone-scope
    /// `--all` question is asked once of the clone itself, elsewhere.
    fn elsewhere(&self, at: &Place, head: &WorktreeHead) -> Result<Elsewhere, Reason> {
        if let Some(answered) = self.reachability.borrow().get(head.revision()) {
            // The reason names the site it came from, and this one came from
            // another: re-attribute it so a report still names the directory it
            // is about. The answer itself is the same answer.
            return answered.clone().map_err(|reason| reason.about(at));
        }
        let answer = self.ask_elsewhere(at, head);
        self.reachability
            .borrow_mut()
            .insert(head.revision().to_owned(), answer.clone());
        answer
    }

    /// [`Weigher::elsewhere`] with the memo taken out of the way.
    fn ask_elsewhere(&self, at: &Place, head: &WorktreeHead) -> Result<Elsewhere, Reason> {
        match in_the_cache(self.git, self.bare, head.commit()) {
            InTheCache::Reached => Ok(Elsewhere(())),
            InTheCache::Beyond | InTheCache::CouldNotSay => {
                match self
                    .git
                    .unpushed_commits_from(self.clone, head.revision())
                    .said()
                {
                    None => Err(Reason::CouldNotProve {
                        at: at.clone(),
                        blank: Blank::GitWouldNotSay(CouldNotTell::UnpushedNotListed {
                            clone: self.clone.to_path_buf(),
                            reason: format!(
                                "neither the repository cache nor the clone could say whether \
                                 the commits on {} are anywhere else",
                                head.named()
                            ),
                        }),
                    }),
                    Some(unpushed) => match NonEmpty::of(unpushed.lines().map(str::to_owned)) {
                        None => Ok(Elsewhere(())),
                        Some(commits) => Err(Reason::Holds {
                            at: at.clone(),
                            // No tag account: this probe names one revision and
                            // asks the clone about it, where #522's account is
                            // about which of a clone's refs are tags the mirror
                            // does not have. A site has no such question.
                            losses: Box::new(Losses::one(Loss::Unpushed {
                                commits,
                                by_tags: None,
                            })),
                        }),
                    },
                }
            }
        }
    }

    /// Q2 for one present worktree, asked through the admin directory derived
    /// from the join.
    ///
    /// **Ignored content is not weighed, and it is not weighed at clone scope
    /// either — that is the point.** One conjunction wants one definition of
    /// what makes a tree dirty, and a clone's own ignored bytes have never
    /// counted, so counting them one level in would be the same rule written
    /// twice and disagreeing about the same bytes. It is a real limit rather
    /// than an oversight: `git worktree remove` deletes a worktree whose only
    /// content is gitignored, exit 0 and silent, and so does the removal here.
    ///
    /// The limit is stated rather than closed because closing it at this scope
    /// alone costs more than it buys. An installed `.pixi/envs/default` is
    /// ignored content and is the whole reason an agent worktree is worth
    /// reclaiming — 18 of the 72 on the reference host, and the difference
    /// between 104 GB and about 10 — so weighing it put every one of those
    /// directories behind `--force-worktrees`, which also carries past a lock
    /// and past another repository's worktree. Whether ignored bytes should be
    /// weighed is one question for both scopes, and [`Git::worktree_dirt`] and
    /// `docs/cleanup.md` carry it with its reason.
    ///
    /// An untracked or ignored entry is dropped from the answer only when
    /// everything under it is a site the child forest accounts for — each such
    /// site answers for itself, with its own verdict, attributed to its own
    /// path. Excluding by what a thing *is* rather than where it sits is
    /// devlaunch#442 review S4; a tracked change is never dropped, whatever its
    /// path.
    fn clean(
        &self,
        at: &Place,
        admin: &Path,
        directory: &Path,
        forest: &[PathBuf],
    ) -> Result<Clean, Reason> {
        let dirt = match self.git.worktree_dirt(admin, directory).said() {
            None => {
                return Err(Reason::CouldNotProve {
                    at: at.clone(),
                    blank: Blank::GitWouldNotSay(CouldNotTell::GitCouldNotRead {
                        clone: directory.to_path_buf(),
                        reason: "git could not read this worktree through the clone's admin \
                                 directory"
                            .to_owned(),
                    }),
                });
            }
            Some(dirt) => dirt,
        };
        let lines = dirt
            .lines()
            .filter(|line| !accounted_for_by_the_forest(directory, line, forest))
            .map(str::to_owned);
        match NonEmpty::of(lines) {
            None => Ok(Clean(())),
            Some(changed) => Err(Reason::Holds {
                at: at.clone(),
                losses: Box::new(Losses::one(Loss::Uncommitted(changed))),
            }),
        }
    }

    /// Q4: the listing already answered; this reads it.
    fn unclaimed(&self, at: &Place, joined: &NonEmpty<Joined>) -> Result<Unclaimed, Reason> {
        match joined.iter().find_map(|it| it.locked.clone()) {
            None => Ok(Unclaimed(())),
            Some(lock) => Err(Reason::CouldNotProve {
                at: at.clone(),
                blank: Blank::ThirdPartyClaim(lock.reason),
            }),
        }
    }

    /// This site's own verdict, children not consulted — the conjunction over
    /// children is [`weigh`]'s and only [`weigh`]'s.
    fn own_verdict(&self, site: &Site, forest: &[PathBuf]) -> Verdict {
        let at = Place::ASite(site.inside.clone());
        match &site.kind {
            SiteKind::NotOurs { why, .. } => {
                Verdict::Stands(Standing::one(Reason::CouldNotProve {
                    at,
                    blank: Blank::NotThisClonesToAccountFor(*why),
                }))
            }
            SiteKind::OursGone { joined } => {
                let mut reasons = Vec::new();
                let mut elsewhere = None;
                for one in joined.iter() {
                    match self.elsewhere(&at, &one.head) {
                        Ok(witness) => elsewhere = Some(witness),
                        Err(reason) => reasons.push(reason),
                    }
                }
                if let Err(reason) = self.unclaimed(&at, joined) {
                    reasons.push(reason);
                }
                match (Standing::of(reasons), elsewhere) {
                    (Some(standing), _) => Verdict::Stands(standing),
                    (None, Some(elsewhere)) => Verdict::Collectable(Proof {
                        how: ProofHow::HoldsNothing {
                            _empty: NoBytes(()),
                            _elsewhere: elsewhere,
                        },
                    }),
                    // Unreachable in practice — a joined site has at least one
                    // head to ask about — but the honest answer for it is that
                    // nothing was proved, not that nothing objected.
                    (None, None) => Verdict::Stands(Standing::one(Reason::CouldNotProve {
                        at,
                        blank: Blank::NothingToAskThrough,
                    })),
                }
            }
            SiteKind::OursHere {
                at: dir,
                joined,
                admin,
            } => {
                let mut reasons = Vec::new();
                let clean = match admin {
                    Some(admin) => match self.clean(&at, admin, dir, forest) {
                        Ok(witness) => Some(witness),
                        Err(reason) => {
                            reasons.push(reason);
                            None
                        }
                    },
                    // Registered a moment ago and its admin directory already
                    // gone: a concurrent prune's window. The working tree cannot
                    // be asked, and an unasked question is not a clean answer.
                    None => {
                        reasons.push(Reason::CouldNotProve {
                            at: at.clone(),
                            blank: Blank::NothingToAskThrough,
                        });
                        None
                    }
                };
                let mut elsewhere = None;
                for one in joined.iter() {
                    match self.elsewhere(&at, &one.head) {
                        Ok(witness) => elsewhere = Some(witness),
                        Err(reason) => reasons.push(reason),
                    }
                }
                let unclaimed = match self.unclaimed(&at, joined) {
                    Ok(witness) => Some(witness),
                    Err(reason) => {
                        reasons.push(reason);
                        None
                    }
                };
                match (Standing::of(reasons), clean, elsewhere, unclaimed) {
                    (Some(standing), _, _, _) => Verdict::Stands(standing),
                    (None, Some(clean), Some(elsewhere), Some(unclaimed)) => {
                        Verdict::Collectable(Proof {
                            how: ProofHow::EverythingAsked {
                                _clean: clean,
                                _elsewhere: elsewhere,
                                _unclaimed: unclaimed,
                            },
                        })
                    }
                    // No reason and no full witness set cannot happen — every
                    // missing witness pushed a reason — but the arm exists and
                    // it points towards keeping.
                    (None, _, _, _) => Verdict::Stands(Standing::one(Reason::CouldNotProve {
                        at,
                        blank: Blank::NothingToAskThrough,
                    })),
                }
            }
        }
    }
}

/// Whether one `git status` line is nothing but sites the forest already
/// accounts for.
///
/// Only ever true of an **untracked** entry on the `.claude/worktrees/` spine.
/// A tracked change under the same path is somebody's edit to a file the
/// repository knows about and is never dropped, and an entry anywhere else is
/// not this module's business. There is no ignored arm because the probe does
/// not ask for ignored entries; see [`Weigher::clean`].
fn accounted_for_by_the_forest(root: &Path, line: &str, forest: &[PathBuf]) -> bool {
    let Some(entry) = line.strip_prefix("?? ") else {
        return false;
    };
    // git quotes a path holding anything unusual. Quoted means unparsed here,
    // which reads as work, which is the direction that keeps the directory.
    if entry.starts_with('"') {
        return false;
    }
    let entry = Path::new(entry.trim_end_matches('/'));
    on_the_worktrees_spine(entry) && covered_by_sites(&root.join(entry), forest)
}

/// Whether `entry` is the `.claude/worktrees/` spine or something under it —
/// the cheap guard in front of the walk, so an untracked `build/` is not walked
/// in full to establish what everyone already knows.
fn on_the_worktrees_spine(entry: &Path) -> bool {
    let parts: Vec<String> = entry
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.first().is_some_and(|first| first == WORKTREES_DIR[0])
        && parts.get(1).is_none_or(|second| second == WORKTREES_DIR[1])
}

/// Whether every leaf under `path` is inside a site the forest holds.
///
/// The forest, not the gitfile: "shaped like a worktree" is not "accounted
/// for", and the tail-only reading here is the other half of the defect
/// devlaunch#463 measured. A site of any kind qualifies, because every site
/// carries its own verdict and pins its own ancestors — so dropping it from the
/// dirt answer trades an opaque `??` line for a report line that names it.
fn covered_by_sites(path: &Path, forest: &[PathBuf]) -> bool {
    if std::fs::symlink_metadata(path).is_ok_and(|it| it.is_symlink()) {
        return forest.iter().any(|site| site == path);
    }
    if forest.iter().any(|site| site == path) {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        // A file, or a directory that will not be read. Neither is a site, so
        // neither is something to drop from the answer.
        return false;
    };
    entries
        .filter_map(Result::ok)
        .all(|entry| covered_by_sites(&entry.path(), forest))
}

// ===========================================================================
// the plan: what goes, what stands
// ===========================================================================

/// One collectable unit, exactly matching one operation's radius.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Collectable {
    /// A subtree to remove, and every registration the one removal accounts
    /// for: this site's and every nested site's.
    Directory(GoingDirectory),
    /// No directory. The forget is the whole of the work.
    Registration(GoingRegistration),
}

/// A subtree that goes. Private fields — a value a caller could fill could pair
/// a removal with forgets that never rode on it, which is the state
/// devlaunch#445 makes unrepresentable. The `public-api.rest.txt` snapshot pins
/// the absent constructor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoingDirectory {
    at: PathBuf,
    usage: DiskUsage,
    forgets: Vec<Recorded>,
}

impl GoingDirectory {
    pub fn at(&self) -> &Path {
        &self.at
    }

    pub fn usage(&self) -> &DiskUsage {
        &self.usage
    }

    /// The registrations this one removal accounts for, in the order they will
    /// be forgotten. Each is a path a listing printed.
    pub fn forgets(&self) -> &[Recorded] {
        &self.forgets
    }
}

/// A registration with nothing at its place, going by name alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoingRegistration {
    place: Inside,
    forgets: Vec<Recorded>,
}

impl GoingRegistration {
    pub fn place(&self) -> &Inside {
        &self.place
    }

    pub fn forgets(&self) -> &[Recorded] {
        &self.forgets
    }
}

/// Nothing objected, or `--force-worktrees` carried this unit past what did.
///
/// Carried per unit rather than read from a run-wide flag: a plan-wide boolean
/// says "the user insisted" about every unit in the plan, including the ones
/// nothing objected to, and the acting pass would then skip its re-check for all
/// of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreePromotion {
    Unopposed,
    Insisted { despite: Standing },
}

impl WorktreePromotion {
    fn insistence(&self) -> Insistence {
        match self {
            Self::Unopposed => Insistence::NotInsisted,
            Self::Insisted { .. } => Insistence::Insisted,
        }
    }
}

/// One unit this run will act on, and on what authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Going {
    what: Collectable,
    promotion: WorktreePromotion,
}

impl Going {
    pub fn what(&self) -> &Collectable {
        &self.what
    }

    pub fn promotion(&self) -> &WorktreePromotion {
        &self.promotion
    }

    /// Every registration this one unit accounts for, whichever arm it is.
    ///
    /// The unit's blast radius over the metadata, in one place, so the acting
    /// pass compares the two passes' radii rather than their roots.
    pub fn forgets(&self) -> &[Recorded] {
        match &self.what {
            Collectable::Directory(directory) => directory.forgets(),
            Collectable::Registration(registration) => registration.forgets(),
        }
    }

    fn identity(&self) -> GoingIdentity<'_> {
        match &self.what {
            Collectable::Directory(directory) => GoingIdentity::At(&directory.at),
            Collectable::Registration(registration) => GoingIdentity::Place(&registration.place),
        }
    }
}

#[derive(PartialEq, Eq)]
enum GoingIdentity<'a> {
    At(&'a Path),
    Place(&'a Inside),
}

/// One site this run leaves standing, with its own reasons.
///
/// One entry per standing site, never one per pinned ancestor: an ancestor that
/// stands only because of what is nested in it is accounted for by the nested
/// site's own line, which names the child — a parent reported as "kept" with the
/// child unnamed is the invisible straggler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandingSite {
    at: PathBuf,
    reasons: Standing,
}

impl StandingSite {
    pub fn at(&self) -> &Path {
        &self.at
    }

    pub fn reasons(&self) -> &Standing {
        &self.reasons
    }
}

/// One clone's share of the sweep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneWorktrees {
    // Private, all of them: this is an answer, and a caller that could fill it
    // could pair one clone with a classification of another clone's sites.
    clone: PathBuf,
    owner: String,
    repo: String,
    going: Vec<Going>,
    standing: Vec<StandingSite>,
    /// The tagged derivative subtrees inside the sites this run is leaving
    /// standing (devlaunch#468). A derivative inside a site that is itself
    /// going is not in here: the site's own removal accounts for it, which is
    /// devlaunch#446 §6's two-recursions rule extended one artifact over.
    derivatives: Vec<Tagged>,
}

impl CloneWorktrees {
    pub fn clone_path(&self) -> &Path {
        &self.clone
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// The units this run will act on: directory removals biggest first, then
    /// bare forgets, path breaking ties, so two runs over an unchanged cache
    /// read alike.
    pub fn going(&self) -> &[Going] {
        &self.going
    }

    /// The sites this run leaves, each with its own reasons.
    pub fn standing(&self) -> &[StandingSite] {
        &self.standing
    }

    /// Every tagged derivative inside the sites this run leaves standing, the
    /// ones it will reclaim and the ones it will not, each with its bytes.
    pub fn derivatives(&self) -> &[Tagged] {
        &self.derivatives
    }

    fn nothing_to_do(&self) -> bool {
        self.going.is_empty() && !self.derivatives.iter().any(|it| it.derivable().is_some())
    }

    fn nothing_to_say(&self) -> bool {
        self.going.is_empty() && self.standing.is_empty() && self.derivatives.is_empty()
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
    pub fn clones(&self) -> &[CloneWorktrees] {
        &self.clones
    }

    /// What reclaiming the tagged derivatives inside the *standing* sites would
    /// free.
    ///
    /// Its own figure beside [`Self::freed`] rather than folded into it, for
    /// the reason `PrunePlan::clones_freed` gives about the clones: these are a
    /// different claim about a different set of directories — every one of them
    /// is inside a site this run has just said it is leaving — and one number
    /// covering both would describe neither.
    pub fn derivatives_freed(&self) -> DiskUsage {
        disk_usage::total_usage(self.clones.iter().flat_map(|clone| {
            clone
                .derivatives
                .iter()
                .filter_map(|it| it.derivable().map(|one| one.usage().clone()))
        }))
    }

    /// What the whole sweep would free.
    pub fn freed(&self) -> DiskUsage {
        disk_usage::total_usage(self.clones.iter().flat_map(|clone| {
            clone.going.iter().filter_map(|going| match &going.what {
                Collectable::Directory(directory) => Some(directory.usage.clone()),
                Collectable::Registration(_) => None,
            })
        }))
    }

    pub fn nothing_to_do(&self) -> bool {
        self.clones.iter().all(CloneWorktrees::nothing_to_do)
    }

    pub fn nothing_to_say(&self) -> bool {
        self.clones.is_empty()
    }

    pub(crate) fn record(&mut self, found: Option<CloneWorktrees>) {
        if let Some(found) = found.filter(|it| !it.nothing_to_say()) {
            self.clones.push(found);
        }
    }
}

/// What one subtree's weighing concluded.
struct Weighed {
    /// When the whole subtree may go: everything the one removal accounts for.
    /// `None` means something in it stands.
    removable: Option<Removable>,
    /// Going roots strictly inside this subtree, materialized because an
    /// ancestor stands. Empty while `removable` is `Some`.
    going: Vec<Going>,
    /// Standing sites in this subtree, own reasons only. Empty while
    /// `removable` is `Some` — an insisted subtree's reasons ride in `despite`.
    standing: Vec<StandingSite>,
    /// The tagged derivatives inside this subtree. Empty while `removable` is
    /// `Some`, and that emptiness is the two-recursions rule rather than an
    /// omission: a subtree that is going takes its derivatives with it, and
    /// billing them a second time is exactly the double count R3 forbids.
    derivatives: Vec<Tagged>,
}

/// The whole subtree, ready to be one [`Going`] if the parent absorbs it or to
/// be materialized if not.
struct Removable {
    /// The subtree root to remove, when there are bytes. `None` for a
    /// registration with nothing at its place.
    dir: Option<PathBuf>,
    place: Inside,
    forgets: Vec<Recorded>,
    /// Everything `--force-worktrees` carried this subtree past, child sites'
    /// reasons included, so the plan line names what insisting means here.
    despite: Vec<Reason>,
}

/// Weigh one site bottom-up: children first, then the conjunction.
///
/// This recursion is the T1 answer, so it is worth saying what cannot happen in
/// it: the collectable arm is reachable only when every child's recursion
/// handed one back, and the caller passes no child list — so "a parent goes
/// while a nested site stands" is not guarded against, it has no path.
///
/// `claims` is every claimant reason in force from an ancestor. It flows *down*
/// while the verdict folds up, which is why this site's own verdict is taken
/// before its children are weighed rather than after: a lock is a claim over
/// everything inside the directory it names, and the derivative fold one level
/// in has to know about it.
fn weigh(
    weigher: &Weigher<'_, '_>,
    site: &Site,
    insistence: Insistence,
    forest: &[PathBuf],
    claims: &[Reason],
) -> Weighed {
    let own = weigher.own_verdict(site, forest);
    let claims_here = claims_over(claims, &own);
    let children: Vec<Weighed> = site
        .nested
        .iter()
        .map(|child| weigh(weigher, child, insistence, forest, &claims_here))
        .collect();
    let own_removable: Option<Vec<Reason>> = match &own {
        Verdict::Collectable(_) => Some(Vec::new()),
        Verdict::Stands(standing) => match insistence {
            Insistence::Insisted => Some(standing.iter().cloned().collect()),
            Insistence::NotInsisted => None,
        },
    };
    let every_child_removable = children.iter().all(|child| child.removable.is_some());
    if let (Some(despite), true) = (own_removable, every_child_removable) {
        let mut merged = Removable {
            dir: match &site.kind {
                SiteKind::OursHere { at, .. } | SiteKind::NotOurs { at, .. } => Some(at.clone()),
                SiteKind::OursGone { .. } => None,
            },
            place: site.inside.clone(),
            forgets: own_forgets(site),
            despite,
        };
        for child in children {
            let removable = child
                .removable
                .expect("every_child_removable checked every one");
            merged.forgets.extend(removable.forgets);
            merged.despite.extend(removable.despite);
        }
        return Weighed {
            removable: Some(merged),
            going: Vec::new(),
            standing: Vec::new(),
            derivatives: Vec::new(),
        };
    }
    // Something here stands, so nothing above this site can go: materialize the
    // children that can go on their own, and attribute the standing to the
    // sites that stand of their own accord.
    let mut going = Vec::new();
    let mut standing = Vec::new();
    let mut derivatives = own_derivatives(weigher, site, &claims_here, forest);
    for child in children {
        if let Some(removable) = child.removable {
            going.push(materialize(removable));
        }
        going.extend(child.going);
        standing.extend(child.standing);
        derivatives.extend(child.derivatives);
    }
    if let Verdict::Stands(reasons) = own {
        standing.push(StandingSite {
            at: site.at(weigher.clone),
            reasons,
        });
    }
    Weighed {
        removable: None,
        going,
        standing,
        derivatives,
    }
}

/// The tagged derivatives inside one standing site's own directory.
///
/// Two arms take none. A registration with nothing at its place has no
/// directory to walk. And a **symlink** in the worktrees place is never walked:
/// following it is how a removal leaves the tree `--prune` is scoped to, which
/// is the same reason [`walk_sites`] never follows one either.
///
/// A worktree of another repository *does* get walked, and that is decided
/// rather than overlooked: devlaunch#468 §6 names
/// [`Blank::NotThisClonesToAccountFor`] explicitly, because whose repository a
/// tagged environment belongs to was never part of the argument — the tag and
/// the lockfile beside it say what they say either way.
fn own_derivatives(
    weigher: &Weigher<'_, '_>,
    site: &Site,
    claims: &[Reason],
    forest: &[PathBuf],
) -> Vec<Tagged> {
    if weigher.derivatives == Derivatives::NotAsked {
        return Vec::new();
    }
    match &site.kind {
        SiteKind::OursGone { .. } => Vec::new(),
        SiteKind::NotOurs {
            why: Unaccountable::SymlinkInThePlace,
            ..
        } => Vec::new(),
        SiteKind::OursHere { at, .. } | SiteKind::NotOurs { at, .. } => {
            tagged_in(weigher.clone, at, claims, forest)
        }
    }
}

/// The registrations this site's own removal accounts for. A foreign site's
/// registration lives in a listing devlaunch does not own, so there is nothing
/// to forget and **no git invocation ever names it** — under insistence
/// included.
fn own_forgets(site: &Site) -> Vec<Recorded> {
    match &site.kind {
        SiteKind::OursHere { joined, .. } | SiteKind::OursGone { joined } => {
            joined.iter().map(|it| it.recorded.clone()).collect()
        }
        SiteKind::NotOurs { .. } => Vec::new(),
    }
}

/// Turn one removable subtree into the unit the plan carries. The byte figure
/// is measured here, once, at the outermost thing that goes — the byte
/// recursion stops where the verdict recursion did not.
fn materialize(removable: Removable) -> Going {
    let promotion = match Standing::of(removable.despite) {
        None => WorktreePromotion::Unopposed,
        Some(despite) => WorktreePromotion::Insisted { despite },
    };
    let what = match removable.dir {
        Some(at) => Collectable::Directory(GoingDirectory {
            usage: disk_usage::exclusive_usage(&at),
            at,
            forgets: removable.forgets,
        }),
        None => Collectable::Registration(GoingRegistration {
            place: removable.place,
            forgets: removable.forgets,
        }),
    };
    Going { what, promotion }
}

/// Classify the agent worktrees inside one clone, as `--prune`'s planning pass.
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
    if std::fs::read_dir(worktrees_dir(clone)).is_err() {
        return None;
    }
    let picture = ClonePicture::of(git, clone)?;
    let weighed = weigh_clone(git, clone, bare, &picture, Derivatives::Weighed, |_| {
        insistence
    });
    Some(CloneWorktrees {
        clone: clone.to_path_buf(),
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        going: weighed.going,
        standing: weighed.standing,
        derivatives: weighed.derivatives,
    })
}

/// One clone's whole weighing: what goes, what stands, and the tagged
/// derivatives inside what stands.
///
/// A struct rather than a tuple because the third member arrived and a
/// three-tuple of `Vec`s is three chances to bind the wrong one.
struct Weighing {
    going: Vec<Going>,
    standing: Vec<StandingSite>,
    derivatives: Vec<Tagged>,
}

/// Weigh every root in one clone's forest, with an insistence per going root.
fn weigh_clone(
    git: &Git<'_>,
    clone: &Path,
    bare: Option<&Path>,
    picture: &ClonePicture,
    want: Derivatives,
    insist: impl Fn(&Site) -> Insistence,
) -> Weighing {
    let weigher = Weigher {
        git,
        clone,
        bare,
        reachability: RefCell::new(HashMap::new()),
        derivatives: want,
    };
    let roots = forest_of(clone, picture);
    let mut forest_paths = Vec::new();
    for root in &roots {
        root.paths_into(clone, &mut forest_paths);
    }
    let mut going = Vec::new();
    let mut standing = Vec::new();
    let mut derivatives = Vec::new();
    for root in &roots {
        let weighed = weigh(&weigher, root, insist(root), &forest_paths, &[]);
        if let Some(removable) = weighed.removable {
            going.push(materialize(removable));
        }
        going.extend(weighed.going);
        standing.extend(weighed.standing);
        derivatives.extend(weighed.derivatives);
    }
    going.sort_by(|left, right| {
        let bytes = |unit: &Going| match &unit.what {
            Collectable::Directory(directory) => Some(directory.usage.known_bytes()),
            Collectable::Registration(_) => None,
        };
        let path = |unit: &Going| match &unit.what {
            Collectable::Directory(directory) => directory.at.display().to_string(),
            Collectable::Registration(registration) => registration.place.place.clone(),
        };
        bytes(right)
            .cmp(&bytes(left))
            .then_with(|| path(left).cmp(&path(right)))
    });
    standing.sort_by(|left, right| left.at.cmp(&right.at));
    derivatives.sort_by(|left, right| {
        right
            .usage()
            .known_bytes()
            .cmp(&left.usage().known_bytes())
            .then_with(|| left.at().cmp(right.at()))
    });
    Weighing {
        going,
        standing,
        derivatives,
    }
}

// ===========================================================================
// the acting pass
// ===========================================================================

/// One unit the plan meant to act on that the acting pass would not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithheldWorktree {
    pub path: PathBuf,
    /// Why it is staying — and it is worth saying this was not so when the plan
    /// was printed.
    pub because: Standing,
}

/// One subtree the acting pass removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedWorktree {
    pub path: PathBuf,
    /// The figure the plan measured, so what somebody is told they got back is
    /// what they said yes to.
    pub usage: DiskUsage,
}

/// One forget git refused. The registration is still there, the next listing
/// still prints it, and the next run offers it again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgetRefused {
    /// The path git printed for the registration — reported as text, resolved by
    /// nothing.
    pub registered: PathBuf,
    pub reason: String,
}

/// What the acting pass did about the agent worktrees.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeReport {
    pub removed: Vec<RemovedWorktree>,
    pub withheld: Vec<WithheldWorktree>,
    /// Directories that would not come away. Not empty means the run is
    /// unfinished, and what did go is still gone. Nothing was forgotten for a
    /// unit in here: the forget is a consequence of a removal that completed.
    pub refused: Vec<Refusal>,
    /// Registrations dropped, by name, each one read from a listing.
    pub forgotten: usize,
    pub forget_refused: Vec<ForgetRefused>,
    /// The tagged derivative subtrees this run reclaimed (devlaunch#468).
    pub reclaimed: Vec<ReclaimedDerivative>,
    /// The ones the plan named that the re-read would not hand back.
    pub withheld_derivatives: Vec<WithheldDerivative>,
}

impl WorktreeReport {
    /// What this run actually freed by removing agent worktrees.
    pub fn freed(&self) -> DiskUsage {
        disk_usage::total_usage(self.removed.iter().map(|it| it.usage.clone()))
    }

    /// What reclaiming the tagged derivatives freed, with the plan's own
    /// figures. Its own number for [`WorktreeSweep::derivatives_freed`]'s
    /// reason: two claims about two disjoint sets of directories.
    pub fn derivatives_freed(&self) -> DiskUsage {
        disk_usage::total_usage(self.reclaimed.iter().map(|it| it.usage.clone()))
    }

    pub fn nothing_to_say(&self) -> bool {
        self.removed.is_empty()
            && self.withheld.is_empty()
            && self.refused.is_empty()
            && self.forgotten == 0
            && self.forget_refused.is_empty()
            && self.reclaimed.is_empty()
            && self.withheld_derivatives.is_empty()
    }
}

/// Carry out one clone's share of the sweep, and add what happened to `report`.
///
/// The caller holds the repository lock. **Every site is classified again,
/// under that lock, immediately before anything goes**, by the same weighing
/// the plan ran — one implementation, so the plan and the outcome cannot answer
/// different questions.
///
/// **Matching the two passes by identity is not enough, and that is worth
/// spelling out because it was wrong here once.** A unit's identity is its root;
/// its blast radius is a subtree. A site created inside an approved parent after
/// the plan was printed is weighed by this pass on its own merits, and if it is
/// collectable it is *absorbed into the parent's unit* — so the unit count is
/// unchanged, the identity matches, and something the plan never named goes out
/// with the removal and is handed to `git worktree remove`. Measured on this
/// tree: a plan naming one registration acted on two.
///
/// So a confirmed unit is acted on only when **every registration it now names
/// was named by the plan**. That is the subset check in [`grew_past`], and it is
/// the radius rather than the root: a fresh nested site of ours contributes its
/// registration and fails it, a fresh nested site that is *not* ours contributes
/// no registration but stands, which withholds the parent anyway. The approved
/// set can therefore shrink between the report and the act and can never grow,
/// over the bytes as well as over the list.
///
/// **The directory goes first and the forgets follow, and nothing is forgotten
/// on a partial removal.** That ordering is P2 (devlaunch#462): the recorded
/// path does not resolve at the moment the forget runs, even where it resolved
/// a moment earlier, so the forget can only ever drop the one name it was
/// handed. Interrupted between the two, git is left holding a registration with
/// nothing at its place — which is exactly the shape the next run's
/// [`SiteKind::OursGone`] handles.
pub(crate) fn reclaim(
    git: &Git<'_>,
    plan: &CloneWorktrees,
    bare: Option<&Path>,
    report: &mut WorktreeReport,
) {
    let Some(picture) = ClonePicture::of(git, &plan.clone) else {
        // git will not say what this clone holds any more, so nothing in it is
        // removable: the classification the plan rests on cannot be re-taken.
        report
            .withheld
            .extend(plan.going.iter().map(|going| WithheldWorktree {
                path: going_path(&plan.clone, going),
                because: Standing::one(Reason::CouldNotProve {
                    at: Place::TheCloneItself,
                    blank: Blank::GitWouldNotSay(CouldNotTell::GitCouldNotRead {
                        clone: plan.clone.clone(),
                        reason:
                            "git would not list this clone's worktrees a second time".to_owned(),
                    }),
                }),
            }));
        return;
    };
    // The same weighing as the plan, with the plan's own per-unit insistence: a
    // root the plan promoted is re-weighed as promoted, everything else as not.
    let weighed = weigh_clone(
        git,
        &plan.clone,
        bare,
        &picture,
        Derivatives::Weighed,
        |root| {
            plan.going
                .iter()
                .find(|going| approves(going, root, &plan.clone))
                .map(|going| going.promotion.insistence())
                .unwrap_or(Insistence::NotInsisted)
        },
    );
    let Weighing {
        going: fresh,
        standing: fresh_standing,
        derivatives: fresh_derivatives,
    } = weighed;
    for planned in &plan.going {
        let Some(confirmed) = fresh
            .iter()
            .find(|fresh| fresh.identity() == planned.identity())
        else {
            // The re-check would not hand this unit back: something under it
            // stands now, or it is not what it was. Say why, with the fresh
            // reasons where the fresh pass attributed some to this subtree.
            let path = going_path(&plan.clone, planned);
            let because = fresh_standing
                .iter()
                .find(|standing| standing.at.starts_with(&path) || path.starts_with(&standing.at))
                .map(|standing| standing.reasons.clone())
                .unwrap_or_else(|| {
                    Standing::one(Reason::CouldNotProve {
                        at: Place::TheCloneItself,
                        blank: Blank::NothingToAskThrough,
                    })
                });
            report.withheld.push(WithheldWorktree { path, because });
            continue;
        };
        if let Some(grew) = grew_past(planned, confirmed) {
            // The subtree gained something the plan did not name. Withhold the
            // whole unit rather than removing a smaller part of it: the unit is
            // the radius, and there is no smaller radius to fall back to.
            report.withheld.push(WithheldWorktree {
                path: going_path(&plan.clone, planned),
                because: Standing::one(Reason::CouldNotProve {
                    at: Place::ASite(grew),
                    blank: Blank::AppearedAfterThePlan,
                }),
            });
            continue;
        }
        act_on(git, &plan.clone, planned, confirmed, report);
    }
    reclaim_derivatives(&plan.clone, &plan.derivatives, &fresh_derivatives, report);
}

/// Reclaim the tagged derivatives the plan named, each one re-read first.
///
/// **Both records are read again, and they are read by the same pass that read
/// them for the plan.** `fresh` is [`weigh_clone`]'s answer taken under the
/// lock a moment ago, so the tag, the `conda-meta/pixi` record, the lockfile and
/// the claimant fold have all been put a second time, by one implementation. A
/// plan line and the act on it therefore cannot be answering different
/// questions — the defect this map has punished three times.
///
/// The approved set can shrink and can never grow: only a place the plan named
/// is looked at, and only where the re-read *also* says derivable is anything
/// removed.
///
/// What is removed is the tagged directory alone. Never `.pixi`, which carries
/// no tag and holds the one file `.pixi/.gitignore` un-ignores; never anything
/// above it.
fn reclaim_derivatives(
    clone: &Path,
    planned: &[Tagged],
    fresh: &[Tagged],
    report: &mut WorktreeReport,
) {
    for derivative in planned.iter().filter_map(Tagged::derivable) {
        let path = clone.join(derivative.at().as_str());
        let confirmed = fresh.iter().find(|it| it.at() == derivative.at());
        let Some(_) = confirmed.and_then(Tagged::derivable) else {
            report.withheld_derivatives.push(WithheldDerivative {
                path,
                because: match confirmed {
                    Some(tagged) => NotDerivableNow::Answered(Box::new(tagged.clone())),
                    None => NotDerivableNow::NoTagThere,
                },
            });
            continue;
        };
        match remove_tree_as_far_as_it_goes(&path) {
            TreeSweep::Everything => report.reclaimed.push(ReclaimedDerivative {
                path,
                // The plan's figure, so what somebody is told they got back is
                // what they said yes to — the same rule `act_on` follows.
                usage: derivative.usage().clone(),
            }),
            TreeSweep::WhatItCould(refused) | TreeSweep::Nothing(refused) => {
                report.refused.extend(refused.iter().cloned());
            }
        }
    }
}

/// The first registration `confirmed` names that `planned` did not, or nothing
/// when the confirmed unit's radius is inside the approved one.
///
/// Compared as recorded paths, which is what the forget is invoked with, so this
/// is the same value on both sides of the comparison rather than two derivations
/// of it that could disagree.
fn grew_past(planned: &Going, confirmed: &Going) -> Option<Inside> {
    let approved = planned.forgets();
    confirmed
        .forgets()
        .iter()
        .find(|fresh| !approved.iter().any(|named| named == *fresh))
        .and_then(|fresh| inside_a_worktrees_dir(fresh.as_path()))
}

/// Whether one planned unit is the approval for `root`.
fn approves(going: &Going, root: &Site, clone: &Path) -> bool {
    match &going.what {
        Collectable::Directory(directory) => directory.at == root.at(clone),
        Collectable::Registration(registration) => registration.place == root.inside,
    }
}

fn going_path(clone: &Path, going: &Going) -> PathBuf {
    match &going.what {
        Collectable::Directory(directory) => directory.at.clone(),
        Collectable::Registration(registration) => clone.join(&registration.place.place),
    }
}

/// One unit, carried out: bytes first, then every name that rode in on it.
///
/// Takes both passes' views of the unit deliberately. What is *done* is the
/// confirmed one, because this pass is the one holding the lock; what is
/// *reported* is the plan's figure, because that is the number somebody said yes
/// to. Reporting the re-measurement made the two halves of one report mean
/// different things — the clone arm beside this one has always reported the
/// plan's.
fn act_on(
    git: &Git<'_>,
    clone: &Path,
    planned: &Going,
    confirmed: &Going,
    report: &mut WorktreeReport,
) {
    let insistence = confirmed.promotion.insistence();
    match &confirmed.what {
        Collectable::Directory(directory) => {
            match remove_tree_as_far_as_it_goes(&directory.at) {
                TreeSweep::Everything => {
                    report.removed.push(RemovedWorktree {
                        path: directory.at.clone(),
                        usage: match planned.what() {
                            Collectable::Directory(approved) => approved.usage().clone(),
                            // Unreachable: the subset check above passed, so the
                            // two units are the same shape. Falling back to what
                            // was measured is honest either way.
                            Collectable::Registration(_) => directory.usage.clone(),
                        },
                    });
                    forget(git, clone, &directory.forgets, insistence, report);
                }
                TreeSweep::WhatItCould(refused) | TreeSweep::Nothing(refused) => {
                    // Nothing is forgotten: a registration outliving its bytes
                    // is the state the next run reads correctly, where bytes
                    // outliving their registration is the state that used to
                    // read as "git has already forgotten it" and delete.
                    report.refused.extend(refused.iter().cloned());
                }
            }
        }
        Collectable::Registration(registration) => {
            forget(git, clone, &registration.forgets, insistence, report);
        }
    }
}

/// Drop each registration by the name a listing printed.
///
/// A refusal is contained: it is reported, it forgets nothing else, and the
/// registration it refused is still in the next listing. Measured, the refusal
/// that matters — a recorded path that resolves to something unrelated — is one
/// git will not be argued out of.
fn forget(
    git: &Git<'_>,
    clone: &Path,
    forgets: &[Recorded],
    insistence: Insistence,
    report: &mut WorktreeReport,
) {
    let force = match insistence {
        Insistence::Insisted => ForgetForce::PastALock,
        Insistence::NotInsisted => ForgetForce::AsAsked,
    };
    for recorded in forgets {
        match git.worktree_remove(clone, recorded.as_path(), force).said() {
            Some(_) => report.forgotten += 1,
            None => report.forget_refused.push(ForgetRefused {
                registered: recorded.as_path().to_path_buf(),
                reason: "git would not drop this registration".to_owned(),
            }),
        }
    }
}

// ===========================================================================
// the clone as the root of the same forest
// ===========================================================================

/// What one clone holds, everything nested in it included.
///
/// The clone's own probes conjoined with the verdict of every site inside it —
/// one predicate with one set of readers (`dl <ws> rm`'s guard, `--prune`'s
/// orphan arm, `--ls --json`) rather than a clone predicate and a worktree
/// predicate that can disagree. The clone-level answer alone is structurally
/// blind to nested agent worktrees: `.claude/worktrees/` is ordinarily
/// gitignored, so `git status` at the clone root says nothing about an
/// afternoon of unsaved work one level in. Dirt is per working tree;
/// reachability is per repository (devlaunch#446).
pub(crate) fn clone_verdict(git: &Git<'_>, clone: &Path, bare: BareCache<'_>) -> Verdict {
    let own = workspace_state::holds_unsaved_work(git, clone, bare);
    let own_reasons = lift(own);
    let site_reasons = site_reasons(git, clone);
    match Standing::of(own_reasons.into_iter().chain(site_reasons).collect()) {
        Some(standing) => Verdict::Stands(standing),
        None => Verdict::Collectable(Proof {
            how: ProofHow::CloneProbesAnsweredClear,
        }),
    }
}

/// The clone's branch and its verdict, read together for the listing row.
pub(crate) struct CloneAccount {
    pub(crate) branch: Option<String>,
    pub(crate) holds: Verdict,
}

pub(crate) fn account_of(git: &Git<'_>, clone: &Path, bare: BareCache<'_>) -> CloneAccount {
    let state = workspace_state::read_clone(git, clone, bare);
    let own_reasons = lift(state.unsaved);
    let site_reasons = site_reasons(git, clone);
    let holds = match Standing::of(own_reasons.into_iter().chain(site_reasons).collect()) {
        Some(standing) => Verdict::Stands(standing),
        None => Verdict::Collectable(Proof {
            how: ProofHow::CloneProbesAnsweredClear,
        }),
    };
    CloneAccount {
        branch: state.branch,
        holds,
    }
}

/// The one verdict for a workspace devlaunch has no clone of its own for:
/// nothing of ours to protect.
pub(crate) fn nothing_of_ours() -> Verdict {
    Verdict::Collectable(Proof {
        how: ProofHow::NothingOfOurs,
    })
}

/// A verdict for a clone whose directory could not even be named or read —
/// the guard's refusing arm, carried in the same type as every other answer.
pub(crate) fn could_not_prove(cause: CouldNotTell) -> Verdict {
    Verdict::Stands(Standing::one(Reason::CouldNotProve {
        at: Place::TheCloneItself,
        blank: Blank::GitWouldNotSay(cause),
    }))
}

/// The clone's own [`Unsaved`] answer, lifted into reasons. The lossy direction
/// (a re-encoding of the verdict *into* `Unsaved`) exists nowhere: `Unsaved`
/// survives at the JSON edge as key names, not as a value the verdict is
/// squeezed through.
fn lift(own: Unsaved) -> Vec<Reason> {
    match own {
        Unsaved::NothingToLose => Vec::new(),
        Unsaved::WouldLose(losses) => vec![Reason::Holds {
            at: Place::TheCloneItself,
            losses: Box::new(losses),
        }],
        Unsaved::CouldNotTell(cause) => vec![Reason::CouldNotProve {
            at: Place::TheCloneItself,
            blank: Blank::GitWouldNotSay(cause),
        }],
    }
}

/// The standing reasons of every site in `clone`, or nothing where there are no
/// sites — which is nearly every clone, at the cost of one failed `read_dir`.
///
/// **That early return is the whole of what keeps `dl --ls` affordable**, and it
/// is worth naming because this function is on the listing path, which is a
/// read-only command people run casually. A clone that has never had an agent
/// worktree in it costs one `read_dir` that fails and no git at all: no
/// `worktree list`, no probe, no walk. A clone that *has* them costs one
/// `worktree list` plus, per site, one `status --porcelain` and at most one
/// `rev-list` — the reachability answer is memoised per revision for the whole
/// of one clone's weighing, and sites cut from one commit are the common shape.
///
/// What is not done here, said plainly rather than left to be discovered: the
/// rows are still weighed one after another, and a host carrying dozens of agent
/// worktrees pays that serially on every `--ls --json`. `docs/performance.md`
/// covers the launch path and not this one, so nothing would catch a regression
/// in it. Measuring the listing and giving it a floor is its own piece of work.
///
/// The sibling bare is derived from the clone's place in the cache
/// (`<repos>/<owner>/<repo>/.bare` beside `<repos>/<owner>/<repo>/<leaf>`),
/// which is the same sibling `--prune`'s sweep is handed by the repo manager.
fn site_reasons(git: &Git<'_>, clone: &Path) -> Vec<Reason> {
    if std::fs::read_dir(worktrees_dir(clone)).is_err() {
        return Vec::new();
    }
    let Some(picture) = ClonePicture::of(git, clone) else {
        return vec![Reason::CouldNotProve {
            at: Place::TheCloneItself,
            blank: Blank::GitWouldNotSay(CouldNotTell::GitCouldNotRead {
                clone: clone.to_path_buf(),
                reason: "git would not list this clone's worktrees".to_owned(),
            }),
        }];
    };
    let bare = clone.parent().map(|parent| parent.join(".bare"));
    let bare = bare.as_deref().filter(|path| path.is_dir());
    // `Derivatives::NotAsked`: this is `dl --ls`, and costing a derivative is a
    // full walk of a site plus an `exclusive_usage` over a 12000-file
    // environment. The field it leaves empty is discarded here rather than read
    // as an answer.
    let weighed = weigh_clone(git, clone, bare, &picture, Derivatives::NotAsked, |_| {
        Insistence::NotInsisted
    });
    weighed
        .standing
        .into_iter()
        .flat_map(|site| site.reasons.iter().cloned().collect::<Vec<_>>())
        .collect()
}

// ===========================================================================
// attribution for `dl --ls --size`
// ===========================================================================

/// How much of a clone's bytes are agent git worktrees, or nothing when it has
/// none.
///
/// **Attribution, not an addition.** These bytes are inside the clone, so they
/// are already in what `dl --ls --size` says the clone would free; this says how
/// much of that figure is worktrees. It reached 82% of a whole cache on the
/// reference host while being invisible in `--ls --size`, which is how it got to
/// a full disk (devlaunch#426).
///
/// One walk of the whole `.claude/worktrees/` tree rather than one per worktree,
/// which also means nesting is counted once and counted right. The object store
/// is not in it: a linked worktree shares the clone's, and the clone's objects
/// are hardlinked out of the `.bare` next door, so billing them here would count
/// them two or three times — which [`disk_usage::exclusive_usage`] already
/// refuses to do, because a file's bytes are a tree's only when every link to it
/// is inside that tree.
///
/// `None` rather than a zero, because "this clone has never had an agent
/// worktree in it" and "it has some and they cost nothing" are different facts,
/// and the first is what nearly every clone is.
pub(crate) fn bytes_in(clone: &Path) -> Option<DiskUsage> {
    let root = worktrees_dir(clone);
    root.is_dir().then(|| disk_usage::exclusive_usage(&root))
}

/// The `.claude/worktrees/` inside one directory.
fn worktrees_dir(directory: &Path) -> PathBuf {
    directory.join(WORKTREES_DIR[0]).join(WORKTREES_DIR[1])
}

#[cfg(test)]
mod tests;
