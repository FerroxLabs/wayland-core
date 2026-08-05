#!/bin/bash
# F24-C3-H6 live degradation probe, v3.
#
# v1 defect (mine): started the binary with `< /dev/null`. `--json-stream` is a
# stdio protocol surface, so stdin at EOF means the peer hung up and it shut
# down after two syncs. v1's NOT-WEDGED check then read a stalled sync counter
# off an ALREADY-EXITED process -- it would have reported `syncs 5 -> 5` as a
# product wedge. A fabricated finding of the exact class this program keeps
# hitting (an unchecked liveness assumption; cf. the zombie that read as
# "ignored SIGTERM for 30s").
# v2 defect (also mine): the stdin holder was a FIFO opened inside a command
# substitution; the binary never started at all and every counter read 0.
# v3: stdin is held by `tail -f /dev/null`, the pid is taken from the OS, and
# every reading is paired with a zombie-aware liveness state so an exited
# process can never again be read as a wedged one.
set -u
BIN=/root/wayland-24-h6/target/release/wayland-core
RUN=/root/h6-runs/corrupt3
rm -rf "$RUN"; mkdir -p "$RUN/home/channels"
cd /root/wayland-24-h6 || exit 9
pkill -f 'wayland-core --json-stream' 2>/dev/null; sleep 1
TOK=$(openssl rand -hex 16)

node scripts/f24-matrix-fixture.mjs --journal "$RUN/mx.jsonl" --token "$TOK" \
  --room '!h6room:f24.invalid:2' --max-wait-ms 2000 > "$RUN/fixture.log" 2>&1 &
FIXPID=$!
for i in $(seq 1 30); do
  URL=$(grep -o 'MXFIX_READY url=\S*' "$RUN/fixture.log" 2>/dev/null | head -1 | cut -d= -f2)
  [ -n "${URL:-}" ] && break; sleep 1
done
[ -z "${URL:-}" ] && { echo "FIXTURE-DID-NOT-START"; kill $FIXPID 2>/dev/null; exit 8; }
echo "fixture up at $URL"

printf '[secrets]\n"matrix.h6.access_token" = "%s"\n' "$TOK" > "$RUN/home/credentials.toml"
chmod 600 "$RUN/home/credentials.toml"
printf '[default]\nprovider = "h6fixture"\n\n[providers.h6fixture]\nprovider = "openai"\nmodel = "h6-fixture"\napi_key = "h6-not-a-real-key"\nbase_url = "http://127.0.0.1:1"\n' > "$RUN/home/config.toml"
cat > "$RUN/home/channels/h6matrix.toml" <<EOF
name = "h6matrix"
platform = "matrix"
enabled = true

[options]
homeserver_url = "$URL"
credential_handle_access_token = "matrix.h6.access_token"
user_id = "@h6bot:f24.invalid"

[inbound]
dm = "allowlist"
dm_allowlist = ["@h6allowed:f24.invalid"]
group = "disabled"
require_mention = true
tools = "conversational"
EOF

syncs() { curl -s "$URL/__control/report" | python3 -c 'import json,sys; print(json.load(sys.stdin)["sync_total"])' 2>/dev/null || echo "?"; }
initials() { curl -s "$URL/__control/report" | python3 -c 'import json,sys; print(json.load(sys.stdin)["initial_sync_total"])' 2>/dev/null || echo "?"; }
cursor_file() { ls "$RUN"/home/channel-state/matrix-*.since 2>/dev/null | head -1; }
# Zombie-aware: `kill -0` succeeds for an unreaped corpse, which is the trap.
live() { local p="${1:-}"; [ -z "$p" ] && { echo "DEAD(nopid)"; return; }
  local st; st=$(awk '{print $3}' /proc/"$p"/stat 2>/dev/null)
  case "${st:-GONE}" in Z|X|GONE) echo "DEAD(${st:-gone})";; *) echo "LIVE($st)";; esac; }
naive_live() { local p="${1:-}"; [ -z "$p" ] && { echo "kill0-nopid"; return; }
  kill -0 "$p" 2>/dev/null && echo "kill0-says-ALIVE" || echo "kill0-says-dead"; }

BINPID=""
start_binary() {
  tail -f /dev/null | env WAYLAND_HOME="$RUN/home" RUST_LOG=wcore_channel_matrix=info \
    "$BIN" --json-stream > "$RUN/$1.log" 2>&1 &
  sleep 3
  BINPID=$(pgrep -n -f 'wayland-core --json-stream')
  echo "  started $1 pid=${BINPID:-NONE} state=$(live "${BINPID:-}")"
}
stop_binary() { local p="${1:-}"; [ -z "$p" ] && return
  kill -TERM "$p" 2>/dev/null
  for i in $(seq 1 15); do case "$(live "$p")" in DEAD*) break;; esac; sleep 1; done
  kill -KILL "$p" 2>/dev/null; pkill -f 'tail -f /dev/null' 2>/dev/null; sleep 1; }

echo "--- A: first start, no cursor ---"
start_binary core-a; PA="$BINPID"
for i in $(seq 1 30); do F=$(cursor_file); [ -n "$F" ] && break; echo "  A waiting: ${i} syncs=$(syncs) proc=$(live "$PA")"; sleep 2; done
F=$(cursor_file)
echo "A: cursor=[$(cat "$F" 2>/dev/null)] initials=$(initials) syncs=$(syncs) proc=$(live "$PA")"
SA1=$(syncs); sleep 10; SA2=$(syncs)
echo "A: KNOWN-POSITIVE CONTROL healthy-climb syncs $SA1 -> $SA2 over 10s proc=$(live "$PA") (want an increase)"
stop_binary "$PA"

echo "--- C: corrupt the cursor, restart ---"
printf '\x00not a cursor at all\n' > "$F"
echo "C: corrupted ($(wc -c < "$F") bytes of junk)"
IC=$(initials); SC=$(syncs)
start_binary core-c; PC="$BINPID"
for i in $(seq 1 15); do S=$(syncs); [ "$S" != "$SC" ] && break; echo "  C waiting: ${i} proc=$(live "$PC")"; sleep 2; done
sleep 3
echo "C: initials $IC -> $(initials)  syncs $SC -> $(syncs)  (want initials +1: it re-seeded)"
echo "C: cursor now = [$(cat "$F" 2>/dev/null)]  (want valid: the junk is replaced)"
S1=$(syncs); sleep 10; S2=$(syncs)
echo "C: NOT-WEDGED syncs $S1 -> $S2 over 10s proc=$(live "$PC") [v1's naive check said: $(naive_live "$PC")] (want an increase AND LIVE)"
stop_binary "$PC"

echo "=============== OPERATOR-VISIBLE LINES ==============="
for t in core-a core-c; do echo "--- $t.log ---"; grep -aE "INFO|WARN" "$RUN/$t.log" 2>/dev/null | grep -ai cursor | sed 's/\x1b\[[0-9;]*m//g'; done
kill $FIXPID 2>/dev/null; pkill -f 'tail -f /dev/null' 2>/dev/null
echo "H6-CORRUPT-PROBE3-DONE"
