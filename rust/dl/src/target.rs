//! Which devpod workspace a target word names.
//!
//! `dl <target> stop` and `dl <target> rm` accept everything `dl <target>` does —
//! a workspace id, `owner/repo[@branch]`, a path, a git URL — and every one of
//! them has to become the *one* id devpod is addressed by. Python answers that
//! question in one block above its subcommand dispatch, shared by the launch and
//! by the lifecycle verbs; this is the lifecycle half of that block.
//!
//! # Two of Python's steps are deliberately not here
//!
//! Python's shared block is a *launch* resolution, and a lifecycle verb inherited
//! two things from it that only a launch wants:
//!
//! - `ensure_repo`, which clones the bare cache so a bare `owner/repo` can be
//!   asked for its default branch. Here the branch comes from
//!   [`RepositoryManager::get_default_branch`], which reads the record and then
//!   asks the remote for `HEAD` without a clone.
//! - `prepare_cold`, which clones the *workspace* whenever devpod does not
//!   recognise the derived id — so `dl owner/never-launched@main stop` created a
//!   clone directory and a metadata record on its way to stopping a workspace that
//!   does not exist. Nothing here creates anything.
//!
//! Both are named as divergence candidates in the M6 report; both only ever ran
//! on a target devpod could not place, where the verb was going to fail anyway.
//!
//! # Where this lives
//!
//! In the binary for now, because it is the *only* consumer: M7's launch flow
//! resolves the same target in core, and when it does, this module is what folds
//! into it — with the `pub` promotions in `domain::spec` and
//! `domain::workspace_id` folding back the other way.

use std::path::PathBuf;

use devlaunch_core::clients::devpod::ListingUnreadable;
use devlaunch_core::domain::spec::{self, SpecIdentity, WorkspaceSpec};
use devlaunch_core::domain::workspace_id::{NamePart, UnsafeName, validate_ref_name};
use devlaunch_core::flows::lifecycle::{self, LifecycleNotice};
use devlaunch_core::flows::listing::CommandContext;
use devlaunch_core::runner::Runner;

use crate::session::{self, Records, StartupError};

/// Which workspace the target is, and everything the resolution had to open or
/// say on the way.
///
/// `records` is an `Option` because the warm path must not load `metadata.json` at
/// all (devlaunch#145): a target devpod recognises straight away is resolved from
/// one `devpod status`, and the store is opened only once devpod has *denied* the
/// derived id. A caller that needs the records regardless opens them itself.
pub(crate) struct Addressed<'r> {
    pub(crate) workspace_id: String,
    pub(crate) records: Option<Records<'r>>,
    pub(crate) notices: Vec<LifecycleNotice>,
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
    context: &mut CommandContext<'_>,
    target: &str,
) -> Result<Addressed<'r>, Unaddressable> {
    match spec::parse(target) {
        // A triple: the derived id is a hint, and the record settles it when devpod
        // does not recognise the hint (devlaunch#88).
        WorkspaceSpec::OwnerRepo {
            owner,
            repo,
            branch,
        } => {
            // Before anything builds a path out of them: `repos_dir/<owner>/<repo>`
            // would otherwise act on a traversal first and reject it after.
            validate_ref_name(owner, NamePart::Owner)?;
            validate_ref_name(repo, NamePart::Repo)?;
            match branch {
                Some(branch) => Ok(known(
                    runner,
                    (owner, repo, branch),
                    derived_id(target)?,
                    None,
                    Vec::new(),
                )),
                None => {
                    let records = session::open_records(runner)?;
                    let mut cache = Vec::new();
                    let branch = &records.clones.repo_manager().get_default_branch(
                        &records.storage,
                        owner,
                        repo,
                        &mut cache,
                    );
                    let derived = derived_id(&format!("{owner}/{repo}@{branch}"))?;
                    Ok(known(
                        runner,
                        (owner, repo, branch.as_str()),
                        derived,
                        Some(records),
                        cache.into_iter().map(LifecycleNotice::Cache).collect(),
                    ))
                }
            }
        }
        // A path or a git source: devpod names the workspace after the source, and
        // there is no triple to look a record up by.
        WorkspaceSpec::Path(_)
        | WorkspaceSpec::Url(_)
        | WorkspaceSpec::HostPath(_)
        | WorkspaceSpec::SshUrl(_) => Ok(Addressed {
            workspace_id: derived_id(target)?,
            records: None,
            notices: Vec::new(),
        }),
        // A bare name can only be a workspace devpod already has; everything
        // creatable is a path or a git spec and matched above.
        WorkspaceSpec::ExistingIdOrName(name) => existing(runner, context, name),
    }
}

