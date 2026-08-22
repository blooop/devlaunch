# devlaunch

A streamlined CLI for [devpod](https://devpod.sh) with intuitive autocomplete and a built-in fuzzy selector.

`dl owner/repo@branch` is the whole interface: it clones the repo if it has to, builds or starts the
devcontainer, and drops you into a shell inside it. Every branch gets its own container, and
launching one again attaches to what is already there.

[![Ci](https://github.com/blooop/devlaunch/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/blooop/devlaunch/actions/workflows/ci.yml?query=branch%3Amain)
[![Codecov](https://codecov.io/gh/blooop/devlaunch/branch/main/graph/badge.svg?token=Y212GW1PG6)](https://codecov.io/gh/blooop/devlaunch)
[![GitHub issues](https://img.shields.io/github/issues/blooop/devlaunch.svg)](https://GitHub.com/blooop/devlaunch/issues/)
[![GitHub pull-requests merged](https://badgen.net/github/merged-prs/blooop/devlaunch)](https://github.com/blooop/devlaunch/pulls?q=is%3Amerged)
[![GitHub release](https://img.shields.io/github/release/blooop/devlaunch.svg)](https://GitHub.com/blooop/devlaunch/releases/)
[![PyPI](https://img.shields.io/pypi/v/devlaunch)](https://pypi.org/project/devlaunch/)
[![Conda](https://img.shields.io/badge/conda-v0.0.9-brightgreen?logo=anaconda)](https://prefix.dev/channels/blooop/packages/devlaunch)
[![License](https://img.shields.io/github/license/blooop/devlaunch)](https://opensource.org/license/mit/)
[![Platform](https://img.shields.io/badge/platform-linux--64-blue)](https://github.com/blooop/devlaunch/releases)
[![Pixi Badge](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/prefix-dev/pixi/main/assets/badge/v0.json)](https://pixi.sh)

**Start here:** [Features](#features) · [Installation](#installation) · [Usage](#usage) · [Workspace Commands](#workspace-commands) · [Global Commands](#global-commands) · [aid](#aid-start-a-coding-agent-in-a-workspace) · [A terminal beside the agent](#a-terminal-beside-the-agent) · [GitHub auth](#github-authentication) · [Tools in every workspace](#tools-in-every-workspace) · [Shell completion](#shell-completion)

**Reference:** [Options](#options) · [Workspace IDs](#workspace-ids) · [Cleaning up](#cleaning-up-purge-prune-reconcile) · [pixi cache](#the-shared-pixi-package-cache) · [Worktree backend](#worktree-backend) · [Launch timing](#measuring-launch-time) · [Development](#development)

## Features

- **Fuzzy Selection**: When called without arguments — or with a verb and no workspace — opens a built-in fuzzy selector; nothing to install, and no `fzf` on `PATH` to find
- **Smart Completion**: Tab completion for workspaces, GitHub repos (owner/repo format), and paths
- **GitHub Shorthand**: Use `owner/repo` instead of full URLs - automatically expands to `github.com/owner/repo`
- **Branch Support**: Specify branches with `owner/repo@branch` syntax
- **Fast Autocomplete**: Completion cache for ~3ms response time (vs ~700ms without cache)
- **One Round-Trip Per Question**: every `devpod` call costs ~0.45s, far more than `dl` itself, so a command reads the workspace list at most once — and everything a container needs on the way in (naming it, then the tools probe) rides one setup pass, so an interactive `dl <ws>` and a one-shot `dl <ws> -- <cmd>` cost the same trips

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

`dl` and `aid` are compiled binaries: the wheel is linux-64 and needs a glibc no older than 2.28
(Ubuntu 20.04, RHEL 8), and once installed it needs no Python at all — pip is only how it gets onto
`PATH`. The conda package above has no such floor, so prefer it on an older distribution.

Note: When using pip, you must install [devpod](https://devpod.sh/docs/getting-started/install) separately.
If `devpod` is not on `PATH`, every command that needs it prints a single install hint on stderr and exits `127`
(the shell's "command not found" code). `dl --help` and `dl --version` keep working without it.

A `devpod` that is installed but cannot answer is a different failure and gets a different exit code. If
`devpod list` exits non-zero, or prints something that is not a `--output json` workspace listing, `dl` quotes
what devpod said on stderr and exits `1` rather than reporting that you have no workspaces — so `dl --purge`
stops instead of deleting caches it never checked. Shell completion is the deliberate exception: `dl --install`,
`dl --refresh` and `dl --completion-data` log the failure and carry on with the repos and branches they can
still discover on local disk, so an unreachable devpod costs you workspace-name completion and nothing more.

### Shell Completions

After installation, set up shell completions for `dl` and `aid`:

```bash
dl --install
source ~/.bashrc  # or restart your terminal
```

## Usage

```bash
dl                               # Interactive workspace selector
dl <user/repo>                   # Start workspace and attach shell
dl <user/repo> <cmd>             # Run workspace command (stop, code, etc.)
dl <cmd> <user/repo>             # The same, verb first
dl <cmd>                         # The verb, on a workspace you pick interactively
dl <user/repo> -- <command>      # Run shell command in workspace
```

The selector is built in — no `fzf` on `PATH` and no `iterfzf`, which is why there is nothing to
install for it and why `dl` with its input redirected away from a terminal simply declines to open
one. It draws the search bar on the top line with the matches reading downward from it, so the
first match is the row nearest what you are typing. A reserved verb wins over a workspace name of the same spelling: `dl stop` opens the selector to
stop something, it does not look for a workspace called `stop`.

For the verbs that finish on their own — `up`, `stop`, `rm`, `code` and `dotfiles` — the selector
takes more than one row: TAB marks any number and Enter applies the verb to each in turn, so
`dl rm` can clear five dead workspaces in one visit. The selector says so on its own screen — the
line above the matches names what it will take, TAB included — rather than on the line printed
before it opened, which the picker covers up. The forms that end in an interactive session
(`dl`, `dl -- <command>`, `restart`, `recreate`, `reset`) take exactly one, since several of those
would just be sessions queued behind each other's exit.

### Examples

```bash
dl                               # Select a workspace interactively
dl myproject                     # Open an existing workspace by name
dl loft-sh/devpod                # Create from a GitHub repo
dl blooop/devlaunch@main         # Create from a specific branch
dl ./my-project                  # Create from a local path
dl blooop/devlaunch code         # Open in VS Code
dl blooop/devlaunch -- make test # Run a command in the workspace
dl blooop/devlaunch stop         # Stop the workspace
```

A name that is not `owner/repo`, a URL or a path is looked up among the workspaces you already have.

### Commands that need a terminal

`dl <ws> -- <command>` gives the command a terminal whenever `dl` itself has one,
so interactive programs — a coding agent, `htop`, `git rebase -i`, a REPL — start
and stay up instead of exiting immediately. Redirect the output and the terminal
goes away again, so `dl <ws> -- ls > files.txt` stays free of escape sequences.

This needs the ssh host alias `devpod up` writes to `~/.ssh/config`. If a
workspace has none, `dl` says so and falls back to the plain `devpod ssh`
transport, which has no terminal; `dl <ws> restart` republishes the alias. Set
`DEVLAUNCH_NO_TTY=1` to force the fallback everywhere.

## Workspace Commands

| Command | Description |
|---------|-------------|
| `dl <user/repo> up` | Start (or create) the workspace without attaching — for prewarming a container before a session wants it |
| `dl <user/repo> stop` | Stop the workspace |
| `dl <user/repo> rm` | Delete the workspace |
| `dl <user/repo> code` | Open in VS Code |
| `dl <user/repo> restart` | Stop and start (no rebuild) |
| `dl <user/repo> recreate` | Recreate container |
| `dl <user/repo> reset` | Clean slate (remove all, recreate) |
| `dl <user/repo> dotfiles` | Refresh dotfiles in the running workspace (`chezmoi update`) |
| `dl <user/repo> -- <command>` | Run shell command in workspace (with a terminal, when `dl` has one) |
| `dl <user/repo> --autorm` | Attach, and [delete the workspace when the session ends](#--autorm-the-throwaway-workspace) |

Every verb in that table also takes the workspace second — `dl stop <user/repo>` — and with no
workspace at all it opens the selector and applies itself to what you pick — everything you pick,
for the verbs the selector lets TAB mark several of. `stop` and `rm` answer to
`--stop` and `--rm` as well, since the flag spellings were documented long before they worked.

### `--stop` and `--rm` can be appended to a line that says something else

The flag spellings do one thing the words cannot: they may be typed at the *end* of a
line that already asked for something, and they win over it.

```bash
aid kinisi/repo@fix/x 'review this pr'      # work happens
aid kinisi/repo@fix/x 'review this pr' --rm --force   # ↑, with `--rm --force` appended
```

Both `dl` and `aid` accept it, and a leading verb word on the recalled line is not
mistaken for the workspace, so `dl prune <ws> --force` recalled with `--rm` appended
still removes `<ws>`. This exists because a shell makes appending to the previous line
cheap and rewriting the front of it expensive — deleting the workspace you were just
working in should not cost an edit in the middle of a long prompt.

What the suffix beat is named on stderr before anything is removed:

```
--rm overrode the rest of the line: 'review this pr' was not acted on.
```

That notice is the price of the convenience and is not optional: the line is now
allowed to carry an instruction it will not carry out, so a deliberate `--rm` and a
slip have to be told apart. Note that a `--` command tail cannot be overridden this
way — everything after `--` belongs to the workspace's command, so a `--rm` typed
there is an argument to that command and not a verb.

### `--autorm`: the throwaway workspace

`--autorm` deletes the workspace once the session ends, the way `docker run --rm` does.
It applies to the two forms that hand a session over and come back from it:

```bash
dl kinisi/repo@fix/x --autorm                # shell; the workspace goes when you exit
dl kinisi/repo@fix/x --autorm -- make test   # one command, then the workspace goes
aid kinisi/repo@fix/x 'fix the flaky test' --autorm
```

**It stops at work that is nowhere else.** The removal is `dl <ws> rm`'s, guard included,
so a clone holding uncommitted or unpushed work — or one git could not read to find out —
refuses, says which, and leaves the workspace standing:

```
--autorm: the session has ended, removing kinisi/repo@fix/x.
kinisi-repo-fix-x-1a2b holds 1 uncommitted change(s) (scratch.txt). Push or commit it,
or run: dl kinisi/repo@fix/x rm --force
```

That is what makes it safe to leave on a line you recall: the flag never decides that
your work was disposable. For the same reason `--force` does not compose with it — a
`--force` habitually appended to a recalled `--autorm` line would destroy work hours
later, unattended, with nobody reading the sentence explaining it. Run
`dl <ws> rm --force` when that is what you mean.

**A build that failed is collected too.** The removal runs whenever the launch got as
far as asking devpod for the workspace — including when `devpod up` died in
`postCreateCommand`, which leaves the container *running* and the clone cut. That is
the case an unattended `dl owner/repo --autorm -- make test` in CI most needs covered.
A launch that stopped earlier — an unknown workspace, a branch that could not be named,
a devpod that would not run — created nothing, so nothing is removed and nothing is
said about it.

Three more things it does not promise:

- **The exit code is the launch's.** `dl repo --autorm -- make test` exits with the
  test's status, and a failed build exits with devpod's; a removal that refused is
  never what the code reports. The refusal is on stderr and the workspace is still
  there.
- **It is best-effort, by construction** — but Ctrl-C out of a session is *not* one
  of the gaps. See "How you exit decides whether it fires" below.
- **It does not know about your other shells.** Nothing serialises two sessions on
  one workspace — the launch lock covers the build, not the session — so a second
  `dl <ws>` in another terminal is attached to the same container, and the `--autorm`
  run exiting first removes it from under that one. Use `--autorm` for the workspace
  you opened to throw away, not for one you may already be sitting in elsewhere.

On an `aid` line it is appendable like `--rm`, and unlike `--rm` it **keeps the prompt** —
the agent still runs, and the workspace goes when it is done.

### How you exit decides whether it fires

The removal runs when `dl` gets control back, so what matters is whether your exit ends
the session or kills `dl`.

**Ctrl-C out of the program you were running: fires.** Both session transports allocate
a pty — a bare `dl <ws>` runs `devpod ssh <id>`, and `dl <ws> -- <cmd>` on a terminal
runs `ssh -t` — which puts your local terminal in raw mode and clears `ISIG`. Ctrl-C is
then a byte travelling to the remote pty, not a signal to `dl`: the program *inside* the
container gets the interrupt. So `aid repo 'fix it' --autorm` and Ctrl-C twice to leave
Claude Code ends the remote command, ends the session, and the workspace goes. In an
interactive shell Ctrl-C just hands you a fresh prompt — `exit` or Ctrl-D is what ends
that session, and either fires the removal.

**These do not fire**, because `dl` itself takes the signal and its handler cannot run a
removal (a signal handler may not allocate or lock, and this one `_exit`s):

- Ctrl-C during the clone or the container build, before any pty exists.
- Closing the terminal window — SIGHUP, which `dl` does not handle at all.

One-line check for your own setup: start `dl <ws> --autorm` and press Ctrl-C once. A
fresh prompt *inside* the container means Ctrl-C is being forwarded and the removal will
fire when you leave. Landing back on the host means it reached `dl`, and it will not.

Those two forms and no others. Every verb word refuses the flag rather than ignoring it,
and `code` is the one worth knowing about: it returns while VS Code is still connecting,
so honouring `--autorm` there would delete the container out from under a window that is
still opening. `restart`, `recreate` and `reset` do end in a session and would work, but
they are out too, because `--autorm` is the throwaway workspace and not a cleanup modifier
on every verb that ends in a shell.

### `prune` is no longer a spelling of the `rm` verb

`dl <ws> prune` used to delete one workspace and `dl --prune` removes clone
directories and no workspace at all — one word, two unrelated commands, told apart
by two dashes. Reach for the wrong one and you either lose a workspace you meant to
keep or get refused for a reason the message could not explain
(`--prune takes no workspace: it is not a workspace command.`). So the verb spelling
is gone, and typing it says what to use instead:

```
$ dl <ws> prune
'prune' is no longer a workspace verb. Use 'dl <workspace> rm' to delete a workspace,
or 'dl --prune' to remove the clone directories no workspace opens any more.
```

`dl --prune` is unchanged. The word is still *recognised* rather than forgotten, so
it is never read as a workspace name: a `dl prune <ws> --force` line recalled with
`--rm` appended still removes `<ws>`, and a workspace that really is called `prune`
is still reachable as `dl stop prune`. Use `dl <ws> rm` from now on.

## Global Commands

| Command | Description |
|---------|-------------|
| `dl --ls` | List all workspaces |
| `dl --ls --json` | The same list as JSON, with each workspace's repo, branch, state and [unsaved work](#cleaning-up-workspaces) — for tools that decide what to clean up |
| `dl --ls --size` | Add [what deleting each workspace would free](#how-much-disk-a-workspace-costs). Opt-in: it walks every file in the clone |
| `dl --install` | Install shell completions |
| `dl --prune [-y] [--force]` | Remove [the clone directories no workspace opens any more](#pruning-the-clones-nothing-opens) — and nothing else |
| `dl --reconcile [-y]` | Re-point [devpod workspaces whose recorded source folder no longer holds a checkout](#reconciling-records-that-disagree) at the clone that does |
| `dl --purge [-y]` | Remove all devlaunch data — [the workspaces devlaunch created](#what-purge-deletes), and its caches |
| `dl --refresh` | Refresh completion cache |
| `dl --help, -h` | Show this help |
| `dl --version` | Show version |

```bash
$ dl --version
dl 0.1.0
```

The version and nothing else. `aid --version` reports the same version under its
own name — the two binaries are built from one cargo package and cannot disagree
about it. (Through 0.0.29 an editable install appended `(dev, editable from
<tree>)`, read out of the installed package's PEP 610 metadata. A compiled binary
has no such metadata and no editable installs, so there is nothing left to say.)

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
dl blooop/devlaunch@fix/42 -- IS_SANDBOX=1 claude --dangerously-skip-permissions 'fix the flaky test'
```

That means an `aid` workspace *is* the `dl` workspace: same clone, same workspace
id, same container — started if stopped, attached to if already running, and never
rebuilt just because `aid` asked for it. Anything `dl` learns, `aid` gets.

With no prompt on the command line, `aid` on a terminal does not just drop you
into the agent: it starts the workspace booting in the background and asks for
the prompt while it does. Type it free of shell quoting — no escaping, no
history expansion eating a `!` — and press Enter to launch; the minute the
container takes to come up and the minute the prompt takes to write are the same
minute. An empty Enter (or Ctrl-D) starts the agent's plain session, exactly
what a bare `aid <workspace>` always did. Piping stdin or setting
`DEVLAUNCH_NO_TTY=1` skips the question entirely, so scripts see the old
one-shot behaviour without changing a line.

`claude` is started with `--dangerously-skip-permissions`. The agent is already
inside a disposable container holding only this repo, so the per-tool prompts it
would ask on the host protect nothing here and would stall an unattended run.
`IS_SANDBOX=1` rides along because `claude` otherwise refuses that flag outright
under `uid 0`, and devcontainers that run as root are ordinary. The variable is
scoped to the agent process, not exported into your shell.

The trade is worth stating plainly: an agent started this way edits, runs and
deletes inside the container without asking. It cannot reach your host, but it can
rewrite the checkout it is in, so review an `aid` workspace before pushing rather
than treating it as a sandbox that will stop it for you. `--codex` and `--gemini`
are unaffected, and `dl <ws> -- claude` still runs exactly what you typed.

| Option | Description |
|--------|-------------|
| `--claude`, `--codex`, `--gemini` | Pick the agent (default: `claude`) |
| `--devcontainer <variant\|path>` | Passed through to `dl` |
| `--rm`, `--stop` | Do that to the workspace instead of starting an agent; appendable to a recalled line, `--force` with `--rm` deletes despite unsaved work |
| `--autorm` | Run the agent, then delete the workspace when the session ends |
| `DEVLAUNCH_AID_AGENT=<agent>` | Change the default agent |
| `DEVLAUNCH_NO_TTY=1` | No prompt question, no pty: the old one-shot behaviour |

Everything after the workspace is the prompt, flags and all, so it never needs
quoting to survive `aid`'s own parsing. Managing workspaces — listing, stopping,
deleting, VS Code — stays with `dl`.

The agent's CLI has to be installed in the container; `aid` runs it there, it does
not install it.

## A terminal beside the agent

Every workspace `dl` opens also has [zellij](https://zellij.dev) on `PATH`, which
buys one thing the other tools do not: an agent running in a container can open a
**second terminal next to itself**, in the same container, and you can attach to it
from anywhere to watch or to type.

Nothing has to cooperate for this. It does not come from your dotfiles, it does not
need an edit to any repo's `devcontainer.json`, and it works in images `dl` has never
seen — the same argument the rest of "Tools in every workspace" makes, for the same
reason: `dl` launches arbitrary repos.

### Opening a pane from inside a session

From anywhere inside the container — including from a completely non-interactive
command, with no terminal attached to anything:

```bash
zellij -s devlaunch action new-pane -- htop
```

`-s <name>` is the form to use and the only one worth depending on. Bare
`zellij action new-pane` happens to work by falling back to the single running
session, which stops being a single session the moment there are two of them.

`devlaunch` is the session name `dl` creates and the one to name here.

### Switching the wrap on

The session an agent opens panes into has to exist first, and creating it is
**off by default**:

```bash
DEVLAUNCH_ZELLIJ=1 dl someone/repo -- claude -p "do the thing"
```

| Variable | Description |
|----------|-------------|
| `DEVLAUNCH_ZELLIJ=1` | Before running `dl <spec> -- <command>`, make sure a zellij session named `devlaunch` exists in the container, so the command can open panes into it |

With it off, no invocation changes meaning at all — that is what off means here, and
it is why the switch exists rather than the behaviour simply being on.

**This is not the switch that decides whether zellij gets installed.** That one is
`DEVLAUNCH_NO_ZELLIJ`, under [What it costs](#what-it-costs) below, and the two are
orthogonal on purpose: this one starts a session, that one puts the binary there.
With `DEVLAUNCH_NO_ZELLIJ=1` set and no zellij in the container, `DEVLAUNCH_ZELLIJ=1`
is simply a session setup that fails and a command that runs anyway — which is what
it already does in any container that ended up without zellij for its own reasons.

**The command runs beside the session, not inside a pane of it.** That is deliberate.
Putting the command in a pane would hand its stdin, stdout and exit status to zellij,
and all three are things `dl` promises to leave alone: `dl <ws> -- cmd > file` has to
put the command's own output in the file, and a failing command has to come back with
its own status. Since `zellij -s <name> action new-pane` works perfectly well from a
command that is in no session at all, running beside the session costs nothing and
delivers the same pane.

**The interactive session of a bare `dl <workspace>` is untouched, switched on or
off.** An interactive attach sends no command for the wrap to attach to — that is
exactly what gets it a terminal from devpod — and giving it one would cost either the
terminal or a round trip in front of every shell. You land in an ordinary login shell
with `zellij` on `PATH`, so `zellij attach -c devlaunch` gets you the session, and any
panes an agent has opened in it, whenever you want them.

There is one exception, and it is a pleasant one: if you also run with
`DEVLAUNCH_DOTFILES_ON_ATTACH=1`, that refresh is a command, so it gets wrapped like
any other and the session is already there when the shell arrives.

### Existing workspaces

zellij arrives on the setup pass, which runs on every `devpod up`. So a workspace
that predates this picks it up on its next **`dl <workspace> restart`** — a full
`dl <workspace> recreate` also works but is not needed, because nothing here is a
bind mount and mounts are the thing that only lands at container creation.

Attaching to a workspace that is *already running* skips `devpod up` and so skips
this too, which is what makes the restart necessary rather than automatic.

### What it costs

Almost nothing, and that was measured rather than assumed. zellij is a conda-forge
package installed by pixi into the container, so it lands in the shared package cache
above and every container after the first extracts rather than downloads:

| | |
|---|---|
| **Warm install** (shared cache populated) | 0.56s / 0.23s / 0.23s over three fresh containers |
| **Cold install** (empty cache) | 3.0s, filling 167MB of shared cache |
| **Every launch after the first** | one `command -v`; the whole setup pass measured at 50ms |

**It can never fail a launch.** Provisioning zellij is a stage of the setup pass, so a
container with no network, no pixi and no way to get either reports the stage as
failed, by name, and then opens exactly as it would have. A container that ends up
without zellij still works; with the wrap on, the command still runs, because the
session setup is allowed to fail and the command runs regardless.

`DEVLAUNCH_NO_TOOLS=1` turns this off along with the rest of tool provisioning —
installing zellij is tool provisioning, where naming a container is not.

`DEVLAUNCH_NO_ZELLIJ=1` turns off **only** this:

| Variable | Description |
|----------|-------------|
| `DEVLAUNCH_NO_ZELLIJ=1` | Do not install `zellij` into workspaces. The setup pass still runs and still names the container, and `gh` and `claude` are still provisioned exactly as they were |

It exists because the two questions are different ones. A host whose containers get
zellij another way — their own dotfiles, a base image, a devcontainer feature — or
which wants none in there at all, is asking for one stage to stop running.
`DEVLAUNCH_NO_TOOLS=1` would do that and also surrender the `gh` and `claude`
guarantee that the rest of this README is about, which is a large price for one
`command -v`.

Both variables read the same values: anything but empty, `0`, `false` or `no` means
yes, turn it off. And neither touches the hostname stage — a host that wants no
zellij has not thereby asked for unnamed containers.

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
never on a command line, so it does not appear in `ps`. `dl` installs `gh` itself
(see [Tools in every workspace](#tools-in-every-workspace)), so the login has
something to be spent on whatever the image ships. Check a workspace with:

```bash
dl <workspace> -- gh auth status
```

If no token can be found, `dl` warns on stderr and opens the workspace anyway rather
than failing — and the warning names the config directory `gh` consulted, because the
usual cause is a shell that scoped `XDG_CONFIG_HOME` somewhere `gh` has no login,
not a host that is actually logged out.

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

## Tools in every workspace

`gh` and `claude` are available in every workspace `dl` opens, in every kind of
session — an interactive `dl <workspace>`, a one-shot `dl <workspace> -- <command>`,
and `aid`. The repo's `devcontainer.json` does not have to provide them, and most
do not: `dl` launches arbitrary repos, so a guarantee that depended on the image
would not be a guarantee.

### How they get there

On `devpod up`, at most three round trips, each one earning the next.

**1. The setup pass — the only trip a ready workspace ever pays.** One trip
carries everything the host wants done on the way into a running container: the
stages first, then the probe. Naming the container — the hostname your shell
prompt shows — is the one stage today, and it costs nothing extra because the
probe was paying for the trip anyway. Each stage reports `ok`, `failed` with its
exit status, or *not reached*; one that fails stops neither the stages behind it
nor the probe, and `dl` says which one it was.

The probe is the tail of that trip. The container reports
what only it can know: whether both tools answer at all, where its `claude`
resolves to, and where `~/.local/share/claude/versions` in its own home resolves
to. It reports those and names no verdict; the host reads them, so "a real
`claude`" is defined in exactly one place. The reading is one of three:

- **provisioned** — `gh` answers on the login PATH and `claude` resolves to a
  binary the official installer put in the versions directory. Nothing else
  happens.
- **lendable** — both names answer, but that `claude` is a shim or a wrapper.
- **absent** — a tool is genuinely missing.

**2. A lend, for *lendable* and *absent*.** `dl` streams its own `gh` and `claude`
into the container as a tar over the `devpod ssh` channel it already holds — a
local pipe, no network and no download. Nothing lands outside a staging directory
until both binaries have been run there once, so a container that cannot execute
them (a different libc, a different architecture) is left exactly as it was.

**3. The network install, for *absent* only.** When the host had nothing to lend,
or the lend was refused, `pixi global` installs both tools — and `pixi` itself
first if the image has none. A *lendable* container never reaches this trip: it
stops after the lend, or — when the host had nothing to lend — after the probe
itself. A `claude` already answers there, and this install decides what to do
with the same `command -v` that a shim satisfies, so the trip would install
nothing.

Tools reach the PATH of a login shell through whichever of `~/.bash_profile`,
`~/.bash_login` or `~/.profile` bash actually reads — it sources only the first of
those that exists, so an image shipping a `~/.bash_profile` never reads
`~/.profile`.

An install that fails costs the workspace its tools, not its launch: `dl` logs a
warning and hands you the session anyway.

### The trip a launch can skip

Trip 1 is cheap but it is not free: about 1.7 seconds, almost all of it connection
and process setup rather than the script it carries. A workspace that has had both
tools in it for a week pays that on every `dl <workspace> up` to be told the same
thing it was told last time — so when the answer was *provisioned*, `dl` writes it
down and reuses it.

The marker is one small JSON file per workspace under
`${XDG_CACHE_HOME:-~/.cache}/devlaunch/tool-verdicts/`, holding the verdict and the
modification time of devpod's own `workspace_result.json` for that workspace. devpod
rewrites that file on the way out of every **completed** `up`, whoever ran it —
`dl`, VS Code, a hand-typed `devpod up`, a `--recreate` — so a container that has
been rebuilt has a result file whose mtime no longer matches, and the marker stops
being believed. Anything else unexpected (no marker, an unreadable one, a workspace
`dl` cannot find one result file for) also stops it being believed. Every one of
those falls back to making the trip, which is exactly what happened before the
marker existed.

**Only a launch that finds the workspace already running can skip it**, and that is
a smaller claim than it sounds. The container's hostname lives in a namespace docker
rebuilds from the container's config on every start, so a `devpod up` — creating a
container or starting a stopped one — loses the name and the pass has to run again
to set it. The two paths that skip are `dl <workspace> up` against a container that
is already up (the pre-warm, and where the 1.7s is paid most often), and a launch
that waited on a sibling which had already brought the workspace up. Nothing is ever
skipped after this launch's own `devpod up`.

Nothing has to be cleaned up, and there is nothing to invalidate by hand: the
markers are compared, never trusted on age, and deleting the whole directory costs
one round trip on each workspace's next launch.

### What to bake so a launch does no work at all

To make every `dl` launch of an image stop at trip 1. The probe asks a **login**
shell to resolve each name, so every bullet here is about what a login shell can
find:

- **`gh`** anywhere on the login PATH.
- **`claude`** in the layout its official installer creates — the binary at
  `~/.local/share/claude/versions/<version>`, a **direct child** of that
  directory named for the version, with `~/.local/bin/claude` symlinked to it.
  Nested any deeper — `versions/<version>/bin/claude`, the shape a downloader
  parked there would take — is read as somebody else's tree that merely starts
  with the official path, and does not count.
- **`~/.local/bin` on the login PATH**. The symlink above is how `claude`
  answers at all; a login shell that cannot find that directory reads the image
  as *absent* however carefully the rest was baked, and it pays the full lend.
  Ubuntu's stock `~/.profile` prepends `~/.local/bin` itself — but an image
  shipping a `~/.bash_profile` never reads `~/.profile` (above), and then
  nothing does.

Nothing else counts as a `claude`, and that is the point. A *shim* — a small
launcher that downloads the real binary the first time it is called — answers
`command -v claude` exactly as the real thing does, while the workspace still
owes a multi-hundred-megabyte download at the least convenient moment. So `dl`
resolves the name rather than running it (running a shim *is* the download), reads
a shim as *lendable*, and sends the host's real binary. The lend prepends
`~/.local/bin` to the login PATH, which is what puts the lent binary in front of
the shim from then on — intended, and the reason the next launch probes
*provisioned* and the transfer is paid once rather than forever.

**This repo's own devcontainer feature bakes a shim.**
`.devcontainer/claude-code/install.sh` installs `claude-shim`, so an image built
from it does *not* meet the contract by itself: its first `dl` launch is lent a
real `claude`, and only launches after that do nothing. Build the official layout
into the image if you want the first launch free too.

### What this deliberately does not do

- **No per-tool transfer.** The lend is all-or-nothing — an image with a real `gh`
  but a shimmed `claude` is sent both. Splitting the payload would save part of
  one transfer, paid once per workspace, in exchange for a matrix of half-lent
  states every later step would have to reason about. (The *network* install is
  already per tool: each install guards itself with its own `command -v`.)
- **No version sync.** A real `claude` already in the container is left alone
  whatever its version. `dl` lends what is missing; it is not a package manager,
  and keeping versions in step would mean deciding what to do when the container
  is the newer one. The official binary self-updates in a long-lived workspace,
  and rebuilding one re-provisions it from scratch. The single upgrade `dl` does
  perform is replacing a shim with a real binary.

### Turning it off

```bash
DEVLAUNCH_NO_TOOLS=1 dl someone/repo
```

| Variable | Description |
|----------|-------------|
| `DEVLAUNCH_NO_TOOLS=1` | Do not install `gh` or `claude` into workspaces. The setup pass still runs — one trip per `up`, which still names the container; only the installing is skipped |

`DEVLAUNCH_NO_ZELLIJ=1` is the narrower one: it drops the zellij stage and leaves
`gh` and `claude` provisioning alone. See
[What it costs](#what-it-costs).

Attaching to a workspace that is *already running* skips `devpod up`, and so skips
this too. A workspace started by something other than `dl` — or created before this
existed — picks the tools up on its next `dl <workspace> restart`.

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
backend's background fetch sweep uses), and at most once per `dl` invocation. Commands
that change your workspaces (starting, stopping or deleting one) rebuild it as
soon as they finish, regardless of when it was last built. Commands with no use
for it — `dl --help`, `dl --version` — do not touch it at all.

A branch created on a remote in the last hour may therefore not be offered yet.
`dl --refresh` rebuilds the cache immediately and ignores the interval.

---

*Everything above is what it takes to use `dl`. What follows is reference — flags, ids, disk
accounting, and the internals worth knowing when something surprises you.*

## Options

| Option | Description |
|--------|-------------|
| `--devcontainer <variant\|path>` | Use a non-default `devcontainer.json`. A bare name means `.devcontainer/<name>/devcontainer.json`. Stored with the workspace, so pass it once. |
| `DEVLAUNCH_NO_TTY=1` | Never give a workspace command a terminal; always use the plain `devpod ssh` transport. |
| `DEVLAUNCH_DOTFILES_ON_ATTACH=1` | Refresh dotfiles before handing over an interactive shell. Off by default; see below. |

Projects with demanding devcontainers — several variants, compose sidecars, or a
host-side `initializeCommand` that has to tell branch workspaces apart — are
covered in [docs/devcontainer-projects.md](docs/devcontainer-projects.md).

### Refreshing dotfiles on attach

devpod applies dotfiles when it *provisions* a workspace, so a workspace that has
been up for a fortnight still has the dotfiles it was born with. `dl <ws>
dotfiles` fixes that when you think of it; `DEVLAUNCH_DOTFILES_ON_ATTACH=1` makes
`dl` think of it for you, running the same refresh just before it hands you the
shell.

```bash
DEVLAUNCH_DOTFILES_ON_ATTACH=1 dl someone/repo
```

It is off unless you set it, and that is the point rather than caution. The
refresh is a `devpod ssh` round-trip — measured at ~1.7s, almost all of it
connection setup — with a `git pull` behind it, and it would otherwise be charged
to every attach on every machine to close a gap most people do not have.

Two things it deliberately does not do:

- **It never runs for `dl <ws> -- <command>`.** A one-shot command renders no
  prompt and sources no interactive shell, so refreshing in front of it would
  buy that command nothing and cost it the round-trip. That path is the one
  agent launchers use, and it stays exactly as fast as it was.
- **It never holds the shell hostage.** The refresh gets 60 seconds; an
  unreachable dotfiles remote, or one that wants a password nobody is there to
  type, means a pause and then your shell, not a hang. Failure is a warning —
  you get the workspace either way.

Refreshes run every time you attach, with no cooldown, because you asked for
them. If that is too often, unset the variable and use `dl <ws> dotfiles`.

## Workspace IDs

`dl user/repo@branch` derives one id that names both the devpod workspace (what you
see in `dl --ls`) and the clone directory under `~/.cache/devlaunch/repos/`:

```
<repo-slug>-<branch-slug>-<syllables>      at most 47 characters

blooop/devlaunch@main                             -> devlaunch-main-zovomobo
blooop/devlaunch@feature/auth                     -> devlaunch-feature-auth-poliseno
blooop/devlaunch@feature-auth                     -> devlaunch-feature-auth-nesatabe
blooop/test_renv@nb4                              -> test-renv-nb4-polenita
kinisi-robotics/kinisi_ros@ags-devcontainer-tooling-support
                                                  -> kinisi-ros-ags-devcontainer-tooling-su-lenevere
blooop/devlaunch@dependabot/github_actions/codecov/codecov-action-6
                                                  -> devlaunch-dependabot-codecov-action-6-sifivasa
```

The eight-character syllable suffix is a hash of the full `(owner, repo, branch)` triple.
It is what makes the id unique: the readable part is shortened to fit the length limit,
and shortening it does not affect whether two branches share an id. Long branch names
drop whole `/`-separated middle segments before losing characters, so the part that
identifies the branch survives. Note the third and fourth lines above: `feature/auth` and
`feature-auth` read the same once slugged but are different branches, and they get
different ids.

Owner and repo are matched case-insensitively, the way GitHub treats them, so
`dl NVIDIA/cuda-samples@main` and `dl nvidia/cuda-samples@main` are the same workspace.
Branch names are case-sensitive, because git refs are.

URL specs (`dl github.com/owner/repo`) get an id in the same shape, with the suffix
hashed over the URL.

The id is also the container hostname. 47 characters is one inside devpod's own hard
ceiling of 48 — a 49-character id is refused outright, not truncated — and well inside
the 64-byte hostname limit on its own, but tools that stack their own prefixes onto the
container name have about 17 characters to work with, so a tool that wants more is the
one that has to shorten.

Branch names must be safe as both git refs and directory names — a name with a space or
a leading dash is rejected rather than quietly rewritten.

### Upgrading from an older devlaunch

This id format is new, and the directories and containers on your machine were named by
the previous scheme. The first `dl user/repo…` command after upgrading migrates the cache
once, and leaves what it did behind in the cache directory (the two listings named below).
`dl --help`, `dl --version`, `dl --ls` and opening an existing workspace by name do not trigger it.

**Your clone directories are renamed.** What was
`~/.cache/devlaunch/repos/blooop/devlaunch/main` becomes
`~/.cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-zovomobo`. A workspace is a git
clone whose `origin` points at the `.bare` cache next to it, and `.bare` does not move, so
this is a plain rename: branches, history and **uncommitted changes all survive** — only
the folder name changes. `metadata.json` is updated in the same pass, so nothing is left
pointing at the old name.

**Your existing devpod containers keep their old ids and are orphaned — and they can
often be repaired rather than replaced.** An orphaned container is sourced at the path this
migration just renamed, with the real clone sitting next to it under the new name, which is
precisely what [`dl --reconcile`](#reconciling-records-that-disagree) is for: it re-points
devpod's record at the renamed clone, and `dl <workspace> recreate` finishes the repair.
That gives you back the clone association and the workspace's identity — not state that
lived only inside the old container, which nothing can bring back. The repair is
order-dependent: relaunching the branch claims the renamed clone for a fresh container,
and reconcile never re-points a clone a live container holds — so reconcile first, then
relaunch. Left alone, the next `dl user/repo@branch` simply builds a fresh container
under the new id, and deleting the old one is all that remains for it.

dl does not delete containers for you — deleting by id is how a running sidecar got
destroyed the last time something tried ([kinisi_ros#9766](https://github.com/kinisi-robotics/kinisi_ros/pull/9766)) —
so it writes the old ids to
`~/.cache/devlaunch/orphaned-workspaces.txt`. For the workspaces you are finished with,
the disposal command reads from that listing:

```bash
xargs -r -n1 devpod delete < ~/.cache/devlaunch/orphaned-workspaces.txt
```

**A clone directory with no metadata record is left alone.** Nothing records which branch
it was cloned for, and the old directory name cannot be turned back into one — `feature/auth`
and `feature-auth` both became `feature-auth` — so a guessed name would be worse than no
rename. Those directories stay exactly where they are and are listed in
`~/.cache/devlaunch/unmigrated-clones.txt`.

Running dl again changes nothing: the migration is keyed on the `version` field in
`metadata.json`, not on directory names, so a branch that happens to look like a new-scheme
id is never mistaken for one. If a migration is interrupted, the next run finishes it — the
version is written last, in the same atomic save as the new paths, so it never claims more
than the filesystem has actually done. A rename the filesystem refuses (a read-only mount,
tightened permissions) is treated the same way: the version stays put and every later run
retries the refused directories and repeats the notice until the underlying refusal is
fixed by hand.

## Cleaning up: purge, prune, reconcile

The three global commands that touch what is already on disk, in full. `--prune` takes clone
directories and no workspaces, `--purge` takes both but only what devlaunch made, and `--reconcile`
removes nothing — it repairs records that stopped matching the disk. Each one prints its plan and
asks before acting, and `-y` is what skips the question.

One exception, and it is not a released build: a binary compiled from somebody's
working tree appends `-dev` (`dl <version>-dev`). That comes from the `dev-build`
cargo feature, which `./dev.sh` builds with and nothing that ships enables, and it
is what tells `dl-next` apart from `dl` when both are on PATH — see "Two installs"
in AGENTS.md.

### What a delete takes with it

Removing a workspace removes three things: the devpod workspace, the local clone
(unless it holds work that exists nowhere else — see [cleaning up
workspaces](#cleaning-up-workspaces)), and **the named Docker volumes that
workspace's devcontainer created**. Every path that removes a workspace does all three:
`dl <ws> rm`, the appended `--rm`, `--autorm`, and `--purge`.

Two volumes per workspace, both named from what devpod recorded substituting into
the devcontainer:

| volume | declared by |
| --- | --- |
| `<workspace-folder-basename>-pixi` | a `mounts` entry in the devcontainer, for the `.pixi` cache — this repo's own devcontainer has one |
| `dind-var-lib-docker-<devcontainerId>` | the [`docker-in-docker`](https://github.com/devcontainers/features/tree/main/src/docker-in-docker) feature, for the nested daemon's `/var/lib/docker` |

They used to be left behind, and that was measured rather than assumed: on one
development machine, **39 orphaned volumes holding 37.28 GB**, not one of them
with a surviving workspace in `devpod list` (devlaunch#324). `devpod delete`
removes the container and never a volume, and Docker never garbage-collects a
*named* volume, so nothing in the picture was ever going to reclaim them.

Three things about how the removal behaves, all of them so that a delete cannot
be made worse by it:

- **The names are read from devpod's own record, never guessed from a pattern.**
  devpod writes down what it substituted, and that is the only thing consulted. A
  workspace devpod never finished creating has no such record, so nothing is named
  and no `docker` command runs at all — rather than one carrying a made-up name,
  which would be somebody else's disk.
- **It is best-effort, and cannot fail a delete.** The workspace is gone either
  way; reporting failure would send you looking for a workspace that is not there.
  A volume Docker will not release (one another container still holds, say) is a
  line on stderr and nothing more. A machine with no `docker` at all says nothing,
  because a machine with no Docker never made these volumes.
- **Images are still yours.** See [the disk neither command
  frees](#the-disk-neither-command-frees) — that boundary is about images now, and
  deliberately stays there.

### What purge deletes

devpod's workspace list is shared. A workspace you made with `devpod up`, or that
another tool made, sits in the same list as the ones `dl` made, and `dl --purge`
has no business destroying it. So it deletes only the workspaces devlaunch
created — the clones it made under its own cache directory (`$XDG_CACHE_HOME` or
`~/.cache`, then `devlaunch/repos/<owner>/<repo>/<id>`), which is exactly the
directory the purge is about to remove anyway. Everything else keeps working
afterwards, because nothing a purge touches backs it.

Anything it is leaving is named before it asks:

```
$ dl --purge
This will remove all devlaunch data:
  - 4 DevPod workspace(s)
  - /home/you/.cache/devlaunch/ (workspace clones, repo caches, the shared pixi cache, completions)

Leaving 2 workspace(s) devlaunch did not create:
  - pythontemplate
  - my-hand-made-workspace

Are you sure? [y/N]
```

Three things `dl` does create are in that second list rather than the first.
`dl ./some/path` and `dl <git-url>` open a source `dl` did not clone, so it
cannot tell them from a workspace you made by hand — and a `config.toml` that
points `repos_dir` outside the cache puts the clones somewhere `--purge` does not
remove either, so those are left too. Delete any of them with `dl <workspace> rm`.
Erring this way is deliberate — a purge that skips one of your own workspaces
costs you a command, and the other kind of mistake costs you work you cannot get
back.

#### When part of the cache will not go

A container writes into its clone as its own user — `vscode`, uid 1000, in the
standard devcontainer base image. Where your host user is uid 1000 too, nothing
here comes up. Where it is not — CI, a shared machine, a container running as
root, or devlaunch developed inside its own devcontainer — the directories the
container made cannot be emptied by you, and the purge cannot remove them.

It removes everything else anyway, and names what is left:

```
$ dl --purge -y
Removed what was permitted under /home/you/.cache/devlaunch. These refused:
  - /home/you/.cache/devlaunch/repos/blooop/bencher/bencher-main-kivagede: Permission denied

Usually this means a container wrote them as a different user, and:
  sudo rm -rf '/home/you/.cache/devlaunch'
clears them. Check the reasons above first -- it does not fix all of them.
devlaunch does not manage Docker images: the images these workspaces built may still hold disk, and `docker system df` shows what Docker is holding.
```

That last line ends every purge, including one that found nothing to purge and
one you answered `n` to — `dl --prune` ends on the same one, in the same words.
See [the disk neither command frees](#the-disk-neither-command-frees).

Exit status is `1`, because a clone you were told would go is still on disk. It
used to be `1` with the *whole* cache still standing: the first refusal stopped
the purge, so the completion caches, `metadata.json` and every other clone
survived on account of one directory.

When **none** of it goes — nothing under the cache came away at all, which is
what a symlinked cache root gives you, or one that cannot even be looked at, or
one whose every entry refused — the headline says that instead of claiming a
partial success:

```
$ dl --purge -y
Removed nothing under /home/you/.cache/devlaunch. These refused:
  - /home/you/.cache/devlaunch: Permission denied
```

The report underneath is the same one, and so is the exit status: `0` means the
cache is gone and nothing else does, which is the only distinction a script can
act on. Removed everything, removed what it was permitted to and removed nothing
are three outcomes rather than two, and the sentence is where the third one
lives — because it is the one that decides whether you still have clones to go
and look for.

What is listed is the directory, once — not the hundreds of files inside it.
Unlinking needs write permission on the directory rather than on the file, so
every entry in that clone refuses separately and they are all the same fact.
Two *separately* unwritable directories on one path are two lines, though,
because clearing the inner one would leave the outer one just as stuck.

Each line carries what the system actually said. A container running as another
user is the common cause, but a read-only mount, `chattr +i` and a busy
mountpoint all land here too — and `sudo rm -rf` does not fix those, which is
why the report offers the cause rather than asserting it.

If you have **moved your cache** by making `~/.cache/devlaunch` a symlink, a
purge refuses it and names the target rather than following it. Remove the real
directory yourself if you meant to: following the link would empty a directory
you never named, and removing just the link would report a clean sweep while
your clones sat on the other volume.

### Pruning the clones nothing opens

A workspace per branch means clone directories accumulate under the cache, and
until now nothing removed them: measured on one host, **52 clone directories for
17 live devpod workspaces — 37 of them attached to nothing, 4.00 GB, against
7.86 GB still in use.** `--purge` is the wrong tool for that, being
all-or-nothing: the only way to get the 4 GB back was to destroy the 7.86 GB
too, and every bare cache with it.

`dl --prune` removes exactly the clone directories no live workspace opens. It
never deletes a devpod workspace, a container, an image or a volume, never
touches a repo's `.bare` cache (0.08 GB for seven repos, and it is what makes
the next clone of a repo fast), and never looks outside
`<cache>/devlaunch/repos`. Every directory it finds is one of three things:

- **a live workspace opens it** — kept, and named with the workspace that has
  it. "Opens" means at *or under*: a workspace opened on a subdirectory of a
  clone still needs the clone;
- **nothing opens it** — removed, unless it holds work that exists nowhere else,
  or `git` would not say what it holds. A clone a container wrote as another
  user is unreadable rather than empty, and "cannot tell" is kept, not removed;
- **`dl`'s records and devpod's disagree about it** — kept, always. This is
  [#88](https://github.com/blooop/devlaunch/issues/88)'s shape. On that ticket's
  host, 36 devpod workspaces out of 39 recorded a source folder that was gone or
  was a config-only stub, while the real checkout sat beside it under a newer
  naming scheme — so a perfectly healthy clone was opened by nobody, and the
  stub was the only thing anything pointed at. `--prune` will not guess which
  clone such a workspace needs: it keeps every clone of that repository and
  names the record to go and fix. `--force` does not move any of them.
  [`dl --reconcile`](#reconciling-records-that-disagree) is what fixes them.

Note that *every* directory two levels under `<cache>/devlaunch/repos` is a
candidate — a stray directory somebody left there is looked at like any other.
The cache is `dl`'s to manage; things that are not clones do not belong in it.
But `git` cannot say what a directory that is not a repository holds, and
"cannot say" is kept rather than removed, so clearing junk out of the cache
takes `--force`. That is the same refusal a clone with a half-written `.git`
gets, and deliberately so: telling the two apart would mean `--prune` forming
its own opinion about a directory `dl <workspace> rm` already refuses on.

```
$ dl --prune
Clone directories under /home/you/.cache/devlaunch/repos:

Removing 2 that nothing references -- 1.4 GiB:
  - /home/you/.cache/devlaunch/repos/blooop/bencher/bencher-test1-pipagito (1.1 GiB)
  - /home/you/.cache/devlaunch/repos/blooop/devlaunch/devlaunch-t1-vebilote (317.0 MiB)

Leaving 3:
  - /home/you/.cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-zovomobo: workspace devlaunch-main-zovomobo still opens it
  - /home/you/.cache/devlaunch/repos/blooop/wayfinder/wayfinder-devlaunch-kilarabo: holds 2 unpushed commit(s) -- add --force to remove it anyway
  - /home/you/.cache/devlaunch/repos/blooop/rockerc/rockerc-main-ludomane: devpod lists workspace rockerc-main-ludomane and sources it at /home/you/.cache/devlaunch/repos/blooop/rockerc/main; see devlaunch#88

Dropping 12 record(s) of directories already gone.

Are you sure? [y/N]
```

`-y` skips the question. **A clone holding uncommitted or unpushed work is kept
and named**, in the same words [`dl <workspace> rm`](#cleaning-up-workspaces)
refuses in — 13 of those 37 stale clones did, two of them with real unpushed
commits, so this is load-bearing rather than a formality. `--force` promotes
that one case and nothing else. Erring this way costs you a flag; erring the
other way costs work that cannot be recovered.

The sizes are the same *exclusive* bytes `dl --ls --size` reports, and they mean
[the same thing](#how-much-disk-a-workspace-costs): what removing that directory
would actually free, not what `du` would print. Where a walk could not read
something the figure reads `≥` and so does the total, because a floor printed as
a total is a cleanup tool telling you a directory is small when it is not.

Directories that will not come away are named the same way [a purge names
them](#when-part-of-the-cache-will-not-go), the rest still go, and the exit
status is `1`.

**Nothing here runs on its own.** A full scan measured 1017 ms on that host —
about two warm launches — and it gets slower exactly as the cache gets fuller,
so it is never on a launch path and never folded into `dl --ls`. Answering `n`
*is* the read-only view; there is no separate flag for it. It costs one
`devpod list` to build the plan and no `devpod status` at all, because whether a
workspace is running has no bearing on whether a directory is opened by one. A
run you say yes to pays a second `devpod list` before it removes anything, and
classifies every directory again: a launch that finishes while the report is on
screen registers a workspace for one of the directories in the plan, and that is
the one thing the plan cannot be re-checked against from disk. The set you
approved can shrink between the report and the act. It can never grow.

It also drops the `metadata.json` records of directories that are already gone.
That file was append-only in practice — 49 records for 17 live workspaces on the
same host — and this is the first thing that prunes it.

#### The disk neither command frees

Both commands end on the same line, in the same words:

```
$ dl --prune -y
...
Removed 2 clone director(ies) -- 1.4 GiB.
devlaunch does not manage Docker images: the images these workspaces built may still hold disk, and `docker system df` shows what Docker is holding.
```

The gigabytes a cleanup reports are usually not the ones you are looking for. On
the host this was measured, `--prune` had 4.00 GB of stale clones to give back
while `docker system df` read **86.5 GB of reclaimable images, 43.18 GB of
volumes and 13.88 GB of build cache** — an order of magnitude more, sitting
behind a command that had just said "Removed". Saying nothing is what makes a
freed figure read as *all* of it, so both commands say this instead, whether they
removed 40 clones, found nothing to remove, or were answered `n` at the
confirmation. The report you get for saying `n` is a reason to print it, not an
exception: that is where somebody is deciding what is worth deleting.

**The sentence used to say "images or volumes", and volumes came off it**
(devlaunch#325). Deleting a workspace now removes the named volumes its
devcontainer created — see [what a delete takes with
it](#what-a-delete-takes-with-it) — so a disclaimer that still covered them would
be describing a leak that has been fixed. The `--prune` half of the pair still
frees no volume at all, and that is not an oversight either: it removes clone
*directories* and never deletes a workspace, so there is no workspace whose
volumes it could be taking.

**It is a sentence, not a measurement.** `dl` runs no `docker` command to print
it, so there is nothing to be slow and nothing to fail where Docker is absent,
stopped, or reachable only as another user. The figures above are this README's,
from the host it was measured on, not from your machine — `docker system df` is
where yours are.

**And it points rather than offers.** There is deliberately no `dl` flag that
removes an image, and no list of image ids here to paste into `docker image rm`.
Images devpod builds carry no devlaunch or devpod label, so any list `dl` printed
would be a guess at which of them belong to these workspaces, and `docker image
prune -a` is not scoped to devlaunch at all — it would take images built by
everything else on the machine. Deleting them is a decision with your own
containers on the other side of it, and `docker system df` is the tool that shows
you what it costs.

### Reconciling records that disagree

`dl` keeps its own record of every workspace, and devpod keeps one too. They
agree until the naming that connects them moves — and it did move once, when
workspace ids and clone-directory names gained a hashed suffix. `dl`'s records
were migrated to the new naming; devpod's were not, because nothing knew to
touch them. On the host that reported it, **36 of 39 devpod workspaces recorded
a source folder that was missing, or was a stub with no `.git` in it**, while the
real checkout sat next to it under the new name. Nothing was deleted and nothing
was corrupted: `dl` was simply asking devpod about workspaces devpod had never
been given, and devpod was answering correctly that there were none.

Two things fix that, and they are different jobs. `dl` now **writes the devpod
workspace id down** when it creates a workspace, so the naming can move again
without taking anything with it — that is automatic and needs no command. It
does nothing for the records that already disagree, because they were written
before there was a field to write it in. `dl --reconcile` is for those:

```
$ dl --reconcile
devpod workspaces sourced under /home/you/.cache/devlaunch/repos at something that is not a clone:

Re-pointing 2:
  - devlaunch-main: .../blooop/devlaunch/main -> .../blooop/devlaunch/devlaunch-main-zovomobo
  - bencher-test1: .../blooop/bencher/test1 -> .../blooop/bencher/bencher-test1-pipagito

Each of these needs `dl <workspace> recreate` afterwards: the container
still has the old source bind-mounted, and no record change moves a mount.

Leaving 1, which dl will not guess at:
  - rockerc-main (.../blooop/rockerc/main): no clone of that repository answers to this name

Nothing here is deleted. `dl <workspace> rm` is how one goes, if it should.
```

It matches the two sides **by path, never by id** — the id is the thing that
changed, so it connects nothing, while the source folder devpod kept still names
the owner and the repository exactly, and its last component still names the
branch in one of the three ways `dl` has named a clone directory. Where that
match is not unique it is refused rather than guessed: a clone a live workspace
already opens — at it, or anywhere under it — is never taken from it, a clone two
dead records both match is claimed by neither, and a name that two clones answer
to (the old flattened spelling turned `feature/auth`, `feature auth` and
`feature:auth` all into `feature-auth`) adopts neither of them. If a live
workspace's source cannot be followed at all, the whole command stops the way
`dl --prune` does, because such a workspace could be holding any of the clones on
offer. **Nothing is ever deleted.** A workspace `dl` cannot match
is named and left exactly where it is, because whether a workspace is finished
with is not something `dl` can know, and the two mistakes are not the same size.

Run it as often as you like — a repaired workspace is no longer sourced at a
non-checkout, so a second run finds nothing to do.

**A re-pointed workspace still needs rebuilding.** Its container was built with
the dead path bind-mounted into it, and changing a record does not move a mount.
`dl <workspace> recreate` is what finishes the repair, and it is the step that
needs Docker.

**Do not point an old `dl` at a reconciled cache.** A `dl` from before the naming
changed derives the old directory name, does not find it, and treats the launch
as a cold one: it clones a second directory under the old name, registers a
second devpod workspace, and rewrites that branch's record with the old naming
and an empty workspace id — undoing the repair for that one workspace, and
leaving you two clones of the branch. It is not destructive and the next
`dl --reconcile` sorts it out, but a machine that runs both builds against one
cache will keep re-breaking. Upgrade the old one, or give it its own
`XDG_CACHE_HOME`.

### Cleaning up workspaces

One workspace per branch means workspaces accumulate, and `--purge` is the wrong
tool for tidying: it is all-or-nothing and takes the caches with it.

**devlaunch does not decide which workspaces are finished.** Whether a piece of
work is over is a fact about a ticket, a review, or somebody's intent, and `dl`
knows about clones and containers. Inferring it from the branch — merged into
the default, or deleted from the remote — was tried and dropped: it reads like a
git fact but is a guess at intent, and it cannot tell a squash-merged branch
from an abandoned one. So `dl` supplies the two halves a tool that *does* know
needs, and that tool drives the cleanup:

```bash
dl --ls --json          # what exists, and what each workspace holds
dl --ls --json --size   # ...and what removing each one would free
dl <workspace> rm       # remove one
```

The JSON reports, per workspace: `id`, `devlaunch` (did `dl` create it),
`repo`, `branch` (what the workspace was made for), `checkedOut` (what its clone
is on now, which can differ), `path`, `state`, `lastUsed`, and — the field a
cleanup tool must not ignore — `unsaved`:

```json
{
  "id": "devlaunch-wayfinder-devlaunch-80-ladepomi",
  "devlaunch": true,
  "repo": "blooop/devlaunch",
  "branch": "wayfinder/devlaunch-80",
  "state": "Stopped",
  "unsaved": {
    "wouldLose": "2 uncommitted change(s) (pixi.lock, notes.md) and 1 unpushed commit(s)"
  }
}
```

`unsaved` is an object with exactly one key, and the key says which of three
answers it is:

| `unsaved` | Meaning |
| --- | --- |
| `{"nothingToLose": true}` | Everything in the clone exists on a remote too. Deleting it costs nothing. |
| `{"wouldLose": "<what>"}` | Uncommitted changes (untracked files included), commits no remote has, or both. |
| `{"couldNotTell": "<why>"}` | `git` could not read the clone as a repository — a half-removed `.git`, an interrupted delete. The files are still there and nothing has established that they exist anywhere else. |

The changed paths are named, not just counted, and that matters more than it
looks: a devcontainer that runs a package install in its `postCreateCommand` can
leave a tracked lockfile modified in *every* workspace it builds — this repo's
own did, until that install became `pixi install --frozen` — and as a bare count
that is indistinguishable from an hour of unsaved work. A cleanup tool believing the count would then never clean anything. Named,
it is judgeable. A workspace `dl` did not create reports `devlaunch: false` and
no `unsaved` — `unsaved` is `null` exactly where `devlaunch` is `false`, and
nowhere else: there is no clone of `dl`'s to protect, and it has no business
inspecting your checkout. (`repo` and `branch` are a weaker test and not the
same set: they come from `dl`'s metadata record, and a clone `dl` owns can have
lost its record while the clone and the work in it are still on disk. That clone
is inspected and reported like any other.)

**`dl <workspace> rm` refuses to delete a clone it would lose work from — when
the recorded clone holds unsaved work, and when it cannot tell what that clone
holds**, so a caller that forgets to read the field is still caught. (Recorded,
because that is the directory the guard reads; the case with no record is
neither, and is described below.)

```
$ dl blooop/repo@feature rm
error: devlaunch-repo-feature-xyz holds 1 unpushed commit(s).
       Push or commit it, or run: dl blooop/repo@feature rm --force
```

```
$ dl blooop/repo@feature rm
error: devlaunch-repo-feature-xyz: git could not read /home/…/repo/feature:
       fatal: not a git repository. devlaunch will not delete a clone it cannot
       check. Look at it, or run: dl blooop/repo@feature rm --force
```

That refusal is the only judgement `dl` makes here, and it is not about finished
work — it is `dl` declining to destroy the only copy of something, including
when it cannot prove there is another copy. Say `--force` if you mean it.

`--force` changes one more answer: an already-absent workspace counts as
deleted, like `rm -f`. Unforced, `rm` reports devpod's refusal to delete a
workspace it does not have; forced, the contract is the state afterwards, not
that a delete happened — which is what lets the [cold benchmark's per-run
reset](#measuring-launch-time) run before the first launch, when there is
nothing to remove yet.

The guard reads `dl`'s metadata record, so the recorded directory is the one it
asks about. (The delete does not always remove that same directory: when the
recorded path is not on disk it falls back to a derived one. That divergence is
older than this guard and is tracked as devlaunch#174.) One case is therefore
neither a refusal nor a delete: a clone under `dl`'s cache that has **no** record — a metadata write
that failed, a record pruned, a cache restored without one. The listing still
reports what that clone holds, so `unsaved` is the field to read; but `rm`
removes the devpod workspace, exits `0` without asking for `--force`, and leaves
the clone on disk, because there is no recorded directory for it to remove
either. Nothing is destroyed, and nothing then points at the clone: it is yours
to keep or to `rm -rf` by hand.

[`wf`](https://github.com/blooop/wayfinder) is the caller this was built for: it
names its branches after its tickets, so it knows which workspaces belong to
finished work and removes those.

### How much disk a workspace costs

`dl --ls --size` adds a `SIZE` column, and `dl --ls --json --size` adds a `disk`
object beside the other per-workspace facts:

```
$ dl --ls --size
WORKSPACE                      TYPE   SOURCE                                              SIZE  LAST USED
kinisi-ros-main-lubadaha       local  /home/…/repos/kinisi-robotics/kinisi_ros/main    64.9 MiB  2026-08-08 11:43:27
my-own-checkout                local  /home/…/projects/scratch                                -  2026-08-01 09:12:04
```

**The number is what deleting that workspace would give back, not what `du`
prints.** Those differ, and the gap is the point of the design. A repo is cloned
once into a bare cache and every workspace clone hardlinks its git objects out
of that one copy, so the objects exist once on disk however many workspaces
share them. A size that walked each workspace on its own — which is what `du`
does when you point it at one directory, counting the blocks every file in it
occupies — bills each workspace for the whole shared pool.

The measurement the row above comes from, taken with the shipped code on one
machine (Ubuntu 24.04, ext4, warm page cache) on a real clone of that repo made
by `git clone` from the bare in `dl`'s own cache:

| | bytes |
| --- | --- |
| `du -s --block-size=1` on the clone alone | 353,230,848 |
| what `dl --ls --size` reports for it | 68,050,944 |
| what `dl --ls --size` reports for the bare it clones from | 651,264 |
| `du -sc --block-size=1` over both together | 353,882,112 |

`du` bills that workspace **5.2x** what deleting it would actually free. The
difference is a single 270,823,424-byte pack file with one link in the clone and
one in the bare, so removing either end frees none of it.

**That sharing is a promise, not a coincidence, and a test holds it to that.**
`git clone <path> <path>` hardlinks pack files by default, and the default is
all that was ever keeping it true — a `file://` URL, an intermediate copy, or an
explicit `--no-hardlinks` would each forfeit it with nothing failing and no
warning printed. Measured on this repo — `du -sc` over the cache and each
clone's `.git`, ext4, git 2.55.0 — that is 2400 KB for the cache plus one
workspace against 4472 KB unshared, and 196 KB rather than 2268 KB of `.git` for
every workspace after the first. So an integration test asserts the pack files are
the cache's — same inode, more than one link — and that assertion goes red on
all three. No clone flag is used to guard it: `--local` is already the default
and does not even reject a `file://` source, and `--shared`/`--reference` were
measured to leave a workspace that fails `git fsck` once the cache has fetched
and gc'd, for a 2 KB saving.

Sharing does erode, in one measured way that is a safety property rather than a
fault: when the cache repacks, an existing workspace's pack loses its second
link and becomes that workspace's own complete copy, still passing `git fsck`.
The workspace stops being cheap and never stops being valid — which is the trade
`--shared` and `--reference` get wrong, and the reason they are not used.

**Large files are shared the same way, but nothing about `git clone` does it for
you.** git-lfs objects are not git objects: the clone does not carry them at
all, so a workspace of an LFS repo used to download the entire payload from the
forge and keep a private copy of it in `.git/lfs/objects` — every workspace,
every time, on top of the worktree copy. `dl` now makes the bare cache the
repo's LFS store as well: the payload is fetched once into `<repo>/.bare/lfs`
for the branch being launched, and each workspace materializes out of *that*,
which git-lfs does by hardlinking. Measured with git-lfs 3.7.1 on ext4: the
workspace's object file is the same `(st_dev, st_ino)` as the cache's, so its
store costs nothing, and the materialization succeeds with the remote deleted
from disk — the second workspace of an LFS repo touches the network for its
large files not at all. What remains per workspace is the worktree copy, which
is real bytes and cannot be shared: a container build has to be able to read
them. If the cache cannot supply an object — a first launch offline, a payload
the branch alone introduces — the old download from `origin` still runs, and a
workspace left holding pointer files is retried on the next launch rather than
written off.

Nothing about that is written into the workspace's `.git/config`, and that
restraint is load-bearing rather than tidy: `dl` bind-mounts the *clone*
directory into the devcontainer and `.bare` is a sibling that is not mounted, so
an `lfs.storage` entry or an added remote naming a host path would break every
`git checkout` of an LFS repo inside the container while working perfectly on
the host. A test asserts the clone keeps exactly one remote, still pointing at
the forge, and no `lfs.storage` at all.

So `dl` counts a file only when every one of its hardlinks lies inside the
workspace being measured. Two consequences, both deliberate:

- **The sizes do not add up to the size of the cache.** Bytes shared between
  workspaces belong to none of them, because deleting any one frees none of
  them. They become the last workspace's the moment it is the last one — which
  is exactly when deleting it *would* free them. In the table above that is the
  last two rows read against each other: 68,702,208 reported bytes against
  353,882,112 held.
- **A workspace's size can change without the workspace changing**, when a
  sibling that was sharing with it goes away. That is the truth about shared
  storage.

A workspace `dl` did not create reads `-` (`null` in JSON): there is no clone of
`dl`'s there to measure, and walking your own project directory is not `dl`'s to
do. The table and the JSON decide that from the same rule — is the clone one
`dl` put in its own cache, the same question `--purge` deletes by — so the two
always name the same set of workspaces as measurable. Where a walk hits a
directory it cannot read — a container writes into its
clone as its own user, so this happens — the answer is a floor rather than a
total: `≥2.0 MiB` in the table, and `{"atLeastBytes": …, "unreadable": 1}` in
JSON instead of `{"exclusiveBytes": …}`. A partial measurement never comes back
looking like a complete one.

**It is opt-in because it walks the whole clone.** Plain `dl --ls` is one devpod
round-trip and no filesystem work at all, and the walk is O(files) with no
ceiling. Measured with the shipped code on one machine — Ubuntu 24.04, ext4,
warm page cache, five runs after a warm-up, the machine otherwise busy — a real
8,309-entry clone walked in 24–28 ms, this repo's own tree with its built
environment inside it (9,124 entries) in 17–21 ms, and a 114,817-entry tree in
232–239 ms. No cold-cache figure is quoted because none was taken: dropping the
page cache needs root on that machine. Those are one machine's numbers on warm
cache and yours will differ, but the shape is the point — it grows with the file
count, and a devcontainer that builds its environment *inside* the clone (this
repo's own does) is most of that count. That is not a bill a listing should
present unasked.

Docker images and named volumes are not counted: `dl` did not create the layer
store, and a volume is not a directory it can walk. `docker system df` is the
tool that knows — the same boundary [`--prune` and `--purge`
name](#the-disk-neither-command-frees) when they finish. Not counting a volume is
a different thing from not removing it: a workspace's volumes go when the
workspace does ([what a delete takes with
it](#what-a-delete-takes-with-it)); what is missing here is only the *figure*.

## The shared pixi package cache

Every container `dl` creates gets one host directory bound into it, and
`PIXI_CACHE_DIR` pointed at it, so that dotfiles which provision their tools with
`pixi global sync` download each package once per machine instead of once per
container:

| | |
|---|---|
| **On the host** | `$XDG_CACHE_HOME`, or `~/.cache`, then `devlaunch/pixi` |
| **In the container** | `/var/tmp/devlaunch-pixi` |

Measured on the profile this was built for — 23 pixi-global environments — a
container with a cold cache spends 62–113 s and downloads 1.2 GB; one that finds
the packages already there finishes in 18–28 s and fetches nothing. Two containers
syncing against it at the same time is fine: the downloads are content-addressed
and rattler takes a lock per package.

**Deleting it is always safe, at any moment, including while containers are
running.** It holds nothing but downloaded package archives — every one of them
re-fetchable from the network, and none of them referenced by a path anything
inside a container has stored. The worst a deletion costs is the next container's
download.

```bash
rm -rf ~/.cache/devlaunch/pixi
```

`dl --purge` takes it away with the rest of `~/.cache/devlaunch/`, for the same
reason.

Two things it deliberately is not. It is **not the host's own**
`~/.cache/rattler/cache`: containers write into it as their own remote user,
whose uid only happens to match yours, and a `pixi clean cache` you run for your
own reasons must not be able to pull packages out from under a running container.
And it is **not a shared `PIXI_HOME`** — the installed environments and their
trampolines are baked with absolute paths, and two containers sharing one
environment tree is [pixi#5476](https://github.com/prefix-dev/pixi/issues/5476).
Only the download cache is shared, which is the part that is safe to share.

If the directory cannot be created, or is not there when the launch reaches it —
a full disk, a read-only cache home, a cache swept between the two — the launch
goes ahead without the mount and the container downloads its own packages,
exactly as it did before this existed.

**Sharing requires the container's user to be able to write the directory**,
which in practice means its uid matches yours or it is root. The mount carries
host ownership through unchanged, and pixi does not degrade to reading a cache
it cannot write: pointing `PIXI_CACHE_DIR` at a directory owned by another uid
fails the install outright (`Permission denied` on the repodata, exit 1) even
when every package it wants is already in there. So an image whose remote user
is neither root nor your uid does not merely lose the sharing — its `pixi global
sync` fails, and its tools do not get provisioned.

`dl` cannot see the container's uid before it launches, so it cannot decide this
for you. In practice the common case is safe: every mainstream base declares a
remote user at uid 1000, which is the first human user on a Linux host. If you
hit the failure, the fixes available to you are to run that image as your own
uid, or to take the cache out of play for it (`rm -rf ~/.cache/devlaunch/pixi`
recovers a directory an earlier container left owned by someone else).

The case that is not a developer's machine is CI. What makes the common case
safe is that uid 1000 is *both* the base image's remote user and the first human
user on a Linux host — and on a hosted runner it is only the first of those. A
GitHub runner's own user is somebody else, so a launch there hits this on its
first container and every container after it. This repo's own launch benchmark
did exactly that for twenty consecutive merges to `main`: `failed to create
directory /var/tmp/devlaunch-pixi/pkgs: Permission denied`, from the benched
repo's `pixi install`, before anything was timed.

Where you know the uid you are handing the directory to, there is a third fix
the list above does not offer, and it is what `.github/workflows/bench.yml` now
does — create the directory yourself and widen it, before the first launch:

```bash
mkdir -p ~/.cache/devlaunch/pixi && chmod 1777 ~/.cache/devlaunch/pixi
```

`dl`'s own `mkdir` does not re-mode a directory it finds, so the mode survives
every launch after it. `1777` is what `/var/tmp` carries at the other end of the
same mount, and it makes the same trade: every uid can write, and the sticky bit
means none of them can unlink another's entries.

### Where the tools themselves land

The cache above is shared; the *environments* devlaunch installs into are not,
and they are not the container's `~/.pixi` either. `gh`, `claude` and `zellij` go
into **`~/.devlaunch/pixi`**, a pixi home of devlaunch's own, because
`pixi global install` is not only an install — it is an edit to
`$PIXI_HOME/manifests/pixi-global.toml`, a declarative file that in a container
already has an owner. Writing there made devlaunch a second author, and cost
something in both directions:

- `pixi global sync` removes every environment the manifest does not list, so a
  dotfiles apply that rewrites the manifest and syncs **uninstalls** the zellij
  devlaunch just installed — and the next launch reinstalls it, forever.
- The manifest is not always a file. A devcontainer is free to symlink
  `~/.pixi/manifests/pixi-global.toml` onto a tracked file inside the checkout,
  and one does; the append then landed in the work tree and every `git status`
  in the workspace came up dirty.

Neither is expressible against a home devlaunch created: nothing syncs that
manifest, and no repo state can sit under that path. It costs a duplicate
extracted prefix in the one case where a tool is installed but unreachable from
a login shell — disk only, since `PIXI_HOME` does not move the download cache —
and that is the case where the old behaviour reinstalled on every launch anyway.

Not `~/.local/share/devlaunch/pixi`, which is the conventional path and the wrong
one here: containers bind-mount `~/.cache`, `~/.config` and `~/.local/share`
straight from the host, so a prefix tree under one of them would be shared by
every container on the machine and written into your own home — pixi#5476 again,
the hazard the cache mount is careful to keep `PIXI_HOME` away from.

`PIXI_HOME` is set only for devlaunch's own install scripts, never exported into
the login profile, so **your own `pixi global install` in a workspace still goes
to your own `~/.pixi`**. Only the bin directory goes on `PATH`.

### Existing containers, and what a recreate is for

**A mount lands only when a container is created.** devpod re-applies
`--workspace-env` on every `up`, but it will not add a bind mount to a container
that already exists — passing `--mount` there is a silent no-op. So a container
built before this feature, or before a change to where the mount lands, keeps
whatever it was created with until `dl <workspace> recreate`, and only then
picks the current arrangement up.

In between, `PIXI_CACHE_DIR` points at `/var/tmp/devlaunch-pixi` with nothing
mounted on it. That is a working private cache, not a failure — `/var/tmp` is
world-writable in every image, so pixi creates the directory and fills it. The
container re-warms itself and simply never shares, abandoning whatever pixi had
already warmed in its default location. **This is the reason the container-side
path is under `/var/tmp` rather than somewhere tidier like `/var/cache`:** a
target whose parent is root-owned is a hard `pixi global sync` failure on every
container that predates it, not a lost optimisation.

One older breakage needs the recreate rather than a restart. Devlaunch briefly
mounted this cache inside `~/.cache`, which left that directory root-owned in
any image that ships no `~/.cache` of its own. `$HOME` lives on the container's
own layer, so `dl <workspace> stop` and a fresh `up` keep the root-owned
directory; `dl <workspace> recreate` gets a new layer where `~/.cache` is the
user's own again.

## Worktree Backend

For git repositories, devlaunch uses an efficient worktree backend by default:

- **Efficient Storage**: Repos are cloned once to `~/.cache/devlaunch/repos/owner/repo/`, then git worktrees are created for each branch
- **Shared Git Objects**: All branches share git objects, saving disk space
- **Targeted Fetch**: A launch fetches only the one branch it is launching, so no launch waits on a repo-wide refresh

### What you get when you push and immediately launch

- **Attaching to a workspace devpod already knows**: no git at all. The workspace
  is exactly as you left it; freshness inside it is your own `git pull`.
- **A cold launch** (first time this branch is launched on this machine, or a
  clone devpod has forgotten): one targeted fetch of that branch, every time.
  Push upstream and immediately `dl` the branch and you get the pushed tip.
- **A branch that does not exist yet**: created from the default branch's freshly
  fetched tip.
- **Offline**: a warning, and the launch proceeds from whatever the cache holds.
  It only fails when there is nothing cached to launch from.
- **Everything else** (other branches, tags, prunes) is refreshed by the
  background updater within the configured interval (default: 1 hour), which
  never blocks a launch.

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

`dl <workspace> up` is the other way to prepare one, and running it repeatedly is
cheap on purpose: against a container that is already up, a second `up` costs one
`devpod status` and nothing else. It used to also pay the tools setup pass —
~1.7s of `devpod ssh` to be told the tools it was told about last time — and now
reuses the recorded answer instead. See
[The trip a launch can skip](#the-trip-a-launch-can-skip) for what makes a recorded
answer stop being believed; the short version is that any completed `devpod up`,
by anything, does.

## Measuring launch time

Set `DEVLAUNCH_TIMING=1` and a `dl` command ends with one summary on stderr,
naming each subprocess round trip and the total. Unset (or `0`) records nothing
and prints nothing.

Captured from a real warm launch (the launch's own output elided):

```bash
$ DEVLAUNCH_TIMING=1 dl-next blooop/mcp-devtasks -- true
...
dl-timing: devpod status 0.454s
dl-timing: gh auth token 0.036s
dl-timing: devpod ssh 1.952s
dl-timing: total 2.444s (in-process, excluding interpreter startup)
```

### The same launch, machine-readable

`DEVLAUNCH_TIMING=json` swaps that prose for one document on a single
`dl-timing-json:` line, so a trend job can read a launch without scraping
prose. It decomposes the launch into five **ownership-boundary stages** — one
per party that could actually make it faster — with the round trips nested
inside the stage that paid for them:

| stage | what it owns |
|---|---|
| `handoff` | the gap between the keystroke that resolved to this exec and dl starting (see the stamps below) |
| `host-prep` | the host's own git work — the bare clone and its fetches, the lock waits, the LFS probe and, for an LFS repo, the cache's LFS fetch and the workspace's materialization out of it — and the `gh auth token` trip, wherever on the launch it falls |
| `devpod-up` | the arm that gets a container running: the existence probe and, when it is not running, the `up` itself. On a warm launch that arm is the probe alone |
| `tools` | the probe trip and the conditional lend, including staging the payload tar |
| `attach` | the last trip, into the running command |

Two rules are worth knowing before reading one:

- **A stage that never ran is absent, not zero.** A warm launch reports no
  `host-prep` at all, because it did none. A stage that failed is present,
  timed up to the failure, and marked `failed`.
- **A stage totals over its whole arm**, not just over its round trips, so the
  host-side work between two spawns is attributed rather than lost. Stages
  never double-count each other: `tools` runs inside the launch `devpod-up`
  brackets, and those seconds are charged to `tools` alone. The in-process
  stages therefore add up to the total — measured on a real cold launch below,
  they came to 20.834s against a 20.834s total.

`handoff` is the exception to that sum, and the only one: it ends where
`total` begins, so it is time the process could not have measured from inside
itself. A consumer adding stages up against the total leaves it out.

Two optional environment variables let whatever launches `dl` — a shell
function, an agent front-end — close the loop on the time before dl existed.
Both are Unix epoch seconds, which is what `date +%s.%N` prints:

| variable | meaning |
|---|---|
| `DEVLAUNCH_HANDOFF_T0` | the keystroke that resolved to this exec. Becomes the `handoff` stage — the only measurement of exec plus interpreter startup there is, since `total` begins after both |
| `DEVLAUNCH_PREWARM_FIRED_AT` | when a prewarm (`dl <ws> up`) was fired for this workspace, if one was |

With the prewarm stamp set, the document also reports what that prewarm was
worth: the head start it bought, and which shape the launch then took — `hit`
(the workspace was already up), `partial` (this launch queued behind a prewarm
still running) or `miss` (this launch ran the `up` itself). dl decides that,
not the firer: a prewarm is fired and forgotten, so only the launch that
followed can see whether it helped. **A stamp that is missing, unreadable, or
ahead of this clock reports nothing** rather than a zero — an absent handoff
and an instantaneous one are different facts, and a trend cannot tell them
apart once one is written as the other.

Captured from a real warm launch with both stamps set (one line, wrapped and
elided here for reading):

```bash
$ DEVLAUNCH_TIMING=json DEVLAUNCH_HANDOFF_T0=$(date +%s.%N) \
    DEVLAUNCH_PREWARM_FIRED_AT=... dl-next blooop/mcp-devtasks -- true
...
dl-timing-json: {"total": 2.210768, "total_epoch": "in-process, excluding interpreter startup",
  "stages": [{"stage": "handoff",   "seconds": 0.130542, "outcome": "ok", "spans": []},
             {"stage": "host-prep", "seconds": 0.027799, "outcome": "ok",
              "spans": [{"label": "gh auth token", "seconds": 0.02747}]},
             {"stage": "devpod-up", "seconds": 0.455188, "outcome": "ok",
              "spans": [{"label": "devpod status", "seconds": 0.455158}]},
             {"stage": "attach",    "seconds": 1.726961, "outcome": "ok",
              "spans": [{"label": "devpod ssh", "seconds": 1.72661}]}],
  "prewarm": {"head_start_seconds": 42.489243, "shape": "hit"}}
```

Stages appear in the order the launch first entered them, and only the ones it
reached appear at all — this warm launch built nothing and lent nothing, so
there is no `tools` stage and no `devpod up` inside `devpod-up`. That
`handoff: 0.131s` is the exec and the interpreter start, which nothing else
measures.

The cold launch of the same repo, same host and session, decomposed as:
`host-prep` 2.257s (`git clone --bare` 1.602 + `git fetch` 0.427 + workspace
`git clone` 0.065 + LFS probe 0.002 + token 0.034), `devpod-up` 5.848s (`devpod
up` 5.113 of it), `tools` 10.224s (probe trip 1.584 + `tools tar` 0.111 +
transfer 8.445), `attach` 2.505s — 20.834s of stages against a 20.834s total.

For before/after numbers, `scripts/bench_launch.py` runs a command N times and
reports the median — one command per side of a change:

```bash
python3 scripts/bench_launch.py -n 5 -- dl-next owner/repo -- true   # warm launch
```

(`pixi run bench -n 5 -- ...` in the devcontainer.) It reports no median if any
run fails, so a broken launch cannot pass as a fast one. See `bench_launch.py
--help` for `--before` — the per-run reset that makes a *cold* median cold and
whose `rm --force` also succeeds on the first run, when there is nothing to
remove yet — and for why its wall clock and `dl-timing: total` are not the
same quantity. For scale, on the host the session above was captured on, the
warm median over 5 runs was 2.176s. Running the cold recipe exactly as the
epilog writes it — `-n 5`, container recreated per run — gave a median of
15.899s (runs: 15.9, 20.0, 15.2, 15.7, 17.8). Read that as the cost of
recreating a container, not of a first-ever launch: the reset removes the
workspace but leaves the docker image layers and the bare clone cache, so
every run after the first starts from both. A machine that must also pull or
build the image pays more, by an amount this recipe does not measure — but the
gap is large: an earlier 3-run median on this same host, reported as its first
real launch, was 33.204s.

Every number in the two paragraphs above was copied into this prose by hand.
`--record` is how that stops: it writes the same invocation as one JSON object
a trend job can upload without anyone reading it.

```bash
python3 scripts/bench_launch.py -n 5 --record warm.json --shape warm \
    -- dl-next owner/repo -- true
```

The record holds the command, the run count, each run's wall time and
per-stage seconds, and the medians of those — and nothing else, because a CI
job stamps its own commit, clock and host better than this script can. Four
things about it are load-bearing:

- **The median is the point, the runs are its evidence.** A trend compares a
  point against the immediately previous one, so N runs published as N points
  would read ordinary spread as a regression.
- **A stage no run reported is absent, not zero.** A warm launch legitimately
  has no cold-path stages; a zero would claim the work happened instantly
  rather than not at all. A stage only *some* runs reported is a median over
  those runs, carrying a count of how many — a median of two and a median of
  five are not the same claim.
- **Recording asks the launch for its stages** (`DEVLAUNCH_TIMING=json`), so a
  run that reports no timing document is an error and no record — same
  discipline as no median over a failed run.
- **`--shape` labels the trend line** (`warm`, `cold-recreate`). It is the
  caller's to say: the same command benches either shape depending on
  `--before`, and a wrong label is worse in a trend than a missing one.

### The trend on main

Every push to `main` runs `.github/workflows/bench.yml`, which benches both
shapes on the runner and publishes one point per stage to
<https://blooop.github.io/devlaunch/dev/bench/>. It can also be dispatched by
hand. Reading it needs nothing but the chart; what follows is for changing it.

`scripts/bench_points.py` is the step between the two formats — bench records
in, one flat array of trend cases out:

```bash
python3 scripts/bench_points.py warm.json cold-recreate.json --out bench.json \
    --require-stages-on cold-recreate
```

(`pixi run bench-points ...` in the devcontainer.) One case per stage the shape
reported, plus that shape's own total: `warm / host-prep`,
`cold-recreate / devpod-up`, `warm / total`. Six properties of it are
load-bearing:

- **The published value is the median, and the case name is a key.** The trend
  compares a point against the immediately previous point and nothing older, so
  the de-noising has to have happened before publishing. Renaming a stage
  starts a new, empty series beside a frozen old one.
- **The spread rides along as the point's error bar**, and the outside
  stopwatch (`wall=`) as part of its `extra`. Evidence beside the number,
  rather than a second trend line for the same launch that disagrees with the
  first by a constant.
- **An absent stage is absent.** A warm launch lends nothing, so there is no
  `warm / tools` case at all — never a zero, which would claim an instantaneous
  lend and would drag the line down exactly where a regression should show.
- **`--require-stages-on` fails the job rather than the trend.** The way this
  decomposition is expected to break is an absence, not a wrong number: a stage
  stops being emitted and the total keeps working. So the cold-recreate shape,
  where every stage is known to be present, asserts them all, and a run that
  lost one publishes nothing and goes red. Naming a shape that was not benched
  fails too — an assertion that covers nothing reads exactly like one that
  passed.
- **A record that is missing or unreadable refuses the same way.** The step
  that writes the records can exit 0 without having written one, so the absence
  arrives here — and it prints `bench_points: <reason>` naming the file and
  writes nothing, like every other refusal, rather than a traceback.
- **A regression alerts; it never gates.** The workflow is deliberately not a
  job in `ci.yml`: a job there would join the CI gate's `needs` by house
  convention and turn a noisy wall-clock measurement into a merge gate. A point
  above the threshold leaves a commit comment and a red mark on the chart, and
  the build stays green.

## Going back to the Python build

`0.1.0` is the Rust rewrite: same commands, same cache, same `metadata.json`, different
implementation (the whole of it is in [docs/rust-rewrite-plan.md](docs/rust-rewrite-plan.md), whose
divergence table is the complete list of what changed on purpose). The last Python release is
`0.0.29`, and nothing in the cache stops you going back to it:

```bash
pixi global install --channel conda-forge --channel https://prefix.dev/blooop "devlaunch<0.1"
pip install "devlaunch<0.1"
```

Both builds read and write the same `metadata.json` at schema 2 under the same locks, so a downgrade
keeps every workspace you already have and needs no cleanup. Please open an issue for whatever sent
you back.

## Development

`dl` and `aid` are Rust. `rust/` is what ships: one cargo workspace, built and tested with cargo,
and it is where the tests of `dl`'s own behaviour live.

```bash
cd rust
cargo build --release                          # target/release/{dl,aid}
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Those cargo commands need no toolchain of your own inside this repository's devcontainer: `rust`
1.97.1 is in the `default` pixi environment, so `pixi run cargo test --workspace` works the moment a
container comes up. `pixi run` keeps the directory it was called from, so `cd rust` first, exactly as
above. `rust/rust-toolchain.toml` remains the pin of record — a conda `rust` does not read it, so the
version is named again in `pyproject.toml` and moves by hand with `ci.yml`'s three toolchain
steps.

The Python that remains is the **acceptance harness** and the repo's own documentation guards. It
judges the shipped binaries from outside — spawning them against a real devpod, or against the fake
one in `test/fixtures/devpod_shim.py` — and imports nothing that ships:

```bash
pixi run ci                                    # ruff, pylint, ty, and the harness under coverage
pixi run test-e2e                              # the real-devpod tier: builds real containers
```

Both build `rust/target/release/{dl,aid}` first, because that is what the harness runs.
`DEVLAUNCH_DL_CMD` / `DEVLAUNCH_AID_CMD` are the seams that point it somewhere else — a debug build,
an installed release, the wheel's binary:

```bash
# a bare pytest, so the release build the `test` task depends on is skipped too
DEVLAUNCH_DL_CMD='cargo run -q --manifest-path rust/Cargo.toml -p dl --bin dl --' pixi run pytest
```

### Coverage: two numbers, and neither is the other

The crates that ship and the harness that judges them are measured separately, because they are
different things and a single figure is about neither:

```bash
pixi run coverage-rust                         # cargo llvm-cov over the whole workspace
cd rust && pixi run cargo llvm-cov --workspace --html -- --test-threads=1   # ...as a tree to open
```

The task carries its own `cwd`, so it runs from anywhere in the checkout; a bare `cargo llvm-cov`
needs the `cd` like any other cargo command.

```bash
pixi run coverage && pixi run coverage-report  # the Python harness, `scripts/` and the doc guards
```

CI runs both — `rust-coverage` and `ci` — and uploads them to Codecov under the `rust` and `python`
flags, which `codecov.yml` keeps from being averaged. Before #294 only the second one existed, and
after #267 retired the Python `dl` what it measured was `scripts/`: the shipped code's coverage was
nobody's for two releases.

**One thing to know if you write a boundary test.** The suites in `rust/dl/tests` and
`rust/aid/tests` spawn the real binary with `env_clear()`, so the child gets exactly the world the
test built and nothing from your shell. An instrumented binary needs `LLVM_PROFILE_FILE` to write
its counters anywhere `cargo llvm-cov` will read them, so every one of those spawns calls
`.keeping_coverage()` straight after `.env_clear()` — the one variable that is allowed back in.
Leave it out of a new spawn and the test still passes; it just stops counting, silently, and takes
whatever it was the only cover for down with it. Measured before that seam existed, one five-test
suite that runs `dl` end to end reported `render.rs`, `commands.rs` and `cli.rs` at 0.00%.

Two cargo tests are outside the CI measurement on purpose: `dl --test interrupt` and
`--test lock_wait` kill real process trees, and a SIGKILLed process writes no counters. They run in
the `rust` job like everything else, and `pixi run coverage-rust` includes them locally.

Inside this repository's devcontainer, `pixi run dl` and `pixi run aid` are `cargo run` over the
working tree; on a host, `./dev.sh` installs it as `dl-next`/`aid-next` beside the released pair. Both
print a `-dev` version so a working-tree build is never mistaken for a released one. See
[AGENTS.md](AGENTS.md).

Until 0.1.0 this repository held a second, Python implementation of `dl`, and a parity harness that
ran both against the same fixtures and compared them. Both retired once the binaries shipped; the
history is in [docs/rust-rewrite-plan.md](docs/rust-rewrite-plan.md), whose divergence table still
records every deliberate behavioural difference from that build.

The two published artifacts are built from the same cargo package, both taking their version from
`rust/Cargo.toml` — the only place it is written down:

```bash
cd packaging/wheel && maturin build --release --locked -o dist    # the PyPI wheel: two binaries
rattler-build build --experimental --recipe conda.recipe/recipe.yaml   # the conda package
```

`.github/workflows/publish.yml` and `conda-publish.yml` do exactly that on a version bump, in that
order, off one tag; `ci.yml`'s `packaging` job builds the wheel and renders the recipe on every pull
request, so a broken release is a red tick rather than a surprise.

The Python half uses [pixi](https://pixi.sh) for environment management.

```bash
# Run tests
pixi run test

# Run the e2e suite: real devpod, real containers
pixi run test-e2e

# Run full CI suite
pixi run ci

# Format and lint
pixi run style
```

`pixi run test` skips the e2e tests, which need devpod and a Docker daemon and
build real containers. CI runs `pixi run test-e2e` in a job of its own, outside
the Python matrix, on a throwaway runner — on every push to `main` and on every
pull request, whatever branch that pull request targets. Stacked chains, where
each link targets its predecessor rather than `main`, get the same CI as anything
else.

Alongside the matrix and e2e there is a `gate` job that does nothing but fail
unless every other job in that workflow succeeded. It exists so that a branch
ruleset has one stable name to require rather than a list: requiring the jobs one
by one means literal strings in a repository setting, which nobody reviews and
which goes stale the moment a job is added or renamed — and a required check that
no longer exists does not turn a merge red, it stops gating it. Adding a job
means adding it to `gate`'s `needs`, in the same pull request, where it can be
seen. It reaches only as far as its own workflow file, so the `prek` lint job is
not behind it and has to be required alongside it.

Running it yourself is a different proposition. This repo's devcontainer carries
a Docker daemon of its own, through the `docker-in-docker` feature, and pins the
same devpod a host installs, so `pixi run test-e2e` from inside it builds its
containers in there rather than on your Docker. You can also run it on a machine
you do not mind it writing to — an ephemeral CI runner, say. It is skipped by
default rather than gated on a container, because what it needs is a daemon, not
nesting. Either way the suite exercises `dl --purge`, so it gives itself a
private devpod namespace before collection begins — but the containers it builds
are real ones, and it wants several minutes and a 1.25 GB image pull the first
time.

Its skips mean one thing only. A test that opts out does so through
`fixtures.e2e_guard.opt_out`, and any other skip is reported as a failure,
because a run that could not reach a registry used to be indistinguishable from
a healthy one. Every run also prints what it actually built:

```
--------------------------------- e2e session ---------------------------------
22 e2e tests attempted, 5 workspaces created: e2e-test-create, e2e-test-lifecycle, e2e-test-git, e2e-purge-devlaunchs, e2e-purge-hand-made
```

A run whose workspace-building tests built nothing does not pass: the shortfall
is counted into the last line of the run, so `4 passed, 18 skipped` becomes
`1 failed, 4 passed, 18 skipped`. A run with no workspace-building tests in it —
`pytest -m e2e -k TestDLCommandsE2E`, say — has nothing to answer for and says
so instead.

The nested daemon is also why the devcontainer does not join the host's network
namespace: a nested daemon needs a namespace of its own, or it co-manages the
host's `docker0` bridge and writes its NAT rules into the host's netfilter
tables.

### The prebuilt dev container image

Opening this repository's devcontainer used to build it: base image, pixi, the
local `claude-code` feature and `docker-in-docker`, several minutes of it, once
per branch. CI publishes that image now, so opening a workspace pulls it instead.

```
ghcr.io/blooop/devlaunch-devcontainer
```

Nothing has to be configured to use it. `.devcontainer/devcontainer.json` names
the repository under `customizations.devpod.prebuildRepository`, and `devpod up`
— which is every `dl` launch — checks it before building anything. The check is
for one exact tag: devpod hashes the build config together with the build
context and asks for `<repository>:devpod-<hash>`. A hit is used directly,
features included; a miss falls through to a local build, silently and without
failing. So the prebuild is a speed-up that cannot break a launch, and the
question to ask when a container open is slow is whether the tag matched.

Two consequences worth knowing:

- **The build context is `.devcontainer`, not the repository root**, and that is
  what makes the tag usable. The Dockerfile copies nothing out of the context,
  so the root was never needed — but it was hashed, which meant a different tag
  on every commit to any file and a prebuilt image that never matched one.
  Scoped to `.devcontainer`, the tag moves when `.devcontainer/**` moves, the
  Dockerfile and the local feature's scripts included.
- **A commit whose `.devcontainer/` differs from the last prebuild builds
  locally.** That is the correct answer rather than a gap: the alternative is a
  container built from something other than what the branch asks for. The pull
  comes back once the change is on `main`. What the tag does not promise is the
  converse — that one `.devcontainer/` tree always yields one image; see "What
  the prebuild tag does not promise" below.

`.github/workflows/devcontainer-prebuild.yml` publishes it, on pushes to `main`
that touch `.devcontainer/**` and on manual dispatch. Its path filter is exactly
the set of inputs to the hash, so a commit it skips is one that could not have
moved the tag. To publish from a branch by hand:

```bash
docker login ghcr.io                  # a PAT with write:packages, or `gh auth token`
pixi run devcontainer-prebuild        # devpod build . --tag latest
```

Both are idempotent: an existing prebuild is found and returned rather than
rebuilt and repushed. By hand publishes for the architecture of the machine you
run it on and no other — see "Two architectures, two tags" below — and takes the
moving alias as an argument (`pixi run devcontainer-prebuild latest-arm64`) if
that machine is not amd64.

**The package came up public on its own, and needed no manual step.** Measured on
the first run (`d05e4ce`): an anonymous pull of both tags returns `200`, with
nobody having touched a visibility setting. GHCR gave the package the visibility
of the public repository whose workflow published it — the
`org.opencontainers.image.source` label in the Dockerfile is what links the two.

This is worth checking rather than trusting, because the failure is silent: a
private package makes the lookup return `DENIED`, devpod reads that as a cache
miss, and every launch quietly builds locally — the behaviour from before any of
this existed.

```bash
curl -s -o /dev/null -w '%{http_code}\n' \
  -H "Authorization: Bearer $(curl -s \
    'https://ghcr.io/token?scope=repository:blooop/devlaunch-devcontainer:pull&service=ghcr.io' \
    | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')" \
  https://ghcr.io/v2/blooop/devlaunch-devcontainer/manifests/latest
# 200 => public, anonymous pulls work
# 401/403 => private; make it public as below
```

If it ever *is* private — a package created some other way, or a visibility that
gets changed — the fix is a one-time setting, and one no workflow can make: there
is no REST endpoint for container visibility and no `gh` subcommand.
<https://github.com/blooop?tab=packages> → *devlaunch-devcontainer* → *Package
settings* → *Danger Zone* → *Change visibility* → **Public**. Nothing to
configure before the first publish creates the package, so those pages 404 until
then.

The `:latest` tag that command also pushes is not what devpod looks for. It is
the moving alias `build.cacheFrom` points at, a best-effort layer cache for
builders that know nothing about devpod prebuilds — VS Code's "Reopen in
Container", a plain `devcontainer up`. Those still run a build; what they save is
whatever layers the cache can serve them.

**Two architectures, two tags.** The target architecture is hashed along with the
build config and the context, so amd64 and arm64 ask for different tags, and the
workflow publishes both — a matrix over `ubuntu-latest` and `ubuntu-24.04-arm`,
GitHub's hosted arm64 runner, which is free without limit on a public repository.
Nothing merges the two into a multi-arch manifest, because devpod never looks one
up: it asks for one exact tag and pulls the variant for the architecture it is
running on. An architecture whose tag is missing is not a failure, only the same
silent local build as any other miss.

Neither leg passes `--platform`, and that is the point of using a native runner
rather than emulation. The architecture in the hash is the one the driver reports,
which for docker is the `runtime.GOARCH` of the devpod binary doing the build — so
the runner decides it, exactly as the launching machine decides it on the lookup
side, where `devpod up` passes no platform at all. `--platform linux/arm64` would
hash to the same string on an arm64 runner, which is the argument against it: a
second source of truth for something already settled, and one that can disagree
without saying so.

The legs are `fail-fast: false`. Each publishes a tag nothing else reads, so a run
where one architecture fails leaves the other pulling, which is better than the
two local builds that cancelling the survivor would leave behind.

`latest` is amd64's, and arm64 publishes `latest-arm64`. devpod arch-qualifies
nothing, so two legs passing the same alias would race for it and the winner would
be whichever finished last; and the one thing that reads `latest` is
`build.cacheFrom`, a layer cache which serves nothing at all across architectures.
So `latest` keeps meaning what it has always meant, instead of becoming a cache
that works or does not per run for reasons nobody could see. What `dl` reads is
neither alias: it is the `devpod-<hash>` tag for its own architecture.

Making the arm64 leg possible at all needed `linux-aarch64` in the pixi workspace
(`platforms` in `pyproject.toml`). A pixi workspace refuses outright on a platform
it does not declare, so without it the leg fails at `pixi install` before it can
build anything — and so does an arm64 container's own `postCreateCommand`, which
would have made an arm64 prebuild an image that pulls fast and then cannot come
up. It says nothing about the release: the wheel and the conda package are still
linux-64.

The arm64 leg went in unexercised, since this repository is developed on x86: every
piece the image needs was checked to exist for linux-aarch64 — the multi-arch base,
the aarch64 pixi and `claude-shim` builds, devpod itself — and none of it was
checked to build. If it turns out not to, the symptom on an arm64 host is the local
build that host was already doing.

`postCreateCommand` is not in the image and cannot be: `pixi install` and the
provider registration run at container create, after the image exists. The
`<workspace>-pixi` volume is what makes them cheap the second time.

That install is `pixi install --frozen`, which is the same resolution CI uses
(`frozen: true` in `ci.yml`) and is load-bearing for two reasons beyond matching
it. A bare `pixi install` treats a lock it cannot read as a missing one: it warns,
exits 0, solves a fresh environment, and rewrites the tracked `pixi.lock` on its
way past — so the container that is supposed to reproduce the committed
environment quietly stops being it, and says nothing. And the solve it does
instead reaches the network: resolving pypi dependencies alongside conda ones
needs a conda-pypi name mapping that pixi fetches remotely (the compressed
mapping is served out of `prefix-dev/parselmouth` on raw.githubusercontent.com),
and a create whose fetch fails dies in `postCreateCommand` with the workspace
never opening.

`--frozen` installs the committed resolution, so there is no solve and no mapping
to fetch. That is measured rather than reasoned: with the mapping cache deleted,
`pixi install --frozen --offline` installs the default environment and never
recreates the cache, while a solving install recreates it on the spot. And a lock
the container's pixi cannot read now fails immediately and says which pixi it
would need, instead of succeeding into the wrong environment.

#### What the prebuild tag does not promise

The tag is a hash of the build *recipe* — the config and the context — and not of
the image the recipe produces. Three of the recipe's inputs float, so the same
`.devcontainer/` tree published on two different days is two different images:
the base `mcr.microsoft.com/devcontainers/base:ubuntu-24.04` is a moving tag,
`ghcr.io/devcontainers/features/docker-in-docker:2` is a moving major, and the
local feature's `pixi global install … claude-shim` names no version. (The
Dockerfile's `ARG PIXI_VERSION` is the one that is pinned.) A prebuild pulled
today and a local build of the identical tree tomorrow can therefore differ, and
the direction they differ in is forwards: a republish picks up whatever those
three point at now.

That is written down rather than fixed, and the shim is the case that shows why.
**`claude-shim` is deliberately unversioned**, and pinning it was considered and
rejected on four grounds:

- **The package holds no `claude`.** It is 21 KB of bash — `claude`, `cld`,
  `cldr` — whose first run downloads the current stable binary from Anthropic's
  GCS bucket into `~/.claude/cache`, and which re-checks stable hourly after
  that. The image cannot serve a stale `claude`, because it contains none; a pin
  would freeze the fetcher and nothing it fetches.
- **On the launch path this container is opened by, the baked shim never runs.**
  `dl`'s probe reads a shim-provided `claude` as *lendable* and the lend puts the
  host's real binary in front of it on the PATH — "What to bake so a launch does
  no work at all", above. The shim is baked so that `command -v claude` answers
  at all.
- **A pin freezes it harder than no pin does.** Unversioned, the shim is
  refreshed by every republish, which is every commit to `.devcontainer/**`.
  Pinned, it stops at whatever was current the day the pin was written.
- **One package would then have two policies.** The shipping provisioner
  (`rust/devlaunch-core/src/flows/provision.rs`) installs the same package,
  unversioned, into every workspace `dl` opens — the copy that reaches users
  rather than us. `test/unit/test_devcontainer_manifest.py` asserts that spec and
  the feature installer's still match, so pinning one side alone fails rather
  than drifting, while pinning both deliberately passes.

Measured 2026-08-20: the published prebuild carries `claude-shim 0.7.0`, the
newest of the channel's 14 releases and unmoved since 2026-04-06, so the drift a
pin would have prevented is currently zero. A publish is also the gate on a
broken shim release, not a channel for it: the feature installer checks the
trampoline exists and returns non-zero if it does not, which fails
`devpod build` and leaves the previous prebuild as the newest published tag.

If image contents ever do need pinning, the base image tag is the input that
carries the packages; pinning the shim alone would be precision about the
smallest of the three.

### Disk cost of the dev container

Opening a devcontainer for a branch costs about **3.3 GB on the host before you do
anything in it**: ~600 MB of image layers unique to this image, a ~680 MB container
writable layer, and a ~2.0 GB `<workspace>-pixi` volume.

That volume was ~520 MB until the `rust` feature went into the `default`
environment — measured either side of that change, the pixi environment is 521 MB
without it and 2044 MB with it, and only about a quarter of the difference is the
compiler itself: conda's `rust` links with a gcc toolchain, a sysroot and binutils.
Adding `cargo-llvm-cov` and `llvm-tools` to that feature (#294) took it to
**2348 MB**, measured either side on the same machine — 301 MB, nearly all of it
`libllvm22`, for `pixi run coverage-rust` working in here rather than only on a
runner. The same argument as below applies to it and the numbers are an order of
magnitude smaller.

**That 1.5 GB per branch is bought deliberately, and what it buys is isolation.**
The toolchain that builds the crates is pinned in the project environment beside
the pinned `devpod`, so a workspace compiles what it is editing with no host
toolchain, no `rustup`, and no global install anywhere in the picture — and it is
the same version on every machine and in CI. Paying for that once per branch
workspace is the trade; a leaner environment that sent whoever is working in the
container back to a host `cargo` would be the wrong end of it.

The container carries its own Docker daemon, and that daemon's `/var/lib/docker`
lives on a second named volume. One `pixi run test-e2e` plus a couple of nested
workspaces puts **~2.3 GB** in there, and nothing garbage-collects it — the inner
daemon reports ~45% of its images reclaimable with no reclaimer. Nested daemons
share no layers with the host or with each other, so this is paid once per branch.

**Budget ~5.5 GB per branch you are actively developing and e2e-testing — about 17 GB
for three concurrent branches.**

The time cost is cold pulls in a fresh nested daemon: the first `devpod up` inside a
new container takes ~25s, ~16s of which is pulling a base image the host already has.
Workspaces after that reuse it and take ~8s.

**Both volumes now go when the workspace does** — see [what a delete takes with
it](#what-a-delete-takes-with-it). They did not always: `devpod delete` removes
the container with `docker rm` and never touches a volume, and Docker never
garbage-collects a *named* volume, so `<workspace>-pixi` and
`dind-var-lib-docker-*` used to outlive every workspace that made them (39 of
them, 37.28 GB, on the machine devlaunch#324 was measured on).

Volumes left by workspaces deleted before that fix are still there, and so are
any whose removal Docker refused. To see what has piled up:

```bash
docker system df -v      # under Local Volumes, LINKS 0 means no container uses it
```

Cross-check a name against `devpod list` before removing it with `docker volume rm`:
a volume belonging to a live workspace shows `LINKS 1`.
