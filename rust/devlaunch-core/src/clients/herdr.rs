//! What a session manager needs in order to see an agent dl started.
//!
//! A manager like [herdr](https://herdr.dev) classifies a pane in two steps: it
//! decides *which* agent the pane holds, and only then matches that agent's rules
//! against the pane's screen. Under dl the first step fails and takes the second
//! with it — the pane's foreground processes are `dl`, `ssh` and two `devpod`s,
//! with no agent among them — while the second step was never broken, because
//! `dl <ws> -- <agent>` pipes the agent's own TUI through the pane.
//!
//! So the whole of what dl owes a manager is the agent's *name*, and the place a
//! manager can read it is the environment of a host process it can see. That is
//! the ssh child dl spawns, not dl itself: a `setenv` after start does not rewrite
//! `/proc/<pid>/environ`, which the kernel fixes at exec, so dl's own row carries
//! nothing however early it is written. herdr walks the pane's whole foreground
//! process list, which is what makes the child enough — measured against herdr
//! 0.8.2 on a pane whose parent's environ was deliberately empty of it.
//!
//! `aid` already does this for the session it opens (`aid/src/main.rs`), because
//! aid *picked* the agent. dl is the other half: it did not pick the agent, but
//! when the command it was handed **is** an agent by name, saying so is a reading
//! of the command rather than a guess about it.

use super::gh::Forwarding;
use crate::runner::EnvSpec;

/// The variable a session manager reads to learn which agent a wrapper stands for.
///
/// herdr's name for it, and the one manager-specific word in core. It is written
/// for the ssh child and never forwarded into the container: herdr's own docs say
/// the hint "applies only to that foreground process" and that it "cannot see it
/// if you set it only inside a VM or container", so a name sent through the
/// transport would be a name nothing reads.
pub(crate) const AGENT_VAR: &str = "HERDR_AGENT";

/// Every agent devlaunch knows by name, in the spelling a session manager uses.
///
/// `aid`'s own table (`aid/src/rewrite.rs`) knows the same agents and much more
/// about each of them — the command, the prompt flags, the environment — so the
/// names exist twice by necessity: this const is `pub(crate)` and aid is a
/// different binary. `test/unit/test_session_manager.py` diffs the two lists, per
/// the standing rule in CLAUDE.md that a second hand-maintained copy of a fact
/// needs a test beside it doing exactly that.
///
/// A name here is a claim that a manager has detection rules under that label. It
/// is the reason this is a list and not "whatever program the command names":
/// `dl <ws> -- make test` naming an agent called `make` would tell a manager the
/// pane holds an agent it has never heard of, which is a worse answer than the
/// silence dl gives today.
pub(crate) const AGENT_NAMES: &[&str] = &["claude", "codex", "gemini"];

/// The agent a workspace command starts, when its program is one by name.
///
/// Reads the command the way a shell would begin to: leading `NAME=value`
/// assignments are the shell's, not the program's, so they are stepped over, and
/// the program itself is compared by its last path component so that
/// `/usr/local/bin/claude` and `claude` answer alike.
///
/// Everything past the program is ignored, prompt and flags included, because
/// nothing after the program can change which agent is about to run.
///
/// Deliberately not a parse. A command holding a pipe, a `&&` or a subshell gets
/// the answer for its *first* program, which is the one whose screen the pane will
/// hold at the moment it starts; a command that starts an agent halfway through a
/// chain is not named, and silence is the same answer dl gave before this existed.
pub(crate) fn agent_in(command: &str) -> Option<&'static str> {
    let program = command
        .split_whitespace()
        .find(|word| !is_assignment(word))?;
    let name = program.rsplit('/').next()?;
    AGENT_NAMES.iter().copied().find(|known| *known == name)
}

/// Whether a word is a shell assignment prefix rather than the program.
///
/// `FOO=bar claude` runs claude. The name half must be non-empty for the same
/// reason the shell requires it: `=x` is a program named `=x`, however unlikely,
/// and not an assignment.
fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => !name.is_empty(),
        None => false,
    }
}

/// Add the agent's name to what the ssh child will be run with.
///
/// The name goes in the child's environment and **not** in the `SendEnv` permit
/// list, which is why this extends only the environment half of a [`Forwarding`]
/// and leaves `args` alone. A permit list entry would ask the container for the
/// value, where nothing reads it, and would change the multiplexed control
/// socket's identity for a variable that never crosses the transport.
pub(crate) fn extend_openssh_forwarding(base: Forwarding, agent: Option<&str>) -> Forwarding {
    let Some(agent) = agent else {
        return base;
    };
    let Forwarding { args, env } = base;
    Forwarding {
        args,
        env: inherited(env).and(AGENT_VAR, agent),
    }
}

/// The same name, for a child whose transport carries no permit list.
///
/// The devpod route has no `SendEnv`: `devpod ssh` takes `--set-env`, which puts a
/// value inside the *container*, and that is the wrong place for this one. What
/// reads it is the manager on this host, which walks dl's descendants and reads
/// their `/proc/<pid>/environ`; the descendant it finds on this route is the
/// `devpod` process itself. So the name goes in that child's environment and
/// nowhere else, which is what the OpenSSH route does with it too
/// ([`extend_openssh_forwarding`], env and never the permit list).
pub(crate) fn inherited_with(env: EnvSpec, agent: Option<&str>) -> EnvSpec {
    match agent {
        None => env,
        Some(agent) => inherited(env).and(AGENT_VAR, agent),
    }
}

