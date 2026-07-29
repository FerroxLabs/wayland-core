#!/usr/bin/env bash
# THE COLD-START END-TO-END PRODUCT JOURNEY (steps 2-8).
#
# One WAYLAND_HOME, created empty here, carried forward across every step --
# because the point is a continuous day-one journey, not eight isolated probes.
#
# FLUX_API_KEY must already be exported by the caller (injected on stdin by the
# ssh wrapper; never in argv, never written to disk). Every capture in $OUT is
# redacted through `redact()` before it is read back or copied anywhere.
#
# GRADING. Each step prints exactly one `E2E_STEP=<id> RESULT=<PASS|FAIL> ...`
# line. A step NEVER aborts the run: a smoke test that stops at the first
# failure measures one thing, and the brief is explicit that the point is to
# measure all of them. Steps that genuinely cannot run print RESULT=NOT_REACHED
# with a reason.
#
# ANTI-SELF-PASS. Two rules are enforced structurally, not by intention:
#   1. Every refusal step is PAIRED with a permitted case that must SUCCEED in
#      the same run, same binary, same config. A sandbox that denies everything
#      fails the pair and therefore fails the step -- "universal-denial green"
#      cannot be reached from here.
#   2. Every capability step is graded on GROUND TRUTH the model cannot fake --
#      a file on disk, a nonce that exists only inside an out-of-process
#      server, a token that exists only inside a file the agent had to read.
#      The model's own narration ("I called the tool") is never sufficient.
set -uo pipefail

BIN="${BIN:?set BIN}"
OUT="${1:?usage: $0 <outdir>}"
MODEL="${MODEL:-flux-standard}"
: "${FLUX_API_KEY:?FLUX_API_KEY must be exported by the caller}"

mkdir -p "$OUT"
HOME_DIR="$OUT/home"          # the product's WAYLAND_HOME -- born empty here
WS="$OUT/ws"                  # the user's project directory
FAKEHOME="$OUT/fakehome"
mkdir -p "$HOME_DIR" "$WS" "$FAKEHOME"

RESULTS="$OUT/RESULTS.txt"; : > "$RESULTS"
say()  { echo "$*" | tee -a "$RESULTS"; }
step() { echo "E2E_STEP=$1 RESULT=$2 $3" | tee -a "$RESULTS"; }

redact() {
  sed -e "s|${FLUX_API_KEY}|<REDACTED_FLUX_KEY>|g" \
      -e 's/sk-[A-Za-z0-9_-]\{20,\}/<REDACTED_LONGSTRING>/g'
}

# A throwaway vault passphrase. NOT a secret: it protects a vault that holds
# only this run's throwaway state, and it is deliberately committed so the run
# is reproducible.
export WAYLAND_VAULT_PASSPHRASE="e2e-product-smoke-throwaway-not-a-secret"

cat > "$HOME_DIR/config.toml" <<'TOML'
[default]
provider = "flux-router"

[providers.flux-router]
base_url = "https://api.fluxrouter.ai/v1"
TOML
chmod 600 "$HOME_DIR/config.toml"

# ---------------------------------------------------------------------------
# run_agent <label> <prompt> [extra argv...]
# One non-interactive turn against the real provider, in $WS, with the shared
# WAYLAND_HOME. Captures are redacted immediately. Never inherits the caller's
# environment beyond what is listed.
# ---------------------------------------------------------------------------
run_agent() {
  local label="$1"; shift
  local prompt="$1"; shift
  local o="$OUT/$label.stdout" e="$OUT/$label.stderr"
  ( cd "$WS" && env -i \
      PATH=/usr/bin:/bin:/usr/local/bin \
      HOME="$FAKEHOME" \
      WAYLAND_HOME="$HOME_DIR" \
      WAYLAND_VAULT_PASSPHRASE="$WAYLAND_VAULT_PASSPHRASE" \
      FLUX_API_KEY="$FLUX_API_KEY" \
      TERM=dumb NO_COLOR=1 RUST_LOG="${RUST_LOG:-warn}" \
      timeout 300 "$BIN" -m "$MODEL" --force --no-tui "$@" "$prompt" \
      > "$o" 2> "$e" < /dev/null )
  local rc=$?
  redact < "$o" > "$o.r" && mv "$o.r" "$o"
  redact < "$e" > "$e.r" && mv "$e.r" "$e"
  AGENT_RC=$rc
  return $rc
}

