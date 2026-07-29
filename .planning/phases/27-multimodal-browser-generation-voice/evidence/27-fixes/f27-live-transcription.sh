#!/usr/bin/env bash
# LIVE end-to-end re-measurement of C4's transcription clause THROUGH THE PRODUCT.
# Not curl: this drives the real `wayland-core` binary and observes the resolver
# log line and the tool result.
#
# FLUX_API_KEY is read from the environment. It is never written to disk here,
# never echoed, and never placed in a command line (so it cannot appear in `ps`).
# Every capture is redacted and byte-counted.
#
# Gate discipline (lane brief §3.2): this script runs a NEGATIVE CONTROL with the
# credential removed. If the "tool hidden" path does not fire in the control, the
# positive result proves nothing and the script says so.
set -uo pipefail

BIN="${BIN:?set BIN to the wayland-core binary}"
WAV="${WAV:?set WAV to the positive fixture}"
OUT="${1:?usage: $0 <outdir>}"
mkdir -p "$OUT"
[ -z "${FLUX_API_KEY:-}" ] && { echo "FATAL: FLUX_API_KEY unset" >&2; exit 2; }

redact() { sed -e "s/${FLUX_API_KEY}/<REDACTED_KEY>/g" -e 's/[A-Za-z0-9_-]\{40,\}/<REDACTED_LONGSTRING>/g'; }

# Headless host: without this every turn dies with "Session persistence authority
# unavailable ... no OS keyring was usable" while discovery still looks healthy.
export WAYLAND_VAULT_PASSPHRASE="lane-27-fixes-throwaway-not-a-secret"
export RUST_LOG="${RUST_LOG:-wcore_agent::tool_backends=info,wcore_agent=info}"

CFG="$OUT/config.toml"
cat > "$CFG" <<'TOML'
[default]
provider = "flux-router"
model = "flux-fast"

[providers.flux-router]
base_url = "https://api.fluxrouter.ai/v1"
TOML
echo "config written ($(wc -c < "$CFG" | tr -d ' ') bytes) — contains NO credential:"
sed 's/^/    | /' "$CFG"

# run_turn <label> <prompt>   — ONE turn per invocation. Never queue two
# messages on one stdin: that silently drops the second turn (measured).
run_turn() {
  local label="$1" prompt="$2"
  local o="$OUT/$label.stdout.txt" e="$OUT/$label.stderr.txt"
  timeout 300 "$BIN" --config "$CFG" --force "$prompt" > "$o" 2> "$e" < /dev/null
  local rc=$?
  redact < "$o" > "$OUT/.t" && mv "$OUT/.t" "$o"
  redact < "$e" > "$OUT/.t" && mv "$OUT/.t" "$e"
  echo "== $label rc=$rc stdout_bytes=$(wc -c < "$o"|tr -d ' ') stderr_bytes=$(wc -c < "$e"|tr -d ' ')"
  return $rc
}

PROMPT="Call the transcribe_audio tool on the local file $WAV and reply with ONLY the exact transcript text it returns, nothing else."

echo
echo "############ 1. POSITIVE — credential present ############"
run_turn positive "$PROMPT"
echo "--- resolver log line (the defect's observable) ---"
grep -i 'transcription:' "$OUT/positive.stderr.txt" | head -5
echo "RESOLVER_CHOSE_FLUX=$(grep -c 'transcription: using flux-router' "$OUT/positive.stderr.txt")"
echo "RESOLVER_SAID_HIDDEN=$(grep -c 'transcription: no API key found' "$OUT/positive.stderr.txt")"
echo "--- model reply ---"
sed 's/^/    | /' "$OUT/positive.stdout.txt"
echo "VERBATIM_MATCH=$(grep -ci 'quick brown fox jumps over the lazy dog near the riverbank' "$OUT/positive.stdout.txt")"

echo
echo "############ 2. NEGATIVE CONTROL — credential removed ############"
echo "(if this does NOT report the tool hidden, the positive above proves nothing)"
(
  unset FLUX_API_KEY
  o="$OUT/control.stdout.txt"; e="$OUT/control.stderr.txt"
  timeout 300 "$BIN" --config "$CFG" --force "$PROMPT" > "$o" 2> "$e" < /dev/null
  echo "== control rc=$? stdout_bytes=$(wc -c < "$o"|tr -d ' ') stderr_bytes=$(wc -c < "$e"|tr -d ' ')"
)
echo "--- resolver log line ---"
grep -i 'transcription:' "$OUT/control.stderr.txt" | head -5
echo "CONTROL_SAID_HIDDEN=$(grep -c 'transcription: no API key found' "$OUT/control.stderr.txt")"
echo "CONTROL_CHOSE_FLUX=$(grep -c 'transcription: using flux-router' "$OUT/control.stderr.txt")"

echo
echo "############ VERDICT ############"
pos_flux=$(grep -c 'transcription: using flux-router' "$OUT/positive.stderr.txt")
pos_text=$(grep -ci 'quick brown fox jumps over the lazy dog near the riverbank' "$OUT/positive.stdout.txt")
ctl_hidden=$(grep -c 'transcription: no API key found' "$OUT/control.stderr.txt")
echo "positive_resolved_flux=$pos_flux positive_verbatim=$pos_text control_hidden=$ctl_hidden"
if [ "$pos_flux" -ge 1 ] && [ "$pos_text" -ge 1 ] && [ "$ctl_hidden" -ge 1 ]; then
  echo "LIVE_RESULT=PASS (resolver reached flux, product returned the verbatim transcript, and the control still hides)"
  exit 0
fi
echo "LIVE_RESULT=FAIL_OR_INCONCLUSIVE"
exit 1
