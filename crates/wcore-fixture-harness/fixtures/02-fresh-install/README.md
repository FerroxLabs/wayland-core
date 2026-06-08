# Archetype 02: fresh-install

## What this represents

A brand-new `$WAYLAND_HOME` with only the minimal config the engine writes on
first launch. No sessions, no cron, no plans, no plugins. No API key is
configured — the provider block is present but empty.

This is the zero-state baseline. Every bug that affects the happy-path first
launch is visible here and invisible in the power-user fixtures.

## Bug classes targeted

| Class | Finding | What breaks without this fixture |
|---|---|---|
| **B-1** Silent diagnostic dropout | F-001 | `tracing_subscriber` never installed; engine boots silently with no observable events |
| **B-2** Boot-path order / flag short-circuit | F-012, F-018 | `Config::resolve` runs before flag dispatch; `--list-sessions` creates `~/.wayland-core` files in the real $HOME |
| **B-5** Hermeticity / sandbox leak | F-010, F-035 | Engine reads real `~/Library/Application Support/wayland-core/` during an empty-home boot |

## Scenarios that replay against this fixture (Wave 2+)

1. Boot with `WAYLAND_HOME=<this dir>`, `HOME=/tmp/sealed-home` — assert nothing
   written outside `WAYLAND_HOME`. Catches B-5.
2. `--list-sessions` with this fixture — assert exit 0, empty list, zero files
   created in sealed HOME. Catches B-2.
3. Boot full engine — assert `ready` event emitted on stderr within 2s. Catches B-1.
4. Boot with no api_key — assert a `provider_error` json event (not a panic/crash).

## Anti-leak gate result

```
grep -rE 'sk-[a-zA-Z0-9]{20,}|AIza[a-zA-Z0-9_-]{20,}|seandonahoe\.com|<<HOME>>/' .
# 0 matches — verified 2026-05-24
```
