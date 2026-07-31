#!/usr/bin/env bash
# Phase 27 Criterion 1 — LIVE exercise of the consolidated media intake against
# the SHIPPED binary. Nothing here calls an internal function.
#
# Each observation changes exactly ONE thing (the path handed to the tool) and
# records what the SHIPPED binary put on the wire.
#
#   f27-c1-intake-live.sh <repo-root> <out-dir>
set -u

REPO="${1:?repo root}"
OUT="${2:?output dir}"
# `WL_BIN` lets a second platform point at whichever profile it actually built
# (the macOS leg runs the debug binary). Defaults to the original release path.
BIN="${WL_BIN:-$REPO/target/release/wayland-core}"
CORPUS="$REPO/crates/wcore-fixture-harness/fixtures/f27/intake"
MOCK="$REPO/.planning/scripts/f27-mock-provider.py"
PORT=18931

mkdir -p "$OUT/live" "$OUT/wire"
: > "$OUT/OBS-RAW.log"
log() { echo "$*" | tee -a "$OUT/OBS-RAW.log"; }

start_mock() {
  python3 "$MOCK" "$PORT" "$1" ${2:+"$2"} ${3:+"$3"} > "$OUT/mock.stdout" 2>&1 &
  MOCK_PID=$!
  for _ in $(seq 1 60); do
    grep -q F27-MOCK-READY "$OUT/mock.stdout" 2>/dev/null && return 0
    sleep 0.1
  done
  echo "MOCK FAILED TO START" >&2; return 1
}
stop_mock() { kill "$MOCK_PID" 2>/dev/null; wait "$MOCK_PID" 2>/dev/null; }

write_config() {
  local home="$1"
  {
    echo '[default]'
    echo 'provider = "anthropic"'
    echo 'model = "claude-sonnet-4-20250514"'
    echo ''
    echo '[providers.anthropic]'
    echo 'api_key = "f27-fixture-credential"'
    echo "base_url = \"http://127.0.0.1:$PORT\""
    echo ''
    echo '[session]'
    echo 'enabled = false'
  } > "$home/config.toml"
}

# ── the fixtures ───────────────────────────────────────────────────────────
FIX="$(mktemp -d)"
# A REAL WAV header, so nothing below can be refused for its format.
printf 'RIFF\x24\x08\x00\x00WAVEfmt \x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00' > "$FIX/good.wav"
# Same bytes under a deny-listed credential name.
mkdir -p "$FIX/.ssh"; cp "$FIX/good.wav" "$FIX/.ssh/id_rsa"
# Same bytes reachable only through a `..` component.
mkdir -p "$FIX/sub"
# Same bytes behind a symlink.
ln -sf "$FIX/good.wav" "$FIX/linked.wav"
# Sparse file three times the 25 MB cap, real WAV header at the front.
python3 - "$FIX/over-cap.wav" <<'PY'
import sys
cap = 25*1024*1024
p = sys.argv[1]
with open(p, "wb") as f:
    f.truncate(cap*3)
with open(p, "r+b") as f:
    f.write(b"RIFF\x24\x08\x00\x00WAVEfmt " + b"\x00"*16)
PY

# Drive the transcribe_audio TOOL through the shipped binary.
# $1 = label, $2 = audio_path handed to the tool
obs_audio() {
  local label="$1" apath="$2"
  local home; home="$(mktemp -d)"
  local cap="$OUT/wire/audio-$label.jsonl"
  write_config "$home"
  start_mock "$cap" "transcribe_audio" "{\"audio_path\":\"$apath\"}" || return 1
  {
    printf '{"type":"message","msg_id":"m1","content":"transcribe it"}\n'
    sleep 8
  } | env -i \
        HOME="$home" WAYLAND_HOME="$home" TERM=dumb \
        PATH=/usr/local/bin:/usr/bin:/bin \
        GROQ_API_KEY=f27-fixture-not-a-real-key \
        "$BIN" --json-stream --provider anthropic \
      > "$OUT/live/audio-$label.jsonl" 2> "$OUT/live/audio-$label.stderr"
  stop_mock
  log ""
  log "=== OBS audio/$label — audio_path=$apath"
  # The tool_result the engine put BACK on the wire is the measurement.
  python3 "$REPO/.planning/scripts/f27-c1-extract-toolresult.py" "$cap" | tee -a "$OUT/OBS-RAW.log"
}

# Drive the composer/host-protocol surface with one attachment.
obs_host() {
  local label="$1" entry="$2"
  local home; home="$(mktemp -d)"
  local cap="$OUT/wire/host-$label.jsonl"
  write_config "$home"
  start_mock "$cap" || return 1
  {
    printf '{"type":"message","msg_id":"m1","content":"describe the attachment","files":["%s"]}\n' "$entry"
    sleep 6
  } | env -i \
        HOME="$home" WAYLAND_HOME="$home" TERM=dumb \
        PATH=/usr/local/bin:/usr/bin:/bin \
        "$BIN" --json-stream --provider anthropic \
      > "$OUT/live/host-$label.jsonl" 2> "$OUT/live/host-$label.stderr"
  stop_mock
  log ""
  log "=== OBS host/$label — $entry"
  log "--- what the USER saw (error/result events on the protocol stream):"
  grep -o '"type":"error"[^}]*}' "$OUT/live/host-$label.jsonl" | head -3 | tee -a "$OUT/OBS-RAW.log" || true
  grep -o '"message":"[^"]*"' "$OUT/live/host-$label.jsonl" | head -3 | tee -a "$OUT/OBS-RAW.log" || true
  log "--- what went ON THE WIRE to the provider:"
  python3 "$REPO/.planning/scripts/f27-extract-wire.py" "$cap" 2>/dev/null | head -8 | tee -a "$OUT/OBS-RAW.log" || true
}

log "binary:  $BIN"
log "version: $("$BIN" --version 2>&1 | head -1)"
log "fixtures: $FIX"

# §3b-ii — read the arm back from the product's own traffic, not from the env.
log ""
log "### Provider read-back: every capture below is a file THIS MOCK wrote."
log "### If the engine had talked to api.anthropic.com the capture would be empty."

obs_audio "valid"        "$FIX/good.wav"
obs_audio "denylisted"   "$FIX/.ssh/id_rsa"
obs_audio "traversal"    "$FIX/sub/../good.wav"
obs_audio "symlink"      "$FIX/linked.wav"
obs_audio "over-cap"     "$FIX/over-cap.wav"
obs_audio "relative"     "good.wav"

obs_host  "valid-png"    "$CORPUS/valid-image.png"
obs_host  "mismatch"     "$CORPUS/mismatch-png-body-jpg-ext.jpg"
obs_host  "pdf-as-image" "$CORPUS/valid-doc.pdf"

log ""
log "### secret sweep — the fixture bytes must not appear in any capture"
log "matches for the fixture credential string in $OUT: $(grep -rl 'f27-fixture-not-a-real-key' "$OUT" 2>/dev/null | wc -l)"

rm -rf "$FIX"
log ""
log "DONE"
