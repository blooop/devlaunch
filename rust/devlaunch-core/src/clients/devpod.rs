//! Everything devlaunch asks `devpod`, and everything it reads back.
//!
//! Ported from `dl.py`'s `run_devpod`/`run_devpod_session`/`parse_workspaces`,
//! `devlaunch/devpod_provider.py` and `devlaunch/devpod_ssh.py`; see
//! docs/rust-rewrite-plan.md (M3).
//!
//! # One spawn, four ways of not answering
//!
//! `dl.py` funnels every devpod process through one helper so that exactly one
//! place can tell "devpod is not installed" from "devpod ran and failed". That
//! stays true here: [`capture`], [`run`] and [`session`] are the only callers of
//! the runner in this module, and each answers `Result<_, `[`NotRun`]`>` — so a
//! devpod that never spoke cannot be read as a devpod that answered. Python
//! raises `DevpodNotInstalled` for that case and `main()` renders exit 127; the
//! arm is [`NotRun::NotInstalled`] and the rendering is still the binary's.
//!
//! Being the one spawn is also what makes it the one place a devpod round trip is
//! *timed*: each of the three opens a [`timing`] span named
//! [`Call::round_trip`] — Python's `" ".join(cmd[:2])` — so a new devpod call is
//! measured by existing rather than by its caller remembering to wrap it. A span
//! recorded here lands in whatever stage the calling flow has open, which is how
//! `devpod status` is charged to `devpod-up` without this module knowing there are
//! stages.
//!
//! # What devpod says, as data
//!
//! Four things devpod is asked are *read* rather than merely run, and each has
//! a typed refusal beside its answer, because the failure to read an answer must
//! never be served as an answer (devlaunch#171's rule, arrived at three times
//! over):
//!
//! - the workspace listing — [`list_workspaces`], where an unreadable listing is
//!   [`ListingUnreadable`] and **not** an empty list;
//! - one workspace's container state — [`status`], over devpod's own vocabulary
//!   plus [`ContainerState::Unknown`], which is what keeps this total against a
//!   devpod newer than this build;
//! - the registered providers — [`provider_names`], where output that could not
//!   be read is not "no providers are registered" (which would mean "go add
//!   one", turning an unreadable answer into an action);
//! - the context options — [`context_options`].
//!
//! # What is deliberately absent
//!
//! - **The memoized listing.** Python remembers one `devpod list` per process
//!   and every mutation invalidates it. That is flow state — which command just
//!   changed what devpod would list — so it belongs to the flows that mutate,
//!   not to a client whose functions are otherwise stateless over a runner.
//! - **The on-disk context-options cache** (TTL plus a staleness check against
//!   devpod's own `config.yaml`). Same reason, plus it is storage.
//! - **The argv of `up`, `stop` and `delete`.** Those tails are composed from
//!   config, mounts, credentials and dotfiles by the lifecycle and launch flows;
//!   what this module owns is the spawn they go through and the answers they
//!   read back.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

use crate::json::JsonKind;
use crate::runner::{
    CapturedText, EnvSpec, Exit, Invocation, OsFailure, Outcome, Runner, SessionOutput, SpawnSpec,
    StdinPlan,
};
use crate::timing;

/// The program every call in this module runs. One constant, so no caller
/// spells it and the response table of a test has one name to match on.
pub(crate) const PROGRAM: &str = "devpod";

// ---------------------------------------------------------------- the spawn

/// devpod never ran, or was killed before it could answer.
///
/// Three arms rather than an exit status, because none of them is one:
///
/// - [`NotRun::NotInstalled`] is Python's `DevpodNotInstalled`, which travels as
///   a type nothing between the spawn and `main()` catches. Folded into a status,
///   a caller branching on the status would carry on as though devpod had
///   answered. The `dl` binary renders it as exit 127.
/// - [`NotRun::TimedOut`] is a child this process killed, so any status it has is
///   devlaunch's doing and says nothing about the work.
/// - [`NotRun::Blocked`] is every other OS refusal — an unreadable stdin file, a
///   working directory that is gone — carrying the errno and no message, because
///   a message for a person is the binary's to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotRun {
    NotInstalled,
    TimedOut,
    Blocked(OsFailure),
}

/// One devpod call: the argv *after* `devpod`, plus the three exotics Python's
/// `run_devpod` carries.
///
/// The argv tail rather than the whole argv, because that is how every caller
/// builds it — a subcommand and its flags — and because the program name is not
/// theirs to choose. Capture is not a field: it is the function called, so a
/// captured answer carries text and an inherited one carries none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Call {
    args: Vec<String>,
    /// The environment devpod runs with. [`EnvSpec::empty`] plus entries is
    /// Python's `env=` — the whole environment replaced — which exists so a
    /// secret can be handed to devpod without putting it in argv, where `ps`
    /// would show it to every other user on the host.
    env: EnvSpec,
    stdin: StdinPlan,
    timeout: Option<Duration>,
    /// Whether a passthrough of this call leads a process group of its own, so
    /// `dl`'s interrupt handler can tear it down independently. Only `devpod up`
    /// sets it; see [`SpawnSpec::own_group`].
    own_group: bool,
}

impl Call {
    pub(crate) fn new<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            args: args.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    #[must_use]
    pub(crate) fn with_env(mut self, env: EnvSpec) -> Self {
        self.env = env;
        self
    }

    /// This file becomes devpod's stdin — how `tools.py` streams the host's
    /// binaries into a container over `devpod ssh`. A path rather than bytes,
    /// because the payload runs to hundreds of megabytes.
    #[must_use]
    pub(crate) fn with_stdin_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.stdin = StdinPlan::File(path.into());
        self
    }

    /// One verb sets a bound, and both of its calls do: `dl <ws> kill`'s
    /// resolution, through [`Patience::UpTo`], and the delete it ends in, through
    /// `lifecycle::WEDGED_DELETE`. Every other devpod call waits as long as devpod
    /// does. The two are the same argument twice: `kill` is typed by somebody who
    /// has just sat through devpod's own five-second lock loop, so no call it makes
    /// may be capable of joining it.
    #[must_use]
    pub(crate) fn with_timeout(mut self, limit: Duration) -> Self {
        self.timeout = Some(limit);
        self
    }

    /// A passthrough of this call should lead a process group of its own, so
    /// `dl`'s interrupt handler can `killpg` it — for the long `devpod up` build.
    /// See [`SpawnSpec::own_group`].
    #[must_use]
    pub(crate) fn leading_its_own_group(mut self) -> Self {
        self.own_group = true;
        self
    }

    /// The whole argv, `devpod` included — what a recorded call compares against.
    pub(crate) fn argv(&self) -> Vec<String> {
        self.spec().invocation.argv()
    }

    /// How a timing summary names this round trip: `" ".join(cmd[:2])`, which is
    /// what Python's `run_devpod` spans every devpod call under.
    ///
    /// The subcommand and not the whole argv, so the summary names each trip
    /// (`devpod status`, `devpod ssh`, `devpod up`) without leaking a workspace id
    /// into it. A call with no subcommand at all is not one anything makes, and
    /// naming it `devpod` is what Python's slice does with it.
    fn round_trip(&self) -> String {
        match self.args.first() {
            Some(subcommand) => format!("{PROGRAM} {subcommand}"),
            None => PROGRAM.to_owned(),
        }
    }

    fn spec(&self) -> SpawnSpec {
        SpawnSpec {
            invocation: Invocation::new(PROGRAM)
                .with_args(self.args.iter().cloned())
                .with_env(self.env.clone()),
            stdin: self.stdin.clone(),
            timeout: self.timeout,
            own_group: self.own_group,
        }
    }
}

/// What a captured devpod call answered: how it ended, and what it wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Answer {
    pub(crate) exit: Exit,
    pub(crate) text: CapturedText,
}

impl Answer {
    pub(crate) fn succeeded(&self) -> bool {
        self.exit.is_success()
    }

    pub(crate) fn stdout(&self) -> &str {
        &self.text.stdout
    }

    pub(crate) fn stderr(&self) -> &str {
        &self.text.stderr
    }
}

/// Ask devpod something and read both its streams.
pub(crate) fn capture(runner: &dyn Runner, call: &Call) -> Result<Answer, NotRun> {
    let _span = timing::span(call.round_trip());
    let (exit, text) = ran(runner.capture(&call.spec()))?;
    Ok(Answer { exit, text })
}

/// Run devpod with this process's streams, capturing nothing: `devpod up` builds
/// an image for minutes and its progress belongs on the user's terminal.
pub(crate) fn run(runner: &dyn Runner, call: &Call) -> Result<Exit, NotRun> {
    let _span = timing::span(call.round_trip());
    ran(runner.passthrough(&call.spec())).map(|(exit, ())| exit)
}

/// The same call, with devpod's stderr read a line at a time as it arrives.
///
/// stdin and stdout are still this process's, so a caller that was a passthrough
/// stays one from the outside: every line handed over is written straight back to
/// stderr, in order, and the only thing that changed is that dl has seen it.
///
/// For the calls where a line devpod prints is worth *acting* on rather than only
/// showing. That is a narrow set and deliberately so: it is the lines that mean
/// the call is not going to finish, which nothing downstream of it can report,
/// because there is no downstream.
pub(crate) fn run_watching_stderr(
    runner: &dyn Runner,
    call: &Call,
    on_line: &mut dyn FnMut(&str),
) -> Result<Exit, NotRun> {
    let _span = timing::span(call.round_trip());
    let mut forward = |line: &str| {
        eprintln!("{line}");
        on_line(line);
    };
    ran(runner.session(&call.spec(), &mut forward)).map(|(exit, ())| exit)
}

/// The same call, echoed as it arrives, with the silences between lines timed.
///
/// For the one call that goes quiet for minutes at a stretch and gives a reader
/// no way to tell that from a hang (devlaunch#576). The lines are written back to
/// stderr exactly as [`run_watching_stderr`] writes them, so nothing about what
/// devpod's output looks like changes; what is added is a report on the gaps,
/// which is the thing no line sink can produce because a callback that is not
/// being called says nothing.
///
/// `interval` is how often a continuing silence is reported and the measurement
/// restarts at every line, so what `on_quiet` is handed is the age of the step
/// devpod is on rather than the age of the call.
pub(crate) fn run_watching_silence(
    runner: &dyn Runner,
    call: &Call,
    interval: Duration,
    on_quiet: &mut dyn FnMut(Duration),
) -> Result<Exit, NotRun> {
    let _span = timing::span(call.round_trip());
    let mut watcher = Echoing { interval, on_quiet };
    ran(runner.watched_session(&call.spec(), &mut watcher)).map(|(exit, ())| exit)
}

/// [`run_watching_silence`]'s watcher: echo the lines, hand the gaps over.
struct Echoing<'a> {
    interval: Duration,
    on_quiet: &'a mut dyn FnMut(Duration),
}

