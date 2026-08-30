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

# ── RETRY-FLAKE GATE (wayland#1169) ─────────────────────────────────────────
#
# The gate above proves the suite RAN. These prove a test that ran, FAILED, and
# was retried into a pass cannot report as silence.
#
# THE REPRODUCTION, measured on this tree: `wcore-tools::
# inv2_round5_adversarial_test a_save_during_an_edit_is_not_lost` failed 14 of
# 48 runs (29 %) at `--retries 0` on the integration branch, and the SAME defect
# reported as `FLAKY 2/3` inside a PASSING run under the `ci` profile. It sits on
# the #1155 data-loss race. A 29 % data-loss race was invisible to CI, and at the
# 6.5 % rate #1169 measured for the Edit/Write arms the visible-report frequency
# is 0.065^3, about one run in 3,600.
#
# Every "must fail" case below is paired with a "must pass" case over the same
# code, because a gate that always failed would satisfy the first half alone —
# and a permanently-red gate is worth as little as a permanently-green one.
#
# `FLAKE_GATE_TODAY` is injected everywhere rather than letting the gate read the
# clock: a test whose verdict changes with the calendar is a test that will one
# day be wrong with nothing having changed.

GRADER="$HERE/../grade-retry-flakes.sh"

flake_case() {
  # flake_case <name> <expected_exit> <evidence_dir> <allowlist> <today>
  local name=$1 want=$2 dir=$3 allow=$4 today=$5
  local out rc
  out=$(EVIDENCE_DIR="$dir" FLAKE_ALLOWLIST="$allow" FLAKE_GATE_TODAY="$today" \
        bash "$GRADER" 2>&1)
  rc=$?
  if [ "$rc" -eq "$want" ]; then
    PASS=$((PASS + 1)); printf "ok   %-58s exit=%s\n" "$name" "$rc"
  else
    FAIL=$((FAIL + 1)); printf "FAIL %-58s exit=%s want=%s\n" "$name" "$rc" "$want"
    printf "%s\n" "$out" | sed "s/^/       | /"
  fi
}

# nextest's REAL shape for a test that failed once and passed on the retry,
# taken from a run reproduced on hetzner-dsm under `retries = 2`. The
# `failures="0"` on BOTH elements is the entire defect in one attribute: the job
# conclusion, dorny/test-reporter and a human skimming all read zero, and the
# only record of the failed attempt is the `<flakyFailure>` child that nothing
# was reading.
FLAKY='<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="2" failures="0" errors="0">
    <testsuite name="wcore-tools" tests="2" disabled="0" errors="0" failures="0">
        <testcase name="clean_test" classname="wcore-tools::inv2_round5_adversarial_test" time="0.005">
        </testcase>
        <testcase name="a_save_during_an_edit_is_not_lost" classname="wcore-tools::inv2_round5_adversarial_test" time="0.004">
            <flakyFailure message="assertion failed: saved content survived" type="test failure with exit code 101">panicked at edit.rs
                <system-out>running 1 test</system-out>
            </flakyFailure>
        </testcase>
    </testsuite>
</testsuites>'

# The identical suite with no retry in it. Same files, same count, same gate.
CLEAN='<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="2" failures="0" errors="0">
    <testsuite name="wcore-tools" tests="2" disabled="0" errors="0" failures="0">
        <testcase name="clean_test" classname="wcore-tools::inv2_round5_adversarial_test" time="0.005">
        </testcase>
        <testcase name="a_save_during_an_edit_is_not_lost" classname="wcore-tools::inv2_round5_adversarial_test" time="0.004">
        </testcase>
    </testsuite>
</testsuites>'

# Attributes in the other order, and a SELF-CLOSING testcase immediately before
# the flaky one. Both are parser traps: `name="..."` also matches inside
# `classname="..."`, so an unanchored matcher keys the test by its binary id
# twice; and a self-closing element leaves no `</testcase>`, so state from it
# would be inherited by whatever came next and mis-attribute the flake.
ODD='<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="2" failures="0" errors="0">
    <testsuite name="wcore-swarm" tests="2" failures="0">
        <testcase classname="wcore-swarm::dispatch_smoke" name="self_closed_and_clean" time="0.01"/>
        <testcase classname="wcore-swarm::worker_runtime_limits" name="multi_worker_output_exhaustion_fails_without_retaining_buffers" time="0.02">
            <flakyFailure message="timed out" type="test failure">boom</flakyFailure>
            <flakyFailure message="timed out" type="test failure">boom again</flakyFailure>
        </testcase>
    </testsuite>
