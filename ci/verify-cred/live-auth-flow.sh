#!/usr/bin/env bash
# VERIFY-CRED lane. Drives the REAL `wayland-core auth` product flow with a
# SYNTHETIC credential this script generates. Not for merge.
#
# Every token here is manufactured locally and is worthless. Nothing prints a
# credential value: the only thing echoed from a token is a plan-type nonce
# that this script puts there specifically so a round-trip can be observed
# without observing the secret.
#
# Legs:
#   A. keyring host   — login (import) / status / rotate / status / logout
#   B. isolated home  — the vault rung, WITH a passphrase
#   C. the cliff      — the same profile re-opened WITHOUT the passphrase
#   D. recovery       — the same profile re-opened WITH it again
#   E. no secure rung — a NEW login must be refused, not written in cleartext
set -uo pipefail

BIN=""
FAILURES=0
say() { printf '\n=== %s ===\n' "$*"; }
fail() { printf 'DEFECT: %s\n' "$*"; FAILURES=$((FAILURES + 1)); }
pass() { printf 'OK: %s\n' "$*"; }

say "build"
cargo build -p wcore-cli --bin wayland-core || exit 1
for candidate in target/debug/wayland-core target/debug/wayland-core.exe; do
  [ -x "$candidate" ] && BIN="$PWD/$candidate"
done
[ -n "$BIN" ] || { echo "could not find the wayland-core binary"; exit 1; }
echo "binary: $BIN"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# --- synthetic Codex login ------------------------------------------------
# `import_codex_cli_tokens` requires a JWT whose payload carries
# `https://api.openai.com/auth.chatgpt_account_id`. `chatgpt_plan_type` is the
# nonce we watch for: `auth status` prints it, so an intact round-trip of a
# ~4 KB token is observable without printing any of it.
make_auth_json() {
  local dir="$1" nonce="$2"
  mkdir -p "$dir"
  chmod 700 "$dir" 2>/dev/null || true
  python3 - "$dir/auth.json" "$nonce" <<'PY'
import base64, json, sys, time
path, nonce = sys.argv[1], sys.argv[2]
def seg(obj):
    return base64.urlsafe_b64encode(json.dumps(obj).encode()).decode().rstrip("=")
payload = {
    "https://api.openai.com/auth": {
        "chatgpt_account_id": "acct-" + nonce,
        "chatgpt_plan_type": nonce,
    },
    "exp": int(time.time()) + 3600,
    # ~2.5 KB of filler so the whole token set lands well over the Windows
    # 1280-unit single-entry ceiling and MUST span entries.
    "pad": nonce[0] * 2500,
}
access = seg({"alg": "none", "typ": "JWT"}) + "." + seg(payload) + ".sig"
doc = {"tokens": {
    "access_token": access,
    "refresh_token": "rt-" + nonce[0] * 1200,
    "id_token": access,
}}
open(path, "w").write(json.dumps(doc))
PY
  chmod 600 "$dir/auth.json" 2>/dev/null || true
}

run_status() { "$BIN" auth status 2>&1; }

# ==========================================================================
say "LEG A — real host credential store (no WAYLAND_HOME, keyring rung)"
unset WAYLAND_HOME WAYLAND_VAULT_PASSPHRASE 2>/dev/null || true
export CODEX_HOME="$WORK/codexA"
make_auth_json "$CODEX_HOME" "alpha1"

OUT="$("$BIN" auth login chatgpt --import-codex 2>&1)"; echo "$OUT"
if echo "$OUT" | grep -qi "imported chatgpt login"; then
  pass "A1 login stored a spanned token through the real credential store"

  OUT="$(run_status)"; echo "$OUT"
  echo "$OUT" | grep -q "alpha1" \
    && pass "A2 the token read back intact in a NEW process (plan nonce survived)" \
    || fail "A2 status did not return the stored plan nonce — the token did not read back"

  # rotate
  export CODEX_HOME="$WORK/codexB"; make_auth_json "$CODEX_HOME" "bravo2"
  "$BIN" auth login chatgpt --import-codex >/dev/null 2>&1
  OUT="$(run_status)"; echo "$OUT"
  if echo "$OUT" | grep -q "bravo2" && ! echo "$OUT" | grep -q "alpha1"; then
    pass "A3 rotation replaced the login cleanly (new nonce, no trace of the old)"
  else
    fail "A3 rotation did not cleanly replace the stored token"
  fi

  "$BIN" auth logout chatgpt 2>&1 | tail -1
  OUT="$(run_status)"; echo "$OUT"
  echo "$OUT" | grep -qi "not signed in" \
    && pass "A4 logout left an honest not-signed-in state" \
    || fail "A4 after logout, status is not 'not signed in'"
