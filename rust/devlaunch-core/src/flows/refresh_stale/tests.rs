use super::*;
use crate::testing::ScriptedRunner;
use devlaunch_test_support::Response;

const HERDR: &str = "/opt/herdr/bin/herdr";

fn stale() -> StaleImages {
    // `of_pairs` names each container after its workspace.
    StaleImages::of_pairs([("myws", "ghcr.io/o/img:latest")])
}

/// One process as the in-container script prints it: its stat line and its
/// argv joined by unit separators.
fn process(pid: u32, ppid: u32, comm: &str, argv: &[&str]) -> String {
    format!(
        "{pid} ({comm}) S {ppid} {pid} {pid} 0 -1 4194560\n{}\n",
        argv.join("\u{1f}")
    )
}

fn table(processes: &[String]) -> String {
    format!("{}devlaunch-end\n", processes.concat())
}

fn quiet() -> Vec<String> {
    vec![
        process(1, 0, "sleep", &["sleep", "infinity"]),
        process(40, 1, "sshd", &["sshd: vscode"]),
        process(41, 40, "bash", &["-bash"]),
    ]
}

fn with_claude() -> Vec<String> {
    let mut processes = quiet();
    processes.push(process(
        50,
        41,
        "claude",
        &["claude", "--remote-control=myws", "--resume", "4b1e"],
    ));
    // An MCP server: started directly, not through a shell.
    processes.push(process(51, 50, "node", &["node", "/usr/lib/mcp/server.js"]));
    processes
}

fn runner_reading(processes: &[String]) -> ScriptedRunner {
    ScriptedRunner::new().with_script(
        ["docker", "exec", "--user", "0", "myws", "sh", "-c"],
        Response::stdout(table(processes)),
    )
}

fn plan_of(
    runner: &ScriptedRunner,
    manager: Option<&agent_sessions::Manager>,
    cache: &Path,
    session: Option<String>,
) -> Vec<Verdict> {
    plan(
        runner,
        &stale(),
        ["myws", "current"],
        manager,
        &LaunchLocks::under(cache),
        &|_| session.clone(),
    )
    .into_iter()
    .map(|planned| {
        assert_eq!(planned.workspace_id, "myws");
        assert_eq!(planned.reference, "ghcr.io/o/img:latest");
        planned.verdict
    })
    .collect()
}

fn cache() -> tempfile::TempDir {
    tempfile::tempdir().expect("a cache dir")
}

#[test]
fn a_stale_workspace_where_nothing_runs_is_refreshed_and_a_current_one_is_left_out() {
    let runner = runner_reading(&quiet());
    let cache = cache();

    assert_eq!(
        plan_of(&runner, None, cache.path(), None),
        vec![Verdict::Refresh]
    );
}

#[test]
fn each_build_program_is_a_reason_to_skip() {
    for program in BUILD_PROGRAMS {
        let mut processes = quiet();
        processes.push(process(77, 41, program, &[program, "build"]));
        let runner = runner_reading(&processes);
        let cache = cache();

        assert_eq!(
            plan_of(&runner, None, cache.path(), None),
            vec![Verdict::Skip(Skip::Building {
                program: (*program).to_owned(),
                pid: 77,
            })],
            "{program}"
        );
    }
}

/// A finished compiler its parent has not reaped yet is not a build.
#[test]
fn a_zombie_is_not_a_build() {
    let mut processes = quiet();
    processes.push("77 (cc1plus) Z 41 77 77 0 -1 4194560\n\n".to_owned());
    let runner = runner_reading(&processes);
    let cache = cache();

    assert_eq!(
        plan_of(&runner, None, cache.path(), None),
        vec![Verdict::Refresh]
    );
}

#[test]
fn a_shell_under_claude_is_a_command_it_runs() {
    let mut processes = with_claude();
    processes.push(process(60, 50, "bash", &["/bin/bash", "-c", "sleep 600"]));
    let runner = runner_reading(&processes);
    let cache = cache();

    assert_eq!(
        plan_of(&runner, None, cache.path(), None),
        vec![Verdict::Skip(Skip::AgentRunsACommand {
            program: "bash".to_owned(),
            pid: 60,
        })]
    );
}

