#!/usr/bin/env bash
# PROBE 3 -- two questions run 2 left genuinely open, each with the instrument
# defect that produced the ambiguity repaired first.
#
# ---------------------------------------------------------------------------
# INSTRUMENT DEFECT #2, found in run 2 and repaired here.
# ---------------------------------------------------------------------------
# Run 2 detected the sandboxed shell child with `pgrep -f "sleep 240"`. But the
# string `sleep 240` is INSIDE THE PROMPT, so it appears in the command line of
# the harness subshell AND of the wayland-core process itself. `pgrep -f`
# matched those. Proof: after the run, the only surviving match on the whole
# host was the ssh command line of the query that went looking for them.
#
# So run 2's `CHILD_ALIVE=1` was a false positive and `orphans=6` counted
# command lines, not orphans. Step 8b's verdict was therefore NOT a measurement
# in either direction, and it is withdrawn rather than reported.
#
# The repair does not grep by name at all. It records the victim's real
# descendant PIDs from /proc BEFORE the kill, then asks about those exact PIDs
# afterwards, including whether they were reparented to init (PPid 1) -- which
# is what "orphaned" actually means. A pattern cannot self-match a PID.
#
# Self-test, three assertions, per LANE-BRIEF 6b-ii:
#   (1) known-positive : a real descendant we spawn IS found;
#   (2) known-negative : after it exits it is NOT found;
#   (3) the old matcher would have missed it: `pgrep -f` on the marker also
#       matches this script's own command line, so it reports a live child
#       when there is none.
set -uo pipefail
BIN="${BIN:?}"; OUT="${1:?usage: $0 <outdir>}"
: "${FLUX_API_KEY:?}"
MODEL="${MODEL:-flux-standard}"
mkdir -p "$OUT"
R="$OUT/RESULTS.txt"; : > "$R"
say() { echo "$*" | tee -a "$R"; }
step(){ echo "E2E_STEP=$1 RESULT=$2 $3" | tee -a "$R"; }
hits(){ local n; n=$(/usr/bin/grep -c -F -- "$2" "$1" 2>/dev/null); echo "${n:-0}"; }

# ---- the repaired descendant tracker -------------------------------------
descendants() {  # $1 = root pid -> prints every live descendant pid
  /usr/bin/python3 - "$1" <<'PY'
import os, sys
root = int(sys.argv[1])
kids = {}
for p in os.listdir('/proc'):
    if not p.isdigit(): continue
    try:
        with open(f'/proc/{p}/status') as fh:
            ppid = next(int(l.split()[1]) for l in fh if l.startswith('PPid:'))
    except Exception:
        continue
    kids.setdefault(ppid, []).append(int(p))
seen, stack = [], [root]
while stack:
    cur = stack.pop()
    for k in kids.get(cur, []):
        if k not in seen:
            seen.append(k); stack.append(k)
print(' '.join(str(x) for x in seen))
PY
}
alive_with_ppid() {  # $1 = pid -> "<ppid>" if alive, "" if gone
  /usr/bin/awk '/^PPid:/{print $2}' "/proc/$1/status" 2>/dev/null
}

say "### instrument self-test for the descendant tracker"
MARKER="e2e-selftest-marker-$$"
sleep 9 & SELFKID=$!
sleep 1
FOUND=$(descendants $$ | tr ' ' '\n' | /usr/bin/grep -c "^${SELFKID}$")
[ "${FOUND:-0}" -ge 1 ] && say "  selftest 1/3 known-positive PASS (found real descendant $SELFKID)" \
                        || { say "  selftest 1/3 FAIL"; exit 3; }
/bin/kill -9 $SELFKID 2>/dev/null; wait $SELFKID 2>/dev/null; sleep 1
GONE=$(descendants $$ | tr ' ' '\n' | /usr/bin/grep -c "^${SELFKID}$")
[ "${GONE:-0}" = "0" ] && say "  selftest 2/3 known-negative PASS (reaped descendant not reported)" \
                       || { say "  selftest 2/3 FAIL (got $GONE)"; exit 3; }
