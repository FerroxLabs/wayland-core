---
phase: 20A-native-windows-macos-uat
plan: "03"
subsystem: infra
tags: [windows, eol, autocrlf, gitattributes, swarm, landing, dirty-check, refuted, no-defect]

requires:
  - phase: 20A-native-windows-macos-uat
    provides: "the measured four-suite baseline (9/5/4 for transactional_delegated_mutation_test) and the 20A-02 AppContainer bind that unmasked the four dirty-checkout failures"
provides:
  - "The single-variable determination that the reported dirty-checkout symptom is a TEST FIXTURE artifact, not a product defect: production mints the integration checkout through the same scrub that later judges it"
  - "Measured proof that the in-tree `.gitattributes` `eol=lf` rule DOES override `core.autocrlf` and DOES survive `GIT_ATTR_NOSYSTEM=1`, vindicating the brief's mechanical claim while refuting its conclusion"
  - "Measured proof that deleting the forced `-c core.autocrlf=false` would change nothing, because the emptied system/global config already leaves autocrlf unset (default false)"
  - "The discovery that the dirty-checkout refusal was MASKING a deeper `\\?\\` extended-length path defect that fails the same four tests"
  - "Two recorded, deliberately unfixed findings: the fixture's unscrubbed clone (F-EOL-1) and the `assert_clean` false positive on users' own Windows repos (F-EOL-2)"
affects: [20A-04]

tech-stack:
  added: []
  patterns: []

key-files:
  created:
    - .planning/phases/20A-native-windows-macos-uat/20A-03-EOL-DECISION.md
    - .planning/phases/20A-native-windows-macos-uat/20A-03-SUMMARY.md
  modified: []

decisions:
  - "REFUTED-NO-DEFECT (termination state 2). No production file changed. The Task 2 blocking checkpoint did not run, per the plan's own rule."
  - "No reconciliation option was selected, because the determination left none applicable: `attributes-authoritative` is moot (the rule already works), `scrub-normalizes` is already the shipped design at this surface, and `relax-dirty-check` would blind a check that is reporting a true byte difference in a checkout production never builds."
  - "F-EOL-2 (`assert_clean` false-positives on users' own non-normalized Windows repos) was NOT silently resolved. Every candidate value costs something real; the plan reserves that choice for a blocking checkpoint and Sean was not available to take it."
  - "F-EOL-3 (`\\?\\` extended-length path rejected by git) was reported, not opened, per the execution bounds on new out-of-surface blockers."

metrics:
  duration: "~1h"
  completed: 2026-07-25

status: complete
---

# Phase 20A Plan 03: End-of-Line Reconciliation Summary

**The premise was false in the way that mattered, and measurement said so cleanly: the landing's dirty-checkout refusal is a test-fixture artifact, not a Windows product defect — and it was masking a deeper `\\?\` path defect underneath.**

Terminal state: **2 — Premise refuted, no defect.** No production code changed.

---

## What was measured, and how each alternative was excluded

Full command-by-command evidence is in
[`20A-03-EOL-DECISION.md`](20A-03-EOL-DECISION.md). All determination work ran in
a scratch clone at `C:\eol-scratch`, removed afterwards; `C:\ferrox-win` was read
only.

**Starting configuration, with origins.** `core.autocrlf=true` sourced from
`system  file:C:/Program Files/Git/etc/gitconfig` — the Git for Windows system
default, nothing Wayland or Sean set. `core.eol` unset in every scope.
`git version 2.54.0.windows.1`.

**The decisive chain.** Each step changed exactly one variable:

| # | Variable changed | Observation |
|---|---|---|
| M1 | — (fixture source built as `init_repo` does) | worktree + index both `base\n`; **no `.gitattributes` present**; clean |
| M1a | clone with ambient config (what the fixture does) | worktree becomes `base\r\n` — **CRLF** |
| M1b→M1c | **only** `core.autocrlf` | `true` → CLEAN; `false` → ` M README.md` |
| M1d | — | `check-attr` returns `text: unspecified, eol: unspecified` |
| M2 | **only** the clone's `core.autocrlf` (`false`, as production forces) | worktree stays `base\n`; landing invocation reads **CLEAN** |
| M3 | full scrub env replication | landing's own invocation reads **DIRTY** — the symptom does reach the landing |
| M5 | — | the dirty checkout was minted **seconds** before measurement |
| M6 | positive control on `C:\ferrox-win` under `GIT_ATTR_NOSYSTEM=1` | `text: set, eol: lf`; LF bytes on disk; landing invocation **CLEAN** |
| M7 | **only** the presence of a committed `.gitattributes` | ambient clone lands **LF** instead of CRLF; **CLEAN** |
| M8 | **only** the ambient autocrlf the fixture's clone inherits | the dirty error disappears from all four tests |

