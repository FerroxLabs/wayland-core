# Phase 25 — status at hand-back (2026-07-26)

**The phase goal is NOT achieved.** One of four plans executed. This document grades every
Success Criterion in `ROADMAP.md` verbatim and says plainly what was not done.

Branch: `frontier/p25-remote-reach` (worktree of `waylandcore-ferrox`, based on
`plan/f20-unified-audit-repair` @ `1058965e`).

---

## Plans

| plan | wave | state |
|---|---|---|
| 25-01 — execution-backend contract + four reference backends | 1 | **COMPLETE, termination state 2** (bounded cloud gap). `25-01-SUMMARY.md` |
| 25-02 — twelve-verb plugin lifecycle | 1 | **NOT STARTED** |
| 25-03 — node/device contract | 2 | **NOT STARTED** |
| 25-04 — hostile fail-closed matrix + orphan scanner | 3 | **NOT STARTED** |

25-02 was read in full and scoped before the decision to stop. It requires ten new CLI verbs, a
retained-generation install layout, a signing path bound to the existing Ed25519 trust root, a
loader-level approval gate in `wcore-agent`, and a live transcript on two operating systems. That
is comparable in size to everything 25-01 delivered. Building half of it — particularly half of
the approval gate, which the plan itself calls "a security gate, not a confirmation prompt" —
would have put unproven security-relevant code in the tree and produced verbs that print success
while changing nothing. The plan's own execution rules name that as the forbidden move. Stopping
and reporting is the correct outcome, not a failure to try.

---

## Success Criteria, graded verbatim

> **1. The same task runs locally, in a container, over SSH, and on one hibernating cloud backend
> with equivalent policy, receipts, cancellation, and cleanup.**

**NOT MET.** Three of four surfaces. The same deterministic task ran through the shipped
`wayland-core` binary on `hetzner-dsm` on local, container and ssh; all three receipts verify
individually; the normalized four-way diff reads **EQUIVALENT** with only backend identity,
transport and timing differing; cancellation reached a `Cancelled` terminal on all three with zero
residual confirmed against the real process table, the real container listing and the real remote
process table. The hibernating cloud leg is **UNEXERCISED** — no Fly credential exists on any
proof host, and the backend fails closed rather than falling back. The criterion names four
surfaces. Evidence: `25-01-EQUIVALENCE-EVIDENCE.md`.

Second, smaller shortfall recorded rather than glossed: the ssh leg targeted a containerized sshd
on the same physical host. Separate network namespace, filesystem and process table, reached over
a real ssh connection with a real key — it proves the transport and remote-session cancellation,
not the cross-machine case. `backend.instance_id` being identical across all three receipts is the
evidence of that.

> **2. Nodes pair, advertise capability, revoke, recover offline, and handle mixed versions without
> losing authority attribution.**

**NOT MET — nothing was built.** Plan 25-03 was not started. No node contract, no pairing, no
capability advertisement, no revocation, no offline recovery, no mixed-version handling. Zero
evidence exists and none is claimed.

> **3. Plugins can be scaffolded, tested, signed, installed, approved, inspected, updated, rolled
> back, removed, published, and recovered.**

**NOT MET — two of eleven verbs exist, and they already existed.** `wayland-core plugin` today
offers `install`, `list`, `available`, `remove` and three `marketplace` verbs. Of the eleven the
criterion names, only **installed** and **removed** are possible. Scaffold, test, sign, approve,
inspect, update, rollback, publish and recover are all absent. Plan 25-02 was not started, so
nothing in this phase changed that.

> **4. Compromised keys/plugins/backends and denied secret/egress paths fail closed with no
> orphaned execution.**

**PARTIALLY EVIDENCED, NOT MET.** What was actually proven live, on `hetzner-dsm`:

- **No orphaned execution after an induced failure, on three backends.** Cancellation was driven
  through the shipped binary from a *second* process against a live task, and afterwards the real
  process table, the real container listing and the real remote process table were all empty.
  Pre-cancel the same scanner found the live work, so it is measured capable of finding something
  rather than merely of returning zero.
- **An absent credential fails closed.** `backend run --backend cloud` exits non-zero with
  `refusing to run and NOT falling back`, and never silently degrades to a local backend.
- **A tampered receipt and an unpinned backend identity are both rejected**, proven in the
  conformance harness against a pinned key.
- **An unscannable surface is reported as `enumerated=false`, never as zero orphans.**

What is NOT proven, and therefore what keeps this criterion unmet: compromised *keys*, compromised
*plugins*, compromised *backend attestation*, denied *secret* paths, denied *egress* paths, and
key rotation — none has a hostile case. And "across every reference backend" cannot hold while the
cloud backend is unexercised. Plan 25-04 was not started.

---

## What is genuinely broken or unresolved

1. **The cloud credential is the only thing standing between Criterion 1 and MET on all four
   surfaces.** Everything else for that leg is built. Exact closing command and exactly what to
   mint — one throwaway Fly org, one token scoped to it, nothing wider — are in
   `evidence/25-01-cloud-credential-probe.txt`. **Reserved to Sean.**
2. **`SandboxBackend` exposes no handle a cross-process cancel can signal.** Both its buffered and
   streaming entry points own the child internally, so the local reference backend owns its own
   process group instead of routing through the containment trait, and says so in its effective
   policy rather than implying containment it did not apply. Closing this is a `wcore-sandbox`
   change and is out of Phase 25's fence.
3. **The M5.1 plugin-API sandbox allowlist is a real open architectural question.**
   `wcore-plugin-api` deliberately depends on `wcore-sandbox` so `PluginContext` can hand plugins a
   `SandboxRegistry` handle. The 25-01 plan believed this was an oversight in
   `FORBIDDEN_CORE_IMPORTS`; it is not, and adding it fails the build. Whether a plugin should hold
   that handle at all belongs to whoever owns the M5.1 decision.
4. **Plugin-declared execution backends are captured but never reified.** The mirror and the host
   registrar exist so the isolation boundary is right, but a plugin-declared backend describes a
   transport the host has no factory for. No stub pretends otherwise.
5. **No Windows leg for anything in this phase.** No `wayland-core` build exists on `SeanDesktop`
   in this window. Windows parity was measured for the *vendor API reachability* only.

## Backlog candidates (MEDIUM and below, non-blocking)

Filed here rather than in `.planning/BACKLOG.md`, which is fenced for this wave — the integrator
should move them.

- `[MED]` `[execution_backends]` configuration keys were not added to `wcore-config/src/config.rs`.
  The crate reads its state dir and targets from environment names instead. No criterion depends
  on it.
- `[MED]` `wcore-eval-scenarios` has no test asserting a *production* receipt against the F04
  oracle's own verifier. `receipt.rs` reimplements every oracle rule and tests them, but the two
  are not cross-checked by one executable.
- `[LOW]` The container reference backend hardcodes `docker.io/library/busybox:1.36` unless
  `WAYLAND_EXEC_CONTAINER_IMAGE` overrides it; the image is not pinned by digest.
- `[LOW]` A disposable `wayland-f25-sshd` container and `/root/f25-ssh`, `/root/f25-state`,
  `/root/f25-evidence`, `/root/wayland-p25` remain on `hetzner-dsm`. They are named for this phase
  and safe to remove.
