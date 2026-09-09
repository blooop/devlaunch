# The command line, in full

[README](../README.md) has the commands you need. This page is the rest: how the
selector decides what you picked, which verb refreshes git state and which only
touch the container, what a pull request link resolves to, which commands get a
terminal, what `--rm` promises and where
it stops, which exits fire it, the spellings that were retired and what they say
now, which full-auto flag `aid` starts each agent with and why codex gets the one it
gets, what `aid`'s Remote Control default starts and how to turn it off, what
`kill` does to a workspace that will not answer, and what happens when devpod is
missing, will not answer, or injects the wrong agent binary.

## The selector

The selector is built in. There is no `fzf` on `PATH` and no `iterfzf`, which is
why there is nothing to install for it, and why `dl` with its input redirected
away from a terminal simply declines to open one.

It is a table. Each row is `<owner> | <repo> | <branch>`, under headings that say so:

```
Select workspaces (type to filter, TAB to mark several):
OWNER  | REPO             | BRANCH
blooop | devlaunch        | main
blooop | wayfinder        | wayfinder/devlaunch-467
myfork | bencher          | fix/thing
-      | someones-project
```

The owner and the repo are read off the clone's place in dl's layout,
`<cache>/repos/<owner>/<repo>/<id>`, and the branch is read out of the clone's own
`HEAD`. The hashed suffix does not appear: it is there to keep two branches from
sharing an id, and reading it is no part of choosing a workspace.

Every cell is padded to its column's widest entry, the heading counted as one of
them, so a column starts in the same place on every line. The headings are drawn in
the picker's header rather than offered as a row, which is what keeps them from
being filtered away, marked with TAB or picked. A column is named exactly when some
row has something in it, so a list of nothing dl cloned is headed `OWNER | REPO` and
stops there.

**The branch is the one checked out now**, not the one the workspace was made for,
so a `git switch` inside a container shows up here. The columns used to be recovered
by taking the id apart instead, which could only answer with a slug: `feature/auth`
read as `feature-auth`, indistinguishable from the branch of that name, and a long
one read short. Reading `HEAD` costs one small file and answers exactly.

The row is still three columns rather than `owner/repo@branch`, because that reads
like something you could retype and the picker is not a place to retype anything. To
act on what you picked, pick it.

Two rows can still be drawn alike, when two workspaces of one repository sit on one
branch. The id-scheme migration leaves exactly that pair for a while: the renamed
clone under its new id, and the container still on the old one. When it happens
**both** rows gain a fourth column holding their whole id, and the table gains the
heading for it:

```
OWNER  | REPO      | BRANCH | WORKSPACE
blooop | devlaunch | main   | devlaunch-main-3j1t
blooop | devlaunch | main   | devlaunch-main-legacy
```

The row's own text is how `dl` knows which workspace you picked, so two rows reading
the same would be one workspace deleted in place of another, and the id is what
settles it. It is appended rather than replacing the columns, and that is the
correction to what this used to do: both rows collapsed into their bare ids, which
took the branch off screen to fix an ambiguity the branch never caused. Both of those
rows are on `main`. Picking between two ids is harder than picking between two rows
that say `main` and carry a tiebreak, and `dl rm` is one of the verbs this opens for.

Only the rows that collide grow the column. A third workspace of the same repository
on another branch keeps its three.

A row whose clone is not on disk shows its whole id in the repo column and stops
there, since there is no `HEAD` to read and so no branch to draw. That is the honest
answer: the workspace's source is gone. It sits in the column rather than running on
past it, which is what puts every row on one grid, and the price is that a long name
widens the repo column for the rows around it. `REPO` heads it either way, and that
is the one place the heading is looser than the cells under it.

