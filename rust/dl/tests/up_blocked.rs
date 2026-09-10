//! A launch parked on devpod's workspace lock, judged at the binary boundary.
//!
//! devpod's `initLock` is a blocking `flock` acquire with no deadline, logging
//! `Trying to lock workspace …` every five seconds for as long as the holder
//! lives. `dl <ws> rm` has watched for that line since devlaunch#484 and says what
//! clears it; a launch ran its `devpod up` as a plain passthrough and read nothing,
//! so the same wedge left every launch verb silent (devlaunch#600). The trace in
//! that ticket is the shape here: an interrupted create left a `devpod up`
//! holding the lock, and the next launch sat behind it with nothing on the
//! terminal but devpod's own line.
//!
//! The fake `devpod` below has `up` print devpod's line to stderr and then block,
//! which is what holding the flock by hand would produce and needs no sibling
//! process to do it. `dl`'s stderr goes to a file so it can be read while `dl` is
//! still blocked, which is the only time the notice is worth anything; a Ctrl-C
//! then ends the run the way the person reading the notice would, and the drain
//! from devlaunch#304 is what makes that exit 130 rather than a hang of its own.
//!
//! Same world as `dl/tests/interrupt.rs`, minus `--gh`: no token is staged here
//! because nothing here is about the token.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use devlaunch_test_support::KeepingCoverage;

/// The stable half of devpod's line, as `devpod::says_it_is_blocked` matches it.
const DEVPOD_LINE: &str = "info Trying to lock workspace, seems like another process is running \
                           that blocks this workspace machine_client.go:311";

/// The opening of `dl`'s notice, as `render::launch_notice` phrases it.
const NOTICE: &str = "dl: devpod is waiting for another process to let go of ";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the repository root")
}

/// A scratch world of the shape `dl/tests/launch.rs` builds, then its `devpod`
/// replaced by one whose `up` says it is blocked and then is.
struct World {
    root: PathBuf,
    _scratch: tempfile::TempDir,
}

impl World {
    fn blocked_up() -> Self {
        let scratch = tempfile::Builder::new()
            .prefix("dlblk")
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

        // `up` prints devpod's line three times, the way devpod's five-second timer
        // would over a longer wait, and then blocks. Every other subcommand
        // delegates to the shim the scenario installed, reusing its exact `exec`
        // line so `status`/`list` behave as before.
        let devpod = root.join("bin/devpod");
        let original = std::fs::read_to_string(&devpod).expect("the scenario's devpod");
        let delegate = original
            .lines()
            .find(|line| line.starts_with("exec "))
            .expect("the delegate exec line");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"up\" ]; then\n\
             \x20 echo '{DEVPOD_LINE}' >&2\n\
             \x20 echo '{DEVPOD_LINE}' >&2\n\
             \x20 echo '{DEVPOD_LINE}' >&2\n\
             \x20 exec sleep 30\n\
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
}

fn wait_for(mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

#[test]
fn a_launch_blocked_on_the_workspace_lock_says_so_once_while_it_is_blocked() {
    let world = World::blocked_up();
    let root = world.root.display().to_string();
    let err_path = world.root.join("launch.stderr");

    // stderr goes to a file rather than a pipe, so it can be read while `dl` is
    // still sitting in the `up`: the notice is judged mid-block, not after.
    let mut child = Command::new(env!("CARGO_BIN_EXE_dl"))
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
        .env("GIT_SSH_COMMAND", "false")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&err_path).expect("a stderr file"))
        .spawn()
        .expect("the dl binary runs");

    let stderr_so_far = || std::fs::read_to_string(&err_path).unwrap_or_default();
    // devpod's line first: it proves the `up` is the one blocking, and that dl
    // forwarded it, so the watch cost the user none of devpod's own output.
    assert!(
        wait_for(|| stderr_so_far().contains(DEVPOD_LINE)),
        "devpod's lock line never reached dl's stderr; stderr so far:\n{}",
        stderr_so_far()
    );
    let said = wait_for(|| stderr_so_far().contains(NOTICE));
    assert!(
        matches!(child.try_wait(), Ok(None)),
        "dl ended on its own instead of blocking behind the up; stderr:\n{}",
        stderr_so_far()
    );
    assert!(
        said,
        "dl printed no notice while it was blocked on devpod's lock; stderr was:\n{}",
        stderr_so_far()
    );

    // The person reading the notice types Ctrl-C here (or `kill` in the other
    // terminal it names, which is not this test's to run).
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .expect("kill is installed")
            .success(),
        "sending SIGINT to dl"
    );
    let status = child.wait().expect("dl exits");
    assert_eq!(status.code(), Some(130), "a Ctrl-C mid-up drains at 130");

    let stderr = stderr_so_far();
    let notice = stderr
        .lines()
        .find(|line| line.starts_with(NOTICE))
        .expect("the notice is a line of its own");
    assert!(
        notice.contains("'dl ") && notice.contains(" kill'"),
        "the notice names the kill that clears it: {notice}"
    );
    assert!(
        notice.contains("another terminal"),
        "the notice sends the reader to another terminal, since this one is busy: {notice}"
    );
    // Once, however many times devpod said it: three lines in, one notice out.
    assert_eq!(
        stderr.matches(DEVPOD_LINE).count(),
        3,
        "every one of devpod's own lines was forwarded:\n{stderr}"
    );
    assert_eq!(
        stderr.matches(NOTICE).count(),
        1,
        "the notice was said exactly once:\n{stderr}"
    );
}
