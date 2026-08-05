#!/usr/bin/env bash
# F27-C3 — live probe of billable media generation against a real FluxRouter
# account, through the shipped `wayland-core` binary.
#
# CREDENTIAL HANDLING (LANE-BRIEF §0 sanctioned exception):
#   The key arrives on STDIN only. It is exported into the child's environment
#   — which is how the product reads it — and is never placed in argv, never
#   written to any file, never echoed, and never included in any capture. The
#   script runs with `set +x` throughout and every capture is swept afterwards.
#
# PROVIDER SELECTION (LANE-BRIEF §3b-ii):
#   `/root/.wayland/.env` on this host injects ANTHROPIC_API_KEY regardless of
#   what the shell unsets. A previous lane's "live Flux proof" was actually
#   running on Anthropic because of it. So this script does NOT infer the arm
#   from what it exported: every capture includes the resolver's own log line
#   and the caller asserts on THAT.
#
# Usage, from the Mac:
#   ssh hetzner-dsm 'bash /root/wayland-27c3/.../live-probe.sh' \
#       < ~/.wayland-secrets/flux.env

set -u +x

ROOT="${ROOT:-/root/wayland-27c3}"
BIN="$ROOT/target/debug/wayland-core"
OUT="${OUT:-/root/wayland-27c3-live}"
rm -rf "$OUT"; mkdir -p "$OUT"

# --- credential intake, stdin only -----------------------------------------
# The file may be `FLUX_API_KEY=value` or a bare value. Parse without echoing.
# NOTE, and this cost a wasted probe: the file is `export FLUX_API_KEY=...`.
# The first version of this parser anchored on `FLUX_API_KEY` at line start,
# missed the `export ` prefix, fell through to the bare-value branch and sent
# the ENTIRE LINE as the credential. The provider replied
# `401 ... Received=expo****` — i.e. it had been handed the word "export".
# A parser bug is indistinguishable from a dead key at the call site, and
# would have been reported as a credential blocker. Handle both spellings, and
# reject anything that still looks like a shell fragment.
RAW="$(cat)"
KEY="$(printf '%s' "$RAW" | sed -n 's/^[[:space:]]*\(export[[:space:]]\{1,\}\)\{0,1\}FLUX_API_KEY[[:space:]]*=[[:space:]]*//p' | tr -d '"'"'"'\r' | head -1)"
if [ -z "$KEY" ]; then
  KEY="$(printf '%s' "$RAW" | tr -d '\r' | head -1)"
fi
unset RAW
case "$KEY" in
  *=*|*" "*)
    echo "PROBE=ABORT reason=key_still_contains_shell_syntax_after_parse len=${#KEY}"
    exit 2 ;;
esac
if [ -z "$KEY" ]; then
  echo "PROBE=ABORT reason=no_key_on_stdin"
  exit 2
fi
echo "PROBE key_received=yes key_len=${#KEY}   # length only, never the value"

# --- isolated home ----------------------------------------------------------
HOME_DIR="$OUT/home"
mkdir -p "$HOME_DIR"
cat > "$HOME_DIR/config.toml" <<'TOML'
[default]
provider = "flux-router"
model = "flux-fast"

[tools]
auto_approve = true

# Durable sessions need an unlocked vault; this headless isolated profile has
# none, and without this the run dies at
# "Session persistence authority unavailable" BEFORE any turn. That failure
# looked like a probe result on the first attempt and was not one.
[session]
enabled = false
TOML
echo "PROBE config_written=$HOME_DIR/config.toml   # contains NO credential"
grep -c "API_KEY\|sk-\|Bearer" "$HOME_DIR/config.toml" > /dev/null 2>&1
echo "PROBE config_credential_tokens=$(grep -c -E 'API_KEY|sk-|Bearer' "$HOME_DIR/config.toml" || true)"

run_agent() {
  local label="$1" prompt="$2"; shift 2
  echo "=== RUN $label"
  (
    export WAYLAND_HOME="$HOME_DIR"
    export FLUX_API_KEY="$KEY"
    # Strip every other provider credential we can see. This does NOT defeat
    # the /root/.wayland/.env injection — which is exactly why the arm is read
    # back from the product's own output instead of trusted from here.
    unset ANTHROPIC_API_KEY OPENAI_API_KEY GEMINI_API_KEY GOOGLE_API_KEY \
          FAL_API_KEY HF_API_KEY OPENROUTER_API_KEY API_KEY GROQ_API_KEY
    export RUST_LOG="wcore_agent::tool_backends=info,wcore::media_cost=info,wcore_agent::bootstrap=info"
    # NOT --json-stream: with a positional prompt, json-stream mode waits for
    # message frames on stdin and exits after the capability handshake without
    # ever driving a turn. The first version of this probe used it and produced
    # three byte-identical 4506-byte captures with no model turn in any of them
    # — a capture that looks like evidence and contains none.
    "$@" timeout 180 "$BIN" --no-tui --yolo -p flux-router -m flux-fast \
      "$prompt"
  ) > "$OUT/$label.stdout" 2> "$OUT/$label.stderr"
  local rc=$?
  echo "RC_$label=$rc"
  echo "  stdout_bytes=$(wc -c < "$OUT/$label.stdout")  stderr_bytes=$(wc -c < "$OUT/$label.stderr")"
}

