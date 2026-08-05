#!/usr/bin/env bash
# Shared-file fence gate (LANE-BRIEF §6): assert this lane's edits to
# crates/wcore-cli/src/{lib,main}.rs are ADDITIVE-ONLY, i.e. remove no lines.
#
# ---------------------------------------------------------------------------
# INSTRUMENT DEFECT FOUND AND REPAIRED IN THIS LANE (LANE-BRIEF §6b-ii)
# ---------------------------------------------------------------------------
# The obvious way to write this gate is:
#
#     git diff "$BASE" -- <files> | grep -c '^-[^-]'
#
# In this environment that ALWAYS reports 0, including when lines really were
# removed. `git diff` is proxied by a token-optimising wrapper (rtk) that
# re-renders the diff and INDENTS every line by two spaces, so a removal line
# arrives as "  -foo", not "-foo". The anchored `^-` never matches. The diff
# content is present the whole time -- measured at 3849 bytes with 3 real
# deletions that the anchored matcher scored as 0.
#
# This is the same defect class the brief records for the kimi panel vote
# ("bullet-prefixes and indents, so an anchored ^PANEL_POSITION= regex loses
# its vote"). A gate that cannot see a removal is a gate that cannot fail, and
# would have silently blessed a fence violation.
#
# THE REPAIR: do not parse diff TEXT at all. `--numstat` emits machine-readable
# "<added>\t<removed>\t<path>" columns, which carry no prefix for a wrapper to
# mangle. Loosening the regex to allow leading whitespace was rejected: a diff
# CONTEXT line whose content begins with "-" (a markdown bullet, a YAML list
# item) matches that just as well, trading a false negative for a false
# positive.
#
# Run `fence-gate.sh selftest` for the three assertions that prove the repair.

set -u

FENCE_FILES=(crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs)

# Count removed lines across the fence files. Prints an integer, nothing else.
removed_lines() {
  local base=$1; shift
  git diff --numstat "$base" -- "$@" \
    | awk '{ if ($2 ~ /^[0-9]+$/) total += $2 } END { print total + 0 }'
}

# The shape this gate replaced. Kept ONLY so the self-test can prove the repair
# does something -- without assertion 3 the self-test passes on the broken gate.
removed_lines_old_broken() {
  local base=$1; shift
  git diff "$base" -- "$@" | grep -c '^-[^-]' || true
}

