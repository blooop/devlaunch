use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

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

fn fake_waiting_herdr(dir: &std::path::Path) {
    let path = dir.join("herdr");
    fs::write(
        &path,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$HERDR_CALLS\"\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn command(dir: &std::path::Path) -> Command {
    let calls = dir.join("calls");
    let mut command = Command::new(env!("CARGO_BIN_EXE_dl"));
    command
        .arg("--herdr-editor-ready")
        .env_clear()
        .env("HOME", dir)
        .env("PATH", format!("{}:/usr/bin:/bin", dir.display()))
        .env("HERDR_ENV", "1")
        .env("HERDR_BIN_PATH", dir.join("herdr"))
        .env("HERDR_PANE_ID", "w1:p1")
        .env("HERDR_CALLS", calls)
        .env("DEVLAUNCH_HERDR_EDITOR", "nvim");
    command
}

fn run(dir: &std::path::Path) -> Output {
    let mut child = command(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _lease = child.stdin.take().unwrap();
    child.wait_with_output().unwrap()
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

#[test]
fn uses_herdrs_exported_binary_and_repairs_a_deleted_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let binary = fake_herdr(dir.path());
    let mut child = command(dir.path())
        .env("PATH", "/usr/bin:/bin")
        .env("HERDR_BIN_PATH", format!("{} (deleted)", binary.display()))
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let _lease = child.stdin.take().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("editor waiter ignored HERDR_BIN_PATH and kept polling");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(), "{status:?}");
    assert!(
        fs::read_to_string(dir.path().join("calls"))
            .unwrap()
            .starts_with("agent get w1:p1\n")
    );
}

#[test]
fn stops_waiting_when_the_initiating_process_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    fake_waiting_herdr(dir.path());
    let mut child = command(dir.path()).stdin(Stdio::piped()).spawn().unwrap();
    drop(child.stdin.take());

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{status:?}");
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("editor waiter outlived the process that initiated it");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
