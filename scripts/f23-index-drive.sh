#!/usr/bin/env bash
# F23-06 live driver — exercise the persistent repository index against THIS
# REAL WORKSPACE through the SHIPPED wayland-core binary, and measure every
# gate the plan demands.
#
# Contract (shared with the rest of the f23 driver family):
#   --binary <path>   the wayland-core binary to drive
#   --sha <commit>    the commit under test; the binary's own --build-info
#                     source SHA must equal it, so a stale binary REDDENS
#                     instead of silently proving old code
#   --nonce <hex>     caller-generated at run time, echoed in the terminal PASS
#                     marker, so a stale log cannot satisfy the caller's check
#   --repo <path>     optional; the workspace to index (default: the repo this
#                     script lives in)
#
# Emits exactly one terminal marker:
#   F23_03_DRIVE=PASS platform=<linux|macos> nonce=<the given nonce>
# and ONLY after every measurement and every check passed. Any failure exits
# non-zero and emits no PASS marker. A MISSING MEASUREMENT IS A FAILURE, never
# a skip.
#
# ── Gate discipline (this file is a gate, so it obeys the same rules) ────────
#
# 1. No check here is a pipeline into a filter. A pipeline's exit status is the
#    LAST command's, so `cmd | grep -v noise` reports grep's status: any
#    surviving line greens it even when `cmd` failed, and grep's exit 1 on
#    empty output reddens it on silent success. Every command's status is
#    captured on the line AFTER it runs and asserted on directly.
# 2. `set -e` is deliberately NOT used. This driver must run every check and
#    report the total, not die on the first one; failures are counted.
# 3. Every measurement is emitted as a machine-readable
#    `F23_03_MEASURE=<name> platform=<p> sample=<n> value=<v>` line, so the
#    caller asserts on the DRIVE LOG rather than on any prose written later.

set -uo pipefail

BINARY=""
SHA=""
NONCE=""
REPO=""

while [ $# -gt 0 ]; do
  case "$1" in
    --binary) BINARY="${2:-}"; shift 2 ;;
    --sha)    SHA="${2:-}";    shift 2 ;;
    --nonce)  NONCE="${2:-}";  shift 2 ;;
    --repo)   REPO="${2:-}";   shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done

[ -n "$BINARY" ] || { echo "FATAL: --binary is required" >&2; exit 64; }
[ -n "$SHA" ]    || { echo "FATAL: --sha is required" >&2; exit 64; }
[ -n "$NONCE" ]  || { echo "FATAL: --nonce is required" >&2; exit 64; }
[ -x "$BINARY" ] || { echo "FATAL: $BINARY is not an executable file" >&2; exit 65; }

BINARY=$(cd "$(dirname "$BINARY")" && pwd)/$(basename "$BINARY")

if [ -z "$REPO" ]; then
  REPO=$(cd "$(dirname "$0")/.." && pwd)
fi
[ -d "$REPO" ] || { echo "FATAL: --repo $REPO is not a directory" >&2; exit 64; }

case "$(uname -s)" in
  Linux)  PLATFORM=linux ;;
  Darwin) PLATFORM=macos ;;
  *) echo "FATAL: unsupported platform $(uname -s)" >&2; exit 66 ;;
esac

# ── Provenance: refuse to measure anything with a binary built from other code ──
# A measurement taken against an unidentifiable binary is not a measurement.
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
echo "F23_03_PROVENANCE=ok platform=$PLATFORM sha=$SHA"

RUN_DIR=$(mktemp -d)
STORES="$RUN_DIR/stores"
TRANSCRIPTS="$RUN_DIR/transcripts"
SCRATCH="$RUN_DIR/scratch"
mkdir -p "$STORES" "$TRANSCRIPTS" "$SCRATCH"

cleanup() { rm -rf "$RUN_DIR"; }
trap cleanup EXIT

FAILURES=0
fail() { echo "  FAIL: $*" >&2; FAILURES=$((FAILURES + 1)); }

# Run `wayland-core index …` and capture stdout+stderr into a transcript.
# Sets IDX_OUT and IDX_RC. Never pipes; the status is read on the next line.
idx() {
  local label="$1"; shift
  local t="$TRANSCRIPTS/$label.txt"
  echo "# invocation: $BINARY index $*" > "$t"
  IDX_OUT=$("$BINARY" index "$@" 2>>"$t")
  IDX_RC=$?
  printf '%s\n' "$IDX_OUT" >> "$t"
  echo "# exit: $IDX_RC" >> "$t"
}

