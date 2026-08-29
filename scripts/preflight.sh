#!/usr/bin/env bash
# Lane pre-flight: everything CI's ci-linux job checks on the HOST, before it
# builds the container image.
#
# WHY THIS EXISTS. Lanes gate on hetzner because CI's Linux leg takes ~67
# minutes to report what hetzner reports in 2-3. But "gate on hetzner" was
# incomplete: hetzner ran the TESTS and none of the repo gates, so a lane could
# be fully green locally and still red CI. Measured 2026-08-29 — the #365 c2
# lane passed nextest 150/150 on hetzner and failed CI at "No unserialized test
# writes to shared process globals", because that check is a repo gate and not a
# test. Running these here makes the hetzner gate actually complete.
#
# This is NOT a substitute for CI's macOS and Windows legs. Nothing here
# compiles for another platform. A criterion needing cross-platform evidence
# still needs a real run on that platform.
set -uo pipefail
cd "$(git rev-parse --show-toplevel)" || exit 2

# Each entry is the exact invocation ci.yml uses, in ci.yml's order.
GATES=(
  "python3 scripts/check-no-vacuous-cargo-test.py --self-test"
  "python3 scripts/check-no-vacuous-cargo-test.py"
  "python3 scripts/check-model-limits-freshness.py --self-test"
  "python3 scripts/check-no-personal-identifiers.py --self-test"
  "python3 scripts/check-no-personal-identifiers.py"
  "python3 scripts/check-test-env-globals.py --self-test"
  "python3 scripts/check-test-env-globals.py"
  "python3 scripts/check-criteria-ledger.py --self-test"
  "python3 scripts/check-criteria-ledger.py --offline"
  "python3 scripts/check-release-readiness.py --self-test"
  "python3 scripts/check-windows-attribution.py --self-test"
  "python3 scripts/check-windows-attribution.py"
  "python3 scripts/flake-ledger.py --self-test"
)

# ── DRIFT GUARD ────────────────────────────────────────────────────────────
# A hand-maintained copy of a CI step list goes stale silently, and a
# pre-flight that under-covers is worse than none: it converts "I did not
# check" into "I checked and it was fine". So derive ci.yml's real host-side
# gate set and refuse to run if this list no longer matches it.
python3 - <<'PY' || exit 2
import re, sys
lines = open(".github/workflows/ci.yml").read().splitlines()
start = stop = None
for i, l in enumerate(lines):
    if re.match(r"^  ci-linux:", l): start = i
    if start is not None and "- name: Build CI image" in l and i > start:
        stop = i; break
if start is None or stop is None:
    print("PREFLIGHT DRIFT GUARD: could not locate the ci-linux host-side region "
          "in ci.yml. The job or the image step was renamed; fix this script.")
    sys.exit(2)
in_ci = set(re.findall(r"scripts/[A-Za-z0-9_.-]+\.py", "\n".join(lines[start:stop])))
mine  = set(re.findall(r"scripts/[A-Za-z0-9_.-]+\.py", open("scripts/preflight.sh").read()))
missing = in_ci - mine
if missing:
    print("PREFLIGHT DRIFT GUARD: ci.yml runs host-side gate(s) this pre-flight "
          "does NOT, so a green here would not mean a green there:")
    for m in sorted(missing): print("   ", m)
    print("Add them to GATES above, then re-run.")
    sys.exit(2)
print(f"drift guard: ok ({len(in_ci)} host-side gate script(s) in ci.yml, all covered)")
PY

fail=0
for cmd in "${GATES[@]}"; do
  if out="$($cmd 2>&1)"; then
    printf '  ok    %s\n' "$cmd"
  else
    printf '  FAIL  %s\n' "$cmd"
    printf '%s\n' "$out" | tail -25 | sed 's/^/        /'
    fail=1
  fi
done

echo
if [ "$fail" -ne 0 ]; then
  echo "PRE-FLIGHT FAILED — CI would red on this tree. Fix before pushing."
  exit 1
fi
echo "PRE-FLIGHT PASSED — every host-side gate ci.yml runs before the image build."
echo "Still owed separately: cargo fmt --check, clippy -D warnings, nextest,"
echo "and any macOS/Windows evidence your criteria name."
