#!/usr/bin/env bash
# Failure-RATE harness. Runs a cargo test invocation N times and reports
# pass/fail counts, asserting a non-zero executed-test count on every rep.
#
# Anti-vacuity (LANE-BRIEF §3.2): a suite can exit 0 having run ZERO tests.
# So a rep only counts as a PASS if it BOTH exited 0 AND reported >= MIN_TESTS
# executed. A rep that ran too few tests is graded VACUOUS, never PASS.
#
# Uses the absolute cargo path — `rtk` rewrites cargo output and strips the
# `0 ignored` / `0 filtered out` fields this check depends on.
set -u

CARGO=/root/.cargo/bin/cargo
REPS="${REPS:-25}"
MIN_TESTS="${MIN_TESTS:-1}"
LABEL="${LABEL:-run}"
OUT="${OUT:-/tmp/flake-root-fix-${LABEL}}"

mkdir -p "$OUT"

pass=0; fail=0; vacuous=0
for i in $(seq 1 "$REPS"); do
  log="$OUT/rep-$i.log"
  "$CARGO" test "$@" > "$log" 2>&1
  rc=$?
  # Sum every "N passed" across all test binaries in this rep.
  ran=$(awk 'match($0, /[0-9]+ passed/) {
               s = substr($0, RSTART, RLENGTH); sub(/ passed/, "", s); t += s
             } END { print t+0 }' "$log")
  if [ "$ran" -lt "$MIN_TESTS" ]; then
    vacuous=$((vacuous+1)); verdict="VACUOUS(ran=$ran)"
  elif [ "$rc" -eq 0 ]; then
    pass=$((pass+1)); verdict="PASS(ran=$ran)"
  else
    fail=$((fail+1)); verdict="FAIL(ran=$ran)"
    grep -E '^(test .* FAILED|failures:|---- )' "$log" | head -20 \
      >> "$OUT/failure-names.txt"
  fi
  echo "[$LABEL] rep $i/$REPS rc=$rc $verdict"
done

echo "=============================================="
echo "[$LABEL] REPS=$REPS  PASS=$pass  FAIL=$fail  VACUOUS=$vacuous"
echo "[$LABEL] FAILURE RATE = $fail / $REPS"
echo "=============================================="
