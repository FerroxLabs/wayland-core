---
issue: 1230
repo: FerroxLabs/wayland
kind: defect
title: "A served context slot below core's own baseline turn still truncates: 2,457 of a 4,096 slot is tool schemas before the user types"
status: open
last_verified_commit: 6bcf1b503
criteria:
  - id: c1
    text: "The un-compactable floor of a turn (system prompt + tool schemas -- everything degradation rungs 1 and 2 cannot touch) is a value the code computes from the assembled request, not the hardcoded BASELINE_TURN_TOKENS. Evidence: a test that derives it and a live figure quoted beside it."
    state: not-met
    owner: core
    note: "NOT STARTED by lane/f13-context, and recorded here so the release gate counts it rather than anyone having to remember it. What this lane DID verify at integ/f13 70a47aaed, so the next reader does not re-derive it: BASELINE_TURN_TOKENS is still the hardcoded 3_118 at crates/wcore-config/src/compact.rs:126, it is still the only thing CompactConfig::supports_compaction (compact.rs:610-611) compares either boundary against, and nothing anywhere recomputes it from an assembled request - a grep for the identifier returns the definition, the two supports_compaction comparisons, and test assertions in issue_1179_small_window_buffers_test.rs, and no producer. The ticket's own measurement (real floor 3,207 against the constant's 3,118, 2.8% low) therefore still stands unregraded. This lane's capacity went to the perf half of its brief (wayland#1235 / wayland-core#395); the window-arithmetic half of the brief (#1199, #1200, #1210, #1218, wayland-core#382) is already carried by the unmerged lane/f13-w2-window-arc, and #1230 is the one item in that group NO lane has taken. It is not covered by w2-window-arc either: that branch's ledger diff touches #1150, #1161, #1172, #1199, #1200, #1210, #1218, #1255 and wayland-core#382, and not #1230."
  - id: c2
    text: "BASELINE_TURN_TOKENS = 3,118 (compact.rs:126) is a snapshot of a number that moves whenever the system prompt or the tool set changes, and it gates every small-window decision. Either it is derived at runtime, or a test reds when the real floor drifts from it."
    state: not-met
    owner: core
    note: "NOT STARTED by lane/f13-context, and recorded here so the release gate counts it rather than anyone having to remember it. What this lane DID verify at integ/f13 70a47aaed, so the next reader does not re-derive it: BASELINE_TURN_TOKENS is still the hardcoded 3_118 at crates/wcore-config/src/compact.rs:126, it is still the only thing CompactConfig::supports_compaction (compact.rs:610-611) compares either boundary against, and nothing anywhere recomputes it from an assembled request - a grep for the identifier returns the definition, the two supports_compaction comparisons, and test assertions in issue_1179_small_window_buffers_test.rs, and no producer. The ticket's own measurement (real floor 3,207 against the constant's 3,118, 2.8% low) therefore still stands unregraded. This lane's capacity went to the perf half of its brief (wayland#1235 / wayland-core#395); the window-arithmetic half of the brief (#1199, #1200, #1210, #1218, wayland-core#382) is already carried by the unmerged lane/f13-w2-window-arc, and #1230 is the one item in that group NO lane has taken. It is not covered by w2-window-arc either: that branch's ledger diff touches #1150, #1161, #1172, #1199, #1200, #1210, #1218, #1255 and wayland-core#382, and not #1230."
  - id: c3
    text: "At a served window below that floor core takes a NAMED decision, not the current silent proceed-and-truncate: reduce the floor for the session, negotiate the slot upward where the endpoint supports it, or refuse the turn naming both numbers. The choice is documented with its tradeoff."
    state: not-met
    owner: core
    note: "NOT STARTED by lane/f13-context, and recorded here so the release gate counts it rather than anyone having to remember it. What this lane DID verify at integ/f13 70a47aaed, so the next reader does not re-derive it: BASELINE_TURN_TOKENS is still the hardcoded 3_118 at crates/wcore-config/src/compact.rs:126, it is still the only thing CompactConfig::supports_compaction (compact.rs:610-611) compares either boundary against, and nothing anywhere recomputes it from an assembled request - a grep for the identifier returns the definition, the two supports_compaction comparisons, and test assertions in issue_1179_small_window_buffers_test.rs, and no producer. The ticket's own measurement (real floor 3,207 against the constant's 3,118, 2.8% low) therefore still stands unregraded. This lane's capacity went to the perf half of its brief (wayland#1235 / wayland-core#395); the window-arithmetic half of the brief (#1199, #1200, #1210, #1218, wayland-core#382) is already carried by the unmerged lane/f13-w2-window-arc, and #1230 is the one item in that group NO lane has taken. It is not covered by w2-window-arc either: that branch's ledger diff touches #1150, #1161, #1172, #1199, #1200, #1210, #1218, #1255 and wayland-core#382, and not #1230."
  - id: c4
    text: "LIVE PROOF, not a mock: a run against a real stock Ollama serving CONTEXT 4096, through a request-logging proxy, in which either every turn's usage.prompt_tokens is strictly below the slot, or the run stops with c3's refusal BEFORE the first truncated request. Evidence must quote the proxy log."
    state: not-met
    owner: core
    note: "NOT STARTED, and it is the criterion that cannot be closed from this host at all as written: it requires a run against a real stock Ollama serving CONTEXT 4096 through a request-logging proxy, quoting the proxy log. The instrument the reporter used still exists on hetzner (/root/w-f13/nc-proxy.py, captures under /root/w-f13/nc-live2/proxylog/) and is named here so a later lane does not rebuild it. needs-live-endpoint."
  - id: c5
    text: "NEGATIVE CONTROL: the same binary against a slot that CAN hold a turn (>= 8,192) still completes the task."
    state: not-met
    owner: core
    note: "NOT STARTED by lane/f13-context, and recorded here so the release gate counts it rather than anyone having to remember it. What this lane DID verify at integ/f13 70a47aaed, so the next reader does not re-derive it: BASELINE_TURN_TOKENS is still the hardcoded 3_118 at crates/wcore-config/src/compact.rs:126, it is still the only thing CompactConfig::supports_compaction (compact.rs:610-611) compares either boundary against, and nothing anywhere recomputes it from an assembled request - a grep for the identifier returns the definition, the two supports_compaction comparisons, and test assertions in issue_1179_small_window_buffers_test.rs, and no producer. The ticket's own measurement (real floor 3,207 against the constant's 3,118, 2.8% low) therefore still stands unregraded. This lane's capacity went to the perf half of its brief (wayland#1235 / wayland-core#395); the window-arithmetic half of the brief (#1199, #1200, #1210, #1218, wayland-core#382) is already carried by the unmerged lane/f13-w2-window-arc, and #1230 is the one item in that group NO lane has taken. It is not covered by w2-window-arc either: that branch's ledger diff touches #1150, #1161, #1172, #1199, #1200, #1210, #1218, #1255 and wayland-core#382, and not #1230."
---

This entry exists so the release gate can see #1230 at all: it was in
lane/f13-context's brief and it had no ledger file, which is the state that
makes a ticket invisible to an "all criteria met" reading.

Nothing here is done. The lane it was assigned to spent its capacity on the
perf half of the same brief (wayland#1235 / wayland-core#395, both closed
with measurements and red arms), and the window-arithmetic half it sits
beside -- #1199, #1200, #1210, #1218 and wayland-core#382 -- is already
carried by the unmerged `lane/f13-w2-window-arc`. #1230 is the one item in
that group no lane has taken, and it is not covered by that branch: its
ledger diff touches #1150, #1161, #1172, #1199, #1200, #1210, #1218, #1255
and wayland-core#382, and not this one.

What was verified rather than assumed, so the next reader starts from a
re-derived position: `BASELINE_TURN_TOKENS` is still the hardcoded 3,118,
`supports_compaction` still compares both boundaries against it and nothing
else, and no producer anywhere recomputes it from an assembled request. The
ticket's measured 2.8% drift is unregraded.

c4 additionally cannot be closed from a Linux build host without the live
endpoint. The instrument the reporter used is still on hetzner and is named
in c4's note so it is not rebuilt.
