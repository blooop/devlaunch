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
"""

import fcntl
import os
import subprocess
import threading
from datetime import datetime, timedelta
from pathlib import Path
from typing import List, Optional
from unittest.mock import patch

import pytest

from devlaunch import dl
from devlaunch.worktree.config import get_worktree_config
from devlaunch.worktree.models import BaseRepository
from devlaunch.worktree.storage import MetadataStorage

BROAD_FETCH = ["git", "fetch", "origin", "+refs/heads/*:refs/heads/*", "--tags", "--prune"]


class Subprocesses:
    """Records every command line and answers it plausibly enough to continue.

    The child refreshes completions before it sweeps, so plenty of git and
    devpod runs pass through here that have nothing to do with fetching; the
    assertions look at `fetches` alone.
    """

    def __init__(self, fetch_fails_in: Optional[Path] = None) -> None:
        self.commands: List[List[str]] = []
        # A remote this recorder cannot reach, named by the bare repo it would
        # be fetched into — how `git fetch` reports an unreachable origin.
        self.fetch_fails_in = fetch_fails_in

    def run(self, args, **kwargs) -> subprocess.CompletedProcess:
        argv = [str(a) for a in args]
        self.commands.append(argv)
        cwd = kwargs.get("cwd")
        if (
            argv[:2] == ["git", "fetch"]
            and self.fetch_fails_in is not None
            and cwd is not None
            and Path(cwd) == self.fetch_fails_in
        ):
            raise subprocess.CalledProcessError(128, argv, stderr="no route to host")
        stdout = ""
        if argv[:1] == ["devpod"]:
            stdout = "{}" if argv[1:3] == ["context", "options"] else "[]"
        return subprocess.CompletedProcess(args=argv, returncode=0, stdout=stdout, stderr="")

    @property
    def fetches(self) -> List[List[str]]:
        """Every `git fetch`, whoever ran it."""
        return [c for c in self.commands if c[:2] == ["git", "fetch"]]


class CachedRepo:
    """One repo in the bare-clone cache, with a fetch clock the test sets."""

    def __init__(self, owner: str = "owner", repo: str = "repo") -> None:
        self.owner = owner
        self.repo = repo
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
                remote_url=f"https://github.com/{self.owner}/{self.repo}.git",
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


def run_updater(recorder: Subprocesses, timeout: float = 20.0) -> None:
    """Run the detached child's command line, failing rather than hanging.

    On a thread with a deadline because the behaviour under test includes *not
    blocking*: a sweep that queued behind a held lock would otherwise wedge the
    whole suite instead of failing one test.
    """
    outcome: List[int] = []

    def child() -> None:
        with patch("subprocess.run", side_effect=recorder.run):
            outcome.append(dl.main(["--update-cache", "--force"]))

    worker = threading.Thread(target=child, daemon=True)
    worker.start()
    worker.join(timeout)
    assert not worker.is_alive(), "the cache updater blocked instead of moving on"
    assert outcome == [0]


class TestTheUpdaterSweepsTheCache:
    """Freshness converges in the background, on the interval it always had."""

    def test_a_repo_past_its_interval_is_fetched(self, cached_repo):
        """The broad sweep the launch path used to pay for, run by the child."""
        cached_repo.last_fetched_at(datetime.now() - timedelta(hours=2))
        recorder = Subprocesses()

        run_updater(recorder)

        assert recorder.fetches == [BROAD_FETCH]

    def test_fetching_advances_the_shared_fetch_clock(self, cached_repo):
        """`last_fetched` is shared with the launch path, so a sweep is what
        stops a launch reaching for the same fetch a second time."""
        stale = datetime.now() - timedelta(hours=2)
        cached_repo.last_fetched_at(stale)

        run_updater(Subprocesses())

        assert cached_repo.last_fetched() > stale

    def test_a_repo_within_its_interval_is_left_alone(self, cached_repo):
        """The interval is the whole point: this is not a fetch-every-command."""
        cached_repo.last_fetched_at(datetime.now())
        recorder = Subprocesses()

        run_updater(recorder)

        assert recorder.fetches == []


class TestTheUpdaterYieldsToLaunches:
    """Background defers to foreground; never the other way round."""

    def test_a_repo_another_run_is_holding_is_skipped(self, cached_repo):
        """A launch holds the repo lock while it clones. The sweep must neither
        wait for it nor fetch behind its back — it just comes back next hour."""
        cached_repo.last_fetched_at(datetime.now() - timedelta(hours=2))
        recorder = Subprocesses()
        held = os.open(cached_repo.lock_path, os.O_RDWR | os.O_CREAT, 0o600)
        fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
        try:
            run_updater(recorder)
        finally:
            os.close(held)

        assert recorder.fetches == []

    def test_a_skipped_repo_keeps_its_fetch_clock(self, cached_repo):
        """Nothing was fetched, so nothing may claim it was: moving the clock
        would buy the contended repo another hour of staleness."""
        stale = datetime.now() - timedelta(hours=2)
        cached_repo.last_fetched_at(stale)
        held = os.open(cached_repo.lock_path, os.O_RDWR | os.O_CREAT, 0o600)
        fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
        try:
            run_updater(Subprocesses())
        finally:
            os.close(held)

        assert cached_repo.last_fetched() == stale


class TestTheSweepSurvivesABadRepo:
    """A detached child has nobody to complain to, so it complains to nobody."""

    def test_a_failing_fetch_does_not_stop_the_next_repo(self, cached_repo):
        """One unreachable remote must not cost every other repo its refresh."""
        first = cached_repo
        first.last_fetched_at(datetime.now() - timedelta(hours=2))
        second = CachedRepo(owner="owner", repo="other")
        second.last_fetched_at(datetime.now() - timedelta(hours=2))

        recorder = Subprocesses(fetch_fails_in=first.bare)

        run_updater(recorder)

        assert len(recorder.fetches) == 2
        assert second.last_fetched() > first.last_fetched()
