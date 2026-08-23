# Rust rewrite: the execution plan

> **Status: done, and partly historical.** The cutover shipped (0.1.0), and the
> Python tree and the parity apparatus this plan describes were retired
> afterwards — [#267](https://github.com/blooop/devlaunch/issues/267). So
> `rust/parity.py`, `rust/parity-manifest.txt`, `rust/spec-ledger.md`,
> `rust/pending-count.txt`, `rust/golden_vectors.py` and `rust/tools/` no longer
> exist, the `rust-parity` CI job is now `rust` (cargo only), and the milestones
> and cutover checklist below are a record of how the port was run rather than
> instructions to follow.
>
> **The divergence table is the exception and is still live.** It is the only
> written record of every deliberate behavioural difference from the Python build,
> it is cited by row number from comments and tests throughout `rust/`, and those
> citations are the reason a reader can still tell a decision from an accident.
> Keep it.

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

- **One cargo workspace, three crates** (#251), living in this repo under
  `rust/`: `devlaunch-core` (lib), `dl` (bin), `devlaunch-test-support`
  (dev-only fake runner + fixtures). Versioned together, single-sourced in
  `Cargo.toml`. *Amendment (M3):* a fourth, internal leaf crate
  `devlaunch-runner` holds the runner layer, re-exported by core at its
  original path — the fake implements `Runner`, and test-support depending on
  core while core dev-depends on test-support made core's unit tests see two
  different `Runner` traits. wf still consumes only `devlaunch-core`.
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
(`owner/repo@branch` and friends), `model` (Workspace, container-state
enum with `Unknown(String)`, the three-arm `Unsaved` sum in `workspace_state`
— `NothingToLose | WouldLose | CouldNotTell`), `metadata` (serde
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
| 3 | `--help` layout is clap's, not the hand-rolled text — reordered so the verbs and examples land above the options table. | Generated help feeds the README rule. |
| 4 | Failure output: no Python tracebacks, ever; typed errors rendered as one-line diagnostics with the same exit codes. OS-level reasons inside diagnostics use Rust's phrasing (`Permission denied (os error 13)`) rather than Python's `[Errno 13] …`, and JSON type names in refusals may differ (`int` where Python said `float`). | #251 §5. |
| 5 | Startup/perf deltas asserted nowhere; `bench.yml` stays outside the gate. | Perf is not a parity dimension. |
| 6 | The fuzzy selector is embedded skim, not spawned fzf; `iterfzf`/fzf are no longer runtime dependencies. With no terminal (`dl < /dev/null`, workspaces present) the picker declines silently where fzf wrote `inappropriate ioctl for device` to stderr; stdout and exit 1 are identical. | Removes a launch-failure class (#251 §6). |
| 7 | Safe-name validation accepts a measured superset of Python's `\w` (Unicode combining marks, enclosed letters, and codepoints newer than Python's UCD). Every name Python accepts keeps its exact id — no workspace on disk moves; only names Python refused are now accepted. | `char::is_alphanumeric` vs `\w`; verified per-codepoint against Python 3.14. The same superset applies to `--reconcile`'s legacy leaf spelling, reachable only via a hand-edited record. |
| 8 | Metadata/config loads refuse wrong-typed values where Python coerced: a mistyped entry field skips the entry (with backup) instead of loading a mistyped model; `fetch_interval`/`prune_after_days` of the wrong type are typed refusals; a falsy-but-non-null `last_fetched` (`[]`, `false`, `0`) skips the entry. | Correctness aim winning; parse-don't-validate at the storage boundary. |
| 9 | `repos_dir = "~/…"` in `config.toml` is tilde-expanded (Python's loader wrapped the string in `Path()` before expansion could run, so `~` was taken literally); the mkdir-if-under-home-or-`/tmp` check is path-component-wise, so `/tmp2/x` no longer counts. | Fixes a latent config bug rather than porting it. |
| 10 | `dl --install` re-run over an already-current install rewrites nothing (Python rewrote byte-identical files, touching rc mtimes on every run); the outcome is reported as already-installed. | Idempotence made observable. |
| 11 | Boundary documents are read typed, not sniffed: a completion cache missing a key reads as empty (Python's `--repos` fell back to asking devpod when the `repos` key was absent); `--completion-data` re-serializes the four known keys, dropping a newer writer's extras; a non-string `state` from `devpod status` renders as `null`; `remote_branch_exists` counts parsed `ls-remote` lines, not any non-empty stdout. A `devpod context options` answer whose option bodies are not objects is discarded and re-asked, not cached empty — matching Python, whose `value` lookup raises and yields an uncached `{}`. | Parse-don't-validate at every input boundary (row 8's aim); the distinguishing inputs are malformed documents no released tool writes. |
| 12 | Non-UTF-8 clone paths render lossily (U+FFFD) where Python round-tripped the bytes via surrogateescape. | Rust has no surrogateescape; only undecodable paths differ. |
| 13 | The primary failure survives compound failures: a failed clone reports its git reason even when the debris cleanup also fails (Python raised the cleanup `OSError`, masking the clone's), and a migration that cannot write its orphan/unmigrated listing files says so in the migration report instead of aborting. | #251 §5 error shaping — no cause is masked. |
| 14 | CLI usage errors (unknown flags, malformed combinations clap itself rejects) exit 2, clap's convention, where Python exited 1. Refusals Python also made keep exit 1 and Python's wording. Inputs: `dl --nope`; `dl myws echo hi` (three positional words); `dl -leading-dash` (Python probed it as a workspace name, one `devpod status` round trip); `dl --ls --ls` (Python's argparse accepted the repeat). | Generated parser owns its usage errors (companion to rows 2–3). |
| 15 | Grammar completions and tightenings: `--stop`/`--rm` flag spellings work as verbs (dl.py's docstring promised them; its dispatch parsed them as a spec and failed); `--json`/`--size` require `--ls`; a global command refuses a stray target, a meaningless `-y`/`--force`, or `--devcontainer`; a trailing `-- <cmd>` beside a non-attach verb is refused; two global commands together (`dl --refresh --ls`, `dl --repos --ls`) are refused where Python ran whichever argparse happened to check first; no-target verb forms (`dl --stop`, `dl -- <cmd>`) open the selector; the `Ignoring --devcontainer: it does not apply` diagnostic names the resolved verb (`rm` for the `prune` spelling). Python silently discarded all of these. | Documented grammar made real; silent discards made refusals (rows 1–2's aim). **The `--stop`/`--rm` half is superseded by row 32**: `--rm` is now docker's session flag and `--stop` is retired, both refused by name. Everything else in this row stands. |
| 16 | `--version` always prints the bare version; Python appended `(dev, editable from <tree>)` for editable installs. A compiled binary has no editable-install metadata. | PEP 610 provenance has no Rust analogue; released builds printed identically anyway. |
| 17 | Provisioning never kills a launch devpod already ran: an unparsable probe stage line (`failed ²`, a bignum status) reads as not-reached where Python raised inside `stage_outcomes`; a trampoline sidecar with a non-string `exe` lends nothing where Python's `Path(2)` raised `TypeError`; a host with no home directory still runs the setup pass and network install where Python's `Path.home()` raised after `devpod up`. | Typed reads at the probe/sidecar boundary (rows 8/11's aim); every arm ends in the network-install fallback Python promised. |
| 18 | The host-tools bundle streams as plain ustar, not Python's PAX default; an archive member name ≥100 bytes is refused (`NameTooLong`) and provisioning falls through to the network install, where Python wrote a PAX long-name header. | Hand-rolled writer (no tar dependency); `tar xf -` observes identical members for every real payload; the fallback is the path Python took for any bundle failure. |
| 19 | A `-- <cmd>` argument containing a NUL byte is a typed refusal before anything spawns; Python quoted-and-mangled it into the remote payload. | The same refusal `ssh.rs` already makes for a workdir; a NUL cannot survive an argv anyway. |
| 20 | Path-spec workspace naming is lexical: `~` expanded, `.`/`..` normalised, symlinks NOT followed. Python's `Path.resolve()` followed symlinks, so `dl /a/symlink-to-b` could be named after `b`. | Identical for every real path without symlinked components; lexical naming means the name a user typed is the name they get. |
| 21 | The no-argument selector's choice routes through the same launch path as `dl <ws>`: one `devpod status` probe buys fast-attach for an already-running pick. Python's fzf path called `workspace_up` directly (no probe, always `devpod up`). | One behaviour for one shape; the extra probe is the fast-attach contract every other entry already pays. |
| 22 | A confirmation question nobody can answer is answered no: `dl --purge`, `dl --prune`, and `dl --reconcile` reading EOF on their `[y/N]` prompt (`< /dev/null`) print `Aborted.` and exit 0 where Python's `input()` raised `EOFError` (traceback, exit 1). A scripted run therefore reports success having done nothing. | The abort path already exists; EOF is a no, not a crash. |
| 23 | A lifecycle verb creates nothing on its way to removing something: `dl owner/repo@branch rm` on a never-launched spec no longer runs `prepare_cold` (a clone + a record), and `dl owner/repo stop` no longer runs `ensure_repo` (a bare clone) just to name the default branch — the branch comes from the record or `git ls-remote --symref`. | Removal that first creates is a footgun, not a contract; observable only as absent side effects. Same family: `dl <ws> <unknown-verb>` refuses from the grammar with no `devpod status` round trip (identical text and exit 1; only the shim log differs) — and when the target AND the verb are both unknown, the grammar names the verb where Python, validating the target first, named the target (`dl nosuchws nosuchverb`: Rust `Unknown command 'nosuchverb'…`, Python `Unknown workspace 'nosuchws'…`). |
| 24 | dl.py:4888's `Failed to create workspace: {e}` wrapper string has no producer: the `OSError`/`RuntimeError` class it caught is typed at the source (devpod missing/blocked/timed out render as their own one-liners; a NUL command is row 19's refusal), each with its Python exit code. | One failure, one line, no generic wrapper (row 4's aim). |
| 25 | A path spec that normalises to `/` (`dl /`, `//`, `/.`, `/..`) is refused with `'/' does not name a workspace: its path has no final component to name one after.` (exit 1) instead of handing devpod an empty `--id`. Python also exited 1, but only after `devpod up` ran with a blank id and the follow-up `devpod ssh ""` failed. | Refusing an unnameable spec up front beats creating a blank-id workspace; exit code matches, wording differs. |
| 26 | `dl --version <extra-arg>` / `aid --version <extra-arg>` exit 2 (clap rejects an argument beside `--version`) where Python ignored the extra and printed the version (exit 0). | Companion to rows 2/14/15 — the generated parser owns its usage errors. |
| 27 | On SIGINT dl prints no timing summary and no partial output — the handler is `_exit(130)` after an async-signal-safe cleanup (unlink the staged token / temp files, `killpg` the `devpod up` group). Python emitted a timing summary from a `finally`. | A signal handler may not allocate, lock, or format; correctness of the credential cleanup wins over the summary (row 5: timing is not a parity dimension). |
| 28 | The `metadata.json` reader takes strict JSON where Python's `json.loads` took its extensions: `NaN`/`Infinity` literals, numbers beyond f64 range (`1e400`, which Python reads as `inf`), and lone-surrogate `\uD800` escapes all read as corruption, so a file carrying one is quarantined to `metadata.json.corrupt` (bytes intact, single slot) and the run starts with an empty cache where Python loaded the records around the value. | serde_json cannot represent a lone surrogate in a Rust `String` and f64 cannot hold `1e400`, so accepting them would mean values the store could not round-trip; no build of dl (Python or Rust) ever writes any of them — only a hand-edited or third-party-written file can carry one — and the quarantine keeps the bytes recoverable. Pinned by `pythons_json_extensions_read_as_corruption_here` in `domain/metadata.rs`. |
| 29 | A clone the host refused as not-found gets a second line naming the same repository under an owner the completion cache knows: `Did you mean 'kinisi-robotics/kinisi_ros'? …`. Python printed git's stderr and stopped. The line is added only for a host's own not-found wording (GitHub, GitLab, Bitbucket), only from a clone step, and only when the cache holds the same repo name under another owner *and* not under the owner typed; at most three candidates are named and any remainder is counted. The git text above it is unchanged in every case. | A mistyped owner and a revoked permission produce the same six lines from git, so git's words cannot tell them apart — and devlaunch already holds the answer in the list its own shell completion offers. Additive: nothing is removed or reworded, and a machine with no cache entry sees exactly what it saw before. The suggestion names a *spec*, not a `dl …` command line, because `aid` renders through the same function and its own invocation carries an agent prompt. Pinned by the `wrong_owner_hint` tests in `dl/tests/launch.rs` and `dl/src/render.rs`. |

| 30 | `--stop` and `--rm` are **appendable**: on a line that already said something else they win, and the words they displace are reported rather than refused. `dl <ws> 'review this pr' --rm` was `Unknown command 'review this pr'` (exit 1) in both Python and earlier Rust; `dl prune <ws> --force --rm` took `prune` as the *workspace* and reported it unknown. A leading verb word is now skipped when finding the target, a word spelling the flag's own verb is absorbed silently, and everything else displaced is named on stderr (`--rm overrode the rest of the line: 'review this pr' was not acted on.`) before anything is removed. `aid` peels the same two flags — plus a `--force` in their company — off the end of its argv before the prompt join, so `aid <ws> <prompt> --rm --force` removes the workspace and hands no agent command to dl; a verb flag before the spec is collected the same way rather than passed through beside a `--` tail dl would refuse. The peel is bounded to the exact words at the very end of the line, so a prompt that mentions `--rm` or ends in a bare `--force` is untouched, and a `--` command tail cannot be overridden at all. | **Superseded by row 32**, which takes both spellings back: `--rm` became docker's `--rm` and `--stop` was retired, so no flag overrides a line any more. A shell makes appending to a recalled line cheap and rewriting its front expensive, so "and now delete it" has to be spellable as a suffix. The notice is what makes it safe rather than a silent discard: the line may now carry an instruction it will not carry out, so a deliberate suffix and a slip have to be distinguishable. Row 15 made the flag spellings work; this makes them the one form that can be typed last. Pinned by the `suffix_verb` tests in `dl/src/cli.rs` and `aid/src/rewrite.rs`, and by the `--aid -- --help` allowance in `rust/tools/parity_cases.txt` — aid's help text is hand-written and was byte-compared against Python's, so documenting a flag Python's aid has not got is where this row becomes visible to the parity harness. |

| 31 | `prune` is retired as a workspace verb. `dl <ws> prune` / `dl prune <ws>` / `dl prune` were Python's second spelling of `rm` and now refuse with `'prune' is no longer a workspace verb. Use 'dl <workspace> rm' to delete a workspace, or 'dl --prune' to remove the clone directories no workspace opens any more.` (exit 1). The word stays in a `RETIRED` table rather than being dropped, so it is still never read as a workspace name: `dl prune <ws> --force --rm` removes `<ws>` (row 30's suffix form, with `prune` absorbed as the same verb the flag asks for), and a live verb beside it still wins from either position, so `dl stop prune` stops a workspace named `prune` as before. `dl --prune` is unchanged. Row 15's parenthetical about the `Ignoring --devcontainer` diagnostic naming `rm` for the `prune` spelling is now unreachable. | **Superseded in part by row 32**, which retires row 30's suffix form: `dl prune <ws> --rm` is now this row's refusal rather than a removal of `<ws>`, and the word is still in `RETIRED` and still never read as a workspace name. One word meant two unrelated commands, separated only by two dashes: `dl <ws> prune` deleted a workspace and `dl --prune` deletes clone directories and no workspace. The two failure modes were losing a workspace meant to be kept, and being refused by `--prune takes no workspace` for a reason that sentence cannot explain. `rm` has no such twin, and the retirement is a refusal that names both meanings rather than a silent removal. Pinned by the `prune_is_refused_as_a_verb_from_either_position`, `a_retired_word_is_never_read_as_the_workspace` and `a_recalled_prune_line_is_the_retirement_and_the_pair_is_named_ahead_of_it` tests in `dl/src/cli.rs`. |

| 32 | **`--rm` is docker's `--rm`, and the flag-spelled verbs are retired.** `dl <ws> --rm` and `dl <ws> --rm -- <cmd>` now hand over a session and delete the workspace once it ends — what row 30's `--autorm` did, under the name docker gives it. The word `rm` is unchanged and is the only way to delete one *now*, so `docker rm` / `docker run --rm` is the whole of the grammar and no spelling has to be read twice. Three withdrawals pay for it, each refused at exit 1 rather than reinterpreted: `--autorm` (`--autorm is now spelled --rm: …`); `--stop`, whose only reason to exist was being a flag (rows 15 and 30) and which cannot stay a *cancelling* suffix beside a `--rm` that runs the line; and row 30's suffix override itself, so `aid <ws> 'review this pr' --rm` now runs the review and deletes afterwards where it used to delete instead, and `dl prune <ws> --rm` is row 31's refusal rather than a removal (with `--force` also on the line it is the `--force`-beside-`--rm` refusal instead, named first because the pair is the more confused half). `--force` still does not compose with `--rm` — docker keeps `-f` on `rm` too — and `dl <ws> rm --rm` is refused as the two requests it is. Retired flags are answered *before* anything else the line got wrong, since both moved on account of `--rm`'s new meaning. aid peels the retired spellings so dl refuses them by name instead of joining them into a prompt, and builds no agent command for such a line, so nothing is booted on the way to exit 1. `Overridden`, `pick_target` and the `--rm overrode the rest of the line` notice are gone with the override. | Row 30 bought "and now delete it" as a suffix and paid for it with a flag whose meaning was unguessable from its spelling: `--rm` cancelled the line, `--autorm` ran it, and the two looked like a pair. docker had already split the same problem the other way — a verb for now, a `run` flag for after — and taking that split makes the common line (`aid <ws> <prompt> --rm`: send the agent in, get the disk back) the one the short spelling names, at the cost of the rarer one, which `dl rm` and a pick does in fewer keystrokes than recalling a long prompt to append to it. The withdrawals are recognised rather than deleted, for row 31's reason: a flag dropped from `Cli` is clap's `unexpected argument` at exit 2, naming the spelling and not the replacement — and the spelling is exactly what cannot explain a line that stopped working because a *different* flag changed meaning. Pinned by the `retired_flag` / `rm_` tests in `dl/src/cli.rs`, `the_retired_flag_spellings_name_the_words_that_replaced_them` and `autorm_is_refused_with_the_spelling_that_replaced_it` in `dl/tests/lifecycle.rs`, the `rm_on_exit_` tests in `dl/tests/launch.rs`, and `a_retired_spelling_starts_no_agent_and_is_handed_to_dl_to_refuse` in `aid/src/rewrite.rs`. |

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