impl SessionOutput for Echoing<'_> {
    fn line(&mut self, line: &str) {
        eprintln!("{line}");
    }

    fn quiet_interval(&self) -> Option<Duration> {
        Some(self.interval)
    }

    fn quiet(&mut self, quiet: Duration) {
        (self.on_quiet)(quiet);
    }
}

/// Whether this is devpod saying it is blocked on a workspace's lock.
///
/// devpod's `initLock` is a *blocking* `flock` acquire with no deadline behind it,
/// and it logs this on a five-second timer while it waits — so the line does not
/// mean "slow", it means "for as long as whatever holds the lock lives". Matched
/// on the stable half of the sentence: the tail names a source file and a line
/// number that move between devpod releases.
pub(crate) fn says_it_is_blocked(line: &str) -> bool {
    line.contains("Trying to lock workspace")
}

/// An outcome, split into "it ran, this is what came back" and "it did not".
///
/// One function over all four arms rather than a `_ =>` at each call site: the
/// three ways of not running are then translated in exactly one place, and no
/// caller has an arm it has to claim is unreachable.
fn ran<T>(outcome: Outcome<T>) -> Result<(Exit, T), NotRun> {
    match outcome {
        Outcome::Ran { exit, io } => Ok((exit, io)),
        Outcome::ProgramNotFound => Err(NotRun::NotInstalled),
        Outcome::TimedOut => Err(NotRun::TimedOut),
        // A devpod found on PATH but that could not be exec'd — the classic case
        // is a wrapper whose interpreter is missing — fails with ENOENT at exec
        // time, which arrives here as a `NotStarted` carrying `NotFound` rather
        // than as `ProgramNotFound`. Python does not draw that line: `run_devpod`
        // catches every `FileNotFoundError` `subprocess.run` raises — a devpod not
        // on PATH and one that could not be exec'd alike — and turns both into
        // `DevpodNotInstalled`, which renders exit 127 with the install hint. So a
        // NotFound `NotStarted` reaches the same devpod-missing path here; only a
        // different OS refusal (permission, a working directory that is gone)
        // stays `Blocked`, carrying its errno for the binary to phrase (parity F1).
        Outcome::NotStarted(failure) if failure.kind == std::io::ErrorKind::NotFound => {
            Err(NotRun::NotInstalled)
        }
        Outcome::NotStarted(failure) => Err(NotRun::Blocked(failure)),
    }
}

// ------------------------------------ how a session ended (devpod_ssh.py)

/// How a `devpod ssh` session ended.
///
/// devpod means to pass a remote process's exit status through, and its
/// top-level handler type-asserts on `*ssh.ExitError` — but by then the error has
/// been wrapped three times with `%w`, which a bare type assertion does not see
/// through. So every nonzero remote exit misses that branch, lands on devpod's
/// generic failure path, and exits 1 with the status buried in a `fatal` line.
///
/// Nothing has gone wrong in that case: a login shell exits with the status of
/// its last command, so one Ctrl-C before `exit` is enough to end a perfectly
/// ordinary session 130. The distinction the rest of devlaunch needs is *which
/// process* the number came from, which is why this is a sum and not an `i32`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SshOutcome {
    /// devpod ran the remote program, and it exited with `status`.
    ///
    /// Not a devlaunch failure, whatever `status` is: the shell or command the
    /// user asked for ran to completion, and the status belongs to it.
    RemoteExit { status: i32 },
    /// devpod never ran the remote program, or lost it partway.
    ///
    /// `exit` is devpod's own ending. It carries no message: devpod has already
    /// written its diagnostics to the user's stderr by the time this exists.
    DevpodFailed { exit: Exit },
}

/// devpod prints this immediately before the fatal it belongs to, so it has to
/// be held for one line to see which fatal that is.
const DEBUG_HINT: &str = "Try using the --debug flag to see a more verbose output";

/// The sentence `golang.org/x/crypto/ssh` formats into an `*ssh.ExitError`,
/// followed by the status. Optionally trailed by " from signal SIGINT" and
/// ". Reason was: …", neither of which is read.
const REMOTE_EXIT_MARKER: &str = "ssh session: Process exited with status ";

/// devpod's own level tag, which the report has to be anchored on as well: a
/// remote program printing the same sentence on its own stderr (which reaches
/// devlaunch only when there is no pty) must not be mistaken for devpod's report.
const FATAL_TAG: &str = "fatal";

/// Forward devpod's stderr, holding back its report of a remote exit status.
///
/// Stateful because it is fed one line at a time as the session runs: the
/// `--debug` hint is released *ahead* of the fatal it introduces rather than
/// after it, which costs exactly one line of lookahead.
#[derive(Debug, Default)]
pub(crate) struct StderrFilter {
    held_hint: Option<String>,
    remote_status: Option<i32>,
}

impl StderrFilter {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Feed one line, forwarding whatever the user should see.
    ///
    /// Lines arrive without their newline — the runner strips it — and are
    /// forwarded exactly as they came, because everything devpod says for its own
    /// sake must read as it does today.
    pub(crate) fn push(&mut self, line: &str, forward: &mut dyn FnMut(&str)) {
        if let Some(status) = recovered_status(line) {
            self.remote_status = Some(status);
            // The hint introduced this fatal, so it goes with it.
            self.held_hint = None;
            return;
        }
        if line.contains(DEBUG_HINT) {
            self.held_hint = Some(line.to_owned());
            return;
        }
        if let Some(hint) = self.held_hint.take() {
            forward(&hint);
        }
        forward(line);
    }

    /// The stream ended: release a hint nothing followed, and report the status
    /// devpod buried, if it buried one.
    pub(crate) fn finish(&mut self, forward: &mut dyn FnMut(&str)) -> Option<i32> {
        if let Some(hint) = self.held_hint.take() {
            forward(&hint);
        }
        self.remote_status
    }
}

/// The remote exit status `line` reports, if it is devpod reporting one.
///
/// Hand-rolled rather than a regex, and it is the same predicate Python's
/// pattern spells: a `fatal` tag with a word boundary after it, then the
/// x/crypto sentence with a word boundary before it, then digits. No boundary is
/// required *before* `fatal`: devpod colours the tag and the escape it emits ends
/// in `m`, so there is no word boundary in front of it.
fn recovered_status(line: &str) -> Option<i32> {
    for (at, _) in line.match_indices(FATAL_TAG) {
        let after_tag = at + FATAL_TAG.len();
        if !boundary_before(&line[after_tag..]) {
            continue;
        }
        let rest = &line[after_tag..];
        for (marker_at, _) in rest.match_indices(REMOTE_EXIT_MARKER) {
            if !boundary_after(&rest[..marker_at]) {
                continue;
            }
            let digits: String = rest[marker_at + REMOTE_EXIT_MARKER.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            // A count too large for an exit status is not a status anything
            // could have exited with, so it is not devpod reporting one.
            if let Ok(status) = digits.parse::<i32>() {
                return Some(status);
            }
        }
    }
    None
}

/// Whether a word boundary falls at the start of `after` — i.e. the character
/// following the match is not a word character, or there is none.
fn boundary_before(after: &str) -> bool {
    after.chars().next().is_none_or(|next| !is_word(next))
}

/// Whether a word boundary falls at the end of `before`.
fn boundary_after(before: &str) -> bool {
    before.chars().next_back().is_none_or(|prev| !is_word(prev))
}

/// `\w` as Python's `re` reads it for a `str`: alphanumeric or underscore.
fn is_word(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Decide what a finished `devpod ssh` actually reported.
///
/// A recovered remote status wins over devpod's own ending, because devpod
/// reports 1 alongside it regardless of what the remote program returned. With
/// no status to recover, devpod's own ending is passed on: either devpod really
/// did fail, or a future devpod unwraps the error properly and exits with the
/// remote status itself — in which case this is still the right number, and
/// devpod stayed quiet, so nothing spurious is printed either way.
pub(crate) fn interpret(devpod_exit: Exit, remote_status: Option<i32>) -> SshOutcome {
    match (remote_status, devpod_exit) {
        (Some(status), _) => SshOutcome::RemoteExit { status },
        (None, Exit::Code(0)) => SshOutcome::RemoteExit { status: 0 },
        (None, exit) => SshOutcome::DevpodFailed { exit },
    }
}

/// Run a devpod command that hands its stdin and stdout to a terminal session.
///
/// stdin and stdout are inherited untouched — devpod puts the real terminal into
/// raw mode through them and requests a pty on that basis, so a pipe on either
/// changes what devpod does. Only stderr is read, which under a pty carries
/// devpod's own warnings and nothing else, so its report of how the session
/// ended can be interpreted rather than dumped on the user.
///
/// `forward` is where the lines that *should* be seen go. Python writes them to
/// `sys.stderr` from inside the filter; core writes to nobody's stream, so the
/// sink is the caller's.
pub(crate) fn session(
    runner: &dyn Runner,
    call: &Call,
    forward: &mut dyn FnMut(&str),
) -> Result<SshOutcome, NotRun> {
    // The span covers the whole session, as Python's does: what the summary names
    // is the round trip the user waited on, not just the process spawn.
    let _span = timing::span(call.round_trip());
    let mut filter = StderrFilter::new();
    let outcome = runner.session(&call.spec(), &mut |line| filter.push(line, forward));
    let remote_status = filter.finish(forward);
    let (exit, ()) = ran(outcome)?;
    Ok(interpret(exit, remote_status))
}

// ------------------------------------------------------- container state

/// Whether a workspace's container is up, in devpod's own vocabulary.
///
/// Python carries this as `Optional[str]` and compares it to `"Running"`, which
/// makes every reader a place a typo could hide and gives "devpod said something
/// new" no representation at all. The four words are devpod's; the fifth arm is
/// what makes this total over a devpod newer than this build — a word nobody has
/// heard of is data, not a parse failure, and a reader asking
/// [`ContainerState::is_running`] gets the same answer either way.
#[derive(Clone, Debug, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub enum ContainerState {
    Running,
    /// devpod is working on this workspace right now.
    Busy,
    Stopped,
    /// devpod has a record of the workspace but no container for it.
    NotFound,
    /// A word this build does not know, kept whole so a report can show it.
    Unknown(String),
}

impl ContainerState {
    pub(crate) fn from_devpod_word(word: &str) -> Self {
        match word {
            "Running" => Self::Running,
            "Busy" => Self::Busy,
            "Stopped" => Self::Stopped,
            "NotFound" => Self::NotFound,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// The word devpod would print. An unknown state answers with what devpod
    /// said, which is the only honest thing it has.
    pub(crate) fn as_devpod_word(&self) -> &str {
        match self {
            Self::Running => "Running",
            Self::Busy => "Busy",
            Self::Stopped => "Stopped",
            Self::NotFound => "NotFound",
            Self::Unknown(word) => word,
        }
    }

    /// The one question every caller in `dl.py` asks of this value.
    pub(crate) fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }
}

/// Why devpod's answer about one workspace's state could not be read.
///
/// Python collapses all of these to `None`, which its callers then read as
/// "devpod knows no workspace for this triple". That reading is a decision about
/// what to do next, so it is left to the flow that has to make it rather than
/// baked into the parser: a devpod that refused the question and a devpod whose
/// answer was unreadable are different facts.
#[derive(Clone, Debug, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub enum StatusUnreadable {
    NotRun(NotRun),
    /// devpod ran and refused — which is what it does for a workspace it has
    /// never heard of.
    Failed {
        exit: Exit,
        stderr: String,
    },
    NotJson {
        output: String,
        reason: String,
    },
    /// JSON with no `state` string in it.
    NoState {
        output: String,
    },
}

/// How long a caller will wait for one `devpod status` to answer.
///
/// A parameter rather than one bound written into the call, because the two
/// callers want opposite things of it. A launch is asking devpod to *work*: a
/// provider that takes half a minute to describe a machine on the other side of
/// an ssh connection is slow rather than broken, and a deadline there would fail
/// a launch that was going to succeed. `dl <ws> kill` runs *because* something on
/// this host has stopped answering, so the call it opens with is exactly the one
/// that must not join the wedge — [`super::ps`] bounds its own read of the
/// process table on that argument, and the argument does not stop holding one
/// call earlier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub enum Patience {
    /// However long devpod takes. What every caller but the kill's resolution
    /// wants.
    AsLongAsItTakes,
    /// Abandon the call after this long, which answers
    /// [`StatusUnreadable::NotRun`] carrying [`std::io::ErrorKind::TimedOut`].
    UpTo(Duration),
}

