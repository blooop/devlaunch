# devlaunch

A streamlined CLI for [devpod](https://devpod.sh) with intuitive autocomplete and fzf fuzzy selection.

## Continuous Integration Status

[![Ci](https://github.com/blooop/devlaunch/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/blooop/devlaunch/actions/workflows/ci.yml?query=branch%3Amain)
[![Codecov](https://codecov.io/gh/blooop/devlaunch/branch/main/graph/badge.svg?token=Y212GW1PG6)](https://codecov.io/gh/blooop/devlaunch)
[![GitHub issues](https://img.shields.io/github/issues/blooop/devlaunch.svg)](https://GitHub.com/blooop/devlaunch/issues/)
[![GitHub pull-requests merged](https://badgen.net/github/merged-prs/blooop/devlaunch)](https://github.com/blooop/devlaunch/pulls?q=is%3Amerged)
[![GitHub release](https://img.shields.io/github/release/blooop/devlaunch.svg)](https://GitHub.com/blooop/devlaunch/releases/)
[![PyPI](https://img.shields.io/pypi/v/devlaunch)](https://pypi.org/project/devlaunch/)
[![Conda](https://img.shields.io/badge/conda-v0.0.5-brightgreen?logo=anaconda)](https://prefix.dev/channels/blooop/packages/devlaunch)
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

### Shell Completions

After installation, set up shell completions:

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

## Worktree Backend

For git repositories, devlaunch uses an efficient worktree backend by default:

- **Efficient Storage**: Repos are cloned once to `~/.cache/devlaunch/repos/owner/repo/`, then git worktrees are created for each branch
- **Shared Git Objects**: All branches share git objects, saving disk space
- **Lazy Fetch**: Remote updates are only fetched if the configured interval has elapsed (default: 1 hour)
- **Short Prompts**: Your terminal prompt shows `~/work` instead of long worktree paths

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

### Backend Selection

```bash
dl --backend devpod owner/repo  # Force legacy DevPod backend
DEVLAUNCH_BACKEND=devpod dl owner/repo  # Use environment variable
```

## Shell Completion

After running `dl --install`, you get intelligent tab completion:

- Workspace names from your devpod list
- Known GitHub owners and repositories from your workspaces
- File/directory paths when starting with `./`, `/`, or `~`
- All global flags (`--ls`, `--install`, etc.) and workspace commands

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
