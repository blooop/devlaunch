//! One capture, run from inside a session that owns a controlling terminal.
//!
//! The companion child of `tests/terminal.rs`. Two of this crate's promises are
//! about which process group a captured child lands in — it may read the
//! terminal, and a terminal Ctrl-C reaches it — and neither is observable from a
//! `cargo test` process, which has no controlling terminal at all. So the test
//! opens a pty, spawns this on it, and reads what it prints back.
//!
//! An example rather than a test helper because it has to be a *process*: the
//! session leader on the pty is the one whose process group a capture's child
//! inherits, and that leader is this.

use devlaunch_runner::{Invocation, ProcessRunner, Runner};

/// The shell, as in the unit tests: POSIX `/bin/sh`, nothing bash-only.
fn sh(script: &str) -> Invocation {
    Invocation::new("/bin/sh").with_arg("-c").with_arg(script)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("a mode");
    match mode.as_str() {
        // Reads the controlling terminal, the way ssh's host-key confirmation,
        // ssh's passphrase prompt and git's credential prompt all do (through
        // `/dev/tty`, whatever stdin is). A child outside the terminal's
        // foreground process group takes SIGTTIN on that read and stops, and a
        // stopped child is not an exited one — the wait never ends.
        "read-tty" => {
            let what = sh("printf 'prompt: ' >&2; head -c 3 /dev/tty");
            println!("outcome: {:?}", ProcessRunner.capture(&what.into()));
        }
        // Outlives the Ctrl-C the test types, if anything lets it: the marker
        // appears only if the child never got the terminal's SIGINT.
        "outlive-interrupt" => {
            let marker = args.next().expect("a marker path");
            println!("started");
            let what = sh(&format!("sleep 2; : > {marker}"));
            println!("outcome: {:?}", ProcessRunner.capture(&what.into()));
        }
        other => panic!("unknown mode {other:?}"),
    }
}
