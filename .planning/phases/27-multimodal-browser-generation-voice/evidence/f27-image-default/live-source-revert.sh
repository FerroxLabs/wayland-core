#!/usr/bin/env bash
# F-27C3-04 — LIVE source-revert known-negative.
#
# `live-probe.sh` arm A2 proves `gpt-image-1` fails on a Flux key, but it forces
# that model with an env var. It therefore does NOT prove that MY compat default
# is what makes arm A1 send `flux-image` — something else in the environment
# could be. This script closes that gap the only way it can be closed: it
# reverts the compat default IN SOURCE, rebuilds the real binary, boots it, and
# reads the model back off the product's own resolver line. Then it restores
# from a byte copy, rebuilds, and re-reads.
#
# It uses a NON-GENERATING prompt. The `image_gen: using <model> at <endpoint>`
# line is emitted at tool-registration time, before any generation, so the
# claim is fully observable without spending on two more billable images.
#
# Credential handling and arm readback: identical to live-probe.sh (stdin only,
# never echoed; §3b-ii readback because this host injects ANTHROPIC_API_KEY).

set -u +x
ROOT=/root/wayland-f27imgdef
CARGO=/root/.cargo/bin/cargo
COMPAT=$ROOT/crates/wcore-config/src/compat.rs
BIN="$ROOT/target/debug/wayland-core"
OUT=/root/wayland-f27imgdef-revert
BAK=/root/wayland-f27imgdef-revertbak
rm -rf "$OUT"; mkdir -p "$OUT" "$BAK"
cp "$COMPAT" "$BAK/compat.rs.orig"

RAW="$(cat)"
KEY="$(printf '%s' "$RAW" | sed -n 's/^[[:space:]]*\(export[[:space:]]\{1,\}\)\{0,1\}FLUX_API_KEY[[:space:]]*=[[:space:]]*//p' | tr -d '"'"'"'\r' | head -1)"
[ -z "$KEY" ] && KEY="$(printf '%s' "$RAW" | tr -d '\r' | head -1)"
unset RAW
case "$KEY" in *=*|*" "*) echo "ABORT key_parse"; exit 2 ;; esac
[ -z "$KEY" ] && { echo "ABORT no_key"; exit 2; }
echo "key_received=yes key_len=${#KEY}"

HOME_DIR="$OUT/home"; mkdir -p "$HOME_DIR"
cat > "$HOME_DIR/config.toml" <<'TOML'
[default]
provider = "flux-router"
model = "flux-fast"
[tools]
auto_approve = true
[session]
enabled = false
TOML

boot_and_read() {          # label -> echoes MODEL=<model>
  local label="$1"
  (
    export WAYLAND_HOME="$HOME_DIR"
    export FLUX_API_KEY="$KEY"
    unset ANTHROPIC_API_KEY OPENAI_API_KEY GEMINI_API_KEY GOOGLE_API_KEY \
          FAL_API_KEY HF_API_KEY OPENROUTER_API_KEY API_KEY GROQ_API_KEY \
          OPENAI_IMAGE_MODEL
    export RUST_LOG="wcore_agent::tool_backends=info"
    timeout 120 "$BIN" --no-tui --yolo -p flux-router -m flux-fast \
      "Reply with exactly: PROBE_OK. Do not use any tools."
  ) > "$OUT/$label.stdout" 2> "$OUT/$label.stderr"
  local arm; arm="$(grep -h 'image_gen: using' "$OUT/$label.stderr" | head -1)"
  local m;   m="$(printf '%s' "$arm" | sed -n 's/.*image_gen: using \([^ ]*\) at .*/\1/p')"
  # A run that never booted a turn is not a measurement.
  local turn; turn="$(grep -c 'PROBE_OK' "$OUT/$label.stdout" || true)"
  echo "  ${arm:-(NO RESOLVER LINE)}"
  echo "  ${label}_MODEL=${m:-UNREADABLE}  turn_reached=${turn}"
  printf '%s' "$m" > "$OUT/$label.model"
}

echo "=== STAGE 1 — REVERTED: flux_router_defaults declares no image model"
sed -i 's|^            image_model: Some("flux-image".into()),|            image_model: None,|' "$COMPAT"
if grep -q 'image_model: Some("flux-image"' "$COMPAT"; then
  echo "MUTATION_DID_NOT_APPLY"; cp "$BAK/compat.rs.orig" "$COMPAT"; exit 3
fi
$CARGO build -p wcore-cli 2>&1 | tail -1
boot_and_read reverted

echo "=== STAGE 2 — RESTORED"
cp "$BAK/compat.rs.orig" "$COMPAT"
/usr/bin/diff -q "$BAK/compat.rs.orig" "$COMPAT" >/dev/null && echo "  source restored byte-identical"
$CARGO build -p wcore-cli 2>&1 | tail -1
boot_and_read restored

REV="$(cat "$OUT/reverted.model")"
RES="$(cat "$OUT/restored.model")"
echo
echo "=== VERDICT"
echo "  reverted_model=${REV:-UNREADABLE}  restored_model=${RES:-UNREADABLE}"
if [ "$REV" = "gpt-image-1" ] && [ "$RES" = "flux-image" ]; then
  echo "LIVE_SOURCE_REVERT=PASS  removing the compat default live-restores the defect; restoring it fixes it again"
elif [ -z "$REV" ] || [ -z "$RES" ]; then
  echo "LIVE_SOURCE_REVERT=VOID  a resolver line was unreadable — not a negative result"
else
  echo "LIVE_SOURCE_REVERT=FAIL"
fi
unset KEY FLUX_API_KEY
echo "DONE captures_in=$OUT"
