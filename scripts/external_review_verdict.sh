#!/usr/bin/env bash
# The verdict on whether an external reviewer actually reviewed a pull request.
#
# Separate from the workflow that calls it so that the classification is
# executable outside CI, which is what `test/test_external_review_guard.py`
# executes. A `case` statement living only inside a `run:` block is testable
# only by copying it, and a copied guard is a guard that drifts.
#
# Reads the concatenated review bodies on stdin. Exit 0 means an external
# reviewer answered about the code; exit 1 means it did not, and the reason is
# on stdout as a GitHub workflow-command error so the annotation appears on the
# pull request rather than only in a log nobody opens.
set -euo pipefail

OVERRIDE=${OVERRIDE_LABEL:-no-external-review}
reviews=$(cat)

if [ -z "${reviews//[[:space:]]/}" ]; then
  echo "::error::no external review. Either the reviewer is not installed on"\
       "this repository any more, or it is not reaching this pull request."\
       "Label '$OVERRIDE' to merge anyway."
  exit 1
fi

# Both refusals are quota refusals, and neither says anything about the code.
# They are matched on the stable half of each sentence: the numbers move when
# the plan does, and `@blooop` is not the only account that can open a pull
# request here.
case $reviews in
  *"you have reached your weekly rate limit"*)
    echo "::error::the external reviewer is out of weekly quota and reviewed"\
         "nothing. It posts that refusal *as a review*, which is why this is"\
         "not otherwise visible. Wait for the quota, or label '$OVERRIDE' to"\
         "merge unreviewed."
    exit 1
    ;;
  *"larger than the review limit"*)
    echo "::error::this pull request is over the external reviewer's per-diff"\
         "cap, so it went unreviewed -- on the size of change that least"\
         "tolerates that. Split it, or label '$OVERRIDE' to merge unreviewed."
    exit 1
    ;;
esac

echo "the external reviewer ran and answered about the code"
