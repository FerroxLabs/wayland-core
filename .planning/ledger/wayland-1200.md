---
issue: 1200
repo: FerroxLabs/wayland
kind: defect
title: "The accumulated-tool-result ceiling is a window-independent constant, so it permits about 80,000 tokens on a 32,768-token model"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The tool-result budget is derived from the resolved context window when one is known, with today's constants as the unknown fallback"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D17, found while verifying wayland#1150 — [Bug]: Absurd Input Token Size). Nothing has been done. The measured finding, verbatim: The c4 accumulated-tool-result ceiling is a window-independent constant, so at its shipped defaults it cannot keep the reporter's model inside its own context window. `total_budget_bytes = 120_000` bytes and `keep_recent = 4` (compact.rs:733-741) are absolute figures, never derived from the resolved context window. The four protected newest results are exempt 'however large', and each can carry up to `Tool::max_result_size()` = 50,000 chars (wcore-tools/src/lib.rs:529). Worst case carried tool-result payload = 120,000 + 4 × 50,000 = 320,000 bytes ≈ 80,000 tokens — about 2.4x the entire 32,768-token window the same release assumes for an unlisted model, and just under the 83,208 tokens the reporter measured. The ceiling's guarantee ('carried bytes stop growing with the session') is true and is what the evidence test measures; the guarantee a 32k user needs ('carried bytes fit the window') is neither claimed nor delivered, and nothing in the ledger says so."
  - id: c2
    text: "A test asserts that on a 32,768-token window the worst-case carried payload (total_budget_bytes + keep_recent x max_result_size) fits the window; shown RED against today's 120,000 + 4 x 50,000"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D17). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "The ledger entry for wayland#1150 c4 states WHICH guarantee is delivered -- carried bytes stop growing, or carried bytes fit the window -- rather than leaving a reader to assume the second"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D17). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The c4 accumulated-tool-result ceiling is a window-independent constant, so at its shipped defaults it cannot keep the reporter's model inside its own context window. `total_budget_bytes = 120_000` bytes and `keep_recent = 4` (compact.rs:733-741) are absolute figures, never derived from the resolved context window. The four protected newest results are exempt 'however large', and each can carry up to `Tool::max_result_size()` = 50,000 chars (wcore-tools/src/lib.rs:529). Worst case carried tool-result payload = 120,000 + 4 × 50,000 = 320,000 bytes ≈ 80,000 tokens — about 2.4x the entire 32,768-token window the same release assumes for an unlisted model, and just under the 83,208 tokens the reporter measured. The ceiling's guarantee ('carried bytes stop growing with the session') is true and is what the evidence test measures; the guarantee a 32k user needs ('carried bytes fit the window') is neither claimed nor delivered, and nothing in the ledger says so.

**Where.** crates/wcore-config/src/compact.rs:733-741 (default_tr_total_budget_bytes / default_tr_keep_recent) consumed by crates/wcore-agent/src/compact/micro.rs:647 bound_accumulated_tool_results

**Why it matters.** It shifts the entire load for small-window users back onto autocompact/emergency (c1), which is the loud, lossy path. A user on a 32k local model — the population this ticket is about — gets a 'ceiling' that permits more tool-result bytes than their model can accept, so the visible symptom (compact churn, or the endpoint truncating) persists after the fix. Deriving the budget from `known_context_window` when it is Some, with the current constants as the None fallback, would close it.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
