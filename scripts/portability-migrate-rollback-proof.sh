#!/bin/sh
# portability-migrate-rollback-proof.sh
#
# SC3, the clause that was NOT met: "restore exact pre-operation state on
# rollback", for `migrate`.
#
# `portability-migrate-interrupt-proof.sh` proves the other half — that an
# interrupted import does not lose data, because re-driving the product
# converges on the completed state. That is forward convergence. It is not
# rollback, and 26-PHASE-VERDICT.md graded Criterion 3 PARTIAL on exactly that
# distinction.
#
# What this measures instead: kill a real `migrate` MID-APPLY, roll back, and
# require the home to be BYTE-IDENTICAL to the state it was in before the import
# started. Several interruption points, not one, because the interesting
# failures cluster in specific windows (the admit loop, the index rewrite, the
# single atomic config write at the end).
#
# ---------------------------------------------------------------------------
# The three ways this proof could pass without proving anything, and the control
# that closes each:
#
#   1. THE KILL LANDS OUTSIDE THE APPLY. Then "the home is unchanged" is true
#      because nothing ever happened. Every trial is classified `pre` / `mid` /
#      `post` from the home's own state, and the run FAILS unless enough trials
#      landed `mid`.
#
#   2. THE PRE-OPERATION STATE IS EMPTY. Restoring nothing to nothing is
#      byte-identical for free. The home is PRE-POPULATED by a real prior
#      import plus out-of-scope state, and the run FAILS if that state is thin.
#
#   3. THE DIGEST CANNOT REPORT A DIFFERENCE. This is the one that matters most,
#      and it is the class this programme keeps finding. Arm `norollback` runs
#      the IDENTICAL kills and SKIPS the rollback: it must report DIGEST-EQUAL
#      **no**. If it does not, the comparison is dead and arm `rollback` proves
#      nothing. The run FAILS if the known-negative fails to fail.
#
# A fourth control, `nokill`, runs to completion and requires the recovery pass
# to find NOTHING and the home to equal the POST-import state — proving recovery
# does not blindly revert work that finished.
#
# Usage: portability-migrate-rollback-proof.sh [--peer hermes|openclaw]
#                                              [--trials N] [--items N]
#                                              [--evidence DIR] <bin>
set -u

FAIL() { echo "PROOF: FAIL — $*"; exit 1; }

PEER=hermes
TRIALS=9
ITEMS=220
EVIDENCE=""
BIN=""
while [ $# -gt 0 ]; do
    case "$1" in
        --peer)     PEER="$2"; shift 2 ;;
        --trials)   TRIALS="$2"; shift 2 ;;
        --items)    ITEMS="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        -*)         FAIL "unknown option $1" ;;
        *)          BIN="$1"; shift ;;
    esac
done
[ -n "$BIN" ] || FAIL "usage: $0 [--peer P] [--trials N] [--items N] [--evidence DIR] <bin>"
[ -x "$BIN" ] || FAIL "not executable: $BIN"

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
CORPUS_GEN="$HERE/portability-migrate-corpus.py"
STATE="$HERE/portability-migrate-state.py"
[ -f "$CORPUS_GEN" ] || FAIL "missing $CORPUS_GEN"
[ -f "$STATE" ] || FAIL "missing $STATE"

echo "=== migrate ROLLBACK proof (SC3: exact pre-operation state) ==="
echo "BIN: $BIN"
echo "BIN-SHA256: $(sha256sum "$BIN" 2>/dev/null | cut -d' ' -f1)"
echo "PEER: $PEER"
echo "TRIALS-PER-ARM: $TRIALS"
echo "ITEMS: $ITEMS"
echo "DATE-UTC: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "HOST: $(hostname)"

WORK=$(mktemp -d) || FAIL "could not create a work directory"
trap 'rm -rf "$WORK"' EXIT
[ -n "$EVIDENCE" ] && mkdir -p "$EVIDENCE"

# --- corpora ------------------------------------------------------------------
# SEED is a small prior import; MAIN is the large one this proof interrupts. Two
# distinct corpora, so the pre-operation home genuinely holds earlier work that
# the interrupted import must not destroy.
SEED_CORPUS="$WORK/seed-peer"
MAIN_CORPUS="$WORK/main-peer"
python3 "$CORPUS_GEN" --kind "$PEER" --out "$SEED_CORPUS" --items 24 \
    || FAIL "seed corpus generation failed"
python3 "$CORPUS_GEN" --kind "$PEER" --out "$MAIN_CORPUS" --items "$ITEMS" \
    || FAIL "main corpus generation failed"

