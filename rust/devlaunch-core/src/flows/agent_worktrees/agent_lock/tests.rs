//! The stale-lock proof, at its own seam: a fake procfs on disk for this
//! machine, and a scripted docker for the containers.

use devlaunch_test_support::{FakeRunner, Response};

use std::os::unix::fs::MetadataExt;

use super::*;

/// The lock that stopped `dl kinisi-ros-feat-esdf-false-negative-audit-986w rme`
/// on 2026-10-05, word for word.
const THE_INCIDENT: &str = "claude agent agent-a540dbaa96aa45899 (pid 8621 start 329153)";

fn incident() -> AgentLock {
    parse(THE_INCIDENT).expect("the incident's lock parses")
}

/// One `/proc/<pid>/stat` line with `start` in field 22, and a command name
/// that has a space and a parenthesis in it, the way a real one may.
fn stat(pid: u32, start: u64) -> String {
    stat_with_flags(pid, 4_194_560, start)
}

/// kthreadd's flags on a real host: `PF_KTHREAD` is among them.
const KTHREADD_FLAGS: u64 = 2_129_984;

fn stat_with_flags(pid: u32, flags: u64, start: u64) -> String {
    format!(
        "{pid} (my (odd) name) S 0 {pid} {pid} 0 -1 {flags} 100 0 0 0 1 2 0 0 20 0 1 0 {start} \
         1000 10 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 3 0 0 0 0 0\n"
    )
}

// =======================================================================
// the reason
// =======================================================================

#[test]
fn the_incidents_reason_reads_as_an_agent_lock() {
    assert_eq!(
        parse(THE_INCIDENT),
        Some(AgentLock {
            pid: 8621,
            start: 329_153
        })
    );
}

#[test]
fn every_other_shape_is_not_an_agent_lock() {
    for reason in [
        // No start time: nothing to tell a reused pid apart by.
        "claude agent agent-a540 (pid 8621)",
        // A person's `claude --worktree`, not an agent.
        "claude session s-1 (pid 8621 start 329153)",
        // Claude Code on macOS writes `ps -o lstart`, which is not ticks.
        "claude agent agent-a540 (pid 8621 start Mon Oct  5 10:00:00 2026)",
        "claude agent agent-a540 (pid 8621 start 329153) and more",
        "claude agent  (pid 8621 start 329153)",
        "claude agent a b (pid 8621 start 329153)",
        "claude agent agent-a540 (pid 0 start 329153)",
        "claude agent agent-a540 (pid -1 start 329153)",
        "claude agent agent-a540 (pid 99999999999 start 329153)",
        "claude agent agent-a540 (pid 8621 start )",
        "Claude agent agent-a540 (pid 8621 start 329153)",
        "",
    ] {
        assert_eq!(parse(reason), None, "{reason:?}");
    }
}

// =======================================================================
// one process table
// =======================================================================

fn process(pid: u32, start: u64, nspid: Option<&[u32]>) -> Process {
    Process {
        pid,
        kernel_thread: false,
        start,
        nspid: nspid.map(<[u32]>::to_vec),
    }
}

#[test]
fn a_table_without_the_pid_is_gone() {
    let table = [
        process(1, 10, Some(&[1])),
        process(77, 329_153, Some(&[77])),
    ];
    assert_eq!(decide(&table, &incident()), InOne::Gone);
}

#[test]
fn the_pid_with_the_start_time_is_running() {
    let table = [process(1, 10, Some(&[1])), process(8621, 329_153, None)];
    assert_eq!(decide(&table, &incident()), InOne::Running);
}

#[test]
fn a_reused_pid_is_gone() {
    let table = [
        process(1, 10, Some(&[1])),
        process(8621, 400_000, Some(&[8621])),
    ];
    assert_eq!(decide(&table, &incident()), InOne::Gone);
}

#[test]
fn an_agent_in_a_nested_namespace_is_running() {
    // Seen from outside, the nested agent has another pid. Its innermost NSpid
    // entry is the one it wrote into the lock.
    let table = [
        process(1, 10, Some(&[1])),
        process(53_000, 329_153, Some(&[53_000, 8621])),
    ];
    assert_eq!(decide(&table, &incident()), InOne::Running);
}

#[test]
fn a_same_moment_process_whose_nspid_is_unknown_could_not_be_told_apart() {
    let table = [process(1, 10, Some(&[1])), process(53_000, 329_153, None)];
    assert!(matches!(
        decide(&table, &incident()),
        InOne::CouldNotTell(_)
    ));
}

