#!/usr/bin/env bash
# Phase 27 Criterion 1 — the "ONE intake path" gate.
#
# The criterion's unmet clause is the word *one*. This gate asserts that no
# media surface opens a caller-named file except through the shared chokepoint.
#
# WHY THIS GATE CAN FAIL, and how that was proved rather than asserted:
#   - it is run at the BASE commit as well as at HEAD. At base it must FAIL
#     (several surfaces open their own files); at HEAD it must PASS. A gate
#     that was already green at base proves nothing (LANE-BRIEF §3.2).
#   - every count comes from `/usr/bin/grep`, never the rtk-proxied one, and
#     each search is paired with a KNOWN-POSITIVE in the same invocation so a
#     zero can never come from a dead instrument (LANE-BRIEF §3b-i).
#
#   f27-c1-one-path-gate.sh <repo-root>
set -u
REPO="${1:?repo root}"
cd "$REPO" || exit 2
G=/usr/bin/grep
FAIL=0

CHOKEPOINT=crates/wcore-tools/src/media_intake.rs

# The media surfaces in scope. `video_analyze` is deliberately absent: it never
# ingests the caller's bytes, it hands the path to ffmpeg as a subprocess
# argument (see 27-C1-NOTES.md M6).
SURFACES="
crates/wcore-tools/src/vision_tools.rs
crates/wcore-tools/src/transcription_tools.rs
crates/wcore-tools/src/pdf_tool.rs
crates/wcore-tools/src/doc_tool.rs
crates/wcore-cli/src/attachments.rs
crates/wcore-agent/src/channel_media.rs
"

echo "### f27-c1 one-path gate — repo: $REPO"
echo "### HEAD: $(/usr/bin/git rev-parse --short HEAD 2>/dev/null)"
echo

# ── 0. instrument liveness: a KNOWN-POSITIVE in the same tool invocation ────
POS=$($G -c "fn " "$CHOKEPOINT" 2>/dev/null || echo 0)
echo "instrument check: 'fn ' in $CHOKEPOINT -> $POS (must be > 0)"
if [ "$POS" -eq 0 ]; then
  echo "FATAL: the grep instrument returned zero on a term that is certainly present."
  echo "Every absence below would be free. Refusing to report."
  exit 2
fi
echo

# ── 1. no surface may open a caller-named media file itself ────────────────
# Production code only: the `#[cfg(test)]` block of each file is excluded, since
# a test fixture legitimately creates and opens its own files.
echo "--- CHECK 1: no media surface opens a file outside the chokepoint"
for f in $SURFACES; do
  [ -f "$f" ] || { echo "  SKIP (absent at this commit): $f"; continue; }
  prod=$(awk '/^#\[cfg\(test\)\]/{exit} {print}' "$f")
  hits=$(printf '%s\n' "$prod" | $G -nE 'File::open|OpenOptions::new|fs::read\(|fs::read_to_string\(|std::fs::metadata\(|libc::openat' || true)
  if [ -n "$hits" ]; then
    echo "  FAIL $f"
    printf '%s\n' "$hits" | sed 's/^/        /'
    FAIL=1
  else
    echo "  ok   $f"
  fi
done
echo

# ── 2. every surface must actually REACH the chokepoint ────────────────────
# Guards the inverse failure: a surface that opens nothing because it ingests
# nothing would silently pass check 1.
echo "--- CHECK 2: every media surface reaches the chokepoint"
for f in $SURFACES; do
  [ -f "$f" ] || continue
  if $G -q 'media_intake' "$f"; then
    echo "  ok   $f"
  else
    echo "  FAIL $f does not reference media_intake"
    FAIL=1
  fi
done
echo

# ── 3. the open primitive must stay private to the chokepoint ──────────────
echo "--- CHECK 3: the open primitive is private (compiler-enforced, not convention)"
if $G -q '^fn open_once' "$CHOKEPOINT"; then
  echo "  ok   open_once is private in $CHOKEPOINT"
elif $G -q '^pub fn open_once' "$CHOKEPOINT"; then
  echo "  FAIL open_once is pub — any module could bypass the sequence"
  FAIL=1
else
  echo "  FAIL open_once not found in $CHOKEPOINT"
  FAIL=1
fi
echo

# ── 4. one magic-byte table ────────────────────────────────────────────────
echo "--- CHECK 4: exactly one magic-byte table"
TBL=$($G -rlE '\\x89PNG' --include="*.rs" crates/wcore-tools/src crates/wcore-agent/src crates/wcore-cli/src 2>/dev/null \
      | xargs -r $G -lE 'fn (detect|from)_' 2>/dev/null | sort -u || true)
prod_tbl=""
for f in $TBL; do
  # a file whose PNG signature appears only inside its test module is a fixture,
  # not a second table
  if awk '/^#\[cfg\(test\)\]/{exit} {print}' "$f" | $G -q '\\x89PNG'; then
    prod_tbl="$prod_tbl $f"
  fi
done
prod_tbl=$(echo $prod_tbl)
n=$(echo "$prod_tbl" | wc -w)
echo "  production files carrying a PNG signature: $n -> $prod_tbl"
if [ "$n" -eq 1 ] && [ "$prod_tbl" = "$CHOKEPOINT" ]; then
  echo "  ok   the only table is the chokepoint's"
else
  echo "  FAIL more than one magic-byte table, or it is not the chokepoint's"
  FAIL=1
fi
echo

if [ "$FAIL" -eq 0 ]; then
  echo "GATE: PASS"
else
  echo "GATE: FAIL"
fi
exit "$FAIL"
