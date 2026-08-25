//! The one seam to the outside: a spawn spec in, an outcome out.
//!
//! Every process devlaunch starts — `devpod`, `git`, `gh`, `ssh` — goes through
//! this module, and nothing above it calls [`std::process`]. That is what makes
//! the tool clients above testable without a PATH shim (the fake in
//! `devlaunch-test-support` implements [`Runner`], and the argv every flow
//! issues is visible to a test), and it is what makes "devpod is not installed"
//! a fact this layer can state once rather than a guess each caller makes.
//!
//! # The four exotics
//!
//! Python's `dl.py` spawns processes four ways, and the differences are
//! load-bearing rather than incidental. They are the four methods of [`Runner`]:
//!
//! - [`Runner::capture`] — both streams read as text. `git`'s answers,
//!   `gh auth token`, `devpod list --output json`: the output *is* the answer
//!   the caller branches on. Carries a per-call timeout, because a `git fetch`
//!   against a hung remote must cost one pass rather than the session.
//! - [`Runner::passthrough`] — both streams inherited. `devpod up` builds an
//!   image for minutes and its progress belongs on the user's terminal as it
//!   happens. Nothing is captured, and the outcome says so by carrying no text
//!   at all rather than a pair of empty strings.
//! - [`Runner::session`] — stdin and stdout inherited, stderr read line by line
//!   as it arrives. The terminal-session case: devpod puts the real terminal
//!   into raw mode through stdin/stdout and asks for a pty on that basis, so a
//!   pipe on either changes what devpod does; only stderr is read, so devpod's
//!   report of how the session ended can be interpreted instead of dumped on
//!   the user. Lines reach the caller while the session is still running.
//! - [`Runner::detach`] — a new session (`setsid`) with null stdio, never waited
//!   for. The background completion refresh: it outlives the `dl` that started
//!   it, and a Ctrl-C in the shell that started `dl` must not reach it.
//!
//! The fifth exotic is a field rather than a method: [`StdinPlan::File`] hands
//! an open descriptor to the child, because the payload `tools.py` streams into
//! a container runs to hundreds of megabytes that have no business in this
//! process's memory.
//!
//! # What is deliberately absent
//!
//! Python wraps each of these spawns in a `timing.span`. That does not live here
//! — the span names a round trip (`devpod status`, `devpod ssh`), which is the
//! tool client's idea of what it was doing, not this layer's; a span per spawn
//! here would name argv fragments and would time the `git` calls Python does not
//! time. The registry is `timing`'s, and the spans are the callers'.
//!
//! # No English
//!
//! Nothing here holds a message meant for a person (#251 §5). An outcome
//! carries what the child wrote, how it ended, and — when it never ran — an
//! [`std::io::ErrorKind`] and an errno. Turning any of that into a diagnostic
//! and an exit code is the `dl` binary's rendering.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

pub mod interrupt;

#[cfg(test)]
mod tests;

/// Whether the parent's environment is the starting point, or nothing is.
///
/// [`EnvBase::Empty`] is what Python's `env=` does — the given mapping replaces
/// the whole environment — and it exists so a secret can be handed to a child
/// without putting it in argv, where `ps` shows it to every other user on the
/// host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EnvBase {
    /// Start from this process's environment.
    #[default]
    Parent,
    /// Start from nothing: the child sees exactly [`EnvSpec::entries`].
    Empty,
}

/// The environment a child runs with: a base, plus the entries this call sets.
///
/// A product rather than a three-armed sum (`Inherit | InheritWith | Replace`)
/// so that "inherit" and "inherit with nothing added" cannot be two different
/// values of the same idea. All four combinations mean something: an empty base
/// with entries is Python's `env=`, a parent base with entries is what every
/// caller that wants to *add* one variable builds by hand today.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnvSpec {
    pub base: EnvBase,
    /// Sorted, so a recorded call compares and prints the same way every run.
    pub entries: BTreeMap<String, String>,
}

impl EnvSpec {
    /// The parent's environment, unchanged.
    pub fn inherited() -> Self {
        Self::default()
    }

    /// Nothing but what [`EnvSpec::and`] adds.
    pub fn empty() -> Self {
        Self {
            base: EnvBase::Empty,
            entries: BTreeMap::new(),
        }
    }

    /// Set one variable.
    #[must_use]
    pub fn and(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.entries.insert(name.into(), value.into());
        self
    }
}

