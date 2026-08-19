//! Which devpod workspace a lifecycle verb's target word names.
//!
//! `dl <target> stop` and `dl <target> rm` accept everything `dl <target>` does —
//! a workspace id, `owner/repo[@branch]`, a path, a git URL — and every one of
//! them has to become the *one* id devpod is addressed by. Python answers that
//! question in one block above its subcommand dispatch, shared by the launch and
//! by the lifecycle verbs.
//!
//! # The resolution is core's; the two subtractions are this module
//!
//! M6 stood this module up as its own copy of that block, because the launch flow
//! had not been wired yet. It is no longer a copy: the classification is
//! [`launch::plan`] and the devpod-first lookup is [`launch::resolve_triple`], the
//! same two calls `dl <spec>` makes. What is left here is the pair of steps a
//! *lifecycle* verb must not take, and both are **divergence row 23**:
//!
//! - `ensure_repo`, which clones the bare cache so a bare `owner/repo` can be
//!   asked for its default branch. Here the branch comes from
//!   [`RepositoryManager::get_default_branch`], which reads the record and then
//!   asks the remote for `HEAD` without a clone — where [`launch::plan`]'s caller
//!   would go through [`launch::name_default_branch`] and clone.
//! - `prepare_cold`, which clones the *workspace* whenever devpod does not
//!   recognise the derived id — so `dl owner/never-launched@main stop` created a
//!   clone directory and a metadata record on its way to stopping a workspace that
//!   does not exist. Nothing here creates anything: a triple devpod denies keeps
//!   the derived id, and the verb fails on devpod's own refusal.
//!
//! One thing the fold changed on purpose: **divergence row 20**. This module used
//! to name a path spec by `Path::canonicalize`, which follows symlinks, while
//! [`launch::plan`] normalises lexically. Two answers to "which workspace is
//! `./x`" is one answer too many, so the naming is now core's for both, and it is
//! the lexical one.

use devlaunch_core::clients::devpod::{ListingUnreadable, NotRun};
use devlaunch_core::domain::workspace_id::{UnsafeName, WorkspaceId};
use devlaunch_core::flows::launch::{self, LaunchNotice, Plan, Resolution};
use devlaunch_core::flows::lifecycle;
use devlaunch_core::flows::listing::CommandContext;
use devlaunch_core::runner::Runner;

use crate::cold::ColdPath;
use crate::session::StartupError;

/// Which workspace the target is, and everything the resolution had to say on the
/// way.
///
/// The records are not in here: they are the [`ColdPath`]'s, which is the caller's,
/// so a resolution that had to open them and a delete that needs them are looking
/// at one store rather than two. That is also what keeps the warm path clear of
/// `metadata.json` (devlaunch#145) — a target devpod recognises straight away is
/// resolved from one `devpod status`, and the [`ColdPath`] is never asked.
pub(crate) struct Addressed {
    pub(crate) workspace_id: String,
    /// Notices in [`LaunchNotice`]'s vocabulary because that is the vocabulary the
    /// shared resolution reports in; it carries the lifecycle and cache arms whole.
    pub(crate) notices: Vec<LaunchNotice>,
}

/// Why no workspace could be named.
pub(crate) enum Unaddressable {
    /// The target names no workspace devpod has and nothing dl could create an id
    /// for.
    Unknown { target: String },
    /// An owner, repo or ref that is not a safe git name — refused before anything
    /// builds a path out of it.
    Name(UnsafeName),
    /// devpod's listing could not be read, so "no such workspace" could not be
    /// told from "devpod did not answer".
    Listing(ListingUnreadable),
    /// The `devpod status` that would have said which workspace this is could not
    /// be run at all — devpod missing, blocked, or out of time. Its own arm rather
    /// than [`Unaddressable::Unknown`]: a devpod nobody can run has not denied
    /// anything, and Python raises out of `get_workspace_state` here.
    DevpodNotRun(NotRun),
    /// dl's own records or config could not be opened.
    Startup(StartupError),
}

