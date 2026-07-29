#!/usr/bin/env bash
# THE COLD-START JOURNEY, RUN 2 -- with the instrument repaired.
#
# ===========================================================================
# WHY THERE IS A RUN 2, STATED PLAINLY
# ===========================================================================
# Run 1's own harness was defective and it corrupted the grading in BOTH
# directions. The defect:
#
#     hits() { /usr/bin/grep -c -F -- "$2" "$1" 2>/dev/null || echo 0; }
#
# `grep -c` PRINTS "0" and EXITS 1 when it finds nothing. So the `|| echo 0`
# fired on top of the "0" grep had already printed, and the function returned
# the two-line string "0\n0". Every `[ "$x" = "0" ]` comparison against a
# true-negative then evaluated FALSE.
#
# Consequences in run 1, all of them wrong:
#   - step 3a-edit  reported FAIL while the file on disk was correct;
#   - step 4        reported "control leaked the token" while the control was clean;
#   - step 3bc      reported "sandbox did NOT refuse: shadow content leaked"
#                   while the command had in fact been REFUSED -- the gate
#                   printed the exact opposite of what happened;
#   - steps 8a/8b   reported FAIL on process counts that were 0 and 0.
#
# LANE-BRIEF §6b-ii: a written-up instrument defect is a defect you have agreed
# to keep, and the one recorded recurrence on this program happened precisely
# because a lane documented one instead of repairing it. So the instrument is
# repaired here, and the repair carries the mandated THREE assertions --
# known-positive passes, known-negative fails, AND the old broken matcher would
# have missed it. That third assertion is the only one that proves the repair
# does anything; without it the self-test passes on the broken instrument too.
#
# Substantive corrections carried into run 2, each with its reason:
#   3bc -- run 1's "refused" arm was `cat /etc/shadow`, which the CREDENTIAL-
#          EXFILTRATION DENYLIST refused by pattern before containment was ever
#          consulted. That is denylist theatre with respect to the property
#          under test. Run 2 tests containment with a path that is outside the
#          workspace but matches no denylist pattern, and tests the denylist
#          separately as its own (real, worth-having) defense.
#   5   -- run 1's control reused the same PROJECT directory, which carries its
#          own project-tier `memory.db`. The control was therefore not isolated
#          and could not have failed. Run 2 isolates home AND project.
#   6   -- run 1's oracle token was shaped `ORACLE_TOKEN=<hex>`, and the product
#          correctly scrubbed it as `[REDACTED:SECRET_ASSIGNMENT`. Good product
#          behaviour, bad probe. Run 2 uses a token with no `=` assignment.
#   7   -- run 1 used a session id the CLI rejects (`must be 6-40 hex
#          characters`). It rejected the id symmetrically at create AND resume,
#          which is correct behaviour; the probe was wrong. Run 2 uses hex.
#   8b  -- run 1 SIGKILLed the process while it was idle in an LLM call, so
#          there were no descendants to orphan and the test proved nothing.
#          Run 2 kills it while a shell-tool child is demonstrably alive.
set -uo pipefail

BIN="${BIN:?set BIN}"
OUT="${1:?usage: $0 <outdir>}"
MODEL="${MODEL:-flux-standard}"
: "${FLUX_API_KEY:?FLUX_API_KEY must be exported by the caller}"

mkdir -p "$OUT"
HOME_DIR="$OUT/home"; WS="$OUT/ws"; FAKEHOME="$OUT/fakehome"
mkdir -p "$HOME_DIR" "$WS" "$FAKEHOME"
RESULTS="$OUT/RESULTS.txt"; : > "$RESULTS"
say()  { echo "$*" | tee -a "$RESULTS"; }
step() { echo "E2E_STEP=$1 RESULT=$2 $3" | tee -a "$RESULTS"; }

redact() {
  sed -e "s|${FLUX_API_KEY}|<REDACTED_FLUX_KEY>|g" \
      -e 's/sk-[A-Za-z0-9_-]\{20,\}/<REDACTED_LONGSTRING>/g'
}

