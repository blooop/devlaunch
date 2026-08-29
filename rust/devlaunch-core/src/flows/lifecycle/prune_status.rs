//! `dl --prune`: what one clone directory is.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::delete_guard::Insistence;
use super::locations::WorkspaceLocations;
use crate::clients::git::Git;
use crate::domain::model::WorktreeInfo;
use crate::domain::workspace_state::BareCache;
use crate::flows::agent_worktrees::{self, Standing, Verdict};
use crate::flows::disk_usage::{self, DiskUsage};

/// Which arm one clone directory is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloneStatus {
    /// A live devpod workspace opens this exact clone directory.
    Referenced { workspace_id: String },
    /// Nothing opens this directory and no record ties it to a live workspace.
    ///
    /// `verdict` sits inside this arm rather than beside the classification
    /// because it is only ever actionable here: "unsaved work on a clone that is
    /// staying anyway" is a sentence this type cannot say. It is
    /// [`agent_worktrees::clone_verdict`] — the clone's own probes conjoined
    /// with every agent worktree nested in it, so an orphan clone whose
    /// gitignored `.claude/worktrees/` holds an afternoon of unsaved work can no
    /// longer read as free to delete (devlaunch#446). `usage` is here for the
    /// same reason and earns its place twice over — the walk behind it is
    /// O(files) with no ceiling, and this is the only arm whose bytes anybody is
    /// going to get back, so putting it here is what keeps the other two arms
    /// from being walked at all.
    Orphaned { verdict: Verdict, usage: DiskUsage },
    /// devpod lists a workspace whose records and devlaunch's disagree about where
    /// it is.
    ///
    /// devlaunch#88's shape, and the reason `--prune` does not wait on #88: under
    /// that state a healthy clone at the *new* path is sourced by nobody. Read as
    /// an orphan it would be deleted; read as referenced it would silently hide
    /// disk. It is neither — it is two records disagreeing, and the answer to a
    /// disagreement is to keep the directory and say so.
    Disputed {
        workspace_id: String,
        sourced_at: String,
    },
}

/// The repository one clone directory belongs to.
///
/// Three fields that are one fact and are always read together: the names the
/// scan reports the clone under, and the mirror beside it. Grouped because they
/// arrive together at both call sites, from the same walk over
/// `repos/<owner>/<repo>`, and because passing the mirror of a *different*
/// repository would be a guard reading the wrong tags.
#[derive(Clone, Copy)]
pub(crate) struct RepoAt<'a> {
    pub(crate) owner: &'a str,
    pub(crate) repo: &'a str,
    /// dl's mirror for this repository, whether or not it is on disk. A path that
    /// is not there refuses when asked, and the guard reads that as "no mirror",
    /// which counts every tag in the clone as local (#487).
    pub(crate) bare: &'a Path,
}

/// Which arm `clone` is, asked in the order that fails towards keeping it.
///
/// devpod's own listing is consulted first, and by containment rather than by the
/// lexical predicate the reporting surface uses: the question is whether any live
/// workspace's source is at or under *this* directory.
///
/// Then the two ways devpod's records and devlaunch's can disagree, both
/// devlaunch#88's shape: a live workspace somewhere in *this repository's* clone
/// tree that is not a clone (36 of 39 workspaces on #88's host), and this
/// directory's own record naming a workspace devpod still lists and sources
/// elsewhere. A record naming a workspace devpod has *forgotten* is not a
/// disagreement — it is the ordinary stale clone this command exists for.
///
/// The unsaved probe and the disk walk run last and only on the arm that could be
/// removed. Together they are the expensive half of a scan (593 ms of git over 37
/// clones on the reference host, measured before the tag comparison #487 added,
/// plus a walk with no ceiling), and asking them about a directory no answer could
/// affect is time spent to learn nothing.
///
/// The bare is named rather than looked up, and it is this repository's own: every
/// clone this walk reaches is a subdirectory of `repos/<owner>/<repo>`, so the
/// mirror beside them is the one they were cloned from. `--prune` deletes clones
/// without being asked twice, which is exactly the surface #487 was about.
pub(crate) fn clone_status(
    git: &Git<'_>,
    clone: &Path,
    of: RepoAt<'_>,
    locations: &WorkspaceLocations,
    record_for: &HashMap<PathBuf, WorktreeInfo>,
    listed_at: &HashMap<String, String>,
) -> CloneStatus {
    let RepoAt { owner, repo, bare } = of;
    if let Some(workspace_id) = locations.holder(clone) {
        return CloneStatus::Referenced {
            workspace_id: workspace_id.to_owned(),
        };
    }
    if let Some(misplaced) = locations.misplaced_in(owner, repo) {
        return CloneStatus::Disputed {
            workspace_id: misplaced.workspace_id.clone(),
            sourced_at: misplaced.sourced_at.clone(),
        };
    }
    if let Some(record) = record_for.get(clone)
        && let Some(elsewhere) = listed_at.get(&record.workspace_id)
    {
        return CloneStatus::Disputed {
            workspace_id: record.workspace_id.clone(),
            sourced_at: elsewhere.clone(),
        };
    }
    CloneStatus::Orphaned {
        verdict: agent_worktrees::clone_verdict(git, clone, BareCache::At(bare)),
        usage: disk_usage::exclusive_usage(clone),
    }
}

