# NOTES — lane `cont-skills-cache`

Base: `b2ddf113681647221dc9e5bbfc7de79b1da90b54` (integration `plan/f20-unified-audit-repair`).
Branch: `lane/cont-skills-cache`.

Scope: the two unbuilt sub-areas of `CONT-*` —
1. governed skills (`23A-C1`), contested between a 2026-07-28 NOT-MET and a 2026-07-30 MET re-grade;
2. cache economics (`F23-04`, `F05-TRUTH-1`, `F05-TRUTH-3` — "Unavailable: no production constructor").

Append after EVERY measurement. Do not batch.

---

## Instrument log

- **`--include=*.rs` unquoted was eaten by zsh** (LANE-BRIEF §3b-i, verbatim). First readers-grep
  returned `(eval):1: no matches found` for all four searches. **The known-positive caught it**:
  `GovernanceStore` came back `0` when it must be non-zero. Re-run with `--include='*.rs'`:
  `GovernanceStore` = 56, known-negative `zzz_no_such_symbol` = 0. Instrument alive in both
  directions.
- All counts below captured by redirect-to-file + Read tool, never read through a Bash pipe
  (LANE-BRIEF §3b `--numstat` finding).

---

## M1 — does the governed-promotion surface exist at HEAD?

Capture: `/tmp/cont-skills-cache-m1.txt`, `/tmp/cont-skills-cache-m2b.txt`.

`crates/wcore-cli/src/skill_govern.rs` **EXISTS at HEAD**, 14143 bytes. Ships four verbs:

| verb | fn | line |
|---|---|---|
| `--skills-govern` | `run_list` | `skill_govern.rs:96` |
| `--skills-revoke <NAME>` | `run_revoke` | `skill_govern.rs:212` |
| `--skills-rollback <ID>` | `run_rollback` | `skill_govern.rs:238` |
| `--skills-promote <NAME\|UUID>` | `run_promote` | `skill_govern.rs:256` |

`revoke` = 72 hits, `rollback` = 110 hits under `crates/wcore-cli/src/`.

**Provisional: the 2026-07-30 re-grade reading is the true one on presence; the 2026-07-28
competitive-ledger row is STALE.** The ledger's claim that "`promote`, `revoke` and `rollback` do
not exist" and that "`run_skills_promote` fails closed at `wcore-cli/src/main.rs:2408`" is false at
`b2ddf113`. Still to establish: whether the surface is *load-bearing* or a zero-reader façade.

## M2 — is promotion load-bearing, or a zero-reader status field?

This is the real question and the one the re-grade does not fully answer. Readers found:

- `crates/wcore-skills/src/loader.rs:185` `apply_governance()` — the catalog choke point, called
  from `load_all_skills` at `:156`. Reads `store.live_revocations()` + `store.promotions()`
  (`:191`), drops revoked skills from the catalog entirely (`:228-236`), and lifts
  `disable_model_invocation` for a promoted generated draft (`:244-257`).
- `crates/wcore-agent/src/bootstrap.rs:4255` and `crates/wcore-agent/src/slash/skill.rs:319` also
  read `live_revocations()`.

So promotion/revocation have **real readers in the shipped agent path**, not zero. Still to prove
by execution, in BOTH directions:

- [ ] unpromoted generated draft is genuinely NOT model-invocable (can-fail direction)
- [ ] promoted draft IS model-invocable (can-pass direction — the §3b-iii control)
- [ ] revoked skill is absent from the catalog; rollback restores it

## Open

- [ ] `F23A-01-H2` — "any errored tool call kills the session". Gap-ledger line 528 says FIXED at
      `32a5fc90` with five wired regression tests; competitive ledger says open and committed red.
      Contested — measure at HEAD.
- [ ] Cache economics: locate the F05-TRUTH-1 (pricing refresher) and F05-TRUTH-3 (cooldown
      tracker) types and count production construction sites.
