# Fixture Catalog

Each subdirectory here is a named archetype — a sanitized snapshot of a
`$WAYLAND_HOME` directory that represents a specific user-environment shape.

The T6 fixture-replay harness (Wave 2) will spawn the release binary against
each archetype directory and assert on emitted json-stream events, stderr
cleanliness, and post-run state diff.

## Sanitization

All files are sanitized per `.blackboard/E2E-FIXTURE-SANITIZATION-2026-05-24.md`.
No fixture file contains a real API key, personal email, machine path, or
identifiable session content.

## Adding a new archetype

1. Create `fixtures/<name>/`.
2. Populate the `$WAYLAND_HOME` tree (config.toml, cron/, sessions/, etc.).
3. Run the anti-leak grep (see sanitization spec §5.1) and confirm zero matches.
4. Write `MANIFEST.json` (schema in sanitization spec §4.2).
5. Write `README.md` covering: purpose, bug classes targeted, scenarios that
   will replay against it.

## Current archetypes

| Directory | Bug classes | Purpose |
|---|---|---|
| `01-legacy-yaml-power-user/` | B-11, B-3, B-4 | yaml+toml dual config, Desktop-app cron |
| `02-fresh-install/` | B-2, B-5 | empty home, first-launch boot path |
| `03-migration-mid-flight/` | B-11, B-3, B-7 | yaml→toml partial migration, v0 sessions |
| `04-corrupt-recovery/` | B-7, B-4, B-9 | malformed files, recovery posture |
| `05-mcp-multi-server/` | B-9, B-3, B-5 | multiple MCP servers, env hermeticity |
