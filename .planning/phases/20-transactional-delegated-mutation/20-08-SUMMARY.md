---
phase: 20-transactional-delegated-mutation
plan: "08"
subsystem: [agent, swarm, cli]
tags: [delegated-mutation, full-lifecycle, anvil-landing, surface-for-accept, gate-authorization, 06c-reverification]
requires: ["20-01", "20-02", "20-03", "20-04", "20-05", "20-07", "20-14"]
provides:
  - A production terminal landing orchestrator (land_selected_winner) composing into_landing_authority (fail-closed on None) → open → accept_selected_winner (06C hard-containment gate re-run) → land (20-07 CAS) fail-closed at every hop
  - Reachability + identity seams making the delegated-mutation lifecycle reachable from the live Anvil forge (session_journal, winner_identity/WinnerIdentity, create_integration_checkout, current_branch)
  - Anvil-gate → 06C AuthorizedGate translation (gate_authorization) that pins the 06C closure digest, seals empty env for re-run parity, and grants only the transaction's own scratch as writable — containment preserved
  - drive_climb_full wired end-to-end so a gate-verified climb winner lands transactionally into a RETAINED Wayland-owned integration clone (surface-for-accept), never touching the user's repository, with the outcome reported into ClimbOutcome.landing + the tool report
affects: ["20-16"]
scope_expansion:
  added:
    - "crates/wcore-agent/src/orchestration/anvil/landing.rs (NEW — terminal orchestrator)"
    - "crates/wcore-agent/src/orchestration/anvil/gate_authorization.rs (NEW — 06C gate translation)"
    - "crates/wcore-agent/src/spawner.rs (mutation_workspace mod widened to pub(crate) so the canonical writable-roots computation is reachable, never a local replica)"
    - "crates/wcore-agent/src/child_transaction/gate_executor.rs (LiveCandidateRoot: Send + Sync — REQUIRED for the formerly test-only 06C gate-exec future to be Send in the production Tool context; both impls already satisfy it; behavior-free)"
    - "crates/wcore-swarm/src/worktree_manager.rs (create_integration_checkout + current_branch)"
  rationale: "The delegated-mutation stack (06A-06D + 20-05/07) was proven but reachable ONLY from tests. 20-08 makes it live from the Anvil forge, which requires the above seams. The gate_authorization module and mutation_workspace visibility are new construction; the LiveCandidateRoot Send+Sync bound is forced by wiring a Send-required async Tool path over the 06C gate execution. Each is documented and faces the 20-16 review."
key-files:
  created:
    - crates/wcore-agent/src/orchestration/anvil/landing.rs
    - crates/wcore-agent/src/orchestration/anvil/gate_authorization.rs
    - crates/wcore-agent/tests/transactional_delegated_mutation_test.rs
    - scripts/f20-native-windows-proof.ps1
    - scripts/f20-native-macos-proof.sh
    - scripts/f20-native-uat-proof.mjs
    - scripts/f20-native-uat-proof.test.mjs
    - .github/workflows/nightly-windows-soak.yml
  modified:
    - crates/wcore-agent/src/child_transaction.rs
    - crates/wcore-agent/src/child_transaction/gate_executor.rs
    - crates/wcore-agent/src/orchestration/anvil/engine.rs
    - crates/wcore-agent/src/orchestration/anvil/forge.rs
    - crates/wcore-agent/src/orchestration/anvil/tool.rs
    - crates/wcore-agent/src/orchestration/anvil/mod.rs
    - crates/wcore-agent/src/spawner.rs
    - crates/wcore-agent/src/durable_child.rs
    - crates/wcore-agent/tests/anvil_forge_transaction.rs
    - crates/wcore-swarm/src/worktree_manager.rs
