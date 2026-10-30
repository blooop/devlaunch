# herdr

[herdr](https://herdr.dev) is a terminal multiplexer built for coding agents: tabs, panes, and
a server that knows which agent each pane is running. `dl` and `dla` work in any terminal, and
nothing on this page is needed to use them. In a herdr pane they do more, and this page lists
what, with a link to the section that has the detail.

Most of it needs nothing turned on. Four parts need a step from you: the pane shell, which
`dl --herdr-setup` installs once; the editor split, which also needs `NVIM_SPLIT=1`; the
per-workspace environments, which also need `dl --herdr-env`; and the in-container agent
report, which `DEVLAUNCH_HERDR=1` asks for.

## Setup

```bash
dl --herdr-setup
herdr server reload-config
```

`--herdr-setup` installs `~/.local/bin/dl-herdr-shell` and points herdr's
`[terminal] default_shell` at it, keeping the rest of the config file and its comments. If
chezmoi manages the config file, setup refuses the edit and names the source file to change
instead. `dl --install` installs the same script and prints the config line, for a config you
edit by hand. A `default_shell` that already names some other program is refused too, with
the line to change by hand, rather than replaced.

Without this step everything below still works except the pane shell, the editor split and
the per-workspace environments, which all go through `dl-herdr-shell`.
[workspace-tools.md](workspace-tools.md#opening-a-new-pane-in-the-workspace-its-tab-already-holds)
has why it is a script in `~/.local/bin` and not a flag.

## What a launch does in a herdr pane

| Feature | Needs | Detail |
|---|---|---|
| The tab is renamed after the workspace, `repo@branch`, or its id when `dl` holds no record for it | Nothing, unless `DEVLAUNCH_NO_TITLE=1` turns it off | [The herdr tab](workspace-tools.md#the-herdr-tab-which-is-renamed-and-not-written-to) |
| `HERDR_AGENT` names the agent `dla` or `dl <ws> -- claude` started | Nothing | [Telling a session manager](workspace-tools.md#telling-a-session-manager-which-agent-is-running) |
| A herdr restart or a reboot brings each `dla` session back, except a `--rm` one | herdr 0.9.2 or later, and `resume_agents_on_restore = true` in herdr's config | [Coming back after herdr restarts](workspace-tools.md#coming-back-after-herdr-restarts) |
| `dl <ws> recreate` and `--refresh-stale` start the agents they end again | Nothing | [A recreate starts the agents it ends again](cli.md#a-recreate-starts-the-agents-it-ends-again) |
| A split opens in the container while a pane in the tab runs a `dl` session | `dl --herdr-setup` | [Opening a new pane](workspace-tools.md#opening-a-new-pane-in-the-workspace-its-tab-already-holds) |
| An editor opens beside the agent in a one-pane tab, and the agent keeps focus | `NVIM_SPLIT=1` and the pane shell | [Opening an editor beside an agent](workspace-tools.md#opening-an-editor-beside-an-agent) |
| Each herdr workspace has its own Claude login and host variables | `dl --herdr-env` and the pane shell | [Workspace environments](workspace-tools.md#workspace-environments-and-claude-logins) |
| A `claude` you start inside the container reports to the pane, through a Claude Code hook | `DEVLAUNCH_HERDR=1` | [Reporting an agent started inside the workspace](workspace-tools.md#reporting-an-agent-started-inside-the-workspace) |
| `dl rmt` deletes the workspace the tab is named after | The tab rename above | [Below](#dl-rmt-delete-the-workspace-this-tab-was-for) |

Every one of these is best-effort. A herdr that is too old, that does not answer, or that is
not there costs the feature and never the launch.

## The tab

A launch in a herdr pane renames its tab with `herdr tab rename`, using `HERDR_TAB_ID` from the
pane's environment. herdr keeps the name after the session ends, so the tab still says which
workspace it was for once the agent has exited. One thing reads that name back: `dl rmt`.
`DEVLAUNCH_NO_TITLE=1` turns the rename off with the rest of the terminal naming, and a tab
that was never renamed gives `dl rmt` no workspace to find.

## `dl rmt`: delete the workspace this tab was for

When a `dla` session ends, its tab still carries the workspace's name. Type `dl rmt` in that
tab and `dl` asks herdr for the name, prints the workspace it resolves to, deletes it and
hangs up the shell it was typed in, which closes the tab when that shell was the tab's last
pane. It is `rme` with the tab as its workspace, guard and `--force` included, and
it refuses rather than guess when the name matches no workspace or two.
[cli.md](cli.md#rmt-the-workspace-this-herdr-tab-is-named-after) has the whole contract: how
the name becomes an id, every refusal, and the hangup.

## The variables herdr and `dl` pass

| Variable | Direction | What it is for |
|---|---|---|
| `HERDR_TAB_ID` | herdr exports, `dl` reads | The tab to rename, to read back for `rmt`, and to scan for a split |
| `HERDR_BIN_PATH` | herdr exports, `dl` reads | The herdr to ask. A `PATH` lookup is the fallback for an older herdr |
| `HERDR_AGENT` | `dl` writes, herdr reads | Which agent the session runs |
| `DEVLAUNCH_HERDR` | You set it | Lend herdr into the container so an agent started there reports to the pane |
| `DEVLAUNCH_NO_TITLE` | You set it | Leave the tab, and the rest of the terminal, unnamed |
| `NVIM_SPLIT` | You set it | Open `$VISUAL`, then `$EDITOR`, then `nvim` beside an agent launch |