/// A base environment that is a parent environment.
///
/// `Forwarding::default()` carries `EnvSpec::default()`, which is already the
/// inherited one; this says so out loud for [`super::claude::extend_openssh_forwarding`]'s
/// reason — a change to `EnvSpec`'s default must not silently hand a session an
/// empty environment.
fn inherited(env: EnvSpec) -> EnvSpec {
    if env == EnvSpec::default() {
        EnvSpec::inherited()
    } else {
        env
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_command_names_its_agent() {
        assert_eq!(agent_in("claude"), Some("claude"));
        assert_eq!(agent_in("codex"), Some("codex"));
        assert_eq!(agent_in("gemini"), Some("gemini"));
    }

    /// The prompt is the common shape and must not hide the program.
    #[test]
    fn arguments_after_the_program_are_not_read() {
        assert_eq!(agent_in("claude 'fix the bug'"), Some("claude"));
        assert_eq!(
            agent_in("claude --dangerously-skip-permissions"),
            Some("claude")
        );
    }

    /// What `aid` composes reaches dl as a path in some layouts and a bare name in
    /// others, and the two are one agent.
    #[test]
    fn a_path_names_the_agent_its_last_component_does() {
        assert_eq!(agent_in("/usr/local/bin/claude"), Some("claude"));
        assert_eq!(agent_in("~/.local/bin/codex"), Some("codex"));
    }

    /// The shape `aid` uses for the two variables its table sets.
    #[test]
    fn assignments_are_stepped_over() {
        assert_eq!(agent_in("IS_SANDBOX=1 claude"), Some("claude"));
        assert_eq!(
            agent_in("CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude"),
            Some("claude")
        );
    }

    /// A program named `=x` is a program. Only a non-empty name is an assignment.
    #[test]
    fn a_leading_equals_is_not_an_assignment() {
        assert_eq!(agent_in("=x claude"), None);
    }

    #[test]
    fn an_ordinary_command_names_nothing() {
        assert_eq!(agent_in("make test"), None);
        assert_eq!(agent_in("pixi run ci"), None);
        assert_eq!(agent_in(""), None);
        assert_eq!(agent_in("   "), None);
    }

    /// A name that merely contains an agent's is not that agent: a manager would
    /// be told to match claude's rules against something else's screen.
    #[test]
    fn a_longer_name_is_a_different_program() {
        assert_eq!(agent_in("claude-monitor"), None);
        assert_eq!(agent_in("myclaude"), None);
    }

    #[test]
    fn the_name_lands_in_the_environment_and_not_the_permit_list() {
        let forwarding = extend_openssh_forwarding(Forwarding::default(), Some("claude"));
        assert_eq!(
            forwarding.env,
            EnvSpec::inherited().and(AGENT_VAR, "claude"),
            "the ssh child is what a manager reads"
        );
        assert!(
            forwarding.args.is_empty(),
            "a name nothing in the container reads must not be asked of the container"
        );
    }

    /// A command that names no agent must leave the launch exactly as it was.
    #[test]
    fn no_agent_changes_nothing() {
        let base = Forwarding {
            args: vec!["GH_TOKEN".to_owned()],
            env: EnvSpec::inherited().and("GH_TOKEN", "gho_x"),
        };
        let extended = extend_openssh_forwarding(base.clone(), None);
        assert_eq!(extended.args, base.args);
        assert_eq!(extended.env, base.env);
    }

    // ------------------------------------------------- reporting from inside

    fn in_a_pane() -> HostEnv {
        HostEnv {
            enabled: Some("1".to_owned()),
            in_pane: Some("1".to_owned()),
            pane_id: Some("w1:p3".to_owned()),
            socket: Some("/run/user/1000/herdr/herdr.sock".to_owned()),
            binary: Some("/usr/bin/herdr".to_owned()),
        }
    }

    #[test]
    fn a_host_in_a_pane_that_consented_can_report() {
        let reporting = Reporting::resolve(&in_a_pane()).expect("all five present");
        assert_eq!(reporting.pane_id(), "w1:p3");
        assert_eq!(reporting.host_binary(), Path::new("/usr/bin/herdr"));
    }

    /// The consent is the only one of the five a person sets, and without it
    /// nothing else is even read.
    #[test]
    fn no_consent_means_no_reporting() {
        for value in [None, Some("0".to_owned()), Some("false".to_owned())] {
            let host = HostEnv {
                enabled: value.clone(),
                ..in_a_pane()
            };
            assert_eq!(Reporting::resolve(&host), None, "{value:?} consented");
        }
    }

    /// Consent on a machine with no manager is not an error and not a warning: it
    /// is a profile that says yes wherever the person happens to be.
    #[test]
    fn consent_outside_a_pane_reports_nothing() {
        let host = HostEnv {
            in_pane: None,
            ..in_a_pane()
        };
        assert_eq!(Reporting::resolve(&host), None);
    }

    /// Each coordinate is load-bearing, so each one missing has to answer the same
    /// way. An empty value counts as missing: herdr exports these, and a manager
    /// that exported an empty pane id has told us nothing.
    #[test]
    fn a_missing_or_empty_coordinate_reports_nothing() {
        let blanks = [None, Some(String::new()), Some("   ".to_owned())];
        for blank in blanks {
            for host in [
                HostEnv {
                    pane_id: blank.clone(),
                    ..in_a_pane()
                },
                HostEnv {
                    socket: blank.clone(),
                    ..in_a_pane()
                },
                HostEnv {
                    binary: blank.clone(),
                    ..in_a_pane()
                },
            ] {
                assert_eq!(Reporting::resolve(&host), None, "{host:?} resolved");
            }
        }
    }

    /// The colon is not cosmetic: `ssh -R` splits its argument on colons, so a
    /// pane id spelled into a listen path verbatim would make a spec ssh reads as
    /// three fields.
    #[test]
    fn the_listen_path_carries_no_colon() {
        let reporting = Reporting::resolve(&in_a_pane()).expect("resolvable");
        assert_eq!(
            reporting.container_socket(),
            "/tmp/devlaunch-herdr-w1-p3.sock"
        );
        assert!(!reporting.container_socket().contains(':'));
    }

    /// Two panes attached to one workspace must not report over one socket.
    #[test]
    fn each_pane_gets_a_socket_of_its_own() {
        let one = Reporting::resolve(&in_a_pane()).expect("resolvable");
        let two = Reporting::resolve(&HostEnv {
            pane_id: Some("w1:p4".to_owned()),
            ..in_a_pane()
        })
        .expect("resolvable");
        assert_ne!(one.container_socket(), two.container_socket());
    }

    #[test]
    fn the_forward_is_a_connection_of_its_own_that_runs_no_command() {
        let reporting = Reporting::resolve(&in_a_pane()).expect("resolvable");
        let argv = reporting.forward_argv(Path::new("/home/me/.ssh/config"), "myws");
        assert_eq!(
            argv,
            vec![
                "ssh",
                "-F",
                "/home/me/.ssh/config",
                "-N",
                "-o",
                "ControlPath=none",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "StreamLocalBindUnlink=yes",
                "-R",
                "/tmp/devlaunch-herdr-w1-p3.sock:/run/user/1000/herdr/herdr.sock",
                "myws.devpod",
            ]
        );
    }

    /// `ControlPath=none` is the whole reason this is a separate connection:
    /// devlaunch#549 measured that a forward asked for on a multiplexed session
    /// accumulates on the master and is inherited by a later trip that asked for
    /// nothing at all.
    #[test]
    fn the_forward_never_joins_a_shared_master() {
        let reporting = Reporting::resolve(&in_a_pane()).expect("resolvable");
        let argv = reporting.forward_argv(Path::new("/cfg"), "myws");
        assert!(argv.iter().any(|arg| arg == "ControlPath=none"));
        assert!(
            !argv.iter().any(|arg| arg.starts_with("ControlMaster")),
            "{argv:?}"
        );
    }

    /// A forward that cannot bind must take the connection down rather than leave
    /// a session that looks wired up and reports nothing: the warning OpenSSH
    /// would print otherwise is below the `LogLevel error` devpod's alias sets.
    #[test]
    fn a_forward_that_cannot_bind_is_not_silent() {
        let reporting = Reporting::resolve(&in_a_pane()).expect("resolvable");
        let argv = reporting.forward_argv(Path::new("/cfg"), "myws");
        assert!(argv.iter().any(|arg| arg == "ExitOnForwardFailure=yes"));
    }

    /// The rewrite is the point: herdr's names, dl's values, and the host's own
    /// socket path must not travel into the container where it resolves to nothing.
    #[test]
    fn the_coordinates_name_the_containers_paths() {
        let reporting = Reporting::resolve(&in_a_pane()).expect("resolvable");
        assert_eq!(
            reporting.coordinates(),
            vec![
                (IN_PANE_VAR.to_owned(), "1".to_owned()),
                (PANE_VAR.to_owned(), "w1:p3".to_owned()),
                (
                    SOCKET_VAR.to_owned(),
                    "/tmp/devlaunch-herdr-w1-p3.sock".to_owned()
                ),
                (BIN_VAR.to_owned(), CONTAINER_BINARY.to_owned()),
            ]
        );
        assert!(
            !reporting
                .coordinates()
                .iter()
                .any(|(_, value)| value.contains("/run/user")),
            "the host's own socket path reached the container"
        );
    }

    #[test]
    fn the_devpod_transport_sets_the_coordinates_rather_than_asking_for_them() {
        let reporting = Reporting::resolve(&in_a_pane()).expect("resolvable");
        let flags = reporting.devpod_flags();
        assert_eq!(flags.len(), 8, "four pairs: {flags:?}");
        assert!(flags.iter().all(|flag| flag != "--send-env"));
        assert!(flags.contains(&"--set-env".to_owned()));
        assert!(flags.contains(&format!("{IN_PANE_VAR}=1")));
        assert!(flags.contains(&format!("{SOCKET_VAR}=/tmp/devlaunch-herdr-w1-p3.sock")));
    }

    /// Unlike the agent's name, these cross the transport, so they are permitted
    /// as well as supplied -- which also puts them in the control socket's
    /// identity, where they belong.
    #[test]
    fn the_openssh_transport_permits_and_supplies_the_coordinates() {
        let reporting = Reporting::resolve(&in_a_pane()).expect("resolvable");
        let forwarding = reporting.extend_openssh_forwarding(Forwarding {
            args: vec!["GH_TOKEN".to_owned()],
            env: EnvSpec::inherited().and("GH_TOKEN", "gho_x"),
        });
        assert_eq!(
            forwarding.args,
            vec!["GH_TOKEN", IN_PANE_VAR, PANE_VAR, SOCKET_VAR, BIN_VAR],
            "the credential's permit list lost or gained an entry"
        );
        assert_eq!(
            forwarding.env.entries.get(SOCKET_VAR).map(String::as_str),
            Some("/tmp/devlaunch-herdr-w1-p3.sock")
        );
        assert_eq!(
            forwarding.env.entries.get("GH_TOKEN").map(String::as_str),
            Some("gho_x")
        );
    }

    /// The probe answers by exit status, and it has to check all three things: a
    /// container with the binary but no hook reports nothing, and would be marked
    /// as prepared by a probe that only looked for the binary.
    #[test]
    fn the_probe_checks_the_binary_the_hook_and_the_settings() {
        let command = probe_command(17_740_520, "/tmp/devlaunch-herdr-w1-p3.sock");
        assert!(command.contains(CONTAINER_BINARY));
        assert!(command.contains(CONTAINER_HOOK));
        assert!(command.contains(CONTAINER_SETTINGS));
        assert!(
            command.contains("17740520"),
            "the probe does not compare the size: {command}"
        );
    }

    /// The fourth thing, which no other check can stand in for.
    ///
    /// The forward is detached: its stderr is `/dev/null` and nothing waits for
    /// its exit, so a container whose user cannot bind the listen path -- a
    /// root-owned `/tmp`, a stale root-owned socket -- reports a pid and then
    /// nothing. The probe is where that becomes visible, and it answers with a
    /// status of its own because no amount of lending fixes it.
    #[test]
    fn the_probe_asks_whether_the_socket_arrived() {
        let command = probe_command(17_740_520, "/tmp/devlaunch-herdr-w1-p3.sock");
        assert!(
            command.starts_with("test -S /tmp/devlaunch-herdr-w1-p3.sock"),
            "the socket is not asked about first: {command}"
        );
        assert!(
            command.contains(&format!("exit {PROBE_NO_SOCKET}")),
            "a missing socket answers like any other failure: {command}"
        );
    }

    /// Somebody else's managed settings are policy, and this feature is a status
    /// indicator. The install refuses before it writes.
    ///
    /// `/etc/claude-code/managed-settings.json` is Claude Code's highest-precedence
    /// configuration. An image can ship `permissions.deny` there and mean it, and
    /// dl opens other people's repositories, so a `tee` straight onto it takes away
    /// rules dl knows nothing about -- with no merge, no backup and no notice.
    #[test]
    fn the_install_refuses_to_overwrite_settings_dl_did_not_write() {
        let command = install_command();
        assert!(
            command.contains(&format!("exit {INSTALL_FOREIGN_SETTINGS}")),
            "the install overwrites a foreign settings file: {command}"
        );
        let guard = command
            .split("; ")
            .find(|clause| clause.contains(&format!("exit {INSTALL_FOREIGN_SETTINGS}")))
            .expect("the guard is one clause");
        assert!(
            command.find(guard) < command.find("tee /etc/claude-code"),
            "the guard runs after the write it is guarding: {command}"
        );
    }

    /// A settings file is matched by what is in it, not by its being there.
    ///
    /// `test -f` read somebody else's policy file as a prepared workspace, so the
    /// hook was never installed and the container reported nothing, forever.
    #[test]
    fn the_probe_reads_the_settings_rather_than_counting_them() {
        let command = probe_command(17_740_520, "/tmp/devlaunch-herdr-w1-p3.sock");
        assert!(
            command.contains(&format!("grep -qF {CONTAINER_HOOK} {CONTAINER_SETTINGS}")),
            "the probe cannot tell dl's settings from anyone else's: {command}"
        );
    }

    /// A lend onto the path a concurrent session is running earns ETXTBSY, so the
    /// bytes land elsewhere and are renamed into place.
    #[test]
    fn the_lend_never_writes_over_a_running_binary() {
        let command = lend_command();
        assert!(command.contains("mv "), "{command}");
        assert!(command.contains("sudo -n"), "a password prompt would hang");
    }

    /// Five events, and each one has to name the state it stands for. A settings
    /// file that fires the hook with no argument reports nothing at all: the
    /// hook's `case` has no default arm by design.
    #[test]
    fn the_settings_map_every_event_to_a_state() {
        let settings = managed_settings();
        for (event, state) in [
            ("SessionStart", "idle"),
            ("UserPromptSubmit", "working"),
            ("Notification", "blocked"),
            ("Stop", "idle"),
            ("SessionEnd", "release"),
        ] {
            assert!(settings.contains(&format!("\"{event}\"")), "{settings}");
            assert!(
                settings.contains(&format!("sh {CONTAINER_HOOK} {state}")),
                "{event} does not report {state}: {settings}"
            );
        }
    }

    /// It is JSON that Claude Code has to parse, and it is built by hand.
    #[test]
    fn the_settings_are_json() {
        let parsed: serde_json::Value =
            serde_json::from_str(&managed_settings()).expect("the settings are JSON");
        let events = parsed
            .get("hooks")
            .and_then(serde_json::Value::as_object)
            .expect("the settings carry a hooks object");
        assert_eq!(events.len(), 5, "{events:?}");
    }

    /// The container has no python3, no jq, no socat and no nc -- which is why
    /// herdr's own claude hook cannot fire in one. This hook must not acquire a
    /// dependency on any of them.
    #[test]
    fn the_hook_needs_nothing_but_a_shell() {
        // Comment lines are skipped, and one of them names python3 on purpose:
        // the comment explaining why the hook cannot use it must not be the thing
        // that fails this test.
        let code: String = HOOK
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        for absent in ["python3", "python", "jq", "socat", "nc ", "curl"] {
            assert!(
                !code.contains(absent),
                "the hook depends on {absent}, which a general container has not got"
            );
        }
    }

    /// Every guard exits 0. A hook that fails is a hook that costs the agent's
    /// session, and "no manager listening" is the ordinary case rather than an
    /// error.
    #[test]
    fn the_hook_is_silent_when_there_is_nothing_to_report_to() {
        for guard in [
            "[ \"${HERDR_ENV:-}\" = \"1\" ] || exit 0",
            "[ -n \"${HERDR_PANE_ID:-}\" ] || exit 0",
            "[ -n \"${HERDR_SOCKET_PATH:-}\" ] || exit 0",
            "[ -S \"${HERDR_SOCKET_PATH}\" ] || exit 0",
        ] {
            assert!(HOOK.contains(guard), "the hook lost its guard: {guard}");
        }
    }

    /// Lifecycle, not session identity. Measured against herdr 0.8.2: a
    /// `report-agent-session` reports a session id for a pane that already has an
    /// agent and does not establish one, so a container sending only those stays
    /// invisible.
    #[test]
    fn the_hook_reports_lifecycle_state() {
        assert!(HOOK.contains("pane report-agent "));
        assert!(HOOK.contains("--state"));
        assert!(HOOK.contains("pane release-agent"));
        assert!(
            !HOOK.contains("report-agent-session"),
            "session identity alone does not make a pane visible"
        );
    }

    /// The report's own failure must not become the hook's, which needs the hook
    /// to be *run* rather than read.
    ///
    /// The `-S` guard above passes on a socket file whose other end has gone, and
    /// that file outlives the forward: devpod's server creates the listen path and
    /// does not remove it (devlaunch#549 measured exactly this). So a hook firing
    /// after a forward has died connects to a corpse, herdr exits non-zero, and an
    /// `exec` hands that status to Claude Code as the hook's own.
    #[test]
    fn a_report_that_cannot_be_delivered_still_leaves_the_hook_silent() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let hook = dir.path().join("herdr-hook.sh");
        std::fs::write(&hook, HOOK).expect("the hook");
        // A socket file with nobody behind it: what the container is left holding
        // when the forward goes down.
        let socket = dir.path().join("herdr.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("a socket");
        // A manager binary that refuses, as one that cannot reach its socket does.
        let binary = dir.path().join("herdr");
        std::fs::write(&binary, "#!/bin/sh\nexit 3\n").expect("the stub");
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .expect("an executable stub");

        for state in ["idle", "working", "blocked", "release"] {
            let status = std::process::Command::new("sh")
                .arg(&hook)
                .arg(state)
                .env("HERDR_ENV", "1")
                .env("HERDR_PANE_ID", "w1:p3")
                .env("HERDR_SOCKET_PATH", &socket)
                .env("HERDR_BIN_PATH", &binary)
                .stdin(std::process::Stdio::null())
                .status()
                .expect("the hook runs under sh");
            assert_eq!(
                status.code(),
                Some(0),
                "reporting {state} to a dead socket cost the agent's turn"
            );
        }
    }

    /// The credentials already on the line survive: this extends a forwarding, it
    /// does not replace one.
    #[test]
    fn the_forwarded_credentials_survive() {
        let base = Forwarding {
            args: vec!["GH_TOKEN".to_owned()],
            env: EnvSpec::inherited().and("GH_TOKEN", "gho_x"),
        };
        let extended = extend_openssh_forwarding(base, Some("claude"));
        assert_eq!(extended.args, vec!["GH_TOKEN".to_owned()]);
        assert_eq!(
            extended.env,
            EnvSpec::inherited()
                .and("GH_TOKEN", "gho_x")
                .and(AGENT_VAR, "claude")
        );
    }
}

// ===========================================================================
// reporting from inside the container
// ===========================================================================
//
// Everything above is about a pane whose screen holds the agent, which is what
// `dl <ws> -- <agent>` and `aid` give a manager. This half is about the other
// case, and it is the one a host-side variable cannot reach: `dl <ws>` opens a
// shell, somebody types `claude` at it, and the agent is a process inside the
// container. herdr's own documentation is explicit that the hint "cannot see it
// if you set it only inside a VM or container", and it is right -- the manager
// walks host processes, and there is no host process to walk.
//
// So the container reports for itself, over herdr's documented socket protocol.
// Four things have to be true inside, and dl can arrange all four (measured on
// hardware against herdr 0.8.2, devlaunch#549):
//
// 1. **A socket that connects.** `ssh -R <container path>:<host path>` forwards
//    the manager's unix socket per connection, so it works on attach against a
//    container that has been running for a week -- unlike a bind mount, which
//    lands only at container creation. devpod's own `-R` was tried first and
//    hangs with no output for a unix socket, so the forward is a separate
//    OpenSSH child ([`Reporting::forward_argv`]) rather than a flag on whichever
//    transport carries the terminal. That also makes it transport-agnostic: a
//    bare `dl <ws>` attaches through devpod, and the forward does not care.
// 2. **A client that speaks the protocol.** There is no python3, no socat and no
//    nc in a general container, so the herdr binary itself is lent in over the
//    same channel -- the payload class `gh` and `claude` already travel as, at
//    1.4s for 17MB over the devpod stdio tunnel.
// 3. **The coordinates**, as environment: `HERDR_ENV`, `HERDR_PANE_ID`, and the
//    socket and binary paths rewritten to the container's.
// 4. **Something that fires.** Claude Code's hooks, installed at
//    `/etc/claude-code/managed-settings.json` -- container-local, and that is the
//    whole reason for the choice. This repo's devcontainer bind-mounts the host's
//    `~/.claude` into the container, so a hook written to `~/.claude/settings.json`
//    from inside would be an edit to the user's own machine-wide config.
//
// The state is authoritative rather than screen-scraped, which is a real gain
// over the host-side half: `pane report-agent` is what establishes both identity
// and state. `pane report-agent-session` was tried and does not -- it reports a
// session id for a pane that already has an agent, so a container using it alone
// stays invisible.

use std::path::{Path, PathBuf};

/// The consent that turns container-side reporting on.
///
/// A consent and not a denial, for [`crate::flows::provision::ZELLIJ_VAR`]'s
/// reason: it costs a lend, an install trip and a second ssh connection, so the
/// default has to be that no launch pays for it. Read through the shared
/// [`crate::flows::provision::provisioning_disabled`] parse, so a value spelled
/// the same way cannot answer differently here than in the other switches.
pub(crate) const ENABLE_VAR: &str = "DEVLAUNCH_HERDR";

/// herdr's own four, which it exports into every pane it spawns.
///
/// Read rather than set, and read from *this* process's environment: they are how
/// a launch learns it is running in a manager's pane at all, and which pane.
pub(crate) const IN_PANE_VAR: &str = "HERDR_ENV";
pub(crate) const PANE_VAR: &str = "HERDR_PANE_ID";
pub(crate) const SOCKET_VAR: &str = "HERDR_SOCKET_PATH";
pub(crate) const BIN_VAR: &str = "HERDR_BIN_PATH";

/// Where the lent binary lands, and where the hook looks for it.
///
/// `/usr/local/bin` rather than `~/.local/bin`, where the `gh` and `claude` lends
/// go: the install already needs root for the managed settings, this puts the
/// binary on every user's PATH inside the container rather than one home's, and a
/// container whose home is a volume does not carry a stale copy into the next
/// image.
pub(crate) const CONTAINER_BINARY: &str = "/usr/local/bin/herdr";

/// The hook script's path inside the container.
pub(crate) const CONTAINER_HOOK: &str = "/usr/local/share/devlaunch/herdr-hook.sh";

/// Claude Code's container-local settings file.
///
/// Read by every `claude` started in the container, including one typed at the
/// shell by hand, which is the case this whole half exists for.
pub(crate) const CONTAINER_SETTINGS: &str = "/etc/claude-code/managed-settings.json";

/// What this host's environment says about a session manager.
///
/// Values rather than reads, like [`super::gh::HostEnv`]: every decision below is
/// then a function of its inputs, and a test states the host it means.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HostEnv {
    /// [`ENABLE_VAR`]: the consent.
    pub(crate) enabled: Option<String>,
    /// [`IN_PANE_VAR`]: herdr saying this process is in one of its panes.
    pub(crate) in_pane: Option<String>,
    /// [`PANE_VAR`]: which pane, in herdr's own opaque spelling (`w1:p3`).
    pub(crate) pane_id: Option<String>,
    /// [`SOCKET_VAR`]: the manager's socket, on this host.
    pub(crate) socket: Option<String>,
    /// [`BIN_VAR`]: the manager's own binary, which is the thing to lend.
    pub(crate) binary: Option<String>,
}

