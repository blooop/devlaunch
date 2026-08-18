//! Every transition of the fake devpod, and the shapes it answers in.
//!
//! Driven through [`DevpodMachine`] directly rather than through the fake
//! runner: this is the machine's own contract, and the runner's tests cover the
//! wiring. The fidelity claims — no state field in a listing, `ssh` starting a
//! stopped workspace, a refusal for an unknown id — are checked here because
//! they are the ones a port could pass tests without honouring.

use super::*;
use devlaunch_runner::Exit;

/// One devpod call, spelled the way a caller does.
fn call(machine: &mut DevpodMachine, args: &[&str]) -> Response {
    machine.answer(&args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>())
}

fn stdout_of(response: &Response) -> String {
    match response {
        Response::Ran { stdout, .. } => stdout.clone(),
        other => panic!("expected a run, got {other:?}"),
    }
}

fn stderr_of(response: &Response) -> String {
    match response {
        Response::Ran { stderr, .. } => stderr.clone(),
        other => panic!("expected a run, got {other:?}"),
    }
}

fn json_of(response: &Response) -> serde_json::Value {
    serde_json::from_str(&stdout_of(response)).expect("devpod answers JSON when asked for it")
}

fn failed_with(response: &Response, code: i32) {
    match response {
        Response::Ran {
            exit: Exit::Code(actual),
            ..
        } => assert_eq!(*actual, code),
        other => panic!("expected exit {code}, got {other:?}"),
    }
}

// ------------------------------------------------------------------------- up

#[test]
fn up_creates_a_running_workspace() {
    let mut machine = DevpodMachine::new();
    let response = call(
        &mut machine,
        &["up", "/clones/owner/repo", "--id", "owner-repo-main"],
    );

    assert_eq!(stdout_of(&response), "Workspace owner-repo-main is ready\n");
    assert_eq!(
        machine.state_of("owner-repo-main"),
        Some(WorkspaceState::Running)
    );
    assert_eq!(
        machine.get("owner-repo-main").map(|w| w.source.clone()),
        Some(Source::LocalFolder("/clones/owner/repo".to_string()))
    );
}

#[test]
fn up_without_a_source_refuses() {
    let mut machine = DevpodMachine::new();
    let response = call(&mut machine, &["up"]);
    failed_with(&response, 1);
    assert!(stderr_of(&response).contains("no workspace source given"));
}

#[test]
fn up_derives_an_id_from_the_source_when_it_is_given_none() {
    let mut machine = DevpodMachine::new();
    call(
        &mut machine,
        &["up", "https://github.com/Owner/My.Repo.git"],
    );
    assert_eq!(machine.ids(), ["my-repo"]);
    assert_eq!(
        machine.get("my-repo").map(|w| w.source.clone()),
        Some(Source::GitRepository(
            "https://github.com/Owner/My.Repo.git".to_string()
        ))
    );
}

#[test]
fn up_on_a_stopped_workspace_addressed_by_id_restarts_it() {
    let mut machine = DevpodMachine::new();
    machine.insert(FakeWorkspace::new(
        "ws",
        Source::LocalFolder("/clones/ws".to_string()),
        WorkspaceState::Stopped,
    ));
    call(&mut machine, &["up", "ws"]);

    assert_eq!(machine.ids(), ["ws"], "no second workspace was invented");
    assert_eq!(machine.state_of("ws"), Some(WorkspaceState::Running));
}

#[test]
fn up_stamps_last_used() {
    let mut machine = DevpodMachine::new();
    machine.stamp = "2026-08-18T09:00:00+0000".to_string();
    call(&mut machine, &["up", "/clones/ws", "--id", "ws"]);
    assert_eq!(
        machine.get("ws").map(|w| w.last_used.clone()),
        Some("2026-08-18T09:00:00+0000".to_string())
    );
}

// ---------------------------------------------------------------- stop, delete

#[test]
fn stop_stops_a_running_workspace() {
    let mut machine = with_workspace(WorkspaceState::Running);
    let response = call(&mut machine, &["stop", "ws"]);
    failed_with(&response, 0);
    assert_eq!(machine.state_of("ws"), Some(WorkspaceState::Stopped));
}

