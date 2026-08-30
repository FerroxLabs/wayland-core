#!/usr/bin/env bash
# Self-tests + wiring guard for FerroxLabs/wayland-core#325.
#
# Two halves, and both are load-bearing:
#
#   PART A grades the DECISION SCRIPT. It proves the close path fires on a
#          genuinely all-green run (a guard that refuses everything is not a
#          fix) and does not fire for any run where a sibling job failed, was
#          cancelled, was skipped, is missing from the roster, or reported a
#          result the script cannot interpret.
#
#   PART B grades the WIRING, because a correct decision script that nothing
#          calls is worth nothing. It proves the close/report steps left the
#          single job that could not see the run, that the tracker job depends
#          on every scheduled job, and — the guard against this defect
#          returning by omission — that REQUIRED_JOBS names EVERY job the
#          scheduled soak actually runs, computed from the workflow itself.
#
#   PART C RUNS THE JOB. A and B together still only prove that a correct
#          decision exists and that the right strings are present; the
#          criterion (#325 c2) says a RUN whose sibling failed POSTS a red
#          report. soak-tracker-run.test.py drives the real YAML through the
#          real decision script and the real github-script bodies against a
#          stubbed Octokit, and asserts on the API calls that come out — which
#          is the only thing that grades the JOB_RESULTS interpolation, the
#          $GITHUB_OUTPUT hand-off and the report body at all.
#
# Run: bash .github/scripts/tests/soak-tracker-truth.test.sh
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
DECIDE="$ROOT/.github/scripts/soak-tracker-decision.sh"
WF="$ROOT/.github/workflows/nightly-windows-soak.yml"
PASS=0
FAIL=0

ok() {
  PASS=$((PASS + 1))
  printf "ok   %s\n" "$1"
}
bad() {
  FAIL=$((FAIL + 1))
  printf "FAIL %s\n" "$1"
}

# ── PART A — the decision itself ───────────────────────────────────────────

# decide <label> <expected-action> <expected-exit> <required-jobs> <roster>
decide() {
  local label="$1" want_action="$2" want_exit="$3" required="$4" roster="$5"
  local out rc action
  out=$(REQUIRED_JOBS="$required" JOB_RESULTS="$roster" bash "$DECIDE" 2>&1)
  rc=$?
  action=$(printf '%s\n' "$out" | grep -E '^action=' | tail -1 | cut -d= -f2)
  if [ "$action" = "$want_action" ] && [ "$rc" = "$want_exit" ]; then
    ok "$label"
  else
    bad "$label (want action=$want_action exit=$want_exit; got action=${action:-<none>} exit=$rc)"
    printf '%s\n' "$out" | sed 's/^/       | /'
  fi
}

REQ="windows-soak keyring-blob-size windows-live-acceptance"

# THE NEGATIVE CONTROL FOR THE WHOLE FIX. A genuinely all-green run must still
# close the issue. Without this, "never close" would pass every other case here.
decide "all three jobs green -> close" close 0 "$REQ" \
  'windows-soak=success
keyring-blob-size=success
windows-live-acceptance=success'

# THE DEFECT, EXACTLY AS MEASURED: reporting job green, sibling job red.
decide "soak green + live-acceptance red -> report, never close" report 0 "$REQ" \
  'windows-soak=success
keyring-blob-size=success
windows-live-acceptance=failure'

decide "keyring leg red -> report" report 0 "$REQ" \
  'windows-soak=success
keyring-blob-size=failure
windows-live-acceptance=success'

decide "soak itself red -> report" report 0 "$REQ" \
  'windows-soak=failure
keyring-blob-size=success
windows-live-acceptance=success'

# A skipped or cancelled sibling is not a pass. The run did not prove green, so
# nothing is closed — and nothing is reported either, because nothing failed.
decide "a skipped sibling closes nothing" none 0 "$REQ" \
  'windows-soak=success
keyring-blob-size=success
windows-live-acceptance=skipped'

decide "a cancelled sibling closes nothing" none 0 "$REQ" \
  'windows-soak=success
keyring-blob-size=cancelled
windows-live-acceptance=success'

# `needs.<typo>.result` expands to the empty string, which a naive
# `!= 'failure'` test reads as green. Fail closed, and loudly: this is a wiring
# defect, not a product one.
decide "an empty result is uninterpretable, not green" none 1 "$REQ" \
  'windows-soak=success
keyring-blob-size=
windows-live-acceptance=success'

decide "a definite failure outranks an uninterpretable sibling" report 0 "$REQ" \
  'windows-soak=failure
keyring-blob-size=
windows-live-acceptance=success'

# The omission guard: a job that exists but was left out of the roster.
decide "a required job missing from the roster closes nothing" none 1 "$REQ" \
  'windows-soak=success
keyring-blob-size=success'

decide "an empty roster closes nothing" none 1 "$REQ" ''

# ── PART B — the wiring ────────────────────────────────────────────────────

want_grep() { # want_grep <label> <file> <fixed-pattern>
  if grep -qF -- "$3" "$2"; then ok "$1"; else bad "$1 (missing: $3)"; fi
}
want_no_grep() { # want_no_grep <label> <file> <fixed-pattern>
  if grep -qF -- "$3" "$2"; then bad "$1 (present but must not be: $3)"; else ok "$1"; fi
}

if [ -f "$DECIDE" ]; then ok "decision script exists"; else bad "decision script exists"; fi

# The two steps must no longer live in a job that can only see itself.
want_no_grep "no job-scoped 'if: success()' step survives in the soak workflow" \
  "$WF" "        if: success()"
