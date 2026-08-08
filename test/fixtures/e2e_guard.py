"""Make an e2e run say what it actually did, not just that it finished.

`skipped` means two unrelated things in this suite, and the summary line cannot
tell them apart. One is *opted out by design*: `test_interactive_session.py`
wants a workspace named in `DEVLAUNCH_E2E_WORKSPACE` and there is none, so those
thirteen tests do not apply. The other is *could not run*: a registry served the
1.25 GB fixture image at 640 B/s, the first workspace test gave up, and the run
reported `7 passed, 14 skipped` having created no containers at all -- the same
line a healthy run prints. Thirteen declared skips is exactly the noise floor
that makes the fourteenth invisible.

This module gives the deliberate case a type of its own, so everything else that
skips can be treated as the absence it is, and keeps a tally of what the session
built so that a green run has to name something.

None of it is a substitute for the tests failing when they should. It is for the
case where nothing failed and nothing happened either.
"""

from typing import List, NoReturn, Optional


# What `pytest.skip` raises -- the same object as `pytest.skip.Exception`, taken
# from the module that defines it because this one subclasses it, and a class
# reached through a function attribute is not one a type checker can inherit
# from. Nothing else in this file touches `_pytest`.
from _pytest.outcomes import Skipped


class DeclaredOptOut(Skipped):
    """A skip somebody chose, told apart from one that merely happened.

    The distinction is carried by the exception's *type*. The hook that judges
    a skip has the live exception -- `call.excinfo` -- so it can ask what was
    raised instead of reading the text pytest printed about it, and the type
    answers two questions the text got wrong:

    - An `xfail`ing test also reports `skipped`, but what it raised is the
      assertion that failed, not a `Skipped`. Matching on text would have
      rewritten every future `@pytest.mark.xfail` under `test/e2e/` into a hard
      failure; matching on type lets it through untouched.
    - Nothing can spell its way into an opt-out by writing a plausible reason
      into an ordinary `pytest.skip`.

    `_use_item_location` puts the `-ra` line against the test that opted out
    rather than against this file, which is where a reader of a skip reason
    wants to be sent. It is the same flag `@pytest.mark.skip` uses.
    """

    def __init__(self, reason: str, *, module_level: bool = False) -> None:
        super().__init__(
            msg=f"declared e2e opt-out: {reason}",
            allow_module_level=module_level,
            _use_item_location=True,
        )


def opt_out(reason: str, *, module_level: bool = False) -> NoReturn:
    """Skip because this test does not apply here, and say so in that word.

    Use this only when the environment is *entitled* to lack what the test
    needs -- an opt-in the caller did not take, a precondition that genuinely
    does not exist on this machine. Anything the test needed and could not get
    is a failure; `pytest.skip` for those is what let a broken run look clean.

    `module_level=True` is the collection-time form, for a whole module that
    does not apply. It exists because an undeclared module-level skip is
    reported as a collection error, and a module that is entitled to decline
    still needs a way to say so.
    """
    raise DeclaredOptOut(reason, module_level=module_level)


def is_undeclared_skip(excinfo) -> bool:
    """Whether an outcome is a skip nobody declared -- an absence, not a choice.

    Takes the exception, not the report text: `pytest_runtest_makereport` and
    `pytest_make_collect_report` both have it, and it is the only form of the
    question that cannot be answered wrongly by something that merely looks
    like a skip in print.
    """
    if excinfo is None:
        return False
    return excinfo.errisinstance(Skipped) and not excinfo.errisinstance(DeclaredOptOut)


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
        self.builders_attempted = 0
        self.workspaces_created: List[str] = []

    def record_test_attempted(self, *, builds_workspace: bool) -> None:
        self.tests_attempted += 1
        if builds_workspace:
            self.builders_attempted += 1

    def record_workspace_created(self, workspace_id: str) -> None:
        self.workspaces_created.append(workspace_id)

    def summary(self) -> str:
        """One line that reads differently for each thing that can have happened."""
        attempted = _plural(self.tests_attempted, "e2e test")
        if self.workspaces_created:
            built = ", ".join(self.workspaces_created)
            created = _plural(len(self.workspaces_created), "workspace")
            return f"{attempted} attempted, {created} created: {built}"
        if self.builders_attempted == 0:
            return f"{attempted} attempted, none of which builds a workspace"
        return f"{attempted} attempted, no workspaces created"

    def shortfall(self) -> Optional[str]:
        """Why this session must not be believed, or None to believe it.

        The question is not "did any e2e test run" but "did the tests that
        promised a container produce one". Those are different runs, and
        conflating them fails an honest one: `pytest -m e2e
        test/e2e/test_interactive_session.py` with `DEVLAUNCH_E2E_WORKSPACE`
        set attaches to a workspace somebody else built, correctly creates
        none, and has nothing to answer for.

        Which tests promise a container is declared by them, with
        `@pytest.mark.creates_workspace`, rather than guessed at from the far
        end of the run -- the guess is what would have to be wrong for a green
        tick to be wrong.
        """
        if self.builders_attempted == 0:
            return None
        if not self.workspaces_created:
            return (
                "the e2e session ran tests that build workspaces and finished with "
                "none built, so nothing it reports is evidence that devpod or Docker "
                "work. A slow or unreachable registry looks exactly like this."
            )
        return None


# One session per process, so one ledger. It is shared between the helper that
# creates workspaces and the hooks that report on the run, which live in
# different modules only because pytest wants its hooks in a conftest.
LEDGER = E2ELedger()
