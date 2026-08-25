# What every workspace gets, and how

`dl` launches arbitrary repos, so nothing here can depend on the image. This
page is how each guarantee is actually delivered: the GitHub login, `gh` and
`claude`, zellij, the terminal title, the shared pixi cache, and the dotfiles
refresh.

## GitHub authentication

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
than failing. The warning names the config directory `gh` consulted, because the
usual cause is a shell that scoped `XDG_CONFIG_HOME` somewhere `gh` has no login,
not a host that is actually logged out.

### Who gets the token

Everything running in the container does, including a `postCreateCommand` from a
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
stays in place, including one it was given before you set
`DEVLAUNCH_NO_GH_TOKEN`. Run `dl <workspace> restart` to replace it.

## Tools in every workspace

`gh` and `claude` are available in every workspace `dl` opens, in every kind of
session: an interactive `dl <workspace>`, a one-shot `dl <workspace> -- <command>`,
and `aid`. The repo's `devcontainer.json` does not have to provide them, and most
do not. `dl` launches arbitrary repos, so a guarantee that depended on the image
would not be a guarantee.

### How they get there

On `devpod up`, at most three round trips, each one earning the next.

**1. The setup pass, the only trip a ready workspace ever pays.** One trip
carries everything the host wants done on the way into a running container: the
stages first, then the probe. Three stages exist today. Naming the container,
which is the hostname your shell prompt shows, always runs; zellij's config
and the terminal title each run only when its switch is on. They cost
nothing extra because the probe was paying for the trip anyway. Each stage reports `ok`, `failed` with its
exit status, or *not reached*; one that fails stops neither the stages behind it
nor the probe, and `dl` says which one it was.

The probe is the tail of that trip. The container reports
what only it can know: whether both tools answer at all, where its `claude`
resolves to, and where `~/.local/share/claude/versions` in its own home resolves
to. It reports those and names no verdict; the host reads them, so "a real
`claude`" is defined in exactly one place. The reading is one of three:

- **provisioned**, when `gh` answers on the login PATH and `claude` resolves to a
  binary the official installer put in the versions directory. Nothing else
  happens.
- **lendable**, when both names answer but that `claude` is a shim or a wrapper.
- **absent**, when a tool is genuinely missing.

**2. A lend, for *lendable* and *absent*.** `dl` streams its own `gh` and `claude`
into the container as a tar over the `devpod ssh` channel it already holds: a
local pipe, no network and no download. Nothing lands outside a staging directory
until both binaries have been run there once, so a container that cannot execute
them (a different libc, a different architecture) is left exactly as it was.

**3. The network install, for *absent* only.** When the host had nothing to lend,
or the lend was refused, `pixi global` installs both tools, and `pixi` itself
first if the image has none. A *lendable* container never reaches this trip: it
stops after the lend, or after the probe itself when the host had nothing to lend.
A `claude` already answers there, and this install decides what to do
with the same `command -v` that a shim satisfies, so the trip would install
nothing.

Tools reach the PATH of a login shell through whichever of `~/.bash_profile`,
`~/.bash_login` or `~/.profile` bash actually reads. It sources only the first of
those that exists, so an image shipping a `~/.bash_profile` never reads
`~/.profile`.

An install that fails costs the workspace its tools, not its launch: `dl` logs a
warning and hands you the session anyway.

### The trip a launch can skip

Trip 1 is cheap but it is not free: about 1.7 seconds, almost all of it connection
and process setup rather than the script it carries. A workspace that has had both
tools in it for a week pays that on every `dl <workspace> up` to be told the same
thing it was told last time. So when the answer was *provisioned*, `dl` writes it
down and reuses it.

The marker is one small JSON file per workspace under
`${XDG_CACHE_HOME:-~/.cache}/devlaunch/tool-verdicts/`, holding the verdict and the
modification time of devpod's own `workspace_result.json` for that workspace. devpod
rewrites that file on the way out of every **completed** `up`, whoever ran it:
`dl`, VS Code, a hand-typed `devpod up`, a `--recreate`. So a container that has
been rebuilt has a result file whose mtime no longer matches, and the marker stops
being believed. Anything else unexpected also stops it being believed: no marker, an
unreadable one, a workspace `dl` cannot find one result file for. Every one of
those falls back to making the trip, which is exactly what happened before the
marker existed.

