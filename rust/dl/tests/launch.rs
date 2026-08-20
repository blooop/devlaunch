//! The eight launch verbs, judged at the binary boundary.
//!
//! Every expectation in this file was captured by running the frozen Python build
//! — `python -m devlaunch.dl` — against `tests/launch_scenario.py`'s world with
//! `test/fixtures/devpod_shim.py` on PATH as `devpod`, under a scratch
//! `HOME`/`XDG_CACHE_HOME`/`XDG_CONFIG_HOME`/`DEVPOD_HOME`, and pasting what it
//! printed. Nothing here was read off the Rust implementation. Where the two
//! deliberately differ, the divergence row is cited beside the expectation.
//!
//! Python's diagnostics go to stderr as the bare message: `dl.py` configures
//! `logging.basicConfig(level=logging.INFO, format="%(message)s")`, so `info`,
//! `warning` and `error` all arrive with no level, no logger name and no prefix —
//! and a `debug` arrives not at all, which is why the launch lock's refusal and
//! `devpod ssh failed with exit code N` appear in none of these goldens.
//!
//! Every golden below is Python's bytes **in Python's order**, notices and session
//! output alike: core says each notice at the moment it happens (its channel is a
//! sink, not a list the binary drains at the end), so `Workspace X is already
//! running, attaching...` comes before the session it announces and the storage
//! flows' progress lines come before the work they explain.
//!
//! # The one thing that is deliberately not compared line for line
//!
//! - **A `--command` payload longer than 24 characters**, which is a 40-line POSIX
//!   script for the two provisioning trips. [`Calls::summarised`] clips it the way
//!   the golden-capture harness clipped it; the scripts themselves are pinned
//!   against Python's bytes in `devlaunch-core`'s own `flows::provision` goldens,
//!   which is where a shell fix would have to move them.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// The workspace id this build derives for `blooop/devlaunch@main`, and the one
/// `launch_scenario.py` records and devpod knows.
const MAIN: &str = "devlaunch-main-zovomobo";

/// The same for `blooop/devlaunch@cold`, which nothing has ever launched.
const COLD: &str = "devlaunch-cold-sadetohe";

/// The repository root, from the crate this test is compiled into.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the repository root")
}

/// One scratch world, and the `dl` runs against it.
struct World {
    root: PathBuf,
    _scratch: tempfile::TempDir,
}

impl World {
    fn with(fixtures: &[&str]) -> Self {
        let scratch = scratch_dir();
        let root = scratch.path().to_path_buf();
        let built = Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/launch_scenario.py"))
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

    fn dl(&self, args: &[&str]) -> Run {
        self.dl_with(args, &[])
    }

    /// Run `dl`, with `extra` on top of the world's environment.
    ///
    /// `{ROOT}` in an argument is the scratch root, so a test can name a path in
    /// the world it built.
    fn dl_with(&self, args: &[&str], extra: &[(&str, &str)]) -> Run {
        let root = self.root.display().to_string();
        let expanded: Vec<String> = args
            .iter()
            .map(|word| word.replace("{ROOT}", &root))
            .collect();
        let mut command = Command::new(env!("CARGO_BIN_EXE_dl"));
        command
            .args(&expanded)
            .env_clear()
            // /usr/bin and /bin for git, which a cold launch really runs; the fake
            // devpod is first, under its real name, and the fake `gh` is in a
            // directory of its own that only the `--gh` fixture fills.
            .env("PATH", format!("{root}/bin:{root}/gh-bin:/usr/bin:/bin"))
            .env("HOME", format!("{root}/home"))
            .env("XDG_CACHE_HOME", format!("{root}/cache"))
            .env("XDG_CONFIG_HOME", format!("{root}/config"))
            .env("DEVPOD_HOME", format!("{root}/devpod"))
            .env("DEVPOD_SHIM_STATE", format!("{root}/shim-state.json"))
            .env("DEVPOD_SHIM_LOG", format!("{root}/shim-log.jsonl"))
            .env("DEVPOD_SHIM_CONFIG", format!("{root}/shim-config.json"))
            // No network in a test: the fixture's remote is a local `origin.git`,
            // so this only ever refuses a call nothing here makes.
            .env("GIT_SSH_COMMAND", "false")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null");
        // Make the world hermetic about gh: the `--gh` fixture puts a fake gh in
        // gh-bin, and a test without it means "no gh". PATH still carries /usr/bin
        // for git, and a CI runner has a real /usr/bin/gh there — so without this
        // the runner's gh leaks in, runs against the scratch config, exits 1, and
        // adds a "no GitHub login" line (and a `gh auth token` span) these tests
        // do not expect. The opt-out reproduces the no-gh world deterministically.
        if !self.root.join("gh-bin/gh").exists() {
            command.env("DEVLAUNCH_NO_GH_TOKEN", "1");
        }
        for (name, value) in extra {
            command.env(name, value);
        }
        let output = command.output().expect("the dl binary runs");
        Run::of(&output, &self.root)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// An `ssh` that fails the way a host refusing a clone fails, saying *reason*.
    ///
    /// The world's stock `GIT_SSH_COMMAND` is `false`, which is silent — enough to
    /// keep a test offline, and useless for anything that reads what the host said.
    /// Returned as a path for the caller to pass back in, so the wording is the
    /// test's own.
    fn fake_ssh(&self, reason: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;

        let path = self.path("bin/ssh-refusing");
        std::fs::write(&path, format!("#!/bin/sh\necho '{reason}' >&2\nexit 128\n"))
            .expect("the fake ssh is written");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("the fake ssh is executable");
        path
    }

    /// A completion cache holding *repos* and nothing else.
    ///
    /// Only the `repos` list is filled: it is the one list the wrong-owner hint
    /// reads, and a cache dl wrote itself would carry branches and workspaces this
    /// has no use for.
    fn write_completion_cache(&self, repos: &[&str]) {
        let dir = self.path("cache/devlaunch");
        std::fs::create_dir_all(&dir).expect("the cache directory");
        let repos = repos
            .iter()
            .map(|repo| format!("\"{repo}\""))
            .collect::<Vec<_>>()
            .join(", ");
        std::fs::write(
            dir.join("completions.json"),
            format!(
                "{{\"workspaces\": [], \"repos\": [{repos}], \"owners\": [], \"branches\": []}}"
            ),
        )
        .expect("the completion cache is written");
    }

    /// The devpod calls made so far, in order.
    ///
    /// **`devpod list --output json` is left out**, and that is the known race
    /// rather than a convenience: every one of these verbs ends by spawning the
    /// detached completion refresh, whose first act is a `devpod list` into this
    /// very log, and whether it lands before the parent exits is a matter of
    /// scheduling. No launch path asserted here makes a `list` call of its own —
    /// the one that does is the unknown-workspace refusal, which spawns no
    /// refresh, and which asks for the full log through [`Calls::including_list`].
    fn calls(&self) -> Calls {
        Calls {
            argvs: self.raw_calls(),
            keep_list: false,
        }
    }

    /// The same log with the listing left in, for a command that spawns no child.
    fn calls_including_list(&self) -> Calls {
        Calls {
            argvs: self.raw_calls(),
            keep_list: true,
        }
    }

    fn raw_calls(&self) -> Vec<Vec<String>> {
        std::fs::read_to_string(self.path("shim-log.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let call: serde_json::Value = serde_json::from_str(line).expect("a log line");
                call["argv"]
                    .as_array()
                    .expect("an argv")
                    .iter()
                    .map(|word| word.as_str().expect("a word").to_owned())
                    .collect()
            })
            .collect()
    }

    /// Whether `relative` appears within as long as a detached child could
    /// reasonably take. The one thing a detached child can be observed by from out
    /// here: nothing waits on it, so its arrival is the assertion.
    fn appears(&self, relative: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if self.path(relative).exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }
}

/// The devpod calls one run made, as lines a golden can be pasted as.
struct Calls {
    argvs: Vec<Vec<String>>,
    keep_list: bool,
}

impl Calls {
    /// One `devpod <argv>` line per call, with every word whole.
    fn exact(&self, root: &Path) -> Vec<String> {
        self.lines(root, |word| word.to_owned())
    }

    /// The same, with a word over 24 characters clipped to its first line: this is
    /// how the golden-capture harness rendered the two provisioning payloads, whose
    /// bytes are pinned in core rather than here.
    fn summarised(&self, root: &Path) -> Vec<String> {
        self.lines(root, |word| {
            if word.chars().count() <= 24 {
                return word.to_owned();
            }
            let head: String = word.lines().next().unwrap_or("").chars().take(24).collect();
            format!("{head}…")
        })
    }

    fn lines(&self, root: &Path, shape: impl Fn(&str) -> String) -> Vec<String> {
        let template = root.display().to_string();
        self.argvs
            .iter()
            .filter(|argv| self.keep_list || argv.first().map(String::as_str) != Some("list"))
            .map(|argv| {
                let words: Vec<String> = argv
                    .iter()
                    .map(|word| shape(&word.replace(&template, "{ROOT}")))
                    .collect();
                format!("devpod {}", words.join(" "))
            })
            .collect()
    }
}

/// A scratch directory whose path is always the same *length*, as the two sibling
/// suites make: the golden-capture harness makes `/tmp/dltXXXXXX` too, and a path
/// length that varies between capture and comparison is one more thing to think
/// about.
fn scratch_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("dlt")
        .rand_bytes(6)
        .tempdir_in("/tmp")
        .expect("a scratch directory under /tmp")
}

/// What one run printed and how it ended, with the scratch root templated out.
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

