//! The interactive default, judged on a real pty.
//!
//! `dla <workspace>` with no prompt on a terminal boots the workspace in the
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

/// `dla` on a pty: the child, a way to type at it, and everything it printed.
struct PtyDla {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn std::io::Write + Send>,
    seen: Arc<Mutex<Vec<u8>>>,
    // Held so the master side outlives the session; dropping it would hang up
    // the terminal under the child.
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl PtyDla {
    fn spawn(world: &World, args: &[&str], extra: &[(&str, &str)]) -> Self {
        Self::spawn_behind(world, &[env!("CARGO_BIN_EXE_dla")], args, extra)
    }

    /// `dla` as a child of a shell that leads the pty's session and stays for a
    /// minute after dla ends. Neither dla's death nor the shell's is then a
    /// session leader's exit within that minute, so the kernel sends the
    /// foreground group no SIGHUP, and only what dla itself arranged reaches its
    /// children.
    fn spawn_under_a_shell(world: &World, args: &[&str], extra: &[(&str, &str)]) -> Self {
        Self::spawn_behind(
            world,
            &[
                "sh",
                "-c",
                "\"$0\" \"$@\"; exec sleep 60",
                env!("CARGO_BIN_EXE_dla"),
            ],
            args,
            extra,
        )
    }

    fn spawn_behind(world: &World, lead: &[&str], args: &[&str], extra: &[(&str, &str)]) -> Self {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("a pty");
        let root = world.root.display().to_string();
        let mut command = CommandBuilder::new(lead[0]);
        command.args(&lead[1..]);
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
        // A terminal type, as every real terminal sets: skim reads terminfo by it
        // and draws no picker without one. `dla`'s answer to no `TERM` is its own
        // test below, which removes it again.
        command.env("TERM", "xterm-256color");
        if !world.root.join("gh-bin/gh").exists() {
            command.env("DEVLAUNCH_NO_GH_TOKEN", "1");
        }
        for (name, value) in extra {
            command.env(name, value);
        }
        let child = pty
            .slave
            .spawn_command(command)
            .expect("dla spawns on the pty");
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
        PtyDla {
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
    ///
    /// Enter is `\r`, which is what a terminal sends for it. The prompt editor
    /// holds the terminal in raw mode, where nothing turns it into `\n`, and a
    /// `\n` is Ctrl-J, which adds a line rather than submitting. So a `\n` inside
    /// `line` is a line of the prompt.
    ///
    /// One `write_all`, so the Enter comes in the same burst as the text. The
    /// editor reads an Enter with more input right behind it as a pasted line
    /// break; nothing follows this one, so it submits.
    fn send_line(&mut self, line: &str) {
        self.writer
            .write_all(format!("{line}\r").as_bytes())
            .and_then(|()| self.writer.flush())
            .expect("typing into the pty");
    }

    /// A terminal Ctrl-C. In cooked mode the line discipline turns it into SIGINT
    /// for the whole foreground process group. The prompt editor and the pickers
    /// hold the terminal in raw mode, where it is a byte they read and act on.
    fn interrupt(&mut self) {
        self.writer
            .write_all(b"\x03")
            .and_then(|()| self.writer.flush())
            .expect("interrupting the pty");
    }

    /// Press keys in a picker. Raw bytes, because skim holds the terminal in raw
    /// mode: Enter is `\r` there, not a line.
    fn press(&mut self, keys: &str) {
        self.writer
            .write_all(keys.as_bytes())
            .and_then(|()| self.writer.flush())
            .expect("typing into the pty");
    }

    /// Wait for the picker whose header holds `header` and answer it with `keys`.
    ///
    /// Counted by occurrences, so a second picker with the same words is waited
    /// for rather than answered from the first one's screen.
    fn answer(&mut self, header: &str, keys: &str) {
        let before = self.text().matches(header).count();
        assert!(
            wait_for(|| self.text().matches(header).count() > before),
            "the picker {header:?} never appeared; the pty said:\n{}",
            self.text()
        );
        // skim draws its header before it reads keys; a short pause keeps a key
        // from landing while it is still setting up the terminal.
        std::thread::sleep(Duration::from_millis(150));
        self.press(keys);
    }

    /// Answer a picker by typing `query`, then pressing Enter once skim has matched.
    ///
    /// Two writes with a pause between, because skim takes Enter against the last
    /// match it finished: a query and its Enter in one write reach it before the
    /// match does, and Enter takes the old first row.
    fn answer_typed(&mut self, header: &str, query: &str) {
        self.answer(header, query);
        std::thread::sleep(Duration::from_millis(400));
        self.press("\r");
    }

    /// Wait for the prompt editor. A line that names its workspace reaches it with
    /// no picker in the way.
    fn reach_the_editor(&self) {
        self.expect(BANNER);
    }

    /// Take the first row of dl's workspace picker, which is how a line with no
    /// workspace names one. In the scenario that row is [`MAIN`], and only a line
    /// that took this way in is asked for the agent, the model and the effort.
    fn pick_the_workspace(&mut self) {
        self.answer(WORKSPACE_PICKER, "\r");
    }

    /// Take the first row of the agent, model and effort pickers, then wait for
    /// the prompt editor. On a first run that is claude with every default. Only a
    /// line with no workspace meets these pickers.
    fn take_the_defaults(&mut self) {
        self.answer(AGENT_PICKER, "\r");
        self.answer(MODEL_PICKER, "\r");
        self.answer(EFFORT_PICKER, "\r");
        self.expect(BANNER);
    }

    fn wait(mut self) -> u32 {
        self.child.wait().expect("dla exits").exit_code()
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

/// Words in the header of claude's model picker.
const MODEL_PICKER: &str = "Model for claude";

/// Words in the header of claude's effort picker.
const EFFORT_PICKER: &str = "Effort for claude";

/// Words in the header of the agent picker, which also chooses claude's login.
const AGENT_PICKER: &str = "Agent for this launch";

/// The name [`MAIN`] is put on a terminal under.
///
/// The label and not the id, although the id is what is typed: the scenario records
/// the triple, and a workspace reached by its id is looked up rather than parsed
/// back out (blooop/devlaunch#632).
const TITLED: &str = "devlaunch@main";

/// The OSC 2 escape that names a terminal *name*, bytes and all.
fn osc_title(name: &str) -> String {
    format!("\x1b]2;{name}\x07")
}

#[test]
fn the_terminal_really_is_named_on_a_real_pty() {
    // The one test that proves the bytes arrive. Everything else about the title
    // is judged on a `Host` value or a notice; this watches the escape come out of
    // the pty, through the shipped binary, which is the only place the
    // stderr-is-a-tty guard is exercised for real.
    //
    // A *typed* prompt rather than an argv one, because the editor window is where
    // the name is decided and where a wrong one is looked at longest. `dla <id>`
    // with nothing on the line names the terminal itself, in front of a boot it
    // cannot wait for, and holds that name for the typing plus an entire
    // `devpod up`. So the run writes the escape twice -- once before the banner,
    // once at the handover -- and waiting only for the second one passes whether or
    // not the first said `devlaunch-main-3j1t`, which is exactly what
    // blooop/devlaunch#632 was reported as.
    let titled = osc_title(TITLED);
    let by_id = osc_title(MAIN);
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &[MAIN], &[]);
    session.reach_the_editor();
    // Exact bytes and from the very first one: nothing precedes the name on this
    // terminal, so a prefix check is the whole claim about what the editor window
    // is titled.
    assert!(
        session.text().starts_with(&titled),
        "the editor window was not named {titled:?}; the pty said:\n{:?}",
        session.text()
    );
    session.send_line("fix the bug");
    session.expect("dla -> dl");
    let seen = Arc::clone(&session.seen);
    assert_eq!(session.wait(), 0);

    let whole = String::from_utf8_lossy(&seen.lock().expect("the pty buffer")).into_owned();
    assert_eq!(
        whole.matches(&titled).count(),
        2,
        "the name is written twice, by the editor and by the launch; the pty said:\n{whole:?}"
    );
    assert!(
        !whole.contains(&by_id),
        "the id was written as a title; the pty said:\n{whole:?}"
    );
}

#[test]
fn the_title_switch_really_silences_it_on_a_real_pty() {
    // Off means no escape at all, not an empty title: `ESC ] 2 ; BEL` would blank
    // the terminal's name, which is not what "leave it alone" means.
    let world = World::with(&["--warm"]);
    let session = PtyDla::spawn(
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
fn a_falsey_no_tty_leaves_the_terminal_alone_on_a_real_pty() {
    // One variable, one reading. `dla`'s prompt and the ssh transport are both
    // gated on `DEVLAUNCH_NO_TTY`, and they used to disagree about what it says:
    // core lowercased and stripped the value before comparing it against the
    // falsey words, and `dl`'s own copy of that predicate compared the raw bytes
    // with a bare `matches!`. So `FALSE` — a spelling a person writes without
    // thinking, and the one `DEVLAUNCH_NO_TITLE=FALSE` would get right — kept the
    // pty for the transport and took it away from the prompt.
    //
    // Cased *and* padded, because those were two separate halves of the divergence
    // (`to_lowercase` and `osext::strip`) and either one alone still hides the
    // other. The banner appearing is the whole assertion: no banner means dla
    // decided there was no terminal to prompt at.
    // One world for both spellings rather than one each: these tests are timing
    // sensitive under a loaded `--workspace` run, and a scenario build is the
    // expensive half of a case that only needs a warm workspace to prompt about.
    let world = World::with(&["--warm"]);
    for value in ["FALSE", " no "] {
        let mut session = PtyDla::spawn(&world, &[MAIN], &[("DEVLAUNCH_NO_TTY", value)]);
        session.reach_the_editor();
        session.send_line("fix the bug");
        session.expect("dla -> dl");
        assert_eq!(session.wait(), 0, "DEVLAUNCH_NO_TTY={value:?}");
    }

    // And the truthy direction, in the same test because it is the same claim:
    // the reading is consulted at all. Without this, the wrapper's whole body
    // could be `false` — killing `DEVLAUNCH_NO_TTY=1` for every `dl` and `dla`
    // there is — and all three crates stay green, which is what deleting `dl`'s
    // own truthy test left behind.
    //
    // Asserted by what a skipped prompt *does* rather than by waiting out a
    // banner that never comes: with no terminal to prompt at, `dla` hands the
    // agent line straight to dl, so `dla -> dl` arriving with nothing typed is
    // the opt-out working. Waiting for the banner's absence would cost the
    // 60-second deadline on the passing path.
    let opted_out = PtyDla::spawn(&world, &[MAIN], &[("DEVLAUNCH_NO_TTY", "1")]);
    opted_out.expect("dla -> dl");
    assert!(
        !opted_out.text().contains(BANNER),
        "the editor prompted anyway; the pty said:\n{}",
        opted_out.text()
    );
    assert_eq!(opted_out.wait(), 0);
}

#[test]
fn a_typed_prompt_reaches_the_agent_with_no_shell_in_the_way() {
    // The double quotes are the point: they reach the agent literally, because
    // the prompt never passes through a shell on the host — the escaping pain the
    // editor exists to end.
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &[MAIN], &[]);
    session.reach_the_editor();
    session.send_line("fix the \"flaky\" test");
    session.expect("dla -> dl");
    assert_eq!(session.wait(), 0);
    assert_eq!(
        &without_session_id(world.devpod_calls().last().expect("a session")),
        &format!(
            "devpod ssh {MAIN} --log-output json --command bash -lc 'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions --remote-control={MAIN} \
             '\"'\"'fix the \"flaky\" test'\"'\"''"
        )
    );
}

#[test]
fn a_named_workspace_goes_straight_to_the_editor_with_no_picker() {
    // The pickers belong to a bare `dla`. A line that names its workspace said
    // what it wanted, so it gets the editor alone and the agent's defaults.
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &[MAIN], &[]);
    session.expect(BANNER);
    for picker in [AGENT_PICKER, MODEL_PICKER, EFFORT_PICKER] {
        assert!(
            !session.text().contains(picker),
            "{picker:?} was drawn for a named workspace; the pty said:\n{}",
            session.text()
        );
    }
    session.send_line("go");
    assert_eq!(session.wait(), 0);
    assert_eq!(
        &without_session_id(world.devpod_calls().last().expect("a session")),
        &claude_session("", "go")
    );
}

#[test]
fn a_choice_a_bare_dla_remembered_stays_out_of_dla_workspace() {
    // The cache remembers what a bare `dla` chose, but only to put it first in
    // the next picker. A line that names its workspace draws no picker, so it
    // gets the agent's defaults, not the last choice.
    let world = World::with(&["--warm"]);
    let mut first = PtyDla::spawn(&world, &[], &[]);
    first.pick_the_workspace();
    first.answer(AGENT_PICKER, "\r");
    first.answer_typed(MODEL_PICKER, "sonnet");
    first.answer_typed(EFFORT_PICKER, "low");
    first.expect(BANNER);
    first.send_line("one");
    assert_eq!(first.wait(), 0);

    let mut second = PtyDla::spawn(&world, &[MAIN], &[]);
    second.reach_the_editor();
    second.send_line("two");
    assert_eq!(second.wait(), 0);
    assert_eq!(
        &without_session_id(world.devpod_calls().last().expect("a session")),
        &claude_session("", "two")
    );
}

#[test]
fn a_lone_esc_at_the_editor_does_not_swallow_the_next_key() {
    // The pickers teach that Esc cancels, so a person presses it here too. On its
    // own, with a pause after it, it is not the Alt of the next key.
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &[MAIN], &[]);
    session.reach_the_editor();
    session.press("\x1b");
    std::thread::sleep(Duration::from_millis(100));
    session.press("go");
    std::thread::sleep(Duration::from_millis(100));
    session.press("\r");
    session.expect("dla -> dl");
    assert_eq!(session.wait(), 0);
    let last = world.devpod_calls().last().expect("a session").clone();
    assert!(last.ends_with(" go'"), "{last}");
}

#[test]
fn a_pasted_multi_line_prompt_arrives_whole_rather_than_leaking() {
    // A paste delivers its newlines with it, and the terminal holds the later
    // lines as completed input. The editor must drain them into the prompt: a
    // submission cut at the first newline would hand the agent line one and leave
    // the rest queued in the terminal, to land inside the agent's session as
    // keystrokes.
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &[MAIN], &[]);
    session.reach_the_editor();
    // One write, as a terminal without bracketed paste delivers one. Each line
    // ends in `\r`, the same byte as Enter, and the second line is already queued
    // when the first `\r` is read: that queued input is all that makes the first
    // `\r` a line break. The last one has nothing behind it and submits.
    session.press("fix this\rand then that\r");
    assert_eq!(session.wait(), 0);
    assert_eq!(
        &without_session_id(world.devpod_calls().last().expect("a session")),
        &format!(
            "devpod ssh {MAIN} --log-output json --command bash -lc 'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions --remote-control={MAIN} \
             '\"'\"'fix this\nand then that'\"'\"''"
        )
    );
}

#[test]
fn an_empty_enter_is_the_plain_session_it_always_was() {
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &[MAIN], &[]);
    session.reach_the_editor();
    session.send_line("");
    assert_eq!(session.wait(), 0);
    assert_eq!(
        &without_session_id(world.devpod_calls().last().expect("a session")),
        &format!(
            "devpod ssh {MAIN} --log-output json --command bash -lc 'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions --remote-control={MAIN}'"
        )
    );
}

/// The session line `dla resume` hands dl for [`MAIN`]: the agent's own line with
/// `--resume` as its last word.
fn resumed_session() -> String {
    format!(
        "devpod ssh {MAIN} --log-output json --command bash -lc 'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
         --dangerously-skip-permissions --remote-control={MAIN} --resume'"
    )
}

#[test]
fn a_resume_line_on_a_terminal_opens_no_editor() {
    // A resume line has no prompt, and an empty prompt on a terminal is exactly
    // what opens the editor for an agent line. Only the real gate on a real pty
    // says which of the two a resume line is.
    let world = World::with(&["--warm"]);
    let session = PtyDla::spawn(&world, &["resume", MAIN], &[]);
    session.expect("dla -> dl");
    let seen = Arc::clone(&session.seen);
    assert_eq!(session.wait(), 0);

    let whole = String::from_utf8_lossy(&seen.lock().expect("the pty buffer")).into_owned();
    assert!(
        !whole.contains("Type the prompt") && !whole.contains(BANNER),
        "a resume line opened the editor; the pty said:\n{whole}"
    );
    assert_eq!(
        world.devpod_calls().last().expect("a session"),
        &resumed_session()
    );
}

#[test]
fn a_resume_with_no_workspace_resumes_the_row_the_picker_took() {
    // `dla resume` alone asks dl's picker, and the id that comes back is the one
    // the line is then built around. The row is taken through skim, whose answer is
    // a label that dl maps back to an id, so only a pick made on a terminal proves
    // the id that reaches the session is the row's.
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &["resume"], &[("TERM", "xterm-256color")]);
    session.expect("Select workspace (type to filter):");
    session.expect("blooop | devlaunch | main");
    session
        .writer
        .write_all(b"\r")
        .and_then(|()| session.writer.flush())
        .expect("taking the row on the pty");
    session.expect("dla -> dl");
    let seen = Arc::clone(&session.seen);
    assert_eq!(session.wait(), 0);

