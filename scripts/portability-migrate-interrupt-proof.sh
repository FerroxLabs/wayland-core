#!/bin/sh
# portability-migrate-interrupt-proof.sh — Phase 26 Success Criterion 3, the two
# clauses that were never exercised: PROFILE MIGRATION and RECIPROCAL
# PORTABILITY under a real mid-apply interruption.
#
# Usage:
#   sh scripts/portability-migrate-interrupt-proof.sh \
#        [--peer hermes|openclaw] [--trials N] [--items N] [--no-kill] <bin>
#
# WHAT THIS PROVES, AND WHY IT IS BUILT THIS WAY
#
# 26-04 graded SC3 OPEN and named the exact unmet clause: only `backup restore`
# was ever interrupted, so exact rollback for the MIGRATION path rested on a
# partial-failure argument rather than on a measured interruption. This kills a
# real `migrate` mid-apply and then DRIVES THE PRODUCT AGAIN, asserting on the
# state that results rather than on the presence of recovery code.
#
# Three ways a proof of this shape can pass while proving nothing, each closed:
#
#   1. THE KILL LANDS AFTER THE APPLY FINISHED. Then every downstream check is
#      trivially consistent. So each trial is CLASSIFIED from observed state --
#      `pre` (nothing written), `mid` (partial), `post` (complete) -- and the
#      script FAILS if no trial landed `mid`. `--no-kill` is the negative
#      control: the identical procedure with the kill removed, which must
#      produce zero `mid` trials, proving the classifier can tell them apart.
#
#   2. THE COMPARISON IS AGAINST NOTHING. A fresh empty home trivially matches
#      another fresh empty home. So the reference run's fingerprint is asserted
#      NON-VACUOUS first: it must carry profiles in `config.toml` AND a
#      populated quarantine index, and the script fails if it does not.
#
#   3. THE COMPARISON IS TOO LOOSE. `Provenance::imported_at` is wall-clock, so
#      raw tree digests differ between two CORRECT runs. Exactly that one field
#      is normalised (see portability-migrate-state.py) and nothing else --
#      payload bytes, entry set, recorded digests and `config.toml` all count.
#
# ISOLATION is proven the way 26-04 proved it: a sentinel tree OUTSIDE every
# target, digested by the product's own `backup digest` before and after, and
# required unchanged. An interrupted write is exactly when isolation is most
# likely to fail, so this is not ceremony.
#
# Prints a fixed-grammar verdict block on stdout. Exits non-zero on any failure.

set -u

FAIL() { echo "PROOF-FAIL: $*"; exit 1; }

PEER=hermes
TRIALS=9
ITEMS=440
DO_KILL=yes

while [ $# -gt 1 ]; do
    case "$1" in
        --peer)    PEER="$2"; shift 2 ;;
        --trials)  TRIALS="$2"; shift 2 ;;
        --items)   ITEMS="$2"; shift 2 ;;
        --no-kill) DO_KILL=no; shift ;;
        *) break ;;
    esac
done

BIN="${1:-}"
[ -n "$BIN" ] || FAIL "usage: $0 [--peer hermes|openclaw] [--trials N] <path-to-wayland-core>"
[ -x "$BIN" ] || FAIL "binary does not exist or is not executable: $BIN"
case "$PEER" in hermes|openclaw) ;; *) FAIL "unknown peer: $PEER" ;; esac
command -v python3 >/dev/null 2>&1 || FAIL "python3 is required to fingerprint a home"

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
STATE="$HERE/portability-migrate-state.py"
CORPUS_GEN="$HERE/portability-migrate-corpus.py"
[ -f "$STATE" ] || FAIL "missing $STATE"
[ -f "$CORPUS_GEN" ] || FAIL "missing $CORPUS_GEN"

"$BIN" migrate --help >/dev/null 2>&1 || FAIL "binary does not support 'migrate': $BIN"
"$BIN" backup digest --help >/dev/null 2>&1 || FAIL "binary does not support 'backup digest': $BIN"

echo "PEER: $PEER"
echo "TRIALS: $TRIALS"
echo "ITEMS: $ITEMS"
echo "KILL-ENABLED: $DO_KILL"
echo "BINARY: $BIN"
"$BIN" --build-info 2>/dev/null | sed -n 's/^/BUILD-INFO: /p' | head -6