impl HostEnv {
    pub(crate) fn from_process() -> Self {
        Self {
            enabled: crate::osext::env_str(ENABLE_VAR),
            in_pane: crate::osext::env_str(IN_PANE_VAR),
            pane_id: crate::osext::env_str(PANE_VAR),
            socket: crate::osext::env_str(SOCKET_VAR),
            binary: crate::osext::env_str(BIN_VAR),
        }
    }
}

/// A launch that can report an agent inside the container to a manager outside it.
///
/// Constructed only from a host that has all of it ([`Reporting::resolve`]), so
/// every field below is known to be present and non-empty and no caller has to
/// re-check. The absence of any one of them is not an error and not a warning: it
/// is the ordinary case of a launch that is not running in a manager's pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Reporting {
    pane_id: String,
    host_socket: PathBuf,
    host_binary: PathBuf,
}

impl Reporting {
    /// What `host` asks for, or nothing.
    ///
    /// Five conditions, and each one of them is a different way of not being in a
    /// manager's pane. The consent is checked first because it is the only one the
    /// user sets: a machine that never opts in never reads the rest.
    pub(crate) fn resolve(host: &HostEnv) -> Option<Self> {
        if !crate::flows::provision::provisioning_disabled(host.enabled.as_deref()) {
            return None;
        }
        if !crate::flows::provision::provisioning_disabled(host.in_pane.as_deref()) {
            return None;
        }
        let pane_id = non_empty(host.pane_id.as_deref())?;
        let host_socket = non_empty(host.socket.as_deref())?;
        let host_binary = non_empty(host.binary.as_deref())?;
        Some(Self {
            pane_id,
            host_socket: PathBuf::from(host_socket),
            host_binary: PathBuf::from(host_binary),
        })
    }

