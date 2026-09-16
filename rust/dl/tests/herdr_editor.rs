use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

fn fake_herdr(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("herdr");
    fs::write(
        &path,
        r##"#!/bin/sh
printf '%s\n' "$*" >> "$HERDR_CALLS"
case "$*" in
  "agent get w1:p1") printf '%s\n' '{"result":{"agent":{"pane_id":"w1:p1"}}}' ;;
  "pane layout --pane w1:p1") printf '%s\n' '{"result":{"layout":{"panes":[{"pane_id":"w1:p1"}]}}}' ;;
  "pane split w1:p1 --direction right --no-focus") printf '%s\n' '{"result":{"pane":{"pane_id":"w1:p2"}}}' ;;
  "pane run w1:p2 nvim") printf '%s\n' '{"result":{}}' ;;
  *) exit 1 ;;
esac
"##,
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn run(dir: &std::path::Path) -> Output {
    let calls = dir.join("calls");
    Command::new(env!("CARGO_BIN_EXE_dl"))
        .arg("--herdr-editor-ready")
        .env_clear()
        .env("HOME", dir)
        .env("PATH", format!("{}:/usr/bin:/bin", dir.display()))
        .env("HERDR_ENV", "1")
        .env("HERDR_PANE_ID", "w1:p1")
        .env("HERDR_CALLS", calls)
        .env("DEVLAUNCH_HERDR_EDITOR", "nvim")
        .output()
        .unwrap()
}

#[test]
fn starts_an_editor_to_the_right_without_taking_agent_focus() {
    let dir = tempfile::tempdir().unwrap();
    fake_herdr(dir.path());
    let output = run(dir.path());
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(dir.path().join("calls")).unwrap(),
        "agent get w1:p1\n\
         pane layout --pane w1:p1\n\
         pane split w1:p1 --direction right --no-focus\n\
         pane run w1:p2 nvim\n"
    );
}