# Same, but for non-agent subcommands (session/sandbox/...).
run_cmd() {
  local label="$1"; shift
  local o="$OUT/$label.stdout" e="$OUT/$label.stderr"
  ( cd "$WS" && env -i \
      PATH=/usr/bin:/bin:/usr/local/bin \
      HOME="$FAKEHOME" \
      WAYLAND_HOME="$HOME_DIR" \
      WAYLAND_VAULT_PASSPHRASE="$WAYLAND_VAULT_PASSPHRASE" \
      FLUX_API_KEY="$FLUX_API_KEY" \
      TERM=dumb NO_COLOR=1 RUST_LOG="${RUST_LOG:-warn}" \
      timeout 300 "$BIN" "$@" > "$o" 2> "$e" < /dev/null )
  local rc=$?
  redact < "$o" > "$o.r" && mv "$o.r" "$o"
  redact < "$e" > "$e.r" && mv "$e.r" "$e"
  CMD_RC=$rc
  return $rc
}

# grep a capture for a literal token; echo the count. Unproxied grep.
hits() { /usr/bin/grep -c -F -- "$2" "$1" 2>/dev/null || echo 0; }

say "########## E2E COLD-START PRODUCT JOURNEY ##########"
say "binary : $BIN"
say "version: $("$BIN" --version 2>&1 | head -1)"
say "source : $("$BIN" --build-info 2>&1 | head -1)"
say "model  : $MODEL"
say "home   : $HOME_DIR (created empty by this script)"
say "ws     : $WS"
say ""

# ===========================================================================
# STEP 2 -- provider configured, one real completed turn.
# Reach proof: the answer to 17*23 is 391. An unreached provider, a stubbed
# client, or a dead harness cannot produce that digit string; an empty stdout
# certainly cannot. This is the same discipline BL-23B-H1 lacked -- its
# harness pointed at 127.0.0.1:1 and folded non-reaching runs into "ok".
# ===========================================================================
say "### STEP 2 -- provider configuration and a real turn"
run_agent s2-turn "Reply with ONLY the digits of 17 multiplied by 23. No words, no punctuation."
S2_HIT=$(hits "$OUT/s2-turn.stdout" "391")
say "rc=$AGENT_RC stdout: $(head -c 200 "$OUT/s2-turn.stdout" | tr '\n' ' ')"
if [ "$AGENT_RC" = "0" ] && [ "$S2_HIT" -ge 1 ]; then
  step 2 PASS "provider_reached=yes arithmetic_correct=yes rc=0"
  PROVIDER_LIVE=1
else
  step 2 FAIL "rc=$AGENT_RC reach_token_391=$S2_HIT"
  say "--- s2 stderr tail ---"; tail -5 "$OUT/s2-turn.stderr" | sed 's/^/   | /'
  PROVIDER_LIVE=0
fi
say ""

# ===========================================================================
# STEP 3a -- tools doing real work.
# Every assertion is filesystem ground truth or a token that exists only
# inside a file the agent had to open.
# ===========================================================================
say "### STEP 3a -- Read / Write / Edit / Grep / Glob"
mkdir -p "$WS/haystack/sub"
printf 'the token in this file is NEEDLE_ALPHA_7731\n' > "$WS/haystack/a.txt"
printf 'status: PENDING\n' > "$WS/target.txt"
for i in $(seq 1 12); do printf 'filler line %s\n' "$i" > "$WS/haystack/f$i.log"; done
for i in $(seq 1 5); do printf 'filler\n' > "$WS/haystack/sub/g$i.log"; done
printf 'nothing special here\nNEEDLE_BETA_9902 lives here\n' > "$WS/haystack/sub/buried.txt"
LOG_COUNT=$(/usr/bin/find "$WS/haystack" -name '*.log' | /usr/bin/wc -l | tr -d ' ')
say "prepared: $LOG_COUNT .log files, 1 alpha token, 1 buried beta token"

