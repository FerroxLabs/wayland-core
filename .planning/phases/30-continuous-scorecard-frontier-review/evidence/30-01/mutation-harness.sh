#!/bin/bash
# KNOWN-GOOD / KNOWN-BAD SEPARATION PROOF for resolve-evidence-ids.py.
#
# An instrument nobody has watched fail is the defect it hunts. This program has
# found seven instruments carrying the exact defect they hunt, so this resolver
# is not trusted until it has been shown to (a) accept the real ledger and
# (b) reject mutated copies of that SAME real ledger.
#
# Mutations are applied to a COPY. The real .planning/intel/COMPETITIVE-LEDGER.md
# is never written. Mutations are of the real artifact, not of a synthetic toy —
# the Phase 28 verifier standard.
#
# Usage: bash mutation-harness.sh <repo-root> <out-dir>
set -u

REPO=$1
OUT=$2
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

LEDGER="$REPO/.planning/intel/COMPETITIVE-LEDGER.md"
RESOLVER="$OUT/resolve-evidence-ids.py"
SCRATCH="$WORK/scratch"
mkdir -p "$SCRATCH"

# counts <ledger-file> -> "CONFIRMED PARTIAL UNRESOLVED"
counts() {
  python3 "$RESOLVER" "$REPO" "$SCRATCH" "$1" 2>/dev/null \
    | /usr/bin/awk -F'\t' '
        {c[$2]++}
        END {printf "%d %d %d", c["CONFIRMED"]+0, c["PARTIAL"]+0, c["UNRESOLVED"]+0}'
}

FAIL=0
report() { # name expect_change baseline mutated
  local name=$1 expect=$2 base=$3 mut=$4
  if [ "$expect" = "change" ]; then
    if [ "$base" = "$mut" ]; then
      echo "FAIL  $name : counts UNCHANGED ($mut) — the instrument did not notice the mutation"
      FAIL=1
    else
      echo "PASS  $name : $base -> $mut"
    fi
  else
    if [ "$base" = "$mut" ]; then
      echo "PASS  $name : $base -> $mut (control, correctly unchanged)"
    else
      echo "FAIL  $name : control moved $base -> $mut — the instrument is unstable"
      FAIL=1
    fi
  fi
}

echo "=== KNOWN-GOOD / KNOWN-BAD SEPARATION — resolve-evidence-ids.py"
echo "ledger : $LEDGER"
echo "date   : $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo

BASE=$(counts "$LEDGER")
echo "BASELINE (pristine real ledger)  CONFIRMED PARTIAL UNRESOLVED = $BASE"
echo

# --- M1: a real evidence ID's artifact path is repointed at a file that does
#         not exist. Must move a CONFIRMED to UNRESOLVED.
/usr/bin/sed 's#phases/27-multimodal-browser-generation-voice/27-PHASE-VERDICT.md#phases/27-multimodal-browser-generation-voice/27-PHASE-VERDICT-THAT-DOES-NOT-EXIST.md#' \
  "$LEDGER" > "$WORK/m1.md"
M1=$(counts "$WORK/m1.md")
report "M1 path repointed at a nonexistent file" change "$BASE" "$M1"

# --- M2: one byte of a real pinned commit SHA is flipped. Must be caught.
/usr/bin/sed 's#32e2f57d09fe4b287e513081862217dc9daa5901#32e2f57d09fe4b287e513081862217dc9daa5902#' \
  "$LEDGER" > "$WORK/m2.md"
M2=$(counts "$WORK/m2.md")
report "M2 one flipped byte in a pinned commit SHA" change "$BASE" "$M2"

# --- M3: a wholly fabricated evidence-ID row is appended to the index. This is
#         the over-claim shape: a row that reads exactly like the others.
cp "$LEDGER" "$WORK/m3.md"
printf '| `F31-FABRICATED@deadbeef` | `phases/31-nonexistent/31-01-SUMMARY.md` — a row that reads exactly like a real one |\n' \
  >> "$WORK/m3.md"
M3=$(counts "$WORK/m3.md")
report "M3 fabricated evidence-ID row appended" change "$BASE" "$M3"

# --- M4: a real artifact digest is altered. Must break DIGEST resolution.
/usr/bin/sed 's#5028fe28#5028fe29#g' "$LEDGER" > "$WORK/m4.md"
M4=$(counts "$WORK/m4.md")
report "M4 altered artifact sha256 digest" change "$BASE" "$M4"

# --- C1: CONTROL. Prose is reworded without touching any citation. A verifier
#         that rejects everything would move here too, and would be useless.
/usr/bin/sed 's#Records presence/absence of a counterpart, never a performance claim.#Records presence or absence of a counterpart; it never makes a performance claim.#' \
  "$LEDGER" > "$WORK/c1.md"
C1=$(counts "$WORK/c1.md")
report "C1 CONTROL prose reworded, no citation touched" nochange "$BASE" "$C1"

echo
echo "real ledger unmodified check: $(/usr/bin/git -C "$REPO" status --porcelain -- .planning/intel/COMPETITIVE-LEDGER.md | /usr/bin/wc -l | tr -d ' ') modified paths (expect 0)"
echo
if [ "$FAIL" -eq 0 ]; then
  echo "SEPARATION_RESULT=PASS  (4 mutations detected, 1 control held)"
  exit 0
else
  echo "SEPARATION_RESULT=FAIL"
  exit 1
fi
