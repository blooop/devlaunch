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

  **Only that one variable, and there is nothing left behind it.** A personal
  `config.toml` used to be able to pin `repos_dir` back at the real cache and beat
  `XDG_CACHE_HOME` outright; that key is retired (#467) and the clone root is
  derived from the cache directory, so one variable now scopes the whole of what
  `dl` stores. `test/conftest.py` still scopes `XDG_CONFIG_HOME` as well, which is
  belt-and-braces rather than a load-bearing guard, and a scratch run leaves it
  alone deliberately: scoping it hides the host's `gh` login from `gh auth token`,
  so every workspace would open with no GitHub credentials.

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
  clone directories under `<cache>/repos` and never touches a devpod workspace at
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
  measurements, is under "What the prebuild tag does not promise" in docs/development.md.

`pixi run devcontainer-prebuild` publishes by hand after `docker login ghcr.io`,
for the architecture of the machine it runs on; the alias is its one argument and
defaults to `latest`.

The package must be public or the lookup returns DENIED and every launch silently
builds locally. It came up public on its own — GHCR gave it the visibility of the
public repository that published it — so there is nothing to do, but it is worth
checking rather than trusting, because a private package fails at nothing. See
"The prebuilt dev container image" in docs/development.md for the check.

## Documentation Maintenance

- **Keep README up to date**: When modifying CLI commands, flags, or usage patterns, update the README.md to reflect the current tool behavior. Run `pixi run dl --help` to see the current help output and ensure the README matches.

- **README orients, `docs/` explains.** The README is deliberately short: what
  `dl` is, the quickstart, the command tables, and one environment-variable
  table. Depth lives in `docs/` (`cli.md`, `workspaces.md`, `workspace-tools.md`,
  `cleanup.md`, `performance.md`, `development.md`) and is linked from the README's
  Docs table. A new paragraph of design rationale belongs in the docs page for its
  topic, not in the README. Adding a flag still means naming it in the README,
  because `test/test_readme_cli_doc.py` requires every flag `dl --help` offers to
  appear there.

- **Some guards read a specific document.** `test_readme_cli_doc.py` reads the
  README; `test_bench_doc.py` reads `docs/performance.md`;
  `test_public_api_snapshots_doc.py` reads `docs/development.md`; and the Rust
  `flows::provision::lending_contract` module reads `docs/workspace-tools.md`,
  matching on headings. Moving one of those sections between files means moving
  the path in its guard, in the same change.

- **A citation spelled as a path has to point somewhere.** Comments all over
  `rust/` name the test that pins the behaviour they describe, which is what
  makes the record worth reading. `test/test_citations_resolve.py` holds every
  one of those names to being a file: a path is checked as written, a bare
  filename against the test tree. Retiring the Python implementation (#267)
  broke about sixty of them at once, which is why the rule exists. When a suite
  is gone, name it without the extension and say it retired, the way the
  comments now do: `test_purge_ownership` (Python, retired in #267). The
  history stays greppable and stops impersonating a live guard. `CHANGELOG.md`
  and `docs/rust-port-scope.md` are out of scope as records of what was true
  when they were written.

- **No em or en dashes in the README or the `docs/` pages it links.** They read
  as machine-written. Use a full stop or a comma, or end the sentence, and write
  a numeric range as "18s to 28s". `test/test_docs_prose.py` asserts it and
  derives the scope by globbing `docs/`, so a new page is covered as soon as it
  exists. The two archival planning documents (`rust-rewrite-plan.md`,
  `rust-port-scope.md`) are the one hand-written exclusion: they record a port
  that has already happened, and rewriting them to a style rule would edit a
  record.

- **A page under `docs/` is not the README.** Six sentences survived the split
  still saying "the rest of this README" or "the figures above are this README's"
  from inside a docs page. Same test guards the phrase.

## Standing rules: second copies of a fact, and when a review happens

- **A second hand-maintained copy of a fact is allowed only if a test named
  beside it diffs it against the first.** The 2026-08 review found six copies of
  one fact and three had already drifted: the two devpod fakes, `dl.bash`'s
  tables, CI's test list. No generator is required and none is mandated —
  `dl.bash` stays hand-written for that reason, since `clap_complete` cannot
  express the dynamic half and a generator erases the comments that explain why
  owners win over ids. The rule lives here as one paragraph and not as a register
  under `docs/`, because a register is itself a second copy of a fact and would
  need a guard of its own. Enforcement is nothing new: the guards that already
  exist (`completion_tables`, `public-api`, the doc tests) are the enforcement,
  and a reviewer who notices a new copy asks for a diff test beside it. A prek
  hook or a per-PR gate was weighed and refused — one person writes and breaks
  this rule, so the same attention enforces it either way, and the tax is real
  while the miss rate is not.

- **An architecture review is chartered as a wayfinder map when the reviewer
  judges enough has changed, never as a per-PR gate.** No schedule triggers it,
  because the charter month produced three reviews unprompted and the trigger was
  never the missing part. The mechanism that does run every time is smaller: the
  fresh-context adversarial review of each PR by an agent that did not write it,
  which is where the last three real defects were caught — #427's layering
  violation, #415's four-hole guard, #428's dropped `Absent`.
