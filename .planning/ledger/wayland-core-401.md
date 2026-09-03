---
issue: 401
repo: FerroxLabs/wayland-core
kind: defect
title: "an_unknown_window_sizes_the_skill_listing precondition cannot pass in a clean container, so every integ/f13 CI run is red"
status: open
last_verified_commit: 6e4eca07
criteria:
  - id: c1
    text: "The test passes in a clean container (no ambient non-bundled skills) and on a host with a populated skills catalogue, without either being special-cased — shown by running it in both."
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1150_unknown_context_window_test.rs::the_fixture_overflows_every_budget_under_test"
    owner: core
    note: "MET at 6e4eca07. The FIXTURE, not the host, supplies the overflow: FILLER_SKILLS=30 x FILLER_DESC_LEN=400 = 12,000 planted characters against the 8,000-char largest budget under test, and the_fixture_overflows_every_budget_under_test ASSERTS that rather than assuming it. Clean-container arm: CI run 33637957153 job 100273473415 CI (linux-containerized), PASS ( 3498/17812) an_unknown_window_sizes_the_skill_listing_like_the_window_it_assumes. Populated-host arm: hetzner already passed this file pre-fix, which is the asymmetry the issue reports. So the red both this issue and its duplicate FerroxLabs/wayland#1271 describe is GONE -- and not because a lane repaired it after the fact: the broken form never reached main. The test did not exist at cfa89a9c8 and arrived at 93ede3424 already carrying its 30-filler-skill fixture; the red was on branch integ/f13. ORIGINAL: Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c2
    text: "The discriminating precondition is preserved, not deleted: whatever replaces it must still fail on a catalogue that renders no skills at all, shown RED by emptying the catalogue."
    state: not-met
    owner: core
    note: "STAYS NOT-MET, and the reason is demonstration rather than substance: the property itself IS preserved in the tree at test file :420-431, which asserts a 200,000-token window really does buy a longer listing "or this test could pass on a catalogue with no skills in it". What is missing is a committed artifact recording the empty-catalogue RED arm, and that cannot be produced without a build. Settle on a scratch copy only: set FILLER_SKILLS = 0, delete the plant of the issue-1150 marker skill, and the test must FAIL on its precondition; restore and `touch` every restored file before re-running (mtime-restore trap). ORIGINAL: Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c3
    text: "The fixture supplies whatever the budget must bind against (e.g. a planted NON-bundled skill large enough that 1,310 characters truncates it), so the assertion does not depend on `$HOME`."
    state: met
    evidence: "file:crates/wcore-agent/tests/issue_1150_unknown_context_window_test.rs:141:const FILLER_DESC_LEN: usize = 400;"
    owner: core
    note: "MET at 6e4eca07. The fixture plants 30 non-bundled filler skills of 400 chars each into a tempdir bound via .extra_skill_dirs (test file :140-141, :153-166, :196), so the assertion no longer takes any discriminating power from $HOME. ORIGINAL: Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c4
    text: "A grep or a test proves no other assertion in `crates/wcore-agent/tests/` depends on the ambient skills catalogue for its discriminating power — the sibling comment shows this is a repeated shape, not one site."
    state: not-met
    owner: core
    note: "STAYS NOT-MET. The sweep was run statically and came out clean, but it produces no machine-resolvable evidence token, and a green clean-container leg cannot distinguish "not ambient-dependent" from "vacuous there". Static sweep at 6e4eca07: `grep -rln extra_skill_dirs crates/wcore-agent/tests/` returns exactly issue_1150_unknown_context_window_test.rs, issue_1150_ordinary_turn_payload_test.rs and issue_1280_skills_ceiling_test.rs, each of which plants its own catalogue into a tempdir; the only other skills-budget grader, issue_1280_skills_ceiling_test.rs:386/:434/:460-468, builds SkillRefs in memory and calls format_skills_section directly. To make this a resolvable met, land a committed guard test and cite it with a `test:` token. ORIGINAL: Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c5
    text: "The property in c1 is still GRADED after the change, shown RED by a mutation that restores the 200,000 fabrication in `get_char_budget`'s `None` arm, with `cargo check -p wcore-agent --tests` exit 0 quoted so the red is a behaviour change and not a build failure."
    state: not-met
    owner: core
    note: "TRANSPLANTED VERBATIM from FerroxLabs/wayland#1271 c2 so that duplicate can close without losing its one unique criterion. #1271 and this issue are the same defect filed 84 minutes apart in two trackers, naming the same test, the same file and the same CI run 33291781675, with neither body nor comment referencing the other; this issue is the carrier because it is a superset. Settle on a scratch copy only: edit crates/wcore-skills/src/prompt.rs:57 `None => DEFAULT_CHAR_BUDGET,` to `None => 8_000,`, quote `cargo check -p wcore-agent --tests` exit 0, then the test must FAIL; restore and `touch` the file before re-running. The compile-time guard at prompt.rs:31-33 constrains DEFAULT_CHAR_BUDGET's derivation, not the `None` arm's literal, so this mutation does compile."
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on
wayland and EVERY open issue on wayland-core. This issue was in scope from
the moment it was filed and had no ledger file, so
`scripts/check-release-readiness.py` -- which reads ledger files and nothing
else -- could not count it. CI runs the coverage gate with `--offline`, the
arm that would have reported the gap, so nothing said so for two days.

Criteria are transcribed from the issue body without edit. Where the body's
wording is loose it is LEFT loose rather than tightened here: sharpening a
criterion inside the ledger is how a criterion quietly becomes an easier
adjacent property. Whoever takes this restates it on the ISSUE first.
