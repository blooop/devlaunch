//! Keeping the agent sessions a workspace holds across a recreate (devlaunch#673).
//!
//! `dl <ws> recreate` replaces the container, and with it every process in it:
//! the agent in each herdr pane attached to the workspace ends. Its conversation
//! does not, because the transcript lives in `~/.claude`, which outlives the
//! container. So a recreate can bring the agent back, if it knows how each one
//! was started and where.
//!
//! herdr already knows both. It keeps, per pane, the argv it types to start that
//! pane's agent again after herdr itself restarts (`agent_resume.argv` in
//! `session.json`): `dl <ws> -- ... claude ... --resume <id>`. dl reports that
//! argv when it launches an agent, and the container hook reports it again when
//! the session id changes (`/clear`, an in-agent `/resume`). That is the line to
//! type after a recreate too, into the same pane, once the old transport has
//! exited and the pane is back at its shell.
//!
//! Two steps, so a caller can put the recreate between them:
//!
//! 1. [`collect`] asks herdr which panes hold a live session into this workspace
//!    and reads the line each one would be started again with.
//! 2. [`restart`] types each line into its pane with `herdr pane run`.
//!
//! **Every failure is a notice, never a refusal of the recreate.** A herdr that
//! does not answer, a pane with no saved line, a pane that will not take the
//! line: each costs that agent its automatic restart, and the caller says so with
//! the line to type by hand. A recreate asked for is a recreate done.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::clients::herdr;
use crate::flows::session_manager;
use crate::runner::Runner;

/// What one question to herdr may take.
///
/// Per question rather than one budget for the scan, because the scan is not on
/// a pane's path the way [`session_manager::pane_destination`] is: a recreate
/// takes minutes, and a pane skipped because an earlier one was slow would be an
/// agent ended with nobody told.
const ASK_WITHIN: Duration = Duration::from_secs(2);

/// The herdr this process can ask, and where it keeps its saved session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manager {
    binary: String,
    /// The pane this process runs in. Nothing types into it: its foreground is
    /// this `dl`.
    own_pane: Option<String>,
    session_file: PathBuf,
}

impl Manager {
    /// The herdr whose pane `host` runs in, or `None` outside one.
    pub fn from_host(host: &crate::flows::launch::Host) -> Option<Self> {
        Self::resolve(&host.herdr, host.herdr_bin.as_deref())
    }

    /// The herdr whose pane `env` describes, or `None` outside one.
    ///
    /// The socket is required because its directory is where herdr saves the
    /// session, which is the one record of how each pane's agent starts again.
    /// `binary` falls back to a `PATH` lookup, the rule every other herdr call in
    /// dl follows.
    pub(crate) fn resolve(env: &herdr::HostEnv, binary: Option<&str>) -> Option<Self> {
        if !crate::flows::provision::provisioning_disabled(env.in_pane.as_deref()) {
            return None;
        }
        let socket = herdr::non_empty(env.socket.as_deref())?;
        let session_file = Path::new(&socket).parent()?.join("session.json");
        Some(Self {
            binary: herdr::non_empty(binary)
                .unwrap_or_else(|| crate::flows::launch::HERDR_BIN_FALLBACK.to_owned()),
            own_pane: herdr::non_empty(env.pane_id.as_deref()),
            session_file,
        })
    }

    fn ask(&self, runner: &dyn Runner, args: &[String]) -> Option<String> {
        session_manager::ask(
            runner,
            &self.binary,
            args,
            &mut session_manager::Budget::of(ASK_WITHIN),
        )
    }
}

/// One pane's agent, and the line that starts it again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldSession {
    pub pane_id: String,
    /// herdr's saved resume argv for the pane, as herdr would type it.
    pub line: Vec<String>,
}

/// What [`collect`] found in herdr's panes for one workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeldSessions {
    /// The sessions that can be started again.
    pub sessions: Vec<HeldSession>,
    /// Panes that hold a live agent in this workspace and no saved line for it.
    pub unresumable: Vec<String>,
}

