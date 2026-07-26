---
phase: 25-remote-reach-nodes-plugin-lifecycle
plan: "01"
subsystem: execution-backends
tags: [f25-01, f25-02, execution-backend, receipts, attestation, ssh, container, cloud, egress]
status: complete
termination_state: 2
requires:
  - wcore-sandbox containment registry
  - wcore-egress single outbound-HTTP chokepoint
  - wcore-eval-scenarios remote-execution receipt oracle (as a specification, not a dependency)
provides:
  - wcore-exec-backend crate — the provider-neutral ExecutionBackend contract
  - four reference backends (local, container, ssh, cloud) behind one conformance harness
  - wayland-core backend operator surface
  - ExecutionBackendSpec plugin mirror + HostExecutionBackendRegistrar
affects:
  - crates/wcore-cli (new subcommand; two fenced-file lines filed as SEAM-25-1..3)
  - crates/wcore-plugin-api (new mirror + one FORBIDDEN_CORE_IMPORTS entry)
  - crates/wcore-agent (new host adapter)
tech-stack:
  added: []
  patterns: [compose-not-replace, oracle-conformance, plugin-mirror, live-product-exercise, fail-closed]
key-files:
  created:
    - crates/wcore-exec-backend/src/{lib,contract,receipt,policy,registry,conformance,error}.rs
    - crates/wcore-exec-backend/src/backends/{mod,local,container,ssh,cloud}.rs
    - crates/wcore-exec-backend/tests/{conformance_matrix,live_equivalence}.rs
    - crates/wcore-plugin-api/src/execution_backend_spec.rs
    - crates/wcore-plugin-api/src/registry/execution_backends.rs
    - crates/wcore-plugin-api/tests/execution_backend_spec_mirror_test.rs
    - crates/wcore-agent/src/plugins/adapters/exec_backend_adapter.rs
    - crates/wcore-cli/src/backend.rs
  modified:
    - Cargo.toml, Cargo.lock (one workspace member, zero third-party crates)
    - crates/wcore-plugin-api/build.rs
    - crates/wcore-cli/{lib.rs,main.rs,Cargo.toml}
decisions:
  - "Cloud reference backend is fly-machines, committed on a 4/4 panel; basis=majority."
  - "Reference hibernation transition is `suspend`, never `stop` (binding condition C1)."
  - "wcore-sandbox deliberately NOT added to FORBIDDEN_CORE_IMPORTS — the plan's premise was wrong."
  - "The local backend owns its own process group instead of routing through SandboxBackend, because that trait exposes no handle a cross-process cancel could signal. Reported, not hidden."
metrics:
  tests_added: 33
  new_third_party_crates: 0
  defects_found_live: 5
  panel_members: 4
completed: 2026-07-26
---

# Phase 25 Plan 01: Provider-Neutral Execution-Backend Contract — Summary

A new mid-tier crate carries the whole F25-01 contract; four reference backends pass ONE
conformance harness; three of them ran the same deterministic task through the shipped binary on
hetzner-dsm and diffed to EQUIVALENT. **Success Criterion 1 is NOT MET** — the hibernating cloud
leg is unexercised for want of a credential only Sean can mint.

**Termination state 2: complete with a bounded cloud gap.** That is the state the plan defined for
exactly this outcome, and it was reached without a halt.

---

## 1. What was built

`crates/wcore-exec-backend` — a new workspace member with **zero new third-party crates**
(Cargo.lock went 1015 → 1016 packages, the one addition being the crate itself).

- **`contract.rs`** — the `ExecutionBackend` trait, with every F25-01 surface as a NAMED type
  rather than a blob: declared capabilities, effective policy (egress decision *and* its source,
  plus the secret-exposure set), artifact transfer, resource limits, cancellation, attestation,
  receipt emission, lifecycle health, and an orphan scan.
- **`receipt.rs`** — the production receipt. It **satisfies** the F04 remote-execution oracle
  rather than competing with it: ordered events from sequence 1, a reserved `task_accepted`,
  content addressing, exactly one terminal event that is also the last, denial *before* acceptance
  with no accepted event at all, one shared output budget across streamed text and artifact bytes,
  and Ed25519 attestation whose `key_id` is the SHA-256 of the pinned verifying key.
- **`policy.rs`** — reads the egress disposition from the `wcore-egress` shared policy instead of
  re-deriving it, and says `allow-all-default-no-policy-installed` out loud when nothing is
  installed rather than laundering a fail-open into the word "allow".
- **`backends/`** — local, container, ssh, cloud.
- **`conformance.rs`** — one function that drives any backend through the whole behaviour list, so
  no backend is proven by code written for it.
