---
issue: 1173
repo: FerroxLabs/wayland
title: "CLI refuses to start against a keyless local endpoint despite having a self-hosted placeholder path"
status: closed
last_verified_commit: cfa89a9c
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
    state: superseded
    owner: core
    note: "the repro used a base_url ending /v1, which now starts but 404s because api_path appends another /v1. Carried by #1178; the e2e here uses the bare root so it structurally cannot catch it"
---

Closed in v0.13.10, with the caveat above recorded as an explicit unmet
criterion rather than buried in prose.

A keyless self-hosted endpoint now starts when three conditions hold at once:
the user declared the endpoint, it is genuinely self-hosted, and the
provider's wire has a keyless path. That relaxes the REQUIREMENT for a key;
it does not widen what counts as one, which is what the control proves.

The working invocation is `--base-url http://127.0.0.1:11434` — the bare
root, not `/v1`. Anyone copying the repro out of the ticket body will hit
#1178 immediately.
