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
//! The case comments below name the Python suite each case came from. Every one of
//! those retired with the Python tree (#267), so they are provenance to grep the
//! history for rather than files to open; what pins the behaviour now is the case
//! the name sits on.
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

        deaf_to_sighup();
        let root = self.root.display().to_string();
        let mut command = Command::new(env!("CARGO_BIN_EXE_dl"));
        child_hears_sighup(&mut command);
        let mut child = command
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

    /// Run `dl` from inside a shell, and report what became of *the shell*.
    ///
    /// **The one thing `dl()` above cannot be asked, and it is a safety property
    /// rather than an inconvenience.** `dl <ws> rme` sends SIGHUP to its parent
    /// process, and the parent of a child this harness spawns is the test binary:
    /// judging `rme` through `dl()` would hang up `cargo test` itself, taking every
    /// other test in the run with it. So a shell is put in between to be the thing
    /// that dies, and it is the only correct place to judge this verb from anyway —
    /// what `rme` claims is about a shell, and nothing else in the suite has one.
    ///
    /// The shell prints its own pid **before** it runs `dl`, which does two jobs.
    /// It is the value `dl`'s own line is checked against, so a test can say the
    /// signal went to the shell and not to something else. And it forces the fork:
    /// `sh -c` with a single simple command `exec`s it, which would make `dl`'s
    /// parent the test binary after all — a compound list whose first word has
    /// already run cannot.
    ///
    /// [`Hung`] is what comes back, because a shell that was hung up has no exit
    /// code to report: it was killed by a signal, and the line it would have
    /// printed afterwards is the evidence that it never got that far.
    fn dl_inside_a_shell(&self, args: &[&str]) -> Hung {
        self.shell_run("", args)
    }

    /// `prelude` runs in the shell before `dl` does, and is where a test disarms a
    /// signal or sets a variable the run is about to inherit.
    fn shell_run(&self, prelude: &str, args: &[&str]) -> Hung {
        use std::process::Stdio;

        let root = self.root.display().to_string();
        let quoted: Vec<String> = args.iter().map(|word| format!("\"{word}\"")).collect();
        let script = format!(
            // `still-here` is only reached by a shell the signal did not kill, and
            // it carries dl's exit code so the ordinary endings are readable too.
            "echo \"shell:$$\"; {prelude}\"$DL\" {}; echo \"still-here:$?\"",
            quoted.join(" ")
        );
        deaf_to_sighup();
        let mut command = Command::new("/bin/sh");
        child_hears_sighup(&mut command);
        let output = command
            .arg("-c")
            .arg(script)
            .env_clear()
            .keeping_coverage()
            .env("DL", env!("CARGO_BIN_EXE_dl"))
            .env("PATH", format!("{root}/bin:/usr/bin:/bin"))
            .env("HOME", format!("{root}/home"))
            .env("XDG_CACHE_HOME", format!("{root}/cache"))
            .env("XDG_CONFIG_HOME", format!("{root}/config"))
            .env("DEVPOD_HOME", format!("{root}/devpod"))
            .env("DEVPOD_SHIM_STATE", format!("{root}/shim-state.json"))
            .env("DEVPOD_SHIM_LOG", format!("{root}/shim-log.jsonl"))
            .env("DEVPOD_SHIM_CONFIG", format!("{root}/shim-config.json"))
            .env("GIT_SSH_COMMAND", "false")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("/bin/sh runs");
        Hung::of(&output, &self.root)
    }

    /// The same, from a shell that has already disarmed SIGHUP.
    ///
    /// `trap "" HUP` sets `SIG_IGN`, and a `SIG_IGN` survives `exec` and is
    /// inherited by every child — which is precisely what `nohup` does and the only
    /// thing `dl` can observe about it. So this is a faithful stand-in for
    /// `nohup dl <ws> rme` in the one respect that decides the behaviour, and a
    /// better test than `nohup` itself would be: under real `nohup` the shell also
    /// ignores the signal, so it would survive whether or not `dl` sent one, and
    /// the assertion would pass on the bug.
    fn dl_inside_a_shell_that_ignores_sighup(&self, args: &[&str]) -> Hung {
        self.shell_run("trap \"\" HUP; ", args)
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

    /// Commit one file into a clone the fixture built, with the fixture's identity.
    ///
    /// The one thing a test does to a world after `lifecycle_scenario.py` hands it
    /// over, and it is here rather than behind another fixture flag because it is
    /// the *difference* between two worlds that a test wants to name: the same
    /// clone, with and without a commit of its own. Every git variable the scenario
    /// sets is set again, so the commit does not depend on the identity or the
    /// config of the machine running the suite.
    fn commit_in(&self, clone: &str, file: &str, contents: &str) {
        std::fs::write(self.path(clone).join(file), contents).expect("a file in the clone");
        for args in [
            vec!["add", "-A"],
            vec!["commit", "-q", "-m", "work nobody else has"],
        ] {
            let done = Command::new("git")
                .args(&args)
                .current_dir(self.path(clone))
                .envs([
                    ("GIT_CONFIG_GLOBAL", "/dev/null"),
                    ("GIT_CONFIG_SYSTEM", "/dev/null"),
                    ("GIT_AUTHOR_NAME", "t"),
                    ("GIT_AUTHOR_EMAIL", "t@t"),
                    ("GIT_COMMITTER_NAME", "t"),
                    ("GIT_COMMITTER_EMAIL", "t@t"),
                ])
                .output()
                .expect("git is installed");
            assert!(
                done.status.success(),
                "git {args:?} in {clone}: {}",
                String::from_utf8_lossy(&done.stderr)
            );
        }
    }

    /// Put a `git` first on the run's PATH that refuses `pack-refs` and is the
    /// real git for everything else.
    ///
    /// The one failure in this file that has to come from a *subprocess* rather
    /// than from a fixture flag: what the sweep records is git's own words, and a
    /// world that supplied them itself would be pinning the test's spelling of a
    /// refusal instead of git's.
    fn given_a_git_that_will_not_pack_refs(&self) {
        let real = ["/usr/bin/git", "/bin/git"]
            .into_iter()
            .map(Path::new)
            .find(|candidate| candidate.exists())
            .expect("git is installed");
        let shim = self.path("bin/git");
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\n\
                 for argument in \"$@\"; do\n\
                 \x20 if [ \"$argument\" = pack-refs ]; then\n\
                 \x20   echo \"{REFUSED_PACK}\" >&2\n\
                 \x20   exit 1\n\
                 \x20 fi\n\
                 done\n\
                 exec {} \"$@\"\n",
                real.display()
            ),
        )
        .expect("a git that will not pack");
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))
            .expect("an executable fake");
    }

    /// Wind every repository's fetch clock back to before the interval.
    ///
    /// The sweep is interval-gated on `last_fetched`, which the pass before it just
    /// moved, so a second pass in the same test would find nothing due and step over
    /// the very repository the test is about.
    fn restale_the_fetch_clock(&self) {
        let mut record: serde_json::Value =
            serde_json::from_str(&self.read("cache/devlaunch/metadata.json")).expect("a record");
        for repository in record["repositories"]
            .as_object_mut()
            .expect("the repositories")
            .values_mut()
        {
            repository["last_fetched"] = serde_json::json!("2020-01-01T00:00:00");
        }
        std::fs::write(
            self.path("cache/devlaunch/metadata.json"),
            record.to_string(),
        )
        .expect("a record with a stale clock");
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

/// Make this test binary deaf to SIGHUP, and hand every child the default back.
///
/// `rme` signals `dl`'s parent, and the parent of a child [`World::answering`]
/// spawns is the test binary. So a test that runs a *successful* `rme` through the
/// plain runner kills the entire run — `signal: 1, SIGHUP` out of the harness, with
/// no failing test to point at. That is a one-word mistake to make, because the
/// refusal cases genuinely do belong on the plain runner: `rme` there is correct
/// right up until the removal succeeds.
///
/// Both halves are needed and the second is the easy one to miss. A `SIG_IGN` is
/// inherited across `fork` *and* `exec`, so ignoring it here alone would hand every
/// spawned `dl` an already-disarmed SIGHUP — which `dl` now reads as `nohup` and
/// declines to hang anything up for, quietly turning every assertion about the
/// hangup into a test of the declining path. `pre_exec` puts `SIG_DFL` back in the
/// child, which is what a shell in a terminal would have given it.
///
/// Nothing is lost by ignoring it: every claim about the hangup is asserted on the
/// wrapper shell in [`World::dl_inside_a_shell`], never on this process.
fn deaf_to_sighup() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: setting a disposition on this process. Called before the first
        // child is spawned, and `SIG_IGN` needs no handler to be safe.
        unsafe {
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
        }
    });
}

