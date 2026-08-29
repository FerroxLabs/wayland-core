---
issue: 1237
repo: FerroxLabs/wayland
kind: defect
title: "Typed failure category on ErrorInfo: core can name three of #388's five and reports all of them as prose"
status: open
last_verified_commit: 5639e5ff
criteria:
  - id: c1
    text: "ErrorInfo carries a typed failure category covering the three #388 names core can decide: context/token limit, tool/runtime failure, local Wayland error"
    state: not-met
    owner: core
    note: "crates/wcore-protocol/src/events.rs:2289-2293 is `ErrorInfo { code: String, message: String, retryable: bool }` -- a free-form code with no typed category, no provider identity and no upstream status. Verified at this commit, not inherited from #388's note."
  - id: c2
    text: "Every terminal error exit of the run loop sets the category, and the set of exits is enumerated rather than sampled"
    state: not-met
    owner: core
    note: "The engine already KNOWS which category it is at each exit -- the context-ceiling abort, the output-cap truncation gate, the tool-failure breaker and the local authority faults are separate code paths. The risk this criterion exists to catch is a default arm that silently swallows a new exit, which is why it asks for an exhaustive match and not a spot check. The enumeration to start from is the one wayland#388 c8 used for the incompleteness admission: four provider-failure Err exits, four UserAborted exits, ~nine SessionAuthority exits."
  - id: c3
    text: "The field is additive on the wire: a host that does not know it still parses the frame"
    state: not-met
    owner: core
    note: "ErrorInfo is in the pinned protocol contract, so this needs a contract-corpus regeneration and a fixture decode of a pre-change payload."
  - id: c4
    text: "A failure core cannot classify reports as unknown rather than picking one"
    state: not-met
    owner: core
    note: "The router-versus-provider split is #1184's and cannot be decided from this repo: both arrive as the same non-2xx from the same host. The failure mode here is not omission, it is a guess presented as a classification, so the criterion is written as a refusal that can be tested -- a bare non-2xx from an OpenAI-shaped endpoint must come back as neither `rate_limit` nor `router_failure`."
---

Decomposed from FerroxLabs/wayland#388 c7 on 2026-08-30, by the lane that
closed #388's harness half.

#388 asks the product to expose which of five things went wrong. Two of them —
provider rate limit and router failure — are indistinguishable from outside the
router and belong to #1184. The other three are decidable inside core today and
still reach the host as prose, so a Desktop app, a JSON-stream consumer or a CI
wrapper has to pattern-match English to find out why a long run died.

Split out rather than closed inside #388 for two reasons. It is a contract
addition to a pinned protocol surface, which is a different kind of change from
the harness fixes that lane shipped. And it overlaps wayland-core#314 c5 — the
same shape, an untyped frame where a typed one is owed, on the grant-refusal
path — so doing them separately means two contract bumps for one contract
change. Whoever takes either should read both.
