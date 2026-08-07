# Rust rewrite feasibility — facts

Research for [#52](https://github.com/blooop/devlaunch/issues/52) (map: [#51](https://github.com/blooop/devlaunch/issues/51)).
Facts only; the go/no-go decision belongs to #53. Version numbers and statuses are as of 2026-08-07.

## Facts most relevant to the go/no-go

- **Every crate devlaunch needs exists and is maintained** — clap for CLI, skim 5.x (actively released, July 2026) as an embeddable fzf-alike, `std::process::Command` maps 1:1 onto the current list-form `subprocess.run` usage, `toml`/`serde_json` cover config/state.
- **The one ecosystem gap is dynamic shell completion**: clap_complete's native/dynamic completions are still behind an `unstable-dynamic` feature flag with an open tracking issue and known bash formatting bugs. This gap is moot for devlaunch: the existing 158-line bash completion never invokes Python — it sources a cached `completions.bash` file — so it carries over unchanged and keeps its ~3ms hot path.
- **No distribution channel must be lost.** maturin's `bin` bindings ship a Rust binary as a PyPI wheel (`pip install` keeps working — uv and prek both do exactly this), and rattler-build has a first-class Rust tutorial for the prefix.dev channel the project already publishes to. The conda package changes from `noarch: python` to per-platform compiled builds.
- **Port scope**: ~2.6K LOC of Python, most of which is list-form subprocess calls, regex spec parsing, and serde-shaped dataclasses that translate mechanically. The dominant rewrite cost is the test suite: 413 tests lean heavily on `unittest.mock`/monkeypatch at subprocess seams, which have no direct Rust equivalent and would be rebuilt (trait seams or PATH-shim fake binaries).
- **Prior art (prek, the closest analogue — a Rust rewrite of the pre-commit git-hook wrapper) reports**: single binary with no Python runtime, faster and lighter on disk, and it kept PyPI, conda, cargo, npm, and installer channels simultaneously. uv/ruff's headline 10–100x speedups came from replacing CPU-bound Python work; devlaunch's runtime is dominated by `devpod`/`git` child processes, which a rewrite does not speed up — the gain would be limited to process startup and distribution.

---

## 1. Ecosystem fit

### CLI parsing + shell completions: clap + clap_complete

- `clap_complete` 4.6.9 (released 2026-08-06) generates **static** completion scripts for Bash, Zsh, Fish, Elvish, PowerShell via the stable `Shell` enum; this part is stable. [docs.rs/clap_complete](https://docs.rs/clap_complete/latest/clap_complete/)
- **Dynamic (native) completions exist but are unstable**: the `engine` and `env` modules are gated behind the `unstable-dynamic` feature flag. The mechanism is `CompleteEnv` — the shell calls back into the binary with `COMPLETE=bash your_program`, registered via `source <(COMPLETE=bash your_program)`. Bash, Zsh, Fish, Elvish, PowerShell adapters exist. The docs warn the shell-code/binary interface is unstable and recommend re-sourcing on upgrade rather than caching the generated script. [docs.rs/clap_complete/env](https://docs.rs/clap_complete/latest/clap_complete/env/index.html)
- The stabilization tracking issue [clap-rs/clap#3166](https://github.com/clap-rs/clap/issues/3166) is **still open**, with bash-specific known issues (output formatting as a single completion rather than a list) and no stated timeline.
- **Relevance to devlaunch**: the current completion hot path never runs Python. `devlaunch/completions/dl.bash` (158 lines) parses `COMP_LINE` in pure bash and `source`s `~/.cache/devlaunch/completions.bash`, which `dl` regenerates in the background. That script and cache format are language-agnostic — a Rust `dl` only needs to keep writing the same `DL_WORKSPACES=...`/`DL_REPOS=...` cache file. clap_complete's dynamic completion is therefore not on the critical path at all; devlaunch would not need it.

### Fuzzy selection

- **skim** ([skim-rs/skim](https://github.com/skim-rs/skim)) is explicitly usable as a library ("Use as a Library" README section; `SkimOptionsBuilder`, `Skim::run_with`, custom `SkimItem` trait, optional tokio async). Maintenance is active: version 5.6.1 published 2026-07-27, with 30+ releases across 2025–2026 ([crates.io/crates/skim](https://crates.io/crates/skim)). It takes over the terminal like fzf, matching devlaunch's current `iterfzf` interaction ([docs.rs/skim](https://docs.rs/skim/latest/skim/)).
- **nucleo** ([helix-editor/nucleo](https://github.com/helix-editor/nucleo)) is a fuzzy **matcher** library (same scoring as fzf, claims ~6x faster matching than skim), maintained by the Helix editor team — but it is not a managed interactive picker; its README directs matcher-only users to `nucleo-matcher` and notes the high-level API still anticipates changes. Using it for `dl`'s selector would mean building the TUI loop separately.
- **Shelling out to fzf** remains an option and is closest to today's behavior: the current Python code uses `iterfzf`, which drives an fzf binary. A Rust port could spawn `fzf` the same way, at the cost of a runtime dependency on the fzf binary; skim removes that dependency by embedding the picker.

### Subprocess orchestration

- devlaunch's subprocess usage is uniformly list-form `subprocess.run([...], capture_output=..., timeout=...)` (see `run_devpod`, `_git_ls_remote` in `devlaunch/dl.py`). This maps 1:1 onto `std::process::Command` (`.arg()`/`.args()`, `.output()` for capture, `.status()` for passthrough) — no shell interpolation is used anywhere, so nothing needs `sh -c`. Timeouts are the one gap: `std::process` has no built-in timeout, requiring `wait_timeout` or a helper crate.
- **duct** 1.1.1 (published 2025-11-09, 34M+ downloads, actively maintained by Jack O'Connor) provides pipelines, capture, and expression-style composition ([crates.io/crates/duct](https://crates.io/crates/duct), [oconnor663/duct.rs](https://github.com/oconnor663/duct.rs)).
- **xshell** stable 0.2.7 (2024-11-16, 10M+ downloads) with 0.3.0 pre-releases (Dec 2024); macro-based `cmd!` ergonomics aimed at shell-script-like code ([crates.io/crates/xshell](https://crates.io/crates/xshell)).
- Given devlaunch never pipes processes into each other, plain `std::process::Command` plus a small `run_devpod`-style helper covers the entire surface; helper crates are optional ergonomics.

### Config/state: TOML + JSON

- **toml** 1.1.4 (2026-07-28) is the standard serde-compatible TOML crate — direct parity with the current `tomli` read / `tomli_w` write of `~/.config/devlaunch/config.toml` ([docs.rs/toml](https://docs.rs/toml/latest/toml/)). Note the current Python code also does not preserve user formatting on write, so plain `toml` is parity; if format-preserving writes were ever wanted, **toml_edit** 0.25.13 (2026-07-14) preserves comments, whitespace, and item order ([docs.rs/toml_edit](https://docs.rs/toml_edit/latest/toml_edit/)).
- **serde_json** 1.0.151 covers the completion cache (`completions.json`) and `devpod ... --output json` parsing, both as typed structs (`#[derive(Deserialize)]` replaces `Workspace.from_json`) and untyped `Value` ([docs.rs/serde_json](https://docs.rs/serde_json/latest/serde_json/)).
- The dataclass `to_dict`/`from_dict` pairs in `devlaunch/worktree/models.py` (Path and datetime field massaging) become serde derive attributes; `chrono` or `time` with serde features handles the ISO-8601 timestamps.

## 2. Distribution trade-offs

### What a static Rust binary buys

- **Single-file install**: uv's announcement describes the pattern — "uv ships as a single static binary" with "no direct Python dependency, so you can install it separately from Python itself" ([astral.sh/blog/uv](https://astral.sh/blog/uv)). For devlaunch this removes the Python-interpreter prerequisite (currently `python >=3.10` plus `iterfzf` in both PyPI and conda requirements).
- **cargo-binstall** ([cargo-bins/cargo-binstall](https://github.com/cargo-bins/cargo-binstall)) installs prebuilt binaries by searching the crate's linked repository releases, falling back to the quickinstall host and finally to `cargo install` from source — GitHub-release binaries become installable with zero build.
- **Release automation**: cargo-dist / dist ([axodotdev/cargo-dist](https://github.com/axodotdev/cargo-dist)) generates GitHub Actions workflows that build per-platform binaries, tarballs, and shell/PowerShell installers and publish them as GitHub Releases. The repo is active (3,400+ commits) though it sits under the axodotdev org whose commercial arm wound down — worth checking release cadence before depending on it.

### Conda / prefix.dev via rattler-build

- **Yes** — rattler-build has an official Rust tutorial ([rattler-build.prefix.dev/latest/tutorials/rust](https://rattler-build.prefix.dev/latest/tutorials/rust/)): the recipe's build script runs `cargo install --locked --bins --root ${PREFIX} --path .`, requirements use `${{ compiler('rust') }}`, and `cargo-bundle-licenses` generates the THIRDPARTY.yml the docs say to ship.
- The project's existing pipeline already runs `prefix-dev/rattler-build-action` and uploads to the prefix.dev `blooop` channel (`.github/workflows/conda-publish.yml`), so the publishing half is unchanged. What changes: the current recipe (`conda.recipe/recipe.yaml`) is `noarch: python` — one package for all platforms. A Rust binary is a compiled, per-platform package, so CI would build one artifact per target platform instead of one universal package.

### PyPI: is `pip install devlaunch` lost?

- **Not necessarily.** maturin's `bin` bindings package a pure Rust binary into a Python wheel: "Binaries are packaged into the wheel as 'scripts' and are available on the user's `PATH`" ([maturin.rs/bindings](https://www.maturin.rs/bindings.html)). No PyO3 involvement — this is binary-in-a-wheel, not an extension module.
- This is exactly how uv ships: its `pyproject.toml` declares maturin as build backend with `bin` bindings, publishing the Rust binary to PyPI installable on Python 3.8+ ([astral-sh/uv pyproject.toml](https://github.com/astral-sh/uv/blob/main/pyproject.toml)). prek does the same: "prek is published as Python binary wheel to PyPI", installable via pip/uv/pipx ([j178/prek README](https://github.com/j178/prek/blob/master/README.md)).
- The cost is CI shape, not channel loss: instead of one pure-Python wheel from hatchling, the release builds a per-platform wheel matrix (maturin-generated GitHub Actions workflows are the standard route). The current `etils-actions/pypi-auto-publish` single-wheel flow (`.github/workflows/publish.yml`) would be replaced.

## 3. Effort signal (from the code at HEAD)

### Inventory

| Component | LOC | Character |
|---|---|---|
| `devlaunch/dl.py` | 1291 | argv dispatch, spec-parsing regexes, completion cache, devpod wrappers, main flow |
| `devlaunch/worktree/` (6 files) | 1206 | bare-repo clone/fetch (`repo_manager` 297), branch ops (`branch_manager` 237), workspace clones + LFS (`workspace_clone` 359), JSON metadata (`storage` 131), TOML config (116), dataclass models (66) |
| `devlaunch/completion.py` + loader | 112 | installs the bash block into rc files |
| `devlaunch/completions/dl.bash` | 158 | pure-bash completion; sources cached `completions.bash` |
| Tests | 413 test functions | heavily mock/monkeypatch-based on subprocess seams, plus docker/e2e/integration suites |

### Ports ~1:1

- **All subprocess plumbing**: every call is list-form with no shell interpolation — `Command::new("devpod").args(...)` is a mechanical translation. The 5s/2s timeouts on `git ls-remote`/`for-each-ref` need a `wait_timeout`-style helper (std has none).
- **Spec parsing**: `OWNER_REPO_PATTERN`, `GIT_URL_PATTERNS`, and the URL-expansion logic move to the `regex` crate essentially verbatim.
- **Completion cache**: JSON via serde_json; the `completions.bash` writer is string formatting. Atomic write-then-rename is `std::fs::rename` (same POSIX semantics).
- **The bash completion script and its cache format carry over byte-identical** — `dl.bash` never executes `dl`; it sources the cache file. Nothing about the completion UX changes.
- **Models and metadata**: `BaseRepository`/`WorktreeInfo` dataclasses → serde-derive structs; the manual `to_dict`/`from_dict` Path/datetime conversion disappears into serde attributes.
- **Worktree managers**: `RepositoryManager`/`BranchManager`/`WorkspaceCloneManager` are sequences of git commands with path bookkeeping — same structure in Rust.

### Needs redesign

- **Argument handling**: `main()` is a hand-rolled positional dispatcher with a custom `extract_devcontainer_flag` scanner that respects `--` pass-through. Porting to clap means reshaping this (clap's `trailing_var_arg`/`allow_hyphen_values` cover the `--` case); keeping the hand-rolled scanner is also viable since it is only ~30 lines.
- **Fuzzy selector**: `iterfzf` → either the skim library (new API, drops the fzf binary dependency) or spawning `fzf` (same behavior, keeps the dependency).
- **Self-respawn**: `update_cache_background()` re-executes `sys.executable -m devlaunch.dl --update-cache`; the Rust version respawns via `std::env::current_exe()` — simpler, but a behavioral seam tests currently mock.
- **Error handling**: pervasive broad `try/except (OSError, SubprocessError)` returning `None`/`[]` becomes `Result`/`Option` plumbing (`anyhow` or `thiserror`); mechanical but touches every function signature.

### What the test suite implies

- The 413 tests are dominated by `unittest.mock`/`monkeypatch` patching at Python-level seams (`subprocess.run`, module functions). **These mocks do not port** — Rust has no runtime monkeypatching; the equivalents are (a) injecting a command-runner trait, or (b) PATH-shim fake `devpod`/`git` executables, both of which mean rewriting the harness, not translating tests.
- The tests' durable value is as a **behavior spec**: they pin exact devpod argument sequences (`up --id ... --ide none --init-env DEVLAUNCH_WORKSPACE_ID=...`), workspace-ID sanitization rules, and cache formats — precisely the invariants a port must preserve.
- The `test/docker`, `test/e2e`, and `test/integration` suites exercise real git (and devpod) and are language-agnostic at the CLI boundary; they carry over as the port's acceptance harness with minimal change.
- Net: application code is a largely mechanical ~2.6K-LOC translation; the test harness is the redesign-heavy part of the port.

## 4. Prior art

- **prek** ([j178/prek](https://github.com/j178/prek)) — the closest analogue: a Rust rewrite of pre-commit, a Python devtool wrapper that orchestrates git and subprocesses, by an outside author. Its README claims: "A single binary with no dependencies, does not require Python or any other runtime"; "Faster than `pre-commit` and more efficient in disk space usage" (shared hook environments instead of per-repo duplication). Adoption listed includes CPython, FastAPI, Godot, Home Assistant. Distribution kept **every** channel simultaneously: PyPI binary wheel, conda, Homebrew, Nix, cargo/cargo-binstall, npm, standalone installers, GitHub releases ([README](https://github.com/j178/prek/blob/master/README.md)).
- **uv** ([astral.sh/blog/uv](https://astral.sh/blog/uv)) — reported "8-10x faster than `pip` and `pip-tools` without caching, and 80-115x faster when running with a warm cache", shipping as "a single static binary", still pip/pipx-installable. Note the announcement attributes the gains to a global module cache, Copy-on-Write/hardlink installs, and resolver engineering — i.e., replacing CPU- and I/O-bound Python work, not merely changing language.
- **ruff** ([astral-sh/ruff README](https://github.com/astral-sh/ruff)) — "10-100x faster than existing linters (like Flake8) and formatters (like Black)", with user-reported extremes (250K-LOC pylint run: 2.5 min → 0.4 s). Again a CPU-bound workload (parsing/linting every file in-process).
- **Transferability caveat (fact, not verdict)**: devlaunch's latency profile differs from all three — its commands spend their time inside `devpod` and `git` child processes, which a rewrite cannot speed up, and its completion hot path is already pure bash + a cached file. The measurable wins a rewrite offers here are the ones prek demonstrates (no interpreter startup on each `dl` invocation, no Python runtime prerequisite, single-file distribution), not uv/ruff-class throughput gains.
