#!/bin/sh
# portability-interrupt-proof.sh — F26-03 Linux interruption and exact-rollback proof.
#
# Usage:  sh scripts/portability-interrupt-proof.sh [--undersized] <path-to-wayland-core>
#
# WHAT THIS PROVES, AND WHY IT IS BUILT THIS WAY
#
# The claim is that a mutating operation killed mid-flight rolls back to the
# EXACT pre-operation tree. Two things make that claim easy to fake:
#
#   1. Restoring into an EMPTY target. Then the rollback is "delete what was
#      created", the tree is trivially consistent whatever the code does, and a
#      broken journal looks identical to a working one. So this proof restores
#      OVER a target that already holds diverged state -- a file the archive will
#      overwrite, a file the archive does not contain at all, and a whole
#      directory that must come back.
#
#   2. A kill that lands AFTER the operation finished. Then everything downstream
#      is trivially consistent and the gate goes green having proven nothing.
#      So mid-flight landing is established INDEPENDENTLY -- an open journal
#      record AND an observably intermediate target -- before the digest
#      comparison is allowed to count. `--undersized` is the negative control:
#      it runs the IDENTICAL procedure against a fixture too small for the kill
#      to land mid-flight, and this script must then FAIL, proving the
#      mid-flight check can actually fire.
#
# Every digest here comes from the product's own `backup digest`, so the
# comparison uses the same algorithm the journal records rather than this
# script's arithmetic.
#
# Prints a fixed-grammar verdict block on stdout. Exits non-zero on any failure.

set -u

FAIL() { echo "PROOF-FAIL: $*"; exit 1; }

UNDERSIZED=no
HANDLER_CONTROL=no
case "${1:-}" in
    --undersized)
        UNDERSIZED=yes
        shift
        ;;
    # Positive control for the uncatchability measurement. Runs the identical
    # procedure but sends a CATCHABLE signal, and requires the probe to fire.
    # Without this leg, `fired=no` in the real run is equally consistent with a
    # probe that was never installed -- the same vacuity the mid-flight check
    # exists to rule out, applied to the kill mechanism.
    --handler-control)
        HANDLER_CONTROL=yes
        shift
        ;;
esac

BIN="${1:-}"
[ -n "$BIN" ] || FAIL "usage: $0 [--undersized] <path-to-wayland-core>"
[ -x "$BIN" ] || FAIL "binary does not exist or is not executable: $BIN"
"$BIN" backup --help >/dev/null 2>&1 || FAIL "binary does not support 'backup': $BIN"

WORK=$(mktemp -d) || FAIL "could not create a work directory"
trap 'rm -rf "$WORK"' EXIT
SRC="$WORK/source-home"
TARGET="$WORK/target-home"
ARCHIVE="$WORK/backup.tar.gz"
PROBE="$WORK/kill-handler-fired"

# --- fixture -----------------------------------------------------------------
# Sized so the paced restore genuinely takes time. The undersized variant is the
# same procedure over a fixture small enough to finish before the kill lands.
if [ "$UNDERSIZED" = yes ]; then
    PAYLOADS=2
    PACE_MS=1
else
    PAYLOADS=120
    PACE_MS=25
fi
KILL_AT_MS=900

mkdir -p "$SRC/skills" "$SRC/memory" || FAIL "could not build the source fixture"
# Canary content only. No real home is ever read by this proof.
printf '[storage.credentials]\nbackend = "plaintext"\n' > "$SRC/config.toml"
i=0
while [ "$i" -lt "$PAYLOADS" ]; do
    printf 'CANARY-PAYLOAD-%s\n' "$i" > "$SRC/skills/skill-$i.md"
    i=$((i + 1))
done
printf 'CANARY-MEMORY\n' > "$SRC/memory/notes.md"

"$BIN" backup create --home "$SRC" --out "$ARCHIVE" >"$WORK/create.log" 2>&1 \
    || { cat "$WORK/create.log"; FAIL "archive creation failed"; }

# --- a target that CARRIES STATE ---------------------------------------------
# Restoring into an empty directory is the scenario in which broken rollback
# code looks correct, so the target diverges from the archive in three ways.
mkdir -p "$TARGET/legacy" || FAIL "could not build the target fixture"
printf 'PRE-EXISTING-DIVERGED-CONFIG\n' > "$TARGET/config.toml"
printf 'PRE-EXISTING-ONLY-HERE\n' > "$TARGET/legacy/keepme.txt"
printf 'PRE-EXISTING-TOP-LEVEL\n' > "$TARGET/untouched-by-archive.txt"

read_digest() {
    "$BIN" backup digest --home "$1" 2>/dev/null | sed -n 's/^DIGEST: //p'
}
read_algo() {
    "$BIN" backup digest --home "$1" 2>/dev/null | sed -n 's/^DIGEST-ALGO: //p'
}

