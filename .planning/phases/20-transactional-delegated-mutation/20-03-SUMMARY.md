---
phase: 20-transactional-delegated-mutation
plan: "03"
subsystem: sandbox
tags: [delegated-mutation, retained-authority, sandbox, swarm, bash, docker, bwrap, process-tree, native-uat]
requires: ["20-02"]
provides:
  - Platform-neutral retained directory/regular-file capability substrate used across validation and use
  - Public Swarm dispatch that carries retained filesystem, process, workspace, heartbeat, cleanup, and reservation authority end to end
  - Owned standalone Git checkout per mutating child with independent metadata and object store
  - Bounded canonical archive export/import for Docker Desktop into container-owned /workspace (no ambient host bind mount)
  - Hard process-tree ownership with terminal teardown before workspace cleanup and reservation release
  - Bash containment bound to the supplied isolated workspace authority plus a transaction-private scratch directory
affects: [20-04, 20-06, 20-15, delegated-mutation, sandbox, swarm, bash-tool]
tech-stack:
  added: []
  patterns: [retained filesystem capability, public-lifecycle authority carry, standalone owned checkout, bounded archive transport, hard process-tree ownership, isolated Bash containment]
key-files:
  created:
    - crates/wcore-sandbox/src/directory_authority.rs
    - crates/wcore-sandbox/src/directory_authority_file.rs
    - crates/wcore-sandbox/src/directory_authority_archive.rs
    - crates/wcore-sandbox/src/directory_authority_windows.rs
    - crates/wcore-sandbox/src/directory_authority_windows_tests.rs
    - crates/wcore-sandbox/src/directory_authority_tests.rs
    - crates/wcore-sandbox/src/backends/docker_tests.rs
    - crates/wcore-sandbox/src/backends/appcontainer/windows_impl/command.rs
    - crates/wcore-sandbox/src/backends/appcontainer/windows_impl/handles.rs
    - crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs
    - crates/wcore-sandbox/src/backends/appcontainer/windows_impl/tests.rs
    - crates/wcore-swarm/src/worktree_cleanup.rs
    - crates/wcore-swarm/src/worktree_manager.rs
    - crates/wcore-swarm/src/worktree_tests/linux.rs
    - crates/wcore-swarm/tests/workspace_authority.rs
    - crates/wcore-tools/src/bash/policy.rs
    - crates/wcore-tools/src/bash/tests.rs
    - crates/wcore-tools/src/workspace_policy/discovery.rs
    - crates/wcore-tools/src/workspace_policy/tests.rs
  modified:
    - Cargo.lock
    - crates/wcore-sandbox/Cargo.toml
    - crates/wcore-sandbox/src/lib.rs
    - crates/wcore-sandbox/src/backends/mod.rs
    - crates/wcore-sandbox/src/backends/appcontainer.rs
    - crates/wcore-sandbox/src/backends/bwrap.rs
    - crates/wcore-sandbox/src/backends/docker.rs
    - crates/wcore-sandbox/src/backends/process_tree.rs
    - crates/wcore-swarm/Cargo.toml
    - crates/wcore-swarm/src/lib.rs
    - crates/wcore-swarm/src/dispatch.rs
    - crates/wcore-swarm/src/heartbeat.rs
    - crates/wcore-swarm/src/worktree.rs
    - crates/wcore-swarm/src/worktree_security.rs
    - crates/wcore-swarm/src/worktree_tests.rs
    - crates/wcore-swarm/tests/dispatch_smoke.rs
    - crates/wcore-swarm/tests/heartbeat_test.rs
    - crates/wcore-swarm/tests/swarm_worker_failure_reporting_e2e.rs
    - crates/wcore-swarm/tests/worker_runtime_limits.rs
    - crates/wcore-tools/src/bash.rs
    - crates/wcore-tools/src/workspace_policy.rs
    - crates/wcore-tools/tests/bash_sandbox_routing_test.rs
key-decisions:
  - "Retain directory and regular-file identities as capabilities across validation and use; descendant reads, writes, renames, enumeration, cleanup, and command working directories bind to the retained objects, never a reopened mutable absolute path."
  - "Carry authority through the PUBLIC Swarm dispatch path (not a private worktree helper) across worker launch, heartbeat, terminal teardown, cleanup, and reservation release."
  - "Docker Desktop never receives the host workspace via ambient bind mount: retained authority exports a bounded canonical archive into container-owned /workspace and imports a bounded result transaction only after identity/reservation/budget revalidation."
  - "Bash is always mutation-capable and never reclassifies shell text; the canonical RequestedChildWorkspace is selected before execution and Bash cannot downgrade it."
  - "Native Windows and macOS execution is deferred to plan 20-08 final exact-candidate UAT; a foreign target, cross-compile, or source review is never a native PASS."
