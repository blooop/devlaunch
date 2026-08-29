# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **The JSON writers now escape DEL, as CPython does.** `metadata.json`,
  `completions.json` and `dl --ls --json` are all written to agree byte for byte
  with what the Python build wrote, and one character disagreed: `U+007F`. The
  escaper's gate was `is_ascii()` where CPython's is `' '` through `'~'`, so DEL
  went out as a raw byte where `json.dumps` writes the six characters `\u007f`.
  It is the only non-printable ASCII character serde hands to the escaper instead
  of escaping itself, which is how it stayed wrong through three copies of the
  loop.

  No branch name can carry a DEL (git rejects control characters in ref names),
  so nothing in the wild hit it going out. Coming back in did: the metadata loader
  parses and re-encodes, so a `metadata.json` written by the Python build with an
  escaped DEL in it re-saved as a raw byte, quietly breaking the round-trip the
  file's own contract asserts. Each of the three writers is now pinned against the
  line `json.dumps` printed for every ASCII character at once, which says the same
  thing in the other direction too: nothing in `' '..'~'` is escaped that Python
  leaves bare.

- **A session that dies badly no longer takes your terminal with it.** A terminal
  is not only a stream: a full screen program switches modes on in the emulator for
  its own use, the kitty keyboard protocol, bracketed paste, mouse reporting, the
  alternate screen, and undoes them on the way out. One that is *killed* never
  gets to, and those modes live in the emulator rather than in the connection, so
  nothing between the program and the glass undoes them.

  That is what you were seeing when a container went away underneath a live
  session. devpod says `error tunneling to container: exit status 137`, the agent
  inside dies unceremoniously, and what is left behind is baffling rather than
  obviously broken: ssh restores the tty settings on its way out, so the shell
  still echoes and still edits lines, and yet Ctrl-C does nothing and ordinary keys
  print things like `9;133u` at the prompt. Those are kitty keyboard protocol key
  reports, still switched on. Ctrl-C is one of them, arriving as an escape sequence
  instead of as the byte that raises SIGINT.

  `dl` is the last process holding that terminal, so `dl` now repairs it. Every
  session ends with a short restore written to the terminal, over either transport,
  and the interrupt handler writes the same thing before it exits. It goes out
  after a clean session too, because ssh's exit status does not say whether the far
  end cleaned up after itself, and that costs nothing: every sequence in it is
  chosen for doing nothing when the mode it names is already off. Nothing is
  written when `dl`'s own output is not a terminal, so `dl <ws> -- ls > files.txt`
  keeps its output free of escape sequences.
  [docs/cli.md](docs/cli.md) has the detail, including the one sequence that is
  deliberately not in the set.

## [0.24.0] - 2026-08-28

### Changed

- **`dl <ws> kill` deletes the workspace it just unwedged.** The sweep and the
  `rm` typed after it were never independently useful: clearing the lock is
  precisely what lets a `devpod delete` through, and a workspace wedged badly
  enough to need the hammer is one you are throwing away. So `kill` now ends in the
  `rm` verb's delete.

  **And that delete neither refuses nor waits, which is what the word is for.**
  There is no `dl <ws> kill --force`, because the guard would refuse in exactly the
  case the verb exists for: a wedged workspace's clone is dirty almost by
  construction, since whatever wedged it interrupted the work going on in it. So
  the guard still runs and its finding is *reported* rather than acted on, and the
  line naming what is about to be destroyed is printed before the delete rather
  than after it. `rm` is untouched and remains the happy path, guard, `--force` and
  all. Alongside that, `kill`'s call passes devpod's own `--force`, so a workspace
  whose container devpod can no longer reach is deleted rather than refused, and it
  is the one delete in dl that carries a deadline, so a holder arriving between the
  sweep and the delete cannot put the verb back into the lock loop it was typed to
  escape. If that deadline fires, dl says the workspace and its clone are still
  there and that running `kill` again picks up where it stopped.

  **What withholds the delete is a holder the sweep could not clear**, which is a
  question about the host rather than about what the sweep managed to signal. A
  live `devpod up` is one: the sweep spares somebody's build on purpose, along with
  its containers and its busy marker, and a delete over it would undo all three.
  A process with nothing waiting on it is the other, whether it sat through SIGKILL
  or lost its parent while the sweep was running. Both hold the flock, and devpod's
  acquire has no deadline behind it, so a delete attempted anyway would answer a
  hang with a hang.

  A session is neither, and that is the reported failure: an attended `devpod ssh`
  takes the flock and gives it straight back, which is why `dl <ws> rm` deletes a
  workspace somebody is sitting in without ever noticing them. One live ssh session
  was enough to make `kill` do nothing at all, and the `rm` typed next deleted the
  workspace unaided. The report now names every holder left standing and which of
  the three it is, and sends you back to `kill` once they are gone.

  The exit code is now the delete's rather than the sweep's, which is the stronger
  reading of the same thing: a `dl <ws> kill && ...` that used to mean "the
  workspace is free" now means "the workspace is gone". `dl kill` from the picker
  deletes every row marked with TAB, the way `dl rm` over the same rows does.

- **An `rm` that meets the wedge points at `kill`**, in both of the ways it can
  meet one.

  A delete devpod *refuses* cannot, from outside, tell a devcontainer.json that
  moved from a workspace something on this host is still holding, so the sentence
  now offers both ways out instead of the first one alone. A `kill` whose own
  delete is refused prints no such line: the sweep it would be asking for is
  already on screen above it.

  A delete devpod cannot get the lock for is the harder half, and the one the
  report that prompted this actually hit. It never refuses. devpod waits on that
  lock with no deadline, logging `Trying to lock workspace` every five seconds for
  as long as the holder lives, so there is no exit code for anything downstream to
  read and the run has to be Ctrl-C'd. dl now reads devpod's stderr as it arrives
  and answers the first of those lines while the command is still blocked, naming
  the `dl <ws> kill` to run in another terminal. Once, however many times devpod
  says it.

## [0.23.0] - 2026-08-28

### Added

- **`claude` in a workspace starts logged in.** A workspace whose image carries no
  Claude credential of its own used to open on a login prompt, every time, and the
  login it asked for was one the host already had. `dl` now forwards the host's
  access token as `CLAUDE_CODE_OAUTH_TOKEN`, read from `~/.claude/.credentials.json`
  or from an inherited variable of the same name.

  What it forwards and where is deliberately narrower than the `GH_TOKEN` flow
  beside it. The refresh token stays on the host, so what travels is short-lived
  and cannot be renewed from inside. It rides `--send-env` on the two session
  transports `dl` itself opens, so only the variable's name ever reaches a command
  line, nothing is written to the container's disk or to devpod's persisted
  workspace config, and a `postCreateCommand` from a repo you did not write never
  sees it.

  It also declines rather than guesses. A probe reads `/proc/self/mountinfo` for
  mounts at, under or above the container's Claude config directory, honouring
  `CLAUDE_CONFIG_DIR`, and the token is forwarded only when that comes back an
  affirmative "nothing of anyone else's is mounted here". A devcontainer that binds
  its own `~/.claude` in, as this repo's does, is left alone, because Claude Code
  prefers the variable to the file and forwarding would shadow a refreshable
  credential with an expiring one. Every reading that is not affirmative, including
  every way the probe can fail to tell, forwards nothing: the failure direction is
  a login prompt, never a leaked or clobbered credential.

  The answer is memoized per workspace and anchored to the container's result-file
  mtime, so a rebuild re-asks. A workspace that predates this needs one
  `dl <ws> up` before it picks the login up. `DEVLAUNCH_NO_CLAUDE_TOKEN=1` skips
  the whole thing, and
  [docs/workspace-tools.md](docs/workspace-tools.md) has the trust model, including
  who gets the token when the repo is a stranger's.

### Changed

- **Remote Control is on by default, so every `aid` claude session is one you can
  pick up on your phone.** It shipped in 0.21.0 as `--remote-control`, and a flag
  you have to remember is a flag you use once:

  ```
  aid blooop/devlaunch@fix/42 fix the flaky test
  ```

  now starts the session named `blooop/devlaunch@fix/42` on claude.ai/code and in
  the Claude app, with nothing typed. **This changes what an existing `aid
  <workspace>` does**, which is the point, and it is worth reading the next two
  paragraphs before upgrading.

  `--no-remote-control` is the way back to a plain local session, and
  `DEVLAUNCH_AID_REMOTE_CONTROL=0` is the way back for every launch from a shell.
  The variable takes `1`, `true`, `on` or `yes` and `0`, `false`, `off` or `no`,
  and refuses anything else by name rather than guessing which you meant; a flag
  on the command line beats it in both directions. `--remote` and `--no-remote`
  are accepted as the short spellings, because the long one is four hyphenated
  words and the guess at it used to fall through to `dl` and exit 2. Either switch
  can be appended to the end of a line the way `--rm` can, so a recalled line can be
  turned off without retyping the front of it, and the prompt survives it.

  **`aid` runs claude with permissions skipped, so a default-on session is
  drivable by the claude.ai account signed in inside the container.** That is the
  whole of the change in one sentence, and the variable above is how to turn it
  off globally.

  A default is not a request, so `aid --codex` and `aid --gemini` start with no
  Remote Control and say nothing: those two have not got the feature, and a
  default that refused would have refused every launch of them. Typing
  `--remote-control` beside either still exits 1 naming the agent, because that
  is somebody asking for a thing by name.

## [0.22.0] - 2026-08-28

### Added

- **`dl <ws> rme`: the delete, and then the shell.** A workspace tab reaches the
  same end every time. You are done with the branch, you type `dl <ws> rm`, you
  wait out a container teardown, and then you type `exit` to close a tab that has
  had nothing left to do since the delete started. `rme` is that pair as one word:
  the `rm` verb, and on a removal that worked a SIGHUP to whatever started `dl`,
  which ends an interactive shell and takes its terminal with it.

  ```
  $ dl blooop/devlaunch@fix/x rme
  Removing workspace devlaunch-fix-x-1a2b...
  Removed workspace clone: ~/.cache/devlaunch/repos/blooop/devlaunch/devlaunch-fix-x-1a2b
  Removed local clone for devlaunch-fix-x-1a2b
  Removed workspace devlaunch-fix-x-1a2b.
  Hanging up the shell dl was called from (pid 48213).
  ```

  The removal is `rm`'s, guard and `--force` and exit codes included, and the
  hangup is reached only if it came back clean. `--force` is the exception, since
  it asks for absence rather than a removal: a forced `rme` of a workspace that
  was never there succeeds and still closes the shell, which is `rm --force`'s
  existing hazard with the receipt's reader removed. That is the whole of the
  ordering: every way a delete can stop writes a sentence to stderr, and closing
  the window it was written to is a guaranteed way for nobody to read it. A guard
  that refused, or a devpod that would not finish, leaves the shell standing with
  the reason on screen and the workspace still there to retry. `dl rme` with five
  rows marked removes all five and hangs the shell up once, when the last of them
  has gone.

  What it hangs up is `dl`'s parent process, because there is no way to ask
  whether a parent owns a terminal, and which process that is depends on the shell
  rather than on the line: a subshell running one command is usually replaced by
  it, so `$(dl <ws> rme)` closes your terminal too, while the same line with a
  redirection leaves a subshell to take the signal. So `dl` names the pid instead
  of guessing. `nohup dl <ws> rme` is refused outright and says so, because
  disarming SIGHUP is how a run outlives its terminal in the first place and `dl`
  already honours that for its own handlers. A shell hung up this way takes its
  background jobs with it, which is what a shell does on SIGHUP, so `rme` is for
  the tab that has nothing left in it and `rm` for the one you are still working
  in.

  Neither of the two spellings already there: `rm` deletes now, `--rm` deletes
  when a session ends, and this one deletes now and then ends the *shell*.
  [docs/cli.md](docs/cli.md) has the contract.

## [0.21.0] - 2026-08-28

### Added

- **`aid --remote-control`, for a session you can carry on from your phone.**
  Claude Code's Remote Control makes a running session readable and steerable
  from claude.ai/code and the Claude app, and `aid` now has a flag for it:

  ```
  aid --remote-control blooop/devlaunch@fix/42 fix the flaky test
  ```

  The session is named after the workspace you typed, so the list on claude.ai
  reads as the workspaces you opened. That name is not optional here even though
  claude's flag makes it so: `claude --remote-control [name]` would take the
  first word of the prompt as the name, so `aid` always sends
  `--remote-control=<workspace>` as one word.

  Nothing about where the work happens changes. The agent is still the process
  in the container on your machine, so stopping or removing the workspace takes
  the session offline; the entry lingers in the claude.ai list for roughly
  4 hours afterwards, which is the web side timing out and not something still
  running.

  It is claude's feature alone, so `--codex` or `--gemini` beside it is refused
  by name before anything boots, as is a `DEVLAUNCH_AID_AGENT` naming either of
  them. It also wants a claude.ai (Pro, Max or Team) login inside the workspace:
  an API key cannot pair a session.

## [0.20.0] - 2026-08-26

### Added

- **`dl <ws> kill`, for a workspace that will not respond.** A `devpod up` that
  outlives the `dl` which started it holds the workspace `flock` forever, and
  until now nothing in `dl` got you out of it:

  ```
  $ dl restart
  09:58:51 info Trying to lock workspace, seems like another process is running that blocks this workspace
  09:58:56 info Trying to lock workspace, seems like another process is running that blocks this workspace
  ```

  That line is not a retry with a timeout behind it. devpod takes a blocking
  `flock` and logs the same string on a timer while it waits, so it waits for as
  long as the holder lives, and an init-reparented process is never reaped. The
  existing flags were all the wrong shape: `--force` means "delete despite
  unpushed work", and `--purge` removes every workspace on the machine, which is
  not what you want when one of them is wedged. The way out was `ps`, a `kill -9`,
  and knowing which of the two files named `workspace.lock` was the safe one.

  `kill` reads the host's own process table and acts on what is there. It kills
  the `devpod` processes that name this workspace and whose own parent has died,
  SIGTERM first and then SIGKILL for whatever sat through it; removes devpod's
  stale busy marker, but only once nothing is left holding the workspace; kills
  any container the workspace's compose project still has running; and prints
  every one of them, with the pid and the whole command line. It exits `0` only
  when nothing is left holding the workspace, so `dl <ws> kill && dl <ws>` is safe
  to type.

  Two things it deliberately does not do. **It never touches the lock file.**
  Killing the holder is what releases it, because the kernel drops an `flock` when
  its holder dies. Unlinking it would be worse than the hang: the old holder keeps
  a lock on an inode nobody else can see, the next caller locks a fresh file, and
  two processes both believe they have the workspace. And **it leaves a live build
  alone**, containers and process both, because those are that build's and killing
  what it is in the middle of creating would break it as surely as signalling it
  would. The report says so in both cases.

  It also asks devpod nothing when you name a workspace by id. Every other
  lifecycle verb resolves its target through a `devpod status` with no deadline
  behind it, which on the host this verb exists for is the same wait arriving one
  call early. A workspace id is already the id, so there is nothing for a round
  trip to settle, and a name devpod has never heard of sweeps nothing and is
  reported as a workspace nobody is holding. `dl owner/repo kill` does have to ask
  which workspace that resolved to, and gives that one call five seconds before
  falling back to the derived id. The two docker calls carry deadlines for the
  same reason: a daemon that never answers must not swallow the report of a
  SIGKILL that has already landed.

  `kill` takes several workspaces from the selector, the way `stop` and `rm` do. A
  machine that was suspended, or one whose `dl` the OOM killer took, wedges every
  workspace that was open at the time.

## [0.19.1] - 2026-08-26

### Fixed

