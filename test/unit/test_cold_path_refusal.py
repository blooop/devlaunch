"""What a run whose records will not open tells the person who typed it.

The cold path is the config, `metadata.json`, the cache migration and the clone
manager, opened together and only by a command that needs them. When that open
fails there is nothing else dl can do for the launch, so the whole of the
outcome is the sentence it refuses with.

Judged from outside, through the binary, because that is the only place the two
halves meet: since #340 the reason travels out of `devlaunch-core` as a type
(`ColdRefused::Startup(StartupError::Metadata(..))`, pinned in
`flows::launch`'s own tests) and the words are written in `dl`'s renderer
(pinned in `render`'s). Nothing inside either crate can say that the real
`ColdPath` produces the arm the renderer is given -- the open resolves its paths
from the process environment, so only a real process with a scoped one runs it.

The refusal is provoked by putting a *file* where dl's cache directory belongs.
`MetadataStorage::open` creates the directory its store lives in before it does
anything else, and it cannot create that one.
"""

import os
import subprocess
from pathlib import Path

from fixtures.e2e_helpers import dl_command


def cache_dir_is_a_file() -> Path:
    """Make the one directory dl keeps everything in impossible to create."""
    path = Path(os.environ["XDG_CACHE_HOME"]) / "devlaunch"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("not a directory\n")
    return path


def run_dl(devpod_shim, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [*dl_command(), *args],
        env=devpod_shim.env(),
        capture_output=True,
        text=True,
        check=False,
    )


class TestRecordsThatWillNotOpen:
    def test_the_launch_says_which_of_the_three_things_went_wrong(self, devpod_shim):
        """The reason, not a generic failure: this one is fixed on the filesystem."""
        blocked = cache_dir_is_a_file()

        result = run_dl(devpod_shim, "blooop/devlaunch")

        assert result.returncode != 0
        assert "could not create the directory for dl's records" in result.stderr, result.stderr
        assert str(blocked) in result.stderr, result.stderr

    def test_the_reason_is_quoted_into_the_line_the_launch_refuses_with(self, devpod_shim):
        """`Repository 'owner/repo': <reason>` is Python's sentence and still is.

        This is what the typed refusal has to compose into. The reason phrase
        carries no prefix of its own precisely so this line reads as one
        sentence, which is why the renderer is asked for a phrase and the caller
        writes the opening.
        """
        cache_dir_is_a_file()

        result = run_dl(devpod_shim, "blooop/devlaunch")

        said = [line for line in result.stderr.splitlines() if line.startswith("Repository ")]
        assert said, result.stderr
        assert said[0].startswith("Repository 'blooop/devlaunch': could not create the directory")

    def test_nothing_was_asked_of_devpod_for_a_workspace_that_cannot_be_named(
        self, devpod_shim
    ):
        """The refusal comes before the round trips, so there is nothing to clean up.

        A `devpod up` here would leave a container behind for a launch that never
        got as far as knowing which branch it was for.
        """
        cache_dir_is_a_file()

        run_dl(devpod_shim, "blooop/devlaunch")

        assert [call for call in devpod_shim.calls() if "up" in call] == []
