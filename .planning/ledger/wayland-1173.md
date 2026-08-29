---
issue: 1173
repo: FerroxLabs/wayland
kind: defect
title: "CLI refuses to start against a keyless local endpoint despite having a self-hosted placeholder path"
status: closed
last_verified_commit: ca211126
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
    note: "Successor wayland#1178 verified OPEN, so the handover is structurally valid, and #1178's own criteria are now all met in this tree: de90aae2 added wcore_config::compat::join_endpoint and routed openai.rs:660, openai.rs:798 and compat.rs:1150 through it, with negative controls for a substring match and for an authority literally named v1. c3 is still NOT met HERE: this ticket's own e2e (keyless_local_endpoint_e2e.rs:199-200) still drives the bare root and structurally cannot catch the doubled path, and the verbatim keyless-plus-/v1 repro has no test. The grading of the fix belongs to #1178."
  - id: c4
    text: "The locality predicate cannot be talked into calling a public host self-hosted, whatever the URL spelling"
    state: met
    evidence: "test:crates/wcore-config/tests/keyless_self_hosted_endpoint_test.rs::a_smuggled_authority_does_not_exempt_a_public_endpoint"
    owner: core
    note: "Raised by the close-sweep as D29 and OBSERVED, not modelled: with every credential var unset, --base-url \"https://api.openai.com?x=@127.0.0.1\" did not emit \"No API key found\" -- it started and made a live request to api.openai.com. is_self_hosted_base_url took the authority as everything before the first '/', so a query string, a fragment or a WHATWG backslash left the private literal for rsplit('@') to find, and c2's control passed because its public-host list had none of those spellings. The authority now ends at the first of '/', '?', '#' or '\\'. The gate-level test asserts the REFUSAL for all three spellings; the predicate-level list in the_locality_predicate_rejects_public_hosts carries them too."
  - id: c5
    text: "The keyless self-hosted exemption is applied at every credential gate, not just the CLI's"
    state: met
    evidence: "test:crates/wcore-config/tests/keyless_self_hosted_endpoint_test.rs::the_council_gate_honours_the_same_keyless_self_hosted_exemption"
    owner: core
    note: "Raised by the close-sweep as D30. There are exactly two production credential gates -- Config::resolve (config.rs:2683) and resolve_council_provider (config.rs:3589); every other resolve_api_key caller uses unwrap_or_default and refuses nothing. The council gate re-implemented the chain without the exemption, so a member pointed at a keyless local Ollama was classified CouncilProviderError::Keyless and dropped before spawn while the CLI ran the same endpoint happily. Negative control the_council_gate_still_skips_every_non_exempt_keyless_member holds all three conditions at the council gate (public host, provider default endpoint, a wire with no keyless path, explicit opt-out) and passes in both arms."
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
