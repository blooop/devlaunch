"""What a `config.toml` naming a key this build no longer reads gets told.

`worktree.repos_dir` used to decide where dl put its clones (#467, carrying out
[#460](https://github.com/blooop/devlaunch/issues/460)). It is retired, and a
user who set it has a real clone tree at a path dl will stop looking at. Nothing
is moved and nothing is deleted, so the whole of the migration is that the tree
is *named*: silence is what would strand it.

Judged from outside, through the binary, because "the user is told" is a claim
about what a run prints and not about what a function returns.

The notice rides with dl's records, which is every command that opens the cache:
a launch, `--ls --json`, `--prune`, `--reconcile`. `dl --ls`'s table is
deliberately not one of them -- it reads devpod and nothing else, config
included, which is what keeps a listing to one round trip.
"""

import os
import subprocess
from pathlib import Path

from fixtures.e2e_helpers import dl_command

CONFIGURED = "/srv/devlaunch-clones"


def write_config(text: str) -> Path:
    """A `config.toml` where dl looks for one, under the suite's scoped config home."""
    path = Path(os.environ["XDG_CONFIG_HOME"]) / "devlaunch" / "config.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    return path


def run_dl(devpod_shim, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [*dl_command(), *args],
        env=devpod_shim.env(),
        capture_output=True,
        text=True,
        check=False,
    )


class TestTheRetiredReposDirIsNamed:
    def test_the_directory_it_named_is_printed(self, devpod_shim):
        """With the path, so whoever set it can find the tree and move or delete it."""
        write_config(f'[worktree]\nrepos_dir = "{CONFIGURED}"\n')

        result = run_dl(devpod_shim, "--ls", "--json")

        assert result.returncode == 0, result.stderr
        assert "repos_dir" in result.stderr
        assert CONFIGURED in result.stderr
        # And it points at the thing that does move the cache now.
        assert "XDG_CACHE_HOME" in result.stderr

    def test_it_is_said_once(self, devpod_shim):
        write_config(f'[worktree]\nrepos_dir = "{CONFIGURED}"\n')

        result = run_dl(devpod_shim, "--ls", "--json")

        assert result.stderr.count(CONFIGURED) == 1, result.stderr

    def test_it_is_not_an_error(self, devpod_shim):
        """A stale file is not punished: the run carries on with its other keys."""
        write_config(f'[worktree]\nrepos_dir = "{CONFIGURED}"\nfetch_interval = 7200\n')

        result = run_dl(devpod_shim, "--ls", "--json")

        assert result.returncode == 0, result.stderr
        assert result.stdout.strip() == "[]"

    def test_the_maintenance_commands_are_told_too(self, devpod_shim):
        """Not one command's line: it rides with the records, so every command that
        opens dl's cache says it. A launch is the one that matters most and is the
        same seam, but a launch reaches the network and this suite does not."""
        write_config(f'[worktree]\nrepos_dir = "{CONFIGURED}"\n')

        result = run_dl(devpod_shim, "--prune", "-y")

        assert CONFIGURED in result.stderr

    def test_a_config_that_does_not_name_it_says_nothing(self, devpod_shim):
        write_config("[worktree]\nfetch_interval = 7200\n")

        result = run_dl(devpod_shim, "--ls", "--json")

        assert result.returncode == 0, result.stderr
        assert "repos_dir" not in result.stderr

    def test_no_config_at_all_says_nothing(self, devpod_shim):
        result = run_dl(devpod_shim, "--ls", "--json")

        assert result.returncode == 0, result.stderr
        assert "repos_dir" not in result.stderr
