#!/usr/bin/env bash
# F-27C3-04 — LIVE source-revert known-negative.  ** v2, REPAIRED **
#
# `live-probe.sh` arm A2 proves `gpt-image-1` fails on a Flux key, but it forces
# that model with an env var, so it does NOT prove that MY compat default is
# what makes arm A1 send `flux-image`. This script closes that gap: it reverts
# the compat default IN SOURCE, rebuilds the real binary, boots it, and reads
# the model back off the product's own resolver line — then restores from a
# byte copy, rebuilds, and re-reads.
#
# It uses a NON-GENERATING prompt: `image_gen: using <model> at <endpoint>` is
# emitted at tool-registration time, before any generation, so the claim is
# fully observable without spending on more billable images.
#
# ---------------------------------------------------------------------------
# INSTRUMENT DEFECT FOUND AND REPAIRED IN THIS LANE (LANE-BRIEF §6b-ii)
#
# v1 of this script never `cd`'d into $ROOT, so `cargo build` died with
# "could not find Cargo.toml in /root" — and BOTH stages then measured the
# STALE binary that was already on disk. Two things made that invisible:
#
#   1. `$CARGO build ... | tail -1` — the pipe steals the exit status (§3.2),
#      and nothing graded the one line it printed.
#   2. Nothing asserted the binary had actually changed.
#
# v1 happened to print FAIL, but that was luck, not detection: it could not
# distinguish "the build failed, so this is VOID" from "the build worked and
# the behaviour did not change, so this is a real FAIL". Those are different
# results and it reported one as the other. Worse, with the arms in the other
# order a stale binary yields a false PASS.
#
# The repair: `cd` into $ROOT; grade the build's real exit status with no pipe
# in the way; and assert the binary's mtime+size MOVED across each rebuild. A
# stage that cannot prove a fresh binary is VOID, never a result.
#
# Run `--self-test` to exercise the repair; see the three assertions there.
# ---------------------------------------------------------------------------

set -u +x
ROOT=/root/wayland-f27imgdef
CARGO=/root/.cargo/bin/cargo
COMPAT=$ROOT/crates/wcore-config/src/compat.rs
BIN="$ROOT/target/debug/wayland-core"
OUT=/root/wayland-f27imgdef-revert
BAK=/root/wayland-f27imgdef-revertbak

# --- the repaired build step ------------------------------------------------
# Returns 0 only when the build EXITED 0 *and* produced a binary whose
# (mtime,size) differs from the stamp captured before it ran. Echoes a reason
# on failure. No pipe anywhere near the exit status.
bin_stamp() { stat -c '%Y:%s' "$BIN" 2>/dev/null || echo "MISSING"; }

rebuild() {                       # -> 0 = fresh binary proven, 1 = VOID
  local before after rc log
  before="$(bin_stamp)"
  log="$OUT/build-$1.log"
  ( cd "$ROOT" && "$CARGO" build -p wcore-cli ) > "$log" 2>&1
  rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "  BUILD_VOID stage=$1 rc=$rc  $(tail -2 "$log" | tr '\n' ' ')"
    return 1
  fi
  after="$(bin_stamp)"
  if [ "$after" = "MISSING" ]; then
    echo "  BUILD_VOID stage=$1 reason=no_binary_on_disk"; return 1
  fi
  if [ "$after" = "$before" ]; then
    echo "  BUILD_VOID stage=$1 reason=binary_unchanged stamp=$after"
    echo "             (a source edit that produces an identical binary means"
    echo "              the edit did not reach this build — measuring it would"
    echo "              measure the previous stage)"
    return 1
  fi
  echo "  build_ok stage=$1 rc=0 stamp ${before} -> ${after}"
  return 0
}

