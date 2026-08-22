//! The production runner, tested against real processes.
//!
//! Every test here spawns something: `/bin/sh` and coreutils, never a mock of
//! them. The four exotics (stdin from a real file, setsid detach, stderr-only
//! live capture, a timeout that kills) are the whole reason this seam exists,
//! and each is only true of a real child — a fake would prove that the spec
//! type has a field, not that the field does anything.
//!
//! Linux-only and deliberately so: the observations are `/proc/<pid>/fd` (which
//! stream is which file) and `/proc/<pid>/stat` (which session the child is
//! in), which is the only way to check a detach from outside the child, and
//! this port ships linux-64 (#254).

use super::*;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::time::Instant;

/// The shell every test spawns. `/bin/sh` rather than bash: it is what a POSIX
/// script may assume, and the scripts here stay inside that.
const SH: &str = "/bin/sh";

fn sh(script: &str) -> Invocation {
    Invocation::new(SH).with_arg("-c").with_arg(script)
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().expect("a scratch directory")
}

/// The captured text of a run that was expected to finish, or a panic naming
/// what came back instead. Every assertion below wants the payload, and a
/// `match` per test would bury the assertion in ceremony.
fn ran(outcome: Outcome<CapturedText>) -> (Exit, CapturedText) {
    match outcome {
        Outcome::Ran { exit, io } => (exit, io),
        other => panic!("expected a finished child, got {other:?}"),
    }
}

fn exit_of(outcome: Outcome) -> Exit {
    match outcome {
        Outcome::Ran { exit, .. } => exit,
        other => panic!("expected a finished child, got {other:?}"),
    }
}

/// The session id of `pid`, read from `/proc/<pid>/stat`.
///
/// Split at the **last** `)` rather than by whitespace from the start: field 2
/// is the executable name in parentheses and may itself contain spaces and
/// parens, which is the classic way to misparse this file.
fn session_of(pid: u32) -> u32 {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).expect("the child's /proc entry");
    let tail = stat
        .rsplit_once(')')
        .expect("a comm field in /proc/<pid>/stat")
        .1;
    // After comm: state, ppid, pgrp, session.
    tail.split_whitespace()
        .nth(3)
        .expect("a session field")
        .parse()
        .expect("a numeric session id")
}

/// Kill and reap a detached child, so a passing test leaves no zombie behind.
/// `setsid` does not reparent, so it is still ours to wait for.
fn reap(pid: u32) {
    // SAFETY: both calls take a pid we were handed for a child of this process.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
        let mut status = 0;
        libc::waitpid(pid as libc::pid_t, &mut status, 0);
    }
}

/// A script that records what the shell sees on each of its own three standard
/// descriptors, as three symlinks `fd0`/`fd1`/`fd2` under `dir`.
///
/// `ln -s` rather than a redirection, and that is not a stylistic choice: dash
/// performs a redirection in the shell process itself (saving the original
/// descriptor elsewhere and restoring it when the command finishes), so
/// `readlink /proc/$$/fd/1 > file` reports the file being written to rather than
/// what the runner handed over — the probe would answer about itself. A command
/// substitution forks, so the shell's own descriptors stay as they were, and the
/// answer travels as an argument rather than through a stream.
fn fd_probe(dir: &Path) -> Invocation {
    let script = ["0", "1", "2"]
        .map(|fd| {
            format!(
                "ln -s \"$(readlink /proc/$$/fd/{fd})\" {}/fd{fd}",
                dir.display()
            )
        })
        .join("; ");
    sh(&script)
}

