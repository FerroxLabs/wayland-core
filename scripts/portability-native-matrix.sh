#!/bin/sh
# F26-05 — the POSIX half of the cross-platform determinism and isolation proof.
#
# Usage: portability-native-matrix.sh <wayland-core-binary> <portable-report-out>
#
# Writes TWO files:
#   <report>            the PORTABLE report. Byte-compared against the Windows
#                       run. Carries only claims that must hold identically on
#                       both platforms.
#   <report>.platform   the PLATFORM report. Carries the cases whose corpus
#                       DIFFERS by construction on this filesystem (case-only
#                       and normal-form-only name collisions, and the Windows
#                       name hazards). These are recorded per platform and are
#                       never cross-compared, because comparing them would be
#                       comparing two different corpora and calling the
#                       difference a defect.
#
# WHY THE SPLIT IS NOT A LOOSENED COMPARISON
# ------------------------------------------
# A case-only name collision is TWO items on Linux and ONE on Windows. That is
# the filesystem, not the product. Putting it in the byte-compared report would
# guarantee a diff that says nothing about determinism, and the honest response
# to that diff would be to loosen the comparison — which this phase forbids. So
# the platform-variant cases are asserted SEPARATELY on each platform against
# their own declared outcome (in the .platform file, and in
# crates/wcore-cli/tests/portability_hostile_corpus.rs), and the byte-compared
# report carries the platform-invariant surface, in full, including each case's
# corpus digest — so byte equality also proves the two INDEPENDENT materialisers
# built identical corpora, rather than merely both having run.
#
# SELF-RED: handed a binary that does not exist this exits non-zero. A matrix
# script that cannot go red produces two identical reports that prove nothing.

set -u

BIN="${1:-}"
REPORT="${2:-}"

die() {
    echo "MATRIX-FAIL: $*" >&2
    exit 2
}

[ -n "$BIN" ] && [ -n "$REPORT" ] || die "usage: $0 <binary> <report-out>"
[ -f "$BIN" ] || die "binary '$BIN' does not exist"
[ -x "$BIN" ] || die "binary '$BIN' is not executable"

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO=$(dirname "$HERE")
GEN="$REPO/scripts/portability-hostile-gen.py"
SPEC="$REPO/crates/wcore-cli/tests/fixtures/portability-hostile/corpus-spec.json"
[ -f "$GEN" ] || die "the hostile generator is missing at $GEN"
[ -f "$SPEC" ] || die "the committed corpus spec is missing at $SPEC"

command -v python3 >/dev/null 2>&1 || die "python3 is required to materialise the corpora"
command -v shasum >/dev/null 2>&1 && SHACMD="shasum -a 256" || SHACMD="sha256sum"

WORK=$(mktemp -d) || die "cannot create a work directory"
trap 'rm -rf "$WORK"' EXIT

# --- 1. the spec, and proof that the committed copy has not drifted --------
python3 "$GEN" --emit-spec "$WORK/spec.json" >/dev/null 2>"$WORK/spec.err" ||
    die "spec emission failed: $(cat "$WORK/spec.err")"
if ! /usr/bin/diff -q "$WORK/spec.json" "$SPEC" >/dev/null 2>&1; then
    echo "MATRIX-FAIL: the committed corpus spec has drifted from the generator." >&2
    /usr/bin/diff "$SPEC" "$WORK/spec.json" >&2
    echo "The Windows leg materialises from the COMMITTED spec, so a drift here" >&2
    echo "means the two platforms would build different corpora and the byte" >&2
    echo "comparison would be measuring the drift rather than the product." >&2
    exit 3
fi
SPEC_SHA=$($SHACMD "$SPEC" | /usr/bin/awk '{print $1}')

# --- 2. materialise every corpus on THIS platform, at run time -------------
python3 "$GEN" --out "$WORK/corpora" >"$WORK/gen.log" 2>&1 ||
    { cat "$WORK/gen.log" >&2; die "corpus materialisation failed"; }