/// What to run: the program, its arguments, where, and with which environment.
///
/// The whole argv including the program name, because the callers differ on
/// which end they own: `tty_session` composes a complete `ssh …` command while
/// each `devpod` caller builds a subcommand tail.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
    /// Where the child runs. `None` is this process's directory.
    ///
    /// Never the way a repository is selected — `git` is told which repository
    /// it is looking at by `--git-dir`/`--work-tree` (devlaunch#171); a `cwd`
    /// alone leaves git's discovery walking up the parent chain.
    pub cwd: Option<PathBuf>,
    pub env: EnvSpec,
}

impl Invocation {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn with_arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    #[must_use]
    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    #[must_use]
    pub fn with_cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    #[must_use]
    pub fn with_env(mut self, env: EnvSpec) -> Self {
        self.env = env;
        self
    }

    /// Add one variable to whatever base the environment already has.
    #[must_use]
    pub fn with_var(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.entries.insert(name.into(), value.into());
        self
    }

    /// Program name first, then the arguments — what a recorder prints and what
    /// an argv-sequence assertion compares against.
    pub fn argv(&self) -> Vec<String> {
        let mut argv = Vec::with_capacity(self.args.len() + 1);
        argv.push(self.program.clone());
        argv.extend(self.args.iter().cloned());
        argv
    }
}

/// Where a child's stdin comes from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum StdinPlan {
    /// This process's stdin — the terminal, in an interactive session.
    #[default]
    Inherit,
    /// `/dev/null`. For a child that runs in the middle of something else and
    /// must not eat input belonging to the command the user actually asked for.
    Null,
    /// This file becomes the child's stdin.
    ///
    /// A path rather than bytes, and the runner opens it and hands the
    /// descriptor to the child: the tools bundle is hundreds of megabytes, so
    /// nothing on this path may buffer it.
    File(PathBuf),
}

/// A run this process waits for: what to run, where its stdin comes from, and
/// how long it may take.
///
/// The three waiting methods of [`Runner`] all take this. What is *not* here is
/// the capture decision: that is the method, so a captured outcome carries text
/// and an inherited one carries nothing, with no arm either caller has to
/// handle and cannot reach.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpawnSpec {
    pub invocation: Invocation,
    pub stdin: StdinPlan,
    /// How long the child may take before it is killed and reaped. `None` is
    /// Python's default: wait as long as it takes.
    pub timeout: Option<Duration>,
    /// Whether a [`Runner::passthrough`] child leads a process group of its own.
    ///
    /// `true` means the child leads its own process group, so this process's
    /// SIGINT handler can `killpg` it as a unit — the `devpod up` build comes
    /// down with `dl` rather than outliving it, even when the interrupt arrived
    /// as `kill -INT <pid>` rather than through the terminal. `false` (the
    /// default) keeps the child in this process's group, so it stays the
    /// controlling terminal's foreground group and can read the PTY without
    /// taking SIGTTIN — required for an interactive `ssh -t`, and what the Python
    /// original did. Only `passthrough` reads this field.
    pub own_group: bool,
}

impl From<Invocation> for SpawnSpec {
    fn from(invocation: Invocation) -> Self {
        Self {
            invocation,
            ..Self::default()
        }
    }
}

impl SpawnSpec {
    pub fn new(invocation: Invocation) -> Self {
        invocation.into()
    }

    #[must_use]
    pub fn with_stdin_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.stdin = StdinPlan::File(path.into());
        self
    }

    #[must_use]
    pub fn with_stdin_null(mut self) -> Self {
        self.stdin = StdinPlan::Null;
        self
    }

    #[must_use]
    pub fn with_timeout(mut self, limit: Duration) -> Self {
        self.timeout = Some(limit);
        self
    }

    /// The [`Runner::passthrough`] child should lead a process group of its own,
    /// so this process's SIGINT handler can tear it down independently. For
    /// `devpod up`; see [`SpawnSpec::own_group`].
    #[must_use]
    pub fn leading_its_own_group(mut self) -> Self {
        self.own_group = true;
        self
    }

    pub fn program(&self) -> &str {
        &self.invocation.program
    }

    pub fn args(&self) -> &[String] {
        &self.invocation.args
    }
}

/// How a child that ran to completion ended.
///
/// Two arms rather than Python's one negative-for-a-signal integer: a status
/// and a signal are different facts, and nothing above has to know that -15
/// means SIGTERM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    Signal(i32),
}

