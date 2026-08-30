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
#   REQUIRED_LEGS   optional. One line per leg that MUST have reported, as
#                   `<artifact-subdirectory> <job-name> <job-result>`. Blank
#                   lines and `#` comments are ignored. See the per-leg block
#                   below for why an aggregate floor cannot do this job.
#
# Exit 0 when at least EXPECTED_MIN reports exist AND they carry at least
# MIN_TESTS test cases between them AND every leg named in REQUIRED_LEGS
# contributed at least one report holding at least one test case, 1 otherwise.
#
# ── A PRESERVED ATTEMPT IS NOT COVERAGE (wayland#1216) ──────────────────────
#
# `outer-attempt-*.xml` is EXCLUDED from every count in this file. Those files
# are the JUnit of an attempt an outer retry loop preserved (wayland#1177);
# `grade-retry-flakes.sh` owns them and `grade-failing-set.sh` already skips
# them for the same reason. Counting them here inflates the number that is
# meant to prove the suite RAN with the number of times part of it FAILED, so a
# leg that reported nothing could be covered by another leg's preserved
# failures.
set -euo pipefail

: "${EVIDENCE_DIR:?EVIDENCE_DIR is required}"
: "${EXPECTED_MIN:?EXPECTED_MIN is required}"
: "${LABEL:?LABEL is required}"
UPSTREAM_RESULT="${UPSTREAM_RESULT:-unknown}"
MIN_TESTS="${MIN_TESTS:-1}"
HINT="${HINT:-}"

mkdir -p "$EVIDENCE_DIR"
# `! -name outer-attempt-*.xml` is load-bearing, not tidiness — see the header.
FOUND=$(find "$EVIDENCE_DIR" -type f -name "*.xml" ! -name "outer-attempt-*.xml" | sort)
COUNT=$(printf "%s" "$FOUND" | grep -c . || true)
# A REPORT IS NOT A TEST. `grep -c` exits 1 when no report holds a test case,
# which is exactly the state this gate exists to fail on, so `|| true` keeps
# `pipefail` from turning that into a bare script error instead of the named
# annotation below.
TESTS=$({ find "$EVIDENCE_DIR" -type f -name "*.xml" ! -name "outer-attempt-*.xml" -exec grep -oh "<testcase" {} + 2>/dev/null || true; } | grep -c . || true)

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

