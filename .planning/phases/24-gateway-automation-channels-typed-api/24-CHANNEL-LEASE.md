# 24-CHANNEL-LEASE — the cross-process inbound consumption race

**Lane** `lane/channel-lease` · **base** `ef1d97be` (`plan/f20-unified-audit-repair`)
**Severity I would assign: HIGH.** Reasoning in §6.
**Verdict: finding CONFIRMED by measurement, fix LANDED, all four proof legs green.**
**One residual I did NOT fix is named in §7 and all three cross-audit reviewers graded it
`must-fix`.**

Binaries: reproduction on `0.12.25 (source 3d7d4a01…)`, fix on `0.12.25 (source e41dbd0e…)`.
All execution on `hetzner-dsm`. Nothing was compiled on the Mac (`cargo fmt --all -- --check`
clean, exit 0).

---

## 1. The finding, and that it is real

F24-C3-H4 closed a double `ChannelManager` **within one process**. It says nothing about a
second process, and three production sites each construct a manager and call `start_all()`:

| site | reached by |
|---|---|
| `crates/wcore-agent/src/bootstrap.rs` | **every ordinary `wayland-core` session** |
| `crates/wcore-agent/src/cron.rs` | `wayland-core cron daemon` (ships launchd + systemd units) |
| `crates/wcore-cli/src/gateway.rs` | the installed service |

Confirmed by reading before measuring: `.without_channels(true)` has **20 call sites in the
whole repository and every one is a test**, plus `channel_dispatch.rs:230` (the per-session
recursion guard). There are **zero production callers**, so the session path is not a corner
case — it is the default. `grep -rn "flock\|ScheduleLease" crates/wcore-channels/` returned
**no match**: before this lane there was no cross-process exclusion anywhere in the channel
stack.

Inbound polling is a **destructive read**. Telegram's `getUpdates?offset=N` permanently
deletes every update below `N` *for every consumer*; IMAP `FETCH` sets `\Seen`; Discord
allows one gateway session per token. So a second poller does not duplicate a message — it
**destroys** it for the first, and tells nobody.

---

## 2. Reproduction, with counts

Driver: `scripts/f24-channel-lease.mjs` (mine). It uses `scripts/f24-tg-fixture.mjs` — which
belongs to a concurrently-running lane — as an **unmodified black box** over its documented
`/__control/*` surface. No file belonging to another lane was edited. The fixture mints its
own bot token; **no vendor credential was involved and none was printed.**

### Leg 1 — startup / backlog theft (`LOSS REPRODUCED`)

Eight updates pending. **Nothing else running.** A single ordinary session started —
`wayland-core "<prompt>"`, the shipped one-shot surface, no flags, no test hooks. On its
*second* poll, two milliseconds after its first:

```
{'kind':'getUpdates.open','at':'19:40:02.076Z','poll':1,'offset':0,'open':1}
{'kind':'confirm',        'at':'19:40:02.078Z','poll':2,'offset':9,
                          'deleted':[1,2,3,4,5,6,7,8]}
```

| measure | value |
|---|---|
| submitted | 8 |
| destroyed by the session | **8** |
| received by the installed service (started afterwards) | **0** |
| still pending after the session exited | 0 |
| `instrument.fault` | `false` |

No error, no warning, no retry. The messages are simply gone.

### Leg 2 — steady state (`RACE REPRODUCED`)

| window | processes | `max_open` | polls / 20s |
|---|---|---|---|
| A | service alone | **1** | 18–19 |
| B | service **+** an ordinary session | **2** | 41 |

`poll_rate_ratio = 2.16`. **Two independent signals agree**, which is why both are carried:
the window-scoped concurrency reader goes 1 → 2, and the poll rate doubles. An alternating
pair that never overlapped would read `max_open = 1` and still be caught by the rate; a rate
confounded by load would still be caught by `max_open`. Neither is a log line the binary
printed about itself — both come from the fixture, in another OS process.

Steady state was included deliberately: it is what raised the *in-process* version of this
finding from MEDIUM to HIGH, and a startup-only run would have missed it.

