//! Making an agent inside a workspace visible to a session manager outside it.
//!
//! [`crate::clients::herdr`] holds every decision this flow makes and every string
//! it sends; this is the round trips, in the order they have to happen:
//!
//! 1. [`start_forward`] opens the connection that carries the manager's socket in,
//!    and hands back something the caller stops when the session ends.
//! 2. [`prepare`] asks the container what it already has -- the socket from step 1
//!    included -- and puts the manager's own binary and a Claude Code hook there if
//!    it is missing either. One round trip in the steady state, three when there is
//!    work to do. The question is asked every launch on purpose: nothing on this
//!    host can know whether the container that was prepared is the container that
//!    is running now, and nothing on this host can see whether a detached forward
//!    bound its listen path.
//!
//! In that order, because the forward is what step 2 asks about. It is also the
//! cheap one to undo: a `prepare` that refuses leaves the caller a forward to stop
//! and no container state to unpick.
//!
//! Both are no-ops unless the launch resolved a [`Reporting`], which needs the
//! consent variable *and* a manager's coordinates in this process's environment.
//! Nothing here runs on an ordinary launch.
//!
//! **Every failure is reported and then tolerated.** A workspace whose sudo is
//! locked down, a container with no `/etc`, a forward the container user cannot
//! bind: each of those costs the reporting and none of them costs the session. The
//! alternative would be refusing to open a shell because a status indicator could
//! not be wired up, which is the wrong way round.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::clients::herdr::{self, Reporting};
use crate::clients::kill::{self, Signal};
use crate::clients::{devpod, ssh};
use crate::domain::workspace_state::NonEmpty;
use crate::flows::launch;
use crate::runner::interrupt;
use crate::runner::{DetachOutcome, Exit, Invocation, Outcome, Runner, SpawnSpec};

/// What [`prepare`] came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Prepared {
    /// The container has the binary and the hook, whether or not this call is what
    /// put them there.
    Ready,
    /// It does not, and this is what stopped it. The session still opens.
    Refused { reason: String },
}

/// Put the manager's binary and the hook that calls it inside the container.
///
/// Three trips at worst and one at best, and the one is not optional. A cache of
/// what a workspace was given used to make the steady state free, and it was
/// wrong in the way a cache of someone else's state is always wrong: `dl <ws>
/// recreate`, `dl <ws> reset`, a `devpod delete`, or a rebuilt image all replace
/// the container and none of them change the workspace id, so a marker keyed on
/// that id went on describing a container that no longer existed. dl then printed
/// "reporting agents in this workspace" over a container that had never heard of
/// herdr, and no agent inside it was ever visible again -- the exact silent
/// nothing this whole flow exists to remove, now stated positively.
///
/// The container is the only thing that knows, so the container is asked. The
/// probe compares the binary's size, which is the cheapest thing that changes
/// when the manager is updated, and it asks after the forwarded socket in the
/// same trip -- which is why it runs *after* [`start_forward`] and not before.
pub(crate) fn prepare(
    runner: &dyn Runner,
    config: &Path,
    workspace_id: &str,
    reporting: &Reporting,
) -> Prepared {
    let Ok(metadata) = std::fs::metadata(reporting.host_binary()) else {
        return Prepared::Refused {
            reason: format!(
                "{} names {}, which this host cannot read",
                herdr::BIN_VAR,
                reporting.host_binary().display()
            ),
        };
    };
    // A non-zero probe is the ordinary answer and not a failure -- it is a `test`,
    // and the container has not been prepared yet. Only a trip that never ran at
    // all stops a launch; see [`Trip`], which is where that has gone wrong twice.
    match run_remote(
        runner,
        config,
        workspace_id,
        &herdr::probe_command(metadata.len(), &reporting.container_socket()),
        None,
        "probe the container",
    ) {
        // Prepared already, by this launch or an earlier one, and nothing to do.
        Trip::Ran { exit, .. } if exit.is_success() => return Prepared::Ready,
        // The forward reported a pid and delivered nothing. Lending into this
        // container would work and buy nothing: the hook would fire, find no
        // socket, and stay quiet. Nothing dl can install repairs it, so this is
        // the one probe answer that is a refusal rather than a to-do list.
        Trip::Ran {
            exit: Exit::Code(herdr::PROBE_NO_SOCKET),
            ..
        } => {
            return Prepared::Refused {
                reason: format!(
                    "the manager's socket did not arrive at {} in the workspace, so the \
                     forward bound nothing",
                    reporting.container_socket()
                ),
            };
        }
        Trip::Ran { .. } => {}
        Trip::NeverRan { reason } => return Prepared::Refused { reason },
    }

    for errand in [
        Errand {
            command: herdr::lend_command().to_owned(),
            stdin: Some(reporting.host_binary().to_path_buf()),
            doing: "lend the session manager",
            refuses: None,
        },
        Errand {
            command: herdr::install_command(),
            stdin: None,
            doing: "install the agent hook",
            refuses: Some((
                herdr::INSTALL_FOREIGN_SETTINGS,
                format!(
                    "the workspace already holds managed Claude Code settings dl did not write \
                     at {}, and whatever policy they carry is worth more than a status indicator",
                    herdr::CONTAINER_SETTINGS
                ),
            )),
        },
    ] {
        let Errand {
            command,
            stdin,
            doing,
            refuses,
        } = errand;
        match run_remote(runner, config, workspace_id, &command, stdin, doing) {
            Trip::Ran { exit, .. } if exit.is_success() => {}
            Trip::Ran { exit, complaint } => {
                return Prepared::Refused {
                    reason: match refuses {
                        Some((code, said)) if exit == Exit::Code(code) => said,
                        _ => refusal(doing, complaint.as_deref()),
                    },
                };
            }
            Trip::NeverRan { reason } => return Prepared::Refused { reason },
        }
    }

    Prepared::Ready
}

/// One thing [`prepare`] asks the container to do.
///
/// `refuses` is the one status this errand answers with that means something more
/// specific than "it failed", so the refusal can say the specific thing. Only the
/// install has one, and a status is per-errand rather than global because the
/// numbers are only meaningful against the command that returned them.
struct Errand {
    command: String,
    stdin: Option<std::path::PathBuf>,
    doing: &'static str,
    refuses: Option<(i32, String)>,
}

/// What one non-interactive trip into the workspace came to.
///
/// Two states, and the line between them is drawn by the **exit status alone**:
/// a command that ran and said no is an answer whatever it printed on the way,
/// and only a trip that never happened is a refusal. Drawing it anywhere else is
/// the bug this shape has now had twice. The first spelling read an empty pair of
/// streams as the answer, which left a `test` that fails silently reported as a
/// refusal; the second read *any* output as a refusal, which is worse, because
/// output on a failed trip is the common case rather than the odd one -- Debian's
/// bash runs the remote `~/.bashrc` on `ssh host <cmd>`, so a pixi or nvm init
/// line, a motd, or a locale warning rides along with every exit status.
///
/// The complaint therefore travels beside the status instead of standing in for
/// it: a refusal that has one still quotes the container's own last word.
enum Trip {
    /// The container ran the command, and this is how it ended and what it said.
    Ran {
        exit: Exit,
        complaint: Option<String>,
    },
    /// Nothing ran, or nothing came back. The reason is the user's to read.
    NeverRan { reason: String },
}

