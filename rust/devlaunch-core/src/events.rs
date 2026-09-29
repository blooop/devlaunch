//! A persistent log of workspace lifecycle events, one JSON object per line.
//!
//! Every `dl` process that launches, ends a session in, stops, kills, removes or
//! prunes a workspace appends a line, and so does every `aid` that starts an agent
//! (`aid_start`) and outlives it (`aid_end`), to `$XDG_STATE_HOME/devlaunch/events.jsonl`
//! (`~/.local/state/devlaunch/events.jsonl` by default). A weekly usage review
//! reads it, and joins it with herdr's and Claude Code's own logs on the
//! workspace id and the herdr pane, which is why both ride on the launch line.
//!
//! Modelled on [`crate::timing`]'s switch, with the default the other way round:
//! the log is on unless `DEVLAUNCH_EVENTS=0` (or `false`, `no`), and `DEVLAUNCH_EVENTS_PATH` puts the
//! file somewhere else (the tests use it). The same shape too: [`begin`] reads the
//! environment once at the top of the command and installs a process-global
//! sink, so the flows that know the facts can record them without a sink in
//! their signatures.
//!
//! **It must never fail or slow a command.** Every IO error is swallowed, the
//! line is built in memory and handed to one `write` on a file opened with
//! `O_APPEND`, so two `dl` processes appending at once interleave whole lines,
//! and a command whose log cannot be written behaves exactly as it would have.
//!
//! The field names are a contract with the script that reads them:
//!
//! ```text
//! {"ts":"2026-09-29T10:15:02.123Z","ev":"launch","ws":"devlaunch-main-3j1t",
//!  "repo":"blooop/devlaunch","branch":"main","host":"box",
//!  "cold":false,"seconds":4.8,"stages":null,"herdr":false,"herdr_pane":null}
//! ```
//!
//! binary surface — not part of the frozen wf API (#251 §7)

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::domain::metadata::MetadataStorage;
use crate::domain::workspace_id::WorkspaceId;
use crate::flows::launch::{Plan, plan};

/// The switch. Unset (or empty) is on; `0`, `false` or `no` is off, the values
/// every other `DEVLAUNCH_*` switch reads as "not set".
pub(crate) const ENV_VAR: &str = "DEVLAUNCH_EVENTS";

/// Where the log goes instead of the XDG state directory.
pub(crate) const PATH_VAR: &str = "DEVLAUNCH_EVENTS_PATH";

/// The values that turn the log off, compared stripped and lowercased.
const OFF: [&str; 3] = ["0", "false", "no"];

