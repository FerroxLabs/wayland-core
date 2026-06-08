# fixtures/

Placeholder. T6-T8 ship fixture trees here:

- `s02-fixture/`, `s03-fixture/`, … — scaffold crates for the code-dev scenarios.
- `s17-fixture/`, `s18-fixture/`, `s30-fixture/`, `s33-fixture/` — multi-file / parsing fixtures.
- `mock_mcp_echo.sh` (or `.py`) — 50-line stdio MCP server for S35.
- `golden/s11-trending.json` — captured-good session JSON for the verbatim repro.

Per the plan, fixtures should be regenerated from scratch by each scenario's `setup` closure when possible (cheaper to inspect than a checked-in tree). Use this dir only for inputs that are too large or too binary for in-source scaffolding.