digest_of() { "$BIN" backup digest --home "$1" 2>/dev/null | sed -n 's/^DIGEST: //p'; }
field() { sed -n "s/^$2: //p" "$1" | head -1; }

# `WAYLAND_MIGRATE_SCOPE_PROBE=1` makes the product itself verify that the apply
# wrote nothing outside the paths the journal's undo store covers. Without it the
# scoping is a claim; with it, a write outside the scope fails the migrate.
run_migrate() {
    WAYLAND_HOME="$1" WAYLAND_MIGRATE_SCOPE_PROBE=1 \
        "$BIN" migrate "$PEER" --home "$2" --yes --overwrite
}

# --- the pre-operation home ---------------------------------------------------
# Built ONCE, then copied per trial. It holds:
#   * a real prior import (quarantine entries + config profiles) — in scope;
#   * out-of-scope user state the rollback must not revert or copy.
TEMPLATE="$WORK/template-home"
mkdir -p "$TEMPLATE"
run_migrate "$TEMPLATE" "$SEED_CORPUS" > "$WORK/seed.log" 2>&1 \
    || { sed -n '1,30p' "$WORK/seed.log"; FAIL "the seeding import failed"; }
mkdir -p "$TEMPLATE/sessions" "$TEMPLATE/skills/hand-written"
printf 'PRIOR-USER-MEMORY-DB\n' > "$TEMPLATE/memory.db"
printf '{"session":"prior"}\n' > "$TEMPLATE/sessions/prior.json"
printf 'HAND WRITTEN SKILL BODY\n' > "$TEMPLATE/skills/hand-written/SKILL.md"

python3 "$STATE" "$TEMPLATE" > "$WORK/template.fp" || FAIL "could not fingerprint the template"
TPL_ENTRIES=$(field "$WORK/template.fp" ENTRIES)
TPL_PROFILES=$(field "$WORK/template.fp" CONFIG-PROFILES)
TPL_PAYLOADS=$(field "$WORK/template.fp" PAYLOAD-FILES)
echo "TEMPLATE-ENTRIES: $TPL_ENTRIES"
echo "TEMPLATE-PROFILES: $TPL_PROFILES"
echo "TEMPLATE-PAYLOAD-FILES: $TPL_PAYLOADS"

# NON-VACUITY of the pre-operation state (hazard 2). Restoring an empty home to
# an empty home is byte-identical for free.
[ "${TPL_ENTRIES:-0}" -ge 4 ] || FAIL "the pre-operation home holds ${TPL_ENTRIES} quarantine entries; too thin to prove anything"
[ "${TPL_PROFILES:-0}" -ge 1 ] || FAIL "the pre-operation home carries no profile"
[ "${TPL_PAYLOADS:-0}" -ge 4 ] || FAIL "the pre-operation home carries no payload bytes"

# No journal may be left over from the seeding import: a residual undo store
# would be recovered by the first trial and every later measurement would be
# against the wrong baseline.
[ -d "$TEMPLATE/.wayland-backup-journal" ] \
    && FAIL "the seeding import left journal bookkeeping behind"

PRE_DIGEST=$(digest_of "$TEMPLATE")
[ -n "$PRE_DIGEST" ] || FAIL "could not take the pre-operation digest"
echo "PRE-OPERATION-DIGEST: $PRE_DIGEST"

# The copy must be faithful, or "identical to PRE" is a statement about `cp`.
CPCHK="$WORK/copy-check"
cp -a "$TEMPLATE" "$CPCHK" || FAIL "template copy failed"
CPCHK_DIGEST=$(digest_of "$CPCHK")
[ "$CPCHK_DIGEST" = "$PRE_DIGEST" ] \
    || FAIL "a copy of the template does not digest equal to it ($CPCHK_DIGEST); the comparand is unusable"
echo "COPY-FIDELITY-CONTROL: pass"
rm -rf "$CPCHK"

# --- reference timing ---------------------------------------------------------
# How long the interruptible window actually is, measured on THIS hardware.
REF="$WORK/ref-home"
cp -a "$TEMPLATE" "$REF" || FAIL "reference copy failed"
T0=$(date +%s%N)
run_migrate "$REF" "$MAIN_CORPUS" > "$WORK/ref.log" 2>&1
REF_RC=$?
T1=$(date +%s%N)
[ "$REF_RC" -eq 0 ] || { sed -n '1,30p' "$WORK/ref.log"; FAIL "the reference import failed (rc=$REF_RC)"; }
DUR_MS=$(( (T1 - T0) / 1000000 ))
[ "$DUR_MS" -gt 0 ] || DUR_MS=1
POST_DIGEST=$(digest_of "$REF")
echo "REFERENCE-DURATION-MS: $DUR_MS"
echo "POST-IMPORT-DIGEST: $POST_DIGEST"