    /// The pane this launch reports against.
    pub(crate) fn pane_id(&self) -> &str {
        &self.pane_id
    }

    /// The host binary to lend.
    pub(crate) fn host_binary(&self) -> &Path {
        &self.host_binary
    }

    /// Where the forwarded socket lands inside the container.
    ///
    /// Named after the pane, so two panes attached to one workspace do not fight
    /// over a socket: each reports to its own manager connection, and a stale
    /// socket from a pane that has gone away is replaced rather than reused
    /// (`StreamLocalBindUnlink=yes`).
    ///
    /// `/tmp` and not the home directory, because the container user has to be
    /// able to create the listen path and a remote forward that cannot fails
    /// silently under the `LogLevel error` devpod's alias sets.
    pub(crate) fn container_socket(&self) -> String {
        format!("/tmp/devlaunch-herdr-{}.sock", sanitised(&self.pane_id))
    }

    /// The forward's own ssh invocation: no command, one remote forward.
    ///
    /// A connection of its own rather than a flag on the session, for two reasons
    /// that both come out of devlaunch#549's measurements. devpod's `-R` hangs for
    /// a unix socket, and a bare `dl <ws>` is a devpod attach, so a flag on the
    /// session would serve one route and not the other. And a forward asked for on
    /// a *multiplexed* OpenSSH session outlives the trip that asked for it, so a
    /// later launch that asked for nothing silently inherits it; `ControlPath=none`
    /// keeps this connection out of the master entirely, which is the same
    /// "unrepresentable rather than documented" move a keyed socket would be, at
    /// the cost of one handshake.
    ///
    /// `ExitOnForwardFailure=yes` is what makes this connection *end* when the
    /// bind fails, rather than sit there having achieved nothing: the warning
    /// itself is emitted below the alias's `LogLevel error` and nobody sees it,
    /// and a detached child's streams go nowhere anyway. What makes the failure
    /// visible is the probe, which asks the container whether the socket arrived
    /// ([`probe_command`], and `PROBE_NO_SOCKET` for the answer).
    pub(crate) fn forward_argv(&self, config: &Path, workspace_id: &str) -> Vec<String> {
        vec![
            super::ssh::PROGRAM.to_owned(),
            "-F".to_owned(),
            config.display().to_string(),
            "-N".to_owned(),
            "-o".to_owned(),
            "ControlPath=none".to_owned(),
            "-o".to_owned(),
            "ExitOnForwardFailure=yes".to_owned(),
            "-o".to_owned(),
            "StreamLocalBindUnlink=yes".to_owned(),
            "-R".to_owned(),
            format!("{}:{}", self.container_socket(), self.host_socket.display()),
            super::ssh::host_alias(workspace_id),
        ]
    }

