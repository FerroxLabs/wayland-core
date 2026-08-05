#!/usr/bin/env bash
# 26-SC2-PEERS — the three-assertion self-test for the live proof.
#
# `f26-sc2-peers-live-proof.sh` passing tells you nothing on its own: an
# instrument that always says PASS also passes. This runs the three assertions
# the programme requires, against the REAL release binary:
#
#   A  KNOWN-POSITIVE     the unmutated build passes.
#   B  KNOWN-NEGATIVE     a build with the exec-bit mitigation removed FAILS,
#                         and fails at the specific assertions that measure it.
#   C  OLD-SHAPE-MISSES   the base commit — before this lane — cannot even be
#                         asked the question, because `migrate grok` and
#                         `migrate gemini` do not exist there.
#   D  RESTORED           the tree returns to green at the same commit, which is
#                         what makes B's red attributable to the mutation.
#
# B mutates `write_tree` into a MODE-PRESERVING writer rather than deleting
# `strip_execute_bits`. Deletion is not discriminating: `fs::write` yields 0644
# on a new path anyway, so a deleted guard would leave the proof green while
# doing nothing. The realistic regression is a copy-based writer, and that is
# what is simulated — the same reasoning `f26-quarantine-known-negative.sh`
# records for its mutation 2.
#
# USAGE: f26-sc2-peers-known-negative.sh <repo-root> <staged-dir> <base-sha>
set -uo pipefail

REPO="${1:?usage: f26-sc2-peers-known-negative.sh <repo> <staged> <base-sha>}"
STAGED="${2:?}"
BASE_SHA="${3:?}"
CARGO=/root/.cargo/bin/cargo
BIN="$REPO/target/release/wayland-core"
PROOF="$(dirname "$0")/f26-sc2-peers-live-proof.sh"
[ -x "$PROOF" ] || PROOF=/tmp/lane-26-sc2-peers-live-proof.sh
TARGET="$REPO/crates/wcore-cli/src/migrate/content.rs"
BACKUP=/tmp/lane-26-sc2-peers-content.rs.bak
W=/tmp/lane-26-sc2-peers-kn
rc=0
say()  { printf '%s\n' "$*"; }
fail() { say "FAIL: $*"; rc=1; }
ok()   { say "PASS: $*"; }

rm -rf "$W"; mkdir -p "$W"
cp "$TARGET" "$BACKUP"
restore() { cp "$BACKUP" "$TARGET"; }
trap restore EXIT

say "=== 26-SC2-PEERS known-negative self-test ==="
say "repo    : $REPO"
say "base    : $BASE_SHA"

# ---------------------------------------------------------------------------
# A — KNOWN-POSITIVE
# ---------------------------------------------------------------------------
say
say "--- A: known-positive (unmutated build) ---"
"$PROOF" "$BIN" "$STAGED" "$W/a" >"$W/a.log" 2>&1
A_RC=$?
say "A-RC=$A_RC   $(grep -c '^PASS:' "$W/a.log") PASS / $(grep -c '^FAIL:' "$W/a.log") FAIL"
if [ "$A_RC" -eq 0 ]; then ok "A the unmutated build passes"; else
  fail "A the unmutated build already fails — B proves nothing"; fi

# ---------------------------------------------------------------------------
# B — KNOWN-NEGATIVE: a mode-preserving writer
# ---------------------------------------------------------------------------
say
say "--- B: known-negative — write_tree preserves the source execute bit ---"
python3 - "$TARGET" <<'PY'
import sys
p = sys.argv[1]
s = open(p, encoding='utf-8').read()
old = """        fs::write(&target, bytes)?;
        strip_execute_bits(&target)?;"""
new = """        fs::write(&target, bytes)?;
        // 26-SC2-PEERS KNOWN-NEGATIVE MUTATION — a copy-based writer carries
        // the source mode over. The guard is removed on purpose.
        #[cfg(unix)]
        if tree.executable.contains(rel) {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&target)?.permissions();
            let m = perms.mode();
            perms.set_mode(m | 0o111);
            fs::set_permissions(&target, perms)?;
        }"""
if old not in s:
    sys.stderr.write("MUTATION-ANCHOR-MISSING\n"); sys.exit(3)
open(p, 'w', encoding='utf-8').write(s.replace(old, new, 1))
PY
if [ $? -ne 0 ] || cmp -s "$BACKUP" "$TARGET"; then
  say "B APPLIED: no"
  fail "B the mutation did not apply; the red below would be meaningless"
else
  say "B APPLIED: yes ($(diff "$BACKUP" "$TARGET" | grep -c '^[<>]') changed lines)"
fi

(cd "$REPO" && "$CARGO" build -p wcore-cli --release --bin wayland-core) \
  >"$W/b-build.log" 2>&1