impl From<StartupError> for Unaddressable {
    fn from(error: StartupError) -> Self {
        Unaddressable::Startup(error)
    }
}

impl From<UnsafeName> for Unaddressable {
    fn from(error: UnsafeName) -> Self {
        Unaddressable::Name(error)
    }
}

/// The workspace `target` names, asked of devpod first.
pub(crate) fn resolve<'r>(
    runner: &'r dyn Runner,
    context: &mut CommandContext<'r>,
    cold: &mut ColdPath<'r>,
    target: &str,
) -> Result<Addressed, Unaddressable> {
    match launch::plan(target)? {
        // A path or a git source: devpod names the workspace after the source, and
        // there is no triple to look a record up by. Nothing is asked of devpod
        // about it, as nothing is on the launch path either.
        Plan::Creatable { workspace_id, .. } => Ok(Addressed {
            workspace_id,
            notices: Vec::new(),
        }),
        // A bare name can only be a workspace devpod already has; everything
        // creatable is a path or a git spec and matched above.
        Plan::Existing { name } => existing(runner, context, name),
        Plan::Triple {
            owner,
            repo,
            branch,
            ..
        } => triple(context, cold, owner, repo, branch),
    }
}

/// `owner/repo[@branch]`: the derived id is a hint, and the record settles it when
/// devpod does not recognise the hint (devlaunch#88).
fn triple(
    context: &mut CommandContext<'_>,
    cold: &mut ColdPath<'_>,
    owner: String,
    repo: String,
    branch: Option<String>,
) -> Result<Addressed, Unaddressable> {
    let mut notices: Vec<LaunchNotice> = Vec::new();
    let branch = match branch {
        Some(branch) => branch,
        None => {
            // Row 23: the record first, then `git ls-remote --symref`. A verb on
            // its way to removing something must not clone a repository to find
            // out what its default branch is called.
            let records = cold.records()?;
            let mut cache = Vec::new();
            let named = records.clones.repo_manager().get_default_branch(
                &records.storage,
                &owner,
                &repo,
                &mut cache,
            );
            notices.extend(cache.into_iter().map(LaunchNotice::Cache));
            named
        }
    };
    // Constructing the WorkspaceId is the parse boundary: an unsafe owner, repo or
    // ref is rejected here, before it can name a container or a directory.
    let workspace = WorkspaceId::new(&owner, &repo, &branch)?;
    let resolved = launch::resolve_triple(context, cold, &workspace, &mut notices)
        .map_err(Unaddressable::DevpodNotRun)?;
    let workspace_id = match resolved {
        Resolution::Warm { placement } => placement.workspace_id().to_owned(),
        // devpod knows nothing about it. The derived id is what the verb addresses,
        // and devpod's own refusal is what the user sees — no clone, no record.
        Resolution::Cold { workspace } => workspace.value(),
    };
    Ok(Addressed {
        workspace_id,
        notices,
    })
}

/// A bare word: devpod's own answer, with its listing as the second opinion.
///
/// `status` failing is not the same as the workspace not existing, and the
/// difference decides whether the user can clean it up: `status` consults the
/// provider, while `list` only reads devpod's own records, so a workspace whose
/// provider is broken or gone still lists and cannot be described — and that is
/// precisely the workspace somebody is about to run `dl <ws> rm` on. The listing
/// gets the final word, at the price of one round trip on the failure path.
fn existing(
    runner: &dyn Runner,
    context: &mut CommandContext<'_>,
    name: String,
) -> Result<Addressed, Unaddressable> {
    let described = lifecycle::workspace_state(runner, &name).is_ok();
    if !described {
        let listed = context.workspaces().map_err(Unaddressable::Listing)?;
        if !listed.iter().any(|workspace| workspace.id == name) {
            return Err(Unaddressable::Unknown { target: name });
        }
    }
    Ok(Addressed {
        workspace_id: name,
        notices: Vec::new(),
    })
}
