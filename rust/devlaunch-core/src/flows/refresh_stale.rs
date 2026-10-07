//! Which stale workspaces `dl --refresh-stale` may recreate now, and why it
//! leaves each of the others alone (devlaunch#673, part 3).
//!
//! [`crate::flows::stale_images`] says which containers run an older image than
//! their reference names. A recreate fixes that and ends every process in the
//! container. [`crate::flows::agent_sessions`] brings the agents back, so what is
//! left to lose is work in flight: an agent in the middle of a turn, a command
//! an agent started, a build. This module decides, per stale workspace, whether
//! there is any. It decides and does nothing; the binary runs the recreates.
//!
//! # The rule
//!
//! A stale workspace is refreshed only when every one of these holds, checked in
//! this order, and the first that fails is the reason it is skipped:
//!
//! 1. **No `dl` is launching it.** Its launch lock
//!    ([`crate::flows::launch_locks`]) is free.
//! 2. **Its processes can be read.** One `docker exec` as root reads
//!    `/proc/<pid>/stat` and `/proc/<pid>/cmdline` for every process. A
//!    container that does not answer (stopped, gone, no `sh`) is skipped: no
//!    reading is no grounds to recreate.
//! 3. **No build runs in it.** A process whose name (`comm`) is one of
//!    [`BUILD_PROGRAMS`] is a build. The list is short on purpose: compilers,
//!    linkers and the build drivers seen on the hosts this was made for.
//! 4. **No agent runs a command in it.** An agent is a process of any agent dl
//!    knows by name ([`herdr::agent_named`]): Claude, codex, gemini. One whose
//!    child is a shell is running a tool call or a background task: the agents
//!    run both through a shell. Claude's MCP servers are started directly, not
//!    through a shell, so they do not count.
//! 5. **Every agent session in it can be brought back, and is idle.** With no
//!    agent process there is nothing to bring back and this holds. Otherwise dl
//!    has to see the sessions, which takes a herdr pane: outside herdr it skips.
//!    herdr has to answer, every pane in the workspace with a live agent has to
//!    have a saved line ([`agent_sessions::HeldSessions::unresumable`]), each
//!    held agent has to be `idle` or `done`, and there must be no more agent
//!    sessions in the container than herdr's panes hold. Both sides of that count
//!    are agents of any kind. A session started from
//!    a terminal outside herdr would otherwise end with nothing to start it again.
//!
//! The check is a moment's reading, and a recreate starts a moment later. A
//! session that starts working in between is ended like any recreate ends it.

use std::path::Path;

use crate::clients::docker;
use crate::clients::herdr;
use crate::domain::locks;
use crate::flows::agent_sessions;
use crate::flows::launch_locks::LaunchLocks;
use crate::flows::stale_images::StaleImages;
use crate::runner::Runner;

/// The process names that mean a build runs, compared with the kernel's `comm`
/// (the first 15 bytes of the program's file name).
pub const BUILD_PROGRAMS: &[&str] = &[
    "bazel", "bazelisk", "cargo", "rustc", "cc1", "cc1plus", "make", "gmake", "ninja", "cmake",
    "pixi", "colcon", "gcc", "g++", "clang", "clang++", "ld", "ld.lld", "ld.gold", "collect2",
];

/// The shells an agent runs its tool calls and background tasks through.
const SHELLS: &[&str] = &["sh", "bash", "dash", "zsh", "fish"];

/// What `dl --refresh-stale` will do with one stale workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub workspace_id: String,
    /// The reference that now names a newer image.
    pub reference: String,
    pub verdict: Verdict,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Recreate it, and start its held agents again.
    Refresh,
    /// Leave it alone, for this reason.
    Skip(Skip),
}

