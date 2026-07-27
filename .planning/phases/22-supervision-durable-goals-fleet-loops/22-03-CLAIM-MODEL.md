# 22-03 Claim Model — measured pre-ledger baseline

**Scope of this document, stated up front.** This is Task 1's deliverable: the
measured durability baseline of today's fanout. **Task 2 (the four-way panel) was
not run, and no claim-revocation model was chosen.** There are therefore no
`OPTION:` lines with duplicate-execution and lost-completion windows in this file.
Inventing them without the measurements behind them is exactly the forgery this
plan names, so they are absent rather than estimated.

What the baseline found instead is that the premise of Tasks 2–4 did not hold:
**today's fanout could not fan out, and a killed fanout cannot be restarted at
all.** Those had to be established before any claim model could be reasoned
about, and they consumed the task.

Hosts: `hetzner-dsm` (Linux) and `SeanD@seandesktop` (Windows).
Binaries: `de977949` (base, pre-fix) and `ba4e541e` (post-fix) on Linux;
`2ecdfdf5` on Windows.

---

## F-1 (HIGH, FIXED) — the fanout refused dispatch over its own worktree root

`WorktreeManager::new` mints `<repo>/.swarm-worktrees/` **inside** the repository
whose cleanliness `assert_clean()` judges. Git does not report an *empty*
untracked directory, so the root is invisible at construction; the moment the
first worker materializes a checkout inside it, `git status --porcelain` reports
`?? .swarm-worktrees/` and every later caller is refused.

Measured on Linux, real release binary, throwaway repo, **no `.gitignore`**:

| Binary | Width | Succeeded | Refused (dirty) | Exit |
|---|---|---|---|---|
| `de977949` pre-fix, run 1 | 8 | **1** | 7 | **0** |
| `de977949` pre-fix, run 2 | 8 | **0** | — | **1** (whole dispatch refused) |
| `ba4e541e` post-fix, runs 1–3 | 8 | **8, 8, 8** | 0 | 0 |

Two consequences, both silent:

* **effective parallelism was one.** `--workers 8` ran one worker and refused
  seven while the CLI still exited **0**.
* **restart was impossible.** The directory survives cleanup, so the *second*
  dispatch on any repository failed outright with zero workers. That is the
  restart path Success Criterion 2 depends on.

Single-variable root-cause proof: adding one line (`.swarm-worktrees/`) to
`.gitignore` on the pre-fix binary restored 8/8 on both runs. The only variable
changed was that line.

**Why the suite missed it:** every in-repo fixture first commits a `.gitignore`
naming `.swarm-worktrees/` — a workaround the shipping product never performs
(`worktree_tests.rs:941`, whose own comment says the manager mints its root
inside the repository). The suite proved the fixture, not the product.

Fixed in `f43c279c`; regression guard in `1fab91f7`. The guard was itself
self-passing on the first draft — asserting on a freshly constructed manager is a
tautology because the empty root is invisible to git — and was corrected only
after being verified to fail against a neutralized fix.

Confirmed on Windows too: pre-fix, `no-gitignore` run 2 → exit 1, refused. The
parallelism half did not reproduce there (run 1 gave 8/8) because it is a race
between checkout materialization and the sibling admission check, and Windows'
slower worker startup happened to win it. A race that one platform wins is not a
race that is absent.

---

## F-2 (HIGH, **FIXED 2026-07-27** on `lane/swarm-durability`) — a killed fanout cannot be restarted

> Status updated after this document was written. Fixed by `reclaim_abandoned_transactions()`
> at dispatch, discriminating on the transaction's own kernel-enforced `flock` lease rather than
> any age or heartbeat heuristic. Live: restart after `kill -9` of 8 workers went 0/8 to 8/8.
> Also worse than described below: **2** orphans exhaust the budget at width 8, not 8.

This is the load-bearing baseline for Success Criterion 2.