/// The resolution one `devpod status` and (only then) one record lookup make.
fn known<'r>(
    runner: &'r dyn Runner,
    triple: (&str, &str, &str),
    derived: String,
    already_open: Option<Records<'r>>,
    mut notices: Vec<LifecycleNotice>,
) -> Addressed<'r> {
    let (owner, repo, branch) = triple;
    // Opened by the lookup only if the lookup happens, which is what keeps the
    // warm path clear of the metadata lock, the parse and the migration.
    let mut opened = already_open;
    let resolved = lifecycle::resolve_known_workspace(
        runner,
        triple,
        &derived,
        || {
            if opened.is_none() {
                opened = session::open_records(runner).ok();
            }
            let records = opened.as_ref()?;
            lifecycle::recorded_devpod_workspace_id(&records.storage, owner, repo, branch)
        },
        &mut notices,
    );
    Addressed {
        workspace_id: resolved.workspace_id().to_owned(),
        records: opened,
        notices,
    }
}

/// A bare word: devpod's own answer, with its listing as the second opinion.
///
/// `status` failing is not the same as the workspace not existing, and the
/// difference decides whether the user can clean it up: `status` consults the
/// provider, while `list` only reads devpod's own records, so a workspace whose
/// provider is broken or gone still lists and cannot be described — and that is
/// precisely the workspace somebody is about to run `dl <ws> rm` on. The listing
/// gets the final word, at the price of one round trip on the failure path.
fn existing<'r>(
    runner: &'r dyn Runner,
    context: &mut CommandContext<'_>,
    name: &str,
) -> Result<Addressed<'r>, Unaddressable> {
    let described = lifecycle::workspace_state(runner, name).is_ok();
    if !described {
        let listed = context.workspaces().map_err(Unaddressable::Listing)?;
        if !listed.iter().any(|workspace| workspace.id == name) {
            return Err(Unaddressable::Unknown {
                target: name.to_owned(),
            });
        }
    }
    Ok(Addressed {
        workspace_id: name.to_owned(),
        records: None,
        notices: Vec::new(),
    })
}

/// The workspace id `spec` derives — Python's `spec_to_workspace_id`, total over
/// [`SpecIdentity`]'s four arms.
fn derived_id(spec: &str) -> Result<String, UnsafeName> {
    Ok(match spec::identity(spec)? {
        SpecIdentity::Workspace(id) => id,
        // A repo with no ref: not a workspace identity, and Python's answer for
        // one all the same. Unreachable from [`resolve`], which resolves the
        // default branch before it derives.
        SpecIdentity::RepoLabel(label) => label,
        SpecIdentity::PathLeaf(path) => path_leaf(path),
        SpecIdentity::ExistingName(name) => name.to_owned(),
    })
}

/// The directory name a path spec resolves to — Python's
/// `Path(spec).expanduser().resolve().name`.
///
/// A path that will not resolve keeps its own last component, which is what the
/// text already says the directory is called: there is no workspace to find either
/// way, and the id is what the "unknown workspace" refusal will name.
fn path_leaf(path: &str) -> String {
    let expanded = expanduser(path);
    let resolved = lifecycle::canonical(&expanded.to_string_lossy()).unwrap_or(expanded);
    resolved
        .file_name()
        .map(|leaf| leaf.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `~` and `~/…` against this user's home directory. `~user` is left alone, as
/// Python leaves it when it cannot name the user.
fn expanduser(path: &str) -> PathBuf {
    let Some(rest) = path.strip_prefix('~') else {
        return PathBuf::from(path);
    };
    let Some(home) = std::env::home_dir() else {
        return PathBuf::from(path);
    };
    match rest {
        "" => home,
        rest if rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
        _ => PathBuf::from(path),
    }
}