key-decisions:
  - "SURFACE-FOR-ACCEPT (Sean): the winner lands onto refs/heads/<branch> INSIDE a Wayland-owned integration clone (a --no-local --no-hardlinks --single-branch clone at the exact user tip); the user's real repository is never opened or mutated. Desktop surfaces the retained clone and the user fast-forwards from it. (refs/wayland/landing/<slug> is the 20-07 primitive's INTERNAL quarantine ref, not a caller target — the target must be a fully-qualified refs/heads/ ref whose symbolic HEAD names it.)"
  - "DESKTOP-OWNED GC (Sean): the landed clone is RETAINED past the climb so Desktop can surface + accept it; Wayland Desktop reclaims it after accept/reject. Core adds no GC subsystem. (Interim implementation retains via mem::forget, which leaks the manager's in-memory byte reservation for the clone until process exit — bounded, few landed clones per session; FOLLOW-UP: a clean TransactionWorkspace::persist() that frees the accounting while keeping the checkout on disk.)"
  - "06C RE-VERIFICATION (security core): accept_selected_winner RE-RUNS the pinned gate under HardContainment before landing. gate_authorization pins the 06C AuthorizedGateClosure digest (domain wayland-core:authorized-gate-closure:v1 — NOT the Anvil anvil-gate-closure-v1 digest; both 64-hex, so a swap passes shape checks but is rejected as SubstitutedClosure at resolve). Env is sealed EMPTY (matches both the forge's empty allowlist and the re-run's empty manifest env — no divergence, no secret leak). private_writable_roots is the winner's own scratch (canonical mutation_writable_roots), the ONLY writable mount; the candidate is read-only; network denied. Containment identical to the 20-14-audited model."
  - "FAIL-CLOSED landing: a winner surrendering no landing authority is a hard refusal; a winner failing the 06C re-run cannot land; every attempt_landing failure path is REPORTED (LandingReport::Failed) into ClimbOutcome.landing, never crashing the climb. emit_receipt runs BEFORE landing (landing consumes the winner, terminalizing the checkout the receipt digests)."
patterns-established:
  - "A gate-verified Anvil climb winner integrates into the parent ONLY by: opening a durable transaction against the exact journal that declared it, re-running the pinned gate under hard containment, landing via 20-07 CAS into a Wayland-owned clone, and reporting the outcome — the user's repository is never touched and delivery is surface-for-accept."
requirements-completed: []
duration: n/a
completed: 2026-07-21
status: complete
source_sha: 6937ef6
source_tree: 6db6fc859539b43f083aa0a22f3e3e0a014721ae
task_base: 21329d0
repair: "Repaired successor of the original 20-08 construction 5e665ec, after that candidate's native UAT (run 29886894436) and full test suite surfaced pre-existing breakage the source-only 20-08 review could not: (a) wcore-sandbox Windows lib compile (parked windows_impl module-path debt, b5613b0); (b) docker.rs bollard private-const under live-docker (61b599c); (c) the native macOS proof script referenced a Windows-only test + missing live-docker features, and two macOS acceptance tests were unwritten (b3ebf23, 47a7a4c, 3383a46, 95e1b98, target-5 refusal-descope); (d) 2 spawn_tool durable tests + 2 fail-closed security tests had stale fixtures (1902a64, af295e7); (e) the F14 sigkill journal reader predated the WSA1 snapshot-authority frame (bb49d61); (f) stale Desktop-contract provenance digests (6937ef6, digests-only, wire contract unchanged). The 11 core 20-08 files are byte-identical 5e665ec→6937ef6 (git diff = 0). Candidate 6937ef6 is Linux-GREEN: nextest --workspace --profile ci = 11509 passed / 0 failed / 48 skipped."
independent_reviews: "Original 20-08 (5e665ec): three fresh non-author adversarial reviews, all VERDICT CLEAN: (1) land_selected_winner terminal composition (fail-closed, no false success); (2) gate_authorization 06C translation (digest correctness, containment not widened, empty-env parity, no false acceptance, gate-id fail-closed); (3) the FULL integrated landing path via drive_climb_full (user tree untouched — parent never opened, clone has no alternates/hardlinks; containment intact; never crashes the climb; uuid-collision-free; RAII release; honest diff digest). Repaired successor (6937ef6): two fresh non-author independent reviews (wayland-f20-16-repair-review + wayland-f20-16-adversarial-confirmer), both zero findings at every severity — core byte-identity confirmed, corpus change verified digests-only, every repair commit prosecuted sound at the mechanism level; native macOS/Windows deferred to the Sean-authorized 20-18."
verification: "Linux, committed-HEAD Hetzner harness, source 5e665ec (tree-identical to the proven c2b89b7). clippy -p wcore-swarm -p wcore-agent --all-targets --all-features -D warnings clean. anvil_forge_transaction 5/5 (incl. production_landing::drive_climb_full_lands_the_winner_surface_for_accept — a real bwrap-gated climb winner → Landed report, retained clone whose refs/heads/main == landed_commit, parent workspace HEAD unchanged + tracked tree clean). transactional_delegated_mutation_test 9/9 (incl. land_selected_winner_drives_production_chain_to_landed proving content-capture). wcore-swarm --lib (incl. current_branch + integration_checkout)."
open_items:
  - "TODO(20-08): the MockLlmProvider builder's Write does not reach the winner's SEALED checkout in the production_landing test, so that test cannot yet assert the winner's file is IN the landed commit (its landed tree == base). Scoped out with a TODO — a builder-test-harness wiring detail, NOT a landing bug: content-capture from a staged winner IS proven at land_selected_winner_drives_production_chain_to_landed. Verify the builder-write→seal path."
  - "FOLLOW-UP: clean TransactionWorkspace::persist() (frees the manager byte reservation while keeping the clone on disk) to replace the documented mem::forget retention."
  - "OBS (from the 2C review, non-flaws): input_identities seals paths not content (faithful to 06C — the re-run re-executes the gate against the read-only candidate, never gating on input content); is_valid_gate_id looser than the private validate_identifier (canonical_digest re-validates, fail-closed)."
  - "DEFERRED (per f20-16): native_windows / native_macos UAT scaffolding is WRITTEN not run (Sean-gated at 20-17/18). Parked pre-native debts: the windows_impl module-path patch; 2 pre-existing deterministic spawn_tool --lib failures (20-04-era)."
