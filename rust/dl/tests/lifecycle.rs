//! The lifecycle commands, judged at the binary boundary.
//!
//! Every expectation in this file was captured by running the frozen Python build
//! — `python -m devlaunch.dl` — against `tests/lifecycle_scenario.py`'s world with
//! `test/fixtures/devpod_shim.py` on PATH as `devpod`, under a scratch
//! `HOME`/`XDG_CACHE_HOME`/`XDG_CONFIG_HOME`/`DEVPOD_HOME`, and pasting what it
//! printed. Nothing here was read off the Rust implementation. Where a golden could
//! not come from Python — the `--stop`/`--rm` flag spellings, which Python parsed
//! as a workspace name — the divergence row is cited beside it.
//!
//! Python's diagnostics go to stderr as the bare message: `dl.py` configures
//! `logging.basicConfig(level=logging.INFO, format="%(message)s")`, so `info`,
//! `warning` and `error` all arrive with no level, no logger name and no prefix.
//! That is why the refusals below are compared as plain lines.
//!
//! The one thing not compared byte for byte is a *size*: what a clone costs on
//! disk depends on whether `git clone` could hardlink its objects, which is a
//! property of the filesystem and not of dl. Every golden that carries one goes
//! through [`without_sizes`].

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use devlaunch_test_support::KeepingCoverage;

/// The repository root, from the crate this test is compiled into.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the repository root")
}

/// One scratch world, and the `dl` runs against it.
///
/// The sibling of `read_side.rs`'s `World`, kept separate because that one builds
/// the read side's fixture and its `--ls` goldens are measured from it: a world
/// that grew a workspace for `--prune` to remove would move them.
struct World {
    root: PathBuf,
    /// Kept so the directory outlives every run in the test.
    _scratch: tempfile::TempDir,
}

impl World {
    /// The base world, plus whatever fixtures this test needs.
    fn with(fixtures: &[&str]) -> Self {
        let scratch = scratch_dir();
        let root = scratch.path().to_path_buf();
        let built = Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lifecycle_scenario.py"))
            .arg(&root)
            .arg(repo_root().join("test/fixtures/devpod_shim.py"))
            .args(fixtures)
            .output()
            .expect("python3 is installed");
        assert!(
            built.status.success(),
            "lifecycle_scenario.py failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        World {
            root,
            _scratch: scratch,
        }
    }

    fn base() -> Self {
        Self::with(&[])
    }

    /// Run `dl` with nothing on stdin.
    fn dl(&self, args: &[&str]) -> Run {
        self.answering("", args)
    }

    /// Run `dl` with `stdin` for the `[y/N]` question to read.
    fn answering(&self, stdin: &str, args: &[&str]) -> Run {
        use std::io::Write as _;
        use std::process::Stdio;

        let root = self.root.display().to_string();
        let mut child = Command::new(env!("CARGO_BIN_EXE_dl"))
            .args(args)
            .env_clear()
            .keeping_coverage()
            // /usr/bin and /bin for git, which these commands really run; the fake
            // devpod is first, under its real name.
            .env("PATH", format!("{root}/bin:/usr/bin:/bin"))
            .env("HOME", format!("{root}/home"))
            .env("XDG_CACHE_HOME", format!("{root}/cache"))
            .env("XDG_CONFIG_HOME", format!("{root}/config"))
            .env("DEVPOD_HOME", format!("{root}/devpod"))
            .env("DEVPOD_SHIM_STATE", format!("{root}/shim-state.json"))
            .env("DEVPOD_SHIM_LOG", format!("{root}/shim-log.jsonl"))
            .env("DEVPOD_SHIM_CONFIG", format!("{root}/shim-config.json"))
            // No network in a test: `git ls-remote` over ssh fails at once.
            .env("GIT_SSH_COMMAND", "false")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the dl binary runs");
        child
            .stdin
            .take()
            .expect("a pipe")
            .write_all(stdin.as_bytes())
            .expect("stdin is writable");
        let output = child.wait_with_output().expect("dl finishes");
        Run::of(&output, &self.root)
    }

    /// Make the fake devpod answer a call from the response table instead of from
    /// its state machine — the failure-injection channel.
    fn devpod_answers(&self, prefix: &[&str], code: i32, stderr: &str) {
        let responses = serde_json::json!({
            "responses": [{
                "prefix": prefix,
                "returncode": code,
                "stderr": stderr,
            }]
        });
        std::fs::write(
            self.path("shim-config.json"),
            serde_json::to_string(&responses).expect("a config document"),
        )
        .expect("a shim config");
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.path(relative)).unwrap_or_default()
    }

    fn exists(&self, relative: &str) -> bool {
        self.path(relative).exists()
    }

    /// The whole cache, contents included, as a listing two runs can be compared by.
    ///
    /// The instrument for a command that must leave the cache **alone** — one
    /// answered `no`, or refused before it acted. `exists()` on the one path the
    /// test happens to think of cannot see the record rewritten somewhere else,
    /// which is the shape most of these defects have. See
    /// `devlaunch_test_support::cache_fingerprint`.
    fn cache_fingerprint(&self) -> Vec<String> {
        devlaunch_test_support::cache_fingerprint(&self.root)
    }

    /// Every path the cache holds, without contents.
    ///
    /// The instrument for a command that must leave a particular shape behind.
    fn cache_shape(&self) -> Vec<String> {
        devlaunch_test_support::cache_shape(&self.root)
    }

    /// The docker calls made so far, in order, as the argv tail of each.
    ///
    /// Empty where nothing ran docker at all, which is what the fake writes no log
    /// for — and what every world but `--devcontainer-volumes` produces, because
    /// devpod recorded no create result for their workspaces to name volumes from.
    fn docker_calls(&self) -> Vec<String> {
        self.read("docker-log").lines().map(str::to_owned).collect()
    }

    /// The devpod calls made so far, in order, as `devpod <argv>` lines.
    fn devpod_calls(&self) -> Vec<String> {
        self.read("shim-log.jsonl")
            .lines()
            .map(|line| {
                let call: serde_json::Value = serde_json::from_str(line).expect("a log line");
                let argv: Vec<String> = call["argv"]
                    .as_array()
                    .expect("an argv")
                    .iter()
                    .map(|word| word.as_str().expect("a word").to_owned())
                    .collect();
                format!("devpod {}", argv.join(" "))
            })
            .collect()
    }

    /// Wait for `relative` to appear, for as long as a detached child could
    /// reasonably take. Answers whether it did.
    ///
    /// The one thing a *detached* child can be observed by from out here: nothing
    /// waits on it, so its arrival is the assertion. Polling rather than sleeping
    /// so a machine that is quick pays nothing.
    fn appears(&self, relative: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if self.exists(relative) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }
}

