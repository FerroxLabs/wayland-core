#!/bin/sh
# LIVE exercise v2 — repairs the instrument defect in v1 and adds the
# journal-exists-but-key-is-gone measurement.
#
# v1 DEFECT, found by reading its own log: three of five arms passed session ids
# beginning "dp", which the product rejects ("must be 6-40 hex characters").
# Those arms died before reaching the code under test, so the two CONTROL arms
# never ran and the fifth arm measured a profile that had never journalled. v1's
# two real arms were unaffected (they are re-run here and must agree).
#
# §6b-ii: the instrument is repaired in the same lane, and the repair carries a
# self-test — every arm asserts it got PAST session-id validation, so this exact
# defect cannot recur silently.
set -u
BIN=/root/dp-wayland-core-56d6ff89
SEEDER=$(/bin/ls -t /root/wayland-durable-posture/target/debug/deps/f14_sigkill_recovery-* 2>/dev/null | /usr/bin/grep -v '\.d$' | head -1)
ROOT=/root/dp-live2-$$
OUT=/root/wayland-durable-posture/dp-live2.log
: > "$OUT"

echo "BINARY=$BIN" >> "$OUT"
"$BIN" --version >> "$OUT" 2>&1
echo "SEEDER=$SEEDER" >> "$OUT"
echo "HOSTNAME=$(hostname)" >> "$OUT"

# All ids hex, 6-40 chars — the v1 defect.
ID_DEGRADE=da00000000000000000000000000001
ID_REQUIRE=da00000000000000000000000000002
ID_VAULT=da00000000000000000000000000003
ID_REQVAULT=da00000000000000000000000000004
ID_SEEDED=da00000000000000000000000000005

report() {
  name="$1"; home="$2"; outf="$3"; errf="$4"
  python3 - "$outf" "$name" >> "$OUT" 2>&1 <<'PY'
import json, sys
path, name = sys.argv[1], sys.argv[2]
types, saw_ready, bad_id = [], False, False
for line in open(path, encoding="utf-8", errors="replace"):
    line = line.strip()
    if not line:
        continue
    try:
        frame = json.loads(line)
    except json.JSONDecodeError:
        types.append("<unparseable>")
        continue
    ty = frame.get("type")
    if ty == "ready":
        saw_ready = True
        types.append(f"ready(session_id={frame.get('session_id')!r})")
    elif ty == "error":
        msg = frame.get("error", {}).get("message", "")
        if "must be 6-40 hex" in msg:
            bad_id = True
        types.append(f"error(code={frame.get('error', {}).get('code')!r},"
                     f"retryable={frame.get('error', {}).get('retryable')!r})")
        print(f"ERROR_MESSAGE[{name}]:", msg)
print(f"FRAME_TYPES[{name}]:", types[:3], "...", len(types), "frames")
# SELF-TEST for the v1 defect: no arm may die on session-id validation.
print(f"REACHED_CODE_UNDER_TEST[{name}]:", "NO -- INVALID SESSION ID" if bad_id
      else ("yes(ready)" if saw_ready else "yes(refused before ready)"))
PY
  echo "NOTICE[$name]=$(/usr/bin/grep -ac 'durable session persistence is OFF' "$errf")" >> "$OUT"
  echo "SESSION_ENTRIES[$name]=$(find "$home/sessions" -mindepth 1 2>/dev/null | wc -l | tr -d ' ')" >> "$OUT"
  find "$home/sessions" -mindepth 1 2>/dev/null | sed "s#$home/sessions/#    #" >> "$OUT"
}

arm() {
  name="$1"; require="$2"; vault="$3"; session="$4"
  home="$ROOT/$name/.wayland-core"
  mkdir -p "$home/sessions" "$ROOT/$name/proj"
  {
    printf '[default]\nprovider = "anthropic"\nmodel = "claude-sonnet-4-20250514"\n\n'
    printf '[session]\ndirectory = "%s/sessions"\n' "$home"
    [ "$require" = yes ] && printf 'require_durability = true\n'
  } > "$home/config.toml"
  echo "" >> "$OUT"
  echo "########## ARM $name (require=$require vault=$vault) ##########" >> "$OUT"
  set -- env -u DBUS_SESSION_BUS_ADDRESS -u WAYLAND_VAULT_PASSPHRASE -u WAYLAND_VAULT_PASSPHRASE_FD \
      HOME="$ROOT/$name" USERPROFILE="$ROOT/$name" WAYLAND_HOME="$home" TERM=dumb \
      ANTHROPIC_API_KEY=sk-ant-not-a-real-key-0000000000000000
  [ "$vault" = yes ] && set -- "$@" WAYLAND_VAULT_PASSPHRASE=dp-live-throwaway-not-a-real-secret
  "$@" "$BIN" --json-stream --project-dir "$ROOT/$name/proj" --session-id "$session" \
      < /dev/null > "$ROOT/$name.out" 2> "$ROOT/$name.err"
  echo "ARM_RC[$name]=$?" >> "$OUT"
  report "$name" "$home" "$ROOT/$name.out" "$ROOT/$name.err"
}

