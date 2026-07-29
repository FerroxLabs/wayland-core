# 23B C3 — user-model half — NOTES (append-only, committed as I go)

Lane `23b-c3-usermodel`. Base `plan/f20-unified-audit-repair` @ `eaff921d`.

## T+15min — the headline finding, before any fix

**There are TWO disjoint "user model" surfaces, and the one G3c names is NOT the one
that reaches the model.** This is the same defect shape the memory half just closed
(controls on `Partition::Episodic` while only `Partition::Semantic` reached the prompt),
and it would have swallowed a literal reading of G3c whole.

| | surface | mutation verbs | reaches the outbound prompt? |
|---|---|---|---|
| **A** | `wcore-memory` P5 `user_model` k/v partition — `MemoryApi::update_user_model` / `user_model()` | `update_user_model` (**`AccessToken::System` only**) | **NO** |
| **B** | `wcore-user-model` `UserModelBackend` → `UserBrief` + `Preferences` | **`observe` only** — an EMA inference fold. **No correction verb exists at all.** | **YES** — `bootstrap.rs:2108-2118` |

### Evidence for "A does not reach the prompt"

Readers of `MemoryApi::user_model()` (`/usr/bin/grep -rn "\.user_model(" --include="*.rs" crates/`,
filtered of `update_user_model`) — 16 hits, every one of which is a test, a fixture, a
trait-forwarding impl, or a **display** surface:

- `wcore-agent/src/slash/memory.rs:608` — `/memory show` renders it as text to the user
- `wcore-cli/src/main.rs:2810` — CLI display
- `engine.rs:21011`, `21166` — decorator forwarding
- `memory.rs:334`, `null.rs:104`, `e2e_fixture.rs:259` — plumbing/fixtures
- remainder are `crates/**/tests/`

Concept search (not one keyword) over the two files that assemble the system prompt
(`bootstrap.rs`, `engine.rs`) for `user.?model|user_ctx|user_context|UserBrief|Preferences`:
the P5 partition appears **only in comments and in the session-end write** at
`engine.rs:14080`. Instrument alive in the same invocation: the known-positive
`fn recall_relevant_facts` returned `engine.rs:13696`.

### Evidence for "B reaches the prompt"

`bootstrap.rs:2108`:
```rust
let user_ctx_block = if let Some(b) = user_model_backend.as_ref() {
    let brief = b.brief(user_id).await.unwrap_or_default();
    let prefs = b.preferences(user_id).await.unwrap_or_default();
    crate::user_context::render_user_context_block(&brief, &prefs)
} else { None };
let mut system_prompt = system_prompt;
if let Some(block) = user_ctx_block { system_prompt.push_str(&block); }
```
Sole call site of `render_user_context_block` outside its own module.

### So G3c as written is real but aimed at the dead half

G3c: "`update_user_model` is `SystemToken`-only and `UserModelInferencer::infer` overwrites at
every session end." Both clauses are TRUE (`core_inference.rs:120-128`, `engine.rs:14074-14090`).
But a user correction to surface **A** would not have reached the model even if it survived.
**Fixing only A would have reproduced the memory half's mistake exactly.**

The real defect on the wire-bearing surface **B** is not a precedence bug — it is an
**absence**: `UserModelBackend` has `brief` / `preferences` / `observe` / `backend_tag`
and **no correction, no forget, no user-authored write of any kind**
(`wcore-user-model/src/lib.rs:37-55`). The only mutation is `observe`, which EMA-folds
(`local.rs:100-111`), so even a hypothetical correction would be diluted turn by turn.

### Consequence for the fix

The fix must land on **B** or it is not on the wire. Plan: a user-authored correction
layer with provenance that inference cannot touch, winning at the render site; plus
close A's clobber so the two halves agree.

## Still to establish
- [ ] does P5 storage carry any provenance/source column (`partition/core.rs`)?
- [ ] `nudges` — decide caller vs. de-advertise
- [ ] wire proof harness: correct → session end → new session → outbound body

## T+40min — design settled, both halves, and the P5 storage answer

`partition/core.rs` (111 lines) — `CorePartition::update` is a bare
`INSERT INTO user_model (key, value_json, ts) ... ON CONFLICT(key) DO UPDATE SET
value_json = excluded.value_json, ts = excluded.ts` (lines 57-61). `UserModelEntry` is
`{key, value: Value, ts}`. The DDL (`schema/v1.sql:112-116`) is three columns:
`key TEXT PRIMARY KEY, value_json TEXT NOT NULL, ts INTEGER NOT NULL`.
**No source/provenance column, no origin, nothing.**

So on surface A there is nowhere to record "a human said this" — which is *why* the
inferencer can clobber: the row it overwrites is indistinguishable from one it wrote.

### The design I am building

**One provenance concept, applied on both surfaces, precedence at the point of use.**

- **B (the wire-bearing half)** — add `UserCorrection` + two trait verbs to
  `UserModelBackend`: `correct(user_id, correction)` and `corrections(user_id)`.
  `LocalBackend` stores them in a **separate `corrections` map on `UserRecord` that
  `observe` never reads or writes**, so no amount of inference can dilute one.
  `render_user_context_block` renders corrections **after** and **overriding** the
  inferred values, labelled as user-stated.
- **A (the named G3c half)** — `update_user_model` grows an origin so a user-authored
  entry is distinguishable, and `UserModelInferencer`'s persist path **skips a key a user
  has authored** instead of overwriting it.

Rejected alternative: wiring A into the prompt. That changes prompt content for every
existing user and duplicates B's block; the criterion asks for control over what the
agent believes, not for a second copy of it.
