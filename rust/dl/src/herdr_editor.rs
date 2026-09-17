//! An editor beside an agent launched through `dl` in Herdr.
//!
//! The agent has to start first. A split made before the transport exists has no
//! live devlaunch sibling for `dl-herdr-shell` to inspect, so it opens on the host
//! instead of in the container. The parent therefore starts this short-lived
//! helper, which waits until Herdr recognises the agent, splits that pane without
//! taking focus, and starts the shell's configured editor in the new pane.

use std::io::{self, Read as _};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const SPLIT_VAR: &str = "NVIM_SPLIT";
const AGENT_VAR: &str = "HERDR_AGENT";
const EXPECTED_AGENT_VAR: &str = "DEVLAUNCH_HERDR_SPLIT_EXPECTED_AGENT";
const BASELINE_VAR: &str = "DEVLAUNCH_HERDR_SPLIT_BASELINE";
const READY_WORD: &str = "--herdr-editor-ready";
const WAIT_FOR_AGENT: Duration = Duration::from_secs(30 * 60);
const RETRY: Duration = Duration::from_millis(500);
static PARENT_LEASES: Mutex<Vec<ChildStdin>> = Mutex::new(Vec::new());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AgentKind(&'static str);

impl AgentKind {
    pub(crate) fn from_program(program: &str) -> Option<Self> {
        devlaunch_core::clients::herdr_agent_named(program).map(Self)
    }

    fn parse(name: &str) -> Option<Self> {
        Self::from_program(name).filter(|known| known.0 == name)
    }

    pub(crate) fn as_str(self) -> &'static str {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Activity {
    Live,
    Done,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct AgentObservation {
    kind: String,
    // Herdr's global monotonic sequence distinguishes a new session even when
    // the pane starts the same kind of agent twice.
    state_change_seq: u64,
    activity: Activity,
}

impl AgentObservation {
    fn from_response(response: &Value) -> Option<Self> {
        let agent = response.pointer("/result/agent")?;
        let kind = agent.get("agent")?.as_str()?.to_owned();
        let state_change_seq = agent.get("state_change_seq")?.as_u64()?;
        let activity = match agent.get("agent_status")?.as_str()? {
            "done" => Activity::Done,
            "idle" | "working" | "blocked" => Activity::Live,
            _ => return None,
        };
        Some(Self {
            kind,
            state_change_seq,
            activity,
        })
    }

    fn is_new_live_agent(&self, expected: AgentKind, baseline: Option<&Self>) -> bool {
        self.kind == expected.as_str()
            && self.activity == Activity::Live
            && baseline.is_none_or(|old| self.state_change_seq > old.state_change_seq)
    }
}

/// Start the detached waiter when this process has enough context to do so.
pub(crate) fn start(expected: AgentKind) {
    if std::env::var("HERDR_ENV").as_deref() != Ok("1") || editor().is_none() {
        return;
    }
    let Ok(pane) = std::env::var("HERDR_PANE_ID") else {
        return;
    };
    // Completed agents remain queryable in their pane. Carry what was there
    // before this launch so the waiter cannot mistake it for the new transport.
    let baseline = agent_observation(&pane);
    let Ok(me) = std::env::current_exe() else {
        return;
    };
    let Some(mut command) = waiter_command(me, expected, baseline.as_ref()) else {
        return;
    };
    let Ok(mut waiter) = command.spawn() else {
        return;
    };
    if let Some(lease) = waiter.stdin.take() {
        // This process is the lease. The kernel closes every retained writer when
        // it exits, which cancels the waiter without addressing a possibly reused PID.
        PARENT_LEASES
            .lock()
            .expect("lease lock poisoned")
            .push(lease);
    }
}

fn waiter_command(
    program: impl AsRef<std::ffi::OsStr>,
    expected: AgentKind,
    baseline: Option<&AgentObservation>,
) -> Option<Command> {
    let mut command = Command::new(program);
    command
        .arg(READY_WORD)
        // `aid` sets this before entering `dl`. Letting the waiter inherit it
        // makes the waiter itself satisfy the agent probe before a transport exists.
        .env_remove(AGENT_VAR)
        .env(EXPECTED_AGENT_VAR, expected.as_str())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match baseline {
        Some(observation) => {
            command.env(BASELINE_VAR, serde_json::to_string(observation).ok()?);
        }
        None => {
            command.env_remove(BASELINE_VAR);
        }
    }
    Some(command)
}

/// The internal re-entry: wait for the agent and build its two-pane tab.
pub(crate) fn ready() {
    let Some(editor) = editor() else { return };
    let Some(expected) = std::env::var(EXPECTED_AGENT_VAR)
        .ok()
        .as_deref()
        .and_then(AgentKind::parse)
    else {
        return;
    };
    let baseline = match std::env::var(BASELINE_VAR) {
        Ok(raw) => match serde_json::from_str::<AgentObservation>(&raw) {
            Ok(observation) => Some(observation),
            Err(_) => return,
        },
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => return,
    };
    let Ok(pane) = std::env::var("HERDR_PANE_ID") else {
        return;
    };
    let parent = parent_lease();
    let deadline = Instant::now() + WAIT_FOR_AGENT;
    while Instant::now() < deadline {
        if lease_closed(&parent) {
            return;
        }
        if agent_observation(&pane)
            .is_some_and(|current| current.is_new_live_agent(expected, baseline.as_ref()))
        {
            break;
        }
        if !retry_while_parent_lives(&parent) {
            return;
        }
    }
    if Instant::now() >= deadline || lease_closed(&parent) {
        return;
    }

    let Some(layout) = herdr(["pane", "layout", "--pane", pane.as_str()]) else {
        return;
    };
    if lease_closed(&parent) || pane_count(&layout) != Some(1) {
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
        if lease_closed(&parent) {
            return;
        }
        if herdr(["pane", "run", editor_pane.as_str(), editor.as_str()]).is_some() {
            return;
        }
        if !retry_while_parent_lives(&parent) {
            return;
        }
    }
}

fn agent_observation(pane: &str) -> Option<AgentObservation> {
    let response = herdr(["agent", "get", pane])?;
    AgentObservation::from_response(&response)
}

fn parent_lease() -> Receiver<()> {
    let (closed, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut input = io::stdin();
        let mut byte = [0_u8; 1];
        loop {
            match input.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = closed.send(());
    });
    receiver
}

fn lease_closed(parent: &Receiver<()>) -> bool {
    !matches!(parent.try_recv(), Err(TryRecvError::Empty))
}

fn retry_while_parent_lives(parent: &Receiver<()>) -> bool {
    matches!(parent.recv_timeout(RETRY), Err(RecvTimeoutError::Timeout))
}

fn editor() -> Option<String> {
    let enabled = std::env::var(SPLIT_VAR).ok();
    if !split_enabled_value(enabled.as_deref()) {
        return None;
    }
    Some(
        ["VISUAL", "EDITOR"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok())
            .find_map(|value| runnable_editor(&value))
            .unwrap_or_else(|| "nvim".to_owned()),
    )
}

fn runnable_editor(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && !value.chars().any(char::is_control)).then(|| value.to_owned())
}

fn split_enabled_value(value: Option<&str>) -> bool {
    let Some(value) = value else { return false };
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no"
    )
}

