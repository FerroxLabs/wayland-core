#!/usr/bin/env bash
# A TEST SUITE THAT NEVER RAN MUST NOT BE REPORTABLE AS ANYTHING BUT A FAILURE.
#
# Both aggregate "report" jobs (ci.yml and e2e.yml) publish a JUnit summary and
# both used to conclude SUCCESS when there was no JUnit at all: the publish step
# is guarded by `hashFiles(...) != `, so zero reports means the only step that
# does anything is skipped and the job ends green. `report` is a REQUIRED status
# context, so that green was read as evidence by every reviewer and by the
# release process while certifying nothing (wayland#1115; ci.yml hit the same
# shape earlier via lane/fix-clippy-gate, when Windows read green having run no
# tests since 4d5f8ec9).
#
# This script is the single implementation of that gate, shared by both jobs so
# they cannot drift apart again — divergence is exactly how e2e.yml ended up
# without the check ci.yml already had.
#
# Inputs (env):
#   EVIDENCE_DIR    directory the JUnit artifacts were downloaded into
#   EXPECTED_MIN    minimum number of *.xml reports that must be present
#   LABEL           human name of the suite, used in the failure annotation
#   UPSTREAM_RESULT result of the job that was supposed to produce them
#   MIN_TESTS       minimum number of <testcase> elements across all reports
#                   (default 1). COUNTING FILES IS NOT ENOUGH: nextest writes a
#                   junit.xml even for a run whose filter matched zero tests, so
#                   a report file proves an invocation happened, never that a
#                   test did. Measured on this tree, the e2e command this
#                   workflow runs emitted `<testsuites ... tests="0">` while
#                   exiting 4 -- and the file-count form of this gate accepted
#                   that as evidence, which is wayland#1115 again one layer
#                   down.
#   REQUIRE_LEGS    optional space/comma separated list of artifact directory
#                   names directly under EVIDENCE_DIR, each of which must
#                   contribute at least one coverage report. See the per-leg
#                   note below.
#   HINT            optional extra sentence appended to the failure annotation
#
# ── A PER-LEG FLOOR, BECAUSE A REPO-WIDE ONE CANNOT SEE A MISSING LEG ──────
#
# EXPECTED_MIN is counted across every leg aggregated into EVIDENCE_DIR, so one
# leg uploading a report satisfies it even when the leg that runs the FULL
# workspace suite uploaded nothing -- and `if-no-files-found: ignore` on every
# upload step makes that silent. That is wayland#1115 one level finer: a green
# `report` over a suite that never ran, per leg rather than per repo. Measured
# instance: on run 33227927478 the whole `ci-linux` leg died before nextest and
# produced zero XML while `report` still had macOS reports to count.
#
# REQUIRE_LEGS names the legs whose absence is not survivable. It is opt-in
# because the `ci` matrix legs are conditioned per platform and a blanket
# per-leg floor would be a different, riskier gate.
#
# ── PRESERVED FAILURES ARE NOT COVERAGE ───────────────────────────────────
#
# `outer-attempt-<N>.xml` (wayland#1177) is a copy of a FAILED attempt's report,
# preserved so the retry-flake grader can see it. It lands inside the same
# artifact and matches the same `*.xml` glob, so counting it toward EXPECTED_MIN
# or MIN_TESTS lets a leg's preserved failures stand in for the coverage they
# are evidence of losing. They are excluded from both counts here and read only
# by grade-retry-flakes.sh, which is the file that is actually about them.
#
# Exit 0 when at least EXPECTED_MIN coverage reports exist, they carry at least
# MIN_TESTS test cases between them, and every leg named in REQUIRE_LEGS
# contributed at least one of them; 1 otherwise.
set -euo pipefail

: "${EVIDENCE_DIR:?EVIDENCE_DIR is required}"
: "${EXPECTED_MIN:?EXPECTED_MIN is required}"
: "${LABEL:?LABEL is required}"
UPSTREAM_RESULT="${UPSTREAM_RESULT:-unknown}"
MIN_TESTS="${MIN_TESTS:-1}"
REQUIRE_LEGS="${REQUIRE_LEGS:-}"
HINT="${HINT:-}"

