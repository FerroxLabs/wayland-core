# 24-CHANNEL-STARVATION — running notes

**Lane** `lane/channel-starvation` · **base** `2f5e6479` (`plan/f20-unified-audit-repair`,
the merge commit that landed `lane/channel-lease`).
Committed from minute ~14 per LANE-BRIEF §6b-i. Appended after every measurement.

---

## T+0 — instrument defect found before writing a line of code

**The `git` I was handed is not `git`.** A Claude Code hook rewrites bare `git …` to
`rtk git …` (a token-reducing proxy). On this repo it returned a *different history* from
the real binary:

| command | first line of `log --oneline` | `rev-parse HEAD` |
|---|---|---|
| `git log --oneline -3` (rtk-proxied) | `ee984efd docs(24-channel-lease): final report` | — |
| `git rev-parse HEAD` | — | `2f5e6479` |
| `/usr/bin/git log --oneline -3` | `2f5e6479 merge(channel-lease): …` | `2f5e6479` |

The proxy **silently dropped the merge commit** — i.e. it dropped exactly the commit that
carries the work I am extending. `command git` does NOT bypass it (the hook rewrites the
command *text*, so the `command` builtin is inside the rewritten string).

Consequence, and it is the §6b-ii shape: had I taken `git log` at face value I would have
computed `$BASE` from a commit that is not my base, and every fence diff and every
"what landed" claim in this report would have been wrong by one merge.

**Repair, in this lane:** every git measurement in this lane uses the absolute path
`/usr/bin/git`. Not documented-and-moved-on — the driver script resolves the real binary
and asserts it. Self-test with the third assertion pending (see §Instrument below).

---

## T+10 — what I have read, and the shape that follows

### Read

- `.planning/phases/24-…/24-CHANNEL-LEASE.md` — the landing lane's report. §7.1 and §7.2
  are the two residuals I own.
- `crates/wcore-agent/src/channel_lease.rs` — the whole module (331 lines).
- `crates/wcore-cron/src/lease.rs` — `ScheduleLease`, `attempt_named`, the `flock`/
  `LockFileEx` primitive, `Drop` = release.

### The four production facts that constrain the design

1. **The exclusion is `flock` on `<home>/channels/channel-poll.lock`.** `flock` is owned by
   the *open file description*. There is no preemption primitive: a second process cannot
   take a held `flock`, and the holder cannot be forced to drop it. Any "the service wins"
   rule therefore has to be a **voluntary yield by the holder**, not a seizure by the
   claimant.
2. **Ownership is decided exactly once, at boot,** at three sites:
   `bootstrap.rs:3278` (`"session"`), `cron.rs:463` (`"cron-daemon"`),
   `wcore-cli/gateway.rs:850` (`"gateway"`). Nothing re-attempts, ever. That single fact is
   both residual §7.1 (first-come) and residual §7.2 (an observer never re-attempts).
3. **The owner record already carries a self-reported `holder` string** (`LeaseRecord.holder`
   — `"gateway"` / `"session"` / `"cron-daemon"`), readable *without* the lock. So role is
   already on disk at the contention point. I do not need to invent a way to learn it.
4. **`ChannelManager` has both `start_all()` (manager.rs:161) and `stop_all()`
   (manager.rs:411), both async.** So a role change is expressible at runtime — an owner
   that yields can stop, an observer that acquires can start. Without `stop_all` this lane
   would have needed a new lifecycle concept; it does not.

### The design I am going to build (recorded BEFORE building it)

One mechanism, unchanged: **`flock` remains the sole proof of ownership.** Added on top is
an *advisory* claim, which cannot grant or deny anything:

- Each process publishes a **claim** (`role rank` + pid + refreshed timestamp) when it
  wants to poll but lost.
- The **owner** periodically reads claims. If a claim outranks it *strictly*, it
  `stop_all()`s, drops the lease, and becomes an observer.
- Every **observer** periodically re-attempts the lease. When it wins, it `start_all()`s.

Why this is not a second exclusion concept (the mistake that made the double-manager bug):
**the claim file cannot make anybody poll and cannot stop anybody polling.** It only causes
a lower-ranked *owner* to volunteer. If it is absent, stale, corrupt or unreadable, the
worst case is that the current owner keeps polling — i.e. exactly today's shipped
behaviour. It degrades toward first-come, never toward two pollers. That direction is the
whole safety argument and I will test it.

**Precedence** (to be cross-audited before it is final): `gateway` > `cron-daemon` >
`session`. Strict inequality only — equal ranks never preempt, which is what makes
oscillation impossible by construction (two sessions cannot ping-pong; a service cannot
ping-pong with another service).

### Open questions I have not answered yet

