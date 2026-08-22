//! A run cut short during a `devpod up`, judged at the binary boundary.
//!
//! The concurrency review found (F2/H4/R8, F3) that `dl`'s `_exit` signal handler
//! ran no destructors, so a SIGINT during the minutes-long `devpod up` left the
//! plaintext GitHub-token file (`$TMPDIR/devlaunch-gh-*.env`, mode 0600) on disk
//! and orphaned the `up` child — which then went on running after the launch lock
//! `dl` held had already been released. Python's unwinding `KeyboardInterrupt`
//! cleaned both up. #304 then found the same pair reachable through two doors
//! SIGINT's handler did not cover: `kill <dl>` and a closed terminal window.
//!
//! These tests reproduce that exact moment with a fake `devpod` whose `up` blocks
//! forever, so the signal lands while the token is staged and the child is live,
//! and assert the fix for each signal that handles it: the process exits
//! 128 + signo, the token file is gone and the `up` child is dead. The last of
//! them covers the asymmetry in the inherited-`SIG_IGN` rule, which is the one
//! part of this that is a judgement rather than a mechanism.
//!
//! Linux-only (as the whole port is, #254): it reads `/proc`-style liveness
//! through `kill -0` and derives paths the same way the sibling suites do.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use devlaunch_test_support::KeepingCoverage;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the repository root")
}

/// A scratch world of the same shape `dl/tests/launch.rs` builds, with `--gh` so
/// a token is staged, then its `devpod` replaced by one that blocks on `up`.
struct World {
    root: PathBuf,
    _scratch: tempfile::TempDir,
}

impl World {
    fn blocking_up() -> Self {
        let scratch = tempfile::Builder::new()
            .prefix("dlint")
            .tempdir_in("/tmp")
            .expect("a scratch directory under /tmp");
        let root = scratch.path().to_path_buf();
        let built = Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/launch_scenario.py"))
            .arg(&root)
            .arg(repo_root().join("test/fixtures/devpod_shim.py"))
            .arg("--gh")
            .output()
            .expect("python3 is installed");
        assert!(
            built.status.success(),
            "launch_scenario.py failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );

        // Replace the fake `devpod`: `up` records its own pid and then blocks,
        // every other subcommand delegates to the shim the scenario installed.
        // The original is `#!/bin/sh` + one `exec <python> <shim> "$@"` line, and
        // the delegate reuses that exact line so `status`/`list`/`ssh` behave as
        // before.
        let devpod = root.join("bin/devpod");
        let original = std::fs::read_to_string(&devpod).expect("the scenario's devpod");
        let delegate = original
            .lines()
            .find(|line| line.starts_with("exec "))
            .expect("the delegate exec line");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"up\" ]; then\n\
             \x20 echo \"$$\" > \"$DL_UP_PID\"\n\
             \x20 : > \"$DL_UP_STARTED\"\n\
             \x20 exec sleep 30\n\
             fi\n\
             {delegate}\n"
        );
        std::fs::write(&devpod, script).expect("rewrite devpod");
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&devpod, std::fs::Permissions::from_mode(0o755))
            .expect("keep devpod executable");

