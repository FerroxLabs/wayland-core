#!/usr/bin/env bash
# Phase 27 / plan 27-02 Task 1 — measure what the shipped handshake CLAIMS
# against what this machine can actually do, one absence at a time.
#
# The claim under test, taken verbatim from crates/wcore-cli/tests/
# release_binary_smoke.rs: `capabilities.browser_suite` and
# `capabilities.computer_use` are derived from PLUGIN LINKAGE
# (`PluginCapabilitySet::from_verified`), not from any runtime probe. If that
# is so, the handshake is invariant under every absence below — and a host
# reading those booleans as "this capability is available" is being misled.
#
#   f27-readiness-observe.sh <repo-root> <out-dir>
set -u

REPO="${1:?repo root}"
OUT="${2:?out dir}"
BIN="$REPO/target/release/wayland-core"
mkdir -p "$OUT/handshake"
: > "$OUT/READINESS.log"
log() { echo "$*" | tee -a "$OUT/READINESS.log"; }

# Capture the ready handshake under one named environment condition.
# $1 = label; remaining args = env assignments applied on top of the base.
capture() {
  local label="$1"; shift
  local home; home="$(mktemp -d)"
  local f="$OUT/handshake/$label.jsonl"
  # A credential is required to reach the handshake at all; it is deliberately
  # a fixture value pointed at an unreachable loopback port, so no request can
  # leave the box. This is a constant across every observation and therefore
  # cannot be the variable any delta below is attributed to.
  cat > "$home/config.toml" <<'CFG'
[default]
provider = "anthropic"
model = "claude-sonnet-4-20250514"

[providers.anthropic]
api_key = "f27-fixture-credential"
base_url = "http://127.0.0.1:1"

[session]
enabled = false
CFG
  printf '{"type":"message","msg_id":"probe","content":"hello"}\n' | \
    timeout 45 env -i HOME="$home" WAYLAND_HOME="$home" TERM=dumb \
      PATH="${PROBE_PATH:-/usr/local/bin:/usr/bin:/bin}" "$@" \
      "$BIN" --json-stream > "$f" 2>"$OUT/handshake/$label.stderr"
  local rc=$?
  log "--- HANDSHAKE $label (rc=$rc, $(wc -l < "$f") events) ---"
  log "CLAIMS: $(python3 - "$f" <<'PY'
import json,sys
for line in open(sys.argv[1]):
    try: e=json.loads(line)
    except Exception: continue
    if e.get("type")=="ready":
        c=e["capabilities"]
        print(" ".join(f"{k}={c.get(k)}" for k in ("browser_suite","computer_use","plugins","mcp")))
        break
else:
    print("NO READY EVENT")
PY
)"
  log "ACTIVATION-STAGES: $(grep -o '"capability":"[a-z_]*","stage":"[a-z]*"' "$f" | tr '\n' ' ')"
  rm -rf "$home"
}

log "F27-02 READINESS OBSERVATION"
log "HOST: $(hostname)"
log "OS: $(uname -sr)"
log "SHA: $(cd "$REPO" && git rev-parse HEAD)"
log "BIN: $BIN"
log ""
log "MACHINE FACTS (what this box can actually do):"
log "  DISPLAY=${DISPLAY:-<unset>}  WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-<unset>}"
log "  chromium on PATH: $(command -v chromium chromium-browser google-chrome 2>/dev/null | tr '\n' ' ' || echo NONE)"
log "  camoufox on PATH: $(command -v camoufox 2>/dev/null || echo NONE)"
log "  X server socket:  $(ls /tmp/.X11-unix 2>/dev/null | tr '\n' ' ' || echo NONE)"
log ""

# --- OBSERVATION A: the box as it is. Baseline. ---------------------------
capture baseline

# --- OBSERVATION B: browser backend made unresolvable, ALONE. -------------
# PATH is emptied of everything but the toolchain the binary itself needs, so
# no browser binary can be resolved. Nothing else changes.
PROBE_PATH="/nonexistent-f27-probe-dir" capture no-browser-backend
unset PROBE_PATH

# --- OBSERVATION C: display removed, ALONE. -------------------------------
# The base environment is already display-less (env -i), so this observation
# runs the mirror case: a display ADVERTISED where none exists. If the
# handshake is derived from linkage it will not move either way.
capture display-advertised DISPLAY=:99

# --- OBSERVATION D: cloud credentials removed / present, ALONE. -----------
capture cloud-creds-absent
capture cloud-creds-present BROWSERBASE_API_KEY=f27-not-a-real-key

log ""
log "DELTA: byte-comparison of the ready line across every observation."
for f in "$OUT"/handshake/*.jsonl; do
  h=$(head -1 "$f" | python3 -c 'import hashlib,sys; print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest()[:16])')
  log "  $(basename "$f")  ready-line-sha256[0:16]=$h"
done
log "DONE"
