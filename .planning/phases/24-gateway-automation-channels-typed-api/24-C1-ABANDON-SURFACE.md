# 24-C1 — the abandon surface, two adapter dedup tokens, and a measured loss path

Lane `lane/24-abandon-surface`. Merge-base `1b2577e1b61447f1599e127679b8e2eb3552b61b`.
Evidence: `evidence/24-C1-abandon-surface/`.

**Verdict: all three tasks achieved.** Task 1 built and live-proven against a real
abandonment. Task 2 fixed for both adapters with the capability claim bound to the wire.
Task 3 measured against a real homeserver, graded **HIGH**, and the fix proven rather than
assumed.

---

## Correction to the brief, up front

The brief cites `crates/wcore-channels/src/ledger.rs:214`. **There is no `ledger.rs` in
`wcore-channels`.** The file is `crates/wcore-gateway/src/ledger.rs`, where the cited line
numbers land correctly. Every claim about the code was re-verified there before acting; the
substance of the brief was right.

---

## Task 1 — an abandoned delivery is now nameable

### What was actually wrong (refined from the brief)

The brief says `Abandoned` has "no consumer anywhere outside `ledger.rs`". That is **too
strong for one abandon path and exactly right for the other.** There are two:

| Site | Cause | Surface before this lane |
|---|---|---|
| `drain.rs:176` | forced drain past budget | `DrainReport.abandoned` → printed by `wcore-cli/src/gateway.rs` |
| `automation.rs:179` | outcome unknown, destination cannot dedupe (F24-C-H1) | **nothing but `tracing::warn!`** |

The drain path's listing is **ephemeral** — it exists only in the stdout of the invocation
that caused it, and a service-managed drain has no terminal. The automation path had no
surface at all, and it is the one that fires unattended, right after a crash, when nobody is
watching. `automation.rs:172-175` claimed the delivery was *"recorded, terminal, and
nameable by an operator"*: `recorded` and `terminal` were implemented, `nameable` was not.

**Two further losses found while reading, neither in the brief:**

1. **No reason was stored.** `Record` had `id`/`state`/`at`/`delivered` and nothing for WHY.
   The two causes demand opposite operator responses — a drain-budget abandonment is safe to
   re-run, an unknown-outcome one may already have landed and must be checked at the
   destination first — and the journal could not tell them apart.
2. **Compaction destroyed `at`.** `compact()` rewrote every retained record with
   `at: now`, so a surviving abandonment reported the compaction's time, not the moment the
   product gave up. The surface would have named the message and then lied about when.

### What was built

`wayland-core gateway abandoned [--json]` — a new `GatewayCmd` reusing `ScopeArgs` and
`DeliveryLedger::open`. No new subsystem. It reads the journal **from disk** rather than
asking a running gateway, because an abandonment is usually written by a process that has
since exited.

Supporting changes in `wcore-gateway/src/ledger.rs`:

- `destination` persisted at **`accept()`**, not at abandon — the drain path iterates
  `pending()` and has no target in scope, so recording it at abandon time would populate one
  path and leave the other blank.
- `reason` persisted at `abandon()` as a typed `AbandonReason`.
- `states` now holds the whole last `Record` rather than a bare `DeliveryState`, which is
  what lets compaction preserve `at` verbatim.
- **Settled and Abandoned bounded separately.** They shared one budget, so ordinary settled
  traffic could evict the record of a message the product had decided not to deliver.
  Anything dropped past `ABANDON_RETENTION` is counted into `dropped_abandonments()` and
  both warned and surfaced — never silent.
- Every new field is `#[serde(default)]`. `open()` quarantines what it cannot parse, so a
  non-defaulted field would make an upgrade read every pre-existing record as a torn tail —
  turning a version bump into a mass phantom loss. `a_journal_written_before_the_new_fields_still_loads`
  feeds the exact old on-disk shape to the new reader.

`Abandoned` remains excluded from `pending()`/`pending_count()`, deliberately: putting it
back would re-dispatch the delivery the product decided not to send. The policy is untouched
per the brief — only its visibility changed.

### Cross-audit (§4), 4-way

Two judgement calls went to the panel. **Codex 5.6 Sol hung on stdin and silently returned
39 bytes** — the §4 vote-dropping trap; re-run with `< /dev/null` for a real answer.