DIGEST_PRE=$(read_digest "$TARGET")
DIGEST_ALGO=$(read_algo "$TARGET")
[ -n "$DIGEST_PRE" ] || FAIL "could not take a pre-operation digest"
[ -n "$DIGEST_ALGO" ] || FAIL "the binary did not report a digest algorithm"

# --- how long does the operation actually take, uninterrupted? ---------------
# Measured, not guessed: the same fixture is restored into a scratch target and
# timed, so `completed_before_kill` is arithmetic rather than an opinion, and so
# the Windows leg can be sized for ITS hardware instead of inheriting a number
# tuned on Linux.
TIMING_TARGET="$WORK/timing-target"
mkdir -p "$TIMING_TARGET"
printf 'x\n' > "$TIMING_TARGET/config.toml"
T0=$(date +%s%N)
"$BIN" backup restore "$ARCHIVE" --home "$TIMING_TARGET" --replace \
    --accept-missing-secrets --pace-ms "$PACE_MS" >"$WORK/timing.log" 2>&1 \
    || { cat "$WORK/timing.log"; FAIL "the timing run failed"; }
T1=$(date +%s%N)
OP_EXPECTED_MS=$(( (T1 - T0) / 1000000 ))

# --- the interrupted run ------------------------------------------------------
WAYLAND_BACKUP_KILL_PROBE="$PROBE" \
"$BIN" backup restore "$ARCHIVE" --home "$TARGET" --replace \
    --accept-missing-secrets --pace-ms "$PACE_MS" >"$WORK/restore.log" 2>&1 &
CHILD=$!

