# Condition 1 — `worktree_manager.rs:235`: DECIDED

**Decision: OPTION A — remove `retained_worker_count` from the authoritative
effect surface. Condition 1 is FULLY MET.**

**Basis: majority, 3 of 4** (Gemini 3.1 Pro: A; Kimi K3: A; internal adversarial
pass: A; Codex 5.6 Sol: B). Recorded as an explicit amendment to the claim
model's Condition 1, with the evidence below — **not** as a silent rounding-up.
That process requirement is Codex's, adopted from the minority position, and
both A-voters independently named it as the correct handling in their own
counter-argument. Raw panel output: `22-03-EVIDENCE/decision/wire/`.

---

## The condition, verbatim, so it is graded and not paraphrased

> **Condition 1.** Ship only once every *authoritative* effect path is routed
> through a structurally guarded commit API that requires the claim epoch and
> compares it atomically against the ledger's committed epoch. Status and
> diagnostic writes are exempt **only** if they cannot affect completion,
> reassignment, accounting, or dependency release — which means
> `worktree_manager.rs:235` (reservation accounting) is **not** exempt. A
> half-closed fence is worse than an open one because it manufactures confidence.

## What is actually at that line

Verified with `git show 91979ec8:crates/wcore-swarm/src/worktree_manager.rs`,
the commit the condition was written against. Line 235 is the first line of:

```rust
pub fn retained_worker_count(&self, stop_after: usize) -> Result<usize>
```

Four measured facts, none of them assumed:

| # | Fact | How it was established |
|---|---|---|
| F1 | It **writes nothing**. `&self`, a `read_dir`, returns a `usize`. | Read at the cited commit. The decision record's label "reservation accounting *write path*" is inaccurate for the code at that line. |
| F2 | Its only non-test caller is `Swarm::dispatch` (`lib.rs:312`), feeding `DispatchLimits::admit(count, retained)` — a process-wide admission gate evaluated **once, before any worker process exists**, under the `dispatch_gate` mutex. | `grep -rn retained_worker_count crates` → one production call site. |
| F3 | **No task owner can reach it.** Swarm workers are separate OS processes running an arbitrary `--worker-command`; they do not link `wcore-swarm`. Their sandbox manifest sets `fs_write_allow = [checkout, scratch]` and `network = Deny`, so a worker cannot write the journal or any sibling's state either. | `dispatch.rs::worker_manifest`. |
| F4 | The resource it counts is reclaimed by `reclaim_abandoned_transactions()`, discriminating on the transaction's own **kernel-enforced `flock` lease** — released by the OS only when the holder exits. A dead owner's workspace is reclaimed; a **live-but-superseded** owner's is not, and keeps being counted. No timeout, age or heartbeat heuristic. | `lib.rs:305`, F-2's fix. |

## Why A rather than B or C

A fencing token exists to refuse **a superseded owner's write**. Here there is
no write and no owner: the call happens in the dispatching parent before any
owner exists, and returns a number. Requiring `&TaskAuthority` on it would be
requiring a claim epoch from a caller that holds no claim, to guard a read that
commits nothing. It is not that closing it is *hard* — it is that there is
nothing at this line for the mechanism to act on.

The exemption clause turns on "cannot affect completion, reassignment,
accounting, or dependency release". The accounting that *can* affect those is the
**budget reservation** accounting, and that is fenced: `require_live_epoch` plus
the per-task reservation-ceiling check, both in the reducer at the durable
boundary (`reducer.rs`, `GoalTaskTransition::Claimed`). What
`retained_worker_count` feeds is a **disk-retention evidence quota** and a
per-worker output-byte budget — `DispatchLimits::admit` returns exactly one
field, `worker_stream_bytes`. Neither can move a completion, a reassignment, a
dependency release, or a token.

This is the same reduction-by-removal already accepted for `heartbeat.rs:56`,
and it is cleaner: that one needed a showing that nothing *reads* it for an
authoritative decision, whereas this one is not a write in the first place.

## The dependency constraint, restated so it is not mistaken for the reason

