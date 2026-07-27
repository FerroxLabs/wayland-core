# 24-02 — The Automation Contract

The recorded ownership model, the trigger matrix with each type's bound, the
retry and history bounds, and the evidence behind each.

Pinned to lane branch `lane/24` in `/Users/seandonahoe/dev/waylandcore-ferrox`.
Every number below was read from tool output on `hetzner-dsm:/root/wayland-24`;
none is recalled.

---

## 1. Ownership

### 1.1 The model

| Role | May | May not |
|---|---|---|
| **Owner** | Fire the schedule; advance `last_fired`; write history; write fire results | — |
| **Observer** | Read jobs, report status, show history | Fire anything. `tick_once_at` returns before it touches the store's run list |

Exactly one process is the owner of a **schedule directory**
(`$WAYLAND_HOME/cron`, i.e. the directory `jobs.json` already lives in). Every
other process attaching to that directory is an observer.

### 1.2 Who may hold it

| Holder | How | Released by |
|---|---|---|
| The gateway | `AutomationPlane::start` takes it on the `Started` transition | `drain_and_release`, which surrenders it **before** closing admission |
| A session-boot runner | `bootstrap.rs` attempts it, and degrades to observing on contention | The `CronRunner`'s `shutdown`/`Drop`, which releases before stopping the tick |
| `cron daemon` | Same attempt path | Process exit |

A session that boots with **no** gateway running takes the lease itself and
behaves exactly as it did before this plan. That path is asserted, not assumed:
`a_slash_target_still_stages_honestly_when_no_dispatcher_is_live`.

### 1.3 How a dead holder is proved dead

**By the operating system releasing the lock, and by nothing else.**

The claim is an `flock` (Unix) / `LockFileEx` (Windows) exclusive lock on a
one-byte sentinel `schedule.lock`. The OS releases it when the holding
descriptor closes — on `SIGKILL`, on a panic, and after a power loss followed
by a reboot. So *"the lock is acquirable"* **is** the proof of death.

Three things are explicitly **not** proof, each with a test that would go red
if they were used:

| Not proof | Why | Test |
|---|---|---|
| A stale-looking timestamp | A healthy holder stopped for longer than the heuristic would lose its schedule to a second firer — the exact double-fire the lease prevents | `a_live_holders_schedule_is_not_reclaimable_however_old_the_record_looks` (backdates the record a year; the live holder keeps it) |
| The recorded pid | Pid recycling: an unrelated process inheriting the identifier cannot hold the lock, so it cannot masquerade | `a_stale_record_alone_never_grants_ownership` |
| A `fcntl` record lock | Owned by the **process**, so a second open inside one process merges rather than conflicts, and the single-owner test could never go red | `a_second_attempt_in_one_process_is_refused` |

The readable owner record `schedule.owner` is **never locked**, so an observer
can name the owner while the owner holds the claim. On Windows the lock is
mandatory, which is why it sits on a separate one-byte sentinel nothing reads
(`the_owner_record_is_readable_while_the_lock_is_held`,
`the_sentinel_stays_one_byte`).

### 1.4 A fire in flight when the lease is lost

Ownership is re-checked **immediately before every dispatch**, not only once at
the top of the tick. When it has been lost:

- the dispatch does **not** happen;
- `CronFireOutcome::Abandoned { reason }` is written to history and to
  `last_result`;
- **`last_fired` is not advanced**, because the job did not run and the
  incoming owner still owes it.

Test: `a_lease_lost_mid_tick_abandons_the_selected_fire_and_records_it`.

### 1.5 The staged-fire hole

`CronFireOutcome::Staged` records a fire that was staged but had no live
dispatcher. For a slash target that happened on **every** fire outside a
session.

It is **closed by wiring, not by renaming**: `AutomationPlane` takes an
injected dispatcher, and the CLI supplies a live one. The honest outcome is
preserved for the honest case — with nothing live, `Staged` still occurs and
is still reported.