# ── THE FLOOR IS PER-LEG, NOT PER-RUN (wayland#1216) ───────────────────────
#
# The two gates above are AGGREGATE: every leg's artifact is downloaded into
# one directory and counted together. `EXPECTED_MIN: 1` over that aggregate is
# satisfied the moment ANY leg uploads a junit.xml, so the leg that runs the
# whole workspace suite can contribute NOTHING and the required `report` check
# still goes green — silently, because the upload uses
# `if-no-files-found: ignore`, so a leg that died before its test step simply
# creates no artifact. That is wayland#1115 one level finer: not "no suite ran"
# but "the suite did not run".
#
# Raising EXPECTED_MIN would not fix it. A count cannot say WHICH leg reported,
# and the `ci` matrix legs are conditioned per platform, so any fixed number is
# either unreachable on a lane push or satisfied by the wrong legs. The name is
# the only thing that identifies a leg, so the floor is keyed on the artifact
# subdirectory `actions/download-artifact` creates for it.
#
# A leg whose job was CANCELLED or SKIPPED is not required to have reported —
# the same rule the aggregate gate already applies to its own upstream — so
# this cannot turn a conditioned platform into a permanent red.
REQUIRED_LEGS="${REQUIRED_LEGS:-}"
LEG_FAILURES=0
while read -r leg_dir leg_job leg_result; do
  [ -n "${leg_dir:-}" ] || continue
  case "$leg_dir" in \#*) continue ;; esac
  leg_job="${leg_job:-unknown}"
  leg_result="${leg_result:-unknown}"
  if [ "$leg_result" = "cancelled" ] || [ "$leg_result" = "skipped" ]; then
    echo "required leg   : $leg_dir ($leg_job) was $leg_result — not required to report"
    continue
  fi
  # THE LEG THAT REPORTED NOTHING HAS NO DIRECTORY AT ALL. That is the
  # headline case of wayland#1216 -- `actions/download-artifact` creates a
  # subdirectory per artifact, and a leg that died before its test step
  # uploaded no artifact for one to be created from. `find` over a path that
  # is not there exits 1, and under `set -euo pipefail` that aborts the script
  # BEFORE the annotation below is written: exit 1 with no diagnostic, no leg
  # named, no reason a reader could act on. Right exit code, wrong mechanism,
  # and it would evaporate the moment anyone relaxed `set -e`. So the absent
  # directory is READ AS ZERO here and falls through to the named failure.
  leg_root="$EVIDENCE_DIR/$leg_dir"
  if [ -d "$leg_root" ]; then
    leg_found=$(find "$leg_root" -type f -name "*.xml" ! -name "outer-attempt-*.xml" | sort)
    leg_count=$(printf "%s" "$leg_found" | grep -c . || true)
    leg_tests=$({ find "$leg_root" -type f -name "*.xml" ! -name "outer-attempt-*.xml" -exec grep -oh "<testcase" {} + 2>/dev/null || true; } | grep -c . || true)
  else
    echo "required leg   : $leg_dir uploaded no artifact at all — no $leg_root exists"
    leg_count=0
    leg_tests=0
  fi
  echo "required leg   : $leg_dir ($leg_job, result $leg_result) -> $leg_count report(s), $leg_tests test case(s)"
  if [ "$leg_count" -lt 1 ] || [ "$leg_tests" -lt 1 ]; then
    echo "::error title=NO TEST SIGNAL FROM $leg_job ($LABEL)::The leg '${leg_job}' finished with result '${leg_result}' and contributed ${leg_count} JUnit report(s) holding ${leg_tests} test case(s) to ${EVIDENCE_DIR}/${leg_dir}. Another leg's upload cannot stand in for it: this leg is named here because it runs coverage no other leg runs (wayland#1216). A leg that dies before its test step uploads nothing at all, silently, because the upload is if-no-files-found: ignore. ${HINT}"
    LEG_FAILURES=$((LEG_FAILURES + 1))
  fi
done <<REQUIRED_LEGS_EOF
$REQUIRED_LEGS
REQUIRED_LEGS_EOF

if [ "$LEG_FAILURES" -ne 0 ]; then
  exit 1
fi

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
# `|| RETRY_RC=$?`, not a bare call followed by `$?`. This file runs under
# `set -e`: a bare failing call exits HERE, and the failing-set gate below
# would never run on any red retry-flake report. The self-test arm named
# "both graders report on one run" is what holds this open.
RETRY_RC=0
bash "$GRADER" || RETRY_RC=$?

# ── A COUNT OF FAILURES IS NOT A SET OF FAILURES (wayland-core#367) ────────
#
# The two gates above prove the suite RAN and that no failure was retried into
# silence. Neither can tell one failing test from another. A workspace run on
# `integ/f13` reported `1 failed`; this repository has a standing known
# failure, so `1 failed` was read as `the known 1 failed`. It was a different
# test, and what shipped was a never-merge red-arm instrument that reopened a
# process-tree leak (wayland#1156). Three more commits landed on top before
# anyone opened the name.
#
# Delegated and invoked BY PATH for the same reasons as the retry grader: one
# responsibility per file, one wiring to keep in sync, and fail-closed on its
# own absence — a gate that can be silently deleted is worth as little as one
# that cannot fail.
#
# Its exit code is combined rather than short-circuited: both gates read the
# same evidence and a reader who fixes only the first complaint should still
# see the second on the same run.
SETGRADER="$(cd "$(dirname "$0")" && pwd)/grade-failing-set.sh"
if [ ! -f "$SETGRADER" ]; then
  echo "::error title=Failing-set gate missing::${SETGRADER} is not present, so no run on this repository is comparing its failing-test SET against .config/known-failing-tests.txt (wayland-core#367). Restore it rather than removing this call."
  exit 1
fi
echo ""
SET_RC=0
bash "$SETGRADER" || SET_RC=$?

if [ "$RETRY_RC" -ne 0 ] || [ "$SET_RC" -ne 0 ]; then
  exit 1
fi
exit 0
