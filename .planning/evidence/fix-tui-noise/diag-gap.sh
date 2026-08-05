#!/usr/bin/env bash
# diag-gap.sh — name the 9.8s boot stall. Runs the turn under strace with
# per-syscall timing and reports (a) the longest single syscall, (b) every
# syscall whose duration exceeds 0.5s, and (c) the wall-clock window between the
# two tool-registration passes so the syscalls can be located inside it.
#
# LANE-BRIEF: WLRC/WLDONE status; every number to a file; lane-unique paths.
set -u
LABEL="${1:?}"; BIN="${2:?}"; OUT="${3:?}"; LANEHOME="${4:?}"
mkdir -p "$OUT"; R="$OUT/$LABEL"; : > "$R.result"
say() { echo "$1" >> "$R.result"; }

KEY=$(/usr/bin/grep -m1 '^ANTHROPIC_API_KEY=' /root/.wayland/.env | cut -d= -f2-)
[ -z "$KEY" ] && { say "NO_KEY"; say "WLRC=97"; say "WLDONE"; exit 97; }
export ANTHROPIC_API_KEY="$KEY"; unset KEY
export HOME="$LANEHOME" PROVIDER=anthropic MODEL=claude-sonnet-4-5-20250929
export WAYLAND_VAULT_PASSPHRASE="uat-throwaway-not-a-real-secret"

say "LABEL=$LABEL"
say "BIN_SHA256=$(sha256sum "$BIN" | cut -d' ' -f1)"

# -T times each syscall, -tt timestamps it, -f follows threads. The trace is
# large; it stays on hetzner and only the derived numbers come back.
/usr/bin/strace -f -tt -T -o "$R.strace" \
  "$BIN" --no-tui 'What is 17 times 23? Reply with just the number.' \
  > "$R.stdout" 2> "$R.stderr"
RC=$?
say "PROC_RC=$RC"
say "STRACE_LINES=$(wc -l < "$R.strace" | tr -d ' ')"

# ── participant-alive: a trace of a process that never ran has no syscalls ───
if [ "$(wc -l < "$R.strace" | tr -d ' ')" -lt 100 ]; then
  say "ASSERT_TRACED=FAIL"; say "WLRC=95"; say "WLDONE"; exit 95
fi
say "ASSERT_TRACED=OK"
say "ANSWER_HAS_391=$(/usr/bin/grep -c 391 "$R.stdout" || true)"

# Every syscall slower than 0.5s, longest first. strace appends <duration> at
# end of line under -T.
/usr/bin/grep -oE '^[0-9]+ +[0-9:.]+ +[a-z_0-9]+\(.*<[0-9]+\.[0-9]+>$' "$R.strace" \
  | awk '{ n=split($0,a,"<"); d=a[n]; sub(/>$/,"",d); if (d+0 > 0.5) printf "%8.3f  %s\n", d, substr($0,1,160) }' \
  | sort -rn > "$R.slow.txt"
say "SLOW_SYSCALLS_OVER_0.5S=$(wc -l < "$R.slow.txt" | tr -d ' ')"

# Which syscall NAMES dominate the slow list.
awk '{ if (match($0, /[a-z_0-9]+\(/)) print substr($0, RSTART, RLENGTH-1) }' "$R.slow.txt" \
  | sort | uniq -c | sort -rn > "$R.slow.tally.txt"

# ── control pair (§3b-i) ─────────────────────────────────────────────────────
# POSITIVE: `execve` must appear in any real trace.
# NEGATIVE: a syscall that does not exist must not.
say "CTRL_POS_execve=$(/usr/bin/grep -c 'execve(' "$R.strace" || true)"
say "CTRL_NEG_bogus=$(/usr/bin/grep -c 'zzqq_not_a_syscall(' "$R.strace" || true)"

say "WLRC=0"
say "WLDONE"
exit 0
