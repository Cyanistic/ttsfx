#!/usr/bin/env bash
# Hit local ttsfx and write WAVs under test_samples/ (gitignored audio).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/test_samples"
BASE="${TTSFX_URL:-http://127.0.0.1:3000}"
mkdir -p "$OUT"

post() {
  local name="$1"
  local json="$2"
  echo "→ $name"
  curl -sS -o "$OUT/$name.wav" -w "  %{http_code} %{size_download} bytes in %{time_total}s\n" \
    -X POST "$BASE/v1/audio/speech" \
    -H 'Content-Type: application/json' \
    -H "x-request-id: test-$name" \
    -d "$json"
  file "$OUT/$name.wav" | sed 's/^/  /'
}

post baseline '{
  "model": "kokoro",
  "voice": "af_heart",
  "input": "Baseline. BOOM Done.",
  "response_format": "wav"
}'

post speed_fast '{
  "model": "kokoro",
  "voice": "af_heart",
  "input": "Fast speech. BOOM End.",
  "speed": 1.35,
  "response_format": "wav"
}'

post speed_slow '{
  "model": "kokoro",
  "voice": "af_heart",
  "input": "Slow speech. BOOM End.",
  "speed": 0.85,
  "response_format": "wav"
}'

post norm_off '{
  "model": "kokoro",
  "voice": "af_heart",
  "input": "No normalize. BOOM OK.",
  "normalization_options": { "normalize": false },
  "response_format": "wav"
}'

post norm_and_speed '{
  "model": "kokoro",
  "voice": "af_heart",
  "input": "Both opts. BOOM Finish.",
  "speed": 1.15,
  "normalization_options": { "normalize": false },
  "response_format": "wav"
}'

echo "Wrote samples under $OUT/"
ls -la "$OUT"/*.wav