/// The container state devpod reports for one workspace.
pub(crate) fn status(
    runner: &dyn Runner,
    workspace_id: &str,
    patience: Patience,
) -> Result<ContainerState, StatusUnreadable> {
    let call = Call::new(["status", workspace_id, "--output", "json"]);
    let call = match patience {
        Patience::AsLongAsItTakes => call,
        Patience::UpTo(limit) => call.with_timeout(limit),
    };
    let answer = capture(runner, &call).map_err(StatusUnreadable::NotRun)?;
    if !answer.succeeded() {
        return Err(StatusUnreadable::Failed {
            exit: answer.exit,
            stderr: answer.stderr().to_owned(),
        });
    }
    parse_status(answer.stdout())
}

/// The `state` in a `devpod status --output json` answer.
pub(crate) fn parse_status(output: &str) -> Result<ContainerState, StatusUnreadable> {
    let parsed: serde_json::Value =
        serde_json::from_str(output).map_err(|error| StatusUnreadable::NotJson {
            output: output.to_owned(),
            reason: error.to_string(),
        })?;
    match parsed.get("state").and_then(serde_json::Value::as_str) {
        Some(word) => Ok(ContainerState::from_devpod_word(word)),
        None => Err(StatusUnreadable::NoState {
            output: output.to_owned(),
        }),
    }
}

// ---------------------------------------------------- the workspace listing

/// A devpod workspace, as `devpod list --output json` lists it.
///
/// No container state, deliberately: real devpod answers state only to `status`,
/// per workspace, so a field for it here would be a field nothing could fill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub id: String,
    pub(crate) source: WorkspaceSource,
    pub(crate) last_used: String,
    pub(crate) provider: String,
    pub(crate) ide: String,
    /// The devpod context this workspace belongs to. Ids are unique per context,
    /// not globally, so it is half of the workspace's address on disk: it is one
    /// of the two things [`super::devpod_home::DevpodHome::record`] needs to name
    /// the record `--reconcile` re-points.
    pub(crate) context: String,
}

impl Workspace {
    /// What this workspace opens.
    ///
    /// An accessor rather than a public field: the field is filled by this module's
    /// parser and by nothing else, which is what makes holding a [`WorkspaceSource`]
    /// evidence that devpod said so. Readable because the `dl` binary renders it —
    /// the `--ls` table and the fuzzy picker both show how a source reads.
    ///
    /// binary surface — not part of the frozen wf API (#251 §7)
    pub fn source(&self) -> &WorkspaceSource {
        &self.source
    }
}

/// What a workspace opens.
///
/// A sum rather than Python's original tag-beside-a-parallel-string, where two
/// of the three tags described what the field held — a path, a URL — and the
/// third put a rendering of devpod's object in the same field. Each arm carries
/// only what that arm has, so the tag and the value are one fact.
#[derive(Clone, Debug, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub enum WorkspaceSource {
    /// devpod is opening this directory on this machine. Never empty: `git -C ""`
    /// is a no-op that succeeds, so an empty path would be credited with
    /// whatever repository the person running `dl` was standing in.
    LocalFolder(String),
    /// devpod is opening a repository it clones itself — usually named by URL.
    ///
    /// Usually, not always: `devpod up <path-to-a-repo>` records this arm
    /// carrying a path on this machine, which is why a placement reader reads it
    /// instead of refusing it (devlaunch#224). Never empty, as above.
    GitRepository(String),
    /// devpod says this workspace opens a folder, and devlaunch cannot say which.
    ///
    /// Its own arm rather than sharing [`WorkspaceSource::Unrecognised`], because
    /// the two are opposite answers to the one question a deletion has to ask,
    /// and sharing made the dangerous one silent: `--prune` reads "no path" as
    /// "contributes no path and no alarm", which is right for an image workspace
    /// and wrong for a live workspace that really is opening a directory.
    ///
    /// Reached by a `localFolder` devpod filled with something that is not a
    /// non-empty string: an object, a number, a list. The payload is the whole
    /// source object, so a report can show what devpod actually said.
    UnreadableLocalFolder(serde_json::Map<String, serde_json::Value>),
    /// A source that opens no directory on this machine.
    ///
    /// Reachable rather than defensive: devpod's workspace source also carries
    /// `image` and `container`, so `devpod up ubuntu:24.04` lands here. It has no
    /// path and no URL, and that is a *fact about the workspace* rather than a
    /// gap in devlaunch's reading — an image workspace mounts no folder here, so
    /// no clone directory can be at risk from it.
    Unrecognised(serde_json::Map<String, serde_json::Value>),
}

/// Why a `devpod list --output json` answer is not a listing.
///
/// Distinct from "this machine has no workspaces", which is a listing that reads
/// fine and is empty. The two used to share one representation — an empty list —
/// so `dl --purge` could report that there was nothing to purge when the truth
/// was that it never found out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotAListing {
    /// devpod said nothing. It prints `[]` when there are none, so silence is
    /// devpod failing to answer — and it gets an arm of its own rather than
    /// falling into the JSON parser, whose report of it (`not JSON: ''`) reads
    /// like a bug in dl rather than a devpod that never spoke.
    Silence,
    NotJson {
        output: String,
        reason: String,
    },
    NotAnArray {
        kind: JsonKind,
    },
    EntryNotAnObject {
        kind: JsonKind,
    },
    /// A `source` that is not an object at all. Refused for the listing rather
    /// than as a source dl cannot read, because the unreadable *source* arms hold
    /// the object devpod sent, and can only do that if it was one.
    SourceNotAnObject {
        workspace_id: String,
        kind: JsonKind,
    },
}

/// Why asking devpod for the workspace list produced no workspaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListingUnreadable {
    NotRun(NotRun),
    /// devpod ran and refused. Its stderr travels whole; keeping a report to one
    /// line is the binary's rendering.
    Failed {
        exit: Exit,
        stderr: String,
    },
    Unreadable(NotAListing),
}

/// Every workspace devpod lists, or why it listed none that can be believed.
///
/// Which of them belong to devlaunch is a separate question and not one this
/// answers; it answers only whether the list can be believed at all.
pub(crate) fn list_workspaces(runner: &dyn Runner) -> Result<Vec<Workspace>, ListingUnreadable> {
    let answer = capture(runner, &Call::new(["list", "--output", "json"]))
        .map_err(ListingUnreadable::NotRun)?;
    if !answer.succeeded() {
        return Err(ListingUnreadable::Failed {
            exit: answer.exit,
            stderr: answer.stderr().to_owned(),
        });
    }
    parse_workspaces(answer.stdout()).map_err(ListingUnreadable::Unreadable)
}

/// The workspaces in a `devpod list --output json` listing.
///
/// Anything that is not such a listing refuses rather than parsing to nothing.
pub(crate) fn parse_workspaces(listing: &str) -> Result<Vec<Workspace>, NotAListing> {
    if listing.trim().is_empty() {
        return Err(NotAListing::Silence);
    }
    let parsed: serde_json::Value =
        serde_json::from_str(listing).map_err(|error| NotAListing::NotJson {
            output: listing.to_owned(),
            reason: error.to_string(),
        })?;
    let serde_json::Value::Array(entries) = parsed else {
        return Err(NotAListing::NotAnArray {
            kind: JsonKind::of(&parsed),
        });
    };
    let mut workspaces = Vec::with_capacity(entries.len());
    for entry in entries {
        let serde_json::Value::Object(entry) = entry else {
            return Err(NotAListing::EntryNotAnObject {
                kind: JsonKind::of(&entry),
            });
        };
        workspaces.push(read_workspace(&entry)?);
    }
    Ok(workspaces)
}

/// One listing entry, or the reason the whole listing cannot be read.
fn read_workspace(
    entry: &serde_json::Map<String, serde_json::Value>,
) -> Result<Workspace, NotAListing> {
    let id = text(entry.get("id")).unwrap_or_default();
    let source = match entry.get("source") {
        None => serde_json::Map::new(),
        Some(serde_json::Value::Object(source)) => source.clone(),
        Some(other) => {
            return Err(NotAListing::SourceNotAnObject {
                workspace_id: id,
                kind: JsonKind::of(other),
            });
        }
    };
    Ok(Workspace {
        id,
        source: parse_source(source),
        last_used: text(entry.get("lastUsed")).unwrap_or_default(),
        provider: named(entry.get("provider")),
        ide: named(entry.get("ide")),
        // A falsy context is the default one, as Python's `or "default"` reads it.
        context: text(entry.get("context")).unwrap_or_else(|| DEFAULT_CONTEXT.to_owned()),
    })
}

/// The context a workspace is in when its listing does not say.
pub(crate) const DEFAULT_CONTEXT: &str = "default";

