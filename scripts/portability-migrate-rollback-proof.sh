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

# --- INSTRUMENT REPAIR: what `$!` actually names ------------------------------
#
# The first version of this script backgrounded the shell FUNCTION above:
#
#     run_migrate "$H" "$MAIN_CORPUS" > log 2>&1 &
#     PID=$!
#     kill -9 "$PID"
#
# `$!` is then the pid of the SUBSHELL the function body runs in, not the
# product. `kill -9` killed the wrapper and the product ran to completion, so
# every one of 27 trials reported a home that had been fully imported and the
# proof failed for a reason that had nothing to do with rollback. The failure was
# loud only because the byte-identity gate happened to be strict; a proof whose
# gate had been "the home is not corrupt" would have PASSED having never
# interrupted anything.
#
# The repair is to background the product itself — a simple command with an
# environment prefix, whose `$!` is the product's pid — and then to CHECK that,
# rather than trusting the shell's semantics on whichever /bin/sh is present.
launch_migrate_bg() {
    # $1 = home, $2 = corpus, $3 = logfile
    WAYLAND_HOME="$1" WAYLAND_MIGRATE_SCOPE_PROBE=1 \
        "$BIN" migrate "$PEER" --home "$2" --yes --overwrite > "$3" 2>&1 &
}

# PID-TARGETING CONTROL, with the third assertion §6b-ii requires: it is not
# enough that the check passes now, it must be a check the BROKEN version would
# have failed. So both launch mechanisms are measured and compared.
pid_comm() { cat "/proc/$1/comm" 2>/dev/null; }

PC_HOME="$WORK/pidctl-home"; mkdir -p "$PC_HOME"
launch_migrate_bg "$PC_HOME" "$MAIN_CORPUS" "$WORK/pidctl.log"
GOOD_PID=$!
GOOD_COMM=$(pid_comm "$GOOD_PID")
kill -9 "$GOOD_PID" 2>/dev/null; wait "$GOOD_PID" 2>/dev/null

PC_HOME2="$WORK/pidctl-home2"; mkdir -p "$PC_HOME2"
run_migrate "$PC_HOME2" "$MAIN_CORPUS" > "$WORK/pidctl2.log" 2>&1 &
BAD_PID=$!
BAD_COMM=$(pid_comm "$BAD_PID")
kill -9 "$BAD_PID" 2>/dev/null; wait "$BAD_PID" 2>/dev/null
pkill -9 -f "migrate $PEER --home $MAIN_CORPUS" 2>/dev/null

echo "PID-TARGETING-CONTROL: direct-launch comm='$GOOD_COMM' function-launch comm='$BAD_COMM'"
case "$GOOD_COMM" in
    wayland-core*) : ;;
    *) FAIL "the kill target is '$GOOD_COMM', not the product; this proof cannot interrupt anything" ;;
esac
if [ "$GOOD_COMM" = "$BAD_COMM" ]; then
    # Not a failure of the product — a failure of this control to discriminate.
    # If both mechanisms name the same thing the control proves nothing, and a
    # future regression back to the function launch would go unnoticed.
    FAIL "the pid-targeting control cannot tell the two launch mechanisms apart ('$GOOD_COMM'); it would not have caught the defect it exists for"
