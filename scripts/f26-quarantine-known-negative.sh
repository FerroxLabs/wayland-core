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
TARGET2="$ROOT/crates/wcore-cli/src/migrate/content.rs"
BACKUP="$(mktemp)"
BACKUP2="$(mktemp)"
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

# Tests that MUST go red when the exec-bit guard is removed (mutation 2).
REQUIRED_RED2=(
  t25_an_imported_peer_script_arrives_without_its_execute_bit
)

cleanup() {
  if [ -s "$BACKUP" ]; then cp "$BACKUP" "$TARGET"; fi
  if [ -s "$BACKUP2" ]; then cp "$BACKUP2" "$TARGET2"; fi
  rm -f "$BACKUP" "$BACKUP2"
  rm -rf "$WORK"
}
trap cleanup EXIT

say() { printf '%s\n' "$*"; }
fail() { say "FAIL: $*"; rc_overall=1; }

# --- the compile detector, and why it is a function -------------------------
#
# The FIRST version of this check was `grep -qE '^error' <log>` and it was
# structurally incapable of ever passing. `cargo test` prints
#
#     error: test failed, to rerun pass `-p wcore-cli --test migrate_quarantine`
#
# whenever ANY test fails — which is the exact condition this script exists to
# produce. So the check reported "the mutant did not compile" on a mutant that
# had plainly compiled and run 33 tests, and it would have done so on every
# successful run forever.
#
# The repair reads a POSITIVE signal instead of scanning for a negative one:
# the test harness prints `running <N> tests` only after the binary was built
# and started. Compiler errors are still counted, but cargo's own post-run
# summary lines are excluded by name rather than by hope.
#
# `--self-test` exercises this on three synthetic logs; see `self_test()`.
compiled_ok() {
  local log="$1"
  local harness compile_errs
  harness="$(grep -cE '^running [0-9]+ tests' "$log")"
  compile_errs="$(grep -E '^error' "$log" \
    | grep -vE '^error: test failed' \
    | grep -vE "^error: process didn't exit successfully" \
    | grep -cE '.')"
  [ "$harness" -ge 1 ] && [ "$compile_errs" -eq 0 ]
}

# The pre-repair matcher, kept ONLY so the self-test can demonstrate that the
# repair changes an outcome. Never used for a real reading.
compiled_ok_old_broken() {
  ! grep -qE '^error' "$1"
}

self_test() {
  local d rc
  d="$(mktemp -d)"
  rc=0

  # A1 known-positive: a build that SUCCEEDED and then had failing tests. This
  # is the exact log shape the real run produces, and the shape the old matcher
  # got wrong.
  cat >"$d/pos.log" <<'LOG'
   Compiling wcore-cli v0.12.25
warning: unused import: `x`
    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.78s
     Running tests/migrate_quarantine.rs
running 33 tests
test t5_quarantined_content_is_absent_from_what_the_agent_would_load ... FAILED
test result: FAILED. 23 passed; 10 failed; 0 ignored; 0 measured; 0 filtered out
error: test failed, to rerun pass `-p wcore-cli --test migrate_quarantine`
LOG

  # A2 known-negative: a genuine compile failure. No harness line, a real
  # rustc diagnostic.
  cat >"$d/neg.log" <<'LOG'
   Compiling wcore-cli v0.12.25
error[E0308]: mismatched types
  --> crates/wcore-cli/src/migrate/quarantine.rs:171:9
error: could not compile `wcore-cli` (lib) due to 1 previous error
LOG

  if compiled_ok "$d/pos.log"; then
    echo "SELF-TEST A1 known-positive        : PASS (a compiled-then-failed log reads as compiled)"
  else
    echo "SELF-TEST A1 known-positive        : FAIL"; rc=1
  fi
  if compiled_ok "$d/neg.log"; then
    echo "SELF-TEST A2 known-negative        : FAIL"; rc=1
  else
    echo "SELF-TEST A2 known-negative        : PASS (a real rustc error reads as not compiled)"
  fi
  # A3 is the assertion that proves the repair DOES something. Without it, this
  # self-test would pass on the broken matcher too — which is the defect class
  # this programme keeps re-finding.
  if compiled_ok_old_broken "$d/pos.log"; then
    echo "SELF-TEST A3 old-matcher-misses-it : FAIL (the old matcher agreed, so the repair is inert)"; rc=1
  else
    echo "SELF-TEST A3 old-matcher-misses-it : PASS (the OLD matcher calls A1 'did not compile')"
  fi
  rm -rf "$d"
  [ "$rc" -eq 0 ] && echo "SELF-TEST: PASS" || echo "SELF-TEST: FAIL"
  return "$rc"
}

