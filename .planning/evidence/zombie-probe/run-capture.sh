#!/usr/bin/env bash
# run-capture.sh — capture a command's TRUE exit status across a log tail.
#
# WHY THIS EXISTS (LANE-BRIEF §6b-ii): the first cross-target check in this
# lane was written as
#
#     cargo check --target $T 2>&1 | tail -25 ; echo "RC=$?"
#
# and printed `RC=0` for a check that had FAILED with three E0425 errors,
# because `$?` after a pipeline is `tail`'s status, not cargo's. That is the
# textbook self-passing gate this program keeps re-discovering, and I wrote it
# by hand inside the lane whose whole subject is instruments that cannot
# distinguish the outcomes they exist to distinguish. Repaired here rather
# than written up and carried.
#
# Usage:  run-capture.sh <logfile> <command> [args...]
# Prints: TRUE_RC=<n> on its own line, then a bounded tail of the log.

set -uo pipefail

run_capture() {
  local log="$1"; shift
  # No pipeline. The command's status is read directly, before anything else
  # can overwrite $?.
  "$@" > "$log" 2>&1
  local rc=$?
  echo "TRUE_RC=${rc}"
  echo "LOG_BYTES=$(wc -c < "$log" | tr -d ' ')"
  tail -30 "$log"
  return "$rc"
}

# ---------------------------------------------------------------------------
# Self-test: three assertions, and the third is the one that proves the repair
# does anything. Without it, the self-test passes on the BROKEN instrument too.
# ---------------------------------------------------------------------------
self_test() {
  local tmp; tmp="$(mktemp -d)"
  local failures=0 checks=0

  # 1. known-positive: a succeeding command reports 0.
  checks=$((checks + 1))
  local got
  got="$(run_capture "$tmp/a.log" sh -c 'echo hello; exit 0' | grep -c '^TRUE_RC=0$')"
  if [ "$got" = "1" ]; then
    echo "SELFTEST 1 PASS  known-positive -> TRUE_RC=0"
  else
    echo "SELFTEST 1 FAIL  known-positive did not report 0"; failures=$((failures + 1))
  fi

  # 2. known-negative: a failing command reports its REAL code, not just 1.
  checks=$((checks + 1))
  got="$(run_capture "$tmp/b.log" sh -c 'echo boom >&2; exit 7' | grep -c '^TRUE_RC=7$')"
  if [ "$got" = "1" ]; then
    echo "SELFTEST 2 PASS  known-negative -> TRUE_RC=7 (exact code, not collapsed to 1)"
  else
    echo "SELFTEST 2 FAIL  known-negative did not report 7"; failures=$((failures + 1))
  fi

  # 3. THE ONE THAT MATTERS: the old broken shape reports 0 for that same
  #    failing command. If this assertion ever fails, the repair is a no-op
  #    and assertions 1-2 were passing on a broken instrument.
  #
  #    NOTE, and this is the second instrument defect found in this lane: the
  #    first version of this assertion ran the defective idiom inside THIS
  #    script, which sets `pipefail` at the top -- so the pipeline returned 7
  #    and the assertion reported "this platform does not exhibit the defect".
  #    It does exhibit it. The reproduction has to happen in the same shell
  #    regime where the defect actually occurred: a plain non-interactive
  #    `bash -c` over ssh, which does NOT set pipefail. A self-test that
  #    reproduces a defect under settings the defect cannot survive is just a
  #    slower way of not testing.
  checks=$((checks + 1))
  local old_rc
  old_rc="$(
    bash -c 'set +o pipefail; sh -c "echo boom >&2; exit 7" 2>&1 | tail -30 > /dev/null; echo $?'
  )"
  if [ "$old_rc" = "0" ]; then
    echo "SELFTEST 3 PASS  old shape reported rc=0 for a command that exited 7 -- the repair is load-bearing"
  else
    echo "SELFTEST 3 FAIL  old shape reported rc=${old_rc}; this platform does not exhibit the defect, so the repair proves nothing here"
    failures=$((failures + 1))
  fi

  rm -rf "$tmp"
  echo "SELFTEST SUMMARY: ${checks} checks, ${failures} failed"
  [ "$failures" -eq 0 ]
}

if [ "${1:-}" = "--self-test" ]; then
  self_test
  exit $?
fi

if [ "$#" -lt 2 ]; then
  echo "usage: $0 <logfile> <command> [args...]   |   $0 --self-test" >&2
  exit 64
fi

run_capture "$@"
