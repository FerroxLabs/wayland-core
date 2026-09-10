---
issue: 413
repo: FerroxLabs/wayland-core
kind: defect
title: "The DENY_CACHE_MAX_DIRS branch of deny_cache is ungraded and needs 100,001 directories to reach (split from #398 c5)"
status: open
last_verified_commit: b47c5013d
criteria:
  - id: c1
    text: "The `DENY_CACHE_MAX_DIRS` cap in `deny_cache` is reachable from a test without building a 100,001-directory fixture -- the cap is injectable, or the branch is removed if it is dead -- with the choice and its reason recorded."
    state: met
    evidence: "symbol:crates/wcore-tools/src/workspace_policy.rs::with_deny_cache_max_dirs"
    owner: core
    note: "THE CHOICE IS INJECTABLE, NOT REMOVED, AND THE REASON IS THAT THE BRANCH IS NOT DEAD. `dirs` is one `PathBuf` plus one `SystemTime` retained per directory for the life of the session, and this is the only thing bounding it on a large checkout; deleting it would make the memo unbounded, which is the opposite of what #1111 bought. HOW: `secret_deny_paths_for_backend` now compares `dirs.len()` against a per-policy `deny_cache_max_dirs` FIELD, which all three production constructors (`trusted_local`, `contained`, `delegated_mutation`) set from `DENY_CACHE_MAX_DIRS`. The only writer is `with_deny_cache_max_dirs`, which is `#[cfg(test)]` and `pub(crate)` -- no shipped binary can reach it, there is no config key, no builder and no env var, so this is a test seam and not policy configurability. c3 carries the half that keeps the seam honest. PRIOR NOTE PRESERVED: Carrier for the residual cut out of core#398 c5. c5 named `nested_stores_memoized`, which does not exist in this lineage -- grep returns 0 with `is_vcs_content_store` at 9 in the same call as a known-positive control. The surviving `DENY_CACHE_MAX_DIRS` branch belongs to `deny_cache` inside `secret_deny_paths_for_backend`, a DIFFERENT memo that #398 never touched. Left inside c5 it was a residual pointed at a symbol nobody can find, which is not tracked but lost."
  - id: c2
    text: "The branch-s behaviour at the cap is graded by a test that is driven RED by inverting the branch, with `cargo check` RC=0 first so the red is behaviour and not a build break."
    state: met
    evidence: "test:crates/wcore-tools/src/workspace_policy/tests.rs::the_deny_cache_is_retained_at_the_cap_and_dropped_above_it"
    owner: core
    note: "GRADED AT THE BOUNDARY IN BOTH DIRECTIONS, on hetzner-dsm through tools/remote-proof.py slot parallel-2, source 9a48af98f / 1f1bca706 / 4b03d642f. The boundary is MEASURED in the same run rather than assumed: a first call under `usize::MAX` establishes that this fixture stamps 9 directories, then a policy capped at 9 must memoise and one capped at 8 must not, with the deny list asserted EQUAL on both sides so a cap that changed the answer rather than its retention would fail. PASS-AFTER at 9a48af98f: `test result: ok. 2 passed; 0 failed` (`cargo test --locked -p wcore-tools --lib -- deny_cache`). TWO RED ARMS, each with `cargo check --locked -p wcore-tools --tests` rc=0 recorded on the SAME mutated commit before the red is believed, and each landing on an expression rather than a comment (the mutated line is quoted in the run record). ARM 1, the branch DROPPED (`(!dirs.is_empty() && dirs.len() <= self.deny_cache_max_dirs)` -> `(!dirs.is_empty())`, commit 1f1bca706): check rc=0, then rc=101 with `a walk that stamped 9 directories against a cap of 8 must NOT be memoised` -- the cap-enforcement assertion itself. ARM 2, the branch INVERTED (`<=` -> `>`, commit 9d6244f25): check rc=0, then rc=101 -- but it fails EARLIER, at the instrument control (`a walk under an unreachable cap must memoise`), because inverting also breaks the measurement step. That is a real red and it is recorded as what it is; arm 1 is the one that grades the cap. Both arms restored by `git reset --hard`, and the tree HEAD was re-verified green afterwards."
  - id: c3
    text: "If the cap is made injectable, the production default is asserted by a test, so the injectable seam cannot silently change what ships."
    state: met
    evidence: "test:crates/wcore-tools/src/workspace_policy/tests.rs::every_production_constructor_ships_the_production_deny_cache_cap"
    owner: core
    note: "BOTH WAYS THE SHIPPED BOUND COULD MOVE ARE PINNED, and each has its own red arm on hetzner-dsm with `cargo check` rc=0 first. (1) THE CONSTANT: `assert_eq!(DENY_CACHE_MAX_DIRS, 100_000)`. RED at commit 0f3029241 (`100_000` -> `99_999`): check rc=0, then rc=101 `left: 99999 / right: 100000`. (2) A CONSTRUCTOR PASSING SOMETHING ELSE: every one of `contained`, `trusted_local` and `delegated_mutation` is constructed and its `deny_cache_max_dirs` compared to the constant. RED at commit 4b03d642f (`contained` seeded with `512`): check rc=0, then rc=101 `contained does not ship the production deny-cache cap / left: 512 / right: 100000`. THE LIMIT, STATED: this does not prove the shipped cap BEHAVES at 100,000 -- nothing in this ledger has built a 100,001-directory tree and nothing should. What is proven is that the value the branch compares against in a shipped binary is the constant, and that the branch behaves correctly at whatever that value is (c2). Those two together are the claim; neither alone is."
---

Split out of core#398 c5 on 2026-08-31, which could not be met as written.

Why 0.13.13 rather than 0.13.12: it is an ungraded branch in a memo, not a leak and not a
wrong refusal. #398's own cost work is measured and green without it. What made this worth
a ticket rather than a note is that the criterion carrying it named a function that does
not exist, so nothing could ever have discharged it -- a gate that cannot pass is worth as
little as one that cannot fail.
