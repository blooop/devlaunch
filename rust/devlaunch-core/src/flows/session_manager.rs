//! Making an agent inside a workspace visible to a session manager outside it.
//!
//! [`crate::clients::herdr`] holds every decision this flow makes and every string
//! it sends; this is the round trips, in the order they have to happen:
//!
//! 1. [`start_forward`] opens the connection that carries the manager's socket in,
//!    and hands back something the caller stops when the session ends.
//! 2. [`prepare`] asks the container what it already has -- the socket from step 1
//!    included -- and puts the manager's own binary and a Claude Code hook there if
//!    it is missing either. One round trip in the steady state, three when there is
//!    work to do. The question is asked every launch on purpose: nothing on this
//!    host can know whether the container that was prepared is the container that
//!    is running now, and nothing on this host can see whether a detached forward
//!    bound its listen path.
//!
//! In that order, because the forward is what step 2 asks about. It is also the
//! cheap one to undo: a `prepare` that refuses leaves the caller a forward to stop
//! and no container state to unpick.
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
use crate::runner::interrupt;
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
/// Three trips at worst and one at best, and the one is not optional. A cache of
/// what a workspace was given used to make the steady state free, and it was
/// wrong in the way a cache of someone else's state is always wrong: `dl <ws>
/// recreate`, `dl <ws> reset`, a `devpod delete`, or a rebuilt image all replace
/// the container and none of them change the workspace id, so a marker keyed on
/// that id went on describing a container that no longer existed. dl then printed
/// "reporting agents in this workspace" over a container that had never heard of
/// herdr, and no agent inside it was ever visible again -- the exact silent
/// nothing this whole flow exists to remove, now stated positively.
///
/// The container is the only thing that knows, so the container is asked. The
/// probe compares the binary's size, which is the cheapest thing that changes
/// when the manager is updated, and it asks after the forwarded socket in the
/// same trip -- which is why it runs *after* [`start_forward`] and not before.
pub(crate) fn prepare(
    runner: &dyn Runner,
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
    // A non-zero probe is the ordinary answer and not a failure -- it is a `test`,
    // and the container has not been prepared yet. Only a trip that never ran at
    // all stops a launch; see [`Trip`], which is where that has gone wrong twice.
    match run_remote(
        runner,
        config,
        workspace_id,
        &herdr::probe_command(metadata.len(), &reporting.container_socket()),
        None,
        "probe the container",
    ) {
        // Prepared already, by this launch or an earlier one, and nothing to do.
        Trip::Ran { exit, .. } if exit.is_success() => return Prepared::Ready,
        // The forward reported a pid and delivered nothing. Lending into this
        // container would work and buy nothing: the hook would fire, find no
        // socket, and stay quiet. Nothing dl can install repairs it, so this is
        // the one probe answer that is a refusal rather than a to-do list.
        Trip::Ran {
            exit: Exit::Code(herdr::PROBE_NO_SOCKET),
            ..
        } => {
            return Prepared::Refused {
                reason: format!(
                    "the manager's socket did not arrive at {} in the workspace, so the \
                     forward bound nothing",
                    reporting.container_socket()
                ),
            };
        }
        Trip::Ran { .. } => {}
        Trip::NeverRan { reason } => return Prepared::Refused { reason },
    }

    for errand in [
        Errand {
            command: herdr::lend_command().to_owned(),
            stdin: Some(reporting.host_binary().to_path_buf()),
            doing: "lend the session manager",
            refuses: None,
        },
        Errand {
            command: herdr::install_command(),
            stdin: None,
            doing: "install the agent hook",
            refuses: Some((
                herdr::INSTALL_FOREIGN_SETTINGS,
                format!(
                    "the workspace already holds managed Claude Code settings dl did not write \
                     at {}, and whatever policy they carry is worth more than a status indicator",
                    herdr::CONTAINER_SETTINGS
                ),
            )),
        },
    ] {
        let Errand {
            command,
            stdin,
            doing,
            refuses,
        } = errand;
        match run_remote(runner, config, workspace_id, &command, stdin, doing) {
            Trip::Ran { exit, .. } if exit.is_success() => {}
            Trip::Ran { exit, complaint } => {
                return Prepared::Refused {
                    reason: match refuses {
                        Some((code, said)) if exit == Exit::Code(code) => said,
                        _ => refusal(doing, complaint.as_deref()),
                    },
                };
            }
            Trip::NeverRan { reason } => return Prepared::Refused { reason },
        }
    }

    Prepared::Ready
}

