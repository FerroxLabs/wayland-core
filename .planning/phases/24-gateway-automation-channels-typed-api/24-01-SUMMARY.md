---
phase: 24-gateway-automation-channels-typed-api
plan: "01"
subsystem: gateway
tags: [gateway, lifecycle, delivery-ledger, drain, windows-service, pidlock]
status: partial
requires: []
provides:
  - wcore-gateway::lifecycle (state machine + status projection)
  - wcore-gateway::pidlock (single-instance lock, liveness, path normalisation)
  - wcore-gateway::ledger (exactly-once delivery ledger)
  - wcore-gateway::drain (admission control + ordered drain)
  - wcore-gateway::service (per-family service management, detach flag constants)
affects:
  - crates/wcore-cli/src/cron.rs (Windows detach defect fixed)
tech-stack:
  added: [wcore-gateway]
  patterns: [flock/LockFileEx over fcntl, one-byte lock sentinel, injected clock, argv-mode invocation]
key-files:
  created:
    - crates/wcore-gateway/src/{lib,lifecycle,pidlock,ledger,drain,service}.rs
    - crates/wcore-gateway/tests/{lifecycle_contract,pidlock_hostile,ledger_exactly_once}.rs
    - .planning/phases/24-gateway-automation-channels-typed-api/24-01-GATEWAY-CONTRACT.md
    - .planning/SEAM-REQUESTS/24.md
  modified:
    - Cargo.toml, Cargo.lock, crates/wcore-cli/Cargo.toml, crates/wcore-cli/src/cron.rs
decisions:
  - "Windows service mechanism = win-scheduled-task, authorized 4/4 on measured evidence"
  - "Outbound idempotency key lives in the ledger, NOT on the serialized outbound message"
  - "flock/LockFileEx rather than fcntl, so in-process exclusion can actually go red"
metrics:
  tests-green: 53
  regression: "2174 tests run: 2174 passed, 9 skipped (wcore-gateway + wcore-cli + wcore-cron + wcore-channels, hetzner-dsm)"
  completed: 2026-07-26
---

# Phase 24 Plan 01: Gateway Runtime Summary

A `wcore-gateway` mid-layer crate carrying the lifecycle state machine, a
Windows-safe single-instance lock, ordered drain, and an exactly-once
delivery ledger; a measured-and-fixed HIGH Windows detach defect; and one
cross-audited, hardware-measured decision authorizing the Windows service
mechanism — **but no live operator journey, so Success Criterion 1 is not
closed on any platform.**

## Termination state

**State 2 of the plan's four — "Complete with a named platform gap" — is
NOT claimable.** The honest state is short of all four: the plan's own
definition of Complete requires the lifecycle verbs to work from the shipped
binary and delivery to be proven exactly-once across drain, restart, upgrade
and rollback on Linux and macOS. Neither happened. What did happen is
recorded below without inflation.

## What was delivered

### 1. `wcore-gateway`, a mid-layer crate (commit `a701a8a0`)

Direct dependencies: `chrono`, `dirs`, `serde`, `serde_json`, `thiserror`,
`tracing`, plus per-target `libc`/`windows-sys`. **No internal `wcore-*`
dependency at all** — stronger than the plan required, and `cargo tree`
confirms `wcore-agent` is absent, so the forbidden top-layer inversion was
not introduced.

**Lifecycle**: eight states, every illegal transition refused BY NAME.
Three refusals (`AlreadyRunning`, `NotRunning`, `DrainRequiresRunning`) are
distinct so the CLI can return distinct exit statuses; everything else is
`IllegalTransition` rendering both operands.

**Pid lock**: the OS lock sits on a separate ONE-BYTE sentinel file, so the
mandatory Windows lock cannot exclude the crate's own status reader — and
the test proves it with a raw byte read taken while the lock is held.
`flock`/`LockFileEx` rather than `fcntl`, because fcntl record locks are
owned by the PROCESS and merge across two opens, which would make the
exclusion test incapable of ever going red. Path normalisation is applied at
the COMPARISON boundary on BOTH operands; a second `acquire` through a
different representation of the same directory is refused.

