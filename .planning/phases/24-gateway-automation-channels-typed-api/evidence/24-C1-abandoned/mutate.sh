#!/bin/bash
# Mutation proof: every gate this lane added must be SHOWN able to fail.
#
# Each mutation reverts one specific part of the fix and must redden exactly the
# tests that assert that part. A gate that stays green under its own mutation is
# not testing what it claims to.
#
# The tree is restored with `git checkout -- <file>` (a single named path, never
# a blanket reset — other lanes share this repository's object store).
set -u
export PATH=/root/.cargo/bin:$PATH
R=/root/wayland-24-c1-abandoned
L=$R/crates/wcore-gateway/src/ledger.rs
G=$R/crates/wcore-cli/src/gateway.rs
cd $R

run() { # run(label, crate, extra...)
  echo "--- $1 ---"
  cargo test -p "$2" --lib 2>&1 | grep -E "^test result|FAILED|panicked at|^test .*FAILED" | head -20
}

restore() { git -C $R checkout -- "$1"; }

echo "=== BASELINE (must be all green) ==="
run "baseline wcore-gateway" wcore-gateway
run "baseline wcore-cli" wcore-cli

echo ""
echo "=== M1: compaction bounds ALL abandonments again (the pre-lane behaviour) ==="
python3 - <<PYEOF
import re
p="$L"; s=open(p).read()
s=s.replace(
 '.chain(unacknowledged)\n            .chain(acknowledged.into_iter().skip(abandoned_from))',
 '.chain(unacknowledged.into_iter().skip(0))\n            .chain(acknowledged.into_iter().skip(abandoned_from))')
# make the cap apply to the union, as it did before
s=s.replace('let abandoned_from = acknowledged.len().saturating_sub(ABANDON_RETENTION);',
            'let abandoned_from = (acknowledged.len()+unacknowledged.len()).saturating_sub(ABANDON_RETENTION);')
s=s.replace('.chain(unacknowledged.into_iter().skip(0))',
            '.chain(unacknowledged.into_iter().skip(abandoned_from))')
open(p,"w").write(s)
PYEOF
run "M1" wcore-gateway
restore "$L"

echo ""
echo "=== M2: a re-send silently acknowledges (the rejected auto-ack design) ==="
python3 - <<PYEOF
p="$L"; s=open(p).read()
s=s.replace("""        rec.resent = Some(Self::now());
        self.append_record(rec)""",
"""        rec.resent = Some(Self::now());
        rec.acknowledged = Some(Self::now());
        self.append_record(rec)""")
open(p,"w").write(s)
PYEOF
run "M2" wcore-gateway
restore "$L"

echo ""
echo "=== M3: acknowledge overwrites the first review time ==="
python3 - <<PYEOF
p="$L"; s=open(p).read()
s=s.replace("""        if rec.acknowledged.is_some() {
            return Ok(());
        }
""","")
open(p,"w").write(s)
PYEOF
run "M3" wcore-gateway
restore "$L"

echo ""
echo "=== M4: abandon stops recording whether an attempt had started ==="
python3 - <<PYEOF
p="$L"; s=open(p).read()
s=s.replace("""                Some(r.state == DeliveryState::Attempted),""",
            """                None,""")
open(p,"w").write(s)
PYEOF
run "M4" wcore-gateway
restore "$L"

echo ""
echo "=== M5: ack/resend accept a delivery that was never abandoned ==="
python3 - <<PYEOF
p="$L"; s=open(p).read()
s=s.replace("""            Some(r) if r.state != DeliveryState::Abandoned => Err(LedgerError::NotAbandoned {
                id: id.to_string(),
                state: Some(r.state),
            }),
""","")
open(p,"w").write(s)
PYEOF
run "M5" wcore-gateway
restore "$L"

echo ""
echo "=== M6: the resend guard treats UNKNOWN as safe (the duplicate-authorising bug) ==="
python3 - <<PYEOF
p="$G"; s=open(p).read()
s=s.replace("    was_attempted != Some(false)\n","    was_attempted == Some(true)\n")
open(p,"w").write(s)
PYEOF
run "M6" wcore-cli
restore "$G"

echo ""
echo "=== TREE RESTORED — must be clean ==="
git -C $R status --porcelain -- crates/wcore-gateway/src/ledger.rs crates/wcore-cli/src/gateway.rs
echo "MUTATE_DONE"