    let whole = String::from_utf8_lossy(&seen.lock().expect("the pty buffer")).into_owned();
    assert!(
        whole.contains(&format!("Picked blooop | devlaunch | main -> {MAIN}")),
        "the pick named no row; the pty said:\n{whole:?}"
    );
    assert_eq!(
        world.devpod_calls().last().expect("a session"),
        &resumed_session()
    );
}

#[test]
fn a_resume_with_no_workspace_on_a_terminal_no_entry_can_draw_on_says_how_to_name_one() {
    // `TERMINFO_DIRS` is the whole of the terminfo search when it is set, so an
    // empty one leaves no entry, the fallback's included, that can move the
    // cursor. No picker can be drawn, and dla says how to name the workspace
    // instead of opening anything.
    let world = World::with(&["--warm"]);
    let nothing = world.root.join("terminfo");
    std::fs::create_dir_all(&nothing).expect("an empty terminfo directory");
    let mut session = PtyDla::spawn(
        &world,
        &["resume"],
        &[
            ("TERM", "xterm-no-such-entry"),
            ("TERMINFO_DIRS", &nothing.display().to_string()),
        ],
    );
    let seen = Arc::clone(&session.seen);
    // Bounded: a picker that opened anyway would wait on this pty for ever.
    let mut code = None;
    wait_for(|| {
        code = session.child.try_wait().expect("dla's status");
        code.is_some()
    });
    let _ = session.child.kill();
    assert_eq!(code.map(|status| status.exit_code()), Some(1));

    // The reader thread may still hold the last bytes, so the line is waited for.
    let said = || String::from_utf8_lossy(&seen.lock().expect("the pty buffer")).into_owned();
    assert!(
        wait_for(|| said().contains(
            "so dla resume cannot draw its picker. Name the workspace instead: \
             dla resume <workspace>"
        )),
        "the refusal was not given; the pty said:\n{:?}",
        said()
    );
    assert!(
        !said().contains("\x1b[?1049h"),
        "a picker was opened anyway: {:?}",
        said()
    );
    assert!(
        !world
            .devpod_calls()
            .iter()
            .any(|call| call.starts_with("devpod ssh")),
        "a refused picker opened a session: {:?}",
        world.devpod_calls()
    );
}

