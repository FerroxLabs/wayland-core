#!/usr/bin/env bash
# Phase 27 Criterion 4 — can the flux-router credential reach transcription and
# TTS at all, and what does the product do with it?
#
# Reads FLUX_API_KEY from the environment. NEVER prints it: every captured
# string is passed through the redactor before it reaches stdout.
#
# Three questions, answered separately because they have different answers:
#   1. does the PROVIDER serve a transcription route, in the wire shape the
#      shipped `OpenAiCompatWhisperBackend` actually sends?
#   2. does the PROVIDER serve a TTS route?
#   3. can the PRODUCT reach either of them?  (answered by the resolver, not
#      here -- see `build_transcription_backend`, tool_backends/mod.rs)
#
# The positive input is generated locally and free with macOS `say`, so the
# probe has a KNOWN expected transcript. A silence file of identical duration
# and format is the negative control: without it, a driver that always echoed
# the expected string would be indistinguishable from a working provider.

set -u
BASE="${FLUX_BASE_URL:-https://api.fluxrouter.ai/v1}"
: "${FLUX_API_KEY:?FLUX_API_KEY must be set in the environment}"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

redact() { sed "s|${FLUX_API_KEY}|<REDACTED-KEY>|g"; }

EXPECTED="The quick brown fox jumps over the lazy dog near the riverbank."

echo "=== inputs ==="
if command -v say >/dev/null 2>&1 && command -v afconvert >/dev/null 2>&1; then
  say -v Samantha -o "$WORK/speech.aiff" "$EXPECTED"
  afconvert -f WAVE -d LEI16@16000 -c 1 "$WORK/speech.aiff" "$WORK/speech.wav"
else
  echo "SKIP: this probe's positive input needs macOS \`say\`+\`afconvert\`" >&2
  exit 3
fi
python3 - "$WORK" <<'PY'
import sys, wave
w = wave.open(sys.argv[1] + "/speech.wav")
n = w.getnframes()
print(f"POSITIVE ch={w.getnchannels()} rate={w.getframerate()} width={w.getsampwidth()} "
      f"frames={n} seconds={n/w.getframerate():.2f}")
s = wave.open(sys.argv[1] + "/silence.wav", "wb")
s.setnchannels(1); s.setsampwidth(2); s.setframerate(16000)
s.writeframes(b"\x00\x00" * n); s.close()
print(f"NEGATIVE ch=1 rate=16000 width=2 frames={n} (all-zero samples)")
PY
echo "POSITIVE_BYTES=$(wc -c < "$WORK/speech.wav" | tr -d ' ')"
echo "NEGATIVE_BYTES=$(wc -c < "$WORK/silence.wav" | tr -d ' ')"
echo

echo "=== Q1: provider transcription route, in the product's own wire shape ==="
# `response_format=verbose_json` is what openai_compat_whisper.rs:61 sends.
for pair in "speech.wav:POSITIVE" "silence.wav:NEGATIVE_CONTROL"; do
  f=${pair%%:*}; label=${pair##*:}
  code=$(curl -sS -D "$WORK/h-$f.txt" -o "$WORK/r-$f.json" -w '%{http_code}' \
    "$BASE/audio/transcriptions" \
    -H "Authorization: Bearer $FLUX_API_KEY" \
    -F "file=@$WORK/$f" -F "model=${FLUX_VOICE_MODEL:-flux-voice-fast}" \
    -F "response_format=verbose_json")
  echo "$label http=$code bytes=$(wc -c < "$WORK/r-$f.json" | tr -d ' ')"
  python3 - "$WORK/r-$f.json" "$EXPECTED" <<'PY' | redact
import json, sys
try:
    d = json.load(open(sys.argv[1]))
except Exception as e:
    print(f"  UNPARSEABLE: {e}"); sys.exit(0)
text = (d.get("text") or "").strip()
print(f"  text={text!r}")
print(f"  verbatim_match={text == sys.argv[2]}")
print(f"  has_segments={bool(d.get('segments'))} language={d.get('language')!r} duration={d.get('duration')}")
print(f"  body_carries_cost={'cost_usd' in json.dumps(d) or 'usage' in d}")
PY
  echo "  cost headers: $(grep -i 'x-flux-cost-usd\|x-flux-billed' "$WORK/h-$f.txt" | tr -d '\r' | tr '\n' ' ')"
done
echo

echo "=== Q2: provider TTS route ==="
for m in flux-voice flux-voice-fast flux-auto; do
  code=$(curl -sS -o "$WORK/tts-$m.out" -w '%{http_code}' "$BASE/audio/speech" \
    -H "Authorization: Bearer $FLUX_API_KEY" -H 'Content-Type: application/json' \
    -d "{\"model\":\"$m\",\"input\":\"barge in test\",\"voice\":\"alloy\"}")
  echo "model=$m http=$code bytes=$(wc -c < "$WORK/tts-$m.out" | tr -d ' ') body=$(head -c 140 "$WORK/tts-$m.out" | tr -d '\0' | redact)"
done
echo
echo "WLDONE_FLUX_VOICE_PROBE"
