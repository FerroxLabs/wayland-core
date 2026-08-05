#!/usr/bin/env bash
# Determine WHY `wayland-core image` with no --model returns 401.
#
# Reads FLUX_API_KEY from the environment. NEVER prints it. Every captured body
# is passed through a redactor before display.
#
# Cost discipline: this script issues NO successful image generation. It only
# lists models (free) and provokes error responses (free). The one paid call in
# this lane is driven separately and deliberately.
set -uo pipefail

BASE="https://api.fluxrouter.ai/v1"
OUT="${1:?usage: $0 <outdir>}"
mkdir -p "$OUT"

if [ -z "${FLUX_API_KEY:-}" ]; then echo "FATAL: FLUX_API_KEY unset" >&2; exit 2; fi

# Redactor: mask the key if it ever appears, plus any long bearer-ish token.
redact() { sed -e "s/${FLUX_API_KEY}/<REDACTED_KEY>/g" -e 's/[A-Za-z0-9_-]\{40,\}/<REDACTED_LONGSTRING>/g'; }

# curl_json <label> <method> <path> [json-body]
# Writes body+status to files; returns curl's OWN exit status (no pipeline).
curl_json() {
  local label="$1" method="$2" path="$3" body="${4:-}"
  local bodyf="$OUT/$label.body.json" codef="$OUT/$label.code.txt"
  local code
  if [ -n "$body" ]; then
    code=$(curl -sS -o "$bodyf" -w '%{http_code}' -X "$method" "$BASE$path" \
      -H "Authorization: Bearer $FLUX_API_KEY" -H 'Content-Type: application/json' \
      --data "$body")
  else
    code=$(curl -sS -o "$bodyf" -w '%{http_code}' -X "$method" "$BASE$path" \
      -H "Authorization: Bearer $FLUX_API_KEY")
  fi
  local rc=$?
  # Redact in place, then byte-count (brief: byte-count every capture).
  redact < "$bodyf" > "$bodyf.tmp" && mv "$bodyf.tmp" "$bodyf"
  printf '%s' "$code" > "$codef"
  local bytes
  bytes=$(wc -c < "$bodyf" | tr -d ' ')
  echo "== $label  http=$code  curl_rc=$rc  body_bytes=$bytes"
  return $rc
}

echo "### 1. Model catalogue (free) — is the default arm even listed?"
curl_json models GET /models
MODELS_RC=$?
echo "models_curl_rc=$MODELS_RC"

# Extract ids WITHOUT a pipeline stealing status: python does the parse and sets rc.
python3 - "$OUT/models.body.json" "$OUT/model-ids.txt" <<'PY'
import json, sys
src, dst = sys.argv[1], sys.argv[2]
try:
    d = json.load(open(src))
except Exception as e:
    print(f"MODEL_PARSE=FAIL {e}"); sys.exit(3)
ids = sorted(m.get("id","") for m in d.get("data", []))
open(dst, "w").write("\n".join(ids) + "\n")
print(f"MODEL_COUNT={len(ids)}")
for probe in ("flux-image-together-flux", "flux-image", "flux-voice-fast"):
    print(f"LISTED[{probe}]={'YES' if probe in ids else 'NO'}")
PY
echo "model_parse_rc=$?"

echo
echo "### 2. The default arm, exactly as the product sends it (error expected, free)"
curl_json img-default POST /images/generations \
  '{"model":"flux-image-together-flux","prompt":"a red cube on a white background","n":1}'
echo "--- body (redacted) ---"; cat "$OUT/img-default.body.json"; echo

echo
echo "### 3. The arm that WORKS, same key, same session (dry contrast: bad param so it cannot bill)"
curl_json img-working-badparam POST /images/generations \
  '{"model":"flux-image","prompt":"","n":0}'
echo "--- body (redacted) ---"; cat "$OUT/img-working-badparam.body.json"; echo

echo
echo "### 4. A model id that certainly does not exist (control)"
curl_json img-nonexistent POST /images/generations \
  '{"model":"definitely-not-a-real-model-xyz","prompt":"a red cube","n":1}'
echo "--- body (redacted) ---"; cat "$OUT/img-nonexistent.body.json"; echo

echo
echo "### 5. No credential at all (control: what a GENUINELY bad key looks like)"
code=$(curl -sS -o "$OUT/img-nokey.body.json" -w '%{http_code}' -X POST "$BASE/images/generations" \
  -H 'Authorization: Bearer sk-obviously-invalid-key-for-control' -H 'Content-Type: application/json' \
  --data '{"model":"flux-image","prompt":"a red cube","n":1}')
echo "== img-nokey  http=$code  body_bytes=$(wc -c < "$OUT/img-nokey.body.json" | tr -d ' ')"
echo "--- body ---"; cat "$OUT/img-nokey.body.json"; echo