    /// What the agent inside the container needs in its environment.
    ///
    /// The socket and the binary are the container's paths, not the host's, which
    /// is the whole of the rewrite: the names are herdr's and the values are dl's.
    pub(crate) fn coordinates(&self) -> Vec<(String, String)> {
        vec![
            (IN_PANE_VAR.to_owned(), "1".to_owned()),
            (PANE_VAR.to_owned(), self.pane_id.clone()),
            (SOCKET_VAR.to_owned(), self.container_socket()),
            (BIN_VAR.to_owned(), CONTAINER_BINARY.to_owned()),
        ]
    }

    /// The coordinates as `devpod ssh --set-env` flags.
    ///
    /// `--set-env` and not `--send-env`: the values are the container's paths,
    /// which this host's environment does not hold and must not be asked for.
    pub(crate) fn devpod_flags(&self) -> Vec<String> {
        self.coordinates()
            .into_iter()
            .flat_map(|(name, value)| ["--set-env".to_owned(), format!("{name}={value}")])
            .collect()
    }

    /// The coordinates on an OpenSSH session: names permitted, values supplied.
    ///
    /// Unlike [`extend_openssh_forwarding`]'s agent name, these do cross the
    /// transport, so they go in the permit list as well as the environment. That
    /// puts them in the identity of the multiplexed control socket, which is
    /// correct: a session carrying a manager's coordinates and one carrying none
    /// are different sessions.
    pub(crate) fn extend_openssh_forwarding(
        &self,
        base: super::gh::Forwarding,
    ) -> super::gh::Forwarding {
        let super::gh::Forwarding { mut args, mut env } = base;
        env = inherited(env);
        for (name, value) in self.coordinates() {
            args.push(name.clone());
            env = env.and(name, value);
        }
        super::gh::Forwarding { args, env }
    }
}