patterns-established:
  - "One delegated mutation owns one standalone checkout, one retained filesystem authority, and one hard-owned process tree."
  - "Import takes the exclusive retained mutation lease; monitors, heartbeat, accounting, cleanup, and command launches take a shared retained read lease; durable journal recovery completes under the exclusive lease before a new observer is admitted."
  - "Terminal teardown reaps the worker descendant tree before owner-bound cleanup and capacity-reservation release, retaining only bounded terminal evidence."
requirements-completed: []
coverage:
  - id: SC1
    description: "Canonical RequestedChildWorkspace precedes execution; Bash cannot redefine or downgrade it; public Swarm dispatch carries retained authority end to end; owned standalone repo; hostile identity replacement denied."
    verification:
      - kind: integration
        ref: "remote-cargo.sh f20-03-linux-swarm-lib ... nextest run --all-features -p wcore-swarm --lib"
        status: pass
      - kind: integration
        ref: "remote-cargo.sh f20-03-linux-swarm-integrations ... nextest -p wcore-swarm --test dispatch_smoke,heartbeat_test,swarm_worker_failure_reporting_e2e,worker_runtime_limits,workspace_authority"
        status: pass
      - kind: integration
        ref: "remote-cargo.sh f20-03-linux-public-dispatch ... -E test(=public_dispatch_owns_git_authority_and_preserves_parent_and_sibling_state)"
        status: pass
  - id: SC2
    description: "Retained filesystem capability substrate; descendant ops bound to retained objects; hard process-tree ownership with terminal teardown before cleanup/reservation release."
    verification:
      - kind: integration
        ref: "remote-cargo.sh f20-03-linux-sandbox ... nextest run --all-features -p wcore-sandbox --lib"
        status: pass
      - kind: integration
        ref: "remote-cargo.sh f20-03-linux-bwrap-admission ... -E test(=backends::bwrap::tests::required_live_bwrap_admission)"
        status: pass
      - kind: integration
        ref: "remote-cargo.sh f20-03-linux-bwrap-enforcement ... -E test(=backends::bwrap::tests::required_live_bwrap_retained_cwd_enforcement)"
        status: pass
      - kind: integration
        ref: "remote-cargo.sh f20-03-linux-process-teardown ... -E test(=backends::process_tree::linux_tests::required_live_descendant_teardown_before_workspace_cleanup)"
        status: pass
      - kind: integration
        ref: "remote-cargo.sh f20-03-linux-terminal-order ... -E test(=cancellation_kills_worker_descendant_and_releases_owned_workspace)"
        status: pass
      - kind: integration
        ref: "remote-cargo.sh f20-03-linux-capacity ... -E test(=independent_cli_processes_cannot_overbook_shared_capacity)"
        status: pass
  - id: SC3
    description: "Docker Desktop carries retained workspace through bounded canonical export/import into container-owned /workspace, never an ambient host bind mount."
    verification:
      - kind: integration
        ref: "remote-cargo.sh f20-03-linux-docker ... -E test(=backends::docker::tests::required_live_docker_admission_enforcement_and_teardown)"
        status: pass
  - id: SC4
    description: "Bash containment: write access only to the standalone checkout and a transaction-private scratch dir; parent/sibling reads/writes, global tmp, parent Git metadata, inherited config/memory/credentials, and symlink/reparse aliases denied; live parent+descendant confinement."
    verification:
      - kind: integration
        ref: "remote-cargo.sh f20-03-linux-bash-complete ... nextest -p wcore-tools --test bash_sandbox_routing_test"
        status: pass
      - kind: integration
        ref: "remote-cargo.sh f20-03-linux-bash ... -E test(=delegated_mutation_required_live_sandbox_confines_parent_and_descendants)"
        status: pass
  - id: SC5
    description: "Strict all-target/all-feature Clippy and repository formatting pass at the exact integrated plan commit on Linux."
    verification:
      - kind: other
        ref: "remote-cargo.sh f20-03-linux-clippy ... clippy -p wcore-sandbox -p wcore-swarm -p wcore-tools --all-targets --all-features -- -D warnings"
        status: pass
      - kind: other
        ref: "remote-cargo.sh f20-03-linux-fmt ... fmt --all -- --check"
        status: pass
  - id: SC6
    description: "Native Windows and macOS retained-handle/reparse and AppContainer execution."
    verification:
      - kind: manual_procedural
        ref: "deferred to plan 20-08 final exact-candidate native UAT (identities authored: directory_authority_windows_tests.rs, appcontainer/windows_impl/tests.rs)"
        status: unknown
    human_judgment: true
    rationale: "Native Windows/macOS behavior is an exact-candidate UAT concern for 20-08, not an admission gate for this dependency predecessor. A foreign target, cross-compile, or source review never substitutes for native evidence."
