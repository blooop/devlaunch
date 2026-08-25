//! The interactive default, judged on a real pty.
//!
//! `aid <workspace>` with no prompt on a terminal boots the workspace in the
//! background while the prompt is typed, then attaches with what was typed. A
//! terminal is the one thing `Command` cannot fake — the flow is gated on
//! `isatty` of stdin *and* stdout — so these tests drive the binary through a
//! pty and type at it the way a person would. The world is `dl`'s scenario with
//! `test/fixtures/devpod_shim.py` as devpod, same as `tests/rewrite.rs`; the
//! Ctrl-C case rebuilds `tests/interrupt.rs`'s blocking `up` on the pty.
//!
//! The non-terminal half of the contract — a piped or null stdin keeps the old
//! one-shot behaviour — is pinned in `tests/rewrite.rs`, whose runs have no tty
//! by construction.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

/// The workspace id this build derives for `blooop/devlaunch@main`, which the
/// scenario records and devpod knows.
const MAIN: &str = "devlaunch-main-3j1t";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the repository root")
}

/// One scratch world, the shape `tests/rewrite.rs` builds.
struct World {
    root: PathBuf,
    _scratch: tempfile::TempDir,
}

impl World {
    fn with(fixtures: &[&str]) -> Self {
        let scratch = tempfile::Builder::new()
            .prefix("aidpty")
            .tempdir_in("/tmp")
            .expect("a scratch directory under /tmp");
        let root = scratch.path().to_path_buf();
        let dl_tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dl/tests");
        let built = Command::new("python3")
            .arg(dl_tests.join("launch_scenario.py"))
            .arg(&root)
            .arg(repo_root().join("test/fixtures/devpod_shim.py"))
            .args(fixtures)
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

    /// The devpod calls made so far, in order, with `devpod list` left out — the
    /// detached completion refresh makes one of its own on its own schedule.
    ///
    /// Unlike `tests/rewrite.rs`'s copy, this one is polled *while* the shim is
    /// appending (the overlap test watches for the `up` mid-edit), so a line read
    /// half-written is skipped rather than panicked on: it is whole on the next
    /// poll, and by the time the post-exit assertions read the log nothing is
    /// still writing.
    fn devpod_calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("shim-log.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                let call: serde_json::Value = serde_json::from_str(line).ok()?;
                call["argv"]
                    .as_array()?
                    .iter()
                    .map(|word| word.as_str().map(str::to_owned))
                    .collect::<Option<Vec<String>>>()
            })
            .filter(|argv| argv.first().map(String::as_str) != Some("list"))
            .map(|argv| format!("devpod {}", argv.join(" ")))
            .collect()
    }
}

