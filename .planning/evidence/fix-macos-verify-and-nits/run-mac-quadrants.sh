#!/usr/bin/env bash
# run-mac-quadrants.sh — the fix-tui-first-message quadrants, on macOS.
#
# Same five quadrants, same texts, same 0.14 s/char (7.1 chars/sec) human speed,
# same 30 s settle as the Linux run, so the two are directly comparable.
#
# The FluxRouter key is read from ~/.wayland-secrets/flux.env and handed to the
# harness on STDIN ONLY — never in argv, never written to disk, never echoed.
# Only its length is ever recorded.
#
# Usage: run-mac-quadrants.sh <path-to-binary> <out-dir>
set -u

BIN="${1:?usage: run-mac-quadrants.sh <bin> <outdir>}"
OUT="${2:?usage: run-mac-quadrants.sh <bin> <outdir>}"
HERE="$(cd "$(dirname "$0")" && pwd)"
H="$HERE/mac-type.sh"
SEED="$HERE/../fix-tui-first-message/seed-config-no-secret.toml"
HOMEDIR=/tmp/lfmvn-home
mkdir -p "$OUT"

keyfile=~/.wayland-secrets/flux.env
getkey() { sed -n 's/^export FLUX_API_KEY=//p' "$keyfile" | tr -d "\"'" | head -1; }

echo "== Q1: credentials present, no modal expected, 44 chars =="
getkey | "$H" --bin "$BIN" --home "$HOMEDIR" --out "$OUT" --label Q1-keys-present \
  --text "Use the bash tool to run echo SLOWTYPE_TOKEN" --settle 30 --with-key \
  --arg -p --arg flux-router --arg -m --arg flux-auto > "$OUT/Q1.stdout" 2>&1
echo "   rc=$?"

echo "== Q1b: same, the discriminating 20-char case =="
getkey | "$H" --bin "$BIN" --home "$HOMEDIR" --out "$OUT" --label Q1b-keys-marker \
  --text "MARKERSTART_what is two plus two_MARKEREND" --settle 30 --with-key \
  --arg -p --arg flux-router --arg -m --arg flux-auto > "$OUT/Q1b.stdout" 2>&1
echo "   rc=$?"

echo "== Q2/Q3a: credentials ABSENT — the card must still appear (control) =="
"$H" --bin "$BIN" --home "$HOMEDIR" --out "$OUT" --label Q2Q3a-no-keys \
  --text "Use the bash tool to run echo Q3TOKEN" --settle 30 > "$OUT/Q2.stdout" 2>&1
echo "   rc=$?"

echo "== Q3b: modal forced over a resolving config, onboarding completed =="
getkey | "$H" --bin "$BIN" --home "$HOMEDIR" --out "$OUT" --label Q3b-delivered \
  --text "Use the bash tool to run echo Q3DELIVERED" --settle 30 --with-key \
  --seed-config "$SEED" --arg setup \
  --send Down --send Down --send Enter --send Enter > "$OUT/Q3b.stdout" 2>&1
echo "   rc=$?"

echo "== Q5: control — deliberate s then Enter, nothing typed =="
getkey | "$H" --bin "$BIN" --home "$HOMEDIR" --out "$OUT" --label Q5-deliberate-skip-control \
  --text "" --settle 30 --with-key --seed-config "$SEED" --arg setup \
  --send s --send Enter > "$OUT/Q5.stdout" 2>&1
echo "   rc=$?"

echo "ALLDONE"
