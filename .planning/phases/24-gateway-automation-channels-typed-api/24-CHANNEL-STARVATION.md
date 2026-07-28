# 24-CHANNEL-STARVATION — the installed service now wins, and the loser takes it back

**Lane** `lane/channel-starvation` · **base** `2f5e6479` (`plan/f20-unified-audit-repair`, the
merge commit that landed `lane/channel-lease`) · **HEAD** see §9.

**Verdict: all four proof legs GREEN on the shipped binary, live, on `hetzner-dsm`, plus a fifth (leg E) that closes the one gap this report first declared unproven.**
`instrument.fault = false`. Across the whole four-leg run — 144 polls, three handovers —
the endpoint never saw more than **one** concurrent poller, and **4 of 4** submitted messages
were delivered. What I did **not** prove is in §7 and it is not small.

Nothing was compiled on the Mac. `cargo fmt --all -- --check` clean, exit 0.

---

## 1. The residual I own, and why it mattered as much as the defect it replaced

`lane/channel-lease` closed a real, severe cross-process defect: an ordinary one-shot session,
no flags, deleted updates 1–8 from a consume-on-read endpoint on its second poll, and the
installed service — started afterwards — received **0 of 8**. A `flock` `ScheduleLease` now
guarantees one poller.

**But ownership was first-come.** A session that started first made the **installed service**
the observer, and it stayed one for that session's whole life. All three cross-audit reviewers
independently returned `STARVATION=must-fix`.

The failure had *moved*, not vanished. Before, a user running a session beside the service lost
messages. After, the service stops receiving for as long as that session lives — and a
long-lived interactive session (tmux, a VS Code terminal, an ssh session left open) means the
thing the user installed to be always-on is silently idle. It is quieter than the original
defect, and that is precisely what makes it worse in one specific way: **nothing is lost, so
nothing looks wrong, and mail simply stops arriving.**

A second residual travelled with it, from the same single line of code: **an existing observer
never re-attempted.** After the owner died, only a *newly started* process could take over. The
in-place upgrade required operator action.

Both are the same fact — ownership was decided **once, at boot**, at three sites
(`bootstrap.rs`, `cron.rs`, `wcore-cli/gateway.rs`) — and both are closed here.

---

## 2. The precedence rule, and the justification

```
gateway  (30)   >   cron-daemon  (20)   >   session  (10)
```

Strictly. Anything unrecognised ranks at the floor (`session`), so a self-reported or
mistyped holder string can never preempt anybody.

**Why role and not arrival order.** The rule has to encode *intent*, and arrival order encodes
nothing. A gateway exists **only** because the user installed a unit to be always-on; a session
exists because somebody typed a command and will close the terminal. The correct invariant is
"if the service is alive, it polls" — everything else is a degraded mode. Ranking `cron-daemon`
above `session` for the same reason also preserves the one genuinely good property first-come
had: **a session started while no service is running polls immediately, and cedes the moment a
service appears.**

**Cross-audited per brief §4 — 3/3 unanimous on all three tokens**, plus an internal adversarial
pass (§8). Raw votes in `24-CHANNEL-STARVATION-evidence/panel-{codex,gemini,kimi}.txt`.

| panellist | precedence | mechanism | rank |
|---|---|---|---|
| codex `gpt-5.6-sol` | `service` | `yield` | `gateway>cron>session` |
| gemini `3.1-pro-preview` | `service` | `yield` | `gateway>cron>session` |
| kimi K3 | `service` | `yield` | `gateway>cron>session` |

