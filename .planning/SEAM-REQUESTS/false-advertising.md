# Seam request — lane/false-advertising

**One request, and it is not the one this lane expected to file.**

Measured base: `d53fd54a9976cf71407c70a21ea22d89a5ae6a1e`.
Lane head: `b7e16e29`.

---

## SR-FA-1 — regenerate the Desktop contract corpus ONCE over the merged tree

**Files (all FENCED):**
`crates/wcore-protocol/contracts/desktop/v1/manifest.json`, `events/ready.json`,
`adversarial/events/{fixture,schema,version}-mismatch.jsonl`.

**Action:** `wcore-contract generate` (or `just desktop-contract-check`'s write
path), run **once, after every lane has merged**, on Sean's authorization. This
lane did NOT run it.

### Why this is required, measured rather than assumed

`crates/wcore-protocol/src/contract/spec.rs:833` — `SOURCE_INPUTS` — digests
**forty engine source files**, not just the protocol crate. Among them:

```
crates/wcore-agent/src/output/protocol_sink.rs
crates/wcore-agent/src/bootstrap.rs
crates/wcore-cli/src/main.rs          <-- the shared file EVERY lane edits
crates/wcore-tools/src/registry.rs
crates/wcore-mcp/src/manager.rs
... 35 more
```

So `source_inputs_digest` moves for **any** edit to those files. `events/ready.json`
embeds the descriptor, and `insert_negotiation_fixtures` writes it into the three
adversarial negotiation fixtures, so `fixture_digest` moves with it.

**Consequence the orchestrator must plan for:** `wcore-cli/src/main.rs` is both a
digested input and the file `LANE-BRIEF.md §6` designates as the shared additive
file every lane touches. **Every lane that edits it is therefore red on
`wcore-protocol --test desktop_contract_corpus`, through no wire-shape change of
its own.** One regeneration over the merged tree clears all of them; N per-lane
regenerations would conflict on a byte-exact artifact. `SEAM-REQUESTS/27.md`
already anticipated exactly this ("do NOT bump twice ... produced once, from the
merged tree, after both source changes are in").

### What actually moved — read-only measurement, no `generate` run

`cargo run -p wcore-protocol --bin wcore-contract -- digest` (the read-only
`digest` subcommand, not `generate`):

| Digest | base `d53fd54a` | lane head `b7e16e29` | moved? |
|---|---|---|---|
| `schema_digest` | `sha256:e5d1744a…2e54` | `sha256:e5d1744a…2e54` | **NO** |
| `source_inputs_digest` | `sha256:25170996…9336` | `sha256:2ec10eab…1aa18` | yes |
| `fixture_digest` | `sha256:634bbbe9…30fa` | `sha256:0a496996…a010` | yes |
| `generator` | `wcore-desktop-contract-gen/11` | `wcore-desktop-contract-gen/11` | **NO** |

**`schema_digest` is unchanged.** No wire shape changed: no field added, removed,
renamed or retyped; no `CapabilityId` variant added; no `CONTRACT_MINOR` bump
needed or taken. The corpus drift set is `missing=[], extra=[]` and exactly the
five descriptor-carrying artifacts — no `schema/*.json` drifted.

This is **byte-for-byte the same shape** as the regeneration Sean authorized at
`c743f398` (per `HANDOFF-2026-07-28.md` §2: "`schema_digest` **UNCHANGED**, only
`fixture_digest` and `source_inputs_digest` moved, no non-digest change").

**Still owed, unchanged from that precedent:** Desktop must re-pin in the **same
release train**. `observation.rs:342` compares `source_inputs_digest` and
`observation.rs:329` makes a mismatch a hard error at `ready` negotiation, so an
un-re-pinned Desktop will not connect.

---

## What this lane did NOT request

- **No `CONTRACT_MINOR` bump.** SR-27-2 asked for 8 → 9 for the richer
  `chain-plus-derived-flags` design (new `CapabilityId::{Browser, ComputerUse,
  Web}`, an activation ladder, reason codes). **None of that was implemented
  here** and SR-27-1..3 remain open and unstarted.
- **No hand-edit of any contract artifact.**
- **No change to `crates/wcore-protocol/`.** `git diff <base> -- crates/wcore-protocol`
  is empty.

---

## A correction this lane had to make to itself

A cross-audit panel was asked whether narrowing an existing boolean's *value*
required a contract bump, and answered **3 of 3: no**. That answer is correct on
the question asked — the wire *schema* genuinely does not change, and
`schema_digest` measured identical, confirming them.

**But the question was incomplete, and I wrote it.** I did not know
`source_inputs_digest` covers engine source files, so I never asked. The panel
could not have supplied a fact absent from the prompt. The contract-corpus test
supplied it instead, by going red.

Two things follow. Recording them because both are cheap to re-learn the
expensive way:

1. **A panel cannot audit a premise you did not give it.** Three confident,
   well-reasoned, mutually-independent agreements did not make the plan safe.
   The artifact did.
2. **The relevant reflex was the right one anyway:** run the fenced crate's own
   tests even when your diff does not touch that crate. Had this lane trusted
   "I changed no protocol file, so the protocol is fine", it would have handed
   the orchestrator a silent red.
