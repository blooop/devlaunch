"""The pre-create hook detaches file mounts whose backing inode is gone.

Every file the claude-code feature bind-mounts one at a time -- and the
`known_hosts` file the consuming devcontainer mounts the same way -- is a file
its owner replaces *by rename*: Claude rewrites `.claude.json` on nearly every
host session, a token refresh rewrites `.credentials.json`, and ssh rewrites
`known_hosts` when a key rotates. A rename swaps the inode, and a bind mount
pins the old one, so inside any container created before the rename the mounted
path is now a mount of a **deleted** inode. The container itself keeps reading
it -- which is exactly why nothing notices -- but a Docker daemon *inside* that
container refuses it as a bind source: runc resolves the source, finds a mount
rooted on a deleted inode, and fails the create with `no such file or
directory`. That is devlaunch#326: `dl <repo>` inside the devlaunch
devcontainer cannot open any workspace whose devcontainer mounts one of these
files, minutes after the host has run Claude.

The hook is the one piece of this repo that runs host-side before every
container create -- including inside a container, where "host" is the container
about to nest another one -- so it is where the stale mount can be caught before
the nested daemon trips on it. The heal it promises: a path that is a mount of
a deleted inode is detached and rewritten as an ordinary file carrying the same
bytes and mode, so the nested bind has a real inode to pin.

The kernel is the only honest fixture for "a mount of a deleted inode", so
these tests build one: a private mount namespace (`unshare`), a real
`mount --bind`, and an `rm` of the source. The namespace comes unprivileged
where the kernel allows it and through passwordless `sudo` where it does not
(CI runners, this repo's own devcontainer); a host with neither -- a hardened
desktop kernel without passwordless root -- skips rather than fakes the mount,
because a fake would test string handling and not the behavior. Either way the
mounts are private to the namespace and die with it, so what a test asserts
from outside is what *persisted*: the underlying file's bytes and mode, plus a
mountinfo count the script wrote down while the namespace was still alive.

The heal must also be a coward. A *live* file mount -- root not deleted -- is
the container's working connection to the host file; detaching it would sever
every container this repo builds from its own configuration. The second test
holds the hook to leaving those alone.
"""

import stat
import subprocess

import pytest

from unit.test_claude_code_feature_mounts import (
    CONFIG_DIRNAME,
    FEATURE_DIR,
    READ_ONLY_HEADING,
    READ_WRITE_HEADING,
    documented_paths,
)

INIT_HOST = FEATURE_DIR / "init-host.sh"

PINNED_BYTES = '{"pinned": "bytes the mount kept after the host renamed over them"}'

# One namespace, one mount, one run of the shipped hook. `set -eu` so a step
# that cannot happen fails the test instead of preparing a different scenario,
# and the mountinfo probes are the precondition and the residue: the first
# proves the fixture really made a deleted-inode mount (root ends `//deleted`),
# the second writes down how many mounts still cover the path after the hook,
# because the namespace -- and every mount in it -- is gone once this exits.
STALE_MOUNT_SCENARIO = """
set -eu
home="$1"; target="$2"; hook="$3"
printf '%s' "$4" > "$home/replaced"
chmod 600 "$home/replaced"
: > "$target"
chmod 644 "$target"
mount --bind "$home/replaced" "$target"
rm "$home/replaced"
awk -v p="$target" '$5 == p && $4 ~ /\\/\\/deleted$/ { found = 1 } END { exit 1 - found }' \
    /proc/self/mountinfo
HOME="$home" sh -e "$hook"
awk -v p="$target" '$5 == p { n++ } END { print n + 0 }' /proc/self/mountinfo \
    > "$home/mounts-after"
chown -R "$(stat -c %u:%g "$home")" "$home" || :
"""

# The consumer also mounts the ssh agent *socket*, and a socket goes stale the
# same way -- the host's agent restarts and binds a fresh inode at the same
# path -- with the same consequence for a nested create. There are no bytes to
# carry over: the deleted inode is a dead endpoint whichever way it is mounted,
# so the whole heal is the detach. Staged with a FIFO rather than a socket
# because `mkfifo` is everywhere `sh` is and it is the *harsher* member of the
# class: a read on it blocks forever, so a heal that wrongly takes the
# copy-the-bytes path wedges here (and trips the runner's timeout) instead of
# merely erroring.
STALE_SPECIAL_MOUNT_SCENARIO = """
set -eu
home="$1"; target="$2"; hook="$3"
mkfifo "$home/replaced-fifo"
: > "$target"
mount --bind "$home/replaced-fifo" "$target"
rm "$home/replaced-fifo"
awk -v p="$target" '$5 == p && $4 ~ /\\/\\/deleted$/ { found = 1 } END { exit 1 - found }' \
    /proc/self/mountinfo
HOME="$home" sh -e "$hook"
awk -v p="$target" '$5 == p { n++ } END { print n + 0 }' /proc/self/mountinfo \
    > "$home/mounts-after"
chown -R "$(stat -c %u:%g "$home")" "$home" || :
"""