**Invocation note.** Codex's first run returned **39 bytes** — `Reading additional input from
stdin...` — silently dropping its vote, which would have read as a 2/3 majority. Re-run with
`< /dev/null`: 7188 bytes, vote present. That is a **fourth** way this panel drops a vote
quietly, on top of the three the brief lists. Extraction was unanchored and took the **last**
match, per §4.

**The strongest argument against, stated because it is real.** Codex and kimi both raised it:
this trades a *static, silent misallocation that never loses a message mid-stream* for a
mechanism whose every handover is a window where a destructive read races a lease transition.
My answer is that the floor is preserved by construction (§3) — and because that is a claim, it
is measured in §4 rather than asserted.

Gemini's: it makes a foreground session harder to use for debugging inbound routing, because a
running service will always take polling back. That is a genuine cost and it is not mitigated.

---

## 3. The mechanism — no new exclusion concept

**`flock` remains the sole proof of ownership.** It has no preemption: a claimant cannot seize a
held lock and the holder cannot be forced to drop one, so "the service wins" is necessarily a
**voluntary yield**. On top of the unchanged lease sits an **advisory claim**:

- a non-owner publishes `<home>/channels/channel-poll.claim.<pid>` carrying its rank, and
  refreshes it every tick;
- an owner that sees a **fresh** claim of strictly higher rank calls `stop_all()`, **then**
  drops the lease;
- every non-owner re-attempts the lease each tick and `start_all()`s when it wins.

**The claim is not a second exclusion mechanism, and that distinction is the entire safety
argument.** A claim cannot grant polling and cannot deny it. If claims are missing, unreadable,
corrupt, or stale, behaviour degrades to the *previous* first-come lease. It cannot degrade
toward two pollers, because two pollers would require two holders of one `flock`. A second
exclusion concept is exactly what produced the original double-manager defect, and it was
avoided deliberately.

Reuse is total: `ScheduleLease::attempt_named` was already there, `wcore-agent` and `wcore-cli`
already depend on `wcore-cron`. **Zero `Cargo.toml` / `Cargo.lock` churn** — verified, §9.

### The two failure modes the panel found in my design, and the one device that closes both

I had written the design down before asking. Two panellists broke it:

- **Gemini — WEDGE.** My first design suppressed contention while any higher claim existed. A
  dead high-rank claimant's file then suppresses the ex-owner forever: it yielded, the claimant
  never takes the lock, **nobody polls**. That is manufactured denial promoted from a test
  artifact into a production bug.
- **Codex — OSCILLATION.** Strict rank inequality does **not** prevent oscillation, contrary to
  what I had asserted in the question. It prevents *equal-rank* ping-pong only. A yielding
  ex-owner can beat the higher-ranked claimant back to the free lock, then yield again.

One device closes both: **a claim suppresses contention only while it is FRESH** (`mtime` within
`3 × tick`). A live claimant refreshes every tick and stays suppressive — so the ex-owner never
races it. A dead one stops refreshing, goes stale within the TTL, and the ex-owner contends and
wins — so nothing wedges.

**Where I take the minority position.** All three panellists asked for a **pid-liveness check**
(`kill(pid,0)`), and codex asked for pid *plus process-start identity* to defeat pid reuse. I
did neither. `wcore-cron`'s own lease already argues, correctly, that a recorded pid proves
nothing; freshness needs no process identity at all, so the pid-reuse hazard all three then had
to patch around simply does not arise. The pid in a claim is for the operator message and is
never consulted for a decision. Their requirement — that a stale claim must not be honoured — is
met; only the detector differs, and mine has no failure mode of its own to reason about.

**A gap loses nothing.** Telegram retains updates until an `offset=` confirm, IMAP until
`\Seen`. A window with no poller costs latency; a wedge would cost silence. That asymmetry is
why the TTL exists, and why the wedge was worth closing at the price of a bounded gap.

---

## 4. The proof legs

All on the shipped `wayland-core` binary (debug, `0.12.25`, source `37a99a2c`), on
`hetzner-dsm`, one run: `.../24-CHANNEL-STARVATION-evidence/run-full/`. Driver exit `0`,
`instrument.fault = false`, `ss_available = true`, 184 attribution samples, 144 polls.

**Attribution is not self-reported.** "Which process is polling" is read by the driver — a
**third OS process** — from `ss -tnpH`, as the set of pids holding an established TCP connection
to the fixture. The binaries' own `F24_CHANNEL_LEASE=` tokens are carried only for the property
that genuinely *is* self-report: that a non-polling process says so out loud.

### Leg A — the service starts SECOND and still ends up polling · `SERVICE WINS FROM BEHIND`

Session first (pid 3337956), gateway second (pid 3350595).

| window | `max_open` | polls | pid attributed by `ss` | grade |
|---|---|---|---|---|
| W1 — session alone | **1** | 10 | **3337956** (session) | `OK` |
| W2 — after handover | **1** | 10 | **3350595** (gateway)| `OK` |

- session yielded at `20:56:33.061`, naming `to_pid=3350595 to_holder=gateway`;
- gateway acquired at `20:56:37.267`;
- **delivered while the session owned: yes** (update served, `deleted_by` poll 12);
- **delivered after the handover: yes** (`deleted_by` poll 34);
- both processes still alive at both measurements.

W1 is the positive baseline **in the same run**: a post-handover `1` means "exactly one poller",
not "nothing polls", because `1` was already established while the session held it.

### Leg B — the service starts FIRST and is not disturbed · `SERVICE KEEPS IT`

Gateway first (pid 3356349), session second (pid 3358778).

| window | `max_open` | polls | pid attributed | grade |
|---|---|---|---|---|
| W1 — gateway alone | 1 | 7 | 3356349 (gateway) | `OK` |
| W2 — both alive | 1 | 10 | **3356349** (gateway) | `OK` |

Negative assertions, both held: `gateway_yielded_wrongly = false`,
`session_acquired_wrongly = false`. The session emitted its observer token. Message delivered to
the gateway (`deleted_by` poll 98).

### Leg C — nothing is lost, counted · `NOTHING LOST`

| measure | value |
|---|---|
| submitted across the whole run | **4** |
| delivered | **4** |
| never delivered | **0** |
| served more than once | **0** |
| still pending at the end | **0** |

### Leg D — a LIVE observer takes over on the holder's death, unaided · `LIVE OBSERVER TOOK OVER`

Gateway owner (pid 3372003) + session observer (pid 3372116), both alive. Gateway `SIGKILL`ed at
`20:58:43.336`. **Nothing was started afterwards** — that is the point of the leg.

| measure | value |
|---|---|
| session was a live observer before the kill | **true** |
| session alive after the kill | **true** |
| session emitted `F24_CHANNEL_LEASE=acquired` | **true**, 4.74s after the kill |
| window after takeover | `max_open = 1`, 10 polls, attributed to **3372116** (the session) |
| message delivered after takeover | **true** (`deleted_by` poll 144) |

This is deliberately **stronger** than the landing lane's leg 4, which started a *new* process
after the kill. That proves the recovery a service manager performs (restart the unit); it does
not prove the in-place upgrade, which was the residual.

### Leg E — a DEAD claimant must not wedge the home · `NO WEDGE — RECOVERED UNAIDED`

Added after the first four legs were already green, because §7 named this the most valuable
missing leg and a measured refusal to close it would have been a worse deliverable than the
extra run. Separate run: `.../24-CHANNEL-STARVATION-evidence/run-e/`, driver exit `0`,
`instrument.fault = false`.

This is the failure that would be **strictly worse than the starvation being fixed**: the owner
yields to a higher-ranked claimant, the claimant dies before taking the lock, and nobody polls —
for ever. Gemini and codex both named it; it is manufactured denial promoted out of the harness
and into production.

**The claimant is dead by construction.** The driver plants a well-formed gateway-ranked claim
naming pid `4242424` and **never refreshes it**. No process is started; nothing touches the file
again. The only thing that can end the standoff is the claim ageing past its TTL — which is the
property under test. (Note that `4242424` may well be a *valid* pid on Linux: the design never
consults pid liveness, so it does not matter, and this leg demonstrates that freshness alone
suffices.)

| measure | value |
|---|---|
| session owned polling first | **true** (W1: `max_open=1`, 10 polls, attributed to pid 3586270) |
| session yielded to the dead claim | **true**, at `21:12:26.978` |
| session recovered **without help** | **true**, at `21:12:31.853` |
| **wedge window** | **4875 ms** (bound asserted at 30 000 ms, not merely reported) |
| W2 after recovery | `max_open=1`, 10 polls, attributed to **the same session** | 
| message delivered after recovery | **true** (`deleted_by` poll 22) |
| whole-run max concurrent `getUpdates` | **1** |

The zero-poller window is confirmed **independently of the binary's own log**, from the fixture
journal in another process: **4770 ms** with no `getUpdates` open at all, from `21:12:27.084` to
`21:12:31.854`. The two numbers agree to within one poll interval, and they were measured by two
different processes from two different sources.

At the harness's 1s tick with `TTL = 3 × tick`, recovery took ~4.9s. At the production 2s
default expect roughly **10s**. Nothing is lost in that window (§3).

This leg is also the one that proves the anti-denial grader can fire on something **real**: it
deliberately creates a genuine zero-poller window, and the run is graded a pass only because
polling *resumed* and a message was *delivered* afterwards.

### Anti-denial — a green cannot be manufactured by universal denial

Three guards, all live:

- `polls == 0` in any steady window is graded **`DENIAL`**, a FAILURE, not a pass;
- every leg establishes a **positive baseline** window in the same run;
- every leg requires a **message actually delivered**, counted by the fixture in another process
  — 4 of 4.

### The handover transient, measured rather than excluded

I said I would exclude the transition from the steady windows and then **measure and report**
it. Measured from the fixture journal over the handover, widened 3s each side so the transient
cannot hide just outside the token times:

| window | max concurrent `getUpdates` |
|---|---|
| leg A handover (`yield-3s` … `acquire+3s`) | **1** |
| leg D handover (`kill-3s` … `kill+12s`) | **1** |
| **the whole four-leg run** | **1** |

The feared instrument artifact — an aborted long-poll still counted OPEN by the fixture for up
to 2s, reading as a phantom second poller — **did not occur**: the abort closes the connection
and the fixture's counter drops. The trace shows it cleanly, poll 12 closing before poll 13
opens:

```
20:56:33.170  getUpdates.close  open=1  poll=12     <- session's last poll
20:56:37.267  getUpdates.open   open=1  poll=13     <- gateway's first poll
```

**Longest window with no poller anywhere in the run: 4097 ms**, and it is that handover. At the
production 2s cadence expect roughly double. Nothing is lost in it (§3).

---

## 5. Regression — all on `hetzner-dsm`, counts read back, never inferred from exit status

| suite | result |
|---|---|
| `wcore-agent --lib channel_lease::`, `--test-threads=1` | **14 passed, 0 failed, 0 ignored** (expected 14: 4 pre-existing + 10 new) |
| `wcore-agent --lib`, `--test-threads=1` | **2145 passed, 0 failed, 3 ignored** (landing lane measured 2135 at its commit; +10 is exactly my new tests) |
| `wcore-agent --test bootstrap_test` | **27 passed, 0 failed, 0 ignored** |
| `wcore-cron` (whole crate) | **73 + 11 + 3 + 8 + 13 = 108 passed, 0 failed, 0 ignored** — identical to the landing lane's, so `attempt_named` reuse still does not regress the schedule lease |
| `wcore-cli --lib` | **1831 passed, 0 failed, 1 ignored** |
| `cargo clippy -p wcore-agent -p wcore-cli --all-targets` | clean — the only warning is the pre-existing `imap-proto v0.10.2` future-incompat notice, which is at base |
| `cargo fmt --all -- --check` (Mac) | clean, exit 0 |

**I did not see the ~19-test parallel cluster the brief describes**, because I ran serially
throughout, as instructed. I therefore say nothing about it either way. Box load was 3.17 during
the serial run — light, unlike the runs that produced the cluster.

The state machine is unit-tested by driving **two simulated processes with injected pids inside
one test process**, so every ordering is exact rather than raced against a timer. `flock` is
owned by the open file description, so two attempts in one process genuinely conflict — the same
property the landing lane relied on. That is what makes the wedge and oscillation cases testable
at all:

- `the_service_takes_polling_from_a_session_that_got_there_first`
- `the_service_keeps_polling_when_it_started_first`
- `equal_ranks_never_preempt_each_other`
- `an_observer_defers_to_a_fresh_better_claim_rather_than_racing_it`
- `a_dead_claimants_stale_claim_cannot_wedge_polling`
- `an_already_running_observer_takes_over_when_the_owner_dies`
- `the_rank_order_is_total_and_floors_unknown_holders`
- `an_unparseable_or_future_dated_claim_is_treated_as_absent`
- `a_claim_file_cannot_be_mistaken_for_a_channel_config`
- `the_tick_interval_is_clamped`

---

## 6. Instrument defects found and REPAIRED in this lane (brief §6b-ii)

Four. Each repaired here rather than written up, each with a three-assertion self-test whose
**third** assertion is that the old shape would have missed it. **27 assertions, 27 passing**
(`node scripts/f24-channel-starvation.mjs --self-test`,
`24-CHANNEL-STARVATION-evidence/self-test.txt`).

**1. The `git` I was handed is not `git`.** A harness hook rewrites bare `git …` to `rtk git …`,
a token-reducing proxy. In this repository its `git log --oneline` **silently omitted the merge
commit that is this lane's base**, while `git rev-parse HEAD` reported it. Two readers of one
fact disagreed and neither said so. Taken at face value it would have put the wrong base SHA in
this report and in every fence diff. `command git` does *not* bypass it — the hook rewrites the
command text. **Repair:** the reader cross-checks `rev-parse` against `log -1 --format=%H` and
**refuses to answer when they disagree**, rather than merely preferring an absolute path (a
habit, and habits are what get documented and then forgotten).

**2. My own grader returned DENIAL for a window in which polling demonstrably happened — a false
CRITICAL.** Found by **mutation-testing the grader before it ever ran**: I broke the
zero-poller check and the suite stayed green at 21/21. Chasing why exposed a real defect. The
fixture pushes a concurrency sample on poll OPEN (after increment, `>= 1`) *and again* on poll
CLOSE (after decrement, possibly `0`). A window catching only close-side samples reads
`max_open = 0` with `polls > 0`. My grader tested `maxOpen === 0` first and called it `DENIAL`.
**A false CRITICAL is as damaging as a false green** — this is the landing lane's
`WEDGED`-on-a-run-with-no-successor in a new costume. **Repair:** poll *count* is the
anti-denial measure (it is the direct one); `max_open` is read only once an open-side sample has
landed, and a window without one is graded `UNREADABLE` — neither a pass nor a denial.

**3. Codex silently returned 39 bytes and dropped its panel vote** (`Reading additional input
from stdin...`). **Repair:** panel invocations pass `< /dev/null`. Recorded in §2.

**4. Every window's attribution was recorded as `"pids": {}`.** The set of attributed pids was
carried as a JS `Set`, and `JSON.stringify` serialises a `Set` as `{}`. The first live run's
evidence file therefore recorded the **strongest possible negative** — "no process was
attributed" — for windows where attribution had *succeeded*. The evidence file is the
deliverable; a reader would have been misled by it. **Repair:** both sides speak arrays, and the
evidence carries a sorted pid list.

### The instrument can fail — mutants run against it

| mutant | killed by |
|---|---|
| `ss` parser loses peer anchoring | `ss/known-negative` |
| token matcher stops stripping whitespace | `token/known-positive` |
| git reader stops cross-checking | `git/known-negative` |
| grader stops treating zero polls as `DENIAL` | 3 assertions |
| grader reverts to `maxOpen`-first (defect #2) | `falsedenial/known-positive` + its old-shape assertion |

### One production defect the live run found in my own fix

The first live run emitted **23 `WARN` lines from one observer session in ninety seconds**: the
supervisor's per-tick re-attempt reused the boot attempt's log levels. At the production 2s
cadence that is a `WARN` every two seconds for the entire life of any non-polling process —
which would **bury the one line that matters, the role change, under the noise it created**.
Loudness that is always on is not loudness. Routine re-attempts now log at `trace`; the boot
announce and every role change are unchanged. Re-measured on the second live run: **1 `WARN`,
1 stderr line, 20 `TRACE`** (the trace lines visible only because the harness sets
`wcore_agent::channel_lease=trace`).

The brief's requirement that **the observer stay observable** is met and extended: the boot
observer token is unchanged, and two new stable tokens mark the transitions —
`F24_CHANNEL_LEASE=yielded holder=… to_pid=… to_holder=…` and
`F24_CHANNEL_LEASE=acquired holder=… pid=…` — on stderr as well as tracing, so both are visible
without `RUST_LOG`.

---

## 7. What I did NOT prove — read this before treating the lane as finished

1. **The `cron-daemon` role was never live-exercised.** Its rank, its supervisor and its call
   site are code and unit tests only. Legs A/B/D used `session` and `gateway`. The mechanism is
   role-agnostic and the rank order is unit-tested, but "a cron daemon takes polling from a
   session and hands it to a gateway" is argued, not measured.
2. **Only the Telegram polling path, end to end** — the same limitation the landing lane
   recorded. IMAP `\Seen` and Discord's one-session-per-token are argued from their documented
   semantics. The supervisor gates `start_all`/`stop_all`, not any one adapter, so the fix
   covers them; the *behaviour* was demonstrated for Telegram only, the one polling adapter with
   a fixture seam.
3. **Linux only.** `ScheduleLease` carries the `LockFileEx` implementation and the new unit
   tests are cross-platform, but **no Windows leg was run in this lane.** Claim files are
   ordinary files with temp+rename, which is `MoveFileEx`-backed on Windows, but that is
   reasoning, not a measurement.
4. ~~**The wedge bound is unit-tested, not live-tested.**~~ **CLOSED — see leg E (§4).** This was
   named here first as not proven, then proven: the wedge bound is now measured live on the
   shipped binary at **4875 ms**, with an independent **4770 ms** zero-poller window read from
   the fixture in another process. What remains unproven is only the *production-cadence* figure
   — leg E ran at a 1s tick, and the 2s default is extrapolated, not measured.
5. **Redelivery across a handover was not exercised.** Kimi raised it: a handover between
   *receive* and *confirm* legitimately redelivers a message to the successor. Leg C measured
   `served_more_than_once = 0`, but no message was in flight during any of my handovers, so the
   in-flight case is **untested**. It would be duplication, not loss, and it is bounded by one
   batch — but I did not measure it.
6. **`BootstrapResult::channel_poll_lease` changed meaning**, from a value fixed at boot to one
   that changes over the process's life. No production consumer reads it (grep: zero call sites
   outside the three assignment points), so this is a latent trap for a future reader rather
   than a live defect. The field doc says so explicitly.
7. **The 2s default cadence is a judgement, not a measurement.** I did not benchmark handover
   latency against tick cost. At the harness's 1s tick the observed handover gap was 4.1s;
   expect roughly double in production.
8. I did not merge, open a PR, tag, release, close an issue, or run `wcore-contract generate`.

---

## 8. Internal adversarial pass — arguing against my own conclusion

**"The four legs are green because the driver chose windows where they would be."** Partly fair,
and it is why the transient is measured separately (§4) rather than only excluded. The strongest
answer is the run-wide figure, which is chosen by nobody: **max concurrent `getUpdates` over the
entire run = 1**, across 144 polls and three handovers.

**"`max_open = 1` after the handover only proves one poller, not the RIGHT one."** True of the
concurrency reader alone, which is exactly why kernel-side attribution exists: `ss` named
**3350595, the gateway** in leg A's post-handover window and **3337956, the session** before it.
Neither number came from a log the binary wrote.

**"The tokens are self-reported, so `session_emitted_yield_token` proves nothing."** Correct, and
they are not used to establish who polls — attribution and delivery counts do that. The tokens
are used only for the property that *is* self-report: whether a non-polling process announces
itself. That property is the whole point of keeping the observer observable.

**"You have replaced a starvation bug with a churn bug."** The honest limit is §7.4 and §7.5:
oscillation is closed by design and by unit test, but not live. What *is* live evidence against
churn is that leg B ran both processes side by side for ~70s with
`gateway_yielded_wrongly = false` and zero role changes, and that the whole run recorded exactly
three handovers, all intended.

**"A 4-second delivery gap is a regression."** It is a real cost and it did not exist before.
The defence is that the gap loses nothing (§3) and it replaces an *unbounded* silence at the
service. I would rather report the number than omit it.

---

## 9. Files, and the seams I did not touch

Diffed against the **merge-base SHA** captured once at the start —
`2f5e64798c4aa18cfde13774faeb0cf7d9ffb2fb` — never against the branch name (brief §6).

| path | change |
|---|---|
| `crates/wcore-agent/src/channel_lease.rs` | role ranks, advisory claims, `PollControl`, `ChannelPollSupervisor`, +10 tests |
| `crates/wcore-agent/src/bootstrap.rs` | hold a supervisor for the session lifetime |
| `crates/wcore-agent/src/cron.rs` | same, holder `cron-daemon` |
| `crates/wcore-cli/src/gateway.rs` | same, holder `gateway` (not a fenced file) |
| `scripts/f24-channel-starvation.mjs` | **new** — 4 legs, kernel-side attribution, 27 self-test assertions |
| `.planning/…/24-CHANNEL-STARVATION-NOTES.md` | running notes, committed from minute ~14 |
| `.planning/…/24-CHANNEL-STARVATION-evidence/` | panel votes, raw logs, `result.json`, conn samples, transient analysis, self-test |

- **Fenced files untouched:** `git diff $BASE -- crates/wcore-cli/src/{lib,main}.rs` → **0 lines**.
- **No `Cargo.toml` / `Cargo.lock` churn:** `git diff --name-only $BASE -- '*Cargo.toml'
  Cargo.lock` → **empty**. No shared-seam risk for the other lanes.
- **No protocol or wire-contract change.** Nothing for the orchestrator to serialise.
- The Telegram fixture `scripts/f24-tg-fixture.mjs` belongs to `lane/channel-lease` and was used
  **unmodified**, as a black box over its `/__control/*` surface.
- No vendor credential was involved and none was printed; the run mints its own bot token.

Evidence on `hetzner-dsm`: `/root/f24cs-run-full` (the four-leg run), `/root/f24cs-run-a`
(the first run, which failed on my own 60s budget — kept because §6's budget defect is read off
it), `/root/wayland-cs-reg.log` (regressions).
