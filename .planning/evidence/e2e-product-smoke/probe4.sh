#!/usr/bin/env bash
# PROBE 4 -- close the two things probe 3 left mis-measured.
#
# ---------------------------------------------------------------------------
# INSTRUMENT DEFECT #3 (probe 3, section A): a helper that both PRINTED its
# working and `echo`ed its result was called in `$(...)`, so the variable
# captured the whole narration instead of the number. The per-arm numbers it
# printed were correct and are unaffected; only the roll-up verdict line was
# garbage, and it graded FAIL on data that reads PASS. Repaired by separating
# measurement from narration.
#
# INSTRUMENT DEFECT #4 (probe 3, section B) -- the substantive one. The SIGKILL
# was delivered to the harness's WRAPPER SUBSHELL, not to wayland-core. Every
# "orphan" it then reported is just POSIX: killing a shell does not kill its
# children, and a reparent to init is exactly what must happen. So probe 3's
# section B did not test the product at all, and its FAIL is WITHDRAWN rather
# than reported. Probe 4 execs the binary so the backgrounded PID *is*
# wayland-core, verifies that from /proc/<pid>/cmdline BEFORE killing, and only
# then asks the orphan question.
#
# Self-test, three assertions:
#   (1) known-positive: the recorded victim pid's cmdline really is the product;
#   (2) known-negative: a descendant we reap ourselves is not reported alive;
#   (3) the OLD approach (kill the wrapper) would have reported orphans even
#       for a perfectly-behaved child -- demonstrated inline with /bin/sleep,
#       which owns nothing and cannot possibly leak.
set -uo pipefail
BIN="${BIN:?}"; OUT="${1:?usage: $0 <outdir>}"; : "${FLUX_API_KEY:?}"
MODEL="${MODEL:-flux-standard}"
mkdir -p "$OUT"; R="$OUT/RESULTS.txt"; : > "$R"
say(){ echo "$*" | tee -a "$R"; }
step(){ echo "E2E_STEP=$1 RESULT=$2 $3" | tee -a "$R"; }
hits(){ local n; n=$(/usr/bin/grep -c -F -- "$2" "$1" 2>/dev/null); echo "${n:-0}"; }

descendants(){ /usr/bin/python3 - "$1" <<'PY'
import os,sys
root=int(sys.argv[1]); kids={}
for p in os.listdir('/proc'):
    if not p.isdigit(): continue
    try:
        with open(f'/proc/{p}/status') as fh:
            ppid=next(int(l.split()[1]) for l in fh if l.startswith('PPid:'))
    except Exception: continue
    kids.setdefault(ppid,[]).append(int(p))
seen,stack=[],[root]
while stack:
    c=stack.pop()
    for k in kids.get(c,[]):
        if k not in seen: seen.append(k); stack.append(k)
print(' '.join(map(str,seen)))
PY
}
ppid_of(){ /usr/bin/awk '/^PPid:/{print $2}' "/proc/$1/status" 2>/dev/null; }
cmd_of(){ tr '\0' ' ' < "/proc/$1/cmdline" 2>/dev/null; }

# ---------------- self-test -------------------------------------------------
say "### self-test (3 assertions)"
# (3) first: prove the OLD method manufactures orphans from a clean child.
( exec /bin/sleep 8 ) & OKID=$!
( /bin/sleep 8 ) &      WRAP=$!     # wrapper subshell, the probe-3 shape
sleep 1
WRAPKIDS=$(descendants "$WRAP")
/bin/kill -9 "$WRAP" 2>/dev/null; wait "$WRAP" 2>/dev/null; sleep 1
FAKE_ORPH=0
for p in $WRAPKIDS; do [ -n "$(ppid_of "$p")" ] && FAKE_ORPH=$((FAKE_ORPH+1)); done
if [ "$FAKE_ORPH" -ge 1 ]; then
  say "  selftest 3/3 old-method-manufactures-orphans PASS (killing a WRAPPER left $FAKE_ORPH 'orphan(s)' from /bin/sleep, which leaks nothing)"
else
  say "  selftest 3/3 INCONCLUSIVE (wrapper had no separate child here)"
fi
for p in $WRAPKIDS; do /bin/kill -9 "$p" 2>/dev/null; done
# (1) known-positive: exec'd child's cmdline is what we think it is.
C=$(cmd_of "$OKID")
case "$C" in *sleep*) say "  selftest 1/3 known-positive PASS (exec'd pid $OKID cmdline is '$(echo "$C"|cut -c1-40)')";;
             *) say "  selftest 1/3 FAIL (cmdline '$C')"; exit 3;; esac
