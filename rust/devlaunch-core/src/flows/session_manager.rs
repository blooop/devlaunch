//! Making an agent inside a workspace visible to a session manager outside it.
//!
//! [`crate::clients::herdr`] holds every decision this flow makes and every string
//! it sends; this is the round trips, in the order they have to happen:
//!
//! 1. [`prepare`] puts the manager's own binary and a Claude Code hook inside the
//!    container. At most twice per workspace per version of that binary, and
//!    normally never: a marker under dl's cache directory records what was
//!    installed, so the steady state costs no round trip at all.
//! 2. [`start_forward`] opens the connection that carries the manager's socket in,
//!    and hands back something the caller stops when the session ends.
//!
//! Both are no-ops unless the launch resolved a [`Reporting`], which needs the
//! consent variable *and* a manager's coordinates in this process's environment.
//! Nothing here runs on an ordinary launch.
//!
//! **Every failure is reported and then tolerated.** A workspace whose sudo is
//! locked down, a container with no `/etc`, a forward the container user cannot
//! bind: each of those costs the reporting and none of them costs the session. The
//! alternative would be refusing to open a shell because a status indicator could
//! not be wired up, which is the wrong way round.

use std::path::Path;

use crate::clients::herdr::{self, Reporting};
use crate::clients::kill::{self, Signal};
use crate::clients::ssh;
use crate::domain::workspace_state::NonEmpty;
use crate::runner::{DetachOutcome, Exit, Invocation, Outcome, Runner, SpawnSpec};

/// What [`prepare`] came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Prepared {
    /// The container has the binary and the hook, whether or not this call is what
    /// put them there.
    Ready,
    /// It does not, and this is what stopped it. The session still opens.
    Refused { reason: String },
}

/// Put the manager's binary and the hook that calls it inside the container.
///
/// Three trips at worst and none at best. The marker is keyed on the host binary's
/// size, which is the cheapest thing that changes when the manager is updated: a
/// `--version` comparison would mean running the host binary on every launch to
/// learn something that is almost always unchanged.
pub(crate) fn prepare(
    runner: &dyn Runner,
    cache_dir: &Path,
    config: &Path,
    workspace_id: &str,
    reporting: &Reporting,
) -> Prepared {
    let Ok(metadata) = std::fs::metadata(reporting.host_binary()) else {
        return Prepared::Refused {
            reason: format!(
                "{} names {}, which this host cannot read",
                herdr::BIN_VAR,
                reporting.host_binary().display()
            ),
        };
    };
    let stamp = herdr::stamp(metadata.len());
    let marker = herdr::marker_path(cache_dir, workspace_id);
    if std::fs::read_to_string(&marker).is_ok_and(|held| held.trim() == stamp) {
        return Prepared::Ready;
    }

    // The probe is worth its trip: a workspace prepared by an earlier run of dl
    // whose marker this run cannot see (a scratch cache directory, a cleared
    // cache) is the common case for anyone testing this, and lending 17MB again
    // to learn nothing is the expensive way to find out.
    //
    // A probe that comes back *non-zero* is the ordinary answer -- it is a `test`,
    // and the container has not been prepared yet -- so only a trip that never
    // ran at all stops this. Reading a failed test as a refusal is the bug this
    // shape exists to make impossible: it left every unprepared workspace
    // reporting "could not probe the container" with nothing after the colon,
    // because a `test` that fails says nothing on either stream.
    match run_remote(
        runner,
        config,
        workspace_id,
        &herdr::probe_command(metadata.len()),
        None,
        "probe the container",
    ) {
        Trip::Answered { success: true } => {
            // Prepared already, by an earlier run whose marker is gone. Record it
            // and spend nothing.
            record(&marker, &stamp);
            return Prepared::Ready;
        }
        Trip::Answered { success: false } => {}
        Trip::Unanswered { reason } => return Prepared::Refused { reason },
    }

    for (command, stdin, doing) in [
        (
            herdr::lend_command().to_owned(),
            Some(reporting.host_binary().to_path_buf()),
            "lend the session manager",
        ),
        (herdr::install_command(), None, "install the agent hook"),
    ] {
        match run_remote(runner, config, workspace_id, &command, stdin, doing) {
            Trip::Answered { success: true } => {}
            Trip::Answered { success: false } => {
                return Prepared::Refused {
                    reason: format!("could not {doing}"),
                };
            }
            Trip::Unanswered { reason } => return Prepared::Refused { reason },
        }
    }

    record(&marker, &stamp);
    Prepared::Ready
}