want_no_grep "no job-scoped 'if: failure()' step survives in the soak workflow" \
  "$WF" "        if: failure()"

want_grep "a tracker job exists" "$WF" "  soak-tracker:"
want_grep "the tracker runs whatever the siblings did" "$WF" \
  "if: \${{ always() && github.event.inputs.f20_candidate != 'true' }}"
want_grep "the tracker calls the shared decision script" "$WF" \
  "run: bash .github/scripts/soak-tracker-decision.sh"
want_grep "the close step is gated on the whole-run decision" "$WF" \
  "if: \${{ steps.decide.outputs.action == 'close' }}"
want_grep "the report step is gated on the whole-run decision" "$WF" \
  "if: \${{ steps.decide.outputs.action == 'report' }}"

# Least privilege: the soak job no longer writes issues, the tracker does.
if awk '/^  windows-soak:/{inj=1} /^  [a-z0-9_-]+:$/ && !/^  windows-soak:/{inj=0} inj && /issues: write/{found=1} END{exit !found}' "$WF"; then
  bad "the windows-soak job no longer holds issues: write"
else
  ok "the windows-soak job no longer holds issues: write"
fi

# ── The omission guard, computed from the workflow ─────────────────────────
#
# Every top-level job that the SCHEDULED soak runs must be in the tracker's
# needs: and in REQUIRED_JOBS. Candidate-mode-only jobs
# (`if: github.event.inputs.f20_candidate == 'true'`) are excluded because they
# never run on the cron tick, and the tracker job itself cannot depend on
# itself. Derived from the YAML rather than hard-coded here, so a fourth test
# job added tomorrow reds THIS test instead of silently narrowing the tracker's
# view — which is precisely how core#325 happened.
#
# Only the JOB-LEVEL `if:` (indent 4, plus its `>-` continuation lines at
# deeper indent) counts as the candidate marker. A STEP-level
# `if: ... f20_candidate == 'true'` exists inside windows-live-acceptance at
# indent 8; treating that as a job-level condition dropped a real scheduled job
# out of this roster on the first draft, which would have made the guard agree
# with the bug.
SCHEDULED_JOBS=$(awk '
  /^jobs:/ { injobs = 1; next }
  !injobs { next }
  /^  [A-Za-z0-9_-]+:[ \t]*$/ {
    if (job != "" && !candidate && job != "soak-tracker") print job
    job = $1; sub(/:$/, "", job); candidate = 0; inif = 0; next
  }
  /^    [A-Za-z_-]+:/ { inif = ($0 ~ /^    if:/) ? 1 : 0 }
  inif && /f20_candidate == .true./ { candidate = 1 }
  END { if (job != "" && !candidate && job != "soak-tracker") print job }
' "$WF" | sort)

REQUIRED_DECLARED=$(grep -E '^ +REQUIRED_JOBS:' "$WF" | head -1 |
  sed 's/^[^:]*://' | tr -d '"' | tr ' ' '\n' | sed '/^$/d' | sort)

if [ -z "$SCHEDULED_JOBS" ]; then
  bad "workflow job roster parsed (parser found no scheduled jobs)"
elif [ "$SCHEDULED_JOBS" = "$REQUIRED_DECLARED" ]; then
  ok "REQUIRED_JOBS names every job the scheduled soak runs"
else
  bad "REQUIRED_JOBS names every job the scheduled soak runs"
  printf '       | scheduled in workflow: %s\n' "$(printf '%s' "$SCHEDULED_JOBS" | tr '\n' ' ')"
  printf '       | declared REQUIRED_JOBS: %s\n' "$(printf '%s' "$REQUIRED_DECLARED" | tr '\n' ' ')"
fi

# ...and the same set must be in `needs:` of the tracker job, or the results are
# not observable at all.
NEEDS_LINE=$(awk '/^  soak-tracker:/{t=1} t && /^    needs:/{print; exit}' "$WF")
MISSING_NEEDS=""
while IFS= read -r j; do
  [ -z "$j" ] && continue
  case "$NEEDS_LINE" in *"$j"*) ;; *) MISSING_NEEDS="${MISSING_NEEDS}${j} " ;; esac
done <<EOF
${SCHEDULED_JOBS}
EOF
if [ -z "$MISSING_NEEDS" ] && [ -n "$NEEDS_LINE" ]; then
  ok "the tracker job needs: every scheduled job"
else
  bad "the tracker job needs: every scheduled job (missing: ${MISSING_NEEDS:-<no needs: line>})"
fi

# ── PART C — the job, executed ─────────────────────────────────────────────
#
# Skipping this half silently would leave exactly the gap #325 c2 was graded
# against, so a missing dependency is a FAILURE here, never a skip.
RUNNER="$HERE/soak-tracker-run.test.py"
if [ ! -f "$RUNNER" ]; then
  bad "the executed-job harness exists ($RUNNER)"
elif ! python3 -c "import yaml" 2>/dev/null && ! pip install --quiet pyyaml 2>/dev/null; then
  bad "PyYAML is available to read the workflow (install it; do not skip PART C)"
elif ! command -v node >/dev/null 2>&1; then
  bad "node is available to run the github-script bodies (do not skip PART C)"
else
  echo ""
  if python3 "$RUNNER"; then
    ok "PART C: the tracker job posts a red report on a red sibling, and closes only on green"
  else
    bad "PART C: the tracker job did not behave as #325 c2 requires (output above)"
  fi
fi

echo ""
echo "soak-tracker-truth: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
