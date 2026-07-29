#!/usr/bin/env bash
# F-27C3-04 — live proof against the real billable FluxRouter account, through
# the shipped `wayland-core` binary.
#
# THE CLAIM UNDER TEST: a FluxRouter session with **no OPENAI_IMAGE_MODEL set**
# now generates an image. That "no env var" condition is the entire defect, so
# arm A1 must NOT export it, and the model actually sent must be read back from
# the product's own resolver log — never inferred from what this script set.
#
# CREDENTIAL HANDLING (LANE-BRIEF §0 sanctioned exception): the key arrives on
# STDIN only, is exported into the child's environment (how the product reads
# it), never appears in argv, is never written to a file, never echoed, never
# in a capture. Swept afterwards with a liveness control.
#
# PROVIDER SELECTION (LANE-BRIEF §3b-ii): /root/.wayland/.env on this host
# injects ANTHROPIC_API_KEY into every process regardless of what is unset. A
# previous lane's "live Flux proof" was actually running on Anthropic. So the
# arm is asserted from the product's `image_gen: using <model> at <endpoint>`
# line, not from this script's environment setup.
#
# KEY PARSER: the secrets file is `export FLUX_API_KEY=...`. A parser that
# misses the `export ` prefix hands the provider the whole shell line and the
# provider replies 401 — indistinguishable from a dead key, and one step from a
# falsely-reported credential blocker. Inherited verbatim from 27-c3-media,
# including the post-parse reject.
#
# Usage from the Mac:
#   ssh hetzner-dsm 'bash /root/wayland-f27imgdef-live-probe.sh' \
#       < ~/.wayland-secrets/flux.env

set -u +x

ROOT="${ROOT:-/root/wayland-f27imgdef}"
BIN="$ROOT/target/debug/wayland-core"
# Lane-scoped output path: /tmp and /root are shared between lanes and an
# over-broad glob has already produced one false red on this program.
OUT="${OUT:-/root/wayland-f27imgdef-live}"
rm -rf "$OUT"; mkdir -p "$OUT"

RAW="$(cat)"
KEY="$(printf '%s' "$RAW" | sed -n 's/^[[:space:]]*\(export[[:space:]]\{1,\}\)\{0,1\}FLUX_API_KEY[[:space:]]*=[[:space:]]*//p' | tr -d '"'"'"'\r' | head -1)"
if [ -z "$KEY" ]; then KEY="$(printf '%s' "$RAW" | tr -d '\r' | head -1)"; fi
unset RAW
case "$KEY" in
  *=*|*" "*) echo "PROBE=ABORT reason=key_still_contains_shell_syntax_after_parse len=${#KEY}"; exit 2 ;;
esac
[ -z "$KEY" ] && { echo "PROBE=ABORT reason=no_key_on_stdin"; exit 2; }
echo "PROBE key_received=yes key_len=${#KEY}   # length only, never the value"

HOME_DIR="$OUT/home"
mkdir -p "$HOME_DIR"
cat > "$HOME_DIR/config.toml" <<'TOML'
[default]
provider = "flux-router"
model = "flux-fast"

[tools]
auto_approve = true

# Durable sessions need an unlocked vault; a headless isolated profile has
# none, and without this the run dies at "Session persistence authority
# unavailable" BEFORE any turn — a failure that looks like a probe result.
[session]
enabled = false
TOML
echo "PROBE config_credential_tokens=$(grep -c -E 'API_KEY|sk-|Bearer' "$HOME_DIR/config.toml" || true)   # must be 0"

run_agent() {
  local label="$1" prompt="$2"; shift 2
  echo "=== RUN $label"
  (
    export WAYLAND_HOME="$HOME_DIR"
    export FLUX_API_KEY="$KEY"
    # Strip every other provider credential we can see. This does NOT defeat
    # the /root/.wayland/.env injection — which is why the arm is read back
    # from the product's own output instead of trusted from here.
    unset ANTHROPIC_API_KEY OPENAI_API_KEY GEMINI_API_KEY GOOGLE_API_KEY \
          FAL_API_KEY HF_API_KEY OPENROUTER_API_KEY API_KEY GROQ_API_KEY \
          OPENAI_IMAGE_MODEL
    export RUST_LOG="wcore_agent::tool_backends=info,wcore::media_cost=info"
    # NOT --json-stream: with a positional prompt it exits after the capability
    # handshake without driving a turn (measured by 27-c3-media).
    "$@" timeout 240 "$BIN" --no-tui --yolo -p flux-router -m flux-fast "$prompt"
  ) > "$OUT/$label.stdout" 2> "$OUT/$label.stderr"
  echo "RC_$label=$?"
  echo "  stdout_bytes=$(wc -c < "$OUT/$label.stdout")  stderr_bytes=$(wc -c < "$OUT/$label.stderr")"
}

GEN_PROMPT="Use the image_generate tool right now to generate an image with the prompt 'a small red lighthouse at dusk'. Call the tool. Do not ask permission."