/// Remember what this workspace was given, so the next launch spends no trip.
///
/// Written only after the container has agreed, never before: a marker that runs
/// ahead of the install is a workspace that never gets prepared and never says
/// why. Failures are ignored on purpose -- an unwritable cache costs a round trip
/// per launch and nothing else.
fn record(marker: &Path, stamp: &str) {
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(marker, format!("{stamp}\n"));
}

/// What one non-interactive trip into the workspace came to.
///
/// Three states and not two, because the difference between them decides whether
/// a launch gives up: a command that ran and said no is an answer, and only a
/// trip that never happened is a refusal.
enum Trip {
    /// The container ran the command and this is what it exited with.
    Answered { success: bool },
    /// Nothing ran, or nothing came back. The reason is the user's to read.
    Unanswered { reason: String },
}

/// One non-interactive trip into the workspace.
fn run_remote(
    runner: &dyn Runner,
    config: &Path,
    workspace_id: &str,
    command: &str,
    // The lend's payload, and the one trip that has one: `Some` is a file handed
    // over as the child's stdin, `None` a trip that reads nothing.
    stdin: Option<std::path::PathBuf>,
    doing: &str,
) -> Trip {
    let invocation = Invocation::new(ssh::PROGRAM)
        .with_arg("-F")
        .with_arg(config.display().to_string())
        .with_arg(ssh::host_alias(workspace_id))
        .with_arg(command);
    let spec = match stdin {
        Some(path) => SpawnSpec::new(invocation).with_stdin_file(path),
        None => SpawnSpec::new(invocation),
    };
    match runner.capture(&spec) {
        Outcome::Ran { exit, io } => {
            if exit.is_success() {
                return Trip::Answered { success: true };
            }
            // ssh's own 255 is the one exit that means the trip did not happen:
            // the workspace is unreachable, which is a different thing from a
            // command inside it saying no.
            let complaint = last_line(&io.stderr)
                .or_else(|| last_line(&io.stdout))
                .unwrap_or_default();
            if exit == Exit::Code(SSH_TRANSPORT_FAILURE) {
                return Trip::Unanswered {
                    reason: format!("could not {doing}: {complaint}"),
                };
            }
            if complaint.is_empty() {
                Trip::Answered { success: false }
            } else {
                Trip::Unanswered {
                    reason: format!("could not {doing}: {complaint}"),
                }
            }
        }
        Outcome::ProgramNotFound => Trip::Unanswered {
            reason: format!("could not {doing}: no ssh on this host"),
        },
        Outcome::TimedOut => Trip::Unanswered {
            reason: format!("could not {doing}: the workspace did not answer"),
        },
        Outcome::NotStarted(failure) => Trip::Unanswered {
            reason: format!("could not {doing}: {failure:?}"),
        },
    }
}

/// OpenSSH's own failure exit, which is not the remote command's.
const SSH_TRANSPORT_FAILURE: i32 = 255;

/// The last line that says anything, which is where a shell puts its complaint.
fn last_line(stream: &str) -> Option<String> {
    stream
        .lines()
        .map(str::trim)
        // devpod's alias resolves no hostname inside the container, so `sudo`
        // warns about it on every single trip. It is not this flow's news.
        .rfind(|line| !line.is_empty() && !line.contains("unable to resolve host"))
        .map(str::to_owned)
}

/// The connection that carries the manager's socket into the container.
///
/// Detached rather than waited on: it has to outlive this call and live exactly as
/// long as the session that follows it, which is the caller's business and not
/// this function's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Forward {
    pid: u32,
}

impl Forward {
    /// Take the forward down.
    ///
    /// `SIGTERM` and not `SIGKILL`: OpenSSH removes the remote listen path on a
    /// clean exit, and a killed forward leaves a dead socket in the container that
    /// the next launch has to unlink. It is unlinked either way
    /// (`StreamLocalBindUnlink=yes`), so this is tidiness rather than correctness,
    /// which is why nothing here waits to see it happen.
    pub(crate) fn stop(self, runner: &dyn Runner) {
        let _ = kill::signal(runner, Signal::Terminate, &NonEmpty::one(self.pid));
    }
}

