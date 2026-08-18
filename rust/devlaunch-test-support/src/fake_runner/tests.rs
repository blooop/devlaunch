//! What the fake promises every later milestone's tests: that it remembers
//! every spawn, that a scripted answer wins over the state machine, and that
//! what it hands back is the same shape the production runner hands back.

use super::*;
use devlaunch_core::runner::{EnvSpec, Exit, OsFailure, StdinPlan};
use std::time::Duration;

fn devpod(args: &[&str]) -> SpawnSpec {
    SpawnSpec::from(Invocation::new("devpod").with_args(args.iter().copied()))
}

// ---------------------------------------------------------------- the recorder

#[test]
fn every_call_is_recorded_in_order_whatever_mode_it_was_run_in() {
    let fake = FakeRunner::new().with_running("ws");
    fake.capture(&devpod(&["list", "--output", "json"]));
    fake.passthrough(&devpod(&["up", "/clones/ws", "--id", "ws"]));
    fake.session(&devpod(&["ssh", "ws"]), &mut |_| {});
    fake.detach(&Invocation::new("dl").with_arg("--update-cache"));

    assert_eq!(
        fake.argvs(),
        [
            vec!["devpod", "list", "--output", "json"],
            vec!["devpod", "up", "/clones/ws", "--id", "ws"],
            vec!["devpod", "ssh", "ws"],
            vec!["dl", "--update-cache"],
        ]
    );
    assert_eq!(fake.call_count(), 4);
}

#[test]
fn the_mode_of_each_call_is_recorded_too() {
    let fake = FakeRunner::new();
    fake.capture(&devpod(&["version"]));
    fake.passthrough(&devpod(&["up", "/clones/ws"]));
    fake.session(&devpod(&["ssh", "ws"]), &mut |_| {});
    fake.detach(&Invocation::new("dl"));

    let modes: Vec<&str> = fake
        .calls()
        .iter()
        .map(|call| match call {
            Call::Capture(_) => "capture",
            Call::Passthrough(_) => "passthrough",
            Call::Session(_) => "session",
            Call::Detach(_) => "detach",
        })
        .collect();
    assert_eq!(modes, ["capture", "passthrough", "session", "detach"]);
}

#[test]
fn the_whole_spec_is_recorded_not_just_the_argv() {
    let fake = FakeRunner::new();
    let spec = SpawnSpec::from(
        Invocation::new("git")
            .with_args(["status", "--porcelain"])
            .with_cwd("/clones/ws")
            .with_var("GH_TOKEN", "s3cret"),
    )
    .with_stdin_file("/payload.tar")
    .with_timeout(Duration::from_secs(30));
    fake.capture(&spec);

    assert_eq!(fake.calls(), [Call::Capture(spec)]);
}

#[test]
fn calls_can_be_read_back_by_program() {
    let fake = FakeRunner::new().with_running("ws");
    fake.capture(&devpod(&["status", "ws"]));
    fake.capture(&SpawnSpec::from(
        Invocation::new("git").with_arg("rev-parse"),
    ));
    fake.capture(&devpod(&["list"]));

    assert_eq!(fake.args_to("devpod"), [vec!["status", "ws"], vec!["list"]]);
    assert_eq!(fake.calls_to("git").len(), 1);
}

#[test]
fn the_record_can_be_forgotten_without_forgetting_the_state() {
    let fake = FakeRunner::new().with_stopped("ws");
    fake.capture(&devpod(&["ssh", "ws"]));
    fake.forget_calls();

    assert_eq!(fake.call_count(), 0);
    assert_eq!(fake.state_of("ws"), Some(WorkspaceState::Running));
}

// ------------------------------------------------------------ the argv table

#[test]
fn a_script_matches_a_program_and_an_argv_prefix() {
    let fake = FakeRunner::new().with_script(
        ["devpod", "list", "--output", "json"],
        Response::stdout("[]\n"),
    );

    assert_eq!(
        fake.capture(&devpod(&["list", "--output", "json"])),
        Outcome::Ran {
            exit: Exit::Code(0),
            io: CapturedText {
                stdout: "[]\n".to_string(),
                stderr: String::new(),
            },
        }
    );
}

