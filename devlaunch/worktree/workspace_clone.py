"""Workspace clone manager for DevLaunch.

Orchestrates: bare repo caching → workspace clone → branch checkout.
Clones from the local bare reference repo (fast, saves bandwidth),
then fixes the remote URL so push/pull work against GitHub.

Directory layout, for repos/blooop/devlaunch/:
    ├── .bare/                          # bare git repo (hidden)
    ├── devlaunch-main-zovomobo/            # workspace clone
    └── devlaunch-feature-auth-poliseno/    # workspace clone

The leaf names are ``WorkspaceId.value`` — the same string that names the devpod
workspace. A bare branch name would be unique only *within* its parent, which is
what let a downstream consumer reading one path component collapse every branch of
a repo onto a single identity (kinisi-robotics/kinisi_ros#9766).
"""

import logging
import os
import shutil
import subprocess
from pathlib import Path
from typing import Optional

from ..workspace_id import WorkspaceId, validate_ref_name
from .branch_manager import BranchManager
from .config import WorktreeConfig, get_worktree_config
from .locks import hold_lock
from .models import WorktreeInfo
from .repo_manager import (
    FetchFailed,
    RefMissingOnRemote,
    RepositoryManager,
    Updated,
    unhandled_fetch_outcome,
)
from .storage import MetadataStorage

# Every git-lfs pointer file starts with this; see the git-lfs pointer spec.
_LFS_POINTER_PREFIX = b"version https://git-lfs"

logger = logging.getLogger(__name__)


def _on_disk(path: Path) -> Optional[bool]:
    """Whether *path* is there: True, False, or None for "could not look".

    Written out rather than left as ``Path.exists()`` for the reason
    :func:`read_clone` gives at length about ``Path.is_dir()``, which this
    repeated one layer out and got caught by the Python matrix rather than
    locally: ``exists()`` swallows ENOENT, ENOTDIR, EBADF and ELOOP and
    **re-raises everything else**, so a clone whose parent is mode ``000``
    raised ``PermissionError`` straight out of here on 3.10-3.13 while
    3.14 returned False. One expression, two behaviours, and the 3.14 one is
    the dangerous half: "not there" is what sends this method off to derive
    a different directory.
    """
    try:
        os.stat(path)
        return True
    except (FileNotFoundError, NotADirectoryError):
        return False
    except (OSError, ValueError):
        return None


