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
    state: not-met
    owner: core
    note: "The corpus tests are written as forward assertions; no verbatim red arm from before 1839b9ad is quoted in the commit body or in the tree. The behaviour that PROVOKED the ticket - cache_ledger_engine_test going red on an impossible fixture - is real and recorded on the issue, but it is not the same thing as a quoted red arm for this fix."
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