# The import must actually CHANGE the home, or PRE and POST are the same value
# and every comparison below is trivially true.
[ "$POST_DIGEST" != "$PRE_DIGEST" ] \
    || FAIL "a completed import did not change the home; PRE and POST are identical, so no comparison here can fail"
echo "MUTATION-CONTROL: pass (PRE != POST)"

# The scope probe was armed for that run and it exited 0, so the product itself
# confirmed it wrote nothing outside the journal's coverage.
echo "SCOPE-PROBE: armed, reference import rc=0"

# --- trial machinery ----------------------------------------------------------
# $1 arm name, $2 do_kill(yes|no), $3 do_rollback(yes|no)
MID=0; EQUAL=0; DIFFER=0; RESIDUE=0; RECOVERED_TOTAL=0
run_arm() {
    ARM="$1"; DO_KILL="$2"; DO_ROLLBACK="$3"
    MID=0; EQUAL=0; DIFFER=0; RESIDUE=0; RECOVERED_TOTAL=0
    echo
    echo "--- ARM: $ARM (kill=$DO_KILL rollback=$DO_ROLLBACK) ---"
    echo "TRIAL-TABLE[$ARM]: trial delay_ms class recovered digest_equal journal_residue"
    k=1
    while [ "$k" -le "$TRIALS" ]; do
        H="$WORK/$ARM-$k"
        rm -rf "$H"
        cp -a "$TEMPLATE" "$H" || FAIL "trial copy failed"

        # Sweep the delay across the measured apply window, so the kills land at
        # DIFFERENT points rather than all in one place.
        DELAY_MS=$(( DUR_MS * k / (TRIALS + 1) ))

        if [ "$DO_KILL" = yes ]; then
            run_migrate "$H" "$MAIN_CORPUS" > "$WORK/$ARM-$k.log" 2>&1 &
            PID=$!
            python3 -c "import time,sys; time.sleep(float(sys.argv[1])/1000.0)" "$DELAY_MS"
            # SIGKILL is uncatchable: no handler, no atexit, no Drop. Whatever
            # the journal does after this, the process did not choose to do.
            kill -9 "$PID" 2>/dev/null
            wait "$PID" 2>/dev/null
        else
            run_migrate "$H" "$MAIN_CORPUS" > "$WORK/$ARM-$k.log" 2>&1
        fi

        # Classify from the home's own state, BEFORE any rollback.
        KILL_DIGEST=$(digest_of "$H")
        if [ "$KILL_DIGEST" = "$PRE_DIGEST" ]; then
            CLASS=pre
        elif [ "$KILL_DIGEST" = "$POST_DIGEST" ]; then
            CLASS=post
        else
            CLASS=mid; MID=$((MID + 1))
        fi

        REC=0
        if [ "$DO_ROLLBACK" = yes ]; then
            "$BIN" backup recover --home "$H" > "$WORK/$ARM-$k.recover" 2>&1
            REC=$(sed -n 's/^recovered_operations: //p' "$WORK/$ARM-$k.recover" | head -1)
            REC=${REC:-0}
            RECOVERED_TOTAL=$((RECOVERED_TOTAL + REC))
        fi

        FINAL=$(digest_of "$H")
        if [ "$FINAL" = "$PRE_DIGEST" ]; then EQ=yes; EQUAL=$((EQUAL + 1)); else EQ=no; DIFFER=$((DIFFER + 1)); fi

        # A digest that EXCLUDES the journal directory cannot see bookkeeping
        # left behind, so that is checked separately rather than assumed.
        if [ -d "$H/.wayland-backup-journal" ]; then RES=yes; RESIDUE=$((RESIDUE + 1)); else RES=no; fi

        printf 'TRIAL[%s]: %d %d %s %s %s %s\n' "$ARM" "$k" "$DELAY_MS" "$CLASS" "$REC" "$EQ" "$RES"

        if [ -n "$EVIDENCE" ] && [ "$CLASS" = mid ] && [ "$k" -le 3 ]; then
            D="$EVIDENCE/$ARM-trial-$k"; mkdir -p "$D"
            cp "$WORK/$ARM-$k.log" "$D/kill-run.log" 2>/dev/null
            cp "$WORK/$ARM-$k.recover" "$D/recover.log" 2>/dev/null
            python3 "$STATE" "$H" > "$D/final-state.txt" 2>/dev/null
            {
                echo "PRE-OPERATION-DIGEST: $PRE_DIGEST"
                echo "POST-KILL-DIGEST:     $KILL_DIGEST"
                echo "FINAL-DIGEST:         $FINAL"
                echo "CLASS: $CLASS  RECOVERED: $REC  DIGEST-EQUAL: $EQ"
            } > "$D/digests.txt"
        fi
        k=$((k + 1))
    done
    echo "ARM-$ARM CLASS-MID: $MID"
    echo "ARM-$ARM DIGEST-EQUAL-PRE: $EQUAL"
    echo "ARM-$ARM DIGEST-DIFFERS-PRE: $DIFFER"
    echo "ARM-$ARM JOURNAL-RESIDUE: $RESIDUE"
    echo "ARM-$ARM RECOVERED-OPERATIONS: $RECOVERED_TOTAL"
}