/// What happened. Each arm's fields are the ones its line carries past the six
/// every line has.
///
/// binary surface — not part of the frozen wf API (#251 §7)
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A session is about to be handed over.
    Launch {
        /// This launch brought the container up (created or started it), as
        /// against attaching to one that was already running.
        cold: bool,
        /// From the top of the command to the session.
        seconds: f64,
        /// The stages [`crate::timing`] closed on the way, by name. Only a run
        /// with `DEVLAUNCH_TIMING` set measures them, so `None` otherwise.
        stages: Option<Vec<(&'static str, f64)>>,
        /// `DEVLAUNCH_HERDR` was on.
        herdr: bool,
        /// `HERDR_PANE_ID`, when this launch runs in a herdr pane.
        herdr_pane: Option<String>,
    },
    /// The session handed over at the launch has ended.
    SessionEnd {
        seconds: f64,
        exit: i32,
    },
    Stop,
    Kill,
    Remove,
    /// `dl --prune` removed clone directories. `bytes_freed` is `None` when some
    /// of them could not be measured, because a floor is not a total.
    Prune {
        removed: usize,
        bytes_freed: Option<u64>,
    },
    /// `aid` is about to hand its line to dl, which starts the agent. The `launch`
    /// line dl writes next measures the rest of the way to the agent.
    AidStart {
        /// The agent the line starts: `claude`, `codex`, ...
        agent: String,
        /// From the top of `aid` to the hand-off.
        seconds: f64,
        /// aid's own steps on the way, by name, in the order they ran. A step the
        /// line did not take is absent rather than zero.
        stages: Vec<(&'static str, f64)>,
        /// `aid resume`: the agent reopens an earlier session.
        resume: bool,
    },
    /// dl has handed back, so the agent's session is over.
    AidEnd {
        seconds: f64,
        exit: i32,
    },
}

impl Event {
    fn name(&self) -> &'static str {
        match self {
            Event::Launch { .. } => "launch",
            Event::SessionEnd { .. } => "session_end",
            Event::Stop => "stop",
            Event::Kill => "kill",
            Event::Remove => "remove",
            Event::Prune { .. } => "prune",
            Event::AidStart { .. } => "aid_start",
            Event::AidEnd { .. } => "aid_end",
        }
    }

    fn fields(&self, into: &mut Map<String, Value>) {
        match self {
            Event::Launch {
                cold,
                seconds,
                stages,
                herdr,
                herdr_pane,
            } => {
                into.insert("cold".into(), json!(cold));
                into.insert("seconds".into(), json!(round3(*seconds)));
                let stages = stages.as_deref().map(stages_object);
                into.insert("stages".into(), json!(stages));
                into.insert("herdr".into(), json!(herdr));
                into.insert("herdr_pane".into(), json!(herdr_pane));
            }
            Event::SessionEnd { seconds, exit } | Event::AidEnd { seconds, exit } => {
                into.insert("seconds".into(), json!(round3(*seconds)));
                into.insert("exit".into(), json!(exit));
            }
            Event::Stop | Event::Kill | Event::Remove => {}
            Event::Prune {
                removed,
                bytes_freed,
            } => {
                into.insert("removed".into(), json!(removed));
                into.insert("bytes_freed".into(), json!(bytes_freed));
            }
            Event::AidStart {
                agent,
                seconds,
                stages,
                resume,
            } => {
                into.insert("agent".into(), json!(agent));
                into.insert("seconds".into(), json!(round3(*seconds)));
                into.insert("stages".into(), Value::Object(stages_object(stages)));
                into.insert("resume".into(), json!(resume));
            }
        }
    }
}

