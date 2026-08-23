#!/usr/bin/env bash
# Positive control for .github/scripts/assert-test-evidence.sh (wayland#1115).
#
# The defect being guarded is a gate that cannot fail, so the first case here is
# the reproduction: the exact state of e2e.yml's report job on PR #315 head
# ae389c3e — the suite was skipped for a missing credential, zero JUnit reports
# were downloaded, and the job concluded SUCCESS. That case MUST exit non-zero.
# Every "must fail" case is paired with a "must pass" case over the same code so
# a script that simply always failed would not satisfy this file.
#
# Run: bash .github/scripts/tests/assert-test-evidence.test.sh
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
SCRIPT="$HERE/../assert-test-evidence.sh"
PASS=0
FAIL=0

run_case() {
  # run_case <name> <expected_exit> <reports_dir> <expected_min> <upstream> [min_tests]
  local name=$1 want=$2 dir=$3 min=$4 upstream=$5 mintests=${6:-1}
  local out rc
  out=$(EVIDENCE_DIR="$dir" EXPECTED_MIN="$min" LABEL="E2E Tests" \
        MIN_TESTS="$mintests" UPSTREAM_RESULT="$upstream" bash "$SCRIPT" 2>&1)
  rc=$?
  if [ "$rc" -eq "$want" ]; then
    PASS=$((PASS + 1))
    printf "ok   %-58s exit=%s\n" "$name" "$rc"
  else
    FAIL=$((FAIL + 1))
    printf "FAIL %-58s exit=%s want=%s\n" "$name" "$rc" "$want"
    printf "%s\n" "$out" | sed "s/^/       | /"
  fi
}

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# A junit report that actually certifies something...
REAL='<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="1" failures="0" errors="0">
<testsuite name="wcore-agent::e2e" tests="1"><testcase name="t" classname="c" time="0.1"/></testsuite>
</testsuites>'
# ...and the one nextest ACTUALLY wrote on this tree for a filter that matched
# nothing. Captured verbatim from
#   cargo nextest run -p wcore-agent --profile e2e --test e2e -E 'test(anthropic)'
# which exited 4 with "no tests to run" and still produced this file. The
# file-count form of this gate accepted it as proof the suite ran.
EMPTY='<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="0" failures="0" errors="0" uuid="928e89c4" timestamp="2026-08-23T15:41:56.194+00:00" time="0.000">
</testsuites>'

# 1. THE REPRODUCTION: suite skipped for a missing key, nothing downloaded.
mkdir -p "$TMP/empty"
run_case "skipped suite, zero reports -> RED" 1 "$TMP/empty" 2 success

# 2. Control: the artifact directory does not even exist (download-artifact
#    with continue-on-error and no matching artifacts leaves no directory).
run_case "no evidence directory at all -> RED" 1 "$TMP/absent" 2 success

# 3. Control that the gate can PASS — otherwise it is a permanently-red gate,
#    which is worth exactly as little as a permanently-green one.
mkdir -p "$TMP/full/e2e-junit-anthropic" "$TMP/full/e2e-junit-openai"
printf "%s\n" "$REAL" > "$TMP/full/e2e-junit-anthropic/junit.xml"
printf "%s\n" "$REAL" > "$TMP/full/e2e-junit-openai/junit.xml"
run_case "both legs reported -> GREEN" 0 "$TMP/full" 2 success

# 4. Partial: one leg reported, one did not. Half a suite is not the suite.
mkdir -p "$TMP/half/e2e-junit-anthropic"
printf "%s\n" "$REAL" > "$TMP/half/e2e-junit-anthropic/junit.xml"
run_case "one leg of two reported -> RED" 1 "$TMP/half" 2 success

# 5. Same partial evidence, but only one leg was in scope (workflow_dispatch
#    with a single provider): that is a complete suite and must pass.
run_case "one leg in scope, one leg reported -> GREEN" 0 "$TMP/half" 1 success

