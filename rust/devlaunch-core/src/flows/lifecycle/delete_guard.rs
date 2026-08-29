//! The delete guard: the one judgement dl makes on its own account.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use super::delete::Persistence;
use super::notices::{LifecycleNotice, extend_with_cache};
use crate::clients::git::Git;
use crate::domain::metadata::MetadataStorage;
use crate::domain::model::WorktreeInfo;
use crate::flows::agent_worktrees::{Standing, Verdict};
use crate::flows::listing::{self, ClonePathResolver};
use crate::flows::repo_manager::CacheNotice;
use crate::flows::workspace_clone::WorkspaceCloneManager;
use crate::notices::Notices;

/// Which removal this is, of the three `dl` performs.
///
/// One value rather than the four flags it stands for — the unsaved-work guard,
/// devpod's `--ignore-not-found`, devpod's `--force` and a deadline — because those
/// are not independent settings anybody would want to mix. They are one decision
/// about how badly the caller wants the workspace gone, and spelling them
/// separately makes seven combinations writable of which three are meant. The three
/// that are meant are these, and each one names a command line rather than a
/// setting.
///
/// It lived in the `dl` binary until the removal fold, which is what made the
/// guard skippable: core took the flags one at a time, so the sequence that turns
/// them into a removal was the caller's to get right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    /// `dl <ws> rm`, the happy path. Stops at work that exists nowhere else, names
    /// it, and offers `--force`. devpod is asked with its own defaults and given as
    /// long as it needs, because a container that is slow to come down is a
    /// container that is coming down.
    Guarded,
    /// `dl <ws> rm --force`. The guard does not even look, and an absent workspace
    /// counts as deleted, which is what makes it `rm -f` rather than a louder `rm`.
    /// devpod is still asked politely: this is a workspace you are sure about, not
    /// one that is stuck.
    Insisted,
    /// `dl <ws> kill`. The verb for a workspace that is wedged and finished with,
    /// so nothing here refuses and nothing here waits indefinitely: the guard looks
    /// and *reports* rather than stopping, devpod gets `--force` so a workspace it
    /// can no longer reach still goes, and the call carries a deadline so it cannot
    /// join the five second lock loop the sweep in front of it was reached for.
    ///
    /// The guard still looks, and that is the difference between this and
    /// [`Removal::Insisted`] rather than a leftover: work that exists nowhere else
    /// is about to be destroyed, and the person who typed `kill` is owed the list
    /// even though they are not being asked to confirm it.
    Wedged,
}

impl Removal {
    /// Whether dl will accept an absent workspace as a delete, and what devpod's
    /// `--ignore-not-found` rides on.
    ///
    /// Public because it is also what a *rendering* of the answer turns on: "Removed
    /// workspace X" and "Workspace X is gone" are the two things a zero exit
    /// established, and only this tells them apart.
    pub fn insistence(self) -> Insistence {
        match self {
            Self::Guarded => Insistence::NotInsisted,
            Self::Insisted | Self::Wedged => Insistence::Insisted,
        }
    }

    /// How hard devpod is pushed, and whether the call carries a deadline.
    pub(super) fn persistence(self) -> Persistence {
        match self {
            Self::Guarded | Self::Insisted => Persistence::Ordinary,
            Self::Wedged => Persistence::Wedged,
        }
    }

    /// Whether the unsaved-work probe is worth running, and what its answer does.
    ///
    /// [`Removal::Insisted`] is the one that skips it, and it skips it to save the
    /// work rather than to hide the answer: the probe is a `git status` and a
    /// `git log` per clone, and `rm --force` has said in advance that it will not
    /// act on either. Probing unconditionally is the one-line accident this fold
    /// invites, so the skip is a total function of the removal rather than a
    /// condition anybody writes twice.
    pub(super) fn probe(self) -> Probe {
        match self {
            Self::Guarded => Probe::Look(Finding::Refuses),
            Self::Insisted => Probe::Skip,
            Self::Wedged => Probe::Look(Finding::Says),
        }
    }
}

/// Whether the removal looks for work that exists nowhere else.
///
/// Nested rather than three flat arms so that [`Finding`] is unreachable from the
/// arm that never looks: a removal that skips the probe has no finding to act on,
/// and a flat third arm left every `match` on the answer with a case its author had
/// to invent a body for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Probe {
    /// Look, and then do this with what is found.
    Look(Finding),
    /// Do not look. `rm --force`'s.
    Skip,
}