`TaskAuthority` must be unforgeable, so it lives where it is constructed
(`wcore-agent`). `wcore-swarm` sits **below** `wcore-agent`, so this path cannot
require a `&TaskAuthority` without an upward edge the crate map forbids; and
moving the type to `wcore-types` does not help, because a constructor public
enough for `wcore-agent` is public enough for anyone, and the unforgeability *is*
the mechanism.

All of that is true, and **it is not why this is closed.** It is closed because
F1–F4 show the path is not authoritative. Had the path been authoritative, the
dependency constraint would have been a reason to do the work, not a reason to
grant an exemption. Recording the distinction because the previous report leaned
on the constraint, and a constraint is not an argument.

## The panel's unanimous objection — and the measurement that DISPROVES it

All four members raised the same sub-question and all four answered it the same
way: `dispatch_gate` is a per-process `tokio::Mutex`, so two `wayland-core swarm`
**processes** on one repository both read the retained count before either has
created its worktrees, and both are admitted against a stale value — over-admitting
past `MAX_RETAINED_WORKTREES`. Kimi bounded the damage at "roughly one parent's
worth"; Gemini called it a clear TOCTOU; Codex asked for a repository-wide lock.

**It does not reproduce.** Measured on Linux with the shipped binary, two
concurrent `swarm --workers 6` dispatches against one repository
(`22-03-EVIDENCE/wire-live/linux/crossproc.txt`):

```
MAX_RETAINED_WORKTREES = 256
A_EXIT=1  B_EXIT=0  PEAK_RETAINED_ROOTS=8      (12 requested across two processes)
per-worker refusals: "worktree create: dispatch admission refused:
                      aggregate workspace budget exhausted"
```

Two things the panel — including my own pass — got wrong by reasoning instead of
measuring:

1. **The retained count is not the binding gate.** `MAX_RETAINED_WORKTREES` is
   **256**. At any width the swarm CLI permits, that gate is never close to
   binding, so a stale read of it changes nothing.
2. **The gate that binds is a different one, and it fails closed under
   concurrency.** `reserved_workspace_bytes()` is re-read from on-disk
   reservation files at **each workspace creation**, not once at admission. The
   excess workers were refused individually, at creation, in both processes.
   Peak concurrent roots was 8 of 12 requested and one process exited 1.

So the hazard four reviewers agreed on is bounded by a mechanism none of them
looked at. This is exactly the case the standing rule is about: live-test the
decision wherever it is testable rather than reasoning about it. Filing it as a
finding would have been a false finding.

## What is NOT claimed

* The enumeration remains a **lower bound**, as the original said. Ten paths were
  considered; the workspace is not exhaustively audited.
* `GoalFleetDriver` drives `FleetDispatcher` — the in-process sharded dispatcher
  named in 22-03's `files_modified` — and **not** `Swarm`'s worktree fanout. So
  `retained_worker_count` is not on the authoritative path of the wire this lane
  built at all; it is on the path of the separate `swarm` subcommand. That
  further weakens any claim it gates a Criterion-2 property, and it is also a
  limitation of the wire, stated in the summary rather than buried here.
* One real cross-process gap remains and is **not** this one: the journal's
  writer lease, which refuses a second opener, is `#[cfg(unix)]`-gated. On
  Windows two supervisors can hold one journal and only the epoch fence stands
  between them. Pre-existing, already recorded as threat T-22-06 in the phase
  verdict, and now pinned by
  `a_second_opener_is_refused_the_writer_lease_on_unix`.

## Dissent, recorded

**Codex 5.6 Sol voted B** — stays review-only, Condition 1 partially met:

> "Condition 1 names this exact path and explicitly declares reservation
> accounting non-exempt; correcting its classification from write to read does
> not erase its admission-accounting role. […] No concrete stale-authority attack
> supports C, so the condition should be amended explicitly rather than silently
> rounded up to fully met."

Its own strongest-counter line concedes the mechanism —

> "this side-effect-free, pre-owner read cannot commit a stale task effect, and
> adding a task-epoch check would prevent no demonstrated failure"

— so the split is about **process, not substance**: whether the condition may be
declared met by re-derivation, or must be amended in writing first. That is a
fair objection and this document is the amendment it asks for. Its cross-process
prediction is the one measurement disproved.
