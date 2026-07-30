#!/bin/bash
# Lane fix-headless-keyring live harness.
# argv: <label> <binary> <mode: novault|vault> <prompt>
# The FluxRouter API key arrives on STDIN only. It is never in argv, never
# written to disk, never echoed.
set -u
LABEL="$1"; BIN="$2"; MODE="$3"; PROMPT="$4"
OUT=/root/fixhk-evidence
HOME_DIR="/root/fixhk-home-${LABEL}"
STATUS="${OUT}/${LABEL}.status"
LOG="${OUT}/${LABEL}.log"
rm -rf "$HOME_DIR"; mkdir -p "$HOME_DIR"; chmod 700 "$HOME_DIR"
rm -f "$STATUS" "$LOG"

read -r FLUX_KEY
export FLUX_API_KEY="$FLUX_KEY"
unset FLUX_KEY

export WAYLAND_HOME="$HOME_DIR"
unset WAYLAND_VAULT_PASSPHRASE WAYLAND_VAULT_PASSPHRASE_FD || true
if [ "$MODE" = "vault" ]; then
  export WAYLAND_VAULT_PASSPHRASE="fixhk-throwaway-not-a-real-secret"
fi

{
  echo "LABEL=${LABEL}"
  echo "BIN=${BIN}"
  echo "MODE=${MODE}"
  echo "BIN_SHA256=$(sha256sum "$BIN" | cut -d" " -f1)"
  echo "BUILD_INFO_BEGIN"
  "$BIN" --build-info 2>&1 | sed "s/^/  /"
  echo "BUILD_INFO_END"
  echo "VAULT_PASSPHRASE_SET=$( [ -n "${WAYLAND_VAULT_PASSPHRASE:-}" ] && echo yes || echo no )"
  echo "WAYLAND_HOME=${WAYLAND_HOME}"
  echo "RUN_BEGIN"
} > "$LOG"

timeout 180 "$BIN" -p flux-router -m flux-auto --no-tui "$PROMPT" >> "$LOG" 2>&1
rc=$?

{
  echo ""
  echo "RUN_END"
  echo "SESSION_FILES=$(find "$HOME_DIR" -type f -name "*.json" -path "*session*" 2>/dev/null | wc -l | tr -d " ")"
  echo "SESSION_DIR_LISTING_BEGIN"
  find "$HOME_DIR" -type f 2>/dev/null | sed "s/^/  /"
  echo "SESSION_DIR_LISTING_END"
} >> "$LOG"

echo "WLRC=${rc}" > "$STATUS"
echo "LABEL=${LABEL}" >> "$STATUS"
echo "WLDONE" >> "$STATUS"
