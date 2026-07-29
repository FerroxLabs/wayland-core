#!/usr/bin/env bash
# F24-C3 crate gates. Every invocation's EXECUTED COUNT is read back, not its
# exit status.
#
# Four measured ways a cargo invocation reports success having run zero tests:
# every test `#[ignore]`d; an env-gated early `return`; a filter matching no
# test name; a file-level `#![cfg(feature=...)]`. Reading `N passed` back is
# necessary but not sufficient — one suite in this repo prints `8 passed` from
# a support module while running neither real case — so each line below names
# the count it expects and the specific test names it expects to see.
set -uo pipefail
export PATH=/root/.cargo/bin:$PATH
cd /root/wayland-24c3

OUT=/root/f24c3/tests.log
: > "$OUT"
say() { echo "$*" | tee -a "$OUT"; }

say "=== commit under test ==="
git rev-parse HEAD | tee -a "$OUT"

say ""
say "=== 1. wcore-channel-sms (F24-C3-H3 fix + its regression test) ==="
# vacuity-checked: rc via PIPESTATUS[0]; executed count read back from /tmp/sms.txt below.
cargo test -p wcore-channel-sms 2>&1 | tee -a "$OUT" | grep -E "^test result:|^running|^test f24|^error" | tee /tmp/sms.txt
rc1=${PIPESTATUS[0]}
say "rc=${rc1}"

say ""
say "=== 2. wcore-agent, the F24-C3-H1 policy-home test, BY FILE ==="
# By file (--test <name>), not by filter: a filter that matches no test name
# exits 0 having run nothing, and is the easiest of the four flavours to write
# by accident.
# vacuity-checked: rc via PIPESTATUS[0]; executed count read back from /tmp/agent.txt below.
cargo test -p wcore-agent --test f24_c3_inbound_policy_home_test 2>&1 | tee -a "$OUT" | grep -E "^test result:|^running|^test |^error" | tee /tmp/agent.txt
rc2=${PIPESTATUS[0]}
say "rc=${rc2}"

say ""
say "=== 3. wcore-channels (the session-key kernel H3 turns on) ==="
# vacuity-checked: rc via PIPESTATUS[0]; executed count read back from /tmp/channels.txt below.
cargo test -p wcore-channels 2>&1 | tee -a "$OUT" | grep -E "^test result:|^error" | tee /tmp/channels.txt
rc3=${PIPESTATUS[0]}
say "rc=${rc3}"

say ""
say "=== EXECUTED COUNTS, read back ==="
say "--- wcore-channel-sms ---"; cat /tmp/sms.txt
say "--- wcore-agent policy-home ---"; cat /tmp/agent.txt
say "--- wcore-channels ---"; cat /tmp/channels.txt

say ""
say "=== named-test presence (a count alone is not enough) ==="
grep -c 'the_conversation_is_the_peer_not_the_bots_own_number' /tmp/sms.txt /tmp/agent.txt 2>/dev/null
grep -E 'two_people_texting_the_same_bot_number_do_not_share_a_session|inbound_policy_is_read_from_the_profile_home' /tmp/sms.txt /tmp/agent.txt | tee -a "$OUT"

say ""
say "RC1=${rc1} RC2=${rc2} RC3=${rc3}"
echo "WLDONE"
