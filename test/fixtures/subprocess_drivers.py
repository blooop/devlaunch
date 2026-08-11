"""Running a driver script as a real second process, and waiting on what it does.

Several things dl promises are only true *between* processes — the flock in
``worktree/locks.py`` is per open file description, and the clone and metadata
races it exists to settle need two interpreters to reproduce at all. Threads
cannot stand in for that: two threads share one file description and one GIL,
so a threaded test of an inter-process lock proves something about a lock that
is not the one shipping.

So the drivers are written to disk and run with ``sys.executable``, and the
parent coordinates with them through flag files. This module is the shared half
of that arrangement: ``test_concurrent_launches.py`` and ``test_locks.py`` both
drive processes this way, and a second copy of the harness would let the two
drift on the details that matter — inheriting the environment (so
``XDG_CACHE_HOME`` reaches the child and the test's isolation holds), capturing
both streams, and bounding every wait.

**Every wait here is bounded.** A test of a lock fails by hanging, not by
returning the wrong answer, and an unbounded wait turns that into a job that
sits until the CI runner's own timeout kills it with no output.
"""

import contextlib
import os
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import Optional, Sequence

# How long a driver may take to reach a flag, or to finish, before the wait is
# called a hang. Generous on purpose: this bounds a failure, it does not time
# anything, and a cold import of `devlaunch.worktree` on a loaded runner is
# slower than anyone expects.
DRIVER_TIMEOUT = 60


def spawn_driver(driver: str, args: list, tmp_path: Path, name: str) -> subprocess.Popen:
    """Write *driver* to ``tmp_path/name.py`` and start it as its own process.

    The environment is inherited, which is load-bearing rather than incidental:
    the suite's isolation from the developer's real cache is a set of variables
    (``XDG_CACHE_HOME``, ``XDG_CONFIG_HOME``, ``DEVPOD_HOME``), and a child
    started without them would reach the very directories every fixture exists
    to keep it out of.
    """
    script = tmp_path / f"{name}.py"
    script.write_text(driver, encoding="utf-8")
    return subprocess.Popen(
        [sys.executable, str(script), *[str(a) for a in args]],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=os.environ.copy(),
        # Its own process group, so `stop` can signal the driver *and* whatever
        # it started. A driver that shells out to `git clone` and is then killed
        # on its own leaves that clone running against a tmp_path pytest is
        # about to delete.
        start_new_session=True,
    )


def finish(proc: subprocess.Popen, label: str, timeout: int = DRIVER_TIMEOUT) -> str:
    """Wait for *proc*, insist it succeeded, and return its stdout.

    The failure message carries the child's stderr, because a driver that died
    on an ImportError is otherwise reported only as a return code.

    *timeout* is a parameter because the drivers differ in kind: one that only
    takes a lock is hung after a few seconds, while one that runs a real
    ``git clone --bare`` is merely slow. Both are bounded; they are not bounded
    by the same number.
    """
    out, err = proc.communicate(timeout=timeout)
    assert proc.returncode == 0, f"{label} failed (rc={proc.returncode}):\n{err}"
    return out


def await_flags(*flags: Path, watching: Sequence[subprocess.Popen] = ()) -> None:
    """Block until every one of *flags* exists, or fail saying which did not.

    Pass the drivers as *watching* and a child that dies before signalling ends
    the wait immediately, with its stderr in the message. Without that, the most
    common way to break a driver -- an ImportError from a renamed module, or a
    child interpreter that cannot see ``devlaunch`` -- means no flag is ever
    written, the wait burns the full timeout, and the failure reads "drivers
    never became ready" with the traceback that explains it discarded. One
    self-diagnosing second instead of a two-minute mystery.
    """
    deadline = time.monotonic() + DRIVER_TIMEOUT
    while not all(flag.exists() for flag in flags):
        for proc in watching:
            if proc.poll() is not None:
                _, err = proc.communicate()
                raise AssertionError(
                    f"a driver exited (rc={proc.returncode}) before signalling:\n{err}"
                )
        missing = [str(f) for f in flags if not f.exists()]
        assert time.monotonic() < deadline, f"drivers never became ready: {missing}"
        time.sleep(0.01)


def stop(proc: Optional[subprocess.Popen]) -> None:
    """Kill a driver that is still running, and *reap* it.

    A bare ``kill()`` is what a cleanup block usually does and it is not enough
    here. It returns before the child is gone, so nothing waits on it; and
    SIGKILL to the driver never reaches the ``git clone`` the driver started,
    which is reparented and goes on writing into ``tmp_path`` while pytest tears
    that directory down -- surfacing as teardown errors that bury the real
    failure, and as one orphan git per wedged run on CI.

    So: kill the process group, then drain and wait. Draining matters on its own
    -- the pipes hold the stderr that says why the driver was stuck, and closing
    them unread throws it away.
    """
    if proc is None or proc.poll() is not None:
        return
    with contextlib.suppress(OSError):
        os.killpg(proc.pid, signal.SIGKILL)
    with contextlib.suppress(OSError, ValueError):
        proc.kill()
    with contextlib.suppress(subprocess.TimeoutExpired, ValueError):
        proc.communicate(timeout=10)


def await_blocked_on_lock(proc: subprocess.Popen, settle: float = 0.5) -> None:
    """Block until *proc* is actually asleep waiting for a file lock.

    The thing a contention test needs to know before it releases the holder,
    and the thing a flag file cannot tell it. A driver touches its flag *before*
    calling ``hold_lock``, so the flag proves only that the child reached the
    line — release on the strength of that and a child still starting up walks
    straight into a free lock and reports no contention, which is a red tick on
    a perfectly correct lock.

    On Linux the kernel says so directly: a task parked in ``flock`` reports
    ``locks_lock_inode_wait`` in ``/proc/<pid>/wchan``. That is an observation
    rather than a guess about scheduling, so it is used where it is available.

    Everywhere else — and if the field ever stops reading that way — this falls
    back to *settle* seconds, which is what the observation replaces rather than
    supplements. The fallback can be wrong in one direction only: too short and
    the test reports a contention that did not happen, which fails loudly.

    Returns early if *proc* has exited. A process that ran to completion is
    never going to park on the lock, so waiting out the timeout would only make
    the assertion that follows slow as well as red — which is exactly the case
    when the lock under test has stopped excluding, i.e. the failure this is
    most likely to be watching.
    """
    wchan = Path(f"/proc/{proc.pid}/wchan")
    if not wchan.exists():
        time.sleep(settle)
        return
    deadline = time.monotonic() + DRIVER_TIMEOUT
    while time.monotonic() < deadline:
        try:
            if "lock" in wchan.read_text(encoding="utf-8"):
                return
        except OSError:
            # /proc is not readable the way this expects. Nothing to observe;
            # fall back rather than spin.
            break
        if proc.poll() is not None:
            return
        time.sleep(0.01)
    time.sleep(settle)
