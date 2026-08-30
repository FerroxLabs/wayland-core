---
issue: 401
repo: FerroxLabs/wayland-core
kind: defect
title: "an_unknown_window_sizes_the_skill_listing precondition cannot pass in a clean container, so every integ/f13 CI run is red"
status: open
last_verified_commit: c680860b3
criteria:
  - id: c1
    text: "The test passes in a clean container (no ambient non-bundled skills) and on a host with a populated skills catalogue, without either being special-cased -- shown by running it in both"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by the w3-vcs-residuals lane while gating; unrelated to that lane's four issues, filed rather than absorbed. MEASURED on both sides, not modelled. CI, integ/f13 tip 70a47aae run 33291781675, three retries identical: `precondition: a 200,000-token window really does buy a longer listing here, or this test could pass on a catalogue with no skills in it (200k = 5046 bytes, unknown = 5046 bytes)`. CI, lane/f13-w3-vcs-residuals at 875bf32c run 33303443008, three retries identical, different absolute size: `(200k = 4890 bytes, unknown = 4890 bytes)`. Same test on hetzner at the same tree: `7 tests run: 7 passed`. Neither branch touches wcore-agent bootstrap or the skills catalogue."
  - id: c2
    text: "The discriminating precondition is preserved, not deleted: whatever replaces it must still fail on a catalogue that renders no skills at all, shown RED by emptying the catalogue"
    state: not-met
    owner: core
    note: "Nothing done. Recorded as a criterion because the cheap fix is to delete the precondition, and the precondition is the only thing stopping the identity assertion above it from passing on two empty listings."
  - id: c3
    text: "The fixture supplies whatever the budget must bind against (e.g. a planted NON-bundled skill large enough that 1,310 characters truncates it), so the assertion does not depend on $HOME"
    state: not-met
    owner: core
    note: "Nothing done. CAUSE, from the sibling test's own trailing comment in the same file: `format_skills_within_budget` picks between truncated and name-only degradation from the BUNDLED/non-bundled split of whatever is in the catalogue, and it has a C-5 escape hatch that returns full entries when every skill is bundled -- which is exactly a clean CI container. The budget therefore never binds there and the 200,000-token arm renders the same bytes as the 32,768-token one."
  - id: c4
    text: "A grep or a test proves no other assertion in crates/wcore-agent/tests/ depends on the ambient skills catalogue for its discriminating power"
    state: not-met
    owner: core
    note: "Nothing done. This is the SHAPE half: the sibling test in the same file already hit this, documented it, and chose a portable form -- and the defect was then reintroduced one function down in the guard rather than in the assertion. A comment is not a guard."
---

`an_unknown_window_sizes_the_skill_listing_like_the_window_it_assumes`
(`crates/wcore-agent/tests/issue_1150_unknown_context_window_test.rs:372`) fails
on EVERY CI run of `integ/f13` and of every lane branch cut from it, and passes
on the build host. It is not flaky and it is not the assertion the test exists to
make — the identity it grades passes. What fails is its own anti-vacuity
precondition, and in a clean container that precondition can never be satisfied.
A gate that cannot pass grades nothing.

Beyond the red: every lane branch inherits a red `linux-containerized` leg, so
`report` fails, so no lane can be graded green by CI. The signal that would
report a REAL regression in that job is already spent.

Criteria are taken verbatim from the issue's Acceptance section.