# --- 3. the isolation sentinel, OUTSIDE every target home ------------------
SENTINEL="$WORK/sentinel"
mkdir -p "$SENTINEL/nested/deeper"
printf 'sentinel-value-do-not-touch\n' > "$SENTINEL/credentials.toml"
printf 'sentinel = true\n' > "$SENTINEL/nested/config.toml"
printf 'sentinel skill body\n' > "$SENTINEL/nested/deeper/SKILL.md"

"$BIN" backup digest --home "$SENTINEL" > "$WORK/sentinel-before.txt" 2>"$WORK/sentinel-before.err"
RC=$?
[ $RC -eq 0 ] || { cat "$WORK/sentinel-before.err" >&2; die "backup digest failed on the sentinel (rc=$RC)"; }
DIGEST_ALGO=$(/usr/bin/grep '^DIGEST-ALGO: ' "$WORK/sentinel-before.txt" | /usr/bin/sed 's/^DIGEST-ALGO: //')
SENTINEL_BEFORE=$(/usr/bin/grep '^DIGEST: ' "$WORK/sentinel-before.txt" | /usr/bin/sed 's/^DIGEST: //')
[ -n "$SENTINEL_BEFORE" ] || die "no sentinel digest was produced"

CANARIES=$(python3 - "$WORK/corpora/cases.json" <<'PY'
import json, sys
print("\n".join(json.load(open(sys.argv[1]))["canaries"]))
PY
)

# --- 4. run every case ------------------------------------------------------
PORTABLE="$WORK/portable.txt"
PLATFORM="$WORK/platform.txt"
: > "$PORTABLE"
: > "$PLATFORM"
MISMATCH=0

CASE_IDS=$(python3 - "$WORK/corpora/cases.json" <<'PY'
import json, sys
for c in json.load(open(sys.argv[1]))["cases"]:
    print(c["id"])
PY
)

for ID in $CASE_IDS; do
    META=$(python3 - "$WORK/corpora/cases.json" "$ID" <<'PY'
import json, sys
m = json.load(open(sys.argv[1]))
c = next(x for x in m["cases"] if x["id"] == sys.argv[2])
print(c["class"]); print(c["expect"]); print(c["scope"])
print(c["corpus_digest"]); print(c["corpus"])
print("yes" if c["collapsed"] else "no")
PY
)
    KLASS=$(printf '%s\n' "$META" | /usr/bin/sed -n 1p)
    EXPECT=$(printf '%s\n' "$META" | /usr/bin/sed -n 2p)
    SCOPE=$(printf '%s\n' "$META" | /usr/bin/sed -n 3p)
    CDIGEST=$(printf '%s\n' "$META" | /usr/bin/sed -n 4p)
    CORPUS=$(printf '%s\n' "$META" | /usr/bin/sed -n 5p)
    COLLAPSED=$(printf '%s\n' "$META" | /usr/bin/sed -n 6p)

    HOME_DIR="$WORK/homes/$ID"
    mkdir -p "$HOME_DIR"
    OUT="$WORK/run-$ID.out"
    ERR="$WORK/run-$ID.err"
    ( WAYLAND_HOME="$HOME_DIR" HOME="$HOME_DIR" TERM=dumb \
      "$BIN" migrate hermes --home "$CORPUS" --yes ) > "$OUT" 2> "$ERR"
    EXIT=$?

    ACC=$(/usr/bin/grep '^Accounting: ' "$OUT" 2>/dev/null | /usr/bin/head -1)
    if [ -n "$ACC" ]; then
        D=$(printf '%s\n' "$ACC" | /usr/bin/sed -E 's/.*discovered=([0-9]+).*/\1/')
        I=$(printf '%s\n' "$ACC" | /usr/bin/sed -E 's/.*imported=([0-9]+).*/\1/')
        Q=$(printf '%s\n' "$ACC" | /usr/bin/sed -E 's/.*quarantined=([0-9]+).*/\1/')
        X=$(printf '%s\n' "$ACC" | /usr/bin/sed -E 's/.*excluded=([0-9]+).*/\1/')
        if [ "$((I + Q + X))" -eq "$D" ]; then BAL=yes; else BAL=no; fi
        PRESENT=present
    else
        D=-1; I=-1; Q=-1; X=-1; BAL=na; PRESENT=absent
    fi

    # Canary hits: every corpus secret is synthetic, and none may appear in
    # anything the product emitted.
    HITS=0
    for C in $CANARIES; do
        if /usr/bin/grep -qF -- "$C" "$OUT" 2>/dev/null || /usr/bin/grep -qF -- "$C" "$ERR" 2>/dev/null; then
            HITS=$((HITS + 1))
        fi
    done

    # Did the product NAME a refusal? yes/no only — never the text, which
    # carries paths and would make the report non-portable.
    if /usr/bin/grep -qEi 'refus|too large|too many|exceed|symlink|escape|error|cannot|conflict|already exists' "$OUT" "$ERR" 2>/dev/null; then
        NAMED=yes
    else
        NAMED=no
    fi

    # Panics are never an acceptable response to hostile input.
    if /usr/bin/grep -qF 'panicked at' "$ERR" 2>/dev/null; then
        PANIC=yes
        MISMATCH=$((MISMATCH + 1))
        echo "MATRIX-CASE-FAIL: $ID PANICKED on hostile input" >&2
    else
        PANIC=no
    fi

    if [ "$HITS" -ne 0 ]; then
        MISMATCH=$((MISMATCH + 1))
        echo "MATRIX-CASE-FAIL: $ID leaked $HITS canary value(s) into its output" >&2
    fi
    if [ "$BAL" = "no" ]; then
        MISMATCH=$((MISMATCH + 1))
        echo "MATRIX-CASE-FAIL: $ID broke the conservation invariant ($ACC)" >&2
    fi

    LINE="CASE: id=$ID class=$KLASS expect=$EXPECT corpus_digest=$CDIGEST exit=$EXIT discovered=$D imported=$I quarantined=$Q excluded=$X balances=$BAL accounting=$PRESENT canary_hits=$HITS refusal_named=$NAMED panicked=$PANIC"
    if [ "$SCOPE" = "portable" ]; then
        printf '%s\n' "$LINE" >> "$PORTABLE"
    else
        printf '%s collapsed=%s\n' "$LINE" "$COLLAPSED" >> "$PLATFORM"
    fi
