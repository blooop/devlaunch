"""Make the run's private devpod home usable, and make the run tell the truth.

Two jobs, both scoped to this directory so no other kind of test pays for them.

The first is setup. The root conftest points `DEVPOD_HOME` at a directory
created for this run. A brand new devpod home has no providers in it at all, so
nothing can be brought up there until one is installed. Doing that here, autouse
and session-scoped, means every test in this directory gets it without asking.

The second is the guard described in `fixtures/e2e_guard.py`: any skip nobody
declared becomes a failure, and a session that built no workspaces does not get
to exit zero. Both live in hooks rather than fixtures for the same reason the
devpod scoping does -- a fixture is something a test has to ask for, and the
test that must not forget is the one nobody has written yet.
"""

import os
import subprocess

import pytest

from devpod_scoping import DEVPOD_HOME_VAR
from fixtures.e2e_guard import LEDGER, is_declared_opt_out
from fixtures.e2e_helpers import require_devpod


@pytest.fixture(scope="session", autouse=True)
def docker_provider_in_scoped_devpod_home():
    """Install the docker provider into this run's devpod home.

    `--use` rewrites the *default provider* of whichever devpod home is live, so
    this is the one unconditional write to a devpod home the suite performs. Its
    safety rests entirely on the scoping the root conftest sets up, which makes
    it the one place worth asserting that scoping rather than assuming it: if
    DEVPOD_HOME ever stops being set, this fixture is what would reach into the
    developer's own ~/.devpod.

    The install is a precondition, not a teardown, so a failure is raised rather
    than swallowed -- otherwise every later e2e test fails against a
    provider-less devpod home with an unrelated error, while the stderr that
    explains it was captured and discarded.

    Whether devpod exists at all is checked here too, once, rather than by each
    test that needs it. It used to be per-test and it used to skip, which meant
    a machine with no devpod on it reported an e2e suite that passed. Nothing
    downstream needs its own guard now: a session that gets past this fixture
    has a devpod and a provider.
    """
    assert os.environ.get(DEVPOD_HOME_VAR), (
        "refusing to add a devpod provider: DEVPOD_HOME is unset, so `--use` "
        "would rewrite the default provider in the developer's real ~/.devpod"
    )

    require_devpod()

    result = subprocess.run(
        ["devpod", "provider", "add", "docker", "--use"],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        pytest.fail(
            f"could not install the docker provider into {os.environ[DEVPOD_HOME_VAR]}:\n{result.stderr}"
        )


UNDECLARED_SKIP = (
    "This e2e test skipped without declaring an opt-out, which means something "
    "it needed was missing rather than not wanted. Skips that are deliberate go "
    "through fixtures.e2e_guard.opt_out and say so; everything else is an "
    "absence, and an absence that reports as `skipped` is how a run with a dead "
    "registry in it passes. Original reason:\n"
)


@pytest.hookimpl(wrapper=True)
def pytest_runtest_makereport(item, call):  # noqa: ARG001  # pylint: disable=unused-argument
    """Count what was attempted, and turn undeclared skips into failures.

    Rewriting the report rather than checking at the end of the session is
    deliberate: the complaint this whole guard answers is that the summary line
    lies, so the correction has to land in the summary line. A test that could
    not run shows up as `F` against its own name, where a reader will look.
    """
    report = yield

    if report.when == "setup":
        LEDGER.record_test_attempted()

    if report.skipped and not is_declared_opt_out(str(report.longrepr)):
        report.outcome = "failed"
        report.longrepr = UNDECLARED_SKIP + str(report.longrepr)

    return report


def pytest_sessionfinish(session, exitstatus):  # noqa: ARG001  # pylint: disable=unused-argument
    """Refuse to hand back success for a session that did nothing.

    Every other check in pytest answers "did anything fail". This one answers
    "did anything happen", which is the question a slow registry gets wrong: no
    assertion fails when no container is ever built, so the run is a success by
    every measure the tool has.
    """
    shortfalls = LEDGER.shortfalls()
    if shortfalls:
        session.exitstatus = pytest.ExitCode.TESTS_FAILED


def pytest_terminal_summary(terminalreporter, exitstatus, config):  # noqa: ARG001  # pylint: disable=unused-argument
    """Print what the run built, so two different runs read differently.

    Without this a healthy run and one that created nothing print the same
    summary line. This is the line to read when a green tick is in doubt.
    """
    if LEDGER.tests_attempted == 0:
        return

    terminalreporter.write_sep("-", "e2e session")
    terminalreporter.write_line(LEDGER.summary())
    for shortfall in LEDGER.shortfalls():
        terminalreporter.write_line(f"FAILED: {shortfall}", red=True, bold=True)
