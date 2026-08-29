//! `dl <ws> stop`.

use super::refresh::{Refresh, RefreshReason};
use crate::clients::devpod::{self, Call, NotRun};
use crate::flows::listing::CommandContext;
use crate::runner::Exit;

/// How a stop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    Stopped,
    /// devpod refused. Its own diagnostics are already on the user's stderr — the
    /// call inherits this process's streams, as Python's does — so there is
    /// nothing to carry but the ending.
    DevpodRefused {
        exit: Exit,
    },
}

/// Stop a workspace: `devpod stop <id>`.
///
/// A stopped workspace still appears in `devpod list`, with different details, so
/// the snapshot is forgotten either way — and the completion cache is wrong
/// regardless of age, which is why the refresh is [`RefreshReason::Forced`].
pub fn workspace_stop(
    context: &mut CommandContext<'_>,
    refresh: &mut Refresh<'_>,
    workspace_id: &str,
) -> Result<StopOutcome, NotRun> {
    let exit = devpod::run(context.runner(), &stop_call(workspace_id))?;
    context.forget_workspaces();
    refresh.ask(context.runner(), RefreshReason::Forced);
    Ok(if exit.is_success() {
        StopOutcome::Stopped
    } else {
        StopOutcome::DevpodRefused { exit }
    })
}

/// `devpod stop <id>` — argv-exact.
fn stop_call(workspace_id: &str) -> Call {
    Call::new(["stop", workspace_id])
}
