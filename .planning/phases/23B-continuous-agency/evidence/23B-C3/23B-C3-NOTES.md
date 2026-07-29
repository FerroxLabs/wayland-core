# 23B Criterion 3 — lane `23b-c3-memory` working NOTES

Criterion text (verbatim, `23B-PHASE-VERDICT.md:19`):

> See and control memory/user-model activation, provenance, correction, forgetting, privacy,
> retention, nudges.

Verdict grade at lane start: **NOT MET**. Base `lane/grade-23b`, worktree HEAD `5bbb0fbc`.

**Seven verbs are graded separately below. Nothing here is graded as a whole.**

---

## Verb ledger (updated as measurements land)

| # | Verb | State at lane start | State now |
|---|---|---|---|
| 1 | activation | unmeasured | — |
| 2 | provenance | wired, never driven live | — |
| 3 | correction | wired for episodes; user-model precedence absent (G3c) | — |
| 4 | forgetting | wired to the row; **no outbound-prompt proof** (G3a) | — |
| 5 | privacy | wired, never driven live | — |
| 6 | retention | wired, never driven live | — |
| 7 | nudges | `NudgeBudget` exists, **no user-reachable surface** (G3b) | — |

---

## M0 — baseline measurements (Mac, unproxied `/usr/bin/grep`, quoted globs)

Instrument liveness proved in each invocation by a known-positive in the same run.

- `/memory` sub-verbs dispatched at `crates/wcore-agent/src/slash/memory.rs:47-58`;
  implementations `provenance` :97, `correct` :158, `forget` :186, `privacy` :202, `retention` :251.
  All five are `Runtime`-only and refuse out loud on `Stub` (`runtime_api()` :75).
- `NudgeBudget` — `/usr/bin/grep -rn "NudgeBudget" crates/ "--include=*.rs"` → **7 hits, in exactly
  2 files**: `wcore-memory/src/lib.rs:50` (re-export) and `wcore-memory/src/provenance.rs`
  (:28 doc, :545 struct, :573 impl, :929/:942/:950 tests). Known-positive in the same flags:
  `MemoryControls` → 12 hits across the tree. **So the instrument is alive and the nudge surface is
  genuinely absent** outside `wcore-memory`. Confirms verdict M7.
  *(First attempt returned zsh's `no matches found: --include=*.rs` — the §3b-i trap. Quoted.)*
- `received_requests` (the outbound-body capture the F23-03 acceptance mechanism needs) →
  22 files, **none in `wcore-memory` or `wcore-agent/src/slash`**. The mock lives at
  `crates/wcore-cli/tests/support/mock_llm.rs`. That is the vehicle for G3a.
- User-model surface is real and large: crate `wcore-user-model`, `wcore-agent/src/user_context.rs`,
  `wcore-memory/src/partition/core.rs` + `core_inference.rs`. So "user-model correction precedence"
  has somewhere concrete to land — it is not a hypothetical subsystem.

## M1 — THE FINDING: every control verb misses the ONE partition that is auto-injected

This is the structural reason C3 is NOT MET, and it is worse than "the outbound-body proof is
missing". **The missing proof is exactly why nobody noticed.**

**Two disjoint retrieval paths exist.**

1. `PartitionDispatcher::search` (`partition/mod.rs:253-291`) = `search_basic` (**episodic**)
   **plus** `retrieve::facts_search` (**semantic**, appended at :277).
2. `PartitionDispatcher::search_with_provenance` (`partition/mod.rs:305-331`) =
   `search_basic_with_provenance` — **episodic ONLY**, by its own comment at :310-313: *"those hits
   are deliberately not reported here rather than reported wrongly."*

**What actually reaches the outbound provider request body.** `AgentEngine::recall_relevant_facts`
(`engine.rs:13334-13410`) runs on the first user turn of every session, calls path (1), and
**keeps `Partition::Semantic` hits only** (`engine.rs:13360`: `if h.partition == Partition::Semantic`),
then pushes them as a `<system-reminder>` user message (`:13405`). So the content auto-injected into
the prompt is **exclusively semantic facts**.

**What the seven controls act on.**

- `MemoryControls::correct_episode` (`provenance.rs:244`) — `UPDATE episodes`, `Partition::Episodic`
  hardcoded.
- `MemoryControls::forget_episode` (`provenance.rs:290`) — `DELETE FROM episodes`,
  `Partition::Episodic` hardcoded.
- privacy enforcement — **exactly one call site**: `retrieve.rs:47`,
  `read_privacy_scope(db, Partition::Episodic, q.tier)`.
- retention enforcement — **exactly one call site**: `retrieve.rs:58`,
  `read_retention(db, Partition::Episodic, q.tier)`.
- provenance reporting — episodic only (above).

Measured with `/usr/bin/grep -rn "read_privacy_scope|read_retention|facts_search" crates/
"--include=*.rs"`. `facts_search` returns **2 hits** (def + the one call site) in the same
invocation, proving the instrument alive; `read_privacy_scope` returns 4 (def, re-export,
`MemoryControls::privacy_scope` reader, and the single **enforcement** site);
`read_retention` likewise 4.

**Consequence table — measured, not inferred:**

| | reaches the outbound prompt automatically | privacy enforced | retention enforced | provenance reported | `/memory forget` reaches | `/memory correct` reaches |
|---|---|---|---|---|---|---|
| episodic | no (only if the model calls `session_search`) | YES | YES | YES | YES | YES |
| **semantic facts** | **YES, every cold turn** | **NO** | **NO** | **NO** | **NO** | **NO** |

So today: a user runs `/memory why <q>`, is shown episodic items, and is shown **nothing about the
facts that are in fact in their prompt**. If they somehow learn a fact id, `/memory forget <id>`
returns `NotFound` (the `DELETE` matches 0 rows in `episodes`). `/memory privacy semantic <reason>`
is accepted and audited and **changes nothing about what is sent**. That last one is the
privacy problem the dispatch names: **a control that reports success and does not act.**

`facts_search` (`retrieve.rs:228`) reads
`SELECT ... FROM facts WHERE tier=?1 AND superseded_by IS NULL AND embedding IS NOT NULL` with no
privacy/retention predicate at all.

## Planned order (dispatch says prioritise forgetting, correction, provenance)

1. G3a forgetting-in-the-prompt proof (mock provider body capture) — the criterion's legal weight.
2. G3c user-model correction precedence.
3. Verb 1 "activation": measure whether any see/control surface exists at all.
4. G3b nudge surface.
5. G3d live drive of all verbs through the shipped binary on hetzner.

## Standing constraints for this lane

- Cache/compaction is lane `23b-c4-cache`'s. Do not touch `compact/`, `cache_*`.
- `wcore-memory/src/db.rs` WAL journal-mode handling was changed today by lane `wal-nfs`. Do not
  revert.
- No merge, no PR, no tag, no `wcore-contract generate`.
