---
issue: 174
repo: FerroxLabs/wayland
title: "[Feature]: prevent runaway token spend with budget guards and spend audits"
status: open
last_verified_commit: 7ee8f90a
criteria:
  - id: c1
    text: "Budget presets expand into concrete caps that reach the engine"
    state: met
    evidence: "test:crates/wcore-config/tests/budget_preset_test.rs::tiny_preset_reaches_the_engine_as_tiny_limits"
    owner: core
    note: "the sibling large-preset test in the same file pins the other end of the range"
  - id: c2
    text: "A per-task spend audit record is produced after every task"
    state: met
    evidence: "test:crates/wcore-agent/tests/spend_governance_test.rs::every_task_writes_its_own_audit_record_even_with_no_mode_configured"
    owner: core
    note: "Built on the existing wcore-budget ledger, not beside it: SpendAuditor accumulates, JsonlSpendAuditSink persists under the same cross-process lock as DailySpendStore, and AgentEngine::emit_task_spend_audit fires from a wrapper around the real run_with_content body so every terminal path emits exactly once (finish is idempotent). Metering is the SpendGuardProvider stream tap (symbol:crates/wcore-agent/src/spend_guard.rs::charge_usage), NOT the budget reservation - the e2e test caught that a session with no cap takes no reservation and produced a record with zero dispatches, and that the reservation never covered compaction at all. RED ARM observed: dropping the decorator wrap in install_spend_guard reddens this test with 'assertion `left == right` failed: the settled provider dispatch must be charged to the task record'. Known limit: cost comes from the wcore-pricing catalog only, so a model priced solely by the ProviderCompat heuristic records cost_usd null and lands in unpriced_dispatches, never silently as $0."
  - id: c3
    text: "A no-paid-models mode exists and is enforced"
    state: met
    evidence: "test:crates/wcore-agent/tests/spend_governance_test.rs::no_paid_mode_refuses_a_metered_model_at_the_engine"
    owner: core
    note: "[budget] mode = no-paid resolves to symbol:crates/wcore-budget/src/spend.rs::SpendPolicy, enforced at provider dispatch by symbol:crates/wcore-agent/src/spend_guard.rs::SpendGuardProvider, which returns ProviderError::NotAttempted before the inner provider is reached - the test asserts the inner call count is 0, so an advisory mode cannot pass it. An UNPRICED model is refused too (test:crates/wcore-budget/src/spend.rs::no_paid_refuses_metered_and_unpriced_but_admits_free_and_local); treating 'no price found' as free is how a router alias walks through this mode. Layer merge is strictest-wins so a repo-local config cannot relax a machine-owner mode. RED ARM observed: making SpendPolicy::admit return Ok unconditionally fails this test and the local-only one with 'local-only must refuse a hosted model BEFORE the provider is reached; an advisory mode would have let this through'."
  - id: c4
    text: "A local-only mode exists and is enforced"
    state: met
    evidence: "test:crates/wcore-agent/tests/spend_governance_test.rs::local_only_stops_a_hosted_model_before_the_provider_is_reached"
    owner: core
    note: "local-only is strictly stronger than no-paid: a FREE hosted model is still refused, because a free model still ships the conversation off the machine (test:crates/wcore-budget/src/spend.rs::local_only_refuses_even_a_free_hosted_model). Local inference still runs - test:crates/wcore-agent/tests/spend_governance_test.rs::a_local_model_still_runs_under_local_only_and_is_audited asserts the dispatch DOES reach the provider, so the mode cannot pass by refusing everything. symbol:crates/wcore-agent/src/spend_guard.rs::classify_model puts the ollama: prefix ahead of the catalog so local-only does not depend on a pricing row existing. Same two red arms as c3."
  - id: c5
    text: "Silent model escalation is blocked and every escalation reason is durably recorded"
    state: met
    evidence: "symbol:crates/wcore-budget/src/spend.rs::EscalationGate"
    owner: core
    note: "The gate holds the run authorized price ceiling: a move up it with no recorded reason is refused, a move down is free so the routing tier swap and cheap compaction model keep working, an UNPRICED target always escalates because its rate reads 0.0 and a numeric comparison alone would call it a downgrade (test:crates/wcore-budget/src/spend.rs::an_unpriced_target_escalates_even_though_its_rate_reads_zero), and a blank reason is refused rather than stored. Call sites that can move the model, each gated: the routing tier swap, the skill/hook apply_switch_model (admit only - a hook cannot supply an operator reason), AgentEngine::set_model (authorize plus durable record, proven by test:crates/wcore-agent/tests/spend_governance_test.rs::an_operator_model_change_is_recorded_and_a_forbidden_one_is_refused), rebind_provider, and the configured provider fallback, which needed its OWN gate because ResilientProvider swaps provider AND model inside its own stream below the engine guarded handle. An escalation whose durable write fails is reverted rather than left in force (symbol:crates/wcore-budget/src/spend.rs::EscalationGate::revert_last_authorization). Coverage is graded by census: test:crates/wcore-agent/tests/spend_governance_test.rs::every_production_provider_dispatch_site_is_named_and_assigned_a_guard walks every crate src/ and fails on any unlisted .stream( site. RED ARM observed: it found 16 sites the first draft had missed - 'a NEW provider dispatch site appeared and has not been assigned a spend guard'. RE-VERIFIED INDEPENDENTLY at 7ee8f90a, all four red arms re-run rather than taken from the note above: (a) deleting one real entry from EXPECTED_DISPATCH_SITES reddens the census with 'a NEW provider dispatch site appeared and has not been assigned a spend guard ... [\"wcore-providers/src/resilient.rs::fallback.provider\"]'; (b) making SpendPolicy::admit return Ok unconditionally reddens BOTH mode tests, c4 with 'local-only must refuse a hosted model BEFORE the provider is reached; an advisory mode would have let this through'; (c) dropping the SpendGuardProvider wrap in install_spend_guard reddens the c2 audit test with 'the settled provider dispatch must be charged to the task record'. DEFECT FOUND AND FIXED while closing the gate: is_escalation guarded the Unpriced TARGET (rule 2) but compared an Unpriced BASELINE by its placeholder 0.0 rate, so for any session whose configured model has no pricing row EVERY catalogued model read as an escalation above it - the configured provider fallback and the smart-routing tier swap were both refused and a pre-send primary failure took the whole turn down with MaxTurns (caught by the pre-existing test:crates/wcore-agent/src/engine.rs::audit_2026_05_22_tests::missing_key_releases_primary_reservation_before_fallback, which the first draft reddened). Rule 3 now admits above an unknown ceiling; SpendPolicy is checked first and independently so no-paid and local-only are untouched, pinned by test:crates/wcore-budget/src/spend.rs::an_unpriced_baseline_is_not_a_zero_ceiling_that_refuses_every_priced_model, whose own RED ARM (removing the rule-3 branch) fails it with 'anthropic/opus must not read as an escalation above an unpriced baseline'. Same class also fixed in the shared test_spend_guard helper, whose Free baseline was a real ceiling that refused the hook switch_model plumbing tests. The desktop contract corpus was regenerated: engine.rs is a SOURCE_INPUTS file, schema_digest is unchanged at sha256:47e255b8...da6f and every touched corpus file differs only in its embedded digest fields. KNOWN LIMIT this creates, stated rather than hidden: with an Unpriced baseline the gate cannot RANK models at all, so an upward move is admitted instead of refused and c5 degrades to the mode policy for that session class. It does not go unrecorded - every dispatch lands in the durable per-task audit with its provider and model (symbol:crates/wcore-budget/src/spend_audit.rs::SpendAuditDispatch), so the move is visible, just not blocked. The alternative, treating an unreadable price as $0.00 and refusing everything above it, is the false precision that caused the outage above."