/// One thing [`prepare`] asks the container to do.
///
/// `refuses` is the one status this errand answers with that means something more
/// specific than "it failed", so the refusal can say the specific thing. Only the
/// install has one, and a status is per-errand rather than global because the
/// numbers are only meaningful against the command that returned them.
struct Errand {
    command: String,
    stdin: Option<std::path::PathBuf>,
    doing: &'static str,
    refuses: Option<(i32, String)>,
}

/// What one non-interactive trip into the workspace came to.
///
/// Two states, and the line between them is drawn by the **exit status alone**:
/// a command that ran and said no is an answer whatever it printed on the way,
/// and only a trip that never happened is a refusal. Drawing it anywhere else is
/// the bug this shape has now had twice. The first spelling read an empty pair of
/// streams as the answer, which left a `test` that fails silently reported as a
/// refusal; the second read *any* output as a refusal, which is worse, because
/// output on a failed trip is the common case rather than the odd one -- Debian's
/// bash runs the remote `~/.bashrc` on `ssh host <cmd>`, so a pixi or nvm init
/// line, a motd, or a locale warning rides along with every exit status.
///
/// The complaint therefore travels beside the status instead of standing in for
/// it: a refusal that has one still quotes the container's own last word.
enum Trip {
    /// The container ran the command, and this is how it ended and what it said.
    Ran {
        exit: Exit,
        complaint: Option<String>,
    },
    /// Nothing ran, or nothing came back. The reason is the user's to read.
    NeverRan { reason: String },
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
            let complaint = last_line(&io.stderr).or_else(|| last_line(&io.stdout));
            match exit {
                // ssh's own 255 is the one exit that means the trip did not
                // happen: the workspace is unreachable, which is a different
                // thing from a command inside it saying no. A signalled ssh is
                // the same news -- whatever the remote command was doing, this
                // end stopped listening before it heard the answer.
                Exit::Code(SSH_TRANSPORT_FAILURE) | Exit::Signal(_) => Trip::NeverRan {
                    reason: refusal(doing, complaint.as_deref()),
                },
                exit => Trip::Ran { exit, complaint },
            }
        }
        Outcome::ProgramNotFound => Trip::NeverRan {
            reason: format!("could not {doing}: no ssh on this host"),
        },
        Outcome::TimedOut => Trip::NeverRan {
            reason: format!("could not {doing}: the workspace did not answer"),
        },
        Outcome::NotStarted(failure) => Trip::NeverRan {
            reason: format!("could not {doing}: {failure:?}"),
        },
    }
}

