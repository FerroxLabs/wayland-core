#!/usr/bin/env bash
# §6b-ii — instrument repair + self-test for THIS lane's spend meter.
#
# THE DEFECT (found in this lane, in this lane's own instrument):
#   The first meter read the credit counter from `/v1/models`:
#       curl -D - .../v1/models | awk 'tolower($1)=="x-flux-available:"{print $2}'
#   `/v1/models` does NOT carry `x-flux-available` (verified: it returns only
#   date/content-type/cf-* headers). So the meter printed `CREDIT_BEFORE=` and
#   `CREDIT_AFTER=` — EMPTY — with exit status 0. A silently empty spend figure
#   is precisely the "silently destroys a result rather than failing loudly"
#   class the lane brief names, occurring inside the instrument built to meter
#   spend.
#
# THE REPAIR (two parts, both required):
#   1. read the counter from `/v1/chat/completions`, which does carry it;
#   2. FAIL LOUDLY when the header is absent instead of returning empty.
#
# Run: ./credit-meter-selftest.sh     (no network, no credential, no spend)
set -uo pipefail

TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

# --- fixtures ------------------------------------------------------------
# A response that DOES carry the counter (shape copied from a real
# /v1/chat/completions response; value is synthetic).
printf 'HTTP/2 200 \r\ndate: Wed, 29 Jul 2026 00:26:51 GMT\r\nx-flux-routed-model: mistral-small\r\nx-flux-available: 264213742\r\nx-flux-cost-usd: 0.000036\r\n\r\n' > "$TMP/with.txt"
# A response that does NOT (shape copied from a real /v1/models response —
# the exact thing the broken meter was pointed at).
printf 'HTTP/2 200 \r\ndate: Wed, 29 Jul 2026 00:26:51 GMT\r\ncontent-type: application/json\r\ncf-cache-status: DYNAMIC\r\nserver: cloudflare\r\n\r\n' > "$TMP/without.txt"

# --- the OLD, broken matcher --------------------------------------------
# Bare awk, no absence detection. Returns empty + rc 0 when the header is gone.
old_matcher() { tr -d '\r' < "$1" | awk 'tolower($1)=="x-flux-available:"{print $2}'; }

# --- the REPAIRED matcher ------------------------------------------------
# Extracts the counter; if absent, prints CREDIT_UNREADABLE and returns 3.
# Note the value is captured WITHOUT a pipeline stealing the status.
new_matcher() {
  local v
  v=$(tr -d '\r' < "$1" | awk 'tolower($1)=="x-flux-available:"{print $2; found=1} END{exit !found}')
  local rc=$?
  if [ $rc -ne 0 ] || [ -z "$v" ]; then
    echo "CREDIT_UNREADABLE(no x-flux-available header in $1)"
    return 3
  fi
  echo "$v"
  return 0
}

fail=0

# ASSERT 1 — known positive: repaired matcher reads the counter.
out=$(new_matcher "$TMP/with.txt"); rc=$?
if [ "$rc" -eq 0 ] && [ "$out" = "264213742" ]; then
  echo "ASSERT_1_KNOWN_POSITIVE=PASS (repaired matcher read $out, rc=0)"
else
  echo "ASSERT_1_KNOWN_POSITIVE=FAIL (rc=$rc out='$out')"; fail=1
fi

# ASSERT 2 — known negative: repaired matcher FAILS LOUDLY, does not return empty.
out=$(new_matcher "$TMP/without.txt"); rc=$?
if [ "$rc" -eq 3 ] && [ "${out#CREDIT_UNREADABLE}" != "$out" ]; then
  echo "ASSERT_2_KNOWN_NEGATIVE=PASS (repaired matcher rc=3 and said UNREADABLE)"
else
  echo "ASSERT_2_KNOWN_NEGATIVE=FAIL (rc=$rc out='$out')"; fail=1
fi

# ASSERT 3 — the one that proves the repair does anything:
# the OLD matcher reports SUCCESS-SHAPED output (rc 0, empty string) on the
# same known-negative, i.e. it would have silently reported no spend at all.
old_out=$(old_matcher "$TMP/without.txt"); old_rc=$?
new_out=$(new_matcher "$TMP/without.txt"); new_rc=$?
if [ "$old_rc" -eq 0 ] && [ -z "$old_out" ] && [ "$new_rc" -ne 0 ]; then
  echo "ASSERT_3_OLD_MATCHER_MISSED_IT=PASS (old rc=0 + empty on a counter-less response; repaired rc=$new_rc)"
else
  echo "ASSERT_3_OLD_MATCHER_MISSED_IT=FAIL (old_rc=$old_rc old_out='$old_out' new_rc=$new_rc)"; fail=1
fi

[ "$fail" -eq 0 ] && echo "SELFTEST_RESULT=PASS" || echo "SELFTEST_RESULT=FAIL"
exit "$fail"
