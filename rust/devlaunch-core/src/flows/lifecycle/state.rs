//! Which workspace a name means, and what state that workspace is in.

use super::notices::LifecycleNotice;
use crate::clients::devpod::{self, ContainerState, NotRun, Patience, StatusUnreadable};
use crate::domain::metadata::MetadataStorage;
use crate::domain::workspace_id::WorkspaceId;
use crate::notices::Notices;
use crate::runner::Runner;
use crate::timing;

/// The container state devpod reports for one workspace.
///
/// Charged to the `devpod-up` stage, as Python's `@timing.staged("devpod-up")`
/// charges it: this round trip is what a warm attach spends instead of building a
/// container, and a summary that left it uncharged would show a launch with a gap
/// in it. A stage *guard* rather than
/// [`timing::stage_result`](crate::timing::stage_result), because an unreadable
/// answer is Python's `None` return and not an exception — the stage completed,
/// devpod just had nothing to say.
pub fn workspace_state(
    runner: &dyn Runner,
    workspace_id: &str,
    patience: Patience,
) -> Result<ContainerState, StatusUnreadable> {
    let mut stage = timing::stage(timing::Stage::DevpodUp);
    let answer = devpod::status(runner, workspace_id, patience);
    // Python's `@timing.staged("devpod-up") get_workspace_state` returns `None`
    // for a devpod that ran and refused, gave non-JSON, or omitted `state` — the
    // stage stays `ok`. Only a devpod that could not be run at all raises
    // (`DevpodNotInstalled`, or another spawn `OSError`) and the decorator marks
    // the stage `failed`. `NotRun` is that case; mark it so the timing document
    // does not report `ok` for a launch step devpod never performed (P12/C8).
    if matches!(answer, Err(StatusUnreadable::NotRun(_))) {
        stage.fail();
    }
    answer
}

/// Which devpod workspace a triple is, and what devpod said about it.
///
/// A sum rather than Python's `(workspace_id, Optional[state])` pair, whose
/// docstring has to promise that *state is None exactly when devpod knows no
/// workspace for this triple, in which case workspace_id is the derived id*. Both
/// halves of that promise are the type here: there is no way to build a
/// [`KnownWorkspace::Unknown`] carrying a recorded id, and no way to read a state
/// off one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KnownWorkspace {
    /// devpod knows this workspace and reported this state. The two come from one
    /// round trip: asking "which id" and then "what state" separately is how a
    /// command ends up addressing one workspace and reporting another's state.
    Known {
        workspace_id: String,
        state: ContainerState,
    },
    /// devpod knows no workspace for this triple. `derived` is the id a create
    /// would use.
    Unknown { derived: String },
}

impl KnownWorkspace {
    /// The id every later step addresses, whichever arm this is.
    ///
    /// Only this module's tests read it; the flows match the arms directly.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn workspace_id(&self) -> &str {
        match self {
            Self::Known { workspace_id, .. } => workspace_id,
            Self::Unknown { derived } => derived,
        }
    }

    /// The state devpod gave, or nothing when it knows no such workspace.
    ///
    /// Only this module's tests (via [`Self::is_running`]) read it; the flows
    /// match the arms directly.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn state(&self) -> Option<&ContainerState> {
        match self {
            Self::Known { state, .. } => Some(state),
            Self::Unknown { .. } => None,
        }
    }

    /// Whether a launch may attach straight away.
    ///
    /// Only this module's tests read it; the flows match the arms directly.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_running(&self) -> bool {
        matches!(self.state(), Some(state) if state.is_running())
    }
}