</testsuites>'

ALLOW_LIVE="$TMP/allow-live.txt"
printf '%s\n' \
  '# comment line, must be ignored' \
  '' \
  '2026-12-31  wcore-tools::inv2_round5_adversarial_test::a_save_during_an_edit_is_not_lost  gh#1155  TOCTOU in the guarded write path, fix in flight.' \
  > "$ALLOW_LIVE"

ALLOW_EXPIRED="$TMP/allow-expired.txt"
printf '%s\n' \
  '2026-01-01  wcore-tools::inv2_round5_adversarial_test::a_save_during_an_edit_is_not_lost  gh#1155  Same entry, past its date.' \
  > "$ALLOW_EXPIRED"

ALLOW_UNOWNED="$TMP/allow-unowned.txt"
printf '%s\n' \
  '2026-12-31  wcore-tools::inv2_round5_adversarial_test::a_save_during_an_edit_is_not_lost  because-i-said-so  no issue number' \
  > "$ALLOW_UNOWNED"

ALLOW_UNJUSTIFIED="$TMP/allow-unjustified.txt"
printf '%s\n' \
  '2026-12-31  wcore-tools::inv2_round5_adversarial_test::a_save_during_an_edit_is_not_lost  gh#1155' \
  > "$ALLOW_UNJUSTIFIED"

ALLOW_BADDATE="$TMP/allow-baddate.txt"
printf '%s\n' \
  'soon  wcore-tools::inv2_round5_adversarial_test::a_save_during_an_edit_is_not_lost  gh#1155  no expiry date' \
  > "$ALLOW_BADDATE"

ALLOW_EMPTY="$TMP/allow-empty.txt"
: > "$ALLOW_EMPTY"

mkdir -p "$TMP/flaky/leg-a" "$TMP/clean/leg-a" "$TMP/odd/leg-a"
printf "%s\n" "$FLAKY" > "$TMP/flaky/leg-a/junit.xml"
printf "%s\n" "$CLEAN" > "$TMP/clean/leg-a/junit.xml"
printf "%s\n" "$ODD"   > "$TMP/odd/leg-a/junit.xml"

# F1. THE REPRODUCTION: a retried failure with nobody owning it.
flake_case "unlisted retried failure -> RED" 1 "$TMP/flaky" "$ALLOW_EMPTY" 2026-08-28
# F2. Paired control over the SAME evidence: an owned, dated, justified entry.
flake_case "same flake, live allowlist entry -> GREEN" 0 "$TMP/flaky" "$ALLOW_LIVE" 2026-08-28
# F3. Paired control over the SAME allowlist: a suite with no retry in it.
#     Proves F1 fails on the flake and not merely on being a stricter script.
flake_case "no flake at all -> GREEN" 0 "$TMP/clean" "$ALLOW_EMPTY" 2026-08-28
# F4. THE SHRINK MECHANISM. Identical entry to F2, one day past its date.
flake_case "allowlisted but expired -> RED" 1 "$TMP/flaky" "$ALLOW_EXPIRED" 2026-08-28
# F5. ...and the same entry read from BEFORE its expiry still passes, so F4
#     fails on the date and not on the file.
flake_case "the expired entry, read before its date -> GREEN" 0 "$TMP/flaky" "$ALLOW_EXPIRED" 2025-12-31
# F6. An exemption nobody owns is a retry nobody will ever remove.
flake_case "entry with no owning issue -> RED" 1 "$TMP/flaky" "$ALLOW_UNOWNED" 2026-08-28
# F7. The justification is the point of the file.
flake_case "entry with no stated reason -> RED" 1 "$TMP/flaky" "$ALLOW_UNJUSTIFIED" 2026-08-28
# F8. A malformed line must not be silently skipped: skipping fails GREEN,
#     which is the class of defect this whole gate exists to close.
flake_case "entry with no expiry date -> RED" 1 "$TMP/flaky" "$ALLOW_BADDATE" 2026-08-28
# F9. A deleted allowlist must fail closed, never open.
flake_case "allowlist file missing entirely -> RED" 1 "$TMP/flaky" "$TMP/nope.txt" 2026-08-28
# F10. ...and an absent allowlist over a clean suite is still green, so F9
#      fails on the flake rather than on the missing file.
flake_case "missing allowlist, clean suite -> GREEN" 0 "$TMP/clean" "$TMP/nope.txt" 2026-08-28
# F11. The two parser traps: swapped attribute order and a self-closing
#      neighbour. A mis-key here would report the WRONG test name, which is
#      worse than reporting none — it sends triage at an innocent test.
flake_case "swapped attrs + self-closing neighbour -> RED" 1 "$TMP/odd" "$ALLOW_EMPTY" 2026-08-28
# F12. No evidence at all is exit 0 here: that is assert-test-evidence.sh's
#      gate, and reporting one defect as two makes both harder to read.
flake_case "no evidence directory -> GREEN (not this gate's job)" 0 "$TMP/absent-dir" "$ALLOW_EMPTY" 2026-08-28

