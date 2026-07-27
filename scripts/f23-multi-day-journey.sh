#!/usr/bin/env bash
# F23-05 Task 2 — the self-recording multi-day wait / resume / complete journey.
#
# Invoked ONCE to start the journey and ONCE per subsequent calendar day to
# resume it. Between days the process genuinely does not exist: this script
# exits and nothing of it remains resident.
#
#   --binary  <path>    the shipped wayland-core binary; its --build-info source
#                       SHA must equal --sha on EVERY invocation, so a multi-day
#                       journey cannot silently switch binaries mid-span
#   --sha     <commit>  the commit under test
#   --nonce   <hex>     caller-generated; planted on day one, echoed in the PASS
#   --harness <path>    the compiled multi_day_journey_test binary
#   --root    <dir>     journey state root; persists across days
#   --span-seconds <n>  the wait's condition: real elapsed seconds
#   --day     <n>       1 opens the journey; 2.. resume it
#   --verify            re-observe the finished journey and emit the platform
#                       gate's PASS marker
#
# The elapsed span is evidenced by the RUN LOG'S OWN first and last timestamps,
# recomputed here, never by this script's claim. A span shorter than the
# authorized threshold in 23B-04-CLOCK-DECISION.md FAILS, so a journey run back
# to back in one afternoon cannot be reported as multi-day.
#
# Gate discipline: no check here is a pipeline into a filter. Every command's
# exit status is captured on the line AFTER it runs and asserted on directly.

set -uo pipefail

BINARY=""
SHA=""
NONCE=""
HARNESS=""
ROOT=""
SPAN_SECONDS=""
DAY=""
VERIFY=0

while [ $# -gt 0 ]; do
  case "$1" in
    --binary)        BINARY="${2:-}";       shift 2 ;;
    --sha)           SHA="${2:-}";          shift 2 ;;
    --nonce)         NONCE="${2:-}";        shift 2 ;;
    --harness)       HARNESS="${2:-}";      shift 2 ;;
    --root)          ROOT="${2:-}";         shift 2 ;;
    --span-seconds)  SPAN_SECONDS="${2:-}"; shift 2 ;;
    --day)           DAY="${2:-}";          shift 2 ;;
    --verify)        VERIFY=1;              shift 1 ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done

[ -n "$BINARY" ] || { echo "FATAL: --binary is required" >&2; exit 64; }
[ -n "$SHA" ]    || { echo "FATAL: --sha is required" >&2; exit 64; }
[ -n "$NONCE" ]  || { echo "FATAL: --nonce is required" >&2; exit 64; }
[ -x "$BINARY" ] || { echo "FATAL: $BINARY is not an executable file" >&2; exit 65; }

BINARY=$(cd "$(dirname "$BINARY")" && pwd)/$(basename "$BINARY")
REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

case "$(uname -s)" in
  Linux)  PLATFORM=linux ;;
  Darwin) PLATFORM=macos ;;
  *) echo "FATAL: unsupported platform $(uname -s); Windows runs the .ps1 port" >&2; exit 66 ;;
esac

[ -n "$ROOT" ] || ROOT="$HOME/.f23-journey-$PLATFORM"
RUNLOG="$ROOT/runlog.txt"
DECISION="$REPO/.planning/phases/23B-continuous-agency/23B-04-CLOCK-DECISION.md"

# ── Provenance, asserted on EVERY invocation ─────────────────────────────────
BUILD_INFO=$("$BINARY" --build-info 2>&1)
rc=$?
if [ "$rc" -ne 0 ]; then
  echo "FATAL: --build-info exited $rc: $BUILD_INFO" >&2
  exit 67
fi
BIN_SHA=$(printf '%s\n' "$BUILD_INFO" | sed -n 's/.*(source \([0-9a-f]*\)).*/\1/p')
if [ "$BIN_SHA" != "$SHA" ]; then
  echo "FATAL: binary source SHA '$BIN_SHA' != commit under test '$SHA'" >&2
  exit 68
fi
echo "F23_04_PROVENANCE=ok platform=$PLATFORM sha=$SHA"