**Only a launch that finds the workspace already running can skip it**, and that is
a smaller claim than it sounds. The container's hostname lives in a namespace docker
rebuilds from the container's config on every start, so a `devpod up` (creating a
container or starting a stopped one) loses the name and the pass has to run again
to set it. The two paths that skip are `dl <workspace> up` against a container that
is already up, which is the pre-warm and where the 1.7s is paid most often, and a
launch that waited on a sibling which had already brought the workspace up. Nothing
is ever skipped after this launch's own `devpod up`.

Nothing has to be cleaned up, and there is nothing to invalidate by hand: the
markers are compared, never trusted on age, and deleting the whole directory costs
one round trip on each workspace's next launch.

### What to bake so a launch does no work at all

To make every `dl` launch of an image stop at trip 1. The probe asks a **login**
shell to resolve each name, so every bullet here is about what a login shell can
find:

- **`gh`** anywhere on the login PATH.
- **`claude`** in the layout its official installer creates: the binary at
  `~/.local/share/claude/versions/<version>`, a **direct child** of that
  directory named for the version, with `~/.local/bin/claude` symlinked to it.
  Nested any deeper, as in `versions/<version>/bin/claude`, the shape a downloader
  parked there would take, is read as somebody else's tree that merely starts
  with the official path, and does not count.
- **`~/.local/bin` on the login PATH**. The symlink above is how `claude`
  answers at all; a login shell that cannot find that directory reads the image
  as *absent* however carefully the rest was baked, and it pays the full lend.
  Ubuntu's stock `~/.profile` prepends `~/.local/bin` itself, but an image
  shipping a `~/.bash_profile` never reads `~/.profile` (above), and then
  nothing does.

Nothing else counts as a `claude`, and that is the point. A *shim*, a small
launcher that downloads the real binary the first time it is called, answers
`command -v claude` exactly as the real thing does, while the workspace still
owes a multi-hundred-megabyte download at the least convenient moment. So `dl`
resolves the name rather than running it (running a shim *is* the download), reads
a shim as *lendable*, and sends the host's real binary. The lend prepends
`~/.local/bin` to the login PATH, which is what puts the lent binary in front of
the shim from then on. That is intended, and the reason the next launch probes
*provisioned* and the transfer is paid once rather than forever.

**This repo's own devcontainer feature bakes a shim.**
`.devcontainer/claude-code/install.sh` installs `claude-shim`, so an image built
from it does *not* meet the contract by itself: its first `dl` launch is lent a
real `claude`, and only launches after that do nothing. Build the official layout
into the image if you want the first launch free too.

### What this deliberately does not do

- **No per-tool transfer.** The lend is all-or-nothing, so an image with a real `gh`
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
| `DEVLAUNCH_NO_TOOLS=1` | Do not install `gh` or `claude` into workspaces. The setup pass still runs, one trip per `up`, which still names the container; only the installing is skipped |

