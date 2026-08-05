---
phase: 22-supervision-durable-goals-fleet-loops
plan: "goal-kernel"
subsystem: durable-goals
tags: [kernel, journal, reducer, crash-recovery, claim-model, live-proof]
requires:
  - "22-01"
  - "22-02"
provides:
  - the durable Goal kernel that waves 3 and 4 were blocked on
  - the F12 non-regression canary 22-01 named and did not build
  - the committed claim-revocation model, decided on a measured four-way panel
affects:
  - crates/wcore-agent
tech-stack:
  added: []
  patterns:
    - "durable record carries a digest over its own envelope so replay reconstructs exactly or refuses"
    - "sole-writer enforced by the public-append denylist, mirroring child-transaction authority events"
key-files:
  created:
    - crates/wcore-agent/src/goal/mod.rs
    - crates/wcore-agent/src/goal/kernel.rs
    - crates/wcore-agent/src/goal/record.rs
    - crates/wcore-agent/tests/goal_kernel_test.rs
    - crates/wcore-agent/tests/goal_journal_compat_test.rs
    - crates/wcore-agent/examples/p22_goal_live.rs
  modified:
    - crates/wcore-agent/src/session_journal/model.rs
    - crates/wcore-agent/src/session_journal/reducer.rs
    - crates/wcore-agent/src/session_journal.rs
    - crates/wcore-agent/src/lib.rs
decisions:
  - "Claim model = lease-with-fencing-token, basis majority 4-of-4, with two binding conditions"
  - "No goal/terminal.rs in wcore-agent: the taxonomy lives once in wcore-types per 22-01"
  - "22-03 Tasks 3 and 4 NOT built - the ledger's live gate is blocked by F-2 and F-3"
metrics:
  duration: one session
  completed: 2026-07-27
status: partial
---

# Phase 22 — Durable Goal Kernel — Summary

**Lane branch:** `lane/22-goal-kernel` · **HEAD at proof:** `91979ec8`

The blocker is cleared. `crates/wcore-agent/src/goal/` exists, the durable state
machine is built on the record shape 22-01 authorized, and it survives a real
`kill -9` and resumes from its ledger on the real Linux host. The claim model is
decided with its dissent. **22-03 Tasks 3 and 4 are NOT built, and the reason is
a measured blocking dependency, not a budget excuse.**

---

## 1. What landed

### The kernel (F22-02)

Six durable transitions entering additively at schema 5 — `GoalOpened`,
`GoalIterationStarted`, `GoalWaitBegun`, `GoalWaitResolved`, `GoalRunResumed`,
`GoalTerminated` — folded by a new reducer arm into a `goals` map on
`ReducedSessionState`, with `GoalKernel` as the sole writer.

Four properties are structural rather than conventional:

| Property | How it is made structural |
|---|---|
| Sole writer | `SessionJournal::append` refuses every Goal variant, exactly as it does the child-transaction authority events. A transition with no attributable kernel append cannot exist (T-22-04). |
| Cursor not forgeable | Goal events reduce through a cursor-bearing path that takes the envelope's own `seq`/`checksum`, so the recovery cursor is derived by the reducer, never supplied by the event author. Follows the `ChildTransactionOpened` precedent. |
| Authority reconstruct-or-refuse | The durable record carries a digest over its envelope fields. A resume that cannot reproduce it — or whose parent envelope has moved — parks as `AuthorityUnreconstructable` instead of re-deriving (T-22-02). |
| Verified unforgeable | The compiler closes the model-authored route (`HostGateObservation` is not `Deserialize`). The reducer closes the remaining one: a hand-built record cannot stamp `verified` on a strategy whose verification owner is a model judge (T-22-03). |

No second store, no second reducer, no second cursor, no sidecar file. Appends
go through `append_built_from_head` — the same primitive Phase 21 repaired for
the parallel-sibling budget TOCTOU (`1eb9b5ca`), which 22-03's own record names
as the seam to re-enter rather than duplicate.

### The F12 non-regression canary (Criterion 5's missing half)

22-01 graded itself: *"The corpus is retained as evidence but no test pins its
reduction… a snapshot is not a canary."* Built now.
`goal_journal_compat_test.rs` reduces the real **82,367-byte** journal the real
release binary wrote at `2ecdfdf5` and pins the digest.

