# Wayland Core F00 Characterization Baseline

**Status:** static/source characterization and exact Linux release build complete; macOS and Windows runtime receipts pending.

**Frozen source:** `85881ce2299a8eb56b907e2446df0e57deed7e7f`

**Production release source:** `61b79c4f90f71fe2cf243affa7620b3c9b607f14` (`0.12.25`)

This document is the pre-change oracle for F06 and the inventory guard for later Frontier work. A file/line reference is evidence of the current source path, not runtime proof. Runtime claims remain open until an exact-source binary receipt reaches the evidence level named below.

## 1. Baseline identity

| Item | Frozen value | Result |
|---|---|---|
| Workspace version | `0.12.25` | Confirmed in the local workspace |
| Release-main commit | `61b79c4f90f71fe2cf243affa7620b3c9b607f14` | Confirmed |
| Documentation commit | `112c91c03564d0d5fd2672dc0f76846bd8756a58` | README/CHANGELOG only |
| Frontier-plan commit | `85881ce2299a8eb56b907e2446df0e57deed7e7f` | Current frozen source |
| Production-code delta, release-main to frozen source | None under `crates/`, `Cargo.toml`, or `Cargo.lock` | Confirmed with `git diff --quiet` |
| Workspace packages/members | 55 / 55 | `cargo metadata --no-deps --format-version 1` |
| `Cargo.toml` SHA-256 | `afaba8208bdf2143f105f5fc249e9ca871053b8dce42035bad6c19c1de665e93` | Frozen |
| `Cargo.lock` SHA-256 | `dbe4fb8189c88bd7565d9634d460299bb4fad31a7bc98e2c0b7a28f406bc6f3f` | Frozen |
| Gate thresholds | `crates/wcore-eval-scenarios/frontier-thresholds-v1.toml` | Frozen before candidate results |

Installed binaries were rejected as baseline evidence:

- `/opt/homebrew/bin/wayland-core` reports `0.12.12`.
- `/root/wayland/target/release/wayland-core` on `hetzner-dsm` reports `0.12.24` from a `0.12.24` checkout.

The exact frozen source was transferred without GitHub to an isolated Hetzner worktree at `/root/wayland-frontier-m0`. The pinned Linux receipt is:

| Receipt field | Frozen result |
|---|---|
| Source | `85881ce2299a8eb56b907e2446df0e57deed7e7f` |
| Version | `wayland-core 0.12.25` |
| Build info | `wayland-core 0.12.25 (source 85881ce2)` |
| Cargo | `cargo 1.95.0 (f2d3ce0bd 2026-03-21)` |
| Rust | `rustc 1.95.0 (59807616e 2026-04-14)` |
| Binary SHA-256 | `97c0df4290692f0b5794f6d30779745e1706014e22422afe55d0867edced6c67` |
| Binary kind | ELF 64-bit x86-64 PIE, dynamically linked |
| Host | Ubuntu 24.04, Linux 6.8.0-101-generic x86-64 |
| Receipt time | `2026-07-13T13:50:04Z` |

An earlier build that `vx` incorrectly ran under Rust 1.97 is explicitly rejected. The receipt above comes from an explicit `cargo +1.95.0 build --release -p wcore-cli` invocation.

### Baseline erratum discovered during F06 verification

The `0.12.25` release commit changed the workspace version in `Cargo.toml` but did not regenerate `Cargo.lock`. The frozen lock still described internal packages as `0.12.24` and omitted the `wcore-cli` `serde_yaml` dependency. A clean `cargo +1.95.0 check --locked` therefore failed before compilation because Cargo needed to update the lockfile.

The original lock digest and binary receipt above remain the historical baseline, but the binary receipt is rejected as dependency-lock provenance for Frontier candidate comparisons. Commit `32301e682bb457897812545d2afc317401247c45` regenerates the lockfile offline using Cargo 1.95.0. The corrected `Cargo.lock` SHA-256 is `f7b7922c992a7ee144373c1619a0d10d9d16f13345e9517a657713b355a9d2a6`; `cargo +1.95.0 check --locked --offline` passes on the isolated Hetzner worktree. All later evidence receipts must bind the corrected lock digest and a rebuilt binary.

## 2. Production activation map

