#!/usr/bin/env bash
# ADMISSION: unconditional -- every workflow step that runs this script must
# be admitted by a status-check function (`always()` / `!cancelled()`). An
# absence of Windows signal is most worth saying when something else in the
# job has also gone wrong, and a plain condition is skipped exactly then.
#
# A SKIPPED WINDOWS LEG IS NOT WINDOWS COVERAGE (gh#1146).
#
# `report` is a REQUIRED status context on main and it aggregates the whole
# run, so whatever it concludes is what a reviewer, a release and a dashboard
# read as "CI passed". It has never said a word about WHICH platforms produced
# the verdict it is publishing.
#
# The measured instance: on PR #341 `CI (windows-latest, hosted)` was SKIPPED
# (that leg is opt-in behind a `[ci-windows]` commit marker and is deliberately
# NOT a required check — see its own comment block), and the overall report
# still read as passing. A skipped leg contributes no red, so it silently
# contributes to a green. #1146 filed that as the unearned green, and it is a
# different defect from the three tests that fail on hosted Windows — which is
# why this script ANNOTATES and never fails: the point is honesty about
# coverage, not gating main on a known-red Windows test.
#
# What counts as coverage here is EVIDENCE, not a job conclusion: the JUnit
# artifact a Windows leg uploads, holding at least one <testcase>. A job result
# cannot tell "ran and passed" from "was never scheduled" — `needs.ci.result`
# is the whole matrix, and it reads `success` on a lane push whose matrix
# vector never contained the Windows entry at all. The leg results are still
# reported, as the EXPLANATION of an absence rather than as the test for it.
#
# Inputs (env):
#   EVIDENCE_DIR          directory the JUnit artifacts were downloaded into
#   SELF_HOSTED_ARTIFACT  artifact dir for the `CI (Array)` self-hosted leg.
#                         `Array` is not a typo: the matrix entry is a LABEL
#                         LIST, and a GitHub expression renders a list
#                         interpolated into a string as `Array` — which is also
#                         why the check itself is named `CI (Array)`. Verified
#                         against live runs 33067407466 / 33067301867.
#   HOSTED_ARTIFACT       artifact dir for the `CI (windows-latest, hosted)` leg
#   SELF_HOSTED_RESULT    needs.ci.result
#   HOSTED_RESULT         needs.ci-windows-hosted.result
#
# ALWAYS EXITS 0. Enforced (wiring + behaviour, both directions) by
# scripts/check-windows-attribution.py.
set -uo pipefail

EVIDENCE_DIR="${EVIDENCE_DIR:-junit-reports}"
SELF_HOSTED_ARTIFACT="${SELF_HOSTED_ARTIFACT:-nextest-junit-Array}"
HOSTED_ARTIFACT="${HOSTED_ARTIFACT:-nextest-junit-windows-latest-hosted}"
SELF_HOSTED_RESULT="${SELF_HOSTED_RESULT:-unknown}"
HOSTED_RESULT="${HOSTED_RESULT:-unknown}"
SUMMARY="${GITHUB_STEP_SUMMARY:-/dev/null}"

# COUNTING FILES IS NOT ENOUGH — nextest writes a junit.xml with tests="0" for a
# run whose filter matched nothing, so a file proves an invocation, never a
# test. Same reasoning as .github/scripts/assert-test-evidence.sh's MIN_TESTS.
count_cases() { # count_cases <dir>
  local dir=$1
  if [ ! -d "$dir" ]; then
    echo 0
    return 0
  fi
  { find "$dir" -type f -name '*.xml' -exec grep -oh '<testcase' {} + 2>/dev/null || true; } | grep -c . || true
}

SELF_HOSTED_CASES=$(count_cases "$EVIDENCE_DIR/$SELF_HOSTED_ARTIFACT")
HOSTED_CASES=$(count_cases "$EVIDENCE_DIR/$HOSTED_ARTIFACT")
TOTAL=$((SELF_HOSTED_CASES + HOSTED_CASES))

# `skipped` is the state this whole script exists for, so it is spelled out
# rather than left as a bare job result the reader has to interpret.
leg_state() { # leg_state <cases> <job result>
  if [ "$1" -gt 0 ]; then
    echo "ran ($1 test case(s))"
  elif [ "$2" = "skipped" ]; then
    echo "SKIPPED - no Windows test ran on this leg"
  else
    echo "no test report uploaded (job result: $2)"
  fi
}

SELF_HOSTED_STATE=$(leg_state "$SELF_HOSTED_CASES" "$SELF_HOSTED_RESULT")
HOSTED_STATE=$(leg_state "$HOSTED_CASES" "$HOSTED_RESULT")

# "One Windows leg ran" and "both did" are not the same statement, and the
# difference is the whole subject of #1146. Measured on PR #341: `CI (Array)`
# FAILED, `CI (windows-latest, hosted)` was SKIPPED, and `report` concluded
# SUCCESS. So the count of legs that ran, and the count that were skipped, go
# in the HEADLINE — a reader who never opens the table still sees the gap.
RAN=0
SKIPPED=0
for state in "$SELF_HOSTED_STATE" "$HOSTED_STATE"; do
  case "$state" in
    ran*) RAN=$((RAN + 1)) ;;
    SKIPPED*) SKIPPED=$((SKIPPED + 1)) ;;
  esac
done

echo "evidence dir                    : $EVIDENCE_DIR"
echo "artifacts downloaded            : $(ls -1 "$EVIDENCE_DIR" 2>/dev/null | tr '\n' ' ')"
echo "CI (Array)                      : $SELF_HOSTED_STATE"
echo "CI (windows-latest, hosted)     : $HOSTED_STATE"
echo "needs.ci.result                 : $SELF_HOSTED_RESULT"
echo "needs.ci-windows-hosted.result  : $HOSTED_RESULT"
echo "windows test cases in this run  : $TOTAL"

if [ "$TOTAL" -gt 0 ]; then
  echo "Windows: exercised - $TOTAL test case(s) from $RAN of 2 Windows legs; $SKIPPED SKIPPED (a skipped leg is not coverage)"
  {
    echo "### Windows: exercised ($RAN of 2 legs ran, $SKIPPED SKIPPED)"
    echo
    echo "| Windows leg | state |"
    echo "| --- | --- |"
    echo "| \`CI (Array)\` (self-hosted) | $SELF_HOSTED_STATE |"
    echo "| \`CI (windows-latest, hosted)\` | $HOSTED_STATE |"
    echo
    echo "A leg marked SKIPPED ran no Windows test and certifies nothing."
  } >> "$SUMMARY"
  exit 0
fi

echo "::warning title=Windows: not exercised::This run published a test report with NO Windows test result in it. Every Windows leg was skipped or produced no JUnit, so the report conclusion certifies nothing about Windows - a skipped leg is not a pass (gh#1146)."
{
  echo "### Windows: not exercised"
  echo
  echo "**This \`report\` conclusion covers no Windows test result.** A skipped leg"
  echo "contributes no red, so it silently contributes to a green; it is not coverage."
  echo
  echo "| Windows leg | state |"
  echo "| --- | --- |"
  echo "| \`CI (Array)\` (self-hosted) | $SELF_HOSTED_STATE |"
  echo "| \`CI (windows-latest, hosted)\` | $HOSTED_STATE |"
  echo
  echo "To get a Windows verdict, push with \`[ci-windows]\` in the commit message."
} >> "$SUMMARY"
exit 0
