# Archetype 01: legacy-yaml-power-user

## What this represents

A power-user machine where the IJFW-era Desktop app wrote `config.yaml` and the
Wayland engine subsequently also wrote `config.toml` — both files coexist in the
same `$WAYLAND_HOME`. Two MCP servers are registered (one with an API key in its
`env` block). Two real sessions from different providers. Two cron jobs written by
the Desktop app using the `schedule` alias field (not the canonical `expression`
field the engine expects). Two plan files.

This is the closest archetype to an actual power user who has been running Wayland
across the Desktop app and the CLI for several weeks.

## Bug classes targeted

| Class | Finding | What breaks without this fixture |
|---|---|---|
| **B-11** Real-config layout drift | F-011, F-018 | `Config::resolve` loads default instead of either yaml or toml when both exist; the resolution order is never tested |
| **B-3** Protocol contract drift | R-001 | The Desktop-app `schedule` alias field on cron jobs silently fails to deserialize to `expression`; cron list shows empty |
| **B-9** Subprocess / plugin lifecycle | F-016 | MCP server inherits host env including real `WAYLAND_IMG_API_KEY`; with sealed env, the MCP server must get only what the fixture's `env` block declares |
| **B-4** Registered but unreachable | F-003 | Dual-config path means init_history may load the wrong config's system prompt template |

## Scenarios that replay against this fixture (Wave 2+)

1. Boot the engine with `WAYLAND_HOME=<this dir>` and assert the `ready` event
   contains `provider = "anthropic"` (config.toml wins over config.yaml for the
   canonical block, but yaml-only keys are merged).
2. Run `cron list` and assert both jobs appear with correct schedule expressions
   (proving the `schedule` alias deserialized).
3. Spawn the engine with sealed env (`env_clear()` + allowlist) and assert the
   wayland-image-generation MCP server either receives its key from the fixture
   env block OR fails gracefully (not silently leaking the host's real key).

## Anti-leak gate result

```
grep -rE 'sk-[a-zA-Z0-9]{20,}|AIza[a-zA-Z0-9_-]{20,}|seandonahoe\.com|<<HOME>>/' .
# 0 matches — verified 2026-05-24
```
