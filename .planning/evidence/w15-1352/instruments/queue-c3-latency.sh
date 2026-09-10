#!/usr/bin/env bash
# w15/relay1352 c3: concurrency-1 16 MiB fast-turn latency on both sources,
# interleaved in blocks of 5 so host-load drift is shared by every arm.
set -u
A=/root/w15-relay1352/arm.sh
O=/root/w15-relay1352/c3-latency1
RB=/root/w15-relay1352/bin/base-release/wayland-core
RS=cd85dac8872eedace6fbbd6421e2acdf4f2f0f60
TB=/root/w15-relay1352/bin/src3530-release/wayland-core
TS=3530199fb757f3fcefecf9e40ba820775b867d84
DB=/root/w15-leak2/bin/fix/wayland-core
DS=008d88ce960322967adcef7637b44a5cfad44207
mkdir -p "$O"
echo "queue start utc=$(date -u +%FT%TZ)" >> "$O/status"
for block in 1 2; do
  "$A" "$O" "rel-base-b$block"   "$RB" "$RS" 1 5 on
  "$A" "$O" "rel-3530-b$block"   "$TB" "$TS" 1 5 on
  "$A" "$O" "dbg-base-b$block"   "$DB" "$DS" 1 5 on
done
echo "queue state=done utc=$(date -u +%FT%TZ)" >> "$O/status"
