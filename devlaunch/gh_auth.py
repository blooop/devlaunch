"""Carry the host's GitHub CLI credentials into every workspace devlaunch opens.

devpod forwards the ssh agent and a git credential helper, but nothing carries
`gh` authentication, so `gh` starts out logged out in every container — including
the one devlaunch itself is developed in. A devcontainer.json can bind-mount
~/.config/gh, but that only helps the projects that opted in, the mount target
has to name the container user's home directory, and it hands over nothing at
all when the host keeps its token in a keyring instead of hosts.yml.

A token in the environment needs no cooperation from the image, the
devcontainer.json, or the container user, so it works for whatever devlaunch is
asked to launch. `gh` reads GH_TOKEN ahead of its own config, and `gh auth
token` sources the token whether the host stores it in a file or a keyring.

The token reaches devpod out of band — through a private file for `devpod up`
and through devpod's own environment for `devpod ssh` — so it never sits in a
command line that `ps` shows to every other user on the host.
"""

import contextlib
import functools
import logging
import os
import re
import shutil
import subprocess
import tempfile
from typing import Dict, Iterator, List, Optional, Tuple

from devlaunch import timing
from devlaunch.xdg import config_home

# The variable set inside the container. gh consults it before its config file.
TOKEN_VAR = "GH_TOKEN"

# Host variables to reuse before paying for a `gh` subprocess. Honouring
# GH_TOKEN also means a devlaunch running inside a devlaunch workspace passes
# its own forwarded token further down.
HOST_TOKEN_VARS = ("GH_TOKEN", "GITHUB_TOKEN")

# Set this to opt a machine out of forwarding entirely.
DISABLE_VAR = "DEVLAUNCH_NO_GH_TOKEN"

_FALSEY = ("", "0", "false", "no")

# Every GitHub token form is a flat ASCII string; anything else came from a
# broken gh install or a wrapper script that printed a message on stdout.
_TOKEN_PATTERN = re.compile(r"\A[A-Za-z0-9_.\-]+\Z")

# gh may have to unlock a keyring, so don't let it stall a workspace forever.
_GH_TIMEOUT_SECONDS = 10


def forwarding_disabled() -> bool:
    """Whether the user opted this machine out of gh token forwarding."""
    return os.environ.get(DISABLE_VAR, "").strip().lower() not in _FALSEY


def _is_token(value: str) -> bool:
    return bool(value) and bool(_TOKEN_PATTERN.match(value))


def _token_from_gh_cli() -> Optional[str]:
    """Ask the gh CLI for the host's token, or None if it has none to give."""
    if not shutil.which("gh"):
        return None
    try:
        # Host prep wherever it lands: the token is the host's to produce, and
        # the trip is charged to that owner even when it happens in the middle
        # of the attach that needed it.
        with timing.stage("host-prep"), timing.span("gh auth token"):
            # nosec B603 B607 - list form, not shell=True; no command injection risk
            result = subprocess.run(
                ["gh", "auth", "token"],
                capture_output=True,
                text=True,
                check=False,
                # gh must not eat stdin that belongs to the command `dl` was asked
                # to run, and must not leave the terminal in a state of its own.
                stdin=subprocess.DEVNULL,
                timeout=_GH_TIMEOUT_SECONDS,
            )
    except (OSError, subprocess.SubprocessError) as e:
        logging.warning(
            "Could not read a GitHub token from gh (%s), so this workspace opens "
            "without a GitHub login.",
            e,
        )
        return None
    if result.returncode != 0:
        # Name the config dir: a run that scoped XDG_CONFIG_HOME to a scratch
        # directory hides the host's gh login, so gh refuses even though the user
        # is logged in, and `gh auth login` is exactly the wrong remedy for it.
        logging.warning(
            "gh auth token exited %s, so this workspace opens without a GitHub login. "
            "gh read its config from %s -- if you are logged in on this host, that "
            "directory is the thing to check before `gh auth login`.",
            result.returncode,
            config_home(),
        )
        return None
    # Never log the value itself, only whether it was usable: what gh printed may
    # be a malformed credential, and a warning is not a place to put one.
    if not _is_token(result.stdout.strip()):
        logging.warning(
            "gh auth token printed something that is not a token, so this workspace "
            "opens without a GitHub login."
        )
        return None
    return result.stdout.strip()


