"""Inter-process locks for the shared cache.

Several dl processes can run at once — two agents launched on their own
branches, a completion refresh in the background — and they share one bare-clone
cache and one metadata.json. These locks are what keeps simultaneous runs from
racing each other over that state: without them, two first launches of a repo
both ran ``git clone --bare`` into the same path (and the loser's cleanup
deleted the winner's half-written clone), and metadata writers rewrote the file
from stale in-memory copies, dropping each other's records.

``flock`` rather than a pid file: the kernel releases it when the process dies,
however it dies, so a crashed dl never leaves the cache wedged.

Two deliberate limits, both load-bearing:

- **Not reentrant.** Acquiring a path twice in one process deadlocks (the second
  open file description blocks on the first). Call sites are structured so no
  lock is ever taken while the same lock is held. For the per-repo lock,
  ``RepositoryManager.hold_repo_lock`` is the one scope that takes it, and the
  steps running under it require the ``RepoLock`` token that scope mints. What
  the token buys is narrower than it looks: it is proof the lock **is** held, so
  a step that has one has no reason left to acquire the lock itself, and a step
  written without one cannot be called from inside the scope at all. It is not a
  guard against re-locking. ``ensure_repo`` is still public and still takes the
  lock, with nothing in its signature marking it unsafe in here, so a callee
  under the scope that reaches for it deadlocks exactly as it always would.
  Structuring the call sites remains what prevents that; the token is what makes
  the structure visible in the types instead of remembered.
- **The lock file is never deleted.** Unlinking an flock'd file is the classic
  self-defeating move: a process that opened the old inode still "holds" a lock
  nobody else can see, while new arrivals lock a fresh file and walk straight
  past it. A few empty ``.lock`` files in the cache are the price of the
  guarantee; ``dl --purge`` sweeps them away with everything else.

**Lock ordering is an invariant, not a habit.** Only one order between the
per-repo lock (``RepositoryManager.lock_path``) and the single metadata lock
(``MetadataStorage.exclusive``) is legal:

    the metadata lock may be taken while a repo lock is held; never the reverse.

Every site that writes metadata while holding a repo lock takes them in that
order, and a single site taking them the other way round would be enough to
deadlock two dl runs against each other with nothing looking wrong at either
site. There is a third lock — the per-workspace launch lock — but it is only
ever the outermost one, so it does not participate in the ordering above.

The enumeration of the sites deliberately lives in the code rather than here: a
list in a docstring goes stale the first time someone adds a writer, and a stale
list is worse than none because it is what the next reader trusts. Together with
the non-reentrancy above, the rule in full: no lock is taken while the same lock
is held, and repo always precedes metadata.
"""

import contextlib
import fcntl
import os
import sys
from pathlib import Path
from typing import Callable, Iterator, Optional

from .. import timing


@contextlib.contextmanager
def hold_lock(lock_path: Path, waiting_note: Optional[str] = None) -> Iterator[bool]:
    """Hold an exclusive inter-process lock on *lock_path* for the block.

    Blocks until the lock is free. When another process already holds it and
    *waiting_note* is given, one line is printed to stderr first, so a dl run
    that sits waiting on a sibling's long clone says why it is sitting.

    Yields whether the lock was **contended** — True when this process had to
    wait for another holder. Contention is information: a launch that waited
    knows the world may have changed while it did (a sibling may have brought
    the very workspace it wants up) and can re-check cheaply, where a launch
    that walked straight in knows its earlier reads still stand. Callers that
    only want the mutual exclusion ignore the value, which is what the
    pre-existing `with hold_lock(...):` sites do.
    """
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        waited = False
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            waited = True
            if waiting_note:
                print(f"dl: waiting for {waiting_note}", file=sys.stderr)
            # Only the blocking acquisition is spanned, not the holding: an
            # uncontended lock costs nothing and records nothing, and what the
            # summary should show is the time this process spent queued behind
            # a sibling rather than the time its own work then took.
            with timing.span("lock wait"):
                fcntl.flock(fd, fcntl.LOCK_EX)
        yield waited
    finally:
        # Closing the descriptor releases the lock; nothing is unlinked.
        os.close(fd)


def run_if_lock_free(lock_path: Path, work: Callable[[], None]) -> bool:
    """Run *work* holding *lock_path*, but only if the lock is free right now.

    A plain function taking the work rather than a context manager yielding
    "did I get it", because those are not the same guarantee. A block that runs
    either way needs a guard the caller can forget, and forgetting it does the
    protected work *unlocked* while reading exactly like the correct code. Here
    the not-acquired case has no body to run: the lock is either held for the
    whole of *work* or *work* never happens.

    Returns whether *work* ran. Ignoring that answer is safe — it reports what
    happened, it does not protect anything — so the only thing a caller can lose
    by dropping it is the ability to say "skipped".

    This is what background work uses and ``hold_lock`` is what foreground work
    uses, and the difference is who is waiting on whom. A launch that waits for
    a sibling's clone gets the clone; a sweep that waited for a launch would be
    taxing the very path it exists to keep clear.

    Note what this does **not** buy, because the asymmetry is easy to overstate:
    it makes the caller never queue, not the lock cheap to hold. Once *work* has
    started, this holds an ordinary exclusive lock, and anything taking the same
    path with ``hold_lock`` blocks for the whole of *work* — so background work
    still owes the foreground a bound on how long *work* can run. The guarantee
    in one line: **the caller never queues for anyone, and anyone may still
    queue for the caller.**

    Like ``hold_lock`` it is not reentrant and never unlinks the lock file — and
    a miss releases nothing, because there was nothing here to release.
    """
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return False
        work()
        return True
    finally:
        os.close(fd)
