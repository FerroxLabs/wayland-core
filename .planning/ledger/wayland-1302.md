---
issue: 1302
repo: FerroxLabs/wayland
kind: defect
title: "The credential-store timeout tells the operator to repair a keyring that is not broken - 104 reproductions with a healthy store"
status: open
last_verified_commit: cdf6f4d03
criteria:
  - id: c1
    text: "The timeout distinguishes 'the store answered slowly or refused' from 'the wait expired without the store being reached', and says which. Graded by a test that drives both and asserts the two messages differ."
    state: met
    evidence: "test:crates/wcore-agent/src/recovery_confidential.rs::both_key_store_timeout_causes_are_told_apart"
    owner: core
    note: "The discriminant is OBSERVED, not inferred: the loader closure sets an AtomicBool as it enters the store call, on the loader thread and never on the spawning side, so a load this host never ran cannot set it. KeyStoreTimedOut carries that reach (KeyStoreReach::Asked | NeverAsked) plus the configured backend, and key_store_timeout_message is the single place either is worded. The test drives BOTH arms concurrently through the production trait method preflight -- a wedged store double for Asked, a starved one for NeverAsked -- asserts the two rendered messages differ, and asserts each reach was recorded. RED ARM at dafe3c056, which adds the observation and the tests but deliberately leaves the message unchanged: the two arms render byte-identical and the assert_ne fails."
  - id: c2
    text: "In the starvation case the message does NOT instruct the operator to unlock or repair the keyring, and does not offer disabling durable sessions as the remedy for it."
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::a_starved_key_load_does_not_tell_the_user_to_repair_the_keyring"
    owner: core
    note: "Graded on the surface a person actually reads, not on the Display alone: the notice travels on OutputSink::emit_durability_degraded, since with RUST_LOG unset the tracing::warn! beside it reaches nobody. The test asserts the notice contains neither 'repair' nor 'unlock' and the unit test asserts the Display carries no 'enabled = false'. The hard-refusal path is graded separately by require_durability_refusal_names_starvation_not_a_broken_keyring, which asserts the same two absences on the SessionAuthority string the operator gets instead of a turn. CONTROL, and it is the reason this is not a wording softener: a_wedged_key_store_still_lets_the_turn_reach_the_provider_and_says_so and both_key_store_timeout_causes_are_told_apart both assert that a store which WAS asked and stayed silent still gets 'Unlock or repair'."
  - id: c3
    text: "Whatever the store actually reported - the backend chosen, and its error if any - reaches the message. Today a healthy store and a locked one are indistinguishable in the output."
    state: not-met
    owner: core
    note: "HALF LANDED, AND THE HALF THAT DID NOT IS NAMED. The backend chosen now reaches the timeout message -- configured_backend_label renders storage.credentials.backend, which is the operator's own cleartext config, and both timeout arms assert it is present -- and the NeverAsked arm states positively that the store reported nothing. What is still discarded is the store's own error: load_key_from_configured_store collapses ConfidentialKeyStoreError into four variants (recovery_confidential.rs, the map_err after the store opens), so which rung failed and what it said never reaches SecureStoreUnreadable, MissingRecoveryKey or Unavailable. That is a live wire into wcore-config's confidential-blob surface and is NOT in the minimal cut for the false diagnosis; grading it met on the backend half alone would be a fabricated pass."
  - id: c4
    text: "The 5s value is NOT raised as part of this."
    state: met
    evidence: "file:crates/wcore-agent/src/recovery_confidential.rs:296:pub(crate) const KEY_STORE_ACQUIRE_BUDGET: Duration = Duration::from_secs(5);"
    owner: core
    note: "Untouched, and the fix does not want it touched: raising the budget cannot help the starvation case at all -- a thread the host is not scheduling is not scheduled any sooner by waiting longer for it -- and it would trade the patience semantics the constant is deliberately tied to (STREAM_SILENCE_NOTICE_AFTER) for a slower version of the same wrong message. RESUME_KEY_WAIT_BUDGET is likewise unchanged at 30s. The scheduling half stays with wayland#1289."
---

# A true timeout with a false diagnosis

The wait really did expire. Everything the message said about WHY was a guess,
and in 104 measured reproductions the guess was wrong: the store was healthy
and the thread was simply never scheduled.

## What the product now knows, and where it says it

The budget is a wall-clock deadline on a thread that does the work, so its
expiry had two causes that nothing distinguished. It does now: the loader
closure marks a flag as it enters the store call, and the waiter reads that
flag exactly when the load has NOT returned — which is the only moment the
answer is needed and the only reason it cannot be a return value.

Three surfaces render the verdict and all three were wrong for the starved
case, so all three are graded:

* the error's own `Display`, which is what a refused resume shows
  (`locked_session_refusal`);
* the degrade notice, `emit_durability_degraded` — the one a normal turn puts
  in front of a user, and the one a `tracing::warn!` could never have reached
  them on;
* the `require_durability = true` refusal, which is why this was a blocker
  rather than a nuisance: there the false diagnosis costs the turn as well.

## The reverse error, held open on purpose

A genuinely wedged or locked store must still be sent to. `KeyStoreReach::Asked`
keeps the repair remedy verbatim, and two tests fail if it ever stops doing so.
A fix that never says "repair the keyring" would be exactly as wrong as one that
always does.

This remains independent of whether the starvation itself is ever fixed. Even
after wayland#1289, a genuinely locked keyring and a starved runner still arrive
at the same code path — they just no longer arrive at the same sentence.
