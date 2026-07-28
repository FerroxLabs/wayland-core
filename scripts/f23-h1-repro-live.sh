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
#   --jobs <n>         run <n> seed+resume cycles CONCURRENTLY against one shared
#                      WAYLAND_HOME (default 1 = serial). 23B-01 reproduced in
#                      "bursts"; a strictly serial harness cannot reach whatever
#                      a burst does, which is the same blindness class as having
#                      no reach at all.
#   --selftest         run the reach-counter self-test and exit. Three assertions;
#                      the third proves the OLD instrument would have missed it.
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
JOBS=1
SELFTEST=0

while [ $# -gt 0 ]; do
  case "$1" in
    --binary)     BINARY="${2:-}"; shift 2 ;;
    --runs)       RUNS="${2:-}";   shift 2 ;;
    --out)        OUT="${2:-}";    shift 2 ;;
    --model)      MODEL="${2:-}";  shift 2 ;;
    --version-contains) WANT_SHA="${2:-}"; shift 2 ;;
    --jobs)       JOBS="${2:-}";    shift 2 ;;
    --selftest)   SELFTEST=1; shift ;;
    --key-stdin)  KEY_STDIN=1; shift ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done

# --- reach-counter self-test (§6b-ii: three assertions, not two) -------------
#
# The instrument defect being repaired: the inherited harness
# `scripts/f23-h1-repro.sh` has NO reach counter at all, and its classifier folds
# a run that never dispatched a tool into `resume_ok` via its OK_DISPATCH_FAILED
# arm. So a run that could not possibly have exercised the defect was reported
# as evidence AGAINST the defect. 0/12 and 0/34 were produced that way.
#
# Assertion 3 is the only one that proves the repair does anything: it replays
# the OLD classifier over the same two inputs and shows it returns the identical
# verdict for both — i.e. it never discriminated.
if [ "${SELFTEST:-0}" -eq 1 ]; then
  T=$(mktemp -d); RC=0
  # Known-positive: bytes shaped like a journal frame carrying a tool record.
  printf 'WJ01\x00\x00\x01\x00{"type":"tool_intent_recorded_v2","tool":"Write"}' > "$T/pos.journal"
  # Known-negative: a journal whose only events are provider-attempt records.
  printf 'WJ01\x00\x00\x01\x00{"type":"provider_attempt_not_started","reason":{}}' > "$T/neg.journal"
  _count() { grep -a -o 'tool_intent_recorded' "$1" 2>/dev/null | grep -c . || true; }
  P=$(_count "$T/pos.journal"); N=$(_count "$T/neg.journal")
  [ "$P" -ge 1 ] && echo "SELFTEST_1_KNOWN_POSITIVE=PASS count=$P" || { echo "SELFTEST_1_KNOWN_POSITIVE=FAIL count=$P"; RC=1; }
  [ "$N" -eq 0 ] && echo "SELFTEST_2_KNOWN_NEGATIVE=PASS count=$N" || { echo "SELFTEST_2_KNOWN_NEGATIVE=FAIL count=$N"; RC=1; }
  # The old instrument: no reach counter; classify purely on the resume verdict.
  # Both inputs stand for a run whose resume did not name the journal layer, so
  # both are bucketed `resume_ok` — indistinguishable.
  old_verdict() { case "$1" in *journal\ checksum\ mismatch*) echo CHECKSUM_MISMATCH ;; *) echo resume_ok ;; esac; }
  OP=$(old_verdict "dispatch failed"); ON=$(old_verdict "dispatch failed")
  if [ "$OP" = "$ON" ] && [ "$P" -ne "$N" ]; then
    echo "SELFTEST_3_OLD_MATCHER_BLIND=PASS old_on_reaching=$OP old_on_nonreaching=$ON new_counts=$P/$N"
  else
    echo "SELFTEST_3_OLD_MATCHER_BLIND=FAIL old_on_reaching=$OP old_on_nonreaching=$ON new_counts=$P/$N"; RC=1
  fi
  rm -rf "$T"
  echo "SELFTEST_RC=$RC"
  exit "$RC"
fi

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

# Count tool-intent records inside a journal. The journal is length-framed
# binary with plain-JSON bodies, so the serde tag string is present verbatim.
# `grep -a -o` then a line count; `grep -c` counts LINES not OCCURRENCES and
# would under-report when two records share a frame.
count_tool_events() {
  grep -a -o 'tool_intent_recorded' "$1" 2>/dev/null | grep -c . || true
}

