# herdr

[herdr](https://herdr.dev) is a terminal multiplexer built for coding agents: tabs, panes, and
a server that knows which agent each pane is running. `dl` and `dla` work in any terminal, and
nothing on this page is needed to use them. In a herdr pane they do more, and this page lists
what, with a link to the section that has the detail.

Most of it needs nothing turned on. Two parts need a step from you: the pane shell, which
`dl --herdr-setup` installs once, and the in-container agent report, which `DEVLAUNCH_HERDR=1`
asks for.

## Setup

```bash
dl --herdr-setup
herdr server reload-config
```

`--herdr-setup` installs `~/.local/bin/dl-herdr-shell` and points herdr's
`[terminal] default_shell` at it, keeping the rest of the config file and its comments. If
chezmoi manages the config file, setup refuses the edit and names the source file to change
instead. `dl --install` installs the same script and prints the config line, for a config you
edit by hand.

Without this step everything below still works except the pane shell, the editor split and
the per-workspace environments, which all go through `dl-herdr-shell`.
[workspace-tools.md](workspace-tools.md#opening-a-new-pane-in-the-workspace-its-tab-already-holds)
has why it is a script in `~/.local/bin` and not a flag.

## What a launch does in a herdr pane

| Feature | Needs | Detail |
|---|---|---|
| The tab is renamed after the workspace, `repo@branch` | Nothing | [The herdr tab](workspace-tools.md#the-herdr-tab-which-is-renamed-and-not-written-to) |
| `HERDR_AGENT` names the agent `dla` or `dl <ws> -- claude` started | Nothing | [Telling a session manager](workspace-tools.md#telling-a-session-manager-which-agent-is-running) |
| A herdr restart or a reboot brings each `dla` session back | herdr 0.9.2 or later | [Coming back after herdr restarts](workspace-tools.md#coming-back-after-herdr-restarts) |
| `dl <ws> recreate` and `--refresh-stale` start the agents they end again | Nothing | [A recreate starts the agents it ends again](cli.md#a-recreate-starts-the-agents-it-ends-again) |
| A split in a devlaunch tab opens in that tab's container | `dl --herdr-setup` | [Opening a new pane](workspace-tools.md#opening-a-new-pane-in-the-workspace-its-tab-already-holds) |
| An editor opens beside the agent, and the agent keeps focus | `NVIM_SPLIT=1` and the pane shell | [Opening an editor beside an agent](workspace-tools.md#opening-an-editor-beside-an-agent) |
| Each herdr workspace has its own Claude login and host variables | `dl --herdr-env` and the pane shell | [Workspace environments](workspace-tools.md#workspace-environments-and-claude-logins) |
| An agent you start inside the container reports to the pane | `DEVLAUNCH_HERDR=1` | [Reporting an agent started inside the workspace](workspace-tools.md#reporting-an-agent-started-inside-the-workspace) |
| `dl rmt` deletes the workspace the tab is named after | Nothing | [Below](#dl-rmt-delete-the-workspace-this-tab-was-for) |

Every one of these is best-effort. A herdr that is too old, that does not answer, or that is
not there costs the feature and never the launch.

## The tab

A launch in a herdr pane renames its tab with `herdr tab rename`, using `HERDR_TAB_ID` from the
pane's environment. herdr keeps the name after the session ends, so the tab still says which
workspace it was for once the agent has exited. Two things read that name back: the pane shell,
which opens a new split in the same container, and `dl rmt`.

## `dl rmt`: delete the workspace this tab was for

The usual end of a `dla` session is a tab named `devlaunch@fix/42` holding a host shell, and a
workspace you are finished with. `dl rmt` deletes that workspace and then closes the tab:

```
$ dl rmt
This tab is named 'devlaunch@fix/42', which is workspace devlaunch-fix-42-3j1t.
Removing workspace devlaunch-fix-42-3j1t...
...
Hanging up the shell dl was called from (pid 4242).
```

It takes no workspace name and opens no selector. The tab name is the workspace. `dl` asks
herdr for the name (`herdr tab get $HERDR_TAB_ID`), and prints the workspace id it resolved to
before it deletes anything.

After that it is `rme`, the delete and then the hangup:

- The guard is `rm`'s. A clone with uncommitted or unpushed work is kept and named, and
  `dl rmt --force` is the way past it.
- A refusal leaves the shell open with the reason on screen.
- Once the workspace has gone, `dl` hangs up the shell it was typed in, so the tab closes.

It refuses, and deletes nothing, when:

- the shell is not in a herdr tab. It then asks herdr nothing.
- herdr does not answer.
- the tab name matches no workspace. A tab nobody renamed carries its number, which names
  none.
- the tab name matches two workspaces.

`rmt` takes no `--rm`, no command and no workspace name, and refuses a line that gives one.
[cli.md](cli.md#rmt-the-workspace-this-herdr-tab-is-named-after) has how the name becomes an
id, and [`rme`](cli.md#rme-the-delete-and-then-the-shell) has which process the hangup reaches
and when it is skipped.

## The variables herdr and `dl` pass

| Variable | Direction | What it is for |
|---|---|---|
| `HERDR_TAB_ID` | herdr exports, `dl` reads | The tab to rename, to read back for `rmt`, and to scan for a split |
| `HERDR_BIN_PATH` | herdr exports, `dl` reads | The herdr to ask. A `PATH` lookup is the fallback for an older herdr |
| `HERDR_AGENT` | `dl` writes, herdr reads | Which agent the session runs |
| `DEVLAUNCH_HERDR` | You set it | Lend herdr into the container so an agent started there reports to the pane |
| `NVIM_SPLIT` | You set it | Open `$VISUAL`, then `$EDITOR`, then `nvim` beside an agent launch |