/// One non-interactive trip into the workspace.
fn run_remote(
    runner: &dyn Runner,
    config: &Path,
    workspace_id: &str,
    command: &str,
    // The lend's payload, and the one trip that has one: `Some` is a file handed
    // over as the child's stdin, `None` a trip that reads nothing.
    stdin: Option<std::path::PathBuf>,
    doing: &str,
) -> Trip {
    let invocation = Invocation::new(ssh::PROGRAM)
        .with_arg("-F")
        .with_arg(config.display().to_string())
        .with_arg(ssh::host_alias(workspace_id))
        .with_arg(command);
    let spec = match stdin {
        Some(path) => SpawnSpec::new(invocation).with_stdin_file(path),
        None => SpawnSpec::new(invocation),
    };
    match runner.capture(&spec) {
        Outcome::Ran { exit, io } => {
            let complaint = last_line(&io.stderr).or_else(|| last_line(&io.stdout));
            match exit {
                // ssh's own 255 is the one exit that means the trip did not
                // happen: the workspace is unreachable, which is a different
                // thing from a command inside it saying no. A signalled ssh is
                // the same news -- whatever the remote command was doing, this
                // end stopped listening before it heard the answer.
                Exit::Code(SSH_TRANSPORT_FAILURE) | Exit::Signal(_) => Trip::NeverRan {
                    reason: refusal(doing, complaint.as_deref()),
                },
                exit => Trip::Ran { exit, complaint },
            }
        }
        Outcome::ProgramNotFound => Trip::NeverRan {
            reason: format!("could not {doing}: no ssh on this host"),
        },
        Outcome::TimedOut => Trip::NeverRan {
            reason: format!("could not {doing}: the workspace did not answer"),
        },
        Outcome::NotStarted(failure) => Trip::NeverRan {
            reason: format!("could not {doing}: {failure:?}"),
        },
    }
}

/// One refusal sentence, with the container's last word if it had one.
fn refusal(doing: &str, complaint: Option<&str>) -> String {
    match complaint {
        Some(said) => format!("could not {doing}: {said}"),
        None => format!("could not {doing}"),
    }
}

/// OpenSSH's own failure exit, which is not the remote command's.
const SSH_TRANSPORT_FAILURE: i32 = 255;

/// The last line that says anything, which is where a shell puts its complaint.
fn last_line(stream: &str) -> Option<String> {
    stream
        .lines()
        .map(str::trim)
        // devpod's alias resolves no hostname inside the container, so `sudo`
        // warns about it on every single trip. It is not this flow's news.
        .rfind(|line| !line.is_empty() && !line.contains("unable to resolve host"))
        .map(str::to_owned)
}

/// The connection that carries the manager's socket into the container.
///
/// Detached rather than waited on: it has to outlive this call and live exactly as
/// long as the session that follows it, which is the caller's business and not
/// this function's.
#[derive(Debug)]
pub(crate) struct Forward {
    pid: u32,
    /// Registered for the interrupt handler, and held for exactly as long as the
    /// forward is: dropping it frees the slot so the handler cannot signal a pid
    /// that has since been recycled. `None` only if every slot was full, which
    /// costs this forward its interrupt-time cleanup and nothing else.
    ///
    /// Without it a `dl` that is Ctrl-C'd leaves the forward running: the child is
    /// `setsid`'d, so a terminal's signal to the foreground process group never
    /// reaches it, and `dl`'s handler `_exit`s without unwinding, so [`stop`] --
    /// which only runs on the ordinary return path -- never happens.
    ///
    /// [`stop`]: Forward::stop
    _cleanup: Option<interrupt::Registration>,
}

impl Forward {
    /// Take the forward down.
    ///
    /// `SIGTERM` and not `SIGKILL`, and it is tidiness rather than correctness,
    /// which is why nothing here waits to see it happen.
    ///
    /// Not for the reason this comment used to give. OpenSSH removes a remote
    /// listen path on a clean exit, but the path here is not OpenSSH's: the remote
    /// end is devpod's own Go server, and devlaunch#549 measured that the socket
    /// survives the forward either way. What the container is left holding is a
    /// socket file with nobody behind it -- which is why the hook cannot treat
    /// `[ -S ]` as proof anyone is listening, and does not.
    pub(crate) fn stop(self, runner: &dyn Runner) {
        let _ = kill::signal(runner, Signal::Terminate, &NonEmpty::one(self.pid));
    }
}

/// Open the forward, or say why there is none.
pub(crate) fn start_forward(
    runner: &dyn Runner,
    config: &Path,
    workspace_id: &str,
    reporting: &Reporting,
) -> Result<Forward, String> {
    let argv = reporting.forward_argv(config, workspace_id);
    let (program, args) = argv.split_first().expect("the forward names ssh");
    match runner.detach(&Invocation::new(program.clone()).with_args(args.iter().cloned())) {
        DetachOutcome::Started { pid } => Ok(Forward {
            pid,
            _cleanup: interrupt::register_pid(i32::try_from(pid).unwrap_or(0)),
        }),
        DetachOutcome::ProgramNotFound => Err("no ssh on this host".to_owned()),
        DetachOutcome::NotStarted(failure) => Err(format!("{failure:?}")),
    }
}

// ===========================================================================
// where a pane herdr just made should put its shell
// ===========================================================================

/// Where the shell in a pane herdr has just created belongs.
///
/// Two arms and no third, because there is no useful way to half-answer this: a
/// pane either opens inside a workspace or opens the shell it would have opened
/// anyway. Every refusal below -- no manager, no tab, a socket that will not
/// answer, an answer that is not JSON -- is [`PaneDestination::HostShell`], which
/// is what makes this safe to put in front of every pane on the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
// binary surface -- not part of the frozen wf API (#251 §7)
pub enum PaneDestination {
    /// The tab holds a live devlaunch session, in this workspace.
    Workspace {
        workspace_id: String,
        /// The Claude profile the session beside this pane was launched with, when
        /// its `dl` argv names one.
        ///
        /// Read rather than remembered, which is what lets it exist at all: a
        /// profile is named per launch and deliberately **not** stored with the
        /// workspace, so there is nothing on disk for a new pane to inherit. The
        /// live sibling process is the only record, and herdr is already being
        /// asked what that pane is running.
        claude_profile: Option<String>,
    },
    /// Open an ordinary shell on this host.
    HostShell,
}

/// Where a pane herdr just spawned a shell into should actually put it.
///
/// The whole of the feature, from this process's own environment to one answer.
/// [`launch::HERDR_TAB_VAR`] says which tab the new pane is in; herdr says which
/// panes that tab holds and what each of them is running; and dl reads its own
/// transport out of those argvs.
///
/// Both variables are read from the constants that already declare them -- the tab
/// from `flows::launch`, where the tab rename put it, and the binary from
/// `clients::herdr` beside herdr's other exports. Neither is spelled again here.
///
/// Nothing is remembered between launches and nothing is written down, which is
/// the property the whole design turns on. A tab whose session has exited answers
/// [`PaneDestination::HostShell`] the moment it has, and a tab reused for
/// something else was never claimed in the first place -- where a note kept
/// against a tab id would go on naming a workspace nobody in that tab is in.
pub fn pane_destination(runner: &dyn Runner) -> PaneDestination {
    let tab_id = crate::osext::env_str(launch::HERDR_TAB_VAR);
    let binary = crate::osext::env_str(herdr::BIN_VAR);
    destination_for(
        runner,
        PaneEnv {
            tab_id: tab_id.as_deref(),
            binary: binary.as_deref(),
        },
    )
}

/// What herdr exported into this pane, as the pane shell reads it.
///
/// A struct rather than two `Option<&str>` parameters, which is what this was:
/// the two are the same type, adjacent, and swapping them compiles clean and
/// answers [`PaneDestination::HostShell`] for every pane on the machine -- a
/// silent, total loss of the feature that nothing would catch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PaneEnv<'a> {
    /// [`launch::HERDR_TAB_VAR`]: which tab, and the whole of the detection.
    tab_id: Option<&'a str>,
    /// [`herdr::BIN_VAR`]: which herdr, or none and a `PATH` lookup.
    binary: Option<&'a str>,
}

