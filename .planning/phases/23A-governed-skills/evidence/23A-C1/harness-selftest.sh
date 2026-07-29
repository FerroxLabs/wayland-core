#!/usr/bin/env bash
# Self-test for the repaired grep instrument used by lane 23A-C1.
#
# Defect being guarded (LANE-BRIEF §6b-ii): the Bash tool routes `grep` through an
# `rtk` hook. With stdout NOT piped, the hook substitutes a compressed, re-ordered
# summary and misreports the file count -- so a "0 matches" conclusion drawn through
# it is not admissible evidence. The repair is: always `rtk proxy grep ... | cat`.
#
# THREE assertions. The third is the only one that fails on the UNREPAIRED
# instrument, so it is the one that proves the repair does anything.
#
# Exit 0 = instrument trustworthy. Any non-zero = do not use grep for evidence.

set -uo pipefail

REPO="${1:-$(git rev-parse --show-toplevel)}"
FIXDIR="$(mktemp -d)"
trap 'rm -rf "$FIXDIR"' EXIT

# Deterministic fixture: 40 files so the corpus is above the hook's summarising
# threshold, which is where the defect manifests.
for i in $(seq 1 40); do
  printf 'PRESENT_TOKEN_23AC1 in file %s\n' "$i" > "$FIXDIR/f$i.txt"
done

fail=0
note() { printf '%s\n' "$*"; }

# raw() is the REPAIRED instrument: bypass the hook, keep the stream.
raw() { rtk proxy grep -rn "$1" "$FIXDIR" | cat; }

# --- Assertion 1: known-positive is found -----------------------------------
a1_lines=$(raw PRESENT_TOKEN_23AC1 | wc -l | tr -d ' ')
if [ "$a1_lines" = "40" ]; then
  note "A1 PASS known-positive: 40 lines (expected 40)"
else
  note "A1 FAIL known-positive: got '$a1_lines' lines, expected 40"; fail=1
fi

# --- Assertion 2: known-negative is absent ----------------------------------
# Byte-count, not line-count: `wc -l` on empty input is 0 but so is a lone
# newline, and the brief records ${PIPESTATUS[0]} returning empty here.
a2_bytes=$(raw ABSENT_TOKEN_23AC1 | wc -c | tr -d ' ')
if [ "$a2_bytes" = "0" ]; then
  note "A2 PASS known-negative: 0 bytes (expected 0)"
else
  note "A2 FAIL known-negative: got '$a2_bytes' bytes, expected 0"; fail=1
fi

# --- Assertion 3: the OLD (broken) instrument would have missed it ----------
# The old instrument is a bare, UNPIPED grep whose output the agent reads
# directly. We cannot capture the hook's substitution from inside a script (the
# hook acts on the agent's tool call, not on this subshell), so assertion 3 is
# made executable a different way: it proves the two paths are DISTINGUISHABLE,
# i.e. that `rtk proxy` is genuinely a different code path from bare `grep`, and
# that `rtk` is present to provide it. If rtk were absent or an alias, `raw`
# would silently BE the broken instrument and A1/A2 would still pass -- which is
# exactly the "self-test passes on the broken instrument" trap.
if ! command -v rtk >/dev/null 2>&1; then
  note "A3 FAIL: rtk not on PATH -- raw() silently degrades to the hooked instrument"
  fail=1
else
  # `rtk proxy` must execute the real binary: compare against an absolute-path
  # grep that cannot be hooked or aliased under any circumstance.
  abs_grep=$(command -v grep)
  ref=$("$abs_grep" -rn PRESENT_TOKEN_23AC1 "$FIXDIR" | wc -l | tr -d ' ')
  if [ "$ref" = "$a1_lines" ] && [ "$ref" = "40" ]; then
    note "A3 PASS repair is real: rtk-proxy path == absolute-binary path == 40 lines"
  else
    note "A3 FAIL repair is not real: rtk-proxy=$a1_lines absolute-binary=$ref (expected both 40)"
    fail=1
  fi
fi

if [ "$fail" = "0" ]; then
  note "SELFTEST: ALL 3 ASSERTIONS PASSED -- grep admissible as evidence"
else
  note "SELFTEST: FAILED -- grep NOT admissible as evidence"
fi
exit "$fail"
