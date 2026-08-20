# Agent Instructions

## Development Environment

This project uses a devcontainer with pixi for environment management.

### Available Tools

- **GitHub CLI (`gh`)**: Available via `pixi run gh` or directly if using a login shell. Authentication comes from the host: `dl` forwards the host's token into every workspace it opens as `GH_TOKEN`, and this project's devcontainer also mounts `~/.config/gh` for containers started some other way (VS Code, plain `devpod up`).

### Running Commands

When using pixi tasks, prefer `pixi run <task>`. See `pixi task list` for available tasks.

For tools installed as dependencies (like `gh`), you can run them via:
- `pixi run gh <args>` - works in any shell
- `gh <args>` - works in login shells (`bash -l -c '...'`)

## Two installs: `dl` and `dl-next`

`dl` and `aid` on PATH are the **released** build, installed by pixi global from
the `blooop` channel (`~/.pixi/bin/dl`). Leave them alone: they are what keeps
working while this checkout is mid-change, and what actually opens the user's
workspaces.

`dl-next` and `aid-next` are **this working tree**, compiled by `./dev.sh` into
`~/.local/share/devlaunch-dev/bin` and symlinked from `~/.local/bin`. Both builds
are on PATH at once under names that cannot collide, so running the wrong one is
not possible by accident.

```
./dev.sh
```

Three things to know about it:

- **It is a compile, so there is a build step and there is a snapshot.**
  `dl-next` is the tree as it was the last time you ran `./dev.sh`, and it moves
  at no other time — a half-finished edit is invisible until it compiles, and an
  edit that compiles is invisible until you re-run the script. (This is the same
  trade `wf-next` in blooop/wayfinder makes, and the opposite of the one the
  Python build used to make: no build step, but equally no snapshot, so a
  half-saved edit was live immediately.) The copies live under
  `~/.local/share/devlaunch-dev/bin` rather than being symlinks into
  `rust/target/release/`, which is what makes that promise true: an ordinary
  `cargo build --release` in the tree would otherwise move `dl-next` underneath
  you, and `cargo clean` would delete it.
- **`--version` says which build it is.** `dl-next --version` prints
  `dl <version>-dev`, where the released `dl --version` prints the bare version.
  The `-dev` comes from the `dev-build` cargo feature that `./dev.sh` builds with
  (`rust/dl/Cargo.toml`); it is off in every artifact that ships. So the two names
  are told apart by output as well as by name, which is the whole reason to have
  two names.
- **It touches real state.** `dl` mutates `metadata.json`, the bare clone cache
  and live devpod workspaces, which is how a half-finished change costs someone
  their workspace list. Everything it stores resolves through `XDG_CACHE_HOME`,
  so point that at a scratch directory when the change being tested is anywhere
  near storage:

  ```
  XDG_CACHE_HOME=/tmp/dl-scratch/cache dl-next owner/repo
  ```

  **Only that one variable, and it is a trade rather than a free simplification.**
  Scoping `XDG_CONFIG_HOME` too would guard one more thing — a personal
  `config.toml` under it can pin `repos_dir` back at the real cache and beat
  `XDG_CACHE_HOME` outright, which is why `test/conftest.py` scopes both — but it
  also hides the host's `gh` login from `gh auth token`, so every workspace opens
  with no GitHub credentials. The credential loss happens on every run; the
  `repos_dir` hazard needs a `config.toml` most hosts do not have.

  That isolates the bookkeeping, not the machine — `dl` drives devpod and docker
  on the host either way, so the workspaces a scratch run creates are real ones
  that `devpod list` shows and that need deleting like any other.

  One thing the scratch cache now does cover: **`dl --purge`**. It deletes only
  workspaces whose source is a clone under devlaunch's cache directory, so a run
  pointed at a scratch cache finds every real workspace unrecognised and leaves
  it standing, naming what it left. It used to delete every workspace `devpod
  list` returned, which no `XDG_*` variable could scope — `devpod list` reads
  `~/.devpod`. `DEVPOD_HOME` is still what scopes devpod itself, and is what
  `test/conftest.py` sets for the suite.

  **`dl --prune`** is scoped by the same variable and more narrowly: it removes
  clone directories under `repos_dir` and never touches a devpod workspace at
  all, so a scratch cache leaves it with nothing to find. It prints its plan and
  asks before removing anything, and `-y` is what skips the question — so a
  scratch run of it is a read-only way to see the classification.

### Inside the devcontainer: one build, and it is `pixi run dl`

Everything above is about the host. This repo's devcontainer carries the same Rust
toolchain the binaries ship from — see "cargo is in the project env too" below —
so the container can build the tree itself. **Inside, run `pixi run dl` and
`pixi run aid`.** They are `cargo run` over this working tree, and `--version`
says so: `dl <version>-dev`.

They are pixi *tasks* now, not commands that happen to be in the environment.
That is worth knowing because it changes what a bare `dl` means in here: nothing.
There is no released `dl` in the container and nothing puts one on PATH — which is
correct, because on the host `dl` is the way in, and inside you are already in.