`DEVLAUNCH_NO_ZELLIJ=1` is the narrower one: it drops the zellij stage and leaves
`gh` and `claude` provisioning alone. See
[What it costs](#what-it-costs).

Attaching to a workspace that is *already running* skips `devpod up`, and so skips
this too. A workspace started by something other than `dl`, or created before this
existed, picks the tools up on its next `dl <workspace> restart`.

## A terminal beside the agent

Every workspace `dl` opens also has [zellij](https://zellij.dev) on `PATH`, which
buys one thing the other tools do not: an agent running in a container can open a
**second terminal next to itself**, in the same container, and you can attach to it
from anywhere to watch or to type.

Nothing has to cooperate for this. It does not come from your dotfiles, it does not
need an edit to any repo's `devcontainer.json`, and it works in images `dl` has never
seen. That is the same argument the rest of "Tools in every workspace" makes, for the
same reason: `dl` launches arbitrary repos.

### Opening a pane from inside a session

From anywhere inside the container, including from a completely non-interactive
command with no terminal attached to anything:

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

With it off, no invocation changes meaning at all. That is what off means here, and
it is why the switch exists rather than the behaviour simply being on.

**This is not the switch that decides whether zellij gets installed.** That one is
`DEVLAUNCH_NO_ZELLIJ`, under [What it costs](#what-it-costs) below, and the two are
orthogonal on purpose: this one starts a session, that one puts the binary there.
With `DEVLAUNCH_NO_ZELLIJ=1` set and no zellij in the container, `DEVLAUNCH_ZELLIJ=1`
is simply a session setup that fails and a command that runs anyway, which is what
it already does in any container that ended up without zellij for its own reasons.

**The command runs beside the session, not inside a pane of it.** That is deliberate.
Putting the command in a pane would hand its stdin, stdout and exit status to zellij,
and all three are things `dl` promises to leave alone: `dl <ws> -- cmd > file` has to
put the command's own output in the file, and a failing command has to come back with
its own status. Since `zellij -s <name> action new-pane` works perfectly well from a
command that is in no session at all, running beside the session costs nothing and
delivers the same pane.

**The interactive session of a bare `dl <workspace>` is untouched, switched on or
off.** An interactive attach sends no command for the wrap to attach to, which is
exactly what gets it a terminal from devpod, and giving it one would cost either the
terminal or a round trip in front of every shell. You land in an ordinary login shell
with `zellij` on `PATH`, so `zellij attach -c devlaunch` gets you the session, and any
panes an agent has opened in it, whenever you want them.

There is one exception, and it is a pleasant one: if you also run with
`DEVLAUNCH_DOTFILES_ON_ATTACH=1`, that refresh is a command, so it gets wrapped like
any other and the session is already there when the shell arrives.

### Existing workspaces

zellij arrives on the setup pass, which runs on every `devpod up`. So a workspace
that predates this picks it up on its next **`dl <workspace> restart`**. A full
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

`DEVLAUNCH_NO_TOOLS=1` turns this off along with the rest of tool provisioning.
Installing zellij is tool provisioning, where naming a container is not.

`DEVLAUNCH_NO_ZELLIJ=1` turns off **only** this:

| Variable | Description |
|----------|-------------|
| `DEVLAUNCH_NO_ZELLIJ=1` | Do not install `zellij` into workspaces. The setup pass still runs and still names the container, and `gh` and `claude` are still provisioned exactly as they were |

It exists because the two questions are different ones. A host whose containers get
zellij another way, through their own dotfiles, a base image or a devcontainer feature,
or which wants none in there at all, is asking for one stage to stop running.
`DEVLAUNCH_NO_TOOLS=1` would do that and also surrender the `gh` and `claude`
guarantee the rest of this page is about, which is a large price for one
`command -v`.

Both variables read the same values: anything but empty, `0`, `false` or `no` means
yes, turn it off. And neither touches the hostname stage. A host that wants no
zellij has not thereby asked for unnamed containers.

## Naming the terminal after the workspace

Every launch names the terminal after the workspace it is opening, just before the
session takes over:

```
ESC ] 2 ; devlaunch-main-3j1t BEL
```

That is one escape sequence to whichever stream dl was given, and the point of
doing it that way is that dl does not have to know what is reading it. zellij and
tmux both take OSC 2 as the focused pane's title, and a bare terminal takes it as
the window title. So `dl` names the pane in zellij, in byobu-on-tmux, and in a
plain kitty or xterm window, with one write and no detection.

It is on unless you turn it off:

| Variable | Description |
|----------|-------------|
| `DEVLAUNCH_NO_TITLE=1` | Do not name the terminal: neither the escape below nor the profile edit under [What keeps it named](#what-keeps-it-named). Everything else about the launch is unchanged |

A "no" variable, where `DEVLAUNCH_ZELLIJ` is an opt-in one, because the two are not
the same size of decision. That one installs a session into a container; this one
writes an escape sequence and one line into a profile.

**It is the [workspace id](workspaces.md#workspace-ids), however you named the
workspace.** `dl blooop/devlaunch` names the pane `devlaunch-main-3j1t`: the string
devpod is addressed by, the container's hostname, and the `WORKSPACE` column of
`dl --ls`. One workspace, one name, so a tab and a listing row match by eye.

It used to be the spec you typed, resolved, `blooop/devlaunch@main`, and read on its
own that is the better name. An id carries no owner at all, so a fork and its
upstream are two tabs spelled the same, and it spells the branch as a slug, so
`feature/auth` reads as `feature-auth`, the name of a different branch the same
repository could have. What the spec left out is that only one of the four ways of
naming a workspace has one. A bare `dl myworkspace` *is* its id, and `dl ./some/dir`
or a plain URL never had a branch for an `@` to precede, so three of the four were
titled by id anyway and the shape of the tab depended on how you had reached the
workspace.

It stays short enough for a tab bar, and that now comes with the choice rather than
needing an argument of its own: an id is at most 47 characters, because devpod
refuses to create or report a workspace whose name runs past 48. A spec had no such
bound. A triple is checked for the characters it holds and not for its length, so a
200-character branch made a 200-character tab.

**Written to stderr, and only when stderr is a terminal.** stdout belongs to the
completion machinery and to `wf`, which parse it. The tty check is on stderr for
the same reason: `dl <ws> -- make test > log` has redirected stdout and still has a
terminal worth naming, while a run whose stderr is a pipe would only be writing
escapes into somebody else's capture.

### What keeps it named

A terminal title has exactly one value and the last writer sets it. An interactive
shell overwrites dl's within a second of arriving. Ubuntu's stock `~/.bashrc` puts
`\e]0;\u@\h: \w\a` at the *front* of `PS1`, so every prompt renames the pane after
the container's hostname and the working directory, which is more than the tab
wants and in a different shape.

So the setup pass appends one line to the profile a login shell reads:

```
case $- in *i*) [ -n "$BASH_VERSION" ] && PS1="$PS1\[\e]2;"devlaunch-main-3j1t"\a\]" ;; esac
```

Appended, and that is the whole mechanism: two escapes in one prompt are applied in
order, so the last one sets the title. Nothing is rewritten. The visible
`vscode@devlaunch-main-3j1t:~/repo$` still says the hostname, and only the tab
changes. (A `PROMPT_COMMAND` cannot do this job: bash runs that *before* it prints
`PS1`, so the stock escape would land afterwards and win.) Interactive bash only.
`bash -lc` reads the same profile on every `dl <ws> -- cmd` one-shot, and `\[`, `\e`
and `\a` mean nothing to dash, which is `/bin/sh` and which reads `~/.profile` too,
so an unguarded line would print the escape at every prompt instead of acting on
it.

It is written once, since the line carries a content-hash comment the next launch
recognises, and it rides the same round trip as the hostname stage, so it costs no
extra trip.

**The name is the workspace id, and it is the same one everywhere.** The hostname is
that id, `dl --ls` prints that id, and devpod is addressed by it, so a tab and a
listing row match by eye. It also has to be one string for a second reason: the line
is recognised by a hash of its own text, so a second, different name for one
workspace would not replace the first but sit after it, and the last one wins. The
id is the one name every launch arm has, so a workspace gets at most one such line,
ever. This used to install `owner/repo@main` instead, which only a launch that
resolved a triple had, so `dl myworkspace` installed nothing and opening the same
workspace both ways left two lines.

**It is installed when a workspace enters Running, not on every attach.** A
workspace that is already up keeps whatever its profile was given, so
`DEVLAUNCH_NO_TITLE=1 dl <ws>` silences dl's own escape and leaves the prompt's;
`dl <ws> recreate` is what re-decides it. That is the same bargain the hostname
stage makes, and for the same reason: the alternative is a round trip per attach.

### The one other writer worth knowing

**claude** writes the title continuously from its own read of what the session is
doing, which would leave a `dl <ws> -- claude` pane named after the task rather than
the workspace within a second. `aid` therefore starts claude with
`CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1`, so the workspace name is what stands. What
claude is doing is on screen inside the pane; which workspace the pane *is* is not
otherwise anywhere.

This is `aid`'s doing and not `dl`'s: a `dl <ws> -- claude ...` you typed yourself
is your command, and dl does not rewrite it. Set the variable yourself if you want
the same result from the long form.

Two multiplexer limits are worth stating, because neither is dl's to fix:

- **In tmux the *window* name needs `allow-rename on`** (off by default in recent
  tmux), and the outer terminal title needs `set-titles on`. The pane title always
  takes it. Both are your tmux config.
- **GNU screen**, byobu's other backend, names windows with `ESC k <name> ESC \`
  and ignores OSC 2. Emitting both sequences would put stray text in any terminal
  that groks neither, so screen is out of scope rather than half-served.

**zellij tab names are not this.** A zellij *tab* is renamed only by `zellij
action rename-tab` or a plugin; no escape sequence reaches it, which is why this
names the pane instead. The window title zellij then publishes to the outer
terminal is `<session> | <pane title>`, so the workspace id is what shows up in a
kitty tab bar.

## The shared pixi package cache

Every container `dl` creates gets one host directory bound into it, and
`PIXI_CACHE_DIR` pointed at it, so that dotfiles which provision their tools with
`pixi global sync` download each package once per machine instead of once per
container:

| | |
|---|---|
| **On the host** | `$XDG_CACHE_HOME`, or `~/.cache`, then `devlaunch/pixi` |
| **In the container** | `/var/tmp/devlaunch-pixi` |

Measured on the profile this was built for, 23 pixi-global environments: a
container with a cold cache spends 62s to 113s and downloads 1.2 GB; one that finds
the packages already there finishes in 18s to 28s and fetches nothing. Two containers
syncing against it at the same time is fine, since the downloads are content-addressed
and rattler takes a lock per package.

**Deleting it is always safe, at any moment, including while containers are
running.** It holds nothing but downloaded package archives, every one of them
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
And it is **not a shared `PIXI_HOME`**. The installed environments and their
trampolines are baked with absolute paths, and two containers sharing one
environment tree is [pixi#5476](https://github.com/prefix-dev/pixi/issues/5476).
Only the download cache is shared, which is the part that is safe to share.

If the directory cannot be created, or is not there when the launch reaches it,
because of a full disk, a read-only cache home, or a cache swept between the two,
the launch goes ahead without the mount and the container downloads its own packages,
exactly as it did before this existed.

**Sharing requires the container's user to be able to write the directory**,
which in practice means its uid matches yours or it is root. The mount carries
host ownership through unchanged, and pixi does not degrade to reading a cache
it cannot write: pointing `PIXI_CACHE_DIR` at a directory owned by another uid
fails the install outright (`Permission denied` on the repodata, exit 1) even
when every package it wants is already in there. So an image whose remote user
is neither root nor your uid does not merely lose the sharing. Its `pixi global
sync` fails, and its tools do not get provisioned.

`dl` cannot see the container's uid before it launches, so it cannot decide this
for you. In practice the common case is safe: every mainstream base declares a
remote user at uid 1000, which is the first human user on a Linux host. If you
hit the failure, the fixes available to you are to run that image as your own
uid, or to take the cache out of play for it (`rm -rf ~/.cache/devlaunch/pixi`
recovers a directory an earlier container left owned by someone else).

The case that is not a developer's machine is CI. What makes the common case
safe is that uid 1000 is *both* the base image's remote user and the first human
user on a Linux host, and on a hosted runner it is only the first of those. A
GitHub runner's own user is somebody else, so a launch there hits this on its
first container and every container after it. This repo's own launch benchmark
did exactly that for twenty consecutive merges to `main`: `failed to create
directory /var/tmp/devlaunch-pixi/pkgs: Permission denied`, from the benched
repo's `pixi install`, before anything was timed.

Where you know the uid you are handing the directory to, there is a third fix
the list above does not offer, and it is what `.github/workflows/bench.yml` now
does. Create the directory yourself and widen it, before the first launch:

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
`pixi global install` is not only an install. It is an edit to
`$PIXI_HOME/manifests/pixi-global.toml`, a declarative file that in a container
already has an owner. Writing there made devlaunch a second author, and cost
something in both directions:

- `pixi global sync` removes every environment the manifest does not list, so a
  dotfiles apply that rewrites the manifest and syncs **uninstalls** the zellij
  devlaunch just installed, and the next launch reinstalls it, forever.
- The manifest is not always a file. A devcontainer is free to symlink
  `~/.pixi/manifests/pixi-global.toml` onto a tracked file inside the checkout,
  and one does; the append then landed in the work tree and every `git status`
  in the workspace came up dirty.

Neither is expressible against a home devlaunch created: nothing syncs that
manifest, and no repo state can sit under that path. It costs a duplicate
extracted prefix in the one case where a tool is installed but unreachable from
a login shell, disk only since `PIXI_HOME` does not move the download cache,
and that is the case where the old behaviour reinstalled on every launch anyway.

Not `~/.local/share/devlaunch/pixi`, which is the conventional path and the wrong
one here: containers bind-mount `~/.cache`, `~/.config` and `~/.local/share`
straight from the host, so a prefix tree under one of them would be shared by
every container on the machine and written into your own home. That is pixi#5476
again, the hazard the cache mount is careful to keep `PIXI_HOME` away from.

`PIXI_HOME` is set only for devlaunch's own install scripts, never exported into
the login profile, so **your own `pixi global install` in a workspace still goes
to your own `~/.pixi`**. Only the bin directory goes on `PATH`.

### Existing containers, and what a recreate is for

**A mount lands only when a container is created.** devpod re-applies
`--workspace-env` on every `up`, but it will not add a bind mount to a container
that already exists; passing `--mount` there is a silent no-op. So a container
built before this feature, or before a change to where the mount lands, keeps
whatever it was created with until `dl <workspace> recreate`, and only then
picks the current arrangement up.

In between, `PIXI_CACHE_DIR` points at `/var/tmp/devlaunch-pixi` with nothing
mounted on it. That is a working private cache, not a failure: `/var/tmp` is
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

### Refreshing dotfiles on attach

Under chezmoi, the refresh is `chezmoi update`. If that fails **and** the
chezmoi source directory is a git repository, it regenerates the workspace's
`chezmoi.toml` with `chezmoi init` and tries once more. That second attempt is
for one failure in particular: the dotfiles repo grew a template variable the
workspace's config predates, so every apply dies rendering a config that has no
entry for it. It is guarded on the source directory already being a repository
because `chezmoi init` with no repo argument would otherwise create an empty one,
which has no upstream and can never update again.

devpod applies dotfiles when it *provisions* a workspace, so a workspace that has
been up for a fortnight still has the dotfiles it was born with. `dl <ws>
dotfiles` fixes that when you think of it; `DEVLAUNCH_DOTFILES_ON_ATTACH=1` makes
`dl` think of it for you, running the same refresh just before it hands you the
shell.

```bash
DEVLAUNCH_DOTFILES_ON_ATTACH=1 dl someone/repo
```

It is off unless you set it, and that is the point rather than caution. The
refresh is a `devpod ssh` round-trip, measured at ~1.7s and almost all of it
connection setup, with a `git pull` behind it, and it would otherwise be charged
to every attach on every machine to close a gap most people do not have.

Two things it deliberately does not do:

- **It never runs for `dl <ws> -- <command>`.** A one-shot command renders no
  prompt and sources no interactive shell, so refreshing in front of it would
  buy that command nothing and cost it the round-trip. That path is the one
  agent launchers use, and it stays exactly as fast as it was.
- **It never holds the shell hostage.** The refresh gets 60 seconds; an
  unreachable dotfiles remote, or one that wants a password nobody is there to
  type, means a pause and then your shell, not a hang. Failure is a warning,
  and you get the workspace either way.

Refreshes run every time you attach, with no cooldown, because you asked for
them. If that is too often, unset the variable and use `dl <ws> dotfiles`.

## Shell completion

After running `dl --install`, tab completion offers:

- Workspace names from your devpod list
- Known GitHub owners and repositories from your workspaces
- File/directory paths when starting with `./`, `/`, or `~`
- All global flags (`--ls`, `--install`, etc.) and workspace commands

### How the completion cache stays current

The data behind completions lives in `~/.cache/devlaunch/completions.json`, and
building it means a `git ls-remote` per known repo, which is seconds of work. So it is
rebuilt in the background at most once an hour (the same interval the background
fetch sweep uses, see [How fresh a launch is](workspaces.md#how-fresh-a-launch-is)), and at
most once per `dl` invocation. Commands
that change your workspaces, by starting, stopping or deleting one, rebuild it as
soon as they finish, regardless of when it was last built. Commands with no use
for it, `dl --help` and `dl --version`, do not touch it at all.

A branch created on a remote in the last hour may therefore not be offered yet.
`dl --refresh` rebuilds the cache immediately and ignores the interval.