| Case | Outcome | Test |
|---|---|---|
| No live dispatcher | `Staged` (unchanged) | `a_slash_target_still_stages_honestly_when_no_dispatcher_is_live` |
| Gateway holds the lease and supplies a dispatcher | `Success` | `a_slash_target_stops_staging_once_a_live_dispatcher_is_wired` |

### 1.6 The delivery spine

Every **delivery-bearing** fire goes through the 24-01 exactly-once ledger via
`LedgeredHandler` in `wcore-gateway::automation`. There is no second path.

- A `Channel` target is a delivery — it leaves the machine.
- `Slash` and `Skill` are local work with no external destination; ledgering
  them would inflate the pending count with work that cannot be duplicated at a
  sink, and drain would never converge. Asserted: `only_a_channel_target_is_a_delivery`.

**The idempotency key is derived from the SCHEDULED instant**, not the attempt:
`cron:{job_id}:{scheduled_for_epoch_ms}`. Two runs of the same daily job carry
byte-identical targets, so `&Target` alone cannot produce a stable key; the
scheduled instant does, and it does not move across a restart. That is what
makes the retry after a hard kill the *same* delivery rather than a second one.

---

## 2. The trigger matrix

A target says **what**; a trigger says **when**. They are two independent
fields, not one enum.

| Kind | Parameters | Default bound | Terminal? | Clock-driven? |
|---|---|---|---|---|
| `once` | `at` | min interval 1s, 1 in flight, **no deadline** | Structurally — the anchor passes `at` and `next_after` returns `None` forever | yes |
| `interval` | `every_secs` | min interval `max(every_secs, 60)`, 1 in flight | no | yes |
| `cron` | `expression` | min interval 60s, 1 in flight | only if the expression has no future occurrence | yes |
| `event` | `topic` | min interval 1s, **2 in flight** | no | **no** |
| `webhook` | `path`, `require_auth` (default **true**) | min interval **5s**, 1 in flight | no | **no** |
| `poll` | `url`, `every_secs` | min interval `max(every_secs, 300)`, 1 in flight | no | yes |
| `commitment` | `deadline`, `heartbeat_secs` | min interval `max(heartbeat_secs, 1)`, 1 in flight, **deadline** | yes, at the deadline | yes |

Hard ceilings applied to every variant regardless: `FLOOR_INTERVAL_SECS = 1`,
`CEILING_IN_FLIGHT = 16`.

### 2.1 Bounds narrow only

`TriggerBound::clamp_to` is **one-way**. A persisted job may ask to be bounded
more tightly and gets it; a job asking to be bounded more loosely — by a
hand-edited `jobs.json`, a Desktop write, or a typo — is narrowed back, and the
**earlier** of the two deadlines wins so a job cannot extend its own.

Tests: `a_bound_cannot_be_widened_by_a_stored_value`,
`a_bound_can_be_narrowed_by_a_stored_value`, `the_earlier_deadline_wins`,
`a_hand_edited_bound_cannot_make_a_job_fire_faster` (drives the real tick with
a hostile stored bound and asserts zero fires inside the window).

### 2.2 Two defects the matrix found

Both were reds produced by the matrix and both were real:

1. **A commitment past its deadline fired forever.** Spentness was evaluated
   against the *anchor* — "was this trigger already spent when it last ran" —
   which is never true of a live job. The check is now against **now**. Commit
   `fix(24-02): evaluate a terminal deadline against now, not the anchor`.
2. **A one-shot could never fire at all.** With now-relative spentness, giving
   `once` a deadline equal to its fire instant made it terminal at exactly the
   moment it became due. Its terminal property is structural instead. Commit
   `fix(24-02): a one-shot must not carry a deadline equal to its fire instant`.

Neither was found by review. Both were found by a matrix case that could go red.

### 2.3 Externally driven triggers predict nothing

`event` and `webhook` return `None` from `next_after`. The tick must not invent
a fire for them, and `cron add` prints *"driven externally — not predictable
from the clock"* rather than an empty line, because silence there reads as
"it will never fire". Test: `externally_driven_triggers_never_fire_from_the_clock_alone`.