/// Why a stale workspace is left alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Skip {
    /// Another `dl` holds its launch lock.
    LaunchUnderWay,
    /// Its launch lock could not be asked.
    LockUnreadable { why: String },
    /// Its container's processes could not be read.
    ProcessesUnread { why: String },
    /// A build runs in it.
    Building { program: String, pid: u32 },
    /// An agent runs a command in it.
    AgentRunsACommand { program: String, pid: u32 },
    /// An agent runs in it and this `dl` is not in a herdr pane, so it cannot see
    /// the sessions to bring them back.
    SessionsUnseen,
    /// herdr did not say which panes it has.
    HerdrUnanswered,
    /// This pane holds a live agent in the workspace and no line to start it again.
    Unresumable { pane_id: String },
    /// The agent in this pane is not idle.
    AgentBusy { pane_id: String, state: AgentState },
    /// More agent sessions run in the container than herdr's panes hold.
    SessionsOutsidePanes { running: usize, held: usize },
}

/// What a held agent is doing, when it is not something a refresh may end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    Working,
    /// Waiting on a permission prompt.
    Blocked,
    /// herdr said a state this build does not know.
    Unknown,
    /// herdr did not answer for the pane.
    Unanswered,
}

/// What to do with each stale workspace among `workspace_ids`, in their order.
///
/// A workspace `stale` does not name is not stale and is left out. `manager` is
/// the herdr this `dl` runs in, `None` outside one; `read` reads herdr's saved
/// session file, as [`agent_sessions::collect`] takes it.
pub fn plan<'w>(
    runner: &dyn Runner,
    stale: &StaleImages,
    workspace_ids: impl IntoIterator<Item = &'w str>,
    manager: Option<&agent_sessions::Manager>,
    launch_locks: &LaunchLocks,
    read: &dyn Fn(&Path) -> Option<String>,
) -> Vec<Planned> {
    workspace_ids
        .into_iter()
        .filter_map(|workspace_id| {
            let image = stale.of(workspace_id)?;
            let verdict = match judge(
                runner,
                workspace_id,
                image.container(),
                manager,
                launch_locks,
                read,
            ) {
                Some(skip) => Verdict::Skip(skip),
                None => Verdict::Refresh,
            };
            Some(Planned {
                workspace_id: workspace_id.to_owned(),
                reference: image.reference().to_owned(),
                verdict,
            })
        })
        .collect()
}

/// The first rule this workspace fails, or `None` when it may be refreshed.
fn judge(
    runner: &dyn Runner,
    workspace_id: &str,
    container: &str,
    manager: Option<&agent_sessions::Manager>,
    launch_locks: &LaunchLocks,
    read: &dyn Fn(&Path) -> Option<String>,
) -> Option<Skip> {
    match locks::run_if_lock_free(&launch_locks.path_for(workspace_id), || ()) {
        Ok(Some(())) => {}
        Ok(None) => return Some(Skip::LaunchUnderWay),
        Err(error) => {
            return Some(Skip::LockUnreadable {
                why: crate::flows::launch::lock_reason(&error),
            });
        }
    }
    let table = match docker::exec_as_root(runner, container, &["sh", "-c", READ_THE_TABLE])
        .and_then(|out| process_table(&out))
    {
        Ok(table) => table,
        Err(why) => return Some(Skip::ProcessesUnread { why }),
    };
    if let Some(build) = table
        .iter()
        .find(|process| BUILD_PROGRAMS.contains(&process.comm.as_str()))
    {
        return Some(Skip::Building {
            program: build.comm.clone(),
            pid: build.pid,
        });
    }
    let is_agent = |pid: u32| {
        table
            .iter()
            .any(|process| process.pid == pid && process.is_agent())
    };
    if let Some(command) = table
        .iter()
        .find(|process| SHELLS.contains(&process.comm.as_str()) && is_agent(process.ppid))
    {
        return Some(Skip::AgentRunsACommand {
            program: command.comm.clone(),
            pid: command.pid,
        });
    }
    let running = table
        .iter()
        .filter(|process| process.is_agent() && !is_agent(process.ppid))
        .count();
    if running == 0 {
        return None;
    }
    sessions_skip(runner, workspace_id, running, manager, read)
}

