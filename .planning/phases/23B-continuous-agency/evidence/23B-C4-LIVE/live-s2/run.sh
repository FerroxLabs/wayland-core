#!/bin/bash
# lane c4-live-cache, session 2. One scenario, one binary, one tag.
# Usage: run.sh <TAG> <BINARY>
set -u
TAG="$1"; BIN="$2"
BASE=/root/c4-live-s2
WS="$BASE/ws-$TAG"; HD="$BASE/home-$TAG"
rm -rf "$WS" "$HD"; mkdir -p "$WS" "$HD"
printf 'ALPHA\n' > "$WS/one.txt"
printf 'OMEGA\n' > "$WS/two.txt"
cat > "$HD/config.toml" <<'TOML'
[default]
provider = "anthropic"
model = "claude-haiku-4-5"
max_tokens = 300
max_turns = 12

[providers.anthropic]

[session]
enabled = false

[tools]
auto_approve = true
allow_list = ["Read"]

[compact]
compaction = "safe"
enabled = true
context_window = 30000
output_reserve = 2000
autocompact_buffer = 20000
emergency_buffer = 1000
TOML
chmod 600 "$HD/config.toml"

# Credential: already present on this box at /root/.wayland/.env (mode 600).
# Sourced into this shell only; never echoed, never written anywhere.
set -a; . /root/.wayland/.env; set +a
if [ -z "${ANTHROPIC_API_KEY:-}" ]; then echo "KEY_PRESENT=no"; exit 9; fi
echo "KEY_PRESENT=yes LEN=${#ANTHROPIC_API_KEY}"
echo "BIN=$BIN"
echo "BIN_MTIME=$(stat -c %y "$BIN")"

PROMPT="$(cat /root/c4-live-s2/blob.txt)$(printf '\n\n---\nThe document above is background context only. Now do this: Read one.txt. Then read two.txt. The files may be edited between reads, so after you have read both, read one.txt again, and then read two.txt again, to confirm their current contents. Finally reply with the two file contents joined by a hyphen.')"

cd "$WS"
WAYLAND_HOME="$HD" "$BIN" "$PROMPT" > "$BASE/$TAG-session.out" 2> "$BASE/$TAG-session.err"
echo "SESSION_RC=$?"
WAYLAND_HOME="$HD" "$BIN" cache report > "$BASE/$TAG-report.txt" 2> "$BASE/$TAG-report.err"
echo "RC_REPORT=$?"
WAYLAND_HOME="$HD" "$BIN" cache show > "$BASE/$TAG-show.txt" 2> "$BASE/$TAG-show.err"
echo "RC_SHOW=$?"
WAYLAND_HOME="$HD" "$BIN" cache verify > "$BASE/$TAG-verify.txt" 2> "$BASE/$TAG-verify.err"
echo "RC_VERIFY=$?"
echo "DONE_$TAG"