- **The delete guard counted tags, so a repository that tags releases could not
  have a workspace deleted at all.** `dl <ws> rm` asks git what the clone holds
  that no remote does, and the question spanned every ref in the clone, `refs/tags`
  included. Tag a release, merge the branch, delete it — the ordinary shape of a
  release — and the tag is the last ref reaching those commits, so no
  `refs/remotes/*` contains them and the guard reads them as work that exists
  nowhere else. One repository carries 265 such commits, and six of the eight
  workspaces on a host would not be deleted because of them, reporting 265 to 269
  apiece and none of it real:

  ```
  kinisi-ros-feat-bt-with-wheels-on-brake-4ixn holds 265 unpushed commit(s).
  Push or commit it, or run: dl ... rm --force
  ```

  Read once, that is a clone kept when it could have gone — disk against the cost
  of the work, and the safe direction to fail in. Measured, it is a guard that
  cannot be satisfied by pushing anything, on every workspace of that repository
  forever, which teaches `--force` as the ordinary way to delete one. The clone
  that does hold an unpushed hour of work goes the same way, unread.

  `refs/tags` now comes out of the ref set and nothing else does, so local
  branches, every worktree's HEAD including detached ones, and `refs/stash` are
  asked about exactly as before (#485, and #471 keeps its answer). What is given up
  is a commit reachable only from a local tag, with no branch, worktree HEAD or
  stash in the clone naming it too.

## [0.19.0] - 2026-08-25

### Changed

- **The selector is a table, headings and all.** It had columns already, and two of
  the three were padded to a common width. What it did not have was a line saying
  what they were, or a promise that a column starts in the same place on every row:

  ```
  Select workspaces (type to filter, TAB to mark several):
  OWNER  | REPO             | BRANCH                  | WORKSPACE
  blooop | devlaunch        | main                    | devlaunch-main-3j1t
  blooop | devlaunch        | main                    | devlaunch-main-legacy
  blooop | wayfinder        | wayfinder/devlaunch-467
  myfork | bencher          | fix/thing
  -      | someones-project
  ```

  Three things moved to get there. The branch column is padded now, which is what
  lets the fourth column line up in the case that draws one: two workspaces of one
  repository on one branch, where both rows say their whole id. The heading is
  measured as a cell, so a column of short owners is padded out to `OWNER` rather
  than the word overhanging the column it names. And the headings are drawn in the
  picker's own header, beside the invitation, rather than offered as a row: a
  heading in the list would be filterable, markable with TAB and pickable, in a
  picker `dl rm` opens.

  A column is named exactly when some row has something in it, so a list of nothing
  dl cloned is headed `OWNER | REPO` and stops there.

  The one behaviour change under the rows: a workspace dl did not clone keeps
  devpod's name for it, and that name now sits *in* the repo column instead of
  running on through the space a repo and a branch would have taken. It is measured
  for that column too, so a long foreign name widens it for every row, which is the
  price of one grid and the same price a long repository name has always cost. It
  reads `REPO` over a workspace name, which is the one place the heading is looser
  than the cells under it: the alternative puts the only thing that row says
  furthest right on the line, after two empty cells.

## [0.18.0] - 2026-08-25

### Changed

- **Tab completion of the first word offers owners before workspace ids, so an
  owner completes past the ids of its own repository.** `dl kin<TAB>` used to stop
  at `kinisi-ro` and print a screen of workspace ids. Both namespaces were in one
  `compgen -W` list, and they collide for a single repository: an id is
  `<repo-slug>-<ref-slug>-<suffix>` and the slug turns `_` into `-`, so
  `kinisi-robotics/kinisi_ros` derives ids beginning `kinisi-ros`, against an owner
  named `kinisi-robotics`. bash completes to the longest prefix its candidates
  share, which was those nine characters. No fork and no second owner were needed
  to cause it, and typing more did not help until the two spellings diverged.

  The owner wins the tie because it continues: `/` is the next keystroke and the
  repository completes from there, so `dl kin<TAB><TAB>` now reaches
  `kinisi-robotics/kinisi_ros`. Workspace ids are held back rather than dropped,
  and for a prefix no owner matches they are what is offered, which is the case
  that had to keep working: an id pasted or half-typed out of `dl --ls`, since
  `dl <id>` is still a way to name a workspace. One keystroke swaps the list, so
  `dl kinisi-ros<TAB>` offers the ids and nothing else. The hold-back applies to a
  prefix only: `dl <TAB>` with nothing typed lists both, as it always did, and an
  id typed out in full is offered beside the owner it shares a name with, for
  which no longer prefix exists.

  A completing id also gets its trailing space back. The old branch suppressed the
  space for every candidate because some of them were owners, which need the
  cursor left against the `/`; now only the owners do, and an id can be followed
  straight by a verb.

  Nothing about `completions.bash` changed, so an older `dl` writing the cache and
  this script reading it agree, and the reverse.

## [0.17.0] - 2026-08-25

### Changed

- **The terminal tab reads `devlaunch@main` again, and the selector never gives up
  a branch name to keep two rows apart.** Both are the same correction: one
  workspace id, rendered for the surface that is reading it, rather than one string
  everywhere.

  The tab is the id with the two characters a glance cannot use taken off: the
  four-character identity suffix, and the dash between the repo and the branch,
  spelled `@`. `dl blooop/devlaunch` names the pane `devlaunch@main` where devpod,
  the container hostname and the `WORKSPACE` column of `dl --ls` all still say
  `devlaunch-main-3j1t`. It is a rendering of the id and not a second derivation of
  the spec, so the two are one string with two characters changed and a tab still
  matches a listing row by eye. The branch stays the id's slug, cut to the id's
  budget, which is what keeps a 200-character branch from making a
  200-character tab. The full spec, `owner/repo@branch`, had no such bound and is
  not what came back.

  What it costs is a name that is no longer a pure function of the workspace across
  every way of reaching it. Which dash the `@` replaces cannot be read off an id,
  since a repo slug holds dashes of its own, so the name travels with the launch that
  resolved the branch. A workspace named by its bare id on the command line is titled
  by that id, and a workspace opened both ways carries two profile lines with the
  last one winning.

  The selector is not one of those, and it is the one that matters, since `dl` with
  no arguments is how a workspace is reopened. It hands the launch an id like any
  other caller, but it read the owner, the repo and the branch to draw the row, so
  the pick carries them and the tab reads `devlaunch@main` either way. A triple that
  no longer derives that very id, which is what a `git switch` inside the container
  leaves, is dropped in favour of the id.

  In the selector, two rows that would be drawn alike now gain a fourth column
  holding their whole id instead of collapsing into it:

  ```
  blooop | devlaunch | main | devlaunch-main-3j1t
  blooop | devlaunch | main | devlaunch-main-legacy
  ```

  Both of those rows are on `main`, so the branch was never what was ambiguous, and
  taking it off screen left a person picking between two ids. `dl rm` is one of the
  verbs the selector opens for. Only the rows that collide grow the column.


## [0.16.0] - 2026-08-25

### Changed

- **A delete now says which workspace it deleted.** `dl rm` from the picker said
  nothing that named its own work: skim hands back a workspace id and takes its
  screen away, and the only thing naming the workspace afterwards was `devpod
  delete`'s own line on stdout — devpod's wording rather than dl's, on the other
  stream from every line dl says about the same delete, and absent entirely for a
  workspace with no clone recorded, where none of the clone notices fire either.
  The delete now names it going in, once the unsaved-work guard has passed so a
  refusal is never announced as a removal, and again on the way out, closing the
  block so a batch reads by its ends.

  A pick names **the row it took beside the id it resolved to**
  (`Picked blooop | devlaunch | main -> devlaunch-main-3j1t`), and both halves
  are load-bearing: an id is `<repo-slug>-<ref-slug>-<suffix>` and carries no
  owner, so a fork and its upstream are one id apart only in the hashed suffix
  the picker deliberately never draws. Reporting the id alone handed the user a
  name they could not check against the row they chose. A batch of TAB-marked rows
  is listed under a heading before the first workspace is touched, which is the
  only thing in the run that says how many rows were taken.

  `--force` closes with `Workspace <id> is gone.` rather than `Removed`, because
  that is what its exit code proves. It passes devpod's `--ignore-not-found`, so
  "there was nothing there" succeeds and cannot be told apart from a real delete,
  and a path is resolved without asking devpod anything at all — so
  `dl ./wrong-directory rm --force` reaches the delete and comes back successful.
  Saying `Removed` there would be dl affirming a delete that never happened.

## [0.15.0] - 2026-08-25

### Changed

- **Every workspace id moves, and the clone directories are renamed onto the new
  ones.** The identity suffix was eight characters spelled in pronounceable
  syllables (`devlaunch-main-zovomobo`); it is now four characters of base 36
  (`devlaunch-main-3j1t`). Base 36 carries 5.17 bits per character against the
  syllable table's 3.0, so the suffix halves in length and the four
  characters it gives back go to the branch, which is the part anyone reads: the
  ref budget goes from 17 characters to 21, and
  `kinisi-ros-ags-devcontainer-tooling-su-lenevere` becomes
  `kinisi-ros-ags-devcontainer-tooling-suppor-17uu`.

  What it costs is collision headroom, 20.7 bits against 24. The population that
  can actually collide is one repository's near-identical long refs, since two
  triples must also truncate to the same readable half, so ten of those carry a
  0.003% chance, one in thirty-seven thousand. Five hundred, the crop that broke
  an earlier 18-bit width, would be 7%. A collision is not detected today and
  means one clone directory and one devpod workspace shared silently;
  blooop/devlaunch#438 is the guard for that.

  `metadata.json` goes to schema 3, and the migration renames every clone
  directory onto its new id, so **uncommitted work survives**. The containers
  cannot be renamed in place: they keep their old ids, are reported as orphaned,
  and are listed in `orphaned-workspaces.txt` for `dl --reconcile` and
  `dl <workspace> recreate`.

- **One workspace, one name.** The terminal tab, the container hostname and the
  `WORKSPACE` column of `dl --ls` are all the workspace id now. They used to be
  three different strings: the tab carried `owner/repo@branch`, the hostname
  carried the id with its suffix dropped, and only the listing carried the id
  itself. A tab and a listing row can now be matched by eye.

  What that costs is real. The tab no longer names the owner, so a fork and its
  upstream read alike, and it spells the branch as a slug. The spec was the better
  name read on its own, but only one of the four launch arms ever had one: a bare
  devpod name, a path and a URL never form a triple, so three quarters of launches
  were titled by id anyway and the tab's shape depended on how the workspace had
  been reached. The tab was also unbounded, since nothing validates a ref's length,
  so a 200-character branch made a 200-character tab.

  The hostname is 47 characters where it was 38, which leaves 17 bytes of the
  64-byte limit for tools that stack their own prefixes onto the container name.

- **The picker reads the branch out of the clone instead of out of the id.** The
  `<owner> | <repo> | <branch>` columns used to be recovered by taking an id apart:
  strip the repo prefix, strip the suffix, slug what is left. That answered with a
  slug, so `feature/auth` and `feature-auth` drew one row twice and a long branch
  drew short, and it only worked because the syllable suffix had a shape a parser
  could recognise, which base 36 does not. The branch comes from the clone's own
  `HEAD` now: exact, slashes intact, one small file read with no subprocess and no
  metadata lock. It also reports the branch that is checked out *now* rather than
  the one the workspace was created for. Nothing reconstructs a triple from an id
  any more.

- **zellij is installed only into workspaces that asked for it, and
  `DEVLAUNCH_ZELLIJ=1` is the ask.** It used to go into every container `dl`
  opened. The seconds are real — the stage costs 2.2s warm to 3.5s cold of a
  setup pass, and **1.70s of that is bootstrapping pixi**, not installing zellij
  — but they are not what decided it. What decided it is that the install was
  opt-out while every use of it was opt-in, so the default combination paid for a
  capability the same defaults guaranteed nothing would touch. `DEVLAUNCH_ZELLIJ`
  now means "I want zellij in my containers" rather than "wrap this command", and
  one variable answers both halves: the setup pass carries the stage, and a
  `dl <spec> -- <cmd>` still makes sure the session exists first.

  Two costs, said plainly. A documented guarantee shrinks: workspaces no longer
  all have zellij, only the ones that asked. And an interactive attach that used
  to be able to type `zellij attach -c devlaunch` now needs the variable set, once
  in a shell profile, or gets `command not found`.

  Setting it on a workspace that is already up lands the stage on the next
  `dl <ws> up`, with no restart and no extra machinery: the verdict cache already
  records which switches a pass ran under, so a launch that wants the stage does
  not trust a marker written by one that skipped it and the top-up pass travels
  carrying the stage. An attach against a running workspace picks nothing up
  whatever shape it takes: `dl <ws>` and `dl <ws> -- <cmd>` both go straight to the
  attach, so opting in and immediately running a command against a container that
  is already up wraps it in a zellij that is not there yet.

- **`DEVLAUNCH_NO_ZELLIJ` is retired.** With skip as the default it had no
  remaining job. A stale `DEVLAUNCH_NO_ZELLIJ=1` in a shell profile is read by
  nothing and, in particular, can never turn provisioning back on for somebody who
  had asked for it off. `DEVLAUNCH_NO_TOOLS=1` still overrides a launch that did
  ask, because installing zellij is still tool provisioning.

### Fixed

- **`dl` looks for devpod's ssh host aliases where devpod writes them, so the pty
  transport stops disappearing in silence.** `dl <ws> -- <cmd>` runs over OpenSSH
  through the alias `devpod up` publishes, and `dl` decided whether that alias
  existed by reading a hardcoded `~/.ssh/config`. devpod does not necessarily
  write there: it targets the `SSH_CONFIG_INCLUDE_PATH` context option, else
  `--ssh-config` (whose default it fills in from `$DEVPOD_SSH_CONFIG`), else the
  `SSH_CONFIG_PATH` context option, else `~/.ssh/config` — and it writes to
  whichever it picks and to *no other*. So on any host that exports
  `DEVPOD_SSH_CONFIG` there was no `~/.ssh/config` to find, `dl` concluded the
  workspace had no alias, dropped every command to `devpod ssh --command` (no
  pty, `TERM=dumb`, interactive programs exiting on sight), and the only thing it
  said was that the *workspace* needed restarting. This repo's own scratch
  convention, `test/conftest.py` and `scripts/bench_launch.py` all export that
  variable, so the transport was unreachable on exactly the hosts we measure on:
  **before/after numbers from a run that set `DEVPOD_SSH_CONFIG` are not
  comparable across this change**, because the earlier side of them was silently
  on the other transport.
  The two context-option paths are read from the copy `dl` has already cached and
  never by a fresh `devpod context options`: that trip costs 0.4-0.7s (#393) on a
  path where the whole decision is worth less, and a cache miss falls back to
  devpod's own defaults, which is what `dl` assumed unconditionally before.
  And the silence is gone from the type rather than from a log line. `Terminal`'s
  one `NoAlias` arm was two facts wearing one name; it is now `NoAlias` (this
  config has no entry for this workspace, and `dl <ws> restart` republishes it),
  `ConfigMissing` (there is no ssh config where `dl` expects devpod to publish,
  named in the notice, and a restart helps only if devpod writes to that same
  file) and `ConfigUnlocatable` (no home directory and nothing naming a config,
  so there is nowhere to look). Each has its own sentence, and each names the
  path `dl` read. (#421)
- **`dl` hands OpenSSH the ssh config it read the alias out of, as `-F <path>`.**
  The other half of the same bug, and the half that made it worse rather than
  quieter. OpenSSH reads neither `$DEVPOD_SSH_CONFIG` nor `$HOME`; it resolves the
  default user config through `getpwuid(getuid())`. So once `dl` started deciding
  the alias existed by reading devpod's real config, the invocation it built still
  pointed OpenSSH at `~/.ssh/config`, and on a host where devpod publishes
  elsewhere `ssh -t <ws>.devpod <cmd>` failed with `Could not resolve hostname` at
  exit 255: the command did not run at all, where before it had merely run without
  a terminal. The config now travels inside `Terminal::Usable` and
  `ssh::command_args` takes it as a parameter, so an invocation with no `-F` is not
  expressible. **The trade:** `-F` makes OpenSSH read that file *instead of*
  `~/.ssh/config`, and it skips `/etc/ssh/ssh_config` unconditionally, including
  when the file named *is* your own user config. So a system-wide `Host *` block
  never applies to a devlaunch session, and one of your own stops applying as soon
  as devpod publishes somewhere other than your user config. Taken deliberately: `dl` cannot know whether `ssh` would have
  read the file `dl` read, since `$HOME` and `getpwuid`'s home are allowed to
  differ, and devpod's own block is self-contained. The e2e suite's `ssh` shim,
  which had been supplying exactly this `-F` and hiding the defect from a green
  run, is deleted. (#421)

## [0.14.0] - 2026-08-25

### Fixed

- **`dl --reconcile` no longer reports an adoption as landed when it wrote no
  record.** Re-pointing devpod at the clone and recording the worktree are two
  steps, and the second can find nothing to update — another run removed the
  record while the plan sat there waiting to be applied. That case was reported
  as `Repointed` like any other, with the run finishing successfully, so the one
  outcome worth knowing about looked identical to the ordinary one. It is now its
  own `Unrecorded` arm: devpod re-pointed, dl's record not written, said on
  stderr, and the run does not report itself finished. A store that refused the
  write lands in the same arm for the same reason — it is the same half-done
  adoption — and the refusal is named beside it.

### Added

- **CI fails when nothing reviewed a pull request.** Sourcery answers a quota
  refusal *as a review*, so `Sorry @blooop, you have reached your weekly rate
  limit…` arrives in the same shape as a review that found nothing. Twenty-six
  consecutive pull requests merged behind that sentence — the largest changes in
  this repo among them — and nothing anywhere said so. The `review` job, inside
  `gate`'s `needs`, asks whether the code was reviewed rather than whether
  Sourcery answered: a review by anyone but the author satisfies it, so does a
  `wf-review` report by the author (recognised by the provenance line those
  reports open with), and so does the `no-external-review` label. A quota outage
  lasts a week, and a gate that stopped a week of merges would be one somebody
  deletes. An author's plain "lgtm" does not satisfy it, and merging on a
  self-review is recorded as a notice on the run, and a review that predates the
  head is warned about rather than failed on.

### Fixed

- **claude no longer takes the terminal title back in a `dl` session it started
  itself.** dl names a pane after the workspace and claude renames it continuously
  from its own read of what the session is doing, so the two are one contest and
  claude wins every round after the first: dl's name was gone within about a
  second. The feature shipped with `aid` setting
  `CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1`, but only as a prefix on the claude it
  launched, which left every `claude` typed at a workspace's own prompt losing the
  name. The title stage now writes the variable into the container's login
  profile, so it reaches whoever starts claude without
  rewriting anybody's command — the one rule aid was right to keep. `DEVLAUNCH_NO_TITLE`
  governs all three pieces of the feature, and since a profile is read by shells that
  start after it is written, a workspace that was already up wants a re-login or a
  `dl <ws> recreate`.

- **`DEVLAUNCH_NO_TTY=FALSE` no longer means one thing to the prompt and another
  to the transport.** `dl` gates its own terminal behaviour on the same variable
  the ssh transport is gated on, and the two read it differently: core lowercases
  the value and strips it the way Python's `str.strip()` does before comparing it
  against the falsey words, while `dl` kept a copy that compared the raw bytes
  with a bare `matches!`. So `FALSE` and `" no "` opted out of the prompt and not
  out of the pty, and a non-UTF-8 value opted out of the *pty* and not the prompt
  — `std::env::var(..).ok()` reports such a value as unset, which is the
  opt-out-into-opt-in inversion `osext` exists to prevent and names in as many
  words. The copy existed because `osext` was `pub(crate)` and neither half of
  what makes the reading right — the lossy read or Python's strip — can be
  spelled without it, so the fix is a seam rather than a third copy:
  `clients::ssh::tty_disabled_by_environment` is binary surface, and `dl` asks it.
  The deleted tests only ever covered the spellings where the two agreed, which is
  why the divergence was invisible.

- **`aid` no longer starts the default agent when `DEVLAUNCH_AID_AGENT` holds a
  value it cannot decode.** The variable was read through
  `std::env::var(..).ok()`, which reports a value that is not valid UTF-8 as
  *unset* — so `DEVLAUNCH_AID_AGENT=$'\xff'` was not a broken agent name, it was
  no agent name at all. The default agent was chosen, the workspace was opened,
  and nothing anywhere mentioned the variable that had asked for something else.
  The `is not a known agent` refusal that `DEVLAUNCH_AID_AGENT=nope` already got
  was unreachable for exactly the values that cannot be written as a string. The
  read is a lossy decode now, so the undecodable byte arrives present under
  U+FFFD: a name to refuse rather than a variable to ignore, refused by name
  with no devpod call made.

- **On git 2.20 and older, a launch that asks for a new branch no longer fails
  outright.** Up to v2.20.0 git wrote `Couldn't find remote ref %s` through a
  bare `die()` (`remote.c:1785`) — capital C, and never routed through gettext,
  so the `LC_ALL=C` the fetch is pinned under could not normalise it; it is
  lowercase and translatable from v2.21.0. The reader that recognises "the
  remote has not got this ref" is the one that falls back to the default branch,
  and it matched only the later wording. On a host still running that git the
  ref-missing *answer* therefore read as a failure, the fallback never ran, and
  an ordinary "start a new branch" launch died on a message about a ref nobody
  had asked to exist yet. The refusal is classified from the verb that produced
  it now, and case does not decide it.

- **A repository that is not there on Codeberg or Forgejo now gets the
  wrong-owner hint.** Those hosts answer a 404 with `remote: Not found.`, and
  none of the three phrases the renderer sniffed for appear anywhere in that
  output — all three were GitHub's and GitLab's wordings. The only other line is
  git's own `fatal: repository '<url>' not found`, which does not contain the
  substring `repository not found` the renderer was looking for, so a mistyped
  owner on those hosts got the bare clone failure and no suggestion of what to
  try. The reader matches git's line as a whole line ending in `' not found`
  now, which is the host's wording out of the decision entirely, and still keeps
  out `branch '%s' not found`, `tag '%s' not found` and `repository '%s' does
  not exist`.

### Changed

- **devpod's on-disk layout is one module's, and that module now exists.**
  `clients::devpod` was the seam for devpod-the-command; devpod-the-filesystem
  had none, so `<devpod home>/contexts/<ctx>/workspaces/<id>/…` was rebuilt
  wherever it was needed — four times in `devlaunch-core`'s implementation and
  four more in its test fixtures, under a comment claiming it was spelled "in one
  place and not three". `clients::devpod_home::DevpodHome` owns it now, including
  the one write (`--reconcile` re-points a record's `source.localFolder`, which
  devpod v0.26.1 offers no subcommand for), and `devlaunch-core`'s new
  `tests/devpod_layout.rs` fails if any other module in the crate spells it
  again. `lifecycle::devpod_home()` becomes `DevpodHome::locate()`, and
  `lifecycle::RepointFailure` moves with the write that raises it;
  `api::workspace_delete`, `lifecycle::purge_all_data` and
  `lifecycle::apply_reconciliation` take a `DevpodHome` where they took a bare
  path. It also takes a test module back out of the crate's internal surface:
  `flows::lifecycle`'s `mod tests` was `pub(crate)` only so two other flows could
  import a devpod-home fixture from it, and the fixture now lives with the
  layout it builds.

## [0.13.0] - 2026-08-24

### Changed

- **The public-API freeze is three snapshots instead of one, so its diff means something
  again.** `devlaunch-core`'s snapshot splits into `public-api.api.txt` — the 37 declarations
  written at the `devlaunch_core::api` path, where a diff is a change to the promised contract —
  and `public-api.rest.txt`, the tripwire over the binary surface that a refactor may move
  freely. That guard is one-way, and the README says so: `cargo public-api` renders methods and
  impls only at a type's canonical path, so a promised type's constructors, methods and derived
  impls diff in the *rest* file (renaming `api::Launch::run` leaves the promise file
  byte-identical). Widening the classifier is #352.
  `devlaunch-runner` gets one of its own: the trait an external `Runner` implementer
  writes against used to enter core's snapshot as a single unexpanded glob row, so removing a
  method from it moved nothing and passed CI. `scripts/public-api-snapshots.sh` regenerates all
  three and is what CI runs, so the filter deciding which row is a promise, the `-ss` flag and
  the pinned `cargo-public-api` exist in one place; see "The public-API snapshots" in README.md.

### Fixed

- **A workspace brought up with `DEVLAUNCH_NO_ZELLIJ=1` no longer stays without
  zellij forever.** The verdict cache's marker recorded which container a pass was
  about and not which switches it ran under, so a pass that skipped the zellij
  stage still probed *provisioned* — rightly, since the probe is about the tools —
  and wrote a marker that the next launch, with no variable set, trusted. The trip
  was skipped, zellij was never installed, and no later top-up could notice,
  because every one of them read the same marker and skipped the same trip: silent
  for the life of the container. The marker now carries the switches, and one
  written under different switches reads as no verdict. A marker from an earlier
  build has no such field, fails to parse and is therefore untrusted — one
  redundant round trip on the first launch after upgrading, which is the direction
  this cache is allowed to be wrong in.

- **The dotfiles refresh's recovery can no longer make a workspace permanently
  unrecoverable.** When `chezmoi update` failed, the retry ran `chezmoi init`
  unconditionally — and `chezmoi init` with no repo argument `git init`s the
  source directory when it is not already a repository. The repo it makes has no
  upstream, so `update` then fails with `no tracking information` on that refresh
  and on every one after it, because each later attempt finds a repository and
  `init` is a no-op. One actionable error was converted into a permanent one with
  its own diagnosis destroyed. The retry now asks first, so a failure it cannot
  fix fails with the error that mattered.
- **The same retry can now fix the case it was written for.** chezmoi's `--force`
  is "make all changes without prompting", which is about changes and not about
  the `prompt*` functions a config template calls — so a dotfiles repo that added
  a `promptString` variable made `init` ask for it, and a refresh nobody is
  watching either died on `could not open a new TTY` or blocked on the question.
  `init` now carries `--promptDefaults`, and `--no-tty` so that a prompt with no
  default is a fast error rather than a hang inside the refresh's bound.

- **A `kill` or a closed terminal now runs the same cleanup Ctrl-C does.** Only
  SIGINT had a handler, so `kill <dl>` — a supervisor timing a run out, a CI job
  being cancelled, a shutdown sweep — and closing the terminal window (SIGHUP)
  both ended `dl` where it stood, leaving the staged plaintext `GH_TOKEN` file on
  disk and the `devpod up` child orphaned: the exact pair the SIGINT handler
  exists to prevent, and in SIGHUP's case unwatched, since the window any
  complaint would have appeared in is the one that just went away. All three
  signals now run the one async-signal-safe drain — kill the child's process
  group, unlink the registered temp files, `_exit` — and the code they exit with
  is **128 + the signal number**: 130 for Ctrl-C, 143 for a `kill`, 129 for a
  closed terminal. What still does not run on any of them is the `--rm`
  removal, which a signal handler may not do (#304).

  **`nohup dl …` still outlives its terminal.** A SIGTERM or SIGHUP that was
  already set to be ignored when `dl` started stays ignored and ends nothing,
  which is how `nohup` works and what keeps it working. Ctrl-C is deliberately
  not switchable the same way, and is unchanged from previous releases: a shell
  script backgrounding a job hands its child an ignored SIGINT whether anyone
  wanted one or not, so honouring it would silently stop the cleanup for every
  `dl` launched from a script or a CI step.

## [0.12.0] - 2026-08-23

### Removed

- **Reporting agent state to a host-side herdr session, added in 0.8.0, is gone** —
  the mount, the setup stage, the hook, the pane variables on the agent's command
  line and `DEVLAUNCH_NO_HERDR`. The container half needed a `python3` and most
  containers do not have one, which made the feature's majority case "installs
  nothing".

  It is the requirement rather than the code that decides this.
  `mcr.microsoft.com/devcontainers/base:ubuntu-24.04` — this repo's own base image,
  and the family a large share of devcontainers start from — carries no python at
  all: not `/usr/bin/python3`, not a versioned binary without the symlink, nothing
  on any entry of a login shell's PATH. So the stage's `command -v python3` failed
  and it exited before writing anything, on a container that had every other piece
  in place. The hook needs to talk to a unix socket and merge a JSON settings file,
  and 0.8.0's own history is the argument against doing that in shell: the version
  that shelled out installed an *empty* hook in a container with no `cat` and
  reported success. Neither half of that is fixable by writing the payload
  differently; a hook that runs inside the developer's own agent needs an
  interpreter that is really there.

  The other half of why it goes is that the failure could not be read. The stage
  prints its reason to stderr precisely so a developer can act on it — but only the
  exit status becomes a `ProvisionEvent`, so a launch said
  `<ws>: the herdr setup stage exited 1.` and nothing else, on every launch, with
  the sentence naming the cause discarded. A feature whose majority case is a
  refusal has to be able to say which refusal.

  **What a host running herdr can still do, with nothing from `dl`:**
  `herdr --remote <workspace-id>.devpod` runs a herdr server *inside* the
  workspace, where the agent really is the pane's foreground process, so the badges
  are correct with no mount and no hook. The price is one view per workspace rather
  than one spanning them all, and a herdr in the container whose version matches
  the host's.

  **Two leftovers in a workspace created by 0.8.0-0.11.0, and neither is removed by
  upgrading.** Both land only at creation, so both go on `dl <ws> recreate` and on
  nothing else. Run that on any workspace you created while 0.8.0-0.11.0 was
  installed.

  The **hook** — `~/.devlaunch/herdr-agent-state.py` plus its entries in that
  container's own `~/.claude/settings.json` — is inert *as `dl` now runs*: it returns
  immediately without the variables `aid` no longer sets, and it exits 0 in every
  state including every failure. It is not inert unconditionally, and the
  replacement recommended above is what wakes it. `HERDR_ENV`, `HERDR_PANE_ID` and
  `HERDR_SOCKET_PATH` are herdr's own variables, not `dl`'s, so a
  `herdr --remote <workspace-id>.devpod` session sets all three itself — and the
  leftover hook reads them and resumes reporting agent state, from a `dl` that no
  longer has anything to say about it. Because a reported state overrides herdr's
  native detection and does not decay, a stale report can pin a badge that the
  remote server would otherwise have got right.

  The **socket mount** is not inert at all: it is a capability the container still
  holds. 0.8.0's README said what it grants, and that sentence went out with the
  feature —

  > Mounting the socket gives the container control of your herdr session — it is
  > the same socket herdr's own CLI drives, so something in there could open panes
  > or read other panes' output.

  — which is unchanged for a container that already has the mount. `DEVLAUNCH_NO_HERDR`
  was the opt-out and is no longer read, so it cannot be turned off after the fact
  either; `dl <ws> recreate` is the only thing that removes it. `DEVLAUNCH_NO_TOOLS`
  is untouched and still covers the rest of provisioning.

## [0.11.0] - 2026-08-23

### Changed

- **The container hostname is now the workspace id without its identity suffix**, so
  a prompt reads `vscode@devlaunch-main:~/repo$` where it used to read
  `vscode@devlaunch-main-zovomobo:~/repo$`. The id is unchanged and still addresses
  everything it did — the devpod workspace, the clone directory, the verdict cache —
  and only the name in the container's UTS namespace is shorter.

  The suffix is eight characters of hash. It is in the id to make the id injective,
  one workspace per `(owner, repo, ref)`, and nothing addresses a container by its
  hostname: dl reads the id back from devpod, never from the container. So the
  hostname carried nine characters that no reader of a prompt has any use for, in the
  one place they were read most often.

  What it costs is one prompt for two workspaces where the suffix was the only thing
  telling them apart: one repository under two owners, and `feature/auth` beside
  `feature-auth`, which slug alike. They remain two workspaces with two ids, two
  containers and two clones, and the tab — which carries the spec whole,
  `owner/repo@ref` — is what tells them apart. A prompt long enough to be unique was
  not thereby legible.

  **A workspace that is already running keeps its old hostname.** The stage rides the
  pass that follows a `devpod up`, so the name is decided when a container starts and
  nothing re-sets it on attach — `dl <ws> restart` or `dl <ws> recreate` is what
  re-decides it, and the container this build was compiled in was named by the build
  that opened it.

  Two things follow that are worth knowing. A name dl did not derive is *mostly* left
  alone: `dl myworkspace` still gets `myworkspace`, because the suffix is parsed — its
  fixed width and its consonant-vowel alphabet both — rather than counted off the end.
  Mostly, because four consonant-vowel pairs is a shape English words have too, so a
  hand-named workspace ending in one — `foo-motorola` — does lose that word from its
  prompt. And the 64-byte hostname reserve that held the id at 38 characters until
  0.3.0 is no longer the binding one: what a downstream tool builds a name onto now
  tops out at 38 characters rather than 47, leaving ~26 of the 64. devpod's
  48-character ceiling on the id is what keeps the cap at 47.

### Fixed

- **The launch-latency trend has been publishing nothing since 0.3.0, and the bench
  workflow now puts devpod on the PATH it runs `dl` from.** `dl` shells out to a bare
  `devpod`, devpod is a pixi dependency rather than an install step, and the priming
  launch is the one launch in that job that runs outside `pixi run` — so it had no
  devpod at all. `devpod not found on PATH`, exit 127, eighty seconds, nothing timed:
  23 consecutive merges to main, every one from the commit that retired the Python
  tree through 0.10.0.

  The same edit is what made the trend measure the right build in the first place —
  before it, these steps ran `pixi run dl`, which resolved to the editable Python
  install — so the fix is a devpod symlink into the directory that already puts the
  release `dl` on PATH, not a return to `pixi run dl`. A symlink rather than the pixi
  environment's whole `bin`, because that directory carries a python and a git too,
  and what the benched launch resolves is part of what is being measured.


## [0.10.0] - 2026-08-22

### Changed

- **A workspace is now *named* by the spec you typed, where it used to be named by
  its id.** Two places a person reads a workspace changed, and the id itself did
  not: it still addresses every workspace, names every clone directory and is still
  the container's hostname.

  An id is `<repo-slug>-<ref-slug>-<suffix>`, and two of those three parts are what
  somebody looking at a workspace wants while the third is machinery. The suffix is
  eight characters of hash, there so two branches cannot share an id, and reading it
  is no part of choosing a workspace or of knowing which one a tab is. What the id
  *lacks* is the owner — it carries none at all, so a fork and its upstream were a
  row and a tab spelled the same — and it spells the branch as a slug, so
  `feature/auth` reads as `feature-auth`, which is also the name of a different
  branch the same repository could have.

- **The selector draws `owner | repo | branch`.**

  ```
  blooop          | devlaunch  | main
  kinisi-robotics | kinisi_ros | ags-devcontainer-tooling-su
  -               | myproject
  ```

  Both halves come off the source devpod already reported, so the picker still opens
  no records and reads no config. Three columns rather than `owner/repo@branch`,
  which reads like a spec `dl` would accept and is not one: a ref-slug is lossy, so
  retyping one can address the other branch.

  The elision is never unconditional. A picked row is mapped back to its workspace by
  the row's own text, so two rows drawn alike would act on one workspace — and `dl
  rm` is one of the verbs the selector opens for. Where a split would collide, the
  rows go back to their whole ids, which puts the suffix on screen in exactly the
  case it is doing work.

- **A tab is named `blooop/devlaunch@main`, and keeps that name for the session.**
  The escape dl writes just before the handover used to last about a second: Ubuntu's
  stock `~/.bashrc` puts `\e]0;\u@\h: \w\a` at the *front* of `PS1`, so every prompt
  renamed the pane after the hostname, which is the id. The setup pass now appends
  one line to the profile a login shell reads, so dl's name is the last write of
  every prompt:

  ```
  case $- in *i*) [ -n "$BASH_VERSION" ] && PS1="$PS1\[\e]2;"blooop/devlaunch@main"\a\]" ;; esac
  ```

  Appended, because two escapes in one prompt are applied in order and the last one
  wins — a `PROMPT_COMMAND` would lose, since bash runs that before it prints `PS1`.
  Nothing is rewritten, so the visible `user@host:path$` still says the hostname and
  only the tab changes. It rides the hostname stage's existing round trip and costs
  no extra one.

  Two bargains worth knowing. The line is installed when a workspace enters Running
  rather than on every attach — the same trade the hostname stage makes — so
  `DEVLAUNCH_NO_TITLE=1 dl <ws>` on a running workspace silences dl's own escape but
  not the prompt's, and `dl <ws> recreate` is what re-decides it. And only a spec is
  installed: a container told to title after its own id is told nothing, since that
  is already its hostname.

  `DEVLAUNCH_NO_TITLE` governs both halves.

### Fixed

- **A workspace named after another row's columns drew the same selector row.** A
  workspace dl did not clone, named `devlaunch | main`, drew
  `blooop | devlaunch | main` beside a clone of `blooop/devlaunch@main` — and picking
  it acted on the clone. Distinctness is now established over the drawn labels rather
  than argued from the names devpod permits.

- **An id that two repo spellings explain read back as the wrong branch.** The repo
  slug is cut to twenty characters only when an id would overflow, so a reader has
  two spellings to try, and for a repo whose slug has a dash at exactly the cap both
  can explain one id. The branch column showed the shorter reading, which is a branch
  name the row could plausibly have had. Such an id is now refused and the row draws
  the id whole.

- **Opening a workspace by its id renamed its tab back to the hash.** The profile
  line is deduped by a hash of its own text, so a second name for one workspace
  appended rather than replaced, and the last append won every prompt.

- **A dash login shell printed the title escape instead of setting it.** `~/.profile`
  is read by any POSIX login shell, and `/bin/sh` is dash on Debian and Ubuntu, where
  `\[`, `\e` and `\a` mean nothing — so the prompt showed the escape at every line. A
  corrupted prompt is worse than an unnamed tab.

- **A ref's trailing newline reached the profile unfiltered**, splitting one `PS1`
  assignment across two lines of a file every login sources. Both halves of the title
  now take the same filtered name.

## [0.9.0] - 2026-08-22

### Changed

- **`--rm` is docker's `--rm`: the workspace goes when the session ends.**
  `dl <ws> --rm` and `dl <ws> --rm -- <cmd>` hand over a session and delete the
  workspace and its clone once it ends — which is what `--autorm` did, under the
  name docker gives it. The `rm` verb is unchanged and is the only way to delete a
  workspace *now*, so the whole grammar is `docker rm` beside `docker run --rm`,
  and neither spelling has to be read twice to work out which was meant:

  ```bash
  dl kinisi/repo@fix/x rm                            # delete it now
  dl kinisi/repo@fix/x --rm                          # shell; it goes when you exit
  dl kinisi/repo@fix/x --rm -- make test             # one command, then it goes
  aid kinisi/repo@fix/x 'fix the flaky test' --rm    # the agent runs, then it goes
  ```

  Everything the removal already promised is unchanged: it stops at work that is
  nowhere else and leaves the workspace standing, it collects a build that died in
  `postCreateCommand`, and the exit code is the session's and never the removal's.
  `--force` still does not compose with it — that is `dl <ws> rm --force`, which is
  where docker keeps its `-f` too.

### Removed

- **`--autorm` is now spelled `--rm`.** A rename and nothing else; the behaviour
  above is what it always did. The old spelling is recognised and refused with the
  new one rather than dropped, so a line recalled from history says what happened
  instead of quietly doing nothing:

  ```
  $ dl <ws> --autorm
  --autorm is now spelled --rm: 'dl <workspace> --rm' opens the workspace and deletes
  it when the session ends, the way 'docker run --rm' does. Use 'dl <workspace> rm' to
  delete one now.
  ```

- **`--stop` is retired, and so is appending `--rm` to cancel a line.** Both were
  the *suffix* form of a verb: typed at the end of a line that already asked for
  something and winning over it, so `aid <ws> 'review this pr' --rm` deleted the
  workspace and printed `--rm overrode the rest of the line`. That shape cannot
  survive a `--rm` that means "delete when the session ends" — the two spellings
  look like a pair, and one cancelling the line while the other runs it is exactly
  the pair nobody can keep straight. So `aid <ws> 'review this pr' --rm` now runs
  the review and deletes afterwards, and `--stop` refuses with the word to use:

  ```
  $ dl <ws> --stop
  --stop is no longer a flag: the flag spellings now modify a session (--rm deletes
  the workspace once one ends) rather than name a verb. Use 'dl <workspace> stop' to
  stop a workspace.
  ```

  For "I am done with this workspace", `dl <ws> rm` names it and `dl rm` picks it —
  and the pick marks several with TAB, which for a long `aid` prompt line is fewer
  keystrokes than recalling it to type at the end. What is genuinely gone is
  deleting a workspace without naming or picking it, by appending to whatever the
  last line happened to be.

  One consequence worth knowing: `dl prune <ws> --rm` used to remove `<ws>`, and
  is now the `prune` retirement's refusal, since nothing overrides a line any more.
  Add `--force` to that line and the `--force`-beside-`--rm` refusal is what you
  get, because the pair is the more confused half and is named first.
  `dl --prune` is unchanged.

- **`dl <ws> rm --rm` is refused as the two requests it is**, rather than being
  quietly treated as one of them. The sentence names the verb, which is the
  spelling that already does what such a line most likely meant.

## [0.8.0] - 2026-08-22

### Added

- **A host-side [herdr](https://herdr.dev) session now shows what the agent in a
  workspace is doing.** herdr shows, per pane, whether the coding agent in it is
  working, idle or blocked waiting for a human, and under `aid` it showed none of
  that. The cause is structural: herdr identifies a pane's agent from the pane's
  foreground process, and the host's tree is `aid → dl → ssh` with the agent inside
  the container, where no process table on the host can see it.

  It was measured rather than assumed. Two panes in one container, same claude —
  one running it directly, one behind `script -qc claude /dev/null`, which is the
  same pty-proxy shape as the ssh hop — produce equivalent detection snapshots, and
  herdr registers an agent for the first and `agent_not_found` for the second.
  Screen content is not the missing signal; process identity is. herdr takes an
  answer for that over its socket, so devlaunch supplies it.

  Three things cross the container boundary and nothing else. The socket, bind
  mounted at `/var/tmp/devlaunch-herdr.sock`. The pane's identity, on the agent's
  own command line rather than in workspace environment — a pane id is a fact about
  this session, and attaching to a running workspace skips the `up` that would
  refresh workspace environment, so the container would otherwise report into
  whichever pane was current when it was last built. And a hook wired to claude's
  `SessionStart`, `UserPromptSubmit`, `Notification`, `Stop` and `SessionEnd`,
  because a held report nothing updates is a badge that lies. `Notification` is the
  event that earns the feature: it is what claude fires when it is waiting for a
  human.

  Not herdr's own `integration install claude`: that hook sends only
  `pane.report_agent_session`, which registers nothing by itself — calling it
  against an unregistered pane still answers `agent_not_found` — so forwarding it
  would carry session metadata for an agent herdr does not believe exists.

  It pairs with the terminal title above rather than competing with it. herdr can
  read state from a title too, but `aid` suppresses claude's own title so the
  workspace id is what stands, which is exactly why the state comes from the hook:
  the badge says what the agent is doing and the pane name says where.

  On a host not running herdr nothing happens at all — no mount, no stage, no
  notice, every command line byte-identical. That is decided from whether a socket
  exists rather than from whether the feature is on, so it is true of the payload
  and not merely of its effects. `DEVLAUNCH_NO_HERDR=1` opts out, and
  `DEVLAUNCH_NO_TOOLS=1` covers it too. Mounting the socket gives the container
  control of your herdr session, which is the trade and the reason for the opt-out;
  it is on by default for the reason the forwarded GitHub token is.

  The stage can never fail a launch, and three things have to be true for it to do
  anything: a `python3` in the container, a claude configuration directory that is
  not shared with the host, and the socket mount — which lands only at container
  creation, so a workspace that predates it needs `dl <ws> recreate` rather than a
  restart. The middle one refuses rather than merging, and a mounted *parent*
  counts: `dl` never mounts `~/.claude` into a workspace, but this repo's own
  devcontainer does, so devlaunch's own workspaces report the stage and install
  nothing while the arbitrary repos `dl` exists to launch get the badge.

## [0.7.3] - 2026-08-22

### Fixed

- **The selector's invitation is drawn inside the picker, so TAB is discoverable
  at last.** The line that says what the rows are — and, for `dl rm`, `dl stop`,
  `dl up`, `dl code` and `dl dotfiles`, the only thing that says TAB marks several
  — was printed to stdout immediately before the picker opened. skim's first act is
  to switch to the alternate screen, which replaces the visible screen wholesale, so
  that sentence was gone for the entire time the picker was up and came back only
  once it had exited. Multi-select had shipped in every release since 0.6.0 with its
  one piece of documentation on a screen nobody could see while choosing.

  It is now skim's sticky header, which the picker draws itself, directly above the
  matches and below the search bar. Wording is unchanged. stdout keeps the line only
  for the run that has no picker to put it on — no terminal, so no header either,
  and stdout is the only surface left; on a terminal the sentence is shown once, in
  the one place it can be read.

  Proved on a pty, not in the options: the test runs `dl` on a real terminal and
  reads the screen back, which is the only seam that can tell an option that is
  spelled right from one that draws something. It pins both halves — the sentence
  is on the picker's screen, and it is not on the screen the picker covers — so the
  redundant print cannot come back unnoticed.

## [0.7.2] - 2026-08-22

### Fixed

- **Changing your Claude account on the host reaches the containers again.** The
  `claude-code` feature mounted `~/.claude` a path at a time, and four of those
  paths were *files*. A bind mount of a file is attached to the dentry, so when
  the host replaces it by rename — which is what Claude does on every token
  refresh, and on an account switch — the mount is left pointing at an inode with
  no name. The container reads it happily and forever, which is why nothing
  reported it as broken: a workspace created before the switch went on
  authenticating as the account you had left, and each one froze at a different
  moment, so three running containers held three different credentials files and
  none of them held the host's.

  `~/.claude` is now mounted as the directory, read-write, and a directory mount
  resolves names on each access — so it follows the rename and the container
  reads what the host has. `.credentials.json` and `.claude.json` have no mount
  of their own any more; they are reached through it.

- **The read-only mounts over `CLAUDE.md` and `settings.json` are gone, because
  they were not read-only.** The same rename removes a nested file mount from the
  namespace entirely, and the path then falls through to whatever the parent
  provides. Measured on Docker, both ways round, since the direction of the
  failure follows the parent and neither direction is safe: under a read-write
  parent the file ends up **writable**, so a protection the manifest still
  advertises is silently gone from the first host edit onwards, and under a
  read-only parent a read-write file mount ends up **read-only**, so a token
  refresh fails.

  A mount of a *directory* survives the same rename with its flags intact, so the
  read-only list is now exactly the five instruction directories — `agents/`,
  `commands/`, `hooks/`, `skills/`, `wf-skills/` — and every source in the
  manifest is a directory. A test asserts that rather than asserting the paths,
  because the tempting change is to protect the two files by naming them again,
  which passes review, appears in `docker inspect`, and stops being true on the
  next edit.

  The cost is stated where it is met rather than buried: `CLAUDE.md` and
  `settings.json` are writable from the container, and `settings.json` can name
  hook commands inline, so this is a real hole. It is the same hole the previous
  layout had after one edit, minus the claim that it was closed.

- **The pre-create hook no longer seeds empty files.** With nothing mounted a
  file at a time, a missing `.credentials.json` can no longer refuse the create,
  and Claude writes each of these itself on first use. An empty `{}` credentials
  file on a host that has never run Claude is indistinguishable from a logged-out
  session, and existed only to satisfy a bind source that is gone. The stale-mount
  heal stays, for containers built by the older layout that are still running.
## [0.7.1] - 2026-08-22

### Changed

- **The fuzzy selector's columns are the owner and the workspace id.** The picker
  drew `id | local | /a/long/path`, and both of the columns that have gone were
  answering a question nobody standing at the picker is asking: the middle one
  reads `local` for every workspace `dl` makes, since `dl` always hands devpod a
  path, and the last is the clone directory `dl` chose and manages — whose own
  last component is already the id beside it.

  What an id cannot say is whose repository it is. An id is
  `<repo-slug>-<ref-slug>-<suffix>`, so it carries the repo and no owner, and a
  fork and its upstream are two rows spelled the same. The owner column is
  derived from the source devpod already reported — a git URL's owner, or `dl`'s
  own clone layout read backwards — and never from `metadata.json`, which records
  the owner outright but is a file a warm launch must be able to prove it never
  opened.

  This is a search change as much as a display one: the row text is what the
  fuzzy matcher matches, so typing an owner now narrows the list, which no
  arrangement of the id alone could do.

  A workspace whose owner `dl` cannot establish — one opened from a path, from a
  URL that is not GitHub's, or a clone under a `repos_dir` a `config.toml` moved
  outside the cache — shows `-`, padded like any other owner so the ids stay in
  one column.

## [0.7.0] - 2026-08-22

### Added

- **Every launch names the terminal after the workspace, so a multiplexer's tab bar
  says which workspace a pane is.** `dl` writes the workspace id as an OSC 2 title
  just before the session takes the terminal — one escape sequence to stderr, and
  therefore no detection: zellij and tmux both read it as the focused pane's title,
  and a bare kitty or xterm reads it as the window title. zellij publishes
  `<session> | <pane title>` outward, so the id is what reaches the outer tab bar.
  `DEVLAUNCH_NO_TITLE=1` turns it off; a "no" variable rather than an opt-in one
  because, unlike `DEVLAUNCH_ZELLIJ`, what it does is write bytes the next shell
  prompt overwrites anyway.

  The workspace id rather than the spec, because it exists for every launch where
  the spec does not: a bare `owner/repo` still has its branch unresolved at that
  point, and `./some/dir` is not a spec at all. It is also already the container's
  hostname, so dl's title and the `user@host` an interactive prompt paints over it
  are the same string rather than two. It stays tab-bar short because devpod
  refuses to create or report a workspace whose name runs past 48 characters, not
  because dl truncates anything.

  Written to stderr because stdout is parsed by the completion machinery and by
  `wf`, and skipped unless stderr is a terminal — which is why `dl <ws> -- make test
  > log`, whose stdout is a file and whose terminal is still there, is still named.

- **`aid` starts claude with `CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1`, which is what
  makes the workspace name stick.** A terminal title has one value and the last
  writer sets it; claude writes one continuously from its own read of what the
  session is doing, so the two are not two signals but one contest that claude wins
  within a second. Turning claude's off is the trade worth taking: what claude is
  doing is already on screen in the pane, and which workspace the pane *is* is not
  otherwise anywhere. Scoped to `aid`, which is what decided to start claude — a
  `dl <ws> -- claude ...` somebody typed themselves is their command and not aid's
  to rewrite.

  Nothing new had to be plumbed for it. aid's agent table is already an env prefix on
  a payload that runs under `bash -lc`, so this is one more entry beside the
  `IS_SANDBOX=1` that was already there, and no host variable is forwarded.

## [0.6.1] - 2026-08-22

### Changed

- **The fuzzy selector's search bar is at the top of the picker, not the bottom.**
  The prompt is now the first line and the matches read downward from it, so the
  best match is the row next to what you are typing rather than the one furthest
  from it, and narrowing a query no longer walks the whole list up past the cursor.
  skim's default layout is the other way round — query at the bottom, list growing
  upward — and `dl` had been taking that default rather than choosing it. Nothing
  else about the picker moves: devpod's order is still the order, and TAB still
  marks any number of rows for the verbs that take several.

  The layout is now pinned by tests that open a terminal, run `dl` on it and read
  the screen back, rather than by ones that read the options `dl` asked for. That
  distinction is the whole of why it is worth saying: skim carries a `reverse` flag
  next to the layout, documented as shorthand for exactly this, which is expanded by
  a `build()` that the entry point `dl` uses never calls — so setting it compiles,
  reviews as the fix, and draws the old picture. Measured, not reasoned about.

## [0.6.0] - 2026-08-22

### Added

- **The fuzzy selector takes several workspaces for the verbs that can use them.**
  `dl rm`, `dl stop`, `dl up`, `dl code` and `dl dotfiles` (and the `--rm`/`--stop`
  spellings) with no workspace named now open the picker in multi-select: TAB marks
  any number of rows, Enter applies the verb to each in turn, so five dead
  workspaces are cleared in one visit instead of five. Every marked workspace is
  attempted whatever happened to the ones before it — one `rm` refused over unsaved
  work does not drop the rest of the batch — and the exit code is the first
  failure's, so scripts still learn something went wrong. The forms that end in an
  interactive session (`dl`, `dl -- <command>`, `restart`, `recreate`, `reset`)
  still take exactly one, since several of those would just be sessions queued
  behind each other's exit.

## [0.5.0] - 2026-08-21

### Fixed

- **Workspace removal no longer leaks the devcontainer's Docker volumes.** Deleting
  a workspace now removes the two named volumes that workspace's devcontainer
  created — `<workspace-folder-basename>-pixi` from a devcontainer `mounts` entry,
  and `dind-var-lib-docker-<devcontainerId>` from the `docker-in-docker` feature —
  on every path that removes one: `dl <ws> rm`, the appended `--rm`, `--autorm` and
  `--purge`. Nothing in devlaunch had ever run a volume command, `devpod delete`
  removes the container and never a volume, and Docker never garbage-collects a
  *named* volume, so both outlived every workspace that made them. Measured on one
  development machine: **39 orphaned volumes holding 37.28 GB**, not one with a
  surviving workspace in `devpod list` (#324, #325).

  **The names are read from devpod's own record and never guessed from a pattern.**
  devpod writes down what it substituted into the devcontainer, and both names are
  built from that one read — so a workspace devpod never finished creating names
  nothing and runs no `docker` at all, rather than one carrying a made-up name that
  would be somebody else's disk. It is also why the names are read *before* the
  delete: `devpod delete` takes that record away with the workspace, so a removal
  that read afterwards would find nothing every time and look like it worked.

  **Best-effort by construction, and it cannot fail a delete.** The workspace is
  gone either way, and reporting failure would send the caller looking for a
  workspace that is not there — the same bargain the clone-removal arm beside it
  makes. A volume Docker will not release is a line on stderr; a machine with no
  `docker` says nothing at all, because a machine with no Docker never made these
  volumes.

### Changed

- **The sentence both cleanups end on now disclaims images rather than "images or
  volumes".** With the volumes of a deleted workspace actually removed, the old
  wording described a leak that had been fixed. Images stay outside deliberately
  rather than for want of a fix — they are shared between workspaces, expensive to
  rebuild, and which workspace owns one is genuinely ambiguous — which is why the
  sentence still exists (#325).
- **`devlaunch_core::api::workspace_delete` takes one more argument**, and so does
  `flows::lifecycle::purge_all_data`: an `Option<&Path>` for devpod's home
  directory, which is where the volume names are read from. Resolved by the caller
  rather than in core, for the reason every other environment answer is — the
  process that knows what its environment says hands the answer down, and a second
  answer inside core could disagree with the first. This is a breaking change to
  the frozen `api::` surface: an out-of-tree caller passes
  `devlaunch_core::flows::lifecycle::devpod_home().as_deref()` there, or `None` to
  keep the old behaviour of removing no volumes (#325).

## [0.4.1] - 2026-08-21

### Fixed

- **A dotfiles refresh can now recover a workspace whose chezmoi config predates
  a new template variable.** `chezmoi update` applies with the config rendered by
  `chezmoi init` at create, and nothing regenerated it — so a dotfiles repo that
  adds a variable left every existing workspace pulling fine and then aborting on
  `map has no entry for key`, on `dl <ws> dotfiles` and the attach refresh alike.
  The refresh now re-renders the config and applies again when the first attempt
  fails, after the pull rather than before it, since the template that names the
  new variable arrives in that pull. A workspace whose update succeeds never takes
  the branch.

## [0.4.0] - 2026-08-21

### Added

- **`aid` asks for the prompt while the workspace boots.** A promptless
  `aid <workspace>` on a terminal now starts the workspace booting in the
  background and reads the agent's prompt from the terminal while it does — typed
  free of shell quoting, submitted with Enter, and handed to the agent through
  the same rewrite an argv prompt takes. An empty Enter (or Ctrl-D) is the plain
  session a bare `aid` always started, and a piped stdin or `DEVLAUNCH_NO_TTY=1`
  skips the question entirely, so scripts are untouched. The boot is a background
  `aid --boot-up` child running dl's own `up` verb — the prewarm shape the
  per-workspace launch lock already serializes — with its output parked in a log
  and replayed after the Enter, so the build's progress is still seen, just not
  interleaved with the typing. Ctrl-C at the editor tears the whole boot down
  through the shared interrupt disposition: the boot child kills its `devpod up`
  and unlinks the staged token, exactly as an interrupted foreground launch does.
  One visible seam: with `DEVLAUNCH_TIMING` set, the boot child's summary arrives
  inside the replayed log beside the foreground run's own.

- **`DEVLAUNCH_NO_ZELLIJ=1`: no zellij in the container, without giving up the
  tools.** A second opt-out beside `DEVLAUNCH_NO_TOOLS`, dropping the zellij stage
  from the setup pass and nothing else — the pass still runs, the container is still
  named, and `gh` and `claude` are still probed for, lent and installed.

  Two variables because they answer two questions. A host whose containers get
  zellij another way — their own dotfiles, a base image, a devcontainer feature — or
  which wants none in there at all, is asking for one stage to stop running;
  `DEVLAUNCH_NO_TOOLS=1` is the switch that would also surrender the `gh` and
  `claude` guarantee `dl` forwards a GitHub token for, which is a large price for
  one `command -v`. The two are an **and**: the stage runs only when neither
  variable asked for it to go, and all three ways of being without it compose the
  same script byte for byte.

  Deliberately unrelated to `DEVLAUNCH_ZELLIJ`, which decides whether a
  `dl <ws> -- <cmd>` first makes sure a zellij *session* exists to open panes into.
  That one already tolerates a container with no zellij — its setup is allowed to
  fail and the command runs regardless — so the two switches compose without either
  one having to know about the other.

### Changed

- **A `dl <ws> up` against a container that is already up no longer pays the tools
  round trip.** The setup pass is one `devpod ssh`, ~1.7s of which almost all is
  connection and process setup rather than the script it carries, and a
  long-provisioned workspace was paying it on every launch to be told the same thing
  as last time. The verdict is now written down — one small JSON file per workspace
  under `${XDG_CACHE_HOME:-~/.cache}/devlaunch/tool-verdicts/` — and reused.

  **What makes a written-down verdict stop being true is a container that is not the
  one it was about**, and the host can read that without asking anything: devpod
  rewrites `workspace_result.json` on the way out of every completed `up`, whoever
  ran it — `dl`, VS Code, a hand-typed `devpod up`, a `--recreate`. The marker
  records that file's mtime and is believed only while the mtime is still *equal* to
  it. Equality rather than "no newer", because the question is which container this
  is and not how old the answer is. Every other doubt — no marker, one that will not
  parse, a workspace with no single result file to key on — also reads as no verdict,
  and every one of those falls back to making the trip, which is exactly the
  behaviour that shipped before.

  **Only a launch that found the container already running skips it**, and the
  hostname is why. `sudo hostname <ws>` is a stage of that same pass, and the name
  lives in the container's UTS namespace, which docker rebuilds from the container's
  config on every start — so a `devpod up` that created a container or started a
  stopped one has lost the name, and the pass has to run again to set it before the
  session reads it into a prompt. The two paths that skip are the `dl <ws> up`
  top-up (the pre-warm, where the trip is paid most often) and a launch that waited
  on a sibling which had already brought the workspace up. The pass after this
  launch's own `devpod up` always travels.

  Only a pass that probed *provisioned* is recorded. A lend, an install and a kept
  shim are not: `ShimKept` in particular is a documented residual in which a
  container re-attempts one failing transfer on every `up`, and a marker would
  quietly turn that into never attempting again. Each of those is followed by a
  later pass that probes provisioned, and that is the pass that records.

### Fixed

- **`aid`'s completion refresh no longer dies on arrival.** dl re-spawns its
  detached completion refresh through `current_exe --update-cache`, which under
  `aid` is `aid --update-cache` — a line aid used to refuse as "aid needs a
  workspace", so every refresh an aid launch fired silently failed and
  completions never refreshed. aid now forwards `--update-cache` verbatim to dl.

## [0.3.3] - 2026-08-21

### Added

- **`--autorm`: the workspace goes when the session does.** `dl <ws> --autorm` and
  `dl <ws> --autorm -- <cmd>` delete the workspace and its clone once the session
  they handed over has ended, the way `docker run --rm` does; `aid` takes it too,
  appendable like `--rm` but keeping the prompt, so the agent runs and *then* the
  workspace goes.

  The removal is `dl <ws> rm`'s rather than a second one, which decides three things
  at once. It goes through the **unsaved-work guard**, so a clone holding
  uncommitted or unpushed work — or one git could not read to find out — refuses,
  names which, and leaves the workspace standing: the flag never decides that your
  work was disposable, which is what makes it safe to leave on a line you recall.
  It resolves the target through the same `target::resolve` the `rm` verb uses, at
  the cost of one `devpod status` on the exit path, so `--autorm` and `rm` cannot
  disagree about which workspace they are addressing. And the **launch's** exit
  code is what reaches the shell — `dl repo --autorm -- make test` exits with the
  test's status — because a cleanup that refused is not the test failing.

  **What counts as "there is something to remove" is carried, not inferred from the
  exit code.** `dl` returns exit 1 both for a workspace that never existed and for a
  container that came up and would not open a session, and those are opposite answers
  to that question — so `render_launch` now reports a `Reached` beside the `Ending`.
  The case this gets right is a `devpod up` that dies in `postCreateCommand`: it
  leaves the container **running**, devpod's record written and the clone cut (which
  is why `lifecycle::create_record` exists at all), so an unattended
  `dl owner/repo --autorm -- make test` against a broken devcontainer would otherwise
  leak, every run, exactly the workspace the flag was reached for. A session devpod
  refused, and an OpenSSH that is not installed, leave the same thing behind and are
  collected for the same reason. A launch that stopped before devpod was asked for
  anything removes nothing.

  **The completion cache is rewritten after the removal, not only before it.**
  `Refresh` is one detached child per command, and the launch spends it the moment
  the session returns — describing a world with the workspace still in it. The new
  `Refresh::rearm` lets the one command that legitimately changes the workspace list
  *twice* spawn a second child, so tab-completion stops offering a workspace that has
  just been deleted instead of going on offering it until the cache TTL expires.

  `--force` does **not** compose with it, and is refused rather than ignored: a
  `--force` habitually appended to a recalled `--autorm` line would destroy work
  hours later, unattended, with nobody reading the sentence that explained it. Run
  `dl <ws> rm --force` when that is what you mean.

  Every verb word refuses the flag rather than dropping it, and `code` is why that
  has to be a refusal — it returns while VS Code is still connecting, so honouring
  `--autorm` there would delete the container out from under a window that is still
  opening. `restart`, `recreate` and `reset` are refused for a different reason
  worth not conflating with it: those three *do* end in a session, so the removal
  would work behind them, and they are out because `--autorm` is the throwaway
  workspace rather than a cleanup modifier on every verb that ends in a shell. The
  refusal names the two forms that work instead of claiming those verbs hand over
  nothing. The grammar makes the flag unwritable rather than merely refused:
  `Autorm` is carried by the two `Verb` arms it is defined for, so no other arm has
  a field to put it in, and widening it later is adding arms there.

  Best-effort by construction, and documented as such: `dl`'s Ctrl-C disposition is
  a signal handler that `_exit`s, so a Ctrl-C during the container build — or a
  closed terminal — ends `dl` before the session does and leaves the workspace
  behind. A Ctrl-C *inside* a running session is unaffected, since the terminal is
  in raw mode and the interrupt goes to the remote process.

## [0.3.2] - 2026-08-20

### Fixed

- **A workspace whose create never finished is no longer attached to**
  ([#291](https://github.com/blooop/devlaunch/pull/291)). A `devpod up` that dies in
  its lifecycle hooks leaves the container **running**, so `devpod status` answered
  `Running`, the fast-attach arm fired, and `dl` attached to a workspace devpod never
  finished setting up — on that launch and every later one, because a running
  container was the whole test. What the user saw was not a setup error: devpod
  records a create's result only on its way out of a *successful* `up`, and the
  remote user lives in that result (`.MergedConfig.remoteUser`), so `devpod ssh`
  without one falls back to **root**. Everything the image put on the remote user's
  PATH is then missing and the session dies naming whichever binary it reached for
  first, which has nothing to do with the cause.

  Measured against devpod 0.26.1 in an isolated `DEVPOD_HOME`, with a devcontainer
  whose `postCreateCommand` exits 1: `devpod status` answers `Running`, no
  `workspace_result.json` is written, no `Host <id>.devpod` alias is published, and
  `devpod ssh --command whoami` answers `root` with `HOME=/root`. The result file
  survives `devpod stop`, a later `up`, and a `docker restart`, so reading its
  absence does not misfire on a workspace that was merely restarted.

  `dl` now reads devpod's own records and brings such a workspace up instead of
  attaching to it, which re-runs the hooks that failed and surfaces their failure —
  the diagnosis that was unavailable before. Three states, not two: a host whose
  devpod records cannot be read, or that holds one id under two contexts, is
  `Unknown` and attaches exactly as it did before, so the check only ever acts on
  positive evidence that a create did not finish. All three places that read
  "running" are covered — the fast attach, `dl <ws> up` (the verb documented as the
  recovery, which previously answered `already running` and declined to perform it),
  and the sibling-skip on the far side of the launch lock, where a launch that
  waited out a failing sibling would otherwise inherit its container.

  The one accepted cost: a workspace created by a devpod too old to write
  `workspace_result.json` is rebuilt **once**, and the rebuild writes the file.

### Changed

- **The workspace-id length budget is pinned where the two sweeps meet**
  ([#282](https://github.com/blooop/devlaunch/pull/282)). Tests only, no behaviour
  change. `every_repo_length_fits` and `every_ref_length_fits` each move one length
  at a time, but the cap is a function of the *pair* — the repo slug decides how much
  room `fit_ref` gets — so reintroducing the pre-[#64](https://github.com/blooop/devlaunch/issues/64)
  hole (guarding at a literal instead of at the budget) left a 30-character repo with
  a one-character ref deriving an over-long id while the other 63 tests in the module
  still passed. The cross product now covers it, and asserts the budget is *reached*
  as well as respected, because a budget nothing reaches would pass a bound test
  while truncating harder than the format intends. Two boundary tests join it: the
  repo-slug floor against the total budget, and the ref shapes the segment pass
  cannot shorten.

## [0.3.1] - 2026-08-20

### Fixed

- **This repo's devcontainer installs the committed lock instead of solving its
  own** ([#288](https://github.com/blooop/devlaunch/pull/288)). `postCreateCommand`
  ran a bare `pixi install`, and a bare install treats a lock it cannot read as a
  missing one: it warns, exits 0, solves a fresh environment, and rewrites the
  tracked `pixi.lock` on the way past. Measured against this repo's own manifest
  and lock with the pixi the container pinned before #281 (0.63.1, committed lock
  version 7), that install exits 0 having rewritten `pixi.lock` from version 7 down
  to 6, where `pixi install --frozen` exits 1 and names the version gap. The solve
  also reaches the network — resolving pypi dependencies alongside conda ones needs
  a conda-pypi name mapping fetched remotely — and a create whose fetch failed died
  in `postCreateCommand` with the workspace never opening, which is how this
  surfaced. #281 pinned the container's pixi so it *can* read the lock; `--frozen`
  is the other half, because a create ignoring the lock is not a version question.
  With the mapping cache deleted, `pixi install --frozen --offline` installs the
  default environment and never recreates it, where a solving install does. No
  change to what ships: `dl` and `aid` are untouched.

### Changed

- **The dev-loop names build the implementation that ships**
  ([#268](https://github.com/blooop/devlaunch/issues/268)). `./dev.sh` compiles
  `rust/` and installs the two binaries as `dl-next`/`aid-next`; inside this repo's
  devcontainer, `pixi run dl` and `pixi run aid` are `cargo run` over the working
  tree. Both used to be the Python build, so after the 0.1.0 cutover the `-next`
  names previewed something that was no longer what shipped. A `dev-build` cargo
  feature appends `-dev` to the version line, so `dl-next --version` prints
  `dl <version>-dev` where the released `dl` prints the bare version — divergence
  row 16 removed Python's `(dev, editable from <tree>)` suffix and left the two
  builds distinguishable only by the name they were typed under. Nothing that ships
  enables the feature. The trade `dev.sh` makes is now the opposite one: a compiled
  snapshot that moves when you re-run it, where the editable install had no build
  step and no snapshot.
- **The pixi Rust pin names one toolchain.** `rust = "1.97.*"` allowed a patch the
  gate never used, while AGENTS.md told a reader the environment was "pinned to the
  1.97.1 that rust-toolchain.toml names". Tightened to `1.97.1.*`, which is what the
  lockfile already held, and kept in lockstep by a test.

- **A fake devpod honours `--ignore-not-found`.** The test shim refused a delete of
  a workspace that was already gone, where real devpod v0.26.1 exits 0 — and
  `dl <ws> rm --force` passes that flag on every forced remove, so a run against an
  absent workspace failed under the shim and succeeded against the real thing. The
  shim is what the Rust integration tests use as their whole devpod, so a fidelity
  gap there is the one way a fake can do real harm.

### Removed

- **The Python implementation and the parity harness are retired**
  ([#267](https://github.com/blooop/devlaunch/issues/267)). `dl` and `aid` have been
  compiled binaries since 0.1.0; the `devlaunch/` Python package was kept afterwards
  as the frozen reference implementation a two-way parity ratchet compared the Rust
  build against. With the manifest empty since cutover, what that ratchet asserted
  was that two implementations agreed — and there is one now. Gone with it:
  `rust/parity.py`, `rust/parity-manifest.txt`, `rust/spec-ledger.md`,
  `rust/pending-count.txt`, `rust/golden_vectors.py`, `rust/tools/`, and the ~1,200
  mock-based tests that judged Python internals. The goldens under `rust/` stay —
  they pin today's contract, whoever wrote it — and so does the divergence table in
  `docs/rust-rewrite-plan.md`, which is still cited by row number throughout the
  Rust sources.

  No user-facing behaviour changes: nothing published contained the Python package
  after 0.1.0. What a contributor sees is that CI no longer runs a py310–py313
  matrix (the interpreter was the product's and is now only the harness's), the root
  `pyproject.toml` builds nothing, and `pixi run test` builds
  `rust/target/release/{dl,aid}` first because the acceptance suite judges the
  binaries from outside. That suite did **not** retire: `test/` still spawns the real
  binaries against a real devpod and against the fake one, through the
  `DEVLAUNCH_DL_CMD` seam, which now defaults to the release build.

  The lending contract's README guards moved into
  `rust/devlaunch-core/src/flows/provision/`, where the constants and script
  generators they assert against actually live.

## [0.3.0] - 2026-08-20

### Changed

- **Workspace ids may now be 47 characters instead of 38, so long branch names
  keep their tail.** The budget was never a limit anything enforced: devpod's own
  ceiling is 48 and fatal (`workspace name cannot be longer than 48 characters` —
  a 49-character id is refused, not truncated), and 38 was devlaunch reserving 10
  characters for downstream tooling that stacks prefixes onto the container name
  against a 64-byte limit. 47 spends nine of those characters on legibility and
  keeps one against devpod's wall, leaving ~17 for whatever bolts itself on.
  `kinisi_ros@ags-devcontainer-tooling-support` reads
  `kinisi-ros-ags-devcontainer-tooling-su-lenevere` rather than
  `kinisi-ros-ags-devcontainer-t-lenevere`, and a dependabot ref keeps the whole
  action name (`devlaunch-dependabot-codecov-action-6-sifivasa`). Truncation
  policy has no effect on collisions — the eight-character suffix is hashed over
  the full `(owner, repo, ref)` triple before anything is cut — so nothing about
  uniqueness moves with this. Existing containers are unaffected: `dl` addresses
  a workspace by the devpod id recorded in `metadata.json` (#88), so an id that
  was derived under the old budget keeps opening the same container. A clone
  directory whose leaf does change is re-cloned under the new name on the next
  launch, leaving the old directory for `dl --prune` to report.

## [0.2.2] - 2026-08-20

### Fixed

- **The tools devlaunch installs no longer edit the container's own pixi
  manifest.** `gh`, `claude` and `zellij` go into `~/.devlaunch/pixi` rather than
  `~/.pixi`, because `pixi global install` is not only an install: it is an edit
  to `$PIXI_HOME/manifests/pixi-global.toml`, a *declarative* file that in a
  container already has an owner. Writing there cost something in both
  directions. `pixi global sync` removes every environment the manifest does not
  list, so a dotfiles apply that rewrote the manifest and synced **uninstalled**
  the zellij devlaunch had just installed — invisibly, and the next launch
  reinstalled it, forever. And the manifest is not always a file: a devcontainer
  is free to symlink it onto a tracked file inside the checkout, and
  `kinisi_ros`'s does, so the append landed in the work tree and every
  `git status` in the workspace came up dirty. An install that dirties the tree
  it was pointed at is the launch damaging the work it exists to serve, and
  `dl` launches arbitrary repos. A home devlaunch created makes both
  unrepresentable rather than handled: nothing syncs that manifest, and no repo
  state can sit beneath that path.

  Not `~/.local/share/devlaunch/pixi`, which is the conventional path and the
  wrong one here — containers bind-mount `~/.cache`, `~/.config` and
  `~/.local/share` straight from the host, so a prefix tree under one would be
  shared by every container on the machine and written into your own home;
  prefixes are baked with absolute paths, which is prefix-dev/pixi#5476, the
  hazard the shared package cache already keeps `PIXI_HOME` away from.
  `PIXI_HOME` is set only for devlaunch's own install scripts and never exported
  into the login profile, so **your own `pixi global install` in a workspace
  still goes to your own `~/.pixi`**; only the bin directories go on `PATH`.

  Existing containers are untouched: their tools are already on the login `PATH`,
  so the presence check passes and nothing is reinstalled. The devcontainer
  feature's installer still writes `~/.pixi` deliberately — it runs at image
  build time, where that path is the image's own and there is no checkout to
  dirty.

## [0.2.1] - 2026-08-20

### Changed

- **`dl --help` puts the verbs and the examples above the options table.** clap's default
  layout renders every flag between the usage line and the `after_help` block, so the half of
  this CLI clap cannot describe from its own arguments — the workspace verbs, the examples, the
  suffix-flag and retired-word notes, the environment variables — sat below fourteen options
  nobody opened `--help` to read. The examples and the verb list now come straight after the
  usage line (`before_help`, plus a `help_template` that is clap's own default with
  `{before-help}` moved above `{all-args}`), `Environment:` stays last, and a unit test pins
  the order. Note that clap already does this for a CLI whose verbs are subcommands — it writes
  `Commands:` before `Options:` — and dl's verbs are positional words only because the grammar
  is workspace-first, so this is the layout clap would have produced anyway. Same flags, same
  verbs, same text, for `-h` and `--help` alike.
- **The README is ordered the same way, for the same reason.** The options table, the workspace
  id derivation and the purge/prune/reconcile and disk-accounting detail used to come before
  the things someone actually types, and `## Examples` sat 1200 lines down. What a reader needs
  on the way in — features, install, usage with the examples beside it, workspace and global
  commands, `aid` — is now the top half; a rule and a note mark where reference begins; and the
  header carries a two-row index. The `dl --ls` table was split out of `## Global Commands`
  from the 500 lines of cleanup detail beneath it, now `## Cleaning up: purge, prune,
  reconcile`. One duplicated example block (`## Workspace Sources`) was folded into
  `### Examples`. Every existing anchor still resolves.

## [0.2.0] - 2026-08-20

### Added

- **`--stop` and `--rm` can be appended to a line that already says something
  else.** Deleting the workspace you were just working in used to cost an edit in
  the middle of a long line: `aid owner/repo@fix/x 'review this pr'` recalled from
  history had to have its prompt removed before a verb would fit, and `dl <ws>
  'review this pr' --rm` was `Unknown command 'review this pr'`. The two
  flag-spelled verbs are now a suffix: recall the previous line, type `--rm
  --force` at the end of it, and the workspace goes. `dl prune <ws> --force` with
  `--rm` appended works too — a leading verb word is no longer mistaken for the
  workspace name — and `aid` peels the same flags off the end of its own argv, so
  the prompt never reaches an agent. Whatever the suffix displaced is named on
  stderr before anything is removed (`--rm overrode the rest of the line: 'review
  this pr' was not acted on.`), because a line that deletes a workspace must not
  silently carry an instruction it will not carry out. The peel is bounded to the
  exact words at the very end of the line, so a prompt mentioning `--rm`, or one
  ending in a bare `--force`, is untouched; a `--` command tail cannot be
  overridden at all. Unsaved work still refuses the delete without `--force`.
  Divergence row 30.

### Removed

- **`prune` is no longer a spelling of the `rm` verb.** `dl <ws> prune` deleted one
  workspace; `dl --prune` removes clone directories and no workspace at all. One
  word, two unrelated commands, told apart by two dashes — so reaching for the wrong
  one either lost a workspace that was meant to be kept, or was refused with
  `--prune takes no workspace: it is not a workspace command.`, a sentence that
  cannot explain what happened. The verb spelling now says what to use instead and
  names both commands. `dl --prune` is unchanged, and `dl <ws> rm` is what deletes a
  workspace. The word is still recognised rather than forgotten, so it is never read
  as a workspace name: a `dl prune <ws> --force` line recalled with `--rm` appended
  still removes `<ws>`, and `dl stop prune` still stops a workspace that really is
  called `prune`. Divergence row 31.
## [0.1.2] - 2026-08-20

### Added

- **A clone refused as not-found names the owner dl knows.** `dl kinisi/kinisi_ros`
  where the repository is `kinisi-robotics/kinisi_ros` used to end at git's own six
  lines — "Repository not found" plus ssh advice — which read identically to a
  permissions problem and named nothing to try instead. dl now checks the same
  completion cache the shell offers and adds one line: `Did you mean
  'kinisi-robotics/kinisi_ros'?`, with the branch carried back when the spec had one.
  Only a host's own not-found wording earns it, and only when the cache does *not*
  also know the repository under the owner typed — so a refused key, a dead network
  and a revoked permission are all left alone. git's text above it is unchanged in
  every case. Divergence row 29.

## [0.1.1] - 2026-08-20

The Rust port's review follow-ups ([#265](https://github.com/blooop/devlaunch/pull/265),
closing #262, #263 and #264). No command grammar or JSON shape changed.

### Fixed

- **Three messages render Python's exact wording again.** An unsafe git ref name reads
  `Invalid git ref name: '--evil'` instead of a Rust debug rendering, and an unfetchable
  recorded default branch gets Python's own sentence (`Cannot fetch recorded default
  branch: ...`). Restorations, not divergences — the parity goldens pin them.
- **Provisioning notices stream as they happen** instead of arriving in a batch after the
  install finishes — a cold tool install streams hundreds of megabytes, and the warning is
  worth something while it is still happening. Line bytes unchanged.
- **A `--purge` failure is reported once**, at the step where it happened, instead of twice.
- **`--reconcile` reports each adoption's actual ending** — done or refused, in the order they
  were attempted — instead of inferring success from absence in a refusal list.
- **Two overlapping completion-cache refreshes can no longer clobber each other**: each write
  stages through its own per-target, per-process temp name. Final paths and bytes unchanged.
- **The background completions refresh survives `pixi global update` swapping the binary
  mid-run**: when `current_exe()` names a deleted path, the refresh child is spawned by bare
  name through `PATH` instead of failing silently.

### Changed

- **`devlaunch-core`'s public API is frozen by CI.** A `cargo public-api` snapshot
  (`rust/devlaunch-core/public-api.txt`) is checked on every PR; 50 declarations the binaries
  never name were demoted to `pub(crate)`. The `--ls --json` document remains the one external
  contract.
- **The strict `metadata.json` reader is divergence row 28**: `NaN`/`Infinity`, numbers beyond
  f64 range and lone-surrogate escapes — values no build of dl ever writes — read as corruption
  and quarantine the file (bytes intact) where Python's `json.loads` accepted them.

## [0.1.0] - 2026-08-19

### Changed

- **`dl` and `aid` are Rust.** Same commands, same cache, same `metadata.json`; a compiled
  binary instead of a Python package. The port, its order, and the complete list of what
  changed on purpose are in
  [docs/rust-rewrite-plan.md](https://github.com/blooop/devlaunch/blob/main/docs/rust-rewrite-plan.md)
  — the [divergence table](https://github.com/blooop/devlaunch/blob/main/docs/rust-rewrite-plan.md#divergence-table-grade-c)
  is that list, 24 numbered rows, and nothing outside it changed by intent. The Python build is
  feature-frozen at 0.0.29 and stays in the repository as the reference the parity harness runs
  against.

  The four rows most likely to be noticed:

  - **The fuzzy selector is built in** (row 6). No `fzf` on `PATH`, no `iterfzf` — one launch-failure
    class gone. With no terminal (`dl < /dev/null`) the picker declines silently where fzf wrote
    `inappropriate ioctl for device`; stdout and the exit code are unchanged.
  - **Reserved verbs win over bare workspace specs** (row 1). `dl stop` opens the selector to stop
    something rather than looking for a workspace named `stop`, and every verb takes the workspace
    on either side (`dl stop <ws>` and `dl <ws> stop`).
  - **Usage errors exit 2** (row 14), clap's convention, where the old build exited 1: unknown flags,
    `--json`/`--size` without `--ls`, two global commands at once, and the other combinations the
    documented grammar never meant to accept and used to discard silently (row 15).
  - **No tracebacks, ever** (row 4). A failure is one line with the same exit code; OS reasons read in
    Rust's phrasing (`Permission denied (os error 13)`).

- **Both channels ship two binaries and nothing else.** The PyPI wheel is a maturin bin-wheel
  (linux-64, `manylinux_2_28`: glibc 2.28 and newer — Ubuntu 20.04, RHEL 8) and the conda package is
  built with `cargo install`. Neither depends on Python, `iterfzf` or `tomli` any more; `devpod` on
  `PATH` is still the one requirement, and the conda package still declares it. The version is read
  from `rust/Cargo.toml` by everything that names one, so the wheel, the conda package and
  `dl --version` cannot disagree.

### Removed

- **The `(dev, editable from <tree>)` suffix on `--version`** (row 16). It came from the installed
  Python package's PEP 610 metadata, which a compiled binary does not have; `dl --version` now always
  prints the bare version. Released builds printed identically before.

### Rollback

0.0.29 is the last Python release, and going back to it needs no cleanup: both builds read and write
`metadata.json` at schema 2 under the same locks, so every workspace survives the downgrade.

```bash
pixi global install --channel conda-forge --channel https://prefix.dev/blooop "devlaunch<0.1"
pip install "devlaunch<0.1"
```

## [0.0.29] - 2026-08-18

### Fixed

- **The host's Agent Skills reach the container: `~/.claude/skills/` and
  `~/.claude/wf-skills/` are mounted read-only.** The claude-code feature's
  allow-list left both out, so a workspace this repo's devcontainer built came up
  with no `~/.claude/skills` at all — every skill installed on the host invisible
  inside, `/wf` and the rest of blooop/wayfinder's bundle included, which is how
  a `wf` launch into a node lands in a session that cannot see the skill it was
  launched to run. Two paths rather than one because `wf skills install` leaves
  *relative* links (`skills/wf -> ../wf-skills/wf`) and the bodies live in the
  sibling: mounting `skills/` alone delivers a directory of dangling links.
  Read-only, like `commands/` and `hooks/`, so the allow-list gains no write path
  onto the host — a skill is executable instructions. The one cost: `wf` refreshes
  its links on every launch, so a `wf` run from *inside* a container now prints
  that it could not, and carries on.

- **A launch reads every arm of the base answer, so a third one cannot be mistaken
  for a fresh base** ([#245](https://github.com/blooop/devlaunch/issues/245)).
  0.0.28 shipped the stale-base report; `prepare_cold` still consumed it with a bare
  `isinstance` check, which would have read an arm added later as a fresh base — the
  silent launch-from-stale-cache the sum type was introduced to close. The dispatch
  now names both arms and refuses an unhandled one, and the default branch travels
  as one name-or-why value rather than a name and a reason that can disagree.

## [0.0.28] - 2026-08-18

### Fixed

- **A launch that could not refresh the ref it bases on now says so, instead of only
  logging it** ([#245](https://github.com/blooop/devlaunch/issues/245)). 0.0.27 shipped the
  guarantee that a new branch is cut from the default branch's freshly fetched tip; this
  closes the three degraded arms beside that happy path, each of which used to proceed
  from a cache of unbounded age behind nothing but a `logger.warning`: the follow-up fetch
  of the default branch failing after the remote had confirmed the branch is new, the
  default branch not resolving at all (which cut the branch from the bare cache's own
  `HEAD`), and a recorded default-branch name the fetch validator refuses. Observed in the
  wild on `blooop/bencher`: a workspace checked out **201 commits behind** `origin/main`
  reporting success, which then reproduced a deprecation warning upstream had already
  fixed.

  `ensure_branch` now answers with which of the two things it delivered — a fresh base, or
  a stale one naming the unrefreshed ref and the reason nothing refreshed it — and a cold
  launch turns a stale answer into one consequence-stating line: `Prepared
  'owner/repo@branch' from the cache's 'main', which could not be refreshed (…); it may be
  behind the remote.` That is the line an agent harness reading `dl`'s output can act on,
  and it is emitted only when it is true.

  **Nothing became fatal, deliberately.** Launch-from-cache on an unreachable remote is
  the contract [#144](https://github.com/blooop/devlaunch/issues/144) settled and it is
  unchanged: losing the network still costs you freshness rather than the workspace. The
  happy path's network cost is unchanged too — still one targeted fetch, two when the
  branch is brand new. What changed is that a stale base can no longer pass for a fresh
  one silently.

  Two limits worth stating, both unchanged and both scoped out on purpose. Relaunching a
  workspace clone that already exists does not advance it — a plain checkout preserves
  local work, and the fast attach skips host preparation altogether — so freshness is a
  property of the launch that *creates* a workspace. And a branch that already exists on
  the remote is launched at its own tip, which may legitimately sit behind the default
  branch.

## [0.0.27] - 2026-08-18

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

### Added

- **Every workspace now has `zellij` on `PATH`, so an agent can open a terminal beside
  itself** ([#242](https://github.com/blooop/devlaunch/issues/242)). With a session
  running in the container, `zellij -s devlaunch action new-pane -- <cmd>` opens a
  working pane from a completely non-interactive command, with no terminal attached to
  anything — which is what lets an agent inside a container hand you a shell next to it
  that you can attach to and type in. It depends on no dotfiles and on no repo's
  `devcontainer.json`, because `dl` launches arbitrary repos and a guarantee that
  depended on the image would not be one.

  zellij comes from pixi, container-side, and that was decided by measurement rather
  than taste: a warm `pixi global install zellij` against the shared package cache cost
  0.56s, 0.23s and 0.23s over three fresh containers (3.0s against an empty cache).
  The alternative on the table — mounting a static musl binary downloaded host-side —
  is faster still but adds an upstream GitHub-release dependency and a bind mount that
  only lands at container creation, which is not worth buying for a fifth of a second.
  Because nothing here is a mount, an existing workspace picks zellij up on its next
  `dl <workspace> restart` rather than needing a full recreate.

  It rides the setup pass that every `devpod up` already pays, so it costs **no extra
  round trip**, and every launch after the first is one `command -v` — the whole warm
  pass measured at 50ms. It also **cannot fail a launch**: provisioning is a stage,
  whose failure is contained and reported by name, so a container with no network and
  no pixi opens exactly as it would have, without zellij. `DEVLAUNCH_NO_TOOLS=1` skips
  it along with the rest of tool provisioning.

- **`DEVLAUNCH_ZELLIJ=1` makes `dl <spec> -- <command>` run beside a zellij session**
  ([#242](https://github.com/blooop/devlaunch/issues/242)), so the command can open
  panes into it. **Off by default**, and off means no existing invocation changes
  meaning. The command runs *beside* the session rather than inside a pane of it, on
  purpose: a pane would hand the command's stdin, stdout and exit status to zellij, and
  `dl <ws> -- cmd > file` has to keep putting the command's own output in the file. A
  bare `dl <workspace>`'s session is untouched either way — it sends no command to
  wrap, which is what gets it a terminal from devpod, and it lands you in a login shell
  where `zellij attach -c devlaunch` reaches the session by hand. The one command a
  bare attach can send ahead of that shell is the opt-in `DEVLAUNCH_DOTFILES_ON_ATTACH`
  refresh, which is wrapped like any other command, so running both switches together
  means the session is already waiting when the shell arrives.

- **Every container `dl` creates now shares one host directory of downloaded pixi
  packages** ([#232](https://github.com/blooop/devlaunch/issues/232)). `devpod up`
  gains a bind mount of `~/.cache/devlaunch/pixi` (under `$XDG_CACHE_HOME` if you
  set one) onto `/var/tmp/devlaunch-pixi`, plus `PIXI_CACHE_DIR` pointed
  at it for both the workspace and the dotfiles install script — dotfiles that
  provision their tools with `pixi global sync` are the consumer, and devpod gives
  that script an environment separate from the workspace's, so the assignment is
  passed twice.

  Measured on a real pair of containers while building this: the first container
  downloaded 667 MB; the second, a fresh container finding the packages already
  there, finished its sync in **16 s having fetched 13 KB**. Two containers syncing
  against one empty cache at the same time both exited 0 and between them downloaded
  the same 667 MB rather than 667 MB each.

  **Deleting the directory is always safe**, at any moment, including under running
  containers — it holds nothing but re-fetchable package archives, and nothing inside
  a container refers into it. `dl --purge` takes it with the rest of the cache.

  Only the *download cache* is shared. `PIXI_HOME` is not, and must not be: installed
  environments are baked with absolute paths, and two containers on one environment
  tree is [pixi#5476](https://github.com/prefix-dev/pixi/issues/5476). Nor is it the
  host's own `~/.cache/rattler/cache` — a dedicated directory keeps a host-side
  `pixi clean cache` from pulling packages out from under a live container, and keeps
  containers writing as their own remote user out of a cache you rely on. A cache
  directory that cannot be created costs the sharing and not the launch: the container
  downloads its own packages, exactly as it did before. Nor does a source that is
  gone by the time the launch reaches it: the mount arguments are emitted only when
  the directory really exists, because a bind source that is not there fails
  `devpod up` and the `ssh` that follows a failed `up` starts the container anyway.

  **The container-side path is `/var/tmp/devlaunch-pixi`, and both halves of that
  are load-bearing** ([#240](https://github.com/blooop/devlaunch/issues/240)). It is
  outside every home directory, because a bind target whose parent the image does not
  ship is created by the runtime as `root:root` — pointed into `~/.cache`, this mount
  took the container's own home cache away from it on every image that ships no
  `~/.cache`, which the stock `devcontainers/base:ubuntu`, `base:ubuntu-24.04` and
  `rust:latest` do not. And its parent is world-writable, because a mount lands only
  when a container is created while `PIXI_CACHE_DIR` is re-applied on every `up`: a
  container that predates this gets the variable pointing at bare image filesystem,
  where pixi does not degrade to reading but fails the install outright. Measured as
  uid 1000 on the stock base with nothing mounted, `pixi global install jq` exits 1
  with `Permission denied` under `/var/cache/devlaunch/pixi` and 0 under
  `/var/tmp/devlaunch-pixi`. Being out of `$HOME` also puts the cache beyond dotfiles
  installs that chown a root-owned `$HOME/.cache` — which this mount used to
  manufacture, and the chown wrote through the bind onto the host.

  Both earlier targets existed only on `main`: [#232](https://github.com/blooop/devlaunch/issues/232)
  merged 2026-08-15 and the newest release, `v0.0.26`, is dated 2026-08-14, so **no
  release ever carried them** and there is nobody to migrate. What does need a
  migration is any container you built from `main` in between: mounts are fixed at
  creation, so it keeps the target it was born with until `dl <workspace> recreate`,
  and a `~/.cache` an earlier build left root-owned heals on that recreate but not on
  a `stop` and `up`.

  **Sharing requires the container's user to be able to write the host directory** —
  its uid matching yours, or root. This is a documented constraint, not something
  `dl` fixes: pixi fails an unwritable cache outright rather than reading it, and
  `dl` cannot see a container's uid before launching it. The common case is safe,
  since every mainstream base runs at uid 1000; the README states the limit and the
  recovery.

- **`DEVLAUNCH_DOTFILES_ON_ATTACH=1` refreshes dotfiles just before an interactive
  attach hands over the shell** ([#183](https://github.com/blooop/devlaunch/issues/183)).
  devpod applies dotfiles only when it *provisions* a workspace, so a long-lived one
  keeps whatever it was born with until somebody runs `dl <ws> dotfiles`. Set the
  variable and `dl` runs that same refresh for you, in front of the shell rather than
  behind it — dotfiles that arrive after the shell has started are dotfiles it has
  already finished sourcing.

  **Off unless you set it, and that is the feature.** An earlier attempt refreshed on
  every attach and cost every user a `devpod ssh` round-trip — ~1.7s, of which ~99% is
  connection setup — plus a git pull, to close a gap most of them did not have. Someone
  who has not asked for this sees no change at all: the spawn-count pins that measure
  the attach path are unedited and still green, which is the actual claim rather than
  the flag being present.

  Two limits, both deliberate. It **never runs for `dl <ws> -- <command>`**, on the same
  reasoning the attach has always applied — a one-shot renders no prompt and sources no
  interactive shell, so a refresh in front of it buys that command nothing and costs it
  a round-trip; that is the path agent launchers use. And it is **bounded at 60
  seconds**, spent inside the container, so an unreachable dotfiles remote or one
  waiting on a password nobody is there to type is a pause and then your shell rather
  than a hang. The bound signals the whole process group, so the git process actually
  doing the waiting dies too instead of lingering with the session held open.

  `dl <ws> dotfiles`, typed by hand, is unchanged and still unbounded: a deadline is
  worth having on the refresh nobody asked for, and is a way to abandon a half-finished
  `pixi global sync` on the one somebody did.

- **`dl --reconcile` re-points devpod workspaces the id-scheme change orphaned**
  ([#88](https://github.com/blooop/devlaunch/issues/88)). When workspace ids and clone
  directories gained a hashed suffix, `dl`'s own records were migrated and devpod's were
  not — on the reporting host, **36 of 39 devpod workspaces recorded a source folder that
  was missing (35) or was a config-only stub devpod had rebuilt from its cache (1)**,
  while the real checkout sat beside it under the new name. This matches the two record
  sets **by path, never by id** — the id is exactly what moved, so it joins nothing —
  rewrites devpod's `source.localFolder` at the clone that holds the checkout, and fills
  in the workspace id on `dl`'s record.

  **It deletes nothing, and it guesses at nothing.** A clone a live workspace already
  opens is never taken from it, a clone two dead records both match is claimed by
  neither, and an orphan with no clone to adopt is named and left standing — `dl` cannot
  know whether a workspace is finished with, and the two mistakes are not the same size.
  Shaped like `--purge` and `--prune`: print the plan, name what is being left and why,
  confirm, `-y` to skip. Running it twice changes nothing.

  It is deliberately **not** a mode of `--prune`, which states as its contract that it
  never touches a devpod workspace. A re-pointed workspace still needs
  `dl <workspace> recreate`: its container was built with the dead path bind-mounted, and
  no record change moves a mount. The plan says so before asking, not after acting.

- **`DEVLAUNCH_TIMING=json` reports a launch as one machine-readable document**, decomposed
  into five ownership-boundary stages — `handoff`, `host-prep`, `devpod-up`, `tools`,
  `attach` — with the finer per-subprocess spans nested inside the stage that paid for
  them, plus the total ([#194](https://github.com/blooop/devlaunch/issues/194)). A stage
  is total over its whole arm rather than over its round trips alone, so the host-side
  work between two spawns is attributed rather than lost. `DEVLAUNCH_TIMING=1` is
  untouched: the same flat prose summary, the same labels.

  A stage that never ran is **absent** from the document, never a `0.000s` that would
  claim it ran and cost nothing; a stage that raised on its way out is present, timed up
  to the failure, and marked `failed`. `worktree/` (the bare clone and its fetches, the
  lock waits, the LFS probe) and `tools.py` (the payload tar) gained spans, which the
  unmeasured launch still pays nothing for — with the switch off, both `span` and `stage`
  are the stdlib no-op.

- **Two env vars naming the hand-off seam**, read by dl and written by whatever launches
  it. `DEVLAUNCH_HANDOFF_T0` is the keystroke that resolved to this exec, as Unix epoch
  seconds (what `date +%s.%N` prints); the gap from it to dl starting becomes the
  `handoff` stage, which is the only measurement of exec plus interpreter startup that
  exists — `dl-timing: total` begins after both. `DEVLAUNCH_PREWARM_FIRED_AT` is when a
  prewarm was fired for this workspace, if one was.

  From the pair, the document reports what the prewarm was worth: the head start it
  bought, and which shape the launch then took — `hit` (already up), `partial` (queued
  behind a prewarm still running), or `miss` (this launch ran the `up` itself). The
  outcome is derived here rather than claimed by the firer, which fires and forgets and
  so can only stamp its own past action. No stamp means no field at all — not a zero, and
  not a `miss`.

- **`--prune` and `--purge` now end by naming the disk they do not free**
  ([#160](https://github.com/blooop/devlaunch/issues/160)) — one line, the same
  in both: devlaunch does not manage Docker images or volumes, the containers
  these workspaces used may still hold disk, and `docker system df` shows what
  Docker is holding. On the host this was measured a prune had 4.00 GB of stale
  clones to give back while `docker system df` read 86.5 GB of reclaimable
  images, 43.18 GB of volumes and 13.88 GB of build cache — so a freed figure
  printed on its own reads as all of it, and is off by an order of magnitude.
  A purge or prune you answered `n` to ends on it too: a report of what would
  go is exactly where the disk that would not is worth naming.

  A sentence, not a measurement: no `docker` process is started, so there is
  nothing to be slow and nothing to fail where Docker is absent or stopped. It
  points and never offers — no image ids to paste into `docker image rm`, and no
  `docker image prune -a`, which is unscoped and would take images devlaunch
  never built (the footgun [#129](https://github.com/blooop/devlaunch/pull/129)
  removed from `--purge`). devpod's images carry no devlaunch or devpod label, so
  a list of "yours" would be a guess.

### Fixed

- **Launching a branch now fetches exactly that ref first, every time — push upstream,
  then `dl` the branch, and you get the pushed tip**
  ([#144](https://github.com/blooop/devlaunch/issues/144), built in
  [#149](https://github.com/blooop/devlaunch/issues/149)/[#150](https://github.com/blooop/devlaunch/issues/150),
  merged as [#185](https://github.com/blooop/devlaunch/pull/185), which shipped without a
  changelog entry of its own; this adds it). The launch path used to refresh the bare
  cache through an interval-gated fetch, so any launch within `fetch_interval` (an hour
  by default) of the last one ran against whatever the cache held. The visible casualty
  was every brand-new branch — including each ticket branch `wf` asks devlaunch to cut —
  which was based on the cached default-branch tip rather than the remote's current one,
  silently up to an hour out of date with `main`. Now the requested ref is fetched
  unconditionally, and a branch that exists nowhere yet is created from a freshly
  fetched default-branch tip — one more targeted fetch, and no more than one. Offline
  behaviour is unchanged by design: a fetch failure warns and the launch of a cached
  branch proceeds. Every other ref still converges within `fetch_interval` via the
  detached updater sweep, so nothing new blocks the launch path.

- **An ssh key stored under a directory with a space in its name now reaches ssh whole**
  ([#225](https://github.com/blooop/devlaunch/issues/225)). Pushing a new branch with a
  named key builds `GIT_SSH_COMMAND`, which git hands to a shell rather than running as
  argv — so an unquoted path was split on whitespace, ssh got a truncated `-i` and the
  rest of the path as a hostname, and the push failed on the one piece of setup that
  naming a key exists to guarantee. The path is now shell-quoted, so any key path works
  regardless of what is in it. Keys at paths without shell metacharacters behave exactly
  as before.

  **A push git said nothing about now names its exit code instead of reporting `None`.**
  When the failure carried no stderr, the error read `Failed to push branch to remote:
  None` — a message with nothing in it to act on. Same class of gap
  [#212](https://github.com/blooop/devlaunch/issues/212) guarded against in the
  branch-creation arm, one function over (that arm printed an empty tail on a silent
  failure until [#234](https://github.com/blooop/devlaunch/issues/234), below, brought all
  three arms onto one answer).

- **A branch creation git said nothing about now names its exit code too**
  ([#234](https://github.com/blooop/devlaunch/issues/234)). A creation that failed with no
  stderr to quote raised `Failed to create branch: ` — a message that ends at the colon,
  and so reports only that something went wrong, which the exception itself already said.
  It now reads `Failed to create branch: git branch exited 128`, the answer the push arm
  has given since [#225](https://github.com/blooop/devlaunch/issues/225). Failures git did
  explain read as before, save that the quoted text no longer drags git's trailing newline
  into the middle of the sentence; which failures count as the benign "branch is already
  there" is unchanged.

  Behind all three — branch creation, branch push, and ref fetch — the text is now derived
  in one place rather than three. The missing-stderr guard, the trim and the exit-code
  fallback can no longer drift apart between siblings, and the reasoning for them is
  written once where they share it instead of restated at each site.

- **The rest of the git failures `dl` reports now name their exit code instead of
  reporting `None`** ([#238](https://github.com/blooop/devlaunch/issues/238)). Five
  messages were left quoting git's stderr raw, and printed the word `None` whenever git
  failed without writing any — the cached clone of a repository, the sweep that fetches
  every ref in it, the workspace clone taken from that cache, the remote repoint that
  follows it, and the branch checkout every launch runs. The last two are the ones most
  likely to be met: a checkout runs on every warm launch, and a local clone that fails
  usually fails for a reason git has nothing to say about, such as a full disk. They now
  read `Failed to checkout branch 'x': git checkout exited 128` and so on, each naming
  the subcommand that failed. Failures git did explain read as before, except that the
  quoted text no longer drags git's trailing newline into the middle of the sentence.

  With these, every worktree message that quotes a failed git's captured stderr derives
  its text in the one place [#234](https://github.com/blooop/devlaunch/issues/234)
  established. Two sites stay outside it on purpose: the git-lfs pull never captures
  stderr at all, so the exit code it already reports is everything it ever had to say;
  and the working-tree read behind the unsaved-work check that `dl --prune` and
  `dl <ws> rm` both consult inspects a completed process rather than an exception, and
  so cannot encounter the missing stderr the shared helper exists to guard.

- **`dl --reconcile` and `dl --prune` no longer answer differently depending on which
  directory you ran them from** ([#224](https://github.com/blooop/devlaunch/issues/224)).
  Run from inside `<repos-root>/<owner>/<repo>/`, `--reconcile` listed every workspace
  devpod sources from a git URL — anything started by `devpod up <url>`, by another tool,
  or by an older `dl` — as an orphan of whichever repository you happened to be standing
  in, at invented paths like `<root>/blooop/devlaunch/git@github.com:blooop/wayfinder.git`.
  From a neutral directory the same command listed none of them.

  A workspace source that carries a local path has to keep counting — `devpod up
  <path-to-a-repo>` records one, and a path `--prune` does not know about is a directory
  it would call unreferenced — so the arm was resolved as a path unconditionally. A remote
  URL is relative-looking text, so resolving it produced a path under the current
  directory. Text that is URL-shaped is now recognised as naming a repository elsewhere
  before anything tries to resolve it, and contributes no location at all; text that is a
  path is treated exactly as before. (`file://` URLs count as URL-shaped: they previously
  resolved to garbage like `<cwd>/file:/...`, so they contributed nothing real to lose.)

  **Nothing was ever deleted or re-pointed because of this.** Every affected path ran
  toward refusing: `--reconcile` reported and adopted nothing (no clone directory can be
  named the same as a URL), and `--prune` withheld clones it could have offered. What
  changes is that the reports are now about your repositories rather than about where your
  shell was.

- **A cache migration that could not rename everything no longer marks itself done**
  ([#180](https://github.com/blooop/devlaunch/issues/180)). The one-shot migration onto the
  post-#64 id scheme renamed what it could and then wrote the new schema header regardless.
  A rename the filesystem refuses — a read-only mount, a directory whose permissions were
  tightened, a full disk — is not a crash, so it never got the crash's resume treatment: the
  header said 2, the next run's version comparison returned immediately, and those records
  kept their pre-#64 `workspace_id` for good. Since `remove_workspace_by_id` matches the id
  `dl` derives today, `dl <owner>/<repo>@<branch> rm` could no longer find them, and a clone
  directory that may hold uncommitted work would be orphaned alongside its record.

  The header now advances only when nothing was refused. The save still happens either way,
  so renames that did work are recorded immediately and are not repeated; the next run finds
  them as the already-documented "destination present, source gone" resume and retries only
  the directories that were refused.

  **`MetadataStorage.save` writes the version it loaded rather than the current constant**,
  which is the half that makes the rest hold: the migration is not the only thing that
  writes `metadata.json`, and a save from opening a workspace or reconciling would otherwise
  re-stamp the current version and re-strand exactly the records the migration had
  deliberately left for a later run. A file written by a *newer* build is still rewritten at
  this build's version, because its entries have just been rewritten in this build's shape.

  **In practice this is template hygiene rather than a live rescue.** Caches still on the
  old scheme are effectively nonexistent — the builds that wrote them saw almost no use —
  so this is unlikely to have stranded anyone's workspace. It is worth fixing because this
  migration is the code a future schema bump gets copied from, and a header that can claim
  more than the filesystem has done is the kind of defect that gets copied with it. A cache
  that refuses permanently now re-reports on every invocation; the walk is bounded and the
  notice names directories that really do still need a hand.

- **The `claude-code` feature's read-only mounts now exist**
  ([#108](https://github.com/blooop/devlaunch/issues/108)). The feature's README documented
  granular mounts, with `CLAUDE.md`, `settings.json`, `agents/`, `commands/` and `hooks/`
  read-only, and gave the reason: those files hold *executable instructions*, so a prompt
  injection that edits one is not confined to the session that fell for it — it is on the
  host, and it runs again in every later session, in every other container. The manifest
  mounted the whole of `~/.claude` read-write, so the container could write every one of
  them. The protection was documented and absent from the first commit; there was no
  regression to find, and the feature's manifest had never been edited since.

  The mounts are now the list the README describes, plus the two files it argues have to
  stay writable — `.credentials.json` for token refresh, `.claude.json` for onboarding
  state. Checked in a real container rather than on the page: with this feature's mounts in
  place, `/proc/mounts` reports `ro` for all five protected paths, an append to `CLAUDE.md`
  and a new file under `hooks/` are refused with `Read-only file system`, the host's files
  are unchanged afterwards, and the writable pair still takes a write.

  **What this costs.** The mount list is an allow-list now, so everything else under
  `~/.claude` — session transcripts, `projects/`, `history.jsonl`, `skills/`, `plugins/` —
  is the container's own and dies with it. A container no longer resumes a session started
  on the host, and the skills and plugins installed on the host are not visible in there.
  That is what the allow-list buys: a directory Claude starts writing to next month cannot
  silently become another way onto the host.

  **What it does not cost.** Every mounted path has to exist before the container starts,
  and the host-side hook this repo already runs before each create now creates all of them,
  rewriting nothing that is already there — which is what lets that hook keep running
  *inside* a container built this way, where those paths are the read-only mounts. A host
  with no `~/.claude` fails exactly as it did before this change: `bind mount source path
  does not exist`, refused before any container exists, leaving nothing behind on the host.

- **`dl --purge` no longer says it removed what it was permitted to when it removed
  nothing** ([#182](https://github.com/blooop/devlaunch/issues/182)). The removal's answer
  was a flat list of refusals, which records *what refused* and never *whether anything
  went*, so a cache whose root was itself the obstruction — sealed, a symlink, or one that
  could not even be looked at — printed `Removed what was permitted under <cache>` with
  the whole cache still standing. The per-refusal reasons underneath were right in both
  cases, so nobody was left without information; the headline was simply false, and it is
  the line somebody reads before deciding whether they still have clones to go and find.

  The removal now answers with one of three values — removed everything, removed what it
  could, removed nothing — and the two that can carry refusals are the only two that have
  anywhere to put them, so "removed everything, and here is what it refused" is not a
  value this code can build. Whether anything came away is counted as it happens rather
  than inferred from the report afterwards, which is the inference that could not be made.
  A total refusal now reads `Removed nothing under <cache>. These refused:` over the same
  report as before.

  **The exit status is unchanged and stays two-valued**: `0` means the cache is gone and
  nothing else does, which is the only distinction a script can act on, and a third code
  would be an interface to keep forever. Which of the two failures happened is in the
  sentence, where the person who can act on it reads it. `dl --prune` shares the removal
  and its behaviour is unchanged — a clone directory is one unit of work there, and only
  the arm that says it is entirely gone counts it as removed. A fourth outcome would now
  be a type error at every reader rather than being read silently as one of these three.

- **The devcontainer feature's installer now writes its PATH lines into the profile bash
  actually reads** ([#191](https://github.com/blooop/devlaunch/issues/191)). bash sources
  only the first of `~/.bash_profile`, `~/.bash_login` and `~/.profile` that exists, which
  is why `dl`'s provision and lend scripts resolve the file instead of naming one.
  `.devcontainer/claude-code/install.sh` named `~/.profile` flatly, so on an image
  shipping a `~/.bash_profile` it wrote both `# devlaunch:`-marked lines into a file
  nothing ever sources — and looked for its marks there too, missing the identical lines
  the provision script had written in the file bash *does* read. The feature's PATH setup
  was dead on exactly those images, and the two writers deduped against different files.
  Latent rather than live on this repo's own base image, which ships no `~/.bash_profile`.

  The resolution is now rendered from one place and pasted into the installer with
  `$TARGET_HOME` for `$HOME` — the installer edits a home it does not run in — the same
  way the two marked append lines have been pasted since
  [#164](https://github.com/blooop/devlaunch/issues/164). A test runs each writer's edit
  in a scratch home of every shape and then asks a real login `bash` what its PATH is, so
  what pins this is bash's own answer rather than a rule restated; a second asserts each
  writer carries the one rendering, so the copies cannot drift apart again.

- **`dl` now writes down the devpod workspace id it creates, and follows it**
  ([#88](https://github.com/blooop/devlaunch/issues/88)). The id `dl` handed devpod was
  derived from `(owner, repo, ref)` on every command and stored nowhere, so the
  derivation was the only copy of it in existence — and when it changed, every workspace
  created under the old one stopped being addressable in the same instant. `WorktreeInfo`
  has declared a `devpod_workspace_id` field since the worktree backend was written and
  nothing had ever assigned it; it is now written when a clone is prepared, including on
  re-registration, so records from older builds acquire one without a migration.

  A launch still derives an id and asks devpod about it first, and only consults the
  record when devpod **denies** it — which keeps
  [#145](https://github.com/blooop/devlaunch/issues/145)'s warm attach path clear of the
  metadata lock, the parse and the migration check that reading the record costs. A
  stored id devpod also denies is not used: `metadata.json` is append-mostly, so a record
  naming a workspace deleted months ago is ordinary, and addressing it would substitute
  one absent workspace for another. `stop`, `rm`, `restart`, `recreate`, `reset` and the
  attach all read the one resolution, so they were fixed together.

  This is the lesson `remove_workspace_by_id` already records for the clone *path*,
  applied to the id. It prevents the next derivation change from costing anything; it
  repairs nothing already broken, which is what `dl --reconcile` above is for.

- **A non-English host locale can no longer turn git's harmless "branch already
  exists" into a fatal error** ([#187](https://github.com/blooop/devlaunch/issues/187)).
  Creating the branch in the bare cache tells the harmless "it is already there" from a
  real failure by looking for `already exists` in git's stderr — and git translates that
  message. Measured against git's own German catalog: under `LANGUAGE=de` it reads
  `Schwerwiegend: Branch 'dup' existiert bereits`, so the harmless case was classified
  as fatal. In the shipped tree that arm is reachable only in a race — both callers
  check for the branch before creating it, under the repo lock — so on a non-English
  host this was a latent misclassification waiting at the seam, not an everyday launch
  failure; it is closed before anything makes the arm ordinary. The
  git call now runs with `LC_ALL=C` and `LANGUAGE=C` — both, so the guarantee does not
  rest on the one glibc rule that lets `LC_ALL=C` outrank `LANGUAGE` — layered on the
  inherited environment rather than replacing it, so ssh auth survives. Same hazard and
  same pin as the fetch path already carried; this was the one remaining site that
  classified translated text without it.

  Found alongside it and fixed with it: the fetch of the branch to base a *new* branch
  on tested only its failure outcome and let the rest fall through, so an outcome the
  sum type does not yet have would have been read as a clean fetch. Every arm is now
  named and an unrecognised one is rejected loudly, matching what the dispatch beside it
  already guaranteed. No behaviour changes for the three outcomes that exist today.

- **Pushing a branch with a named ssh key no longer strips git's whole environment**
  ([#212](https://github.com/blooop/devlaunch/issues/212)). The push handed git a bare
  `{"GIT_SSH_COMMAND": ...}`, which *replaces* the inherited environment rather than
  extending it — so the one call that was told which key to authenticate with ran with
  no `PATH` to find `ssh` on, no `HOME` to read `~/.ssh/known_hosts` or the key's own
  config from, and no `SSH_AUTH_SOCK` to reach the agent. Naming a key was what broke
  the auth it was meant to set up, and the failure only reached the machines that pass
  one. Same env-replacement hazard [#187](https://github.com/blooop/devlaunch/issues/187)
  closed at the locale pin, same fix: the key's ssh command is now layered on the
  inherited environment.

  Aligned in the same pass, from #187's review: the two places that classify git's
  outcome by reading stderr text disagreed on `stderr` being `None` — the fetch path
  guarded it, branch creation did not. A failure git wrote nothing to stderr for now
  raises the same "failed to create branch" error as any other failure, instead of a
  `TypeError` from testing membership in `None` that names neither the branch nor the
  cause.

- **The devpod provider guard's three entry points can be stood in for by a test again**
  ([#217](https://github.com/blooop/devlaunch/issues/217)). Each named
  `run=subprocess.run` as a *default argument*. Python evaluates a default once, when the
  module is imported, so all three held the real `subprocess.run` for the life of the
  process and `mock.patch` could not reach them by construction — the seam was written,
  documented and inert. A caller that named no runner spawned a real `devpod provider ...`
  from under a suite that believed it had replaced every process in the tree, and if that
  suite had also patched `Popen`, the real `run` built its process out of the stand-in and
  died inside CPython's `subprocess.py` with an `AttributeError` naming neither devpod nor
  the fixture. The default is now `None`, resolved to `subprocess.run` at call time, which
  is what the signature always read as doing.

  **What this does not claim.** It is the door, not a witness to anything having walked
  through it. Nothing in `devlaunch/` imports this module today, and its own tests all
  pass `run=` explicitly, so the once-observed `FinishedSession`/`kill` flake that led
  here still has no established origin — closing this removes the only known leak of that
  shape without proving it was the one. The named-error guard
  [#216](https://github.com/blooop/devlaunch/pull/216) added to the spawn-count fixture
  stays, and would name any recurrence for what it is rather than leaving it to surface as
  a missing attribute two libraries away.

### Changed

- **The cache migration's orphaned-container notice now offers repair before disposal**
  ([#227](https://github.com/blooop/devlaunch/issues/227)). It used to answer "N devpod
  containers are orphaned" with nothing but `xargs -r -n1 devpod delete`, advice written
  before `dl --reconcile` existed. A container orphaned by the migration is sourced at the
  path the migration just renamed, with the real clone next to it under the new name —
  which is exactly the case `--reconcile` adopts, so the notice now names `dl --reconcile`
  and the `dl <workspace> recreate` that finishes the repair first, and states the limit:
  that gives back the clone association and the workspace's identity, not state that lived
  only inside the old container. The bulk delete is unchanged and still printed, as the
  answer for workspaces you are finished with rather than the only answer offered. Nothing
  about what the migration does changed; only what it tells you to do next.

- **A cold launch now holds the per-repo lock across the whole of its host
  preparation, where it used to take and release it four times**
  ([#200](https://github.com/blooop/devlaunch/issues/200)). Clone-if-missing, the
  targeted ref fetch, branch creation and the workspace clone run inside one scope, so
  the sequence can no longer be interrupted partway through by another `dl` acting on
  the same repository between two of the old acquisitions.

  **What that costs is a wider serialization window, and it is worth knowing about.**
  Two cold launches of *different branches of the same repo* now queue: the second waits
  out the whole of the first's preparation rather than interleaving with it. Launches of
  different repositories are unaffected — the lock has always been per repo — and warm
  attaches are unaffected, because they take the lock zero times for a named branch and
  once for a bare `owner/repo`. The wait is the clone and one fetch, and a launch that
  sits on it says so.

  One user-visible message changed with it. The three per-step failures of the old
  sequence became one, which names what was being prepared: `Failed to prepare workspace
  'owner/repo@branch': …`. The repo and branch used to be carried by the individual
  messages and would otherwise have been dropped.

- **The `auto_fetch` config knob is gone**
  ([#188](https://github.com/blooop/devlaunch/issues/188)). It never gated anything. The
  fetch that shared its name was gated by a separate `ensure_repo` parameter, no caller
  ever passed the config value into it, and that parameter went away with the fetch when
  the launch path stopped sweeping every ref
  ([#150](https://github.com/blooop/devlaunch/issues/150)) — so the knob was inert for its
  whole life, not merely stranded by the rework.

  **A `config.toml` that still sets it needs no edit.** The loader reads the keys it knows
  by name and ignores everything else, with no unknown-key warning to start firing, so a
  stale `auto_fetch = false` is now simply passed over and the rest of the file applies as
  before. Nothing about how often devlaunch fetches changes: `fetch_interval` is untouched
  and still decides that.

- **The executable-doc guard now reads the README too, and `bench_launch.py` stops
  accepting abbreviated flags** ([#192](https://github.com/blooop/devlaunch/issues/192)).
  Three documented-command defects have shipped in this lineage — a reset naming a
  `delete` subcommand `dl` does not have (twice), a recipe that said `-n 5` beside a
  median taken over three runs, and `python scripts/bench_launch.py` published to readers
  whose host has no `python` outside pixi. Each was found by *running* the documentation.
  The guard built after the first two extracts the cold recipe from the script's own
  epilog and drives it through `dl`'s `main()` — and the third defect shipped in the
  README, which the guard never opened.

  It does now: every fenced invocation of a bench script under "Measuring launch time" is
  extracted and handed to whatever would have caught each defect. The **interpreter** goes
  to a list of the two routes this repo can vouch for, because no parser will ever see
  that word; the **flags** to the script's own parser; the **`pixi run` shortcuts** the
  prose offers beside them to the project manifest, since a renamed task leaves the
  sentence confidently wrong; and any **`--before` reset** to the same nothing-to-delete
  harness the epilog's reset already goes through. While the section documents no reset of
  its own, the guard holds it to the pointer that stands in for one, so the cold recipe
  cannot become unreachable from the page.

  `bench_launch.py` was built on an argparse that accepts any unambiguous *prefix* of a
  long flag, which is what made those guards weaker than they read: rename `--record` to
  something it is a prefix of and every document keeps saying `--record`, the parser keeps
  accepting it, and the rename ships behind a green guard. It now refuses abbreviations,
  the way the sibling points script always has, and a test abbreviates each documented
  flag by one letter to keep both parsers that way. No documented or scripted invocation
  used an abbreviation.

- **The bare cache is now the repo's git-lfs store, and workspaces hardlink out of it**
  ([#163](https://github.com/blooop/devlaunch/issues/163), deciding
  [#154](https://github.com/blooop/devlaunch/issues/154)). git-lfs objects are not git
  objects, so the cache that makes a workspace's history free carried none of the large
  files: every workspace of an LFS repo downloaded the whole payload from the forge and
  kept a private copy of it in `.git/lfs/objects`, on top of the worktree copy. `dl` now
  fetches the launched branch's LFS objects once into `<repo>/.bare/lfs` — with the bare
  as cwd, and with `lfs.fetchrecentrefsdays`/`lfs.fetchrecentcommitsdays` zeroed so one
  branch does not drag several branches' payloads down with it — and materializes each
  workspace with `git lfs pull "file://<bare>"`.

  **Measured** (git-lfs 3.7.1, ext4): that pull **hardlinks**, so the workspace's object
  file is the same `(st_dev, st_ino)` as the cache's and its store costs zero bytes, and
  it completes with the remote **deleted from disk** — the second workspace of an LFS
  repo touches the network for its large files not at all. The worktree copy git-lfs
  writes is the one per-workspace cost that remains, and is not shareable: a container
  build has to read those bytes.

  **Nothing host-specific is persisted into the clone** — no `lfs.storage`, no added
  remote — and that is the constraint the design is shaped around rather than a detail.
  `dl` bind-mounts the clone into the devcontainer while `.bare` is a sibling that is
  not mounted, so a host path in the clone's config breaks every in-container
  `git checkout` of an LFS repo while working perfectly on the host. (`-c lfs.storage`
  was rejected on top of that: it was measured to break against local-path remotes
  outright, because `GIT_CONFIG_PARAMETERS` is inherited by the remote-side git-lfs
  child.) An integration test holds the clone to exactly one remote, still the forge.

  **The old behaviour is unchanged as the fallback.** The `origin` pull still runs when
  a pointer survives the cache phase — a first launch offline, an object the cache
  cannot supply — decided by the same pointer-content predicate that opens
  materialization rather than by the cache commands' exit codes, since git-lfs can exit
  zero having fetched only some objects. Neither cache step can fail a launch, the
  `RuntimeError`-and-retry contract is the one it always was, and materialization is
  still not gated on "did we just clone this".

- **One setup pass on the way into a running container, and one fewer `devpod ssh` on
  both the cold and the warm path** ([#168](https://github.com/blooop/devlaunch/issues/168),
  deciding [#157](https://github.com/blooop/devlaunch/issues/157) and
  [#167](https://github.com/blooop/devlaunch/issues/167)). Naming the container — the
  hostname the shell prompt shows — was a `devpod ssh` of its own. It is now a named
  stage in front of the tools probe, in the trip the probe already pays, composed on the
  host from the shipped probe script verbatim so there is still exactly one definition of
  what the probe asks.

  **Measured, on this machine and this date**: the two steps composed into one trip take
  **1.64s** against **3.56s** run as two sequential trips — medians of 5, `/usr/bin/time`
  around real `devpod` against a real container under a throwaway `DEVPOD_HOME`. **~1.92s
  saved per cold launch.** A `devpod ssh --command` trip is ~1.73s and is ~99% connection
  and process setup, so collapsing two trips into one saves a whole trip, near exactly.
  (The per-call figure quoted elsewhere for `devpod list` / `devpod status` does not
  apply here: those never enter a container, and they cost a fraction of a trip that
  does.)

  The warm interactive attach drops the same trip and does not replace it: every entry
  into Running goes through the pass, so a workspace you attach to is already named.
  `dl <ws>` is now two devpod processes (`status`, `ssh`) instead of three, a further
  ~1.75s off the warm path.

- **A cold `dl <ws> -- cmd` and a cold `dl <ws> up` now name their container too**, where
  before they left it nameless until somebody attached interactively. It costs them
  nothing — the trip was already being paid.

- **Each stage of the pass reports itself**: `ok`, `failed` with its exit status, or *not
  reached* when no outcome came back at all, and dl names any stage that is not `ok`
  rather than discarding it. The hostname's failure reports at **info** level, because it
  fails by design on every unprivileged image — setting a hostname needs `CAP_SYS_ADMIN`,
  which Docker drops by default — so it is the majority case and a warning would erode
  the signal. Until now that failure was a boolean nothing looked at, which is why
  containers that could never be named looked exactly like containers that were.

- **`DEVLAUNCH_NO_TOOLS` no longer switches off container naming.** It opts a machine out
  of installing tools, and the pass that carries the naming now runs either way — whether
  the pass runs and whether the tools work runs are two questions. A machine with the
  opt-out set pays one round trip per `up` that it did not pay before.

- **A workspace clone sharing the bare cache's pack files is now a stated, tested design
  rather than a default nobody had written down**
  ([#162](https://github.com/blooop/devlaunch/issues/162)). No behaviour changes: this is
  the guard that was missing. `git clone <path> <path>` hardlinks pack files by default,
  and that default was all that kept every workspace after the first cheap — a `file://`
  URL, an intermediate copy, or an explicit `--no-hardlinks` would each have forfeited it
  silently, with git exiting 0 either way. Measured on this repo — `du -sc` over the cache
  and each clone's `.git`, ext4, git 2.55.0 — cache plus one workspace is **2400 KB**
  shared against **4472 KB** unshared, and each further workspace's `.git` costs
  **196 KB** instead of **2268 KB**. (#154 decided this on the same measurement over a
  smaller history: 2044 KB against 3788 KB, 180 KB against 1924 KB. The ratio is what
  holds; the absolute figures grow with the repo.)

  An `integration` test now asserts each pack file in a workspace *is* the cache's file —
  equal `st_ino`, `st_nlink >= 2` — and goes red on all three of those changes; it also
  refuses to pass over an empty pack set, which the suite's three-object fixture would
  otherwise have given it. **No clone flag was added**, deliberately: `--local` is already
  the default and does not even reject a `file://` source, so it pins nothing, and
  `--shared`/`--reference` were measured to leave an fsck-broken workspace after the
  cache's force-refspec fetch and gc, for a 2 KB saving.

  Two erosions are now written down where the clone happens, both measured. A repack of
  the cache drops an existing clone's pack to one link — its own complete copy, still
  passing `git fsck`, which is the safety property that makes alternates unnecessary. And
  a destination on another filesystem makes git fall back to a full copy with exit 0 and
  no warning (measured across ext4 and tmpfs); devlaunch's layout puts `.bare` and every
  clone inside one directory, so that one is unreachable by construction.

## [0.0.26] - 2026-08-14

### Added

- `dl <workspace> dotfiles` refreshes dotfiles inside a workspace that is already
  provisioned — `chezmoi update --force && pixi global sync` over one `devpod ssh`,
  falling back to cloning `DOTFILES_URL` and running `install.sh` when `chezmoi` is
  not on the image. devpod applies dotfiles only when it *provisions* a workspace,
  and attaching to a Running one deliberately skips `devpod up` altogether, so a
  long-lived workspace otherwise keeps whatever dotfiles it was born with. The
  subcommand starts the workspace first if it is not Running.

- `dl --prune [-y] [--force]` removes exactly the clone directories no live
  workspace opens any more, and nothing else
  ([#159](https://github.com/blooop/devlaunch/issues/159)).

### Changed

- Launch and update timing: the interval fetch moved into the detached updater, and
  an env-gated launch timing summary plus a median bench harness now make the
  remaining cost measurable rather than argued about.

- **The 0.0.24 lending entry no longer claims the transfer is "checksum-verified"**
  ([#152](https://github.com/blooop/devlaunch/issues/152)). Nothing in `devlaunch/`
  computes a checksum, and nothing ever did: the only gate is that both lent
  binaries are run in a staging directory, and nothing moves into place until they
  do. The entry now says that instead. The `5.1s` beside it is gone as well — it
  was an inherited working number that nobody measured, unlike the 342MB, which
  [#158](https://github.com/blooop/devlaunch/issues/158) reproduced and which is
  now attributed there.

  Noted here rather than corrected silently, because the edit changes a section
  that has already shipped: anyone who read the 0.0.24 notes before this was
  relying on an integrity check the lend does not perform, and a released claim
  about verification is not one to withdraw without saying so.

### Fixed

- **Two guards around the tools lend were weaker than what was claimed of them**
  ([#166](https://github.com/blooop/devlaunch/issues/166)) — the follow-ups the
  final re-verdict on [#164](https://github.com/blooop/devlaunch/pull/164) judged
  non-blocking, worked once. "The probe never executes the candidate `claude`"
  was asserted by scrubbing the rendered script's text for an invocation, and a
  probe that executed `"$(command -v claude)"` walked straight past it — once the
  lookup is scrubbed, no literal `claude` remains in the execution. The property
  is now proven behaviourally: the probe runs against a claude that records
  being invoked (shell builtins only, so the recorder cannot silently fail on
  the probe's stripped `PATH`), and the record must stay empty. The shipped
  probe passes — it never did execute the candidate — so this hardens a guard
  rather than fixing a live defect; the text scrub stays as a complement under
  a docstring that no longer overclaims.

  The `# devlaunch:` marks guarding profile edits used hand-picked tags, and
  nothing kept the tags distinct: two lines under one mark silently drop
  whichever is appended second, with every script still exiting 0. The tag is
  now derived from the content of the line it guards, so a collision is
  unrepresentable rather than asserted. A profile marked by an older devlaunch
  gains one duplicate `PATH` entry the first time it is seen — the cost the
  module already accepts for any mark change — and nothing after.

  `.devcontainer/claude-code/install.sh` now carries the same guards. It was
  excluded from the #164 sweep on the stated grounds that it guards its own
  line in its own file; that reason was wrong — `$TARGET_HOME/.profile` is the
  *user's* login profile, and the installer's guards substring-matched
  `.pixi/bin` and `pixi/envs/claude-shim`, the very strings that sweep stopped
  matching. Latent rather than live (the stock base-image profile contains
  neither string), and corrected along with the record. Its two edits are now
  devlaunch's own rendered fragments verbatim, marks included, so the installer
  (at image build) and the provision script (at `up`) recognise each other's
  work instead of each appending its own copy of the same line.

- **`dl <ws> rm` could destroy unsaved work without being asked for `--force`**
  ([#174](https://github.com/blooop/devlaunch/issues/174)). The guard read the
  clone directory off `local_path` while the delete fell back to the derived path
  whenever that one was not on disk, so a record pointing somewhere stale had the
  guard clearing an *absent* directory — nothing absent holds anything — and the
  delete then removing the derived one, which was the directory holding the work.
  Exit 0, nothing logged. `dl --ls --json` read the record the same way, so its
  `path` and `unsaved` could describe a third directory again.

  All three now resolve through one method, `WorkspaceCloneManager.resolve_clone_path`.

  A second face of the same field: `WorktreeInfo.from_dict` builds `local_path`
  with `Path(data["local_path"])`, and `Path("")` is `Path(".")` — truthy, and
  its `exists()` is True. An empty recorded path therefore passed both of the old
  tests and handed `shutil.rmtree` **dl's own working directory**, which it
  emptied, `.git` included, before failing on `os.rmdir(".")`. A recorded path is
  now honoured only when it is absolute.

## [0.0.25] - 2026-08-10

### Added

- `docs/rust-port-scope.md` — a scoping note reconciling three records that
  disagreed about how wayfinder should consume devlaunch: [#53](https://github.com/blooop/devlaunch/issues/53)
  decided **GO** on a Rust `devlaunch-core` crate for `wf` to link and rejected
  subprocess-on-PATH, this changelog then deferred the rewrite, and
  [blooop/wayfinder#80](https://github.com/blooop/wayfinder/issues/80) — with no
  crate to consume — chose subprocess-on-PATH and recorded the rewrite as out of
  scope, reinforced. Neither cited the other.

  It adds the measurements both arguments were missing: the 370 MB environment is
  275 MB of CPython, 118 MB of devpod and 84 KiB of devlaunch, so a Rust port
  lands near 120 MB and devpod is 118 MB of that — the floor is devpod, not the
  language. The port is 7,373 lines of source against 18,039 lines of tests, of
  which the 61 mock-free acceptance tests carry over.

  It also records the one prediction #53 made that has since been tested: that
  the crate wins because a breaking change fails `wf`'s build where subprocess
  drift is silent until runtime. `dl <workspace> up` shipping in `wf` 0.14.0
  before 0.0.24 carried it was exactly that, and neither repo's CI noticed. `wf`
  0.15.0 now holds the `dl` it finds to a version floor, which is the strongest
  thing a subprocess seam can do.

  No decision is taken there. The live question is narrowed to #53's own
  falsifier — whether `wf` renders per-ticket workspace state.

## [0.0.24] - 2026-08-10

### Added

- `dl <workspace> up` starts or creates a workspace **without attaching**. The
  warm half of a launch, for a caller that wants the container ready before a
  user arrives: wayfinder fires it in the background the moment a launch is
  staged, so the container builds while the human is still choosing a mode and
  typing steering text. Idempotent — a workspace already running is a no-op
  and says so.

### Changed

- **A workspace with no LFS content no longer forks git-lfs on every launch.**
  Preparing a workspace always asked `git lfs ls-files` whether there was
  anything to materialize, and git-lfs is a large binary whose startup dominates
  that answer. The question is now settled first from the clone itself.
  `git lfs ls-files` reports the union of HEAD's tree and the index, and
  `git ls-files --with-tree=HEAD` enumerates exactly that union — so if none of
  those paths holds a pointer, the probe has nothing to report and the fork is
  skipped.

  That check is cheaper, not free: it is a `git ls-files` fork plus reading the
  first few bytes of each listed path, so it is the same O(tracked files) shape
  as the probe it replaces, at a much smaller constant. Measured on the
  reference machine, median of 7–9 runs: ~34ms → ~4ms for this repo's own
  checkout (124 tracked files), ~119ms → ~18ms at 3000 files, ~1180ms → ~202ms
  at 50 000. A workspace that really is holding pointers still pays the probe
  and materializes exactly as before, and a clone whose paths cannot be
  enumerated at all falls open to probing rather than being written off.

  The union is load-bearing, not belt-and-braces. The index alone is a strictly
  smaller set than what git-lfs can name: a clone left with no `.git/index` —
  an interrupted clone or checkout, which is precisely what the retry path
  exists to recover from — makes `git ls-files` succeed with *empty* output, and
  reading that as "nothing tracked, therefore no pointers" would strand the
  workspace on stub files on every later launch.

  Deliberately a question about pointer content rather than about whether the
  repo declares `filter=lfs`: a repo can hold committed pointers while declaring
  nothing, and can be LFS-tracked through attributes git reads from outside the
  clone. Either would have been read as "no LFS here" — leaving that workspace on
  stub files on every launch, not just once.

- **A warm launch no longer builds the clone manager it never uses.** Every
  `dl owner/repo@branch -- cmd` read `config.toml`, loaded `metadata.json` twice
  under its flock, created `repos_dir` and ran the id-scheme migration — and
  then attached to the running workspace without touching any of it.
  Construction now happens in the two arms that need it: resolving the default
  branch for a bare `owner/repo`, and the cold clone. The cold path is
  unaffected and the pinned devpod argv sequences are unchanged.

  Measured on the reference machine, the removed work is **~0.29ms** (median of
  25 fresh processes, 4-worktree `metadata.json`, already migrated) — real, but
  small next to the two devpod round trips a launch spends. The reason to do it
  is that a warm attach now touches no shared cache state at all.

  The consequence worth knowing: on that one shape, the one-shot cache
  migration and the quarantine of an unreadable `metadata.json` no longer run.
  They run on the next command that does build the manager, which is any cold
  launch, any bare `owner/repo`, and every workspace-management command.

- **A launch is one `devpod status`, not a `devpod list` and then a status.**
  Every workspace command opened with `devpod list --output json` purely to ask
  whether devpod knew this workspace, then asked `devpod status` about the same
  workspace a moment later. One `status` answers both questions, so the listing
  is gone from the path: `dl <ws> -- cmd` on a running workspace is now three
  devpod spawns rather than four, and `dl owner/repo@branch -- cmd` — the shape
  wayfinder hands every agent launch — two rather than three. Measured on the
  reference machine, ~0.4–0.5s off every launch, warm or cold, and it no longer
  grows with the number of workspaces on the machine.

  The trade is that `devpod status` cannot distinguish "no such workspace" from
  "devpod failed to answer", where the listing raised
  `UnreadableWorkspaceList`. A launch made wrongly cold by that redoes
  idempotent git work and hands devpod a source it already knows; devpod's own
  error then names the real problem. A **bare name** gets a second opinion
  before being refused, because there the wrong answer is worse: `status`
  consults the provider while `list` only reads devpod's own records, so a
  workspace whose provider is broken or gone still lists and cannot be
  described — and that is exactly the workspace somebody is about to
  `dl <ws> rm`. Refusing on the status alone would be both a wrong diagnosis
  and a refusal of the command that fixes it, so the listing decides. It is
  read only on that path.

  `validate_workspace_spec` went with the listing — it existed to check a spec
  against a list nothing fetches any more, and leaving it invited someone to
  fetch one again.

- **A cold container is lent the host's own `claude` and `gh` instead of
  downloading its own.** Provisioning ran `curl | bash` for pixi and two
  `pixi global install`s inside every fresh container, and the `claude-shim`
  package then pulled a ~285MB binary from GCS — tens of seconds to minutes of
  network, per container, on the critical path of every cold launch. The host
  running `dl` almost always has both tools already, and the container is one
  pipe away on the same disk, so they are now streamed in as a tar over the
  `devpod ssh` channel dl already holds. The payload is **342MB**, measured in
  [#158](https://github.com/blooop/devlaunch/issues/158), with both lent binaries
  proved to run in a staging directory before anything was moved into place.

  The network install is still there and still correct — it runs when the host
  has nothing to lend (no official `claude` install, no resolvable `gh`) or
  when the lent binaries do not run in that container. A pixi trampoline on the
  host is resolved to the binary it names; copying the launcher alone would
  copy nothing that runs.

  **Nothing lands in the container until it has been proved to run there.** The
  tar is unpacked into a staging directory, both binaries are run once, and
  only then are they moved into place, symlinked and put on the login `PATH`;
  a trap removes the staging directory whichever way the script leaves. Doing
  it the other way round was worse than a failed transfer: the host's `claude`
  is dynamically linked, so a musl or older-glibc container fails that check
  routinely, and an earlier arrangement that unpacked straight into `$HOME`
  left the `PATH` edit and a broken `claude` symlink behind when it did. The
  network fallback then decided what to install with `command -v`, which a
  broken binary satisfies — so it installed nothing, reported success, and
  every later launch's probe agreed with it. The workspace was left with a
  `claude` that could never run.

  The probe that decides all this is captured, unlike the trips that may follow
  it: what it prints is the answer the caller branches on rather than progress
  anybody needs to watch.

- **A baked `claude` counts as provisioned only if it is the real one.** The
  probe used to ask `command -v claude` and believe the answer, which the
  `claude-shim` downloader satisfies — so an image carrying the shim (including
  any built from this repo's own `.devcontainer/claude-code/` feature) skipped
  the lend and paid the ~285MB GCS download on first use, the exact cost the
  lending exists to remove. It now answers one of three states: `provisioned`
  when `gh` is on the login `PATH` **and** `claude` resolves to a binary the
  official installer itself put in `~/.local/share/claude/versions/`;
  `lendable` when both answer but the claude is a shim or wrapper; `absent`
  otherwise.

  "The official install" is one definition, asked from both ends of the pipe.
  The container reports two facts only it can know — where its `claude`
  resolves to, and where that directory in its own home resolves to — and
  which state those mean is decided on the host, by the same code that decides
  what the host may lend. Two copies of that rule, one per language, is what a
  shared constant alone does not prevent: a downloader parked at
  `versions/latest/bin/claude` satisfies "somewhere under the versions
  directory" while failing "a binary the installer wrote", and a probe holding
  the looser of the two opinions trusts it.

  Both paths are compared fully resolved, which is what makes the upgrade
  terminate on an image whose `$HOME` is reached through a symlink: a
  `lendable` container is quietly upgraded — the host streams in its own binary
  and the transfer's `~/.local/bin` `PATH` prepend is what makes it win from
  then on — so the *next* launch probes `provisioned` and the tar is paid once
  rather than on every launch for the life of the workspace.

  That prepend only actually happens because the guard in front of it now asks
  the right question. **Every line devlaunch appends to a container's login
  profile is written under a `# devlaunch:` mark, and the "have I already done
  this?" guard is an exact match on that mark** — not, as before, a search for
  the directory being added. Searching for the directory made the answer a base
  image's to give, and it gave the wrong one: Ubuntu's stock `~/.profile`
  prepends `~/.local/bin` itself, near the top of the file, so on
  `mcr.microsoft.com/devcontainers/base:ubuntu-24.04` — the image this repo's
  own devcontainer builds on — the transfer read that block as its own work,
  skipped the prepend, and left the shim ahead of the binary it had just lent.
  The workspace never converged: it answered `lendable`, re-paid the whole
  transfer, and answered `lendable` again, every `devpod up`, forever. The
  convergence test now lets the profile decide `PATH` — sourced from a home
  seeded with that image's stock file plus the lines this repo's devcontainer
  appends — because a test that builds `PATH` itself cannot see the ordering
  the lend depends on. A profile some earlier devlaunch already edited gains
  one duplicate `PATH` entry the first time it is seen, and nothing after that.

  If the host has
  nothing to lend, or the lent binaries do not run there, the container is
  accepted as it stands rather than falling through to the network install —
  that install decides what to do with its own `command -v` guards, which a
  shim already satisfies, so the trip would install nothing.

  The probe never executes the candidate `claude`; on a shim, *any* invocation
  triggers the very download the probe exists to detect. It resolves the path
  instead. It also exits 0 in every state now — including in an image that
  never set `HOME` — which retires the red devpod
  `fatal ... Process exited with status 1` that the old probe's everyday cold
  answer painted on the terminal.

  Two things this deliberately does not do. It does not compare versions: a
  real `claude` already in the container is left alone however old it is, since
  keeping versions in sync would make this a package manager (the binary
  self-updates, and a workspace rebuild re-provisions). And it does not make
  the payload per-tool — an image with only `gh` still receives both.

- **Two `up`s of one workspace serialize on a per-workspace lock.** Background
  prewarming makes concurrent `up`s of a single workspace an everyday event
  rather than an edge case, and two `devpod up`s of one workspace is not a race
  devpod promises to survive. The loser waits; a loser that *had* to wait
  re-checks the state first, because the likeliest reason for the wait is that
  the winner just brought this very workspace up — so the launch attaches to
  the container the prewarm built instead of re-walking a whole container
  lifecycle to arrive where it already is. The re-check costs one status round
  trip and is paid only on contention. It is skipped for calls wanting a side
  effect a sibling cannot have had: an IDE to open, a recreate, a reset, or a
  `--devcontainer` variant — that last one especially, since skipping it would
  hand the user the default container while they asked for another and say
  nothing about it.

  A skipped `up` still checks the tools. `Running` says the sibling's `devpod
  up` returned, not that its install did: it can be interrupted between the two
  (the flock dies with the process), its `up` can fail after the container has
  started, and it can have run with `DEVLAUNCH_NO_TOOLS` set where this one did
  not. The check is a probe round trip against a workspace already up, and
  silent when there is nothing to do.

  A lock that cannot be taken does not fail the launch. The cache directory can
  be unwritable — a container writing as another uid is a documented occurrence
  in this very cache — and serialization guards a race that may not be
  happening, so an errno traceback in front of a `devpod up` that would have
  worked is the worse answer.

- `devpod context options` is cached on disk for an hour. It was re-read in
  front of every `up` to fetch two dotfiles settings that change only when
  somebody runs `devpod context set-options`. The TTL is not the only thing
  that expires it: these options are per *context* and this is one cache file,
  so a cache older than devpod's own config file is stale whatever its age —
  otherwise `devpod context use <other>` would feed the previous context's
  settings to `devpod up` for an hour, a wrong answer nobody could connect to a
  cache they did not know existed.

- **The hourly freshness fetch is now the background updater's job.** `dl
  --update-cache` — the detached child dl already spawns to refresh completions
  after a command — now also sweeps the bare-clone cache, fetching every repo
  whose fetch interval has elapsed. It takes each repo lock non-blockingly, so a
  repo some launch is mid-clone in is skipped and picked up next time: the sweep
  never queues behind a launch. It is worth being exact about the other
  direction, since it is the one that can cost somebody time — a launch *can*
  still queue behind the sweep, because the lock is held for the length of the
  fetch, and the wait is reported only as "waiting for another dl run" even
  though the holder is a detached child nothing on screen accounts for. The
  background fetch is therefore capped at five minutes, so that wait has a
  ceiling that is dl's rather than the network's. A failed or timed-out fetch is
  stepped over rather than reported, since the interval brings it round again
  and a detached child has no terminal to report to.

  Nothing about freshness changes yet — the launch path still runs the same
  interval fetch when it draws the short straw, and both sides read the same
  `last_fetched` clock, so whichever gets there first spares the other. What
  changes is that on most machines the background child gets there first, and
  that launch never pays. Taking the fetch off the launch path for good is the
  next step.

### Fixed

- **`dl <workspace> rm` could delete a clone that held unsaved work, and say
  nothing** ([#171](https://github.com/blooop/devlaunch/issues/171)). The guard
  ran `git` in the clone directory with nothing pinning it there — no
  `--git-dir`, no `--work-tree`, no ceiling — so git's repository discovery
  walked up the parent chain. A clone whose `.git` was unusable (half-removed by
  an interrupted delete, truncated, never finished) did not make git refuse: it
  made git find an **ancestor** repository and answer about that one. With
  `dl`'s cache under `$XDG_CACHE_HOME` and a dotfiles repository in `$HOME`,
  that ancestor is ordinary — and when it was clean and fully pushed, the guard
  reported "nothing would be lost" about somebody else's repository and the
  clone went, untracked scratch files and all. Only a *tidy* host could hit it:
  a dirty ancestor made the guard fire for the wrong reason and hid the bug.

  Git is now asked about one directory and cannot leave it, and "could not tell"
  is an answer of its own that refuses the delete exactly as "would lose" does
  — previously both were `None` and `None` meant delete freely. A directory that
  is *there* but is not a repository git can read is now a refusal rather than a
  clean bill of health; a directory that is *not* there still holds nothing, so
  clearing up after a half-finished delete needs no `--force`. `--force` still
  overrides, in both cases.

  **Breaking, in `dl --ls --json`:** `unsaved` was a string or `null` and is now
  an object with exactly one key — `{"nothingToLose": true}`,
  `{"wouldLose": "<what>"}` or `{"couldNotTell": "<why>"}` — the shape `disk`
  already uses. It is `null` exactly where `devlaunch` is `false`: a workspace
  `dl` did not create. The break is the safe way round: a reader that tested the
  old field for truthiness now sees a truthy object for every arm, so it leaves
  workspaces alone rather than deleting them.

  `unsaved`, `checkedOut` and `path` are answered for every workspace `dl` owns,
  not only for the ones it still has a metadata record for. They used to gate on
  the record while `devlaunch` and `disk` gated on the clone directory, so a
  clone under the cache whose record had gone reported `devlaunch: true` with a
  measured `disk` and `unsaved: null` beside them — `null` documented as "not
  `dl`'s clone", on a clone `dl` had just called its own. That is the same
  sentinel this entry is about, one layer out, and the same divergence
  [PR #165](https://github.com/blooop/devlaunch/pull/165) closed for `disk`.

  A clone dl cannot even look at — a parent directory it has no search
  permission on — is a "could not tell" too, and `Path.is_dir()` had no way to
  say so. It gave a different wrong answer on each supported Python: up to and
  including 3.13 it re-raised `PermissionError`, so `dl <ws> rm` failed closed
  by crashing and `dl --ls --json` became a traceback for the whole listing
  because of one workspace; on 3.14 it returns `False`, which read as "not
  there, so nothing to lose" — a clone that may be full of work, reported as
  free to delete. The errno is now read directly: ENOENT and ENOTDIR mean there
  is no clone there, and everything else means dl was not allowed to find out.
  A path with a NUL byte in it — a record a hand-edited `metadata.json` can
  produce — is a "could not tell" as well rather than a `ValueError` out of the
  listing.

  The boundary above was executed on 3.10.20, 3.11.15, 3.12.13, 3.13.14 and
  3.14.6, and the `ci` matrix now runs every one of those minor versions; it
  previously stopped at 3.13, so `pixi run ci` never ran on the newest Python
  this project supports. This entry said "3.13+" for two rounds of review, and
  no test would have said otherwise — what they assert is the same on every
  version, so they are green either side of wherever the prose puts the line.
  Somebody running it is what corrected it.

- A cold `devpod up` no longer prints a red `fatal ... Process exited with
  status 1` from the tools probe. The probe asks a yes/no question and reports
  nothing; "no" is its everyday answer on a fresh workspace, and devpod
  rendered that as an error describing the probe working. It is captured now.

- `.dockerignore` excludes `.pixi` at any depth, not just at the repository
  root. A git worktree under `.claude/worktrees/` has an environment of its
  own, and one left behind by an earlier effort put the very symlink the file
  was written to exclude back into the build context — so the e2e suite failed
  to build a container with the exact error the comment above the pattern
  quotes.

## [0.0.23] - 2026-08-08

### Fixed

- `dl --purge` no longer abandons the whole cache when one directory refuses to
  be removed ([#131](https://github.com/blooop/devlaunch/issues/131)). A
  container writes into its bind-mounted clone as its own user — uid 1000 in the
  standard devcontainer base image — so where the host user is not also uid 1000
  (CI, a shared machine, a container running as root, devlaunch developed inside
  its own devcontainer) those directories cannot be emptied by the host. The
  purge used `shutil.rmtree`, which stops at the first failure, so a single
  unremovable clone left the completion caches, `metadata.json` and every other
  clone standing, and reported an errno.

  It now removes everything it is permitted to and names the paths that refused,
  with the command that finishes the job. Exit status is still `1` — a clone the
  user was told would go is still on disk — but the report distinguishes
  "removed most of it" from "removed none of it", which an exit code cannot.

  Only paths that actually obstructed are listed, and the obstruction is not
  the path that raised. Unlinking needs write permission on the *directory*,
  not on the file, so a clone owned by the container's user refuses every one
  of its children separately — on a real e2e workspace that was forty-odd
  `.git/objects` entries, hooks and a README, none an ancestor of another and
  all of them the same single fact. A failure is attributed upward to the
  outermost directory that cannot be written into, which is the directory the
  original errno named, so that clone is now one line.

  Found by the `e2e` job on the first attempt at this fix, which no unit test
  could have caught: a directory owned by *another user* is not something a
  test process can build.

  A symlinked cache root is refused rather than followed, naming what it points
  at. `os.walk`'s `followlinks=False` governs subdirectories only — the top is
  always scanned — so a hand-rolled walk descends a symlinked
  `~/.cache/devlaunch`, empties whatever it points at, and reports a clean
  sweep. `shutil.rmtree` refuses that outright, and losing the refusal turned it
  into a silent recursive delete outside the named directory.

  Unlinking just the link was tried first and is also wrong: the clones are
  still on the other volume and the purge says `Removed`. A cache root is a
  symlink because somebody moved their cache, so following it and unlinking it
  cost them the same thing by opposite routes — one deletes the workspaces, the
  other reports them gone. Refusing is the only one of the three that is not a
  lie, and `sudo rm -rf <cache>` would remove the link and nothing else, so the
  reason carries the real location. Both found in review; there had been no
  symlink coverage at all.

  Each refusal now carries what the system actually said, and the advice is
  offered rather than asserted. The old report claimed "Written by a container
  running as a different user" unconditionally without ever looking at the
  errno — false for a read-only mount, `chattr +i` or a busy mountpoint, none of
  which `sudo rm -rf` fixes either. That path is also `shlex.quote`d now: it is
  handed to a person to paste into `sudo rm -rf`, and `$XDG_CACHE_HOME` with a
  space in it made that two targets, the first of them wrong.

  "Cannot look at it" is no longer read as "it is gone". A cache whose parent
  directory could not be traversed came out as `No data to purge.` and exit 0
  with the cache fully intact. `Path.exists()` is what could not tell the two
  apart, and it is not consistent about how it fails to: it returns False on
  Python 3.14 and raises `PermissionError` on 3.13, so the old check answered
  wrongly on one version and crashed on the next. Presence and symlink-ness now
  come from a single `os.lstat`, where the three outcomes are distinguishable.

  Two *separately* unwritable directories on one path are reported as two lines.
  Clearing the inner one leaves the outer one just as stuck, so each is work
  somebody has to do, and the earlier "ancestors are never listed" wording
  described neither the code nor what is useful.

  What a purge reports is decided from the disk once the walk is over, rather
  than from what raised during it. Randomised trees found why that matters:
  `os.walk` cannot scan an unlistable directory and says so, but if that
  directory is empty the `rmdir` afterwards succeeds — so reporting at the point
  of raising named a path that is not there, and through the ancestor rule could
  have silenced a genuine refusal above it. Deciding afterwards makes both
  invariants — nothing survives unsaid, nothing is said that is not there — hold
  by construction.

## [0.0.22] - 2026-08-08

### Changed

- `unsaved` now names the first few changed paths, not just a count:
  `1 uncommitted change(s) (pixi.lock)`. Found by using 0.0.21 for real — this
  repo's own devcontainer runs `pixi install` in its `postCreateCommand`, which
  leaves the tracked lockfile modified in **every** workspace it builds. As a
  bare count that is indistinguishable from an hour of someone's unsaved work,
  so a cleanup tool reading it correctly refuses to clean anything, forever.
  Named, the same fact is judgeable — by a person, and by a caller deciding
  whether to insist.

### Fixed

- The first named path lost its first character (`ixi.lock` for `pixi.lock`).
  `git status --porcelain` writes a modified *tracked* file as `` M path`` —
  leading space — and this module stripped git's output at both ends, eating
  the status column of the first line only. Untracked entries start `??` and
  were unharmed, which is why the tests written alongside the feature all
  passed. Only trailing newlines are trimmed now.

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
