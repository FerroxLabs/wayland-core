#!/usr/bin/env bash
# F26 SC2 — the quarantine known-negative, as a re-runnable instrument.
#
# THE QUESTION THIS ANSWERS
#
# "Executable content is quarantined" is an ABSENCE claim: the payload did not
# run, the payload is not in the live skills root. This programme has measured
# repeatedly that an absence assertion is the single easiest thing to pass
# without doing any work — a dead matcher, a wrong path, an empty home and a
# suite that ran zero tests all produce the comforting zero. Phase 26 already
# shipped one such shape (the SC1 redaction claim, self-passing until a
# planted-positive control was added).
#
# So the containment suite is only worth what it costs to make it FAIL. This
# script rips quarantine out of the product, rebuilds, and requires the named
# tests to go red. If they stay green, the suite proves nothing and this script
# exits non-zero saying so.
#
# WHY THIS SCRIPT CARRIES CONTROLS OF ITS OWN
#
# A mutation harness has the same failure modes as the thing it audits:
#
#   * a `sed` that matched nothing leaves the product intact, the suite passes,
#     and a careless reading calls that "tests are robust". So the mutation is
#     VERIFIED APPLIED (M1) before anything is built.
#   * a mutation that does not compile produces a red run for the wrong reason —
#     `cargo test` exits non-zero on a build failure exactly as it does on an
#     assertion failure. So the mutated build must COMPILE (M2), and the red is
#     read from the `test result:` line rather than from the exit status.
#   * a tree left dirty by an earlier abort makes every later result garbage. So
#     the restore is verified byte-identical (M4) and the suite is re-run to
#     GREEN afterwards (M5) — which is what proves the red came from the
#     mutation and not from the tree.
#
# USAGE:  scripts/f26-quarantine-known-negative.sh [repo-root]
set -uo pipefail

ROOT="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
CARGO="${CARGO:-/root/.cargo/bin/cargo}"
command -v "$CARGO" >/dev/null 2>&1 || CARGO="$(command -v cargo)"
TARGET="$ROOT/crates/wcore-cli/src/migrate/quarantine.rs"
BACKUP="$(mktemp)"
WORK="$(mktemp -d)"
rc_overall=0

# Tests that MUST go red when containment is removed. These are the security
# assertions, not the classifier unit tests: t5 asks the real agent-facing
# enumeration what it would load, t19 drives the real binary and looks for the
# payload's side effect on disk.
REQUIRED_RED=(
  t5_quarantined_content_is_absent_from_what_the_agent_would_load
  t19_live_negative_leg_quarantined_payload_does_not_execute
)

cleanup() {
  if [ -s "$BACKUP" ]; then cp "$BACKUP" "$TARGET"; fi
  rm -f "$BACKUP"
  rm -rf "$WORK"
}
trap cleanup EXIT

say() { printf '%s\n' "$*"; }
fail() { say "FAIL: $*"; rc_overall=1; }

say "=== F26 SC2 quarantine known-negative ==="
say "repo:  $ROOT"
say "cargo: $CARGO"
[ -f "$TARGET" ] || { say "FATAL: $TARGET not found"; exit 2; }
cp "$TARGET" "$BACKUP"

# --- M0: the suite is GREEN before anything is touched ----------------------
# Without this, a red run later cannot be attributed to the mutation.
say
say "--- M0 baseline: the suite must be green BEFORE the mutation ---"
( cd "$ROOT" && "$CARGO" test -p wcore-cli --test migrate_quarantine ) \
  >"$WORK/base.log" 2>&1
base_line="$(grep -E '^test result:' "$WORK/base.log" | tail -1)"
say "M0 ${base_line:-<no test result line>}"
if [ -z "$base_line" ]; then
  fail "M0 produced no 'test result:' line — the suite did not run"
elif ! printf '%s' "$base_line" | grep -q '^test result: ok\.'; then
  fail "M0 baseline is not green; every later reading is meaningless"
fi
# Anti-vacuity: a suite that exits 0 having run ZERO tests prints
# "test result: ok. 0 passed". Read the executed count back explicitly.
base_passed="$(printf '%s' "$base_line" | sed -n 's/.*ok\. \([0-9]*\) passed.*/\1/p')"
say "M0 executed: ${base_passed:-0} passed"
if [ "${base_passed:-0}" -lt 20 ]; then
  fail "M0 ran ${base_passed:-0} tests — a suite that runs nothing exits 0"
