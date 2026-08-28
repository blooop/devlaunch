//! What a captured child may do with the terminal, judged on a real pty.
//!
//! A capture pipes stdout and stderr, but not stdin: an inherited stdin is the
//! default, and `/dev/tty` is reachable whatever stdin is. So the terminal is
//! part of a capture's contract in two ways that only a controlling terminal can
//! show:
//!
//! - the child may **read** it — ssh's host-key confirmation, ssh's passphrase
//!   prompt and git's credential prompt all do, and three captures pass no
//!   timeout at all (`git clone --bare`, `git push -u`, the launch-path fetch),
//!   so a child that stops on SIGTTIN instead hangs `dl` for good;
//! - a **Ctrl-C** at that terminal reaches it, which is what stops `dl`'s
//!   `_exit` from leaving an unsignalled `git fetch` writing the bare cache with
//!   the repo lock already released (concurrency review F3).
//!
//! Both are properties of the process group the child lands in, and a `cargo
//! test` process has no controlling terminal to observe them from. So each test
//! here opens a pty and re-executes *this binary* on it — the session leader on a
//! pty is what a capture's child inherits its group from — then types at it the
//! way a person would. Same shape as `aid/tests/interactive.rs`, which drives a
//! real `aid` through a pty for the same reason.

use std::io::{Read as _, Write as _};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devlaunch_runner::{Invocation, ProcessRunner, Runner};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

/// Which side of the test a copy of this binary is: set, it is the one on the pty
/// running the capture; unset, it is the test driving one.
///
/// This binary re-executed rather than a separate example or helper binary,
/// because the coverage run is `cargo llvm-cov`, which spawns `cargo test
/// --tests` — and that builds no examples. A helper that is missing under one of
/// the two CI runs is worse than one that cannot go missing.
const ROLE: &str = "DEVLAUNCH_TEST_TERMINAL_ROLE";

/// Where the interrupt test's captured child would leave its mark, if anything
/// let it get that far.
const MARKER: &str = "DEVLAUNCH_TEST_TERMINAL_MARKER";

/// The shell, as in the unit tests: POSIX `/bin/sh`, nothing bash-only.
fn sh(script: &str) -> Invocation {
    Invocation::new("/bin/sh").with_arg("-c").with_arg(script)
}

/// The pty side of the test: one capture from inside a session that owns a
/// controlling terminal, with what came back printed for the test to read.
fn capture_beside_the_terminal(role: &str) {
    let what = match role {
        // Reads the controlling terminal, the way ssh's host-key confirmation,
        // ssh's passphrase prompt and git's credential prompt all do (through
        // `/dev/tty`, whatever stdin is). A child outside the terminal's
        // foreground process group takes SIGTTIN on that read and stops — and a
        // stopped child is not an exited one, so the wait never ends.
        "read-tty" => sh("printf 'prompt: ' >&2; head -c 3 /dev/tty"),
        // Outlives the Ctrl-C the test types, if anything lets it: the marker
        // appears only if the child never got the terminal's SIGINT.
        "outlive-interrupt" => {
            let marker = std::env::var(MARKER).expect("a marker path");
            sh(&format!("sleep 2; : > {marker}"))
        }
        other => panic!("unknown role {other:?}"),
    };
    println!("started");
    println!("outcome: {:?}", ProcessRunner.capture(&what.into()));
}

/// The bytes a killed child leaves switched on, and the bytes that undo them.
///
/// The push is the kitty keyboard protocol's, because that is the mode whose
/// absence is silent: with it stranded, echo and line editing still work and yet
/// Ctrl-C arrives at the shell as `ESC [ 99;5 u` rather than as the byte the line
/// discipline turns into SIGINT.
const KEYBOARD_PUSH: &str = "\x1b[>1u";
const KEYBOARD_POP: &str = "\x1b[<u";

/// What the child says before it dies, so the test can tell "the restore came
/// after the child" from "the restore came instead of it".
const CHILD_DONE: &str = "CHILD-DONE";