#[test]
fn the_boot_runs_while_the_prompt_is_still_being_typed() {
    // The overlap itself: a stopped workspace's `devpod up` is on the shim's log
    // while the editor is still open — nothing has been typed yet — and the
    // attach that follows the Enter finds it running.
    let world = World::with(&["--stopped"]);
    let mut session = PtyDla::spawn(&world, &[MAIN], &[]);
    session.reach_the_editor();
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
        &without_session_id(world.devpod_calls().last().expect("a session")),
        &format!(
            "devpod ssh {MAIN} --log-output json --command bash -lc 'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions --remote-control={MAIN} go'"
        )
    );
}

#[test]
fn a_ctrl_c_at_the_editor_tears_the_whole_boot_down() {
    // `tests/interrupt.rs` on the pty. The editor holds the terminal in raw mode,
    // so the Ctrl-C is a byte and no SIGINT is sent: dla itself interrupts the
    // boot child, whose own handler kills the blocked `devpod up` (in a group of
    // its own) and unlinks the staged token.
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

    let mut session = PtyDla::spawn(
        &world,
        &["blooop/devlaunch@cold"],
        &[
            ("TMPDIR", &tmpdir.display().to_string()),
            ("DL_UP_PID", &up_pid.display().to_string()),
            ("DL_UP_STARTED", &up_started.display().to_string()),
        ],
    );
    session.reach_the_editor();
    // Interrupt only once the boot is mid-`up` with the token staged — the exact
    // state the interrupt handler exists to clean.
    assert!(
        wait_for(|| up_started.exists() && token_file(&tmpdir).is_some()),
        "devpod up never blocked with a token staged"
    );
    let up = std::fs::read_to_string(&up_pid).expect("the up pid");
    let up = up.trim().to_owned();

    session.interrupt();
    assert_eq!(session.wait(), 130, "dla exits 130 on interrupt");
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

/// A dla that is SIGKILLed at the editor takes its boot with it, and the boot
/// still unlinks its staged token.
///
/// A SIGKILL runs no handler, so dla never reaches the `cancel` a Ctrl-C does.
/// The kernel's parent-death signal is what reaches the boot instead, and it is a
/// SIGINT so the boot's own handler runs: it kills the `devpod up` and unlinks the
/// token file, as the Ctrl-C above has it do.
#[cfg(target_os = "linux")]
#[test]
fn a_sigkilled_dla_takes_its_boot_down_and_the_token_with_it() {
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
         \x20 exec sleep 120\n\
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

    let session = PtyDla::spawn_under_a_shell(
        &world,
        &["blooop/devlaunch@cold"],
        &[
            ("TMPDIR", &tmpdir.display().to_string()),
            ("DL_UP_PID", &up_pid.display().to_string()),
            ("DL_UP_STARTED", &up_started.display().to_string()),
        ],
    );
    session.reach_the_editor();
    assert!(
        wait_for(|| up_started.exists() && token_file(&tmpdir).is_some()),
        "devpod up never blocked with a token staged"
    );
    let up = std::fs::read_to_string(&up_pid).expect("the up pid");
    let up = up.trim().to_owned();
    let only_child = |parent: &str| {
        let children = Command::new("ps")
            .args(["-o", "pid=", "--ppid", parent])
            .output()
            .expect("ps is installed");
        String::from_utf8_lossy(&children.stdout)
            .split_whitespace()
            .next()
            .expect("a child")
            .to_owned()
    };
    let shell = session
        .child
        .process_id()
        .expect("the shell's pid")
        .to_string();
    let dla = only_child(&shell);
    let boot = only_child(&dla);
    let alive = |pid: &str| {
        Command::new("kill")
            .args(["-0", pid])
            .output()
            .expect("kill is installed")
            .status
            .success()
    };

    assert!(
        Command::new("kill")
            .args(["-KILL", &dla])
            .status()
            .expect("kill is installed")
            .success(),
        "sending SIGKILL to dla"
    );
    let dla_gone = wait_for(|| !alive(&dla));

    let boot_gone = wait_for(|| !alive(&boot));
    let token_gone = wait_for(|| token_file(&tmpdir).is_none());
    let up_gone = wait_for(|| !alive(&up));
    for pid in [&boot, &up, &shell] {
        let _ = Command::new("kill").args(["-KILL", pid]).output();
    }
    let _ = session.wait();
    assert!(dla_gone, "dla (pid {dla}) outlived its SIGKILL");
    assert!(
        boot_gone,
        "the boot (pid {boot}) outlived the SIGKILLed dla"
    );
    assert!(token_gone, "the boot left its token file behind");
    assert!(up_gone, "the boot left its devpod up (pid {up}) behind");
}

/// The one staged GitHub-token file under `dir`, if any.
fn token_file(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        let path = entry.path();
        let name = path.file_name()?.to_string_lossy().into_owned();
        (name.starts_with("devlaunch-gh-") && name.ends_with(".env")).then_some(path)
    })
}

