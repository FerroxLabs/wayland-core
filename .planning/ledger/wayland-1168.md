---
issue: 1168
repo: FerroxLabs/wayland
kind: defect
title: "Turn-1 transient injection poisons the prompt-cache prefix: both fixes (system-prefix date / trailing transient message) have measured collisions"
status: closed
last_verified_commit: 894ace48d
criteria:
  - id: c1
    text: "The turn-1 transient no longer lands at messages[1]; it moves into the cached system prefix"
    state: met
    evidence: "commit:6762f218"
    owner: core
    note: "Option A, landed on lane/w559; measured hit ratio 0.0358 -> 0.6526, monotonic, no late collapse"
  - id: c2
    text: "The positive control was re-anchored rather than deleted, so it still fails if the peel stops discriminating"
    state: met
    evidence: "test:crates/wcore-agent/tests/untrusted_channel_wire_test.rs::the_peel_removes_named_runtime_blocks_and_stops_on_anything_else"
    owner: core
    note: "retains a catch_unwind negative arm"
  - id: c3
    text: "The skill-router hint and PrePrompt hook contributions no longer land at messages[1] on turn 1"
    state: superseded
    successor: FerroxLabs/wayland#559
    owner: core
    handoff: "FerroxLabs/wayland#559"
    note: "same shape as the 26-byte date that dominated the measured collapse, much smaller, not closed by this change. Carried as a criterion on #559, which is the open ticket that owns the cache-prefix outcome REFUTED 2026-08-29 by the 0.13.12 close-sweep, recorded verbatim: The MECHANICAL condition holds; the SUBSTANTIVE handover does not. Mechanically: c3's note names #559, `gh issue view 559` returns state OPEN, and `scripts/check-criteria-ledger.py:387-392` only requires that the note match `#(/d+)` and that the issue exist and be open — so the gate is satisfied. Substantively the residual is closed nowhere. #559's c6 carries the criterion text VERBATIM ('The skill-router hint and PrePrompt hook contributions no longer land at messages[1] on turn 1') and is already graded `met` — against a different property, which its own note states openly: 'Closed as a cache-boundary fact, not a positional one, and the difference is deliberate... The transient still sits inside the turn-1 user message.' I confirmed the substituted property is real code, not vapour (`wcore-observability/src/cache.rs:62` stamps MessageCacheHint::Transient, `wcore-providers/src/anthropic.rs:312` declares request_has_transient_tail, and the three prompt_cache_prefix_test.rs tests it cites are declared at :280/:300/:351), so the harm IS mitigated on explicit-breakpoint providers — but the criterion as written is false and is marked met. That defeats the exact safeguard #1168's closing note relied on ('the gate refuses a superseded whose successor does not exist or is already closed', and #559's ledger adds 'the gate refuses that handover if this ticket ever closes with c6 unmet'): c6 says met, so nothing will refuse. The gate cannot catch it either — its own docstring says 'It does not judge whether a criterion is a GOOD criterion.' Remedy belongs on #559, not #1168: re-grade c6 as not-met against its own text, or rewrite its text to the boundary property it actually closed. State kept `superseded` because #559 exists and is open, which is all the gate checks. REPAIRED 2026-08-30 where the remedy belongs, on FerroxLabs/wayland#559: c6 is re-graded `not-met` against its own sentence, and the boundary property that WAS delivered and measured is split out as a new #559 c7 with the evidence, controls and red arm carried over intact -- so the delivered work is still graded, and the positional residual this note hands over is live and refusable again. NOTE THE GATE`S LIMIT, which is its own documented non-goal: scripts/check-criteria-ledger.py checks that a successor issue EXISTS and is open, not that the successor CRITERION is still unmet. A future substitution inside the successor is caught by a human re-grade, not by the script. HANDOFF LINE ADDED 2026-08-30 after the partials verifier pointed out it was missing: the successor was named in prose only, and a target that lives in prose is not machine-checkable. `handoff: FerroxLabs/wayland#559` now carries it, matching the convention already in use across the ledger directory. LABEL CORRECTED IN THE SAME PASS, because the lane`s own summary overstated this: it read `DECOMPOSED, core`s half CLOSED as a new wayland#559 c7`. That is wrong and the verifier is right to reject it. #559 c7 is a DIFFERENT property (`A turn-1 transient is never a cache WRITE point`), it was delivered BEFORE this lane, and its evidence (prompt_cache_prefix_test.rs:280) predates this work. NOTHING in c3`s own sentence was closed: `Self::attach_transient_block(last, hint)` and `apply_pre_prompt_contribution` both still write into `request.messages.last_mut()`, which on turn 1 IS messages[1]. What this lane actually did was re-grade #559 c6 from `met` to `not-met` against c3`s verbatim sentence, and split the delivered boundary property out as c7 so it is still graded somewhere -- which makes the residual live and refusable again instead of silently absorbed. That is bookkeeping repair, not closure. The positional residual remains OPEN on #559 c6."

---

Closed in v0.13.10 with a residual stated out loud.

A transient injected on turn 1 sat at `messages[1]`, byte 0 — inside every
provider's cached prefix — so the prefix changed on every single turn and the
prompt cache never warmed. Moving `Current date:` into the cached system
prefix took the measured hit ratio from 0.0358 to 0.6526.

Criterion c3 is `superseded`, not `met`: the same shape survives for the
skill-router hint and PrePrompt hooks. It is far smaller than the date that
dominated the collapse, and it is not closed. It is carried as a live
criterion on #559 rather than left in prose, because a residual nobody can
find is a residual nobody fixes — and the gate refuses a `superseded` whose
successor does not exist or is already closed.

That last sentence was doing less work than it looked. It is true, and it
passed, while the successor criterion on #559 sat graded `met` against a
substituted property — so nothing would ever have refused this handover. Fixed
on 2026-08-29 where the remedy belongs, on #559: c6 is `not-met` against its
own text and the boundary property that WAS delivered is graded separately as
c7. The residual is live again. The gate still cannot see a substitution; only
a re-grade can.