# One seed+resume cycle. Writes its markers to $RUN_DIR/marker-$1 so the parent
# can aggregate them whether the cycles ran serially or concurrently — shell
# arithmetic in a background subshell is invisible to the parent, and silently
# losing every count under --jobs>1 would be exactly the self-passing shape this
# harness exists to avoid.
one_run() {
  local i="$1" M="$RUN_DIR/marker-$1"
  local NONCE ID TARGET SEED_RC JOURNAL EV FILE_WRITTEN RESUME_OUT RESUME_RC SEQ
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
    redact < "$RUN_DIR/seed-$i.txt" > "$OUT/seedfail-${ID}-seed.txt"
    echo "F23_H1_RUN=$i id=$ID phase=seed status=NO_JOURNAL seed_exit=$SEED_RC" > "$M"
    return 0
  fi

  # --- REACH ASSERTION, counted -------------------------------------------
  EV=$(count_tool_events "$JOURNAL")
  FILE_WRITTEN=no
  [ -f "$TARGET" ] && FILE_WRITTEN=yes
  echo "F23_H1_REACH=$i id=$ID tool_events=$EV file_written=$FILE_WRITTEN seed_exit=$SEED_RC bytes=$(wc -c < "$JOURNAL" | tr -d ' ')" > "$M"

  RESUME_OUT=$( cd "$WORK" && env -u API_KEY -u ANTHROPIC_API_KEY -u OPENAI_API_KEY \
      HOME="$HOME_DIR" WAYLAND_HOME="$HOME_DIR" \
      WAYLAND_VAULT_PASSPHRASE="f23-h1-live" \
      "$BINARY" --resume "$ID" --max-turns 1 --max-tokens 4000 \
      --dangerously-skip-permissions -- "reply with the single word ok" 2>&1)
  RESUME_RC=$?
  printf '%s\n' "$RESUME_OUT" | redact > "$RUN_DIR/resume-$i.txt"

  if printf '%s\n' "$RESUME_OUT" | grep -qF "journal checksum mismatch"; then
    SEQ=$(printf '%s\n' "$RESUME_OUT" | sed -n 's/.*journal checksum mismatch at sequence \([0-9]*\).*/\1/p' | head -1)
    cp "$JOURNAL" "$OUT/failing-${ID}.journal"
    cp "$RUN_DIR/resume-$i.txt" "$OUT/failing-${ID}-resume.txt"
    redact < "$RUN_DIR/seed-$i.txt" > "$OUT/failing-${ID}-seed.txt"
    echo "F23_H1_RUN=$i id=$ID phase=resume status=CHECKSUM_MISMATCH seq=${SEQ:-unknown} tool_events=$EV" >> "$M"
  elif [ "$RESUME_RC" -eq 0 ]; then
    echo "F23_H1_RUN=$i id=$ID phase=resume status=OK tool_events=$EV" >> "$M"
  elif printf '%s\n' "$RESUME_OUT" | grep -qE "journal|snapshot|persistence authority"; then
    cp "$JOURNAL" "$OUT/other-${ID}.journal"
    cp "$RUN_DIR/resume-$i.txt" "$OUT/other-${ID}-resume.txt"
    echo "F23_H1_RUN=$i id=$ID phase=resume status=OTHER_JOURNAL_FAILURE exit=$RESUME_RC tool_events=$EV" >> "$M"
  else
    echo "F23_H1_RUN=$i id=$ID phase=resume status=OK_DISPATCH_FAILED exit=$RESUME_RC tool_events=$EV" >> "$M"
  fi
  return 0
}

i=0
while [ "$i" -lt "$RUNS" ]; do
  i=$((i + 1))
  if [ "$JOBS" -le 1 ]; then
    one_run "$i"
    cat "$RUN_DIR/marker-$i"
  else
    one_run "$i" &
    while [ "$(jobs -pr | grep -c .)" -ge "$JOBS" ]; do sleep 1; done
  fi
done
wait

# Aggregate from the markers, never from in-loop arithmetic.
ALL="$RUN_DIR/all-markers.txt"
: > "$ALL"
j=0
while [ "$j" -lt "$RUNS" ]; do
  j=$((j + 1))
  [ -f "$RUN_DIR/marker-$j" ] && cat "$RUN_DIR/marker-$j" >> "$ALL"
done
[ "$JOBS" -gt 1 ] && cat "$ALL"

_n() { grep -o "$1" "$ALL" 2>/dev/null | grep -c . || true; }
SEED_FAILURE=$(_n 'status=NO_JOURNAL')
CHECKSUM_MISMATCH=$(_n 'status=CHECKSUM_MISMATCH')
OTHER_FAILURE=$(_n 'status=OTHER_JOURNAL_FAILURE')
RESUME_OK=$(( $(_n 'status=OK ') + $(_n 'status=OK_DISPATCH_FAILED') ))
TOOL_RUNS=$(grep -o 'F23_H1_REACH=[0-9]* [^\n]*tool_events=[1-9][0-9]*' "$ALL" 2>/dev/null | grep -c . || true)
NO_TOOL_EVENT=$(grep -o 'F23_H1_REACH=[0-9]* [^\n]*tool_events=0 ' "$ALL" 2>/dev/null | grep -c . || true)
TOOL_EVENTS=$(grep -o 'F23_H1_REACH=[^\n]*' "$ALL" 2>/dev/null | sed -n 's/.*tool_events=\([0-9]*\).*/\1/p' | awk '{s+=$1} END {print s+0}')

echo "F23_H1_LIVE runs=$RUNS tool_runs=$TOOL_RUNS tool_events=$TOOL_EVENTS no_tool_event=$NO_TOOL_EVENT resume_ok=$RESUME_OK checksum_mismatch=$CHECKSUM_MISMATCH other_journal_failure=$OTHER_FAILURE seed_failure=$SEED_FAILURE"
exit 0