/// The agent sessions herdr's panes hold in `workspace_id`, or `None` when herdr
/// did not answer the question of which panes there are.
///
/// A pane counts when its foreground is a dl transport into this workspace (the
/// reading the pane shell does, [`session_manager::workspace_among`]) and herdr
/// does not say its agent has ended. Its line is herdr's saved resume argv, taken
/// only when it names this workspace and the words after its `--` start an agent
/// dl knows by name.
///
/// `herdr agent get` decides the two doubtful cases. A saved line in a pane whose
/// agent herdr calls `done`, or where herdr names no agent, is a line left over
/// from an agent the user already quit; typing it would start one nobody asked
/// for. A pane with no saved line and a live agent is an agent this recreate ends
/// with no way back, which the caller says. A herdr that does not answer
/// `agent get` is no reason to drop a line it saved.
pub fn collect(
    runner: &dyn Runner,
    manager: &Manager,
    workspace_id: &str,
    read: &dyn Fn(&Path) -> Option<String>,
) -> Option<HeldSessions> {
    let panes = manager
        .ask(runner, &herdr::pane_list_argv())
        .as_deref()
        .and_then(herdr::panes_in)?;
    let session = read(&manager.session_file);
    let mut held = HeldSessions::default();
    for pane in panes {
        let Some(info) = manager
            .ask(runner, &herdr::process_info_argv(&pane.pane_id))
            .as_deref()
            .and_then(herdr::process_info_in)
        else {
            continue;
        };
        if session_manager::workspace_among(&info).as_deref() != Some(workspace_id) {
            continue;
        }
        let line = session
            .as_deref()
            .and_then(|json| herdr::saved_argv(json, &pane.pane_id))
            .flatten()
            .filter(|argv| starts_an_agent(argv, workspace_id));
        let live = match agent_reading(runner, manager, &pane.pane_id) {
            herdr::AgentReading::Unanswered => None,
            herdr::AgentReading::NoAgent => Some(false),
            herdr::AgentReading::Agent(agent) => Some(agent.status != herdr::AgentStatus::Done),
        };
        match (line, live) {
            (Some(line), Some(true) | None) => held.sessions.push(HeldSession {
                pane_id: pane.pane_id,
                line,
            }),
            (None, Some(true)) => held.unresumable.push(pane.pane_id),
            (Some(_) | None, _) => {}
        }
    }
    Some(held)
}

/// What herdr says one pane's agent is doing.
pub fn agent_reading(runner: &dyn Runner, manager: &Manager, pane_id: &str) -> herdr::AgentReading {
    manager
        .ask(runner, &herdr::agent_get_argv(pane_id))
        .as_deref()
        .map_or(herdr::AgentReading::Unanswered, herdr::agent_reading_in)
}

/// Whether a saved line starts an agent in `workspace_id`:
/// `dl <workspace_id> [...] -- [NAME=value ...] <agent> ...`.
///
/// The workspace word is compared with the id because that is what dl writes
/// there, both from the launch and through the container hook. A line naming
/// another workspace was saved by an earlier session in the same pane, and would
/// start its agent there.
fn starts_an_agent(argv: &[String], workspace_id: &str) -> bool {
    argv.get(1).map(String::as_str) == Some(workspace_id)
        && argv
            .iter()
            .skip_while(|word| *word != "--")
            .skip(1)
            .find(|word| !herdr::is_assignment(word))
            .is_some_and(|program| herdr::agent_named(program).is_some())
}

/// What became of one held session after the recreate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Restarted {
    /// herdr typed the line into the pane.
    Typed { pane_id: String },
    /// The pane is the one this `dl` runs in, so nothing was typed into it.
    /// `line` is the command to run there, as one shell line.
    OwnPane { pane_id: String, line: String },
    /// herdr would not take the line in the time allowed.
    NotTyped { pane_id: String, line: String },
}

