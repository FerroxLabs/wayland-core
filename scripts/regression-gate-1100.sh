#!/usr/bin/env bash
#
# Regression gate for FerroxLabs/wayland#1100 — the ONLY instrument in CI that
# can observe this defect class.
#
# Everywhere else CI runs `cargo nextest run` (`just test-ci`), which gives every
# test its OWN process. A test whose precondition is a cold process-global cache
# is therefore green under nextest no matter what a sibling does, and reverting
# the fix would leave every other check green. `cargo test` runs the whole lib
# suite in ONE process, and `--test-threads=1` makes the ordering deterministic,
# so the cache-warming sibling — `a_failed_probe_records_a_cause_the_operator_
# can_actually_read`, which sorts EARLIER by name — is guaranteed to run first
# and warm the probe cache before `session_selection_reaches_ready_without_
# running_the_appcontainer_probe` reads it. That is exactly the reported defect.
#
# Windows-only: the probe cache lives in the `#[cfg(windows)]` AppContainer
# backend, so on any other platform neither test is compiled in and this gate
# would certify nothing. Callers gate on `runner.os == 'Windows'`.
#
# Usage: scripts/regression-gate-1100.sh [logfile]
set -uo pipefail

# Under `target/` on purpose: the self-hosted Windows runner does NOT reimage
# between runs, so a log dropped at the repo root would linger as an untracked
# file in a workspace that later runs care about.
LOG="${1:-target/sandbox-lib-one-process.log}"
mkdir -p "$(dirname "$LOG")"

# The two tests whose ORDERING is the defect. Naming them here is what stops
# this gate degrading into a vacuous `cargo test` that exits 0 having compiled
# an empty suite: if either one is not observed executing, the run proved
# nothing about the ordering and that is a hard failure, not a pass.
COLD="session_selection_reaches_ready_without_running_the_appcontainer_probe"
WARM="a_failed_probe_records_a_cause_the_operator_can_actually_read"

fail() {
  echo "::error title=#1100 regression gate::$*"
  echo "FAIL: $*"
  exit 1
}

# vacuity-checked: rc is captured on its own line (never through a pipe), the
# `test result:` roll-up is parsed and its passed count required to be > 0, and
# BOTH tests of the ordering pair above are asserted present by name in the
# per-test output. `cargo test` on a suite that compiled EMPTY prints
# `test result: ok. 0 passed` and exits 0; here that is a FAIL.
vx cargo test -p wcore-sandbox --lib -- --test-threads=1 >"$LOG" 2>&1
rc=$?
cat "$LOG"

# 1. Did the harness run at all? A compile error produces no roll-up line, and
#    must not be reported as "the tests are missing".
grep -q "test result:" "$LOG" \
  || fail "the lib test harness never produced a \`test result:\` line (compile failure or harness error) — see $LOG"

# 2. Was each half of the ordering pair actually executed IN THIS PROCESS?
#    No \$ anchor: cargo writes CRLF on Windows and Git bash grep would miss it.
for t in "$COLD" "$WARM"; do
  grep -q "::${t} \.\.\." "$LOG" \
    || fail "\`$t\` did not execute — the pair this gate exists for was not observed, so a green here certifies nothing (cfg drift, a stale filter, or the test was renamed/removed)"
done

# 3. Non-zero executed count. Belt to the braces of (2): the roll-up is what a
#    reader quotes, so assert on it directly rather than inferring it.
passed=$(grep -oE "test result: [a-zA-Z]+\. [0-9]+ passed" "$LOG" | grep -oE "[0-9]+" | head -1)
[ -n "${passed:-}" ] || fail "could not parse a passed count out of the \`test result:\` line — see $LOG"
[ "$passed" -gt 0 ] || fail "the lib suite reported $passed passed. A suite that executed nothing certifies nothing."

# 4. Only now does exit status mean anything.
[ "$rc" -eq 0 ] \
  || fail "\`cargo test -p wcore-sandbox --lib -- --test-threads=1\` exited $rc. This is the #1100 class: a test whose precondition is a process-global cache, inherited from the scheduler instead of established by the test."

echo "#1100 regression gate PASSED — $passed test(s) executed in ONE process, single-threaded; both \`$WARM\` and \`$COLD\` observed."
