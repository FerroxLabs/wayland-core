#!/usr/bin/env bash
# Self-test for the "is there a cost record anywhere?" matcher used by the
# 27-credentialled lane.
#
# WHY THIS EXISTS (LANE-BRIEF §6b-ii): the first version of the accounting
# sweep in `shapeA.sh` was
#
#     grep -in 'cost\|usd\|usage' FILES | head -20
#     echo "COST_GREP_RC=$?"        # <-- reports head(1)'s status, not grep's
#
# and it printed `COST_GREP_RC=0` against files containing ZERO matches. A
# reader would take 0 as "the sweep succeeded" when it is `head` exiting 0
# unconditionally. This is the pipe-steals-exit-status class the brief names,
# reproduced inside this lane's own instrument. It is repaired here rather
# than merely written up.
#
# Three assertions, per §6b-ii:
#   1. known-positive  -> repaired matcher reports FOUND
#   2. known-negative  -> repaired matcher reports ABSENT
#   3. the OLD matcher would have MISSED it -- the old form reports rc=0
#      (found-shaped) on the very same known-negative input.
# Assertion 3 is the only one that proves the repair does anything.

set -u
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

PATTERN='cost_usd|"usage"|billed|price_usd'

printf 'generation ok\n{"usage":{"cost_usd":0.08}}\n' > "$TMP/positive.txt"
printf 'wrote /tmp/a-flux.png (46216 bytes)\n' > "$TMP/negative.txt"

# --- repaired matcher: grep is the LAST command in its own statement, its
# --- status is captured directly, and output is truncated separately.
cost_record_present() {
  local out
  out=$(grep -Eih "$PATTERN" "$@" 2>/dev/null)
  local rc=$?
  printf '%s' "$out" | head -c 2000
  return $rc
}

fail=0

# 1. known-positive
if cost_record_present "$TMP/positive.txt" >/dev/null; then
  echo "ASSERT_1_KNOWN_POSITIVE=PASS (repaired matcher reports FOUND)"
else
  echo "ASSERT_1_KNOWN_POSITIVE=FAIL"; fail=1
fi

# 2. known-negative
if cost_record_present "$TMP/negative.txt" >/dev/null; then
  echo "ASSERT_2_KNOWN_NEGATIVE=FAIL (repaired matcher claimed FOUND on clean input)"; fail=1
else
  echo "ASSERT_2_KNOWN_NEGATIVE=PASS (repaired matcher reports ABSENT)"
fi

# 3. the OLD broken matcher would have missed it
old_rc=$( { grep -Eih "$PATTERN" "$TMP/negative.txt" | head -20 >/dev/null; echo $?; } )
new_rc=$( { cost_record_present "$TMP/negative.txt" >/dev/null; echo $?; } )
if [ "$old_rc" = "0" ] && [ "$new_rc" != "0" ]; then
  echo "ASSERT_3_OLD_MATCHER_MISSED_IT=PASS (old rc=$old_rc found-shaped on a clean file; repaired rc=$new_rc)"
else
  echo "ASSERT_3_OLD_MATCHER_MISSED_IT=FAIL (old rc=$old_rc new rc=$new_rc — the repair changes nothing)"; fail=1
fi

echo "SELFTEST_RESULT=$([ $fail -eq 0 ] && echo PASS || echo FAIL)"
exit $fail
