# Workspace ids, and how fresh a launch is

Two facts about a workspace that are worth knowing but that you never have to
type: the id `dl` derives for it, and how recent the branch you land on is.

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

The container hostname is this id **without the suffix**: `devlaunch-main` for the
workspace devpod addresses as `devlaunch-main-zovomobo`. The suffix is what makes the
id injective, and nothing addresses a container by the name in its UTS namespace, so
a prompt carries the half of the name that is read. That is 38 characters at most,
leaving ~26 of the 64-byte hostname limit for tools that stack their own prefixes onto
the container name.

47 is held by devpod instead: it is one character inside devpod's own hard ceiling of
48, and a 49-character id is refused outright rather than truncated.

The cost is that two workspaces differing only in their suffix now show one prompt,
whether that is one repo under two owners or `feature/auth` beside `feature-auth`. They
are still two workspaces, and the tab is what tells them apart.

The id is *not* what you read. A tab shows `owner/repo@branch` (see [Naming the
terminal after the workspace](workspace-tools.md#naming-the-terminal-after-the-workspace)) and the
selector shows `owner | repo | branch`. The id addresses the workspace; those name
it.

Branch names must be safe as both git refs and directory names, so a name with a space or
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
this is a plain rename: branches, history and **uncommitted changes all survive**, and only
the folder name changes. `metadata.json` is updated in the same pass, so nothing is left
pointing at the old name.

**Your existing devpod containers keep their old ids and are orphaned, and they can
often be repaired rather than replaced.** An orphaned container is sourced at the path this
migration just renamed, with the real clone sitting next to it under the new name, which is
precisely what [`dl --reconcile`](cleanup.md#reconciling-records-that-disagree) is for: it re-points
devpod's record at the renamed clone, and `dl <workspace> recreate` finishes the repair.
That gives you back the clone association and the workspace's identity, though not state that
lived only inside the old container, which nothing can bring back. The repair is
order-dependent: relaunching the branch claims the renamed clone for a fresh container,
and reconcile never re-points a clone a live container holds, so reconcile first, then
relaunch. Left alone, the next `dl user/repo@branch` simply builds a fresh container
under the new id, and deleting the old one is all that remains for it.

dl does not delete containers for you. Deleting by id is how a running sidecar got
destroyed the last time something tried ([kinisi_ros#9766](https://github.com/kinisi-robotics/kinisi_ros/pull/9766)),
so it writes the old ids to
`~/.cache/devlaunch/orphaned-workspaces.txt`. For the workspaces you are finished with,
the disposal command reads from that listing:

```bash
xargs -r -n1 devpod delete < ~/.cache/devlaunch/orphaned-workspaces.txt
```

**A clone directory with no metadata record is left alone.** Nothing records which branch
it was cloned for, and the old directory name cannot be turned back into one, since `feature/auth`
and `feature-auth` both became `feature-auth`, so a guessed name would be worse than no
rename. Those directories stay exactly where they are and are listed in
`~/.cache/devlaunch/unmigrated-clones.txt`.

Running dl again changes nothing: the migration is keyed on the `version` field in
`metadata.json`, not on directory names, so a branch that happens to look like a new-scheme
id is never mistaken for one. If a migration is interrupted, the next run finishes it. The
version is written last, in the same atomic save as the new paths, so it never claims more
than the filesystem has actually done. A rename the filesystem refuses, on a read-only mount
or under tightened permissions, is treated the same way: the version stays put and every
later run retries the refused directories and repeats the notice until the underlying
refusal is fixed by hand.

## How fresh a launch is

`dl` keeps one bare clone per repo at `~/.cache/devlaunch/repos/owner/repo/.bare/`
and cuts every workspace's checkout from it as a sibling directory named after the
workspace id. That is an ordinary clone whose git objects are hardlinks into that cache,
not a git worktree. [How much disk a workspace costs](cleanup.md#how-much-disk-a-workspace-costs)
is the accounting for that. What follows is the other half: how fresh the branch
you land on is, which is what decides whether the tip you just pushed is the tip
you get. A launch fetches only the one branch it is launching, so no launch waits
on a repo-wide refresh.

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

### Preparing a workspace without attaching

`dl <workspace> up` prepares one, and running it repeatedly is
cheap on purpose: against a container that is already up, a second `up` costs one
`devpod status` and nothing else. It used to also pay the tools setup pass,
~1.7s of `devpod ssh` to be told the tools it was told about last time, and now
reuses the recorded answer instead. See
[The trip a launch can skip](workspace-tools.md#the-trip-a-launch-can-skip) for what makes a recorded
answer stop being believed; the short version is that any completed `devpod up`,
by anything, does.

There is no flag that shares one container across several branches, and none that
warms a workspace in the background. An earlier revision of this section
documented `--shared` and `--warm` with worked examples; neither has ever existed
in the shipped `dl`, which exits 2 on both, and the guard in
`test/test_readme_cli_doc.py` is why a third cannot appear here unnoticed.

