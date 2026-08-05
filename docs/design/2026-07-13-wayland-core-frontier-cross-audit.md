# Wayland Core Frontier Build Plan — Cross-Audit

> **Date:** 2026-07-13
> **Source baseline:** `112c91c03564d0d5fd2672dc0f76846bd8756a58`
> **Decision:** Conditional approval; blocker amendments applied to the canonical plan
> **Auditors:** Grok 4.5 (native, code-grounded), Fable 5 (independent plan review), Codex (code verification and reconciliation)
> **Canonical plan:** [2026-07-13-wayland-core-frontier-build-plan.md](2026-07-13-wayland-core-frontier-build-plan.md)

## 1. Audit routes and limits

| Route | Scope | Result | Evidentiary weight |
|---|---|---|---|
| Grok 4.5 native CLI | Read-only sandbox; full plan set and current checkout | Completed | Independent code-grounded review |
| Fable 5 through Flux | Canonical plan embedded; no repository or web access | Completed after the full three-document request timed out | Independent architecture, ordering and gate review; no code claims accepted from this lane |
| Codex | Current checkout, prior audits and reviewer findings | Completed | Reproduced high-severity claims and reconciled plan amendments |
| Fable 5 and Grok 4.3 through installed Wayland Core | Hermetic homes, JSON-stream mode, Flux aliases | Both emitted `ready` and then no audit events for more than six minutes; terminated | Dogfood reliability signal only; excluded from current-source conclusions because the installed binary is `0.12.12`, not the audited baseline |

The failed Core-routed runs do not prove a bug in the current checkout. They prove that the installed artifact was stale and could not produce a bounded review outcome. The amended F00 now rejects a source/binary mismatch, and F16 explicitly includes provider request/stream stalls.

## 2. Verdict

The plan is approved only for the first containment-and-proof slice. It was not safe to hand to several writing agents before amendment.

The architecture remains correct:

- finish and integrate rather than rewrite;
- Core enforces authority while Desktop controls the GUI experience;
- Smart Default, Managed and explicit Dangerous behavior remain separate;
- deterministic packaged-runtime evidence precedes live-model claims;
- crash completeness precedes True Continue;
- durable children precede gateway/product breadth;
- native cross-platform proof precedes frontier positioning.

The original plan had four blockers:

1. F06 emergency skill containment depended on the evaluation stack even though the first packet correctly wanted it immediately after F00.
2. F06 understated the active path: default memory can construct the legacy drafter, observation is ungated, in-process registration makes drafts model-visible, and the registry is process-global.
3. Evidence receipts were load-bearing without early integrity/provenance requirements, while hostile tests could run on the sole long-lived build host.
4. Supply-chain and release integrity had no owned task.

Those blockers are now addressed in the canonical plan.

## 3. Reproduced code findings

### 3.1 Autonomous-skill containment is broader than a configuration merge

- `want_memory` is true when either memory or skills lifecycle is enabled, and memory is enabled by default: `crates/wcore-agent/src/bootstrap.rs:1234`, `crates/wcore-config/src/config.rs:435-445`.
- Bootstrap installs the legacy `SkillDrafter` whenever the shared memory database exists, without a lifecycle check at that construction site: `crates/wcore-agent/src/bootstrap.rs:2331-2356`.
- `observe_auto_skill` has no `self.skills_lifecycle` guard before bucketing and drafting: `crates/wcore-agent/src/engine.rs:3867-3924`.
- The legacy drafter calls `register_bundled_skill` with `disable_model_invocation: false` and `user_invocable: true`: `crates/wcore-agent/src/auto_skill/drafter.rs:147-167`.
- The disk loader quarantines `needs_review` drafts, but the direct in-process registration bypasses that loader boundary: `crates/wcore-skills/src/loader.rs:411-418`.
- Bundled skills use a process-global registry: `crates/wcore-skills/src/bundled/mod.rs:51-62`.
- Project/global lifecycle merging is boolean OR, so project `false` cannot override a default-true global value: `crates/wcore-config/src/config.rs:3547-3549`.

Conclusion: the emergency fix must gate construction and observation, stop direct model-visible registration, quarantine drafts and prevent cross-session catalog pollution. Fixing the boolean merge alone is insufficient.

### 3.2 Process-global enforcement state is wider than egress

- Egress policy uses a global `OnceLock`: `crates/wcore-egress/src/policy.rs:66-91`.
- Sandbox configuration uses a global `OnceLock<RwLock<...>>`, and later calls replace the process-wide override: `crates/wcore-sandbox/src/lib.rs:58-87`.
- Bundled skill registration is global: `crates/wcore-skills/src/bundled/mod.rs:51-62`.

Conclusion: F09 must scope egress, sandbox configuration, skill catalog, MCP and related mutable host state per runtime/session. Parallel sessions with conflicting policy are the acceptance test.

### 3.3 The dependency contradiction was real

The original F06 required F00 and F02-F05, while the first execution packet scheduled it immediately after F00. F04 implementation also required F01-F03 while the packet omitted F03. Security containment must not wait for a complete evaluation system, but M0 closure still requires packaged proof.

