//! `aid` at the binary boundary: what it prints, what it hands dl, and how it ends.
//!
//! Every expectation here was captured by running the frozen Python build —
//! `python -m devlaunch.aid` — against `dl/tests/launch_scenario.py`'s world with
//! `test/fixtures/devpod_shim.py` on PATH as `devpod`, under a scratch
//! `HOME`/`XDG_CACHE_HOME`/`XDG_CONFIG_HOME`/`DEVPOD_HOME`, and pasting what it
//! printed. Nothing here was read off the Rust implementation.
//!
//! The world is `dl`'s, deliberately: aid's whole contract is that it reaches a
//! workspace through dl and through nothing else, so the fixture that judges it has
//! to be the one that judges dl.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use devlaunch_test_support::KeepingCoverage;

/// The workspace id this build derives for `blooop/devlaunch@main`, which the
/// scenario records and devpod knows.
const MAIN: &str = "devlaunch-main-3j1t";

/// The version both binaries print. Read from the manifest, because that is where
/// the release reads it from too.
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the repository root")
}

/// One scratch world, and the `aid` runs against it.
struct World {
    root: PathBuf,
    _scratch: tempfile::TempDir,
}

impl World {
    fn with(fixtures: &[&str]) -> Self {
        // A fixed-length scratch path, as the sibling suites make: the
        // golden-capture harness makes `/tmp/dltXXXXXX` too.
        let scratch = tempfile::Builder::new()
            .prefix("dlt")
            .rand_bytes(6)
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

    fn aid(&self, args: &[&str]) -> Run {
        self.aid_with(args, &[])
    }

    fn aid_with(&self, args: &[&str], extra: &[(&str, &str)]) -> Run {
        let root = self.root.display().to_string();
        let mut command = Command::new(env!("CARGO_BIN_EXE_aid"));
        command
            .args(args)
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
            .env("GIT_SSH_COMMAND", "false")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null");
        // Hermetic about gh, exactly as dl/tests/launch.rs: with no fake gh in
        // gh-bin the world is "no gh", but PATH keeps /usr/bin for git and a CI
        // runner's /usr/bin/gh would otherwise leak in and warn. The opt-out
        // reproduces the no-gh world regardless of host.
        if !self.root.join("gh-bin/gh").exists() {
            command.env("DEVLAUNCH_NO_GH_TOKEN", "1");
        }
        for (name, value) in extra {
            command.env(name, value);
        }
        Run::of(&command.output().expect("the aid binary runs"), &self.root)
    }

    /// The devpod calls made so far, in order, with `devpod list` left out — the
    /// detached completion refresh a launch spawns makes one of its own, and whether
    /// it lands before the parent exits is a matter of scheduling.
    fn devpod_calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("shim-log.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let call: serde_json::Value = serde_json::from_str(line).expect("a log line");
                call["argv"]
                    .as_array()
                    .expect("an argv")
                    .iter()
                    .map(|word| word.as_str().expect("a word").to_owned())
                    .collect::<Vec<String>>()
            })
            .filter(|argv| argv.first().map(String::as_str) != Some("list"))
            .map(|argv| format!("devpod {}", argv.join(" ")))
            .collect()
    }
}

struct Run {
    out: String,
    err: String,
    code: Option<i32>,
}

impl Run {
    fn of(output: &Output, root: &Path) -> Self {
        let template = |bytes: &[u8]| {
            String::from_utf8_lossy(bytes).replace(&root.display().to_string(), "{ROOT}")
        };
        Run {
            out: template(&output.stdout),
            err: template(&output.stderr),
            code: output.status.code(),
        }
    }

    fn exited(&self, code: i32) -> &Self {
        assert_eq!(
            self.code,
            Some(code),
            "expected exit {code}; stdout: {}; stderr: {}",
            self.out,
            self.err
        );
        self
    }
}

// ===========================================================================
// the three answers aid gives on its own
// ===========================================================================

