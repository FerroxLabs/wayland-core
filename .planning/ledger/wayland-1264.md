---
issue: 1264
repo: FerroxLabs/wayland
kind: defect
title: "Egress: an allowlisted apex is admitted on the host match alone, so tool-driven traffic is never shape-checked (split from #1195 c8)"
status: closed
last_verified_commit: 4a738f2e
criteria:
  - id: c1
    text: "A decision is recorded in `.planning/DECISIONS.md` with its reasoning: either the allowlist grant is split by traffic origin (provider vs tool-driven), or the current posture is affirmed as intended and the reason is written down where an operator reading the egress policy can see it."
    state: met
    evidence: "file:.planning/DECISIONS.md"
    owner: core
    note: "Recorded as Q-1264, both as a table row and as a reasoned section. It records the choice (split the grant by the REQUEST's origin), the two shapes considered, and WHY the other was refused -- a policy split per client makes the boundary depend on who constructed the client, which is a bypass factory rather than a boundary. It also records what the decision does NOT cover, in two named residuals rather than silence: a model can still influence a percent-encoded path segment of a scoped API call, and the ModelDirected stamp is an opt-in a future model-URL backend could forget. The operator-facing half of c1 is discharged at the policy itself: the branch in classify.rs carries the reasoning at the code an operator reading the egress policy lands on."
  - id: c2
    text: "Egress to an allowlisted host is shape-checked (method, path, query) when the destination was chosen by the MODEL rather than built by the product, and a test drives the real `WebFetch` surface against an allowlisted apex carrying a query payload, shown RED against the `classify.rs:229` early return."
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1264_model_directed_egress_test.rs::a_model_chosen_query_payload_to_an_allowlisted_apex_is_refused_unattended"
    owner: core
    note: "RESTATED on the issue first (comment 5474881252) and only then graded. `tool-driven` as written is not deliverable and the issue body already carries the reason: narrowing the grant would deny the agent's own LLM POSTs, and the same holds one layer up -- anthropic_vision, openai_vision, openai_compat_whisper, tts, image_gen and firecrawl_web all POST bodies to allowlisted API hosts, so shape-checking `tool-driven` traffic is an outage of six tools, not a boundary. The deliverable property is who chose the DESTINATION, which is what c2's own WebFetch example describes. RED ARM RUN 2026-08-31 on the real instrument, twice and on different mutations. (1) Reverting the classifier branch to its unconditional Allow: the end-to-end arm FETCHED https://github.com/?leak=<24-char base64 token> over the LIVE network and returned Ok { status: 200, content_type: text/html } -- the exfil is demonstrated, not modelled, closing the issue's own `graded from source reading, not a live run` caveat. (2) Reverting the WebFetch origin stamp to Product: the same arm reds AND the wiring arm reds, so the stamp is load-bearing rather than decorative. `cargo check -p wcore-agent --tests` RC=0 before each mutation, so both reds are behaviour and not a build failure. Restored, git status --porcelain = 0, 49/49 green."
  - id: c3
    text: "If the split is taken: provider/LLM traffic to the same apex still receives unconditional `Allow`, with a test that fails if the new check is applied to it — the wrong-refusal control."
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1264_model_directed_egress_test.rs::provider_traffic_to_the_same_apex_keeps_its_unconditional_allow"
    owner: core
    note: "The wrong-refusal control, met as written. A POST to api.github.com -- the same apex the c2 arm is denied on -- keeps EgressDecision::Allow at Product origin. It fails if the origin distinction is ever lost, which is the shape that would refuse every LLM call in the product. Two further controls carry the same weight and are not this criterion: every pre-existing egress arm (classify, policy, defaults, the two tests the issue names as pinning the old behaviour) still passes UNCHANGED through a shim that states they grade Product origin, and `a_scoped_api_backend_is_not_model_directed` pins that http_github is NOT model-directed, without which the wiring assertion would hold for every client in the workspace and prove nothing about WebFetch."
  - id: c4
    text: "`wayland#1195` c8 is resolved against the recorded decision rather than left blocked."
    state: met
    evidence: "file:.planning/ledger/wayland-1195.md"
    owner: core
    note: "RESTATED on the issue first (comment 5474881252). As written c4 is the `if the current posture is affirmed instead` branch; that branch was NOT taken, so it cannot be met as written and grading it met on the original text would be meeting an adjacent property. Its second half stands whichever branch is taken and is the real handoff, so that is what it now says. Discharged: wayland-1195 c8 is no longer `blocked` / `owner: maintainer` -- it is graded met against this work, restated to the delivered scope on its own issue (comment 5474884197) with the residuals named. c4's first half is delivered anyway: every pre-existing arm now reads through a shim naming this decision and saying why those arms grade Product origin."
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on
wayland and EVERY open issue on wayland-core. This issue was in scope from
the moment it was filed and had no ledger file, so
`scripts/check-release-readiness.py` -- which reads ledger files and nothing
else -- could not count it. CI runs the coverage gate with `--offline`, the
arm that would have reported the gap, so nothing said so for two days.

Criteria are transcribed from the issue body without edit. Where the body's
wording is loose it is LEFT loose rather than tightened here: sharpening a
criterion inside the ledger is how a criterion quietly becomes an easier
adjacent property. Whoever takes this restates it on the ISSUE first.
