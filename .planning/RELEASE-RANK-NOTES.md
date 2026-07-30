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
| 3 | 3 / `24-C5`+`24-C1` | `24-C5` MET; `24-C1` PARTIAL, no-loss failing 9/10 adapters | **SPLIT CONFIRMED; "9 of 10" is WRONG — it is 7 of 10** |
| 4 | 4 / `23A-C1` | MET on shipped surface, no longer blocking | **CONFIRMED CLOSED** |
| 5 | 5 / `27-C2(b)` | closed by liveness probe (NOT `bootstrap.rs:754`, which is `true` forever) | **CONFIRMED CLOSED** |
| 6 | 6 / `24-C3` | still genuinely NOT MET | **GRADE SURVIVES; "open unfixed HIGH" is WRONG — F24-C3-H5 is fixed** |
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

### NEW INSTRUMENT FINDING — `rtk` proxies `find` too (a SIXTH tool)

Not on LANE-BRIEF §3b's list. `find . -iname '*journey*' -not -path './.git/*'` returned
**nothing**, and the reason was `rtk: rtk find does not support compound predicates or actions
(e.g. -not, -exec)` — a **tool refusal rendered as an empty result set**. I had already written
"no journey artifacts on disk" into a draft. The truth, via `/usr/bin/find`, is that
**all three platform journey logs exist.** This is §3b-i exactly: an absence claim passing for
free on a dead instrument. Behaviour is also *inconsistent* — an earlier `find ... -not -path`
in the same session DID return results, so the refusal is not deterministic and cannot be
detected by eye. **Use `/usr/bin/find`.** Written up for the brief.

### H3 — split CONFIRMED, but the "9 of 10" number is WRONG

**`24-C5`: MET.** `24-C5-finish-evidence/` holds `linux-journey-at-candidate.log`,
`macos-journey.log`, `windows-journey.log` — each a **17-step** setup-to-recovery journey
(preflight → binary-identity → install → start → automation-add → deliveries → **hard-kill** →
platform-recover → delivery-reconcile → **upgrade-in-place** → **rollback** → redaction-canary →
drain-uninstall-clean), all `OK`, each ending `JOURNEY COMPLETE`, with receipts of 10.5 / 11.0 /
14.0 KB. Both non-Linux logs terminate with the **sentinel pair LANE-BRIEF §3.2 mandates**
(`MACRC=0`/`MACDONE`, `JOURNEY_RC=0`/`JOURNEY_FINISHED`) rather than a bare exit status.
Driver `wcore-eval-scenarios/bin/wayland-journey.rs`, contract
`tests/journey_receipt_contract.rs`. **All three of this row's claimed absences are false.**

**`24-C1`: PARTIAL — and the residual is SMALLER than the ledger says.**
The ledger (`:624-627`) asserts Slack is *"the sole override of a trait method that defaults to
`false`"*. **At HEAD there are THREE overrides**, not one:

| Adapter | Site | Wire mechanism | Bound by test |
|---|---|---|---|
| Slack | `wcore-channel-slack/src/lib.rs:249` | dedupe key on the wire | `:502` |
| **Matrix** | `wcore-channel-matrix/src/lib.rs:294` | `{txnId}` path segment of the send PUT | `:521 matrix_declares_idempotency_only_because_the_txn_id_is_derived_from_the_key` |
| **Discord** | `wcore-channel-discord/src/lib.rs:344` | `nonce` field of create-message | `:563 discord_declares_idempotency_only_because_the_nonce_is_derived_from_the_key` |

Both new ones were done *properly*: each flipped from `false` **only after** the underlying id
stopped being restart-unstable (Matrix's counter reset to 1; Discord's nonce was deliberately
distinct across restarts), and each comment states that flipping earlier *"would have converted a
visible duplicate into an invisible one."* Default remains `false` at
`wcore-channels/src/lib.rs:139`.

**So exactly-once is 3 of 10, not 1 of 10; no-loss fails on 7 of 10, not 9 of 10.** Those 7 are
precisely the set the ledger itself identifies as having **no idempotency primitive at all**
(Telegram, Twilio SMS, Meta Graph/WhatsApp, SMTP, signal-cli, AppleScript iMessage, MS Teams).
**Every adapter that could be fixed in code has been fixed.** The residual is now 100% the
product decision and **zero lane-sessions of implementation** — the ledger's own "≈0.5 session
each for Matrix and Discord" is spent.
Operator recovery surface also confirmed built: `gateway.rs:820 resend_needs_confirmation`,
`:861 async fn resend(.. also_ack)`, `:847` acknowledge, regression asserts at `:2080-2088`.

### H6 — `24-C3` grade SURVIVES, but its stated reason does NOT

**NOT MET stands.** `24-C3-FINISH.md:15` — *"Five lanes have now declined to mark it."*
Measured at HEAD, per-adapter override tally across all ten channel crates:

```
adapter    edit_message  delete_message  react  fetch_media
discord         0              0           2         1
email           0              0           0         1
imessage        0              0           0         1
matrix          0              0           2         1
msteams         0              0           0         0
signal          0              0           0         1
slack           0              0           1         1
sms             0              0           0         1
telegram        0              0           1         1
whatsapp        0              0           2         1
```

- **`edit`/`delete`: 0 of 10.** Genuinely absent.
- **`react`: 5 of 10.**
- **`fetch_media`: 9 of 10** (all but MS Teams). **The handoff's *"media and native actions still
  at zero"* is false for media** — that phrasing describes the *criterion clause* (never driven
  end-to-end from the binary), not the code, and the two must not be conflated.

**The "open unfixed HIGH" is CLOSED.** `F24-C3-H5` (`channel reload` registers an adapter without
its access policy) is repaired at HEAD across **both** facets, via a new shared module
`wcore-agent/src/channel_policy.rs` — *"This is the ONLY place either map is built"* (`:89`) —
read through the shared registry on **every** message (`channel_inbound.rs:261`), tool posture
included (`channel_dispatch.rs:341`). Regression test
`wcore-agent/tests/f24_c3_h5_reload_policies_test.rs`: **2 tests, 0 `#[ignore]`**, asserting
*"registered, healthy, 200 — and every message denied"* is the state that must redden.

**Crucially for ranking:** the absent native actions are NOT a silent-failure surface. The trait
defaults return a **named** `ChannelError::Unsupported` (`wcore-channels/src/lib.rs:204, :220,
:300`), pinned by `framework_matrix.rs:412
edit_delete_and_react_default_to_a_named_unsupported_never_a_silent_ok`.

### MOVING THE WRONG WAY — an RC blocker §3 has NO ROW FOR

`crates/wcore-config/src/config.rs:4431` at **integration HEAD**:

```rust
enabled: global.security.enabled && project.security.enabled,
```

`enabled = true` means the egress boundary is ON, so `&&` lets **either** layer switch it off —
including a project config the same file twice calls *"untrusted (checked into a cloned repo)"*
under GHSA-8r7g. Reinforced at `:4546` `restricted.security.enabled = project.security.enabled;`
inside `restrict_untrusted_project_config`, whose own comment at `:4537` reads *"Preserve project
narrowing, never project grants"* and at `:4543` *"A repository may **tighten** egress"* —
**but `false` loosens it.** The code contradicts its own stated contract.

**The correct shape already exists 13 lines below**, for a different field: `:4559` —
*"Only an explicit `false` is carried forward. `Some(true)` is deliberately NOT preserved"* —
i.e. the presence-aware treatment `lane/egress-merge-polarity` independently concluded is needed.

**Status: NOT merged to integration.** `lane/egress-merge-polarity` holds the fix and has already
shown the orchestrator's originally-prescribed `||` is itself defective (`security.enabled`
defaults to `true`, so `||` breaks the operator's off switch). Graded here as **NOT YET MERGED**
per my brief. Same for `lane/cli-danger-tiers`.

### Other CAN SHIP OPEN rows re-checked — no adverse movement

- **`27-C4` (voice)** — still off by default. `wcore-cli/Cargo.toml:31`
  `default = ["remote-registry", "workflow", "monitor", "review_artifact"]`; `voice` is opt-in at
  `:58`. Classification holds: not a surface because it is not shipped.
- **`27-C5` / platform envelope** — `release.yml` still ships **6 targets across 3 OS families**
  (`:64-80`). The envelope has NOT narrowed, so the default remains that it blocks. The two
  aarch64 targets are verified by **file-header shape only** and never executed (`:624`, `:660`) —
  honestly disclosed in-workflow, still NOT MEASURED as running code.