/// The passthrough side: a child that takes the terminal, switches a mode on, and
/// is then killed without ever switching it off.
///
/// `kill -9 $$` rather than a plain exit, because that is the case from the
/// report: the container goes away underneath an `ssh -t` session and the agent
/// inside it dies with no chance to clean up. A child that exits normally would
/// prove the restore runs, but not that it runs when it is the only thing left to
/// run.
fn passthrough_beside_the_terminal() {
    // `\033[>1u` spelled the way `printf` reads it: the same bytes as
    // [`KEYBOARD_PUSH`], which is what the assertions look for.
    let what = sh(&format!(
        "printf '\\033[>1u'; printf '{CHILD_DONE}\\n'; kill -9 $$"
    ));
    println!("started");
    let outcome = ProcessRunner.passthrough(&what.into());
    println!("outcome: {outcome:?}");
}

/// This binary on a pty: the child, a way to type at it, and everything it said.
struct Pty {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn std::io::Write + Send>,
    seen: Arc<Mutex<Vec<u8>>>,
    // Held so the master side outlives the child; dropping it would hang up the
    // terminal underneath it.
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl Pty {
    /// Re-execute this binary on a pty, running `test` alone and in `role`.
    /// `--nocapture` so what it prints reaches the terminal rather than libtest's
    /// buffer.
    fn spawn(test: &str, role: &str, marker: Option<&str>) -> Self {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("a pty");
        let mut command = CommandBuilder::new(std::env::current_exe().expect("this test binary"));
        command.args(["--exact", test, "--nocapture", "--test-threads=1"]);
        command.env(ROLE, role);
        if let Some(marker) = marker {
            command.env(MARKER, marker);
        }
        let child = pty
            .slave
            .spawn_command(command)
            .expect("this binary spawns on the pty");
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader().expect("the pty reader");
        let writer = pty.master.take_writer().expect("the pty writer");
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
        Pty {
            child,
            writer,
            seen,
            _master: pty.master,
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.seen.lock().expect("the pty buffer")).into_owned()
    }

    /// Wait for `needle` to appear, or panic with everything that did.
    fn expect(&self, needle: &str, why: &str) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if self.text().contains(needle) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!(
            "{needle:?} never appeared: {why}. The pty said:\n{}",
            self.text()
        );
    }

    /// Type a line and press Enter, the way a person at the terminal would.
    fn send_line(&mut self, line: &str) {
        self.writer
            .write_all(line.as_bytes())
            .and_then(|()| self.writer.write_all(b"\n"))
            .and_then(|()| self.writer.flush())
            .expect("typing into the pty");
    }

    /// A terminal Ctrl-C: the byte the line discipline turns into SIGINT for the
    /// whole foreground process group.
    fn interrupt(&mut self) {
        self.writer
            .write_all(b"\x03")
            .and_then(|()| self.writer.flush())
            .expect("interrupting the pty");
    }

    /// Wait for the child to exit, or panic — a test must not leave a process of
    /// its own behind, and the interesting failure here is a hang.
    fn wait_for_exit(&mut self, why: &str) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                _ => std::thread::sleep(Duration::from_millis(25)),
            }
        }
        panic!("the child never exited: {why}");
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // A red run here is a hang by construction, so the child is still there.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A capture's child stays in this process's group, so it is still part of the
/// terminal's foreground group and its read of `/dev/tty` is served rather than
/// stopped with SIGTTIN.
///
/// Measured against the process-group build this replaces (#301, #302): there the
/// capture never returned and the child sat in state `T`.
#[test]
fn a_captured_child_may_read_the_terminal() {
    if let Ok(role) = std::env::var(ROLE) {
        return capture_beside_the_terminal(&role);
    }
    let mut pty = Pty::spawn("a_captured_child_may_read_the_terminal", "read-tty", None);
    pty.expect("started", "the child never started its capture");
    // The prompt the captured child writes goes to its stderr *pipe*, not to the
    // terminal, so there is nothing more on the pty to wait for — and the line
    // discipline holds what is typed until the child reads it.
    pty.send_line("yes");
    pty.expect(
        "outcome: ",
        "the capture never returned — a child outside the terminal's foreground \
         process group takes SIGTTIN on a terminal read, and a stopped child never \
         exits",
    );
    let said = pty.text();
    assert!(
        said.contains(r#"stdout: "yes""#) && said.contains(r#"stderr: "prompt: ""#),
        "the child did not read what was typed:\n{said}"
    );
    pty.wait_for_exit("after the capture returned");
}

/// A terminal Ctrl-C reaches a capture's child, because the child is in the
/// foreground process group the line discipline signals. `capture` — unlike
/// `passthrough` — never notes a foreground child for the interrupt handler to
/// `killpg`, and `dl`'s handler `_exit`s without waiting, so group membership is
/// the *only* thing that kills the child.
///
/// Measured against the process-group build this replaces: the child outlived the
/// interrupt and left the marker.
#[test]
fn a_terminal_interrupt_reaches_a_captured_child() {
    if let Ok(role) = std::env::var(ROLE) {
        return capture_beside_the_terminal(&role);
    }
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let marker = scratch.path().join("survived");
    let mut pty = Pty::spawn(
        "a_terminal_interrupt_reaches_a_captured_child",
        "outlive-interrupt",
        Some(&marker.display().to_string()),
    );
    pty.expect("started", "the child never started its capture");
    pty.interrupt();
    pty.wait_for_exit("it ignored the terminal's SIGINT");
    // The captured child would touch the marker two seconds in; give it three.
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        !marker.exists(),
        "the captured child outlived the terminal's Ctrl-C: nothing else kills it"
    );
}