---

The canonical Token Spend Governance tracking issue. It asks for per-task,
per-agent and per-model budgets with soft warnings, hard stops, an escalation
approval gate, a retry circuit breaker, a live spend meter and a per-task spend
audit, plus presets including local-only and no-paid-models.

A large part landed. The engine mechanisms shipped in 0.12.6 — the routing-tier
swap that actually dispatches and bills the cheap model, cheap and
usage-accurate compaction, a bounded retry re-bill, cache hygiene — after a
ten-angle investigation that also refuted several alarmist candidates. Presets
reach the engine, usage is visible during a run and the numbers on the paths
that used to lie are now true, and repeated provider failures trip a circuit
breaker in `wcore-providers`.

Four of the issue's eleven acceptance bullets were open, and the criteria above
are those four plus the one landed item they are most easily confused with. Each
was checked by name against the tree rather than taken from the status comment:
`spend_audit`, `no_paid` and `local_only` returned nothing. The remaining work
split cleanly into the audit surface, the two modes, and the escalation gate.

All four are now closed, in `wcore-budget` (`spend.rs`, `spend_audit.rs` — the
decision tables, dependency-free) and `wcore-agent` (`spend_guard.rs` — the
enforcement, where pricing and the live provider are reachable). They build on
the existing cost ledger rather than adding a second one: `SpendAuditRecord`
sits beside `DailySpendStore` under the same lock discipline, and `[budget]
mode` is a field on the existing `BudgetConfig`.

The design decision worth carrying forward is that enforcement is a PROVIDER
DECORATOR, not a check at each dispatch site. `install_spend_guard` is the one
place the engine installs a provider handle, so the conversation turn, the
compaction call and the online-evolution paraphrase are all admitted without
any of them asking. Exactly two sites escape it — `ResilientProvider`'s
configured fallback and `ProviderChain`'s next slot, both of which change
provider AND model inside their own `stream()` — and both funnel through
`retry::admit_configured_fallback`, which the engine's admitter gates. That
claim is not asserted, it is CENSUSED: a test walks every crate's `src/` and
fails when an unlisted `.stream(` site appears. It found sixteen on its first
run.
