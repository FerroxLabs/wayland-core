# Phase 24 lane C — the independent delivery sink, and what it measured

The open half of Success Criterion 1. Lane 24-B proved delivery **continuity**
across a `kill -9` — twelve deliveries carried, counted and named — but counted
them by reading the gateway's own delivery ledger. That is the sender's record
of what it believes it did. Reading it out-of-process rules out a runtime
grading its own in-memory state; it does not rule out the runtime being wrong
about the world. **A gateway whose sends never leave the process writes exactly
the same ledger as one whose sends all land.**

This document holds the instrument, the measurement, the defect it found, the
fix, and the re-measurement.

---

## 1. The instrument

`crates/wcore-eval-scenarios/src/fixtures/channel.rs` plus the standalone
process `bin/wayland-channel-sink.rs`.

Three properties make it independent, and all three are load-bearing:

1. **It is a different process.** Started before the gateway, it outlives the
   gateway's `kill -9` and the platform's restart. The gateway's only way to
   add a line to the arrivals journal is to complete a real TCP round trip to a
   process it does not control and cannot restart.
2. **The sink assigns the message identity.** The `ts` handed back comes from
   the sink's own monotonic counter, so a receipt the sender holds is proof the
   sink saw the request — not proof the sender formatted one.
3. **The arrival is journalled BEFORE the response is written.** An arrival the
   sender never learns about is still recorded. That asymmetry is the whole
   point: it is the only way to tell "did not arrive" apart from "arrived, and
   the sender does not know it".

### No fixture adapter, no eleventh platform, no vendor credential

`wcore-channel-slack` already carries an `api_base_url` override in its on-disk
TOML schema (`#[serde(default)]`). So the **real production adapter, entirely
unmodified in its transport**, auto-registered by `wcore-channels-registry`
from `$WAYLAND_HOME/channels/*.toml`, is pointed at the sink. The code path
under test is the shipped one. The bearer is a fixture string; it is never
journalled — only a truncated SHA-256 fingerprint is.

### The stall mode, and why a clean sink cannot find the defect

AGENTS.md §11: *a live test proves nothing if its scenario is too clean to
reach the defect.* A sink that always answers immediately can only ever produce
deliveries in the ledger's `Settled` state, so a kill lands between deliveries
and every carried delivery is of the harmless `Accepted` class.

The interesting class — `Attempted`, outcome **UNKNOWN** — is reachable only if
the destination can accept a request and then never answer. `SinkMode::
StallAfter(n)` journals arrival `n+1` and holds the connection open forever.

---

## 2. The scenario, and the state it deliberately carries

`24-C-arrival-evidence/f24c-live.sh`, on `hetzner-dsm`, real `systemd --user`,
release build of `wayland-core 0.12.25`, throwaway home, profile `f24c`.

| State carried | Why it is there |
|---|---|
| 12 distinct deliveries, distinct bodies | the body is the discriminator the sink tallies over; identical bodies would make a duplicate invisible |
| a real registered service, not a foreground process | the restart has to be the platform's, not the harness's |
| a non-empty ledger at the moment of the kill | an empty ledger makes the carried-delivery path unreachable |
| a destination that accepts one delivery and never answers | the only way to reach the outcome-unknown class from outside |

Sequence: sink up → real slack adapter pointed at it → 12 jobs seeded through
the shipped binary → `gateway install` → `gateway start` → wait until the sink
has taken the stalled delivery → `kill -9` → let systemd restart it → tally at
the sink.

---

## 3. Run 1 — the defect, measured

`24-C-arrival-evidence/run1-before-fix/`

```
total   = 10
unique  =  9
dupes   = ['f24c-delivery-09']     ← the duplicate Criterion 1 forbids
stalled =  2
```

```
seq  9  f24c-delivery-09  answered=false  at 03:09:37   ← landed, then kill -9
seq 10  f24c-delivery-09  answered=false  at 03:09:44   ← landed AGAIN after restart
```

The gateway's own ledger, for contrast — same delivery id both times:

```
cron:b4049f60-…:1785121776528  accepted   03:09:37.350797
cron:b4049f60-…:1785121776528  attempted  03:09:37.350814   ← first send, stalled
cron:b4049f60-…:1785121776528  attempted  03:09:43.028807   ← resume() re-marks
cron:b4049f60-…:1785121776528  attempted  03:09:44.030822   ← the re-fire
```

`[gateway] started … carried=1 (unattempted 0 / unknown-outcome 1)`

