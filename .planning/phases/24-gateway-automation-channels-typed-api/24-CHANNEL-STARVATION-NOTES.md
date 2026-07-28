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

## Still to establish

- [ ] precedence rule cross-audited (4-way panel)
- [ ] the three call sites' runtime/manager ownership
- [ ] leg A: service owns when it starts SECOND (preemption)
- [ ] leg B: service owns when it starts FIRST (unchanged; must not regress)
- [ ] leg C: no message lost while the session is the observer — counted, delivered
- [ ] leg D: handover on holder exit, no operator action
- [ ] anti-denial: positive baseline in the same run; `max_open == 0` graded DENIAL
- [ ] `wcore-agent --lib` serial run (parallel cluster is NOT mine — brief §6)
