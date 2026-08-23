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
  # run_case <name> <expected_exit> <reports_dir> <expected_min> <upstream>
  local name=$1 want=$2 dir=$3 min=$4 upstream=$5
  local out rc
  out=$(EVIDENCE_DIR="$dir" EXPECTED_MIN="$min" LABEL="E2E Tests" \
        UPSTREAM_RESULT="$upstream" bash "$SCRIPT" 2>&1)
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

# 1. THE REPRODUCTION: suite skipped for a missing key, nothing downloaded.
mkdir -p "$TMP/empty"
run_case "skipped suite, zero reports -> RED" 1 "$TMP/empty" 2 success

# 2. Control: the artifact directory does not even exist (download-artifact
#    with continue-on-error and no matching artifacts leaves no directory).
run_case "no evidence directory at all -> RED" 1 "$TMP/absent" 2 success

# 3. Control that the gate can PASS — otherwise it is a permanently-red gate,
#    which is worth exactly as little as a permanently-green one.
mkdir -p "$TMP/full/e2e-junit-anthropic" "$TMP/full/e2e-junit-openai"
printf "<testsuites/>\n" > "$TMP/full/e2e-junit-anthropic/junit.xml"
printf "<testsuites/>\n" > "$TMP/full/e2e-junit-openai/junit.xml"
run_case "both legs reported -> GREEN" 0 "$TMP/full" 2 success

# 4. Partial: one leg reported, one did not. Half a suite is not the suite.
mkdir -p "$TMP/half/e2e-junit-anthropic"
printf "<testsuites/>\n" > "$TMP/half/e2e-junit-anthropic/junit.xml"
run_case "one leg of two reported -> RED" 1 "$TMP/half" 2 success

# 5. Same partial evidence, but only one leg was in scope (workflow_dispatch
#    with a single provider): that is a complete suite and must pass.
run_case "one leg in scope, one leg reported -> GREEN" 0 "$TMP/half" 1 success

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

echo "---"
echo "passed: $PASS  failed: $FAIL"
[ "$FAIL" -eq 0 ]