# ===========================================================================
# THE REPAIRED INSTRUMENT
# ===========================================================================
# `grep -c` already prints 0 on no-match; it exits 1, which must NOT be turned
# into a second line of output. The only real hazard is a MISSING FILE, where
# grep prints nothing at all -- handled by the ${n:-0} default, not by `||`.
hits() {
  local n
  n=$(/usr/bin/grep -c -F -- "$2" "$1" 2>/dev/null)
  echo "${n:-0}"
}
# Same shape for pgrep, which prints NOTHING and exits 1 on no-match.
procs() {
  local n
  n=$(/usr/bin/pgrep -c -f "$1" 2>/dev/null)
  echo "${n:-0}"
}
# The broken original, kept ONLY so the self-test can prove the repair matters.
hits_OLD() { /usr/bin/grep -c -F -- "$2" "$1" 2>/dev/null || echo 0; }

selftest() {
  local d; d=$(mktemp -d)
  printf 'alpha KNOWN_POSITIVE_TOKEN beta\n' > "$d/present.txt"
  printf 'nothing to see here\n'             > "$d/absent.txt"
  local fails=0

  # (1) known-positive: the instrument must SEE what is there.
  local p; p=$(hits "$d/present.txt" "KNOWN_POSITIVE_TOKEN")
  if [ "$p" = "1" ]; then say "  selftest 1/3 known-positive  PASS (got $p)"
  else say "  selftest 1/3 known-positive  FAIL (got '$p', want 1)"; fails=$((fails+1)); fi

  # (2) known-negative: the instrument must report exactly "0" -- a value that
  #     COMPARES EQUAL to "0", which is the whole point.
  local n; n=$(hits "$d/absent.txt" "KNOWN_POSITIVE_TOKEN")
  if [ "$n" = "0" ]; then say "  selftest 2/3 known-negative  PASS (got '$n', compares equal to 0)"
  else say "  selftest 2/3 known-negative  FAIL (got '$(echo "$n" | tr '\n' '/')', want 0)"; fails=$((fails+1)); fi

  # (3) THE ASSERTION THAT PROVES THE REPAIR DID SOMETHING. The old matcher
  #     must FAIL this same known-negative. If it passed, the repair is inert
  #     and this self-test would be green on a broken instrument.
  local o; o=$(hits_OLD "$d/absent.txt" "KNOWN_POSITIVE_TOKEN")
  if [ "$o" != "0" ]; then
    say "  selftest 3/3 old-matcher-was-broken PASS (old returned '$(echo "$o" | tr '\n' '/')' which != 0)"
  else
    say "  selftest 3/3 old-matcher-was-broken FAIL (old also returned 0 -- repair proves nothing)"
    fails=$((fails+1))
  fi
  rm -rf "$d"
  return $fails
}

say "########## E2E COLD-START PRODUCT JOURNEY -- RUN 2 (repaired instrument) ##########"
say "### instrument self-test (must pass before ANY product measurement is believed)"
if ! selftest; then
  say "INSTRUMENT SELF-TEST FAILED -- refusing to report product results from a dead instrument."
  exit 3
fi
say ""

export WAYLAND_VAULT_PASSPHRASE="e2e-product-smoke-throwaway-not-a-secret"
cat > "$HOME_DIR/config.toml" <<'TOML'
[default]
provider = "flux-router"

[providers.flux-router]
base_url = "https://api.fluxrouter.ai/v1"
TOML
chmod 600 "$HOME_DIR/config.toml"

