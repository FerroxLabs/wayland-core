#!/usr/bin/env bash
# Phase 27 — put ONE bundle to all four panel members and capture each response
# verbatim beneath its bundle digest.
#
# Each member has a documented way of silently dropping its own vote:
#   - gemini refuses without --skip-trust and returns nothing,
#   - kimi needs its absolute path because this shell predates the PATH,
#   - codex repeats its final block, so any extraction must take the LAST match.
# All three are handled here rather than left to the reader.
#
#   f27-run-panel.sh <panel-dir>
set -u
P="${1:?panel dir}"
B=$(/usr/bin/shasum -a 256 "$P/PROMPT.md" | /usr/bin/awk '{print $1}')
Q=$(/bin/cat "$P/PROMPT.md")
echo "BUNDLE: $B"

echo "BUNDLE-SHA256: $B" > "$P/codex.txt"
codex exec -m gpt-5.6-sol --sandbox read-only --skip-git-repo-check "$Q" >> "$P/codex.txt" 2>&1
echo "codex rc=$? lines=$(wc -l < "$P/codex.txt")"

echo "BUNDLE-SHA256: $B" > "$P/gemini.txt"
gemini -p "$Q" -m gemini-3.1-pro-preview -o text --skip-trust >> "$P/gemini.txt" 2>&1
echo "gemini rc=$? lines=$(wc -l < "$P/gemini.txt")"

echo "BUNDLE-SHA256: $B" > "$P/kimi.txt"
/Users/seandonahoe/.kimi-code/bin/kimi -p "$Q" --output-format text >> "$P/kimi.txt" 2>&1
echo "kimi rc=$? lines=$(wc -l < "$P/kimi.txt")"
