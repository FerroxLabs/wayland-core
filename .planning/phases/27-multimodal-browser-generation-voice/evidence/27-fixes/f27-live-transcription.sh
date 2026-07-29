#!/usr/bin/env bash
# LIVE end-to-end re-measurement of C4's transcription clause THROUGH THE PRODUCT.
# Not curl: this drives the real `wayland-core` binary and observes both the
# resolver's own log line and the tool result.
#
# FLUX_API_KEY is read from the environment. It is never written to disk here,
# never echoed, and never placed in a command line (so it cannot appear in `ps`).
# Every capture is redacted and byte-counted.
#
# Gate discipline (lane brief §3.2): a NEGATIVE CONTROL runs the SAME binary
# through the SAME function with the credential removed. If the control does not
# report the tool hidden, the positive proves nothing and this script says so.
#
# Driver-defect history (recorded because it nearly produced a false product
# failure): v1 used `--config <file>`. There is no such flag — `--config-path` is
# a BOOLEAN that PRINTS the resolved config path. The positive run therefore
# exited 0 having printed a path and executed no turn, and the control died on an
# arg error. Config is selected by `WAYLAND_HOME` ($WAYLAND_HOME/config.toml).
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
export RUST_LOG="${RUST_LOG:-info}"

WHOME="$OUT/home"; mkdir -p "$WHOME"
cat > "$WHOME/config.toml" <<'TOML'
[default]
provider = "flux-router"
model = "flux-fast"

[providers.flux-router]
base_url = "https://api.fluxrouter.ai/v1"
TOML
echo "config at \$WAYLAND_HOME/config.toml ($(wc -c < "$WHOME/config.toml" | tr -d ' ') bytes) — contains NO credential:"
sed 's/^/    | /' "$WHOME/config.toml"
echo "resolved config path: $(WAYLAND_HOME="$WHOME" "$BIN" --config-path)"

PROMPT="Call the transcribe_audio tool on the local file $WAV and reply with ONLY the exact transcript text it returns, nothing else."

# run_turn <label>   — ONE turn per invocation. Never queue two messages on one
# stdin: that silently drops the second turn (measured on this program).
# The credential is whatever FLUX_API_KEY is in the caller's environment, so the
# control differs from the positive in exactly one variable.
run_turn() {
  local label="$1"
  local o="$OUT/$label.stdout.txt" e="$OUT/$label.stderr.txt"
  WAYLAND_HOME="$WHOME" timeout 300 "$BIN" -p flux-router -m flux-fast --force \
      "$PROMPT" > "$o" 2> "$e" < /dev/null
  local rc=$?
  redact < "$o" > "$OUT/.t" && mv "$OUT/.t" "$o"
  redact < "$e" > "$OUT/.t" && mv "$OUT/.t" "$e"
  echo "== $label rc=$rc stdout_bytes=$(wc -c < "$o"|tr -d ' ') stderr_bytes=$(wc -c < "$e"|tr -d ' ')"
}

# count <pattern> <file>  — greps WITHOUT a pipeline stealing the status, and
# joins wrapped console lines first (a newline inside the searched phrase made a
# sibling lane report absence against a log containing the string four times).
count() { tr -d '\r' < "$2" | tr '\n' ' ' | grep -o -i -- "$1" | wc -l | tr -d ' '; }

echo
echo "############ 1. POSITIVE — credential present ############"
run_turn positive
echo "--- transcription resolver log lines ---"
grep -i 'transcription:' "$OUT/positive.stderr.txt" | head -5
POS_FLUX=$(count 'transcription: using flux-router' "$OUT/positive.stderr.txt")
POS_HIDDEN=$(count 'transcription: no API key found' "$OUT/positive.stderr.txt")
POS_TEXT=$(count 'lazy dog near the riverbank' "$OUT/positive.stdout.txt")
echo "RESOLVER_CHOSE_FLUX=$POS_FLUX  RESOLVER_SAID_HIDDEN=$POS_HIDDEN  VERBATIM_MATCH=$POS_TEXT"
echo "--- model reply ---"
sed 's/^/    | /' "$OUT/positive.stdout.txt"

echo
echo "############ 2. NEGATIVE CONTROL — same binary, same function, no credential ############"
(
  unset FLUX_API_KEY
  redact() { cat; }   # nothing to redact once the key is gone
  run_turn control
)
echo "--- transcription resolver log lines ---"
grep -i 'transcription:' "$OUT/control.stderr.txt" | head -5
CTL_HIDDEN=$(count 'transcription: no API key found' "$OUT/control.stderr.txt")
CTL_FLUX=$(count 'transcription: using flux-router' "$OUT/control.stderr.txt")
echo "CONTROL_SAID_HIDDEN=$CTL_HIDDEN  CONTROL_CHOSE_FLUX=$CTL_FLUX"

echo
echo "############ VERDICT ############"
echo "positive: chose_flux=$POS_FLUX said_hidden=$POS_HIDDEN verbatim=$POS_TEXT"
echo "control : said_hidden=$CTL_HIDDEN chose_flux=$CTL_FLUX"
# All four conditions must hold. In particular the control MUST hide, else the
# positive is unfalsifiable.
if [ "$POS_FLUX" -ge 1 ] && [ "$POS_TEXT" -ge 1 ] && [ "$CTL_HIDDEN" -ge 1 ] && [ "$CTL_FLUX" -eq 0 ]; then
  echo "LIVE_RESULT=PASS"
  exit 0
fi
echo "LIVE_RESULT=FAIL_OR_INCONCLUSIVE"
exit 1