    fn stderr_lines(&self) -> Vec<&str> {
        self.err.lines().collect()
    }
}

// ===========================================================================
// dl <ws> and dl <spec>: the fast attach
// ===========================================================================

#[test]
fn a_warm_attach_is_one_status_probe_and_then_the_session() {
    // The whole of devlaunch#145 as an observation: a workspace devpod already
    // reports as Running is attached to without a `devpod up` and without the
    // records ever being opened.
    let world = World::with(&["--warm"]);
    let run = world.dl(&[MAIN]);
    run.exited(0);
    assert_eq!(run.out, "");
    assert_eq!(
        run.stderr_lines(),
        [
            &format!("Workspace {MAIN} is already running, attaching...") as &str,
            &format!("SSH command: devpod ssh {MAIN}"),
        ]
    );
    assert_eq!(
        world.calls().exact(&world.root),
        [
            format!("devpod status {MAIN} --output json"),
            format!("devpod ssh {MAIN}"),
        ]
    );
}

#[test]
fn a_warm_triple_asks_devpod_about_the_derived_id_and_stops_there() {
    // The same two calls for `owner/repo@branch` as for the bare id: the derived id
    // is the hint, devpod recognises it, and no record is read to second-guess it.
    let world = World::with(&["--warm"]);
    let run = world.dl(&["blooop/devlaunch@main"]);
    run.exited(0);
    assert_eq!(
        world.calls().exact(&world.root),
        [
            format!("devpod status {MAIN} --output json"),
            format!("devpod ssh {MAIN}"),
        ]
    );
}

#[test]
fn a_command_travels_as_one_quoted_bash_lc_payload() {
    // Byte parity on the payload, which is a contract with a remote shell rather
    // than prose for a person: `shlex.quote`'s spelling, and the login shell that
    // gives `-- <cmd>` the same PATH an interactive attach gets.
    let world = World::with(&["--warm"]);
    let run = world.dl(&[MAIN, "--", "echo", "hi"]);
    run.exited(0);
    assert_eq!(
        run.stderr_lines()[1],
        format!("SSH command: devpod ssh {MAIN} --command bash -lc 'echo hi'")
    );
    assert_eq!(
        world.calls().exact(&world.root),
        [
            format!("devpod status {MAIN} --output json"),
            format!("devpod ssh {MAIN} --command bash -lc 'echo hi'"),
        ]
    );
}