/// [`pane_destination`], against a stated environment rather than this process's.
fn destination_for(runner: &dyn Runner, host: PaneEnv<'_>) -> PaneDestination {
    let Some(tab_id) = host.tab_id.filter(|id| !id.is_empty()) else {
        // Not in a manager's pane at all, which is the ordinary case for every
        // shell on a machine that has one, and the case this must be cheapest in:
        // no herdr is spawned and nothing is asked.
        return PaneDestination::HostShell;
    };
    let binary = host
        .binary
        .filter(|path| !path.is_empty())
        .unwrap_or(launch::HERDR_BIN_FALLBACK);
    // One budget for the whole question. Spending it per call instead let a tab
    // of N panes cost `ANSWER_WITHIN * (N + 1)`, and nothing bounds N.
    let mut budget = Budget::of(herdr::ANSWER_WITHIN);
    let Some(panes) = ask(runner, binary, &herdr::pane_list_argv(), &mut budget)
        .as_deref()
        .and_then(herdr::panes_in)
    else {
        return PaneDestination::HostShell;
    };
    for pane in in_tab(tab_id, &panes) {
        let Some(info) = ask(
            runner,
            binary,
            &herdr::process_info_argv(&pane.pane_id),
            &mut budget,
        )
        .as_deref()
        .and_then(herdr::process_info_in) else {
            continue;
        };
        if let Some(workspace_id) = workspace_among(&info) {
            return PaneDestination::Workspace {
                workspace_id,
                // Asked of the *same* pane, so one pane answers both questions and a
                // tab holding two sessions cannot pair one session's workspace with
                // another's account.
                claude_profile: profile_among(&info).map(str::to_owned),
            };
        }
    }
    PaneDestination::HostShell
}

/// Tell the manager which Claude account this pane's session is running as.
///
/// Display only: it sets a metadata token herdr renders as `$profile`, and touches no
/// lifecycle state, so it cannot disturb the idle/working/blocked the hook reports.
///
/// **Reported and then tolerated**, like everything else in this flow. A herdr that is
/// gone, too old for `report-metadata`, or simply slow costs a label and never a
/// session, so the outcome is deliberately dropped.
///
/// Called with `None` as well as with a name, because a pane is reused and a stale
/// label is worse than none: see [`herdr::profile_metadata_argv`].
pub(crate) fn report_profile(
    runner: &dyn Runner,
    reporting: &herdr::Reporting,
    profile: Option<&str>,
) {
    let mut budget = Budget::of(herdr::ANSWER_WITHIN);
    let program = reporting.host_binary().display().to_string();
    let _ = ask(
        runner,
        &program,
        &herdr::profile_metadata_argv(reporting.pane_id(), profile),
        &mut budget,
    );
}

/// What is left of the time the whole question may take.
///
/// A budget rather than a deadline, because the [`Runner`] seam takes a duration
/// per call and knows nothing about wall-clock instants -- which is also what
/// makes this testable without a clock: a test reads the durations the calls were
/// given and adds them up.
struct Budget {
    left: Duration,
}

impl Budget {
    fn of(total: Duration) -> Self {
        Self { left: total }
    }

    /// What the next call may take, or `None` when the budget is gone.
    ///
    /// `None` stops the questioning rather than asking with no bound: a run that
    /// has already spent the whole budget is a herdr that is not answering, and
    /// the caller reads that as no workspace, which is a shell.
    fn next(&self) -> Option<Duration> {
        (!self.left.is_zero()).then_some(self.left)
    }

    /// Charge what a call actually cost.
    ///
    /// Saturating, so a call that overran leaves nothing rather than wrapping.
    fn spend(&mut self, taken: Duration) {
        self.left = self.left.saturating_sub(taken);
    }
}

/// Ask herdr one question, and treat every way of not answering alike.
///
/// stdout only, and only when the call succeeded: herdr writes its refusals as
/// well-formed JSON on a non-zero exit, so a caller that read those would be
/// parsing an envelope with no `result` in it to reach the same `None` this
/// returns for free.
fn ask(runner: &dyn Runner, program: &str, args: &[String], budget: &mut Budget) -> Option<String> {
    let spec = SpawnSpec::new(Invocation::new(program.to_owned()).with_args(args.iter().cloned()))
        .with_timeout(budget.next()?);
    let started = Instant::now();
    let outcome = runner.capture(&spec);
    budget.spend(started.elapsed());
    match outcome {
        Outcome::Ran { exit, io } if exit.is_success() => Some(io.stdout),
        // A refusal, a herdr that is not installed, a timeout, an OS that would
        // not start it: four facts, one consequence.
        _ => None,
    }
}

/// The panes of one tab, the focused one first.
///
/// Focused first so that a tab holding two devlaunch sessions answers with the
/// one the person was last looking at, which is the only reading of "the same
/// container" that is not arbitrary. Two sessions in one tab is unusual; a stable
/// answer for it is cheap.
fn in_tab<'a>(tab_id: &str, panes: &'a [herdr::PaneInfo]) -> Vec<&'a herdr::PaneInfo> {
    let mut mine: Vec<&herdr::PaneInfo> =
        panes.iter().filter(|pane| pane.tab_id == tab_id).collect();
    mine.sort_by_key(|pane| !pane.focused);
    mine
}

/// The workspace any of one pane's foreground processes names.
///
/// The whole foreground list and not just its head, because a dl session is a
/// chain: the pane's own foreground process is `dl`, which names a *spec* and not
/// an id, and the transport that names the id is its child. herdr publishes the
/// chain, which is the same reading it does to identify an agent.
fn workspace_among(info: &herdr::PaneProcessInfo) -> Option<String> {
    info.foreground_processes
        .iter()
        .filter_map(|process| process.argv.as_deref())
        .find_map(workspace_named_by)
        .map(str::to_owned)
}

/// The workspace an argv names, when the argv is one of dl's two transports.
///
/// **This reads dl's own writing**, which is what makes it a reading and not a
/// guess: both argvs are built in this crate.
///
/// | transport | shape | built by |
/// |---|---|---|
/// | devpod | `devpod ssh <id> ...` | `flows::launch`'s `devpod_session` |
/// | OpenSSH | `ssh ... <id>.devpod <payload>` | [`ssh::command_args`] |
///
/// It is still a second copy of a fact, and CLAUDE.md's standing rule asks for a
/// test beside it that diffs the two. That is
/// `both_transports_name_the_workspace_they_were_built_for`: it runs both real
/// builders and asserts this recovers the id each was given, so a **structural**
/// drift fails it -- an alias that gains a `user@`, a `devpod ssh` that grows a
/// flag before the id, a payload that becomes two arguments. Checked by making
/// each of those changes and watching it fail.
///
/// What it cannot catch is a *coordinated* rename, because both halves read
/// [`ssh::HOST_SUFFIX`] and a change to that constant moves them together. That
/// is what the tests spelling `.devpod` out as a literal are for, and it is the
/// right division: a shared constant is one copy of the fact, and the second copy
/// is the shape around it.
///
/// The program is compared by its last path component, so a `/usr/bin/ssh` and a
/// bare `ssh` answer alike, exactly as [`herdr::agent_in`] does it.
/// The Claude profile a pane's processes name, if one of them was given one.
///
/// Asked only of a pane that has already named a workspace, so the question is
/// "which account is *this devlaunch session* running as" rather than a scan of the
/// machine. That coupling is the guard: an unrelated pane is never consulted.
///
/// **Elements are compared whole, and the scan stops at the first bare `--`.** Both
/// matter for one case. `aid <ws> add --claude-profile support to the docs` becomes a
/// `dl` line whose prompt is a single argument containing that text, and the ssh
/// transport carries the same string as one payload argument; neither is an argv
/// *element* equal to `--claude-profile`, so neither is read as a flag. The `--` stop
/// says the same thing a second way, and cheaply.
///
/// Deliberately not gated on the program name. `dl` on a host may be `dl`, `dl-next`
/// or an absolute path, and matching that family by prefix is exactly the fuzzy test
/// that made [`ssh_host`] necessary. The pane-level coupling above is the stronger
/// guard and needs no such guess.
fn profile_among(info: &herdr::PaneProcessInfo) -> Option<&str> {
    info.foreground_processes
        .iter()
        .filter_map(|process| process.argv.as_deref())
        .find_map(profile_named_by)
}