/// What one launch can tell a session manager.
///
/// The two halves of this feature travel together because they arrive at the same
/// place and are decided at the same moment: `agent` is a name for a pane whose
/// screen already holds the agent, `reporting` is a socket for an agent that is
/// hidden inside the container. A launch can have either, both or neither.
///
/// One parameter rather than two, because two `Option`s of unrelated meaning next
/// to each other in a signature are two values a caller can swap without the
/// compiler noticing.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Visibility<'a> {
    pub(crate) agent: Option<&'a str>,
    pub(crate) reporting: Option<&'a Reporting>,
}

/// A value that is present and not just whitespace.
fn non_empty(value: Option<&str>) -> Option<String> {
    let value = crate::osext::strip(value?);
    (!value.is_empty()).then(|| value.to_owned())
}

/// A pane id as a path component.
///
/// herdr's ids are opaque and carry a colon (`w1:p3`). A colon is legal in a unix
/// path, but it is not legal in an ssh `-R` spec, which splits on it -- so this is
/// a correctness requirement and not tidiness.
fn sanitised(pane_id: &str) -> String {
    pane_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The install's exit status when the container already holds managed settings
/// that dl did not write.
///
/// `/etc/claude-code/managed-settings.json` is Claude Code's highest-precedence
/// configuration: an image can ship `permissions.deny` there, or turn off bypass
/// mode, and mean it. Overwriting it with a hooks-only file of dl's own would take
/// that policy away silently, so this answer refuses instead, and the reporting is
/// what gets lost rather than the workspace's own rules. dl opens other people's
/// repositories, so the file is not hypothetical.
pub(crate) const INSTALL_FOREIGN_SETTINGS: i32 = 11;

/// The probe's exit status when the forwarded socket is not in the container.
///
/// A number of its own because it is the one probe answer that cannot be repaired
/// by lending anything: the binary and the hook are dl's to install, and the
/// socket is the forward's to deliver. Ten and not one, so it cannot be confused
/// with a `test` that simply said no.
pub(crate) const PROBE_NO_SOCKET: i32 = 10;

/// Whether the container has the socket, this binary and this hook.
///
/// Answers by exit status rather than by output, so the caller has nothing to
/// parse. The size comparison is asked of the container because the container is
/// the only thing that knows what it is holding.
///
/// The socket is asked about first and answers with [`PROBE_NO_SOCKET`], because
/// it fails for a different reason and is fixed a different way. It is also the
/// only check here that a *detached* forward cannot report on itself: its stderr
/// goes to `/dev/null` and nothing waits for its exit, so a container that cannot
/// bind the listen path would otherwise be announced as reporting.
///
/// The settings are matched by content and not by existence, because a container
/// can hold a managed settings file that is nothing to do with dl -- see
/// [`INSTALL_FOREIGN_SETTINGS`]. `test -f` read one of those as a prepared
/// workspace and reported nothing thereafter.
pub(crate) fn probe_command(host_binary_len: u64, container_socket: &str) -> String {
    format!(
        "test -S {container_socket} || exit {PROBE_NO_SOCKET}; \
         test -x {CONTAINER_BINARY} \
         && test \"$(stat -c %s {CONTAINER_BINARY})\" = {host_binary_len} \
         && test -x {CONTAINER_HOOK} \
         && grep -qF {CONTAINER_HOOK} {CONTAINER_SETTINGS}"
    )
}

/// The command that receives the lent binary on stdin.
///
/// Written to a temporary name and moved into place, because the destination may
/// be the binary a *concurrent* session is running: a `tee` straight onto a
/// running executable earns ETXTBSY, and a rename is atomic for anyone who opens
/// it afterwards.
///
/// `sudo -n` throughout: a container whose sudo wants a password must fail here
/// rather than sit on a prompt no one can see, since this trip has no terminal.
pub(crate) fn lend_command() -> &'static str {
    concat!(
        "set -e; ",
        "sudo -n mkdir -p /usr/local/bin; ",
        "sudo -n tee /usr/local/bin/.herdr.devlaunch >/dev/null; ",
        "sudo -n chmod 0755 /usr/local/bin/.herdr.devlaunch; ",
        "sudo -n mv /usr/local/bin/.herdr.devlaunch /usr/local/bin/herdr"
    )
}