#[test]
fn a_quoted_prompt_reaches_the_agent_intact() {
    // The payload travels in argv and argv is what the harness compares, so the
    // *spelling* of the quoting is the contract and not just its meaning: Python
    // always single-quotes and escapes each `'` as `'"'"'`, where the `shlex` crate
    // would switch to double quotes for the same word. Both are the same word to a
    // POSIX shell and only one of them is the same bytes.
    let world = World::with(&["--warm"]);
    let run = world.dl(&[MAIN, "--", "claude", "it's here"]);
    run.exited(0);
    assert_eq!(
        world.calls().exact(&world.root).last(),
        Some(&format!(
            "devpod ssh {MAIN} --command bash -lc 'claude it'\"'\"'s here'"
        ))
    );
}

#[test]
fn the_zellij_opt_in_puts_a_session_beside_the_command() {
    // #242, and the `;` rather than `&&` is the point: what the payload exits with
    // is the command's status, never the session setup's.
    let world = World::with(&["--warm"]);
    let run = world.dl_with(
        &[MAIN, "--", "claude", "fix it"],
        &[("DEVLAUNCH_ZELLIJ", "1")],
    );
    run.exited(0);
    assert_eq!(
        run.stderr_lines()[1],
        format!(
            "SSH command: devpod ssh {MAIN} --command bash -lc 'zellij attach -b devlaunch \
             >/dev/null 2>&1 || true; claude fix it'"
        )
    );
}

#[test]
fn a_devcontainer_choice_a_running_workspace_cannot_honour_is_said_not_discarded() {
    let world = World::with(&["--warm"]);
    let run = world.dl(&[MAIN, "--devcontainer", "robot"]);
    run.exited(0);
    assert_eq!(
        run.stderr_lines()[1],
        format!(
            "Ignoring --devcontainer: {MAIN} is already running. Use 'dl {MAIN} recreate \
             --devcontainer ...' to switch config."
        )
    );
}

#[test]
fn a_warm_triple_launch_does_no_metadata_io_at_all() {
    // devlaunch#145, observed on disk rather than through a mock. The cache is
    // seeded with an unparsable `metadata.json`: any path that *reads* it
    // quarantines it to `metadata.json.corrupt` and says so, and any path that takes
    // the metadata lock leaves `metadata.json.lock` behind. A launch that did no
    // metadata I/O leaves the garbage byte-identical and creates neither sibling.
    let world = World::with(&["--warm"]);
    std::fs::write(world.path("cache/devlaunch/metadata.json"), "not json")
        .expect("a corrupt document");

    let run = world.dl(&["blooop/devlaunch@main", "--", "echo", "hi"]);
    run.exited(0);

    assert_eq!(
        std::fs::read_to_string(world.path("cache/devlaunch/metadata.json")).unwrap(),
        "not json"
    );
    assert!(!world.path("cache/devlaunch/metadata.json.corrupt").exists());
    assert!(!world.path("cache/devlaunch/metadata.json.lock").exists());
    // And the shape wayfinder hands dl for every agent launch is still two trips.
    assert_eq!(
        world.calls().exact(&world.root),
        [
            format!("devpod status {MAIN} --output json"),
            format!("devpod ssh {MAIN} --command bash -lc 'echo hi'"),
        ]
    );
}

// ===========================================================================
// the opt-in dotfiles refresh
// ===========================================================================

#[test]
fn the_dotfiles_opt_in_refreshes_in_front_of_an_interactive_shell() {
    // devlaunch#183: off unless switched on, because the fix costs a ~1.73s round
    // trip in front of every shell. In front of the session and not behind it,
    // because the shell being handed over is the whole point — dotfiles that landed
    // after it started are dotfiles it has already finished sourcing.
    let world = World::with(&["--warm"]);
    let run = world.dl_with(&[MAIN], &[("DEVLAUNCH_DOTFILES_ON_ATTACH", "1")]);
    run.exited(0);
    assert_eq!(
        world.calls().summarised(&world.root),
        [
            format!("devpod status {MAIN} --output json"),
            "devpod context options --output json".to_owned(),
            // Bounded, and spent *inside* the container: `timeout` puts the managed
            // command in its own process group, so the git or pixi process actually
            // waiting dies with the shell that started it rather than holding the
            // session open.
            format!("devpod ssh {MAIN} --command bash -lc 'timeout 60 bas…"),
            format!("devpod ssh {MAIN}"),
        ]
    );
}

#[test]
fn a_one_shot_command_is_not_worth_a_dotfiles_refresh() {
    // The same reasoning the hostname round-trip was skipped for: `dl <ws> -- cmd`
    // renders no prompt and sources no interactive shell, so a refresh in front of it
    // buys it nothing and costs it a round trip. That path is the shape wayfinder
    // hands dl for every agent launch.
    let world = World::with(&["--warm"]);
    let run = world.dl_with(
        &[MAIN, "--", "echo", "hi"],
        &[("DEVLAUNCH_DOTFILES_ON_ATTACH", "1")],
    );
    run.exited(0);
    assert_eq!(
        world.calls().exact(&world.root),
        [
            format!("devpod status {MAIN} --output json"),
            format!("devpod ssh {MAIN} --command bash -lc 'echo hi'"),
        ]
    );
}

#[test]
fn a_switch_set_to_a_denial_is_still_off() {
    // `_FALSEY`: a variable exported with no value, or set to `0`/`false`/`no`, means
    // no. Anything else a user went out of their way to set means what they set it
    // for.
    for denial in ["", "0", "false", "NO"] {
        let world = World::with(&["--warm"]);
        let run = world.dl_with(&[MAIN], &[("DEVLAUNCH_DOTFILES_ON_ATTACH", denial)]);
        run.exited(0);
        assert_eq!(
            world.calls().exact(&world.root),
            [
                format!("devpod status {MAIN} --output json"),
                format!("devpod ssh {MAIN}"),
            ],
            "DEVLAUNCH_DOTFILES_ON_ATTACH={denial:?} refreshed anyway"
        );
    }
}

// ===========================================================================
// the cold path
// ===========================================================================