/// The profile one argv names, in either of clap's two spellings.
fn profile_named_by(argv: &[String]) -> Option<&str> {
    let mut rest = argv.iter();
    while let Some(argument) = rest.next() {
        if argument == "--" {
            // Everything after this belongs to the command the workspace runs, and a
            // prompt is allowed to contain anything at all.
            return None;
        }
        if let Some(value) = argument.strip_prefix("--claude-profile=") {
            return (!value.is_empty()).then_some(value);
        }
        if argument == "--claude-profile" {
            return rest.next().map(String::as_str).filter(|v| !v.is_empty());
        }
    }
    None
}

fn workspace_named_by(argv: &[String]) -> Option<&str> {
    let program = argv.first()?.rsplit('/').next()?;
    if program == devpod::PROGRAM {
        // `devpod up`, `devpod status` and the rest name a workspace too, and not
        // one of them is a session anybody can join. Only `ssh` counts.
        let mut rest = argv[1..].iter();
        if rest.next().map(String::as_str) != Some("ssh") {
            return None;
        }
        return rest.next().map(String::as_str).filter(|id| plausible(id));
    }
    if program == ssh::PROGRAM {
        return ssh_host(&argv[1..]).and_then(alias_workspace);
    }
    None
}

/// The host an `ssh` argv connects to: its first positional argument.
///
/// A value-taking option's value is skipped **with** the option, which is the
/// whole of what this adds over scanning for the suffix. The scan that came first
/// guarded on the characters in an argument -- no `/`, no `=` -- and a `-F` config
/// path with neither passed it: a sibling pane running
/// `ssh -F myconf.devpod somehost`, which has nothing to do with devlaunch, made
/// the pane shell claim the workspace `myconf` and then close, reproduced against
/// live herdr 0.8.2. An argument's meaning is its position, not its spelling.
///
/// The list is OpenSSH's, not dl's. dl emits `-F` and `-o` and nothing else, so a
/// list of two would serve every argv this crate builds -- and this is read
/// against argvs the *user* wrote, in a sibling pane, where any of them can
/// appear. `-o` is on it twice over: `ssh -o SendEnv=X host` and
/// `ssh -oSendEnv=X host` are both legal, and the attached form needs no skip
/// because it is not a bare word.
fn ssh_host(args: &[String]) -> Option<&str> {
    const TAKES_A_VALUE: &[&str] = &[
        "-B", "-b", "-c", "-D", "-E", "-e", "-F", "-I", "-i", "-J", "-L", "-l", "-m", "-O", "-o",
        "-p", "-Q", "-R", "-S", "-W", "-w",
    ];
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if TAKES_A_VALUE.contains(&arg.as_str()) {
            // The value is the next word, whatever it looks like.
            rest.next();
        } else if !arg.starts_with('-') {
            return Some(arg);
        }
        // Anything else starting with `-` is a bundle of flags taking no value
        // (`-t`, `-tt`, `-nNT`) or an option with its value attached (`-oX=y`).
        // Neither is a host and neither eats the word after it.
    }
    None
}

/// The workspace a host name is the alias for, or `None` if it is somebody's own
/// host that happens to end the same way.
///
/// [`ssh_host`] has already established that this argument is the host, so what is
/// left is only whether devpod published it. A `user@` prefix is refused rather
/// than stripped: devpod publishes a bare alias and dl passes it through
/// unchanged, so a host carrying one is not an alias dl wrote, and guessing which
/// half of it is the workspace is how a launch gets handed a name nobody has.
fn alias_workspace(host: &str) -> Option<&str> {
    if host.contains('@') {
        return None;
    }
    host.strip_suffix(ssh::HOST_SUFFIX)
        .filter(|id| plausible(id))
}

