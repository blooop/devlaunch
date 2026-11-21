//! The event log: one JSON object per line, appended to
//! `${XDG_STATE_HOME:-~/.local/state}/devlaunch/events.jsonl`, so a usage review
//! can measure how long a start takes without anybody having set
//! `DEVLAUNCH_TIMING` in advance.
//!
//! Always on. `DEVLAUNCH_EVENTS=0` turns it off. What is written:
//!
//! - `launch`: the workspace is ready, written the moment the session is about
//!   to start (or the `up` has finished). `seconds` is the wall time from the
//!   top of the command, `cold` says the `up` created the workspace, and
//!   `stages` holds the named [`timing`] stages in seconds.
//! - `session_end`: the session a `launch` started has ended. `seconds` is how
//!   long it lasted and `exit` is how it ended.
//! - `stop`, `kill`, `remove`, `prune`: the verb ran; `seconds` is how long it took.
//! - `aid_start` and `aid_end`: the same two moments for `dla`, which runs dl in
//!   process. `aid` is dla's name from before it was dla, and the name the
//!   readers of this file already filter on.
//!
//! **Writing it never fails a command.** Every I/O error is dropped where it
//! happens: a log that cannot be written costs the log and nothing else. Each
//! line goes in one `write` on a file opened for append, so two `dl` processes
//! writing at once do not interleave inside a line.
//!
//! The file is the binary's, not core's. Core renders no output (#251) and hands
//! the stages over as a [`timing::Ready`] through the hook [`timing::begin`]
//! takes; this module is that hook.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devlaunch_core::domain::spec::{self, WorkspaceSpec};
use devlaunch_core::timing;
use serde_json::{Map, Value, json};

/// The switch. Unset, or anything but an off word, keeps the log on.
pub const ENV_VAR: &str = "DEVLAUNCH_EVENTS";

/// Whether `raw`, the value of [`ENV_VAR`], leaves the log on.
pub(crate) fn wanted(raw: Option<&str>) -> bool {
    let value = raw.unwrap_or("").trim().to_ascii_lowercase();
    !matches!(value.as_str(), "0" | "false" | "no" | "off")
}

/// The hook for [`timing::begin`]: `Some` unless the log is switched off.
pub(crate) fn hook() -> Option<fn(&timing::Ready)> {
    wanted(devlaunch_core::osext::env_str(ENV_VAR).as_deref())
        .then_some(on_ready as fn(&timing::Ready))
}

/// The log file: `$XDG_STATE_HOME/devlaunch/events.jsonl`, or the
/// `~/.local/state` the XDG spec falls back to. `None` with neither.
pub(crate) fn path() -> Option<PathBuf> {
    let state = devlaunch_core::osext::env_str("XDG_STATE_HOME")
        .map(PathBuf::from)
        // The spec says a relative value is invalid and is to be ignored.
        .filter(|path| path.is_absolute())
        .or_else(|| devlaunch_core::osext::home_dir().map(|home| home.join(".local/state")))?;
    Some(state.join("devlaunch").join("events.jsonl"))
}

// --- what one command has seen -----------------------------------------------

/// The workspace verb this command runs, as the dispatch saw it.
#[derive(Debug, Clone)]
struct Verb {
    word: &'static str,
    target: Option<String>,
    started: Instant,
}

/// When the launch was ready, and which workspace it was.
#[derive(Debug, Clone)]
struct Readied {
    at: Instant,
    workspace_id: Option<String>,
    target: Option<String>,
}

