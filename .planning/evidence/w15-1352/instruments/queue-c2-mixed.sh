#!/usr/bin/env bash
# w15/relay1352 c2: base-vs-fix mixed-reader A/B through the real acp serve.
# Each arm: 8 batches at c8 (4 fast + 2 slow + 2 disconnected) and 2 at c32
# (16 fast + 8 slow + 8 disconnected), all 16 MiB -> 32 fast rows per
# concurrency per arm. Arms interleave base/fix within each build profile.
# usage: queue-c2-mixed.sh OUTDIR
set -u
O=${1:?outdir}
D=/root/w15-relay1352/w15-relay1352-mixed.py
SCHEDULE=8,8,32,8,8,8,32,8,8,8
mkdir -p "$O"
arm() { # NAME BINARY
  local name=$1 bin=$2
  local src
  src=$("$bin" --build-info | sed -n 's/.*(source \([0-9a-f]\{40\}\)).*/\1/p')
  echo "arm=$name start utc=$(date -u +%FT%TZ) loadavg=$(cut -d' ' -f1-3 /proc/loadavg) bin_sha=$(sha256sum "$bin" | cut -d' ' -f1) source=$src" >> "$O/status"
  python3 "$D" --binary "$bin" --source "$src" --root "$O/$name" --label "$name" --schedule "$SCHEDULE" > "$O/$name.stdout" 2>&1
  echo "arm=$name exit=$? end utc=$(date -u +%FT%TZ) loadavg=$(cut -d' ' -f1-3 /proc/loadavg)" >> "$O/status"
}
echo "queue start utc=$(date -u +%FT%TZ) driver_sha=$(sha256sum "$D" | cut -d' ' -f1)" >> "$O/status"
arm base-debug   /root/w15-leak2/bin/fix/wayland-core
arm fix-debug    /root/w15-relay1352/bin/fix-debug/wayland-core
arm base-release /root/w15-relay1352/bin/base-release/wayland-core
arm fix-release  /root/w15-relay1352/bin/fix-release/wayland-core
echo "queue state=done utc=$(date -u +%FT%TZ)" >> "$O/status"