/// The command that installs the hook and the settings that call it.
///
/// It refuses before it writes if the container already holds a managed settings
/// file without dl's hook in it: that file is somebody's policy, and this feature
/// is a status indicator. See [`INSTALL_FOREIGN_SETTINGS`]. A container with no
/// `grep` fails that guard closed, which is the right way round.
///
/// Both payloads travel inside the command rather than on stdin, because stdin is
/// spoken for on the lend trip and two trips that differ only in their payload are
/// harder to read than one heredoc each. The delimiters are quoted (`<<'EOF'`), so
/// the shell expands nothing in either payload -- the `$HERDR_*` in the hook are
/// read when the hook runs, inside the agent's own environment, and must survive
/// this trip untouched.
pub(crate) fn install_command() -> String {
    format!(
        "set -e; \
         if [ -f {CONTAINER_SETTINGS} ] && ! grep -qF {CONTAINER_HOOK} {CONTAINER_SETTINGS}; \
         then exit {INSTALL_FOREIGN_SETTINGS}; fi; \
         sudo -n mkdir -p /usr/local/share/devlaunch /etc/claude-code; \
         sudo -n tee {CONTAINER_HOOK} >/dev/null <<'DEVLAUNCH_HOOK_EOF'\n\
         {hook}\n\
         DEVLAUNCH_HOOK_EOF\n\
         sudo -n chmod 0755 {CONTAINER_HOOK}; \
         sudo -n tee {CONTAINER_SETTINGS} >/dev/null <<'DEVLAUNCH_SETTINGS_EOF'\n\
         {settings}\n\
         DEVLAUNCH_SETTINGS_EOF\n\
         true",
        hook = HOOK,
        settings = managed_settings(),
    )
}