#[test]
fn stopping_a_workspace_devpod_never_heard_of_refuses() {
    let mut machine = DevpodMachine::new();
    let response = call(&mut machine, &["stop", "ghost"]);
    failed_with(&response, 1);
    assert!(stderr_of(&response).contains("couldn't find workspace ghost"));
}

#[test]
fn delete_removes_the_workspace() {
    let mut machine = with_workspace(WorkspaceState::Running);
    let response = call(&mut machine, &["delete", "ws"]);
    assert_eq!(stdout_of(&response), "Successfully deleted workspace ws\n");
    assert_eq!(machine.ids(), Vec::<String>::new());
    assert_eq!(machine.state_of("ws"), None);
}

#[test]
fn deleting_a_workspace_devpod_never_heard_of_refuses() {
    let mut machine = DevpodMachine::new();
    let response = call(&mut machine, &["delete", "ghost"]);
    failed_with(&response, 1);
    assert!(stderr_of(&response).contains("couldn't find workspace ghost"));
}

// ---------------------------------------------------------------------- list

#[test]
fn a_json_listing_carries_no_state_field() {
    // Real devpod answers state only to `status`, per workspace. A listing that
    // carried it would let a port read a field devpod never sends.
    let mut machine = with_workspace(WorkspaceState::Running);
    let listing = json_of(&call(&mut machine, &["list", "--output", "json"]));
    let entry = &listing.as_array().expect("an array of workspaces")[0];

    assert!(entry.get("state").is_none(), "{entry}");
    assert_eq!(entry["id"], "ws");
    assert_eq!(entry["source"]["localFolder"], "/clones/ws");
    assert_eq!(entry["provider"]["name"], "docker");
    assert_eq!(entry["lastUsed"], DEFAULT_STAMP);
}

#[test]
fn a_json_listing_lists_every_workspace_and_an_empty_one_is_an_answer() {
    let mut machine = DevpodMachine::new();
    assert_eq!(
        json_of(&call(&mut machine, &["list", "--output", "json"])),
        serde_json::json!([])
    );

    machine.insert(FakeWorkspace::new(
        "b",
        Source::GitRepository("https://example.com/b".to_string()),
        WorkspaceState::Stopped,
    ));
    machine.insert(FakeWorkspace::new(
        "a",
        Source::LocalFolder("/clones/a".to_string()),
        WorkspaceState::Running,
    ));
    let listing = json_of(&call(&mut machine, &["list", "--output", "json"]));
    let ids: Vec<&str> = listing
        .as_array()
        .expect("an array")
        .iter()
        .map(|entry| entry["id"].as_str().expect("an id"))
        .collect();
    assert_eq!(ids, ["a", "b"]);
    assert_eq!(
        listing[1]["source"]["gitRepository"],
        "https://example.com/b"
    );
}

#[test]
fn a_plain_listing_is_a_table() {
    let mut machine = with_workspace(WorkspaceState::Running);
    assert_eq!(
        stdout_of(&call(&mut machine, &["list"])),
        format!("ws  docker  {DEFAULT_STAMP}\n")
    );
}

// -------------------------------------------------------------------- status

#[test]
fn status_answers_the_state_of_one_workspace() {
    let mut machine = with_workspace(WorkspaceState::Stopped);
    let answer = json_of(&call(&mut machine, &["status", "ws", "--output", "json"]));
    assert_eq!(
        answer,
        serde_json::json!({
            "id": "ws",
            "context": "default",
            "provider": "docker",
            "state": "Stopped",
        })
    );

    machine.set_state("ws", WorkspaceState::Running);
    let answer = json_of(&call(&mut machine, &["status", "ws", "--output", "json"]));
    assert_eq!(answer["state"], "Running");
}

#[test]
fn a_plain_status_is_a_sentence() {
    let mut machine = with_workspace(WorkspaceState::Running);
    assert_eq!(
        stdout_of(&call(&mut machine, &["status", "ws"])),
        "Workspace ws is Running\n"
    );
}

