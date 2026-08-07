# devlaunch

A streamlined CLI for [devpod](https://devpod.sh) with intuitive autocomplete and fzf fuzzy selection.

## Continuous Integration Status

[![Ci](https://github.com/blooop/devlaunch/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/blooop/devlaunch/actions/workflows/ci.yml?query=branch%3Amain)
[![Codecov](https://codecov.io/gh/blooop/devlaunch/branch/main/graph/badge.svg?token=Y212GW1PG6)](https://codecov.io/gh/blooop/devlaunch)
[![GitHub issues](https://img.shields.io/github/issues/blooop/devlaunch.svg)](https://GitHub.com/blooop/devlaunch/issues/)
[![GitHub pull-requests merged](https://badgen.net/github/merged-prs/blooop/devlaunch)](https://github.com/blooop/devlaunch/pulls?q=is%3Amerged)
[![GitHub release](https://img.shields.io/github/release/blooop/devlaunch.svg)](https://GitHub.com/blooop/devlaunch/releases/)
[![PyPI](https://img.shields.io/pypi/v/devlaunch)](https://pypi.org/project/devlaunch/)
[![Conda](https://img.shields.io/badge/conda-v0.0.9-brightgreen?logo=anaconda)](https://prefix.dev/channels/blooop/packages/devlaunch)
[![License](https://img.shields.io/github/license/blooop/devlaunch)](https://opensource.org/license/mit/)
[![Python](https://img.shields.io/badge/python-3.10%20%7C%203.11%20%7C%203.12%20%7C%203.13-blue)](https://www.python.org/downloads/)
[![Pixi Badge](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/prefix-dev/pixi/main/assets/badge/v0.json)](https://pixi.sh)

## Installation

### Pixi (Recommended)

```bash
pixi global install --channel conda-forge --channel https://prefix.dev/blooop devlaunch
```

This installs `devlaunch` along with `devpod` and all dependencies automatically.

### Pip

```bash
pip install devlaunch
```

Note: When using pip, you must install [devpod](https://devpod.sh/docs/getting-started/install) separately.
If `devpod` is not on `PATH`, every command that needs it prints a single install hint on stderr and exits `127`
(the shell's "command not found" code). `dl --help` and `dl --version` keep working without it.

### Shell Completions

After installation, set up shell completions for `dl` and `aid`:

```bash
dl --install
source ~/.bashrc  # or restart your terminal
```

## Usage

```bash
dl                               # Interactive workspace selector (fzf)
dl <user/repo>                   # Start workspace and attach shell
dl <user/repo> <cmd>             # Run workspace command (stop, code, etc.)
dl <user/repo> -- <command>      # Run shell command in workspace
```

## aid: start a coding agent in a workspace

`aid` is `dl` with a coding agent started for you:

```bash
aid <user/repo>[@branch] [prompt...]   # Open the workspace, start the agent
```

It is a shortcut, not a second launcher. `aid` rewrites its command line into a
`dl` one and hands it to `dl` itself, so

```bash
aid blooop/devlaunch@fix/42 fix the flaky test
```

is exactly

```bash
dl blooop/devlaunch@fix/42 -- claude 'fix the flaky test'
```

That means an `aid` workspace *is* the `dl` workspace: same clone, same workspace
id, same container — started if stopped, attached to if already running, and never
rebuilt just because `aid` asked for it. Anything `dl` learns, `aid` gets.

| Option | Description |
|--------|-------------|
| `--claude`, `--codex`, `--gemini` | Pick the agent (default: `claude`) |
| `--devcontainer <variant\|path>` | Passed through to `dl` |
| `DEVLAUNCH_AID_AGENT=<agent>` | Change the default agent |

Everything after the workspace is the prompt, flags and all, so it never needs
quoting to survive `aid`'s own parsing. Managing workspaces — listing, stopping,
deleting, VS Code — stays with `dl`.

The agent's CLI has to be installed in the container; `aid` runs it there, it does
not install it.

## Workspace Sources

```bash
dl myproject                     # Existing workspace by name
dl user/repo                     # Create from GitHub repo
dl user/repo@branch              # Create from specific branch
dl ./path                        # Create from local path
```

## Workspace Commands

| Command | Description |
|---------|-------------|
| `dl <user/repo> stop` | Stop the workspace |
| `dl <user/repo> rm, prune` | Delete the workspace |
| `dl <user/repo> code` | Open in VS Code |
| `dl <user/repo> restart` | Stop and start (no rebuild) |
| `dl <user/repo> recreate` | Recreate container |
| `dl <user/repo> reset` | Clean slate (remove all, recreate) |
| `dl <user/repo> -- <command>` | Run shell command in workspace |

## Options

| Option | Description |
|--------|-------------|
| `--devcontainer <variant\|path>` | Use a non-default `devcontainer.json`. A bare name means `.devcontainer/<name>/devcontainer.json`. Stored with the workspace, so pass it once. |

Projects with demanding devcontainers — several variants, compose sidecars, or a
host-side `initializeCommand` that has to tell branch workspaces apart — are
covered in [docs/devcontainer-projects.md](docs/devcontainer-projects.md).

## GitHub Authentication

Every workspace `dl` opens inherits the host's GitHub login, so `gh` is already
authenticated inside the container and the devcontainer.json does not have to
arrange anything for it. devpod forwards the ssh agent and git credentials on its
own, but nothing else carries `gh`.

devlaunch takes the token from `GH_TOKEN`, `GITHUB_TOKEN`, or `gh auth token`,
whichever answers first, and hands it to the container as `GH_TOKEN`. That reaches
any image and any container user, unlike a bind-mount of `~/.config/gh`, and it
works whether the host keeps its token in `hosts.yml` or in a keyring. The token
is passed to devpod through a private file and through devpod's own environment,
never on a command line, so it does not appear in `ps`. The container still needs
`gh` installed for the login to be of any use. Check a workspace with:

```bash
dl <workspace> -- gh auth status
```

### Who gets the token

Everything running in the container does — including a `postCreateCommand` from a
repo you did not write. `dl someone/repo` builds and runs that project's
devcontainer with your GitHub token in its environment, and a `gh auth login` token
usually carries `repo`, `workflow`, `gist` and `read:org` scopes. devpod already
forwards the ssh agent to every workspace, so this is not a new trust boundary, but
it is a wider one. Skip it for a repo you have not read:

```bash
DEVLAUNCH_NO_GH_TOKEN=1 dl someone/repo
```

| Variable | Description |
|----------|-------------|
| `DEVLAUNCH_NO_GH_TOKEN=1` | Do not forward the host's GitHub login into workspaces |

### When the token changes

`dl` refreshes the token on every start, so rotating it on the host is enough for
any workspace that gets started or restarted afterwards. Attaching to a workspace
that is *already running* skips that step, and the token it was given at startup
stays in place — including one it was given before you set
`DEVLAUNCH_NO_GH_TOKEN`. Run `dl <workspace> restart` to replace it.

## Global Commands

| Command | Description |
|---------|-------------|
| `dl --ls` | List all workspaces |
| `dl --install` | Install shell completions |
| `dl --purge [-y]` | Remove all devlaunch data |
| `dl --prune-worktrees [days]` | Remove unused worktrees (default: 30 days) |
| `dl --refresh` | Refresh completion cache |
| `dl --help, -h` | Show this help |
| `dl --version` | Show version |

## Examples

```bash
dl                               # Select workspace with fzf
dl devpod                        # Open existing workspace
dl loft-sh/devpod                # Create from GitHub
dl blooop/devlaunch@main         # Create from specific branch
dl ./my-project                  # Create from local folder
dl blooop/devlaunch code         # Open in VS Code
dl blooop/devlaunch -- make test # Run command in workspace
dl blooop/devlaunch stop         # Stop workspace
```

## Features

- **Fuzzy Selection**: When called without arguments, uses fzf for interactive workspace selection
- **Smart Completion**: Tab completion for workspaces, GitHub repos (owner/repo format), and paths
- **GitHub Shorthand**: Use `owner/repo` instead of full URLs - automatically expands to `github.com/owner/repo`
- **Branch Support**: Specify branches with `owner/repo@branch` syntax
- **Fast Autocomplete**: Completion cache for ~3ms response time (vs ~700ms without cache)
- **One Round-Trip Per Question**: every `devpod` call costs ~0.45s, far more than `dl` itself, so a command reads the workspace list at most once — and `dl <ws> -- <cmd>` skips the extra round-trip that names an interactive prompt, since a one-shot command has none

## Worktree Backend

For git repositories, devlaunch uses an efficient worktree backend by default:

- **Efficient Storage**: Repos are cloned once to `~/.cache/devlaunch/repos/owner/repo/`, then git worktrees are created for each branch
- **Shared Git Objects**: All branches share git objects, saving disk space
- **Lazy Fetch**: Remote updates are only fetched if the configured interval has elapsed (default: 1 hour)

### Container Sharing Mode

Use `--shared` to share a single container across multiple branches of the same repo:

```bash
dl --shared owner/repo@branch1  # Creates container "owner-repo"
dl --shared owner/repo@branch2  # Reuses "owner-repo" container
```

### Pre-warming

Use `--warm` to prepare a workspace without attaching a shell:

```bash
dl --warm owner/repo@branch  # Creates container in background
```

## Shell Completion

After running `dl --install`, you get intelligent tab completion:

- Workspace names from your devpod list
- Known GitHub owners and repositories from your workspaces
- File/directory paths when starting with `./`, `/`, or `~`
- All global flags (`--ls`, `--install`, etc.) and workspace commands

### How the completion cache stays current

The data behind completions lives in `~/.cache/devlaunch/completions.json`, and
building it means a `git ls-remote` per known repo — seconds of work. So it is
rebuilt in the background at most once an hour (the same interval the worktree
backend uses for lazy fetches), and at most once per `dl` invocation. Commands
that change your workspaces (starting, stopping or deleting one) rebuild it as
soon as they finish, regardless of when it was last built. Commands with no use
for it — `dl --help`, `dl --version` — do not touch it at all.

A branch created on a remote in the last hour may therefore not be offered yet.
`dl --refresh` rebuilds the cache immediately and ignores the interval.

## Development

This project uses [pixi](https://pixi.sh) for environment management.

```bash
# Run tests
pixi run test

# Run full CI suite
pixi run ci

# Format and lint
pixi run style
```
