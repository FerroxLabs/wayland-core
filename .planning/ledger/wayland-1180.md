---
issue: 1180
repo: FerroxLabs/wayland
title: "The bridge-backed approval resume path in main.rs is untestable where it lives, and is the one approval seam still ungraded"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "A test drives the bridge-backed approval resume in the active-turn command handler, by extraction or as an integration test"
    state: not-met
    owner: core
    note: "reaching it as it stands needs a spawned --json-stream binary taken to a live egress-consent Ask verdict"
  - id: c2
    text: "That test goes red under the mutation: remove approval_bridge.resolve from the active-turn handler"
    state: not-met
    owner: core
    note: "the issue states this mutation IS the specification; a test that survives it has not closed the seam"
  - id: c3
    text: "The new test does not resolve through bridge.pending_tokens, the shortcut no host has"
    state: not-met
    owner: core
    note: "that shortcut is why the doorbell's own tests passed vacuously against an empty resume_token"
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
