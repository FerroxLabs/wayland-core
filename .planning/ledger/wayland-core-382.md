---
issue: 382
repo: FerroxLabs/wayland-core
kind: defect
title: "The truncation notice tells the user core is sizing against the served window on the one turn when it is not"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The notice's wording is true when it is emitted: conditional on sizing_window() being Some and on supports_compaction(served), or it says plainly that corroboration is still pending"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D10, found while verifying wayland-core#353). Nothing has been done. The measured finding, verbatim: The truncation notice tells the user 'Core is now sizing this session against the {served}-token window this endpoint has actually demonstrated' — but after #353 that statement is FALSE on every first Regression, which is the only turn the notice ever fires for that arm. `observe()` returns evidence on the first backwards report (so `emit_info` runs, quoting served=4050 / 8192 / whatever), while `sizing_window()` is still None, so `narrow_to_served_window` returns the window unchanged and neither the autocompact trigger nor the pre-flight ceiling moves. Worse, the notice is emitted exactly ONCE per figure: the second regression that actually does corroborate hits the `if self.served_window == Some(served_window) { return None }` suppression at context_window.rs:355, so no second notice is sent. The user is told core is sizing against X at the one moment it isn't, and is never told when it starts. The same sentence is also false whenever `supports_compaction(served)` refuses the window (the #1179 gate, e.g. the 4,096 slot #1172 actually measured) — that half predates #353, but #353 widened it to the entire Regression arm."
  - id: c2
    text: "When corroboration lands and sizing actually moves, the user is told -- the once-per-figure suppression at context_window.rs:355 does not swallow the only truthful notice"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D10). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "A test asserts the notice TEXT against the sizing state in the same body, for both the first-regression and the corroborated case; shown RED against today's wording"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D10). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The truncation notice tells the user 'Core is now sizing this session against the {served}-token window this endpoint has actually demonstrated' — but after #353 that statement is FALSE on every first Regression, which is the only turn the notice ever fires for that arm. `observe()` returns evidence on the first backwards report (so `emit_info` runs, quoting served=4050 / 8192 / whatever), while `sizing_window()` is still None, so `narrow_to_served_window` returns the window unchanged and neither the autocompact trigger nor the pre-flight ceiling moves. Worse, the notice is emitted exactly ONCE per figure: the second regression that actually does corroborate hits the `if self.served_window == Some(served_window) { return None }` suppression at context_window.rs:355, so no second notice is sent. The user is told core is sizing against X at the one moment it isn't, and is never told when it starts. The same sentence is also false whenever `supports_compaction(served)` refuses the window (the #1179 gate, e.g. the 4,096 slot #1172 actually measured) — that half predates #353, but #353 widened it to the entire Regression arm.

**Where.** crates/wcore-agent/src/engine.rs:15742-15765 (the emit_info block), against crates/wcore-agent/src/engine.rs:8462-8477 (narrow_to_served_window -> sizing_window) and crates/wcore-config/src/context_window.rs:355 (once-per-figure notice suppression)

**Why it matters.** #1172's whole justification for the notice is that this is SILENT data loss and the operator must be told the one thing that fixes it. An operator who reads 'Core is now sizing this session against the 4,050-token window' will reasonably stop worrying and not raise num_ctx / set [compact] context_window — while core is in fact still running on the catalogued 128,000 and will keep overflowing. Both existing tests pin this behaviour without noticing: `a_single_regression_tells_the_user_but_does_not_yet_size_the_session` asserts the notice fires AND that sizing is None in the same test body, and the engine twin asserts the same pair. The fix is to make the notice's wording conditional on `sizing_window()` (and on `supports_compaction`), or to re-emit when corroboration lands.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
