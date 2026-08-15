"""Repository manager for worktree backend."""

import contextlib
import dataclasses
import logging
import os
import shutil
import subprocess
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Iterator, NoReturn, Optional, TYPE_CHECKING, Union

from .. import timing
from ..workspace_id import validate_ref_name
from . import locks
from .git_errors import git_failure_reason
from .models import BaseRepository
from .storage import MetadataStorage

if TYPE_CHECKING:
    from .config import WorktreeConfig

logger = logging.getLogger(__name__)


def _branch_of(ref: str) -> str:
    """The branch a symbolic ref names, with its namespace prefix removed.

    Not the last path segment: `release/1.0` and `feature/auth` are ordinary
    branch names, and taking the segment after the final slash renames them to
    `1.0` and `auth` -- refs the repository does not have.
    """
    for prefix in ("refs/remotes/origin/", "refs/heads/"):
        if ref.startswith(prefix):
            return ref[len(prefix) :]
    return ref.rsplit("/", maxsplit=1)[-1]


@dataclass(frozen=True)
class Updated:
    """The requested ref in the cache now matches the remote's.

    Carries no payload: "it is current" is the whole message. A distinct type
    rather than True so that the three answers cannot collapse into a bool that
    reads "fetched / did not fetch" and loses the difference between the two ways
    of not fetching -- which is the difference the caller acts on.
    """


@dataclass(frozen=True)
class RefMissingOnRemote:
    """The remote has no such ref, and answered to say so.

    Not a failure. This is what an ordinary "start a new branch" launch gets, and
    the caller's response is to base the branch on the default branch instead. A
    reachable remote is *evidence* here -- the ref's absence is established
    rather than assumed, which is what makes basing a new branch on the default
    branch the right move rather than a guess.
    """


@dataclass(frozen=True)
class FetchFailed:
    """The remote could not be asked, and *reason* is what git said.

    Distinct from :class:`RefMissingOnRemote` because nothing was learned about
    the remote: the ref may well exist. So the caller may only fall back to
    whatever the cache already holds, and must not invent a branch off the
    default branch on the strength of an answer it never got.

    The reason is carried rather than reconstructed at the print site, following
    :class:`devlaunch.workspace_state.CouldNotTell`: no such host, a refused
    connection, an expired credential and a bare cache that is not there all
    arrive here and read differently to whoever has to fix it.
    """

    reason: str


# Three arms and no fourth. "The cache is not on disk" is a FetchFailed rather
# than a fourth arm: nothing was learned about the remote, which is exactly the
# property that decides what a caller may do next.
FetchOutcome = Union[Updated, RefMissingOnRemote, FetchFailed]


def unhandled_fetch_outcome(outcome: NoReturn) -> NoReturn:
    """Reject a fetch outcome nobody handled -- at type-check time, not at runtime.

    The same shape as :func:`devlaunch.workspace_state.unhandled_unsaved`, and
    exported for the same reason: the arms are read outside this module, and an
    ``else`` hand-rolled at a call site is how a fourth arm gets silently read as
    one of the existing three.
    """
    raise AssertionError(f"Unhandled fetch outcome: {outcome!r}")


# The one value that turns a RepoLock constructor call into a real token, held
# privately by this module so that :meth:`RepositoryManager.hold_repo_lock` is
# the only place able to pass it.
_MINTED_INSIDE_THE_LOCK = object()