### 2.4 Webhook authentication

`require_auth` defaults to **true**, and the default survives a persisted record
that omits the field entirely (`a_webhook_defaults_to_requiring_authentication`
deserializes `{"kind":"webhook","path":"/h"}` and asserts it reads authenticated).
An open endpoint has to be typed out — `--trigger webhook:/p:open` — and is
rendered in the operator-facing descriptor as `(OPEN)` in capitals.

---

## 3. Retry

| Knob | Default | Ceiling |
|---|---|---|
| `max_attempts` | 3 (first try plus two retries) | 10 |
| `base_backoff_secs` | 60 | ≥ 1 |
| `max_backoff_secs` | 3600 | 86400 |

Backoff doubles from the base and stops at the ceiling. `RetryPolicy::clamped`
is one-way for the same reason bounds are.

**The give-up is a named state, not an absence.**
`CronFireOutcome::GaveUp { attempts, message }` is written to history and to
`last_result`, and `cron list` shows `GAVE_UP(after N attempts)` on the job's
own line. A job that gave up an hour ago and a job that is between attempts are
otherwise indistinguishable from outside.

Evidence:

- `a_failing_target_gives_up_inside_its_cap_and_the_give_up_is_in_history` —
  three attempts, terminal `GaveUp` in history, then **100 further ticks add
  nothing**.
- `the_backoff_actually_holds_a_failing_job_off_between_ticks` — nineteen ticks
  inside a ten-minute window produce one record, **and the boundary tick
  produces the second**. The second half matters: without it the assertion
  would pass equally against a job that had simply stopped forever, which is a
  different bug wearing the same green.

### 3.1 What retry deliberately does NOT cover

A process that died mid-attempt. That outcome is **unknown**, not failed, and
belongs to the ledger's `Attempted` state. Conflating the two would either
retry deliveries that already landed or abandon ones that did not.

---

## 4. History

`DEFAULT_MAX_RECORDS = 1000`, enforced on the **write** path.

Before this the file was append-only with no cap at all: "ring-buffered"
appeared in the module documentation and the code appended forever. A cap
applied only on read would leave the file growing and merely hide it.

Trimming keeps the **tail**, so the verb still returns recent records after the
file stops growing. A torn line from a crash mid-write is skipped **and
counted**, never silently dropped.

Evidence — `sustained_firing_through_the_runner_stops_growing_the_history_file`
drives 1250 real ticks through `tick_once_at` and asserts the file sits at
exactly 1000. It also asserts the count is non-zero first, because a
zero-record history would make the bound assertion vacuous.

---

## 5. Natural-language authoring

A phrase produces a **candidate**, printed with the next three computed fire
times, and **writes nothing without `--confirm`**. An uninterpretable phrase is
quoted back verbatim and nothing is written — with or without `--confirm`.

The vocabulary is small and deterministic on purpose. A fuzzy match is how a
sentence becomes a schedule the operator did not intend (threat T-24-02-01).

| Phrase | Resolves to |
|---|---|
| `every 15 minutes` | `interval 900s` |
| `every 2 hours` | `interval 7200s` |
| `every minute` / `every hour` | `interval 60s` / `interval 3600s` |
| `every day at 9am` / `daily at 17:30` | `cron 0 9 * * *` / `cron 30 17 * * *` |
| `every weekday at 9am` | `cron 0 9 * * 1-5` |
| `every weekend at …` | `cron … * * 0,6` |
| `every monday at 8:15am` … `every sunday at …` | `cron … * * 1` … `* * 0` |

Refused (asserted, not assumed): `whenever`, `every 15 fortnights`,
`every day at 25:00`, `every day at 9:99`, `every blursday at 9am`, and the
empty phrase.

Clock forms accepted: `9am`, `9:30am`, `12pm` → 12, `12am` → 0, `17:00`,
`09:05`.