/// The three flags that put a container on the host's shared pixi package cache,
/// which every `devpod up` dl makes carries and which end every one of these argvs.
///
/// The third is not a duplicate of the second: devpod gives the dotfiles install
/// script an environment of its own, so a variable set only for the workspace never
/// reaches the `pixi global sync` that is the whole consumer of this cache.
const PIXI: &str = "--mount type=bind,source={ROOT}/cache/devlaunch/pixi,target=/var/tmp/\
                    devlaunch-pixi --workspace-env PIXI_CACHE_DIR=/var/tmp/devlaunch-pixi \
                    --dotfiles-script-env PIXI_CACHE_DIR=/var/tmp/devlaunch-pixi";

/// The one `devpod up` line out of a call log, byte for byte.
///
/// Byte-exact where the sequences around it are summarised, because this argv *is*
/// the contract: a flag dropped or reordered here is a container built differently.
fn up_argv(world: &World) -> String {
    let calls = world.calls().exact(&world.root);
    let up = calls
        .iter()
        .find(|call| call.starts_with("devpod up "))
        .unwrap_or_else(|| panic!("no `devpod up` in {calls:?}"));
    up.clone()
}

#[test]
fn a_cold_triple_prepares_a_clone_creates_the_workspace_and_attaches() {
    let world = World::with(&[]);
    let run = world.dl(&["blooop/devlaunch@cold"]);
    run.exited(0);
    // devpod's own line, on stdout because the `up` inherits this process's streams.
    assert_eq!(run.out, format!("Workspace {COLD} is ready\n"));
    // The host's own work first, said as it happens: the one targeted fetch, what the
    // branch step found, and the clone about to be cut. Then the two setup stages,
    // which report nothing because the fake devpod's `ssh` runs no remote command —
    // the same answer a real container with no `readlink` gives, named rather than
    // passed over.
    assert_eq!(
        run.stderr_lines(),
        [
            "Fetching cold for blooop/devlaunch",
            "Branch cold already exists locally and remotely",
            &format!(
                "Creating workspace clone at {{ROOT}}/cache/devlaunch/repos/blooop/devlaunch/{COLD}"
            ),
            &format!("{COLD}: the hostname setup stage did not report; it may not have run."),
            &format!("{COLD}: the zellij setup stage did not report; it may not have run."),
            &format!("SSH command: devpod ssh {COLD}"),
        ]
    );
    assert_eq!(
        world.calls().summarised(&world.root),
        [
            // The derived id first: cold is what devpod *denying* it means.
            format!("devpod status {COLD} --output json"),
            "devpod context options --output json".to_owned(),
            "devpod up {ROOT}/cache/devlaunch/r… --id devlaunch-cold-sadetohe --ide none \
             --init-env DEVLAUNCH_WORKSPACE_ID=d… --mount type=bind,source={ROOT}/… \
             --workspace-env PIXI_CACHE_DIR=/var/tmp/… --dotfiles-script-env \
             PIXI_CACHE_DIR=/var/tmp/…"
                .to_owned(),
            // Then the three trips the tools cost: the setup pass, the network
            // install a container with nothing gets, and the session.
            format!("devpod ssh {COLD} --command bash -lc 'if sudo hostna…"),
            format!("devpod ssh {COLD} --command bash -lc 'set -u…"),
            format!("devpod ssh {COLD}"),
        ]
    );
    // The clone as the source, the derived id as `--id` (a create), `--ide none` so
    // devpod opens no editor over dl's own shell, and the stamp a project's
    // host-side `initializeCommand` reads to tell branch workspaces apart.
    assert_eq!(
        up_argv(&world),
        format!(
            "devpod up {{ROOT}}/cache/devlaunch/repos/blooop/devlaunch/{COLD} --id {COLD} \
             --ide none --init-env DEVLAUNCH_WORKSPACE_ID={COLD} {PIXI}"
        )
    );
    // The clone and the record the preparation made are the cold path's own trace.
    assert!(
        world
            .path(&format!("cache/devlaunch/repos/blooop/devlaunch/{COLD}"))
            .is_dir(),
        "the workspace clone was not created"
    );
}

#[test]
fn a_bare_owner_repo_names_the_default_branch_before_it_derives_an_id() {
    // No `@branch`, so the default branch has to be named first — from the record
    // here, which is what keeps this offline — and the id derived from it is the one
    // devpod is asked about.
    let world = World::with(&["--no-workspaces"]);
    let run = world.dl(&["blooop/devlaunch"]);
    run.exited(0);
    assert!(
        world
            .calls()
            .summarised(&world.root)
            .starts_with(&[format!("devpod status {MAIN} --output json")]),
        "expected the derived id to be asked about first: {:?}",
        world.calls().summarised(&world.root)
    );
}

#[test]
fn a_path_spec_is_named_lexically_symlinks_and_all() {
    // **Divergence row 20.** `foreign/link` points at `foreign/real`; Python's
    // `Path.resolve()` followed the link and named the workspace `real`, and this
    // names it `link`. Every other byte of the two runs is the same, which is what
    // the row promises: identical for every real path without symlinked components,
    // and the name a user typed is the name they get.
    let world = World::with(&["--symlinked-path"]);
    let run = world.dl(&["{ROOT}/foreign/link"]);
    run.exited(0);
    assert_eq!(run.out, "Workspace link is ready\n");
    // Nothing is asked of devpod before the `up`: everything creatable goes through
    // `up`, which is idempotent for a workspace devpod already has.
    assert_eq!(
        world.calls().summarised(&world.root)[0],
        "devpod context options --output json"
    );
    // Python's line for the same run names `real` in all three places this names
    // `link`, and is otherwise identical.
    assert_eq!(
        up_argv(&world),
        format!(
            "devpod up {{ROOT}}/foreign/link --id link --ide none --init-env \
             DEVLAUNCH_WORKSPACE_ID=link {PIXI}"
        )
    );
}

// ===========================================================================
// dl <ws> up, code
// ===========================================================================

