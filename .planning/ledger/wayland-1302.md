---
issue: 1302
repo: FerroxLabs/wayland
kind: defect
title: "The credential-store timeout tells the operator to repair a keyring that is not broken - 104 reproductions with a healthy store"
status: open
last_verified_commit: 6e4eca07
criteria:
  - id: c1
    text: "The timeout distinguishes 'the store answered slowly or refused' from 'the wait expired without the store being reached', and says which. Graded by a test that drives both and asserts the two messages differ."
    state: not-met
    owner: core
    note: "Filed 2026-09-03 out of the wayland#1289 measurement. KEY_STORE_ACQUIRE_BUDGET (recovery_confidential.rs:223) is a WALL-CLOCK deadline on a thread that does the work -- acquire_key runs the blocking store call on its own thread (:492) and waits rx.recv_timeout(budget) (:554) -- so expiry means EITHER the store is slow or locked OR the loader thread was never scheduled. The message asserts the first unconditionally."
  - id: c2
    text: "In the starvation case the message does NOT instruct the operator to unlock or repair the keyring, and does not offer disabling durable sessions as the remedy for it."
    state: not-met
    owner: core
    note: "MEASURED: the exact payload was produced 104 times against a HEALTHY store, purely from CPU oversubscription (19 of 20 runs at 192 threads on a 96-core box; 0 of 80 at nominal parallelism). Both remedies it names are wrong in that case -- one sends the operator to repair something that is not broken, the other gives up a feature to work around a transient scheduling condition."
  - id: c3
    text: "Whatever the store actually reported - the backend chosen, and its error if any - reaches the message. Today a healthy store and a locked one are indistinguishable in the output."
    state: not-met
    owner: core
    note: "On Linux both rungs are fast and their outcomes are known at the point of failure: the keyring rung fails in 0-4ms (no bus, or a locked `login` collection) and the fallback vault is local CPU, Argon2id measured at 142ms single-threaded and 1244ms worst case at 192-way. The product HAS this information and discards it, which is the same shape as the is_stale fail-closed arms in wayland#1300."
  - id: c4
    text: "The 5s value is NOT raised as part of this."
    state: not-met
    owner: core
    note: "Out of scope on purpose, and recorded so a later lane does not take the easy lever. recovery_confidential.rs:200-222 deliberately ties KEY_STORE_ACQUIRE_BUDGET to STREAM_SILENCE_NOTICE_AFTER; raising it changes user-facing patience semantics to paper over a test-host condition. The scheduling half belongs to wayland#1289."
---

# A true timeout with a false diagnosis

The wait really did expire. Everything the message says about WHY is a guess, and
in 104 measured reproductions the guess was wrong: the store was healthy and the
thread was simply never scheduled.

This is independent of whether the starvation itself is ever fixed. Even after
wayland#1289, a genuinely locked keyring and a starved runner will still produce
the same sentence, and only one of them is the operator's to repair.