#[test]
fn a_table_that_does_not_list_pid_one_proves_nothing() {
    // No procfs, or `hidepid`: every pid would read as gone.
    let table = [process(500, 10, Some(&[500]))];
    assert!(matches!(
        decide(&table, &incident()),
        InOne::CouldNotTell(_)
    ));
    assert!(matches!(decide(&[], &incident()), InOne::CouldNotTell(_)));
}

#[test]
fn the_containers_listing_is_read_whole_or_not_at_all() {
    let out = format!(
        "{}{}devlaunch-nspid\n/proc/1/status:NSpid:\t1\n/proc/40/status:NSpid:\t40\t8621\n\
         devlaunch-end\n",
        stat(1, 10),
        stat(40, 329_153)
    );
    let table = container_table(&out).expect("a whole listing reads");
    assert_eq!(
        table,
        [
            process(1, 10, Some(&[1])),
            process(40, 329_153, Some(&[40, 8621]))
        ]
    );

    let cut_short = out.replace("devlaunch-end\n", "");
    assert!(container_table(&cut_short).is_err());
    let no_middle = stat(1, 10);
    assert!(container_table(&no_middle).is_err());
    let odd_line = out.replace(&stat(40, 329_153), "40 (x) S\n");
    assert!(container_table(&odd_line).is_err());
}

// =======================================================================
// a fake machine: a procfs, a clone, and the containers docker lists
// =======================================================================

struct Machine {
    dir: tempfile::TempDir,
    docker: FakeRunner,
}

impl Machine {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let machine = Self {
            dir,
            docker: FakeRunner::new(),
        };
        std::fs::create_dir_all(machine.site()).expect("the site");
        std::fs::create_dir_all(machine.proc_root()).expect("a procfs");
        machine.host_process(1, 10, &[1]);
        machine.whole_machine();
        machine
    }

    /// Make this machine's procfs a pid namespace's own: no kernel thread.
    fn only_a_namespace(&self) {
        std::fs::remove_dir_all(self.proc_root().join("2")).expect("the kernel thread going");
    }

    fn clone(&self) -> PathBuf {
        self.dir.path().join("repos").join("o").join("r").join("ws")
    }

    fn site(&self) -> PathBuf {
        self.clone()
            .join(".claude/worktrees/agent-a540dbaa96aa45899")
    }

    fn proc_root(&self) -> PathBuf {
        self.dir.path().join("proc")
    }

    fn host_process(&self, pid: u32, start: u64, nspid: &[u32]) {
        self.host_process_with_flags(pid, 4_194_560, start, nspid);
    }

    fn host_process_with_flags(&self, pid: u32, flags: u64, start: u64, nspid: &[u32]) {
        let at = self.proc_root().join(pid.to_string());
        std::fs::create_dir_all(&at).expect("a process directory");
        std::fs::write(at.join("stat"), stat_with_flags(pid, flags, start)).expect("its stat");
        let nspid: Vec<String> = nspid.iter().map(u32::to_string).collect();
        std::fs::write(
            at.join("status"),
            format!("Name:\tx\nNSpid:\t{}\n", nspid.join("\t")),
        )
        .expect("its status");
    }

    /// Make this machine's procfs the initial pid namespace's: a kernel thread
    /// is in it. No other namespace lists one.
    fn whole_machine(&self) {
        self.host_process_with_flags(2, KTHREADD_FLAGS, 0, &[2]);
    }

    /// docker lists these containers, described by `inspect`.
    fn containers(&self, described: &[serde_json::Value]) {
        let ids: Vec<String> = described
            .iter()
            .map(|it| it["Id"].as_str().expect("an id").to_owned())
            .collect();
        self.docker.script(
            ["docker", "ps", "--all"],
            Response::stdout(format!("{}\n", ids.join("\n"))),
        );
        self.docker.script(
            ["docker", "inspect"],
            Response::stdout(serde_json::Value::Array(described.to_vec()).to_string()),
        );
    }

    /// What reading the process table inside `id` prints.
    fn inside(&self, id: &str, processes: &[(u32, u64, &[u32])]) {
        let mut out = String::new();
        for (pid, start, _) in processes {
            out.push_str(&stat(*pid, *start));
        }
        out.push_str("devlaunch-nspid\n");
        for (pid, _, nspid) in processes {
            let nspid: Vec<String> = nspid.iter().map(u32::to_string).collect();
            out.push_str(&format!(
                "/proc/{pid}/status:NSpid:\t{}\n",
                nspid.join("\t")
            ));
        }
        out.push_str("devlaunch-end\n");
        self.docker.script(
            [
                "docker",
                "exec",
                "--user",
                "0",
                id,
                "sh",
                "-c",
                READ_THE_TABLE,
            ],
            Response::stdout(out),
        );
    }

    /// What asking `id` whether the recorded path is the site prints.
    fn inside_the_path_is(&self, id: &str, answer: &str) {
        self.docker.script(
            [
                "docker",
                "exec",
                "--user",
                "0",
                id,
                "sh",
                "-c",
                IS_IT_THE_SITE,
            ],
            Response::stdout(format!("{answer}\n")),
        );
    }

    /// The site's device and inode, as `stat -c %d:%i` prints them.
    fn site_identity(&self) -> String {
        let here = std::fs::metadata(self.site()).expect("the site");
        format!("{}:{}", here.dev(), here.ino())
    }

    fn owner(&self, recorded: &str) -> Owner {
        Owners::reading(&self.docker, self.proc_root()).owner(
            &self.site(),
            Path::new(recorded),
            &incident(),
        )
    }
}

