#!/usr/bin/env bash
# A/B live re-measurement of C4's transcription clause THROUGH THE PRODUCT.
#
# Same credential, same config, same prompt, same host, same minute. The ONLY
# variable is the binary:
#   BASE  = 54203b25 (pre-fix)   -> expected: tool hidden, no transcript
#   FIXED = this lane            -> expected: resolver reaches flux, verbatim transcript
#
# This is the control the first attempt lacked. Unsetting the credential was NOT
# a valid control: the binary then dies at PROVIDER init ("No API key found")
# before the transcription resolver ever runs, so it could not distinguish a
# hidden tool from a dead session.
#
# FLUX_API_KEY comes from the environment; never echoed, never written to disk,
# never in a command line. Every capture is redacted and byte-counted.
set -uo pipefail

BASE_BIN="${BASE_BIN:?}"; FIXED_BIN="${FIXED_BIN:?}"; WAV="${WAV:?}"
OUT="${1:?usage: $0 <outdir>}"; mkdir -p "$OUT"
[ -z "${FLUX_API_KEY:-}" ] && { echo "FATAL: FLUX_API_KEY unset" >&2; exit 2; }

redact() { sed -e "s/${FLUX_API_KEY}/<REDACTED_KEY>/g" -e 's/[A-Za-z0-9_-]\{40,\}/<REDACTED_LONGSTRING>/g'; }

export WAYLAND_VAULT_PASSPHRASE="lane-27-fixes-throwaway-not-a-secret"
export RUST_LOG="${RUST_LOG:-info}"

WHOME="$OUT/home"; mkdir -p "$WHOME"
cat > "$WHOME/config.toml" <<'TOML'
[default]
provider = "flux-router"
model = "flux-fast"

[providers.flux-router]
base_url = "https://api.fluxrouter.ai/v1"
TOML

PROMPT="Call the transcribe_audio tool on the local file $WAV and reply with ONLY the exact transcript text it returns, nothing else."

# Repaired matchers (see resolver-log-matcher-selftest.sh, 3/3 assertions pass).
# Console lines are joined before matching so a wrap cannot hide the phrase, and
# no pipeline is allowed to stand in for a status.
resolved_flux() { tr -d '\r' < "$1" | tr '\n' ' ' | grep -o -i -- 'transcription: using flux-voice-fast at https://[^ ]*/audio/transcriptions' | wc -l | tr -d ' '; }
said_hidden()   { tr -d '\r' < "$1" | tr '\n' ' ' | grep -o -i -- 'transcription: no API key found' | wc -l | tr -d ' '; }
verbatim()      { tr -d '\r' < "$1" | tr '\n' ' ' | grep -o -i -- 'lazy dog near the riverbank' | wc -l | tr -d ' '; }

run_arm() {
  local label="$1" bin="$2"
  local o="$OUT/$label.stdout.txt" e="$OUT/$label.stderr.txt"
  WAYLAND_HOME="$WHOME" timeout 300 "$bin" -p flux-router -m flux-fast --force \
      "$PROMPT" > "$o" 2> "$e" < /dev/null
  local rc=$?
  redact < "$o" > "$OUT/.t" && mv "$OUT/.t" "$o"
  redact < "$e" > "$OUT/.t" && mv "$OUT/.t" "$e"
  echo "== $label rc=$rc stdout_bytes=$(wc -c < "$o"|tr -d ' ') stderr_bytes=$(wc -c < "$e"|tr -d ' ')"
  echo "   resolver lines: $(grep -c -i 'transcription:' "$e" 2>/dev/null || echo 0)"
  grep -i 'transcription:' "$e" 2>/dev/null | sed 's/\x1b\[[0-9;]*m//g' | sed 's/^/     | /' | head -3
}

echo "############ ARM A — BASE binary (pre-fix, 54203b25) ############"
run_arm base "$BASE_BIN"
A_FLUX=$(resolved_flux "$OUT/base.stderr.txt"); A_HID=$(said_hidden "$OUT/base.stderr.txt"); A_TXT=$(verbatim "$OUT/base.stdout.txt")
echo "BASE: resolved_flux=$A_FLUX said_hidden=$A_HID verbatim=$A_TXT"

echo
echo "############ ARM B — FIXED binary (this lane) ############"
run_arm fixed "$FIXED_BIN"
B_FLUX=$(resolved_flux "$OUT/fixed.stderr.txt"); B_HID=$(said_hidden "$OUT/fixed.stderr.txt"); B_TXT=$(verbatim "$OUT/fixed.stdout.txt")
echo "FIXED: resolved_flux=$B_FLUX said_hidden=$B_HID verbatim=$B_TXT"
echo "--- FIXED model reply ---"; sed 's/^/    | /' "$OUT/fixed.stdout.txt"

echo
echo "############ VERDICT ############"
echo "BASE  resolved_flux=$A_FLUX said_hidden=$A_HID verbatim=$A_TXT"
echo "FIXED resolved_flux=$B_FLUX said_hidden=$B_HID verbatim=$B_TXT"
# The gate can fail: it requires the BASE arm to exhibit the defect AND the
# FIXED arm to resolve and transcribe. If BASE already worked, the fix proved
# nothing and this reports FAIL.
if [ "$A_HID" -ge 1 ] && [ "$A_FLUX" -eq 0 ] && [ "$A_TXT" -eq 0 ] \
   && [ "$B_FLUX" -ge 1 ] && [ "$B_HID" -eq 0 ] && [ "$B_TXT" -ge 1 ]; then
  echo "AB_RESULT=PASS (base exhibits the defect; fixed resolves flux and returns the verbatim transcript)"
  exit 0
fi
echo "AB_RESULT=FAIL_OR_INCONCLUSIVE"
exit 1