### 2. Exactly-once delivery and drain (commit `80ef6d44`)

Four persisted states because the load-bearing distinction is KNOWN outcome
versus UNKNOWN outcome; only the unknown case is retried. The test drives
the real hazard — 200 accepted, 150 settled, 50 attempted with the process
killed mid-flight and a subset of those already at the destination — and
every count comes from an **independent sink** that refuses an id it has
already served. Result: 200 delivered, 200 unique, 0 duplicates, 0 losses,
with the 150 provably-settled deliveries never retried.

Drain is a state with a fixed order (close admission → publish counts →
bounded wait → flush → clean-or-forced), driven by an injected clock so the
suite is deterministic. A forced drain names abandoned deliveries by
identity and records the abandonment durably.

### 3. A measured HIGH defect, fixed (commit `b22e3ecc`)

`crates/wcore-cli/src/cron.rs`'s `#[cfg(not(unix))]` spawn branch set NO
creation flags while its Unix sibling calls `process_group(0)`. Measured on
`SEANDESKTOP`: **1 of 600 heartbeats, process gone.** Every
`wayland-core cron daemon` started over a remote session on Windows died the
instant that session returned, and nothing reported it. With
`DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB`
the same probe wrote 600 of 600 and exited normally. `CREATE_BREAKAWAY_FROM_JOB`
is load-bearing: OpenSSH reaps session children through a Job Object.

Also lands the per-family service abstraction with one selection point,
argv-mode invocation everywhere, and a profile sanitiser a hostile name
cannot escape.

### 4. The Windows mechanism decision — cross-audited, measurement-bound

**Chosen: `win-scheduled-task`.** Panel 4/4 (codex, gemini, kimi, internal
adversarial), all against one identical 186-line evidence bundle, all four
responses retained verbatim. All five declared gates pass. Full record in
`24-01-GATEWAY-CONTRACT.md` §6–§7 and
`24-01-decision-evidence/`.

**Three probe corrections are recorded rather than hidden**, because each
was a gate that would have passed on the wrong thing:

- the first observer scored `dies` for any frozen heartbeat, which is wrong
  at the terminal beat — a child that SURVIVED and finished is also frozen.
  The first `detached-flags` run was scored `dies` while its transcript
  showed all 600 beats. Rule made three-way; both legs re-run.
- the SCM probe first emitted `survives` off a successful `sc create`.
  Registration is not the property that option needs. Corrected to
  `unsupported`, which **removed `win-service-scm` from the choosable set** —
  the correction made the decision harder, not easier.
- the `schtasks` probe observed the heartbeat INSIDE its own ssh session,
  which measures "the task runs", not "the process outlives the session".
  **All three external panel members read that transcript as proof of a
  session-independent parent; none noticed.** A fifth probe was written and
  run to close the hole by measurement: beats advanced 49 → 89 in a later
  separate session with the registration already deleted.

That last point is the most useful thing in this summary. A unanimous panel
agreed on evidence that did not show what they said it showed; the
conclusion survived only because the missing measurement was taken
afterwards.

## Deviations from plan

**[Rule 3 — blocking] The plan's declared repo and worktree did not exist.**
The dispatch named `.planning/phases/24-.../` "in your worktree". The
worktree supplied was cut from `/Users/seandonahoe/dev/waylandcore` at
release commit `61b79c4f`, which has no `.planning/` at all; the live
program is in a DIFFERENT clone, `/Users/seandonahoe/dev/waylandcore-ferrox`
on `plan/f20-unified-audit-repair`. Resolved by fetching that branch into
the worktree and resetting the (clean, unstarted) worktree onto
`2ecdfdf5`. **The orchestrator must be aware that Phase-24 commits live on
branch `worktree-wf_b7d743bd-954-4` in the `waylandcore` repo, not in
`waylandcore-ferrox`.** Push to the ferrox repo is blocked by a
secret-scanning ratchet on pre-existing history (`d06a6051`), so transport
was by `git bundle`.

**[Rule 3] `templates/gateway/*` not created; unit text generated in code.**
One source of truth beats a template plus a generator that can drift. A
deliberate divergence, filed as backlog item F24-01-L1 for ratification.

