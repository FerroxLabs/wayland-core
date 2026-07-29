#!/bin/bash
# lane/cost-provider, C4-F3 live A/B. Usage: run.sh <TAG> <BINARY>
# Local ollama model only -- this leg spends NOTHING, so the revert arm is free.
set -u
TAG="$1"; BIN="$2"
BASE=/root/lane-cost-provider-live
WS="$BASE/ws-$TAG"; HD="$BASE/home-$TAG"
rm -rf "$WS" "$HD"; mkdir -p "$WS" "$HD"
cat > "$HD/config.toml" <<"TOML"
[default]
provider = "anthropic"
model = "ollama:smollm2:135m"
max_tokens = 64
max_turns = 1

[providers.anthropic]

[session]
enabled = false

[tools]
auto_approve = true
allow_list = []
TOML
chmod 600 "$HD/config.toml"
# The configured profile is deliberately anthropic: that is the value the
# ledger used to record. The route is chosen by the ollama: model prefix.
echo "BIN=$BIN"
echo "BIN_MTIME=$(stat -c %y "$BIN")"
echo "BIN_SHA=$(sha256sum "$BIN" | cut -c1-16)"
cd "$WS"
WAYLAND_HOME="$HD" "$BIN" "Reply with exactly the word: pong" > "$BASE/$TAG-session.out" 2> "$BASE/$TAG-session.err"
echo "SESSION_RC=$?"
WAYLAND_HOME="$HD" "$BIN" cache report > "$BASE/$TAG-report.txt" 2> "$BASE/$TAG-report.err"
echo "RC_REPORT=$?"
WAYLAND_HOME="$HD" "$BIN" cache show > "$BASE/$TAG-show.txt" 2> "$BASE/$TAG-show.err"
echo "RC_SHOW=$?"
WAYLAND_HOME="$HD" "$BIN" cache verify > "$BASE/$TAG-verify.txt" 2> "$BASE/$TAG-verify.err"
echo "RC_VERIFY=$?"
echo "DONE_$TAG"
