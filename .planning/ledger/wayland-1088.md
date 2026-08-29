---
issue: 1088
repo: FerroxLabs/wayland
kind: defect
title: "Bug report: Chat Interface Bug and Restricted Read / Glob / Write / Edit"
status: open
last_verified_commit: f0060a2e8
criteria:
  - id: c1
    text: "The typed event exists and the generated contract corpus row is correct"
    state: met
    evidence: "symbol:crates/wcore-protocol/src/contract/generate.rs::generated_artifacts"
    owner: core
    note: "the guard now lives INSIDE generated_artifacts(), so a violating corpus cannot be emitted at all rather than being caught after the fact"
  - id: c2
    text: "The user-visible half — the chat interface no longer reports Read/Glob/Write/Edit as restricted"
    state: blocked
    owner: desktop
    handoff: "FerroxLabs/wayland#1223"
    note: "AUDITED 2026-08-29 and CONFIRMED cross-team; the carrier already existed and the ledger had simply never named it. Core emits the typed workspace_policy receipt. Desktop decodes it into lastWorkspacePolicy and then drops it twice before any UI sees it -- WCoreManager.ts holds an explicit forward-allowlist of empty-msg_id session events that workspace_policy is not in, and it then hits the `if (!data.msg_id) return;` guard -- and an exhaustive sweep of Desktop source plus all 15 locale bundles finds NO Desktop-authored 'restricted' string at all, so the sentence the reporter sees is model-authored and the one receipt that would contradict it never arrives. Nothing under crates/ renders it. #1188 is open, needs:desktop, and carries the contract plus a second and worse defect found while investigating: ToolsPane ships a hardcoded copy of core's default allow-list that omits Write/Edit/Bash, and one unrelated toggle freezes it into config.toml, genuinely disabling them HANDOFF TARGET RECONCILED ON MERGE: this criterion was decomposed twice on 2026-08-29, by two lanes that could not see each other. The audit lane pointed it at FerroxLabs/wayland#1188, which already existed and already carries the work; the decomposition lane filed FerroxLabs/wayland#1223 and that is the ticket named above, because it is scoped to this criterion. BOTH ARE OPEN AND THEY OVERLAP -- both are the Desktop workspace_policy / stale restricted-tools defect. Core does not close issues, so this is recorded rather than acted on: a maintainer should dedupe the pair, and whichever survives is the carrier. The audit evidence in this note was gathered against FerroxLabs/wayland#1188 and applies to either."
---

Partially fixed in v0.13.10.

Core's half was to emit a typed event the host can render correctly, and to
stop the generated contract corpus from carrying the wrong row. The guard was
moved inside `generated_artifacts()` so a violating corpus cannot be produced
in the first place — a check that runs after generation can be skipped; one
that runs inside it cannot.

What the reporter actually sees is rendered by Desktop, so this stays open
against that lane.
