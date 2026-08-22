//! Telling a host-side [herdr](https://herdr.dev) session what the agent in a
//! container is doing.
//!
//! herdr is a terminal multiplexer that shows, per pane, whether the coding agent
//! in it is working, idle or waiting for input. It decides *which* agent a pane
//! holds from the pane's foreground process, and that is the one signal a dl
//! workspace cannot supply: run `aid` in a herdr pane and the host's process tree
//! is `aid → dl → ssh`, with the agent inside the container where no process table
//! on the host can see it. herdr registers no agent for the pane and shows no
//! state.
//!
//! What it is *not* is a rendering problem. The pane's screen bytes and its OSC
//! title cross the ssh hop intact — herdr's own state rules match against them
//! correctly once it believes an agent is there — so everything needed to classify
//! the state already arrives. Only the identification is missing, and herdr takes
//! an answer for that over its socket: `pane.report_agent` names the pane, the
//! agent and the state, and holds until released.
//!
//! So this module carries three things across the boundary and nothing else:
//!
//! 1. **The socket**, bind-mounted into the container at [`CONTAINER_SOCKET`], so
//!    something inside can reach herdr at all ([`up_args`]).
//! 2. **The pane's identity**, as variables on the agent's own command line
//!    ([`session_env`]) — not workspace configuration, for the reason
//!    [`session_env`] gives.
//! 3. **A hook** that turns the agent's own lifecycle events into reports
//!    ([`hook_script`]), because the state has to come from something that knows
//!    it, and a held report that never changes is a badge that lies.
//!
//! Nothing here runs on a host that is not using herdr: every entry point starts
//! from [`Session::from_env`] or [`HostSocket::from_env`] answering `None`.
//!
//! # What this deliberately does not do
//!
//! It does not install herdr in the container, and it does not use herdr's own
//! `herdr integration install claude`. That integration's hook sends
//! `pane.report_agent_session` — session *identity* for a pane herdr already knows
//! holds an agent — which registers nothing on its own, so forwarding it would
//! carry session metadata for an agent herdr does not believe exists. The reports
//! here are the ones that make the pane appear.

use std::path::{Path, PathBuf};

use crate::shell::quote;

// ===========================================================================
// the constants both ends of the boundary are written against
// ===========================================================================

/// Set this to opt a machine out of reporting agent state to herdr.
///
/// A variable of its own rather than a value of
/// [`crate::flows::provision::DISABLE_VAR`], for the reason
/// [`crate::flows::provision::ZELLIJ_DISABLE_VAR`] is separate: the questions
/// differ. `DEVLAUNCH_NO_TOOLS` is "put nothing in my containers", which costs the
/// `gh` and `claude` guarantee; this is "do not mount my herdr socket into
/// them", which is a question about one socket and answers nothing about tools.
pub(crate) const DISABLE_VAR: &str = "DEVLAUNCH_NO_HERDR";

/// herdr's marker that a process is running inside one of its panes.
const ENV_MARK_VAR: &str = "HERDR_ENV";

/// The pane a report is about, as herdr numbers them (`w1:p2`).
const PANE_VAR: &str = "HERDR_PANE_ID";

/// Where herdr's control socket is. Read from the environment on the host, and
/// *written* — pointing at [`CONTAINER_SOCKET`] — for the agent in the container.
const SOCKET_VAR: &str = "HERDR_SOCKET_PATH";

/// Where the host's herdr socket is mounted inside a workspace.
///
/// An absolute path under `/var/tmp` rather than anything below `$HOME`, for the
/// reason the pixi cache target is: a mount target is decided on the host, before
/// this launch has ever asked the container where its home directory is, and a
/// guess at that is a mount that lands somewhere nobody reads.
pub(crate) const CONTAINER_SOCKET: &str = "/var/tmp/devlaunch-herdr.sock";

/// The `source` every report carries.
///
/// herdr keys held authority by source, so this is what lets a report be replaced
/// and released by its own author without disturbing anyone else's.
const REPORT_SOURCE: &str = "devlaunch";

/// Where the hook lands in the container, and the string that identifies this
/// feature's own entries in a settings file.
///
/// One constant for both, because the command a wired hook runs *is* this path: a
/// separate marker would be a second thing to keep in step with the first.
///
/// Under devlaunch's own directory, which is the same argument
/// [`crate::flows::provision`] makes for `$HOME/.devlaunch/pixi`: every path below
/// it is one devlaunch created, so nothing else syncs, prunes or rewrites it.
const HOOK_RELPATH: &str = ".devlaunch/herdr-agent-state.py";

/// Where the installer reads this container's mount table from.
///
/// A variable rather than the one path, so the host-shared refusal can be tested
/// against a stated set of mounts. A test cannot mount anything -- that needs
/// privileges a test run has no business holding -- and a guard nothing exercises
/// is a guard that is wrong the first time it matters.
const MOUNTINFO_VAR: &str = "DEVLAUNCH_MOUNTINFO";

// ===========================================================================
// the opt-out
// ===========================================================================

/// Whether this launch may report agent state to herdr.
///
/// A type of its own rather than a bool, for the reason
/// [`crate::flows::provision::ZellijSwitch`] is one: it travels beside other
/// switches, and two bools next to each other in a signature are two a caller can
/// swap without the compiler noticing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HerdrSwitch {
    /// The default: a machine that set no variable reports.
    #[default]
    Report,
    Skip,
}

impl HerdrSwitch {
    /// What `DEVLAUNCH_NO_HERDR` set to `value` asks for.
    ///
    /// Through the same [`crate::flows::provision::provisioning_disabled`] the
    /// other opt-outs read, so `DEVLAUNCH_NO_HERDR=0` means what
    /// `DEVLAUNCH_NO_TOOLS=0` means. Two escape hatches a user spells the same way
    /// must answer the same way.
    pub(crate) fn requested(value: Option<&str>) -> Self {
        if crate::flows::provision::provisioning_disabled(value) {
            Self::Skip
        } else {
            Self::Report
        }
    }

