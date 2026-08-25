# Workspace ids, and how fresh a launch is

Two facts about a workspace that are worth knowing but that you never have to
type: the id `dl` derives for it, and how recent the branch you land on is.

## Workspace IDs

`dl user/repo@branch` derives one id that names both the devpod workspace (what you
see in `dl --ls`) and the clone directory under `~/.cache/devlaunch/repos/`:

```
<repo-slug>-<branch-slug>-<hash>           at most 47 characters

blooop/devlaunch@main                             -> devlaunch-main-3j1t
blooop/devlaunch@feature/auth                     -> devlaunch-feature-auth-np10
blooop/devlaunch@feature-auth                     -> devlaunch-feature-auth-lsi0
blooop/test_renv@nb4                              -> test-renv-nb4-n95z
kinisi-robotics/kinisi_ros@ags-devcontainer-tooling-support
                                                  -> kinisi-ros-ags-devcontainer-tooling-suppor-17uu
blooop/devlaunch@dependabot/github_actions/codecov/codecov-action-6
                                                  -> devlaunch-dependabot-codecov-action-6-amlt
```

The four-character suffix is a hash of the full `(owner, repo, branch)` triple, written
in base 36. It is what makes the id unique: the readable part is shortened to fit the
length limit, and shortening it does not affect whether two branches share an id. Long
branch names drop whole `/`-separated middle segments before losing characters, so the
part that identifies the branch survives. Note the third and fourth lines above:
`feature/auth` and `feature-auth` read the same once slugged but are different branches,
and they get different ids.

The suffix was eight characters and spelled in pronounceable syllables (`zovomobo`,
`hesirora`). Base 36 packs 5.17 bits into a character where the syllable table managed
3.0, so the suffix halves in length, and the four characters it gives back go to the
branch, which is the part anyone reads. What it costs is collision headroom: a tenth of
the old room, 20.7 bits against the old 24, or roughly one chance in
37000 that ten near-identical long branches in one repository collide. Only branches
whose readable half truncates to the same string can contend at all, so the number that
matters is a repository's crop of `release/999...176`-shaped names rather than the count
of workspaces on the machine.

Owner and repo are matched case-insensitively, the way GitHub treats them, so
`dl NVIDIA/cuda-samples@main` and `dl nvidia/cuda-samples@main` are the same workspace.
Branch names are case-sensitive, because git refs are.

URL specs (`dl github.com/owner/repo`) get an id in the same shape, with the suffix
hashed over the URL.

One workspace has one name. The id is what devpod is addressed by, what `dl --ls`
prints, what the container's hostname is set to, and what dl writes on the terminal
tab, so a tab and a listing row can be matched by eye. The hostname used to drop the
suffix and the tab used to carry `owner/repo@branch` instead, which meant three
spellings for one workspace and a tab whose shape depended on how the workspace had
been reached. A bare devpod name, a path and a URL never had an `owner/repo@branch` to
show. What the change costs is that the tab no longer names the owner, so a fork and
its upstream read alike, and it spells the branch as a slug.

At 47 characters the id leaves 17 of the 64-byte hostname limit for tools that stack
their own prefixes onto the container name. That was about 26 when the hostname was the
id without its suffix.

47 is held by devpod instead: it is one character inside devpod's own hard ceiling of
48, and a 49-character id is refused outright rather than truncated.

Where the id is *not* what you read is the selector: it draws `owner | repo | branch`
(see [The selector](cli.md#the-selector)), because picking a row off a list is the one
job the suffix is no help with. Everywhere else it is the id, including
[the terminal tab](workspace-tools.md#naming-the-terminal-after-the-workspace).

Branch names must be safe as both git refs and directory names, so a name with a space or
a leading dash is rejected rather than quietly rewritten.

### Upgrading from an older devlaunch

The ids on your machine were derived by an older rule, so the directories and containers
already there are named by it. The first `dl user/repo…` command after upgrading migrates
the cache once, and leaves what it did behind in the cache directory (the two listings
named below). `dl --help`, `dl --version`, `dl --ls` and opening an existing workspace by
name do not trigger it.

This has happened twice. Clone directories were once named after the flattened branch
alone (`main`), and the suffix was once eight characters of pronounceable syllables
(`devlaunch-main-zovomobo`). Both reach today's names through the same pass, which
re-derives every id from the `(owner, repo, branch)` its record already stores, so there
is nothing to do differently depending on which one your cache is on.

**Your clone directories are renamed.** What was
`~/.cache/devlaunch/repos/blooop/devlaunch/main`, or
`~/.cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-zovomobo`, becomes
`~/.cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-3j1t`. A workspace is a git
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
in the shipped `dl`, which exits 2 on both. `test/test_readme_cli_doc.py` is what
caught that, by handing every flag the README writes on a `dl` line to the
binary's own parser. It reads the README and not this page, so the protection
comes from documenting flags there rather than here.