Procedure, Linux, post-fix binary `ba4e541e`: 8 workers, each writing `START` to
an `effect.log` in its own checkout, sleeping 25s, then writing `DONE`. Killed
uncatchably at **2026-07-27T01:21:45Z** with `kill -9 -<PGID>` on the process
group, 12s in — with every worker mid-flight.

**What survived:**

| Observation | Value |
|---|---|
| Transaction directories left behind | 8 (+1 control) |
| `effect.log` files | 8 |
| `START` lines | **8** |
| `DONE` lines | **0** |
| Heartbeat `.swarm-status.json` files | **0** |

**What the restart knew:** the product *did* notice the crash —

```
warning: previous run did not shut down cleanly
         (crash sentinel found at .../.wayland/.dirty-death.2876367)
```

— and then **refused to run**:

```
error: swarm dispatch failed: dispatch admission refused:
       dispatch aggregate workspace budget is already committed
```

The 8 orphaned transaction roots still carry their reservations.
`reserved_workspace_bytes()` (`worktree_cleanup.rs:67-139`) sums every direct
child of the swarm root; an orphan with no retained receipt in the new manager is
counted at the **full** `MAX_TRANSACTION_WORKSPACE_BYTES` ceiling
(`worktree_cleanup.rs:118-123`). Eight of those exhaust the aggregate budget, and
admission refuses (`worktree_manager.rs:396`).

`cleanup_all()` exists and can reclaim, but **nothing invokes it on a fresh
process at dispatch time**. So the baseline is not "a restart loses completions" —
it is **a killed fanout cannot be restarted at all** until a human deletes
`.swarm-worktrees/`.

**Descendant survival, Linux (measured, not assumed).** Worker processes are
re-parented out of the parent's process group by the sandbox, so a pgid-scoped
count is blind to them; counted globally by command line instead:

| | before kill | after kill |
|---|---|---|
| `bwrap` containers | 8 | **0** |
| worker shells | 17 | **0** |
| `DONE` lines after waiting past the full sleep | 0 | **0** |

So on Linux an uncatchable process-group kill reaps the whole tree and **no
orphan completes work nobody is tracking**. The duplicate-execution risk on this
platform therefore comes from *re-running lost work*, not from a superseded owner
still running — which is the opposite of the plan's stated worry and would have
changed the fencing tradeoff had Task 2 been reached.

**Windows kill leg: NOT VALIDLY RUN.** Two attempts. The second failed on *my
harness*, not the product: PowerShell's `Start-Process -ArgumentList` split
`cmd.exe /c worker.cmd`, so `/c` reached the CLI as a stray argument
(`error: unexpected argument '/c' found`) and no worker ever started. The first
killed at 14s, before any checkout had materialized. Recorded as unmeasured
rather than inferred from Linux.

---

## F-3 (HIGH, **FIXED 2026-07-27** on `lane/swarm-durability`) — workers fail as a function of how long they run

> Status updated after this document was written, and the root cause is NOT where this section
> looks for it. It is not in `wcore-swarm` at all: `RegularFileAuthority::read_bounded` rewound
> and drained a `try_clone()` of the retained descriptor, and `try_clone` is `dup` on unix and
> `DuplicateHandle` on Windows — **both share the file offset**. Two concurrent validators
> interleave and the loser reads zero bytes from an intact file. Elapsed time was only a proxy
> for how many racing pairs occur. Fixed with positional reads; 4/4 at every duration to 45s.
> The suspicion recorded below that F-1 would make F-3 more visible was CORRECT — it unmasked
> it (proven at `de977949`, the commit before), it did not cause it.

Discovered while building the kill instrument. `--worker-command /bin/sleep N`,
4 workers, Linux, post-fix binary:

| Worker duration | Succeeded | `invalid retained workspace reservation` |
|---|---|---|
| 1s | 4/4 | 0 |
| 2s | 3/4 | 1 |
| 5s | 3/4 | 1 |
| 10s | **1/4** | 3 |

