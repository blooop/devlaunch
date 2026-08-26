//! `dl <ws> kill`: the hammer for a workspace that will not answer.
//!
//! The wedge this exists for is devlaunch#484, and it is worth stating exactly,
//! because the shape of the verb follows from it. A `devpod up` outlived the `dl`
//! that started it, was reparented to init, and is holding the workspace's
//! `flock`. Every later `devpod up` on that workspace blocks in `initLock`, which
//! logs `Trying to lock workspace` every five seconds behind a *blocking*
//! acquire: there is no deadline, so it waits for as long as the orphan lives,
//! which for an init-reparented process is forever. Killing the orphan is the
//! whole fix; nothing else has to be repaired.
//!
//! # Two files called `workspace.lock`, and only one of them is swept
//!
//! - **The flock**, under devpod's `contexts/<ctx>/locks`. This is what blocks,
//!   and it is **never unlinked** — [`crate::domain::locks`] argues the case at
//!   length and it applies verbatim to devpod's: a process holding the old inode
//!   still holds a lock nobody else can see, while new arrivals lock a fresh file
//!   and walk past it. Two processes then both believe they hold the workspace.
//!   The kernel drops this one when the holder dies, so the kill *is* the release.
//! - **The busy marker**, under devpod's `agent/contexts/<ctx>/workspaces/<id>`.
//!   A plain file, created and removed by a `defer` in devpod's agent, and a
//!   `defer` does not run under SIGKILL — so this is the one that actually goes
//!   stale, and the one worth removing. Only ever when nothing live is still
//!   building the workspace: a marker removed out from under an attended build
//!   tells devpod's daemon the build finished when it has not.
//!
//! # What this deliberately does not answer
//!
//! Why the orphan exists at all. Something killed a `dl` and left its child
//! running, which is either a path outside #304's SIGTERM drain or a signal that
//! drain cannot catch. That is a different question with a different fix, and a
//! verb that treats the symptom does not stop being worth having while it is
//! open.

use std::path::PathBuf;
use std::time::Duration;

use crate::clients::devpod_home::{self as devpod_home, DevpodHome};
use crate::clients::docker;
use crate::clients::kill::{self as signals, Sent, Signal};
use crate::clients::ps;

pub use crate::clients::ps::HostProcess;
use crate::domain::workspace_state::NonEmpty;
use crate::osext::system_words;
use crate::runner::{Exit, OsFailure, Runner};

/// Whether anything is still waiting on a process that names this workspace.
///
/// The distinction the whole verb turns on. An orphan is a leftover: nobody
/// reaps it, nobody reads its output, and it will hold the flock until the
/// machine reboots. An attended process is somebody's launch, mid-build, and
/// killing it takes a workspace away from whoever asked for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Parentage {
    /// Reparented to init, or a parent that has left the table.
    Orphaned,
    /// The parent is still running.
    Attended,
}

/// One process that names this workspace, and whether anything is behind it.
///
/// A record rather than two lists, because both questions are answered off the
/// same reading and a second list is how they come to disagree: what to signal
/// is the orphans, and whether the busy marker may go is whether any *attended*
/// holder is left. Split into two vectors, a table read once could be filtered
/// twice by two different rules.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Holder {
    process: HostProcess,
    parentage: Parentage,
}

/// Every devpod process naming `workspace_id`, classified by what is behind it.
///
/// **Only devpod**, and that is the safety property rather than a filter for
/// tidiness: the `dl` running this very command names the workspace in its own
/// argv, and so does the agent that launched it, and so does an editor's ssh.
/// devpod is what holds devpod's lock, so devpod is the whole of what is in
/// scope — anything wider is a verb that kills the terminal it was typed into.
///
/// **A whole word**, never a substring. Workspace ids share prefixes by
/// construction: the same repo on two branches differs in a suffix, so a
/// substring match on the shorter one would sweep up the longer one's live
/// launch. A value after `=` counts as a word, because devpod's helpers pass the
/// id that way.
fn holders(table: &[HostProcess], workspace_id: &str) -> Vec<Holder> {
    table
        .iter()
        .filter(|process| is_devpod(&process.command) && names(&process.command, workspace_id))
        .map(|process| Holder {
            process: process.clone(),
            parentage: parentage(table, process),
        })
        .collect()
}

/// Whether this command line runs devpod, by the name at the end of its path.
///
/// devpod is reached by bare name from a shell and by absolute path from
/// anything that resolved it once, and a workspace does not stop being wedged
/// because the holder was spawned the second way.
fn is_devpod(command: &str) -> bool {
    command
        .split_whitespace()
        .next()
        .and_then(|program| program.rsplit('/').next())
        .is_some_and(|program| program == crate::clients::devpod::PROGRAM)
}

