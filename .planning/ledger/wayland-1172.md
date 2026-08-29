---
issue: 1172
repo: FerroxLabs/wayland
kind: defect
title: "Core cannot see a self-hosted endpoint's served context window: stock Ollama silently discards the system prompt while core reports 6% pressure"
status: open
last_verified_commit: e7144c30a
criteria:
  - id: c1
    text: "Core learns the window an endpoint actually serves, from the token counts already in its responses"
    state: met
    evidence: "symbol:crates/wcore-config/src/context_window.rs::ServedWindowTracker"
    owner: core
    note: "no probing and no address sniffing — the signal was already in the bytes we receive"
  - id: c2
    text: "The shortfall is named to the user, and says the HEAD of the prompt is what was lost"
    state: met
    evidence: "file:crates/wcore-agent/src/engine.rs:15348"
    owner: core
    note: "Upgraded from the corpus test, which evidences DETECTION rather than the user-facing notice this criterion is about. engine.rs:15348-15351 is the emit_info site that names the shortfall and says the HEAD of the prompt is what was lost. SOFT SPOT: no test asserts the notice STRING - grep for the phrase returns the production site only."
  - id: c3
    text: "COMPENSATION: the learned window feeds the pre-flight guard and autocompact, so the truncation stops"
    state: not-met
    evidence: "test:crates/wcore-agent/src/engine.rs::a_learned_served_window_narrows_the_preflight_window_when_it_is_workable"
    owner: core
    handoff: "FerroxLabs/wayland#1230"
    note: "DECOMPOSED 2026-08-29 at e7144c30a. This criterion is two clauses and only the first holds; it was marked met on the first, which is the substituted-property failure. CLAUSE 1 -- the learned window feeds the pre-flight guard and autocompact -- IS MET, and is now red-armed rather than merely asserted. narrow_to_served_window (engine.rs:8462) is the single chokepoint reached by the #255 guard (:13243), the length-finish check (:15089), autocompact_threshold_now (:8492) and smart_compact_fraction (:8554). Baseline on hetzner-dsm: 5 tests run, 5 passed. RED ARM A, `Some(window.map_or(served, |w| w.min(served)))` -> `window` at engine.rs:8476 (the mutated line printed back, so it landed on the return expression, not a comment), touched, rebuilt: `Summary 5 tests run: 3 passed, 2 failed`, verbatim `panicked at crates/wcore-agent/src/engine.rs:27396:9: assertion left == right failed: an observed served window outranks the catalogue` and `:27433:9: assertion left == right failed: the trigger must follow the window the endpoint actually serves`. The two too-small-window tests stayed GREEN under that mutation, which is the correct polarity. Restored with git checkout -- + touch, tree clean, baseline back to 5/5. CLAUSE 2 -- so the truncation stops -- IS FALSE in the configuration this ticket reports, and is not reachable by the wiring clause 1 describes. narrow_to_served_window returns the window unchanged unless CompactConfig::supports_compaction(served) (compact.rs:608). At 4,096 with MAX_RESERVE_FRACTION 0.55: scaled reserves output 1,365 / autocompact 887 / emergency 204, so autocompact_threshold_for_window = 1,844 and input_ceiling_for_window = 2,527, both below BASELINE_TURN_TOKENS = 3,118 -- so the learned 4,095 is deliberately not narrowed onto and core keeps sizing against UNVERIFIED_CONTEXT_WINDOW = 32,768. Separating the two decisions the one predicate answers does NOT rescue clause 2: every consumer of the answer either aborts the turn (the #255 guard, the length-finish check) or summarizes it (autocompact, smart-compact); none makes the outgoing request smaller. Sizing against 4,095 sets the ceiling to 2,527, which the un-compactable floor exceeds on turn 1, and neither degradation rung can touch that floor (rung 1 sheds tool RESULTS, rung 2 truncates/drops MESSAGES, while est(&[]) == overhead), so every run would terminate at engine.rs:13362 before the user's first turn. LIVE, the evidence this criterion always needed: real stock Ollama on hetzner (ollama ps -> CONTEXT 4096), qwen3:8b, through a byte-logging reverse proxy in front of /v1/chat/completions. Turn 1, before any tool result exists: body_bytes 14,548, char/4 estimate 3,652 (system prompt 1,153 + eight tool schemas 2,457), and the endpoint answered usage prompt_tokens 3,207 / completion_tokens 1,631 / total_tokens 4,838 -- 742 over the 4,096 slot on the FIRST turn. So 3,207 real confirms BASELINE_TURN_TOKENS is accurate to ~3%. SECOND RUN, the one that settles the ticket own sentence: same binary, same stock 4,096 slot, one 23,292-byte file to read. Turn 2 carried the tool result and core sent body_bytes 40,861 -- 5 messages (system 4,617 chars, user 88, assistant 180, tool result 26,054, user 173) plus eight tool schemas -- a char/4 estimate of 10,235 tokens, and the endpoint answered usage prompt_tokens 3,223 / completion_tokens 509 / total_tokens 3,732. Ollama processed 3,223 of an estimated 10,235 and silently discarded the rest, the file content included. That is within 3% of the ~10,529 the ticket itself measured. Core meanwhile sizes against UNVERIFIED_CONTEXT_WINDOW 32,768, so 10,235 reads as ~31% pressure, no guard fires and no compaction runs. The answer to the ticket own question is therefore YES: stock Ollama at a 4,096 slot is still truncated at this commit, and clause 2 of c3 does not hold. The remainder is FerroxLabs/wayland#1230 with measured criteria (derive the un-compactable floor, regrade the hardcoded BASELINE_TURN_TOKENS, take a named decision below the floor, live proof at a real 4,096 slot, and a negative control at 8,192). Not a duplicate of #1179, which scoped ITSELF to the 8k-32k workable band and named the 4,096 case out."
---

Detection shipped in v0.13.10. Compensation did not — and this ticket's BODY
sets that bar, not its title.

The reporter's own words: "32,768 is still 8x the served 4,096 slot, so the
truncation persists". Core now knows the real window and says so, which turns
a silent wrong answer into a visible one. The prompt is still truncated.

c3 is two clauses and only the first shipped. The wiring landed in eb2f2635 and
#1179 removed the saturation, so the learned window now DOES feed the guard and
both compaction triggers -- wherever the window can hold a turn. At the 4,096
slot this ticket measured it still does not, by design, and the truncation
therefore persists: measured live at e7144c30a, turn 1 sends 3,207 real prompt
tokens and the endpoint reports total_tokens 4,838 against a 4,096 slot. The
remainder is FerroxLabs/wayland#1230.
