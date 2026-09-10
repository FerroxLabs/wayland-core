#!/bin/bash
# w15/soak1349 (wayland#1349 c2): launch ONE fresh 7200 s mixed soak with the
# driver of record (evidence/mixed-soak353/run2/driver.py) changed in exactly ONE
# line -- the pinned binary hash -- against the release+voice binary.
# hetzner is shared: waits up to MAXWAIT seconds for no visible cargo/rustc, then
# launches anyway and DISCLOSES what was running (it does not refuse on load).
# Usage: soak-launch.sh <source-sha> <run-name> <max-wait-seconds>
set -euo pipefail
SRC=$1; RUN=$2; MAXWAIT=$3
L=/root/w15-soak1349
W=$L/soak
BIN=$L/bin/wayland-core
ORIG=/root/waylandcore-stabilization-20260905/evidence/mixed-soak353/run2/driver.py
ORIG_SHA=f64b20bd620911a08406449817f88a9df96d8ddc3dc0b2673e75f261ede318ce
OLD_PIN=c68855d52d4ebc7ec2f5dc3ac41fc2cab646db6de9bef43c031146f6da5918da
BUILD_CMD="WCORE_PROOF_SLOT=parallel-1 tools/remote-proof.py build --locked --release --features voice -p wcore-cli -j 6"
ROOT=$W/$RUN
mkdir -p "$W"
test ! -e "$ROOT" || { echo "REFUSED: $ROOT exists"; exit 4; }
test ! -e "$W/$RUN.launch" || { echo "REFUSED: $W/$RUN.launch exists"; exit 4; }
test "$(sha256sum "$ORIG" | cut -d' ' -f1)" = "$ORIG_SHA" || { echo "REFUSED: original driver hash drifted"; exit 5; }
BINSHA=$(sha256sum "$BIN" | cut -d' ' -f1)
"$BIN" --build-info | grep -q "$SRC" || { echo "REFUSED: binary is not source $SRC"; exit 7; }
DRV=$W/driver-$RUN.py
python3 - "$ORIG" "$DRV" "$BINSHA" "$OLD_PIN" <<'PY'
import sys
src, dst, sha, old_pin = sys.argv[1:]
s = open(src).read()
old = 'assert receipt["binary_sha256"] == "%s", "immutable binary hash mismatch"' % old_pin
assert s.count(old) == 1 and s.count(old_pin) == 1
open(dst, "w").write(s.replace(old, old.replace(old_pin, sha)))
PY
diff "$ORIG" "$DRV" > "$W/driver-$RUN.diff" || true
test "$(grep -c '^[<>]' "$W/driver-$RUN.diff")" = 2 || { echo "REFUSED: driver diff is not exactly one line"; exit 8; }
test "$(grep -c '^[0-9]' "$W/driver-$RUN.diff")" = 1 || { echo "REFUSED: driver diff has more than one hunk"; exit 8; }

# Match real compiler/test executables by process NAME, never by argv text.
builds() { ps -eo pid,etimes,comm,args --no-headers | awk '$3=="cargo"||$3=="rustc"||$3=="cargo-nextest"||$3=="ld.mold"||$3=="mold"' | cut -c1-220 || true; }
START=$(date +%s)
: > "$W/$RUN.wait"
while :; do
  B=$(builds)
  N=$(printf '%s' "$B" | grep -c . || true)
  echo "$(date -u +%FT%TZ) loadavg=$(cat /proc/loadavg) builds=$N" >> "$W/$RUN.wait"
  if [ "$N" = 0 ]; then CALM=yes; break; fi
  if [ $(( $(date +%s) - START )) -ge "$MAXWAIT" ]; then CALM=no; break; fi
  sleep 20
done
printf '%s\n' "$B" > "$W/$RUN.builds-at-launch"
{
  echo "launched_utc=$(date -u +%FT%TZ)"
  echo "host=$(hostname) nproc=$(nproc) kernel=$(uname -r) mem=$(grep MemTotal /proc/meminfo | tr -s ' ')"
  echo "source=$SRC"
  echo "binary=$BIN"
  echo "binary_sha256=$BINSHA"
  echo "build_command=$BUILD_CMD"
  echo "driver_original=$ORIG"
  echo "driver_original_sha256=$(sha256sum "$ORIG" | cut -d' ' -f1)"
  echo "driver_run_sha256=$(sha256sum "$DRV" | cut -d' ' -f1)"
  echo "loadavg_at_launch=$(cat /proc/loadavg)"
  echo "waited_for_calm_seconds=$(( $(date +%s) - START )) max_wait=$MAXWAIT calm=$CALM"
  echo "builds_running_at_launch=$N (list in $RUN.builds-at-launch)"
  echo "isolation=setsid nohup unshare --net"
} > "$W/$RUN.launch"
setsid nohup unshare --net python3 "$DRV" --binary "$BIN" --source "$SRC" --root "$ROOT" \
  --build-profile release-voice > "$W/$RUN.log" 2>&1 < /dev/null &
DPID=$!
echo "driver_pid=$DPID" >> "$W/$RUN.launch"
setsid nohup python3 "$L/soak-monitor.py" "$RUN" "$DPID" > "$W/$RUN.monitor.log" 2>&1 < /dev/null &
echo "monitor_pid=$!" >> "$W/$RUN.launch"
cat "$W/$RUN.launch"