/// Whether any word of this command line *is* the workspace id.
fn names(command: &str, workspace_id: &str) -> bool {
    command
        .split_whitespace()
        .any(|word| word == workspace_id || word.rsplit('=').next() == Some(workspace_id))
}

/// Whether anything is still behind `process`, as this table sees it.
///
/// Two ways to have nothing behind you, and both are the same fact. PPID 1 is
/// the reparented case the issue caught. A parent that is not in the table at
/// all is the same orphan seen a moment later, and it is the case a host running
/// a subreaper shows instead: there, a dead parent's children are reparented to
/// the reaper rather than to init, so PPID 1 alone would find nothing.
fn parentage(table: &[HostProcess], process: &HostProcess) -> Parentage {
    let reparented = process.parent <= 1;
    let parent_gone = !table.iter().any(|other| other.pid == process.parent);
    if reparented || parent_gone {
        Parentage::Orphaned
    } else {
        Parentage::Attended
    }
}

// ===========================================================================
// the sweep
// ===========================================================================

/// How long an orphan is given to unwind after SIGTERM.
///
/// A `devpod up` that takes the signal drops the flock as it goes, and a second
/// or two is the difference between a clean unwind and a SIGKILL that leaves the
/// busy marker behind for this flow to sweep. Not longer, because the person
/// typing this has already waited through the five-second loop that sent them
/// here.
const UNWIND: Duration = Duration::from_secs(2);

/// How long the host is given to reap a process after SIGKILL.
///
/// Not a grace period — nothing runs after SIGKILL — but the kernel still has
/// to tear the process down and the parent still has to reap it, and a table read
/// in the same instant can still show it.
const REAP: Duration = Duration::from_millis(200);

/// How far the escalation had to go before a process stopped holding the lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// SIGTERM was enough, so the unwind ran and the marker went with it.
    Terminated,
    /// It sat through SIGTERM and went under SIGKILL. Nothing it deferred ran.
    Killed,
    /// Still there after both signals. Almost always another user's process:
    /// devlaunch has no privilege to add, and the report has to say so rather
    /// than let the next `dl up` walk back into the same wait.
    Survived,
}

/// One orphan and what became of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signalled {
    pub process: HostProcess,
    pub ending: Ending,
}

/// Why the host's process table could not be read.
///
/// Core renders no English (#251): this carries the fact, and the binary is what
/// phrases it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TableUnreadable {
    /// No `ps` on this host, or one that could not be exec'd.
    NoPs,
    /// `ps` ran and refused, having written this.
    Refused { exit: Exit, stderr: String },
    /// `ps` never answered.
    NotStarted(OsFailure),
}

/// What `dl <ws> kill` came to.
///
/// The refusal is one nested arm rather than two flat ones so that the renderer
/// for it is total over the ways of *not* sweeping and cannot be handed a sweep:
/// a function that had to answer for `Swept` would need a sentence nobody should
/// ever read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Killed {
    Swept(Sweep),
    /// Nothing was swept, because this host cannot answer what the sweep is
    /// built on. Not an empty sweep: a person told "nothing was holding it" on a
    /// machine with no `ps` goes looking in the wrong place.
    Unavailable(HostCannot),
}

/// What this host cannot do, which is why nothing was swept on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostCannot {
    /// Nothing was read, so nothing was signalled and nothing was swept. Every
    /// step past the first is conditioned on knowing what is running.
    ReadItsProcessTable(TableUnreadable),
    /// Orphans were found and there is no `kill(1)` here to signal them with.
    SendASignal,
}

/// What became of devpod's busy marker for this workspace.
///
/// Five arms and not a bool, because four of them mean "the file is still there"
/// for four unrelated reasons and only one of them is a problem. The one that
/// matters is [`Marker::LeftForALiveHolder`]: a marker removed out from under a
/// live build tells devpod's daemon that build has finished, which is a worse
/// state than the wedge this verb was reached for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Marker {
    /// Removed. Nothing was left holding the workspace, so the marker was stale:
    /// a SIGKILLed `up` never reached the `defer` that would have removed it.
    Removed(PathBuf),
    /// Nothing there to remove. Either the workspace never had one, or the
    /// unwind SIGTERM allowed removed it on the way out.
    Absent,
    /// Left alone: something is still holding this workspace, so the marker is
    /// not stale and taking it would be a lie to devpod's daemon.
    LeftForALiveHolder,
    /// There, and it would not go. `reason` is the OS's own words.
    Unremovable { path: PathBuf, reason: String },
    /// Nowhere to look: no devpod home on this host, or no single context whose
    /// records name this workspace. Not a failure — every other part of the sweep
    /// still happened, and the marker is devpod's file rather than devlaunch's.
    Unlocatable,
}

