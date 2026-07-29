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