---

## 6. Verification — commands run and what they returned

Host `hetzner-dsm`, worktree `/root/wayland-24`, `PATH=/root/.local/bin:/root/.cargo/bin:$PATH`.

```
cargo test -p wcore-cron
  wcore-cron unit          66 passed, 0 failed
  tests/history_bounds      3 passed, 0 failed
  tests/single_owner        8 passed, 0 failed
  tests/trigger_matrix     13 passed, 0 failed
cargo test -p wcore-gateway
  wcore-gateway unit       (green) + lifecycle 9 + pidlock 8 + ledger 7
cargo clippy -p wcore-cron -p wcore-gateway --all-targets -- -D warnings
  Finished — clean
cargo fmt --all -- --check   (macOS, the one permitted Cargo command there)
  clean
```

### 6.1 Gates proved capable of going red

Not asserted — **measured**, by mutating the implementation and reading the
failure.

| Mutation | Result |
|---|---|
| Delete the observer early-return in `tick_once_at` | `an_observer_leaves_the_store_untouched` FAILED (`an observer must not write a fire result`); `two_runners_against_one_store_fire_a_due_job_exactly_once` FAILED (2 history records, expected 1) |
| Delete the pre-dispatch ownership re-check | `a_lease_lost_mid_tick_abandons_the_selected_fire_and_records_it` FAILED (`left: 2, right: 1`) |

**One gate was found self-passing and was fixed.** In its first form,
`two_runners_against_one_store_fire_a_due_job_exactly_once` ticked the *owner*
first. That fire advanced `last_fired`, the job stopped being due, and the
observer then fired nothing **whether or not the ownership check existed at
all** — the mutation left it green. Ticking the observer first, against a job
that is still due, is what makes it capable of going red. Commit
`test(24-02): make the two-runner gate capable of going red`.

That is the fourth self-passing shape from the standing list, found by
measurement rather than by review: *the gate was already green at base*.

---

## 6.2 LIVE — the shipped binary, Linux

`hetzner-dsm`, `/root/wayland-24/target/release/wayland-core` (`wayland-core 0.12.25`),
`WAYLAND_HOME=/root/f24-02-live`. Verbatim.

### Every trigger type added through the shipped binary

```
$ wayland-core cron add --trigger once:2027-01-01T09:00:00Z --slash /brief
next[0]: 2027-01-01T09:00:00+00:00
added 2e1a5155-212f-41e8-9967-44f49e75d268
$ wayland-core cron add --trigger every:900 --slash /brief
next[0]: 2026-07-27T01:49:54.730470955+00:00
next[1]: 2026-07-27T02:04:54.730470955+00:00
next[2]: 2026-07-27T02:19:54.730470955+00:00
added b257cbdd-93ca-42ee-9d9b-7fa94a48088b
$ wayland-core cron add --trigger "cron:0 9 * * *" --slash /brief
next[0]: 2026-07-27T09:00:00+00:00 … added 16313186-8dd4-4d3d-862b-59761f226bcd
$ wayland-core cron add --trigger event:build.finished --slash /brief
next:    driven externally (event) — not predictable from the clock
added d0ef23b4-ebd9-4dec-b3f4-c7c8125c0aa9
$ wayland-core cron add --trigger webhook:/hooks/build --slash /brief
next:    driven externally (webhook) — not predictable from the clock
added c7905336-b743-4c93-9af7-0f20cc7b2d65
$ wayland-core cron add --trigger poll:https://status.test/health:300 --slash /brief
next[0]: 2026-07-27T01:39:54.925575998+00:00 … added f5e4bf24-72ef-4e45-9e1d-18fbf4c0a0ce
$ wayland-core cron add --trigger commit:2027-01-01T17:00:00Z:900 --slash /brief
next[0]: 2026-07-27T01:49:54.973596615+00:00 … added 8b2190de-2586-4b31-9452-3d08184bb7db
```