**The ledger knew the key was identical and dispatched it anyway.** In
`LedgeredHandler::dispatch_fire`, only a `Settled` state short-circuited; an
`Attempted` (outcome-unknown) delivery fell through to `begin_attempt` and was
sent again. The ledger's own module documentation names the missing half — the
key lives in the ledger and not on the wire, so *"a destination which needs the
key transmitted must be handed it explicitly by its adapter"* — and nothing
handed it. So the destination could not suppress the replay, and nothing else
suppressed it either.

**This is Criterion 1's "without lost or duplicate delivery" clause failing
directly, and no ledger read could have shown it.** Lane 24-B's evidence —
`carried=12`, then drain-abandoned — looks correct from inside.

---

## 4. The fix

Both halves are needed. Transmitting the key without gating the retry still
duplicates at every destination that ignores it; gating the retry without
transmitting the key converts every duplicate into a loss.

1. **Hand the key to the destination.** `Channel::send_message_idempotent`
   carries the ledger's stable delivery id; `ChannelManager::send_to_keyed`
   passes it; `EngineJobHandler::dispatch_fire` supplies it from
   `FireContext::delivery_id()`; the Slack adapter transmits it as
   `Idempotency-Key` (inert against real Slack, which ignores unknown request
   headers).
2. **Gate the retry on a declared capability.** `Channel::
   supports_outbound_idempotency` and `JobHandler::dispatch_is_idempotent` both
   default to **false**. `LedgeredHandler` consults it: an outcome-unknown
   delivery to a destination that cannot recognise a replay is **abandoned —
   recorded, terminal and nameable** — rather than sent a second time.

The conservative default matters. Nine of the ten registered adapters inherit
`false`, so none of them can silently reintroduce the duplicate.

### The subtlest property, and the one the live fix rests on

A key is registered at the sink when the message is **journalled**, not when it
is answered. A stalled arrival is a message the destination holds and the sender
never heard about, so a replay of its key is still a duplicate of something
already there. Keying off "answered" would leave exactly the dangerous case
unprotected while every easy case still passed. Pinned by
`a_replay_of_a_stalled_deliverys_key_is_suppressed_not_duplicated`, and proved
red under mutation M7.

---

## 5. Run 2 — the same scenario, re-measured

`24-C-arrival-evidence/run2-after-fix/`

```
journal records    = 11
messages at dest   = 10
unique bodies      = 10
DUPLICATED         = []            ← was ['f24c-delivery-09']
suppressed replays =  1
```

```
seq  9  f24c-delivery-09  key=cron:55a9790c-…:1785124088790  suppressed=false  answered=false
seq 10  f24c-delivery-09  key=cron:55a9790c-…:1785124088790  suppressed=true   answered=true
```

Same body, **same key**, second arrival collapsed at the destination and the
sender handed back the identity the first attempt created. `[gateway] started …
carried=1 (unattempted 0 / unknown-outcome 1)` — the carry is unchanged; what
changed is that carrying it no longer produces a second message.

### What run 2 does NOT prove — stated rather than glossed

`f24c-delivery-11` and `f24c-delivery-12` have no arrival record. **They are not
losses.** The sink stalls permanently from arrival 9 onward, so after the
restart the tick loop blocked on delivery-10 and the observation window closed
with 11 and 12 never attempted. That is an artefact of the instrument, not a
product defect — and the honest claim is therefore scoped:

> Of the ten deliveries that were attempted, ten distinct messages exist at an
> independent destination and none arrived twice; specifically, the one delivery
> whose outcome was UNKNOWN across a `kill -9` and a platform-driven restart
> produced exactly one message.

A full 12-of-12 clean tally needs a sink that resumes answering after the kill.
That is a one-line change to the harness and it was not run. **Carried to
24-04 as F24-C-M1.**

---

## 6. Gates proved able to go red — seven mutations, by measurement

Each reddened exactly the intended test and nothing else; each reverted with
`git diff --stat` printing nothing.

| # | Mutation | Result |
|---|---|---|
| M1 | journal the arrival only on the answering path | 1 red — `a_stalling_sink_journals_the_arrival_it_never_answers` |
| M2 | fingerprint returns the raw bearer | 2 red — both secret assertions |
| M3 | tally dedups before counting | 1 red — `a_duplicate_at_the_sink_is_named_not_merely_counted` |
| M4 | sink returns a constant identity instead of its counter | 1 red — `the_sink_assigns_the_message_identity…` |
| M5 | remove the `destination_dedupes` guard (restores the defect) | 1 red — `an_unknown_outcome_delivery_is_not_re_sent…` |
| M6 | Slack keeps claiming the capability but stops sending the header | 1 red — `slack_declares_idempotency_only_because_it_sends_the_header` |
| M7 | register the key only when the arrival was ANSWERED | 1 red — `a_replay_of_a_stalled_deliverys_key_is_suppressed…` |

