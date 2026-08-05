#!/usr/bin/env bash
# run-headless.sh — hetzner-side wrapper. Injects the box's own Anthropic key
# into the child's environment WITHOUT ever printing it, points the product at
# the lane-private home, then defers to noise-headless.sh for the measurement.
#
# usage: run-headless.sh <label> <binary> <outdir> <lanehome> [extra args...]
set -u
LABEL="${1:?}"; BIN="${2:?}"; OUT="${3:?}"; LANEHOME="${4:?}"; shift 4

# Value never leaves this process's environment. Length only is recorded.
KEY=$(/usr/bin/grep -m1 '^ANTHROPIC_API_KEY=' /root/.wayland/.env | cut -d= -f2-)
if [ -z "$KEY" ]; then echo "NO_KEY_IN_ENV_FILE" >&2; exit 97; fi
export ANTHROPIC_API_KEY="$KEY"
mkdir -p "$OUT"
echo "KEY_LEN=${#KEY}" >> "$OUT/$LABEL.keylen"
unset KEY

export HOME="$LANEHOME"
export PROVIDER=anthropic
export MODEL=claude-sonnet-4-5-20250929

exec /root/wayland-fix-tui-noise-harness/noise-headless.sh "$LABEL" "$BIN" "$OUT" "$@"
