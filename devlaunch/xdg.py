"""Where the XDG base directories point on this machine.

Two unrelated places ask for the config home: the worktree loader, which reads
`config.toml` under it, and the gh-token warning, which names it so a user whose
shell scoped the variable can see why `gh auth token` refused. Those two have to
agree -- a warning that names one directory while the loader reads another is
worse than no warning -- so they share the answer rather than each spelling it.
"""

import os
from pathlib import Path


def config_home() -> Path:
    """`$XDG_CONFIG_HOME`, or the `~/.config` the spec falls back to.

    An empty value counts as unset, which is what the XDG basedir spec says and
    what a shell exporting the variable with no value means. Reading it any other
    way resolves the config path relative to the working directory instead.
    """
    return Path(os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config")
