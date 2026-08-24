# Cleaning up, and what a workspace costs

`dl --prune`, `dl --purge` and `dl --reconcile` in full, plus the disk
accounting behind `dl --ls --size` and the JSON a cleanup tool reads.

## Cleaning up: purge, prune, reconcile

The three global commands that touch what is already on disk, in full. `--prune` takes clone
directories and no workspaces, `--purge` takes both but only what devlaunch made, and `--reconcile`
removes nothing; it repairs records that stopped matching the disk. Each one prints its plan and
asks before acting, and `-y` is what skips the question.

One exception, and it is not a released build: a binary compiled from somebody's
working tree appends `-dev` (`dl <version>-dev`). That comes from the `dev-build`
cargo feature, which `./dev.sh` builds with and nothing that ships enables, and it
is what tells `dl-next` apart from `dl` when both are on PATH. See "Two installs"
in AGENTS.md.

### What a delete takes with it

Removing a workspace removes three things: the devpod workspace, the local clone
(unless it holds work that exists nowhere else, see [cleaning up
workspaces](#cleaning-up-workspaces)), and **the named Docker volumes that
workspace's devcontainer created**. Every path that removes a workspace does all three:
`dl <ws> rm`, `dl <ws> --rm`, and `--purge`.

Two volumes per workspace, both named from what devpod recorded substituting into
the devcontainer:

| volume | declared by |
| --- | --- |
| `<workspace-folder-basename>-pixi` | a `mounts` entry in the devcontainer, for the `.pixi` cache. This repo's own devcontainer has one |
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
  and no `docker` command runs at all, rather than one carrying a made-up name,
  which would be somebody else's disk.
- **It is best-effort, and cannot fail a delete.** The workspace is gone either
  way; reporting failure would send you looking for a workspace that is not there.
  A volume Docker will not release, one another container still holds, say, is a
  line on stderr and nothing more. A machine with no `docker` at all says nothing,
  because a machine with no Docker never made these volumes.
- **Images are still yours.** See [the disk neither command
  frees](#the-disk-neither-command-frees). That boundary is about images now, and
  deliberately stays there.

### What purge deletes

devpod's workspace list is shared. A workspace you made with `devpod up`, or that
another tool made, sits in the same list as the ones `dl` made, and `dl --purge`
has no business destroying it. So it deletes only the workspaces devlaunch
created, meaning the clones it made under its own cache directory (`$XDG_CACHE_HOME` or
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
cannot tell them from a workspace you made by hand. And a `config.toml` that
points `repos_dir` outside the cache puts the clones somewhere `--purge` does not
remove either, so those are left too. Delete any of them with `dl <workspace> rm`.
Erring this way is deliberate: a purge that skips one of your own workspaces
costs you a command, and the other kind of mistake costs you work you cannot get
back.

#### When part of the cache will not go

A container writes into its clone as its own user, `vscode` at uid 1000 in the
standard devcontainer base image. Where your host user is uid 1000 too, nothing
here comes up. Where it is not, on CI, a shared machine, a container running as
root, or devlaunch developed inside its own devcontainer, the directories the
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
one you answered `n` to. `dl --prune` ends on the same one, in the same words.
See [the disk neither command frees](#the-disk-neither-command-frees).

Exit status is `1`, because a clone you were told would go is still on disk. It
used to be `1` with the *whole* cache still standing: the first refusal stopped
the purge, so the completion caches, `metadata.json` and every other clone
survived on account of one directory.

When **none** of it goes, meaning nothing under the cache came away at all, which is
what a symlinked cache root gives you, or one that cannot even be looked at, or
one whose every entry refused, the headline says that instead of claiming a
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
lives, because it is the one that decides whether you still have clones to go
and look for.

What is listed is the directory, once, not the hundreds of files inside it.
Unlinking needs write permission on the directory rather than on the file, so
every entry in that clone refuses separately and they are all the same fact.
Two *separately* unwritable directories on one path are two lines, though,
because clearing the inner one would leave the outer one just as stuck.

Each line carries what the system actually said. A container running as another
user is the common cause, but a read-only mount, `chattr +i` and a busy
mountpoint all land here too, and `sudo rm -rf` does not fix those, which is
why the report offers the cause rather than asserting it.

If you have **moved your cache** by making `~/.cache/devlaunch` a symlink, a
purge refuses it and names the target rather than following it. Remove the real
directory yourself if you meant to: following the link would empty a directory
you never named, and removing just the link would report a clean sweep while
your clones sat on the other volume.

### Pruning the clones nothing opens

A workspace per branch means clone directories accumulate under the cache, and
until now nothing removed them: measured on one host, **52 clone directories for
17 live devpod workspaces, 37 of them attached to nothing, 4.00 GB, against
7.86 GB still in use.** `--purge` is the wrong tool for that, being
all-or-nothing: the only way to get the 4 GB back was to destroy the 7.86 GB
too, and every bare cache with it.

`dl --prune` removes exactly the clone directories no live workspace opens. It
never deletes a devpod workspace, a container, an image or a volume, never
touches a repo's `.bare` cache (0.08 GB for seven repos, and it is what makes
the next clone of a repo fast), and never looks outside
`<cache>/devlaunch/repos`. Every directory it finds is one of three things:

- **a live workspace opens it.** Kept, and named with the workspace that has
  it. "Opens" means at *or under*: a workspace opened on a subdirectory of a
  clone still needs the clone.
- **nothing opens it.** Removed, unless it holds work that exists nowhere else,
  or `git` would not say what it holds. A clone a container wrote as another
  user is unreadable rather than empty, and "cannot tell" is kept, not removed.
- **`dl`'s records and devpod's disagree about it.** Kept, always. This is
  [#88](https://github.com/blooop/devlaunch/issues/88)'s shape. On that ticket's
  host, 36 devpod workspaces out of 39 recorded a source folder that was gone or
  was a config-only stub, while the real checkout sat beside it under a newer
  naming scheme, so a perfectly healthy clone was opened by nobody, and the
  stub was the only thing anything pointed at. `--prune` will not guess which
  clone such a workspace needs: it keeps every clone of that repository and
  names the record to go and fix. `--force` does not move any of them.
  [`dl --reconcile`](#reconciling-records-that-disagree) is what fixes them.

Note that *every* directory two levels under `<cache>/devlaunch/repos` is a
candidate, so a stray directory somebody left there is looked at like any other.
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
refuses in. 13 of those 37 stale clones did, two of them with real unpushed
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

**Nothing here runs on its own.** A full scan measured 1017 ms on that host,
about two warm launches, and it gets slower exactly as the cache gets fuller,
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
That file was append-only in practice, 49 records for 17 live workspaces on the
same host, and this is the first thing that prunes it.

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
volumes and 13.88 GB of build cache**. An order of magnitude more, sitting
behind a command that had just said "Removed". Saying nothing is what makes a
freed figure read as *all* of it, so both commands say this instead, whether they
removed 40 clones, found nothing to remove, or were answered `n` at the
confirmation. The report you get for saying `n` is a reason to print it, not an
exception: that is where somebody is deciding what is worth deleting.

**The sentence used to say "images or volumes", and volumes came off it**
(devlaunch#325). Deleting a workspace now removes the named volumes its
devcontainer created, see [what a delete takes with
it](#what-a-delete-takes-with-it), so a disclaimer that still covered them would
be describing a leak that has been fixed. The `--prune` half of the pair still
frees no volume at all, and that is not an oversight either: it removes clone
*directories* and never deletes a workspace, so there is no workspace whose
volumes it could be taking.

**It is a sentence, not a measurement.** `dl` runs no `docker` command to print
it, so there is nothing to be slow and nothing to fail where Docker is absent,
stopped, or reachable only as another user. The figures above are this README's,
from the host it was measured on, not from your machine. `docker system df` is
where yours are.

**And it points rather than offers.** There is deliberately no `dl` flag that
removes an image, and no list of image ids here to paste into `docker image rm`.
Images devpod builds carry no devlaunch or devpod label, so any list `dl` printed
would be a guess at which of them belong to these workspaces, and `docker image
prune -a` is not scoped to devlaunch at all: it would take images built by
everything else on the machine. Deleting them is a decision with your own
containers on the other side of it, and `docker system df` is the tool that shows
you what it costs.

### Reconciling records that disagree

`dl` keeps its own record of every workspace, and devpod keeps one too. They
agree until the naming that connects them moves, and it did move once, when
workspace ids and clone-directory names gained a hashed suffix. `dl`'s records
were migrated to the new naming; devpod's were not, because nothing knew to
touch them. On the host that reported it, **36 of 39 devpod workspaces recorded
a source folder that was missing, or was a stub with no `.git` in it**, while the
real checkout sat next to it under the new name. Nothing was deleted and nothing
was corrupted: `dl` was simply asking devpod about workspaces devpod had never
been given, and devpod was answering correctly that there were none.

Two things fix that, and they are different jobs. `dl` now **writes the devpod
workspace id down** when it creates a workspace, so the naming can move again
without taking anything with it. That is automatic and needs no command. It
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

It matches the two sides **by path, never by id**. The id is the thing that
changed, so it connects nothing, while the source folder devpod kept still names
the owner and the repository exactly, and its last component still names the
branch in one of the three ways `dl` has named a clone directory. Where that
match is not unique it is refused rather than guessed: a clone a live workspace
already opens, at it or anywhere under it, is never taken from it, a clone two
dead records both match is claimed by neither, and a name that two clones answer
to (the old flattened spelling turned `feature/auth`, `feature auth` and
`feature:auth` all into `feature-auth`) adopts neither of them. If a live
workspace's source cannot be followed at all, the whole command stops the way
`dl --prune` does, because such a workspace could be holding any of the clones on
offer. **Nothing is ever deleted.** A workspace `dl` cannot match
is named and left exactly where it is, because whether a workspace is finished
with is not something `dl` can know, and the two mistakes are not the same size.

Run it as often as you like. A repaired workspace is no longer sourced at a
non-checkout, so a second run finds nothing to do.

**A re-pointed workspace still needs rebuilding.** Its container was built with
the dead path bind-mounted into it, and changing a record does not move a mount.
`dl <workspace> recreate` is what finishes the repair, and it is the step that
needs Docker.

**Do not point an old `dl` at a reconciled cache.** A `dl` from before the naming
changed derives the old directory name, does not find it, and treats the launch
as a cold one: it clones a second directory under the old name, registers a
second devpod workspace, and rewrites that branch's record with the old naming
and an empty workspace id. That undoes the repair for that one workspace, and
leaves you two clones of the branch. It is not destructive and the next
`dl --reconcile` sorts it out, but a machine that runs both builds against one
cache will keep re-breaking. Upgrade the old one, or give it its own
`XDG_CACHE_HOME`.

### Cleaning up workspaces

One workspace per branch means workspaces accumulate, and `--purge` is the wrong
tool for tidying: it is all-or-nothing and takes the caches with it.

**devlaunch does not decide which workspaces are finished.** Whether a piece of
work is over is a fact about a ticket, a review, or somebody's intent, and `dl`
knows about clones and containers. Inferring it from the branch, merged into
the default or deleted from the remote, was tried and dropped: it reads like a
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
is on now, which can differ), `path`, `state`, `lastUsed`, and the field a
cleanup tool must not ignore, `unsaved`:

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
| `{"couldNotTell": "<why>"}` | `git` could not read the clone as a repository: a half-removed `.git`, an interrupted delete. The files are still there and nothing has established that they exist anywhere else. |

The changed paths are named, not just counted, and that matters more than it
looks: a devcontainer that runs a package install in its `postCreateCommand` can
leave a tracked lockfile modified in *every* workspace it builds. This repo's
own did, until that install became `pixi install --frozen`, and as a bare count
that is indistinguishable from an hour of unsaved work. A cleanup tool believing the count would then never clean anything. Named,
it is judgeable. A workspace `dl` did not create reports `devlaunch: false` and
no `unsaved`. `unsaved` is `null` exactly where `devlaunch` is `false`, and
nowhere else: there is no clone of `dl`'s to protect, and it has no business
inspecting your checkout. (`repo` and `branch` are a weaker test and not the
same set: they come from `dl`'s metadata record, and a clone `dl` owns can have
lost its record while the clone and the work in it are still on disk. That clone
is inspected and reported like any other.)

**`dl <workspace> rm` refuses to delete a clone it would lose work from, both when
the recorded clone holds unsaved work and when it cannot tell what that clone
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
work. It is `dl` declining to destroy the only copy of something, including
when it cannot prove there is another copy. Say `--force` if you mean it.

`--force` changes one more answer: an already-absent workspace counts as
deleted, like `rm -f`. Unforced, `rm` reports devpod's refusal to delete a
workspace it does not have; forced, the contract is the state afterwards, not
that a delete happened, which is what lets the [cold benchmark's per-run
reset](performance.md#measuring-launch-time) run before the first launch, when there is
nothing to remove yet.

The guard reads `dl`'s metadata record, so the recorded directory is the one it
asks about. (The delete does not always remove that same directory: when the
recorded path is not on disk it falls back to a derived one. That divergence is
older than this guard and is tracked as devlaunch#174.) One case is therefore
neither a refusal nor a delete: a clone under `dl`'s cache that has **no** record, from a metadata write
that failed, a record pruned, or a cache restored without one. The listing still
reports what that clone holds, so `unsaved` is the field to read; but `rm`
removes the devpod workspace, exits `0` without asking for `--force`, and leaves
the clone on disk, because there is no recorded directory for it to remove
either. Nothing is destroyed, and nothing then points at the clone: it is yours
to keep or to `rm -rf` by hand.

[`wf`](https://github.com/blooop/wayfinder) is the caller this was built for. It
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
share them. A size that walked each workspace on its own, which is what `du`
does when you point it at one directory, counting the blocks every file in it
occupies, bills each workspace for the whole shared pool.

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
all that was ever keeping it true. A `file://` URL, an intermediate copy, or an
explicit `--no-hardlinks` would each forfeit it with nothing failing and no
warning printed. Measured on this repo, with `du -sc` over the cache and each
clone's `.git`, ext4, git 2.55.0, that is 2400 KB for the cache plus one
workspace against 4472 KB unshared, and 196 KB rather than 2268 KB of `.git` for
every workspace after the first. So an integration test asserts the pack files are
the cache's, same inode and more than one link, and that assertion goes red on
all three. No clone flag is used to guard it: `--local` is already the default
and does not even reject a `file://` source, and `--shared`/`--reference` were
measured to leave a workspace that fails `git fsck` once the cache has fetched
and gc'd, for a 2 KB saving.

Sharing does erode, in one measured way that is a safety property rather than a
fault. When the cache repacks, an existing workspace's pack loses its second
link and becomes that workspace's own complete copy, still passing `git fsck`.
The workspace stops being cheap and never stops being valid, which is the trade
`--shared` and `--reference` get wrong, and the reason they are not used.

**Large files are shared the same way, but nothing about `git clone` does it for
you.** git-lfs objects are not git objects: the clone does not carry them at
all, so a workspace of an LFS repo used to download the entire payload from the
forge and keep a private copy of it in `.git/lfs/objects`, every workspace,
every time, on top of the worktree copy. `dl` now makes the bare cache the
repo's LFS store as well: the payload is fetched once into `<repo>/.bare/lfs`
for the branch being launched, and each workspace materializes out of *that*,
which git-lfs does by hardlinking. Measured with git-lfs 3.7.1 on ext4: the
workspace's object file is the same `(st_dev, st_ino)` as the cache's, so its
store costs nothing, and the materialization succeeds with the remote deleted
from disk. The second workspace of an LFS repo touches the network for its
large files not at all. What remains per workspace is the worktree copy, which
is real bytes and cannot be shared: a container build has to be able to read
them. If the cache cannot supply an object, on a first launch offline or for a
payload the branch alone introduces, the old download from `origin` still runs, and a
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
  them. They become the last workspace's the moment it is the last one, which
  is exactly when deleting it *would* free them. In the table above that is the
  last two rows read against each other: 68,702,208 reported bytes against
  353,882,112 held.
- **A workspace's size can change without the workspace changing**, when a
  sibling that was sharing with it goes away. That is the truth about shared
  storage.

A workspace `dl` did not create reads `-` (`null` in JSON): there is no clone of
`dl`'s there to measure, and walking your own project directory is not `dl`'s to
do. The table and the JSON decide that from the same rule, is the clone one
`dl` put in its own cache, the same question `--purge` deletes by, so the two
always name the same set of workspaces as measurable. Where a walk hits a
directory it cannot read, and a container writing into its clone as its own user
makes that happen, the answer is a floor rather than a
total: `≥2.0 MiB` in the table, and `{"atLeastBytes": …, "unreadable": 1}` in
JSON instead of `{"exclusiveBytes": …}`. A partial measurement never comes back
looking like a complete one.

**It is opt-in because it walks the whole clone.** Plain `dl --ls` is one devpod
round-trip and no filesystem work at all, and the walk is O(files) with no
ceiling. Measured with the shipped code on one machine, on Ubuntu 24.04 and ext4
with a warm page cache, five runs after a warm-up and the machine otherwise busy:
a real 8,309-entry clone walked in 24 to 28 ms, this repo's own tree with its built
environment inside it (9,124 entries) in 17 to 21 ms, and a 114,817-entry tree in
232 to 239 ms. No cold-cache figure is quoted because none was taken: dropping the
page cache needs root on that machine. Those are one machine's numbers on warm
cache and yours will differ, but the shape is the point. It grows with the file
count, and a devcontainer that builds its environment *inside* the clone (this
repo's own does) is most of that count. That is not a bill a listing should
present unasked.

Docker images and named volumes are not counted: `dl` did not create the layer
store, and a volume is not a directory it can walk. `docker system df` is the
tool that knows, the same boundary [`--prune` and `--purge`
name](#the-disk-neither-command-frees) when they finish. Not counting a volume is
a different thing from not removing it: a workspace's volumes go when the
workspace does, see [what a delete takes with
it](#what-a-delete-takes-with-it). What is missing here is only the *figure*.

