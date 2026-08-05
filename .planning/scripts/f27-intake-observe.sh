#!/usr/bin/env bash
# Phase 27 / plan 27-01 Task 1 — live intake observations against the SHIPPED binary.
#
# Single-variable discipline: each observation changes exactly ONE thing (the
# corpus entry, or the provider's vision capability) and records the result.
#
# Usage (on the host that holds the built binary):
#   f27-intake-observe.sh <repo-root> <out-dir>
set -u

REPO="${1:?repo root}"
OUT="${2:?output dir}"
BIN="$REPO/target/release/wayland-core"
CORPUS="$REPO/crates/wcore-fixture-harness/fixtures/f27/intake"
MOCK="$REPO/.planning/scripts/f27-mock-provider.py"

mkdir -p "$OUT"
: > "$OUT/OBS-RAW.log"

log() { echo "$*" | tee -a "$OUT/OBS-RAW.log"; }

PORT=18927

start_mock() {
  # $1 = capture file; $2/$3 optional tool name + JSON input for the first reply
  python3 "$MOCK" "$PORT" "$1" ${2:+"$2"} ${3:+"$3"} > "$OUT/mock.stdout" 2>&1 &
  MOCK_PID=$!
  for _ in $(seq 1 50); do
    grep -q F27-MOCK-READY "$OUT/mock.stdout" 2>/dev/null && return 0
    sleep 0.1
  done
  echo "MOCK FAILED TO START" >&2
  return 1
}

stop_mock() { kill "$MOCK_PID" 2>/dev/null; wait "$MOCK_PID" 2>/dev/null; }

# $1 = home dir, $2 = supports_vision literal (true|false|unset)
write_config() {
  local home="$1" vision="$2"
  {
    echo '[default]'
    echo 'provider = "anthropic"'
    echo 'model = "claude-sonnet-4-20250514"'
    echo ''
    echo '[providers.anthropic]'
    echo 'api_key = "f27-fixture-credential"'
    echo "base_url = \"http://127.0.0.1:$PORT\""
    if [ "$vision" != "unset" ]; then
      echo ''
      echo '[providers.anthropic.compat]'
      echo "supports_vision = $vision"
    fi
    echo ''
    echo '[session]'
    echo 'enabled = false'
  } > "$home/config.toml"
}

# Drive the host-protocol (composer) surface with one attachment.
# $1 = label, $2 = corpus entry, $3 = supports_vision
obs_host() {
  local label="$1" entry="$2" vision="$3"
  local home; home="$(mktemp -d)"
  local cap="$OUT/wire-$label.jsonl"
  write_config "$home" "$vision"
  start_mock "$cap" || return 1

  local stream="$OUT/live/host-$label.jsonl"
  mkdir -p "$OUT/live"
  {
    printf '{"type":"message","msg_id":"m1","content":"describe the attachment","files":["%s"]}\n' \
      "$CORPUS/$entry"
    # Give the engine time to complete the turn before stdin closes.
    sleep 6
  } | env -i \
        HOME="$home" WAYLAND_HOME="$home" TERM=dumb \
        PATH=/usr/local/bin:/usr/bin:/bin \
        "$BIN" --json-stream --provider anthropic \
      > "$stream" 2> "$OUT/live/host-$label.stderr"
  local rc=$?
  stop_mock

  log "=== OBS $label ==="
  log "CMD: $BIN --json-stream --provider anthropic   (entry=$entry supports_vision=$vision)"
  log "RC: $rc"
  log "--- emitted stream ($(wc -l < "$stream") lines) ---"
  sed -n '1,60p' "$stream" | tee -a "$OUT/OBS-RAW.log" > /dev/null
  cat "$stream" >> "$OUT/OBS-RAW.log"
  log "--- outbound request capture ($(wc -l < "$cap" 2>/dev/null || echo 0) requests) ---"
  cat "$cap" >> "$OUT/OBS-RAW.log" 2>/dev/null
  log ""
  rm -rf "$home"
}

# Drive the standalone surface: a headless prompt naming a corpus document.
# $1 = label, $2 = corpus entry
obs_standalone() {
  local label="$1" entry="$2"
  local home; home="$(mktemp -d)"
  local cap="$OUT/wire-$label.jsonl"
  write_config "$home" unset
  # Force the FIRST model reply to be a real pdf_extract tool_use so the tool
  # path is genuinely exercised, rather than hoping a mock text reply calls it.
  start_mock "$cap" pdf_extract "{\"file_path\":\"$CORPUS/$entry\"}" || return 1

  mkdir -p "$OUT/live"
  local sout="$OUT/live/standalone-$label.txt"
  env -i HOME="$home" WAYLAND_HOME="$home" TERM=dumb \
      PATH=/usr/local/bin:/usr/bin:/bin \
      timeout 90 "$BIN" --provider anthropic --dangerously-skip-permissions \
      "Read $CORPUS/$entry with pdf_extract and print its text verbatim." \
      > "$sout" 2>&1
  local rc=$?
  stop_mock

  log "=== OBS $label (standalone) ==="
  log "CMD: $BIN --provider anthropic '<prompt naming $entry>'"
  log "RC: $rc"
  cat "$sout" >> "$OUT/OBS-RAW.log"
  log "--- outbound capture ---"
  cat "$cap" >> "$OUT/OBS-RAW.log" 2>/dev/null
  log ""
  rm -rf "$home"
}

log "F27 INTAKE OBSERVATIONS"
log "BIN: $BIN"
log "SHA: $(cd "$REPO" && git rev-parse HEAD)"
log "HOST: $(hostname)"
log "VERSION: $("$BIN" --version 2>&1)"
log ""

obs_host valid-png              valid-image.png                 unset
obs_host mismatch-png-in-jpg    mismatch-png-body-jpg-ext.jpg   unset
obs_host empty-png              empty.png                       unset
obs_host truncated-png          truncated-header.png            unset
obs_host under-vision-min       boundary-under-vision-min.png   unset
obs_host at-vision-min          boundary-at-vision-min.png      unset
obs_host degrade-vision-off     valid-image.png                 false
obs_host degrade-vision-on      valid-image.png                 true
obs_host pdf-as-attachment      valid-doc.pdf                   unset

obs_standalone pdf-valid        valid-doc.pdf
obs_standalone pdf-mismatch     mismatch-not-a-pdf.pdf

log "DONE"
