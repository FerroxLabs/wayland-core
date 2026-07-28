#!/usr/bin/env bash
# 23B-H1 reproduction harness — LIVE PROVIDER variant.
#
# Why this exists. `scripts/f23-h1-repro.sh` points the binary at a closed port
# with a placeholder key. Every run there ends `status=OK_DISPATCH_FAILED`: the
# turn is journalled, the provider call fails, and **no tool event is ever
# recorded**. 23B-H1 is a defect in reading back a journal that contains real
# tool records, so that harness provably cannot reach the code under suspicion.
# 0/12 and 0/34 from it are the evidentiary form of a gate that cannot fail.
#
# This variant drives a REAL provider so the turn reaches a real tool dispatch,
# and — the whole point — it COUNTS the reach instead of assuming it. A run that
# records no tool event is reported in its own bucket (`no_tool_event`) and is
# NOT counted as a non-reproduction.
#
# Credential handling. The key is read from stdin, held in a shell variable, and
# exported only into the child process environment. It is never written to a
# file, never passed in argv, and every transcript this script preserves is
# filtered through a redactor before it is written to --out.
#
#   --binary <path>    the wayland-core binary to drive
#   --runs <n>         independent seed+resume cycles
#   --out <dir>        where to preserve journals and (redacted) transcripts
#   --model <id>       provider model id (default: flux-fast)
#   --version-contains <s>  assert `<binary> --version` contains <s> (stale-binary guard)
#   --key-stdin        REQUIRED. read the bearer key from stdin (first line)
#
# Provenance note. This build's `--version` prints `wayland-core 0.12.25` with no
# source sha, so the version string alone cannot pin a commit. The caller must
# ALSO record the binary's sha256 and the checkout's HEAD; --version-contains is
# only a coarse guard against driving a completely different build.
#
# Terminal marker:
#   F23_H1_LIVE runs=<n> tool_runs=<n> tool_events=<n> no_tool_event=<n> \
#     resume_ok=<n> checksum_mismatch=<n> other_journal_failure=<n> seed_failure=<n>
#
# Exit status is 0 whenever the harness itself completed. Reproducing the defect
# is the harness's purpose, so reproduction must not be an error. Grade the
# markers, never the exit status.

set -uo pipefail

BINARY=""
RUNS=5
OUT=""
MODEL="flux-fast"
WANT_SHA=""
KEY_STDIN=0

while [ $# -gt 0 ]; do
  case "$1" in
    --binary)     BINARY="${2:-}"; shift 2 ;;
    --runs)       RUNS="${2:-}";   shift 2 ;;
    --out)        OUT="${2:-}";    shift 2 ;;
    --model)      MODEL="${2:-}";  shift 2 ;;
    --version-contains) WANT_SHA="${2:-}"; shift 2 ;;
    --key-stdin)  KEY_STDIN=1; shift ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done

[ -n "$BINARY" ] || { echo "FATAL: --binary is required" >&2; exit 64; }
[ -x "$BINARY" ] || { echo "FATAL: $BINARY is not an executable file" >&2; exit 65; }
[ -n "$OUT" ]    || { echo "FATAL: --out is required" >&2; exit 64; }
[ "$KEY_STDIN" -eq 1 ] || { echo "FATAL: --key-stdin is required (no other key path is accepted)" >&2; exit 64; }

IFS= read -r FLUX_KEY || true
[ -n "${FLUX_KEY:-}" ] || { echo "FATAL: empty key on stdin" >&2; exit 66; }
export FLUX_API_KEY="$FLUX_KEY"

# Redactor. Every byte this harness preserves goes through it. Uses a literal
# fixed-string replace via awk so no regex metacharacter in the key can break it.
redact() { awk -v k="$FLUX_KEY" '{ while ((i = index($0, k)) > 0) { $0 = substr($0,1,i-1) "<REDACTED>" substr($0, i+length(k)) } print }'; }

BINARY=$(cd "$(dirname "$BINARY")" && pwd)/$(basename "$BINARY")
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)

# Stale-binary guard. Provenance is asserted before any measurement, because a
# measurement taken against the wrong binary is worse than no measurement.
VER=$("$BINARY" --version 2>&1)
BIN_SHA=$( (sha256sum "$BINARY" 2>/dev/null || shasum -a 256 "$BINARY") | cut -c1-16)
echo "F23_H1_LIVE_BINARY=$VER binary_sha256_16=$BIN_SHA"
if [ -n "$WANT_SHA" ]; then
  case "$VER" in
    *"$WANT_SHA"*) : ;;
    *) echo "FATAL: binary provenance does not contain $WANT_SHA" >&2; exit 3 ;;
  esac
fi

RUN_DIR=$(mktemp -d)
cleanup() { rm -rf "$RUN_DIR"; }
trap cleanup EXIT

HOME_DIR="$RUN_DIR/home"
SESSIONS="$HOME_DIR/sessions"
WORK="$RUN_DIR/work"
mkdir -p "$SESSIONS" "$WORK"

# api_key is deliberately ABSENT from this file. It is resolved from
# $FLUX_API_KEY by wcore-config::resolve_api_key_from_env (ProviderType::FluxRouter).
cat > "$HOME_DIR/config.toml" <<EOF
[default]
provider = "flux-router"
model = "$MODEL"

[providers.flux-router]
base_url = "https://api.fluxrouter.ai/v1"
EOF

RESUME_OK=0
CHECKSUM_MISMATCH=0
OTHER_FAILURE=0
SEED_FAILURE=0
TOOL_RUNS=0
TOOL_EVENTS=0
NO_TOOL_EVENT=0

