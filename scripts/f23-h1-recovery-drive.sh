#!/usr/bin/env bash
# 23B-H1 read-side recovery — drive a REAL wayland-core binary against a journal
# in the pre-fix encoding.
#
# 23B-01 raised the HIGH as a live symptom: `--resume` and every operator verb
# fail with `journal checksum mismatch at sequence N` on a session that still
# exists. 23B-H1 fixed the WRITE path and left the data already on disk
# unreadable. This driver proves the read-side recovery the same way the symptom
# was found — against the shipped binary, not against a test harness.
#
# Two modes, and BOTH have to be run for the evidence to mean anything:
#   --expect unreadable   a binary WITHOUT the recovery must still fail, with
#                         the exact 23B-01 error. This is the red half; without
#                         it the readable half proves nothing.
#   --expect readable     a binary WITH the recovery must read the journal AND
#                         surface the identifiers the journal carried.
#
# The caller generates the nonce at run time and it is planted in the session id
# and in the turn payload, so a stale log from an earlier run cannot satisfy the
# caller's check. `--build-info` is asserted against `--sha` before anything is
# exercised, so a stale binary reddens instead of silently proving old code.
#
# Exit status is the primary gate. The terminal marker
# `F23_H1_DRIVE=PASS platform=<p> mode=<m> nonce=<n>` is the second, independent
# one, and is emitted ONLY after every check passed.
set -uo pipefail

BINARY=""; SHA=""; NONCE=""; EXPECT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --binary) BINARY="$2"; shift 2 ;;
    --sha)    SHA="$2";    shift 2 ;;
    --nonce)  NONCE="$2";  shift 2 ;;
    --expect) EXPECT="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
for required in BINARY SHA NONCE EXPECT; do
  if [ -z "${!required}" ]; then
    echo "missing --$(echo "$required" | tr '[:upper:]' '[:lower:]')" >&2; exit 2
  fi
done
case "$EXPECT" in readable|unreadable) ;; *) echo "--expect must be readable|unreadable" >&2; exit 2 ;; esac
[ -x "$BINARY" ] || { echo "binary is missing or not executable: $BINARY" >&2; exit 2; }

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GEN="$HERE/f23-h1-legacy-journal.py"
[ -f "$GEN" ] || { echo "fixture generator is missing: $GEN" >&2; exit 2; }

case "$(uname -s)" in
  Darwin) PLATFORM=macos ;;
  Linux)  PLATFORM=linux ;;
  *)      PLATFORM="$(uname -s)" ;;
esac

# Provenance FIRST. A binary that does not report the commit under test cannot
# prove anything about it.
BUILD_INFO="$("$BINARY" --build-info 2>&1)" || { echo "--build-info failed: $BUILD_INFO" >&2; exit 3; }
case "$BUILD_INFO" in
  *"$SHA"*) ;;
  *) echo "binary reports '$BUILD_INFO' which does not carry --sha $SHA" >&2; exit 3 ;;
esac
echo "provenance: $BUILD_INFO"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
SESSION="s-$NONCE"

python3 "$GEN" --out "$WORK/$SESSION.journal" --session-id "$SESSION" --nonce "$NONCE" || exit 4
# The fixture must actually carry the defect, or the whole run is vacuous.
grep -q '"effect_receipt":null' "$WORK/$SESSION.journal" \
  || { echo "fixture does not carry the explicit null it exists to carry" >&2; exit 4; }

run_verb() {
  local verb="$1"
  set +e
  OUT="$("$BINARY" session --dir "$WORK" "$verb" "$SESSION" 2>&1)"
  RC=$?
  set -e
  printf '%s\n' "--- session $verb $SESSION (exit $RC)" "$OUT"
}

FAILED=0
fail() { echo "FAIL: $1" >&2; FAILED=1; }

if [ "$EXPECT" = unreadable ]; then
  # Both verbs read the journal directly, so both must reproduce 23B-01's
  # symptom. A zero exit here would mean the binary under test already carries
  # the recovery and the red half is vacuous.
  for verb in reconcile cancel; do
    run_verb "$verb"
    [ "$RC" -ne 0 ] || fail "$verb exited 0 on a pre-fix journal"
    case "$OUT" in
      *"journal checksum mismatch"*) ;;
      *) fail "expected the 23B-01 error from $verb, got: $OUT" ;;
    esac
  done
else
  run_verb reconcile
  [ "$RC" -eq 0 ] || fail "reconcile exited $RC on a journal the recovery must read"
  case "$OUT" in
    *"journal checksum mismatch"*) fail "reconcile still reports the 23B-01 error" ;;
  esac
  # CONTENT, not merely "it opened". The run-time nonce rides in the session id
  # so a stale log cannot satisfy this, and the tool execution id, tool name and
  # turn id exist nowhere but inside the recovered journal.
  case "$OUT" in
    *"F23_SESSION=reconcile id=$SESSION outstanding="*) ;;
    *) fail "reconcile did not surface the recovered session id $SESSION" ;;
  esac
  case "$OUT" in
    *"ref=x1 tool=Write turn=t1"*) ;;
    *) fail "reconcile did not surface the journal's tool execution, tool and turn" ;;
  esac

  # `cancel` reaches the reducer, which refuses while a tool execution is
  # outstanding — exit 5 in `session_cmd`'s documented map. That refusal is
  # itself proof the journal was READ: an unreadable journal never gets this
  # far, it fails with `could not be read` before any reducer state exists.
  run_verb cancel
  [ "$RC" -eq 5 ] || fail "cancel exited $RC, expected the documented 5 (outstanding reconcile)"
  case "$OUT" in
    *"could not be read"*) fail "cancel still cannot read the journal" ;;
    *"outstanding reconcile item"*) ;;
    *) fail "cancel did not report the outstanding item it read from the journal" ;;
  esac

  # Reading is repeatable, not a one-shot side effect of the first open.
  run_verb reconcile
  [ "$RC" -eq 0 ] || fail "the journal stopped being readable on a second pass"
fi

[ "$FAILED" -eq 0 ] || { echo "F23_H1_DRIVE=FAIL platform=$PLATFORM mode=$EXPECT nonce=$NONCE"; exit 1; }
echo "F23_H1_DRIVE=PASS platform=$PLATFORM mode=$EXPECT nonce=$NONCE"
