#!/usr/bin/env bash
# F23-05 Task 1 — the LIVE determination probe for the budget wall-clock
# authority.
#
# The question this answers, empirically and across a REAL process death:
#
#   * does the absolute-deadline form derive its comparison from the system
#     clock read at EVALUATION time, or from a value captured at RESERVATION
#     time?
#   * does either form charge downtime — the seconds during which no process of
#     the session existed?
#   * does ANY non-privileged seam exist — environment variable, config key or
#     CLI flag — that could accelerate the absolute-deadline form honestly,
#     without real elapsed time and without moving the host system clock?
#
# Contract (shared with the rest of the f23 driver family):
#   --binary  <path>   the shipped wayland-core binary, whose --build-info
#                      source SHA must equal --sha
#   --sha     <commit> the commit under test
#   --nonce   <hex>    caller-generated, echoed in the terminal PASS marker
#   --harness <path>   the compiled multi_day_journey_test binary (optional;
#                      resolved through cargo when omitted)
#   --gap-seconds <n>  the REAL gap experiments A and C elapse (default 45)
#
# Why the harness and not the shipped binary for experiments A/B/C: MEASURED,
# not assumed. `BudgetWallClockAuthority::AbsoluteDeadline` has NO production
# construction site — every BudgetAuthoritySeed the shipped code builds
# hardcodes ActiveRuntime and BudgetConfig exposes no deadline field. The form
# is therefore unreachable from the shipped binary, and the strongest honest
# measurement available is a REAL process arming durable authority through the
# product's own public API, exiting, and a SECOND REAL process binding it after
# a REAL gap. That is a real process death over real durable state. It is NOT a
# claim that a user can reach the form, and this probe records that separately
# rather than blurring the two.
#
# Gate discipline: no check here is a pipeline into a filter. Every command's
# exit status is captured on the line AFTER it runs and asserted on directly.

set -uo pipefail

BINARY=""
SHA=""
NONCE=""
HARNESS=""
GAP_SECONDS=45
# Must match PROBE_WALL_CAP_SECS in the harness. A gap shorter than this cannot
# discriminate experiment A from its control.
PROBE_WALL_CAP_SECS=20

while [ $# -gt 0 ]; do
  case "$1" in
    --binary)       BINARY="${2:-}";      shift 2 ;;
    --sha)          SHA="${2:-}";         shift 2 ;;
    --nonce)        NONCE="${2:-}";       shift 2 ;;
    --harness)      HARNESS="${2:-}";     shift 2 ;;
    --gap-seconds)  GAP_SECONDS="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done

[ -n "$BINARY" ] || { echo "FATAL: --binary is required" >&2; exit 64; }
[ -n "$SHA" ]    || { echo "FATAL: --sha is required" >&2; exit 64; }
[ -n "$NONCE" ]  || { echo "FATAL: --nonce is required" >&2; exit 64; }
[ -x "$BINARY" ] || { echo "FATAL: $BINARY is not an executable file" >&2; exit 65; }

if [ "$GAP_SECONDS" -le "$PROBE_WALL_CAP_SECS" ]; then
  echo "FATAL: --gap-seconds $GAP_SECONDS cannot exceed the armed ${PROBE_WALL_CAP_SECS}s cap" >&2
  exit 64
fi

BINARY=$(cd "$(dirname "$BINARY")" && pwd)/$(basename "$BINARY")
REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
START_EPOCH=$(date -u +%s)

# ── Provenance: refuse to prove anything with a binary built from other code ──
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

echo "F23_04_PROBE_NONCE=$NONCE"
echo "F23_04_PROBE_HOST=$(hostname)"
echo "F23_04_PROBE_UNAME=$(uname -srm | tr ' ' '_')"
echo "F23_04_PROBE_SHA=$SHA"
echo "F23_04_PROBE_BINARY_BUILD_INFO=$(printf '%s' "$BUILD_INFO" | tr '\n' ' ')"

