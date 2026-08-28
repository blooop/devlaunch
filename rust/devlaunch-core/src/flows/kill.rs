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
/// A record rather than two lists, because the two questions asked of it are
/// asked of the same reading: what to signal is the orphans, and what to report
/// as somebody's live build is the holders with a parent still behind them. Were
/// it split into two vectors at the point of classification, one table read once
/// could be filtered twice by rules that had drifted apart, and a process could
/// end up in both or in neither.
///
/// Neither list is the complement of the other once the sweep has run, and that
/// is not a leak in the record: an orphan that outlived SIGKILL is reported as a
/// [`Signalled`] and is in nobody's attended list, while still holding the
/// workspace exactly as firmly. [`Holding`] is what counts it.
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
///
/// **Any devpod subcommand**, where devlaunch#484 names three (`up`, `ssh`,
/// `helper`). Deliberately wider than the issue, because the thing that wedges a
/// workspace is devpod's `flock` and *every* subcommand that addresses a
/// workspace takes it: an orphaned `devpod delete my-ws` blocks the next launch
/// exactly as an orphaned `up` does, and a filter that named three subcommands
/// would leave the reader of the wedge no verb at all. Nothing is killed for
/// being a `delete` in any case — it is killed for having no parent, and an
/// operation nobody is waiting on has already lost whatever it was mid-way
/// through.
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
    let reparented = process.parent == 1;
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

/// How long the host is given after `signal` before the table is read again.
///
/// A function of the signal rather than a column of a table the loop iterates,
/// which is [`Ending::under`]'s reason: the pairing is total, and a triple of
/// signal, grace and ending makes `(Signal::Kill, UNWIND, Ending::Terminated)`
/// writable — a SIGKILL reported as a SIGTERM, waited on for ten times as long.
///
/// **SIGTERM: two seconds.** A `devpod up` that takes the signal drops the flock
/// as it goes, and a second or two is the difference between a clean unwind and a
/// SIGKILL that leaves the busy marker behind for this flow to sweep. Not longer,
/// because the person typing this has already waited through the five-second loop
/// that sent them here.
///
/// **SIGKILL: a fifth of one**, and not a grace period at all — nothing runs
/// after SIGKILL. The kernel still has to tear the process down and the parent
/// still has to reap it, and a table read in the same instant can still show it.
fn grace(signal: Signal) -> Duration {
    match signal {
        Signal::Terminate => Duration::from_secs(2),
        Signal::Kill => Duration::from_millis(200),
    }
}

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

impl Ending {
    /// What a process that stopped holding the workspace under `signal` ended as.
    ///
    /// [`Ending::Survived`] is not reachable from here on purpose: it is not a
    /// fact about a signal but about a process that took both and is still there,
    /// so it is written once, where the escalation runs out.
    fn under(signal: Signal) -> Self {
        match signal {
            Signal::Terminate => Self::Terminated,
            Signal::Kill => Self::Killed,
        }
    }
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
    /// Orphans were found and nothing on this host would signal them.
    SendASignal(NoSignal),
}

/// Why the signals could not be sent.
///
/// Two arms rather than one, for [`TableUnreadable`]'s reason: only the first is
/// a sentence about the machine being unusual, and a reader told this host has no
/// `kill` on it goes looking for a program that is already there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoSignal {
    /// No `kill(1)` on this host, or one that could not be exec'd.
    NoKillHere,
    /// `kill` is here and would not run. `failure` is the OS's own reason.
    NotRun(OsFailure),
}

