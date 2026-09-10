#!/usr/bin/env bash
# w15/relay1352 c3 residual: is the release-base c1 16 MiB slowdown the glibc
# main-heap trim churn? Same base binary with ONLY the trim threshold raised
# through a glibc tunable, interleaved with the unchanged base and 3530199fb.
set -u
P=/root/w15-relay1352/w15-leak2-probe6.py
O=/root/w15-relay1352/c3-trim1
RB=/root/w15-relay1352/bin/base-release/wayland-core
RS=cd85dac8872eedace6fbbd6421e2acdf4f2f0f60
TB=/root/w15-relay1352/bin/src3530-release/wayland-core
TS=3530199fb757f3fcefecf9e40ba820775b867d84
TUNE=GLIBC_TUNABLES=glibc.malloc.trim_threshold=268435456
mkdir -p "$O"
run() { # NAME BIN SRC [EXTRA_ENV]
  local name=$1 bin=$2 src=$3 extra=${4:-}
  echo "arm=$name start utc=$(date -u +%FT%TZ) loadavg=$(cut -d' ' -f1-3 /proc/loadavg) extra=$extra" >> "$O/status"
  if [ -n "$extra" ]; then
    python3 "$P" --binary "$bin" --source "$src" --root "$O/$name" --cycles 5 --text-bytes 16777216 \
      --chunk-bytes 32768 --concurrency 1 --session-durability on --extra-env "$extra" > "$O/$name.stdout" 2>&1
  else
    python3 "$P" --binary "$bin" --source "$src" --root "$O/$name" --cycles 5 --text-bytes 16777216 \
      --chunk-bytes 32768 --concurrency 1 --session-durability on > "$O/$name.stdout" 2>&1
  fi
  echo "arm=$name exit=$? end utc=$(date -u +%FT%TZ) loadavg=$(cut -d' ' -f1-3 /proc/loadavg)" >> "$O/status"
}
echo "queue start utc=$(date -u +%FT%TZ)" >> "$O/status"
for block in 1 2; do
  run "rel-base-b$block"      "$RB" "$RS"
  run "rel-base-trim-b$block" "$RB" "$RS" "$TUNE"
  run "rel-3530-b$block"      "$TB" "$TS"
done
echo "strace start utc=$(date -u +%FT%TZ)" >> "$O/status"
python3 "$P" --binary "$RB" --source "$RS" --root "$O/strace-rel-base-trim" --cycles 2 --text-bytes 16777216 \
  --chunk-bytes 32768 --concurrency 1 --session-durability on --extra-env "$TUNE" \
  --wrap "strace -f -c -o $O/strace-rel-base-trim.txt -e trace=%memory" > "$O/strace-rel-base-trim.stdout" 2>&1
echo "strace exit=$? utc=$(date -u +%FT%TZ)" >> "$O/status"
echo "queue state=done utc=$(date -u +%FT%TZ)" >> "$O/status"