/// A container in docker's own words, with its mounts as (source,
/// destination) pairs.
fn container(id: &str, status: &str, mounts: &[(&Path, &str)]) -> serde_json::Value {
    serde_json::json!({
        "Id": id,
        "Name": format!("/{id}-name"),
        "State": { "Status": status },
        "Mounts": mounts
            .iter()
            .map(|(source, destination)| serde_json::json!({
                "Type": "bind",
                "Source": source.display().to_string(),
                "Destination": destination,
            }))
            .collect::<Vec<_>>(),
    })
}

/// Where the workspace container mounts the clone, as the kinisi_ros
/// devcontainer does.
const MOUNTED_AT: &str = "/home/kinisi/kinisi/kinisi_ros";
const RECORDED: &str = "/home/kinisi/kinisi/kinisi_ros/.claude/worktrees/agent-a540dbaa96aa45899";

#[test]
fn the_incident_with_the_agent_gone_from_its_running_container_is_gone() {
    let machine = Machine::new();
    machine.containers(&[container(
        "ws",
        "running",
        &[(&machine.clone(), MOUNTED_AT)],
    )]);
    machine.inside("ws", &[(1, 10, &[1]), (77, 500, &[77])]);

    assert_eq!(machine.owner(RECORDED), Owner::Gone);
}

#[test]
fn the_agent_still_running_in_its_container_is_named() {
    let machine = Machine::new();
    machine.containers(&[container(
        "ws",
        "running",
        &[(&machine.clone(), MOUNTED_AT)],
    )]);
    machine.inside("ws", &[(1, 10, &[1]), (8621, 329_153, &[8621])]);

    let Owner::StillRunning(place) = machine.owner(RECORDED) else {
        panic!("a live agent's lock must stand");
    };
    assert!(
        place.contains("ws-name") && place.contains("8621"),
        "{place}"
    );
}

#[test]
fn a_stopped_container_has_no_agent_left() {
    for status in ["exited", "created", "dead"] {
        let machine = Machine::new();
        machine.containers(&[container("ws", status, &[(&machine.clone(), MOUNTED_AT)])]);
        assert_eq!(machine.owner(RECORDED), Owner::Gone, "{status}");
        assert!(
            machine
                .docker
                .args_to("docker")
                .iter()
                .all(|it| it[0] != "exec"),
            "a stopped container is not exec'd into"
        );
    }
}

#[test]
fn a_paused_or_restarting_container_could_not_be_told() {
    for status in ["paused", "restarting", "removing"] {
        let machine = Machine::new();
        machine.containers(&[container("ws", status, &[(&machine.clone(), MOUNTED_AT)])]);
        assert!(
            matches!(machine.owner(RECORDED), Owner::CouldNotTell(_)),
            "{status}"
        );
    }
}

