# 24-C2 — live evidence

Every transcript below is a verbatim capture from the **real `wayland-core`
binary** on `hetzner-dsm`, built from this lane's tree
(`cargo build -p wcore-cli --bin wayland-core`, `target/debug/wayland-core`).
Nothing here is a test harness result.

---

## 0. The measurement that established ground truth (BEFORE any change)

Probe test driving the real `tick_once_at` loop over six ticks spanning six
hours, at base `d53fd54a`. The probe asserted `usize::MAX` so the observed count
was printed by the failure message; it was deleted once it had been read.

```
PROBE event fires=0
PROBE webhook fires=0
PROBE poll fires=6
```

**This contradicts `CRITERIA-GAP-LEDGER.md` row 24-C2 on `poll`.** The ledger
records all three as "validates, persists, lists, NEVER FIRES". `event` and
`webhook` never fire — confirmed. `poll` **fires on the clock, once per due
window, having never contacted the URL**: `next_after` returned
`Some(after + every_secs)`, so `--trigger poll:https://x/health:300` was
`--trigger every:300` with an ignored URL string. It ran the job's action
unconditionally, regardless of what the remote would have said.

### The stated mechanism was refuted

The brief suggested `crates/wcore-agent/src/cron.rs`'s doc comment — *"a missing
surface logs the fire and returns Ok"* — was the likely mechanism. **It is not.**
That comment is **stale**: `EngineJobHandler::dispatch` returns
`CronError::NoDispatcher` for a missing slash/skill sink and
`CronError::Dispatch` for a missing channel sink, and `log_only_slash_and_skill_
return_no_dispatcher` guards exactly that. It describes `Target` (what a job
does), not `Trigger` (when it fires), and no trigger of any kind reached it.

The real mechanism is one level up, in `wcore-cron`:

| kind | mechanism | fires? |
|---|---|---|
| `event` | `Trigger::next_after` → `None`; the tick logs "schedule has no future occurrence; skipping". **Nothing publishes a topic — no bus exists.** | never |
| `webhook` | `Trigger::next_after` → `None`. `wcore-agent/src/inbound_webhook.rs` exists but routes only `/webhooks/:channel` to chat connectors, keyed on per-connector platform signature verification. It has no knowledge of cron at all, and no credential scheme exists for a cron path. | never |
| `poll` | `next_after` → `Some(after + every_secs)` — **clock-driven**. No HTTP client exists anywhere in the dependency graph of `wcore-cron` (zero workspace deps) or `wcore-gateway`. | **fires, unconditionally, without ever polling** |

A workspace-wide grep confirms no producer: `Trigger::Event|Webhook|Poll` is
constructed nowhere outside `wcore-cron/src`, `wcore-cli/src/cron.rs` and test
files.

---

## 1. `webhook` and `poll` are refused at add time, with a non-zero exit

```
$ export WAYLAND_HOME=/tmp/wl-24c2-live
$ wayland-core cron add --trigger webhook:/hooks/build --slash /brief
wayland-core cron: refusing to create a webhook job: webhook triggers have no
producer in this build: nothing routes an inbound HTTP request to a job, and no
authentication scheme exists for one. This job will never fire. Use
`cron publish` with an `event:` trigger instead.
Nothing was written.
  EXIT=1

$ wayland-core cron add --trigger poll:https://status.test/health:300 --slash /brief
wayland-core cron: refusing to create a poll job: poll triggers have no producer
in this build: nothing performs the HTTP request, so no response can say work is
due. This job will never fire. Use `every:SECONDS` if you wanted a plain timer.
Nothing was written.
  EXIT=1

$ wayland-core cron list
(no cron jobs)
store: /tmp/wl-24c2-live/cron/jobs.json
```

Both refused, both exit 1, nothing persisted.

---

## 2. `event` is accepted and now has a producer

```
$ wayland-core cron add --trigger event:build.finished --channel team --text "build is green"
next:    driven externally (event) — not predictable from the clock
added 7879d294-66af-44fc-9fe6-637999359918
  EXIT=0

$ wayland-core cron list
on  7879d294-66af-44fc-9fe6-637999359918  [event     ] @event build.finished  channel  team :: build is green  last_fired=never
```

---

## 3. Publish → real daemon → the job ACTUALLY RUNS

This is the end-to-end drive. The observable is the fire record and the queue
consumption, not a return code.