/// What became of the containers this workspace still had running.
///
/// [`Containers::NoneRunning`] is the *ordinary* ending rather than a
/// disappointment, and the issue is where that comes from: the container there
/// had exited 137 a full minute before the restart that hung, so a `docker kill`
/// would have found nothing to kill. The container is not usually what is stuck.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Containers {
    /// docker killed these, by id.
    Killed(Vec<String>),
    /// This workspace's compose project has nothing running.
    NoneRunning,
    /// No docker on this machine, which is a machine that started no containers.
    NoDocker,
    /// docker was asked and would not deliver.
    Standing(ContainerRefusal),
}

/// Why a container this workspace has running is still running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContainerRefusal {
    /// docker ran and refused — the listing or the kill. `stderr` is its words.
    Refused { exit: Exit, stderr: String },
    /// docker never answered.
    NotRun { failure: OsFailure },
}

/// Whether anything is holding the workspace now that the sweep has finished.
///
/// **One fact, read once off the last process-table reading**, rather than
/// assembled by each of its three readers from the lists below. It is what the
/// exit code means, what the busy marker's removal hangs on, and what the closing
/// line of the report says, and three derivations of one question are how those
/// three come to disagree — a holder that arrived while the sweep ran is in
/// neither list and holds the workspace all the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holding {
    /// Nothing on this host names this workspace any more.
    Free,
    /// Something still does: a live build, a process that outlived SIGKILL, or
    /// one that arrived after the signals went out.
    StillHeld,
}

/// What one sweep found and what it did about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sweep {
    /// The orphans, in pid order, and how each one ended. Empty is a finding:
    /// nothing on this host is holding the workspace, so the hang is somewhere
    /// this verb does not reach.
    pub signalled: Vec<Signalled>,
    /// Live builds left standing. Reported rather than passed over in silence,
    /// because "it killed nothing" and "it found a build and left it alone" send
    /// the reader to two different places.
    pub attended: Vec<HostProcess>,
    /// What became of devpod's busy marker.
    pub marker: Marker,
    /// What became of the containers the workspace still had running.
    pub containers: Containers,
    /// Whether anything is holding the workspace now.
    pub holding: Holding,
}

/// Kill whatever is holding this workspace, and say what was killed.
///
/// `wait` is the grace period, passed in rather than slept, for the reason
/// `DevpodHome::located` takes its home-directory lookup: the escalation is the
/// behaviour worth pinning and a test should not have to spend two seconds of
/// real time per assertion to reach it. The binary passes `thread::sleep`.
///
/// **Three readings at most, and the last one is the one that counts.** What was
/// signalled is settled by looking again rather than by `kill`'s exit status: a
/// pid that exited between the reading and the signal makes `kill` exit non-zero
/// on a *successful* run, and a pid that survived makes it exit zero.
pub fn workspace_kill(
    runner: &dyn Runner,
    devpod_home: Option<&DevpodHome>,
    workspace_id: &str,
    wait: &mut dyn FnMut(Duration),
) -> Killed {
    let mut current = match look(runner, workspace_id) {
        Ok(holders) => holders,
        Err(why) => return Killed::Unavailable(HostCannot::ReadItsProcessTable(why)),
    };
    let mut signalled: Vec<Signalled> = Vec::new();
    let mut last_pass: Vec<HostProcess> = Vec::new();
    for (signal, grace, ending) in [
        (Signal::Terminate, UNWIND, Ending::Terminated),
        (Signal::Kill, REAP, Ending::Killed),
    ] {
        let orphans = orphaned(&current);
        let Some(pids) = NonEmpty::of(orphans.iter().map(|process| process.pid)) else {
            break;
        };
        if signals::signal(runner, signal, &pids) == Sent::NoKillOnThisHost {
            return Killed::Unavailable(HostCannot::SendASignal);
        }
        wait(grace);
        // A reading that failed where the first one succeeded says nothing about
        // what is left, and "nothing is left" is the one thing it must not be
        // read as — that is a report claiming a kill that never happened. The
        // last good reading stands instead.
        current = look(runner, workspace_id).unwrap_or(current);
        for process in &orphans {
            if !still_there(&current, process) {
                signalled.push(Signalled {
                    process: process.clone(),
                    ending,
                });
            }
        }
        last_pass = orphans;
    }
    // What the *last* pass signalled and did not shift, rather than everything
    // orphaned in the final reading: a process that reparented while the sweep
    // was running took neither signal, and "still running after SIGKILL" is a
    // sentence about a signal it never got. [`Holding`] is what counts it.
    for process in last_pass {
        if still_there(&current, &process) {
            signalled.push(Signalled {
                process,
                ending: Ending::Survived,
            });
        }
    }
    signalled.sort_by_key(|signalled| signalled.process.pid);
    // Every holder still standing, whatever its parentage: an orphan that sat
    // through SIGKILL is holding the workspace exactly as firmly as a live build
    // is, so both are reasons to leave the marker where it is.
    let holding = if current.is_empty() {
        Holding::Free
    } else {
        Holding::StillHeld
    };
    Killed::Swept(Sweep {
        signalled,
        attended: current
            .into_iter()
            .filter(|holder| holder.parentage == Parentage::Attended)
            .map(|holder| holder.process)
            .collect(),
        marker: sweep_marker(devpod_home, workspace_id, holding),
        containers: kill_containers(runner, workspace_id),
        holding,
    })
}