WORK=$(mktemp -d) || FAIL "could not create a work directory"
trap 'rm -rf "$WORK"' EXIT
CORPUS="$WORK/peer-home"
SENTINEL="$WORK/sentinel"

# --- the external sentinel ----------------------------------------------------
# OUTSIDE every target home this proof ever names. Digested by the PRODUCT's own
# `backup digest`, so isolation is measured with the same algorithm the product
# uses rather than this script's arithmetic.
mkdir -p "$SENTINEL/skills/sentinel-skill" "$SENTINEL/memory" || FAIL "sentinel setup"
printf 'SENTINEL-CONFIG-DO-NOT-TOUCH\n' > "$SENTINEL/config.toml"
printf 'SENTINEL-SKILL-BODY\n' > "$SENTINEL/skills/sentinel-skill/SKILL.md"
printf 'SENTINEL-MEMORY\n' > "$SENTINEL/memory/notes.md"
SENT_PRE=$("$BIN" backup digest --home "$SENTINEL" 2>/dev/null | sed -n 's/^DIGEST: //p')
[ -n "$SENT_PRE" ] || FAIL "could not take a pre-run sentinel digest"
echo "SENTINEL-DIGEST-PRE: $SENT_PRE"

# --- corpus -------------------------------------------------------------------
python3 "$CORPUS_GEN" --kind "$PEER" --out "$CORPUS" --items "$ITEMS" \
    || FAIL "corpus generation failed"

run_migrate() {
    # $1 = target home. Everything else is fixed: no credentials, no overwrite,
    # no prompt. A real operator invocation, not a test-only path.
    WAYLAND_HOME="$1" "$BIN" migrate "$PEER" --home "$CORPUS" --yes
}

fingerprint() { python3 "$STATE" "$1"; }
field() { sed -n "s/^$2: //p" "$1" | head -1; }

# --- reference run: what a CLEAN, uninterrupted migration produces -------------
REF_HOME="$WORK/ref-home"
mkdir -p "$REF_HOME"
T0=$(date +%s%N 2>/dev/null || echo 0)
run_migrate "$REF_HOME" > "$WORK/ref.log" 2>&1
REF_RC=$?
T1=$(date +%s%N 2>/dev/null || echo 0)
[ "$REF_RC" -eq 0 ] || { sed -n '1,40p' "$WORK/ref.log"; FAIL "the reference migration failed (rc=$REF_RC)"; }
DUR_MS=$(( (T1 - T0) / 1000000 ))
[ "$DUR_MS" -gt 0 ] || DUR_MS=1
echo "REFERENCE-RC: $REF_RC"
echo "REFERENCE-DURATION-MS: $DUR_MS"

fingerprint "$REF_HOME" > "$WORK/ref.fp" || FAIL "could not fingerprint the reference home"
REF_FP=$(field "$WORK/ref.fp" FINGERPRINT)
REF_ENTRIES=$(field "$WORK/ref.fp" ENTRIES)
REF_PROFILES=$(field "$WORK/ref.fp" CONFIG-PROFILES)
REF_PAYLOADS=$(field "$WORK/ref.fp" PAYLOAD-FILES)
echo "REFERENCE-FINGERPRINT: $REF_FP"
echo "REFERENCE-ENTRIES: $REF_ENTRIES"
echo "REFERENCE-PROFILES: $REF_PROFILES"
echo "REFERENCE-PAYLOAD-FILES: $REF_PAYLOADS"

# NON-VACUITY. Without this, every later "matches the reference" would be a
# comparison of two empty homes.
[ "${REF_ENTRIES:-0}" -ge 8 ] || FAIL "reference quarantine is near-empty (${REF_ENTRIES}); nothing to interrupt"
[ "${REF_PROFILES:-0}" -ge 1 ] || FAIL "reference config.toml carries no profile; the apply wrote nothing"
[ "${REF_PAYLOADS:-0}" -ge 8 ] || FAIL "reference carries no payload bytes; the admit loop did no work"

