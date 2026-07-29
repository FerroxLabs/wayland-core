#!/bin/sh
# portability-backup-rollback-sweep.sh
#
# SC3 for the OTHER two nouns: `backup` and `restore`, swept across MANY
# interruption points rather than one.
#
# `portability-interrupt-proof.sh` already proves exact rollback from a single
# uncatchable mid-flight kill, and it does that carefully — handler probe,
# mid-flight liveness checks, an undersized negative control. What it cannot
# show is whether the property holds ACROSS the operation, and the interesting
# failures cluster in specific windows: the gap between the intent record and
# the undo store being complete, the clear-target window where the home is
# emptiest, the first payload, the last payload, and the config rewrite.
#
# So this sweeps the kill across the measured window and requires byte-identity
# at every point.
#
# It also covers the noun `backup` itself, which the interruption work had left
# implicit: a killed `backup create` must leave the SOURCE HOME untouched (it is
# a read-only operation on the home, but that is a claim until measured) and
# must leave either no archive or one that verifies — never a corpse that
# verifies as valid or blocks the retry.
#
# The controls are the same three the migrate rollback proof uses, for the same
# reasons:
#   * arm `rollback`   — the property;
#   * arm `norollback` — identical kills, recovery removed. MUST differ, or the
#                        digest comparison is a dead instrument;
#   * arm `nokill`     — runs to completion. Recovery must find NOTHING.
#
# Usage: portability-backup-rollback-sweep.sh [--trials N] [--pace-ms N]
#                                             [--evidence DIR] <bin>
set -u

FAIL() { echo "PROOF: FAIL — $*"; exit 1; }

TRIALS=9
PACE_MS=40
EVIDENCE=""
BIN=""
while [ $# -gt 0 ]; do
    case "$1" in
        --trials)   TRIALS="$2"; shift 2 ;;
        --pace-ms)  PACE_MS="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        -*)         FAIL "unknown option $1" ;;
        *)          BIN="$1"; shift ;;
    esac
done
[ -n "$BIN" ] || FAIL "usage: $0 [--trials N] [--pace-ms N] [--evidence DIR] <bin>"
[ -x "$BIN" ] || FAIL "not executable: $BIN"
"$BIN" backup --help >/dev/null 2>&1 || FAIL "binary does not support 'backup'"

echo "=== backup/restore ROLLBACK sweep (SC3, multi-point) ==="
echo "BIN: $BIN"
echo "BIN-SHA256: $(sha256sum "$BIN" 2>/dev/null | cut -d' ' -f1)"
echo "TRIALS-PER-ARM: $TRIALS"
echo "PACE-MS: $PACE_MS"
echo "DATE-UTC: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "HOST: $(hostname)"

WORK=$(mktemp -d) || FAIL "could not create a work directory"
trap 'rm -rf "$WORK"' EXIT
[ -n "$EVIDENCE" ] && mkdir -p "$EVIDENCE"

digest_of() { "$BIN" backup digest --home "$1" 2>/dev/null | sed -n 's/^DIGEST: //p'; }

# --- the archive to restore ---------------------------------------------------
SRC="$WORK/src"
mkdir -p "$SRC/skills/a" "$SRC/skills/b" "$SRC/memory"
printf '[storage]\nx = 1\n' > "$SRC/config.toml"
i=1
while [ "$i" -le 40 ]; do
    printf 'ARCHIVED BODY %d\n' "$i" > "$SRC/skills/a/s$i.md"
    printf 'ARCHIVED OTHER %d\n' "$i" > "$SRC/skills/b/s$i.md"
    i=$((i + 1))
done
printf 'ARCHIVED MEMORY\n' > "$SRC/memory/notes.md"
ARC="$WORK/backup.tar.gz"
"$BIN" backup create --home "$SRC" --out "$ARC" > "$WORK/create.log" 2>&1 \
    || { cat "$WORK/create.log"; FAIL "archive creation failed"; }
ARC_PAYLOADS=$(sed -n 's/^payloads: //p' "$WORK/create.log" | head -1)
echo "ARCHIVE-PAYLOADS: $ARC_PAYLOADS"
[ "${ARC_PAYLOADS:-0}" -ge 40 ] || FAIL "archive carries ${ARC_PAYLOADS} payloads; too few to interrupt meaningfully"

