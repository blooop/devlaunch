# Measuring a launch

Where a launch's seconds go, how to get that as JSON, and how the trend on
`main` is built.

## One round trip per question

Every `devpod` call costs about 0.45s, far more than `dl` itself spends on
anything. That figure was measured on the Python build on 2026-08-07
(`22afd52`) and no host has re-measured it into this prose since, but it is
still the right order: `devpod status` on `dl 0.13.0`, nested inside this
repo's devcontainer, measures 0.43s to 0.67s
([#388](https://github.com/blooop/devlaunch/issues/388)), and the trend's own
Rust `warm / devpod-up` points, 47 of them on GitHub runners, run 0.23s to
0.49s with all but one under 0.45s. So read it as an order rather than as a
stopwatch reading, and not as a floor: the fastest of those points halves it.
That one figure sets the shape of the whole program: a command reads the
workspace list at most once, and everything a container needs on the way in,
naming it and then the tools probe, rides a single setup pass. So an
interactive `dl <ws>` and a one-shot
`dl <ws> -- <cmd>` cost the same trips.

## The listing's questions are asked together

`dl --ls --json` is the one command whose cost grows with the machine, and the
`--json` is load-bearing: the human table `dl --ls` prints has no state column, so
it costs the single `devpod list` and nothing per row. The document is the surface
that carries a `state` for every workspace, and a state is a question `devpod list`
does not answer, so the document asks `devpod status` once per workspace,
including about the ones devlaunch did not make. That is one round trip per row
and there is no way around it: a document of forty workspaces asks forty
questions. `the_table_asks_devpod_for_the_list_and_nothing_else` holds the two
apart.

What it no longer does is wait for each one before asking the next. The questions
are independent, so they go out in batches of eight and the waiting overlaps:
forty workspaces cost five batches rather than forty trips end to end. The trips
themselves are unchanged in number, which is why
`the_listing_costs_one_list_and_one_status_per_workspace` still reads the same;
what changed is only how much of the waiting happens at once.

**A batch costs the slowest trip in it, not the average.** The eight are started
together and all eight are waited for before the next eight begin, so this is a
barrier rather than a pool of eight permits: one slow answer leaves seven threads
idle until it lands. It is never worse than asking serially, which is what it
replaced, but a work queue that started the ninth trip the moment any of the first
eight returned would be better on a machine where one workspace is much slower to
answer than the rest. Worth knowing before reading a slow `--ls --json` as
something else.

**A trip also costs more when eight are in flight, so the win is not the width.**
Measured on 2026-09-01, on one host with ten workspaces on the local docker
provider and devpod 0.26.1: asked serially the ten trips averaged 0.465s each and
the `devpod-up` stage took 4.656s. Batched, the same ten averaged 0.641s each,
about 38% more, and the stage took 1.724s. The command went from 5.132s to 1.395s.
So read the win as roughly 3.7x rather than the 8x the width suggests, and read
"forty workspaces cost five batches" as five batches whose trips are each slower
than a lone one would be.

That run shows the barrier's cost rather than describing it. The first chunk's
eight trips landed between 0.593s and 0.656s, and the second chunk, holding the
two rows left over, took 0.443s and 1.066s. The slower of those two is 62% of the
whole stage, and the work queue above would have spent the faster one's thread on
something instead of idling it.

Eight is chosen for the shape of the wait rather than for the core count. A trip
is one process blocking on devpod's own work rather than arithmetic, so the useful
width is set by how many of those the machine will schedule. It is bounded rather
than unlimited because a row costs a devpod process, and the cost of starting
sixty at once is a real one. What the figures above settle is that the contention
is real rather than hypothetical and that eight still pays; what they do not
settle is where the curve turns, since nothing has been measured at four or at
sixteen, and eight is a conservative pick rather than a tuned one.

One thing the change does move: the `devpod-up` **stage** seconds a timing
document reports for `dl --ls --json` used to be the sum of the per-row status
times and are now the wall time of the batch loop, which is smaller. The span
count, and every individual span, are unchanged. So the stage now reports less
than the spans inside it add up to, which is the one place this page's arithmetic
stops being addition.

This is also why the document is something somebody asks for rather than something
on the launch path. A launch asks about one workspace, and pays one trip for it.

## One connection per workspace

The trip that carries `dl <ws> -- <cmd>` at a terminal is OpenSSH, over the host
alias `devpod up` publishes, and that connection is now reused. Three options go
on an argv `dl` already built: `ControlMaster=auto`, a derived `ControlPath`, and
`ControlPersist=60`. The first trip into a live workspace opens a master, every
trip for the next minute joins it, and nothing pre-warms anything, so `dl` starts
no process it did not start before.

What it saves, measured on one loaded host (load average 21 for the whole run, so
the absolute seconds sit two to three times above a quiet machine and only the
ratio travels): a fresh `ssh -t` cost 2590ms to 3140ms and a reused one 16ms to
28ms. About 100x, and it is worth being plain about where it lands. A first launch
into a cold workspace moves by nothing. A repeat command into a workspace that is
already up saves most of two seconds. And the case it helps most is not a stopwatch
reading at all: trips that are not multiplexed serialize on a per workspace lock,
so eight commands fired at one workspace at once used to finish over a 9.9s to
23.9s staircase, where eight over one master all finished at 8.02s. A fleet of
agents attaching to one workspace is this repository's own daily shape.

The socket path is derived rather than configured, and what goes into the digest
is the point of the whole thing. A master filters `SendEnv` against **its own**
permit list, in silence, at exit 0, so a master opened by a run with no GitHub
token would hand the next run an empty `GH_TOKEN` and an unauthenticated `gh` with
nothing anywhere to say so. The digest therefore covers the host alias, the
`SendEnv` permit list and `$SSH_AUTH_SOCK`: a run whose permit list differs from
the master's cannot find that master, so the mismatch has nowhere to happen. The
sockets live under `ssh-control` in `dl`'s own cache directory, in a directory
this user alone can read, and `dl --purge` takes them with everything else. Losing
one costs the next trip its couple of seconds and nothing more.

It fails closed in both directions. A socket path too long for a unix socket, or a
directory `dl` cannot create, means the session runs unmultiplexed rather than
failing. A master that has gone away leaves a socket the next client unlinks, and
`ControlPersist` is a minute rather than an hour because a live master holds a
resident `devpod ssh --stdio` process and a `docker exec` per key, and `dl` must
not be the reason a container never goes idle.

## While the launch is still running

`DEVLAUNCH_TIMING` answers after the fact, which is exactly when you no longer
need it. What a cold launch needed was something during, because a cold launch
spends minutes on one step and says nothing while it does. Measured on one host,
streaming devpod's output, two consecutive lines **5m01s apart**:

```
11:42:52  Pixi task (ui-install-locked): npm --prefix frontend ci
11:47:53  added 585 packages in 5m
```

Nothing in between said the launch was alive, which step it was on, or how long
that step had been running. The same launch warm was 21.6s in total with that
step taking 5s, so the ratio between a warm and a cold launch of one branch is
about 16x and a reader had no way to tell from the terminal which one they were
in.

`dl` now times the gaps between devpod's own lines and says so every 30 seconds
while one lasts:

```
11:42:52  Pixi task (ui-install-locked): npm --prefix frontend ci
Still working: devpod has printed nothing for 30s.
Still working: devpod has printed nothing for 1m00s.
...
11:47:53  added 585 packages in 5m
```

Three things it does not claim. **It is not a diagnosis**: a step that is slow
and a step that is stuck both produce it, and the only thing separating them is
the number going up. **It is not devlaunch's time**: the five minutes above are
the repository's own `postCreateCommand`, `dl` cannot make `npm ci` faster and
does not try. And **it is not the age of the launch**: the measurement restarts
at every line devpod writes, so it is the age of the step devpod is on, and its
restarting is what tells a long step from a wedged one.

A launch that is talking never produces one, because devpod's own steps are
seconds apart. That is what keeps the line off a warm launch entirely and off
most of a cold one.

**What it costs.** `devpod up`'s stderr is a pipe now rather than the terminal,
so it is no longer a terminal to devpod and loses whatever devpod does with one.
stdin and stdout are untouched, and so is the process group the build leads: a
Ctrl-C still takes the build down with `dl` rather than orphaning it holding the
launch lock.

## Measuring launch time

Set `DEVLAUNCH_TIMING=1` and a `dl` command ends with one summary on stderr,
naming each subprocess round trip and the total. Unset (or `0`) records nothing
and prints nothing.

**Every second quoted in this section came off one host on 2026-08-14, over
three sessions that day (`d3b8ffb`, `5510b1e`, `461f740`), and that host was
running the Python build `0.1.0` replaced.** The `dl-next` in these captures is
that Python install, not the compiled Rust working tree the name means now. The
formats are current: the stage names are frozen in
`rust/devlaunch-core/src/timing.rs` and pinned by its tests. The seconds are
not. They describe an implementation that no longer ships, and no host has
measured the Rust binary into this prose, which is deliberate rather than a gap
to fill by hand. The Rust build's own per-stage decomposition was re-measured
nested inside this repo's devcontainer
([#388](https://github.com/blooop/devlaunch/issues/388)), which is
docker-in-docker rather than a host, so what it carries across is the per-stage
arithmetic and not anyone's wall clock.

Current per-commit numbers come from [the trend on main](#the-trend-on-main),
which has published a Rust point on nearly every push since 2026-08-22, two
lost runs aside. Read it forward from `03b7de2`, the first of them, and not
across it: every point before that measures the Python build, and nothing on
the chart says which is which.

Captured from a real warm launch (the launch's own output elided):

```bash
$ DEVLAUNCH_TIMING=1 dl-next blooop/mcp-devtasks -- true
...
dl-timing: devpod status 0.454s
dl-timing: gh auth token 0.036s
dl-timing: devpod ssh 1.952s
dl-timing: total 2.444s (in-process, excluding interpreter startup)
```

### The same launch, machine-readable

`DEVLAUNCH_TIMING=json` swaps that prose for one document on a single
`dl-timing-json:` line, so a trend job can read a launch without scraping
prose. It decomposes the launch into five **ownership-boundary stages**, one
per party that could actually make it faster, with the round trips nested
inside the stage that paid for them:

| stage | what it owns |
|---|---|
| `handoff` | the gap between the keystroke that resolved to this exec and dl starting (see the stamps below) |
| `host-prep` | the host's own git work: the bare clone and its fetches, the lock waits, the LFS probe and, for an LFS repo, the cache's LFS fetch and the workspace's materialization out of it. Plus the `gh auth token` trip, wherever on the launch it falls |
| `devpod-up` | the arm that gets a container running: the existence probe and, when it is not running, the `up` itself. On a warm launch that arm is the probe alone |
| `tools` | the probe trip and the conditional lend, including staging the payload tar |
| `attach` | the last trip, into the running command |

Two rules are worth knowing before reading one:

- **A stage that never ran is absent, not zero.** A warm launch reports no
  `host-prep` at all, because it did none. A stage that failed is present,
  timed up to the failure, and marked `failed`.
- **A stage totals over its whole arm**, not just over its round trips, so the
  host-side work between two spawns is attributed rather than lost. Stages
  never double-count each other: `tools` runs inside the launch `devpod-up`
  brackets, and those seconds are charged to `tools` alone. The in-process
  stages therefore add up to the total. Measured on a real cold launch below,
  they came to 20.834s against a 20.834s total.

`handoff` is the exception to that sum, and the only one: it ends where
`total` begins, so it is time the process could not have measured from inside
itself. A consumer adding stages up against the total leaves it out.

Two optional environment variables let whatever launches `dl`, a shell
function or an agent front-end, close the loop on the time before dl existed.
Both are Unix epoch seconds, which is what `date +%s.%N` prints:

| variable | meaning |
|---|---|
| `DEVLAUNCH_HANDOFF_T0` | the keystroke that resolved to this exec. Becomes the `handoff` stage, the only measurement of exec plus interpreter startup there is, since `total` begins after both |
| `DEVLAUNCH_PREWARM_FIRED_AT` | when a prewarm (`dl <ws> up`) was fired for this workspace, if one was |

With the prewarm stamp set, the document also reports what that prewarm was
worth: the head start it bought, and which shape the launch then took. `hit`
means the workspace was already up, `partial` means this launch queued behind a
prewarm still running, `miss` means this launch ran the `up` itself. dl decides
that, not the firer: a prewarm is fired and forgotten, so only the launch that
followed can see whether it helped. **A stamp that is missing, unreadable, or
ahead of this clock reports nothing** rather than a zero. An absent handoff
and an instantaneous one are different facts, and a trend cannot tell them
apart once one is written as the other.

Captured from a real warm launch with both stamps set (one line, wrapped and
elided here for reading):

```bash
$ DEVLAUNCH_TIMING=json DEVLAUNCH_HANDOFF_T0=$(date +%s.%N) \
    DEVLAUNCH_PREWARM_FIRED_AT=... dl-next blooop/mcp-devtasks -- true
...
dl-timing-json: {"total": 2.210768, "total_epoch": "in-process, excluding interpreter startup",
  "stages": [{"stage": "handoff",   "seconds": 0.130542, "outcome": "ok", "spans": []},
             {"stage": "host-prep", "seconds": 0.027799, "outcome": "ok",
              "spans": [{"label": "gh auth token", "seconds": 0.02747}]},
             {"stage": "devpod-up", "seconds": 0.455188, "outcome": "ok",
              "spans": [{"label": "devpod status", "seconds": 0.455158}]},
             {"stage": "attach",    "seconds": 1.726961, "outcome": "ok",
              "spans": [{"label": "devpod ssh", "seconds": 1.72661}]}],
  "prewarm": {"head_start_seconds": 42.489243, "shape": "hit"}}
```

Stages appear in the order the launch first entered them, and only the ones it
reached appear at all. This warm launch built nothing and lent nothing, so
there is no `tools` stage and no `devpod up` inside `devpod-up`. That
`handoff: 0.131s` is the exec and the interpreter start, which nothing else
measures.

The cold launch of the same repo, same host and session, decomposed as:
`host-prep` 2.257s (`git clone --bare` 1.602 + `git fetch` 0.427 + workspace
`git clone` 0.065 + LFS probe 0.002 + token 0.034), `devpod-up` 5.848s (`devpod
up` 5.113 of it), `tools` 10.224s (probe trip 1.584 + `tools tar` 0.111 +
transfer 8.445), `attach` 2.505s. That is 20.834s of stages against a 20.834s total.

For before/after numbers, `scripts/bench_launch.py` runs a command N times and
reports the median, one command per side of a change:

```bash
python3 scripts/bench_launch.py -n 5 -- dl-next owner/repo -- true   # warm launch
```

(`pixi run bench -n 5 -- ...` in the devcontainer.) It reports no median if any
run fails, so a broken launch cannot pass as a fast one. See `bench_launch.py
--help` for `--before`, the per-run reset that makes a *cold* median cold and
whose `rm --force` also succeeds on the first run when there is nothing to
remove yet, and for why its wall clock and `dl-timing: total` are not the
same quantity. For scale, on the host the session above was captured on, the
warm median over 5 runs was 2.176s. Running the cold recipe exactly as the
epilog writes it, `-n 5` with the container recreated per run, gave a median of
15.899s (runs: 15.9, 20.0, 15.2, 15.7, 17.8). Read that as the cost of
recreating a container, not of a first-ever launch: the reset removes the
workspace but leaves the docker image layers and the bare clone cache, so
every run after the first starts from both. A machine that must also pull or
build the image pays more, by an amount this recipe does not measure, and the
gap is large: an earlier 3-run median on this same host, reported as its first
real launch, was 33.204s.

Every number in the two paragraphs above was copied into this prose by hand,
which is how they came to outlive the build that produced them. `--record` is
how that stops: it writes the same invocation as one JSON object
a trend job can upload without anyone reading it.

```bash
python3 scripts/bench_launch.py -n 5 --record warm.json --shape warm \
    -- dl-next owner/repo -- true
```

The record holds the command, the run count, each run's wall time and
per-stage seconds, and the medians of those, and nothing else, because a CI
job stamps its own commit, clock and host better than this script can. Four
things about it are load-bearing:

- **The median is the point, the runs are its evidence.** A trend compares a
  point against the immediately previous one, so N runs published as N points
  would read ordinary spread as a regression.
- **A stage no run reported is absent, not zero.** A warm launch legitimately
  has no cold-path stages; a zero would claim the work happened instantly
  rather than not at all. A stage only *some* runs reported is a median over
  those runs, carrying a count of how many. A median of two and a median of
  five are not the same claim.
- **Recording asks the launch for its stages** (`DEVLAUNCH_TIMING=json`), so a
  run that reports no timing document is an error and no record. Same
  discipline as no median over a failed run.
- **`--shape` labels the trend line** (`warm`, `cold-recreate`). It is the
  caller's to say: the same command benches either shape depending on
  `--before`, and a wrong label is worse in a trend than a missing one.

### The trend on main

Every push to `main` runs `.github/workflows/bench.yml`, which benches both
shapes on the runner and publishes one point per stage to
<https://blooop.github.io/devlaunch/dev/bench/>. It can also be dispatched by
hand.

**The port is in that series and nothing on the chart marks it**, which is the
one thing worth knowing before reading it. The first 35 points end at `1566c86`
and measure the Python build, because until `1654daf` these steps ran the
console script an editable Python install left in the environment. Publishing
then stopped for two days, the retirement having put a bare `dl` on `PATH` with
no `devpod` beside it, until
[#373](https://github.com/blooop/devlaunch/pull/373) fixed it. The Rust series
starts at `03b7de2` on 2026-08-22 and has a point for nearly every push since.
Two are missing, `5573f77` and `7be9986`: the workflow serialises on one
concurrency group, GitHub keeps only one run queued behind the one it is
running, and five merges landed inside four minutes that day, so the runs in
the middle were cancelled before they benched anything. A missing point is
therefore a cancelled run rather than a skipped commit, and the comparison
across one spans two commits' worth of change. A comparison within either run
of points is otherwise sound, and one that straddles the port is not: it reads
a whole change of implementation as a regression.
[#292](https://github.com/blooop/devlaunch/issues/292) is open on that, and on
its record-keeping instruction rather than on a red workflow: the commit of the
first Rust point has to be written into the map's decisions so nobody later
reads that step as something `dl` did. Past that, reading the chart needs
nothing but the chart; what follows is for changing it.

`scripts/bench_points.py` is the step between the two formats, bench records
in and one flat array of trend cases out:

```bash
python3 scripts/bench_points.py warm.json cold-recreate.json --out bench.json \
    --require-stages-on cold-recreate
```

(`pixi run bench-points ...` in the devcontainer.) One case per stage the shape
reported, plus that shape's own total: `warm / host-prep`,
`cold-recreate / devpod-up`, `warm / total`. Six properties of it are
load-bearing:

- **The published value is the median, and the case name is a key.** The trend
  compares a point against the immediately previous point and nothing older, so
  the de-noising has to have happened before publishing. Renaming a stage
  starts a new, empty series beside a frozen old one.
- **The spread rides along as the point's error bar**, and the outside
  stopwatch (`wall=`) as part of its `extra`. Evidence beside the number,
  rather than a second trend line for the same launch that disagrees with the
  first by a constant.
- **An absent stage is absent.** A warm launch lends nothing, so there is no
  `warm / tools` case at all. Never a zero, which would claim an instantaneous
  lend and would drag the line down exactly where a regression should show.
- **`--require-stages-on` fails the job rather than the trend.** The way this
  decomposition is expected to break is an absence, not a wrong number: a stage
  stops being emitted and the total keeps working. So the cold-recreate shape,
  where every stage is known to be present, asserts them all, and a run that
  lost one publishes nothing and goes red. Naming a shape that was not benched
  fails too, since an assertion that covers nothing reads exactly like one that
  passed.
- **A record that is missing or unreadable refuses the same way.** The step
  that writes the records can exit 0 without having written one, so the absence
  arrives here. It prints `bench_points: <reason>` naming the file and
  writes nothing, like every other refusal, rather than a traceback.
- **A regression alerts; it never gates.** The workflow is deliberately not a
  job in `ci.yml`: a job there would join the CI gate's `needs` by house
  convention and turn a noisy wall-clock measurement into a merge gate. A point
  above the threshold leaves a commit comment and a red mark on the chart, and
  the build stays green.
