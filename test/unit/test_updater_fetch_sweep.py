# pylint: disable=redefined-outer-name
"""The hourly freshness fetch, once it belongs to the detached updater.

`dl --update-cache` is the child devlaunch already spawns and forgets after a
command finishes. Nobody waits on it, which is exactly why the broad
`+refs/heads/*` sweep belongs here rather than on a launch: out here a slow
network costs a user nothing, and a repo another dl run is busy with is simply
left for next time.

Pinned at the subprocess boundary, like test_devpod_spawn_counts.py, because
what matters is which git commands the child actually runs — a sweep that
fetched under a foreground launch's nose, or one that never fetched at all,
both look fine from inside the code and are told apart only by the argv.

**The boundary is replaced in this thread and nowhere else.** An earlier version
ran the child on a daemon thread and entered `patch("subprocess.run")` inside it,
so a thread that outlived its 20s join could restore the real `subprocess.run`
underneath a later test — and a unit run that reaches the network is worthless
whichever way it then goes. The updater runs inline here instead, with a signal
deadline standing in for the join: the "does not block" property is still
enforced, and the replacement cannot outlive the test that made it.
"""

import contextlib
import fcntl
import json
import os
import signal
import subprocess
import threading
from datetime import datetime, timedelta
from pathlib import Path
from typing import List, Optional

import pytest

from devlaunch import dl
from devlaunch.workspace_id import WorkspaceId
from devlaunch.worktree.config import get_worktree_config
from devlaunch.worktree.models import BaseRepository
from devlaunch.worktree.repo_manager import RepositoryManager
from devlaunch.worktree.storage import SCHEMA_VERSION, MetadataStorage
from devlaunch.xdg import devlaunch_cache

BROAD_FETCH = ["git", "fetch", "origin", "+refs/heads/*:refs/heads/*", "--tags", "--prune"]


class Subprocesses:
    """Records every command line and answers it plausibly enough to continue.

    The child refreshes completions before it sweeps, so plenty of git and
    devpod runs pass through here that have nothing to do with fetching; the
    assertions look at `fetches` alone.
    """

    def __init__(
        self,
        fetch_fails_in: Optional[Path] = None,
        fetch_hangs_in: Optional[Path] = None,
    ) -> None:
        self.commands: List[List[str]] = []
        self.kwargs: List[dict] = []
        # A remote this recorder cannot reach, named by the bare repo it would
        # be fetched into — how `git fetch` reports an unreachable origin.
        self.fetch_fails_in = fetch_fails_in
        # A remote that accepts the connection and then says nothing — how a
        # fetch reaches its timeout rather than its error.
        self.fetch_hangs_in = fetch_hangs_in

    def run(self, args, **kwargs) -> subprocess.CompletedProcess:
        argv = [str(a) for a in args]
        self.commands.append(argv)
        self.kwargs.append(kwargs)
        cwd = kwargs.get("cwd")
        is_fetch = argv[:2] == ["git", "fetch"]
        if is_fetch and cwd is not None and Path(cwd) == self.fetch_hangs_in:
            raise subprocess.TimeoutExpired(argv, kwargs.get("timeout") or 0)
        if is_fetch and self.fetch_fails_in is not None and Path(cwd or "") == self.fetch_fails_in:
            raise subprocess.CalledProcessError(128, argv, stderr="no route to host")
        stdout = ""
        if argv[:1] == ["devpod"]:
            stdout = "{}" if argv[1:3] == ["context", "options"] else "[]"
        return subprocess.CompletedProcess(args=argv, returncode=0, stdout=stdout, stderr="")

    @property
    def fetches(self) -> List[List[str]]:
        """Every `git fetch`, whoever ran it."""
        return [c for c in self.commands if c[:2] == ["git", "fetch"]]

    @property
    def fetch_kwargs(self) -> List[dict]:
        """The keyword arguments each `git fetch` was run with."""
        return [k for c, k in zip(self.commands, self.kwargs) if c[:2] == ["git", "fetch"]]