duration: n/a
completed: 2026-07-20
status: complete
source_sha: d343fc720c38c05d0097821ff3117f88e12fa203
source_tree: 7eca6e83107f1d7d1692f32ae17d6ed6ddf92135
execution_base: fda8ba1e7db1272dddcc9f3462b226aaa147d6a6
---

# Phase 20 Plan 03: Isolated Mutation Substrate Summary

**Delegated mutation runs inside a retained-capability substrate: an owned standalone checkout, hard process-tree ownership, bounded Docker archive transport, and Bash confined to the supplied isolated workspace — proven on Linux at the exact integrated commit.**

## Performance

- **Duration:** construction under staged Ferrox executors, cross-audited per task
- **Completed:** 2026-07-20
- **Tasks:** 5 (1A, 1B, 1C, 1D, 2)
- **Files modified:** 41 (19 added, 22 modified)
- **Integrated commit:** `d343fc72` — tree `7eca6e8`
- **Execution base (accepted plan):** `fda8ba1` (seal `d13a675` + pinned-rustfmt cleanup)

## Accomplishments

- Established a platform-neutral retained directory/regular-file capability facade (`directory_authority*`) that keeps descendant reads, writes, renames, enumeration, cleanup, and command working directories bound to opened filesystem objects rather than reopened mutable absolute paths.
- Wired retained authority through the **public** Swarm dispatch lifecycle — worker launch, heartbeat, terminal teardown, cleanup, and reservation release — with an owned standalone Git checkout whose metadata and object bytes are independent from the parent.
- Preserved retained workspace authority across Docker Desktop via a bounded canonical archive export/import into container-owned `/workspace`, admitting the result transaction only after identity, reservation, and budget revalidation — no ambient host bind mount.
- Hard-owned the child process tree in the enforcing backend with terminal teardown before workspace cleanup and capacity-reservation release; no unsandboxed/unowned fallback.
- Bound Bash containment to the supplied isolated workspace plus a transaction-private scratch directory, denying parent/sibling reads and writes, global temp, parent Git metadata, inherited Wayland config/memory/credentials, and symlink/reparse aliases.

## Task Commits

1. **Task 1A: Retained filesystem capability substrate + backend authority binding** — `8738b24`, `deddcf9`, `d04e61f`
2. **Task 1B: Wire retained authority through the public Swarm production lifecycle** — `80f595a`
3. **Task 1C: Prove public lifecycle, hostile identity changes, bounded cleanup** — `5685b9b`
4. **Task 1D: Preserve retained workspace authority across Docker Desktop (+ import-rollback + sandbox backend live proofs)** — `7074030`, `319cd92`, `d584724`
5. **Task 2: Bind Bash containment to the supplied isolated workspace authority (+ private scratch tmp/cache)** — `7b3a6bd`, `bcd8463`
6. **20-15 review-driven repair: fail-closed transaction cleanup on outstanding checkout loan** — `d343fc72`

## Verification

All gates ran on the committed-head Hetzner harness at the exact integrated commit `d343fc72` (tree `7eca6e8`), `--all-features`, `--no-tests=fail` (a missing identity or unavailable Bubblewrap/Docker runtime is a failure, not a skip):

- **Focused sandbox lib** (`-p wcore-sandbox --lib`): PASS
- **Swarm lib** (`-p wcore-swarm --lib`): PASS
- **Swarm integrations** (`dispatch_smoke`, `heartbeat_test`, `swarm_worker_failure_reporting_e2e`, `worker_runtime_limits`, `workspace_authority`): PASS
- **Bash routing suite** (`-p wcore-tools --test bash_sandbox_routing_test`): PASS
- **Named `required_live` identity receipts:** bwrap admission, bwrap retained-cwd enforcement, process-tree descendant teardown, Docker admission+teardown, public-dispatch git authority, terminal cancellation teardown, capacity overbooking, delegated-mutation Bash containment — all PASS
- **Clippy** (`-p wcore-sandbox -p wcore-swarm -p wcore-tools --all-targets --all-features -- -D warnings`): PASS
- **Formatting** (`cargo fmt --all -- --check`): PASS
- **Diff hygiene** (`git diff --check`) and clean-tree/HEAD assertions: PASS
- **Scope gate** (`verify-f20-03-scope.sh final`): `paths=41`, exact canonical match.
- **Native Windows / macOS execution:** NOT RUN and NOT CLAIMED — deferred to plan 20-08 final exact-candidate UAT. The native identities are authored (`directory_authority_windows_tests.rs`, `appcontainer/windows_impl/tests.rs`) and are non-skipping under native compilation.