// ===========================================================================
// the pickers ahead of the editor
// ===========================================================================

/// The claude payload a launch sends, with the words the pickers added.
fn claude_session(settings: &str, prompt: &str) -> String {
    format!(
        "devpod ssh {MAIN} --log-output json --command bash -lc 'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 \
         IS_SANDBOX=1 claude --dangerously-skip-permissions {settings}--remote-control={MAIN} {prompt}'"
    )
}

#[test]
fn a_model_that_is_not_listed_is_typed_and_reaches_the_agent() {
    // Nothing lists `claude-opus-5-5`, so the query matches no row and Enter takes
    // the query itself. The effort is picked from the list by filtering to it.
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &[], &[]);
    session.pick_the_workspace();
    session.answer(AGENT_PICKER, "\r");
    session.answer_typed(MODEL_PICKER, "claude-opus-5-5");
    session.answer_typed(EFFORT_PICKER, "max");
    session.expect("(model claude-opus-5-5, effort max)");
    session.expect(BANNER);
    session.send_line("go");
    assert_eq!(session.wait(), 0);
    assert_eq!(
        &without_session_id(world.devpod_calls().last().expect("a session")),
        &claude_session("--model claude-opus-5-5 --effort max ", "go")
    );
}

#[test]
fn the_last_choice_is_the_first_row_of_the_next_launch() {
    // One world, two launches: the second takes the first row of each picker and
    // gets what the first launch chose, because the cache remembered it.
    let world = World::with(&["--warm"]);
    let mut first = PtyDla::spawn(&world, &[], &[]);
    first.pick_the_workspace();
    first.answer(AGENT_PICKER, "\r");
    first.answer_typed(MODEL_PICKER, "sonnet");
    first.answer_typed(EFFORT_PICKER, "low");
    first.expect(BANNER);
    first.send_line("one");
    assert_eq!(first.wait(), 0);

    let mut second = PtyDla::spawn(&world, &[], &[]);
    second.pick_the_workspace();
    second.take_the_defaults();
    second.send_line("two");
    assert_eq!(second.wait(), 0);
    assert_eq!(
        &without_session_id(world.devpod_calls().last().expect("a session")),
        &claude_session("--model sonnet --effort low ", "two")
    );
}

