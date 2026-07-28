#!/usr/bin/env bash
# F24-C3 mutation proofs. A regression test that cannot fail is not evidence.
#
# Each mutation restores the EXACT pre-fix expression and asserts the named
# test goes red, then restores the fix and asserts it goes green again. The
# restore-and-recheck is not ceremony: a mutation that left the tree broken
# would make every later figure meaningless.
#
# Exit status is captured on the line after each command — never through a
# pipe, which reports only its last stage.
set -uo pipefail
export PATH=/root/.cargo/bin:$PATH
cd /root/wayland-24c3

OUT=/root/f24c3/mutations.log
: > "$OUT"
say() { echo "$*" | tee -a "$OUT"; }

# Run one test target and classify it. Returns 0 green / 1 red / 3 ran-nothing.
# "ran nothing" is graded separately from "passed" because a suite that
# executes zero tests exits 0 and would otherwise read as a pass.
run_named() {
  local pkg="$1" testfile="$2" name="$3" log="$4"
  cargo test -p "$pkg" --test "$testfile" -- --exact "$name" > "$log" 2>&1
  local rc=$?
  local ran
  ran=$(grep -oE '[0-9]+ passed' "$log" | head -1 | grep -oE '[0-9]+')
  ran="${ran:-0}"
  local failed
  failed=$(grep -oE '[0-9]+ failed' "$log" | head -1 | grep -oE '[0-9]+')
  failed="${failed:-0}"
  echo "  rc=${rc} passed=${ran} failed=${failed}"
  if [ "$ran" = "0" ] && [ "$failed" = "0" ]; then return 3; fi
  if [ "$rc" -ne 0 ]; then return 1; fi
  return 0
}

verdict() {
  case "$1" in
    0) echo "GREEN" ;;
    1) echo "RED" ;;
    3) echo "RAN-NOTHING" ;;
    *) echo "UNKNOWN($1)" ;;
  esac
}

say "=== commit under mutation ==="
git rev-parse HEAD | tee -a "$OUT"

# ── M1: F24-C3-H1, the policy home ──────────────────────────────────────────
say ""
say "=== M1: point the policy loader back at ChannelConfigLoader::default_root() ==="
AGENT=crates/wcore-agent/src/bootstrap.rs
cp "$AGENT" /tmp/bootstrap.orig
python3 - <<'PY'
p='crates/wcore-agent/src/bootstrap.rs'
s=open(p).read()
old='wcore_channels::config::ChannelConfigLoader::new(wcore_channels_registry::channels_dir())'
new='wcore_channels::config::ChannelConfigLoader::new(wcore_channels::config::ChannelConfigLoader::default_root())'
assert s.count(old)==1, f"expected exactly 1 site, found {s.count(old)}"
open(p,'w').write(s.replace(old,new))
print("M1 applied")
PY
mut_rc=$?
say "mutation applied rc=${mut_rc}"
[ "$mut_rc" -ne 0 ] && { say "M1 ABORTED: could not apply"; cp /tmp/bootstrap.orig "$AGENT"; exit 3; }

say "M1 mutated:"
run_named wcore-agent f24_c3_inbound_policy_home_test \
  inbound_policy_is_read_from_the_profile_home_not_the_host_home /tmp/m1.log | tee -a "$OUT"
m1_mut=${PIPESTATUS[0]}
say "M1 mutated verdict: $(verdict $m1_mut)  (want RED)"
grep -E '^\s+(left|right|assertion|the policy loader)' /tmp/m1.log | head -6 | tee -a "$OUT"

cp /tmp/bootstrap.orig "$AGENT"
say "M1 restored:"
run_named wcore-agent f24_c3_inbound_policy_home_test \
  inbound_policy_is_read_from_the_profile_home_not_the_host_home /tmp/m1r.log | tee -a "$OUT"
m1_res=${PIPESTATUS[0]}
say "M1 restored verdict: $(verdict $m1_res)  (want GREEN)"

# ── M2: F24-C3-H3, the SMS conversation id ──────────────────────────────────
say ""
say "=== M2: put the SMS conversation id back to the bot's own number ==="
SMS=crates/wcore-channel-sms/src/inbound.rs
cp "$SMS" /tmp/sms.orig
python3 - <<'PY'
p='crates/wcore-channel-sms/src/inbound.rs'
s=open(p).read()
# rustfmt splits this call across lines, so the mutation targets the
# multi-line form. The count assertion below is what catches a drift in
# either direction: a mutation that matched nothing would leave the tree
# unchanged and the test would 'pass', reading as a proof it is not.
old='''..IncomingMessage::new(
            sid,
            from.clone(),
            from,'''
new='''..IncomingMessage::new(
            sid,
            to,
            from,'''
assert s.count(old)==1, f"expected exactly 1 site, found {s.count(old)}"
open(p,'w').write(s.replace(old,new))
print("M2 applied")
PY
mut_rc=$?
say "mutation applied rc=${mut_rc}"
[ "$mut_rc" -ne 0 ] && { say "M2 ABORTED"; cp /tmp/sms.orig "$SMS"; exit 3; }

for t in the_conversation_is_the_peer_not_the_bots_own_number \
         two_people_texting_the_same_bot_number_do_not_share_a_session; do
  say "M2 mutated — ${t}:"
  run_named wcore-channel-sms f24_c3_sms_peer_conversation_test "$t" "/tmp/m2-${t}.log" | tee -a "$OUT"
  rc=${PIPESTATUS[0]}
  say "  verdict: $(verdict $rc)  (want RED)"
done

cp /tmp/sms.orig "$SMS"
for t in the_conversation_is_the_peer_not_the_bots_own_number \
         two_people_texting_the_same_bot_number_do_not_share_a_session; do
  say "M2 restored — ${t}:"
  run_named wcore-channel-sms f24_c3_sms_peer_conversation_test "$t" "/tmp/m2r-${t}.log" | tee -a "$OUT"
  rc=${PIPESTATUS[0]}
  say "  verdict: $(verdict $rc)  (want GREEN)"
done

say ""
say "=== tree restored to HEAD? (empty diff expected) ==="
git diff --stat -- crates/wcore-agent/src/bootstrap.rs crates/wcore-channel-sms/src/inbound.rs | tee -a "$OUT"
# `git diff --stat` exits 0 unconditionally, so its status is NOT the gate.
# `--quiet` exits 1 on any difference, which is.
git diff --quiet -- crates/wcore-agent/src/bootstrap.rs crates/wcore-channel-sms/src/inbound.rs
say "restore check rc=$? (0 = tree identical to HEAD)"
echo "WLDONE"