/// What `dla` knows that dl does not: when it started, what its own steps cost,
/// and which agent it starts.
#[derive(Debug, Clone)]
struct Dla {
    started: Instant,
    stages: Vec<(&'static str, f64)>,
    agent: Option<String>,
    resume: bool,
    workspace_id: Option<String>,
}

#[derive(Debug, Default)]
struct Seen {
    verb: Option<Verb>,
    readied: Option<Readied>,
    dla: Option<Dla>,
}

static SEEN: Mutex<Seen> = Mutex::new(Seen {
    verb: None,
    readied: None,
    dla: None,
});

fn seen() -> MutexGuard<'static, Seen> {
    SEEN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A new command: forget the last one's verb and readiness. dla's half stays,
/// because dla sets it before the command it hands to dl begins.
pub(crate) fn begin() {
    let mut seen = seen();
    seen.verb = None;
    seen.readied = None;
}

/// The workspace verb this command runs, and the target it names (`prune`
/// names none).
pub(crate) fn workspace(target: Option<&str>, word: &'static str) {
    seen().verb = Some(Verb {
        word,
        target: target.map(str::to_owned),
        started: Instant::now(),
    });
}

/// The command ended with `exit`. Write what its verb ended.
pub(crate) fn end(exit: i32) {
    if !wanted(devlaunch_core::osext::env_str(ENV_VAR).as_deref()) {
        return;
    }
    let (verb, readied) = {
        let seen = seen();
        (seen.verb.clone(), seen.readied.clone())
    };
    let lifecycle = verb.as_ref().and_then(|verb| match verb.word {
        "stop" => Some("stop"),
        "kill" => Some("kill"),
        "rm" | "rme" => Some("remove"),
        "prune" => Some("prune"),
        _ => None,
    });
    if let (Some(ev), Some(verb)) = (lifecycle, &verb) {
        let mut event = base(ev, SystemTime::now(), verb.target.as_deref(), None);
        event.insert("seconds".into(), seconds(verb.started.elapsed()));
        event.insert("exit".into(), exit.into());
        append(&Value::Object(event));
        return;
    }
    // An `up` is ready when it ends; there is no session after it.
    let upped = verb.as_ref().is_some_and(|verb| verb.word == "up");
    if let Some(readied) = readied.filter(|_| !upped) {
        let mut event = base(
            "session_end",
            SystemTime::now(),
            readied.target.as_deref(),
            readied.workspace_id.as_deref(),
        );
        event.insert("seconds".into(), seconds(readied.at.elapsed()));
        event.insert("exit".into(), exit.into());
        append(&Value::Object(event));
    }
}

/// [`timing::begin`]'s hook: the launch is ready.
fn on_ready(ready: &timing::Ready) {
    let now = Instant::now();
    let (target, dla) = {
        let mut seen = seen();
        let target = seen.verb.as_ref().and_then(|verb| verb.target.clone());
        seen.readied = Some(Readied {
            at: now,
            workspace_id: ready.workspace_id.clone(),
            target: target.clone(),
        });
        if let Some(dla) = seen.dla.as_mut() {
            dla.workspace_id = ready.workspace_id.clone();
        }
        (target, seen.dla.clone())
    };
    let verb = {
        let seen = seen();
        seen.verb.as_ref().map(|verb| verb.word)
    };
    for event in ready_events(
        ready,
        target.as_deref(),
        verb,
        dla.as_ref(),
        now,
        SystemTime::now(),
    ) {
        append(&event);
    }
}

/// The lines one ready writes: `launch`, then `aid_start` when dla handed a
/// start over. Both carry `at` as their `ts`.
fn ready_events(
    ready: &timing::Ready,
    target: Option<&str>,
    verb: Option<&str>,
    dla: Option<&Dla>,
    now: Instant,
    at: SystemTime,
) -> Vec<Value> {
    let mut events = vec![launch_event(ready, target, verb, at)];
    if let Some(dla) = dla {
        events.push(aid_start_event(dla, ready, target, now, at));
    }
    events
}

/// The `launch` line for `ready`.
fn launch_event(
    ready: &timing::Ready,
    target: Option<&str>,
    verb: Option<&str>,
    at: SystemTime,
) -> Value {
    let mut event = base("launch", at, target, ready.workspace_id.as_deref());
    event.insert("cold".into(), ready.cold.into());
    event.insert("seconds".into(), ready.seconds.into());
    event.insert("stages".into(), stages(&ready.stages));
    if let Some(verb) = verb {
        event.insert("verb".into(), verb.into());
    }
    Value::Object(event)
}

/// The `aid_start` line: dla's own steps, then dl's launch as one more stage.
fn aid_start_event(
    dla: &Dla,
    ready: &timing::Ready,
    target: Option<&str>,
    now: Instant,
    at: SystemTime,
) -> Value {
    let mut event = base("aid_start", at, target, ready.workspace_id.as_deref());
    let mut steps = dla.stages.clone();
    steps.push(("dl", ready.seconds));
    event.insert("cold".into(), ready.cold.into());
    event.insert(
        "seconds".into(),
        seconds(now.saturating_duration_since(dla.started)),
    );
    event.insert("stages".into(), stages(&steps));
    event.insert("agent".into(), dla.agent.clone().into());
    event.insert("resume".into(), dla.resume.into());
    Value::Object(event)
}

// --- dla's half ---------------------------------------------------------------

/// dla's view of one start, which it fills in before it hands the line to dl.
///
/// `dla` calls [`DlaStart::begin`] at its top, [`DlaStart::stage`] after each of
/// its own steps, and [`DlaStart::hand_over`] just before `dl::run`. The `aid_start`
/// line is written when dl's launch is ready, and [`dla_end`] writes `aid_end`.
#[derive(Debug)]
pub struct DlaStart {
    started: Instant,
    last: Instant,
    stages: Vec<(&'static str, f64)>,
}

impl DlaStart {
    /// The clock starts now.
    pub fn begin() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last: now,
            stages: Vec::new(),
        }
    }

