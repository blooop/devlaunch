//! The launch verbs: `dl <spec>`, `dl <spec> -- <cmd>`, and the six words that
//! open a workspace some other way.
//!
//! One call into [`Launch::run`] and one rendering of what it answered. The five
//! stages, the fast-attach probe, the launch lock, the token and the tools are all
//! core's; what is here is the mapping from [`cli::Verb`] to [`LaunchVerb`], the
//! sentences, and the exit codes.
//!
//! # Two things this module owns that core deliberately does not
//!
//! - **Where devpod's session diagnostics go.** `attach_workspace` takes a
//!   `forward` closure and calls it *as the session runs*, because a session lives
//!   for hours and devpod's warning about it is worth nothing an hour late. Core
//!   writes to nobody's stream, so the sink is here.
//! - **Whether the tools get lent in.** [`Provision`] is a trait for the reason
//!   its docstring gives, and [`ToolProvisioning`] is the implementation that
//!   really provisions: it reads the two host facts once
//!   ([`ToolsSwitch::from_env`], [`HostLayout::from_env`]) and renders each pass's
//!   events at the moment the pass makes them.
//!
//! # When the notices are said
//!
//! [`Launch`] collects its notices and hands them over when `run` returns, so they
//! are printed after the launch rather than during it. For every verb that ends
//! without a session that is the same output in the same order; for the attach
//! family it is not — Python prints `Workspace X is already running, attaching...`
//! *before* the shell, and this prints it after. Making it live is a one-field
//! change in core's `Launch` (a `&mut dyn FnMut(LaunchNotice)` where the `Vec` is)
//! and is named as a follow-up rather than worked around here, because working
//! around it means the binary re-deciding the stage order core already decides.

use std::cell::Cell;
use std::path::Path;

use devlaunch_core::domain::spec::DevcontainerPath;
use devlaunch_core::flows::launch::{
    Host, Launch, LaunchAborted, LaunchRefusal, LaunchVerb, Launched, Provision, Session,
};
use devlaunch_core::flows::lifecycle::{Refresh, RefreshReason};
use devlaunch_core::flows::listing::CommandContext;
use devlaunch_core::flows::provision::{
    self, DevpodMissing, HostLayout, Provisioning, ToolsSwitch,
};
use devlaunch_core::runner::Runner;

use crate::cli::Verb;
use crate::cold::ColdPath;
use crate::commands::Ending;
use crate::render;

/// Which family a workspace verb is in.
///
/// The split is a value rather than a fall-through arm in the dispatcher, so that
/// adding a verb to [`Verb`] breaks [`family`] — the one exhaustive match over the
/// grammar's verbs — instead of quietly landing in whichever family the `_` arm
/// happened to be.
pub(crate) enum Family {
    /// `dl <ws> stop`.
    Stop,
    /// `dl <ws> rm` / `prune`, and whether `--force` was typed.
    Remove { force: bool },
    /// Everything that opens a workspace.
    Launch(LaunchVerb),
}

/// What each verb asks for, from the word the grammar resolved.
pub(crate) fn family(verb: &Verb) -> Family {
    Family::Launch(match verb {
        Verb::Stop => return Family::Stop,
        Verb::Remove { force } => return Family::Remove { force: *force },
        Verb::Attach => LaunchVerb::Attach { command: None },
        // Python's `" ".join(args[2:])`: the words are rejoined with single spaces
        // and the result is one shell command, quoted whole into the remote
        // payload. A word that needed quoting to survive the *host's* shell has
        // already been unquoted by it, so the join is what the user typed.
        Verb::Run(words) => LaunchVerb::Attach {
            command: Some(
                words
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<&str>>()
                    .join(" "),
            ),
        },
        Verb::Up => LaunchVerb::Up,
        Verb::Code => LaunchVerb::Code,
        Verb::Recreate => LaunchVerb::Recreate,
        Verb::Restart => LaunchVerb::Restart,
        Verb::Reset => LaunchVerb::Reset,
        Verb::Dotfiles => LaunchVerb::Dotfiles,
    })
}

/// Lending the host's tools into every workspace dl opens.
///
/// The two host facts are read once, when the value is built, rather than per pass:
/// a launch can provision twice (a sibling's `up` won the race, then this one's
/// `up` ran) and a switch that changed between them would make one launch two
/// different launches.
pub(crate) struct ToolProvisioning {
    tools: ToolsSwitch,
    host: Option<HostLayout>,
    /// Set if devpod went missing between the `up` that just worked and the pass
    /// that follows it.
    ///
    /// A `Cell` because [`Provision::provision_tools`] answers nothing — it cannot,
    /// since a launch does not branch on whether the tools landed — while this one
    /// failure is not a failure of the thing being attempted: Python gives
    /// `DevpodNotInstalled` a class that its `except OSError` cannot catch, so it
    /// travels out of the launch and `main()` renders exit 127 for it. Recording it
    /// here is how that number survives a trait that returns `()`.
    devpod_missing: Cell<bool>,
}

impl ToolProvisioning {
    /// What this host will lend, and whether it may.
    pub(crate) fn from_env() -> Self {
        Self {
            tools: ToolsSwitch::from_env(),
            // `None` is a machine with no home directory to look in: nothing to
            // lend, rather than nothing to do — the setup pass still runs, because
            // the stages it carries are not tools work.
            host: HostLayout::from_env(),
            devpod_missing: Cell::new(false),
        }
    }