# --- self-test (three assertions, per §6b-ii) -------------------------------
if [ "${1:-}" = "--self-test" ]; then
  mkdir -p "$OUT"
  echo "=== SELF-TEST of the repaired build step"
  P=0; F=0
  ok() { echo "  [OK]   $1"; P=$((P+1)); }
  bad() { echo "  [BAD]  $1"; F=$((F+1)); }

  echo "-- A (known-positive): a real build in the right directory is graded ok"
  # Touch a source file so the binary is genuinely relinked.
  touch "$ROOT/crates/wcore-cli/src/main.rs"
  if rebuild selftest-a; then ok "A: a genuine rebuild passes"; else bad "A: a genuine rebuild was rejected"; fi

  echo "-- B (known-negative): the EXACT v1 defect — build run from the wrong"
  echo "   directory — must be graded VOID, not passed through as a result"
  _REAL_ROOT="$ROOT"; ROOT=/root      # /root has no Cargo.toml: v1's bug verbatim
  if rebuild selftest-b; then bad "B: a failed build was graded as ok"; else ok "B: a failed build is VOID"; fi
  ROOT="$_REAL_ROOT"

  echo "-- C: the OLD (v1) instrument would have MISSED B."
  echo "   v1's build step was:  \$CARGO build -p wcore-cli 2>&1 | tail -1"
  echo "   with no cd, no rc check and no binary-freshness check."
  V1_OUT="$( cd /root && $CARGO build -p wcore-cli 2>&1 | tail -1 )"
  V1_RC=$?
  echo "   v1 observed rc=$V1_RC (the pipe's status, i.e. tail's) and text:"
  echo "     \"$V1_OUT\""
  if [ "$V1_RC" -eq 0 ]; then
    ok "C: v1 saw rc=0 for a build that did NOT happen — it could not have caught B"
  else
    bad "C: v1 surfaced a non-zero status, so the repair is not load-bearing"
  fi

  echo
  echo "SELF_TEST_PASSES=$P SELF_TEST_FAILS=$F"
  [ "$F" -eq 0 ] && [ "$P" -eq 3 ] && echo "INSTRUMENT_SELF_TEST=PASS" || echo "INSTRUMENT_SELF_TEST=FAIL"
  exit 0
fi

# --- main run ---------------------------------------------------------------
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

boot_and_read() {
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
  local arm m turn
  arm="$(grep -h 'image_gen: using' "$OUT/$label.stderr" | head -1)"
  m="$(printf '%s' "$arm" | sed -n 's/.*image_gen: using \([^ ]*\) at .*/\1/p')"
  turn="$(grep -c 'PROBE_OK' "$OUT/$label.stdout" || true)"
  echo "  ${arm:-(NO RESOLVER LINE)}"
  echo "  ${label}_MODEL=${m:-UNREADABLE}  turn_reached=${turn}"
  printf '%s' "$m" > "$OUT/$label.model"
}

VOID=0

echo "=== STAGE 1 — REVERTED: flux_router_defaults declares no image model"
sed -i 's|^            image_model: Some("flux-image".into()),|            image_model: None,|' "$COMPAT"
if grep -q 'image_model: Some("flux-image"' "$COMPAT"; then
  echo "MUTATION_DID_NOT_APPLY"; cp "$BAK/compat.rs.orig" "$COMPAT"; exit 3
fi
rebuild reverted || VOID=1
[ "$VOID" -eq 0 ] && boot_and_read reverted

echo "=== STAGE 2 — RESTORED"
cp "$BAK/compat.rs.orig" "$COMPAT"
/usr/bin/diff -q "$BAK/compat.rs.orig" "$COMPAT" >/dev/null && echo "  source restored byte-identical"
rebuild restored || VOID=1
[ "$VOID" -eq 0 ] && boot_and_read restored

REV="$(cat "$OUT/reverted.model" 2>/dev/null || true)"
RES="$(cat "$OUT/restored.model" 2>/dev/null || true)"
echo
echo "=== VERDICT"
echo "  reverted_model=${REV:-UNREADABLE}  restored_model=${RES:-UNREADABLE}"
if [ "$VOID" -ne 0 ]; then
  echo "LIVE_SOURCE_REVERT=VOID  a stage could not prove a fresh binary; this is NOT a negative result"
elif [ -z "$REV" ] || [ -z "$RES" ]; then
  echo "LIVE_SOURCE_REVERT=VOID  a resolver line was unreadable"
elif [ "$REV" = "gpt-image-1" ] && [ "$RES" = "flux-image" ]; then
  echo "LIVE_SOURCE_REVERT=PASS  removing the compat default live-restores the defect; restoring it fixes it again"
else
  echo "LIVE_SOURCE_REVERT=FAIL"
fi
# Leave the tree as we found it whatever happened.
cp "$BAK/compat.rs.orig" "$COMPAT"
unset KEY FLUX_API_KEY
echo "DONE captures_in=$OUT"