**The pin's provenance is the point:** `4f5713e2a625…` was captured by running
this test against `cd5b4e9b`, **before** the kernel added a field — a pre-change
observation, not a value recomputed from the post-change binary.

---

## 2. Gate results, with real numbers

| Gate | Host | Result |
|---|---|---|
| `cargo fmt --all -- --check` | Mac | **PASS** |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Linux | **PASS**, 0 errors |
| `cargo build --release --locked -p wcore-agent --example …` | Linux | **PASS** (`--locked` works; Cargo.lock untouched) |
| `goal_kernel_test` | Linux | **10 passed, 0 failed** |
| `goal_journal_compat_test` | Linux | **2 passed, 0 failed** |
| Live kill/restart harness | Linux | **12 PASS, 0 failures** |
| Windows clippy + goal suites | Windows | **NOT RUN — cold build still in flight at session end** |
| Full Linux aggregate `nextest --profile ci` | Linux | **NOT RUN** |

Proved at commit `91979ec8` with `dirty=0` — the build tree was attached to the
real pushed commit and rebuilt, after an earlier run whose evidence would have
cited a commit that did not contain the changes.

### Both load-bearing gates were verified to FAIL, not assumed to work

- Removing `skip_serializing_if` from the new `goals` field moved the corpus
  digest `4f5713e2…` → `ec12277d…` and the canary went **RED**. That is exactly
  the failure mode 22-01 warned about, now guarded.
- Neutralizing both `verified` guards turned the forgery test **RED** (9 passed,
  1 failed) and nothing else.

---

## 3. Live evidence — the bar was a crash, not a clean shutdown

Real release binary, real journal on disk, **two real `SIGKILL`s**, on
`hetzner-dsm`. Transcript: `22-GOAL-KERNEL-EVIDENCE/linux-kill-restart.log`.

```
kill -9 120419 at 2026-07-27T02:07:06Z   (writer mid-flight, lease held)
GOAL-LIVE: RESUMED iterations=1 resumes=1 max_tokens=Some(500) max_cost_cents=Some(25) strategy=Anvil
kill -9 120737 at 2026-07-27T02:07:07Z   (second process genuinely holding the lease)
GOAL-LIVE: RESUMED iterations=1 resumes=2 …
GOAL-LIVE: PARKED terminal=AuthorityUnreconstructable { detail: "parent envelope digest moved…" }
GOAL-KERNEL-LIVE: failures=0
```