impl Exit {
    /// Whether this is the one ending every caller treats as "it worked".
    pub fn is_success(self) -> bool {
        matches!(self, Exit::Code(0))
    }
}

/// What a captured run read from the child.
///
/// Decoded lossily: what a subprocess writes is not this process's promise, and
/// a container's `ssh` banner with one bad byte in it must not turn an answer
/// into an error.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CapturedText {
    pub stdout: String,
    pub stderr: String,
}

/// The OS refused, and this is all it said: no message, because a message for a
/// person is the `dl` binary's to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OsFailure {
    pub kind: std::io::ErrorKind,
    pub errno: Option<i32>,
}

impl From<&std::io::Error> for OsFailure {
    fn from(error: &std::io::Error) -> Self {
        Self {
            kind: error.kind(),
            errno: error.raw_os_error(),
        }
    }
}

/// How a run this process waited for turned out.
///
/// `T` is what the run read back: [`CapturedText`] for [`Runner::capture`],
/// `()` for the two that leave the streams alone.
///
/// The three ways of not running are three arms, not exit codes:
///
/// - [`Outcome::ProgramNotFound`] is what `DevpodNotInstalled` and the exit-127
///   rendering are built on. Folded into a status, a caller branching on the
///   status would carry on as though devpod had answered.
/// - [`Outcome::TimedOut`] is not a status either: the child was killed, so any
///   status it has is this runner's doing and says nothing about the work.
/// - [`Outcome::NotStarted`] is every other OS refusal — an unreadable stdin
///   file, a working directory that is gone — kept apart from "not on PATH"
///   because both arrive as one ENOENT from `spawn`, and reporting the wrong
///   one points the user at the wrong thing entirely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome<T = ()> {
    /// The child ran to completion.
    Ran { exit: Exit, io: T },
    /// `program` is not on PATH, or is not something that can be executed.
    ProgramNotFound,
    /// The timeout elapsed; the child was killed and reaped. Whatever it had
    /// written is dropped, as Python's `subprocess.run(timeout=)` drops it.
    TimedOut,
    /// The child never ran — or, in the case POSIX makes practically
    /// unreachable for one's own child, its ending could not be collected.
    NotStarted(OsFailure),
}

impl<T> Outcome<T> {
    /// Whether the child ran and ended the way every caller wants.
    pub fn succeeded(&self) -> bool {
        matches!(self, Outcome::Ran { exit, .. } if exit.is_success())
    }
}

/// How a detached spawn turned out.
///
/// Its own type because a detached child has no ending to report: this process
/// deliberately never waits for it, so there is no status to carry and no
/// timeout to apply. Python spells the same thing as a `Popen` nobody calls
/// `wait()` on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetachOutcome {
    /// Started, and left alone. The pid is here so a caller (or a test) can
    /// observe the child; nothing in devlaunch waits on it.
    Started {
        pid: u32,
    },
    ProgramNotFound,
    NotStarted(OsFailure),
}

/// The one seam: every process devlaunch starts is started through here.
///
/// One implementation does real work, [`ProcessRunner`], and it is the only impl
/// outside test code — which is what makes this a seam rather than a habit, and
/// is asserted in `tests/one_seam.rs` rather than left to this sentence. One
/// implementation stands in for it across the suite:
/// `devlaunch_test_support::FakeRunner`, a call recorder, an argv-prefix response
/// table, and the fake devpod behind them (`DevpodMachine`, the one the
/// conformance corpus pins).
///
/// Tests wrap those two rather than implementing the trait afresh, and the
/// wrappers come and go, so the enumeration is the scan in `tests/one_seam.rs`,
/// which lists every impl in the workspace with its file and its verdict, rather
/// than a number written here. (`grep -rn "impl Runner for"` is the eyeball
/// version and over-reports: this sentence matches it, and so do that test's own
/// fixtures.) Two shapes are worth recognising before reading one:
///
/// - **Part real.** A wrapper holding a [`ProcessRunner`] beside a fake answers
///   for the programs a unit test must not really run — devpod above all, and
///   docker, whose real daemon would be the developer's own — and hands it
///   whatever they do not fake, git above all. That is how a test drives a real
///   repository against a workspace that never existed. Three do this today, and
///   they are worth naming because a reader otherwise finds them by grepping:
///   `flows::listing`'s `FakeDevpodRealGit`, which is named after the pattern,
///   `flows::lifecycle`'s `Devpod`, and `flows::workspace_clone`'s `StubbedLfs`.
///   The last is the one to read carefully rather than by analogy: it routes on
///   the `git lfs` subcommand rather than on the program, and only its `capture`
///   reaches `ProcessRunner` at all.
/// - **A recorder.** A wrapper that answers by call index rather than from argv
///   is not a fake devpod at all: it plays back a list the test handed it and
///   keeps the argv for the assertions to read. `flows::provision`'s `Trips` is
///   the example, and its doc says why a recorder cannot join the corpus.
pub trait Runner {
    /// Run to completion, reading both streams as text.
    fn capture(&self, spec: &SpawnSpec) -> Outcome<CapturedText>;