/// Claude Code's hooks, as the settings file spells them.
///
/// Five events, and the mapping is the whole state machine: a turn starts
/// (`UserPromptSubmit`) and the agent is working; it ends (`Stop`) and the agent is
/// idle; a permission dialog or a wait for input (`Notification`) is blocked; a
/// session appears (`SessionStart`) idle and leaves (`SessionEnd`) reporting
/// nothing at all, which releases the pane rather than leaving it claiming an
/// agent that has gone.
///
/// A lifecycle report and not a session-identity one, which was measured: herdr's
/// `pane report-agent-session` reports a session id *for a pane that already has an
/// agent* and does not establish one, so a container that only ever sent those
/// stays invisible. `pane report-agent` establishes both.
///
/// Hand-built rather than serialised through a JSON library, for the reason the
/// rest of this module's strings are: what goes into the container is the subject
/// here, and a reader should be able to see it.
pub(crate) fn managed_settings() -> String {
    let event = |name: &str, state: &str| {
        format!(
            "    \"{name}\": [\n      {{\n        \"matcher\": \"*\",\n        \"hooks\": [\n          \
             {{ \"type\": \"command\", \"command\": \"sh {CONTAINER_HOOK} {state}\", \"timeout\": 5 }}\n        \
             ]\n      }}\n    ]"
        )
    };
    let events = [
        event("SessionStart", "idle"),
        event("UserPromptSubmit", "working"),
        event("Notification", "blocked"),
        event("Stop", "idle"),
        event("SessionEnd", "release"),
    ]
    .join(",\n");
    format!("{{\n  \"hooks\": {{\n{events}\n  }}\n}}\n")
}

/// The hook itself: POSIX sh, no interpreter beyond the shell.
///
/// It cannot be anything richer. There is no python3, no jq, no socat and no nc in
/// a general container -- which is also why herdr's *own* claude integration,
/// present in this repo's containers through a bind mount, can never fire there:
/// its first act is `command -v python3`.
///
/// Every path out is a silent `exit 0`, the guards and the report alike. A hook
/// that fails loudly costs the agent's session -- Claude Code reads a hook's exit
/// status, and treats 2 on `UserPromptSubmit` as a reason to discard the prompt --
/// and every condition here is an ordinary "no manager is listening".
pub(crate) const HOOK: &str = r#"#!/bin/sh
# Installed by devlaunch. Reports this agent's lifecycle to the session manager
# on the host, over the unix socket dl forwarded into this container.
[ "${HERDR_ENV:-}" = "1" ] || exit 0
[ -n "${HERDR_PANE_ID:-}" ] || exit 0
[ -n "${HERDR_SOCKET_PATH:-}" ] || exit 0
[ -S "${HERDR_SOCKET_PATH}" ] || exit 0
BIN="${HERDR_BIN_PATH:-/usr/local/bin/herdr}"
[ -x "$BIN" ] || exit 0

# Claude Code hands a hook its JSON on stdin. Nothing here reads it -- there is no
# python3 or jq to read it with -- but it is drained so the writer never blocks.
cat >/dev/null 2>&1

# Nanoseconds since the epoch: herdr orders reports by --seq, and two hooks can
# land inside one second.
seq=$(date +%s%N 2>/dev/null || echo 0)

# Run the reporter, never `exec` it: an `exec` makes the reporter's exit status
# this hook's, and Claude Code reads a hook's status. A socket file whose other
# end has gone -- which outlives the forward, since devpod's own server creates it
# and does not clean it up -- passes the `-S` guard above and then fails to
# connect, so this is reachable in the ordinary course of a day.
case "${1:-}" in
  release)
    "$BIN" pane release-agent "$HERDR_PANE_ID" --source devlaunch:claude >/dev/null 2>&1
    ;;
  idle|working|blocked)
    "$BIN" pane report-agent "$HERDR_PANE_ID" --source devlaunch:claude \
      --agent claude --state "$1" --seq "$seq" >/dev/null 2>&1
    ;;
esac
exit 0
"#;