---

## 3. The fix

**Reused `wcore-cron`'s `ScheduleLease`** — the `flock`/`LockFileEx` lease Phase 24 already
shipped for the cron schedule — via a new **additive** `attempt_named(dir, holder,
lock_file, record_file)`. A second exclusion concept was explicitly avoided: two mechanisms
for one invariant is how the double-manager defect arose, and a second release story would
be a fresh chance to reintroduce a stale-lock wedge.

New module `crates/wcore-agent/src/channel_lease.rs`, applied at **all three** `start_all()`
sites. `wcore-agent` and `wcore-cli` already depend on `wcore-cron` and `lease` is already
`pub mod`, so this needed **zero `Cargo.toml` / `Cargo.lock` churn** — no shared-seam risk
for the other four lanes. Neither fenced file (`wcore-cli/src/lib.rs`, `main.rs`) was touched.

The lease lives in `<home>/channels` as `channel-poll.lock` (one byte) plus a freely readable
`channel-poll.owner`. Neither ends in `.toml`, so the channel loaders cannot mistake them for
a config — asserted by a test.

**Release is by OS descriptor close**, inherited from `ScheduleLease`. Nothing has to run for
the next process to acquire, so `SIGKILL`, a panic or power loss all free it.

### What the loser does — decided, not parked

**(c) run normally without inbound polling, loudly.** It does not block (the loser is usually
an interactive session opened for unrelated work; hanging it would be a worse regression than
the defect) and it does not exit (same reason). Sending is unaffected.

Cross-audited per §4 of the brief — **3/3 unanimous**, plus an internal adversarial pass:

| panellist | position |
|---|---|
| codex `gpt-5.6-sol` | `PANEL_POSITION=c` |
| gemini `3.1-pro-preview` | `PANEL_POSITION=c` |
| kimi K3 | `PANEL_POSITION=c` |

Loudness is load-bearing, not decoration: a second process that *silently* has no channels is
a NEW silent failure substituted for the old one. The observer emits a stable token on
**stderr as well as tracing**, so it is visible without `RUST_LOG`:

```
F24_CHANNEL_LEASE=observer owner_pid=2369307 holder=session: another wayland-core process
is already receiving messages for this home; this one will not poll for inbound messages.
Sending still works.
```

Lease failures **fail closed** — if ownership cannot be established it has not been
established, and polling anyway is the exact defect being closed.

---

## 4. The four proof legs

All on the fixed binary, `instrument.fault = false` on every one.

### Leg 3 — the loser must not poll (`LEASE HOLDS`)

Service starts and owns; an ordinary session arrives second. Three claims, deliberately
separated **by source**:

| claim | source | result |
|---|---|---|
| the loser does not poll | fixture (another process) | `max_open_in_window = 1` |
| the loser is not silent | the loser's own stderr — that *is* the property under test | `loser_emitted_observer_token = true` |
| the holder gets **everything** | fixture, counted per message | `delivered_to_holder = 8 / 8` |

The holder also emitted `F24_CHANNEL_LEASE=owner`, and named the correct pid in the observer's
warning.

Leg 1 cannot serve as this test and is not presented as one: it runs the session *alone*, so
there is no contention and no lease decision — a session on its own is the legitimate owner
and polls both before and after the fix. Leg 1 characterises the destructive read; leg 3
tests the exclusion.

### Leg 2 post-fix — steady state (`NO RACE`)

| window | processes | `max_open` | polls |
|---|---|---|---|
| A | service alone | 1 | 18 |
| B | service + session | **1** (was **2**) | 21 |

`poll_rate_ratio 1.17` (was `2.16`).

### Anti-denial — a green cannot be manufactured by universal denial

The trap the brief names: a "fix" that stops *both* processes polling passes every
"no duplicate consumption" check. Three guards, all live:

- `max_open == 0` is graded **`DENIAL — nothing polled; a green here would be manufactured`**,
  which is a FAILURE, not a pass.
- Window A establishes the positive baseline at `max_open = 1` in the same run, so a
  post-fix `1` provably means "exactly one poller", not "nothing polls".
