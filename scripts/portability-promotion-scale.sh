#!/bin/sh
# Measure the F26-02 containment contract at real import scale.
#
# The operator question — "would you tolerate this at 540 items?" — is answered
# here by NUMBERS THIS SCRIPT PRINTS, never by an executor's opinion about what
# they saw. Every verdict line below is derived from running the REAL binary
# against a REAL corpus and reading its output, or from reading the source that
# declares a constant. Nothing is transcribed from a document.
#
# Usage: portability-promotion-scale.sh <binary> <corpus-root> <out-dir>
#
# The repository root is taken from $PWD (the script re-runs the paired
# inertness legs through cargo, which needs the workspace), so run it from the
# worktree being measured.
#
# SELF-RED: handed a binary that does not exist, this exits non-zero and says
# why. A measurement script that cannot fail turns every number it prints into
# decoration.

set -u

BIN="${1:-}"
CORPUS="${2:-}"
OUT="${3:-}"
REPO="${PWD}"

if [ -z "$BIN" ] || [ -z "$CORPUS" ] || [ -z "$OUT" ]; then
    echo "usage: $0 <binary> <corpus-root> <out-dir>" >&2
    exit 2
fi
if [ ! -x "$BIN" ]; then
    echo "FAIL: binary '$BIN' does not exist or is not executable — there is nothing to measure" >&2
    exit 3
fi
if [ ! -d "$CORPUS" ]; then
    echo "FAIL: corpus root '$CORPUS' is not a directory — there is nothing to import" >&2
    exit 4
fi
mkdir -p "$OUT" || exit 5

WT="$REPO/crates/wcore-config/src/workspace_trust.rs"
if [ ! -s "$WT" ]; then
    echo "FAIL: cannot read $WT — the ceiling constants would otherwise be restated from memory" >&2
    exit 6
fi

# --- a throwaway home per measurement -------------------------------------
# Each measurement starts from the SAME state. Reusing one home would let an
# earlier promotion change what a later one costs, and the slope would then be
# an artefact of the ordering rather than of the subset size.
fresh_home() {
    d=$(mktemp -d)
    printf '%s' "$d"
}

# Apply an import into a fresh home; echo the home path.
do_import() {
    h="$1"
    WAYLAND_HOME="$h" XDG_DATA_HOME= "$BIN" migrate hermes \
        --home "$CORPUS" --yes >"$OUT/import.txt" 2>"$OUT/import.err"
    return $?
}

H1=$(fresh_home)
do_import "$H1"
IMPORT_RC=$?
if [ "$IMPORT_RC" -ne 0 ]; then
    echo "FAIL: the import exited $IMPORT_RC; measuring its output would be meaningless" >&2
    /usr/bin/sed 's/^/    /' "$OUT/import.err" >&2
    exit 7
fi

# --- the four counts, read from the binary's own accounting line -----------
ACCT=$(/usr/bin/grep -oE 'Accounting: discovered=[0-9]+ imported=[0-9]+ quarantined=[0-9]+ excluded=[0-9]+' "$OUT/import.txt" | /usr/bin/tail -1)
if [ -z "$ACCT" ]; then
    echo "FAIL: the import printed no accounting line; the four counts cannot be measured" >&2
    exit 8
fi
DISC=$(printf '%s' "$ACCT" | /usr/bin/sed -E 's/.*discovered=([0-9]+).*/\1/')
IMP=$(printf '%s' "$ACCT" | /usr/bin/sed -E 's/.*imported=([0-9]+).*/\1/')
QUAR=$(printf '%s' "$ACCT" | /usr/bin/sed -E 's/.*quarantined=([0-9]+).*/\1/')
EXCL=$(printf '%s' "$ACCT" | /usr/bin/sed -E 's/.*excluded=([0-9]+).*/\1/')

SUM=$((IMP + QUAR + EXCL))
if [ "$SUM" -eq "$DISC" ]; then BAL=yes; else BAL=no; fi

