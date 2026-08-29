---
issue: 1199
repo: FerroxLabs/wayland
kind: defect
title: "The skills prompt budget still sizes against the fabricated 200,000-token window for exactly the unlisted-model case #1150 c1 removed"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "get_char_budget(None) no longer grants a budget derived from 200k: the unknown-window case is sized against UNVERIFIED_CONTEXT_WINDOW or refuses to guess, the rule compact.rs:449-454 already states"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D16, found while verifying wayland#1150 — [Bug]: Absurd Input Token Size). Nothing has been done. The measured finding, verbatim: The fabricated 200,000-token window that #1150 c1 removed still governs the skills prompt budget for exactly the unlisted-model case c1 is about. `wcore_skills::prompt::get_char_budget(None)` returns `DEFAULT_CHAR_BUDGET = 8_000`, whose own source comment reads `// Fallback: 1% of 200k × 4`. For an unlisted model with no `[compact] context_window` — the reporter's exact configuration — `known_context_window` correctly returns `None`, the bootstrap passes that `None` straight through (bootstrap.rs:2326-2329), and the skills listing is sized against 200k while every other boundary in the session is sized against UNVERIFIED_CONTEXT_WINDOW = 32,768. Measured arithmetic: 1% × 32,768 tokens × 4 chars = 1,310 chars intended, 8,000 chars actually granted — 6.1x, i.e. ~2,000 tokens of a 32,768-token window spent on the skill listing. The lane knew this number: bootstrap.rs:2320-2325 says in its own words 'every real session listed skills against the flat 8,000-character default: a 32k model spent 6x its fair share of the window on the listing'. They fixed the wiring for the known-window case and left the unknown-window case on the old constant. The guard test cannot catch it: `the_bootstrap_prompt_uses_the_real_window_derived_skill_budget` (issue_1150_unknown_context_window_test.rs:227) boots BOTH arms with an explicit window — `Some(1_000_000)` and `Some(2_000)` — so the `None` path is untested, even though the test's own failure message is about sessions being 'on the flat 8,000-char default'. c1's doc comment at compact.rs:449-454 names 'the skills prompt budget' as one of the callers that 'must not act on a guess'; the guess simply moved downstream into get_char_budget's None arm."
  - id: c2
    text: "A test boots the bootstrap prompt with known_context_window = None and asserts the skills budget; shown RED against today's DEFAULT_CHAR_BUDGET"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D16). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "The measured arithmetic here is reproduced after the change: an unlisted model's skill listing no longer receives 8,000 chars while every other boundary is sized against 32,768"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D16). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The fabricated 200,000-token window that #1150 c1 removed still governs the skills prompt budget for exactly the unlisted-model case c1 is about. `wcore_skills::prompt::get_char_budget(None)` returns `DEFAULT_CHAR_BUDGET = 8_000`, whose own source comment reads `// Fallback: 1% of 200k × 4`. For an unlisted model with no `[compact] context_window` — the reporter's exact configuration — `known_context_window` correctly returns `None`, the bootstrap passes that `None` straight through (bootstrap.rs:2326-2329), and the skills listing is sized against 200k while every other boundary in the session is sized against UNVERIFIED_CONTEXT_WINDOW = 32,768. Measured arithmetic: 1% × 32,768 tokens × 4 chars = 1,310 chars intended, 8,000 chars actually granted — 6.1x, i.e. ~2,000 tokens of a 32,768-token window spent on the skill listing. The lane knew this number: bootstrap.rs:2320-2325 says in its own words 'every real session listed skills against the flat 8,000-character default: a 32k model spent 6x its fair share of the window on the listing'. They fixed the wiring for the known-window case and left the unknown-window case on the old constant. The guard test cannot catch it: `the_bootstrap_prompt_uses_the_real_window_derived_skill_budget` (issue_1150_unknown_context_window_test.rs:227) boots BOTH arms with an explicit window — `Some(1_000_000)` and `Some(2_000)` — so the `None` path is untested, even though the test's own failure message is about sessions being 'on the flat 8,000-char default'. c1's doc comment at compact.rs:449-454 names 'the skills prompt budget' as one of the callers that 'must not act on a guess'; the guess simply moved downstream into get_char_budget's None arm.

**Where.** crates/wcore-skills/src/prompt.rs:9 and :20 (DEFAULT_CHAR_BUDGET / get_char_budget None arm); consumers at crates/wcore-agent/src/bootstrap.rs:2326 and crates/wcore-agent/src/late_mcp.rs:132; untested path guarded by crates/wcore-agent/tests/issue_1150_unknown_context_window_test.rs:227

**Why it matters.** It is the same defect class #1150 was filed for, at the same reporter configuration, in a site c1's own documentation claims to have covered. It also silently blunts the fix: the session correctly tells the user 'compaction falls back to a conservative 32,768-token assumption' while one prompt section keeps spending as if the window were 200,000.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