/// Read one `devpod list --output json` source object.
///
/// The keys are checked in devpod's own order of specificity, and a key has to be
/// non-empty text before it counts — an arm is only as honest as the value put
/// into it. Neither refusal is an unreadable *listing*: the object is right here
/// and is kept whole. Which unreadable arm it is turns on whether devpod claimed
/// a folder at all, because only a claimed folder can put a clone at risk.
pub(crate) fn parse_source(source: serde_json::Map<String, serde_json::Value>) -> WorkspaceSource {
    if let Some(path) = text(source.get("localFolder")) {
        return WorkspaceSource::LocalFolder(path);
    }
    // Not readable text, then — so if devpod put anything else there, it named a
    // folder devlaunch cannot name back: an object, a number, a list. `null` and
    // `""` are devpod saying there is no local folder at all, which is what an
    // image or container workspace has.
    if let Some(claimed) = source.get("localFolder")
        && !claimed.is_null()
        && claimed.as_str() != Some("")
    {
        return WorkspaceSource::UnreadableLocalFolder(source);
    }
    if let Some(url) = text(source.get("gitRepository")) {
        return WorkspaceSource::GitRepository(url);
    }
    WorkspaceSource::Unrecognised(source)
}

/// `value` if devpod filled it with non-empty text, else nothing.
///
/// devpod's listing is JSON dl did not write, so a key holds whatever the JSON
/// held; this is what keeps the arms above from being handed a value of a type
/// their field does not describe.
fn text(value: Option<&serde_json::Value>) -> Option<String> {
    match value.and_then(serde_json::Value::as_str) {
        Some(text) if !text.is_empty() => Some(text.to_owned()),
        _ => None,
    }
}

/// The `name` inside a `{"name": …}` object, or nothing.
///
/// Python reads `data.get("provider", {}).get("name", "")`, which raises if
/// devpod ever sends something that is not an object there — and a listing dl
/// cannot fully read is still a listing, whose ids every caller needs.
fn named(value: Option<&serde_json::Value>) -> String {
    value
        .and_then(|value| text(value.get("name")))
        .unwrap_or_default()
}

// ----------------------------------------------------------- the providers

/// Why devpod's provider listing could not be read.
///
/// Distinct from "no providers are registered", which is a listing that reads
/// fine and is empty. The distinction is the whole point: the old guard grepped
/// devpod's colourised table, and output it could not read came back as an empty
/// set of providers — and an empty set means "go add one", so an unreadable
/// answer turned into an action.
// The provider guard is complete and tested here; the flow that registers a
// provider is not wired yet.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NotAProviderListing {
    NotJson { output: String, reason: String },
    NotKeyedByName { kind: JsonKind },
}

/// Why asking devpod which providers are registered produced no answer.
// The provider guard is complete and tested here; the flow that registers a
// provider is not wired yet.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProviderListUnreadable {
    NotRun(NotRun),
    Failed { exit: Exit, stderr: String },
    Unreadable(NotAProviderListing),
}

/// Why `devpod provider add` did not register a provider.
///
/// Kept apart from a listing that could not be read: devpod answered the
/// question it was asked, and then refused to do the thing.
// The provider guard is complete and tested here; the flow that registers a
// provider is not wired yet.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AddFailed {
    NotRun(NotRun),
    Refused { exit: Exit, stderr: String },
}

/// Why the provider guard could not finish.
// The provider guard is complete and tested here; the flow that registers a
// provider is not wired yet.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EnsureProviderFailed {
    ListUnreadable(ProviderListUnreadable),
    AddFailed(AddFailed),
}

/// Whether the guard had to register the provider, or found it already there.
// The provider guard is complete and tested here; the flow that registers a
// provider is not wired yet.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProviderRegistration {
    AlreadyRegistered,
    Added,
}

/// The names of the providers devpod has registered.
// The provider guard is complete and tested here; the flow that registers a
// provider is not wired yet.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn provider_names(
    runner: &dyn Runner,
) -> Result<BTreeSet<String>, ProviderListUnreadable> {
    let call = Call::new(["provider", "list", "--output", "json"]);
    let answer = capture(runner, &call).map_err(ProviderListUnreadable::NotRun)?;
    if !answer.succeeded() {
        return Err(ProviderListUnreadable::Failed {
            exit: answer.exit,
            stderr: answer.stderr().to_owned(),
        });
    }
    parse_provider_names(answer.stdout()).map_err(ProviderListUnreadable::Unreadable)
}

/// Every registered provider in a `--output json` listing: an object keyed by
/// provider name, which is the same information as devpod's table in a form that
/// has no rendering to change.
// The provider guard is complete and tested here; the flow that registers a
// provider is not wired yet.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn parse_provider_names(listing: &str) -> Result<BTreeSet<String>, NotAProviderListing> {
    let parsed: serde_json::Value =
        serde_json::from_str(listing).map_err(|error| NotAProviderListing::NotJson {
            output: listing.to_owned(),
            reason: error.to_string(),
        })?;
    match parsed {
        serde_json::Value::Object(providers) => {
            Ok(providers.into_iter().map(|(name, _)| name).collect())
        }
        other => Err(NotAProviderListing::NotKeyedByName {
            kind: JsonKind::of(&other),
        }),
    }
}

/// Register `name` with devpod unless it is already registered.
///
/// Refuses rather than guessing when devpod's answer cannot be read: acting on an
/// unreadable listing is the defect this guard exists to have fixed.
// The provider guard is complete and tested here; the flow that registers a
// provider is not wired yet.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn ensure_provider(
    runner: &dyn Runner,
    name: &str,
) -> Result<ProviderRegistration, EnsureProviderFailed> {
    let registered = provider_names(runner).map_err(EnsureProviderFailed::ListUnreadable)?;
    if registered.contains(name) {
        return Ok(ProviderRegistration::AlreadyRegistered);
    }
    // Captured, so devpod's own explanation of a refused add survives into the
    // report: without capture there is nothing to quote.
    let answer = capture(runner, &Call::new(["provider", "add", name]))
        .map_err(|not_run| EnsureProviderFailed::AddFailed(AddFailed::NotRun(not_run)))?;
    if !answer.succeeded() {
        return Err(EnsureProviderFailed::AddFailed(AddFailed::Refused {
            exit: answer.exit,
            stderr: answer.stderr().to_owned(),
        }));
    }
    Ok(ProviderRegistration::Added)
}

// ------------------------------------------------------ the context options

/// Why devpod's context options could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OptionsUnreadable {
    NotRun(NotRun),
    Failed {
        exit: Exit,
        stderr: String,
    },
    NotJson {
        output: String,
        reason: String,
    },
    NotKeyedByOption {
        kind: JsonKind,
    },
    /// A top-level option whose body is not an object — `{"FOO": 3}` rather than
    /// `{"FOO": {"value": …}}`. Python's `v.get("value")` raises `AttributeError`
    /// on such a value and dl returns `{}` without caching it; this is that
    /// refusal, so the unusable answer is re-asked next run rather than remembered
    /// as an empty cache (P9).
    OptionNotAnObject {
        option: String,
        kind: JsonKind,
    },
}

/// The devpod context options that have a value set, as `{name: value}`.
///
/// Per *context*, which is why the flow that caches this on disk expires the
/// cache when devpod's own config file changes and not only on a TTL: `devpod
/// context use <other>` would otherwise feed the previous context's settings to
/// `devpod up` for an hour.
pub(crate) fn context_options(
    runner: &dyn Runner,
) -> Result<BTreeMap<String, String>, OptionsUnreadable> {
    let call = Call::new(["context", "options", "--output", "json"]);
    let answer = capture(runner, &call).map_err(OptionsUnreadable::NotRun)?;
    if !answer.succeeded() {
        return Err(OptionsUnreadable::Failed {
            exit: answer.exit,
            stderr: answer.stderr().to_owned(),
        });
    }
    parse_context_options(answer.stdout())
}

