"""
dl - DevLaunch CLI

A streamlined CLI for devpod with intuitive autocomplete and fzf fuzzy selection.
Provides an renv-like UX for managing devcontainer workspaces.

Usage:
    dl                           # fzf selector for existing workspaces
    dl <workspace>               # open/create workspace, attach shell
    dl <workspace> <command>     # run command in workspace
    dl owner/repo                # create from git repo (github.com)
    dl owner/repo@branch         # specific branch
    dl ./path                    # create from local path
    dl --ls                      # list workspaces
    dl --stop <workspace>        # stop workspace
    dl --rm <workspace>          # delete workspace
    dl --code <workspace>        # open in VS Code
    dl --install                 # install completions
"""

import sys
import subprocess
import json
import logging
import os
import pathlib
import re
import shlex
import time
from importlib.metadata import version as pkg_version, PackageNotFoundError, distribution
from typing import List, Optional, Dict, Any
from dataclasses import dataclass
from urllib.parse import urlparse
from urllib.request import url2pathname

from . import devpod_ssh, gh_auth, tools
from .completion import install_completions
from .workspace_id import TARGET_LENGTH, WorkspaceId, slug, source_workspace_id, validate_ref_name
from .worktree.config import get_worktree_config
from .worktree.migration import migrate_cache
from .worktree.workspace_clone import WorkspaceCloneManager


class DevpodNotInstalled(Exception):
    """The devpod binary dl shells out to is not on PATH.

    Deliberately not an OSError (FileNotFoundError is one) and not a
    RuntimeError: dl catches both broadly in a dozen places so that a flaky
    command degrades to an empty list or a "failed to prepare workspace"
    message. A missing binary reported through one of those handlers is
    reported wrongly, so it travels as a type nothing between run_devpod and
    main() catches, and main() is the only place that handles it.
    """


# One line, so a completion helper that trips over it cannot spew into the
# user's shell. It names both install routes because devpod ships with the
# pixi/conda package and does not ship with the pip one (see README).
DEVPOD_MISSING_MESSAGE = (
    "devpod not found on PATH: dl cannot manage workspaces without it. "
    "Install devpod from https://devpod.sh/docs/getting-started/install "
    "(pixi/conda installs of devlaunch include it; pip installs do not)."
)

# The shell's own "command not found" code, which says more than a bare 1 and
# cannot be confused with a devpod command that ran and failed.
DEVPOD_MISSING_EXIT_CODE = 127


def _install_provenance() -> Optional[str]:
    """Describe the install this dl was launched from, or None if unremarkable.

    The released build and the editable dev install report the same version, so
    the version alone cannot say which one just ran. PEP 610 records how a dist
    was installed in ``direct_url.json``: ``dir_info.editable`` is true only for
    an editable install, and ``url`` is the tree it resolves to. That is read
    from the dist's own metadata rather than inferred from where the files sit,
    so no path is stat'd and no install location is pattern-matched.

    Everything here is best-effort: a dist with no direct-url metadata, or with
    metadata that does not parse or does not carry the keys, is simply not
    described. --version must never fail over provenance, so this returns None
    instead of raising - an ambiguous version beats a broken one.
    """
    try:
        raw = distribution("devlaunch").read_text("direct_url.json")
        if not raw:
            return None
        info = json.loads(raw)
        dir_info = info.get("dir_info") if isinstance(info, dict) else None
        if not isinstance(dir_info, dict) or not dir_info.get("editable"):
            return None
        url = info.get("url")
        if not isinstance(url, str):
            return None
        parsed = urlparse(url)
        if parsed.scheme != "file":
            return None
        tree = url2pathname(parsed.path)
        if not tree:
            return None
        return f"dev, editable from {tree}"
    except Exception:
        return None


def get_version() -> str:
    """Get the package version, noting the install it came from when notable."""
    try:
        base = pkg_version("devlaunch")
    except PackageNotFoundError:
        return "unknown"
    provenance = _install_provenance()
    return f"{base} ({provenance})" if provenance else base


logging.basicConfig(level=logging.INFO, format="%(message)s")


def _get_cache_dir() -> pathlib.Path:
    """Get the cache directory, honoring XDG_CACHE_HOME."""
    xdg_cache = os.environ.get("XDG_CACHE_HOME")
    if xdg_cache:
        return pathlib.Path(xdg_cache) / "devlaunch"
    return pathlib.Path.home() / ".cache" / "devlaunch"


# Cache configuration (honors XDG_CACHE_HOME)
CACHE_DIR = _get_cache_dir()
CACHE_FILE = CACHE_DIR / "completions.json"
BASH_CACHE_FILE = CACHE_DIR / "completions.bash"

# How long a completion cache is considered current. Refreshing it means a
# `git ls-remote` per known repo, so doing it on every invocation costs seconds
# of machine for a list of names that barely moves. An hour mirrors
# WorktreeConfig.fetch_interval, which already decides how stale devlaunch is
# willing to let its view of a repo's branches get; branch names are exactly
# what the expensive part of this refresh collects, so the two should agree.
# `dl --refresh` remains the escape hatch when a user wants it now.
COMPLETION_CACHE_TTL_SECONDS = 3600


def get_cache_path() -> pathlib.Path:
    """Get the path to the completion cache file."""
    return CACHE_FILE


def read_completion_cache() -> Optional[Dict[str, Any]]:
    """Read completion data from cache file."""
    cache_path = get_cache_path()
    if not cache_path.exists():
        return None
    try:
        with open(cache_path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, json.JSONDecodeError):
        return None


def write_completion_cache(data: Dict[str, Any]) -> None:
    """Write completion data to cache file (JSON format)."""
    cache_path = get_cache_path()
    try:
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        # Write to temp file first, then atomic rename
        temp_path = cache_path.with_suffix(".tmp")
        with open(temp_path, "w", encoding="utf-8") as f:
            json.dump(data, f)
        # Atomic rename (on POSIX systems)
        temp_path.replace(cache_path)
    except OSError:
        pass


def write_bash_completion_cache(data: Dict[str, Any]) -> None:
    """Write completion data as a sourceable bash file."""
    try:
        BASH_CACHE_FILE.parent.mkdir(parents=True, exist_ok=True)
        workspaces = " ".join(data.get("workspaces", []))
        repos = " ".join(data.get("repos", []))
        owners = " ".join(data.get("owners", []))
        branches = " ".join(data.get("branches", []))
        lines = [
            "# Auto-generated by dl - do not edit",
            f'DL_WORKSPACES="{workspaces}"',
            f'DL_REPOS="{repos}"',
            f'DL_OWNERS="{owners}"',
            f'DL_BRANCHES="{branches}"',
        ]
        # Write to temp file first, then atomic rename
        temp_path = BASH_CACHE_FILE.with_suffix(".tmp")
        with open(temp_path, "w", encoding="utf-8") as f:
            f.write("\n".join(lines) + "\n")
        # Atomic rename (on POSIX systems)
        temp_path.replace(BASH_CACHE_FILE)
    except OSError:
        pass