/// Open the forward, or say why there is none.
pub(crate) fn start_forward(
    runner: &dyn Runner,
    config: &Path,
    workspace_id: &str,
    reporting: &Reporting,
) -> Result<Forward, String> {
    let argv = reporting.forward_argv(config, workspace_id);
    let (program, args) = argv.split_first().expect("the forward names ssh");
    match runner.detach(&Invocation::new(program.clone()).with_args(args.iter().cloned())) {
        DetachOutcome::Started { pid } => Ok(Forward { pid }),
        DetachOutcome::ProgramNotFound => Err("no ssh on this host".to_owned()),
        DetachOutcome::NotStarted(failure) => Err(format!("{failure:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::herdr::HostEnv;
    use crate::testing::ScriptedRunner;
    use devlaunch_test_support::Response;

    fn reporting() -> Reporting {
        let binary = std::env::current_exe().expect("this test binary is a readable file");
        Reporting::resolve(&HostEnv {
            enabled: Some("1".to_owned()),
            in_pane: Some("1".to_owned()),
            pane_id: Some("w1:p3".to_owned()),
            socket: Some("/run/user/1000/herdr.sock".to_owned()),
            binary: Some(binary.display().to_string()),
        })
        .expect("a host in a manager's pane")
    }

    /// The steady state, and the one that has to be free: a marker that matches
    /// the host binary means no trip at all.
    #[test]
    fn a_prepared_workspace_costs_no_round_trip() {
        let cache = tempfile::tempdir().expect("a scratch cache");
        let reporting = reporting();
        let len = std::fs::metadata(reporting.host_binary())
            .expect("readable")
            .len();
        let marker = herdr::marker_path(cache.path(), "myws");
        std::fs::create_dir_all(marker.parent().expect("a parent")).expect("the marker dir");
        std::fs::write(&marker, format!("{}\n", herdr::stamp(len))).expect("the marker");
        let runner = ScriptedRunner::new();

        assert_eq!(
            prepare(
                &runner,
                cache.path(),
                Path::new("/tmp/ssh-config"),
                "myws",
                &reporting
            ),
            Prepared::Ready
        );
        assert!(
            runner.calls().is_empty(),
            "a prepared workspace was asked something: {:?}",
            runner.calls()
        );
    }

    /// The bug this shape exists to prevent, as a test.
    ///
    /// The probe is a `test`, so an unprepared container answers non-zero and says
    /// nothing on either stream. Reading that as a refusal meant every workspace
    /// that had not been prepared reported "could not probe the container" with
    /// nothing after the colon and was never prepared -- the one path this whole
    /// flow exists for.
    #[test]
    fn an_unprepared_container_is_prepared_rather_than_refused() {
        let cache = tempfile::tempdir().expect("a scratch cache");
        let reporting = reporting();
        let len = std::fs::metadata(reporting.host_binary())
            .expect("readable")
            .len();
        let config = Path::new("/tmp/ssh-config");
        let runner = ScriptedRunner::new();
        // The probe: ran, said no, said nothing.
        runner.script(
            [
                "ssh",
                "-F",
                "/tmp/ssh-config",
                "myws.devpod",
                &herdr::probe_command(len),
            ],
            Response::exited(1),
        );
        // The lend and the install, in that order, both fine.
        runner.script(["ssh"], Response::ok());

        assert_eq!(
            prepare(&runner, cache.path(), config, "myws", &reporting),
            Prepared::Ready
        );
        let commands: Vec<String> = runner
            .calls_to("ssh")
            .into_iter()
            .filter_map(|call| call.args().last().map(|arg| arg.to_owned()))
            .collect();
        assert_eq!(commands.len(), 3, "{commands:?}");
        assert!(
            commands[1].contains("mv /usr/local/bin/"),
            "the second trip is not the lend: {}",
            commands[1]
        );
        assert!(
            commands[2].contains("managed-settings.json"),
            "the third trip is not the install: {}",
            commands[2]
        );
        assert!(
            herdr::marker_path(cache.path(), "myws").exists(),
            "a prepared workspace was not recorded, so the next launch pays again"
        );
    }

    /// A container that answers the probe cleanly has been prepared by an earlier
    /// run whose marker is gone, and must cost one trip rather than a fresh lend.
    #[test]
    fn a_container_that_is_already_prepared_is_not_lent_to_again() {
        let cache = tempfile::tempdir().expect("a scratch cache");
        let reporting = reporting();
        let runner = ScriptedRunner::new();
        runner.script(["ssh"], Response::ok());

        assert_eq!(
            prepare(
                &runner,
                cache.path(),
                Path::new("/tmp/ssh-config"),
                "myws",
                &reporting
            ),
            Prepared::Ready
        );
        assert_eq!(runner.calls_to("ssh").len(), 1, "the probe was not enough");
        assert!(herdr::marker_path(cache.path(), "myws").exists());
    }

    /// An unreachable workspace is not an unprepared one: ssh's own 255 must not
    /// be read as a container saying no, or a launch would lend into a workspace
    /// that is not answering.
    #[test]
    fn an_unreachable_workspace_is_refused_rather_than_lent_to() {
        let cache = tempfile::tempdir().expect("a scratch cache");
        let runner = ScriptedRunner::new();
        runner.script(["ssh"], Response::failed(255, "Connection refused"));

        let Prepared::Refused { reason } = prepare(
            &runner,
            cache.path(),
            Path::new("/tmp/ssh-config"),
            "myws",
            &reporting(),
        ) else {
            panic!("an unreachable workspace cannot be prepared");
        };
        assert!(reason.contains("Connection refused"), "{reason}");
        assert_eq!(
            runner.calls_to("ssh").len(),
            1,
            "dl kept going after the workspace stopped answering"
        );
    }

    /// A marker naming a different binary is not this binary's marker.
    #[test]
    fn a_marker_from_another_version_is_not_believed() {
        let cache = tempfile::tempdir().expect("a scratch cache");
        let reporting = reporting();
        let marker = herdr::marker_path(cache.path(), "myws");
        std::fs::create_dir_all(marker.parent().expect("a parent")).expect("the marker dir");
        std::fs::write(&marker, herdr::stamp(1)).expect("the marker");
        let runner = ScriptedRunner::new();
        runner.script(["ssh"], Response::failed(255, "no such container"));

        assert!(matches!(
            prepare(
                &runner,
                cache.path(),
                Path::new("/tmp/ssh-config"),
                "myws",
                &reporting
            ),
            Prepared::Refused { .. }
        ));
        assert!(
            !runner.calls().is_empty(),
            "nothing was asked of the container"
        );
    }

    /// The refusal carries the container's own last word, and not the `sudo`
    /// hostname warning that rides along on every trip.
    #[test]
    fn a_refusal_names_what_the_container_said() {
        let cache = tempfile::tempdir().expect("a scratch cache");
        let runner = ScriptedRunner::new();
        runner.script(
            ["ssh"],
            Response::failed(
                1,
                "sudo: unable to resolve host myws\nsudo: a password is required",
            ),
        );

        let Prepared::Refused { reason } = prepare(
            &runner,
            cache.path(),
            Path::new("/tmp/ssh-config"),
            "myws",
            &reporting(),
        ) else {
            panic!("a container that refuses sudo cannot be prepared");
        };
        assert!(
            reason.contains("a password is required"),
            "the reason lost the container's words: {reason}"
        );
        assert!(
            !reason.contains("unable to resolve host"),
            "the reason kept sudo's hostname noise: {reason}"
        );
    }

    /// A refusal must not leave a marker: the next launch has to try again.
    #[test]
    fn a_refused_workspace_is_not_marked_as_prepared() {
        let cache = tempfile::tempdir().expect("a scratch cache");
        let runner = ScriptedRunner::new();
        runner.script(["ssh"], Response::failed(1, "nope"));

        let _ = prepare(
            &runner,
            cache.path(),
            Path::new("/tmp/ssh-config"),
            "myws",
            &reporting(),
        );

        assert!(
            !herdr::marker_path(cache.path(), "myws").exists(),
            "a refused install left a marker claiming it succeeded"
        );
    }
}
