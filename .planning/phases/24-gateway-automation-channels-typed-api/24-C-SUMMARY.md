---
phase: 24-gateway-automation-channels-typed-api
plan: "C"
subsystem: delivery
tags: [independent-sink, delivery-arrival, outbound-idempotency, exactly-once, criterion-1]
status: partial
plans-not-executed:
  - "24-03 — NOT STARTED (Tasks 1 and 2); Task 3's fixture endpoint was pulled forward and built"
  - "24-04 — NOT STARTED"
requires:
  - "24-01"
  - "24-02"
  - "24-B"
provides:
  - wcore_eval_scenarios::fixtures::channel::ChannelSink (independent hermetic delivery destination)
  - "wayland-channel-sink (separate-process sink binary that outlives a gateway kill)"
  - wcore_channels::Channel::send_message_idempotent + supports_outbound_idempotency
  - wcore_channels::ChannelManager::send_to_keyed
  - wcore_cron::JobHandler::dispatch_is_idempotent
affects:
  - crates/wcore-gateway/src/automation.rs (outcome-unknown deliveries are no longer blindly re-sent)
  - crates/wcore-agent/src/cron.rs (dispatch_fire supplies the ledger key to the adapter)
  - crates/wcore-channel-slack/src/{lib,api}.rs (transmits Idempotency-Key)
tech-stack:
  added: []
  patterns:
    - "the destination records the arrival, never the sender"
    - "journal the arrival BEFORE answering, so an unacknowledged arrival is still a fact"
    - "capability defaults to false — a retry is permitted only where a replay can be recognised"
key-files:
  created:
    - crates/wcore-eval-scenarios/src/fixtures/channel.rs
    - crates/wcore-eval-scenarios/bin/wayland-channel-sink.rs
    - .planning/phases/24-gateway-automation-channels-typed-api/24-C-ARRIVAL-CONTRACT.md
    - .planning/phases/24-gateway-automation-channels-typed-api/24-C-arrival-evidence/
  modified:
    - crates/wcore-channels/src/{lib,manager}.rs
    - crates/wcore-channel-slack/src/{lib,api}.rs
    - crates/wcore-cron/src/runner.rs
    - crates/wcore-gateway/src/automation.rs
    - crates/wcore-agent/src/cron.rs
decisions:
  - "The independent sink is a SEPARATE PROCESS; an in-process fixture dies with the harness and could never observe a systemd-supervised gateway"
  - "The real unmodified slack adapter is pointed at the sink via its existing api_base_url override — no fixture adapter, no eleventh platform, no vendor credential"
  - "An outcome-unknown delivery to a destination that cannot recognise a replay is ABANDONED (recorded, terminal, nameable), not re-sent"
  - "supports_outbound_idempotency defaults to FALSE so nine of ten adapters cannot silently reintroduce the duplicate"
metrics:
  tests-green: 539
  completed: 2026-07-27
---

# Phase 24 Lane C: Independent Delivery Sink Summary

**The delivery-ARRIVAL half of Criterion 1 was open because every count in this
phase had been read from the gateway's own ledger. Building an independent sink
found a real HIGH defect within one run: a delivery landed at the destination
TWICE across a `kill -9` and a platform restart.** It is fixed, re-measured, and
mutation-proved. **24-03 Tasks 1 and 2 and all of 24-04 were not started.**

## Termination state

**Partial.** One unclaimed deliverable — the instrument the phase could not
close Criterion 1 without — executed to completion, live-proven, and it found
and fixed a HIGH. Two claimed plans not begun. Graded honestly below rather than
averaged.

## 1. Why this was worth the whole budget

Lane 24-B closed delivery *continuity* and said so precisely: its twelve-delivery
evidence was read out-of-process from the gateway's own ledger, which is not an
independent sink, so *arrival* was unproven. That is not a bookkeeping quibble.
**A gateway whose sends never leave the process writes exactly the same ledger as
one whose sends all land.** `delivered` was not a fact this workspace could
observe.

The instrument, its three independence properties, the scenario design, both
measured runs, the seven mutations and the full gate table are in
**`24-C-ARRIVAL-CONTRACT.md`**. The headline:

| | run 1 (before) | run 2 (after) |
|---|---|---|
| messages at the independent destination | 10 | 10 |
| unique bodies | 9 | 10 |
| **DUPLICATED** | **`['f24c-delivery-09']`** | **`[]`** |
| suppressed replays | — | 1 |

## 2. F24-C-H1 — the defect, and why only a sink could see it

`f24c-delivery-09` reached the destination, the gateway was `kill -9`'d before it
could settle, systemd restarted it, and the destination recorded **the same body
again**. The gateway's ledger held the *identical* delivery id on both attempts:

```
cron:b4049f60-…:1785121776528  attempted  03:09:37.350814   ← first send, stalled
cron:b4049f60-…:1785121776528  attempted  03:09:44.030822   ← the re-fire
```

In `LedgeredHandler::dispatch_fire` only a `Settled` state short-circuited, so an
outcome-UNKNOWN delivery fell through and was sent again. The ledger's own module
doc had already named the missing half — the key lives in the ledger, not on the
wire, so *"a destination which needs the key transmitted must be handed it
explicitly by its adapter"*. Nothing handed it.

From inside, this looks fine: `carried=1 (unknown-outcome 1)` is exactly what the
design intends. **The duplicate is only visible at the destination.** This is the
shape the program keeps finding — a system attesting to its own delivery.

**Fix, both halves** (either alone is wrong — transmitting without gating still
duplicates at every destination that ignores the key; gating without transmitting
turns every duplicate into a loss): the ledger's stable key is handed to the
adapter and transmitted as `Idempotency-Key`, and the retry is gated on a
capability that **defaults to false**, so an unknown-outcome delivery to a
destination that cannot recognise a replay is abandoned — recorded, terminal and
nameable — rather than sent twice.

## 3. Gates proved able to go red — seven mutations

M1 journal only on the answer path · M2 fingerprint leaks the bearer · M3 tally
dedups before counting · M4 sink returns a constant identity · M5 remove the
`destination_dedupes` guard (restores the defect) · M6 Slack keeps claiming the
capability but stops sending the header · M7 register the key only when answered.

Each reddened exactly the intended test and nothing else; each reverted with
`git diff --stat` printing nothing. M7 is the one that matters most: a key must
be registered when the message is **journalled**, not when it is **answered**,
or the stalled-then-killed case — the only case that matters — stays unprotected
while every easy case passes.

## 4. Verification

| Gate | Result |
|---|---|
| `cargo nextest run` (6 crates, `--no-fail-fast`) | **539 run: 539 passed, 5 skipped** (rc=0) |
| `cargo clippy` (7 crates, `--all-targets -- -D warnings`) | clean (rc=0) |
| `cargo nextest run -p wcore-agent -E "test(cron)"` | 19 passed (rc=0) |
| `cargo fmt --all -- --check` | clean (rc=0) |
| SEAM (inlined pathspec, **with a control proving it returns 1 on a real edit**) | clean |

Every exit status captured into a variable before any filtering. **No gate in
this lane terminates in a pipe.** The seam gate uses the inlined-pathspec form;
the `$SEAM` variable form is self-passing under zsh.

Clippy initially failed on two raw `reqwest::Client::new()` calls in my own
tests. Fixed by routing through `wcore_egress::EgressClient`, the sanctioned
chokepoint — **not** by adding an `#[allow]`.

## 5. What was NOT delivered — stated plainly

1. **24-03 Tasks 1 and 2 were not started.** No probe, binding/routing, media
   normalisation, edit/delete/reaction, health, or channel CLI verbs; no roles,
   command idempotency, event cursor, negotiation, or support bundle. `wcore-acp`
   is **completely untouched**. Task 3's fixture endpoint was pulled forward and
   built because Criterion 1 could not be closed without it — that reordering is
   the deviation in §6.
2. **24-04 was not started.** No journey driver, no receipt schema, no platform
   receipt. Its terminal publication was therefore never reached, and nothing was
   pushed to main, merged, tagged, released, or used to close an issue.
3. **No macOS and no Windows evidence.** CI now fires on `lane/**` and
   `lane/24c` is pushed, so the artefacts are obtainable — I did not get to
   downloading and exercising them. This is a budget outcome, not an
   impossibility; `.planning/intel/MACOS-BINARY-IS-OBTAINABLE.md` is the method.
4. **24-02's CONTINUATION gate is not passed as literally written.** It names
   `/tmp/f24-02-run/continuation-sink-{linux,macos}.ids`; this lane's artefacts
   are `24-C-arrival-evidence/run*/arrivals.jsonl` and there is no macOS run.
   The *instrument* that gate was missing now exists and the Linux measurement is
   done; the gate's own file contract is unmet.
5. **24-02's SURFACE gate (PTY capture) was not attempted at all.**
6. **Run 2's tally covers 10 of 12 deliveries.** The permanently-stalling sink
   blocks the tick loop, so 11 and 12 were never attempted. They are **not**
   losses — an instrument artefact, filed F24-C-M1 — and the claim in
   `24-C-ARRIVAL-CONTRACT.md` §5 is scoped accordingly rather than rounded up.

## 6. Deviations