/bin/kill -9 "$OKID" 2>/dev/null; wait "$OKID" 2>/dev/null; sleep 1
# (2) known-negative
if [ -z "$(ppid_of "$OKID")" ]; then say "  selftest 2/3 known-negative PASS (reaped pid not reported alive)"
else say "  selftest 2/3 FAIL"; exit 3; fi
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
printf 'OUTSIDE_SECRET_TOKEN_9931\n' > "$OUT/outside.txt"
printf 'api_key = "sk-fake-CREDSTORE-CANARY-0000000000"\n' > "$HOME_DIR/credentials.toml"
chmod 600 "$HOME_DIR/credentials.toml"
redact(){ sed -e "s|${FLUX_API_KEY}|<REDACTED_FLUX_KEY>|g" -e 's/sk-[A-Za-z0-9_-]\{20,\}/<REDACTED_LONGSTRING>/g'; }

# ================== A (re-graded, narration separated from measurement) =====
say "### A -- read containment reach, re-graded"
arm(){ # $1 label $2 cmd $3 token ; sets ARM_IN / ARM_RC, prints nothing capturable
  local o="$OUT/$1.out"
  ( cd "$WS" && env -i PATH=/usr/bin:/bin:/usr/local/bin HOME="$FAKEHOME" WAYLAND_HOME="$HOME_DIR" \
      WAYLAND_VAULT_PASSPHRASE="$WAYLAND_VAULT_PASSPHRASE" FLUX_API_KEY="$FLUX_API_KEY" \
      TERM=dumb NO_COLOR=1 RUST_LOG=warn timeout 120 \
      "$BIN" sandbox exec --workspace "$WS" "$2" > "$o" 2> "$OUT/$1.err" < /dev/null )
  ARM_RC=$?
  redact < "$o" > "$o.t" && mv "$o.t" "$o"
  ARM_IN=$(hits "$o" "$3")
}
arm a-in   "cat $WS/inside.txt"              "NEEDLE_ALPHA_7731";        A_IN=$ARM_IN
say "  permitted  in-workspace read      : inside=$A_IN rc=$ARM_RC | $(head -c 90 "$OUT/a-in.out"|tr '\n' ' ')"
arm a-out  "cat $OUT/outside.txt"            "OUTSIDE_SECRET_TOKEN_9931"; A_OUT=$ARM_IN
say "  contained  arbitrary outside file : inside=$A_OUT rc=$ARM_RC | $(head -c 110 "$OUT/a-out.out"|tr '\n' ' ')"
arm a-cred "cat $HOME_DIR/credentials.toml"  "CREDSTORE-CANARY";          A_CRED=$ARM_IN
say "  contained  product credential store: inside=$A_CRED rc=$ARM_RC | $(head -c 110 "$OUT/a-cred.out"|tr '\n' ' ')"
arm a-host "cat /etc/hostname"               "$(cat /etc/hostname|tr -d '\n')"; A_HOST=$ARM_IN
say "  bwrap /etc bind  /etc/hostname    : inside=$A_HOST rc=$ARM_RC | $(head -c 90 "$OUT/a-host.out"|tr '\n' ' ')"
arm a-shad "head -c 40 /etc/shadow"          "root:";                     A_SHAD=$ARM_IN
say "  denylist   /etc/shadow            : inside=$A_SHAD rc=$ARM_RC | $(head -c 110 "$OUT/a-shad.out"|tr '\n' ' ')"
CTL_OUT=$(hits "$OUT/outside.txt" "OUTSIDE_SECRET_TOKEN_9931")
CTL_CRED=$(hits "$HOME_DIR/credentials.toml" "CREDSTORE-CANARY")
CTL_SHAD=$(/usr/bin/grep -c "^root:" /etc/shadow 2>/dev/null); CTL_SHAD=${CTL_SHAD:-0}
say "  LIVENESS CONTROLS outside the sandbox: outside_file=$CTL_OUT credential_store=$CTL_CRED etc_shadow=$CTL_SHAD (all must be >=1)"
if [ "$A_IN" = "0" ]; then step A FAIL "permitted arm dead -- any deny would be universal-denial green"
elif [ "$CTL_OUT" = "0" ] || [ "$CTL_CRED" = "0" ] || [ "$CTL_SHAD" = "0" ]; then step A FAIL "liveness controls dead"
elif [ "$A_OUT" = "0" ] && [ "$A_CRED" = "0" ] && [ "$A_SHAD" = "0" ]; then
  step A PASS "permitted=ok outside_file=BLOCKED credential_store=BLOCKED etc_shadow=BLOCKED (/etc/hostname=$A_HOST is the read-only /etc bind)"
else step A FAIL "leak: outside=$A_OUT cred=$A_CRED shadow=$A_SHAD"; fi
say ""

# ================== B (kill the PRODUCT, not a wrapper) =====================
say "### B -- orphans after SIGKILL DELIVERED TO wayland-core ITSELF"
CSID=$(/usr/bin/head -c 6 /dev/urandom | /usr/bin/od -An -tx1 | tr -d ' \n')
( cd "$WS" && exec env -i PATH=/usr/bin:/bin:/usr/local/bin HOME="$FAKEHOME" WAYLAND_HOME="$HOME_DIR" \
    WAYLAND_VAULT_PASSPHRASE="$WAYLAND_VAULT_PASSPHRASE" FLUX_API_KEY="$FLUX_API_KEY" \
    TERM=dumb NO_COLOR=1 RUST_LOG=warn "$BIN" -m "$MODEL" --force --no-tui --session-id "$CSID" \
    "Run this exact shell command using your shell tool and wait for it to finish: sleep 300" \
    > "$OUT/b.out" 2> "$OUT/b.err" < /dev/null ) &
