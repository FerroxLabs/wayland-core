# Archetype 03: migration-mid-flight

## What this represents

A machine that ran the v0.8.1 -> v0.8.2 upgrade but the migration is only
half-complete: `config.toml` has the new `[default]` block but the old
`config.yaml` still exists. The `jobs.json` still uses the Desktop-app `schedule`
alias field (canonical re-serialization to `expression` hasn't happened yet).
One session file is v0-schema (missing `schema_version` field).

This catches the entire class of "upgrade in progress" bugs that only appear
between the first and second boot after a version bump.

## Bug classes targeted

| Class | Finding | What breaks without this fixture |
|---|---|---|
| **B-11** Real-config layout drift | F-011, F-018 | Config resolution silently picks wrong source when both yaml and migrated toml exist |
| **B-3** Protocol contract drift | R-001 | `schedule` alias field on cron jobs deserializes incorrectly; cron list silently drops jobs |
| **B-7** State file corruption / loss | F-031, F-032 | Session loader drops v0 sessions instead of running migration ladder; `--list-sessions` returns partial list |

## Scenarios that replay against this fixture (Wave 2+)

1. Boot engine, assert `ready` event has `provider = "anthropic"` (from toml
   `[default]`, not from yaml `model.provider`). Catches B-11.
2. `cron list` — assert the Weekly Review job appears with `expression = "0 18 * * 5"`.
   Catches B-3 (`schedule` alias).
3. `--list-sessions` — assert `fixture00b001` appears in list. Load it — assert
   `schema_version` is now 1 (migration ladder ran). Catches B-7 forward-compat.

## Anti-leak gate result

```
grep -rE 'sk-[a-zA-Z0-9]{20,}|AIza[a-zA-Z0-9_-]{20,}|seandonahoe\.com|<<HOME>>/' .
# 0 matches — verified 2026-05-24
```
