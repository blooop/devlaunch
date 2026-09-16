//! An editor beside an agent launched through `dl` in Herdr.
//!
//! The agent has to start first. A split made before the transport exists has no
//! live devlaunch sibling for `dl-herdr-shell` to inspect, so it opens on the host
//! instead of in the container. The parent therefore starts this short-lived
//! helper, which waits until Herdr recognises the agent, splits that pane without
//! taking focus, and starts the configured editor in the new pane.

use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

pub(crate) const EDITOR_VAR: &str = "DEVLAUNCH_HERDR_EDITOR";
const AGENT_VAR: &str = "HERDR_AGENT";
const READY_WORD: &str = "--herdr-editor-ready";
const WAIT_FOR_AGENT: Duration = Duration::from_secs(30 * 60);
const RETRY: Duration = Duration::from_millis(500);

/// Start the detached waiter when this process has enough context to do so.
pub(crate) fn start() {
    if std::env::var("HERDR_ENV").as_deref() != Ok("1")
        || std::env::var_os("HERDR_PANE_ID").is_none()
        || editor().is_none()
    {
        return;
    }
    let Ok(me) = std::env::current_exe() else {
        return;
    };
    let _ = waiter_command(me).spawn();
}

fn waiter_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command
        .arg(READY_WORD)
        // `aid` sets this before entering `dl`. Letting the waiter inherit it
        // makes the waiter itself satisfy the agent probe before a transport exists.
        .env_remove(AGENT_VAR)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// The internal re-entry: wait for the agent and build its two-pane tab.
pub(crate) fn ready() {
    let Some(editor) = editor() else { return };
    let Ok(pane) = std::env::var("HERDR_PANE_ID") else {
        return;
    };
    let deadline = Instant::now() + WAIT_FOR_AGENT;
    while Instant::now() < deadline {
        if herdr(["agent", "get", pane.as_str()]).is_some() {
            break;
        }
        thread::sleep(RETRY);
    }
    if Instant::now() >= deadline {
        return;
    }

    let Some(layout) = herdr(["pane", "layout", "--pane", pane.as_str()]) else {
        return;
    };
    if pane_count(&layout) != Some(1) {
        return;
    }
    let Some(split) = herdr([
        "pane",
        "split",
        pane.as_str(),
        "--direction",
        "right",
        "--no-focus",
    ]) else {
        return;
    };
    let Some(editor_pane) = split_pane(&split) else {
        return;
    };

    let editor_deadline = Instant::now() + WAIT_FOR_AGENT;
    // A devlaunch sibling makes the new pane enter the same container, but that
    // attach can take a moment. `pane run` refuses a busy pane without typing
    // into it, so retry until its shell owns the foreground.
    while Instant::now() < editor_deadline {
        if herdr(["pane", "run", editor_pane.as_str(), editor.as_str()]).is_some() {
            return;
        }
        thread::sleep(RETRY);
    }
}

fn editor() -> Option<String> {
    let value = std::env::var(EDITOR_VAR).ok()?;
    let value = value.trim();
    (!value.is_empty() && !value.chars().any(char::is_whitespace)).then(|| value.to_owned())
}

fn herdr<const N: usize>(args: [&str; N]) -> Option<Value> {
    let output = Command::new("herdr")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| serde_json::from_slice(&output.stdout).unwrap_or(Value::Null))
}

fn pane_count(response: &Value) -> Option<usize> {
    response
        .pointer("/result/layout/panes")?
        .as_array()
        .map(Vec::len)
}

fn split_pane(response: &Value) -> Option<String> {
    response
        .pointer("/result/pane/pane_id")?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_layout_and_split_response_shapes() {
        let layout = serde_json::json!({"result": {"layout": {"panes": [{"pane_id": "w1:p1"}]}}});
        let split = serde_json::json!({"result": {"pane": {"pane_id": "w1:p2"}}});
        assert_eq!(pane_count(&layout), Some(1));
        assert_eq!(split_pane(&split).as_deref(), Some("w1:p2"));
    }

    #[test]
    fn the_waiter_does_not_advertise_itself_as_the_agent() {
        let command = waiter_command("/bin/true");
        let marker = command
            .get_envs()
            .find(|(name, _)| *name == std::ffi::OsStr::new(AGENT_VAR));
        assert_eq!(marker, Some((std::ffi::OsStr::new(AGENT_VAR), None)));
    }
}