else
  echo "NOT MEASURED (leg A) — this host refused the login: no keyring rung."
fi

# ==========================================================================
say "LEG B — isolated profile with a vault passphrase"
export WAYLAND_HOME="$WORK/home1"
mkdir -p "$WAYLAND_HOME"
export WAYLAND_VAULT_PASSPHRASE="verify-cred-synthetic-passphrase"
export CODEX_HOME="$WORK/codexC"; make_auth_json "$CODEX_HOME" "charlie3"

OUT="$("$BIN" auth login chatgpt --import-codex 2>&1)"; echo "$OUT"
echo "$OUT" | grep -qi "imported chatgpt login" \
  && pass "B1 the vault rung accepted the login" \
  || fail "B1 an unlocked vault refused a login"

OUT="$(run_status)"; echo "$OUT"
echo "$OUT" | grep -q "charlie3" \
  && pass "B2 the vault returned the token intact" \
  || fail "B2 the unlocked vault did not return the stored token"

echo "-- cleartext audit of the profile home --"
if grep -rl "rt-cccc" "$WAYLAND_HOME" 2>/dev/null | head; then
  fail "B3 the refresh token is on disk in cleartext under WAYLAND_HOME"
else
  pass "B3 no cleartext refresh token anywhere under the profile home"
fi
ls -la "$WAYLAND_HOME" "$WAYLAND_HOME/oauth" 2>/dev/null

# ==========================================================================
say "LEG C — THE CLIFF: same profile, passphrase removed"
unset WAYLAND_VAULT_PASSPHRASE
OUT="$(run_status)"; echo "$OUT"
if echo "$OUT" | grep -qi "not signed in"; then
  fail "C1 a signed-in profile reports NOT SIGNED IN once the passphrase is gone \
— this is the silent sign-out the refusal was supposed to close"
elif echo "$OUT" | grep -qi "WAYLAND_VAULT_PASSPHRASE"; then
  pass "C1 the cliff refuses and names the passphrase remedy"
else
  fail "C1 the cliff produced neither a signed-out claim nor an actionable refusal"
fi
echo "$OUT" | grep -qi "delete" \
  && pass "C2 the refusal names the escape hatch (clear the record)" \
  || fail "C2 the refusal does not tell the user how to get unstuck"

say "LEG C2 — can a NEW login be made while stranded?"
export CODEX_HOME="$WORK/codexD"; make_auth_json "$CODEX_HOME" "delta4"
OUT="$("$BIN" auth login chatgpt --import-codex 2>&1)"; echo "$OUT"
echo "$OUT" | grep -qi "imported chatgpt login" \
  && echo "NOTE: a new login SUCCEEDS while stranded (no secure rung expected here)" \
  || echo "NOTE: a new login is refused while stranded (fail-closed)"

# ==========================================================================
say "LEG D — RECOVERY: the passphrase comes back"
export WAYLAND_VAULT_PASSPHRASE="verify-cred-synthetic-passphrase"
OUT="$(run_status)"; echo "$OUT"
if echo "$OUT" | grep -qE "charlie3|delta4"; then
  pass "D1 restoring the passphrase restores the login — the cliff is recoverable"
else
  fail "D1 restoring the passphrase did NOT restore the login — the tokens are stranded"
fi

# ==========================================================================
say "LEG E — a fresh isolated profile with NO secure rung at all"
unset WAYLAND_VAULT_PASSPHRASE
export WAYLAND_HOME="$WORK/home2"; mkdir -p "$WAYLAND_HOME"
export CODEX_HOME="$WORK/codexE"; make_auth_json "$CODEX_HOME" "echo5"
OUT="$("$BIN" auth login chatgpt --import-codex 2>&1)"; echo "$OUT"
if echo "$OUT" | grep -qi "imported chatgpt login"; then
  fail "E1 a login was accepted with no secure rung mounted"
else
  pass "E1 a login with no secure rung is REFUSED, not downgraded"
fi
if grep -rl "rt-eeee" "$WAYLAND_HOME" 2>/dev/null | head; then
  fail "E2 the refused login still left the refresh token on disk in cleartext"
else
  pass "E2 the refused login left no cleartext token behind"
fi
OUT="$(run_status)"; echo "$OUT"
echo "$OUT" | grep -qi "not signed in" \
  && pass "E3 a refused login reports an honest not-signed-in, not a stuck refusal" \
  || fail "E3 a refused login left the profile stuck in a refusal"

printf '\n=== VERDICT: %s defect(s) ===\n' "$FAILURES"
exit 0