mkdir -p "$EVIDENCE_DIR"
# `! -name outer-attempt-*.xml`: a preserved failed attempt is not coverage.
# See the note above.
coverage_reports() { # coverage_reports [root]
  find "${1:-$EVIDENCE_DIR}" -type f -name "*.xml" ! -name "outer-attempt-*.xml" | sort
}
FOUND=$(coverage_reports)
COUNT=$(printf "%s" "$FOUND" | grep -c . || true)
PRESERVED=$(find "$EVIDENCE_DIR" -type f -name "outer-attempt-*.xml" | sort)
PRESERVED_COUNT=$(printf "%s" "$PRESERVED" | grep -c . || true)
# A REPORT IS NOT A TEST. `grep -c` exits 1 when no report holds a test case,
# which is exactly the state this gate exists to fail on, so `|| true` keeps
# `pipefail` from turning that into a bare script error instead of the named
# annotation below.
TESTS=$({ find "$EVIDENCE_DIR" -type f -name "*.xml" ! -name "outer-attempt-*.xml" -exec grep -oh "<testcase" {} + 2>/dev/null || true; } | grep -c . || true)

echo "suite              : $LABEL"
echo "upstream result    : $UPSTREAM_RESULT"
echo "junit report count : $COUNT (need at least $EXPECTED_MIN)"
echo "test case count    : $TESTS (need at least $MIN_TESTS)"
echo "preserved attempts : $PRESERVED_COUNT (wayland#1177; not counted as coverage)"
echo "required legs      : ${REQUIRE_LEGS:-<none>}"
printf "%s\n" "$FOUND"

if [ "$COUNT" -lt "$EXPECTED_MIN" ]; then
  echo "::error title=NO TEST SIGNAL ($LABEL)::${LABEL} produced ${COUNT} JUnit report(s), fewer than the ${EXPECTED_MIN} expected, so the suite did not run to completion. The upstream job result was '${UPSTREAM_RESULT}'. A leg that dies before its test step, or that skips it for a missing credential, leaves no artifact — without this check the report job would go GREEN having certified nothing. ${HINT}"
  exit 1
fi

if [ "$TESTS" -lt "$MIN_TESTS" ]; then
  echo "::error title=NO TEST SIGNAL ($LABEL)::${LABEL} produced ${COUNT} JUnit report(s) holding ${TESTS} test case(s), fewer than the ${MIN_TESTS} expected. The files exist and certify NOTHING: nextest writes a junit.xml with tests=0 for a run whose filter matched no test at all (an unset cargo feature, a renamed test, a typo in -E), so an artifact proves the command ran, never that a test did. The upstream job result was '${UPSTREAM_RESULT}'. ${HINT}"
  exit 1
fi

# ── EVERY NAMED LEG MUST HAVE CONTRIBUTED (wayland#1177 c2 / D34) ──────────
#
# The two gates above are counted across all legs, so they cannot notice that
# one particular leg contributed nothing. This one can.
for leg in ${REQUIRE_LEGS//,/ }; do
  leg_dir="$EVIDENCE_DIR/$leg"
  leg_reports=$(coverage_reports "$leg_dir" 2>/dev/null || true)
  leg_count=$(printf "%s" "$leg_reports" | grep -c . || true)
  echo "leg $leg: $leg_count coverage report(s)"
  if [ "$leg_count" -lt 1 ]; then
    echo "::error title=NO TEST SIGNAL ($leg)::The '${leg}' leg contributed ZERO JUnit coverage reports to ${LABEL}, so nothing it was supposed to run is certified by this check. The other legs' reports satisfied the aggregate count, which is exactly why this per-leg floor exists (wayland#1177 c2). Its artifact uploads with 'if-no-files-found: ignore', so a leg that dies before its test step -- or a wrapper that dies before invoking nextest -- disappears silently. The upstream job result was '${UPSTREAM_RESULT}'. ${HINT}"
    exit 1
  fi
done

# ── A RETRIED FAILURE IS A SIGNAL, NOT SILENCE (wayland#1169) ───────────────
#
# The gate above proves the suite RAN. It says nothing about a test that ran,
# FAILED, and was retried into a pass — which `[profile.ci] retries = 2`
# converts into a green run conclusion with the evidence buried in a log nobody
# reads. Measured cost: the #1155 data-loss race failed 6.5 % of runs at
# `--retries 0` and would have been reported roughly once in 3,600 CI runs.
#
# Delegated rather than inlined so each file keeps one responsibility, and
# invoked from HERE rather than from a new workflow step so it inherits the
# wiring both `report` jobs already have — the shared-single-implementation
# discipline this file was written for in the first place. In e2e.yml it is a
# no-op by construction (`[profile.e2e] retries = 0` cannot emit a flake).
#
# FAIL-CLOSED ON ITS OWN ABSENCE. A gate that can be silently deleted is worth
# as little as one that cannot fail, and this one is invoked by path.
GRADER="$(cd "$(dirname "$0")" && pwd)/grade-retry-flakes.sh"
if [ ! -f "$GRADER" ]; then
  echo "::error title=Retry-flake gate missing::${GRADER} is not present, so no run on this repository is grading retried failures (wayland#1169). Restore it rather than removing this call."
  exit 1
fi
echo ""
bash "$GRADER"