- **Q1, retention:** codex + gemini → own cap with the dropped count reported; kimi → retain
  all, warn at a threshold. **Taken: 2-1 majority, cap + report.** Kimi's objection — that a
  bare counter "defeats the surface" — is answered by the drop being reported and warned
  rather than silent, which preserves the non-silence property this lane exists for while
  keeping the journal genuinely bounded. Internal adversarial pass agreed: "retain
  everything" is not a bound, and an unbounded ledger is its own outage.
- **Q2, message body:** **unanimous 3-0 — id + destination only, no body.** The body is
  recoverable from the cron job the delivery id names; copying bodies into a durable
  append-only file creates an independent retention and deletion surface for personal data.

### Live proof — a REAL abandonment (`evidence/.../live-abandon-proof.log`)

Shipped release binary at `8d03c96e`, as a real systemd service, against an independent
`wayland-channel-sink` process on hetzner.

```
ARRIVALS_AT_INDEPENDENT_SINK=4              <- positive delivery COUNTED first
surface BEFORE                              -> "No abandoned deliveries recorded"
kill -9 mid-stall  -> STATE_COUNTS_AFTER_KILL = {'settled': 3, 'attempted': 1}
   CARRIED: cron:4c974ae2-...:1785271074538 state=attempted dest=f24c1csink
systemd restart, NRestarts=1
   [gateway] started pid=3023952 carried=1 (unattempted 0 / unknown-outcome 1)
   [gateway] drain Forced: observations=21 abandoned=1 flushed=true
   [gateway] ABANDONED delivery cron:4c974ae2-...:1785271074538
```

The surface then answers:

```
1 abandoned delivery in /tmp/f24c1c-run/home:

  cron:4c974ae2-91ca-4e4a-8897-89c8d74d3bbb:1785271074538
    to:     f24c1csink
    when:   2026-07-28T20:38:04.783500018+00:00
    why:    shutdown drain ran out of budget before this delivery finished
```

`--json` carries `"reason": "drain_budget_expired"`, and the raw ledger cross-check agrees
exactly (`raw_abandoned_records=1`).

**Trap discipline.** Positive delivery is counted at the independent sink *before* any
abandonment claim — a run in which nothing sends abandons nothing and agrees with itself for
the wrong reason. The surface is also shown **empty first**: a list that is never empty
proves nothing.

**Two earlier attempts are retained as measurements, not deleted:**

- `live-attempt1-drain-budget-1000ms.log` — a 1000ms budget expired before the gateway's 1s
  tick could notice the drain request. It also established that **a stalled delivery blocks
  the tick loop**, so the drain is only noticed after the stall resolves, by which time
  everything has settled. That is why the successful proof uses a `kill -9` to leave carried
  work instead.
- `live-attempt2-telegram-sink-mismatch.log` — the telegram adapter got a transport error
  from the slack-shaped sink, and **the harness aborted on zero arrivals rather than
  proceeding to a meaningless green.** The guard doing its job is itself evidence.

---

## Task 2 — both adapters were discarding a token already handed to them

The gateway's delivery key is `cron:{job_id}:{epoch_millis}[:{occurrence}]`
(`wcore-cron/src/runner.rs:324`), derived from job identity and scheduled instant, so **it
already does not move across a restart** — and it was already being passed to
`send_message_idempotent`. Neither adapter implemented that method, so both fell through to
the trait's pass-through default and the key was dropped. Each then manufactured a worse
token:

- **Matrix** — `AtomicU64::new(1)` (`rest.rs:13`), reset to 1 on every start.
- **Discord** — `next_nonce()` documented as keeping the token *"distinct across restarts"*.
  Stated as a feature; it is precisely the defect, since the cross-restart replay is the only
  one the nonce field could suppress.

Both now derive their token from the delivery key. Unkeyed sends keep a process-unique
token, because a message with no logical identity must never present a collapsible one.
**Discord hashes rather than truncates:** the key's distinguishing tail is the timestamp, and
a 25-char truncation would collapse two occurrences of one job into one nonce and lose the
second.

`supports_outbound_idempotency` flips to `true` for both — and only now. It was **honest at
`false`** before: the adapters were putting a token on the wire that could not survive the
restart it was meant to cover, and flipping it without fixing the token would have converted
a visible duplicate into an invisible one. Each claim is bound to the wire by a mock matching
the exact token, following Slack's existing pattern.

