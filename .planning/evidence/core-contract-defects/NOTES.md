# NOTES — lane/core-contract-defects

Base: c9ab048b952c5bc74c75ea8f76df06788408de59
Worktree: /Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-core-contract-defects

## Instrument posture

- All JSON-Schema measurement done with python3 `jsonschema` 4.23.0, `Draft202012Validator`.
- Script committed: `verify-contract-defects.py`. It carries a **both-directions instrument
  control** as its first block (a tiny schema that must reject `{}` and accept `{"a":1}`),
  so a dead validator cannot produce a free "0 errors".
- Every number below is read from a file (`01-base-verification.txt`) via the Read tool,
  never off a proxied stdout (LANE-BRIEF §3b).

## Verified at base — all five scoping claims REPRODUCE

### C5 — unsatisfiable schema. CONFIRMED.

Corpus sweep, 52 event fixtures:

| schema | rejected |
|---|---|
| `schema/core-event.schema.json` | 1 / 52 — `goal_snapshot.json`, schemaPath `oneOf` |
| `schema/producer-complete.schema.json` | 1 / 52 — `goal_snapshot.json`, schemaPath `anyOf` |

Isolated to `core-event.oneOf[50]` / `producer-complete.anyOf[73]`, both erroring at
`/goal/tasks/0` and `/goal/tasks/1` with schemaPath
`properties/goal/properties/tasks/items/oneOf`.

That `items.oneOf` has 2 branches, both `additionalProperties: true`, neither with
`required`. Machine-checked membership: `{}`, `task[0]` and `task[1]` each match
**branches [0, 1]** — so `oneOf` (exactly one) can never be satisfied by any object.

**ROOT CAUSE FOUND — it is one function, not the JSON.** `crates/wcore-protocol/src/contract/generate.rs`:

- `inferred_schema()` line 108-118 emits every object as
  `{"additionalProperties": true, "properties": {...}, "type": "object"}` — permissive,
  no `required`.
- `inferred_schema()` line 101-105 combines >=2 distinct inferred item schemas with
  **`oneOf`**.

Permissive schemas are never mutually exclusive, so `oneOf` over them is unsatisfiable
by construction. Correct combinator for descriptive/permissive union is `anyOf`.

### C4 — `workspace_policy` in neither contract nor DEFERRED. CONFIRMED.

- manifest.json events = 52; `workspace_policy` present = **False**;
  `execution_policy` present = **True** (CONTROL, proves the lookup is alive).
- core-event.schema.json: `workspace_policy` = **False**; `execution_policy` = **True** (CONTROL).
- Present only in `producer-complete.anyOf[76]`, title
  `"Non-Desktop producer inventory discriminator"`, inside an 8-member `enum`.

### C3 — duplicate discriminator branches. CONFIRMED, AND WIDER THAN BRIEFED.

- core-event: 52 distinct discriminators, duplicates = `{"sub_agent_event": [26, 27]}`.
- producer-complete: 82 distinct, duplicates =
  `{"approval_resume": [4, 59], "sub_agent_event": [49, 50]}`.

`approval_resume` is a second duplicate the orchestrator brief does not mention.
Neither duplicate currently causes a rejection (only `goal_snapshot` is rejected),
so C3 is a **latent** `oneOf` hazard.

### C2 — no gate. CONFIRMED (see below).
### C1 — contracts not a signed release asset. CONFIRMED (see below).

## THE HARD CONSTRAINT — measured, and it governs the whole lane

`crates/wcore-protocol/src/contract/check.rs::check_contract()` regenerates the corpus
**in memory** and rejects any byte drift against the committed files. It is enforced twice:

- `crates/wcore-protocol/tests/desktop_contract_corpus.rs:203`
- `.github/workflows/ci.yml:274` and `justfile:59` — `wcore-contract check`

Therefore **any** change to the generator's output, and equally any hand-edit of the
committed JSON, turns both of those red until someone runs `wcore-contract generate` —
which is orchestrator-reserved (LANE-BRIEF §0) and must be the last action before a tag.

So C5/C3/C4 cannot be *landed* by this lane. What this lane can do is land the gates
that catch them (C2), land C1, and hand the orchestrator a proven, ready-to-apply
generator fix.

## Plan

1. C2a — variant-coverage gate (`ProtocolEvent` variants vs manifest). Catches C4.
2. C2b — schema-satisfiability gate (published schema must accept every corpus fixture,
   and no discriminator may appear in two branches). Catches C5 and C3.
3. C1 — release.yml contract bundle, ordered before `manifest-build`.
4. Generator fix, delivered as a fenced seam request + a proof harness, NOT committed
   as a live generator change (it would redden `check_contract`).