`pixi run` is not a style preference there, because `dl` shells out to a bare
`devpod` resolved from `PATH` and *which* devpod that finds depends on how the
container was opened. Open it with devpod — which is what `dl` does — and devpod
injects its own agent binary at `/usr/local/bin/devpod`, a working CLI sitting on
the bare `PATH`. Open the same devcontainer through VS Code or a plain
`devcontainer up` and nothing puts it there. A devlaunch run outside the project
env therefore has a devpod to drive on one route in and none on the other, which
is worse to diagnose than never working. The project env's devpod is there either
way, and it is the version the tree is pinned against rather than whatever the
host happened to inject — a difference that has already hidden a bug once, since
devpod 0.8 asks for a pty on `ssh --command` and 0.26 never does.

Note what choosing the project env does *not* buy: both binaries read the same
`~/.devpod`, so the workspace list and the configured providers are shared. The
project env is where the code and its devpod agree, not an isolation boundary.

### cargo is in the project env too, with nothing to install first

`cargo`, `rustc`, `clippy` and `rustfmt` come from the same `default` environment
(`[tool.pixi.feature.rust.dependencies]` in `pyproject.toml`), pinned to the
1.97.1 that `rust/rust-toolchain.toml` names and `ci.yml` installs.
`postCreateCommand`'s `pixi install` is what puts them there, so they are present
the moment the container comes up: **no toolchain step, no `rustup`, nothing to
fetch.** A change to the crates can be built and tested **in here** rather than by
pushing it and reading CI, which is all that was available before:

```
cd rust
pixi run cargo test --workspace
pixi run cargo clippy --locked --all-targets -- -D warnings
pixi run cargo fmt --check
```

`pixi run` keeps the directory it was called from, so the `cd rust` is what points
cargo at the workspace — the pixi manifest is found by searching upward and does
not have to be the directory you stand in. Note that a conda `rust` does not read
`rust-toolchain.toml` (that file is rustup's), so the version is named again in
`pyproject.toml` and moves by hand with the other two.

**Do not run `./dev.sh` in the container.** It exits at its first check, because
`cargo` is on PATH only inside the pixi environment — and that refusal is correct
rather than a gap to fill. Run it under `pixi run` and it would install a second
compiled copy of the *same* tree, printing the *same* `-dev` version string as
`pixi run dl` — the same build under a second name. On the host the convention is
worth its keep because the two names stand for genuinely different builds,
released and working tree; in here there would be nothing to tell apart, and
nothing gained by telling it. When the tree is half-edited, `pixi run dl` fails
to compile and says why, which is the whole of what a compiled build does for you
there; the host's released `dl` — the build that opened this container — is one
level up, untouched.

## The devcontainer is prebuilt

Opening this repo's devcontainer pulls `ghcr.io/blooop/devlaunch-devcontainer`
rather than building it. `.devcontainer/devcontainer.json` declares it under
`customizations.devpod.prebuildRepository`, and `devpod up` looks for one exact
tag — a hash of the build config, the build context and the target architecture —
before it builds anything. A miss is silent and falls through to a local build.

Four rules follow from that, and all of them are about the hash:

- **The build context is `.devcontainer`, deliberately.** Widening it back to the
  repository root makes the tag move on every commit to any file, which is a
  prebuild that never matches again. Nothing in the Dockerfile copies from the
  context, so there is no reason to widen it.
- **Changing anything under `.devcontainer/` invalidates the prebuild** — the
  Dockerfile and the `claude-code` feature's scripts included, since they live
  inside the context. `.github/workflows/devcontainer-prebuild.yml` republishes
  on pushes to `main` that touch that directory; its path filter is exactly the
  hash's inputs, so keep the two in step. Until it runs, that commit builds
  locally.
- **Each architecture is a tag of its own**, so the workflow is a matrix over
  `ubuntu-latest` and `ubuntu-24.04-arm` and nothing merges a multi-arch
  manifest. Neither leg passes `--platform`: the arch in the hash is the one the
  driver reports, which is the runner's, and on the lookup side `devpod up`
  passes no platform either. Two things hang off that. The pixi workspace has to
  declare `linux-aarch64` or the arm64 leg dies at `pixi install` — and so does
  an arm64 container's `postCreateCommand`. And `latest` belongs to amd64 alone:
  it is `build.cacheFrom`'s target, devpod arch-qualifies no alias, so a second
  leg passing it would race for a tag whose value decides whether a cache serves
  layers. arm64 publishes `latest-arm64`.
- **The hash covers the recipe, not what the recipe pulls.** The base image tag,
  the `docker-in-docker` major and the `claude-shim` the local feature installs
  all float, so one `.devcontainer/` tree can publish two different images. That
  is deliberate: **do not pin `claude-shim` here.** It carries no `claude` — the
  binary is downloaded on first run — a pin freezes it harder than no pin does,
  and the shipping implementation installs the same package unversioned into
  every workspace `dl` opens (`rust/devlaunch-core/src/flows/provision.rs`),
  which a unit test holds this spec against. The whole argument, with the
  measurements, is under "What the prebuild tag does not promise" in README.md.

`pixi run devcontainer-prebuild` publishes by hand after `docker login ghcr.io`,
for the architecture of the machine it runs on; the alias is its one argument and
defaults to `latest`.

The package must be public or the lookup returns DENIED and every launch silently
builds locally. It came up public on its own — GHCR gave it the visibility of the
public repository that published it — so there is nothing to do, but it is worth
checking rather than trusting, because a private package fails at nothing. See
"The prebuilt dev container image" in README.md for the check.

## Documentation Maintenance

- **Keep README up to date**: When modifying CLI commands, flags, or usage patterns, update the README.md to reflect the current tool behavior. Run `pixi run dl --help` to see the current help output and ensure the README matches.