def get_remote_branches(owner_repo: str) -> List[str]:
    """Get list of branches from a remote GitHub repository."""
    url = f"git@github.com:{owner_repo}.git"
    try:
        result = subprocess.run(
            ["git", "ls-remote", "--heads", url],
            capture_output=True,
            text=True,
            check=False,
            timeout=5,  # Don't hang on slow connections
        )
        if result.returncode == 0:
            branches = []
            for line in result.stdout.strip().split("\n"):
                if line and "refs/heads/" in line:
                    branch = line.split("refs/heads/")[-1]
                    branches.append(branch)
            return branches
    except (OSError, subprocess.SubprocessError, subprocess.TimeoutExpired):
        pass
    return []


def get_local_branches(owner_repo: str) -> List[str]:
    """Get list of local branches from the bare repo cache.

    Discovers branches that exist locally but may not yet be on the remote
    (e.g. branches created via ``dl owner/repo@new-branch``).
    """
    owner, repo = owner_repo.split("/", 1)
    repos_dir = pathlib.Path(get_worktree_config().repos_dir)
    bare_path = repos_dir / owner / repo / ".bare"
    if not bare_path.is_dir():
        return []
    try:
        result = subprocess.run(
            ["git", "for-each-ref", "--format=%(refname:short)", "refs/heads/"],
            cwd=bare_path,
            capture_output=True,
            text=True,
            check=False,
            timeout=2,
        )
        if result.returncode == 0:
            return [b for b in result.stdout.strip().split("\n") if b]
    except (OSError, subprocess.SubprocessError, subprocess.TimeoutExpired):
        pass
    return []


def update_completion_cache() -> Dict[str, Any]:
    """Update the completion cache with current data."""
    workspaces = list_workspaces()
    workspace_ids = [ws.id for ws in workspaces]
    repos = discover_repos_from_workspaces(workspaces)

    # Merge repos discovered from the local cache directory so that repos
    # cloned locally (even without a devpod workspace yet) appear in completions
    for owner, repo_list in discover_repos_from_cache_dir().items():
        if owner not in repos:
            repos[owner] = []
        for repo in repo_list:
            if repo not in repos[owner]:
                repos[owner].append(repo)

    # Flatten repos to list of owner/repo strings
    known_repos = []
    for owner, repo_list in sorted(repos.items()):
        for repo in sorted(repo_list):
            known_repos.append(f"{owner}/{repo}")

    # Extract unique owners
    owners = sorted(repos.keys())

    # Fetch branches for all known repos (as owner/repo@branch strings)
    # Merge remote and local branches so locally-created branches also complete
    all_branches = []
    for owner_repo in known_repos:
        remote = set(get_remote_branches(owner_repo))
        local = set(get_local_branches(owner_repo))
        for branch in sorted(remote | local):
            all_branches.append(f"{owner_repo}@{branch}")

    data = {
        "workspaces": workspace_ids,
        "repos": known_repos,
        "owners": owners,
        "branches": all_branches,
    }
    write_completion_cache(data)
    write_bash_completion_cache(data)
    return data


def completion_cache_age_seconds() -> Optional[float]:
    """Seconds since the completion cache was last written, or None if there is none.

    The timestamp is the cache file's own mtime rather than a field inside it:
    write_completion_cache renames the finished file into place, so mtime marks a
    *completed* refresh; caches written by earlier versions carry no timestamp
    field and would otherwise read as infinitely stale; and a stat() keeps the
    check free on the path whose entire purpose is to do no work.
    """
    try:
        return max(0.0, time.time() - get_cache_path().stat().st_mtime)
    except OSError:
        return None


def completion_cache_is_fresh() -> bool:
    """Whether the completion cache is new enough to leave alone."""
    age = completion_cache_age_seconds()
    return age is not None and age < COMPLETION_CACHE_TTL_SECONDS


# One background refresh per process, at most. A dl process is one command the
# user typed and exits when that command is done, so per-process state is
# per-invocation state -- there is no long-running process here for the latch to
# go stale in. Held in a dict so it can be mutated without a `global`.
_refresh_state: Dict[str, bool] = {"spawned": False}


def cache_refresh_spawned() -> bool:
    """Whether this process has already spawned a background cache refresh."""
    return _refresh_state["spawned"]


def reset_cache_refresh_state() -> None:
    """Forget that a refresh was spawned, so another one may be (tests)."""
    _refresh_state["spawned"] = False


