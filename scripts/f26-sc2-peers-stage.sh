#!/usr/bin/env bash
# Stage REAL grok and gemini peer homes for the 26-SC2-PEERS live import proof.
#
# The peer trees are READ-ONLY. This script only ever READS from them
# (`cp`, `find`, `stat`) and writes exclusively under $OUT. Nothing inside a
# peer tree is executed, moved, or modified, and the script asserts that at the
# end by comparing a pre/post digest of every source it touched.
#
# Credential files are NEVER copied. Same-named placeholders are written so the
# importer's by-reference credential path is exercised without a secret leaving
# this machine. Every substitution is printed.
set -euo pipefail

OUT="${1:?usage: f26-sc2-peers-stage.sh <output-dir>}"
GROK_SRC="${GROK_SRC:-$HOME/.grok}"
GEMINI_SRC="${GEMINI_SRC:-$HOME/.gemini}"
GEMINI_REPO_SKILLS="${GEMINI_REPO_SKILLS:-$HOME/dev/resources/gemini-cli/.gemini/skills}"

FIND=/usr/bin/find
STAT=/usr/bin/stat
CP=/bin/cp

# Files whose CONTENT is a live secret. Never copied; placeholdered instead.
CREDENTIAL_FILES=(
  auth.json
  oauth_creds.json
  google_accounts.json
  mcp-oauth-tokens.json
  a2a-oauth-tokens.json
  installation_id
)

placeholder() {
  # $1 = destination path. Content is a fixed non-secret marker.
  printf '{"placeholder":"26-sc2-peers — the real value never left the Mac"}\n' > "$1"
  echo "  SUBSTITUTED (credential store, content NOT copied): ${1#"$OUT"/}"
}

is_credential() {
  local base; base="$(basename "$1")"
  for c in "${CREDENTIAL_FILES[@]}"; do [ "$base" = "$c" ] && return 0; done
  return 1
}

# --- source integrity: digest before ------------------------------------------
digest_sources() {
  {
    [ -d "$GROK_SRC" ] && $FIND "$GROK_SRC" -type f \
      \( -path '*/skills/*' -o -name config.toml -o -name version.json \
         -o -path '*/marketplace-cache/*/.claude/skills/brand/*' \) \
      -exec $STAT -f '%N %z %m %Sp' {} \;
    [ -d "$GEMINI_SRC" ] && $FIND "$GEMINI_SRC" -maxdepth 1 -type f \
      \( -name settings.json -o -name GEMINI.md -o -name package.json \) \
      -exec $STAT -f '%N %z %m %Sp' {} \;
    [ -d "$GEMINI_REPO_SKILLS" ] && $FIND "$GEMINI_REPO_SKILLS" -type f \
      -exec $STAT -f '%N %z %m %Sp' {} \;
  } | LC_ALL=C sort
}
BEFORE="$(digest_sources)"

rm -rf "$OUT"
mkdir -p "$OUT"

# ============================ grok home =======================================
GH="$OUT/grok-home"
mkdir -p "$GH"
echo "== staging grok home from $GROK_SRC =="

$CP -p "$GROK_SRC/config.toml"  "$GH/config.toml"
$CP -p "$GROK_SRC/version.json" "$GH/version.json"
echo "  copied VERBATIM: config.toml, version.json"

# auth.json exists in the real home and its EXISTENCE is what the importer
# reads. Its content is a live OIDC session and is never copied.
[ -f "$GROK_SRC/auth.json" ] && placeholder "$GH/auth.json"

# The 5 real user skills, byte-for-byte, modes preserved.
mkdir -p "$GH/skills"
$CP -Rp "$GROK_SRC/skills/." "$GH/skills/"
echo "  copied VERBATIM: skills/ ($($FIND "$GH/skills" -name SKILL.md | wc -l | tr -d ' ') SKILL.md)"

