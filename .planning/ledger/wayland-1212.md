---
issue: 1212
repo: FerroxLabs/wayland
kind: defect
title: "The #1173 keyless self-hosted exemption is applied at one credential gate and not at resolve_council_provider"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "A keyless self-hosted council member is not classified CouncilProviderError::Keyless"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D30, found while verifying wayland#1173). Nothing has been done. The measured finding, verbatim: The #1173 keyless-self-hosted exemption is applied at only ONE of the two credential gates. `resolve_council_provider` re-implements the same chain and does not consult `declared_keyless_self_hosted_endpoint`, so a keyless self-hosted council member is classified `CouncilProviderError::Keyless` and dropped before spawn -- the same configuration that the main CLI path now runs happily."
  - id: c2
    text: "The two gates consult ONE predicate rather than two re-implemented chains, or a test asserts they agree on identical config"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D30). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "A test drives a council member at a keyless self-hosted endpoint and asserts it is not skipped; shown RED against today's code"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D30). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The #1173 keyless-self-hosted exemption is applied at only ONE of the two credential gates. `resolve_council_provider` re-implements the same chain and does not consult `declared_keyless_self_hosted_endpoint`, so a keyless self-hosted council member is classified `CouncilProviderError::Keyless` and dropped before spawn -- the same configuration that the main CLI path now runs happily.

**Where.** crates/wcore-config/src/config.rs:3589-3607 (the `Err(_) => ... return Err(CouncilProviderError::Keyless(...))` arm) versus the exemption at config.rs:2683-2686 / :2751; the skip is consumed at crates/wcore-agent/src/orchestration/council/run.rs:183-190 (`skipped.push(SkippedProposer{...})`) and crates/wcore-agent/src/orchestration/council/resolver.rs:111.

**Why it matters.** Class-closure gap for this ticket: the guard is unit-tested and graded at one entry point while a second, ungraded entry point makes the opposite decision on identical config. A user who points a council member at a local Ollama with no key loses that member. Mitigating: the drop is recorded as a `SkippedProposer` with a reason rather than being wholly silent, and `all_keyless_proposers_skipped_yields_insufficient` (run.rs:565) means an all-local council fails loudly. Outside #1173's stated symptom ('CLI refuses to start'), so it does not block the close, but it should be tracked.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
