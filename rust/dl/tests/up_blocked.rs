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
//! The fake `devpod` below has `up` print devpod's line and then block, which is
//! what holding the flock by hand would produce and needs no sibling process to
//! do it. It prints on **stdout**, because that is where devpod puts it: its
//! logger sends `info` to stdout and only `error` and `fatal` to stderr, and a
//! watch on stderr alone was measured against the real orphan from devlaunch#600
//! and said nothing. An ordinary build line goes to stderr as well, so the test
//! also holds that both streams are forwarded, each to the stream it came from.
//! `dl`'s streams go to files so they can be read while
//! `dl` is still blocked, which is the only time the notice is worth anything; a
//! Ctrl-C then ends the run the way the person reading the notice would, and the
//! drain from devlaunch#304 is what makes that exit 130 rather than a hang.
//!
//! devlaunch#602 added the second test, and it is the one with the bar in it: a
//! launch behind an **orphan** clears it and goes on to build, rather than
//! describing the wedge and waiting. Its shim blocks the way devpod really does —
//! polling, printing the line every second while a holder lives, and continuing
//! the moment it is gone — because that poll is what makes the fix as small as it
//! is: the blocked `up` takes the flock itself, so dl frees it and re-runs
//! nothing. The holder is a real orphan (reparented to init, argv naming the
//! workspace) rather than the shim itself, since the shim is dl's own child and
//! the sweep spares anything with somebody behind it.
//!
//! Same world as `dl/tests/interrupt.rs`, minus `--gh`: no token is staged here
//! because nothing here is about the token.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use devlaunch_test_support::KeepingCoverage;

/// The stable half of devpod's line, as `devpod::says_it_is_blocked` matches it.
const DEVPOD_LINE: &str = "info Trying to lock workspace, seems like another process is running \
                           that blocks this workspace machine_client.go:311";

/// A line of the build's own, on stderr: forwarded, and not the notice's business.
const BUILD_LINE: &str = "info creating devcontainer up.go:581";

/// The opening of `dl`'s notice, as `render::launch_notice` phrases it.
const NOTICE: &str = "dl: devpod is waiting for another process to let go of ";

/// The sweep's finding when nothing on the host holds the workspace.
const SWEPT_NOTHING: &str = "nothing on this host is holding";

/// The sweep's finding when it took a holder off the lock.
const CLEARED: &str = "dl: cleared what was holding ";

/// A line devpod's shim prints once it is past the lock and really building.
const PAST_THE_LOCK: &str = "info creating devcontainer for real up.go:900";

/// The workspace `blooop/devlaunch@cold` derives, as `dl/tests/launch.rs` pins it.
/// The orphan has to name the same one the launch does, or the sweep is aimed at
/// a workspace nothing is holding.
const COLD: &str = "devlaunch-cold-8iyb";

