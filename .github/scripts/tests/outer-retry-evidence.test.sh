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
# PART D grades the setup failure -- a wrapper that cannot create its attempt
#        directory must say so instead of looking like a failing suite -- and
#        the reserve script that stops it happening.
# PART E is the demonstration wayland#1177 c2 actually asked for: attempt 1
#        fails, attempt 2 passes, and the artifacts are then run through the
#        REAL scripts the required `report` check runs, in the layout
#        download-artifact produces. Parts A-C grade the pieces; this grades the
#        path. The earlier suite was 19/19 green on a tree where the mechanism
#        was 100 % inoperative on the runner, which is what a piecewise grade
#        buys you.
#
# Run: bash .github/scripts/tests/outer-retry-evidence.test.sh
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
GRADER="$ROOT/.github/scripts/grade-retry-flakes.sh"
WRAPPER="$ROOT/.github/scripts/run-tests-with-attempt-evidence.sh"
CI="$ROOT/.github/workflows/ci.yml"
ASSERT="$ROOT/.github/scripts/assert-test-evidence.sh"
RESERVE="$ROOT/.github/scripts/reserve-attempt-evidence-tree.sh"
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

# ── A RETIRED ENTRY THAT COMES BACK (wayland#1182) ─────────────────────────
#
# The line for `contained_construction_does_not_walk_the_workspace` was deleted
# in c461293f once the test's liveness control stopped being a wall-clock ratio,
# and merge 9c9f27b0 put it back. Nothing noticed, and `git log -S` skips merges
# so the obvious search could not see it. A resurrected exemption must red.
RESURRECTED="$TMP/resurrected.txt"
cat >"$RESURRECTED" <<'ALLOW'
# retired: probe::races_under_load gh#1182  fixed; the timing control it named is gone
2026-12-31 probe::races_under_load gh#1182 shared-runner contention, owned
ALLOW
grade "a retired allowlist entry that came back reds the run" "$TMP/a" 1 "$RESURRECTED"
case "${LAST_GRADE_OUT:-}" in
  *"Retired flake allowlist entry is back"*) ok "the resurrected entry is named as such" ;;
  *) bad "the resurrected entry is named as such" ;;
esac

# ANTI-VACUITY, both directions. Without the retirement record the SAME entry
# must still cover the SAME flake (or the arm above proves only that this
# allowlist is broken), and a retirement record for a DIFFERENT key must not
# reach in and red an unrelated entry.
UNRETIRED="$TMP/unretired.txt"
printf '%s\n' "2026-12-31 probe::races_under_load gh#1182 shared-runner contention, owned" >"$UNRETIRED"
grade "control: the same entry without a retirement record still covers it" "$TMP/a" 0 "$UNRETIRED"
OTHER="$TMP/other-retired.txt"
cat >"$OTHER" <<'ALLOW'
# retired: probe::some_other_test gh#1182  unrelated
2026-12-31 probe::races_under_load gh#1182 shared-runner contention, owned
ALLOW
grade "control: a retirement for another key leaves this entry alone" "$TMP/a" 0 "$OTHER"

# The shipped file must be in the state the fix put it in, not merely capable of
# it. This is the instance the mechanism above exists for.
SHIPPED="$ROOT/.config/flaky-allowlist.txt"
RESURRECTED_KEY="wcore-tools::workspace_policy::tests::contained_construction_does_not_walk_the_workspace"
if grep -q "^[0-9-]\{10\}[[:space:]].*$RESURRECTED_KEY" "$SHIPPED"; then
  bad "the #1182 entry is gone from the shipped allowlist"
else
  ok "the #1182 entry is gone from the shipped allowlist"
fi
if grep -q "^#[[:space:]]*retired:[[:space:]]*$RESURRECTED_KEY" "$SHIPPED"; then
  ok "the shipped allowlist records that retirement, so a merge cannot undo it silently"
else
  bad "the shipped allowlist records that retirement, so a merge cannot undo it silently"
fi
# The retirement is only true while the test's direct-observation instrument is
# there. If someone puts the wall-clock ratio back, the retirement's reason is
# false and the entry should be reconsidered rather than blocked.
if grep -q "walk_entries()" "$ROOT/crates/wcore-tools/src/workspace_policy/tests.rs"; then
  ok "the retirement's stated reason is still true (the walk is counted, not timed)"