/// Which devpod workspace `(owner, repo, branch)` is, asked of devpod first.
///
/// devlaunch#88. The id `dl` hands devpod used to be derived on every command and
/// written down nowhere, so the derivation was the only copy of it that existed.
/// PR #81 moved that derivation and every workspace created under the old one
/// became unaddressable in the same instant — 36 of 39 on the reporting host.
/// Nothing was lost and nothing was corrupted; `dl` simply began asking devpod
/// about ids devpod had never been given. The record is the second copy, and this
/// is where it is read.
///
/// **Derived first, record second, and that order is a trade rather than a
/// shortcut.** Reading the record means loading `metadata.json` under its lock,
/// parsing it and running the id-scheme migration's version check — three things
/// devlaunch#145 deliberately took off the warm attach path, which is the path a
/// user waits on. Asking devpod about the derived id is already paid for by the
/// status round trip this function is built around, so the record is consulted
/// only once devpod has *denied* the derived id, which is the only case in which
/// it can say anything new.
///
/// That ordering is why `recorded_id` is a closure rather than an
/// `Option<String>`: a parameter would have to be computed by the caller before
/// the call, which is exactly the metadata read this defers. Passing the *lookup*
/// makes "the warm path reads no metadata" a property of the signature.
///
/// A stored id devpod also denies is not used. `metadata.json` is append-mostly
/// and nothing prunes it, so a record naming a workspace deleted months ago is
/// ordinary; addressing it would substitute one absent workspace for another and
/// lose the derived id a create needs.
///
/// **A devpod that could not be run at all is not a denial.** Python's
/// `get_workspace_state` folds a non-zero exit into `None` but *raises*
/// `DevpodNotInstalled` (and its siblings) out of `run_devpod`, so a host with no
/// devpod on it ends the command here rather than being told its workspace is
/// unknown. Reading the two the same way is worse than a wrong message: the cold
/// path it sends a launch down fetches a branch and builds a workspace clone on a
/// host that cannot open it, and leaves both behind for the exit-127 to be
/// discovered after. So the error is the runner's, and it travels.
///
/// **The triple and the id it derives arrive as one value**, which is what keeps
/// the notice below honest. They used to be two arguments -- a `(owner, repo,
/// ref)` tuple and a `derived: &str` beside it -- and nothing tied them together,
/// while the one sentence this function emits names *both*: "addressing
/// `<recorded>` instead of `<derived>` for `<owner>/<repo>@<branch>`". A caller
/// that handed over an id from a different triple got a line about a workspace
/// nobody has, and a launch pointed at whichever of the two the record matched.
pub(crate) fn resolve_known_workspace(
    runner: &dyn Runner,
    workspace: &WorkspaceId,
    recorded_id: impl FnOnce() -> Option<String>,
    notices: &mut dyn Notices<LifecycleNotice>,
    patience: Patience,
) -> Result<KnownWorkspace, NotRun> {
    let derived = workspace.value();
    match workspace_state(runner, derived, patience) {
        Ok(state) => {
            return Ok(KnownWorkspace::Known {
                workspace_id: derived.to_owned(),
                state,
            });
        }
        Err(StatusUnreadable::NotRun(not_run)) => return Err(not_run),
        // devpod ran and refused, answered something unparsable, or answered
        // without a state: Python's three `None`s, and all three mean "devpod
        // knows no workspace by this name".
        Err(_) => {}
    }
    let unknown = || {
        Ok(KnownWorkspace::Unknown {
            derived: derived.to_owned(),
        })
    };
    let Some(recorded) = recorded_id() else {
        return unknown();
    };
    if recorded == derived {
        return unknown();
    }
    let state = match workspace_state(runner, &recorded, patience) {
        Ok(state) => state,
        Err(StatusUnreadable::NotRun(not_run)) => return Err(not_run),
        Err(_) => return unknown(),
    };
    notices.say(LifecycleNotice::AddressingRecordedWorkspace {
        recorded: recorded.clone(),
        derived: derived.to_owned(),
        owner: workspace.owner().to_owned(),
        repo: workspace.repo().to_owned(),
        branch: workspace.git_ref().to_owned(),
    });
    Ok(KnownWorkspace::Known {
        workspace_id: recorded,
        state,
    })
}

/// The devpod workspace id `metadata.json` holds for a triple, if any.
///
/// `None` covers both "no record" and "a record from before this field was ever
/// written", which are the same answer to the only question asked of it: there is
/// nothing here to follow, so the derivation stands.
///
/// A cache dl cannot read answers `None` too, and that is one level up: a store
/// that could not be opened never becomes a [`MetadataStorage`], so the caller
/// holding one has already handled the failure — and the way it handles it is to
/// pass a closure that answers `None`, because a lookup that failed must not be
/// able to stop a command that would otherwise have worked.
pub(crate) fn recorded_devpod_workspace_id(
    storage: &MetadataStorage,
    owner: &str,
    repo: &str,
    branch: &str,
) -> Option<String> {
    storage
        .get_worktree(owner, repo, branch)?
        .devpod_workspace_id
        .clone()
}