/// What a removal does with work it found that exists nowhere else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Finding {
    /// Stop, and hand the refusal back for the caller to name. `rm`'s.
    Refuses,
    /// Say it, and remove it. `kill`'s.
    Says,
}

/// Whether the caller typed `--force`.
///
/// Named arms rather than a bool, because `--force` means two different things on
/// this path and both are decisions: it carries a delete past the unsaved-work
/// guard, *and* it makes an already-absent workspace count as deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Insistence {
    Insisted,
    NotInsisted,
}

/// What one `dl --prune` was told to go ahead despite, and which flag said it.
///
/// One value with named fields rather than two [`Insistence`] parameters side by
/// side, because they answer different hazards and a caller could not be stopped
/// from swapping them. The swap in the dangerous direction is `--force` reaching
/// the worktree sweep, which would quietly widen a flag people already type from
/// "past a clone holding work nowhere else" to "past a locked worktree somebody
/// may be working in".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Insisted {
    /// `--force`.
    pub clones: Insistence,
    /// `--force-worktrees`.
    pub worktrees: Insistence,
}

impl Insisted {
    /// Nothing insisted on: what a plain `dl --prune` means.
    ///
    /// The command builds its pair from the flags it was given, so this spelling
    /// of it is the tests' convenience and nothing else's.
    #[cfg(test)]
    pub(crate) fn nothing() -> Self {
        Self {
            clones: Insistence::NotInsisted,
            worktrees: Insistence::NotInsisted,
        }
    }
}

/// Why `dl <ws> rm` will not delete this workspace.
///
/// **The standing is rendered into words here rather than carried, and that is
/// the seam doing its job.** This type is re-exported at [`crate::api`], so
/// every type it names is part of the promise. Carrying
/// [`agent_worktrees::Standing`](crate::flows::agent_worktrees::Standing) would
/// name a type the promise does not include — the defect the comment beside that
/// re-export already records —
/// and promising it honestly would drag `StandingSite`, `Reason`, `Place`,
/// `Blank`, `Subject` and `NonEmpty<Loss>` along with it, which is most of a
/// module's internal vocabulary arriving in the one tier whose value is being
/// small and stable. So the standing stays exactly as it is inside `flows`,
/// and what crosses the promise is [`RemovalGrounds`]: the same words, made of
/// `String`. It is the move
/// [`agent_worktrees::Verdict::unsaved_json`](crate::flows::agent_worktrees::Verdict::unsaved_json)
/// makes for the wire, at the same boundary and for the same reason
/// (devlaunch#531).
///
/// Nothing about the modelling changed. #446's "a refusal carries the whole
/// standing" is still true of the domain type, and still true here:
/// [`RemovalGrounds`]
/// has an arm for holding work *and* having a question that could not be put,
/// so a refusal still never has to pick one of two true things to say.
/// "Could not be proved" is refused for not knowing: the work is still on disk
/// and nothing has shown it exists anywhere else, which is the same standing as
/// unpushed work and gets the same refusal and the same way past it
/// (devlaunch#171).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovalRefused {
    pub workspace_id: String,
    pub because: RemovalGrounds,
}

/// What a refusal has to say, in the words it will be said in.
///
/// **Three arms and not two `Option<String>`s.** A standing is non-empty by
/// construction and every reason in it is either a proved loss or an unproved,
/// so "neither" cannot happen — and a pair of options is a type in which it can.
/// Both render sites used to match the pair and carry a fourth arm apologising
/// for being unreachable; this is that arm deleted rather than commented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemovalGrounds {
    /// Work that exists nowhere else, and every question was answered.
    WouldLose(String),
    /// No proved loss, but a question that could not be put — which is refused
    /// for not knowing rather than waved through.
    CouldNotTell(String),
    /// Both, which is the case that makes this a sum and not a choice.
    BothAtOnce {
        would_lose: String,
        could_not_tell: String,
    },
}

/// Render one standing into the words that cross the promise.
///
/// Deliberately not a method on [`RemovalGrounds`] and not `pub`: a public
/// constructor taking a [`Standing`] would put that type back in the promised
/// tier's signature list, which is the whole thing this seam exists to avoid.
/// The conversion belongs to the boundary, so it lives at the boundary.
pub(super) fn refusal_from(standing: &Standing) -> RemovalGrounds {
    match (standing.would_lose(), standing.could_not_tell()) {
        (Some(would_lose), Some(could_not_tell)) => RemovalGrounds::BothAtOnce {
            would_lose,
            could_not_tell,
        },
        (Some(would_lose), None) => RemovalGrounds::WouldLose(would_lose),
        (None, Some(could_not_tell)) => RemovalGrounds::CouldNotTell(could_not_tell),
        // Unreachable: a standing is non-empty and each reason answers one of
        // the two. Rendering the whole thing is the honest fallback -- it says
        // what the reasons say, rather than inventing a sentence or panicking
        // on a path a caller cannot trigger.
        (None, None) => RemovalGrounds::CouldNotTell(standing.describe()),
    }
}