/// A scratch directory whose path is always the same *length*.
///
/// `read_side.rs` needs this because the `--ls` table's column widths are measured
/// from the paths in it. Nothing here measures a column, and it is the same
/// directory shape all the same: the golden-capture harness makes `/tmp/dltXXXXXX`
/// too, and a path length that varies between capture and comparison is one more
/// thing to have to think about.
fn scratch_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("dlt")
        .rand_bytes(6)
        .tempdir_in("/tmp")
        .expect("a scratch directory under /tmp")
}

/// What one run printed and how it ended, with the scratch root templated out so
/// the expectations are the same on every machine.
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

/// The entries `listing` has and `other` does not, in `listing`'s order.
///
/// Two calls describe a cleanup completely: `only_in(before, after)` is what it
/// removed and `only_in(after, before)` is what it added, and asserting both pins
/// the whole of `after` — relative to the world the fixture built, which is the
/// only thing a test here is ever really saying.
///
/// Stated as a difference rather than as a whole listing on purpose. The listing
/// is the stronger assertion of the two and the weaker test: a fixture that grew
/// one directory would move every line of it, so it would be re-pasted rather than
/// read, and a re-pasted golden asserts whatever the binary last did. A difference
/// of one or two lines is the sentence the command printed, checked against the
/// disk, and it stays readable when the fixture moves.
fn only_in(listing: &[String], other: &[String]) -> Vec<String> {
    listing
        .iter()
        .filter(|entry| !other.contains(entry))
        .cloned()
        .collect()
}

/// A `Vec<String>` comparison against nothing, spelled so the type is inferable.
const NOTHING: [&str; 0] = [];

