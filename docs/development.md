# Developing devlaunch

Building and testing the crates, the guards CI runs, the prebuilt dev container
image, and what a branch workspace costs on disk.

## Development

`dl` and `aid` are Rust. `rust/` is what ships: one cargo workspace, built and tested with cargo,
and it is where the tests of `dl`'s own behaviour live.

```bash
cd rust
cargo build --release                          # target/release/{dl,aid}
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Those cargo commands need no toolchain of your own inside this repository's devcontainer: `rust`
1.97.1 is in the `default` pixi environment, so `pixi run cargo test --workspace` works the moment a
container comes up. `pixi run` keeps the directory it was called from, so `cd rust` first, exactly as
above. `rust/rust-toolchain.toml` remains the pin of record. A conda `rust` does not read it, so the
version is named again in `pyproject.toml` and moves by hand with `ci.yml`'s three toolchain
steps.

The Python that remains is the **acceptance harness** and the repo's own documentation guards. It
judges the shipped binaries from outside, spawning them against a real devpod or against the fake
one in `test/fixtures/devpod_shim.py`, and imports nothing that ships:

```bash
pixi run ci                                    # ruff, pylint, ty, and the harness under coverage
pixi run test-e2e                              # the real-devpod tier: builds real containers
```

Both build `rust/target/release/{dl,aid}` first, because that is what the harness runs.
`DEVLAUNCH_DL_CMD` / `DEVLAUNCH_AID_CMD` are the seams that point it somewhere else: a debug build,
an installed release, the wheel's binary.

```bash
# a bare pytest, so the release build the `test` task depends on is skipped too
DEVLAUNCH_DL_CMD='cargo run -q --manifest-path rust/Cargo.toml -p dl --bin dl --' pixi run pytest
```

### The public-API snapshots

Three files under `rust/` are the crates' public API as `cargo public-api` renders it, and
CI's `public-api` job fails until a change to any of them is committed. They are not one file
because they are not one promise:

| File | What a diff means |
| --- | --- |
| `devlaunch-core/public-api.api.txt` | **A change to the promised contract.** A removal or a changed signature breaks a consumer, an addition is a deliberate widening. Holds the 37 declarations written *at* the `devlaunch_core::api` path, and only those. |
| `devlaunch-core/public-api.rest.txt` | Mostly routine. The binary API (`flows::`, `domain::`, `clients::`) is reachable but never promised, so read it for the accidental `pub`. **But** the promised types' methods and impls are in here too (see below), and a diff touching one of those is a contract change. |
| `devlaunch-runner/public-api.txt` | The process seam an external `Runner` implementer writes against. |

**The promise file holds declarations, not behaviour.** `cargo public-api` renders inherent methods
and trait impls only at a type's *canonical* path, never at the path it is re-exported under, so
the classifier cannot see them. `api::Launch`'s only constructor and only method are rendered
`flows::launch::Launch::{new, run}` and land in the rest file, along with `CommandContext::new`,
`DevcontainerPath::as_str` and every derived `Clone`/`Debug`/`PartialEq` on the promised types: 42
of the 79 rows the generator emits for the `api` section. Measured consequence: renaming
`api::Launch::run` leaves `public-api.api.txt` byte-identical. The guard is therefore one-way. A
diff in the promise file is a change to the promise, but not every change to the promise diffs it.
Widening the classifier is [#352](https://github.com/blooop/devlaunch/issues/352).

The runner had no snapshot of its own until #338: its whole API entered core's as the single
unexpanded row `pub use devlaunch_core::runner::<<devlaunch_runner::*>>`, so removing a trait
method moved nothing and passed. And core's one file mixed the two tiers, which is worse than it
sounds. A change to the promised declarations arrives as one row inside two thousand of internal
churn, and reads as routine.

Regenerate all three with one command, from the repository root (or by absolute path from
anywhere, since the script resolves the checkout from its own location):

```bash
scripts/public-api-snapshots.sh
```

That script is also what CI runs, into a scratch tree and then diffing the files it names via
`--print-files`, so the filter that decides which row is a promise, the `-ss` flag, the pinned
`cargo-public-api` version and the list of snapshots all exist in exactly one place. Two
prerequisites, and this repository's devcontainer has neither, so it is a host command: a nightly
toolchain (cargo-public-api's rustdoc-JSON backend is nightly-only; the crates themselves still
build on the stable pin) and the pinned tool.

```bash
rustup toolchain install nightly
cargo install cargo-public-api --locked --version "$(scripts/public-api-snapshots.sh --print-pin)"
```

Committing a regenerated `public-api.api.txt` is committing a change to the promised contract, so
say which one in the pull request. If the change was to a promised type's methods or impls,
the diff to point at is in `public-api.rest.txt`. `rust/devlaunch-core/tests/public_api_snapshots.rs`
holds the two core files to the split itself, every promised row an `api` declaration and none
of the others one, so a hand-edited snapshot fails in the Rust suite rather than in review.

**What a failed run leaves behind**, precisely, because "nothing" would be a claim rather than a
fact. The script checks every destination is writable before it generates anything, then writes
into a staging directory *inside* the destination and moves the files into place only once all
three exist. So a run that fails while generating, on a compile error, a guard firing or a Ctrl-C,
leaves the checked-in snapshots byte-identical. Staging on the same filesystem makes each move a
rename rather than a copy, so no file is ever seen half-written. What is *not* atomic is the set of
three: a crash between renames leaves some files new and some old, each one whole. CI's
regenerate-and-diff is what catches that, since a mixed set still satisfies every invariant the
tests over these files can check.

### Coverage: two numbers, and neither is the other

The crates that ship and the harness that judges them are measured separately, because they are
different things and a single figure is about neither:

```bash
pixi run coverage-rust                         # cargo llvm-cov over the whole workspace
cd rust && pixi run cargo llvm-cov --workspace --html -- --test-threads=1   # ...as a tree to open
```

The task carries its own `cwd`, so it runs from anywhere in the checkout; a bare `cargo llvm-cov`
needs the `cd` like any other cargo command.

```bash
pixi run coverage && pixi run coverage-report  # the Python harness, `scripts/` and the doc guards
```

CI runs both, `rust-coverage` and `ci`, and uploads them to Codecov under the `rust` and `python`
flags, which `codecov.yml` keeps from being averaged. Before #294 only the second one existed, and
after #267 retired the Python `dl` what it measured was `scripts/`: the shipped code's coverage was
nobody's for two releases.

**One thing to know if you write a boundary test.** The suites in `rust/dl/tests` and
`rust/aid/tests` spawn the real binary with `env_clear()`, so the child gets exactly the world the
test built and nothing from your shell. An instrumented binary needs `LLVM_PROFILE_FILE` to write
its counters anywhere `cargo llvm-cov` will read them, so every one of those spawns calls
`.keeping_coverage()` straight after `.env_clear()`, the one variable that is allowed back in.
Leave it out of a new spawn and the test still passes; it just stops counting, silently, and takes
whatever it was the only cover for down with it. Measured before that seam existed, one five-test
suite that runs `dl` end to end reported `render.rs`, `commands.rs` and `cli.rs` at 0.00%.

Two cargo tests are outside the CI measurement on purpose: `cargo test -p dl --test interrupt`
and `--test lock_wait` kill real process trees, and a SIGKILLed process writes no counters. They run in
the `rust` job like everything else, and `pixi run coverage-rust` includes them locally.

Inside this repository's devcontainer, `pixi run dl` and `pixi run aid` are `cargo run` over the
working tree; on a host, `./dev.sh` installs it as `dl-next`/`aid-next` beside the released pair. Both
print a `-dev` version so a working-tree build is never mistaken for a released one. See
[AGENTS.md](AGENTS.md).

### The Quickstart's demo GIFs

The four GIFs in the Quickstart come from VHS tapes in `docs/demo/`:

```bash
pixi run demo                  # all four, in order
pixi run demo 2-branches       # one, while you tune it
```

`scripts/record_demo.sh` records them, optimises them with `gifsicle`, and prints what to check
before you commit. A GIF that has not been recorded yet keeps its README line commented out, so a
clone without them shows no broken images; the script uncomments each line as its file appears.

The tapes film the released `dl`, not this working tree. So the GIFs show what someone who installed
devlaunch sees, and they go stale when the released UI changes. Re-record after a release, not
before.

Until 0.1.0 this repository held a second, Python implementation of `dl`, and a parity harness that
ran both against the same fixtures and compared them. Both retired once the binaries shipped; the
history is in [docs/rust-rewrite-plan.md](docs/rust-rewrite-plan.md), whose divergence table still
records every deliberate behavioural difference from that build.

The two published artifacts are built from the same cargo package, both taking their version from
`rust/Cargo.toml`, the only place it is written down:

```bash
cd packaging/wheel && maturin build --release --locked -o dist    # the PyPI wheel: two binaries
rattler-build build --experimental --recipe conda.recipe/recipe.yaml   # the conda package
```

`.github/workflows/publish.yml` and `conda-publish.yml` do exactly that on a version bump, in that
order, off one tag; `ci.yml`'s `packaging` job builds the wheel and renders the recipe on every pull
request, so a broken release is a red tick rather than a surprise.

Two places in this document restate that number, the `dl --version` transcript under
[Global commands](../README.md#global-commands) and the conda badge at the top, and both are read back
against `rust/Cargo.toml` by `test/test_readme_cli_doc.py`, so a bump that forgets them is a
failing test rather than a README advertising a release from last year. That guard reads the
same file for the flags: every long flag written on a `dl` command line anywhere here is handed
to the binary's own parser, which is what makes a documented flag that does not exist a red tick
too.

The Python half uses [pixi](https://pixi.sh) for environment management.

```bash
# Run tests
pixi run test