# (3) the old approach self-matches: this script's own argv contains MARKER.
OLD=$(/usr/bin/pgrep -c -f "$MARKER" 2>/dev/null); OLD="${OLD:-0}"
if [ "$OLD" -ge 1 ]; then
  say "  selftest 3/3 old-matcher-was-broken PASS (pgrep -f found $OLD 'processes' for a marker no process runs)"
else
  say "  selftest 3/3 old-matcher-was-broken INCONCLUSIVE (pgrep -f returned $OLD here)"
fi
say ""

WS="$OUT/ws"; HOME_DIR="$OUT/home"; FAKEHOME="$OUT/fakehome"
mkdir -p "$WS" "$HOME_DIR" "$FAKEHOME"
export WAYLAND_VAULT_PASSPHRASE="e2e-product-smoke-throwaway-not-a-secret"
cat > "$HOME_DIR/config.toml" <<'TOML'
[default]
provider = "flux-router"

[providers.flux-router]
base_url = "https://api.fluxrouter.ai/v1"
TOML
chmod 600 "$HOME_DIR/config.toml"
printf 'workspace token NEEDLE_ALPHA_7731\n' > "$WS/inside.txt"
printf 'OUTSIDE_SECRET_TOKEN_9931\n' > "$OUT/outside.txt"   # outside $WS, same host

redact(){ sed -e "s|${FLUX_API_KEY}|<REDACTED_FLUX_KEY>|g" -e 's/sk-[A-Za-z0-9_-]\{20,\}/<REDACTED_LONGSTRING>/g'; }
sbexec() {  # $1 label, $2 command
  local o="$OUT/$1.out" e="$OUT/$1.err"
  ( cd "$WS" && env -i PATH=/usr/bin:/bin:/usr/local/bin HOME="$FAKEHOME" WAYLAND_HOME="$HOME_DIR" \
      WAYLAND_VAULT_PASSPHRASE="$WAYLAND_VAULT_PASSPHRASE" FLUX_API_KEY="$FLUX_API_KEY" \
      TERM=dumb NO_COLOR=1 RUST_LOG=warn timeout 120 \
      "$BIN" sandbox exec --workspace "$WS" "$2" > "$o" 2> "$e" < /dev/null )
  SB_RC=$?
  redact < "$o" > "$o.t" && mv "$o.t" "$o"
}

# =========================================================================
# A -- HOW FAR DOES THE SANDBOX'S READ CONTAINMENT ACTUALLY REACH?
# `sandbox status` advertises enforces_read_deny=true. Run 2 found
# /etc/hostname readable from inside. The question that matters to a user is
# narrower and sharper: can a sandboxed shell command read THE PRODUCT'S OWN
# CREDENTIAL STORE, and arbitrary files elsewhere on the machine?
#
# Every arm below is paired with the same read performed OUTSIDE the sandbox,
# so a "blocked" result can never be an artifact of the file being absent or
# unreadable to begin with.
# =========================================================================
say "### A -- read containment reach (advertised enforces_read_deny=true)"
# Make a credential-store-shaped file in WAYLAND_HOME, which is outside $WS.
printf 'api_key = "sk-fake-CREDSTORE-CANARY-0000000000"\n' > "$HOME_DIR/credentials.toml"
chmod 600 "$HOME_DIR/credentials.toml"

probe() {  # $1 label, $2 command, $3 token expected on success, $4 outside-truth path
  sbexec "$1" "$2"
  local inside; inside=$(hits "$OUT/$1.out" "$3")
  local outside=0
  [ -n "${4:-}" ] && outside=$(/usr/bin/grep -c -F -- "$3" "$4" 2>/dev/null) && outside="${outside:-0}"
  say "  $1: rc=$SB_RC token_visible_INSIDE=$inside  control_readable_OUTSIDE=$outside"
  say "     -> $(head -c 150 "$OUT/$1.out" | tr '\n' ' ')"
  echo "$inside"
}