- Does `bootstrap.rs` hold a `ChannelManager` it can `stop_all()` on, or only a lease? (The
  lease is carried on `BootstrapResult`; the manager's ownership at that site is unread.)
- Is there a tokio runtime alive for the whole session lifetime at each of the three sites?
- Does `wcore-gateway`'s pre-existing **PID lock** (the thing that produced the landing
  lane's false CRITICAL, its §8.5) interfere with running a second gateway in my legs? It
  refuses a second gateway per home — so my "service starts second" leg must not be
  "start two gateways".

## Instrument (§6b-ii) — running tally

| # | defect | status |
|---|---|---|
| 1 | rtk `git` proxy hides a merge commit from `log` while `rev-parse` sees it | found T+0; repair = absolute `/usr/bin/git` everywhere; **self-test not yet written** |

---

## T+55 — panel result: 3/3 unanimous on all three tokens, and two of them corrected me

Raw votes in `24-CHANNEL-STARVATION-evidence/panel-{codex,gemini,kimi}.txt`, question in
`panel-question.txt`. Extracted **unanchored** and taking the **LAST** match (brief §4 — codex
repeats its final block, kimi indents and bullet-prefixes):

| panellist | precedence | mechanism | rank |
|---|---|---|---|
| codex `gpt-5.6-sol` | `service` | `yield` | `gateway>cron>session` |
| gemini `3.1-pro-preview` | `service` | `yield` | `gateway>cron>session` |
| kimi K3 | `service` | `yield` | `gateway>cron>session` |

**Invocation note (§4):** codex's first run returned **39 bytes** — `Reading additional input
from stdin...`. It had silently dropped its vote and would have read as a 2/3 majority. Re-run
with `< /dev/null`: 7188 bytes, vote present. This is the fourth distinct way this panel drops
a vote quietly, on top of the three the brief already lists.

### Two panellists found a defect in the design I had already written down

**Gemini — the stale claim can WEDGE, i.e. produce zero pollers.** My T+10 design said an
observer contends "only when no live claim outranks it". A dead high-rank claimant's file then
suppresses the ex-owner forever: it yielded, the claimant never takes the lock, nobody polls.
That is **the manufactured-denial failure the brief warns about, promoted from a test artifact
into a production bug.** I had not seen it.

**Codex — strict inequality does NOT prevent oscillation**, contrary to what I asserted in the
question. It prevents *equal-rank* ping-pong only. After a yield, the lower-ranked ex-owner can
reacquire before the higher-ranked claimant gets there, then yield again, indefinitely.

Both defects are fixed by the same single device, so the design gets **one** addition, not two:

> **The claim is suppressive only while it is FRESH.** An observer declines to contend while a
> strictly-higher-ranked claim exists whose file mtime is within `ttl = 3 x tick`. A live
> claimant refreshes its claim every tick, so it stays suppressive; a dead one stops refreshing,
> goes stale within `ttl`, and the ex-owner contends and wins.

- oscillation (codex): suppression while the claim is fresh stops the ex-owner racing the
  claimant at all — it does not contend, so it cannot win and re-yield.
- wedge (gemini): staleness bounds the no-poller window at `ttl`, ~6s by default.

**Where I take the minority position, and why.** All three panellists asked for a **pid-liveness
check** (`kill(pid,0)` / `OpenProcess`); codex went further and asked for pid + process-start
identity to defeat pid reuse. **I am not doing that.** `lease.rs` already argues, correctly, that
a recorded pid is not proof of anything. Freshness needs no process identity at all, so the pid
reuse hazard that all three then had to patch around simply does not arise. The pid stays in the
claim for the operator message and is never consulted for a decision. Their concern — that a
stale claim must not be honoured — is *met*; only the detector differs, and mine has no failure
mode of its own to reason about.

**Bounded gap is latency, not loss.** A window with no poller loses nothing: Telegram retains
updates until an `offset=` confirm, IMAP retains until `\Seen`. That is what makes a `ttl`-bounded
gap an acceptable price and a wedge unacceptable.

### The precedence rule, final

`gateway > cron-daemon > session`, strictly, preemptive-by-voluntary-yield.

**Justification (mine, not the panel's):** the rule has to encode *intent*, and arrival order
encodes nothing. A gateway exists only because the user installed a unit to be always-on; a
session exists because someone typed a command and will close the terminal. Ranking cron above
session for the same reason also preserves the one good property of first-come — a session
started while no service is running polls immediately, and cedes when one appears.

**Strongest argument against, stated because it is real** (codex and kimi both raised it): this
trades a *static, silent misallocation that never loses a message mid-stream* for a mechanism
whose handovers are each a window where a destructive read races a lease transition. My answer
is that the floor is preserved by construction — a claim cannot grant or deny ownership, only
`flock` can — so a bug in the claim layer degrades to first-come, never to two pollers. That
claim is load-bearing, so it gets tested rather than asserted.

---

## T+70 — a measurement trap in MY OWN instrument, found before it fired

Handover aborts the outgoing poll task (`ChannelManager::stop_all` → `handle.abort()`). The
fixture increments `open` when a `getUpdates` arrives and decrements after it answers, and it
**long-polls up to `--max-wait-ms` (2000ms)**. An aborted client request therefore stays counted
as OPEN in the fixture for up to 2s after the poller is gone. If the successor's first poll lands
in that window, `max_concurrent_getupdates` reads **2** with only ONE real poller.

That is a **false CRITICAL** — the exact failure the landing lane recorded as its instrument
defect #5, in a new costume. So:

- `max_open` is asserted in **steady-state windows only** (pre-handover, post-handover), each of
  which must read exactly 1;
- the transition window's transient is **measured and reported**, never hidden or silently
  excluded;
- `max_open == 0` in a steady-state window is graded `DENIAL`, a FAILURE, exactly as the landing
  lane graded it.

## Still to establish

- [x] precedence rule cross-audited (4-way panel) — 3/3 unanimous, recorded above
- [ ] the three call sites' runtime/manager ownership
- [ ] leg A: service owns when it starts SECOND (preemption)
- [ ] leg B: service owns when it starts FIRST (unchanged; must not regress)
- [ ] leg C: no message lost while the session is the observer — counted, delivered
- [ ] leg D: handover on holder exit, no operator action
- [ ] anti-denial: positive baseline in the same run; `max_open == 0` graded DENIAL
- [ ] `wcore-agent --lib` serial run (parallel cluster is NOT mine — brief §6)
