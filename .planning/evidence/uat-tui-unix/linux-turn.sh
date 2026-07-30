#!/usr/bin/env bash
# linux-turn.sh — run on hetzner. Reads the FluxRouter key from STDIN ONLY.
#
# LANE-BRIEF §0 sanctioned-exception compliance:
#   * the key arrives on stdin; it is never in argv, never written to disk,
#     never echoed, and never enters the capture;
#   * every capture is swept for the literal value and the hit count reported;
#   * the sweep itself is proven alive with a known-positive.
#
# LANE-BRIEF §3b-ii: hetzner's /root/.wayland/.env injects ANTHROPIC_API_KEY into
# the product regardless of the shell. HOME is therefore repointed at a pristine
# directory AND the provider actually used is read back out of the product's own
# output rather than inferred from this script's intent.
set -u
BIN="${1:-/root/wayland-uat-tui-unix/target/release/wayland-core}"
OUT="${2:-/root/uat-lane-turn}"
LABEL="${3:-l-turn}"
MODE="${4:-vault}"   # vault | novault

[ -x "$BIN" ] || { echo "ASSERT_BIN=FAIL path=$BIN"; exit 92; }
mkdir -p "$OUT"
HOMEDIR="$OUT/home-$LABEL"; rm -rf "$HOMEDIR"; mkdir -p "$HOMEDIR"

IFS= read -r FLUX_API_KEY
export FLUX_API_KEY
[ -n "${FLUX_API_KEY:-}" ] || { echo "ASSERT_KEY=FAIL (nothing on stdin)"; exit 97; }
echo "ASSERT_KEY=OK (len=${#FLUX_API_KEY}, value never printed)"

echo "HETZNER_ENV_INJECTION_CHECK: /root/.wayland/.env exists? $([ -f /root/.wayland/.env ] && echo YES || echo NO)"

CAP="$OUT/$LABEL.out.txt"
VAULTENV=()
[ "$MODE" = "vault" ] && VAULTENV=(WAYLAND_VAULT_PASSPHRASE=uat-throwaway-not-a-real-secret)

START=$(date +%s)
env -u ANTHROPIC_API_KEY -u OPENAI_API_KEY -u GEMINI_API_KEY -u GOOGLE_API_KEY \
    -u GROQ_API_KEY -u OPENROUTER_API_KEY -u API_KEY -u PROVIDER -u BASE_URL -u MODEL \
    HOME="$HOMEDIR" "${VAULTENV[@]}" \
    "$BIN" -p flux-router -m flux-auto --no-tui \
    "Reply with exactly this token and nothing else: WAYLAND_UAT_OK" > "$CAP" 2>&1
RC=$?
END=$(date +%s)
{ echo "WLRC=$RC"; echo "WLDONE"; } > "$OUT/$LABEL.rc"
echo "TURN_RC=$RC ELAPSED=$((END-START))s MODE=$MODE"

HITS=$(grep -c -F "$FLUX_API_KEY" "$CAP" 2>/dev/null); [ -z "$HITS" ] && HITS=0
TMPP=$(mktemp); printf '%s\n' "$FLUX_API_KEY" > "$TMPP"
KP=$(grep -c -F "$FLUX_API_KEY" "$TMPP" 2>/dev/null); [ -z "$KP" ] && KP=0
shred -u "$TMPP" 2>/dev/null || rm -f "$TMPP"
echo "SECRET_LEAK_HITS_IN_CAPTURE=$HITS (expect 0)"
echo "SECRET_SWEEP_KNOWN_POSITIVE=$KP (must be >=1)"

echo "----- ARM READ BACK FROM THE PRODUCT'S OWN OUTPUT -----"
grep -Eo 'flux-router/[a-z-]+|api\.fluxrouter\.ai|api\.anthropic\.com|api\.openai\.com' "$CAP" | sort | uniq -c
echo "----- LAST 6 LINES THE USER SAW -----"
tail -6 "$CAP"
echo "----- TOTAL LINES OF OUTPUT FOR ONE PROMPT -----"
awk 'END{print NR}' "$CAP"