#[test]
fn every_container_that_sees_the_site_has_to_say_gone() {
    // The kinisi_ros devcontainers bind the whole of `~/.cache` too, so another
    // workspace's container sees this clone under its own path. A stopped
    // container and a running one with the agent in it: the live one wins.
    let machine = Machine::new();
    let cache = machine.dir.path().join("repos");
    let through_the_cache =
        "/home/kinisi/.cache/repos/o/r/ws/.claude/worktrees/agent-a540dbaa96aa45899";
    machine.containers(&[
        container(
            "ws",
            "exited",
            &[
                (&machine.clone(), MOUNTED_AT),
                (&cache, "/home/kinisi/.cache/repos"),
            ],
        ),
        container("other", "running", &[(&cache, "/home/kinisi/.cache/repos")]),
    ]);
    machine.inside("other", &[(1, 10, &[1]), (8621, 329_153, &[8621])]);

    assert!(matches!(
        machine.owner(through_the_cache),
        Owner::StillRunning(_)
    ));
}

#[test]
fn another_workspaces_container_at_the_same_path_is_not_asked() {
    // Every kinisi_ros container mounts its own clone at the same path. Only
    // the one whose clone this is sees the site there.
    let machine = Machine::new();
    let elsewhere = machine.dir.path().join("repos/o/r/other-ws");
    std::fs::create_dir_all(&elsewhere).expect("another clone");
    machine.containers(&[
        container("ws", "running", &[(&machine.clone(), MOUNTED_AT)]),
        container("other", "running", &[(&elsewhere, MOUNTED_AT)]),
    ]);
    machine.inside("ws", &[(1, 10, &[1])]);
    machine.inside("other", &[(1, 10, &[1]), (8621, 329_153, &[8621])]);

    assert_eq!(machine.owner(RECORDED), Owner::Gone);
    assert!(
        machine
            .docker
            .args_to("docker")
            .iter()
            .filter(|it| it[0] == "exec")
            .all(|it| !it.contains(&"other".to_owned())),
        "the other workspace's container is never read"
    );
}

#[test]
fn a_mount_deeper_than_the_clone_shadows_it() {
    // A volume over `.claude/worktrees` inside the container: the recorded path
    // lands in the volume, not on the site, so nothing sees the site there.
    let machine = Machine::new();
    let volume = machine.dir.path().join("volume");
    std::fs::create_dir_all(&volume).expect("a volume directory");
    machine.containers(&[container(
        "ws",
        "running",
        &[
            (&machine.clone(), MOUNTED_AT),
            (&volume, "/home/kinisi/kinisi/kinisi_ros/.claude/worktrees"),
        ],
    )]);
    machine.inside("ws", &[(1, 10, &[1])]);

    let Owner::CouldNotTell(why) = machine.owner(RECORDED) else {
        panic!("the clone's mount is shadowed there, so nothing sees the site");
    };
    assert!(why.contains("nothing on this machine sees it"), "{why}");
}

#[test]
fn a_path_no_namespace_maps_to_the_site_could_not_be_told() {
    let machine = Machine::new();
    machine.containers(&[]);
    let Owner::CouldNotTell(why) = machine.owner(RECORDED) else {
        panic!("no namespace is no proof");
    };
    assert!(why.contains("nothing on this machine sees it"), "{why}");
}

#[test]
fn a_mount_source_this_machine_cannot_see_could_not_be_told() {
    // A source that will not resolve for a reason other than being absent
    // (here a symlink loop, which holds for root too) may still be the site.
    let machine = Machine::new();
    let unresolvable = machine.dir.path().join("loop");
    std::os::unix::fs::symlink(&unresolvable, &unresolvable).expect("a symlink loop");
    beside_a_clean_container(
        &machine,
        container("other", "running", &[(&unresolvable, MOUNTED_AT)]),
    );

    let Owner::CouldNotTell(why) = machine.owner(RECORDED) else {
        panic!("a source that will not resolve proves nothing");
    };
    assert!(why.contains("could not see"), "{why}");
}

#[test]
fn a_mapped_path_that_will_not_resolve_could_not_be_told() {
    // The source resolves, but the path under it does not, and not because
    // nothing is there.
    let machine = Machine::new();
    let source = machine.dir.path().join("source");
    std::fs::create_dir_all(&source).expect("a source");
    std::os::unix::fs::symlink(source.join(".claude"), source.join(".claude"))
        .expect("a symlink loop");
    beside_a_clean_container(
        &machine,
        container("other", "running", &[(&source, MOUNTED_AT)]),
    );

    let Owner::CouldNotTell(why) = machine.owner(RECORDED) else {
        panic!("a path that will not resolve proves nothing");
    };
    assert!(why.contains("could not resolve"), "{why}");
}