Running the **full** `bash_sandbox_routing_test` suite (not only the named `-E` live subset) caught a stale test-teardown: `delegated_mutation_refuses_scratch_identity_substitution_before_spawn` tore down the scratch root with single-level `remove_dir`, which failed `DirectoryNotEmpty` once the `delegated_mutation` constructor began materializing the private scratch `tmp`/`cache` subtree. The product assertions were never reached; the teardown was corrected to `remove_dir_all` and folded into the Task 2 commit (`bcd8463`). This is why the plan mandates the broad suites, not just the live receipts.

**Independent-review-driven repair (commit `d343fc72`).** The parallel 20-15 review (public-lifecycle lens) found a HIGH defect the builder missed and the passing proof did not cover: when `release_terminal`'s escaped-descendant quarantine fired it returned without carrying `workspace`, so the workspace dropped and `TransactionCleanup::Drop → release()` deleted the checkout and freed the capacity reservation with no outstanding-loan check — defeating the quarantine and violating the locked must-have that detached `Drop` cleanup cannot authorize release or successor admission. The retained-authority lens independently flagged the same root cause as LOW (the Unix cleanup primitive lacked the loan self-guard the Windows path has). Repaired by binding the shared-loan checkout authority into `TransactionCleanup` and making `release()` fail closed (retaining reservation and root) while a descendant still holds the retained checkout descriptor, plus a hostile test (`release_refuses_while_checkout_loan_outstanding`). The full committed-head Linux proof was re-run green at the repaired source `d343fc72`.

## Salvage Disposition

- `94f014d0` — reconciled product-source ancestry; adopted as the integration base and adapted to the exact 41-path scope, retained-capability seams, and public-lifecycle wiring this plan requires.
- `2ab5ca3` — retained-capability reference; adopted path-by-path for the `RetainedWorkspaceAuthority` bridge and `/proc/self/fd/N` retained-cwd seam.
- `d1e623c` — Windows retained-handle/reparse reference; adopted into `directory_authority_windows*` and `appcontainer/windows_impl/*`.
- `e200c0a1` — bash-containment reference; adopted into `bash/policy.rs`, `workspace_policy*`, and the routing test.
- `ccf824b9` — already integrated upstream of the seal; retained only as provenance.

## Deviations from Plan

- **Pinned-rustfmt base cleanup (Issue B).** The seal `d13a675` was not `cargo fmt --all -- --check` clean under the pinned toolchain: exactly 8 files carried toolchain drift accumulated since 20-02. Seven are outside this plan's 41-path scope (`wcore-agent/tests/session_journal{,_compaction}_test.rs`, `wcore-sandbox/src/backends/appcontainer/acl_lease{,/mutation_lock,/storage}.rs`, `wcore-sandbox/tests/live_fs_acl.rs`, `wcore-types/src/child_transaction/tests.rs`) and were reformatted in a single `chore(fmt)` commit `fda8ba1` inserted between the seal and this plan's chain; the eighth (`appcontainer.rs`) is in-scope and its 20-03 revision is already fmt-clean. The 20-03 chain was rebased conflict-free onto `fda8ba1`; the accepted-plan authority tuple was re-pointed accordingly, and the final scope gate re-verified `paths=41`. This resolves the deviation recorded in 20-02's SUMMARY. **Program note:** the toolchain drift affected every phase's `fmt` gate; the base cleanup should be surfaced to Sean as a program-wide baseline correction.
- **Native Windows/macOS execution deferred to 20-08**, per the plan's own success criteria.

## Next Phase Readiness

Construction and the exact committed-HEAD Linux proof are complete, but **20-03 does not by itself unblock 20-04**. Plan **20-15** remains the required fresh, non-author review gate over this exact source (`fda8ba1..d343fc72`), and 20-04 cannot begin from this summary alone. This plan contributes to F20-01 and F20-02 without marking either requirement complete. Native Windows and macOS behavior must not be represented as passed until plan 20-08 runs the authored native identities on an authorized exact candidate.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-20*
