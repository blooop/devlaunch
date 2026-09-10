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

`dl <workspace> -- <command>` is an ordinary subprocess, and four things about it are
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

**stdin is the command's.** `echo input | dl ws -- cat` reaches the command inside the
container.

**No terminal is required.** None of the above changes when stdin is a pipe or stdout
is a file. `dl` decides between handing over a session and running a command from the
grammar of the line, not from whether it can see a tty.

The command runs in the devcontainer's workspace folder, as the container's user. The
folder is the devcontainer's own choice and not something `dl` imposes, so read it
rather than assuming a path: `pwd` in the container is the honest answer, and it is
`/workspaces/<workspace-id>` only for devcontainers that do not say otherwise.

These four are pinned by `test/e2e/test_agent_subprocess_contract.py`, which builds one
real workspace and asks each of them of it. They are e2e and skipped by default, because
they need a Docker daemon.

## stderr is not yours yet

The one place the contract does not hold. A command's stderr comes back through devpod's
stream logger rather than as itself:

```
$ dl ws -- sh -c 'echo boom >&2'
11:18:55 info boom stream_logger.go:492
```

Timestamped, level-prefixed, ANSI-coloured and with a Go source location appended. For a
caller that is reading a compiler's diagnostics or a test runner's traceback off stderr,
this is the difference between output it can parse and output it cannot.

Until that is fixed, merge the streams inside the container rather than outside it:

```bash
dl ws -- sh -c 'make test 2>&1'
```

The merge happens before devpod sees the output, so both streams arrive on stdout
verbatim and the exit status is still the command's. This is the recommended form for
any programmatic call whose stderr matters, which is most of them.

Note what the workaround is not. `dl ws -- make test 2>&1` merges on the *host*, after
the mangling has already happened, and gives you the logger's version of stderr mixed
into good stdout. The redirection has to be inside the command `dl` is asked to run.

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
