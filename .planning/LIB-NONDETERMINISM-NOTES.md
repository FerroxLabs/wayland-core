# `cargo test -p wcore-agent --lib` is nondeterministic — root cause and fix

Lane: test-nondeterminism. Branch `lane/lib-nondeterminism`, worktree `/root/wt-libnondet`,
base `f738a560`. All builds and runs on `hetzner-dsm` (96 cores).

**Headline: the mechanism is NOT test-only. It is production code, it is reachable from
the shipped agent, and it has a second instance in a different lease.**

---

## 1. Reproduction

Unmodified code at `f738a560`, default parallelism, nothing else running.

| run | result | failures |
|---|---|---|
| 1 | FAILED | 19 |
| 2 | FAILED | 16 |
| 3 | FAILED | 20 |
| 4 | FAILED | 21 |
| 5 | FAILED | 18 |
| diag A (instrumented) | FAILED | 16 |
| diag B (instrumented) | FAILED | 13 |

Union across runs: **28 distinct tests**. No test fails in every run. Logs:
`/root/wt-libnondet/nd/run_*.log`, failing sets in `nd/run_*.fails`.

The failing set clusters completely — session, session journal, session lifecycle,
engine restart/resume, orchestration durability, channel lease. Every one of them
either drops a journal and reopens it, or asserts that a lease was released.

### Is it parallel-only?

Yes. `--test-threads=1` is green (2326 passed, 0 failed), and the failing union run
alone passes at both `-j1` and `-j96` — measured independently by the variance lane and
consistent with everything I saw. It is cross-test interference under whole-suite
parallelism.

**It is not the live-corpus variance cause.** 0 of 24 corpus runs touched this path.
That question stays open; this is a different bug that happened to look like it.

---

## 2. Mechanism

### Where the lock actually fails

The brief pointed at `WriterLease::acquire` (`session_journal/lease.rs:253`). **That is
wrong**, and it matters for the fix. I instrumented both `AlreadyOwned` construction
sites and ran the whole suite against each:

| site | file:line | hits |
|---|---|---|
| `WriterLease::acquire` (the `.writer.lock` sentinel) | `session_journal/lease.rs:302` | **0 of 16** |
| `lease::lock_data_file` (the `.journal` data file) | `session_journal/lease.rs:545` | **13 of 13**, then **16 of 16** |

Callers: `JournalWriter::open` (`session_journal.rs:1379`) and
`SessionStorageLease::acquire` (`session_journal.rs:988`). Both take the `.writer.lock`
sentinel **first**, and it **succeeds** — so no live competing writer exists. The refusal
comes from the data-file lock, and it is a ghost.

### What holds it

Instrumentation on the contended path recorded, per failure:

* `/proc/locks` contains a real `FLOCK ADVISORY WRITE` on the journal inode, attributed
  to the test process's own pid — the same pid for every failure in a run.
* Scanning `/proc/*/fd` finds **no descriptor anywhere** pointing at that file except the
  caller's own.
* Re-attempting the lock at 1 ms intervals succeeds after **3 µs to 37 ms**
  (`attempts=1` to `attempts=36`) with no other intervention.

A lock that is real in the kernel, owned by no visible descriptor, and evaporates on its
own in milliseconds is a **fork-duplicated open file description**.

### The asymmetry — this is the bug

`flock` is bound to the **open file description**, not the descriptor and not the process.
`close(2)` releases the lock only when the *last* descriptor referring to that description
goes away. `fork(2)` duplicates the whole descriptor table, so every subprocess spawned
while a journal is open keeps that description — and its lock — alive until the child
`exec`s (`O_CLOEXEC`) or exits.

* `WriterLease::drop` (`session_journal/lease.rs:665`) explicitly calls
  `unlock_authority(&self.file)`. `LOCK_UN` releases the description for **every**
  duplicate of it. The sentinel therefore never leaks. This is why it had 0 hits.
* The data-file lock taken at `session_journal/lease.rs:545` had **no explicit unlock
  anywhere**. It relied on `close(2)`, which cannot reach a description a child holds.

The agent forks constantly while a journal is open — the Bash tool, `git status`
(`engine.rs:13732`), the spawner (`spawner.rs:4205`), the forge
(`orchestration/anvil/forge.rs:1285`). Under whole-suite parallelism some other test is
almost always mid-`fork`, which is exactly why this is parallel-only and why the failing
set is random.

---

