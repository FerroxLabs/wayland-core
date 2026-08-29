---
issue: 1170
repo: FerroxLabs/wayland
kind: defect
title: "Journal snapshot round-trip test cannot detect an unthreaded LlmRequest field — 17 fields unasserted"
status: closed
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "The round-trip test destructures LlmRequest with no `..`, so a new field is a compile error rather than silent coverage loss"
    state: met
    evidence: "file:crates/wcore-agent/tests/session_journal_test/foundation_cases.rs:189:// as its default and the comparison fails."
    owner: core
  - id: c2
    text: "Every field is asserted BY VALUE, and the assertion itself fails if the expected value equals Default"
    state: met
    evidence: "symbol:crates/wcore-agent/tests/session_journal_test/foundation_cases.rs::assert_journaled_field"
    owner: core
    note: "a field asserted against its own default is a test that cannot fail"
  - id: c3
    text: "The live data-loss bug the gap was hiding is fixed: ContentBlock::Thinking.extra survives the journal"
    state: met
    evidence: "test:crates/wcore-agent/tests/session_journal_test/foundation_cases.rs::prepared_provider_request_snapshot_round_trips_every_request_field"
    owner: core
  - id: c4
    text: "Existing sessions' journal digests do not move — a thinking block with no metadata still encodes byte-identically"
    state: met
    evidence: "test:crates/wcore-agent/tests/session_journal_test/foundation_cases.rs::prepared_provider_request_snapshot_keeps_a_thinking_block_without_extra_byte_identical"
    owner: core
---

Closed in v0.13.10. The test gap was hiding a live bug, which is the reason
this is worth a ledger entry rather than a line in a changelog.

The journal encoded `ContentBlock::Thinking { thinking, .. }` and decoded it
with `extra: None`, so the provider reasoning signature never existed in the
persisted shape. Recovery re-dispatches the decoded request, so a
crash-recovered Gemini turn went back on the wire with its signature stripped
and the server rejected it. Recovery did not resume the turn; it failed it.

Seventeen of eighteen `LlmRequest` fields were unasserted, which is why
nothing caught it.