    /// Charge the time since the last stage (or the start) to `name`.
    pub fn stage(&mut self, name: &'static str) {
        let now = Instant::now();
        let took = now.saturating_duration_since(self.last);
        self.last = now;
        self.stages.push((name, round(took.as_secs_f64())));
    }

    /// Hand the start to dl, with the agent it starts and whether it resumes one.
    pub fn hand_over(self, agent: Option<&str>, resume: bool) {
        seen().dla = Some(Dla {
            started: self.started,
            stages: self.stages,
            agent: agent.map(str::to_owned),
            resume,
            workspace_id: None,
        });
    }
}

/// dla ended with `exit`: write `aid_end`, if dla handed a start over.
pub fn dla_end(exit: i32) {
    let (dla, target) = {
        let mut seen = seen();
        let target = seen.verb.as_ref().and_then(|verb| verb.target.clone());
        (seen.dla.take(), target)
    };
    let Some(dla) = dla else {
        return;
    };
    if !wanted(devlaunch_core::osext::env_str(ENV_VAR).as_deref()) {
        return;
    }
    let mut event = base(
        "aid_end",
        SystemTime::now(),
        target.as_deref(),
        dla.workspace_id.as_deref(),
    );
    event.insert("seconds".into(), seconds(dla.started.elapsed()));
    event.insert("exit".into(), exit.into());
    event.insert("agent".into(), dla.agent.into());
    append(&Value::Object(event));
}

// --- one line ---------------------------------------------------------------

/// The fields every line has: when, what, and which workspace.
///
/// `ws` is the workspace id when the launch knew it, else the target as typed.
/// `repo` and `branch` come from the target when it is `owner/repo[@branch]`.
fn base(
    ev: &str,
    at: SystemTime,
    target: Option<&str>,
    workspace_id: Option<&str>,
) -> Map<String, Value> {
    let (repo, branch) = match target.map(spec::parse) {
        Some(WorkspaceSpec::OwnerRepo {
            owner,
            repo,
            branch,
        }) => (Some(format!("{owner}/{repo}")), branch.map(str::to_owned)),
        _ => (None, None),
    };
    let mut event = Map::new();
    event.insert("ts".into(), iso8601(at).into());
    event.insert("ev".into(), ev.into());
    event.insert("ws".into(), workspace_id.or(target).into());
    event.insert("repo".into(), repo.into());
    event.insert("branch".into(), branch.into());
    event
}