#[test]
fn a_script_matches_a_longer_argv_that_starts_the_same_way() {
    let fake =
        FakeRunner::new().with_script(["git", "fetch"], Response::failed(128, "no remote\n"));
    let outcome = fake.capture(&SpawnSpec::from(
        Invocation::new("git").with_args(["fetch", "--prune", "origin"]),
    ));

    match outcome {
        Outcome::Ran { exit, io } => {
            assert_eq!(exit, Exit::Code(128));
            assert_eq!(io.stderr, "no remote\n");
        }
        other => panic!("expected the scripted failure, got {other:?}"),
    }
}

#[test]
fn the_first_matching_script_wins() {
    let fake = FakeRunner::new()
        .with_script(
            ["devpod", "up", "--id", "special"],
            Response::stdout("first\n"),
        )
        .with_script(["devpod", "up"], Response::stdout("second\n"));

    let (special, general) = (
        fake.capture(&devpod(&["up", "--id", "special"])),
        fake.capture(&devpod(&["up", "--id", "ordinary"])),
    );
    assert!(matches!(special, Outcome::Ran { io, .. } if io.stdout == "first\n"));
    assert!(matches!(general, Outcome::Ran { io, .. } if io.stdout == "second\n"));
}

#[test]
fn a_script_for_one_program_does_not_answer_for_another() {
    let fake = FakeRunner::new().with_script(["git", "status"], Response::stdout("dirty\n"));
    let outcome = fake.capture(&SpawnSpec::from(Invocation::new("hg").with_arg("status")));

    assert!(matches!(outcome, Outcome::Ran { io, .. } if io.stdout.is_empty()));
}

#[test]
fn a_missing_program_is_scripted_once_and_answers_in_every_mode() {
    // The `DevpodNotInstalled` case: whichever way `dl` reaches for devpod, it
    // finds out that there is no devpod rather than reading a failed run.
    let fake = FakeRunner::new().with_missing("devpod");

    assert_eq!(fake.capture(&devpod(&["list"])), Outcome::ProgramNotFound);
    assert_eq!(
        fake.passthrough(&devpod(&["up", "/clones/ws"])),
        Outcome::ProgramNotFound
    );
    assert_eq!(
        fake.session(&devpod(&["ssh", "ws"]), &mut |_| {}),
        Outcome::ProgramNotFound
    );
    assert_eq!(
        fake.detach(&Invocation::new("devpod")),
        DetachOutcome::ProgramNotFound
    );
}

#[test]
fn a_timeout_and_an_os_refusal_are_scriptable_too() {
    let refusal = OsFailure {
        kind: std::io::ErrorKind::PermissionDenied,
        errno: Some(13),
    };
    let fake = FakeRunner::new()
        .with_script(["git", "fetch"], Response::TimedOut)
        .with_script(["git", "clone"], Response::NotStarted(refusal));

    assert_eq!(
        fake.capture(&SpawnSpec::from(Invocation::new("git").with_arg("fetch"))),
        Outcome::TimedOut
    );
    assert_eq!(
        fake.capture(&SpawnSpec::from(Invocation::new("git").with_arg("clone"))),
        Outcome::NotStarted(refusal)
    );
}

#[test]
fn a_script_short_circuits_the_state_machine() {
    // The failure-injection channel has to win, or a scripted `up` failure would
    // still leave a workspace behind and the test would prove nothing.
    let fake =
        FakeRunner::new().with_script(["devpod", "up"], Response::failed(1, "build failed\n"));
    fake.passthrough(&devpod(&["up", "/clones/ws", "--id", "ws"]));

    assert_eq!(fake.workspace_ids(), Vec::<String>::new());
    assert_eq!(fake.state_of("ws"), None);
}

#[test]
fn scripts_can_be_cleared_and_replaced_mid_test() {
    let fake = FakeRunner::new().with_script(["devpod", "up"], Response::failed(1, "boom\n"));
    assert!(!fake.passthrough(&devpod(&["up", "/clones/ws"])).succeeded());

    fake.clear_scripts();
    assert!(fake.passthrough(&devpod(&["up", "/clones/ws"])).succeeded());
    assert_eq!(fake.workspace_ids(), ["ws"]);
}