# ---------------------------------------------------------------------------
# A1 — THE CLAIM. FluxRouter, DEFAULT everything, OPENAI_IMAGE_MODEL UNSET.
# ---------------------------------------------------------------------------
run_agent a1-flux-default "$GEN_PROMPT"
echo "--- A1 arm readback (the product's own output — LANE-BRIEF §3b-ii):"
A1_ARM="$(grep -h "image_gen: using" "$OUT/a1-flux-default.stderr" | head -1)"
echo "  ${A1_ARM:-(NONE — image_generate did not register)}"
A1_MODEL="$(printf '%s' "$A1_ARM" | sed -n 's/.*image_gen: using \([^ ]*\) at .*/\1/p')"
A1_ENDPOINT="$(printf '%s' "$A1_ARM" | sed -n 's/.*image_gen: using [^ ]* at \([^ ]*\).*/\1/p')"
echo "  A1_MODEL_SENT=${A1_MODEL:-UNREADABLE}"
echo "  A1_ENDPOINT=${A1_ENDPOINT:-UNREADABLE}"
echo "--- A1 media_cost record (the product's own accounting line):"
grep -h "media call accounted" "$OUT/a1-flux-default.stderr" | head -2 || echo "  (none)"
A1_OK="$(grep -c '"status":"ok"' "$OUT/a1-flux-default.stderr" || true)"
A1_FAILED="$(grep -c 'call_failed_billing_unknown' "$OUT/a1-flux-default.stderr" || true)"
echo "  A1_OUTCOME_OK_HITS=$A1_OK  A1_OUTCOME_FAILED_HITS=$A1_FAILED"
echo "--- A1 image artifact in the tool result? (data URI or url):"
echo "  A1_IMAGE_HITS=$(grep -c -E 'data:image/|"image"' "$OUT/a1-flux-default.stdout" || true)"

# ---------------------------------------------------------------------------
# A2 — KNOWN-NEGATIVE. Force the PRE-FIX model back with the env override, one
#      variable, everything else identical. The old failure must return. A fix
#      whose failure mode cannot be reproduced on demand is asserted, not
#      measured.
# ---------------------------------------------------------------------------
run_agent a2-known-negative-gpt-image-1 "$GEN_PROMPT" env OPENAI_IMAGE_MODEL=gpt-image-1
echo "--- A2 arm readback:"
A2_ARM="$(grep -h "image_gen: using" "$OUT/a2-known-negative-gpt-image-1.stderr" | head -1)"
echo "  ${A2_ARM:-(NONE)}"
A2_MODEL="$(printf '%s' "$A2_ARM" | sed -n 's/.*image_gen: using \([^ ]*\) at .*/\1/p')"
echo "  A2_MODEL_SENT=${A2_MODEL:-UNREADABLE}"
grep -h "media call accounted" "$OUT/a2-known-negative-gpt-image-1.stderr" | head -2 || echo "  (none)"
A2_OK="$(grep -c '"status":"ok"' "$OUT/a2-known-negative-gpt-image-1.stderr" || true)"
A2_FAILED="$(grep -c 'call_failed_billing_unknown' "$OUT/a2-known-negative-gpt-image-1.stderr" || true)"
echo "  A2_OUTCOME_OK_HITS=$A2_OK  A2_OUTCOME_FAILED_HITS=$A2_FAILED"

# ---------------------------------------------------------------------------
# VERDICT — graded on the model READ BACK FROM THE PRODUCT, plus a differential.
# ---------------------------------------------------------------------------
echo
echo "=== VERDICT"
if [ -z "$A1_MODEL" ]; then
  echo "LIVE_A1=UNREADABLE   # no resolver line: the run is VOID, not a negative"
elif [ "$A1_MODEL" = "flux-image" ]; then
  echo "LIVE_A1_MODEL=PASS   sent=$A1_MODEL with OPENAI_IMAGE_MODEL unset"
else
  echo "LIVE_A1_MODEL=FAIL   sent=$A1_MODEL (expected flux-image)"
fi
if [ "$A1_OK" -ge 1 ] && [ "$A1_FAILED" -eq 0 ]; then
  echo "LIVE_A1_OUTCOME=PASS  the default Flux session generated an image"
else
  echo "LIVE_A1_OUTCOME=FAIL  ok_hits=$A1_OK failed_hits=$A1_FAILED"
fi
if [ "$A2_MODEL" = "gpt-image-1" ] && [ "$A2_FAILED" -ge 1 ]; then
  echo "LIVE_A2_KNOWN_NEGATIVE=PASS  the pre-fix model still fails on this key"
else
  echo "LIVE_A2_KNOWN_NEGATIVE=INCONCLUSIVE  model=$A2_MODEL failed_hits=$A2_FAILED ok_hits=$A2_OK"
fi
# Differential: if both arms produce the SAME model, the env override is not
# the only variable and neither arm means anything.
if [ -n "$A1_MODEL" ] && [ "$A1_MODEL" != "${A2_MODEL:-}" ]; then
  echo "LIVE_ARMS_DIFFER=YES  ($A1_MODEL vs ${A2_MODEL:-UNREADABLE})"
else
  echo "LIVE_ARMS_DIFFER=NO   <-- the two arms are not distinguishable; VOID"
fi

unset KEY FLUX_API_KEY
echo
echo "PROBE_DONE captures_in=$OUT"
