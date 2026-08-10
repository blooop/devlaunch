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
dl                               # Interactive workspace selector (fzf)
dl <user/repo>                   # Start workspace and attach shell
dl <user/repo> <cmd>             # Run workspace command (stop, code, etc.)
dl <user/repo> -- <command>      # Run shell command in workspace
```

### Commands that need a terminal

`dl <ws> -- <command>` gives the command a terminal whenever `dl` itself has one,
so interactive programs — a coding agent, `htop`, `git rebase -i`, a REPL — start
and stay up instead of exiting immediately. Redirect the output and the terminal
goes away again, so `dl <ws> -- ls > files.txt` stays free of escape sequences.

This needs the ssh host alias `devpod up` writes to `~/.ssh/config`. If a
workspace has none, `dl` says so and falls back to the plain `devpod ssh`
transport, which has no terminal; `dl <ws> restart` republishes the alias. Set
`DEVLAUNCH_NO_TTY=1` to force the fallback everywhere.

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

## Workspace IDs

`dl user/repo@branch` derives one id that names both the devpod workspace (what you
see in `dl --ls`) and the clone directory under `~/.cache/devlaunch/repos/`:

```
<repo-slug>-<branch-slug>-<syllables>      at most 38 characters

blooop/devlaunch@main                             -> devlaunch-main-zovomobo
blooop/devlaunch@feature/auth                     -> devlaunch-feature-auth-poliseno
blooop/devlaunch@feature-auth                     -> devlaunch-feature-auth-nesatabe
blooop/test_renv@nb4                              -> test-renv-nb4-polenita
kinisi-robotics/kinisi_ros@ags-devcontainer-tooling-support
                                                  -> kinisi-ros-ags-devcontainer-t-lenevere
blooop/devlaunch@dependabot/github_actions/codecov/codecov-action-6
                                                  -> devlaunch-dependabot-codecov-sifivasa
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

The id is also the container hostname, so it stays well inside the 38-character budget
to leave room for tools that add their own prefixes.

Branch names must be safe as both git refs and directory names — a name with a space or
a leading dash is rejected rather than quietly rewritten.

### Upgrading from an older devlaunch

This id format is new, and the directories and containers on your machine were named by
the previous scheme. The first `dl user/repo…` command after upgrading migrates the cache
once and prints what it did. `dl --help`, `dl --version`, `dl --ls` and opening an existing
workspace by name do not trigger it.

**Your clone directories are renamed.** What was
`~/.cache/devlaunch/repos/blooop/devlaunch/main` becomes
`~/.cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-zovomobo`. A workspace is a git
clone whose `origin` points at the `.bare` cache next to it, and `.bare` does not move, so
this is a plain rename: branches, history and **uncommitted changes all survive** — only
the folder name changes. `metadata.json` is updated in the same pass, so nothing is left
pointing at the old name.