# Count tool-intent records inside a journal. The journal is length-framed
# binary with plain-JSON bodies, so the serde tag string is present verbatim.
# `grep -a -o` then a line count; `grep -c` counts LINES not OCCURRENCES and
# would under-report when two records share a frame.
count_tool_events() {
  grep -a -o 'tool_intent_recorded' "$1" 2>/dev/null | grep -c . || true
}

i=0
while [ "$i" -lt "$RUNS" ]; do
  i=$((i + 1))
  NONCE=$(od -An -N6 -tx1 /dev/urandom | tr -cd '0-9a-f')
  ID="ee${NONCE}"
  TARGET="$WORK/aardvark-$NONCE.txt"

  ( cd "$WORK" && env -u API_KEY -u ANTHROPIC_API_KEY -u OPENAI_API_KEY \
      HOME="$HOME_DIR" WAYLAND_HOME="$HOME_DIR" \
      WAYLAND_VAULT_PASSPHRASE="f23-h1-live" \
      "$BINARY" --session-id "$ID" --max-turns 4 --max-tokens 4000 \
      --dangerously-skip-permissions -- \
      "Use the Write tool right now to create the file $TARGET containing exactly the text aardvark-$NONCE and nothing else. Do not ask; just call the tool." ) \
      > "$RUN_DIR/seed-$i.txt" 2>&1
  SEED_RC=$?

  JOURNAL="$SESSIONS/${ID}.journal"
  if [ ! -f "$JOURNAL" ]; then
    SEED_FAILURE=$((SEED_FAILURE + 1))
    redact < "$RUN_DIR/seed-$i.txt" > "$OUT/seedfail-${ID}-seed.txt"
    echo "F23_H1_RUN=$i id=$ID phase=seed status=NO_JOURNAL seed_exit=$SEED_RC"
    continue
  fi

  # --- REACH ASSERTION, counted -------------------------------------------
  EV=$(count_tool_events "$JOURNAL")
  FILE_WRITTEN=no
  [ -f "$TARGET" ] && FILE_WRITTEN=yes
  TOOL_EVENTS=$((TOOL_EVENTS + EV))
  if [ "$EV" -gt 0 ]; then
    TOOL_RUNS=$((TOOL_RUNS + 1))
  else
    NO_TOOL_EVENT=$((NO_TOOL_EVENT + 1))
  fi
  echo "F23_H1_REACH=$i id=$ID tool_events=$EV file_written=$FILE_WRITTEN seed_exit=$SEED_RC bytes=$(wc -c < "$JOURNAL" | tr -d ' ')"

  RESUME_OUT=$( cd "$WORK" && env -u API_KEY -u ANTHROPIC_API_KEY -u OPENAI_API_KEY \
      HOME="$HOME_DIR" WAYLAND_HOME="$HOME_DIR" \
      WAYLAND_VAULT_PASSPHRASE="f23-h1-live" \
      "$BINARY" --resume "$ID" --max-turns 1 --max-tokens 4000 \
      --dangerously-skip-permissions -- "reply with the single word ok" 2>&1)
  RESUME_RC=$?
  printf '%s\n' "$RESUME_OUT" | redact > "$RUN_DIR/resume-$i.txt"

  if printf '%s\n' "$RESUME_OUT" | grep -qF "journal checksum mismatch"; then
    CHECKSUM_MISMATCH=$((CHECKSUM_MISMATCH + 1))
    SEQ=$(printf '%s\n' "$RESUME_OUT" | sed -n 's/.*journal checksum mismatch at sequence \([0-9]*\).*/\1/p' | head -1)
    cp "$JOURNAL" "$OUT/failing-${ID}.journal"
    cp "$RUN_DIR/resume-$i.txt" "$OUT/failing-${ID}-resume.txt"
    redact < "$RUN_DIR/seed-$i.txt" > "$OUT/failing-${ID}-seed.txt"
    echo "F23_H1_RUN=$i id=$ID phase=resume status=CHECKSUM_MISMATCH seq=${SEQ:-unknown} tool_events=$EV"
  elif [ "$RESUME_RC" -eq 0 ]; then
    RESUME_OK=$((RESUME_OK + 1))
    echo "F23_H1_RUN=$i id=$ID phase=resume status=OK tool_events=$EV"
  elif printf '%s\n' "$RESUME_OUT" | grep -qE "journal|snapshot|persistence authority"; then
    OTHER_FAILURE=$((OTHER_FAILURE + 1))
    cp "$JOURNAL" "$OUT/other-${ID}.journal"
    cp "$RUN_DIR/resume-$i.txt" "$OUT/other-${ID}-resume.txt"
    echo "F23_H1_RUN=$i id=$ID phase=resume status=OTHER_JOURNAL_FAILURE exit=$RESUME_RC tool_events=$EV"
  else
    RESUME_OK=$((RESUME_OK + 1))
    echo "F23_H1_RUN=$i id=$ID phase=resume status=OK_DISPATCH_FAILED exit=$RESUME_RC tool_events=$EV"
  fi
done

echo "F23_H1_LIVE runs=$RUNS tool_runs=$TOOL_RUNS tool_events=$TOOL_EVENTS no_tool_event=$NO_TOOL_EVENT resume_ok=$RESUME_OK checksum_mismatch=$CHECKSUM_MISMATCH other_journal_failure=$OTHER_FAILURE seed_failure=$SEED_FAILURE"
exit 0