if [ "$PROVIDER_LIVE" = "1" ]; then
  # -- Read
  run_agent s3-read "Read the file haystack/a.txt and reply with ONLY the token string it contains."
  R=$(hits "$OUT/s3-read.stdout" "NEEDLE_ALPHA_7731")
  [ "$R" -ge 1 ] && step 3a-read PASS "token_recovered_from_disk=yes" \
                 || step 3a-read FAIL "token_hits=$R rc=$AGENT_RC out=$(head -c 120 "$OUT/s3-read.stdout"|tr '\n' ' ')"

  # -- Write  (ground truth: the file must exist with the exact content)
  rm -f "$WS/e2e-write.txt"
  run_agent s3-write "Create a file named e2e-write.txt in the current directory whose entire contents are exactly: WRITTEN_BY_AGENT_4412"
  if [ -f "$WS/e2e-write.txt" ] && /usr/bin/grep -qF "WRITTEN_BY_AGENT_4412" "$WS/e2e-write.txt"; then
    step 3a-write PASS "file_on_disk=yes content_match=yes bytes=$(/usr/bin/wc -c < "$WS/e2e-write.txt"|tr -d ' ')"
  else
    step 3a-write FAIL "file_exists=$([ -f "$WS/e2e-write.txt" ] && echo yes || echo no) rc=$AGENT_RC"
  fi

  # -- Edit  (ground truth: the ORIGINAL word must be gone and the new one present)
  run_agent s3-edit "Edit the file target.txt, replacing the word PENDING with the word COMPLETE. Change nothing else."
  E_NEW=$(hits "$WS/target.txt" "COMPLETE"); E_OLD=$(hits "$WS/target.txt" "PENDING")
  if [ "$E_NEW" -ge 1 ] && [ "$E_OLD" = "0" ]; then
    step 3a-edit PASS "new_present=yes old_gone=yes content=$(cat "$WS/target.txt"|tr -d '\n')"
  else
    step 3a-edit FAIL "COMPLETE=$E_NEW PENDING=$E_OLD rc=$AGENT_RC"
  fi

  # -- Grep
  run_agent s3-grep "Search the haystack directory for the exact string NEEDLE_BETA_9902 and reply with ONLY the path of the file that contains it."
  G=$(hits "$OUT/s3-grep.stdout" "buried.txt")
  [ "$G" -ge 1 ] && step 3a-grep PASS "located_correct_file=yes" \
                 || step 3a-grep FAIL "buried.txt_hits=$G rc=$AGENT_RC out=$(head -c 160 "$OUT/s3-grep.stdout"|tr '\n' ' ')"

  # -- Glob
  run_agent s3-glob "Using the glob tool, count every file under the haystack directory whose name ends in .log, including subdirectories. Reply with ONLY the number."
  GL=$(hits "$OUT/s3-glob.stdout" "$LOG_COUNT")
  [ "$GL" -ge 1 ] && step 3a-glob PASS "count_correct=$LOG_COUNT" \
                  || step 3a-glob FAIL "expected=$LOG_COUNT out=$(head -c 120 "$OUT/s3-glob.stdout"|tr '\n' ' ')"
else
  step 3a-read NOT_REACHED "provider not live at step 2"
  step 3a-write NOT_REACHED "provider not live at step 2"
  step 3a-edit NOT_REACHED "provider not live at step 2"
  step 3a-grep NOT_REACHED "provider not live at step 2"
  step 3a-glob NOT_REACHED "provider not live at step 2"
fi
say ""

# ===========================================================================
# STEP 3b/3c -- Bash THROUGH THE SANDBOX, as a matched pair.
#
# `sandbox exec` dispatches through BashTool::execute_with_ctx -- the agent's
# own shell tool, the same function -- so what it demonstrates is transitive to
# what the agent does. The two arms run back to back, same binary, same
# workspace, same invocation shape:
#   PERMITTED : read a file INSIDE the workspace          -> must SUCCEED
#   REFUSED   : read /etc/shadow, outside the workspace   -> must be REFUSED
# The step passes only if BOTH hold. A backend that denied everything would
# fail the permitted arm and take the whole step down with it.
# ===========================================================================
say "### STEP 3b/3c -- sandboxed shell, permitted and refused arms"
run_cmd s3-sbstatus sandbox status --json
say "sandbox status: $(cat "$OUT/s3-sbstatus.stdout" | tr -d '\n' | head -c 300)"