/// What became of devpod's busy marker for this workspace.
///
/// Five arms and not a bool, because "gone" is two of them and the other three
/// are three different things to say. Two mean the file is still there —
/// [`Marker::LeftForALiveHolder`] on purpose and [`Marker::Unremovable`] against
/// its will — one means it was never there to begin with, and one means nobody
/// could say where to look. The one that matters is `LeftForALiveHolder`: a
/// marker removed out from under a live build tells devpod's daemon that build
/// has finished, which is a worse state than the wedge this verb was reached for.
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
    /// docker was not asked at all: somebody's build is still running, and its
    /// containers are the build's. The sweep leaves that `devpod up` standing
    /// (see [`Holding::StillHeld`]), and killing what it is building underneath it
    /// would break it just as surely — with the additional insult of not saying it
    /// was a build that was broken.
    LeftForALiveBuild,
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
/// assembled by each of its readers from the lists in [`Sweep`]. It is what the
/// exit code means, what the busy marker's removal hangs on, whether the
/// containers are the sweep's to kill, and what the closing line of the report
/// says, and four derivations of one question are how those four come to
/// disagree — a holder that arrived while the sweep ran is in no list and holds
/// the workspace all the same.
///
/// **The live builds hang off `StillHeld` rather than sitting beside it**, which
/// is that same argument taken one step further. `Sweep { attended: vec![p],
/// holding: Holding::Free }` was constructible, and it is a nonsense: a process
/// with a parent behind it is a holder, so an attended build is *why* the
/// workspace is held. Hanging the vector off the arm makes the contradiction
/// unwritable rather than merely unwritten, which is what the paragraph above
/// was asking for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Holding {
    /// Nothing on this host names this workspace any more.
    Free,
    /// Something still does. `attended` is the live builds among them — reported
    /// rather than passed over in silence, because "it killed nothing" and "it
    /// found a build and left it alone" send the reader to two different places.
    /// Empty means what still holds the workspace took both signals and stayed,
    /// or arrived after they went out.
    StillHeld { attended: Vec<HostProcess> },
}

impl Holding {
    /// The live builds this sweep left standing, of which there are none on a
    /// workspace nothing is holding.
    pub fn attended(&self) -> &[HostProcess] {
        match self {
            Self::Free => &[],
            Self::StillHeld { attended } => attended,
        }
    }
}

/// What one sweep found and what it did about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sweep {
    /// The orphans, in pid order, and how each one ended. Empty is a finding:
    /// nothing on this host is holding the workspace, so the hang is somewhere
    /// this verb does not reach.
    pub signalled: Vec<Signalled>,
    /// What became of devpod's busy marker.
    pub marker: Marker,
    /// What became of the containers the workspace still had running.
    pub containers: Containers,
    /// Whether anything is holding the workspace now, and which builds those are.
    pub holding: Holding,
}

