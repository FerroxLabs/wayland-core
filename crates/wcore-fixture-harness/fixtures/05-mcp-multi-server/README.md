# Archetype 05: mcp-multi-server

## What this represents

Four MCP servers registered simultaneously: one stdio/deferred (image-gen with
an API key in its env block), one stdio/eager (local-fs, no key), one SSE
(remote-sse with a Bearer token), and one stdio/deferred (tools-pack). The
`top_k=10` curation limit is set. One cron job uses the `Skill` target kind.

This directly models Sean's real machine state (the `wayland-image-generation`
MCP server with the `WAYLAND_IMG_API_KEY`) and expands it to cover all MCP
transport types and the full curation path.

The primary bug this catches is F-016: the engine's stdio MCP subprocess
inheriting the host shell's full env, which means any `*_API_KEY` present in the
developer's shell leaks into the MCP server process. With sealed env, the MCP
server must get only what the fixture's `[mcp.servers.*.env]` block declares.

## Bug classes targeted

| Class | Finding | What breaks without this fixture |
|---|---|---|
| **B-9** Subprocess / plugin lifecycle | F-016 | MCP server subprocess inherits host env; `WAYLAND_IMG_API_KEY` leaks even when the sealed env is supposed to prevent it |
| **B-3** Protocol contract drift | F-021 | Deferred vs eager tool schema serialization produces different ready-event shapes; the LLM sees different tool lists depending on boot order |
| **B-5** Hermeticity / sandbox leak | F-010, F-035 | Multi-server boot reads from outside `$WAYLAND_HOME` when resolving server command paths |

## Scenarios that replay against this fixture (Wave 2+)

1. Boot with `env_clear()` + allowlist — assert `image-gen` subprocess env does
   NOT contain the host's `WAYLAND_IMG_API_KEY`. Catches F-016.
2. Boot — assert `ready` event tool list has deferred stubs for `image-gen` and
   `tools-pack`, full schema for `local-fs`. Catches B-3 deferred/eager split.
3. `cron list` — assert job appears with `target.kind = "skill"`. Catches
   F-014 (Skill target arm exercised).
4. Boot — assert `top_k` curation caps tool count at 10 in turn payloads.

## Anti-leak gate result

```
grep -rE 'sk-[a-zA-Z0-9]{20,}|AIza[a-zA-Z0-9_-]{20,}|seandonahoe\.com|<<HOME>>/' .
# 0 matches — verified 2026-05-24
```
