---
issue: 355
repo: FerroxLabs/wayland-core
kind: defect
title: "A command-floor refusal makes the model improvise and hand the user a confident wrong answer instead of blocked by policy"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "A floor refusal is distinguishable by the model from a transient tool failure, carrying a marker that says this is a policy decision"
    state: not-met
    owner: core
    note: "REPO_CONTROL_REFUSAL is a bare &str at command_floor.rs:192 returned as an ordinary refusal reason. Grepping command_floor.rs for a policy marker of any spelling returns zero hits, so nothing distinguishes it from a command-not-found error."
  - id: c2
    text: "The refusal instructs the model to surface it to the user rather than work around it, and says so in the PAYLOAD rather than a log line"
    state: not-met
    owner: core
    note: "The refusal string names the rule but gives the model no instruction to stop. A log line cannot close this: RUST_LOG is unset on a default install and the TUI branch is file-only."
  - id: c3
    text: "A test drives a real floor refusal end to end and asserts the USER-VISIBLE output names the refusal"
    state: not-met
    owner: core
    note: "Asserting the string is present in the tool result is what already happens and is what failed to prevent the reported incident. No end-to-end user-visible assertion exists in the graded tree."
  - id: c4
    text: "A red arm is quoted verbatim, reproducing the improvisation"
    state: not-met
    owner: core
    note: "No reproduction is recorded in the graded tree."
---

The more dangerous half of the command-floor over-refusal report, and independent
of it: once the floor stops over-refusing, a floor refusal in any OTHER situation
will still fail this way.

Refused in place, the model improvised — staged the skill under `/tmp`, wrote a
file at the destination, `cd`'d into it, and then told the user it could not run
the brief. The user did not see "blocked by policy". They saw a confident wrong
answer plus side effects nobody asked for and nobody was told about.

An un-liftable guard is only as good as the model's willingness to stop when it
fires. The floor is not the only un-liftable guard in the product, so this is a
class defect, not a floor defect. Graded against `origin/integ/next` at
`43848f75`: nothing here has been started.

UPDATE 2026-08-29: the rule-1 over-refusal hotfix HAS now landed in `integ/next`
(20d99006, graded by `skill_scripts_under_wayland_core_are_runnable` with the
control `the_wayland_core_control_surface_stays_refused`). That removes the
reported instance of the trigger and changes nothing about this ticket: the
behaviour under any other floor refusal is untouched, which is exactly why this
was filed on its own.