- **`registry.rs`** — an on-disk live-task registry, because `backend cancel` runs in a different
  process from `backend run` and cancellation cannot be an in-memory handle.

Plus the plugin mirror (`ExecutionBackendSpec` + `ScopedExecutionBackendRegistry` +
`HostExecutionBackendRegistrar`) and the `wayland-core backend` operator surface.

## 2. Why a new crate, recorded so it is not re-litigated

`wcore-sandbox` is the containment crate whose posture Phase 20 and 20A spent months proving. SSH
and a credentialed cloud REST client are network **reach**, not containment, and folding a
credentialed transport into that crate would put new attack surface inside the one crate carrying
the anti-swap and hard-containment guarantees. The contract is also strictly broader than
`SandboxBackend`. The new crate **composes** containment; it replaces and bypasses nothing.

## 3. The cross-audited decision (Task 2)

Four panel members, one shared evidence bundle, `(Recommended)` stripped and the options rotated so
the panel could not simply echo the plan's own prior. **All four returned `fly-machines`;
basis = `majority`.** Full record: `25-01-CLOUD-BACKEND-DECISION.md`; verbatim captures and the
dissent in `evidence/`.

Two of the three live measurements **refuted things the plan expected to be decisive**:

- Every vendor is dependency-satisfiable with **zero** new crates. `aws-sigv4` is already pinned at
  `=1.4.3` and `aws-sdk-ec2` is absent from the lockfile, so "EC2 needs a new crate" is measured
  FALSE. Dependency satisfiability therefore discriminates between nothing.
- The real discriminator turned out to be **response encoding**, which the plan's option text never
  anticipated: Fly and E2B answer JSON, the EC2 Query API answers `text/xml`, and the workspace
  carries no XML parser — so EC2 would have needed hand-rolled parsing or a forbidden new crate.

The adversarial pass did not rubber-stamp. It sustained two attacks: the panel decided on
ergonomics a question that is **tied at zero** on the criterion that grades it (no vendor
credential exists anywhere), and it underweighted that Fly's *ordinary* idle behaviour is
stop-not-suspend. Both were converted into binding conditions rather than discarded.

## 4. Live proof — Success Criterion 1

Host `hetzner-dsm`, 2026-07-26, release binary, driven **only** through `wayland-core backend`.

```
BACKEND    KIND       AVAILABLE  PROBE BASIS            DETAIL
local      local      yes        sandbox_backend_probe  platform containment backend 'bubblewrap' probed available
container  container  yes        daemon_ping            container daemon answered a version ping: server 29.2.1
ssh        ssh        yes        ssh_handshake          ssh handshake to f25-ssh-target reached the far end
cloud      cloud      NO         credential_absent      backend cloud has no credential (WAYLAND_F25_CLOUD_TOKEN); refusing to run and NOT falling back
```

Three receipts, all verifying individually, all agreeing:

| backend | artifact sha256 | input sha256 | workspace sha256 | terminal |
|---|---|---|---|---|
| local | `55eae7c6…` | `55eae7c6…` | `9c743b3c…` | success |
| container | `55eae7c6…` | `55eae7c6…` | `9c743b3c…` | success |
| ssh | `55eae7c6…` | `55eae7c6…` | `9c743b3c…` | success |

`NORMALIZED DIFF: EQUIVALENT`. The only differing fields were backend identity, transport and
timing — every one excluded by design and reported alongside. Cancellation reached
`Cancelled { reason: "operator cancelled" }` on all three, with zero residual confirmed against the
**real** process table, the **real** container listing and the **real** remote process table.

Detail: `25-01-EQUIVALENCE-EVIDENCE.md`; ledger `evidence/25-01-equivalence-ledger.txt`.

## 5. Five defects the live exercise found

Not one was a crash. All five were **false answers**, the class that survives a green suite, and
every one was found by driving the binary rather than by running tests.

1. **Task children inherited stdin** and the first task ate the operator script feeding the
   caller's stdin. The run stopped silently after `backend list`.
2. **The remote orphan scanner found itself** — the nonce travels on the scan script's own argv, so
   every scan reported one orphan that did not exist.
3. **The remote killer killed itself** via `pkill -f <nonce>`, then reported
   `remote kill failed:` with an EMPTY stderr while the work had in fact died.
4. **The remote scan could not see the real work at all** — the task's argv (`sleep 120`) carries
   no nonce, so a genuine orphan would have been invisible.
5. **The first equivalence run compared four different tasks**, because the task id was suffixed
   per backend. The diff correctly said DIVERGENT while every content digest matched.