/// Kill whatever containers this workspace's compose project still has up.
///
/// Unconditional, where the marker's removal is not: a container is not a claim
/// about whether anything is building, so nothing here has to be true of the
/// process table first. The listing comes first because a project with nothing
/// running is the common case and a `docker kill` with no arguments is an error
/// rather than a no-op.
fn kill_containers(runner: &dyn Runner, workspace_id: &str) -> Containers {
    let ids = match docker::running_for_project(runner, workspace_id) {
        docker::Running::These(ids) => ids,
        docker::Running::NotInstalled => return Containers::NoDocker,
        docker::Running::Refused { exit, stderr } => {
            return Containers::Standing(ContainerRefusal::Refused { exit, stderr });
        }
        docker::Running::NotStarted(failure) => {
            return Containers::Standing(ContainerRefusal::NotRun { failure });
        }
    };
    let Some(ids) = NonEmpty::of(ids) else {
        return Containers::NoneRunning;
    };
    match docker::kill_containers(runner, &ids) {
        docker::Answer::Ran { exit, .. } if exit.is_success() => {
            Containers::Killed(ids.iter().cloned().collect())
        }
        docker::Answer::Ran { exit, stderr } => {
            Containers::Standing(ContainerRefusal::Refused { exit, stderr })
        }
        // A docker that listed containers and then went missing is not a machine
        // without docker on it, whatever the second spawn reports; but the fix is
        // still to look at the docker on this host, so it gets the same arm.
        docker::Answer::NotInstalled => Containers::NoDocker,
        docker::Answer::NotStarted(failure) => {
            Containers::Standing(ContainerRefusal::NotRun { failure })
        }
    }
}

/// Remove devpod's busy marker, but only once nothing holds the workspace.
///
/// The precondition is the whole of this function's judgement, and it arrives as
/// a [`Holding`] rather than being worked out here: the marker is stale exactly
/// when no process is left to remove it, which is the same fact the exit code and
/// the closing line are reading, taken from the same place.
fn sweep_marker(devpod_home: Option<&DevpodHome>, workspace_id: &str, holding: Holding) -> Marker {
    if holding == Holding::StillHeld {
        return Marker::LeftForALiveHolder;
    }
    let Some(path) = devpod_home::sole_busy_marker(devpod_home, workspace_id) else {
        return Marker::Unlocatable;
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Marker::Removed(path),
        // Already gone is the good ending, not a failure: a `devpod up` that took
        // SIGTERM ran the `defer` that removes it.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Marker::Absent,
        Err(error) => Marker::Unremovable {
            path,
            reason: system_words(&error),
        },
    }
}

/// Read the host's table and pick out what holds this workspace.
fn look(runner: &dyn Runner, workspace_id: &str) -> Result<Vec<Holder>, TableUnreadable> {
    match ps::processes(runner) {
        ps::Answer::Read(table) => Ok(holders(&table, workspace_id)),
        ps::Answer::NotInstalled => Err(TableUnreadable::NoPs),
        ps::Answer::Refused { exit, stderr } => Err(TableUnreadable::Refused { exit, stderr }),
        ps::Answer::NotStarted(failure) => Err(TableUnreadable::NotStarted(failure)),
    }
}

fn orphaned(holders: &[Holder]) -> Vec<HostProcess> {
    holders
        .iter()
        .filter(|holder| holder.parentage == Parentage::Orphaned)
        .map(|holder| holder.process.clone())
        .collect()
}

