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
| 1 | activation | no surface of any kind | **CLOSED** for memory (per-process limitation stated) |
| 2 | provenance | episodic only; blind to what is in the prompt | **CLOSED** for memory |
| 3 | correction | episodes only; user-model precedence absent (G3c) | **CLOSED** for memory; G3c NOT DONE |
| 4 | forgetting | row-level only; **no outbound-prompt proof** (G3a) | **CLOSED**, wire- and live-proved |
| 5 | privacy | accepted and did nothing for semantic | **CLOSED**, wire- and live-proved |
| 6 | retention | accepted and did nothing for semantic | **CLOSED**, wire-proved |
| 7 | nudges | `NudgeBudget` unreachable (G3b) | **HALF** — reachable+settable, but nothing exists to bound |

**Final: see `23B-C3-SUMMARY.md`.** Five of seven closed for the memory subsystem; nudges half;
the user-model half of the criterion untouched. Criterion as a whole: still NOT MET.

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

## M2 — the finding PROVED at the wire, red at base and green after the fix

Two runs of the same probe shape, both on `hetzner-dsm`, both `cargo nextest ... --no-tests=fail
--retries 0`, both counts read back.

**RED at base `5bbb0fbc`** (`evidence/23B-C3/base-redproof.log`, source
`evidence/23B-C3/c3_redproof.rs.txt`). The file calls the base control API —
`MemoryControls::forget_episode` / `set_privacy_scope` / `set_retention`, which is exactly what
`slash/memory.rs` called — and performs the identical two-turn wiremock capture:

```
Summary [0.585s] 5 tests run: 1 passed, 4 failed, 0 skipped
  PASS  the_probe_can_fail                                         <- harness alive
  FAIL  forget_at_base_leaves_the_value_in_the_outbound_body
  FAIL  privacy_at_base_reports_success_and_changes_nothing
  FAIL  retention_at_base_reports_success_and_changes_nothing
  FAIL  provenance_at_base_says_nothing_about_the_facts_in_the_prompt
```

with these diagnostic lines printed by the tests themselves:

```
BASE_FORGET_OUTCOME=Some("memory item not found: partition=episodic tier=project id=367a69b3-…")
BASE_PRIVACY_CONTROL_REPORTED=ok
BASE_RETENTION_CONTROL_REPORTED=ok
BASE_PROVENANCE hits=0 provenance_entries=0
```

Read those four lines together and the criterion's whole failure is on one screen:

- **forget could not even address the item.** `NotFound partition=episodic` for a fact the same
  test had just proved was in the outbound body.
- **privacy and retention both returned `ok`** and the nonce was still on the wire on the next
  turn. A control that reports success and does not act.
- **`search_with_provenance` returned `hits=0`** for a query the probe proved does inject —
  `/memory why` was structurally incapable of describing the content in the prompt.

`the_probe_can_fail` PASSING in the same run is what makes the four failures findings rather than
a dead instrument (§3b-i, §3.2).

**GREEN after the fix**, HEAD of `lane/23b-c3-memory` (`evidence/23B-C3/fixed-green.log`):

```
Summary [0.716s] 8 tests run: 8 passed, 0 skipped
```

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