All fixed, all re-proved live, each with a regression test.

## 6. Deviations from the plan

**[Plan correction — HIGH] `wcore-sandbox` was NOT added to `FORBIDDEN_CORE_IMPORTS`.**
The plan asserted its absence was an unnoticed hole that "would silently permit exactly the
dependency audit finding F2 exists to forbid". Measured: `wcore-plugin-api/Cargo.toml` already
carries `wcore-sandbox.workspace = true` as an **explicitly annotated M5.1 security-allowlisted
dependency**, because `PluginContext` hands plugins an `Arc<SandboxRegistry>` so a
`requires_sandbox` tool can be contained at all. Adding it fails the build of the existing tree —
`build.rs` exit 101, verified on hetzner-dsm. Only `wcore-exec-backend` was added. The distinction
that makes both calls right: a `SandboxRegistry` can only **narrow** what a plugin may do, while an
execution backend would **widen** it. Reversing the M5.1 allowlist is a real architectural question
belonging to whoever owns it, not to a phase that walked past it.

**[Rule 2 — missing critical functionality] `Stdio::null` on task children.** See defect 1.

**[Rule 3 — blocking] `WAYLAND_EXEC_SSH_CONFIG`.** The ssh backend needed a way to reach a target
with a non-default port and identity. Added as a **file path** (`-F`), deliberately not a
free-form option string: a space-separated "extra ssh options" variable would be an
argument-injection surface pointed at the most attacker-adjacent binary in the phase.

**[Scope — not done] `[execution_backends]` keys in `crates/wcore-config/src/config.rs`.**
That file is 325 KB and is a named cross-phase seam. The crate reads its state directory from
`WAYLAND_EXEC_BACKEND_STATE_DIR` or `wayland_config_dir()`, and the ssh/cloud targets from
environment names. No Success Criterion depends on the config keys. Recorded as an explicit gap,
not silently dropped.

## 7. Known gaps, stated plainly

- **Cloud leg UNEXERCISED.** Credential absent everywhere. `evidence/25-01-cloud-credential-probe.txt`
  carries the exact closing command and exactly what to mint — one throwaway Fly org, one token
  scoped to it, nothing wider.
- **The SSH leg targeted a containerized sshd on the same physical host.** Separate network
  namespace, filesystem and process table, reached over a real ssh connection with a real key — it
  proves the transport and remote-session cancellation, **not** the cross-machine case.
  `backend.instance_id` being identical across all three receipts is the evidence of that, and it
  is reported rather than glossed. `SeanD@seandesktop` is reachable but is Windows, which has no
  `setsid`, `cat` or `base64` on the default path.
- **No Windows leg.** No `wayland-core` build exists on SeanDesktop in this window. Recorded as
  unexercised rather than inferred from Linux.
- **The local backend does not route the child through `SandboxBackend::execute`.** That trait owns
  the child internally and returns no handle, so a `backend cancel` from another process would have
  nothing to signal, and cancellation that only works in-process is not cancellation. The backend
  owns its own process group and the containment mechanism it did **not** apply is named in the
  effective policy rather than quietly implied. Closing this needs a pid-or-handle surface on
  `SandboxBackend` — a `wcore-sandbox` change, recorded as a finding.
- **Plugin-declared backends are captured but not reified.** A plugin-declared backend describes a
  transport the host has no implementation for, so reification needs a transport factory this phase
  does not build. The mirror exists so the isolation boundary is right when that factory lands. No
  stub pretends otherwise.

## 8. Requirements

- **F25-01** — *not* marked complete. The contract exists and is proven across three surfaces, but
  the criterion it serves is not met.
- **F25-02** — *not* marked complete. It names four surfaces including one hibernating cloud
  reference backend; three ran.

## 9. Verification

- `cargo nextest run -p wcore-exec-backend --no-fail-fast` — **33 passed, 1 skipped** (hetzner-dsm).
- `cargo clippy -p wcore-exec-backend --all-targets --all-features -- -D warnings` — clean.
- `cargo build --release -p wcore-cli --bin wayland-core` — clean; the binary answers
  `backend list` on hetzner-dsm.
- Conformance matrix: local PASS (15 checks), container PASS (15 checks), ssh and cloud reported
  UNEXERCISED **with reasons** — never silently skipped.

## Self-Check: PASSED

All named files exist in the worktree; all commits are present on `frontier/p25-remote-reach`
(`d0fc5095`, `7432a897`, `995e29b1`, `09e9949f`, `574364cc`, `3b686fe4`, `5fc08ae5`).