/// Whether this exact process is still in the reading.
///
/// The command line as well as the pid, because pids are reused: a `devpod up`
/// that died and a fresh process that inherited its number are the same row to a
/// pid comparison, and reporting the second as the first is a kill this never
/// made.
fn still_there(holders: &[Holder], process: &HostProcess) -> bool {
    holders.iter().any(|holder| {
        holder.process.pid == process.pid && holder.process.command == process.command
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use devlaunch_test_support::{FakeRunner, Response};

    use super::*;
    use crate::clients::devpod_home::{
        ScratchHome, devpod_home_with, sole_busy_marker, untouchable_flock,
    };

    /// The issue's own `ps` row, with the id shortened.
    fn orphaned_up() -> HostProcess {
        HostProcess {
            pid: 732_721,
            parent: 1,
            command: "devpod up my-ws --ide none --devcontainer-path .devcontainer/x.json"
                .to_owned(),
        }
    }

    fn init() -> HostProcess {
        HostProcess {
            pid: 1,
            parent: 0,
            command: "/sbin/init".to_owned(),
        }
    }

    /// The whole diagnosis of the issue, as a table: PPID 1, sleeping, no
    /// children, its `dl` long gone.
    #[test]
    fn an_init_reparented_devpod_naming_the_workspace_is_an_orphan() {
        let table = [init(), orphaned_up()];

        assert_eq!(
            holders(&table, "my-ws"),
            [Holder {
                process: orphaned_up(),
                parentage: Parentage::Orphaned,
            }]
        );
    }

    /// The launch somebody is sitting in front of. Its parent is a live `dl`, so
    /// it is somebody's build rather than a leftover, and killing it would take a
    /// working workspace away from whoever asked for it.
    #[test]
    fn a_devpod_whose_parent_is_still_running_is_attended_rather_than_orphaned() {
        let table = [
            init(),
            HostProcess {
                pid: 5000,
                parent: 1,
                command: "dl my-ws".to_owned(),
            },
            HostProcess {
                pid: 5001,
                parent: 5000,
                command: "devpod up my-ws --ide none".to_owned(),
            },
        ];

        assert_eq!(
            holders(&table, "my-ws")
                .iter()
                .map(|holder| holder.parentage)
                .collect::<Vec<Parentage>>(),
            [Parentage::Attended]
        );
    }

    /// Reparenting is not the only way to lose a parent: a pid whose parent is
    /// simply not in the table has nobody waiting on it either, and a host with a
    /// subreaper never shows PPID 1 at all.
    #[test]
    fn a_devpod_whose_parent_left_the_table_is_an_orphan() {
        let table = [
            init(),
            HostProcess {
                pid: 5001,
                parent: 4999,
                command: "devpod ssh my-ws".to_owned(),
            },
        ];

        assert_eq!(
            holders(&table, "my-ws")
                .iter()
                .map(|holder| holder.parentage)
                .collect::<Vec<Parentage>>(),
            [Parentage::Orphaned]
        );
    }

    /// The `dl` that is running this very command names the workspace in its own
    /// argv, and so does the agent that launched it. Only devpod holds devpod's
    /// lock, so only devpod is in scope — the alternative is a verb that kills
    /// the shell it was typed into.
    #[test]
    fn a_process_that_is_not_devpod_is_left_alone_however_it_names_the_workspace() {
        let table = [
            init(),
            HostProcess {
                pid: 5000,
                parent: 1,
                command: "dl my-ws kill".to_owned(),
            },
            HostProcess {
                pid: 5002,
                parent: 1,
                command: "ssh my-ws.devpod".to_owned(),
            },
        ];

        assert!(holders(&table, "my-ws").is_empty());
    }

    /// devpod is reached by path as often as by name, and a workspace does not
    /// stop being wedged because somebody typed the full path to the binary.
    #[test]
    fn devpod_is_recognised_by_the_name_at_the_end_of_its_path() {
        let table = [
            init(),
            HostProcess {
                pid: 5001,
                parent: 1,
                command: "/usr/local/bin/devpod up my-ws".to_owned(),
            },
        ];

        assert_eq!(holders(&table, "my-ws").len(), 1);
    }

    /// Workspace ids share prefixes by construction — the same repo on two
    /// branches differs in a suffix — so a substring match would sweep up a
    /// neighbour's live launch. The id has to be a whole word.
    #[test]
    fn a_workspace_whose_id_is_a_prefix_of_another_is_not_swept_up_with_it() {
        let table = [
            init(),
            HostProcess {
                pid: 5001,
                parent: 1,
                command: "devpod up my-ws-other --ide none".to_owned(),
            },
        ];

        assert!(holders(&table, "my-ws").is_empty());
    }

    /// devpod's own helpers name the workspace after an `=` rather than as a word
    /// of their own, and one of those holding the lock is the same wedge.
    #[test]
    fn an_id_given_as_a_flags_value_names_the_workspace_too() {
        let table = [
            init(),
            HostProcess {
                pid: 5001,
                parent: 1,
                command: "devpod helper ssh-server --workspace-id=my-ws".to_owned(),
            },
        ];

        assert_eq!(holders(&table, "my-ws").len(), 1);
    }

    // --------------------------------------------------- the escalation

    /// The issue's row, as `ps` prints it.
    const WEDGED: &str = "732721       1 devpod up my-ws --ide none\n";
    const NOTHING: &str = "    1       0 /sbin/init\n";

    /// A fake whose process table can be swapped between passes, which is what
    /// the injected wait is for: the grace period is where the process actually
    /// dies, so the test spends it the way the host would.
    fn host_showing(table: &str) -> FakeRunner {
        let fake = FakeRunner::new();
        showing(&fake, table);
        fake
    }

    fn showing(fake: &FakeRunner, table: &str) {
        fake.clear_scripts();
        fake.script(["ps"], Response::stdout(table.to_owned()));
    }

    fn showing_with_containers(fake: &FakeRunner, table: &str, containers: &str) {
        showing(fake, table);
        fake.script(["docker", "ps"], Response::stdout(containers.to_owned()));
    }

    fn swept(killed: Killed) -> Sweep {
        match killed {
            Killed::Swept(sweep) => sweep,
            other => panic!("expected a sweep, got {other:?}"),
        }
    }

    /// SIGTERM is an ask, and an orphan that takes it is never hit again. The
    /// second signal is not sent, which is the whole reason the first one is.
    #[test]
    fn an_orphan_that_stops_for_sigterm_is_never_sigkilled() {
        let fake = host_showing(WEDGED);
        let sweep = swept(workspace_kill(&fake, None, "my-ws", &mut |_| {
            showing(&fake, NOTHING);
        }));

        assert_eq!(
            fake.args_to("kill"),
            [["-TERM", "732721"]],
            "a process that stopped for SIGTERM must not also be SIGKILLed"
        );
        assert_eq!(
            sweep
                .signalled
                .iter()
                .map(|signalled| (signalled.process.pid, signalled.ending))
                .collect::<Vec<(u32, Ending)>>(),
            [(732_721, Ending::Terminated)]
        );
    }

    /// The escalation, and the reason SIGKILL is here at all: a `devpod up`
    /// blocked in a syscall never gets round to its handler.
    #[test]
    fn an_orphan_that_ignores_sigterm_is_sigkilled() {
        let fake = host_showing(WEDGED);
        let mut passes = 0;
        let sweep = swept(workspace_kill(&fake, None, "my-ws", &mut |_| {
            passes += 1;
            if passes == 2 {
                showing(&fake, NOTHING);
            }
        }));

        assert_eq!(
            fake.args_to("kill"),
            [["-TERM", "732721"], ["-KILL", "732721"]]
        );
        assert_eq!(
            sweep
                .signalled
                .iter()
                .map(|signalled| signalled.ending)
                .collect::<Vec<Ending>>(),
            [Ending::Killed]
        );
    }

    /// A pid that is still there after SIGKILL belongs to somebody else, and the
    /// report says so rather than claiming the workspace is free. Reporting it
    /// killed is what would send the next `dl up` back into the five-second loop
    /// with nothing left to try.
    #[test]
    fn a_process_that_outlives_sigkill_is_reported_rather_than_claimed() {
        let fake = host_showing(WEDGED);
        let sweep = swept(workspace_kill(&fake, None, "my-ws", &mut |_| {}));

        assert_eq!(
            sweep
                .signalled
                .iter()
                .map(|signalled| signalled.ending)
                .collect::<Vec<Ending>>(),
            [Ending::Survived]
        );
    }

    /// The one thing this verb must never do. A `devpod up` whose `dl` is still
    /// running is somebody's build, and it is holding the lock legitimately.
    #[test]
    fn a_build_somebody_is_still_watching_is_left_standing() {
        let fake = host_showing(
            "    1       0 /sbin/init\n 5000       1 dl my-ws\n 5001    5000 devpod up my-ws\n",
        );

        let sweep = swept(workspace_kill(&fake, None, "my-ws", &mut |_| {}));

        assert!(fake.args_to("kill").is_empty(), "nothing was signalled");
        assert_eq!(
            sweep
                .attended
                .iter()
                .map(|process| process.pid)
                .collect::<Vec<u32>>(),
            [5001]
        );
    }

    /// Nothing holding the workspace is a finding, not a failure: it says the
    /// hang is somewhere this verb does not reach, which is worth knowing.
    #[test]
    fn a_workspace_nothing_is_holding_sweeps_nothing_and_says_so() {
        let fake = host_showing(NOTHING);

        let sweep = swept(workspace_kill(&fake, None, "my-ws", &mut |_| {}));

        assert!(fake.args_to("kill").is_empty());
        assert!(sweep.signalled.is_empty());
        assert!(sweep.attended.is_empty());
    }

    /// Without a process table nothing here can be established, so nothing is
    /// done: every later step is conditioned on knowing what is running, and a
    /// marker removed on a guess tells devpod's daemon a live build has finished.
    #[test]
    fn a_host_whose_process_table_will_not_read_sweeps_nothing() {
        let fake = FakeRunner::new();
        fake.script_missing("ps");

        assert_eq!(
            workspace_kill(&fake, None, "my-ws", &mut |_| {}),
            Killed::Unavailable(HostCannot::ReadItsProcessTable(TableUnreadable::NoPs))
        );
        assert!(fake.args_to("kill").is_empty());
    }

    /// A host with no `kill` cannot be unwedged by this verb, and saying so beats
    /// reporting a sweep that signalled nothing.
    #[test]
    fn a_host_with_nothing_to_signal_with_says_so_rather_than_sweeping() {
        let fake = host_showing(WEDGED);
        fake.script_missing("kill");

        assert_eq!(
            workspace_kill(&fake, None, "my-ws", &mut |_| {}),
            Killed::Unavailable(HostCannot::SendASignal)
        );
    }

    // --------------------------------------------------- the busy marker

    fn home_for(workspace_id: &str) -> ScratchHome {
        devpod_home_with(&[("default", workspace_id, Some(()))])
    }

    /// The stale marker a SIGKILLed `up` leaves behind, put where devpod's agent
    /// would have put it.
    fn write_marker(home: &DevpodHome, workspace_id: &str) -> PathBuf {
        let path = sole_busy_marker(Some(home), workspace_id).expect("a marker path");
        std::fs::create_dir_all(path.parent().expect("a marker directory"))
            .expect("a marker directory");
        std::fs::write(&path, "").expect("a marker");
        path
    }

    /// The file a `defer` was going to remove and a SIGKILL never let it. Once
    /// nothing is left holding the workspace it is stale by definition, and
    /// devpod's daemon reads it as a build still running.
    #[test]
    fn the_busy_marker_goes_once_nothing_is_left_building_the_workspace() {
        let home = home_for("my-ws");
        let marker = write_marker(&home, "my-ws");
        let fake = host_showing(WEDGED);

        let sweep = swept(workspace_kill(&fake, Some(&home), "my-ws", &mut |_| {
            showing(&fake, NOTHING);
        }));

        assert_eq!(sweep.marker, Marker::Removed(marker.clone()));
        assert!(!marker.exists(), "the stale marker is gone");
    }

    /// The condition the removal hangs on. A marker taken out from under a live
    /// build tells devpod's daemon that build has finished, which is a worse
    /// state than the one this verb was reached for.
    #[test]
    fn the_busy_marker_stays_while_something_is_still_holding_the_workspace() {
        let home = home_for("my-ws");
        let marker = write_marker(&home, "my-ws");
        let fake = host_showing(
            "    1       0 /sbin/init\n 5000       1 dl my-ws\n 5001    5000 devpod up my-ws\n",
        );

        let sweep = swept(workspace_kill(&fake, Some(&home), "my-ws", &mut |_| {}));

        assert_eq!(sweep.marker, Marker::LeftForALiveHolder);
        assert!(marker.exists(), "somebody's build still owns this marker");
    }

    /// An orphan that sat through both signals is still holding the workspace,
    /// however much it was signalled, so the marker is not stale yet either.
    #[test]
    fn the_busy_marker_stays_when_an_orphan_outlived_both_signals() {
        let home = home_for("my-ws");
        let marker = write_marker(&home, "my-ws");
        let fake = host_showing(WEDGED);

        let sweep = swept(workspace_kill(&fake, Some(&home), "my-ws", &mut |_| {}));

        assert_eq!(sweep.marker, Marker::LeftForALiveHolder);
        assert!(marker.exists());
    }

    /// A SIGTERM the process took ran its `defer`, so the marker is already gone
    /// and there was nothing to sweep. Reported apart from a removal, because
    /// "removed a stale marker" and "found none" say different things about how
    /// the process died.
    #[test]
    fn a_marker_that_was_never_left_behind_is_not_a_removal() {
        let home = home_for("my-ws");
        let fake = host_showing(WEDGED);

        let sweep = swept(workspace_kill(&fake, Some(&home), "my-ws", &mut |_| {
            showing(&fake, NOTHING);
        }));

        assert_eq!(sweep.marker, Marker::Absent);
    }

    /// The hazard this verb is built around avoiding. Unlinking an flock'd file
    /// leaves its holder holding an inode nobody else can see while the next
    /// caller locks a fresh one, and then two processes both believe they hold
    /// the workspace. Killing the holder is what releases it; the file stays.
    #[test]
    fn the_flock_devpod_blocks_on_is_never_unlinked() {
        let home = home_for("my-ws");
        let flock = untouchable_flock(&home, "default", "my-ws");
        write_marker(&home, "my-ws");
        let fake = host_showing(WEDGED);

        swept(workspace_kill(&fake, Some(&home), "my-ws", &mut |_| {
            showing(&fake, NOTHING);
        }));

        assert!(
            flock.exists(),
            "the kill releases devpod's flock; unlinking it is what must never happen"
        );
    }

    /// A host whose devpod records cannot say which context holds this workspace
    /// has no marker to name, and guessing at one is deleting a file belonging to
    /// a workspace nobody asked about.
    #[test]
    fn a_workspace_with_no_addressable_marker_says_so_rather_than_guessing() {
        let fake = host_showing(WEDGED);

        let sweep = swept(workspace_kill(&fake, None, "my-ws", &mut |_| {
            showing(&fake, NOTHING);
        }));

        assert_eq!(sweep.marker, Marker::Unlocatable);
    }

    // --------------------------------------------------- the containers

    /// A container the workspace still has up is killed, and the report names it.
    /// A silent hammer is worse than the hang it was reached for.
    #[test]
    fn a_container_the_workspace_still_has_up_is_killed_and_named() {
        let fake = host_showing(WEDGED);
        fake.script(["docker", "ps"], Response::stdout("abc123\n"));

        let sweep = swept(workspace_kill(&fake, None, "my-ws", &mut |_| {
            showing_with_containers(&fake, NOTHING, "abc123\n");
        }));

        assert_eq!(
            sweep.containers,
            Containers::Killed(vec!["abc123".to_owned()])
        );
        assert_eq!(
            fake.args_to("docker").last(),
            Some(&vec!["kill".to_owned(), "abc123".to_owned()])
        );
    }

    /// The issue's own case: the container had exited 137 a minute before the
    /// restart, so a `docker kill` would have found nothing to kill. Nothing to
    /// kill is a finding, and it points at the process rather than the container.
    #[test]
    fn a_workspace_whose_container_already_died_kills_no_container() {
        let fake = host_showing(WEDGED);
        fake.script(["docker", "ps"], Response::stdout(""));

        let sweep = swept(workspace_kill(&fake, None, "my-ws", &mut |_| {}));

        assert_eq!(sweep.containers, Containers::NoneRunning);
        assert_eq!(
            fake.args_to("docker").len(),
            1,
            "nothing to kill means no `docker kill` at all"
        );
    }

    /// Silent on a host with no docker, for the reason the volume sweep is: a
    /// machine with no docker started no containers.
    #[test]
    fn a_machine_with_no_docker_is_not_a_machine_with_containers_left_running() {
        let fake = host_showing(WEDGED);
        fake.script_missing("docker");

        let sweep = swept(workspace_kill(&fake, None, "my-ws", &mut |_| {}));

        assert_eq!(sweep.containers, Containers::NoDocker);
    }

    /// **One fact, read once.** Whether the workspace is free is what the exit
    /// code means, what the marker's removal hangs on, and what the closing line
    /// says, and all three are the same question about the *last* reading. A
    /// holder that arrived while the sweep ran was signalled by neither pass and
    /// appears in neither list, so a verdict assembled from the lists would call
    /// a held workspace free.
    #[test]
    fn a_holder_that_arrived_while_the_sweep_ran_still_leaves_the_workspace_held() {
        let fake = host_showing(WEDGED);
        let mut passes = 0;
        let sweep = swept(workspace_kill(&fake, None, "my-ws", &mut |_| {
            passes += 1;
            if passes == 2 {
                // A second `devpod` reparented after both signals had gone out.
                showing(&fake, &format!("{WEDGED}  999       1 devpod ssh my-ws\n"));
            }
        }));

        assert_eq!(sweep.holding, Holding::StillHeld);
        assert_eq!(
            sweep
                .signalled
                .iter()
                .map(|signalled| (signalled.process.pid, signalled.ending))
                .collect::<Vec<(u32, Ending)>>(),
            [(732_721, Ending::Survived)],
            "a process the sweep never signalled is not one it reports a signal for"
        );
    }
}
