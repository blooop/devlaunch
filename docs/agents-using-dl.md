# Driving `dl` from a script or an agent

Two different readers are called "agents" around this repository, and until now only
one of them had a page. `AGENTS.md` is for an agent **working on devlaunch**: `dev.sh`,
the `dl`/`dl-next` split, which pixi task to run. This page is for the other one, a
program that runs `dl` as a tool to get work done somewhere else, and what it needs is
the opposite kind of fact. Not how to change `dl`, but what `dl` promises when nobody
is watching the terminal.

Most of what follows already held before it was written down. That is the problem it
solves: a contract nothing states is a contract nothing preserves, and the parts of it
that matter most to a caller are exactly the parts a refactor cannot see it is breaking.

## The subprocess contract

`dl <workspace> -- <command>` is an ordinary subprocess, and five things about it are
promised rather than incidental.

**The exit status is the command's.** `dl ws -- sh -c 'exit 42'` exits 42. `dl`'s own
failures are distinguishable: 1 for a refusal, 127 for a missing devpod.

A command that dies of a **signal** is the exception, and it is worth knowing before
you write the branch. It does not come back as 128 plus the signal number, the way a
shell would report it. Measured on this transport, every signal comes back as **255**:

```bash
$ dl ws -- sh -c 'kill -INT $$';   echo $?   # 255
$ dl ws -- sh -c 'kill -TERM $$';  echo $?   # 255
$ dl ws -- sh -c 'kill -KILL $$';  echo $?   # 255
```

