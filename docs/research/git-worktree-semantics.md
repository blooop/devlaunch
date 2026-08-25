# Nested worktrees, and what `git worktree prune` actually reaches

Research for [#448](https://github.com/blooop/devlaunch/issues/448) on the
[reclaim map](https://github.com/blooop/devlaunch/issues/444). Knowledge only.
What the sweep should *be* is the keystone,
[#445](https://github.com/blooop/devlaunch/issues/445).

Two review findings on [PR #442](https://github.com/blooop/devlaunch/pull/442)
rest on git's behaviour rather than on devlaunch's, and both were asserted
without a version or a citation: T1, that a worktree nested inside a removed one
is lost unreported even when `git worktree lock`ed, and T2, that
`git worktree prune`'s blast radius is wider than the enumeration the sweep gates
it with. Both are true, and both are worse than stated. One of the premises the
current code rests on is false.

## How this was established, and on what

Every claim is labelled **[doc]** when it comes from git's documentation, source
or test suite, and **[exp]** when it comes from running git here.

The experiments ran on **four gits**: 2.51.1 (this devcontainer,
`/usr/local/bin/git`, the version the reviewer measured on), 2.43.0 (this
container's Ubuntu 24.04 `/usr/bin/git`), 2.34.1 (`ubuntu:22.04`) and 2.30.2
(`debian:bullseye`), the last two under docker. That range brackets what a
devlaunch host and a devlaunch container plausibly are.

**Every probe gave the same answer on all four**, with two cosmetic exceptions,
noted where they land: 2.30.2 prints no `locked` or `prunable` annotations in
`git worktree list --porcelain` at all (they arrived in 2.31.0), and the error
text for a refused `repair` differs.

Documentation quotes are from `Documentation/git-worktree.adoc` and
`Documentation/gitrepository-layout.adoc` at tag **v2.51.1**, which is the
binary the probes ran on. Where master (2.54.0 onward) has reworded a passage
that is noted; no behaviour differs.

The container-path case, which is devlaunch's whole situation, was reproduced by
adding an ordinary linked worktree and then rewriting
`<clone>/.git/worktrees/<id>/gitdir` to a `/workspaces/...` path that does not
exist on the machine running git. That is byte-for-byte the state a host meets
after a container registers a worktree, and it is how every probe below that says
"container path" was set up.

## The short version

1. **There is a supported way to drop one registration, and devlaunch is not
   using it.** `git worktree remove <recorded-path>` succeeds, silently and with
   exit 0, on a registration whose recorded path does not resolve, and it drops
   exactly that registration. git's own error messages name it as the way to
   clear one. The doc comment on `Git::worktree_prune` says git offers no such
   thing and is wrong.
2. **Nesting is not a relationship git records.** A nested worktree's admin
   directory is a sibling of its parent's, flat under the main repository, and its
   gitfile points at the main repository. The relation exists only as a prefix
   relation between two recorded paths, so a sweep that wants it has to compute
   it. It is tested by git and documented nowhere.
3. **`git worktree remove` on a parent deletes a nested worktree's working tree,
   with its uncommitted work, and says nothing.** A lock on the nested worktree
   does not stop it, and neither does the absence of `--force` if the parent
   ignores the nested directory, which for an agent worktree directory it usually
   does.
4. **A lock protects against exactly three subcommands** and asserts nothing about
   liveness. #426's "locked is sacred" is devlaunch's policy, honouring the agent
   harness's convention. Git agrees with the mechanism and is silent on the
   meaning.
5. **`--porcelain` is not enough.** `locked` masks `prunable`; three registration
   states are pruned while never being listed at all; and `prunable` can be true
   of a directory full of data and false of a path holding no worktree.
6. **Prune has an arm that deregisters a live worktree.** Two registrations
   recording one path, or one recording the main worktree's path, are dropped as
   `duplicate entry` with no age gate and no path check.

## 1. Nesting

### Where the admin directory lands

**[doc]** `gitrepository-layout`, on the registration:

> `worktrees`::
>   Contains administrative data for linked
>   working trees. Each subdirectory contains the working tree-related
>   part of a linked working tree. This directory is ignored if
>   $GIT_COMMON_DIR is set, in which case
>   "$GIT_COMMON_DIR/worktrees" will be used instead.
>
> `worktrees/<id>/gitdir`::
>   A text file containing the absolute path back to the .git file
>   that points to here. This is used to check if the linked
>   repository has been manually removed and there is no need to
>   keep this directory any more. The mtime of this file should be
>   updated every time the linked repository is accessed.

**[doc]** `git-worktree`, `DETAILS`:

> Each linked worktree has a private sub-directory in the repository's
> `$GIT_DIR/worktrees` directory.  The private sub-directory's name is usually
> the base name of the linked worktree's path, possibly appended with a
> number to make it unique.

The name is the **basename**, made unique with a number. There is no hierarchy
in it and no room for one.

**[exp]** A worktree `outer`, and a worktree `inner` created from inside `outer`
at `outer/inner`:

```
$ ls main-repo/.git/worktrees/
inner  outer
$ cat outer/.git
gitdir: .../main-repo/.git/worktrees/outer
$ cat outer/inner/.git
gitdir: .../main-repo/.git/worktrees/inner
$ cat main-repo/.git/worktrees/inner/commondir
../..
$ git -C outer/inner rev-parse --git-common-dir
.../main-repo/.git
```

Identical on 2.30.2, 2.34.1, 2.43.0, 2.51.1.

So the inner worktree's gitfile points at the **main repository**, not at the
outer worktree, and `commondir` is `../..` for both, at any depth. There is no
parent field, no child list, no ordering: the two registrations are siblings in
the only place git writes anything down. **Removing `outer`'s registration cannot
have a defined effect on `inner`'s, because nothing connects them.** The only
trace of the nesting is that one recorded path is a prefix of the other, which is
a fact about two strings that git does not maintain.

### Supported, or merely not refused?

**[doc]** The phrase "nested worktree" does not appear anywhere in git's
documentation or source, in either direction: no statement of support, no
warning, no `BUGS` entry. `git-worktree`'s `BUGS` section is about something else
entirely:

> Multiple checkout in general is still experimental, and the support
> for submodules is incomplete. It is NOT recommended to make multiple
> checkouts of a superproject.

**[doc]** `git worktree add`'s only path validation is `check_candidate_path()`,
which tests two things: whether the path already exists non-empty, and whether
that exact path is already registered. Nothing tests containment in another
worktree, and nothing tests depth.

**[doc]** But git's own test suite exercises nesting, in two places, which is the
strongest primary-source signal available that it is meant to work:

> ```sh
> test_expect_success '"add" from a linked checkout' '
>   (
>     cd here &&
>     git worktree add --detach nested-here main &&
>     cd nested-here &&
>     git fsck
>   )
> '
> ```
> `t/t2400-worktree-add.sh`

> ```sh
> test_expect_success 'not prune proper worktrees inside linked worktree with relative paths' '
>   ...
>       git worktree add ../wt_ext &&
>       git worktree add wt_int &&
>       cd wt_int &&
>       git worktree prune -v >out &&
>       test_must_be_empty out &&
> ```
> `t/t2401-worktree-prune.sh`

So the answer is neither of the two the ticket offered: nesting is **tested and
undescribed**. Creating one works and is meant to. What git has never said
anything about is what an outer worktree's removal means for an inner one, which
is the question devlaunch needs, and the answer is that git does not model the
relation at all.

### What removing the outer one actually does

**[exp]** All four versions, all silent, all exit 0 except where noted.

| probe | result |
| --- | --- |
| `git worktree remove ../outer`, nested `inner` untracked in outer | refuses: `'../outer' contains modified or untracked files, use --force to delete it` |
| `git worktree remove ../outer`, nested `inner` **ignored** by outer's `.gitignore` | **succeeds. `inner`'s working tree is deleted. No output.** |
| `git worktree remove --force ../outer` | succeeds. `inner`'s working tree is deleted. No output. |
| `git worktree remove --force ../outer`, `inner` **`git worktree lock`ed** | succeeds. `inner`'s working tree is deleted. No output. |

`remove` deletes the parent's directory tree, and the nested worktree is inside
that tree. Nothing consults the nested registration, so nothing honours its lock,
its uncommitted work, or its unpushed commits. What survives is the registration:

```
$ git worktree list --porcelain          # after remove --force ../outer
worktree .../r
worktree .../outer/inner
prunable gitdir file points to non-existent location
```

and when the inner one was locked, the same record reads `locked live` with **no
`prunable` line** (section 5, and this matters).

Two things follow that are worth stating separately.

**The untracked-files guard is not a guard.** It fired in the first row only
because the nested directory was untracked content in the parent. Ignore it in
the parent's `.gitignore` and plain `git worktree remove`, no flag typed, quietly
destroys the nested worktree. Agent worktree directories are routinely
gitignored, so this is the ordinary case, not the exotic one.

**A lock is not inherited and does not radiate.** `lock` is honoured by exactly
the subcommands that name the locked worktree (section 4). Deleting something
that *contains* it never names it.

### Cross-repository nesting is worse

**[exp]** A worktree of repository `r2` placed inside a worktree of `r1`:

```
$ git -C r1 worktree list --porcelain | grep ^worktree
worktree .../r1
worktree .../outer1
$ git -C r2 worktree list --porcelain | grep ^worktree
worktree .../r2
worktree .../outer1/nested2
$ git -C r1 worktree remove --force .../outer1      # exit 0, no output
nested2 exists? NO
$ git -C r2 worktree list --porcelain | grep -E '^worktree|prunable'
worktree .../r2
worktree .../outer1/nested2
prunable gitdir file points to non-existent location
```

`r1` never sees the nested registration, and no `git worktree prune` in `r1` will
ever clean it: the stale registration lives in `r2`, which nobody in this sweep
is looking at. A sweep whose unit is "a registration in this clone" cannot see
this case at all. Its detectable shape from the host is a gitfile under a
candidate directory pointing at an admin directory that is not in this clone.

## 2. `git worktree prune`'s blast radius

### It takes no argument, and never has

**[doc]** Synopsis, unchanged across every version in the window:

```
git worktree prune [-n] [-v] [--expire <expire>]
```

and the refusal is asserted by git's own test:

> ```sh
> test_expect_success 'worktree prune on normal repo' '
>   git worktree prune &&
>   test_must_fail git worktree prune abc
> '
> ```
> `t/t2401-worktree-prune.sh`

**[exp]** `git worktree prune <path>` is a usage error on all four. There is no
`--worktree`, no path argument, no include or exclude. The command's domain is
the repository, by construction.

### What the documentation says, which is nearly nothing

**[doc]** v2.51.1's entire description:

> `prune`::
>
> Prune worktree information in `$GIT_DIR/worktrees`.

**[doc]** and `--expire`:

> `--expire <time>`::
>   With `prune`, only expire unused worktrees older than `<time>`.
> +
> With `list`, annotate missing worktrees as prunable if they are older than
> `<time>`.

**[doc]** with the automatic case in `DESCRIPTION`:

> If a working tree is deleted without using `git worktree remove`, then
> its associated administrative files, which reside in the repository
> (see "DETAILS" below), will eventually be removed automatically (see
> `gc.worktreePruneExpire` in linkgit:git-config[1]), or you can run
> `git worktree prune` in the main or any linked worktree to clean up any
> stale administrative files.

> `gc.worktreePruneExpire`::
>   When 'git gc' is run, it calls
>   'git worktree prune --expire 3.months.ago'.
>   This config variable can be used to set a different grace
>   period. The value "now" may be used to disable the grace
>   period and prune `$GIT_DIR/worktrees` immediately, or "never"
>   may be used to suppress pruning.

That is the whole documented contract. It does not say which registrations count
as unused, does not mention locks, and does not say what `<time>` is measured
against. Everything below comes from the source and the experiments.

Note the second half of `--expire`: it changes `list`'s annotation too, and
`list` defaults it to `TIME_MAX`, so by default every unlocked missing-path
registration is annotated `prunable`. `--expire` on `list` can silently remove
that annotation from a registration that is still absent.

### Exactly what it drops and what it spares

**[doc]** The decision is `should_prune_worktree()`, and its checks run in this
order (in `builtin/worktree.c` up to 2.47, `worktree.c` from 2.54; the logic is
unchanged from 2.30.0 to master, which was verified by diffing the two):

```c
	if (!is_directory(repo_path.buf)) { reason = "not a valid directory";        rc = 1; }
	if (file_exists(".../locked"))    { goto done; }              /* SPARED */
	if (stat(gitdir.buf, &st))        { reason = "gitdir file does not exist";   rc = 1; }
	if (fd < 0)                       { reason = "unable to read gitdir file";   rc = 1; }
	if (read_result != len)           { reason = "short read (...)";             rc = 1; }
	if (!len)                         { reason = "invalid gitdir file";          rc = 1; }
	if (!file_exists(dotgit.buf)) {
		if (stat(".../index", &st) || st.st_mtime <= expire)
		                          { reason = "gitdir file points to non-existent location"; rc = 1; }
	}
```

and the driver then removes duplicates among everything that survived:

```c
static void prune_dups(struct string_list *l)
{
	QSORT(l->items, l->nr, prune_cmp);
	for (i = 1; i < l->nr; i++)
		if (!fspathcmp(l->items[i].string, l->items[i - 1].string))
			prune_worktree(l->items[i].util, "duplicate entry");
}
```

**[exp]** Each arm, reproduced:

| state of the registration | pruned? | reason printed |
| --- | --- | --- |
| admin path is not a directory | yes | `not a valid directory` |
| **`locked` file present** | **no**, whatever else is true of it | -- |
| `gitdir` file missing from the admin directory | yes, **even when the working tree is alive and present** | `gitdir file does not exist` |
| `gitdir` file unreadable, or short read | yes | `unable to read gitdir file (...)`, `short read (...)` |
| `gitdir` file empty | yes | `invalid gitdir file` |
| recorded path's `.git` absent (deleted worktree, or a container path) | yes, subject to `--expire` | `gitdir file points to non-existent location` |
| recorded path's `.git` present | no | -- |
| **two registrations recording the same path** | yes, **all but the first, live or not, no age gate** | `duplicate entry` |
| **a registration recording the main worktree's path** | yes, that one | `duplicate entry` |
| the main worktree itself | never; it has no entry under `worktrees/` to iterate | -- |

**[doc]** Prune deletes only `$GIT_COMMON_DIR/worktrees/<id>`, recursively
(`delete_git_dir`). It never touches a working tree on disk. That is the one
thing about prune that is unambiguously safe.

Four of these arms deserve emphasis.

**Locked wins over everything.** Locked and absent, locked and present, locked
with its `gitdir` file deleted: spared in every combination, on all four
versions, and the check precedes every gitdir check. Git's own test pins it:

> ```sh
> test_expect_success 'not prune locked checkout' '
>   test_when_finished rm -r .git/worktrees &&
>   mkdir -p .git/worktrees/ghi &&
>   : >.git/worktrees/ghi/locked &&
>   git worktree prune &&
>   test_path_is_dir .git/worktrees/ghi
> '
> ```
> `t/t2401-worktree-prune.sh`

This confirms the review's claim and dates it: prune has spared locked entries
since before 2.10.0. There is no flag that overrides it.

**`gitdir file does not exist` ignores both liveness and `--expire`.** A live,
present, non-empty working tree loses its registration if the admin directory's
`gitdir` file goes missing:

```
$ git worktree add ../a -b a && rm .git/worktrees/a/gitdir
$ git worktree prune -n -v --expire=3.months.ago
Removing worktrees/a: gitdir file does not exist       # and ../a is still there
```

**`duplicate entry` deregisters a live worktree, and no `--expire` gates it.**
[exp] Two registrations both recording the path of a present, healthy worktree:

```
$ git worktree prune -n -v
Removing worktrees/b: duplicate entry
```

and a registration recording the main worktree's own path is dropped the same
way. Locking either one takes it out of the comparison entirely, so the duplicate
then survives. On a host where two containers have registered the same path this
arm fires against a worktree somebody is working in.

**The order of arms decides the reason, and the reason is all a caller sees.**
Two registrations recording the *same absent* path never reach duplicate
detection; both are dropped as `gitdir file points to non-existent location`
instead. [exp], reproduced.

### `--expire` is relevant, and its clock is the index

**[exp]** `--expire` gates **only** the `gitdir file points to non-existent
location` arm, and the mtime it compares is `.git/worktrees/<id>/index`:

```
$ git worktree add ../a -b a && rm -rf ../a
$ git worktree prune -n -v --expire=3.months.ago     # (nothing: spared, index is fresh)
$ touch -d '1 year ago' .git/worktrees/a/index
$ git worktree prune -n -v --expire=3.months.ago
Removing worktrees/a: gitdir file points to non-existent location
$ touch .git/worktrees/a/index && touch -d '1 year ago' .git/worktrees/a/gitdir
$ git worktree prune -n -v --expire=3.months.ago     # (nothing: the gitdir mtime is not the clock)
```

**[doc]** `gitrepository-layout`'s claim that "the mtime of this file should be
updated every time the linked repository is accessed", about `gitdir`, describes
an intent no current code implements: `should_prune_worktree()` stats `gitdir`
only for its size, and the grace period compares `worktrees/<id>/index`. A
missing `index` also means prune, however generous `--expire` is. So the de facto
last-accessed stamp for a registration is its index, not the file the layout
document names.

**[doc]** With no `--expire`, `expire` is initialised to `TIME_MAX`, so plain
`git worktree prune` prunes regardless of age. The practical relevance of
`--expire` to devlaunch runs the other way: `git gc` calls
`git worktree prune --expire` with `gc.worktreePruneExpire`, three months by
default, so **any `git gc` inside a container, including an automatic one, is a
third party that drops stale registrations.** Registrations disappearing between
two `dl` runs without `dl` doing it is a supported thing to happen.

### The blast radius exceeds `worktree list`'s domain by construction

**[exp]** Three registration states that prune deletes and
`git worktree list --porcelain` **never shows at all**: a missing `gitdir` file,
an empty one, and an unreadable one.

```
$ git worktree add ../a -b a && git worktree add ../b -b b
$ rm .git/worktrees/a/gitdir
$ git worktree list --porcelain
worktree .../r
...
worktree .../b                      # a is absent from the listing entirely
...
$ ls .git/worktrees
a  b
$ git worktree prune -n -v
Removing worktrees/a: gitdir file does not exist
```

This is T2's shape with no timing involved. Even a sweep that re-read the listing
at the instant before pruning would gate the prune with a set that cannot contain
these. `worktree list` enumerates registrations it can resolve; `prune`
enumerates directories under `worktrees/`. The second set is strictly larger, and
`duplicate entry` means it can act on entries the first set shows as healthy.

**[exp]** Prune does classify at run time rather than trusting an earlier dry
run: a healthy registration created between `prune -n` and `prune` survives. So
the concurrency half of T2 is devlaunch's plan going stale, not git's.

### So: is there a supported way to prune one registration?

**Yes.** `git worktree remove <recorded-path>` on a registration whose recorded
path does not resolve drops exactly that registration, exit 0, no output, nothing
else touched.

**[doc]** `remove`'s prose does not mention the case:

> `remove`::
>
> Remove a worktree. Only clean worktrees (no untracked files and no
> modification in tracked files) can be removed. Unclean worktrees or ones
> with submodules can be removed with `--force`. The main worktree cannot be
> removed.

**[doc]** but the code is explicit about it. `remove_worktree()` validates with
`WT_VALIDATE_WORKTREE_MISSING_OK`, puts both the cleanliness check and the
working-tree deletion inside `if (file_exists(wt->path))`, and then drops the
registration unconditionally:

```c
	if (validate_worktree(wt, &errmsg, WT_VALIDATE_WORKTREE_MISSING_OK))
		die(_("validation failed, cannot remove working tree: %s"), errmsg.buf);
	if (file_exists(wt->path)) {
		if (!force)
			check_clean_worktree(wt, av[0]);
		ret |= delete_git_work_tree(wt);
	}
	/* continue on even if ret is non-zero, there's no going back from here. */
	ret |= delete_git_dir(wt->id);
```

**[doc]** and git tells the user so, in the error `add` raises on a stale path:

> `'%s' is a missing but already registered worktree;`
> `use '%s -f' to override, or 'prune' or 'remove' to clear`

> `'%s' is a missing but locked worktree;`
> `use '%s -f -f' to override, or 'unlock' and 'prune' or 'remove' to clear`

`remove` is named there as the alternative to `prune` for clearing one entry.
That is as close to a documented contract as this gets, and it is git's own text.

**[doc]** The identifier rule is fully documented, and is what makes a container
path usable as the argument:

> `<worktree>`::
>   Worktrees can be identified by path, either relative or absolute.
> +
> If the last path components in the worktree's path is unique among
> worktrees, it can be used to identify a worktree. For example if you only
> have two worktrees, at `/abc/def/ghi` and `/abc/def/ggg`, then `ghi` or
> `def/ghi` is enough to point to the former worktree.

**[exp]** In devlaunch's exact shape: one clone, three registrations rewritten to
container paths, one locked, one nested inside another, invoked the way
`Git::about` invokes.

```
$ git worktree list --porcelain
worktree .../clone
...
worktree /workspaces/clone/.claude/worktrees/agent-a
prunable gitdir file points to non-existent location
worktree /workspaces/clone/.claude/worktrees/agent-a/nested
prunable gitdir file points to non-existent location
worktree /workspaces/clone/.claude/worktrees/agent-b
locked claude session

$ git --git-dir=<clone>/.git --work-tree=<clone> \
      worktree remove /workspaces/clone/.claude/worktrees/agent-a
                                                              # exit 0, no output
$ ls <clone>/.git/worktrees
agent-b  nested

$ git --git-dir=<clone>/.git --work-tree=<clone> \
      worktree remove /workspaces/clone/.claude/worktrees/agent-b
fatal: cannot remove a locked working tree, lock reason: claude session
use 'remove -f -f' to override or unlock first          # exit 128

$ git --git-dir=<clone>/.git --work-tree=<clone> \
      worktree remove /workspaces/clone/.claude/worktrees/agent-a/nested
$ ls <clone>/.git/worktrees                            # exit 0, no output
agent-b
```

The branches survive (`git branch --list` still shows `agent-a` and `nested`,
minus the `+` that marks a branch checked out in a worktree), the host
directories are untouched, and the nested registration is removable on its own,
in any order, because nothing relates it to its parent.

This holds on 2.30.2, 2.34.1, 2.43.0 and 2.51.1.

**[exp]** The refusals line up with what principle 1 wants:

| what the recorded path is on the machine running git | `remove` | `remove -f -f` |
| --- | --- | --- |
| absent | drops the registration, exit 0 | same |
| absent, and `locked` | `fatal: cannot remove a locked working tree` | drops it |
| **present, but not this worktree** (no `.git` there) | `fatal: validation failed, cannot remove working tree: '<path>/.git' does not exist` | **same refusal** |
| present and valid | the ordinary removal: dirty check, then delete the tree | forces past both |

The third row is the safety property worth naming. A recorded container path that
*does* resolve on the host, to somebody else's directory, is refused, not
deleted, and `-f -f` does not get past it either:

```
$ mkdir collide && echo precious > collide/p.txt      # not a worktree
$ # ... a registration recorded at .../collide
$ git worktree remove -f -f .../collide
fatal: validation failed, cannot remove working tree: '.../collide/.git' does not exist
$ cat collide/p.txt
precious
```

So `git worktree remove <recorded path>` cannot delete an unrelated directory: it
either finds the worktree it was told about or refuses. `git worktree prune`, in
the same state, drops the registration and leaves the directory stranded.

**[exp]** Ambiguity also fails towards keeping. Two registrations recorded at
`/workspaces/x/agent` and `/workspaces/y/agent`:

```
$ git worktree remove agent
fatal: 'agent' is not a working tree           # both matched; neither chosen
$ git worktree remove x/agent                  # exit 0, unique suffix
```

and passing a full recorded path that is also a *suffix* of another
registration's path (`/workspaces/clone/agent-a` alongside
`/host/mnt/workspaces/clone/agent-a`) removes the one whose path it exactly is.
Passing the whole recorded path, unmodified, is the unambiguous call.

**[doc][exp]** Two other commands drop a single stale registration as a side
effect, both documented, neither a fit here: `git worktree add -f <stale path>`
(*"if `<path>` is already assigned to some worktree but is missing [...] This
option overrides these safeguards"*) deletes the registration and immediately
creates a new one at the same id, and `git worktree move <src> <stale dst>` does
the same to the destination entry. `remove` is the one that only removes.

## 3. Dropping one registration from the host

Given a registration whose recorded path is a container path:

- **`git worktree remove <recorded-path>` is the supported per-registration
  operation.** Section 2. Honours the lock, refuses on a collision, drops nothing
  else, deletes no working tree that is not there.
- **`git worktree prune` is the repository-wide operation.** On a host it drops
  *every* container-registered worktree at once, plus the three states
  `worktree list` cannot show, plus any `duplicate entry`.
- **`git worktree repair` is not relevant and cannot be made to be.** [doc]:

  > `repair [<path>...]`::
  >
  > Repair worktree administrative files, if possible, if they have become
  > corrupted or outdated due to external factors.
  > +
  > For instance, if the main worktree (or bare repository) is moved, linked
  > worktrees will be unable to locate it. Running `repair` in the main
  > worktree will reestablish the connection from linked worktrees back to the
  > main worktree.
  > +
  > Similarly, if the working tree for a linked worktree is moved without
  > using `git worktree move`, the main worktree (or bare repository) will be
  > unable to locate it. Running `repair` within the recently-moved worktree
  > will reestablish the connection. If multiple linked worktrees are moved,
  > running `repair` from any worktree with each tree's new `<path>` as an
  > argument, will reestablish the connection to all the specified paths.

  Repair has two directions, and the guard that matters is in the source:
  `repair_gitfile()` carries the comment `/* missing worktree can't be repaired
  */` above `if (!file_exists(wt->path)) goto done;`, and
  `repair_worktree_at_path()` reads the registration's id **out of a `.git` file
  at the path it was given**. So repair can never be told "registration `<id>`
  now lives at `<somewhere>`"; it can only be told a path and asked to look.

  **[exp]** With a container-path registration present, `git worktree repair`
  with no arguments is a no-op: exit 0, no output, the `gitdir` file unchanged.
  Given the container path as an argument it refuses (`error: not a valid path:
  /workspaces/...`, exit 1, on 2.51.1 and 2.43.0; `fatal: Invalid path
  '/workspaces': No such file or directory`, exit 128, on 2.34.1 and 2.30.2) and
  changes nothing. **Repair never deletes a registration**, on any version, in
  any invocation: its only mutating call is `write_worktree_linking_files()`.
  It is the adopt-a-new-location command, driven from the worktree's side, and
  there is no worktree on this side to drive it from.
- **`git worktree move` is likewise out.** [exp] `git worktree move
  /workspaces/... <host path>` fails validation identically on all four:
  `fatal: validation failed, cannot move working tree: '/workspaces/.../.git'
  does not exist`. There is no rewrite-then-remove route.
- Deleting `<clone>/.git/worktrees/<id>/` by hand works and is what prune does
  internally, but it is not a documented interface, and there is no reason to
  reach for it now that `remove` is known to work. The only manual edit the docs
  sanction is the opposite direction, writing `gitdir` to follow a moved
  worktree, and they immediately recommend `repair` instead.

## 4. What a lock is

**[doc]** The whole of what git says a lock means. `COMMANDS`:

> `lock`::
>
> If a worktree is on a portable device or network share which is not always
> mounted, lock it to prevent its administrative files from being pruned
> automatically. This also prevents it from being moved or deleted.
> Optionally, specify a reason for the lock with `--reason`.
>
> `unlock`::
>
> Unlock a worktree, allowing it to be pruned, moved or deleted.

`DETAILS`:

> To prevent a `$GIT_DIR/worktrees` entry from being pruned (which
> can be useful in some situations, such as when the
> entry's worktree is stored on a portable device), use the
> `git worktree lock` command, which adds a file named
> `locked` to the entry's directory. The file contains the reason in
> plain text.

`gitrepository-layout`:

> `worktrees/<id>/locked`::
>   If this file exists, the linked working tree may be on a
>   portable device and not available. The presence of this file
>   prevents `worktrees/<id>` from being pruned either automatically
>   or manually by `git worktree prune`. The file may contain a string
>   explaining why the repository is locked.

**Git nowhere says a lock means the worktree is in use, live, or being worked
in.** Every documented rationale is removable or unavailable media, and the one
place git gestures at another use ("which can be useful in some situations") does
not name one. The documented semantics are exactly three: it blocks prune, it
blocks move, it blocks remove.

If anything, git's wording argues the opposite of liveness: a lock says "this
path being unresolvable is not evidence that this registration is garbage",
which is precisely the host's situation for every container-registered worktree,
lock or no lock.

**[doc][exp]** What it protects against, exhaustively. `worktree_lock_reason()`
is called from `builtin/worktree.c` and nowhere else in git; the `locked` file is
additionally tested inside `should_prune_worktree()`, which is reached from
`git worktree prune` and from `builtin/gc.c`.

| command | locked worktree |
| --- | --- |
| `git worktree prune`, and `git gc`'s call to it | spared, unconditionally, no command-line override |
| `git worktree remove` | `fatal: cannot remove a locked working tree, lock reason: <r>` / `use 'remove -f -f' to override or unlock first`; `-f -f` proceeds |
| `git worktree move`, and an `add`/`move` **destination** that is a stale locked entry | refuses; `-f -f` proceeds |
| `git worktree list` | reports it, and suppresses `prunable` (section 5) |
| **anything that deletes the containing directory** | **not protected** (section 1) |
| `checkout`, `switch`, `branch -d`, `status`, `commit`, `rebase`, `fsck` | do not consult it at all. A lock is not a write lock |

**[doc]** Two edges: the main worktree can never be locked
(`worktree_lock_reason()` returns NULL for it), and `add` itself writes a
`locked` file while it sets a new worktree up, with the comment *"lock the
incomplete repo so prune won't delete it, unlock after the preparation is
over"*. So git's maintainers do treat the file as a general "do not reap this"
flag, they just have not written that down as a contract.

**So devlaunch's "locked is sacred" is devlaunch's policy.** #426's Ask 2 and its
sharpening comment already say the right thing: a lock is set by the agent
harness as a courtesy, a killed session leaves one behind, and it is neither
necessary nor sufficient for "in use right now". Git's contract supports the
mechanism and is silent on the meaning. Honouring it is a choice about whose
convention to trust, and there is nothing in git to appeal to for either the
"lock implies live" reading or against it.

## 5. Is `--porcelain` enough on its own?

**No, in three independent ways.**

**[doc]** The format, in full:

> The porcelain format has a line per attribute.  If `-z` is given then the lines
> are terminated with NUL rather than a newline.  Attributes are listed with a
> label and value separated by a single space.  Boolean attributes (like `bare`
> and `detached`) are listed as a label only, and are present only
> if the value is true.  Some attributes (like `locked`) can be listed as a label
> only or with a value depending upon whether a reason is available.  The first
> attribute of a worktree is always `worktree`, an empty line indicates the
> end of the record.

with seven labels and no others: `worktree <path>` (always first), `bare`,
`HEAD <oid>`, `detached`, `branch <ref>`, `locked [<reason>]`,
`prunable <reason>`. `bare` excludes the `HEAD`/`detached`/`branch` group.
`prunable` always carries a reason; `locked` may not. Without `-z` the lock
reason is c-quoted per `core.quotePath`.

**[doc]** Versions: `locked` reached the human format in **2.30.0**, both
`locked` and `prunable` reached `--porcelain` in **2.31.0**, and `-z` arrived in
**2.36.0** as the fix for unquotable paths and reasons. **[exp]** 2.30.2 prints
neither annotation in porcelain, so on that version the answer to this whole
section is a flat no.

**[exp]** The four states asked about, mapped onto the output:

| state | porcelain says | distinguishable? |
| --- | --- | --- |
| registered and present | `worktree <path>`, `HEAD`, `branch`/`detached` | only by the *absence* of the other two labels |
| registered and absent | ... plus `prunable gitdir file points to non-existent location` | yes, if unlocked |
| locked (present) | ... plus `locked <reason>` | not from locked-and-absent |
| locked (absent) | ... plus `locked <reason>`, **and no `prunable` line** | **no** |
| prunable | `prunable <reason>` | yes, unless also locked |
| registration with a missing, empty or unreadable `gitdir` file | **nothing. Not listed.** | **no** |

**`locked` masks `prunable`.** `worktree_prune_reason()` delegates to
`should_prune_worktree()`, whose lock check precedes every path check, so a locked
registration never carries a `prunable` line whatever the state of its path.
Porcelain output for locked-and-present is byte-identical to locked-and-absent.
To learn whether a locked registration's directory is there, something has to
look at the filesystem.

**Three states are pruned and never listed.** Section 2. No amount of parsing the
listing reveals them, so a sweep that reasons only over the listing has a blind
spot that `prune` acts inside.

**And `prunable` is a claim about one filesystem entry, not about the worktree.**
[exp] The check is whether the recorded `<path>/.git` exists as a filesystem
entry, so:

- A directory that exists and holds gigabytes but has no `.git` reads
  `prunable gitdir file points to non-existent location`. Pruning it strands the
  directory.
- A recorded path whose `.git` is a real repository directory, or a gitfile
  pointing at a *different* admin directory, reads with **no annotation at all**,
  as if healthy.
- A `HEAD` and `branch` line are emitted for an absent worktree too, because they
  are read from `worktrees/<id>/HEAD` inside the admin area, which outlives the
  working tree. A `HEAD` line is not evidence the path exists.
- `prunable` depends on `--expire`, which defaults to `TIME_MAX` for `list`. Pass
  one and a recently-touched absent registration silently loses the annotation.

`prunable` means "git would drop this registration", which is what the docs say
it means. It does not mean "there is nothing at that path", and on a host it is
true of every container-registered worktree at once.

## What this settles for the unit question

Not a design, but these are the facts #445 has to be consistent with.

- **The unit cannot be "the clone", because a per-registration removal exists.**
  `git worktree remove <recorded path>` is the operation. It honours locks,
  refuses on collisions, drops nothing that was not named, and works on the
  container paths a host sees. The all-or-nothing gate around
  `git worktree prune` is a workaround for a constraint that is not there.
- **The unit cannot be "a directory the plan enumerated"**, because prune reaches
  three registration states no listing shows and can drop a healthy registration
  as a duplicate, and because a registration and a directory are independent
  things: either exists without the other.
- **Nesting is not a unit git has.** The parent-child relation must be derived
  from path containment, nested registrations are siblings that can be acted on
  in any order, and a nested worktree belonging to a *different* repository is
  invisible to this clone entirely.
- **Nothing git offers makes a nested worktree safe from its parent's removal.**
  Not a lock, not the untracked check, not the absence of `--force`. That
  guarantee has to come from devlaunch deciding a parent's fate conjunctively
  over its children, which is what T1 already proposed.
- **A lock is worth honouring and worth not trusting**, and git supports both
  halves of that: the mechanism is real and unoverridable in prune, and the
  meaning is undefined.
- **Two claims in the tree are false and should go with the fix.** The doc
  comment on `Git::worktree_prune` ("git offers no way to drop one registration
  and keep another"), and `docs/cleanup.md`'s argument that nested worktrees can
  be excluded from a parent's dirty check because they are reasoned about
  separately, which for a nested worktree inside a going parent is not true.
