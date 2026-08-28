//! What `dl` leaves the terminal in, judged on a terminal.
//!
//! `devlaunch-runner`'s own `tests/terminal.rs` proves the repair at the seam that
//! writes it: a child takes the terminal, is killed, and `passthrough` puts the
//! modes back. It cannot prove that a `dl` *run* reaches that seam. Everything
//! between is real code with real ways to fail — the transport `dl` picks, whether
//! a session goes through `passthrough` or `session` at all, whether the binary
//! that ships is built from the crate that has the repair in it — and every one of
//! those failures leaves the runner's tests green and the user's terminal broken.
//!
//! So this file runs the shipped binary on a pty, gives it a devpod whose session
//! dies the way the reported one died, and reads back the bytes that reached the
//! terminal. Same seam `tests/picker.rs` uses and for the same reason: the thing
//! under test is what a person's terminal receives, and nothing below the process
//! boundary can answer for it.
//!
//! The session here takes the `devpod ssh` transport, because the scratch world
//! publishes no ssh host alias. That is the transport with *less* of `dl` in front
//! of it, so a run that reaches the repair through it reaches it through the pty
//! transport too, which shares the tail.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

/// The workspace id this build derives for `blooop/devlaunch@main`, and the one
/// `launch_scenario.py` records and the fake devpod knows.
const MAIN: &str = "devlaunch-main-3j1t";

/// What the dying session pushes, and what has to undo it.
///
/// The kitty keyboard protocol, because it is the mode whose absence is silent:
/// with it stranded the shell still echoes and still edits lines, and Ctrl-C
/// arrives as `ESC [ 99;5 u` rather than as the byte that raises SIGINT.
const KEYBOARD_PUSH: &str = "\x1b[>1u";
const KEYBOARD_POP: &str = "\x1b[<u";

/// What the session says before it dies, so "the repair came after the session"
/// can be told from "the repair came instead of one".
const SESSION_SAID: &str = "REMOTE-PUSHED";

/// Long enough for a process spawn and a debug-profile binary's startup. The wait
/// below stops as soon as the pty has what it is waiting for, so this costs
/// nothing when things work.
const DEADLINE: Duration = Duration::from_secs(60);

#[test]
fn a_session_that_dies_holding_the_terminal_gives_it_back() {
    let world = World::warm();
    // The failure from the report, at this boundary: the session writes a mode to
    // the terminal and then ends without undoing it. `dl` cannot tell that from a
    // session that cleaned up after itself, which is the whole reason the repair
    // is unconditional.
    world.devpod_answers_ssh_with(
        &format!("{KEYBOARD_PUSH}{SESSION_SAID}\n"),
        // 255 is what OpenSSH exits with when the connection breaks under it, and
        // what devpod's own `exit status 137` tunnel failure surfaces as.
        255,
    );

    let seen = world.dl_on_a_terminal(&[MAIN, "--", "true"]);

    let pushed = seen.find(KEYBOARD_PUSH).unwrap_or_else(|| {
        panic!("the session never reached the terminal, so this proves nothing:\n{seen:?}")
    });
    let said = seen.find(SESSION_SAID).expect("the session's own output");
    let popped = seen.find(KEYBOARD_POP).unwrap_or_else(|| {
        panic!(
            "dl exited leaving the terminal in the dead session's keyboard mode: \
             nothing wrote {KEYBOARD_POP:?}, so Ctrl-C at this terminal would arrive \
             as an escape sequence rather than as SIGINT.\nThe terminal saw:\n{seen:?}"
        )
    });
    assert!(
        pushed < said && said < popped,
        "the repair has to land after the session it repairs, not before: push at \
         {pushed}, session output at {said}, repair at {popped}.\nThe terminal \
         saw:\n{seen:?}"
    );
}

// ===========================================================================
// the world
// ===========================================================================

/// One scratch world: `launch_scenario.py`'s, as `tests/launch.rs` builds it, with
/// the fake devpod on PATH under its real name.
struct World {
    root: PathBuf,
    _scratch: tempfile::TempDir,
}

