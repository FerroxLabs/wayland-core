#!/usr/bin/env bash
# Lane pre-flight: everything CI's ci-linux job checks on the HOST, before it
# builds the container image, plus the Desktop contract corpus gate (see its
# own block below for why that one is here and not in GATES).
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
  "python3 .planning/evidence/ci-macos-budget/gate.py --self-test .github/workflows/ci.yml"
  "python3 .planning/evidence/ci-macos-budget/gate.py .github/workflows/ci.yml"
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
# Any .py gate, not only scripts/ -- the macOS admission gate lives under
# .planning/evidence/, and a guard that only looks in one directory is exactly
# how that gate stayed uncovered while it was red.
PAT = r"(?:scripts|\.planning/evidence)/[A-Za-z0-9_./-]+\.py"
in_ci = set(re.findall(PAT, "\n".join(lines[start:stop])))
mine  = set(re.findall(PAT, open("scripts/preflight.sh").read()))
missing = in_ci - mine
if missing:
    print("PREFLIGHT DRIFT GUARD: ci.yml runs host-side gate(s) this pre-flight "
          "does NOT, so a green here would not mean a green there:")
    for m in sorted(missing): print("   ", m)
    print("Add them to GATES above, then re-run.")
    sys.exit(2)
print(f"drift guard: ok ({len(in_ci)} host-side gate script(s) in ci.yml, all covered)")
PY

# ── DESKTOP CONTRACT CORPUS ────────────────────────────────────────────────
# NOT in GATES above, because GATES is a mirror of ci.yml's HOST-side steps and
# this one runs inside ci-linux's container. It is here because of how a lane
# gets it wrong, measured on lane/f13-w2-mcp-transports, 2026-08-30:
#
#   The corpus hashes a fixed list of source files (`SOURCE_INPUTS`) BY PATH,
#   read from disk at test time. `crates/wcore-cli/src/main.rs` is one of them.
#   A lane added 773 lines to that file, ran `cargo nextest run -p wcore-mcp
#   -p wcore-cli`, ran this pre-flight, got 0 from both, and reported green.
#   `cargo nextest run -p wcore-protocol` was 100: two corpus tests red.
#
# The lane's mistake was choosing which crates to test, and the class of that
# mistake is not closable by telling lanes to also run -p wcore-protocol, or by
# diffing the changed-file list against SOURCE_INPUTS -- both of those are
# proxies that need a correct base, a matching path spelling, and someone to
# have remembered. The question asked here instead is the one that actually
# decides, and it is total: IS THE CHECKED-IN CORPUS CURRENT WITH THE TREE ON
# DISK, RIGHT NOW. No diff, no base, no path list, no crate selection. Any
# staleness reddens here whatever the lane edited and whatever it chose to test.
#
# It is the REAL generator, not a reimplementation of the digest -- a second
# implementation of a hash is a thing that drifts silently, and the binary that
# writes the corpus is the only honest oracle for whether the corpus is current.
#
# Remedy when it fails is printed by the binary: `wcore-contract diff` to see
# which manifest keys moved, then `wcore-contract generate` and commit
# crates/wcore-protocol/contracts/desktop/v1/. Only fixture_digest and
# source_inputs_digest may move for a source-hash rebase; schema_digest moving
# means a WIRE change and is not a re-pin.
CORPUS_GATE="cargo run -q -p wcore-protocol --bin wcore-contract -- check"

fail=0
if out="$($CORPUS_GATE 2>&1)"; then
  printf '  ok    %s\n' "$CORPUS_GATE"
else
  printf '  FAIL  %s\n' "$CORPUS_GATE"
  printf '%s\n' "$out" | tail -25 | sed 's/^/        /'
  fail=1
fi

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