@dataclass(frozen=True)
class RepoLock:
    """Evidence that the per-repo lock for ``(owner, repo)`` is held right now.

    The same proof-carrying pattern as :class:`devlaunch.workspace_id.WorkspaceId`
    -- holding it *is* the evidence -- applied to a different kind of fact. A
    method that takes one cannot be called without the lock, so the rule
    ``hold_lock`` needs from its callers is stated by the signature instead of
    begged for in a comment at each site. Three such comments used to stand in
    front of the acquisitions on the cold path, and a comment is not read by the
    caller who is about to deadlock against it.

    **It carries the pair, and that is not decoration.** A bare marker type would
    let a lock taken on ``owner/repo`` vouch for work on ``owner/other``: the
    lock genuinely held, the wrong repository genuinely unserialized, and nothing
    in the signature able to tell. Every method that takes a token checks it
    against the repository it is about to touch -- see :meth:`covers`.

    Minted only by :meth:`RepositoryManager.hold_repo_lock`; constructing one by
    hand raises. A token anybody could build would prove nothing, which is the
    whole of what this type is for.
    """

    owner: str
    repo: str
    #: Not data: the sentinel the lock scope passes to prove it is the minter.
    #: An ``InitVar`` so it is checked and then forgotten, leaving a token that
    #: carries the pair and nothing else.
    mint: dataclasses.InitVar[object] = None

    def __post_init__(self, mint: object) -> None:
        if mint is not _MINTED_INSIDE_THE_LOCK:
            raise TypeError(
                "A RepoLock is proof that the repo lock is held, so it is minted "
                "only by RepositoryManager.hold_repo_lock()."
            )

    def covers(self, owner: str, repo: str) -> bool:
        """Whether this token is evidence about *owner*/*repo* specifically."""
        return (self.owner, self.repo) == (owner, repo)

    def require(self, owner: str, repo: str) -> None:
        """Raise unless this token is evidence about *owner*/*repo*.

        ``ValueError`` rather than an assertion: it is a caller mistake of the
        same kind as an unsafe ref, and the launch path already treats a
        ``ValueError`` out of this layer as the launch failing rather than as a
        crash to report.
        """
        if not self.covers(owner, repo):
            raise ValueError(
                f"repo lock held for {self.owner}/{self.repo} cannot vouch for {owner}/{repo}"
            )


