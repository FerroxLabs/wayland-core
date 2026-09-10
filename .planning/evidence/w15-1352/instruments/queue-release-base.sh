#!/usr/bin/env bash
# w15/relay1352: does the RELEASE base binary (cd85dac88) reproduce the relay
# cancellation, and what is its c1 16 MiB latency?
set -u
A=/root/w15-relay1352/arm.sh
O=/root/w15-relay1352/release-base1
B=/root/w15-relay1352/bin/base-release/wayland-core
S=cd85dac8872eedace6fbbd6421e2acdf4f2f0f60
mkdir -p "$O"
echo "queue start utc=$(date -u +%FT%TZ)" >> "$O/status"
"$A" "$O" rel-c8-16m-optional  "$B" "$S" 8  16 optional
"$A" "$O" rel-c32-16m-optional "$B" "$S" 32 32 optional
"$A" "$O" rel-c1-16m-on        "$B" "$S" 1  10 on
echo "queue state=done utc=$(date -u +%FT%TZ)" >> "$O/status"