```
$ wayland-core cron list
on  2e1a5155-…  [once      ] @once 2027-01-01T09:00:00+00:00   slash /brief  last_fired=never
on  b257cbdd-…  [interval  ] @every 900s                       slash /brief  last_fired=never
on  16313186-…  [cron      ] 0 9 * * *                         slash /brief  last_fired=never
on  d0ef23b4-…  [event     ] @event build.finished             slash /brief  last_fired=never
on  c7905336-…  [webhook   ] @webhook /hooks/build (auth)      slash /brief  last_fired=never
on  f5e4bf24-…  [poll      ] @poll https://status.test/health every 300s  slash /brief  last_fired=never
on  8b2190de-…  [commitment] @commit by 2027-01-01T17:00:00+00:00 heartbeat 900s  slash /brief  last_fired=never
```

Seven types, one verb, every one listable. The two externally driven types
print that they are not predictable rather than printing nothing.

### Natural-language authoring writes nothing unreviewed

```
$ wayland-core cron add --describe "every weekday at 9am" --slash /standup
phrase:  "every weekday at 9am"
becomes: 0 9 * * 1-5
next[0]: 2026-07-27T09:00:00+00:00
next[1]: 2026-07-28T09:00:00+00:00
next[2]: 2026-07-29T09:00:00+00:00

nothing written. re-run with --confirm to persist this schedule.
jobs before=7 after=7  (must be equal)

$ wayland-core cron add --describe "whenever the vibes are right" --confirm --slash /x
wayland-core cron: could not interpret "whenever the vibes are right" as a schedule; nothing was written
EXIT=1
jobs now=7

$ wayland-core cron add --describe "every weekday at 9am" --confirm --slash /standup
… added c8c701ae-3857-4ee5-aa86-386570b9c69f
jobs now=8
```

The phrase is quoted back **verbatim** on refusal, and `--confirm` does not
rescue an uninterpretable phrase.

### Single ownership, two real processes, one schedule

```
$ setsid wayland-core cron daemon &   # daemon 1
[cron-daemon] role=owner — this process fires the schedule
$ cat $WAYLAND_HOME/cron/schedule.owner
{ "pid": 3476747, "acquired_at": "2026-07-27T01:40:09.587086662+00:00", "holder": "cron-daemon" }

$ setsid wayland-core cron daemon &   # daemon 2, same schedule
[cron-daemon] role=observer — pid 3476747 already owns this schedule; firing nothing
$ cat $WAYLAND_HOME/cron/schedule.owner    # unchanged — still daemon 1
{ "pid": 3476747, … }

$ kill -9 3476747                     # the owner dies with no chance to clean up
$ setsid wayland-core cron daemon &   # daemon 3
[cron-daemon] role=owner — this process fires the schedule
$ cat $WAYLAND_HOME/cron/schedule.owner
{ "pid": 3483613, "acquired_at": "2026-07-27T01:40:18.600837548+00:00", "holder": "cron-daemon" }

# after every daemon is stopped:
$ cat $WAYLAND_HOME/cron/schedule.owner
(removed — no owner)
```

**Nine seconds** between the `SIGKILL` and the successor's claim, with no
timeout anywhere: the OS released the lock when the killed process's
descriptor closed, and that release is the whole reclamation mechanism.

### The fire count under two daemons

```
$ wayland-core cron add --trigger every:60 --slash /brief   # job 99354343-…
$ setsid wayland-core cron daemon &   # role=owner
$ setsid wayland-core cron daemon &   # role=observer
# … 150 seconds …
$ wayland-core cron history 99354343-9ac5-4a3e-89f9-4552b9aaedbc -n 20
2026-07-27T01:42:42Z  staged (no live dispatcher)
2026-07-27T01:41:42Z  staged (no live dispatcher)
$ wc -l < $WAYLAND_HOME/cron/history.jsonl
2
```

Two fires in 150 seconds on a 60-second trigger, with two daemons attached.