else
  bad "the retirement's stated reason is still true (the walk is counted, not timed)"
fi

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

# ── PART D — a setup failure must not masquerade as a test failure ─────────
#
# wayland#1177 c1. On run 33227927478 the wrapper died at `mkdir -p
# "$ATTEMPT_DIR"` because target/ was root-owned by an earlier in-container
# step, and the ONLY thing the step surfaced was "Child_process exited with
# error code 2" -- byte-identical to what a genuinely failing test suite
# produces. Nothing had run. A red that names the wrong cause costs what a
# false green costs, so the two states must be distinguishable.

D="$(mktemp -d)"
touch "$D/blocker"                       # a FILE where the parent dir must be,
                                         # so mkdir fails even as root -- CI
                                         # runs this suite unprivileged, hetzner
                                         # runs it as root, and the arm has to
                                         # reproduce on both.
setup_out="$(ATTEMPT_DIR="$D/blocker/attempts" JUNIT_PATH="$D/j.xml" \
  bash "$WRAPPER" true 2>&1)"
setup_status=$?

if [ "$setup_status" -eq 2 ]; then
  ok "an unusable attempt directory exits 2 without running the command"
else
  bad "an unusable attempt directory exits 2 without running the command (got $setup_status)"
fi

case "$setup_out" in
  *"SETUP FAILURE, no test ran"*)
    ok "the setup failure says it is a setup failure, not a test failure" ;;
  *)
    bad "the setup failure says it is a setup failure, not a test failure (got: $setup_out)" ;;
esac

# The counterpart: the same wrapper, given a usable directory, must actually
# invoke the command. Without this the arm above is satisfiable by a wrapper
# that refuses everything.
run_out="$(ATTEMPT_DIR="$D/ok/attempts" JUNIT_PATH="$D/j.xml" \
  bash "$WRAPPER" echo WRAPPER-RAN-THE-COMMAND 2>&1)"
case "$run_out" in
  *WRAPPER-RAN-THE-COMMAND*)
    ok "a usable attempt directory still runs the command (anti-vacuity)" ;;
  *)
    bad "a usable attempt directory still runs the command (anti-vacuity)" ;;
esac
rm -rf "$D"

# ── THE RESERVE SCRIPT ─────────────────────────────────────────────────────
#
# The fix for the root cause. It first shipped as a bare `mkdir -p` in ci.yml
# graded by a `grep` for that string -- and the step was in the WRONG PLACE,
# after a `docker run ... cargo run` that creates target/ as root, so it failed
# with the same `Permission denied` one step earlier and the grep passed anyway.
# The ORDERING is asserted in Rust (contract_gate_topology.rs::the_outer_retry_
# evidence_tree_is_reserved_before_any_container_mounts_the_workspace); what is
# graded here is that the script survives the state that ordering exists to
# prevent.

if [ -f "$RESERVE" ]; then ok "reserve script exists"; else bad "reserve script exists"; fi

R="$(mktemp -d)"
if (cd "$R" && ATTEMPT_TREE="target/nextest/ci/outer-attempts" bash "$RESERVE" >/dev/null 2>&1) \
  && [ -d "$R/target/nextest/ci/outer-attempts" ]; then
  ok "the reserve script creates the tree on a clean workspace"
else
  bad "the reserve script creates the tree on a clean workspace"
fi

# THE MEASURED STATE. `target/` owned by another user, exactly as a root
# container step leaves it. Setting that up needs privilege, so the arm runs
# where privilege exists (CI's ubuntu runner has passwordless sudo; hetzner runs
# this suite as root) and says so out loud where it does not, rather than
# passing silently.
R2="$(mktemp -d)"
if [ "$(id -u)" -eq 0 ]; then
  # Running as root, so a root-owned target/ refuses nothing and the state this
  # arm is about cannot be built. Named rather than silently passed: CI's ubuntu
  # runner is uid 1001 with passwordless sudo, which is where it does run.
  ok "SKIPPED (this suite is running as root): the root-owned target/ recovery arm"
