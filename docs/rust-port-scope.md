# The Rust port, as it stands — scoping note

Written 2026-08-10, from the state of `devlaunch` 0.0.24 and `wf` 0.14.0. Not a
plan and not a decision: it reconciles three records that currently disagree,
and puts numbers on the parts that were argued without them.

## Three records, and they contradict each other

| record | says |
|---|---|
| [devlaunch#53](https://github.com/blooop/devlaunch/issues/53) — *Decide: Rust rewrite — go or no-go* | **GO.** A cargo workspace: `devlaunch-core` (lib) + `dl` (bin). `wf` takes `devlaunch-core` as a pinned git dependency and calls typed functions. **Subprocess-on-PATH was considered and rejected.** |
| `CHANGELOG.md`, `[Unreleased]` | The rewrite **is deferred**; Python remains the implementation, with no cutoff to plan around. Work previously ruled out ahead of a rewrite is back on the table. |
| [wayfinder#80](https://github.com/blooop/wayfinder/issues/80) — *Which tool does `wf` shell out to* | **`dl`, over PATH, as a subprocess.** "That `dl` is Python is not a cost `wf` pays… The Rust/Python boundary is a process boundary, which is the cheapest one there is." Out of scope: "Rewriting devlaunch in Rust or porting it off devpod" — *reinforced, not redrawn*. |

The first and third answered the same question — *how does `wf` consume
devlaunch* — and reached opposite conclusions, each without citing the other.
The second is why nobody had to resolve it: with the port deferred, the crate
was not available to consume, so wf#80's decision was the only one that could
ship. It shipped. `wf` 0.14.0 execs
`dl owner/repo@wayfinder/<repo>-<n> -- claude …` today.

**The disagreement is still live**, because #53's GO was never withdrawn — only
deferred — and #53's stated driver was precisely this integration.

## What has happened since #53, and what it settled empirically

#53's case for the crate over the subprocess rested on one argument: both
options create a second surface, and the crate is the one **the compiler
watches**.

> Crate → a breaking change **fails `wf`'s build**. Subprocess → drift is
> **silent until a user hits it at runtime**.

That prediction has now been tested, and it held, in the first release where it
could:

- `wf` 0.14.0 prewarms a launch with `dl <workspace> up`.
- `up` did not exist in any released `dl`. It lands in **0.0.24**.
- Every check `wf` made — `dl` is on PATH, the checkout declares a
  devcontainer — passed against the released `dl` 0.0.23, and the launch failed
  *inside* the prewarm on an argument that release had never heard of.
- Nothing caught it. Not CI in either repo, not the recipe, not a version
  constraint, because there was none to violate.

The mitigation now in `wf` is a **runtime version floor**: `wf` probes
`dl --version` once per process and treats a `dl` below the floor as unusable,
degrading to a host launch that states the reason. That is the best a subprocess
seam can do — it converts a mid-launch failure into a visible degradation — and
it is strictly weaker than what #53 promised, which was a build failure at the
point the incompatibility was introduced.

A second coupling landed the same day: `wf`'s conda recipe now declares
`devlaunch >=0.0.24` as a run dependency, so `pixi global install wf` brings
`dl` with it. That makes the pairing installable in one step, and it makes the
runtime coupling explicit in packaging metadata — but it is the *opposite*
direction from #53's endpoint, and is worth reading as a cost of the deferral
rather than as a decision against the crate. Under #53's shape, `wf` would not
depend on the devlaunch *package* at all: it would link `devlaunch-core` at
compile time and declare `devpod` at runtime.

## The numbers the arguments were missing

### Distribution size — measured on this machine, `pixi global` envs

`devlaunch`'s environment is **370 MB**. Its own code is **84 KiB** of that:

| package | payload |
|---|---|
| python 3.14.6 | 275 MB |
| devpod 0.26.1 | 118 MB |
| libstdcxx | 72 MB |
| icu | 48 MB |
| openssl | 20 MB |
| tk | 11 MB |
| iterfzf | 4.5 MB |
| **devlaunch** | **84 KiB** |

For comparison, `wf`'s whole environment is **3.2 MB** (a 3.1 MB binary from
8,765 lines of Rust).

So a Rust `dl` removes the CPython stack and `iterfzf` and lands at roughly
**120 MB — and devpod is 118 MB of it.** The floor is devpod, not the language.
Two thirds off is real, but "a small static binary" is not on offer while `dl`
drives devpod, and #53 already recorded that even in Rust,
`devlaunch-core` still shells out to it.

`tk` and `icu` are worth a footnote: nothing in `dl` asks for them. They arrive
with conda-forge's `python` build. A Python `dl` could shed ~60 MB of the 370 MB
without any rewrite if a leaner interpreter dependency were available — the
cheapest size win on this list, and it is not a Rust win.

### Port size

| | count |
|---|---|
| `devlaunch/*.py` source | **7,373 lines**, 21 modules |
| `test/` | **18,039 lines** |
| mock-free acceptance tests (per #53: e2e 8, integration 10, storage 20, models 7, config 7, spec-parsing 7, helpers 2) | **61** — port; language-agnostic, aimed at the CLI boundary |
| mock-based tests (`test_dl.py` alone = 234) | **376** — read as a behaviour spec, not ported |
| `wf`'s existing Rust, for scale | 8,765 lines |

The port is therefore roughly "another `wf`" of source, against a test suite
2.4× the size of the code, most of which does not come with it. #53's migration
answer — parallel implementation, cut over at acceptance parity, with the 61
mock-free tests as the shared harness — remains the only shape that keeps `dl`
shipping throughout, and nothing since has weakened it.

What the CHANGELOG since 0.0.11 adds to the estimate is the *character* of the
behaviour to be re-established, which is where a port of a tool like this
actually goes wrong: per-workspace `flock` serialisation of concurrent `up`s, a
clone guard for a half-removed `.git`, the `gh` token kept off argv and
refreshed per start, a three-state tools probe against "the official claude
layout", `--purge` that removes what it is permitted to and names what refused,
a symlinked-cache-root refusal, git-lfs probing gated on pointer content. Each
of those is a bug that was found in use, and each is a line the 376 mock-based
tests pin and a Rust suite would have to re-pin from reading them.

### Performance — settled in #53, restated so it is not re-argued

`devpod list --output json` = 0.44 s; `dl --help` (whole interpreter start) =
0.09 s. One devpod round trip costs 5× the entire Python startup, and `dl.py`
has 19 subprocess call sites. Perf is not an argument for the rewrite in either
direction; the wins are in the devpod call graph and transfer to either
language.

## What is actually being decided

Not "Rust or Python" — #53 answered that on reuse grounds and the answer was
never withdrawn. The live question is narrower:

> **Does `wf` consuming `dl` as a subprocess — now shipped, now version-floored,
> now declared as a conda run dependency — satisfy the need that #53's GO was
> for?**

Two coherent positions, and they differ in what they do with the evidence above:

1. **The subprocess seam is sufficient; retire #53's GO.** The integration works
   today. The one drift #53 predicted did occur and is now handled by a floor,
   at a cost of one constant in `wf` and one release ritual in devlaunch. Under
   this reading, wf#80's Out-of-scope line stands, the CHANGELOG's deferral
   becomes permanent, and #53 should be reopened only to record that its driver
   was met another way. The price accepted is 370 MB per install, a runtime
   version contract policed by a probe rather than a compiler, and #53's
   falsifier left unaddressed: `wf` is expected to **read and render** per-ticket
   workspace state (live/stopped/none), and doing that over a subprocess means
   inventing the JSON contract #53 rejected inventing.
2. **The GO stands and the deferral is the thing to revisit.** The evidence for
   #53's central argument is now empirical rather than predicted, `wf` has grown
   from "launch and forget" toward reading workspace state, and the size figure
   (370 → ~120 MB) is a genuine if secondary win. The price is ~7.4k lines
   ported against a 61-test acceptance harness, a cross-repo semver tax #53
   named and accepted, and a period where both implementations exist.

The falsifier #53 named is still the right one to check, and it is now
checkable rather than hypothetical: **does `wf` render workspace state?** If it
stays launch-only, subprocess wins on every axis and position 1 is correct. The
moment it wants live/stopped/none beside a ticket, the choice is a typed model
versus a hand-rolled JSON contract, which is the argument #53 already had.

## Recommended next step

Reopen #53 (or file its successor) with **one** question — the state-rendering
one above — rather than re-litigating the language. Whichever way it goes,
record it in *both* repos this time: wayfinder#80's Out-of-scope line and this
CHANGELOG's deferral paragraph are the two places that went out of sync, and
they are the two places that need to agree.

Whatever is decided, three things are true now and should not be undone by it:
the version floor in `wf`, `up` shipping in 0.0.24, and the run dependency that
makes the pair installable in one step. If the crate lands, the run dependency
changes target — `devpod`, not `devlaunch` — and the 275 MB of CPython goes with
it.
