# 23B — what is left, measured rather than guessed

Written by the 23Bb lane, which closed the 23B-H1 residual and did **not** execute 23B-02
Task 2/3, 23B-03 or 23B-04. This is the triage I did before deciding that, so the next lane
starts from a measurement instead of repeating it.

## 23B-02 — PARTIAL. Task 1 shipped; Tasks 2 and 3 not started.

Criterion 3 is graded NOT MET deliberately and correctly. The plan's acceptance is
**absence from the real outbound provider request body**, captured via
`crates/wcore-cli/tests/support/mock_llm.rs`'s `received_requests`; what exists proves a
deleted row, which the plan names by hand as the engineered green to avoid.

Smallest honest closure of Criterion 3 — **does not** require the rest of Task 3:
`crates/wcore-cli/tests/memory_control_lifecycle.rs`, plant a run-time value through real
turns against the mock provider, assert it IS present in the captured outbound body, forget
it, take one more turn, assert it is absent from the next captured body. The plan is
explicit that a value which was always absent proves nothing, so the before-assert is the
load-bearing half. Task 2 (`cache_diagnostics.rs`, `compact/state.rs`, `/cost`, `/compact`)
is independent of that and is genuinely unstarted.

The three-platform driver family in Task 3 is a much larger job than the assertion above,
and the assertion is what F23-03 actually turns on.

## 23B-03 — NOT STARTED. A whole feature, not a gap.

Wave 3, `depends_on: 23B-02`, F23-06. Touches nine files across `wcore-repomap` and
`wcore-cli` (`store.rs`, `scope.rs`, `search.rs`, incremental index + retrieval-quality
suites, an `index_cmd` surface, TUI wiring, and two drive scripts). No partial credit is
available here; it is a plan-sized piece of work.

## 23B-04 — NOT STARTED, and partly not startable on demand.

Wave 4, terminal plan for the phase, `depends_on: 23B-01, 23B-02, 23B-03`. Its own
`must_haves` require that **at least one leg runs against real elapsed wall time with real
process restarts**, because an absolute budget deadline is meaningful precisely for having
survived the process not existing. That leg cannot be compressed into one session by
construction — it has to be started, left, and returned to.

It also carries the phase-level disposition and the D2 boundary statement, so it should be
run last and only after 23B-02 and 23B-03 have real dispositions to authenticate.

## Sequencing I would recommend

1. 23B-02 Criterion 3 via the outbound-body assertion above — smallest step to a real
   requirement disposition.
2. 23B-02 Task 2, then Task 3's driver family.
3. 23B-03.
4. 23B-04, starting its wall-clock leg early so it elapses while the rest proceeds.

## Two operational notes, both measured

- **macOS is obtainable.** `.planning/intel/MACOS-BINARY-IS-OBTAINABLE.md` is correct, and
  this lane used it for three macOS rows. For a binary carrying your OWN change you must
  also add your branch to `ci.yml`'s `push.branches` — CI fires only on `main` and
  `plan/f20-unified-audit-repair`, so a lane branch otherwise gets zero runs and zero
  artifacts. Revert that line before handback.
- **CI on this branch cannot reach the test step.** `Check Desktop protocol contract corpus
  drift` fails first, on macOS and on the self-hosted Windows runner. It fails identically on
  the untouched base (run 30232008236), so it is pre-existing, and clearing it means
  `wcore-contract generate`, which lanes are forbidden to run. Plan for cross-platform
  evidence to come from live binary legs, not from CI test results, until someone with
  release authority regenerates the corpus.