# --- the live home the restore replaces ---------------------------------------
# DIVERGED on purpose: files the archive will overwrite, files it knows nothing
# about, and a directory it does not contain. Restoring into an empty directory
# would have nothing to lose and would prove nothing.
TEMPLATE="$WORK/template"
mkdir -p "$TEMPLATE/legacy" "$TEMPLATE/skills/a"
printf '[storage]\nx = 999\nLIVE = true\n' > "$TEMPLATE/config.toml"
printf 'LIVE ONLY, NOT IN THE ARCHIVE\n' > "$TEMPLATE/legacy/keepme.txt"
printf 'TOP LEVEL LIVE FILE\n' > "$TEMPLATE/untouched-by-archive.txt"
printf 'LIVE VERSION OF AN ARCHIVED PATH\n' > "$TEMPLATE/skills/a/s1.md"

PRE_DIGEST=$(digest_of "$TEMPLATE")
[ -n "$PRE_DIGEST" ] || FAIL "could not take the pre-operation digest"
echo "PRE-OPERATION-DIGEST: $PRE_DIGEST"

CPCHK="$WORK/copy-check"
cp -a "$TEMPLATE" "$CPCHK" || FAIL "template copy failed"
[ "$(digest_of "$CPCHK")" = "$PRE_DIGEST" ] \
    || FAIL "a copy of the template does not digest equal to it; the comparand is unusable"
echo "COPY-FIDELITY-CONTROL: pass"
rm -rf "$CPCHK"

run_restore() {
    "$BIN" backup restore "$ARC" --home "$1" --replace --pace-ms "$PACE_MS"
}

# --- INSTRUMENT REPAIR: what `$!` actually names ------------------------------
#
# Backgrounding the shell FUNCTION above makes `$!` the pid of the SUBSHELL the
# body runs in, not the product; `kill -9` then kills the wrapper and the
# product runs to completion. Measured 2026-07-29 in the migrate peer of this
# proof: 27 of 27 trials reported a fully-completed operation while the script
# believed it had interrupted every one. The repair is to background the product
# itself and then CHECK the pid names it.
launch_restore_bg() {
    # $1 = home, $2 = logfile
    "$BIN" backup restore "$ARC" --home "$1" --replace --pace-ms "$PACE_MS" > "$2" 2>&1 &
}

pid_comm() { cat "/proc/$1/comm" 2>/dev/null; }

PC1="$WORK/pidctl1"; cp -a "$TEMPLATE" "$PC1"
launch_restore_bg "$PC1" "$WORK/pidctl1.log"
GOOD_PID=$!
GOOD_COMM=$(pid_comm "$GOOD_PID")
kill -9 "$GOOD_PID" 2>/dev/null; wait "$GOOD_PID" 2>/dev/null

PC2="$WORK/pidctl2"; cp -a "$TEMPLATE" "$PC2"
run_restore "$PC2" > "$WORK/pidctl2.log" 2>&1 &
BAD_PID=$!
BAD_COMM=$(pid_comm "$BAD_PID")
kill -9 "$BAD_PID" 2>/dev/null; wait "$BAD_PID" 2>/dev/null
pkill -9 -f "backup restore $ARC" 2>/dev/null

echo "PID-TARGETING-CONTROL: direct-launch comm='$GOOD_COMM' function-launch comm='$BAD_COMM'"
case "$GOOD_COMM" in
    wayland-core*) : ;;
    *) FAIL "the kill target is '$GOOD_COMM', not the product; this proof cannot interrupt anything" ;;
esac
[ "$GOOD_COMM" != "$BAD_COMM" ] \
    || FAIL "the pid-targeting control cannot tell the two launch mechanisms apart ('$GOOD_COMM'); it would not have caught the defect it exists for"
rm -rf "$PC1" "$PC2"

