---
issue: 113
repo: FerroxLabs/wayland-core
kind: defect
title: "Browser tool non-functional by default - Camoufox sidecar never spawned + policy-disabled (web automation/screenshots broken)"
status: open
last_verified_commit: be4467ed
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
    evidence: "file:.planning/DECISIONS.md:16"
    owner: core
    note: "SPLIT 2026-08-29: the old c5 asked for two different things -- a record and a close -- and only one of them was ever a maintainer act. The record is core's and is now DONE. Line 16 of .planning/DECISIONS.md is the Q-113 row taking the decision (close as refuted, recording deny-by-default), and the core lane posted that record as a comment on the issue on 2026-08-29, so the in-tree decision and the issue now agree -- previously the latest comment on #113 was a verification write-up and the posture appeared nowhere a reader of the issue could find it. The close is c6"
  - id: c6
    text: "The issue is dispositioned: closed as refuted, or the decision reversed"
    state: blocked
    owner: maintainer
    handoff: "FerroxLabs/wayland#1229"
    note: "The residue of the old c5, and a genuine maintainer act: only the maintainer closes issues in this repo. There is no code owed -- c1 through c4 refute or supersede three of the four reported claims with a test each, and the fourth (deny-by-default) is the intended posture with a config snippet that is round-tripped to a real Allow decision. wayland-core#364 carries it with the evidence table, and states the alternative plainly: perform the close, or reverse Q-113 and name a lane to build the opposite HANDOFF TARGET RECONCILED ON MERGE: this criterion was decomposed twice on 2026-08-29, by two lanes that could not see each other. The audit lane pointed it at FerroxLabs/wayland-core#364, which already existed and already carries the work; the decomposition lane filed FerroxLabs/wayland#1229 and that is the ticket named above, because it is scoped to this criterion. BOTH ARE OPEN AND THEY OVERLAP -- both are the same maintainer disposition -- close #113 recording deny-by-default, or reverse Q-113 and name a lane. Core does not close issues, so this is recorded rather than acted on: a maintainer should dedupe the pair, and whichever survives is the carrier. The audit evidence in this note was gathered against FerroxLabs/wayland-core#364 and applies to either."
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
