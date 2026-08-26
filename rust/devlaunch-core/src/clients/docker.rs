//! Everything devlaunch asks `docker`, and everything it reads back.
//!
//! Three calls, and between them they cover two commands. `dl <ws> rm` removes
//! the named volumes a workspace's devcontainer created, once the workspace
//! itself is gone (devlaunch#325). `dl <ws> kill` asks which containers a wedged
//! workspace's compose project still has up, and kills them (devlaunch#484) —
//! the one reading in the module, and the reason [`Running`] exists beside
//! [`Answer`].
//!
//! Nothing here removes an *image* — images are shared, they are expensive to
//! rebuild, and which workspace owns one is genuinely ambiguous, so they stay
//! outside the line `dl --purge` and `dl --prune` both print.
//!
//! # Why this is a module and not three lines in the flow
//!
//! For the reason [`crate::clients::devpod`] is one: **exactly one place may
//! decide that docker is absent.** A host with no docker never created these
//! volumes, so "not installed" is the one answer that must be silent — and folded
//! into an exit status it would read as a docker that ran and refused, which is a
//! sentence about a machine that has nothing to remove. [`Answer`] keeps the two
//! apart, and these are the only spawns that can produce either.

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
    answer(runner.capture(&spec))
}

/// The containers one compose project has running, or why docker could not say.
///
/// Its own type rather than an [`Answer`] carrying stdout, because this is the
/// one call in the module whose *output* is the answer: everything else here is
/// a removal, and `Answer` drops stdout on purpose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Running {
    /// The ids docker listed, in its order. Empty is a project with nothing up,
    /// which is the ordinary case rather than a failure.
    These(Vec<String>),
    /// No docker on PATH, or one that cannot be exec'd.
    NotInstalled,
    /// docker ran and would not answer, having written this to stderr.
    Refused { exit: Exit, stderr: String },
    /// docker never answered.
    NotStarted(OsFailure),
}

/// The containers docker has running for one compose project.
///
/// **The project name is the workspace id**, because devpod names the compose
/// project itself and derives it from the id — it does not honour a
/// `COMPOSE_PROJECT_NAME` from the project's own `.env`. That is devpod's
/// convention rather than devlaunch's, and `docs/devcontainer-projects.md`
/// carries the rest of what follows from it.
///
/// **The honest limit**: a single-container devcontainer is in no compose project
/// at all, so this finds nothing for one. That is not a hole in the sweep so much
/// as a statement about what the sweep is for — the container is not what blocks
/// a launch, the flock is, and killing the process that holds it is what unwedges
/// the workspace either way.
pub(crate) fn running_for_project(runner: &dyn Runner, project: &str) -> Running {
    let spec = SpawnSpec::from(Invocation::new(PROGRAM).with_args([
        "ps",
        "--quiet",
        "--no-trunc",
        "--filter",
        &format!("label=com.docker.compose.project={project}"),
    ]));
    match runner.capture(&spec) {
        Outcome::Ran {
            exit,
            io: CapturedText { stdout, stderr },
        } => {
            if exit.is_success() {
                Running::These(
                    stdout
                        .lines()
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(str::to_owned)
                        .collect(),
                )
            } else {
                Running::Refused { exit, stderr }
            }
        }
        Outcome::ProgramNotFound => Running::NotInstalled,
        Outcome::TimedOut => Running::NotStarted(OsFailure {
            kind: ErrorKind::TimedOut,
            errno: None,
        }),
        Outcome::NotStarted(failure) if failure.kind == ErrorKind::NotFound => {
            Running::NotInstalled
        }
        Outcome::NotStarted(failure) => Running::NotStarted(failure),
    }
}

/// Kill these containers now, in one call.
///
/// `kill` rather than `stop`, and that is the verb the whole command is named
/// for: a container whose build was SIGKILLed has nothing left to shut down in an
/// orderly way, and `stop` would spend its ten-second grace period on each one
/// before sending the same signal.
pub(crate) fn kill_containers(runner: &dyn Runner, ids: &NonEmpty<String>) -> Answer {
    let mut args = vec!["kill".to_owned()];
    args.extend(ids.iter().cloned());
    let spec = SpawnSpec::from(Invocation::new(PROGRAM).with_args(args));
    answer(runner.capture(&spec))
}

/// The one reading of a docker spawn every removal in this module shares.
fn answer(outcome: Outcome<CapturedText>) -> Answer {
    match outcome {
        Outcome::Ran {
            exit,
            io: CapturedText { stderr, .. },
        } => Answer::Ran { exit, stderr },
        Outcome::ProgramNotFound => Answer::NotInstalled,
        // Unreachable as this build spawns docker — no call here passes a timeout
        // — and mapped rather than claimed impossible, because a bound added later
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

    #[test]
    fn a_compose_projects_containers_are_asked_for_by_label() {
        let fake = FakeRunner::new();

        let _ = running_for_project(&fake, "my-ws");

        assert_eq!(
            fake.argvs(),
            [[
                "docker",
                "ps",
                "--quiet",
                "--no-trunc",
                "--filter",
                "label=com.docker.compose.project=my-ws",
            ]]
        );
    }

    /// One id per line, and a project with nothing up prints nothing at all --
    /// which is the ordinary case rather than a failure, since the container is
    /// usually the first thing to die.
    #[test]
    fn the_listing_is_one_container_id_per_line() {
        let fake = FakeRunner::new();
        fake.script(["docker", "ps"], Response::stdout("abc123\ndef456\n"));

        assert_eq!(
            running_for_project(&fake, "my-ws"),
            Running::These(vec!["abc123".to_owned(), "def456".to_owned()])
        );

        let empty = FakeRunner::new();
        empty.script(["docker", "ps"], Response::stdout(""));
        assert_eq!(running_for_project(&empty, "my-ws"), Running::These(vec![]));
    }

    #[test]
    fn a_machine_with_no_docker_lists_no_containers_rather_than_none() {
        let fake = FakeRunner::new();
        fake.script_missing("docker");

        assert_eq!(running_for_project(&fake, "my-ws"), Running::NotInstalled);
    }

    /// `kill`, not `stop`: a container belonging to a workspace whose build was
    /// SIGKILLed has nothing left to shut down gracefully, and `stop` would spend
    /// its ten-second grace on each one before doing this anyway.
    #[test]
    fn every_container_is_killed_in_one_call() {
        let fake = FakeRunner::new();

        kill_containers(&fake, &names(&["abc123", "def456"]));

        assert_eq!(fake.argvs(), [["docker", "kill", "abc123", "def456"]]);
    }

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