Reproduced on Windows in an unrelated run (7/8, same error string).

Isolation: a worker that only *writes* (`touch marker.txt`) succeeds 2/2, and
`/bin/true` succeeds 2/2 — so the trigger is **elapsed time**, not filesystem
writes. The failure surfaces in teardown/heartbeat accounting
(`"heartbeat filesystem authority before read: dispatch admission refused:
invalid retained workspace reservation"`), which means the worker's own work
completes and the *bookkeeping* fails. Not root-caused; `validate_reservation_contents`
(`worktree.rs:551-567`) reports this string only when the reservation file fails
to parse as a `u64`.

Real fanout workers run for minutes, so at the observed trend this makes the
Fleet path unusable for real work. It does not block a kill experiment, because
a killed parent never reaches teardown.

**Corroboration, and a caveat stated precisely.** The full `wcore-swarm` suite on
this lane's branch reported `147 tests run: 147 passed (1 flaky)`, and the flaky
one — `swarm_reports_failed_worker_status_and_succeeding_workers_complete` — failed
its first attempt with this exact error before passing on retry. So the defect is
already intermittently red in the existing suite and is being hidden by the retry.

What that does **not** establish is whether fixing F-1 unmasked it. Two readings
are possible: before F-1 only one worker ever ran, so a concurrency-dependent
reservation race could not manifest; or the flake is unrelated to worker count.
The evidence does not separate them — the test was run 10 times in isolation at
base (`de977949`) and 10 times on this branch (`7dab7840`) and failed **0/10 on
both**. It only appeared under full-suite concurrency on a host at load average
~149. The reliable evidence for F-3 is therefore the duration sweep above, not
that flake, and anyone merging F-1 should expect F-3 to become *more* visible
rather than assume it was introduced.

---

## Budget re-entry seam (named from source, as Task 1 requires)

The child-budget entry a reassigned attempt must re-enter is
`BudgetAuthorityCoordinator::build_and_append`
(`crates/wcore-agent/src/budget_authority.rs:559`), which appends through
`SessionJournal::append_built_from_head` (`session_journal.rs:422-435`). That
primitive holds the writer lock, builds the event from the **committed** reduced
head, and appends — so it is already the atomic compare-and-append a claim needs,
and it is the seam Phase 21 repaired for the parallel-sibling TOCTOU
(`1eb9b5ca`, in this lane's base). Any ledger built later should re-enter it
rather than mint a second one.

No sibling budget faults were observed in any run in this lane, consistent with
that fix holding.

---

## Structural blocker for Tasks 3–4

`crates/wcore-agent/src/goal/` **does not exist**. Plan 22-03 Task 3 reads from
`goal/kernel.rs` and `goal/terminal.rs` and builds `goal/ledger.rs` on top of
them; 22-01's own SUMMARY records that the kernel, the `SessionEvent` variants,
the reducer arm and the `ReducedSessionState` field were never built — only the
vocabulary in `wcore-types::goal`. So the ledger has nothing to extend.

This is termination state **3-adjacent but not identical**: the plan's state 3 is
"the Task record shape cannot express what the ledger needs". Here the record
shape does not exist at all. The honest label is a missing dependency, not a
finding against 22-01's shape, and it is recorded rather than routed around.

The journal seam is nonetheless ready for it and was surveyed:
`SESSION_JOURNAL_SCHEMA_VERSION = 5` (`session_journal.rs:48`); `SessionEvent`
is `#[non_exhaustive]` with 135 variants and its `apply_event` match is
exhaustive with no wildcard (`reducer.rs:1655-3354`), so a new record cannot
enter without a deliberate reduction arm; and new `ReducedSessionState` fields
must be `#[serde(default, skip_serializing_if = ...)]`
(`model.rs:1178-1189`) or 22-01's measured byte-identity property stops holding.
