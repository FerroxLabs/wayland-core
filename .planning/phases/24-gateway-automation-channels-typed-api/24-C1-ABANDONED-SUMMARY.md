# 24-C1-abandoned — disposing of an abandoned delivery: ack, re-send, and a re-graded finding

Lane `lane/24-c1-abandoned`. Merge-base `f8b8ec25372fb4ed4280a5aa365873ae8465abfc`
(asserted against `git ls-remote gh plan/f20-unified-audit-repair` before any work).
Build host: hetzner `hz/24-c1-abandoned`. Evidence: `evidence/24-C1-abandoned/`.

**Verdict: the HIGH is closed. Two of the three sub-items were already done by a lane
the brief did not know about; I built the two that were missing and drove all of them
live. The Matrix leg is independently re-measured and confirmed. The Discord leg is
BLOCKED on a credential only Sean can supply, and is reported unmeasured rather than
estimated.**

---

## 1. Correction to the brief, up front: three of the four §3 claims have moved

The work order restates four claims from `24-C1-IDEMPOTENCY-SUMMARY.md` §3 and asks me to
verify them before building. **They were measured against a tree that no longer exists.** A
later lane, `lane/24-abandon-surface`, landed `c74dd4bd feat(24-C1): make an abandoned
delivery nameable by an operator` and wrote it up in `24-C1-ABANDON-SURFACE.md`. That work is
in my base. Re-verified against `crates/wcore-gateway/src/ledger.rs` at `f8b8ec25`:

| §3 claim | At base | Evidence |
|---|---|---|
| `pending()` filters to `Accepted\|Attempted`, excluding `Abandoned` | **STILL TRUE — and correct** | `ledger.rs:392`. Now documented as deliberate: re-adding it would re-dispatch the delivery the product chose not to send. It is no longer the *only* read path, which is what made it a defect. |
| `pending_count()` same filter, doc *"the number drain publishes"* | **STILL TRUE, unchanged** | `ledger.rs:400` |
| `compact()` classes `Abandoned` as terminal history under `retain_settled`, so it is compactable out | **NO LONGER TRUE** | Separate budgets since `c74dd4bd`: `ABANDON_RETENTION = 10_000`, settled traffic cannot evict an abandonment, overflow counted into `dropped_abandonments()` and warned. |
| `DeliveryState::Abandoned` has no consumer outside `ledger.rs` | **NO LONGER TRUE** | `wcore-cli/src/gateway.rs:642` (`abandoned()`), `gateway/support.rs:214` (`abandoned_count()`). Instrument known-positive on the same query: 36 `Abandoned` hits outside `ledger.rs`, i.e. the grep was alive. |

So the finding was real and is now **partly closed**. What remained open, and what this lane
built, is the other two thirds of the brief's own prescription: *"exempt them from compaction
**until acknowledged**"* — there was no acknowledge concept at all — *"and provide a re-send
path"* — there was none.

Concept search for the missing half, unproxied `/usr/bin/grep`, over `wcore-gateway/src` and
`wcore-cli/src/gateway*`, multiple vocabularies:
`resend|re_send|re-send|requeue|re_queue|acknowledge|acknowledged|ack` → **4 hits, every one
prose inside a doc comment, zero implementation** (`ledger.rs:105`, `automation.rs:176`,
`automation.rs:379`, `gateway.rs:129`). Known-positive for the same instrument in the same
tree: `grep -c bandoned crates/wcore-cli/src/gateway.rs` = **14**. An absence measured with a
proven-live instrument, per §3b-i.

---

## 2. What was built

### `gateway ack <id>` — the signature that retires an abandonment

`acknowledge()` on the ledger, plus the verb. Idempotent, and the **first** acknowledgement's
timestamp wins: re-running the verb must not rewrite when the incident was actually reviewed.
It never moves the abandonment's own `at`.

### `gateway resend <id> [--confirm-not-delivered] [--ack]` — the repair path

The ledger deliberately holds no message bodies (a 3-0 cross-audit in the prior lane), so the
message is reconstructed from **the cron job the delivery id names**, matched by prefix rather
than by splitting on `:` — a job id may itself contain a colon — with the longest match
winning so one job id cannot shadow another. Real adapters are registered from the home's own
`channels/` directory through the same `auto_register_from_dir` path `gateway run` uses.