        std::fs::create_dir_all(root.join("tmp")).expect("a scratch TMPDIR");
        World {
            root,
            _scratch: scratch,
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
}

/// Whether pid `pid` still exists, via `kill -0` — waiting briefly, since a
/// killed child is reaped a moment after the signal.
fn is_dead(pid: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let alive = Command::new("kill")
            .args(["-0", pid])
            .output()
            .expect("kill is installed")
            .status
            .success();
        if !alive {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The one staged GitHub-token file under `dir`, if any.
fn token_file(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        let path = entry.path();
        let name = path.file_name()?.to_string_lossy().into_owned();
        (name.starts_with("devlaunch-gh-") && name.ends_with(".env")).then_some(path)
    })
}

fn wait_for(mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

/// What a signalled `dl` left behind — the three facts every one of these tests
/// judges, gathered as one value so a test reads as a single expectation.
#[derive(Debug, PartialEq, Eq)]
struct Aftermath {
    /// The exit code, or `None` for a `dl` that died *by* the signal instead of
    /// exiting. The difference is observable to whatever spawned `dl`, which is
    /// why the code is asserted rather than just "it stopped".
    code: Option<i32>,
    /// Whether the plaintext GitHub-token file is still on disk.
    token_left: bool,
    /// Whether the `devpod up` child outlived the `dl` that started it.
    up_alive: bool,
}

/// A `dl` held at the exact moment the leak needs: `devpod up` blocking, the
/// plaintext token staged, the `up` child alive in a process group of its own.
/// Every signal these tests deliver is delivered here.
struct MidUp {
    _world: World,
    child: std::process::Child,
    tmpdir: PathBuf,
    up: String,
}

impl MidUp {
    /// A `dl` reached the ordinary way, with every signal at its default
    /// disposition.
    fn reached() -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dl"));
        command.arg("blooop/devlaunch@cold");
        Self::spawned(command)
    }

    /// A `dl` reached through a shell that first sets `signal` to be ignored: what
    /// `nohup` does to SIGHUP, and what a non-interactive shell does to SIGINT for
    /// a job it backgrounds. The disposition survives the `exec`, so `dl` starts
    /// with that signal already ignored, in the same process the shell occupied.
    ///
    /// One caveat this is the right place to record, since it is what one row of
    /// `INHERITED_IGNORE` turns on: the ignore reaches everything `dl` goes on to
    /// spawn, not just `dl`.
    fn reached_with_signal_ignored(signal: &str) -> Self {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(format!("trap '' {signal}; exec \"$@\""))
            .arg("sh")
            .arg(env!("CARGO_BIN_EXE_dl"))
            .arg("blooop/devlaunch@cold");
        Self::spawned(command)
    }

    fn spawned(mut command: Command) -> Self {
        let world = World::blocking_up();
        let root = world.root.display().to_string();
        let tmpdir = world.path("tmp");
        let up_pid = world.path("up.pid");
        let up_started = world.path("up.started");

        let child = command
            .env_clear()
            .keeping_coverage()
            .env("PATH", format!("{root}/bin:{root}/gh-bin:/usr/bin:/bin"))
            .env("HOME", format!("{root}/home"))
            .env("XDG_CACHE_HOME", format!("{root}/cache"))
            .env("XDG_CONFIG_HOME", format!("{root}/config"))
            .env("DEVPOD_HOME", format!("{root}/devpod"))
            .env("DEVPOD_SHIM_STATE", format!("{root}/shim-state.json"))
            .env("DEVPOD_SHIM_LOG", format!("{root}/shim-log.jsonl"))
            .env("DEVPOD_SHIM_CONFIG", format!("{root}/shim-config.json"))
            // The token is staged under TMPDIR, so pointing it at the scratch
            // tree is what lets these tests both find the file and prove it is
            // gone.
            .env("TMPDIR", tmpdir.display().to_string())
            .env("DL_UP_PID", up_pid.display().to_string())
            .env("DL_UP_STARTED", up_started.display().to_string())
            .env("GIT_SSH_COMMAND", "false")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .spawn()
            .expect("the dl binary runs");

        // Wait until `devpod up` is blocking and the token has been staged: both
        // are the preconditions the leak needs, so a test that signalled earlier
        // would prove nothing.
        assert!(
            wait_for(|| up_started.exists() && token_file(&tmpdir).is_some()),
            "devpod up never blocked with a token staged"
        );
        assert!(
            token_file(&tmpdir).is_some(),
            "the token is on disk before the signal"
        );
        let up = std::fs::read_to_string(&up_pid).expect("the up pid");
        MidUp {
            _world: world,
            child,
            tmpdir,
            up: up.trim().to_string(),
        }
    }

    /// Send `signal` to `dl` alone — the way a terminal Ctrl-C reaches `dl`'s
    /// group while the `up`, now in a group of its own, is spared, and the way a
    /// `kill <dl>` from another shell arrives.
    fn send(&self, signal: &str) {
        assert!(
            Command::new("kill")
                .args([&format!("-{signal}"), &self.child.id().to_string()])
                .status()
                .expect("kill is installed")
                .success(),
            "sending SIG{signal} to dl"
        );
    }

    /// Whether `dl` is still running after `grace` — for the signal that must
    /// *not* end it.
    fn survives(&mut self, grace: Duration) -> bool {
        std::thread::sleep(grace);
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Deliver `signal` and report what the run left behind.
    fn signalled(mut self, signal: &str) -> Aftermath {
        self.send(signal);
        let status = self.child.wait().expect("dl exits");
        Aftermath {
            code: status.code(),
            token_left: token_file(&self.tmpdir).is_some(),
            up_alive: !is_dead(&self.up),
        }
    }
}

/// The drain ran and left nothing behind, ending on `code`.
fn drained(code: i32) -> Aftermath {
    Aftermath {
        code: Some(code),
        token_left: false,
        up_alive: false,
    }
}

#[test]
fn a_ctrl_c_mid_up_removes_the_token_file_and_kills_the_up() {
    assert_eq!(MidUp::reached().signalled("INT"), drained(130));
}

#[test]
fn a_kill_mid_up_removes_the_token_file_and_kills_the_up() {
    // `kill <dl>` — a supervisor timing a run out, a CI job being cancelled, a
    // shell shutting down — is the same moment as a Ctrl-C and leaked the same
    // pair, because only SIGINT was handled. 143 is 128 + SIGTERM.
    assert_eq!(MidUp::reached().signalled("TERM"), drained(143));
}

#[test]
fn closing_the_terminal_mid_up_removes_the_token_file_and_kills_the_up() {
    // Closing the terminal window is a SIGHUP, and it left the same token on
    // disk and the same build orphaned — with nobody watching, since the window
    // it would have been reported in is the one that just went away. 129 is
    // 128 + SIGHUP.
    assert_eq!(MidUp::reached().signalled("HUP"), drained(129));
}

/// What an inherited `SIG_IGN` buys one signal.
enum Ignored {
    /// The inherited ignore loses: the signal drains anyway, exiting this code.
    StillDrains(i32),
    /// The inherited ignore wins: the signal ends nothing, and the run is left for
    /// a Ctrl-C to finish — which drains at 130 and reaches the `devpod up` child,
    /// unless `up_survives` says the disarming reached the child too.
    Honoured { up_survives: bool },
}

/// The inherited-ignore rule stated as data, one row per signal `dl` handles —
/// which is what the SIGINT half was missing when it slipped through once.
///
/// **This list cannot see the one it mirrors.** An integration test is its own
/// crate, so the handled-signal set in `dl`'s library is private to it (measured:
/// `constant DRAINED is private`), and nothing here can check the two agree. What
/// stands in for that is a guard on the far side: the `drained_signals` unit test
/// beside that set pins its exact contents, so a signal added there fails a test
/// whose message names *this* table. So the guarantee is "the set cannot grow in
/// silence", not "the two lists cannot disagree" — and a row deleted from here
/// while the set stays put is caught by nothing but review.
const INHERITED_IGNORE: [(&str, Ignored); 3] = [
    ("INT", Ignored::StillDrains(130)),
    // The one row whose child outlives the Ctrl-C, and not because of anything
    // `dl` decides: `trap '' TERM` is inherited by everything `dl` spawns, and the
    // drain fells the build with a `killpg(…, SIGTERM)`. Disarming SIGTERM for the
    // run therefore disarms the drain's own reach into the child — inherent to
    // killing a group with the signal the caller switched off, and true of any
    // program that tears its children down that way.
    ("TERM", Ignored::Honoured { up_survives: true }),
    ("HUP", Ignored::Honoured { up_survives: false }),
];

#[test]
fn an_inherited_ignore_wins_for_the_two_signals_that_mean_it_and_loses_for_ctrl_c() {
    for (signal, expected) in INHERITED_IGNORE {
        let mut run = MidUp::reached_with_signal_ignored(signal);
        match expected {
            // Ctrl-C is left exactly as it was, and an inherited ignore is where
            // that matters most: a non-interactive shell backgrounding a job hands
            // its child an ignored SIGINT under POSIX job control, with nobody
            // having asked for it. Honouring it there would abandon the staged
            // credential and the build — the very leak this closes — in the case
            // where nobody is watching for it.
            Ignored::StillDrains(code) => assert_eq!(
                run.signalled(signal),
                drained(code),
                "a SIG{signal} inherited as ignored must still drain"
            ),
            // For the two signals this branch adds, an inherited ignore is a
            // statement: `nohup dl …` disarms SIGHUP precisely so the run outlives
            // the terminal, and draining on it would take that away.
            Ignored::Honoured { up_survives } => {
                run.send(signal);
                assert!(
                    run.survives(Duration::from_millis(500)),
                    "a SIG{signal} inherited as ignored must not end the run"
                );
                // Disarming one signal disarms only that one: the run is still
                // reachable, and still cleans up, through the ones it did not.
                assert_eq!(
                    run.signalled("INT"),
                    Aftermath {
                        code: Some(130),
                        token_left: false,
                        up_alive: up_survives,
                    },
                    "after SIG{signal} was disarmed the run must still answer Ctrl-C"
                );
            }
        }
    }
}

/// A `--warm` world whose `devpod ssh` blocks, so an interrupt can land *during the
/// session* rather than during the build.
fn blocking_session() -> (tempfile::TempDir, PathBuf) {
    let scratch = tempfile::Builder::new()
        .prefix("dlint")
        .tempdir_in("/tmp")
        .expect("a scratch directory under /tmp");
    let root = scratch.path().to_path_buf();
    let built = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/launch_scenario.py"))
        .arg(&root)
        .arg(repo_root().join("test/fixtures/devpod_shim.py"))
        .arg("--warm")
        .output()
        .expect("python3 is installed");
    assert!(
        built.status.success(),
        "launch_scenario.py failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let devpod = root.join("bin/devpod");
    let original = std::fs::read_to_string(&devpod).expect("the scenario's devpod");
    let delegate = original
        .lines()
        .find(|line| line.starts_with("exec "))
        .expect("the delegate exec line");
    // `ssh` blocks; everything else delegates to the shim, so the log still records
    // the `status` probe and would record a `delete` if one were ever made.
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = \"ssh\" ]; then\n\
         \x20 : > \"$DL_SSH_STARTED\"\n\
         \x20 exec sleep 30\n\
         fi\n\
         {delegate}\n"
    );
    std::fs::write(&devpod, script).expect("rewrite devpod");
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&devpod, std::fs::Permissions::from_mode(0o755))
        .expect("keep devpod executable");
    (scratch, root)
}

#[test]
fn a_ctrl_c_that_reaches_dl_mid_session_leaves_a_throwaway_workspace_standing() {
    // The limit `--rm` documents, measured rather than reasoned about. dl's
    // SIGINT disposition is a signal handler, and a handler may not run a removal —
    // it cannot allocate, cannot lock, and does not return — so a SIGINT delivered
    // *to dl* ends the process before the removal it was going to make.
    //
    // What this does **not** say is that a terminal Ctrl-C reaches dl during an
    // ordinary session. It does not: both transports allocate a pty (`ssh -t`, and a
    // bare `devpod ssh`), which puts the local terminal in raw mode and clears
    // ISIG, so Ctrl-C travels to the remote program as a byte and dl never sees a
    // signal. This test reaches dl the way a Ctrl-C during the *build* does — before
    // any pty exists — and that is the case the README calls best-effort.
    let (_scratch, root_path) = blocking_session();
    let root = root_path.display().to_string();
    let ssh_started = root_path.join("ssh.started");

    let mut child = Command::new(env!("CARGO_BIN_EXE_dl"))
        .args(["devlaunch-main-zovomobo", "--rm"])
        .env_clear()
        .keeping_coverage()
        .env("PATH", format!("{root}/bin:{root}/gh-bin:/usr/bin:/bin"))
        .env("HOME", format!("{root}/home"))
        .env("XDG_CACHE_HOME", format!("{root}/cache"))
        .env("XDG_CONFIG_HOME", format!("{root}/config"))
        .env("DEVPOD_HOME", format!("{root}/devpod"))
        .env("DEVPOD_SHIM_STATE", format!("{root}/shim-state.json"))
        .env("DEVPOD_SHIM_LOG", format!("{root}/shim-log.jsonl"))
        .env("DEVPOD_SHIM_CONFIG", format!("{root}/shim-config.json"))
        .env("DEVLAUNCH_NO_GH_TOKEN", "1")
        .env("DL_SSH_STARTED", ssh_started.display().to_string())
        .env("GIT_SSH_COMMAND", "false")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .spawn()
        .expect("the dl binary runs");

    assert!(
        wait_for(|| ssh_started.exists()),
        "the session never started, so the interrupt would prove nothing"
    );
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .expect("kill is installed")
            .success(),
        "sending SIGINT to dl"
    );
    let status = child.wait().expect("dl exits");
    assert_eq!(status.code(), Some(130), "dl's own interrupted ending");

    let log = std::fs::read_to_string(root_path.join("shim-log.jsonl")).unwrap_or_default();
    assert!(
        !log.contains("\"delete\""),
        "the removal ran from a signal handler, which cannot be: {log}"
    );
    assert!(
        root_path
            .join("cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-zovomobo")
            .exists(),
        "the clone went without a removal having run"
    );
}