# Determinism control: a SECOND clean run must reproduce the reference
# fingerprint. If it does not, the fingerprint is unstable and no later
# comparison means anything -- so this is checked before any kill.
DET_HOME="$WORK/det-home"
mkdir -p "$DET_HOME"
run_migrate "$DET_HOME" > "$WORK/det.log" 2>&1 || FAIL "the determinism control migration failed"
DET_FP=$(fingerprint "$DET_HOME" | sed -n 's/^FINGERPRINT: //p')
if [ "$DET_FP" != "$REF_FP" ]; then
    FAIL "two clean runs disagree ($REF_FP vs $DET_FP); the fingerprint is not a usable comparand"
fi
echo "DETERMINISM-CONTROL: pass"

# --- trials -------------------------------------------------------------------
MID=0; PRE=0; POST=0
RECOVERED=0; NOT_RECOVERED=0
CORRUPT_INDEX=0; EMPTY_INDEX=0; ORPHANS_SEEN=0
REDRIVE_FAILED=0
echo "TRIAL-TABLE: trial delay_ms class index entries payloads orphans cfg redrive_rc recovered"

k=1
while [ "$k" -le "$TRIALS" ]; do
    TH="$WORK/trial-$k"
    mkdir -p "$TH"
    # Sweep the whole apply window rather than one guessed point: a fixed delay
    # would only ever probe one instant of the loop.
    DELAY_MS=$(( DUR_MS * k / (TRIALS + 1) ))
    [ "$DELAY_MS" -lt 1 ] && DELAY_MS=1

    if [ "$DO_KILL" = yes ]; then
        WAYLAND_HOME="$TH" "$BIN" migrate "$PEER" --home "$CORPUS" --yes \
            > "$WORK/trial-$k.log" 2>&1 &
        PID=$!
        # SIGKILL is uncatchable: no handler, no atexit, no Drop. A catchable
        # signal would let the product tidy up and would prove far less.
        python3 -c "import time,sys; time.sleep(float(sys.argv[1])/1000.0)" "$DELAY_MS"
        kill -9 "$PID" 2>/dev/null
        wait "$PID" 2>/dev/null
        KILL_RC=$?
    else
        run_migrate "$TH" > "$WORK/trial-$k.log" 2>&1
        KILL_RC=$?
    fi

    fingerprint "$TH" > "$WORK/trial-$k.fp"
    T_INDEX=$(field "$WORK/trial-$k.fp" INDEX)
    T_ENTRIES=$(field "$WORK/trial-$k.fp" ENTRIES)
    T_PAYLOADS=$(field "$WORK/trial-$k.fp" PAYLOAD-FILES)
    T_ORPHANS=$(field "$WORK/trial-$k.fp" ORPHAN-PAYLOAD-DIRS)
    T_CFG=$(field "$WORK/trial-$k.fp" CONFIG-PROFILES)
    T_FP=$(field "$WORK/trial-$k.fp" FINGERPRINT)

    # CLASSIFY FROM OBSERVED STATE, never from the delay we asked for.
    #   post -> the apply completed: the fingerprint already equals the reference
    #   pre  -> nothing was written at all
    #   mid  -> anything else: some work landed and the run did not finish
    if [ "$T_FP" = "$REF_FP" ]; then
        CLASS=post; POST=$((POST + 1))
    elif [ "${T_ENTRIES:-0}" -eq 0 ] && [ "${T_PAYLOADS:-0}" -eq 0 ] \
         && [ "${T_CFG:-0}" -eq 0 ] && [ "$T_INDEX" = absent ]; then
        CLASS=pre; PRE=$((PRE + 1))
    else
        CLASS=mid; MID=$((MID + 1))
    fi

    [ "$T_INDEX" = corrupt ] && CORRUPT_INDEX=$((CORRUPT_INDEX + 1))
    [ "$T_INDEX" = empty ] && EMPTY_INDEX=$((EMPTY_INDEX + 1))
    [ "${T_ORPHANS:-0}" -gt 0 ] && ORPHANS_SEEN=$((ORPHANS_SEEN + 1))

    # --- DRIVE THE PRODUCT AGAIN. This is the observation, not an assertion. ---
    run_migrate "$TH" > "$WORK/trial-$k.redrive.log" 2>&1
    REDRIVE_RC=$?
    [ "$REDRIVE_RC" -ne 0 ] && REDRIVE_FAILED=$((REDRIVE_FAILED + 1))

    FINAL_FP=$(fingerprint "$TH" | sed -n 's/^FINGERPRINT: //p')
    if [ "$FINAL_FP" = "$REF_FP" ]; then
        REC=yes; RECOVERED=$((RECOVERED + 1))
    else
        REC=no; NOT_RECOVERED=$((NOT_RECOVERED + 1))
        cp "$WORK/trial-$k.fp" "$WORK/UNRECOVERED-$k-postkill.fp" 2>/dev/null
        fingerprint "$TH" > "$WORK/UNRECOVERED-$k-final.fp" 2>/dev/null
    fi

    printf 'TRIAL: %d %d %s %s %s %s %s %s %s %s\n' \
        "$k" "$DELAY_MS" "$CLASS" "$T_INDEX" "${T_ENTRIES:-0}" \
        "${T_PAYLOADS:-0}" "${T_ORPHANS:-0}" "${T_CFG:-0}" "$REDRIVE_RC" "$REC"
    k=$((k + 1))
