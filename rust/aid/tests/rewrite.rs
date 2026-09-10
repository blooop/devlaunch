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

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
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

    /// `extra` is keyed by [`OsStr`] values rather than `&str` because one of the
    /// values a caller has to be able to set is not a string: a
    /// `DEVLAUNCH_AID_AGENT` holding undecodable bytes is the case the reader used
    /// to report as unset, and it cannot be written as one.
    fn aid_with(&self, args: &[&str], extra: &[(&str, &OsStr)]) -> Run {
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
    let run = world.aid_with(
        &[MAIN, "hi"],
        &[("DEVLAUNCH_AID_AGENT", OsStr::new("nope"))],
    );
    run.exited(1);
    assert_eq!(
        run.err,
        "DEVLAUNCH_AID_AGENT='nope' is not a known agent. Choose one of: claude, codex, gemini.\n"
    );
    assert!(world.devpod_calls().is_empty());
}

#[test]
fn an_agent_name_that_does_not_decode_is_refused_rather_than_read_as_unset() {
    // The same refusal as above, for the value that used to slip past it. `aid`
    // read this variable with `std::env::var(..).ok()`, which reports a value that
    // is not valid UTF-8 as *absent* -- so `DEVLAUNCH_AID_AGENT=$'\xff'` was not a
    // broken agent name, it was no agent name, and aid quietly started the default
    // agent instead of saying the variable is wrong. That is the inversion
    // `osext::env_str` exists to prevent, one hatch along from the
    // `DEVLAUNCH_NO_TTY` one.
    //
    // The name comes back as U+FFFD because the reading is a lossy decode: the
    // undecodable byte is *present* under the replacement character, which is what
    // makes the value a name to refuse rather than a variable to ignore.
    let world = World::with(&["--warm"]);
    let undecodable = OsStr::from_bytes(b"\xff");
    let run = world.aid_with(&[MAIN, "hi"], &[("DEVLAUNCH_AID_AGENT", undecodable)]);
    run.exited(1);
    assert_eq!(
        run.err,
        "DEVLAUNCH_AID_AGENT='\u{fffd}' is not a known agent. \
         Choose one of: claude, codex, gemini.\n"
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
    //
    // The echoed `aid -> dl` line shows the tail as the words it is, because that is
    // what aid now hands dl; the `--command` below is unchanged, since dl puts back
    // the same quoting aid used to apply itself.
    //
    // Remote Control rides along with nothing typed, which is what it is now: the
    // default. The name is the spec, which here is the workspace the line named.
    let world = World::with(&["--warm"]);
    let run = world.aid(&[MAIN, "fix", "the", "bug"]);
    run.exited(0);
    assert_eq!(
        run.err.lines().collect::<Vec<&str>>(),
        [
            "aid -> dl devlaunch-main-3j1t -- CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 \
             IS_SANDBOX=1 claude --dangerously-skip-permissions \
             --remote-control=devlaunch-main-3j1t 'fix the bug'",
            "Workspace devlaunch-main-3j1t is already running, attaching...",
            "SSH command: devpod ssh devlaunch-main-3j1t --command bash -lc \
             'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions --remote-control=devlaunch-main-3j1t \
             '\"'\"'fix the bug'\"'\"''",
        ]
    );
    assert_eq!(
        world.devpod_calls(),
        [
            format!("devpod status {MAIN} --output json"),
            format!(
                "devpod ssh {MAIN} --command bash -lc \
                 'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions --remote-control={MAIN} \
                 '\"'\"'fix the bug'\"'\"''"
            ),
        ]
    );
}

#[test]
fn no_remote_control_is_the_one_way_back_to_a_purely_local_session() {
    // The off switch at the boundary, in both spellings, and the variable that turns
    // the default off for every line. All three have to leave the payload with no
    // `--remote-control` in it at all: a session half turned off is one published to
    // claude.ai by somebody who thought they had said no.
    for flag in ["--no-remote-control", "--no-remote"] {
        let world = World::with(&["--warm"]);
        world.aid(&[flag, MAIN, "hi"]).exited(0);
        assert_eq!(
            world.devpod_calls().last().expect("a session"),
            &format!(
                "devpod ssh {MAIN} --command bash -lc \
                 'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions hi'"
            ),
            "{flag}"
        );
    }

    let by_variable = World::with(&["--warm"]);
    by_variable
        .aid_with(
            &[MAIN, "hi"],
            &[("DEVLAUNCH_AID_REMOTE_CONTROL", OsStr::new("0"))],
        )
        .exited(0);
    assert!(
        !by_variable
            .devpod_calls()
            .last()
            .expect("a session")
            .contains("--remote-control"),
        "{:?}",
        by_variable.devpod_calls()
    );
}

#[test]
fn an_appended_off_switch_is_observed_from_outside_to_turn_it_off() {
    // The position the off switch is actually typed in, watched at the boundary
    // rather than in the parse. Appending to a recalled line is the cheap edit a
    // shell offers, and this line used to publish a session to claude.ai anyway and
    // hand claude `--no-remote` to read as its prompt.
    let world = World::with(&["--warm"]);
    world
        .aid(&[MAIN, "fix", "the", "bug", "--no-remote"])
        .exited(0);

    assert_eq!(
        world.devpod_calls().last().expect("a session"),
        &format!(
            "devpod ssh {MAIN} --command bash -lc \
             'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions '\"'\"'fix the bug'\"'\"''"
        )
    );
}

#[test]
fn a_remote_control_variable_that_is_neither_a_yes_nor_a_no_is_refused() {
    // Read where `DEVLAUNCH_AID_AGENT` is read and refused the same way, before
    // anything opens: both readings of a value like this are a guess, and both leave
    // somebody sure they set something they did not.
    let world = World::with(&["--warm"]);
    let run = world.aid_with(
        &[MAIN, "hi"],
        &[("DEVLAUNCH_AID_REMOTE_CONTROL", OsStr::new("maybe"))],
    );
    run.exited(1);
    assert_eq!(
        run.err,
        "DEVLAUNCH_AID_REMOTE_CONTROL='maybe' is not a yes or a no. \
         Choose one of: 1, true, on, yes, 0, false, off, no.\n"
    );
    assert!(world.devpod_calls().is_empty());
}

#[test]
fn no_prompt_starts_the_agents_plain_session() {
    // The Python `test_interactive_command::TestAidReachesTheTtyTransport`'s first
    // case at the boundary (that suite retired with the Python tree in #267, and
    // this is where the case lives now): no prompt, so no prompt flags — and the
    // same transport `dl <ws>` uses, which is what makes aid a rewrite rather than
    // a launcher. (The OpenSSH pty half of that class needs a published ssh alias
    // and is M9's.)
    let world = World::with(&["--warm"]);
    let run = world.aid(&[MAIN]);
    run.exited(0);
    assert_eq!(
        world.devpod_calls().last().expect("a session"),
        &format!(
            "devpod ssh {MAIN} --command bash -lc \
             'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions --remote-control={MAIN}'"
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
    // flag only appears beside a prompt; codex takes no prompt flag and no
    // variable. Each line also carries the agent's full-auto flag, which is the
    // spelling of one rule in three CLIs and is held as a rule by
    // `every_agent_starts_in_full_auto` (rust/aid/src/rewrite.rs).
    let world = World::with(&["--warm"]);
    world.aid(&["--gemini", MAIN, "explain", "this"]).exited(0);
    assert_eq!(
        world.devpod_calls().last().expect("a session"),
        &format!(
            "devpod ssh {MAIN} --command bash -lc 'gemini --yolo --prompt-interactive '\"'\"'explain this'\"'\"''"
        )
    );

    let bare = World::with(&["--warm"]);
    bare.aid(&["--gemini", MAIN]).exited(0);
    assert_eq!(
        bare.devpod_calls().last().expect("a session"),
        &format!("devpod ssh {MAIN} --command bash -lc 'gemini --yolo'")
    );

    // codex is the one agent whose payload carries a prefix, because it is the one
    // that cannot authenticate from a forwarded variable alone: `dl` writes it the
    // redacted `auth.json` the host's login was reduced to before starting it.
    // `devlaunch_core::clients::codex` has the whole argument. Asserted by part
    // rather than as one string, since the prefix is a shell block and the point
    // here is that codex gets one and the other agents do not.
    let codex = World::with(&["--warm"]);
    codex.aid(&["--codex", MAIN, "hi"]).exited(0);
    let session = codex.devpod_calls().last().expect("a session").clone();
    for part in [
        "DEVLAUNCH_CODEX_AUTH",
        "auth.json",
        "codex --dangerously-bypass-approvals-and-sandbox hi",
    ] {
        assert!(session.contains(part), "{part}: {session}");
    }
}

#[test]
fn remote_control_reaches_claude_as_one_named_flag_and_dl_never_sees_it() {
    // Both halves observed from outside. dl has never heard of `--remote-control`,
    // so a version of this that passed the flag through as an unknown leading
    // option would exit 2 here rather than open anything; and the session name has
    // to arrive as one argv word, since `--remote-control [name]` would otherwise
    // read the prompt as the name.
    let world = World::with(&["--warm"]);
    let run = world.aid(&["--remote-control", MAIN, "fix", "the", "bug"]);
    run.exited(0);
    assert_eq!(
        world.devpod_calls().last().expect("a session"),
        &format!(
            "devpod ssh {MAIN} --command bash -lc \
             'CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions --remote-control={MAIN} '\"'\"'fix the bug'\"'\"''"
        )
    );
}

#[test]
fn remote_control_beside_an_agent_that_has_none_is_refused_before_anything_opens() {
    // The refusal is aid's, and it lands where every other usage refusal does:
    // ahead of the boot, so nothing is created for a line that cannot run.
    let world = World::with(&["--warm"]);
    let run = world.aid(&["--codex", "--remote-control", MAIN, "hi"]);
    run.exited(1);
    assert_eq!(
        run.err,
        "--remote-control starts Claude Code's Remote Control, which only the claude agent has, \
         not codex. Drop the flag or pick --claude.\n"
    );
    assert!(world.devpod_calls().is_empty());
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

/// The full-auto table `docs/cli.md` prints, held to the lines `aid` actually runs.
///
/// The "Full auto: every agent, every launch" section writes each agent's flag into
/// a table. That is a hand-maintained copy of a fact owned by `rewrite.rs`'s agent
/// table, and this repository allows a second copy only with a test beside it that
/// diffs it against the first. `every_agent_starts_in_full_auto` in
/// `rust/aid/src/rewrite.rs` is not that test: it pins the *behaviour*, and would
/// still pass with a flag changed and the page left naming the old one.
///
/// Diffed against the command as devpod receives it, rather than against the source
/// table, so the page is checked against what a launch does and not against another
/// copy of the same list.
///
/// The set of names is asserted, not the count. Counting was the first shape of this
/// guard and Sourcery broke it on sight: three rows reading claude, claude, codex
/// satisfy a length check while the page has quietly lost gemini. The names the
/// table has to carry are therefore spelled out here, and every row is launched, so
/// a row naming an agent this build has never heard of fails at the launch rather
/// than being skipped as unrecognised.
#[test]
fn the_full_auto_section_names_the_flags_each_agent_is_actually_started_with() {
    let doc = std::fs::read_to_string(repo_root().join("docs/cli.md")).expect("docs/cli.md");
    let rows = full_auto_rows(&doc);
    let mut named: Vec<&str> = rows.iter().map(|(agent, _)| agent.as_str()).collect();
    named.sort_unstable();
    assert_eq!(
        named,
        ["claude", "codex", "gemini"],
        "docs/cli.md's full-auto table names {named:?}, not one row per agent"
    );

    for (agent, flag) in rows {
        let world = World::with(&["--warm"]);
        world.aid(&[&format!("--{agent}"), MAIN]).exited(0);
        let command = world
            .devpod_calls()
            .last()
            .unwrap_or_else(|| panic!("`aid --{agent}` opened no session"))
            .clone();

        // As a whole word and not by `contains`, which is the same trap
        // `the_force_placement_section_quotes_the_refusals_it_says_it_does` records:
        // every truncation of a flag is a substring of it, so a table reflowed or
        // mistyped down to `--d` would satisfy a substring test against a launch
        // that says `--dangerously-bypass-approvals-and-sandbox`.
        assert!(
            command
                .split_whitespace()
                .any(|word| word.trim_matches('\'') == flag),
            "docs/cli.md says `{agent}` runs with {flag:?}; the launch is {command:?}"
        );
    }
}

/// The `(agent, flag)` pairs the full-auto table states.
///
/// Matched on the heading rather than on a phrase under it, so the prose around the
/// table stays free to be rewritten while this test keeps pointing at one span. A
/// missing heading says so rather than yielding an empty section that every
/// assertion passes over.
///
/// The agent is the first cell's single backticked word; the flag is the first
/// `--word` in the second cell, which is the whole of what the copy has to get
/// right. The rest of the cell is prose and is deliberately not read.
///
/// Requiring the backticks is what separates a row from the two lines every markdown
/// table starts with. `| Agent |` is not backticked and the `| --- |` separator is
/// not either, which matters more than it looks: the separator's cells begin `--`
/// and would otherwise parse as an agent named `---` asking for a flag named `---`.
/// No allow-list of names is applied here on purpose, so an unknown name reaches the
/// caller and fails there.
fn full_auto_rows(document: &str) -> Vec<(String, String)> {
    const HEADING: &str = "## Full auto: every agent, every launch";
    let start = document
        .find(HEADING)
        .unwrap_or_else(|| panic!("docs/cli.md no longer has a '{HEADING}' section"));
    let rest = &document[start + HEADING.len()..];
    let section = match rest.find("\n## ") {
        Some(end) => &rest[..end],
        None => rest,
    };

    section
        .lines()
        .filter_map(|line| {
            let mut cells = line.trim().strip_prefix('|')?.split('|');
            let agent = cells
                .next()?
                .trim()
                .strip_prefix('`')?
                .strip_suffix('`')?
                .to_owned();
            if agent.split_whitespace().count() != 1 {
                return None;
            }
            let flag = cells
                .next()?
                .split_whitespace()
                .map(|word| {
                    word.trim_start_matches('`')
                        .trim_end_matches(|c: char| !c.is_ascii_alphanumeric())
                })
                .find(|word| word.starts_with("--"))?
                .to_owned();
            Some((agent, flag))
        })
        .collect()
}
