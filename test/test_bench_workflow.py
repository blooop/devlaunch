"""The trend workflow's charter, guarded where it can be edited (#198).

Map #193's first decision is *alert-don't-gate*: this workflow notices a
regression and must never be able to block a merge on one, because a wall-clock
measurement on a shared runner is either too loose to fire or flaky. That is a
property of a YAML file, so it is one edit away from being lost — a
`pull_request:` trigger added for convenience, a `fail-on-alert: true` copied
from another project's example, the job wired into the CI gate's `needs`.

These are text assertions on config, deliberately: the invariants live in the
file's own words, there is no YAML parser in this project's dependencies, and
adding one to assert four strings would cost more than it guards. Each check
below is a thing somebody could plausibly write, with the reason it must not be
written.
"""

import os
from pathlib import Path

WORKFLOWS = Path(__file__).parent.parent / ".github" / "workflows"
BENCH = WORKFLOWS / "bench.yml"
CI = WORKFLOWS / "ci.yml"
BENCH_JOB = "bench"
RESET = Path(__file__).parent.parent / "scripts" / "bench_cold_reset.sh"
# How a step of the bench job begins, at the one indentation the file uses for
# them. What `bench_step` splits on.
STEP_MARKER = "      - name: "
# The host half of the shared pixi package cache, relative to the job's own
# XDG_CACHE_HOME. dl's constant, spelled here because the workflow has to name
# the same directory dl will mount and neither can import the other.
PIXI_CACHE_LEAF = "devlaunch/pixi"


def bench_workflow() -> str:
    assert BENCH.is_file(), f"{BENCH.name} is what publishes the trend"
    return BENCH.read_text(encoding="utf-8")


def bench_settings() -> str:
    """The workflow with its commentary removed.

    Half of what this file says about itself is a comment explaining a key it
    deliberately does *not* set, so a check for "this key is absent" has to
    read the settings rather than the prose about them.
    """
    return "\n".join(
        line for line in bench_workflow().splitlines() if not line.lstrip().startswith("#")
    )


def cold_reset() -> str:
    """The command the cold-recreate shape runs before every timed run.

    A line rather than a parse: `--before` takes one shell-quoted command and
    the step writes it on its own continuation line, so what this returns is
    that command and the properties below are about what it does.
    """
    lines = [line for line in bench_settings().splitlines() if "--before" in line]
    assert len(lines) == 1, "the cold-recreate shape is the only one that resets between runs"
    return lines[0]


def bench_step(name_fragment: str) -> str:
    """One step of the bench job, its commentary included, as it stands.

    Split on the step marker rather than parsed, for the reason the module
    docstring gives -- and the comments are kept because half of what a step of
    this workflow says is why it is there, and two of the checks below are about
    exactly that.
    """
    blocks = bench_workflow().split(STEP_MARKER)
    matching = [block for block in blocks[1:] if name_fragment in block.splitlines()[0]]
    assert len(matching) == 1, f"{name_fragment!r} names {len(matching)} steps, not one"
    return STEP_MARKER + matching[0]


def bench_step_commands(name_fragment: str) -> str:
    """One step of the bench job with its commentary removed.

    The mirror of `bench_settings` at one step's scale: most of what a step of
    this file contains is prose about why, so a check for "this step actually
    runs X" has to read the commands rather than the paragraph above them.
    """
    return "\n".join(
        line
        for line in bench_step(name_fragment).splitlines()
        if not line.lstrip().startswith("#")
    )


def reset_script() -> str:
    """What the cold reset actually does, which is a file rather than a string.

    The step names a script and the script holds the reset; both halves are
    asserted below, because a reset that is named but does nothing and a reset
    that does the right thing but is never named fail identically in CI.
    """
    assert RESET.is_file(), f"{RESET.name} is what re-establishes the cold state"
    return RESET.read_text(encoding="utf-8")