/// Undo [`deaf_to_sighup`] in the child, between `fork` and `exec`.
fn child_hears_sighup(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;

    // SAFETY: `signal` is async-signal-safe and allocates nothing, which is the bar
    // for a `pre_exec` closure.
    unsafe {
        command.pre_exec(|| {
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            Ok(())
        });
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

/// One `dl` run and the shell it was run from, as [`World::dl_inside_a_shell`]
/// leaves them.
///
/// The two streams are the shell's, which is to say both processes': `dl` inherits
/// them, so its own lines and the shell's are in one order here, the order they
/// were written in.
struct Hung {
    /// The shell's pid, from the line it printed before it forked.
    shell: i32,
    /// The signal that killed the shell, if one did.
    killed_by: Option<i32>,
    /// `dl`'s exit code, from the line the shell prints afterwards. `None` when the
    /// shell never got that far.
    dl_exit: Option<i32>,
    out: String,
    err: String,
}

impl Hung {
    fn of(output: &Output, root: &Path) -> Self {
        use std::os::unix::process::ExitStatusExt as _;

        let Run { out, err, .. } = Run::of(output, root);
        let shell = out
            .lines()
            .find_map(|line| line.strip_prefix("shell:"))
            .and_then(|pid| pid.parse().ok())
            .unwrap_or_else(|| panic!("the shell printed its own pid; it printed {out:?}"));
        let dl_exit = out
            .lines()
            .find_map(|line| line.strip_prefix("still-here:"))
            .and_then(|code| code.parse().ok());
        Hung {
            shell,
            killed_by: output.status.signal(),
            dl_exit,
            out,
            err,
        }
    }

    /// The shell was hung up: killed by SIGHUP, having never reached the line after
    /// the `dl` it was running.
    ///
    /// Both halves, because either alone would pass on the wrong thing. A SIGHUP
    /// with the line printed would be a shell that died later, of something else;
    /// the line missing with no signal would be a shell that died of anything at
    /// all.
    fn was_hung_up(&self) {
        assert_eq!(
            self.killed_by,
            Some(libc::SIGHUP),
            "the shell was not killed by SIGHUP; it said {:?} / {:?}",
            self.out,
            self.err
        );
        assert_eq!(
            self.dl_exit, None,
            "the shell ran the command after dl, so it was not hung up: {:?}",
            self.out
        );
    }

    /// The shell survived, and this is what `dl` exited with.
    fn survived_with(&self, code: i32) {
        assert_eq!(
            self.killed_by, None,
            "the shell was killed by a signal: {:?} / {:?}",
            self.out, self.err
        );
        assert_eq!(
            self.dl_exit,
            Some(code),
            "dl's exit code, as the surviving shell read it: {:?}",
            self.out
        );
    }
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
    // devlaunch#88, and the Python `test_stored_workspace_id`'s
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
    // The workspace named before the round trip and again once it has gone, with the
    // clone's two lines between them in Python's order: `worktree/workspace_clone.py`
    // logs the directory from inside the removal, and `dl.py` logs the workspace
    // after it returns. That middle pair is why the notice channel is a sink — a
    // storage flow's line has to land where the storage flow said it. The outer two
    // are dl's own, and they are the only lines that name what was deleted on a
    // workspace with no clone recorded under it, where devpod's stdout is all there
    // used to be.
    assert_eq!(
        run.err,
        "Removing workspace devlaunch-main-legacy...\nRemoved workspace clone: \
         {ROOT}/cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-legacy\nRemoved local clone \
         for devlaunch-main-legacy\nRemoved workspace devlaunch-main-legacy.\n"
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

/// The workspace dl has no clone for is the case the two new lines exist for: none
/// of the clone notices fire, so before them the only thing on any stream naming
/// what had been deleted was `devpod delete`'s own stdout — devpod's wording to
/// change, and gone entirely if it ever stops printing it.
///
/// It is also what `dl rm` reads like from the picker, which hands an id back and
/// then draws its screen away: the id was never on screen while the row was being
/// chosen, and these are the lines that put it there.
#[test]
fn a_workspace_with_no_clone_is_still_named_going_in_and_coming_out() {
    let world = World::base();

    let run = world.dl(&["someones-project", "rm"]);

    run.exited(0);
    // A foreign workspace has no clone lines, so these two are all there is.
    assert_eq!(
        run.err,
        "Removing workspace someones-project...\nRemoved workspace someones-project.\n"
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
    // The Python `test_workspace_state`'s guard texts: uncommitted work and unpushed commits
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
fn a_clone_whose_last_tag_the_remote_carries_too_is_deleted_like_any_other() {
    // devlaunch#485 at the binary boundary, and it is the guard refusing a clean
    // clone rather than a dirty one — the failure that costs nothing visible until
    // it has taught you to type `--force` without reading. The clone tags a
    // release, the branch goes from both sides, and the tag is the last ref
    // reaching those commits: the remote has all of it, the clone holds nothing of
    // its own, and `dl rm` must delete it with no more ceremony than the clean one.
    let world = World::with(&["--tagged-release"]);
    let clone = "cache/devlaunch/repos/blooop/devlaunch/devlaunch-tagged-release";
    assert!(world.exists(clone), "the fixture's tagged clone");

    let run = world.dl(&["devlaunch-tagged-release", "rm"]);

    run.exited(0);
    assert_eq!(
        run.out,
        "Successfully deleted workspace devlaunch-tagged-release\n"
    );
    assert!(!world.exists(clone), "the clone was kept for a pushed tag");
    assert!(
        !world
            .read("cache/devlaunch/metadata.json")
            .contains("devlaunch-tagged-release"),
        "the record was left behind"
    );
}

#[test]
fn a_pushed_tag_does_not_hide_a_commit_that_really_is_nowhere_else() {
    // The other half of #485: the same clone with one commit of its own is still
    // refused, and refused for the commit rather than for the tag, so the exclusion
    // did not buy the test above by making the guard answer `NothingToLose` to
    // everything.
    //
    // It does not pin the *width* of the ref set, and the comment here said it did
    // until somebody checked: the commit lands on the checked-out branch, so every
    // ref set down to the branch alone finds it. `--branches` passes both of these.
    // The narrowing guards are at the clone-state seam, where the ref that would go
    // is the one under test: `a_commit_on_a_detached_worktree_head_is_still_unsaved`
    // and `a_stashed_change_is_unsaved_too` in `domain/workspace_state/tests.rs`.
    let world = World::with(&["--tagged-release"]);
    let clone = "cache/devlaunch/repos/blooop/devlaunch/devlaunch-tagged-release";
    world.commit_in(clone, "mine.txt", "an hour of work\n");

    let run = world.dl(&["devlaunch-tagged-release", "rm"]);

    run.exited(1);
    assert_eq!(
        run.err,
        "devlaunch-tagged-release holds 1 unpushed commit(s). Push or commit it, or run: dl \
         devlaunch-tagged-release rm --force\n"
    );
    assert!(world.exists(clone), "the refusal deleted the clone anyway");
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
    // The same "is gone" as the mistyped path above, on a workspace that really was
    // there: one phrasing for `--force` whatever it found, because the flag asks for
    // absence and absence is the whole of what its success establishes. A receipt
    // that read `Removed` here and `is gone` there would be dl guessing which of the
    // two happened.
    assert!(
        run.err
            .ends_with("Workspace devlaunch-dirty-fqta is gone.\n"),
        "{:?}",
        run.err
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

/// `--force` establishes absence, not a removal, so the receipt must not claim one.
///
/// A path spec is resolved without asking devpod anything — there is no triple to
/// look a record up by — and `--force` passes devpod's own `--ignore-not-found`,
/// which turns "there was nothing there" into exit 0. Both halves are deliberate,
/// and between them a mistyped directory reaches the delete, succeeds, and has
/// nothing at all to distinguish it from a workspace that was really removed.
///
/// So `Removed workspace <id>.` would be dl affirming, on its own account, a delete
/// that never happened. Mistype a path with `--force` and you get a confident
/// receipt for a workspace that never existed.
#[test]
fn a_forced_delete_says_the_workspace_is_gone_and_never_that_it_removed_one() {
    let world = World::base();

    let run = world.dl(&["./no-such-directory-here", "rm", "--force"]);

    run.exited(0);
    assert_eq!(
        run.err,
        "Removing workspace no-such-directory-here...\nWorkspace no-such-directory-here is \
         gone.\n",
        "a workspace that never existed was reported as removed"
    );
    // The delete really was attempted, which is what makes the receipt above the
    // only thing standing between a user and a false claim.
    assert_eq!(
        world.devpod_calls(),
        ["devpod delete no-such-directory-here --ignore-not-found"]
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
        // The announcement stands even though the delete then failed: it says what
        // was attempted, and the refusal under it says what came of it. No
        // `Removed workspace` line, which is the whole point of that one being last.
        //
        // The `kill` sentence is on the end because a devpod that refuses a delete
        // has two causes behind it and this run cannot tell which: the file that
        // moved, which the shim is standing in for here, and the workspace something
        // on the host is still holding. `dl <ws> kill` is the whole way out of the
        // second, sweep and delete together.
        "Removing workspace devlaunch-main-legacy...\ndevpod: cannot read \
         devcontainer.json\ndevpod could not delete devlaunch-main-legacy; keeping the local clone \
         so it stays retryable. If its devcontainer.json moved, restore the path or run: devpod \
         delete devlaunch-main-legacy --force. If something on this host is holding it instead, \
         run: dl devlaunch-main-legacy kill\n"
    );
    assert!(
        world.exists("cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-legacy"),
        "the clone was removed even though the workspace is still there"
    );
}

// ===========================================================================
// dl <ws> kill
// ===========================================================================

/// The verb's second half, at the boundary: a sweep that found nothing to kill
/// still ends in the delete, because "nothing is holding it" is not a reason to
/// leave the workspace standing. This is the shape the issue behind it asked for,
/// where the sweep printed a refusal and the `rm` typed after it deleted the
/// workspace unaided.
///
/// **No `devpod status` in the calls, and that is load-bearing rather than
/// incidental.** The delete `kill` reuses is `rm`'s, and `rm` resolves its target
/// through a status call with no deadline behind it; `kill` must not, because the
/// workspace it is typed at is the one whose devpod has stopped answering. So the
/// one call here is the delete.
#[test]
fn a_kill_that_found_nothing_holding_the_workspace_still_removes_it() {
    let world = World::base();
    let clone = "cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-legacy";
    assert!(world.exists(clone), "the fixture's clean clone");

    let run = world.dl(&["devlaunch-main-legacy", "kill"]);

    run.exited(0);
    assert!(
        run.err.contains(
            "Nothing on this host is holding workspace devlaunch-main-legacy.\nRemoving workspace \
             devlaunch-main-legacy..."
        ),
        "the sweep's verdict did not hand over to the delete: {}",
        run.err
    );
    // The insisted delete's wording: `kill` asks for absence rather than for a
    // removal, so the closing line is `is gone` rather than `Removed workspace`.
    assert!(
        run.err.contains("Workspace devlaunch-main-legacy is gone."),
        "the kill did not remove the workspace: {}",
        run.err
    );
    assert!(!world.exists(clone), "the clone was left behind");
    // Both flags, and they answer two different questions: `--ignore-not-found` is
    // dl's, so a workspace devpod never heard of counts as deleted, and `--force`
    // is devpod's, so one whose container it can no longer reach goes anyway. A
    // wedged workspace is routinely both.
    assert_eq!(
        world.devpod_calls(),
        ["devpod delete devlaunch-main-legacy --ignore-not-found --force"]
    );
}

/// The whole point of the verb having no `--force`: a wedged workspace's clone is
/// dirty almost by construction, because whatever wedged it interrupted the work
/// that was going on in it. A `kill` that stopped there would refuse in exactly the
/// case it exists for, so it names the work and deletes it.
#[test]
fn a_kill_names_the_work_it_is_about_to_destroy_and_destroys_it() {
    let world = World::base();

    let run = world.dl(&["devlaunch-dirty-fqta", "kill"]);

    run.exited(0);
    assert!(
        run.err.contains(
            "devlaunch-dirty-fqta holds 1 uncommitted change(s) (scratch.txt), and kill is \
             deleting it anyway."
        ),
        "the work destroyed was not named: {}",
        run.err
    );
    assert!(
        run.err.contains("Workspace devlaunch-dirty-fqta is gone."),
        "the kill did not remove the workspace: {}",
        run.err
    );
    // And the sentence is said *before* the delete it is about, which is the only
    // place it is worth anything: after the delete it would be a receipt for
    // something already gone.
    let named = run.err.find("holds 1 uncommitted").expect("the naming");
    let removing = run.err.find("Removing workspace").expect("the delete");
    assert!(named < removing, "the work was named after the delete ran");
}

/// The failure the transcript behind this feature actually hit, and the one an
/// exit code cannot carry: `devpod delete` blocked on the workspace's lock, which
/// it waits on with no deadline, logging the same line every five seconds. There
/// is no exit to inspect and no timeout on `rm`'s delete, so dl said nothing at
/// all and the run had to be Ctrl-C'd. Now the line is read as it arrives and
/// answered while the command is still blocked.
#[test]
fn an_rm_devpod_cannot_get_the_lock_for_names_the_kill_that_clears_it() {
    let world = World::base();
    world.devpod_answers(
        &["delete"],
        0,
        "info Trying to lock workspace, seems like another process is running that blocks this \
         workspace machine_client.go:311\n",
    );

    let run = world.dl(&["devlaunch-main-legacy", "rm"]);

    // devpod's own line is still forwarded verbatim: reading it must not consume
    // it, or the reader loses the evidence the advice is about.
    assert!(
        run.err.contains("info Trying to lock workspace"),
        "devpod's line was swallowed: {}",
        run.err
    );
    assert!(
        run.err.contains(
            "dl: devpod is waiting for another process to let go of devlaunch-main-legacy"
        ) && run
            .err
            .contains("'dl devlaunch-main-legacy kill' clears whatever is holding it"),
        "the blocked delete offered no way out: {}",
        run.err
    );
}

/// `rm` is untouched by all of it. The guard is still the thing dl refuses on its
/// own account, and the way past is still `--force`, because `rm` is the happy
/// path and the happy path does not destroy work to save a keystroke.
#[test]
fn rm_still_stops_where_kill_no_longer_does() {
    let world = World::base();

    let run = world.dl(&["devlaunch-dirty-fqta", "rm"]);

    run.exited(1);
    assert!(
        run.err.contains("run: dl devlaunch-dirty-fqta rm --force"),
        "the refusal did not offer the way past: {}",
        run.err
    );
    // Not "no devpod calls": `rm` resolves its target through a `devpod status`,
    // which `kill` deliberately skips. No *delete* is the claim.
    assert!(
        !world
            .devpod_calls()
            .iter()
            .any(|call| call.starts_with("devpod delete")),
        "a refused rm deleted something: {:?}",
        world.devpod_calls()
    );
}

/// The advice `rm` gained does not come back round at the person already taking
/// it. A `kill` whose delete devpod refuses has swept the host in the lines
/// directly above, so "run: dl <ws> kill" would be telling somebody to re-run the
/// command they are reading the output of.
#[test]
fn a_kill_whose_delete_is_refused_does_not_tell_you_to_run_kill() {
    let world = World::base();
    world.devpod_answers(&["delete"], 3, "devpod: cannot read devcontainer.json\n");

    let run = world.dl(&["devlaunch-main-legacy", "kill"]);

    run.exited(3);
    assert!(
        run.err
            .contains("devpod could not delete devlaunch-main-legacy"),
        "the delete was not attempted: {}",
        run.err
    );
    assert!(
        !run.err.contains("run: dl devlaunch-main-legacy kill"),
        "the kill told the reader to run kill: {}",
        run.err
    );
}

// ===========================================================================
// dl <ws> rme
// ===========================================================================

#[test]
fn rme_removes_the_workspace_and_hangs_up_the_shell_that_asked() {
    // The whole verb, end to end: the same delete `rm` does — the same devpod
    // calls, the same clone gone, the same lines — and then the shell it was typed
    // in, killed by the signal `dl` sent it. Judged from inside a shell for the
    // reason `dl_inside_a_shell` gives: there is nowhere else the claim exists.
    let world = World::base();
    let clone = "cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-legacy";
    assert!(world.exists(clone), "the fixture's clean clone");

    let hung = world.dl_inside_a_shell(&["devlaunch-main-legacy", "rme"]);

    hung.was_hung_up();
    // The pid is the shell's own, which is the whole claim of that line: which
    // process `dl` signals depends on the shell rather than on the line, so a
    // reader can only find out from the pid, and it has to be the one really
    // signalled.
    assert!(
        hung.err.contains(&format!(
            "Hanging up the shell dl was called from (pid {}).",
            hung.shell
        )),
        "{}",
        hung.err
    );
    // And the removal happened before any of that, with rm's own lines in rm's own
    // order. The hangup line is last because it is the only thing that comes after
    // the workspace has gone.
    assert!(
        hung.err.starts_with(
            "Removing workspace devlaunch-main-legacy...\nRemoved workspace clone: \
             {ROOT}/cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-legacy\nRemoved local \
             clone for devlaunch-main-legacy\nRemoved workspace devlaunch-main-legacy.\n"
        ),
        "{}",
        hung.err
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
        ],
        "rme asked devpod for something rm does not"
    );
}

#[test]
fn a_config_choice_rme_cannot_honour_is_said_in_the_word_the_line_used() {
    // `rm` and `rme` are one arm of the dispatcher, so the notice has to read the
    // verb to know which word to quote. Judged through the ordinary runner rather
    // than through a shell: this line is printed on the way to a refusal, and a
    // refusal hangs nothing up.
    let world = World::base();
    let run = world.dl(&["no-such-workspace", "rme", "--devcontainer", "gpu"]);
    run.exited(1);
    assert!(
        run.err
            .starts_with("Ignoring --devcontainer: it does not apply to 'rme'.\n"),
        "{}",
        run.err
    );
}

/// The one run where the shell goes with no removal behind it, pinned so it is a
/// decision rather than a surprise.
///
/// `--force` passes devpod's `--ignore-not-found` and a path spec is resolved
/// without asking devpod anything, so a mistyped directory reaches the delete and
/// succeeds. That is `rm --force`'s hazard already, and
/// `a_forced_delete_says_the_workspace_is_gone_and_never_that_it_removed_one` pins
/// the wording that exists to keep dl from affirming a delete that never happened.
/// What `rme` adds is that the line has no reader: the terminal closes over it.
///
/// It stands rather than being special-cased because absence is what `--force` asks
/// for, and the ordinary forced run is a real workspace whose uncommitted work you
/// have decided against, which is the run that most wants the tab closed. Telling
/// the two apart would need the target resolution to carry whether devpod ever
/// confirmed the workspace, which `target::Addressed` does not.
#[test]
fn a_successful_rme_through_the_plain_runner_does_not_end_the_run() {
    // The harness guard, exercised rather than trusted. This is the shape that
    // killed the suite once: `rme` on the plain runner, reaching a removal that
    // works, signalling the test binary as its parent. It has to end as an ordinary
    // passing test, and every other test in this file has to still be running
    // afterwards — which is what the rest of the file passing asserts.
    let world = World::base();

    world.dl(&["devlaunch-main-legacy", "rme"]).exited(0);

    assert!(
        !world.exists("cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-legacy"),
        "the removal did not happen"
    );
}

#[test]
fn a_refused_rme_offers_the_way_past_in_the_word_that_was_typed() {
    // The guard's sentence ends in a command to run, and `rm --force` is the wrong
    // one for a line that said `rme`: it deletes, leaves the tab open, and hands
    // back the `exit` that `rme` exists to absorb. Both words refuse identically,
    // so both are owed their own way past.
    let world = World::base();

    let run = world.dl(&["devlaunch-dirty-fqta", "rme"]);

    run.exited(1);
    assert_eq!(
        run.err,
        "devlaunch-dirty-fqta holds 1 uncommitted change(s) (scratch.txt). Push or commit it, \
         or run: dl devlaunch-dirty-fqta rme --force\n"
    );
    // And the line it offers is one that works: `--force` composes with `rme`, and
    // the shell goes as it would for any other removal. Through the shell harness
    // and not `dl()`, because this one *succeeds*: a forced `rme` that reached the
    // plain runner would hang up the test binary and take the whole run with it.
    let forced = World::base();
    let hung = forced.dl_inside_a_shell(&["devlaunch-dirty-fqta", "rme", "--force"]);
    hung.was_hung_up();
    assert!(
        !forced.exists("cache/devlaunch/repos/blooop/devlaunch/devlaunch-dirty-fqta"),
        "the way past the guard did not get past it"
    );
}

#[test]
fn a_forced_rme_closes_the_shell_over_an_absence_it_never_removed() {
    let world = World::base();

    let hung = world.dl_inside_a_shell(&["./no-such-directory-here", "rme", "--force"]);

    hung.was_hung_up();
    // The receipt that says dl did not affirm a removal, and the reader it lost.
    assert!(
        hung.err
            .contains("Workspace no-such-directory-here is gone."),
        "{}",
        hung.err
    );
    assert!(
        !hung.err.contains("Removed workspace"),
        "a workspace that never existed was reported as removed: {}",
        hung.err
    );
    // Nothing was deleted, which is the whole of what makes this worth pinning.
    assert_eq!(
        world.devpod_calls(),
        ["devpod delete no-such-directory-here --ignore-not-found"]
    );
}

#[test]
fn rme_under_nohup_leaves_the_terminal_it_was_told_to_outlive() {
    // `nohup dl <ws> rme` used to hang up the terminal, which is the one thing
    // `nohup` is typed to prevent. Two things make it that rather than a curiosity.
    // A shell runs `nohup dl …` by `exec`ing it in place, so dl's parent is the
    // interactive shell itself and not some intermediate the signal could stop at.
    // And dl already has a position on an inherited `SIG_IGN`: `install_signal_handlers`
    // treats it as a deliberate statement and leaves SIGHUP disarmed for the whole
    // run, because that is how `nohup` outlives a terminal at all. Sending the
    // signal dl itself refuses to act on is dl disagreeing with itself.
    let world = World::base();
    let clone = "cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-legacy";

    let ran = world.dl_inside_a_shell_that_ignores_sighup(&["devlaunch-main-legacy", "rme"]);

    // The removal still happens: `nohup` is about what outlives the terminal, not
    // about doing less work.
    ran.survived_with(0);
    assert!(!world.exists(clone), "the removal did not happen");
    assert!(
        ran.err.contains(
            "rme: SIGHUP was already ignored when dl started, so the shell stays. The removal is \
             done."
        ),
        "{}",
        ran.err
    );
    // Asserted separately from the survival, because a shell that ignores SIGHUP
    // survives a signal that was sent: the line is the only evidence of which
    // happened.
    assert!(
        !ran.err.contains("Hanging up"),
        "dl sent the signal it had itself been told to ignore: {}",
        ran.err
    );
}

#[test]
fn rm_in_the_same_shell_leaves_it_standing() {
    // The control, and the reason the assertion above is about a *shell* rather
    // than about a signal reaching something: everything else being equal, the word
    // without the `e` hands the shell back and the shell runs the next line.
    let world = World::base();

    let ran = world.dl_inside_a_shell(&["devlaunch-main-legacy", "rm"]);

    ran.survived_with(0);
    assert!(
        !ran.err.contains("Hanging up"),
        "rm hung up the shell anyway: {}",
        ran.err
    );
}

#[test]
fn a_refused_rme_leaves_the_shell_up_so_the_reason_can_be_read() {
    // The refusal is the case the whole ordering exists for. `dl` writes why it
    // would not delete the workspace to stderr, and hanging up the terminal that
    // was written to is the one way to guarantee nobody reads it — so a guard that
    // refuses ends the command and nothing else. The workspace is still there
    // afterwards, which is what makes the sentence worth reading.
    let world = World::base();
    let clone = "cache/devlaunch/repos/blooop/devlaunch/devlaunch-dirty-fqta";

    let refused = world.dl_inside_a_shell(&["devlaunch-dirty-fqta", "rme"]);

    refused.survived_with(1);
    assert!(
        refused.err.contains(
            "devlaunch-dirty-fqta holds 1 uncommitted change(s) (scratch.txt). Push or commit it, \
             or run: dl devlaunch-dirty-fqta rme --force"
        ),
        "{}",
        refused.err
    );
    assert!(
        !refused.err.contains("Hanging up"),
        "a refusal hung up the shell: {}",
        refused.err
    );
    assert!(world.exists(clone), "the refusal deleted the clone anyway");
}

#[test]
fn an_rme_devpod_would_not_finish_leaves_the_shell_up_too() {
    // The other half of "only a removal that worked": this one got past the guard,
    // said what it was about to do, and then devpod refused. The clone is kept and
    // the delete stays retryable — which is a thing to retry *in this shell*, and
    // the whole reason not to close it.
    let world = World::base();
    world.devpod_answers(&["delete"], 3, "devpod: cannot read devcontainer.json\n");

    let refused = world.dl_inside_a_shell(&["devlaunch-main-legacy", "rme"]);

    refused.survived_with(3);
    assert!(
        refused
            .err
            .contains("devpod could not delete devlaunch-main-legacy"),
        "{}",
        refused.err
    );
    assert!(
        !refused.err.contains("Hanging up"),
        "a delete devpod refused hung up the shell: {}",
        refused.err
    );
    assert!(
        world.exists("cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-legacy"),
        "the clone went with a workspace that is still there"
    );
}

// ===========================================================================
// the background refresh
// ===========================================================================

#[test]
fn a_command_that_changed_the_workspace_list_refreshes_the_completions_behind_it() {
    // The Python `test_dl`'s refresh classes, spawn half: the child is this build, run as
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
    // The Python `test_updater_fetch_sweep`'s TestTheChildMigratesLikeEveryOtherRun: a
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

/// What the fake git writes when it refuses to pack, and what the record must
/// come to hold: git's words, not dl's.
const REFUSED_PACK: &str = "fatal: unable to create 'packed-refs.lock': Permission denied";

#[test]
fn a_pack_the_sweep_could_not_do_is_readable_when_somebody_next_lists() {
    // devlaunch#480, end to end and in that order: the refusal is raised inside a
    // detached child whose three descriptors are `/dev/null`, so the only way it
    // reaches anybody is the record. Before this, no test could tell the notice
    // being raised from the notice being read, because nothing read it.
    let world = World::base();
    world.given_a_git_that_will_not_pack_refs();

    world.dl(&["--update-cache", "--force"]).exited(0);

    let record = world.read("cache/devlaunch/metadata.json");
    assert!(
        record.contains("\"refs_not_packed\"") && record.contains(REFUSED_PACK),
        "the sweep's refusal is not in the record: {record}"
    );

    let listed = world.dl(&["--ls"]);
    listed.exited(0);
    assert!(
        listed.err.contains(&format!(
            "Last cache sweep of blooop/devlaunch: could not pack the refs it fetched: \
             {REFUSED_PACK}"
        )),
        "--ls did not read the note back: {}",
        listed.err
    );

    let document = world.dl(&["--ls", "--json"]);
    document.exited(0);
    let rows: serde_json::Value = serde_json::from_str(&document.out).expect("the wire document");
    let notes: Vec<&serde_json::Value> = rows
        .as_array()
        .expect("an array")
        .iter()
        .filter_map(|row| row.get("lastSweep"))
        .collect();
    assert!(
        !notes.is_empty()
            && notes.iter().all(|note| {
                note["trouble"] == "refs_not_packed" && note["said"] == REFUSED_PACK
            }),
        "the wire document does not carry the note verbatim: {}",
        document.out
    );
}

#[test]
fn a_later_sweep_that_went_fine_takes_the_complaint_back_out() {
    // Overwritten on every pass that acts, which is what lets the record hold this
    // at all: no rotation, no second file, and a cache whose trouble has been fixed
    // stops complaining without anybody clearing it by hand.
    let world = World::base();
    world.given_a_git_that_will_not_pack_refs();
    world.dl(&["--update-cache", "--force"]).exited(0);
    assert!(
        world
            .read("cache/devlaunch/metadata.json")
            .contains("refs_not_packed"),
        "the first pass left nothing to clear"
    );

    std::fs::remove_file(world.path("bin/git")).expect("the real git is back on PATH");
    world.restale_the_fetch_clock();
    world.dl(&["--update-cache", "--force"]).exited(0);

    assert!(
        !world
            .read("cache/devlaunch/metadata.json")
            .contains("last_sweep"),
        "a clean pass left the last one's complaint standing: {}",
        world.read("cache/devlaunch/metadata.json")
    );
    let listed = world.dl(&["--ls"]);
    listed.exited(0);
    assert!(
        !listed.err.contains("Last cache sweep"),
        "--ls is still reading a note nothing is complaining about: {}",
        listed.err
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

/// Python's `dl --purge` plan for the base world, with devlaunch#461's two
/// additions to the block that names the survivors.
///
/// **The second deliberate divergence from Python in this file**, beside
/// [`DOCKER_BOUNDARY`] below. Python printed a bare workspace id per survivor, and
/// an id is exactly what a user cannot decide on: `someones-project` could be a
/// `dl ./project` of theirs, a `dl <git-url>`, or a workspace somebody made with
/// `devpod up`. So each one is named by its source, and the sentence under the
/// list says what removing the cache costs the workspaces that are staying.
///
/// This is the plan a run that is **about to ask** prints. `-y` prints
/// [`PURGE_PLAN_YES`], which differs in the one line that offers an action, and
/// the two are spelled out separately rather than derived from each other so that
/// a change to either is read as the output change it is.
const PURGE_PLAN: &str = "\
This will remove all devlaunch data:
  - 2 DevPod workspace(s)
  - {ROOT}/cache/devlaunch/ (workspace clones, repo caches, the shared pixi cache, completions)

Leaving 1 workspace(s) devlaunch did not create:
  - someones-project: {ROOT}/foreign/proj

Removing the cache also drops what dl recorded about them. They keep working, and `dl <workspace> rm` still removes one.
A clone an older dl placed outside the cache is named only by a record in there, though, so remove such a workspace now if the clone should go with it.

";

/// The same plan under `-y`, where the last sentence is in the tense that run has
/// earned.
///
/// "Remove such a workspace now" is an action only a reader with the question
/// still in front of them can take; the same run deletes the records that make it
/// possible three lines later. So `-y` gets the same fact as what will be true
/// from then on, and every `-y` golden below is built from this one.
const PURGE_PLAN_YES: &str = "\
This will remove all devlaunch data:
  - 2 DevPod workspace(s)
  - {ROOT}/cache/devlaunch/ (workspace clones, repo caches, the shared pixi cache, completions)

Leaving 1 workspace(s) devlaunch did not create:
  - someones-project: {ROOT}/foreign/proj

Removing the cache also drops what dl recorded about them. They keep working, and `dl <workspace> rm` still removes one.
A clone an older dl placed outside the cache is named only by a record in there, though, so from here on `dl <workspace> rm` takes such a workspace and leaves its clone standing.

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
fn the_leaving_list_names_each_survivors_source_beside_its_id() {
    // devlaunch#461. The id on its own is not something a user can decide on: a
    // `dl ./project` of theirs, a `dl <git-url>` and a workspace they made with
    // `devpod up` all read the same, and this is the one screen where somebody is
    // deciding. The source is what tells them apart.
    let world = World::base();
    let run = world.answering("n\n", &["--purge"]);
    run.exited(0);
    assert!(
        run.out
            .contains("  - someones-project: {ROOT}/foreign/proj\n"),
        "the leaving list named an id and no source:\n{}",
        run.out
    );
    assert!(
        run.out
            .contains("Removing the cache also drops what dl recorded about them."),
        "the block did not say what the purge costs the survivors:\n{}",
        run.out
    );
}

#[test]
fn a_survivor_whose_source_dl_cannot_read_is_said_to_be_one() {
    // The third arm of the source, and the reason the leaving list does not simply
    // print the detail: devpod's own object after a colon reads like a source, and
    // this is the one row where dl has nothing truer to say than the object.
    let world = World::with(&["--unplaceable"]);
    let run = world.answering("n\n", &["--purge"]);
    run.exited(0);
    assert!(
        run.out.contains(
            "  - a-source-nobody-can-read: a source dl cannot read, {\"localFolder\": 42}\n"
        ),
        "{}",
        run.out
    );
}

#[test]
fn a_purge_names_the_clone_a_retired_repos_dir_left_outside_the_cache() {
    // devlaunch#461, the case #467's review reproduced. A pre-#467 `dl` put this
    // clone under `worktree.repos_dir`, so the workspace opening it is foreign
    // here: the purge leaves it standing and removes the record that is the last
    // thing on the machine pointing at the tree. It used to print `devlaunch-main-3j1t`
    // and nothing else, which names neither the clone nor the fact that it is one.
    let world = World::with(&["--stranded-clone"]);
    let run = world.answering("n\n", &["--purge"]);
    run.exited(0);
    assert!(
        run.out.contains(
            "Leaving 2 workspace(s) devlaunch did not create:\n  \
             - someones-project: {ROOT}/foreign/proj\n  \
             - devlaunch-main-3j1t: {ROOT}/old-repos/blooop/devlaunch/devlaunch-main-3j1t\n"
        ),
        "the stranded clone's path is not in the plan:\n{}",
        run.out
    );
    // And the retired key earns its notice on this path too, which is the half the
    // list cannot cover: a clone under that root with no workspace left opening it
    // has no line in any plan, and this run is what removes its record.
    assert!(
        run.err.contains("worktree.repos_dir = '{ROOT}/old-repos'"),
        "the purge said nothing about the key that put a tree there:\n{}",
        run.err
    );

    // What the sentence under the list is warning about, on disk: the tree stays
    // and the record naming it does not.
    let purged = world.dl(&["--purge", "-y"]);
    purged.exited(0);
    assert!(
        world.exists("old-repos/blooop/devlaunch/devlaunch-main-3j1t"),
        "the purge removed a clone outside its own cache"
    );
    assert!(
        !world.exists("cache/devlaunch/metadata.json"),
        "the record survived, so the sentence about losing it is wrong"
    );
    assert_eq!(
        world.devpod_calls().last().map(String::as_str),
        Some("devpod delete devlaunch-dirty-fqta --force"),
        "the stranded workspace was deleted, or the ownership scope moved"
    );
}

#[test]
fn removing_a_stranded_workspace_before_the_purge_takes_its_clone() {
    // What the plan's last sentence advises, checked rather than assumed. It is
    // true because `resolve_clone_path` prefers the record's absolute `local_path`
    // over the path derived from the cache root, and every unit test around that
    // function uses a path *under* the clone root -- so a later "only remove trees
    // under the cache" hardening would turn a printed sentence into bad advice
    // with nothing failing. Found in review of devlaunch#461.
    let world = World::with(&["--stranded-clone"]);
    let run = world.dl(&["devlaunch-main-3j1t", "rm"]);
    assert!(
        !world.exists("old-repos/blooop/devlaunch/devlaunch-main-3j1t"),
        "the clone stayed: exit {:?}\nout:{}\nerr:{}",
        run.code,
        run.out,
        run.err
    );
}

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
            "{PURGE_PLAN_YES}Deleting DevPod workspace: devlaunch-main-legacy\n\
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
    // The Python `test_purge_partial_removal`'s TestTheReportIsActionable and
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
            "{PURGE_PLAN_YES}Deleting DevPod workspace: devlaunch-main-legacy\n\
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
            "{PURGE_PLAN_YES}Deleting DevPod workspace: devlaunch-main-legacy\n\
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
            "{PURGE_PLAN_YES}Deleting DevPod workspace: devlaunch-main-legacy\n\
             Deleting DevPod workspace: devlaunch-dirty-fqta\n{DOCKER_BOUNDARY}"
        )
    );
}

#[test]
fn a_purge_that_cannot_read_the_workspace_list_refuses_rather_than_purging_nothing() {
    // The Python `test_workspace_listing`'s purge-will-not-act half: a purge that quietly
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
    // The Python `test_prune_orphaned_clones`'s report and input classes, and
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
    // The Python `test_reconcile_orphaned_workspaces`'s report and confirm classes: the
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