/// The npm install runs Claude as `node .../claude`; its tool calls count too.
#[test]
fn a_claude_run_by_node_is_claude() {
    let mut processes = quiet();
    processes.push(process(
        50,
        41,
        "node",
        &["node", "/usr/local/bin/claude", "--resume", "4b1e"],
    ));
    processes.push(process(60, 50, "sh", &["/bin/sh", "-c", "make"]));
    let runner = runner_reading(&processes);
    let cache = cache();

    assert_eq!(
        plan_of(&runner, None, cache.path(), None),
        vec![Verdict::Skip(Skip::AgentRunsACommand {
            program: "sh".to_owned(),
            pid: 60,
        })]
    );
}

#[test]
fn outside_herdr_a_workspace_running_claude_is_skipped() {
    let runner = runner_reading(&with_claude());
    let cache = cache();

    assert_eq!(
        plan_of(&runner, None, cache.path(), None),
        vec![Verdict::Skip(Skip::SessionsUnseen)]
    );
}

#[test]
fn a_container_whose_processes_cannot_be_read_is_skipped() {
    let runner = ScriptedRunner::new().with_script(
        ["docker", "exec"],
        Response::failed(1, "Error response from daemon: container is not running\n"),
    );
    let cache = cache();

    let verdicts = plan_of(&runner, None, cache.path(), None);

    let [Verdict::Skip(Skip::ProcessesUnread { why })] = verdicts.as_slice() else {
        panic!("{verdicts:?}");
    };
    assert!(why.contains("is not running"), "{why}");
}

#[test]
fn a_listing_cut_short_is_not_read_as_a_quiet_container() {
    let runner = ScriptedRunner::new().with_script(
        ["docker", "exec"],
        Response::stdout(process(77, 41, "cc1plus", &["cc1plus"])),
    );
    let cache = cache();

    assert!(matches!(
        plan_of(&runner, None, cache.path(), None).as_slice(),
        [Verdict::Skip(Skip::ProcessesUnread { .. })]
    ));
}

#[test]
fn a_workspace_another_dl_is_launching_is_skipped_before_docker_is_asked() {
    let runner = runner_reading(&quiet());
    let cache = cache();
    let locks = LaunchLocks::under(cache.path());
    let _held = locks::hold_lock(&locks.path_for("myws")).expect("the lock");

    assert_eq!(
        plan_of(&runner, None, cache.path(), None),
        vec![Verdict::Skip(Skip::LaunchUnderWay)]
    );
    assert!(runner.args_to("docker").is_empty());
}

// --- the sessions herdr holds

fn manager() -> agent_sessions::Manager {
    agent_sessions::Manager::resolve(
        &herdr::HostEnv {
            enabled: None,
            in_pane: Some("1".to_owned()),
            pane_id: Some("w1:p9".to_owned()),
            socket: Some("/run/herdr/herdr.sock".to_owned()),
            binary: Some(HERDR.to_owned()),
        },
        Some(HERDR),
    )
    .expect("a herdr pane")
}

const CLAUDE_LINE: &str =
    r#"["dl","myws","--","claude","--remote-control=myws","--resume","4b1e"]"#;

