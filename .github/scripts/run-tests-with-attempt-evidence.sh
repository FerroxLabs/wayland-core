#!/usr/bin/env bash
# Run nextest ONCE and retain complete command output plus per-attempt evidence.
# No exit code or missing JUnit justifies retrying a suite: infrastructure retries
# belong to classified setup steps BEFORE test execution. Manual invocations keep
# distinct attempt records. Legacy failed JUnit names/final-status remain readable
# by grade-retry-flakes.sh; a later invocation cannot erase a previous failure.
# JUNIT_PATH defaults to target/nextest/ci/junit.xml.
# ATTEMPT_DIR defaults to target/nextest/ci/outer-attempts.
# EVIDENCE_SOURCE_SHA is optional (capture helper falls back to git HEAD).
set -uo pipefail
JUNIT_PATH="${JUNIT_PATH:-target/nextest/ci/junit.xml}"
ATTEMPT_DIR="${ATTEMPT_DIR:-target/nextest/ci/outer-attempts}"
HERE=$(cd "$(dirname "$0")" && pwd)
if [ "$#" -lt 1 ]; then
  echo "usage: run-tests-with-attempt-evidence.sh <command> [args...]" >&2
  exit 2
fi
if ! mkdir -p "$ATTEMPT_DIR"; then
  echo "SETUP FAILURE, no test ran: cannot create '$ATTEMPT_DIR'" >&2
  exit 2
fi
attempt=0
if [ -f "$ATTEMPT_DIR/.attempt" ]; then
  attempt=$(cat "$ATTEMPT_DIR/.attempt")
fi
case "$attempt" in '' | *[!0-9]*)
  echo "SETUP FAILURE, no test ran: invalid attempt counter" >&2
  exit 2 ;;
esac
attempt=$((attempt + 1))
# Remove the last report before launching: a failed launch cannot reuse it.
if ! rm -f "$JUNIT_PATH"; then
  echo "SETUP FAILURE, no test ran: cannot clear stale JUnit" >&2
  exit 2
fi
printf '%s\n' "$attempt" >"$ATTEMPT_DIR/.attempt" || exit 2
printf 'failure\n' >"$ATTEMPT_DIR/final-status.txt" || exit 2
CAPTURE_DIR="$ATTEMPT_DIR/attempt-$attempt" \
  bash "$HERE/capture-test-command.sh" "$@"
status=$?
if [ -f "$JUNIT_PATH" ]; then
  if [ "$status" -ne 0 ]; then
    cp "$JUNIT_PATH" "$ATTEMPT_DIR/outer-attempt-${attempt}.xml" || exit 2
  fi
elif [ "$status" -eq 0 ]; then
  echo "EVIDENCE FAILURE: command exited 0 without fresh JUnit" >&2
  status=2
fi
if [ "$status" -eq 0 ]; then
  printf 'success\n' >"$ATTEMPT_DIR/final-status.txt" || exit 2
fi
# Command exit lives in metadata.json; this receipt also captures a missing-report
# evidence failure after a zero command exit. No synthetic JUnit is generated.
printf '%s\n' "$status" >"$ATTEMPT_DIR/attempt-$attempt/runner-exit-code.txt" || exit 2
exit "$status"
