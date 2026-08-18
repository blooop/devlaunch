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


class PendingRatchet(unittest.TestCase):
    def test_equal_passes(self):
        self.assertEqual(parity.lint_pending_ratchet(50, 50), [])

    def test_growth_fails(self):
        errs = parity.lint_pending_ratchet(51, 50)
        self.assertEqual(len(errs), 1)
        self.assertIn("grew", errs[0])

    def test_shrink_requires_recording(self):
        errs = parity.lint_pending_ratchet(49, 50)
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
        unexpected, _ = parity.compare_failures(
            failed=["test/test_new.py::test_z"], patterns=[]
        )
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


if __name__ == "__main__":
    unittest.main()
