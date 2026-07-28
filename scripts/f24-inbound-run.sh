#!/usr/bin/env bash
# Launch the inbound matrix detached, from a FILE rather than an inline ssh
# command string.
#
# Why a file: the inline form kept killing its own launcher. `pkill -f
# f24-sink` matches on the full command line, and an ssh command string that
# MENTIONS the script is itself a match — so the cleanup step reaped the very
# shell that was about to start the run, leaving no log and no process. The
# pattern is `pkill -f` against a name that appears in the launcher; the fix is
# to put the launcher in a file so the pattern cannot see itself.
set -uo pipefail

BINARY="${1:?usage: f24-inbound-run.sh <binary> <run-dir> <log>}"
RUN_DIR="${2:?run-dir}"
LOG="${3:?log}"
HERE="$(cd "$(dirname "$0")" && pwd)"

pkill -f 'wayland-core --json-stream' 2>/dev/null
pkill -f 'scripts/f24-sink.mjs' 2>/dev/null
pkill -f 'scripts/f24-llm-fixture.mjs' 2>/dev/null
sleep 1

: > "$LOG"
setsid nohup node "${HERE}/f24-inbound.mjs" \
  --binary "$BINARY" --run-dir "$RUN_DIR" --platform linux \
  >> "$LOG" 2>&1 < /dev/null &
echo "launched pid=$! log=${LOG}"
