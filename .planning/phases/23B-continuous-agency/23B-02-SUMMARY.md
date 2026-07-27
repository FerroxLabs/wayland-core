---
phase: 23B-continuous-agency
plan: "02"
subsystem: memory-provenance-and-control
status: complete-with-named-open-controls
requirements:
  - F23-03
  - F23-04
requirements_disposition:
  F23-03: incomplete
  F23-04: not-started
tags: [memory, provenance, forgetting, privacy, retention, nudge-bound, recall]
provides:
  - crates/wcore-memory/src/provenance.rs
  - crates/wcore-memory/src/schema/v6_recall_control.sql
key-files:
  created:
    - crates/wcore-memory/src/provenance.rs
    - crates/wcore-memory/src/schema/v6_recall_control.sql
  modified:
    - crates/wcore-memory/src/lib.rs
    - crates/wcore-memory/src/api.rs
    - crates/wcore-memory/src/memory.rs
    - crates/wcore-memory/src/partition/mod.rs
    - crates/wcore-memory/src/retrieve.rs
    - crates/wcore-memory/src/staleness.rs
    - crates/wcore-memory/src/cdc.rs
    - crates/wcore-memory/src/error.rs
    - crates/wcore-memory/src/schema/mod.rs
    - crates/wcore-agent/src/slash/memory.rs
commits:
  - dba9b9e5
  - ee2c3853
  - 14e29753
  - e8100cc4
---

# Phase 23B Plan 02: Memory Provenance and Control Summary

Recall provenance and the operator controls over it are built, gated, audited and
reachable from `/memory` on the shipped surface; the cache, compaction and cost half of
this plan (F23-04) was **not started**, and no leg of it was driven live.

**Termination state: 2 — complete with named open controls.**

## What shipped

`crates/wcore-memory/src/provenance.rs` answers "why is this in my context window?".
Every item a retrieval places in a prompt now carries the partition and tier it came
from, every modality that selected it with the rank it held inside that modality, its
rank and score in the fused list, its age, and its staleness verdict. Separately it
carries the items and cells that were **excluded** — a privacy scope with its reason, an
expired item with its age and the bound it exceeded — because the difference between
"nothing matched" and "you excluded this" is the entire value of reporting it.

Four controls run over those records: correct, forget, privacy scope, retention bound.
Each goes through the **unmodified** `MemoryAccessGate` with an appropriate token, each is
audited, and a forget is additionally represented in the CDC changelog so a downstream
consumer sees a deletion rather than a row that quietly vanished. Privacy and retention
live in a new schema v6 keyed by the same `(partition, tier)` cell the gate governs, so a
control cannot name a cell the gate does not recognise.

`/memory` gained `why`, `correct`, `forget`, `privacy` and `retention` on its Runtime
variant. The Stub variant's strings are byte-unchanged and every test resting on them
still passes.

## The decision that makes the provenance worth reading

**Provenance is emitted by the fusion that produced the ranking.**
`rrf_fuse_with_contributions` is now the only fusion implementation. A provenance record
computed by a second, parallel pass could describe a ranking that never happened, and a
user shown a provenance that does not match their context window is worse off than one
shown nothing. There is no second pass to drift from. The scoring math is unchanged and
the golden RRF tests that pin it still pin it, through a shim onto the surviving
function — so the math they pin is exactly the math provenance is captured from.

The same rule drove two smaller calls. `search_with_provenance`'s default impl returns
the ordinary hits with an **empty** report, reading as "this backend cannot tell you",
rather than fabricating records. And the dispatcher's implementation deliberately omits
the semantic-fact pass that `search` appends afterwards: those hits have no fused rank, so
claiming one for them would be a fabrication.

No control has a silent no-op. Forgetting an id that is not there refuses with
`NotFound`, because a user who mistypes an id and is told "ok" believes content is gone
when it is not. A backend without controls refuses out loud for the same reason.

## Deviations from plan

- **Files outside `files_modified` were touched, and had to be.** The plan says to capture
  provenance "at the point of fusion so the record cannot diverge from the selection", but
  does not list `retrieve.rs`. Recomputing provenance inside `provenance.rs` instead would
  create exactly the divergence the plan forbids. Also touched: `api.rs`, `memory.rs`,
  `partition/mod.rs` (to make the controls reachable from the product at all), `cdc.rs`
  (`append` is private; a forget needs a changelog path), `error.rs`, and `schema/`.