/// Every human-readable size stood down, so a document can be compared without its
/// filesystem-dependent half.
fn without_sizes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(|c: char| c.is_ascii_digit()) {
        let (before, from_digit) = rest.split_at(at);
        let number: String = from_digit
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let after = &from_digit[number.len()..];
        let unit = ["B", "KiB", "MiB", "GiB", "TiB"]
            .into_iter()
            .find(|unit| after.starts_with(&format!(" {unit}")));
        match unit {
            Some(unit) => {
                out.push_str(before);
                out.push_str("<size>");
                rest = &after[unit.len() + 1..];
            }
            None => {
                out.push_str(before);
                out.push_str(&number);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

// ===========================================================================
// dl <ws> stop
// ===========================================================================

#[test]
fn a_stop_addresses_the_workspace_and_says_nothing() {
    let world = World::base();
    let run = world.dl(&["someones-project", "stop"]);
    run.exited(0);
    assert_eq!(run.out, "");
    assert_eq!(run.err, "");
    assert_eq!(
        world.devpod_calls(),
        [
            "devpod status someones-project --output json",
            "devpod stop someones-project",
        ]
    );
}

#[test]
fn a_stop_reaches_the_workspace_the_record_names() {
    // devlaunch#88, and `test_stored_workspace_id.py`'s
    // TestTheSubcommandsAddressWhatWasResolved at the boundary: the record holds a
    // devpod workspace id this build does not derive, devpod has only that one, and
    // the stop has to reach it. The derived id is in the line because the two are
    // what a person needs to see; both builds derive it identically.
    let world = World::base();
    let run = world.dl(&["blooop/devlaunch@main", "stop"]);
    run.exited(0);
    assert_eq!(run.out, "");
    assert_eq!(
        run.err,
        "Addressing devpod workspace 'devlaunch-main-legacy' from the record for \
         blooop/devlaunch@main; this build derives 'devlaunch-main-3j1t'\n"
    );
    assert_eq!(
        world.devpod_calls(),
        [
            "devpod status devlaunch-main-3j1t --output json",
            "devpod status devlaunch-main-legacy --output json",
            "devpod stop devlaunch-main-legacy",
        ],
        "the derived id is asked first and the record is read only after it is denied"
    );
}

#[test]
fn a_devpod_that_will_not_stop_hands_its_own_status_back() {
    let world = World::base();
    world.devpod_answers(&["stop"], 7, "devpod: provider is gone\n");
    let run = world.dl(&["someones-project", "stop"]);
    run.exited(7);
    // devpod's own diagnostics are already on this process's stderr: the call
    // inherits the streams, so dl has nothing to add.
    assert_eq!(run.err, "devpod: provider is gone\n");
    assert_eq!(run.out, "");
}

#[test]
fn a_target_nothing_answers_to_is_refused_before_devpod_is_asked_to_act() {
    let world = World::base();
    let run = world.dl(&["nope", "stop"]);
    run.exited(1);
    assert_eq!(
        run.err,
        "Unknown workspace 'nope'. Use 'dl --ls' to list workspaces, or specify owner/repo or \
         ./path\n"
    );
    assert_eq!(
        world.devpod_calls(),
        [
            "devpod status nope --output json",
            "devpod list --output json",
        ],
        "the listing gets the final word: a workspace whose provider is broken \
         still lists and cannot be described"
    );
}

#[test]
fn a_config_choice_a_stop_cannot_honour_is_said_rather_than_discarded() {
    let world = World::base();
    let run = world.dl(&["--devcontainer", "robot", "someones-project", "stop"]);
    run.exited(0);
    assert_eq!(
        run.err,
        "Ignoring --devcontainer: it does not apply to 'stop'.\n"
    );
}

#[test]
fn the_retired_flag_spellings_name_the_words_that_replaced_them() {
    // Divergence rows 15 and 30 gave `--stop` and `--rm` a verb-first grammar and
    // then made them appendable; row 32 takes both spellings back, because `--rm`
    // now means "delete the workspace when the session ends" and a look-alike flag
    // that cancels the line instead is the one thing that pair must not be. Refused
    // at exit 1 with the word to type — never clap's exit 2, which would name the
    // spelling and not the replacement.
    let world = World::base();
    let stopping = world.dl(&["--stop", "someones-project"]);
    stopping.exited(1);
    assert!(
        stopping.err.contains("dl <workspace> stop"),
        "{}",
        stopping.err
    );
    assert!(world.devpod_calls().is_empty(), "a refusal touched devpod");

    // And the words still do the work, from either position.
    let stopped = World::base();
    stopped.dl(&["stop", "someones-project"]).exited(0);
    assert_eq!(
        stopped.devpod_calls().last().expect("a call"),
        "devpod stop someones-project"
    );

    let removing = World::base();
    removing.dl(&["someones-project", "rm"]).exited(0);
    assert_eq!(
        removing.devpod_calls().last().expect("a call"),
        "devpod delete someones-project"
    );
}

#[test]
fn autorm_is_refused_with_the_spelling_that_replaced_it() {
    // The one that matters most for a line recalled from history: the behaviour is
    // unchanged, only the name moved, so a silent no-op would be the worst answer
    // and a refusal naming `--rm` is the whole of what is owed.
    let world = World::base();
    let run = world.dl(&["someones-project", "--autorm"]);
    run.exited(1);
    assert!(
        run.err.contains("--autorm is now spelled --rm"),
        "{}",
        run.err
    );
    assert!(world.devpod_calls().is_empty(), "a refusal touched devpod");
}

// ===========================================================================
// dl <ws> rm
// ===========================================================================

#[test]
fn a_clean_clone_is_deleted_with_its_workspace() {
    let world = World::base();
    let clone = "cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-legacy";
    assert!(world.exists(clone), "the fixture's clean clone");
    let run = world.dl(&["devlaunch-main-legacy", "rm"]);
    run.exited(0);
    // devpod's own line, inherited: the delete is a passthrough call.
    assert_eq!(
        run.out,
        "Successfully deleted workspace devlaunch-main-legacy\n"
    );
    // Both lines, in Python's order: `worktree/workspace_clone.py` logs the
    // directory from inside the removal, and `dl.py` logs the workspace after it
    // returns. The first one is why the notice channel is a sink — a storage flow's
    // line has to land where the storage flow said it.
    assert_eq!(
        run.err,
        "Removed workspace clone: {ROOT}/cache/devlaunch/repos/blooop/devlaunch/\
         devlaunch-main-legacy\nRemoved local clone for devlaunch-main-legacy\n"
    );
    assert!(!world.exists(clone), "the clone was left behind");
    assert!(
        !world
            .read("cache/devlaunch/metadata.json")
            .contains("devlaunch-main-legacy"),
        "the record was left behind"
    );
    assert_eq!(
        world.devpod_calls(),
        [
            "devpod status devlaunch-main-legacy --output json",
            "devpod delete devlaunch-main-legacy",
        ]
    );
}

/// devlaunch#325 at the binary boundary: the two volumes go with the workspace,
/// and it is the *binary* that has to hand core the devpod home they are named
/// from — a `None` there would leave every core test passing and every volume on
/// disk.
#[test]
fn a_removed_workspaces_devcontainer_volumes_go_with_it() {
    let world = World::with(&["--devcontainer-volumes"]);

    world.dl(&["devlaunch-main-legacy", "rm"]).exited(0);

    assert_eq!(
        world.docker_calls(),
        ["volume rm --force devlaunch-main-legacy-pixi dind-var-lib-docker-0f4b2c1d"]
    );
}

/// The purge is a second wiring site — it issues its own `devpod delete --force`
/// and never calls the delete flow — so it is asserted separately rather than
/// assumed to inherit.
#[test]
fn a_purge_takes_the_devcontainer_volumes_of_the_workspaces_it_deleted() {
    let world = World::with(&["--devcontainer-volumes"]);

    world.dl(&["--purge", "-y"]).exited(0);

    // One call, for the one workspace devpod had recorded a create result for. The
    // other workspace devlaunch made named nothing, and the foreign workspace was
    // never deleted.
    assert_eq!(
        world.docker_calls(),
        ["volume rm --force devlaunch-main-legacy-pixi dind-var-lib-docker-0f4b2c1d"]
    );
}

/// A workspace devpod never finished creating has no create result, so there is
/// nothing to name and docker is not run at all — not run with a guessed name in
/// it, which would be somebody else's disk.
#[test]
fn a_delete_with_nothing_recorded_to_name_runs_no_docker() {
    let world = World::base();

    world.dl(&["devlaunch-main-legacy", "rm"]).exited(0);

    assert_eq!(world.docker_calls(), Vec::<String>::new());
}

#[test]
fn a_clone_holding_work_that_is_nowhere_else_is_refused() {
    let world = World::base();
    let clone = "cache/devlaunch/repos/blooop/devlaunch/devlaunch-dirty-fqta";
    let run = world.dl(&["devlaunch-dirty-fqta", "rm"]);
    run.exited(1);
    assert_eq!(
        run.err,
        "devlaunch-dirty-fqta holds 1 uncommitted change(s) (scratch.txt). Push or commit it, \
         or run: dl devlaunch-dirty-fqta rm --force\n"
    );
    assert_eq!(run.out, "");
    assert!(world.exists(clone), "the refusal deleted the clone anyway");
    assert_eq!(
        world.devpod_calls(),
        ["devpod status devlaunch-dirty-fqta --output json"],
        "nothing was asked of devpod but which workspace this is"
    );
}

#[test]
fn a_clone_git_cannot_be_asked_about_is_refused_for_not_knowing() {
    // devlaunch#171's third answer: the files are still on disk and nothing has
    // established that they exist anywhere else, which is the same standing as
    // unpushed work and gets the same refusal and the same way past it.
    let world = World::with(&["--not-a-clone"]);
    let run = world.dl(&["devlaunch-opaque-nogit", "rm"]);
    run.exited(1);
    assert_eq!(
        run.err,
        format!(
            "devlaunch-opaque-nogit: git could not read {clone}: fatal: not a git repository: \
             '{clone}/.git'. devlaunch will not delete a clone it cannot check. Look at it, or \
             run: dl devlaunch-opaque-nogit rm --force\n",
            clone = "{ROOT}/cache/devlaunch/repos/blooop/devlaunch/devlaunch-opaque-nogit"
        )
    );
}

#[test]
fn a_clone_holding_a_commit_no_remote_has_is_refused_and_named_as_that() {
    // `test_workspace_state.py`'s guard texts: uncommitted work and unpushed commits
    // are different losses, and the refusal says which — a user who reads
    // "uncommitted" for a committed change would go looking in the wrong place.
    let world = World::with(&["--unpushed"]);
    let run = world.dl(&["devlaunch-unpushed-committed", "rm"]);
    run.exited(1);
    assert_eq!(
        run.err,
        "devlaunch-unpushed-committed holds 1 unpushed commit(s). Push or commit it, or run: dl \
         devlaunch-unpushed-committed rm --force\n"
    );
}

#[test]
fn force_deletes_despite_the_work_and_asks_devpod_to_ignore_an_absence() {
    let world = World::with(&["--not-a-clone"]);
    let run = world.dl(&["devlaunch-dirty-fqta", "rm", "--force"]);
    run.exited(0);
    assert_eq!(
        run.out,
        "Successfully deleted workspace devlaunch-dirty-fqta\n"
    );
    assert_eq!(
        world.devpod_calls(),
        [
            "devpod status devlaunch-dirty-fqta --output json",
            // devpod's own --ignore-not-found: a forced remove is "ensure absent",
            // the way `rm -f` is.
            "devpod delete devlaunch-dirty-fqta --ignore-not-found",
        ]
    );
    assert!(
        !world.exists("cache/devlaunch/repos/blooop/devlaunch/devlaunch-dirty-fqta"),
        "the clone survived a forced delete"
    );
}

#[test]
fn a_delete_devpod_refuses_keeps_the_clone_and_hands_the_status_back() {
    let world = World::base();
    world.devpod_answers(&["delete"], 3, "devpod: cannot read devcontainer.json\n");
    let run = world.dl(&["devlaunch-main-legacy", "rm"]);
    run.exited(3);
    assert_eq!(
        run.err,
        "devpod: cannot read devcontainer.json\ndevpod could not delete devlaunch-main-legacy; \
         keeping the local clone so it stays retryable. If its devcontainer.json moved, restore \
         the path or run: devpod delete devlaunch-main-legacy --force\n"
    );
    assert!(
        world.exists("cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-legacy"),
        "the clone was removed even though the workspace is still there"
    );
}

// ===========================================================================
// the background refresh
// ===========================================================================

#[test]
fn a_command_that_changed_the_workspace_list_refreshes_the_completions_behind_it() {
    // `test_dl.py`'s refresh classes, spawn half: the child is this build, run as
    // `<program> --update-cache --force`, detached — so what can be observed from
    // out here is that a completion cache appears without this process waiting for
    // one, and that the child asked devpod for the list.
    let world = World::base();
    assert!(!world.exists("cache/devlaunch/completions.json"));
    world.dl(&["someones-project", "stop"]).exited(0);
    assert!(
        world.appears("cache/devlaunch/completions.json"),
        "no detached refresh wrote a completion cache"
    );
    assert!(
        world
            .devpod_calls()
            .iter()
            .filter(|call| call.starts_with("devpod list"))
            .count()
            >= 1,
        "the refresh child never asked devpod anything: {:?}",
        world.devpod_calls()
    );
}

#[test]
fn a_read_only_command_warms_the_cache_on_the_way_in() {
    // The other half of the same latch: `--ls` reads the cache and leaves the
    // workspace list alone, so it warms it before running rather than after.
    let world = World::base();
    world.dl(&["--ls"]).exited(0);
    assert!(
        world.appears("cache/devlaunch/completions.json"),
        "no startup refresh wrote a completion cache"
    );
}

// ===========================================================================
// --update-cache: the child itself
// ===========================================================================

#[test]
fn the_refresh_child_writes_the_cache_and_sweeps_the_bare_clones() {
    let world = World::base();
    let run = world.dl(&["--update-cache", "--force"]);
    run.exited(0);
    // Nothing on stdout: the arms this command reports of its own are Python's
    // `logging.debug`. The sweep's two `logger.info` lines are on stderr, byte for
    // byte as Python printed them — which nobody sees in the child that matters,
    // since a detached refresh has both streams on /dev/null.
    assert_eq!(run.out, "");
    assert_eq!(
        run.err,
        "Fetching updates for blooop/devlaunch\nSuccessfully fetched updates for \
         blooop/devlaunch\n"
    );
    assert!(
        world
            .read("cache/devlaunch/completions.json")
            .contains("blooop/devlaunch"),
        "the completion cache was not written"
    );
    let fetched = world.read("cache/devlaunch/metadata.json");
    assert!(
        !fetched.contains("2020-01-01T00:00:00"),
        "the fetch sweep did not run: last_fetched is still the fixture's, {fetched}"
    );
}

#[test]
fn the_child_migrates_the_cache_like_every_other_run() {
    // `test_updater_fetch_sweep.py`'s TestTheChildMigratesLikeEveryOtherRun: a
    // detached child is the worst place to skip the one-shot id-scheme migration,
    // because nobody is watching it write records in a shape the rest of dl no
    // longer reads. It reaches metadata through the same construction point every
    // other command does, which is where the migration runs.
    let world = World::base();
    let v1 = world
        .read("cache/devlaunch/metadata.json")
        .replace("\"version\": 3", "\"version\": 1");
    std::fs::write(world.path("cache/devlaunch/metadata.json"), &v1).expect("a v1 document");
    world.dl(&["--update-cache", "--force"]).exited(0);
    assert!(
        world
            .read("cache/devlaunch/metadata.json")
            .contains("\"version\": 3"),
        "the refresh child did not migrate the cache"
    );
}

#[test]
fn a_fresh_cache_stops_the_child_before_it_sweeps() {
    // The TTL is re-checked in the child as well as in the parent that spawned it:
    // two parents can both see a stale cache before either child has written one,
    // and the second sweep would be pure waste.
    let world = World::base();
    world.dl(&["--update-cache", "--force"]).exited(0);
    let swept = world.read("cache/devlaunch/metadata.json");
    world.dl(&["--update-cache"]).exited(0);
    assert_eq!(
        world.read("cache/devlaunch/metadata.json"),
        swept,
        "the second child swept a cache that was still fresh"
    );
}

// ===========================================================================
// --purge
// ===========================================================================

/// Python's `dl --purge` plan for the base world, verbatim.
const PURGE_PLAN: &str = "\
This will remove all devlaunch data:
  - 2 DevPod workspace(s)
  - {ROOT}/cache/devlaunch/ (workspace clones, repo caches, the shared pixi cache, completions)

Leaving 1 workspace(s) devlaunch did not create:
  - someones-project

";

/// The sentence both cleanups end on.
///
/// **A deliberate divergence from Python, and the one line in this file that is
/// not a golden** (devlaunch#325). Python's sentence disclaimed volumes as well as
/// images, and it was true of every build up to this one: nothing devlaunch ran
/// removed a volume. Now the named volumes a workspace's devcontainer created go
/// with the workspace, so the disclaimer is narrowed to what is still true. Images
/// remain outside on purpose — shared, expensive to rebuild, ownership genuinely
/// ambiguous — which is why the sentence still exists at all.
const DOCKER_BOUNDARY: &str = "devlaunch does not manage Docker images: the images these \
                               workspaces built may still hold disk, and `docker system df` \
                               shows what Docker is holding.\n";

#[test]
fn a_purge_answered_no_removes_nothing_and_still_names_the_disk_it_does_not_free() {
    let world = World::with(&["--prunable"]);
    let before = world.cache_fingerprint();
    let run = world.answering("n\n", &["--purge"]);
    run.exited(0);
    assert_eq!(
        run.out,
        format!("{PURGE_PLAN}Are you sure? [y/N] Aborted.\n{DOCKER_BOUNDARY}")
    );
    assert_eq!(run.err, "");
    // The whole cache, not the one file this test used to name. `--fingerprint`
    // in the retired `compare.py` is what covered this: an abort has to leave
    // every path and every byte where it found them, and a stdout comparison
    // cannot see a record rewritten under a path nobody thought to check.
    assert_eq!(
        world.cache_fingerprint(),
        before,
        "an abort moved something on disk"
    );
    assert_eq!(
        world.devpod_calls(),
        ["devpod list --output json"],
        "an abort asked devpod to delete something"
    );
}

#[test]
fn an_answer_that_is_not_yes_is_no() {
    // The empty one is a closed stdin, and it is the one answer that is not
    // Python's: `input()` raised `EOFError` there, which reached the top and
    // printed a traceback, exit 1. **Divergence candidate** (report's (d)): a
    // question nobody can answer is answered no, which is the reading the rest of
    // this table already gives every other unrecognised answer.
    for answer in ["", "\n", "no\n", "Y es\n", "q\n"] {
        let world = World::base();
        let run = world.answering(answer, &["--purge"]);
        run.exited(0);
        assert!(
            run.out.ends_with(&format!("Aborted.\n{DOCKER_BOUNDARY}")),
            "{answer:?} was read as yes"
        );
    }
    // And the two spellings that are yes.
    for answer in ["y\n", "YES\n"] {
        let world = World::base();
        let run = world.answering(answer, &["--purge"]);
        run.exited(0);
        assert!(
            run.out.contains("Deleting DevPod workspace: "),
            "{answer:?} was read as no"
        );
    }
}

#[test]
fn a_purge_deletes_the_workspaces_devlaunch_made_and_its_cache() {
    let world = World::base();
    let run = world.dl(&["--purge", "-y"]);
    run.exited(0);
    assert_eq!(
        run.out,
        format!(
            "{PURGE_PLAN}Deleting DevPod workspace: devlaunch-main-legacy\n\
             Deleting DevPod workspace: devlaunch-dirty-fqta\n\
             Removed: {{ROOT}}/cache/devlaunch\n{DOCKER_BOUNDARY}"
        )
    );
    assert_eq!(run.err, "");
    // Two claims in one line, and the second is the one an `exists()` check on
    // `cache/devlaunch` cannot make. Everything devlaunch put under
    // `XDG_CACHE_HOME` is gone -- a lock, a stale record, a clone it could not
    // walk, anywhere in the tree, would appear here. And `XDG_CACHE_HOME` *itself*
    // is still standing: this command deletes devlaunch's cache directory, not the
    // user's cache, and the difference between those two is somebody's whole
    // `~/.cache`.
    assert_eq!(
        world.cache_shape(),
        ["cache/"],
        "the purge left something of its own behind, or took the cache root with it"
    );
    assert_eq!(
        world.devpod_calls(),
        [
            "devpod list --output json",
            "devpod delete devlaunch-main-legacy --force",
            "devpod delete devlaunch-dirty-fqta --force",
        ],
        "the foreign workspace was deleted, or the ownership scope was wider than \
         the plan the user answered"
    );
}

#[test]
fn a_purge_that_could_not_remove_everything_says_which_paths_refused() {
    // `test_purge_partial_removal.py`'s TestTheReportIsActionable and
    // TestThePurgeSaysWhichOfTheThreeHappened, at the boundary: the headline says
    // *which* of the three endings this was, the paths and their reasons are what
    // somebody acts on, and the `sudo rm -rf` line is quoted.
    if unsafe { libc::geteuid() } == 0 {
        // root can unlink anything, so there is no refusal to report.
        return;
    }
    let world = World::with(&["--unwritable"]);
    let run = world.dl(&["--purge", "-y"]);
    run.exited(1);
    assert_eq!(
        run.out,
        format!(
            "{PURGE_PLAN}Deleting DevPod workspace: devlaunch-main-legacy\n\
             Deleting DevPod workspace: devlaunch-dirty-fqta\n\
             Removed what was permitted under {{ROOT}}/cache/devlaunch. These refused:\n  \
             - {{ROOT}}/cache/devlaunch/repos/blooop/devlaunch/devlaunch-gone-locked/held: \
             Permission denied\n\n\
             Usually this means a container wrote them as a different user, and:\n  \
             sudo rm -rf {{ROOT}}/cache/devlaunch\n\
             clears them. Check the reasons above first -- it does not fix all of them.\n\
             {DOCKER_BOUNDARY}"
        )
    );
}

#[test]
fn a_purge_that_removed_not_one_path_says_that_rather_than_the_other_sentence() {
    // devlaunch#182, the other half of the pair: "one clone stayed behind" and "not a
    // byte of it moved" used to arrive at the caller as the same value, and the
    // second printed the first's sentence. A symlinked cache root is the second —
    // devlaunch#131 refuses to follow it, and refuses to unlink it, because
    // following deletes a tree nobody pointed at and unlinking reports a clone as
    // reclaimed while it sits on another volume.
    let world = World::with(&["--symlinked-cache"]);
    let run = world.dl(&["--purge", "-y"]);
    run.exited(1);
    assert_eq!(
        run.out,
        format!(
            "{PURGE_PLAN}Deleting DevPod workspace: devlaunch-main-legacy\n\
             Deleting DevPod workspace: devlaunch-dirty-fqta\n\
             Removed nothing under {{ROOT}}/cache/devlaunch. These refused:\n  \
             - {{ROOT}}/cache/devlaunch: is a symbolic link to {{ROOT}}/elsewhere/devlaunch, \
             which a purge will not follow\n\n\
             Usually this means a container wrote them as a different user, and:\n  \
             sudo rm -rf {{ROOT}}/cache/devlaunch\n\
             clears them. Check the reasons above first -- it does not fix all of them.\n\
             {DOCKER_BOUNDARY}"
        )
    );
    assert!(
        world.exists("elsewhere/devlaunch/metadata.json"),
        "the purge followed the link"
    );
}

#[test]
fn a_purge_of_a_machine_with_nothing_on_it_says_so() {
    let world = World::with(&["--no-cache", "--no-workspaces"]);
    let run = world.dl(&["--purge", "-y"]);
    run.exited(0);
    assert_eq!(
        run.out,
        format!(
            "This will remove all devlaunch data:\n  - 0 DevPod workspace(s)\n  \
             - {{ROOT}}/cache/devlaunch/ (workspace clones, repo caches, the shared pixi cache, \
             completions)\n\nNo data to purge.\n{DOCKER_BOUNDARY}"
        )
    );
}

#[test]
fn a_purge_that_deleted_workspaces_and_found_no_cache_says_nothing_about_the_cache() {
    // A purge that deleted two workspaces has not done nothing, so it does not say
    // "No data to purge." — and there was no cache directory to report removing
    // either, so it says nothing at all about it. Python reaches the same exit code
    // through a branch that prints neither sentence.
    let world = World::with(&["--no-cache"]);
    let run = world.dl(&["--purge", "-y"]);
    run.exited(0);
    assert_eq!(
        run.out,
        format!(
            "{PURGE_PLAN}Deleting DevPod workspace: devlaunch-main-legacy\n\
             Deleting DevPod workspace: devlaunch-dirty-fqta\n{DOCKER_BOUNDARY}"
        )
    );
}

#[test]
fn a_purge_that_cannot_read_the_workspace_list_refuses_rather_than_purging_nothing() {
    // `test_workspace_listing.py`'s purge-will-not-act half: a purge that quietly
    // did nothing used to look exactly like a purge that had nothing to do.
    let world = World::base();
    world.devpod_answers(&["list"], 1, "context not found: default\n");
    let run = world.dl(&["--purge", "-y"]);
    run.exited(1);
    assert_eq!(run.out, "");
    assert_eq!(
        run.err,
        "error: `devpod list` exited 1: 'context not found: default'\n"
    );
    assert!(world.exists("cache/devlaunch"), "it purged anyway");
}

// ===========================================================================
// --prune
// ===========================================================================

#[test]
fn a_prune_with_nothing_to_remove_says_why_each_directory_is_staying() {
    let world = World::base();
    let run = world.dl(&["--prune"]);
    run.exited(0);
    assert_eq!(
        run.out,
        format!(
            "Clone directories under {{ROOT}}/cache/devlaunch/repos:\n\n\
             Leaving 2:\n  \
             - {{ROOT}}/cache/devlaunch/repos/blooop/devlaunch/devlaunch-dirty-fqta: \
             workspace devlaunch-dirty-fqta still opens it\n  \
             - {{ROOT}}/cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-legacy: \
             workspace devlaunch-main-legacy still opens it\n\n\
             Nothing to prune.\n{DOCKER_BOUNDARY}"
        )
    );
    assert_eq!(run.err, "");
    assert_eq!(
        world.devpod_calls(),
        ["devpod list --output json"],
        "a plan with nothing in it paid for the acting pass's second listing"
    );
}

/// Python's plan for the prunable world, sizes stood down.
const PRUNE_PLAN: &str = "\
Clone directories under {ROOT}/cache/devlaunch/repos:

Removing 1 that nothing references -- <size>:
  - {ROOT}/cache/devlaunch/repos/blooop/devlaunch/devlaunch-gone-nobody (<size>)

Leaving 3:
  - {ROOT}/cache/devlaunch/repos/blooop/devlaunch/devlaunch-dirty-fqta: workspace devlaunch-dirty-fqta still opens it
  - {ROOT}/cache/devlaunch/repos/blooop/devlaunch/devlaunch-gone-dirty: holds 1 uncommitted change(s) (scratch.txt) -- add --force to remove it anyway
  - {ROOT}/cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-legacy: workspace devlaunch-main-legacy still opens it

Dropping 1 record(s) of directories already gone.

";

#[test]
fn a_prune_answered_no_removes_nothing_and_the_report_is_the_read_only_view() {
    // `test_prune_orphaned_clones.py`'s report and input classes, and
    // TestTheDiskThisCommandDoesNotFree: the plan names what is going, what is
    // staying and why, and the run ends on the boundary sentence whichever way it
    // ends.
    let world = World::with(&["--prunable", "--stale-record"]);
    let before = world.cache_fingerprint();
    let run = world.answering("no\n", &["--prune"]);
    run.exited(0);
    assert_eq!(
        without_sizes(&run.out),
        format!("{PRUNE_PLAN}Are you sure? [y/N] Aborted.\n{DOCKER_BOUNDARY}")
    );
    // The read-only claim, made about the whole cache rather than about the one
    // directory the plan offered to remove. This world also carries a *stale
    // record* the acting pass would drop, which lives in `metadata.json` and not
    // in the tree shape — so an abort that rewrote it would have passed the
    // single-`exists()` check this replaces, and fails here.
    assert_eq!(
        world.cache_fingerprint(),
        before,
        "an abort moved something on disk"
    );
    assert_eq!(
        world.devpod_calls(),
        ["devpod list --output json"],
        "an abort paid for the acting pass's second listing"
    );
}

#[test]
fn a_prune_removes_what_the_second_pass_also_finds_unreferenced() {
    let world = World::with(&["--prunable", "--stale-record"]);
    let before = world.cache_shape();
    let run = world.dl(&["--prune", "-y"]);
    run.exited(0);
    assert_eq!(
        without_sizes(&run.out),
        format!("{PRUNE_PLAN}Removed 1 clone director(ies) -- <size>.\n{DOCKER_BOUNDARY}")
    );
    assert!(!world.exists("cache/devlaunch/repos/blooop/devlaunch/devlaunch-gone-nobody"));
    assert!(
        world.exists("cache/devlaunch/repos/blooop/devlaunch/devlaunch-gone-dirty"),
        "the clone holding work went without --force"
    );
    assert!(
        world.exists("cache/devlaunch/repos/blooop/devlaunch/.bare"),
        "the bare cache every clone hardlinks out of was removed"
    );
    assert!(
        !world
            .read("cache/devlaunch/metadata.json")
            .contains("devlaunch-ancient-forgotten"),
        "the record for a directory already gone was kept"
    );
    // And the whole tree, so a prune that removed the right directory and *also*
    // something else fails here rather than passing every line above. Stated as
    // the difference from the shape it started with, because that is the claim --
    // one directory left, nothing else moved -- and a reader can check it without
    // holding the fixture in their head.
    let after = world.cache_shape();
    assert_eq!(
        only_in(&before, &after),
        [
            "cache/devlaunch/repos/blooop/devlaunch/devlaunch-gone-nobody/",
            "cache/devlaunch/repos/blooop/devlaunch/devlaunch-gone-nobody/.git/ (git store, contents omitted)",
            "cache/devlaunch/repos/blooop/devlaunch/devlaunch-gone-nobody/README.md",
        ],
        "a prune removed more, or less, than the one clone it reported"
    );
    assert_eq!(
        only_in(&after, &before),
        NOTHING,
        "a prune left something new behind"
    );
    assert_eq!(
        world.devpod_calls(),
        [
            "devpod list --output json",
            // The one question whose answer cannot be re-derived from disk, paid
            // only after a user has said yes to a deletion.
            "devpod list --output json",
        ]
    );
}

#[test]
fn force_removes_the_clone_holding_work_and_says_what_it_was() {
    let world = World::with(&["--prunable"]);
    let run = world.dl(&["--prune", "-y", "--force"]);
    run.exited(0);
    let out = without_sizes(&run.out);
    assert!(
        out.contains(
            "  - {ROOT}/cache/devlaunch/repos/blooop/devlaunch/devlaunch-gone-dirty (<size>) -- \
             holds 1 uncommitted change(s) (scratch.txt), removing anyway\n"
        ),
        "what --force is answering belongs on the line of the directory it answers \
         for: {out}"
    );
    assert!(out.contains("Removed 2 clone director(ies) -- <size>.\n"));
    assert!(!world.exists("cache/devlaunch/repos/blooop/devlaunch/devlaunch-gone-dirty"));
}

#[test]
fn a_prune_that_cannot_come_away_completely_names_the_paths_that_refused() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let world = World::with(&["--unwritable"]);
    let run = world.dl(&["--prune", "-y", "--force"]);
    run.exited(1);
    let out = without_sizes(&run.out);
    assert!(
        out.contains(
            "Removed 0 clone director(ies) -- <size>.\n\
             Some directories would not come away. These refused:\n  \
             - {ROOT}/cache/devlaunch/repos/blooop/devlaunch/devlaunch-gone-locked/held: \
             Permission denied\n\n\
             Usually this means a container wrote them as a different user, and:\n  \
             sudo rm -rf {ROOT}/cache/devlaunch/repos/blooop/devlaunch/devlaunch-gone-locked/held\n\
             clears them. Check the reasons above first -- it does not fix all of them.\n"
        ),
        // Each refusal's own path here, where a purge names the cache root: what
        // somebody has to `sudo rm -rf` is what would not come away.
        "expected the refusal report, got {out}"
    );
    assert!(out.ends_with(DOCKER_BOUNDARY));
}

#[test]
fn a_prune_stops_while_a_live_workspace_cannot_be_placed() {
    // Not a warning above a report: a workspace whose source cannot be followed
    // could be opening *any* of the candidates, so while one exists there is no
    // directory this command can honestly call unreferenced.
    let world = World::with(&["--prunable", "--unplaceable"]);
    let run = world.dl(&["--prune", "-y"]);
    run.exited(1);
    assert_eq!(
        run.out,
        format!(
            "dl --prune cannot follow these live workspaces' sources:\n  \
             - a-source-nobody-can-read: {{\"localFolder\": 42}}\n\n\
             Nothing was removed: no clone is unreferenced while a workspace is unaccounted \
             for.\n{DOCKER_BOUNDARY}"
        )
    );
    assert!(world.exists("cache/devlaunch/repos/blooop/devlaunch/devlaunch-gone-nobody"));
}

#[test]
fn a_prune_that_never_looked_at_a_directory_names_no_docker_disk() {
    // The boundary sentence belongs under a report, and a listing dl could not read
    // is not one: Python raised that refusal out of the command, past the line that
    // prints it. Every ending that *is* a report ends on it — the four tests above
    // are the four of them.
    let world = World::with(&["--prunable"]);
    world.devpod_answers(&["list"], 1, "context not found: default\n");
    let run = world.dl(&["--prune", "-y"]);
    run.exited(1);
    assert_eq!(run.out, "");
    assert_eq!(
        run.err,
        "error: `devpod list` exited 1: 'context not found: default'\n"
    );
}

#[test]
fn an_unknown_option_is_claps_usage_error_now() {
    // Divergence row 14. Python refused this itself —
    // `Unknown option(s) for --prune: --dry-run`, exit 1, and no boundary sentence
    // because it never got as far as looking at a directory. clap gets there first
    // and exits 2, which is the same refusal one layer out.
    let world = World::base();
    let run = world.dl(&["--prune", "--dry-run"]);
    run.exited(2);
    assert!(run.err.contains("--dry-run"), "{:?}", run.err);
    assert_eq!(run.out, "", "clap's usage error reached stdout");
}

// ===========================================================================
// --reconcile
// ===========================================================================

#[test]
fn a_reconcile_with_no_orphans_reports_that_and_asks_nothing() {
    let world = World::base();
    let run = world.dl(&["--reconcile"]);
    run.exited(0);
    assert_eq!(
        run.out,
        "devpod workspaces sourced under {ROOT}/cache/devlaunch/repos at something that is not a \
         clone:\n\nNothing to reconcile.\n"
    );
    assert_eq!(run.err, "");
}

/// Python's plan for the orphaned world, verbatim.
const RECONCILE_PLAN: &str = "\
devpod workspaces sourced under {ROOT}/cache/devlaunch/repos at something that is not a clone:

Re-pointing 1:
  - other-feature-x-legacy: {ROOT}/cache/devlaunch/repos/blooop/other/feature-x -> {ROOT}/cache/devlaunch/repos/blooop/other/other-feature-x-t0h1

Each of these needs `dl <workspace> recreate` afterwards: the container
still has the old source bind-mounted, and no record change moves a mount.

Leaving 1, which dl will not guess at:
  - other-nothing-here ({ROOT}/cache/devlaunch/repos/blooop/other/nothing-answers): no clone of that repository answers to this name

Nothing here is deleted. `dl <workspace> rm` is how one goes, if it should.

";

#[test]
fn a_reconcile_answered_no_changes_nothing() {
    // `test_reconcile_orphaned_workspaces.py`'s report and confirm classes: the
    // repair the migration's notice promises is stated before it is consented to,
    // and nothing here is deleted.
    let world = World::with(&["--orphan"]);
    let before =
        world.read("devpod/contexts/default/workspaces/other-feature-x-legacy/workspace.json");
    let cache_before = world.cache_fingerprint();
    let run = world.answering("n\n", &["--reconcile"]);
    run.exited(0);
    assert_eq!(
        run.out,
        format!("{RECONCILE_PLAN}Re-point these? [y/N] Aborted.\n")
    );
    assert_eq!(run.err, "");
    assert_eq!(
        world.read("devpod/contexts/default/workspaces/other-feature-x-legacy/workspace.json"),
        before,
        "devpod's record was rewritten by a run that was told not to"
    );
    // And devlaunch's own side of the join, which the line above does not cover:
    // `--reconcile` writes a stored id into devpod's record *and* reads
    // devlaunch's, so an abort has to leave both alone.
    assert_eq!(
        world.cache_fingerprint(),
        cache_before,
        "an abort moved something in devlaunch's own cache"
    );
}

#[test]
fn a_reconcile_re_points_devpods_record_and_writes_the_id_beside_it() {
    let world = World::with(&["--orphan"]);
    let run = world.dl(&["--reconcile", "-y"]);
    run.exited(0);
    assert_eq!(
        run.out,
        format!(
            "{RECONCILE_PLAN}Re-pointed other-feature-x-legacy at \
             {{ROOT}}/cache/devlaunch/repos/blooop/other/other-feature-x-t0h1\n"
        )
    );
    assert_eq!(run.err, "");

    let record: serde_json::Value = serde_json::from_str(
        &world.read("devpod/contexts/default/workspaces/other-feature-x-legacy/workspace.json"),
    )
    .expect("devpod's record is still JSON");
    assert_eq!(
        record["source"]["localFolder"].as_str().expect("a folder"),
        world
            .path("cache/devlaunch/repos/blooop/other/other-feature-x-t0h1")
            .display()
            .to_string()
    );
    assert_eq!(
        record["uid"].as_str(),
        Some("uid-other-feature-x-legacy"),
        "a key devpod knows about and dl does not was dropped"
    );
    assert!(
        world
            .read("cache/devlaunch/metadata.json")
            .contains("\"other-feature-x-legacy\""),
        "the second copy of the id — what stops this happening again — was not written"
    );
    // Nothing was deleted, and the one dl would not guess at is still there.
    assert!(
        world
            .devpod_calls()
            .iter()
            .all(|call| call.starts_with("devpod list")),
        "a reconcile asked devpod to do something: {:?}",
        world.devpod_calls()
    );
}

#[test]
fn a_reconcile_that_cannot_write_a_record_says_which_and_exits_one() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let world = World::with(&["--orphan"]);
    let held = world.path("devpod/contexts/default/workspaces/other-feature-x-legacy");
    let mut permissions = std::fs::metadata(&held)
        .expect("the record's directory")
        .permissions();
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o500);
    }
    std::fs::set_permissions(&held, permissions).expect("a read-only record directory");
    let run = world.dl(&["--reconcile", "-y"]);
    run.exited(1);
    // Divergence row 4 in the reason: Python quoted the `OSError`
    // (`[Errno 13] Permission denied: '…workspace.json.dl-tmp'`) and this quotes
    // the OS's words. The refusal, the record it names and the exit code are
    // Python's.
    assert_eq!(
        run.err,
        "Could not re-point other-feature-x-legacy: could not write \
         {ROOT}/devpod/contexts/default/workspaces/other-feature-x-legacy/workspace.json: \
         Permission denied\n"
    );
    assert!(
        !run.out.contains("Re-pointed "),
        "it reported a repair it did not make"
    );
}
