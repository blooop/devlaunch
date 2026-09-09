//! A launch cut short *after* `devpod up` and *before* dl's setup pass finished,
//! and what the next launch does about it.
//!
//! The sibling of `dl/tests/interrupt.rs`, and the moment just after the one that
//! file judges. There the `devpod up` is still running, so the next launch runs it
//! again and everything lands. Here the `up` has already finished: devpod has
//! written its `workspace_result.json`, the container is up, and what the Ctrl-C
//! interrupts is the `devpod ssh --command` that carries dl's own stages -- the
//! hostname, the terminal title, the onboarding memo, `gh` and `claude`.
//!
//! Nothing about that container says it is unfinished. `devpod status` answers
//! `Running` and devpod's create record is complete, which is what the fast-attach
//! arm asks, so every later `dl <ws>` attaches straight into a container whose
//! setup never ran -- forever, and identically, so the workspace is not broken in a
//! way anyone can see. It is the "sometimes it works" the report is about: whether
//! you get a whole workspace depends on which second the Ctrl-C landed in.
//!
//! So a run that opens a pass writes down that it did, and a run that finds that
//! record still standing over the container standing now finishes the pass before
//! it hands over a shell.
//!
//! Linux-only, like the sibling suite, and it spells devpod's own paths on purpose:
//! `devlaunch-core/tests/devpod_layout.rs` scopes its one-spelling rule to that
//! crate's `src` precisely so an end-to-end test can check that what dl wrote
//! landed where devpod will look for it, without routing the check through the code
//! under test.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use devlaunch_test_support::KeepingCoverage;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the repository root")
}

/// The cold world `dl/tests/launch.rs` builds, with a `devpod` that keeps devpod's
/// *records* as well as its state.
///
/// The scenario's fake devpod answers `status`, `list` and `up` out of one JSON
/// file and writes nothing under `DEVPOD_HOME`. Real devpod writes
/// `workspace.json` on the way in and `workspace_result.json` on the way out of a
/// completed `up`, and both halves of this launch read them: the fast-attach arm
/// asks whether the create finished, and the host's memory of a pass is anchored to
/// the result file's mtime so that a rebuilt container is never described by an
/// older container's record. A fake that skipped them would make every question
/// here unanswerable and the test vacuous.
struct World {
    root: PathBuf,
    _scratch: tempfile::TempDir,
}

impl World {
    fn cold() -> Self {
        let scratch = tempfile::Builder::new()
            .prefix("dlresume")
            .tempdir_in("/tmp")
            .expect("a scratch directory under /tmp");
        let root = scratch.path().to_path_buf();
        let built = Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/launch_scenario.py"))
            .arg(&root)
            .arg(repo_root().join("test/fixtures/devpod_shim.py"))
            .output()
            .expect("python3 is installed");
        assert!(
            built.status.success(),
            "launch_scenario.py failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );

        let devpod = root.join("bin/devpod");
        let original = std::fs::read_to_string(&devpod).expect("the scenario's devpod");
        let delegate = original
            .lines()
            .find(|line| line.starts_with("exec "))
            .expect("the delegate exec line");
        // Without the `exec`, so the `up` arm gets to keep going afterwards.
        let run = delegate.strip_prefix("exec ").expect("an exec line");
        // `$DL_BLOCK_PASS` is what makes one run the interrupted one: the setup pass
        // is the `ssh --command` trip, and blocking it holds `dl` at exactly the
        // moment a Ctrl-C during provisioning arrives. The session `ssh` carries no
        // `--command`, so it is never the one that blocks.
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"up\" ]; then\n\
             \x20 {run} \"$@\" || exit $?\n\
             \x20 ws=$2; prev=\n\
             \x20 for a in \"$@\"; do\n\
             \x20   if [ \"$prev\" = \"--id\" ]; then ws=$a; fi\n\
             \x20   prev=$a\n\
             \x20 done\n\
             \x20 d=\"$DEVPOD_HOME/contexts/default/workspaces/$ws\"\n\
             \x20 mkdir -p \"$d\"\n\
             \x20 printf '{{}}' > \"$d/workspace.json\"\n\
             \x20 printf '{{}}' > \"$d/workspace_result.json\"\n\
             \x20 exit 0\n\
             fi\n\
             if [ \"$1\" = \"ssh\" ] && [ -n \"$DL_BLOCK_PASS\" ]; then\n\
             \x20 case \" $* \" in\n\
             \x20   *\" --command \"*)\n\
             \x20     echo \"$$\" > \"$DL_PASS_PID\"\n\
             \x20     : > \"$DL_PASS_STARTED\"\n\
             \x20     exec sleep 300 ;;\n\
             \x20 esac\n\
             fi\n\
             {delegate}\n"
        );
        std::fs::write(&devpod, script).expect("rewrite devpod");
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&devpod, std::fs::Permissions::from_mode(0o755))
            .expect("keep devpod executable");