run_agent() {
  local label="$1"; shift
  local prompt="$1"; shift
  local o="$OUT/$label.stdout" e="$OUT/$label.stderr"
  ( cd "$WS" && env -i PATH=/usr/bin:/bin:/usr/local/bin HOME="$FAKEHOME" \
      WAYLAND_HOME="$HOME_DIR" WAYLAND_VAULT_PASSPHRASE="$WAYLAND_VAULT_PASSPHRASE" \
      FLUX_API_KEY="$FLUX_API_KEY" TERM=dumb NO_COLOR=1 RUST_LOG="${RUST_LOG:-warn}" \
      timeout 300 "$BIN" -m "$MODEL" --force --no-tui "$@" "$prompt" \
      > "$o" 2> "$e" < /dev/null )
  AGENT_RC=$?
  redact < "$o" > "$o.r" && mv "$o.r" "$o"; redact < "$e" > "$e.r" && mv "$e.r" "$e"
  return $AGENT_RC
}
run_cmd() {
  local label="$1"; shift
  local o="$OUT/$label.stdout" e="$OUT/$label.stderr"
  ( cd "$WS" && env -i PATH=/usr/bin:/bin:/usr/local/bin HOME="$FAKEHOME" \
      WAYLAND_HOME="$HOME_DIR" WAYLAND_VAULT_PASSPHRASE="$WAYLAND_VAULT_PASSPHRASE" \
      FLUX_API_KEY="$FLUX_API_KEY" TERM=dumb NO_COLOR=1 RUST_LOG="${RUST_LOG:-warn}" \
      timeout 300 "$BIN" "$@" > "$o" 2> "$e" < /dev/null )
  CMD_RC=$?
  redact < "$o" > "$o.r" && mv "$o.r" "$o"; redact < "$e" > "$e.r" && mv "$e.r" "$e"
  return $CMD_RC
}

say "binary : $("$BIN" --build-info 2>&1 | head -1)"
say "model  : $MODEL"
say ""

# ===================================================== STEP 2
say "### STEP 2 -- provider configured, one real completed turn"
run_agent s2 "Reply with ONLY the digits of 17 multiplied by 23. No words, no punctuation."
S2=$(hits "$OUT/s2.stdout" "391")
say "rc=$AGENT_RC out=[$(head -c 120 "$OUT/s2.stdout" | tr '\n' ' ')] reach_token_391=$S2"
if [ "$AGENT_RC" = "0" ] && [ "$S2" -ge 1 ]; then step 2 PASS "provider_reached=yes rc=0"; LIVE=1
else step 2 FAIL "rc=$AGENT_RC reach=$S2"; LIVE=0; fi
say ""

# ===================================================== STEP 3a
say "### STEP 3a -- Read / Write / Edit / Grep / Glob (filesystem ground truth)"
mkdir -p "$WS/haystack/sub"
printf 'the token in this file is NEEDLE_ALPHA_7731\n' > "$WS/haystack/a.txt"
printf 'status: PENDING\n' > "$WS/target.txt"
for i in $(seq 1 12); do printf 'filler %s\n' "$i" > "$WS/haystack/f$i.log"; done
for i in $(seq 1 5);  do printf 'filler\n'       > "$WS/haystack/sub/g$i.log"; done
printf 'nothing special\nNEEDLE_BETA_9902 lives here\n' > "$WS/haystack/sub/buried.txt"
LOGN=$(/usr/bin/find "$WS/haystack" -name '*.log' | /usr/bin/wc -l | tr -d ' ')