/// Rule 5: the agent sessions in the workspace, as herdr's panes hold them.
fn sessions_skip(
    runner: &dyn Runner,
    workspace_id: &str,
    running: usize,
    manager: Option<&agent_sessions::Manager>,
    read: &dyn Fn(&Path) -> Option<String>,
) -> Option<Skip> {
    let Some(manager) = manager else {
        return Some(Skip::SessionsUnseen);
    };
    let Some(held) = agent_sessions::collect(runner, manager, workspace_id, read) else {
        return Some(Skip::HerdrUnanswered);
    };
    if let Some(pane) = held.unresumable.first() {
        return Some(Skip::Unresumable {
            pane_id: pane.pane_id.clone(),
        });
    }
    for session in &held.sessions {
        let state = match agent_sessions::agent_reading(runner, manager, &session.pane_id) {
            herdr::AgentReading::Agent(agent) => match agent.status {
                herdr::AgentStatus::Idle | herdr::AgentStatus::Done => continue,
                herdr::AgentStatus::Working => AgentState::Working,
                herdr::AgentStatus::Blocked => AgentState::Blocked,
                herdr::AgentStatus::Unknown => AgentState::Unknown,
            },
            herdr::AgentReading::NoAgent => continue,
            herdr::AgentReading::Unanswered => AgentState::Unanswered,
        };
        return Some(Skip::AgentBusy {
            pane_id: session.pane_id.clone(),
            state,
        });
    }
    (running > held.sessions.len()).then_some(Skip::SessionsOutsidePanes {
        running,
        held: held.sessions.len(),
    })
}

/// What runs inside the container to print its processes: for each, its `stat`
/// line, then its `cmdline` with every NUL and newline made a unit separator
/// (0x1f), one process per two lines. A closing marker says the script ran to
/// its end. `sh`, `cat` and `tr` are in every image `dl` opens.
const READ_THE_TABLE: &str = "for d in /proc/[0-9]*; do \
     s=$(cat \"$d/stat\" 2>/dev/null) || continue; \
     c=$(tr '\\000\\n' '\\037\\037' < \"$d/cmdline\" 2>/dev/null); \
     printf '%s\\n%s\\n' \"$s\" \"$c\"; done; echo devlaunch-end";

/// One process, as much of it as the rule reads.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Process {
    pid: u32,
    ppid: u32,
    comm: String,
    argv: Vec<String>,
}

impl Process {
    /// An agent dl knows by name ([`herdr::agent_named`]): named for one,
    /// started as one, or a `node` running a script named for one (the npm
    /// install).
    fn is_agent(&self) -> bool {
        let named = |word: &String| herdr::agent_named(word).is_some();
        herdr::agent_named(&self.comm).is_some() || self.argv.iter().take(2).any(named)
    }
}

/// Read what [`READ_THE_TABLE`] printed. A zombie is not running anything, so
/// it is left out.
fn process_table(out: &str) -> Result<Vec<Process>, String> {
    let body = out
        .strip_suffix("devlaunch-end\n")
        .ok_or("the listing stopped before its end")?;
    let lines: Vec<&str> = body.lines().collect();
    let mut table = Vec::new();
    for pair in lines.chunks(2) {
        let [stat, cmdline] = pair else {
            return Err("the listing ended halfway through a process".to_owned());
        };
        let (open, close) = stat
            .find(" (")
            .zip(stat.rfind(") "))
            .ok_or_else(|| format!("a stat line did not parse: {stat}"))?;
        let mut rest = stat[close + 2..].split(' ');
        let state = rest.next();
        let (Ok(pid), Some(Ok(ppid))) = (
            stat[..open].parse::<u32>(),
            rest.next().map(str::parse::<u32>),
        ) else {
            return Err(format!("a stat line did not parse: {stat}"));
        };
        if state == Some("Z") {
            continue;
        }
        table.push(Process {
            pid,
            ppid,
            comm: stat[open + 2..close].to_owned(),
            argv: cmdline
                .split('\u{1f}')
                .filter(|word| !word.is_empty())
                .map(str::to_owned)
                .collect(),
        });
    }
    Ok(table)
}

#[cfg(test)]
mod tests;