# 5b. THE SECOND REPRODUCTION (adversarial review, 2026-08-23): the expected
#     number of report files are present and every one of them is nextest's
#     zero-match junit.xml. File count says "the suite ran"; the reports
#     certify nothing. This is wayland#1115 one layer down, and it is the state
#     this repo reaches the moment the API secrets are configured, because the
#     workflow ran the e2e binary without its `live-*` cargo features and so
#     matched zero tests.
mkdir -p "$TMP/zero/e2e-junit-anthropic" "$TMP/zero/e2e-junit-openai"
printf "%s\n" "$EMPTY" > "$TMP/zero/e2e-junit-anthropic/junit.xml"
printf "%s\n" "$EMPTY" > "$TMP/zero/e2e-junit-openai/junit.xml"
run_case "two reports, zero test cases -> RED" 1 "$TMP/zero" 2 success

# 5c. Paired control over the SAME code path: identical file count, identical
#     EXPECTED_MIN, but the reports hold test cases. Proves 5b fails on the
#     test count and not merely on being a stricter script.
run_case "two reports holding test cases -> GREEN" 0 "$TMP/full" 2 success

# 5d. One real report and one empty one still clears MIN_TESTS=1: the file
#     count is what catches a half-run suite, and it already does (case 4).
mkdir -p "$TMP/mixed/a" "$TMP/mixed/b"
printf "%s\n" "$REAL" > "$TMP/mixed/a/junit.xml"
printf "%s\n" "$EMPTY" > "$TMP/mixed/b/junit.xml"
run_case "one real report and one empty -> GREEN at MIN_TESTS=1" 0 "$TMP/mixed" 2 success
run_case "...and RED when both legs must carry tests" 1 "$TMP/mixed" 2 success 2

# 6. A non-xml file is not evidence.
mkdir -p "$TMP/junk"
printf "not a report\n" > "$TMP/junk/junit.txt"
run_case "non-xml artifact is not evidence -> RED" 1 "$TMP/junk" 1 success

# 7. Upstream failure with reports present still passes this gate: publishing
#    the failures is the point, and the leg itself is already red.
run_case "upstream failed but reports exist -> GREEN" 0 "$TMP/full" 2 failure

# 8. Missing required input is a hard error, not a silent pass.
out=$(EVIDENCE_DIR="$TMP/full" LABEL="E2E Tests" bash "$SCRIPT" 2>&1)
rc=$?
if [ "$rc" -ne 0 ]; then
  PASS=$((PASS + 1)); printf "ok   %-58s exit=%s\n" "unset EXPECTED_MIN -> RED" "$rc"
else
  FAIL=$((FAIL + 1)); printf "FAIL %-58s exit=%s want!=0\n" "unset EXPECTED_MIN -> RED" "$rc"
fi

# The failure annotation must be the one CI surfaces, not a bare exit code.
# (captured, not piped: `pipefail` would report the script's own exit 1 for the
# whole pipeline and mask grep's verdict.)
annotation=$(EVIDENCE_DIR="$TMP/empty" EXPECTED_MIN=2 LABEL="E2E Tests" UPSTREAM_RESULT=success \
   bash "$SCRIPT" 2>&1)
if printf "%s" "$annotation" | grep -q "::error title=NO TEST SIGNAL (E2E Tests)::"; then
  PASS=$((PASS + 1)); printf "ok   %-58s\n" "failure emits a named ::error annotation"
else
  FAIL=$((FAIL + 1)); printf "FAIL %-58s\n" "failure emits a named ::error annotation"
fi

zero_annotation=$(EVIDENCE_DIR="$TMP/zero" EXPECTED_MIN=2 MIN_TESTS=1 LABEL="E2E Tests" \
   UPSTREAM_RESULT=success bash "$SCRIPT" 2>&1)
if printf "%s" "$zero_annotation" | grep -q "holding 0 test case(s)"; then
  PASS=$((PASS + 1)); printf "ok   %-58s\n" "zero-test failure names the test count"
else
  FAIL=$((FAIL + 1)); printf "FAIL %-58s\n" "zero-test failure names the test count"
fi

echo "---"
echo "passed: $PASS  failed: $FAIL"
[ "$FAIL" -eq 0 ]
