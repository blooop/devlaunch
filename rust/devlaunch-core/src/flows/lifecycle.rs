//! Stopping, deleting and cleaning up: everything that removes something.
//!
//! Ported from the lifecycle half of `dl.py` — `workspace_stop`,
//! `workspace_delete` and its unsaved-work guard, `get_workspace_state`,
//! `resolve_known_workspace`, `prune_command`, `purge_all_data`,
//! `reconcile_command`, `update_cache_background` and `sweep_repo_fetches`. See
//! docs/rust-rewrite-plan.md (M6); this is also `wf`'s consumption surface
//! (#250), which is why everything here is public rather than module-private and
//! why the names are the ones a caller outside dl would reach for.
//!
//! Everything reachable here is **binary surface — not part of the frozen wf API
//! (#251 §7)** unless [`api`](crate::api) re-exports it: the `dl` binary is a
//! separate crate and every sentence a user reads is written there, so a rendering
//! layer that could not name these typed results would not be a rendering layer.
//! Five names from this module are promised rather than merely reachable — the
//! three §7 verbs (`list`, `remove`, `up`), and, since #340, [`Refresh`] and
//! [`SelfInvocation`], which `api::Launch::new` cannot be called without. The
//! authority is `api`'s re-export list and the snapshot over it, not this comment.
//!
//! # Three commands remove things, and none of them decides what is finished
//!
//! - **`dl <ws> rm`** deletes one workspace, its clone and the named docker
//!   volumes its devcontainer created ([`VolumeSweep`]), and refuses when the
//!   clone holds work that exists nowhere else. That refusal is the one judgement
//!   dl makes on its own account (see [`crate::domain::workspace_state`]).
//! - **`dl --prune`** removes the clone *directories* no live workspace opens, and
//!   reclaims the docker volumes of workspaces devpod no longer lists, from
//!   devlaunch's own copies of what devpod substituted
//!   ([`crate::flows::kept_copies`]). It never touches a devpod workspace, a
//!   container or an image, and it leaves every bare cache alone. The volume half
//!   arrived with devlaunch#456 and its reason is the inverse of the old one: prune
//!   still deletes no workspace, and *because* it deletes none, the volumes it
//!   reclaims belong to workspaces something else already deleted — where devpod's
//!   own record is gone and devlaunch's copy is the only thing that names them.
//! - **`dl --purge`** deletes the workspaces devlaunch created — volumes and all —
//!   and its whole cache directory. Ownership-scoped
//!   ([`crate::flows::listing::workspace_ownership`]), and it names what it leaves
//!   standing. It does *not* share `rm`'s delete: it issues its own captured
//!   `devpod delete --force` per workspace, which is why the volume sweep is wired
//!   into it explicitly rather than inherited.
//!
//! `dl --reconcile` is the fourth of the family and removes nothing at all: it
//! re-points devpod records the id-scheme change orphaned (devlaunch#88), and an
//! orphan it cannot adopt is reported and left where it is.
//!
//! # The plan is a value, and the question is the binary's
//!
//! Every one of these commands prints what it would do, asks, and then acts. The
//! report a user answers and the set that actually dies must come from the *same
//! object*, because the difference between them is somebody's directory — so the
//! classification is a value ([`PrunePlan`], [`PurgePlan`], [`ReconcilePlan`])
//! that core hands over, the `y/N` question belongs to the binary, and the acting
//! pass takes the plan back. `-y` never reaches core: it is the binary's answer to
//! its own question.
//!
//! Nothing here prints and nothing here is a sentence. Every refusal, notice and
//! outcome is a typed value carrying exactly what the line it replaces
//! interpolated; the words and the exit codes are the `dl` binary's (#251).
//!
//! # Every mutation forgets the snapshot
//!
//! A flow that changes what `devpod list` would say takes the
//! [`CommandContext`](crate::flows::listing::CommandContext) **mutably** and
//! calls
//! [`CommandContext::forget_workspaces`](crate::flows::listing::CommandContext::forget_workspaces)
//! — Python's `invalidate_workspace_list_cache()`. Taking it mutably is the
//! point: a flow that mutates devpod cannot be handed a shared reference, so it
//! cannot quietly leave a stale snapshot behind.
//!
//! # This module is a table of contents
//!
//! Every item below is declared in one of the files beside this one and
//! re-exported here, so `flows::lifecycle::Thing` names what it always named and
//! no caller has a path to change. The members are private and the re-exports
//! are what is public: there is one path to each of these types and not two,
//! which is also why splitting the file moved no row of the API snapshots.
//!
//! The cut follows the thirteen banner comments the single file had grown, since
//! those were already the module boundaries — they had just never been made into
//! any. `tests/lifecycle_is_split.rs` is what keeps them made: it fails if this
//! root grows a body of its own, which is the only road back to one file.

mod delete;
mod delete_guard;
mod fetch_sweep;
mod locations;
mod notices;
mod prune_plan;
mod prune_run;
mod prune_status;
mod purge;
mod reconcile;
mod refresh;
mod state;
mod stop;

pub use delete::*;
pub use delete_guard::*;
pub use fetch_sweep::*;
pub use locations::*;
pub use notices::*;
pub use prune_plan::*;
pub use prune_run::*;
pub use prune_status::*;
pub use purge::*;
pub use reconcile::*;
pub use refresh::*;
pub use state::*;
pub use stop::*;

#[cfg(test)]
mod tests;