# --- reference timing ---------------------------------------------------------
REF="$WORK/ref"
cp -a "$TEMPLATE" "$REF" || FAIL "reference copy failed"
T0=$(date +%s%N)
run_restore "$REF" > "$WORK/ref.log" 2>&1
REF_RC=$?
T1=$(date +%s%N)
[ "$REF_RC" -eq 0 ] || { sed -n '1,30p' "$WORK/ref.log"; FAIL "the reference restore failed (rc=$REF_RC)"; }
DUR_MS=$(( (T1 - T0) / 1000000 ))
[ "$DUR_MS" -gt 0 ] || DUR_MS=1
POST_DIGEST=$(digest_of "$REF")
echo "REFERENCE-DURATION-MS: $DUR_MS"
echo "POST-RESTORE-DIGEST: $POST_DIGEST"
[ "$POST_DIGEST" != "$PRE_DIGEST" ] \
    || FAIL "a completed restore did not change the home; PRE and POST are identical, so no comparison here can fail"
echo "MUTATION-CONTROL: pass (PRE != POST)"

# --- trial machinery ----------------------------------------------------------
MID=0; PRECLASS=0; POSTCLASS=0; INT_EQ=0; INT_NEQ=0; COMP_EQ=0; MID_DIFFER=0
RESIDUE=0; RECOVERED_TOTAL=0; DIFF_UNEXPLAINED=0
run_arm() {
    ARM="$1"; DO_KILL="$2"; DO_ROLLBACK="$3"
    MID=0; PRECLASS=0; POSTCLASS=0; INT_EQ=0; INT_NEQ=0; COMP_EQ=0; MID_DIFFER=0
    RESIDUE=0; RECOVERED_TOTAL=0; DIFF_UNEXPLAINED=0
    echo
    echo "--- ARM: $ARM (kill=$DO_KILL rollback=$DO_ROLLBACK) ---"
    echo "TRIAL-TABLE[$ARM]: trial delay_ms class recovered digest_equal journal_residue"
    k=1
    while [ "$k" -le "$TRIALS" ]; do
        H="$WORK/$ARM-$k"
        rm -rf "$H"
        cp -a "$TEMPLATE" "$H" || FAIL "trial copy failed"
        DELAY_MS=$(( DUR_MS * k / (TRIALS + 1) ))

        if [ "$DO_KILL" = yes ]; then
            launch_restore_bg "$H" "$WORK/$ARM-$k.log"
            PID=$!
            python3 -c "import time,sys; time.sleep(float(sys.argv[1])/1000.0)" "$DELAY_MS"
            kill -9 "$PID" 2>/dev/null
            wait "$PID" 2>/dev/null
        else
            run_restore "$H" > "$WORK/$ARM-$k.log" 2>&1
        fi

        # An OPEN JOURNAL RECORD is the mid-flight classifier, not a digest
        # comparison: it states directly that the process died inside the window
        # the journal covers, which is the thing being claimed.
        KILL_DIGEST=$(digest_of "$H")
        OPEN_RECORDS=$(find "$H/.wayland-backup-journal" -maxdepth 1 -name '*.json' 2>/dev/null | wc -l | tr -d ' ')
        if [ "${OPEN_RECORDS:-0}" -gt 0 ]; then CLASS=mid; MID=$((MID + 1))
        elif [ "$KILL_DIGEST" = "$PRE_DIGEST" ]; then CLASS=pre; PRECLASS=$((PRECLASS + 1))
        else CLASS=post; POSTCLASS=$((POSTCLASS + 1)); fi
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
            mid|pre) if [ "$EQ" = yes ]; then INT_EQ=$((INT_EQ + 1)); else INT_NEQ=$((INT_NEQ + 1)); fi ;;
            post)    [ "$EQ" = yes ] && COMP_EQ=$((COMP_EQ + 1)) ;;
        esac
        if [ -d "$H/.wayland-backup-journal" ]; then RES=yes; RESIDUE=$((RESIDUE + 1)); else RES=no; fi

        # INDEPENDENT byte-level check: the digest excludes Wayland bookkeeping
        # by design, so it cannot be the only evidence or the exclusion list
        # becomes a place to hide a difference.
        diff -rq "$TEMPLATE" "$H" > "$WORK/$ARM-$k.diff" 2>&1
        UNEXPLAINED=$(grep -v -e '\.wayland-backup-journal' -e '\.dirty-death' < "$WORK/$ARM-$k.diff" | grep -c . )
        if [ "$CLASS" != post ] && [ "${UNEXPLAINED:-0}" -gt 0 ]; then
            DIFF_UNEXPLAINED=$((DIFF_UNEXPLAINED + 1))
        fi

        printf 'TRIAL[%s]: %d %d %s %s %s %s\n' "$ARM" "$k" "$DELAY_MS" "$CLASS" "$REC" "$EQ" "$RES"

        if [ -n "$EVIDENCE" ] && [ "$CLASS" = mid ] && [ "$k" -le 3 ]; then
            D="$EVIDENCE/$ARM-trial-$k"; mkdir -p "$D"
            cp "$WORK/$ARM-$k.log" "$D/kill-run.log" 2>/dev/null
            cp "$WORK/$ARM-$k.recover" "$D/recover.log" 2>/dev/null
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
    echo "ARM-$ARM BYTE-DIFF-UNEXPLAINED: $DIFF_UNEXPLAINED"
}

