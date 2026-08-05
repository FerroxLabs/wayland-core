#!/usr/bin/env bash
# Self-test for the REPAIRED handle_in_store matcher (LANE-BRIEF §6b-ii).
#
# Three assertions, not two. The third is the only one that proves the repair
# does anything — without it the self-test passes on the broken matcher too.
#
# The absent arm's home is a perfect fixture: it holds ONE stored handle and
# ONE missing handle in the same `channel credential list` output.
set -uo pipefail
BIN=/root/wayland-authprod/target/debug/wayland-core
WLHOME=/root/wl-authprod-slack-2-absent-fixed

LIST=$(WAYLAND_HOME="$WLHOME" "$BIN" channel credential list 2>/dev/null)

old_matcher() { printf '%s\n' "$LIST" | grep -c "^\s*$1\b\|$1"; }
new_matcher() { printf '%s\n' "$LIST" | awk -v h="$1" 'index($0,h)>0 && $NF=="stored" {n++} END{print n+0}'; }

STORED_HANDLE="slack.authprod.signing_secret"   # known-positive: really stored
MISSING_HANDLE="slack.authprod.ABSENT_KEY"      # known-negative: really absent

echo "=== handle_in_store matcher self-test ==="
printf '%s\n' "$LIST" | grep -E 'stored|MISSING'
echo

A=$(new_matcher "$STORED_HANDLE")
B=$(new_matcher "$MISSING_HANDLE")
C=$(old_matcher "$MISSING_HANDLE")

echo "1. known-positive  new_matcher($STORED_HANDLE)  = $A   (must be 1)"
echo "2. known-negative  new_matcher($MISSING_HANDLE) = $B   (must be 0)"
echo "3. old matcher on the known-negative            = $C   (must be NON-zero,"
echo "   i.e. the old matcher WOULD have missed this — proving the repair acts)"
echo

RC=0
[ "$A" = "1" ] || { echo "FAIL assertion 1"; RC=1; }
[ "$B" = "0" ] || { echo "FAIL assertion 2"; RC=1; }
[ "$C" != "0" ] || { echo "FAIL assertion 3 — repair is a no-op"; RC=1; }
[ "$RC" = "0" ] && echo "SELFTEST=PASS (all three)" || echo "SELFTEST=FAIL"
exit $RC
