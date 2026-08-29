#!/usr/bin/env bash
# PER-ATTEMPT EVIDENCE UNDER AN OUTER RETRY. FerroxLabs/wayland#1177.
#
# ── THE DEFECT THIS CLOSES ─────────────────────────────────────────────────
#
# `.github/workflows/ci.yml` wraps the containerized Linux test run in
# `nick-fields/retry@v3` with `max_attempts: 2`. Attempt 2 runs the same
# nextest command, and nextest's `ci` profile writes its report to a FIXED
# path — `target/nextest/ci/junit.xml`. So attempt 2 OVERWRITES attempt 1.
#
# A test that genuinely failed on attempt 1 and passed on attempt 2 therefore
# leaves no structured trace at all. It is not even recorded as a flake: the
# flake record lives in the file that was just destroyed. The retry-flake
# grader added for wayland#1169 reads that very file, so its fix is complete
# one layer down and blind one layer up.
#
# ── WHY PRESERVE RATHER THAN DROP THE OUTER RETRY ──────────────────────────
#
# The obvious alternative — delete the outer retry and lean on nextest's own
# `retries = 2`, which #1169 already grades — does not work, and the reason is
# specific rather than aesthetic. The outer retry exists for a documented,
# reproduced GHA runner-agent crash: the agent dies ~56s into the nextest step
# and nextest reports "Cancelling due to signal: N tests still running" with
# ZERO actual test failures. That kills the nextest PROCESS. Per-test retries
# inside a process that is being SIGTERMed cannot absorb it; only re-running
# the process can. Dropping the outer retry would hand back an infra crash that
# reds unrelated merges.
#
# So the retry stays and the evidence stops being destroyed. The two properties
# compose into exactly the behaviour wanted: an agent crash writes no JUnit at
# all (nothing to preserve, nothing to grade, the retry silently absorbs it —
# which is what it was built for), while a real test failure on attempt 1
# leaves a preserved report that the grader reds the run for.
#
# ── WHAT THIS WRAPPER DOES ─────────────────────────────────────────────────
#
#   1. Deletes the previous attempt's JUnit BEFORE running. Without this, an
#      attempt that dies before writing a report leaves the PREVIOUS attempt's
#      file in place and it is uploaded as if it described this attempt.
#   2. Runs the command, preserving its exit status verbatim.
#   3. On a failed attempt, copies the report (if one was written) to
#      `<ATTEMPT_DIR>/outer-attempt-<N>.xml`.
#   4. Records the LATEST attempt's outcome in `<ATTEMPT_DIR>/final-status.txt`.
#      The retry action stops at the first success, so the last value written is
#      the step's real outcome. The grader reads it to distinguish "failed and
#      was retried into a green run" (must be reported — nothing else will) from
#      "failed on the final attempt" (the job is already red; reporting it again
#      turns every ordinary red into a confusing second complaint).
#
# ── INPUTS (env) ───────────────────────────────────────────────────────────
#
#   JUNIT_PATH   report the command writes (default target/nextest/ci/junit.xml)
#   ATTEMPT_DIR  where preserved attempts land (default target/nextest/ci/outer-attempts)
#
# Self-tests: .github/scripts/tests/outer-retry-evidence.test.sh
set -uo pipefail

JUNIT_PATH="${JUNIT_PATH:-target/nextest/ci/junit.xml}"
ATTEMPT_DIR="${ATTEMPT_DIR:-target/nextest/ci/outer-attempts}"

if [ "$#" -lt 1 ]; then
  echo "usage: run-tests-with-attempt-evidence.sh <command> [args...]" >&2
  exit 2
fi

mkdir -p "$ATTEMPT_DIR" || exit 2
COUNTER="$ATTEMPT_DIR/.attempt"

attempt=0
if [ -f "$COUNTER" ]; then
  attempt=$(tr -dc '0-9' <"$COUNTER" 2>/dev/null || printf '0')
fi
case "$attempt" in '' | *[!0-9]*) attempt=0 ;; esac
attempt=$((attempt + 1))
printf '%s\n' "$attempt" >"$COUNTER"

echo "-- outer-retry attempt ${attempt} (wayland#1177) -----------------------"
echo "junit path  : ${JUNIT_PATH}"
echo "attempt dir : ${ATTEMPT_DIR}"

# Point 1 above: a stale report must never be mistaken for this attempt's.
rm -f "$JUNIT_PATH"

"$@"
status=$?

if [ "$status" -ne 0 ]; then
  printf 'failure\n' >"$ATTEMPT_DIR/final-status.txt"
  if [ -f "$JUNIT_PATH" ]; then
    cp "$JUNIT_PATH" "$ATTEMPT_DIR/outer-attempt-${attempt}.xml"
    echo "attempt ${attempt} FAILED (exit ${status}); preserved its JUnit as outer-attempt-${attempt}.xml"
  else
    # The runner-agent-crash shape: the process died before writing a report.
    # Nothing to preserve and nothing to grade, which is correct — no test was
    # recorded as having failed.
    echo "attempt ${attempt} FAILED (exit ${status}) and wrote no JUnit; nothing to preserve"
  fi
else
  printf 'success\n' >"$ATTEMPT_DIR/final-status.txt"
  echo "attempt ${attempt} passed"
fi

exit "$status"