| Capability | Construction or configuration | Production caller/outcome | Characterization |
|---|---|---|---|
| `PricingRefresher` | Defined in `crates/wcore-pricing/src/refresh.rs` | Repository-wide symbol search finds definition/tests only | Built but not wired |
| `CooldownTracker` | Defined in `crates/wcore-providers/src/cooldown.rs` | Repository-wide symbol search finds definition/tests only | Built but not wired |
| `MidFlightMonitor` | Defined in `crates/wcore-agent/src/orchestration/monitor.rs` | Integration/unit tests call it; no production construction found | Built but not wired |
| `LearnedPolicy` | Optional policy reaches orchestration types | `engine.rs` and `node_executor.rs` document production callers passing no learned policy | Built but not wired |
| Smart handoff | `compact.smart_handoff_to_memory` is enabled by default | `engine.rs` invokes the memory handoff on the smart-compaction path | Production path exists; deterministic outcome proof still required |
| New skill writer | Engine caches `skills_lifecycle` and calls `try_draft_skill_for_turn` | Stages through `DraftWriter`; returns immediately when lifecycle is false | Live and gated |
| Legacy skill drafter | Bootstrap installs `SkillDrafter` whenever the memory DB exists (`bootstrap.rs:2331-2356`) | `observe_auto_skill` is called on max-turn and normal terminal paths (`engine.rs:4192`, `6207`) without its own lifecycle check | Live, split, and insufficiently gated |
| Delegate isolation | `Delegate` creates `ForkOverrides` in `crates/wcore-tools/src/delegate.rs` | Overrides tool selection; no worktree/cwd isolation contract exists | Live delegation, no filesystem transaction isolation |

### Skill-drafting authority chain

The current high-risk chain is source-confirmed:

1. `MemoryConfig::default().enabled` is true (`wcore-config/src/config.rs:435-445`).
2. `want_memory = memory.enabled || skills_lifecycle` (`wcore-agent/src/bootstrap.rs:1234`).
3. A memory DB causes bootstrap to install the legacy `SkillDrafter` independently of the resolved lifecycle value (`bootstrap.rs:2331-2356`).
4. `observe_auto_skill` has no lifecycle guard and is called by real terminal paths (`engine.rs:3867-3924`, `4192`, `6207`).
5. `SkillDrafter::draft` writes loader-visible and legacy disk copies, records a `PromptStore` candidate, and directly registers a process-global bundled skill (`auto_skill/drafter.rs:97-178`).
6. The direct registration sets `disable_model_invocation = false` and `user_invocable = true` before human review.
7. Disk reload is safer: `wcore-skills/src/loader.rs:411-441` marks a `needs_review = true` draft non-model-invocable. That loader quarantine does not undo the direct in-process registration.
8. The bundled registry is a process-global `OnceLock<Mutex<Vec<_>>>` (`wcore-skills/src/bundled/mod.rs:51-76`). It is also used intentionally for plugin/bundled skill delivery, so F06 must not delete that general facility.
9. The legacy secondary disk copy contains `SKILL.md` but no manifest (`auto_skill/drafter.rs:128-141`). If that copy is migrated or discovered, its provenance and review state are lost.
10. Loader quarantine fails open: missing, unreadable, malformed, or flag-less manifests return `false` from `draft_needs_review` (`loader.rs:429-443`). A damaged or omitted draft manifest therefore makes generated content model-visible.
11. `disable_model_invocation` controls advertisement, not execution. `SkillTool` lists every catalog reference in errors and resolves a caller-supplied name directly (`skill_tool.rs:129-160`), so a model that knows or guesses a hidden name can invoke it.
12. `--skills-promote` changes the requested database row, then calls a migration whose `_procedure_id` is unused and which copies every legacy auto-draft missing from the loader directory (`wcore-cli/src/main.rs:2141-2223`). It neither binds the migrated artifact to the promoted procedure nor writes/clears a review manifest.

The combined behavior means the current system does not have one enforceable quarantine boundary. F06 must block legacy production drafting and promotion until a single governed lifecycle owns draft identity, persistence, visibility, invocation, promotion, and audit. Merely setting `disable_model_invocation` on generated metadata is insufficient.

## 3. Current authority, containment, and recovery behavior

