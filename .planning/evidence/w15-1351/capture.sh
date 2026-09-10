#!/bin/bash
# The key below is a placeholder that is never dialled: the endpoint is a
# refused loopback port, so the run dies at connect and no credential is used.
# wayland#1351 c2 -- capture the USER-FACING stream of a real CLI run with
# RUST_LOG unset, once against a memory store that cannot open (degraded arm)
# and once against a healthy one (control).
set -u
BIN=/root/waylandcore-stabilization-20260905/slots/parallel-1/target/debug/wayland-core
OUT=/root/w15-c2
rm -rf "$OUT"; mkdir -p "$OUT"

run_arm () {
  ARM=$1
  HOME_DIR=$OUT/$ARM/home
  MEM_DIR=$OUT/$ARM/mem
  mkdir -p "$HOME_DIR" "$MEM_DIR/memory"
  if [ "$ARM" = "degraded" ]; then
    python3 - "$MEM_DIR/memory/memory.db" <<'PY'
import sqlite3, sys
c = sqlite3.connect(sys.argv[1])
c.execute("CREATE TABLE schema_version (version INTEGER NOT NULL PRIMARY KEY)")
c.execute("INSERT INTO schema_version VALUES (99)")
c.commit(); c.close()
PY
  fi
  ( cd "$OUT/$ARM"
    env -u RUST_LOG -u WAYLAND_DEBUG \
      WAYLAND_HOME="$HOME_DIR" \
      WCORE_MEMORY_DIR="$MEM_DIR" \
      "$BIN" --provider openai --base-url http://127.0.0.1:1 \
             --api-key "${W15_FAKE_KEY:-not-a-key}" --model gpt-test-model \
             "say hi" > "$OUT/$ARM.stdout" 2> "$OUT/$ARM.stderr" )
  echo "ARM=$ARM exit=$? rust_log=${RUST_LOG:-<unset>}"
  echo "-- stdout bytes: $(wc -c < "$OUT/$ARM.stdout") --"
  echo "-- stderr bytes: $(wc -c < "$OUT/$ARM.stderr") --"
  echo "-- hits for the memory notice (stdout+stderr) --"
  cat "$OUT/$ARM.stdout" "$OUT/$ARM.stderr" | grep -c "long-term memory is OFF for this session"
  echo "-- the line itself, if any --"
  cat "$OUT/$ARM.stdout" "$OUT/$ARM.stderr" | grep "long-term memory is OFF" || echo "(none)"
  echo "-- any mention of memory at all --"
  cat "$OUT/$ARM.stdout" "$OUT/$ARM.stderr" | grep -i "memory" || echo "(none)"
  echo "============================================================"
}

echo "RUST_LOG in this shell: ${RUST_LOG:-<unset>}"
run_arm degraded
run_arm control
echo "-- full degraded stdout --"
cat "$OUT/degraded.stdout"
echo "-- full degraded stderr --"
cat "$OUT/degraded.stderr"