# ── The authorized real-span threshold for THIS platform ────────────────────
# Read from the decision record, which Task 1 wrote. Absent or unreadable is a
# refusal, never a default: a journey with no authorized threshold has nothing
# to be measured against.
read_required_span() {
  if [ ! -f "$DECISION" ]; then
    echo ""
    return
  fi
  grep -oE "^${PLATFORM}_required_real_span_seconds=[0-9]+" "$DECISION" | tail -1 | cut -d= -f2
}
REQUIRED_SPAN=$(read_required_span)

# The span the journey's wait condition uses. Defaults to the authorized
# threshold so the two cannot drift apart.
if [ -z "$SPAN_SECONDS" ]; then
  SPAN_SECONDS="$REQUIRED_SPAN"
fi
if [ -z "$SPAN_SECONDS" ]; then
  echo "FATAL: no --span-seconds and no ${PLATFORM}_required_real_span_seconds= in $DECISION" >&2
  exit 70
fi

# ── Resolve the harness ─────────────────────────────────────────────────────
if [ -z "$HARNESS" ]; then
  BUILD_LOG=$(mktemp)
  ( cd "$REPO" && cargo test -p wcore-agent --test multi_day_journey_test --no-run --message-format=json ) > "$BUILD_LOG" 2>/dev/null
  rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "FATAL: could not build the journey harness (cargo exited $rc)" >&2
    rm -f "$BUILD_LOG"
    exit 69
  fi
  HARNESS=$(sed -n 's/.*"executable":"\([^"]*multi_day_journey_test[^"]*\)".*/\1/p' "$BUILD_LOG" | tail -1)
  rm -f "$BUILD_LOG"
fi
if [ ! -x "$HARNESS" ]; then
  echo "FATAL: journey harness '$HARNESS' is not executable" >&2
  exit 69
fi

mkdir -p "$ROOT"
touch "$RUNLOG"

# Epoch seconds from an RFC3339 UTC stamp, on GNU and BSD date alike.
epoch_of() {
  local ts="$1" out
  out=$(date -u -d "$ts" +%s 2>/dev/null)
  if [ -n "$out" ]; then printf '%s' "$out"; return 0; fi
  out=$(date -u -j -f '%Y-%m-%dT%H:%M:%SZ' "$ts" +%s 2>/dev/null)
  if [ -n "$out" ]; then printf '%s' "$out"; return 0; fi
  return 1
}

# The harness prints through the libtest runner, which prefixes the FIRST marker
# on a line with `test f23_journey_step ... `. Every consumer of these markers —
# this driver's own span extraction, and the plan's gates — anchors at column
# one, so an unnormalized prefix silently loses the day record. That is the same
# defect class as an anchored regex losing a bullet-prefixed panel vote: the read
# succeeds, finds nothing, and reports the wrong thing. Normalize on the way IN,
# and read unanchored anyway, so a log written before this fix still parses.
normalize_markers() { sed 's/^.*\(F23_04_[A-Z]\)/\1/'; }

run_step() {
  F23_JOURNEY_ROOT="$ROOT" \
  F23_JOURNEY_DAY="${1:-0}" \
  F23_JOURNEY_MODE="${2:-day}" \
  F23_JOURNEY_NONCE="$NONCE" \
  F23_JOURNEY_SPAN_SECONDS="$SPAN_SECONDS" \
  F23_JOURNEY_PLATFORM="$PLATFORM" \
  F23_JOURNEY_HOST="$(hostname)" \
    "$HARNESS" --exact f23_journey_step --nocapture --test-threads=1 2>&1
  return $?
}