| Surface | Existing strength | Current boundary or gap |
|---|---|---|
| Approval | Tokenized request/resume flow; `default`, `auto_edit`, and `force` modes; project config cannot loosen the global approval posture | `force` vocabulary is overloaded; typed Smart/Managed/Dangerous postures are F07 |
| Sandbox | Backend selection is centralized; unsandboxed fallback fails closed without the explicit opt-in; filesystem and symlink tests exist | Global mutable configuration and complete three-OS packaged proof remain open |
| Budget | Execution and tracker budgets, cancellation propagation, protocol events, and workflow limits exist | Default execution caps are `None`; reservation/overshoot and a finite smart default are F11 |
| Session recovery | Snapshot persistence plus a WAL recovery path exists | WAL records user-message text, not a crash-complete journal of tool calls, approvals, budgets, and irreversible effects (`session.rs:183-244`) |
| Provider recovery | Region failover, resilient-provider circuit behavior, and typed failover reasons have tests | `CooldownTracker` is dark; provider request/stream stall proof belongs to F16 |
| Children | Spawn lifecycle, cancellation propagation, budget attachment, relays, and graph merge have tests | Durable inspect/cancel/resume and worktree transaction isolation are not the ordinary child contract |
| Egress | Central policy boundary exists | Enforcement state is process-global; runtime/session scoping belongs to F09 |
| Anvil workspace | Leases, a recovery journal, network-denied verification gates, and a forge transaction exist | Gate allowlists expose the live workspace for read/write; the forge changes process-global cwd across an `await`; receipts are not trusted top-level host events |

Additional source checks sharpen later milestones:

- `ExecutionBudget` is constructed and monitored, but production searches found no calls to token, cost, tool-run, or agent accounting hooks; wall time is the only clearly connected cap. F11 must prove every cap at the real call site.
- The WAL flushes without `sync_all`, does not explicitly set owner-only permissions, silently skips malformed records, and deduplicates equal user text across the history. F12-F14 must replace these semantics before claiming crash durability.
- Production failover excludes a fallback on a different provider and cannot safely replay the current partial streamed turn. F16 must not describe current region/model failover as provider failover.
- Child agents share the parent filesystem and process cwd. Tool restriction is not filesystem transaction isolation.

## 4. Existing standalone-product inventory

Later work must extend these implementations, not create parallel stacks.

| Area | Existing implementation | Boundary recorded for later work |
|---|---|---|
| Channels | `wcore-channels`, ten adapter crates, `wcore-channels-registry`, and agent inbound/dispatch/media/send paths | Gateways live for the lifetime of an owning process; there is no unified install/start/stop/restart/status/logs/drain service surface |
| Cron | `wcore-cron`, `wcore-agent/src/cron.rs`, cron tools, evaluator scenarios, and `wayland-core cron ... daemon` | Some headless outcomes are explicitly `Staged`; daemon lifecycle is not an OS service manager |
| Migration | `wcore-cli/src/migrate/{mod,hermes}.rs` plus Hermes tests | Hermes import exists; OpenClaw and full persona/memory/skill parity do not |
| Anvil | `wcore-cli/src/anvil.rs` and `wcore-agent/src/orchestration/anvil/` implement the gated forge, journal, leases, ledger, climb, and gates | Keep Anvil's transactional worktree/gate contract distinct from ordinary Delegate until F20 unifies child isolation deliberately |
| Delegate | `wcore-tools/src/delegate.rs` and the dispatcher path are live | No ordinary per-child worktree/cwd transaction boundary |
| Service-adjacent | Channel gateways, cron daemon, ACP/A2A/REST host modes, inbound webhook, and Desktop JSON stream integration | These are process modes and side effects, not one coherent persistent Agent Gateway lifecycle |

## 5. Characterization-test inventory

The repository already has meaningful component and integration coverage. F00 does not treat test count as proof of the production call graph.

