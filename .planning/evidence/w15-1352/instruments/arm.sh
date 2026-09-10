#!/usr/bin/env bash
# w15/relay1352: run ONE probe6 arm and append a status line.
# usage: arm.sh OUTDIR NAME BINARY SOURCE CONCURRENCY CYCLES DURABILITY [TEXT_BYTES]
set -u
OUT=$1; NAME=$2; BIN=$3; SRC=$4; CONC=$5; CYC=$6; DUR=$7; BYTES=${8:-16777216}
P=/root/w15-relay1352/w15-leak2-probe6.py
mkdir -p "$OUT"
ST="$OUT/status"
echo "arm=$NAME start utc=$(date -u +%FT%TZ) loadavg=$(cut -d' ' -f1-3 /proc/loadavg) bin_sha=$(sha256sum "$BIN" | cut -c1-16) conc=$CONC cycles=$CYC durability=$DUR bytes=$BYTES" >> "$ST"
python3 "$P" --binary "$BIN" --source "$SRC" --root "$OUT/$NAME" --cycles "$CYC" \
  --text-bytes "$BYTES" --chunk-bytes 32768 --concurrency "$CONC" --session-durability "$DUR" > "$OUT/$NAME.stdout" 2>&1
echo "arm=$NAME exit=$? end utc=$(date -u +%FT%TZ) loadavg=$(cut -d' ' -f1-3 /proc/loadavg)" >> "$ST"
