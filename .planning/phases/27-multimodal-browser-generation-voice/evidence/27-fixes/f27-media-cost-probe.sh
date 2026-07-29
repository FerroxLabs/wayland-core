#!/usr/bin/env bash
# Does the provider return ANY billing data for each media call shape?
#
# This is the decisive question for defect 3: the product cannot price a media
# call the provider never prices. Captures RESPONSE HEADERS (where transcription
# cost was previously found) as well as the JSON body.
#
# Reads FLUX_API_KEY from the environment. NEVER prints it; every capture is
# redacted and byte-counted.
#
# COST: 1 transcription (~$0.0167, 10s floor) + 1 image (~$0.08). Deliberate.
set -uo pipefail

BASE="https://api.fluxrouter.ai/v1"
OUT="${1:?usage: $0 <outdir>}"
AUDIO="${2:?usage: $0 <outdir> <wavfile>}"
mkdir -p "$OUT"
[ -z "${FLUX_API_KEY:-}" ] && { echo "FATAL: FLUX_API_KEY unset" >&2; exit 2; }

redact() { sed -e "s/${FLUX_API_KEY}/<REDACTED_KEY>/g" -e 's/[A-Za-z0-9_-]\{40,\}/<REDACTED_LONGSTRING>/g'; }

# Report the credit counter so spend is metered, not estimated.
avail() {
  curl -sS -D - -o /dev/null "$BASE/models" -H "Authorization: Bearer $FLUX_API_KEY" \
    | tr -d '\r' | awk 'tolower($1)=="x-flux-available:"{print $2}'
}

echo "CREDIT_BEFORE=$(avail)"

echo
echo "### A. transcription — the exact wire shape openai_compat_whisper.rs sends"
# multipart + response_format=verbose_json + model, headers dumped to a file.
curl -sS -D "$OUT/stt.headers.txt" -o "$OUT/stt.body.json" \
  -X POST "$BASE/audio/transcriptions" \
  -H "Authorization: Bearer $FLUX_API_KEY" \
  -F "file=@$AUDIO" \
  -F "model=${FLUX_STT_MODEL:-flux-voice-fast}" \
  -F "response_format=verbose_json"
STT_RC=$?
redact < "$OUT/stt.headers.txt" > "$OUT/t" && mv "$OUT/t" "$OUT/stt.headers.txt"
redact < "$OUT/stt.body.json"   > "$OUT/t" && mv "$OUT/t" "$OUT/stt.body.json"
echo "stt_curl_rc=$STT_RC header_bytes=$(wc -c < "$OUT/stt.headers.txt"|tr -d ' ') body_bytes=$(wc -c < "$OUT/stt.body.json"|tr -d ' ')"
echo "--- cost-bearing response headers ---"
grep -iE '^(x-flux|x-request|x-ratelimit)' "$OUT/stt.headers.txt"
echo "STT_COST_HEADER_PRESENT=$(grep -ic 'x-flux-cost' "$OUT/stt.headers.txt")"
echo "--- transcript text ---"
python3 -c "
import json,sys
d=json.load(open('$OUT/stt.body.json'))
print('text=%r' % d.get('text'))
print('body_has_cost_field=%s' % any(k for k in d if 'cost' in k.lower() or 'usage' in k.lower()))
print('body_keys=%s' % sorted(d.keys()))
"

echo
echo "### B. image generation — one real image on the arm the key can use"
curl -sS -D "$OUT/img.headers.txt" -o "$OUT/img.body.json" \
  -X POST "$BASE/images/generations" \
  -H "Authorization: Bearer $FLUX_API_KEY" -H 'Content-Type: application/json' \
  --data '{"model":"flux-image","prompt":"a single red cube on a plain white background","n":1}'
IMG_RC=$?
redact < "$OUT/img.headers.txt" > "$OUT/t" && mv "$OUT/t" "$OUT/img.headers.txt"
echo "img_curl_rc=$IMG_RC header_bytes=$(wc -c < "$OUT/img.headers.txt"|tr -d ' ') body_bytes=$(wc -c < "$OUT/img.body.json"|tr -d ' ')"
echo "--- cost-bearing response headers ---"
grep -iE '^(x-flux|x-request|x-ratelimit)' "$OUT/img.headers.txt"
echo "IMG_COST_HEADER_PRESENT=$(grep -ic 'x-flux-cost' "$OUT/img.headers.txt")"
python3 -c "
import json
d=json.load(open('$OUT/img.body.json'))
print('body_top_level_keys=%s' % sorted(d.keys()))
print('has_usage=%s  has_cost=%s' % ('usage' in d, any('cost' in k.lower() for k in d)))
print('n_images=%d' % len(d.get('data',[])))
import base64
b=d['data'][0].get('b64_json')
print('image_bytes=%d' % (len(base64.b64decode(b)) if b else -1))
"
# Do not keep the image payload (large, and not evidence).
rm -f "$OUT/img.body.json"

echo
echo "CREDIT_AFTER=$(avail)"