#[test]
fn up_on_a_running_workspace_says_so_and_still_provisions_the_tools() {
    // dl.py:4770. `up` is one of the two verbs named as how a workspace that missed
    // provisioning gets it, so returning here without the tools would make the
    // documented recovery the one path that cannot recover. One setup-pass round
    // trip against a workspace that is already up, and no `devpod up` at all.
    let world = World::with(&["--warm"]);
    let run = world.dl(&[MAIN, "up"]);
    run.exited(0);
    assert_eq!(
        run.stderr_lines(),
        [
            // In Python's order: the workspace is reported already up *before* the
            // pass that tops its tools up runs, because that is when it was found.
            &format!("Workspace {MAIN} is already running.") as &str,
            &format!("{MAIN}: the hostname setup stage did not report; it may not have run."),
            &format!("{MAIN}: the zellij setup stage did not report; it may not have run."),
        ]
    );
    assert_eq!(
        world.calls().summarised(&world.root),
        [
            format!("devpod status {MAIN} --output json"),
            format!("devpod ssh {MAIN} --command bash -lc 'if sudo hostna…"),
            format!("devpod ssh {MAIN} --command bash -lc 'set -u…"),
        ]
    );
}

#[test]
fn up_on_a_stopped_workspace_brings_it_up_and_hands_over_no_session() {
    let world = World::with(&["--stopped"]);
    let run = world.dl(&[MAIN, "up"]);
    run.exited(0);
    assert_eq!(run.out, format!("Workspace {MAIN} is ready\n"));
    let calls = world.calls().summarised(&world.root);
    assert_eq!(
        calls[..2],
        [
            format!("devpod status {MAIN} --output json"),
            "devpod context options --output json".to_owned(),
        ]
    );
    // No `--id`: devpod already knows this workspace, so `up` addresses it by the id
    // it answers to and passes no create-id at all.
    assert_eq!(
        up_argv(&world),
        format!("devpod up {MAIN} --ide none --init-env DEVLAUNCH_WORKSPACE_ID={MAIN} {PIXI}")
    );
    // The session is the one call that is *not* here: `up` is the warm half of a
    // launch, for a caller that wants the container ready before a user arrives.
    assert!(
        !calls
            .iter()
            .any(|call| call == &format!("devpod ssh {MAIN}")),
        "up attached: {calls:?}"
    );
}

#[test]
fn code_asks_devpod_for_the_ide_and_hands_over_no_session() {
    // An IDE to open is a request a running workspace is not the answer to, so
    // `code` runs `devpod up` even on the workspace the fast attach would have
    // taken — and then leaves, because the editor is the session.
    let world = World::with(&["--warm"]);
    let run = world.dl(&[MAIN, "code"]);
    run.exited(0);
    assert_eq!(
        up_argv(&world),
        format!("devpod up {MAIN} --ide vscode --init-env DEVLAUNCH_WORKSPACE_ID={MAIN} {PIXI}")
    );
    let calls = world.calls().summarised(&world.root);
    assert!(
        !calls
            .iter()
            .any(|call| call == &format!("devpod ssh {MAIN}")),
        "code attached: {calls:?}"
    );
}

// ===========================================================================
// restart, recreate, reset
// ===========================================================================

#[test]
fn restart_stops_and_starts_without_rebuilding_then_attaches() {
    let world = World::with(&["--warm"]);
    let run = world.dl(&[MAIN, "restart"]);
    run.exited(0);
    let calls = world.calls().summarised(&world.root);
    assert_eq!(
        calls[..3],
        [
            format!("devpod status {MAIN} --output json"),
            format!("devpod stop {MAIN}"),
            "devpod context options --output json".to_owned(),
        ]
    );
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with(&format!("devpod up {MAIN} --ide none"))),
        "expected a plain up: {calls:?}"
    );
    assert_eq!(
        calls.last(),
        Some(&format!("devpod ssh {MAIN}")),
        "restart did not attach: {calls:?}"
    );
}

#[test]
fn a_stop_that_refuses_takes_the_restart_with_it_and_says_nothing() {
    // devpod's own diagnostics are already on this process's stderr, so dl has
    // nothing to add but the status — and nothing was started.
    let world = World::with(&["--warm", "--fail-stop"]);
    let run = world.dl(&[MAIN, "restart"]);
    run.exited(9);
    assert_eq!(run.err, "devpod: provider is gone\n");
    assert_eq!(
        world.calls().summarised(&world.root),
        [
            format!("devpod status {MAIN} --output json"),
            format!("devpod stop {MAIN}"),
        ]
    );
}

#[test]
fn recreate_and_reset_each_pass_their_own_flag_and_then_attach() {
    for (verb, flag) in [("recreate", "--recreate"), ("reset", "--reset")] {
        let world = World::with(&["--stopped"]);
        let run = world.dl(&[MAIN, verb]);
        run.exited(0);
        // The rebuild flag goes after the stamp and before the pixi mount, which is
        // Python's order.
        assert_eq!(
            up_argv(&world),
            format!(
                "devpod up {MAIN} --ide none --init-env DEVLAUNCH_WORKSPACE_ID={MAIN} {flag} \
                 {PIXI}"
            )
        );
        let calls = world.calls().summarised(&world.root);
        assert_eq!(
            calls.last(),
            Some(&format!("devpod ssh {MAIN}")),
            "{verb} did not attach: {calls:?}"
        );
    }
}

// ===========================================================================
// dotfiles
// ===========================================================================

#[test]
fn dotfiles_refreshes_a_running_workspace_without_bringing_anything_up() {
    let world = World::with(&["--warm"]);
    let run = world.dl(&[MAIN, "dotfiles"]);
    run.exited(0);
    let calls = world.calls().summarised(&world.root);
    assert_eq!(
        calls,
        [
            // Two state probes and not one, which is Python's shape: the spec
            // resolution asks whether devpod knows this workspace, and the verb asks
            // again whether it is *running*. The second is the one that decides
            // whether anything is brought up.
            format!("devpod status {MAIN} --output json"),
            format!("devpod status {MAIN} --output json"),
            // Then the context options the refresh's fallback clone URL comes from,
            // and one session carrying the refresh.
            "devpod context options --output json".to_owned(),
            format!("devpod ssh {MAIN} --command bash -lc 'if command -v …"),
        ]
    );
    // Unbounded, unlike the refresh nobody asked for: this one is typed, in the
    // foreground and interruptible, and a deadline on it is a way to lose work.
    assert!(
        !run.err.contains("timeout "),
        "the foreground refresh was bounded: {}",
        run.err
    );
}

