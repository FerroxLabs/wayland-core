---
issue: 1281
repo: FerroxLabs/wayland
kind: task
title: "[Maintainer] wayland#1218 c1 cannot be met as written: the literal clamp is a measured wrong refusal (128,000 -> 23,000)"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "The maintainer decides whether Q-1218 stands: either wayland#1218 c1 is amended to the property the product guarantees (est + ask <= the window in force) and closed, or the literal clamp is adopted and the truncation regression it causes for large catalogued models is owned with a plan"
    state: not-met
    evidence: "file:.planning/DECISIONS.md"
    owner: maintainer
    note: "Filed 2026-08-31 by the core lane during the 0.13.12 release gate, and ledgered the same day so the criterion it carries has a named owner rather than being blocked on nobody. wayland#1218 c1 reads that the max_tokens sent is never larger than the reserve the ceiling withheld. MEASURED at HEAD, that literal reading is a WRONG REFUSAL rather than a guard: at window 8,192 with an input estimate of 0 the ask is 8,192 - 0 - 512 = 7,680 while the withheld reserve is 3,139, so the ask exceeds the reserve at every input below the ceiling and that request still fits the window. RED ARM on hetzner-dsm, cargo check -p wcore-agent --tests RC=0 before the edit: replacing the final clamp with the literal reading (sized.min(room(window)).min(output_reserve + emergency_buffer)) reddens a_window_in_force_that_is_the_catalogued_one_changes_no_sizing with left: 23000 / right: 128000 -- claude-opus-4-7 cut from its real 128,000-token output ceiling to the 23,000-token compaction reserve on its own catalogued 200,000-token window, with no narrowing anywhere. That is every answer from every large model silently truncated, on every turn, with nothing in the product saying so. Restored, touched, tree clean, 32/32 green. DECISIONS.md Q-1218 already records the deviation (clamp to the room left in the window in force, not to the withheld reserve); what no lane can do is grade its own refusal green, which is why this is owner: maintainer and not core work. RECOMMENDATION, stated rather than left open: amend #1218 c1 to the property the product actually guarantees -- est + ask <= the window in force, which c2 and c3 already grade on the production path -- and close it. NOT IN QUESTION: #1218 c2, c3 and c4 are met at the production path, proven by mutating the production window derivation, under which the pre-existing pure-function tests stay GREEN -- their vacuity is measured, not asserted."
---

# A criterion whose literal wording is the regression

`wayland#1218` c1 asks for a clamp that, measured, cuts `claude-opus-4-7` from 128,000
tokens of output to 23,000 on its own catalogued 200,000-token window. The code
deliberately does something else, `DECISIONS.md` Q-1218 records why, and core cannot mark
its own refusal green. This issue carries that decision so c1 is blocked on a named owner
rather than on nobody.