class CachedRepo:
    """One repo in the bare-clone cache, with a fetch clock the test sets."""

    def __init__(self, owner: str = "owner", repo: str = "repo") -> None:
        self.owner = owner
        self.repo = repo
        self.remote_url = f"https://github.com/{owner}/{repo}.git"
        repos_dir = Path(get_worktree_config().repos_dir)
        self.root = repos_dir / owner / repo
        self.bare = self.root / ".bare"
        self.bare.mkdir(parents=True, exist_ok=True)
        (self.bare / "HEAD").write_text("ref: refs/heads/main\n", encoding="utf-8")
        self.lock_path = self.root / ".lock"

    def last_fetched_at(self, when: Optional[datetime]) -> None:
        """Write the repository record with this last-fetch time."""
        MetadataStorage().add_repository(
            BaseRepository(
                owner=self.owner,
                repo=self.repo,
                remote_url=self.remote_url,
                local_path=self.bare,
                last_fetched=when,
            )
        )

    def last_fetched(self) -> Optional[datetime]:
        """Read the last-fetch time back off disk."""
        stored = MetadataStorage().get_repository(self.owner, self.repo)
        assert stored is not None
        return stored.last_fetched


@pytest.fixture
def cached_repo() -> CachedRepo:
    """A repo the cache already knows, in the suite's isolated XDG cache."""
    return CachedRepo()


@contextlib.contextmanager
def deadline(seconds: float, note: str):
    """Fail after *seconds* instead of hanging, without leaving this thread.

    A wall-clock alarm rather than a worker thread with a join: the property
    under test is that the sweep never queues behind a held lock, and a sweep
    that did queue must cost one failed test rather than a wedged suite. Doing
    it with a signal keeps the whole run — including the subprocess boundary it
    replaces — inside the test, which a thread could not promise.

    Generous by design. This is a wedge guard, not a budget: the work it times
    is entirely in-process once the boundary is replaced, so any value that a
    loaded machine can miss would be trading one flake for another.
    """

    def fired(_signum, _frame):
        raise AssertionError(note)

    previous = signal.signal(signal.SIGALRM, fired)
    signal.setitimer(signal.ITIMER_REAL, seconds)
    try:
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous)


def run_updater(monkeypatch, recorder: Subprocesses, timeout: float = 30.0) -> None:
    """Run the detached child's command line inline, failing rather than hanging.

    `monkeypatch` and not `mock.patch`: pytest undoes it at teardown of this very
    test, in this thread, whatever the test did — so no later test can inherit a
    real `subprocess.run` from a replacement that got away.
    """
    monkeypatch.setattr(subprocess, "run", recorder.run)
    with deadline(timeout, "the cache updater blocked instead of moving on"):
        assert dl.main(["--update-cache", "--force"]) == 0


class TestTheUpdaterSweepsTheCache:
    """Freshness converges in the background, on the interval it always had."""

    def test_a_repo_past_its_interval_is_fetched(self, cached_repo, monkeypatch):
        """The broad sweep the launch path used to pay for, run by the child."""
        cached_repo.last_fetched_at(datetime.now() - timedelta(hours=2))
        recorder = Subprocesses()

        run_updater(monkeypatch, recorder)

        assert recorder.fetches == [BROAD_FETCH]

    def test_fetching_advances_the_shared_fetch_clock(self, cached_repo, monkeypatch):
        """`last_fetched` is shared with the launch path, so a sweep is what
        stops a launch reaching for the same fetch a second time."""
        stale = datetime.now() - timedelta(hours=2)
        cached_repo.last_fetched_at(stale)

        run_updater(monkeypatch, Subprocesses())

        assert cached_repo.last_fetched() > stale

    def test_a_repo_within_its_interval_is_left_alone(self, cached_repo, monkeypatch):
        """The interval is the whole point: this is not a fetch-every-command."""
        cached_repo.last_fetched_at(datetime.now())
        recorder = Subprocesses()

        run_updater(monkeypatch, recorder)

        assert recorder.fetches == []


