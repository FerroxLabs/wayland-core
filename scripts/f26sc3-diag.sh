#!/bin/sh
# Throwaway diagnostic: what EXACTLY differs after a rolled-back migrate?
set -u
BIN="$1"
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
W=$(mktemp -d)
echo "WORK: $W"
python3 "$HERE/portability-migrate-corpus.py" --kind hermes --out "$W/seed" --items 24 >/dev/null
python3 "$HERE/portability-migrate-corpus.py" --kind hermes --out "$W/main" --items 220 >/dev/null

T="$W/tpl"; mkdir -p "$T"
WAYLAND_HOME="$T" "$BIN" migrate hermes --home "$W/seed" --yes --overwrite >/dev/null 2>&1
mkdir -p "$T/sessions"; printf 'DB\n' > "$T/memory.db"; printf '{}\n' > "$T/sessions/p.json"
PRE=$("$BIN" backup digest --home "$T" | sed -n 's/^DIGEST: //p')
echo "PRE: $PRE"

H="$W/h"; cp -a "$T" "$H"
echo "COPY: $("$BIN" backup digest --home "$H" | sed -n 's/^DIGEST: //p')"

WAYLAND_HOME="$H" WAYLAND_MIGRATE_SCOPE_PROBE=1 "$BIN" migrate hermes --home "$W/main" --yes --overwrite > "$W/run.log" 2>&1 &
PID=$!
python3 -c "import time; time.sleep(0.08)"
kill -9 "$PID" 2>/dev/null; wait "$PID" 2>/dev/null
echo "COMM-WAS: (pid $PID)"
echo "POSTKILL: $("$BIN" backup digest --home "$H" | sed -n 's/^DIGEST: //p')"
echo "--- journal before recover ---"
ls -la "$H/.wayland-backup-journal" 2>/dev/null | head -10
for f in "$H/.wayland-backup-journal"/*.json; do
    [ -f "$f" ] && { echo "RECORD $f:"; cat "$f"; }
done
echo "--- undo store contents ---"
for d in "$H/.wayland-backup-journal"/undo-*; do
    [ -d "$d" ] && { echo "UNDO $d:"; ls -la "$d" | head; }
done

"$BIN" backup recover --home "$H"
echo "FINAL: $("$BIN" backup digest --home "$H" | sed -n 's/^DIGEST: //p')"

echo "=== diff -rq template vs recovered home ==="
diff -rq "$T" "$H" 2>&1 | head -40
echo "=== end diff ==="
echo "WORK RETAINED: $W"
