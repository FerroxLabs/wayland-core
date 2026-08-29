#!/usr/bin/env bash
# Self-tests + wiring guard for FerroxLabs/wayland#1177.
#
# The defect: ci.yml's outer `nick-fields/retry@v3` re-runs nextest, and the
# second attempt overwrites `target/nextest/ci/junit.xml`. A test that failed
# on attempt 1 and passed on attempt 2 left NO structured trace — the #1169
# retry-flake grader reads exactly the file that was destroyed.
#
# PART A grades the reader (grade-retry-flakes.sh) against fixture evidence.
# PART B grades the writer (run-tests-with-attempt-evidence.sh) by running it.
# PART C grades the wiring, because a preserved file nothing uploads and
#        nothing reads is not evidence.
#
# Run: bash .github/scripts/tests/outer-retry-evidence.test.sh
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
GRADER="$ROOT/.github/scripts/grade-retry-flakes.sh"
WRAPPER="$ROOT/.github/scripts/run-tests-with-attempt-evidence.sh"
CI="$ROOT/.github/workflows/ci.yml"
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

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
EMPTY_ALLOWLIST="$TMP/empty-allowlist.txt"
: >"$EMPTY_ALLOWLIST"

clean_junit() { # clean_junit <path>
  mkdir -p "$(dirname "$1")"
  cat >"$1" <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="1" failures="0" errors="0">
  <testsuite name="probe" tests="1" failures="0" errors="0">
    <testcase name="always_passes" classname="probe" time="0.01" />
  </testsuite>
</testsuites>
XML
}

failing_junit() { # failing_junit <path> <test-name>
  mkdir -p "$(dirname "$1")"
  cat >"$1" <<XML
<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="1" failures="1" errors="0">
  <testsuite name="probe" tests="1" failures="1" errors="0">
    <testcase name="$2" classname="probe" time="0.01">
      <failure message="assertion failed: rows.len() == 3" type="test failure with exit code 101">thread panicked</failure>
    </testcase>
  </testsuite>
</testsuites>
XML
}

# grade <label> <evidence-dir> <want-exit> [allowlist]
grade() {
  local label="$1" dir="$2" want="$3" allow="${4:-$EMPTY_ALLOWLIST}"
  local out rc
  out=$(EVIDENCE_DIR="$dir" FLAKE_ALLOWLIST="$allow" FLAKE_GATE_TODAY="2026-08-29" \
    bash "$GRADER" 2>&1)
  rc=$?
  if [ "$rc" = "$want" ]; then
    ok "$label"
  else
    bad "$label (want exit=$want, got exit=$rc)"
    printf '%s\n' "$out" | sed 's/^/       | /'
  fi
  LAST_GRADE_OUT="$out"
}

# ── PART A — the reader ────────────────────────────────────────────────────

# THE DEFECT. Attempt 1 failed, attempt 2 passed, the step is green. The only
# surviving trace is the preserved attempt file; the grader must red the run.
A="$TMP/a/leg"
clean_junit "$A/junit.xml"
failing_junit "$A/outer-attempts/outer-attempt-1.xml" "races_under_load"
printf 'success\n' >"$A/outer-attempts/final-status.txt"
grade "attempt-1 failure retried into a green step reds the report" "$TMP/a" 1
case "${LAST_GRADE_OUT:-}" in
  *"probe::races_under_load"*) ok "the erased failure is named" ;;
  *) bad "the erased failure is named" ;;
esac

# NEGATIVE CONTROL 1 — must pass in BOTH arms. A clean suite stays green; a
# gate that reds everything is not a fix.
B="$TMP/b/leg"
clean_junit "$B/junit.xml"
grade "a clean suite is still green" "$TMP/b" 0

# NEGATIVE CONTROL 2 — must pass in BOTH arms. An ordinary failing run (the job
# is red on its own account, no preserved attempts) must NOT be re-reported by
# this gate. Widening it to every <failure> element anywhere would turn every
# red run into a confusing second complaint about retries.
C="$TMP/c/leg"
failing_junit "$C/junit.xml" "genuinely_broken"
grade "an ordinary failing run is not re-reported as a retry" "$TMP/c" 0

# The final attempt failed, so the job is already red and junit.xml still
# describes the failure. Nothing was erased; do not report it twice.
D="$TMP/d/leg"
failing_junit "$D/junit.xml" "races_under_load"
failing_junit "$D/outer-attempts/outer-attempt-1.xml" "races_under_load"
printf 'failure\n' >"$D/outer-attempts/final-status.txt"
grade "a step that failed on its final attempt is not double-reported" "$TMP/d" 0

# Fail closed: preserved evidence with no outcome label is graded. "I cannot
# tell whether this was retried into green" must not read as "it was not".
E="$TMP/e/leg"
clean_junit "$E/junit.xml"
failing_junit "$E/outer-attempts/outer-attempt-1.xml" "races_under_load"
grade "an unlabelled preserved attempt is graded, not waved through" "$TMP/e" 1

# The allowlist applies to this layer too, on the same dated/owned/justified
# terms as the nextest-level flakes.
ALLOW="$TMP/allow.txt"
printf '%s\n' "2026-12-31 probe::races_under_load gh#1177 shared-runner contention, owned" >"$ALLOW"
grade "a live allowlist entry covers an erased failure" "$TMP/a" 0 "$ALLOW"