    /// Run to completion with this process's streams, capturing nothing.
    fn passthrough(&self, spec: &SpawnSpec) -> Outcome;

    /// Run a terminal session: stdin and stdout inherited, stderr handed to
    /// `on_stderr_line` a line at a time as the child writes it.
    ///
    /// Lines arrive without their newline. The sink decides what to forward —
    /// the point of reading stderr at all is that some of it (devpod's report
    /// of a remote exit status) is held back rather than shown.
    fn session(&self, spec: &SpawnSpec, on_stderr_line: &mut dyn FnMut(&str)) -> Outcome;

    /// Start a child in a session of its own with null stdio, and do not wait.
    ///
    /// Takes an [`Invocation`] rather than a [`SpawnSpec`]: a child nothing
    /// waits for cannot have a timeout, and one that survives this process
    /// cannot be reading its stdin, so neither field has a meaning to give.
    fn detach(&self, what: &Invocation) -> DetachOutcome;
}

/// The production runner: [`std::process::Command`], and nothing else.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessRunner;

impl ProcessRunner {
    pub fn new() -> Self {
        Self
    }
}

impl Runner for ProcessRunner {
    fn capture(&self, spec: &SpawnSpec) -> Outcome<CapturedText> {
        // The child stays in this process's group, and the timeout kill is a
        // single-pid kill because of it. A capture pipes stdout and stderr but
        // not stdin, and `/dev/tty` is reachable whatever stdin is: ssh's
        // host-key confirmation, ssh's passphrase prompt and git's credential
        // prompt all read the terminal from inside a capture. A child outside
        // the terminal's foreground process group takes SIGTTIN on that read and
        // stops — and `try_wait` never reports a stopped child, so the wait runs
        // to its deadline, or forever for the three captures that pass no
        // timeout. Group membership is also the only thing that delivers a
        // terminal Ctrl-C here, since `capture` (unlike `passthrough`) notes no
        // foreground child for the interrupt handler to `killpg`. Both measured
        // under a pty and pinned in `tests/terminal.rs`; a group of its own was
        // tried for the sake of killing the tree and cost more than it bought
        // (#301, #302).
        let mut child = match start(spec, Stdio::piped(), Stdio::piped(), OwnGroup::No) {
            Ok(child) => child,
            Err(outcome) => return outcome.retyped(),
        };
        // Drained on threads rather than after the wait: a child that fills a
        // pipe while this process is waiting for it to exit would deadlock, and
        // the timeout below has to be able to elapse while output is pending.
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());
        let ending = wait(&mut child, spec.timeout);
        match ending {
            // Bounded, not joined (see [`collect`] and [`DRAIN_GRACE`]): the
            // child has exited, but a descendant it forked into a session of its
            // own — git's ssh ControlMaster is the production case — can hold
            // the pipe open with no EOF ever coming, and this is the path that
            // reaches that far the more often of the two.
            Ending::Ended(exit) => Outcome::Ran {
                exit,
                io: CapturedText {
                    stdout: collect(stdout),
                    stderr: collect(stderr),
                },
            },
            // The same bound, at zero: a timed-out outcome drops whatever was
            // written (see [`Outcome::TimedOut`]), so there is nothing here to
            // wait even a moment for. The drains are dropped mid-read and the
            // threads end when — if — the pipe ever closes.
            Ending::Killed => Outcome::TimedOut,
            Ending::Lost(failure) => Outcome::NotStarted(failure),
        }
    }

    fn passthrough(&self, spec: &SpawnSpec) -> Outcome {
        // Whether this child leads a process group of its own is the caller's
        // decision, carried on the spec (see [`SpawnSpec::own_group`]).
        //
        // `devpod up` sets it: it is the one long-running foreground child, and
        // the one a Ctrl-C used to orphan — `dl`'s `_exit(130)` released the
        // launch lock while the build carried on holding it (concurrency review
        // F3). Leading its own group lets `dl`'s interrupt handler `killpg` the
        // build before it exits, so the build comes down with `dl` rather than
        // outliving it, even when the interrupt arrived as `kill -INT <pid>`.
        //
        // An interactive `ssh -t` (the other passthrough caller) must NOT: a
        // child in a group of its own is no longer the controlling terminal's
        // foreground group, so its first read of the PTY earns a SIGTTIN and the
        // session hangs. It stays in this process's group, which is also what
        // the Python original did — as do `session`'s and `capture`'s children,
        // for the same reason.
        if spec.own_group {
            let mut child = match start(spec, Stdio::inherit(), Stdio::inherit(), OwnGroup::Yes) {
                Ok(child) => child,
                Err(outcome) => return outcome.retyped(),
            };
            // The child led its own group from its `pre_exec`, so its pgid is its
            // pid; set it from the parent too to close the fork-to-exec window.
            let pgid = child.id() as i32;
            // SAFETY: `setpgid` on our own just-spawned child; EACCES (already
            // exec'd) or ESRCH (already gone) are both fine — the child's own
            // `pre_exec` establishes the group regardless.
            unsafe {
                libc::setpgid(pgid, pgid);
            }
            interrupt::note_foreground_child(pgid);
            let ending = wait(&mut child, spec.timeout);
            // Reaped now, so the handler must not signal a possibly-recycled pgid.
            interrupt::clear_foreground_child();
            ending.into()
        } else {
            // The child stays in this process's group. Its "pgid" would be this
            // process's own group, so it must NOT be noted for the interrupt
            // handler — a `killpg` on it would fell `dl` and the whole foreground
            // group. Just spawn, wait, and return.
            let mut child = match start(spec, Stdio::inherit(), Stdio::inherit(), OwnGroup::No) {
                Ok(child) => child,
                Err(outcome) => return outcome.retyped(),
            };
            wait(&mut child, spec.timeout).into()
        }
    }

    fn session(&self, spec: &SpawnSpec, on_stderr_line: &mut dyn FnMut(&str)) -> Outcome {
        let mut child = match start(spec, Stdio::inherit(), Stdio::piped(), OwnGroup::No) {
            Ok(child) => child,
            Err(outcome) => return outcome.retyped(),
        };
        // The lines are read on a thread and handed over here as they arrive,
        // so a session that runs for an hour reports devpod's warnings when
        // devpod writes them — and so a timeout can still elapse while the
        // child is quiet.
        let (lines, reader) = match child.stderr.take() {
            Some(pipe) => {
                let (tx, rx) = mpsc::channel();
                let reader = thread::spawn(move || {
                    let mut reader = BufReader::new(pipe);
                    let mut buffer = Vec::new();
                    loop {
                        buffer.clear();
                        match reader.read_until(b'\n', &mut buffer) {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {
                                let mut line = String::from_utf8_lossy(&buffer).into_owned();
                                while line.ends_with('\n') || line.ends_with('\r') {
                                    line.pop();
                                }
                                if tx.send(line).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                });
                (Some(rx), Some(reader))
            }
            None => (None, None),
        };

        let deadline = spec.timeout.map(|limit| Instant::now() + limit);
        let mut timed_out = false;
        if let Some(lines) = lines {
            loop {
                let received = match deadline {
                    None => lines.recv().map_err(|_| Waited::Closed),
                    Some(deadline) => {
                        let left = deadline.saturating_duration_since(Instant::now());
                        lines.recv_timeout(left).map_err(|error| match error {
                            mpsc::RecvTimeoutError::Timeout => Waited::Elapsed,
                            mpsc::RecvTimeoutError::Disconnected => Waited::Closed,
                        })
                    }
                };
                match received {
                    Ok(line) => on_stderr_line(&line),
                    Err(Waited::Closed) => break,
                    Err(Waited::Elapsed) => {
                        timed_out = true;
                        break;
                    }
                }
            }
        }
        let ending = if timed_out {
            kill(&mut child)
        } else {
            // The pipe is closed, so the child is on its way out; whatever is
            // left of its timeout is what it has to finish in.
            wait(
                &mut child,
                deadline.map(|d| d.saturating_duration_since(Instant::now())),
            )
        };
        // The reader is joined only when the pipe closed of its own accord (the
        // loop broke on Closed, so the thread is already on its way out). On a
        // timeout it is abandoned: a descendant in a session of its own can hold
        // the stderr pipe past the kill, and `read_until` would block the join
        // forever (#301). A bound of zero, where `capture`'s success path takes
        // [`DRAIN_GRACE`], and for the same reason its own timeout path does:
        // the lines have already been handed over as they arrived, so there is
        // nothing left here that waiting could collect.
        if !timed_out && let Some(reader) = reader {
            let _ = reader.join();
        }
        ending.into()
    }

    fn detach(&self, what: &Invocation) -> DetachOutcome {
        let mut command = command(what);
        // Null on all three, where Python nulls only stdout and stderr. A child
        // in another session that reads the terminal is stopped by SIGTTIN
        // rather than served, so the descriptor buys it nothing and holding the
        // terminal's stdin is the one way it could interfere with the shell it
        // was started from.
        command.stdin(Stdio::null());
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
        // SAFETY: `setsid` is a bare syscall, which is all a pre-exec hook may
        // do — no allocation, no locks. It is `start_new_session=True`'s
        // meaning exactly: a new *session*, which `process_group(0)` does not
        // give (that is setpgid, which leaves the child in this session and
        // attached to the controlling terminal). After a fork the child is
        // never a process group leader, so the call cannot fail for the one
        // reason setsid ever refuses.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        match command.spawn() {
            // Dropped, never waited for — as Python drops the `Popen`. The child
            // therefore stays a zombie of this process if it finishes first, and
            // becomes init's when this process exits, whichever way round they
            // end. Waiting is what a detach exists not to do.
            Ok(child) => DetachOutcome::Started { pid: child.id() },
            Err(error) => match classify(&what.program, &error) {
                Refusal::NotOnPath => DetachOutcome::ProgramNotFound,
                Refusal::Os(failure) => DetachOutcome::NotStarted(failure),
            },
        }
    }
}

/// Why a session's wait for the next stderr line ended.
enum Waited {
    /// The child closed stderr: there are no more lines coming.
    Closed,
    /// The timeout ran out first.
    Elapsed,
}

/// Why the OS would not give us a child.
enum Refusal {
    NotOnPath,
    Os(OsFailure),
}

/// A spawn that never happened, in a form either outcome type can take.
struct NoChild(Refusal);

impl NoChild {
    fn retyped<T>(self) -> Outcome<T> {
        match self.0 {
            Refusal::NotOnPath => Outcome::ProgramNotFound,
            Refusal::Os(failure) => Outcome::NotStarted(failure),
        }
    }
}

/// How a child that was started ended.
enum Ending {
    Ended(Exit),
    /// Killed for taking longer than its timeout.
    Killed,
    /// Started, and then its ending could not be collected.
    Lost(OsFailure),
}

impl<T> From<Ending> for Outcome<T>
where
    T: Default,
{
    fn from(ending: Ending) -> Self {
        match ending {
            Ending::Ended(exit) => Outcome::Ran {
                exit,
                io: T::default(),
            },
            Ending::Killed => Outcome::TimedOut,
            Ending::Lost(failure) => Outcome::NotStarted(failure),
        }
    }
}

fn command(what: &Invocation) -> Command {
    let mut command = Command::new(&what.program);
    command.args(&what.args);
    if let Some(dir) = &what.cwd {
        command.current_dir(dir);
    }
    if let EnvBase::Empty = what.env.base {
        command.env_clear();
    }
    for (name, value) in &what.env.entries {
        command.env(name, value);
    }
    command
}

/// Whether a child leads a process group of its own.
///
/// [`OwnGroup::Yes`] is `setpgid(0, 0)`, not `setsid`: the child stays in this
/// process's *session* and keeps the controlling terminal (unlike a
/// [`Runner::detach`], which starts a new session with null stdio), but leads
/// its own process *group* so the interrupt handler can `killpg` it as a unit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OwnGroup {
    No,
    Yes,
}

