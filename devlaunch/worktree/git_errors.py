"""What to report when a git subprocess failed.

Imports nothing from this package, so every module that shells out to git can
reach it without arranging its imports around it.
"""

import subprocess


def _git_failure_reason(e: subprocess.CalledProcessError, verb: str) -> str:
    """What a failed ``git <verb>`` gives a caller to act on.

    Three things a caller of git needs, and they are here once rather than at
    each arm that shells out:

    - ``CalledProcessError.stderr`` is ``None`` whenever the output was never
      captured, so it is guarded before anything reads it. An arm that
      classifies the failure by looking for a phrase in the text -- "already
      exists", "couldn't find remote ref" -- would otherwise raise ``TypeError``
      out of a path whose whole job is to report a failure, naming neither the
      original problem nor anything to do about it (#212, #225).
    - The absence of stderr is reported as the exit code rather than as itself.
      Interpolated raw it reads "...: None", and read as text it reads "...: "
      with nothing after the colon; both tell whoever hits them only that
      something went wrong, which the raised error already said. The exit code
      is what is left to say about a command git was silent about.
    - The text is stripped, because git's messages end in a newline and these
      end up interpolated mid-sentence.

    *verb* is the git subcommand as it would be typed (``push``, ``fetch``,
    ``branch``); it appears only in the fallback, which is why it is passed
    rather than recovered from ``e.cmd`` -- that carries whatever the caller
    happened to hand ``subprocess.run``.
    """
    return (e.stderr or "").strip() or f"git {verb} exited {e.returncode}"
