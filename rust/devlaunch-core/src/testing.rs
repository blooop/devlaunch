//! Core's own seam onto the workspace's one fake runner.
//!
//! The fake itself is [`devlaunch_test_support::FakeRunner`]: a call recorder, an
//! argv-prefix response table, and a devpod state machine behind them. It used to
//! be unusable from in here — `devlaunch-test-support` depended back on
//! `devlaunch-core`, so a unit-test build linked *two* cores and their `Runner`
//! traits were different traits — and each client module grew a small local
//! recorder of its own instead. That back-edge is gone (the trait moved down to
//! the `devlaunch-runner` leaf crate, which is what [`crate::runner`] re-exports),
//! so the local copies are gone too and this is the single seam that replaced
//! them.
//!
//! What `FakeRunner` cannot supply on its own, and this module adds, is the
//! timing exclusion: [`crate::timing`] lives in *this* crate, and
//! `devlaunch-test-support` depends only on `devlaunch-runner`, so a fake defined
//! there could not take a guard even in principle. [`ScriptedRunner`] is that one
//! addition and nothing else.

use std::ops::Deref;

use devlaunch_test_support::{FakeRunner, Response};

use crate::runner::{CapturedText, DetachOutcome, Invocation, Outcome, Runner, SpawnSpec};
use crate::timing;

/// [`FakeRunner`], holding [`timing::exclusive`] for its own lifetime.
///
/// Every call through it is spanned against the **process-global** timing
/// registry — `clients::devpod` names each round trip, `domain::workspace_state`
/// opens the `devpod-up` stage, `flows::listing` opens one for a container-state
/// probe — so without the guard a test that merely asks devpod something writes
/// into whatever document a concurrent measured test installed. The guard is in
/// the fixture rather than at the top of each test, so no test has to remember
/// it; the exclusion is reentrant per thread, so a test that also builds a
/// `Cache` or a `Scene` does not deadlock.
#[derive(Debug)]
pub(crate) struct ScriptedRunner {
    fake: FakeRunner,
    /// See [`timing::exclusive`]. Last field, so it is dropped last.
    _serialized: timing::Exclusive,
}

impl ScriptedRunner {
    pub(crate) fn new() -> Self {
        Self {
            fake: FakeRunner::new(),
            _serialized: timing::exclusive(),
        }
    }

    /// [`FakeRunner::with_script`], for a chain that builds the fake in one
    /// expression. Re-offered here rather than reached through [`Deref`] because
    /// the chain takes `self` by value.
    #[must_use]
    pub(crate) fn with_script<I, S>(self, argv: I, response: Response) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.script(argv, response);
        self
    }

    /// [`FakeRunner::with_missing`], as a chain: this program is not installed.
    #[must_use]
    pub(crate) fn with_missing(self, program: impl Into<String>) -> Self {
        self.script_missing(program);
        self
    }
}

impl Default for ScriptedRunner {
    fn default() -> Self {
        Self::new()
    }
}

/// Everything a test reads back — `calls`, `argvs`, `args_to`, `only_call` — and
/// everything it scripts by reference is the fake's own, unchanged.
impl Deref for ScriptedRunner {
    type Target = FakeRunner;

    fn deref(&self) -> &Self::Target {
        &self.fake
    }
}

impl Runner for ScriptedRunner {
    fn capture(&self, spec: &SpawnSpec) -> Outcome<CapturedText> {
        self.fake.capture(spec)
    }

    fn passthrough(&self, spec: &SpawnSpec) -> Outcome {
        self.fake.passthrough(spec)
    }

    fn session(&self, spec: &SpawnSpec, on_stderr_line: &mut dyn FnMut(&str)) -> Outcome {
        self.fake.session(spec, on_stderr_line)
    }

    fn detach(&self, what: &Invocation) -> DetachOutcome {
        self.fake.detach(what)
    }
}
