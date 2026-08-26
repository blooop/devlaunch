//! Signalling host processes: `kill(1)`, and nothing more of it than two signals.
//!
//! # Why a spawn rather than the syscall
//!
//! `rustix` is already a dependency and `kill(2)` is one call, so this is a
//! deliberate choice and not the shortest path. Two things buy it:
//!
//! - **It is the seam.** Every other process devlaunch starts goes through the
//!   runner, so a test scripts a `kill` the way it scripts a `devpod up`, and the
//!   pids a run signalled are an argv assertion rather than a syscall trace.
//!   Reaching past the seam to signal would put the one irreversible thing
//!   `dl <ws> kill` does in the one place no test can watch.
//! - **The per-pid answer is not worth having.** `kill(2)` distinguishes ESRCH
//!   from EPERM per pid; `kill(1)` gives one exit status for the batch. Neither
//!   matters here, because the flow re-reads the process table afterwards and
//!   *that* is what says which processes are gone. An errno would be a second
//!   answer to a question already answered better.
//!
//! # Two signals, in this order
//!
//! SIGTERM first, because a `devpod up` that takes it unwinds: it drops the
//! flock, and on a good day it removes the busy marker its own `defer` was going
//! to remove anyway. SIGKILL second, for the ones that do not, and it is the
//! reason the marker has to be swept by hand — a `defer` does not run under
//! SIGKILL.

use crate::domain::workspace_state::NonEmpty;
use crate::runner::{Invocation, Outcome, Runner, SpawnSpec};

/// The program every call in this module runs.
pub(crate) const PROGRAM: &str = "kill";

/// Which of the two signals this pass sends.
///
/// A pair rather than a signal number, because these are the only two
/// `dl <ws> kill` has any business sending and the escalation between them is
/// the whole of what the verb does. A number would invite a third.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    /// SIGTERM: the ask.
    Terminate,
    /// SIGKILL: the one nothing catches.
    Kill,
}

impl Signal {
    /// The flag `kill(1)` spells this signal with.
    fn flag(self) -> &'static str {
        match self {
            Self::Terminate => "-TERM",
            Self::Kill => "-KILL",
        }
    }
}

/// Whether the signal was sent at all.
///
/// Deliberately not "whether it landed": a pid that exited between the reading
/// and the signal makes `kill` exit non-zero, and that is the *expected* ending
/// on a good run rather than a failure. What survived is read back off the
/// process table, so the only thing left for this to report is a host where
/// nothing could be signalled in the first place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sent {
    /// `kill` ran. Whether each pid took the signal is the table's answer.
    Attempted,
    /// No `kill` on PATH, or one that could not be exec'd or started. A host
    /// where nothing can be signalled is one `dl <ws> kill` cannot unwedge, and
    /// this is what lets it say so instead of reporting an empty sweep.
    NoKillOnThisHost,
}

/// Send `signal` to every pid, in one call.
///
/// One call rather than one per pid because the batch is the point: these
/// processes hold one workspace's lock between them, and signalling them one at
/// a time gives the survivors a window in which to notice.
pub(crate) fn signal(runner: &dyn Runner, signal: Signal, pids: &NonEmpty<u32>) -> Sent {
    let mut args = vec![signal.flag().to_owned()];
    args.extend(pids.iter().map(u32::to_string));
    let spec = SpawnSpec::from(Invocation::new(PROGRAM).with_args(args));
    match runner.capture(&spec) {
        Outcome::Ran { .. } => Sent::Attempted,
        Outcome::ProgramNotFound => Sent::NoKillOnThisHost,
        // A `kill` on PATH that cannot be exec'd, one the OS refused to start,
        // and one that outlasted a bound this call does not set: none of them
        // signalled anything, and the fix for all three is the same host.
        Outcome::TimedOut | Outcome::NotStarted(_) => Sent::NoKillOnThisHost,
    }
}

#[cfg(test)]
mod tests {
    use devlaunch_test_support::{FakeRunner, Response};

    use super::*;

    fn pids(of: &[u32]) -> NonEmpty<u32> {
        NonEmpty::of(of.iter().copied()).expect("at least one pid")
    }

    #[test]
    fn every_pid_takes_the_signal_in_one_call() {
        let fake = FakeRunner::new();

        signal(&fake, Signal::Terminate, &pids(&[732_721, 1234]));

        assert_eq!(fake.argvs(), [["kill", "-TERM", "732721", "1234"]]);
    }

    #[test]
    fn the_second_pass_is_the_signal_nothing_catches() {
        let fake = FakeRunner::new();

        signal(&fake, Signal::Kill, &pids(&[732_721]));

        assert_eq!(fake.argvs(), [["kill", "-KILL", "732721"]]);
    }

    /// A pid that died between the reading and the signal is `kill`'s one
    /// routine failure, and it must not read as a signal that did not land: the
    /// table is re-read either way, and the table is what settles it.
    #[test]
    fn a_kill_that_refused_is_still_a_signal_that_was_sent() {
        let fake = FakeRunner::new();
        fake.script(
            ["kill"],
            Response::failed(1, "kill: (1234): No such process\n"),
        );

        assert_eq!(
            signal(&fake, Signal::Terminate, &pids(&[1234])),
            Sent::Attempted
        );
    }

    /// A host with no `kill` on it cannot be unwedged, and has to say so rather
    /// than report that nothing needed killing.
    #[test]
    fn a_machine_with_no_kill_says_so() {
        let fake = FakeRunner::new();
        fake.script_missing("kill");

        assert_eq!(
            signal(&fake, Signal::Terminate, &pids(&[1234])),
            Sent::NoKillOnThisHost
        );
    }
}