/// How many times one pane is asked to take its line, and how far apart.
///
/// herdr refuses `pane run` while the pane's foreground is not its shell, and
/// the old transport exits on its own schedule once the container it spoke to is
/// gone. Ten seconds per pane is well past that, and short enough that a pane
/// herdr closed, or one that never returns to a shell, costs little.
pub(crate) const RUN_TRIES: u32 = 20;
pub(crate) const RUN_TICK: Duration = Duration::from_millis(500);

/// Type each held session's line into the pane it came from.
///
/// Call it once the recreate's `up` has finished: the line runs `dl` again, and
/// that `dl` attaches to whatever container is up when it starts. `wait` sleeps
/// between tries.
///
/// The pane this `dl` runs in is never typed into. Its foreground is this
/// process, so `pane run` would refuse it for as long as this `dl` runs; its line
/// is handed back for the caller to say.
pub fn restart(
    runner: &dyn Runner,
    manager: &Manager,
    held: &HeldSessions,
    wait: &dyn Fn(Duration),
) -> Vec<Restarted> {
    held.sessions
        .iter()
        .map(|session| {
            let pane_id = session.pane_id.clone();
            let line = crate::shell::join(session.line.iter().map(String::as_str));
            if manager.own_pane.as_deref() == Some(pane_id.as_str()) {
                return Restarted::OwnPane { pane_id, line };
            }
            let run = herdr::pane_run_argv(&pane_id, &line);
            for tick in 0..RUN_TRIES {
                if tick > 0 {
                    wait(RUN_TICK);
                }
                if manager.ask(runner, &run).is_some() {
                    return Restarted::Typed { pane_id };
                }
            }
            Restarted::NotTyped { pane_id, line }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ScriptedRunner;
    use devlaunch_test_support::Response;

    const HERDR: &str = "/opt/herdr/bin/herdr";

    fn env(pane: &str) -> herdr::HostEnv {
        herdr::HostEnv {
            enabled: None,
            in_pane: Some("1".to_owned()),
            pane_id: Some(pane.to_owned()),
            socket: Some("/run/herdr/herdr.sock".to_owned()),
            binary: Some(HERDR.to_owned()),
        }
    }

    fn manager() -> Manager {
        Manager::resolve(&env("w1:p9"), Some(HERDR)).expect("a herdr pane")
    }

    fn pane_list(panes: &[&str]) -> String {
        let rows: Vec<String> = panes
            .iter()
            .map(|pane| {
                format!(
                    r#"{{"pane_id":"{pane}","terminal_id":"t","workspace_id":"w1","tab_id":"w1:t1","focused":false,"agent_status":"idle"}}"#
                )
            })
            .collect();
        format!(
            r#"{{"id":"cli:pane:list","result":{{"type":"pane_list","panes":[{}]}}}}"#,
            rows.join(",")
        )
    }

    fn process_info(chain: &[&[&str]]) -> String {
        let rows: Vec<String> = chain
            .iter()
            .map(|argv| {
                let words: Vec<String> = argv.iter().map(|word| format!("\"{word}\"")).collect();
                format!(r#"{{"pid":100,"argv":[{}]}}"#, words.join(","))
            })
            .collect();
        format!(
            r#"{{"id":"cli:pane:process_info","result":{{"type":"pane_process_info","process_info":{{"foreground_processes":[{}]}}}}}}"#,
            rows.join(",")
        )
    }

    /// herdr 0.9.2's session file: panes `w1:p1`..`w1:pN`, each with its argv.
    fn saved(argvs: &[Option<&str>]) -> String {
        let numbers: Vec<String> = (1..=argvs.len()).map(|n| format!(r#""{n}":{n}"#)).collect();
        let panes: Vec<String> = argvs
            .iter()
            .enumerate()
            .map(|(index, argv)| {
                let resume = argv.map_or(String::new(), |argv| {
                    format!(
                        r#","agent_resume":{{"source":"devlaunch:claude","agent":"claude","argv":{argv}}}"#
                    )
                });
                format!(r#""{}":{{"cwd":"/"{resume}}}"#, index + 1)
            })
            .collect();
        format!(
            r#"{{"workspaces":[{{"id":"w1","public_pane_numbers":{{{}}},"tabs":[{{"panes":{{{}}}}}]}}]}}"#,
            numbers.join(","),
            panes.join(",")
        )
    }

    const CLAUDE_LINE: &str =
        r#"["dl","myws","--","IS_SANDBOX=1","claude","--remote-control=myws","--resume","4b1e"]"#;
    const OTHER_LINE: &str = r#"["dl","other","--","claude","--resume","77aa"]"#;

    fn words(json: &str) -> Vec<String> {
        serde_json::from_str(json).expect("an argv")
    }

    const TRANSPORT: &[&str] = &["ssh", "-F", "/tmp/c", "-t", "myws.devpod", "claude"];

    #[test]
    fn the_panes_in_this_workspace_are_collected_with_their_saved_lines() {
        let runner = ScriptedRunner::new();
        runner.script(
            [HERDR, "pane", "list"],
            Response::stdout(pane_list(&["w1:p1", "w1:p2", "w1:p3"])),
        );
        runner.script(
            [HERDR, "pane", "process-info", "--pane", "w1:p1"],
            Response::stdout(process_info(&[&["dl", "myws", "--", "claude"], TRANSPORT])),
        );
        runner.script(
            [HERDR, "pane", "process-info", "--pane", "w1:p2"],
            Response::stdout(process_info(&[&["ssh", "-t", "other.devpod", "claude"]])),
        );
        // A plain shell in this workspace: nothing saved, and herdr names no
        // agent in it, so there is nothing to start again.
        runner.script(
            [HERDR, "pane", "process-info", "--pane", "w1:p3"],
            Response::stdout(process_info(&[TRANSPORT])),
        );
        let file = saved(&[Some(CLAUDE_LINE), Some(OTHER_LINE), None]);

        let held = collect(&runner, &manager(), "myws", &|path| {
            assert_eq!(path, Path::new("/run/herdr/session.json"));
            Some(file.clone())
        });

        assert_eq!(
            held,
            Some(HeldSessions {
                sessions: vec![HeldSession {
                    pane_id: "w1:p1".to_owned(),
                    line: words(CLAUDE_LINE),
                }],
                unresumable: Vec::new(),
            })
        );
    }

    fn agent(name: &str, status: &str) -> String {
        format!(
            r#"{{"id":"cli:agent:get","result":{{"type":"agent","agent":{{"pane_id":"w1:p1","agent":"{name}","agent_status":"{status}","state_change_seq":4}}}}}}"#
        )
    }

    /// herdr's word on the agent decides the two doubtful panes: a line saved for
    /// an agent that has ended is not typed again, and a live agent with no line
    /// is named so the caller can say it ends.
    #[test]
    fn herdr_says_which_saved_lines_still_stand_for_a_live_agent() {
        let runner = ScriptedRunner::new();
        runner.script(
            [HERDR, "pane", "list"],
            Response::stdout(pane_list(&["w1:p1", "w1:p2", "w1:p3"])),
        );
        runner.script(
            [HERDR, "pane", "process-info"],
            Response::stdout(process_info(&[TRANSPORT])),
        );
        runner.script(
            [HERDR, "agent", "get", "w1:p1"],
            Response::stdout(agent("claude", "done")),
        );
        runner.script(
            [HERDR, "agent", "get", "w1:p2"],
            Response::stdout(agent("claude", "working")),
        );
        runner.script(
            [HERDR, "agent", "get", "w1:p3"],
            Response::stdout(agent("claude", "idle")),
        );
        let file = saved(&[Some(CLAUDE_LINE), None, Some(CLAUDE_LINE)]);

        let held = collect(&runner, &manager(), "myws", &|_| Some(file.clone()));

        assert_eq!(
            held,
            Some(HeldSessions {
                sessions: vec![HeldSession {
                    pane_id: "w1:p3".to_owned(),
                    line: words(CLAUDE_LINE),
                }],
                unresumable: vec!["w1:p2".to_owned()],
            })
        );
    }

    /// A saved line that does not start an agent is a plain attach, and typing it
    /// again would bring back nothing a recreate took.
    #[test]
    fn a_saved_line_that_starts_no_agent_is_not_a_session() {
        let runner = ScriptedRunner::new();
        runner.script(
            [HERDR, "pane", "list"],
            Response::stdout(pane_list(&["w1:p1"])),
        );
        runner.script(
            [HERDR, "pane", "process-info"],
            Response::stdout(process_info(&[TRANSPORT])),
        );
        let file = saved(&[Some(r#"["dl","myws","--","make","claude"]"#)]);

        let held = collect(&runner, &manager(), "myws", &|_| Some(file.clone()));

        assert_eq!(held, Some(HeldSessions::default()));
    }

    /// A pane attached to this workspace can still hold a line saved by an
    /// earlier `dl other -- claude` in it, since a plain attach saves nothing.
    /// Typing that line would start an agent in the other workspace, so the live
    /// agent here is one the recreate ends with no way back.
    #[test]
    fn a_saved_line_into_another_workspace_is_not_this_workspaces_session() {
        let runner = ScriptedRunner::new();
        runner.script(
            [HERDR, "pane", "list"],
            Response::stdout(pane_list(&["w1:p1"])),
        );
        runner.script(
            [HERDR, "pane", "process-info"],
            Response::stdout(process_info(&[TRANSPORT])),
        );
        runner.script(
            [HERDR, "agent", "get", "w1:p1"],
            Response::stdout(agent("claude", "idle")),
        );
        let file = saved(&[Some(OTHER_LINE)]);

        let held = collect(&runner, &manager(), "myws", &|_| Some(file.clone()));

        assert_eq!(
            held,
            Some(HeldSessions {
                sessions: Vec::new(),
                unresumable: vec!["w1:p1".to_owned()],
            })
        );
    }

    /// What part 3 of devlaunch#673 reads to tell an idle workspace from a busy
    /// one, word for word as herdr 0.9 spells `agent_status`.
    #[test]
    fn each_agent_status_herdr_reports_is_read_as_itself() {
        use herdr::{AgentReading, AgentStatus, PaneAgent};
        for (word, status) in [
            ("idle", AgentStatus::Idle),
            ("working", AgentStatus::Working),
            ("blocked", AgentStatus::Blocked),
            ("done", AgentStatus::Done),
            ("unknown", AgentStatus::Unknown),
            ("thinking-hard", AgentStatus::Unknown),
        ] {
            let runner = ScriptedRunner::new().with_script(
                [HERDR, "agent", "get"],
                Response::stdout(agent("codex", word)),
            );
            assert_eq!(
                agent_reading(&runner, &manager(), "w1:p1"),
                AgentReading::Agent(PaneAgent {
                    agent: "codex".to_owned(),
                    status,
                }),
                "{word}"
            );
        }
        let none = ScriptedRunner::new().with_script(
            [HERDR, "agent", "get"],
            Response::stdout(r#"{"id":"x","result":{"type":"agent","agent":null}}"#),
        );
        assert_eq!(
            agent_reading(&none, &manager(), "w1:p1"),
            AgentReading::NoAgent
        );
        let refused = ScriptedRunner::new().with_script(
            [HERDR, "agent", "get"],
            Response::failed(1, "").and_stdout(r#"{"error":{"code":"pane_not_found"}}"#),
        );
        assert_eq!(
            agent_reading(&refused, &manager(), "w1:p1"),
            AgentReading::Unanswered
        );
    }

    #[test]
    fn a_herdr_that_will_not_list_its_panes_is_no_answer() {
        let runner =
            ScriptedRunner::new().with_script([HERDR, "pane", "list"], Response::exited(1));

        assert_eq!(collect(&runner, &manager(), "myws", &|_| None), None);
    }

    fn held(panes: &[&str]) -> HeldSessions {
        HeldSessions {
            sessions: panes
                .iter()
                .map(|pane| HeldSession {
                    pane_id: (*pane).to_owned(),
                    line: words(CLAUDE_LINE),
                })
                .collect(),
            unresumable: Vec::new(),
        }
    }

    const TYPED: &str = "dl myws -- IS_SANDBOX=1 claude --remote-control=myws --resume 4b1e";

    #[test]
    fn each_line_is_typed_into_the_pane_it_came_from() {
        let runner = ScriptedRunner::new();
        runner.script([HERDR, "pane", "run"], Response::ok());

        let restarted = restart(&runner, &manager(), &held(&["w1:p1", "w1:p4"]), &|_| {
            panic!("nothing was refused, so nothing waits")
        });

        assert_eq!(
            restarted,
            vec![
                Restarted::Typed {
                    pane_id: "w1:p1".to_owned()
                },
                Restarted::Typed {
                    pane_id: "w1:p4".to_owned()
                },
            ]
        );
        assert_eq!(
            runner.args_to(HERDR),
            vec![
                vec!["pane", "run", "w1:p1", TYPED],
                vec!["pane", "run", "w1:p4", TYPED],
            ]
        );
    }

    /// The pane this `dl` runs in has this `dl` in its foreground, and a line
    /// typed there would wait behind the shell the recreate attaches to.
    #[test]
    fn the_pane_this_dl_runs_in_is_handed_its_line_rather_than_typed_into() {
        let runner = ScriptedRunner::new();

        let restarted = restart(&runner, &manager(), &held(&["w1:p9"]), &|_| {});

        assert_eq!(
            restarted,
            vec![Restarted::OwnPane {
                pane_id: "w1:p9".to_owned(),
                line: TYPED.to_owned(),
            }]
        );
        assert!(runner.args_to(HERDR).is_empty());
    }

    /// herdr refuses `pane run` into a pane whose foreground is not its shell, and
    /// the old transport takes a moment to exit once its container is gone. So a
    /// refusal is asked again, a bounded number of times, and then given up.
    #[test]
    fn a_pane_that_stays_busy_is_given_up_on_after_a_bounded_wait() {
        let runner = ScriptedRunner::new();
        runner.script(
            [HERDR, "pane", "run"],
            Response::failed(1, "").and_stdout(r#"{"error":{"code":"pane_busy"}}"#),
        );
        let waited = std::cell::Cell::new(Duration::ZERO);

        let restarted = restart(&runner, &manager(), &held(&["w1:p1"]), &|pause| {
            waited.set(waited.get() + pause);
        });

        assert_eq!(
            restarted,
            vec![Restarted::NotTyped {
                pane_id: "w1:p1".to_owned(),
                line: TYPED.to_owned(),
            }]
        );
        let tries = runner.args_to(HERDR).len();
        assert!(tries > 1, "a refusal was not asked again");
        assert_eq!(waited.get(), RUN_TICK * u32::try_from(tries - 1).unwrap());
        assert!(
            waited.get() <= Duration::from_secs(30),
            "{:?}",
            waited.get()
        );
    }

    #[test]
    fn outside_a_herdr_pane_there_is_no_manager_to_ask() {
        let mut outside = env("w1:p9");
        outside.in_pane = None;
        assert_eq!(Manager::resolve(&outside, Some(HERDR)), None);
        let mut no_socket = env("w1:p9");
        no_socket.socket = None;
        assert_eq!(Manager::resolve(&no_socket, Some(HERDR)), None);
    }
}