# F13. ATTRIBUTION. A count with no name attached is the state of affairs
#      already — in a log instead of a file. The annotation must name the test.
flake_annotation=$(EVIDENCE_DIR="$TMP/flaky" FLAKE_ALLOWLIST="$ALLOW_EMPTY" \
  FLAKE_GATE_TODAY=2026-08-28 bash "$GRADER" 2>&1)
if printf "%s" "$flake_annotation" | \
   grep -q "::error title=Retried failure (wayland#1169)::wcore-tools::inv2_round5_adversarial_test::a_save_during_an_edit_is_not_lost FAILED 1 time"; then
  PASS=$((PASS + 1)); printf "ok   %-58s\n" "the ::error names the test and the attempt count"
else
  FAIL=$((FAIL + 1)); printf "FAIL %-58s\n" "the ::error names the test and the attempt count"
  printf "%s\n" "$flake_annotation" | sed "s/^/       | /"
fi

# F14. Two failed attempts on one test must be reported as two, not one. The
#      rate is the whole argument in #1169; a gate that flattened it to a
#      boolean would have nothing to say about severity.
odd_annotation=$(EVIDENCE_DIR="$TMP/odd" FLAKE_ALLOWLIST="$ALLOW_EMPTY" \
  FLAKE_GATE_TODAY=2026-08-28 bash "$GRADER" 2>&1)
if printf "%s" "$odd_annotation" | grep -q "multi_worker_output_exhaustion_fails_without_retaining_buffers FAILED 2 time"; then
  PASS=$((PASS + 1)); printf "ok   %-58s\n" "attempt counts are summed, not flattened to a flag"
else
  FAIL=$((FAIL + 1)); printf "FAIL %-58s\n" "attempt counts are summed, not flattened to a flag"
  printf "%s\n" "$odd_annotation" | sed "s/^/       | /"
fi

# F15. An allowlisted flake must still be SAID OUT LOUD. Silence is what #1169
#      is about; the allowlist buys a green run, not a quiet one.
allowed_annotation=$(EVIDENCE_DIR="$TMP/flaky" FLAKE_ALLOWLIST="$ALLOW_LIVE" \
  FLAKE_GATE_TODAY=2026-08-28 bash "$GRADER" 2>&1)
if printf "%s" "$allowed_annotation" | grep -q "::warning title=Known-flaky test retried::"; then
  PASS=$((PASS + 1)); printf "ok   %-58s\n" "an allowlisted flake still emits a ::warning"
else
  FAIL=$((FAIL + 1)); printf "FAIL %-58s\n" "an allowlisted flake still emits a ::warning"
fi

# F16. THE WIRING, exercised rather than asserted: the same flake reaching the
#      gate through assert-test-evidence.sh, which is what the report job
#      actually invokes. A grader that works standalone and is not reached is a
#      gate that cannot fail.
out=$(EVIDENCE_DIR="$TMP/flaky" EXPECTED_MIN=1 LABEL="ci matrix" MIN_TESTS=1 \
      UPSTREAM_RESULT=success FLAKE_ALLOWLIST="$ALLOW_EMPTY" \
      FLAKE_GATE_TODAY=2026-08-28 bash "$SCRIPT" 2>&1)
rc=$?
if [ "$rc" -eq 1 ] && printf "%s" "$out" | grep -q "Retry-flake gate FAILED"; then
  PASS=$((PASS + 1)); printf "ok   %-58s exit=%s\n" "flake reaches the gate through the wired entry point" "$rc"
else
  FAIL=$((FAIL + 1)); printf "FAIL %-58s exit=%s want=1\n" "flake reaches the gate through the wired entry point" "$rc"
  printf "%s\n" "$out" | sed "s/^/       | /"
fi

