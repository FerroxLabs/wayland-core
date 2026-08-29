#!/usr/bin/env bash
# Precondition for steps that carry `if: ${{ !cancelled() }}` and therefore run
# even when an earlier step in the job failed.
#
# When an earlier step fails, GitHub SKIPS the intervening steps -- including
# "Build CI image". A `!cancelled()` step then reaches `docker run "$CI_IMAGE"`
# with no such image, docker exits non-zero, and the step reports ITS OWN domain
# failure: the swarm step accuses the product of a delegated-dispatch regression,
# the drill step accuses the signing path. Neither ran. A red that names the
# wrong cause costs exactly what a false green costs -- it sends the next person
# to the wrong place.
#
# Fail CLOSED either way: a missing prerequisite must never read as a pass. The
# only thing this changes is WHICH failure gets reported.
set -euo pipefail
step="${1:?usage: require-ci-image.sh <step name>}"
: "${CI_IMAGE:?CI_IMAGE is unset -- the job-level env has changed}"

if docker image inspect "$CI_IMAGE" >/dev/null 2>&1; then
  exit 0
fi

echo "::error title=${step}::PREREQUISITE MISSING -- this is NOT a ${step} failure. The CI image '${CI_IMAGE}' does not exist, because the 'Build CI image' step was SKIPPED by an earlier failing step in this job. This step therefore proves NOTHING about ${step}: no test was collected and none ran. Fix the earlier red step and re-run; do not read this as a regression."
exit 1