#[test]
fn a_mount_docker_described_without_a_source_could_not_be_told() {
    // docker prints `"Source": ""` for a tmpfs, so an empty source is a mount
    // with no directory behind it. A field that is absent or not a string is
    // a description this module cannot read, and the container it belongs to
    // might be the one with the agent in it.
    let shadowing = |source: Option<serde_json::Value>| {
        let mut mount = serde_json::json!({
            "Type": "tmpfs",
            "Destination": "/home/kinisi/kinisi/kinisi_ros/.claude/worktrees",
        });
        if let Some(source) = source {
            mount["Source"] = source;
        }
        let mut other = container("other", "running", &[]);
        other["Mounts"] = serde_json::json!([mount]);
        other
    };
    let with = |source: Option<serde_json::Value>| {
        let machine = Machine::new();
        machine.containers(&[
            container("ws", "running", &[(&machine.clone(), MOUNTED_AT)]),
            shadowing(source),
        ]);
        machine.inside("ws", &[(1, 10, &[1])]);
        machine.owner(RECORDED)
    };

    assert_eq!(with(Some(serde_json::json!(""))), Owner::Gone);
    for source in [
        None,
        Some(serde_json::Value::Null),
        Some(serde_json::json!(7)),
    ] {
        assert!(
            matches!(with(source.clone()), Owner::CouldNotTell(_)),
            "{source:?}"
        );
    }
}

#[test]
fn every_docker_failure_could_not_be_told() {
    let missing = Machine::new();
    missing.docker.script_missing("docker");
    assert!(matches!(missing.owner(RECORDED), Owner::CouldNotTell(_)));

    let refused = Machine::new();
    refused
        .docker
        .script(["docker", "ps"], Response::failed(1, "no daemon\n"));
    assert!(matches!(refused.owner(RECORDED), Owner::CouldNotTell(_)));

    let slow = Machine::new();
    slow.docker.script(["docker", "ps"], Response::TimedOut);
    assert!(matches!(slow.owner(RECORDED), Owner::CouldNotTell(_)));

    let odd = Machine::new();
    odd.docker
        .script(["docker", "ps", "--all"], Response::stdout("ws\n"));
    odd.docker.script(
        ["docker", "inspect"],
        Response::stdout("[{\"Id\": \"ws\"}]"),
    );
    assert!(matches!(odd.owner(RECORDED), Owner::CouldNotTell(_)));

    let exec_failed = Machine::new();
    exec_failed.containers(&[container(
        "ws",
        "running",
        &[(&exec_failed.clone(), MOUNTED_AT)],
    )]);
    exec_failed.docker.script(
        ["docker", "exec"],
        Response::failed(126, "exec: \"sh\": executable file not found\n"),
    );
    assert!(matches!(
        exec_failed.owner(RECORDED),
        Owner::CouldNotTell(_)
    ));

    let no_init = Machine::new();
    no_init.containers(&[container(
        "ws",
        "running",
        &[(&no_init.clone(), MOUNTED_AT)],
    )]);
    no_init.inside("ws", &[(77, 500, &[77])]);
    assert!(matches!(no_init.owner(RECORDED), Owner::CouldNotTell(_)));
}

#[test]
fn the_container_is_read_as_root() {
    let machine = Machine::new();
    machine.containers(&[container(
        "ws",
        "running",
        &[(&machine.clone(), MOUNTED_AT)],
    )]);
    machine.inside("ws", &[(1, 10, &[1])]);
    machine.owner(RECORDED);

    let exec = machine
        .docker
        .args_to("docker")
        .into_iter()
        .find(|it| it[0] == "exec")
        .expect("one exec");
    assert_eq!(&exec[..4], ["exec", "--user", "0", "ws"]);
}

#[test]
fn a_lock_written_on_this_machine_is_asked_of_this_machines_procfs() {
    let machine = Machine::new();
    machine.containers(&[]);
    let recorded = machine.site().display().to_string();

    assert_eq!(machine.owner(&recorded), Owner::Gone);

    machine.host_process(8621, 329_153, &[8621]);
    let Owner::StillRunning(place) = machine.owner(&recorded) else {
        panic!("a live agent on this machine must stand");
    };
    assert!(place.contains("on this machine"), "{place}");
}

#[test]
fn a_lock_written_on_this_machine_with_no_docker_could_not_be_told() {
    // This machine's table says gone, but a container docker would have
    // listed may mount the same path, so no docker is no answer.
    let machine = Machine::new();
    machine.docker.script_missing("docker");
    let recorded = machine.site().display().to_string();

    assert!(matches!(machine.owner(&recorded), Owner::CouldNotTell(_)));
}