fn herdr<const N: usize>(args: [&str; N]) -> Option<Value> {
    let binary = devlaunch_core::clients::herdr_binary_from_process()?;
    let output = Command::new(binary)
        .args(args)
        // `aid` exports this for the later transport. An automation subprocess
        // must not briefly identify itself as that agent while asking Herdr.
        .env_remove(AGENT_VAR)
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
    fn the_split_switch_is_off_until_enabled() {
        for value in [None, Some(""), Some("0"), Some(" false "), Some("NO")] {
            assert!(!split_enabled_value(value), "{value:?}");
        }
        for value in [Some("1"), Some("true"), Some("yes"), Some("nvim")] {
            assert!(split_enabled_value(value), "{value:?}");
        }
    }

    #[test]
    fn the_waiter_does_not_advertise_itself_as_the_agent() {
        let baseline = AgentObservation {
            kind: "claude".to_owned(),
            state_change_seq: 41,
            activity: Activity::Done,
        };
        let command = waiter_command(
            "/bin/true",
            AgentKind::from_program("codex").unwrap(),
            Some(&baseline),
        )
        .unwrap();
        let environment = command
            .get_envs()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(environment[std::ffi::OsStr::new(AGENT_VAR)], None);
        assert_eq!(
            environment[std::ffi::OsStr::new(EXPECTED_AGENT_VAR)],
            Some(std::ffi::OsStr::new("codex"))
        );
        assert_eq!(
            serde_json::from_str::<AgentObservation>(
                environment[std::ffi::OsStr::new(BASELINE_VAR)]
                    .unwrap()
                    .to_str()
                    .unwrap()
            )
            .unwrap(),
            baseline
        );
    }
}