---

## Task 3 — MEASURED: the Matrix restart loss path is real. HIGH.

The previous lane flagged this and declined to grade it. Measured against a **real Synapse**
(`matrixdotorg/synapse:latest`, Docker on hetzner), one real access token, one real room.
Raw: `evidence/.../synapse-txn-reuse.log`, harness `synapse-measure.sh`.

```
--- process life 1 (counter seeded at 1) ---
PUT txn=1 body=MSG-A -> {"event_id":"$A0jD_zARYyEZ2s9hOveta3q6YgyUiblnYqGV-RsgfaY"}
PUT txn=2 body=MSG-B -> {"event_id":"$inhJrHmCdSgHuhzBUEoUZPqK-YwEoIT1LrIBFDWDyss"}
--- process life 2: SAME access token, counter RESET to 1 ---
PUT txn=1 body=MSG-C -> {"event_id":"$A0jD_zARYyEZ2s9hOveta3q6YgyUiblnYqGV-RsgfaY"}
VERDICT_A=REPLAY_SUPPRESSED
BODIES_IN_ROOM = ['MSG-B-before-restart', 'MSG-A-before-restart']
MSG_C_LOST = True
```

**The homeserver returned HTTP 200 and MSG-A's event id for a PUT carrying MSG-C's body, and
MSG-C never entered the room.** Ground truth is read from `/rooms/{id}/messages` — the room's
own contents — not from the response the sender got, because the response is exactly what
lies here.

This is a **different failure from the duplicate the earlier lanes chased, and worse in one
specific way**: a duplicate is visible at the destination; this is a disappearance that
reports success. The sender settles the delivery `delivered=true` holding a plausible
`event_id` that belongs to somebody else's message. Nothing anywhere is red.

**Graded HIGH** — silent outbound message loss on a supported channel, triggered by the
ordinary event of a process restart, with no error surfaced at any layer.

Scope, stated honestly: the collision needs the reused `(access_token, txn_id)` pair to still
be within the homeserver's transaction retention. It is therefore likeliest on the **first
sends after a restart** — which is exactly when a gateway replays its carried ledger work.

### The fix eliminates it — proven, not assumed

Same homeserver, same room, same run:

```
PUT txn=cron:job-a:1785121776528                    -> $PvBnVig5...
PUT txn=cron:job-a:1785121776528 (REPLAY, same id)  -> $PvBnVig5...   DEDUP_WORKS=yes
PUT txn=cron:job-a:1785121776529 (DIFFERENT id)     -> $rB3Eai_4...   DISTINCT_WORKS=yes
MSG_D_count = 1        (replay suppressed, not duplicated)
MSG_E_present = True   (a genuinely new delivery survived)
```

The key-derived id has no counter to reset, so the loss cannot recur through it. Unkeyed
Matrix sends are additionally reseeded from the wall clock rather than from 1, closing the
same hole for traffic carrying no delivery key.

---

## Gates — real numbers, and which run each came from

Compiled and run on `hetzner-dsm` at `8d03c96e`; only `cargo fmt --all -- --check` on the Mac.

| Gate | Result |
|---|---|
| `cargo test -p wcore-gateway --lib --test ledger_exactly_once` | **42 passed + 7 passed, 0 failed, 0 ignored** |
| `cargo test -p wcore-channel-matrix -p wcore-channel-discord` | **51 passed + 24 passed, 0 failed, 0 ignored** |
| `cargo clippy -p wcore-gateway --all-targets` | 0 warnings, 0 errors |
| `cargo clippy -p wcore-channel-matrix -p wcore-channel-discord --all-targets` | 0 warnings, 0 errors |
| `cargo fmt --all -- --check` | clean |
| Fenced files vs **merge-base** | **empty diff** — `wcore-cli/src/lib.rs` and `main.rs` untouched |
| `wcore-eval-scenarios/src/journey.rs` | not touched (0) |

Executed counts were read back, not inferred from exit status; all new tests were confirmed
present by name in the output.

**One failure in the wider run, and it is not mine.** The combined
`-p wcore-gateway -p wcore-channel-matrix -p wcore-channel-discord -p wcore-cli` run showed
`migrate_hermes::import_is_idempotent_without_overwrite` FAILED. Re-run **isolated at the
identical commit: 7 passed, 0 failed.** It is the documented wall-clock-budget flake under
full-suite load, and nothing in this lane touches hermes migration. Reported as two numbers
rather than one.

