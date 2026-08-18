# Rust rewrite: the execution plan

The committed spec for the Rust port, assembling the decisions of map
[#248](https://github.com/blooop/devlaunch/issues/248) — tickets
[#249](https://github.com/blooop/devlaunch/issues/249) (inventory),
[#250](https://github.com/blooop/devlaunch/issues/250) (wf surface),
[#251](https://github.com/blooop/devlaunch/issues/251) (architecture),
[#252](https://github.com/blooop/devlaunch/issues/252) (harness),
[#253](https://github.com/blooop/devlaunch/issues/253) (moving target),
[#254](https://github.com/blooop/devlaunch/issues/254) (distribution) — into the
port order, the divergence table, and the cutover checklist
([#255](https://github.com/blooop/devlaunch/issues/255)). Written 2026-08-18
against `main` at 0.0.29. This document decides sequencing only; where it
restates a ticket, the ticket is the authority.

## Standing decisions (inputs, not open questions)

- **One cargo workspace, three crates**, living in this repo under `rust/`:
  `devlaunch-core` (lib), `dl` (bin), `devlaunch-test-support` (dev-only fake
  runner + fixtures). Versioned together, single-sourced in `Cargo.toml`.
- **Two binding invariants**: the `dl` binary holds nothing beyond parsing,
  rendering, and interactive selection; the acceptance harness targets the CLI
  binary.
- **Four layers, dependencies strictly downward**: runner → tool clients
  (devpod/git/gh/ssh) → domain (workspace_id, spec, model, metadata, config,
  xdg) → flows (launch, lifecycle, listing, provision, completion, disk_usage,
  timing). Python's domain vocabulary is kept; dl.py's shape is not.
- **Typed per-operation errors; no user-facing English in core.** Text and exit
  codes (including exit-127 for devpod-missing) are rendering, owned by `dl`.
- **The harness is the existing pytest suite** parameterized by
  `DEVLAUNCH_DL_CMD`; two tiers per push (fake-devpod PATH shim; real-devpod
  e2e) × both binaries; a required `rust-parity` CI job ratcheted by an
  expected-failure manifest; a per-file spec ledger with a shrink-only
  `pending` lint.
- **Python is feature-frozen (2026-08-18); big-bang cutover.** The Rust binary
  first ships as `0.1.0` when the manifest is empty on both tiers and the
  ledger has zero `pending` rows. linux-64 only; PyPI maturin bin-wheel +
  prefix.dev conda; no pre-releases; one pinned toolchain in
  `rust-toolchain.toml`, no MSRV.
- **Correctness and maintainability over performance** — the tiebreaker for
  every remaining choice. Prefer illegal-states-unrepresentable types, total
  functions, and the simpler structure.

## Port order

Each milestone leaves the tree green: `cargo test` passes, `rust-parity` passes
under the then-current manifest, and every ledger row it claims moves out of
`pending`. Milestones are sized to one focused agent session. Later milestones
depend only on earlier ones; milestones marked (∥) are mutually independent.

**M0 — harness and scaffold.** The judge before any defendant:
`DEVLAUNCH_DL_CMD` seam in `test/fixtures/e2e_helpers.py` (default
`python -m devlaunch.dl`) with `test_full_workflow.py`'s direct spawn sites
routed through it; the fake `devpod` PATH shim as a standalone,
implementation-blind program (argv→response table + workspace state machine +
invocation log — `devpod_mock.py`'s design re-homed); the cargo workspace with
all three crates compiling and all anticipated dependencies declared;
`rust/spec-ledger.md` seeded with one row per Python test file; the empty
`rust/parity-manifest.txt`; the `rust-parity` CI job with the two-way ratchet
and the ledger lint, required from day one.

**M1 — the runner.** The one `Runner` trait (spawn-spec in, outcome out), the
spawn-spec carrying the four exotics as data (stdin-from-file, setsid detach,
stderr-only capture, timeout); the production `std::process::Command`
implementation; in `devlaunch-test-support`, the single fake: call recorder,
argv→response table, devpod workspace state machine.

**M2 (∥) — domain.** `workspace_id` (the #55/#64 parse boundary; one
constructor over the unsanitized triple), `spec` parsing
(`owner/repo@branch#variant` and friends), `model` (Workspace, container-state
enum with `Unknown(String)`, the four-arm `Unsaved` sum), `metadata` (serde
over the version-headered v2 document, forward-tolerant, plus the ported v1→v2
migration), `config` (TOML), `xdg` (written once), `locks` (fd-lock over the
same flock semantics: per-workspace, never-unlinked, kernel-released).

**M3 (∥) — tool clients.** `git` (the #54-duplicated helpers merged: clone,
fetch, worktree/branch ops, status-porcelain parsing, LFS pointer sniff +
`git lfs` calls, `git_errors` shaping), `devpod` (up/status/list/ssh argv
building, JSON parsing, provider probe, wrapped-exit-status recovery from
`devpod_ssh`), `gh` (token via private file, never argv), `ssh` (tty argv with
the option-injection refusal, `shlex`-quoted remote payloads).

**M4 — storage flows.** The worktree backend re-drawn over M2+M3: bare-clone
cache (`repo_manager`: fetch, half-removed-clone recovery, symlinked-root
refusal), workspace clones (`workspace_clone`: hardlink sharing, LFS), branch
management, all under the M2 locks. This milestone re-pins the internal
invariants (flock per-OFD semantics, recovery) as Rust-native tests and
re-expresses the observable ones (object sharing, clone races) at the binary
boundary.

**M5 — read-side flows + the first binary.** `listing`/`workspace_state`
(everything behind `--ls --json`), `disk_usage`, `timing`
(`DEVLAUNCH_TIMING=1|json` on an explicit registry), `completion` cache
writing; the `dl` binary appears here: clap (derive) grammar, rendering of
typed results to text and exit codes, `--version`. From M5 on, the parity job
has a binary to judge and the manifest starts shrinking.

**M6 — lifecycle flows.** `rm` (unsaved-work guard, typed refusal, `--force`),
`stop`, `up` (prewarm, per-workspace launch lock), `--prune`/`--purge`
(ownership-scoped, names what it refused), `--reconcile`, orphan handling.
This is wf's consumption surface; core's public API (§7 of #251: `list()`,
`remove(id, force)`, `up(spec)`, spec/branch helpers, handoff constants) is
frozen at the end of M6 — everything else stays crate-private.

**M7 — launch.** The one designed-fresh flow: spec → clone/branch → devpod up
→ fast attach → `-- <cmd>` exec through `devpod ssh --command` with the
shlex-quoted payload contract, handoff env stamps (`DEVLAUNCH_HANDOFF_T0`,
`DEVLAUNCH_PREWARM_FIRED_AT`), launch serialization against the prewarm,
interactive `tty_session`.

**M8 — provisioning + the second entry point.** `tools.py`'s port: the
generated POSIX scripts carry over verbatim as string constants; the
composition/quoting layer uses `shlex`; binaries stream as a real file on
stdin. `aid` as a second thin binary rewriting its argv into a `dl` command
line. The verb-first selector grammar lands here too: reserved verbs beat bare
specs, no-target verbs open the embedded skim picker.

**M9 — parity burn-down.** Re-express the remaining contract areas as
binary-boundary tests in priority order (wf's six call sites; exit
codes/refusals; CLI grammar incl. divergence points; lifecycle→devpod argv via
the shim log); shrink the manifest to empty on both tiers; drive the ledger to
zero `pending`; manual exploratory testing against a scratch
`XDG_CACHE_HOME`/`DEVPOD_HOME`, with every found deviation landing as a test
before the fix.

**M10 — distribution.** maturin bin-wheel on PyPI, rattler-build
`cargo install` recipe for prefix.dev, version single-sourced from
`Cargo.toml`; the cutover release PR (0.1.0) flips the released `dl` to Rust
in one step. Sequenced last and gated on the checklist below; the port PRs
before it change nothing user-facing.

wf's switch to linking `devlaunch-core` is out of scope here (wayfinder's
maps own it); it is bounded to at-or-after cutover.

## Divergence table (Grade C)

Every deliberate behavioral difference between the Python and Rust binaries is
a numbered row here; the harness may branch per-binary only when citing a row.
Nothing diverges silently.

| # | Divergence | Rationale |
|---|---|---|
| 1 | Reserved verb words (`stop`, `prune`, …) win over bare workspace specs; verb-first with no target opens the fuzzy selector. Python parses `dl stop` as a spec. | Owner requirement folded into #251 §6. |
| 2 | clap strictness: unknown flags are rejected; no argparse-style prefix abbreviation. | Correctness aim winning over accidental laxity. |
| 3 | `--help` layout is clap's, not the hand-rolled text. | Generated help feeds the README rule. |
| 4 | Failure output: no Python tracebacks, ever; typed errors rendered as one-line diagnostics with the same exit codes. | #251 §5. |
| 5 | Startup/perf deltas asserted nowhere; `bench.yml` stays outside the gate. | Perf is not a parity dimension. |
| 6 | The fuzzy selector is embedded skim, not spawned fzf; `iterfzf`/fzf are no longer runtime dependencies. | Removes a launch-failure class (#251 §6). |
| 7 | Safe-name validation accepts a measured superset of Python's `\w` (Unicode combining marks, enclosed letters, and codepoints newer than Python's UCD). Every name Python accepts keeps its exact id — no workspace on disk moves; only names Python refused are now accepted. | `char::is_alphanumeric` vs `\w`; verified per-codepoint against Python 3.14. |
| 8 | Metadata/config loads refuse wrong-typed values where Python coerced: a mistyped entry field skips the entry (with backup) instead of loading a mistyped model; `fetch_interval`/`prune_after_days` of the wrong type are typed refusals; a falsy-but-non-null `last_fetched` (`[]`, `false`, `0`) skips the entry. | Correctness aim winning; parse-don't-validate at the storage boundary. |
| 9 | `repos_dir = "~/…"` in `config.toml` is tilde-expanded (Python's loader wrapped the string in `Path()` before expansion could run, so `~` was taken literally); the mkdir-if-under-home-or-`/tmp` check is path-component-wise, so `/tmp2/x` no longer counts. | Fixes a latent config bug rather than porting it. |
| 10 | `dl --install` re-run over an already-current install rewrites nothing (Python rewrote byte-identical files, touching rc mtimes on every run); the outcome is reported as already-installed. | Idempotence made observable. |

Additions require a PR that updates this table; the row number is cited by any
per-binary harness branch.

## Cutover checklist

The cutover release (0.1.0) may ship when every box is checked:

1. `rust/parity-manifest.txt` is empty and `rust-parity` is green on **both**
   tiers (shim + real devpod) against the Rust binary.
2. `rust/spec-ledger.md` has zero `pending` rows; every row is `re-expressed
   at boundary`, `re-pinned in Rust`, `covered by divergence row #N`, or `out
   of port scope`.
3. Coexistence verified on a dev machine: Rust and Python binaries alternated
   against one real cache — `metadata.json` (schema v2) read/written
   byte-compatibly under the same flocks, completion caches identical, the
   v1→v2 migration exercised; the cutover release itself migrates nothing.
4. Both channels ship from one tag: PyPI bin-wheel (maturin) and prefix.dev
   conda (rattler-build), linux-64, version read from `Cargo.toml`.
5. README regenerated against `dl --help`; divergence table final.
6. Rollback documented: pin `devlaunch <0.1`.

Emergency fixes to released 0.0.x remain legal during the port; each must land
with a manifest debt row naming its PR (the #253 rule — empty-manifest cutover
is the payment deadline).

## The spec ledger

`rust/spec-ledger.md`, one row per file under `test/` (class granularity where
a file mixes fates). Dispositions: `pending` | `re-expressed at boundary` |
`re-pinned in Rust` | `covered by divergence row #N` | `out of port scope`.
The `rust-parity` job lints that every test file has a row and that the
`pending` count only shrinks. Each milestone above names the rows it moves;
the spec gets read exactly when its module gets ported.