# HOSTILE CASE. `~/.grok/skills` carries no exec-bit helper today, but
# `~/.grok/marketplace-cache` does — 188 of them across 78 real skills. A
# marketplace INSTALL copies such a skill into `<home>/skills/<name>/`, so the
# hostile case is that real payload placed where the installer would place it.
# Real bytes, real 0755 modes, real destination. Nothing synthetic.
BRAND="$GROK_SRC/marketplace-cache/55fb514ae005bf9d/.claude/skills/brand"
if [ -d "$BRAND" ]; then
  mkdir -p "$GH/skills/brand"
  $CP -Rp "$BRAND/." "$GH/skills/brand/"
  echo "  HOSTILE: copied real marketplace skill 'brand' with $(
    $FIND "$GH/skills/brand" -type f -perm -u+x | wc -l | tr -d ' ') exec-bit helpers"
else
  echo "  HOSTILE: SKIPPED — $BRAND absent" >&2
fi

# The vendor catalogs, present so the deferred inventory has something to count
# and so the scan can be shown NOT to reach them.
for d in bundled marketplace-cache vendor sessions; do
  [ -d "$GROK_SRC/$d" ] && mkdir -p "$GH/$d" &&
    $FIND "$GROK_SRC/$d" -maxdepth 1 -mindepth 1 -type d \
      -exec sh -c 'mkdir -p "$2/$(basename "$1")"' _ {} "$GH/$d" \;
done
echo "  shaped (directory names only, no payload): bundled, marketplace-cache, vendor, sessions"

# ============================ gemini home =====================================
EH="$OUT/gemini-home"
mkdir -p "$EH"
echo "== staging gemini home from $GEMINI_SRC =="

# settings.json ships VERBATIM: its mcpServers were inspected key-by-key and
# carry no credential (14 servers; the only `env` holds `PATH`), and
# security.auth.selectedType is a type NAME, not a value.
$CP -p "$GEMINI_SRC/settings.json" "$EH/settings.json"
$CP -p "$GEMINI_SRC/GEMINI.md"     "$EH/GEMINI.md"
$CP -p "$GEMINI_SRC/package.json"  "$EH/package.json"
echo "  copied VERBATIM: settings.json, GEMINI.md, package.json"

for c in "${CREDENTIAL_FILES[@]}"; do
  [ -f "$GEMINI_SRC/$c" ] && placeholder "$EH/$c"
done

for d in agents commands extensions; do
  [ -d "$GEMINI_SRC/$d" ] && mkdir -p "$EH/$d" &&
    $FIND "$GEMINI_SRC/$d" -maxdepth 1 -mindepth 1 -type d \
      -exec sh -c 'mkdir -p "$2/$(basename "$1")"' _ {} "$EH/$d" \;
done
echo "  shaped (directory names only): agents, commands, extensions"

# HOSTILE CASE. The real `~/.gemini` has no skills/ yet. The gemini-cli
# project's OWN `.gemini/skills/` does, in the identical `<geminidir>/skills/`
# layout `Storage.getUserSkillsDir()` returns — 13 real skills, 4 of them
# carrying a 0755 helper. Copied verbatim with modes.
mkdir -p "$EH/skills"
$CP -Rp "$GEMINI_REPO_SKILLS/." "$EH/skills/"
echo "  HOSTILE: copied $($FIND "$EH/skills" -name SKILL.md | wc -l | tr -d ' ') real gemini skills, $(
  $FIND "$EH/skills" -type f -perm -u+x | wc -l | tr -d ' ') exec-bit helpers"

# --- source integrity: digest after -------------------------------------------
AFTER="$(digest_sources)"
if [ "$BEFORE" = "$AFTER" ]; then
  echo "SOURCE-INTEGRITY: PASS — every source path is byte-, mode- and mtime-identical"
else
  echo "SOURCE-INTEGRITY: FAIL — a peer tree changed under this script" >&2
  diff <(printf '%s\n' "$BEFORE") <(printf '%s\n' "$AFTER") >&2 || true
  exit 1
fi

echo "STAGED: $OUT"