/// What the guard decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Guarded {
    /// Nothing dl can establish would be lost, or the caller insisted.
    MayRemove,
    Refused(RemovalRefused),
}

/// Whether `dl <ws> rm` may go ahead.
///
/// The one thing dl refuses on its own account. It is not a judgement about
/// whether the work is finished — dl has no way to know that — but about whether
/// this clone is the only place the work exists.
///
/// Total over [`Verdict`]'s two arms, and only one of them is permission — and
/// that one carries a proof no caller can mint (devlaunch#446), where
/// devlaunch#171's version of this guard could still be handed a "nothing to
/// lose" that nothing had established.
///
/// `--force` is checked *after* the answer is read, not instead of reading it, so
/// the refusal a forced delete carried past is still available to the caller — and
/// so a future `--force` that wanted to report what it overrode has it.
pub(crate) fn guard_removal(
    workspace_id: &str,
    verdict: Verdict,
    insistence: Insistence,
) -> Guarded {
    let refusal = match verdict {
        Verdict::Collectable(_) => return Guarded::MayRemove,
        Verdict::Stands(standing) => RemovalRefused {
            workspace_id: workspace_id.to_owned(),
            because: refusal_from(&standing),
        },
    };
    match insistence {
        Insistence::Insisted => Guarded::MayRemove,
        Insistence::NotInsisted => Guarded::Refused(refusal),
    }
}

/// [`ClonePathResolver`] over the production clone manager.
///
/// The listing's resolver trait cannot carry the [`CacheNotice`]s
/// [`WorkspaceCloneManager::resolve_clone_path`] produces — it is read from a
/// row-building loop that has nowhere to put them — so they are collected here and
/// drained by whoever built this. That keeps one answer to "which directory is
/// this record's clone in" across the listing, this guard and the delete itself,
/// which is the whole of devlaunch#174: they used to name it separately and could
/// disagree, with the guard clearing an absent directory while the delete removed
/// the one holding the work.
pub struct CloneDirectories<'a, 'r> {
    clones: &'a WorkspaceCloneManager<'r>,
    notices: RefCell<Vec<CacheNotice>>,
}

impl<'a, 'r> CloneDirectories<'a, 'r> {
    pub fn of(clones: &'a WorkspaceCloneManager<'r>) -> Self {
        Self {
            clones,
            notices: RefCell::new(Vec::new()),
        }
    }

    /// The notices resolving produced, leaving none behind.
    pub fn take_notices(&self) -> Vec<CacheNotice> {
        std::mem::take(&mut self.notices.borrow_mut())
    }
}

impl ClonePathResolver for CloneDirectories<'_, '_> {
    fn clone_path(&self, record: &WorktreeInfo) -> Option<PathBuf> {
        self.clones
            .resolve_clone_path(record, &mut *self.notices.borrow_mut())
    }

    fn bare_path(&self, record: &WorktreeInfo) -> Option<PathBuf> {
        self.clones.resolve_bare_path(record)
    }
}

/// What deleting `workspace_id` would destroy, as far as dl can establish —
/// the clone's own probes and every agent worktree nested in it, one verdict.
///
/// [`listing::unsaved_work_in`]'s reader, wired to the production resolver. The
/// answer for a workspace dl has no record of is collectable, which is the
/// honest answer rather than a permissive one: those are workspaces opened
/// from a path or a URL that dl never cloned and does not manage, so it has no
/// clone of its own to protect and no business inspecting somebody's checkout to
/// find one.
pub(crate) fn unsaved_work_in(
    clones: &WorkspaceCloneManager<'_>,
    storage: &MetadataStorage,
    git: &Git<'_>,
    cache_dir: &Path,
    workspace_id: &str,
    notices: &mut dyn Notices<LifecycleNotice>,
) -> Verdict {
    let directories = CloneDirectories::of(clones);
    let view = listing::DlView {
        cache_dir,
        storage,
        clones: &directories,
    };
    let unsaved = listing::unsaved_work_in(git, &view, workspace_id);
    extend_with_cache(notices, directories.take_notices());
    unsaved
}