/// Spawn `spec`, or say why there is no child.
fn start(
    spec: &SpawnSpec,
    stdout: Stdio,
    stderr: Stdio,
    own_group: OwnGroup,
) -> Result<Child, NoChild> {
    let stdin = match &spec.stdin {
        StdinPlan::Inherit => Stdio::inherit(),
        StdinPlan::Null => Stdio::null(),
        // The descriptor itself, handed over. Nothing is read here, so the size
        // of the file is the child's business and not this process's memory.
        StdinPlan::File(path) => match File::open(path) {
            Ok(file) => Stdio::from(file),
            // Not `ProgramNotFound`, whatever the errno says: the program was
            // never the thing that could not be found.
            Err(error) => return Err(NoChild(Refusal::Os(OsFailure::from(&error)))),
        },
    };
    let mut command = command(&spec.invocation);
    command.stdin(stdin).stdout(stdout).stderr(stderr);
    if let OwnGroup::Yes = own_group {
        // SAFETY: `setpgid` is a bare syscall, which is all a pre-exec hook may
        // do — no allocation, no locks. Just after a fork the child is never a
        // process group leader, so `setpgid(0, 0)` cannot fail with EPERM; the
        // only reason it ever refuses does not apply.
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    command
        .spawn()
        .map_err(|error| NoChild(classify(&spec.invocation.program, &error)))
}

/// Which refusal an `io::Error` from `spawn` actually is.
///
/// A missing program and a missing working directory are the same ENOENT here,
/// so the errno cannot decide it: PATH is asked about the program instead. Get
/// this wrong and `git` in a clone somebody deleted renders as "git is not
/// installed".
fn classify(program: &str, error: &std::io::Error) -> Refusal {
    if error.kind() == std::io::ErrorKind::NotFound && !is_executable_somewhere(program) {
        Refusal::NotOnPath
    } else {
        Refusal::Os(OsFailure::from(error))
    }
}

/// Whether `program` names something this process could execute.
///
/// A program with a slash in it is a path and is checked as one; anything else
/// is looked for along PATH, the way the kernel's `execvp` would.
fn is_executable_somewhere(program: &str) -> bool {
    if program.contains('/') {
        return is_executable_file(Path::new(program));
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        // An empty PATH entry means the current directory, as it does for the
        // shell.
        let dir = if dir.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            dir
        };
        is_executable_file(&dir.join(program))
    })
}

fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// A pipe being read to EOF on a thread of its own, and what it has read so far.
///
/// The bytes live behind a lock rather than in the thread's return value so that
/// they can be taken *without* joining it. Joining is what cannot be relied on:
/// a descendant that inherited the write end — ssh's ControlMaster is the
/// production case — holds the pipe open long after the child that wrote them
/// exited, and EOF, the read, and the join with it, then never come at all
/// (#301, #302).
struct Drain {
    read: Arc<(Mutex<Reading>, Condvar)>,
}

/// What a drain has read so far, and whether the pipe has reached its end.
///
/// `ended` is the drain thread's own answer, set under the lock as it leaves, so
/// that the condvar it then signals cannot lose a wakeup: [`collect`] tests this
/// flag under the same lock it waits on.
struct Reading {
    bytes: Vec<u8>,
    ended: bool,
}

/// Read a pipe to the end on a thread of its own. `None` in means `None` out —
/// a stream that was inherited has nothing to read, which is not an error.
fn drain(pipe: Option<impl Read + Send + 'static>) -> Option<Drain> {
    pipe.map(|mut pipe| {
        let read = Arc::new((
            Mutex::new(Reading {
                bytes: Vec::new(),
                ended: false,
            }),
            Condvar::new(),
        ));
        let sink = Arc::clone(&read);
        // Detached, never joined: see [`Drain`]. What replaces the join is the
        // `ended` flag below, which says the same thing without the wait being
        // able to outlast the pipe.
        thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                match pipe.read(&mut chunk) {
                    // Retried, not an ending: `EINTR` means a signal arrived
                    // mid-read, which says nothing about the pipe. Treating it
                    // as EOF is silent truncation reported as `Outcome::Ran`
                    // with a success exit — the one wrong answer worse than an
                    // error, since callers parse this text. `read_to_end`, which
                    // this loop replaced, retried it for free; the loop has to
                    // say so.
                    Err(again) if again.kind() == std::io::ErrorKind::Interrupted => continue,
                    // Any other read that fails mid-stream leaves what it got:
                    // partial output is what the child wrote, and there is
                    // nowhere better to put it.
                    Ok(0) | Err(_) => break,
                    Ok(read) => held(&sink.0).bytes.extend_from_slice(&chunk[..read]),
                }
            }
            held(&sink.0).ended = true;
            sink.1.notify_all();
        });
        Drain { read }
    })
}