if [ "${1:-}" = "--self-test" ]; then
  trap - EXIT
  rm -f "$BACKUP" "$BACKUP2"; rm -rf "$WORK"
  self_test
  exit $?
fi

say "=== F26 SC2 quarantine known-negative ==="
say "repo:  $ROOT"
say "cargo: $CARGO"
[ -f "$TARGET" ]  || { say "FATAL: $TARGET not found"; exit 2; }
[ -f "$TARGET2" ] || { say "FATAL: $TARGET2 not found"; exit 2; }
cp "$TARGET" "$BACKUP"
cp "$TARGET2" "$BACKUP2"

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
if compiled_ok "$WORK/mut.log"; then
  say "M2 COMPILED: yes"
else
  say "M2 COMPILED: no"
  fail "M2 the mutant did not compile — a red from a build failure is NOT evidence"
  grep -E '^error' "$WORK/mut.log" | head -5
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

# --- M6..M8: the SECOND mutation — a mode-preserving writer -----------------
#
# The first mutation removes containment. This one removes the guard that keeps
# an imported peer SCRIPT inert, and it exists because t25's mode assertion is
# NOT self-evidently discriminating: `fs::write` produces 0644 on a new path
# regardless, so simply DELETING `strip_execute_bits` leaves t25 green and the
# guard would read as load-bearing while doing nothing.
#
# The realistic regression is not deletion, it is a copy-based writer: the
# obvious refactor of `write_tree` to `fs::copy` (or any implementation that
# carries the source mode over) reintroduces 0755 imports. That is what this
# mutation simulates, and t25 MUST go red under it or the guard is decoration.
say
say "--- M6 mutation 2: write_tree preserves the source execute bit ---"
cp "$BACKUP" "$TARGET"        # undo mutation 1 before layering the next
python3 - "$TARGET2" <<'PY2'
import sys
p = sys.argv[1]
s = open(p, encoding='utf-8').read()
old = """        fs::write(&target, bytes)?;
        strip_execute_bits(&target)?;"""
new = """        fs::write(&target, bytes)?;
        // F26-KNOWN-NEGATIVE MUTATION 2 — a copy-based writer carries the
        // source mode over. The guard is removed on purpose.
        #[cfg(unix)]
        if tree.executable.contains(rel) {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&target)?.permissions();
            let m = perms.mode();
            perms.set_mode(m | 0o111);
            fs::set_permissions(&target, perms)?;
        }"""
if old not in s:
    sys.stderr.write("MUTATION2-ANCHOR-MISSING\n")
    sys.exit(3)
open(p, 'w', encoding='utf-8').write(s.replace(old, new, 1))
PY2
if [ $? -ne 0 ] || cmp -s "$BACKUP2" "$TARGET2"; then
  say "M6 APPLIED: no"
  fail "M6 could not apply mutation 2; a green run below would be meaningless"
else
  say "M6 APPLIED: yes ($(diff "$BACKUP2" "$TARGET2" | grep -c '^[<>]') changed lines)"
  ( cd "$ROOT" && "$CARGO" test -p wcore-cli --test migrate_quarantine ) \
    >"$WORK/mut2.log" 2>&1
  if compiled_ok "$WORK/mut2.log"; then
    say "M7 COMPILED: yes"
  else
    say "M7 COMPILED: no"
    fail "M7 mutant 2 did not compile — a red from a build failure is NOT evidence"
    grep -E '^error' "$WORK/mut2.log" | head -5
  fi
  mut2_line="$(grep -E '^test result:' "$WORK/mut2.log" | tail -1)"
  say "M8 ${mut2_line:-<no test result line>}"
  say "M8 the tests that went red:"
  grep -E '^test .* FAILED$' "$WORK/mut2.log" | sed 's/^/    /' || true
  for t in "${REQUIRED_RED2[@]}"; do
    if grep -qE "^test $t \.\.\. FAILED$" "$WORK/mut2.log"; then
      say "M8 REQUIRED-RED $t: FAILED (as required)"
    else
      fail "M8 REQUIRED-RED $t stayed green with the exec-bit guard removed"
    fi
  done
fi
cp "$BACKUP2" "$TARGET2"

# --- M4 + M5: restore, prove the restore, and re-run to GREEN ---------------
say
say "--- M4/M5: restore and re-prove ---"
cp "$BACKUP" "$TARGET"
if cmp -s "$BACKUP" "$TARGET" && cmp -s "$BACKUP2" "$TARGET2"; then
  say "M4 RESTORED: yes (both files byte-identical to their pre-mutation state)"
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