/// One refusal sentence, with the container's last word if it had one.
fn refusal(doing: &str, complaint: Option<&str>) -> String {
    match complaint {
        Some(said) => format!("could not {doing}: {said}"),
        None => format!("could not {doing}"),
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
#[derive(Debug)]
pub(crate) struct Forward {
    pid: u32,
    /// Registered for the interrupt handler, and held for exactly as long as the
    /// forward is: dropping it frees the slot so the handler cannot signal a pid
    /// that has since been recycled. `None` only if every slot was full, which
    /// costs this forward its interrupt-time cleanup and nothing else.
    ///
    /// Without it a `dl` that is Ctrl-C'd leaves the forward running: the child is
    /// `setsid`'d, so a terminal's signal to the foreground process group never
    /// reaches it, and `dl`'s handler `_exit`s without unwinding, so [`stop`] --
    /// which only runs on the ordinary return path -- never happens.
    ///
    /// [`stop`]: Forward::stop
    _cleanup: Option<interrupt::Registration>,
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
        DetachOutcome::Started { pid } => Ok(Forward {
            pid,
            _cleanup: interrupt::register_pid(i32::try_from(pid).unwrap_or(0)),
        }),
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

    /// Every launch asks the container, because only the container knows.
    ///
    /// A marker under dl's cache directory used to answer this for free, keyed on
    /// the workspace id and the host binary's size. Both survive the container:
    /// `dl <ws> recreate` and `dl <ws> reset` destroy it, keep the id, and end in
    /// a session, so the second launch here returned `Ready` after asking nothing
    /// at all -- and dl then announced it was reporting agents to a pane over a
    /// container that had no herdr in it, forever, with the failure visible
    /// nowhere.
    #[test]
    fn a_second_launch_asks_the_container_again() {
        let reporting = reporting();
        let config = Path::new("/tmp/ssh-config");
        let runner = ScriptedRunner::new();
        // The probe, answering that this container is prepared already.
        runner.script(["ssh"], Response::ok());

        for _ in 0..2 {
            assert_eq!(
                prepare(&runner, config, "myws", &reporting),
                Prepared::Ready
            );
        }
        assert_eq!(
            runner.calls_to("ssh").len(),
            2,
            "the second launch believed a host-side record over the container: {:?}",
            runner.calls_to("ssh")
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
                &herdr::probe_command(len, &reporting.container_socket()),
            ],
            Response::exited(1),
        );
        // The lend and the install, in that order, both fine.
        runner.script(["ssh"], Response::ok());

        assert_eq!(
            prepare(&runner, config, "myws", &reporting),
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
    }

    /// A container that answers the probe cleanly is prepared already, and must
    /// cost that one trip rather than a fresh 17MB lend.
    #[test]
    fn a_container_that_is_already_prepared_is_not_lent_to_again() {
        let reporting = reporting();
        let runner = ScriptedRunner::new();
        runner.script(["ssh"], Response::ok());

        assert_eq!(
            prepare(&runner, Path::new("/tmp/ssh-config"), "myws", &reporting),
            Prepared::Ready
        );
        assert_eq!(runner.calls_to("ssh").len(), 1, "the probe was not enough");
    }

    /// An unreachable workspace is not an unprepared one: ssh's own 255 must not
    /// be read as a container saying no, or a launch would lend into a workspace
    /// that is not answering.
    #[test]
    fn an_unreachable_workspace_is_refused_rather_than_lent_to() {
        let runner = ScriptedRunner::new();
        runner.script(["ssh"], Response::failed(255, "Connection refused"));

        let Prepared::Refused { reason } =
            prepare(&runner, Path::new("/tmp/ssh-config"), "myws", &reporting())
        else {
            panic!("an unreachable workspace cannot be prepared");
        };
        assert!(reason.contains("Connection refused"), "{reason}");
        assert_eq!(
            runner.calls_to("ssh").len(),
            1,
            "dl kept going after the workspace stopped answering"
        );
    }

    /// The same bug as above, with the trigger that is common rather than rare.
    ///
    /// A failed probe that *says* something is still a probe that answered. The
    /// first fix for this read an empty pair of streams as the answer, so it
    /// covered a silent `test` and nothing else -- and a container is rarely
    /// silent. Debian's bash is built with `SSH_SOURCE_BASHRC`, so
    /// `ssh host <cmd>` runs the remote `~/.bashrc`: a pixi or nvm init line, an
    /// `/etc/bash.bashrc` message or a locale warning arrives on stderr beside
    /// the exit status. Reading that as "the trip never happened" refused every
    /// such workspace on every launch, forever, and never lent anything.
    #[test]
    fn a_probe_that_answers_no_out_loud_is_still_an_answer() {
        let reporting = reporting();
        let len = std::fs::metadata(reporting.host_binary())
            .expect("readable")
            .len();
        let config = Path::new("/tmp/ssh-config");
        let runner = ScriptedRunner::new();
        runner.script(
            [
                "ssh",
                "-F",
                "/tmp/ssh-config",
                "myws.devpod",
                &herdr::probe_command(len, &reporting.container_socket()),
            ],
            Response::failed(1, "bash: warning: setlocale: LC_ALL: cannot change locale"),
        );
        // The lend and the install, in that order, both fine.
        runner.script(["ssh"], Response::ok());

        assert_eq!(
            prepare(&runner, config, "myws", &reporting),
            Prepared::Ready,
            "a container that warned about its locale was read as unreachable"
        );
        assert_eq!(
            runner.calls_to("ssh").len(),
            3,
            "the probe answered no and nothing was lent: {:?}",
            runner.calls_to("ssh")
        );
    }

    /// A forward that reported a pid and bound nothing is not a prepared
    /// workspace, and lending into it would buy silence.
    ///
    /// `detach` gives the forward `/dev/null` for stderr and never waits for it, so
    /// `ExitOnForwardFailure=yes` takes the connection down and tells nobody. A
    /// container whose user cannot create the listen path -- a root-owned `/tmp`, a
    /// stale root-owned socket -- therefore used to be announced as "reporting
    /// agents in this workspace" and report nothing. The probe is where it becomes
    /// visible, and its answer is a refusal because no lend repairs it.
    #[test]
    fn a_forward_that_bound_nothing_is_refused_rather_than_lent_to() {
        let reporting = reporting();
        let runner = ScriptedRunner::new();
        runner.script(["ssh"], Response::exited(herdr::PROBE_NO_SOCKET));

        let Prepared::Refused { reason } =
            prepare(&runner, Path::new("/tmp/ssh-config"), "myws", &reporting)
        else {
            panic!("a workspace the socket never reached cannot report an agent");
        };
        assert!(
            reason.contains(&reporting.container_socket()),
            "the reason does not say which socket never arrived: {reason}"
        );
        assert_eq!(
            runner.calls_to("ssh").len(),
            1,
            "17MB was lent into a workspace that has no socket to report over"
        );
    }

    /// The refusal for a foreign settings file says what it is, because "could not
    /// install the agent hook" would send a reader looking for a broken install.
    #[test]
    fn a_workspace_with_its_own_managed_settings_is_left_alone() {
        let reporting = reporting();
        let len = std::fs::metadata(reporting.host_binary())
            .expect("readable")
            .len();
        let config = Path::new("/tmp/ssh-config");
        let runner = ScriptedRunner::new();
        runner.script(
            [
                "ssh",
                "-F",
                "/tmp/ssh-config",
                "myws.devpod",
                &herdr::probe_command(len, &reporting.container_socket()),
            ],
            Response::exited(1),
        );
        runner.script(
            [
                "ssh",
                "-F",
                "/tmp/ssh-config",
                "myws.devpod",
                herdr::lend_command(),
            ],
            Response::ok(),
        );
        runner.script(
            [
                "ssh",
                "-F",
                "/tmp/ssh-config",
                "myws.devpod",
                &herdr::install_command(),
            ],
            Response::exited(herdr::INSTALL_FOREIGN_SETTINGS),
        );

        let Prepared::Refused { reason } = prepare(&runner, config, "myws", &reporting) else {
            panic!("dl overwrote settings it did not write");
        };
        assert!(
            reason.contains(herdr::CONTAINER_SETTINGS) && reason.contains("did not write"),
            "the refusal reads as a broken install rather than a file left alone: {reason}"
        );
    }

    /// The refusal carries the container's own last word, and not the `sudo`
    /// hostname warning that rides along on every trip.
    #[test]
    fn a_refusal_names_what_the_container_said() {
        let runner = ScriptedRunner::new();
        runner.script(
            ["ssh"],
            Response::failed(
                1,
                "sudo: unable to resolve host myws\nsudo: a password is required",
            ),
        );

        let Prepared::Refused { reason } =
            prepare(&runner, Path::new("/tmp/ssh-config"), "myws", &reporting())
        else {
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
}
