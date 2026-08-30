---
issue: 1161
repo: FerroxLabs/wayland
kind: defect
title: "--resume mints a new conversation_id, breaking Flux sticky routing and fragmenting the cache ledger"
status: closed
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "Resuming a session restores the persisted conversation id instead of minting a new one"
    state: met
    evidence: "file:crates/wcore-agent/src/engine.rs:5381:let resumed_conversation_id = session"
    owner: core
    note: "RE-ANCHORED 2026-08-30 for wayland#1198: was engine.rs:4875, which had drifted onto a skills_lifecycle caching comment unrelated to this criterion; the resume path reads the persisted id at :5041 and threads it at :5265. resume_with_provider_parts reads the persisted id; line-anchored because the change is a statement, not a named item"
  - id: c2
    text: "A resume appends to the existing cache ledger rather than starting a second one"
    state: met
    evidence: "test:crates/wcore-agent/tests/cache_ledger_engine_test.rs::resuming_a_session_appends_to_its_ledger_rather_than_starting_a_new_one"
    owner: core
    note: "asserts the ledger FILE COUNT stays 1, so a fix that keys correctly but still starts a fresh ledger fails it"
---

Closed in v0.13.10. `--resume` used to mint a fresh `conversation_id`, which
broke Flux sticky routing (the router could no longer recognise the session)
and split one conversation's cache ledger across two files.

The guard test is the oracle the issue itself named. It does not assert the
id — it asserts that after a resume there is still exactly ONE ledger file.
An earlier version of this change keyed correctly and still overwrote the
first launch's rows; that test is what caught it.