A_WS=$(probe a-inside-workspace   "cat $WS/inside.txt"            "NEEDLE_ALPHA_7731"        "$WS/inside.txt")
A_OUT=$(probe a-outside-file      "cat $OUT/outside.txt"          "OUTSIDE_SECRET_TOKEN_9931" "$OUT/outside.txt")
A_CRED=$(probe a-credential-store "cat $HOME_DIR/credentials.toml" "CREDSTORE-CANARY"         "$HOME_DIR/credentials.toml")
A_HOST=$(probe a-etc-hostname     "cat /etc/hostname"             "$(cat /etc/hostname|tr -d '\n')" "/etc/hostname")
# /etc/shadow, spelled so the credential-exfiltration DENYLIST pattern does not
# fire -- this separates "the denylist stopped it" from "containment stopped it".
A_SHADOW=$(probe a-etc-shadow-via-head "head -c 40 /etc/shadow"   "root:"                    "/etc/shadow")

say ""
say "  SUMMARY of read containment:"
say "    inside workspace ............ $A_WS   (must be 1, else every deny below is universal-denial)"
say "    arbitrary file outside ...... $A_OUT"
say "    product's credential store .. $A_CRED"
say "    /etc/hostname ............... $A_HOST"
say "    /etc/shadow (denylist-evaded) $A_SHADOW"
if [ "$A_WS" = "0" ]; then
  step A FAIL "permitted arm dead -- results below would be universal-denial green"
elif [ "$A_OUT" = "0" ] && [ "$A_CRED" = "0" ] && [ "$A_SHADOW" = "0" ]; then
  step A PASS "read containment holds for everything outside the workspace (etc_hostname=$A_HOST is the only leak and is a standard bwrap /etc bind)"
else
  step A FAIL "READ CONTAINMENT DOES NOT HOLD: outside_file=$A_OUT credential_store=$A_CRED etc_shadow=$A_SHADOW while enforces_read_deny=true is advertised"
fi
say ""

# =========================================================================
# B -- ORPHANS AFTER SIGKILL, measured by PID rather than by name.
# `owns_descendants_hard=true` is advertised by `sandbox status`.
# =========================================================================
say "### B -- orphaned descendants after SIGKILL (advertised owns_descendants_hard=true)"
CSID=$(/usr/bin/head -c 6 /dev/urandom | /usr/bin/od -An -tx1 | tr -d ' \n')
( cd "$WS" && env -i PATH=/usr/bin:/bin:/usr/local/bin HOME="$FAKEHOME" WAYLAND_HOME="$HOME_DIR" \
    WAYLAND_VAULT_PASSPHRASE="$WAYLAND_VAULT_PASSPHRASE" FLUX_API_KEY="$FLUX_API_KEY" \
    TERM=dumb NO_COLOR=1 RUST_LOG=warn "$BIN" -m "$MODEL" --force --no-tui --session-id "$CSID" \
    "Run this exact shell command using your shell tool and wait for it to finish: sleep 300" \
    > "$OUT/b-crash.out" 2> "$OUT/b-crash.err" < /dev/null ) &
VICTIM=$!
DESC=""
for i in $(seq 1 24); do
  sleep 5
  DESC=$(descendants "$VICTIM")
  N=$(echo "$DESC" | /usr/bin/wc -w | tr -d ' ')
  echo "  waiting for a real shell-tool descendant: iteration $i, live descendants=$N, $(date +%H:%M:%S)"
  # a descendant that is NOT the wayland-core process itself means a tool ran
  if [ "$N" -ge 3 ]; then break; fi
done
say "  victim=$VICTIM descendant pids before kill: [${DESC:-<none>}] count=$(echo "$DESC" | /usr/bin/wc -w | tr -d ' ')"
for p in $DESC; do say "     pid=$p cmd=$(tr '\0' ' ' < /proc/$p/cmdline 2>/dev/null | cut -c1-90)"; done

/bin/kill -9 "$VICTIM" 2>/dev/null; wait "$VICTIM" 2>/dev/null
sleep 6
SURV=""; REPARENTED=0
for p in $DESC; do
  pp=$(alive_with_ppid "$p")
  if [ -n "$pp" ]; then
    SURV="$SURV $p(ppid=$pp)"
    [ "$pp" = "1" ] && REPARENTED=$((REPARENTED+1))
  fi