class RepositoryManager:
    """Manages base git repositories."""

    def __init__(
        self,
        repos_dir: Path,
        storage: Optional[MetadataStorage] = None,
        config: Optional["WorktreeConfig"] = None,
    ):
        """Initialize repository manager."""
        self.repos_dir = repos_dir
        self.repos_dir.mkdir(parents=True, exist_ok=True)
        self.storage = storage or MetadataStorage()
        self.config = config
        # Default fetch interval: 1 hour
        self.fetch_interval = config.fetch_interval if config else 3600

    def get_repo_path(self, owner: str, repo: str) -> Path:
        """Get root directory for a repository (contains .bare/ and branch clones)."""
        return self.repos_dir / owner / repo

    def get_bare_path(self, owner: str, repo: str) -> Path:
        """Get the bare git directory for a repository."""
        return self.get_repo_path(owner, repo) / ".bare"

    def lock_path(self, owner: str, repo: str) -> Path:
        """The lock every process takes before mutating repos/<owner>/<repo>.

        A file, not a directory, inside the repo dir: every walker of the cache
        filters on ``is_dir()``, so it is invisible to discovery, migration and
        completion scans.
        """
        return self.get_repo_path(owner, repo) / ".lock"

    def clone_repo(self, owner: str, repo: str, remote_url: str) -> BaseRepository:
        """Clone a new base repository as bare (no working directory).

        Using --bare ensures no branch is checked out, so all branches can have
        worktrees created without conflicts.

        Layout:
            repos/<owner>/<repo>/.bare/   # bare git data
            repos/<owner>/<repo>/<branch>/ # workspace clones (managed by WorkspaceCloneManager)

        The cache and the clones being *siblings of one directory* is load-bearing
        and not merely tidy: it is what puts every workspace clone on the same
        filesystem as the objects it clones from, so the local transport can
        hardlink the pack files rather than copy them. See
        ``WorkspaceCloneManager._prepare_workspace`` for what that is worth and
        for the cross-filesystem fallback this layout makes unreachable.
        """
        bare_path = self.get_bare_path(owner, repo)

        if bare_path.exists():
            logger.warning(f"Repository {owner}/{repo} already exists at {bare_path}")
            existing_repo = self.get_repo(owner, repo)
            if existing_repo:
                return existing_repo
            if (bare_path / "HEAD").exists():
                # The bare clone is already on disk but this process has no
                # record of it -- another process just made it (this process's
                # metadata was loaded before that one saved), or an earlier run
                # died between clone and save. Either way the clone on disk is
                # the authority and the record is derived state: rebuild the
                # record. Cloning over it instead is not an option -- git
                # refuses the non-empty destination, and the failure cleanup
                # below would then delete a cache another launch is using.
                return self._register_existing_bare(owner, repo, remote_url, bare_path)
            # No HEAD: a dead run's partial clone. No live process owns it, and
            # what says so is the repo lock: in production this is reached only
            # through clone_if_missing, which cannot be called without the
            # RepoLock token that hold_repo_lock alone mints. Naming an
            # entrypoint instead would name the wrong gate -- ensure_repo and
            # prepare_cold both reach this, and what they have in common is the
            # lock, not the route. So clear it and clone fresh.
            #
            # `clone_repo` itself is public and takes no token, so this is a
            # property of the callers rather than one the signature enforces:
            # the tests call it directly and unlocked, which is safe only
            # because each has the cache to itself. A new production caller
            # would have to come through the lock scope to keep the rmtree
            # below true, and nothing here would stop it doing otherwise.
            logger.warning(f"Removing partial clone at {bare_path}")
            shutil.rmtree(bare_path)

        # Create parent directory
        bare_path.parent.mkdir(parents=True, exist_ok=True)

        logger.info(f"Cloning repository {remote_url} to {bare_path}")

        try:
            # Clone as bare repo - no working directory, all branches available for worktrees
            with timing.span("git clone --bare"):
                result = subprocess.run(
                    ["git", "clone", "--bare", remote_url, str(bare_path)],
                    capture_output=True,
                    text=True,
                    check=True,
                )
            logger.debug(f"Clone output: {result.stdout}")

            # Get default branch
            default_branch = self._get_default_branch(bare_path)

            # Create repository metadata
            base_repo = BaseRepository(
                owner=owner,
                repo=repo,
                remote_url=remote_url,
                local_path=bare_path,
                default_branch=default_branch,
                last_fetched=datetime.now(),
                worktrees=[],
            )

            # Save metadata
            self.storage.add_repository(base_repo)

            logger.info(f"Successfully cloned {owner}/{repo}")
            return base_repo

        except subprocess.CalledProcessError as e:
            logger.debug(f"Failed to clone repository: {e.stderr}")
            # Clean up the partial clone. Safe to delete: the exists-cases were
            # all handled above, so this directory is one this call created.
            if bare_path.exists():
                shutil.rmtree(bare_path)
            raise RuntimeError(f"Failed to clone repository: {e.stderr}") from e

    def _register_existing_bare(
        self, owner: str, repo: str, remote_url: str, bare_path: Path
    ) -> BaseRepository:
        """Rebuild the metadata record for a bare clone already on disk."""
        base_repo = BaseRepository(
            owner=owner,
            repo=repo,
            remote_url=remote_url,
            local_path=bare_path,
            default_branch=self._get_default_branch(bare_path),
            last_fetched=datetime.now(),
            worktrees=[],
        )
        self.storage.add_repository(base_repo)
        return base_repo

    def fetch_repo(self, owner: str, repo: str, timeout: Optional[float] = None) -> None:
        """Fetch latest changes from remote.

        *timeout* bounds the fetch, and matters because this runs under the repo
        lock: whoever holds that lock for the length of a fetch is somebody every
        other dl run wanting the same repo has to wait for. A launch is watched
        and interruptible, so it passes None and keeps the behaviour it had. The
        background sweep is neither -- it is a detached child in its own session,
        so a fetch of it that never returns is a repo wedged until reboot -- and
        it passes a bound. Reaching the bound raises like any other failure.
        """
        bare_path = self.get_bare_path(owner, repo)

        if not bare_path.exists():
            raise ValueError(f"Repository {owner}/{repo} does not exist locally")

        logger.info(f"Fetching updates for {owner}/{repo}")

        try:
            # Fetch all branches and tags
            with timing.span("git fetch"):
                result = subprocess.run(
                    ["git", "fetch", "origin", "+refs/heads/*:refs/heads/*", "--tags", "--prune"],
                    cwd=bare_path,
                    capture_output=True,
                    text=True,
                    check=True,
                    timeout=timeout,
                )
            logger.debug(f"Fetch output: {result.stdout}")

            # Update metadata
            base_repo = self.storage.get_repository(owner, repo)
            if base_repo:
                base_repo.last_fetched = datetime.now()
                self.storage.add_repository(base_repo)

            logger.info(f"Successfully fetched updates for {owner}/{repo}")

        except subprocess.TimeoutExpired as e:
            # subprocess has already killed the child. git writes new objects to
            # a temp pack and moves refs only at the end, so a fetch cut off
            # partway leaves the clone usable and the next pass redoes the work.
            logger.debug(f"Fetch of {owner}/{repo} exceeded {timeout}s")
            raise RuntimeError(f"Fetch of {owner}/{repo} timed out after {timeout}s") from e
        except subprocess.CalledProcessError as e:
            logger.debug(f"Failed to fetch repository: {e.stderr}")
            raise RuntimeError(f"Failed to fetch repository: {e.stderr}") from e

    def fetch_ref(self, owner: str, repo: str, branch: str) -> FetchOutcome:
        """Fetch exactly one branch into the bare cache, and say what happened.

        The launch path's entire network budget. Where :meth:`fetch_repo` sweeps
        every head and tag, this moves one ref, so the time it can hold the repo
        lock is bounded by one branch's worth of objects rather than by the size
        of the repository's whole history of branches.

        Unconditional by design -- no interval gate. The conditional version is
        more code and yields a mushy contract (fresh for branches you have not
        seen, stale for the ones you have); one single-ref fetch is noise next to
        the clone and `devpod up` this path already pays for.

        Deliberately does **not** write ``last_fetched``: that is the broad
        sweep's bookkeeping, and claiming it here would suppress the sweep for a
        whole interval on the strength of having fetched one branch. Not writing
        it also keeps the repo-lock→metadata-lock nesting off this path.

        Writes a ref in the shared bare repo, so it must not run unserialized.
        Its one caller, :meth:`WorkspaceCloneManager.ensure_branch`, holds a
        :class:`RepoLock` for this repository, which is what says so.
        """
        # The branch is interpolated into a refspec that reaches git as argv, so
        # it is checked here rather than trusted. ensure_branch's caller usually
        # holds a WorkspaceId proving it, but the default-branch retry arrives
        # from stored metadata unproven -- and this is a public method besides.
        validate_ref_name(branch)
        bare_path = self.get_bare_path(owner, repo)

        if not bare_path.exists():
            # Not RefMissingOnRemote: nothing was asked of the remote, so nothing
            # is known about it. Sending the caller off to base a branch on the
            # default branch would be basing it in a cache that is equally absent.
            return FetchFailed(f"Repository {owner}/{repo} does not exist locally")

        logger.info(f"Fetching {branch} for {owner}/{repo}")
        try:
            with timing.span("git fetch"):
                subprocess.run(
                    ["git", "fetch", "origin", f"+refs/heads/{branch}:refs/heads/{branch}"],
                    cwd=bare_path,
                    capture_output=True,
                    text=True,
                    check=True,
                    # The ref-missing arm below is classified from git's stderr
                    # text, which git translates -- so git is addressed in the C
                    # locale, or a German host would collapse the three-way outcome
                    # to two. LANGUAGE is pinned too: under gettext it outranks a
                    # non-C LC_ALL, and the guarantee should not hang on the one
                    # glibc rule that exempts C.
                    env={**os.environ, "LC_ALL": "C", "LANGUAGE": "C"},
                )
            return Updated()
        except subprocess.CalledProcessError as e:
            # The guard inside git_failure_reason is load-bearing for the
            # membership test below -- pinned by the silent-failure case in
            # test_worktree_repo_manager (#225); rationale at the helper.
            reason = git_failure_reason(e, "fetch")
            if "couldn't find remote ref" in reason:
                # git reached the remote and was told the ref is not there. This
                # is the one case where a non-zero exit is an *answer*.
                logger.debug(f"Remote has no ref {branch} for {owner}/{repo}")
                return RefMissingOnRemote()
            return FetchFailed(reason)

    def _should_fetch(self, repo: BaseRepository) -> bool:
        """Check if repository should be fetched based on fetch_interval.

        Returns True if:
        - Repository has never been fetched
        - Time since last fetch exceeds fetch_interval
        """
        if not repo.last_fetched:
            return True
        elapsed = (datetime.now() - repo.last_fetched).total_seconds()
        return elapsed > self.fetch_interval

    def lazy_fetch(self, owner: str, repo: str, timeout: Optional[float] = None) -> bool:
        """Fetch only if the fetch interval has elapsed since the last fetch.

        Returns True if a fetch was performed, False if skipped.
        Raises ValueError if the repository is not in metadata.
        *timeout* is passed straight to fetch_repo; see there for why.
        """
        base_repo = self.storage.get_repository(owner, repo)
        if not base_repo:
            raise ValueError(f"Repository {owner}/{repo} not found in metadata")
        if self._should_fetch(base_repo):
            self.fetch_repo(owner, repo, timeout=timeout)
            return True
        return False

    @contextlib.contextmanager
    def hold_repo_lock(self, owner: str, repo: str) -> Iterator[RepoLock]:
        """Hold the per-repo lock for the block, and hand out the proof.

        The only place a :class:`RepoLock` is minted, which is what makes the
        token mean something: every method that takes one is reachable only from
        inside a scope like this one.

        One scope per launch is the shape devlaunch#200 settled on. The
        alternatives were an outer ``with`` in the command layer, which leaks the
        lock-ordering doctrine into code that has no business knowing it, and a
        reentrant lock, which makes ownership invisible -- with nothing in a
        signature to say who holds what, "is this call already under the lock?"
        becomes a question answered by reading upwards through call sites.

        The contended flag ``hold_lock`` yields is deliberately not surfaced
        here. A launch that waited may well find the world changed, but nothing
        under this lock acts on that: every step below is idempotent and
        re-checks the disk itself.
        """
        with locks.hold_lock(
            self.lock_path(owner, repo),
            waiting_note=f"another dl run preparing {owner}/{repo}",
        ):
            yield RepoLock(owner, repo, _MINTED_INSIDE_THE_LOCK)

    def clone_if_missing(
        self, lock: RepoLock, owner: str, repo: str, remote_url: str
    ) -> BaseRepository:
        """Clone the bare cache for *owner*/*repo* if it is not already there.

        Clone-if-missing and nothing else. It deliberately does **not** refresh a
        cache that is already there, however stale: the broad sweep that used to
        run from here is the detached updater's job now (devlaunch#149), and the
        launch path's entire network budget is the one targeted ref fetch in
        :meth:`WorkspaceCloneManager.ensure_branch`. A fetch here would be
        unbounded network *under the repo lock*, so the launch that drew the short
        straw paid for everyone's freshness and every concurrent launch of the
        same repo queued behind it — the defect devlaunch#144 resolved.

        Freshness is not lost, it moved: see the staleness contract on
        :meth:`WorkspaceCloneManager.ensure_branch`.

        Takes the *lock* rather than acquiring one, because the whole
        exists-check-then-clone sequence has to be serialized and the cold path
        wants it serialized together with what follows it: without the lock, two
        processes launching the same repo at once both saw no clone and both ran
        ``git clone --bare`` into the same path — and the loser's cleanup in
        clone_repo deleted the winner's half-written cache. Serialized, the loser
        just waits and then reuses the winner's clone.
        """
        lock.require(owner, repo)
        if self.repo_exists(owner, repo):
            existing_repo = self.get_repo(owner, repo)
            if existing_repo:
                return existing_repo
            # Metadata doesn't exist but repo exists - fall through to clone
            # (which will add metadata)

        return self.clone_repo(owner, repo, remote_url)

    @timing.staged("host-prep")
    def ensure_repo(self, owner: str, repo: str, remote_url: str) -> BaseRepository:
        """Clone-if-missing in a lock scope of its own.

        What a bare ``owner/repo`` spec needs before it can name the default
        branch, and the one repo-lock cycle the launch path takes outside
        :meth:`WorkspaceCloneManager.prepare_cold`. Folding it into that scope
        would mean holding this lock across the fast-attach ``devpod status``
        that comes between them, so every sibling launch of the repo would queue
        behind a subprocess — a far worse trade than the uncontended flock it
        saves (devlaunch#200). Only the branch *name* crosses the gap, and the
        collapsed scope re-verifies clone-if-missing under its own lock.
        """
        with self.hold_repo_lock(owner, repo) as lock:
            return self.clone_if_missing(lock, owner, repo, remote_url)

    def repo_exists(self, owner: str, repo: str) -> bool:
        """Check if repository exists locally."""
        bare_path = self.get_bare_path(owner, repo)
        return bare_path.exists() and (bare_path / "HEAD").exists()

    def get_repo(self, owner: str, repo: str) -> Optional[BaseRepository]:
        """Get repository metadata."""
        base_repo = self.storage.get_repository(owner, repo)

        if base_repo and not self.repo_exists(owner, repo):
            # Repository metadata exists but directory doesn't
            logger.warning(f"Repository {owner}/{repo} metadata exists but directory missing")
            return None

        return base_repo

    def _get_default_branch(self, repo_path: Path) -> str:
        """Get the default branch of a repository.

        Works with both bare repos and regular repos.
        """
        try:
            # For bare repos, HEAD points directly to refs/heads/<branch>
            result = subprocess.run(
                ["git", "symbolic-ref", "HEAD"],
                cwd=repo_path,
                capture_output=True,
                text=True,
                check=True,
            )
            # Output is like "refs/heads/main". The prefix is stripped rather
            # than the last path segment taken: a branch name may contain
            # slashes, so `split("/")[-1]` turned a default branch of
            # `release/1.0` into `1.0` -- a ref the repository does not have,
            # recorded as the one every later operation targets.
            return _branch_of(result.stdout.strip())
        except subprocess.CalledProcessError:
            pass

        # Fallback: try the remote HEAD (for regular repos)
        try:
            result = subprocess.run(
                ["git", "symbolic-ref", "refs/remotes/origin/HEAD"],
                cwd=repo_path,
                capture_output=True,
                text=True,
                check=True,
            )
            return _branch_of(result.stdout.strip())
        except subprocess.CalledProcessError:
            # Fallback to main or master
            try:
                result = subprocess.run(
                    ["git", "branch", "-r"],
                    cwd=repo_path,
                    capture_output=True,
                    text=True,
                    check=True,
                )
                branches = result.stdout.strip()
                if "origin/main" in branches:
                    return "main"
                if "origin/master" in branches:
                    return "master"
            except subprocess.CalledProcessError:
                pass

        return "main"  # Default fallback

    @timing.staged("host-prep")
    def get_default_branch(self, owner: str, repo: str) -> str:
        """Get the default branch for a repository.

        Checks local repo first, then queries remote. Falls back to 'main'.
        """
        # Check if repo exists locally
        existing_repo = self.get_repo(owner, repo)
        if existing_repo and existing_repo.default_branch:
            return existing_repo.default_branch

        # Try to get from remote
        remote_url = f"git@github.com:{owner}/{repo}.git"
        try:
            result = subprocess.run(
                ["git", "ls-remote", "--symref", remote_url, "HEAD"],
                capture_output=True,
                text=True,
                check=False,
                timeout=10,
            )
            if result.returncode == 0:
                # Parse output like: ref: refs/heads/main\tHEAD
                for line in result.stdout.strip().split("\n"):
                    if line.startswith("ref:") and "HEAD" in line:
                        ref_part = line.split()[1]
                        if ref_part.startswith("refs/heads/"):
                            return ref_part[len("refs/heads/") :]
        except (OSError, subprocess.SubprocessError, subprocess.TimeoutExpired):
            pass

        return "main"

    def list_repositories(self):
        """List all managed repositories."""
        return self.storage.list_repositories()

    def remove_repository(self, owner: str, repo: str, remove_directory: bool = True) -> None:
        """Remove a repository from management."""
        # Remove metadata
        self.storage.remove_repository(owner, repo)

        # Optionally remove directory
        if remove_directory:
            repo_path = self.get_repo_path(owner, repo)
            if repo_path.exists():
                shutil.rmtree(repo_path)
                logger.info(f"Removed repository directory {repo_path}")