class TestItCannotBecomeAMergeGate:
    """Three independent ways this could turn into a required check, and the
    reason each stays shut."""

    def test_it_is_its_own_workflow_rather_than_a_job_in_the_gated_one(self):
        """House convention is that a new job in the CI workflow joins the
        gate's `needs` in the same pull request. A slow, noisy benchmark that
        did that would be exactly the merge gate the charter grilled out."""
        needs = [line for line in CI.read_text(encoding="utf-8").splitlines() if "needs:" in line]
        assert needs, "the CI gate's needs list is what this must stay out of"
        for line in needs:
            assert BENCH_JOB not in line, line

    def test_it_does_not_run_on_pull_requests_at_all(self):
        """Per-PR benching is out of scope under alert-don't-gate — the value
        is the trend line, not a pre-merge verdict — and a job that reports on
        a pull request is a job somebody can require."""
        assert "pull_request" not in bench_settings()

    def test_a_slow_point_alerts_and_does_not_fail_the_build(self):
        """`fail-on-alert` is the whole map in one setting."""
        text = bench_settings()
        assert "fail-on-alert: false" in text
        assert "fail-on-alert: true" not in text


class TestItMeasuresEveryCommitTheSameWay:
    def test_nothing_is_path_filtered(self):
        """A performance regression can arrive in any file, and a trend with
        holes in it cannot be compared against its own previous point."""
        text = bench_settings()
        assert "paths:" not in text
        assert "paths-ignore:" not in text

    def test_it_runs_on_main_and_on_demand(self):
        text = bench_settings()
        assert "branches: [main]" in text
        assert "workflow_dispatch:" in text

    def test_two_merges_cannot_bench_at_once(self):
        """The publishing action commits to a branch; concurrent runs race on
        that push, and two benches sharing one runner-class measure each
        other."""
        assert "concurrency:" in bench_settings()


class TestTheAlertReachesSomebody:
    def test_the_alert_comment_names_a_user(self):
        """On a push there is no pull request, so the action leaves a commit
        comment — and commit comments notify almost nobody. The @-mention is
        what actually generates a notification, so without it the alert is
        functionally invisible (#197)."""
        assert "alert-comment-cc-users" in bench_settings()


class TestTheRunnerStateIsScoped:
    def test_the_bench_writes_to_the_runners_own_scratch_state(self):
        """The job runs real launches, which create real containers and real
        devpod state. Both variables, because they scope different halves:
        `DEVPOD_HOME` is devpod's own, `XDG_CACHE_HOME` is devlaunch's clone
        cache and bookkeeping."""
        text = bench_settings()
        assert "DEVPOD_HOME:" in text
        assert "XDG_CACHE_HOME:" in text


class TestTheColdResetSurvivesAContainerWrittenClone:
    """A launch leaves its clone owned by the container's user — uid 1000 in
    the standard devcontainer base image — and a GitHub runner's own user is
    not that uid. `dl rm` then deletes the workspace, warns that the clone
    would not go, and exits 0; the next launch dies writing `.git/index.lock`
    in a clone it does not own (run 31838698495, follow-up to #198). The runner
    has passwordless sudo, so the reset takes the clone back before removing
    it. This is a property of a CI runner, not of `dl`, which is why the
    recovery lives here — in a script, for the reason the first test gives."""

    def test_the_reset_is_a_script_the_step_names_rather_than_quoting_inside_quoting(self):
        """`--before` reaches the bench through `pixi run bench`, and pixi's
        task shell re-joins and re-parses the arguments appended to a task —
        so a `bash -c '...'` nested inside the step's own quotes arrives with
        its inner quotes gone. Run 31840842480 is what that costs: argparse
        saw `-d` as a stray argument and exited 2, pixi's shell ran the tail
        after the `;` as a command of its own, and the STEP still passed with
        no record written. A bare path plus one argument has nothing left to
        re-parse."""
        reset = cold_reset()
        assert RESET.name in reset
        assert "bash -c" not in reset, "a nested shell is the quoting that did not survive"
        assert "'" not in reset, "a second level of quotes is what pixi's task shell eats"
        assert ";" not in reset, "pixi's shell reads a `;` in the argument as its own separator"

    def test_the_script_is_executable_so_the_bench_can_run_it_directly(self):
        """The bench splits `--before` and runs the list without a shell, so
        the file has to be runnable on its own."""
        assert os.access(RESET, os.X_OK), f"{RESET.name} is run as a command, not sourced"

    def test_the_reset_takes_the_clone_back_before_removing_it(self):
        reset = reset_script()
        assert "sudo chown" in reset
        assert "rm --force" in reset
        assert reset.index("sudo chown") < reset.index("rm --force"), (
            "chowning after the remove recovers nothing: the remove is what fails"
        )

    def test_the_reset_only_reclaims_this_jobs_own_clone_cache(self):
        """Scoped to the cache the job already scoped to `/tmp`. A recursive
        chown is a blunt instrument and a wider one would take ownership of
        things this job did not create."""
        assert "XDG_CACHE_HOME" in reset_script()

    def test_the_script_says_why_it_exists_where_somebody_would_delete_it(self):
        """A three-line script that chowns a cache reads like leftover
        scaffolding unless the uid mismatch that forces it is written down
        beside it."""
        assert "uid" in reset_script()

    def test_the_recovery_runs_per_run_rather_than_once_before_the_bench(self):
        """Every run recreates those files, so a one-time step before the bench
        recovers run 1 and leaves runs 2..5 with exactly the clone that broke
        it. `--before` is the only place with the right cardinality — so the
        script is named once in the workflow, and that once is there."""
        assert bench_settings().count(RESET.name) == 1, (
            "a second mention is a one-time step doing per-run work"
        )
        assert RESET.name in cold_reset()


