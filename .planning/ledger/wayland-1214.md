---
issue: 1214
repo: FerroxLabs/wayland
kind: defect
title: "Static context/output arms are live for host-variable open-weights ids, and model_output_ceiling ignores the provider entirely"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The minimax-m* and deepseek-v4* arms either resolve per provider, or are removed under the rule host_variable_open_weights_stay_unknown already enforces for qwen"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D32, found while verifying wayland#1176). Nothing has been done. The measured finding, verbatim: Static context/output arms are live for host-variable open-weights ids, and `model_output_ceiling` ignores the provider entirely (`pub fn model_output_ceiling(_provider: &str, model: &str)`, limits.rs:38 -- the provider parameter is underscore-prefixed and unused). So the arm applies to EVERY host serving that id, not just the vendor. Measured against a live models.dev pull (2026-08-29, 4,425,612 bytes): `minimax-m2.5` resolves to (output 128,000, context 204,800) while digitalocean serves it at context 65,536, nebius caps output at 8,192, alibaba-coding-plan at 24,576, cloudferro-sherlock at 16,000, dinference at 32,000, tencent-coding-plan at 32,768. `deepseek-v4-pro` resolves to (384,000, 1,000,000) while deepinfra caps output at 16,384, frogbot at context 128,000 / output 8,192, crof and hpc-ai at ~128-131k output. Both directions of the #165 harm follow: `known_context_window` (compact.rs:455-461) returns the arm's window verbatim, so a run on digitalocean will not compact until 204,800 against a real 65,536 -- a 3.1x over-claim, the exact shape that killed a customer run at 178,336 tokens in #165; and per the freshness script's own words 'an arm REVOKES `should_omit_max_tokens`, so an over-claim is a hard 400 mid-run' (confirmed at engine.rs:1204-1215, which omits the wire field only when `model_output_ceiling(...).is_none()`)."
  - id: c2
    text: "The passthrough module doc's claim 'No open-weights family is listed here' is either true or removed"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D32). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "PASSTHROUGH_IN_SCOPE's minimax/deepseek floor patterns no longer instruct a release owner to add an arm for a family whose hosts disagree"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D32). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c4
    text: "The host spread measured here is reproduced against a fresh models.dev pull and recorded, so the decision is made on data rather than on the arms that happen to be present"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D32). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

Static context/output arms are live for host-variable open-weights ids, and `model_output_ceiling` ignores the provider entirely (`pub fn model_output_ceiling(_provider: &str, model: &str)`, limits.rs:38 -- the provider parameter is underscore-prefixed and unused). So the arm applies to EVERY host serving that id, not just the vendor. Measured against a live models.dev pull (2026-08-29, 4,425,612 bytes): `minimax-m2.5` resolves to (output 128,000, context 204,800) while digitalocean serves it at context 65,536, nebius caps output at 8,192, alibaba-coding-plan at 24,576, cloudferro-sherlock at 16,000, dinference at 32,000, tencent-coding-plan at 32,768. `deepseek-v4-pro` resolves to (384,000, 1,000,000) while deepinfra caps output at 16,384, frogbot at context 128,000 / output 8,192, crof and hpc-ai at ~128-131k output. Both directions of the #165 harm follow: `known_context_window` (compact.rs:455-461) returns the arm's window verbatim, so a run on digitalocean will not compact until 204,800 against a real 65,536 -- a 3.1x over-claim, the exact shape that killed a customer run at 178,336 tokens in #165; and per the freshness script's own words 'an arm REVOKES `should_omit_max_tokens`, so an over-claim is a hard 400 mid-run' (confirmed at engine.rs:1204-1215, which omits the wire field only when `model_output_ceiling(...).is_none()`).

**Where.** crates/wcore-config/src/limits.rs:306-314 (minimax arms) and the deepseek-v4 arm near :265; crates/wcore-config/src/limits/passthrough.rs (the minimax-m2/m2.1/m2.5/m2.7/m3 and deepseek-v4-pro/flash rows, plus the module-doc claim 'No open-weights family is listed here', which is false); scripts/check-model-limits-freshness.py:156 (PASSTHROUGH_IN_SCOPE minimax/deepseek floor patterns); contrast crates/wcore-config/src/limits/catalogue.rs:433 `host_variable_open_weights_stay_unknown`, which enforces the opposite policy for qwen.

**Why it matters.** AGENTS.md's third model-limits rule exists precisely to prevent this, and the codebase already enforces it for qwen with the stated threshold 'anything at or above the 200,000 CompactConfig default makes the small hosts WORSE than the status quo' -- minimax-m2.5's arm is 204,800, above DEFAULT_CONTEXT_WINDOW (200,000). The arms predate #1176, but #1176 now locks them in with a per-PR Rust equality assertion and grades them only against vendor-operated endpoints, which by construction cannot observe the host spread -- so the new guard certifies the violation as green. The forward-looking half is worse: the in-scope patterns are documented floors, so the release gate will redden on a future minimax-m4 or deepseek-v5 and instruct the release owner to 'Add the arm if it has none' -- automating the creation of new violations. Closest existing ticket is #1157 (closed), which covered the opposite, under-claim direction for these same two families; the over-claim-on-third-party-hosts direction appears unfiled (`gh search issues` for qwen3.6-27b / open-weights / host-variable returned []).

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