/// `aid` on a pty: the child, a way to type at it, and everything it printed.
struct PtyAid {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn std::io::Write + Send>,
    seen: Arc<Mutex<Vec<u8>>>,
    // Held so the master side outlives the session; dropping it would hang up
    // the terminal under the child.
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl PtyAid {
    fn spawn(world: &World, args: &[&str], extra: &[(&str, &str)]) -> Self {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("a pty");
        let root = world.root.display().to_string();
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_aid"));
        command.args(args);
        command.env_clear();
        // `KeepingCoverage` by hand: the trait extends `std::process::Command`,
        // and this builder is not one.
        if let Ok(profile) = std::env::var("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        command.env("PATH", format!("{root}/bin:{root}/gh-bin:/usr/bin:/bin"));
        command.env("HOME", format!("{root}/home"));
        command.env("XDG_CACHE_HOME", format!("{root}/cache"));
        command.env("XDG_CONFIG_HOME", format!("{root}/config"));
        command.env("DEVPOD_HOME", format!("{root}/devpod"));
        command.env("DEVPOD_SHIM_STATE", format!("{root}/shim-state.json"));
        command.env("DEVPOD_SHIM_LOG", format!("{root}/shim-log.jsonl"));
        command.env("DEVPOD_SHIM_CONFIG", format!("{root}/shim-config.json"));
        command.env("GIT_SSH_COMMAND", "false");
        command.env("GIT_CONFIG_GLOBAL", "/dev/null");
        command.env("GIT_CONFIG_SYSTEM", "/dev/null");
        if !world.root.join("gh-bin/gh").exists() {
            command.env("DEVLAUNCH_NO_GH_TOKEN", "1");
        }
        for (name, value) in extra {
            command.env(name, value);
        }
        let child = pty
            .slave
            .spawn_command(command)
            .expect("aid spawns on the pty");
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
        PtyAid {
            child,
            writer,
            seen,
            _master: pty.master,
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.seen.lock().expect("the pty buffer")).into_owned()
    }

    fn expect(&self, needle: &str) {
        assert!(
            wait_for(|| self.text().contains(needle)),
            "{needle:?} never appeared; the pty said:\n{}",
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

    fn wait(mut self) -> u32 {
        self.child.wait().expect("aid exits").exit_code()
    }
}

fn wait_for(mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

/// The words the editor's banner ends with — the string the tests key on, and
/// the one the e2e suite keys on too.
const BANNER: &str = "press Enter";

/// The OSC 2 title `dl` writes for [`MAIN`], bytes and all.
const TITLE: &str = "\x1b]2;devlaunch-main-3j1t\x07";

#[test]
fn the_terminal_really_is_named_on_a_real_pty() {
    // The one test that proves the bytes arrive. Everything else about the title
    // is judged on a `Host` value or a notice; this watches the escape come out of
    // the pty the launch was handed, through the shipped binary, which is the only
    // place the stderr-is-a-tty guard is exercised for real. An argv prompt rather
    // than a typed one so the editor is out of the way and the title is the only
    // thing under test.
    let world = World::with(&["--warm"]);
    let session = PtyAid::spawn(&world, &[MAIN, "fix", "the", "bug"], &[]);
    session.expect(TITLE);
    assert_eq!(session.wait(), 0);
}

#[test]
fn the_title_switch_really_silences_it_on_a_real_pty() {
    // Off means no escape at all, not an empty title: `ESC ] 2 ; BEL` would blank
    // the terminal's name, which is not what "leave it alone" means.
    let world = World::with(&["--warm"]);
    let session = PtyAid::spawn(
        &world,
        &[MAIN, "fix", "the", "bug"],
        &[("DEVLAUNCH_NO_TITLE", "1")],
    );
    // `SSH command:` is said from inside the session call, which the title is
    // written in front of -- so once this is on screen, a title that was coming
    // has already come and its absence means something.
    session.expect("SSH command:");
    let seen = session.text();

    assert!(
        !seen.contains("\x1b]2;"),
        "an OSC 2 was written anyway; the pty said:\n{seen:?}"
    );
    assert_eq!(session.wait(), 0);
}

#[test]
fn a_typed_prompt_reaches_the_agent_with_no_shell_in_the_way() {
    // The double quotes are the point: they reach the agent literally, because
    // the prompt never passes through a shell on the host — the escaping pain the
    // editor exists to end.
    let world = World::with(&["--warm"]);
    let mut session = PtyAid::spawn(&world, &[MAIN], &[]);
    session.expect(BANNER);
    session.send_line("fix the \"flaky\" test");
    session.expect("aid -> dl");
    assert_eq!(session.wait(), 0);
    assert_eq!(
        world.devpod_calls().last().expect("a session"),
        &format!(
            "devpod ssh {MAIN} --command bash -lc 'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions '\"'\"'fix the \"flaky\" test'\"'\"''"
        )
    );
}

#[test]
fn a_pasted_multi_line_prompt_arrives_whole_rather_than_leaking() {
    // A paste delivers its newlines with it, and the terminal holds the later
    // lines as completed input. The editor must drain them into the prompt: a
    // submission cut at the first newline would hand the agent line one and leave
    // the rest queued in the terminal, to land inside the agent's session as
    // keystrokes.
    let world = World::with(&["--warm"]);
    let mut session = PtyAid::spawn(&world, &[MAIN], &[]);
    session.expect(BANNER);
    // One write, as a terminal delivers a paste: both lines arrive together, so
    // the second is already queued when the first's Enter is read.
    session.send_line("fix this\nand then that");
    assert_eq!(session.wait(), 0);
    assert_eq!(
        world.devpod_calls().last().expect("a session"),
        &format!(
            "devpod ssh {MAIN} --command bash -lc 'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions '\"'\"'fix this\nand then that'\"'\"''"
        )
    );
}

#[test]
fn an_empty_enter_is_the_plain_session_it_always_was() {
    let world = World::with(&["--warm"]);
    let mut session = PtyAid::spawn(&world, &[MAIN], &[]);
    session.expect(BANNER);
    session.send_line("");
    assert_eq!(session.wait(), 0);
    assert_eq!(
        world.devpod_calls().last().expect("a session"),
        &format!(
            "devpod ssh {MAIN} --command bash -lc 'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions'"
        )
    );
}

#[test]
fn the_boot_runs_while_the_prompt_is_still_being_typed() {
    // The overlap itself: a stopped workspace's `devpod up` is on the shim's log
    // while the editor is still open — nothing has been typed yet — and the
    // attach that follows the Enter finds it running.
    let world = World::with(&["--stopped"]);
    let mut session = PtyAid::spawn(&world, &[MAIN], &[]);
    session.expect(BANNER);
    assert!(
        wait_for(|| {
            world
                .devpod_calls()
                .iter()
                .any(|call| call.starts_with("devpod up "))
        }),
        "the boot never asked devpod for an up while the editor was open: {:?}",
        world.devpod_calls()
    );
    session.send_line("go");
    assert_eq!(session.wait(), 0);
    assert_eq!(
        world.devpod_calls().last().expect("a session"),
        &format!(
            "devpod ssh {MAIN} --command bash -lc 'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions go'"
        )
    );
}

#[test]
fn a_ctrl_c_at_the_editor_tears_the_whole_boot_down() {
    // `tests/interrupt.rs` on the pty: the boot child is in aid's process group,
    // so the terminal's SIGINT reaches it, and its own handler kills the blocked
    // `devpod up` — now in a group of its own — and unlinks the staged token.
    let world = World::with(&["--gh"]);
    let devpod = world.root.join("bin/devpod");
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
    let tmpdir = world.root.join("tmp");
    std::fs::create_dir_all(&tmpdir).expect("a scratch TMPDIR");
    let up_pid = world.root.join("up.pid");
    let up_started = world.root.join("up.started");

    let mut session = PtyAid::spawn(
        &world,
        &["blooop/devlaunch@cold"],
        &[
            ("TMPDIR", &tmpdir.display().to_string()),
            ("DL_UP_PID", &up_pid.display().to_string()),
            ("DL_UP_STARTED", &up_started.display().to_string()),
        ],
    );
    session.expect(BANNER);
    // Interrupt only once the boot is mid-`up` with the token staged — the exact
    // state the interrupt handler exists to clean.
    assert!(
        wait_for(|| up_started.exists() && token_file(&tmpdir).is_some()),
        "devpod up never blocked with a token staged"
    );
    let up = std::fs::read_to_string(&up_pid).expect("the up pid");
    let up = up.trim().to_owned();

    session.interrupt();
    assert_eq!(session.wait(), 130, "aid exits 130 on interrupt");
    // The boot child cleans up on its own clock, a moment after the parent died.
    assert!(
        wait_for(|| token_file(&tmpdir).is_none()),
        "the token file must be gone after the interrupt"
    );
    assert!(
        wait_for(|| !Command::new("kill")
            .args(["-0", &up])
            .output()
            .expect("kill is installed")
            .status
            .success()),
        "the orphaned devpod up (pid {up}) must have been killed"
    );
}

/// The one staged GitHub-token file under `dir`, if any.
fn token_file(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        let path = entry.path();
        let name = path.file_name()?.to_string_lossy().into_owned();
        (name.starts_with("devlaunch-gh-") && name.ends_with(".env")).then_some(path)
    })
}
