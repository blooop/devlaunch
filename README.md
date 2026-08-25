# devlaunch

One command opens a devcontainer for any repo and branch, and drops you into a shell inside it.

```bash
dl blooop/devlaunch@fix/123
```

No clone step, no `devcontainer.json` to write, no build command. `dl` clones the repo if it has
to, builds or starts the devcontainer, and leaves you at a prompt in the checkout. Every
`repo@branch` gets a container of its own, so switching branches is switching containers.

It drives [devpod](https://devpod.sh), which does the container work. devlaunch is the front end:
one argument instead of a clone, a config file and a build command.

[![Ci](https://github.com/blooop/devlaunch/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/blooop/devlaunch/actions/workflows/ci.yml?query=branch%3Amain)
[![Codecov](https://codecov.io/gh/blooop/devlaunch/branch/main/graph/badge.svg?token=Y212GW1PG6)](https://codecov.io/gh/blooop/devlaunch)
[![GitHub issues](https://img.shields.io/github/issues/blooop/devlaunch.svg)](https://GitHub.com/blooop/devlaunch/issues/)
[![GitHub pull-requests merged](https://badgen.net/github/merged-prs/blooop/devlaunch)](https://github.com/blooop/devlaunch/pulls?q=is%3Amerged)
[![GitHub release](https://img.shields.io/github/release/blooop/devlaunch.svg)](https://GitHub.com/blooop/devlaunch/releases/)
[![PyPI](https://img.shields.io/pypi/v/devlaunch)](https://pypi.org/project/devlaunch/)
[![Conda](https://img.shields.io/badge/conda-v0.17.0-brightgreen?logo=anaconda)](https://prefix.dev/channels/blooop/packages/devlaunch)
[![License](https://img.shields.io/github/license/blooop/devlaunch)](https://opensource.org/license/mit/)
[![Platform](https://img.shields.io/badge/platform-linux--64-blue)](https://github.com/blooop/devlaunch/releases)
[![Pixi Badge](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/prefix-dev/pixi/main/assets/badge/v0.json)](https://pixi.sh)

## Contents

[Quickstart](#quickstart) · [Install](#install) · [Usage](#usage) · [Commands](#commands) ·
[aid](#aid-an-agent-instead-of-a-shell) · [What every workspace gets](#what-every-workspace-gets) ·
[Cleaning up](#cleaning-up) · [Environment variables](#environment-variables) ·
[Docs](#docs) · [Development](#development)

## Quickstart

```bash
pixi global install --channel conda-forge --channel https://prefix.dev/blooop devlaunch
```

### 1. Name a repo, land in a shell inside it

![Launching a workspace](docs/demo/1-launch.gif)

```bash
dl blooop/devlaunch
```

The first run of a repo builds an image and takes minutes. Every run after that attaches to the
container that is already there, in about a second. The line that created the workspace is the
line that gets you back to it tomorrow.

Exit the shell and the container keeps running. Nothing is lost between visits.

### 2. A branch is a container, not a checkout

![Two branches, two containers](docs/demo/2-branches.gif)

```bash
dl blooop/devlaunch@feature/json-flag
```

That is a second container for the same repo, on that branch, running beside the first. The
branch does not have to exist yet; `dl` creates it from the default branch if it cannot find it.

Two branches never share a working tree, an installed dependency set or a running service. A
half-finished experiment on one cannot break a review on another, and you never stash to switch.

Run `dl` with no arguments to pick from everything you have:

```bash
dl
```

The selector is built in. No `fzf` to install, no plugin. Type to filter, Enter to attach.

### 3. An agent instead of a shell

![Starting a coding agent](docs/demo/3-agent.gif)

```bash
aid blooop/devlaunch@fix/flaky-test
```

`aid` is `dl` with a coding agent started for you, in the same container `dl` would have given
you. Name the repo and it asks for the prompt while the container boots, so the two minutes are
the same minute. Or put the prompt on the line and skip the question:

```bash
aid blooop/devlaunch@feature/json-flag add a --json flag
```

### 4. Clear them out

![Deleting workspaces from the selector](docs/demo/4-cleanup.gif)

```bash
dl rm
```

Workspaces pile up. A verb with no workspace opens the same selector, TAB marks several rows, and
`dl rm` clears them in one pass.

## Install

### pixi (recommended)

```bash
pixi global install --channel conda-forge --channel https://prefix.dev/blooop devlaunch
```

That brings `devpod` and everything else along with it.

### pip

```bash
pip install devlaunch
```

The wheel is linux-64 and needs glibc 2.28 or newer (Ubuntu 20.04, RHEL 8). `dl` and `aid` are
compiled binaries, so once installed they need no Python at all. Install
[devpod](https://devpod.sh/docs/getting-started/install) yourself; pip does not.

The conda package has no glibc floor, so prefer it on an older distribution.

### Shell completions

```bash
dl --install
source ~/.bashrc
```

Completion covers workspace names, GitHub owners and repos you have used, paths, and the flags and
verbs below. It answers from a cache in about 3ms, against roughly 700ms without one. The cache
rebuilds in the background at most once an hour, and after any command that changes your
workspaces. `dl --refresh` rebuilds it now.

## Usage

```bash
dl                               # pick a workspace interactively
dl <user/repo>                   # open it, on its default branch
dl <user/repo>@<branch>          # open it, on that branch
dl ./my-project                  # open a local folder
dl <user/repo> <verb>            # apply a verb (stop, code, rm, ...)
dl <verb> <user/repo>            # the same, verb first
dl <verb>                        # the verb, on a workspace you pick
dl <user/repo> -- <command>      # run one command inside the workspace
```

`owner/repo` expands to `github.com/owner/repo`. A name that is not `owner/repo`, a URL or a path
is looked up among the workspaces you have. Owner and repo match case-insensitively, the way
GitHub treats them. Branch names are case-sensitive, because git refs are.

`dl <ws> -- <command>` gets a terminal whenever `dl` has one, so `htop`, `git rebase -i`, a REPL
or a coding agent all work. Redirect the output and the terminal goes away, so
`dl <ws> -- ls > files.txt` stays free of escape sequences.

### The selector

Each row is `owner | repo | branch`:

```
blooop          | devlaunch  | main
blooop          | devlaunch  | picker-columns
kinisi-robotics | kinisi_ros | ags-devcontainer-tooling-su
-               | myproject
```

The search bar is the top line and the matches read downward from it, so the first match is the
row nearest what you are typing. A workspace `dl` did not clone has no owner or repo to read, so
it keeps devpod's name for it and a dash where the owner would go. The branch column is the
branch as the workspace id spells it, which is slugged and shortened, so it is not always
retypeable. Pick the row rather than copying it; the row carries the real id underneath. Where
two rows would read alike, both fall back to their full ids so that picking one cannot delete the
other.

For the verbs that finish on their own (`up`, `stop`, `rm`, `code`, `dotfiles`) TAB marks any
number of rows and Enter applies the verb to each, and the line above the matches says so. The
forms that end in a session (`dl`, `-- <command>`, `restart`, `recreate`, `reset`) take exactly
one.

A reserved verb wins over a workspace of the same name: `dl stop` opens the selector to stop
something rather than looking for a workspace called `stop`.

The selector needs a terminal. Redirect `dl`'s input away from one and it declines to open a
selector rather than hanging or picking for you, so a script never blocks on a prompt nobody is
there to answer. [docs/cli.md](docs/cli.md) has the rest.

## Commands

### Workspace commands

| Command | What it does |
|---|---|
| `dl <ws>` | Open a shell in it |
| `dl <ws> up` | Start it without attaching, to prewarm a container |
| `dl <ws> stop` | Stop it. Frees memory, keeps disk |
| `dl <ws> rm` | Delete it. Refuses if the clone holds work that is nowhere else |
| `dl <ws> code` | Open it in VS Code |
| `dl <ws> restart` | Stop and start, no rebuild |
| `dl <ws> recreate` | Recreate the container |
| `dl <ws> reset` | Clean slate: remove everything, recreate |
| `dl <ws> dotfiles` | Refresh dotfiles (`chezmoi update`) |
| `dl <ws> -- <cmd>` | Run one command inside it |
| `dl <ws> --rm` | Open it, and delete it when the session ends |

Every verb also takes the workspace second (`dl stop <ws>`), and with no workspace at all it
opens the selector.

`rm` and `--rm` are docker's two commands rather than two spellings of one. `dl <ws> rm` deletes
now; `dl <ws> --rm` deletes once the session ends, the way `docker run --rm` does. Both stop at
work that exists nowhere else: a clone with uncommitted or unpushed changes is kept and named,
and `dl <ws> rm --force` is how you override that.

`--stop` and the `prune` verb are retired. Both are still recognised and print what to use
instead. [docs/cli.md](docs/cli.md) has the full `--rm` contract, including which exits fire it.

### Global commands

| Command | What it does |
|---|---|
| `dl --ls` | List every workspace |
| `dl --ls --json` | The same, machine-readable, with what each workspace would lose if deleted |
| `dl --ls --size` | Add what deleting each one would free. Opt-in: it walks every file |
| `dl --prune` | Remove the clone directories no workspace opens any more |
| `dl --reconcile` | Re-point workspaces whose recorded source folder went missing. Deletes nothing |
| `dl --purge` | Remove devlaunch's own workspaces and caches |
| `dl --install` | Install shell completions |
| `dl --refresh` | Rebuild the completion cache now |
| `dl --version` | Print the version |
| `dl --help`, `-h` | Print help |

`--prune`, `--reconcile` and `--purge` print their plan and ask first. `-y` skips the question,
and for `--prune` and `rm`, `--force` goes ahead despite work that is nowhere else.

```bash
$ dl --version
dl 0.17.0
```

`--devcontainer <variant|path>` picks a non-default `devcontainer.json`. A bare name means
`.devcontainer/<name>/devcontainer.json`. It is stored with the workspace, so pass it once.
Projects with several variants, compose sidecars, or a host-side `initializeCommand` are covered
in [docs/devcontainer-projects.md](docs/devcontainer-projects.md).

`dl --help` is the complete reference and is kept in step with the binary by a test.

## aid: an agent instead of a shell

```bash
aid <user/repo>[@branch] [prompt...]
```

`aid` is a shortcut, not a second launcher. It rewrites its command line into a `dl` one, so

```bash
aid blooop/devlaunch@fix/42 fix the flaky test
```

is exactly

```bash
dl blooop/devlaunch@fix/42 -- IS_SANDBOX=1 claude --dangerously-skip-permissions 'fix the flaky test'
```

Same clone, same workspace, same container. Everything after the workspace is the prompt, flags
included, so it never needs quoting.

With no prompt on the line, `aid` starts the container booting and asks for the prompt while it
does. Type it free of shell quoting, with no escaping and no history expansion eating a `!`. An
empty Enter starts the agent's plain session. Piping stdin or setting `DEVLAUNCH_NO_TTY=1` skips
the question and launches one-shot, so scripts behave as they always have.

| Option | What it does |
|---|---|
| `--claude`, `--codex`, `--gemini` | Pick the agent. Default `claude` |
| `--rm` | Delete the workspace when the agent is done. Appendable to a recalled line |
| `--devcontainer <variant\|path>` | Passed through to `dl` |

**The trade, stated plainly.** `claude` starts with `--dangerously-skip-permissions`, because the
agent is already inside a disposable container holding only this repo, and the per-tool prompts
would stall an unattended run. `IS_SANDBOX=1` rides along because `claude` otherwise refuses that
flag under `uid 0`, and devcontainers that run as root are ordinary. That variable is scoped to
the agent process and is not exported into your shell.

The agent cannot reach your host, but it can rewrite the checkout it is in. Review an `aid`
workspace before pushing rather than treating it as a sandbox that will stop the agent for you.

This applies to `aid` starting `claude` and nothing else. `--codex` and `--gemini` are unaffected,
and `dl <ws> -- claude` runs exactly what you typed.

The agent's CLI has to be in the container already. `aid` runs it; it does not install it.

## What every workspace gets

`dl` launches arbitrary repos, so none of this can depend on the image, and no repo has to add
anything to its `devcontainer.json`.

- **Your GitHub login.** `gh` is authenticated inside the container. The token comes from
  `GH_TOKEN`, `GITHUB_TOKEN` or `gh auth token`, whichever answers first, and is passed through a
  private file rather than a command line. Everything in the container can read it, including a
  `postCreateCommand` from a repo you did not write, so `DEVLAUNCH_NO_GH_TOKEN=1` skips it.
- **`gh` and `claude` on `PATH`.** If the image has them, nothing happens. If not, `dl` streams
  its own copies in over the ssh channel it already holds, with no download. A failed install
  costs the workspace its tools, never its launch.
- **[zellij](https://zellij.dev) on `PATH` when you ask for it**, so an agent can open a second
  terminal beside itself in the same container, and you can attach to it from anywhere.
  `DEVLAUNCH_ZELLIJ=1` is the ask, once in a shell profile or per launch; it costs 2.2s to 3.5s
  of a cold launch, which is why it waits to be asked.
- **A terminal named after the workspace.** `dl blooop/devlaunch` names the pane
  `devlaunch-main-3j1t` in zellij, tmux, or a plain terminal window: the workspace id,
  the same string `dl --ls` prints and the container's hostname carries.
- **A shared pixi package cache**, bound in from the host, so dotfiles that provision tools with
  `pixi global sync` download each package once per machine instead of once per container. On one
  measured profile that is 18s to 28s instead of 62s to 113s and 1.2 GB.

How each of those is delivered, and what it costs, is in
[docs/workspace-tools.md](docs/workspace-tools.md).

## Cleaning up

One workspace per branch means workspaces and clones accumulate. Three commands, and they do
different jobs:

| Command | Takes | Leaves |
|---|---|---|
| `dl --prune` | Clone directories no workspace opens | Every workspace, container, image and volume |
| `dl --purge` | The workspaces devlaunch created, and its caches | Workspaces it did not create, named before it asks |
| `dl --reconcile` | Nothing | Repairs records that stopped matching the disk |

Two promises worth knowing. **Nothing deletes work that exists nowhere else:** a clone with
uncommitted or unpushed changes, or one git cannot read to find out, is kept and named, and
`--force` is what overrides that. And **`dl` does not decide which workspaces are finished**,
because that is a fact about a ticket or somebody's intent. It reports what exists and what each
one holds, via `dl --ls --json`, and leaves the choosing to you or to a tool that knows.

Deleting a workspace takes its clone and the named Docker volumes its devcontainer created.
Images are yours: `docker system df` is what shows those.

[docs/cleanup.md](docs/cleanup.md) has all three in full, plus the disk accounting behind
`dl --ls --size` and the JSON a cleanup tool reads.

## Environment variables

| Variable | Effect |
|---|---|
| `DEVLAUNCH_NO_GH_TOKEN=1` | Do not forward the host's GitHub login into workspaces |
| `DEVLAUNCH_NO_TOOLS=1` | Do not install `gh` or `claude`. The setup pass still names the container |
| `DEVLAUNCH_ZELLIJ=1` | Install zellij, and create the session a command can open panes into. Off by default |
| `DEVLAUNCH_NO_TITLE=1` | Do not name the terminal after the workspace |
| `DEVLAUNCH_DOTFILES_ON_ATTACH=1` | Refresh dotfiles before every interactive attach. Off by default |
| `DEVLAUNCH_NO_TTY=1` | Never give a workspace command a terminal |
| `DEVLAUNCH_AID_AGENT=<agent>` | Change `aid`'s default agent |
| `DEVLAUNCH_TIMING=1\|json` | Write a timing summary to stderr. See [docs/performance.md](docs/performance.md) |
| `DEVPOD_SSH_CONFIG=<path>` | devpod's own, honoured rather than set: it is where `devpod up` publishes host aliases, so it is where `dl` looks for them. See [docs/cli.md](docs/cli.md) |

Every switch here reads the same values: anything but empty, `0`, `false` or `no` counts as
set. On a "no" variable that means turn it off; on an opt-in one it means turn it on. Three
rows are not switches and do not follow it: `DEVLAUNCH_AID_AGENT` and `DEVPOD_SSH_CONFIG`
take a value, and `DEVLAUNCH_TIMING` counts only empty and `0` as off, so `false` and `no`
turn it on.

Changes to what a container gets land on `devpod up`, so a workspace that is already running
keeps what it was given. `dl <ws> restart` is what re-decides it, and `dl <ws> recreate` is what
re-decides a mount.

## Docs

| Page | What is in it |
|---|---|
| [docs/cli.md](docs/cli.md) | The selector, which commands get a terminal, the `--rm` contract, retired spellings, devpod exit codes |
| [docs/workspaces.md](docs/workspaces.md) | How workspace ids are derived, and how fresh a launch is |
| [docs/workspace-tools.md](docs/workspace-tools.md) | GitHub auth, `gh` and `claude`, zellij, terminal titles, the pixi cache |
| [docs/cleanup.md](docs/cleanup.md) | `--prune`, `--purge`, `--reconcile`, and what a workspace costs on disk |
| [docs/performance.md](docs/performance.md) | Where a launch's seconds go, and the trend on `main` |
| [docs/devcontainer-projects.md](docs/devcontainer-projects.md) | Projects with demanding devcontainers |
| [docs/development.md](docs/development.md) | Building, testing, CI guards, the prebuilt image |

## Development

`dl` and `aid` are Rust. `rust/` is what ships: one cargo workspace, and where the tests of
`dl`'s own behaviour live.

```bash
cd rust
cargo build --release     # target/release/{dl,aid}
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Inside this repository's devcontainer the toolchain is already there, so `pixi run cargo test
--workspace` works the moment a container comes up. The Python that remains is an acceptance
harness that judges the shipped binaries from outside:

```bash
pixi run test              # unit and integration
pixi run test-e2e          # real devpod, real containers
pixi run ci                # lint, types, and the harness under coverage
```

[docs/development.md](docs/development.md) covers the CI guards, the public-API snapshots,
coverage, the prebuilt dev container image, and what a branch workspace costs on disk.
[AGENTS.md](AGENTS.md) is the working-tree build (`dl-next`) and how it differs from the released
`dl` on your PATH.

## License

MIT. See [LICENSE](LICENSE).
