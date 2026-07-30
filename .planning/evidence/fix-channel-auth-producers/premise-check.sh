#!/usr/bin/env bash
# Verify the credential premise BEFORE building any arm on it (LANE-BRIEF M6
# lesson: a lane once built quadrant 1 on a token it assumed was revoked and
# which actually worked). Secrets on stdin only; nothing is echoed.
set -uo pipefail

SLACK_BOT_TOKEN=""; DISCORD_BOT_TOKEN=""
while IFS= read -r line || [ -n "$line" ]; do
  key="${line%%=*}"; val="${line#*=}"
  val="${val%\"}"; val="${val#\"}"; val="${val%\'}"; val="${val#\'}"
  case "$key" in
    SLACK_BOT_TOKEN) SLACK_BOT_TOKEN="$val" ;;
    DISCORD_BOT_TOKEN) DISCORD_BOT_TOKEN="$val" ;;
  esac
done


. "$(dirname "${BASH_SOURCE[0]}")/bogus-credentials.sh"

echo "=== CREDENTIAL PREMISE CHECK (both directions, same session) ==="

if [ -n "$SLACK_BOT_TOKEN" ]; then
  echo "--- slack.com/api/auth.test ---"
  echo -n "no token at all        -> "
  curl -s -X POST https://slack.com/api/auth.test | head -c 200; echo
  echo -n "BOGUS token (arm 1/4)  -> "
  curl -s -X POST -H "Authorization: Bearer $BOGUS_SLACK" https://slack.com/api/auth.test | head -c 200; echo
  echo -n "REAL token (arm 3)     -> "
  curl -s -X POST -H "Authorization: Bearer $SLACK_BOT_TOKEN" https://slack.com/api/auth.test \
    | sed -E 's/"(user_id|team_id)":"[^"]*"/"\1":"<redacted>"/g' | head -c 250; echo
fi

if [ -n "$DISCORD_BOT_TOKEN" ]; then
  echo "--- discord.com/api/v10/users/@me ---"
  echo -n "no token at all        -> HTTP "
  curl -s -o /dev/null -w '%{http_code}' https://discord.com/api/v10/users/@me; echo
  echo -n "BOGUS token (arm 1/4)  -> HTTP "
  curl -s -o /tmp/lane-authprod-dbogus.json -w '%{http_code}' \
    -H "Authorization: Bot $BOGUS_DISCORD" https://discord.com/api/v10/users/@me; echo
  echo -n "   body: "; head -c 160 /tmp/lane-authprod-dbogus.json; echo
  echo -n "REAL token (arm 3)     -> HTTP "
  curl -s -o /tmp/lane-authprod-dreal.json -w '%{http_code}' \
    -H "Authorization: Bot $DISCORD_BOT_TOKEN" https://discord.com/api/v10/users/@me; echo
  echo -n "   bot username present: "
  grep -o '"username"' /tmp/lane-authprod-dreal.json | head -1 || echo "(none)"
  rm -f /tmp/lane-authprod-dbogus.json /tmp/lane-authprod-dreal.json
fi
