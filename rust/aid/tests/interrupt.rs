//! Ctrl-C during an `aid` launch, judged at the binary boundary.
//!
//! `aid` runs the same launch flow as `dl`, in-process (`dl::run`), so a SIGINT
//! mid-`devpod up` stages and orphans exactly what a `dl` launch does: the
//! plaintext GitHub-token file (`$TMPDIR/devlaunch-gh-*.env`, mode 0600) and the
//! `up` child. The audit that followed the port found aid's `main` had kept a bare
//! `_exit` handler that cleaned up neither, where `dl`'s handler did — the two
//! process entry points had drifted. Both now install one shared disposition
//! (`dl::install_interrupt_handler`); this is aid's half of the proof, the twin of
//! `dl/tests/interrupt.rs`.
//!
//! Linux-only (as the whole port is, #254): it reads liveness through `kill -0` and
//! derives paths the same way the sibling suites do.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the repository root")
}

/// A scratch world of the shape `aid/tests/rewrite.rs` builds (it reuses `dl`'s
/// `launch_scenario.py`), with `--gh` so a token is staged, then its `devpod`
/// replaced by one that blocks on `up`.
struct World {
    root: PathBuf,
    _scratch: tempfile::TempDir,
}

impl World {
    fn blocking_up() -> Self {
        let scratch = tempfile::Builder::new()
            .prefix("aidint")
            .tempdir_in("/tmp")
            .expect("a scratch directory under /tmp");
        let root = scratch.path().to_path_buf();
        let dl_tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dl/tests");
        let built = Command::new("python3")
            .arg(dl_tests.join("launch_scenario.py"))
            .arg(&root)
            .arg(repo_root().join("test/fixtures/devpod_shim.py"))
            .arg("--gh")
            .output()
            .expect("python3 is installed");
        assert!(
            built.status.success(),
            "launch_scenario.py failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );

        // Replace the fake `devpod`: `up` records its own pid and then blocks,
        // every other subcommand delegates to the shim the scenario installed —
        // the same rewrite `dl/tests/interrupt.rs` makes.
        let devpod = root.join("bin/devpod");
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

        std::fs::create_dir_all(root.join("tmp")).expect("a scratch TMPDIR");
        World {
            root,
            _scratch: scratch,
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
}

/// Whether pid `pid` still exists, via `kill -0` — waiting briefly, since a killed
/// child is reaped a moment after the signal.
fn is_dead(pid: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let alive = Command::new("kill")
            .args(["-0", pid])
            .output()
            .expect("kill is installed")
            .status
            .success();
        if !alive {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The one staged GitHub-token file under `dir`, if any.
fn token_file(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        let path = entry.path();
        let name = path.file_name()?.to_string_lossy().into_owned();
        (name.starts_with("devlaunch-gh-") && name.ends_with(".env")).then_some(path)
    })
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
fn a_ctrl_c_mid_up_removes_the_token_file_and_kills_the_up() {
    let world = World::blocking_up();
    let root = world.root.display().to_string();
    let tmpdir = world.path("tmp");
    let up_pid = world.path("up.pid");
    let up_started = world.path("up.started");

    // `aid <spec> <prompt>`: the agent runs only after `up` returns, and `up`
    // blocks forever, so the token stages during `up` exactly as it does for `dl`
    // and the agent binary is never reached (none is installed).
    let mut child = Command::new(env!("CARGO_BIN_EXE_aid"))
        .args(["blooop/devlaunch@cold", "start"])
        .env_clear()
        .env("PATH", format!("{root}/bin:{root}/gh-bin:/usr/bin:/bin"))
        .env("HOME", format!("{root}/home"))
        .env("XDG_CACHE_HOME", format!("{root}/cache"))
        .env("XDG_CONFIG_HOME", format!("{root}/config"))
        .env("DEVPOD_HOME", format!("{root}/devpod"))
        .env("DEVPOD_SHIM_STATE", format!("{root}/shim-state.json"))
        .env("DEVPOD_SHIM_LOG", format!("{root}/shim-log.jsonl"))
        .env("DEVPOD_SHIM_CONFIG", format!("{root}/shim-config.json"))
        // The token is staged under TMPDIR, so pointing it at the scratch tree is
        // what lets this test both find the file and prove it is gone.
        .env("TMPDIR", tmpdir.display().to_string())
        .env("DL_UP_PID", up_pid.display().to_string())
        .env("DL_UP_STARTED", up_started.display().to_string())
        .env("GIT_SSH_COMMAND", "false")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .spawn()
        .expect("the aid binary runs");

    // Wait until `devpod up` is blocking and the token has been staged: both are
    // the preconditions the leak needs, so a test that interrupted earlier would
    // prove nothing.
    assert!(
        wait_for(|| up_started.exists() && token_file(&tmpdir).is_some()),
        "devpod up never blocked with a token staged"
    );
    let staged = token_file(&tmpdir).expect("a staged token file");
    assert!(staged.exists(), "the token is on disk before the interrupt");
    let up = std::fs::read_to_string(&up_pid).expect("the up pid");
    let up = up.trim();

    // The interrupt itself: SIGINT to `aid` alone, exactly as a terminal Ctrl-C
    // reaches `aid`'s group while the `up` — now in its own group — does not.
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .expect("kill is installed")
            .success(),
        "sending SIGINT to aid"
    );

    let status = child.wait().expect("aid exits");
    assert_eq!(status.code(), Some(130), "aid exits 130 on interrupt");
    assert!(
        token_file(&tmpdir).is_none(),
        "the token file must be gone after the interrupt, was {staged:?}"
    );
    assert!(
        is_dead(up),
        "the orphaned devpod up (pid {up}) must have been killed"
    );
}