run_arm rollback yes yes
R_MID=$MID; R_INT_EQ=$INT_EQ; R_INT_NEQ=$INT_NEQ; R_COMP_EQ=$COMP_EQ
R_RESIDUE=$RESIDUE; R_REC=$RECOVERED_TOTAL; R_DIFFU=$DIFF_UNEXPLAINED

run_arm norollback yes no
N_MID=$MID; N_MID_DIFFER=$MID_DIFFER

run_arm nokill no yes
K_POST=$POSTCLASS; K_REC=$RECOVERED_TOTAL; K_COMP_EQ=$COMP_EQ

# --- the noun `backup` itself -------------------------------------------------
# A killed `backup create` must leave the source home untouched, and must leave
# either NO archive or one that verifies. A truncated archive that passed
# verification would be the worst outcome this family can produce, because the
# operator would then hold a backup that is not one.
echo
echo "--- ARM: create (kill mid-archive) ---"
CREATE_HOME_MOVED=0; CREATE_BAD_ARCHIVE=0; CREATE_MID=0
SRC_DIGEST=$(digest_of "$SRC")
k=1
while [ "$k" -le "$TRIALS" ]; do
    OUT="$WORK/killed-$k.tar.gz"
    rm -f "$OUT"
    "$BIN" backup create --home "$SRC" --out "$OUT" > "$WORK/create-$k.log" 2>&1 &
    PID=$!
    # `create` is fast, so sweep sub-millisecond to millisecond delays: the point
    # is to land inside it at all, and a spread of delays is how.
    python3 -c "import time,sys; time.sleep(float(sys.argv[1])/1000.0)" "$k"
    kill -9 "$PID" 2>/dev/null
    wait "$PID" 2>/dev/null

    NOW=$(digest_of "$SRC")
    [ "$NOW" = "$SRC_DIGEST" ] || CREATE_HOME_MOVED=$((CREATE_HOME_MOVED + 1))

    if [ -f "$OUT" ]; then
        if "$BIN" backup verify "$OUT" > "$WORK/verify-$k.log" 2>&1; then
            VERD=verifies
        else
            VERD=rejected
            CREATE_BAD_ARCHIVE=$((CREATE_BAD_ARCHIVE + 1))
        fi
    else
        VERD=absent; CREATE_MID=$((CREATE_MID + 1))
    fi
    printf 'TRIAL[create]: %d delay_ms=%d home_moved=%s archive=%s\n' \
        "$k" "$k" "$([ "$NOW" = "$SRC_DIGEST" ] && echo no || echo YES)" "$VERD"
    k=$((k + 1))
done
echo "ARM-create SOURCE-HOME-MOVED: $CREATE_HOME_MOVED"
echo "ARM-create PARTIAL-ARCHIVES-ON-DISK: $CREATE_BAD_ARCHIVE"
echo "ARM-create NO-ARCHIVE-PUBLISHED: $CREATE_MID"