/// Stage seconds as one object, `{"name": seconds, ...}`, in the order given.
fn stages_object(stages: &[(&'static str, f64)]) -> Map<String, Value> {
    stages
        .iter()
        .map(|(name, seconds)| ((*name).to_owned(), json!(round3(*seconds))))
        .collect()
}

fn round3(seconds: f64) -> f64 {
    (seconds * 1e3).round() / 1e3
}

/// Which workspace an event is about, as far as dl's records can say.
///
/// binary surface — not part of the frozen wf API (#251 §7)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subject {
    ws: Option<String>,
    repo: Option<String>,
    branch: Option<String>,
}

impl Subject {
    /// An event about no one workspace: a prune.
    pub fn none() -> Self {
        Self::default()
    }

    /// `workspace_id`, with the triple `metadata.json` under `cache_dir` holds for
    /// it. A workspace dl has no record of (a path source, a store that will not
    /// read) keeps its id and logs no repository or branch.
    ///
    /// The lookup takes no lock and writes nothing ([`MetadataStorage::look`]).
    pub fn recorded(cache_dir: &Path, workspace_id: &str) -> Self {
        let records = MetadataStorage::look(MetadataStorage::path_in(cache_dir));
        let record = records.worktree_for_workspace_id(workspace_id);
        Subject {
            ws: Some(workspace_id.to_owned()),
            repo: record.map(|record| format!("{}/{}", record.owner, record.repo)),
            branch: record.map(|record| record.branch.clone()),
        }
    }

    /// What `spec` says it names, before anything is resolved: for `aid`, which
    /// writes its line before dl has looked the workspace up.
    ///
    /// Read through [`plan`], the launch's own parse boundary, so a spec the
    /// launch will refuse names nothing here either. `owner/repo@branch` derives
    /// its id, a bare `owner/repo` has no branch yet and so no id, a path or git
    /// source carries the id dl will give it, and a bare name is looked up in the
    /// records as [`Subject::recorded`] does.
    pub fn of_spec(cache_dir: &Path, spec: &str) -> Self {
        match plan(spec) {
            Ok(Plan::Triple {
                owner,
                repo,
                branch,
                ..
            }) => Subject {
                ws: branch.as_ref().and_then(|branch| {
                    WorkspaceId::new(&owner, &repo, branch)
                        .ok()
                        .map(|workspace| workspace.value().to_owned())
                }),
                repo: Some(format!("{owner}/{repo}")),
                branch,
            },
            Ok(Plan::Creatable { workspace_id, .. }) => Subject {
                ws: Some(workspace_id),
                ..Subject::none()
            },
            Ok(Plan::Existing { name }) => Subject::recorded(cache_dir, &name),
            Err(_) => Subject::none(),
        }
    }
}

/// The one line an event is written as, without its newline.
fn line(ts: &str, host: &str, subject: &Subject, event: &Event) -> String {
    let mut object = Map::new();
    object.insert("ts".into(), json!(ts));
    object.insert("ev".into(), json!(event.name()));
    object.insert("ws".into(), json!(subject.ws));
    object.insert("repo".into(), json!(subject.repo));
    object.insert("branch".into(), json!(subject.branch));
    object.insert("host".into(), json!(host));
    event.fields(&mut object);
    Value::Object(object).to_string()
}

/// Now, as UTC ISO 8601 to the millisecond.
fn now_utc() -> String {
    jiff::Timestamp::now()
        .strftime("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

fn hostname() -> String {
    rustix::system::uname()
        .nodename()
        .to_string_lossy()
        .into_owned()
}

/// Where the log is appended to, once the switch has been read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sink {
    path: PathBuf,
}

impl Sink {
    /// The sink the environment asks for, or `None` when the log is off or there
    /// is nowhere to put it.
    fn from_env() -> Option<Self> {
        Self::resolve(
            crate::osext::env_str(ENV_VAR).as_deref(),
            crate::osext::env_str(PATH_VAR).as_deref(),
            crate::domain::xdg::state_home().ok(),
        )
    }

    /// [`Sink::from_env`] as a function of its three inputs, so a test can state
    /// them instead of mutating an environment every other test shares.
    fn resolve(
        switch: Option<&str>,
        path: Option<&str>,
        state_home: Option<PathBuf>,
    ) -> Option<Self> {
        if let Some(switch) = switch
            && OFF.contains(&crate::osext::strip(switch).to_lowercase().as_str())
        {
            return None;
        }
        let path = match path.filter(|path| !path.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => state_home?.join("devlaunch").join("events.jsonl"),
        };
        Some(Sink { path })
    }

    #[cfg(test)]
    pub(crate) fn at(path: impl Into<PathBuf>) -> Self {
        Sink { path: path.into() }
    }

    /// Append `event`, and give up quietly on any error.
    pub(crate) fn write(&self, subject: &Subject, event: &Event) {
        let mut bytes = line(&now_utc(), &hostname(), subject, event).into_bytes();
        bytes.push(b'\n');
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        else {
            return;
        };
        // One `write`, not `write_all`: a short write to an append-only log is a
        // torn line either way, and a retry loop is time a command would pay.
        let _ = file.write(&bytes);
    }
}

// --- the process-global handle ---------------------------------------------

struct Armed {
    sink: Sink,
    started: Instant,
}

static ARMED: Mutex<Option<Armed>> = Mutex::new(None);

fn armed() -> MutexGuard<'static, Option<Armed>> {
    ARMED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Read the switch and start the command's clock. Called once at the top of the
/// command, as [`crate::timing::begin`] is.
pub fn begin() {
    install(Sink::from_env());
}

/// Install `sink` as the process's, replacing whatever was there.
pub(crate) fn install(sink: Option<Sink>) {
    *armed() = sink.map(|sink| Armed {
        sink,
        started: Instant::now(),
    });
}

/// Whether the log is on, for a caller that has to gather an event's subject
/// before the thing it describes destroys it (a removal takes its record).
pub fn on() -> bool {
    armed().is_some()
}

/// How long ago [`begin`] ran, if the log is on.
pub(crate) fn since_begin() -> Option<Duration> {
    armed().as_ref().map(|armed| armed.started.elapsed())
}

/// Append `event` about the workspace `subject` names, if the log is on.
///
/// `subject` is a closure so that a log that is off pays for no records lookup.
pub fn record(subject: impl FnOnce() -> Subject, event: Event) {
    let sink = match armed().as_ref() {
        Some(armed) => armed.sink.clone(),
        None => return,
    };
    sink.write(&subject(), &event);
}

/// The launch line, with the facts only this process's environment and clock
/// know filled in.
pub(crate) fn launch(cold: bool) -> Option<Event> {
    let seconds = since_begin()?.as_secs_f64();
    // The shared switch parse, whose name reads backwards here: "disabled" is
    // what it answers for the provisioning switches, and for a consent the same
    // answer means "on" -- which is how `clients::herdr` reads this variable too.
    let herdr = crate::flows::provision::provisioning_disabled(
        crate::osext::env_str(crate::clients::herdr::ENABLE_VAR).as_deref(),
    );
    Some(Event::Launch {
        cold,
        seconds,
        stages: crate::timing::stage_seconds(),
        herdr,
        herdr_pane: crate::osext::env_str(crate::clients::herdr::PANE_VAR)
            .filter(|pane| !pane.is_empty()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject() -> Subject {
        Subject {
            ws: Some("devlaunch-main-3j1t".into()),
            repo: Some("blooop/devlaunch".into()),
            branch: Some("main".into()),
        }
    }

    fn parsed(line: &str) -> Value {
        serde_json::from_str(line).expect("a JSON line")
    }

    #[test]
    fn a_line_carries_the_six_common_fields_first_and_in_order() {
        let written = line("2026-09-29T10:15:02.123Z", "box", &subject(), &Event::Stop);
        assert_eq!(
            written,
            r#"{"ts":"2026-09-29T10:15:02.123Z","ev":"stop","ws":"devlaunch-main-3j1t","repo":"blooop/devlaunch","branch":"main","host":"box"}"#
        );
    }

    #[test]
    fn a_launch_line_carries_its_own_fields() {
        let event = Event::Launch {
            cold: true,
            seconds: 12.34567,
            stages: Some(vec![("host-prep", 1.5), ("devpod-up", 9.0)]),
            herdr: true,
            herdr_pane: Some("w1:p2".into()),
        };
        let value = parsed(&line("t", "box", &subject(), &event));
        assert_eq!(value["ev"], "launch");
        assert_eq!(value["cold"], true);
        assert_eq!(value["seconds"], 12.346);
        assert_eq!(value["stages"]["host-prep"], 1.5);
        assert_eq!(value["stages"]["devpod-up"], 9.0);
        assert_eq!(value["herdr"], true);
        assert_eq!(value["herdr_pane"], "w1:p2");
    }

    #[test]
    fn a_prune_is_about_no_workspace_and_says_what_it_freed() {
        let event = Event::Prune {
            removed: 2,
            bytes_freed: None,
        };
        let value = parsed(&line("t", "box", &Subject::none(), &event));
        assert_eq!(value["ev"], "prune");
        assert_eq!(value["ws"], Value::Null);
        assert_eq!(value["repo"], Value::Null);
        assert_eq!(value["removed"], 2);
        assert_eq!(value["bytes_freed"], Value::Null);
    }

    #[test]
    fn a_session_end_says_how_long_and_how_it_ended() {
        let event = Event::SessionEnd {
            seconds: 61.0,
            exit: 130,
        };
        let value = parsed(&line("t", "box", &subject(), &event));
        assert_eq!(value["ev"], "session_end");
        assert_eq!(value["seconds"], 61.0);
        assert_eq!(value["exit"], 130);
    }

    #[test]
    fn an_aid_start_names_the_agent_and_its_own_steps_in_order() {
        let event = Event::AidStart {
            agent: "codex".into(),
            seconds: 7.25,
            stages: vec![("prompt", 5.0), ("boot_wait", 2.0)],
            resume: false,
        };
        let written = line("t", "box", &subject(), &event);
        assert!(
            written.ends_with(
                r#""agent":"codex","seconds":7.25,"stages":{"prompt":5.0,"boot_wait":2.0},"resume":false}"#
            ),
            "{written}"
        );
        let ended = parsed(&line(
            "t",
            "box",
            &subject(),
            &Event::AidEnd {
                seconds: 90.0,
                exit: 1,
            },
        ));
        assert_eq!(ended["ev"], "aid_end");
        assert_eq!(ended["exit"], 1);
    }

    #[test]
    fn a_spec_names_what_it_states_and_no_more() {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let triple = Subject::of_spec(dir.path(), "blooop/devlaunch@main");
        assert_eq!(triple.ws.as_deref(), Some("devlaunch-main-3j1t"));
        assert_eq!(triple.repo.as_deref(), Some("blooop/devlaunch"));
        assert_eq!(triple.branch.as_deref(), Some("main"));
        let no_branch = Subject::of_spec(dir.path(), "blooop/devlaunch");
        assert_eq!(no_branch.ws, None);
        assert_eq!(no_branch.repo.as_deref(), Some("blooop/devlaunch"));
        assert_eq!(no_branch.branch, None);
        let name = Subject::of_spec(dir.path(), "some-workspace");
        assert_eq!(name.ws.as_deref(), Some("some-workspace"));
        assert_eq!(name.repo, None);
    }

    #[test]
    fn the_sink_appends_one_parseable_line_per_event() {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let path = dir.path().join("nested/events.jsonl");
        let sink = Sink::at(&path);
        sink.write(&subject(), &Event::Stop);
        sink.write(&subject(), &Event::Remove);
        let written = std::fs::read_to_string(&path).expect("the log was written");
        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(parsed(lines[0])["ev"], "stop");
        assert_eq!(parsed(lines[1])["ev"], "remove");
        let ts = parsed(lines[0])["ts"]
            .as_str()
            .expect("a string")
            .to_owned();
        assert_eq!(ts.len(), "2026-09-29T10:15:02.123Z".len(), "{ts}");
        assert!(ts.ends_with('Z'), "{ts}");
        assert!(
            !parsed(lines[0])["host"]
                .as_str()
                .expect("a host")
                .is_empty()
        );
    }

    #[test]
    fn a_path_that_cannot_be_written_is_swallowed() {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let file = dir.path().join("a-file");
        std::fs::write(&file, "").expect("a file");
        // A directory under a regular file cannot be made, and the log is not
        // worth a panic or an error.
        Sink::at(file.join("events.jsonl")).write(&subject(), &Event::Stop);
        Sink::at(dir.path()).write(&subject(), &Event::Stop);
    }

    #[test]
    fn the_log_is_on_by_default_under_the_state_home() {
        assert_eq!(
            Sink::resolve(None, None, Some(PathBuf::from("/state"))),
            Some(Sink::at("/state/devlaunch/events.jsonl"))
        );
        assert_eq!(
            Sink::resolve(Some("1"), None, Some(PathBuf::from("/state"))),
            Some(Sink::at("/state/devlaunch/events.jsonl"))
        );
    }

    #[test]
    fn zero_turns_the_log_off() {
        for off in ["0", " 0 ", "false", "No"] {
            assert_eq!(
                Sink::resolve(Some(off), Some("/x"), Some(PathBuf::from("/state"))),
                None,
                "{off:?}"
            );
        }
    }

    #[test]
    fn the_path_variable_wins_and_an_empty_one_is_unset() {
        assert_eq!(
            Sink::resolve(None, Some("/tmp/e.jsonl"), None),
            Some(Sink::at("/tmp/e.jsonl"))
        );
        assert_eq!(
            Sink::resolve(None, Some(""), Some(PathBuf::from("/s"))),
            Some(Sink::at("/s/devlaunch/events.jsonl"))
        );
        assert_eq!(Sink::resolve(None, None, None), None, "no home, no log");
    }

    #[test]
    fn a_workspace_with_no_record_keeps_its_id_and_names_no_repository() {
        let dir = tempfile::tempdir().expect("a scratch dir");
        assert_eq!(
            Subject::recorded(dir.path(), "some-path-ws"),
            Subject {
                ws: Some("some-path-ws".into()),
                repo: None,
                branch: None,
            }
        );
    }
}