#[test]
fn a_machine_with_no_procfs_could_not_be_told() {
    let machine = Machine::new();
    machine.containers(&[]);
    std::fs::remove_dir_all(machine.proc_root()).expect("no procfs");
    let recorded = machine.site().display().to_string();

    assert!(matches!(machine.owner(&recorded), Owner::CouldNotTell(_)));
}

#[test]
fn a_relative_recorded_path_could_not_be_told() {
    let machine = Machine::new();
    machine.containers(&[]);
    assert!(matches!(
        machine.owner(".claude/worktrees/agent-a540dbaa96aa45899"),
        Owner::CouldNotTell(_)
    ));
}

// =======================================================================
// a mount source that is not here any more
// =======================================================================

#[test]
fn a_stopped_container_whose_clone_was_deleted_is_ignored() {
    // Seen on the reference host: an exited container from eleven days ago,
    // mounting a clone `dl` has since removed, at the same path every
    // kinisi_ros container mounts its own clone. It has no process to veto
    // with, so it does not block the answer.
    let machine = Machine::new();
    let deleted = machine.dir.path().join("repos/o/r/deleted-ws");
    machine.containers(&[
        container("old", "exited", &[(&deleted, MOUNTED_AT)]),
        container("ws", "running", &[(&machine.clone(), MOUNTED_AT)]),
    ]);
    machine.inside("ws", &[(1, 10, &[1])]);

    assert_eq!(machine.owner(RECORDED), Owner::Gone);
}

#[test]
fn a_stopped_container_is_still_the_answer_when_it_sees_the_site() {
    let machine = Machine::new();
    let deleted = machine.dir.path().join("repos/o/r/deleted-ws");
    machine.containers(&[
        container("old", "exited", &[(&deleted, MOUNTED_AT)]),
        container("ws", "exited", &[(&machine.clone(), MOUNTED_AT)]),
    ]);
    assert_eq!(machine.owner(RECORDED), Owner::Gone);

    let alone = Machine::new();
    let deleted = alone.dir.path().join("repos/o/r/deleted-ws");
    alone.containers(&[container("old", "exited", &[(&deleted, MOUNTED_AT)])]);
    assert!(matches!(alone.owner(RECORDED), Owner::CouldNotTell(_)));
}

#[test]
fn a_running_container_whose_source_is_gone_is_asked_by_inode() {
    // A bind mount follows the directory, not its name: a clone renamed while
    // its container runs is still the site in there.
    let machine = Machine::new();
    let renamed_from = machine.dir.path().join("repos/o/r/old-name");
    machine.containers(&[container("ws", "running", &[(&renamed_from, MOUNTED_AT)])]);
    machine.inside_the_path_is("ws", &machine.site_identity());
    machine.inside("ws", &[(1, 10, &[1]), (8621, 329_153, &[8621])]);

    assert!(matches!(machine.owner(RECORDED), Owner::StillRunning(_)));
}

#[test]
fn a_running_container_that_holds_something_else_there_is_not_asked() {
    for answer in ["devlaunch-absent", "1:2"] {
        let machine = Machine::new();
        let renamed_from = machine.dir.path().join("repos/o/r/old-name");
        machine.containers(&[
            container("other", "running", &[(&renamed_from, MOUNTED_AT)]),
            container("ws", "running", &[(&machine.clone(), MOUNTED_AT)]),
        ]);
        machine.inside_the_path_is("other", answer);
        machine.inside("other", &[(1, 10, &[1]), (8621, 329_153, &[8621])]);
        machine.inside("ws", &[(1, 10, &[1])]);

        assert_eq!(machine.owner(RECORDED), Owner::Gone, "{answer}");
    }
}

/// A running container `ws` that sees the site and says gone, beside a
/// container `other` that the test makes the only doubt.
fn beside_a_clean_container(machine: &Machine, other: serde_json::Value) {
    machine.containers(&[
        other,
        container("ws", "running", &[(&machine.clone(), MOUNTED_AT)]),
    ]);
    machine.inside("ws", &[(1, 10, &[1])]);
}