devpod's ssh server reports a signalled remote process as `Process exited with status
255`, with no signal in the line, and `dl` passes that number through rather than
inventing one. So a caller can tell a signalled command from `exit 42`, and cannot tell
*which* signal, or tell either from a command that genuinely exited 255. If your
orchestrator needs the distinction, have the command report it itself, for example
`sh -c 'cmd; echo $? > /tmp/rc'`, rather than reading it off `dl`.

This is not the 130 that [cli.md](cli.md) documents for Ctrl-C. That number is `dl`'s
own exit when a signal reaches `dl`, which is a different event from the command inside
the container dying of one.

**stdout is the command's, verbatim.** Nothing `dl` prints goes there. Every progress
line, every "already running, attaching...", every echoed ssh invocation is on stderr,
which is what makes `output=$(dl ws -- cat some.json)` safe to parse.

Including when the workspace was **stopped** and the call had to start it. That is worth
saying separately because it is the half that was broken until devlaunch#621: devpod's
logger sends its `info` lines to stdout, `dl` echoed each line of a watched `devpod up`
back to the stream it arrived on, and a cold call therefore put the whole build
transcript on stdout ahead of the command's own output. It measured 26KB against a
prebuilt image. Every test of this clause asked a warm workspace, which runs no `up` at
all, so the guard and the page agreed with each other and not with the binary. A caller
that parsed `dl ws -- cat some.json` worked until the first time somebody stopped the
workspace.

**stderr is the command's too.** `dl ws -- sh -c 'echo boom >&2'` puts `boom` on stderr
and nothing else around it, so a compiler's diagnostics and a test runner's traceback
arrive parseable. `dl`'s own narration shares that stream, and it all comes before the
command starts, so the command's output is the tail of it.

Two caveats worth knowing before you match on it. The command's stderr is still read a
line at a time on the way out, so a partial last line arrives with a newline appended
that the command did not write. And devpod's transport strips ANSI escapes from it, so
a tool that colours its errors arrives uncoloured; since the command sees a pipe rather
than a terminal, most tools emit no colour there anyway.

This clause used to be the one the transport did not keep, and callers were told to
merge the streams inside the container instead. That merge still works and is still the
right call when you want one interleaved stream rather than two:

```bash
dl ws -- sh -c 'make test 2>&1'
```

Note where the redirection is. Inside the command `dl` is asked to run, both streams
arrive on stdout in the order the command wrote them. `dl ws -- make test 2>&1` merges
on the host instead, and folds `dl`'s own narration in with the output. It is no longer
a workaround for anything, though, so reach for it only when you actually want the
interleaving.

**stdin is the command's.** `echo input | dl ws -- cat` reaches the command inside the
container.

**No terminal is required.** None of the above changes when stdin is a pipe or stdout
is a file. `dl` decides between handing over a session and running a command from the
grammar of the line, not from whether it can see a tty.

The command runs in the devcontainer's workspace folder, as the container's user. The
folder is the devcontainer's own choice and not something `dl` imposes, so read it
rather than assuming a path: `pwd` in the container is the honest answer, and it is
`/workspaces/<workspace-id>` only for devcontainers that do not say otherwise.

These five are pinned by `test/e2e/test_agent_subprocess_contract.py`, which builds one
real workspace and asks each of them of it. They are e2e and skipped by default, because
they need a Docker daemon.

## The unit of isolation is the branch

A workspace id is derived from the `(owner, repo, branch)` triple, so one branch is one
workspace, one clone and one container. Two agents pointed at the same branch of the
same repository do not get two sandboxes; they get one, and they will collide inside it
exactly as two processes in one working tree always have.

This is the single most important thing for an orchestrator to know, and it is a
property of the design rather than a limitation to route around: separate clones per
branch is what `dl` is for. Give each agent its own branch and the isolation is real.
Give two agents one branch and there is none.

What is safe is the concurrency itself. Several `dl` processes are expected to run at
once, and the shared state they meet on the way (one bare clone per repository, one
`metadata.json`) is protected by `flock(2)` with a documented lock ordering in
`rust/devlaunch-core/src/domain/locks.rs`. A crashed `dl` releases its locks the way any
process does, so a killed agent does not wedge the cache for the others.

## Finding out what is there

`dl --ls --json` is the machine-readable listing and the intended entry point for a
caller deciding what to do:

```json
[
  {
    "id": "bencher-ruff-waxd",
    "devlaunch": true,
    "repo": "blooop/bencher",
    "branch": "ruff",
    "checkedOut": "ruff",
    "path": "/home/user/.cache/devlaunch/repos/blooop/bencher/bencher-ruff-waxd",
    "state": "Running",
    "lastUsed": "2026-09-10T09:24:10Z",
    "unsaved": { "nothingToLose": true }
  }
]
```

The document is an **array** of those, always, including when it holds one row or
none. `json.loads(out)` is a list.

Three fields earn their place in a script. `state` says whether a call will pay a cold
start, and is `null` when `devpod status` would not answer. `devlaunch` separates the
workspaces `dl` made from the ones it merely found, so a cleanup pass can leave other
people's alone; on a row where it is `false`, `repo`, `branch`, `checkedOut` and `path`
are `null` too, because there is no clone behind it for `dl` to have read them from.

And `unsaved` is the one worth reading before anything destructive. It is the same
judgement the `rm` verb makes, and it is the only way to make it without a `dl` process
in the loop. **It is not two-valued**, and a caller that treats it as two-valued deletes
work:

- `{ "nothingToLose": true }`. Nothing would be lost.
- `{ "wouldLose": "..." }`. Named work a delete would take with it.
- `{ "couldNotTell": "..." }`. git could not be read, so nothing is known either way.
  `dl <ws> rm` refuses on this rather than waving it through, and so should you: it is
  the absence of an answer, not an answer of no.
- `{ "wouldLose": "...", "couldNotTell": "..." }`. Both at once, when part of it could
  be read and part could not. Refuse on this too.
- `null`, for a workspace `dl` did not make and has no clone for. Indexing it raises.

So the only safe test is `nothingToLose` present and true. Everything else, `null`
included, is a reason not to destroy anything without a person in the loop, and a
caller that tests for `wouldLose` alone will force-delete exactly the rows `rm` itself
would have stopped at.

`--json` is currently `--ls` only. Other commands report in English, so a caller that
creates a workspace and then needs to address it should derive the id from a subsequent
`--ls --json` rather than parsing the launch.

## Start once, then run many

A stopped workspace has to be started before a command can run in it, and that cost is
paid per call. A loop of short `dl ws -- ...` invocations against a stopped workspace
pays it every time.

```bash
dl blooop/bencher@ruff up          # once, and it returns when the workspace is up
dl bencher-ruff-waxd -- sh -c '...'  # then as many as you like
```

`up` is the verb for exactly this: it starts a workspace without attaching to it. How
long a start takes depends on the image and whether it is prebuilt, and
[performance.md](performance.md) has the measurements.

## Cleaning up after a run

Three different things, and picking the wrong one is how an agent either leaks
workspaces or destroys work.

- `dl <ws> --rm -- <command>` is docker's `--rm`: run the command, delete the workspace
  when it ends. Best effort by nature, since a killed `dl` never reaches the removal.
- `dl <ws> rm` deletes one now. It refuses if the clone holds uncommitted or unpushed
  work, or if git could not be read to find out. `--force` overrides, and an agent
  should reach for it only against a workspace whose `unsaved` it has already read.
  Before it refuses over unpushed commits alone, `rm` fetches the clone's `origin`
  once, bounded at 30 seconds, so commits pushed by URL rather than through `origin`
  do not block it. `--ls --json` does not fetch, so `unsaved` there can still count
  those commits. A fetch that fails leaves the refusal standing and says so.
- `dl <ws> kill` is for a workspace that is wedged rather than merely unwanted. It
  reports what it destroys instead of refusing, so it is not a louder `rm`.

Both refusals are the useful default for a caller that might be wrong about what is
disposable. An agent that leaks a workspace costs disk; an agent that force-deletes a
branch nobody pushed costs the work. [cleanup.md](cleanup.md) has `--prune` and
`--purge`, which sweep rather than delete one thing.

## What this does not give you

`dl` isolates git state and processes. It is not a security boundary, and it is worth
being explicit about that here because "sandbox" invites the other reading.

A workspace runs whatever its devcontainer asks for, and that is the project's decision
rather than `dl`'s: a devcontainer that requests `privileged` and binds `/dev` gets
both. Host credentials are forwarded on purpose so that tools inside the workspace work,
including the `gh` token and the Claude login, and several host directories are mounted
read-write by the devcontainers themselves. `DEVLAUNCH_NO_GH_TOKEN=1` and
`DEVLAUNCH_NO_CLAUDE_TOKEN=1` turn the two forwards off where a caller does not want
them. [workspace-tools.md](workspace-tools.md) has what is forwarded and why.

So: run agents you would run on the host anyway, and let `dl` keep them from treading on
each other. Do not reach for it to contain one you would not.