#[test]
fn a_setting_a_flag_gave_is_not_asked_for() {
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &["--model", "opus"], &[]);
    session.pick_the_workspace();
    session.answer(AGENT_PICKER, "\r");
    session.answer(EFFORT_PICKER, "\r");
    session.expect(BANNER);
    assert!(
        !session.text().contains(MODEL_PICKER),
        "the model picker was drawn although --model gave the model"
    );
    session.send_line("go");
    assert_eq!(session.wait(), 0);
    assert_eq!(
        &without_session_id(world.devpod_calls().last().expect("a session")),
        &claude_session("--model opus ", "go")
    );
}

#[test]
fn with_no_terminal_type_the_pickers_are_drawn_as_the_fallback_terminal() {
    // skim cannot draw without a `TERM`, and used to panic on one that was not
    // set. The pickers take the workspace picker's fallback and say so, and each
    // one is still answered with Enter.
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &[], &[("TERM", "")]);
    session.pick_the_workspace();
    session.answer(AGENT_PICKER, "\r");
    session.answer(MODEL_PICKER, "\r");
    session.answer(EFFORT_PICKER, "\r");
    session.expect(BANNER);
    assert!(
        session.text().contains("drawn as xterm-256color"),
        "{}",
        session.text()
    );
    session.send_line("go");
    assert_eq!(session.wait(), 0);
    assert_eq!(
        &without_session_id(world.devpod_calls().last().expect("a session")),
        &claude_session("", "go")
    );
}

