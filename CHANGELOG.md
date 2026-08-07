# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.8] - 2026-08-07

### Added
- The host's GitHub CLI login is forwarded into every workspace as `GH_TOKEN`, so
  `gh` works inside whatever container is launched without its devcontainer.json
  arranging anything. The token comes from `GH_TOKEN`, `GITHUB_TOKEN`, or
  `gh auth token`, and reaches devpod through a private file (`devpod up`) and
  devpod's own environment (`devpod ssh`) rather than a command line, so it stays
  out of `ps`. Everything in the container can read it, including a repo's own
  `postCreateCommand`, so set `DEVLAUNCH_NO_GH_TOKEN=1` — for one launch or for the
  machine — to opt out.

### Fixed
- A corrupt `metadata.json` no longer takes down every `dl` command, `dl --help`
  included. Loading is total now: an unreadable or non-object file is quarantined
  to `metadata.json.corrupt` and load continues with empty state, a single
  malformed entry is skipped instead of the whole file, and an entry carrying a
  field only a newer build declares loads without that field rather than failing.
  Any load that drops information copies the original to `metadata.json.bak`
  before the next write can overwrite it, and says so on stderr.
- On a box without devpod, workspace commands print one line on stderr and exit
  127 instead of a raw `FileNotFoundError` traceback. `--help`, `--version` and
  the completion paths never touch devpod and still work; `--update-cache` now
  leaves a good cache in place rather than overwriting it with an empty one.

### Removed
- Deletion-only hygiene pass, no behavior change: template leftovers from the
  python_template origin (`PROMPT.md`, `ralph.yml`, `@fix_plan.md`, `@AGENT.md`,
  `WORKTREE_BACKEND_PLAN.md`, `WORKTREE_BACKEND_README.md`) and dead code with no
  references from source or tests — `dl.get_git_branches`, `dl.workspace_status`,
  `dl.get_remote_head_sha`, `worktree.config.save_config`,
  `BranchManager.checkout_branch` and `BranchManager.create_remote_branch_via_ssh`.
- The README's "Backend Selection" section, which documented a `--backend` flag
  and `DEVLAUNCH_BACKEND` env var that exist nowhere in the code.

## [0.0.7] - 2026-08-06

### Added
- `--devcontainer <variant|path>` to select a non-default `devcontainer.json`, for
  repos carrying several variants. A bare name expands to the spec's
  `.devcontainer/<name>/devcontainer.json`; a path is used as given. Accepts
  `--devcontainer=x` too, and tab-completes the repo's variant directories. devpod
  stores the choice with the workspace, so it only has to be passed once.
- `DEVLAUNCH_WORKSPACE_ID` is injected into workspace initialization (via devpod's
  `--init-env`), so a project's host-side `initializeCommand` can tell branch
  workspaces apart. devpod passes the hook no workspace identity of its own, and
  devlaunch clones every branch to `<repo>/<branch>`, so a project deriving
  per-checkout names from the path cannot distinguish them. See
  `docs/devcontainer-projects.md`.
- Worktree backend for efficient multi-branch workspace management
  - Clones repositories once, then creates git worktrees for each branch
  - Shares git objects across all branches for faster workspace creation
  - Automatic backend selection based on workspace spec (owner/repo format uses worktree)
  - Backend override via `--backend worktree|devpod` flag or `DEVLAUNCH_BACKEND` env var
- New worktree module with:
  - `RepositoryManager` for cloning and managing base repositories
  - `WorktreeManager` for creating and managing git worktrees
  - `WorkspaceManager` for DevPod workspace lifecycle with worktree backing
  - `BranchManager` for branch operations (create, track, push)
  - `MetadataStorage` for persistent worktree tracking
- Configurable worktree directories via `~/.config/devlaunch/config.toml`
- `--purge` command to remove all devlaunch data (repos, worktrees, caches)
- All data now stored in `~/.cache/devlaunch/` (XDG compliant)

### Fixed
- Cloning a git-lfs repository no longer fails during checkout. Workspaces are
  cloned from the local bare cache, which holds no LFS objects, so the smudge
  filter aborted; LFS content is now pulled from the real remote after the origin
  URL is set. A failed or interrupted pull is retried on the next run — whether
  content is missing is decided by looking for pointer files, so a workspace
  cannot get stuck holding pointers.
- `dl <ws>` no longer starts the session in `$HOME` for projects that set a custom
  `workspaceFolder`. It passed a guessed `--workdir /workspaces/<id>`, and devpod
  falls back to `$HOME` when that path does not exist in the container.
- `dl <ws>` no longer opens VS Code on top of the terminal shell it attaches when
  devpod's default IDE is configured. `dl <ws> code` is unaffected.
- A failed `devpod delete` no longer strands a workspace. devpod re-parses the
  workspace's `devcontainer.json` to tear the container down, so deletion fails if
  that file has moved — and the local clone was removed regardless, leaving devpod
  with no config to retry from. The clone is now kept unless devpod succeeded.
- Proper exception handling for workspace creation failures
- Pylint compliance for all worktree module code

### Removed
- `devlaunch.dl.get_container_workdir()`. It built a guessed container path that
  is no longer passed to `devpod ssh` (see the `workspaceFolder` fix above), so it
  had no correct use. `workspace_ssh(workdir=...)` still accepts an explicit
  override.

## [0.0.4] - 2026-01-18

### Added
- Branch completion and auto-creation for `dl` command
- Support for multiple branch workspaces

### Fixed
- Use SSH for git operations instead of HTTPS
- Type checker None check in tests

## [0.0.3] - 2026-01-17

### Changed
- Updated README to match current CLI syntax and `--help` output

### Added
- PyPI badge to README

## [0.0.2] - 2026-01-17

### Added
- `--version` flag to display version information
- Comprehensive tests and improved coverage

### Changed
- CLI to workspace-first syntax (`dl <workspace> <command>`)
- Reorganized restart/reset/recreate commands

### Removed
- `nocache` command (devpod doesn't support it)

## [0.0.1] - 2026-01-17

### Added
- Initial release of DevLaunch
- `dl` CLI wrapper for devpod workspaces
- Commands: `up`, `ssh`, `stop`, `delete`, `status`, `restart`, `reset`, `recreate`
- Shell completion support with `--install` flag
- Fuzzy workspace selection via `iterfzf`