    /// What the process environment asks for.
    ///
    /// A host with no herdr socket answers `Skip`, and that is not an optimisation:
    /// the stage this gates prints a line whenever it cannot do its work, at the
    /// level [`crate::flows::provision::HERDR_STAGE`] declares — so a machine that
    /// has never heard of herdr would be told, on every single launch, that a stage
    /// it did not ask for exited 1. Deciding it here rather than inside the stage is
    /// what makes "nothing changes on a host not running herdr" true of the payload
    /// rather than only of its effects.
    ///
    /// Resolved separately from the socket [`crate::flows::launch::Host`] carries,
    /// and the two can in principle disagree — a herdr that stopped between the two
    /// reads. Harmless in both directions: a mount with no stage installs no hook,
    /// and a stage with no mount is the case the stage's own first check names.
    pub fn from_env() -> Self {
        Self::decide(
            crate::osext::env_str(DISABLE_VAR).as_deref(),
            HostSocket::from_env().as_ref(),
        )
    }

    /// [`Self::from_env`]'s decision, as a function of its two inputs.
    ///
    /// Split out for the reason [`crate::flows::provision::provisioning_disabled`]
    /// is a parameter rather than a read: a test states the host it means instead of
    /// mutating an environment the whole binary shares.
    fn decide(disable: Option<&str>, socket: Option<&HostSocket>) -> Self {
        match (Self::requested(disable), socket) {
            (Self::Skip, _) | (Self::Report, None) => Self::Skip,
            (Self::Report, Some(_)) => Self::Report,
        }
    }
}

// ===========================================================================
// the host's half: is there a herdr, and which pane are we in
// ===========================================================================

/// A herdr control socket on this host, which exists and is a socket.
///
/// Checked rather than assumed, because the whole of what this is for is being
/// bind-mounted, and `--mount` is strict about its source in a way `-v` is not:
/// `-v` on a missing path invents a directory, where `--mount` refuses with
/// `bind source path does not exist` and takes `devpod up` — and so the launch —
/// down with it. Resolving the socket first is what keeps a herdr that is not
/// running from costing a workspace.
///
/// It narrows that to a race rather than closing it: a herdr that stops between
/// this check and the `up` leaves a mount whose source has gone, and that launch
/// fails. Nothing the host can read rules it out, and the window is one launch's
/// setup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostSocket(PathBuf);

impl HostSocket {
    /// The socket this host's herdr is listening on, if it is listening.
    ///
    /// `HERDR_SOCKET_PATH` first, because inside a pane that is herdr's own answer
    /// and it is authoritative. Falling back to the XDG location covers the case
    /// this feature needs it for: the container is created by a `dl` that is *not*
    /// in a pane — a prewarm, a `dl <ws> up`, an `aid` in a plain terminal — and a
    /// mount only lands at creation, so a socket found only inside a pane would
    /// leave every such workspace unable to report later without a
    /// `dl <ws> recreate`.
    pub fn from_env() -> Option<Self> {
        let named = crate::osext::env_str(SOCKET_VAR)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let candidate = match named {
            Some(path) => path,
            None => default_socket()?,
        };
        Self::at(&candidate)
    }

    /// The socket at `path`, if there is a socket there and it can be named in a
    /// mount specification.
    ///
    /// The second condition is not fussiness. A specification is comma-delimited
    /// `key=value` pairs, so a comma in the path does not produce a wrong mount —
    /// it produces a `docker run` that rejects its own command line, which fails
    /// `devpod up` and with it the launch. Refusing here is what keeps that
    /// impossible: no socket is a state the whole module already handles, and the
    /// mount is the one part of this that a stage's "can never fail a launch"
    /// contract does not cover.
    fn at(path: &Path) -> Option<Self> {
        use std::os::unix::fs::FileTypeExt;
        let name = path.to_str()?;
        if name.contains(',') || name.chars().any(char::is_control) {
            return None;
        }
        let kind = std::fs::metadata(path).ok()?.file_type();
        kind.is_socket().then(|| Self(path.to_owned()))
    }

    /// The path, for a mount argument.
    pub(crate) fn as_str(&self) -> Option<&str> {
        self.0.to_str()
    }
}

/// herdr's socket where XDG says its config lives.
///
/// `XDG_CONFIG_HOME` and not a bare `~/.config`, so a host that moved its config
/// tree is followed rather than guessed past. This is only ever used to *find* a
/// socket that must already exist, so a wrong answer here fails closed.
fn default_socket() -> Option<PathBuf> {
    let config = match crate::osext::env_str("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => crate::osext::home_dir()?.join(".config"),
    };
    Some(config.join("herdr").join("herdr.sock"))
}

/// The herdr pane this process is running in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pane_id: String,
}

impl Session {
    /// The pane this process is in, if it is in one.
    ///
    /// Both signals are required. `HERDR_ENV` alone says a herdr is around but not
    /// which pane a report would be about, and a pane id alone (an inherited
    /// variable, a copied shell) names a pane nothing here is running in. herdr's
    /// own hook makes the same pair of checks before it reports anything.
    pub fn from_env() -> Option<Self> {
        let marked = crate::osext::env_str(ENV_MARK_VAR)?;
        if crate::osext::strip(&marked) != "1" {
            return None;
        }
        let pane_id = crate::osext::env_str(PANE_VAR).filter(|value| !value.is_empty())?;
        Some(Self { pane_id })
    }

    /// The pane, as herdr names it.
    pub fn pane_id(&self) -> &str {
        &self.pane_id
    }

    /// A named pane, for a caller that already knows which one it is in.
    ///
    /// Public because the one thing built on top of this lives in another crate:
    /// `aid` puts a session's variables on the agent's command line, and a test of
    /// that has to be able to say which pane it means without setting variables the
    /// rest of the test binary shares.
    pub fn for_pane(pane_id: impl Into<String>) -> Self {
        Self {
            pane_id: pane_id.into(),
        }
    }
}

// ===========================================================================
// what a launch adds
// ===========================================================================

/// The `devpod up` flags that put the herdr socket in the container.
///
/// A bind mount of the socket *file*, not of the directory holding it — the
/// argument this repo's own devcontainer makes for mounting two files out of
/// `~/.ssh` rather than the directory: a directory mount carries everything else
/// in it too, and herdr's config directory holds this developer's keybindings and
/// its own state.
///
/// Empty without a socket, which is what a host with no herdr gets, and is why
/// this can be called unconditionally.
pub(crate) fn up_args(socket: Option<&HostSocket>, switch: HerdrSwitch) -> Vec<String> {
    let (HerdrSwitch::Report, Some(socket)) = (switch, socket) else {
        return Vec::new();
    };
    let Some(source) = socket.as_str() else {
        return Vec::new();
    };
    vec![
        "--mount".to_owned(),
        format!("type=bind,source={source},target={CONTAINER_SOCKET}"),
    ]
}

