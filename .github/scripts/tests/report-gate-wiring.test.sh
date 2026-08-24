#!/usr/bin/env bash
# Wiring guard for wayland#1115.
#
# assert-test-evidence.test.sh proves the gate SCRIPT fails on a suite that did
# not run. This file proves the script is actually WIRED to both aggregate
# report jobs, and that the two no longer emit the same check name. Deleting
# the step, or renaming a job back, reopens #1115 with every other test green —
# that is precisely how e2e.yml ended up without the check ci.yml already had.
#
# Run: bash .github/scripts/tests/report-gate-wiring.test.sh
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
CI="$ROOT/.github/workflows/ci.yml"
E2E="$ROOT/.github/workflows/e2e.yml"
GATE="$ROOT/.github/scripts/assert-test-evidence.sh"
PASS=0
FAIL=0

ok()   { PASS=$((PASS + 1)); printf "ok   %s\n" "$1"; }
bad()  { FAIL=$((FAIL + 1)); printf "FAIL %s\n" "$1"; }

want_grep() { # want_grep <label> <file> <fixed-pattern>
  if grep -qF -- "$3" "$2"; then ok "$1"; else bad "$1 (missing: $3)"; fi
}
want_no_grep() { # want_no_grep <label> <file> <fixed-pattern>
  if grep -qF -- "$3" "$2"; then bad "$1 (present but must not be: $3)"; else ok "$1"; fi
}

# The gate must exist and be runnable.
if [ -f "$GATE" ]; then ok "gate script exists"; else bad "gate script exists"; fi

# Both report jobs must call the ONE gate. Two copies is how they diverged.
want_grep "ci.yml report job runs the shared evidence gate" \
  "$CI" "run: bash .github/scripts/assert-test-evidence.sh"
want_grep "e2e.yml report job runs the shared evidence gate" \
  "$E2E" "run: bash .github/scripts/assert-test-evidence.sh"

# `report` is a REQUIRED status context on main. Exactly one job may emit it,
# and it must be pinned by an explicit name rather than by a job id.
want_grep "ci.yml pins the required check name" "$CI" "    name: report"
want_grep "e2e.yml report job has a distinct name" "$E2E" "    name: E2E report"
if grep -qE "^  report:" "$E2E"; then
  bad "e2e.yml no longer defines a job id 'report'"
else
  ok "e2e.yml no longer defines a job id 'report'"
fi

# The e2e gate must NOT be skipped when the e2e job itself was skipped: "no leg
# ran at all" is the failure it exists to catch, not a reason to stand down.
want_grep "e2e gate runs unless the suite was cancelled" \
  "$E2E" "if: \${{ needs.e2e.result != 'cancelled' }}"
want_no_grep "e2e gate does not stand down on a skipped suite" \
  "$E2E" "needs.e2e.result != 'skipped'"

# merge-multiple collapses both legs' junit.xml onto one path, which would cap
# the evidence count at 1 and blind the gate to a half-run suite.
# Matched as a YAML KEY, not as text: the comment above that step explains why
# the flag was removed and would satisfy a bare substring search.
if grep -qE "^[[:space:]]+merge-multiple:" "$E2E"; then
  bad "e2e download does not merge artifacts onto one path"
else
  ok "e2e download does not merge artifacts onto one path"
fi

# A credential-less leg must fail rather than conclude success on zero tests.
want_grep "a missing E2E credential fails its leg" "$E2E" "NO E2E CREDENTIAL"

# ...and a CREDENTIALLED leg must actually have tests to run. Every file under
# tests/e2e/ is `#![cfg(feature = "live-...")]`, so an invocation without the
# feature compiles an EMPTY test binary, matches nothing, and still writes a
# junit.xml — evidence by file count, nothing by test count.
want_grep "the e2e run passes the live-* cargo feature" \
  "$E2E" "--features \${{ matrix.features }}"
want_grep "the anthropic leg names its feature" "$E2E" "features: live-anthropic"
want_grep "the openai leg names its feature" "$E2E" "features: live-openai"

# The evidence gate must be told to require test cases, not just files.
want_grep "the e2e evidence gate requires real test cases" "$E2E" "MIN_TESTS: 1"

# Out-of-scope legs must stay out of scope: a single-provider dispatch must not
# check out, build and run the other provider's suite.
if [ "$(grep -cF -- "if: steps.filter.outputs.run == 'true'" "$E2E")" -ge 5 ]; then
  ok "every e2e work step is guarded by the provider filter"
else
  bad "every e2e work step is guarded by the provider filter (found $(grep -cF -- "if: steps.filter.outputs.run == 'true'" "$E2E"), need 5)"
fi

# Control: the same matchers must be capable of reporting absence, or every
# want_no_grep above would pass vacuously.
if grep -qF -- "definitely-not-in-this-file-$$" "$E2E"; then
  bad "control: matcher reports a bogus pattern as present"
else
  ok "control: matcher finds nothing for a bogus pattern"
fi
# ...and capable of reporting presence.
if grep -qF -- "name: E2E Tests" "$E2E" || grep -qF -- "name: E2E Tests" "$E2E"; then
  ok "control: matcher finds a pattern that is present"
else
  bad "control: matcher missed a pattern that is present"
fi

echo "---"
echo "passed: $PASS  failed: $FAIL"
[ "$FAIL" -eq 0 ]
