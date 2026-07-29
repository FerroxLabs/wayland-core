#!/usr/bin/env bash
# F23A-C1-H3 kill-distribution driver.
#
# Kills a real process with SIGKILL at randomised offsets inside a skill restore, then grades
# the user's skill directory as ABSENT / WHOLE / PARTIAL. Run BOTH modes:
#
#   ./kill-23a-c1-atomic.sh atomic 35      # the fix       -> expect PARTIAL = 0
#   ./kill-23a-c1-atomic.sh legacy 35      # the control   -> expect PARTIAL > 0
#
# Exit codes:
#   0  measurement completed and is valid (read the counts; this does NOT mean "pass")
#   2  INVALID MEASUREMENT -- the harness could not observe what it claims to measure
#   3  usage
#
# The exit code deliberately does not encode the verdict. A driver that exits 0 on "PARTIAL=0"
# is one broken pipe away from reporting a pass it never established; the counts are the
# result, and the caller reads them.
set -uo pipefail

MODE="${1:-}"
TRIALS="${2:-35}"
case "$MODE" in
  atomic|legacy) ;;
  *) echo "usage: $0 <atomic|legacy> [trials]" >&2; exit 3 ;;
esac

CARGO="${CARGO:-/root/.cargo/bin/cargo}"
REPO="${REPO:-$(cd "$(dirname "$0")" && git rev-parse --show-toplevel)}"
BIN="$REPO/target/debug/examples/f23a_c1_kill_restore"
WORK="${WORK:-/tmp/f23a-c1-kill-$MODE}"

echo "== building the harness =="
"$CARGO" build -p wcore-skills --example f23a_c1_kill_restore --manifest-path "$REPO/Cargo.toml" \
  2>&1 | tail -3
[ -x "$BIN" ] || { echo "INVALID MEASUREMENT: harness binary missing at $BIN" >&2; exit 2; }

rm -rf "$WORK"; mkdir -p "$WORK"

# ---------------------------------------------------------------------------
# Calibrate. A fixed sleep guesses at a window whose width depends on the disk;
# guessing low means every kill lands before BEGIN and the run is vacuous.
# ---------------------------------------------------------------------------
CAL="$WORK/cal"
mkdir -p "$CAL"
"$BIN" prepare "$CAL" >/dev/null || { echo "INVALID MEASUREMENT: prepare failed" >&2; exit 2; }
CAL_ID="$(cat "$CAL/id.txt")"
T0=$(date +%s%N)
"$BIN" restore "$CAL" "$CAL_ID" "$MODE" >/dev/null || {
  echo "INVALID MEASUREMENT: an uninterrupted restore failed; nothing below is meaningful" >&2
  exit 2; }
T1=$(date +%s%N)
WINDOW_MS=$(( (T1 - T0) / 1000000 ))
[ "$WINDOW_MS" -lt 5 ] && WINDOW_MS=5
echo "== calibrated: an uninterrupted $MODE restore takes ${WINDOW_MS}ms =="

CAL_GRADE=$("$BIN" grade "$CAL")
echo "   uninterrupted control: $CAL_GRADE"
case "$CAL_GRADE" in
  GRADE=WHOLE*) ;;
  *) echo "INVALID MEASUREMENT: an uninterrupted restore did not grade WHOLE ($CAL_GRADE)." >&2
     echo "   The grader disagrees with a known-good state, so every PARTIAL below is suspect." >&2
     exit 2 ;;
esac

ABSENT=0; WHOLE=0; PARTIAL=0; NOT_STARTED=0; COMPLETED=0; IN_WINDOW=0
STAGED=0; RETRY_OK=0; RETRY_FAIL=0
DETAIL="$WORK/trials.tsv"
: > "$DETAIL"

for i in $(seq 1 "$TRIALS"); do
  T="$WORK/t$i"
  rm -rf "$T"; mkdir -p "$T"
  "$BIN" prepare "$T" >/dev/null 2>&1 || { echo "trial $i: prepare failed, skipping"; continue; }
  ID="$(cat "$T/id.txt")"

  # Deterministic per-trial offset spread across the calibrated window, so the run is
  # reproducible and the kills are not all bunched at one point in the copy.
  DELAY_MS=$(( (i * 7919) % (WINDOW_MS + 1) ))

  "$BIN" restore "$T" "$ID" "$MODE" >/dev/null 2>&1 &
  PID=$!
  # `sleep` with a fractional argument; bash has no sub-second builtin.
  sleep "$(awk -v m="$DELAY_MS" 'BEGIN{printf "%.4f", m/1000}')"
  kill -9 "$PID" 2>/dev/null
  wait "$PID" 2>/dev/null

  G=$("$BIN" grade "$T")
  # Unanchored extraction: a leading/indenting wrapper must not lose the field.
  STATE=$(echo "$G"  | grep -o 'GRADE=[A-Z]*'  | head -1 | cut -d= -f2)
  BEGAN=$(echo "$G"  | grep -o 'BEGAN=[01]'    | head -1 | cut -d= -f2)
  DONEF=$(echo "$G"  | grep -o 'DONE=[01]'     | head -1 | cut -d= -f2)
  STAGEDF=$(echo "$G"| grep -o 'STAGED=[01]'   | head -1 | cut -d= -f2)
  printf '%s\t%sms\t%s\n' "$i" "$DELAY_MS" "$G" >> "$DETAIL"

  if [ "${BEGAN:-0}" != "1" ]; then
    NOT_STARTED=$((NOT_STARTED+1))
  elif [ "${DONEF:-0}" = "1" ]; then
    COMPLETED=$((COMPLETED+1))
  else
    IN_WINDOW=$((IN_WINDOW+1))
    case "$STATE" in
      ABSENT)  ABSENT=$((ABSENT+1)) ;;
      WHOLE)   WHOLE=$((WHOLE+1)) ;;
      PARTIAL) PARTIAL=$((PARTIAL+1)) ;;
    esac
    [ "${STAGEDF:-0}" = "1" ] && STAGED=$((STAGED+1))

    # Recovery: a killed restore must be retryable to a whole state without manual repair.
    if "$BIN" restore "$T" "$ID" "$MODE" >/dev/null 2>&1; then
      R=$("$BIN" grade "$T")
      case "$R" in
        GRADE=WHOLE*) RETRY_OK=$((RETRY_OK+1)) ;;
        *) RETRY_FAIL=$((RETRY_FAIL+1)); printf '%s\tRETRY\t%s\n' "$i" "$R" >> "$DETAIL" ;;
      esac
    else
      RETRY_FAIL=$((RETRY_FAIL+1)); printf '%s\tRETRY\tERRORED\n' "$i" >> "$DETAIL"
    fi
  fi
  echo "trial $i/$TRIALS delay=${DELAY_MS}ms $G"
done

echo
echo "===== F23A-C1-H3 KILL DISTRIBUTION: mode=$MODE trials=$TRIALS ====="
echo "window_ms=$WINDOW_MS"
echo "not_started=$NOT_STARTED  completed_before_kill=$COMPLETED  IN_WINDOW=$IN_WINDOW"
echo "of the IN_WINDOW kills:  ABSENT=$ABSENT  WHOLE=$WHOLE  PARTIAL=$PARTIAL"
echo "staging_left=$STAGED  retry_to_whole=$RETRY_OK  retry_failed=$RETRY_FAIL"
echo "per-trial detail: $DETAIL"

if [ "$IN_WINDOW" -eq 0 ]; then
  echo "INVALID MEASUREMENT: no kill landed inside a restore. PARTIAL=$PARTIAL means nothing." >&2
  exit 2
fi
exit 0