/// The two tests here run one at a time, and the reason is the feature's rather
/// than the harness's.
///
/// The sweep reads the **host's** process table, not a scripted one, and both
/// tests launch `blooop/devlaunch@cold`, which derives the same workspace id.
/// Run together, the launch that must find nothing holding its workspace finds
/// the other test's orphan instead — names it, kills it, and reports the
/// clearance the other test was about to assert for itself. Both then fail, for
/// the same correct behaviour.
///
/// Worth stating as a property and not just a lock: two launches of one workspace
/// id on one machine are not independent, by construction. That is what makes the
/// sweep able to clear a wedge nothing else can reach, and it is why it spares
/// anything with somebody behind it.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// The lock above, unpoisoned: [`Orphan`]'s `Drop` is what clears the process
/// table after a panic, so the second test's world is sound whatever happened to
/// the first, and failing it with a poisoning error would hide the failure that
/// matters.
fn one_at_a_time() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(|held| held.into_inner())
}

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

        // `up` prints one build line on stderr, then devpod's lock line twice on
        // stdout, the way devpod's five-second timer would over a longer wait, and
        // then blocks. Every other subcommand delegates to the shim the scenario
        // installed, reusing its exact `exec` line so `status`/`list` behave as
        // before.
        let devpod = root.join("bin/devpod");
        let original = std::fs::read_to_string(&devpod).expect("the scenario's devpod");
        let delegate = original
            .lines()
            .find(|line| line.starts_with("exec "))
            .expect("the delegate exec line");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"up\" ]; then\n\
             \x20 echo '{BUILD_LINE}' >&2\n\
             \x20 echo '{DEVPOD_LINE}'\n\
             \x20 echo '{DEVPOD_LINE}'\n\
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

    /// The same world, with an `up` that blocks the way devpod's `initLock` really
    /// does: it *polls*. While the pid in `pidfile` is alive it prints devpod's
    /// line once a second; the moment that pid is gone it stops printing and
    /// delegates to the scenario's own `devpod`, so the launch completes exactly
    /// as an unblocked one would.
    ///
    /// That poll is not decoration. It is the measured behaviour the fix rests on
    /// (devlaunch#602): a real `devpod up` parked on the flock took it and printed
    /// `creating devcontainer` one second after the holder was killed, which is
    /// why dl frees the lock and re-runs nothing.
    fn blocked_while_holder_lives(pidfile: &Path) -> Self {
        let world = Self::blocked_up();
        let devpod = world.root.join("bin/devpod");
        let original = std::fs::read_to_string(&devpod).expect("the scenario's devpod");
        let delegate = original
            .lines()
            .find(|line| line.starts_with("exec "))
            .expect("the delegate exec line");
        let pidfile = pidfile.display();
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"up\" ]; then\n\
             \x20 echo '{BUILD_LINE}' >&2\n\
             \x20 while [ -s '{pidfile}' ] && kill -0 \"$(cat '{pidfile}')\" 2>/dev/null; do\n\
             \x20   echo '{DEVPOD_LINE}'\n\
             \x20   sleep 1\n\
             \x20 done\n\
             \x20 echo '{PAST_THE_LOCK}' >&2\n\
             fi\n\
             {delegate}\n"
        );
        std::fs::write(&devpod, script).expect("rewrite devpod");
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&devpod, std::fs::Permissions::from_mode(0o755))
            .expect("keep devpod executable");
        world
    }
}

/// A process that looks to `ps` exactly like the orphan devlaunch#602 was opened
/// about, owned so that it cannot outlive the test that started it.
///
/// **A handle with a `Drop` rather than a bare pid**, because the only thing that
/// ever reaps this process on the happy path is the code under test. An assertion
/// that fires before that leaves a detached `sleep` on the host for two minutes,
/// with argv naming a workspace, and the next test is serialized behind this one
/// and launches that same workspace: it sweeps the leftover, prints a clearance
/// where it asserts there is nothing to clear, and fails with a message about the
/// wrong thing. A failing test must not turn the next one into a liar.
///
/// **argv, not a name.** The sweep picks holders by two tests over the command
/// line, that it runs devpod and that it names the workspace as a whole word, so
/// `exec -a` is what makes `sleep` answer both. A process merely *called* devpod
/// would not name the workspace and would be left alone.
///
/// **Reparented, not merely backgrounded.** The distinction the sweep turns on is
/// whether anything is waiting on the holder; a child of this test process is
/// attended and would be spared, which is the sweep working correctly and the
/// test proving nothing. The shell that starts it exits immediately, so the
/// kernel hands it to init.
struct Orphan {
    pid: u32,
}

impl Orphan {
    fn holding(workspace_id: &str, pidfile: &Path) -> Self {
        let started = Command::new("bash")
            .arg("-c")
            .arg(format!(
                "setsid bash -c 'echo $$ > {pidfile}; exec -a \"devpod up {workspace_id} \
                 --ide none\" sleep 120' &",
                pidfile = pidfile.display(),
            ))
            .status()
            .expect("bash is installed");
        assert!(started.success(), "spawning the orphan");

        assert!(
            wait_for(|| read_pid(pidfile).is_some_and(|pid| parent_of(pid) == Some(1))),
            "the orphan never reparented to init",
        );
        Orphan {
            pid: read_pid(pidfile).expect("the orphan's pidfile"),
        }
    }
}

impl Drop for Orphan {
    fn drop(&mut self) {
        // Best effort and SIGKILL: on the happy path the code under test has
        // already taken it, so the usual outcome here is "no such process".
        let _ = Command::new("kill")
            .args(["-KILL", &self.pid.to_string()])
            .output();
    }
}

