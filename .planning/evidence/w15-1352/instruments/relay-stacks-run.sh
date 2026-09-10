#!/usr/bin/env bash
# w15/relay1352: attach the cancellation-stack instrument, reproduce the c8
# 16 MiB relay cancellation on the debug fix binary, then a c1 control.
set -u
W=/root/w15-relay1352/stacks1
BIN=/root/w15-leak2/bin/fix/wayland-core
SRC=008d88ce960322967adcef7637b44a5cfad44207
P=/root/w15-relay1352/w15-leak2-probe6.py
mkdir -p "$W"
ST="$W/status"
echo "state=starting utc=$(date -u +%FT%TZ) loadavg=$(cut -d' ' -f1-3 /proc/loadavg)" > "$ST"
echo "binary_sha256=$(sha256sum $BIN | cut -d' ' -f1)" >> "$ST"
bpftrace /root/w15-relay1352/relay-cancel-stacks.bt > "$W/bpf.out" 2> "$W/bpf.err" &
BPF=$!
echo "bpftrace_pid=$BPF" >> "$ST"
for i in $(seq 1 240); do grep -q Attaching "$W/bpf.out" && break; sleep 1; done
grep -q Attaching "$W/bpf.out" || { echo "state=bpf_attach_failed" >> "$ST"; kill "$BPF"; exit 1; }
echo "attached_utc=$(date -u +%FT%TZ)" >> "$ST"
echo "armA_start loadavg=$(cut -d' ' -f1-3 /proc/loadavg)" >> "$ST"
python3 "$P" --binary "$BIN" --source "$SRC" --root "$W/armA-c8-16m-optional" --cycles 16 \
  --text-bytes 16777216 --chunk-bytes 32768 --concurrency 8 --session-durability optional > "$W/armA.stdout" 2>&1
echo "armA_exit=$? loadavg=$(cut -d' ' -f1-3 /proc/loadavg) utc=$(date -u +%FT%TZ)" >> "$ST"
echo "MARK armA_done ns_utc=$(date -u +%FT%TZ)" >> "$W/marks"
python3 "$P" --binary "$BIN" --source "$SRC" --root "$W/armB-c1-16m-optional" --cycles 3 \
  --text-bytes 16777216 --chunk-bytes 32768 --concurrency 1 --session-durability optional > "$W/armB.stdout" 2>&1
echo "armB_exit=$? loadavg=$(cut -d' ' -f1-3 /proc/loadavg) utc=$(date -u +%FT%TZ)" >> "$ST"
kill -INT "$BPF"
wait "$BPF"
echo "state=done utc=$(date -u +%FT%TZ)" >> "$ST"