# --------------------------------------------------------------------------
# P1 — registration + arm readback. Only FLUX_API_KEY is supplied. If
#      image_generate registers, then the FluxRouter/OpenAI-wire config arm
#      enables image generation — which the honest-unavailable advisory's
#      hint does NOT name. That is measured here rather than read off source.
# --------------------------------------------------------------------------
run_agent p1-arm-readback "Reply with exactly: PROBE_OK. Do not use any tools."
echo "--- P1 resolver arm line (the product's own output):"
grep -h "image_gen: using" "$OUT/p1-arm-readback.stderr" || echo "  (none — image_generate did NOT register)"
echo "--- P1 turn actually ran? (a capture with no turn is not evidence):"
echo "  probe_ok_in_output=$(grep -c "PROBE_OK" "$OUT/p1-arm-readback.stdout" || true)"

# --------------------------------------------------------------------------
# P2 — billable generation with the product's DEFAULT image model in a Flux
#      session. One variable versus P3.
# --------------------------------------------------------------------------
run_agent p2-default-model \
  "Use the image_generate tool right now to generate an image with the prompt 'a small red lighthouse at dusk'. Call the tool. Do not ask permission."
echo "--- P2 accounting blocks found in the protocol stream:"
grep -o '"accounting":{[^}]*}[^}]*}[^}]*}' "$OUT/p2-default-model.stdout" | head -3 || true
grep -c 'accounting' "$OUT/p2-default-model.stdout" || true
echo "--- P2 media_cost tracing lines:"
grep -h "media call accounted" "$OUT/p2-default-model.stderr" | head -3 || echo "  (none)"

# --------------------------------------------------------------------------
# P3 — same call, image model switched to the one this key is entitled to.
# --------------------------------------------------------------------------
run_agent p3-entitled-model \
  "Use the image_generate tool right now to generate an image with the prompt 'a small red lighthouse at dusk'. Call the tool. Do not ask permission." \
  env OPENAI_IMAGE_MODEL=flux-image
echo "--- P3 resolver arm line:"
grep -h "image_gen: using" "$OUT/p3-entitled-model.stderr" || echo "  (none)"
echo "--- P3 media_cost tracing lines:"
grep -h "media call accounted" "$OUT/p3-entitled-model.stderr" | head -3 || echo "  (none)"
echo "--- P3 accounting presence:"
echo "  accounting_occurrences=$(grep -c 'accounting' "$OUT/p3-entitled-model.stdout" || true)"

# --------------------------------------------------------------------------
# P4 — the OTHER billable generation surface: the `image` subcommand, which
#      goes through FluxImageClient, not through the tool. Measured for an
#      accounting record; F27-C3 asks whether accounting is CONSISTENT.
# --------------------------------------------------------------------------
echo "=== RUN p4-image-subcommand"
(
  export WAYLAND_HOME="$HOME_DIR"
  export FLUX_API_KEY="$KEY"
  unset ANTHROPIC_API_KEY OPENAI_API_KEY
  export RUST_LOG="info"
  timeout 180 "$BIN" image --model flux-image \
    --prompt "a small red lighthouse at dusk" --out "$OUT/p4.png"
) > "$OUT/p4-image-subcommand.stdout" 2> "$OUT/p4-image-subcommand.stderr"
echo "RC_p4-image-subcommand=$?"
echo "  artifact_bytes=$(wc -c < "$OUT/p4.png" 2>/dev/null || echo 0)"
echo "--- P4 any cost/accounting token in either stream:"
echo "  accounting_hits=$(cat "$OUT/p4-image-subcommand".std* | grep -c -iE 'accounting|cost_usd|unpriced' || true)"
echo "--- P4 liveness control for that grep (a known-positive in the SAME files):"
# The first version grepped for words that happen not to appear in a successful
# run's 52-byte output, so the control returned 0 and proved nothing. Grep for
# a string the successful run demonstrably writes.
echo "  control_hits_wrote=$(cat "$OUT/p4-image-subcommand".std* | grep -c -E 'wrote|bytes' || true)"
echo "  p4_stderr_verbatim: $(cat "$OUT/p4-image-subcommand.stderr")"

echo "--- P4 accounting lines (present as of the 27-c3 repair):"
grep -h "^accounting" "$OUT/p4-image-subcommand.stderr" || echo "  (none)"

# --------------------------------------------------------------------------
# P5 — the SAME subcommand with two variables moved (n=1 -> 2, size unknown ->
#      1024x1024). The record must change. A record that is identical to P4's
#      is a constant, not a measurement — which is the whole failure this lane
#      exists to avoid.
# --------------------------------------------------------------------------
echo "=== RUN p5-image-subcommand-varied"
(
  export WAYLAND_HOME="$HOME_DIR"
  export FLUX_API_KEY="$KEY"
  unset ANTHROPIC_API_KEY OPENAI_API_KEY
  export RUST_LOG="info"
  timeout 240 "$BIN" image --model flux-image --n 2 --size 1024x1024 \
    --prompt "a small red lighthouse at dusk" --out "$OUT/p5.png"
) > "$OUT/p5-image-subcommand-varied.stdout" 2> "$OUT/p5-image-subcommand-varied.stderr"
echo "RC_p5-image-subcommand-varied=$?"
echo "--- P5 accounting lines:"
grep -h "^accounting" "$OUT/p5-image-subcommand-varied.stderr" || echo "  (none)"
echo "--- P4 vs P5 accounting_json differ?"
P4J="$(grep -h '^accounting_json:' "$OUT/p4-image-subcommand.stderr" || echo P4_MISSING)"
P5J="$(grep -h '^accounting_json:' "$OUT/p5-image-subcommand-varied.stderr" || echo P5_MISSING)"
if [ "$P4J" = "$P5J" ]; then
  echo "  CLI_RECORD_VARIES=NO   <-- the record is a constant; this is a FAILURE"
else
  echo "  CLI_RECORD_VARIES=YES"
fi

unset KEY FLUX_API_KEY
echo
echo "PROBE_DONE captures_in=$OUT"
ls -la "$OUT" | sed -n '1,20p'