done

# --- isolation ----------------------------------------------------------------
SENT_POST=$("$BIN" backup digest --home "$SENTINEL" 2>/dev/null | sed -n 's/^DIGEST: //p')
[ -n "$SENT_POST" ] || FAIL "could not take a post-run sentinel digest"
echo "SENTINEL-DIGEST-POST: $SENT_POST"
if [ "$SENT_PRE" = "$SENT_POST" ]; then
    echo "SENTINEL-UNCHANGED: yes"
else
    echo "SENTINEL-UNCHANGED: no"
fi

echo "CLASS-PRE: $PRE"
echo "CLASS-MID: $MID"
echo "CLASS-POST: $POST"
echo "RECOVERED: $RECOVERED"
echo "NOT-RECOVERED: $NOT_RECOVERED"
echo "REDRIVE-NONZERO: $REDRIVE_FAILED"
echo "CORRUPT-INDEX-TRIALS: $CORRUPT_INDEX"
echo "EMPTY-INDEX-TRIALS: $EMPTY_INDEX"
echo "ORPHAN-PAYLOAD-TRIALS: $ORPHANS_SEEN"

# --- verdict ------------------------------------------------------------------
RC=0
[ "$SENT_PRE" = "$SENT_POST" ] || { echo "PROOF-FAIL: the external sentinel tree CHANGED"; RC=1; }

if [ "$DO_KILL" = yes ]; then
    # The mid-flight requirement. Without it the whole run is vacuous.
    if [ "$MID" -lt 1 ]; then
        echo "PROOF-FAIL: no trial landed mid-apply; nothing was actually interrupted"
        RC=1
    fi
    if [ "$NOT_RECOVERED" -gt 0 ]; then
        echo "PROOF-FAIL: $NOT_RECOVERED trial(s) did not return to the reference state after re-driving the product"
        RC=1
    fi
    if [ "$REDRIVE_FAILED" -gt 0 ]; then
        echo "PROOF-FAIL: $REDRIVE_FAILED re-drive(s) exited non-zero"
        RC=1
    fi
else
    # Negative control: with no kill, EVERY trial must complete. A `mid` here
    # would mean the classifier reports partial state for a run that finished,
    # and every `mid` in the real run would be worthless.
    if [ "$MID" -gt 0 ]; then
        echo "PROOF-FAIL: --no-kill produced $MID mid-apply classifications; the classifier is unsound"
        RC=1
    fi
    if [ "$POST" -ne "$TRIALS" ]; then
        echo "PROOF-FAIL: --no-kill completed only $POST of $TRIALS trials"
        RC=1
    fi
fi

if [ "$RC" -eq 0 ]; then
    echo "PROOF: PASS peer=$PEER trials=$TRIALS mid=$MID recovered=$RECOVERED"
else
    echo "PROOF: FAIL peer=$PEER trials=$TRIALS mid=$MID recovered=$RECOVERED not_recovered=$NOT_RECOVERED"
fi
exit "$RC"