- **`scripts/f23-macos-binary.sh` was not written and no macOS leg ran.** The plan decides
  the macOS leg builds its own binary on this Mac. This phase's controlling execution
  instruction forbids running Cargo on the Mac, `cargo fmt` excepted. I honoured the
  controlling instruction, as 23B-01 did. The conflict is unchanged and still escalated.
- **Task 2 (cache, compaction, cost truth) was not started.** `cache_diagnostics.rs`,
  `compact/state.rs` and the `/cost` and `/compact` registry entries are untouched.
- **Task 3 (live drivers) was not written.** `scripts/f23-context-economics-drive.sh` and
  its PowerShell port do not exist, and `crates/wcore-cli/tests/memory_control_lifecycle.rs`
  and `crates/wcore-agent/tests/context_economics_test.rs` were not written.
- **The nudge bound is implemented but not surfaced.** `NudgeBudget` refuses past its cap
  and honours an off switch, proved by driving past the cap including under eight
  concurrent claimants. No CLI or TUI command exposes it, so it is not yet a control a
  user can reach.

## What this does NOT prove — read this before grading Criterion 3

**Nothing here was driven live.** Every claim above rests on tests, including tests that
reach a real SQLite store through the real open path. Per this program's own standing
rule, that is necessary and never sufficient. Specifically:

- `/memory why|correct|forget|privacy|retention` were **not driven through a real TUI
  session on any platform**. They are proved reachable through the slash dispatcher
  against a real store, not observed on a user's screen.
- **The acceptance mechanism this plan is built around was not used.** The plan's central
  demand is that forgetting be proved by a value's ABSENCE FROM THE ACTUAL OUTBOUND
  PROVIDER REQUEST BODY, via `mock_llm.rs`'s `received_requests`. That was not done. What
  is proved is that a forgotten row is deleted, is gone from subsequent retrieval, and is
  represented in the changelog. **The plan explicitly names "asserting a deleted row" as
  the engineered green to avoid, and that is what the current evidence is.** F23-03 cannot
  be called met on this evidence.
- Retention expiry is proved at the retrieval layer, not against a prompt.

## Requirement dispositions

| Requirement | Disposition |
|---|---|
| F23-03 | **INCOMPLETE.** Provenance, correction, forgetting, privacy, retention and the nudge bound exist, are gated and audited, and are reachable from `/memory`. Not proved against an outbound request body; not driven live on any platform; nudges not surfaced; user-model correction precedence not implemented. |
| F23-04 | **INCOMPLETE — not started.** No cache invalidation cause, token-pressure report, compaction quality verdict or cost reconciliation. |

## Verification

- `cargo clippy -p wcore-memory -p wcore-agent --all-targets -- -D warnings` — clean on
  `hetzner-dsm`. Three findings were **fixed rather than allowed**: a nine-argument
  function, a dead fusion wrapper, and a collapsible `if`.
- `cargo fmt --all -- --check` — clean on the Mac.
- `cargo nextest run -p wcore-agent -p wcore-memory --profile ci --no-fail-fast` —
  **3418 tests run: 3418 passed, 13 skipped**, and no test consumed a retry (grepped for
  `TRY 2`, zero hits), so nothing here is a flake papered over by the retry policy.
- `cargo test -p wcore-memory --lib` — **348 passed, 0 failed**, every pre-existing test
  unchanged.
- `cargo test -p wcore-agent --lib slash::memory` — **13 passed, 0 failed**.
- **Not run:** any live driver, any Windows or macOS leg, any TUI leg.

**One result worth recording rather than hiding.** The raw `cargo test -p wcore-agent
--lib` harness reported **14 failures** on the 96-core build host. Every one passed in
isolation, and the same suite run with `--test-threads=1` was **2101 passed, 0 failed**.
The failing set was entirely session-lease and journal-authority tests — the class that
contends on process-wide state — and the raw harness runs them 96-way parallel where
nextest gives each its own process. The project's own authoritative harness is nextest,
and it is green. I am reporting the red because it is real about the raw harness even
though it is not a product defect.

One wiring bug was caught by a test before it shipped: `Memory` — the type the CLI
actually holds — would have inherited the new trait defaults, so `controls()` returned
`None` on the one backend that has controls and every operator command refused with "this
backend exposes no operator controls". Fixed in `14e29753`.

## Self-Check: PASSED

Both created files exist on disk; all four commits resolve in `git log`.