/// What a drain has read, waiting up to [`DRAIN_GRACE`] for it to reach EOF.
///
/// Called once the child is gone, so the wait is for the pipe to close rather
/// than for anything more to be written; when it expires the reading thread is
/// abandoned with the pipe it will never see the end of, and the bytes it did
/// read are the answer.
///
/// A condvar rather than a poll of `JoinHandle::is_finished`, because this is on
/// the hot path: every capture ends here, and the drain thread usually reaches
/// EOF a few microseconds *after* the wait for the child returned. Polling at
/// [`POLL_INTERVAL`] therefore charged nearly every capture a full sleep — 5ms,
/// twice, against a `capture` that otherwise costs well under a millisecond —
/// where waiting to be woken costs the microseconds it actually takes. The bound
/// is the same; only the waiting is exact.
fn collect(drain: Option<Drain>) -> String {
    let Some(drain) = drain else {
        return String::new();
    };
    let (reading, ended) = &*drain.read;
    let deadline = Instant::now() + DRAIN_GRACE;
    let mut read = held(reading);
    while !read.ended {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        read = ended
            .wait_timeout(read, left)
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .0;
    }
    String::from_utf8_lossy(&read.bytes).into_owned()
}

/// The bytes a drain has read, however the last reader left the lock.
///
/// A poisoned lock means a panic somewhere else in the process, not a doubt
/// about the bytes: the drain thread does nothing under this lock but extend a
/// `Vec`, so what is there is what was read.
fn held<T>(bytes: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    bytes
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Wait for `child`, killing it if `timeout` runs out first.
fn wait(child: &mut Child, timeout: Option<Duration>) -> Ending {
    let Some(limit) = timeout else {
        return match child.wait() {
            Ok(status) => ended(status),
            Err(error) => Ending::Lost(OsFailure::from(&error)),
        };
    };
    // Polled rather than signal-driven: correctness and maintainability over
    // performance (#251), and the cost is at most one poll interval of latency
    // on a process that has already run for as long as its timeout allows.
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return ended(status),
            Ok(None) => {}
            Err(error) => return Ending::Lost(OsFailure::from(&error)),
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return kill(child);
        }
        thread::sleep(left.min(POLL_INTERVAL));
    }
}