impl World {
    /// A world whose workspace already exists and is running, so `dl <ws> -- <cmd>`
    /// is one `devpod status` and then the session.
    fn warm() -> Self {
        let scratch = tempfile::Builder::new()
            .prefix("dlt")
            .rand_bytes(6)
            .tempdir_in("/tmp")
            .expect("a scratch directory under /tmp");
        let root = scratch.path().to_path_buf();
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .canonicalize()
            .expect("the repository root");
        let built = std::process::Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/launch_scenario.py"))
            .arg(&root)
            .arg(repo_root.join("test/fixtures/devpod_shim.py"))
            .arg("--warm")
            .output()
            .expect("python3 is installed");
        assert!(
            built.status.success(),
            "launch_scenario.py failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        World {
            root,
            _scratch: scratch,
        }
    }

    /// Make the fake devpod answer the session call with these bytes and this
    /// status, instead of running its workspace state machine.
    fn devpod_answers_ssh_with(&self, stdout: &str, returncode: i32) {
        let document = serde_json::json!({
            "responses": [{
                "prefix": ["ssh", MAIN],
                "stdout": stdout,
                "returncode": returncode,
            }],
        });
        std::fs::write(
            self.root.join("shim-config.json"),
            serde_json::to_string(&document).expect("a shim config"),
        )
        .expect("the shim config is written");
    }

    /// Run `dl` on a pty and hand back every byte that reached the terminal.
    ///
    /// The environment is `tests/launch.rs`'s, which is what makes the run
    /// hermetic; `TERM` is the one addition, as in `tests/picker.rs`, since this
    /// run has a terminal.
    fn dl_on_a_terminal(&self, args: &[&str]) -> String {
        let root = self.root.display().to_string();
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_dl"));
        for argument in args {
            command.arg(argument);
        }
        command.env_clear();
        // `KeepingCoverage` is a `std::process::Command` trait and this is not one,
        // so the one variable it would have re-admitted is passed by hand. Without
        // it a coverage run records nothing for this test.
        if let Ok(profile) = std::env::var("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        command.env("PATH", format!("{root}/bin:/usr/bin:/bin"));
        command.env("HOME", format!("{root}/home"));
        command.env("XDG_CACHE_HOME", format!("{root}/cache"));
        command.env("XDG_CONFIG_HOME", format!("{root}/config"));
        command.env("DEVPOD_HOME", format!("{root}/devpod"));
        command.env("DEVPOD_SHIM_STATE", format!("{root}/shim-state.json"));
        command.env("DEVPOD_SHIM_CONFIG", format!("{root}/shim-config.json"));
        command.env("TERM", "xterm-256color");
        command.env("GIT_SSH_COMMAND", "false");
        command.env("GIT_CONFIG_GLOBAL", "/dev/null");
        command.env("GIT_CONFIG_SYSTEM", "/dev/null");
        // No gh in this world, so no "no GitHub login" line and no token trip.
        command.env("DEVLAUNCH_NO_GH_TOKEN", "1");

        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("a pty");
        let mut child = pty
            .slave
            .spawn_command(command)
            .expect("the dl binary spawns on the pty");
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader().expect("the pty reader");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while let Ok(read) = reader.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                sink.lock()
                    .expect("the pty buffer")
                    .extend_from_slice(&chunk[..read]);
            }
        });

        let deadline = Instant::now() + DEADLINE;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                _ if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "dl never exited. The terminal saw:\n{}",
                        String::from_utf8_lossy(&seen.lock().expect("the pty buffer"))
                    );
                }
                _ => std::thread::sleep(Duration::from_millis(25)),
            }
        }
        // The reader thread is still draining what the child wrote on its way out.
        std::thread::sleep(Duration::from_millis(200));
        String::from_utf8_lossy(&seen.lock().expect("the pty buffer")).into_owned()
    }
}
