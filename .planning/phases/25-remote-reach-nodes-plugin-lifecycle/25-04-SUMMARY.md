---
phase: 25-remote-reach-nodes-plugin-lifecycle
plan: "04"
subsystem: fail-closed-and-orphans
tags: [f25-05, orphan-scanner, fail-closed, false-negative, containment]
status: complete
termination_state: 2
requires:
  - wcore-exec-backend contract + receipt (25-01)
  - plugin approval gate (25-02)
  - node registry + attribution (25-03)
provides:
  - wcore-exec-backend::orphan — a scanner whose zero is only a number when something looked
  - the hostile fail-closed matrix across every reference backend
  - `wayland-core backend scan` operator surface
affects:
  - crates/wcore-exec-backend (new orphan module; local backend scan widened)
  - crates/wcore-cli (one new backend subcommand)
tech-stack:
  added: []
  patterns: [observed-not-assumed, instrument-self-test, unrepresentable-bad-state, independent-cross-check]
key-files:
  created:
    - crates/wcore-exec-backend/src/orphan.rs
    - crates/wcore-exec-backend/tests/fail_closed_matrix.rs
    - .planning/phases/25-remote-reach-nodes-plugin-lifecycle/25-04-FAIL-CLOSED-EVIDENCE.md
    - .planning/phases/25-remote-reach-nodes-plugin-lifecycle/evidence/25-04-WINDOWS-FALSE-ZERO.md
  modified:
    - crates/wcore-exec-backend/src/{lib,backends/local}.rs
    - crates/wcore-cli/src/backend.rs
decisions:
  - "ProcessTableScan is Enumerated | CannotDetermine with no count(), so 'could not look' is unrepresentable as zero."
  - "The enumeration self-tests against this process's own row, because Win32_Process.CommandLine can be NULL."
  - "SSH is recorded BestEffort and cloud None; ProcessTreeMechanism has no variant for either."
  - "The receipt was NOT given an embedded orphan field — see §6."
metrics:
  tests_added: 26
  new_third_party_crates: 0
  defects_found_live: 3
  panel_members: 0
completed: 2026-07-27
---

# Phase 25 Plan 04: Hostile Fail-Closed Matrix + Orphan Scanner — Summary

All five hostile cases fail closed on **both** hosts, and the orphan scanner is now backed
by an independent enumeration that agrees with it **in the state where it could disagree**.

**Success Criterion 4 is NOT MET.** Two of the four reference surfaces — SSH and cloud —
cannot be enumerated on the proof hosts, so their orphan counts are `NOT MEASURED`, and a
criterion that says "across every reference backend" is not satisfied by two.

**Termination state 2: complete with findings.** Three HIGH findings, all fixed and
re-proved.

---

## 1. The five hostile cases

Every compromise induced for real; each capture records its command, output and exit.

| Case | `hetzner-dsm` | `SeanDesktop` |
|---|---|---|
| ROTATED-KEY | REFUSED exit=1 | REFUSED exit=1 |
| TAMPERED-BUNDLE | REFUSED exit=1 | REFUSED exit=1 |
| ATTESTATION-MISMATCH | REFUSED exit=1 | REFUSED exit=1 |
| DENIED-SECRET | REFUSED exit=1, **no receipt produced** | REFUSED exit=1 |
| DENIED-EGRESS | REFUSED exit=1 | REFUSED exit=1 |

No fallback anywhere: `SubmissionVerdict` has no `Rerouted` variant, so the type cannot
express one, and the matrix asserts the negative — that no other backend and no other node
picked the work up.

## 2. Three HIGH findings, none of them a crash

All three were **false answers**. Every one was found by driving the real thing and by
running the independent enumeration as a *check on* the scanner rather than a copy of it.

**2.1 — The local scan could not see an orphan at all.** It consulted only the live-task
registry, and a terminal event *removes* the registry entry, so a process that outlived its
task was by construction invisible. Measured: independent `ps` found 1, scanner said 0.

**2.2 — Fixing that made the scanner count itself.** `backend scan --task-id <nonce>`
carries the nonce on its own argv. Measured: scanner 1, independent 0, and the row was the
scanner. This is the *same defect plan 25-01 already hit remotely*, recurring the moment
the local scan started reading the real process table.