M6 exists because the capability declaration is a claim the wire has to back: a
`true` with no header would make every retry a duplicate again.

---

## 7. Verification

Host `hetzner-dsm`, worktree `/root/wayland-24c`. Every exit status captured
into a variable before any filtering; **no gate in this lane terminates in a
pipe.**

| Gate | Result |
|---|---|
| `cargo nextest run -p wcore-gateway -p wcore-channels -p wcore-channel-slack -p wcore-cron -p wcore-eval-scenarios -p wcore-channels-registry --no-fail-fast` | **539 run: 539 passed, 5 skipped** (rc=0) |
| `cargo clippy -p wcore-gateway -p wcore-channels -p wcore-channel-slack -p wcore-cron -p wcore-eval-scenarios -p wcore-agent -p wcore-cli --all-targets -- -D warnings` | clean (rc=0) |
| `cargo nextest run -p wcore-agent -E "test(cron)"` | 19 run: 19 passed (rc=0) |
| `cargo fmt --all -- --check` (Mac, the one permitted Cargo command there) | clean (rc=0) — see F24-C-L1 |

### Seam gate — inlined pathspec form, with a control

The variable form (`SEAM="a b c" … -- $SEAM`) is self-passing under zsh, which
does not word-split unquoted expansions. Inlined:

```
SEAM_TRACKED_RC=0  SEAM_STAGED_RC=0  SEAM_UNTRACKED=''
control: git diff --quiet 24bc2821 -- crates/wcore-gateway/src/automation.rs → rc=1
```

The control is the point: the same gate form returns 1 against a file this lane
did change, so the three zeros above mean "unmodified" rather than "the
pathspec matched nothing".

`Cargo.toml` (workspace root), `Cargo.lock`, `crates/wcore-config/src/config.rs`
and `crates/wcore-protocol` are **untouched**. `crates/wcore-cli/src/lib.rs` and
`crates/wcore-cli/src/main.rs` — the two §6 fenced files — are **untouched**;
this lane made no shared-fence edit at all.

Only `crates/wcore-eval-scenarios/Cargo.toml` changed, to register the sink
binary. It is a crate-local manifest, adds no dependency, and therefore leaves
`Cargo.lock` untouched.

**No `wcore-contract generate` was run and no contract fixture was regenerated.**
Nothing in this lane touches protocol events, commands, the config schema or the
desktop manifest, so no contract change is required.

---

## 8. Findings

| ID | Severity | Status |
|---|---|---|
| **F24-C-H1 — an outcome-unknown delivery is re-sent to a destination that cannot recognise the replay, producing a duplicate at the destination across `kill -9` + platform restart** | **HIGH** | **FIXED**, measured before and after against an independent sink, mutation-proved (M5, M6, M7) |
| F24-C-M1 — run 2's tally covers 10 of 12 deliveries; the permanently-stalling sink blocks the tick loop so 11 and 12 are never attempted | MEDIUM | BACKLOG — instrument artefact, not a product loss; needs a resume-after-kill sink mode |
| F24-C-M2 — `gateway status` freezes while a delivery is in flight, so `deliveries_pending` read 0 with nine deliveries already at the destination | MEDIUM | BACKLOG — same family as F24-B-H3, which fixed only the drain case |
| F24-C-L1 — `cargo fmt --all -- --check` is **red at base commit `24bc2821`** on `crates/wcore-agent/examples/p22_goal_live.rs`, a file no lane in this wave owns | LOW | FIXED with pure rustfmt output; flagged for attribution because it is not this lane's file |

No CRITICAL. The one HIGH is fixed with executable evidence.

---

## 9. What this closes, and what it does not

**Closes.** The delivery-ARRIVAL half of Criterion 1, on Linux, for the scope
stated in §5 — and it closes it by finding a real defect rather than by
confirming an expectation. 24-02's CONTINUATION gate now has the
independent-sink instrument it named as missing.

**Does not close.** 24-02's CONTINUATION gate as literally written (it names
`/tmp/f24-02-run/continuation-sink-{linux,macos}.ids`; this lane's artefacts are
`24-C-arrival-evidence/run*/arrivals.jsonl`, and no macOS run exists).
24-02's SURFACE gate (PTY capture) was not attempted. 24-03 Tasks 1 and 2 and
all of 24-04 were not started. See `24-C-SUMMARY.md` §"What was NOT delivered".
