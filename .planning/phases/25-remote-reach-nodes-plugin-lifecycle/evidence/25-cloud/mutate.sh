#!/bin/bash
# Mutation controls for the F25 cloud hibernation discriminator.
# Each mutation must redden EXACTLY the test that claims to cover it.
export PATH=/root/.cargo/bin:$PATH
cd /root/wayland-25-cloud || exit 1
F=crates/wcore-exec-backend/src/backends/cloud.rs
cp "$F" /tmp/cloud.rs.pristine

run() {
  cargo test -p wcore-exec-backend --lib cloud::tests 2>&1 \
    | grep -E '^test result|FAILED'
}

mutate() {
  python3 -c "
import sys
p='crates/wcore-exec-backend/src/backends/cloud.rs'
s=open(p).read()
old=sys.argv[1]; new=sys.argv[2]
if old not in s:
    print('MUTATION DID NOT APPLY - needle absent:', old); sys.exit(9)
open(p,'w').write(s.replace(old,new,1))
" "$1" "$2" || exit 9
}

echo "########## CONTROL: PRISTINE must be ACCEPTED ##########"
run

echo
echo "########## M1: drop the previous_state clause ##########"
mutate 'if previous_state != "suspended" {' 'if false {'
run
cp /tmp/cloud.rs.pristine "$F"

echo
echo "########## M2: drop the RAM-witness clause ##########"
mutate 'if after.witness != planted {' 'if false {'
run
cp /tmp/cloud.rs.pristine "$F"

echo
echo "########## M3: drop the boot-id clause ##########"
mutate 'if before.boot_id.is_empty() || after.boot_id != before.boot_id {' 'if false {'
run
cp /tmp/cloud.rs.pristine "$F"

echo
echo "########## M4: drop the nonce tag from machine create ##########"
mutate '"metadata": { (NONCE_METADATA_KEY): task.nonce },' '"metadata": {},'
run
cp /tmp/cloud.rs.pristine "$F"

echo
echo "########## RESTORED: must be ACCEPTED again ##########"
run
echo "--- restoration check ---"
git diff --exit-code -- "$F" >/dev/null 2>&1 && echo "RESTORED CLEAN" || echo "FILE STILL DIRTY"