elif sudo -n true 2>/dev/null; then
  sudo -n mkdir -p "$R2/target/debug"
  sudo -n chown -R 0:0 "$R2/target"
  # Positive control on the SETUP: if a plain mkdir still works, the arm below
  # would pass without ever meeting the condition it is about.
  if (cd "$R2" && mkdir -p target/nextest/ci/outer-attempts 2>/dev/null); then
    bad "control: a root-owned target/ actually blocks a plain mkdir (it did not; running as root?)"
  else
    ok "control: a root-owned target/ blocks the plain mkdir this defect is"
    if (cd "$R2" && ATTEMPT_TREE="target/nextest/ci/outer-attempts" bash "$RESERVE" >/dev/null 2>&1) \
      && (cd "$R2" && : >target/nextest/ci/outer-attempts/probe 2>/dev/null); then
      ok "the reserve script recovers a root-owned target/ and leaves it writable"
    else
      bad "the reserve script recovers a root-owned target/ and leaves it writable"
    fi
  fi
  sudo -n rm -rf "$R2"
else
  ok "SKIPPED (no passwordless sudo): the root-owned target/ recovery arm"
fi
rm -rf "$R"

# Anti-vacuity for the two arms above: the script must still FAIL on a tree it
# genuinely cannot reserve, otherwise "it recovered" means only "it exits 0".
R3="$(mktemp -d)"
touch "$R3/target"
if (cd "$R3" && ATTEMPT_TREE="target/nextest/ci/outer-attempts" bash "$RESERVE" >/dev/null 2>&1); then
  bad "the reserve script fails on a tree it cannot create (anti-vacuity)"
else
  ok "the reserve script fails on a tree it cannot create (anti-vacuity)"
fi
rm -rf "$R3"

# ── PART E — the demonstration c2 asked for ────────────────────────────────
#
# "a failure on attempt 1 followed by a pass on attempt 2 is still visible to
# the required `report` check". Not a grep for the wiring: the wrapper is RUN
# twice, its outputs are assembled into the exact layout `download-artifact`
# produces from the two upload paths in ci.yml, and the REAL
# assert-test-evidence.sh -- the script the `report` step invokes -- is then run
# over it with that step's own environment.

# leg_run <dir> -- run the stub through the wrapper twice (fail, then pass) into
# a nextest-shaped tree, then lay the artifact out as download-artifact does.
leg_run() {
  local base="$1" mode="${2:-flaky}"
  mkdir -p "$base/work/target/nextest/ci"
  cat >"$base/work/stub.sh" <<STUB
#!/usr/bin/env bash
n=0
[ -f "\$STUB_COUNTER" ] && n=\$(cat "\$STUB_COUNTER")
n=\$((n + 1)); printf '%s\\n' "\$n" > "\$STUB_COUNTER"
mkdir -p "\$(dirname "\$STUB_JUNIT")"
if [ "\$n" = 1 ] && [ "$mode" = flaky ]; then
  cat > "\$STUB_JUNIT" <<'X'
<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="2" failures="1" errors="0">
  <testsuite name="probe" tests="2" failures="1" errors="0">
    <testcase name="races_under_load" classname="probe" time="0.01">
      <failure message="assertion failed" type="test failure with exit code 101">thread panicked</failure>
    </testcase>
    <testcase name="steady" classname="probe" time="0.01" />
  </testsuite>
</testsuites>
X
  exit 100
fi
cat > "\$STUB_JUNIT" <<'X'
<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="2" failures="0" errors="0">
  <testsuite name="probe" tests="2" failures="0" errors="0">
    <testcase name="races_under_load" classname="probe" time="0.01" />
    <testcase name="steady" classname="probe" time="0.01" />
  </testsuite>
</testsuites>
X
exit 0
STUB
  chmod +x "$base/work/stub.sh"
  local junit="$base/work/target/nextest/ci/junit.xml"
  local attempts="$base/work/target/nextest/ci/outer-attempts"
  local n
  for n in 1 2; do
    STUB_COUNTER="$base/work/counter" STUB_JUNIT="$junit" \
      JUNIT_PATH="$junit" ATTEMPT_DIR="$attempts" \
      bash "$WRAPPER" "$base/work/stub.sh" >/dev/null 2>&1
    [ -f "$junit" ] && [ "$(cat "$base/work/counter")" = 2 ] && break
  done
  # `download-artifact` without merge-multiple: one directory per artifact name,
  # rooted at the common parent of the uploaded paths (target/nextest/ci).
  mkdir -p "$base/junit-reports/nextest-junit-linux-containerized"
  cp -r "$junit" "$base/junit-reports/nextest-junit-linux-containerized/junit.xml"
  [ -d "$attempts" ] && cp -r "$attempts" "$base/junit-reports/nextest-junit-linux-containerized/outer-attempts"
  # A second leg, so the aggregate EXPECTED_MIN can never be what is failing.
  mkdir -p "$base/junit-reports/nextest-junit-macos-latest"
  clean_junit "$base/junit-reports/nextest-junit-macos-latest/junit.xml"
}