## 3. The decisive question: test-only, or shared with production?

**Shared with production.** Not a measurement artefact.

`crates/wcore-agent/tests/prod_probe.rs` uses production APIs only —
`SessionJournal::open` and `wcore_config::shell::shell_command_argv`, the same helper the
Bash tool and `git` paths use. No `#[cfg(test)]` code, no test harness state. Each
iteration opens a journal, drops it — the only owner — and immediately reopens.

| spawner threads | CPU load | refusals / 2000 reopens (before) | after fix |
|---|---|---|---|
| 1 | idle | 2 (0.1%) | **0** |
| 16 | idle | 47 (2.4%) | **0** |
| 48 | 64 burners | **951 (47.6%)** | **0** |
| 96 | 96 burners | 926 (46.3%) | **0** |

Even one subprocess-spawning thread on an idle box refuses 1 reopen in 1000.

### The case a retry cannot cover

`O_CLOEXEC` closes an inherited descriptor at `exec`, which is why the race above is
measured in milliseconds. A child that forks and **never execs** — any daemonised helper,
anything the agent backgrounds — pins the description for its whole lifetime.

`dropping_a_journal_releases_it_even_when_a_forked_child_never_execs` in the same file
forks such a child deliberately. Red arm (fix reverted, source `touch`ed, rebuilt):

```
panicked at crates/wcore-agent/tests/prod_probe.rs:166:5:
dropping a journal must release its data-file lock even while a forked child still holds
the open file description: Some(AlreadyOwned { lease_path: "/tmp/.tmp3t8zB8/pinned.journal" })
test result: FAILED. 0 passed; 1 failed ... finished in 0.01s
```

Deterministic in 0.01 s — not a race, an unconditional pin. Green arm passes. No retry
budget survives this case, which is why the retry was the wrong primary fix.

### Production surfaces with the drop-then-reacquire shape

`switch_active_session` (`engine.rs:4050`) states the contract in its own comment —
"replacing `session_journal` releases the old session's lease" — and the shape recurs in
resume and restart. A refusal there surfaces to a user as a session switch or resume
failing for no reason, load-dependent, with no model involved.

### Can it explain a user-visible symptom already on file?

**Not one I can attribute to it, and I am not going to imply otherwise.**

The one live (non-test) occurrence on file is
`.planning/phases/22-supervision-durable-goals-fleet-loops/22-C3-NOTES.md:259` — "First
live invocation died at `session journal writer lease is already held`". I read it: that
was a genuine double-open (`goal run --terminate` opened one handle for
`GoalFleetDriver` and a second for `GoalLoop`), fixed in `60c919b0` by cloning the
handle. Same error string, different root cause. It is **not** evidence for this defect.

Every other filed mention (`23B-H1-SUMMARY.md:224`, `24-C3-H4-NOTES.md:261`,
`21-REVERIFICATION.md:455`, `27-C3-MEDIA-SUMMARY.md:253`, `REGRADE-AUDIT-NOTES.md:178`,
and others) is the same test-suite artefact, repeatedly re-observed and never
root-caused. `23B-H1-SUMMARY.md` got closest — "each contends with itself because a prior
handle has not released its advisory lock in time" — but stopped there.

So: the defect is real in production and proven reachable, but **no filed user report is
attributable to it**. What is on file is nine months of the same suite artefact.

---

## 4. Second instance of the identical defect

The residual failures after fixing the journal were all `channel_lease`, a **different
lease**, and it has the same bug. `ScheduleLease::drop`, `crates/wcore-cron/src/lease.rs:370`,
before the fix:

```rust
// The OS releases the lock itself when `_sentinel` closes — which is also why an UNCLEAN
// death (SIGKILL, panic, power loss) still frees the lease: nothing here has to run for
// the next process to acquire it.
```

That assumption is the bug, stated explicitly. No `unlock` call existed.

This one has a worse production consequence than the journal. The channel poll lease
governs inbound message polling — a ghost holder makes a process print *"another
wayland-core process is already receiving messages for this home; this one will not poll
for inbound messages"* and go dark. The test that fails says exactly this: **"a released
lease must be reclaimable, or loss becomes unavailability"** (`channel_lease.rs:883`).

---

## 5. What I fixed

Both fixes are the same one-line-of-behaviour change: release the lock explicitly on
drop, mirroring what `WriterLease::drop` already did. `LOCK_UN` reaches the duplicated
descriptions that `close(2)` cannot, so it closes the window rather than narrowing it.