fn read_pid(pidfile: &Path) -> Option<u32> {
    std::fs::read_to_string(pidfile)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
}

/// The parent of `pid`, or `None` once it has left the table.
fn parent_of(pid: u32) -> Option<u32> {
    let out = Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .expect("ps is installed");
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn alive(pid: u32) -> bool {
    // `output` rather than `status`: a `kill -0` on a pid that is gone writes to
    // stderr, and the whole point of asking is that it usually is gone.
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .expect("kill is installed")
        .status
        .success()
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
    let _serialized = one_at_a_time();
    let world = World::blocked_up();
    let root = world.root.display().to_string();
    let out_path = world.root.join("launch.stdout");
    let err_path = world.root.join("launch.stderr");

    // Both streams go to files rather than pipes, so they can be read while `dl`
    // is still sitting in the `up`: the notice is judged mid-block, not after.
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
        .stdout(std::fs::File::create(&out_path).expect("a stdout file"))
        .stderr(std::fs::File::create(&err_path).expect("a stderr file"))
        .spawn()
        .expect("the dl binary runs");

    let stdout_so_far = || std::fs::read_to_string(&out_path).unwrap_or_default();
    let stderr_so_far = || std::fs::read_to_string(&err_path).unwrap_or_default();
    // devpod's lines first: they prove the `up` is the one blocking, and that dl
    // forwarded them, so the watch cost the user none of devpod's own output.
    assert!(
        wait_for(|| stdout_so_far().contains(DEVPOD_LINE) && stderr_so_far().contains(BUILD_LINE)),
        "devpod's lines never reached dl's streams; stdout so far:\n{}\nstderr so far:\n{}",
        stdout_so_far(),
        stderr_so_far()
    );
    let said = wait_for(|| stderr_so_far().contains(NOTICE));
    // And then its sweep's own finding, waited for separately: the notice lands
    // first and the sweep runs a `ps` behind it, so a Ctrl-C sent on the strength
    // of the first line alone would cut dl off mid-sweep and read as a launch
    // that never reported one.
    let swept = wait_for(|| stderr_so_far().contains(SWEPT_NOTHING));
    assert!(
        matches!(child.try_wait(), Ok(None)),
        "dl ended on its own instead of blocking behind the up; stderr:\n{}",
        stderr_so_far()
    );
    assert!(
        swept,
        "dl never reported what its sweep found; stderr was:\n{}",
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
        notice.contains("no deadline"),
        "the notice says devpod's wait is unbounded, which is why the next line matters: {notice}"
    );
    // devlaunch#602 took the instruction out of the notice, which is said before
    // dl knows whether it can clear the lock: advising a command dl is about to
    // run unasked sends the reader to do the thing happening in front of them,
    // and `kill` deletes the workspace they just asked to launch. The sweep's own
    // report does name it again, but only in the arms where dl looked and chose
    // not to act, and this run is not one of those: nothing here holds the
    // workspace, so no line in this stderr may name the verb.
    assert!(
        !stderr.contains(" kill'") && !stderr.contains("another terminal"),
        "dl still tells the reader to run kill by hand:\n{stderr}"
    );
    // Nothing on this host names this workspace, so the sweep found no holder.
    // Said rather than passed over: it is the finding that tells the reader the
    // wait is not an orphan and not dl's to clear.
    assert!(
        stderr.contains(SWEPT_NOTHING),
        "dl never reported what its sweep found:\n{stderr}"
    );
    // Once, however many times devpod said it: two lock lines in, one notice out,
    // and every line back on the stream it came from.
    let stdout = stdout_so_far();
    assert_eq!(
        stdout.matches(DEVPOD_LINE).count(),
        2,
        "devpod's stdout lines were forwarded to stdout:\n{stdout}"
    );
    assert_eq!(
        stderr.matches(BUILD_LINE).count(),
        1,
        "devpod's stderr line was forwarded to stderr:\n{stderr}"
    );
    assert_eq!(
        stderr.matches(DEVPOD_LINE).count(),
        0,
        "nothing moved devpod's stdout lines onto stderr:\n{stderr}"
    );
    assert_eq!(
        stderr.matches(NOTICE).count(),
        1,
        "the notice was said exactly once:\n{stderr}"
    );
}

/// The bar devlaunch#602 sets, at the binary boundary: the second command
/// **connects**.
///
/// An orphan holds the workspace and a launch walks into it. Before this, that
/// launch printed correct advice and then waited for as long as the orphan lived,
/// which for an init-reparented process is until the machine reboots — the
/// transcript in the issue was Ctrl-C'd out of after four and a half minutes. Now
/// it sweeps the orphan, devpod's own poll takes the freed flock, and the build
/// goes on.
///
/// No Ctrl-C anywhere in this test, and that absence is the assertion: the launch
/// has to end on its own. The `up` is neither restarted nor abandoned — the same
/// `devpod up` that was blocked is the one that builds.
#[test]
fn a_launch_blocked_behind_an_orphan_clears_it_and_gets_past_the_up() {
    let _serialized = one_at_a_time();
    let scratch = tempfile::Builder::new()
        .prefix("dlorph")
        .tempdir_in("/tmp")
        .expect("a scratch directory under /tmp");
    let pidfile = scratch.path().join("orphan.pid");
    let orphan = Orphan::holding(COLD, &pidfile);
    let world = World::blocked_while_holder_lives(&pidfile);
    let root = world.root.display().to_string();
    let out_path = world.root.join("launch.stdout");
    let err_path = world.root.join("launch.stderr");

    let mut child = Command::new(env!("CARGO_BIN_EXE_dl"))
        .arg("blooop/devlaunch@cold")
        .env_clear()
        .keeping_coverage()
        // /usr/bin and /bin carry the `ps` and `kill` the sweep runs, as they
        // carry the git a cold launch runs.
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
        .stdout(std::fs::File::create(&out_path).expect("a stdout file"))
        .stderr(std::fs::File::create(&err_path).expect("a stderr file"))
        .spawn()
        .expect("the dl binary runs");

    let stderr_so_far = || std::fs::read_to_string(&err_path).unwrap_or_default();
    let ended = wait_for(|| matches!(child.try_wait(), Ok(Some(_))));
    let stderr = stderr_so_far();
    if !ended {
        let _ = child.kill();
    }
    assert!(
        ended,
        "the launch never got past the up; it is still waiting behind the orphan. stderr:\n{stderr}"
    );

    assert!(
        !alive(orphan.pid),
        "the launch left the orphan holding the workspace"
    );
    assert!(
        stderr.contains(CLEARED),
        "dl never said it cleared the orphan:\n{stderr}"
    );
    // The pid and the command line, as `kill`'s own report names them: the
    // process is gone by the time anybody reads this, so the report is the only
    // record of what was killed on their behalf.
    let cleared = stderr
        .lines()
        .find(|line| line.contains(CLEARED))
        .expect("the sweep's line");
    assert!(
        cleared.contains(&orphan.pid.to_string()) && cleared.contains("devpod up"),
        "the line names the pid and command it killed: {cleared}"
    );
    // And the `up` that was blocked is the one that built: the shim only prints
    // this after its poll sees the holder gone.
    assert!(
        stderr.contains(PAST_THE_LOCK),
        "the blocked up never got past the lock:\n{stderr}"
    );
}

/// The fixture's own promise, since nothing else checks it: an [`Orphan`] that
/// goes out of scope takes its process with it.
///
/// Its own workspace id, so it cannot be swept by either test above and does not
/// need the lock they share.
#[test]
fn an_orphan_fixture_does_not_outlive_the_test_that_started_it() {
    let scratch = tempfile::Builder::new()
        .prefix("dlguard")
        .tempdir_in("/tmp")
        .expect("a scratch directory under /tmp");
    let pid = {
        let orphan = Orphan::holding("dl602-guard-probe", &scratch.path().join("orphan.pid"));
        assert!(alive(orphan.pid), "the fixture starts a live process");
        orphan.pid
    };

    assert!(
        wait_for(|| !alive(pid)),
        "the orphan outlived its handle and is still on the host as {pid}"
    );
}