**2.3 — The Windows scanner reported a MEASURED ZERO while an orphan ran.**
`tasklist` does not print command lines at all. Full red/green write-up:
`evidence/25-04-WINDOWS-FALSE-ZERO.md`.

```
RED  (tasklist):        planted=1  scannerPlanted=0
GREEN (Win32_Process):  planted=1  scannerPlanted=1
```

A false negative in containment is the worst output this module can produce — strictly
worse than an error, because a measured zero *reads as proof of correctness*.

## 3. What the fix actually is, beyond swapping the instrument

Swapping `tasklist` for `Win32_Process` fixed *this* instrument and not the *failure mode*.
`Win32_Process.CommandLine` returns NULL without sufficient privilege, and an enumeration
that "succeeds" with every command line blank reproduces the identical false zero with a
different tool.

So the instrument **self-tests**: this process's own row must be present *and* carry a
non-empty command line. We know our own pid and we know we have a command line; if we
cannot see our own, we cannot see anyone's. And the type makes the bad answer
unrepresentable:

```rust
pub enum ProcessTableScan { Enumerated { rows: Vec<String> }, CannotDetermine { reason: String } }
```

There is deliberately **no `count()`**. The only path to a number is the `Enumerated` arm,
so "could not look" cannot be rendered as `0` — not discouraged, *unrepresentable*. It
flows to `OrphanEvidence::unobserved`, which prints `NOT MEASURED — <reason>`.

The NULL-`CommandLine` case cannot be induced on a Linux CI box, so it is pinned by five
unit tests over captured output shapes. "We could not reproduce it so we did not test it"
is how it survived the first time.

## 4. The evidence gate was itself self-passing

Worth separating out, because it is the more instructive half. The first Windows run wrote:

```
F25-SC4-SCANNER-AGREEMENT: AGREE scanner=0 manual=0
```

That comparison ran **only after the reap**, where both sides are legitimately zero — so it
agreed while the scanner was structurally incapable of returning anything else. A gate
available only in the state where both sides are zero cannot detect a scanner that always
says zero. It is the "already green at base" failure class, sitting inside this plan's own
evidence.

Both ledgers now carry `SCANNER-AGREEMENT-PLANTED`, taken while the orphan is planted, and
a run in which the plant never appeared is recorded DISAGREE rather than passing vacuously.

```
Linux:   AGREEMENT-PLANTED: AGREE scanner=1 manual=1   AGREEMENT: AGREE scanner=0 manual=0
Windows: AGREEMENT-PLANTED: AGREE scanner=1 manual=1   AGREEMENT: AGREE scanner=0 manual=0
```

## 5. Re-measurement list

Bounded and verified by inspection.

| Consumer | Status |
|---|---|
| `wayland-core backend scan` (Windows) | **RE-MEASURED.** False-zero window `f846e471` → `b0bb30d5`, both bounds inside this plan. |
| `orphan::{scan_all, scan_one}` (Windows) | **RE-MEASURED**, same window. |
| 25-04 Windows ledger, first run | **SUPERSEDED AND RE-MEASURED**; the spurious `AGREE scanner=0 manual=0` is replaced. |

**Verified NOT affected:** 25-01's cancellation/zero-residual proof (Linux only; its SUMMARY
records "No Windows leg"); 25-02 and 25-03 (no orphan claims);
`wcore-browser::supervisor::process_alive` and
`wcore-exec-backend::backends::local::process_alive` (both shell to `tasklist` but filter on
**PID**, a column it does print — the defect was specific to filtering on a command line);
everything outside this plan (the scanner is new in `f846e471`).

## 6. Deviations from the plan

**[Scope — not done] the receipt was NOT given an embedded orphan field.** The plan asks for
the scan to feed `ProcessEvidenceV1`-shaped evidence into the receipt. It does not, and the
reason is the same one the rest of this module is built on: a receipt is sealed at task
completion, and an orphan is by definition something that outlives the task. A count taken
at seal time would systematically under-report — it would be a *measured* number that
structurally could not see the thing it names, which is the exact defect class in §2.3.
Embedding it also needs every backend's `execute` path to become scan-aware, i.e. a change
to three completed plans' production code for a field no criterion reads. The evidence lives
in the scan artifact instead, where its timing is honest. Recorded as an explicit gap.

