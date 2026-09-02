---
issue: 1173
repo: FerroxLabs/wayland
kind: defect
title: "CLI refuses to start against a keyless local endpoint despite having a self-hosted placeholder path"
status: closed
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "A keyless self-hosted endpoint the user declared starts and dispatches, with the placeholder bearer pinned on the wire"
    state: met
    evidence: "test:crates/wcore-cli/tests/keyless_local_endpoint_e2e.rs::keyless_local_endpoint_starts_and_dispatches_with_the_placeholder_bearer"
    owner: core
    note: "pins the Authorization header actually sent, not the config that produced it"
  - id: c2
    text: "A remote endpoint without a key is still refused — the relaxation does not widen what counts as a key"
    state: met
    evidence: "test:crates/wcore-cli/tests/keyless_local_endpoint_e2e.rs::a_remote_endpoint_without_a_key_is_still_refused"
    owner: core
    note: "control: passes in both arms, so it proves the relaxation is bounded rather than that the test suite is green"
  - id: c3
    text: "The ticket's VERBATIM repro invocation works, /v1 suffix and all"
    state: met
    evidence: "test:crates/wcore-cli/tests/keyless_local_endpoint_e2e.rs::the_verbatim_repro_starts_and_dispatches_with_the_v1_suffix"
    owner: core
    note: "RE-GRADED 2026-08-31 (lane/f13-w3-win-honesty), replacing a note that refuted itself. The earlier flip from superseded to met moved this only because the successor wayland#1178 had CLOSED and check-criteria-ledger.py:923-929 refuses a residual handed to a closed issue -- it silenced the gate rather than answering it. It also left standing, verbatim, `c3 is still NOT met HERE ... the verbatim keyless-plus-/v1 repro has no test`, and it anchored the criterion to compat.rs::join_endpoint_collapses_a_duplicated_v1_segment, a pure STRING test in a crate that cannot observe wcore-cli`s startup credential gate at all, so no regression in `the invocation works` could ever redden it. MET NOW BECAUSE THE TEST WAS WRITTEN, not because a successor closed. the_verbatim_repro_starts_and_dispatches_with_the_v1_suffix spawns the real binary with the ticket`s own --base-url <root>/v1, with every credential env var stripped from the child, and asserts the run exits 0, that the mock`s recorded request path is exactly [/v1/chat/completions], that every dispatch carries the SELF_HOSTED_PLACEHOLDER_KEY bearer, and that the model`s answer reaches stdout -- so both halves of c3, keyless AND /v1-suffixed, are driven jointly through the shipped path. MEASURED non-vacuous on both halves, restored and touched between arms, 27/27 green before and after: (a) `if false &&` on join_endpoint`s overlap loop (compat.rs:1385) makes the run post to /v1/v1/chat/completions, the mock answers 404, and THIS test FAILED while the bare-root positive keyless_local_endpoint_starts_and_dispatches_with_the_placeholder_bearer stayed GREEN -- exactly the gap the old string anchor could not see; (b) `false &&` on ProviderCompat::keyless_self_hosted (compat.rs:1178) produces `No API key found` and FAILED this test and the bare-root positive together. RESIDUAL DISPOSITION: the residual was genuinely DELIVERED by #1178 (wcore_config::compat::join_endpoint) and is now graded HERE against product code, so nothing is left handed to a closed issue."
---

Closed in v0.13.10, with the caveat above recorded as an explicit unmet
criterion rather than buried in prose.

A keyless self-hosted endpoint now starts when three conditions hold at once:
the user declared the endpoint, it is genuinely self-hosted, and the
provider's wire has a keyless path. That relaxes the REQUIREMENT for a key;
it does not widen what counts as one, which is what the control proves.

The working invocation was `--base-url http://127.0.0.1:11434` — the bare
root, not `/v1`. Anyone copying the repro out of the ticket body hit #1178.
That is fixed: `join_endpoint` collapses the duplicated segment, and as of
2026-08-31 the ticket's own `/v1` invocation is driven end to end by
`the_verbatim_repro_starts_and_dispatches_with_the_v1_suffix`, so c3 is met
as written rather than by an adjacent string property.
