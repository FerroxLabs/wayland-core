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

# SCOPED to THIS lane's binary and THIS run's directory.
#
# These three were GLOBAL patterns, and on a host where several lanes run the
# same harness that is not a cleanup, it is a cross-lane kill: `wayland-core
# --json-stream` matches EVERY lane's binary, not this worktree's. Measured
# 2026-07-29 — two lanes' runs destroyed each other on `hetzner-dsm`, one lane
# abandoned a run rather than report contaminated numbers, and a third
# (this one) had a driver killed immediately after it wrote its results.
#
# Note the comment above: this script was ALREADY moved into a file because a
# `pkill -f` matched the launcher itself. That repaired one instance and left
# the class — the patterns stayed global. This narrows the blast radius to the
# only processes this launcher has any business reaping: its own.
#
# Under-killing is the safe failure direction. If a pattern stops matching,
# a stale process from THIS lane's previous run survives and the next bind
# fails loudly; the old behaviour's failure direction was silently destroying
# another lane's in-flight measurement, which is unrecoverable.
pkill -f "${BINARY} --json-stream" 2>/dev/null
pkill -f "f24-sink.mjs.*${RUN_DIR}" 2>/dev/null
pkill -f "f24-llm-fixture.mjs.*${RUN_DIR}" 2>/dev/null
sleep 1

: > "$LOG"
setsid nohup node "${HERE}/f24-inbound.mjs" \
  --binary "$BINARY" --run-dir "$RUN_DIR" --platform linux \
  >> "$LOG" 2>&1 < /dev/null &
echo "launched pid=$! log=${LOG}"