if [ "$LIVE" = "1" ]; then
  run_agent s3read "Read the file haystack/a.txt and reply with ONLY the token string it contains."
  R=$(hits "$OUT/s3read.stdout" "NEEDLE_ALPHA_7731")
  [ "$R" -ge 1 ] && step 3a-read PASS "token_from_disk=yes" || step 3a-read FAIL "hits=$R"

  rm -f "$WS/e2e-write.txt"
  run_agent s3write "Create a file named e2e-write.txt in the current directory whose entire contents are exactly: WRITTEN_BY_AGENT_4412"
  if [ -f "$WS/e2e-write.txt" ] && /usr/bin/grep -qF "WRITTEN_BY_AGENT_4412" "$WS/e2e-write.txt"; then
    step 3a-write PASS "file_on_disk=yes bytes=$(/usr/bin/wc -c < "$WS/e2e-write.txt"|tr -d ' ')"
  else step 3a-write FAIL "no file / wrong content rc=$AGENT_RC"; fi

  run_agent s3edit "Edit the file target.txt, replacing the word PENDING with the word COMPLETE. Change nothing else."
  EN=$(hits "$WS/target.txt" "COMPLETE"); EO=$(hits "$WS/target.txt" "PENDING")
  say "  target.txt now: [$(cat "$WS/target.txt" | tr -d '\n')] COMPLETE=$EN PENDING=$EO"
  if [ "$EN" -ge 1 ] && [ "$EO" = "0" ]; then step 3a-edit PASS "new_present=yes old_gone=yes"
  else step 3a-edit FAIL "COMPLETE=$EN PENDING=$EO"; fi

  run_agent s3grep "Search the haystack directory for the exact string NEEDLE_BETA_9902 and reply with ONLY the path of the file that contains it."
  G=$(hits "$OUT/s3grep.stdout" "buried.txt")
  [ "$G" -ge 1 ] && step 3a-grep PASS "found_correct_file=yes" || step 3a-grep FAIL "hits=$G"

  run_agent s3glob "Using the glob tool, count every file under the haystack directory whose name ends in .log, including subdirectories. Reply with ONLY the number."
  GL=$(hits "$OUT/s3glob.stdout" "$LOGN")
  [ "$GL" -ge 1 ] && step 3a-glob PASS "count_correct=$LOGN" || step 3a-glob FAIL "expected=$LOGN out=[$(head -c 80 "$OUT/s3glob.stdout"|tr '\n' ' ')]"
else
  for s in read write edit grep glob; do step "3a-$s" NOT_REACHED "provider not live"; done
fi
say ""

# ===================================================== STEP 3b/3c
say "### STEP 3b/3c -- sandboxed shell: PERMITTED + REFUSED, as a matched pair"
run_cmd sbstatus sandbox status --json
say "backend: $(cat "$OUT/sbstatus.stdout" | tr -d '\n' | head -c 260)"

# -- ARM 1 (PERMITTED): inside the workspace. MUST SUCCEED.
run_cmd sb-permit sandbox exec --workspace "$WS" "cat $WS/haystack/a.txt"
P_RC=$CMD_RC; P_HIT=$(hits "$OUT/sb-permit.stdout" "NEEDLE_ALPHA_7731")
say "ARM permitted (in-workspace read) : rc=$P_RC token_visible=$P_HIT"
say "   $(head -c 160 "$OUT/sb-permit.stdout" | tr '\n' ' ')"

# -- ARM 2 (PERMITTED-WRITE): writing inside the workspace MUST SUCCEED, so a
#    backend that allowed reads but denied all writes cannot pass by halves.
run_cmd sb-permitw sandbox exec --workspace "$WS" "echo SANDBOX_WROTE_OK > $WS/sb-write.txt && cat $WS/sb-write.txt"
PW_RC=$CMD_RC; PW_HIT=$(hits "$OUT/sb-permitw.stdout" "SANDBOX_WROTE_OK")
say "ARM permitted (in-workspace write): rc=$PW_RC wrote_and_read_back=$PW_HIT"

# -- ARM 3 (CONTAINMENT REFUSAL): read OUTSIDE the workspace. `/etc/hostname`
#    is chosen because it matches NO credential-exfiltration denylist pattern,
#    so a refusal here is CONTAINMENT and not pattern matching. Run 1 used
#    /etc/shadow and got a denylist refusal instead -- theatre with respect to
#    the property under test.
HOSTNAME_TRUTH=$(/usr/bin/cat /etc/hostname 2>/dev/null | tr -d '\n')
run_cmd sb-outside sandbox exec --workspace "$WS" "cat /etc/hostname"
O_RC=$CMD_RC; O_LEAK=$(hits "$OUT/sb-outside.stdout" "$HOSTNAME_TRUTH")
say "ARM containment (out-of-workspace read of /etc/hostname): rc=$O_RC leaked=$O_LEAK"
say "   $(head -c 200 "$OUT/sb-outside.stdout" | tr '\n' ' ')"