# report_gate <label> <evidence-root> <want-exit> [require-legs]
report_gate() {
  local label="$1" root="$2" want="$3" legs="${4-nextest-junit-linux-containerized}"
  local out rc
  out=$(cd "$root" && EVIDENCE_DIR=junit-reports EXPECTED_MIN=1 MIN_TESTS=1 \
    REQUIRE_LEGS="$legs" LABEL="ci matrix (unit + integration)" \
    UPSTREAM_RESULT=success FLAKE_ALLOWLIST="$EMPTY_ALLOWLIST" \
    FLAKE_GATE_TODAY="2026-08-29" bash "$ASSERT" 2>&1)
  rc=$?
  if [ "$rc" = "$want" ]; then
    ok "$label"
  else
    bad "$label (want exit=$want, got exit=$rc)"
    printf '%s\n' "$out" | sed 's/^/       | /'
  fi
  LAST_REPORT_OUT="$out"
}

E2E="$TMP/e2e"; mkdir -p "$E2E"; leg_run "$E2E" flaky
report_gate "THE ASK: attempt-1 failure retried green is visible to the report check" "$E2E" 1
case "${LAST_REPORT_OUT:-}" in
  *"probe::races_under_load"*) ok "the report check names the erased failure" ;;
  *) bad "the report check names the erased failure"
     printf '%s\n' "${LAST_REPORT_OUT:-}" | sed 's/^/       | /' ;;
esac

# ANTI-VACUITY. The identical pipeline with a leg that never failed must go
# green, or the arm above proves only that this gate reds everything.
CLEAN="$TMP/e2e-clean"; mkdir -p "$CLEAN"; leg_run "$CLEAN" clean
report_gate "a leg that passed first time is still green end to end" "$CLEAN" 0

# D34 / wayland#1177 c2: the leg that runs the whole workspace suite uploads
# NOTHING (its wrapper died before nextest, `if-no-files-found: ignore` makes it
# silent) while another leg uploads a clean report. The aggregate floor is
# satisfied; the run must still be red.
MISSING="$TMP/e2e-missing"; mkdir -p "$MISSING/junit-reports/nextest-junit-macos-latest"
clean_junit "$MISSING/junit-reports/nextest-junit-macos-latest/junit.xml"
report_gate "a leg that contributed nothing reds the report even when others did" "$MISSING" 1
case "${LAST_REPORT_OUT:-}" in
  *"nextest-junit-linux-containerized"*) ok "the missing leg is named" ;;
  *) bad "the missing leg is named" ;;
esac
# Its own anti-vacuity: with no leg required, the same tree is green -- so the
# arm above measures the per-leg floor and not some unrelated failure.
report_gate "control: the same tree passes when no leg is required" "$MISSING" 0 ""

# A preserved FAILED attempt is not coverage. A leg whose only file is an
# outer-attempt-*.xml has certified nothing, and the `*.xml` glob used to count
# it as a report holding test cases.
PRESERVED="$TMP/e2e-preserved"
mkdir -p "$PRESERVED/junit-reports/nextest-junit-linux-containerized/outer-attempts"
failing_junit "$PRESERVED/junit-reports/nextest-junit-linux-containerized/outer-attempts/outer-attempt-1.xml" "races_under_load"
# `failure`, deliberately: with `success` the retry-flake grader reds this on
# its own account and the arm would pass without ever exercising the counting
# change. A final-attempt failure is the one state that grader waves through, so
# the only thing left that can red it is the coverage count.
printf 'failure\n' >"$PRESERVED/junit-reports/nextest-junit-linux-containerized/outer-attempts/final-status.txt"
mkdir -p "$PRESERVED/junit-reports/nextest-junit-macos-latest"
clean_junit "$PRESERVED/junit-reports/nextest-junit-macos-latest/junit.xml"
report_gate "a preserved failed attempt does not count as the leg's coverage" "$PRESERVED" 1

echo ""
echo "outer-retry-evidence: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
