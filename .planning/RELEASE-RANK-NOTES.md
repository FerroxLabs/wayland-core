# RELEASE-RANK NOTES — live investigation log

Lane `release-rank`. Branch `lane/release-rank`, forked from integration head
`8955ee6e43d2a6bd6ede0a522eb19cd2eddaaad7`.

**Mandate:** re-rank `CRITERIA-GAP-LEDGER.md` §3 (MUST CLOSE / CAN SHIP OPEN) measured at HEAD.
The existing §3 was written by `lane/criteria-gap` at `873cc389` and is stale.

**Method constraint accepted up front:** grade off the **code and tests at HEAD**, never off a
`SUMMARY.md`. Where a summary and the source disagree, the source wins and that is stated.
Every gate run in **both directions** (LANE-BRIEF §3b-iii): show the instrument can fail *and*
can pass. All load-bearing greps via `/usr/bin/grep`; all git via `/usr/bin/git`.

**This lane modifies no `.rs` file.** Measurement + documentation only.

---

## Status: IN PROGRESS

## Hypotheses handed to me (orchestrator's own words: "probably stale")

| # | §3 item | Claim | Verdict |
|---|---|---|---|
| 1 | 1 / `24-C2` | 3 of 8 trigger kinds can never fire; "worst failure mode in the ledger" | **CONFIRMED STALE** |
| 2 | 2 / `27-C2(a)` | `[browser]` → `[browser.policy]` remediation string | **CONFIRMED CLOSED** |
| 3 | 3 / `24-C5`+`24-C1` | `24-C5` MET; `24-C1` PARTIAL, no-loss failing 9/10 adapters | TBD |
| 4 | 4 / `23A-C1` | MET on shipped surface, no longer blocking | **CONFIRMED CLOSED** |
| 5 | 5 / `27-C2(b)` | closed by liveness probe (NOT `bootstrap.rs:754`, which is `true` forever) | **CONFIRMED CLOSED** |
| 6 | 6 / `24-C3` | still genuinely NOT MET | TBD |
| — | CAN SHIP OPEN | anything moved the *wrong* way; specifically `27-C3` cost record | **COST RECORD EXISTS — caveat dead** |

## Instrument discipline in force

Integration moved to `27c30527` while I was reading; merged forward, HEAD now on `lane/release-rank`.
That commit records the worst `rtk` instance yet: **`git diff --numstat` returned a fabricated
count and `/usr/bin/git` did NOT protect against it.** Repair adopted here for every measurement:
redirect to a file under `/tmp/release-rank-caps/` (lane-unique per §6a-ii), read the file with the
Read tool, and carry a **known-positive and a known-negative in the same capture**.

Two instrument faults already caught by that discipline, in my own first two captures:
- a known-positive that returned **0** (I pointed it at `wcore-agent/src/cron.rs`, which is 28.4K
  but no longer contains the trigger machinery — that moved to the new `wcore-cron` crate). Had I
  not run the control I would have read a real zero as a dead instrument, or worse, the reverse;
- **zsh ate `--include=*.rs`** (`(eval):1: no matches found`), exactly as LANE-BRIEF §3b-i warns.
  Unquoted, that grep searched nothing and would have returned a free zero for any absence claim.

## Measurements taken so far

### H1 — `24-C2`: the "worst failure mode in the ledger" sentence is FALSE at HEAD

- `crates/wcore-cli/src/cron.rs:52-54` — `--help` now says *"`webhook:` and `poll:` are NOT
  accepted: nothing in this build can fire them."* They are no longer advertised.
- `refuse_without_producer()` at `cron.rs:477`, called at **`:434`, `:449`** (the `add` and
  `--describe --confirm` paths). Refusal is implemented, not just documented.
- Pre-existing persisted jobs print `WILL NEVER FIRE — {reason}` at `cron.rs:350`.
- `no_producer_reason()` at `wcore-cron/src/trigger.rs:403` returns `Some(..)` for **only**
  `Webhook`/`Poll`; `has_producer()` at `:397` is `!matches!(self, Webhook | Poll)`.