/// What [`fd_probe`] recorded, waiting for it to appear (a detached child is
/// still running when the runner answers).
fn probed_fds(dir: &Path) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let seen: Vec<String> = (0..3)
            .filter_map(|fd| fs::read_link(dir.join(format!("fd{fd}"))).ok())
            .map(|target| target.display().to_string())
            .collect();
        if seen.len() == 3 {
            return seen;
        }
        assert!(
            Instant::now() < deadline,
            "the child never reported its descriptors: {seen:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// What this process has on its own three standard descriptors, in the same
/// form. A closed descriptor reads back as an empty string, which is what the
/// child would have recorded for it too.
fn our_fds() -> Vec<String> {
    (0..3)
        .map(|fd| {
            fs::read_link(format!("/proc/self/fd/{fd}"))
                .map(|target| target.display().to_string())
                .unwrap_or_default()
        })
        .collect()
}

// ---------------------------------------------------------------- captured runs

#[test]
fn a_captured_run_carries_both_streams_as_text() {
    let (exit, io) = ran(ProcessRunner.capture(&sh("printf out; printf err >&2").into()));
    assert_eq!(exit, Exit::Code(0));
    assert_eq!(io.stdout, "out");
    assert_eq!(io.stderr, "err");
}

#[test]
fn a_nonzero_status_is_reported_as_the_code() {
    let (exit, _) = ran(ProcessRunner.capture(&sh("exit 3").into()));
    assert_eq!(exit, Exit::Code(3));
    assert!(!exit.is_success());
}

#[test]
fn a_child_killed_by_a_signal_reports_the_signal_not_a_code() {
    // The distinction Python spells as a negative returncode. A code and a
    // signal are different facts, so they are different arms; nothing has to
    // know that -15 means SIGTERM.
    let (exit, _) = ran(ProcessRunner.capture(&sh("kill -TERM $$").into()));
    assert_eq!(exit, Exit::Signal(libc::SIGTERM));
    assert!(!exit.is_success());
}

#[test]
fn output_that_is_not_utf8_comes_back_replaced_rather_than_lost() {
    let (exit, io) = ran(ProcessRunner.capture(&sh("printf 'a\\377b'").into()));
    assert_eq!(exit, Exit::Code(0));
    assert_eq!(io.stdout, "a\u{fffd}b");
}

#[test]
fn a_run_in_a_directory_runs_there() {
    let dir = tmp();
    let spec: SpawnSpec = sh("pwd").with_cwd(dir.path()).into();
    let (_, io) = ran(ProcessRunner.capture(&spec));
    assert_eq!(
        Path::new(io.stdout.trim()),
        dir.path().canonicalize().expect("a real scratch directory")
    );
}

// ------------------------------------------------- not found vs ran and failed

#[test]
fn a_program_that_is_not_on_path_is_its_own_arm() {
    // The split the exit-127 rendering and DevpodNotInstalled both stand on: a
    // caller branching on the exit status would otherwise carry on as though
    // devpod had answered.
    let outcome = ProcessRunner.capture(&Invocation::new("devlaunch-no-such-program").into());
    assert_eq!(outcome, Outcome::ProgramNotFound);
}

#[test]
fn a_named_path_that_is_not_there_is_also_not_found() {
    let outcome = ProcessRunner.capture(&Invocation::new("/nonexistent/devpod").into());
    assert_eq!(outcome, Outcome::ProgramNotFound);
}

#[test]
fn a_missing_working_directory_is_not_reported_as_a_missing_program() {
    // Both refusals are ENOENT from one `spawn`, so the arm is decided by
    // asking PATH about the program rather than by trusting the errno. Without
    // that, `git` in a clone somebody deleted would render as "git is not
    // installed" — a diagnostic pointing at the wrong thing entirely.
    let spec: SpawnSpec = sh("true").with_cwd("/devlaunch-no-such-directory").into();
    match ProcessRunner.capture(&spec) {
        Outcome::NotStarted(failure) => {
            assert_eq!(failure.kind, std::io::ErrorKind::NotFound);
            assert_eq!(failure.errno, Some(libc::ENOENT));
        }
        other => panic!("expected NotStarted, got {other:?}"),
    }
}

#[test]
fn an_unreadable_stdin_file_is_not_reported_as_a_missing_program() {
    let spec = SpawnSpec::from(sh("cat")).with_stdin_file("/devlaunch-no-such-file");
    match ProcessRunner.capture(&spec) {
        Outcome::NotStarted(failure) => assert_eq!(failure.kind, std::io::ErrorKind::NotFound),
        other => panic!("expected NotStarted, got {other:?}"),
    }
}

// --------------------------------------------------------------------- stdin

#[test]
fn stdin_from_a_file_hands_the_file_itself_over() {
    // An fd handoff, not a copy: the payload tools.py streams into a container
    // runs to hundreds of megabytes, and nothing here may buffer it. What the
    // child sees on fd 0 is therefore the file, which /proc reports by name —
    // a pipe would read back as `pipe:[…]`.
    let dir = tmp();
    let payload = dir.path().join("tools.tar");
    fs::write(&payload, b"x").expect("a payload");
    let spec = SpawnSpec::from(sh("readlink /proc/self/fd/0")).with_stdin_file(&payload);
    let (_, io) = ran(ProcessRunner.capture(&spec));
    assert_eq!(
        Path::new(io.stdout.trim()),
        payload.canonicalize().expect("a real payload")
    );
}

#[test]
fn stdin_from_a_file_delivers_every_byte() {
    let dir = tmp();
    let payload = dir.path().join("big.bin");
    let chunk = vec![b'z'; 64 * 1024];
    let mut file = fs::File::create(&payload).expect("a payload");
    for _ in 0..16 {
        file.write_all(&chunk).expect("payload bytes");
    }
    drop(file);
    let spec =
        SpawnSpec::from(Invocation::new("/usr/bin/wc").with_arg("-c")).with_stdin_file(&payload);
    let (exit, io) = ran(ProcessRunner.capture(&spec));
    assert_eq!(exit, Exit::Code(0));
    assert_eq!(io.stdout.trim(), (16 * 64 * 1024).to_string());
}

#[test]
fn stdin_can_be_nulled_so_a_child_cannot_eat_what_is_not_its_own() {
    // `gh auth token` runs in the middle of a launch; stdin belongs to whatever
    // the user asked dl to run.
    let spec = SpawnSpec::from(sh("readlink /proc/self/fd/0")).with_stdin_null();
    let (_, io) = ran(ProcessRunner.capture(&spec));
    assert_eq!(io.stdout.trim(), "/dev/null");
}

#[test]
fn stdin_is_inherited_by_default() {
    let ours = fs::read_link("/proc/self/fd/0").ok();
    let (_, io) = ran(ProcessRunner.capture(&sh("readlink /proc/self/fd/0").into()));
    assert_eq!(io.stdout.trim().is_empty(), ours.is_none());
    if let Some(ours) = ours {
        assert_eq!(Path::new(io.stdout.trim()), ours);
    }
}

// ------------------------------------------------------------------ environment

#[test]
fn an_entry_is_added_to_the_environment_the_parent_has() {
    let spec: SpawnSpec = sh("printf %s \"$DEVLAUNCH_TEST_VAR:$PATH\"")
        .with_var("DEVLAUNCH_TEST_VAR", "carried")
        .into();
    let (_, io) = ran(ProcessRunner.capture(&spec));
    let (ours, path) = io.stdout.split_once(':').expect("both halves");
    assert_eq!(ours, "carried");
    assert!(!path.is_empty(), "the parent's PATH should still be there");
}

#[test]
fn an_empty_base_replaces_the_whole_environment() {
    // What Python's `env=` does, and why it exists: a secret handed to a child
    // without putting it in argv, where `ps` shows it to every other user.
    // `/usr/bin/env` by absolute path, because with nothing in the environment
    // there is no PATH to resolve it against.
    let spec: SpawnSpec = Invocation::new("/usr/bin/env")
        .with_env(EnvSpec::empty().and("GH_TOKEN", "s3cret"))
        .into();
    let (exit, io) = ran(ProcessRunner.capture(&spec));
    assert_eq!(exit, Exit::Code(0));
    assert_eq!(io.stdout, "GH_TOKEN=s3cret\n");
}

// --------------------------------------------------------------------- timeout

#[test]
fn a_timeout_gives_up_promptly() {
    let spec = SpawnSpec::from(sh("exec sleep 30")).with_timeout(Duration::from_millis(200));
    let started = Instant::now();
    assert_eq!(ProcessRunner.capture(&spec), Outcome::TimedOut);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "waited {:?}",
        started.elapsed()
    );
}