# -- ARM 4 (CONTAINMENT REFUSAL, WRITE): write outside the workspace.
rm -f /tmp/E2E_ESCAPE_PROOF
run_cmd sb-outw sandbox exec --workspace "$WS" "echo escaped > /tmp/E2E_ESCAPE_PROOF"
OW_RC=$CMD_RC
OW_ESCAPED=$([ -f /tmp/E2E_ESCAPE_PROOF ] && echo 1 || echo 0)
say "ARM containment (out-of-workspace write to /tmp): rc=$OW_RC file_created_outside=$OW_ESCAPED"
rm -f /tmp/E2E_ESCAPE_PROOF

# -- ARM 5 (DENYLIST, a separate defense worth recording on its own):
run_cmd sb-denylist sandbox exec --workspace "$WS" "cat /etc/shadow"
D_RC=$CMD_RC; D_LEAK=$(hits "$OUT/sb-denylist.stdout" "root:")
D_REFUSED=$(hits "$OUT/sb-denylist.stdout" "Refused")
say "ARM denylist (cat /etc/shadow): rc=$D_RC refused_msg=$D_REFUSED shadow_leaked=$D_LEAK"

# -- LIVENESS CONTROLS: both negatives must be provably reachable outside.
CTRL_HOST=$([ -n "$HOSTNAME_TRUTH" ] && echo 1 || echo 0)
CTRL_SHADOW=$(/usr/bin/cat /etc/shadow 2>/dev/null | /usr/bin/grep -c "^root:")
CTRL_TMPW=$( (echo x > /tmp/E2E_CTRL_W 2>/dev/null && echo 1 && rm -f /tmp/E2E_CTRL_W) || echo 0)
say "CONTROLS outside the sandbox: /etc/hostname readable=$CTRL_HOST  /etc/shadow readable=$CTRL_SHADOW  /tmp writable=$CTRL_TMPW"
say "   (each negative above is only meaningful because the same thing SUCCEEDS outside)"

if [ "$P_HIT" = "0" ] || [ "$PW_HIT" = "0" ]; then
  step 3bc FAIL "PERMITTED ARM FAILED -- any refusal here would be universal-denial, not containment (read=$P_HIT write=$PW_HIT)"
elif [ "$CTRL_HOST" = "0" ] || [ "$CTRL_SHADOW" = "0" ] || [ "$CTRL_TMPW" = "0" ]; then
  step 3bc FAIL "negative controls dead outside the sandbox -- refusals would prove nothing"
elif [ "$O_LEAK" = "0" ] && [ "$OW_ESCAPED" = "0" ]; then
  step 3bc PASS "permitted_read=ok permitted_write=ok out_of_workspace_read=BLOCKED out_of_workspace_write=BLOCKED denylist_also_fires=$D_REFUSED"
else
  step 3bc FAIL "CONTAINMENT ESCAPE: out_of_workspace_read_leaked=$O_LEAK out_of_workspace_write_escaped=$OW_ESCAPED"
fi
say ""

# ===================================================== STEP 4
say "### STEP 4 -- skill invoked and taking effect (with absent-skill control)"
SP="Invoke the skill named e2e-canary and reply with ONLY the canary token it gives you."
if [ "$LIVE" = "1" ]; then
  rm -rf "$WS/.wayland-core/skills"
  run_agent s4ctl "$SP"; C4=$(hits "$OUT/s4ctl.stdout" "SKILL_TOKEN_5583")
  mkdir -p "$WS/.wayland-core/skills/e2e-canary"
  cat > "$WS/.wayland-core/skills/e2e-canary/SKILL.md" <<'SKILL'
---
name: e2e-canary
description: Returns the end-to-end smoke-test canary token. Invoke when asked for the canary token.
---

The canary token is SKILL_TOKEN_5583. Report it verbatim and say nothing else.
SKILL
  run_agent s4live "$SP"; L4=$(hits "$OUT/s4live.stdout" "SKILL_TOKEN_5583")
  say "control(skill absent)=$C4  live(skill present)=$L4"
  say "   live out: $(head -c 160 "$OUT/s4live.stdout" | tr '\n' ' ')"
  if [ "$L4" -ge 1 ] && [ "$C4" = "0" ]; then step 4 PASS "skill_effective=yes control_clean=yes"
  elif [ "$C4" != "0" ]; then step 4 FAIL "control leaked the token"
  else step 4 FAIL "skill had no effect: live_hits=$L4"; fi