Conclusion: F06 is now explicitly two-stage—emergency containment after F00, proof closure after F01-F05. F03 is in the first packet, and F04 starts with interface design only.

## 4. Reviewer convergence

| Finding | Grok 4.5 | Fable 5 | Codex decision |
|---|---|---|---|
| Plan is strong but conditional | Agree | Agree | Adopted |
| F06/first-packet dependency contradiction | Blocker | Blocker | Fixed |
| F06 threat model incomplete | Blocker, code-grounded | Repository verification required | Reproduced and expanded |
| Hostile tests must not run on trusted build host | Implicit in hermeticity concerns | High | Added disposable adversarial workers |
| Receipt integrity must begin before M5 | Supports strict proof | High | Added content addressing and CI provenance in F03 |
| Thresholds must be frozen before results | Supports non-gameable gates | High | Added versioned threshold manifest in F00/section 4.5 |
| Supply-chain/release integrity missing | Not a primary finding | High | Added F29 |
| Security resolver tasks cannot be multi-writer | Agree on mega-file collision | High | F07 -> F08 -> F09 is now serial under one owner |
| M4 breadth should not block the safety spine | Defer until M3 | Plan flags scope inflation | Retained after M3; F00 inventory prevents reimplementation |

## 5. Reviewer claims not adopted verbatim

Cross-audit is not majority voting. Several suggestions were narrowed or rejected:

- Grok requested adding pre-call budget reservation to F11. It was already present in the original F11 work contract. The plan keeps it and strengthens only the proof language elsewhere.
- Grok suggested a mandatory broad partition of the roughly 19,000-line engine before later work. The collision risk is real, but a standalone mega-refactor would violate the surgical-change rule and create another unproved critical path. F00 instead freezes interfaces and collision ownership; touched responsibilities should be extracted only when a bounded task requires it.
- Grok suggested removing F24-F27 from the program. They are already outside the M0-M3 serial safety spine and are required for standalone parity. They remain after M3, but F00 must inventory existing channel, cron, migration and service-adjacent code before M4 design.
- Fable treated numeric thresholds as wholly absent because it reviewed the canonical plan alone. The companion evaluation charter contains proposed numeric gates. The valid criticism is that the plan did not make pre-result calibration immutable, so F00 now freezes a versioned threshold manifest.
- Capability unavailability reporting remains useful during M0, but it cannot be used to declare an advertised capability complete. The plan now says wire it to an outcome or remove it from the advertised surface.

## 6. Amendments applied

1. Changed plan status to cross-audited and conditionally approved.
2. Added authoritative receipt integrity and source/binary mismatch rejection.
3. Separated trusted compilation from disposable adversarial execution.
4. Added a pre-result, versioned threshold manifest.
5. Expanded F00 inventory and shared-interface ownership.
6. Bound F03 receipts to source, binary, configuration, fixtures and CI provenance.
7. Split F06 into immediate containment and later packaged-proof closure.
8. Expanded F06 to cover memory-default construction, ungated observation, in-process model registration and global catalog state.
9. Added explicit dangerous-alias honesty to F07.
10. Expanded F09 to egress, sandbox overrides, bundled skills, MCP and provider-request redaction.
11. Added provider request/stream stalls to F16.
12. Required F24 to reconcile existing channel/cron/runtime code before implementation.
13. Strengthened native Windows debugging and soak evidence in F28.
14. Added F29 for dependency policy, SBOM, provenance, artifact/update signing, trust roots, key rotation and revocation.
15. Renumbered peer comparison/release review to F30 and made it depend on release integrity.
16. Serialized F07 -> F08 -> F09 and corrected the first execution packet.

## 7. Minimum safe first slice

The approved first slice is deliberately smaller than M0:

1. F00 captures the exact current behavior, collision map, inventories and threshold manifest.
2. F06 emergency containment stops the active legacy path immediately after characterization.
3. F01 runs one real packaged-binary scenario and returns honest exit status.
4. F02 removes ambient state and credentials from argv/TOML and owns cleanup.
5. F03 emits one integrity-bound receipt before broader reporter work.
6. F04 freezes fixture interfaces; implementation waits for the receipt schema.

Exit proof:

- every lifecycle-false configuration produces no draft, disk promotion, bundled registration or model-visible generated skill;
- lifecycle-true generated content remains quarantined;
- parallel sessions do not share generated catalog state;
- the evaluated binary proves it matches the intended source commit;
- one Smart Default packaged scenario completes without YOLO and emits a provenance-bound, redacted receipt;
- stale, missing, skipped, malformed or mismatched evidence fails rather than turning green.

## 8. Final decision

Proceed with the minimum safe first slice under one lead integrator, one assurance builder and one independent reviewer. Do not start journal, durable-child, gateway or Desktop implementation until M0 provides trustworthy packaged-runtime proof.

The plan is now suitable as the architectural source of truth. GitHub issues remain the live ownership state, and evidence receipts remain the only proof that a task actually works.
