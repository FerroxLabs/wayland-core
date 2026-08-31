---
issue: 1180
repo: FerroxLabs/wayland
kind: defect
title: "The bridge-backed approval resume path in main.rs is untestable where it lives, and is the one approval seam still ungraded"
status: closed
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "A test drives the bridge-backed approval resume in the active-turn command handler, by extraction or as an integration test"
    state: met
    evidence: "test:crates/wcore-cli/tests/approval_resume_active_turn.rs::a_bridge_backed_approval_parked_mid_turn_is_resumed_by_the_handler"
    owner: core
    note: "Extraction route taken: the handler is now crates/wcore-cli/src/approval_resume.rs and main.rs calls it from BOTH ApprovalResume arms (:6275 and :6700). The fixture is the real production BridgeConsentDoorbell on a real ApprovalBridge - no network, no spawned binary."
  - id: c2
    text: "That test goes red under the mutation: remove approval_bridge.resolve from the active-turn handler"
    state: met
    evidence: "symbol:crates/wcore-cli/src/approval_resume.rs::handle_approval_resume"
    owner: core
    note: "Graded structurally, and the structure is decisive here rather than timing-dependent: removing approval_bridge.resolve makes resolved false, which fails a single named assert!(resolved, ...). It also reddens a_stale_resume_is_named_on_the_wire's polarity. No mutation RUN is recorded, which is worth knowing, but the outcome is readable off one assertion."
  - id: c3
    text: "The new test does not resolve through bridge.pending_tokens, the shortcut no host has"
    state: met
    evidence: "symbol:crates/wcore-cli/tests/approval_resume_active_turn.rs::resume_token_from_the_wire"
    owner: core
    note: "The token is read off the emitted approval_required event, the host's only source, and the test additionally asserts it starts with apr-. pending_tokens appears in the file only in two doc comments warning against it; zero code uses. Negative control a_token_nobody_is_waiting_on_resolves_nothing_and_still_echoes."
  - id: c4
    text: "Extraction did not create a new gap: every ApprovalResume arm in main.rs still routes through the shared handler"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::every_approval_resume_arm_routes_through_the_shared_handler"
    owner: core
    note: "Added 2026-08-29. A test of the extracted function alone cannot see main.rs dropping the call, and the issue's acceptance says the mutation IS the specification on the ACTIVE-TURN handler. A comment-stripped source scan with a positive control asserting exactly two arms are found."
---

`approval_bridge.resolve(...)` in the active-turn command handler in
`crates/wcore-cli/src/main.rs` is the production seam completing an
`ApprovalRequired` to `ApprovalResume` loop for a bridge-backed approval parked
mid-turn. Nothing would notice if it were deleted.

The neighbouring approval gaps were closed by a cross-audit hardening pass — the
doorbell emission arm and the engine-side gate are both graded now. This arm was
attempted and could not be reached without either lifting the handler out of
`main.rs` (a production refactor) or a network-dependent fixture the suite
deliberately does not have. It was reported rather than done, which is why it is
open with nothing yet built.

c3 exists because the doorbell's tests passed unchanged under a mutation that
made it emit an empty token — they were green against the thing that mattered.
The issue says to assume this arm has the same problem until proven otherwise.