/// herdr with pane `w1:p1` attached to `myws`, its saved line `line`, and
/// `status` as its agent's state.
fn herdr_holding(runner: &ScriptedRunner, line: Option<&str>, status: &str) -> String {
    runner.script(
        [HERDR, "pane", "list"],
        Response::stdout(
            r#"{"id":"l","result":{"type":"pane_list","panes":[{"pane_id":"w1:p1","tab_id":"w1:t1","focused":false}]}}"#,
        ),
    );
    runner.script(
        [HERDR, "pane", "process-info"],
        Response::stdout(
            r#"{"id":"p","result":{"type":"pane_process_info","process_info":{"foreground_processes":[{"argv":["devpod","ssh","myws"]}]}}}"#,
        ),
    );
    runner.script(
        [HERDR, "agent", "get", "w1:p1"],
        Response::stdout(format!(
            r#"{{"id":"a","result":{{"type":"agent","agent":{{"agent":"claude","agent_status":"{status}"}}}}}}"#
        )),
    );
    let resume = line.map_or(String::new(), |argv| {
        format!(r#","agent_resume":{{"argv":{argv}}}"#)
    });
    format!(
        r#"{{"workspaces":[{{"id":"w1","public_pane_numbers":{{"1":1}},"tabs":[{{"panes":{{"1":{{"cwd":"/"{resume}}}}}}}]}}]}}"#
    )
}

#[test]
fn an_idle_claude_herdr_holds_a_line_for_is_refreshed() {
    for status in ["idle", "done"] {
        let runner = runner_reading(&with_claude());
        let session = herdr_holding(&runner, Some(CLAUDE_LINE), status);
        let cache = cache();

        let verdicts = plan_of(&runner, Some(&manager()), cache.path(), Some(session));

        // A `done` agent's line is not held, so the one Claude process is then
        // one herdr's panes do not hold.
        let expected = if status == "idle" {
            Verdict::Refresh
        } else {
            Verdict::Skip(Skip::SessionsOutsidePanes {
                running: 1,
                held: 0,
            })
        };
        assert_eq!(verdicts, vec![expected], "{status}");
    }
}

#[test]
fn an_agent_that_is_not_idle_is_skipped_with_what_it_is_doing() {
    for (status, state) in [
        ("working", AgentState::Working),
        ("blocked", AgentState::Blocked),
        ("unknown", AgentState::Unknown),
    ] {
        let runner = runner_reading(&with_claude());
        let session = herdr_holding(&runner, Some(CLAUDE_LINE), status);
        let cache = cache();

        assert_eq!(
            plan_of(&runner, Some(&manager()), cache.path(), Some(session)),
            vec![Verdict::Skip(Skip::AgentBusy {
                pane_id: "w1:p1".to_owned(),
                state,
            })],
            "{status}"
        );
    }
}

#[test]
fn a_live_agent_with_no_saved_line_is_skipped() {
    let runner = runner_reading(&with_claude());
    let session = herdr_holding(&runner, None, "idle");
    let cache = cache();

    assert_eq!(
        plan_of(&runner, Some(&manager()), cache.path(), Some(session)),
        vec![Verdict::Skip(Skip::Unresumable {
            pane_id: "w1:p1".to_owned(),
        })]
    );
}

/// A second Claude started from a terminal outside herdr would end with
/// nothing to start it again.
#[test]
fn more_claude_sessions_than_herdr_holds_is_a_skip() {
    let mut processes = with_claude();
    processes.push(process(52, 41, "claude", &["claude"]));
    let runner = runner_reading(&processes);
    let session = herdr_holding(&runner, Some(CLAUDE_LINE), "idle");
    let cache = cache();

    assert_eq!(
        plan_of(&runner, Some(&manager()), cache.path(), Some(session)),
        vec![Verdict::Skip(Skip::SessionsOutsidePanes {
            running: 2,
            held: 1,
        })]
    );
}

#[test]
fn a_herdr_that_does_not_list_its_panes_is_a_skip() {
    let runner = runner_reading(&with_claude());
    runner.script([HERDR, "pane", "list"], Response::exited(1));
    let cache = cache();

    assert_eq!(
        plan_of(&runner, Some(&manager()), cache.path(), None),
        vec![Verdict::Skip(Skip::HerdrUnanswered)]
    );
}

/// A stat line's name sits in parentheses and may hold spaces and parentheses
/// of its own, so the parse takes the last `) ` as its end. A process whose
/// cmdline is empty (a kernel thread, or one that exited mid-read) has no argv.
#[test]
fn the_table_reads_what_the_script_prints() {
    let printed = "1 (sleep) S 0 1 1 0 -1 4194560 155 0 0 0 0 0 0 0 20 0 1 0 1 1\n\
                   sleep\u{1f}infinity\u{1f}\n\
                   12 (a (b) c) R 1 12 1 34816 12 4194304 101 0 0 0 0 0 0 0 20 0 1 0 2 1\n\
                   \n\
                   devlaunch-end\n";

    assert_eq!(
        process_table(printed),
        Ok(vec![
            Process {
                pid: 1,
                ppid: 0,
                comm: "sleep".to_owned(),
                argv: vec!["sleep".to_owned(), "infinity".to_owned()],
            },
            Process {
                pid: 12,
                ppid: 1,
                comm: "a (b) c".to_owned(),
                argv: Vec::new(),
            },
        ])
    );
}
