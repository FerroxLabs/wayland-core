---
issue: 113
repo: FerroxLabs/wayland-core
kind: defect
title: "Browser tool non-functional by default - Camoufox sidecar never spawned + policy-disabled (web automation/screenshots broken)"
status: closed
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "A browser op launches the configured Camoufox sidecar through the real production adapter path"
    state: met
    evidence: "test:crates/wcore-agent/tests/browser_sidecar_launch_wiring_test.rs::a_browser_op_launches_the_configured_sidecar_through_the_real_adapter"
    owner: core
    note: "asserts on the LAUNCHED PROCESS (a stub sidecar writes a file), not on a mock; the path is tool.rs ensure_session -> supervisor ensure_ready -> launch_camoufox_program"
  - id: c2
    text: "A machine with no sidecar on PATH provisions one by default instead of dead-ending on the download placeholder"
    state: met
    evidence: "symbol:crates/wcore-browser/src/binary.rs::provision_sidecar_via_npm"
    owner: core
    note: "reached from resolve_sidecar_program when the program is absent and the pinned-digest download path is off; SidecarAutoInstall defaults enabled for the operator-facing config"
  - id: c3
    text: "The deny-by-default refusal hands the operator a remedy that provably enables the tool"
    state: met
    evidence: "test:crates/wcore-agent/tests/browser_config_hint_roundtrip.rs::allowlist_snippet_actually_enables_the_tool"
    owner: core
    note: "the printed snippet is round-tripped through the real ConfigFile serde types to an actual Allow decision, so it cannot drift into prose that does not parse"
  - id: c4
    text: "No shipped doc still advertises the chromiumoxide fallback backend, which no longer exists in the tree"
    state: met
    evidence: "test:crates/wcore-browser/tests/no_phantom_backend_test.rs::no_shipped_source_advertises_a_chromiumoxide_backend"
    owner: core
    note: "1092e0c1 (red arm) then f83fcd2e (fix). The scanner walks shipped source with a positive control that fails if it reads nothing, plus the_two_real_backends_are_present so the prose cannot be fixed by deleting the real backends. registry-default.json - the description wayland plugin list prints - was fixed too and is guarded in plugin_install_smoke.rs."
  - id: c5
    text: "The deny-by-default browsing posture is recorded as a decision on the issue"
    state: met
    evidence: "file:.planning/DECISIONS.md:16:| Q-113 | core#113 |"
    owner: core
    note: "SPLIT 2026-08-29: the old c5 asked for two different things -- a record and a close -- and only one of them was ever a maintainer act. The record is core's and is now DONE. Line 16 of .planning/DECISIONS.md is the Q-113 row taking the decision (close as refuted, recording deny-by-default), and the core lane posted that record as a comment on the issue on 2026-08-29, so the in-tree decision and the issue now agree -- previously the latest comment on #113 was a verification write-up and the posture appeared nowhere a reader of the issue could find it. The close is c6"
  - id: c6
    text: "The issue is dispositioned: closed as refuted, or the decision reversed"
    state: met
    evidence: "file:.planning/DECISIONS.md"
    owner: maintainer
    note: "MET 2026-09-03. The maintainer performed the close: wayland-core#113 is closed/completed, executing Q-113 (.planning/DECISIONS.md line 16, CLOSE AS REFUTED recording deny-by-default). Every criterion was re-verified against the SHIPPED tree 6e4eca07 before the close rather than taken from this ledger -- all four c1..c4 evidence tokens resolve, and `grep -rl chromiumoxide crates/*/src` returns 0, so the phantom backend really is gone from shipped source rather than only from the docs that described it. THE DUPLICATE CARRIER PAIR IS DEDUPED, which is what this note previously asked a maintainer to do: FerroxLabs/wayland#1229 was closed as the duplicate and FerroxLabs/wayland-core#364 survives, because #364 also holds a SECOND and unrelated maintainer item (the Meta 15-app-per-developer cap behind #934/#1186) and so cannot close on this act alone. Recorded for the next reader: #1229 carried no milestone and no area:core label, so it had no ledger file and the coverage gate could not see it at all -- an issue invisible to scope, which is the failure mode FerroxLabs/wayland#1295 c2 tracks. The deny-by-default posture is UNCHANGED and remains deliberate; reversing it is a new issue naming a lane, not a reopen of this one."
---

The issue reports that the browser tool is advertised to the model but cannot
work: the Camoufox sidecar is never spawned, no sidecar ships, the policy denies
everything, and the CDP fallback is stubbed.

Three of those four claims are refuted or superseded at v0.13.10. There is a
complete production launch path with an end-to-end regression test that watches
the real process; a fresh machine now provisions the sidecar over npm by
default; and the stubbed chromium backend was deleted outright rather than left
half-built. The fourth claim - deny-by-default policy - is still accurate and
still deliberate, but the opaque dead end it caused is gone: the refusal is
routed by reason and carries a config snippet that is proven to work.

What remains is the disposition, and only the disposition. The stale doc comment
is fixed and guarded (c4), and the old c5 has been split: recording the
deny-by-default decision on the issue was core's and is done (c5, posted
2026-08-29), while the close itself is the maintainer's and is queued on
wayland-core#364 (c6). There is no code owed on this issue.

Criteria are transcribed from the cluster F verification note of 2026-08-29 and
each evidence token was re-checked against this tree before being cited; c5 was
split by the 2026-08-29 handoff audit.