class TestTheUpdaterYieldsToLaunches:
    """Background defers to foreground; never the other way round."""

    def test_a_repo_another_run_is_holding_is_skipped(self, cached_repo, monkeypatch):
        """A launch holds the repo lock while it clones. The sweep must neither
        wait for it nor fetch behind its back — it just comes back next hour."""
        cached_repo.last_fetched_at(datetime.now() - timedelta(hours=2))
        recorder = Subprocesses()
        held = os.open(cached_repo.lock_path, os.O_RDWR | os.O_CREAT, 0o600)
        fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
        try:
            run_updater(monkeypatch, recorder)
        finally:
            os.close(held)

        assert recorder.fetches == []

    def test_a_skipped_repo_keeps_its_fetch_clock(self, cached_repo, monkeypatch):
        """Nothing was fetched, so nothing may claim it was: moving the clock
        would buy the contended repo another hour of staleness."""
        stale = datetime.now() - timedelta(hours=2)
        cached_repo.last_fetched_at(stale)
        held = os.open(cached_repo.lock_path, os.O_RDWR | os.O_CREAT, 0o600)
        fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
        try:
            run_updater(monkeypatch, Subprocesses())
        finally:
            os.close(held)

        assert cached_repo.last_fetched() == stale


class TestTheSweepSurvivesABadRepo:
    """A detached child has nobody to complain to, so it complains to nobody."""

    def test_a_failing_fetch_does_not_stop_the_next_repo(self, cached_repo, monkeypatch):
        """One unreachable remote must not cost every other repo its refresh."""
        first = cached_repo
        first.last_fetched_at(datetime.now() - timedelta(hours=2))
        second = CachedRepo(owner="owner", repo="other")
        second.last_fetched_at(datetime.now() - timedelta(hours=2))

        recorder = Subprocesses(fetch_fails_in=first.bare)

        run_updater(monkeypatch, recorder)

        assert len(recorder.fetches) == 2
        assert second.last_fetched() > first.last_fetched()


class TestTheSweepBoundsHowLongItHoldsARepo:
    """The sweep never queues for a launch — but a launch can queue for it.

    `run_if_lock_free` only promises the *sweep* does not wait. The lock it takes
    is the same one `ensure_repo` blocks on, and it is held for the whole of the
    fetch, so a launch of that repo waits for however long the fetch takes. That
    was reproduced with two real processes: a sweep holding the lock across an
    8s fetch made a foreground `hold_lock` wait 6.51s, printing `dl: waiting for
    another dl run preparing owner/repo` — a run the user cannot see, and cannot
    Ctrl-C either, because the child is spawned with `start_new_session=True`.

    Untimed, that wait has no upper bound: a `git fetch` against a remote that
    accepts the connection and then goes silent sits in the kernel's TCP
    keepalive, not in any deadline of git's. Bounding the background fetch is
    what turns "possibly forever" into a number, and it costs the sweep nothing
    it cannot make up on the next hour's pass.
    """

    def test_the_background_fetch_cannot_hold_a_repo_indefinitely(self, cached_repo, monkeypatch):
        """The fetch the sweep runs under the lock is given a deadline."""
        cached_repo.last_fetched_at(datetime.now() - timedelta(hours=2))
        recorder = Subprocesses()

        run_updater(monkeypatch, recorder)

        assert [k.get("timeout") for k in recorder.fetch_kwargs] == [
            dl.BACKGROUND_FETCH_TIMEOUT_SECONDS
        ]

    def test_the_broad_fetch_is_the_sweep_s_alone(self, cached_repo, monkeypatch):
        """The launch path has no broad fetch left to bound.

        This test used to assert the foreground kept an *untimed* version of the
        same fetch, deferring its future to devlaunch#150. #150 landed and the
        answer was to delete it: ensure_repo is now clone-if-missing only, so the
        question of what deadline to give its fetch has no subject. What a cold
        launch does fetch — one targeted ref — is pinned in
        test/test_cold_launch_fetches.py.
        """
        cached_repo.last_fetched_at(datetime.now() - timedelta(hours=2))
        recorder = Subprocesses()
        config = get_worktree_config()
        manager = RepositoryManager(repos_dir=Path(config.repos_dir), config=config)
        monkeypatch.setattr(subprocess, "run", recorder.run)

        # The launch path proper: what ensure_repo does for an already-cloned
        # repo whose interval has elapsed.
        manager.ensure_repo("owner", "repo", cached_repo.remote_url)

        assert recorder.fetch_kwargs == []

    def test_a_fetch_that_hits_its_deadline_does_not_stop_the_next_repo(
        self, cached_repo, monkeypatch
    ):
        """A timeout is one more thing to step over, not a way to end the sweep.

        It arrives as `subprocess.TimeoutExpired`, which is a `SubprocessError`
        and not an `OSError`, so a sweep that only caught the families it already
        knew would propagate it out of the loop and every later repo would lose
        its refresh to the first slow remote.
        """
        first = cached_repo
        first.last_fetched_at(datetime.now() - timedelta(hours=2))
        second = CachedRepo(owner="owner", repo="other")
        second.last_fetched_at(datetime.now() - timedelta(hours=2))

        recorder = Subprocesses(fetch_hangs_in=first.bare)

        run_updater(monkeypatch, recorder)

        assert len(recorder.fetches) == 2
        assert second.last_fetched() > first.last_fetched()


