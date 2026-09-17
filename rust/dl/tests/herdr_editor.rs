use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn fake_herdr(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("herdr");
    fs::write(
        &path,
        r##"#!/bin/sh
printf '<%s>' "$@" >> "$HERDR_CALLS"; echo >> "$HERDR_CALLS"
case "$*" in
  "agent get w1:p1") printf '%s\n' '{"result":{"agent":{"agent":"codex","agent_status":"working","state_change_seq":42}}}' ;;
  "pane layout --pane w1:p1") printf '%s\n' '{"result":{"layout":{"panes":[{"pane_id":"w1:p1"}]}}}' ;;
  "pane split w1:p1 --direction right --no-focus") printf '%s\n' '{"result":{"pane":{"pane_id":"w1:p2"}}}' ;;
  "pane run w1:p2 nvim") printf '%s\n' '{"result":{}}' ;;
  "pane run w1:p2 nvim -p") printf '%s\n' '{"result":{}}' ;;
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
        "#!/bin/sh\nprintf '<%s>' \"$@\" >> \"$HERDR_CALLS\"; echo >> \"$HERDR_CALLS\"\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn fake_replacing_agent(dir: &std::path::Path) {
    let path = dir.join("herdr");
    fs::write(
        &path,
        r##"#!/bin/sh
printf '<%s>' "$@" >> "$HERDR_CALLS"; echo >> "$HERDR_CALLS"
case "$*" in
  "agent get w1:p1")
    if [ ! -e "$HERDR_SECOND_AGENT" ]; then
      : > "$HERDR_SECOND_AGENT"
      printf '%s\n' '{"result":{"agent":{"agent":"claude","agent_status":"done","state_change_seq":41}}}'
    else
      printf '%s\n' '{"result":{"agent":{"agent":"codex","agent_status":"working","state_change_seq":42}}}'
    fi ;;
  "pane layout --pane w1:p1") printf '%s\n' '{"result":{"layout":{"panes":[{"pane_id":"w1:p1"}]}}}' ;;
  "pane split w1:p1 --direction right --no-focus") printf '%s\n' '{"result":{"pane":{"pane_id":"w1:p2"}}}' ;;
  "pane run w1:p2 nvim") printf '%s\n' '{"result":{}}' ;;
  *) exit 1 ;;
esac
"##,
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
        .env("DEVLAUNCH_HERDR_SPLIT_EXPECTED_AGENT", "codex")
        .env("NVIM_SPLIT", "1")
        .env("VISUAL", "nvim");
    command
}

fn drive(mut command: Command) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _lease = child.stdin.take().unwrap();
    child.wait_with_output().unwrap()
}

fn run(dir: &std::path::Path) -> Output {
    drive(command(dir))
}

fn opened_the_editor(dir: &std::path::Path, editor: &str) {
    assert_eq!(
        fs::read_to_string(dir.join("calls")).unwrap_or_default(),
        format!(
            "<agent><get><w1:p1>\n\
             <pane><layout><--pane><w1:p1>\n\
             <pane><split><w1:p1><--direction><right><--no-focus>\n\
             <pane><run><w1:p2><{editor}>\n"
        )
    );
}

#[test]
fn starts_an_editor_to_the_right_without_taking_agent_focus() {
    let dir = tempfile::tempdir().unwrap();
    fake_herdr(dir.path());
    let output = run(dir.path());
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(dir.path().join("calls")).unwrap(),
        "<agent><get><w1:p1>\n\
         <pane><layout><--pane><w1:p1>\n\
         <pane><split><w1:p1><--direction><right><--no-focus>\n\
         <pane><run><w1:p2><nvim>\n"
    );
}

#[test]
fn a_visual_with_arguments_runs_the_whole_command() {
    let dir = tempfile::tempdir().unwrap();
    fake_herdr(dir.path());
    let mut configured = command(dir.path());
    configured.env("VISUAL", "nvim -p");
    let output = drive(configured);
    assert!(output.status.success(), "{output:?}");
    opened_the_editor(dir.path(), "nvim -p");
}

#[test]
fn a_visual_that_cannot_be_run_falls_through_to_editor() {
    for visual in ["", "   ", "nvim\nrm -rf /"] {
        let dir = tempfile::tempdir().unwrap();
        fake_herdr(dir.path());
        let mut configured = command(dir.path());
        configured.env("VISUAL", visual).env("EDITOR", "nvim");
        let output = drive(configured);
        assert!(output.status.success(), "{visual:?} {output:?}");
        opened_the_editor(dir.path(), "nvim");
    }
}

#[test]
fn neither_variable_set_still_opens_nvim() {
    let dir = tempfile::tempdir().unwrap();
    fake_herdr(dir.path());
    let mut configured = command(dir.path());
    configured.env_remove("VISUAL").env_remove("EDITOR");
    let output = drive(configured);
    assert!(output.status.success(), "{output:?}");
    opened_the_editor(dir.path(), "nvim");
}

#[test]
fn is_off_by_default_even_when_an_editor_is_configured() {
    let dir = tempfile::tempdir().unwrap();
    fake_herdr(dir.path());
    let output = command(dir.path())
        .env_remove("NVIM_SPLIT")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(!dir.path().join("calls").exists());
}

#[test]
fn waits_for_a_new_matching_agent_instead_of_accepting_the_old_done_one() {
    let dir = tempfile::tempdir().unwrap();
    fake_replacing_agent(dir.path());
    let mut command = command(dir.path());
    command
        .env("HERDR_SECOND_AGENT", dir.path().join("second-agent"))
        .env(
            "DEVLAUNCH_HERDR_SPLIT_BASELINE",
            r#"{"kind":"claude","state_change_seq":41,"activity":"done"}"#,
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let _lease = child.stdin.take().unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(dir.path().join("calls")).unwrap(),
        "<agent><get><w1:p1>\n\
         <agent><get><w1:p1>\n\
         <pane><layout><--pane><w1:p1>\n\
         <pane><split><w1:p1><--direction><right><--no-focus>\n\
         <pane><run><w1:p2><nvim>\n"
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
            .starts_with("<agent><get><w1:p1>\n")
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

fn fake_wedged_herdr(dir: &std::path::Path) {
    let path = dir.join("herdr");
    fs::write(
        &path,
        "#!/bin/sh\nprintf '<%s>' \"$@\" >> \"$HERDR_CALLS\"; echo >> \"$HERDR_CALLS\"\nsleep 30\n",
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn wait_until(deadline: Instant, mut done: impl FnMut() -> bool) -> bool {
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    done()
}

#[test]
fn gives_up_on_a_herdr_that_takes_the_question_and_never_answers() {
    let dir = tempfile::tempdir().unwrap();
    fake_wedged_herdr(dir.path());
    let calls = dir.path().join("calls");
    let mut child = command(dir.path()).stdin(Stdio::piped()).spawn().unwrap();
    let lease = child.stdin.take().unwrap();

    let asked = wait_until(Instant::now() + Duration::from_secs(5), || {
        fs::read_to_string(&calls)
            .unwrap_or_default()
            .contains("<agent><get><w1:p1>")
    });
    if !asked {
        child.kill().unwrap();
        child.wait().unwrap();
        panic!("the waiter never asked herdr anything");
    }
    drop(lease);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{status:?}");
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("a herdr that never answers left the editor waiter blocked forever");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