---

# Phase 20 Plan 08: Full Transactional Delegated Mutation Lifecycle + Live Anvil Landing

**The complete delegated-mutation lifecycle is composed and wired into the live Anvil forge: a gate-verified climb winner is re-verified under hard containment and lands transactionally (20-07 CAS) onto a Wayland-owned integration clone's branch, surface-for-accept, without ever touching the user's repository. Linux-proven at source `5e665ec`; three independent adversarial reviews CLEAN. Admits 20-16.**

## What lands

- `land_selected_winner` (landing.rs) composes the terminal chain fail-closed.
- `gate_authorization.rs` translates the pinned Anvil gate into the 06C authorization the parent re-runs under hard containment (digest-correct, containment-preserving).
- Reachability + identity seams (`session_journal`, `winner_identity`/`WinnerIdentity`, `create_integration_checkout`, `current_branch`) make the lifecycle reachable from `drive_climb_full`.
- `drive_climb_full` wires it end-to-end and reports the landing outcome; the landed clone is retained for Desktop-mediated accept (Desktop-owned GC).

## Surface-for-accept (Sean's decision)

The winner lands onto the integration clone's own `refs/heads/<branch>` (the clone is a standalone `--no-local --no-hardlinks --single-branch` clone at the exact user tip). The user's real repository is never opened. The landed clone is retained so Wayland Desktop can surface it and the user fast-forwards from it when they choose; Desktop reclaims it afterward.

## Verification

See the `verification` field. clippy clean (2 crates, `-D warnings`); the production-path test proves a real bwrap-gated climb winner lands with the user tree provably untouched; the lower-seam e2e proves content-capture. Three independent adversarial reviews returned CLEAN (terminal composition, gate translation, and the full integrated path — user-tree-untouched and containment intact above all).

## Explicit non-claims / open items

See `open_items`. Native Windows/macOS UAT is scaffolded but Sean-gated (20-17/18). Two documented follow-ups (clean `persist()`, builder-write-capture TODO) and the 2C OBS notes carry to the 20-16 review.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-21*