#[test]
fn a_named_claude_login_is_a_row_of_the_agent_picker_and_reaches_dl() {
    // A host with one named profile holding a credential gets a claude row for it
    // in the agent picker. The row is found by filtering to its name, as a person
    // would.
    let world = World::with(&["--warm"]);
    let profiles = world.root.join("profiles");
    std::fs::create_dir_all(profiles.join("work")).expect("a profile directory");
    std::fs::write(profiles.join("work/.credentials.json"), "{}").expect("a credential");
    let mut session = PtyDla::spawn(
        &world,
        &[],
        &[(
            "DEVLAUNCH_CLAUDE_PROFILES_DIR",
            &profiles.display().to_string(),
        )],
    );
    session.pick_the_workspace();
    session.answer_typed(AGENT_PICKER, "work");
    session.answer(MODEL_PICKER, "\r");
    session.answer(EFFORT_PICKER, "\r");
    session.expect(BANNER);
    session.expect("(account work)");
    session.send_line("go");
    session.expect(&format!("dla -> dl --claude-profile work {MAIN} --"));
    assert_eq!(session.wait(), 0);
}

#[test]
fn another_agent_is_a_row_of_the_same_picker() {
    // codex is chosen by name, and its own pickers follow. Remote Control was only
    // claude's default, so codex starts without it and nothing refuses.
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &[], &[]);
    session.pick_the_workspace();
    session.answer_typed(AGENT_PICKER, "codex");
    session.answer(MODEL_PICKER.replace("claude", "codex").as_str(), "\r");
    session.answer_typed(EFFORT_PICKER.replace("claude", "codex").as_str(), "high");
    session.expect(BANNER);
    session.send_line("hi");
    assert_eq!(session.wait(), 0);
    let last = world.devpod_calls().last().expect("a session").clone();
    assert!(
        last.contains(
            "codex --dangerously-bypass-approvals-and-sandbox -c model_reasoning_effort=high hi"
        ),
        "{last}"
    );
}

#[test]
fn alt_enter_takes_a_model_that_is_a_prefix_of_a_listed_one() {
    // Matching is by substring, so `gpt-5.5` still matches the remembered
    // `gpt-5.5-codex` and Enter takes that row. Alt-Enter (ESC CR on a terminal)
    // takes the query as typed.
    let world = World::with(&["--warm"]);
    let codex_model = MODEL_PICKER.replace("claude", "codex");
    let codex_effort = EFFORT_PICKER.replace("claude", "codex");
    let mut first = PtyDla::spawn(&world, &["--codex"], &[]);
    first.pick_the_workspace();
    first.answer_typed(&codex_model, "gpt-5.5-codex");
    first.answer(&codex_effort, "\r");
    first.expect(BANNER);
    first.send_line("one");
    assert_eq!(first.wait(), 0);

    let mut second = PtyDla::spawn(&world, &["--codex"], &[]);
    second.pick_the_workspace();
    second.answer(&codex_model, "gpt-5.5");
    std::thread::sleep(Duration::from_millis(400));
    second.press("\x1b\r");
    second.answer(&codex_effort, "\r");
    second.expect("(model gpt-5.5)");
    second.expect(BANNER);
    second.send_line("two");
    assert_eq!(second.wait(), 0);
    let last = world.devpod_calls().last().expect("a session").clone();
    assert!(last.contains("--model gpt-5.5 "), "{last}");
}

#[test]
fn a_typed_agent_with_one_login_is_not_asked_which_agent() {
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &["--claude"], &[]);
    session.pick_the_workspace();
    session.answer(MODEL_PICKER, "\r");
    session.answer(EFFORT_PICKER, "\r");
    session.expect(BANNER);
    assert!(!session.text().contains(AGENT_PICKER), "{}", session.text());
    session.send_line("");
    assert_eq!(session.wait(), 0);
}

#[test]
fn esc_in_a_picker_stops_the_boot_and_launches_nothing() {
    // The same blocking `up` as the Ctrl-C test above, with the token staged. In a
    // picker Ctrl-C and Esc are keys, not signals, so nothing reaches the boot
    // unless dla sends it. The cleanup is the proof that it did. Only a line with
    // no workspace gets the pickers, so the workspace is the stopped one the
    // workspace picker lists, which the boot has to `up`.
    let world = World::with(&["--gh", "--stopped"]);
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

    let mut session = PtyDla::spawn(
        &world,
        &[],
        &[
            ("TMPDIR", &tmpdir.display().to_string()),
            ("DL_UP_PID", &up_pid.display().to_string()),
            ("DL_UP_STARTED", &up_started.display().to_string()),
        ],
    );
    session.pick_the_workspace();
    session.answer(AGENT_PICKER, "\r");
    session.answer(MODEL_PICKER, "");
    assert!(
        wait_for(|| up_started.exists() && token_file(&tmpdir).is_some()),
        "devpod up never blocked with a token staged"
    );
    let up = std::fs::read_to_string(&up_pid).expect("the up pid");
    let up = up.trim().to_owned();

    session.press("\x1b");
    session.expect("cancelled");
    let seen = Arc::clone(&session.seen);
    assert_eq!(session.wait(), 130);
    assert!(
        !String::from_utf8_lossy(&seen.lock().expect("the pty buffer")).contains(BANNER),
        "the editor opened after a cancel"
    );
    assert!(
        token_file(&tmpdir).is_none(),
        "the token file must be gone once dla has waited the boot out"
    );
    assert!(
        wait_for(|| !Command::new("kill")
            .args(["-0", &up])
            .output()
            .expect("kill is installed")
            .status
            .success()),
        "the devpod up (pid {up}) must have been killed"
    );
    assert!(
        !world
            .devpod_calls()
            .iter()
            .any(|call| call.starts_with("devpod ssh")),
        "a session was opened after a cancel: {:?}",
        world.devpod_calls()
    );
}