# --- arm 1: the property ------------------------------------------------------
run_arm rollback yes yes
R_MID=$MID; R_EQUAL=$EQUAL; R_DIFFER=$DIFFER; R_RESIDUE=$RESIDUE; R_REC=$RECOVERED_TOTAL

# --- arm 2: the known-negative ------------------------------------------------
# The identical kills with the rollback REMOVED. This must produce differing
# digests, or the comparison in arm 1 is a dead instrument.
run_arm norollback yes no
N_MID=$MID; N_EQUAL=$EQUAL; N_DIFFER=$DIFFER

# --- arm 3: recovery must not revert completed work ---------------------------
run_arm nokill no yes
K_MID=$MID; K_REC=$RECOVERED_TOTAL; K_EQUAL=$EQUAL

echo
echo "=== VERDICT INPUTS ==="
echo "ROLLBACK-ARM-MID: $R_MID"
echo "ROLLBACK-ARM-EQUAL: $R_EQUAL / $TRIALS"
echo "ROLLBACK-ARM-DIFFER: $R_DIFFER"
echo "ROLLBACK-ARM-JOURNAL-RESIDUE: $R_RESIDUE"
echo "ROLLBACK-ARM-RECOVERED: $R_REC"
echo "NOROLLBACK-ARM-MID: $N_MID"
echo "NOROLLBACK-ARM-EQUAL: $N_EQUAL"
echo "NOROLLBACK-ARM-DIFFER: $N_DIFFER"
echo "NOKILL-ARM-MID: $K_MID"
echo "NOKILL-ARM-RECOVERED: $K_REC"
echo "NOKILL-ARM-EQUAL-PRE: $K_EQUAL"

# --- gates --------------------------------------------------------------------
# Hazard 1: the kills must have landed mid-apply.
[ "$R_MID" -ge 3 ] \
    || FAIL "only $R_MID of $TRIALS kills landed mid-apply; the window was not exercised"

# Hazard 3, the important one: the known-negative must actually fail.
[ "$N_DIFFER" -ge 1 ] \
    || FAIL "the known-negative arm reported every home identical to PRE without any rollback; the digest comparison cannot report a difference and arm 1 proves nothing"
[ "$N_DIFFER" -ge "$N_MID" ] \
    || FAIL "the known-negative arm had $N_MID mid-apply kills but only $N_DIFFER differing homes"

# The property.
[ "$R_EQUAL" -eq "$TRIALS" ] \
    || FAIL "$R_DIFFER of $TRIALS rolled-back homes were NOT byte-identical to the pre-operation state"
[ "$R_RESIDUE" -eq 0 ] \
    || FAIL "$R_RESIDUE rolled-back homes still carry journal bookkeeping"

# Recovery must not revert work that finished.
[ "$K_REC" -eq 0 ] \
    || FAIL "the no-kill arm recovered $K_REC operations; recovery acted on a COMPLETED import"
[ "$K_EQUAL" -eq 0 ] \
    || FAIL "the no-kill arm left $K_EQUAL homes equal to the PRE state; a completed import was reverted"

echo
echo "PROOF: PASS"
echo "  $R_MID/$TRIALS kills landed mid-apply; all $TRIALS rolled-back homes are"
echo "  byte-identical to the pre-operation digest $PRE_DIGEST, with no journal residue."
echo "  The known-negative arm (same kills, no rollback) differed on $N_DIFFER/$TRIALS,"
echo "  so the comparison can report a difference."
echo "  The no-kill arm recovered 0 operations, so a completed import is not reverted."
