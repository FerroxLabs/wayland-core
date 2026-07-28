#!/usr/bin/env bash
# 23B-H1 re-verification — mutation-applied check, and its self-test.
#
# WHY THIS FILE EXISTS (brief §6b-ii: repair the instrument, do not merely note it).
#
# The first version of this lane's mutation gate read:
#
#     MUTATION_SITES=$(grep -c 'skip_serializing_if = "Option::is_none"' model.rs)
#
# and printed `MUTATION_SITES=23`. That reading is uninterpretable on its own.
# `model.rs` already contained 21 unrelated `Option::is_none` predicates at base,
# so 23 means "21 pre-existing + the 2 I flipped" — but 21 (sed silently matched
# nothing, e.g. a quoting error inside the heredoc) would have looked just as
# plausible to a reader, and the gate had no baseline to compare against. A
# mutation that never applied would have produced a green "reverted, still passes"
# result, which is precisely the self-passing class this program keeps measuring.
#
# THE REPAIR: count the *target* predicate (`is_absent_json_value`), which has a
# known exact population of 2, and require the before/after transition 2 -> 0.
# An unapplied mutation now reads 2 -> 2 and fails loudly.
#
# Usage:
#   f23-h1-mutation-check.sh state <file>     # prints FIX_PRESENT | FIX_ABSENT | FIX_PARTIAL
#   f23-h1-mutation-check.sh selftest         # three assertions, see below
set -uo pipefail

TARGET_PREDICATE='skip_serializing_if = "is_absent_json_value"'
EXPECTED_POPULATION=2

# Count occurrences of the target predicate. Uses grep -c on a fixed string;
# no pipeline, so no exit-status theft (brief §3.2).
count_target() {
  local file="$1"
  grep -cF "$TARGET_PREDICATE" "$file" 2>/dev/null || true
}

state() {
  local file="$1"
  if [ ! -f "$file" ]; then echo "FILE_MISSING"; return 2; fi
  local n
  n=$(count_target "$file")
  case "$n" in
    "$EXPECTED_POPULATION") echo "FIX_PRESENT" ;;
    0)                      echo "FIX_ABSENT"  ;;
    *)                      echo "FIX_PARTIAL n=$n" ;;
  esac
}

# The instrument this replaces: a single unanchored count of a predicate that is
# already abundant in the file, with no baseline. Reproduced verbatim so the
# self-test can demonstrate it does not discriminate.
old_broken_matcher_says_mutated() {
  local file="$1" n
  n=$(grep -cF 'skip_serializing_if = "Option::is_none"' "$file" 2>/dev/null || true)
  [ "$n" -gt 0 ] && echo "MUTATED" || echo "NOT_MUTATED"
}

selftest() {
  local fixed mutated rc=0
  fixed=$(mktemp) || return 9
  mutated=$(mktemp) || return 9
  # A minimal fixture with the real population: 2 target predicates plus the
  # same abundant Option::is_none noise the real file carries.
  {
    for i in 1 2 3; do echo '    #[serde(default, skip_serializing_if = "Option::is_none")]'; done
    echo '    #[serde(default, skip_serializing_if = "is_absent_json_value")]'
    echo '    effect_receipt: Option<serde_json::Value>,'
    echo '    #[serde(default, skip_serializing_if = "is_absent_json_value")]'
    echo '    effect_receipt: Option<serde_json::Value>,'
  } > "$fixed"
  sed 's/skip_serializing_if = "is_absent_json_value"/skip_serializing_if = "Option::is_none"/g' \
    "$fixed" > "$mutated"

  # (1) KNOWN-POSITIVE: the fixed file must read FIX_PRESENT.
  local a; a=$(state "$fixed")
  if [ "$a" = "FIX_PRESENT" ]; then echo "SELFTEST_1_KNOWN_POSITIVE=PASS"
  else echo "SELFTEST_1_KNOWN_POSITIVE=FAIL got=$a"; rc=1; fi

  # (2) KNOWN-NEGATIVE: the mutated file must read FIX_ABSENT, i.e. the
  #     instrument actually goes red when the fix is gone.
  local b; b=$(state "$mutated")
  if [ "$b" = "FIX_ABSENT" ]; then echo "SELFTEST_2_KNOWN_NEGATIVE=PASS"
  else echo "SELFTEST_2_KNOWN_NEGATIVE=FAIL got=$b"; rc=1; fi

  # (3) THE OLD MATCHER WOULD HAVE MISSED IT. Without this assertion the whole
  #     self-test passes on the broken instrument too, which is what makes it
  #     the only assertion that proves the repair does anything. The old matcher
  #     calls the UNMUTATED file "MUTATED" -- it cannot tell the two apart.
  local old_on_fixed old_on_mutated
  old_on_fixed=$(old_broken_matcher_says_mutated "$fixed")
  old_on_mutated=$(old_broken_matcher_says_mutated "$mutated")
  if [ "$old_on_fixed" = "$old_on_mutated" ]; then
    echo "SELFTEST_3_OLD_MATCHER_BLIND=PASS old_on_fixed=$old_on_fixed old_on_mutated=$old_on_mutated"
  else
    echo "SELFTEST_3_OLD_MATCHER_BLIND=FAIL old matcher discriminated; repair may be unnecessary"
    rc=1
  fi

  rm -f "$fixed" "$mutated"
  echo "SELFTEST_RC=${rc}"
  return "$rc"
}

case "${1:-}" in
  state)    state "${2:?usage: state <file>}" ;;
  selftest) selftest ;;
  *) echo "usage: $0 {state <file>|selftest}" >&2; exit 64 ;;
esac
