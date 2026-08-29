---
issue: 314
repo: FerroxLabs/wayland-core
title: "grant_path, revoke_path and grant_workspace_capability are missing from the published desktop contract schema"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "Every ProtocolCommand variant is published with a schema branch and a fixture, enforced by an exhaustive match that fails to compile when a variant is added"
    state: met
    evidence: "test:crates/wcore-protocol/tests/desktop_contract_corpus.rs::every_protocol_command_variant_is_published_with_a_fixture_and_a_schema_branch"
    owner: core
    note: "the schema now carries 29 oneOf branches including all three grant/revoke commands, parsed as JSON rather than substring-matched"
  - id: c2
    text: "The three command fixtures exist on disk in the published corpus and are digest-pinned in the manifest"
    state: met
    evidence: "file:crates/wcore-protocol/contracts/desktop/v1/commands/grant_path.json"
    owner: core
    note: "revoke_path.json and grant_workspace_capability.json sit beside it; manifest.json is at contract 1.22 with counts.commands 29 and sha256 wire_shapes entries, so a hand-edit moves the digest"
  - id: c3
    text: "A corpus whose command specs and producer union disagree cannot be generated, in either direction"
    state: met
    evidence: "symbol:crates/wcore-protocol/src/contract/generate.rs::generated_artifacts"
    owner: core
    note: "this is the parity gate the event direction already had; #314 is what extended it to commands, so the same hole cannot reopen silently"
  - id: c4
    text: "A refused grant_path emits the workspace_policy receipt the published docs promise after any grant"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::path_grant_refused_by_policy_still_emits_the_policy_receipt"
    owner: core
    note: "d3660467. emit_workspace_policy_receipt now precedes the refusal Info on all FOUR refusal exits (main.rs:4215 launcher-refused, :4243 policy-refused, plus the two capability-grant arms). Four refusal tests plus two must-pass-in-both-arms controls. The emitters now take &dyn ProtocolEmitter, which is what made them testable at all. docs/json-stream-protocol.md:982-990 states the every-exit rule and tells hosts not to read an absent receipt as a refusal."
  - id: c5
    text: "A grant refusal is machine-readable rather than untyped English prose in an Info frame"
    state: blocked
    owner: maintainer
    note: "STALE NOTE CORRECTED 2026-08-29: the previous note pointed at FerroxLabs/wayland#1099, which is CLOSED and is a different subject (an escalation prompt for an out-of-workspace path), so there is NO open cross-lane thread carrying this. Refusals are still untyped English prose - main.rs:4218 and :4246 emit ProtocolEvent::Info with an empty msg_id, no grant_id echo, no machine-readable reason. Q4 in .planning/DECISIONS.md takes the decision (yes, as a contract minor bump with Desktop); what is now owed is a NEW Desktop-facing issue to carry it."
---

The headline claim - three grant and revoke commands missing from the published
desktop contract schema - was true at v0.13.4 and is false at v0.13.10. All
three are published with schema branches, fixtures and digest entries, and two
independent gates now make the same gap impossible to reopen quietly: a
generator that refuses a corpus whose specs and producer union disagree, and a
test driven by an exhaustive match over the command enum, so a new variant does
not compile until someone maps it.

Two of the reporter's sub-claims are also settled: the stale F-005 doc comment
is gone, and the revoke_path receipt asymmetry is deliberate, documented and
correct.

What did not travel with the fix is the refusal side. The docs tell hosts to
wait for a workspace_policy receipt after any grant and then no receipt arrives
when the grant is refused, which is the same published-contract-versus-wire
contradiction #314 was about, just in prose instead of JSON Schema. These three
emitters have zero test coverage of any kind, so for c4 the missing test is the
deliverable and the one-line receipt move is secondary.

Criteria come from the cluster D verification note of 2026-08-29.