/// Kill a child that outstayed its timeout, and reap it — the two halves of
/// what `subprocess.run(timeout=)` does, and a kill without the wait leaves a
/// zombie for the life of the process.
///
/// Single-pid, never a `killpg`: every child that reaches here is in this
/// process's own group, so a group kill would fell `dl` and the whole
/// foreground group with it. What the tool itself forked is therefore left
/// running — knowingly, and at the price the alternative charged (#301, #302).
/// Liveness does not rest on this kill: what makes the outcome come back is
/// that nothing afterwards waits on a pipe a descendant still holds.
fn kill(child: &mut Child) -> Ending {
    let _ = child.kill();
    match child.wait() {
        Ok(_) => Ending::Killed,
        Err(error) => Ending::Lost(OsFailure::from(&error)),
    }
}

const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// How long [`collect`] gives a drained pipe to reach EOF once the child has
/// exited.
///
/// A bound rather than an unbounded join, and this is the whole of #302's fix.
/// The child is gone by the time it is waited on, so everything it wrote is
/// already read or sitting in the pipe buffer and a moment covers the rest; what
/// the wait must not do is outlast a descendant that inherited the write end and
/// lives on, because EOF then never arrives. An unbounded join there hangs `dl`
/// for good on the three captures that pass no timeout at all — `git clone
/// --bare`, `git push -u` and the launch-path fetch — and hangs it for the whole
/// timeout on the rest.
const DRAIN_GRACE: Duration = Duration::from_millis(500);

/// A waited-for child either exited with a status or died of a signal, and
/// `ExitStatus` cannot say which of the two it holds. An answer that is neither
/// is not an ending this can invent a number for, so it is reported as an ending
/// that could not be collected rather than as signal 0 or status 0.
fn ended(status: ExitStatus) -> Ending {
    match (status.code(), status.signal()) {
        (Some(code), _) => Ending::Ended(Exit::Code(code)),
        (None, Some(signal)) => Ending::Ended(Exit::Signal(signal)),
        (None, None) => Ending::Lost(OsFailure {
            kind: std::io::ErrorKind::Other,
            errno: None,
        }),
    }
}