# Extract one `key=value` field from a `F23_INDEX=<kind> …` line.
#
# Splits the line into tokens rather than using one anchored regex. The first
# cut used `s/^F23_INDEX=$1 .*[[:space:]]$2=…/`, which silently returned EMPTY
# for the FIRST field on a line — `agrees=` sits immediately after the literal
# space already consumed, so the `.*[[:space:]]` demanded a second one that
# was not there. It reported `agrees=` blank on a real, correct verify.
field() {
  printf '%s\n' "$IDX_OUT" \
    | sed -n "s/^F23_INDEX=$1 //p" \
    | head -1 \
    | tr ' ' '\n' \
    | sed -n "s/^$2=//p" \
    | head -1
}

measure() {
  echo "F23_03_MEASURE=$1 platform=$PLATFORM sample=$2 value=$3"
}

# Milliseconds since the epoch, portable across GNU and BSD date.
now_ms() {
  if command -v python3 >/dev/null 2>&1; then
    python3 -c 'import time;print(int(time.time()*1000))'
  else
    echo $(( $(date +%s) * 1000 ))
  fi
}

echo "F23_03_CORPUS=repo path=$REPO"

# ─────────────────────────────────────────────────────────────────────────────
# 1. COLD BUILD and WARM START, three samples each.
#
# Three samples because a single one is not a measurement: the Linux host is
# 96 cores and shared between lanes, and the Windows box is shared outright.
# ALL samples are recorded, not just the best.
# ─────────────────────────────────────────────────────────────────────────────
COLD_STORE=""
for SAMPLE in 1 2 3; do
  COLD_STORE="$STORES/cold-$SAMPLE.db"
  rm -f "$COLD_STORE" "$COLD_STORE-wal" "$COLD_STORE-shm"

  T0=$(now_ms)
  idx "cold-build-$SAMPLE" --root "$REPO" --store "$COLD_STORE" build
  T1=$(now_ms)
  if [ "$IDX_RC" -ne 0 ]; then
    fail "cold build sample $SAMPLE exited $IDX_RC"
    continue
  fi
  measure cold-build "$SAMPLE" "$((T1 - T0))"

  RECORDS=$(field build records)
  SYMBOLS=$(field build symbols)
  STORE_BYTES=$(field build store_bytes)
  READ=$(field build read)
  if [ -z "$RECORDS" ] || [ "$RECORDS" -lt 100 ]; then
    fail "cold build sample $SAMPLE indexed $RECORDS records — this corpus is \
supposed to be a real repository, so a tiny count means the walk did not run"
  fi
  measure store-size "$SAMPLE" "$STORE_BYTES"
  echo "F23_03_CORPUS=indexed sample=$SAMPLE records=$RECORDS symbols=$SYMBOLS read=$READ"

  # WARM START: a second open of an UNCHANGED store. Measured as the wall time
  # of a refresh that must report zero reads — if it read anything, the number
  # would be a rebuild's, not a warm start's.
  T2=$(now_ms)
  idx "warm-start-$SAMPLE" --root "$REPO" --store "$COLD_STORE" build
  T3=$(now_ms)
  if [ "$IDX_RC" -ne 0 ]; then
    fail "warm start sample $SAMPLE exited $IDX_RC"
    continue
  fi
  measure warm-start "$SAMPLE" "$((T3 - T2))"

  WARM_READ=$(field build read)
  WARM_EXTRACT=$(field build extracted)
  if [ "$WARM_READ" != "0" ] || [ "$WARM_EXTRACT" != "0" ]; then
    fail "warm start sample $SAMPLE opened $WARM_READ files and extracted \
$WARM_EXTRACT — incrementality is a READ COUNT, and this one is not zero"
  fi
  echo "F23_03_WARM=sample=$SAMPLE read=$WARM_READ extracted=$WARM_EXTRACT"
done

# ─────────────────────────────────────────────────────────────────────────────
# 2. QUERY LATENCY over a fixed query set — median and 95th percentile.
# ─────────────────────────────────────────────────────────────────────────────
LATENCIES=""
QUERY_SET="IndexStore normalize_rel ScopeIdentity SymbolKind IndexOptions \
RepoMapError extract_rust semantic_status LlmProvider SessionManager \
ToolRegistry ProviderCompat MemoryAccessGate WorkflowRunner CheckpointStore \
BudgetAuthorityCoordinator SandboxBackend ExecutionGraph AgentSpawner RepoMap"
for Q in $QUERY_SET; do
  idx "search-$Q" --root "$REPO" --store "$COLD_STORE" search "$Q" --limit 10
  if [ "$IDX_RC" -ne 0 ]; then
    fail "search '$Q' exited $IDX_RC"
    continue
  fi
  US=$(field search elapsed_us)
  if [ -z "$US" ]; then
    fail "search '$Q' reported no elapsed_us — a missing measurement is a failure"
    continue
  fi
  LATENCIES="$LATENCIES $US"