B_BUILD=$?
if [ "$B_BUILD" -eq 0 ] && grep -q 'Finished' "$W/b-build.log"; then
  say "B COMPILED: yes"
else
  say "B COMPILED: no"; tail -20 "$W/b-build.log"
  fail "B the mutant did not build; nothing below is a measurement"
fi

"$PROOF" "$BIN" "$STAGED" "$W/b" >"$W/b.log" 2>&1
B_RC=$?
say "B-RC=$B_RC   $(grep -c '^PASS:' "$W/b.log") PASS / $(grep -c '^FAIL:' "$W/b.log") FAIL"
if [ "$B_RC" -eq 0 ]; then
  fail "B the proof stayed GREEN with the mitigation removed — it cannot fail"
else
  ok "B the proof goes RED with the mitigation removed"
fi
say "B REQUIRED-RED assertions, verbatim:"
grep '^FAIL:' "$W/b.log" | sed 's/^/    /'
for want in 'grok X2' 'grok X4' 'gemini X2' 'gemini X4'; do
  if grep -q "^FAIL: $want" "$W/b.log"; then
    ok "B the specific assertion '$want' went red"
  else
    fail "B '$want' stayed green under the mutation — it does not measure the mitigation"
  fi
done

# ---------------------------------------------------------------------------
# C — OLD SHAPE MISSES IT
# ---------------------------------------------------------------------------
say
say "--- C: the base commit cannot be asked the question at all ---"
BASEDIR=/tmp/lane-26-sc2-peers-base
rm -rf "$BASEDIR"
git -C "$REPO" worktree remove --force "$BASEDIR" 2>/dev/null
git -C "$REPO" worktree add --detach "$BASEDIR" "$BASE_SHA" >"$W/c-wt.log" 2>&1 \
  || { fail "C could not create the base worktree"; cat "$W/c-wt.log"; }
if [ -d "$BASEDIR" ]; then
  say "base HEAD: $(git -C "$BASEDIR" rev-parse HEAD)"
  (cd "$BASEDIR" && "$CARGO" build -p wcore-cli --bin wayland-core) \
    >"$W/c-build.log" 2>&1
  C_BUILD=$?
  if [ "$C_BUILD" -eq 0 ]; then
    say "C base binary built (debug)"
    for peer in grok gemini; do
      "$BASEDIR/target/debug/wayland-core" migrate "$peer" \
        --home "$STAGED/$peer-home" --yes >"$W/c-$peer.log" 2>&1
      say "  base: migrate $peer -> rc=$?"
      sed 's/^/      /' "$W/c-$peer.log" | head -4
      if grep -qiE "unrecognized subcommand|unexpected argument|invalid value" "$W/c-$peer.log"; then
        ok "C the base build has no '$peer' importer — the old shape could not have caught this"
      else
        fail "C the base build accepted 'migrate $peer'; the gap was not what this lane claimed"
      fi
    done
    # Positive control on the SAME binary: a subcommand that DOES exist there,
    # so C's refusals are about the missing importers, not a broken invocation.
    "$BASEDIR/target/debug/wayland-core" migrate quarantined >"$W/c-ctl.log" 2>&1
    say "  base: migrate quarantined -> rc=$?"
    if grep -qiE "unrecognized subcommand" "$W/c-ctl.log"; then
      fail "C-CONTROL the base binary rejects even a subcommand it HAS — C is a dead instrument"
    else
      ok "C-CONTROL the base binary accepts 'migrate quarantined', so C measured absence not breakage"
    fi
  else
    tail -20 "$W/c-build.log"
    fail "C the base build failed; C is unmeasured"
  fi
fi

# ---------------------------------------------------------------------------
# D — RESTORED
# ---------------------------------------------------------------------------
say
say "--- D: restore and re-verify at the same commit ---"
restore
if cmp -s "$BACKUP" "$TARGET"; then say "D RESTORED: yes"; else
  fail "D content.rs was not restored"; fi
(cd "$REPO" && "$CARGO" build -p wcore-cli --release --bin wayland-core) \
  >"$W/d-build.log" 2>&1
"$PROOF" "$BIN" "$STAGED" "$W/d" >"$W/d.log" 2>&1
D_RC=$?
say "D-RC=$D_RC   $(grep -c '^PASS:' "$W/d.log") PASS / $(grep -c '^FAIL:' "$W/d.log") FAIL"
if [ "$D_RC" -eq 0 ]; then
  ok "D green again at the same commit — B's red is attributable to the mutation"
else
  fail "D the tree did not return to green"
fi

git -C "$REPO" worktree remove --force "$BASEDIR" 2>/dev/null

say
if [ "$rc" -eq 0 ]; then say "KNOWN-NEGATIVE SELF-TEST: PASS"; else
  say "KNOWN-NEGATIVE SELF-TEST: FAIL"; fi
exit "$rc"