run_cmd s3-permit sandbox exec --workspace "$WS" "cat $WS/haystack/a.txt"
PERMIT_RC=$CMD_RC
PERMIT_HIT=$(hits "$OUT/s3-permit.stdout" "NEEDLE_ALPHA_7731")
say "PERMITTED arm: rc=$PERMIT_RC workspace_token_visible=$PERMIT_HIT"
say "   out: $(head -c 200 "$OUT/s3-permit.stdout" | tr '\n' ' ')"

run_cmd s3-deny sandbox exec --workspace "$WS" "cat /etc/shadow"
DENY_RC=$CMD_RC
# The shadow file's first field is a username; root is always present on this
# host. Its ABSENCE from the child's output is the refusal signal -- and the
# permitted arm above is the liveness control that makes that absence mean
# something.
DENY_LEAK=$(hits "$OUT/s3-deny.stdout" "root:")
say "REFUSED arm  : rc=$DENY_RC shadow_content_leaked=$DENY_LEAK"
say "   out: $(head -c 200 "$OUT/s3-deny.stdout" | tr '\n' ' ')"
say "   err: $(head -c 200 "$OUT/s3-deny.stderr" | tr '\n' ' ')"

# Control that the negative is not free: prove /etc/shadow IS readable to the
# caller outside the sandbox. If it were not, the refusal arm would pass for a
# reason having nothing to do with containment.
OUTSIDE_LEAK=$(/usr/bin/cat /etc/shadow 2>/dev/null | /usr/bin/grep -c "^root:")
say "CONTROL      : /etc/shadow readable OUTSIDE the sandbox = $OUTSIDE_LEAK (must be >=1 for the refusal to mean anything)"

if [ "$PERMIT_HIT" -ge 1 ] && [ "$DENY_LEAK" = "0" ] && [ "$OUTSIDE_LEAK" -ge 1 ]; then
  step 3bc PASS "permitted_succeeded=yes refused_blocked=yes outside_control=alive"
elif [ "$PERMIT_HIT" = "0" ]; then
  step 3bc FAIL "PERMITTED ARM FAILED -- refusal is universal-denial, not containment (permit_rc=$PERMIT_RC)"
elif [ "$OUTSIDE_LEAK" = "0" ]; then
  step 3bc FAIL "negative control dead: /etc/shadow unreadable outside the sandbox too"
else
  step 3bc FAIL "sandbox did NOT refuse: shadow content leaked into child output"
fi
say ""

# ===========================================================================
# STEP 4 -- a skill invoked and taking effect.
# Paired control: the identical prompt runs first with NO skill installed. The
# canary token must be ABSENT there and PRESENT once the skill exists. Without
# the control, "the token appeared" would also pass if the model had simply
# echoed something from the prompt.
# ===========================================================================
say "### STEP 4 -- skill invocation"
SKILL_PROMPT="Invoke the skill named e2e-canary and reply with ONLY the canary token it gives you."
if [ "$PROVIDER_LIVE" = "1" ]; then
  rm -rf "$WS/.wayland-core/skills"
  run_agent s4-control "$SKILL_PROMPT"
  S4_CTRL=$(hits "$OUT/s4-control.stdout" "SKILL_TOKEN_5583")

  mkdir -p "$WS/.wayland-core/skills/e2e-canary"
  cat > "$WS/.wayland-core/skills/e2e-canary/SKILL.md" <<'SKILL'
---
name: e2e-canary
description: Returns the end-to-end smoke-test canary token. Invoke when asked for the canary token.
---

The canary token is SKILL_TOKEN_5583. Report it verbatim and say nothing else.
SKILL
  run_agent s4-live "$SKILL_PROMPT"
  S4_LIVE=$(hits "$OUT/s4-live.stdout" "SKILL_TOKEN_5583")
  say "control (skill absent): token_hits=$S4_CTRL   live (skill present): token_hits=$S4_LIVE"
  say "   live out: $(head -c 200 "$OUT/s4-live.stdout" | tr '\n' ' ')"
  if [ "$S4_LIVE" -ge 1 ] && [ "$S4_CTRL" = "0" ]; then
    step 4 PASS "skill_took_effect=yes control_clean=yes"
  elif [ "$S4_CTRL" != "0" ]; then
    step 4 FAIL "control leaked the token -- the assertion proves nothing"
  else
    step 4 FAIL "skill did not take effect: token_hits=$S4_LIVE rc=$AGENT_RC"
  fi
