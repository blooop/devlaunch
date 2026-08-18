//! The one fake: a call recorder, a response table, and a devpod behind them.
//!
//! Three parts, in the order a call meets them:
//!
//! 1. **the recorder** — every spawn spec, in order, whatever mode it was run
//!    in. The argv-sequence assertions that were devlaunch#78's durable artifact
//!    fall out of this for free, and so does "how many times did that spawn
//!    `devpod`".
//! 2. **the response table** — program plus an argv prefix, first match wins.
//!    The failure-injection channel: a `devpod` that is not installed, an `up`
//!    that fails halfway, output a parser should refuse.
//! 3. **the devpod machine** — [`DevpodMachine`], which answers any `devpod`
//!    call the table did not claim, statefully.
//!
//! Anything else with no table entry gets [`Unscripted`]'s answer, which is a
//! quiet success by default: the tools that run opportunistically (`gh auth
//! token`) should not have to be scripted by every test that does not care about
//! them. A test that would rather hear about an unanticipated spawn asks for
//! [`Unscripted::Panic`].

use std::sync::Mutex;

use devlaunch_runner::{CapturedText, DetachOutcome, Invocation, Outcome, Runner, SpawnSpec};

use crate::devpod::{DevpodMachine, FakeWorkspace, Source, WorkspaceState};
use crate::response::Response;

/// One recorded spawn. The mode is an arm rather than a field, because the modes
/// do not take the same spec: a detached child has no stdin and no timeout, so
/// there is nothing for those fields to say about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    Capture(SpawnSpec),
    Passthrough(SpawnSpec),
    Session(SpawnSpec),
    Detach(Invocation),
}

impl Call {
    pub fn invocation(&self) -> &Invocation {
        match self {
            Self::Capture(spec) | Self::Passthrough(spec) | Self::Session(spec) => &spec.invocation,
            Self::Detach(invocation) => invocation,
        }
    }

    pub fn program(&self) -> &str {
        &self.invocation().program
    }

    pub fn args(&self) -> &[String] {
        &self.invocation().args
    }

    /// Program first, then arguments — what an argv assertion compares against.
    pub fn argv(&self) -> Vec<String> {
        self.invocation().argv()
    }
}

/// One response-table entry: this program, whose argv starts this way, answers
/// this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scripted {
    pub program: String,
    pub args_prefix: Vec<String>,
    pub response: Response,
}

impl Scripted {
    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == self.program && args.starts_with(&self.args_prefix)
    }
}

/// What to do about a spawn nothing scripted and no state machine owns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Unscripted {
    /// Exit 0 with no output, as `devpod_mock.py` does.
    #[default]
    Succeed,
    /// Fail the test, naming the argv. For a test that means to account for
    /// every process its subject starts.
    Panic,
}

/// The fake [`Runner`]. Interior mutability throughout, because `Runner` takes
/// `&self` — a test holds one of these and reads it back while the code under
/// test is still using it.
#[derive(Debug)]
pub struct FakeRunner {
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    calls: Vec<Call>,
    scripts: Vec<Scripted>,
    devpod: DevpodMachine,
    unscripted: Unscripted,
    next_pid: u32,
}

/// Where the fake's pids start. High enough not to be mistaken for a real one,
/// and it counts up, so two detached spawns are told apart.
const FIRST_FAKE_PID: u32 = 900_001;