impl Sweep {
    /// Whether anything took every signal this sweep had and is still holding on.
    ///
    /// The one question a removal standing after the sweep has to ask, and the
    /// reason it is asked of the *signals* rather than of [`Holding`]: the two
    /// kinds of holder left standing are in devpod's way to completely different
    /// degrees. An attended one is spared on purpose and is not in its way at all
    /// — an idle `devpod ssh` takes the workspace's flock and gives it back, which
    /// is why `dl <ws> rm` deletes a workspace somebody is sitting in without ever
    /// noticing them. A process that sat through SIGKILL is the other case
    /// entirely: it holds the flock, there is no privilege left to take it away,
    /// and devpod's acquire has no deadline behind it. A delete attempted over one
    /// of those does not fail. It joins the five-second log line that sent
    /// somebody here in the first place, which is the one ending this verb exists
    /// to spare them.
    pub fn outlived_the_signals(&self) -> bool {
        self.signalled
            .iter()
            .any(|signalled| signalled.ending == Ending::Survived)
    }
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
    // The set the escalation carries forward, and it is *narrowed* between the
    // passes rather than recomputed from the newest reading. Recomputing it is how
    // a process that was attended when the sweep opened and lost its parent to the
    // SIGTERM would take a SIGKILL as its first signal — which is not what
    // devlaunch#484 asks for ("SIGTERM, then SIGKILL"), and is the one process on
    // the table with the best claim to being asked first.
    let mut remaining = orphaned(&current);
    for signal in [Signal::Terminate, Signal::Kill] {
        let Some(pids) = NonEmpty::of(remaining.iter().map(|process| process.pid)) else {
            break;
        };
        match signals::signal(runner, signal, &pids) {
            Sent::Attempted => {}
            Sent::NoKillHere => {
                return Killed::Unavailable(HostCannot::SendASignal(NoSignal::NoKillHere));
            }
            Sent::NotRun(failure) => {
                return Killed::Unavailable(HostCannot::SendASignal(NoSignal::NotRun(failure)));
            }
        }
        wait(grace(signal));
        // A reading that failed where the first one succeeded says nothing about
        // what is left, and "nothing is left" is the one thing it must not be
        // read as — that is a report claiming a kill that never happened. The
        // last good reading stands instead.
        current = look(runner, workspace_id).unwrap_or(current);
        let (survivors, gone): (Vec<HostProcess>, Vec<HostProcess>) = remaining
            .into_iter()
            .partition(|process| still_there(&current, process));
        signalled.extend(gone.into_iter().map(|process| Signalled {
            process,
            ending: Ending::under(signal),
        }));
        remaining = survivors;
    }
    // Whatever sat through every signal it was sent. Not everything orphaned in
    // the final reading: a process that reparented while the sweep was running
    // took no signal at all, and "still running after SIGKILL" is a sentence about
    // one it never got. [`Holding`] is what counts that one.
    signalled.extend(remaining.into_iter().map(|process| Signalled {
        process,
        ending: Ending::Survived,
    }));
    signalled.sort_by_key(|signalled| signalled.process.pid);
    // Every holder still standing, whatever its parentage: an orphan that sat
    // through SIGKILL is holding the workspace exactly as firmly as a live build
    // is, so both are reasons to leave the marker where it is.
    let holding = if current.is_empty() {
        Holding::Free
    } else {
        Holding::StillHeld {
            attended: current
                .into_iter()
                .filter(|holder| holder.parentage == Parentage::Attended)
                .map(|holder| holder.process)
                .collect(),
        }
    };
    Killed::Swept(Sweep {
        signalled,
        marker: sweep_marker(devpod_home, workspace_id, &holding),
        containers: kill_containers(runner, workspace_id, &holding),
        holding,
    })
}

