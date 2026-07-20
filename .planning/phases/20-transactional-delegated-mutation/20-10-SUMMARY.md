---
phase: 20-transactional-delegated-mutation
plan: "10"
subsystem: sandbox
tags: [delegated-mutation, hard-containment, live-probe, opaque-capability, one-use, source-packet]
requires: ["20-02", "20-09"]
provides:
  - Opaque, one-use, non-serializable, non-cloneable HardContainmentAuthority minted only from a successful semantic live probe of the exact backend + normalized policy used for spawn
  - A read-only-candidate + private-writable-root filesystem model (HardContainmentFilesystem) with network and inherited credentials/config structurally denied, traversal-free path validation, and no bypass field expressible
  - Structural exclusion so no boolean, configured backend name, process group, caller claim, serialized value, or bypass runtime can qualify as hard containment
affects: [20-11]
tech-stack:
  added: []
  patterns: [opaque one-use live-probe capability, structural (pub-type/pub(crate)-field) mint seal, per-field spawn-time drift refusal, fail-closed process-tree ownership, deny-by-default + traversal-free containment filesystem, redacted-Debug opaque capability]
key-files:
  created: []
  modified:
    - crates/wcore-sandbox/src/lib.rs
    - crates/wcore-sandbox/src/manifest.rs
    - crates/wcore-sandbox/src/backends/mod.rs
    - crates/wcore-sandbox/src/backends/appcontainer.rs
    - crates/wcore-sandbox/src/backends/bwrap.rs
    - crates/wcore-sandbox/src/backends/docker.rs
    - crates/wcore-sandbox/src/backends/process_tree.rs
key-decisions:
  - "HardContainmentAuthority is minted ONLY by SandboxRegistry::establish_hard_containment, which fails closed unless (1) the registry does not bypass containment and (2) the backend passes a semantic LIVE probe of its exact mechanism under the normalized policy; it then cross-checks the live probe's identity against the backend's cheap stable identity before minting via a non-pub associated fn."
  - "Structural exclusion is the crux: HardContainmentProbe / HardContainmentIdentity are pub structs with pub(crate) fields, so an external crate can implement SandboxBackend but CANNOT construct the probe proof — its probe_hard_containment can never return Ok, and the trait default is Err(PolicyNotSupported)/None. Only in-crate bwrap, docker, and AppContainer build one."
  - "ProcessTreeMechanism and HardContainmentMechanism deliberately have NO ordinary-process-group / sandbox-exec / no-sandbox variant, so a setsid/setpgid-escapable process group can never by itself name the hard-containment boundary; sandbox-exec, no_sandbox, FailClosedBackend, and stubs keep the trait defaults and are structurally non-qualifying."
  - "The authority is opaque and one-use: no serde, no Clone/Copy, hand-written REDACTED Debug on HardContainmentAuthority / HardContainmentIdentity / HardContainmentProbe / ContainmentPolicyIdentity (executable/runtime identity, candidate + writable-root paths, and spawn argv/cwd never printed), and verify_no_drift takes self BY VALUE (consumed on use). It privately binds backend, executable/runtime identity, mechanism, process-tree mechanism, normalized ContainmentPolicyIdentity, and the exact SpawnIdentity (argv + cwd); any drift in any field refuses at spawn, naming the drifted field."
  - "HardContainmentFilesystem binds the candidate READ-ONLY and permits writes ONLY to parent-created transaction-private roots; every path must be absolute AND traversal-free (no '.'/'..' components — path_has_traversal, matching the crate's Component::Normal discipline) so the lexical denial/overlap checks cannot be defeated by a '..' segment the kernel would later resolve at bind time; denied_location structurally refuses global temp (incl. TMPDIR/TMP/TEMP) and home/config/credential dirs, writable roots must be disjoint from the candidate, and to_manifest() sets network=Deny with every field explicit — no bypass field exists."
  - "Live probes reuse the existing execute_bound/execute paths (ProcessTreeGuard PID-ns descendant reap on Linux / ContainerCleanup force-remove), so any probe-stage failure kills the owned tree and returns an error; the probe command is the benign builtin `true`, NEVER candidate argv, so a failed admission never runs candidate-controlled code."
  - "The accepted 20-02 AppContainer profile/ACL/recovery lifecycle in windows_impl/* is untouched; appcontainer.rs only adds an in-scope mechanism-identity constructor (windows_appcontainer_hard_containment_identity) for a future windows_impl/process.rs override. Until that override lands (out of this packet's scope), the real Windows backend keeps the default and is conservatively non-qualifying — matching the plan's cfg-gated, CI-verified-later intent."