/// A child that is killed while holding the terminal leaves modes switched on
/// that only it was ever going to switch off, so `passthrough` switches them off
/// as soon as it reaps the child.
///
/// This is the reported failure in miniature: `dl` attaches over `ssh -t`, the
/// container goes away underneath the session (`error tunneling to container:
/// exit status 137`), and the agent inside dies without popping the kitty
/// keyboard protocol it pushed on the way in. ssh restores the tty *settings* on
/// its way out, so the terminal still echoes and still edits lines, and Ctrl-C
/// still does nothing.
///
/// The ordering is the assertion, not just the presence: the restore has to land
/// after the child's own output and before anything `dl` prints next, or a
/// session that stranded the alternate screen would have `dl`'s closing notices
/// drawn into a buffer nobody can see.
#[test]
fn a_killed_child_does_not_leave_the_terminal_switched_into_its_own_modes() {
    if std::env::var(ROLE).is_ok() {
        return passthrough_beside_the_terminal();
    }
    let mut pty = Pty::spawn(
        "a_killed_child_does_not_leave_the_terminal_switched_into_its_own_modes",
        "restore-after-passthrough",
        None,
    );
    pty.expect("started", "the child never started its passthrough");
    pty.expect(
        "outcome: ",
        "the passthrough never returned after the child was killed",
    );
    pty.wait_for_exit("after the passthrough returned");

    let said = pty.text();
    let pushed = said.find(KEYBOARD_PUSH).unwrap_or_else(|| {
        panic!("the child never switched the mode on, so this proves nothing:\n{said:?}")
    });
    let done = said.find(CHILD_DONE).expect("the child's own output");
    let popped = said.find(KEYBOARD_POP).unwrap_or_else(|| {
        panic!(
            "the terminal was left in the killed child's keyboard mode: nothing \
             wrote {KEYBOARD_POP:?}, so Ctrl-C at this terminal would arrive as an \
             escape sequence rather than as SIGINT.\nThe pty saw:\n{said:?}"
        )
    });
    let printed_after = said.find("outcome: ").expect("the parent's next line");
    assert!(
        pushed < done && done < popped && popped < printed_after,
        "the restore is out of order: push at {pushed}, child output at {done}, \
         pop at {popped}, the next thing dl printed at {printed_after}.\nThe pty \
         saw:\n{said:?}"
    );
}

/// With stdout on a pipe there is no terminal to repair, and writing the restore
/// anyway would corrupt whatever is reading — `dl <ws> -- ls > out.txt` is a real
/// invocation, and its output has to stay free of escape sequences.
///
/// Same child as the pty test, run with no terminal anywhere near it.
#[test]
fn nothing_is_written_when_the_streams_are_not_a_terminal() {
    if std::env::var(ROLE).is_ok() {
        return passthrough_beside_the_terminal();
    }
    let output = std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .args([
            "--exact",
            "nothing_is_written_when_the_streams_are_not_a_terminal",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ROLE, "restore-after-passthrough")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("this binary runs off a terminal too");
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(
        said.contains(CHILD_DONE),
        "the child never ran, so this proves nothing:\n{said:?}"
    );
    assert!(
        !said.contains(KEYBOARD_POP),
        "a restore was written to a pipe, where there is no terminal to restore \
         and no one who wants escape sequences in their output:\n{said:?}"
    );
}