#[test]
fn a_container_that_cannot_be_asked_could_not_be_told() {
    // `ws` sees the site and says gone, so `other` is the only doubt: a
    // container that could not be asked is not one that does not see it.
    let machine = Machine::new();
    let renamed_from = machine.dir.path().join("repos/o/r/old-name");
    beside_a_clean_container(
        &machine,
        container("other", "running", &[(&renamed_from, MOUNTED_AT)]),
    );
    machine.docker.script(
        ["docker", "exec", "--user", "0", "other"],
        Response::failed(126, "exec: \"sh\": executable file not found\n"),
    );
    assert!(matches!(machine.owner(RECORDED), Owner::CouldNotTell(_)));

    let odd = Machine::new();
    let renamed_from = odd.dir.path().join("repos/o/r/old-name");
    beside_a_clean_container(
        &odd,
        container("other", "running", &[(&renamed_from, MOUNTED_AT)]),
    );
    odd.inside_the_path_is("other", "stat: unrecognized option");
    assert!(matches!(odd.owner(RECORDED), Owner::CouldNotTell(_)));

    let paused = Machine::new();
    let renamed_from = paused.dir.path().join("repos/o/r/old-name");
    beside_a_clean_container(
        &paused,
        container("other", "paused", &[(&renamed_from, MOUNTED_AT)]),
    );
    assert!(matches!(paused.owner(RECORDED), Owner::CouldNotTell(_)));
}

// =======================================================================
// this machine's whole process table
// =======================================================================

#[test]
fn a_kernel_thread_is_what_marks_the_whole_machine_and_a_parent_of_zero_is_not() {
    // Inside a container, every process `docker exec` started reads as parent
    // 0, so a parent of 0 says nothing about which namespace this is.
    assert_eq!(
        proc_stat::parse(&stat_with_flags(2, KTHREADD_FLAGS, 0)).map(|it| it.kernel_thread),
        Some(true)
    );
    assert_eq!(
        proc_stat::parse(&stat_with_flags(90_718, 4_194_560, 0)).map(|it| it.kernel_thread),
        Some(false)
    );
}

#[test]
fn a_lock_on_this_machine_needs_the_whole_machines_table() {
    // `dl` run inside a pid namespace of its own (a container or sandbox
    // started without `--pid=host`) sees a procfs with a pid 1 and none of the
    // processes outside it. A host-side
    // agent would read as gone there.
    let machine = Machine::new();
    machine.only_a_namespace();
    machine.containers(&[]);
    let recorded = machine.site().display().to_string();

    let Owner::CouldNotTell(why) = machine.owner(&recorded) else {
        panic!("a namespace's own table is not this machine's");
    };
    assert!(why.contains("whole machine"), "{why}");
}

#[test]
fn an_agent_the_mounts_missed_is_found_in_the_whole_machines_table() {
    // The container the mounts point at is stopped, but the agent runs in a
    // namespace nothing mapped: a container of another docker, a renamed
    // clone's old path. Seen from the initial namespace, every process on the
    // machine is listed with its innermost pid.
    let machine = Machine::new();
    machine.host_process(53_000, 329_153, &[53_000, 8621]);
    machine.containers(&[container("ws", "exited", &[(&machine.clone(), MOUNTED_AT)])]);

    let Owner::StillRunning(place) = machine.owner(RECORDED) else {
        panic!("a live agent anywhere on this machine must stand");
    };
    assert!(place.contains("53000"), "{place}");
}

#[test]
fn the_whole_machines_table_saying_gone_still_needs_a_namespace_that_sees_the_site() {
    let machine = Machine::new();
    machine.containers(&[]);

    assert!(matches!(machine.owner(RECORDED), Owner::CouldNotTell(_)));
}

#[test]
fn a_whole_machines_table_that_will_not_read_could_not_be_told() {
    // Review finding: an unreadable entry used to drop the whole-machine veto
    // silently, and a mapped container saying gone was then enough.
    let machine = Machine::new();
    machine.host_process(53_000, 329_153, &[53_000, 8621]);
    std::fs::create_dir_all(machine.proc_root().join("777/stat"))
        .expect("an entry that will not read");
    machine.containers(&[container(
        "ws",
        "running",
        &[(&machine.clone(), MOUNTED_AT)],
    )]);
    machine.inside("ws", &[(1, 10, &[1])]);

    assert!(matches!(machine.owner(RECORDED), Owner::CouldNotTell(_)));
}