**What this does and does not prove, stated honestly.** The count is
*consistent* with single ownership but is not by itself discriminating: the
store's advance-on-fire bookkeeping would also produce two in the happy case,
which is exactly why that bookkeeping looked adequate before. The
discriminating evidence is elsewhere and is named rather than implied — the
`role=` lines emitted by the shipped binary, and the mutation measurements in
[§6.1](#61-gates-proved-capable-of-going-red) where deleting the ownership
check reddens the unit gates. This transcript confirms the wiring reaches the
product; it is not offered as the proof of exclusion.

### Live finding, LOW

When two daemons start within the same instant, the loser can read the owner
record before the winner has written it and reports
`role=observer — pid unknown already owns this schedule`. The exclusion is
correct — the loser observes — but the diagnostic is less useful than it
should be. Filed as **F24-02-L2**. Cosmetic; the ownership decision never
depends on the record.

---

## 7. Deviations, recorded

| # | Deviation | Reason |
|---|---|---|
| D1 | **`Cargo.toml`/`Cargo.lock` edited**, which plan 24-02 forbids as a shared seam | The plan's own `key_links` require `wcore-gateway/src/automation.rs` → `wcore-cron/src/lease.rs`. That edge needs a dependency, and a dependency needs a lockfile entry. The stated rationale for the fence — 24-03 running concurrently — does not hold: this lane executes 24-02, 24-03 and 24-04 strictly serially. The delta is **three lines** in the `wcore-gateway` block (`async-trait`, `tokio`, `wcore-cron`). The coordinator's base fix `9a86b287` was cherry-picked first so the shared bytes are byte-identical and only those three lines are lane-local. |
| D2 | Task 1 also edited `crates/wcore-cron/src/job.rs` and `crates/wcore-agent/src/tool_backends/cron.rs`, neither in Task 1's own `<files>` | `job.rs` **is** in the plan's `files_modified`; the task-level list omitted it. `tool_backends/cron.rs` has an exhaustive `match` on `CronFireOutcome` and stopped compiling when the enum grew. Rule 3 (blocking). Both new arms are rendered distinctly rather than folded into "error" — an abandoned fire did not fail, and a give-up will not try again. |
| D3 | The `flock`/`LockFileEx` primitive is **duplicated** between `wcore-gateway::pidlock` and `wcore-cron::lease`, against AGENTS.md "No Duplicate Code Across Crates" | The dependency edge runs gateway → cron, so cron cannot reuse the gateway's copy without a cycle; `wcore-agent` also needs the lease and has no gateway dependency; extracting it lower, or adding `libc`/`windows-sys` to `wcore-cron`, is a further lockfile edit. Declared with local FFI, the precedent `store.rs` already sets for `getuid`. **Filed as F24-02-L1** for unification. |
| D4 | The lease uses a `min`/`max` pair where clippy wanted `clamp` | `clamp` **panics** when max < min, and the upper operand is a persisted, hand-editable value. A bounds-enforcement function that panics on hostile input is a denial of service inside the code that exists to prevent one. |

---

## 8. Open — stated plainly

1. **No live operator journey yet.** Section 6 is a test receipt. Criterion 2
   is not closed by it. See `24-02-SUMMARY.md` §"What was NOT delivered".
2. **`event`, `webhook` and `poll` have a persisted, bounded, inspectable
   trigger but no producer wired to fire them.** They validate, store, list,
   status and round-trip, and the clock correctly refuses to fire the first
   two. Nothing publishes an event, routes an inbound request to a job, or
   performs the poll. Recorded as a named gap, not claimed.
3. **The in-flight bound is stored and clamped but not enforced at dispatch.**
   `max_in_flight` is carried, narrowed and shown; the tick is single-threaded
   per job so it cannot currently exceed 1, which means the field is correct
   today and unproven under any future concurrency.
4. **The heartbeat is advanced by a successful fire only.** There is no
   independent beat channel, so a commitment whose work succeeds beats and one
   whose work fails reads as missed. That is defensible but it is not the same
   thing as an out-of-band liveness signal.
