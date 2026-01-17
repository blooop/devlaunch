# devlaunch

A streamlined CLI for [devpod](https://devpod.sh) with intuitive autocomplete and fzf fuzzy selection.

## Continuous Integration Status

[![Ci](https://github.com/blooop/devlaunch/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/blooop/devlaunch/actions/workflows/ci.yml?query=branch%3Amain)
[![Codecov](https://codecov.io/gh/blooop/devlaunch/branch/main/graph/badge.svg?token=Y212GW1PG6)](https://codecov.io/gh/blooop/devlaunch)
[![GitHub issues](https://img.shields.io/github/issues/blooop/devlaunch.svg)](https://GitHub.com/blooop/devlaunch/issues/)
[![GitHub pull-requests merged](https://badgen.net/github/merged-prs/blooop/devlaunch)](https://github.com/blooop/devlaunch/pulls?q=is%3Amerged)
[![GitHub release](https://img.shields.io/github/release/blooop/devlaunch.svg)](https://GitHub.com/blooop/devlaunch/releases/)
[![License](https://img.shields.io/github/license/blooop/devlaunch)](https://opensource.org/license/mit/)
[![Python](https://img.shields.io/badge/python-3.10%20%7C%203.11%20%7C%203.12%20%7C%203.13-blue)](https://www.python.org/downloads/)
[![Pixi Badge](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/prefix-dev/pixi/main/assets/badge/v0.json)](https://pixi.sh)

## Installation

```bash
# Using pixi (recommended)
pixi global install devlaunch

# Using pip
pip install devlaunch
```

After installation, set up shell completions:

```bash
dl --install
source ~/.bashrc  # or restart your terminal
```

## Usage

```bash
dl                               # Interactive workspace selector (fzf)
dl <workspace>                   # Start workspace and attach shell
dl <workspace> <cmd>             # Run workspace command (stop, code, etc.)
dl <workspace> -- <command>      # Run shell command in workspace
dl owner/repo                    # Create workspace from GitHub repo
dl owner/repo@branch             # Create workspace from specific branch
dl ./path                        # Create workspace from local path
```

## Workspace Commands

| Command | Description |
|---------|-------------|
| `dl <workspace> stop` | Stop the workspace |
| `dl <workspace> rm` | Delete the workspace (alias: `prune`) |
| `dl <workspace> code` | Open workspace in VS Code |
| `dl <workspace> restart` | Stop and start (no rebuild) |
| `dl <workspace> recreate` | Recreate container |
| `dl <workspace> reset` | Reset workspace (clean slate) |
| `dl <workspace> -- <cmd>` | Run shell command in workspace |

## Global Commands

| Command | Description |
|---------|-------------|
| `dl --ls` | List all workspaces |
| `dl --install` | Install shell completions |
| `dl --help, -h` | Show help |
| `dl --version` | Show version |

## Examples

```bash
# Select workspace interactively with fzf
dl

# Open an existing workspace
dl myproject

# Create workspace from GitHub repository
dl loft-sh/devpod

# Create workspace from specific branch
dl blooop/devlaunch@main

# Create workspace from local folder
dl ./my-project

# Open workspace in VS Code
dl blooop/devlaunch code

# Run a command in workspace
dl blooop/devlaunch -- make test

# Stop a workspace
dl blooop/devlaunch stop
```

## Features

- **Fuzzy Selection**: When called without arguments, uses fzf for interactive workspace selection
- **Smart Completion**: Tab completion for workspaces, GitHub repos (owner/repo format), and paths
- **GitHub Shorthand**: Use `owner/repo` instead of full URLs - automatically expands to `github.com/owner/repo`
- **Branch Support**: Specify branches with `owner/repo@branch` syntax
- **Fast Autocomplete**: Completion cache for ~3ms response time (vs ~700ms without cache)

## Shell Completion

After running `dl --install`, you get intelligent tab completion:

- Workspace names from your devpod list
- Known GitHub owners and repositories from your workspaces
- File/directory paths when starting with `./`, `/`, or `~`
- All command flags (`--ls`, `--stop`, etc.)

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