#[test]
fn the_status_of_a_workspace_devpod_never_heard_of_refuses() {
    let mut machine = DevpodMachine::new();
    let response = call(&mut machine, &["status", "ghost", "--output", "json"]);
    failed_with(&response, 1);
    assert!(stderr_of(&response).contains("couldn't find workspace ghost"));
}

// ----------------------------------------------------------------------- ssh

#[test]
fn ssh_starts_a_stopped_workspace() {
    // Real devpod does, which is what lets dl attach without an `up` of its own.
    let mut machine = with_workspace(WorkspaceState::Stopped);
    let response = call(&mut machine, &["ssh", "ws", "--command", "echo hi"]);
    failed_with(&response, 0);
    assert_eq!(machine.state_of("ws"), Some(WorkspaceState::Running));
}

#[test]
fn ssh_into_a_workspace_devpod_never_heard_of_refuses() {
    let mut machine = DevpodMachine::new();
    let response = call(&mut machine, &["ssh", "ghost"]);
    failed_with(&response, 1);
    assert!(stderr_of(&response).contains("couldn't find workspace ghost"));
}

// ------------------------------------------------------- providers and context

#[test]
fn provider_list_answers_an_object_keyed_by_name() {
    let mut machine = DevpodMachine::new();
    let listing = json_of(&call(
        &mut machine,
        &["provider", "list", "--output", "json"],
    ));
    assert_eq!(listing["docker"]["config"]["name"], "docker");
}

#[test]
fn a_provider_can_be_added_and_then_used() {
    let mut machine = DevpodMachine::new();
    failed_with(&call(&mut machine, &["provider", "use", "kubernetes"]), 1);
    failed_with(&call(&mut machine, &["provider", "add", "kubernetes"]), 0);
    assert_eq!(machine.provider_names(), ["docker", "kubernetes"]);
    failed_with(&call(&mut machine, &["provider", "use", "kubernetes"]), 0);
}

#[test]
fn context_options_answers_json() {
    let mut machine = DevpodMachine::new();
    assert_eq!(
        json_of(&call(&mut machine, &["context", "options"])),
        serde_json::json!({})
    );
    failed_with(&call(&mut machine, &["context", "nonsense"]), 1);
}

// ------------------------------------------------------- the edges of the argv

#[test]
fn version_answers_a_version() {
    let mut machine = DevpodMachine::new();
    assert!(stdout_of(&call(&mut machine, &["version"])).starts_with("devpod version "));
}

#[test]
fn a_command_devpod_does_not_know_refuses() {
    let mut machine = DevpodMachine::new();
    let response = call(&mut machine, &["teleport", "ws"]);
    failed_with(&response, 1);
    assert!(stderr_of(&response).contains("unknown command \"teleport\""));
}

#[test]
fn no_command_at_all_refuses() {
    let mut machine = DevpodMachine::new();
    let response = call(&mut machine, &[]);
    failed_with(&response, 1);
    assert!(stderr_of(&response).contains("no command given"));
}

// -------------------------------------------------------------- the classifier

#[test]
fn a_source_is_recorded_the_way_devpod_records_it() {
    assert_eq!(
        Source::classify("/home/user/clones/repo"),
        Source::LocalFolder("/home/user/clones/repo".to_string())
    );
    assert_eq!(
        Source::classify("./repo"),
        Source::LocalFolder("./repo".to_string())
    );
    assert_eq!(
        Source::classify("https://github.com/owner/repo"),
        Source::GitRepository("https://github.com/owner/repo".to_string())
    );
    assert_eq!(
        Source::classify("git@github.com:owner/repo.git"),
        Source::GitRepository("git@github.com:owner/repo.git".to_string())
    );
}

#[test]
fn a_derived_id_is_squeezed_the_way_devpod_squeezes_it() {
    assert_eq!(derive_id("/clones/owner/My_Repo/"), "my-repo");
    assert_eq!(derive_id("https://github.com/owner/repo.git"), "repo");
    assert_eq!(derive_id("---"), "workspace");
}

fn with_workspace(state: WorkspaceState) -> DevpodMachine {
    let mut machine = DevpodMachine::new();
    machine.insert(FakeWorkspace::new(
        "ws",
        Source::LocalFolder("/clones/ws".to_string()),
        state,
    ));
    machine
}
