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

`dl-next` and `aid-next` are **this working tree**, installed by `./dev.sh` into
`~/.local/share/devlaunch-dev` and symlinked from `~/.local/bin`. Both builds are
on PATH at once under names that cannot collide, so running the wrong one is not
possible by accident.

```
./dev.sh
```

Two things to know about it:

- **The install is editable, so there is no build step and no snapshot.**
  `dl-next` is whatever the tree looks like at the moment you run it — a
  half-finished edit is live as soon as it is saved. (`wf-next` in
  blooop/wayfinder is the same idea with the opposite trade: a compiled copy that
  only moves when you rebuild it.) `dl-next --version` names the tree it resolves
  to — `dl <version> (dev, editable from /path/to/checkout)` — where the released
  `dl --version` prints the bare version, so the two are told apart by output as
  well as by name.
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

Everything above is about the host. This repo's devcontainer already installs the
checkout editable — `pyproject.toml` declares
`devlaunch = { path = ".", editable = true }` under `[tool.pixi.pypi-dependencies]`,
and `postCreateCommand` runs `pixi install` — so the container comes up with
`./dev.sh`'s job already done. **Inside, run `pixi run dl` and `pixi run aid`.**
They are the working tree, and `--version` says so:
`dl <version> (dev, editable from /workspaces/<checkout>)`.

`pixi run` is not a style preference there, because `dl` shells out to a bare
`devpod` resolved from `PATH` and *which* devpod that finds depends on how the
container was opened. Open it with devpod — which is what `dl` does — and devpod
injects its own agent binary at `/usr/local/bin/devpod`, a working CLI sitting on
the bare `PATH`. Open the same devcontainer through VS Code or a plain
`devcontainer up` and nothing puts it there. A devlaunch installed outside the
project env therefore has a devpod to drive on one route in and none on the
other, which is worse to diagnose than never working. The project env's devpod is
there either way, and it is the version the tree is pinned against rather than
whatever the host happened to inject — a difference that has already hidden a bug
once, since devpod 0.8 asks for a pty on `ssh --command` and 0.26 never does.

Note what choosing between them does *not* buy: both binaries read the same
`~/.devpod`, so the workspace list and the configured providers are shared. The
project env is where the code and its devpod agree, not an isolation boundary.

**Do not run `./dev.sh` in the container.** It exits at its first check, because
`uv` is not installed there — and that refusal is correct rather than a gap to
fill. A `-next` build would be a second editable install of the *same* tree,
printing the *same* provenance string as `pixi run dl` — the same build under a
second name. On the host the convention is worth its keep because the two names
stand for genuinely different builds, released and working tree; in here there
would be nothing to tell apart, and nothing gained by telling it. No released
`dl` is wanted in there either: the host keeps one because on the host `dl` is
the way in, and inside you are already in. When the tree is
half-edited, `git stash` restores a working `pixi run dl` with no reinstall, and
the host's released `dl` — the build that opened this container — is one level up,
untouched.

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