else
  step 4 NOT_REACHED "provider not live at step 2"
fi
say ""

# ===========================================================================
# STEP 5 -- memory persisting across a session boundary.
# Two separate PROCESSES, two separate sessions, one WAYLAND_HOME. The control
# is a third process on a DIFFERENT WAYLAND_HOME: it must NOT know the
# codeword. Without that arm, a model that guessed or a prompt that leaked
# would pass.
# ===========================================================================
say "### STEP 5 -- memory across a session boundary"
if [ "$PROVIDER_LIVE" = "1" ]; then
  run_agent s5-store "Remember this fact for later: my project's deploy codeword is ZEPHYR_TANGO_66. Store it in memory, then reply with the single word STORED."
  say "   store out: $(head -c 160 "$OUT/s5-store.stdout" | tr '\n' ' ')"
  run_agent s5-recall "What is my project's deploy codeword? Search your memory. Reply with ONLY the codeword."
  S5_RECALL=$(hits "$OUT/s5-recall.stdout" "ZEPHYR_TANGO_66")

  ALT="$OUT/home-alt"; mkdir -p "$ALT"; cp "$HOME_DIR/config.toml" "$ALT/config.toml"; chmod 600 "$ALT/config.toml"
  ( cd "$WS" && env -i PATH=/usr/bin:/bin:/usr/local/bin HOME="$FAKEHOME" WAYLAND_HOME="$ALT" \
      WAYLAND_VAULT_PASSPHRASE="$WAYLAND_VAULT_PASSPHRASE" FLUX_API_KEY="$FLUX_API_KEY" \
      TERM=dumb NO_COLOR=1 RUST_LOG=warn timeout 300 "$BIN" -m "$MODEL" --force --no-tui \
      "What is my project's deploy codeword? Search your memory. Reply with ONLY the codeword." \
      > "$OUT/s5-control.stdout" 2> "$OUT/s5-control.stderr" < /dev/null )
  redact < "$OUT/s5-control.stdout" > "$OUT/.t" && mv "$OUT/.t" "$OUT/s5-control.stdout"
  redact < "$OUT/s5-control.stderr" > "$OUT/.t" && mv "$OUT/.t" "$OUT/s5-control.stderr"
  S5_CTRL=$(hits "$OUT/s5-control.stdout" "ZEPHYR_TANGO_66")

  say "recall (same home): $S5_RECALL   control (fresh home): $S5_CTRL"
  say "   recall out: $(head -c 200 "$OUT/s5-recall.stdout" | tr '\n' ' ')"
  MEMDB=$(/usr/bin/find "$HOME_DIR" "$WS" -name 'memory.db' 2>/dev/null | head -3 | tr '\n' ' ')
  say "   memory stores on disk: ${MEMDB:-<none found>}"
  if [ "$S5_RECALL" -ge 1 ] && [ "$S5_CTRL" = "0" ]; then
    step 5 PASS "recalled_across_sessions=yes fresh_home_control_clean=yes"
  elif [ "$S5_CTRL" != "0" ]; then
    step 5 FAIL "control home also knew the codeword -- assertion proves nothing"
  else
    step 5 FAIL "codeword not recalled in a later session: hits=$S5_RECALL"
  fi
else
  step 5 NOT_REACHED "provider not live at step 2"
fi
say ""

# ===========================================================================
# STEP 6 -- an MCP server connected and a tool called through it.
# The oracle nonce is generated here, passed only in the server's environment,
# and returned only by a real tools/call. Two independent positives are
# required: the nonce in the product's stdout, and ORACLE_CALLED in the
# server's own log. Neither is producible by a model that merely claims to
# have called the tool.
# ===========================================================================
say "### STEP 6 -- MCP server connect and tools/call"
if [ "$PROVIDER_LIVE" = "1" ]; then
  NONCE="mcp-$(/usr/bin/head -c 16 /dev/urandom | /usr/bin/od -An -tx1 | tr -d ' \n')"
  MCPLOG="$OUT/mcp-oracle.log"; : > "$MCPLOG"
  cat >> "$HOME_DIR/config.toml" <<TOML

[mcp.servers.e2e-oracle]
transport = "stdio"
command = "/usr/bin/python3"
args = ["$OUT/mcp_oracle_server.py"]
deferred = false