/// The variables the agent's own command line carries, so the hook knows where to
/// report.
///
/// On the command line and **not** through `--workspace-env`, which is where the
/// pixi cache's variable goes. Two reasons, and either alone decides it:
///
/// - A pane id is a fact about *this session*, not about the workspace. Workspace
///   environment is written by an `up`, and attaching to a workspace that is
///   already running skips the `up` entirely — so the container would keep
///   whichever pane id was current when it was last built and report this session's
///   state into a pane that has since become something else.
/// - `aid` already passes `IS_SANDBOX=1` this way, so the mechanism is one the
///   agent command already uses rather than a second one invented here.
///
/// The socket path is rewritten to [`CONTAINER_SOCKET`] rather than passed through.
/// Forwarding the host's path is the one failure that is silent: the hook finds a
/// variable set, tries a path that does not exist inside the container, and gives
/// up without reporting anything.
pub fn session_env(session: &Session) -> Vec<(String, String)> {
    vec![
        (ENV_MARK_VAR.to_owned(), "1".to_owned()),
        (PANE_VAR.to_owned(), session.pane_id.clone()),
        (SOCKET_VAR.to_owned(), CONTAINER_SOCKET.to_owned()),
    ]
}

// ===========================================================================
// the container's half: the hook
// ===========================================================================

/// The shell script that installs the state-reporting hook in a workspace.
///
/// One stage's command, and like every other stage it may fail without costing the
/// launch anything: a container with no `python3`, or one whose claude
/// configuration directory is shared with the host, is a container that reports no
/// agent state and opens exactly as it would have.
///
/// Two shell tests and then **one** `python3`, which does all of the work: resolve
/// the configuration directory, refuse a shared one, write the hook, make it
/// executable, and merge the settings. Shell does none of it, and that is the
/// lesson of the version that did — it wrote the hook with `cat > "$hook" <<EOF`,
/// and in a container with no `cat` the redirect still created the file, `cat`
/// failed, and the stage reported success having installed an **empty** hook that
/// claude then ran on every event. Every command that version needed (`cat`,
/// `mkdir`, `dirname`, `chmod`, `grep`) is one more thing a container can be
/// missing in a way that ends as a wired-up hook which does nothing, so none of
/// them is used: `python3` was already required, and it can neither half-write a
/// file nor be quietly absent.
pub(crate) fn hook_script() -> String {
    hook_script_at(CONTAINER_SOCKET)
}

/// [`hook_script`] against a named socket, so a test can run the real script
/// against a socket of its own rather than against the one absolute path a
/// container has.
fn hook_script_at(socket: &str) -> String {
    [
        "set -u".to_owned(),
        // A stage shares the probe's stdout, and both readers split marked lines on
        // spaces. Nothing here is the answer to anything, so send the lot to stderr
        // rather than risk one line being read as protocol.
        "exec >&2".to_owned(),
        // The socket first, because it is the one refusal with something the
        // developer can do about it, and the reason has to be printed here: a
        // stage's exit status is all the host learns.
        format!(
            "if [ ! -S {} ]; then echo 'devlaunch: no herdr socket in this container; \
             dl <ws> recreate adds it (a mount only lands at creation)'; exit 1; fi",
            quote(socket)
        ),
        "if ! command -v python3 >/dev/null 2>&1; then echo 'devlaunch: no python3 in this \
         container; herdr agent state needs one'; exit 1; fi"
            .to_owned(),
        format!("python3 - <<'{DELIMITER}'\n{}\n{DELIMITER}", installer()),
    ]
    .join("\n")
}

/// The heredoc delimiter the installer is written with.
///
/// Quoted at the opening (`<<'…'`) so the shell expands nothing inside: the payload
/// is python, and it is full of `$`.
const DELIMITER: &str = "DEVLAUNCH_HERDR_EOF";