    /// Whether a pass found devpod gone.
    pub(crate) fn lost_devpod(&self) -> bool {
        self.devpod_missing.get()
    }
}

impl Provision for ToolProvisioning {
    fn provision_tools(&self, runner: &dyn Runner, workspace_id: &str) {
        let mut events = Vec::new();
        let provisioned = provision::provision_tools(
            runner,
            workspace_id,
            self.tools,
            self.host.as_ref(),
            &mut events,
        );
        // Said here rather than collected for the end of the command: a cold
        // install streams hundreds of megabytes, and a warning about it is worth
        // something while it is still happening.
        for line in render::provision_events(&events) {
            eprintln!("{line}");
        }
        match provisioned {
            // Every way of coming up empty is one of these, and none of them is
            // worth a word beyond the events above: the workspace is up and the
            // user asked for a session, not for an install.
            Ok(outcome) => {
                let _: Provisioning = outcome;
            }
            Err(DevpodMissing) => self.devpod_missing.set(true),
        }
    }
}

/// One launch, rendered.
///
/// `cold` is the caller's so that a lifecycle verb and a launch verb of the same
/// command open dl's records at most once between them; `refresh` is the caller's
/// for the reason [`Refresh`] is one per command.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_launch<'r>(
    runner: &'r dyn Runner,
    context: &mut CommandContext<'r>,
    cache: &Path,
    refresh: &mut Refresh<'_>,
    cold: &mut ColdPath<'r>,
    target: &str,
    verb: &LaunchVerb,
    devcontainer: Option<&DevcontainerPath>,
) -> Ending {
    let host = Host::from_process(cache);
    let provision = ToolProvisioning::from_env();
    // Verbatim and as it happens: this is devpod's own stderr, minus the line it
    // buries a remote exit status in, and a session's warnings belong on the
    // terminal while the session is running.
    let mut forward = |line: &str| eprintln!("{line}");
    let (outcome, notices) = {
        let mut launch = Launch::new(context, refresh, cold, &provision, &host, &mut forward);
        let outcome = launch.run(target, verb, devcontainer);
        (outcome, launch.take_notices())
    };
    for line in render::launch_notices(&notices) {
        eprintln!("{line}");
    }
    let ending = ending_of(runner, refresh, verb, outcome);
    lost_devpod(&provision, ending)
}

/// The exit code this launch ends with, and the one line it may still have to
/// print.
fn ending_of(
    runner: &dyn Runner,
    refresh: &mut Refresh<'_>,
    verb: &LaunchVerb,
    outcome: Result<Launched, LaunchAborted>,
) -> Ending {
    match outcome {
        Err(aborted) => {
            eprintln!("{}", render::launch_abort(&aborted));
            if render::is_binary_missing(&aborted) {
                Ending::DevpodMissing
            } else {
                Ending::Refused
            }
        }
        // The session's own ending, whichever process the number came from:
        // Python's `return ret` from `attach_workspace`, negative status included.
        Ok(Launched::Session(session)) => Ending::Session(Session::exit_status(session)),
        Ok(Launched::Ready | Launched::AlreadyRunning) => Ending::Done,
        Ok(Launched::Refused(refused)) => {
            if let Some(line) = render::launch_refusal(&refused) {
                eprintln!("{line}");
            }
            match refused {
                // devpod's own status back, and nothing printed: its diagnostics
                // are already on this process's stderr, since the call inherits
                // the streams.
                LaunchRefusal::UpRefused { exit } => {
                    // Python asks for the refresh *before* it reads the return
                    // code for these two verbs, so a refused `up` still warms the
                    // cache; every other verb returns first. The latch is what
                    // keeps this from being a second spawn.
                    if matches!(verb, LaunchVerb::Up | LaunchVerb::Code) {
                        refresh.ask(runner, RefreshReason::Forced);
                    }
                    Ending::Child(exit)
                }
                LaunchRefusal::StopRefused { exit } => Ending::Child(exit),
                // Every refusal dl made on its own account: Python's
                // `logging.error(...); return 1`, and the line is already printed.
                LaunchRefusal::UnsafeSpec(_)
                | LaunchRefusal::UnknownWorkspace { .. }
                | LaunchRefusal::BranchNotNamed { .. }
                | LaunchRefusal::NotPrepared { .. }
                | LaunchRefusal::NoSession(_) => Ending::Refused,
            }
        }
    }
}

/// A devpod that went missing during provisioning, which is exit 127.
///
/// Only where no session ran: a launch that already handed the user's command its
/// terminal has an ending of its own, and replacing that command's status with a
/// diagnostic about the tools would lose the number the user is reading. Python
/// never reaches either case — the exception aborts the launch in front of the
/// attach — which is why this arm is a rendering of the same exit code rather than
/// a reproduction of the control flow.
fn lost_devpod(provision: &ToolProvisioning, ending: Ending) -> Ending {
    if !provision.lost_devpod() {
        return ending;
    }
    eprintln!("{}", render::DEVPOD_MISSING);
    match ending {
        Ending::Session(status) => Ending::Session(status),
        _ => Ending::DevpodMissing,
    }
}
