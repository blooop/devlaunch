"""Unit tests for parity.py's pure logic — stdlib only, no pytest.

Run via `python3 rust/parity.py self-test` (or `python3 -m unittest` from
rust/). These pin the manifest parser, the ledger parser, the disposition
rules, and the two-way ratchet comparison; the subprocess-driving parts of
parity.py are exercised by CI itself.
"""

import unittest

import parity


class ParseManifest(unittest.TestCase):
    def test_comments_and_blanks_are_ignored(self):
        m = parity.parse_manifest("# header\n\n# more\n")
        self.assertFalse(m.sentinel_all)
        self.assertEqual(m.patterns, [])

    def test_all_sentinel(self):
        m = parity.parse_manifest("# day one\nALL\n")
        self.assertTrue(m.sentinel_all)
        self.assertEqual(m.patterns, [])

    def test_patterns_survive(self):
        text = "# h\ntest/test_dl.py::test_x\ntest/unit/test_tools.py::*\n"
        m = parity.parse_manifest(text)
        self.assertFalse(m.sentinel_all)
        self.assertEqual(len(m.patterns), 2)

    def test_all_mixed_with_patterns_is_an_error(self):
        with self.assertRaises(parity.ManifestError):
            parity.parse_manifest("ALL\ntest/test_dl.py::test_x\n")


class ParseLedger(unittest.TestCase):
    LEDGER = """# Spec ledger

| file | disposition | notes |
|---|---|---|
| `test/test_workspace_id.py` | pending | |
| `test/test_bench_doc.py` | out of port scope | bench tooling |
| `test/conftest.py` | out of port scope (harness infrastructure) | |
| `test/test_x.py` | covered by divergence row #2 | clap strictness |
"""

    def test_rows_parse(self):
        rows = parity.parse_ledger(self.LEDGER)
        self.assertEqual(len(rows), 4)
        self.assertEqual(rows[0].file, "test/test_workspace_id.py")
        self.assertEqual(rows[0].disposition, "pending")

    def test_parenthetical_notes_on_out_of_scope_are_allowed(self):
        rows = parity.parse_ledger(self.LEDGER)
        self.assertEqual(rows[2].disposition, "out of port scope (harness infrastructure)")
        self.assertEqual(parity.base_disposition(rows[2].disposition), "out of port scope")

    def test_divergence_row_disposition_is_allowed(self):
        rows = parity.parse_ledger(self.LEDGER)
        errs = parity.lint_dispositions(rows)
        self.assertEqual(errs, [])

    def test_unknown_disposition_is_rejected(self):
        rows = parity.parse_ledger(
            "| file | disposition | notes |\n|---|---|---|\n| `test/a.py` | maybe later | |\n"
        )
        errs = parity.lint_dispositions(rows)
        self.assertEqual(len(errs), 1)
        self.assertIn("maybe later", errs[0])


class LedgerCoverage(unittest.TestCase):
    def test_missing_file_is_reported(self):
        rows = parity.parse_ledger(
            "| file | disposition | notes |\n|---|---|---|\n| `test/a.py` | pending | |\n"
        )
        errs = parity.lint_coverage(rows, ["test/a.py", "test/b.py"])
        self.assertEqual(len(errs), 1)
        self.assertIn("test/b.py", errs[0])

    def test_stale_row_is_reported(self):
        rows = parity.parse_ledger(
            "| file | disposition | notes |\n|---|---|---|\n"
            "| `test/a.py` | pending | |\n| `test/gone.py` | pending | |\n"
        )
        errs = parity.lint_coverage(rows, ["test/a.py"])
        self.assertEqual(len(errs), 1)
        self.assertIn("test/gone.py", errs[0])

    def test_duplicate_row_is_reported(self):
        rows = parity.parse_ledger(
            "| file | disposition | notes |\n|---|---|---|\n"
            "| `test/a.py` | pending | |\n| `test/a.py` | pending | |\n"
        )
        errs = parity.lint_coverage(rows, ["test/a.py"])
        self.assertEqual(len(errs), 1)
        self.assertIn("duplicate", errs[0])


class PendingCount(unittest.TestCase):
    def test_equal_passes(self):
        self.assertEqual(parity.lint_pending_count(50, 50), [])

    def test_growth_fails(self):
        errs = parity.lint_pending_count(51, 50)
        self.assertEqual(len(errs), 1)
        self.assertIn("grew", errs[0])

    def test_shrink_requires_recording(self):
        errs = parity.lint_pending_count(49, 50)
        self.assertEqual(len(errs), 1)
        self.assertIn("pending-count.txt", errs[0])