else step 4 NOT_REACHED "provider not live"; fi
say ""

# ===================================================== STEP 5
say "### STEP 5 -- memory across a session boundary (control isolates home AND project)"
if [ "$LIVE" = "1" ]; then
  run_agent s5store "Remember this fact for later: my project's deploy codeword is ZEPHYR_TANGO_66. Store it in memory, then reply with the single word STORED."
  say "   store: $(head -c 120 "$OUT/s5store.stdout" | tr '\n' ' ')"
  run_agent s5recall "What is my project's deploy codeword? Search your memory. Reply with ONLY the codeword."
  R5=$(hits "$OUT/s5recall.stdout" "ZEPHYR_TANGO_66")

  # The control run 1 got wrong: a fresh WAYLAND_HOME is NOT enough, because a
  # project-tier memory.db lives under the PROJECT directory. Isolate both.
  ALT="$OUT/home-alt"; ALTWS="$OUT/ws-alt"; mkdir -p "$ALT" "$ALTWS"
  cp "$HOME_DIR/config.toml" "$ALT/config.toml"; chmod 600 "$ALT/config.toml"
  ( cd "$ALTWS" && env -i PATH=/usr/bin:/bin:/usr/local/bin HOME="$FAKEHOME" WAYLAND_HOME="$ALT" \
      WAYLAND_VAULT_PASSPHRASE="$WAYLAND_VAULT_PASSPHRASE" FLUX_API_KEY="$FLUX_API_KEY" \
      TERM=dumb NO_COLOR=1 RUST_LOG=warn timeout 300 "$BIN" -m "$MODEL" --force --no-tui \
      "What is my project's deploy codeword? Search your memory. Reply with ONLY the codeword." \
      > "$OUT/s5ctl.stdout" 2> "$OUT/s5ctl.stderr" < /dev/null )
  redact < "$OUT/s5ctl.stdout" > "$OUT/.t" && mv "$OUT/.t" "$OUT/s5ctl.stdout"
  C5=$(hits "$OUT/s5ctl.stdout" "ZEPHYR_TANGO_66")
  say "recall(same home+project)=$R5   control(fresh home AND fresh project)=$C5"
  say "   recall out : $(head -c 160 "$OUT/s5recall.stdout" | tr '\n' ' ')"
  say "   control out: $(head -c 160 "$OUT/s5ctl.stdout" | tr '\n' ' ')"
  say "   stores on disk: $(/usr/bin/find "$HOME_DIR" "$WS" -name 'memory.db' 2>/dev/null | tr '\n' ' ')"
  if [ "$R5" -ge 1 ] && [ "$C5" = "0" ]; then step 5 PASS "persisted_across_sessions=yes isolated_control_clean=yes"
  elif [ "$C5" != "0" ]; then step 5 FAIL "isolated control ALSO knew the codeword"
  else step 5 FAIL "not recalled in a later session: hits=$R5"; fi
else step 5 NOT_REACHED "provider not live"; fi
say ""

# ===================================================== STEP 6
say "### STEP 6 -- MCP connect + tools/call (token shaped to survive secret scrubbing)"
if [ "$LIVE" = "1" ]; then
  # Four hex groups, no `=` assignment: run 1's `ORACLE_TOKEN=<hex>` was
  # correctly scrubbed by the product as [REDACTED:SECRET_ASSIGNMENT.
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
  run_agent s6 "Call the e2e_oracle tool and reply with ONLY the oracle token string it returns."
  O6=$(hits "$OUT/s6.stdout" "$NONCE"); S6S=$(hits "$MCPLOG" "ORACLE_CALLED")
  CONN=$(hits "$OUT/s6.stderr" "Connected to 'e2e-oracle'")
  say "connect line in stderr=$CONN   ORACLE_CALLED in server's own log=$S6S   token in product stdout=$O6"
  say "   out: $(head -c 200 "$OUT/s6.stdout" | tr '\n' ' ')"
  if [ "$O6" -ge 1 ] && [ "$S6S" -ge 1 ]; then step 6 PASS "connected=yes tool_called=yes token_round_tripped=yes"
  elif [ "$S6S" -ge 1 ]; then step 6 FAIL "server WAS called but token never reached the user's output"
  else step 6 FAIL "no tools/call reached the server (connect=$CONN)"; fi
