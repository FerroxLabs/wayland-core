---
issue: 1225
repo: FerroxLabs/wayland
kind: defect
title: "Desktop drops the MCP per-tool allowlist on the ACP session/create wire, so switched-off tools stay live (wayland#998 c5)"
status: closed
last_verified_commit: 3d2089b45
criteria:
  - id: c1
    text: "Desktop's ACP session/create request carries the per-tool selection for every narrowed MCP server as mcp_servers[].allowed_tools or the accepted allowedTools alias, with sent JSON captured and attached."
    state: blocked
    owner: desktop
    note: "Live #1323 comment https://github.com/FerroxLabs/wayland/issues/1323#issuecomment-5551140136 corrects the earlier closure inference: Desktop standard ACP uses session/new, Core custom RPC uses session/create, and no adapter serializing Core mcp_servers[].allowed_tools was located. Source grep of allowedTools does not prove this exact ACP route. #1225 is closed on the tracker, but this criterion is not established; no explicit transfer of this criterion is invented."
  - id: c2
    text: "End to end against a real wayland-core acp backend, a tool switched off in the MCP Library is absent from the tools Core offers for that session, with the offered list attached."
    state: superseded
    owner: desktop
    successor: "FerroxLabs/wayland#1323"
    note: "Explicitly carried by #1323 c5 and its Also carried here section, following the 2026-09-05 maintainer decision cited there. Live offered-tool-list capture remains outstanding; JSON-stream fixture pairing is not this ACP capture."
  - id: c3
    text: "All tools off is sent as an empty array, not by omitting the field; grade a case where every tool on one server is switched off."
    state: blocked
    owner: desktop
    note: "Live #1323 comment https://github.com/FerroxLabs/wayland/issues/1323#issuecomment-5551140136 corrects the earlier closure inference: Desktop standard ACP uses session/new, Core custom RPC uses session/create, and no adapter serializing Core mcp_servers[].allowed_tools was located. Source grep of allowedTools does not prove this exact ACP route. #1225 is closed on the tracker, but this criterion is not established; no explicit transfer of this criterion is invented. Empty-array preservation in other Desktop paths does not establish the requested Core ACP wire capture."
  - id: c4
    text: "Version skew is negotiated on ServerCapabilities.mcp_tool_selection and Desktop does not send the field to a Core that does not advertise it."
    state: superseded
    owner: desktop
    successor: "FerroxLabs/wayland#1323"
    note: "Explicit #1323 c1-c3 successor. Its later source-verification comment requires reconciling the actual standard-ACP/custom-RPC route before implementing negotiation. Missing negotiation remains unverified, not met."
---

Recorded from live #1225 body/comments and #1323 body/comments on 2026-09-06. #1225 stays CLOSED; #1323 is OPEN in milestone 0.13.14. The explicit c2/c4 residual moves are preserved. c1/c3 remain blocked because later route evidence contradicts their earlier source-grep-only closure basis; the owning lane must reconcile their disposition. This can keep release-readiness red and is not a new waiver. Parent #998 c5 remains pointed at this closed carrier, whose residual is now recorded.