fn stages(stages: &[(&str, f64)]) -> Value {
    Value::Object(
        stages
            .iter()
            .map(|(name, seconds)| ((*name).to_owned(), json!(seconds)))
            .collect(),
    )
}

fn seconds(took: Duration) -> Value {
    round(took.as_secs_f64()).into()
}

/// Seconds to the microsecond, as the timing document reports them.
fn round(seconds: f64) -> f64 {
    (seconds * 1e6).round() / 1e6
}

/// Append `event` as one line. Every error is dropped: see the module docs.
fn append(event: &Value) {
    let Some(path) = path() else { return };
    let Ok(mut line) = serde_json::to_string(event) else {
        return;
    };
    line.push('\n');
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let _ = file.write_all(line.as_bytes());
}

/// `at` as ISO-8601 in UTC, to the millisecond: `2026-11-21T09:30:00.123Z`.
pub(crate) fn iso8601(at: SystemTime) -> String {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    let of_day = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60,
        since.subsec_millis()
    )
}

/// The proleptic Gregorian date `days` after 1970-01-01, by Howard Hinnant's
/// `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_is_on_unless_switched_off() {
        assert!(wanted(None));
        assert!(wanted(Some("")));
        assert!(wanted(Some("1")));
        for off in ["0", "false", "no", "off", " OFF "] {
            assert!(!wanted(Some(off)), "{off:?}");
        }
    }

    #[test]
    fn a_timestamp_is_iso_8601_utc() {
        assert_eq!(iso8601(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        // 2026-11-21T09:30:05.250Z, a date past a leap day and a month end.
        let at = UNIX_EPOCH + Duration::from_millis(1_795_253_405_250);
        assert_eq!(iso8601(at), "2026-11-21T09:30:05.250Z");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(iso8601(leap), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn a_launch_line_carries_the_triple_the_stages_and_whether_it_was_cold() {
        let ready = timing::Ready {
            workspace_id: Some("repo-main".to_owned()),
            seconds: 4.5,
            cold: true,
            stages: vec![("host-prep", 1.25), ("devpod-up", 3.0)],
        };

        let line = launch_event(
            &ready,
            Some("owner/repo@main"),
            Some("attach"),
            SystemTime::now(),
        );

        assert_eq!(line["ev"], "launch");
        assert_eq!(line["ws"], "repo-main");
        assert_eq!(line["repo"], "owner/repo");
        assert_eq!(line["branch"], "main");
        assert_eq!(line["cold"], true);
        assert_eq!(line["seconds"], 4.5);
        assert_eq!(line["stages"], json!({"host-prep": 1.25, "devpod-up": 3.0}));
        assert_eq!(line["verb"], "attach");
    }

    #[test]
    fn the_launch_and_aid_start_lines_of_one_ready_share_a_timestamp() {
        let ready = timing::Ready {
            workspace_id: Some("repo-main".to_owned()),
            seconds: 4.5,
            cold: false,
            stages: vec![],
        };
        let started = Instant::now();
        let dla = Dla {
            started,
            stages: vec![],
            agent: Some("claude".to_owned()),
            resume: false,
            workspace_id: None,
        };
        let at = UNIX_EPOCH + Duration::from_millis(1_795_253_405_999);

        let lines = ready_events(&ready, Some("repo-main"), None, Some(&dla), started, at);

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["ev"], "launch");
        assert_eq!(lines[1]["ev"], "aid_start");
        for line in &lines {
            assert_eq!(line["ts"], "2026-11-21T09:30:05.999Z");
        }
    }

    #[test]
    fn a_bare_workspace_name_is_the_ws_and_names_no_repo() {
        let line = base("stop", SystemTime::now(), Some("repo-main"), None);

        assert_eq!(line["ws"], "repo-main");
        assert_eq!(line["repo"], Value::Null);
        assert_eq!(line["branch"], Value::Null);
    }
}
