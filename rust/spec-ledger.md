# Spec ledger

One row per file under `test/` (split to class granularity when a file mixes
fates). The `rust-parity` job lints that every test file has a row and that
the `pending` count only shrinks (recorded in `pending-count.txt`).
Dispositions and policy: docs/rust-rewrite-plan.md ("The spec ledger").

| file | disposition | notes |
|---|---|---|
| `test/conftest.py` | out of port scope (harness infrastructure) |  |
| `test/devpod_scoping.py` | out of port scope (harness infrastructure) |  |
| `test/e2e/__init__.py` | out of port scope (harness infrastructure) |  |
| `test/e2e/conftest.py` | out of port scope (harness infrastructure) |  |
| `test/e2e/test_claude_config_protection.py` | pending |  |
| `test/e2e/test_full_workflow.py` | pending |  |
| `test/e2e/test_interactive_session.py` | pending |  |
| `test/e2e/test_ssh_config_isolation.py` | pending |  |
| `test/fixtures/__init__.py` | out of port scope (harness infrastructure) |  |
| `test/fixtures/devpod_mock.py` | out of port scope (harness infrastructure) |  |
| `test/fixtures/devpod_shim.py` | out of port scope (harness infrastructure) | the Tier-1 fake devpod on PATH |
| `test/fixtures/e2e_guard.py` | out of port scope (harness infrastructure) |  |
| `test/fixtures/e2e_helpers.py` | out of port scope (harness infrastructure) |  |
| `test/fixtures/git_fixtures.py` | out of port scope (harness infrastructure) |  |
| `test/fixtures/permissions.py` | out of port scope (harness infrastructure) |  |
| `test/fixtures/pty_helpers.py` | out of port scope (harness infrastructure) |  |
| `test/fixtures/shim_fixtures.py` | out of port scope (harness infrastructure) | shim wiring for the tiers |
| `test/fixtures/subprocess_drivers.py` | out of port scope (harness infrastructure) |  |
| `test/integration/__init__.py` | out of port scope (harness infrastructure) |  |
| `test/integration/test_clone_object_sharing.py` | re-pinned in Rust | flows/workspace_clone.rs real-git: pack sharing by (st_dev, st_ino) + st_nlink, repack breaks links + fsck passes |
| `test/integration/test_clone_race.py` | re-pinned in Rust | flows/repo_manager.rs: in-process two-OFD contention (deterministic, no sleeps), waiter adopts winner's clone, one cross-process flock test |
| `test/integration/test_lfs_object_sharing.py` | re-pinned in Rust | flows/workspace_clone.rs real-git-lfs; steps aside (with eprintln) when git-lfs or filter.lfs.smudge is absent |
| `test/integration/test_lfs_probe_real.py` | re-pinned in Rust | flows/workspace_clone.rs: LFS pointer sniff over real repos |
| `test/integration/test_repo_manager_real.py` | re-pinned in Rust | flows/repo_manager.rs real-git: clone/fetch/adoption/default-branch off the clone |
| `test/integration/test_repo_manager_recovery.py` | re-pinned in Rust | flows/repo_manager.rs: partial-clone clearing, failed clone keeps the lock file, second attempt succeeds |
| `test/test_agents_doc.py` | out of port scope | pins scripts/ or docs, not the shipped binary |
| `test/test_bash_completion.py` | out of port scope | pins devlaunch/completions/dl.bash, embedded verbatim by both binaries; never spawns dl |
| `test/test_bench_doc.py` | out of port scope | pins scripts/ or docs, not the shipped binary |
| `test/test_bench_points.py` | out of port scope | pins scripts/ or docs, not the shipped binary |
| `test/test_bench_record_schema.py` | out of port scope | pins scripts/ or docs, not the shipped binary |
| `test/test_bench_workflow.py` | out of port scope | pins scripts/ or docs, not the shipped binary |
| `test/test_cold_launch_fetches.py` | re-pinned in Rust | flows/workspace_clone.rs: the 8-call cold sequence, one targeted refspec in the bare, no wildcard/--tags/--prune |
| `test/test_concurrent_launches.py` | pending | clone/adoption race + launch-lock exclusion re-pinned (repo_manager.rs, launch.rs); metadata lost-updates in domain/metadata.rs; three cross-process drivers await M9 boundary |
| `test/test_devpod_spawn_counts.py` | re-pinned in Rust | warm/cold spawn chains, opt-ins, no-metadata-io, exit-127 line and cold ssh trips at the boundary (dl/tests/launch.rs); listing/lifecycle rows in their flows |
| `test/test_dl.py` | re-pinned in Rust | flow classes in listing/completion_cache/lifecycle/launch; read-side, lifecycle and launch-verb dispatch byte-pinned at the boundary (dl/tests) |
| `test/test_interactive_command.py` | pending | typed side + devpod-route boundary done (payload parity, exit propagation, no-terminal routing, token forwarding); OpenSSH pty half awaits M9; aid transport awaits M8 |
| `test/test_lending_doc.py` | out of port scope | pins scripts/ or docs, not the shipped binary |
| `test/test_locks.py` | re-pinned in Rust | domain/locks.rs; the literal "dl: waiting" stderr line is rendering (typed event pinned; words are the dl binary's) |
| `test/test_pty_helpers.py` | out of port scope (harness infrastructure) | pins test/fixtures/pty_helpers.py, which survives as the judge |
| `test/test_repo_lock_cycles.py` | pending | token + holds-throughout + warm-shape counts re-pinned (repo_manager.rs, launch.rs); cold 1-vs-2 cycle counts await M9 boundary |
| `test/test_timing.py` | pending | gate/vocabulary/handoff/prewarm + launch spans re-pinned; prose-mode round-trip names at the boundary; tools-stage JSON spans await M9; bench-harness classes out of scope with scripts/ |
| `test/test_workspace_clone.py` | re-pinned in Rust | flows/workspace_clone.rs: argv-exact over the fake runner (real objects where Python mocked the managers) |
| `test/test_workspace_id.py` | re-pinned in Rust | all 53 behaviors; 55 Rust tests in domain/workspace_id.rs incl. 45 Python-generated golden ids |
| `test/test_workspace_state.py` | re-pinned in Rust | state/listing/DeleteGuard/ForcedRemove in workspace_state.rs, listing.rs, lifecycle.rs; caplog texts byte-pinned at the boundary (uncommitted/unpushed/could-not-tell/--force) |
| `test/test_worktree_branch_manager.py` | re-pinned in Rust | flows/branch_manager.rs: 4-state branch decision table as argv sequences; RemoteRefs/CreateRemote sums keep all four boolean combinations |
| `test/test_worktree_config.py` | re-pinned in Rust | domain/config.rs; to_dict untested — no production caller writes config.toml |
| `test/test_worktree_migration.py` | re-pinned in Rust | flows/migration.rs (runner-free by type); TestWiring re-expressed at boundary (rust/dl/tests/read_side.rs: the json listing migrates, the table does not) |
| `test/test_worktree_models.py` | re-pinned in Rust | domain/model.rs; byte-compat golden JSON from Python |
| `test/test_worktree_repo_manager.py` | re-pinned in Rust | flows/repo_manager.rs: RepoLock minting structural (private fields), FetchOutcome exhaustive, one-lstat symlinked-root refusal |
| `test/test_worktree_storage.py` | re-pinned in Rust | domain/metadata.rs; seam-patched tests re-expressed as behavior |
| `test/unit/__init__.py` | out of port scope (harness infrastructure) |  |
| `test/unit/test_aid.py` | pending |  |
| `test/unit/test_claude_code_feature_mounts.py` | out of port scope | repo-artifact agreement (.devcontainer/claude-code README vs its feature manifest + pre-create hook); never spawns dl; keeps running in the normal pytest job |
| `test/unit/test_devcontainer_manifest.py` | out of port scope | pins this repo's own devcontainer.json (ssh mounts, pixi tasks); never spawns dl; keeps running in the normal pytest job |
| `test/unit/test_devpod_provider.py` | re-pinned in Rust | clients/devpod.rs; the standalone `python -m` CLI tests are out of port scope (defect class absent in Rust) |
| `test/unit/test_devpod_scoping.py` | out of port scope (harness infrastructure) | pins test/devpod_scoping.py, suite-side scoping |
| `test/unit/test_devpod_shim.py` | out of port scope (harness infrastructure) | pins the shim, which is the judge's own tooling |
| `test/unit/test_devpod_shim_fixture.py` | out of port scope (harness infrastructure) | pins the shim fixture wiring |
| `test/unit/test_devpod_ssh.py` | re-pinned in Rust | clients/devpod.rs: SshOutcome + StderrFilter, incl. session round trip |
| `test/unit/test_disk_usage.py` | re-pinned in Rust | flows/disk_usage.rs; mid-walk races pinned at the classifier functions |
| `test/unit/test_dl_cmd_seam.py` | out of port scope (harness infrastructure) | pins the DEVLAUNCH_DL_CMD seam itself |
| `test/unit/test_docker_boundary.py` | pending |  |
| `test/unit/test_e2e_guard.py` | out of port scope (harness infrastructure) | pins the e2e guard fixture, which survives as the judge |
| `test/unit/test_e2e_workspace_helper.py` | out of port scope (harness infrastructure) | pins e2e_helpers.py, which survives as the judge |
| `test/unit/test_gh_auth.py` | re-pinned in Rust | decisions in clients/gh.rs; memoization in launch.rs HostToken; warning texts pinned in render unit tests + token staging at the boundary |
| `test/unit/test_launch_serialization.py` | re-pinned in Rust | flows/launch.rs: all four classes; per-OFD flock makes the in-process pin two real acquisitions, stronger than Python's stub |
| `test/unit/test_locks.py` | re-pinned in Rust | domain/locks.rs; the Python-language API-shape guards have no Rust analogue |
| `test/unit/test_prune_orphaned_clones.py` | pending | classification/promotion/withholding re-pinned (lifecycle.rs); report/input/disk classes + unknown-option byte-pinned at boundary; cross-process lock timing awaits M9 |
| `test/unit/test_purge_ownership.py` | re-pinned in Rust | ownership split + action in listing.rs/lifecycle.rs (asked-once patch test unportable, its property pinned); leaving-behind lines byte-pinned at the boundary |
| `test/unit/test_purge_partial_removal.py` | re-pinned in Rust | walk/arms/obstruction/randomised invariants in repo_manager.rs + lifecycle.rs; the three arms' sentences, sudo line and symlink reason byte-pinned at the boundary (dl/tests/lifecycle.rs) |
| `test/unit/test_reconcile_orphaned_workspaces.py` | pending | adoption/refusals/apply re-pinned (lifecycle.rs) and report/confirm at boundary; migration-notice join not yet built from migrate_cache's real leavings |
| `test/unit/test_spec_parsing.py` | re-pinned in Rust | misnamed file: model/config serialization, covered by domain/model.rs + config.rs tests |
| `test/unit/test_stored_workspace_id.py` | re-pinned in Rust | record-vs-derivation, warm-path-reads-no-metadata (never-called closure), failed-lookup in lifecycle.rs; subcommand addressing byte-pinned at the boundary |
| `test/unit/test_tools.py` | re-pinned in Rust | flows/provision.rs: scripts byte-golden vs the Python module, shlex round-trips, ustar payload; TestWorkspaceUpInstallsTools boundary-pinned (up installs / failed up does not / running tops up) |
| `test/unit/test_tty_session.py` | re-pinned in Rust | clients/ssh.rs; chmod-000 case re-expressed root-proof |
| `test/unit/test_updater_fetch_sweep.py` | re-pinned in Rust | sweep classes in lifecycle.rs; child-migrates at the boundary (v1 cache migrated by --update-cache); subprocess-boundary class is harness infrastructure |
| `test/unit/test_workspace_listing.py` | re-pinned in Rust | reads in flows/listing.rs, pinned again at the boundary with Python goldens; purge-will-not-act exit + stderr at the boundary |
| `test/unit/test_workspace_source.py` | pending | source arms re-pinned in clients/devpod.rs; describe_source + unreadable-repo discovery re-pinned (flows/listing.rs); the fuzzy-picker class awaits M8 |
| `test/unit/test_workspace_source_placement.py` | re-pinned in Rust | flows/lifecycle.rs placement: names_a_remote/source_places/site_of/holder/canonical; direction-independence and git-source-with-local-path pinned |
