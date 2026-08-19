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
//! As they happen. [`Launch`] takes a notice *sink*
//! ([`devlaunch_core::notices::Notices`]) rather than filling a list the binary
//! drains at the end, and the sink here is [`render::Saying`] — one line on stderr
//! per notice, at the moment core says it. That is Python's order for every verb,
//! and it is the only order two of these lines are worth anything in: `Cloning
//! repository …` explains a wait that has not finished yet, and `Workspace X is
//! already running, attaching...` announces a shell that is about to take over the
//! terminal. The binary does not re-decide the order — it does not know it; core's
//! stages do.

use std::path::Path;

use devlaunch_core::domain::spec::DevcontainerPath;
use devlaunch_core::flows::launch::{
    self, Host, Launch, LaunchAborted, LaunchRefusal, LaunchVerb, Launched, Plan, Provision,
    Session,
};
use devlaunch_core::flows::lifecycle::Refresh;
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
        }
    }
}

impl Provision for ToolProvisioning {
    fn provision_tools(
        &self,
        runner: &dyn Runner,
        workspace_id: &str,
    ) -> Result<(), DevpodMissing> {
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
        // Every way of coming up empty is an arm of `Provisioning`, and none of them
        // is worth a word beyond the events above: the workspace is up and the user
        // asked for a session, not for an install. A devpod that has gone missing is
        // the one answer that travels — the launch cannot go on without it, and core
        // ends the launch with it.
        provisioned.map(|outcome| {
            let _: Provisioning = outcome;
        })
    }
}

/// One launch, rendered.
///
/// `cold` is the caller's so that a lifecycle verb and a launch verb of the same
/// command open dl's records at most once between them; `refresh` is the caller's
/// for the reason [`Refresh`] is one per command.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_launch<'r>(
    context: &mut CommandContext<'r>,
    cache: &Path,
    refresh: &mut Refresh<'_>,
    cold: &mut ColdPath<'r>,
    target: &str,
    verb: &LaunchVerb,
    devcontainer: Option<&DevcontainerPath>,
) -> Ending {
    // A path or git source whose derived id is empty — `dl /`, `//`, `/.`, `/..`,
    // all of which normalise to a leaf with no final component — would otherwise
    // hand devpod `--id ""` and run a nameless workspace to a reported success
    // (exit 0). Python derived the same empty name and devpod's own lookup failed
    // it (exit 1). An empty id names nothing, so this refuses it before opening
    // anything rather than opening one that cannot be addressed again. (Divergence:
    // Python reached exit 1 through devpod; this reaches it by refusing, so the
    // words differ while the ending matches — docs row for the empty derived id.)
    if let Ok(Plan::Creatable { workspace_id, .. }) = launch::plan(target)
        && workspace_id.is_empty()
    {
        eprintln!(
            "{} does not name a workspace: its path has no final component to name one after.",
            render::python_repr(target)
        );
        return Ending::Refused;
    }
    let host = Host::from_process(cache);
    let provision = ToolProvisioning::from_env();
    // Verbatim and as it happens: this is devpod's own stderr, minus the line it
    // buries a remote exit status in, and a session's warnings belong on the
    // terminal while the session is running.
    let mut forward = |line: &str| eprintln!("{line}");
    // The same treatment for dl's own notices: core says each one when it happens,
    // and this is where it lands.
    let mut said = render::Saying;
    let outcome = {
        let mut launch = Launch::new(
            context,
            refresh,
            cold,
            &provision,
            &host,
            &mut forward,
            &mut said,
        );
        launch.run(target, verb, devcontainer)
    };
    ending_of(outcome)
}

/// The exit code this launch ends with, and the one line it may still have to
/// print.
fn ending_of(outcome: Result<Launched, LaunchAborted>) -> Ending {
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
                // Whether a refused `up` still warmed the completion cache is
                // core's: it is a step of the launch, taken where Python takes it.
                LaunchRefusal::UpRefused { exit } => Ending::Child(exit),
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
