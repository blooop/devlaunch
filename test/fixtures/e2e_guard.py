"""Make an e2e run say what it actually did, not just that it finished.

`skipped` means two unrelated things in this suite, and the summary line cannot
tell them apart. One is *opted out by design*: `test_interactive_session.py`
wants a workspace named in `DEVLAUNCH_E2E_WORKSPACE` and there is none, so those
thirteen tests do not apply. The other is *could not run*: a registry served the
1.25 GB fixture image at 640 B/s, the first workspace test gave up, and the run
reported `7 passed, 14 skipped` having created no containers at all -- the same
line a healthy run prints. Thirteen declared skips is exactly the noise floor
that makes the fourteenth invisible.

This module gives the deliberate case a word of its own, so everything else that
skips can be treated as the absence it is, and keeps a tally of what the session
built so that a green run has to name something.

None of it is a substitute for the tests failing when they should. It is for the
case where nothing failed and nothing happened either.
"""

from typing import List, NoReturn

import pytest

# Carried inside the skip message rather than a marker or a registry, because
# the report a hook sees at the far end of the run is text: whatever survives
# into the terminal is what can be checked. It reads as a sentence in the output
# too, which is where a human meets it.
DECLARED_OPT_OUT = "declared e2e opt-out: "


def opt_out(reason: str) -> NoReturn:
    """Skip because this test does not apply here, and say so in that word.

    Use this only when the environment is *entitled* to lack what the test
    needs -- an opt-in the caller did not take, a precondition that genuinely
    does not exist on this machine. Anything the test needed and could not get
    is a failure; `pytest.skip` for those is what let a broken run look clean.
    """
    pytest.skip(DECLARED_OPT_OUT + reason)


def is_declared_opt_out(report_text: str) -> bool:
    """Whether a skipped test's report says somebody chose that skip."""
    return DECLARED_OPT_OUT in report_text


def _plural(count: int, noun: str) -> str:
    return f"{count} {noun}" if count == 1 else f"{count} {noun}s"


class E2ELedger:
    """What the e2e session actually did, kept alongside what it reported.

    Workspaces are recorded at the one place that creates them, because that is
    the only place that knows. Nothing downstream can infer a container from a
    passing test -- that inference is precisely what the outage run got wrong.
    """

    def __init__(self) -> None:
        self.tests_attempted = 0
        self.workspaces_created: List[str] = []

    def record_test_attempted(self) -> None:
        self.tests_attempted += 1

    def record_workspace_created(self, workspace_id: str) -> None:
        self.workspaces_created.append(workspace_id)

    def summary(self) -> str:
        """One line that reads differently for a healthy run and a dead one."""
        attempted = _plural(self.tests_attempted, "e2e test")
        if not self.workspaces_created:
            return f"{attempted} attempted, no workspaces created"
        built = ", ".join(self.workspaces_created)
        created = _plural(len(self.workspaces_created), "workspace")
        return f"{attempted} attempted, {created} created: {built}"

    def shortfalls(self) -> List[str]:
        """Reasons this session must not be believed. Empty means believe it.

        A session with no e2e tests in it is not judged: `pixi run test`
        collects this directory and deselects it, and a run that never asked
        for e2e is not a run that failed to do e2e. A run where every e2e test
        was collected and none of them built anything is the failure this
        exists for -- and pytest, having seen no assertion fail, would call it
        a success.
        """
        if self.tests_attempted == 0:
            return []
        if not self.workspaces_created:
            return [
                "the e2e session created no workspaces at all, so nothing it "
                "reports is evidence that devpod or Docker work. A slow or "
                "unreachable registry looks exactly like this."
            ]
        return []


# One session per process, so one ledger. It is shared between the helper that
# creates workspaces and the hooks that report on the run, which live in
# different modules only because pytest wants its hooks in a conftest.
LEDGER = E2ELedger()