#[test]
fn dotfiles_starts_a_stopped_workspace_first_and_says_so() {
    let world = World::with(&["--stopped"]);
    let run = world.dl(&[MAIN, "dotfiles"]);
    run.exited(0);
    assert_eq!(
        run.stderr_lines()[0],
        format!("Starting workspace {MAIN}...")
    );
    let calls = world.calls().summarised(&world.root);
    // The `up` this verb makes passes **no** `--init-env` stamp, because Python's
    // call passes only `workspace_id=custom_id`: for a workspace devpod already
    // knows there is no identity, and so no launch lock and no tools either.
    assert_eq!(
        up_argv(&world),
        format!("devpod up {MAIN} --ide none {PIXI}"),
        "the dotfiles up must pass no workspace identity"
    );
    assert!(
        calls.iter().any(|call| {
            call.starts_with(&format!(
                "devpod ssh {MAIN} --command bash -lc 'if command -v "
            ))
        }),
        "the dotfiles refresh did not run: {calls:?}"
    );
}

// ===========================================================================
// the refusals and the exit codes
// ===========================================================================

#[test]
fn an_unknown_workspace_is_refused_only_after_both_answers() {
    // `status` failing is not the same as the workspace not existing, and the
    // difference decides whether the user can clean it up — so the listing gets the
    // final word. It costs a round trip only on the failure path.
    let world = World::with(&["--warm"]);
    let run = world.dl(&["nope"]);
    run.exited(1);
    assert_eq!(
        run.err,
        "Unknown workspace 'nope'. Use 'dl --ls' to list workspaces, or specify owner/repo or \
         ./path\n"
    );
    assert_eq!(
        world.calls_including_list().exact(&world.root),
        [
            "devpod status nope --output json",
            "devpod list --output json",
        ]
    );
}

#[test]
fn a_target_no_spec_shape_matches_gets_the_unknown_workspace_refusal() {
    // `a b/repo@main` has a space in it, so nothing parses it as `owner/repo@ref`
    // and it can only be a bare name — which is the shape the refusal above
    // describes. Python answers identically, and for the same reason.
    let world = World::with(&["--warm"]);
    let run = world.dl(&["a b/repo@main"]);
    run.exited(1);
    assert_eq!(
        run.err,
        "Unknown workspace 'a b/repo@main'. Use 'dl --ls' to list workspaces, or specify \
         owner/repo or ./path\n"
    );
}

#[test]
fn a_repository_that_cannot_be_cloned_says_so_in_gits_own_words() {
    // The line a mistyped repository name ends at. `blooop/other` has no clone in the
    // cache and its remote is the derived GitHub URL, which `GIT_SSH_COMMAND=false`
    // makes unreachable — so this is the ordinary "that repo does not exist" failure,
    // offline. Python's two lines, and the second one carries git's own stderr rather
    // than a rendering of dl's error type.
    let world = World::with(&["--warm"]);
    let run = world.dl(&["blooop/other"]);
    run.exited(1);
    let said = run.stderr_lines();
    assert_eq!(
        said[0],
        "Cloning repository git@github.com:blooop/other.git to \
         {ROOT}/cache/devlaunch/repos/blooop/other/.bare"
    );
    // The rest of the line is git's, whose wording is git's version's; what is dl's
    // is the frame around it, and that is what is pinned.
    assert!(
        said[1].starts_with("Repository 'blooop/other': Failed to clone repository: "),
        "{:?}",
        said[1]
    );
    assert!(
        said.iter()
            .any(|line| line.contains("Could not read from remote repository")),
        "git's own reason did not reach the user: {said:?}"
    );
    // Nothing was asked of devpod: the host could not prepare a workspace to open.
    assert!(world.calls().exact(&world.root).is_empty());
}

#[test]
fn a_repository_not_found_under_one_owner_is_offered_the_owner_dl_knows() {
    // The wrong-owner case, which git cannot diagnose: `kinisi/kinisi_ros` is a
    // repository the host says it has not got, and `kinisi-robotics/kinisi_ros` is
    // in the same completion cache the shell offers. git's own words stay — they
    // are what a reader with no cache entry has — and the second line names the
    // spec to run instead.
    let world = World::with(&["--warm"]);
    let ssh = world.fake_ssh("ERROR: Repository not found.");
    world.write_completion_cache(&["blooop/devlaunch", "kinisi-robotics/kinisi_ros"]);

    let run = world.dl_with(
        &["kinisi/kinisi_ros"],
        &[("GIT_SSH_COMMAND", ssh.to_str().expect("a utf-8 path"))],
    );

    run.exited(1);
    let said = run.stderr_lines();
    assert!(
        said.iter()
            .any(|line| line.contains("Failed to clone repository: ")),
        "git's own refusal did not reach the user: {said:?}"
    );
    assert!(
        said.contains(
            &"Did you mean 'dl kinisi-robotics/kinisi_ros'? git could not find \
              'kinisi/kinisi_ros', and dl knows that repository name under another owner."
        ),
        "the wrong-owner hint is missing: {said:?}"
    );
}

#[test]
fn a_spec_with_a_branch_is_offered_the_same_branch_under_the_owner_dl_knows() {
    // A spec that names a branch fails one step further along — the preparation
    // rather than the default-branch lookup — and the suggestion has to be runnable
    // as it reads, so it carries the branch back.
    let world = World::with(&["--warm"]);
    let ssh = world.fake_ssh("ERROR: Repository not found.");
    world.write_completion_cache(&["kinisi-robotics/kinisi_ros"]);

    let run = world.dl_with(
        &["kinisi/kinisi_ros@fix/support-polygon"],
        &[("GIT_SSH_COMMAND", ssh.to_str().expect("a utf-8 path"))],
    );

    run.exited(1);
    let said = run.stderr_lines();
    assert!(
        said.iter().any(|line| line
            .starts_with("Did you mean 'dl kinisi-robotics/kinisi_ros@fix/support-polygon'?")),
        "the branch did not travel with the suggestion: {said:?}"
    );
}

