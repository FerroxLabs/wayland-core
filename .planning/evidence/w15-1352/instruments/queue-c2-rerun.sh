#!/usr/bin/env bash
# w15/relay1352 c2 rerun after review. Release binaries only.
#  standard : the c2 mixed-reader schedule (32 fast 16 MiB rows at c8 and at c32)
#  churn    : --mixed-chunks (first half 512 KiB events, rest 4 KiB), a c32-heavy
#             schedule so retained history stays over the 64 MiB cap while
#             sessions close; drift would show as later turns detaching.
# usage: queue-c2-rerun.sh OUTDIR
set -u
O=${1:?outdir}
D=/root/w15-relay1352/w15-relay1352-mixed.py
STD=8,8,32,8,8,8,32,8,8,8
CHURN=32,8,32,32,8,32,32,8,32,32
BASE=/root/w15-relay1352/bin/base-release/wayland-core
FIX1=/root/w15-relay1352/bin/fix1-release-d73cb623/wayland-core
FIX2=/root/w15-relay1352/bin/fix2-release/wayland-core
mkdir -p "$O"
arm() { # NAME BINARY SCHEDULE [EXTRA]
  local name=$1 bin=$2 schedule=$3 extra=${4:-}
  local src
  src=$("$bin" --build-info | sed -n 's/.*(source \([0-9a-f]\{40\}\)).*/\1/p')
  echo "arm=$name start utc=$(date -u +%FT%TZ) loadavg=$(cut -d' ' -f1-3 /proc/loadavg) bin_sha=$(sha256sum "$bin" | cut -d' ' -f1) source=$src schedule=$schedule extra=$extra" >> "$O/status"
  python3 "$D" --binary "$bin" --source "$src" --root "$O/$name" --label "$name" --schedule "$schedule" $extra > "$O/$name.stdout" 2>&1
  echo "arm=$name exit=$? end utc=$(date -u +%FT%TZ) loadavg=$(cut -d' ' -f1-3 /proc/loadavg)" >> "$O/status"
}
echo "queue start utc=$(date -u +%FT%TZ) driver_sha=$(sha256sum "$D" | cut -d' ' -f1)" >> "$O/status"
arm base-release        "$BASE" "$STD"
arm fix2-release        "$FIX2" "$STD"
arm base-release-churn  "$BASE" "$CHURN" --mixed-chunks
arm fix1-release-churn  "$FIX1" "$CHURN" --mixed-chunks
arm fix2-release-churn  "$FIX2" "$CHURN" --mixed-chunks
echo "queue state=done utc=$(date -u +%FT%TZ)" >> "$O/status"