done
NSURV=$(echo "$SURV" | /usr/bin/wc -w | tr -d ' ')
say "  6s after SIGKILL -- of the pids recorded before the kill, still alive: [${SURV:-<none>}]"
say "  survivors=$NSURV reparented_to_init=$REPARENTED"
if [ -z "$DESC" ]; then
  step B NOT_REACHED "no descendant was ever observed, so the orphan question is unanswered (not a pass)"
elif [ "$NSURV" = "0" ]; then
  step B PASS "every descendant recorded before the kill was gone 6s after it -- owns_descendants_hard holds"
else
  step B FAIL "ORPHANS: $NSURV descendants survived the parent's SIGKILL ($REPARENTED reparented to init) despite owns_descendants_hard=true"
fi
# never leave anything of ours running -- five other lanes share this host
for p in $DESC; do /bin/kill -9 "$p" 2>/dev/null; done
say ""

# =========================================================================
# C -- STEP 6 RETRY, with the step-4 skill REMOVED.
# Run 2's step 6 failed because the model invoked the still-installed
# `e2e-canary` skill instead of the MCP tool -- a routing confound in the
# probe, not a fact about MCP. Isolate it.
# =========================================================================
say "### C -- MCP tools/call with no competing skill installed"
rm -rf "$WS/.wayland-core/skills"
H=$(/usr/bin/head -c 8 /dev/urandom | /usr/bin/od -An -tx1 | tr -d ' \n')
NONCE="ORCL-${H:0:4}-${H:4:4}-${H:8:4}-${H:12:4}"
MCPLOG="$OUT/mcp.log"; : > "$MCPLOG"
cp "$(dirname "$0")/mcp_oracle_server2.py" "$OUT/mcp_oracle_server2.py"
cat >> "$HOME_DIR/config.toml" <<TOML

[mcp.servers.e2e-oracle]
transport = "stdio"
command = "/usr/bin/python3"
args = ["$OUT/mcp_oracle_server2.py"]
deferred = false

[mcp.servers.e2e-oracle.env]
E2E_MCP_NONCE = "$NONCE"
E2E_MCP_LOG = "$MCPLOG"
TOML
( cd "$WS" && env -i PATH=/usr/bin:/bin:/usr/local/bin HOME="$FAKEHOME" WAYLAND_HOME="$HOME_DIR" \
    WAYLAND_VAULT_PASSPHRASE="$WAYLAND_VAULT_PASSPHRASE" FLUX_API_KEY="$FLUX_API_KEY" \
    TERM=dumb NO_COLOR=1 RUST_LOG=warn timeout 300 "$BIN" -m "$MODEL" --force --no-tui \
    "Call the e2e_oracle tool and reply with ONLY the oracle token string it returns." \
    > "$OUT/c-mcp.out" 2> "$OUT/c-mcp.err" < /dev/null )
C_RC=$?
redact < "$OUT/c-mcp.out" > "$OUT/.t" && mv "$OUT/.t" "$OUT/c-mcp.out"
C_TOK=$(hits "$OUT/c-mcp.out" "$NONCE")
C_SRV=$(hits "$MCPLOG" "ORACLE_CALLED")
C_CONN=$(hits "$OUT/c-mcp.err" "Connected to 'e2e-oracle'")
say "  rc=$C_RC connected=$C_CONN server_saw_call=$C_SRV token_in_user_output=$C_TOK"
say "  out: $(head -c 200 "$OUT/c-mcp.out" | tr '\n' ' ')"
if [ "$C_TOK" -ge 1 ] && [ "$C_SRV" -ge 1 ]; then
  step C PASS "mcp_connected=yes tools_call_reached_server=yes nonce_round_tripped_to_user=yes"
elif [ "$C_SRV" -ge 1 ]; then
  step C FAIL "server was called but the token never reached the user's output"
else
  step C FAIL "no tools/call reached the server (connected=$C_CONN)"
fi
say ""
say "########## PROBE 3 SUMMARY ##########"
/usr/bin/grep '^E2E_STEP=' "$R"