class TestTheBenchedBinaryCanFindDevpod:
    """`dl` manages nothing without devpod, and nothing installs devpod here.

    devpod is a pixi dependency rather than a step, which the trigger section
    says and this job relies on: the steps that bench run `dl` from inside
    `pixi run`, so they inherit the environment's devpod for free. The priming
    launch does not -- it runs `dl` straight from the bare PATH, so it gets
    whatever the build step left there.

    Until this was pinned that was nothing. #267 moved the benched binary from
    `pixi run dl` to the release build on a bare PATH, correctly, and took
    devpod off that PATH in the same edit: `devpod not found on PATH: dl cannot
    manage workspaces without it`, exit 127, in the priming step. Every merge to
    main from that commit through 0.10.0 died there in 80 seconds having timed
    nothing -- 23 runs, 32588170961 back to 32355565253 -- while the 11 runs
    before it published points.

    A symlink into the directory the step already puts on PATH, rather than
    putting the whole pixi environment's `bin` there: that directory holds a
    python, a git and a couple of hundred other things, and what the benched
    launch resolves is part of what is being measured.
    """

    def test_devpod_is_put_on_the_same_path_dl_is(self):
        """The one assertion the regression would have failed."""
        commands = bench_step_commands("Build the release binaries")
        assert "devpod" in commands, "a `dl` with no devpod manages nothing"
        exported = [line for line in commands.splitlines() if "GITHUB_PATH" in line]
        assert len(exported) == 1, "one directory goes on PATH, and both binaries are in it"
        directory = exported[0].split(">>")[0].split()[-1]
        for binary in ("dl", "devpod"):
            assert f"{directory}/{binary}" in commands, f"{binary} is not in the directory on PATH"

    def test_it_is_the_environments_devpod_and_not_a_downloaded_one(self):
        """The version this tree is pinned against, not whatever a runner image
        or a release page happens to carry -- the same reason the trigger
        section gives for having no devpod install step. devpod 0.8 asks for a
        pty on `ssh --command` where 0.26 never does, so which one this is
        decides what the launch being timed actually does."""
        devpod = [
            line
            for line in bench_step_commands("Build the release binaries").splitlines()
            if "devpod" in line
        ]
        assert devpod, "the step has to find devpod somewhere"
        assert any("pixi" in line for line in devpod), "the lockfile is where devpod comes from"
        for line in devpod:
            assert "curl" not in line and "install.sh" not in line, line

    def test_the_priming_launch_is_what_needs_it_there(self):
        """Why the symlink cannot be deleted as redundant with `pixi run`: this
        step is the one launch in the job that runs outside it. Reverting it to
        `pixi run dl` is not the alternative fix -- that is the task, i.e. a
        debug build, and benching a debug build beside a release one is the
        wrong-build bug #267 came out of."""
        priming = bench_step_commands("Prime the image")
        run = [line for line in priming.splitlines() if line.lstrip().startswith("run:")]
        assert len(run) == 1, "the priming step is one launch"
        assert run[0].split("run:", 1)[1].strip().startswith("dl "), run[0]


