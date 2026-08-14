#!/usr/bin/env bash
# Re-establish the cold-recreate shape's starting state, once per benched run.
#
# `.github/workflows/bench.yml` passes this as the bench's `--before`, which is
# the only place with the right cardinality: every run recreates the files this
# recovers, so a one-time step before the bench would fix run 1 and hand runs
# 2..5 exactly the clone that broke it.
#
# THE CHOWN IS WHAT MAKES THE REMOVE POSSIBLE, on a runner. A launch leaves its
# clone owned by the container's user -- uid 1000 in the standard devcontainer
# base image -- and a GitHub runner's own user is not that uid, which is the
# "when part of the cache will not go" case README.md describes. `dl rm` then
# deletes the workspace, warns that the clone would not go, and exits 0; the
# next launch dies writing `.git/index.lock` into a clone it does not own, and
# the shape reports a failed run instead of a time (run 31838698495). Nothing
# about `dl` is wrong there -- it cannot unlink what it does not own -- so the
# recovery belongs to the environment that creates the mismatch, and
# passwordless sudo is a property of GitHub-hosted runners. This script is the
# only place in the repo that assumes it.
#
# It is a FILE rather than a string in the step because the string did not
# survive the trip: `--before` reaches the bench through `pixi run bench`, whose
# task shell re-joins and re-parses the arguments appended to a task, and the
# quotes nested inside the step's own quotes were gone by the time argparse saw
# them (run 31840842480). A path plus one argument has no second quoting level
# to lose.
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $(basename "$0") <owner/repo[@branch]>" >&2
    exit 2
fi

# `test -d` because a reset also runs before the first launch, when there may
# be nothing there yet. Scoped to the job's own clone cache: a recursive chown
# is a blunt instrument, and a wider one would take ownership of things this
# job did not create.
repos="${XDG_CACHE_HOME:?the bench scopes this cache away from the one a developer launches into}/devlaunch/repos"
if [ -d "$repos" ]; then
    sudo chown -R "$(id -u):$(id -g)" "$repos"
fi

exec dl "$1" rm --force