#[test]
fn help_is_asked_for_by_flag_and_printed_by_accident() {
    // Python's pair of endings for one body (`aid.py`:220): the help is what a
    // person who typed `aid` alone needs, and typing `aid` alone is still a command
    // line that named no workspace.
    let world = World::with(&["--warm"]);

    for (args, code) in [(vec!["--help"], 0), (vec!["-h"], 0), (vec![], 1)] {
        let run = world.aid_with(&args, &[]);
        run.exited(code);
        assert!(
            run.out
                .starts_with("aid - AI Develop: start a coding agent in a devlaunch workspace\n"),
            "aid {args:?} printed {:?}",
            run.out
        );
        // The usage text ends with a blank line, as Python's `print(f\"\"\"…\n\"\"\")`
        // does, and it is the whole of what was said.
        assert!(run.out.ends_with("    dl --help\n\n"), "{:?}", run.out);
        assert_eq!(run.err, "");
        assert!(
            world.devpod_calls().is_empty(),
            "the help asked devpod something: {:?}",
            world.devpod_calls()
        );
    }
}

#[test]
fn the_version_is_dls_under_aids_name_with_dls_build_marker() {
    // Both halves come from `dl`, so `aid-next` and `dl-next` cannot disagree
    // about which build they are (#268): the marker is empty in the released build
    // and `-dev` in a working-tree one. **Divergence row 16**: what Python put
    // after the version here was `(dev, editable from <tree>)`, which a compiled
    // binary has no metadata for.
    let world = World::with(&["--warm"]);
    let run = world.aid(&["--version"]);
    run.exited(0);
    assert_eq!(run.out, format!("aid {VERSION}{}\n", dl::BUILD_MARKER));
    assert_eq!(run.err, "");
}

#[test]
fn a_command_line_with_no_workspace_never_reaches_dl() {
    let world = World::with(&["--warm"]);

    // `--gemini` picks an agent and names no workspace; `--devcontainer robot` takes
    // its value with it and leaves nothing either.
    for args in [vec!["--gemini"], vec!["--devcontainer", "robot"]] {
        let run = world.aid_with(&args, &[]);
        run.exited(1);
        assert_eq!(
            run.err,
            "aid needs a workspace: aid <user/repo>[@branch] [prompt]\n"
        );
        assert_eq!(run.out, "");
        assert!(world.devpod_calls().is_empty(), "{args:?} reached devpod");
    }
}

#[test]
fn an_agent_the_environment_invented_is_refused_before_anything_opens() {
    let world = World::with(&["--warm"]);
    let run = world.aid_with(&[MAIN, "hi"], &[("DEVLAUNCH_AID_AGENT", "nope")]);
    run.exited(1);
    assert_eq!(
        run.err,
        "DEVLAUNCH_AID_AGENT='nope' is not a known agent. Choose one of: claude, codex, gemini.\n"
    );
    assert!(world.devpod_calls().is_empty());
}

// ===========================================================================
// the command line it hands dl
// ===========================================================================

#[test]
fn a_prompt_reaches_the_agent_as_one_argument_through_dls_own_launch() {
    // The whole of aid, observed from outside: the rewritten command line on stderr,
    // dl's own launch of the workspace, and one `devpod ssh --command` carrying the
    // agent. Byte for byte Python's, quoting included — the payload travels in argv.
    let world = World::with(&["--warm"]);
    let run = world.aid(&[MAIN, "fix", "the", "bug"]);
    run.exited(0);
    assert_eq!(
        run.err.lines().collect::<Vec<&str>>(),
        [
            "aid -> dl devlaunch-main-3j1t -- 'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 \
             IS_SANDBOX=1 claude --dangerously-skip-permissions '\"'\"'fix the bug'\"'\"''",
            "Workspace devlaunch-main-3j1t is already running, attaching...",
            "SSH command: devpod ssh devlaunch-main-3j1t --command bash -lc \
             'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions '\"'\"'fix the bug'\"'\"''",
        ]
    );
    assert_eq!(
        world.devpod_calls(),
        [
            format!("devpod status {MAIN} --output json"),
            format!(
                "devpod ssh {MAIN} --command bash -lc \
                 'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions '\"'\"'fix the bug'\"'\"''"
            ),
        ]
    );
}

#[test]
fn no_prompt_starts_the_agents_plain_session() {
    // `test/test_interactive_command.py::TestAidReachesTheTtyTransport`'s first case
    // at the boundary: no prompt, so no prompt flags — and the same transport `dl
    // <ws>` uses, which is what makes aid a rewrite rather than a launcher. (The
    // OpenSSH pty half of that class needs a published ssh alias and is M9's.)
    let world = World::with(&["--warm"]);
    let run = world.aid(&[MAIN]);
    run.exited(0);
    assert_eq!(
        world.devpod_calls().last().expect("a session"),
        &format!(
            "devpod ssh {MAIN} --command bash -lc \
             'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions'"
        )
    );
    // `Command::output()` gives aid no terminal, and off a terminal the promptless
    // line must stay the one-shot launch it always was: no editor, no question. The
    // interactive default is pinned by `tests/interactive.rs`, on a pty.
    assert!(
        !run.err.contains("press Enter"),
        "the editor appeared without a terminal: {}",
        run.err
    );
}

