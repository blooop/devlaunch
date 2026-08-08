# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

The Rust rewrite is deferred, and Python remains the implementation. 0.0.11 called
itself the last release of the Python implementation, on the go decision recorded in
[#53](https://github.com/blooop/devlaunch/issues/53); 0.0.12 and 0.0.13 have shipped
since, and there is no cutoff to plan around now. Nothing about how you install or run
`dl` changes, and no release is withdrawn — this only retires an expectation the
changelog had set.

What it does change is what is worth doing to this codebase. Work that was ruled out as
wasted motion in front of a rewrite — the `dl.py` structural refactor #53 was gating,
and paying down anything scoped as "the Rust version will fix it" — is back on the
table and should be judged on its own merits.

## [0.0.13] - 2026-08-08

One fix: `dl <ws> -- <command>` now gives the command a terminal when you have
one, so interactive programs — a coding agent, a REPL, `git rebase -i` — start
and stay up instead of exiting immediately. This is what made `aid <repo>` return
straight to your shell.

### Fixed
- Interactive commands get a terminal. `devpod ssh --command` never requests a
  pty, so anything it started ran with stdin, stdout and stderr on pipes and
  `TERM=dumb`. Nothing about that looks like a missing terminal from the outside:
  `claude` reads the pipe as a non-interactive invocation, switches to `--print`
  mode and exits, so `aid <repo>` left no session behind and `aid <repo> 'fix it'`
  printed one answer and stopped. `dl` now hands such commands to OpenSSH through
  the `<workspace>.devpod` host alias `devpod up` already writes, with `-t`, which
  also puts window size and SIGWINCH in OpenSSH's hands rather than dl's.

  The choice is the one `ssh` itself makes — a terminal when there is a terminal
  to use — so `dl <ws> -- ls > files.txt` keeps the devpod transport and stays
  free of escape sequences. A workspace with no host alias falls back with a
  warning that says how to republish it, and `DEVLAUNCH_NO_TTY=1` forces the
  fallback everywhere. A bare `dl <ws>` attach is untouched; devpod already gives
  that one a pty.

### Changed
- The devpod floor moves from 0.8 to 0.26.1, in the conda recipe and in the
  development environment. dl's behaviour depends on devpod's, and the two differ
  across that range — 0.8 gives `devpod ssh --command` a pty and 0.26 does not —
  so the suite had been exercising a devpod five years of releases behind the one
  `dl` ships alongside, and could not have reproduced the bug above at all.

## [0.0.12] - 2026-08-08

One fix: `dl` stops reporting a failure every time you leave a workspace, and a
one-shot `dl <ws> -- <command>` now exits with the command's own status instead
of a flat 1. Nothing about how you install or run `dl` changes.

### Fixed
- Leaving a workspace no longer reports a failure. devpod turns any nonzero exit
  from the program it ran into a fatal of its own ("tunnel to container: run in
  container: ssh session: Process exited with status 130") and exits 1, because it
  type-asserts on an `*ssh.ExitError` it has already wrapped. Typing `exit` in a
  shell whose last command was interrupted was enough to trigger it. `dl` now
  reads that status back out and reports it as the session's, so an ordinary exit
  is silent and `dl <ws> -- <command>` propagates the command's real exit code
  instead of a flat 1. Failures that are genuinely devpod's still print in full.

## [0.0.11] - 2026-08-08

This completes the review that ran across [#51](https://github.com/blooop/devlaunch/issues/51):
seven targeted fixes to performance, correctness and maintainability, of which six
shipped in 0.0.10 and the cache migration lands here. It is also intended to be the
last release of the Python implementation — the successor is a Rust rewrite whose core
becomes a library shared with [blooop/wayfinder](https://github.com/blooop/wayfinder),
decided in [#53](https://github.com/blooop/devlaunch/issues/53). Nothing about how you
install or run `dl` changes in this release.

> **Superseded.** The Rust rewrite was deferred on 2026-08-08 and Python remains the
> implementation, so this was not the last Python release — 0.0.12 and 0.0.13 followed.
> The rest of the entry stands as shipped. See [Unreleased](#unreleased).

### Changed
- Existing caches are migrated onto the new workspace id scheme once, by the first
  command that touches a workspace. Clone directories are renamed in place —
  `~/.cache/devlaunch/repos/blooop/devlaunch/main` becomes
  `.../devlaunch-main-zovomobo` — a plain `mv` of a git clone, which carries its `.git`
  with it and refers to its own path nowhere, so history and **uncommitted changes
  survive**; `metadata.json` is updated in the same atomic write and its `version`
  becomes `2`. `dl --help`, `dl --version` and `dl --ls` do not trigger it, and a
  second run does nothing.
- Existing devpod containers keep their old ids and are orphaned, since the new id
  names a new container. dl does not delete containers for you: it prints the count
  and writes the old ids to `~/.cache/devlaunch/orphaned-workspaces.txt`, so
  `xargs -r -n1 devpod delete < ~/.cache/devlaunch/orphaned-workspaces.txt` clears
  them when you are ready. A clone directory with no metadata record cannot be
  renamed — nothing records which branch it holds — so it is left alone and listed
  in `~/.cache/devlaunch/unmigrated-clones.txt`.

### Fixed
- The test suite no longer reads or writes the real `~/.cache/devlaunch`. One test
  reached a code path that builds a real clone manager, which with the migration in
  place would have renamed the developer's own workspace clones.

## [0.0.10] - 2026-08-07

### Added
- `dl --version` reports which install it is. A released build and an editable
  install of the same commit both printed a bare `dl <version>`, so a stale
  released binary was indistinguishable from a working tree at runtime — pulling
  a fix and still seeing the old behaviour read as a failed merge rather than as
  the wrong binary on `PATH`. An editable install now says so and names the tree
  it resolves to. Detection reads PEP 610 `direct_url.json` through
  `importlib.metadata` and is strictly additive: absent, malformed or
  missing-key metadata all fall back to the bare output rather than raising.
  `aid --version` inherits it.

### Fixed
- A corrupt `~/.cache/devlaunch/metadata.json` no longer takes down every `dl`
  command, `dl --help` included. It used to raise while the storage object was being
  built, before any command ran. An unreadable file is now moved aside to
  `metadata.json.corrupt` and dl starts with empty metadata; a single malformed entry
  is skipped rather than costing the whole file; and any load that would drop
  information — a skipped entry, a field only a newer build knows about, a newer
  schema version — copies the original to `metadata.json.bak` before the next write
  can overwrite it, and says so. Saving is atomic, writes through a symlinked
  `metadata.json` rather than replacing the link, and preserves the file's mode.
  The file gains a `version` key.
- `dl` says so when devpod is not installed, instead of printing a
  `FileNotFoundError` traceback. One line naming the install page, and exit `127` —
  the shell's own "command not found" code. `dl --help` and `dl --version` keep
  working without devpod, and the completion commands leave stdout empty, since that
  is what the shell parses.
- `dl <repo> -- <cmd>` runs its command in a login shell, so it gets the same
  `PATH` an interactive `dl <repo>` attach gets. devpod runs a `--command` payload
  under a non-login, non-interactive `bash -c`, which sources neither `~/.profile`
  nor `~/.bashrc` — so `PATH` entries an image adds there (notably
  `$HOME/.pixi/bin`) were missing and the payload died with `command not found`
  and exit 127. This is what made `aid` unable to find `claude` in a workspace
  where `dl` could. dl launches arbitrary repos, so the parity comes from the
  invocation rather than from any particular `devcontainer.json`.

### Changed
- Workspace ids are derived at a single parse boundary, with a wider id suffix, so
  two specs can no longer collide onto one workspace.
- Fewer devpod shell-outs per invocation: the same devpod answer is no longer
  fetched twice, and the completion cache refreshes on a TTL once per invocation
  rather than on every completion. Both cut startup latency.
- A development install from the working tree installs as `dl-next`, leaving a
  released `dl` in place, and reads its entry points from `pyproject.toml`.

## [0.0.9] - 2026-08-07

### Added
- `aid`, a second entry point that opens a workspace and starts a coding agent in
  it: `aid owner/repo@branch fix the flaky test`. It is a shortcut, not a second
  launcher — it rewrites its command line into `dl owner/repo@branch -- claude
  'fix the flaky test'` and hands that to `dl`, so an `aid` workspace is the `dl`
  workspace: same clone, same workspace id, same container, reused rather than
  rebuilt. Pick the agent with `--claude` (default), `--codex` or `--gemini`, or
  set `DEVLAUNCH_AID_AGENT`; `--devcontainer` passes through, and everything after
  the workspace is the prompt. This replaces the `aid` in `blooop/rockerc`, which
  ran on rocker and built an image per launch instead of reusing the workspace.
- `dl` and `aid` share one completion function, so `aid` tab-completes the same
  workspaces, repos, owners and branches. Reinstall with `dl --install`.

### Changed
- `dl.main()` takes an optional argv list, so a sibling entry point can hand `dl`
  a command line and get `dl`'s own behaviour rather than a copy of it. Calling it
  with no arguments is unchanged.

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