#[test]
fn a_repository_no_owner_in_the_cache_has_is_left_with_gits_words() {
    // The other half: a name the cache cannot second-guess gets no "did you mean",
    // because there is nothing to mean. A guess here would be noise on every
    // first-ever clone of a repository.
    let world = World::with(&["--warm"]);
    let ssh = world.fake_ssh("ERROR: Repository not found.");
    world.write_completion_cache(&["blooop/devlaunch"]);

    let run = world.dl_with(
        &["blooop/never-heard-of-it"],
        &[("GIT_SSH_COMMAND", ssh.to_str().expect("a utf-8 path"))],
    );

    run.exited(1);
    let said = run.stderr_lines();
    assert!(
        !said.iter().any(|line| line.starts_with("Did you mean")),
        "a repository with no candidate was guessed at anyway: {said:?}"
    );
}

#[test]
fn a_clone_that_failed_on_something_other_than_a_missing_repository_is_not_second_guessed() {
    // A refused key or a dead network names a repository that may well exist under
    // the owner given, and "did you mean" would send the reader after a problem
    // they have not got. Only the host's own not-found wording earns the line.
    //
    // GitHub's refused-key message in full, because its last line is the trap the
    // classification has to survive: git's stock ssh advice ends "and the
    // repository exists", one word away from the wording that *does* earn a hint.
    let world = World::with(&["--warm"]);
    let ssh = world.fake_ssh(
        "git@github.com: Permission denied (publickey).\n\
         fatal: Could not read from remote repository.\n\
         \n\
         Please make sure you have the correct access rights\n\
         and the repository exists.",
    );
    world.write_completion_cache(&["kinisi-robotics/kinisi_ros"]);

    let run = world.dl_with(
        &["kinisi/kinisi_ros"],
        &[("GIT_SSH_COMMAND", ssh.to_str().expect("a utf-8 path"))],
    );

    run.exited(1);
    let said = run.stderr_lines();
    assert!(
        !said.iter().any(|line| line.starts_with("Did you mean")),
        "a credential failure was read as a wrong owner: {said:?}"
    );
}

#[test]
fn a_devpod_up_that_refuses_hands_its_own_status_back_and_adds_nothing() {
    let world = World::with(&["--stopped", "--fail-up"]);
    let run = world.dl(&[MAIN]);
    run.exited(7);
    assert_eq!(run.err, "devpod: image pull failed\n");
    // No session: there is no container to attach to.
    assert!(
        !world
            .calls()
            .exact(&world.root)
            .iter()
            .any(|call| call.starts_with(&format!("devpod ssh {MAIN}"))),
        "a refused up still attached"
    );
}

#[test]
fn a_session_devpod_could_not_start_ends_with_devpods_own_status() {
    let world = World::with(&["--warm", "--fail-session"]);
    let run = world.dl(&[MAIN]);
    run.exited(3);
    // dl's two notices and then devpod's forwarded line, which is the order all
    // three happened in: both channels are live, so a session's own diagnostics land
    // after the lines that announced the session.
    assert_eq!(
        run.stderr_lines(),
        [
            "Workspace devlaunch-main-zovomobo is already running, attaching...",
            "SSH command: devpod ssh devlaunch-main-zovomobo",
            "devpod: connection refused",
        ]
    );
}

#[test]
fn a_remote_program_that_exited_130_ends_130_and_not_devpods_1() {
    // devpod reports 1 for every nonzero remote exit and buries the real status in
    // a `fatal` line, because its own error is wrapped three times before the type
    // assertion that would have found it. The status belongs to the remote program,
    // and the line it was buried in is held back rather than shown.
    let world = World::with(&["--warm", "--remote-exit"]);
    let run = world.dl(&[MAIN, "--", "false"]);
    run.exited(130);
    assert!(
        !run.err.contains("Process exited with status"),
        "devpod's own report of the status reached the user: {}",
        run.err
    );
    assert!(
        !run.err.contains("--debug flag"),
        "the hint that introduced the withheld fatal was released on its own: {}",
        run.err
    );
}

/// The one line a host with no devpod on it gets, whatever it asked for.
const MISSING_DEVPOD: &str = "devpod not found on PATH: dl cannot manage workspaces without it. Install devpod from \
     https://devpod.sh/docs/getting-started/install (pixi/conda installs of devlaunch include \
     it; pip installs do not).\n";

#[test]
fn a_missing_devpod_is_exit_127_and_the_line_that_names_both_installs() {
    let world = World::with(&["--warm", "--no-devpod"]);
    let run = world.dl(&[MAIN]);
    run.exited(127);
    assert_eq!(run.err, MISSING_DEVPOD);
}

#[test]
fn a_missing_devpod_stops_a_cold_launch_before_it_clones_anything() {
    // The fast-attach probe is the first thing a `owner/repo@branch` launch does,
    // and a devpod that is not installed is not an answer of "no such workspace":
    // Python's `get_workspace_state` raises `DevpodNotInstalled` out of
    // `resolve_known_workspace`, so the launch ends at the probe with nothing on
    // disk. Reading that failure as a denial instead sends the launch down the
    // cold path -- which fetches the branch and builds a workspace clone on a host
    // that cannot open it, prints three progress lines nobody can act on, and
    // leaves the clone and its record behind for the 127 to be discovered after.
    let world = World::with(&["--no-devpod"]);
    let run = world.dl(&["blooop/devlaunch@main"]);
    run.exited(127);
    assert_eq!(run.err, MISSING_DEVPOD);
    let clone = world
        .root
        .join("cache/devlaunch/repos/blooop/devlaunch")
        .join(MAIN);
    assert!(
        !clone.exists(),
        "a host with no devpod built the workspace clone at {}",
        clone.display()
    );
}

