//! The host's own process table, read once rather than a `ps` per question.
//!
//! `dl <ws> kill` is the only caller, and the question it asks is the issue's
//! own: which processes name this workspace, and which of them have outlived
//! whatever started them. Both halves need the same reading — a pid is only an
//! orphan relative to the rest of the table — so this returns the table and
//! decides nothing about it.
//!
//! # Why a spawn and not `/proc`
//!
//! For the reason [`super::docker`] is a spawn: it puts the reading on the one
//! seam every other process devlaunch starts already goes through, so a test
//! scripts `ps`'s output the way it scripts `devpod list`'s, and no test reaches
//! the developer's own process table. Walking `/proc` would be a second seam,
//! with a second fake, for a table `ps` already formats.
//!
//! # `-ww`, and it is load-bearing
//!
//! `ps` truncates `args` to the terminal's width by default, and the flags that
//! tell a `devpod up` from a `devpod ssh` are at the end of a line that also
//! carries a devcontainer path. A truncated line reads as a process that does
//! not name the workspace, which is the one mistake this must not make: it kills
//! nothing and reports nothing, and the workspace stays wedged.

use std::io::ErrorKind;
use std::time::Duration;

use crate::runner::{CapturedText, Exit, Invocation, OsFailure, Outcome, Runner, SpawnSpec};

/// The program every call in this module runs.
pub(crate) const PROGRAM: &str = "ps";

/// What reading the whole table may cost before it is abandoned.
///
/// A bound rather than none, unlike the `docker volume rm` beside it: this runs
/// *because* something on the host is wedged, so the reading it opens with is
/// exactly the call that must not join it.
const READ_THE_TABLE: Duration = Duration::from_secs(5);

/// One row of the host's process table.
///
/// `command` is the whole command line as one string rather than a vector of
/// words, because that is what `ps` has: it joins the argv with spaces, and an
/// argument that contained one is already indistinguishable from two arguments.
/// Splitting here would claim a precision the source does not have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostProcess {
    pub pid: u32,
    pub parent: u32,
    pub command: String,
}

/// What asking the host for its process table came to.
///
/// Three ways of not having one, kept apart for [`super::docker::Answer`]'s
/// reason: they are different facts and only one of them is about this machine
/// being unusual. A host with no `ps` is a host `dl <ws> kill` cannot work on at
/// all, and saying so beats reporting that it killed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    /// The table, in the order `ps` listed it.
    Read(Vec<HostProcess>),
    /// `ps` is not on PATH, or is there and cannot be exec'd.
    NotInstalled,
    /// `ps` ran and would not answer. `stderr` is its own words.
    Refused { exit: Exit, stderr: String },
    /// `ps` never answered. `failure.kind` is [`ErrorKind::TimedOut`] where the
    /// child outlasted [`READ_THE_TABLE`].
    NotStarted(OsFailure),
}

/// Every process on this host, with its parent and its command line.
pub(crate) fn processes(runner: &dyn Runner) -> Answer {
    let spec = SpawnSpec::from(Invocation::new(PROGRAM).with_args([
        "-e",
        "-ww",
        "-o",
        "pid=,ppid=,args=",
    ]))
    .with_timeout(READ_THE_TABLE);
    match runner.capture(&spec) {
        Outcome::Ran {
            exit,
            io: CapturedText { stdout, stderr },
        } => {
            if exit.is_success() {
                Answer::Read(parse(&stdout))
            } else {
                Answer::Refused { exit, stderr }
            }
        }
        Outcome::ProgramNotFound => Answer::NotInstalled,
        Outcome::TimedOut => Answer::NotStarted(OsFailure {
            kind: ErrorKind::TimedOut,
            errno: None,
        }),
        // A `ps` found on PATH that could not be exec'd fails with ENOENT at exec
        // time, which arrives here rather than as `ProgramNotFound`; it points at
        // the same fix, so it gets the same answer.
        Outcome::NotStarted(failure) if failure.kind == ErrorKind::NotFound => Answer::NotInstalled,
        Outcome::NotStarted(failure) => Answer::NotStarted(failure),
    }
}