def update_cache_background(force: bool = False) -> None:
    """Refresh the completion cache in a detached process, if it is worth it.

    Skipped entirely when this process already spawned one, and when the cache is
    still fresh. ``force`` is for callers that have just changed what the cache
    describes -- a workspace created, stopped or deleted -- where the cache is
    wrong no matter how recently it was written. A freshness skip deliberately
    does not consume the one spawn: it means "not needed yet", not "already
    done", so a later forced call can still get its refresh.
    """
    if _refresh_state["spawned"]:
        return
    if not force and completion_cache_is_fresh():
        return
    _refresh_state["spawned"] = True
    cmd = [sys.executable, "-m", "devlaunch.dl", "--update-cache"]
    if force:
        # Tell the child its refresh is not subject to the TTL either.
        cmd.append("--force")
    try:
        # pylint: disable=consider-using-with
        subprocess.Popen(
            cmd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
    except OSError:
        pass


def purge_all_data() -> int:
    """Purge all devlaunch data including DevPod workspaces and caches.

    This:
    1. Deletes all DevPod workspaces
    2. Removes ~/.cache/devlaunch/ which contains:
       - completions.json, completions.bash (completion caches)
    """
    import shutil

    cache_dir = _get_cache_dir()

    # First, delete all DevPod workspaces. The list is the same snapshot the
    # caller printed the count from, so the confirmation the user answered and
    # the set actually deleted cannot disagree.
    workspaces = list_workspaces()
    for ws in workspaces:
        print(f"Deleting DevPod workspace: {ws.id}")
        result = run_devpod(["delete", ws.id, "--force"], capture=True)
        if result.returncode != 0:
            logging.warning(f"Failed to delete workspace {ws.id}: {result.stderr}")
    if workspaces:
        invalidate_workspace_list_cache()

    # Then remove local cache
    if not cache_dir.exists():
        if not workspaces:
            print("No data to purge.")
        return 0

    try:
        shutil.rmtree(cache_dir)
        print(f"Removed: {cache_dir}")
        return 0
    except OSError as e:
        print(f"Error removing {cache_dir}: {e}")
        return 1


# Regex to match owner/repo[@branch] format (not a path, not already a URL)

OWNER_REPO_PATTERN = re.compile(r"^[a-zA-Z0-9_.-]+/[a-zA-Z0-9_.-]+(@[a-zA-Z0-9_./%-]+)?$")


def parse_owner_repo_branch(spec: str) -> Optional[tuple]:
    """Parse owner/repo[@branch] spec into (owner_repo, branch) tuple.

    Returns None if spec doesn't match owner/repo format.
    Branch may be None if not specified.
    """
    # Skip paths and full URLs
    if is_path_spec(spec) or "://" in spec:
        return None
    if spec.startswith("github.com/") or spec.startswith("gitlab.com/"):
        return None

    if not OWNER_REPO_PATTERN.match(spec):
        return None

    if "@" in spec:
        owner_repo, branch = spec.split("@", 1)
        return (owner_repo, branch)
    return (spec, None)


def _resolve_devcontainer_ref(ref: str) -> str:
    """Turn a --devcontainer value into a devcontainer.json path for devpod.

    A bare name expands to the spec's one-level-deep variant location,
    `.devcontainer/<name>/devcontainer.json`. Anything containing a separator or
    ending in .json is used as given.

    devpod's own --devcontainer-id takes a bare variant name and looks like the same
    job, but it is silently ignored in devpod 0.26.1: a fresh `devpod up --id x
    --devcontainer-id alt` parses .devcontainer/devcontainer.json and stores no
    devContainerID, while --devcontainer-path selects the variant correctly. Build
    the path here until that is fixed upstream.

    Raises ValueError on a value that cannot be a path, so a missing argument
    (`dl --devcontainer --help`) is an error instead of a nonsense path.
    """
    if not ref or ref.isspace():
        raise ValueError("--devcontainer requires a variant name or path")
    if ref.startswith("-"):
        raise ValueError(f"--devcontainer needs a value, got the flag {ref!r}")
    if "/" in ref or ref.endswith(".json"):
        return ref
    return f".devcontainer/{ref}/devcontainer.json"


# dl options whose value is a separate argument. aid splits its own command line
# before handing it to dl and has to tell such a value from the workspace spec,
# so the list lives here, next to the parsing it describes.
DL_VALUE_OPTIONS = frozenset({"--devcontainer"})


def extract_devcontainer_flag(args: List[str]) -> tuple[List[str], Optional[str]]:
    """Pull `--devcontainer <name-or-path>` out of the argument list.

    Returns (remaining_args, devcontainer_path). Accepts both
    `--devcontainer x` and `--devcontainer=x`.

    Scanning stops at the first bare `--`: everything after it is the shell
    command `dl <ws> -- <command>` runs inside the workspace, and must reach it
    verbatim even when it has flags of its own by the same name.

    Raises ValueError if the flag is given without a usable value.
    """
    remaining: List[str] = []
    selection: Optional[str] = None
    i = 0
    while i < len(args):
        arg = args[i]
        if arg == "--":
            remaining.extend(args[i:])
            break
        if arg.startswith("--devcontainer="):
            selection = _resolve_devcontainer_ref(arg.split("=", 1)[1])
            i += 1
            continue
        if arg == "--devcontainer":
            if i + 1 >= len(args):
                raise ValueError("--devcontainer requires a variant name or path")
            selection = _resolve_devcontainer_ref(args[i + 1])
            i += 2
            continue
        remaining.append(arg)
        i += 1
    return remaining, selection


def is_path_spec(spec: str) -> bool:
    """Check if spec looks like a filesystem path."""
    return spec.startswith("./") or spec.startswith("/") or spec.startswith("~")


def is_git_spec(spec: str) -> bool:
    """Check if spec looks like a git repo (owner/repo or URL)."""
    # Paths are not git specs
    if is_path_spec(spec):
        return False
    if "://" in spec:
        return True
    if spec.startswith("github.com/") or spec.startswith("gitlab.com/"):
        return True
    return bool(OWNER_REPO_PATTERN.match(spec))


def expand_workspace_spec(spec: str) -> str:
    """Expand owner/repo[@branch] to git@github.com:owner/repo.git[@branch] for devpod."""
    # Don't expand if it's a path
    if is_path_spec(spec):
        return spec
    # Don't expand if it already looks like a URL
    if "://" in spec or spec.startswith("github.com/") or spec.startswith("gitlab.com/"):
        return spec
    # Don't expand if it's already an SSH URL (more specific pattern: user@host:path)
    # Matches patterns like git@github.com:, git@gitlab.com:, etc.
    if re.match(r"^[^@]+@[^:]+:", spec):
        return spec
    # Check if it matches owner/repo[@branch] pattern - use SSH URL format for GitHub
    if OWNER_REPO_PATTERN.match(spec):
        if "@" in spec:
            owner_repo, branch = spec.split("@", 1)
            return f"git@github.com:{owner_repo}.git@{branch}"
        return f"git@github.com:{spec}.git"
    # Otherwise return as-is (existing workspace name)
    return spec


def setup_hostname(workspace_id: str) -> bool:
    """Set the container hostname so the terminal prompt shows the project/branch.

    The hostname appears in the bash prompt (user@hostname:path$), giving users
    a clear indicator of which project and branch they're in.

    Best-effort: silently ignores failures (e.g. if sudo is unavailable via
    devpod ssh). The workspace path already contains the workspace ID.

    Args:
        workspace_id: The DevPod workspace ID (e.g. 'bencher-ws5')

    Returns:
        True if hostname was set successfully, False otherwise
    """
    result = run_devpod(
        ["ssh", workspace_id, "--command", f"sudo hostname {workspace_id}"],
        capture=True,
    )
    return result.returncode == 0


def spec_to_workspace_id(spec: str) -> str:
    """Derive the workspace ID for a given spec.

    For owner/repo@branch this is `WorkspaceId(owner, repo, branch).value` — see
    devlaunch.workspace_id for the format. The other spec shapes are not
    (owner, repo, ref) triples, so they cannot be parsed into one:

    - For owner/repo with branch: <repo-slug>-<branch-slug>-<syllables>
    - For owner/repo without branch: <repo-slug>. Not a workspace identity: a
      workspace is a branch checkout, so every caller that creates one resolves
      the default branch first and passes it. This is only a repo label.
    - For other git URLs: the slugged URL, as devpod would name it
    - For paths: the directory name (e.g., ./my-project -> my-project)
    - For existing IDs: the ID as-is

    Raises:
        ValueError: if the spec names an owner, repo or branch that is not a safe
            git name.
    """
    # Check for @branch suffix
    if "@" in spec:
        base_spec, _ = spec.split("@", 1)
    else:
        base_spec = spec

    # For paths, use the directory name
    if is_path_spec(base_spec):
        return pathlib.Path(base_spec).expanduser().resolve().name

    # For git URLs or owner/repo
    if is_git_spec(base_spec):
        parsed = parse_owner_repo_branch(spec)
        if parsed:
            owner_repo, parsed_branch = parsed
            owner, repo_name = owner_repo.split("/", 1)
            if parsed_branch:
                return WorkspaceId(owner, repo_name, parsed_branch).value
            # A repo label, not an identity — see the docstring. Capped anyway, so no
            # caller can get a string from here that overflows devpod's limit.
            return slug(repo_name)[:TARGET_LENGTH].strip("-")

        # Fallback for non-owner/repo git URLs (github.com/..., https://...)
        full_source = expand_workspace_spec(base_spec)
        # Strip protocol prefix if present
        if "://" in full_source:
            full_source = full_source.split("://", 1)[1]
        # Strip SSH URL prefix (user@host:) if present
        ssh_match = re.match(r"^[^@]+@([^:]+):(.*)", full_source)
        if ssh_match:
            full_source = f"{ssh_match.group(1)}/{ssh_match.group(2)}"
        # Strip .git suffix if present
        if full_source.endswith(".git"):
            full_source = full_source[:-4]
        # Same scheme as a triple: slug for legibility, suffix for identity, capped.
        # The old rule here deleted `_` while the owner/repo path turned it into `-`,
        # so one repo derived two ids; applying only the slug rule instead swapped
        # that for a collision, since `my_repo`, `my-repo` and `my.repo` slug alike.
        return source_workspace_id(full_source)

    # Otherwise assume it's already a workspace ID
    return spec


def validate_workspace_spec(spec: str, existing_ids: List[str]) -> Optional[str]:
    """Validate workspace spec and return error message if invalid."""
    # Valid if it's an existing workspace
    if spec in existing_ids:
        return None
    # Valid if it's a path
    if is_path_spec(spec):
        return None
    # Valid if it's a git spec (owner/repo or URL)
    if is_git_spec(spec):
        return None
    # Invalid - provide helpful error
    return f"Unknown workspace '{spec}'. Use 'dl --ls' to list workspaces, or specify owner/repo or ./path"


@dataclass
class Workspace:
    """Represents a devpod workspace."""

    id: str
    source_type: str  # "local" or "git"
    source: str
    last_used: str
    provider: str
    ide: str

    @classmethod
    def from_json(cls, data: Dict[str, Any]) -> "Workspace":
        """Parse workspace from devpod JSON output."""
        source = data.get("source", {})
        if "localFolder" in source:
            source_type = "local"
            source_path = source["localFolder"]
        elif "gitRepository" in source:
            source_type = "git"
            source_path = source["gitRepository"]
        else:
            source_type = "unknown"
            source_path = str(source)

        return cls(
            id=data.get("id", ""),
            source_type=source_type,
            source=source_path,
            last_used=data.get("lastUsed", ""),
            provider=data.get("provider", {}).get("name", ""),
            ide=data.get("ide", {}).get("name", ""),
        )


# Regex patterns for parsing git URLs
GIT_URL_PATTERNS = [
    # git@github.com:owner/repo.git
    re.compile(r"git@github\.com:([^/]+)/([^/]+?)(?:\.git)?$"),
    # https://github.com/owner/repo.git or https://github.com/owner/repo
    re.compile(r"https?://github\.com/([^/]+)/([^/]+?)(?:\.git)?$"),
    # github.com/owner/repo
    re.compile(r"^github\.com/([^/]+)/([^/]+?)(?:\.git)?$"),
]


def parse_owner_repo_from_url(url: str) -> Optional[tuple]:
    """Extract (owner, repo) from a git URL."""
    for pattern in GIT_URL_PATTERNS:
        match = pattern.match(url)
        if match:
            return (match.group(1), match.group(2))
    return None


def get_git_remote_url(path: str) -> Optional[str]:
    """Get the origin remote URL from a git repository."""
    try:
        result = subprocess.run(
            ["git", "-C", path, "remote", "get-url", "origin"],
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode == 0:
            return result.stdout.strip()
    except (OSError, subprocess.SubprocessError):
        pass
    return None


def _git_ls_remote(owner_repo: str, *args: str) -> Optional[str]:
    """Run git ls-remote and return stdout, or None on error.

    Uses SSH URL for consistency with other git operations.
    Includes timeout to prevent hanging on slow/unreachable remotes.
    """
    url = f"git@github.com:{owner_repo}.git"
    try:
        result = subprocess.run(
            ["git", "ls-remote", url, *args],
            capture_output=True,
            text=True,
            check=False,
            timeout=5,
        )
        if result.returncode == 0 and result.stdout.strip():
            return result.stdout
    except (OSError, subprocess.SubprocessError, subprocess.TimeoutExpired):
        pass
    return None


def remote_branch_exists(owner_repo: str, branch: str) -> bool:
    """Check if a branch exists on a remote GitHub repository."""
    output = _git_ls_remote(owner_repo, "--heads", branch)
    return bool(output)


def discover_repos_from_workspaces(workspaces: List[Workspace]) -> Dict[str, List[str]]:
    """Discover owner/repo from workspace git remotes.

    Returns dict mapping owner -> list of repos.
    """
    repos: Dict[str, List[str]] = {}

    for ws in workspaces:
        owner_repo = None

        # For git workspaces, parse the source URL directly
        if ws.source_type == "git":
            owner_repo = parse_owner_repo_from_url(ws.source)

        # For local workspaces, try to get git remote
        elif ws.source_type == "local" and ws.source:
            remote_url = get_git_remote_url(ws.source)
            if remote_url:
                owner_repo = parse_owner_repo_from_url(remote_url)

        if owner_repo:
            owner, repo = owner_repo
            if owner not in repos:
                repos[owner] = []
            if repo not in repos[owner]:
                repos[owner].append(repo)

    return repos


def discover_repos_from_cache_dir() -> Dict[str, List[str]]:
    """Discover owner/repo from bare repos in the local cache directory.

    Scans <repos_dir>/<owner>/<repo>/.bare/ on the filesystem so that repos
    cloned locally (even without a devpod workspace yet) are discovered.

    Returns dict mapping owner -> list of repos (same shape as
    ``discover_repos_from_workspaces``).
    """
    repos: Dict[str, List[str]] = {}
    repos_dir = pathlib.Path(get_worktree_config().repos_dir)
    if not repos_dir.is_dir():
        return repos
    try:
        for owner_dir in repos_dir.iterdir():
            if not owner_dir.is_dir():
                continue
            for repo_dir in owner_dir.iterdir():
                if not repo_dir.is_dir():
                    continue
                if (repo_dir / ".bare").is_dir():
                    owner = owner_dir.name
                    repo = repo_dir.name
                    if owner not in repos:
                        repos[owner] = []
                    if repo not in repos[owner]:
                        repos[owner].append(repo)
    except OSError:
        pass
    return repos


def get_known_repos() -> List[str]:
    """Get list of known owner/repo strings from workspaces."""
    workspaces = list_workspaces()
    repos = discover_repos_from_workspaces(workspaces)
    result = []
    for owner, repo_list in sorted(repos.items()):
        for repo in sorted(repo_list):
            result.append(f"{owner}/{repo}")
    return result


def run_devpod(
    args: List[str], capture: bool = False, env: Optional[Dict[str, str]] = None
) -> subprocess.CompletedProcess:
    """Run a devpod command.

    env replaces devpod's whole environment when given, so a caller that wants
    to add one variable must build it from os.environ. It exists so a secret can
    be handed to devpod without putting it in argv, where ps would expose it.

    Security note: Using list form of subprocess.run (not shell=True) prevents
    command injection. Each list element is passed as a separate argument to
    the executable, so special characters are not interpreted by a shell.

    This is dl's only devpod spawn, so it is also the only place that can tell
    "devpod is not installed" from "devpod ran and failed". The former is
    raised as DevpodNotInstalled rather than folded into a returncode: callers
    branch on returncode and would carry on as though devpod had answered.
    """
    cmd = ["devpod"] + args
    logging.debug("Running: %s", " ".join(cmd))
    try:
        if capture:
            # nosec B603 - using list form, not shell=True; no command injection risk
            return subprocess.run(cmd, capture_output=True, text=True, check=False, env=env)
        # nosec B603 - using list form, not shell=True; no command injection risk
        return subprocess.run(cmd, check=False, env=env)
    except FileNotFoundError as e:
        raise DevpodNotInstalled(DEVPOD_MISSING_MESSAGE) from e


def run_devpod_session(
    args: List[str], env: Optional[Dict[str, str]] = None
) -> devpod_ssh.SshOutcome:
    """Run a devpod command that hands its stdin/stdout to a terminal session.

    stdin and stdout are inherited untouched — devpod puts the real terminal into
    raw mode through them, and requests a pty on that basis. Only stderr is read,
    which under a pty carries devpod's own warnings and errors and nothing else,
    so that devpod's report of how the session ended can be interpreted rather
    than dumped on the user. See devpod_ssh for why that is necessary.
    """
    cmd = ["devpod"] + args
    logging.debug("Running: %s", " ".join(cmd))
    # nosec B603 - using list form, not shell=True; no command injection risk
    with subprocess.Popen(
        cmd,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=env,
    ) as proc:
        # proc.stderr is a pipe because PIPE was asked for, but Popen's type
        # cannot express that, so the narrowing happens here rather than by
        # widening filter_devpod_stderr to a None it would have no answer for.
        pipe = proc.stderr
        remote_status = (
            devpod_ssh.filter_devpod_stderr(pipe, sys.stderr) if pipe is not None else None
        )
    return devpod_ssh.interpret(proc.returncode, remote_status)


# The memoized `devpod list` snapshot. A dict rather than a module-level
# Optional so the accessors below need no `global`, and so "nothing read yet"
# (no key at all) stays distinguishable from "devpod has no workspaces" (an
# empty list) — the two must not be confused, or a real empty answer would be
# re-read on every call.
_WORKSPACE_LIST_KEY = "workspaces"
_workspace_list_cache: Dict[str, List[Workspace]] = {}


def invalidate_workspace_list_cache() -> None:
    """Forget the memoized `devpod list` snapshot.

    Every dl code path that changes what devpod would list calls this, so a
    later read in the same process re-reads devpod instead of answering from a
    snapshot taken before the change.
    """
    _workspace_list_cache.pop(_WORKSPACE_LIST_KEY, None)


def list_workspaces(refresh: bool = False) -> List[Workspace]:
    """List all devpod workspaces, reading devpod at most once per command.

    `devpod list --output json` costs ~0.45s — five times the entire Python
    startup dl pays — and six call sites read it, so `dl --purge` used to spend
    ~0.9s asking the same question twice. dl is a short-lived single-command
    process, so one snapshot per command is enough; the only reader that could
    see a stale one is a reader that runs after dl itself changed a workspace,
    and every such mutation calls invalidate_workspace_list_cache().

    refresh=True bypasses the snapshot for a caller that must have the
    post-mutation truth even if nothing announced the mutation.

    Only an answer devpod actually gave is remembered. A failed or unparsable
    read returns an empty list without caching it, so a transient failure — and
    a missing devpod, which raises out of here — can never be served again as
    "this machine has no workspaces".
    """
    if not refresh:
        cached = _workspace_list_cache.get(_WORKSPACE_LIST_KEY)
        if cached is not None:
            # A copy: a caller that sorts or filters its list in place must not
            # be rewriting what the next caller sees.
            return list(cached)
    result = run_devpod(["list", "--output", "json"], capture=True)
    if result.returncode != 0 or not result.stdout.strip():
        return []
    try:
        data = json.loads(result.stdout)
    except json.JSONDecodeError:
        logging.error("Failed to parse devpod output")
        return []
    workspaces = [Workspace.from_json(ws) for ws in data]
    _workspace_list_cache[_WORKSPACE_LIST_KEY] = workspaces
    return list(workspaces)


def get_workspace_ids() -> List[str]:
    """Get list of workspace IDs for completion."""
    return [ws.id for ws in list_workspaces()]


def print_workspaces():
    """Print workspace list in a nice format."""
    workspaces = list_workspaces()
    if not workspaces:
        print("No workspaces found.")
        return

    # Calculate column widths
    id_width = max(len(ws.id) for ws in workspaces)
    type_width = max(len(ws.source_type) for ws in workspaces)
    source_width = max(len(ws.source) for ws in workspaces)

    # Print header
    print(
        f"{'WORKSPACE':<{id_width}}  {'TYPE':<{type_width}}  {'SOURCE':<{source_width}}  LAST USED"
    )
    print("-" * (id_width + type_width + source_width + 30))

    # Print rows
    for ws in workspaces:
        last_used = ws.last_used[:19].replace("T", " ") if ws.last_used else "never"
        print(
            f"{ws.id:<{id_width}}  {ws.source_type:<{type_width}}  {ws.source:<{source_width}}  {last_used}"
        )


def fuzzy_select_workspace() -> Optional[str]:
    """Interactive fuzzy finder for workspace selection."""
    try:
        from iterfzf import iterfzf
    except ImportError:
        logging.error("iterfzf not available. Install with: pip install iterfzf")
        return None

    workspaces = list_workspaces()
    if not workspaces:
        logging.info("No workspaces found. Create one with: dl owner/repo or dl ./path")
        return None

    # Format options for display: "id | type | source"
    options = []
    ws_map = {}
    for ws in workspaces:
        label = f"{ws.id} | {ws.source_type} | {ws.source}"
        options.append(label)
        ws_map[label] = ws.id

    print("Select workspace (type to filter):")
    try:
        selected = iterfzf(options, multi=False)
    except KeyboardInterrupt:
        return None
    if selected:
        return ws_map.get(selected)
    return None


def get_context_options() -> dict:
    """Fetch all devpod context options as a dict of {name: value}."""
    result = run_devpod(["context", "options", "--output", "json"], capture=True)
    if result.returncode != 0 or not result.stdout.strip():
        return {}
    try:
        data = json.loads(result.stdout)
        return {k: v.get("value") for k, v in data.items() if v.get("value")}
    except (json.JSONDecodeError, AttributeError):
        return {}


def workspace_up(
    workspace: str,
    ide: Optional[str] = None,
    recreate: bool = False,
    reset: bool = False,
    workspace_id: Optional[str] = None,
    workspace_identity: Optional[str] = None,
    devcontainer: Optional[str] = None,
):
    """Start or create a workspace.

    workspace_id is devpod's --id, passed only when creating. workspace_identity
    is the id the workspace is known by either way, injected into the workspace
    initialization environment as DEVLAUNCH_WORKSPACE_ID so a project's host-side
    initializeCommand can tell branch workspaces apart — devpod gives the hook no
    workspace identity of its own (see docs/devcontainer-projects.md).

    devcontainer is a devcontainer.json path from _resolve_devcontainer_ref.
    """
    args = ["up", workspace]
    if workspace_id:
        args.extend(["--id", workspace_id])
    # Default to no IDE: dl attaches a terminal shell, so devpod's configured
    # default IDE (vscode) must not auto-open a window on every `dl <ws>`.
    # `dl <ws> code` passes ide="vscode" explicitly.
    args.extend(["--ide", ide if ide else "none"])
    if devcontainer:
        args.extend(["--devcontainer-path", devcontainer])
    identity = workspace_identity or workspace_id
    if identity:
        args.extend(["--init-env", f"DEVLAUNCH_WORKSPACE_ID={identity}"])
    if recreate:
        args.append("--recreate")
    if reset:
        args.append("--reset")
    ctx = get_context_options()
    if ctx.get("DOTFILES_URL"):
        args.extend(["--dotfiles", ctx["DOTFILES_URL"]])
    if ctx.get("DOTFILES_SCRIPT"):
        args.extend(["--dotfiles-script", ctx["DOTFILES_SCRIPT"]])
    # Give every workspace the host's gh login, whatever its devcontainer.json
    # does or doesn't set up for itself.
    with gh_auth.up_args() as token_args:
        args.extend(token_args)
        result = run_devpod(args)
    # `up` creates and starts workspaces, so any snapshot of `devpod list`
    # taken before it is now out of date.
    invalidate_workspace_list_cache()
    # The tools a session always needs, for whatever repo this is: `up` is the
    # one path that runs for every workspace dl opens and is already slow
    # enough to absorb a round-trip. Only after a successful `up` -- there is
    # no container to install into otherwise.
    if result.returncode == 0 and identity:
        tools.ensure_tools(identity, run_devpod)
    return result


def workspace_ssh(
    workspace: str,
    command: Optional[str] = None,
    workdir: Optional[str] = None,
) -> int:
    """SSH into a workspace, optionally running a command.

    Args:
        workspace: The workspace ID to SSH into
        command: Optional command to run (if None, starts interactive shell)
        workdir: Working directory to start in. Leave unset to land in the
            workspaceFolder from devcontainer.json — devpod falls back to $HOME
            if given a path that doesn't exist in the container, so never guess
            one from the workspace id.
    """
    args = ["ssh", workspace]

    if workdir:
        args.extend(["--workdir", workdir])
    if command:
        # devpod runs --command under a non-login, non-interactive `bash -c`,
        # which sources neither ~/.profile nor ~/.bashrc -- so PATH entries the
        # image adds there (notably $HOME/.pixi/bin) are missing and the payload
        # dies with "command not found". An interactive attach gets a login
        # shell, so wrap here to give both paths the same PATH. dl launches
        # arbitrary repos, so the parity has to come from the invocation rather
        # than from any particular devcontainer.json.
        args.extend(["--command", f"bash -lc {shlex.quote(command)}"])

    # Attaching to a running workspace skips workspace_up, so the gh login has
    # to be offered here too. Only the variable name lands in args; the token
    # travels in devpod's environment.
    token_args, env = gh_auth.ssh_args_and_env()
    args.extend(token_args)

    logging.info(f"SSH command: devpod {' '.join(args)}")
    outcome = run_devpod_session(args, env=env)

    # The two arms carry the same kind of number from different processes, which
    # is exactly the confusion this used to make: `dl` reported devpod's exit
    # code (always 1) for a session that had ended perfectly normally with, say,
    # 130. Whichever arm this is, the status returned is the session's.
    match outcome:
        case devpod_ssh.RemoteExit(status=status):
            return status
        case devpod_ssh.DevpodFailed(exit_code=exit_code):
            logging.debug("devpod ssh failed with exit code %s", exit_code)
            return exit_code
        case _ as unhandled:
            devpod_ssh.assert_never(unhandled)


def attach_workspace(workspace_id: str, shell_command: Optional[str] = None) -> int:
    """Hand the workspace to the user: name its prompt, then ssh in.

    Setting the hostname is a whole extra `devpod ssh` (~0.5s) in front of every
    attach, and it cannot be folded into the attach itself: bash reads the
    hostname once when the shell starts, so it has to be set before the session
    dl hands over begins, and `devpod ssh` exposes no hook inside that session.

    It is skipped for a one-shot `dl <ws> -- cmd`, which renders no prompt for
    the hostname to appear in — the round-trip would buy that command nothing.
    An interactive attach later still names the container, so nothing the user
    can see depends on having paid for it here.
    """
    if shell_command is None:
        setup_hostname(workspace_id)
    return workspace_ssh(workspace_id, shell_command)


def workspace_stop(workspace: str) -> int:
    """Stop a workspace."""
    result = run_devpod(["stop", workspace])
    # A stopped workspace still appears in `devpod list`, but with different
    # details, and `restart` calls straight into `up` after this.
    invalidate_workspace_list_cache()
    # The workspace list just changed, so the cache is wrong regardless of age.
    update_cache_background(force=True)
    return result.returncode


def workspace_delete(workspace: str) -> int:
    """Delete a workspace and its local clone (if any).

    The clone is removed only once devpod has actually let go of the workspace.
    devpod re-parses the workspace's devcontainer.json to tear the container
    down, so a config that has since moved or been renamed makes deletion fail —
    and removing the clone regardless strands the workspace for good, because
    devpod can then never find the config to retry with.
    """
    result = run_devpod(["delete", workspace])
    # Unconditionally: a delete that reports failure may still have got far
    # enough to change what devpod lists.
    invalidate_workspace_list_cache()
    if result.returncode != 0:
        logging.error(
            f"devpod could not delete {workspace}; keeping the local clone so it "
            f"stays retryable. If its devcontainer.json moved, restore the path or "
            f"run: devpod delete {workspace} --force"
        )
        update_cache_background(force=True)
        return result.returncode

    # Clean up local workspace clone (look up by workspace ID in metadata)
    try:
        clone_mgr = _get_clone_manager()
        if clone_mgr.remove_workspace_by_id(workspace):
            logging.info(f"Removed local clone for {workspace}")
    except Exception as e:
        logging.warning(f"Failed to remove local clone: {e}")
    # The workspace list just changed, so the cache is wrong regardless of age.
    update_cache_background(force=True)
    return result.returncode


def get_workspace_state(workspace_id: str) -> Optional[str]:
    """Get workspace state from devpod (e.g., 'Running', 'Stopped')."""
    result = run_devpod(["status", workspace_id, "--output", "json"], capture=True)
    if result.returncode != 0:
        return None
    try:
        data = json.loads(result.stdout)
        return data.get("state")
    except json.JSONDecodeError:
        logging.warning("Failed to parse devpod status JSON for workspace %s", workspace_id)
        return None


def print_help():
    """Print usage help."""
    help_text = """dl - DevLaunch CLI

Usage:
    dl                               Interactive workspace selector (fzf)
    dl <user/repo>                   Start workspace and attach shell
    dl <user/repo> <cmd>             Run workspace command (stop, code, etc.)
    dl <user/repo> -- <shell>        Run shell command in workspace

Options:
    --devcontainer <variant|path>    Use a non-default devcontainer.json. A bare
                                     name means .devcontainer/<name>/devcontainer.json.
                                     Stored with the workspace, so pass it once.

Environment:
    DEVLAUNCH_NO_GH_TOKEN=1          Do not forward the host's gh login into
                                     workspaces (forwarded as GH_TOKEN by default)

Workspace sources:
    dl myproject                     Existing workspace by name
    dl user/repo                     Create from GitHub repo
    dl user/repo@branch              Create from specific branch
    dl ./path                        Create from local path

Workspace commands:
    dl <user/repo> stop              Stop the workspace
    dl <user/repo> rm, prune         Delete the workspace
    dl <user/repo> code              Open in VS Code
    dl <user/repo> restart           Stop and start (no rebuild)
    dl <user/repo> recreate          Recreate container
    dl <user/repo> reset             Clean slate (remove all, recreate)
    dl <user/repo> -- <command>      Run shell command in workspace

Global commands:
    dl --ls                          List all workspaces
    dl --install                     Install shell completions
    dl --refresh                     Refresh completion cache
    dl --purge [-y]                  Remove all DevPod workspaces and caches
    dl --help, -h                    Show this help
    dl --version                     Show version (editable installs name their tree)

Examples:
    dl                               # Select workspace with fzf
    dl devpod                        # Open existing workspace
    dl loft-sh/devpod                # Create from GitHub
    dl blooop/devlaunch@main         # Create from specific branch
    dl ./my-project                  # Create from local folder
    dl blooop/devlaunch code         # Open in VS Code
    dl blooop/devlaunch -- make test # Run command in workspace
    dl blooop/devlaunch stop         # Stop workspace
    dl org/repo --devcontainer robot # Pick a devcontainer variant
"""
    print(help_text)


_cache: dict[str, WorkspaceCloneManager] = {}


def _get_clone_manager() -> WorkspaceCloneManager:
    """Lazy factory for WorkspaceCloneManager, migrating the cache on first use.

    This is where the one-shot id-scheme migration runs, for three reasons. It is
    dl's single construction point for the object that owns every read of a
    workspace path, so nothing can reach a stale path before the rename. It is
    lazy, so the commands that touch no workspace -- `--help`, `--version`,
    `--ls`, the completion commands, `--purge`, and opening an existing workspace
    by name -- never reach it, which keeps #58's promise that help does no work.
    And the memo makes it at most once per process.

    On an already-migrated cache this costs one integer comparison, because the
    trigger is the version header the storage load already parsed. Nothing here
    spawns devpod: the orphaned container ids come from metadata.
    """
    if "clone_manager" not in _cache:
        manager = WorkspaceCloneManager()
        try:
            migrate_cache(manager.storage, pathlib.Path(manager.config.repos_dir))
        except OSError as e:
            # A failed migration must not take the command with it. The renames
            # that did happen are still resumable: the version header is only
            # written by the final save, so an unwritten file means the next run
            # migrates again and finds them already in place.
            logging.warning(f"Could not migrate the workspace cache: {e}")
        _cache["clone_manager"] = manager
    return _cache["clone_manager"]


# Commands that read the completion cache without changing what it describes.
# These are the ones worth warming it for before they run.
_CACHE_READING_COMMANDS = ("--ls", "--repos", "--completion-data")


def wants_startup_cache_refresh(args: List[str]) -> bool:
    """Whether this invocation should warm the completion cache before running.

    Only the commands that read the cache and leave the workspace list alone.
    Everything else either has no use for completions (--help, --version), owns
    the refresh itself (--install, --refresh, --update-cache), is about to delete
    the cache directory (--purge), or is a workspace command -- and those refresh
    once they are finished, when the workspace list they cache is final, instead
    of indexing the state they are about to replace.
    """
    if not args:
        return True  # the fzf picker is a view of the workspace list
    return args[0] in _CACHE_READING_COMMANDS


def main(argv: Optional[List[str]] = None) -> int:
    """Main entry point for dl CLI.

    Thin wrapper so there is exactly one handler for a missing devpod, however
    deep in the command it was noticed. The message goes to stderr because
    stdout is parsed by the completion machinery (--repos, --completion-data).

    argv is the argument list without the program name, defaulting to the real
    one. It is a parameter so that a sibling entry point can hand dl a command
    line it built and get dl's behaviour itself rather than a second copy of it
    (see aid.py) — a copy is what left aid rebuilding containers dl reuses.
    """
    # The workspace-list snapshot is scoped to one command, not to the process:
    # a caller that drives main() twice (a test, a shell wrapper) must not have
    # the first command's view of devpod answer the second command's questions.
    invalidate_workspace_list_cache()
    try:
        return _run_cli(argv)
    except DevpodNotInstalled as e:
        print(e, file=sys.stderr)
        return DEVPOD_MISSING_EXIT_CODE


def _run_cli(argv: Optional[List[str]] = None) -> int:
    """Dispatch a dl command line. See main() for the error handling around it."""
    if argv is None:
        argv = sys.argv[1:]
    try:
        args, devcontainer = extract_devcontainer_flag(argv)
    except ValueError as e:
        logging.error(str(e))
        return 1

    # The decision has to come after parsing: `dl --help` must not pay for a
    # refresh it has no use for.
    if wants_startup_cache_refresh(args):
        update_cache_background()

    # No args - try fzf selection
    if not args:
        selected = fuzzy_select_workspace()
        if not selected:
            print_help()
            return 1
        workspace_up(
            selected,
            workspace_identity=selected,
            devcontainer=devcontainer,
        )
        return attach_workspace(selected)

    # Global commands (no workspace required)
    if args[0] in ("--help", "-h"):
        print_help()
        return 0

    if args[0] == "--version":
        print(f"dl {get_version()}")
        return 0

    if args[0] == "--ls":
        print_workspaces()
        return 0

    if args[0] == "--repos":
        # Output known repos for bash completion (uses cache if available)
        cache = read_completion_cache()
        if cache and "repos" in cache:
            for repo in cache["repos"]:
                print(repo)
        else:
            for repo in get_known_repos():
                print(repo)
        return 0

    if args[0] == "--update-cache":
        # Silent background update. The TTL is re-checked here as well as in the
        # parent that spawned us: two parents can both see a stale cache before
        # either child has written one, and the second sweep would be pure waste.
        # --force marks a refresh that follows a workspace change, where the
        # cache is wrong however new it is.
        if "--force" not in args[1:] and completion_cache_is_fresh():
            return 0
        update_completion_cache()
        return 0

    if args[0] == "--refresh":
        # Manual refresh with feedback
        print("Refreshing completion cache...")
        data = update_completion_cache()
        print(f"Cache updated: {len(data.get('workspaces', []))} workspaces found")
        return 0

    if args[0] == "--completion-data":
        # Output all completion data as JSON (fast, from cache)
        cache = read_completion_cache()
        if cache:
            print(json.dumps(cache))
        else:
            # No cache, generate and cache it
            data = update_completion_cache()
            print(json.dumps(data))
        return 0

    if args[0] == "--install":
        rc_path = None
        if len(args) > 1:
            rc_path = pathlib.Path(args[1])
        # Generate cache so completions work immediately
        update_completion_cache()
        return install_completions(rc_path)

    if args[0] == "--purge":
        # Check for -y flag to skip confirmation
        skip_confirm = len(args) > 1 and args[1] in ("-y", "--yes")
        cache_dir = _get_cache_dir()
        workspaces = list_workspaces()
        print("This will remove all devlaunch data:")
        print(f"  - {len(workspaces)} DevPod workspace(s)")
        print(f"  - {cache_dir}/ (workspace clones, repo caches, completions)")
        print()
        if skip_confirm:
            return purge_all_data()
        confirm = input("Are you sure? [y/N] ").strip().lower()
        if confirm in ("y", "yes"):
            return purge_all_data()
        print("Aborted.")
        return 0

    # Workspace commands: dl <workspace> [subcommand] [-- command]
    raw_spec = args[0]
    subcommand = args[1] if len(args) > 1 else None

    # Validate the workspace spec
    existing_ids = get_workspace_ids()
    error = validate_workspace_spec(raw_spec, existing_ids)
    if error:
        logging.error(error)
        return 1

    # Resolve workspace spec and ID
    is_existing = raw_spec in existing_ids
    parsed = parse_owner_repo_branch(raw_spec)

    if is_existing:
        workspace_spec = raw_spec
        workspace_id = raw_spec
        custom_id = None  # Don't pass --id for existing workspaces
    elif parsed:
        # Git spec (owner/repo[@branch]) — check if workspace already exists first
        owner_repo, branch = parsed
        owner, repo = owner_repo.split("/", 1)

        # Validate owner and repo before anything builds a path out of them. The
        # branch is not resolved yet, so its check waits for the WorkspaceId below,
        # but ensure_repo() joins repos_dir/<owner>/<repo> and would otherwise act on
        # a traversal first and reject it after: `x/..` resolves to repos_dir itself
        # and `../x` leaves it entirely.
        try:
            validate_ref_name(owner, "owner")
            validate_ref_name(repo, "repo")
        except ValueError as e:
            logging.error(str(e))
            return 1

        remote_url = f"git@github.com:{owner_repo}.git"

        clone_mgr = _get_clone_manager()
        repo_ensured = False

        # Resolve branch early so we can compute workspace ID
        if not branch:
            try:
                clone_mgr.repo_manager.ensure_repo(owner, repo, remote_url)
            except (RuntimeError, OSError) as e:
                logging.error(f"Repository '{owner_repo}': {e}")
                return 1
            repo_ensured = True
            branch = clone_mgr.repo_manager.get_default_branch(owner, repo)

        # Compute workspace ID with resolved branch. Constructing the WorkspaceId is
        # the parse boundary: an unsafe owner, repo or ref is rejected here, before it
        # can name a container, a directory or a git command. Nothing downstream
        # re-checks it, because holding the WorkspaceId is the evidence.
        try:
            workspace = WorkspaceId(owner, repo, branch)
        except ValueError as e:
            logging.error(str(e))
            return 1
        workspace_id = workspace.value

        # Fast path: if devpod already knows this workspace, skip clone manager
        if workspace_id in existing_ids:
            workspace_spec = workspace_id
            custom_id = None
        else:
            # Full path: clone locally and pass local path to DevPod
            if not repo_ensured:
                try:
                    clone_mgr.repo_manager.ensure_repo(owner, repo, remote_url)
                except (RuntimeError, OSError) as e:
                    logging.error(f"Repository '{owner_repo}': {e}")
                    return 1

            # Ensure branch exists in bare repo (create on remote if needed)
            try:
                clone_mgr.ensure_branch(owner, repo, branch)
            except (RuntimeError, OSError) as e:
                logging.error(f"Failed to ensure branch '{branch}': {e}")
                return 1

            custom_id = workspace_id

            # Create workspace clone
            try:
                workspace_path = clone_mgr.ensure_workspace(owner, repo, branch, remote_url)
                workspace_spec = str(workspace_path)
            # ValueError: the branch resolved above does go through WorkspaceId, but
            # ensure_workspace may fall back to the default branch recorded in the
            # repo's stored metadata, and that value reaches git unproven. It is
            # checked where it enters argv, which raises from in here.
            except (RuntimeError, OSError, ValueError) as e:
                logging.error(f"Failed to prepare workspace: {e}")
                return 1
    else:
        workspace_spec = expand_workspace_spec(raw_spec)
        workspace_id = spec_to_workspace_id(raw_spec)
        custom_id = workspace_id  # Pass --id to create with our desired ID

    # Handle workspace subcommands
    # Paths below that never call workspace_up cannot honour a config choice.
    # Say so rather than discarding it silently.
    if devcontainer and subcommand in ("stop", "rm", "prune"):
        logging.warning(f"Ignoring --devcontainer: it does not apply to '{subcommand}'.")

    if subcommand == "stop":
        return workspace_stop(workspace_id)

    if subcommand in ("rm", "prune"):
        return workspace_delete(workspace_id)

    if subcommand == "code":
        result = workspace_up(
            workspace_spec,
            ide="vscode",
            workspace_id=custom_id,
            workspace_identity=workspace_id,
            devcontainer=devcontainer,
        )
        update_cache_background(force=True)
        return result.returncode

    if subcommand == "recreate":
        result = workspace_up(
            workspace_spec,
            recreate=True,
            workspace_id=custom_id,
            workspace_identity=workspace_id,
            devcontainer=devcontainer,
        )
        if result.returncode != 0:
            return result.returncode
        ret = attach_workspace(workspace_id)
        update_cache_background(force=True)
        return ret

    if subcommand == "restart":
        # Stop and start without rebuilding
        stop_ret = workspace_stop(workspace_id)
        if stop_ret != 0:
            return stop_ret
        result = workspace_up(
            workspace_spec,
            workspace_id=custom_id,
            workspace_identity=workspace_id,
            devcontainer=devcontainer,
        )
        if result.returncode != 0:
            return result.returncode
        ret = attach_workspace(workspace_id)
        # workspace_stop already asked for a refresh on the way through; the
        # once-per-process latch is what keeps this from being a second one.
        update_cache_background(force=True)
        return ret

    if subcommand == "reset":
        # Clean slate - remove everything and recreate
        result = workspace_up(
            workspace_spec,
            reset=True,
            workspace_id=custom_id,
            workspace_identity=workspace_id,
            devcontainer=devcontainer,
        )
        if result.returncode != 0:
            return result.returncode
        ret = attach_workspace(workspace_id)
        update_cache_background(force=True)
        return ret

    # Check for shell command (after --)
    shell_command = None
    if subcommand == "--" and len(args) > 2:
        shell_command = " ".join(args[2:])
    elif subcommand is not None and subcommand != "--":
        # Unknown subcommand - treat as error
        logging.error(
            f"Unknown command '{subcommand}'. Use 'dl {raw_spec} -- {subcommand}' to run a shell command."
        )
        return 1

    # Fast-attach: skip workspace_up() if workspace is already running
    if custom_id is None and get_workspace_state(workspace_id) == "Running":
        logging.info(f"Workspace {workspace_id} is already running, attaching...")
        if devcontainer:
            logging.warning(
                f"Ignoring --devcontainer: {workspace_id} is already running. "
                f"Use 'dl {raw_spec} recreate --devcontainer ...' to switch config."
            )
        ret = attach_workspace(workspace_id, shell_command)
        update_cache_background(force=True)
        return ret

    # Default: start workspace and attach shell
    try:
        result = workspace_up(
            workspace_spec,
            workspace_id=custom_id,
            workspace_identity=workspace_id,
            devcontainer=devcontainer,
        )
    except (RuntimeError, OSError) as e:
        logging.error(f"Failed to create workspace: {e}")
        return 1

    if result.returncode != 0:
        return result.returncode

    # Attach to workspace (naming its prompt first, for an interactive shell)
    ret = attach_workspace(workspace_id, shell_command)

    # Update cache after workspace operations: this path may have created the
    # workspace, so the refresh has to happen now that it exists.
    update_cache_background(force=True)

    return ret


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