# ── Resolve the harness that arms and restores durable authority ─────────────
if [ -z "$HARNESS" ]; then
  BUILD_LOG=$(mktemp)
  ( cd "$REPO" && cargo test -p wcore-agent --test multi_day_journey_test --no-run --message-format=json ) > "$BUILD_LOG" 2>/dev/null
  rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "FATAL: could not build the probe harness (cargo exited $rc)" >&2
    exit 69
  fi
  HARNESS=$(sed -n 's/.*"executable":"\([^"]*multi_day_journey_test[^"]*\)".*/\1/p' "$BUILD_LOG" | tail -1)
  rm -f "$BUILD_LOG"
fi
if [ ! -x "$HARNESS" ]; then
  echo "FATAL: probe harness '$HARNESS' is not executable" >&2
  exit 69
fi
echo "F23_04_PROBE_HARNESS=$HARNESS"

RUN_DIR=$(mktemp -d)
cleanup() { rm -rf "$RUN_DIR"; }
trap cleanup EXIT

# Run ONE harness step in its OWN process, which then exits. `$1` is the mode,
# `$2` the form, `$3` the tag, `$4` the state root. Output goes to stdout so the
# probe log carries every observation verbatim.
harness_step() {
  local mode="$1" form="$2" tag="$3" root="$4"
  F23_PROBE_MODE="$mode" \
  F23_PROBE_FORM="$form" \
  F23_PROBE_TAG="$tag" \
  F23_PROBE_ROOT="$root" \
    "$HARNESS" --exact f23_clock_probe_step --nocapture --test-threads=1 2>&1
  return $?
}

# Extract one field from a captured harness step.
field() { printf '%s\n' "$2" | sed -n "s/.*[[:space:]]$1=\([^[:space:]]*\).*/\1/p" | tail -1; }

FAILURES=0
fail() { echo "  PROBE-FAIL: $*" >&2; FAILURES=$((FAILURES + 1)); }

# ── Experiment B (the control) — restore well INSIDE the deadline ────────────
# Run first so a broken harness fails before anything waits 45 seconds.
mkdir -p "$RUN_DIR/b"
OUT_B_ARM=$(harness_step arm absolute-deadline control-B "$RUN_DIR/b"); rc=$?
[ "$rc" -eq 0 ] || fail "experiment B arm exited $rc: $OUT_B_ARM"
OUT_B=$(harness_step restore absolute-deadline control-B "$RUN_DIR/b"); rc=$?
[ "$rc" -eq 0 ] || fail "experiment B restore exited $rc: $OUT_B"
B_EXCEEDED=$(field exceeded "$OUT_B")
B_REASON=$(field reason "$OUT_B")
echo "F23_04_EXPERIMENT_B=absolute-deadline gap=0s exceeded=${B_EXCEEDED:-indeterminate} reason=${B_REASON:-none}"

# ── Experiment A — a deadline that falls INSIDE a real gap ───────────────────
mkdir -p "$RUN_DIR/a"
OUT_A_ARM=$(harness_step arm absolute-deadline experiment-A "$RUN_DIR/a"); rc=$?
[ "$rc" -eq 0 ] || fail "experiment A arm exited $rc: $OUT_A_ARM"

# ── Experiment C — the same real gap under the active-runtime form ───────────
mkdir -p "$RUN_DIR/c"
OUT_C_ARM=$(harness_step arm active-runtime experiment-C "$RUN_DIR/c"); rc=$?
[ "$rc" -eq 0 ] || fail "experiment C arm exited $rc: $OUT_C_ARM"

# Both arming processes have now EXITED. Nothing of them is resident. The gap
# below is real elapsed wall time during which no process holds either journal.
echo "F23_04_PROBE_GAP_BEGIN=$(date -u +%Y-%m-%dT%H:%M:%SZ) seconds=$GAP_SECONDS"
sleep "$GAP_SECONDS"
echo "F23_04_PROBE_GAP_END=$(date -u +%Y-%m-%dT%H:%M:%SZ)"

OUT_A=$(harness_step restore absolute-deadline experiment-A "$RUN_DIR/a"); rc=$?
A_RC=$rc
A_EXCEEDED=$(field exceeded "$OUT_A")
A_REASON=$(field reason "$OUT_A")
echo "F23_04_EXPERIMENT_A=absolute-deadline gap=${GAP_SECONDS}s exceeded=${A_EXCEEDED:-indeterminate} reason=${A_REASON:-none} rc=$A_RC"

