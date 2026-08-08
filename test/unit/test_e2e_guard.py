"""The e2e guard: telling "opted out by design" apart from "could not run".

A run of the e2e suite against a registry serving at 640 B/s once reported
`7 passed, 14 skipped` having created zero containers -- byte-identical to a
healthy run, because "the workspace image never arrived" and "these tests were
not asked for" are both spelled `skipped`. These tests pin the two pieces that
make those two runs distinguishable: a word for the deliberate case, and a
tally of what the session actually did.
"""

import pytest

from fixtures.e2e_guard import E2ELedger, is_declared_opt_out, opt_out


class TestDeclaredOptOut:
    """One of the two meanings of `skipped` gets a name; the other does not."""

    def test_an_opt_out_is_recognisable_as_deliberate(self):
        with pytest.raises(pytest.skip.Exception) as raised:
            opt_out("set DEVLAUNCH_E2E_WORKSPACE to run these")

        assert is_declared_opt_out(str(raised.value))

    def test_an_opt_out_still_says_why_in_words(self):
        with pytest.raises(pytest.skip.Exception) as raised:
            opt_out("set DEVLAUNCH_E2E_WORKSPACE to run these")

        assert "set DEVLAUNCH_E2E_WORKSPACE to run these" in str(raised.value)

    def test_an_ordinary_skip_is_not_a_declared_opt_out(self):
        """The whole point: anything nobody declared is an unexplained absence."""
        assert not is_declared_opt_out("Skipped: DevPod not available")

    def test_an_empty_report_is_not_a_declared_opt_out(self):
        assert not is_declared_opt_out("")


class TestSessionFloor:
    """A green run has to have done something, and say what."""

    def test_a_run_that_created_no_workspaces_falls_short(self):
        ledger = E2ELedger()
        ledger.record_test_attempted()
        ledger.record_test_attempted()

        assert ledger.shortfalls()

    def test_the_shortfall_says_no_workspaces_were_created(self):
        ledger = E2ELedger()
        ledger.record_test_attempted()

        assert "no workspaces" in " ".join(ledger.shortfalls())

    def test_a_run_that_created_a_workspace_clears_the_floor(self):
        ledger = E2ELedger()
        ledger.record_test_attempted()
        ledger.record_workspace_created("e2e-test-create")

        assert ledger.shortfalls() == []

    def test_a_run_with_no_e2e_tests_in_it_is_not_judged(self):
        """`pixi run test` collects this directory and deselects it; the floor
        is about e2e runs that happened, not about runs that never asked."""
        assert E2ELedger().shortfalls() == []


class TestSessionTally:
    """The line that makes a healthy run and an outage run read differently."""

    def test_the_tally_names_every_workspace_the_run_built(self):
        ledger = E2ELedger()
        ledger.record_test_attempted()
        ledger.record_workspace_created("e2e-test-create")
        ledger.record_workspace_created("e2e-test-purge")

        summary = ledger.summary()

        assert "e2e-test-create" in summary
        assert "e2e-test-purge" in summary

    def test_the_tally_counts_the_tests_that_were_attempted(self):
        ledger = E2ELedger()
        ledger.record_test_attempted()
        ledger.record_test_attempted()

        assert "2" in ledger.summary()

    def test_a_run_that_built_nothing_says_so_rather_than_saying_nothing(self):
        ledger = E2ELedger()
        ledger.record_test_attempted()

        assert "no workspaces" in ledger.summary()