| Behavior | Existing representative coverage | Missing pre-F06/Future proof |
|---|---|---|
| Approval | `wcore-protocol/tests/approval_manager_test.rs`, `approval_resume_contract.rs`; agent approval round-trip, expiry, requester-crash, JSON-stream tests | Full posture/trust-source matrix through packaged CLI, JSON stream, ACP, and TUI |
| Sandbox | `wcore-sandbox/tests/{backend_integration,live_fs_acl,live_integrity,secret_read_deny}.rs`; tool VFS/symlink/routing tests | Same adversarial corpus and effective backend receipt on Linux, macOS, and Windows |
| Skills lifecycle | `w9_1_skill_drafting_per_turn.rs`, `w9_direct_invocation_test.rs`, `wayland_home_auto_skill_loop.rs`, inline legacy-drafter tests | Global/project lifecycle x memory 2x2x2 bootstrap/run matrix; cross-session registry visibility; PromptStore/disk negative assertions |
| Budget | Agent bootstrap, lifecycle, tracker, execution, workflow and `wcore-budget` cap tests; protocol budget goldens | Default finite bounds and pre-call cost reservation/overshoot |
| WAL | `session.rs` F030 WAL round-trip/resume/orphan tests | Kill-point recovery of tool request/result, approval, budget, and irreversible effect exactly once |
| Failover | Anthropic/OpenAI region tests; resilient/circuit tests; failover reason unit tests | Per-reason cooldown timing and deterministic request/stream stalls |
| Child behavior | Spawn, bus lifecycle, relay, graph merge, child-config and cancellation tests | Durable lifecycle, session restart, independent authority, worktree isolation, idempotent result delivery |

Current Desktop-hosted protocol shapes are protected in part by `golden_w7.rs`, `golden_w8a.rs`, `host_decoder_contract.rs`, `approval_resume_contract.rs`, and `set_config.rs`. F03 must turn these into an explicit versioned host-protocol golden bundle rather than relying on scattered tests.

## 6. F06 pre-change matrix

The current merge is `project.skills_lifecycle || global.skills_lifecycle` (`wcore-config/src/config.rs:3547-3549`). Because both deserialized defaults are true, absence and explicit true are conflated and a project `false` cannot defeat a default-true global object.

For explicitly constructed boolean inputs, the pre-change behavior is:

| Global lifecycle | Project lifecycle | Memory | Resolved lifecycle | `want_memory` | Legacy drafter expected |
|---:|---:|---:|---:|---:|---:|
| false | false | false | false | false | no |
| false | false | true | false | true | **yes: lifecycle bypass** |
| false | true | false | true | true | yes |
| false | true | true | true | true | yes |
| true | false | false | true | true | **yes: project opt-out lost** |
| true | false | true | true | true | **yes: project opt-out lost** |
| true | true | false | true | true | yes |
| true | true | true | true | true | yes |

Direct parsing of one file with `skills_lifecycle = false` works. The failures are cascade semantics and the memory-backed legacy construction/observation path, not boolean deserialization itself.

## 7. Shared-file collision and ownership map

Only the lead integrator changes these shared surfaces during F06. Other lanes may supply tests or review, not competing edits.

| Shared surface | Size at baseline | Current/F06 owner | Frozen handoff |
|---|---:|---|---|
| `wcore-agent/src/engine.rs` | 18,932 lines | F06 lead integrator | F07/F11/F13/F16/F18+ after F06 merge |
| `wcore-agent/src/bootstrap.rs` | 3,655 lines | F06 lead integrator | F05 capability state, then F09 runtime scope |
| `wcore-config/src/config.rs` | 7,108 lines | F06 lead integrator | F07 posture and F11 budgets after lifecycle precedence is frozen |
| `wcore-agent/src/session.rs` | 1,176 lines | No F06 production edit expected | F12-F14 journal/recovery owner |
| `wcore-protocol/src/events.rs` | 1,323 lines | No emergency F06 edit expected | F03/F05 schema owner; F07+ append only after golden freeze |
| `wcore-protocol/src/commands.rs` | 534 lines | No emergency F06 edit expected | F07 posture vocabulary owner |
| `wcore-protocol/src/lib.rs` | 1,059 lines | No emergency F06 edit expected | Approval/host compatibility owner |
| `wcore-agent/src/auto_skill/drafter.rs` | 394 lines | F06 lead integrator | F23 governed lifecycle may replace it only after evaluation/promote contracts exist |

## 8. Evidence receipt status

| Level | Evidence | Status |
|---|---|---|
| L0 | Source identity, dependency digests, call-site inventory, thresholds | Complete |
| L1 | Targeted unit/component characterization | Existing tests inventoried; F06 failure reproductions still to be added |
| L2 | Deterministic real engine path | Pending F01-F04 fixture driver |
| L3 / E3 | Exact packaged binary, source SHA, digest, Linux execution | Complete; strict provenance passed (`1 passed; 0 failed`) under Cargo/Rust 1.95.0 |
| E4/E5 | Three-OS integration, fault/adversarial/soak | Not claimed |

The exact Linux receipt authorizes the surgical F06 emergency containment. It does not close M0: F01-F05 and F06 packaged proof remain required.
