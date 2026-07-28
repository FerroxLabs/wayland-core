# 24-C1 NOTES — abandon surface + adapter dedup tokens

Lane `lane/24-abandon-surface`. Base `1b2577e1b61447f1599e127679b8e2eb3552b61b`.
Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24-abandon-surface`.

Append after EVERY measurement. Never batch to the end (§6b-i).

---

## M1 — the brief's file path is wrong; the defect is real

Brief says `crates/wcore-channels/src/ledger.rs:214`. **There is no `ledger.rs` in
`wcore-channels`.** The real file is `crates/wcore-gateway/src/ledger.rs`, and the cited
line numbers land correctly there. Recording the correction rather than silently
retargeting.

Verified in-tree at base:

- `pending()` — `ledger.rs:214-220`, filter is `Accepted | Attempted`. `Abandoned` excluded. **CONFIRMED.**
- `pending_count()` — `:223-228`, same filter. `Abandoned` excluded. **CONFIRMED.**
- `compact()` — `:253`, `Abandoned` grouped with `Settled` as `terminal`, droppable past
  `retain_settled`. **CONFIRMED.**

## M2 — there are TWO abandon call sites, not one, and they differ

`grep -rn "\.abandon("` across `crates` returns exactly three hits (one is the definition):

| Site | What abandons | Operator surface today |
|---|---|---|
| `wcore-gateway/src/drain.rs:176` | forced drain past budget | `DrainReport.abandoned: Vec<String>` → printed by `wcore-cli/src/gateway.rs:1033-1036` |
| `wcore-gateway/src/automation.rs:179` | F24-C-H1 unknown-outcome, destination cannot dedupe | **nothing but `tracing::warn!` (`:182`)** |

So the brief's "no consumer anywhere outside `ledger.rs`" is **too strong for the drain
path and exactly right for the automation path.** Refining the claim rather than
inheriting it:

- The drain path has a surface, but it is **ephemeral** — it exists only in the stdout of
  the `gateway drain` invocation that caused it. An operator who was not watching that
  terminal cannot recover it afterwards.
- The automation path (`automation.rs:179`) has **no surface at all**. This is the one the
  brief's HIGH is really about, and it is the path that fires unattended, from a scheduled
  cron delivery after a crash — i.e. precisely when nobody is watching a terminal.

## M3 — the claim in the source

`automation.rs:172-175` states the delivery is *"ABANDONED rather than dropped: recorded,
terminal, and nameable by an operator."* `recorded` and `terminal` are implemented.
**`nameable by an operator` is not** — no query path reaches it. `automation.rs:551` asserts
in a test `"it is recorded terminally and nameable, not silently dropped"`, which tests
`ledger.state(id) == Abandoned` — an in-process ledger read, not an operator surface. That
assertion's message overclaims what it checks.

## M4 — two further losses in the record itself (found while reading, not in the brief)

1. **No reason is stored.** `Record` (`ledger.rs:89-97`) has `id`, `state`, `at`,
   `delivered`. There is no field for WHY. The two abandon sites have materially different
   causes (drain-budget expiry vs. unknown-outcome-no-dedupe) and the journal cannot tell
   them apart. The brief asks the surface to answer "why" — the data to answer it is not
   currently persisted.
2. **Compaction destroys `at`.** `compact()` (`:258, :265-270`) rewrites every retained
   record with `at: now` and `delivered: None`. So even the "when" an operator would get is
   the compaction time, not the abandonment time. This silently corrupts the timestamp of
   surviving records — worth grading on its own.

Neither is a reason to widen scope beyond the brief; both are directly load-bearing for
"the message, the destination, when, and why".

## M5 — destination is not in the ledger either

`Record` has no destination. The ledger keys on the caller-supplied delivery id only
(`ledger.rs:19-26` is explicit that the key is the delivery id and nothing rides the wire).
So "the destination" the brief asks for must either be derivable from the delivery id or
persisted at accept time. To establish before designing the surface.

---

## M6 — the delivery id is ALREADY the stable cross-restart token

`wcore-cron/src/runner.rs:324-338`:

```
cron:{job_id}:{scheduled_for.timestamp_millis()}[:{occurrence}]
```

Derived entirely from the job identity and the scheduled instant. **It does not move
across a restart** — the same scheduled occurrence produces the same string on any process.
That is exactly the property Task 2 asks for, and it is already computed and already handed
to the adapter as `send_message_idempotent(msg, key)` (`wcore-channels/src/lib.rs:123-129`).

So Task 2 is not "invent a token". It is "stop discarding the one already being passed in".

Neither Matrix nor Discord implements `send_message_idempotent`, so both fall through to the
trait's pass-through default (`:128`) and the key is dropped on the floor. Their local
generators then manufacture a *worse* token:

- **Matrix** — `rest.rs:13` `AtomicU64::new(1)`, consumed at `:44` and interpolated into the
  PUT path at `:47`. Process-local, resets to 1 on every start. Module doc at `:4` claims
  "make retries idempotent"; true within one process, false across the restart that is the
  entire reason the ledger has an `Attempted` state.
- **Discord** — `rest.rs:58-68` `next_nonce()` = `{wall_clock_ms:x}-{counter:x}`. The doc
  comment at `:55` says the millis prefix "keeps it distinct across restarts". That is
  **stated as a feature and is precisely the defect**: a token deliberately made distinct
  across restarts cannot deduplicate the cross-restart replay the nonce field exists for.
  Discord's own `next_nonce_is_unique_and_within_cap` (`:451`) asserts `a != b`, locking in
  the distinctness.

`supports_outbound_idempotency` — only Slack overrides it to `true`
(`wcore-channel-slack/src/lib.rs:234`). Matrix and Discord inherit `false`
(`wcore-channels/src/lib.rs:139-141`), which is **currently honest** for both.

Slack is the reference for how to bind the claim to the wire:
`slack_declares_idempotency_only_because_it_sends_the_header` (`:240`) drives a mockito
mock that `match_header("idempotency-key", "cron:job-a:1785121776528")`, so deleting the
header reddens the test. Task 2 must produce the equivalent for both adapters.

## M7 — destination must be persisted at ACCEPT, not at abandon

`Target::Channel { channel_name, text }` (`wcore-cron/src/job.rs:24`) carries the
destination, and `automation.rs` has the `target` in scope at its abandon site. **`drain.rs`
does not** — it iterates `ledger.pending()` and sees ids only (`drain.rs:172-177`). So a
destination recorded only at abandon time would be present for one path and absent for the
other. It has to be captured at `accept()`.

## Design decisions taken (D4/D5 out to the panel, §4)

- **D1 — the surface is `wayland-core gateway abandoned [--json]`.** Reuses `GatewayCmd`,
  `ScopeArgs` and `DeliveryLedger::open`; adds no subsystem. `gateway.rs` is NOT one of the
  two fenced files (`wcore-cli/src/lib.rs`, `main.rs`), so this is in-lane.
- **D2 — persist `destination` at `accept()` and `reason` at `abandon()`** (see M7).
- **D3 — store the last whole `Record` per id, not the last `DeliveryState`.** This is what
  lets compaction stop rewriting `at` (M4.2).
- **D4 — compaction retains every `Abandoned` record; `retain_settled` bounds `Settled`
  only.** Dropping an abandonment silently erases the surface this lane exists to build.
  OUT TO PANEL.
- **D5 — do NOT persist the message body**, only delivery id + destination. The body is
  recoverable from the cron job; copying bodies into a second durable store is a
  data-retention harm, not a feature. OUT TO PANEL.

Backward compatibility: every new `Record` field must be `#[serde(default)]`. `open()`
QUARANTINES anything it cannot parse (`ledger.rs:141`), so a non-defaulted field would make
an upgrade read every pre-existing record as a torn tail — converting an upgrade into a
mass phantom loss.

## Still to establish

- [ ] Task 3: whether a restarted Matrix counter makes a homeserver DROP a genuinely new
      message. Plan: real Synapse in Docker on hetzner (`docker 29.2.1` present, 657G free
      on `/root`), same access token, txn_id reuse. Not a mock — the whole question is what
      a real homeserver does.
- [ ] Live proof of the operator surface against a REAL abandonment, not a synthetic row.