/// Whether a word taken from an argv can be a workspace id at all.
///
/// Empty is what a bare `.devpod` or a `devpod ssh` with no workspace leaves, and
/// a leading `-` is a flag that landed where the id should be. Neither is a
/// workspace, and handing either to a launch would be handing it a guess.
fn plausible(id: &str) -> bool {
    !id.is_empty() && !id.starts_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::herdr::HostEnv;
    use crate::testing::ScriptedRunner;
    use devlaunch_test_support::{Response, Unscripted};

    fn reporting() -> Reporting {
        let binary = std::env::current_exe().expect("this test binary is a readable file");
        Reporting::resolve(&HostEnv {
            enabled: Some("1".to_owned()),
            in_pane: Some("1".to_owned()),
            pane_id: Some("w1:p3".to_owned()),
            socket: Some("/run/user/1000/herdr.sock".to_owned()),
            binary: Some(binary.display().to_string()),
        })
        .expect("a host in a manager's pane")
    }

    /// Every launch asks the container, because only the container knows.
    ///
    /// A marker under dl's cache directory used to answer this for free, keyed on
    /// the workspace id and the host binary's size. Both survive the container:
    /// `dl <ws> recreate` and `dl <ws> reset` destroy it, keep the id, and end in
    /// a session, so the second launch here returned `Ready` after asking nothing
    /// at all -- and dl then announced it was reporting agents to a pane over a
    /// container that had no herdr in it, forever, with the failure visible
    /// nowhere.
    #[test]
    fn a_second_launch_asks_the_container_again() {
        let reporting = reporting();
        let config = Path::new("/tmp/ssh-config");
        let runner = ScriptedRunner::new();
        // The probe, answering that this container is prepared already.
        runner.script(["ssh"], Response::ok());

        for _ in 0..2 {
            assert_eq!(
                prepare(&runner, config, "myws", &reporting),
                Prepared::Ready
            );
        }
        assert_eq!(
            runner.calls_to("ssh").len(),
            2,
            "the second launch believed a host-side record over the container: {:?}",
            runner.calls_to("ssh")
        );
    }

    /// The bug this shape exists to prevent, as a test.
    ///
    /// The probe is a `test`, so an unprepared container answers non-zero and says
    /// nothing on either stream. Reading that as a refusal meant every workspace
    /// that had not been prepared reported "could not probe the container" with
    /// nothing after the colon and was never prepared -- the one path this whole
    /// flow exists for.
    #[test]
    fn an_unprepared_container_is_prepared_rather_than_refused() {
        let reporting = reporting();
        let len = std::fs::metadata(reporting.host_binary())
            .expect("readable")
            .len();
        let config = Path::new("/tmp/ssh-config");
        let runner = ScriptedRunner::new();
        // The probe: ran, said no, said nothing.
        runner.script(
            [
                "ssh",
                "-F",
                "/tmp/ssh-config",
                "myws.devpod",
                &herdr::probe_command(len, &reporting.container_socket()),
            ],
            Response::exited(1),
        );
        // The lend and the install, in that order, both fine.
        runner.script(["ssh"], Response::ok());

        assert_eq!(
            prepare(&runner, config, "myws", &reporting),
            Prepared::Ready
        );
        let commands: Vec<String> = runner
            .calls_to("ssh")
            .into_iter()
            .filter_map(|call| call.args().last().map(|arg| arg.to_owned()))
            .collect();
        assert_eq!(commands.len(), 3, "{commands:?}");
        assert!(
            commands[1].contains("mv /usr/local/bin/"),
            "the second trip is not the lend: {}",
            commands[1]
        );
        assert!(
            commands[2].contains("managed-settings.json"),
            "the third trip is not the install: {}",
            commands[2]
        );
    }

    /// A container that answers the probe cleanly is prepared already, and must
    /// cost that one trip rather than a fresh 17MB lend.
    #[test]
    fn a_container_that_is_already_prepared_is_not_lent_to_again() {
        let reporting = reporting();
        let runner = ScriptedRunner::new();
        runner.script(["ssh"], Response::ok());

        assert_eq!(
            prepare(&runner, Path::new("/tmp/ssh-config"), "myws", &reporting),
            Prepared::Ready
        );
        assert_eq!(runner.calls_to("ssh").len(), 1, "the probe was not enough");
    }

    /// An unreachable workspace is not an unprepared one: ssh's own 255 must not
    /// be read as a container saying no, or a launch would lend into a workspace
    /// that is not answering.
    #[test]
    fn an_unreachable_workspace_is_refused_rather_than_lent_to() {
        let runner = ScriptedRunner::new();
        runner.script(["ssh"], Response::failed(255, "Connection refused"));

        let Prepared::Refused { reason } =
            prepare(&runner, Path::new("/tmp/ssh-config"), "myws", &reporting())
        else {
            panic!("an unreachable workspace cannot be prepared");
        };
        assert!(reason.contains("Connection refused"), "{reason}");
        assert_eq!(
            runner.calls_to("ssh").len(),
            1,
            "dl kept going after the workspace stopped answering"
        );
    }

    /// The same bug as above, with the trigger that is common rather than rare.
    ///
    /// A failed probe that *says* something is still a probe that answered. The
    /// first fix for this read an empty pair of streams as the answer, so it
    /// covered a silent `test` and nothing else -- and a container is rarely
    /// silent. Debian's bash is built with `SSH_SOURCE_BASHRC`, so
    /// `ssh host <cmd>` runs the remote `~/.bashrc`: a pixi or nvm init line, an
    /// `/etc/bash.bashrc` message or a locale warning arrives on stderr beside
    /// the exit status. Reading that as "the trip never happened" refused every
    /// such workspace on every launch, forever, and never lent anything.
    #[test]
    fn a_probe_that_answers_no_out_loud_is_still_an_answer() {
        let reporting = reporting();
        let len = std::fs::metadata(reporting.host_binary())
            .expect("readable")
            .len();
        let config = Path::new("/tmp/ssh-config");
        let runner = ScriptedRunner::new();
        runner.script(
            [
                "ssh",
                "-F",
                "/tmp/ssh-config",
                "myws.devpod",
                &herdr::probe_command(len, &reporting.container_socket()),
            ],
            Response::failed(1, "bash: warning: setlocale: LC_ALL: cannot change locale"),
        );
        // The lend and the install, in that order, both fine.
        runner.script(["ssh"], Response::ok());

        assert_eq!(
            prepare(&runner, config, "myws", &reporting),
            Prepared::Ready,
            "a container that warned about its locale was read as unreachable"
        );
        assert_eq!(
            runner.calls_to("ssh").len(),
            3,
            "the probe answered no and nothing was lent: {:?}",
            runner.calls_to("ssh")
        );
    }

    /// A forward that reported a pid and bound nothing is not a prepared
    /// workspace, and lending into it would buy silence.
    ///
    /// `detach` gives the forward `/dev/null` for stderr and never waits for it, so
    /// `ExitOnForwardFailure=yes` takes the connection down and tells nobody. A
    /// container whose user cannot create the listen path -- a root-owned `/tmp`, a
    /// stale root-owned socket -- therefore used to be announced as "reporting
    /// agents in this workspace" and report nothing. The probe is where it becomes
    /// visible, and its answer is a refusal because no lend repairs it.
    #[test]
    fn a_forward_that_bound_nothing_is_refused_rather_than_lent_to() {
        let reporting = reporting();
        let runner = ScriptedRunner::new();
        runner.script(["ssh"], Response::exited(herdr::PROBE_NO_SOCKET));

        let Prepared::Refused { reason } =
            prepare(&runner, Path::new("/tmp/ssh-config"), "myws", &reporting)
        else {
            panic!("a workspace the socket never reached cannot report an agent");
        };
        assert!(
            reason.contains(&reporting.container_socket()),
            "the reason does not say which socket never arrived: {reason}"
        );
        assert_eq!(
            runner.calls_to("ssh").len(),
            1,
            "17MB was lent into a workspace that has no socket to report over"
        );
    }

    /// The refusal for a foreign settings file says what it is, because "could not
    /// install the agent hook" would send a reader looking for a broken install.
    #[test]
    fn a_workspace_with_its_own_managed_settings_is_left_alone() {
        let reporting = reporting();
        let len = std::fs::metadata(reporting.host_binary())
            .expect("readable")
            .len();
        let config = Path::new("/tmp/ssh-config");
        let runner = ScriptedRunner::new();
        runner.script(
            [
                "ssh",
                "-F",
                "/tmp/ssh-config",
                "myws.devpod",
                &herdr::probe_command(len, &reporting.container_socket()),
            ],
            Response::exited(1),
        );
        runner.script(
            [
                "ssh",
                "-F",
                "/tmp/ssh-config",
                "myws.devpod",
                herdr::lend_command(),
            ],
            Response::ok(),
        );
        runner.script(
            [
                "ssh",
                "-F",
                "/tmp/ssh-config",
                "myws.devpod",
                &herdr::install_command(),
            ],
            Response::exited(herdr::INSTALL_FOREIGN_SETTINGS),
        );

        let Prepared::Refused { reason } = prepare(&runner, config, "myws", &reporting) else {
            panic!("dl overwrote settings it did not write");
        };
        assert!(
            reason.contains(herdr::CONTAINER_SETTINGS) && reason.contains("did not write"),
            "the refusal reads as a broken install rather than a file left alone: {reason}"
        );
    }

    /// The refusal carries the container's own last word, and not the `sudo`
    /// hostname warning that rides along on every trip.
    #[test]
    fn a_refusal_names_what_the_container_said() {
        let runner = ScriptedRunner::new();
        runner.script(
            ["ssh"],
            Response::failed(
                1,
                "sudo: unable to resolve host myws\nsudo: a password is required",
            ),
        );

        let Prepared::Refused { reason } =
            prepare(&runner, Path::new("/tmp/ssh-config"), "myws", &reporting())
        else {
            panic!("a container that refuses sudo cannot be prepared");
        };
        assert!(
            reason.contains("a password is required"),
            "the reason lost the container's words: {reason}"
        );
        assert!(
            !reason.contains("unable to resolve host"),
            "the reason kept sudo's hostname noise: {reason}"
        );
    }

    // =======================================================================
    // where a pane herdr just made should put its shell
    // =======================================================================

    /// The herdr a pane's environment names, in the tests below.
    const LENT_HERDR: &str = "/opt/herdr/bin/herdr";

    /// A `pane list` answer, in herdr's own envelope, listing these panes.
    ///
    /// Written out in full rather than built from [`herdr::PaneInfo`], because the
    /// fifteen fields this type does not model are exactly what the parse has to
    /// keep ignoring: a fixture derived from the type could not catch a parse that
    /// started depending on them.
    fn pane_list(panes: &[(&str, &str, bool)]) -> String {
        let rows: Vec<String> = panes
            .iter()
            .map(|(pane_id, tab_id, focused)| {
                format!(
                    r#"{{"pane_id":"{pane_id}","terminal_id":"term-1","workspace_id":"w1","tab_id":"{tab_id}","focused":{focused},"agent_status":"none","revision":7,"label":null,"cwd":"/home/dev","terminal_title":"devlaunch@main"}}"#
                )
            })
            .collect();
        format!(
            r#"{{"id":"cli:pane:list","result":{{"type":"pane_list","panes":[{}]}}}}"#,
            rows.join(",")
        )
    }

    /// A `pane process-info` answer whose foreground chain is these argvs.
    fn process_info(chain: &[&[&str]]) -> String {
        let rows: Vec<String> = chain
            .iter()
            .enumerate()
            .map(|(index, argv)| {
                let words: Vec<String> = argv.iter().map(|word| format!("\"{word}\"")).collect();
                format!(
                    r#"{{"pid":{},"name":"{}","argv0":"{}","argv":[{}],"cmdline":"{}","cwd":"/home/dev"}}"#,
                    100 + index,
                    argv.first().copied().unwrap_or_default(),
                    argv.first().copied().unwrap_or_default(),
                    words.join(","),
                    argv.join(" ")
                )
            })
            .collect();
        format!(
            r#"{{"id":"cli:pane:process_info","result":{{"type":"pane_process_info","process_info":{{"pane_id":"w1:p1","shell_pid":99,"tty":"/dev/pts/3","foreground_process_group_id":100,"foreground_processes":[{}]}}}}}}"#,
            rows.join(",")
        )
    }

    /// A pane herdr spawned in the tab named, with a herdr to ask.
    fn in_pane_of(tab_id: &str) -> PaneEnv<'_> {
        PaneEnv {
            tab_id: Some(tab_id),
            binary: Some(LENT_HERDR),
        }
    }

    /// [`destination_for`], named for what it answers.
    fn destination(runner: &ScriptedRunner, host: PaneEnv<'_>) -> PaneDestination {
        destination_for(runner, host)
    }

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn the_devpod_transport_names_its_workspace() {
        assert_eq!(
            workspace_named_by(&argv(&["devpod", "ssh", "devlaunch-main-3j1t"])),
            Some("devlaunch-main-3j1t")
        );
    }

    #[test]
    fn the_openssh_transport_names_its_workspace() {
        assert_eq!(
            workspace_named_by(&argv(&[
                "ssh",
                "-F",
                "/home/dev/.ssh/devpod/config",
                "-t",
                "devlaunch-main-3j1t.devpod",
                "bash -lc 'claude'",
            ])),
            Some("devlaunch-main-3j1t")
        );
    }

    /// The two arguments that look most like a published alias and are not one. A
    /// config path can end in the suffix and a `ControlPath` can contain it;
    /// neither is a host, and reading either would hand a launch a directory.
    #[test]
    fn neither_a_path_nor_an_option_value_is_read_as_the_alias() {
        assert_eq!(
            workspace_named_by(&argv(&[
                "ssh",
                "-F",
                "/home/dev/.ssh/mine.devpod",
                "-o",
                "ControlPath=/run/user/1000/dl/ws.devpod",
                "-t",
                "real-ws-3j1t.devpod",
                "bash -lc 'true'",
            ])),
            Some("real-ws-3j1t")
        );
    }

    /// The defect: the guards above are about the *characters* in an argument, so
    /// a `-F` config path with no `/` in it passed all of them and was read as the
    /// host. Reproduced against live herdr 0.8.2 -- a sibling pane running
    /// `ssh -F myconf.devpod -o ProxyCommand=... somehost`, which has nothing to do
    /// with devlaunch, made the pane shell claim the workspace `myconf`.
    ///
    /// An argument's meaning comes from its position, not its spelling: the value
    /// of a value-taking option is never a host.
    #[test]
    fn an_option_value_that_ends_in_the_suffix_is_not_the_host() {
        assert_eq!(
            workspace_named_by(&argv(&[
                "ssh",
                "-F",
                "myconf.devpod",
                "-o",
                "ProxyCommand=sleep 600",
                "somehost",
            ])),
            None
        );
    }

    /// And where both are present, the host is the one that is a host.
    #[test]
    fn a_config_path_does_not_win_over_the_alias_beside_it() {
        assert_eq!(
            workspace_named_by(&argv(&[
                "ssh",
                "-F",
                "myconf.devpod",
                "-t",
                "real-ws-3j1t.devpod",
                "bash -lc true",
            ])),
            Some("real-ws-3j1t")
        );
    }

    /// Only `devpod ssh` is a session. The other subcommands name a workspace
    /// just as plainly and not one of them is something a second pane can join.
    #[test]
    fn a_devpod_that_is_not_ssh_names_nothing() {
        assert_eq!(
            workspace_named_by(&argv(&["devpod", "up", "devlaunch-main-3j1t"])),
            None
        );
        assert_eq!(
            workspace_named_by(&argv(&[
                "devpod",
                "status",
                "devlaunch-main-3j1t",
                "--output",
                "json"
            ])),
            None
        );
    }

    /// The head of the chain. `dl` names a *spec*, which is not an id, and the
    /// pane shell's own process names nothing at all -- so the reading has to walk
    /// past both to the transport underneath.
    #[test]
    fn dl_itself_names_no_workspace() {
        assert_eq!(
            workspace_named_by(&argv(&["dl", "blooop/devlaunch@main", "--", "claude"])),
            None
        );
        assert_eq!(workspace_named_by(&argv(&["dl", "--herdr-shell"])), None);
    }

    #[test]
    fn a_program_reached_by_path_answers_as_its_last_component_does() {
        assert_eq!(
            workspace_named_by(&argv(&["/usr/bin/ssh", "-t", "ws-3j1t.devpod", "true"])),
            Some("ws-3j1t")
        );
        assert_eq!(
            workspace_named_by(&argv(&["/usr/local/bin/devpod", "ssh", "ws-3j1t"])),
            Some("ws-3j1t")
        );
    }

    #[test]
    fn an_empty_or_flag_shaped_id_is_not_a_workspace() {
        assert_eq!(workspace_named_by(&argv(&["ssh", "-t", ".devpod"])), None);
        assert_eq!(workspace_named_by(&argv(&["devpod", "ssh"])), None);
        assert_eq!(
            workspace_named_by(&argv(&["devpod", "ssh", "--help"])),
            None
        );
        assert_eq!(workspace_named_by(&[]), None);
    }

    /// The diff test CLAUDE.md's standing rule asks for. [`workspace_named_by`] is
    /// a second copy of a shape two argv builders own, so both builders are run
    /// and their output is fed to the reader. Break either builder and this fails;
    /// without it the pane shell would quietly stop finding workspaces and no test
    /// in the tree would notice.
    /// One foreground process running `words`, beside the existing `argv` helper.
    fn process(words: &[&str]) -> herdr::ForegroundProcess {
        herdr::ForegroundProcess {
            argv: Some(argv(words)),
        }
    }

    #[test]
    fn the_profile_label_is_set_and_cleared_in_herdrs_own_words() {
        // Verified against herdr 0.8.2's `pane report-metadata --help`, not guessed:
        // `[OPTIONS] --source <ID> <PANE_ID>`, where a metadata token is set with
        // `--token` as `NAME=VALUE` and cleared with `--clear-token`. The pane id
        // goes first, as the hook gives it to `report-agent`, which is the
        // invocation measured against a live herdr.
        assert_eq!(
            herdr::profile_metadata_argv("pane-7", Some("work")),
            argv(&[
                "pane",
                "report-metadata",
                "pane-7",
                "--source",
                "devlaunch:claude",
                "--token",
                "profile=work",
            ])
        );
        // No profile clears the label rather than leaving one. A pane is reused, and a
        // sidebar claiming an account the running session is not using is the
        // mislabelling `--claude-profiles` exists to prevent, moved somewhere more
        // visible.
        assert_eq!(
            herdr::profile_metadata_argv("pane-7", None),
            argv(&[
                "pane",
                "report-metadata",
                "pane-7",
                "--source",
                "devlaunch:claude",
                "--clear-token",
                "profile",
            ])
        );
    }

    #[test]
    fn the_hook_reports_under_the_source_this_module_names() {
        // Two copies of one string: this module's argv builders and the hook, which
        // spells it as a shell literal because it is a shell script. CLAUDE.md's
        // standing rule asks for a test beside the second copy, and this is it.
        assert!(
            herdr::HOOK.contains(herdr::REPORT_SOURCE),
            "the hook no longer reports under {}",
            herdr::REPORT_SOURCE
        );
    }

    #[test]
    fn a_profile_label_costs_a_label_and_never_a_session() {
        // Reported and then tolerated, like everything else in this flow. A herdr that
        // is not installed, is too old for `report-metadata`, or simply refuses must
        // cost the label and never the session. The signature is most of the guarantee
        // -- there is no outcome for a caller to branch on -- and this is the other
        // half: the unscripted and missing cases both return rather than panicking.
        let reporting = reporting();
        report_profile(&ScriptedRunner::new(), &reporting, Some("work"));
        report_profile(&ScriptedRunner::new(), &reporting, None);
        let absent =
            ScriptedRunner::new().with_missing(reporting.host_binary().display().to_string());
        report_profile(&absent, &reporting, Some("work"));
    }

    #[test]
    fn a_profile_is_read_out_of_the_line_that_named_it() {
        // Both of clap's spellings, since `dl` accepts either and a recalled line may
        // hold either.
        assert_eq!(
            profile_named_by(&argv(&["dl", "ws", "--claude-profile", "work"])),
            Some("work")
        );
        assert_eq!(
            profile_named_by(&argv(&["dl", "--claude-profile=work", "ws"])),
            Some("work")
        );
        // A line that named none, which is almost every line.
        assert_eq!(profile_named_by(&argv(&["dl", "ws"])), None);
        // A flag with nothing after it is clap's error to report, not a profile.
        assert_eq!(
            profile_named_by(&argv(&["dl", "ws", "--claude-profile"])),
            None
        );
        assert_eq!(
            profile_named_by(&argv(&["dl", "--claude-profile=", "ws"])),
            None
        );
    }

    #[test]
    fn a_prompt_that_mentions_the_flag_is_not_a_profile() {
        // The case that decides how this is written. `aid <ws> add --claude-profile
        // support to the docs` becomes a dl line whose prompt is one argument holding
        // that text, and the ssh transport carries the same string as one payload
        // argument. Elements are compared whole, so neither is a flag; the `--` stop
        // says it a second way.
        assert_eq!(
            profile_named_by(&argv(&[
                "dl",
                "ws",
                "--",
                "claude",
                "add --claude-profile support to the docs",
            ])),
            None
        );
        assert_eq!(
            profile_named_by(&argv(&[
                "ssh",
                "ws.devpod",
                "claude 'add --claude-profile support'",
            ])),
            None
        );
        // And a real flag *before* the `--` is still read, so the stop is a stop and
        // not a refusal.
        assert_eq!(
            profile_named_by(&argv(&[
                "dl",
                "--claude-profile",
                "work",
                "ws",
                "--",
                "claude",
                "--claude-profile decoy",
            ])),
            Some("work")
        );
    }

    #[test]
    fn the_profile_comes_from_the_pane_that_named_the_workspace() {
        // One pane answers both questions, so a tab holding two sessions cannot pair
        // one session's workspace with another's account. The workspace comes off the
        // transport process and the profile off `dl`, which are two processes in the
        // same pane.
        let info = herdr::PaneProcessInfo {
            foreground_processes: vec![
                process(&["dl", "ws", "--claude-profile", "work"]),
                process(&["ssh", "-F", "cfg", "ws-3j1t.devpod", "payload"]),
            ],
        };
        assert_eq!(profile_among(&info), Some("work"));
    }

    #[test]
    fn a_pane_with_no_profile_on_its_line_reports_none() {
        let info = herdr::PaneProcessInfo {
            foreground_processes: vec![process(&["dl", "ws"]), process(&["ssh", "ws.devpod"])],
        };
        assert_eq!(profile_among(&info), None);
    }

    #[test]
    fn both_transports_name_the_workspace_they_were_built_for() {
        let workspace_id = "devlaunch-main-3j1t";

        let openssh = ssh::command_args(
            Path::new("/home/dev/.ssh/devpod/config"),
            workspace_id,
            "claude",
            &["GH_TOKEN".to_owned()],
            Some("/workspaces/devlaunch"),
            &ssh::Reuse::Direct,
        )
        .expect("a quotable command");
        assert_eq!(
            workspace_named_by(&openssh),
            Some(workspace_id),
            "the OpenSSH argv: {openssh:?}"
        );

        // The devpod client takes the subcommand and supplies the program, so the
        // argv a manager reads is the program followed by the call's own args.
        let devpod_argv =
            devpod::Call::new(["ssh", workspace_id, "--workdir", "/workspaces/devlaunch"]).argv();
        assert_eq!(
            workspace_named_by(&devpod_argv),
            Some(workspace_id),
            "the devpod argv: {devpod_argv:?}"
        );
    }

    #[test]
    fn only_the_panes_of_this_tab_are_considered() {
        let panes = vec![
            herdr::PaneInfo {
                pane_id: "w1:p1".to_owned(),
                tab_id: "w1:t1".to_owned(),
                focused: false,
            },
            herdr::PaneInfo {
                pane_id: "w1:p2".to_owned(),
                tab_id: "w1:t2".to_owned(),
                focused: false,
            },
            herdr::PaneInfo {
                pane_id: "w1:p3".to_owned(),
                tab_id: "w1:t1".to_owned(),
                focused: false,
            },
        ];
        let mine: Vec<&str> = in_tab("w1:t1", &panes)
            .iter()
            .map(|pane| pane.pane_id.as_str())
            .collect();
        assert_eq!(mine, ["w1:p1", "w1:p3"]);
    }

    #[test]
    fn the_focused_pane_of_a_tab_is_asked_first() {
        let panes = vec![
            herdr::PaneInfo {
                pane_id: "w1:p1".to_owned(),
                tab_id: "w1:t1".to_owned(),
                focused: false,
            },
            herdr::PaneInfo {
                pane_id: "w1:p2".to_owned(),
                tab_id: "w1:t1".to_owned(),
                focused: true,
            },
            herdr::PaneInfo {
                pane_id: "w1:p3".to_owned(),
                tab_id: "w1:t1".to_owned(),
                focused: false,
            },
        ];
        let mine: Vec<&str> = in_tab("w1:t1", &panes)
            .iter()
            .map(|pane| pane.pane_id.as_str())
            .collect();
        assert_eq!(mine, ["w1:p2", "w1:p1", "w1:p3"]);
    }

    /// The ordinary shell on a machine that has herdr installed. **Nothing is
    /// asked**, which is the part worth asserting: this runs in front of every
    /// pane, and a pane that is not devlaunch's must not pay a round trip.
    #[test]
    fn a_pane_outside_herdr_asks_herdr_nothing() {
        let runner = ScriptedRunner::new();
        runner.on_unscripted(Unscripted::Panic);
        assert_eq!(
            destination_for(&runner, PaneEnv::default()),
            PaneDestination::HostShell
        );
        assert_eq!(runner.call_count(), 0);
    }

    /// herdr exports the variable into every pane it spawns, so an empty value is
    /// a herdr saying "no tab" rather than a herdr that is absent.
    #[test]
    fn an_exported_but_empty_tab_id_asks_herdr_nothing() {
        let runner = ScriptedRunner::new();
        runner.on_unscripted(Unscripted::Panic);
        assert_eq!(
            destination_for(
                &runner,
                PaneEnv {
                    tab_id: Some(""),
                    ..PaneEnv::default()
                }
            ),
            PaneDestination::HostShell
        );
        assert_eq!(runner.call_count(), 0);
    }

    #[test]
    fn a_tab_holding_a_session_puts_the_pane_in_its_workspace() {
        let runner = ScriptedRunner::new();
        runner.on_unscripted(Unscripted::Panic);
        runner.script(
            [LENT_HERDR, "pane", "list"],
            Response::stdout(pane_list(&[("w1:p1", "w1:t1", true)])),
        );
        runner.script(
            [LENT_HERDR, "pane", "process-info", "--pane", "w1:p1"],
            Response::stdout(process_info(&[
                &["dl", "blooop/devlaunch@main"],
                &["devpod", "ssh", "devlaunch-main-3j1t", "--workdir", "/w"],
            ])),
        );
        assert_eq!(
            destination(&runner, in_pane_of("w1:t1")),
            PaneDestination::Workspace {
                workspace_id: "devlaunch-main-3j1t".to_owned(),
                claude_profile: None,
            }
        );
    }

    /// The case a note kept against a tab id gets wrong, and the reason this asks
    /// herdr every time: the session has gone, the tab is still there, and the
    /// answer has to be a host shell.
    #[test]
    fn a_tab_whose_session_has_exited_opens_a_host_shell() {
        let runner = ScriptedRunner::new();
        runner.on_unscripted(Unscripted::Panic);
        runner.script(
            [LENT_HERDR, "pane", "list"],
            Response::stdout(pane_list(&[("w1:p1", "w1:t1", true)])),
        );
        runner.script(
            [LENT_HERDR, "pane", "process-info", "--pane", "w1:p1"],
            Response::stdout(process_info(&[&["bash"]])),
        );
        assert_eq!(
            destination(&runner, in_pane_of("w1:t1")),
            PaneDestination::HostShell
        );
    }

    /// A pane herdr describes as running nothing at all. Distinct from a pane
    /// running something that is not a transport: this one is the shape herdr
    /// answers with when it could read the pty and found no foreground process,
    /// and an empty list must read as "no workspace here" rather than reaching
    /// anything that assumes a head.
    #[test]
    fn a_pane_with_no_foreground_process_holds_no_workspace() {
        let runner = ScriptedRunner::new();
        runner.on_unscripted(Unscripted::Panic);
        runner.script(
            [LENT_HERDR, "pane", "list"],
            Response::stdout(pane_list(&[("w1:p1", "w1:t1", true)])),
        );
        runner.script(
            [LENT_HERDR, "pane", "process-info", "--pane", "w1:p1"],
            Response::stdout(process_info(&[])),
        );
        assert_eq!(
            destination(&runner, in_pane_of("w1:t1")),
            PaneDestination::HostShell
        );
    }

    /// A pane herdr will not describe must not end the search: the tab's other
    /// pane is the one holding the session.
    #[test]
    fn a_pane_herdr_will_not_describe_is_stepped_over() {
        let runner = ScriptedRunner::new();
        runner.on_unscripted(Unscripted::Panic);
        runner.script(
            [LENT_HERDR, "pane", "list"],
            Response::stdout(pane_list(&[
                ("w1:p1", "w1:t1", true),
                ("w1:p2", "w1:t1", false),
            ])),
        );
        runner.script(
            [LENT_HERDR, "pane", "process-info", "--pane", "w1:p1"],
            Response::exited(1),
        );
        runner.script(
            [LENT_HERDR, "pane", "process-info", "--pane", "w1:p2"],
            Response::stdout(process_info(&[&["ssh", "-t", "ws-3j1t.devpod", "true"]])),
        );
        assert_eq!(
            destination(&runner, in_pane_of("w1:t1")),
            PaneDestination::Workspace {
                workspace_id: "ws-3j1t".to_owned(),
                claude_profile: None,
            }
        );
    }

    /// Every way herdr can fail to answer, and one consequence for all of them. A
    /// pane opens either way, which is the whole promise this sits on.
    #[test]
    fn a_herdr_that_will_not_answer_costs_the_container_and_not_the_pane() {
        let refusals = [
            // The refusal herdr actually writes when its server is down: exit 1,
            // and well-formed JSON with `error` where `result` would be.
            Response::Ran {
                exit: crate::runner::Exit::Code(1),
                stdout: r#"{"id":"cli:pane:list","error":{"code":"server_not_running","message":"no herdr server is running"}}"#.to_owned(),
                stderr: String::new(),
            },
            Response::stdout("not json at all"),
            // Exit 0 and an envelope for some other question.
            Response::stdout(r#"{"id":"cli:pane:list","result":{"type":"pong","version":"0.8.2","protocol":20}}"#),
            Response::ProgramNotFound,
            Response::TimedOut,
        ];
        for refusal in refusals {
            let runner = ScriptedRunner::new();
            runner.script([LENT_HERDR], refusal.clone());
            assert_eq!(
                destination(&runner, in_pane_of("w1:t1")),
                PaneDestination::HostShell,
                "{refusal:?}"
            );
        }
    }

    /// The bound is the point rather than the number: this runs in front of every
    /// pane, so a herdr that accepts a connection and never answers has to be
    /// abandoned instead of holding the pane open.
    #[test]
    fn every_question_asked_of_herdr_is_bounded() {
        let runner = ScriptedRunner::new();
        runner.script(
            [LENT_HERDR, "pane", "list"],
            Response::stdout(pane_list(&[])),
        );
        destination(&runner, in_pane_of("w1:t1"));
        let calls = runner.calls_to(LENT_HERDR);
        assert!(!calls.is_empty(), "herdr was never asked");
        for call in calls {
            assert_eq!(
                call.spec().and_then(|spec| spec.timeout),
                Some(herdr::ANSWER_WITHIN),
                "{:?} is unbounded",
                call.argv()
            );
        }
    }

    /// The bound is on the whole question, not on each part of it.
    ///
    /// The per-call spelling that came first let a tab of N panes cost
    /// `ANSWER_WITHIN * (N + 1)` -- a ten-pane tab against a herdr that accepts and
    /// never answers held the new pane for five and a half seconds, while three
    /// separate comments promised half a second "for the lot", and nothing bounds
    /// N.
    ///
    /// Asserted on [`Budget`] rather than on a run, and that is a real limit worth
    /// stating: the scripted runner answers instantly, so a run through it spends
    /// nothing and every call is granted the whole remaining budget. Summing the
    /// grants would therefore assert a property the design does not have. What
    /// bounds the wall clock is this type -- a call that burns its grant leaves
    /// nothing for the next one, and `next()` then declines to ask at all rather
    /// than asking unbounded.
    #[test]
    fn a_spent_budget_stops_the_questioning_rather_than_unbounding_it() {
        let mut budget = Budget::of(Duration::from_millis(500));
        assert_eq!(budget.next(), Some(Duration::from_millis(500)));

        budget.spend(Duration::from_millis(400));
        assert_eq!(
            budget.next(),
            Some(Duration::from_millis(100)),
            "the second question was granted the first one's budget again"
        );

        budget.spend(Duration::from_millis(100));
        assert_eq!(
            budget.next(),
            None,
            "a spent budget asked anyway, and with no bound on the answer"
        );

        // A call that overran its grant leaves nothing, rather than wrapping into
        // a budget larger than the one it started with.
        budget.spend(Duration::from_secs(9));
        assert_eq!(budget.next(), None);
    }

    /// And no single question may be granted more than the whole.
    #[test]
    fn no_question_is_granted_more_than_the_whole_budget() {
        let runner = ScriptedRunner::new();
        runner.script(
            [LENT_HERDR, "pane", "list"],
            Response::stdout(pane_list(&[
                ("w1:p1", "w1:t1", true),
                ("w1:p2", "w1:t1", false),
            ])),
        );
        runner.script(
            [LENT_HERDR, "pane", "process-info"],
            Response::stdout(process_info(&[&["bash"]])),
        );
        destination(&runner, in_pane_of("w1:t1"));

        let calls = runner.calls_to(LENT_HERDR);
        assert!(!calls.is_empty(), "herdr was never asked");
        for call in calls {
            let granted = call.spec().and_then(|spec| spec.timeout);
            assert!(
                granted.is_some_and(|grant| grant <= herdr::ANSWER_WITHIN),
                "{:?} was granted {granted:?}",
                call.argv()
            );
        }
    }

    /// The binary herdr exported, because that copy is the one whose socket owns
    /// this pane. A `PATH` lookup is what a herdr too old to export one leaves.
    #[test]
    fn the_exported_binary_is_preferred_over_the_one_on_path() {
        let runner = ScriptedRunner::new();
        runner.script(
            [LENT_HERDR, "pane", "list"],
            Response::stdout(pane_list(&[])),
        );
        destination(&runner, in_pane_of("w1:t1"));
        assert_eq!(runner.calls_to(LENT_HERDR).len(), 1);

        let runner = ScriptedRunner::new();
        runner.script(
            [launch::HERDR_BIN_FALLBACK, "pane", "list"],
            Response::stdout(pane_list(&[])),
        );
        destination_for(
            &runner,
            PaneEnv {
                tab_id: Some("w1:t1"),
                ..PaneEnv::default()
            },
        );
        assert_eq!(runner.calls_to(launch::HERDR_BIN_FALLBACK).len(), 1);
    }

    /// The three words that belong to another program, pinned where a change to
    /// them is a change to this feature and not a rename.
    #[test]
    fn the_two_questions_are_asked_in_herdrs_own_words() {
        assert_eq!(herdr::pane_list_argv(), ["pane", "list"]);
        assert_eq!(
            herdr::process_info_argv("w1:p7"),
            ["pane", "process-info", "--pane", "w1:p7"]
        );
        assert_eq!(launch::HERDR_TAB_VAR, "HERDR_TAB_ID");
    }
}
