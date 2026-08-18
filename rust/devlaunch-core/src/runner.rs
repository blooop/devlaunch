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
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

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

/// The one seam. Implemented once for real processes ([`ProcessRunner`]) and
/// once for tests (`devlaunch_test_support::FakeRunner`).
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
        let mut child = match start(spec, Stdio::piped(), Stdio::piped()) {
            Ok(child) => child,
            Err(outcome) => return outcome.retyped(),
        };
        // Drained on threads rather than after the wait: a child that fills a
        // pipe while this process is waiting for it to exit would deadlock, and
        // the timeout below has to be able to elapse while output is pending.
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());
        let ending = wait(&mut child, spec.timeout);
        let io = CapturedText {
            stdout: collect(stdout),
            stderr: collect(stderr),
        };
        match ending {
            Ending::Ended(exit) => Outcome::Ran { exit, io },
            Ending::Killed => Outcome::TimedOut,
            Ending::Lost(failure) => Outcome::NotStarted(failure),
        }
    }

    fn passthrough(&self, spec: &SpawnSpec) -> Outcome {
        let mut child = match start(spec, Stdio::inherit(), Stdio::inherit()) {
            Ok(child) => child,
            Err(outcome) => return outcome.retyped(),
        };
        wait(&mut child, spec.timeout).into()
    }

    fn session(&self, spec: &SpawnSpec, on_stderr_line: &mut dyn FnMut(&str)) -> Outcome {
        let mut child = match start(spec, Stdio::inherit(), Stdio::piped()) {
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
        if let Some(reader) = reader {
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

/// Spawn `spec`, or say why there is no child.
fn start(spec: &SpawnSpec, stdout: Stdio, stderr: Stdio) -> Result<Child, NoChild> {
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

/// Read a pipe to the end on a thread of its own. `None` in means `None` out —
/// a stream that was inherited has nothing to read, which is not an error.
fn drain(pipe: Option<impl Read + Send + 'static>) -> Option<thread::JoinHandle<Vec<u8>>> {
    pipe.map(|mut pipe| {
        thread::spawn(move || {
            let mut buffer = Vec::new();
            // A read that fails mid-stream leaves what it got: partial output
            // is what the child wrote, and there is nowhere better to put it.
            let _ = pipe.read_to_end(&mut buffer);
            buffer
        })
    })
}

fn collect(reader: Option<thread::JoinHandle<Vec<u8>>>) -> String {
    let bytes = reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
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
fn kill(child: &mut Child) -> Ending {
    let _ = child.kill();
    match child.wait() {
        Ok(_) => Ending::Killed,
        Err(error) => Ending::Lost(OsFailure::from(&error)),
    }
}

const POLL_INTERVAL: Duration = Duration::from_millis(5);

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