        World {
            root,
            _scratch: scratch,
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// A `dl blooop/devlaunch@cold` over this world.
    ///
    /// `DEVLAUNCH_NO_TOOLS` so the pass carries its stages and stops there: the
    /// stages are not tools work, so the trip this test counts still happens, and
    /// the lend and the network install that would follow it do not. What is being
    /// counted is whether a pass ran at all.
    fn dl(&self) -> Command {
        let root = self.root.display().to_string();
        let mut command = Command::new(env!("CARGO_BIN_EXE_dl"));
        command
            .arg("blooop/devlaunch@cold")
            .env_clear()
            .keeping_coverage()
            .env("PATH", format!("{root}/bin:/usr/bin:/bin"))
            .env("HOME", format!("{root}/home"))
            .env("XDG_CACHE_HOME", format!("{root}/cache"))
            .env("XDG_CONFIG_HOME", format!("{root}/config"))
            .env("DEVPOD_HOME", format!("{root}/devpod"))
            .env("DEVPOD_SHIM_STATE", format!("{root}/shim-state.json"))
            .env("DEVPOD_SHIM_LOG", format!("{root}/shim-log.jsonl"))
            .env("DEVPOD_SHIM_CONFIG", format!("{root}/shim-config.json"))
            .env("DEVLAUNCH_NO_GH_TOKEN", "1")
            .env("DEVLAUNCH_NO_TOOLS", "1")
            .env(
                "DL_PASS_STARTED",
                self.path("pass.started").display().to_string(),
            )
            .env("DL_PASS_PID", self.path("pass.pid").display().to_string())
            .env("GIT_SSH_COMMAND", "false")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null");
        command
    }

    /// Run `dl` to completion and answer how many setup passes it made.
    fn launched(&self) -> Passes {
        self.truncate_log();
        let done = self.dl().output().expect("the dl binary runs");
        assert!(
            done.status.success(),
            "dl failed ({:?}): {}",
            done.status.code(),
            String::from_utf8_lossy(&done.stderr)
        );
        Passes {
            setup_trips: self.setup_trips(),
            said: String::from_utf8_lossy(&done.stderr).into_owned(),
        }
    }

    /// Run `dl` up to the setup pass and SIGTERM it there.
    ///
    /// SIGTERM rather than SIGINT for one reason that is the harness's and not the
    /// behaviour's: a job this test backgrounds inherits an ignored SIGINT under
    /// POSIX job control, which `dl` honours on purpose (`dl/tests/interrupt.rs`
    /// pins that asymmetry). Both signals reach the same drain and the same
    /// `_exit`, which is the part that matters here: nothing unwinds, so nothing
    /// tidies up after the pass that was running.
    fn interrupted_mid_pass(&self) {
        let started = self.path("pass.started");
        let mut child = self
            .dl()
            .env("DL_BLOCK_PASS", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the dl binary runs");
        assert!(
            wait_for(|| started.exists()),
            "the setup pass never started, so the interrupt would prove nothing"
        );
        assert!(
            Command::new("kill")
                .args(["-TERM", &child.id().to_string()])
                .status()
                .expect("kill is installed")
                .success(),
            "sending SIGTERM to dl"
        );
        let status = child.wait().expect("dl exits");
        assert_eq!(
            status.code(),
            Some(143),
            "dl's own interrupted ending, 128 + SIGTERM"
        );
        self.reap_the_blocked_trip();
    }

    /// Kill the trip `dl` left blocking, because `dl` does not.
    ///
    /// Not the behaviour under test, and not a defect this test is asserting the
    /// absence of. The pass's `devpod ssh --command` stays in `dl`'s own process
    /// group on purpose -- `devlaunch_runner`'s `passthrough` explains why only
    /// `devpod up` leads a group of its own -- so a terminal Ctrl-C reaches it from
    /// the kernel and fells it. What does not reach it is the `kill <dl>` this
    /// harness sends, which is aimed at one pid. Left alone the fake trip would
    /// outlive the suite, and a test that leaves processes behind is one that makes
    /// the next one flaky.
    fn reap_the_blocked_trip(&self) {
        let Ok(pid) = std::fs::read_to_string(self.path("pass.pid")) else {
            return;
        };
        let _ = Command::new("kill").args(["-KILL", pid.trim()]).status();
    }

    fn truncate_log(&self) {
        std::fs::write(self.path("shim-log.jsonl"), "").expect("an empty log");
    }

    /// How many `devpod ssh --command` trips the log holds -- the setup pass, and
    /// the only devpod call a launch makes that is dl provisioning the container.
    fn setup_trips(&self) -> usize {
        std::fs::read_to_string(self.path("shim-log.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains("\"ssh\"") && line.contains("\"--command\""))
            .count()
    }

    /// The in-flight records dl's cache holds, by file name.
    fn open_passes(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.path("cache/devlaunch/tool-verdicts")) else {
            return Vec::new();
        };
        let mut found: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".pass"))
            .collect();
        found.sort();
        found
    }
}

/// What one completed `dl` did.
#[derive(Debug)]
struct Passes {
    setup_trips: usize,
    said: String,
}

fn wait_for(mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

#[test]
fn a_launch_killed_mid_pass_is_finished_by_the_next_one_and_only_the_next_one() {
    let world = World::cold();

    // One launch, killed while its setup pass is on the wire. The container it
    // leaves is running and devpod calls its create complete, which is the whole
    // difficulty: nothing about it is distinguishable from a workspace that is ready.
    world.interrupted_mid_pass();
    assert_eq!(
        world.open_passes().len(),
        1,
        "the killed run left no record that its pass was running: {:?}",
        world.open_passes()
    );

    // The next launch. Before this, it attached in one round trip and provisioned
    // nothing -- the fast-attach arm asks devpod's create record, and devpod's
    // create really did finish.
    let second = world.launched();
    assert_eq!(
        second.setup_trips, 1,
        "the interrupted pass was not finished: {second:?}"
    );
    assert!(
        second.said.contains("setup pass did not finish"),
        "the extra round trip went unexplained: {}",
        second.said
    );

    // And the launch after that pays nothing, which is the half that keeps the fast
    // path fast: the record is closed by the pass that finished, not left standing
    // for every attach from then on.
    let third = world.launched();
    assert_eq!(third.setup_trips, 0, "the fast attach stopped being fast");
    assert_eq!(world.open_passes(), Vec::<String>::new());
}
