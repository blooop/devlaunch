# Can `devpod up`'s output be decomposed into image / create / postCreate?

Evidence for [devlaunch#195](https://github.com/blooop/devlaunch/issues/195), a
research ticket on benchmarking map [#193](https://github.com/blooop/devlaunch/issues/193).

**Answer: yes, mechanically — `--log-output json` gives one NDJSON object per
log line with nanosecond timestamps, and the stage boundaries are recoverable
to ~5ms. But the phase identity exists only as English prose inside a free-text
`message` field. There is no phase, stage, or event key, and no schema version.**

Measured on devpod **v0.26.1**, docker provider, Docker 29.7.2.

## What the json mode gives you

Three keys, on **stdout** (not stderr):

```json
{"time":"2026-08-14T19:00:34.815+01:00","message":"image not found, pulling image: image=alpine:3.19","level":"info"}
```

`plain` stamps whole seconds only (useless below ~1s); `raw` has no timestamps
at all. **json is the only mode that can carry a duration.**

## The boundary markers (all substring matches on prose)

| Stage | Starts at | Ends at |
| --- | --- | --- |
| image-acquire | `inspecting image:` / `image not found, pulling image` / `build with docker buildx build` | `running docker command: command=docker, args=run ` |
| container-create | `running docker command: ... args=run ` | `setting up container` |
| lifecycle hook | `running <name>Commands lifecycle hook` | the **next** `ran command: command=` |

## Verification

`parse_devpod_up.py` is the sketch. It was run against every capture in
`captures/`, covering five workspace shapes. Against known `sleep` durations in
the devcontainer's hooks it is accurate to ~5ms:

| Shape | image-acquire | container-create | postCreate | unattributed |
| --- | --- | --- | --- | --- |
| cached image (`json.out`) | 0.052 | 5.576 | 4.004 (sleep 4) | 1.334 |
| pulled image (`pull.out`) | 5.529 (pull) | 1.217 | 3.003 (sleep 3) | 2.443 |
| buildx build (`build.out`) | 9.661 (build) | 1.086 | 2.004 (sleep 2) | 0.425 |
| warm no-op (`warm.out`) | — | — | — | 2.280 |
| four hooks (`hooks.out`) | 0.057 | 0.826 | 2.001 (sleep 2) | 1.337 |

The warm shape emits four lines and no image/create/postCreate markers at all —
correctly, since none of that work happened.

## Traps this sketch had to survive

1. **`ran command:` is logged after *every* lifecycle hook**, not just
   postCreate. A devcontainer with an `onCreateCommand` — perfectly ordinary —
   makes a first-match parser pair postCreate's start with onCreate's end and
   report a **negative duration**. Observed: `-1.003s`. Pairing must be
   sequential. See `captures/hooks.out`.
2. **`message` is `omitempty`.** Blank lines arrive as
   `{"time":...,"level":"info"}` with no `message` key; 7 such lines appear in
   `captures/build.out`. `obj["message"]` raises `KeyError`.
3. **`inspecting image:` is logged *before* `image not found, pulling image`**,
   so a first-match classifier calls a real pull a cache hit. The duration is
   still right; only the label is wrong.
4. **Streams are mixed.** Almost everything is on stdout, but under `--debug`
   one line and every `level:"fatal"` line go to **stderr**. A parser that
   reads only stdout sees a truncated log with a dangling hook start and no
   error at all.
5. **Substring markers collide under `--debug`.** The source also carries
   `"begin setting up container"` and `"done setting up container"` at debug
   level, so `"setting up container" in msg` matches three lines in a
   `--debug` capture and one otherwise. Markers whose message is fixed text
   are therefore compared with `==`, not `in`; only markers with an
   interpolated tail are matched as prefixes.

## Failure semantics

**`devpod up` exits `1` for every phase failure.** A `postCreateCommand` that
exits 7 still yields exit 1 (`captures/fail.*`); an unresolvable image also
yields exit 1 (`captures/badimg.*`). The exit code identifies nothing. The
phase appears only as prose in a `level:"fatal"` line on stderr:

```
lifecycle hooks pre-attach: failed to run: sh -c '...', error: exit status 7
```

## Nothing useful is recorded on disk

`DEVPOD_HOME/contexts/<ctx>/workspaces/<id>/` holds `workspace.json` and
`workspace_result.json` after an up. Neither carries a duration. The only
timestamps are `creationTimestamp`/`lastUsed` (**whole seconds**, UTC) and
`ContainerDetails.Created`/`State.StartedAt` — which are Docker's, not
devpod's, and give one boundary, not three. **A post-hoc read cannot replace
live parsing.** `devpod logs <ws>` prints the container daemon's log, not phase
timings.

## Coverage

The json log window covers ~97% of the process: on a measured pull run, process
start to first log line was 30ms and last log line to exit was 349ms, against a
12.570s total. The residual plus the in-window `unattributed` bucket (0.4–2.8s,
mostly config resolve, agent injection and the daemon wait) is real time that
belongs to no named stage and must be reported, not silently dropped.

## Turning it on costs no code

`--log-output json` has an env equivalent, `DEVPOD_LOG_OUTPUT=json`, and it
works with no flag on the command line (`captures/envmode.out`). Since
`run_devpod` runs `up` with **inherited** stdout, devpod's json goes straight
through `dl` to whatever captures it. So a bench harness can set one env var
and parse `dl`'s own stdout without any change to `dl`'s argv construction, and
without json ever reaching an interactive user. (The env-var and
stdout-inheritance facts were each verified; the full `dl`-level capture was
not run end to end.)

The reason not to make json the default for everyday launches is the same
inheritance: today a human watching `dl` sees devpod's readable progress, and
json mode would replace it with NDJSON.

## Which devpod this actually is, and why the output is not a contract

The pinned `devpod v0.26.1` is **not** `loft-sh/devpod` (whose tags stop at
`v0.6.15`). `go version -m` on the binary reports `github.com/skevetter/devpod
v0.26.1` — a hard fork — installed from the `blooop` prefix.dev channel. So the
strings below belong to that fork, and its release cadence is the one that
matters (v0.22.1 → v0.26.1 inside ~10 weeks).

The output announces its own instability. In plain mode devpod prints the Go
source location of every log call:

```
18:59:28 info creating devcontainer up.go:581
18:59:33 info setting up container tunnelserver.go:426
```

and `cmd/up.go:581` in the fork is exactly `log.Info("creating devcontainer")`.
Output that reports where in the source it came from is a logger's rendering,
not an interface.

The json shape is the private serialization of `skevetter/log` (a fork of
`loft-sh/log`), whose `Line` struct carries `omitempty` on **every** field —
which is why `message` disappears, and why `level` could too. Nothing in the
fork's docs mentions `--log-output`'s schema; `devpod up` has no `--output
json` result interface, and its telemetry records no durations.

The decisive precedent: commit `f0b243ec` (2026-04-14, first released in
**v0.22.1**) moved this very data *out of* a structured `fields` object and
*into* the message string, in an ordinary refactor titled "replace WithFields
with format strings":

```diff
-	d.Log.WithFields(logrus.Fields{"image": options.Image}).Info("inspecting image")
+	d.Log.Infof("inspecting image: image=%s", options.Image)
```

The structured form existed, and was deleted two months before the pinned
release. Marker strings are rewritten in routine refactors, with no
deprecation. (The fork's author has also moved on to a successor project whose
logger renames the json keys `time`→`ts` and `message`→`msg` and changes
`--log-output`'s enum to `text|json|logfmt` — so `plain` would be rejected
outright.)

## Provenance

Captured with everything scoped to a throwaway `DEVPOD_HOME`
(`DEVPOD_HOME`/`DEVPOD_SSH_CONFIG`/`XDG_CACHE_HOME` under a temp dir); all
workspaces, containers and built images were removed afterwards. Host home
paths in `build.out` (the one `--debug` capture) were rewritten to
`/home/<user>/`; the captures are otherwise verbatim.
