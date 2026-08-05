#!/usr/bin/env bash
# 23B-H1 reproduction under concurrent CPU load.
#
# 23B-01 measured the defect at 8/8 and 9/10 under concurrent compile load
# (15-minute load average 28) and 0/3 on a quiet host, so a quiet-host run
# that does not reproduce proves nothing about the defect. This wrapper
# recreates the load condition WITHOUT competing for disk (the build host ran
# out of space during 23B-01) and WITHOUT leaving orphaned processes: every
# load generator is wrapped in `timeout`, so it self-terminates even if the
# controlling ssh connection drops.
#
#   --binary <path>    the wayland-core binary to drive
#   --runs <n>         seed+resume cycles
#   --out <dir>        where failing journals are preserved
#   --load <n>         number of CPU load generators (default 64)
#   --seconds <n>      hard lifetime of each load generator (default 900)

set -uo pipefail

BINARY=""
RUNS=12
OUT=""
LOAD=64
SECONDS_CAP=900

while [ $# -gt 0 ]; do
  case "$1" in
    --binary)  BINARY="${2:-}";      shift 2 ;;
    --runs)    RUNS="${2:-}";        shift 2 ;;
    --out)     OUT="${2:-}";         shift 2 ;;
    --load)    LOAD="${2:-}";        shift 2 ;;
    --seconds) SECONDS_CAP="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done

[ -n "$BINARY" ] || { echo "FATAL: --binary is required" >&2; exit 64; }
[ -n "$OUT" ]    || { echo "FATAL: --out is required" >&2; exit 64; }

HERE=$(cd "$(dirname "$0")" && pwd)

echo "F23_H1_LOAD_BEFORE=$(cut -d' ' -f1 /proc/loadavg)"

PIDS=""
i=0
while [ "$i" -lt "$LOAD" ]; do
  i=$((i + 1))
  timeout "$SECONDS_CAP" nice -n 5 sh -c 'while :; do :; done' >/dev/null 2>&1 &
  PIDS="$PIDS $!"
done

drop_load() {
  for p in $PIDS; do kill -9 "$p" 2>/dev/null; done
}
trap drop_load EXIT

sleep 30
echo "F23_H1_LOAD_DURING=$(cut -d' ' -f1 /proc/loadavg)"

bash "$HERE/f23-h1-repro.sh" --binary "$BINARY" --runs "$RUNS" --out "$OUT"
RC=$?

drop_load
trap - EXIT
echo "F23_H1_LOAD_AFTER=$(cut -d' ' -f1 /proc/loadavg)"
exit "$RC"