echo
echo "=== VERDICT INPUTS ==="
echo "ROLLBACK-ARM-MID: $R_MID"
echo "ROLLBACK-ARM-INTERRUPTED-EQUAL-PRE: $R_INT_EQ"
echo "ROLLBACK-ARM-INTERRUPTED-DIFFERS-PRE: $R_INT_NEQ"
echo "ROLLBACK-ARM-COMPLETED-REVERTED: $R_COMP_EQ"
echo "ROLLBACK-ARM-JOURNAL-RESIDUE: $R_RESIDUE"
echo "ROLLBACK-ARM-RECOVERED: $R_REC"
echo "ROLLBACK-ARM-BYTE-DIFF-UNEXPLAINED: $R_DIFFU"
echo "NOROLLBACK-ARM-MID: $N_MID"
echo "NOROLLBACK-ARM-MID-DAMAGED: $N_MID_DIFFER"
echo "NOKILL-ARM-POST: $K_POST"
echo "NOKILL-ARM-RECOVERED: $K_REC"
echo "NOKILL-ARM-COMPLETED-REVERTED: $K_COMP_EQ"
echo "CREATE-ARM-SOURCE-MOVED: $CREATE_HOME_MOVED"
echo "CREATE-ARM-UNVERIFIABLE-ARCHIVES: $CREATE_BAD_ARCHIVE"

# --- gates --------------------------------------------------------------------
[ "$R_MID" -ge 3 ] \
    || FAIL "only $R_MID of $TRIALS kills landed mid-restore; the window was not exercised"
[ "$R_REC" -ge "$R_MID" ] \
    || FAIL "$R_MID trials were killed mid-restore but only $R_REC operations were recovered"
[ "$N_MID" -ge 1 ] \
    || FAIL "the known-negative arm landed no mid-restore kills, so it cannot demonstrate anything"
[ "$N_MID_DIFFER" -eq "$N_MID" ] \
    || FAIL "the known-negative arm had $N_MID mid-restore kills but only $N_MID_DIFFER left the home differing from PRE; the digest comparison cannot report a difference"
[ "$R_INT_NEQ" -eq 0 ] \
    || FAIL "$R_INT_NEQ interrupted homes were NOT byte-identical to the pre-operation state after rollback"
[ "$R_DIFFU" -eq 0 ] \
    || FAIL "$R_DIFFU interrupted homes differ from the template by something OTHER than Wayland bookkeeping"
[ "$R_RESIDUE" -eq 0 ] \
    || FAIL "$R_RESIDUE rolled-back homes still carry journal bookkeeping"
[ "$R_COMP_EQ" -eq 0 ] \
    || FAIL "$R_COMP_EQ COMPLETED restores were reverted to the pre-operation state"
[ "$K_POST" -eq "$TRIALS" ] \
    || FAIL "the no-kill arm did not complete every restore ($K_POST of $TRIALS)"
[ "$K_REC" -eq 0 ] \
    || FAIL "the no-kill arm recovered $K_REC operations; recovery acted on a COMPLETED restore"
[ "$K_COMP_EQ" -eq 0 ] \
    || FAIL "the no-kill arm reverted $K_COMP_EQ completed restores to the PRE state"
[ "$CREATE_HOME_MOVED" -eq 0 ] \
    || FAIL "$CREATE_HOME_MOVED killed 'backup create' runs moved the SOURCE home; create is not read-only"
[ "$CREATE_BAD_ARCHIVE" -eq 0 ] \
    || FAIL "$CREATE_BAD_ARCHIVE killed 'backup create' runs published an archive that does not verify"

echo
echo "PROOF: PASS"
echo "  restore: $R_MID/$TRIALS kills landed mid-flight (open journal record); all $R_INT_EQ"
echo "  interrupted homes came back byte-identical to $PRE_DIGEST, 0 unexplained byte diffs,"
echo "  no journal residue, $R_REC operations recovered. Known-negative: $N_MID_DIFFER/$N_MID"
echo "  mid-flight kills left the home DIFFERENT from PRE without a rollback."
echo "  No-kill arm: $K_POST/$TRIALS completed, 0 recovered, 0 reverted."
echo "  backup: $TRIALS kills, source home moved 0 times, 0 unverifiable archives published."