**The original delivery key rides the send.** On a destination that honours it, this makes the
re-send free of risk: if the first copy did land, the destination suppresses this one. That is
visible on the wire in the live proof.

**The refusal.** `was_attempted` is now captured at abandon time — the fact that decides
whether a re-send can duplicate. It has to be captured *there*, because the abandon record
replaces the previous one in `states` and nothing afterwards remembers it; and the reason
alone cannot answer it, because the drain path abandons everything still `pending()`, which is
a mix of never-attempted and outcome-unknown work. `None` (a record predating the field) is
read as **"may have landed"**, never as safe.

### Compaction: four budgets, and a design decision that reverses a prior panel

Unacknowledged abandonments are now **exempt from compaction entirely**, like unsettled work.
Acknowledged ones stay bounded at `ABANDON_RETENTION` with overflow counted and warned.

Cross-audit panel (§4) on this exact question — **codex 5.6 Sol = keep the cap, gemini 3.1
Pro = keep the cap, kimi K3 = exempt the unacknowledged**. I took the **minority**, and say so:

- Kimi is the only vote that engages with the premise change. The earlier panel's reasoning
  ("retain everything is not a bound, an unbounded ledger is its own outage") was explicitly
  conditioned on an abandonment being *permanently* terminal — nothing could ever retire one,
  so unbounded retention had no exit. `acknowledge` **is** that exit. Codex and gemini both
  restated the earlier conclusion without addressing that its premise no longer holds.
- The failure modes are not symmetric, and **both majority voters conceded exactly this as
  their own strongest objection**: codex — *"evicted records can no longer be individually
  inspected, acknowledged, or re-sent … permanently degraded recoverability"*; gemini —
  *"guarantees silent data loss of individual transaction records … impossible to perform
  granular reconciliation"*. Unbounded growth is visible, recoverable, and one JSONL line per
  record. A dropped unacknowledged abandonment permanently destroys the only evidence that a
  specific message was never delivered — the precise thing this surface exists to prevent.
- The majority's real worry is neglect, and it is answered by making neglect **loud** rather
  than by truncating: `unacknowledged_abandoned_count()` is surfaced on the verb and in
  `--json`.

Second panel question, **auto-acknowledge on a successful re-send: codex = yes, gemini = yes,
kimi = no**. Here I took kimi's own proposed synthesis rather than either pole: a re-send does
**not** acknowledge, and `--ack` composes the two explicitly. The majority's concern is toil,
which the flag removes; kimi's concern is that a bulk re-send script would drain the surface
to zero with nobody having reviewed anything, which is fatal for a surface whose entire defect
was being untrustworthy. `a_resend_is_recorded_without_erasing_the_abandonment` pins it.

A re-send is recorded **alongside** the abandonment, never in place of it. The state stays
`Abandoned`: the product did give up, and a later human repair does not unmake that.

---

## 3. The operator surface DRIVEN, not merely implemented

Shipped release binary at `848595e9`, a real systemd-supervised gateway, a real `kill -9` with
a delivery in flight, a real platform restart, the shipped binary's own drain deciding to give
up, and an independent `wayland-channel-sink` process. Full transcript:
`evidence/24-C1-abandoned/live-ack-resend-proof.log`.

```
ARRIVALS_AT_INDEPENDENT_SINK=4          <- positive delivery COUNTED first
surface BEFORE                          -> "No abandoned deliveries recorded"
kill -9  -> STATE_COUNTS_AFTER_KILL = {'settled': 3, 'attempted': 1}
restart  -> carried=1 (unattempted 0 / unknown-outcome 1)
drain    -> drain Forced: observations=2 abandoned=1 flushed=true
            ABANDONED delivery cron:b88ad8c8-...:1785343545205
```

The surface then answers, with the fields this lane added:

```
1 abandoned delivery in /tmp/24c1ab-run/home:

  cron:b88ad8c8-be4a-4694-94be-b38985f07df4:1785343545205
    to:     24c1absink
    when:   2026-07-29T16:45:53.383723441+00:00
    why:    shutdown drain ran out of budget before this delivery finished
    resend: CHECK THE DESTINATION FIRST — an attempt was in flight and may have landed
    acked:  no — retained until `gateway ack cron:b88ad8c8-...`

1 abandonment(s) are UNACKNOWLEDGED and are exempt from compaction until they are.
```

**The re-send, driven against the independent sink** — `ARRIVALS2` went `0 → 1`, in a journal
owned by a separate process, with the gateway stopped first so nothing else could write it:

```
ARRIVALS2_BEFORE_RESEND=0
Re-sent cron:b88ad8c8-...:1785343545205 to 24c1absink.
  replay-safe:  yes — the destination honours the delivery key
ARRIVALS2_AFTER_RESEND=1
{"seq":1,"endpoint":"chat.postMessage","text":"24c1ab-delivery-4",
 "idempotency_key":"cron:b88ad8c8-be4a-4694-94be-b38985f07df4:1785343545205",
 "suppressed":false}
```

The original delivery key is **on the wire**, read back from the sink's own journal rather
than from the sender.

Then `resent:` appears on the surface, the record stays listed and unacknowledged, `ack`
records it, and a second `ack` reports *"was already acknowledged at …; left unchanged."*

### Two failed attempts retained as measurements, not deleted

- `live-attempt1-drain-window-missed.log` — the drain arrived at `pending 0` and reported
  `Clean: abandoned=0`. Cause: the carried-work window is **~1 second** (`resume()` leaves the
  delivery `Attempted` but does not re-dispatch; the first cron tick does, and the send then
  settles), and polling `is-active` every 2s plus a `journalctl` call burned it. **The harness
  aborted rather than proceeding to a vacuous green** — the guard doing its job is itself
  evidence. Repaired by firing the drain in a tight spin that runs while the gateway is still
  down; the run loop reads the drain request *before* it ticks, so a request landing in that
  window is processed against `pending=1`.
- `live-attempt2-channel-not-started.log` — **a real defect in the new verb**, found only by
  driving it: `resend` registered the adapters but never called `start_all()`, so every
  re-send failed with `channel not started`. Registration is not connection. Invisible to
  every unit test, because they drive adapters directly. Fixed in `848595e9`.

---

## 4. Instrument defect found and REPAIRED in-lane (§6b-ii)

My own harness read `$?` after a pipe to `tail`, so a genuinely failing command reported
`ACK_BOGUS_RC=0`. That is LANE-BRIEF §3.2's **first named self-passing class, reproduced by
the instrument hunting it** — and it is the twelfth recorded instance on this programme.

Repaired rather than written up. Three assertions per §6b-ii, all in the live log:

```
ACK_BOGUS_RC=1        (known-negative genuinely fails)
RESEND_BOGUS_RC=1     (known-negative genuinely fails)
OLD_PIPED_SHAPE_RC=0  (the OLD shape, same failing command, WRONG status)
```

That third line is the one that proves the repair does anything.

---

## 5. Matrix — INDEPENDENTLY RE-MEASURED. The silent drop is REAL.

The brief asks me to measure the txn-id-reuse loss path and not to take the restatement on
trust. `lane/24-abandon-surface` had already measured it and fixed it. **I re-measured it
myself**, on a fresh `matrixdotorg/synapse:latest` container I stood up under lane-unique
paths and a lane-unique port, with a fresh access token and a fresh room:
`evidence/24-C1-abandoned/synapse-independent-remeasure.log`.

```
--- process life 1 (counter seeded at 1) ---
PUT txn=1 body=MSG-A -> {"event_id":"$SpTj0fVk8JFBUtkHLoMZfQMcV5gY9Ld0faU0mICBSHs"}
PUT txn=2 body=MSG-B -> {"event_id":"$tQnfkxD7RxhCDoOI9ou9OtFH3Wei6zD3ya05ztEAUNA"}
--- process life 2: SAME access token, counter RESET to 1 ---
PUT txn=1 body=MSG-C -> {"event_id":"$SpTj0fVk8JFBUtkHLoMZfQMcV5gY9Ld0faU0mICBSHs"}
VERDICT_A=REPLAY_SUPPRESSED
BODIES_IN_ROOM = ['MSG-B-before-restart', 'MSG-A-before-restart']
MSG_C_LOST = True
```

**Answer to the brief's question: MEASURED REAL.** The homeserver returned HTTP 200 and
MSG-A's event id for a PUT carrying MSG-C's body, and MSG-C never entered the room. Ground
truth is read from `/rooms/{id}/messages` — the room's own contents — not from the response,
because the response is exactly what lies. Different event ids on both sides confirm the
instrument is alive; a broken harness returning nothing could not have produced
`E1 == E3` and `E2 != E1`.

The fix (already in my base, from the prior lane) is re-proven on the same homeserver in the
same run:

```
PUT txn=cron:job-a:1785121776528                    -> $Cnj8s-i3...
PUT txn=cron:job-a:1785121776528 (REPLAY, same id)  -> $Cnj8s-i3...   DEDUP_WORKS=yes
PUT txn=cron:job-a:1785121776529 (DIFFERENT id)     -> $Jr4CATvY...   DISTINCT_WORKS=yes
MSG_D_count = 1        MSG_E_present = True
```

So the brief's item 2 needed no code from me: `rest.rs` already derives the transaction id
from the delivery key, and unkeyed sends are reseeded from the wall clock rather than from 1.
I verified that at base rather than assuming it, and re-proved the behaviour end to end.

---

## 6. Discord — the dedup window is UNMEASURED, and BLOCKED. I did not estimate it.

The nonce fix is in my base and verified present: `nonce_for_key()` derives from the delivery
key and hashes rather than truncates to Discord's 25-char cap (truncating would collapse two
occurrences of one job, since the key's distinguishing tail is the timestamp).
`the_keyed_nonce_is_stable_across_processes_and_the_unkeyed_one_is_not` pins both halves.

**The window itself I could not measure, and I am reporting it unmeasured rather than
estimated.** It is a server-side property of Discord's infrastructure; there is no local
substitute, and Discord does not document it. Measuring it needs a real bot token and a real
guild channel, which is a credential and therefore Sean-reserved (§0). Verified absent rather
than assumed: `/root/.wayland/` has no `channels/` directory, `channel_directory.json` carries
no platform entries, and the only credential the host injects is `ANTHROPIC_API_KEY` (env var
names listed with values stripped; no value was printed, logged or committed).

**The experiment that would close it**, for whoever has the credential: send with nonce `N`,
wait `T`, send the identical nonce `N` with a different body, and read the channel's own
message list — not the API response — to see whether one message or two are present. Bisect
`T`. The residual risk this bounds is *magnitude, not correctness*: a replay arriving after
the window expires duplicates rather than being suppressed. **BLOCKER for Sean.**

---

## 7. Gates — real numbers, and which run each came from

Compiled and run on `hetzner-dsm` at `848595e9`; only `cargo fmt --all -- --check` on the Mac.
Counts read back from an **unproxied** cargo over ssh, with `0 ignored` / `0 filtered out`
intact (the Mac-side `rtk` proxy strips exactly those two fields).

| Gate | Result |
|---|---|
| `cargo test -p wcore-gateway` (lib + all integration targets) | **49 + 7 + 9 + 8 + 4 passed; 0 failed; 0 ignored; 0 filtered out** (one target 1 ignored, pre-existing) |
| `cargo test -p wcore-cli --lib` | **1875 passed; 0 failed; 1 ignored; 0 filtered out** |
| `cargo test -p wcore-channel-matrix -p wcore-channel-discord` | **58 + 36 passed; 0 failed; 0 ignored; 0 filtered out** |
| `cargo clippy -p wcore-gateway --all-targets` | 0 warnings, 0 errors |
| `cargo clippy -p wcore-cli --all-targets` | 0 warnings, 0 errors |
| `cargo check --workspace --all-targets` | Finished, 0 errors — workspace-wide, never `-p` |
| `cargo fmt --all -- --check` (Mac) | **rc=0, 0 lines of output** (read without a pipe) |
| Fenced files vs **merge-base SHA** | **empty diff** — `wcore-cli/src/lib.rs` and `main.rs` untouched |
| Files changed vs merge-base | exactly 2: `wcore-cli/src/gateway.rs`, `wcore-gateway/src/ledger.rs` |

The `always_fails` panic visible in the `wcore-cli` log is a fixture, not a failure: it belongs
to `plugin::scaffold::tests::plugin_test_propagates_a_failing_suite`, which scaffolds a
deliberately failing plugin suite to prove the harness propagates failures. The outer test
passed.

### Mutation proof — every new gate shown able to fail

Six mutations, each reverting one specific part of the fix, each reddening **exactly** the
intended test and nothing else. Tree restored with `git checkout -- <named path>` after each
(never a blanket reset — other lanes share the object store); final `git status --porcelain`
on both paths was empty. Raw: `evidence/24-C1-abandoned/mutation-proof.log`.