**[Reordering] 24-03 Task 3's fixture endpoint was built before Tasks 1 and 2.**
The plan orders the framework contract first. But the fixture is the only
artefact that closes the phase's oldest open item, 24-B named it as the exact
missing piece, and the dispatch brief assigned it to this lane by name. Building
it first meant the HIGH was found on day one instead of after two tasks of
framework work. Tasks 1 and 2 were then not reached — reported, not absorbed.

**[Rule 1 — bug] Files outside 24-03's `files_modified` were edited.**
`crates/wcore-cron/src/runner.rs`, `crates/wcore-gateway/src/automation.rs` and
`crates/wcore-agent/src/cron.rs`. All three are required by the F24-C-H1 fix: the
capability has to reach the delivery spine from the adapter, and it crosses those
crates to get there. Each edit is additive and carries its own test.

**[Deviation] The reference adapter is Slack, not the declared Discord/Email
pair.** 24-03 names Discord (persistent connection) and Email (polling). Slack is
the adapter that already carries an `api_base_url` override in its TOML schema,
so it is the one that can be pointed at a hermetic endpoint **without modifying
its transport**. Using the declared pair would have meant editing two production
adapters to make them testable, which is a worse trade. The declared pair remains
unexercised.

**[F24-C-L1] `cargo fmt --all -- --check` was RED at base commit `24bc2821`**, on
`crates/wcore-agent/examples/p22_goal_live.rs` — a file this lane does not own.
Verified by restoring the base version and re-running (rc=1). Fixed with pure
rustfmt output, because leaving it red would have made this lane's own fmt gate
unreadable. **Now moot:** the owning lane fixed it upstream, and after merging
`7260d43f` that file is absent from this lane's delta entirely.

**Post-merge re-verification.** The base moved 29 commits during this lane. The
merge was clean (no conflicts) and every gate was **re-run on the merged tree**
rather than trusted from the pre-merge run: 539 tests pass, clippy clean, fmt
clean, and the seam and §6-fence checks both return 0 against the new upstream
`7260d43f` with a control returning 1 on a file this lane did change.

**Shared-file fence: not touched at all.** `crates/wcore-cli/src/lib.rs` and
`crates/wcore-cli/src/main.rs` are byte-identical to base. The only manifest
edited is `crates/wcore-eval-scenarios/Cargo.toml` (crate-local, registers the
sink binary, adds no dependency, so `Cargo.lock` is untouched).

**No `wcore-contract generate`, no contract fixture regenerated, no publication.**
Nothing here touches protocol events, commands, the config schema or the desktop
manifest, so no contract change is required and no seam request is needed on that
axis.

## 7. Findings ledger

| ID | Severity | Status |
|---|---|---|
| **F24-C-H1** outcome-unknown delivery re-sent → duplicate at the destination | **HIGH** | **FIXED**, measured before/after at an independent sink, mutation-proved |
| F24-C-M1 run 2 tallies 10 of 12; stalling sink blocks the tick loop | MEDIUM | BACKLOG — instrument artefact, needs a resume-after-kill sink mode |
| F24-C-M2 `gateway status` freezes mid-delivery; `deliveries_pending` read 0 with 9 already delivered | MEDIUM | BACKLOG — same family as F24-B-H3, which fixed only the drain case |
| F24-C-L1 `cargo fmt --check` red at base `24bc2821` on another lane's file | LOW | **MOOT** — the owning lane fixed it upstream; after merging `7260d43f` the file is absent from this lane's delta |

No CRITICAL. The one HIGH is fixed with executable evidence.

## 8. Verdict — the delivery-arrival half of Criterion 1

**CLOSED ON LINUX, WITHIN A SCOPE STATED EXACTLY, AND IT FAILED FIRST.**

Of the ten deliveries attempted, ten distinct messages exist at an independent
destination and none arrived twice; specifically, the one delivery whose outcome
was UNKNOWN across a `kill -9` and a platform-driven restart produced **exactly
one** message. Before the fix, that same delivery produced two.

**Still open on this criterion:** the same measurement on macOS and Windows; a
full 12-of-12 clean tally (F24-C-M1); and the nine adapters that inherit
`supports_outbound_idempotency() == false`, for which an outcome-unknown delivery
is now correctly *abandoned* rather than duplicated — which is safe and honest,
and is not the same thing as delivered.

## Self-Check

Every test count, exit status, tally and journal line above was copied from
captured tool output, not recalled. The arrivals journals and delivery ledgers
for both runs are committed under `24-C-arrival-evidence/`. Files asserted
present were verified on disk; commit subjects were read from `git log`. The
gates that do **not** pass — 24-02's continuation and surface gates, macOS,
Windows — are named as not passing, and the two unstarted plans are named as
unstarted rather than sampled.

**Self-Check: PASSED**