What that establishes: the iteration committed before the kill survived; the
authority envelope was restored **narrowed** (500, not the parent's 1000); the
objective replayed from the chain; a fresh process took the journal even though
`kill -9` left the writer lease file behind; the resume count is durable across
two crash cycles; and a moved parent envelope **parks** rather than resuming
permissively — with the park surviving into a further process, so it is durable
and not an in-memory verdict.

**The honest limit, stated plainly.** This is NOT the shipped `wayland-core`
binary. No user-reachable Goal surface exists yet — that is plan 22-04
(`goal_cmd.rs`), which this lane was told not to execute and which is itself
blocked on this kernel. It is a real separate process running the real kernel
against a real on-disk journal with a real uncatchable signal, which is the
strongest honest live proof available at this commit. A shipped-binary Goal
resume remains unproven and is 22-04's to close.

---

## 4. The claim model — decided, with dissent

**Committed: `lease-with-fencing-token`. Basis `majority`, 4 of 4.**
`dup_window_ms=30000`, `lost_window_ms=0`, `fencing=yes`.
Full record in `22-03-CLAIM-MODEL.md`; evidence in `22-03-EVIDENCE/decision/`.

The three measurable cons were **measured on the real hosts before the panel saw
them**:

| Measurement | Result | Effect on the decision |
|---|---|---|
| Fencing surface | 6 paths, 4 structurally guardable, **2 review-only** | The winning option's own con is real, and the enumeration is a lower bound |
| Process identity | `binding=available` on **both** hosts | Killed `os-process-liveness`'s soundness objection — it still had no fencing half |
| Suspend behavior | 12s stop → **12/12 beats missed**, monotonic clock advanced 12s | Made heartbeat's mass-reassignment con concrete; showed ordering-based refusal is unaffected |

Decisive asymmetry: both revocation-only options record
`refuses_late_write=nothing`, so their windows measure duplicate *execution*;
only the lease-plus-epoch bounds duplicate *effect*. And because **the Windows
kill leg was never validly run**, an ordering-based refusal is the one mechanism
that does not depend on an unmeasured platform's process semantics — inferring
"descendants always die" from Linux alone is the exact error that produced F-1.

**Binding conditions carried from the panel, not decoration:**
1. Ship only once every *authoritative* effect path routes through a structurally
   guarded commit API requiring the claim epoch. Status/diagnostic writes are
   exempt only if they cannot affect completion, reassignment, accounting or
   dependency release — so `worktree_manager.rs:235` is **not** exempt.
2. The Windows kill leg must be validly run before Criterion 2 is claimed on both
   platforms.

**Closest rival was `escalate`,** and it lost only on its own definition — it is
reserved for a mechanism outside the plan's scope, and what the evidence shows is
two open defects *inside* scope. The dissent names all four options.

---

## 5. What was NOT done, and why

- **22-03 Task 3 (the ledger): NOT BUILT.** Its live gate (Task 4) requires
  killing a fanout and restarting it. I verified **first-hand at my own commit**
  that this is impossible: `cleanup_all` is reachable only from an explicit
  `Swarm::cleanup()` (`lib.rs:377`) and nothing invokes it at dispatch admission,
  so orphaned reservations still exhaust the aggregate budget and admission
  refuses at `worktree_manager.rs:396`. That is **F-2, HIGH, open, owned by
  another agent**. **F-3** (workers fail by elapsed time, 1/4 at 10s) compounds
  it. A ledger shipped now would be unit-tested code whose live gate cannot run —
  which this brief says proves nothing.
- **22-03 Task 4 (live fleet kill/restart): NOT RUN**, same reason.
- **Windows leg for the kernel: NOT RUN.** A dedicated worktree `C:\p22gk` was
  created at `91979ec8` and a cold build started, but `target/` did not exist at
  session end. Recorded unmeasured rather than inferred from Linux. Threat
  T-22-06 (Windows byte-range lock semantics under the `#[cfg(unix)]`-gated
  lease) therefore stays **open** — and my live proof depends on exactly that
  lease being recoverable after `kill -9`, so this is a real gap, not a formality.
- **Full Linux aggregate: NOT RUN.** Workspace clippy `--all-targets` passed,
  which compiles every test target, but the aggregate test run was not executed.
- **No `goal/terminal.rs`.** 22-01's plan named it, but the canonical taxonomy
  already shipped in `wcore_types::goal`. Creating a second one in the agent crate
  is the parallel vocabulary this phase exists to remove. Deviation recorded.

---

## 6. Deviations

- **[design] `goal/terminal.rs` deliberately not created** — see above.
- **[Rule 2] Loop bound enforced in the reducer.** `LoopPolicy` was recorded but
  nothing enforced it. A bound that is stored and ignored is not a bound, so
  `GoalIterationStarted` is refused past the recorded ceiling, at the durable
  boundary rather than in whichever process happens to be driving.
- **[scope] `22-03-EVIDENCE/baseline/INDEX.txt` written by this lane.** Task 1's
  index did not exist; the previous lane recorded its findings in prose. The
  `OPTION:` lines are labelled **design parameters**, not measurements, in both
  the index and the bundle the panel judged.
- **Gate clause reported unmet.** Task 2's gate binds the commitment to both
  baseline legs recording `RAN`. Windows is `NOT-RUN`, so that clause is **not
  satisfied** — recorded as unmet rather than satisfied by writing a line nobody
  measured.

---

## 7. Honest verdict

**F22-02: substantially complete on Linux, unproven on Windows.** The durable
kernel exists, owns objective, authority snapshot, evidence, cursor, wait and
terminal state, is the sole writer of those transitions, and survives an
uncatchable kill in a real process. Windows is unmeasured.

**Criterion 5: materially advanced.** The regression guard 22-01 called missing
now exists, is pinned to a pre-change observation, and was proved able to go red.

**Criterion 2: still FAILED, and this lane does not claim otherwise.** The claim
model is decided and the mechanism is authorized, but no ledger is built and the
live exercise is blocked behind two HIGH defects owned by another agent.

**Criteria 1, 3, 4: untouched by this lane.** They are 22-02 Task 3 and 22-04,
both of which are now genuinely unblocked by this kernel — which was the point.