(The `always_fails` / "deliberate" panic in that log is a fixture: it belongs to
`plugin::scaffold::tests::plugin_test_propagates_a_failing_suite`, which scaffolds a
deliberately failing plugin suite to prove the harness propagates failures. The outer test
passed.)

### Mutation proof — every new test was shown able to fail

Five mutations, each reverting a specific part of the fix; each reddened exactly the intended
tests and nothing else. Tree restored clean after each.

| Mutation | Result |
|---|---|
| compaction restamps `at` with `now` (old behaviour) | FAILED 1: `compaction_preserves_the_abandonment_timestamp_and_reason` |
| `abandoned()` returns empty (the defect, reinstated) | FAILED 4, incl. `an_abandonment_is_nameable_after_a_restart` and the automation test |
| Settled and Abandoned share one budget (shipped behaviour) | FAILED 2: `abandonments_dropped_past_the_cap...`, `compaction_preserves...` |
| Discord keyed path reverts to `next_nonce()` | FAILED 1: `discord_declares_idempotency_only_because_the_nonce_is_derived_from_the_key` |
| Matrix reverts to the resetting counter | FAILED 2: `matrix_declares_idempotency_only_...`, `send_message_succeeds_on_200` |

An assertion in `automation.rs` read *"it is recorded terminally and nameable, not silently
dropped"* while checking only `state()` by an id the test already held — an in-process lookup
cannot show that an operator who does *not* know the id can find the delivery. It now
exercises the read path and checks destination, reason and time.

---

## Instrument defect found and REPAIRED in-lane (§6b-ii)

My first Synapse harness reported `Registration has been disabled`, and I nearly recorded
that as an environment limitation. Cause: the generated `homeserver.yaml` has **no trailing
newline**, so `cat >>` glued `enable_registration: true` onto the final line `# vim:ft=yaml`
— the flag became part of a **comment**. It looked correct under `grep`.

Repaired rather than documented-and-moved-on, and the repair is verified **by parsing, not by
grepping**:

```
python3 -c "import yaml; print(yaml.safe_load(open(Y)).get('enable_registration'))"  ->  True
```

Three assertions, per §6b-ii: a correct config parses `True` (known-positive); a
commented-out key parses as `None` (known-negative); and **the naive matcher would have
missed it**, because `grep enable_registration` matches happily inside the comment. That
third assertion is the one that proves the repair does anything.

---

## Findings for the orchestrator

1. **HIGH — Matrix restart transaction-id collision causes silent message loss.** Measured
   against real Synapse; fixed in this lane; fix proven on the same homeserver. Detail above.
2. **MEDIUM / BACKLOG — a stalled outbound delivery blocks the gateway tick loop, so a drain
   request is not noticed until the stall resolves.** Measured in
   `live-attempt1-drain-budget-1000ms.log`: the drain was requested at 20:22:35 and processed
   at 20:23:25, after the 22.8s stall ended, by which time pending had fallen to 0. Not fixed
   — out of this lane's scope, non-blocking per the severity policy, and it makes a
   shutdown-drain slower to take effect than its budget implies.
3. **Not a defect, worth recording:** `resume()`'s local variable is named `settled` but only
   calls `begin_attempt`; carried deliveries stay pending. The behaviour is correct and
   documented; the name misleads.

## What I did NOT do

- Did not change the abandonment policy — explicitly out of scope.
- Did not touch `scripts/f24-inbound.mjs`, the inbound fixtures, or
  `wcore-eval-scenarios/src/journey.rs`.
- Did not touch the two fenced CLI files (verified against the merge-base SHA, not the branch
  name).
- Did not merge, open a PR, tag, or close anything.
- Did not live-prove the **`OutcomeUnknownNoDedup`** abandon path end-to-end. The live proof
  exercises `DrainBudgetExpired`. Reaching the other path live needs a destination that
  declares it cannot dedupe *and* whose API the existing sink can serve; the sink is
  Slack-shaped, and the telegram attempt failed at the transport
  (`live-attempt2-telegram-sink-mismatch.log`). That path is covered by the mutation-proven
  `automation.rs` test, which now asserts the full read-path surface, but it is **test
  evidence, not live evidence**, and I am naming the gap rather than implying otherwise.