- **`event:` has a real producer AND a proven end-to-end fire.** `cron publish <topic>`
  (`CronCmd::Publish`, `wcore-cli/src/cron.rs:112`, dispatched `:235`, `publish_cmd` `:271`).
  `wcore-cron/tests/event_producer.rs:90 a_published_event_actually_fires_its_subscribed_job`
  drives `tick_once_at` and **carries its own known-negative in the same test body** (ticks once
  before publishing and asserts nothing fired) — a both-direction control I did not have to add.
  11 tests in that file, **0 `#[ignore]`** across all 5 `wcore-cron` test files.
- Residual ledger claim *"`max_in_flight` is stored and clamped but not enforced at dispatch"*
  is **still true but no longer silent**: `wcore-cli/src/cron.rs:806-808` prints
  `NOTE: fires are serialized; max_in_flight>1 grants no ...`, and
  `wcore-cron/tests/in_flight_bound.rs:287` is named
  `the_runner_enforces_the_other_two_bound_fields_and_not_this_one`.

**Verdict: grade stays PARTIAL, but the ranking basis is dead.** Silent acceptance is gone.

### H2 — `27-C2(a)`: CLOSED

`crates/wcore-browser/src/config_hint.rs` now owns the remediation snippets and emits
`[browser.policy]` (`:29`, `:37`). Guarded by a test at `:90-91` asserting **no snippet may name a
bare `[browser]`**. `tool.rs:500-501` retains only a comment describing the old bug.

### H4 — `23A-C1`: CLOSED (real implementation, not merely de-advertised)

`main.rs:1588-89` → `run_skills_promote` (`:2657`) → `wcore_cli::skill_govern::run_promote`.
The `bail!` is gone. `main.rs:477` comments *"The flag was hidden while `run_skills_promote` was an
unconditional [bail]"* — past tense, and it is now unhidden. Dedicated test file
`wcore-cli/tests/skills_promote_advertised_and_works.rs`, incl. `:101
advertised_skills_promote_actually_promotes_and_records_provenance`.
**This is stronger than the ledger's accepted minimum** (which was `hide = true` + a Known Issue).

### H5 — `27-C2(b)`: CLOSED, and the old re-grade's line number was wrong

Readiness is published at **`wcore-agent/src/bootstrap.rs:940-941`** —
`PluginCapabilitySet::from_verified(&verified_plugins).narrowed_to_live()`. NOT `:754`.
`narrowed_to_live` (`output/protocol_sink.rs:186`) runs `wcore_browser::liveness::probe(..)` and
`wcore_cua::liveness::probe()`, **can only clear a flag, never set one** (`:166-168`), keeps
`Indeterminate` rather than under-advertising, and WARN-logs each narrowing with reason+remedy.
Guarded by `wcore-agent/tests/capability_liveness_narrowing.rs`, whose header states its purpose is
to redden **if `narrowed_to_live` became a no-op** — the anti-vacuity control this row needed.

### CAN SHIP OPEN — `27-C3`'s escalation caveat is DEAD

The ledger §2 footnote reads: *"`27-C3` flips to blocking if media generation is billable — media
calls currently produce **no cost record**."* **A cost record now exists.**
`crates/wcore-tools/src/media_cost.rs` (27.0K) defines `MediaCostLedger`, `MediaRateCard`,
`MediaCostRecord`, `MediaOutcome`, `MediaUnits`, `ReportedCost`. Wired at
`bootstrap.rs:1317 .with_rate_card(..)`, consumed by `wcore-cli/src/image.rs:29` and
`wcore-tools/src/image_generation_tool.rs:67`. `image_gen.rs:221-223` reads
`x-flux-cost-usd` / `x-cost-usd` / `openai-processing-cost-usd` headers plus `/usage/cost_usd`
body paths. Unit test `media_cost.rs:536 unreported_cost_is_unpriced_not_zero` is the honest-
accounting guard, and `:605 provider_reported_cost_outranks_rate_card` pins precedence.
**Caveat cannot fire. `27-C3` does not escalate.**
*Scope limit, measured:* `video_analyze.rs`, `tts.rs`, `voice_mode.rs` have **zero** cost hits —
the ledger exists for **image** generation only.
