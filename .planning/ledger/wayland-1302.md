---
issue: 1302
repo: FerroxLabs/wayland
kind: defect
title: "The credential-store timeout tells the operator to repair a keyring that is not broken - 104 reproductions with a healthy store"
status: open
last_verified_commit: 7b6c22852
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
    state: met
    evidence: "test:crates/wcore-agent/src/recovery_confidential.rs::what_the_store_reported_reaches_the_message"
    owner: core
    note: "The store's error is now CARRIED, as a bounded value, not passed through as text. ConfidentialStoreDiagnostic (wcore-config/src/confidential_blob.rs) is a STEP (select/lock/read/create/delete/decode/reference/load) plus a CLASS derived from the CredentialsError DISCRIMINANT ONLY -- keyring-error, io-error, credential-file-format, backend-unavailable, no-backend-error. It is set at every site that previously ran `map_err(|_| ...)`, travels on ConfidentialKeyStoreError (now a struct with kind() + diagnostic()), and reaches RecoveryConfidentialError::NoSecureBackendAvailable / SecureStoreUnreadable / Unavailable. The rung is named ONLY where the error identifies one: CredentialsError::Keyring has exactly one producer (the keyring backend, credentials.rs:441-462/734/3431), so keyring-error renders \"the OS keyring\"; backend-unavailable is produced by selection AND by more than one rung, so it is deliberately left unattributed -- an_ambiguous_backend_error_is_not_attributed_to_a_rung fails if that changes. MissingRecoveryKey stays payload-free on purpose: it has one producer (a get that returned Ok(None) at the read step), so its report is a constant and the sentence states it -- that is the HEALTHY-store half of this criterion. THREE SINKS ARE GRADED, not just Display: what_the_store_reported_reaches_the_message (Display, all three arms distinct), the_store_report_reaches_the_resume_refusal (recovery.rs locked_session_refusal, the string a refused --resume prints), and engine.rs a_refused_backend_selection_reaches_the_degrade_notice (emit_durability_degraded, the notice a normal turn shows -- driven end to end through a real AgentEngine with a selection-refusing key store). SECURITY CONTROL, and it is the reason this is a carry and not a passthrough: a_sensitive_sentinel_in_a_backend_error_never_reaches_the_operator and confidential_blob.rs a_backend_error_crosses_as_a_class_and_never_as_its_text embed a sentinel in the backend error and assert it reaches neither Display, nor Debug, nor the resume refusal, WHILE asserting the class and rung do -- an implementation that leaks nothing because it carries nothing fails the second half. FAIL-BEFORE / PASS-AFTER: M1 (91b2ce98e) made ConfidentialStoreDiagnostic::from_backend_error discard the error, re-creating the pre-fix erasure; 8 tests went red across all three surfaces (confidential_blob: a_backend_error_crosses_as_a_class_and_never_as_its_text, a_locked_store_and_a_healthy_one_render_differently, a_refused_write_reports_the_create_step, only_a_class_that_identifies_a_rung_names_one; recovery_confidential: what_the_store_reported_reaches_the_message, the_store_report_reaches_the_resume_refusal, an_ambiguous_backend_error_is_not_attributed_to_a_rung; engine: a_refused_backend_selection_reaches_the_degrade_notice) while the sentinel-leak assertions stayed GREEN, which is what proves that control grades leakage and not carriage. M2 (434b5d1c8) dropped the report at the notice sink only: the engine test went red alone and all 20 recovery_confidential tests stayed green, isolating the wiring from the value. Both mutants were verified present by grep before running and the tree restored to 7b6c22852 with zero diff after. LIMITS, stated: the class is a DISCRIMINANT, so two different keyring faults (locked vs wedged vs quota) are one code -- distinguishing them needs backend text, which is exactly what the sentinel control forbids. The require_durability = true refusal is NOT a store-report surface: it fires only for KeyStoreTimedOut, which by construction received no store report, and a timeout still carries none (a_timeout_still_reports_reach_and_invents_no_store_error asserts the string \"store-report\" is absent from both timeout arms). The store-answered causes refuse the turn on engine.rs's unconditional SessionAuthority arm, which interpolates Display and is therefore covered by the Display grade but has no engine-level test of its own. Nothing here was exercised against a real OS keyring; every arm is a fake backend."
  - id: c4
    text: "The 5s value is NOT raised as part of this."
    state: met
    evidence: "file:crates/wcore-agent/src/recovery_confidential.rs:325:pub(crate) const KEY_STORE_ACQUIRE_BUDGET: Duration = Duration::from_secs(5);"
    owner: core
    note: "RE-ANCHORED at 7b6c22852, not re-graded: the c3 plumbing added lines above it and moved the constant from 296 to 325 (`git diff 2a67a204f` shows no +/- on any from_secs line). Untouched, and the fix does not want it touched: raising the budget cannot help the starvation case at all -- a thread the host is not scheduling is not scheduled any sooner by waiting longer for it -- and it would trade the patience semantics the constant is deliberately tied to (STREAM_SILENCE_NOTICE_AFTER) for a slower version of the same wrong message. RESUME_KEY_WAIT_BUDGET is likewise unchanged at 30s. The scheduling half stays with wayland#1289."
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
