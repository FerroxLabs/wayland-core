#!/usr/bin/env bash
# Lane 28-drift — the full gate set, captured reproducibly.
#
# HARNESS DEFECT FOUND AND REPAIRED IN THIS LANE, recorded because a written-up instrument
# defect is one you have agreed to keep. The first version of this transcript built each
# invocation as a single string and passed it unquoted:
#
#     for a in "--validate $L" ...; do python3 f28-ledger.py $a; done
#
# In bash that word-splits and works. **This agent's shell is zsh, which does NOT word-split
# unquoted parameter expansions**, so the whole string arrived as ONE argv entry, argparse
# rejected it, and three gates reported rc=2 while looking like they had been run. The output
# was read, which is the only reason it was caught. Every invocation below is now an explicit
# argv array, and the harness self-tests that a known-good gate really returns 0.
#
# Run from the repository root. /usr/bin/git only.
set -uo pipefail

ROOT=$(/usr/bin/git rev-parse --show-toplevel)
cd "$ROOT" || exit 2
S=.planning/scripts
D=.planning/phases/28-native-cross-platform-certification
L=$D/evidence/28-04/findings.tsv
ORIG=$D/28-04-CERTIFICATION-RECEIPT.json
S1=$D/28-04-CERTIFICATION-RECEIPT-SUPERSEDING-001.json
S2=$D/28-04-CERTIFICATION-RECEIPT-SUPERSEDING-002.json

run () {  # run <label> <cmd...>
  local label=$1; shift
  local out; out=$(mktemp)
  "$@" >"$out" 2>&1
  local rc=$?
  printf '%-58s rc=%d  bytes=%d\n' "$label" "$rc" "$(wc -c <"$out")"
  grep -E 'F28[LVD]-|--(validate|check-[a-z-]+|verify|self-test): ' "$out" | sed 's/^/    /'
  rm -f "$out"
  return $rc
}

echo "# Gate transcript — lane/28-drift — $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "# Exit codes captured WITHOUT a pipeline (PIPESTATUS returns empty in this shell)."
echo "# HEAD: $(/usr/bin/git rev-parse HEAD)"
echo
echo "## HARNESS SELF-TEST — a known-good gate must return 0 here, or every rc below is noise."
run "f28-ledger.py --self-test" python3 $S/f28-ledger.py --self-test
harness_rc=$?
[ $harness_rc -eq 0 ] || { echo "HARNESS BROKEN: the control gate did not return 0"; exit 2; }
echo
echo "## f28-ledger.py — the adjudicated ledger, now 65 rows"
run "--validate (strict, allow_open=False)" python3 $S/f28-ledger.py --validate "$L"
run "--check-a2"                            python3 $S/f28-ledger.py --check-a2 "$L"
run "--check-downgrades"                    python3 $S/f28-ledger.py --check-downgrades "$L"
run "--check-backlog-ids"                   python3 $S/f28-ledger.py --check-backlog-ids "$L" .planning/BACKLOG.md
echo
echo "## PROOF THE LEDGER GATE STILL BITES — four tampers on F-28-ADJ-001, the row this lane added"
bash "$D/evidence/28-drift/probe-ledger-tampers.sh"
echo
echo "## f28-verify-bindings.py — three receipts, four modes each"
run "--self-test" python3 $S/f28-verify-bindings.py --self-test
for r in "$ORIG" "$S1" "$S2"; do
  echo "  --- $(basename "$r")"
  run "    --verify"                  python3 $S/f28-verify-bindings.py --verify "$r"
  run "    --check-enumeration"       python3 $S/f28-verify-bindings.py --check-enumeration "$r" "$L"
  run "    --check-tamper-detection"  python3 $S/f28-verify-bindings.py --check-tamper-detection "$r"
  run "    --check-claim-limit"       python3 $S/f28-verify-bindings.py --check-claim-limit "$r"
done
run "--check-requirements" python3 $S/f28-verify-bindings.py --check-requirements .planning/REQUIREMENTS.md "$L"
echo
echo "## f28-check-drift.py — the instrument this lane added"
run "--self-test" python3 $S/f28-check-drift.py --self-test
for r in "$ORIG" "$S1" "$S2"; do
  run "  --receipt $(basename "$r") --ref HEAD" python3 $S/f28-check-drift.py --receipt "$r" --ref HEAD
done
echo
echo "## PROOF EVERY F28D CODE CAN FIRE"
bash "$D/evidence/28-drift/probe-drift-codes.sh"
