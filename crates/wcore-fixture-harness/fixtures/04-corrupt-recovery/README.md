# Archetype 04: corrupt-recovery

## What this represents

Deliberately broken state: valid `config.toml` but the cron job store is
truncated mid-array (simulates SIGKILL during a cron write), one of two session
files is truncated mid-JSON (simulates disk-full during session save), and the
config references an MCP server at a path that doesn't exist.

This is the "recovery" fixture — it tests the engine's posture when it encounters
bad data: recover-and-continue, log the error, skip the corrupt entry. Any panic
or silent crash is a regression.

## Bug classes targeted

| Class | Finding | What breaks without this fixture |
|---|---|---|
| **B-7** State file corruption / loss | F-030, F-031, F-033, F-034 | Truncated session file causes panic on load; engine crashes instead of skipping |
| **B-9** Subprocess / plugin lifecycle | F-037, F-086 | Non-existent MCP server causes panic at plugin discovery; engine hangs waiting for stdio |
| **B-4** Registered but unreachable | F-037 | Plugin that fails to load silently removes capabilities without emitting an error event |

## Scenarios that replay against this fixture (Wave 2+)

1. Boot engine — assert exit 0 (not 1), assert error event emitted for truncated
   `jobs.json`, assert `ready` event still fires. Catches B-7.
2. `--list-sessions` — assert `fixture00c001` listed, `fixture00c002` absent
   (or listed with `status=corrupt`). Catches B-7.
3. Boot engine — assert MCP `nonexistent-server` emits a `mcp_spawn_error` event
   and the engine continues without it (no hang, no panic). Catches B-9.
4. Wave 2 chaos injection (T10): SIGKILL mid-session-write, then boot — assert
   recovery of the good session. Catches B-7 / F-030.

## Anti-leak gate result

```
grep -rE 'sk-[a-zA-Z0-9]{20,}|AIza[a-zA-Z0-9_-]{20,}|seandonahoe\.com|<<HOME>>/' .
# 0 matches — verified 2026-05-24
```