VICTIM=$!
sleep 2
VCMD=$(cmd_of "$VICTIM")
say "  victim pid=$VICTIM cmdline=[$(echo "$VCMD" | cut -c1-95)]"
case "$VCMD" in
  *wayland-core*) say "  VERIFIED: the SIGKILL target IS the product, not a wrapper (this is the probe-3 defect closed)";;
  *) step B NOT_REACHED "could not exec the product directly; victim is [$VCMD] -- not measuring"; VICTIM="";;
esac

if [ -n "$VICTIM" ]; then
  DESC=""
  for i in $(seq 1 24); do
    sleep 5
    DESC=$(descendants "$VICTIM"); N=$(echo "$DESC" | /usr/bin/wc -w | tr -d ' ')
    echo "  waiting for a shell-tool descendant: iter $i live_descendants=$N $(date +%H:%M:%S)"
    [ "$N" -ge 2 ] && break
  done
  say "  descendants of the PRODUCT before kill: [$DESC]"
  for p in $DESC; do say "     pid=$p ppid=$(ppid_of $p) cmd=$(cmd_of $p | cut -c1-80)"; done
  SLEEP_PRESENT=0
  for p in $DESC; do case "$(cmd_of $p)" in *"sleep 300"*) SLEEP_PRESENT=1;; esac; done
  say "  a real sandboxed shell child (sleep 300) was alive at kill time: $SLEEP_PRESENT"

  /bin/kill -9 "$VICTIM" 2>/dev/null; wait "$VICTIM" 2>/dev/null
  sleep 8
  SURV=0; DETAIL=""
  for p in $DESC; do
    pp=$(ppid_of "$p")
    if [ -n "$pp" ]; then SURV=$((SURV+1)); DETAIL="$DETAIL $p(ppid=$pp,$(cmd_of $p|cut -c1-28))"; fi
  done
  say "  8s after SIGKILL to the product -- survivors=$SURV [$DETAIL]"
  if [ "$SLEEP_PRESENT" = "0" ]; then
    step B NOT_REACHED "no sandboxed shell child was alive at kill time -- orphan question unanswered, NOT a pass"
  elif [ "$SURV" = "0" ]; then
    step B PASS "product SIGKILLed with a live sandboxed child: every descendant gone within 8s (owns_descendants_hard holds)"
  else
    step B FAIL "ORPHANS: $SURV descendants outlived a SIGKILL delivered to the product itself"
  fi
  for p in $DESC; do /bin/kill -9 "$p" 2>/dev/null; done
fi
say ""

# ================== C -- does a crash leave the product usable? =============
say "### C -- is the product still usable after the crash, and what is left on disk?"
LOCKN=$(/usr/bin/find "$HOME_DIR" \( -name '*.lock' -o -name '*.lease' \) 2>/dev/null | /usr/bin/wc -l | tr -d ' ')
say "  lock/lease files left in WAYLAND_HOME after the crash: $LOCKN"
/usr/bin/find "$HOME_DIR" \( -name '*.lock' -o -name '*.lease' \) 2>/dev/null | sed "s|$HOME_DIR|\$HOME|" | head -8 | sed 's/^/     /' | tee -a "$R"
( cd "$WS" && env -i PATH=/usr/bin:/bin:/usr/local/bin HOME="$FAKEHOME" WAYLAND_HOME="$HOME_DIR" \
    WAYLAND_VAULT_PASSPHRASE="$WAYLAND_VAULT_PASSPHRASE" FLUX_API_KEY="$FLUX_API_KEY" \
    TERM=dumb NO_COLOR=1 RUST_LOG=warn timeout 200 "$BIN" -m "$MODEL" --force --no-tui \
    "Reply with the single word RECOVERED." > "$OUT/c.out" 2> "$OUT/c.err" < /dev/null )
CRC=$?
redact < "$OUT/c.out" > "$OUT/.t" && mv "$OUT/.t" "$OUT/c.out"
CH=$(hits "$OUT/c.out" "RECOVERED")
say "  post-crash run: rc=$CRC token=$CH out=[$(head -c 100 "$OUT/c.out"|tr '\n' ' ')]"
if [ "$CRC" = "0" ] && [ "$CH" -ge 1 ]; then step C PASS "product fully usable after a crash exit; $LOCKN lock files present but non-wedging"
else step C FAIL "product WEDGED after crash: rc=$CRC token=$CH"; fi
say ""
say "########## PROBE 4 SUMMARY ##########"
/usr/bin/grep '^E2E_STEP=' "$R"