OUT_C=$(harness_step restore active-runtime experiment-C "$RUN_DIR/c"); rc=$?
[ "$rc" -eq 0 ] || fail "experiment C restore exited $rc: $OUT_C"
C_EXCEEDED=$(field exceeded "$OUT_C")
C_REASON=$(field reason "$OUT_C")
echo "F23_04_EXPERIMENT_C=active-runtime gap=${GAP_SECONDS}s exceeded=${C_EXCEEDED:-indeterminate} reason=${C_REASON:-none}"

# A restore that REFUSED is a measured outcome, not a skip: record it and let
# the determination below decide whether anything was discriminated.
if [ "$A_RC" -ne 0 ] && [ -z "$A_EXCEEDED" ]; then
  echo "F23_04_EXPERIMENT_A_REFUSED=$(printf '%s' "$OUT_A" | tr '\n' ' ' | tail -c 400)"
fi

# ── The discrimination check ─────────────────────────────────────────────────
# A and B differ only in how much REAL time elapsed while no process existed.
# If their observable outcomes are the same, the experiment discriminated
# nothing and no determination may be reported.
if [ -z "$A_EXCEEDED" ] || [ -z "$B_EXCEEDED" ]; then
  fail "experiment A or its control B produced no determinate outcome"
  ABS_EVAL=""
elif [ "$A_EXCEEDED" = "$B_EXCEEDED" ]; then
  fail "experiment A ($A_EXCEEDED) did not differ from its control B ($B_EXCEEDED); nothing was discriminated"
  ABS_EVAL=""
elif [ "$A_EXCEEDED" = "true" ] && [ "$B_EXCEEDED" = "false" ]; then
  # Identical durable state; only the real gap differed. The comparison
  # therefore consults the system clock at evaluation time.
  ABS_EVAL="system-clock-at-evaluation"
else
  # The gap made the envelope MORE available than no gap did. That can only come
  # from a value frozen at reservation time.
  ABS_EVAL="captured-at-reservation"
fi

# ── Experiment D — the seam search ───────────────────────────────────────────
# For every non-privileged clock override the product actually exposes, attempt
# to reproduce experiment A's outcome WITHOUT real elapsed time and WITHOUT
# moving the host system clock. Anything that succeeds IS the seam.
SEAM="none"
CANDIDATES="
WAYLAND_CLOCK_NOW_MS
WAYLAND_NOW_UNIX_MILLIS
WAYLAND_FAKE_CLOCK
WAYLAND_TEST_CLOCK
WAYLAND_CLOCK_OFFSET_MS
WAYLAND_BUDGET_DEADLINE_UNIX_MILLIS
WCORE_CLOCK_NOW_MS
WCORE_NOW_UNIX_MILLIS
WCORE_FAKE_CLOCK
WCORE_TEST_CLOCK
WCORE_CLOCK_OFFSET_MS
SOURCE_DATE_EPOCH
FAKETIME
"
FAR_FUTURE_MS=$(( ( $(date -u +%s) + 86400 ) * 1000 ))
for VAR in $CANDIDATES; do
  [ -n "$VAR" ] || continue
  D_ROOT="$RUN_DIR/d-$VAR"
  mkdir -p "$D_ROOT"
  OUT=$(F23_PROBE_MODE=arm F23_PROBE_FORM=absolute-deadline F23_PROBE_TAG="seam-$VAR" \
        F23_PROBE_ROOT="$D_ROOT" env "$VAR=$FAR_FUTURE_MS" \
        "$HARNESS" --exact f23_clock_probe_step --nocapture --test-threads=1 2>&1)
  [ $? -eq 0 ] || continue
  OUT=$(F23_PROBE_MODE=restore F23_PROBE_FORM=absolute-deadline F23_PROBE_TAG="seam-$VAR" \
        F23_PROBE_ROOT="$D_ROOT" env "$VAR=$FAR_FUTURE_MS" \
        "$HARNESS" --exact f23_clock_probe_step --nocapture --test-threads=1 2>&1)
  rc=$?
  EX=$(field exceeded "$OUT")
  echo "F23_04_SEAM_ATTEMPT=$VAR rc=$rc exceeded=${EX:-indeterminate}"
  # A seam is a variable that reproduces A's outcome with NO real gap.
  if [ "$rc" -eq 0 ] && [ "$EX" = "true" ]; then
    SEAM="$VAR"
    break
  fi