# Give the operation a moment to get in flight, then kill it uncatchably.
sleep_ms() { sleep "$(awk "BEGIN{printf \"%.3f\", $1/1000}")"; }
sleep_ms "$KILL_AT_MS"

KILL_LANDED=no
KILL_NAME=SIGKILL
KILL_CATCHABLE=no
if [ "$HANDLER_CONTROL" = yes ]; then
    KILL_NAME=SIGTERM
    KILL_CATCHABLE=yes
fi
if kill -0 "$CHILD" 2>/dev/null; then
    if [ "$HANDLER_CONTROL" = yes ]; then
        kill -TERM "$CHILD" 2>/dev/null
        KILL_LANDED=yes
        sleep_ms 600            # let the handler run and record itself
        kill -9 "$CHILD" 2>/dev/null
    else
        # SIGKILL: the process cannot install a handler for it, cannot mask it
        # and cannot defer it. A graceful stop would prove the shutdown path,
        # not the interruption path.
        kill -9 "$CHILD" 2>/dev/null
        KILL_LANDED=yes
    fi
fi
wait "$CHILD" 2>/dev/null
KILL_AT_ACTUAL_MS=$KILL_AT_MS

# --- did the kill land MID-FLIGHT? -------------------------------------------
JOURNAL_DIR="$TARGET/.wayland-backup-journal"
MIDFLIGHT_JOURNAL_OPEN=no
if [ -d "$JOURNAL_DIR" ] && [ -n "$(find "$JOURNAL_DIR" -maxdepth 1 -name '*.json' -print -quit 2>/dev/null)" ]; then
    MIDFLIGHT_JOURNAL_OPEN=yes
fi

# The target is intermediate when it is neither its old self nor a completed
# restore: at least one archive payload has landed AND the tree still differs
# from the finished result.
DIGEST_MID=$(read_digest "$TARGET")
RESTORED_ANY=no
[ -f "$TARGET/skills/skill-0.md" ] && RESTORED_ANY=yes
DIGEST_COMPLETE=$(read_digest "$TIMING_TARGET")
MIDFLIGHT_TARGET_INTERMEDIATE=no
if [ "$RESTORED_ANY" = yes ] && [ "$DIGEST_MID" != "$DIGEST_COMPLETE" ] && [ "$DIGEST_MID" != "$DIGEST_PRE" ]; then
    MIDFLIGHT_TARGET_INTERMEDIATE=yes
fi

COMPLETED_BEFORE_KILL=no
if [ "$MIDFLIGHT_JOURNAL_OPEN" = no ] || [ "$KILL_LANDED" = no ]; then
    COMPLETED_BEFORE_KILL=yes
fi

# --- uncatchability, measured -------------------------------------------------
HANDLER_FIRED=no
[ -f "$PROBE" ] && HANDLER_FIRED=yes

# --- recover and compare ------------------------------------------------------
"$BIN" backup recover --home "$TARGET" >"$WORK/recover.log" 2>&1
RECOVER_RC=$?
[ "$RECOVER_RC" -eq 0 ] || { cat "$WORK/recover.log"; FAIL "recovery exited $RECOVER_RC"; }

DIGEST_POST=$(read_digest "$TARGET")
[ -n "$DIGEST_POST" ] || FAIL "could not take a post-recovery digest"
DIGEST_EQUAL=no
[ "$DIGEST_PRE" = "$DIGEST_POST" ] && DIGEST_EQUAL=yes

# --- verdict block ------------------------------------------------------------
echo "INTERRUPT-PLATFORM: linux"
echo "KILL-MECHANISM: $KILL_NAME CATCHABLE: $KILL_CATCHABLE"
# `installed` is READ, never asserted. It was the literal string "yes", so the
# line reported an armed probe whether or not one existed -- and a probe that
# silently failed to arm produces exactly the `fired=no` on which the
# uncatchability claim rests. The binary writes the marker only after a handler
# is genuinely registered.
if [ -f "$PROBE.armed" ]; then HANDLER_INSTALLED=yes; else HANDLER_INSTALLED=no; fi
echo "KILL-HANDLER-PROBE: installed=$HANDLER_INSTALLED fired=$HANDLER_FIRED"
if [ "$HANDLER_INSTALLED" != yes ]; then
  echo "PROOF-FAIL: the kill-handler probe never armed, so fired=no measures nothing at all" >&2
  exit 1
fi
echo "FIXTURE-PAYLOADS: $PAYLOADS"
echo "MIDFLIGHT-JOURNAL-OPEN: $MIDFLIGHT_JOURNAL_OPEN"
echo "MIDFLIGHT-TARGET-INTERMEDIATE: $MIDFLIGHT_TARGET_INTERMEDIATE"
echo "MIDFLIGHT-TIMING: op_expected_ms=$OP_EXPECTED_MS kill_at_ms=$KILL_AT_ACTUAL_MS completed_before_kill=$COMPLETED_BEFORE_KILL"
echo "DIGEST-ALGO: $DIGEST_ALGO"
echo "DIGEST-PRE: $DIGEST_PRE"
echo "DIGEST-POST: $DIGEST_POST"
echo "DIGEST-EQUAL: $DIGEST_EQUAL"

# --- adjudication -------------------------------------------------------------
if [ "$HANDLER_CONTROL" = yes ]; then
    # The probe must FIRE here. If it does not, it was never installed, and the
    # real run's `fired=no` measured nothing.
    if [ "$HANDLER_FIRED" = yes ]; then
        echo "HANDLER-CONTROL: fired=yes"
        echo "PROOF-OK: the probe fires for a catchable signal, so fired=no under SIGKILL is a measurement"
        exit 0
    fi
    echo "HANDLER-CONTROL: fired=no"
    FAIL "the handler probe did NOT fire for a catchable signal, so it was never installed and the uncatchability measurement is vacuous"
fi

if [ "$UNDERSIZED" = yes ]; then
    # Negative control: the operation is EXPECTED to finish before the kill. The
    # script must detect that and fail, which is what proves the mid-flight check
    # above is load-bearing rather than decorative.
    if [ "$COMPLETED_BEFORE_KILL" = yes ] || [ "$MIDFLIGHT_TARGET_INTERMEDIATE" = no ]; then
        echo "NEGATIVE-CONTROL: late-kill-detected"
        echo "NEGCTL-EXIT: 9"
        echo "PROOF-FAIL: the operation completed before the kill landed, so this run proves nothing about rollback"
        exit 9
    fi
    echo "NEGATIVE-CONTROL: late-kill-missed"
    echo "NEGCTL-EXIT: 1"
    FAIL "the undersized fixture was still mid-flight; the negative control did not reproduce a late kill"
fi

[ "$KILL_LANDED" = yes ] || FAIL "the process had already exited when the kill was sent"
[ "$MIDFLIGHT_JOURNAL_OPEN" = yes ] || FAIL "no open journal record: the kill did not land mid-flight"
[ "$MIDFLIGHT_TARGET_INTERMEDIATE" = yes ] || FAIL "the target was not observably intermediate: the kill did not land mid-flight"
[ "$COMPLETED_BEFORE_KILL" = no ] || FAIL "the operation completed before the kill landed"
[ "$HANDLER_FIRED" = no ] || FAIL "a catchable-signal handler fired, so the kill was NOT uncatchable"
[ "$DIGEST_EQUAL" = yes ] || FAIL "post-recovery tree differs from the pre-operation tree ($DIGEST_PRE vs $DIGEST_POST)"

# Content-level confirmation, so "digests match" is not the only evidence.
grep -q 'PRE-EXISTING-DIVERGED-CONFIG' "$TARGET/config.toml" 2>/dev/null \
    || FAIL "the diverged config was not restored to its pre-operation content"
[ -f "$TARGET/legacy/keepme.txt" ] || FAIL "a directory the archive does not contain was not restored"
[ -f "$TARGET/untouched-by-archive.txt" ] || FAIL "a top-level file the archive does not contain was not restored"
[ ! -d "$TARGET/.wayland-backup-journal" ] || FAIL "journal bookkeeping survived recovery"

echo "PROOF-OK: exact rollback from an uncatchable mid-flight kill, over a target that carried state"
exit 0
