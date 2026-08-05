#!/usr/bin/env bash
# 23B-H1 reproduction harness.
#
# Finding 23B-H1 (raised by 23B-01, confirmed pre-existing against a pristine
# base binary): a `wayland-core` run that EXITS NORMALLY can write a session
# journal the product cannot read back. `--resume` then fails with
# `journal checksum mismatch at sequence N`, and every operator verb that reads
# the journal fails identically, so the session is permanently unreachable.
#
# This harness drives the shipped binary only — no fixtures, no internal APIs —
# and reports the reproduction rate plus a preserved copy of every journal that
# failed, so the failure can be analysed offline.
#
# Contract (shared with the rest of the f23 driver family):
#   --binary <path>   the wayland-core binary to drive
#   --runs <n>        how many independent seed+resume cycles to attempt
#   --out <dir>       where to preserve failing journals and transcripts
#
# Emits one terminal marker:
#   F23_H1_REPRO runs=<n> resume_ok=<n> checksum_mismatch=<n> other_failure=<n>
#
# Exit status is 0 when the harness itself ran to completion. The harness does
# NOT exit non-zero on reproduction: reproducing the defect is its purpose, and
# a non-zero exit would make it unusable as a before/after measurement.

set -uo pipefail

BINARY=""
RUNS=10
OUT=""

while [ $# -gt 0 ]; do
  case "$1" in
    --binary) BINARY="${2:-}"; shift 2 ;;
    --runs)   RUNS="${2:-}";   shift 2 ;;
    --out)    OUT="${2:-}";    shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done

[ -n "$BINARY" ] || { echo "FATAL: --binary is required" >&2; exit 64; }
[ -x "$BINARY" ] || { echo "FATAL: $BINARY is not an executable file" >&2; exit 65; }
[ -n "$OUT" ]    || { echo "FATAL: --out is required" >&2; exit 64; }

BINARY=$(cd "$(dirname "$BINARY")" && pwd)/$(basename "$BINARY")
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)

RUN_DIR=$(mktemp -d)
cleanup() { rm -rf "$RUN_DIR"; }
trap cleanup EXIT

HOME_DIR="$RUN_DIR/home"
SESSIONS="$HOME_DIR/sessions"
mkdir -p "$SESSIONS"

# Placeholder credential assembled at run time so no credential-shaped literal
# is committed. The base_url points at a closed port, so the turn is journalled
# and persisted, the dispatch fails, and the process exits normally.
FAKE_KEY="$(printf 's%s-ant-' k)f23-h1-repro-not-a-real-key-000"
cat > "$HOME_DIR/config.toml" <<EOF
[default]
provider = "anthropic"
model = "claude-3-5-haiku-20241022"

[providers.anthropic]
api_key = "${FAKE_KEY}"
base_url = "http://127.0.0.1:1"
EOF

RESUME_OK=0
CHECKSUM_MISMATCH=0
OTHER_FAILURE=0
SEED_FAILURE=0

i=0
while [ "$i" -lt "$RUNS" ]; do
  i=$((i + 1))
  ID=$(printf 'cccc%08x%08x' "$RANDOM$RANDOM" "$i" | cut -c1-20)
  ID=$(printf '%s' "$ID" | tr -cd '0-9a-f')
  [ ${#ID} -ge 6 ] || ID="cccc0${i}"

  env -u API_KEY -u ANTHROPIC_API_KEY -u OPENAI_API_KEY \
      HOME="$HOME_DIR" WAYLAND_HOME="$HOME_DIR" \
      WAYLAND_VAULT_PASSPHRASE="f23-h1-repro" \
      "$BINARY" --session-id "$ID" --max-turns 1 -- \
      "seed run $i: remember the aardvark" \
      > "$RUN_DIR/seed-$i.txt" 2>&1
  SEED_RC=$?

  if [ ! -f "$SESSIONS/${ID}.journal" ]; then
    SEED_FAILURE=$((SEED_FAILURE + 1))
    echo "F23_H1_RUN=$i id=$ID phase=seed status=NO_JOURNAL seed_exit=$SEED_RC"
    continue
  fi

  RESUME_OUT=$(env -u API_KEY -u ANTHROPIC_API_KEY -u OPENAI_API_KEY \
      HOME="$HOME_DIR" WAYLAND_HOME="$HOME_DIR" \
      WAYLAND_VAULT_PASSPHRASE="f23-h1-repro" \
      "$BINARY" --resume "$ID" --max-turns 1 -- "second message" 2>&1)
  RESUME_RC=$?
  printf '%s\n' "$RESUME_OUT" > "$RUN_DIR/resume-$i.txt"

  if printf '%s\n' "$RESUME_OUT" | grep -qF "journal checksum mismatch"; then
    CHECKSUM_MISMATCH=$((CHECKSUM_MISMATCH + 1))
    SEQ=$(printf '%s\n' "$RESUME_OUT" | sed -n 's/.*journal checksum mismatch at sequence \([0-9]*\).*/\1/p' | head -1)
    cp "$SESSIONS/${ID}.journal" "$OUT/failing-${ID}.journal"
    cp "$RUN_DIR/resume-$i.txt" "$OUT/failing-${ID}-resume.txt"
    echo "F23_H1_RUN=$i id=$ID phase=resume status=CHECKSUM_MISMATCH seq=${SEQ:-unknown} bytes=$(wc -c < "$SESSIONS/${ID}.journal")"
  elif [ "$RESUME_RC" -eq 0 ]; then
    RESUME_OK=$((RESUME_OK + 1))
    echo "F23_H1_RUN=$i id=$ID phase=resume status=OK bytes=$(wc -c < "$SESSIONS/${ID}.journal")"
  else
    # A dispatch failure against the closed port is the EXPECTED shape of a
    # successful read-back: the journal was read, the turn started, the
    # provider call failed. Only a journal-layer refusal counts as a failure
    # to read the session back.
    if printf '%s\n' "$RESUME_OUT" | grep -qE "journal|snapshot|persistence authority"; then
      OTHER_FAILURE=$((OTHER_FAILURE + 1))
      cp "$SESSIONS/${ID}.journal" "$OUT/other-${ID}.journal"
      cp "$RUN_DIR/resume-$i.txt" "$OUT/other-${ID}-resume.txt"
      echo "F23_H1_RUN=$i id=$ID phase=resume status=OTHER_JOURNAL_FAILURE exit=$RESUME_RC"
    else
      RESUME_OK=$((RESUME_OK + 1))
      echo "F23_H1_RUN=$i id=$ID phase=resume status=OK_DISPATCH_FAILED exit=$RESUME_RC bytes=$(wc -c < "$SESSIONS/${ID}.journal")"
    fi
  fi
done

echo "F23_H1_REPRO runs=$RUNS resume_ok=$RESUME_OK checksum_mismatch=$CHECKSUM_MISMATCH other_failure=$OTHER_FAILURE seed_failure=$SEED_FAILURE"
exit 0
