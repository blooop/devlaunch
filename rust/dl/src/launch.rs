//! The launch verbs: `dl <spec>`, `dl <spec> -- <cmd>`, and the six words that
//! open a workspace some other way.
//!
//! One call into [`Launch::run`] and one rendering of what it answered. The five
//! stages, the fast-attach probe, the launch lock, the token and the tools are all
//! core's; what is here is the mapping from [`cli::Verb`] to [`LaunchVerb`], the
//! sentences, and the exit codes.
//!
//! # The one thing this module owns that core deliberately does not
//!
//! **Where devpod's session diagnostics go.** `attach_workspace` takes a `forward`
//! closure and calls it *as the session runs*, because a session lives for hours
//! and devpod's warning about it is worth nothing an hour late. Core writes to
//! nobody's stream, so the sink is here.
//!
//! It used to own a second thing — [`ToolProvisioning`], the implementation that
//! really lends the tools in — and that moved to core in #340. What kept it here
//! was the events sink, which is now a value the constructor takes: the pass reads
//! the host's facts and streams its events, and this module supplies the stream and
//! the words for it and nothing else.
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
use devlaunch_core::domain::workspace_id::WorkspaceId;
use devlaunch_core::flows::completion_cache;
use devlaunch_core::flows::launch::{
    self, ColdPath, Host, Launch, LaunchAborted, LaunchRefusal, LaunchVerb, Launched, Plan,
    Session, ToolProvisioning,
};
use devlaunch_core::flows::lifecycle::Refresh;
use devlaunch_core::flows::listing::CommandContext;

use crate::cli::{RmOnExit, Verb};
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
    /// `dl <ws> kill`.
    ///
    /// Its own family rather than a variant of [`Family::Stop`], because the sweep
    /// asks devpod nothing at all: `stop` is a `devpod stop` and needs a devpod that
    /// answers, and this is what is left when it does not.
    ///
    /// Not a variant of [`Family::Remove`] either, though it ends in that family's
    /// delete. The order is the whole verb: what the sweep does first is what lets
    /// the delete through, so a dispatcher that treated this as a removal with an
    /// extra step would have the two halves the wrong way round. It carries no
    /// `--force` for the same reason [`Verb::Kill`] does not: the delete it reaches
    /// is already the insisting one.
    Kill,
    /// `dl <ws> rm`, and whether `--force` was typed.
    ///
    /// `dl <ws> rme` is this family too, and carries nothing extra: the hangup is
    /// not part of removing a workspace, it is what happens when the whole command
    /// is over. One picked batch is many passes through here and one shell, so a
    /// field on this would be read once per workspace by a dispatcher that must act
    /// on it once — see [`crate::hangup`], which is asked at the other end.
    Remove { force: bool },
    /// Everything that opens a workspace, and whether the workspace is to go once
    /// the session does.
    ///
    /// [`RmOnExit`] rides *with* the launch verb rather than beside it in the
    /// dispatcher, because the two are read together exactly once and only ever
    /// together: the removal is what happens after this launch, not a second thing
    /// the command was asked for.
    Launch { verb: LaunchVerb, rm: RmOnExit },
}

/// What each verb asks for, from the word the grammar resolved.
///
/// The `RmOnExit::No` on the six word verbs is not a default this function chose: the
/// grammar refuses `--rm` on all of them ([`cli::GrammarError::RmNotAllowed`]),
/// so no other answer can reach here — the arms say it because [`Verb`] gives them
/// nothing to say it with.
pub(crate) fn family(verb: &Verb) -> Family {
    let (launched, rm) = match verb {
        Verb::Stop => return Family::Stop,
        Verb::Kill => return Family::Kill,
        // `after` is deliberately not read here: it belongs to the command's
        // ending rather than to this pass over one workspace.
        Verb::Remove { force, after: _ } => return Family::Remove { force: *force },
        Verb::Attach { rm } => (LaunchVerb::Attach { command: None }, *rm),
        // Python's `" ".join(args[2:])`: the words are rejoined with single spaces
        // and the result is one shell command, quoted whole into the remote
        // payload. A word that needed quoting to survive the *host's* shell has
        // already been unquoted by it, so the join is what the user typed.
        Verb::Run(words, rm) => (
            LaunchVerb::Attach {
                command: Some(
                    words
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<&str>>()
                        .join(" "),
                ),
            },
            *rm,
        ),
        Verb::Up => (LaunchVerb::Up, RmOnExit::No),
        Verb::Code => (LaunchVerb::Code, RmOnExit::No),
        Verb::Recreate => (LaunchVerb::Recreate, RmOnExit::No),
        Verb::Restart => (LaunchVerb::Restart, RmOnExit::No),
        Verb::Reset => (LaunchVerb::Reset, RmOnExit::No),
        Verb::Dotfiles => (LaunchVerb::Dotfiles, RmOnExit::No),
    };
    Family::Launch { verb: launched, rm }
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
    cold: &mut ColdPath<'r, '_>,
    target: &str,
    verb: &LaunchVerb,
    devcontainer: Option<&DevcontainerPath>,
    recognised: Option<WorkspaceId>,
) -> Ran {
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
        // Nothing was opened, so there is nothing for `--rm` to close.
        return Ran {
            ending: Ending::Refused,
            reached: Reached::Nothing,
        };
    }
    let host = Host::from_process(cache);
    // The pass's events stream through a sink of their own, and it is the same
    // printer as the launch's notices: one line on stderr at the moment core makes
    // the event, which is Python's order. A cold install streams hundreds of
    // megabytes, and a warning about it is worth something while it is still
    // happening.
    let mut lending = render::Saying;
    let provision = ToolProvisioning::from_env(cache, &mut lending);
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
        )
        .recognised_as(recognised);
        launch.run(target, verb, devcontainer)
    };
    ran(outcome, cache)
}

