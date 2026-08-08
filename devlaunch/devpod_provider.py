"""Answer "is this devpod provider already registered?" from devpod itself.

The question used to be asked by grepping devpod's human-facing table:

    devpod provider list | grep -qw docker || devpod provider add docker

devpod v0.26.1 colourises that table unconditionally and emits the reset
sequence ``ESC[m`` immediately before each cell, so the character preceding
``docker`` is the letter ``m``, ``grep -w``'s left word boundary never matches,
and the guard reported "absent" on every machine that had already registered the
provider. ``NO_COLOR=1`` does not help.

Dropping ``-w`` would have fixed today's rendering and left us reading a table
devpod is free to re-render tomorrow. ``provider list --output json`` is the
same information in a form that has no rendering to change: an object keyed by
provider name.

The other half of the old guard's failure is the part worth keeping out on
purpose. Output it could not read came back as an empty set of providers, and an
empty set of providers means "go add one" -- so an unreadable answer turned into
an action. Here, unreadable is its own outcome: `UnreadableProviderList`. The
caller finds out, rather than being told something false.
"""

import json
import subprocess
from typing import Callable, Optional, Sequence, Set


class UnreadableProviderList(Exception):
    """devpod's provider listing could not be read.

    Distinct from "no providers are registered", which is a listing that reads
    fine and is empty.

    Not a RuntimeError. It used to be, and the CLI below then had to name
    RuntimeError as well to catch the add failure -- which made the handler
    broad enough to report any RuntimeError in the process as a devpod problem.
    The two failures devpod can hand this module now have a type each, and the
    handler names both and nothing else.
    """


class ProviderAddFailed(Exception):
    """`devpod provider add` ran and failed.

    Distinct from a listing that could not be read: devpod answered the question
    it was asked, and then refused to do the thing.
    """


def parse_provider_names(listing: str) -> Set[str]:
    """The names of every registered provider in a `--output json` listing."""
    try:
        parsed = json.loads(listing)
    except json.JSONDecodeError as exc:
        raise UnreadableProviderList(
            f"devpod's provider listing is not JSON: {listing[:120]!r}"
        ) from exc
    if not isinstance(parsed, dict):
        raise UnreadableProviderList(
            f"expected devpod to list providers by name, got {type(parsed).__name__}"
        )
    return set(parsed)


def list_provider_names(
    run: Callable[..., subprocess.CompletedProcess] = subprocess.run,
) -> Set[str]:
    """Ask devpod which providers are registered."""
    result = run(
        ["devpod", "provider", "list", "--output", "json"],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise UnreadableProviderList(
            f"`devpod provider list` exited {result.returncode}: "
            f"{(result.stderr or '').strip()[:200]!r}"
        )
    return parse_provider_names(result.stdout or "")


def ensure_provider(
    name: str, run: Callable[..., subprocess.CompletedProcess] = subprocess.run
) -> bool:
    """Register `name` with devpod unless it is already registered.

    Returns True if it had to be added, False if it was already there. Raises
    `UnreadableProviderList` rather than guessing when devpod's answer cannot
    be read, and `ProviderAddFailed` when the add itself fails.
    """
    if name in list_provider_names(run=run):
        return False
    result = run(
        ["devpod", "provider", "add", name],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise ProviderAddFailed(
            f"`devpod provider add {name}` exited {result.returncode}: "
            f"{(result.stderr or '').strip()[:200]!r}"
        )
    return True


def main(
    argv: Optional[Sequence[str]] = None,
    run: Callable[..., subprocess.CompletedProcess] = subprocess.run,
) -> int:
    """CLI entry point: `python -m devlaunch.devpod_provider <name>`.

    This is what the `dev-add-docker` pixi task runs, so a devpod that cannot be
    read has to stop the task rather than let it carry on as if the provider
    were missing.
    """
    import argparse  # pylint: disable=import-outside-toplevel

    parser = argparse.ArgumentParser(
        prog="python -m devlaunch.devpod_provider",
        description="Register a devpod provider unless it is already registered.",
    )
    parser.add_argument("name", help="provider name, e.g. docker")
    args = parser.parse_args(argv)

    try:
        added = ensure_provider(args.name, run=run)
    except (UnreadableProviderList, ProviderAddFailed) as exc:
        print(f"error: {exc}")
        return 1
    print(f"devpod provider {args.name}: {'added' if added else 'already registered'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
