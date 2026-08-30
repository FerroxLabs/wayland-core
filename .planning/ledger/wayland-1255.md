---
issue: 1255
repo: FerroxLabs/wayland
kind: defect
title: "The accumulated-tool-result pass leaves one 130-byte stub per tool call, so carried bytes grow without bound and cross a 32k window at ~238 calls"
status: open
last_verified_commit: fdbc3f2e
criteria:
  - id: c1
    text: "The bytes bound_accumulated_tool_results leaves behind are bounded in the number of tool calls, or the prompt-cache cost of bounding them is measured and the tradeoff recorded as a decision"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane w2-window-arc while answering the adversarial refutation of FerroxLabs/wayland#1200. Nothing has been done beyond measuring it and pinning the arithmetic in-tree. THE MEASUREMENT: bound_accumulated_tool_results replaces each over-budget tool result with a stub and never re-mutates a stub -- that monotonicity is exactly what makes the pass prompt-cache safe -- so the bytes it leaves are `protected_tail + dropped x stub_len`, and stub_len is 130 bytes for a Read result at the 50,000-char ingestion cap. Driving the real pass with Some(32_768) on session_with_results(n, 50_000) at HEAD of lane/f13-w2-window-arc: n=20 -> 52,470B = 13,117 tok; n=100 -> 62,870B = 15,717 tok; n=500 -> 114,870B = 28,717 tok; n=2000 -> 309,870B = 77,467 tok. The guard on a 32,768-token window admits 20,208 tokens, so the bound HOLDS at 20 and 100 calls and FAILS from about 238 calls onward -- input_ceiling_for_window(32_768) x CHARS_PER_TOKEN = 80,832 bytes, the unconditionally-protected newest result takes 50,000 of them, and 30,832 / 130 = 237 stubs. At 2,000 tool calls the pass carries 2.36x the whole window, which is the state #1200 reports, reached by a longer session instead of by a bigger budget. Even a 131,072-token model goes over at 2,000 calls (459,480B = 114,870 tok against a 108,072-token ceiling). WHY NOTHING CAUGHT IT: #1150 c4's evidence test compared a 20-call session to a 100-call session with a hand-written `+ 20_000` byte slack, and the real difference is 10,400 bytes (80 extra stubs x 130) -- the slack was wide enough to swallow exactly the term it should have measured, so unbounded linear growth read as `does not scale with session length`. That slack is now an equality on the stub term. WHY IT IS NOT FIXED HERE: the residue exists BECAUSE the pass is monotone, and the only change that alters the order of growth is to collapse runs of adjacent stubs, which means re-mutating a stubbed body and invalidating the provider's cached prefix -- the discipline #1150 c6 and #559 were built on. A plausible shape is to collapse at epoch boundaries only, reusing the epoch_results quantization the pass already has, so the prefix is rewritten once per epoch rather than once per turn; the cost of that rewrite has not been measured. Shortening bounded_result_stub is explicitly NOT a fix: it moves the constant, not the O(n). Recorded as Q-1255 in .planning/DECISIONS.md."
  - id: c2
    text: "A test drives the real bound_accumulated_tool_results(.., Some(32_768)) at a session length past the crossing point and asserts the carried payload fits input_ceiling_for_window(32_768); shown RED against today's 309,870 bytes / 77,467 tokens at 2,000 tool calls"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane w2-window-arc. Partially prepared, deliberately not claimed. `test:crates/wcore-agent/src/compact/micro.rs::the_carried_payload_grows_by_one_stub_per_dropped_result` already exists on lane/f13-w2-window-arc and pins the arithmetic from the other side: it asserts carried == MAX_TOOL_RESULT_BYTES + dropped x stub at n = 20 / 100 / 500, asserts the totals are STRICTLY INCREASING so a future change that made them constant would redden it rather than drift, and then constructs the crossing session ((admissible - MAX_TOOL_RESULT_BYTES).div_ceil(stub) + 2 calls) and asserts the carried payload EXCEEDS what the window admits. That test DOCUMENTS this defect; it does not close it. c2 asks for the opposite polarity -- the same call asserting the payload FITS -- and that assertion cannot pass until c1 is done, which is why this is not-met rather than met with a caveat."
---

`bound_accumulated_tool_results` replaces each over-budget tool result with a stub and never re-mutates one, so the bytes it leaves are `protected_tail + dropped_results x stub_len` with `stub_len` = 130. The second term grows without bound in the number of tool calls, so the ceiling FerroxLabs/wayland#1200 installed holds for short sessions and fails for long ones.

Measured on a 32,768-token window at `lane/f13-w2-window-arc`, driving the production pass:

| tool calls | carried bytes | carried tokens | ceiling (tokens) | fits? |
|---|---|---|---|---|
| 20 | 52,470 | 13,117 | 20,208 | yes |
| 100 | 62,870 | 15,717 | 20,208 | yes |
| 500 | 114,870 | 28,717 | 20,208 | no |
| 2,000 | 309,870 | 77,467 | 20,208 | no |

**Where.** crates/wcore-agent/src/compact/micro.rs — `bound_accumulated_tool_results`, its `candidates[..eligible]` stub loop, and `bounded_result_stub`. `is_stubbed_result` correctly keeps stubs out of `candidates` on later passes, which is why the residue only ever accumulates.

**Why it matters.** This is FerroxLabs/wayland#1150's reported symptom ("Absurd Input Token Size") surviving the fix that was supposed to remove it. #1200 moved the threshold from "any session" to "about 238 tool calls" on a 32k model; it did not remove it. A 2,000-tool-call session is ordinary for an agent CLI.

Criteria are the Acceptance section of the issue. The trade behind not fixing it in the filing lane is written out in `.planning/DECISIONS.md` under Q-1255.