# --- what is actually contained, read back from the shipped verb -----------
WAYLAND_HOME="$H1" XDG_DATA_HOME= "$BIN" migrate quarantined >"$OUT/quarantined.txt" 2>&1
IDS=$(/usr/bin/grep -oE '^  • [^ ]+' "$OUT/quarantined.txt" | /usr/bin/sed 's/^  • //')
NIDS=$(printf '%s\n' "$IDS" | /usr/bin/grep -c . || true)

# --- classification breadth, COUNTED in both directions --------------------
#
# Over-broad: a persona, memory note, settings or asset item sitting in
# quarantine. Under-broad: an executable kind that is NOT contained — measured
# two ways, because either alone could miss the case that matters.
DATA_Q=$(printf '%s\n' "$IDS" | /usr/bin/grep -cE '^(persona|memory|memory_note|settings|asset|profile|root_profile):' || true)
[ -z "$DATA_Q" ] && DATA_Q=0

# (a) an item the plan published as executable that never reached the store.
WAYLAND_HOME="$H1" XDG_DATA_HOME= "$BIN" migrate hermes --home "$CORPUS" --json \
    >"$OUT/plan.json" 2>/dev/null
PUB_EXEC=$(/usr/bin/grep -c '"class": "executable"' "$OUT/plan.json" || true)
[ -z "$PUB_EXEC" ] && PUB_EXEC=0
MISSING=0
if [ "$PUB_EXEC" -gt "$NIDS" ]; then
    MISSING=$((PUB_EXEC - NIDS))
fi

