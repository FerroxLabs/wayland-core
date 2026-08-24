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
#   HINT            optional extra sentence appended to the failure annotation
#
# Exit 0 when at least EXPECTED_MIN reports exist AND they carry at least
# MIN_TESTS test cases between them, 1 otherwise.
set -euo pipefail

: "${EVIDENCE_DIR:?EVIDENCE_DIR is required}"
: "${EXPECTED_MIN:?EXPECTED_MIN is required}"
: "${LABEL:?LABEL is required}"
UPSTREAM_RESULT="${UPSTREAM_RESULT:-unknown}"
MIN_TESTS="${MIN_TESTS:-1}"
HINT="${HINT:-}"

mkdir -p "$EVIDENCE_DIR"
FOUND=$(find "$EVIDENCE_DIR" -type f -name "*.xml" | sort)
COUNT=$(printf "%s" "$FOUND" | grep -c . || true)
# A REPORT IS NOT A TEST. `grep -c` exits 1 when no report holds a test case,
# which is exactly the state this gate exists to fail on, so `|| true` keeps
# `pipefail` from turning that into a bare script error instead of the named
# annotation below.
TESTS=$({ find "$EVIDENCE_DIR" -type f -name "*.xml" -exec grep -oh "<testcase" {} + 2>/dev/null || true; } | grep -c . || true)

echo "suite              : $LABEL"
echo "upstream result    : $UPSTREAM_RESULT"
echo "junit report count : $COUNT (need at least $EXPECTED_MIN)"
echo "test case count    : $TESTS (need at least $MIN_TESTS)"
printf "%s\n" "$FOUND"

if [ "$COUNT" -lt "$EXPECTED_MIN" ]; then
  echo "::error title=NO TEST SIGNAL ($LABEL)::${LABEL} produced ${COUNT} JUnit report(s), fewer than the ${EXPECTED_MIN} expected, so the suite did not run to completion. The upstream job result was '${UPSTREAM_RESULT}'. A leg that dies before its test step, or that skips it for a missing credential, leaves no artifact — without this check the report job would go GREEN having certified nothing. ${HINT}"
  exit 1
fi

if [ "$TESTS" -lt "$MIN_TESTS" ]; then
  echo "::error title=NO TEST SIGNAL ($LABEL)::${LABEL} produced ${COUNT} JUnit report(s) holding ${TESTS} test case(s), fewer than the ${MIN_TESTS} expected. The files exist and certify NOTHING: nextest writes a junit.xml with tests=0 for a run whose filter matched no test at all (an unset cargo feature, a renamed test, a typo in -E), so an artifact proves the command ran, never that a test did. The upstream job result was '${UPSTREAM_RESULT}'. ${HINT}"
  exit 1
fi