else step 6 NOT_REACHED "provider not live"; fi
say ""

# ===================================================== STEP 7
say "### STEP 7 -- session resume across a process restart (BL-23B-H1 surface)"
if [ "$LIVE" = "1" ]; then
  # Valid id: the CLI requires 6-40 HEX characters. Run 1 used a decimal PID
  # suffix and was correctly refused at BOTH create and resume.
  SID=$(/usr/bin/head -c 6 /dev/urandom | /usr/bin/od -An -tx1 | tr -d ' \n')
  say "session id: $SID (6-40 hex, as the CLI requires)"
  rm -f "$WS/resume-marker.txt"
  run_agent s7t1 "First create a file called resume-marker.txt containing exactly RESUME_MARKER_8821. Then remember that my lucky number is 4471. Reply with the single word DONE." --session-id "$SID"
  T1_RC=$AGENT_RC
  REACH=$([ -f "$WS/resume-marker.txt" ] && echo 1 || echo 0)
  say "  turn1 rc=$T1_RC  REACH (a real tool event was dispatched)=$REACH"
  say "  turn1 out: $(head -c 140 "$OUT/s7t1.stdout" | tr '\n' ' ')"

  run_cmd s7list session list
  run_cmd s7show session show "$SID"
  LISTED=$(hits "$OUT/s7list.stdout" "$SID")
  say "  session list: $(/usr/bin/grep -c 'F23_SESSION=' "$OUT/s7list.stdout") entries, this id listed=$LISTED"
  say "  session show rc=$CMD_RC head=[$(head -c 200 "$OUT/s7show.stdout" | tr '\n' ' ')]"
  S7SHOW_RC=$CMD_RC

  run_agent s7res "What is my lucky number? Reply with ONLY the number." --resume "$SID"
  RES_RC=$AGENT_RC; H7=$(hits "$OUT/s7res.stdout" "4471")
  say "  resume rc=$RES_RC recovered=$H7 out=[$(head -c 140 "$OUT/s7res.stdout" | tr '\n' ' ')]"
  say "  resume err: $(/usr/bin/grep -v INFO "$OUT/s7res.stderr" | tail -2 | tr '\n' ' ' | head -c 260)"

  if [ "$REACH" = "0" ]; then
    step 7 FAIL "turn 1 dispatched NO tool event -- journal has no tool records, any resume verdict would be vacuous (the exact BL-23B-H1 trap)"
  elif [ "$RES_RC" = "0" ] && [ "$H7" -ge 1 ]; then
    step 7 PASS "reach=yes journal_read_back=ok conversation_recovered=yes show_rc=$S7SHOW_RC listed=$LISTED"
  else
    step 7 FAIL "reach=yes but resume failed: rc=$RES_RC recovered=$H7 show_rc=$S7SHOW_RC"
  fi
else step 7 NOT_REACHED "provider not live"; fi
say ""

# ===================================================== STEP 8
say "### STEP 8 -- clean exit and crash exit"
PAT='wayland-e2e/target/release/wayland-core'
locks() { /usr/bin/find "$HOME_DIR" \( -name '*.lock' -o -name '*.lease' \) 2>/dev/null | /usr/bin/wc -l | tr -d ' '; }

say "-- 8a clean exit --"
B=$(procs "$PAT")
run_agent s8clean "Reply with the single word CLEAN."
CRC=$AGENT_RC; sleep 2
A=$(procs "$PAT"); LOCKS_A=$(locks)
CH=$(hits "$OUT/s8clean.stdout" "CLEAN")
say "  rc=$CRC token=$CH procs before=$B after=$A lockfiles=$LOCKS_A"
if [ "$CRC" = "0" ] && [ "$CH" -ge 1 ] && [ "$A" -le "$B" ]; then
  step 8a PASS "rc=0 no_surviving_process procs_before=$B procs_after=$A lockfiles_left=$LOCKS_A"