**[Finding — HIGH, fixed] `crates/wcore-exec-backend/src/backends/local.rs`** was changed
although the plan lists only `mod.rs`, `ssh.rs` and `cloud.rs`. This is finding 2.1 and it
is recorded as a finding with its cause, not a routine edit, exactly as the plan requires.

**Termination state 4 was NOT triggered.** No fix here changes what the sandbox permits;
all three findings were in the *observation* layer, not the containment layer.

## 7. Gate results

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` (Mac) | clean |
| `cargo clippy -p wcore-exec-backend -p wcore-cli --all-targets --all-features -- -D warnings` | clean |
| `cargo nextest run -p wcore-exec-backend --test-threads=1 --no-fail-fast` | **108 passed**, 1 skipped, run serially |
| `cargo nextest run -p wcore-cli --test plugin_lifecycle_cli` | 14 passed (25-02 unbroken) |
| `cargo nextest run -p wcore-exec-backend --test node_contract` | 18 passed (25-03 unbroken) |
| `wayland-core backend scan --help` | present on the shipped binary, both hosts |
| `Cargo.lock` | untouched by this plan |

## 8. Why Success Criterion 4 is NOT MET

The criterion says compromised keys/plugins/backends and denied secret/egress paths fail
closed **across every reference backend**, with no orphaned execution.

- The five hostile cases **do** fail closed, on both hosts, with named verdicts and nonzero
  exits and no fallback. That half holds.
- The no-orphan half holds for **local** and **container** only. **SSH** and **cloud**
  report `NOT MEASURED` — `WAYLAND_EXEC_SSH_TARGET` is unset and no cloud credential
  exists — and those two are precisely the backends that inherit *no* proven reaping
  mechanism, so they are the ones an orphan claim would most need to cover.

Two of four surfaces unmeasured is not "every reference backend". Reporting them as
`NOT MEASURED` rather than `0` is the correct behaviour and is exactly why the criterion
can be graded honestly at all.

## 9. Known gaps

- **SSH orphan enumeration unexercised**; mechanism recorded `BEST-EFFORT` (no
  `ProcessTreeMechanism` variant crosses an ssh connection).
- **Cloud orphan enumeration unexercised**; mechanism recorded `NONE`.
- **No real egress *policy* denial.** No policy is installed on either host and the only
  credentialed egress surface has no credential, so no request is ever attempted. What is
  proven is that the surface fails closed, not that a policy denied an attempted request.
- **The Windows Job Object reaping mechanism is not proven by this plan** and is not
  claimed. See `evidence/25-04-windows-known-red.txt`: the Phase 25 Windows orphan claim is
  an *observation* and is INDEPENDENT of the escalated
  `live_future_drop_reaps_descendant_job_tree`. A Windows `orphans: 0` means "nothing was
  left behind in this run", not "the mechanism is proven".

## 10. Backlog candidates (MEDIUM and below)

- `[MED]` `process_alive` on Windows does `stdout.contains(&pid.to_string())` — a substring
  match, so pid `123` matches a row for pid `1234`. Pre-existing, in both
  `wcore-browser` and `wcore-exec-backend`. It over-reports liveness, i.e. it would produce
  a *phantom* orphan rather than hide a real one, which is the safe direction — but it is
  wrong and should be an exact field match.
- `[MED]` Embedded receipt orphan evidence, if a consumer ever needs it, with the timing
  problem in §6 solved first.
- `[LOW]` Cleanup on `hetzner-dsm`: `/root/f25-04-lab`, `/root/f25-04-evidence`,
  `/root/f25-04-state`; on `SeanDesktop`: `C:\f25-04-lab`, `C:\f25-04-evidence`,
  `C:\f25-04-state`, `C:\f25-04-*.json`.

## Self-Check: PASSED

All named files exist in the worktree; all commits are present on `lane/25`.