@functools.lru_cache(maxsize=1)
def resolve_token() -> Optional[str]:
    """The host's GitHub token, or None if there is nothing to forward.

    Cached for the life of the process: a single `dl` run can hand the token to
    both `devpod up` and `devpod ssh`, and asking gh twice can mean unlocking a
    keyring twice.
    """
    if forwarding_disabled():
        return None
    for var in HOST_TOKEN_VARS:
        token = os.environ.get(var, "").strip()
        if _is_token(token):
            return token
    return _token_from_gh_cli()


def _stage_token_file(token: str) -> Optional[str]:
    """Write the token to a file only this user can read, or None if that failed.

    Forwarding a credential is a convenience, so a temp dir that is full or
    read-only has to cost the workspace its gh login, not its launch.
    """
    try:
        fd, path = tempfile.mkstemp(prefix="devlaunch-gh-", suffix=".env")
    except OSError as e:
        logging.warning(
            "Could not create a file to pass the GitHub token to devpod (%s), so this "
            "workspace opens without a GitHub login.",
            e,
        )
        return None
    try:
        with os.fdopen(fd, "w") as handle:
            handle.write(f"{TOKEN_VAR}={token}\n")
    except OSError as e:
        logging.warning(
            "Could not write the GitHub token for devpod (%s), so this workspace "
            "opens without a GitHub login.",
            e,
        )
        with contextlib.suppress(OSError):
            os.unlink(path)
        return None
    return path


@contextlib.contextmanager
def up_args() -> Iterator[List[str]]:
    """Yield `devpod up` flags that put the host's token in the workspace env.

    The token goes in a private file rather than on the command line because
    `devpod up` can run for minutes while an image builds, and its argv is
    readable by every user on the host for that whole time. --workspace-env-file
    is a devpod flag of its own, so it adds to whatever the user has configured
    through --workspace-env instead of displacing it.

    devpod re-applies the workspace env on every `up`, so a token that has since
    changed on the host reaches even a container that is already running. It
    only reaches one this way, though: see ssh_args_and_env.
    """
    token = resolve_token()
    path = _stage_token_file(token) if token else None
    if not path:
        yield []
        return
    try:
        yield ["--workspace-env-file", path]
    finally:
        with contextlib.suppress(OSError):
            os.unlink(path)


def ssh_args_and_env() -> Tuple[List[str], Optional[Dict[str, str]]]:
    """Return `devpod ssh` flags plus the environment devpod must be run with.

    This covers attaching to a workspace that is already running, which skips
    `devpod up` and its workspace env entirely. --send-env only names the
    variable — devpod reads the value from its own environment — so the token
    stays out of argv here too.

    devpod lets a workspace env value win over --send-env, so this tops up
    workspaces devlaunch never created rather than overriding a token that
    `devpod up` just delivered. The flip side is that it cannot refresh one
    either: a running workspace whose token has been revoked since it started
    needs `dl <ws> restart` to pick up the new one.
    """
    token = resolve_token()
    if not token:
        return [], None
    return ["--send-env", TOKEN_VAR], {**os.environ, TOKEN_VAR: token}


def openssh_env_names_and_env() -> Tuple[List[str], Optional[Dict[str, str]]]:
    """The same forwarding, for the OpenSSH transport that carries a terminal.

    Interactive payloads reach the workspace through `ssh` rather than `devpod
    ssh` (see tty_session), which spells the same idea `-o SendEnv=NAME`. Only
    the names are returned here; tty_session turns them into flags, and the
    values travel in the environment for the same reason as above.
    """
    token = resolve_token()
    if not token:
        return [], None
    return [TOKEN_VAR], {**os.environ, TOKEN_VAR: token}