```
$ wayland-core cron history 7879d294-…            # BEFORE
(no fire records for 7879d294-66af-44fc-9fe6-637999359918)

$ wayland-core cron publish build.finished
published "build.finished" (e0d7b880-ff06-4ef6-9fe0-37c7d1dd6995)
1 subscribed job(s) will fire on the schedule owner's next tick
  EXIT=0

$ ls -la $WAYLAND_HOME/cron/events/
drwx------ 2 root root 4096 Jul 28 09:47 .
-rw-r--r-- 1 root root  118 Jul 28 09:47 e0d7b880-ff06-4ef6-9fe0-37c7d1dd6995.json

$ wayland-core cron daemon
cron daemon started (pid 631567)
  log:  /tmp/wl-24c2-live/cron-daemon.log
# … 40s …

$ wayland-core cron history 7879d294-…            # AFTER
2026-07-28T09:47:49Z  staged (no live dispatcher)

$ ls $WAYLAND_HOME/cron/events/ | wc -l
0
```

**A fire record exists where none could ever exist before, and the queued event
was consumed by the drain.** The outcome is `staged` because no channel named
`team` is registered on that host — the correct honest outcome for an
unregistered channel target, and materially different from the silence that
preceded this change.

---

## 4. Three independent traces proving the fire reaches the REAL dispatch path

To rule out "the record is written but nothing runs", the same publish was driven
against a `--skill` target three times, each with a different real answer from
the engine-side skill sink built by `build_headless_cron_handler`:

| run | skill placement / body | recorded outcome |
|---|---|---|
| A | `$WAYLAND_HOME/skills/touchproof`, `!shell:` line body | `success (5ms)` — the sink ran the skill |
| B | `~/.config/wayland-core/skills/touchproof` **removed** | `error: dispatch error: skill: Skill 'touchproof' not found. Available skills:` |
| C | ` ```! ` shell block body, skill resolvable | `error: dispatch error: skill: Skill 'touchproof' requires user approval before execution. Skill 'touchproof' requests elevated capabilities: shell execution.` |

Run C is the strongest: the fire travelled publish → queue → drain → job
selection → `dispatch_and_record` → `EngineJobHandler` → `SkillTool` → the real
`SkillPermissionChecker`, and came back with that checker's real verdict. A path
that merely logged and returned `Ok` cannot produce that string.

**What I did NOT obtain:** a filesystem side effect from the skill's shell body.
`build_headless_cron_handler` hard-codes `wcore_config::config::Config::default()`
rather than loading the operator's config, so `tools.skills.allow` /
`auto_approve` cannot be configured for the `cron daemon` at all and run C can
never be approved. That is a **separate pre-existing defect in the headless
daemon, outside this lane** — reported, not fixed, and not counted as evidence.

---

## 5. Test evidence, and which run each figure came from

All on `hetzner-dsm`, isolated per crate.

| suite | result |
|---|---|
| `cargo test -p wcore-cron` | **108 passed, 0 failed** (73 unit + 11 `event_producer` + 3 + 8 + 13) |
| `cargo test -p wcore-gateway` | **63 passed, 0 failed** (34 + 7 + 9 + 8 + 5, 1 pre-existing `live_bundle_canary` ignored at base) |
| `cargo test -p wcore-cli` | **2280 passed, 0 failed** |
| `cargo clippy -p wcore-cron -p wcore-gateway --all-targets` | clean, no warnings |

**One contention artifact, reported rather than hidden.** A first full
`cargo test -p wcore-cli` run — taken while other lanes were building —
reported `import_is_idempotent_without_overwrite` (`tests/migrate_hermes.rs`)
FAILED. Re-run alone at the same commit: **7 passed, 0 failed.** The same test
also passes at base (`/root/wayland-24`, `cd00ff2f`): 6 passed, 0 failed. It is
unrelated to this change (hermes profile migration) and is the known
load-flakiness class. The 2280/0 figure above is the isolated re-run.

---

## 6. Every guard proven able to fail (mutation runs)

Each mutation was applied to the source on `hetzner-dsm`, the suite run, and the
source restored from a backup taken first. Baseline re-verified green after.

| # | mutation | result |
|---|---|---|
| 1 | delete the `drain_published_events` call from `tick_once_at` — **this is literally the state the crate shipped in** | **5 of 11 FAILED** |
| 2 | restore `Self::Poll` to the clock-driven arm of `next_after` — the measured 24-C2 defect | **1 FAILED** (`a_poll_job_never_fires_because_nothing_performs_the_poll`) |
| 3 | `break` after the first matched subscriber (kills fan-out) | **1 FAILED** |
| 4 | delete the rate-bound check in the drain | **2 FAILED** |
| 5 | consume a rate-held event anyway (silent loss) | **2 FAILED** |
| 6 | delete `refuse_without_producer` from `cron add` — restores silent acceptance | **1 FAILED** (`wcore-cli`) |
| — | baseline restored | **11 passed, 0 failed** / **9 passed, 0 failed** |

Mutation 1 is the important one: it reproduces the exact shipped defect, and the
guards go red. The pre-existing suite was fully green in that state.
