"""Making a directory genuinely refuse access, or skipping where it will not.

Several arms of the cache code are reached only by the filesystem saying no: a
rename the mount refuses, a directory the scan cannot read, a clone that cannot
be written. Testing them means arranging a real refusal, and ``chmod`` is the
obvious way to do it.

**A ``chmod`` that returns successfully is not a ``chmod`` that denies
anything.** The mode is stored and then ignored on Docker Desktop's and
Colima's bind mounts (9p, virtiofs, gRPC-FUSE), on some overlay and network
mounts, and for any process holding ``CAP_DAC_OVERRIDE`` — none of which a
``geteuid() == 0`` guard notices. Where that happens the operation under test
quietly succeeds, the assertion that it was refused fails, and a contributor
sees a red suite with no defect anywhere near it. The usual response to that is
to delete the test, which costs the coverage permanently.

So the refusal here is **verified, not assumed**: the mode is applied and then
the forbidden operation is actually attempted. If it goes through, this
filesystem does not do this and the test skips saying exactly that.
"""

import contextlib
from pathlib import Path

import pytest


@pytest.fixture
def refuses_access():
    """Yield ``deny(directory, mode)``: apply *mode*, prove it bites, restore later.

    Skips the calling test — with the directory and mode in the message — if the
    filesystem accepts the operation the mode was supposed to forbid.
    """
    restore = []

    def deny(directory: Path, mode: int = 0o500) -> None:
        restore.append((directory, directory.stat().st_mode & 0o777))
        directory.chmod(mode)

        if not mode & 0o200:
            probe = directory / ".probe-can-i-write"
            try:
                probe.touch()
            except OSError:
                pass
            else:
                probe.unlink()
                pytest.skip(
                    f"{directory} still accepts writes at mode {mode:o}; "
                    "this filesystem does not enforce directory permissions"
                )

        if not mode & 0o400:
            try:
                list(directory.iterdir())
            except OSError:
                pass
            else:
                pytest.skip(
                    f"{directory} is still readable at mode {mode:o}; "
                    "this filesystem does not enforce directory permissions"
                )

    yield deny

    # Innermost first, so a nested denial does not block restoring its parent.
    for directory, mode in reversed(restore):
        with contextlib.suppress(OSError):
            directory.chmod(mode)


@pytest.fixture
def refuses_writes(refuses_access):  # pylint: disable=redefined-outer-name
    """``deny(directory)`` for the common case: readable, listable, not writable."""
    return lambda directory: refuses_access(directory, 0o500)


@pytest.fixture
def refuses_reads(refuses_access):  # pylint: disable=redefined-outer-name
    """``deny(directory)`` for a directory that cannot even be listed."""
    return lambda directory: refuses_access(directory, 0o000)