# F17. ...and the same entry point stays GREEN on a clean suite, so F16 is the
#      flake and not the delegation merely breaking the script.
out=$(EVIDENCE_DIR="$TMP/clean" EXPECTED_MIN=1 LABEL="ci matrix" MIN_TESTS=1 \
      UPSTREAM_RESULT=success FLAKE_ALLOWLIST="$ALLOW_EMPTY" \
      FLAKE_GATE_TODAY=2026-08-28 bash "$SCRIPT" 2>&1)
rc=$?
if [ "$rc" -eq 0 ]; then
  PASS=$((PASS + 1)); printf "ok   %-58s exit=%s\n" "clean suite through the wired entry point -> GREEN" "$rc"
else
  FAIL=$((FAIL + 1)); printf "FAIL %-58s exit=%s want=0\n" "clean suite through the wired entry point -> GREEN" "$rc"
  printf "%s\n" "$out" | sed "s/^/       | /"
fi

# F18. A GATE THAT CAN BE SILENTLY DELETED IS WORTH AS LITTLE AS ONE THAT
#      CANNOT FAIL. Deleting grade-retry-flakes.sh must red the caller, not
#      quietly restore the old behaviour. Exercised on a COPY so the real
#      script is never removed from the tree mid-run.
mkdir -p "$TMP/lonely"
cp "$SCRIPT" "$TMP/lonely/assert-test-evidence.sh"
out=$(EVIDENCE_DIR="$TMP/clean" EXPECTED_MIN=1 LABEL="ci matrix" MIN_TESTS=1 \
      UPSTREAM_RESULT=success bash "$TMP/lonely/assert-test-evidence.sh" 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && printf "%s" "$out" | grep -q "Retry-flake gate missing"; then
  PASS=$((PASS + 1)); printf "ok   %-58s exit=%s\n" "deleting the grader reds its caller" "$rc"
else
  FAIL=$((FAIL + 1)); printf "FAIL %-58s exit=%s want!=0\n" "deleting the grader reds its caller" "$rc"
  printf "%s\n" "$out" | sed "s/^/       | /"
fi

# F19. THE REAL ALLOWLIST, parsed by the real gate. A committed file with a
#      typo'd date or an entry that has quietly expired would fail `report` on
#      main with nothing having changed, so it is validated here where it is
#      cheap. Graded against a fixed date so this case cannot rot into a
#      calendar-triggered failure of its own; the entries' actual expiry is
#      enforced against the true date by the gate itself, in CI.
out=$(EVIDENCE_DIR="$TMP/clean" FLAKE_GATE_TODAY=2026-08-28 bash "$GRADER" 2>&1)
rc=$?
if [ "$rc" -eq 0 ]; then
  PASS=$((PASS + 1)); printf "ok   %-58s exit=%s\n" "the committed allowlist parses and is unexpired" "$rc"
else
  FAIL=$((FAIL + 1)); printf "FAIL %-58s exit=%s want=0\n" "the committed allowlist parses and is unexpired" "$rc"
  printf "%s\n" "$out" | sed "s/^/       | /"
fi


# ── wayland#1216 — THE FLOOR IS PER-LEG, AND A PRESERVED ATTEMPT IS NOT ──────
#    COVERAGE. Both directions on every case: the aggregate gate could not see
#    that the leg running the whole workspace suite contributed nothing,
#    because ANY leg's upload satisfied `EXPECTED_MIN: 1`.

leg_case() {
  # leg_case <name> <expected_exit> <reports_dir> <required_legs>
  local name=$1 want=$2 dir=$3 legs=$4
  local out rc
  out=$(EVIDENCE_DIR="$dir" EXPECTED_MIN=1 LABEL="ci matrix" MIN_TESTS=1 \
        UPSTREAM_RESULT=success REQUIRED_LEGS="$legs" \
        FLAKE_GATE_TODAY=2026-08-28 bash "$SCRIPT" 2>&1)
  rc=$?
  if [ "$rc" -eq "$want" ]; then
    PASS=$((PASS + 1)); printf "ok   %-58s exit=%s\n" "$name" "$rc"
  else
    FAIL=$((FAIL + 1)); printf "FAIL %-58s exit=%s want=%s\n" "$name" "$rc" "$want"
    printf "%s\n" "$out" | sed "s/^/       | /"
  fi
}

# THE REPRODUCTION. Two legs are in scope; the one that runs the whole
# workspace suite uploaded nothing, and the OTHER one uploaded a real report.
# The aggregate floor is satisfied — this is the run wayland#1216 describes.
mkdir -p "$TMP/legs/nextest-junit-macos-latest"
printf "%s\n" "$REAL" > "$TMP/legs/nextest-junit-macos-latest/junit.xml"
leg_case "aggregate floor alone: the silent leg is invisible -> GREEN" 0 \
  "$TMP/legs" ""
leg_case "named leg contributed nothing -> RED" 1 \
  "$TMP/legs" "nextest-junit-linux-containerized ci-linux success"

# L1b. THE EXIT CODE IS NOT THE PROPERTY. The arm above is satisfied by a red
# for ANY reason, and the headline case of wayland#1216 -- the leg uploaded no
# artifact, so `download-artifact` created no subdirectory for it -- used to
# red through `set -e` aborting on `find` over a missing path, BEFORE the
# annotation was written. Same exit code, no leg named, nothing a reader could
# act on, and it would have evaporated the moment anyone relaxed `set -e`. So
# the annotation itself is asserted, on the arm where the directory is ABSENT
# rather than merely empty.
leg_says() {
  # leg_says <name> <expected_exit> <reports_dir> <required_legs> <needle>
  local name=$1 want=$2 dir=$3 legs=$4 needle=$5
  local out rc
  out=$(EVIDENCE_DIR="$dir" EXPECTED_MIN=1 LABEL="ci matrix" MIN_TESTS=1 \
        UPSTREAM_RESULT=success REQUIRED_LEGS="$legs" \
        FLAKE_GATE_TODAY=2026-08-28 bash "$SCRIPT" 2>&1)
  rc=$?
  if [ "$rc" -eq "$want" ] && printf "%s" "$out" | grep -q -- "$needle"; then
    PASS=$((PASS + 1)); printf "ok   %-58s exit=%s\n" "$name" "$rc"
  else
    FAIL=$((FAIL + 1)); printf "FAIL %-58s exit=%s want=%s needle=%s\n" \
      "$name" "$rc" "$want" "$needle"
    printf "%s\n" "$out" | sed "s/^/       | /"
  fi
}
[ ! -d "$TMP/legs/nextest-junit-linux-containerized" ] || {
  echo "FAIL fixture: the absent-directory arm needs the leg dir ABSENT"; FAIL=$((FAIL + 1)); }
leg_says "absent leg dir NAMES the leg, not a bare set -e abort" 1 \
  "$TMP/legs" "nextest-junit-linux-containerized ci-linux success" \
  "NO TEST SIGNAL FROM ci-linux"
# ...and its control: the SAME needle on the arm where the directory exists but
# is empty, so a needle that could never match anything is not what passed L1b.
mkdir -p "$TMP/legs/empty-control"
leg_says "control: present-but-empty leg dir names the leg too" 1 \
  "$TMP/legs" "empty-control ci-linux success" \
  "NO TEST SIGNAL FROM ci-linux"
# ...and the negative control: a leg that DID report must not carry the needle,
# or the assertion above would pass on a script that annotates unconditionally.
leg_says "control: a reporting leg does not emit that annotation" 0 \
  "$TMP/legs" "nextest-junit-macos-latest ci success" \
  "required leg   : nextest-junit-macos-latest"
rmdir "$TMP/legs/empty-control"

# L2. The control for L1: the same run with the leg's report present must pass,
#     so L1 is the missing leg and not the mechanism being permanently red.
mkdir -p "$TMP/legs/nextest-junit-linux-containerized"
printf "%s\n" "$REAL" > "$TMP/legs/nextest-junit-linux-containerized/junit.xml"
leg_case "named leg reported -> GREEN" 0 \
  "$TMP/legs" "nextest-junit-linux-containerized ci-linux success"

# L3. A CANCELLED or SKIPPED leg is not required to have reported. Without
#     this the gate would be permanently red on every lane push that skips a
#     conditioned platform, which is a gate someone switches off.
rm -rf "$TMP/legs/nextest-junit-linux-containerized"
leg_case "skipped leg is not required to report -> GREEN" 0 \
  "$TMP/legs" "nextest-junit-linux-containerized ci-linux skipped"
leg_case "cancelled leg is not required to report -> GREEN" 0 \
  "$TMP/legs" "nextest-junit-linux-containerized ci-linux cancelled"
# ...and the same leg with any other result is still required, so L3 is the
# result and not the leg name being ignored.
leg_case "failed leg that reported nothing is still RED" 1 \
  "$TMP/legs" "nextest-junit-linux-containerized ci-linux failure"

# L4. A leg whose directory holds nextest's zero-match junit.xml has reported a
#     FILE and no coverage. Same rule as the aggregate MIN_TESTS gate, applied
#     per leg — otherwise the per-leg floor is the file-count gate again.
mkdir -p "$TMP/legs/nextest-junit-linux-containerized"
printf "%s\n" "$EMPTY" > "$TMP/legs/nextest-junit-linux-containerized/junit.xml"
leg_case "named leg reported a zero-test junit -> RED" 1 \
  "$TMP/legs" "nextest-junit-linux-containerized ci-linux success"

# L5. PRESERVED ATTEMPTS ARE NOT COVERAGE. `outer-attempt-*.xml` is the JUnit
#     of an attempt an outer retry loop preserved (wayland#1177). Counting it
#     would let a leg's FAILURES stand in for the coverage they are supposed to
#     prove — and here it is the leg's ONLY file.
rm -f "$TMP/legs/nextest-junit-linux-containerized/junit.xml"
mkdir -p "$TMP/legs/nextest-junit-linux-containerized/outer-attempts"
printf "%s\n" "$REAL" > "$TMP/legs/nextest-junit-linux-containerized/outer-attempts/outer-attempt-1.xml"
leg_case "a leg whose only file is a preserved attempt -> RED" 1 \
  "$TMP/legs" "nextest-junit-linux-containerized ci-linux success"
# ...and the control: add the real report back beside it and the leg passes.
printf "%s\n" "$REAL" > "$TMP/legs/nextest-junit-linux-containerized/junit.xml"
leg_case "preserved attempt beside a real report -> GREEN" 0 \
  "$TMP/legs" "nextest-junit-linux-containerized ci-linux success"

# L6. The same exclusion on the AGGREGATE counters. A whole run whose only XML
#     is a preserved attempt certifies nothing, and used to satisfy
#     EXPECTED_MIN: 1 + MIN_TESTS: 1 on its own.
mkdir -p "$TMP/attempts-only/nextest-junit-linux-containerized/outer-attempts"
printf "%s\n" "$REAL" > "$TMP/attempts-only/nextest-junit-linux-containerized/outer-attempts/outer-attempt-1.xml"
run_case "only preserved attempts, no report -> RED" 1 "$TMP/attempts-only" 1 success
# ...control: the identical file NOT named outer-attempt-* is real evidence.
mkdir -p "$TMP/attempts-control/leg"
printf "%s\n" "$REAL" > "$TMP/attempts-control/leg/junit.xml"
run_case "the same XML under a normal name -> GREEN" 0 "$TMP/attempts-control" 1 success

# L6b. THE COUNT EXCLUSION, GRADED ON ITS OWN. L6 above reds through MIN_TESTS,
# not through EXPECTED_MIN: at MIN_TESTS=1 a directory holding nothing but
# preserved attempts fails the test-case floor whether or not the FILE count
# excludes them, so deleting `! -name outer-attempt-*.xml` from the `FOUND=`
# line leaves L6 green. Measured, not assumed. e2e.yml computes EXPECTED_MIN
# from its scope and routinely asks for more than one report, so the file count
# is a live floor there — this arm holds it at 2 with one real report beside one
# preserved attempt, which is exactly "a leg's failures inflating the number
# meant to prove its coverage" (wayland#1216 c2).
mkdir -p "$TMP/count-excl/leg/outer-attempts"
printf "%s\n" "$REAL" > "$TMP/count-excl/leg/junit.xml"
printf "%s\n" "$REAL" > "$TMP/count-excl/leg/outer-attempts/outer-attempt-1.xml"
run_case "a preserved attempt does not count toward EXPECTED_MIN" 1 "$TMP/count-excl" 2 success
# ...control: replace the preserved attempt with a second REAL report and the
# same EXPECTED_MIN=2 is met, so L6b's red is the exclusion and not the floor
# being unreachable.
mkdir -p "$TMP/count-ctrl/leg"
printf "%s\n" "$REAL" > "$TMP/count-ctrl/leg/junit.xml"
printf "%s\n" "$REAL" > "$TMP/count-ctrl/leg/junit-two.xml"
run_case "two real reports meet the same EXPECTED_MIN=2" 0 "$TMP/count-ctrl" 2 success

# L7. Blank lines and comments in REQUIRED_LEGS are ignored rather than read as
#     a leg named "" that can never report — a parser that failed here would
#     make the gate permanently red on a formatting change.
leg_case "blank lines and comments in REQUIRED_LEGS -> GREEN" 0 \
  "$TMP/legs" "
# a comment
nextest-junit-linux-containerized ci-linux success
"

echo "---"
echo "passed: $PASS  failed: $FAIL"
[ "$FAIL" -eq 0 ]
