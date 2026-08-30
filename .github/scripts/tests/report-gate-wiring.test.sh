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

# wayland-core#367 — the failing-SET gate is invoked BY PATH from the shared
# evidence gate, so deleting the script or the call silently stops every run on
# this repository from ever comparing failure identities again. Both halves are
# asserted: the file, and the line that runs it.
SETGATE="$ROOT/.github/scripts/grade-failing-set.sh"
if [ -f "$SETGATE" ]; then ok "failing-set gate script exists"; else bad "failing-set gate script exists"; fi
want_grep "the shared evidence gate runs the failing-set gate" \
  "$GATE" 'bash "$SETGRADER"'
want_grep "the failing-set gate fails closed on its own absence" \
  "$GATE" "Failing-set gate missing"
if [ -f "$ROOT/.config/known-failing-tests.txt" ]; then
  ok "the named allowlist file exists"
else
  bad "the named allowlist file exists (.config/known-failing-tests.txt)"
fi
want_grep "the failing-set self-test runs in lint.yml" \
  "$ROOT/.github/workflows/lint.yml" "bash .github/scripts/tests/failing-set.test.sh"

# Both report jobs must call the ONE gate. Two copies is how they diverged.
want_grep "ci.yml report job runs the shared evidence gate" \
  "$CI" "run: bash .github/scripts/assert-test-evidence.sh"
want_grep "e2e.yml report job runs the shared evidence gate" \
  "$E2E" "run: bash .github/scripts/assert-test-evidence.sh"

# ── ADMISSION: the sweep, over every workflow and every gate script ──────────
#
# Calling the gate is not enough; the step that calls it has to actually RUN.
# ci.yml's caller switched itself off for months while the evidence sat in the
# artifact it had just downloaded (wayland#1177 c1), and e2e.yml's caller still
# carried the same shape after ci.yml's was fixed -- which is the whole reason
# this is a SWEEP and not two more `want_grep`s. gate-admission.py discovers the
# gates from their own `ADMISSION:` declarations and the call sites by parsing
# .github/workflows/, so neither a third caller nor a fourth gate can be missed
# by omission. It proves its own polarity in the same run.
ADMISSION="$HERE/gate-admission.py"
if [ ! -f "$ADMISSION" ]; then
  bad "the admission sweep exists ($ADMISSION)"
elif ! python3 -c "import yaml" 2>/dev/null && ! pip install --quiet pyyaml 2>/dev/null; then
  bad "PyYAML is available to read the workflows (install it; do not skip the sweep)"
else
  sweep_out=$(python3 "$ADMISSION" 2>&1) || true
  sweep_seen=0
  while IFS= read -r line; do
    case "$line" in
      "PASS "*) ok "${line#PASS }"; sweep_seen=$((sweep_seen + 1)) ;;
      "FAIL "*) bad "${line#FAIL }"; sweep_seen=$((sweep_seen + 1)) ;;
      "INFO "*) printf '       | %s\n' "${line#INFO }" ;;
      *) printf '       | %s\n' "$line" ;;
    esac
  done <<SWEEP
$sweep_out
SWEEP
  # The sweep reporting NOTHING must not read as the sweep passing.
  if [ "$sweep_seen" -ge 10 ]; then
    ok "the admission sweep reported its assertions (anti-vacuity)"
  else
    bad "the admission sweep reported its assertions (anti-vacuity; got $sweep_seen)"
  fi
fi

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
#
# THE FORM OF THIS ASSERTION IS THE DEFECT IT NOW GUARDS. Until 2026-08-31 it
# pinned the literal `if: ${{ needs.e2e.result != 'cancelled' }}` -- so it
# REQUIRED the shape that wayland#1177 c1 had just been fixed for in ci.yml,
# and would have reddened the fix. Which exact string a condition is, is not
# what matters; whether the condition can go inert is. The admission sweep
# above decides that for every gate call site in the repository, so all that is
# left here is the one e2e-specific half: `skipped` must not stand it down.
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

# ── RETRY-FLAKE GATE WIRING (wayland#1169) ─────────────────────────────────
#
# assert-test-evidence.test.sh proves the retry gate itself fails on a retried
# failure. This proves it is REACHED. It is invoked from inside
# assert-test-evidence.sh rather than from a workflow step of its own,
# deliberately: that call site already runs in both aggregate report jobs, so
# there is no second piece of YAML to keep in sync and no way to wire it into
# one report job and not the other — which is exactly how #1115 happened.
GRADE="$ROOT/.github/scripts/grade-retry-flakes.sh"
ALLOWFILE="$ROOT/.config/flaky-allowlist.txt"

if [ -f "$GRADE" ]; then ok "retry-flake grader exists"; else bad "retry-flake grader exists"; fi
if [ -f "$ALLOWFILE" ]; then ok "flake allowlist exists"; else bad "flake allowlist exists"; fi

want_grep "the evidence gate invokes the retry-flake grader" \
  "$GATE" 'bash "$GRADER"'
# A gate that can be silently deleted is worth as little as one that cannot
# fail, and this one is invoked by path rather than by import.
want_grep "the evidence gate fails closed if the grader is deleted" \
  "$GATE" "Retry-flake gate missing"

# The gate is only worth wiring while retries are on. If `[profile.ci]` ever
# drops to `retries = 0` this whole mechanism becomes dead code and the comment
# trail above becomes a lie, so say so out loud rather than leaving a check that
# silently cannot fire.
NEXTEST="$ROOT/.config/nextest.toml"
if awk '/^\[profile\.ci\]/{p=1;next} /^\[/{p=0} p && /^retries[[:space:]]*=/{print; found=1} END{exit !found}' \
     "$NEXTEST" | grep -q 'retries[[:space:]]*=[[:space:]]*[1-9]'; then
  ok "[profile.ci] still retries, so the gate has something to grade"
else
  bad "[profile.ci] no longer retries — grade-retry-flakes.sh is now dead code, remove it or the retries"
fi

echo "---"
echo "passed: $PASS  failed: $FAIL"
[ "$FAIL" -eq 0 ]
