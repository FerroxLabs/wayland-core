# 24-C2 repair — event / webhook / poll triggers

**Lane:** `lane/24-triggers` · **merge-base:** `d53fd54a` · **HEAD:** `236bcf0f`
**Full transcripts:** `24-C2-LIVE-EVIDENCE.md` (same directory)

The defect: `crates/wcore-cli/src/cron.rs:49-52` advertised `--trigger event:`,
`--trigger webhook:` and `--trigger poll:` in the shipped `--help`. All three
validated, persisted and appeared in `cron list` with **no error and no
warning**, and could not run. A customer registered automation, saw it accepted,
and it silently never fired.

---

## 1. What I measured before deciding anything

A probe driving the real `tick_once_at` loop over six ticks / six hours at base:

```
PROBE event fires=0      PROBE webhook fires=0      PROBE poll fires=6
```

**Two corrections to the record.**

**(a) The gap ledger is wrong about `poll`.** It lists all three as "validates,
persists, lists, NEVER FIRES". `poll` *did* fire — clock-driven, once per due
window — **having never contacted the URL**. `poll:https://x/health:300` was
`every:300` with an ignored URL string: it ran the job's action unconditionally,
whatever the remote would have said. That is a *stronger* lie than silence,
because the action has an effect. `CRITERIA-GAP-LEDGER.md:271` should be
amended; I have not edited it (it is another lane's artifact).

**(b) The suspected mechanism is refuted.** The brief pointed at
`wcore-agent/src/cron.rs`'s *"a missing surface logs the fire and returns Ok"*.
That comment is **stale** — those arms return `CronError::NoDispatcher` /
`Dispatch` and are guarded by `log_only_slash_and_skill_return_no_dispatcher` —
and it describes `Target` (what a job does), not `Trigger` (when it fires). No
trigger of any kind reached it. The real mechanism is one layer up, in
`wcore-cron::trigger::next_after` returning `None` for event/webhook, and in the
complete absence of any producer for all three.

---

## 2. Disposition per kind

Cross-audited **3/3 AGREE** — codex `gpt-5.6-sol`, gemini `3.1-pro-preview`,
kimi K3 — plus an internal adversarial pass. Each response was verified on-topic
and byte-counted; the panel prompt lived in a lane-unique directory with stdin
from `/dev/null`, and votes were extracted unanchored. Gemini's answer was
shortest and I checked its substance directly rather than trusting the marker;
it contributed the fan-out-starvation failure mode, kimi the crash-durability
and queue-cap ones. All three were folded in.

### `event` — **IMPLEMENTED. Dispatch works end to end.**

New `wcore_cron::events`: a durable, cross-process queue of published topics
that the tick drains, plus a `wayland-core cron publish <topic>` verb. Publisher
and consumer are different processes (a CI step publishes; the gateway owns the
schedule), so the bus is on disk beside `jobs.json`, under the same lease.

One file per event, `write-temp` + `rename`, **opaque UUID filenames** — the
topic lives inside the JSON and never in the path, so `event:../../escape`
cannot address the filesystem. Delivery is **at least once** (consume after
dispatch; the other order loses the event on a crash). **Fan-out**: one event
fires *every* subscriber, not the first. **Exact** topic match. Queue capped at
1024 with a publish past the cap **refused, not dropped**; drain capped at 64 per
tick so a backlog cannot fire as one burst. A job does not consume an event
published before it existed. Directory is `0700`.

Routed through the same `dispatch_and_record` the clock path uses — extracted,
not duplicated — so an event fire inherits the M-18 target scan, bounded retry,
the pre-dispatch lease re-check, the history record and the gateway's delivery
ledger. `FireContext` gains `occurrence` so two publishes of one topic in the
same millisecond are two deliveries rather than one the ledger drops as a
duplicate; clock-fire keys stay byte-identical to those in persisted ledgers.

### `webhook` — **FAILS LOUDLY. Does not close the criterion.**

Refused at `cron add` with a specific error, removed from `--help`. The surface
genuinely does not exist: `inbound_webhook.rs` routes only `/webhooks/:channel`
to chat connectors keyed on per-connector platform signature verification, and
there is **no credential scheme for a cron path**. `require_auth: true` is stored
with nothing to enforce it. Inventing an auth scheme for an unattended
remote-input endpoint is a trust-boundary design job with its own threat model,
not a repair; half-done webhook auth is worse than none because it *looks*
authenticated.

### `poll` — **FAILS LOUDLY. Does not close the criterion.**

Refused at `cron add`, removed from `--help`, **and** made externally driven so
already-persisted poll jobs stop firing unconditionally. Implementing a real poll
needs an HTTP client in a crate that deliberately has zero workspace
dependencies, routing through `wcore-egress` (a naive fetch of an
operator-supplied URL from an unattended daemon is an SSRF surface that would
bypass the egress policy), and a **response contract that is undefined anywhere
in the design docs** — any contract invented here becomes load-bearing API.

Removing behaviour was the contested call. All three panel members took it: the
only thing a user could depend on was the timer, `every:300` is one edit away,
and the removal is *loud* (add-time refusal, plus a marker on persisted jobs).

### Already-persisted jobs are not left silent

A `webhook`/`poll` job written by an earlier build, the Desktop app, or by hand
still loads and is still inspectable — an operator must be able to see the job
they can no longer create — and is now marked in both surfaces:

```
$ wayland-core cron list
on  f5e4bf24-…  [poll      ] @poll https://x/health every 300s  slash /brief  last_fired=never
      ^ WILL NEVER FIRE — poll triggers have no producer in this build: …

$ wayland-core cron status f5e4bf24-…
reachable:   NO — poll triggers have no producer in this build: …
```

---

## 3. The guards, and proof each can fail

Every guard asserts the **observable effect** — the set of targets the handler
actually received — never a return code. Mutations were applied on `hetzner-dsm`,
the suite run, the source restored, and the baseline re-verified green.

| # | mutation | result |
|---|---|---|
| 1 | delete the `drain_published_events` call — **the exact state the crate shipped in** | **5 of 11 FAILED** |
| 2 | restore `Poll` to the clock-driven arm — the measured 24-C2 defect | **1 FAILED** |
| 3 | `break` after the first matched subscriber (kills fan-out) | **1 FAILED** |
| 4 | delete the rate-bound check in the drain | **2 FAILED** |
| 5 | consume a rate-held event anyway (silent loss) | **2 FAILED** |
| 6 | delete `refuse_without_producer` from `cron add` — restores silent acceptance | **1 FAILED** (`wcore-cli`) |
| — | baseline restored | **11/0** and **9/0** |

Mutation 1 matters most: it reproduces the shipped defect exactly, and the
pre-existing suite was **fully green** in that state.

**A red my own guard caught, and the design change it forced.** The first run of
`a_burst_of_publishes_is_held_to_the_triggers_minimum_interval` failed 5≠1: the
job list is a snapshot, so five events each read the same stale `last_fired` and
all fired inside a 60s floor. Fixing that surfaced a worse issue — a rate-held
event would have been *consumed anyway*, i.e. a published event silently never
delivered, the same defect moved one layer down. Held events now **stay queued**;
backpressure lands on the publisher, where the queue cap gives a hard visible
refusal. Two further guards cover it, including one proving a held event *is*
eventually delivered so the first cannot pass against a runner that drops
everything.

**Two existing tests re-targeted, not weakened.**
`a_poll_is_floored_at_five_minutes_however_fast_it_asks` and
`a_hand_edited_bound_cannot_make_a_job_fire_faster` both drove their property
through `Trigger::Poll`. With poll no longer clock-fired they would read zero
whatever the flooring/clamping code did — **self-passing gates**. Both now run on
`interval`, which is genuinely clock-driven, and each gained a second half so it
cannot pass against a runner that fires nothing. Poll was added to
`externally_driven_triggers_never_fire_from_the_clock_alone`. Nothing was
`#[ignore]`d, `#[allow]`ed, deleted, re-gated or given a longer timeout.

---

## 4. Test and live evidence

| suite (isolated, `hetzner-dsm`) | result |
|---|---|
| `cargo test -p wcore-cron` | **108 passed, 0 failed** |
| `cargo test -p wcore-gateway` | **62 passed, 0 failed** (1 pre-existing `live_bundle_canary` ignored at base) |
| `cargo test -p wcore-cli` | **2280 passed, 0 failed** |
| `cargo clippy -p wcore-cron -p wcore-gateway -p wcore-cli --all-targets` | clean |
| `cargo fmt --all -- --check` (Mac) | clean |

**One contention artifact, reported not hidden.** A full `wcore-cli` run taken
while other lanes were building showed `import_is_idempotent_without_overwrite`
FAILED. Isolated re-run at the same commit: **7/0**. Same test at base
(`/root/wayland-24`, `cd00ff2f`): **6/0**. Unrelated to this change (hermes
profile migration), the known load-flakiness class. The 2280/0 above is the
isolated run.

**Live** (real binary, transcripts in `24-C2-LIVE-EVIDENCE.md`): `webhook` and
`poll` refused with `EXIT=1` and nothing written; `event` added; `cron publish`
queued the event on disk; the real `cron daemon` drained it and **a fire record
appeared where none could ever exist before**, with the queue emptied. Three
further runs against a `--skill` target returned three different *real* answers
from the engine-side sink — `success (5ms)`, `Skill 'touchproof' not found`, and
`requires user approval … requests elevated capabilities: shell execution` — the
last proving the fire reached the real `SkillPermissionChecker`. A path that
merely logged and returned `Ok` cannot produce that string.

---

## 5. Honest grade on Criterion 2

> *"Scheduled, event-driven, webhook, polling, and commitment work has bounded
> history, retry, continuation, and delivery."*

### **STILL PARTIAL. This work does NOT close Criterion 2.**

**Event-driven** moved from *not met* to *met*: it now has a producer, fires
end to end from the shipped binary, and is bounded, retried, historied and
delivered through the 24-01 ledger.

**Webhook** and **polling** are **NOT met and are not made met by this lane.**
Stopping a false promise is not the same as keeping it. I removed the silent
failure; I did not build the plane. Naming the still-open clauses precisely:

- **`webhook`** — nothing routes an inbound HTTP request to a job, and no
  authentication scheme exists for one. Threat T-24-02-02's mitigation still
  holds only because no caller can cause a fire at all. Closing it needs an
  inbound route class plus a real credential scheme (HMAC over body+timestamp,
  replay window, secret storage and rotation) — its own design lane. **~1.5
  sessions**, and it should reuse the event bus this lane built: an
  authenticated route that publishes a topic is most of the work.
- **`poll`** — nothing performs the HTTP request. Closing it needs an egress-
  policy-routed client and, first, a **defined response contract** for what "work
  is due" means. **~1 session** once that contract is written down.

Also still open from the ledger and untouched by this lane: the **continuation
gate** (no run has hard-killed a gateway mid-delivery and counted at an
out-of-process sink), the **surface gate** (no PTY drive, no rendered screen),
`max_in_flight` unenforced at dispatch, and **no macOS evidence**.

---

## 6. Scope, fences, and one finding handed on

- **Fence respected.** `git diff <merge-base> -- crates/wcore-cli/src/lib.rs
  crates/wcore-cli/src/main.rs crates/wcore-browser/` is **empty**. The new
  `cron publish` verb is a `CronCmd` variant in `cron.rs`, which needed no
  `main.rs` edit. Diffed against the captured merge-base SHA, never the branch.
- **`crates/wcore-gateway/src/automation.rs` — outside my original scope, and
  justified.** The dispatch path genuinely runs through it: the gateway wraps the
  handler in `LedgeredHandler`, so event fires reach the delivery ledger through
  this file. The edit is **+3 −12 in `mod tests` only** — three `FireContext`
  struct literals mechanically converted to `FireContext::scheduled(...)` because
  the struct gained a field. **No production code in that crate changed.**
- **No collision.** `git diff <merge-base> FETCH_HEAD` over the integration
  branch shows no other lane has touched any file I own.
- **Finding handed on, not fixed (out of lane):**
  `wcore-agent::cron::build_headless_cron_handler` hard-codes
  `wcore_config::config::Config::default()` instead of loading the operator's
  config, so `tools.skills.allow` / `auto_approve` **cannot be configured for the
  `cron daemon` at all** — a skill target can never be approved in a headless
  process. This is why my live drive could not produce a filesystem side effect
  from a skill body, and I have not counted that as evidence.
- **Stale doc comment left in place:** `wcore-agent/src/cron.rs:56-57` still says
  "a missing surface logs the fire and returns Ok", which the code no longer
  does. One-line fix, but it is a different lane's file and not load-bearing here.