/// The agent sessions devpod was asked for so far. The boot's own `devpod ssh`
/// calls provision the workspace and are left out: only a session asks for
/// `--log-output json`.
fn sessions(world: &World) -> Vec<String> {
    world
        .devpod_calls()
        .into_iter()
        .filter(|call| call.starts_with(&format!("devpod ssh {MAIN} --log-output json ")))
        .collect()
}

#[test]
fn the_agent_chosen_last_is_the_first_row_of_the_next_launch() {
    // The first run lists claude first, so an Enter on the second launch's agent
    // picker takes codex only if the first launch's choice moved it up.
    let world = World::with(&["--warm"]);
    let mut first = PtyDla::spawn(&world, &[], &[]);
    first.pick_the_workspace();
    first.answer_typed(AGENT_PICKER, "codex");
    first.answer("Model for codex", "\r");
    first.answer("Effort for codex", "\r");
    first.expect(BANNER);
    first.send_line("one");
    assert_eq!(first.wait(), 0);

    let mut second = PtyDla::spawn(&world, &[], &[]);
    second.pick_the_workspace();
    second.answer(AGENT_PICKER, "\r");
    // Waited for by the words every agent's model picker shares, so a claude row
    // fails here at once rather than at the deadline.
    second.answer("Model for ", "\r");
    assert!(
        second.text().contains("Model for codex"),
        "the first row was not codex; the pty said:\n{}",
        second.text()
    );
    second.answer("Effort for codex", "\r");
    second.expect(BANNER);
    second.send_line("two");
    assert_eq!(second.wait(), 0);
    let last = world.devpod_calls().last().expect("a session").clone();
    assert!(
        last.contains("codex --dangerously-bypass-approvals-and-sandbox two"),
        "{last}"
    );
}

#[test]
fn a_typed_agent_that_is_no_row_is_asked_for_again() {
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &[], &[]);
    session.pick_the_workspace();
    session.answer_typed(AGENT_PICKER, "nonesuch");
    // `answer` counts the header, so it waits for the picker drawn a second time.
    session.answer(AGENT_PICKER, "\x1b");
    session.expect("cancelled");
    let seen = Arc::clone(&session.seen);
    assert_eq!(session.wait(), 130);
    let whole = String::from_utf8_lossy(&seen.lock().expect("the pty buffer")).into_owned();
    assert!(
        !whole.contains(MODEL_PICKER),
        "a typed name that is no agent went on to the model picker; the pty said:\n{whole}"
    );
    assert!(
        sessions(&world).is_empty(),
        "a session was opened: {:?}",
        sessions(&world)
    );
}

#[test]
fn esc_at_the_agent_picker_launches_nothing() {
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &[], &[]);
    session.pick_the_workspace();
    session.answer(AGENT_PICKER, "\x1b");
    session.expect("cancelled");
    let seen = Arc::clone(&session.seen);
    assert_eq!(session.wait(), 130);
    let whole = String::from_utf8_lossy(&seen.lock().expect("the pty buffer")).into_owned();
    assert!(
        !whole.contains(MODEL_PICKER) && !whole.contains(BANNER),
        "the launch went on after the Esc; the pty said:\n{whole}"
    );
    assert!(
        sessions(&world).is_empty(),
        "a session was opened after a cancel: {:?}",
        sessions(&world)
    );
}

#[test]
fn a_setting_a_flag_gave_is_the_first_row_of_the_next_launch() {
    // Remembered as a choice although no picker asked for it.
    let world = World::with(&["--warm"]);
    let mut first = PtyDla::spawn(&world, &["--model", "opus"], &[]);
    first.pick_the_workspace();
    first.answer(AGENT_PICKER, "\r");
    first.answer(EFFORT_PICKER, "\r");
    first.expect(BANNER);
    first.send_line("one");
    assert_eq!(first.wait(), 0);

    let mut second = PtyDla::spawn(&world, &[], &[]);
    second.pick_the_workspace();
    second.take_the_defaults();
    second.send_line("two");
    assert_eq!(second.wait(), 0);
    assert_eq!(
        &without_session_id(world.devpod_calls().last().expect("a session")),
        &claude_session("--model opus ", "two")
    );
}

// ===========================================================================
// the prompt editor
// ===========================================================================

#[test]
fn a_pasted_prompt_keeps_its_line_breaks_and_waits_for_enter() {
    // The paste ends in a line break, as a copied block of text often does. Under
    // the cooked read that break submitted the prompt; here it is text, and the
    // prompt is sent only on the Enter after it. Windows line ends are one break.
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &[MAIN], &[]);
    session.reach_the_editor();
    // Two writes, so one read ends on a `\r` with nothing queued behind it. The
    // open paste is then the only thing that keeps that `\r` a line break.
    session.press("\x1b[200~first line\r");
    std::thread::sleep(Duration::from_millis(200));
    session.press("second line\r\n\x1b[201~");
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !session.text().contains("dla -> dl"),
        "the paste's own line break submitted the prompt; devpod was asked for {:?}",
        world.devpod_calls()
    );
    session.press("\r");
    assert_eq!(session.wait(), 0);
    let last = world.devpod_calls().last().expect("a session").clone();
    assert!(last.contains("first line\nsecond line'"), "{last}");
}

#[test]
fn a_paste_longer_than_the_kernels_line_limit_arrives_whole() {
    // The cooked read went through the line discipline, which holds 4096 bytes of
    // one line and drops the rest. Twice that, in pieces, to be sure.
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &[MAIN], &[]);
    session.reach_the_editor();
    let long = "y".repeat(9000);
    session.press("\x1b[200~");
    for chunk in long.as_bytes().chunks(3000) {
        session.press(std::str::from_utf8(chunk).expect("ascii"));
        std::thread::sleep(Duration::from_millis(50));
    }
    session.press("\x1b[201~");
    std::thread::sleep(Duration::from_millis(200));
    session.press("\r");
    assert_eq!(session.wait(), 0);
    let last = world.devpod_calls().last().expect("a session").clone();
    assert!(
        last.contains(&format!(" {long}'")),
        "{} bytes long",
        last.len()
    );
}

