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

## Status

- [x] M0 instrument check
- [x] M1 premise re-verified (partially false — reported, not papered over)
- [x] M2 root cause located in source
- [x] M3 two blocking interactions found before coding
- [ ] fix
- [ ] four-quadrant control
