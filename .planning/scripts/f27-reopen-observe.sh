#!/usr/bin/env bash
# Phase 27 / plan 27-01 Task 1 — settle the reopen-by-name question BY OBSERVATION.
#
# The claim under test is that the PDF path validates a path and then hands the
# NAME onward, so the file is resolved a second time and the bytes admitted are
# not necessarily the bytes validated. Reading source suggests it; only counting
# the kernel's own path resolutions settles it.
#
# Method: strace the shipped binary, follow forks, and count every syscall that
# resolves the corpus path by name (openat / stat / newfstatat / access).
# The composer/vision path is measured the same way as a control, because it is
# the in-tree implementation that is supposed to resolve exactly once.
#
#   f27-reopen-observe.sh <repo-root> <out-dir>
set -u

REPO="${1:?repo root}"
OUT="${2:?out dir}"
BIN="$REPO/target/release/wayland-core"
CORPUS="$REPO/crates/wcore-fixture-harness/fixtures/f27/intake"
MOCK="$REPO/.planning/scripts/f27-mock-provider.py"
PORT=18929

mkdir -p "$OUT"
: > "$OUT/REOPEN.log"
log() { echo "$*" | tee -a "$OUT/REOPEN.log"; }

start_mock() {
  python3 "$MOCK" "$PORT" "$1" ${2:+"$2"} ${3:+"$3"} > "$OUT/mock.out" 2>&1 &
  MOCK_PID=$!
  for _ in $(seq 1 60); do
    grep -q F27-MOCK-READY "$OUT/mock.out" 2>/dev/null && return 0
    sleep 0.1
  done
  return 1
}
stop_mock() { kill "$MOCK_PID" 2>/dev/null; wait "$MOCK_PID" 2>/dev/null; }

write_config() {
  cat > "$1/config.toml" <<EOF
[default]
provider = "anthropic"
model = "claude-sonnet-4-20250514"

[providers.anthropic]
api_key = "f27-fixture-credential"
base_url = "http://127.0.0.1:$PORT"

[session]
enabled = false
EOF
}

# $1 label, $2 corpus entry, $3 mode (pdf|composer)
measure() {
  local label="$1" entry="$2" mode="$3"
  local home; home="$(mktemp -d)"
  local trace="$OUT/strace-$label.txt"
  write_config "$home"

  if [ "$mode" = pdf ]; then
    start_mock "$OUT/wire-$label.jsonl" pdf_extract "{\"file_path\":\"$CORPUS/$entry\"}" || return 1
    env -i HOME="$home" WAYLAND_HOME="$home" TERM=dumb PATH=/usr/local/bin:/usr/bin:/bin \
      strace -f -y -e trace=openat,open,stat,lstat,newfstatat,access,statx -o "$trace" \
      timeout 90 "$BIN" --provider anthropic --dangerously-skip-permissions \
      "Read $CORPUS/$entry with pdf_extract." > "$OUT/out-$label.txt" 2>&1
  else
    start_mock "$OUT/wire-$label.jsonl" || return 1
    {
      printf '{"type":"message","msg_id":"m1","content":"describe","files":["%s"]}\n' "$CORPUS/$entry"
      sleep 6
    } | env -i HOME="$home" WAYLAND_HOME="$home" TERM=dumb PATH=/usr/local/bin:/usr/bin:/bin \
      strace -f -y -e trace=openat,open,stat,lstat,newfstatat,access,statx -o "$trace" \
      "$BIN" --json-stream --provider anthropic > "$OUT/out-$label.txt" 2>&1
  fi
  local rc=$?
  stop_mock

  # Count only resolutions of the corpus entry BY NAME. A read from an already
  # open descriptor does not appear here, which is exactly the distinction.
  local hits; hits=$(grep -c -- "$CORPUS/$entry" "$trace" 2>/dev/null || echo 0)
  log "=== REOPEN $label (mode=$mode entry=$entry) ==="
  log "RC: $rc"
  log "NAME-RESOLUTIONS-OF-CORPUS-ENTRY: $hits"
  grep -- "$CORPUS/$entry" "$trace" 2>/dev/null | sed 's/^/    /' >> "$OUT/REOPEN.log"
  log ""
  rm -rf "$home"
}

log "F27 REOPEN-BY-NAME OBSERVATION"
log "HOST: $(hostname)"
log "SHA: $(cd "$REPO" && git rev-parse HEAD)"
log "BIN: $BIN"
log ""

measure pdf-path      valid-doc.pdf   pdf
measure composer-path valid-image.png composer

log "DONE"
