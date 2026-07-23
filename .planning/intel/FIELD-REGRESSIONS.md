# Live Field Regression Register — CTRL-03

Current packaged/runtime evidence outranks historical acceptance. An old green receipt does not close a newer contradictory report.

Statuses: `OPEN`, `REPRODUCED`, `ROUTED`, `FIXED`, `PACKAGED_PROVEN`. A source fix or unit test cannot skip directly to `PACKAGED_PROVEN`.

| ID | Symptom | Likely seam | Status | Admission route |
|---|---|---|---|---|
| FIELD-MAC-001 | macOS sandbox blocks ordinary developer tools, browser/loopback, or hides the actionable allow path | Core policy + protocol telemetry + Desktop control | OPEN | reproduce on shipped bundle; route by enforcement vs presentation owner |
| FIELD-MCP-001 | connected MCP is absent from session tool manifest or stdio launcher cannot inherit the configured executable environment/PATH | Core MCP lifecycle/readiness + Desktop launch/config | OPEN | deterministic stdio/late-MCP fixture and packaged host replay |
| FIELD-CONFIG-001 | Desktop-hosted sessions and raw Core resolve different config roots/overrides without visible effective-source truth | Core config precedence + Desktop lowering | OPEN | paired standalone/host fixture and effective-config receipt |
| FIELD-SPACE-001 | temporary workspaces accumulate or retention/pruning truth is unclear after chats are deleted | Core workspace lifecycle + Desktop UX | OPEN | clock-controlled retention/restart/prune corpus |
| FIELD-MEDIA-001 | image understanding/generation or MCP image creation is visible but lacks usable credential/readiness propagation | Core provider/MCP capability truth + Desktop credential lowering | OPEN | built-in/MCP-only/late-MCP packaged corpus in Phase 27 |

Each entry must gain exact version, platform, reproduction, owner, fix candidate, focused proof, packaged proof, and limitation before closure. These findings do not derail unrelated Phase 20 source packets unless reproduction proves a Phase 20 invariant is false.