class TestTheSharedPixiCacheIsWritableByTheContainer:
    """The other end of the same uid mismatch, and the one that stopped the
    trend dead.

    `dl` creates the shared pixi cache's host directory as the invoking user
    with the default umask and binds it into the container, which runs as the
    image's remoteUser -- uid 1000 in every mainstream devcontainer base, and
    not a GitHub runner's uid. `--workspace-env PIXI_CACHE_DIR` then points
    every pixi in the workspace at a directory it cannot write, and pixi does
    not degrade to a cache it can only read: the benched repo's `pixi install`
    postCreate fails, `devpod up` fails, and the priming step fails. Twenty
    consecutive runs, 31886967581 through 32397739366 -- every merge to main
    from the day #232 landed -- died there with `failed to create directory
    /var/tmp/devlaunch-pixi/pkgs: Permission denied`, having timed nothing.

    README's "shared pixi package cache" section documents the requirement as
    dl's own limitation: it cannot see the container's uid before it launches,
    so it cannot pick the mode for you. This job can, for the same reason the
    cold reset can chown -- one image, one remoteUser, an ephemeral runner, and
    a cache the job made under /tmp. So the recovery lives here, and these
    checks are what stop it being deleted as three lines of setup nobody needs.
    """

    def test_the_cache_is_widened_before_anything_launches(self):
        """dl's own mkdir is `exist_ok` and does not re-mode a directory it
        finds, so pre-creating is the whole mechanism -- and a step that did it
        after the priming launch would be recovering a job that had already
        failed."""
        text = bench_workflow()
        prepare = text.index(bench_step("write the shared pixi cache"))
        prime = text.index(bench_step("Prime the image"))
        assert prepare < prime, "the first launch is what needs the directory writable"

    def test_it_widens_the_directory_and_does_not_merely_create_it(self):
        """Creating it is what dl already does, as the runner, at 0755. The
        `chmod` is the whole of the difference between that and a container
        that can write. `1777` because it is what /var/tmp itself carries, and
        the sticky bit costs nothing where one container user creates
        everything under the leaf."""
        step = bench_step("write the shared pixi cache")
        assert "mkdir -p" in step
        assert "chmod 1777" in step

    def test_it_only_widens_the_jobs_own_cache_home(self):
        """Scoped to the cache the job already scoped to /tmp, like the cold
        reset's chown. A world-writable directory in a developer's real cache
        home is not something a CI workaround gets to decide."""
        assert "XDG_CACHE_HOME" in bench_step("write the shared pixi cache")

    def test_it_says_which_uid_mismatch_it_is_for(self):
        """Two lines of mkdir and chmod read like leftover scaffolding unless
        the mismatch that forces them is written down beside them -- and this
        one is invisible on any developer machine, where the host user *is* uid
        1000 and the cache works untouched."""
        assert "uid" in bench_step("write the shared pixi cache")

    def test_it_prepares_the_cache_once_rather_than_before_every_run(self):
        """The opposite cardinality to the cold reset, and for the opposite
        reason: nothing in this job removes this directory. `dl <ws> rm` takes
        the clone and never the shared cache, and `--purge` -- the one command
        that would -- is not run here. A second mention is per-run work that
        has no per-run damage to repair."""
        assert bench_settings().count(PIXI_CACHE_LEAF) == 1


class TestTheExclusionsSayWhyInTheFile:
    def test_the_first_ever_cold_exclusion_is_written_down_where_it_is_made(self):
        """Charter decision 4 excludes the first-ever cold launch — its
        dominant cost is an image transfer this repo does not control. On an
        ephemeral runner that exclusion is a step that primes the image before
        anything is timed, which looks exactly like a step somebody could
        delete as redundant. The reason has to be next to it."""
        assert "first-ever" in bench_workflow()
