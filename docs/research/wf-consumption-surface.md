# wf's consumption surface: what devlaunch-core must expose

Research for [blooop/devlaunch#250](https://github.com/blooop/devlaunch/issues/250), on the map
[Rust rewrite: implementation plan (#248)](https://github.com/blooop/devlaunch/issues/248).

**As of 2026-08-18.** Facts read at:

- `blooop/wayfinder` HEAD `7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba` (wf 0.18.0) — all `file:line` citations below are at this commit.
- `blooop/devlaunch` origin/main HEAD `54d828009a8b6e8a4ee850e0cef7bde427baf18b` (release 0.0.27).

Scope note: this ticket is facts only. The API *design* — what `devlaunch-core` actually exports and how —
belongs to [#251](https://github.com/blooop/devlaunch/issues/251). Where this doc says "minimal surface" it is
naming the set of things wf parses today, i.e. the parsing a linked crate would let it delete.

One headline fact up front: **wf never spawns `devpod` directly.** Every devpod fact wf holds (container
state vocabulary, workspace records) arrives through `dl` — either parsed from `dl --ls --json` or known
only to test scaffolding. The seam devlaunch-core must cover is exactly the `dl` seam.

---

## 1. What wf shells out to `dl`/`devpod` for today

Every production call site, with the data parsed back. All `dl` children except the launch exec are built
through `launch::unstamped` ([src/launch.rs:1484–1490](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1484-L1490)),
which strips the two timing env vars so an inherited handoff stamp cannot ride onto a non-launch child.

### 1.1 `dl --version` — capability probe

- Call site: `devlaunch_on_path()`, [src/launch.rs:1348–1361](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1348-L1361). Memoized in a `OnceLock` — one probe per process, because `Isolation::detect` runs per candidate checkout and each probe starts a Python interpreter (~90 ms, measured in devlaunch#53).
- Parsed back: the first line's first whitespace-separated word starting with a digit, as `DlVersion(u32, u32, u32)` ([src/launch.rs:1224–1245](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1224-L1245)). Deliberately tolerant: accepts the released `dl 0.0.24`, the editable-install trailer `dl 0.0.24 (dev, editable from /path)`, and pre-release tails (`0.0.24rc1` reads as 0.0.24).
- Classified into `Devlaunch::{Absent, Unreadable, TooOld(v), Usable}` ([src/launch.rs:1258–1270](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1258-L1270)) against two version constants:
  - `DEVLAUNCH_FLOOR = 0.0.24` ([src/launch.rs:1152](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1152)) — the oldest `dl` whose command line wf speaks (`up` arrived in 0.0.24). Below it wf **degrades to a host launch** and says why ([`shortfall`](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1298-L1310)); it never refuses the launch.
  - `UNSAVED_IS_AN_OBJECT = 0.0.24` ([src/launch.rs:1381](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1381)) — the release where `--ls --json` began answering `unsaved` for every clone dl made. Governs how a `null` in the listing is read (§1.2).
- The floor is pinned executably: `tests/live_devlaunch.rs` runs a **real** `dl` in four pixi environments (none / floor / latest / 0.0.23-stale) and fails if the pixi pin and the constant name different releases ([tests/live_devlaunch.rs:1–60](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/tests/live_devlaunch.rs#L1-L60)).

### 1.2 `dl --ls --json` — the workspace listing (the big one)

- Call site: `reap::workspaces()`, [src/reap.rs:1000–1018](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L1000-L1018). Errors name the dependency: "needs devlaunch 0.0.21 or newer, which is where --json arrived".
- Two consumers of the one reading:
  1. **`wf reap`** (interactive `wf reap [-y] [-f]` and the autonomous `wf reap --finished <owner/repo#n>…`) — plans deletions from it ([src/reap.rs:1272–1289](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L1272-L1289) is the only deletion path).
  2. **The picker's background survey** — `reclaim::survey_live()` ([src/reclaim.rs:346](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reclaim.rs#L346)) takes one listing plus one batched GraphQL tracker query and derives both the "N reclaimable — wf reap" hint and per-node **liveness** markings (Running / Stalled), spawned off the first frame by [src/refresh.rs:341–371](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/refresh.rs#L341-L371) and fed from [src/picker.rs:341](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/picker.rs#L341). Fails silent by design — no `dl`, failed listing, no network all mean "no hint".
- Parsed back into `Workspace` ([src/reap.rs:76–110](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L76-L110)), serde camelCase, unknown fields ignored so a newer `dl` adding fields does not break it:
  - `id: String` — the devpod workspace id.
  - `devlaunch: bool` — whether `dl` created it. Anything else is not wf's to touch.
  - `repo: Option<String>` — full `owner/name` slug from dl's record.
  - `branch: Option<String>` — the workspace's branch.
  - `state: Option<String>` — **devpod's** state, relayed by dl: `Running`, `Busy`, `Stopped`, `NotFound`, or `null` when `devpod status` would not answer. wf hardcodes the vocabulary in two predicates: `is_running` (`== "Running"`, [src/reap.rs:121–123](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L121-L123)) and `is_down` (`Stopped | NotFound`, [src/reap.rs:144–146](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L144-L146)); `Busy`/`null`/anything newer answers false to both, deliberately.
  - `unsaved: Option<Unsaved>` — what deleting the clone would destroy, in dl's words. This field is where the wire-format pain concentrates; see the `Unsaved` sum in §2.
- Wire tolerance wf carries for `unsaved` ([src/reap.rs:242–360](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L242-L360)): devlaunch ≤ 0.0.23 emitted a bare sentence or `null`; ≥ 0.0.24 emits a one-key object with documented keys `wouldLose` / `couldNotTell` / `nothingToLose` ([src/reap.rs:288–292](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L288-L292)). Both spellings are accepted; an unrecognised value becomes `Unsaved::Unrecognized` (refuses to reap that row, quotes the keys back) rather than failing the whole listing. `nothingToLose` must be literally `true` — `false` or any other payload refuses.
- Version-dependent *semantics* applied after the parse: `answered_where_dl_answers` ([src/reap.rs:1056+](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L1056)) rewrites a `None` unsaved on a dl-made clone into "dl's inspection fell over" **only** when the probed dl ≥ 0.0.24; on older releases `null` is the ordinary clean case. The same bytes mean opposite things either side of one release — only the version can disambiguate, which is why wf carries two constants for one release.

### 1.3 `dl <id> rm [--force]` — workspace removal

- Argv built by `removal_argv(id, insist)` ([src/reap.rs:1030–1037](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L1030-L1037)); executed by the module-private `remove` ([src/reap.rs:1272–1289](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L1272-L1289)) — the only function in wf that deletes a workspace, guarded by a compile-time privacy boundary plus source-text denylists and a recorded-argv probe ([src/probe.rs](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/probe.rs)).
- Parsed back: **exit status only**; stderr is quoted into the error on failure. `--force` passes dl's own unsaved-work waiver through and is only ever a human's `-f` (never automatic; `wf reap --finished -f` is rejected outright).
- wf deliberately relies on dl's refusal as a second guard: `plan` already skipped unsafe rows, so a refusal here means the clone changed between the listing and the rm — a reason to stop, not insist.

### 1.4 `dl <workspace> up` — the prewarm

- Argv built by `prewarm_argv(workspace)` ([src/launch.rs:1190–1196](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1190-L1196)); planned by `prewarm()` ([src/launch.rs:2046–2052](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L2046-L2052)); fired detached via `spawn_detached` — a double-forked `sh -c '… >/dev/null 2>&1 &'` in its own process group ([src/launch.rs:2082–2117](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L2082-L2117)), called from [src/app.rs:1002](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/app.rs#L1002).
- Parsed back: **nothing** — output goes to `/dev/null`, failure is silent (the launch that follows runs the same path and reports what is wrong). Opt-in via `WF_PREWARM`. wf relies on dl serializing the subsequent launch against the prewarm via a per-workspace lock.
- `up` is the verb that forced `DEVLAUNCH_FLOOR`: sent to a real 0.0.23 it fails with `Unknown command 'up'` inside a detached process nobody watches.

### 1.5 `dl <workspace> -- '<agent command>'` — the launch itself (exec, not spawn)

- Argv built by `isolated_argv(workspace, agent)` ([src/launch.rs:1170–1183](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1170-L1183)), selected in `Launch::agent_argv` ([src/launch.rs:1797–1802](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1797-L1802)), and **exec'd** — wf replaces its own process image ([`Launch::exec`, src/launch.rs:1856–1908](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1856-L1908)). Nothing is parsed back; there is no wf left to parse it.
- Two dl contract details wf encodes:
  - The workspace spec is `owner/repo@wayfinder/<repo>-<n>` for a node — one branch, one clone, one container per ticket ([`node_workspace_name`, src/launch.rs:1972–1979](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1972-L1979)); bare `owner/repo` for a creation. wf counts on dl creating the branch locally off the default branch when it does not exist.
  - Everything after `--` is a **shell command, not an argv**: dl joins the args with spaces and hands the string to `devpod ssh --command`, so wf single-quotes every argument itself ([`shell_quote`, src/launch.rs:1400–1402](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1400-L1402)). Recorded as "the one genuinely easy-to-get-wrong detail" on wayfinder#80.
- Isolation is decided by `Isolation::detect` ([src/launch.rs:1104–1113](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1104-L1113)): Claude agent **and** a devcontainer config exists (`.devcontainer/devcontainer.json` or `.devcontainer.json`, existence only — wf never reads the JSONC) **and** the probed dl is `Usable`. Anything else runs on the host, with the shortfall named in the launch notice.

### 1.6 The env-var timing seam (write-only, on the exec)

- On a Devlaunch exec — and only there — wf sets `DEVLAUNCH_HANDOFF_T0` and `DEVLAUNCH_PREWARM_FIRED_AT` ([src/launch.rs:1460, 1463](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1460), [`stamps`, 1818–1830](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1818-L1830)) as `{secs}.{nanos:09}` since the epoch — t0 is the keystroke that resolved to the exec; the second is when a prewarm was fired for this node, if one was. A host launch removes both. The variable names and value spelling are **dl's to mint** (blooop/devlaunch#194); wf is only the writer.

### 1.7 What wf does *not* call, and does not read

- **No `devpod` subprocess anywhere.** The only `Command::new` targets in production `src/` are `dl`, `gh`, `git`, and `sh` (grep at this commit: src/reap.rs, src/fetch.rs, src/launch.rs, src/projects.rs). Devpod's state vocabulary reaches wf exclusively through the listing's `state` field.
- **No production reads of dl's or devpod's on-disk state.** `~/.cache/devlaunch/repos/<owner>/<repo>/<id>`, `~/.cache/devlaunch/metadata.json`, and `~/.devpod/contexts/default/workspaces/<id>` appear only in test scaffolding ([src/probe.rs:88–104](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/probe.rs#L88-L104)), transcribed off a real machine to lay out a scratch `HOME` for destruction probes. Production wf asks dl instead.
- The `gh api graphql` calls beside the listing ([src/reap.rs:1115](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L1115), [src/fetch.rs:282, 422](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/fetch.rs#L282)) are tracker facts, not devlaunch's seam.

---

## 2. The typed model wf reconstructs — required vs anticipated

### Required (exists in wf's code today; each is parsing a linked crate could delete)

| wf type | Reconstructs | Where |
|---|---|---|
| `Workspace` | one row of `dl --ls --json`: id, dl-made, repo slug, branch, devpod state, unsaved | [src/reap.rs:76–110](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L76-L110) |
| `Unsaved` (4-arm sum) | `WouldLose(what)` / `CouldNotTell(why)` / `NothingToLose` / `Unrecognized(said)`, decoded from **two** wire generations plus an unknown-forward arm | [src/reap.rs:149–360](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L149-L360) |
| `is_running` / `is_down` | devpod's state vocabulary as hardcoded strings (`Running`; `Stopped`/`NotFound`), with `Busy`/`null`/unknown deliberately neither | [src/reap.rs:121–146](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L121-L146) |
| `DlVersion` + `Devlaunch` enum | `--version` stdout → ordered triple → Absent/Unreadable/TooOld/Usable, plus the two release constants (floor 0.0.24; unsaved-is-an-object 0.0.24) | [src/launch.rs:1152–1381](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1152-L1381) |
| `node_of` / `Node` | workspace → ticket: dl-made + branch `wayfinder/<short-repo>-<n>` → `Node { repo, number }` | [src/reap.rs:580–598](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L580-L598) |
| `node_workspace_name` | ticket → workspace spec `owner/repo@wayfinder/<repo>-<n>` (the inverse of the above, computed independently) | [src/launch.rs:1972–1979](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1972-L1979) |
| argv builders | `removal_argv`, `prewarm_argv`, `isolated_argv` + `shell_quote` — wf spelling dl's CLI for it | [src/reap.rs:1030](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L1030), [src/launch.rs:1170, 1190, 1400](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1170) |
| `Handoff` + `unstamped` | the timing env-var seam: two names, epoch-seconds spelling, set-on-launch / scrubbed-on-children discipline | [src/launch.rs:1416–1490](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1416-L1490) |
| version-conditional semantics | `answered_where_dl_answers`: what a missing `unsaved` *means* depends on which dl wrote it | [src/reap.rs:1055+](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L1055) |

The cost of this reconstruction is not hypothetical. Recorded incidents, all from version skew across the
subprocess seam:

- `dl <ws> up` shipped in wf 0.14.0 before devlaunch 0.0.24 carried the verb — failed inside a detached process (origin of `DEVLAUNCH_FLOOR`; [src/launch.rs:1140–1151](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/launch.rs#L1140-L1151)).
- Released wf 0.15.1 and dl 0.0.25 **could not parse each other** when `unsaved` became a sum — fails closed, reap collects nothing (recorded on wayfinder#138's resolution, in [#121's decision log](https://github.com/blooop/wayfinder/issues/121)).
- The `unsaved` `null` ambiguity — "clean" vs "not dl's clone" vs "dl's inspection fell over" — needed a version probe to read at all (devlaunch#171 / #174 territory; [src/reap.rs:149–166](https://github.com/blooop/wayfinder/blob/7d0b9ea53d2e715e6b190ac5a642a84cdd47d9ba/src/reap.rs#L149-L166)).
- wf maintains a four-environment live contract-test matrix (`tests/live_devlaunch.rs`) purely to hold its fixtures against the real program — machinery a pinned git dependency makes redundant.

### Anticipated (named on wayfinder's tracker, not in code — wants, not requirements)

From **[wayfinder#159](https://github.com/blooop/wayfinder/issues/159)** (Launch benchmarking: wf's side of the seam):

- dl classifies each launch's prewarm **hit / partial / miss** from the two stamps wf already sets — wf deliberately claims nothing ("hit" is dl's to observe). Shipped on wf's side; the classification is dl's side of the contract (devlaunch#194's vocabulary).
- Prewarm **hit-rate over time** waits on the devlaunch map's local launch-history file — "a field measurement by nature". If devlaunch-core owned launch history, wf would be a reader of it.

From **[wayfinder#121](https://github.com/blooop/wayfinder/issues/121)** (Launch latency), its decision log and out-of-scope list — dl-side items wf explicitly wants but does not code against:

- A **reliable unsaved-work guard** (devlaunch#171: dl's git probe ran without `--git-dir`/ceiling and could answer from an ancestor repo) — auto-reap (#138) was blocked on it; trusting the typed answer is only as good as the inspection behind it.
- **Disk lifecycle that is dl's**: prune/purge automation (devlaunch#88), image-layer GC, clone strategy (shallow/reference/worktree), caching the 342 MB tools tar, idle-stop of abandoned prewarm containers (`Running` hides dead weight from reap).
- `postCreateCommand` churn making pristine clones read as `unsaved` (pixi.lock) — routed to devlaunch.

From **[wayfinder#35](https://github.com/blooop/wayfinder/issues/35)** and the closed **[#80](https://github.com/blooop/wayfinder/issues/80)** (which tool wf shells out to): dl was *chosen* as the front door over raw devpod/devcontainer for per-branch workspaces, the shared-git-objects worktree backend, variant selection, and lifecycle verbs. #80's mechanics list survives in code (the shell-command seam, `--ide none` by default). Two of its facts have since moved: the "checkout path as the seam" resolution was superseded by per-node git specs (`owner/repo@wayfinder/<repo>-<n>`), and its posture "dl stays Python and stays a PATH dependency" is exactly what devlaunch#53's go-decision reversed — wayfinder#80 itself noted the floor is "the honest expression of a subprocess dependency: wf cannot pin dl's version the way a linked crate would (devlaunch#53)".

---

## 3. The minimal first API surface (facts, not design)

The smallest set that lets wf delete its parsing without waiting for the whole port — i.e. the closure of
§1's parsed-back data and §2's required table. Design (naming, sync/async, error types) is #251's.

1. **The workspace listing as a typed value** — everything behind `--ls --json`: workspace id, dl-made flag, repo slug, branch, container state **as an enum** (devpod's Running/Busy/Stopped/NotFound plus unknown-forward), and unsaved **as a sum** (would-lose / could-not-tell / nothing-to-lose, total over future variants). This alone deletes wf's largest parsing module (the `Workspace`/`Unsaved`/`UnsavedWire` layer, ~300 lines of wire tolerance in src/reap.rs), both version constants, the `--version` probe as used by reap, and the `answered_where_dl_answers` version-conditional rewrite — a linked crate has no "which dl wrote this" question.
2. **Typed lifecycle operations, or at minimum their argv/contract**: remove(id, force) with the unsaved-work guard's refusal as a typed error, and up(workspace) for the prewarm. Today wf parses only exit status + stderr from these; typed errors would replace string-quoting.
3. **The workspace-spec / branch convention as shared code**: `owner/repo@<branch>` spec construction and the `wayfinder/<repo>-<n>` node↔workspace mapping, which wf currently computes in two places (`node_workspace_name` forward, `node_of` inverse) against dl's spec parser it cannot see.
4. **The launch-handoff seam's constants**: the two env-var names and the epoch-seconds spelling (devlaunch#194's vocabulary), exported rather than duplicated.
5. **The container-command quoting contract**: today "everything after `--` is a shell command" forces wf to POSIX-quote; whether core exposes a launch that takes a real argv is a #251 design question, but the quoting helper is the parsing-adjacent code wf carries for it.

Explicitly **not** needed for the first cut, on today's facts: the launch exec itself (wf `exec`s `dl` the
binary and parses nothing back — it can keep doing that against a pinned-version `dl` while consuming the
listing as a crate), devcontainer.json reading (wf tests existence only, by policy), and any filesystem
model of `~/.cache/devlaunch` / `~/.devpod` (production wf never reads them).

What linking also retires: the four-environment contract-test matrix, the `Devlaunch::TooOld` degradation
machinery (a pinned crate version replaces the floor), and the memoized ~90 ms Python `--version` probe.