#[test]
fn a_process_at_the_same_start_whose_nspid_will_not_read_could_not_be_told() {
    // The site's own container is exited, so it says gone. Only the whole
    // machine's table can veto, and the one process started at the lock's
    // moment has no `NSpid` to say which pid it is in its own namespace.
    let machine = Machine::new();
    machine.host_process(53_000, 329_153, &[53_000]);
    std::fs::write(machine.proc_root().join("53000/status"), "Name:\tx\n")
        .expect("a status with no NSpid");
    machine.containers(&[container("ws", "exited", &[(&machine.clone(), MOUNTED_AT)])]);

    let Owner::CouldNotTell(why) = machine.owner(RECORDED) else {
        panic!("a process that may be the agent proves nothing gone");
    };
    assert!(why.contains("started at the same moment"), "{why}");
}

#[test]
fn without_the_whole_machines_table_nothing_is_proved_gone() {
    // Review finding: `dl` in a pid namespace of its own cannot see a host
    // agent, nor one in a container its docker does not list, so the
    // namespaces it can see saying gone prove nothing.
    let machine = Machine::new();
    machine.only_a_namespace();
    machine.containers(&[container(
        "ws",
        "running",
        &[(&machine.clone(), MOUNTED_AT)],
    )]);
    machine.inside("ws", &[(1, 10, &[1])]);

    let Owner::CouldNotTell(why) = machine.owner(RECORDED) else {
        panic!("a partial view proves nothing gone");
    };
    assert!(why.contains("whole machine"), "{why}");

    let stopped = Machine::new();
    stopped.only_a_namespace();
    stopped.containers(&[container("ws", "exited", &[(&stopped.clone(), MOUNTED_AT)])]);
    assert!(matches!(stopped.owner(RECORDED), Owner::CouldNotTell(_)));
}

#[test]
fn a_process_that_ends_while_the_table_is_read_is_gone_not_unreadable() {
    assert!(vanished(&std::io::Error::from_raw_os_error(libc::ESRCH)));
    assert!(vanished(&std::io::Error::from(ErrorKind::NotFound)));
    assert!(!vanished(&std::io::Error::from_raw_os_error(libc::EACCES)));
}

#[test]
fn a_listing_of_the_procfs_that_fails_partway_could_not_be_told() {
    // Review finding: a readdir error after pid 1 and a kernel thread were
    // listed left a table that passed for the whole machine, and the agent
    // listed after the error read as gone.
    let machine = Machine::new();
    machine.host_process(8621, 329_153, &[8621]);
    let root = machine.proc_root();
    let listing = vec![
        Ok(root.join("1")),
        Ok(root.join("2")),
        Err(std::io::Error::from_raw_os_error(libc::EIO)),
        Ok(root.join("8621")),
    ];

    assert!(table_of(&root, listing).is_err());
}

const NFS: u32 = 0x6969;

#[test]
fn a_lock_on_an_nfs_site_could_not_be_told_gone() {
    // An NFS home: the agent may be running on another client, in no
    // process table here.
    let machine = Machine::new();
    machine.containers(&[]);
    let recorded = machine.site().display().to_string();

    let owner = Owners::reading(&machine.docker, machine.proc_root())
        .on(|_| Ok(NFS))
        .owner(&machine.site(), Path::new(&recorded), &incident());

    let Owner::CouldNotTell(why) = owner else {
        panic!("a lock on NFS must not read as gone: {owner:?}");
    };
    assert!(why.contains("NFS"), "{why}");
}

#[test]
fn a_lock_on_a_local_filesystem_can_still_be_gone() {
    let locals: [(&str, Filesystem); 5] = [
        ("ext4", |_| Ok(0xEF53)),
        ("btrfs", |_| Ok(0x9123_683E)),
        ("xfs", |_| Ok(0x5846_5342)),
        ("tmpfs", |_| Ok(0x0102_1994)),
        ("overlay", |_| Ok(0x794C_7630)),
    ];
    for (name, filesystem) in locals {
        let machine = Machine::new();
        machine.containers(&[]);
        let recorded = machine.site().display().to_string();

        let owner = Owners::reading(&machine.docker, machine.proc_root())
            .on(filesystem)
            .owner(&machine.site(), Path::new(&recorded), &incident());

        assert_eq!(owner, Owner::Gone, "{name}");
    }
}

#[test]
fn a_site_whose_filesystem_will_not_read_could_not_be_told() {
    let machine = Machine::new();
    machine.containers(&[]);
    let recorded = machine.site().display().to_string();

    let owner = Owners::reading(&machine.docker, machine.proc_root())
        .on(|_| Err("statfs refused".to_owned()))
        .owner(&machine.site(), Path::new(&recorded), &incident());

    assert!(matches!(owner, Owner::CouldNotTell(_)), "{owner:?}");
}