class FailureRatchet(unittest.TestCase):
    PATTERNS = ["test/test_dl.py::test_a", "test/unit/test_tools.py::*"]

    def test_expected_failure_matches(self):
        unexpected, stale = parity.compare_failures(
            failed=["test/test_dl.py::test_a"], patterns=self.PATTERNS
        )
        self.assertEqual(unexpected, [])
        self.assertEqual(stale, ["test/unit/test_tools.py::*"])

    def test_unexpected_failure_is_flagged(self):
        unexpected, _ = parity.compare_failures(failed=["test/test_new.py::test_z"], patterns=[])
        self.assertEqual(unexpected, ["test/test_new.py::test_z"])

    def test_glob_pattern_matches_node_ids(self):
        unexpected, stale = parity.compare_failures(
            failed=["test/unit/test_tools.py::test_q[1]"], patterns=self.PATTERNS
        )
        self.assertEqual(unexpected, [])
        self.assertEqual(stale, ["test/test_dl.py::test_a"])

    def test_parametrized_id_matches_its_bare_entry(self):
        unexpected, stale = parity.compare_failures(
            failed=["test/test_dl.py::test_a[case-two]"],
            patterns=["test/test_dl.py::test_a"],
        )
        self.assertEqual(unexpected, [])
        self.assertEqual(stale, [])


class ReconstructNodeId(unittest.TestCase):
    # The real test tree, as junit's dotted `classname` would report it.
    FILES = {
        "test/test_dl.py",
        "test/test_worktree_migration.py",
        "test/unit/test_tools.py",
    }

    def is_file(self, rel):
        return rel in self.FILES

    def test_module_level_function(self):
        # classname is the module's dotted import path, no class component.
        self.assertEqual(
            parity.reconstruct_node_id("test.test_dl", "test_x", self.is_file),
            "test/test_dl.py::test_x",
        )

    def test_class_based_test(self):
        # The regression H5 names: the old `.replace(".", "/") + ".py::"` turned
        # this into `test/test_worktree_migration/TestRenaming.py::...`, which
        # matched nothing. The class dot must stay a `::`, not a `/`.
        self.assertEqual(
            parity.reconstruct_node_id(
                "test.test_worktree_migration.TestRenaming",
                "test_dirs_are_renamed",
                self.is_file,
            ),
            "test/test_worktree_migration.py::TestRenaming::test_dirs_are_renamed",
        )

    def test_nested_directory(self):
        self.assertEqual(
            parity.reconstruct_node_id("test.unit.test_tools", "test_q", self.is_file),
            "test/unit/test_tools.py::test_q",
        )

    def test_parametrized_name_is_preserved(self):
        self.assertEqual(
            parity.reconstruct_node_id("test.test_dl", "test_x[case-2]", self.is_file),
            "test/test_dl.py::test_x[case-2]",
        )

    def test_unknown_file_falls_back_without_crashing(self):
        # No known file matches: legible best-effort id, never a dropped failure.
        self.assertEqual(
            parity.reconstruct_node_id("pkg.mystery", "test_z", self.is_file),
            "pkg/mystery.py::test_z",
        )


class CheckTierRan(unittest.TestCase):
    def test_healthy_tier_with_expected_failures_passes(self):
        # rc != 0 is normal while the manifest still expects failures.
        parity.check_tier_ran("tier1", rc=1, total=1800, floor=1000, n_failures=5)

    def test_all_green_tier_passes(self):
        parity.check_tier_ran("tier1", rc=0, total=1800, floor=1000, n_failures=0)

    def test_no_tests_collected_is_an_error(self):
        with self.assertRaises(parity.ManifestError) as cm:
            parity.check_tier_ran("tier1", rc=5, total=0, floor=1000, n_failures=0)
        self.assertIn("collected no tests", str(cm.exception))

    def test_below_floor_is_an_error(self):
        with self.assertRaises(parity.ManifestError) as cm:
            parity.check_tier_ran("tier2", rc=0, total=1, floor=3, n_failures=0)
        self.assertIn("below the floor", str(cm.exception))

    def test_nonzero_rc_with_no_junit_failures_is_an_error(self):
        # The session-floor / crash shape: exit code says failed, junit is clean.
        with self.assertRaises(parity.ManifestError) as cm:
            parity.check_tier_ran("tier2", rc=1, total=20, floor=3, n_failures=0)
        self.assertIn("lists no", str(cm.exception))


if __name__ == "__main__":
    unittest.main()