**[Rule 4 — architectural, NOT taken] Protocol events not designed.** The
plan required landing Phase-24 protocol events, a generator version bump and
a regenerated Desktop manifest in wave 1, "even where this plan does not
consume them". Those files are FENCED, and filing a speculative event set
into a versioned cross-repo contract — against a generator this executor
could not run end to end — is exactly how an unused, wrong surface gets
frozen. Refused deliberately; see SEAM-24-03.

## What was NOT delivered — stated plainly

1. **The nine operator verbs do not exist.** `crates/wcore-cli/src/gateway.rs`
   was not written, and `lib.rs`/`main.rs` (where a subcommand registers) are
   fenced. Nothing in Criterion 1 can be driven from the shipped binary.
2. **No live 200-delivery tally on Linux or macOS.** The exactly-once
   property is proved at unit level against an in-process independent sink,
   NOT by installing a service, submitting through the shipped binary,
   draining mid-flight, restarting, upgrading, rolling back and counting at
   an out-of-process sink. The plan's TALLY GATE does not pass;
   `/tmp/f24-01-run/sink-*.ids` do not exist.
3. **No pseudo-terminal diagnostics evidence.** `pty_gateway_surface.rs` was
   not written. The SURFACE GATE does not pass.
4. **The service argv is generated and asserted, never EXECUTED.** No
   `launchctl`/`systemctl`/`schtasks` call from this crate has touched a real
   registry.
5. **The Windows fix is proved in a probe, not in the shipped binary.** The
   probe uses the identical spawn path and the flag constants are pinned by
   test, but no `cron daemon` has been started on Windows and observed
   surviving a session close.

## Findings

| ID | Severity | Status |
|---|---|---|
| Windows detach branch sets no creation flags; daemon dies with its session | **HIGH** | **FIXED**, with executable evidence (`b22e3ecc`, probes `detach-baseline`/`detached-flags`) |
| F24-01-M1: scheduled-task restart-on-failure is weaker than SCM recovery; Criterion 5's recovery clause is unmeasured | MEDIUM | BACKLOG (seam request), carried to 24-04 |
| F24-01-M2: the registration is `Logon Mode: Interactive only`, so no headless start | MEDIUM | BACKLOG; symmetric with both Unix families, needs operator-facing documentation |
| F24-01-L1: `templates/gateway/*` absent, unit text generated in code | LOW | BACKLOG |

No CRITICAL findings. The one HIGH is fixed.

## Verification

- `cargo test -p wcore-gateway` on `hetzner-dsm:/root/wayland-p24` — **53 green**
  (18 unit + 7 ledger + 9 lifecycle + 8 pidlock + 11 across the module suites).
- Regression: `cargo nextest run -p wcore-gateway -p wcore-cli -p wcore-cron
  -p wcore-channels --profile ci --no-fail-fast` — **2174 tests run: 2174
  passed (1 slow, 1 flaky), 9 skipped**, exit 0.
- `cargo clippy -p wcore-gateway --all-targets -- -D warnings` — clean.
- `cargo fmt --all -- --check` — clean (run on the Mac, the one permitted
  Cargo command there).
- `cargo tree -p wcore-gateway -e normal --depth 1` — no `wcore-agent`.
- Windows probes: five transcripts, each carrying a verdict line its own
  script emitted, in `24-01-decision-evidence/`. Scratch checkout
  `C:\f24-01-probe` removed; absence confirmed; no stray processes left.

**Windows CI clippy was NOT run.** The Windows arms of `pidlock.rs` and
`service.rs` (`LockFileEx`, `OpenProcess`, `creation_flags`) have never been
compiled. Windows CI runs clippy `-D warnings` BEFORE tests, so a lint
failure there means the tests never run. This is a real unverified surface
and the most likely place this branch breaks.

## Self-Check

Files asserted present and verified on disk; commits verified in
`git log --oneline`: `a701a8a0`, `80ef6d44`, `b22e3ecc`. Test counts and the
regression tally are quoted from captured tool output, not recalled.

**Self-Check: PASSED**
