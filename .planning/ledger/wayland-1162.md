---
issue: 1162
repo: FerroxLabs/wayland
kind: defect
title: "cache report --session rejects the session id the user set; ledger is keyed by the internal conversation UUID"
status: closed
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "A user-chosen session id resolves against a ledger keyed by the internal conversation UUID"
    state: met
    evidence: "file:crates/wcore-cli/src/cache_cmd.rs:440"
    owner: core
    note: "cache_cmd::resolve tries the ledger key first, then bridges via Session::conversation_id"
  - id: c2
    text: "The resolution is verified through the real binary, not the library"
    state: met
    evidence: "test:crates/wcore-cli/tests/cache_ledger_cli.rs::a_user_chosen_session_id_resolves_to_the_ledger_keyed_by_its_conversation_id"
    owner: core
---

Closed in v0.13.10. The ledger is keyed by an internal conversation UUID, so
`cache report --session <the id you set>` rejected the only id the user knows.
`resolve` now tries the ledger key first and falls back to bridging through
`Session::conversation_id`, and the end-to-end test drives the shipped binary
rather than calling the library directly.