Nothing here is short of room, and that is why the picker spends it differently from
[the terminal tab](workspace-tools.md#naming-the-terminal-after-the-workspace). A row
is read one at a time down the terminal, so the branch is spelled in full, slashes and
all. A tab is a handful of characters read at a glance next to a dozen others, so it
takes the truncated slug and drops the suffix. Same workspace, two jobs.

## What a verb does not refresh

Every verb on this page acts on a container. **`rm` is the only one that refreshes
git state**, because it deletes the clone along with the workspace and the launch
after it is a cold one. `restart`, `recreate` and `reset` all reach a workspace
devpod already knows, and that path runs no git at all: no fetch, no ref update,
no checkout. So a container can be rebuilt repeatedly from a checkout that never
moves, and `reset` in particular is a clean slate for the container and its
volumes rather than for the code in it.

`dl` reports the part of that you cannot see. Launch `owner/repo@branch` against a
workspace devpod already has and, when the checkout is behind the `origin/<branch>`
that clone last fetched, the attach says how far behind before it hands over the
shell. [How fresh a launch is](workspaces.md#how-fresh-a-launch-is) is the whole
of the freshness rules, and the section under it names which verb moves what.

## A pull request where a branch goes

A review request arrives as a link, not as a branch name, and turning one into the
other means opening the page and copying a string out of it. So the link is
accepted wherever `owner/repo@branch` is accepted, in three spellings:

```bash
dl blooop/devlaunch@https://github.com/blooop/devlaunch/pull/579
dl https://github.com/blooop/devlaunch/pull/579
dl blooop/devlaunch@#579
```

All three mean the same thing, and so does every verb form built on them:
`dl <pr-link> stop`, `dl <pr-link> code`, `dl <pr-link> -- pytest`.

**It resolves to a spec and then stops being a pull request.** `dl` asks `gh`
which branch the request's head is, rewrites what you typed into
`owner/repo@branch`, prints the line saying so, and hands that on. Everything
downstream is the ordinary branch launch: the same derived id, the same clone
directory, the same metadata record, the same row in `dl --ls`, the same thing
`dl <ws> rm` removes. Two people opening one branch, one by its link and one by
its name, are in the same workspace, and they are because there is only one spec
left by the time anything looks.

That rewrite happens once, before anything reads the target word, which is what
makes `dl <pr-link> --rm` safe. That form resolves its target twice, on the way in
and again on the way out, and a second lookup could answer differently: a request
can be force-pushed or retargeted in between. One rewrite means the removal and
the launch cannot disagree about which workspace they meant.

**A request from a fork opens the fork.** The rewrite names the repository the
*branch* is in, which for a fork request is not the repository whose URL you
pasted. Checking that branch out of the base repository would find nothing, or
find an unrelated branch of the same name, so `dl` clones the fork and names the
workspace after it. The line it prints says which repository it picked, and that
is the case it exists for.

**`gh` is required, and only for this.** Nothing but the GitHub API can say which
branch a request number is. A host without `gh` gets a refusal saying to name the
branch instead, which is the remedy that needs nothing installed. Nothing else in
`dl` depends on `gh` being there: token forwarding is best effort and falls back
to opening a workspace with no GitHub login.

**Three refusals, before anything is launched.** A spec that names two different
repositories, `a/b@https://github.com/c/d/pull/5`, is refused rather than resolved
either way, because both readings are defensible and a wrong guess opens a
repository you did not ask for. A reference with no usable number, `.../pull/abc`,
is refused as a malformed reference rather than passed on as a URL, which is what
kept the old failure obscure: devpod would report, truthfully, that it could not
clone `.../pull/579`. And a head branch `dl` cannot spell in a spec, one with an
`@` in it, is refused rather than rewritten, since a rewrite that re-reads as some
other spec opens the wrong workspace instead of failing.

Two spellings deliberately do **not** work. A bare `owner/repo@579` stays a branch
name, because `579` is a legal branch name and somebody has one. And a GitHub
Enterprise URL is not recognised, because resolving a number means asking a host
about it, and guessing that an unfamiliar host speaks GitHub's API is worse than
not matching.

A merged or closed request still resolves, and `dl` says which it is before the
checkout runs. GitHub deletes the head branch of most merged requests, and
"couldn't find branch" is a confusing way to be told that something landed.

## What `--` takes

The command and its arguments, one word each. `dl` quotes every word on the way
into the remote payload, so a quoted argument stays one argument: `dl <ws> --
claude 'fix the bug'` runs `claude` with a single argument, and a word holding a
space, a `#`, a `$(...)` or a backtick is that word rather than shell syntax.

It has to be built that way because the payload is one `bash -lc <line>` for both
transports. The words used to be rejoined with plain spaces, which gave the remote
shell back every separator your own shell had already consumed: quoted arguments
were re-split, a `#` commented out the rest of the line, and a `$(...)` ran.

One exception, and it is deliberate. The quoting leaves a word alone when it needs
none, and `=` counts as needing none, so a leading `NAME=value` still reaches the
shell as an assignment prefix and sets that variable for that command only. That
is what makes `dl <ws> -- IS_SANDBOX=1 claude ...` work, which is the spelling
`aid` uses and the one the README shows.

The exception ends where the quoting begins, and it ends abruptly. It is the
whole word that has to need no quoting, value included, so `FOO=bar` is an
assignment and `FOO='a b'` is not: the value's space makes the word
`'FOO=a b'`, and a shell reads a quoted word as a program name, so the command
exits 127 with the variable never set. Values made of `[A-Za-z0-9_@%+=:,./-]`
are the ones that survive. For anything else, name the shell and write the
assignment inside it: `dl <ws> -- bash -lc 'FOO="a b" cmd'`.

A shell snippet is a command like any other, so name the shell: `dl <ws> -- bash
-lc 'a && b'`. Redirections and pipes typed on your own command line belong to
your own shell and never reach `dl`, which is what makes `dl <ws> -- ls >
files.txt` write the file here.

## Commands that need a terminal

`dl <ws> -- <command>` gives the command a terminal whenever `dl` itself has one,
so interactive programs start and stay up instead of exiting immediately. A coding
agent, `htop`, `git rebase -i`, a REPL. Redirect the output and the terminal
goes away again, so `dl <ws> -- ls > files.txt` stays free of escape sequences.

This needs the ssh host alias `devpod up` writes. Where it writes it is devpod's
choice and `dl` follows it: `SSH_CONFIG_INCLUDE_PATH` from devpod's context
options, else `$DEVPOD_SSH_CONFIG`, else the `SSH_CONFIG_PATH` context option,
else `~/.ssh/config`. devpod writes to whichever of those it picks and to no
other, so a host that exports `DEVPOD_SSH_CONFIG` has no `~/.ssh/config` for `dl`
to read. The context options are read from the copy `dl` has already cached, never
by asking devpod again, because that question costs more than the terminal it
decides.

That same file is then handed to OpenSSH as `-F <path>`, because OpenSSH reads
none of the above: it resolves `~` through `getpwuid`, so the config `dl` decided
from has to be named on the command line or the alias does not resolve at all.
One consequence worth knowing: a session over this transport is built from that
file alone. `/etc/ssh/ssh_config` never applies, because naming any file with `-F`
makes OpenSSH skip the system config, and that holds even when the file named is
your own `~/.ssh/config`. A `Host *` block of your own applies only while devpod
publishes into the file `dl` names, so it drops out as soon as devpod publishes
elsewhere.

If a workspace has no alias, `dl` says so and falls back to the plain `devpod ssh`
transport, which has no terminal; `dl <ws> restart` republishes the alias. If
there is no ssh config at all, `dl` says that instead and names the file it looked
in, and the advice it gives is qualified: a restart publishes there only if devpod
writes to that same file, so a notice that comes back means `DEVPOD_SSH_CONFIG` or
one of devpod's ssh-config context options names a different one. Set
`DEVLAUNCH_NO_TTY=1` to force the fallback everywhere.

### Getting the terminal back when a session dies badly

A terminal is not only a stream. A full screen program switches modes on in the
emulator for its own use, the kitty keyboard protocol, bracketed paste, mouse
reporting, the alternate screen, and is expected to switch them off again on the
way out. One that is killed never gets the chance, and those modes live in your
terminal rather than in the connection, so nothing between the program and the
glass undoes them.

That is the failure you see when a container goes away underneath a live session.
devpod reports `error tunneling to container: exit status 137`, the agent inside
dies unceremoniously, and what is left behind is baffling rather than obviously
broken: ssh restores the tty settings on its way out, so the shell still echoes
and still edits lines, and yet Ctrl-C does nothing and ordinary keys print things
like `9;133u` at the prompt. Those are kitty keyboard protocol key reports, still
switched on. Ctrl-C is one of them, arriving as an escape sequence instead of as
the byte that raises SIGINT.

`dl` is the last process holding that terminal, so `dl` repairs it. Every session
ends with a short restore written to the terminal, over either transport, and the
interrupt handler writes the same thing before it exits. It goes out after a
clean session too, because ssh's exit status does not say whether the far end
cleaned up after itself. That costs nothing: every sequence in it is chosen for
doing nothing when the mode it names is already off. The one obvious candidate
that is not, `ESC [ ? 1049 l`, is left out on purpose, because it restores the
cursor as well as the screen and would scramble the display on every ordinary
exit.

Nothing is written when `dl`'s own output is not a terminal, so `dl <ws> -- ls >
files.txt` keeps its output free of escape sequences. If you are ever left with a
wrecked terminal some other way, `printf '\033[<u'` undoes this particular one
and `reset` undoes everything.

### `--rm`: the throwaway workspace

`--rm` deletes the workspace once the session ends, the way `docker run --rm` does. It
applies to the two forms that hand a session over and come back from it:

```bash
dl kinisi/repo@fix/x --rm                # shell; the workspace goes when you exit
dl kinisi/repo@fix/x --rm -- make test   # one command, then the workspace goes
aid kinisi/repo@fix/x 'fix the flaky test' --rm
```

**The word and the flag are docker's two commands, not two spellings of one.**
`docker rm` deletes a container now; `docker run --rm` deletes one when what it ran
has finished; and no docker subcommand takes a `--rm` meaning the first of those. Here
too: `dl <ws> rm` deletes now, `dl <ws> --rm` deletes after, and neither has to be read
twice to work out which was meant. `--force` follows docker as well. It belongs to
`dl <ws> rm --force`, never to `--rm`.

**It stops at work that is nowhere else.** The removal is `dl <ws> rm`'s, guard included,
so a clone holding uncommitted or unpushed work, or one git could not read to find out,
refuses, says which, and leaves the workspace standing:

```
--rm: the session has ended, removing kinisi/repo@fix/x.
kinisi-repo-fix-x-1a2b holds 1 uncommitted change(s) (scratch.txt). Push or commit it,
or run: dl kinisi/repo@fix/x rm --force
```

That is what makes it safe to leave on a line you recall: the flag never decides that
your work was disposable. For the same reason `--force` does not compose with it. A
`--force` habitually appended to a recalled `--rm` line would destroy work hours
later, unattended, with nobody reading the sentence explaining it. Run
`dl <ws> rm --force` when that is what you mean.

**A build that failed is collected too.** The removal runs whenever the launch got as
far as asking devpod for the workspace, including when `devpod up` died in
`postCreateCommand`, which leaves the container *running* and the clone cut. That is
the case an unattended `dl owner/repo --rm -- make test` in CI most needs covered.
A launch that stopped earlier, an unknown workspace, a branch that could not be named,
a devpod that would not run, created nothing, so nothing is removed and nothing is
said about it.

Three more things it does not promise:

- **The exit code is the launch's.** `dl repo --rm -- make test` exits with the
  test's status, and a failed build exits with devpod's; a removal that refused is
  never what the code reports. The refusal is on stderr and the workspace is still
  there.
- **It is best-effort, by construction.** Ctrl-C out of a session is *not* one
  of the gaps, though. See "How you exit decides whether it fires" below.
- **It does not know about your other shells.** Nothing serialises two sessions on
  one workspace, since the launch lock covers the build rather than the session, so a second
  `dl <ws>` in another terminal is attached to the same container, and the `--rm`
  run exiting first removes it from under that one. Use `--rm` for the workspace
  you opened to throw away, not for one you may already be sitting in elsewhere.

On an `aid` line it is **appendable**, and it keeps the prompt: recall the line, type
`--rm` at the end, and the agent still runs, with the workspace going when it is done. That
is the shape a shell makes cheap, appending to the previous line rather than editing
the front of it. Note that a `--` command tail is not appendable this way. Everything
after `--` belongs to the workspace's command, so a `--rm` typed there is an argument
to that command.

### How you exit decides whether it fires

The removal runs when `dl` gets control back, so what matters is whether your exit ends
the session or kills `dl`.

**Ctrl-C out of the program you were running: fires.** Both session transports allocate
a pty. A bare `dl <ws>` runs `devpod ssh <id>`, and `dl <ws> -- <cmd>` on a terminal
runs `ssh -t`. That puts your local terminal in raw mode and clears `ISIG`, so Ctrl-C is
a byte travelling to the remote pty rather than a signal to `dl`: the program *inside* the
container gets the interrupt. So `aid repo 'fix it' --rm` and Ctrl-C twice to leave
Claude Code ends the remote command, ends the session, and the workspace goes. In an
interactive shell Ctrl-C just hands you a fresh prompt, and `exit` or Ctrl-D is what ends
that session, either of which fires the removal.

**These do not fire**, because `dl` itself takes the signal and its handler cannot run a
removal (a signal handler may not allocate or lock, and this one `_exit`s):

- Ctrl-C during the clone or the container build, before any pty exists.
- `kill <dl>` from another shell, and a supervisor or CI runner cancelling the job.
- Closing the terminal window.

What all three *do* run is the cleanup the removal is not: the staged plaintext
`GH_TOKEN` file is unlinked and the `devpod up` child is killed, so none of these three
leaves a credential on disk or a build running behind you. The one exception is a run
whose SIGTERM was disarmed before it started. The drain fells the build with a
`killpg(…, SIGTERM)`, so disarming that signal disarms its own reach into the child too.
Ctrl-\ (SIGQUIT) is not one of them and still does mean "die now and dump core", where
tidying up first is not what it asks for. The workspace is what stays, still there
under its name, and `dl <ws> rm` is how it goes.

They are told apart by the exit code, which is **128 + the signal number**: 130 for
Ctrl-C, 143 for a `kill`, 129 for a closed terminal.

Two of the three can be switched off in the ordinary way, and one cannot. If a SIGTERM
or a SIGHUP was **already set to be ignored** when `dl` started, which is what
`nohup dl …` does to SIGHUP, that stays ignored and ends nothing, so `nohup` still
outlives the terminal it was started from. **Ctrl-C is not switchable like that**, and
that is deliberate rather than an omission: a shell script backgrounding a job (`dl … &`)
hands its child an ignored SIGINT whether or not anyone wanted one, so honouring it there
would quietly stop the cleanup for every `dl` run from a script or a CI step. Ctrl-C
behaves exactly as it always has.

One-line check for your own setup: start `dl <ws> --rm` and press Ctrl-C once. A
fresh prompt *inside* the container means Ctrl-C is being forwarded and the removal will
fire when you leave. Landing back on the host means it reached `dl`, and it will not.

Those two forms and no others. Every verb word refuses the flag rather than ignoring it,
and `code` is the one worth knowing about: it returns while VS Code is still connecting,
so honouring `--rm` there would delete the container out from under a window that is
still opening. `restart`, `recreate` and `reset` do end in a session and would work, but
they are out too, because `--rm` is the throwaway workspace and not a cleanup modifier
on every verb that ends in a shell.

### `rme`: the delete, and then the shell

`dl <ws> rme` is the `rm` verb with one thing added at the end: on a removal that
worked, it sends SIGHUP to whatever started `dl`. For the line it exists for that is
an interactive shell, so the shell ends and the terminal tab it was sitting in closes
on its own.

```bash
dl blooop/devlaunch@fix/x rme        # delete it, and the tab goes with it
dl rme                               # pick, TAB to mark several, then the tab goes
```

It is for the tab opened for one workspace. The delete is a container teardown, which
is seconds and sometimes rather more; the `exit` after it is a keystroke you are only
there to type. `rme` is the pair as one word.

**The removal is `rm`'s, and so is everything that can stop it.** Same guard, same
`--force`, same refusals, same exit codes. The hangup is reached only when the removal
itself came back clean, and the reason is the one thing the terminal is still needed
for: every way this can go wrong writes a sentence to stderr, and closing the window
that sentence was written to is a guaranteed way for nobody to read it.

**`--force` is the one exception, and it is `rm --force`'s hazard with the receipt's
reader removed.** `--force` passes devpod's own `--ignore-not-found`, so a workspace
that was never there counts as deleted, and a `./path` target is resolved without
asking devpod anything. So `dl ./wrong-directory rme --force` deletes nothing,
succeeds, and closes your terminal, taking with it the `Workspace <id> is gone.` line
that `rm --force` prints instead of `Removed` for exactly this reason. It is left
standing rather than special-cased: absence is what `--force` asks for, and the
ordinary forced run is a real workspace whose uncommitted work you have decided
against, which is the run most in need of the tab closing. Type the path carefully,
or drop `--force` and let the guard resolve it.

```
$ dl devlaunch-dirty rme
devlaunch-dirty holds 1 uncommitted change(s) (scratch.txt). Push or commit it,
or run: dl devlaunch-dirty rme --force
$                                        # still here, and so is the workspace
```

A batch is one hangup. `dl rme` with five rows marked removes all five and then hangs
the shell up once, when the last of them has gone, which is the wait the verb saves
most of.

**What it hangs up is `dl`'s parent process, whatever that is.** There is no way to
ask whether a parent owns a terminal, so `dl` does not guess: it signals the process
that started it and names the pid on the way past.

```
Hanging up the shell dl was called from (pid 48213).
```

**Which process that is depends on the shell, not on the line**, which is the reason
the pid is printed at all. A shell running a single command in a subshell usually
*replaces* the subshell with it rather than forking, so `$(dl <ws> rme)` signals the
shell that typed the line and your terminal closes after all. Write the same line
with a redirection, or with a `VAR=x` prefix, and the subshell survives to take the
signal instead. Both measured, on bash 5 and dash. In a script it is the script's
shell that goes, and your terminal is untouched.

Two endings print instead of the signal, and both say the removal is done: a parent
that had already exited by the time `dl` looked prints `rme: dl's parent process has
already gone, so there is no shell to hang up. The removal is done.`, and a signal
the OS refused prints the pid and the error beside it.

**`nohup dl <ws> rme` is refused outright**, and prints `rme: SIGHUP was already
ignored when dl started, so the shell stays. The removal is done.` A `nohup` sets
SIGHUP to `SIG_IGN` and `exec`s in place, so the parent `rme` would signal is the
terminal `nohup` was typed to outlive. `dl` already honours that inherited ignore
for its own signal handling, which is what lets a `nohup dl …` survive a closed
window at all (see [How you exit decides whether it
fires](#how-you-exit-decides-whether-it-fires)), so sending the signal it refuses to
act on would be `dl` arguing both sides. The removal still happens.

One thing to know before reaching for it. A shell hung up this way takes its
background jobs with it, because that is what a shell does on SIGHUP. Use `rme` in
the tab that has nothing left in it, which is the tab it was written for, and `rm` in
the one you are still working in.

### `--stop` and `--autorm` are retired

Both moved because `--rm` changed meaning, and both are still recognised so that a
line recalled from history says what happened instead of quietly doing something else:

```
$ dl <ws> --autorm
--autorm is now spelled --rm: 'dl <workspace> --rm' opens the workspace and deletes it
when the session ends, the way 'docker run --rm' does. Use 'dl <workspace> rm' to
delete one now.

$ dl <ws> --stop
--stop is no longer a flag: the flag spellings now modify a session (--rm deletes the
workspace once one ends) rather than name a verb. Use 'dl <workspace> stop' to stop a
workspace.
```

`--autorm` is a rename and nothing else. The behaviour above is what it always did.

`--stop` is a genuine withdrawal, and so is the thing `--rm` used to do. Both were the
*suffix* form of a verb, appended to a line that already asked for something, and
winning over it, so that `aid <ws> 'review this pr' --rm` deleted the workspace and
printed `--rm overrode the rest of the line`. That shape cannot survive `--rm` meaning
"delete when the session ends": the two spellings look alike, and one cancelling the
line while the other runs it is the one pair a person cannot keep straight.

What replaces it, for "I am done with this workspace":

```bash
dl <ws> rm            # the workspace named
dl rm                 # or pick it; TAB marks several, and rm takes each in turn
```

For a long `aid` prompt line that is the cheaper edit anyway: `dl rm` and a pick
beats recalling the line to type at the end of it. What is genuinely gone is deleting
a workspace *without naming or picking it*, by appending to whatever the last line
happened to be.

### `prune` is no longer a spelling of the `rm` verb

`dl <ws> prune` used to delete one workspace and `dl --prune` removes clone
directories and no workspace at all. One word, two unrelated commands, told apart
by two dashes. Reach for the wrong one and you either lose a workspace you meant to
keep or get refused for a reason the message could not explain
(`--prune takes no workspace: it is not a workspace command.`). So the verb spelling
is gone, and typing it says what to use instead:

```
$ dl <ws> prune
'prune' is no longer a workspace verb. Use 'dl <workspace> rm' to delete a workspace,
or 'dl --prune' to remove the clone directories no workspace opens any more.
```

`dl --prune` is unchanged. The word is still *recognised* rather than forgotten, so it
is never read as a workspace name. `dl prune <ws>` says what moved instead of
reporting an unknown workspace called `prune`, and a workspace that really is called
`prune` is still reachable as `dl stop prune`. Use `dl <ws> rm` from now on.


### Where `--force` may sit

`--force` is read in one position only: after both the workspace and the verb, which
is where every example on this page writes it. The two slots ahead of it are already
spoken for, so a `--force` that lands in either is read as the word that belongs
there and refused:

```
dl <ws> --force rm   ->  Unknown command '--force'.        (exit 1, nothing deleted)
dl --force <ws> rm   ->  Unknown workspace '--force'.      (exit 1, nothing deleted)
```

**Past the verb it is read, but only `rm` and `rme` do anything with it.** Every
other verb takes the flag and drops it: `dl <ws> up --force`, `dl <ws> stop --force`
and `dl <ws> recreate --force` all run exactly as they would without it, silently.
That is the one place this grammar discards a word rather than refusing it, and it is
worth knowing because the two halves of the rule read as though they were one: a
misplaced `--force` refuses, a meaningless one does not. `kill` is the case to
remember, since it is the verb whose whole point is going ahead anyway. It has no
`--force` to type and ignores one offered.

**The selector form cannot be forced at all.** `dl rm` with no workspace opens the
picker, which leaves no slot after the verb, so `dl rm --force` is the verb-slot
refusal above. There is no spelling of "pick some workspaces and force the removal";
name the workspace, or answer the refusal the guard prints. The diagnostic's
suggestion (`dl rm -- --force`) is the generic one for an unknown verb word and would
run `--force` as a shell command, which is not what anybody typing that meant.

**A global command has no slots, so placement stops mattering there.**
`dl --force --prune` and `dl --prune --force` are one line, and the same holds for
every other global. Note this is about the literal word `--force`: `--devcontainer`,
`--force-worktrees` and `-y` have rules of their own.

Pinned by `force_deletes_only_where_it_follows_both_the_name_and_the_verb` and
`a_globals_force_reads_the_same_wherever_it_sits` in `rust/dl/src/cli.rs`, and by
`force_after_the_verb_still_deletes` with its two neighbours in
`rust/dl/tests/grammar.rs`. The two refusals quoted above are a second copy of
strings `render.rs` and `lib.rs` own, so
`the_force_placement_section_quotes_the_refusals_it_says_it_does` reads this
section back and diffs them against what the binary prints.

## Full auto: every agent, every launch

`aid` exists to hand a repo to an agent and walk away, so every agent it starts is
started in that agent's full auto mode. There is no flag to type and no flag to
type differently per agent:

| Agent | Full-auto flag |
| --- | --- |
| `claude` | `--dangerously-skip-permissions`, with `IS_SANDBOX=1` beside it |
| `codex` | `--dangerously-bypass-approvals-and-sandbox` |
| `gemini` | `--yolo` |

The flag and not the whole command line, which is longer than one column: a default
`claude` launch also carries `CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1` and a
`--remote-control=<workspace>`, and gemini takes its initial prompt through
`--prompt-interactive`.

One rule, three spellings, and the table in `rust/aid/src/rewrite.rs` is where they
live. `every_agent_starts_in_full_auto` holds the rule against that table rather
than against one row of it, so an agent added without its flag fails a test instead
of stopping someone's unattended run to ask about its first edit.

Two of those rows have a reason worth reading.

**codex gets the bypass, not `--full-auto`.** codex offers both and only one of them
is this, for two reasons of which the first is the one that matters: `--full-auto`
still escalates to a person. It is an approval policy plus a sandbox rather than an
absence of approvals, so an unattended run stops and asks, which is the whole of
what this rule exists to prevent. Only `--dangerously-bypass-approvals-and-sandbox`
sets the policy to never ask. The second reason is the sandbox `--full-auto` keeps:
workspace write with the network off, which would break `gh`, `cargo fetch` and
`pip install` inside a container that has a network and a checkout the agent is
meant to be able to push from. The container is already the sandbox, so a second one
nested inside it subtracts exactly the capabilities `dl` went to the trouble of
provisioning.

**`IS_SANDBOX=1` is what makes claude's flag usable.** claude refuses
`--dangerously-skip-permissions` outright under `uid 0`, exiting 1 with "cannot be
used with root/sudo privileges", and devcontainers that run as root are ordinary.
The variable is claude's own way of being told the refusal is answering for a
machine that is not there. It is scoped to the agent process and is not exported
into your shell.

What this buys and what it costs is the same sentence: the agent will not stop to
ask, so an `aid owner/repo fix the bug` can run to the end unattended, and it can
also rewrite the checkout it is in without asking. It cannot reach your host. Review
an `aid` workspace before pushing rather than treating it as something that will
stop the agent for you.

None of this reaches a command you typed yourself. `dl <ws> -- claude` runs claude,
exactly as written, with no flags added and no variables set.

## Remote Control: every `aid` session, on your phone too

Every `aid` launch of claude starts with Claude Code's Remote Control on. There is no
flag to type:

```bash
aid blooop/devlaunch@fix/42 fix the flaky test
```

The session is still the one in your terminal, running in the container on your
machine. Remote Control is what also makes it readable and steerable from
claude.ai/code and the Claude mobile app, so you can send it the next thing from a
phone without the workspace being anywhere but where it was.

**The session is named after the workspace you typed**, so the list on claude.ai reads
as the workspaces you opened rather than as a row of untitled sessions. `aid` always
sends a name, because `claude --remote-control [name]` takes an optional one and a bare
flag would read the first word of your prompt as the name instead.

### Turning it off

```bash
aid --no-remote-control blooop/devlaunch@fix/42    # this launch only
aid --no-remote blooop/devlaunch@fix/42            # the same, shorter

export DEVLAUNCH_AID_REMOTE_CONTROL=0              # every launch from this shell
```

The variable takes `1`, `true`, `on` or `yes` and `0`, `false`, `off` or `no`, and
refuses anything else by name rather than guessing which you meant. A flag on the
command line beats it in both directions, so `--remote-control` (or `--remote`) still
turns one launch back on.

Either switch can also be appended to the end of the line, the way `--rm` can, so a
recalled line can be turned off without retyping the front of it: `aid owner/repo fix
the bug --no-remote` starts a purely local session and the prompt survives. That is
bounded the way `--rm` is, to the exact word as a whole argument in the run at the very
end, so a prompt that merely mentions a switch is still a prompt. `aid owner/repo
explain --remote-control please` ends on `please` and asks the agent what you typed.

### Four things worth knowing

**It is claude's and nothing else's.** Remote Control is a Claude Code feature, so
`aid --codex` and `aid --gemini` start with no Remote Control and say nothing about
it: a default that refused would refuse every launch of those two. Typing
`--remote-control` beside either of them is different, because you asked for
something by name, and that is refused by name before anything boots. So is a
`--remote-control` on a line whose `DEVLAUNCH_AID_AGENT` names one of them.

**It needs a claude.ai login inside the workspace.** Remote Control pairs the session
with a Pro, Max or Team account, so a container whose `claude` is signed in with an API
key, or not signed in at all, cannot start one. That login lives in the container along
with the rest of the agent's state, which is what `aid` was already relying on.

**A drivable session is a drivable agent.** `aid` runs claude with
`--dangerously-skip-permissions`, so whoever is signed in to that claude.ai account
can send the agent work and it will not stop to ask. `DEVLAUNCH_AID_REMOTE_CONTROL=0`
is how to turn that off everywhere.

**Nothing survives the workspace.** The session is a process in the container, so
`dl <ws> stop`, `dl <ws> rm`, a `--rm` firing at the end of the line, or Ctrl-C out of
claude all take it offline immediately. The entry can sit in the claude.ai list for
roughly 4 hours after that before it clears, which is the web side timing out rather
than anything still running on your machine.

## `kill`: the workspace that will not answer

`dl <ws> stop` asks devpod to stop a workspace, and it is the right thing to type
right up to the moment devpod itself is the thing that is stuck. Then you get this,
every five seconds, with no deadline behind it:

```
info Trying to lock workspace, seems like another process is running that blocks this workspace
```

That line is not a retry that will eventually give up. devpod takes a blocking
`flock` on the workspace and logs the same string on a timer while it waits, so it
waits for as long as whatever holds the lock lives. The usual holder is a `devpod
up` that outlived the `dl` that started it: reparented to init, sleeping, no
children, and nothing on the machine is ever going to reap it.

`dl <ws> kill` is the way out. The sweep asks devpod nothing, which is the point:
it reads the host's own process table and acts on what is there. It does four
things, and then deletes the workspace:

- **Kills the host processes holding the workspace.** Only `devpod` processes, and
  only ones that name this workspace and whose own parent has died. SIGTERM first,
  then SIGKILL for whatever sat through it. Any devpod subcommand counts, not just
  `up`: they all take the same lock, so an orphaned `devpod delete` or `devpod
  helper` blocks the next launch exactly as an orphaned `up` does. A `devpod up`
  whose `dl` is still running is somebody's build and is left alone, and the report
  says it was.
- **Removes devpod's stale busy marker**, but only once nothing is left holding the
  workspace. This is the file under devpod's `agent` directory, not the lock.
- **Kills any container** the workspace's compose project still has running. Often
  there is none: the container usually dies well before the lock does. Not while a
  live build is standing, though. Those containers are that build's, and killing
  what it is in the middle of creating would break it as surely as signalling it
  would, so they are left alone with it and the report says so.
- **Prints every one of them**, with the pid and the whole command line, so you can
  see what went and how hard it had to be pushed. Every holder it left standing is
  named too, and each says which kind it is: a live build to wait for, a session to
  ignore, or an orphan to go and look at. The two docker calls carry deadlines for
  that reason: a daemon that never answers must not swallow the report of a SIGKILL
  that has already landed.

**And then it deletes the workspace.** The sweep and the delete were never
independently useful: clearing the lock is precisely what lets a `devpod delete`
through, and a workspace wedged badly enough to need the hammer is one you are
throwing away. So `dl <ws> kill` is the whole thing, and the exit code is the
delete's.

**Nothing in that delete refuses, and there is no `--force` to type.** It is `rm`'s
delete with the guard turned into a report: work that exists nowhere else is named
and then destroyed. That is not the guard being dropped for convenience. A wedged
workspace has a dirty clone almost by construction, because whatever wedged it
interrupted the work that was going on in it, so a guard here would refuse in
precisely the case the verb exists for. `rm` is the happy path and keeps its guard
and its `--force`; reach for it whenever the workspace might still be wanted.

**It does stand down for a holder the sweep could not clear**, and there are two of
those. A live `devpod up` is one: the sweep spares somebody's build on purpose,
along with its containers and its busy marker, and deleting the workspace out from
under it would undo all three. A process with nothing waiting on it is the other,
whether it sat through SIGKILL or lost its parent while the sweep was running.
Both hold the flock, so a delete over either would block on it rather than fail.
Neither is a session: an idle `devpod ssh` takes the lock and gives it straight
back, which is why `dl <ws> rm` deletes a workspace somebody is sitting in without
noticing them, and it is why one such session used to be enough to make this verb
do nothing at all. The report names every holder it left and which kind it was,
and the closing line sends you back to `kill` once they are gone.

**And nothing in it waits indefinitely.** Two things buy that. The call carries
devpod's own `--force`, so a workspace whose container or machine devpod can no
longer reach is deleted rather than refused, which is the flag dl's error message
has always told people to type by hand. And it carries a deadline, which no other
delete in dl does: `rm`'s is allowed to take as long as it takes, because a
container that is slow to come down is a container that is coming down, while this
one is being run by somebody who has just sat through the five second lock line
above and must not be put back into it by a holder that arrived after the sweep.
If it does run out, dl says the workspace and its clone are still there and that
running `kill` again picks up where it stopped: devpod is killed a minute into the
job, so it may have got part of the way through.

**The lock file itself is never touched.** Killing the holder is what releases it,
because the kernel drops an `flock` when its holder dies. Unlinking the file would
be worse than the hang: the old holder keeps a lock on an inode nobody else can
see, the next caller locks a fresh file, and two processes both believe they have
the workspace. There is nothing here to delete by hand.

**Naming the workspace is the one place devpod could come into it, and by
workspace id it does not.** Every other lifecycle verb resolves its target through
a `devpod status` with no deadline behind it, which on the host this verb exists
for is the same wait arriving one call early. `dl <ws> kill` skips it: a workspace
id is already the id, so there is nothing for a round trip to settle, and a name
devpod has never heard of sweeps nothing and is reported as a workspace nobody is
holding. `dl owner/repo kill` does have to ask which workspace that resolved to,
and that one call gives devpod five seconds and then falls back to the derived id
rather than refusing.

`kill` takes several workspaces from the selector, like `stop` and `rm` do. A
machine that was suspended, or one whose `dl` was killed by the OOM killer, wedges
every workspace that was open at the time. Now that the verb ends in a delete,
marking five rows deletes five workspaces, and none of the five stops for unsaved
work: each one names what it held on the way past. Marking five rows for `rm`
deletes five workspaces too, so this is the verb doing what it says rather than
something the picker adds, but it is worth knowing before the first TAB.

The advice runs the other way too, in two places, because a plain `rm` can meet the
wedge without knowing it.

A `dl <ws> rm` that devpod *refuses* cannot tell a devcontainer.json that moved from
a workspace something on this host is still holding, so it now names `dl <ws> kill`
alongside the `devpod delete --force` it always named. A `kill` whose own delete is
refused does not print that line, since the sweep it would be asking for is already
on screen above it.

A `dl <ws> rm` that devpod cannot get the lock for is the harder half, because it
never refuses: devpod waits on that lock with no deadline, logging the five second
line at the top of this section for as long as the holder lives, so there is no
exit code for anything downstream to read. dl reads devpod's stderr as it arrives
instead, and answers the first of those lines while the command is still blocked,
naming the `dl <ws> kill` to run in another terminal. It says it once, however many
times devpod says it.


## When devpod is missing or will not answer

These are two different failures and they get two different exit codes.

If `devpod` is not on `PATH`, every command that needs it prints a single install
hint on stderr and exits `127`, the shell's "command not found" code.
`dl --help` and `dl --version` keep working without it.

A `devpod` that is installed but cannot answer is the other case. If `devpod
list` exits non-zero, or prints something that is not a `--output json`
workspace listing, `dl` quotes what devpod said on stderr and exits `1` rather
than reporting that you have no workspaces. That is what stops `dl --purge`
from deleting caches it never checked.

Shell completion is the deliberate exception. `dl --install`, `dl --refresh` and
`dl --completion-data` log the failure and carry on with the repos and branches
they can still discover on local disk, so an unreachable devpod costs you
workspace-name completion and nothing more.

### An `up` that dies with `inject agent … exit status 126`

The message is devpod's and the cause is a substring match. devpod decides which
agent binary to inject by globbing `uname -a` for `arm`, and `uname -a` includes
the container's hostname, so a container whose hostname contains `arm` reads as an
ARM machine on an x86 host. devpod downloads the arm64 agent, the version check
cannot execute it, and 126 is the shell's "not executable". Every workspace whose
branch contains `alarm`, `warm`, `charm`, `swarm`, `harm`, `farm` or `armature` is
a candidate, and `dl`'s own setup pass puts the workspace id into the hostname, so
`dl` is one of the ways the name gets there.

```bash
# the mechanism, in any running container
docker exec <container> sh -c 'case "$(uname -a)" in *arm*) echo ARM;; *) echo NOT;; esac'
# what got injected
docker exec <container> file /usr/local/bin/devpod
```

To unblock a container already in this state without rebuilding it, copy the
right binary in and reconnect:

```bash
docker cp "$(command -v devpod)" <container>:/usr/local/bin/devpod
```

Not `restart` or `recreate`: a recreate wipes it. Renaming the branch is the only
fix that lasts until the upstream match is anchored on `uname -m`.
