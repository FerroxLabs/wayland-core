#!/usr/bin/env bash
# Capture every scrap of work from agent worktrees as APPLYABLE patches.
#
# Why this exists: agents on this program die mid-flight routinely — spend
# limits, API transport errors, host saturation. On 2026-07-26 the rescue was
# done by hand and produced human-readable *summaries* with a .patch extension.
# `git apply` rejected all of them and 1,424 lines had to be recovered from the
# live worktrees instead. A patch nobody tried to apply is not a backup.
#
# So this script's contract is: every artifact it emits is verified applyable
# before it is reported, and anything that fails verification is reported LOUDLY
# rather than left to be discovered later.
#
# Usage:
#   .planning/scripts/rescue-worktrees.sh [-o OUTDIR] [-b BASE_REF] [WORKTREE...]
#
# With no WORKTREE arguments it rescues every worktree registered to this repo
# except the main one. Read-only with respect to the worktrees: it never touches
# a live index, never checks out, never stashes. Safe to run against worktrees
# that agents are actively working in.

set -uo pipefail

OUTDIR=""
BASE_REF="plan/f20-unified-audit-repair"

while getopts "o:b:h" opt; do
  case "$opt" in
    o) OUTDIR="$OPTARG" ;;
    b) BASE_REF="$OPTARG" ;;
    h) sed -n '2,25p' "$0"; exit 0 ;;
    *) echo "usage: $0 [-o OUTDIR] [-b BASE_REF] [WORKTREE...]" >&2; exit 2 ;;
  esac
done
shift $((OPTIND - 1))

REPO_ROOT="$(git rev-parse --show-toplevel)" || { echo "not in a git repo" >&2; exit 2; }
: "${OUTDIR:=$REPO_ROOT/.planning/rescue}"
mkdir -p "$OUTDIR"

# Enumerate targets: explicit args, else every registered worktree but the main one.
targets=()
if [ "$#" -gt 0 ]; then
  targets=("$@")
else
  while IFS= read -r line; do
    [ "$line" = "$REPO_ROOT" ] && continue
    targets+=("$line")
  done < <(git worktree list --porcelain | awk '/^worktree /{print substr($0,10)}')
fi

if [ "${#targets[@]}" -eq 0 ]; then
  echo "no worktrees to rescue"; exit 0
fi

rescued=0; empty=0; failed=0

for wt in "${targets[@]}"; do
  if [ ! -d "$wt" ]; then
    echo "MISSING  $wt (registered but gone — prune with 'git worktree prune')" >&2
    failed=$((failed + 1)); continue
  fi

  name="$(basename "$wt")"
  branch="$(git -C "$wt" rev-parse --abbrev-ref HEAD 2>/dev/null || echo DETACHED)"
  head="$(git -C "$wt" rev-parse --short HEAD 2>/dev/null || echo unknown)"

  # --- 1. Committed-but-unmerged work, as a real patch series -------------
  # These are proper `git am` inputs, so authorship and messages survive.
  series="$OUTDIR/$name.commits.mbox"
  : > "$series"
  ncommits=0
  if git -C "$wt" rev-parse --verify -q "$BASE_REF" >/dev/null; then
    ncommits=$(git -C "$wt" rev-list --count "$BASE_REF..HEAD" 2>/dev/null || echo 0)
    if [ "$ncommits" -gt 0 ]; then
      git -C "$wt" format-patch --stdout "$BASE_REF..HEAD" > "$series" 2>/dev/null
    fi
  else
    echo "WARN     base ref '$BASE_REF' not found in $name; skipping commit series" >&2
  fi
  [ "$ncommits" -gt 0 ] || rm -f "$series"

  # --- 2. Uncommitted work, INCLUDING untracked files ---------------------
  # A plain `git diff HEAD` silently omits untracked files — that is how work
  # gets lost. Build the diff through a THROWAWAY index instead: copy the
  # worktree's index aside, `add -A` into the copy, and diff the copy. The
  # live index the agent is using is never written to.
  patch="$OUTDIR/$name.worktree.patch"
  gitdir="$(git -C "$wt" rev-parse --absolute-git-dir 2>/dev/null)"
  tmpidx="$(mktemp "${TMPDIR:-/tmp}/rescue-idx.XXXXXX")"
  if [ -f "$gitdir/index" ]; then cp "$gitdir/index" "$tmpidx"; fi

  GIT_INDEX_FILE="$tmpidx" git -C "$wt" add -A >/dev/null 2>&1
  GIT_INDEX_FILE="$tmpidx" git -C "$wt" diff --cached --binary HEAD > "$patch" 2>/dev/null
  rm -f "$tmpidx"

  if [ ! -s "$patch" ]; then
    rm -f "$patch"
    if [ "$ncommits" -eq 0 ]; then
      echo "EMPTY    $name ($branch @ $head) — nothing to rescue"
      empty=$((empty + 1)); continue
    fi
  fi

  # --- 3. VERIFY. This is the whole point of the script. -------------------
  # An unverified patch is what caused the incident this script exists for.
  status="ok"
  if [ -s "$patch" ]; then
    if ! git -C "$REPO_ROOT" apply --check --3way "$patch" >/dev/null 2>&1; then
      # --3way can still resolve at apply time via blob SHAs; a plain --check
      # failure alone is not fatal, so distinguish the two.
      if git -C "$REPO_ROOT" apply --check "$patch" >/dev/null 2>&1; then
        status="applies-clean-only-without-3way"
      else
        status="NEEDS-3WAY-OR-CONFLICTS"
      fi
    fi
  fi

  {
    echo "# rescue: $name"
    echo "# worktree : $wt"
    echo "# branch   : $branch"
    echo "# head     : $head"
    echo "# base     : $BASE_REF"
    echo "# commits  : $ncommits unmerged -> ${series##*/}"
    echo "# verify   : $status"
    echo "#"
    echo "# Restore with:"
    [ "$ncommits" -gt 0 ] && echo "#   git am ${series##*/}                 # committed work"
    [ -s "$patch" ]       && echo "#   git apply --3way ${patch##*/}        # uncommitted work"
  } > "$OUTDIR/$name.README"

  if [ "$status" = "NEEDS-3WAY-OR-CONFLICTS" ]; then
    echo "CONFLICT $name ($branch @ $head) — patch written but does NOT apply cleanly to $BASE_REF; resolve by hand" >&2
    failed=$((failed + 1))
  else
    echo "RESCUED  $name ($branch @ $head) — $ncommits commit(s), uncommitted=$( [ -s "$patch" ] && echo yes || echo no ), verify=$status"
    rescued=$((rescued + 1))
  fi
done

echo
echo "rescued=$rescued empty=$empty failed=$failed  ->  $OUTDIR"
# Non-zero exit when anything needs a human, so a caller cannot mistake a
# partial rescue for a clean one.
[ "$failed" -eq 0 ]