class TestTheChildMigratesLikeEveryOtherRun:
    """The detached child reaches metadata the way the rest of dl does.

    It is the process least able to afford its own construction path: nobody
    reads its output, so a child writing records in a shape current dl no longer
    looks for would go unnoticed until some later foreground command could not
    find a workspace it owns.
    """

    def _legacy_cache(self) -> Path:
        """A cache written by a pre-#64 devlaunch: old leaf name, old header."""
        repos_dir = Path(get_worktree_config().repos_dir)
        clone = repos_dir / "blooop" / "devlaunch" / "main"
        (clone / ".git").mkdir(parents=True, exist_ok=True)
        (repos_dir / "blooop" / "devlaunch" / ".bare").mkdir(parents=True, exist_ok=True)
        metadata = devlaunch_cache() / "metadata.json"
        metadata.parent.mkdir(parents=True, exist_ok=True)
        metadata.write_text(
            json.dumps(
                {
                    "version": 1,
                    "repositories": {},
                    "worktrees": {
                        "blooop/devlaunch/main": {
                            "owner": "blooop",
                            "repo": "devlaunch",
                            "branch": "main",
                            "local_path": str(clone),
                            "workspace_id": "devlaunch-main",
                            "created_at": "2024-01-01T10:00:00",
                            "last_used": "2024-01-01T12:00:00",
                            "devpod_workspace_id": None,
                        }
                    },
                }
            ),
            encoding="utf-8",
        )
        return clone

    def test_the_child_migrates_the_cache_before_touching_metadata(self, monkeypatch):
        """The clone directory is renamed onto the current id scheme, which is
        what any dl command run in the foreground would have done first."""
        old_clone = self._legacy_cache()
        new_clone = old_clone.parent / WorkspaceId("blooop", "devlaunch", "main").value

        run_updater(monkeypatch, Subprocesses())

        assert not old_clone.exists()
        assert new_clone.is_dir()

    def test_the_child_leaves_the_metadata_on_the_current_schema(self, monkeypatch):
        """A header still claiming the old version means the next foreground run
        migrates paths this child has already been writing to."""
        self._legacy_cache()

        run_updater(monkeypatch, Subprocesses())

        on_disk = json.loads((devlaunch_cache() / "metadata.json").read_text(encoding="utf-8"))
        assert on_disk["version"] == SCHEMA_VERSION


class TestTheSubprocessBoundaryStaysInsideTheTest:
    """A unit run must not be able to reach the network, ever.

    Two runs of this file were seen failing with the recorder empty while the
    code logged a successful fetch — the boundary had been un-replaced under a
    running test. Nothing about that was ever reproduced on demand, so this pins
    the property that made it possible rather than the mechanism: the updater
    runs in the thread that replaced `subprocess.run`, and pytest puts the real
    one back before the next test starts.
    """

    def test_the_updater_runs_in_the_thread_that_replaced_subprocess_run(
        self, cached_repo, monkeypatch
    ):
        """If it ran anywhere else, a replacement could outlive its test."""
        cached_repo.last_fetched_at(datetime.now() - timedelta(hours=2))
        recorder = Subprocesses()
        callers: List[int] = []

        def record_caller(args, **kwargs):
            callers.append(threading.get_ident())
            return recorder.run(args, **kwargs)

        monkeypatch.setattr(subprocess, "run", record_caller)
        with deadline(30.0, "the cache updater blocked instead of moving on"):
            assert dl.main(["--update-cache", "--force"]) == 0

        assert recorder.fetches == [BROAD_FETCH]
        assert set(callers) == {threading.get_ident()}

    def test_the_real_subprocess_run_is_back_once_a_test_ends(self):
        """Every test above replaced it; this one, which did not, must see the
        real thing — that is what "cannot outlive the test" means."""
        assert subprocess.run.__module__ == "subprocess"
