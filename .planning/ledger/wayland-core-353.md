---
issue: 353
repo: FerroxLabs/wayland-core
title: "A single anomalous usage report can trigger a spurious compaction: the Regression verdict needs corroboration before it moves the autocompact trigger"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "A Regression verdict requires corroboration before it may move the autocompact trigger, and the rule is stated where the verdict is computed"
    state: met
    evidence: "symbol:crates/wcore-config/src/context_window.rs::REGRESSION_CORROBORATION_OBSERVATIONS"
    owner: core
    note: "MERGED as 1839b9ad via 2165c30a. The rule is stated WHERE THE VERDICT IS COMPUTED - context_window.rs:325-341, not only in a comment: a Regression sets corroborated only once regressions >= 2, and served_window() returns the learned slot only when corroborated. A Shortfall carries its own corroboration inside one turn."
  - id: c2
    text: "A test shows a SINGLE anomalous usage report does not move the trigger"
    state: met
    evidence: "test:crates/wcore-config/tests/issue_1172_served_window_corpus_test.rs::a_single_regression_tells_the_user_but_does_not_yet_size_the_session"
    owner: core
    note: "The exact defect: one anomalous usage report reaches the notice but does NOT move the trigger."
  - id: c3
    text: "A test shows a genuine, repeated regression DOES still move the trigger, so the fix is not just disabling the tracker"
    state: met
    evidence: "test:crates/wcore-config/tests/issue_1172_served_window_corpus_test.rs::a_second_regression_corroborates_it_and_the_session_is_sized"
    owner: core
    note: "The required second direction, so the fix is not just disabling the tracker. Siblings a_shortfall_carries_its_own_corroboration and a_shortfall_corroborates_an_earlier_regression cover the other two ways corroboration is reached, and a_model_swap_discards_the_corroboration_too pins that a model swap resets it."
  - id: c4
    text: "The notice path keeps its current one-observation sensitivity; only the trigger needs corroboration"
    state: met
    evidence: "test:crates/wcore-config/tests/issue_1172_served_window_corpus_test.rs::a_reported_count_that_goes_backwards_is_detected"
    owner: core
    note: "The notice path keeps its one-observation sensitivity: detection still fires on a single backwards report. The corroboration gate sits only on served_window(), which is what sizes the session."
  - id: c5
    text: "A red arm is quoted verbatim"
    state: met
    evidence: "symbol:crates/wcore-config/src/context_window.rs::REGRESSION_CORROBORATION_OBSERVATIONS"
    owner: core
    note: "RED ARMS RUN AND QUOTED, hetzner-dsm 2026-08-29, both directions. Baseline: the corpus binary is 13/13 PASS. Mutation 1, REGRESSION_CORROBORATION_OBSERVATIONS 2 -> 1 (i.e. the pre-#353 behaviour where one anomalous report sizes the session). The mutated line was printed back after the edit, so the mutation landed on the `pub const` DECLARATION at context_window.rs:191 and not on the twelve lines of doc comment above it. Verbatim: `thread 'a_single_regression_tells_the_user_but_does_not_yet_size_the_session' (3333905) panicked at crates/wcore-config/tests/issue_1172_served_window_corpus_test.rs:250:5: / assertion left == right failed: one anomalous usage report must not be enough to compact the user's conversation (#353) / left: Some(4050) / right: None`. That is the ticket's defect, reproduced. Two siblings went red with it in the same run (a_second_regression_corroborates_it_and_the_session_is_sized, a_shortfall_corroborates_an_earlier_regression), 3 of 13. Mutation 2, the other direction, 2 -> 3, so corroboration is never reached: `thread 'a_second_regression_corroborates_it_and_the_session_is_sized' (3358595) panicked at crates/wcore-config/tests/issue_1172_served_window_corpus_test.rs:276:5: / assertion left == right failed: a repeated regression must still size the session, or the fix is just a disabled detector / left: None / right: Some(4050)`. So the corpus can distinguish the fix from a disabled detector, and the constant is load-bearing in both directions. context_window.rs was restored to a clean git diff."
---

`lane/finish-b` wired the learned served window into `autocompact_threshold_now`
and `should_autocompact_now`, which is what `#1172` asked for. The side effect is
that a tracker false-positive was upgraded from a spurious NOTICE to a spurious
COMPACTION, and a spurious compaction silently discards conversation context.

One anomalous usage report — a provider bug, an oddly-billed retry, a rewriting
proxy — is enough today. This ticket asks for corroboration on the `Regression`
verdict before it may size the session, with both directions tested so the fix is
not simply turning the tracker off.

Graded against `origin/integ/next` at `43848f75`. `lane/session-tickets` merged
in as `2165c30a` while this ledger pass was running, so the corroboration rule is
in the integration tree: `REGRESSION_CORROBORATION_OBSERVATIONS = 2`, applied
where the verdict is computed, with `served_window()` returning the learned slot
only once corroborated. Both directions are tested and the notice keeps its
one-observation sensitivity. The one thing still missing is a verbatim red arm.