# The same stage with the source left in place: a live mount, the ordinary
# state of these paths inside every container this repo builds. The content
# is read back *through* the mount after the hook has run, because "the file
# still holds the bytes" would also be true of a heal that detached the mount
# and copied them -- the claim here is that the mount itself survived.
LIVE_MOUNT_SCENARIO = """
set -eu
home="$1"; target="$2"; hook="$3"
printf '%s' "$4" > "$home/live-source"
: > "$target"
mount --bind "$home/live-source" "$target"
HOME="$home" sh -e "$hook"
awk -v p="$target" '$5 == p { n++ } END { print n + 0 }' /proc/self/mountinfo \
    > "$home/mounts-after"
cat "$target" > "$home/content-through-mount"
chown -R "$(stat -c %u:%g "$home")" "$home" || :
"""


def namespace_command() -> list:
    """How this environment gets a private mount namespace, probed not assumed.

    Unprivileged first because it needs nothing granted; `sudo -n` second
    because the kernels that refuse the first (Ubuntu's AppArmor userns
    restriction) are common on the machines that run this suite. Empty means
    neither worked and the tests skip.
    """
    for prefix in (
        ["unshare", "--map-root-user", "--mount"],
        ["sudo", "-n", "unshare", "--mount"],
    ):
        probe = subprocess.run(
            [*prefix, "sh", "-c", 'mount --bind "$1" "$1"', "probe", "/etc/hostname"],
            capture_output=True,
            check=False,
        )
        if probe.returncode == 0:
            return prefix
    return []


NAMESPACE = namespace_command()

pytestmark = pytest.mark.skipif(
    not NAMESPACE,
    reason="no private mount namespace is available here, and only the kernel "
    "can stage a deleted-inode mount",
)


def in_namespace(script: str, *args: str) -> subprocess.CompletedProcess:
    # The timeout is an assertion, not a courtesy: a heal that tries to *read*
    # a stale FIFO or socket blocks forever, and initializeCommand hanging is a
    # worse failure than the one being healed. A wedged scenario must fail the
    # test rather than the suite.
    return subprocess.run(
        [*NAMESPACE, "sh", "-c", script, "scenario", *args],
        capture_output=True,
        text=True,
        check=False,
        timeout=60,
    )


def mounted_files() -> list:
    """Every file the feature mounts one at a time, plus the consumer's one.

    Derived from the README the way the config-protection tests derive theirs,
    so a file mount added to the feature is held to healing without anyone
    remembering to extend a list here. The trailing slash is the README's own
    declaration of directory-ness; directories cannot lose their inode to a
    rename and are not healed.
    """
    documented = documented_paths(READ_ONLY_HEADING) | documented_paths(READ_WRITE_HEADING)
    files = sorted(f"{CONFIG_DIRNAME}/{path}" for path in documented if not path.endswith("/"))
    assert files, "the README documents no file mounts"
    return files + [".ssh/known_hosts"]


@pytest.fixture(name="scratch_home")
def scratch_home_fixture(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    return home


@pytest.mark.parametrize("relative", mounted_files())
def test_the_hook_replaces_a_deleted_inode_mount_with_the_bytes_it_pinned(scratch_home, relative):
    target = scratch_home / relative
    target.parent.mkdir(parents=True, exist_ok=True)

    result = in_namespace(
        STALE_MOUNT_SCENARIO, str(scratch_home), str(target), str(INIT_HOST), PINNED_BYTES
    )
    assert result.returncode == 0, f"the scenario could not run: {result.stdout}\n{result.stderr}"

    mounts_after = (scratch_home / "mounts-after").read_text().strip()
    assert mounts_after == "0", f"{relative} is still covered by {mounts_after} mount(s)"
    assert target.read_text() == PINNED_BYTES, f"the heal lost {relative}'s bytes"
    assert stat.S_IMODE(target.stat().st_mode) == 0o600, (
        f"the heal did not carry over {relative}'s mode"
    )


def test_the_hook_detaches_a_deleted_inode_mount_of_the_agent_socket(scratch_home):
    """The agent socket mount goes stale like the files do, and blocked #326 too.

    No bytes survive -- a deleted socket inode is a dead endpoint -- so the
    claim is narrower than the file heal's: the mount is gone, the launch was
    not aborted, and the hook never tried to read it (the FIFO would have hung
    the scenario into the runner's timeout).
    """
    target = scratch_home / ".ssh" / "agent.sock"
    target.parent.mkdir(parents=True, exist_ok=True)

    result = in_namespace(
        STALE_SPECIAL_MOUNT_SCENARIO, str(scratch_home), str(target), str(INIT_HOST)
    )
    assert result.returncode == 0, f"the scenario could not run: {result.stdout}\n{result.stderr}"

    mounts_after = (scratch_home / "mounts-after").read_text().strip()
    assert mounts_after == "0", f"agent.sock is still covered by {mounts_after} mount(s)"


def test_the_hook_leaves_a_live_mount_connected(scratch_home):
    """A mount whose backing file still exists is a working one -- hands off."""
    relative = f"{CONFIG_DIRNAME}/.claude.json"
    target = scratch_home / relative
    target.parent.mkdir(parents=True, exist_ok=True)

    result = in_namespace(
        LIVE_MOUNT_SCENARIO, str(scratch_home), str(target), str(INIT_HOST), PINNED_BYTES
    )
    assert result.returncode == 0, f"the scenario could not run: {result.stdout}\n{result.stderr}"

    assert (scratch_home / "mounts-after").read_text().strip() == "1", (
        "the hook detached a mount whose backing file the host still has"
    )
    assert (scratch_home / "content-through-mount").read_text() == PINNED_BYTES
    assert (scratch_home / "live-source").exists(), "the hook removed the mount's source"
