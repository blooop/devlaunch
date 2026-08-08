"""Where the XDG base directories point on this machine.

Two unrelated places ask for the config home: the worktree loader, which reads
`config.toml` under it, and the gh-token warning, which names it so a user whose
shell scoped the variable can see why `gh auth token` refused. Those two have to
agree -- a warning that names one directory while the loader reads another is
worse than no warning -- so they share the answer rather than each spelling it.

The cache home has the same problem and one more caller's worth of it. Three
places used to spell it out identically: dl's own cache directory, the worktree
config's default `repos_dir`, and the metadata file's default path. They have to
agree because `dl --purge` reads the first to decide which workspaces are
devlaunch's -- workspaces whose clones the other two put on disk -- so a copy
that drifted would make a purge silently stop recognising its own work.
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


def cache_home() -> Path:
    """`$XDG_CACHE_HOME`, or the `~/.cache` the spec falls back to.

    Empty counts as unset, for the same reason as above.
    """
    return Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache")


def devlaunch_cache() -> Path:
    """Everything devlaunch stores on this machine, under one directory.

    The bare repo clones, the workspace clones, the completion caches and
    metadata.json all live here, and `dl --purge` removes exactly this. One
    function rather than a copy per caller, because a purge decides what is its
    own to delete by asking whether a workspace's source is inside it.
    """
    return cache_home() / "devlaunch"