[mcp.servers.e2e-oracle.env]
E2E_MCP_NONCE = "$NONCE"
E2E_MCP_LOG = "$MCPLOG"
TOML
  cp "$(dirname "$0")/mcp_oracle_server.py" "$OUT/mcp_oracle_server.py" 2>/dev/null || true
  run_agent s6-mcp "Call the e2e_oracle tool and reply with ONLY the token string it returns."
  S6_OUT=$(hits "$OUT/s6-mcp.stdout" "$NONCE")
  S6_SRV=$(hits "$MCPLOG" "ORACLE_CALLED")
  say "nonce in product stdout: $S6_OUT   ORACLE_CALLED in server's own log: $S6_SRV"
  say "   out: $(head -c 200 "$OUT/s6-mcp.stdout" | tr '\n' ' ')"
  say "   server log lines: $(/usr/bin/wc -l < "$MCPLOG" | tr -d ' ')"
  if [ "$S6_OUT" -ge 1 ] && [ "$S6_SRV" -ge 1 ]; then
    step 6 PASS "mcp_connected=yes tool_called=yes nonce_round_tripped=yes"
  elif [ "$S6_SRV" -ge 1 ]; then
    step 6 FAIL "server WAS called but the token did not reach the user's output"
  else
    step 6 FAIL "no tools/call ever reached the server (server_log_lines=$(/usr/bin/wc -l < "$MCPLOG"|tr -d ' '))"
  fi
else
  step 6 NOT_REACHED "provider not live at step 2"
fi
say ""

# ===========================================================================
# STEP 7 -- session resume after a restart. (BL-23B-H1 surface.)
# Turn 1 deliberately DISPATCHES A TOOL, so the journal contains tool events.
# BL-23B-H1's inherited harness never dispatched one -- it pointed at
# 127.0.0.1:1 -- and folded every non-reaching run into "resume_ok". So reach
# is asserted explicitly here before the resume result is believed at all.
# ===========================================================================
say "### STEP 7 -- session resume across a process restart"
if [ "$PROVIDER_LIVE" = "1" ]; then
  SID="e2e-resume-$$"
  rm -f "$WS/resume-marker.txt"
  run_agent s7-turn1 "Create a file called resume-marker.txt containing exactly RESUME_MARKER_8821, then remember that my lucky number is 4471. Reply with the single word DONE." --session-id "$SID"
  REACH=$([ -f "$WS/resume-marker.txt" ] && echo 1 || echo 0)
  say "REACH assertion -- turn 1 dispatched a real tool event: $REACH (0 would make every later number meaningless)"

  run_cmd s7-list session list
  run_cmd s7-show session show "$SID"
  say "   session list token lines: $(/usr/bin/grep -c 'F23_SESSION=' "$OUT/s7-list.stdout" 2>/dev/null || echo 0)"
  say "   session show rc=$CMD_RC"
  say "   show head: $(head -c 240 "$OUT/s7-show.stdout" | tr '\n' ' ')"
  say "   show err : $(head -c 240 "$OUT/s7-show.stderr" | tr '\n' ' ')"

  run_agent s7-resume "What is my lucky number? Reply with ONLY the number." --resume "$SID"
  S7_RESUME_RC=$AGENT_RC
  S7_HIT=$(hits "$OUT/s7-resume.stdout" "4471")
  say "   resume rc=$S7_RESUME_RC lucky_number_recovered=$S7_HIT"
  say "   resume out: $(head -c 200 "$OUT/s7-resume.stdout" | tr '\n' ' ')"
  say "   resume err: $(tail -3 "$OUT/s7-resume.stderr" | tr '\n' ' ' | head -c 300)"

  if [ "$REACH" = "0" ]; then
    step 7 FAIL "turn 1 never dispatched a tool -- journal has no tool events, result would be vacuous (BL-23B-H1 trap)"
  elif [ "$S7_RESUME_RC" = "0" ] && [ "$S7_HIT" -ge 1 ]; then
    step 7 PASS "reach=yes journal_read_back=ok conversation_recovered=yes"
  else
    step 7 FAIL "reach=yes but resume did not recover the conversation: rc=$S7_RESUME_RC hits=$S7_HIT"
  fi
else
  step 7 NOT_REACHED "provider not live at step 2"
fi
say ""