done

# Percentiles computed over the collected samples. Nearest-rank, which needs
# no interpolation and is exact for a set this small.
read -r LAT_P50 LAT_P95 LAT_N <<EOF
$(printf '%s\n' $LATENCIES | sort -n | awk '
  { v[NR] = $1 }
  END {
    if (NR == 0) { print "0 0 0"; exit }
    p50 = int((NR * 50 + 99) / 100); if (p50 < 1) p50 = 1
    p95 = int((NR * 95 + 99) / 100); if (p95 < 1) p95 = 1
    print v[p50], v[p95], NR
  }')
EOF
if [ "$LAT_N" -lt 10 ]; then
  fail "only $LAT_N query-latency samples were collected; the fixed query set \
has 20 entries and a missing measurement is a failure, not a skip"
fi
measure latency-p50 1 "$LAT_P50"
measure latency-p95 1 "$LAT_P95"
echo "F23_03_LATENCY=samples n=$LAT_N unit=microseconds all=$LATENCIES"

# ─────────────────────────────────────────────────────────────────────────────
# 3. RETRIEVAL QUALITY over the fixed corpus, through the shipped binary.
#
# Ground truth lives entirely inside crates/wcore-repomap, so no other lane's
# churn can move it, but the corpus competing for each hit is the WHOLE
# workspace — which is what makes the number worth reporting.
# ─────────────────────────────────────────────────────────────────────────────
quality_case() {
  local q="$1"; shift
  local expected="$*"
  idx "quality-$(printf '%s' "$q" | tr -c 'A-Za-z0-9' '_')" \
      --root "$REPO" --store "$COLD_STORE" search "$q" --limit 10
  if [ "$IDX_RC" -ne 0 ]; then
    fail "quality query '$q' exited $IDX_RC"
    QC_P=0; QC_R=0
    return
  fi
  local hits
  hits=$(printf '%s\n' "$IDX_OUT" | sed -n 's/^F23_INDEX=hit .*[[:space:]]path=\([^[:space:]]*\).*/\1/p')
  local top
  top=$(printf '%s\n' "$hits" | head -1)

  QC_P=0
  for e in $expected; do
    [ "$top" = "$e" ] && QC_P=1
  done

  local found=0 total=0
  for e in $expected; do
    total=$((total + 1))
    printf '%s\n' "$hits" | grep -qxF "$e" && found=$((found + 1))
  done
  QC_R=$(awk -v f="$found" -v t="$total" 'BEGIN { printf "%.4f", (t ? f / t : 0) }')
  echo "F23_03_QUALITY_CASE=$(printf '%s' "$q" | tr ' ' '_') precision_at_1=$QC_P recall_at_10=$QC_R top=$top"
}

QSUM=0
RSUM=0
QN=0
run_quality() {
  quality_case "$@"
  QSUM=$(awk -v a="$QSUM" -v b="$QC_P" 'BEGIN { printf "%.4f", a + b }')
  RSUM=$(awk -v a="$RSUM" -v b="$QC_R" 'BEGIN { printf "%.4f", a + b }')
  QN=$((QN + 1))
}

run_quality "IndexStore"            crates/wcore-repomap/src/store.rs
run_quality "normalize_rel"         crates/wcore-repomap/src/scope.rs
run_quality "ScopeIdentity"         crates/wcore-repomap/src/scope.rs
run_quality "semantic_status"       crates/wcore-repomap/src/search.rs
run_quality "extract_rust"          crates/wcore-repomap/src/extractor/rust.rs
run_quality "extract_typescript"    crates/wcore-repomap/src/extractor/typescript.rs
run_quality "strip_comments_rust_style" crates/wcore-repomap/src/extractor/mod.rs
run_quality "SymbolKind"            crates/wcore-repomap/src/types.rs
run_quality "IndexOptions"          crates/wcore-repomap/src/types.rs
run_quality "RepoMapError"          crates/wcore-repomap/src/types.rs
run_quality "first_meaningful"      crates/wcore-repomap/src/lib.rs
run_quality "reciprocal rank fusion" crates/wcore-repomap/src/search.rs
run_quality "content hash invalidation" crates/wcore-repomap/src/store.rs
run_quality "worktree identity"     crates/wcore-repomap/src/scope.rs
run_quality "bm25 full text"        crates/wcore-repomap/src/search.rs
run_quality "walker gitignore hidden" crates/wcore-repomap/src/scope.rs crates/wcore-repomap/src/lib.rs

PRECISION=$(awk -v s="$QSUM" -v n="$QN" 'BEGIN { printf "%.4f", (n ? s / n : 0) }')
RECALL=$(awk -v s="$RSUM" -v n="$QN" 'BEGIN { printf "%.4f", (n ? s / n : 0) }')
measure precision 1 "$PRECISION"
measure recall 1 "$RECALL"
echo "F23_03_QUALITY=corpus queries=$QN precision_at_1=$PRECISION recall_at_10=$RECALL"

# ─────────────────────────────────────────────────────────────────────────────
# 4. FALLBACK and STALENESS, driven for real.
# ─────────────────────────────────────────────────────────────────────────────
idx "fallback" --root "$REPO" --store "$COLD_STORE" search '=> {' --limit 3
if [ "$IDX_RC" -eq 0 ] && [ "$(field search fallback)" = "true" ]; then
  echo "F23_03_FALLBACK_REPORTED=true"
else
  echo "F23_03_FALLBACK_REPORTED=false"
  fail "a punctuation-only literal was not reported as answered by the fallback"
fi

# ─────────────────────────────────────────────────────────────────────────────
# 5. INCREMENTAL MUTATIONS, in a SCRATCH CLONE — never in the measurement
#    checkout. Each mutation is real, and after each one the driver asserts
#    that the UNCHANGED files were not re-extracted.
# ─────────────────────────────────────────────────────────────────────────────
#    The scratch repository is materialised with `git archive HEAD | tar -x`
#    and then `git init`-ed, NOT with `git clone`. Measured reason: the
#    measurement checkout is a detached-HEAD worktree, and `git clone` of a
#    detached HEAD produces "remote HEAD refers to nonexistent ref" and an
#    EMPTY working tree — which would make every mutation below operate on
#    nothing and pass vacuously.
CLONE="$SCRATCH/clone"
mkdir -p "$CLONE"
{
  git -C "$REPO" archive HEAD > "$SCRATCH/tree.tar" && tar -xf "$SCRATCH/tree.tar" -C "$CLONE"
} > "$TRANSCRIPTS/clone.txt" 2>&1
rc=$?
if [ "$rc" -ne 0 ]; then
  echo "FATAL: could not materialise the scratch tree (exit $rc)" >&2
  cat "$TRANSCRIPTS/clone.txt" >&2
  exit 69
fi
rm -f "$SCRATCH/tree.tar"
SCRATCH_FILES=$(find "$CLONE" -type f | wc -l | tr -d ' ')
if [ "$SCRATCH_FILES" -lt 100 ]; then
  echo "FATAL: the scratch tree has only $SCRATCH_FILES files; every mutation \
below would pass vacuously against an empty tree" >&2
  exit 69
fi
echo "F23_03_SCRATCH=files=$SCRATCH_FILES"
{
  git -C "$CLONE" init -q -b main
  git -C "$CLONE" add .
  git -C "$CLONE" -c user.email=f23@example.invalid -c user.name=f23 \
      -c commit.gpgsign=false commit -qm "f23 scratch base"
} >> "$TRANSCRIPTS/clone.txt" 2>&1
rc=$?
if [ "$rc" -ne 0 ]; then
  echo "FATAL: could not initialise the scratch repository (exit $rc)" >&2
  cat "$TRANSCRIPTS/clone.txt" >&2
  exit 69
fi
MUT_STORE="$STORES/mutations.db"
idx "mutation-base" --root "$CLONE" --store "$MUT_STORE" build
if [ "$IDX_RC" -ne 0 ]; then
  echo "FATAL: the scratch clone could not be indexed (exit $IDX_RC)" >&2
  exit 70
fi
BASE_RECORDS=$(field build records)
echo "F23_03_MUTATION_BASE=records=$BASE_RECORDS"

# $1 name, $2 expected non-zero counter field, then the assertion is that
# `extracted` never exceeds the number of files this mutation actually
# touched — i.e. that unchanged files were not re-extracted.
mutation() {
  local name="$1"; local expect_field="$2"; local max_extract="$3"
  idx "mutation-$name" --root "$CLONE" --store "$MUT_STORE" build
  local status=PASS
  if [ "$IDX_RC" -ne 0 ]; then
    status=FAIL
    fail "mutation $name: index build exited $IDX_RC"
  fi
  local got extracted unchanged
  got=$(field build "$expect_field")
  extracted=$(field build extracted)
  unchanged=$(field build unchanged)
  if [ -z "$got" ] || [ "$got" -lt 1 ]; then
    status=FAIL
    fail "mutation $name: expected $expect_field >= 1, got '${got:-<absent>}'"
  fi
  if [ -z "$extracted" ] || [ "$extracted" -gt "$max_extract" ]; then
    status=FAIL
    fail "mutation $name: re-extracted ${extracted:-?} files, at most \
$max_extract were touched — unchanged files were re-extracted"
  fi
  # `unchanged_reextracted` is `extracted` minus the files this mutation
  # touched, floored at zero: the number of files that were re-extracted
  # despite not having changed. It must be exactly 0.
  local surplus=$(( ${extracted:-0} > max_extract ? ${extracted:-0} - max_extract : 0 ))
  echo "F23_03_MUTATION=$name platform=$PLATFORM status=$status unchanged_reextracted=$surplus \
$expect_field=$got extracted=$extracted unchanged=$unchanged"
}

# add
printf 'pub fn f23_drive_added_%s() {}\n' "$NONCE" > "$CLONE/f23_added.rs"
mutation add added 1

# edit
printf 'pub fn f23_drive_added_%s() { let _ = 1; }\n' "$NONCE" > "$CLONE/f23_added.rs"
mutation edit changed 1

# delete
rm -f "$CLONE/f23_added.rs"
mutation delete deleted 0

# rename (content unchanged — must re-extract NOTHING)
printf 'pub fn f23_drive_renamed_%s() {}\n' "$NONCE" > "$CLONE/f23_rename_src.rs"
idx "mutation-rename-seed" --root "$CLONE" --store "$MUT_STORE" build
mv "$CLONE/f23_rename_src.rs" "$CLONE/f23_rename_dst.rs"
mutation rename renamed 0
rm -f "$CLONE/f23_rename_dst.rs"
idx "mutation-rename-cleanup" --root "$CLONE" --store "$MUT_STORE" build

# branch switch — a real second branch differing in exactly one file
git -C "$CLONE" -c user.email=f23@example.invalid -c user.name=f23 \
    -c commit.gpgsign=false checkout -q -b "f23-drive-$NONCE" \
    > "$TRANSCRIPTS/branch.txt" 2>&1
rc=$?
if [ "$rc" -ne 0 ]; then
  echo "F23_03_MUTATION=branch-switch platform=$PLATFORM status=FAIL unchanged_reextracted=0 note=checkout-failed"
  fail "could not create the scratch branch (git checkout exited $rc)"
else
  printf 'pub fn f23_branch_only_%s() {}\n' "$NONCE" > "$CLONE/f23_branch.rs"
  git -C "$CLONE" add f23_branch.rs >> "$TRANSCRIPTS/branch.txt" 2>&1
  git -C "$CLONE" -c user.email=f23@example.invalid -c user.name=f23 \
      -c commit.gpgsign=false commit -qm "f23 drive branch" \
      >> "$TRANSCRIPTS/branch.txt" 2>&1
  mutation branch-switch added 1
  # And the scope identity must have MOVED, not merely the file set.
  idx "branch-status" --root "$CLONE" --store "$MUT_STORE" status
  echo "F23_03_SCOPE_AFTER_SWITCH=$(printf '%s\n' "$IDX_OUT" | sed -n 's/^F23_INDEX=scope recorded=\(.*\)/\1/p' | head -1)"
fi

# ─────────────────────────────────────────────────────────────────────────────
# 6. STALENESS — edit an indexed file and confirm the hit says so.
# ─────────────────────────────────────────────────────────────────────────────
STALE_FILE="$CLONE/f23_stale.rs"
printf 'pub fn f23_stale_marker_%s() {}\n' "$NONCE" > "$STALE_FILE"
idx "stale-build" --root "$CLONE" --store "$MUT_STORE" build
idx "stale-before" --root "$CLONE" --store "$MUT_STORE" search "f23_stale_marker_$NONCE" --limit 3
BEFORE_STALE=$(printf '%s\n' "$IDX_OUT" | sed -n 's/^F23_INDEX=hit .*[[:space:]]content_stale=\([^[:space:]]*\).*/\1/p' | head -1)
printf 'pub fn f23_stale_marker_%s() { /* edited after indexing */ }\n' "$NONCE" > "$STALE_FILE"
idx "stale-after" --root "$CLONE" --store "$MUT_STORE" search "f23_stale_marker_$NONCE" --limit 3
AFTER_STALE=$(printf '%s\n' "$IDX_OUT" | sed -n 's/^F23_INDEX=hit .*[[:space:]]content_stale=\([^[:space:]]*\).*/\1/p' | head -1)
if [ "$BEFORE_STALE" = "false" ] && [ "$AFTER_STALE" = "true" ]; then
  echo "F23_03_STALENESS_REPORTED=true"
else
  echo "F23_03_STALENESS_REPORTED=false"
  fail "staleness: before='$BEFORE_STALE' after='$AFTER_STALE' — the \
before-assert is the load-bearing half; a hit that was ALWAYS stale proves nothing"
fi

# `verify` must agree with what the search just reported.
idx "verify-drifted" --root "$CLONE" --store "$MUT_STORE" verify
VERIFY_RC=$IDX_RC
echo "F23_03_VERIFY=agrees=$(field verify agrees) exit=$VERIFY_RC"
if [ "$VERIFY_RC" -eq 0 ]; then
  fail "verify reported agreement over a tree with a file edited after indexing"
fi

# ─────────────────────────────────────────────────────────────────────────────
# 7. SECRET ISOLATION — a run-time nonce planted in a gitignored file must be
#    absent from the STORE'S OWN BYTES, not merely from query results.
#
#    The CONTROL marker is planted in an INDEXED file and must be PRESENT. If
#    it were absent, the store would hold no content at all and the isolation
#    assertion below would be vacuously true — which is exactly the shape of a
#    gate that cannot go red.
# ─────────────────────────────────────────────────────────────────────────────
SECRET="f23secret${NONCE}zz"
CONTROL="f23control${NONCE}yy"
mkdir -p "$CLONE/f23-ignored"
printf 'const TOKEN: &str = "%s";\n' "$SECRET" > "$CLONE/f23-ignored/creds.rs"
printf 'f23-ignored/\n' >> "$CLONE/.gitignore"
printf 'pub const CONTROL: &str = "%s";\n' "$CONTROL" > "$CLONE/f23_control.rs"

ISO_STORE="$STORES/isolation.db"
rm -f "$ISO_STORE" "$ISO_STORE-wal" "$ISO_STORE-shm"
idx "isolation-build" --root "$CLONE" --store "$ISO_STORE" build
if [ "$IDX_RC" -ne 0 ]; then
  fail "isolation build exited $IDX_RC"
fi

CONTROL_HITS=0
SECRET_HITS=0
for f in "$ISO_STORE" "$ISO_STORE-wal" "$ISO_STORE-shm"; do
  [ -f "$f" ] || continue
  n=$(LC_ALL=C grep -c -a -F -- "$CONTROL" "$f" 2>/dev/null); n=${n:-0}
  CONTROL_HITS=$((CONTROL_HITS + n))
  m=$(LC_ALL=C grep -c -a -F -- "$SECRET" "$f" 2>/dev/null); m=${m:-0}
  SECRET_HITS=$((SECRET_HITS + m))
done
echo "F23_03_STORE_CONTROL_OCCURRENCES=$CONTROL_HITS"
echo "F23_03_STORE_NONCE_OCCURRENCES=$SECRET_HITS"
if [ "$CONTROL_HITS" -lt 1 ]; then
  fail "the CONTROL marker planted in an INDEXED file is absent from the \
store's bytes — the store holds no content, so the isolation assertion is vacuous"
fi
if [ "$SECRET_HITS" -ne 0 ]; then
  fail "CRITICAL: a run-time nonce planted in a gitignored file was found \
$SECRET_HITS time(s) in the store's own bytes — the excluded file was READ"
fi

# ─────────────────────────────────────────────────────────────────────────────
if [ "$FAILURES" -ne 0 ]; then
  echo "F23_03_DRIVE=FAIL platform=$PLATFORM nonce=$NONCE failures=$FAILURES" >&2
  exit 1
fi
echo "F23_03_DRIVE=PASS platform=$PLATFORM nonce=$NONCE"
exit 0