else step 8a FAIL "rc=$CRC token=$CH procs_before=$B procs_after=$A"; fi

say "-- 8b crash exit: SIGKILL WHILE A SHELL-TOOL CHILD IS ALIVE --"
# Run 1 killed the process while it idled in an LLM call, so there was nothing
# to orphan and the test proved nothing. Here the agent is told to run a long
# sleep through its shell tool; the kill lands while that child is running.
CSID=$(/usr/bin/head -c 6 /dev/urandom | /usr/bin/od -An -tx1 | tr -d ' \n')
MARK="e2e-orphan-canary-$$"
( cd "$WS" && env -i PATH=/usr/bin:/bin:/usr/local/bin HOME="$FAKEHOME" WAYLAND_HOME="$HOME_DIR" \
    WAYLAND_VAULT_PASSPHRASE="$WAYLAND_VAULT_PASSPHRASE" FLUX_API_KEY="$FLUX_API_KEY" \
    TERM=dumb NO_COLOR=1 RUST_LOG=warn "$BIN" -m "$MODEL" --force --no-tui --session-id "$CSID" \
    "Run this exact shell command and wait for it to finish: sleep 240 && echo $MARK" \
    > "$OUT/s8crash.stdout" 2> "$OUT/s8crash.stderr" < /dev/null ) &
VICTIM=$!
CHILD_ALIVE=0
for i in $(seq 1 20); do
  sleep 4
  if /usr/bin/pgrep -f "sleep 240" >/dev/null 2>&1; then CHILD_ALIVE=1; break; fi
  echo "  waiting for the shell-tool child, iteration $i, $(date +%H:%M:%S)"
done
SLEEPPIDS=$(/usr/bin/pgrep -f "sleep 240" 2>/dev/null | tr '\n' ' ')
say "  shell-tool child alive before kill=$CHILD_ALIVE (pids: ${SLEEPPIDS:-<none>})"
/bin/kill -9 "$VICTIM" 2>/dev/null; wait "$VICTIM" 2>/dev/null
sleep 5
ORPH_CORE=$(procs "$PAT")
ORPH_SLEEP=$(procs "sleep 240")
LOCKS_B=$(locks)
say "  5s after SIGKILL: wayland-core procs=$ORPH_CORE  orphaned 'sleep 240' children=$ORPH_SLEEP  lockfiles=$LOCKS_B"

run_agent s8post "Reply with the single word RECOVERED."
PRC=$AGENT_RC; PH=$(hits "$OUT/s8post.stdout" "RECOVERED")
say "  post-crash run: rc=$PRC token=$PH"
say "  post-crash err: $(/usr/bin/grep -v INFO "$OUT/s8post.stderr" | tail -1 | head -c 200)"

if [ "$CHILD_ALIVE" = "0" ]; then
  step 8b FAIL "could not establish a live shell-tool child before the kill -- orphan question NOT ANSWERED"
elif [ "$ORPH_SLEEP" != "0" ] || [ "$ORPH_CORE" -gt "$B" ]; then
  step 8b FAIL "ORPHANS SURVIVED SIGKILL: sleep_children=$ORPH_SLEEP core_procs=$ORPH_CORE (baseline $B)"
elif [ "$PRC" != "0" ] || [ "$PH" = "0" ]; then
  step 8b FAIL "no orphans, but the product is WEDGED after a crash: rc=$PRC token=$PH lockfiles=$LOCKS_B"
else
  step 8b PASS "orphans=0 (child was alive at kill) product_usable_after_crash=yes lockfiles_left=$LOCKS_B"
fi
# leave nothing of ours behind for the other five lanes
for p in $SLEEPPIDS; do /bin/kill -9 "$p" 2>/dev/null; done
say ""

say "########## SUMMARY ##########"
/usr/bin/grep '^E2E_STEP=' "$RESULTS"