/// The rows of `pid=,ppid=,args=` output.
///
/// Total over anything `ps` could print: a line whose first two fields are not
/// numbers, or which has no third field, is dropped rather than guessed at. The
/// only use for this table is deciding what to signal, and a row this cannot
/// read is one it must not signal on.
fn parse(text: &str) -> Vec<HostProcess> {
    text.lines().filter_map(row).collect()
}

fn row(line: &str) -> Option<HostProcess> {
    let (pid, rest) = field(line)?;
    let (parent, rest) = field(rest)?;
    // The remainder of the line, not its words rejoined: a rejoin would re-space
    // a command line whose own spacing is the only evidence of where one argument
    // ended and the next began.
    let command = rest.trim();
    if command.is_empty() {
        return None;
    }
    Some(HostProcess {
        pid: pid.parse().ok()?,
        parent: parent.parse().ok()?,
        command: command.to_owned(),
    })
}

/// The first whitespace-delimited field of `text`, and everything after it.
fn field(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    let end = text.find(char::is_whitespace)?;
    Some((&text[..end], &text[end..]))
}

#[cfg(test)]
mod tests {
    use devlaunch_test_support::{FakeRunner, Response};

    use super::*;

    #[test]
    fn the_whole_table_is_read_in_one_untruncated_pass() {
        let fake = FakeRunner::new();

        let _ = processes(&fake);

        assert_eq!(
            fake.argvs(),
            [["ps", "-e", "-ww", "-o", "pid=,ppid=,args="]]
        );
    }

    /// The three columns off the issue's own `ps`, and the argv is kept whole
    /// rather than split: the flags after the workspace id are what tell a
    /// `devpod up` from a `devpod ssh`.
    #[test]
    fn each_line_is_a_pid_a_parent_and_the_rest() {
        let fake = FakeRunner::new();
        fake.script(
            ["ps"],
            Response::stdout(
                "732721       1 devpod up my-ws --ide none\n  1234  732721 devpod ssh my-ws\n",
            ),
        );

        assert_eq!(
            processes(&fake),
            Answer::Read(vec![
                HostProcess {
                    pid: 732_721,
                    parent: 1,
                    command: "devpod up my-ws --ide none".to_owned(),
                },
                HostProcess {
                    pid: 1234,
                    parent: 732_721,
                    command: "devpod ssh my-ws".to_owned(),
                },
            ])
        );
    }

    /// The reading opens a command that runs *because* the host is wedged, so it
    /// is the one call that must not be able to join the wait it was reached for.
    #[test]
    fn the_reading_is_bounded() {
        let fake = FakeRunner::new();

        let _ = processes(&fake);

        assert_eq!(
            fake.calls()[0]
                .spec()
                .expect("a captured spawn carries its bound")
                .timeout,
            Some(READ_THE_TABLE)
        );
    }

    /// A host with no `ps` is a host this cannot work on, and it has to say so:
    /// an empty table would read as a machine with nothing wedged on it, which is
    /// the one wrong answer that leaves the workspace stuck and reports success.
    #[test]
    fn a_machine_with_no_ps_is_not_a_machine_with_no_processes() {
        let fake = FakeRunner::new();
        fake.script_missing("ps");

        assert_eq!(processes(&fake), Answer::NotInstalled);
    }

    /// A row whose numbers are not numbers is dropped rather than guessed at,
    /// because the only use for this table is deciding what to signal.
    #[test]
    fn a_line_that_is_not_a_row_is_dropped_rather_than_guessed_at() {
        let fake = FakeRunner::new();
        fake.script(
            ["ps"],
            Response::stdout("  PID  PPID COMMAND\n  7 1 devpod up my-ws\n  8 2\n"),
        );

        assert_eq!(
            processes(&fake),
            Answer::Read(vec![HostProcess {
                pid: 7,
                parent: 1,
                command: "devpod up my-ws".to_owned(),
            }])
        );
    }

    /// A `ps` that ran and refused keeps its own words: this is a fact about the
    /// host, and the binary is what phrases it.
    #[test]
    fn a_ps_that_refused_answers_with_its_own_words() {
        let fake = FakeRunner::new();
        fake.script(["ps"], Response::failed(1, "error: unsupported option\n"));

        assert_eq!(
            processes(&fake),
            Answer::Refused {
                exit: Exit::Code(1),
                stderr: "error: unsupported option\n".to_owned(),
            }
        );
    }
}
