#!/usr/bin/env bash
# Self-test for the CI-TRIAGE lane's history-reading instrument (LANE-BRIEF §6b-ii).
#
# DEFECT 1 (the one this test guards): the `rtk` git proxy silently drops merge
# commits from `git log`, and backfills with older non-merge commits so the
# output still looks like a well-formed log of the requested length. `rev-parse
# HEAD` and `log HEAD` therefore disagree about what HEAD is.
#
#   /usr/bin/git rev-parse HEAD  -> 3687cbc2  (a merge)
#   rtk git      log -3 HEAD     -> c57a54c5, 7f5c0455, 8afd1934   (merge absent)
#   /usr/bin/git log -3 HEAD     -> 3687cbc2, c57a54c5, 5ea07374   (merge present)
#
# rtk returns rc=0 and 123 bytes of perfectly well-formed output. Nothing about
# the response indicates that a commit was withheld.
#
# WHERE IT BITES: rtk is not on PATH as `git`. It is applied by an agent-harness
# hook that rewrites tool-level `git ...` invocations into `rtk git ...`. Inside a
# shell script, `git` is the real binary -- so this defect is INVISIBLE to any
# test that runs inside a script and calls plain `git`. A3 below therefore
# invokes `rtk` explicitly, which is the only way to reach the broken path.
#
# REPAIR: this lane reads history through /usr/bin/git at the tool layer, never
# through a bare `git`.
#
# DEFECT 2, found while writing THIS script and fixed in it: the first draft used
#   producer | grep -q PATTERN
# under `set -o pipefail`. `grep -q` exits on first match, the producer takes
# SIGPIPE, and pipefail promotes that to a pipeline status of 141 -- so a
# CORRECT match was scored as a FAILURE. Measured: rc=141 while `grep -cx`
# over the identical output returns 1. That is the LANE-BRIEF §3.2 "a pipe steals
# exit status" class, in the instrument built to hunt that class. This script now
# captures to a file and matches the file; it contains no pipes at all.
#
# Three assertions, per §6b-ii. The third is the one that proves the repair does
# anything: without it, this script passes on the BROKEN instrument too.
#
# Run:  bash .planning/scripts/selftest-git-shim.sh
set -uo pipefail

REPAIRED=/usr/bin/git
GREP=/usr/bin/grep
WC=/usr/bin/wc          # NB: PATH `wc -c < file` reads 0 through the shim; use absolute.
fail=0
pass=0

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

note() { printf '%s\n' "$*"; }
ok()   { pass=$((pass + 1)); note "PASS: $1"; }
bad()  { fail=$((fail + 1)); note "FAIL: $1"; }

# Match a full SHA as its own line, WITHOUT a pipe (see DEFECT 2 above).
# Returns 0 on present, 1 on absent. Never inherits a producer's exit status.
sha_present_in() {  # sha_present_in <sha> <file>
  local n
  n="$("$GREP" -cx -- "$1" "$2" 2>/dev/null || true)"
  [ "${n:-0}" -gt 0 ]
}

# Pick a merge commit reachable from HEAD to test against. If the branch has no
# merges the instrument cannot be exercised, and that is a hard error, not a skip
# -- a silent skip is the same defect wearing a different hat.
MERGE_SHA="$("$REPAIRED" rev-list --merges -n 1 HEAD 2>/dev/null)"
if [ -z "$MERGE_SHA" ]; then
  note "FAIL: no merge commit reachable from HEAD; instrument cannot be exercised"
  note "RESULT: 0 passed, 1 failed"
  exit 1
fi

# Depth must be large enough that the merge is genuinely inside the window, so a
# miss is a real omission and not just "it fell off the end".
DEPTH="$(( $("$REPAIRED" rev-list --count "${MERGE_SHA}..HEAD" 2>/dev/null) + 3 ))"
note "fixture: merge commit under test = ${MERGE_SHA}"
note "fixture: log depth            = ${DEPTH}"

# Capture both instruments' output ONCE, to files, and byte-count each capture
# before trusting it (LANE-BRIEF: "byte-count every capture").
RTK="$(command -v rtk || echo /opt/homebrew/bin/rtk)"
"$REPAIRED" --no-pager log --format='%H' -n "$DEPTH" HEAD > "$TMP/repaired.txt" 2>/dev/null
"$RTK" git   --no-pager log --format='%H' -n "$DEPTH" HEAD > "$TMP/shimmed.txt"  2>/dev/null
R_BYTES="$("$WC" -c < "$TMP/repaired.txt")"
S_BYTES="$("$WC" -c < "$TMP/shimmed.txt")"
note "capture: repaired=${R_BYTES// /} bytes, shimmed=${S_BYTES// /} bytes"
if [ "${R_BYTES// /}" -eq 0 ]; then
  note "FAIL: repaired instrument produced an EMPTY capture; nothing below is meaningful"
  note "RESULT: 0 passed, 1 failed"
  exit 1
fi

# --- Assertion 1 (known-positive): the repaired instrument SEES the merge.
if sha_present_in "$MERGE_SHA" "$TMP/repaired.txt"; then
  ok "A1 known-positive: /usr/bin/git log lists ${MERGE_SHA:0:8}"
else
  bad "A1 known-positive: /usr/bin/git log did NOT list ${MERGE_SHA:0:8}"
fi

# --- Assertion 2 (known-negative): the repaired instrument does NOT invent a
# commit that is not in history. A matcher that says yes to everything passes A1.
ABSENT="0000000000000000000000000000000000000000"
if sha_present_in "$ABSENT" "$TMP/repaired.txt"; then
  bad "A2 known-negative: instrument reported an all-zero SHA as present"
else
  ok "A2 known-negative: absent SHA correctly not reported"
fi

# --- Assertion 3 (the old broken instrument WOULD have missed it).
# This is the assertion that proves the repair is load-bearing. If rtk also sees
# the merge, the proxy is fixed and this lane's mitigation can be retired -- which
# we want to be TOLD, explicitly, rather than silently carrying a workaround
# forever. An empty rtk capture is also a failure: absence-by-crash is not
# evidence of the defect we are claiming.
if [ "${S_BYTES// /}" -eq 0 ]; then
  bad "A3 differential: rtk capture was EMPTY -- differential not exercised, defect claim unproven"
elif sha_present_in "$MERGE_SHA" "$TMP/shimmed.txt"; then
  bad "A3 differential: \`rtk git log\` ALSO sees ${MERGE_SHA:0:8} -- proxy no longer drops merges; retire the /usr/bin/git workaround and this test"
else
  ok "A3 differential: \`rtk git log\` returned ${S_BYTES// /} well-formed bytes but MISSES ${MERGE_SHA:0:8} -- the defect is real and the repair is load-bearing"
fi

note "RESULT: ${pass} passed, ${fail} failed"
[ "$fail" -eq 0 ]
