"""The e2e guard: telling "opted out by design" apart from "could not run".

A run of the e2e suite against a registry serving at 640 B/s once reported
`7 passed, 14 skipped` having created zero containers -- byte-identical to a
healthy run, because "the workspace image never arrived" and "these tests were
not asked for" are both spelled `skipped`. These tests pin the two pieces that
make those two runs distinguishable: a type for the deliberate case, and a
tally of what the session actually did.
"""

import pytest

from fixtures.e2e_guard import DeclaredOptOut, E2ELedger, is_undeclared_skip, opt_out


class TestDeclaredOptOut:
    """One of the two meanings of `skipped` gets a type; the other does not."""

    def test_an_opt_out_is_recognisable_as_deliberate(self):
        with pytest.raises(pytest.skip.Exception) as raised:
            opt_out("set DEVLAUNCH_E2E_WORKSPACE to run these")

        assert not is_undeclared_skip(raised)

    def test_an_opt_out_still_says_why_in_words(self):
        with pytest.raises(pytest.skip.Exception) as raised:
            opt_out("set DEVLAUNCH_E2E_WORKSPACE to run these")

        assert "set DEVLAUNCH_E2E_WORKSPACE to run these" in str(raised.value)

    def test_an_ordinary_skip_is_not_a_declared_opt_out(self):
        """The whole point: anything nobody declared is an unexplained absence."""
        with pytest.raises(pytest.skip.Exception) as raised:
            pytest.skip("DevPod not available")

        assert is_undeclared_skip(raised)

    def test_a_skip_reason_cannot_talk_its_way_into_being_an_opt_out(self):
        """The check is the type, not the wording, so wording cannot forge it."""
        with pytest.raises(pytest.skip.Exception) as raised:
            pytest.skip("declared e2e opt-out: honestly, trust me")

        assert is_undeclared_skip(raised)

    def test_a_failing_assertion_is_not_a_skip_at_all(self):
        """`report.skipped` is also true for xfail, and what an xfailing test
        raises is the assertion that failed. Judging the exception rather than
        the report is what keeps a future `@pytest.mark.xfail` under test/e2e/
        from being rewritten into a hard failure."""
        with pytest.raises(AssertionError) as raised:
            assert False, "the thing under test is broken"

        assert not is_undeclared_skip(raised)

    def test_no_exception_at_all_is_not_a_skip(self):
        assert not is_undeclared_skip(None)

    def test_an_opt_out_reports_against_the_test_that_made_it(self):
        """Otherwise `-ra` names this helper as the thing that opted out, and a
        reader chasing a skip reason is sent to the wrong file."""
        assert DeclaredOptOut("because")._use_item_location  # pylint: disable=protected-access

    def test_a_module_can_decline_at_collection_time(self):
        """An undeclared module-level skip is a collection error, so the
        deliberate form of it needs to exist."""
        assert DeclaredOptOut("because", module_level=True).allow_module_level


class TestSessionFloor:
    """A green run has to have done what it said it would, and say what."""

    def test_a_run_whose_workspace_tests_built_nothing_falls_short(self):
        ledger = E2ELedger()
        ledger.record_test_attempted(builds_workspace=True)
        ledger.record_test_attempted(builds_workspace=False)

        assert ledger.shortfall() is not None

    def test_the_shortfall_says_no_workspaces_were_built(self):
        ledger = E2ELedger()
        ledger.record_test_attempted(builds_workspace=True)

        shortfall = ledger.shortfall()

        assert shortfall is not None and "none built" in shortfall

    def test_a_run_that_created_a_workspace_clears_the_floor(self):
        ledger = E2ELedger()
        ledger.record_test_attempted(builds_workspace=True)
        ledger.record_workspace_created("e2e-test-create")

        assert ledger.shortfall() is None

    def test_a_run_with_no_e2e_tests_in_it_is_not_judged(self):
        """`pixi run test` collects this directory and deselects it; the floor
        is about e2e runs that happened, not about runs that never asked."""
        assert E2ELedger().shortfall() is None

    def test_a_run_of_only_attach_tests_is_not_judged_either(self):
        """`pytest -m e2e test/e2e/test_interactive_session.py` with
        DEVLAUNCH_E2E_WORKSPACE set attaches to a workspace somebody else built.
        Creating none is the correct behaviour, and a floor that reddened it
        would be this PR's own complaint in reverse."""
        ledger = E2ELedger()
        ledger.record_test_attempted(builds_workspace=False)
        ledger.record_test_attempted(builds_workspace=False)

        assert ledger.shortfall() is None


class TestSessionTally:
    """The line that makes a healthy run and an outage run read differently."""

    def test_the_tally_names_every_workspace_the_run_built(self):
        ledger = E2ELedger()
        ledger.record_test_attempted(builds_workspace=True)
        ledger.record_workspace_created("e2e-test-create")
        ledger.record_workspace_created("e2e-test-purge")

        summary = ledger.summary()

        assert "e2e-test-create" in summary
        assert "e2e-test-purge" in summary

    def test_the_tally_counts_the_tests_that_were_attempted(self):
        ledger = E2ELedger()
        ledger.record_test_attempted(builds_workspace=False)
        ledger.record_test_attempted(builds_workspace=False)

        assert "2" in ledger.summary()

    def test_a_run_that_built_nothing_says_so_rather_than_saying_nothing(self):
        ledger = E2ELedger()
        ledger.record_test_attempted(builds_workspace=True)

        assert "no workspaces created" in ledger.summary()

    def test_a_run_with_nothing_to_build_says_that_instead(self):
        """Three situations, three lines: built things, built nothing it should
        have, and had nothing to build. Only the middle one is a problem, and
        the reader should not have to work out which they are looking at."""
        ledger = E2ELedger()
        ledger.record_test_attempted(builds_workspace=False)

        assert "none of which builds a workspace" in ledger.summary()