fi
rm -rf "$PC_HOME" "$PC_HOME2"

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
#
# Trials are counted BY CLASS, because the correct outcome differs by class and
# a single "all trials equal PRE" gate would demand that recovery revert a
# COMPLETED import — the opposite of what it must do:
#
#   interrupted (mid|pre) -> after rollback, byte-identical to PRE
#   completed   (post)    -> after rollback, NOT equal to PRE; the work stands
MID=0; PRECLASS=0; POSTCLASS=0
INT_EQ=0; INT_NEQ=0; COMP_EQ=0; MID_DIFFER=0
RESIDUE=0; RECOVERED_TOTAL=0
run_arm() {
    ARM="$1"; DO_KILL="$2"; DO_ROLLBACK="$3"
    MID=0; PRECLASS=0; POSTCLASS=0
    INT_EQ=0; INT_NEQ=0; COMP_EQ=0; MID_DIFFER=0
    RESIDUE=0; RECOVERED_TOTAL=0
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
            launch_migrate_bg "$H" "$MAIN_CORPUS" "$WORK/$ARM-$k.log"
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
        #
        # An OPEN JOURNAL RECORD is the classifier, not a digest comparison.
        # `config.toml` embeds the absolute home in derived keys, so two homes
        # at different paths never digest equal even after identical, complete
        # imports — a digest-only classifier called every completed run `mid`,
        # which is how the first version of this script mislabelled all 27
        # trials. An open record means the process died inside the window the
        # journal covers, which is exactly the thing being claimed.
        KILL_DIGEST=$(digest_of "$H")
        OPEN_RECORDS=$(find "$H/.wayland-backup-journal" -maxdepth 1 -name '*.json' 2>/dev/null | wc -l | tr -d ' ')
        if [ "${OPEN_RECORDS:-0}" -gt 0 ]; then
            CLASS=mid; MID=$((MID + 1))
        elif [ "$KILL_DIGEST" = "$PRE_DIGEST" ]; then
            CLASS=pre; PRECLASS=$((PRECLASS + 1))
        else
            CLASS=post; POSTCLASS=$((POSTCLASS + 1))
        fi
        [ "$CLASS" = mid ] && [ "$KILL_DIGEST" != "$PRE_DIGEST" ] && MID_DIFFER=$((MID_DIFFER + 1))

        REC=0
        if [ "$DO_ROLLBACK" = yes ]; then
            "$BIN" backup recover --home "$H" > "$WORK/$ARM-$k.recover" 2>&1
            REC=$(sed -n 's/^recovered_operations: //p' "$WORK/$ARM-$k.recover" | head -1)
            REC=${REC:-0}
            RECOVERED_TOTAL=$((RECOVERED_TOTAL + REC))
        fi

        FINAL=$(digest_of "$H")
        if [ "$FINAL" = "$PRE_DIGEST" ]; then EQ=yes; else EQ=no; fi
        case "$CLASS" in
            mid|pre)
                if [ "$EQ" = yes ]; then INT_EQ=$((INT_EQ + 1)); else INT_NEQ=$((INT_NEQ + 1)); fi ;;
            post)
                [ "$EQ" = yes ] && COMP_EQ=$((COMP_EQ + 1)) ;;
        esac

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
    echo "ARM-$ARM CLASS-PRE: $PRECLASS"
    echo "ARM-$ARM CLASS-POST: $POSTCLASS"
    echo "ARM-$ARM INTERRUPTED-EQUAL-PRE: $INT_EQ"
    echo "ARM-$ARM INTERRUPTED-DIFFERS-PRE: $INT_NEQ"
    echo "ARM-$ARM COMPLETED-REVERTED-TO-PRE: $COMP_EQ"
    echo "ARM-$ARM MID-DAMAGED-BEFORE-ROLLBACK: $MID_DIFFER"
    echo "ARM-$ARM JOURNAL-RESIDUE: $RESIDUE"
    echo "ARM-$ARM RECOVERED-OPERATIONS: $RECOVERED_TOTAL"
}

# --- arm 1: the property ------------------------------------------------------
run_arm rollback yes yes
R_MID=$MID; R_INT_EQ=$INT_EQ; R_INT_NEQ=$INT_NEQ; R_COMP_EQ=$COMP_EQ
R_RESIDUE=$RESIDUE; R_REC=$RECOVERED_TOTAL

# --- arm 2: the known-negative ------------------------------------------------
# The identical kills with the rollback REMOVED. A mid-apply kill must leave the
# home DIFFERENT from PRE, or the comparison in arm 1 is a dead instrument that
# would report "identical" for anything.
run_arm norollback yes no
N_MID=$MID; N_MID_DIFFER=$MID_DIFFER; N_INT_EQ=$INT_EQ