patterns-established:
  - "A process-containment boundary is trusted only via an opaque, one-use capability minted from a live semantic probe of the exact backend + normalized policy that the spawn will use, with the mint path structurally unreachable from outside the crate, every bound parameter re-checked for drift at spawn, all policy paths traversal-free, and the capability's execution plan never leaked through Debug."
requirements-completed: []
duration: n/a
completed: 2026-07-20
status: complete
source_sha: b1de890363ab82ba952ad03bb5e692461c1cc8b5
source_tree: 8f3ef81889825ead9c26df1b453e871a89e14b34
task_base: 1ace69601b252361fcf0bb993e503a7240f1f0ec
changed-paths:
  - crates/wcore-sandbox/src/lib.rs
  - crates/wcore-sandbox/src/manifest.rs
  - crates/wcore-sandbox/src/backends/mod.rs
  - crates/wcore-sandbox/src/backends/appcontainer.rs
  - crates/wcore-sandbox/src/backends/bwrap.rs
  - crates/wcore-sandbox/src/backends/docker.rs
  - crates/wcore-sandbox/src/backends/process_tree.rs
---

# Phase 20 Plan 10: Opaque One-Use HardContainmentAuthority (06B source packet)

**A live, one-use, opaque process-containment authority minted only from a successful semantic live probe of the exact backend and normalized policy the spawn will use. Linux-proven at source `b1de890`; qualifies for the fresh non-author 20-11 review.**

## What this builds

`SandboxRegistry::establish_hard_containment(fs, cmd)` is the sole public path to a `HardContainmentAuthority`. It fails closed unless the registry does not bypass containment AND the selected backend passes a semantic **live probe** of its exact hard-containment mechanism under the normalized policy, then cross-checks the live probe identity against the backend's stable identity before minting through a non-pub `mint`. The authority privately binds backend, executable/runtime identity, mechanism, process-tree mechanism, normalized `ContainmentPolicyIdentity`, and the exact `SpawnIdentity`; `verify_no_drift` consumes `self` (one-use) and refuses on any drift, naming the field. Its `Debug` is hand-written and **redacted** — the bound execution plan (identities, paths, argv/cwd) never reaches a log.

The **structural seal**: `HardContainmentProbe`/`HardContainmentIdentity` are `pub` structs with `pub(crate)` fields, so a foreign `SandboxBackend` cannot build the probe proof and can never return `Ok` from `probe_hard_containment` (trait default `Err(PolicyNotSupported)`/`None`). Only in-crate bwrap (PID namespace), docker (container), and AppContainer (Job Object) qualify; `sandbox-exec`, `no_sandbox`, `FailClosedBackend`, stubs, and any process-group backstop are structurally non-qualifying — `ProcessTreeMechanism`/`HardContainmentMechanism` carry no such variant.

`HardContainmentFilesystem` encodes the one accepted shape: candidate READ-ONLY, writes only to parent-created transaction-private roots, `network=Deny`, every path **absolute and traversal-free** (`path_has_traversal` rejects `.`/`..` before the lexical denial/overlap checks), and `denied_location` structurally refusing global temp and home credential/config dirs — no bypass field is expressible. Probes reuse the tree-owning `execute_bound`/`execute` paths and run the benign builtin `true` (never candidate argv), so any probe-stage failure kills the owned tree and fails closed.

## Verification (Linux, committed-HEAD Hetzner harness, source `b1de890`)

