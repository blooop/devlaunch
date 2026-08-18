# Port inventory at 0.0.27 — facts

Research for [#249](https://github.com/blooop/devlaunch/issues/249) (map: [#248](https://github.com/blooop/devlaunch/issues/248)).
Facts only; the decisions belong to #251–#255.

Measured 2026-08-18 against `origin/main` at commit `54d828009a8b6e8a4ee850e0cef7bde427baf18b` (the v0.0.27 release merge, 2026-08-18). Successor to
[rust-rewrite-feasibility.md](https://github.com/blooop/devlaunch/blob/research/rust-rewrite-feasibility/docs/research/rust-rewrite-feasibility.md)
(measured 2026-08-07, pre-0.0.11: ~2.6K source LOC, 413 test functions) and to
[docs/rust-port-scope.md](../rust-port-scope.md) (2026-08-10, at 0.0.24: 7,373 source lines, 61 mock-free tests).
File:line references below are at `54d8280` unless stated.

## Headline numbers

| | feasibility doc (2026-08-07) | v0.0.11 (2026-08-08) | port-scope note (0.0.24) | HEAD = v0.0.27 (2026-08-18) |
|---|---|---|---|---|
| `devlaunch/` Python LOC | ~2,600 | 4,016 | 7,373 | **11,175** (26 files) |
| Python modules | 4 + worktree/6 | 17 files | 21 modules | **26 files** (10 new since 0.0.11) |
| test/ LOC | — | — | 18,039 | **30,362** |
| test functions | 413 | 657 | — | **1,514** |
| mock-free acceptance candidates | — | — | 61 (per #53) | **96 boundary + 216 in-process mock-free** (§2) |

The span v0.0.11..v0.0.27 is 298 commits touching 122 files, +33,915/−2,049 lines (`git diff --stat v0.0.11..v0.0.27`).

## 1. Source inventory by module

LOC is `wc -l` per file at `54d8280`. Classification is against the feasibility doc's three buckets.

### Shipped package (`devlaunch/`, 11,175 Python lines + 173 bash)

| module | LOC | vs 0.0.11 | classification | notes |
|---|---|---|---|---|
| `dl.py` | 4,909 | 1,592 (3.1×) | mostly **ports ~1:1**; dispatch/flows **need design** | Still a hand-rolled positional dispatcher (`main()` at dl.py:4409; no argparse in dl.py — argparse appears only in `devpod_provider.py:137`'s standalone probe). New since 0.0.11: fast attach, `up`, `--reconcile`, `--prune`/`--purge` reporting, per-workspace launch locks, stored devpod workspace ids. Subprocess plumbing, cache writing, JSON parsing translate mechanically; the launch flow's lock/probe/attach sequencing is the design-bearing part. `iterfzf` selector unchanged (dl.py:3399) — same skim-vs-spawn-fzf choice as before. |
| `tools.py` | 1,112 | new | **needs design** (composition); embedded scripts **stay language-agnostic** | Tool provisioning: lends the host's `gh`/`claude`, installs `zellij`, shares one host pixi-cache directory into every container. Generates POSIX shell scripts sent through `devpod ssh --command` as `bash -lc <quoted script>` (tools.py:970) and streams host binaries with a real file on the child's stdin, not a pipe (tools.py:983-985, dl.py run_devpod `stdin_file`). The generated scripts are strings and carry over verbatim; the composition/quoting layer needs a `shlex`-equivalent (§3). |
| `worktree/workspace_clone.py` | 808 | 381 | **ports ~1:1** | Workspace clones hardlinking out of the bare cache; git-LFS pointer probing is file-content sniffing (`_is_lfs_pointer`, workspace_clone.py:201) plus `git lfs` subprocess calls — no LFS library involved. |
| `worktree/repo_manager.py` | 640 | 297 | **ports ~1:1** | Bare-clone cache, fetch, recovery of half-removed clones. Git subprocess sequences. |
| `workspace_state.py` | 446 | new | **ports ~1:1** | "What a workspace holds" for `dl --ls --json` / `rm` refusal: parses `git status --porcelain`-family output via one `subprocess.run` (workspace_state.py:277). |
| `timing.py` | 410 | new | **needs design** (mild) | Env-gated wall-clock spans, `DEVLAUNCH_TIMING=1|json`, module-global registry reported at process end. Rust: `std::time::Instant` + explicit registry (`OnceLock`/thread-local); no crate gap, but the global-state shape doesn't translate verbatim. |
| `worktree/storage.py` | 392 | 361 | **ports ~1:1** | metadata.json read/rewrite under lock; serde_json. |
| `worktree/migration.py` | 305 | 264 | **needs design** — whether to port at all | One-time migration of pre-0.0.11 caches onto the current id scheme. A fact for the port scope decision: a Rust binary either carries Python-era migrations or declares a floor. |
| `disk_usage.py` | 275 | new | **ports ~1:1** | Exclusive (would-free) usage: `st_blocks`-based sizes, hardlink dedup keyed on `(st_dev, st_ino)` with `st_nlink` (disk_usage.py:113,182). `std::os::unix::fs::MetadataExt` covers all of it. |
| `aid.py` | 243 | 212 | **ports ~1:1** | Rewrites its argv into a `dl` command line; `shlex.join` for logging only (aid.py:167,235). |
| `workspace_id.py` | 223 | 222 | **ports ~1:1** | Sanitization rules, regex. |
| `gh_auth.py` | 217 | 171 | **ports ~1:1** | `gh auth token` subprocess; token travels via private file, never argv. |
| `worktree/branch_manager.py` | 211 | 188 | **ports ~1:1** | Git branch ops, 6 list-form `subprocess.run` sites. |
| `devpod_provider.py` | 156 | new | **ports ~1:1** | `devpod provider list --output json` parsing; injectable `run` callable defaulting to `subprocess.run` (devpod_provider.py:68-81) — the one place a runner seam is already explicit in the source. |
| `tty_session.py` | 144 | new | **ports ~1:1** | Builds the OpenSSH argv (`ssh -t <alias> <payload>`) that carries a pty into the workspace; workdir travels inside the payload as `cd <quoted> && cmd` (tty_session.py:142). Needs shell-word quoting (§3), not a PTY library — the pty is ssh's. |
| `devpod_ssh.py` | 143 | new | **ports ~1:1** | Recovers the remote exit status from devpod's wrapped error text; pure string/regex work. |
| `worktree/locks.py` | 142 | new | **ports ~1:1** with a crate (§3) | `fcntl.flock` LOCK_EX/LOCK_NB (locks.py:85,95,136); lock file never deleted; kernel-released on death. Semantics preserved by `fd-lock` or `rustix::fs::flock`. |
| `completion.py` + `completion_loader.py` | 112 | 112 | **ports ~1:1** | rc-file install of the bash block. |
| `worktree/config.py` | 95 | 106 | **ports ~1:1** | TOML config (`auto_fetch` knob removed in 0.0.27). |
| `worktree/models.py` | 88 | 88 | **ports ~1:1** | serde-shaped dataclasses. |
| `xdg.py` | 47 | new | **ports ~1:1** | Single shared answer for XDG config/cache homes. |
| `worktree/git_errors.py` | 35 | new | **ports ~1:1** | One function shaping "what a failed `git <verb>` gives a caller", now naming exit codes. |
| `completions/dl.bash` | 173 | 173 | **stays language-agnostic** | Unchanged story: pure bash, sources the cached `completions.bash`, never executes `dl`. |

### Not shipped code — out of port scope

| item | LOC | what it is |
|---|---|---|
| `scripts/bench_launch.py`, `scripts/bench_points.py` | 316 + 310 | Launch-latency benchmarking: drive `dl` as a subprocess over its CLI and record JSON. Dev tooling, language-agnostic at the CLI boundary; not part of the shipped binary. |
| `scripts/*.sh` (`bench_cold_reset.sh`, `setup_host.sh`, `launch_vscode.sh`, `rename_project.sh`, `update_from_template.sh`) | — | Shell tooling; stays as-is. |
| `docs/conf.py`, `example/example.py` | — | Sphinx config and example; docs tooling. |

So of the ~11.9K non-test Python lines on `origin/main`, **11,175 are shipped package code in port scope**; the remaining ~630 are `scripts/` benchmarking/dev tooling that stays language-agnostic.

## 2. Test-suite split at HEAD

1,514 test functions in `test/` (grep `def test_` at `54d8280`), 30,362 lines. Method: grep for `unittest.mock`, `monkeypatch`, `mocker`, patch decorators per file, then per-file inspection to separate *hard* seam-patching (`mock.patch`, `monkeypatch.setattr`, `MagicMock`) from monkeypatch used only for env/cwd isolation (`setenv`/`delenv`/`chdir`), which every test inherits anyway from `test/conftest.py`'s autouse fixtures (conftest.py:41-122). Files verified by reading include `test/integration/test_lfs_object_sharing.py` (monkeypatch = one `GIT_CONFIG_GLOBAL` setenv, line 84 — real git underneath), `test/unit/test_tty_session.py` (env-var opt-out tests only), and `test/integration/test_lfs_probe_real.py` (real git-LFS but spies on `subprocess.run` via `mock.patch`, lines 83-84 — counted mock-based).

**The split: 312 mock-free vs 1,202 mock/patch-based.**

### Mock-free at the CLI/filesystem/process boundary — 96 tests in 13 files (the acceptance-harness candidates, successors of #53's "61")

| file | tests | boundary exercised |
|---|---|---|
| `test/e2e/test_full_workflow.py` | 9 | real devpod + Docker, `dl` end to end |
| `test/e2e/test_interactive_session.py` | 13 | real workspace named in `DEVLAUNCH_E2E_WORKSPACE`, PTY sessions |
| `test/e2e/test_claude_config_protection.py` | 1 | real container filesystem |
| `test/e2e/test_ssh_config_isolation.py` | 1 | real `~/.ssh/config` handling |
| `test/integration/test_repo_manager_real.py` | 15 | real git against local fixture repos |
| `test/integration/test_repo_manager_recovery.py` | 9 | real git, half-removed-clone recovery |
| `test/integration/test_clone_race.py` | 2 | two real processes racing a clone |
| `test/integration/test_clone_object_sharing.py` | 2 | hardlink sharing observed on disk |
| `test/integration/test_lfs_object_sharing.py` | 4 | real git-lfs objects on disk (env-only monkeypatch) |
| `test/test_concurrent_launches.py` | 5 | real second interpreters via `test/fixtures/subprocess_drivers.py` |
| `test/test_locks.py` | 9 | real flock across processes |
| `test/unit/test_locks.py` | 10 | real flock, single process |
| `test/test_pty_helpers.py` | 16 | real `pty.fork` children (tests the PTY harness itself) |

The harness infrastructure these ride on is itself mock-free: `test/fixtures/git_fixtures.py`, `e2e_helpers.py`, `e2e_guard.py`, `subprocess_drivers.py` (real second processes because "the flock in `worktree/locks.py` is per open file description… two threads share one file description", subprocess_drivers.py docstring), and `pty_helpers.py` (real `pty.fork` + `os.execvpe`, pty_helpers.py:93-100). `test/fixtures/devpod_mock.py` is the exception — a `unittest.mock.patch`-based fake devpod for Tier-2 tests.

### Mock-free but in-process — 216 tests

Pure-function or tmpdir-only tests with no patched seams: `test_workspace_id.py` (53), `test_tty_session.py` (22), `test_worktree_config.py` (13), `test_worktree_models.py` (7), `unit/test_spec_parsing.py` (7), `unit/test_devpod_ssh.py` (9), `unit/test_devcontainer_manifest.py` (11), `unit/test_claude_code_feature_mounts.py` (7), `unit/test_workspace_source_placement.py` (5), `unit/test_e2e_guard.py` (17), `unit/test_e2e_workspace_helper.py` (6). These port as ordinary Rust unit tests, not as an acceptance harness. Also in this bucket but **out of port scope**: the bench-tooling tests (`test_bench_points.py` 20, `test_bench_workflow.py` 15, `test_bench_doc.py` 7, `test_bench_record_schema.py` 5) and the doc-consistency guards (`test_lending_doc.py` 12) — 59 tests that pin `scripts/` and docs, not the shipped binary.

### Mock/patch-based behavior spec — 1,202 tests

Dominated by `test_dl.py` (325 tests, 569 hard-patch references), `unit/test_tools.py` (97), `test_workspace_clone.py` (65), `test_timing.py` (64), `test_workspace_state.py` (62), `unit/test_prune_orphaned_clones.py` (56), plus the spawn-count/fetch-count specs (`test_devpod_spawn_counts.py` 37, `test_cold_launch_fetches.py` 6, `test_repo_lock_cycles.py` 8) which patch `subprocess.run`/`Popen` to record exact argv sequences. As in the feasibility doc: these do not port as tests; they are the behavior spec a Rust suite re-pins at trait seams or PATH-shim fakes.

For scale against the predecessor: #53's "61" (e2e 8, integration 10, storage 20, models 7, config 7, spec-parsing 7, helpers 2) has grown to 96 true boundary tests — e2e 8→24, integration 10→32, plus three suites that did not exist (locks 19, concurrency drivers 5, PTY harness 16). The storage/models/config/spec-parsing tests the "61" also counted now sit in the 216 in-process bucket (storage moved: `test_worktree_storage.py` now patches seams — 3 hard references — and is counted mock-based).

## 3. Crate-mapping deltas

New needs beyond the 2026-08-07 survey (clap, skim, `std::process::Command`, toml, serde_json, wait-timeout, maturin, rattler-build — all still applicable). Versions checked on crates.io 2026-08-18.

| new subsystem | need | crate | status |
|---|---|---|---|
| `worktree/locks.py` | advisory `flock`, kernel-released, never-unlinked lock file | **fd-lock** 4.0.4 (2025-03-10, 58M downloads, [crates.io](https://crates.io/crates/fd-lock)); alternative `rustix::fs::flock` | exists, maintained |
| `tools.py` / `tty_session.py` remote payloads | shell-word quoting/joining (Python `shlex.quote`) | **shlex** 2.0.1 (2026-05-17, 736M downloads, [crates.io](https://crates.io/crates/shlex)) | exists, maintained |
| PTY/e2e infra (`test/fixtures/pty_helpers.py`) | fork a child onto a real pty, drive it from tests | **portable-pty** 0.9.0 (wezterm, 2025-02-11, 11.7M downloads) or **expectrl** 0.9.0 (2026-05-11, 643K downloads) | exists, maintained |
| CLI acceptance harness (the 96) | run the built binary, assert on its streams/exit | **assert_cmd** 2.2.2 (2026-05-11, 77M downloads) | exists, maintained |
| `disk_usage.py` | `st_blocks`/`st_nlink`/`(dev,ino)` dedup walk | `std::os::unix::fs::MetadataExt` + `std::fs::read_dir`; **walkdir** 2.5.0 (2024-03-01, 569M downloads) optional | std suffices |
| workspace clones + git-LFS | LFS pointer probing, LFS pulls | **no crate needed**: the probe is file-content sniffing (workspace_clone.py:201) and `git lfs` stays an external binary, as in Python | n/a |
| tool provisioning (zellij/pixi/gh/claude) | generate POSIX scripts, stream binaries over devpod ssh | **no crate needed**: scripts are strings (carry over verbatim); the stream is a real file on stdin (`Stdio::from(File)`) | n/a |
| launch benchmarking | — | out of port scope (`scripts/`); if ever ported, serde_json covers the record format | n/a |
| `timing.py` JSON mode | machine-readable stage report | serde_json + `std::time::Instant` (already surveyed) | n/a |

Two `std::process` details new since the survey: the background cache updater detaches with `start_new_session=True` (dl.py:431-436) — `std::os::unix::process::CommandExt::process_group(0)` / pre-exec `setsid`; and `run_devpod_session` holds a `Popen` with only stderr piped while the terminal keeps stdout (dl.py:3192) — plain `Stdio::piped()` on stderr. Neither is a gap.

**The feasibility doc's "every crate devlaunch needs exists and is maintained" still holds at 0.0.27**, with four additions (fd-lock, shlex, portable-pty/expectrl, assert_cmd), all active and widely used. Nothing the new subsystems do requires a library that does not exist.

## 4. Subprocess surface

Audited at `54d8280` over `devlaunch/` and `scripts/`:

- **Zero `shell=True`.** Every grep hit for the string is a comment asserting its absence (`# nosec B603 … not shell=True` at gh_auth.py:72, dl.py:3165,3169,3191,3797, scripts/bench_launch.py:134,144).
- **Zero `os.system`**, zero f-string/format-string argv (`subprocess.run(f...)` grep: no hits).
- ~33 direct spawn sites, all list-form: `dl.py` 9 (7 `run` + 2 `Popen`), `worktree/workspace_clone.py` 9, `worktree/repo_manager.py` 7, `worktree/branch_manager.py` 6, `workspace_state.py` 1, `gh_auth.py` 1; `devpod_provider.py` and `tools.py` route through an injected runner defaulting to `subprocess.run`/`run_devpod`. `os.execvpe` appears only in the test PTY fixture's forked child (test/fixtures/pty_helpers.py:100).

**One change in character since 2026-08-07, worth stating precisely:** local process invocation is still uniformly list-form — the `std::process::Command` 1:1 story holds — but the tool-provisioning and pty layers now deliberately compose **remote** shell command lines as strings, quoted with `shlex.quote`: `bash -lc <script>` payloads for `devpod ssh --command` (tools.py:970,608,283), `bash -c <transfer script>` (tools.py:984), and `cd <dir> && <cmd>` inside the ssh argv (tty_session.py:142). Those strings are single argv elements handed to a list-form local exec (`run_ssh`, dl.py:3791-3799: "list form, not shell=True, so nothing in the payload is interpreted by a host shell") and are interpreted by the shell *inside the container* — which is the mechanism, not an accident. A port reproduces this with the `shlex` crate's quote/join; it does not break the local `Command` mapping, but "no shell interpolation anywhere" is no longer the whole story and the quoting layer is now load-bearing surface (tty_session.py:125-133 additionally refuses ids that would reach ssh as options).
