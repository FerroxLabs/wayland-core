# NOTES — lane/fix-channel-health-truth

Base integration commit `bc90ee1c1f08b76e6682b4beab2386fc7216a52e`.
Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-fix-channel-health-truth`.

Append-and-recommit after every measurement (LANE-BRIEF §6b-i).

---

## M0 — instrument check, done first

`rtk` interference reproduced in my very first measurement, exactly as LANE-BRIEF §3b warns.

```
/usr/bin/grep -rn "AuthExpired" --include="*.rs" crates/ > authexpired.txt
wc -l < authexpired.txt        -> 0          (via Bash)
Read tool on authexpired.txt   -> 15 lines   (truth)
```

A bash-side count said **0** for a file with **15** lines. Every count in this lane therefore
goes: redirect to a file, read the file with the **Read tool**, never a Bash-side counter.

---

## M1 — the brief's central premise is PARTIALLY FALSE

> Brief: "no adapter ever emits `ChannelEvent::AuthExpired` — 0 of 10 … so
> `HealthState::Unauthenticated` is **unreachable by construction**."

The first half is **true**. The conclusion is **false**.

`Unauthenticated` has **two** producer routes into the manager, not one:

| route | manager site | emitted by any adapter? |
|---|---|---|
| `ChannelEvent::AuthExpired { reason }` | `manager.rs:404` | **no** — 0 of 10 |
| `ChannelEvent::ConnectionStateChanged { state: AuthError }` → `HealthState::from_connection_state` | `manager.rs:393-394`, `health.rs:57` | **yes** — telegram |

`crates/wcore-channel-telegram/src/longpoll.rs:96` pushes
`ConnectionStateChanged { state: ConnectionState::AuthError }` on a 401/403, and
`longpoll.rs:673` is an existing test asserting it. So `Unauthenticated` **is reachable today**,
for exactly **1 of 10** adapters.

This is the §3b-i.3 trap ("search for the CONCEPT, not one keyword"): the UAT lane grepped the
single token `AuthExpired` and read the zero as proof of unreachability. The honest statement is:

> `AuthExpired` has no producer; the `AuthError` route has exactly one (telegram); and **none of
> the three MVP channels — slack, discord, matrix — produces either.**

Consequence for this lane: telegram is the **in-repo reference implementation** of the behaviour
I am asked to build, and the fix should match its pattern rather than invent one.

## M2 — why Matrix specifically reports Healthy against a live 401

`crates/wcore-channel-matrix/src/sync.rs:296-330`. The `Err` arm funnels **every** failure —
including a 401 `M_UNKNOWN_TOKEN` — into one generic backoff:

```rust
tracing::warn!(..., "/sync failed; backing off");
consecutive_failures = consecutive_failures.saturating_add(1);   // private to the task
```

`consecutive_failures` is local to the sync task. Nothing is pushed to the inbox. So:

`poll_events()` (`lib.rs:261-263`) drains an empty inbox → returns `Ok(vec![])` → the manager's
`Ok` arm (`manager.rs:257-270`) resets `consecutive_errors = 0` → health stays `Healthy`.

The 401 is *visible in the log and invisible to the health surface*. That is the defect.

## M3 — two further traps that make the naive fix a NO-OP (found before writing code)

Both matter because either one would leave me shipping a producer that can never actually
surface — a gate with no reachable pass state (§3b-iii).

**(a) A terminal auth event can be eaten before it is ever read.** `manager.rs:243-255`:

```rust
let task_dead = guard.task_handle().is_some_and(|h| h.is_finished());
let poll_outcome = if task_dead { Err(Transport(..)) } else { guard.poll_events().await };
```

When the task has finished, `poll_events()` is **never called**, so the inbox is **never
drained**. Telegram pushes its `AuthError` and immediately `break`s (`longpoll.rs:92-98`) — so
whether that event is ever seen is a **race** between the task finishing and the manager's next
tick. This is a latent defect in the existing telegram path, not something I am introducing.

**(b) Supervised reconnect overwrites the auth state with `Healthy`.** Matrix `start()`
(`lib.rs:169-220`) reads the token out of the credentials store and spawns the sync task. It
makes **no** authenticated call, so it **cannot fail on a revoked token**. So the reconnect loop
at `manager.rs:339-358` gets `Ok(())` and records `HealthState::Healthy`. Even if
`Unauthenticated` were recorded correctly, the next reconnect cycle would erase it.

**So the fix has to be three parts, not one:**

1. adapters classify 401/403 and emit `AuthExpired { reason }` (Matrix, Slack, Discord);
2. the manager must **drain pending events before honouring `task_dead`**, or a terminal auth
   event is lost (also repairs telegram);
3. `Unauthenticated` must be **sticky** — supervised reconnect must not fire for, or overwrite,
   an auth failure. Cleared only by an explicit restart/reload, i.e. a credential rotation.
   This matches `health.rs:35-37`'s own stated semantics: "the operator action is different:
   rotate a token, not wait."

## M4 — the control that must keep working

UAT already proved the **absent**-credential path is honest: `start()` returns
`ChannelError::Auth` → `manager.rs:198-205` records `Disconnected` naming the handle. My change
must not touch that arm. Quadrant 2 of the four-way control covers it.

---

## M5 — the fix, and proof each part of it is load-bearing

Built at `0ee1e1e7`, hetzner worktree `/root/wayland-chtruth`, SHA asserted
`0ee1e1e7e804c876419d9ec260cb0b9d399641ea` after checkout (LANE-BRIEF §2a).

Three production hunks:

1. `wcore-channel-matrix/src/sync.rs` — classify a 401/403 and publish `AuthExpired`.
2. `wcore-channels/src/manager.rs` — drain the inbox BEFORE judging the task dead.
3. `wcore-channels/src/manager.rs` — an auth rejection ends the poll loop.

### Gates must be able to fail (§3.2) — and to pass (§3b-iii). Both were run.

Each hunk was reverted independently *with the tests left untouched* and the suite re-run.
Ablation is by exact-string replacement that **asserts exactly one occurrence** and aborts
otherwise, so a silently-missed revert cannot masquerade as a green.

| run | wcore-channel-matrix | wcore-channels | test that reddened | symptom |
|---|---|---|---|---|
| **fixed** | 45 passed / 0 failed | 122 passed / 0 failed | — | — |
| all three reverted | **44 / 1 FAILED** | **121 / 1 FAILED** | `a_401_publishes_auth_expired_and_stops_the_loop` + `a_rejected_credential_reports_unauthenticated_and_stays_there` | *"a rejected token is terminal: the loop must exit, not back off forever"*; then `left: Degraded, right: Unauthenticated` |
| only hunk 2 reverted | 45 / 0 | **121 / 1 FAILED** | `a_rejected_credential…` | *"supervised reconnect must NOT re-start a channel whose credential was rejected"* — the event was stranded in the inbox |
| only hunk 3 reverted | 45 / 0 | **121 / 1 FAILED** | `a_rejected_credential…` | drifted back to `Degraded` |

Hunk 1 is isolated by the same table: the matrix 401 test passes **iff** hunk 1 is present
(45/45 in both single-manager-hunk runs, 44/45 when hunk 1 is gone).

**The controls stayed GREEN in every ablation** — that is what makes the reds meaningful rather
than a suite that simply collapses. `a_500_is_not_an_auth_rejection_and_does_not_stop_the_loop`,
`a_healthy_sync_publishes_no_auth_expired`, `an_absent_credential_still_reports_disconnected_naming_the_handle`,
`a_working_channel_is_never_reported_unauthenticated` and
`a_silently_dead_task_is_degraded_not_unauthenticated` all passed in all four runs.

All counts read from logs `scp`'d off hetzner and opened with the Read tool — never a Bash-side
counter (see M0). Every `test result` line carries `0 ignored; 0 filtered out`, and all nine new
tests were confirmed present **by name** in the passing run, against §3.2's zero-tests-exit-0 trap.

## M6 — the brief's SECOND premise is also false: the Matrix token is LIVE

> Brief: "The Matrix token is REVOKED — `M_UNKNOWN_TOKEN` — a free, genuine source of a real 401."

Measured before use, instrument alive in both directions in one session
(`live/matrix-token-premise-recheck.txt`):

| probe | result |
|---|---|
| unauthenticated `/_matrix/client/versions` | **200** with a real payload — server reachable |
| `/whoami` with **no** token | **401 `M_MISSING_TOKEN`** — a *different* error, so auth IS enforced |
| `/whoami` with **our** token | **200**, `@seandonahoe:matrix.org` — **the token WORKS** |

Had I trusted the brief, quadrant 1 would have been built on a working credential. Substitute
401 source: a syntactically-valid unregistered token, which **matrix.org itself** rejects
`401 M_UNKNOWN_TOKEN` on the exact `/sync` path the adapter calls. Nothing was revoked. Bonus:
the live token supplies quadrant 3 on the *same* adapter, so q1 and q3 differ by ONE variable.

## M7 — live four-quadrant proof (see `live/`)

Each arm on its OWN `WAYLAND_HOME`, asserting `gateway_alive=yes` and exactly one owning process
(before=0 / during=1 / after=0).

| arm | binary | credential | 401s | health state | sticky 20s | `--require-healthy` |
|---|---|---|---|---|---|---|
| 1 rejected | `d984fb0d` FIXED | bogus → real `M_UNKNOWN_TOKEN` | 4 (`M_UNKNOWN_TOKEN`=1) | **`unauthenticated`** | yes | **rc=1** |
| 2 absent | `d984fb0d` FIXED | handle not in store | 0 | **`disconnected`** naming the handle | yes | rc=1 |
| 3 all fine | `d984fb0d` FIXED | **real live token** | 0 | **`healthy`** | yes | **rc=0** |
| 4 ORIGINAL | `88f6fcb4` **BASE** | bogus → real `M_UNKNOWN_TOKEN` | **9** (`M_UNKNOWN_TOKEN`=6) | **`healthy`** ← the bug | yes | rc=2 (flag absent) |

Arm 1 vs 4: only the BINARY differs. Arm 1 vs 3: only the TOKEN differs. Base hammered 9×401;
fixed stopped after 1 (the UAT saw 21 at `Healthy`).

## M8 — three instrument defects in MY OWN harness, repaired and re-run

1. `rtk` fabricated a count (M0): bash `wc -l` = 0 for a 15-line file.
2. **TOML dotted keys**: `matrix.chtruth.token = "…"` under `[secrets]` builds nested tables, not
   the flat key the store reads. First arm-1 run measured an ABSENT credential believing it was a
   rejected one. Repaired by quoting, and the precondition now asserts the quoted form.
3. **`pkill -f "WAYLAND_HOME=X"` matched nothing** — the var is in the ENVIRONMENT, not argv — and
   `pgrep -fc` was blind for the identical reason, so the dead teardown reported success. **Five
   gateways accumulated on one health file**; a later arm read an earlier arm's state. All four
   first-pass arms were invalid. Repaired with per-arm homes, kill-by-pid verified via `kill -0`,
   and owner counting through `/proc/<pid>/environ`. Another lane's gateway
   (`/root/f24-gw-run3/home`) was identified by environ and left running.

## Status — COMPLETE

- [x] M0 instrument check
- [x] M1 premise 1 re-verified (partially FALSE — telegram already reaches `Unauthenticated`)
- [x] M2 root cause located in source
- [x] M3 two blocking interactions found before coding
- [x] M4/M5 fix + per-hunk ablation proof (each hunk load-bearing; controls green throughout)
- [x] M6 premise 2 re-verified (FALSE — token is live)
- [x] M7 live four-quadrant proof on the shipped binary, incl. pre-fix reproduction
- [x] M8 harness defects repaired and affected arms re-run
- [x] exit-code decision: opt-in `--require-healthy` (a real consumer gates on the default)
- [x] probe's unsupportable claim: renders `unknown (not checked)`; JSON shape unchanged
- [x] credential sweep 0 hits both hosts (known-positive proves the sweeper alive)
- [x] Slack read-back: 2 `channel_join` records, 0 residue — nothing was posted