- **`clippy -p wcore-sandbox --all-targets --all-features -- -D warnings`:** Finished clean (0 warnings) — confirms the private-in-public (`pub`/`pub(crate)`) trait-return shape, the `#[async_trait]` defaulted `probe_hard_containment`, the `#[allow(dead_code)]` enums, the hand-written Debug impls, and the `default_image` visibility change all pass under `-D warnings`.
- **`test -p wcore-sandbox`:** 80 lib tests passed, 0 failed; all integration binaries passed. Includes the **live** `required_live_bwrap_hard_containment_mint_and_drift` (real bwrap PID-namespace probe mints; argv drift refuses), `probe_failure_at_every_stage_fails_closed`, `probe_identity_disagreement_fails_closed`, per-field drift refusals, `non_qualifying_backends_cannot_mint`, `bypass_registry_cannot_mint`, the read-only/private-write/denied-location cases, and the two 20-11-round-1 regression tests `hard_containment_rejects_traversal_components` and `authority_debug_is_redacted`.
- **`check --workspace --all-targets` (at the pre-fix source):** Finished, 0 errors — the two new defaulted trait methods are backward-compatible with all 8 downstream `SandboxBackend` impls in wcore-agent and wcore-tools (per memory bit #231). The finding-fix changed only Debug impls, a private helper, and tests — no trait signature changed — so downstream compatibility is unaffected.
- **Scope:** `verify-task-scope.sh` → `scope-ok base=1ace6960 paths=7`; `git diff --check` clean; `vx cargo fmt` clean.

## Review-driven hardening (20-11 round 1)

The first 06B source (`c7bf6d3`) was prosecuted by the 20-11 independent review, which found two real gaps repaired at source and re-proven on Linux before this recorded successor (`b1de890`):

1. **MEDIUM (policy_sufficiency):** `HardContainmentFilesystem::new` / `denied_location` validated paths with only `is_absolute()` + lexical `starts_with`, so a `..` component escaped both the temp/credential denial and the candidate-overlap check and would then be resolved by the kernel at the `bwrap --ro-bind-try`/`--bind-try` boundary (reaching host `~/.ssh` read or a write inside the read-only candidate). Fixed by rejecting any candidate/writable-root with non-`Normal` (`.`/`..`) components (`path_has_traversal`), matching the crate's existing `Component::Normal` discipline, plus a `..`-path regression test.
2. **LOW (opacity):** `HardContainmentAuthority` (and `HardContainmentIdentity`/`HardContainmentProbe`/`ContainmentPolicyIdentity`) used `#[derive(Debug)]`, printing the bound executable/runtime identity, candidate + writable-root paths, and full spawn argv/cwd — contradicting the opaque-capability property. Fixed with hand-written redacting `Debug` impls (only non-sensitive backend name / mechanism shown) plus a redaction regression test.

## Cross-audit correction (main-context, pre-review)

The executor's first committed source did NOT compile (`super::HardContainmentMechanism` unresolved inside the bwrap test module — `super` there is the `bwrap` module, not `crate::backends`) and shipped one asserting-on-the-wrong-reason test. Both were corrected at source and re-proven before the review: the path fixed to the crate re-export, and the test pointed at a same-named `VanishedBackend` (identity `None`) that genuinely exercises the "backend no longer offers hard containment" fail-closed branch.

## Deviations / deferrals (surfaced to the 20-11 reviewer)

- **`lib.rs` is >1000 lines (>guideline).** The clean fix is extracting `hard_containment_tests` to its own file, which this packet's 7-file scope forbids; flagged for a follow-up extraction.
- **Windows AppContainer live override deferred.** The real `SandboxBackend` impl is in the out-of-scope `windows_impl/process.rs`; in-scope `appcontainer.rs` provides the mechanism-identity wiring for that future override. Today the real Windows backend keeps the default → conservatively non-qualifying. Matches the plan's cfg-gated, CI-verified-later intent.
- **Pre-existing (NOT from 20-10):** default-feature builds warn that `export_tar_bounded`/`replace_from_tar_bounded` in the out-of-scope `directory_authority_archive.rs` are unused (feature-gated; clean under `--all-features`). Noted for the 20-08 workspace gate, not fixed here.

## Explicit non-claims

This packet is solely the containment authority. It makes NO gate-result, receipt, candidate-acceptance, landing, or complete-lifecycle claim. It admits the fresh non-author 20-11 review and advances no downstream source.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-20*
