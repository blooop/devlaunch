# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

The Rust rewrite is deferred, and Python remains the implementation. 0.0.11 called
itself the last release of the Python implementation, on the go decision recorded in
[#53](https://github.com/blooop/devlaunch/issues/53); 0.0.12 and 0.0.13 have shipped
since, and there is no cutoff to plan around now. Nothing about how you install or run
`dl` changes, and no release is withdrawn — this only retires an expectation the
changelog had set.

What it does change is what is worth doing to this codebase. Work that was ruled out as
wasted motion in front of a rewrite — the `dl.py` structural refactor #53 was gating,
and paying down anything scoped as "the Rust version will fix it" — is back on the
table and should be judged on its own merits.

## [0.0.21] - 2026-08-08

One workspace per branch means workspaces accumulate, and until now the only
tool for it was `--purge`, which is all-or-nothing and takes the caches too.

devlaunch deliberately does **not** decide which workspaces are finished:
whether work is over is a fact about a ticket, a review or somebody's intent,
and `dl` knows about clones and containers. A branch-shaped inference — merged
into the default branch, or gone from the remote — was built first and dropped;
it reads as a git fact but is a guess at intent, and it cannot tell a
squash-merged branch from an abandoned one. What ships instead is the mechanism
a tool that *does* know can drive.

### Added

- `dl --ls --json`: the workspace list as JSON, each entry carrying `repo`,
  `branch`, `checkedOut`, `path`, `state`, `lastUsed`, `devlaunch` (did dl make
  it), and `unsaved` — a description of what deleting would destroy, or null.
  Workspaces dl did not create report `devlaunch: false` and are not inspected.

### Changed

- `dl <workspace> rm` refuses when the clone holds uncommitted changes or
  commits no remote has, naming what would be lost and how to insist. `--force`
  deletes anyway. This is the only judgement dl makes here, and it is about the
  only copy of something rather than about finished work. `--purge` is
  unaffected: it already scopes itself to what it is about to delete anyway.

## [0.0.20] - 2026-08-08

Launching several workspaces at the same moment is now safe. It nearly was
already — the point of the isolated-devcontainer work — but the launches
themselves still raced each other over the shared bookkeeping on the host: two
first launches of one repo both ran `git clone --bare` into the same cache path,
and the loser's cleanup deleted the winner's half-written clone out from under
it; and every launch rewrote `metadata.json` from a copy loaded at its own
startup, so simultaneous launches silently dropped each other's workspace
records. Firing two `dl owner/repo@branch` (or two `aid`) at once could
therefore cost you a clone or a workspace listing, with nothing said.

### Fixed
- Concurrent `dl` processes now serialize their work on any one repo's cache
  with an inter-process lock (`repos/<owner>/<repo>/.lock`, `flock`, so a
  crashed run can never leave the cache wedged). The second launch waits — and
  says it is waiting — then reuses the clone the first one made, instead of
  racing it and destroying it.
- `metadata.json` writers reload the file under a lock before rewriting it, so
  a workspace record added by one process can no longer be erased by another
  process that loaded earlier. This also covers the background completion
  refresh, which shares the same file.
- A bare clone found on disk without a metadata record — another process just
  made it, or an earlier run died before saving — is now registered as it
  stands. Previously `dl` tried to clone over it, failed, and its cleanup
  deleted the cache the other launch was using.

## [0.0.19] - 2026-08-08

Three of these are about `dl` and its dev container leaving alone what is not
theirs. `dl --purge` deletes only the workspaces devlaunch itself created, where it
used to take every workspace `devpod list` returned — including ones you made by
hand. The dev container mounts the two ssh files it actually needs instead of your
whole `~/.ssh`, so a devpod running inside it stops leaving entries in your real
config that nothing outside the container can use. And a workspace whose source
`dl` cannot read is now named rather than dropped in silence. The fourth is CI,
which turned out not to have been running on stacked pull requests at all.

Nothing about how you install or run `dl` changes and no workspace needs
rebuilding, but the dev container has to be rebuilt once to pick up the ssh mount
change, and `--purge` now leaves more standing than it used to — read the note
under Changed if you have opened workspaces with `dl ./path` or `dl <git-url>`.

### Added
- A `gate` job that depends on every other job in the CI workflow and fails unless
  all of them succeeded, so that a branch ruleset has one stable name to require
  instead of a list of literal job names that nobody reviews and that goes stale
  whenever a job is added or renamed. It insists on `success` rather than on the
  absence of `failure`, so a job that was cancelled or skipped fails it too — a
  check that did not run is not a check that passed.

### Changed
- `dl --purge` now deletes only the DevPod workspaces devlaunch created — the
  clones it made under its own cache directory — instead of every workspace
  `devpod list` returns. devpod's namespace is shared, and a workspace you made
  with `devpod up`, or that another tool made, was being destroyed along with
  devlaunch's own.
- `dl --purge` names the workspaces it is leaving behind, before it asks for
  confirmation, and the count it asks you to approve is now the number it will
  actually delete. **If you have used `dl ./path` or `dl <git-url>`:** those
  workspaces open a source `dl` did not clone, so `--purge` cannot tell them from
  one you made by hand and now leaves them standing. They are listed in the
  output; remove one with `dl <workspace> rm`.
- A workspace's source is one value rather than a `source_type` tag beside a
  parallel `source` string. Each arm carries only what that arm has — a folder
  path, a repository URL, or the raw payload for a source devlaunch cannot read —
  so the tag and the value can no longer disagree, and every reader of a source is
  exhaustive under the type checker CI already runs.

### Fixed
- CI runs on every pull request, not only on pull requests targeting `main`. The
  `branches:` filter on a `pull_request` trigger matches the base branch, so a
  pull request onto any other base triggered the workflow not at all — no run,
  pending or otherwise. This repository's `/stack` workflow exists to produce
  chains in which every link but the last targets its predecessor, so every one of
  those links was merging with nothing behind it, e2e and the interpreter matrix
  alike.
- The dev container no longer bind-mounts the developer's whole `~/.ssh`. Only the
  ssh agent socket and a read-only `known_hosts` are mounted, so the `Host
  <id>.devpod` blocks that a nested devpod writes stay inside the container and die
  with it, instead of accumulating on the developer's real ssh config with a
  `ProxyCommand` nothing outside the container can run.
- The dev container image builds from a checkout that already has a pixi
  environment in it. It could not before, for want of a `.dockerignore`.
- Pointing `XDG_CACHE_HOME` at a scratch directory now protects `dl --purge`. It
  never did: `devpod list` reads `~/.devpod`, so a scratch run still saw — and
  deleted — every real workspace on the machine.
- `dl` no longer drops a workspace whose source it cannot read. Repo discovery
  skipped it in silence, which is the same outcome as a source it read fine and
  found no repo in; it now says which workspace, and what devpod described. `dl
  --ls` and the fuzzy picker show that payload rather than a Python `repr` of it,
  and the picker still offers the workspace instead of leaving it out of the list.
- A `devpod list` entry `dl` cannot make sense of is refused or reported instead of
  being half-read. A `source` that is not an object at all is now an unreadable
  listing, where it used to reach a substring test — `"localFolder" in
  "/srv/localFolder/x"` is true, and the indexing that follows is a `TypeError`.
  A `localFolder` or `gitRepository` that devpod left empty, or filled with
  something other than text, is a source `dl` cannot read rather than a folder at
  the empty path: `git -C ""` succeeds, so the second of those would have credited
  a workspace with whatever repository you happened to be standing in.

## [0.0.18] - 2026-08-08

Mostly a release about developing `devlaunch` rather than running it: the dev
container now carries its own Docker daemon, so the e2e suite and `dl` itself run
inside it against that daemon and not the host's, and the same suite runs in CI —
where a green tick now means it did something, which it did not always before.
Riding along is the one user-visible fix of the three, a `dl` that stops claiming
you have no workspaces when what actually happened is that it could not find out.
Nothing about how you install or run `dl` changes and no workspace needs
rebuilding, but the dev container has to be rebuilt once and costs meaningfully
more disk than it used to. 0.0.17 shipped the AGENTS.md half of this same arc.

### Added
- The dev container carries its own Docker daemon, so the e2e suite and `dl`
  itself both run inside it — against that daemon, not the host's. Several
  branches can be developed and e2e-tested at once on one machine without
  touching the host's Docker, its devpod workspace list, or each other. `pixi run
  test-e2e` runs the suite; it is still skipped by the default `pixi run test`,
  because what it needs is a real daemon and a real devpod, not nesting, and it
  creates and deletes real containers — in the private devpod home it makes for
  the run, never yours. The container also registers the docker provider when it
  is created — a fresh container has an empty devpod home and devpod seeds nothing
  into it, so without that step `dl` inside would exit on the first command it
  ran.
- The README now says what the dev container costs: roughly 2 GB on the host per
  branch plus about 2.3 GB in the nested daemon's volume, so budget around 4 GB
  per branch you are actively working on and about 12 GB for three at once.
  Nothing reclaims those volumes — `devpod delete` removes containers without
  touching volumes, and Docker never garbage-collects a named one — so the
  section also shows how to find what has piled up. This is documented rather
  than mitigated on purpose: every candidate mechanism cost about an order of
  magnitude more than it saved, and pruning images from a task would have thrown
  away exactly the ones the next e2e run needs.
- The e2e suite runs in CI, in a job of its own, on pushes to `main` and on pull
  requests targeting `main`. It is
  plain `pytest -m e2e` against the runner's own Docker rather than a nested
  daemon: a development machine needs one because it is shared and long-lived,
  and a runner is an ephemeral VM with Docker already on it. The job sits outside
  the py310–py313 matrix, because what it exercises is devpod and Docker and not
  a Python version, and it needs no devpod install step, devpod being a pixi
  dependency already in the lockfile. It finishes before the matrix does.
  `pixi run test-e2e` is the same run on your own machine — where it builds real
  containers on your Docker, so read the README first.

### Changed
- The dev container no longer shares the host's network namespace. Nothing this
  repo reaches for needed it, and it caused a real collision: a listener in the
  container was a listener on the host, so two containers could not both run the
  Claude OAuth callback flow and neither could one while the host held the port.
  It is also incompatible with nesting a daemon, since a second daemon in the
  host's namespace co-manages the host's bridge and writes its NAT rules into the
  host's tables.
- The `claude-code` feature's docs no longer tell you to turn host networking on.
  The argument they made for it had a real mechanism and a wrong conclusion: the
  OAuth callback listener genuinely is inside the container while your browser is
  outside it, but the answer is to authenticate on the host once — which the
  mounted credentials already arrange, so the flow never runs — or to forward the
  port for a single session. Both documents now say that, and say what the flag
  costs. The VS Code extension limitation recorded alongside it is gone, having
  been a consequence of host networking rather than a fact about the feature.

### Fixed
- `dl` no longer reports that a machine has no workspaces when it merely failed
  to find out. `devpod list` can fail by exiting non-zero and it can fail by
  answering with something that is not a listing, and both used to come back as
  an empty list — which is also how devpod says there genuinely are none. The
  sharpest cost was `dl --purge`: it iterates that list, so a purge that never
  learned what to delete printed that there was nothing to purge and then removed
  your local cache anyway, looking exactly like a purge that had nothing to do.
  It now stops before touching anything, quoting what devpod said. Elsewhere the
  same empty list read as "this workspace does not exist yet", which is the wrong
  branch to take when the truth is "I could not tell". A listing that reads fine
  and is empty is still empty, so `dl --ls` on a fresh machine still says "No
  workspaces found" and exits 0. Silence from devpod counts as a failure to
  answer, checked against the real binary: devpod with an empty home prints `[]`.
- The `dev-add-docker` provider guard reports a failed `devpod provider add` with
  devpod's own explanation attached, where it used to report only an exit code.
  Its handler also no longer catches every `RuntimeError` in the process, so an
  unrelated bug in a `pixi run dev` stops looking like a devpod problem.
- Shell completion still installs and still completes when devpod cannot be
  reached. `dl --install` warms the completion cache before it installs, so it is
  the one place that reads the workspace list without the list being the point:
  the repos, owners and branches it offers come off your own disk. It now says
  which part it could not fill in and gets on with the rest, rather than
  installing nothing at all — `dl --install`, `dl --refresh` and
  `dl --completion-data` behave exactly as they did before this release.
- An e2e run that could not do anything no longer reports that it passed. Two
  unrelated outcomes were both spelled `skipped`: tests declining an opt-in they
  were never given, and tests that could not reach what they needed. A run
  against a registry serving the suite's 1.25 GB fixture image at 640 B/s
  reported `7 passed, 14 skipped` having created no containers at all, which is
  the summary line a healthy run prints — and with thirteen legitimate skips in
  the baseline, one more was invisible. Deliberate skips now say so in a word of
  their own — a distinct exception type, so the check is what a test raised and
  not how it worded it; any other skip under the e2e directory is reported as a
  failure against the test's own name; a missing devpod fails the session once
  instead of skipping five tests quietly; and every run prints the workspaces it
  actually built and refuses to exit zero if the tests that promised one built
  none.
- `test_git_status_via_ssh` tests git in a container again. It had been pointed
  at the fixture's bare repository, which has no work tree, so `git status`
  inside the container exited 128 on any machine; it now gets the working copy.
  Nobody had seen it, because until the creation
  step was made unskippable its assertions sat behind a condition that was never
  true on a headless machine. The first session to reach them was the first CI
  run of this suite.

### Removed
- The standalone `test/docker/` dind harness. Its own header named one job, which
  is now verbatim the dev container's job and measured. The one real argument for
  keeping it — a CI runner that cannot nest a container — does not hold, because
  a hosted runner is already an ephemeral VM with Docker on it and has no host to
  protect. It was also the wrong image regardless: Alpine, a plain `pip install`
  and a download of whatever devpod release was newest, diverging from the shipped
  Ubuntu-and-pixi environment on every axis under test and reaching around the
  lockfile the devpod pin exists to enforce.

## [0.0.17] - 2026-08-08

### Changed
- `AGENTS.md` says which build to run inside this repo's devcontainer, instead of
  leaving the host's two-install advice to be followed in a place it does not work.
  There is one build in there and it is the checkout — the devcontainer installs it
  editable at create time — so the answer is `pixi run dl` and `pixi run aid`, and
  `./dev.sh` should not be run in there at all: it exits at its first check because
  the container has no `uv`, which is the right outcome rather than a gap to fill.
  The reason `pixi run` matters is which devpod `dl` finds. devpod injects its own
  agent binary onto the bare `PATH` of every container it creates, so a devlaunch
  installed outside the project environment finds a devpod when the container was
  opened by devpod and none when it was opened by VS Code — intermittent by how you
  got in. The project environment's devpod is present either way and is the version
  the tree is pinned against. Nothing about `dl` on a host changes, and the two
  builds the section already described are unaffected.

## [0.0.16] - 2026-08-08

One fix, and it is the first of this run of releases that an ordinary `dl` user
will notice: a launch that could not find a GitHub login used to say nothing about
it. Nothing about how you install or run `dl` changes, and no workspace needs
rebuilding.

### Fixed
- `dl` says so when it opens a workspace with no GitHub credentials. It forwards
  the host's token into every workspace it launches, but each way of failing to
  find one returned quietly, so the first sign of trouble was `gh` failing inside a
  container that had already been built. Every such path now warns, naming what
  went wrong and never printing the value it read. The `gh auth token` failure also
  names the config directory it read, because the usual cause is a scratch run that
  scoped `XDG_CONFIG_HOME` away from the host's login — for which `gh auth login`
  is exactly the wrong remedy. Relatedly, the scratch-run recipe in `AGENTS.md` and
  `dev.sh` no longer tells you to set `XDG_CONFIG_HOME`, which had been breaking
  `gh auth token` on every run that followed it; the trade that recipe makes is now
  written down rather than implied.

## [0.0.15] - 2026-08-08

Three changes. Only the `aid` one changes what a command does; the other two are to
the test suite and the development tasks, and they matter most to anyone who runs
them on their own machine — where, until now, doing so could cost them their
workspaces. Those two are recorded here after the fact: they were merged into this
release but were not described when it was cut.

### Changed
- `aid` now starts `claude` with `--dangerously-skip-permissions`, so
  `aid owner/repo fix the bug` runs to completion instead of stopping at the first
  tool prompt. The prompts guard a host with your whole filesystem on it; the agent
  `aid` starts is already inside a disposable devpod container with one repo in it,
  where they cost an unattended run its point and buy no isolation that the
  container is not already providing. `--codex` and `--gemini` are unchanged —
  neither takes this flag — and `dl <ws> -- claude` still runs exactly the command
  you typed, permission prompts and all. Nothing here changes what a workspace is
  or how it is built.

  `IS_SANDBOX=1` is set on the agent process for the same reason. `claude` refuses
  `--dangerously-skip-permissions` outright under `uid 0` — it prints "cannot be
  used with root/sudo privileges" and exits 1 — and a devcontainer running as root
  is ordinary, so the flag on its own would have stopped `aid` from starting at all
  in those workspaces rather than merely failing to help. The variable is scoped to
  that one command and is not exported into the login shell around it.

  A side effect worth knowing: an agent started this way will edit, run and delete
  inside the container without asking. It cannot reach the host, but it can rewrite
  the checkout it is in, so treat an `aid` workspace as something to review before
  pushing rather than as a sandbox that will stop it for you.

### Fixed
- The test suite gets a devpod namespace of its own, so running the e2e suite on a
  development machine can no longer delete that machine's real workspaces. The
  suite exercises `dl --purge`, which lists every workspace devpod knows about and
  force-deletes each one; run on a host rather than in a container, "every
  workspace devpod knows about" was the developer's own. `pytest_configure` now
  points `DEVPOD_HOME` and `DEVPOD_SSH_CONFIG` at a fresh per-run directory before
  collection begins, so every devpod subprocess the session spawns — including the
  one inside `--purge` — inherits a namespace with nothing of the user's in it.
  Setting it before collection rather than in a fixture is deliberate: a fixture is
  something a test has to ask for, and the test that must not forget is the one
  nobody has written yet. The per-run directory is left behind rather than cleaned
  up, since a deletion is the failure mode being designed out.
- `pixi run dev`, its siblings and `test-e2e` work again. Their devpod provider
  guard looked for `docker` in the output of `devpod provider list`, which prints a
  colour-coded table; the escape sequence sits directly against the provider name,
  so a word-boundary match never fired and the guard re-added a provider that was
  already there, failing the task before any of its real work started. The guard
  now asks devpod for `--output json` and reads the answer, rather than matching
  against a rendering that is free to change again. In the same pass, an e2e test
  that skipped when its workspace-creation step failed was leaving the container it
  had already built running while reporting green — every e2e workspace now goes
  through one helper that registers the workspace for cleanup before creation is
  attempted, passes `--ide none`, and fails rather than skips.

## [0.0.14] - 2026-08-08

Workspaces come with the tools a session needs. Nothing about how you install or
run `dl` changes; existing workspaces pick the tools up on their next restart.

### Added
- `gh` and `claude` are installed into every workspace `dl` opens, so both are on
  PATH in an interactive session, in `dl <ws> -- <command>`, and under `aid`,
  whatever the repo's devcontainer.json provides. `dl` already forwarded the
  host's GitHub login into every container but not the `gh` to spend it on, so
  `dl <ws> -- gh auth status` died with `command not found` while holding a valid
  token. Installed with `pixi global` on `devpod up` and exposed through
  whichever of `~/.bash_profile`, `~/.bash_login` or `~/.profile` bash actually
  sources; `pixi` itself is installed first if the image has none. A
  workspace that already has both is left alone, so the cost after the first
  launch is one round-trip and no network. An install that fails costs the
  workspace its tools and not its launch. Set `DEVLAUNCH_NO_TOOLS=1` to opt out.
  Attaching to an already-running workspace skips `devpod up` and so skips this;
  such a workspace picks the tools up on its next `dl <ws> restart`.

## [0.0.13] - 2026-08-08

One fix: `dl <ws> -- <command>` now gives the command a terminal when you have
one, so interactive programs — a coding agent, a REPL, `git rebase -i` — start
and stay up instead of exiting immediately. This is what made `aid <repo>` return
straight to your shell.

### Fixed
- Interactive commands get a terminal. `devpod ssh --command` never requests a
  pty, so anything it started ran with stdin, stdout and stderr on pipes and
  `TERM=dumb`. Nothing about that looks like a missing terminal from the outside:
  `claude` reads the pipe as a non-interactive invocation, switches to `--print`
  mode and exits, so `aid <repo>` left no session behind and `aid <repo> 'fix it'`
  printed one answer and stopped. `dl` now hands such commands to OpenSSH through
  the `<workspace>.devpod` host alias `devpod up` already writes, with `-t`, which
  also puts window size and SIGWINCH in OpenSSH's hands rather than dl's.

  The choice is the one `ssh` itself makes — a terminal when there is a terminal
  to use — so `dl <ws> -- ls > files.txt` keeps the devpod transport and stays
  free of escape sequences. A workspace with no host alias falls back with a
  warning that says how to republish it, and `DEVLAUNCH_NO_TTY=1` forces the
  fallback everywhere. A bare `dl <ws>` attach is untouched; devpod already gives
  that one a pty.

### Changed
- The devpod floor moves from 0.8 to 0.26.1, in the conda recipe and in the
  development environment. dl's behaviour depends on devpod's, and the two differ
  across that range — 0.8 gives `devpod ssh --command` a pty and 0.26 does not —
  so the suite had been exercising a devpod five years of releases behind the one
  `dl` ships alongside, and could not have reproduced the bug above at all.

## [0.0.12] - 2026-08-08

One fix: `dl` stops reporting a failure every time you leave a workspace, and a
one-shot `dl <ws> -- <command>` now exits with the command's own status instead
of a flat 1. Nothing about how you install or run `dl` changes.

### Fixed
- Leaving a workspace no longer reports a failure. devpod turns any nonzero exit
  from the program it ran into a fatal of its own ("tunnel to container: run in
  container: ssh session: Process exited with status 130") and exits 1, because it
  type-asserts on an `*ssh.ExitError` it has already wrapped. Typing `exit` in a
  shell whose last command was interrupted was enough to trigger it. `dl` now
  reads that status back out and reports it as the session's, so an ordinary exit
  is silent and `dl <ws> -- <command>` propagates the command's real exit code
  instead of a flat 1. Failures that are genuinely devpod's still print in full.

## [0.0.11] - 2026-08-08

This completes the review that ran across [#51](https://github.com/blooop/devlaunch/issues/51):
seven targeted fixes to performance, correctness and maintainability, of which six
shipped in 0.0.10 and the cache migration lands here. It is also intended to be the
last release of the Python implementation — the successor is a Rust rewrite whose core
becomes a library shared with [blooop/wayfinder](https://github.com/blooop/wayfinder),
decided in [#53](https://github.com/blooop/devlaunch/issues/53). Nothing about how you
install or run `dl` changes in this release.

> **Superseded.** The Rust rewrite was deferred on 2026-08-08 and Python remains the
> implementation, so this was not the last Python release — 0.0.12 and 0.0.13 followed.
> The rest of the entry stands as shipped. See [Unreleased](#unreleased).

### Changed
- Existing caches are migrated onto the new workspace id scheme once, by the first
  command that touches a workspace. Clone directories are renamed in place —
  `~/.cache/devlaunch/repos/blooop/devlaunch/main` becomes
  `.../devlaunch-main-zovomobo` — a plain `mv` of a git clone, which carries its `.git`
  with it and refers to its own path nowhere, so history and **uncommitted changes
  survive**; `metadata.json` is updated in the same atomic write and its `version`
  becomes `2`. `dl --help`, `dl --version` and `dl --ls` do not trigger it, and a
  second run does nothing.
- Existing devpod containers keep their old ids and are orphaned, since the new id
  names a new container. dl does not delete containers for you: it prints the count
  and writes the old ids to `~/.cache/devlaunch/orphaned-workspaces.txt`, so
  `xargs -r -n1 devpod delete < ~/.cache/devlaunch/orphaned-workspaces.txt` clears
  them when you are ready. A clone directory with no metadata record cannot be
  renamed — nothing records which branch it holds — so it is left alone and listed
  in `~/.cache/devlaunch/unmigrated-clones.txt`.

### Fixed
- The test suite no longer reads or writes the real `~/.cache/devlaunch`. One test
  reached a code path that builds a real clone manager, which with the migration in
  place would have renamed the developer's own workspace clones.

## [0.0.10] - 2026-08-07

### Added
- `dl --version` reports which install it is. A released build and an editable
  install of the same commit both printed a bare `dl <version>`, so a stale
  released binary was indistinguishable from a working tree at runtime — pulling
  a fix and still seeing the old behaviour read as a failed merge rather than as
  the wrong binary on `PATH`. An editable install now says so and names the tree
  it resolves to. Detection reads PEP 610 `direct_url.json` through
  `importlib.metadata` and is strictly additive: absent, malformed or
  missing-key metadata all fall back to the bare output rather than raising.
  `aid --version` inherits it.

### Fixed
- A corrupt `~/.cache/devlaunch/metadata.json` no longer takes down every `dl`
  command, `dl --help` included. It used to raise while the storage object was being
  built, before any command ran. An unreadable file is now moved aside to
  `metadata.json.corrupt` and dl starts with empty metadata; a single malformed entry
  is skipped rather than costing the whole file; and any load that would drop
  information — a skipped entry, a field only a newer build knows about, a newer
  schema version — copies the original to `metadata.json.bak` before the next write
  can overwrite it, and says so. Saving is atomic, writes through a symlinked
  `metadata.json` rather than replacing the link, and preserves the file's mode.
  The file gains a `version` key.
- `dl` says so when devpod is not installed, instead of printing a
  `FileNotFoundError` traceback. One line naming the install page, and exit `127` —
  the shell's own "command not found" code. `dl --help` and `dl --version` keep
  working without devpod, and the completion commands leave stdout empty, since that
  is what the shell parses.
- `dl <repo> -- <cmd>` runs its command in a login shell, so it gets the same
  `PATH` an interactive `dl <repo>` attach gets. devpod runs a `--command` payload
  under a non-login, non-interactive `bash -c`, which sources neither `~/.profile`
  nor `~/.bashrc` — so `PATH` entries an image adds there (notably
  `$HOME/.pixi/bin`) were missing and the payload died with `command not found`
  and exit 127. This is what made `aid` unable to find `claude` in a workspace
  where `dl` could. dl launches arbitrary repos, so the parity comes from the
  invocation rather than from any particular `devcontainer.json`.

### Changed
- Workspace ids are derived at a single parse boundary, with a wider id suffix, so
  two specs can no longer collide onto one workspace.
- Fewer devpod shell-outs per invocation: the same devpod answer is no longer
  fetched twice, and the completion cache refreshes on a TTL once per invocation
  rather than on every completion. Both cut startup latency.
- A development install from the working tree installs as `dl-next`, leaving a
  released `dl` in place, and reads its entry points from `pyproject.toml`.

## [0.0.9] - 2026-08-07

### Added
- `aid`, a second entry point that opens a workspace and starts a coding agent in
  it: `aid owner/repo@branch fix the flaky test`. It is a shortcut, not a second
  launcher — it rewrites its command line into `dl owner/repo@branch -- claude
  'fix the flaky test'` and hands that to `dl`, so an `aid` workspace is the `dl`
  workspace: same clone, same workspace id, same container, reused rather than
  rebuilt. Pick the agent with `--claude` (default), `--codex` or `--gemini`, or
  set `DEVLAUNCH_AID_AGENT`; `--devcontainer` passes through, and everything after
  the workspace is the prompt. This replaces the `aid` in `blooop/rockerc`, which
  ran on rocker and built an image per launch instead of reusing the workspace.
- `dl` and `aid` share one completion function, so `aid` tab-completes the same
  workspaces, repos, owners and branches. Reinstall with `dl --install`.

### Changed
- `dl.main()` takes an optional argv list, so a sibling entry point can hand `dl`
  a command line and get `dl`'s own behaviour rather than a copy of it. Calling it
  with no arguments is unchanged.

## [0.0.8] - 2026-08-07

### Added
- The host's GitHub CLI login is forwarded into every workspace as `GH_TOKEN`, so
  `gh` works inside whatever container is launched without its devcontainer.json
  arranging anything. The token comes from `GH_TOKEN`, `GITHUB_TOKEN`, or
  `gh auth token`, and reaches devpod through a private file (`devpod up`) and
  devpod's own environment (`devpod ssh`) rather than a command line, so it stays
  out of `ps`. Everything in the container can read it, including a repo's own
  `postCreateCommand`, so set `DEVLAUNCH_NO_GH_TOKEN=1` — for one launch or for the
  machine — to opt out.

### Fixed
- A corrupt `metadata.json` no longer takes down every `dl` command, `dl --help`
  included. Loading is total now: an unreadable or non-object file is quarantined
  to `metadata.json.corrupt` and load continues with empty state, a single
  malformed entry is skipped instead of the whole file, and an entry carrying a
  field only a newer build declares loads without that field rather than failing.
  Any load that drops information copies the original to `metadata.json.bak`
  before the next write can overwrite it, and says so on stderr.
- On a box without devpod, workspace commands print one line on stderr and exit
  127 instead of a raw `FileNotFoundError` traceback. `--help`, `--version` and
  the completion paths never touch devpod and still work; `--update-cache` now
  leaves a good cache in place rather than overwriting it with an empty one.

### Removed
- Deletion-only hygiene pass, no behavior change: template leftovers from the
  python_template origin (`PROMPT.md`, `ralph.yml`, `@fix_plan.md`, `@AGENT.md`,
  `WORKTREE_BACKEND_PLAN.md`, `WORKTREE_BACKEND_README.md`) and dead code with no
  references from source or tests — `dl.get_git_branches`, `dl.workspace_status`,
  `dl.get_remote_head_sha`, `worktree.config.save_config`,
  `BranchManager.checkout_branch` and `BranchManager.create_remote_branch_via_ssh`.
- The README's "Backend Selection" section, which documented a `--backend` flag
  and `DEVLAUNCH_BACKEND` env var that exist nowhere in the code.

## [0.0.7] - 2026-08-06

### Added
- `--devcontainer <variant|path>` to select a non-default `devcontainer.json`, for
  repos carrying several variants. A bare name expands to the spec's
  `.devcontainer/<name>/devcontainer.json`; a path is used as given. Accepts
  `--devcontainer=x` too, and tab-completes the repo's variant directories. devpod
  stores the choice with the workspace, so it only has to be passed once.
- `DEVLAUNCH_WORKSPACE_ID` is injected into workspace initialization (via devpod's
  `--init-env`), so a project's host-side `initializeCommand` can tell branch
  workspaces apart. devpod passes the hook no workspace identity of its own, and
  devlaunch clones every branch to `<repo>/<branch>`, so a project deriving
  per-checkout names from the path cannot distinguish them. See
  `docs/devcontainer-projects.md`.
- Worktree backend for efficient multi-branch workspace management
  - Clones repositories once, then creates git worktrees for each branch
  - Shares git objects across all branches for faster workspace creation
  - Automatic backend selection based on workspace spec (owner/repo format uses worktree)
  - Backend override via `--backend worktree|devpod` flag or `DEVLAUNCH_BACKEND` env var
- New worktree module with:
  - `RepositoryManager` for cloning and managing base repositories
  - `WorktreeManager` for creating and managing git worktrees
  - `WorkspaceManager` for DevPod workspace lifecycle with worktree backing
  - `BranchManager` for branch operations (create, track, push)
  - `MetadataStorage` for persistent worktree tracking
- Configurable worktree directories via `~/.config/devlaunch/config.toml`
- `--purge` command to remove all devlaunch data (repos, worktrees, caches)
- All data now stored in `~/.cache/devlaunch/` (XDG compliant)

### Fixed
- Cloning a git-lfs repository no longer fails during checkout. Workspaces are
  cloned from the local bare cache, which holds no LFS objects, so the smudge
  filter aborted; LFS content is now pulled from the real remote after the origin
  URL is set. A failed or interrupted pull is retried on the next run — whether
  content is missing is decided by looking for pointer files, so a workspace
  cannot get stuck holding pointers.
- `dl <ws>` no longer starts the session in `$HOME` for projects that set a custom
  `workspaceFolder`. It passed a guessed `--workdir /workspaces/<id>`, and devpod
  falls back to `$HOME` when that path does not exist in the container.
- `dl <ws>` no longer opens VS Code on top of the terminal shell it attaches when
  devpod's default IDE is configured. `dl <ws> code` is unaffected.
- A failed `devpod delete` no longer strands a workspace. devpod re-parses the
  workspace's `devcontainer.json` to tear the container down, so deletion fails if
  that file has moved — and the local clone was removed regardless, leaving devpod
  with no config to retry from. The clone is now kept unless devpod succeeded.
- Proper exception handling for workspace creation failures
- Pylint compliance for all worktree module code

### Removed
- `devlaunch.dl.get_container_workdir()`. It built a guessed container path that
  is no longer passed to `devpod ssh` (see the `workspaceFolder` fix above), so it
  had no correct use. `workspace_ssh(workdir=...)` still accepts an explicit
  override.

## [0.0.4] - 2026-01-18

### Added
- Branch completion and auto-creation for `dl` command
- Support for multiple branch workspaces

### Fixed
- Use SSH for git operations instead of HTTPS
- Type checker None check in tests

## [0.0.3] - 2026-01-17

### Changed
- Updated README to match current CLI syntax and `--help` output

### Added
- PyPI badge to README

## [0.0.2] - 2026-01-17

### Added
- `--version` flag to display version information
- Comprehensive tests and improved coverage

### Changed
- CLI to workspace-first syntax (`dl <workspace> <command>`)
- Reorganized restart/reset/recreate commands

### Removed
- `nocache` command (devpod doesn't support it)

## [0.0.1] - 2026-01-17

### Added
- Initial release of DevLaunch
- `dl` CLI wrapper for devpod workspaces
- Commands: `up`, `ssh`, `stop`, `delete`, `status`, `restart`, `reset`, `recreate`
- Shell completion support with `--install` flag
- Fuzzy workspace selection via `iterfzf`