/// Kill whatever containers this workspace's compose project still has up.
///
/// **Unless somebody is building it**, which is the one condition this shares
/// with the marker's removal, and it is there because the alternative contradicts
/// the sweep standing right above it: a `devpod up` whose `dl` is still running is
/// left alone deliberately, and killing the containers it is in the middle of
/// creating breaks that build just as effectively as signalling it would have. An
/// orphan that outlived SIGKILL is *not* that case and its containers are killed:
/// nothing is waiting on it, and the containers are as stale as it is.
///
/// The listing comes first because a project with nothing running is the common
/// case and a `docker kill` with no arguments is an error rather than a no-op.
fn kill_containers(runner: &dyn Runner, workspace_id: &str, holding: &Holding) -> Containers {
    if !holding.attended().is_empty() {
        return Containers::LeftForALiveBuild;
    }
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
///
/// The unlink itself is [`devpod_home::remove_busy_marker`]'s, because the file
/// is devpod's: that module owns devpod's layout on the way out as well as the
/// way in, and it is where the *other* `workspace.lock` — the flock, which must
/// never be unlinked — is named alongside this one. What is left here is the
/// judgement, which is this flow's.
fn sweep_marker(devpod_home: Option<&DevpodHome>, workspace_id: &str, holding: &Holding) -> Marker {
    if matches!(holding, Holding::StillHeld { .. }) {
        return Marker::LeftForALiveHolder;
    }
    match devpod_home::remove_busy_marker(devpod_home, workspace_id) {
        devpod_home::MarkerRemoval::Removed(path) => Marker::Removed(path),
        devpod_home::MarkerRemoval::AlreadyGone => Marker::Absent,
        devpod_home::MarkerRemoval::Refused { path, reason } => {
            Marker::Unremovable { path, reason }
        }
        devpod_home::MarkerRemoval::Unlocatable => Marker::Unlocatable,
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
                .holding
                .attended()
                .iter()
                .map(|process| process.pid)
                .collect::<Vec<u32>>(),
            [5001]
        );
    }

    /// The question the delete standing after the sweep asks, and the two holders
    /// it separates. Both leave the workspace [`Holding::StillHeld`], and asking
    /// *that* instead is the bug this exists to avoid: one live `devpod ssh` was
    /// enough to make the whole verb do nothing, on a workspace `dl <ws> rm` then
    /// deleted without noticing the ssh at all.
    #[test]
    fn only_a_process_that_outlived_the_signals_stands_in_a_deletes_way() {
        let outlived = swept(workspace_kill(
            &host_showing(WEDGED),
            None,
            "my-ws",
            &mut |_| {},
        ));
        let attended = swept(workspace_kill(
            &host_showing(
                "    1       0 /sbin/init\n 5000       1 dl my-ws\n 5001    5000 devpod ssh my-ws\n",
            ),
            None,
            "my-ws",
            &mut |_| {},
        ));

        assert!(matches!(outlived.holding, Holding::StillHeld { .. }));
        assert!(matches!(attended.holding, Holding::StillHeld { .. }));
        assert!(outlived.outlived_the_signals());
        assert!(!attended.outlived_the_signals());
    }

    /// Nothing holding the workspace is a finding, not a failure: it says the
    /// hang is somewhere this verb does not reach, which is worth knowing.
    #[test]
    fn a_workspace_nothing_is_holding_sweeps_nothing_and_says_so() {
        let fake = host_showing(NOTHING);

        let sweep = swept(workspace_kill(&fake, None, "my-ws", &mut |_| {}));

        assert!(fake.args_to("kill").is_empty());
        assert!(sweep.signalled.is_empty());
        assert!(sweep.holding.attended().is_empty());
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
            Killed::Unavailable(HostCannot::SendASignal(NoSignal::NoKillHere))
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

    /// A marker that will not go is the one ending the report has to spell out:
    /// the workspace is free, so the next launch will run, and devpod's daemon
    /// still has a file saying a build is in progress. Nothing else in the sweep
    /// is affected by it, which is why it is a line rather than a refusal.
    ///
    /// The marker here is a *directory*, which `unlink` refuses whoever is running
    /// the test — a permission bit would prove nothing on a runner that is root.
    #[test]
    fn a_marker_that_will_not_go_is_reported_and_stops_nothing_else() {
        let home = home_for("my-ws");
        let marker = sole_busy_marker(Some(&home), "my-ws").expect("a marker path");
        std::fs::create_dir_all(&marker).expect("a marker that will not unlink");
        let fake = host_showing(WEDGED);

        let sweep = swept(workspace_kill(&fake, Some(&home), "my-ws", &mut |_| {
            showing(&fake, NOTHING);
        }));

        match sweep.marker {
            Marker::Unremovable { path, reason } => {
                assert_eq!(path, marker);
                assert!(!reason.is_empty(), "the OS's own words");
            }
            other => panic!("expected a marker that would not go, got {other:?}"),
        }
        assert_eq!(
            sweep.holding,
            Holding::Free,
            "the workspace is free either way"
        );
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

    /// The containers of a build somebody is still watching are that build's, and
    /// the sweep that leaves the `devpod up` standing has to leave them standing
    /// too: killing what a build is in the middle of creating breaks it exactly as
    /// surely as signalling it would have, and docker is not even asked.
    #[test]
    fn the_containers_of_a_live_build_are_left_alone_with_the_build() {
        let fake = host_showing(
            "    1       0 /sbin/init\n 5000       1 dl my-ws\n 5001    5000 devpod up my-ws\n",
        );
        fake.script(["docker", "ps"], Response::stdout("abc123\n"));

        let sweep = swept(workspace_kill(&fake, None, "my-ws", &mut |_| {}));

        assert_eq!(sweep.containers, Containers::LeftForALiveBuild);
        assert!(
            fake.args_to("docker").is_empty(),
            "docker was not asked at all"
        );
    }

    /// An orphan that outlived SIGKILL holds the workspace, but nobody is waiting
    /// on it, so its containers are as stale as it is and are killed. The guard
    /// above is about a *build*, not about the workspace being held.
    #[test]
    fn the_containers_of_an_orphan_that_outlived_sigkill_are_still_killed() {
        let fake = host_showing(WEDGED);
        fake.script(["docker", "ps"], Response::stdout("abc123\n"));

        let sweep = swept(workspace_kill(&fake, None, "my-ws", &mut |_| {}));

        assert_eq!(
            sweep.containers,
            Containers::Killed(vec!["abc123".to_owned()])
        );
    }

    /// A docker that refuses the kill leaves the containers standing, and the
    /// report carries docker's own words: the sweep did what it could and the rest
    /// is somebody's to look at.
    #[test]
    fn a_docker_that_refused_the_kill_leaves_the_containers_standing() {
        let fake = host_showing(WEDGED);
        fake.script(["docker", "ps"], Response::stdout("abc123\n"));
        fake.script(
            ["docker", "kill"],
            Response::failed(1, "Error response from daemon: cannot kill\n"),
        );

        let sweep = swept(workspace_kill(&fake, None, "my-ws", &mut |_| {}));

        assert_eq!(
            sweep.containers,
            Containers::Standing(ContainerRefusal::Refused {
                exit: Exit::Code(1),
                stderr: "Error response from daemon: cannot kill\n".to_owned(),
            })
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

        assert_eq!(
            sweep.holding,
            Holding::StillHeld {
                attended: Vec::new()
            }
        );
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

    /// devlaunch#484 asks for "SIGTERM, then SIGKILL", and that has to hold for a
    /// process that *became* an orphan during the sweep: a `devpod helper` whose
    /// parent took the SIGTERM is reparented to init a moment later, and the pass
    /// that follows must not open on it with the signal nothing catches. The set
    /// the escalation carries is narrowed between passes rather than re-derived
    /// from the newest reading, which is what makes that true.
    #[test]
    fn a_process_orphaned_by_the_first_pass_is_not_sigkilled_without_being_asked() {
        let held = "  400       1 devpod up my-ws\n  401     400 devpod helper my-ws\n";
        let fake = host_showing(held);
        let mut passes = 0;
        let sweep = swept(workspace_kill(&fake, None, "my-ws", &mut |_| {
            passes += 1;
            if passes == 1 {
                // The parent took SIGTERM; its helper is now init's.
                showing(&fake, "  401       1 devpod helper my-ws\n");
            }
        }));

        assert_eq!(
            fake.args_to("kill"),
            [["-TERM", "400"]],
            "the helper the SIGTERM orphaned took no signal, and certainly not SIGKILL first"
        );
        assert_eq!(
            sweep
                .signalled
                .iter()
                .map(|signalled| (signalled.process.pid, signalled.ending))
                .collect::<Vec<(u32, Ending)>>(),
            [(400, Ending::Terminated)]
        );
    }

    /// A `kill` that is on this host and would not run is not a host with no
    /// `kill` on it. Two arms, because the sentence the binary writes for the
    /// first sends its reader looking for a program that is already there.
    #[test]
    fn a_kill_that_would_not_run_is_not_reported_as_a_host_without_one() {
        let fake = host_showing(WEDGED);
        fake.script(
            ["kill"],
            Response::NotStarted(OsFailure {
                kind: std::io::ErrorKind::PermissionDenied,
                errno: None,
            }),
        );

        assert_eq!(
            workspace_kill(&fake, None, "my-ws", &mut |_| {}),
            Killed::Unavailable(HostCannot::SendASignal(NoSignal::NotRun(OsFailure {
                kind: std::io::ErrorKind::PermissionDenied,
                errno: None,
            })))
        );
    }
}