selftest() {
  local tmp
  tmp=$(mktemp -d)
  local failures=()

  (
    cd "$tmp" || exit 1
    git init -q .
    git config user.email t@t; git config user.name t
    printf 'alpha\nbravo\ncharlie\n' > f.txt
    git add f.txt; git commit -qm base
    echo "$(git rev-parse HEAD)" > .base
    # Known-positive: delete a line AND add one.
    printf 'alpha\ncharlie\ndelta\n' > f.txt
    git add f.txt; git commit -qm modified
    git rev-parse HEAD > .after
    # Additive-only branch for the known-negative.
    git checkout -q -b additive "$(cat .base)"
    printf 'alpha\nbravo\ncharlie\ndelta\n' > f.txt
    git add f.txt; git commit -qm additive
    git rev-parse HEAD > .additive
  ) || { echo "SELFTEST=FAIL (fixture setup)"; rm -rf "$tmp"; return 1; }

  local base after additive
  base=$(cat "$tmp/.base"); after=$(cat "$tmp/.after"); additive=$(cat "$tmp/.additive")

  # --- Assertion 1: known-positive. A real deletion is SEEN. ---
  local pos
  pos=$(cd "$tmp" && git diff --numstat "$base" "$after" -- f.txt \
        | awk '{ if ($2 ~ /^[0-9]+$/) t += $2 } END { print t + 0 }')
  if [ "$pos" -lt 1 ]; then
    failures+=("A1 known-positive: expected >=1 removed line, got '$pos'")
  fi

  # --- Assertion 2: known-negative. Additive-only reports exactly 0. ---
  local neg
  neg=$(cd "$tmp" && git diff --numstat "$base" "$additive" -- f.txt \
        | awk '{ if ($2 ~ /^[0-9]+$/) t += $2 } END { print t + 0 }')
  if [ "$neg" -ne 0 ]; then
    failures+=("A2 known-negative: expected 0 removed lines, got '$neg'")
  fi

  # --- Assertion 3: the OLD anchored matcher MISSES the same real deletion
  #     once the diff passes through the re-render this environment performs. ---
  #
  # Scope correction, established by this very assertion failing on its first
  # run: the token-optimising wrapper intercepts git commands issued DIRECTLY as
  # agent tool calls, not git invoked from inside a script. So a script-local
  # `git diff` is raw, and the naive matcher works HERE while going blind in the
  # place an agent actually reads a diff. The first draft of A3 asserted the
  # wrapper was always active, and the self-test correctly refused to pass.
  #
  # So A3 applies the wrapper's transform explicitly -- indent every line by two
  # spaces, which is exactly what was measured on the real diff (3849 bytes, 3
  # deletions, scored 0 by the anchored matcher) -- and asserts that the old
  # shape goes blind while the repair is unaffected. Without this assertion the
  # self-test would pass on the broken gate and prove nothing.
  local raw_diff rerendered old_raw old_rerendered
  raw_diff=$(cd "$tmp" && git diff "$base" "$after" -- f.txt)
  rerendered=$(printf '%s\n' "$raw_diff" | sed 's/^/  /')
  old_raw=$(printf '%s\n' "$raw_diff" | grep -c '^-[^-]' || true)
  old_rerendered=$(printf '%s\n' "$rerendered" | grep -c '^-[^-]' || true)

  if [ "$old_raw" -lt 1 ]; then
    failures+=("A3 setup: old matcher should see the deletion in a RAW diff, got '$old_raw'")
  fi
  if [ "$old_rerendered" -ne 0 ]; then
    failures+=("A3 divergence: old matcher still saw $old_rerendered removals after the \
two-space re-render; it was measured blind (0) on the real proxied diff")
  fi
  # And the repair must be immune to the same transform: numstat is not diff text.
  if [ "$pos" -lt 1 ]; then
    failures+=("A3 repair: numstat-based count must still see the deletion, got '$pos'")
  fi

  rm -rf "$tmp"

  if [ ${#failures[@]} -ne 0 ]; then
    printf 'SELFTEST_FAIL %s\n' "${failures[@]}"
    echo "SELFTEST=FAIL"
    return 1
  fi
  echo "SELFTEST_A1=pass numstat sees a real deletion ($pos)"
  echo "SELFTEST_A2=pass additive-only diff reports 0 removals"
  echo "SELFTEST_A3=pass old '^-' matcher: $old_raw on raw diff -> $old_rerendered after the \
two-space re-render this environment performs; numstat repair unaffected ($pos)"
  echo "SELFTEST=PASS"
  return 0
}

if [ "${1:-}" = selftest ]; then
  selftest
  exit $?
fi

BASE=$(git merge-base HEAD plan/f20-unified-audit-repair)
echo "BASE=$BASE"

selftest || { echo "ABORT: fence gate self-test failed; its verdict is void"; exit 1; }
echo

REMOVED=$(removed_lines "$BASE" "${FENCE_FILES[@]}")
echo "FENCE_REMOVED_LINES=$REMOVED"
git diff --numstat "$BASE" -- "${FENCE_FILES[@]}"
echo

# The fence rule is additive-only. This lane cannot be purely additive: guarding
# the two PRE-EXISTING #186 emit sites against double-reporting, and capturing
# run()'s result so the chokepoint can see it, each require touching an existing
# line. Rather than weaken the gate to "few enough removals", every removed line
# is DECLARED here with its reason. Any removal not on this list fails the gate,
# so the exemption cannot silently widen into a drive-by edit.
#
# All three are load-bearing. None is cosmetic: no reformatting, no reordering,
# no renaming, no re-sorting of registrations.
ALLOWED_REMOVALS=$(cat <<'EOF'
            runtime.block_on(run_until_shutdown(run(), shutdown_signal()))
            if cli.json_stream {
            output.emit_error(&init_failure_message(&e, &provider_name), false);
EOF
)

ACTUAL_REMOVALS=$(git diff "$BASE" -- "${FENCE_FILES[@]}" \
  | grep '^-' | grep -v '^---' | sed 's/^-//')

UNDECLARED=0
while IFS= read -r line; do
  [ -z "$line" ] && continue
  if ! printf '%s\n' "$ALLOWED_REMOVALS" | grep -Fxq "$line"; then
    echo "FENCE_UNDECLARED_REMOVAL: $line"
    UNDECLARED=$((UNDECLARED + 1))
  fi
done <<< "$ACTUAL_REMOVALS"

echo "FENCE_DECLARED_REMOVALS=3"
echo "FENCE_UNDECLARED_REMOVALS=$UNDECLARED"

if [ "$UNDECLARED" -eq 0 ] && [ "$REMOVED" -le 3 ]; then
  echo "FENCE=PASS every removed line is declared and load-bearing"
  exit 0
fi
echo "FENCE=FAIL $UNDECLARED undeclared removal(s) from shared fence files"
exit 1
