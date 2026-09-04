---
issue: 1225
repo: FerroxLabs/wayland
kind: defect
title: "Desktop drops the MCP per-tool allowlist on the ACP session/create wire, so switched-off tools stay live (wayland#998 c5)"
status: closed
last_verified_commit: d3759f42a
criteria:
  - id: c1
    text: "Desktop's ACP session/create request carries the per-tool selection for every MCP server on which the operator has switched at least one tool off, as mcp_servers[].allowed_tools (or the accepted allowedTools alias)"
    state: met
    evidence: "file:.planning/DECISIONS.md"
    owner: maintainer
    note: "MAINTAINER DECISION 2026-09-05, recorded in .planning/DECISIONS.md under 'Why Q-1225 records the residual rather than reopening'. Verified from Desktop origin/main source rather than from the closing comment: allowedTools is emitted verbatim at mcpSessionConfig.ts:221,234,244 (descriptor emission, presence-checked `!== undefined`) and :322 (hosted/session path), with the stdio spawn filtered at :211,312 via wrapSpawnWithToolFilter(resolved, server.allowedTools ?? []). Graded by the maintainer and NOT by core: the evidence is Desktop-repo source, which cannot be anchored from this tree, and across 199 ledgers every non-core criterion graded met is owner: maintainer. The ticket asked for the sent JSON to be captured and attached; that capture was never made, and the residual verification is carried by FerroxLabs/wayland#1323 c5 rather than being claimed here."
  - id: c2
    text: "End to end against a real wayland-core acp backend: an operator switches a tool off in the MCP Library, and that tool is absent from the tools core offers for that session"
    state: superseded
    successor: "FerroxLabs/wayland#1323"
    owner: maintainer
    note: "NOT evidenced by either closure. Both #1167 and #1225 were closed citing a grep of the Desktop tree ('15 occurrences ... eight explicit references'), which shows the emission exists in source but cannot show the tool list core actually offered on a live session. That is the distinction this criterion was written to force, and composing two separately-verified halves is exactly the failure mode a live capture exists to catch. Carried forward as #1323 c5, which restates it verbatim including the requirement to attach the offered list rather than a screenshot of the switch."
  - id: c3
    text: "All tools off is sent as an empty array, NOT by omitting the field"
    state: met
    evidence: "file:.planning/DECISIONS.md"
    owner: maintainer
    note: "MAINTAINER DECISION 2026-09-05. This is the dangerous half and it is genuinely handled. Core treats an ABSENT allowlist as allow-all and Some([]) as none, so collapsing [] to absent would enable every tool at the exact moment the user asked for none. Desktop guards every emission site with `server.allowedTools !== undefined ? { allowedTools: server.allowedTools } : {}` rather than a truthiness or length check, and WCoreMcpAgent.ts:146,161 carries the [mcp.servers] table with the comment '#1167: presence-checked, NOT truthiness-checked - `[]` must survive.' The type comment at :36-42 states the polarity explicitly: 'undefined -> every tool enabled, [] -> NO tools enabled', and notes smol-toml round-trips the empty array so the distinction survives serialization."
  - id: c4
    text: "Version skew is negotiated on ServerCapabilities.mcp_tool_selection and Desktop does not send the field to a core that does not advertise it"
    state: superseded
    successor: "FerroxLabs/wayland#1323"
    owner: maintainer
    note: "NOT IMPLEMENTED, and this is what checking the closure turned up. `git grep` over all Desktop src/ returns ZERO hits for mcp_tool_selection or mcpToolSelection, against a positive control of 14 allowedTools hits in mcpSessionConfig.ts -- so the zero is a real absence, not a failed query. crates/wcore-acp/src/protocol.rs:135 states a client MUST consult ServerCapabilities::mcp_tool_selection from initialize, and every ACP request type carries deny_unknown_fields, so an older core HARD-REJECTS session/create at parse time rather than ignoring the key. #1225's own text called this out: 'shipping c1 without c4 breaks every older core.' It is superseded rather than left not-met because the residual is genuinely tracked on an open ticket owned by the lane that must fix it, not because the gate needed clearing: the failure requires a Desktop build carrying the c1 emission talking to a core predating SessionCreateRequest::mcp_servers, and shipping a NEWER wayland-core moves every user it reaches away from that pair rather than toward it."
---

# Closed by the desktop lane; two of four criteria carried forward

`#1225` and its duplicate `#1167` were closed at 21:13:51Z and 21:13:54Z on 2026-09-04,
two minutes into the 0.13.13 release run. This ledger exists because `#1225` is the
`handoff:` target of `wayland-998.md` c5, and the release-readiness gate correctly
refused a release in which a closed carrier recorded nothing about whether its residual
was finished or dropped.

Checking that record is what found the c4 gap. The gate did its job.

c1 and c3 shipped and are verified from source. c2 was never evidenced and c4 was never
built; both are carried by `FerroxLabs/wayland#1323`, open and owned by desktop.

Core does not grade another lane's work, so the two `met` rows are maintainer decisions
recorded in `.planning/DECISIONS.md`, not core gradings.