# ===========================================================================
# STEP 8 -- clean exit and crash exit.
# Two things are checked after each: (a) no orphaned descendants, (b) the
# product can still do the SAME work afterwards. (b) is the check that matters
# to a user -- a lock file left behind is only interesting if it wedges the
# next run, and a lock file cleaned up is only proven by the next run working.
# ===========================================================================
say "### STEP 8 -- clean exit and crash exit"
lockfiles() { /usr/bin/find "$HOME_DIR" -name '*.lock' -o -name '*.lease' 2>/dev/null | sed "s|$HOME_DIR|\$HOME|" | sort; }

say "-- 8a clean exit --"
BEFORE=$(/usr/bin/pgrep -c -f 'wayland-e2e/target/release/wayland-core' 2>/dev/null || echo 0)
run_agent s8-clean "Reply with the single word CLEAN."
CLEAN_RC=$AGENT_RC
sleep 2
AFTER=$(/usr/bin/pgrep -c -f 'wayland-e2e/target/release/wayland-core' 2>/dev/null || echo 0)
say "wayland-core processes before=$BEFORE after=$AFTER (this lane's binary path only -- never a global pkill)"
say "locks after clean exit: $(lockfiles | tr '\n' ' ')"
if [ "$CLEAN_RC" = "0" ] && [ "$AFTER" -le "$BEFORE" ]; then
  step 8a PASS "rc=0 orphans=0 procs_before=$BEFORE procs_after=$AFTER"
else
  step 8a FAIL "rc=$CLEAN_RC procs_before=$BEFORE procs_after=$AFTER"
fi

say "-- 8b crash exit (SIGKILL mid-turn) --"
( cd "$WS" && env -i PATH=/usr/bin:/bin:/usr/local/bin HOME="$FAKEHOME" WAYLAND_HOME="$HOME_DIR" \
    WAYLAND_VAULT_PASSPHRASE="$WAYLAND_VAULT_PASSPHRASE" FLUX_API_KEY="$FLUX_API_KEY" \
    TERM=dumb NO_COLOR=1 RUST_LOG=warn "$BIN" -m "$MODEL" --force --no-tui --session-id "e2e-crash-$$" \
    "Count slowly from 1 to 40, one number per line, with a short sentence about each number." \
    > "$OUT/s8-crash.stdout" 2> "$OUT/s8-crash.stderr" < /dev/null ) &
VICTIM=$!
sleep 12
DESC_BEFORE=$(/usr/bin/pgrep -P "$VICTIM" 2>/dev/null | tr '\n' ' ')
say "victim pid=$VICTIM direct children before kill: ${DESC_BEFORE:-<none>}"
/bin/kill -9 "$VICTIM" 2>/dev/null
wait "$VICTIM" 2>/dev/null
sleep 3
ORPHANS=$(/usr/bin/pgrep -c -f 'wayland-e2e/target/release/wayland-core' 2>/dev/null || echo 0)
say "wayland-core processes 3s after SIGKILL: $ORPHANS (baseline before this step was $BEFORE)"
say "locks after crash: $(lockfiles | tr '\n' ' ')"

# The operative question: can the product still work?
run_agent s8-after-crash "Reply with the single word RECOVERED."
POST_RC=$AGENT_RC
POST_HIT=$(hits "$OUT/s8-after-crash.stdout" "RECOVERED")
say "post-crash run: rc=$POST_RC recovered_token=$POST_HIT"
say "   out: $(head -c 200 "$OUT/s8-after-crash.stdout" | tr '\n' ' ')"
say "   err: $(tail -3 "$OUT/s8-after-crash.stderr" | tr '\n' ' ' | head -c 300)"
if [ "$ORPHANS" -le "$BEFORE" ] && [ "$POST_RC" = "0" ] && [ "$POST_HIT" -ge 1 ]; then
  step 8b PASS "no_orphans=yes product_usable_after_crash=yes"
elif [ "$POST_RC" != "0" ]; then
  step 8b FAIL "product WEDGED after crash exit: rc=$POST_RC (locks: $(lockfiles | tr '\n' ' '))"
else
  step 8b FAIL "orphaned processes after SIGKILL: $ORPHANS (baseline $BEFORE)"
fi
say ""

say "########## SUMMARY ##########"
/usr/bin/grep '^E2E_STEP=' "$RESULTS"
say ""
say "captures in $OUT (all redacted)"