arm degrade-default   no  no  "$ID_DEGRADE"
arm require-refuses   yes no  "$ID_REQUIRE"
arm vault-control     no  yes "$ID_VAULT"
arm require-with-vault yes yes "$ID_REQVAULT"

# --------------------------------------------------------------------------
# THE MEASUREMENT: a profile that HAS a real recovery journal, reopened after
# its key has gone away.
#
# The journal is produced by the product's own seeder
# (`f14_seed_recoverable_turn_helper`), which builds a production-shaped engine
# and persists a recoverable turn. A shell arm cannot do this: completing a turn
# needs a provider, and the seeder uses the in-process ScriptedProvider.
# --------------------------------------------------------------------------
echo "" >> "$OUT"
echo "########## SEED: build a real journal under a vault ##########" >> "$OUT"
SEEDHOME="$ROOT/seeded/.wayland-core"
mkdir -p "$SEEDHOME/sessions" "$ROOT/seeded/proj"
printf '[default]\nprovider = "openai"\nmodel = "fixture-chat-v1"\n\n[session]\ndirectory = "%s/sessions"\n' \
  "$SEEDHOME" > "$SEEDHOME/config.toml"
env -u DBUS_SESSION_BUS_ADDRESS \
  HOME="$ROOT/seeded" USERPROFILE="$ROOT/seeded" WAYLAND_HOME="$SEEDHOME" TERM=dumb \
  OPENAI_API_KEY=fixture-local-token \
  WAYLAND_VAULT_PASSPHRASE=dp-live-throwaway-not-a-real-secret \
  WAYLAND_F14_SEED_SESSION_ID="$ID_SEEDED" \
  WAYLAND_F14_SEED_TURN_ID=turn-dp-live-0001 \
  WAYLAND_F14_SEED_PROMPT="DP-LIVE-SEEDED-PROMPT" \
  WAYLAND_F14_SEED_BASE_URL="http://127.0.0.1:1/v1" \
  WAYLAND_F14_SEED_WORKSPACE="$ROOT/seeded/proj" \
  WAYLAND_F14_SEED_DESKTOP_LAUNCH=0 \
  "$SEEDER" --exact f14_seed_recoverable_turn_helper --ignored --nocapture \
  > "$ROOT/seed.out" 2> "$ROOT/seed.err"
echo "SEED_RC=$?" >> "$OUT"
/usr/bin/grep -a "test result:" "$ROOT/seed.out" >> "$OUT"
echo "SEEDED_ENTRIES=$(find "$SEEDHOME/sessions" -mindepth 1 2>/dev/null | wc -l | tr -d ' ')" >> "$OUT"
find "$SEEDHOME/sessions" -mindepth 1 2>/dev/null | sed "s#$SEEDHOME/sessions/#    #" >> "$OUT"

echo "" >> "$OUT"
echo "########## ARM journal-exists-key-gone (relaunch WITHOUT the vault) ##########" >> "$OUT"
env -u DBUS_SESSION_BUS_ADDRESS -u WAYLAND_VAULT_PASSPHRASE -u WAYLAND_VAULT_PASSPHRASE_FD \
  HOME="$ROOT/seeded" USERPROFILE="$ROOT/seeded" WAYLAND_HOME="$SEEDHOME" TERM=dumb \
  OPENAI_API_KEY=fixture-local-token \
  "$BIN" --json-stream --project-dir "$ROOT/seeded/proj" --resume "$ID_SEEDED" \
  < /dev/null > "$ROOT/relaunch.out" 2> "$ROOT/relaunch.err"
echo "ARM_RC[journal-exists-key-gone]=$?" >> "$OUT"
report journal-exists-key-gone "$SEEDHOME" "$ROOT/relaunch.out" "$ROOT/relaunch.err"

echo "" >> "$OUT"
echo "ROOT=$ROOT" >> "$OUT"
echo "WLDONE" >> "$OUT"
