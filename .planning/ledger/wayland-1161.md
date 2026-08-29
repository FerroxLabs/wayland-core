---
issue: 1161
repo: FerroxLabs/wayland
kind: defect
title: "--resume mints a new conversation_id, breaking Flux sticky routing and fragmenting the cache ledger"
status: closed
last_verified_commit: 3262536a
criteria:
  - id: c1
    text: "Resuming a session restores the persisted conversation id instead of minting a new one"
    state: met
    evidence: "file:crates/wcore-agent/src/engine.rs:4875"
    owner: core
    note: "resume_with_provider_parts reads the persisted id; line-anchored because the change is a statement, not a named item"
  - id: c2
    text: "A resume appends to the existing cache ledger rather than starting a second one"
    state: met
    evidence: "test:crates/wcore-agent/tests/cache_ledger_engine_test.rs::resuming_a_session_appends_to_its_ledger_rather_than_starting_a_new_one"
    owner: core
    note: "asserts the ledger FILE COUNT stays 1, so a fix that keys correctly but still starts a fresh ledger fails it"
  - id: c3
    text: "The spend-audit trail is keyed by the same durable conversation id, so a resume files under the key the first launch used"
    state: met
    evidence: "test:crates/wcore-agent/tests/spend_governance_test.rs::a_resumed_session_files_its_spend_under_the_key_the_first_launch_used"
    owner: core
    note: "the same unjoinable-ledger defect one crate over, unfixed by c1/c2: install_spend_guard was called with `&uuid::Uuid::new_v4().to_string()` at BOTH constructors, so every SpendAuditRecord.session_id was minted per engine construction and persisted nowhere. Red arm measured two different random keys for one conversation. The test also asserts the audit key equals the cache ledger's, so the two durable records of a conversation can actually be joined -- and that task ids still differ, so this is a join and not a collapse"
  - id: c4
    text: "A mid-session `/model` rebind does not re-key the audit trail"
    state: met
    evidence: "test:crates/wcore-agent/tests/spend_governance_test.rs::a_model_rebind_does_not_split_one_conversation_into_two_audit_sessions"
    owner: core
    note: "rebind_provider passed budget_session_id() while the constructors passed a random uuid, so one uninterrupted session appeared in spend-audit.jsonl as two unrelated ones -- red arm measured the key set {<uuid>, session-unknown}. All three install sites now pass conversation_id, and sync_spend_audit_identity re-points the guard per turn so a session switch or checkpoint restore cannot strand it"
---

Closed in v0.13.10. `--resume` used to mint a fresh `conversation_id`, which
broke Flux sticky routing (the router could no longer recognise the session)
and split one conversation's cache ledger across two files.

The guard test is the oracle the issue itself named. It does not assert the
id — it asserts that after a resume there is still exactly ONE ledger file.
An earlier version of this change keyed correctly and still overwrote the
first launch's rows; that test is what caught it.