**Your existing devpod containers keep their old ids and are orphaned.** The next
`dl user/repo@branch` builds a fresh container under the new id. dl does not delete
containers for you — deleting by id is how a running sidecar got destroyed the last time
something tried ([kinisi_ros#9766](https://github.com/kinisi-robotics/kinisi_ros/pull/9766)) —
so it prints a one-line notice with the count and writes the old ids to
`~/.cache/devlaunch/orphaned-workspaces.txt`. Remove them when you are ready:

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
than the filesystem has actually done.

## Workspace Commands

| Command | Description |
|---------|-------------|
| `dl <user/repo> up` | Start (or create) the workspace without attaching — for prewarming a container before a session wants it |
| `dl <user/repo> stop` | Stop the workspace |
| `dl <user/repo> rm, prune` | Delete the workspace |
| `dl <user/repo> code` | Open in VS Code |
| `dl <user/repo> restart` | Stop and start (no rebuild) |
| `dl <user/repo> recreate` | Recreate container |
| `dl <user/repo> reset` | Clean slate (remove all, recreate) |
| `dl <user/repo> -- <command>` | Run shell command in workspace (with a terminal, when `dl` has one) |

## Options

| Option | Description |
|--------|-------------|
| `--devcontainer <variant\|path>` | Use a non-default `devcontainer.json`. A bare name means `.devcontainer/<name>/devcontainer.json`. Stored with the workspace, so pass it once. |
| `DEVLAUNCH_NO_TTY=1` | Never give a workspace command a terminal; always use the plain `devpod ssh` transport. |

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

They are installed with `pixi global` on `devpod up`, and put on the PATH of a
login shell through whichever of `~/.bash_profile`, `~/.bash_login` or `~/.profile`
bash actually reads — it sources only the first of those that exists, so an image
shipping a `~/.bash_profile` never reads `~/.profile`. A workspace that already has both is left alone —
the check runs first, so the cost after the first launch is one round-trip and no
network. If `pixi` is missing from the image, `dl` installs that too.

An install that fails costs the workspace its tools, not its launch: `dl` logs a
warning and hands you the session anyway.

```bash
DEVLAUNCH_NO_TOOLS=1 dl someone/repo
```

| Variable | Description |
|----------|-------------|
| `DEVLAUNCH_NO_TOOLS=1` | Do not install `gh` or `claude` into workspaces |

Attaching to a workspace that is *already running* skips `devpod up`, and so skips
this too. A workspace started by something other than `dl` — or created before this
existed — picks the tools up on its next `dl <workspace> restart`.

## Global Commands

| Command | Description |
|---------|-------------|
| `dl --ls` | List all workspaces |
| `dl --ls --json` | The same list as JSON, with each workspace's repo, branch, state and [unsaved work](#cleaning-up-workspaces) — for tools that decide what to clean up |
| `dl --ls --size` | Add [what deleting each workspace would free](#how-much-disk-a-workspace-costs). Opt-in: it walks every file in the clone |
| `dl --install` | Install shell completions |
| `dl --purge [-y]` | Remove all devlaunch data — [the workspaces devlaunch created](#what-purge-deletes), and its caches |
| `dl --prune-worktrees [days]` | Remove unused worktrees (default: 30 days) |
| `dl --refresh` | Refresh completion cache |
| `dl --help, -h` | Show this help |
| `dl --version` | Show version (an editable install also names the tree it runs from) |

A released install prints the version and nothing else. An install made in
editable mode says so and names the checkout it resolves to, so two builds of
the same version are told apart at a glance:

```bash
$ dl --version
dl 0.0.9

$ dl-next --version          # editable install of a working tree
dl 0.0.9 (dev, editable from /path/to/your/devlaunch)
```

`aid --version` reports the same thing under its own name. The provenance comes
from the installed package's own PEP 610 metadata; an install that records none
just prints the bare version.

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
  - /home/you/.cache/devlaunch/ (workspace clones, repo caches, completions)

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
```

Exit status is `1`, because a clone you were told would go is still on disk. It
used to be `1` with the *whole* cache still standing: the first refusal stopped
the purge, so the completion caches, `metadata.json` and every other clone
survived on account of one directory.

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
own does — and as a bare count that is indistinguishable from an hour of unsaved
work. A cleanup tool believing the count would then never clean anything. Named,
it is judgeable. A workspace `dl` did not create reports `devlaunch: false` and
no `unsaved` — `unsaved` is `null` exactly where `devlaunch` is `false`, and
nowhere else: there is no clone of `dl`'s to protect, and it has no business
inspecting your checkout. (`repo` and `branch` are a weaker test and not the
same set: they come from `dl`'s metadata record, and a clone `dl` owns can have
lost its record while the clone and the work in it are still on disk. That clone
is inspected and reported like any other.)

**`dl <workspace> rm` refuses when the clone holds unsaved work — and when it
cannot tell**, so a caller that forgets to read the field is still caught:

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
store and does not manage volumes. `docker system df` is the tool that knows.

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
`pytest -m e2e test/e2e/test_interactive_session.py`, say — has nothing to
answer for and says so instead.

`DEVLAUNCH_E2E_WORKSPACE=<id>` opts in to the interactive-session tests, which
attach to a workspace you already have running rather than building one.

The nested daemon is also why the devcontainer does not join the host's network
namespace: a nested daemon needs a namespace of its own, or it co-manages the
host's `docker0` bridge and writes its NAT rules into the host's netfilter
tables.

### Disk cost of the dev container

Opening a devcontainer for a branch costs about **2 GB on the host before you do
anything in it**: ~600 MB of image layers unique to this image, a ~680 MB container
writable layer, and a ~520 MB `<workspace>-pixi` volume.

The container carries its own Docker daemon, and that daemon's `/var/lib/docker`
lives on a second named volume. One `pixi run test-e2e` plus a couple of nested
workspaces puts **~2.3 GB** in there, and nothing garbage-collects it — the inner
daemon reports ~45% of its images reclaimable with no reclaimer. Nested daemons
share no layers with the host or with each other, so this is paid once per branch.

**Budget ~4 GB per branch you are actively developing and e2e-testing — about 12 GB
for three concurrent branches.**

The time cost is cold pulls in a fresh nested daemon: the first `devpod up` inside a
new container takes ~25s, ~16s of which is pulling a base image the host already has.
Workspaces after that reuse it and take ~8s.

**These volumes are not reclaimed automatically.** `devpod delete` removes the
container with `docker rm` and never touches volumes, and Docker never
garbage-collects a *named* volume — so `<workspace>-pixi` and
`dind-var-lib-docker-*` outlive the workspace that created them. To see what has
piled up:

```bash
docker system df -v      # under Local Volumes, LINKS 0 means no container uses it
```

Cross-check a name against `devpod list` before removing it with `docker volume rm`:
a volume belonging to a live workspace shows `LINKS 1`.