# --- arm 3: recovery must not revert completed work ---------------------------
run_arm nokill no yes
K_MID=$MID; K_POST=$POSTCLASS; K_REC=$RECOVERED_TOTAL; K_COMP_EQ=$COMP_EQ

echo
echo "=== VERDICT INPUTS ==="
echo "ROLLBACK-ARM-MID: $R_MID"
echo "ROLLBACK-ARM-INTERRUPTED-EQUAL-PRE: $R_INT_EQ"
echo "ROLLBACK-ARM-INTERRUPTED-DIFFERS-PRE: $R_INT_NEQ"
echo "ROLLBACK-ARM-COMPLETED-REVERTED: $R_COMP_EQ"
echo "ROLLBACK-ARM-JOURNAL-RESIDUE: $R_RESIDUE"
echo "ROLLBACK-ARM-RECOVERED: $R_REC"
echo "NOROLLBACK-ARM-MID: $N_MID"
echo "NOROLLBACK-ARM-MID-DAMAGED: $N_MID_DIFFER"
echo "NOROLLBACK-ARM-INTERRUPTED-EQUAL-PRE: $N_INT_EQ"
echo "NOKILL-ARM-MID: $K_MID"
echo "NOKILL-ARM-POST: $K_POST"
echo "NOKILL-ARM-RECOVERED: $K_REC"
echo "NOKILL-ARM-COMPLETED-REVERTED: $K_COMP_EQ"

# --- gates --------------------------------------------------------------------
# Hazard 1: the kills must have landed mid-apply.
[ "$R_MID" -ge 3 ] \
    || FAIL "only $R_MID of $TRIALS kills landed mid-apply; the window was not exercised"
[ "$R_REC" -ge "$R_MID" ] \
    || FAIL "$R_MID trials were killed mid-apply but only $R_REC operations were recovered"

# Hazard 3, the important one: the known-negative must actually fail.
[ "$N_MID" -ge 1 ] \
    || FAIL "the known-negative arm landed no mid-apply kills, so it cannot demonstrate anything"
[ "$N_MID_DIFFER" -eq "$N_MID" ] \
    || FAIL "the known-negative arm had $N_MID mid-apply kills but only $N_MID_DIFFER left the home differing from PRE; the digest comparison cannot report a difference and arm 1 proves nothing"

# The property: every INTERRUPTED trial comes back byte-identical.
[ "$R_INT_NEQ" -eq 0 ] \
    || FAIL "$R_INT_NEQ interrupted homes were NOT byte-identical to the pre-operation state after rollback"
[ "$R_INT_EQ" -ge "$R_MID" ] \
    || FAIL "fewer homes came back to PRE ($R_INT_EQ) than were interrupted mid-apply ($R_MID)"
[ "$R_RESIDUE" -eq 0 ] \
    || FAIL "$R_RESIDUE rolled-back homes still carry journal bookkeeping"

# Recovery must not revert work that finished.
[ "$R_COMP_EQ" -eq 0 ] \
    || FAIL "$R_COMP_EQ COMPLETED imports were reverted to the pre-operation state by the rollback arm"
[ "$K_POST" -eq "$TRIALS" ] \
    || FAIL "the no-kill arm did not complete every import ($K_POST of $TRIALS reached post)"
[ "$K_REC" -eq 0 ] \
    || FAIL "the no-kill arm recovered $K_REC operations; recovery acted on a COMPLETED import"
[ "$K_COMP_EQ" -eq 0 ] \
    || FAIL "the no-kill arm reverted $K_COMP_EQ completed imports to the PRE state"

echo
echo "PROOF: PASS"
echo "  $R_MID/$TRIALS kills landed mid-apply (open journal record observed);"
echo "  all $R_INT_EQ interrupted homes came back byte-identical to the pre-operation"
echo "  digest $PRE_DIGEST, with no journal residue and $R_REC operations recovered."
echo "  Known-negative arm: $N_MID_DIFFER/$N_MID mid-apply kills left the home DIFFERENT"
echo "  from PRE without a rollback, so the comparison can report a difference."
echo "  No-kill arm: $K_POST/$TRIALS imports completed, 0 recovered, 0 reverted."
