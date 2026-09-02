---
issue: 413
repo: FerroxLabs/wayland-core
kind: defect
title: "The DENY_CACHE_MAX_DIRS branch of deny_cache is ungraded and needs 100,001 directories to reach (split from #398 c5)"
status: open
last_verified_commit: 6fdf05215
criteria:
  - id: c1
    text: "The `DENY_CACHE_MAX_DIRS` cap in `deny_cache` is reachable from a test without building a 100,001-directory fixture -- the cap is injectable, or the branch is removed if it is dead -- with the choice and its reason recorded."
    state: not-met
    owner: core
    note: "Carrier for the residual cut out of core#398 c5. c5 named `nested_stores_memoized`, which does not exist in this lineage -- grep returns 0 with `is_vcs_content_store` at 9 in the same call as a known-positive control. The surviving `DENY_CACHE_MAX_DIRS` branch belongs to `deny_cache` inside `secret_deny_paths_for_backend`, a DIFFERENT memo that #398 never touched. Left inside c5 it was a residual pointed at a symbol nobody can find, which is not tracked but lost."
  - id: c2
    text: "The branch-s behaviour at the cap is graded by a test that is driven RED by inverting the branch, with `cargo check` RC=0 first so the red is behaviour and not a build break."
    state: not-met
    owner: core
    note: "The red arm is the requirement, not decoration. A test that only passes on today-s file grades nothing, and this repo has shipped that shape before."
  - id: c3
    text: "If the cap is made injectable, the production default is asserted by a test, so the injectable seam cannot silently change what ships."
    state: not-met
    owner: core
    note: "An injectable constant is a way for the tested value and the shipped value to diverge. Whichever way c1 goes, the shipped default is the thing that must be pinned."
---

Split out of core#398 c5 on 2026-08-31, which could not be met as written.

Why 0.13.13 rather than 0.13.12: it is an ungraded branch in a memo, not a leak and not a
wrong refusal. #398's own cost work is measured and green without it. What made this worth
a ticket rather than a note is that the criterion carrying it named a function that does
not exist, so nothing could ever have discharged it -- a gate that cannot pass is worth as
little as one that cannot fail.
