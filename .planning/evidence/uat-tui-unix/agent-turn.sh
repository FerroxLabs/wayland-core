#!/usr/bin/env bash
# agent-turn.sh — drive a REAL provider turn and prove which provider served it.
#
# LANE-BRIEF §3b-ii: never infer the arm from what you exported. hetzner injects
# ANTHROPIC_API_KEY from /root/.wayland/.env regardless of the shell, so a run that
# "should" be on Flux can silently be on Anthropic. Every competing key is unset
# here AND the selected model/provider is read back out of the product's output.
#
# Secret handling: the key is sourced from the env file into the process env only.
# It never appears in argv, in this script, or in any capture. Each capture is
# swept for the literal value afterwards and the hit count is reported (expect 0).
set -u
BIN="$1"; SECRETS="$2"; OUT="$3"; LABEL="$4"; shift 4
PROMPT="${1:-Reply with exactly this token and nothing else: WAYLAND_UAT_OK}"

[ -x "$BIN" ]     || { echo "ASSERT_BIN=FAIL path=$BIN"; exit 92; }
[ -r "$SECRETS" ] || { echo "ASSERT_SECRETS=FAIL path=$SECRETS"; exit 96; }
mkdir -p "$OUT"
HOMEDIR="$OUT/home-$LABEL"; rm -rf "$HOMEDIR"; mkdir -p "$HOMEDIR"

set -a; . "$SECRETS"; set +a
[ -n "${FLUX_API_KEY:-}" ] || { echo "ASSERT_KEY=FAIL (FLUX_API_KEY empty after sourcing)"; exit 97; }
echo "ASSERT_KEY=OK (len=${#FLUX_API_KEY}, value never printed)"

CAP="$OUT/$LABEL.out.txt"
START=$(date +%s)
env -u ANTHROPIC_API_KEY -u OPENAI_API_KEY -u GEMINI_API_KEY -u GOOGLE_API_KEY \
    -u GROQ_API_KEY -u OPENROUTER_API_KEY -u API_KEY -u PROVIDER -u BASE_URL -u MODEL \
    HOME="$HOMEDIR" \
    "$BIN" -p flux-router -m flux-auto --no-tui "$PROMPT" > "$CAP" 2>&1
RC=$?
END=$(date +%s)
echo "WLRC=$RC" > "$OUT/$LABEL.rc"; echo "WLDONE" >> "$OUT/$LABEL.rc"
echo "TURN_RC=$RC ELAPSED=$((END-START))s"

# Secret sweep — report the count, never the value.
HITS=$(grep -c -F "$FLUX_API_KEY" "$CAP" 2>/dev/null); [ -z "$HITS" ] && HITS=0
echo "SECRET_LEAK_HITS_IN_CAPTURE=$HITS (expect 0)"
# Prove the sweep is alive: it must find the value in a file that really contains it.
TMPP=$(mktemp); printf '%s\n' "$FLUX_API_KEY" > "$TMPP"
KP=$(grep -c -F "$FLUX_API_KEY" "$TMPP" 2>/dev/null); [ -z "$KP" ] && KP=0
rm -f "$TMPP"
echo "SECRET_SWEEP_KNOWN_POSITIVE=$KP (must be >=1, else SECRET_LEAK_HITS=0 is meaningless)"

echo "----- WHAT THE USER SAW -----"
cat "$CAP"