/// Why one clone directory is staying.
///
/// A sum rather than Python's `because: str`, so the sentence is the binary's and
/// each arm carries what that sentence interpolates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeptBecause {
    /// A live workspace still opens it.
    StillOpened { workspace_id: String },
    /// At least one thing stands it — work it holds, or a question that could
    /// not be put — and `--force` was not typed.
    Objected(Standing),
    /// devpod lists the workspace this directory's record names, and sources it
    /// somewhere else. See devlaunch#88.
    RecordsDisagree {
        workspace_id: String,
        sourced_at: String,
    },
}

/// Nothing objected to removing this directory, or `--force` carried it past an
/// objection.
///
/// Carried on the decision rather than read from a plan-wide `--force` flag, and
/// that difference is a deletion. A plan-wide boolean says "the user insisted"
/// about every directory in the plan, including the ones nothing objected to — so
/// the later re-probe, which exists to catch work written while the user was
/// reading the report, was skipped for clones `--force` had not promoted at all. A
/// promotion belongs to the directory it promoted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Promotion {
    Unopposed,
    Insisted { despite: Standing },
}

impl Promotion {
    /// The insistence the second pass re-applies to *this* directory alone.
    pub(super) fn insistence(&self) -> Insistence {
        match self {
            Self::Unopposed => Insistence::NotInsisted,
            Self::Insisted { .. } => Insistence::Insisted,
        }
    }
}

/// What `--prune` does about one clone directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    /// This directory goes, this is what it gives back, and this is what was
    /// insisted past to get here.
    ///
    /// The bytes travel with the decision rather than beside it, so "what this run
    /// reclaims" is a total over the things it is actually removing and cannot be
    /// assembled from a different set than the one that dies.
    Remove {
        usage: DiskUsage,
        promotion: Promotion,
    },
    Keep(KeptBecause),
}

/// What `--prune` does about one clone. The only such place.
///
/// Total over [`CloneStatus`]'s arms, and deliberately the single point at which
/// anything becomes deletable: there is no boolean beside a status that a later
/// caller could read without having answered which arm it is, and no path from a
/// directory devlaunch could not classify to one it would remove.
///
/// `--force` promotes exactly one arm. It is not a general override:
/// [`CloneStatus::Referenced`] and [`CloneStatus::Disputed`] are not "refusals to
/// be insisted past", they are devlaunch saying the directory is still in use or
/// that its own records disagree, and there is nothing for a user to mean by
/// insisting.
pub(crate) fn decide(status: CloneStatus, insistence: Insistence) -> Decision {
    match status {
        CloneStatus::Referenced { workspace_id } => {
            Decision::Keep(KeptBecause::StillOpened { workspace_id })
        }
        CloneStatus::Orphaned { verdict, usage } => match verdict {
            Verdict::Collectable(_) => Decision::Remove {
                usage,
                promotion: Promotion::Unopposed,
            },
            Verdict::Stands(standing) => match insistence {
                Insistence::Insisted => Decision::Remove {
                    usage,
                    promotion: Promotion::Insisted { despite: standing },
                },
                Insistence::NotInsisted => Decision::Keep(KeptBecause::Objected(standing)),
            },
        },
        CloneStatus::Disputed {
            workspace_id,
            sourced_at,
        } => Decision::Keep(KeptBecause::RecordsDisagree {
            workspace_id,
            sourced_at,
        }),
    }
}