/// How far a launch got, for the one caller that has to clean up after it.
///
/// **Not derivable from [`Ending`], which is why it is carried.** `Ending::Refused`
/// is both "no such workspace" and "the container came up and the session would not
/// open", and those are opposite answers to "is there something to remove": the
/// first created nothing, the second left a running container and the clone behind.
/// Reading the exit code to guess between them is how `--rm` came to leak the
/// workspaces it exists to collect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reached {
    /// devpod was asked to bring this workspace up — or it was already up and
    /// attached to. A devpod workspace, a container, and a clone may all exist,
    /// **including when the `up` failed**: a create that dies in its lifecycle hooks
    /// leaves the container running and devpod's record written, which is the whole
    /// reason `clients::devpod_home::create_record` exists.
    TheWorkspace,
    /// The launch stopped before devpod was asked for anything: an unsafe spec, a
    /// workspace nothing answers to, a default branch that could not be named, a
    /// host-side clone that was never cut, a devpod that could not be run.
    ///
    /// Nothing to remove, and a removal attempted anyway would answer one refusal
    /// with a second unrelated one — `devpod delete` on an id devpod never had fails,
    /// and the sentence it fails with tells the user to restore a `devcontainer.json`
    /// that was never the problem.
    Nothing,
}

/// One launch: how it ended, and how far it got.
pub(crate) struct Ran {
    pub(crate) ending: Ending,
    pub(crate) reached: Reached,
}

/// The exit code this launch ends with, how far it got, and the one line it may
/// still have to print.
///
/// `cache` is here for one line only: a clone the host answered "no such
/// repository" to is checked against the completion cache, so a mistyped owner is
/// told the name it probably meant instead of being left with git's ssh advice.
/// Read on this path and no other — the file is opened only once a launch has
/// already failed.
fn ran(outcome: Result<Launched, LaunchAborted>, cache: &Path) -> Ran {
    match outcome {
        Err(aborted) => {
            eprintln!("{}", render::launch_abort(&aborted));
            // `SshNotRun` is the odd one: devpod worked, so the workspace is up and
            // only OpenSSH is missing. The other two never got as far as devpod.
            let reached = match aborted {
                LaunchAborted::SshNotRun(_) => Reached::TheWorkspace,
                LaunchAborted::DevpodNotRun(_) | LaunchAborted::ListingUnreadable(_) => {
                    Reached::Nothing
                }
            };
            let ending = if render::is_binary_missing(&aborted) {
                Ending::DevpodMissing
            } else {
                Ending::Refused
            };
            Ran { ending, reached }
        }
        // The session's own ending, whichever process the number came from:
        // Python's `return ret` from `attach_workspace`, negative status included.
        Ok(Launched::Session(session)) => Ran {
            ending: Ending::Session(Session::exit_status(session)),
            reached: Reached::TheWorkspace,
        },
        Ok(Launched::Ready | Launched::AlreadyRunning) => Ran {
            ending: Ending::Done,
            reached: Reached::TheWorkspace,
        },
        Ok(Launched::Refused(refused)) => {
            if let Some(line) = render::launch_refusal(&refused) {
                eprintln!("{line}");
            }
            if let Some(known) =
                completion_cache::read_completion_cache(&completion_cache::cache_path(cache))
                && let Some(hint) = render::wrong_owner_hint(&refused, &known)
            {
                eprintln!("{hint}");
            }
            match refused {
                // devpod's own status back, and nothing printed: its diagnostics
                // are already on this process's stderr, since the call inherits
                // the streams.
                // Whether a refused `up` still warmed the completion cache is
                // core's: it is a step of the launch, taken where Python takes it.
                //
                // `Reached::TheWorkspace` for both, and for `UpRefused` that is the
                // point: a build that failed in `postCreateCommand` leaves the
                // container running and the clone cut, so this is exactly the
                // workspace an unattended `--rm` line is there to collect.
                LaunchRefusal::UpRefused { exit } => Ran {
                    ending: Ending::Child(exit),
                    reached: Reached::TheWorkspace,
                },
                LaunchRefusal::StopRefused { exit } => Ran {
                    ending: Ending::Child(exit),
                    reached: Reached::TheWorkspace,
                },
                // The container came up and no session could be opened. The
                // workspace is there; only the shell is not.
                LaunchRefusal::NoSession(_) => Ran {
                    ending: Ending::Refused,
                    reached: Reached::TheWorkspace,
                },
                // Every refusal dl made before devpod was asked for anything:
                // Python's `logging.error(...); return 1`, and the line is already
                // printed. `NotPrepared` is here rather than above because the
                // failure *is* the clone — there is no devpod workspace to delete,
                // and a directory a half-finished prepare left behind is what
                // `dl --prune` lists.
                LaunchRefusal::UnsafeSpec(_)
                | LaunchRefusal::UnknownWorkspace { .. }
                | LaunchRefusal::IdCollision { .. }
                | LaunchRefusal::BranchNotNamed { .. }
                | LaunchRefusal::NotPrepared { .. } => Ran {
                    ending: Ending::Refused,
                    reached: Reached::Nothing,
                },
            }
        }
    }
}