# (b) a live MCP server definition in the written config that carries a launch
#     command — the child-process surface, read off the artifact the import
#     actually wrote rather than off a report about it.
LIVE_CMD=0
if [ -s "$H1/config.toml" ]; then
    LIVE_CMD=$(/usr/bin/awk '
        /^\[mcp\.servers\./ { inblk=1; next }
        /^\[/ { inblk=0 }
        inblk && /^command[[:space:]]*=/ { n++ }
        END { print n+0 }
    ' "$H1/config.toml")
fi
EXEC_UNCONTAINED=$((MISSING + LIVE_CMD))

# --- promotion cost at TWO subset sizes ------------------------------------
#
# A slope needs two points. The subsets differ by at least a factor of three
# and the larger is at least half of what was quarantined, so the larger point
# is a realistic promotion rather than a token one. For each, the script COUNTS
# the operator invocations the promotion genuinely costs — it runs them.
promote_cost() {
    want="$1"
    h=$(fresh_home)
    WAYLAND_HOME="$h" XDG_DATA_HOME= "$BIN" migrate hermes \
        --home "$CORPUS" --yes >/dev/null 2>&1
    subset=$(WAYLAND_HOME="$h" XDG_DATA_HOME= "$BIN" migrate quarantined 2>/dev/null |
        /usr/bin/grep -oE '^  • [^ ]+' | /usr/bin/sed 's/^  • //' | /usr/bin/head -n "$want")
    n=$(printf '%s\n' "$subset" | /usr/bin/grep -c . || true)
    if [ "$n" -eq 0 ]; then
        printf '0 0'
        return
    fi
    args=""
    for id in $subset; do args="$args --id $id"; done
    inv=0
    # shellcheck disable=SC2086
    if WAYLAND_HOME="$h" XDG_DATA_HOME= "$BIN" migrate promote $args >/dev/null 2>&1; then
        inv=1
    else
        # The set form failed: fall back to one invocation per item and COUNT
        # them, so an unusable promotion path shows up as a linear cost rather
        # than as a script error.
        for id in $subset; do
            WAYLAND_HOME="$h" XDG_DATA_HOME= "$BIN" migrate promote --id "$id" >/dev/null 2>&1
            inv=$((inv + 1))
        done
    fi
    rm -rf "$h"
    printf '%s %s' "$n" "$inv"
}

SMALL=1
LARGE=$(((NIDS + 1) / 2))
[ "$LARGE" -lt $((SMALL * 3)) ] && LARGE=$((SMALL * 3))
[ "$LARGE" -gt "$NIDS" ] && LARGE="$NIDS"

set -- $(promote_cost "$SMALL")
S_ITEMS="$1"
S_INV="$2"
set -- $(promote_cost "$LARGE")
L_ITEMS="$1"
L_INV="$2"

# The scaling rule is fixed HERE so it is not the script's whim: `bounded` when
# the larger subset's invocation count is no more than twice the smaller's.
SCALING=linear
if [ "$L_INV" -le $((S_INV * 2)) ]; then SCALING=bounded; fi

# --- the ceiling constants, READ OUT of the source at run time -------------
CF=$(/usr/bin/grep -oE 'MAX_EXECUTABLE_FILES: usize = [0-9]+' "$WT" | /usr/bin/grep -oE '[0-9]+$')
CFB=$(/usr/bin/grep -oE 'MAX_EXECUTABLE_FILE_BYTES: u64 = [0-9]+ \* 1024 \* 1024' "$WT" | /usr/bin/grep -oE '^MAX_EXECUTABLE_FILE_BYTES: u64 = [0-9]+' | /usr/bin/grep -oE '[0-9]+$')
CTB=$(/usr/bin/grep -oE 'MAX_EXECUTABLE_TOTAL_BYTES: u64 = [0-9]+ \* 1024 \* 1024' "$WT" | /usr/bin/grep -oE '^MAX_EXECUTABLE_TOTAL_BYTES: u64 = [0-9]+' | /usr/bin/grep -oE '[0-9]+$')
if [ -z "$CF" ] || [ -z "$CFB" ] || [ -z "$CTB" ]; then
    echo "FAIL: could not read all three ceiling constants out of $WT" >&2
    exit 9
fi
CFB=$((CFB * 1024 * 1024))
CTB=$((CTB * 1024 * 1024))

# A real Hermes install carries 540 skill directories (26-01, measured against
# Sean's actual home). Does the existing ceiling refuse that?
REAL_DIRS=540
if [ "$REAL_DIRS" -gt "$CF" ]; then REFUSES=yes; else REFUSES=no; fi

# --- the positive control, RE-RUN rather than transcribed -------------------
#
# "The positive control fired" is produced by running Task 3's paired legs
# again at this commit and reading their result — never by reading Task 3's own
# report of them. That distinction is the difference between checking the proof
# and checking the claim about the proof.
PC=not-fired
if command -v cargo >/dev/null 2>&1; then
    if (cd "$REPO" && cargo nextest run --locked -p wcore-cli --no-fail-fast \
        -E 'test(t20_live_positive_control_same_payload_executes_once_promoted) + test(t19_live_negative_leg_quarantined_payload_does_not_execute)') \
        >"$OUT/positive-control.log" 2>&1; then
        # Both legs green AND both actually ran — an empty filter would
        # otherwise report success having executed nothing.
        RAN=$(/usr/bin/grep -oE '[0-9]+ tests? run' "$OUT/positive-control.log" | /usr/bin/tail -1 | /usr/bin/grep -oE '^[0-9]+')
        [ -n "$RAN" ] && [ "$RAN" -ge 2 ] && PC=fired
    fi
else
    echo "WARN: cargo not on PATH; the positive control could not be RE-RUN" >&2
fi

rm -rf "$H1"

# --- the verdict lines -----------------------------------------------------
echo "SCALE-DISCOVERED: $DISC"
echo "SCALE-IMPORTED: $IMP"
echo "SCALE-QUARANTINED: $QUAR"
echo "SCALE-EXCLUDED: $EXCL"
echo "SCALE-BALANCES: $BAL"
echo "PROMOTE-COST: items=$S_ITEMS invocations=$S_INV"
echo "PROMOTE-COST: items=$L_ITEMS invocations=$L_INV"
echo "PROMOTE-SCALING-RULE: bounded when larger_invocations <= 2 * smaller_invocations"
echo "PROMOTE-SCALING: $SCALING"
echo "CLASSIFY-DATA-QUARANTINED: $DATA_Q"
echo "CLASSIFY-EXEC-UNCONTAINED: $EXEC_UNCONTAINED"
echo "CEILING-REFUSES-REALISTIC: $REFUSES"
echo "CEILING-CONSTANTS: files=$CF file_bytes=$CFB total_bytes=$CTB"
echo "POSITIVE-CONTROL: $PC"