#[test]
fn a_paste_that_arrives_late_does_not_leak_into_the_agent() {
    // The second half of the paste comes after a pause. The cooked read took what
    // was queued at the first line break and left the rest for the agent session
    // as keystrokes; the editor waits for the end of the paste.
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &[MAIN], &[]);
    session.reach_the_editor();
    session.press("\x1b[200~part one\r");
    std::thread::sleep(Duration::from_millis(300));
    session.press("part two\x1b[201~");
    std::thread::sleep(Duration::from_millis(200));
    session.press("\r");
    assert_eq!(session.wait(), 0);
    let last = world.devpod_calls().last().expect("a session").clone();
    assert!(last.contains("part one\npart two'"), "{last}");
}

#[test]
fn a_line_is_added_by_typing_alt_enter_or_ctrl_j() {
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &[MAIN], &[]);
    session.reach_the_editor();
    session.press("one\x1b\r");
    std::thread::sleep(Duration::from_millis(100));
    session.press("two\n");
    std::thread::sleep(Duration::from_millis(100));
    session.press("three");
    std::thread::sleep(Duration::from_millis(100));
    session.press("\r");
    assert_eq!(session.wait(), 0);
    let last = world.devpod_calls().last().expect("a session").clone();
    assert!(last.contains("one\ntwo\nthree'"), "{last}");
}

// ===========================================================================
// no workspace named
// ===========================================================================

/// Words in the header of dl's workspace picker.
const WORKSPACE_PICKER: &str = "Select workspace";

#[test]
fn a_bare_dla_picks_a_workspace_as_a_bare_dl_does() {
    // The scenario's one workspace is the only row, so Enter takes it, and the
    // launch that follows is the ordinary one for that workspace.
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &[], &[]);
    session.answer(WORKSPACE_PICKER, "\r");
    session.expect(&format!("-> {MAIN}"));
    session.take_the_defaults();
    session.send_line("go");
    assert_eq!(session.wait(), 0);
    assert_eq!(
        &without_session_id(world.devpod_calls().last().expect("a session")),
        &claude_session("", "go")
    );
}

#[test]
fn leading_flags_with_no_workspace_pick_one_and_keep_the_flags() {
    // `--codex` still chooses the agent, so there is no agent picker after the
    // workspace one: the typed flag already said.
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &["--codex"], &[]);
    session.answer(WORKSPACE_PICKER, "\r");
    session.answer("Model for codex", "\r");
    session.answer("Effort for codex", "\r");
    session.expect(BANNER);
    assert!(!session.text().contains(AGENT_PICKER), "{}", session.text());
    session.send_line("hi");
    assert_eq!(session.wait(), 0);
    let last = world.devpod_calls().last().expect("a session").clone();
    assert!(
        last.contains("codex --dangerously-bypass-approvals-and-sandbox hi"),
        "{last}"
    );
}

#[test]
fn a_quit_workspace_picker_launches_nothing() {
    let world = World::with(&["--warm"]);
    let mut session = PtyDla::spawn(&world, &[], &[]);
    session.answer(WORKSPACE_PICKER, "\x1b");
    assert_eq!(session.wait(), 1);
    assert!(
        world.devpod_calls().is_empty(),
        "{:?}",
        world.devpod_calls()
    );
}

#[test]
fn a_retired_spelling_with_no_workspace_is_refused_before_any_picker() {
    // dl is what refuses a retired spelling, and it needs a workspace to be asked
    // about. A picked spec would land after the retired word, which then no longer
    // ends the line, and the line would become an agent launch.
    for line in [&["--stop"][..], &["resume", "--autorm"]] {
        let world = World::with(&["--warm"]);
        let mut session = PtyDla::spawn(&world, line, &[]);
        let seen = Arc::clone(&session.seen);
        // Bounded: a picker that opened anyway would wait on this pty for ever.
        let mut code = None;
        wait_for(|| {
            code = session.child.try_wait().expect("dla's status");
            code.is_some()
        });
        let _ = session.child.kill();
        let said = || String::from_utf8_lossy(&seen.lock().expect("the pty buffer")).into_owned();
        assert_eq!(
            code.map(|status| status.exit_code()),
            Some(1),
            "{line:?}: the pty said:\n{:?}",
            said()
        );
        assert!(
            wait_for(|| said().contains("dla needs a workspace")),
            "{line:?}: the refusal was not given; the pty said:\n{:?}",
            said()
        );
        assert!(
            !said().contains(WORKSPACE_PICKER),
            "{line:?}: the workspace picker opened: {:?}",
            said()
        );
        assert!(
            !world
                .devpod_calls()
                .iter()
                .any(|call| call.starts_with("devpod ssh")),
            "{line:?}: a session was opened: {:?}",
            world.devpod_calls()
        );
    }
}

/// A session call with dla's `--session-id <uuid>` taken out, after checking it is
/// there: the id is random per launch, and every other byte of the line is the
/// assertion.
fn without_session_id(call: &str) -> String {
    let flag = " --session-id ";
    let at = call
        .find(flag)
        .unwrap_or_else(|| panic!("dla named no session: {call}"));
    let id_starts = at + flag.len();
    let id = &call[id_starts..id_starts + 36];
    assert!(
        id.len() == 36
            && id.chars().filter(|c| *c == '-').count() == 4
            && id.as_bytes()[14] == b'4',
        "not a v4 uuid: {id:?}"
    );
    format!("{}{}", &call[..at], &call[id_starts + 36..])
}