done

# --- 5. isolation: what did NOT change outside every target ----------------
"$BIN" backup digest --home "$SENTINEL" > "$WORK/sentinel-after.txt" 2>&1
SENTINEL_AFTER=$(/usr/bin/grep '^DIGEST: ' "$WORK/sentinel-after.txt" | /usr/bin/sed 's/^DIGEST: //')
if [ "$SENTINEL_BEFORE" = "$SENTINEL_AFTER" ]; then
    UNCHANGED=yes
else
    UNCHANGED=no
    MISMATCH=$((MISMATCH + 1))
    echo "MATRIX-FAIL: the sentinel tree OUTSIDE every target home changed:" >&2
    echo "  before=$SENTINEL_BEFORE after=$SENTINEL_AFTER" >&2
fi

# --- 6. emit ----------------------------------------------------------------
NPORT=$(/usr/bin/grep -c '^CASE: ' "$PORTABLE" 2>/dev/null || echo 0)
NPLAT=$(/usr/bin/grep -c '^CASE: ' "$PLATFORM" 2>/dev/null || echo 0)

{
    echo "MATRIX-VERSION: 1"
    echo "SPEC-SHA256: $SPEC_SHA"
    echo "DIGEST-ALGO: $DIGEST_ALGO"
    LC_ALL=C /usr/bin/sort "$PORTABLE"
    echo "SENTINEL-UNCHANGED: $UNCHANGED"
    echo "PORTABLE-CASES: $NPORT"
} > "$REPORT"

{
    echo "MATRIX-PLATFORM-VERSION: 1"
    echo "PLATFORM: $(uname -s)"
    echo "SPEC-SHA256: $SPEC_SHA"
    LC_ALL=C /usr/bin/sort "$PLATFORM"
    echo "PLATFORM-CASES: $NPLAT"
} > "$REPORT.platform"

echo "MATRIX: portable_cases=$NPORT platform_cases=$NPLAT sentinel_unchanged=$UNCHANGED failures=$MISMATCH"
[ "$NPORT" -ge 10 ] || die "only $NPORT portable cases ran — too few to be evidence"
[ "$MISMATCH" -eq 0 ] || exit 1
exit 0