EXPIRED="$TMP/expired.txt"
printf '%s\n' "2026-01-01 probe::races_under_load gh#1177 stale entry" >"$EXPIRED"
grade "an expired allowlist entry does not cover it" "$TMP/a" 1 "$EXPIRED"

# ── PART B — the writer ────────────────────────────────────────────────────

if [ -f "$WRAPPER" ]; then ok "wrapper script exists"; else bad "wrapper script exists"; fi

W="$TMP/w"
mkdir -p "$W"
# A stub that fails the first time and passes the second, writing a JUnit each
# time exactly as nextest does.
cat >"$W/stub.sh" <<'STUB'
#!/usr/bin/env bash
n=0
[ -f "$STUB_COUNTER" ] && n=$(cat "$STUB_COUNTER")
n=$((n + 1)); printf '%s\n' "$n" > "$STUB_COUNTER"
mkdir -p "$(dirname "$STUB_JUNIT")"
if [ "$n" = 1 ]; then
  cat > "$STUB_JUNIT" <<'X'
<testsuites name="nextest-run" tests="1" failures="1"><testsuite name="probe" tests="1" failures="1"><testcase name="races_under_load" classname="probe"><failure message="boom" type="test failure">t</failure></testcase></testsuite></testsuites>
X
  exit 100
fi
cat > "$STUB_JUNIT" <<'X'
<testsuites name="nextest-run" tests="1" failures="0"><testsuite name="probe" tests="1" failures="0"><testcase name="races_under_load" classname="probe" /></testsuite></testsuites>
X
exit 0
STUB
chmod +x "$W/stub.sh"

export STUB_COUNTER="$W/counter"
export STUB_JUNIT="$W/junit.xml"
rc1=0
JUNIT_PATH="$W/junit.xml" ATTEMPT_DIR="$W/outer-attempts" \
  bash "$WRAPPER" "$W/stub.sh" >/dev/null 2>&1 || rc1=$?
rc2=0
JUNIT_PATH="$W/junit.xml" ATTEMPT_DIR="$W/outer-attempts" \
  bash "$WRAPPER" "$W/stub.sh" >/dev/null 2>&1 || rc2=$?

if [ "$rc1" = 100 ] && [ "$rc2" = 0 ]; then
  ok "the wrapper passes the command's exit status through verbatim"
else
  bad "the wrapper passes the command's exit status through verbatim (got $rc1 then $rc2)"
fi
if [ -f "$W/outer-attempts/outer-attempt-1.xml" ]; then
  ok "the failed attempt's JUnit survives the next attempt"
else
  bad "the failed attempt's JUnit survives the next attempt"
fi
if [ -f "$W/outer-attempts/outer-attempt-2.xml" ]; then
  bad "a passing attempt is not preserved as a failure"
else
  ok "a passing attempt is not preserved as a failure"
fi
if grep -qx 'success' "$W/outer-attempts/final-status.txt" 2>/dev/null; then
  ok "final-status records the step's real outcome"
else
  bad "final-status records the step's real outcome"
fi
# End-to-end: the writer's output is exactly what the reader reds on.
grade "writer output reds the reader (end to end)" "$W" 1

# The runner-agent-crash shape: the process dies before writing any report.
# Nothing is preserved and nothing is graded — the retry absorbs it, which is
# what it exists for.
X="$TMP/x"
mkdir -p "$X"
cat >"$X/crash.sh" <<'CRASH'
#!/usr/bin/env bash
exit 143
CRASH
chmod +x "$X/crash.sh"
JUNIT_PATH="$X/junit.xml" ATTEMPT_DIR="$X/outer-attempts" \
  bash "$WRAPPER" "$X/crash.sh" >/dev/null 2>&1
if [ -z "$(find "$X/outer-attempts" -name 'outer-attempt-*.xml' 2>/dev/null)" ]; then
  ok "an attempt that wrote no JUnit preserves nothing"
else
  bad "an attempt that wrote no JUnit preserves nothing"
fi

# A stale report from the previous attempt must not be uploaded as if it
# described this one.
Y="$TMP/y"
mkdir -p "$Y"
clean_junit "$Y/junit.xml"
JUNIT_PATH="$Y/junit.xml" ATTEMPT_DIR="$Y/outer-attempts" \
  bash "$WRAPPER" "$X/crash.sh" >/dev/null 2>&1
if [ -f "$Y/junit.xml" ]; then
  bad "a stale JUnit is cleared before the attempt runs"
else
  ok "a stale JUnit is cleared before the attempt runs"
fi

# ── PART C — the wiring ────────────────────────────────────────────────────

want_grep() { # want_grep <label> <file> <fixed-pattern>
  if grep -qF -- "$3" "$2"; then ok "$1"; else bad "$1 (missing: $3)"; fi
}

want_grep "the retried containerized test run goes through the wrapper" \
  "$CI" "bash .github/scripts/run-tests-with-attempt-evidence.sh"
want_grep "the preserved attempts are uploaded with the JUnit artifact" \
  "$CI" "target/nextest/ci/outer-attempts/"

# The grader is what reads them, and it runs inside the required `report`
# check via assert-test-evidence.sh. Losing that call would make every
# preservation above ungraded.
want_grep "the retry-flake grader is still invoked from the shared evidence gate" \
  "$ROOT/.github/scripts/assert-test-evidence.sh" "grade-retry-flakes.sh"

echo ""
echo "outer-retry-evidence: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
