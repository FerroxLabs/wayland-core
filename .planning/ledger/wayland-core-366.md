---
issue: 366
repo: FerroxLabs/wayland-core
kind: defect
title: "Container orphan scan is nonce-scoped, so it can never see a leftover from an earlier run"
status: open
last_verified_commit: 6c87400b2
criteria:
  - id: d1
    text: "The product can enumerate wayland-created containers WITHOUT being given a nonce -- a key-presence scan reachable from an operator-facing surface, not only from a nonce-scoped path"
    state: met
    evidence: "symbol:crates/wcore-exec-backend/src/contract.rs::UnscopedOrphanScan"
    owner: core
    note: "`ExecutionBackend::scan_all_orphans` is a new REQUIRED trait method (crates/wcore-exec-backend/src/contract.rs:418+) with no default, so a backend that cannot do it has to SAY so rather than answer zero. The container implementation issues the key-presence query the ticket names -- `docker ps -a --filter label=wayland.task.nonce`, no `=value` -- at crates/wcore-exec-backend/src/backends/container.rs. OPERATOR SURFACE, EXERCISED LIVE, not asserted: `wayland-core backend orphans` with no `--nonce`, run from the built binary on hetzner-dsm against the real daemon with one planted container:\n\n  container  enumerated=true  found=1 via docker ps -a --filter label=wayland.task.nonce (key presence)\n             - wayland-f13-leftover-probe (nonce nonce-from-a-run-that-is-over) [LEFTOVER - no live record in this process]\n  1 leftover surface(s) with no live record in this process; 3 surface(s) could NOT be scanned without a nonce.\n\nThe other three backends decline explicitly rather than returning a clean zero (`local`/`ssh`: a child carries no product-wide marker in its argv; `cloud`: the client-side filter is unexercised without vendor credentials), each line ending 'This is NOT a report of zero orphans.'"
  - id: d2
    text: "The nonce-scoped scan_orphans(nonce) contract is left intact for the caller that genuinely wants one run's orphans (cancel()); the unscoped scan is an addition, not a widening. State which callers move and which do not"
    state: met
    evidence: "symbol:crates/wcore-exec-backend/src/backends/container.rs::list_containers_with_nonce"
    owner: core
    note: "ENUMERATED BY GREP, NOT BY RECALL, because the ticket's own list is wrong and a previous pass reproduced it from memory. `grep -rn 'scan_orphans' --include=*.rs .` over the whole tree gives FOUR production call sites and one test call site, and NONE of them moves:\n\n  crates/wcore-cli/src/backend.rs:498            orphans_scoped() -- reached ONLY when the operator passes --nonce. STAYS: they named a nonce.\n  crates/wcore-exec-backend/src/orphan.rs:230    orphan::scan_all(nonce, ..) -- one caller, wcore-cli/src/backend.rs:649 (`backend scan --task-id`). STAYS.\n  crates/wcore-exec-backend/src/orphan.rs:246    orphan::scan_one(backend_id, nonce, ..) -- one caller, tests/fail_closed_matrix.rs:578. STAYS.\n  crates/wcore-exec-backend/src/conformance.rs:364  conformance check 5. STAYS, with its limits now stated (d4); a NEW check 5b calls the unscoped scan alongside it.\n  crates/wcore-exec-backend/tests/container_orphan_scan.rs:135  the discriminating control in d5's test. STAYS scoped BY DESIGN -- it exists to be blind.\n\nThe remaining `scan_orphans` hits are the trait declaration (contract.rs:418) and the four backend impls (local.rs:294, ssh.rs:428, container.rs:818, cloud.rs:894), which are definitions and not callers.\n\nCORRECTION TO THE TICKET, recorded because the criterion is written around it: `ContainerBackend::cancel()` does NOT call `scan_orphans`. It calls the private helper `list_containers_with_nonce(&entry.nonce)` directly (container.rs, in `cancel`). So 'the caller that genuinely wants one run's orphans' reaches the nonce-scoped enumeration by a different route and is structurally untouched by anything done to the trait -- which makes the intact-contract requirement stronger, not weaker: it holds for a caller the trait change could not have reached even if it had widened the method. The four callers that DO use the trait method were each dispositioned above rather than waved through as a group."
  - id: d3
    text: "An operator surface reports a leftover it did not create in this process -- one whose nonce is not in the live registry"
    state: met
    evidence: "test:crates/wcore-exec-backend/tests/container_orphan_scan.rs::the_unscoped_scan_reports_a_leftover_from_a_nonce_this_process_never_used"
    owner: core
    note: "`UnscopedOrphan::known_to_this_process` is computed against the live registry, and `UnscopedOrphanScan::leftovers()` is the projection over the `false` ones -- the value a nonce-scoped scan can never return, because a leftover's own run already called `registry::forget`. GRADED THROUGH THE PRODUCTION SURFACE, twice: the live CLI run quoted on d1 called the planted container a LEFTOVER, and the regression test asserts the same through `scan_all_orphans` + `leftovers()`. RED ARM (not the easier adjacent property): with `known_to_this_process` forced to `true` -- compiles, `cargo check -p wcore-exec-backend --tests` clean -- the scan still enumerates and still finds the container, and the test reddens on exactly this row:\n\n  panicked at crates/wcore-exec-backend/tests/container_orphan_scan.rs:119:5:\n  a container whose nonce is not in this process's live registry must be reported as a LEFTOVER; reporting only what this process already knows about answers a question nobody needed asked (#366 d3)\n\nThe sibling negative-control test stayed GREEN under that same mutation, which is what tells the two properties apart."
  - id: d4
    text: "The conformance check at conformance.rs:340 is re-examined: it asserts enumerated && found.is_empty() for a nonce chosen so nothing can ever be found"
    state: met
    evidence: "symbol:crates/wcore-exec-backend/src/conformance.rs::UNSCOPED_SCAN_CHECK"
    owner: core
    note: "BOTH branches the criterion offers, not one. (a) ITS LIMITS ARE STATED, and in the check's own NAME rather than only in a comment a reader has to find: it now reports as 'a SCOPED orphan scan enumerates rather than assuming (this grades that the scan RUNS; its nonce is unused by construction, so it can never grade FINDING an orphan -- see #366 d4)'. The comment above it says plainly that `found.is_empty()` cannot fail on this axis and that the whole verdict rests on `enumerated`, and names where the FIND arm lives. (b) AN ARM THAT REQUIRES A FIND EXISTS, in tests/container_orphan_scan.rs, and it is not in conformance.rs for a stated reason: it must PLANT a labelled surface, which is backend-specific and side-effecting on the host, while `run_conformance` is the provider-neutral pass every backend runs. A new check 5b grades the unscoped scan on a property that CAN fail however a backend answers: `enumerated && unsupported_reason.is_some()` (claiming to have looked while saying it cannot) and `!enumerated && !found.is_empty()` (reporting finds from a scan that did not run) are both refused."
  - id: d5
    text: "A regression test plants a labelled leftover under a nonce the running process has never used, and asserts the unscoped scan reports it -- creating the leftover itself and cleaning up after itself"
    state: met
    evidence: "test:crates/wcore-exec-backend/tests/container_orphan_scan.rs::the_unscoped_scan_reports_a_leftover_from_a_nonce_this_process_never_used"
    owner: core
    note: "The file plants its own containers (`docker create --label wayland.task.nonce=orphan366-nonce-from-an-earlier-run`, reaching `Created` and never started -- the shape both #365 leftovers were found in, and the state `docker run --rm` cannot clean up because `--rm` removes on EXIT), asserts, and removes them. It never waits for a dirty host, which is the blind spot #365 c5 named: CI runners are always fresh, so a leftover would never exist there however often the suite ran. RUN ON A REAL DAEMON, hetzner-dsm, docker 29.2.1: `Summary [33.807s] 2 tests run: 2 passed, 0 skipped`. THREE ARMS, and the last two are what stop the cheap fix passing: the scoped scan is run against a fresh nonce on the SAME host at the same instant with the leftover still present and must come back EMPTY (so a scanner that returned everything would fail); an UNLABELLED container must not be reported as ours; and a container whose nonce IS in the live registry must come back `known_to_this_process` and be excluded from `leftovers()`. RED ARM: with `scan_all_orphans` neutered to enumerate nothing -- compiles -- both tests redden, `the unscoped scan did not report the planted leftover orphan366-leftover; found [] via docker ps -a --filter label=wayland.task.nonce (key presence)`."
  - id: d6
    text: "Whether reclamation is in scope is DECIDED and recorded: state whether an unscoped scan only reports, or also reclaims, and justify it against #365's guard"
    state: met
    evidence: "symbol:crates/wcore-exec-backend/src/contract.rs::leftovers"
    owner: core
    note: "DECIDED: REPORT ONLY, never reclaim, recorded on the type itself so it cannot drift out of a commit message. The justification is the asymmetry against #365's submit-path reclaim rather than an inheritance from it: that path CAN prove removal safe because it holds the exact task id it is about to run under and can refuse a running holder or an unlabelled container. An unscoped background scan holds no such claim -- every candidate it finds is BY CONSTRUCTION one this process did not create, so it cannot distinguish a dead leftover from a live task in ANOTHER wayland process on the same daemon, whose nonce is absent from this process's registry for exactly the same reason a leftover's is. Removing on that evidence destroys another agent's running work; the failure mode of reporting is a line an operator has to act on. The decision is also enforced where an operator sees it: the CLI's closing line says 'Nothing here was removed', and it was verified live -- the planted container was still present after the scan and had to be removed by hand."
---

Split out of #365 c6. All six criteria graded against the tree on 2026-08-31 by
lane `sandbox` (branch `lane/f13-sandbox`); every row had been seeded `not-met`
with nothing measured.

Two things found while grading that are worth keeping:

1. **The ticket's caller list is wrong.** `cancel()` does not call
   `scan_orphans`; it calls the private `list_containers_with_nonce` helper. The
   real trait-method call sites are four, and they were enumerated by grep for
   d2 rather than reproduced from the ticket.

2. **The change did not survive the real clippy gate.**
   `live.iter().any(|n| *n == nonce)` is `clippy::manual_contains`, which is
   `-D warnings` in CI. `cargo check` and `cargo nextest` were both green on it.
   Fixed in `6c87400b2`.
