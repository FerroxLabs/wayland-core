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

## M3 — EXECUTED tests at HEAD (hetzner `/root/wayland-cont-skills-cache`, SHA asserted `b2ddf113`)

Counts read back including `ignored` / `filtered out` (LANE-BRIEF §3.2). Captures alongside.

| suite | result |
|---|---|
| `wcore-skills --test govern_catalog_enforcement` | **5 passed; 0 failed; 0 ignored; 0 filtered out** |
| `wcore-skills --test govern_revoke_rollback` | **15 passed; 0 failed; 0 ignored; 0 filtered out** |
| `wcore-skills --test govern_cli_drive` | **6 passed; 0 failed; 0 ignored; 0 filtered out** |
| `wcore-skills --test govern_staging_discovery` | **2 passed; 0 failed; 0 ignored; 0 filtered out** |
| `wcore-cli --test skills_promote_advertised_and_works` | **5 passed; 0 failed; 0 ignored; 0 filtered out** |
| `wcore-agent --lib d1_refusal_terminal_tests` | **5 passed; 0 failed; 0 ignored; 2232 filtered out** |

The last row's `2232 filtered out` is the anti-vacuity read-back: the name filter matched 5 real
tests, not the 0 that flavour (c) of the self-passing class produces.

`govern_catalog_enforcement.rs` runs the control in BOTH directions on the **production catalog
path** (`load_all_skills`, the function the engine calls), with a known-positive before every
absence assertion.

## M4 — LIVE journey through the shipped binary (`wayland-core 0.12.25`)

`LIVE-GOVERN-JOURNEY.txt`. Both directions at the product surface, control skill `wl-control`
held constant across every step:

| step | direction | result |
|---|---|---|
| promote `wl-subject` | CAN-PASS | `status=installed` → `status=promoted`, digest + authority + promotion id recorded |
| promote `wl-no-such-skill` | CAN-FAIL | **RC=1**, `Error: no skill named 'wl-no-such-skill' is installed` |
| revoke `wl-subject` | CAN-PASS | leaves `INSTALLED`, appears under `REVOKED (1)`; grant auto-`WITHDRAWN` |
| rollback bogus id | CAN-FAIL | **RC=1**, `Error: no revocation with id 'not-a-real-revocation-id'` |
| rollback real id | CAN-PASS | returns as `status=installed` — re-quarantined, **not** promoted |
| `--help` | advertised | all four verbs present, `GREP_RC=0` |

## VERDICT ON `23A-C1` — the 2026-07-30 re-grade is TRUE at HEAD; the CONT-* row is STALE

Three specific competitive-ledger claims measured FALSE at `b2ddf113`:

| ledger claim (2026-07-28) | measured at HEAD |
|---|---|
| "`promote`, `revoke` and `rollback` do not exist" | **FALSE** — all four verbs in `skill_govern.rs`, live-exercised above |
| "`run_skills_promote` fails closed at `wcore-cli/src/main.rs:2408`" | **FALSE** — `main.rs:2687` delegates to `skill_govern::run_promote` |
| "satisfied only by the absence of any promotion path … vacuous satisfaction" | **FALSE** — no longer vacuous. `promotion_lifts_quarantine_and_an_edit_puts_it_back` constructs the passing world *and* the failing one |

## `F23A-01-H2` — CLOSED, and now EXECUTED rather than source-verified

`23A-STATUS-CORRECTION.md` §1 verified the fix from source and git and said plainly *"Not re-run by
this lane"*. **Re-run here: 5/5 pass at HEAD.** The competitive ledger's "open and committed red" is
stale. The control `approval_denial_control_leaves_turn_committable` is among the five, so the four
subject tests are falsifiable.

## M5 — CACHE ECONOMICS: the ledger is wrong on column 1 and right on column 2

Both F05 rows read `Unavailable: no production constructor`. **Column 1 is false; column 2 is
the real, unbuilt gap.**

Production construction sites (unproxied grep, known-positive `mid_flight_monitor`=10, known-negative
`zzz_no_such_symbol`=0):

- `PricingRefresher` — `wcore-agent/src/bootstrap.rs:4073` (`build_fallback_providers`) and `:4143`
  (`refresh_pricing_cache_if_enabled`). **Two production sites, not zero.**
- `CooldownTracker` — `wcore-providers/src/resilient.rs:257` (per fallback) and `:263` (primary),
  reached from `bootstrap.rs:1039`, which is **unconditional** on the session bootstrap path.
  **Not zero.**

So the shipped binary reports `cooldown_tracker` `ready` and `pricing_refresher`
`disabled_by_config` — exactly what `lane/22-remaining` flagged and deliberately left for this row.

**Column 2 (`Runtime outcome proof: None`) is TRUE and is the unbuilt work.** Only five
`successful_occurrence` call sites exist (`engine.rs:5220, 5585, 6152, 6173, 6181`) covering
`ProcedureSkillDrafting`, `LegacyAutoSkillDrafting`, `SmartHandoff`, `LearnedPolicy`,
`MidFlightMonitor`. **Neither cache-economics capability emits one.**

### Two zero-measurement surfaces found in the honesty reporter itself

1. **`bootstrap.rs:3040` `cooldown_tracker_constructed: true` is a hardcoded literal.** Every peer
   input is a real fact (`engine.skill_drafter().is_some()`,
   `engine.midflight_monitor_constructed()`, `engine.learned_policy_constructed()`). This one cannot
   be false, so `capability_activation.rs:92-97` — the `NoProductionConstructor` arm for
   `CooldownTracker` — **has no reachable state in production.** That is LANE-BRIEF §3b-iii's
   permanently-green inverse, sitting inside the machinery whose entire job is honest reporting.
2. **`bootstrap.rs:2884` `pricing_refresher_constructed = self.config.provider_chain.enabled`** is a
   *config* read assigned to a `*_constructed` field, directly against
   `StartupCapabilityInputs`'s own doc: *"Configuration can request a capability without its
   dependencies actually being available; keeping those facts separate prevents configured from
   becoming ready by implication."*

## Done

- [x] Runtime outcome proof for `CooldownTracker` (on `CircuitState::Open` only) and
      `PricingRefresher` (on a published live snapshot only).
- [x] Both unmeasured startup inputs replaced with real facts.
- [x] Both-directions controls: unit (3 new, all mutation-proven falsifiable) and live
      (two one-variable differentials through the shipped binary).

**Final write-up: `CONT-SKILLS-CACHE-SUMMARY.md` in this directory.** These notes are the running
log; the SUMMARY is the deliverable.
