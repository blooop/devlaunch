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

One workspace, one id, three ways of reading it. The id itself is what devpod is
addressed by, what `dl --ls` prints and what the container's hostname is set to:
everything that has to be unique, or has to be typed back, or has to fit in a DNS
label.

The other two are renderings of that same id, cut to what their surface is for:

| Where | Reads | Why that shape |
|-------|-------|----------------|
| `dl --ls`, the hostname, devpod | `devlaunch-main-3j1t` | Addressed and typed back, so it must be unique and it must be one word |
| The [terminal tab](workspace-tools.md#naming-the-terminal-after-the-workspace) | `devlaunch@main` | A handful of characters read at a glance beside a dozen others, so the suffix goes and the branch stays the id's slug |
| The [selector](cli.md#the-selector) | `blooop \| devlaunch \| main` | One row at a time with the width of a terminal, so the owner comes back and the branch is spelled in full, out of the clone's `HEAD` |

They are renderings and not separate derivations, which is what keeps them
matchable: the tab is the id with the suffix off and one dash spelled `@`, so the
first two rows are recognisably the same workspace. This table is where the tab's
spelling is decided, and no other page states it as the answer to a command; where
one appears elsewhere it is inside an example of the mechanism, like the escape
sequence and the profile line in
[workspace-tools.md](workspace-tools.md#naming-the-terminal-after-the-workspace).
`a_label_is_the_id_with_the_suffix_off_and_an_at_where_the_dash_was` in
`rust/devlaunch-core/src/domain/workspace_id.rs` pins the tab's cell against what
`WorkspaceId::label` returns.

Which dash the `@` replaces is not readable off the id, since a repo slug holds
dashes of its own, so the tab's name travels with the launch that resolved it rather
than being recovered later. A workspace you name by its bare id on the command line
is therefore titled by that id. The selector is not: it read the triple to draw the
row, and hands it on with the pick.

The tab is the one that gives something up. It does not name the owner, since an id
never carried one, so a fork and its upstream read alike; and it spells the branch as
the id's slug, so `feature/auth` reads as `feature-auth`. Both are recoverable in the
selector, which is where a name is read carefully rather than glanced at.

At 47 characters the id leaves 17 of the 64-byte hostname limit for tools that stack
their own prefixes onto the container name. That was about 26 when the hostname was the
id without its suffix.

47 is held by devpod instead: it is one character inside devpod's own hard ceiling of
48, and a 49-character id is refused outright rather than truncated.

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
  is exactly as you left it; freshness inside it is your own `git pull`. `dl` now
  says so when it matters: launch `owner/repo@branch` and, if the checkout in
  `dl`'s clone is behind the `origin/<branch>` that clone last fetched, the attach
  reports how far behind before handing over the shell. That report is read out of
  the clone and costs no network call, so it says how the checkout stands against
  a ref of whatever age and never claims to know the remote now. You get it for a
  spec that names a branch, since that is the shape which implies a claim about
  one; `dl <workspace-id>` carries no branch and stays silent.
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

### Which verb moves the checkout, and it is only one

`rm` is the only verb that refreshes git state. Everything else acts on the
container and leaves the clone standing:

- `restart`, `recreate` and `reset` all reach a workspace devpod already knows,
  so they take the attach path above and run no git. `reset` is the one worth
  spelling out, because "clean slate" reads like a promise about the checkout: it
  passes devpod's `--reset`, which recreates the container and removes its
  volumes, and additionally removes the *source* only for a workspace devpod
  cloned itself. `dl` hands devpod a local folder (its own clone), never a git
  URL, so there is no devpod-managed source to remove and the clone is untouched.
- `dl <workspace> rm` deletes the workspace and the clone with it, so the next
  launch is a cold one: a fetch, a fresh clone, and the branch reset to the
  fetched ref.
- Inside the container, `git fetch` and `git pull` are yours and always were.

A clone that is already on disk is never fast-forwarded for you, even on a cold
launch: an existing directory gets a plain `git checkout <branch>` so that
uncommitted work survives, and only a directory `dl` has just created is reset to
the fetched ref.

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
