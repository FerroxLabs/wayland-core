#!/bin/bash
# Independent re-verification of CTRL-01's two pinned peer baselines.
#
# READ-ONLY. Only cat-file, merge-base, show, status and rev-parse are used.
# No write verb (checkout/fetch/commit/reset/clean) appears anywhere below —
# the pins depend on these trees staying exactly where they are.
#
# The ledger's own sentence about these pins is NOT trusted: the commit is
# re-resolved, the ancestry is re-run, and the version string is read back out
# of the exact file and line at the pinned commit with `git show <sha>:<path>`
# so the read is bound to the pin rather than to the working tree.
set -u

# repo | pinned commit | version file | line | version the ledger records
PEERS=(
  "/Users/seandonahoe/dev/resources/hermes-agent|dbe734beff0caf5e8ee2acbe4277db7f6cf84a21|pyproject.toml|10|0.17.0"
  "/Users/seandonahoe/dev/resources/openclaw|11a0ad10e91a50d5a0e636494eea4d7ad3eaf9fc|package.json|3|2026.6.2"
)

echo "PEER BASELINE RE-VERIFICATION — read-only"
echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo

RC=0
for row in "${PEERS[@]}"; do
  R=${row%%|*}; rest=${row#*|}
  C=${rest%%|*}; rest=${rest#*|}
  F=${rest%%|*}; rest=${rest#*|}
  LN=${rest%%|*}
  EXP=${rest#*|}

  echo "== $(basename "$R")"
  echo "   repo   : $R"
  echo "   pin    : $C"

  echo "-- cmd: git -C <repo> cat-file -t $C"
  T=$(/usr/bin/git -C "$R" cat-file -t "$C" 2>&1)
  echo "   out: $T"
  [ "$T" = "commit" ] || { echo "   VERDICT: UNRESOLVED (not a commit object)"; RC=1; }

  echo "-- cmd: git -C <repo> merge-base --is-ancestor $C HEAD"
  /usr/bin/git -C "$R" merge-base --is-ancestor "$C" HEAD 2>&1
  A=$?
  echo "   rc: $A  (0 = pinned commit IS an ancestor of local HEAD)"
  [ "$A" -eq 0 ] || { echo "   VERDICT: ANCESTRY BROKEN"; RC=1; }

  echo "-- cmd: git -C <repo> show $C:$F   (line $LN)"
  L=$(/usr/bin/git -C "$R" show "$C:$F" 2>&1 | /usr/bin/sed -n "${LN}p")
  echo "   out: $L"
  echo "   ledger records: $EXP"
  case "$L" in
    *"$EXP"*) echo "   VERDICT: version string AGREES with the ledger" ;;
    *)        echo "   VERDICT: version string DISAGREES — the read is authoritative"; RC=1 ;;
  esac

  echo "-- cmd: git -C <repo> status --short   (write-check: this run must leave it as found)"
  S=$(/usr/bin/git -C "$R" status --short 2>&1)
  echo "   dirty paths: $(printf '%s' "$S" | /usr/bin/grep -c . )"

  echo "-- cmd: git -C <repo> rev-parse HEAD"
  echo "   out: $(/usr/bin/git -C "$R" rev-parse HEAD 2>&1)"
  echo
done

echo "PEER_BASELINE_RESULT=$([ $RC -eq 0 ] && echo PASS || echo FAIL)"
exit $RC