#[test]
fn a_missing_devpod_stops_a_default_branch_launch_before_it_clones_anything() {
    // The same, for the shape that has to name the default branch first: the bare
    // cache is read (offline, off the fixture's clone) and then the probe ends it.
    let world = World::with(&["--no-devpod"]);
    let run = world.dl(&["blooop/devlaunch"]);
    run.exited(127);
    assert_eq!(run.err, MISSING_DEVPOD);
}

// ===========================================================================
// the host's GitHub login
// ===========================================================================

#[test]
fn the_hosts_token_reaches_up_as_a_private_file_and_a_session_as_a_name() {
    // Only the *name* of the variable is ever in argv: the value travels in
    // devpod's environment for the session, and in a 0600 file for the `up`, so
    // `ps` never shows a credential to another user on the host.
    let world = World::with(&["--stopped", "--gh"]);
    let run = world.dl(&[MAIN, "up"]);
    run.exited(0);
    let calls = world.calls().exact(&world.root);
    let up = calls
        .iter()
        .find(|call| call.starts_with(&format!("devpod up {MAIN}")))
        .expect("an up");
    let (_, staged) = up
        .split_once("--workspace-env-file ")
        .expect("the token file flag, last in the argv as Python appends it");
    assert!(
        staged.starts_with("/tmp/devlaunch-gh-") && staged.ends_with(".env"),
        "expected a private token file, got {staged:?}"
    );
    assert!(
        !up.contains("gho_"),
        "the token itself reached argv: {up:?}"
    );
    // The file is removed when the launch that staged it ends.
    assert!(
        !Path::new(staged).exists(),
        "the staged token outlived the launch"
    );

    let warm = World::with(&["--warm", "--gh"]);
    let attach = warm.dl(&[MAIN]);
    attach.exited(0);
    assert_eq!(
        warm.calls().exact(&warm.root),
        [
            format!("devpod status {MAIN} --output json"),
            // Attaching skips `devpod up` and its workspace env entirely, so the
            // login has to be offered here too.
            format!("devpod ssh {MAIN} --send-env GH_TOKEN"),
        ]
    );
}

#[test]
fn a_host_with_no_gh_forwards_nothing_and_says_nothing_about_it() {
    // An absent `gh` is a choice, not a failure: no flags, and no warning either.
    let world = World::with(&["--warm"]);
    let run = world.dl(&[MAIN]);
    run.exited(0);
    assert!(!run.err.contains("GitHub login"), "{}", run.err);
    assert_eq!(
        world.calls().exact(&world.root).last(),
        Some(&format!("devpod ssh {MAIN}"))
    );
}

// ===========================================================================
// the completion refresh every launch verb forces
// ===========================================================================

#[test]
fn a_launch_forces_the_completion_refresh_once_it_is_finished() {
    // dl.py 4803/4822/4837/4876/4900: the cache is wrong however recently it was
    // written, because the workspace list just changed. Observed by the file the
    // detached child writes, which is the only thing a child nobody waits on can be
    // observed by.
    let world = World::with(&["--stopped"]);
    world.dl(&[MAIN, "up"]).exited(0);
    assert!(
        world.appears("cache/devlaunch/completions.json"),
        "no refresh child wrote a cache"
    );
}

#[test]
fn a_refused_up_still_warms_the_cache_where_a_refused_attach_does_not() {
    // Python's order, which is the whole of this: for `up` and `code` it asks for
    // the refresh *before* it reads the return code (dl.py 4803/4822), so a `devpod
    // up` that failed still warms the cache; every other verb returns on the failure
    // first. Reproduced rather than tidied, because the observable difference is a
    // background child.
    let world = World::with(&["--stopped", "--fail-up"]);
    world.dl(&[MAIN, "up"]).exited(7);
    assert!(
        world.appears("cache/devlaunch/completions.json"),
        "a refused up asked for no refresh"
    );
}

#[test]
fn a_refused_launch_asks_for_no_refresh_because_nothing_changed() {
    // The unknown-workspace refusal never got as far as devpod acting, so there is
    // nothing for a cache to be stale about — and this is what lets the test above
    // assert an exact call log.
    let world = World::with(&["--warm"]);
    world.dl(&["nope"]).exited(1);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !world.path("cache/devlaunch/completions.json").exists(),
        "a refusal spawned a refresh"
    );
}

// ===========================================================================
// timing
// ===========================================================================

#[test]
fn the_prose_timing_summary_names_a_launchs_round_trips() {
    // `test_timing.py`'s prose mode at the boundary. The labels are Python's
    // `" ".join(cmd[:2])` for every devpod call, and the vocabulary is read from
    // outside this repo (a trend job that decomposes a launch), so a name here is
    // renamed only deliberately.
    let world = World::with(&["--warm"]);
    let run = world.dl_with(&[MAIN], &[("DEVLAUNCH_TIMING", "1")]);
    run.exited(0);
    let timed: Vec<&str> = run
        .stderr_lines()
        .into_iter()
        .filter(|line| line.starts_with("dl-timing: "))
        .map(|line| {
            &line[.."dl-timing: ".len()
                + line["dl-timing: ".len()..]
                    .find(char::is_numeric)
                    .expect("a duration")]
        })
        .collect();
    assert_eq!(
        timed,
        [
            "dl-timing: devpod status ",
            "dl-timing: devpod ssh ",
            "dl-timing: total "
        ]
    );

    // And the cold half, where the stages a trend job decomposes actually have
    // something in them.
    let cold = World::with(&["--stopped"]);
    let up = cold.dl_with(&[MAIN, "up"], &[("DEVLAUNCH_TIMING", "1")]);
    up.exited(0);
    for label in [
        "devpod status ",
        "devpod context ",
        "devpod up ",
        "devpod ssh ",
    ] {
        assert!(
            up.err.contains(&format!("dl-timing: {label}")),
            "expected {label:?} in {}",
            up.err
        );
    }
}
