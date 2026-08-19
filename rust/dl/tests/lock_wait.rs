//! The contended-lock wait notice, judged at the binary boundary.
//!
//! Python prints one line to stderr when a run sits blocked on another dl run's
//! lock — `dl: waiting for another dl run preparing {owner}/{repo}` for the
//! per-repo lock (`worktree/locks.py:89`, `dl.py` `_repo_lock`) — so a
//! `--prune`/`--reconcile` that has gone quiet says why. The concurrency review
//! (R7) found the Rust prune path dropped it: the typed wait event existed but
//! nothing rendered it, leaving an empty stderr while the command blocked.
//!
//! This holds the repo lock in a sibling process for a couple of seconds and
//! asserts `dl --prune` prints Python's exact line while it waits, then acquires
//! and finishes once the sibling lets go. Building the world reuses
//! `tests/lifecycle_scenario.py`'s `--prunable` fixture (clones under
//! `blooop/devlaunch` for prune to weigh, so prune takes that repo's lock).

use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the repository root")
}

fn scratch_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("dllw")
        .tempdir_in("/tmp")
        .expect("a scratch directory under /tmp")
}

#[test]
fn a_prune_blocked_on_the_repo_lock_says_it_is_waiting() {
    let scratch = scratch_dir();
    let root = scratch.path().to_path_buf();
    let built = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lifecycle_scenario.py"))
        .arg(&root)
        .arg(repo_root().join("test/fixtures/devpod_shim.py"))
        .arg("--prunable")
        .output()
        .expect("python3 is installed");
    assert!(
        built.status.success(),
        "lifecycle_scenario.py failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );

    // Hold blooop/devlaunch's repo lock in a sibling for a few seconds, so
    // `dl --prune` queues behind it long enough to print the notice and then
    // acquires once it lets go. `flock -x` on the very file dl opens (`.lock`
    // beside the bare clone) is what a second dl run would take. The holder leads
    // its own process group with null stdio, so nothing it spawns lingers holding
    // this test's pipes or waits past its short sleep.
    let lock = root.join("cache/devlaunch/repos/blooop/devlaunch/.lock");
    let held = root.join("held");
    let mut holder = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "exec 9>'{lock}'; flock -x 9; : > '{held}'; sleep 3",
            lock = lock.display(),
            held = held.display(),
        ))
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("sh and flock are installed");

    let deadline = Instant::now() + Duration::from_secs(10);
    while !held.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(held.exists(), "the sibling never took the repo lock");

    // `dl --prune -y` to completion: it blocks on the held lock (printing the line
    // under test), then the sibling's short sleep ends, the lock frees and dl
    // finishes on its own. Startup reaches the lock well inside the hold.
    //
    // stderr goes to a FILE, not a pipe, and dl runs in its own process group.
    // Both are load-bearing: `--prune` spawns the detached completion-refresh
    // child, and a pipe stays unread-until-EOF while any writer — including that
    // detached grandchild if it races the parent's exit — still holds it, which
    // wedged the CI runner here (locally the parent won the race). A file has no
    // EOF to wait on, and a deadline with a process-group kill means a genuinely
    // stuck dl fails this one test in seconds instead of holding the runner.
    let rootstr = root.display().to_string();
    let err_path = root.join("prune.stderr");
    let mut prune = Command::new(env!("CARGO_BIN_EXE_dl"))
        .args(["--prune", "-y"])
        .env_clear()
        .env("PATH", format!("{rootstr}/bin:/usr/bin:/bin"))
        .env("HOME", format!("{rootstr}/home"))
        .env("XDG_CACHE_HOME", format!("{rootstr}/cache"))
        .env("XDG_CONFIG_HOME", format!("{rootstr}/config"))
        .env("DEVPOD_HOME", format!("{rootstr}/devpod"))
        .env("DEVPOD_SHIM_STATE", format!("{rootstr}/shim-state.json"))
        .env("DEVPOD_SHIM_LOG", format!("{rootstr}/shim-log.jsonl"))
        .env("DEVPOD_SHIM_CONFIG", format!("{rootstr}/shim-config.json"))
        .env("GIT_SSH_COMMAND", "false")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&err_path).expect("a stderr file"))
        .spawn()
        .expect("the dl binary runs");

    let prune_deadline = Instant::now() + Duration::from_secs(30);
    let timed_out = loop {
        match prune.try_wait().expect("waiting on dl") {
            Some(_) => break false,
            None if Instant::now() >= prune_deadline => {
                // Where was it stuck? Captured before the kill, printed by the
                // assert below — the one shot at diagnosing a CI-only hang.
                let ps = Command::new("ps")
                    .args(["-eo", "pid,ppid,pgid,stat,wchan:24,args"])
                    .output()
                    .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
                    .unwrap_or_default();
                let ours: String = ps
                    .lines()
                    .filter(|l| l.contains(&rootstr) || l.contains("PID"))
                    .collect::<Vec<_>>()
                    .join("\n");
                eprintln!("dl --prune still running at the deadline; processes:\n{ours}");
                // `Child::kill` is the kill(2) syscall on the exact pid — no argv
                // parsing. The group kill needs `--`: without it procps `kill`
                // reads `-<pgid>` as another signal word and exits 0 having
                // signalled nothing, which is how the first version of this
                // deadline never actually killed anything.
                let _ = prune.kill();
                let _ = Command::new("kill")
                    .args(["-KILL", "--", &format!("-{}", prune.id())])
                    .status();
                let _ = prune.wait();
                break true;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };

    // The holder's group, killed so its `sleep` never outlives the test.
    let _ = Command::new("kill")
        .args(["-KILL", "--", &format!("-{}", holder.id())])
        .status();
    let _ = holder.wait();

    let stderr = std::fs::read_to_string(&err_path).unwrap_or_default();
    assert!(
        !timed_out,
        "dl --prune did not finish within 30s of the lock being released; \
         stderr so far:\n{stderr}"
    );
    assert!(
        stderr
            .lines()
            .any(|line| line == "dl: waiting for another dl run preparing blooop/devlaunch"),
        "dl --prune printed no wait notice while it blocked; stderr was:\n{stderr}"
    );
}