# Run the e2e suite: real devpod, real containers
pixi run test-e2e

# Run full CI suite
pixi run ci

# Format and lint
pixi run style
```

`pixi run test` skips the e2e tests, which need devpod and a Docker daemon and
build real containers. CI runs `pixi run test-e2e` in a job of its own, outside
the Python matrix, on a throwaway runner, on every push to `main` and on every
pull request, whatever branch that pull request targets. Stacked chains, where
each link targets its predecessor rather than `main`, get the same CI as anything
else.

Alongside the matrix and e2e there is a `gate` job that does nothing but fail
unless every other job in that workflow succeeded. It exists so that a branch
ruleset has one stable name to require rather than a list: requiring the jobs one
by one means literal strings in a repository setting, which nobody reviews and
which goes stale the moment a job is added or renamed. And a required check that
no longer exists does not turn a merge red, it stops gating it. Adding a job
means adding it to `gate`'s `needs`, in the same pull request, where it can be
seen. It reaches only as far as its own workflow file, so the `prek` lint job is
not behind it and has to be required alongside it.

One of `gate`'s jobs is not a test. `review` reads the pull request's reviews and
fails unless *something* reviewed the code. It is there because the external
reviewer failed silently once and nobody noticed for a day and a half: Sourcery
answers a quota refusal *as a review*, so

```
Sorry @blooop, you have reached your weekly rate limit of 500000 diff characters.
```

arrives in the same shape as a review that found nothing and reads as one to
everything downstream. Twenty-six consecutive pull requests merged behind that
sentence, among them the largest changes in the repo. The other refusal is worse
targeted still: a per-pull-request diff cap, which fires on exactly the changes
least safe to merge unread.

The question it asks is **"was this reviewed"**, not "did Sourcery answer", and
the difference is what makes it survivable. A weekly quota lasts a week, and a job
that stopped every merge for a week would be a job somebody deletes, which is the
same way the flag guard in `test_readme_cli_doc.py` describes losing its own
teeth. So three things satisfy it:

- a review by **anyone other than the author**, whether a person or the bot when
  it is answering;
- a **`wf-review` report by the author**, recognised by the provenance line those
  reports open with. That is the review this repo actually runs when the bot is
  out: two axes, in fresh context that did not see the code written;
- the **`no-external-review` label**, for merging with neither.

An author's plain approval is not enough. "lgtm" from the person who wrote the
code is the thing being guarded against, not a way past it.

Whether a review is a refusal is decided by **who wrote it**, not by what it
says. Sniffing the prose was wrong twice in opposite directions: matched loosely
it ate the first `wf-review` report posted under this rule, because a review of
this guard quotes the refusal sentences; anchored to the `Sorry @` those
sentences open with, it stopped recognising a refusal that had a leading newline,
which is strictly weaker than the loose match it replaced. Only the reviewer can
refuse on its own behalf, so `REFUSING_LOGINS` is the question asked.

The cost is that a new external reviewer's refusals go unrecognised until its
login is added there. So an unlisted account posting something that reads like a
refusal still counts as a review, and the job emits a `::warning::` naming the
account, because a reviewer rename putting us back at the original incident with
no signal is the one way this design fails badly. A determined author
can of course write the provenance line by hand; the guard is against a review
silently not happening, and skipping one on purpose is what the label is for.
Merging on a self-review emits a `::notice::` saying so, because it is worth
seeing in the log afterwards.

Staleness is warned about, not failed on. A review of an earlier commit *did*
happen, which is a different thing from the absence this job exists to catch, and
failing it would mean re-reviewing after every typo fix, which is how a guard
earns being deleted. So when every review predates the head, the job passes and
says the code that merges is not the code that was reviewed. That case is not
hypothetical: it is how the change introducing this rule reached its own merge.

The classification itself is `scripts/review_verdict.sh`, which the workflow
calls and `test/test_review_guard.py` executes, for the reason the public-API
script is a script: a `case` statement inside a `run:` block can be tested only
by copying it, and the copy is the half that goes stale. Its tests run against
the refusal bodies those twenty-six pull requests actually received.

Running it yourself is a different proposition. This repo's devcontainer carries
a Docker daemon of its own, through the `docker-in-docker` feature, and pins the
same devpod a host installs, so `pixi run test-e2e` from inside it builds its
containers in there rather than on your Docker. You can also run it on a machine
you do not mind it writing to, an ephemeral CI runner, say. It is skipped by
default rather than gated on a container, because what it needs is a daemon, not
nesting. Either way the suite exercises `dl --purge`, so it gives itself a
private devpod namespace before collection begins. But the containers it builds
are real ones, and it wants several minutes and a 1.25 GB image pull the first
time.

Its skips mean one thing only. A test that opts out does so through
`fixtures.e2e_guard.opt_out`, and any other skip is reported as a failure,
because a run that could not reach a registry used to be indistinguishable from
a healthy one. Every run also prints what it actually built:

```
--------------------------------- e2e session ---------------------------------
22 e2e tests attempted, 5 workspaces created: e2e-test-create, e2e-test-lifecycle, e2e-test-git, e2e-purge-devlaunchs, e2e-purge-hand-made
```

A run whose workspace-building tests built nothing does not pass: the shortfall
is counted into the last line of the run, so `4 passed, 18 skipped` becomes
`1 failed, 4 passed, 18 skipped`. A run with no workspace-building tests in it,
`pytest -m e2e -k TestDLCommandsE2E` say, has nothing to answer for and says
so instead.

The nested daemon is also why the devcontainer does not join the host's network
namespace: a nested daemon needs a namespace of its own, or it co-manages the
host's `docker0` bridge and writes its NAT rules into the host's netfilter
tables.

### The prebuilt dev container image

Opening this repository's devcontainer used to build it: base image, pixi, the
local `claude-code` feature and `docker-in-docker`, several minutes of it, once
per branch. CI publishes that image now, so opening a workspace pulls it instead.

```
ghcr.io/blooop/devlaunch-devcontainer
```

Nothing has to be configured to use it. `.devcontainer/devcontainer.json` names
the repository under `customizations.devpod.prebuildRepository`, and `devpod up`,
which is every `dl` launch, checks it before building anything. The check is
for one exact tag: devpod hashes the build config together with the build
context and asks for `<repository>:devpod-<hash>`. A hit is used directly,
features included; a miss falls through to a local build, silently and without
failing. So the prebuild is a speed-up that cannot break a launch, and the
question to ask when a container open is slow is whether the tag matched.

Two consequences worth knowing:

- **The build context is `.devcontainer`, not the repository root**, and that is
  what makes the tag usable. The Dockerfile copies nothing out of the context,
  so the root was never needed, but it was hashed, which meant a different tag
  on every commit to any file and a prebuilt image that never matched one.
  Scoped to `.devcontainer`, the tag moves when `.devcontainer/**` moves, the
  Dockerfile and the local feature's scripts included.
- **A commit whose `.devcontainer/` differs from the last prebuild builds
  locally.** That is the correct answer rather than a gap: the alternative is a
  container built from something other than what the branch asks for. The pull
  comes back once the change is on `main`. What the tag does not promise is the
  converse, that one `.devcontainer/` tree always yields one image; see "What
  the prebuild tag does not promise" below.

`.github/workflows/devcontainer-prebuild.yml` publishes it, on pushes to `main`
that touch `.devcontainer/**` and on manual dispatch. Its path filter is exactly
the set of inputs to the hash, so a commit it skips is one that could not have
moved the tag. To publish from a branch by hand:

```bash
docker login ghcr.io                  # a PAT with write:packages, or `gh auth token`
pixi run devcontainer-prebuild        # devpod build . --tag latest
```

Both are idempotent: an existing prebuild is found and returned rather than
rebuilt and repushed. By hand publishes for the architecture of the machine you
run it on and no other, see "Two architectures, two tags" below, and takes the
moving alias as an argument (`pixi run devcontainer-prebuild latest-arm64`) if
that machine is not amd64.

**The package came up public on its own, and needed no manual step.** Measured on
the first run (`d05e4ce`): an anonymous pull of both tags returns `200`, with
nobody having touched a visibility setting. GHCR gave the package the visibility
of the public repository whose workflow published it, and the
`org.opencontainers.image.source` label in the Dockerfile is what links the two.

This is worth checking rather than trusting, because the failure is silent. A
private package makes the lookup return `DENIED`, devpod reads that as a cache
miss, and every launch quietly builds locally, the behaviour from before any of
this existed.

```bash
curl -s -o /dev/null -w '%{http_code}\n' \
  -H "Authorization: Bearer $(curl -s \
    'https://ghcr.io/token?scope=repository:blooop/devlaunch-devcontainer:pull&service=ghcr.io' \
    | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')" \
  https://ghcr.io/v2/blooop/devlaunch-devcontainer/manifests/latest
# 200 => public, anonymous pulls work
# 401/403 => private; make it public as below
```

If it ever *is* private, from a package created some other way or a visibility that
gets changed, the fix is a one-time setting, and one no workflow can make: there
is no REST endpoint for container visibility and no `gh` subcommand.
<https://github.com/blooop?tab=packages> → *devlaunch-devcontainer* → *Package
settings* → *Danger Zone* → *Change visibility* → **Public**. Nothing to
configure before the first publish creates the package, so those pages 404 until
then.

The `:latest` tag that command also pushes is not what devpod looks for. It is
the moving alias `build.cacheFrom` points at, a best-effort layer cache for
builders that know nothing about devpod prebuilds: VS Code's "Reopen in
Container", a plain `devcontainer up`. Those still run a build; what they save is
whatever layers the cache can serve them.

**Two architectures, two tags.** The target architecture is hashed along with the
build config and the context, so amd64 and arm64 ask for different tags, and the
workflow publishes both from a matrix over `ubuntu-latest` and `ubuntu-24.04-arm`,
GitHub's hosted arm64 runner, which is free without limit on a public repository.
Nothing merges the two into a multi-arch manifest, because devpod never looks one
up: it asks for one exact tag and pulls the variant for the architecture it is
running on. An architecture whose tag is missing is not a failure, only the same
silent local build as any other miss.

Neither leg passes `--platform`, and that is the point of using a native runner
rather than emulation. The architecture in the hash is the one the driver reports,
which for docker is the `runtime.GOARCH` of the devpod binary doing the build, so
the runner decides it, exactly as the launching machine decides it on the lookup
side, where `devpod up` passes no platform at all. `--platform linux/arm64` would
hash to the same string on an arm64 runner, which is the argument against it: a
second source of truth for something already settled, and one that can disagree
without saying so.

The legs are `fail-fast: false`. Each publishes a tag nothing else reads, so a run
where one architecture fails leaves the other pulling, which is better than the
two local builds that cancelling the survivor would leave behind.

`latest` is amd64's, and arm64 publishes `latest-arm64`. devpod arch-qualifies
nothing, so two legs passing the same alias would race for it and the winner would
be whichever finished last; and the one thing that reads `latest` is
`build.cacheFrom`, a layer cache which serves nothing at all across architectures.
So `latest` keeps meaning what it has always meant, instead of becoming a cache
that works or does not per run for reasons nobody could see. What `dl` reads is
neither alias: it is the `devpod-<hash>` tag for its own architecture.

Making the arm64 leg possible at all needed `linux-aarch64` in the pixi workspace
(`platforms` in `pyproject.toml`). A pixi workspace refuses outright on a platform
it does not declare, so without it the leg fails at `pixi install` before it can
build anything, and so does an arm64 container's own `postCreateCommand`, which
would have made an arm64 prebuild an image that pulls fast and then cannot come
up. It says nothing about the release: the wheel and the conda package are still
linux-64.

The arm64 leg went in unexercised, since this repository is developed on x86. Every
piece the image needs was checked to exist for linux-aarch64, meaning the multi-arch
base, the aarch64 pixi and `claude-shim` builds, and devpod itself, and none of it was
checked to build. If it turns out not to, the symptom on an arm64 host is the local
build that host was already doing.

`postCreateCommand` is not in the image and cannot be: `pixi install` and the
provider registration run at container create, after the image exists. The
`<workspace>-pixi` volume is what makes them cheap the second time.

That install is `pixi install --frozen`, which is the same resolution CI uses
(`frozen: true` in `ci.yml`) and is load-bearing for two reasons beyond matching
it. A bare `pixi install` treats a lock it cannot read as a missing one: it warns,
exits 0, solves a fresh environment, and rewrites the tracked `pixi.lock` on its
way past, so the container that is supposed to reproduce the committed
environment quietly stops being it, and says nothing. And the solve it does
instead reaches the network: resolving pypi dependencies alongside conda ones
needs a conda-pypi name mapping that pixi fetches remotely (the compressed
mapping is served out of `prefix-dev/parselmouth` on raw.githubusercontent.com),
and a create whose fetch fails dies in `postCreateCommand` with the workspace
never opening.

`--frozen` installs the committed resolution, so there is no solve and no mapping
to fetch. That is measured rather than reasoned: with the mapping cache deleted,
`pixi install --frozen --offline` installs the default environment and never
recreates the cache, while a solving install recreates it on the spot. And a lock
the container's pixi cannot read now fails immediately and says which pixi it
would need, instead of succeeding into the wrong environment.

#### What the prebuild tag does not promise

The tag is a hash of the build *recipe*, the config and the context, and not of
the image the recipe produces. Three of the recipe's inputs float, so the same
`.devcontainer/` tree published on two different days is two different images:
the base `mcr.microsoft.com/devcontainers/base:ubuntu-24.04` is a moving tag,
`ghcr.io/devcontainers/features/docker-in-docker:2` is a moving major, and the
local feature's `pixi global install … claude-shim` names no version. (The
Dockerfile's `ARG PIXI_VERSION` is the one that is pinned.) A prebuild pulled
today and a local build of the identical tree tomorrow can therefore differ, and
the direction they differ in is forwards: a republish picks up whatever those
three point at now.

That is written down rather than fixed, and the shim is the case that shows why.
**`claude-shim` is deliberately unversioned**, and pinning it was considered and
rejected on four grounds:

- **The package holds no `claude`.** It is 21 KB of bash, `claude`, `cld` and
  `cldr`, whose first run downloads the current stable binary from Anthropic's
  GCS bucket into `~/.claude/cache`, and which re-checks stable hourly after
  that. The image cannot serve a stale `claude`, because it contains none; a pin
  would freeze the fetcher and nothing it fetches.
- **On the launch path this container is opened by, the baked shim never runs.**
  `dl`'s probe reads a shim-provided `claude` as *lendable* and the lend puts the
  host's real binary in front of it on the PATH, per "What to bake so a launch does
  no work at all" above. The shim is baked so that `command -v claude` answers
  at all.
- **A pin freezes it harder than no pin does.** Unversioned, the shim is
  refreshed by every republish, which is every commit to `.devcontainer/**`.
  Pinned, it stops at whatever was current the day the pin was written.
- **One package would then have two policies.** The shipping provisioner
  (`rust/devlaunch-core/src/flows/provision.rs`) installs the same package,
  unversioned, into every workspace `dl` opens, the copy that reaches users
  rather than us. `test/unit/test_devcontainer_manifest.py` asserts that spec and
  the feature installer's still match, so pinning one side alone fails rather
  than drifting, while pinning both deliberately passes.

Measured 2026-08-20: the published prebuild carries `claude-shim 0.7.0`, the
newest of the channel's 14 releases and unmoved since 2026-04-06, so the drift a
pin would have prevented is currently zero. A publish is also the gate on a
broken shim release, not a channel for it: the feature installer checks the
trampoline exists and returns non-zero if it does not, which fails
`devpod build` and leaves the previous prebuild as the newest published tag.

If image contents ever do need pinning, the base image tag is the input that
carries the packages; pinning the shim alone would be precision about the
smallest of the three.

### Disk cost of the dev container

Opening a devcontainer for a branch costs about **3.3 GB on the host before you do
anything in it**: ~600 MB of image layers unique to this image, a ~680 MB container
writable layer, and a ~2.0 GB `<workspace>-pixi` volume.

That volume was ~520 MB until the `rust` feature went into the `default`
environment. Measured either side of that change, the pixi environment is 521 MB
without it and 2044 MB with it, and only about a quarter of the difference is the
compiler itself: conda's `rust` links with a gcc toolchain, a sysroot and binutils.
Adding `cargo-llvm-cov` and `llvm-tools` to that feature (#294) took it to
**2348 MB**, measured either side on the same machine. That is 301 MB, nearly all of it
`libllvm22`, for `pixi run coverage-rust` working in here rather than only on a
runner. The same argument as below applies to it and the numbers are an order of
magnitude smaller.

**That 1.5 GB per branch is bought deliberately, and what it buys is isolation.**
The toolchain that builds the crates is pinned in the project environment beside
the pinned `devpod`, so a workspace compiles what it is editing with no host
toolchain, no `rustup`, and no global install anywhere in the picture, and it is
the same version on every machine and in CI. Paying for that once per branch
workspace is the trade; a leaner environment that sent whoever is working in the
container back to a host `cargo` would be the wrong end of it.

The container carries its own Docker daemon, and that daemon's `/var/lib/docker`
lives on a second named volume. One `pixi run test-e2e` plus a couple of nested
workspaces puts **~2.3 GB** in there, and nothing garbage-collects it: the inner
daemon reports ~45% of its images reclaimable with no reclaimer. Nested daemons
share no layers with the host or with each other, so this is paid once per branch.

**Budget ~5.5 GB per branch you are actively developing and e2e-testing, about 17 GB
for three concurrent branches.**

The time cost is cold pulls in a fresh nested daemon: the first `devpod up` inside a
new container takes ~25s, ~16s of which is pulling a base image the host already has.
Workspaces after that reuse it and take ~8s.

**Both volumes now go when the workspace does**, see [what a delete takes with
it](cleanup.md#what-a-delete-takes-with-it). They did not always: `devpod delete` removes
the container with `docker rm` and never touches a volume, and Docker never
garbage-collects a *named* volume, so `<workspace>-pixi` and
`dind-var-lib-docker-*` used to outlive every workspace that made them (39 of
them, 37.28 GB, on the machine devlaunch#324 was measured on).

Volumes left by workspaces deleted before that fix are still there, and so are
any whose removal Docker refused. To see what has piled up:

```bash
docker system df -v      # under Local Volumes, LINKS 0 means no container uses it
```

Cross-check a name against `devpod list` before removing it with `docker volume rm`:
a volume belonging to a live workspace shows `LINKS 1`.
## Going back to the Python build

`0.1.0` is the Rust rewrite: same commands, same cache, same `metadata.json`, different
implementation (the whole of it is in [docs/rust-rewrite-plan.md](docs/rust-rewrite-plan.md), whose
divergence table is the complete list of what changed on purpose). The last Python release is
`0.0.29`, and nothing in the cache stops you going back to it:

```bash
pixi global install --channel conda-forge --channel https://prefix.dev/blooop "devlaunch<0.1"
pip install "devlaunch<0.1"
```

Both builds read and write the same `metadata.json` at schema 2 under the same locks, so a downgrade
keeps every workspace you already have and needs no cleanup. Please open an issue for whatever sent
you back.