| commit | change |
|---|---|
| `d4eb7142` | `tests/prod_probe.rs` — production-path probe + non-exec'ing-child case |
| `02833d6b` | `lease::unlock_data_file`; `Drop for JournalWriter`; `Drop for SessionStorageLease`; the discarding `write_snapshot` wrapper (`session_journal.rs:962`) |
| `ee3fbe02` | `ScheduleLease::drop` releases explicitly; `sys::unlock` for unix / windows / fallback |

No retry layer was added. It is unnecessary once the lock is released correctly, and it
would have converted a correctness bug into a latency bug while leaving the non-exec'ing
child unfixed.

`cargo check -p wcore-cron --all-targets` and
`cargo clippy -p wcore-cron --all-targets --target x86_64-pc-windows-gnu` both exit 0, so
the Windows `UnlockFileEx` branch compiles.

### Full suite, default parallelism

| | run 1 | run 2 | run 3 | run 4 | run 5 |
|---|---|---|---|---|---|
| before | 19 | 16 | 20 | 21 | 18 |
| journal fix only | 1 | 0 | 1 | 2 | 3 |
| both fixes | **0** | **0** | **0** | **0** | **0** |

Every residual after the journal fix was `channel_lease`, i.e. the second instance.

---

## 6. Why CI never caught this

CI and `.gate.sh` run `cargo nextest run`, which executes **each test in its own
process**. A fork in test B's process cannot duplicate test A's descriptors, so the
cross-test interference that dominates `cargo test --lib` cannot occur there. That is why
this survived: the defect is invisible to the instrument the project grades on.

Nextest does **not** make it impossible — a single test that spawns a subprocess and then
drops and reopens its own journal still races, which is precisely what `prod_probe`
demonstrates. And it never protected production at all.

**On pinning `--test-threads=1` in the gate scripts:** I did not apply it. Nothing in
CI or `.gate.sh` invokes `cargo test` on this suite — they all use nextest, which is
already process-isolated, and forcing `--test-threads=1` onto nextest would serialise the
whole workspace suite for no correctness gain. The phantom failures are now fixed at the
source, which is strictly better than hiding them behind serial execution. If a
belt-and-braces guard is still wanted, the right place is a note next to the
`cargo test` anti-vacuity gate at `.github/workflows/ci.yml:1116`, not a global thread
pin.

---

## 7. What remains, ranked by risk

1. **HIGH — audit every other `flock` holder for the same assumption.** Two of two
   inspected had it. I fixed the journal data file and `ScheduleLease`; I have **not**
   swept `orchestration/anvil/lease.rs`, `oauth/refresh_lock.rs`, or any other
   lock-bearing `File` in the workspace. The grep is
   `try_lock|flock|LockFileEx` cross-referenced against `impl Drop`. Any holder whose
   Drop does not call an unlock has this bug.
2. **MEDIUM — snapshot-file locks in `session_journal/snapshot.rs:1233`.** `write_snapshot`
   returns a locked handle; the three internal callers
   (`session_journal.rs:1601`, `:1662`, `:1805`) drop it at end of scope without an
   explicit unlock. I fixed only the public wrapper at `:962`. Same class, lower blast
   radius (snapshot inode, not the journal). The `std::mem::forget` paths are a
   deliberate fail-closed leak and must stay.
3. **MEDIUM — the durability suite's true state has never been known.** Nine months of
   summaries graded around 13-21 phantom failures. Anything that was accepted "modulo the
   known lease failures" should be re-graded now that the suite is clean.
4. **LOW — 176 `env::set_var`/`remove_var` sites in `wcore-agent/src`.** Guarded by
   `#[serial]`, which only serialises against other `#[serial]` tests and does not stop
   concurrent non-serial tests from *reading* the mutated variable. `piper.rs:791` sets
   process-wide `HOME`; `video_analyze.rs:682` removes `ANTHROPIC_API_KEY`. This was the
   prime suspect and it is **not** the cause of the failures measured here — but the
   hazard is real and unclosed.
5. **LOW — the reported "test mutates a committed evidence file".** I checked the working
   tree after every one of 6 full suite runs: clean apart from my own logs. Whatever that
   item refers to, it is not in `wcore-agent --lib` at this commit.
6. **OPEN — the live-corpus variance.** Untouched by this. Ruled out as the cause here.