**The hinge.** The repository that goes dirty is *not this repository*. It is an
ephemeral temp-dir fixture repo with **no `.gitattributes` at all** (M1d), built
by `init_repo` and cloned by `clone_integration`
(`crates/wcore-agent/tests/transactional_delegated_mutation_test.rs:146-176`)
through a bare `std::process::Command::new("git")`.

**Production never builds that shape.**
`WorktreeManager::create_integration_checkout` clones through
`self.git_command(&clone_args)` (`worktree_manager.rs:1164-1180`), and
`git_command` (`worktree_cleanup.rs:414-421`) unconditionally prepends
`-c core.autocrlf=false`. So the checkout is minted by the *same* scrub that
later judges it — it lands LF and reads clean (M2, M4). Wayland already
*guarantees* the representation it judges against rather than inheriting it,
which is exactly what the plan's `scrub-normalizes` option describes as
desirable. It is already implemented.

**Fresh clone vs. stale pre-attributes checkout.** Explicitly distinguished. The
dirty tree was created seconds before the measurement, from an index written
seconds before that (M5); and `C:\ferrox-win` holds LF bytes with `eol: lf`
resolving correctly and reads clean (M6). Neither is stale.

**Alternative exclusion:**

- **(i) stale, never-renormalized checkout** — excluded by M5 and M6c/M6d.
- **(ii) attributes file not in effect** — M1d shows it is not in effect *because
  the fixture repo has none*; M6b/M7b show that wherever one exists it resolves
  exactly as committed.
- **(iii) the dirt is not EOL** — excluded at byte level: worktree
  `98,97,115,101,13,10` vs index `98,97,115,101,10`, a single inserted `0x0D`;
  and M8 shows neutralising only the EOL variable removes exactly that error.
- **(iv) the scrub disables in-tree attributes** — excluded by M6b: under
  `GIT_ATTR_NOSYSTEM=1` plus emptied system/global config, `check-attr` still
  returns `text: set, eol: lf`. That variable governs only the *system*
  attributes file.

**The brief's mechanical claim was right; its conclusion was not.** In-tree
`eol=lf` genuinely does override `core.autocrlf`, and genuinely does survive
`GIT_ATTR_NOSYSTEM=1` (M6b, M7a, M7d). The error was a category error about
*which* repository goes dirty.

**A second measurement worth keeping (M3a).** Under the scrub environment,
`git config --get core.autocrlf` exits 1 — *unset* — because
`GIT_CONFIG_NOSYSTEM=1` and the emptied config files already strip the user's
`autocrlf=true`, and git's default for unset autocrlf is `false`. So **deleting
the explicit `-c core.autocrlf=false` would change nothing.** The naive "just
drop the forced value" workaround is dead on evidence, not on argument.

**Measurement-trap discipline.** Every `cmd` assignment used the trap-safe
`set "VAR=x"` form and each value was echoed back and shown free of a trailing
space before any observation depending on it was trusted
(`[GIT_ATTR_NOSYSTEM=1]`, `[GIT_CONFIG_COUNT=1]`, …). In M8 the injected
override was additionally proved to reach a plain git (`false`) *and* to be
stripped by the scrub's own `env_remove("GIT_CONFIG_COUNT")` (`true`), so the
landing's invocation was provably unaffected by the probe. Every Mac-side grep
used `/usr/bin/grep`.

---

## Decision

The Task 2 blocking checkpoint **did not run**, per the plan's own rule: *"If
Task 1 determined REFUTED-NO-DEFECT, this checkpoint does not run at all: record
the refutation, make no code change, and close."* Task 3 did not run.

Unchanged: `worktree_cleanup.rs`, `worktree/parent.rs`, `worktree_tests.rs`,
`.gitattributes`. `git status --porcelain -- crates/ .gitattributes` is empty.

---

## Re-measured suite result

`cargo nextest run -p wcore-agent --test transactional_delegated_mutation_test --run-ignored all --no-fail-fast`
on `C:\ferrox-win` @ `c252d01d`:

```
Summary [6.152s] 9 tests run: 5 passed, 4 failed, 0 skipped
```

