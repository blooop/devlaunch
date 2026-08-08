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
  lock is ever taken while the same lock is held — see the acquisition comments
  at each site.
- **The lock file is never deleted.** Unlinking an flock'd file is the classic
  self-defeating move: a process that opened the old inode still "holds" a lock
  nobody else can see, while new arrivals lock a fresh file and walk straight
  past it. A few empty ``.lock`` files in the cache are the price of the
  guarantee; ``dl --purge`` sweeps them away with everything else.
"""

import contextlib
import fcntl
import os
import sys
from pathlib import Path
from typing import Iterator, Optional


@contextlib.contextmanager
def hold_lock(lock_path: Path, waiting_note: Optional[str] = None) -> Iterator[None]:
    """Hold an exclusive inter-process lock on *lock_path* for the block.

    Blocks until the lock is free. When another process already holds it and
    *waiting_note* is given, one line is printed to stderr first, so a dl run
    that sits waiting on a sibling's long clone says why it is sitting.
    """
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            if waiting_note:
                print(f"dl: waiting for {waiting_note}", file=sys.stderr)
            fcntl.flock(fd, fcntl.LOCK_EX)
        yield
    finally:
        # Closing the descriptor releases the lock; nothing is unlinked.
        os.close(fd)