# ── Verify mode ─────────────────────────────────────────────────────────────
if [ "$VERIFY" -eq 1 ]; then
  # Every day record the journey actually wrote, replayed verbatim from the
  # append-only log. These rows were emitted by a process that ran on THIS
  # platform on the day it stamps.
  grep -E 'F23_04_(DAY|INVARIANT|LOOP_OWNERS_OBSERVED|GOAL_LIFECYCLE|JOURNAL_CURSOR|WAIT_)' "$RUNLOG" \
    | normalize_markers
  rc=${PIPESTATUS[0]}
  if [ "$rc" -ne 0 ]; then
    echo "FATAL: the run log carries no day records; the journey did not run" >&2
    exit 71
  fi

  FIRST_TS=$(grep -oE 'F23_04_DAY=[0-9]+ platform=[a-z]+ ts=[^ ]+' "$RUNLOG" | head -1 | sed -n 's/.* ts=//p')
  LAST_TS=$(grep -oE 'F23_04_DAY=[0-9]+ platform=[a-z]+ ts=[^ ]+' "$RUNLOG" | tail -1 | sed -n 's/.* ts=//p')
  if [ -z "$FIRST_TS" ] || [ -z "$LAST_TS" ]; then
    echo "FATAL: could not read the run log's own first and last timestamps" >&2
    exit 71
  fi
  FIRST_EPOCH=$(epoch_of "$FIRST_TS"); rc=$?
  [ "$rc" -eq 0 ] || { echo "FATAL: unparsable first timestamp '$FIRST_TS'" >&2; exit 71; }
  LAST_EPOCH=$(epoch_of "$LAST_TS"); rc=$?
  [ "$rc" -eq 0 ] || { echo "FATAL: unparsable last timestamp '$LAST_TS'" >&2; exit 71; }

  SPAN=$(( LAST_EPOCH - FIRST_EPOCH ))
  echo "F23_04_SPAN_FIRST_TS=$FIRST_TS"
  echo "F23_04_SPAN_LAST_TS=$LAST_TS"
  echo "F23_04_SPAN_SECONDS=$SPAN"

  if [ -z "$REQUIRED_SPAN" ]; then
    echo "F23_04_SPAN_MEETS_AUTHORIZED_POLICY=false"
    echo "FATAL: no authorized ${PLATFORM}_required_real_span_seconds= to measure against" >&2
    exit 72
  fi
  echo "F23_04_SPAN_REQUIRED_SECONDS=$REQUIRED_SPAN"
  if [ "$SPAN" -lt "$REQUIRED_SPAN" ]; then
    echo "F23_04_SPAN_MEETS_AUTHORIZED_POLICY=false"
    echo "FATAL: recomputed span ${SPAN}s is short of the authorized ${REQUIRED_SPAN}s;" >&2
    echo "       the journey did not run and must be re-run rather than re-described" >&2
    exit 72
  fi
  echo "F23_04_SPAN_MEETS_AUTHORIZED_POLICY=true"

  # The live re-observation. Its process exit status is the platform gate.
  OUT=$(run_step 0 verify); rc=$?
  printf '%s\n' "$OUT" | normalize_markers
  if [ "$rc" -ne 0 ]; then
    echo "FATAL: the live verify step exited $rc" >&2
    exit 73
  fi

  echo "F23_04_JOURNEY=PASS platform=$PLATFORM nonce=$NONCE"
  exit 0
fi

# ── Day mode ────────────────────────────────────────────────────────────────
[ -n "$DAY" ] || { echo "FATAL: --day <n> or --verify is required" >&2; exit 64; }

# Idempotent per day: a second invocation on the same day must not double-count.
if grep -qE "F23_04_DAY=$DAY platform=$PLATFORM " "$RUNLOG"; then
  echo "F23_04_DAY_ALREADY_RECORDED=$DAY platform=$PLATFORM"
  exit 0
fi

STEP_OUT=$(run_step "$DAY" day | normalize_markers); rc=${PIPESTATUS[0]}
{
  echo "# ---- invocation day=$DAY platform=$PLATFORM ts=$(date -u +%Y-%m-%dT%H:%M:%SZ) host=$(hostname) pid=$$ sha=$SHA rc=$rc"
  printf '%s\n' "$STEP_OUT"
} >> "$RUNLOG"

printf '%s\n' "$STEP_OUT"
if [ "$rc" -ne 0 ]; then
  echo "FATAL: journey day $DAY exited $rc on $PLATFORM" >&2
  exit "$rc"
fi
echo "F23_04_JOURNEY_DAY_RECORDED=$DAY platform=$PLATFORM nonce=$NONCE"
exit 0
