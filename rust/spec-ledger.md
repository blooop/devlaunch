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
| `test/integration/test_clone_object_sharing.py` | pending |  |
| `test/integration/test_clone_race.py` | pending |  |
| `test/integration/test_lfs_object_sharing.py` | pending |  |
| `test/integration/test_lfs_probe_real.py` | pending |  |
| `test/integration/test_repo_manager_real.py` | pending |  |
| `test/integration/test_repo_manager_recovery.py` | pending |  |
| `test/test_agents_doc.py` | out of port scope | pins scripts/ or docs, not the shipped binary |
| `test/test_bash_completion.py` | pending |  |
| `test/test_bench_doc.py` | out of port scope | pins scripts/ or docs, not the shipped binary |
| `test/test_bench_points.py` | out of port scope | pins scripts/ or docs, not the shipped binary |
| `test/test_bench_record_schema.py` | out of port scope | pins scripts/ or docs, not the shipped binary |
| `test/test_bench_workflow.py` | out of port scope | pins scripts/ or docs, not the shipped binary |
| `test/test_cold_launch_fetches.py` | pending |  |
| `test/test_concurrent_launches.py` | pending |  |
| `test/test_devpod_spawn_counts.py` | pending |  |
| `test/test_dl.py` | pending |  |
| `test/test_interactive_command.py` | pending |  |
| `test/test_lending_doc.py` | out of port scope | pins scripts/ or docs, not the shipped binary |
| `test/test_locks.py` | re-pinned in Rust | domain/locks.rs; the literal "dl: waiting" stderr line is rendering (typed event pinned; words are the dl binary's) |
| `test/test_pty_helpers.py` | out of port scope (harness infrastructure) | pins test/fixtures/pty_helpers.py, which survives as the judge |
| `test/test_repo_lock_cycles.py` | pending |  |
| `test/test_timing.py` | pending |  |
| `test/test_workspace_clone.py` | pending |  |
| `test/test_workspace_id.py` | re-pinned in Rust | all 53 behaviors; 55 Rust tests in domain/workspace_id.rs incl. 45 Python-generated golden ids |
| `test/test_workspace_state.py` | pending |  |
| `test/test_worktree_branch_manager.py` | pending |  |
| `test/test_worktree_config.py` | re-pinned in Rust | domain/config.rs; to_dict untested — no production caller writes config.toml |
| `test/test_worktree_migration.py` | pending |  |
| `test/test_worktree_models.py` | re-pinned in Rust | domain/model.rs; byte-compat golden JSON from Python |
| `test/test_worktree_repo_manager.py` | pending |  |
| `test/test_worktree_storage.py` | re-pinned in Rust | domain/metadata.rs; seam-patched tests re-expressed as behavior |
| `test/unit/__init__.py` | out of port scope (harness infrastructure) |  |
| `test/unit/test_aid.py` | pending |  |
| `test/unit/test_claude_code_feature_mounts.py` | pending |  |
| `test/unit/test_devcontainer_manifest.py` | pending |  |
| `test/unit/test_devpod_provider.py` | pending |  |
| `test/unit/test_devpod_scoping.py` | out of port scope (harness infrastructure) | pins test/devpod_scoping.py, suite-side scoping |
| `test/unit/test_devpod_shim.py` | out of port scope (harness infrastructure) | pins the shim, which is the judge's own tooling |
| `test/unit/test_devpod_shim_fixture.py` | out of port scope (harness infrastructure) | pins the shim fixture wiring |
| `test/unit/test_devpod_ssh.py` | pending |  |
| `test/unit/test_disk_usage.py` | pending |  |
| `test/unit/test_dl_cmd_seam.py` | out of port scope (harness infrastructure) | pins the DEVLAUNCH_DL_CMD seam itself |
| `test/unit/test_docker_boundary.py` | pending |  |
| `test/unit/test_e2e_guard.py` | out of port scope (harness infrastructure) | pins the e2e guard fixture, which survives as the judge |
| `test/unit/test_e2e_workspace_helper.py` | out of port scope (harness infrastructure) | pins e2e_helpers.py, which survives as the judge |
| `test/unit/test_gh_auth.py` | pending |  |
| `test/unit/test_launch_serialization.py` | pending |  |
| `test/unit/test_locks.py` | re-pinned in Rust | domain/locks.rs; the Python-language API-shape guards have no Rust analogue |
| `test/unit/test_prune_orphaned_clones.py` | pending |  |
| `test/unit/test_purge_ownership.py` | pending |  |
| `test/unit/test_purge_partial_removal.py` | pending |  |
| `test/unit/test_reconcile_orphaned_workspaces.py` | pending |  |
| `test/unit/test_spec_parsing.py` | re-pinned in Rust | misnamed file: model/config serialization, covered by domain/model.rs + config.rs tests |
| `test/unit/test_stored_workspace_id.py` | pending |  |
| `test/unit/test_tools.py` | pending |  |
| `test/unit/test_tty_session.py` | pending |  |
| `test/unit/test_updater_fetch_sweep.py` | pending |  |
| `test/unit/test_workspace_listing.py` | pending |  |
| `test/unit/test_workspace_source.py` | pending |  |
| `test/unit/test_workspace_source_placement.py` | pending |  |