// ----------------------------------------------------------- what each mode gets

#[test]
fn a_captured_call_reads_the_text_and_a_passthrough_call_does_not() {
    let fake = FakeRunner::new().with_script(
        ["devpod", "version"],
        Response::stdout("devpod version v0.26.1\n"),
    );

    let captured = fake.capture(&devpod(&["version"]));
    assert!(matches!(captured, Outcome::Ran { io, .. } if io.stdout.contains("v0.26.1")));
    // Nothing to carry: the type has no room for output that was never read.
    assert_eq!(
        fake.passthrough(&devpod(&["version"])),
        Outcome::Ran {
            exit: Exit::Code(0),
            io: ()
        }
    );
}

#[test]
fn a_session_is_handed_the_scripted_stderr_a_line_at_a_time() {
    let fake = FakeRunner::new().with_script(
        ["devpod", "ssh"],
        Response::exited(1).and_stderr(
            "error Try using the --debug flag to see a more verbose output\n\
             fatal ssh session: Process exited with status 130\n",
        ),
    );

    let mut lines = Vec::new();
    let outcome = fake.session(&devpod(&["ssh", "ws"]), &mut |line| {
        lines.push(line.to_string())
    });

    assert_eq!(
        outcome,
        Outcome::Ran {
            exit: Exit::Code(1),
            io: ()
        }
    );
    assert_eq!(
        lines,
        [
            "error Try using the --debug flag to see a more verbose output",
            "fatal ssh session: Process exited with status 130",
        ]
    );
}

#[test]
fn a_detached_spawn_starts_and_each_call_gets_its_own_pid() {
    let fake = FakeRunner::new();
    let first = fake.detach(&Invocation::new("dl").with_arg("--update-cache"));
    let second = fake.detach(&Invocation::new("dl").with_arg("--update-cache"));

    match (first, second) {
        (DetachOutcome::Started { pid: one }, DetachOutcome::Started { pid: other }) => {
            assert_ne!(one, other)
        }
        other => panic!("expected two started children, got {other:?}"),
    }
    assert_eq!(fake.call_count(), 2);
}

// -------------------------------------------------------- unanticipated spawns

#[test]
fn a_spawn_nothing_scripted_succeeds_quietly_by_default() {
    let fake = FakeRunner::new();
    let outcome = fake.capture(&SpawnSpec::from(
        Invocation::new("gh").with_args(["auth", "token"]),
    ));

    assert_eq!(
        outcome,
        Outcome::Ran {
            exit: Exit::Code(0),
            io: CapturedText::default()
        }
    );
}

#[test]
#[should_panic(expected = "nothing scripted this spawn")]
fn a_strict_fake_refuses_a_spawn_nothing_scripted() {
    let fake = FakeRunner::new().with_unscripted(Unscripted::Panic);
    fake.capture(&SpawnSpec::from(Invocation::new("gh").with_arg("pr")));
}

#[test]
fn strictness_still_lets_the_devpod_machine_answer() {
    let fake = FakeRunner::new()
        .with_unscripted(Unscripted::Panic)
        .with_stopped("ws");
    assert!(fake.capture(&devpod(&["status", "ws"])).succeeded());
}

// ------------------------------------------------------------------ the spec

#[test]
fn the_fake_is_usable_behind_a_trait_object() {
    // Which is how core will hold it: one runner, injected.
    let fake = FakeRunner::new().with_running("ws");
    let runner: &dyn Runner = &fake;
    assert!(runner.capture(&devpod(&["status", "ws"])).succeeded());
    assert_eq!(fake.args_to("devpod"), [["status", "ws"]]);
}

#[test]
fn a_default_fake_knows_docker_and_nothing_else() {
    let fake = FakeRunner::default();
    assert_eq!(fake.provider_names(), ["docker"]);
    assert_eq!(fake.workspace_ids(), Vec::<String>::new());
    assert_eq!(fake.workspace("ws").map(|workspace| workspace.state), None);
    assert_eq!(
        EnvSpec::default().base,
        devpod(&["list"]).invocation.env.base
    );
    assert_eq!(devpod(&["list"]).stdin, StdinPlan::Inherit);
}