/// The options in a `devpod context options --output json` answer.
///
/// Silence is an empty map rather than a refusal, unlike the workspace listing:
/// an option dl does not find is one it does not pass on, where a listing it
/// could not read decides what gets deleted.
///
/// An option is kept only when devpod gave it a non-empty string value. Python
/// filters on truthiness, which also keeps a number or a `true`; nothing devpod
/// documents puts one there, and a value that is not text has no business on a
/// `devpod up` flag.
pub(crate) fn parse_context_options(
    output: &str,
) -> Result<BTreeMap<String, String>, OptionsUnreadable> {
    if output.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let parsed: serde_json::Value =
        serde_json::from_str(output).map_err(|error| OptionsUnreadable::NotJson {
            output: output.to_owned(),
            reason: error.to_string(),
        })?;
    let serde_json::Value::Object(options) = parsed else {
        return Err(OptionsUnreadable::NotKeyedByOption {
            kind: JsonKind::of(&parsed),
        });
    };
    let mut kept = BTreeMap::new();
    for (name, option) in options {
        // Python evaluates `v.get("value")` for every option; a value that is not
        // itself a JSON object raises AttributeError there, and dl returns `{}`
        // WITHOUT caching. Refuse it here for the same reason: an answer that is
        // not `{name: {value: …}}` is re-asked next run rather than remembered as
        // an empty cache (P9). An empty object `{}` is still an object and simply
        // contributes no option, matching `{}.get("value")` being falsey.
        let serde_json::Value::Object(fields) = &option else {
            return Err(OptionsUnreadable::OptionNotAnObject {
                option: name,
                kind: JsonKind::of(&option),
            });
        };
        if let Some(value) = text(fields.get("value")) {
            kept.insert(name, value);
        }
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    //! # The fake runner
    //!
    //! [`ScriptedRunner`](crate::testing::ScriptedRunner) is the workspace's one
    //! fake — `devlaunch-test-support`'s recorder, its argv-prefix response table
    //! and the devpod state machine behind them — wrapped in the timing exclusion
    //! this crate owns. A smaller copy used to live in this module, because
    //! `devlaunch-test-support` depended back on `devlaunch-core` and a unit-test
    //! build therefore saw two different `Runner` traits. The trait moved down to
    //! the `devlaunch-runner` leaf crate, so the copy went with it and
    //! `clients::gh`, `clients::ssh` and `clients::git` share the one fake.

    use super::*;
    use crate::runner::EnvBase;
    use crate::testing::ScriptedRunner;
    // `devpod::Call` is this module's own type, so the recorded-spawn type keeps
    // the name the assertions here already give it.
    use devlaunch_test_support::{Call as Recorded, Response};

    // ------------------------------------------------------------ the spawn

    #[test]
    fn every_call_spawns_the_devpod_program_and_nothing_else() {
        let fake = ScriptedRunner::new();
        let call = Call::new(["list", "--output", "json"]);

        let _ = capture(&fake, &call);

        let expected = vec![
            "devpod".to_owned(),
            "list".to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
        ];
        assert_eq!(fake.argvs(), vec![expected.clone()]);
        assert_eq!(call.argv(), expected, "and the call knows its own argv");
    }

    #[test]
    fn a_captured_call_reads_both_streams() {
        let fake = ScriptedRunner::new().with_script(
            ["devpod", "version"],
            Response::stdout("v0.26.1\n").and_stderr("a warning\n"),
        );

        let answer = capture(&fake, &Call::new(["version"])).expect("devpod ran");

        assert_eq!(answer.exit, Exit::Code(0));
        assert_eq!(answer.stdout(), "v0.26.1\n");
        assert_eq!(answer.stderr(), "a warning\n");
    }

    #[test]
    fn an_uncaptured_call_leaves_the_streams_alone() {
        // `devpod up` builds an image for minutes; its progress belongs on the
        // user's terminal as it happens, so the call carries no text at all.
        let fake = ScriptedRunner::new();

        let exit = run(&fake, &Call::new(["up", "/clone"])).expect("devpod ran");

        assert_eq!(exit, Exit::Code(0));
        assert!(matches!(
            fake.calls().first(),
            Some(Recorded::Passthrough(_))
        ));
    }

    #[test]
    fn a_devpod_that_is_not_installed_is_its_own_answer() {
        // Python raises DevpodNotInstalled rather than folding this into a
        // returncode, because a caller branching on the code would carry on as
        // though devpod had answered. The `dl` binary renders it as exit 127.
        let fake = ScriptedRunner::new().with_missing("devpod");

        assert_eq!(
            capture(&fake, &Call::new(["list"])).expect_err("devpod is absent"),
            NotRun::NotInstalled
        );
        assert_eq!(
            run(&fake, &Call::new(["up", "/clone"])).expect_err("devpod is absent"),
            NotRun::NotInstalled
        );
    }

    #[test]
    fn a_devpod_found_but_not_executable_reaches_the_missing_path_not_a_generic_refusal() {
        // A wrapper on PATH whose interpreter is missing execs to ENOENT, which the
        // runner reports as a `NotStarted` carrying `NotFound` rather than
        // `ProgramNotFound`. Python's subprocess raises FileNotFoundError for it
        // exactly as for a devpod not on PATH, and `run_devpod` turns both into
        // DevpodNotInstalled (exit 127). So it must reach `NotInstalled`, not the
        // generic `Blocked` that renders exit 1 (parity F1).
        let exec_enoent = OsFailure {
            kind: std::io::ErrorKind::NotFound,
            errno: Some(libc::ENOENT),
        };
        assert_eq!(
            ran::<()>(Outcome::NotStarted(exec_enoent)),
            Err(NotRun::NotInstalled)
        );

        // A different OS refusal is not the missing path: it stays Blocked and
        // carries its errno for the binary to phrase.
        let denied = OsFailure {
            kind: std::io::ErrorKind::PermissionDenied,
            errno: Some(libc::EACCES),
        };
        assert_eq!(
            ran::<()>(Outcome::NotStarted(denied)),
            Err(NotRun::Blocked(denied))
        );
    }

    #[test]
    fn a_call_that_outstayed_its_timeout_is_not_an_exit_status() {
        let fake = ScriptedRunner::new().with_script(["devpod"], Response::TimedOut);

        assert_eq!(
            capture(&fake, &Call::new(["list"])).expect_err("killed"),
            NotRun::TimedOut
        );
    }

    #[test]
    fn an_os_refusal_keeps_the_errno_it_came_with() {
        let failure = OsFailure {
            kind: std::io::ErrorKind::PermissionDenied,
            errno: Some(13),
        };
        let fake = ScriptedRunner::new().with_script(["devpod"], Response::NotStarted(failure));

        assert_eq!(
            capture(&fake, &Call::new(["list"])).expect_err("never ran"),
            NotRun::Blocked(failure)
        );
    }

    #[test]
    fn a_secret_travels_in_a_replaced_environment_and_never_in_argv() {
        // Python's `env=` replaces devpod's whole environment; `ps` shows argv
        // to every other user on the host, so the token may only go in the
        // environment. `--send-env` names the variable, nothing more.
        let fake = ScriptedRunner::new();
        let call = Call::new(["ssh", "myws", "--send-env", "GH_TOKEN"])
            .with_env(EnvSpec::empty().and("GH_TOKEN", "gho_secret"));

        let _ = run(&fake, &call);

        let recorded = fake.only_call();
        assert!(
            !recorded.argv().iter().any(|arg| arg.contains("gho_secret")),
            "the token must not reach argv: {:?}",
            recorded.argv()
        );
        assert_eq!(recorded.invocation().env.base, EnvBase::Empty);
        assert_eq!(
            recorded
                .invocation()
                .env
                .entries
                .get("GH_TOKEN")
                .map(String::as_str),
            Some("gho_secret")
        );
    }

    #[test]
    fn a_streamed_payload_is_handed_over_as_a_file() {
        // tools.py streams hundreds of megabytes of host binaries into a
        // container over `devpod ssh`; nothing on this path may buffer it.
        let fake = ScriptedRunner::new();
        let call = Call::new(["ssh", "myws", "--command", "tar x"])
            .with_stdin_file("/tmp/devlaunch-tools.tar");

        let _ = run(&fake, &call);

        let recorded = fake.only_call();
        match recorded {
            Recorded::Passthrough(spec) => assert_eq!(
                spec.stdin,
                StdinPlan::File(std::path::PathBuf::from("/tmp/devlaunch-tools.tar"))
            ),
            other => panic!("expected a passthrough call, got {other:?}"),
        }
    }

    #[test]
    fn a_call_may_carry_a_deadline() {
        let fake = ScriptedRunner::new();
        let call = Call::new(["status", "myws"]).with_timeout(Duration::from_secs(5));

        let _ = capture(&fake, &call);

        match fake.only_call() {
            Recorded::Capture(spec) => assert_eq!(spec.timeout, Some(Duration::from_secs(5))),
            other => panic!("expected a captured call, got {other:?}"),
        }
    }

    #[test]
    fn a_call_with_no_deadline_waits_as_long_as_it_takes() {
        let fake = ScriptedRunner::new();

        let _ = run(&fake, &Call::new(["up", "/clone"]));

        match fake.only_call() {
            Recorded::Passthrough(spec) => {
                assert_eq!(spec.timeout, None);
                assert_eq!(spec.stdin, StdinPlan::Inherit);
                assert_eq!(spec.invocation.env, EnvSpec::inherited());
            }
            other => panic!("expected a passthrough call, got {other:?}"),
        }
    }

    #[test]
    fn own_group_is_off_unless_a_call_asks_for_it() {
        assert!(
            !Call::new(["up", "/clone"]).spec().own_group,
            "a plain call stays in this process's group"
        );
        assert!(
            Call::new(["up", "/clone"])
                .leading_its_own_group()
                .spec()
                .own_group,
            "leading_its_own_group threads through to the spec"
        );
    }

    // ------------------------------------------------------- the timing spans
    //
    // The Python `test_timing`'s `TestDevpodRoundTripsAreNamed` asserted these
    // labels through `main()`, where they arrived from Python's one spawn helper.
    // They arrive from this module's three, so this is where they are pinned now;
    // that suite retired with the Python tree (#267).

    /// The span labels `record` produced, in order.
    ///
    /// Drives the process-global registry, so it holds [`timing::exclusive`] for
    /// the length of the measurement.
    fn spans_of(record: impl FnOnce()) -> Vec<String> {
        let _serialized = timing::exclusive();
        timing::install(Some(timing::Registry::start(
            timing::Mode::Prose,
            timing::Seam::default(),
            0.0,
        )));
        record();
        match timing::emit().expect("a report from an installed registry") {
            timing::Report::Prose(prose) => {
                prose.spans.iter().map(|span| span.label.clone()).collect()
            }
            other => panic!("asked for the prose summary, got {other:?}"),
        }
    }

    #[test]
    fn every_devpod_round_trip_is_named_by_its_subcommand() {
        // Python spans each call as `" ".join(cmd[:2])`, and all three shapes of
        // call go through it: captured, inherited, and the session.
        let fake = ScriptedRunner::new()
            .with_script(["devpod", "list"], Response::stdout("[]"))
            .with_script(
                ["devpod", "status"],
                Response::stdout(r#"{"state": "Running"}"#),
            );

        let labels = spans_of(|| {
            let _ = list_workspaces(&fake);
            let _ = status(&fake, "myws", Patience::AsLongAsItTakes);
            let _ = run(&fake, &Call::new(["up", "myws", "--ide", "none"]));
            let _ = session(&fake, &Call::new(["ssh", "myws"]), &mut |_| {});
        });

        assert_eq!(
            labels,
            ["devpod list", "devpod status", "devpod up", "devpod ssh"]
        );
    }

    #[test]
    fn the_label_is_the_subcommand_and_never_what_follows_it() {
        // A workspace id in a summary is what the `cmd[:2]` slice exists to keep
        // out of it.
        assert_eq!(
            Call::new(["status", "myws", "--output", "json"]).round_trip(),
            "devpod status"
        );
        assert_eq!(
            Call::new(["context", "options", "--output", "json"]).round_trip(),
            "devpod context"
        );
        assert_eq!(
            Call::new(["provider", "add", "docker"]).round_trip(),
            "devpod provider"
        );
        assert_eq!(
            Call::new(Vec::<String>::new()).round_trip(),
            "devpod",
            "a call with no subcommand is named by the slice it has"
        );
    }

    #[test]
    fn a_round_trip_devpod_never_answered_is_still_named_and_timed() {
        // A spawn that failed still took time, and dropping it would make the
        // parts add up to less than the total.
        let fake = ScriptedRunner::new().with_missing("devpod");

        let labels = spans_of(|| {
            let _ = list_workspaces(&fake);
        });

        assert_eq!(labels, ["devpod list"]);
    }

    // ------------------------------------ how a session ended (devpod_ssh.py)

    /// The two lines a normal `exit` produced, verbatim from the report that
    /// prompted `devpod_ssh` — devpod colours its log lines unconditionally, so
    /// the escapes are part of what has to be read. 130 is a shell exiting with
    /// the status of a Ctrl-C'd last command.
    const DEBUG_HINT_LINE: &str = concat!(
        "\x1b[97;1m20:41:27 \x1b[0m\x1b[91;1merror \x1b[0m",
        "Try using the --debug flag to see a more verbose output       root.go:106",
    );
    const REMOTE_EXIT_LINE: &str = concat!(
        "\x1b[97;1m20:41:27 \x1b[0m\x1b[91;1mfatal \x1b[0m",
        "tunnel to container: run in container: ssh session: ",
        "Process exited with status 130                                root.go:113",
    );

    /// Run the stderr filter over `lines`, as a session would: the recovered
    /// status, and what reached the user.
    fn filter(lines: &[&str]) -> (Option<i32>, Vec<String>) {
        let mut shown = Vec::new();
        let mut filter = StderrFilter::new();
        {
            let mut forward = |line: &str| shown.push(line.to_owned());
            for line in lines {
                filter.push(line, &mut forward);
            }
            let status = filter.finish(&mut forward);
            (status, shown)
        }
    }

    #[test]
    fn exiting_a_shell_is_silent_and_keeps_the_shells_status() {
        let (status, shown) = filter(&[DEBUG_HINT_LINE, REMOTE_EXIT_LINE]);

        assert_eq!(status, Some(130));
        assert!(shown.is_empty(), "nothing has gone wrong: {shown:?}");
        assert_eq!(
            interpret(Exit::Code(1), status),
            SshOutcome::RemoteExit { status: 130 }
        );
    }

    #[test]
    fn a_signal_report_still_yields_the_status() {
        // x/crypto appends the signal when the remote process was killed by one.
        let line = concat!(
            "20:41:27 fatal tunnel to container: run in container: ssh session: ",
            "Process exited with status 130 from signal SIGINT root.go:113",
        );

        let (status, shown) = filter(&[DEBUG_HINT_LINE, line]);

        assert_eq!(status, Some(130));
        assert!(shown.is_empty());
    }

    #[test]
    fn a_clean_exit_reports_zero() {
        assert_eq!(
            interpret(Exit::Code(0), None),
            SshOutcome::RemoteExit { status: 0 }
        );
    }

    #[test]
    fn the_recovered_status_beats_devpods_own_exit_code() {
        // devpod exits 1 next to every remote status, so 1 is never the answer.
        assert_eq!(
            interpret(Exit::Code(1), Some(2)),
            SshOutcome::RemoteExit { status: 2 }
        );
    }

    #[test]
    fn a_genuine_failure_keeps_both_lines_and_their_order() {
        // The hint is held to see what follows it, so it must not end up after.
        let fatal = "20:41:27 fatal tunnel to container: dial tcp: connection refused";

        let (status, shown) = filter(&[DEBUG_HINT_LINE, fatal]);

        assert_eq!(status, None);
        assert_eq!(shown, vec![DEBUG_HINT_LINE.to_owned(), fatal.to_owned()]);
        assert_eq!(
            interpret(Exit::Code(1), status),
            SshOutcome::DevpodFailed {
                exit: Exit::Code(1)
            }
        );
    }

    #[test]
    fn a_trailing_hint_with_nothing_after_it_is_still_released() {
        let (status, shown) = filter(&[DEBUG_HINT_LINE]);

        assert_eq!(status, None);
        assert_eq!(shown, vec![DEBUG_HINT_LINE.to_owned()]);
    }

    #[test]
    fn unrelated_stderr_passes_through_untouched() {
        let lines = [
            "20:41:27 warn workspace is already running",
            "some remote stderr",
        ];

        let (status, shown) = filter(&lines);

        assert_eq!(status, None);
        assert_eq!(shown, lines.map(str::to_owned).to_vec());
    }

    #[test]
    fn a_lost_session_is_a_devpod_failure() {
        // x/crypto's ExitMissingError carries no status, and is a real failure.
        let line = concat!(
            "20:41:27 fatal tunnel to container: run in container: ssh session: ",
            "wait: remote command exited without exit status or exit signal",
        );

        let (status, shown) = filter(&[line]);

        assert_eq!(status, None);
        assert_eq!(shown, vec![line.to_owned()]);
    }

    #[test]
    fn a_remote_program_printing_the_same_sentence_is_not_devpods_report() {
        // Without a pty the remote program's own stderr arrives on this stream,
        // so the report is anchored on devpod's own `fatal` tag as well.
        let line = "Process exited with status 7";

        let (status, shown) = filter(&[line]);

        assert_eq!(status, None);
        assert_eq!(shown, vec![line.to_owned()]);
    }

    #[test]
    fn a_status_too_large_for_an_exit_code_is_not_read_as_one() {
        // Nothing can exit with it, so it is not devpod reporting a status;
        // forwarding the line unchanged tells the user what devpod said.
        let line = "20:41:27 fatal ssh session: Process exited with status 99999999999999999999";

        let (status, shown) = filter(&[line]);

        assert_eq!(status, None);
        assert_eq!(shown, vec![line.to_owned()]);
    }

    #[test]
    fn devpod_dying_of_a_signal_is_a_devpod_failure_carrying_the_signal() {
        // Python folds this into a negative returncode; the ending itself is a
        // different fact from an exit status, and stays one.
        assert_eq!(
            interpret(Exit::Signal(15), None),
            SshOutcome::DevpodFailed {
                exit: Exit::Signal(15)
            }
        );
    }

    #[test]
    fn a_session_holds_back_the_report_it_recovers_and_answers_with_it() {
        let fake = ScriptedRunner::new().with_script(
            ["devpod", "ssh"],
            Response::exited(1).and_stderr(format!("{DEBUG_HINT_LINE}\n{REMOTE_EXIT_LINE}\n")),
        );
        let mut shown = Vec::new();

        let outcome = session(&fake, &Call::new(["ssh", "myws"]), &mut |line| {
            shown.push(line.to_owned())
        })
        .expect("devpod ran");

        assert_eq!(outcome, SshOutcome::RemoteExit { status: 130 });
        assert!(shown.is_empty(), "{shown:?}");
        assert!(matches!(fake.calls().first(), Some(Recorded::Session(_))));
    }

    #[test]
    fn a_session_devpod_could_not_start_is_not_an_outcome() {
        let fake = ScriptedRunner::new().with_missing("devpod");

        assert_eq!(
            session(&fake, &Call::new(["ssh", "myws"]), &mut |_| {}).expect_err("devpod is absent"),
            NotRun::NotInstalled
        );
    }

    // ------------------------------------------------------ container state

    /// `devpod status <id> --output json` for a running workspace, in devpod's
    /// own shape.
    const RUNNING_STATUS: &str = concat!(
        r#"{"id":"myws","context":"default","provider":"docker","#,
        r#""state":"Running"}"#,
        "\n",
    );

    #[test]
    fn the_status_of_a_workspace_is_asked_for_in_json() {
        let fake = ScriptedRunner::new()
            .with_script(["devpod", "status"], Response::stdout(RUNNING_STATUS));

        let state = status(&fake, "myws", Patience::AsLongAsItTakes).expect("devpod answered");

        assert_eq!(state, ContainerState::Running);
        assert_eq!(
            fake.args_to("devpod"),
            vec![vec![
                "status".to_owned(),
                "myws".to_owned(),
                "--output".to_owned(),
                "json".to_owned(),
            ]]
        );
    }

    #[test]
    fn every_word_devpod_has_for_a_container_is_a_state_of_its_own() {
        for (word, expected) in [
            ("Running", ContainerState::Running),
            ("Busy", ContainerState::Busy),
            ("Stopped", ContainerState::Stopped),
            ("NotFound", ContainerState::NotFound),
        ] {
            let fake = ScriptedRunner::new().with_script(
                ["devpod", "status"],
                Response::stdout(format!("{{\"state\":\"{word}\"}}\n")),
            );

            assert_eq!(
                status(&fake, "myws", Patience::AsLongAsItTakes),
                Ok(expected)
            );
        }
    }

    #[test]
    fn a_word_this_build_has_never_heard_is_still_a_state() {
        // Total over devpod's future: a new word is data, not a parse failure,
        // and a reader asking "is it Running" gets the same answer either way.
        let fake = ScriptedRunner::new().with_script(
            ["devpod", "status"],
            Response::stdout("{\"state\":\"Hibernating\"}\n"),
        );

        assert_eq!(
            status(&fake, "myws", Patience::AsLongAsItTakes),
            Ok(ContainerState::Unknown("Hibernating".to_owned()))
        );
    }

    #[test]
    fn a_devpod_that_refused_the_question_has_not_answered_it() {
        // Python returns None here and callers read that as "devpod knows no
        // such workspace"; the refusal is kept apart from a state so a flow
        // decides that rather than inheriting it from a parser.
        let fake = ScriptedRunner::new().with_script(
            ["devpod", "status"],
            Response::failed(
                1,
                "devpod: couldn't find workspace never-heard-of-it\n".to_owned(),
            ),
        );

        let refused = status(&fake, "never-heard-of-it", Patience::AsLongAsItTakes)
            .expect_err("no such workspace");

        match refused {
            StatusUnreadable::Failed { exit, stderr } => {
                assert_eq!(exit, Exit::Code(1));
                assert!(stderr.contains("never-heard-of-it"), "{stderr:?}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn status_output_that_is_not_json_is_unreadable_rather_than_a_state() {
        let fake = ScriptedRunner::new().with_script(
            ["devpod", "status"],
            Response::stdout("Error: no context\n"),
        );

        match status(&fake, "myws", Patience::AsLongAsItTakes).expect_err("not readable") {
            StatusUnreadable::NotJson { output, reason } => {
                assert_eq!(output, "Error: no context\n");
                assert!(!reason.is_empty(), "the parser said something");
            }
            other => panic!("expected unreadable JSON, got {other:?}"),
        }
    }

    #[test]
    fn status_json_with_no_state_in_it_is_unreadable() {
        let fake = ScriptedRunner::new().with_script(
            ["devpod", "status"],
            Response::stdout("{\"id\":\"myws\"}\n"),
        );

        match status(&fake, "myws", Patience::AsLongAsItTakes).expect_err("not readable") {
            StatusUnreadable::NoState { output } => assert!(output.contains("myws")),
            other => panic!("expected a missing state, got {other:?}"),
        }
    }

    // ------------------------------------------------- the workspace listing

    /// Two workspaces as devpod really lists them: **no state field**, because
    /// real devpod answers state only to `status`, per workspace.
    const TWO_WORKSPACES: &str = concat!(
        r#"[{"id":"ws1","source":{"localFolder":"/home/dev/.cache/devlaunch/repos/o/r/ws1"},"#,
        r#""lastUsed":"2026-08-08T11:43:27Z","provider":{"name":"docker"},"#,
        r#""ide":{"name":"none"},"context":"default"},"#,
        r#"{"id":"ws2","source":{"gitRepository":"github.com/blooop/devlaunch"},"#,
        r#""lastUsed":"2026-08-08T11:44:01Z","provider":{"name":"docker"},"#,
        r#""ide":{"name":"openvscode"},"context":"work"}]"#,
        "\n",
    );

    /// One listing element with `source`, parsed the way a listing parses it.
    fn listed(source: &str) -> Workspace {
        let listing = format!(
            r#"[{{"id":"ws","source":{source},"lastUsed":"2026-08-08T11:43:27Z",
                 "provider":{{"name":"docker"}},"ide":{{"name":"none"}}}}]"#
        );
        let mut workspaces = parse_workspaces(&listing).expect("a readable listing");
        assert_eq!(workspaces.len(), 1);
        workspaces.remove(0)
    }

    fn source_of(source: &str) -> WorkspaceSource {
        listed(source).source
    }

    fn payload(pairs: &str) -> serde_json::Map<String, serde_json::Value> {
        match serde_json::from_str(pairs).expect("an object") {
            serde_json::Value::Object(map) => map,
            other => panic!("not an object: {other:?}"),
        }
    }

    #[test]
    fn the_listing_is_asked_for_in_json_and_read_back() {
        let fake =
            ScriptedRunner::new().with_script(["devpod", "list"], Response::stdout(TWO_WORKSPACES));

        let workspaces = list_workspaces(&fake).expect("a readable listing");

        assert_eq!(
            fake.args_to("devpod"),
            vec![vec![
                "list".to_owned(),
                "--output".to_owned(),
                "json".to_owned()
            ]]
        );
        assert_eq!(
            workspaces
                .iter()
                .map(|ws| ws.id.clone())
                .collect::<Vec<_>>(),
            vec!["ws1".to_owned(), "ws2".to_owned()]
        );
        assert_eq!(workspaces[1].provider, "docker");
        assert_eq!(workspaces[1].ide, "openvscode");
        assert_eq!(workspaces[1].context, "work");
        assert_eq!(workspaces[1].last_used, "2026-08-08T11:44:01Z");
    }

    #[test]
    fn a_machine_with_no_workspaces_lists_none() {
        // The distinction cuts both ways: an answer that reads fine and says
        // nothing is listed still answers, and answers with an empty list.
        assert_eq!(parse_workspaces("[]\n"), Ok(Vec::new()));
    }

    #[test]
    fn a_devpod_that_said_nothing_has_not_said_there_are_none() {
        // devpod prints `[]` for a machine with no workspaces, so silence is
        // devpod failing to answer. Its own arm rather than a parse failure,
        // whose report — `not JSON: ''` — reads like a bug in dl.
        for silence in ["", "   \n", "\n\n"] {
            assert_eq!(
                parse_workspaces(silence),
                Err(NotAListing::Silence),
                "{silence:?}"
            );
        }
    }

    #[test]
    fn output_that_is_not_json_is_reported_as_such() {
        match parse_workspaces("Error: no context selected\n").expect_err("not a listing") {
            NotAListing::NotJson { output, reason } => {
                assert_eq!(output, "Error: no context selected\n");
                assert!(!reason.is_empty(), "the parser said something");
            }
            other => panic!("expected unreadable JSON, got {other:?}"),
        }
    }

    #[test]
    fn json_of_the_wrong_shape_is_not_a_listing() {
        // `{"workspaces": []}` is JSON a `for entry in parsed` loop would walk
        // happily — it yields the object's keys — so the top-level shape is
        // checked in its own right and the failure names what came instead.
        for (listing, kind) in [
            (r#"{"workspaces": []}"#, JsonKind::Object),
            ("null", JsonKind::Null),
            ("42", JsonKind::Number),
            (r#""ws1""#, JsonKind::String),
            ("true", JsonKind::Bool),
        ] {
            assert_eq!(
                parse_workspaces(listing),
                Err(NotAListing::NotAnArray { kind }),
                "{listing}"
            );
        }
    }

    #[test]
    fn entries_that_are_not_workspaces_are_reported() {
        assert_eq!(
            parse_workspaces(r#"["ws1", "ws2"]"#),
            Err(NotAListing::EntryNotAnObject {
                kind: JsonKind::String
            })
        );
    }

    #[test]
    fn a_source_that_is_not_an_object_at_all_is_an_unreadable_listing() {
        // The source arms are total over the object devpod documents, and the
        // arm for a source dl cannot read *holds that object* — so something
        // that is not one is not an unreadable source but an unreadable listing,
        // which is an answer this parser already knows how to give.
        for (source, kind) in [
            (r#""/home/dev/project""#, JsonKind::String),
            ("7", JsonKind::Number),
            (r#"["localFolder"]"#, JsonKind::Array),
        ] {
            let listing = format!(r#"[{{"id":"odd","source":{source}}}]"#);
            assert_eq!(
                parse_workspaces(&listing),
                Err(NotAListing::SourceNotAnObject {
                    workspace_id: "odd".to_owned(),
                    kind
                }),
                "{listing}"
            );
        }
    }

    #[test]
    fn a_string_source_is_not_read_as_a_local_folder_by_accident() {
        // `"localFolder" in some_string` is a substring test, so a string source
        // mentioning the key was one indexing error away from being a folder.
        let listing = r#"[{"id":"odd","source":"/srv/localFolder/x"}]"#;

        assert!(matches!(
            parse_workspaces(listing),
            Err(NotAListing::SourceNotAnObject { .. })
        ));
    }

    #[test]
    fn a_listing_devpod_refused_to_produce_is_not_an_empty_list() {
        // devpod exiting nonzero is not an answer. Its stderr travels whole:
        // devpod's own explanation is the only thing that tells anyone what to
        // do about it, and keeping the report to one line is rendering.
        let fake = ScriptedRunner::new().with_script(
            ["devpod", "list"],
            Response::failed(1, "Error: line one\nline two\nline three\n"),
        );

        match list_workspaces(&fake).expect_err("not an answer") {
            ListingUnreadable::Failed { exit, stderr } => {
                assert_eq!(exit, Exit::Code(1));
                assert!(stderr.contains("line three"), "{stderr:?}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_listing_no_devpod_could_produce_is_not_an_empty_list_either() {
        let fake = ScriptedRunner::new().with_missing("devpod");

        assert_eq!(
            list_workspaces(&fake).expect_err("devpod is absent"),
            ListingUnreadable::NotRun(NotRun::NotInstalled)
        );
    }

    // --------------------------------------------- what a source actually is

    #[test]
    fn a_local_folder_source_is_a_path() {
        assert_eq!(
            source_of(r#"{"localFolder":"/home/dev/myproject"}"#),
            WorkspaceSource::LocalFolder("/home/dev/myproject".to_owned())
        );
    }

    #[test]
    fn a_git_source_is_a_url() {
        assert_eq!(
            source_of(r#"{"gitRepository":"github.com/blooop/devlaunch"}"#),
            WorkspaceSource::GitRepository("github.com/blooop/devlaunch".to_owned())
        );
    }

    #[test]
    fn a_source_devlaunch_cannot_read_keeps_what_devpod_sent() {
        // The field this replaced was typed as a path and filled with a
        // rendering of the object, so the one thing a caller could not do with
        // it was read what devpod had said. Indexing it is the assertion.
        let source = source_of(r#"{"image":"ubuntu:24.04"}"#);

        match source {
            WorkspaceSource::Unrecognised(payload) => assert_eq!(
                payload.get("image").and_then(serde_json::Value::as_str),
                Some("ubuntu:24.04")
            ),
            other => panic!("expected an unrecognised source, got {other:?}"),
        }
    }

    #[test]
    fn a_workspace_devpod_listed_with_no_source_at_all_is_unreadable() {
        let mut workspaces = parse_workspaces(r#"[{"id":"minimal"}]"#).expect("a listing");
        let workspace = workspaces.remove(0);

        assert_eq!(workspace.id, "minimal");
        assert_eq!(
            workspace.source,
            WorkspaceSource::Unrecognised(serde_json::Map::new())
        );
    }

    #[test]
    fn a_key_devpod_left_empty_is_not_a_readable_source() {
        // The empty string is the same defect one level down: a sentinel living
        // *inside* the arm that means "a folder devlaunch can read". `git -C ""`
        // is a no-op that succeeds, so an empty folder would be credited with
        // whatever repository the person running `dl` was standing in.
        for source in [r#"{"localFolder":""}"#, r#"{"gitRepository":""}"#] {
            assert_eq!(
                source_of(source),
                WorkspaceSource::Unrecognised(payload(source)),
                "{source}"
            );
        }
    }

    #[test]
    fn a_git_url_that_is_not_text_is_not_a_readable_source() {
        for source in [
            r#"{"gitRepository":{"str_of_a_dict":"is back"}}"#,
            r#"{"gitRepository":7}"#,
            r#"{"gitRepository":["/home/dev"]}"#,
            r#"{"gitRepository":null}"#,
        ] {
            assert_eq!(
                source_of(source),
                WorkspaceSource::Unrecognised(payload(source)),
                "{source}"
            );
        }
    }

    #[test]
    fn a_folder_devpod_named_and_devlaunch_cannot_read_is_its_own_arm() {
        // The same rejection, and *not* the same answer: a `localFolder` this
        // unreadable is still devpod saying the workspace opens a directory on
        // this machine. `--prune` reads "no folder here" as nothing to compare,
        // which is right for an image workspace and wrong for this.
        for source in [
            r#"{"localFolder":{"str_of_a_dict":"is back"}}"#,
            r#"{"localFolder":7}"#,
            r#"{"localFolder":["/home/dev"]}"#,
        ] {
            assert_eq!(
                source_of(source),
                WorkspaceSource::UnreadableLocalFolder(payload(source)),
                "{source}"
            );
        }
    }

    #[test]
    fn a_folder_key_devpod_left_null_claims_no_folder_at_all() {
        // `null` is devpod saying there is no local folder, which is what an
        // image or container workspace has — not a folder that could not be read.
        let source = r#"{"localFolder":null}"#;

        assert_eq!(
            source_of(source),
            WorkspaceSource::Unrecognised(payload(source))
        );
    }

    #[test]
    fn a_source_carrying_both_keys_is_read_as_a_folder() {
        // Key precedence, pinned: a reader that misses an arm is a build
        // failure, but this is a *producer*, and a producer that quietly stops
        // producing an arm compiles fine. This is the one link in the chain the
        // compiler cannot stand in for a test.
        assert_eq!(
            source_of(r#"{"localFolder":"/home/dev/myproject","gitRepository":"github.com/o/r"}"#),
            WorkspaceSource::LocalFolder("/home/dev/myproject".to_owned())
        );
    }

    #[test]
    fn a_workspace_devpod_did_not_place_in_a_context_is_in_the_default_one() {
        // Ids are unique per context, so the context is half a workspace's
        // address on disk; a listing that omits it must still parse.
        for (entry, context) in [
            (r#"{"id":"ws"}"#, "default"),
            (r#"{"id":"ws","context":null}"#, "default"),
            (r#"{"id":"ws","context":""}"#, "default"),
            (r#"{"id":"ws","context":"work"}"#, "work"),
        ] {
            let listing = format!("[{entry}]");
            let workspaces = parse_workspaces(&listing).expect("a listing");
            assert_eq!(workspaces[0].context, context, "{entry}");
        }
    }

    #[test]
    fn a_listing_that_names_no_provider_or_ide_still_parses() {
        // Python reads `data.get("provider", {}).get("name", "")`, which raises
        // if devpod ever sends something else there; a listing dl cannot fully
        // read is still a listing, and the id is what every caller needs.
        let listing = r#"[{"id":"ws","provider":7,"ide":null}]"#;

        let workspaces = parse_workspaces(listing).expect("a listing");

        assert_eq!(workspaces[0].provider, "");
        assert_eq!(workspaces[0].ide, "");
    }

    // --------------------------------------------------------- the providers

    /// `devpod provider list --output json` on a host where `docker` is
    /// registered — the real recording the Python suite reads, byte for byte.
    /// A hand-written approximation is how the colour defect below survives.
    const PROVIDER_LIST: &str = include_str!("../../../../test/fixtures/devpod/provider_list.json");

    /// The same command against a devpod home that has never had one added.
    const NO_PROVIDERS: &str =
        include_str!("../../../../test/fixtures/devpod/provider_list_empty.json");

    /// The same command with no `--output`: the human table, with colour.
    const COLOURISED_TABLE: &str =
        include_str!("../../../../test/fixtures/devpod/provider_list_plain.ansi");

    fn names(of: &[&str]) -> BTreeSet<String> {
        of.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn the_recordings_are_the_real_thing() {
        // Guard the fixtures themselves: the table really is colourised, and the
        // character before `docker` really is a word character, which is the
        // whole reason a text match on it cannot be trusted.
        assert!(COLOURISED_TABLE.contains('\x1b'));
        let at = COLOURISED_TABLE
            .find("docker")
            .expect("docker is in the table");
        assert_eq!(&COLOURISED_TABLE[at - 1..at], "m");
    }

    #[test]
    fn a_registered_provider_is_reported_present() {
        assert_eq!(parse_provider_names(PROVIDER_LIST), Ok(names(&["docker"])));
    }

    #[test]
    fn an_empty_listing_reports_no_providers() {
        assert_eq!(parse_provider_names(NO_PROVIDERS), Ok(BTreeSet::new()));
    }

    #[test]
    fn the_colourised_table_is_not_mistaken_for_an_empty_listing() {
        // A listing dl could not read is not a listing with nothing in it. The
        // original guard collapsed those two into "absent" and tried to add a
        // provider that was already there.
        assert!(matches!(
            parse_provider_names(COLOURISED_TABLE),
            Err(NotAProviderListing::NotJson { .. })
        ));
    }

    #[test]
    fn a_listing_that_is_not_keyed_by_name_is_unreadable() {
        assert_eq!(
            parse_provider_names(r#"["docker"]"#),
            Err(NotAProviderListing::NotKeyedByName {
                kind: JsonKind::Array
            })
        );
    }

    #[test]
    fn the_listing_is_requested_in_machine_readable_form() {
        // The guard must never be handed the coloured table in the first place.
        let fake = ScriptedRunner::new().with_script(
            ["devpod", "provider", "list"],
            Response::stdout(PROVIDER_LIST),
        );

        let registered = provider_names(&fake).expect("a readable listing");

        assert_eq!(registered, names(&["docker"]));
        assert_eq!(
            fake.argvs(),
            vec![vec![
                "devpod".to_owned(),
                "provider".to_owned(),
                "list".to_owned(),
                "--output".to_owned(),
                "json".to_owned(),
            ]]
        );
    }

    #[test]
    fn an_existing_provider_is_not_added_again() {
        let fake = ScriptedRunner::new().with_script(
            ["devpod", "provider", "list"],
            Response::stdout(PROVIDER_LIST),
        );

        assert_eq!(
            ensure_provider(&fake, "docker"),
            Ok(ProviderRegistration::AlreadyRegistered)
        );
        assert_eq!(fake.call_count(), 1, "nothing was added");
    }

    #[test]
    fn a_missing_provider_is_added() {
        let fake = ScriptedRunner::new().with_script(
            ["devpod", "provider", "list"],
            Response::stdout(NO_PROVIDERS),
        );

        assert_eq!(
            ensure_provider(&fake, "docker"),
            Ok(ProviderRegistration::Added)
        );
        assert_eq!(
            fake.args_to("devpod"),
            vec![
                vec![
                    "provider".to_owned(),
                    "list".to_owned(),
                    "--output".to_owned(),
                    "json".to_owned()
                ],
                vec!["provider".to_owned(), "add".to_owned(), "docker".to_owned()],
            ]
        );
    }

    #[test]
    fn an_unreadable_listing_stops_the_guard_rather_than_being_acted_on() {
        let fake = ScriptedRunner::new().with_script(
            ["devpod", "provider", "list"],
            Response::stdout(COLOURISED_TABLE),
        );

        assert!(matches!(
            ensure_provider(&fake, "docker"),
            Err(EnsureProviderFailed::ListUnreadable(_))
        ));
        assert_eq!(fake.call_count(), 1, "nothing was added");
    }

    #[test]
    fn a_failed_listing_is_reported_rather_than_swallowed() {
        let fake = ScriptedRunner::new().with_script(
            ["devpod", "provider", "list"],
            Response::failed(1, "boom\n"),
        );

        match ensure_provider(&fake, "docker").expect_err("not readable") {
            EnsureProviderFailed::ListUnreadable(ProviderListUnreadable::Failed {
                exit,
                stderr,
            }) => {
                assert_eq!(exit, Exit::Code(1));
                assert_eq!(stderr, "boom\n");
            }
            other => panic!("expected a failed listing, got {other:?}"),
        }
        assert_eq!(fake.call_count(), 1, "nothing was added");
    }

    #[test]
    fn a_failed_add_reports_what_devpod_said() {
        // The listing failure next door quotes devpod's stderr, and this one
        // used not to — so the one failure a user can act on was the quiet one.
        let fake = ScriptedRunner::new()
            .with_script(
                ["devpod", "provider", "list"],
                Response::stdout(NO_PROVIDERS),
            )
            .with_script(
                ["devpod", "provider", "add"],
                Response::failed(1, "provider docker already exists\n"),
            );

        match ensure_provider(&fake, "docker").expect_err("the add failed") {
            EnsureProviderFailed::AddFailed(AddFailed::Refused { exit, stderr }) => {
                assert_eq!(exit, Exit::Code(1));
                assert_eq!(stderr, "provider docker already exists\n");
            }
            other => panic!("expected a refused add, got {other:?}"),
        }
    }

    #[test]
    fn both_provider_calls_are_captured_so_devpods_words_survive() {
        // Quoting devpod's stderr means capturing it: without capture there is
        // nothing on the result to quote.
        let fake = ScriptedRunner::new().with_script(
            ["devpod", "provider", "list"],
            Response::stdout(NO_PROVIDERS),
        );

        let _ = ensure_provider(&fake, "docker");

        for call in fake.calls() {
            assert!(
                matches!(call, Recorded::Capture(_)),
                "not captured: {call:?}"
            );
        }
    }

    #[test]
    fn a_provider_guard_with_no_devpod_to_ask_says_so() {
        let fake = ScriptedRunner::new().with_missing("devpod");

        assert_eq!(
            ensure_provider(&fake, "docker"),
            Err(EnsureProviderFailed::ListUnreadable(
                ProviderListUnreadable::NotRun(NotRun::NotInstalled)
            ))
        );
    }

    // ---------------------------------------------------- the context options

    #[test]
    fn the_context_options_are_asked_for_in_json() {
        let fake = ScriptedRunner::new().with_script(
            ["devpod", "context", "options"],
            Response::stdout(
                r#"{"DOTFILES_URL":{"value":"https://github.com/o/dotfiles"},
                    "DOTFILES_SCRIPT":{"value":""},
                    "SSH_INJECT_DOCKER_CREDENTIALS":{},
                    "AGENT_INJECT_GIT_CREDENTIALS":{"value":"true"}}"#,
            ),
        );

        let options = context_options(&fake).expect("readable options");

        assert_eq!(
            fake.args_to("devpod"),
            vec![vec![
                "context".to_owned(),
                "options".to_owned(),
                "--output".to_owned(),
                "json".to_owned(),
            ]]
        );
        assert_eq!(
            options,
            BTreeMap::from([
                (
                    "DOTFILES_URL".to_owned(),
                    "https://github.com/o/dotfiles".to_owned()
                ),
                ("AGENT_INJECT_GIT_CREDENTIALS".to_owned(), "true".to_owned()),
            ]),
            "an option with no value set is not an option devpod is applying"
        );
    }

    #[test]
    fn a_devpod_that_named_no_options_has_named_no_options() {
        // Unlike the workspace listing, silence here is not a refusal: an option
        // dl does not find is simply one it does not pass on, where a listing it
        // could not read decides what gets deleted. Python answers `{}` for
        // both silence and a failure; the failure keeps its arm here so a flow
        // can say so, and defaults to the same empty map.
        let fake = ScriptedRunner::new()
            .with_script(["devpod", "context", "options"], Response::stdout("   \n"));

        assert_eq!(context_options(&fake), Ok(BTreeMap::new()));
    }

    #[test]
    fn context_options_that_are_not_an_object_are_unreadable() {
        let fake = ScriptedRunner::new()
            .with_script(["devpod", "context", "options"], Response::stdout("[1,2]"));

        assert_eq!(
            context_options(&fake),
            Err(OptionsUnreadable::NotKeyedByOption {
                kind: JsonKind::Array
            })
        );
    }

    #[test]
    fn an_option_whose_body_is_not_an_object_is_unreadable_not_an_empty_answer() {
        // Python's `v.get("value")` raises AttributeError on `3`, so dl returns {}
        // WITHOUT caching and re-asks next run. Returning Ok({}) here would let the
        // flow write an empty cache and trust it for the whole TTL (P9).
        let fake = ScriptedRunner::new().with_script(
            ["devpod", "context", "options"],
            Response::stdout(r#"{"FOO": 3}"#),
        );

        assert_eq!(
            context_options(&fake),
            Err(OptionsUnreadable::OptionNotAnObject {
                option: "FOO".to_owned(),
                kind: JsonKind::Number,
            })
        );
    }

    #[test]
    fn context_options_that_are_not_json_are_unreadable() {
        let fake = ScriptedRunner::new().with_script(
            ["devpod", "context", "options"],
            Response::stdout("Error: no context selected\n"),
        );

        assert!(matches!(
            context_options(&fake),
            Err(OptionsUnreadable::NotJson { .. })
        ));
    }

    #[test]
    fn a_devpod_that_refused_the_options_has_not_answered_them() {
        let fake = ScriptedRunner::new().with_script(
            ["devpod", "context", "options"],
            Response::failed(1, "context not found\n"),
        );

        assert_eq!(
            context_options(&fake),
            Err(OptionsUnreadable::Failed {
                exit: Exit::Code(1),
                stderr: "context not found\n".to_owned()
            })
        );
    }

    #[test]
    fn a_status_no_devpod_could_answer_says_so() {
        let fake = ScriptedRunner::new().with_missing("devpod");

        assert_eq!(
            status(&fake, "myws", Patience::AsLongAsItTakes).expect_err("devpod is absent"),
            StatusUnreadable::NotRun(NotRun::NotInstalled)
        );
    }
}