/// The python that installs the hook and wires it up.
///
/// It writes the hook with the interpreter running it as the hook's own shebang
/// (`sys.executable`) rather than `#!/usr/bin/env python3`. The python that
/// installed the hook is by construction one that exists and runs in this
/// container; `env` resolving `python3` again at hook time is a second lookup that
/// can answer differently — a different PATH under claude, or none.
fn installer() -> String {
    let events = HOOK_EVENTS
        .iter()
        .map(|(event, action)| format!("    (\"{event}\", \"{action}\"),"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r####"import json
import os
import stat
import sys

EVENTS = [
{events}
]
HOOK_RELPATH = "{hook_relpath}"
# The table that says which of this container's paths are shared with the host.
# Overridable so the refusal below can be tested against a stated set of mounts:
# the alternative is a test that has to mount something, which needs privileges a
# test run does not have.
MOUNTINFO = os.environ.get("{mountinfo_var}") or "/proc/self/mountinfo"
HOOK_SOURCE = r'''{hook_source}'''

home = os.path.expanduser("~")
config_dir = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.join(home, ".claude")

hook = os.path.join(home, HOOK_RELPATH)
settings_path = os.path.join(config_dir, "settings.json")


def host_shared(path):
    """The mount that would carry a write to `path` back to the host, or None.

    A path is host-shared when the most specific mount covering it is a bind of
    somewhere else -- which is what mountinfo's root field says: it is "/" for a
    filesystem mounted whole (the container's own root, a tmpfs on /tmp) and the
    source subpath for a bind. Asking whether `path` is *itself* a mount point is
    the version of this that does not work, because the common shape is a mounted
    parent: this repo's own devcontainer binds ~/.claude/hooks and ~/.claude/agents
    while ~/.claude itself is container-local, and a container that binds ~/.config
    with CLAUDE_CONFIG_DIR inside it shares the settings file without ever making
    it a mount point.
    """
    try:
        with open(MOUNTINFO, encoding="utf-8") as handle:
            lines = handle.read().splitlines()
    except FileNotFoundError:
        # No mountinfo is no evidence of sharing, and this is the ordinary state
        # of anything that is not Linux.
        return None
    covering = None
    target = os.path.realpath(path)
    for line in lines:
        fields = line.split(" ")
        if len(fields) < 5:
            continue
        root, point = fields[3], os.path.realpath(fields[4])
        if target == point or target.startswith(point.rstrip("/") + "/"):
            if covering is None or len(point) >= len(covering[1]):
                covering = (root, point)
    if covering and covering[0] != "/":
        return covering[1]
    return None


# Writing into a directory the host shares would edit the developer's own claude
# settings, from inside every container, on every launch. Refused rather than
# merged into: the reports this buys are not worth a machine-wide edit nobody
# asked for. An unreadable mountinfo is not taken as permission.
try:
    shared = host_shared(settings_path) or host_shared(hook)
except Exception:
    print("devlaunch: cannot tell whether %s is shared with the host; not writing hooks" % config_dir)
    raise SystemExit(1)
if shared:
    print("devlaunch: %s is mounted from the host; not writing hooks into it" % shared)
    raise SystemExit(1)

os.makedirs(os.path.dirname(hook), exist_ok=True)
os.makedirs(config_dir, exist_ok=True)

# Written whole and then moved, so a hook that exists is a hook that is complete:
# claude runs this file on every event, and a truncated one is a syntax error per
# turn. The shebang is this interpreter, which is one that demonstrably runs here.
source = "#!%s\n%s" % (sys.executable, HOOK_SOURCE)
tmp = hook + ".devlaunch.tmp"
with open(tmp, "w", encoding="utf-8") as handle:
    handle.write(source)
os.chmod(tmp, stat.S_IRWXU | stat.S_IRGRP | stat.S_IXGRP | stat.S_IROTH | stat.S_IXOTH)
os.replace(tmp, hook)

# A merge and not a write. A container's settings file can hold the repo's own
# hooks, a base image's, or a developer's dotfiles' -- and this runs on every
# launch, so a write would delete them all, repeatedly.
try:
    with open(settings_path, encoding="utf-8") as handle:
        settings = json.load(handle)
except FileNotFoundError:
    settings = {{}}
except Exception:
    # An unreadable settings file is somebody else's: truncating it to install a
    # badge would cost more than the badge is worth.
    print("devlaunch: %s is not readable JSON; leaving it alone" % settings_path)
    raise SystemExit(1)
if not isinstance(settings, dict):
    print("devlaunch: %s is not a JSON object; leaving it alone" % settings_path)
    raise SystemExit(1)

hooks = settings.setdefault("hooks", {{}})
if not isinstance(hooks, dict):
    print("devlaunch: the hooks in %s are not an object; leaving them alone" % settings_path)
    raise SystemExit(1)

for event, action in EVENTS:
    entries = hooks.get(event)
    if not isinstance(entries, list):
        entries = []
    # Ours out, everyone else's untouched, ours back in. Found by the hook's own
    # settings_path rather than by an exact command match, so an entry written by an older
    # devlaunch is replaced rather than duplicated.
    kept = []
    for entry in entries:
        inner = entry.get("hooks") if isinstance(entry, dict) else None
        if isinstance(inner, list) and any(
            isinstance(item, dict) and HOOK_RELPATH in str(item.get("command", ""))
            for item in inner
        ):
            continue
        kept.append(entry)
    kept.append(
        {{"hooks": [{{"type": "command", "command": "%s %s" % (hook, action)}}]}}
    )
    hooks[event] = kept

tmp = settings_path + ".devlaunch.tmp"
with open(tmp, "w", encoding="utf-8") as handle:
    json.dump(settings, handle, indent=2)
    handle.write("\n")
os.replace(tmp, settings_path)
"####,
        events = events,
        hook_relpath = HOOK_RELPATH,
        mountinfo_var = MOUNTINFO_VAR,
        hook_source = hook_source(),
    )
}

/// The hook itself, as it lands in the container: one python file.
///
/// Python and not a shell script wrapping python, which is what herdr's own
/// integration ships. The shell half of that shape exists to spool claude's JSON
/// into a temporary file, so it needs `mktemp`, `cat` and `rm` at hook time — three
/// more commands a container can lack, on a path that runs on every event of every
/// turn. Reading `sys.stdin` needs none of them.
///
/// Exits 0 in every state, including every failure. A hook that fails a tool call,
/// or interrupts a session, to report a badge has cost more than the badge is
/// worth — and this one runs inside the developer's own agent, not beside it.
fn hook_source() -> String {
    format!(
        r##"# installed by devlaunch; edits are overwritten on the next launch.
import json
import os
import socket
import sys
import time

# The state each event means. Two of claude's events are deliberately absent:
# PreToolUse/PostToolUse would report "working" many times a turn for a state Stop
# already ends, and SubagentStop is a completion that can arrive after the main
# turn has stopped -- reporting idle from it would end a turn still running.
STATES = {{
    "session-start": "idle",
    "prompt": "working",
    "notification": "blocked",
    "stop": "idle",
}}
RELEASE = "session-end"


def main():
    action = sys.argv[1] if len(sys.argv) > 1 else ""
    pane_id = os.environ.get("{pane}")
    socket_path = os.environ.get("{socket}")
    if os.environ.get("{mark}") != "1" or not pane_id or not socket_path:
        return

    try:
        text = sys.stdin.read()
    except Exception:
        text = ""
    try:
        payload = json.loads(text) if text.strip() else {{}}
    except Exception:
        payload = {{}}
    if not isinstance(payload, dict):
        payload = {{}}

    # A subagent's events are not the session's state: claude runs them inside a
    # turn that is still working, and letting one report idle would end it early.
    if payload.get("agent_id"):
        return

    # Ordering, not identity: reports can cross on the socket, and herdr keeps the
    # newest by this number rather than by arrival.
    params = {{
        "pane_id": pane_id,
        "source": "{source}",
        "agent": "claude",
        "seq": time.time_ns(),
    }}

    if action == RELEASE:
        method = "pane.release_agent"
    else:
        state = STATES.get(action)
        if state is None:
            return
        method = "pane.report_agent"
        params["state"] = state
        session_id = payload.get("session_id")
        if isinstance(session_id, str) and session_id:
            params["agent_session_id"] = session_id
        transcript = payload.get("transcript_path")
        if isinstance(transcript, str) and transcript:
            params["agent_session_path"] = transcript

    request = {{
        "id": "{source}:%d" % params["seq"],
        "method": method,
        "params": params,
    }}
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(0.5)
    try:
        client.connect(socket_path)
        client.sendall((json.dumps(request) + "\n").encode())
        try:
            client.recv(4096)
        except Exception:
            pass
    finally:
        client.close()


try:
    main()
except Exception:
    # Never the agent's problem. A herdr that is not listening, a socket whose
    # session has ended, a claude that changed its payload: all of them are a
    # badge that does not appear, and none of them is a turn that fails.
    pass
"##,
        mark = ENV_MARK_VAR,
        pane = PANE_VAR,
        socket = SOCKET_VAR,
        source = REPORT_SOURCE,
    )
}

/// Which of claude's hook events this reports from, and the word the hook is
/// called with for each.
///
/// The four that bound a turn, plus the end of the session. `Notification` is the
/// one that earns this feature: it is what claude fires when it is waiting for a
/// human, which is the state a fleet view exists to surface.
const HOOK_EVENTS: [(&str, &str); 5] = [
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "prompt"),
    ("Notification", "notification"),
    ("Stop", "stop"),
    ("SessionEnd", "session-end"),
];

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::os::unix::net::UnixListener;
    use std::process::Command;

    /// Run `script` with exactly `env` and nothing else — the hermetic shape
    /// [`crate::flows::provision`]'s script tests use, for the same reason: a test
    /// of "this container has no python3" must not find the developer's own.
    fn bash_with(script: &str, env: &[(&str, &str)]) -> std::process::Output {
        let bash = which("bash").unwrap_or_else(|| PathBuf::from("/bin/bash"));
        let mut command = Command::new(bash);
        command.arg("-c").arg(script).env_clear();
        for (name, value) in env {
            command.env(name, value);
        }
        command.output().expect("bash ran")
    }

    /// The first `program` on the real PATH — python3 is in the project
    /// environment rather than in `/usr/bin`, so a hardcoded list would not find it.
    fn which(program: &str) -> Option<PathBuf> {
        std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(program))
                .find(|candidate| candidate.is_file())
        })
    }

    /// A PATH carrying only the commands named, linked out of the real ones.
    fn sysbin(scratch: &Path, commands: &[&str]) -> PathBuf {
        let sysbin = scratch.join("sysbin");
        fs::create_dir_all(&sysbin).expect("a scratch sysbin");
        for command in commands {
            let found = which(command).unwrap_or_else(|| panic!("the test host needs {command}"));
            let link = sysbin.join(command);
            if !link.exists() {
                std::os::unix::fs::symlink(found, link).expect("a linked command");
            }
        }
        sysbin
    }

    /// Everything the installer script shells out to.
    const INSTALLER_COMMANDS: [&str; 1] = ["python3"];

    /// A scratch home, and a real socket standing in for the mounted one.
    fn a_container(scratch: &Path) -> (PathBuf, PathBuf, UnixListener) {
        let home = scratch.join("home");
        fs::create_dir_all(&home).expect("a scratch home");
        let socket_path = scratch.join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).expect("a scratch socket");
        (home, socket_path, listener)
    }

    #[test]
    fn the_opt_out_reads_the_values_every_other_opt_out_reads() {
        assert_eq!(HerdrSwitch::requested(None), HerdrSwitch::Report);
        for falsey in ["", "0", "false", "no", "NO", " false "] {
            assert_eq!(
                HerdrSwitch::requested(Some(falsey)),
                HerdrSwitch::Report,
                "{falsey:?} must not turn it off"
            );
        }
        for truthy in ["1", "yes", "true", "anything"] {
            assert_eq!(
                HerdrSwitch::requested(Some(truthy)),
                HerdrSwitch::Skip,
                "{truthy:?} must turn it off"
            );
        }
    }

    #[test]
    fn no_socket_is_no_mount() {
        assert_eq!(up_args(None, HerdrSwitch::Report), Vec::<String>::new());
    }

    #[test]
    fn the_opt_out_beats_a_socket_that_is_there() {
        let socket = HostSocket(PathBuf::from("/run/herdr.sock"));
        assert_eq!(
            up_args(Some(&socket), HerdrSwitch::Skip),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_socket_is_mounted_as_a_file_at_the_fixed_target() {
        let socket = HostSocket(PathBuf::from("/home/dev/.config/herdr/herdr.sock"));
        assert_eq!(
            up_args(Some(&socket), HerdrSwitch::Report),
            vec![
                "--mount".to_owned(),
                format!(
                    "type=bind,source=/home/dev/.config/herdr/herdr.sock,target={CONTAINER_SOCKET}"
                ),
            ]
        );
    }

    /// The one failure that would be silent: the container told to talk to a path
    /// that only exists on the host.
    #[test]
    fn the_session_carries_the_container_socket_not_the_hosts() {
        let session = Session {
            pane_id: "w1:p2".to_owned(),
        };
        let env = session_env(&session);
        assert_eq!(
            env,
            vec![
                ("HERDR_ENV".to_owned(), "1".to_owned()),
                ("HERDR_PANE_ID".to_owned(), "w1:p2".to_owned()),
                ("HERDR_SOCKET_PATH".to_owned(), CONTAINER_SOCKET.to_owned()),
            ]
        );
    }

    /// A mount specification is comma-delimited, so a comma anywhere in the source
    /// path does not make a bad mount — it makes `docker run` reject the whole
    /// command line, which fails `devpod up` and with it the launch. This module
    /// promises never to cost a launch anything, and the mount is the one part of it
    /// that is not inside a stage's protection.
    #[test]
    fn a_socket_whose_path_would_break_a_mount_specification_is_refused() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        for name in ["co,mma", "new\nline"] {
            let dir = scratch.path().join(name);
            fs::create_dir_all(&dir).expect("a scratch directory");
            let path = dir.join("herdr.sock");
            let _listener = UnixListener::bind(&path).expect("a scratch socket");
            assert_eq!(
                HostSocket::at(&path),
                None,
                "{name:?} must not reach a mount specification"
            );
        }
    }

    #[test]
    fn a_path_that_is_not_a_socket_is_not_one() {
        let dir = std::env::temp_dir();
        assert_eq!(HostSocket::at(&dir), None);
        assert_eq!(HostSocket::at(&dir.join("devlaunch-absent-xyz")), None);
    }

    #[test]
    fn the_installer_refuses_before_it_writes() {
        let script = hook_script();
        let socket_check = script
            .find(&format!("-S {}", quote(CONTAINER_SOCKET)))
            .expect("a socket check");
        let python_check = script.find("command -v python3").expect("a python3 check");
        let shared_check = script.find("mountinfo").expect("a shared-config check");
        let write = script
            .find("os.replace(tmp, hook)")
            .expect("the hook write");
        assert!(socket_check < write, "the socket check must come first");
        assert!(python_check < write, "the python3 check must come first");
        assert!(
            shared_check < write,
            "the shared-config check must come first"
        );
        // Nothing but python does the work, and that is load-bearing: the version
        // that shelled out wrote an empty hook in a container with no `cat`.
        for command in ["cat ", "mkdir ", "dirname ", "chmod ", "grep "] {
            assert!(
                !script.contains(command),
                "the installer must not depend on {command:?}"
            );
        }
    }

    #[test]
    fn every_wired_event_is_one_the_hook_maps() {
        let body = hook_source();
        for (event, action) in HOOK_EVENTS {
            assert!(
                body.contains(&format!("\"{action}\"")),
                "{event} sends {action}, which the hook does not handle"
            );
        }
    }

    /// The states are herdr's spelling of them, not ours: the socket refuses
    /// anything else, and a refused report is a badge that never appears.
    #[test]
    fn the_reported_states_are_the_ones_herdr_accepts() {
        let body = hook_source();
        for state in ["idle", "working", "blocked"] {
            assert!(body.contains(&format!("\"{state}\"")), "{state} is missing");
        }
        assert!(body.contains("pane.report_agent"));
        assert!(body.contains("pane.release_agent"));
        assert!(
            body.contains(&format!("\"source\": \"{REPORT_SOURCE}\""))
                || body.contains(REPORT_SOURCE)
        );
    }

    /// A turn that is still running must not be ended by something inside it.
    #[test]
    fn a_subagents_events_are_ignored() {
        assert!(hook_source().contains("agent_id"));
    }

    #[test]
    fn the_merge_keeps_hooks_it_did_not_write() {
        let merge = installer();
        assert!(merge.contains("kept.append(entry)"), "others are kept");
        assert!(merge.contains(HOOK_RELPATH), "ours are found by the marker");
        assert!(merge.contains("os.replace"), "the write is atomic");
        for (event, _) in HOOK_EVENTS {
            assert!(merge.contains(event), "{event} is not wired");
        }
    }

    #[test]
    fn a_host_with_no_herdr_socket_does_no_herdr_work() {
        // The switch, not the stage, is where this is decided — see `decide`. A
        // machine that never heard of herdr must not be told about a stage.
        assert_eq!(HerdrSwitch::decide(None, None), HerdrSwitch::Skip);
        let socket = HostSocket(PathBuf::from("/run/herdr.sock"));
        assert_eq!(
            HerdrSwitch::decide(None, Some(&socket)),
            HerdrSwitch::Report
        );
        // And the opt-out still beats a socket that is there.
        assert_eq!(
            HerdrSwitch::decide(Some("1"), Some(&socket)),
            HerdrSwitch::Skip
        );
    }

    // =======================================================================
    // the installer, actually run
    // =======================================================================

    /// The script is the shipped behaviour, so it is run rather than inspected.
    #[test]
    fn the_installer_writes_the_hook_and_wires_every_event() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let (home, socket, _listener) = a_container(scratch.path());
        let path = sysbin(scratch.path(), &INSTALLER_COMMANDS);

        let ran = bash_with(
            &hook_script_at(&socket.to_string_lossy()),
            &[
                ("HOME", &home.to_string_lossy()),
                ("PATH", &path.to_string_lossy()),
            ],
        );

        assert!(
            ran.status.success(),
            "the installer failed: {}",
            String::from_utf8_lossy(&ran.stderr)
        );
        let hook = home.join(HOOK_RELPATH);
        assert!(hook.is_file(), "no hook was written");
        let settings = fs::read_to_string(home.join(".claude/settings.json")).expect("settings");
        let parsed: serde_json::Value = serde_json::from_str(&settings).expect("valid JSON");
        for (event, action) in HOOK_EVENTS {
            let entries = parsed["hooks"][event]
                .as_array()
                .unwrap_or_else(|| panic!("{event} is not wired"));
            assert_eq!(entries.len(), 1, "{event} should have exactly one entry");
            let command = entries[0]["hooks"][0]["command"]
                .as_str()
                .expect("a command");
            assert!(command.ends_with(&format!(" {action}")), "{command}");
            assert!(
                command.starts_with(&hook.to_string_lossy().to_string()),
                "{command}"
            );
        }
    }

    /// It runs on every launch, so running it twice has to be running it once.
    #[test]
    fn the_installer_is_idempotent_and_keeps_foreign_hooks() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let (home, socket, _listener) = a_container(scratch.path());
        let path = sysbin(scratch.path(), &INSTALLER_COMMANDS);
        // Somebody else's hook on an event this feature also wants, and a whole
        // event it does not touch: both have to survive.
        fs::create_dir_all(home.join(".claude")).expect("a claude dir");
        fs::write(
            home.join(".claude/settings.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo theirs"}]}],
                "PreToolUse":[{"hooks":[{"type":"command","command":"echo untouched"}]}]},
                "model":"opus"}"#,
        )
        .expect("seeded settings");

        let script = hook_script_at(&socket.to_string_lossy());
        let env = [
            ("HOME", home.to_string_lossy().into_owned()),
            ("PATH", path.to_string_lossy().into_owned()),
        ];
        let env: Vec<(&str, &str)> = env
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        for pass in 1..=3 {
            let ran = bash_with(&script, &env);
            assert!(
                ran.status.success(),
                "pass {pass} failed: {}",
                String::from_utf8_lossy(&ran.stderr)
            );
        }

        let settings = fs::read_to_string(home.join(".claude/settings.json")).expect("settings");
        let parsed: serde_json::Value = serde_json::from_str(&settings).expect("valid JSON");
        // Three passes, one entry of ours per event: the marker found the old one.
        let stop = parsed["hooks"]["Stop"].as_array().expect("Stop");
        assert_eq!(stop.len(), 2, "theirs plus exactly one of ours: {stop:?}");
        assert_eq!(stop[0]["hooks"][0]["command"], "echo theirs");
        assert!(
            stop[1]["hooks"][0]["command"]
                .as_str()
                .expect("a command")
                .contains(HOOK_RELPATH)
        );
        // An event this feature never mentions, and a setting beside the hooks.
        assert_eq!(
            parsed["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "echo untouched"
        );
        assert_eq!(parsed["model"], "opus");
    }

    /// The three refusals, each proved by removing exactly one thing.
    #[test]
    fn the_installer_refuses_rather_than_writing_half_of_itself() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let (home, socket, _listener) = a_container(scratch.path());
        let full = sysbin(scratch.path(), &INSTALLER_COMMANDS);

        // No socket: the workspace predates the mount.
        let ran = bash_with(
            &hook_script_at(&scratch.path().join("absent.sock").to_string_lossy()),
            &[
                ("HOME", &home.to_string_lossy()),
                ("PATH", &full.to_string_lossy()),
            ],
        );
        assert!(!ran.status.success(), "a missing socket must refuse");
        assert!(String::from_utf8_lossy(&ran.stderr).contains("recreate"));
        assert!(!home.join(HOOK_RELPATH).exists(), "nothing may be written");

        // No python3: neither the merge nor the hook's transport can work.
        let without_python = sysbin(&scratch.path().join("nopython"), &[]);
        let ran = bash_with(
            &hook_script_at(&socket.to_string_lossy()),
            &[
                ("HOME", &home.to_string_lossy()),
                ("PATH", &without_python.to_string_lossy()),
            ],
        );
        assert!(!ran.status.success(), "no python3 must refuse");
        assert!(String::from_utf8_lossy(&ran.stderr).contains("python3"));
        assert!(!home.join(HOOK_RELPATH).exists(), "nothing may be written");
    }

    /// A settings file inside a *mounted parent* is shared with the host just as
    /// surely as one that is itself a mount point, and that is the common shape
    /// rather than the exotic one: this repo's own devcontainer binds
    /// `~/.claude/hooks` and `~/.claude/agents` while `~/.claude` stays
    /// container-local, and a container that binds `~/.config` with
    /// `CLAUDE_CONFIG_DIR` inside it shares the settings file without ever making
    /// it a mount point.
    #[test]
    fn a_settings_file_under_a_mounted_parent_is_refused() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let (home, socket, _listener) = a_container(scratch.path());
        let path = sysbin(scratch.path(), &INSTALLER_COMMANDS);
        let config = home.join(".claude");
        fs::create_dir_all(&config).expect("a config dir");

        // mountinfo's shape: id parent major:minor root mount-point ...
        // The bind is of the *parent*; the config directory itself is never named.
        let mountinfo = scratch.path().join("mountinfo");
        fs::write(
            &mountinfo,
            format!(
                "23 1 0:24 / / rw - overlay overlay rw\n\
                 24 23 0:25 /home/ags{} {} rw - ext4 /dev/sda1 rw\n",
                "",
                home.display()
            ),
        )
        .expect("a scratch mountinfo");

        let ran = bash_with(
            &hook_script_at(&socket.to_string_lossy()),
            &[
                ("HOME", &home.to_string_lossy()),
                ("PATH", &path.to_string_lossy()),
                ("DEVLAUNCH_MOUNTINFO", &mountinfo.to_string_lossy()),
            ],
        );

        assert!(
            !ran.status.success(),
            "a mounted parent must refuse: {}",
            String::from_utf8_lossy(&ran.stdout)
        );
        assert!(
            String::from_utf8_lossy(&ran.stderr).contains("mounted from the host"),
            "{}",
            String::from_utf8_lossy(&ran.stderr)
        );
        assert!(
            !config.join("settings.json").exists(),
            "the host's settings must not be written"
        );
        assert!(!home.join(HOOK_RELPATH).exists(), "nor the hook");
    }

    /// The refusal must not fire on an ordinary container, where the paths written
    /// are under a filesystem mounted whole (root `/`) rather than a bind.
    #[test]
    fn a_container_local_settings_file_is_written() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let (home, socket, _listener) = a_container(scratch.path());
        let path = sysbin(scratch.path(), &INSTALLER_COMMANDS);
        let mountinfo = scratch.path().join("mountinfo");
        // A tmpfs on the scratch root and an overlay on /: both mounted whole, so
        // neither shares anything with a host.
        fs::write(
            &mountinfo,
            format!(
                "23 1 0:24 / / rw - overlay overlay rw\n\
                 25 23 0:26 / {} rw - tmpfs tmpfs rw\n",
                scratch.path().display()
            ),
        )
        .expect("a scratch mountinfo");

        let ran = bash_with(
            &hook_script_at(&socket.to_string_lossy()),
            &[
                ("HOME", &home.to_string_lossy()),
                ("PATH", &path.to_string_lossy()),
                ("DEVLAUNCH_MOUNTINFO", &mountinfo.to_string_lossy()),
            ],
        );

        assert!(
            ran.status.success(),
            "an ordinary container must install: {}",
            String::from_utf8_lossy(&ran.stderr)
        );
        assert!(home.join(".claude/settings.json").is_file());
        assert!(home.join(HOOK_RELPATH).is_file());
    }

    /// What the hook actually puts on the socket, checked against herdr's schema:
    /// the method names, the required parameters and the state spellings are all
    /// herdr's, and a report it refuses is a badge that never appears.
    #[test]
    fn the_hook_sends_herdrs_own_request_shape() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let (home, socket, listener) = a_container(scratch.path());
        let path = sysbin(scratch.path(), &["python3"]);
        let installed = bash_with(
            &hook_script_at(&socket.to_string_lossy()),
            &[
                ("HOME", &home.to_string_lossy()),
                ("PATH", &path.to_string_lossy()),
            ],
        );
        assert!(installed.status.success());
        let hook = home.join(HOOK_RELPATH);

        for (action, expected) in [
            ("prompt", "working"),
            ("notification", "blocked"),
            ("stop", "idle"),
        ] {
            let ran = bash_with(
                &format!(
                    "printf '%s' '{{\"session_id\":\"s-1\",\"transcript_path\":\"/t.jsonl\"}}' | {} {action}",
                    quote(&hook.to_string_lossy())
                ),
                &[
                    ("HOME", &home.to_string_lossy()),
                    ("PATH", &path.to_string_lossy()),
                    (ENV_MARK_VAR, "1"),
                    (PANE_VAR, "w1:p2"),
                    (SOCKET_VAR, &socket.to_string_lossy()),
                ],
            );
            assert!(ran.status.success(), "the hook must never fail the agent");

            let request = read_one_request(&listener);
            assert_eq!(request["method"], "pane.report_agent");
            assert_eq!(request["params"]["pane_id"], "w1:p2");
            assert_eq!(request["params"]["source"], REPORT_SOURCE);
            assert_eq!(request["params"]["agent"], "claude");
            assert_eq!(request["params"]["state"], expected, "{action}");
            assert_eq!(request["params"]["agent_session_id"], "s-1");
            assert_eq!(request["params"]["agent_session_path"], "/t.jsonl");
            assert!(request["params"]["seq"].is_number(), "seq orders reports");
        }

        // The end of a session releases the authority rather than pinning a state:
        // a held report nothing updates is a badge that lies.
        let ran = bash_with(
            &format!(
                "printf '%s' '{{\"session_id\":\"s-1\"}}' | {} session-end",
                quote(&hook.to_string_lossy())
            ),
            &[
                ("HOME", &home.to_string_lossy()),
                ("PATH", &path.to_string_lossy()),
                (ENV_MARK_VAR, "1"),
                (PANE_VAR, "w1:p2"),
                (SOCKET_VAR, &socket.to_string_lossy()),
            ],
        );
        assert!(ran.status.success());
        let request = read_one_request(&listener);
        assert_eq!(request["method"], "pane.release_agent");
        assert_eq!(request["params"]["pane_id"], "w1:p2");
        assert!(request["params"].get("state").is_none());
    }

    /// A subagent's Stop must not end a turn that is still running.
    #[test]
    fn the_hook_says_nothing_for_a_subagent_or_an_unknown_event() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let (home, socket, listener) = a_container(scratch.path());
        let path = sysbin(scratch.path(), &["python3"]);
        assert!(
            bash_with(
                &hook_script_at(&socket.to_string_lossy()),
                &[
                    ("HOME", &home.to_string_lossy()),
                    ("PATH", &path.to_string_lossy()),
                ],
            )
            .status
            .success()
        );
        let hook = home.join(HOOK_RELPATH);

        for (input, action) in [
            (r#"{"session_id":"s","agent_id":"sub-1"}"#, "stop"),
            (r#"{"session_id":"s"}"#, "pre-tool-use"),
        ] {
            let ran = bash_with(
                &format!(
                    "printf '%s' '{input}' | {} {action}",
                    quote(&hook.to_string_lossy())
                ),
                &[
                    ("HOME", &home.to_string_lossy()),
                    ("PATH", &path.to_string_lossy()),
                    (ENV_MARK_VAR, "1"),
                    (PANE_VAR, "w1:p2"),
                    (SOCKET_VAR, &socket.to_string_lossy()),
                ],
            );
            assert!(ran.status.success());
        }
        // Nothing connected, so nothing is waiting to be accepted.
        listener
            .set_nonblocking(true)
            .expect("a non-blocking listener");
        assert!(
            listener.accept().is_err(),
            "a subagent or an unmapped event must send nothing"
        );
    }

    /// Outside a herdr pane the hook is inert, which is what lets it be installed
    /// in a container that is sometimes launched from one and sometimes not.
    #[test]
    fn the_hook_is_inert_with_no_pane_to_report_to() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let (home, socket, listener) = a_container(scratch.path());
        let path = sysbin(scratch.path(), &["python3"]);
        assert!(
            bash_with(
                &hook_script_at(&socket.to_string_lossy()),
                &[
                    ("HOME", &home.to_string_lossy()),
                    ("PATH", &path.to_string_lossy()),
                ],
            )
            .status
            .success()
        );
        let hook = home.join(HOOK_RELPATH);
        let ran = bash_with(
            &format!(
                "printf '%s' '{{}}' | {} prompt",
                quote(&hook.to_string_lossy())
            ),
            &[
                ("HOME", &home.to_string_lossy()),
                ("PATH", &path.to_string_lossy()),
            ],
        );
        assert!(ran.status.success(), "no pane is not an error");
        listener
            .set_nonblocking(true)
            .expect("a non-blocking listener");
        assert!(listener.accept().is_err(), "nothing may be sent");
    }

    /// One request, as the hook writes it: one line of JSON and then a close.
    ///
    /// Bounded rather than a bare `accept`, so a hook that reports nothing fails the
    /// test with a sentence instead of hanging it forever.
    fn read_one_request(listener: &UnixListener) -> serde_json::Value {
        use std::io::Read as _;
        let mut stream = accept_within(listener, std::time::Duration::from_secs(10));
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .expect("a read timeout");
        let mut text = String::new();
        stream.read_to_string(&mut text).expect("a request");
        let line = text.lines().next().expect("one line");
        serde_json::from_str(line).expect("valid JSON")
    }

    /// Accept one connection, or fail saying none came.
    fn accept_within(
        listener: &UnixListener,
        within: std::time::Duration,
    ) -> std::os::unix::net::UnixStream {
        listener
            .set_nonblocking(true)
            .expect("a nonblocking listener");
        let deadline = std::time::Instant::now() + within;
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    listener
                        .set_nonblocking(false)
                        .expect("a blocking listener");
                    stream.set_nonblocking(false).expect("a blocking stream");
                    return stream;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the hook never connected"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(error) => panic!("the listener failed: {error}"),
            }
        }
    }

    /// Every embedded heredoc has to be closed, or the stage is a shell syntax
    /// error that reports as a failure with no reason.
    #[test]
    fn the_heredocs_are_balanced() {
        let script = hook_script();
        assert_eq!(
            script.matches(DELIMITER).count() % 2,
            0,
            "an unbalanced heredoc delimiter"
        );
        assert!(script.contains(&format!("<<'{DELIMITER}'")));
    }
}
