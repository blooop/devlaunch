//! Everything devlaunch asks `docker`, and everything it reads back.
//!
//! One verb, and it is a *removal*: the named volumes a workspace's devcontainer
//! created, once the workspace itself is gone (devlaunch#325). Nothing here reads
//! docker's state, and nothing here removes an image — images are shared, they are
//! expensive to rebuild, and which workspace owns one is genuinely ambiguous, so
//! they stay outside the line `dl --purge` and `dl --prune` both print.
//!
//! # Why this is a module and not three lines in the flow
//!
//! For the reason [`crate::clients::devpod`] is one: **exactly one place may
//! decide that docker is absent.** A host with no docker never created these
//! volumes, so "not installed" is the one answer that must be silent — and folded
//! into an exit status it would read as a docker that ran and refused, which is a
//! sentence about a machine that has nothing to remove. [`Answer`] keeps the two
//! apart, and this is the only spawn that can produce either.

use std::io::ErrorKind;

use crate::domain::workspace_state::NonEmpty;
use crate::runner::{CapturedText, Exit, Invocation, OsFailure, Outcome, Runner, SpawnSpec};

/// The program every call in this module runs. One constant, so no caller spells
/// it and a test's response table has one name to match on.
pub(crate) const PROGRAM: &str = "docker";

/// What asking docker something came to.
///
/// Three arms rather than an exit status, because two of them are not one, and
/// they are not the same absence:
///
/// - [`Answer::NotInstalled`] is a machine with no docker on it. Nothing to
///   report: a machine with no docker made no volumes.
/// - [`Answer::NotStarted`] is a docker that is there and did not answer — the OS
///   refused the spawn, or the child was killed. That is a fact about this
///   machine, and it carries the errno for the binary to phrase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    /// docker ran to completion, having written this to stderr. stdout is
    /// dropped: `docker volume rm` echoes the names back, which nothing reads.
    Ran { exit: Exit, stderr: String },
    /// docker is not on PATH — or is there and cannot be exec'd, which arrives as
    /// the same ENOENT and points at the same fix.
    NotInstalled,
    /// docker never answered. `failure.kind` is
    /// [`ErrorKind::TimedOut`] where the child was killed, which is the one case
    /// carrying no errno.
    NotStarted(OsFailure),
}

/// Remove these named volumes, counting one that is already gone as removed.
///
/// **One call carrying every name, and `--force` is what makes that safe.**
/// Measured against docker 29.7: `--force` makes an absent volume exit 0, so a
/// repository whose devcontainer never declared one of these names is a silent
/// no-op rather than a complaint on every delete — while a volume some container
/// still holds is *still* refused, which is the one refusal worth reporting. A
/// name-at-a-time loop would buy nothing but round trips and a partial-failure
/// state to model.
///
/// Captured rather than inherited: a successful removal has nothing to say to a
/// user's terminal, and a refusal is a notice this process frames.
pub(crate) fn remove_volumes(runner: &dyn Runner, names: &NonEmpty<String>) -> Answer {
    let mut args = vec!["volume".to_owned(), "rm".to_owned(), "--force".to_owned()];
    args.extend(names.iter().cloned());
    let spec = SpawnSpec::from(Invocation::new(PROGRAM).with_args(args));
    match runner.capture(&spec) {
        Outcome::Ran {
            exit,
            io: CapturedText { stderr, .. },
        } => Answer::Ran { exit, stderr },
        Outcome::ProgramNotFound => Answer::NotInstalled,
        // Unreachable as this build spawns docker — the call is given no timeout —
        // and mapped rather than claimed impossible, because a bound added later
        // must not read as a removal that happened.
        Outcome::TimedOut => Answer::NotStarted(OsFailure {
            kind: ErrorKind::TimedOut,
            errno: None,
        }),
        // A docker found on PATH that could not be exec'd fails with ENOENT at
        // exec time, which arrives here rather than as `ProgramNotFound`. It gets
        // the docker-is-missing answer for the same reason devpod's spawn gives it
        // one: the fix is to install a working docker either way.
        Outcome::NotStarted(failure) if failure.kind == ErrorKind::NotFound => Answer::NotInstalled,
        Outcome::NotStarted(failure) => Answer::NotStarted(failure),
    }
}

#[cfg(test)]
mod tests {
    use devlaunch_test_support::{FakeRunner, Response};

    use super::*;

    fn names(items: &[&str]) -> NonEmpty<String> {
        NonEmpty::of(items.iter().map(|name| (*name).to_owned())).expect("at least one name")
    }

    #[test]
    fn every_name_goes_in_one_forced_removal() {
        let fake = FakeRunner::new();

        remove_volumes(&fake, &names(&["ws-pixi", "dind-var-lib-docker-abc"]));

        assert_eq!(
            fake.argvs(),
            [[
                "docker",
                "volume",
                "rm",
                "--force",
                "ws-pixi",
                "dind-var-lib-docker-abc",
            ]]
        );
    }

    #[test]
    fn a_docker_that_refused_answers_with_its_own_words() {
        let fake = FakeRunner::new();
        fake.script(
            ["docker", "volume", "rm"],
            Response::failed(1, "volume is in use\n"),
        );

        assert_eq!(
            remove_volumes(&fake, &names(&["ws-pixi"])),
            Answer::Ran {
                exit: Exit::Code(1),
                stderr: "volume is in use\n".to_owned(),
            }
        );
    }

    #[test]
    fn a_machine_with_no_docker_says_so_rather_than_failing() {
        let fake = FakeRunner::new();
        fake.script_missing("docker");

        assert_eq!(
            remove_volumes(&fake, &names(&["ws-pixi"])),
            Answer::NotInstalled
        );
    }

    /// A docker on PATH that cannot be exec'd reads as a docker that is not there:
    /// the fix is the same, and the alternative is an errno nobody can act on.
    #[test]
    fn a_docker_that_cannot_be_execd_reads_as_no_docker() {
        let fake = FakeRunner::new();
        fake.script(
            ["docker"],
            Response::NotStarted(OsFailure {
                kind: ErrorKind::NotFound,
                errno: Some(2),
            }),
        );

        assert_eq!(
            remove_volumes(&fake, &names(&["ws-pixi"])),
            Answer::NotInstalled
        );
    }

    #[test]
    fn any_other_os_refusal_keeps_its_errno() {
        let fake = FakeRunner::new();
        let failure = OsFailure {
            kind: ErrorKind::PermissionDenied,
            errno: Some(13),
        };
        fake.script(["docker"], Response::NotStarted(failure));

        assert_eq!(
            remove_volumes(&fake, &names(&["ws-pixi"])),
            Answer::NotStarted(failure)
        );
    }
}