done

# The shipped binary's own documented surface: does any flag name a clock,
# deadline or wall-time override? Measured against --help, not against memory.
HELP=$("$BINARY" --help 2>&1)
HELP_HITS=$(printf '%s\n' "$HELP" | grep -ciE -- '--[a-z-]*(clock|deadline|now|fake-?time)' )
echo "F23_04_SEAM_CLI_FLAG_HITS=$HELP_HITS"
if [ "$HELP_HITS" -gt 0 ] && [ "$SEAM" = "none" ]; then
  SEAM="cli-flag-see-F23_04_SEAM_CLI_FLAG_HITS"
fi

# How many places in the SHIPPED code (not tests, not examples) can construct
# the absolute-deadline form at all. Measured over the tree under test.
PROD_SITES=$(grep -rlE 'BudgetWallClockAuthority::AbsoluteDeadline' \
              --include='*.rs' "$REPO/crates" 2>/dev/null \
              | grep -v '/tests/' | grep -v '/examples/' | grep -v '/benches/' \
              | xargs grep -hcE 'BudgetWallClockAuthority::AbsoluteDeadline[^ ]* *\{' 2>/dev/null \
              | awk '{s+=$1} END {print s+0}')
CONSTRUCTION_SITES=$(grep -rn 'wall_clock: *crate::session_journal::BudgetWallClockAuthority::AbsoluteDeadline\|wall_clock: *BudgetWallClockAuthority::AbsoluteDeadline' \
                      --include='*.rs' "$REPO/crates" 2>/dev/null \
                      | grep -vc '/tests/\|/examples/\|/benches/')
echo "F23_04_ABSDEADLINE_MATCH_SITES_NONTEST=$PROD_SITES"
echo "F23_04_ABSDEADLINE_PRODUCT_CONSTRUCTION_SITES=$CONSTRUCTION_SITES"
if [ "$CONSTRUCTION_SITES" -eq 0 ]; then
  echo "F23_04_ABSDEADLINE_PRODUCT_REACHABLE=false"
else
  echo "F23_04_ABSDEADLINE_PRODUCT_REACHABLE=true"
fi

# ── The determination ────────────────────────────────────────────────────────
if [ -n "$ABS_EVAL" ]; then
  echo "F23_04_ABSDEADLINE_EVAL=$ABS_EVAL"
fi
if [ -n "$A_EXCEEDED" ]; then
  echo "F23_04_ABSDEADLINE_CHARGES_DOWNTIME=$A_EXCEEDED"
fi
if [ -n "$C_EXCEEDED" ]; then
  echo "F23_04_ACTIVERUNTIME_CHARGES_DOWNTIME=$C_EXCEEDED"
fi
echo "F23_04_CLOCK_INJECTION_SEAM=$SEAM"
# Acceleration is honest for the absolute-deadline form only if a seam exists
# that reproduces its real-time behaviour without real time passing.
if [ "$SEAM" = "none" ]; then
  echo "F23_04_ACCEL_HONEST_FOR_ABSOLUTE_DEADLINE=false"
else
  echo "F23_04_ACCEL_HONEST_FOR_ABSOLUTE_DEADLINE=true"
fi

END_EPOCH=$(date -u +%s)
echo "F23_04_REAL_GAP_SECONDS=$(( END_EPOCH - START_EPOCH ))"

if [ "$FAILURES" -ne 0 ]; then
  echo "F23_04_PROBE=INDETERMINATE failures=$FAILURES" >&2
  exit 1
fi
echo "F23_04_PROBE=PASS nonce=$NONCE"
exit 0