fi

# --- M1: rip quarantine out, and PROVE the mutation applied -----------------
say
say "--- M1 mutation: classify_skill_body always returns Data ---"
python3 - "$TARGET" <<'PY'
import sys, re
p = sys.argv[1]
s = open(p, encoding='utf-8').read()
old = """    if contains_shell_commands(content, loaded_from) {
        Classification::Executable(ExecutableReason::SkillShellDirective)
    } else {
        Classification::Data
    }"""
new = """    // F26-KNOWN-NEGATIVE MUTATION — quarantine removed on purpose.
    let _ = (content, loaded_from);
    Classification::Data"""
if old not in s:
    sys.stderr.write("MUTATION-ANCHOR-MISSING\n")
    sys.exit(3)
open(p, 'w', encoding='utf-8').write(s.replace(old, new, 1))
PY
mut_rc=$?
if [ "$mut_rc" -ne 0 ]; then
  say "M1 APPLIED: no (anchor not found — the product moved under this script)"
  fail "M1 could not apply the mutation; a green run below would be meaningless"
  exit 1
fi
if cmp -s "$BACKUP" "$TARGET"; then
  say "M1 APPLIED: no (file byte-identical after the edit)"
  fail "M1 mutation was a no-op"
  exit 1
fi
say "M1 APPLIED: yes ($(diff <(cat "$BACKUP") "$TARGET" | grep -c '^[<>]') changed lines)"

# --- M2 + M3: the mutated build compiles, and the security tests go RED -----
say
say "--- M2/M3: build the mutant, then read the test result back ---"
( cd "$ROOT" && "$CARGO" test -p wcore-cli --test migrate_quarantine ) \
  >"$WORK/mut.log" 2>&1
if grep -qE '^error(\[E[0-9]+\])?:' "$WORK/mut.log"; then
  say "M2 COMPILED: no"
  fail "M2 the mutant did not compile — a red from a build failure is NOT evidence"
  sed -n '1,25p' "$WORK/mut.log"
else
  say "M2 COMPILED: yes"
fi
mut_line="$(grep -E '^test result:' "$WORK/mut.log" | tail -1)"
say "M3 ${mut_line:-<no test result line>}"
mut_failed="$(printf '%s' "$mut_line" | sed -n 's/.* \([0-9]*\) failed.*/\1/p')"
if [ -z "$mut_line" ]; then
  fail "M3 the mutant produced no test result line"
elif [ "${mut_failed:-0}" -lt 1 ]; then
  fail "M3 the suite is GREEN with quarantine removed — it proves nothing"
fi
say "M3 failed under mutation: ${mut_failed:-0}"
say "M3 the tests that went red:"
grep -E '^test .* FAILED$' "$WORK/mut.log" | sed 's/^/    /' || true
for t in "${REQUIRED_RED[@]}"; do
  if grep -qE "^test $t \.\.\. FAILED$" "$WORK/mut.log"; then
    say "M3 REQUIRED-RED $t: FAILED (as required)"
  else
    fail "M3 REQUIRED-RED $t stayed green with containment removed"
  fi
done

# --- M4 + M5: restore, prove the restore, and re-run to GREEN ---------------
say
say "--- M4/M5: restore and re-prove ---"
cp "$BACKUP" "$TARGET"
if cmp -s "$BACKUP" "$TARGET"; then
  say "M4 RESTORED: yes (byte-identical to the pre-mutation file)"
else
  fail "M4 the tree was NOT restored"
fi
( cd "$ROOT" && "$CARGO" test -p wcore-cli --test migrate_quarantine ) \
  >"$WORK/post.log" 2>&1
post_line="$(grep -E '^test result:' "$WORK/post.log" | tail -1)"
say "M5 ${post_line:-<no test result line>}"
if ! printf '%s' "$post_line" | grep -q '^test result: ok\.'; then
  fail "M5 the suite did not return to green — the red above is not attributable"
fi

say
if [ "$rc_overall" -eq 0 ]; then
  say "KNOWN-NEGATIVE: PASS — containment removed makes the security tests fail"
else
  say "KNOWN-NEGATIVE: FAIL"
fi
exit "$rc_overall"