#[test]
fn a_timeout_kills_the_child_rather_than_abandoning_it() {
    // Python's subprocess.run(timeout=) kills and reaps. Proof that this one
    // does too: the child would touch a marker a second from now, and does not.
    let dir = tmp();
    let marker = dir.path().join("survived");
    let spec = SpawnSpec::from(sh(&format!("sleep 1; : > {}", marker.display())))
        .with_timeout(Duration::from_millis(100));
    assert_eq!(ProcessRunner.passthrough(&spec), Outcome::TimedOut);
    std::thread::sleep(Duration::from_millis(1600));
    assert!(!marker.exists(), "the child outlived its timeout");
}

/// The `git fetch` over ssh shape: the child forks a descendant in a session of
/// its own (ssh's ControlMaster is the production example) which inherits the
/// stdout pipe, then the child itself outstays its timeout. `read_to_end`
/// returns only at pipe EOF, and a setsid'd descendant escapes any kill aimed
/// at the child or its group — so a runner whose timeout path waits on the
/// drains never returns. `capture` must return [`Outcome::TimedOut`] within a
/// bound regardless of who still holds the pipe.
#[test]
fn a_timed_out_capture_returns_even_when_a_grandchild_holds_the_pipe() {
    let spec = SpawnSpec::from(sh("setsid sleep 30 & exec sleep 30"))
        .with_timeout(Duration::from_millis(200));
    // On a thread with a deadline of this test's own: the defect being pinned
    // is a hang, and a red run must fail rather than stall the suite.
    let capture = std::thread::spawn(move || ProcessRunner.capture(&spec));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !capture.is_finished() {
        assert!(
            Instant::now() < deadline,
            "capture never returned: the timeout path is waiting on a pipe a \
             grandchild still holds"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(capture.join().expect("the capture thread"), Outcome::TimedOut);
}

/// The cleanup half of the same decision (#301): abandoning the drains must
/// not mean abandoning the tree. A capture leads a process group of its own
/// and the expiry kill takes the whole group, so a grandchild the child forked
/// — here one that would leave a marker a second from now — dies with it.
#[test]
fn a_timed_out_capture_kills_the_whole_process_group() {
    let dir = tmp();
    let marker = dir.path().join("survived");
    let spec = SpawnSpec::from(sh(&format!(
        "(sleep 1; : > {}) & exec sleep 30",
        marker.display()
    )))
    .with_timeout(Duration::from_millis(100));
    assert_eq!(ProcessRunner.capture(&spec), Outcome::TimedOut);
    std::thread::sleep(Duration::from_millis(1600));
    assert!(!marker.exists(), "the grandchild outlived the timeout kill");
}

#[test]
fn a_timeout_that_is_not_reached_answers_normally() {
    let spec = SpawnSpec::from(sh("printf quick")).with_timeout(Duration::from_secs(30));
    let (exit, io) = ran(ProcessRunner.capture(&spec));
    assert_eq!(exit, Exit::Code(0));
    assert_eq!(io.stdout, "quick");
}

// ----------------------------------------------------------------- passthrough

#[test]
fn a_passthrough_run_inherits_our_own_streams() {
    // The `devpod up` case: progress belongs on the user's terminal as it
    // happens, so nothing is captured and the outcome carries no text at all —
    // no empty strings standing in for output that was never read.
    let dir = tmp();
    let outcome = ProcessRunner.passthrough(&fd_probe(dir.path()).into());
    assert_eq!(
        outcome,
        Outcome::Ran {
            exit: Exit::Code(0),
            io: ()
        }
    );
    assert_eq!(probed_fds(dir.path()), our_fds());
}

#[test]
fn a_passthrough_run_still_reports_how_it_ended() {
    assert_eq!(
        exit_of(ProcessRunner.passthrough(&sh("exit 42").into())),
        Exit::Code(42)
    );
}

// -------------------------------------------------------------------- session

#[test]
fn a_session_hands_over_every_stderr_line_in_order() {
    let mut lines = Vec::new();
    let outcome = ProcessRunner.session(
        &sh("printf 'first\\nsecond\\n' >&2; exit 7").into(),
        &mut |line| lines.push(line.to_string()),
    );
    assert_eq!(exit_of(outcome), Exit::Code(7));
    assert_eq!(lines, ["first", "second"]);
}

#[test]
fn a_session_hands_each_line_over_as_it_arrives() {
    // Liveness is the point: devpod's stderr is read so its report of how the
    // session ended can be interpreted, and the lines it does not hold back
    // reach the user while the session is still running. The child here will
    // not print its second line until the sink has acted on the first, so a
    // runner that buffered until exit would leave it waiting and it exits 9.
    let dir = tmp();
    let gate = dir.path().join("gate");
    let script = format!(
        "echo one >&2; i=0; while [ ! -f {gate} ] && [ $i -lt 400 ]; do sleep 0.01; i=$((i+1)); done; \
         [ -f {gate} ] || exit 9; echo two >&2",
        gate = gate.display()
    );
    let mut lines = Vec::new();
    let outcome = ProcessRunner.session(&sh(&script).into(), &mut |line| {
        lines.push(line.to_string());
        if line == "one" {
            fs::write(&gate, b"go").expect("the gate");
        }
    });
    assert_eq!(exit_of(outcome), Exit::Code(0));
    assert_eq!(lines, ["one", "two"]);
}

#[test]
fn a_session_leaves_stdin_and_stdout_alone() {
    // devpod puts the real terminal into raw mode through them and asks for a
    // pty on that basis; a pipe on either would change what devpod does.
    let dir = tmp();
    let outcome = ProcessRunner.session(&fd_probe(dir.path()).into(), &mut |_| {});
    assert_eq!(exit_of(outcome), Exit::Code(0));
    let child = probed_fds(dir.path());
    let ours = our_fds();
    assert_eq!(child[0], ours[0], "stdin must be ours, untouched");
    assert_eq!(child[1], ours[1], "stdout must be ours, untouched");
    assert!(
        child[2].starts_with("pipe:"),
        "stderr is the one stream this reads: {}",
        child[2]
    );
}

#[test]
fn a_session_that_times_out_is_killed_like_any_other_run() {
    let spec = SpawnSpec::from(sh("exec sleep 30")).with_timeout(Duration::from_millis(200));
    let started = Instant::now();
    assert_eq!(ProcessRunner.session(&spec, &mut |_| {}), Outcome::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(5));
}

/// The capture hang's shape through `session`'s one pipe: a descendant in a
/// session of its own inherits the stderr pipe and outlives the child, so the
/// reader thread never sees EOF. A `session` whose timeout has expired must
/// still return — its child cannot even be group-killed, since an interactive
/// child moved out of the foreground group takes SIGTTIN (see #301).
#[test]
fn a_timed_out_session_returns_even_when_a_grandchild_holds_stderr() {
    let spec = SpawnSpec::from(sh("setsid sleep 30 & exec sleep 30"))
        .with_timeout(Duration::from_millis(200));
    // On a thread with a deadline of this test's own: the defect being pinned
    // is a hang, and a red run must fail rather than stall the suite.
    let session = std::thread::spawn(move || ProcessRunner.session(&spec, &mut |_| {}));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !session.is_finished() {
        assert!(
            Instant::now() < deadline,
            "session never returned: the timeout path is waiting on a pipe a \
             grandchild still holds"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(session.join().expect("the session thread"), Outcome::TimedOut);
}

#[test]
fn a_session_reports_a_missing_program_too() {
    let outcome = ProcessRunner.session(
        &Invocation::new("devlaunch-no-such-program").into(),
        &mut |_| {},
    );
    assert_eq!(outcome, Outcome::ProgramNotFound);
}

// --------------------------------------------------------------------- detach

#[test]
fn a_detached_child_is_in_a_session_of_its_own() {
    // `start_new_session=True` means setsid(), not setpgid(): a new session,
    // detached from the controlling terminal, so a Ctrl-C in the shell that
    // started dl never reaches it and it outlives the process that spawned it.
    let pid = match ProcessRunner.detach(&sh("exec sleep 30")) {
        DetachOutcome::Started { pid } => pid,
        other => panic!("expected a started child, got {other:?}"),
    };
    let child_session = session_of(pid);
    assert_eq!(
        child_session, pid,
        "setsid makes the child its own session leader"
    );
    assert_ne!(child_session, session_of(std::process::id()));
    reap(pid);
}

#[test]
fn a_detached_child_gets_null_stdio() {
    // Where Python nulls only stdout and stderr. A background refresh has
    // nothing to read and nobody watching what it writes, and the descriptor it
    // does not hold is one it cannot take from the shell dl was started from.
    let dir = tmp();
    let pid = match ProcessRunner.detach(&fd_probe(dir.path())) {
        DetachOutcome::Started { pid } => pid,
        other => panic!("expected a started child, got {other:?}"),
    };
    assert_eq!(probed_fds(dir.path()), ["/dev/null"; 3]);
    reap(pid);
}

#[test]
fn a_detached_child_that_cannot_be_started_says_so() {
    assert_eq!(
        ProcessRunner.detach(&Invocation::new("devlaunch-no-such-program")),
        DetachOutcome::ProgramNotFound
    );
}

// ------------------------------------------------------------- the spec itself

#[test]
fn a_spec_carries_the_whole_argv_for_a_caller_to_read_back() {
    let spec: SpawnSpec = Invocation::new("devpod")
        .with_args(["up", "--id", "owner-repo-main"])
        .into();
    assert_eq!(spec.program(), "devpod");
    assert_eq!(spec.args(), ["up", "--id", "owner-repo-main"]);
    assert_eq!(
        spec.invocation.argv(),
        ["devpod", "up", "--id", "owner-repo-main"]
    );
}

#[test]
fn the_default_spec_touches_nothing() {
    let spec: SpawnSpec = Invocation::new("devpod").into();
    assert_eq!(spec.stdin, StdinPlan::Inherit);
    assert_eq!(spec.timeout, None);
    assert_eq!(spec.invocation.env, EnvSpec::default());
    assert_eq!(spec.invocation.env.base, EnvBase::Parent);
    assert!(spec.invocation.env.entries.is_empty());
    assert_eq!(spec.invocation.cwd, None);
}

// ------------------------------------------------------- the foreground group

/// A script that writes its own pgrp and pid, `pgrp:pid`, to `out`.
fn pgrp_probe(out: &std::path::Path) -> String {
    // /proc/self/stat, in the same after-`)` shape `session_of` reads: fields are
    // state, ppid, pgrp, session, so cut takes the third. `$$` is the child's pid.
    format!(
        "pgrp=$(sed 's/.*) //' /proc/self/stat | cut -d' ' -f3); \
         printf '%s:%s' \"$pgrp\" \"$$\" > {}",
        out.display()
    )
}

/// `passthrough` leads its child's own process group when the spec asks for it
/// (`.leading_its_own_group()`), so `dl`'s interrupt handler can `killpg` a
/// `devpod up` rather than orphan it. The child records its own pgrp and pid;
/// leading its own group means the two are equal, and that group is not this
/// test's.
#[test]
fn passthrough_gives_the_child_its_own_group_when_asked() {
    let dir = tmp();
    let out = dir.path().join("group");
    let spec = SpawnSpec::from(sh(&pgrp_probe(&out))).leading_its_own_group();
    let outcome = ProcessRunner.passthrough(&spec);
    assert!(matches!(outcome, Outcome::Ran { exit, .. } if exit.is_success()));

    let recorded = fs::read_to_string(&out).expect("the child wrote its group");
    let (pgrp, pid) = recorded.split_once(':').expect("pgrp:pid");
    assert_eq!(pgrp.trim(), pid.trim(), "the child leads its own group");

    // SAFETY: `getpgrp` reads this process's own process group; it cannot fail.
    let ours = unsafe { libc::getpgrp() };
    assert_ne!(
        pgrp.trim().parse::<i32>().expect("a numeric pgrp"),
        ours,
        "the child's group is not this process's"
    );
}

/// A default-spec `passthrough` (own_group false) keeps its child in this
/// process's group — required for an interactive `ssh -t`, which would take
/// SIGTTIN and hang if it were moved out of the terminal's foreground group.
#[test]
fn passthrough_keeps_the_child_in_our_group_by_default() {
    let dir = tmp();
    let out = dir.path().join("group");
    let outcome = ProcessRunner.passthrough(&SpawnSpec::from(sh(&pgrp_probe(&out))));
    assert!(matches!(outcome, Outcome::Ran { exit, .. } if exit.is_success()));

    let recorded = fs::read_to_string(&out).expect("the child wrote its group");
    let (pgrp, _pid) = recorded.split_once(':').expect("pgrp:pid");

    // SAFETY: `getpgrp` reads this process's own process group; it cannot fail.
    let ours = unsafe { libc::getpgrp() };
    assert_eq!(
        pgrp.trim().parse::<i32>().expect("a numeric pgrp"),
        ours,
        "the child stays in this process's group"
    );
}