**Delta against the 20A-01 baseline (9/5/4): unchanged, exactly as intended** —
no code was changed, so no count could move. All four failures carry exactly
`parent integration checkout is dirty: M README.md`:

| Test | Cause |
|---|---|
| `happy_path_open_accept_land_receipt_then_rollback` | F-EOL-1 fixture clone |
| `restart_replays_landed_state_from_disk` | F-EOL-1 fixture clone |
| `land_selected_winner_drives_production_chain_to_landed` | F-EOL-1 fixture clone |
| `multi_candidate_only_winner_lands_loser_is_cleaned` | F-EOL-1 fixture clone |

These are precisely the four tests that reach `land_candidate` with a real
integration checkout; the other five never reach the dirty check. Attribution is
100 %, with no residual unexplained failure on this surface.

---

## Findings recorded, deliberately not fixed

**F-EOL-1 — test harness, MEDIUM.** `clone_integration` mints the integration
checkout with a bare unscrubbed `git clone`, unlike production. Sole cause of the
four failures. Not fixed: the file is outside this plan's declared
`files_modified`, and the plan's rule for a fix reaching outside those files is to
stop. The right remedy is also a genuine design question — patch the clone flags,
or have the fixture call `create_integration_checkout` so it exercises the real
minting path — and that should not be improvised.

**F-EOL-2 — product, HIGH, different surface.** `WorktreeManager::assert_clean`
(`worktree_manager.rs:582-598`) runs the same scrubbed status against
`self.repo_root` — the **user's own repository**, which Wayland does not mint and
cannot normalise — and it gates dispatch. On Windows a normally-cloned repo
without a normalizing `.gitattributes` has a CRLF worktree; M3b is exactly that
invocation on exactly that shape and returns ` M README.md`. Such a user is
refused dispatch on a pristine tree, naming a file they never touched. Not fixed:
different function, file outside `files_modified`, and the remedy *is* the design
decision this plan reserves for a blocking checkpoint — `autocrlf=input` would
fix this case but newly false-positive on any repo that legitimately commits
CRLF. The plan is explicit that a silent choice here is the failure mode to
avoid, so none was made. Does not reproduce on `C:\ferrox-win`, which commits the
normalizing rule.

**F-EOL-3 — NEW BLOCKER outside this surface, HIGH. Reported, not opened.** The
dirty-checkout refusal fires in `bind_parent_preimage` → `assert_clean_checkout`,
short-circuiting before the candidate quarantine. With the EOL variable
neutralised (M8) the same four tests advance and fail on:

```
candidate object graph failed quarantine revalidation:
  candidate build step ["read-tree", "cc1bdfa1…"] failed:
  fatal: not a git repository: '\\?\C:\Users\seand\AppData\Local\Temp\.tmpBTSy4s\checkout\.git'
```

A Windows extended-length (`\\?\`) path reaching git, which rejects it. Per the
bounds — *"If a NEW CRITICAL/HIGH blocker appears outside this surface, STOP and
report it — do not open a front"* — recorded and untouched.

**Consequence: there is no green available on this surface.** Even a perfect EOL
fix leaves these four tests red on `\\?\`. That is the most operationally
important thing this plan learned.

---

## Gate evidence

| Gate | Result |
|---|---|
| Scratch clone removed | `SCRATCH-REMOVED` |
| `C:\ferrox-win` SHA | `c252d01d3c885ed97ec0eff9b04280f2e5756672` — pinned, unchanged |
| `C:\ferrox-win` `git status --porcelain` | empty — unmodified, baselines intact |
| `cargo fmt --all -- --check` (Mac) | clean |
| `git status --porcelain -- crates/ .gitattributes` | empty — no production file touched |
| `git diff --exit-code -- scripts/f20-native-windows-proof.ps1` | clean — `$targets` byte-identical |
| Hetzner Linux non-regression | **not run — not required.** No production code changed, so there is no delta to regress. |
| Existing tests weakened / ignored / re-gated / deleted | **none** |

---

## Known unknowns, recorded not resolved

- Whether non-NTFS or network volumes change the checkout representation.
- Whether Git for Windows versions other than 2.54.0 ship a different system
  default for `core.autocrlf`.
- Whether any other repository the swarm consumes carries attributes rules that
  conflict with this one.

---

## Termination state

**State 2 — Premise refuted, no defect.** Recorded with evidence, no code change,
closed. No requirement marked complete; closure is claimed by the downstream
native proof under 20A-04.
