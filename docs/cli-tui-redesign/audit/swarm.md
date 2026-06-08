# Audit: `wcore-swarm` — the real multi-agent substrate

Source: `crates/wcore-swarm/` (lib.rs + 8 modules). Crate doc declares itself
"productized worktree-isolated multi-agent dispatch" (`lib.rs:1`), foundation
for M5.6 (consensus) / M5.7 (memory). The public surface is **SPEC-LOCKED**
(`lib.rs:3-6`).

## 1. Worker model

A "worker" is **a subprocess, not an LLM agent**. `SwarmBrief.worker_command`
is a raw argv vector (`lib.rs:88`) — `["bash","-c","echo hi"]` in the crate's
own example (`lib.rs:20`). `Swarm::dispatch(brief, count)` (`lib.rs:171`) spins
up `count` workers, **all running the identical brief** — there is no per-worker
task, role, or prompt. Identity is `format!("{}-{}", uuid::simple(), i)`
(`lib.rs:175`); the only "role" is a 0-based index baked into the id. No max
count — `count: usize` is unbounded (contrast `Spawn`'s max-5). Workers are
process-isolated: "process boundary; no shared memory" (`lib.rs:37`,
`dispatch.rs:5`).

## 2. Lifecycle states

There is **no running/idle/blocked state machine**. The only enum is terminal:

```rust
// lib.rs:99-106
pub enum WorkerStatus {
    Succeeded,
    Failed(String),
    TimedOut,
    Cancelled,
}
```

Every variant is an end state. A worker is either not-yet-collected (no status
observable) or finished. **There is no `Blocked`, no `WaitingOn`, no
`Running`.** `dispatch` fires all workers concurrently via
`futures::future::join_all` (`lib.rs:184`) — "true parallelism" (`lib.rs:181`).
There is **no dependency graph and no sequential coordination**: every worker
gets the same brief, runs at once, and cannot depend on another worker.
`Cancelled` exists in the enum but is never produced by any code path —
`dispatch.rs` only emits `Succeeded`/`Failed`/`TimedOut`.

## 3. Progress / monitoring signals

Two structs. The post-hoc result:

```rust
// lib.rs:116-123  WorkerHandle  (also SwarmResult, lib.rs:128-137)
pub worker_id, branch, status, stdout, stderr, duration
```

`stdout`/`stderr`/`duration` are **only populated after the worker exits** —
no live streaming ("Live stdout streaming" is explicitly NOT in v0.6,
`lib.rs:42-44`). The only *live* signal is an opt-in heartbeat:

```rust
// heartbeat.rs:27-32
pub struct WorkerStatusFile {
    pub last_alive_at: u64,        // unix-epoch millis
    pub step: Option<String>,     // free-form label, e.g. "running tests"
}
```

The worker writes `.swarm-status.json` into its worktree (~every 5s,
`heartbeat.rs:3`); the orchestrator polls `Swarm::worker_status()`
(`lib.rs:208`). That is the **entire** monitoring surface: a timestamp and an
optional free-text step. **No turn count, no token usage, no completion
percentage, no event stream, no files-touched count, no tool-call count.** And
the heartbeat is opt-in — `worker_status` returns `Ok(None)` if the worker
never writes one (`heartbeat.rs:10`, `lib.rs:204`).

## 4. Worktree isolation

`WorktreeManager` (`worktree.rs:18`) gives each worker one git worktree at
`<repo>/.swarm-worktrees/<worker_id>` on branch
`<worker_branch_prefix>/<worker_id>`, checked out from `base` via
`git worktree add -b` (`worktree.rs:81-89`). `dispatch` first calls
`assert_clean()` — `git status --porcelain`; non-empty output →
`SwarmError::DirtyCheckout`, dispatch refused (`worktree.rs:51-69`,
`lib.rs:32`). `cleanup()` runs `git worktree remove --force` on every entry,
idempotent (`worktree.rs:107-126`).

**There is no merge step.** Nothing in swarm runs `git merge`, `rebase`,
`cherry-pick`, or touches the base branch after dispatch. Worktrees are created
and destroyed; whatever commits a worker made on its branch are simply
**discarded by `cleanup`** unless an external caller harvests them first.
"Results" = captured `stdout`/`stderr`/`status` strings, nothing more.

## 5. Consensus / debate

These are **pure functions over `Vec<SwarmResult>` strings** — not coordination
mechanisms.

- **Consensus** (`consensus.rs:55`): `Consensus::majority` buckets *successful*
  workers' stdout via a `Scorer` (`scorer.rs:17` — `exact_stdout` or
  `normalized_stdout`, byte/trim-lowercase). Strict >50% → `Agreed{value,votes,
  total}`; else `Disputed{top_k: up-to-3, total}` (`consensus.rs:21-39`).
  Failed/timed-out workers excluded from the tally.