impl Default for FakeRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeRunner {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                calls: Vec::new(),
                scripts: Vec::new(),
                devpod: DevpodMachine::new(),
                unscripted: Unscripted::default(),
                next_pid: FIRST_FAKE_PID,
            }),
        }
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    // ------------------------------------------------------------- scripting

    /// Answer `response` to any call to this argv prefix. `argv` starts with the
    /// program: `["devpod", "up"]` matches every `devpod up …`, and `["devpod"]`
    /// matches every devpod call at all.
    ///
    /// Entries are searched in the order they were added, and the first match
    /// wins — so a specific entry goes in before the general one it refines.
    pub fn script<I, S>(&self, argv: I, response: Response) -> &Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut argv = argv.into_iter().map(Into::into);
        let program = argv.next().expect("a scripted argv names a program");
        self.inner().scripts.push(Scripted {
            program,
            args_prefix: argv.collect(),
            response,
        });
        self
    }

    /// [`FakeRunner::script`], for a chain that builds the fake in one
    /// expression.
    #[must_use]
    pub fn with_script<I, S>(self, argv: I, response: Response) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.script(argv, response);
        self
    }

    /// This program is not installed: every call to it answers
    /// [`Outcome::ProgramNotFound`].
    pub fn script_missing(&self, program: impl Into<String>) -> &Self {
        self.script([program.into()], Response::ProgramNotFound)
    }

    /// [`FakeRunner::script_missing`], as a chain.
    #[must_use]
    pub fn with_missing(self, program: impl Into<String>) -> Self {
        self.script_missing(program);
        self
    }

    pub fn clear_scripts(&self) -> &Self {
        self.inner().scripts.clear();
        self
    }

    pub fn on_unscripted(&self, policy: Unscripted) -> &Self {
        self.inner().unscripted = policy;
        self
    }

    /// [`FakeRunner::on_unscripted`], as a chain.
    #[must_use]
    pub fn with_unscripted(self, policy: Unscripted) -> Self {
        self.on_unscripted(policy);
        self
    }

    // --------------------------------------------------------- devpod's state

    /// Give the fake devpod a workspace, sourced from a plausible clone path.
    pub fn add_workspace(&self, id: &str, state: WorkspaceState) -> &Self {
        self.add_workspace_from(id, &format!("/fake/clones/{id}"), state)
    }

    /// Give the fake devpod a workspace with a source of your choosing — a path
    /// or a URL; devpod records the two differently and `--purge` reads it.
    pub fn add_workspace_from(&self, id: &str, source: &str, state: WorkspaceState) -> &Self {
        self.inner()
            .devpod
            .insert(FakeWorkspace::new(id, Source::classify(source), state));
        self
    }

    /// [`FakeRunner::add_workspace`] with [`WorkspaceState::Running`], as a
    /// chain.
    #[must_use]
    pub fn with_running(self, id: &str) -> Self {
        self.add_workspace(id, WorkspaceState::Running);
        self
    }

    /// [`FakeRunner::add_workspace`] with [`WorkspaceState::Stopped`], as a
    /// chain.
    #[must_use]
    pub fn with_stopped(self, id: &str) -> Self {
        self.add_workspace(id, WorkspaceState::Stopped);
        self
    }

    pub fn set_state(&self, id: &str, state: WorkspaceState) -> &Self {
        self.inner().devpod.set_state(id, state);
        self
    }

    /// The state of one workspace, or `None` when the fake devpod has no such
    /// workspace.
    pub fn state_of(&self, id: &str) -> Option<WorkspaceState> {
        self.inner().devpod.state_of(id)
    }

    /// Every workspace the fake devpod holds, sorted.
    pub fn workspace_ids(&self) -> Vec<String> {
        self.inner().devpod.ids()
    }

    /// A copy of one workspace, for a test that wants to look at its source or
    /// its stamp.
    pub fn workspace(&self, id: &str) -> Option<FakeWorkspace> {
        self.inner().devpod.get(id).cloned()
    }

    pub fn add_provider(&self, name: &str) -> &Self {
        self.inner().devpod.add_provider(name);
        self
    }

    pub fn provider_names(&self) -> Vec<String> {
        self.inner().devpod.provider_names()
    }

    /// What `up` stamps `lastUsed` with.
    pub fn set_stamp(&self, stamp: &str) -> &Self {
        self.inner().devpod.stamp = stamp.to_string();
        self
    }

    // ------------------------------------------------------------- the record

    /// Every call, in the order it was made.
    pub fn calls(&self) -> Vec<Call> {
        self.inner().calls.clone()
    }

    pub fn call_count(&self) -> usize {
        self.inner().calls.len()
    }

    /// Every call's whole argv, in order.
    pub fn argvs(&self) -> Vec<Vec<String>> {
        self.inner().calls.iter().map(Call::argv).collect()
    }

    /// Every call to `program`, in order.
    pub fn calls_to(&self, program: &str) -> Vec<Call> {
        self.inner()
            .calls
            .iter()
            .filter(|call| call.program() == program)
            .cloned()
            .collect()
    }

    /// The argv **after** the program name, for each call to `program`: the
    /// shape a `devpod` argv assertion wants, since callers build subcommand
    /// tails.
    pub fn args_to(&self, program: &str) -> Vec<Vec<String>> {
        self.inner()
            .calls
            .iter()
            .filter(|call| call.program() == program)
            .map(|call| call.args().to_vec())
            .collect()
    }

    /// Forget the calls so far, keeping the scripts and devpod's state. For a
    /// test whose setup spawns things it is not asserting about.
    pub fn forget_calls(&self) -> &Self {
        self.inner().calls.clear();
        self
    }

    // ------------------------------------------------------------- answering

    /// Record the call and decide what it answers.
    fn answer(&self, call: Call) -> Response {
        let mut inner = self.inner();
        let program = call.program().to_string();
        let args = call.args().to_vec();
        inner.calls.push(call);
        if let Some(scripted) = inner
            .scripts
            .iter()
            .find(|scripted| scripted.matches(&program, &args))
        {
            return scripted.response.clone();
        }
        if program == "devpod" {
            return inner.devpod.answer(&args);
        }
        match inner.unscripted {
            Unscripted::Succeed => Response::ok(),
            Unscripted::Panic => {
                let mut argv = vec![program];
                argv.extend(args);
                panic!("nothing scripted this spawn: {argv:?}");
            }
        }
    }

    fn next_pid(&self) -> u32 {
        let mut inner = self.inner();
        inner.next_pid += 1;
        inner.next_pid
    }
}

impl Runner for FakeRunner {
    fn capture(&self, spec: &SpawnSpec) -> Outcome<CapturedText> {
        self.answer(Call::Capture(spec.clone())).captured()
    }

    fn passthrough(&self, spec: &SpawnSpec) -> Outcome {
        self.answer(Call::Passthrough(spec.clone())).quiet()
    }

    fn session(&self, spec: &SpawnSpec, on_stderr_line: &mut dyn FnMut(&str)) -> Outcome {
        let response = self.answer(Call::Session(spec.clone()));
        for line in response.stderr_lines() {
            on_stderr_line(&line);
        }
        response.quiet()
    }

    fn detach(&self, what: &Invocation) -> DetachOutcome {
        let response = self.answer(Call::Detach(what.clone()));
        response.detached(self.next_pid())
    }
}

#[cfg(test)]
mod tests;
