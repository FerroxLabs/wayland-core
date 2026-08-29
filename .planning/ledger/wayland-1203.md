---
issue: 1203
repo: FerroxLabs/wayland
kind: defect
title: "The spend-audit trail is keyed by a throwaway UUID minted per engine construction, then swapped for the real session id on a /model rebind"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "install_spend_guard receives the authoritative budget_session_id() at all three call sites, not uuid::Uuid::new_v4() at two of them"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D21, found while verifying wayland#1161). Nothing has been done. The measured finding, verbatim: The spend-audit trail is keyed by a throwaway UUID that is minted fresh on every engine construction, never persisted, never restored on resume -- and then silently swapped for the REAL session id mid-session on a `/model` rebind. This is the exact defect class #1161 just closed for the cache ledger, still open one crate over in the spend ledger. `install_spend_guard` is called with `&uuid::Uuid::new_v4().to_string()` as its `session_id` at BOTH construction sites -- engine.rs:4758-4765 (`new_with_provider`) and engine.rs:5031-5038 (`resume_with_provider_parts`, three lines above the #1161 fix itself). `SpendGuard::new` (spend_guard.rs:137-157) hands that string straight into `SpendAuditor::new(...)` and `EscalationGate::new(session_id, ...)`, and it lands verbatim as `SpendAuditRecord.session_id` (wcore-budget/src/spend_audit.rs:70, populated at :310 and copied at :372), which the `JsonlSpendAuditSink` appends to `~/.wayland/budget/spend-audit.jsonl` (path built by `spend_audit_log_path`, engine.rs:4701-4705). The give-away that the random uuid is not intended is the third call site: `rebind_provider` (engine.rs:6706-6713) passes `&self.budget_session_id()` -- the authoritative id (engine.rs:5942-5950, authority id -> `budget_session_id` -> `current_session_id()`)."
  - id: c2
    text: "A test asserts the SpendAuditRecord.session_id written on a fresh construction, on a resume, and after rebind_provider are the SAME id"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D21). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "A live run is quoted: one conversation's records in ~/.wayland/budget/spend-audit.jsonl share one key across a /model switch and across a --resume"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D21). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The spend-audit trail is keyed by a throwaway UUID that is minted fresh on every engine construction, never persisted, never restored on resume -- and then silently swapped for the REAL session id mid-session on a `/model` rebind. This is the exact defect class #1161 just closed for the cache ledger, still open one crate over in the spend ledger. `install_spend_guard` is called with `&uuid::Uuid::new_v4().to_string()` as its `session_id` at BOTH construction sites -- engine.rs:4758-4765 (`new_with_provider`) and engine.rs:5031-5038 (`resume_with_provider_parts`, three lines above the #1161 fix itself). `SpendGuard::new` (spend_guard.rs:137-157) hands that string straight into `SpendAuditor::new(...)` and `EscalationGate::new(session_id, ...)`, and it lands verbatim as `SpendAuditRecord.session_id` (wcore-budget/src/spend_audit.rs:70, populated at :310 and copied at :372), which the `JsonlSpendAuditSink` appends to `~/.wayland/budget/spend-audit.jsonl` (path built by `spend_audit_log_path`, engine.rs:4701-4705). The give-away that the random uuid is not intended is the third call site: `rebind_provider` (engine.rs:6706-6713) passes `&self.budget_session_id()` -- the authoritative id (engine.rs:5942-5950, authority id -> `budget_session_id` -> `current_session_id()`).

**Where.** crates/wcore-agent/src/engine.rs:4764 and :5037 (`install_spend_guard` session_id argument), against crates/wcore-agent/src/engine.rs:6712 which passes the real `budget_session_id()`; consumed at crates/wcore-agent/src/spend_guard.rs:143-153 and crates/wcore-budget/src/spend_audit.rs:302-310.

**Why it matters.** Three concrete consequences, all of them accounting the operator cannot repair after the fact: (1) the spend-audit records for one conversation cannot be grouped -- a fresh launch and each `--resume` write under different random keys, so a session's true authorized spend can never be totalled from the log; (2) within a SINGLE session, a `/model` switch moves subsequent records from the random uuid to the real session id, so one uninterrupted session appears in the log as two unrelated sessions; (3) `EscalationGate` is constructed with the same random id, so any per-session escalation reasoning is scoped to an identity that does not survive a resume. Scope of my claim, stated honestly: I verified this by reading every call site and the record-construction chain; I did not run a live session and inspect `spend-audit.jsonl`, so the wiring is proven and the on-disk symptom is inferred from it. Not caused by the #1161 fix -- it is symmetric on the fresh and resume paths and predates it -- but it is the same unfiled defect the #1161 investigation walked straight past.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