- **Debate** (`debate.rs:57`): walks an externally-supplied `Vec<DebateRound>`,
  returns the first round that reaches `Agreed` → `Converged`, else `Diverged`
  (`debate.rs:29-44`). The crate explicitly disclaims the loop: "the
  orchestrator is responsible for replaying round-N-1 outputs into round-N's
  briefs (that's a `wcore-agent` concern)" (`debate.rs:9-11`). Swarm only
  *tallies* rounds; it does not *run* a debate.

In a UI these are **batch verdicts after a run completes** — "3 of 4 workers
agreed on output X" — not a live process.

## 6. CLI / REPL wiring

**`wcore-swarm` is entirely unwired.** A workspace-wide grep confirms: the only
`Cargo.toml` files mentioning `wcore-swarm` are the workspace root and the
crate's own manifest; the only `.rs` files referencing `wcore_swarm` are
`crates/wcore-swarm/src/lib.rs` and its own `tests/`. **No `wcore-cli`, no
`wcore-agent`, no `wcore-tools` (and thus no `SpawnTool`) depends on it.** It is
a standalone library with a locked API and smoke tests, never invoked by the
running binary. To use it today a caller would have to construct
`Swarm::new(repo)`, build a `SwarmBrief`, call `dispatch`/`collect`/`cleanup`
manually — that integration code does not exist anywhere in the tree.

## 7. The honest Agent Manager

Real bindable fields per worker (everything swarm can actually give a UI):

| Field | Source | Live? |
|---|---|---|
| `worker_id`, `branch` | `WorkerHandle` `lib.rs:117-118` | at dispatch |
| terminal status (Succeeded/Failed/TimedOut) | `WorkerStatus` `lib.rs:99` | at collect only |
| `duration` | `WorkerHandle.duration` `lib.rs:122` | at collect only |
| `stdout` / `stderr` (full, final) | `WorkerHandle` `lib.rs:120-121` | at collect only |
| `last_alive_at` heartbeat ts | `WorkerStatusFile` `heartbeat.rs:29` | live, **opt-in** |
| `step` free-text label | `WorkerStatusFile.step` `heartbeat.rs:31` | live, **opt-in** |
| worktree path | `<repo>/.swarm-worktrees/<id>` `worktree.rs:79` | static |
| consensus/debate verdict (whole run) | `ConsensusOutcome`/`DebateOutcome` | post-run batch |

Mockup claims, scored against swarm:

- **Per-worker progress bar with a %** — **NOT backed.** No percentage, no
  total-steps, no fraction anywhere. The closest signal is a free-text `step`.
- **"running / blocked / done" status** — **PARTIALLY.** "done" maps to
  `WorkerStatus` (4 terminal variants). "running" is only inferable from a
  fresh heartbeat (and only if the worker opted in). **"blocked — waiting on
  another agent" is FLATLY UNSUPPORTED** — no `Blocked` state, no dependency
  graph, all workers run in parallel with identical briefs.
- **"+ spawn agent" user action** — **NOT backed as drawn.** `dispatch` takes a
  `count` up front and a *single shared* brief; you cannot add one heterogeneous
  agent mid-run. And nothing wires `dispatch` to the REPL at all.
- **"auto-merge when all agents settle"** — **NOT backed.** Swarm has zero
  merge logic; `cleanup` *deletes* worktrees. There is no settle-then-merge.
- **Live per-agent action feed** — **NOT backed.** No event stream; stdout is
  final-only (`lib.rs:42`). Best possible is polling the one-line `step` label.
- **Files / tool-calls / tokens / elapsed per agent** — **only `elapsed`**
  (`duration`, and only post-collect). No files-touched, no tool-call count, no
  token accounting exist in the crate.

## Verdict

An honest Agent Manager on `wcore-swarm` should show: a flat list of N
parallel workers each with `worker_id` + `branch` + worktree path; a coarse
status that is "alive (heartbeat <Ns old) / no-heartbeat / finished" pre-collect
and one of Succeeded/Failed/TimedOut after collect; the optional free-text
`step` label and `last_alive_at` age; final `duration`, `stdout`/`stderr` once
collected; and a single run-level consensus/debate verdict ("3 of 4 agreed").
It must NOT show a per-worker progress percentage, a "blocked — waiting on
another agent" state or any dependency arrows, a live action feed, per-agent
files/tool-calls/token stats, or an "auto-merge on settle" affordance — none of
those exist, and the crate is not even wired into the CLI today, so any
"+ spawn agent" button is doubly fictional.