| Mutation | Result |
|---|---|
| M1 compaction bounds ALL abandonments again (pre-lane behaviour) | FAILED 1: `an_unacknowledged_abandonment_is_never_compacted_away` — 48 passed / 1 failed |
| M2 a re-send silently acknowledges (the rejected auto-ack design) | FAILED 1: `a_resend_is_recorded_without_erasing_the_abandonment` |
| M3 `acknowledge` overwrites the first review time | FAILED 1: `acknowledge_preserves_the_abandon_time_and_the_first_review_time` |
| M4 `abandon` stops recording whether an attempt had started | FAILED 1: `abandon_records_whether_an_attempt_had_started` |
| M5 ack/resend accept a delivery that was never abandoned | FAILED 1: `acknowledge_and_resend_refuse_a_delivery_that_is_not_abandoned` |
| M6 the resend guard treats UNKNOWN as safe (duplicate-authorising) | FAILED 1: `resend_demands_confirmation_unless_the_delivery_provably_never_left` — 1874 passed / 1 failed |

M1 is also the *"the old shape would have missed it"* assertion for the retention change: the
pre-existing cap test still passes under M1, so only the new test distinguishes the two
designs. M6 is the same for the guard: `Some(true)` and `Some(false)` both behave identically
under the mutation, and only the `None` assertion fails.

One existing test changed expectation deliberately and is named here rather than buried:
`abandonments_dropped_past_the_cap_are_counted_and_reported` →
`acknowledged_abandonments_dropped_past_the_cap_are_counted_and_reported`. It now acknowledges
each record before compacting, because the cap no longer applies to unreviewed ones. Its cap
coverage is preserved, not weakened, and M1 proves the new test is what carries the change.

---

## 8. Findings for the orchestrator

1. **HIGH — CLOSED.** An abandoned delivery is now listable, acknowledgeable, re-sendable, and
   exempt from compaction until reviewed. Driven live end to end against an independent sink.
2. **HIGH — Matrix restart transaction-id collision causes silent message loss.** Not mine to
   fix (already fixed in base by `lane/24-abandon-surface`); **independently reproduced and
   confirmed by this lane** against a fresh real Synapse, and the fix re-proven in the same
   run. Recording the confirmation because a single-lane measurement of a HIGH is thinner
   evidence than two.
3. **BLOCKER for Sean — Discord dedup window unmeasured.** Needs a real bot token and guild
   channel. Experiment specified in §6. Bounds residual magnitude, not correctness.
4. **MEDIUM / BACKLOG, real but out of scope — the carried-work window is ~1 second.** After a
   restart, `resume()` leaves carried deliveries `Attempted`, and the first cron tick
   re-dispatches and settles them. Any operator or tool wanting to act on carried work
   (including `gateway drain`) has about one tick to do it. Measured twice here, in
   `live-attempt1` (missed) and `live-ack-resend-proof` (hit on spin 34). **Named, not fixed.**
5. **Cosmetic, not fixed:** `gateway resend` prints `receipt: 1.000000` against the fixture
   sink, because that sink returns its timestamp as the message id. A real adapter returns a
   real id. Not a product defect; noted so a reader of the log is not misled.

## 9. What I did NOT do

- **Did not measure Discord's dedup window.** Blocked on a Sean-reserved credential. Reported
  unmeasured; no number invented.
- **Did not live-prove the `OutcomeUnknownNoDedup` abandon path.** The live abandonment comes
  from `DrainBudgetExpired`. Reaching the other path live needs a destination that declares it
  *cannot* dedupe and whose API the existing sink can serve; the sink is Slack-shaped and
  Slack dedupes, so that arm short-circuits. This is the same gap the prior lane named, and I
  am naming it again rather than implying coverage. It is covered by the mutation-proven
  `automation.rs` tests — **test evidence, not live evidence.**
- **Did not change the abandonment policy.** `Abandoned` stays out of `pending()`; only its
  disposability changed.
- **Did not test the compaction exemption live** — the journal would need to exceed 10,000
  abandonments. Covered by `an_unacknowledged_abandonment_is_never_compacted_away`, which
  reopens the ledger from disk rather than trusting the in-memory map.
- Did not touch the two fenced CLI files (verified against the merge-base SHA, not the branch
  name), `scripts/f24-inbound.mjs`, the inbound path, or `wcore-eval-scenarios/src/journey.rs`.
- Did not merge, push to integration, open a PR, tag, or close anything. Did not run
  `wcore-contract generate`. No credential was printed, logged, committed or transmitted.