class WorkspaceCloneManager:
    """Manages local workspace clones for DevPod.

    Directory layout:
        ~/.cache/devlaunch/repos/blooop/devlaunch/
        ├── .bare/                          # bare git repo
        ├── devlaunch-main-zovomobo/            # workspace clone
        └── devlaunch-feature-auth-poliseno/    # workspace clone
    """

    def __init__(
        self,
        config: Optional[WorktreeConfig] = None,
        repo_manager: Optional[RepositoryManager] = None,
        storage: Optional[MetadataStorage] = None,
        branch_manager: Optional[BranchManager] = None,
    ):
        self.config = config or get_worktree_config()
        self.storage = storage or MetadataStorage()

        if repo_manager:
            self.repo_manager = repo_manager
        else:
            self.repo_manager = RepositoryManager(
                repos_dir=Path(self.config.repos_dir),
                storage=self.storage,
                config=self.config,
            )

        self.branch_manager = branch_manager or BranchManager()

    def get_workspace_path(self, owner: str, repo: str, branch: str) -> Path:
        """Get the path for a workspace clone.

        Goes through :class:`WorkspaceId`, so this path cannot be built from an
        unvalidated ref: there is no other way to name the leaf. That closes the
        gap where this method was the one of three ref-consuming paths with no
        guard, because the old validator returned a naked ``str`` that carried no
        evidence of having been checked.

        Raises:
            ValueError: if owner, repo or branch is not a safe git name.
        """
        workspace = WorkspaceId(owner, repo, branch)
        repo_root = self.repo_manager.get_repo_path(workspace.owner, workspace.repo)
        return repo_root / workspace.value

    def resolve_clone_path(self, wt_info: WorktreeInfo) -> Optional[Path]:
        """The one directory *wt_info*'s clone lives in, or None if dl cannot say.

        Every caller that has to name a record's clone comes through here: the
        `dl <ws> rm` guard, the `--ls --json` row, and the delete itself. They
        used to name it separately and could disagree (devlaunch#174). The guard
        read ``local_path`` unconditionally while the delete fell back to the
        derivation whenever that path was not on disk, so a record left pointing
        somewhere stale had the guard clearing an *absent* directory -- "nothing
        there holds anything", per :func:`read_clone` -- while the delete removed
        the derived one, which was the one holding the work. Exit 0, no
        ``--force``, nothing said. A guard is only a guard if it inspects what
        the delete will actually remove.

        The recorded path wins when it is usable, which is what keeps clones
        made before the current id scheme removable with no migration.

        **Usable has to mean absolute, not merely truthy.**
        :meth:`WorktreeInfo.from_dict` builds this field with
        ``Path(data["local_path"])``, and ``Path("")`` is ``Path(".")`` -- which
        is truthy, and whose ``exists()`` is True. An empty recorded path
        therefore passed both of the old tests and sent ``shutil.rmtree`` at
        dl's own working directory: it emptied the directory, including its
        ``.git``, and only then failed on ``os.rmdir(".")``. A clone path is
        always absolute, so anything relative is a record dl cannot honour.

        **None means dl cannot name a directory at all**, which happens when the
        recorded path is unusable *and* the derivation refuses the record's own
        owner/repo/branch. Every caller has to treat that as a refusal rather
        than as an empty answer, which is why it is None rather than an
        exception: :meth:`get_workspace_path` raises ``ValueError`` on an unsafe
        ref, and one hand-edited record must not be able to take down the whole
        of ``dl --ls --json`` -- the same harm :func:`read_clone`'s stat guard
        exists to prevent, one layer out.
        """
        recorded = None if wt_info.local_path is None else Path(wt_info.local_path)
        if recorded is not None and recorded.is_absolute() and _on_disk(recorded) is not False:
            # True, or "dl was not allowed to look". Both keep the record: a
            # path dl cannot stat is not a path it has established the absence
            # of, and deriving a *different* directory off the back of that
            # would put the guard and the delete back on two answers -- which is
            # the whole defect. `read_clone` then answers `CouldNotTell` for it
            # and the delete stops, which is the right end for both.
            return recorded
        try:
            return self.get_workspace_path(wt_info.owner, wt_info.repo, wt_info.branch)
        except ValueError as e:
            # Named by the triple the derivation refused rather than by the
            # workspace id: that triple *is* what failed, and it is the field
            # a hand-edited metadata.json would have to be fixed in.
            logger.warning(
                f"cannot name the clone directory for "
                f"{wt_info.owner}/{wt_info.repo}@{wt_info.branch}: {e}"
            )
            return None

    def workspace_exists(self, owner: str, repo: str, branch: str) -> bool:
        """Check if a workspace clone exists."""
        ws_path = self.get_workspace_path(owner, repo, branch)
        return ws_path.exists() and (ws_path / ".git").exists()

    def _remote_ref_exists(self, ws_path: Path, branch: str, remote: str = "origin") -> bool:
        """Check if a remote tracking ref exists in a workspace.

        Validates both names with the same predicate the id constructor uses: the
        default branch reaches here from stored metadata rather than from a
        ``WorkspaceId``, so this is the one ref that still arrives unproven.
        """
        validate_ref_name(remote, "remote")
        validate_ref_name(branch)
        result = subprocess.run(
            [
                "git",
                "show-ref",
                "--verify",
                f"refs/remotes/{remote}/{branch}",
            ],
            cwd=ws_path,
            capture_output=True,
            text=True,
            check=False,
        )
        return result.returncode == 0

    @staticmethod
    def _is_lfs_pointer(path: Path) -> bool:
        """True if *path* holds an unmaterialized git-lfs pointer.

        A path that will not open is not a pointer. Every ordinary workspace has
        several — a deleted file, a dangling symlink, a submodule's directory, a
        path a sparse checkout leaves off disk — and none of them says anything
        about LFS. Answering True instead is not a harmless over-estimate: it
        reinstates the git-lfs fork at the gate, and at the materialization call
        site it drives ``git lfs pull origin`` — unbounded and uncaptured — on
        every launch of such a workspace, forever, since the pull cannot put a
        path the checkout excludes back on disk.
        """
        try:
            with open(path, "rb") as f:
                return f.read(len(_LFS_POINTER_PREFIX)) == _LFS_POINTER_PREFIX
        except OSError:
            return False

    @classmethod
    def _may_hold_lfs_pointers(cls, ws_path: Path) -> bool:
        """True unless nothing git-lfs could name holds a pointer.

        A necessary condition for _has_lfs_pointers, standing in front of the
        git-lfs fork. ``git lfs ls-files`` reports the union of HEAD's tree and
        the index, and ``--with-tree=HEAD`` is what makes ``git ls-files``
        enumerate that same union — so if none of those paths holds a pointer
        the probe would answer False anyway, and forking git-lfs to hear it is
        pure cost, which the overwhelmingly common non-LFS repo pays on every
        single launch.

        Cheaper than the probe, not free: one fork plus the first few bytes of
        each listed path. It is the same O(tracked files) shape as the probe it
        stands in front of, at a much smaller constant.

        The union is load-bearing. The index alone is a strictly smaller set
        than what git-lfs can name, and the gap is reachable with no user
        action: a clone left with no ``.git/index`` — an interrupted clone or
        checkout, exactly what the materialization retry exists to recover from
        — makes ``git ls-files`` exit *zero with empty output*, and reading that
        as "nothing tracked, so no pointers" would strand the workspace on stub
        files on every later launch.

        Deliberately a question about pointer *content*, not about declarations:
        a repo can hold committed pointers with no ``filter=lfs`` attribute of
        its own, and can be LFS-tracked through attributes git reads from
        outside the clone. Reading either as "no LFS here" would leave such a
        workspace on stub files permanently. Content is the thing the caller
        actually needs to know, and it is also the thing that stops being true
        once materialization succeeds.

        Fails open: paths that cannot be enumerated mean "can't tell", not "no
        LFS", so the probe runs — the same degradation _lfs_tracked_files
        refuses. An unborn HEAD lands there too, and pays one probe to be told
        that a repo with no commits holds nothing.
        """
        result = subprocess.run(
            ["git", "ls-files", "-z", "--with-tree=HEAD"],
            cwd=ws_path,
            capture_output=True,
            check=False,
        )
        if result.returncode != 0:
            logger.warning(f"Could not list tracked files: {result.stderr.decode().strip()}")
            return True
        return any(
            cls._is_lfs_pointer(ws_path / os.fsdecode(name))
            for name in result.stdout.split(b"\0")
            if name
        )

    @staticmethod
    def _lfs_tracked_files(ws_path: Path) -> list[str]:
        """Paths in the tree that git-lfs tracks."""
        result = subprocess.run(
            ["git", "lfs", "ls-files", "--name-only"],
            cwd=ws_path,
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode != 0:
            # Don't degrade silently to "no LFS here" — that would ship a tree of
            # pointer files as though it were complete.
            logger.warning(f"Could not list git-lfs files: {result.stderr.strip()}")
            return []
        return [line for line in result.stdout.splitlines() if line]

    @classmethod
    def _has_lfs_pointers(cls, ws_path: Path) -> bool:
        """True if any LFS-tracked file is still an unmaterialized pointer.

        Checked by content rather than by "did we just clone this", so an
        interrupted or failed materialization is retried on the next run instead
        of leaving the workspace on pointer files for good.

        The working-tree scan runs first and can only rule the answer out, never
        in, so the git-lfs probe still decides which pointer-shaped files are
        really LFS — the answer is what it always was, minus a fork nobody
        needed.
        """
        if shutil.which("git-lfs") is None:
            return False
        if not cls._may_hold_lfs_pointers(ws_path):
            return False
        return any(cls._is_lfs_pointer(ws_path / name) for name in cls._lfs_tracked_files(ws_path))

    def _materialize_lfs(self, ws_path: Path) -> None:
        """Replace LFS pointer files with real content from the origin remote.

        The workspace is cloned from the local bare cache with
        GIT_LFS_SKIP_SMUDGE=1 (the cache has no LFS objects), so after the origin
        URL is fixed to point at the real remote the pointers must be materialized
        explicitly — a same-commit checkout won't rewrite them.

        Output is not captured: a multi-gigabyte fetch has to be able to show
        progress rather than look like a hang.
        """
        if not self._has_lfs_pointers(ws_path):
            return
        logger.info("Fetching git-lfs objects from origin")
        try:
            subprocess.run(["git", "lfs", "pull", "origin"], cwd=ws_path, check=True)
        except subprocess.CalledProcessError as e:
            raise RuntimeError(
                f"Failed to pull git-lfs objects (exit {e.returncode}). The workspace "
                f"still holds pointer files; re-run to retry."
            ) from e

    def ensure_branch(self, owner: str, repo: str, branch: str) -> None:
        """Ensure a branch exists in the bare repo, at the remote's current tip.

        This is the whole of the launch path's network use, and the staleness
        contract devlaunch#144 settled is stated here because this is the code
        that keeps it:

        - **Push upstream, then immediately dl the branch → you get the pushed
          tip.** One targeted fetch of the requested ref, every time, no interval
          gate.
        - **A branch that exists nowhere yet** is created from the default
          branch's freshly fetched tip — one more targeted fetch, and no more
          than one.
        - **Offline, or any other fetch failure:** warn and carry on with
          whatever the cache holds. The launch of an already-cached branch still
          works. When there is nothing to launch from at all, the branch
          creation at the end of this method raises — it is the first thing
          that actually consults the cache, so that is where its emptiness is
          discovered.
        - **Every other ref** (other branches, tags, prunes) converges within
          ``fetch_interval`` via the detached updater sweep, blocking nobody.

        Does not push to the remote.

        Runs under the repo lock: the fetch and the branch creation both write
        refs in the shared bare repo, and two processes doing so at once trip
        over git's own ref locks. (hold_lock is not reentrant; no callee here
        takes the repo lock -- `fetch_ref` documents that it expects to be
        called with it held.)
        """
        bare_path = self.repo_manager.get_bare_path(owner, repo)
        with hold_lock(
            self.repo_manager.lock_path(owner, repo),
            waiting_note=f"another dl run preparing {owner}/{repo}",
        ):
            outcome = self.repo_manager.fetch_ref(owner, repo, branch)

            try:
                default_branch = self.repo_manager.get_default_branch(owner, repo)
            except (RuntimeError, subprocess.CalledProcessError, OSError) as e:
                logger.warning(f"Failed to resolve default branch: {e}")
                default_branch = None

            # Each arm named, so a fourth one cannot be silently read as one of
            # these three.
            if isinstance(outcome, Updated):
                pass
            elif isinstance(outcome, RefMissingOnRemote):
                # The remote answered, so the branch really is new: base it on the
                # default branch, and fetch *that* so it is based on something
                # current. Whatever this second fetch answers, the branch creation
                # below proceeds -- there is no third ref to fall back to, and the
                # cache's own default branch is the best remaining start point.
                if default_branch:
                    try:
                        base_outcome = self.repo_manager.fetch_ref(owner, repo, default_branch)
                    except ValueError as e:
                        # Unlike `branch`, this name is read back from metadata.json
                        # and carries no proof. Caught rather than propagated so a
                        # hand-edited record cannot change what this method raises:
                        # dl's launch path guards it with (RuntimeError, OSError),
                        # so a ValueError here would surface as a traceback.
                        logger.warning(f"Cannot fetch recorded default branch: {e}")
                    else:
                        if isinstance(base_outcome, FetchFailed):
                            # Same condition as the arm below, warned the same
                            # way: the new branch is about to be cut from a
                            # possibly stale cache, and the reason is carried
                            # precisely so it can be printed here.
                            logger.warning(
                                f"Could not fetch {default_branch} for {owner}/{repo}: "
                                f"{base_outcome.reason}"
                            )
            elif isinstance(outcome, FetchFailed):
                # Not an error here: a cached branch still launches. Deliberately
                # no default-branch fetch -- nothing was learned about the remote,
                # so nothing licenses treating this branch as new.
                logger.warning(f"Could not fetch {branch} for {owner}/{repo}: {outcome.reason}")
            else:
                unhandled_fetch_outcome(outcome)

            self.branch_manager.ensure_branch_exists(
                bare_path,
                branch,
                create_remote=False,
                start_point=default_branch or "HEAD",
                use_local_refs=True,
            )

    def ensure_workspace(
        self,
        owner: str,
        repo: str,
        branch: str,
        remote_url: str,
    ) -> Path:
        """Ensure a workspace clone exists and is on the right branch.

        1. Ensure the bare reference repo is cloned/fetched
        2. Clone from bare repo to workspace path (if not already there)
        3. Fix remote URL to point to GitHub (not the bare repo)
        4. Checkout the requested branch
        5. Track workspace in metadata for deletion

        The workspace id written to metadata is derived here rather than passed in.
        It has to equal the clone directory's leaf name for later lookups to find
        the clone, and an id-shaped argument could disagree with it silently.

        Returns the workspace path.
        """
        workspace = WorkspaceId(owner, repo, branch)
        # Step 1: Ensure bare reference repo exists
        self.repo_manager.ensure_repo(owner, repo, remote_url)
        bare_repo_path = self.repo_manager.get_bare_path(owner, repo)

        ws_path = self.get_workspace_path(owner, repo, branch)

        # Steps 2-5 mutate the workspace clone, so they run under the repo
        # lock: fire the same workspace twice at once and, unserialized, each
        # process saw no clone, both cloned into the same path, and the loser's
        # cleanup deleted the winner's. The lock is taken only after
        # ensure_repo (which takes the same lock) has returned -- hold_lock is
        # not reentrant.
        with hold_lock(
            self.repo_manager.lock_path(owner, repo),
            waiting_note=f"another dl run preparing {owner}/{repo}",
        ):
            return self._prepare_workspace(workspace, bare_repo_path, ws_path, remote_url)

    def _prepare_workspace(
        self,
        workspace: WorkspaceId,
        bare_repo_path: Path,
        ws_path: Path,
        remote_url: str,
    ) -> Path:
        """Steps 2-5 of ensure_workspace; the caller holds the repo lock."""
        owner, repo, branch = workspace.owner, workspace.repo, workspace.ref
        is_new_workspace = False
        if not self.workspace_exists(owner, repo, branch):
            is_new_workspace = True
            # Step 2: Clone from bare repo
            logger.info(f"Creating workspace clone at {ws_path}")
            ws_path.parent.mkdir(parents=True, exist_ok=True)

            try:
                # Skip LFS smudge during clone: the clone source is the local
                # bare cache, which has no LFS objects, so smudging here fails
                # with "remote missing object". LFS content is pulled from the
                # real remote after the origin URL is fixed (see below).
                clone_env = os.environ.copy()
                clone_env["GIT_LFS_SKIP_SMUDGE"] = "1"
                subprocess.run(
                    ["git", "clone", str(bare_repo_path), str(ws_path)],
                    capture_output=True,
                    text=True,
                    check=True,
                    env=clone_env,
                )
            except subprocess.CalledProcessError as e:
                logger.error(f"Failed to clone workspace: {e.stderr}")
                if ws_path.exists():
                    shutil.rmtree(ws_path)
                raise RuntimeError(f"Failed to clone workspace: {e.stderr}") from e

            # Step 3: Fix remote URL to point to GitHub
            try:
                subprocess.run(
                    ["git", "remote", "set-url", "origin", remote_url],
                    cwd=ws_path,
                    capture_output=True,
                    text=True,
                    check=True,
                )
            except subprocess.CalledProcessError as e:
                logger.error(f"Failed to set remote URL: {e.stderr}")
                raise RuntimeError(f"Failed to set remote URL: {e.stderr}") from e

        # No fetch in the workspace clone. A new one was just cloned from a bare
        # cache that ensure_branch had already fetched the requested ref into, and
        # for an existing one the fetch's output was never read: the checkout
        # below is a plain `git checkout <branch>` against the local branch and
        # consults no remote-tracking ref. It was a network round-trip per launch
        # that bought nothing (devlaunch#144).

        # Step 4: Checkout branch
        try:
            if is_new_workspace:
                # For new workspaces, reset the branch to the remote ref to
                # ensure we start from the latest commit, not a stale clone.
                # No validation call here: `workspace` is the proof.
                if self._remote_ref_exists(ws_path, workspace.ref):
                    checkout_cmd = ["git", "checkout", "-B", branch, f"origin/{branch}"]
                else:
                    base_repo = self.repo_manager.get_repo(owner, repo)
                    default_branch = base_repo.default_branch if base_repo else "main"
                    if not self._remote_ref_exists(ws_path, default_branch):
                        raise RuntimeError(
                            f"Cannot create branch '{branch}': neither "
                            f"'origin/{branch}' nor 'origin/{default_branch}' "
                            f"exist on the remote"
                        )
                    checkout_cmd = [
                        "git",
                        "checkout",
                        "-B",
                        branch,
                        f"origin/{default_branch}",
                    ]
            else:
                # Existing workspace: plain checkout preserves local work
                checkout_cmd = ["git", "checkout", branch]
            subprocess.run(
                checkout_cmd,
                cwd=ws_path,
                capture_output=True,
                text=True,
                check=True,
            )
        except subprocess.CalledProcessError as e:
            logger.error(f"Failed to checkout branch '{branch}': {e.stderr}")
            raise RuntimeError(f"Failed to checkout branch '{branch}': {e.stderr}") from e

        # Materialize LFS content. Not gated on is_new_workspace: a failed pull
        # leaves a workspace that already "exists", and gating here would take the
        # existing-workspace path forever after, silently building against pointers.
        self._materialize_lfs(ws_path)

        # Step 5: Track in metadata
        try:
            wt_info = WorktreeInfo(
                owner=owner,
                repo=repo,
                branch=branch,
                local_path=ws_path,
                workspace_id=workspace.value,
            )
            self.storage.add_worktree(wt_info)
        except Exception as e:
            logger.warning(f"Failed to save workspace metadata: {e}")

        return ws_path

    def remove_workspace(self, owner: str, repo: str, branch: str) -> bool:
        """Remove a workspace clone, locating it by deriving its path.

        Returns True if removed, False if it didn't exist.
        """
        return self._remove_clone(self.get_workspace_path(owner, repo, branch), owner, repo, branch)

    def remove_workspace_by_id(self, workspace_id: str) -> bool:
        """Remove a workspace clone by its workspace ID.

        Looks the workspace up in metadata and removes the directory
        :meth:`resolve_clone_path` names — the same one the `dl <ws> rm` guard
        inspected before deciding this was safe to call. Resolving it in one
        place is what makes those two the same directory (devlaunch#174).

        Following the record matters because the derivation has changed: every
        workspace created before the current id scheme has a bare branch name as its
        clone-directory leaf. Re-deriving the leaf here looked for a directory that
        never existed, so removal deleted the devpod workspace and then reported
        failure — orphaning the clone and its metadata entry, silently, because the
        caller only logs on success. The stored path makes old and new workspaces
        both removable with no migration.

        Returns True if removed, False if not found.
        """
        wt_info = self.storage.get_worktree_by_workspace_id(workspace_id)
        if not wt_info:
            return False
        ws_path = self.resolve_clone_path(wt_info)
        if ws_path is None:
            # dl cannot name the directory, so it must not delete one. Same
            # answer as "no record": removes nothing, says so.
            return False
        return self._remove_clone(ws_path, wt_info.owner, wt_info.repo, wt_info.branch)

    def _remove_clone(self, ws_path: Path, owner: str, repo: str, branch: str) -> bool:
        """Delete *ws_path* and its metadata entry. False if it was not there."""
        if not ws_path.exists():
            logger.info(f"No workspace clone to remove at {ws_path}")
            return False
        shutil.rmtree(ws_path)
        logger.info(f"Removed workspace clone: {ws_path}")
        # Clean up metadata
        try:
            self.storage.remove_worktree(owner, repo, branch)
        except Exception as e:
            logger.warning(f"Failed to remove workspace metadata: {e}")
        return True