#[test]
fn the_detached_cache_refresh_reaches_dl_through_aids_own_name() {
    // dl re-spawns its completion refresh through `current_exe`, which under aid
    // *is* aid — so `aid --update-cache` must be dl's `--update-cache`, not a
    // command line that lost its workspace. Before this arm existed, every refresh
    // an aid launch fired died as "aid needs a workspace" and completions silently
    // never refreshed.
    let world = World::with(&["--warm"]);
    let run = world.aid(&["--update-cache"]);
    run.exited(0);
    assert!(
        !run.err.contains("aid needs a workspace"),
        "the refresh was refused as an aid line: {}",
        run.err
    );
}

#[test]
fn each_agent_is_started_the_way_its_own_cli_takes_a_prompt() {
    // gemini's initial prompt is a flag that is a syntax error without one, so the
    // flag only appears beside a prompt; codex takes neither a flag nor a variable.
    let world = World::with(&["--warm"]);
    world.aid(&["--gemini", MAIN, "explain", "this"]).exited(0);
    assert_eq!(
        world.devpod_calls().last().expect("a session"),
        &format!(
            "devpod ssh {MAIN} --command bash -lc 'gemini --prompt-interactive '\"'\"'explain this'\"'\"''"
        )
    );

    let bare = World::with(&["--warm"]);
    bare.aid(&["--gemini", MAIN]).exited(0);
    assert_eq!(
        bare.devpod_calls().last().expect("a session"),
        &format!("devpod ssh {MAIN} --command bash -lc gemini")
    );

    let codex = World::with(&["--warm"]);
    codex.aid(&["--codex", MAIN, "hi"]).exited(0);
    assert_eq!(
        codex.devpod_calls().last().expect("a session"),
        &format!("devpod ssh {MAIN} --command bash -lc 'codex hi'")
    );
}

#[test]
fn a_dl_option_is_passed_through_and_a_flag_after_the_spec_is_prompt() {
    // `--devcontainer` reaches dl, which says what it thinks of it for a workspace
    // that is already running; `--verbose` after the spec is a word of the prompt,
    // and dl never sees it as a flag.
    let world = World::with(&["--warm"]);
    let run = world.aid(&[
        "--devcontainer",
        "robot",
        MAIN,
        "explain",
        "--verbose",
        "mode",
    ]);
    run.exited(0);
    assert!(
        run.err.contains(&format!(
            "Ignoring --devcontainer: {MAIN} is already running."
        )),
        "the option did not reach dl: {}",
        run.err
    );
    assert!(
        world
            .devpod_calls()
            .last()
            .expect("a session")
            .ends_with("'\"'\"'explain --verbose mode'\"'\"''"),
        "{:?}",
        world.devpod_calls()
    );
}

// ===========================================================================
// the exit code
// ===========================================================================

#[test]
fn the_exit_code_is_the_one_dl_ends_with() {
    // aid returns `dl.main(...)` and adds nothing: an unknown workspace is dl's
    // refusal and dl's 1, and a devpod that is not installed is dl's 127.
    let unknown = World::with(&["--warm"]);
    let refused = unknown.aid(&["nope"]);
    refused.exited(1);
    assert!(
        refused.err.contains("Unknown workspace 'nope'"),
        "{}",
        refused.err
    );

    let missing = World::with(&["--warm", "--no-devpod"]);
    let lost = missing.aid(&[MAIN]);
    lost.exited(127);
    assert!(
        lost.err.contains("devpod not found on PATH"),
        "{}",
        lost.err
    );
}

#[test]
fn a_remote_agent_that_failed_ends_with_the_agents_status() {
    // The session's own ending, whichever process the number came from — the
    // property `dl` already pins, reached through aid to show it is not re-decided
    // here.
    let world = World::with(&["--warm", "--remote-exit"]);
    world.aid(&[MAIN, "boom"]).exited(130);
}