- Leg 3 counts **8/8 delivered to the holder** and leg 4 requires a message delivered after
  takeover.

### Leg 4 — the lease must release on ungraceful death (`TAKEOVER OK`)

| measure | value |
|---|---|
| holder emitted owner token | `true` |
| polls in the 8s after `SIGKILL` | **0** |
| successor alive | `true` |
| successor emitted owner token | `true` |
| takeover polls observed | `true` |
| message delivered after takeover | `true` |

`polls_in_8s_after_kill = 0` is worth naming twice: it proves the holder really was the sole
poller (so leg 3's `max_open = 1` is not "the fixture stopped working"), and it shows that
until a successor acquires, nothing is received — which is precisely why release matters.
**The stale-lease wedge that this program hit in the sandbox last night is not reintroduced
here**, and that is measured rather than argued.

---

## 5. Regression check

| suite | result |
|---|---|
| `wcore-cron` (whole crate) | **73 + 11 + 3 + 8 + 13 passed, 0 failed** — the pre-existing schedule-lease tests still pass, so `attempt_named` did not regress them |
| `wcore-agent --lib channel_lease::` | **4 passed** (expected 4, **0 ignored**) |
| `wcore-agent --test bootstrap_test` | **27 passed, 0 failed** |
| `wcore-agent --lib`, `--test-threads=1` | **2135 passed, 0 failed, 3 ignored** |

Counts are read back, never inferred from exit status (§3.2 — a suite can exit 0 having run
zero tests).

**A parallel `wcore-agent --lib` run first showed a ~19-test failure cluster** in
`engine::audit_2026_05_22_tests`, `session::`, `session_journal::` and `orchestration::`,
including one of my own. Per §6 of the brief I re-ran before reporting a regression: the same
cluster isolated at the same commit gave **119 passed, 0 failed**, and the full lib
single-threaded at the same commit gave **2135 passed, 0 failed**. Box load average was 4.44
with other lanes building. I am reporting the isolated numbers and saying which run each came
from. **I did not run a base-commit control**, so I cannot state with certainty that the
parallel cluster pre-exists my change — only that it does not reproduce single-threaded at my
commit, and that the crates I touched are green.

---

## 6. Severity: HIGH

- **No unusual configuration is required.** An installed service plus a user opening a
  normal session is the intended product usage. The session path has zero production
  `without_channels` callers.
- **It destroys user data** — inbound messages — not merely availability.
- **It is completely silent.** Nothing errors, warns, retries or records. The user's evidence
  is a message that never arrived.
- **Measured, not theorised:** 8/8 at startup; two concurrent pollers and 2.16x poll rate in
  steady state.

Not CRITICAL: it needs channels configured, it does not corrupt persistent state, and it does
not leak credentials.

---

## 7. What I did NOT do — read this before treating the lane as finished

1. **Starvation is NOT fixed.** Ownership is first-come. If a session takes the lease and
   the installed service starts afterwards, the **service** becomes the observer even though
   it is the intended owner, and stays one until that session exits. **All three cross-audit
   reviewers independently returned `STARVATION=must-fix`.** It is strictly better than the
   defect it replaces (today both poll and messages are destroyed) and it is bounded by the
   session's lifetime rather than unbounded — but it is real. Recommended follow-up, which
   needs **no new mechanism**: role-aware reacquisition — a long-lived service that loses the
   lease retries in the background and takes ownership the moment the session leaves.
2. **An existing observer never re-attempts.** The lease is attempted once, at boot. After
   the owner dies, an already-running observer does not take over; only a newly started
   process does. Leg 4 proves the recovery path a service manager actually uses (restart the
   unit), which is why it is graded OK — but the in-place upgrade does not happen. Same fix
   as (1).
3. **Only the Telegram polling path was exercised end to end.** IMAP `\Seen` and the Discord
   one-session-per-token constraint are argued from their documented semantics, not measured.
   The lease is adapter-agnostic (it gates `start_all`, not any one adapter), so the fix
   covers them, but the *loss* was only demonstrated for Telegram — it is the one polling
   adapter with a fixture seam.
4. **Linux only.** `ScheduleLease` carries a `LockFileEx` implementation for Windows and its
   unit tests are cross-platform, but no Windows leg was run in this lane.
5. **No base-commit control** for the parallel-run test cluster — see §5.
6. I did not merge, open a PR, tag, release, close an issue, or run `wcore-contract generate`.

---

## 8. Instrument defects found and repaired IN THIS LANE (§6b-ii)

Five, each repaired rather than written up, each with a three-assertion self-test whose third
assertion shows the OLD shape would have missed it. **21 assertions, 21 passing**
(`node scripts/f24-channel-lease.mjs --self-test`). Two mutants were run against the
concurrency reader and the grader and each killed exactly its target assertion, so the suite
can fail. It also went red on its own twice during development, which is the same evidence
obtained for free.

1. **LLM stub in-process → deadlock.** `Atomics.wait` blocked the same event loop that had to
   `accept`, so `listen()` never completed. Would *also* have starved the session silently
   for the 2s the steady-state leg blocks between submissions — reading as "a session that
   booted and declined to poll", i.e. a **false refutation of the finding under test**.
2. **Stub answered plain JSON, not SSE.** The engine logged `SSE stream closed before any
   terminal event`, retried, tripped its circuit breaker, and the session hung 90s. The
   self-test now asserts the **terminal** event (`data: [DONE]` + `finish_reason:"stop"`), not
   merely that bytes came back — a weaker assertion passes on the broken stub.
3. **Pooled HTTP agent → `socket hang up`** at the measurement point.
4. **Unretried `submit()`** turned one transient failure into a destroyed run.
5. **Leg 4 graded `WEDGED` — the most alarming verdict this driver can return — on a run
   where the successor never started.** `wcore-gateway`'s *pre-existing* PID lock refused a
   second gateway per home ("gateway already running for this home", 106-byte log), so "no
   polls after the kill" measured nothing about the channel lease. **A false CRITICAL is as
   damaging as a false green.** The grader now refuses to interpret takeover at all unless the
   successor was alive, and leg 4 starts the successor after the holder is dead.

Also carried: a **whitespace-stripping token matcher**, because a console wrap inside a token
reads as absence to `String.includes` — the defect this program has now measured twice, the
second time *because the first sighting was documented instead of repaired*.

### One claim I could not prove, and did not keep

The pooling repair was first self-tested as *"the old pooled agent FAILS across the idle
gap"*. **That assertion went red.** The failure is a race — it needs the server's idle close
to land in flight with the client's reuse — and a graceful close is normally detected and the
socket evicted. I did not delete the inconvenient assertion. I replaced it with a
deterministic one that proves what the repair actually does: the default agent reuses **one**
socket for two requests, `agent: false` opens **two**, so the reuse the race requires is gone
by construction. **The race is not reproduced on demand and is not claimed to be.**

---

## 9. Files

| path | change |
|---|---|
| `crates/wcore-cron/src/lease.rs` | additive `attempt_named` / `read_record_named`; record basename carried for `Drop` |
| `crates/wcore-agent/src/channel_lease.rs` | **new** — the inbound-polling lease + 4 tests |
| `crates/wcore-agent/src/lib.rs` | `pub mod channel_lease;` |
| `crates/wcore-agent/src/bootstrap.rs` | gate `start_all`; carry the lease on `BootstrapResult` for the session lifetime |
| `crates/wcore-agent/src/cron.rs` | gate `start_all`; `EngineJobHandler::with_channel_poll_lease` |
| `crates/wcore-cli/src/gateway.rs` | gate `start_all` (not a fenced file) |
| `scripts/f24-channel-lease.mjs` | **new** — the driver, 4 legs + 21 self-test assertions |
| `.planning/…/24-CHANNEL-LEASE-NOTES.md` | running notes, committed from minute 12 |

Evidence on `hetzner-dsm`: `/root/f24cl-run-base` (reproduction), `/root/f24cl-run-fixed`
(legs 3+2), `/root/f24cl-run-l4` (leg 4).
