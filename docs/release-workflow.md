# Release verification workflow

The workflow has three stages. A missing prerequisite is a failed admission,
not evidence that the product passed or failed its behavioral checks.

| Stage | Owner | Work |
| --- | --- | --- |
| Fast admission | `admission` | Validate evidence/prerequisite controls and Windows routing before allocating native jobs. |
| Build and affected admission | `build`, native `ci`, `ci-linux` | Build each declared target; verify matching artifact identity, release smoke and packaged behavior before long suites. |
| Qualification and publication | native/Linux CI, `report`, release workflow | Complete the existing native, shared-process, feature, security and release gates. Publication requires their actual results. |

## Builds and runner ownership

`Build (<target>)` owns the default-feature release binary for that target.
Native macOS and Windows CI reuse this producer's binary after their debug/lint
work. The native consumer jobs depend on build completion before reserving a
runner, preventing waiting consumers from exhausting a constrained native pool.
The consumer requires the same workflow run and attempt, a successful
producer job, actual checkout SHA, event head SHA, toolchain, target, profile,
features, runner image/ABI contract and binary digest. Missing or mismatched
identity fails admission; there is no silent fallback compilation.

PR head identity and the synthetic merge checkout are separate fields.
The downloaded binary must report the actual checkout identity. A branch name
or an artifact filename is insufficient proof.

Linux's container/sysroot contract and voice-enabled release builds are different
build contracts. They are deliberately not substituted with a default-feature
native CI binary. Debug test binaries and `tool_token_bench` also remain distinct
from release binaries.

Hosted Windows supplies the required `CI (Array)` context, so an offline
personal machine does not hold up ordinary qualification. Private-fleet checks
stay in their explicit manual/nightly workflow and do not borrow a hosted
image's identity. Ordinary CI refuses a self-hosted opt-in instead of claiming
that a hosted pass qualified a different machine. Fork PRs remain hosted.

## Run each check in its proper stage

Release smoke and packaged-driver checks run before the expensive workspace
suite. The workspace selector excludes the release-smoke binary because the
early stage already executes it with `WCORE_SMOKE_REQUIRE_PREBUILT=1`.

The Windows 1,000-dispatch boundary runs alone before the workspace suite and is
excluded from that subsequent invocation. Its original 400-by-3 workload,
1,000-dispatch limit and 240-second timeout are unchanged. This is resource
isolation, not a performance improvement claim. Its own result remains required.

The Linux recovery fault-cut target and macOS walk-identity timing controls also
run in isolated steps before bulk execution, then are excluded from that bulk
invocation on the corresponding platform. Their existing cases, assertions and
timeouts are unchanged. The separation removes unrelated test competition; it
does not reinterpret an earlier failure as a pass.

Shared-process library and integration tests stay separate: nextest's process
isolation cannot prove that concurrent tests safely share global state. Their
existing minimum counts and behavioral assertions remain enforced.

Nightly Windows soak and repeated flake measurement remain in their existing
scheduled/manual workflows. They are not ordinary PR triggers. Required budget,
cancellation, credential, containment and release acceptance checks remain gates.

## Evidence and resumption

Test commands execute once at the workflow level. A failed test never starts a
second whole-workspace attempt automatically. Existing historical failed-attempt
receipts remain readable, and per-test retries remain visible to the flake gate.
Infrastructure retries belong to setup, before tests begin.

Every wrapped invocation records full stdout/stderr, source, argv, working
directory, timestamps and exit/signal status. Every fresh JUnit report is copied
before a later invocation can replace it. Missing fresh JUnit cannot report a
successful test run. Raw diagnostics upload even when a later step fails.

Use `qualification-plan` and `qualification-status` from the justfile with the
explicit task contract described in [qualification-status.md](qualification-status.md).
Unambiguous fixture ownership can select affected targets; production, shared
helper, configuration or unknown changes require broader qualification.
Reusing a pass requires matching declared complete inputs and execution identity.
The original receipt's source is preserved, not relabeled as the new candidate.
Partial or reused evidence never silently satisfies a missing required check.

Readiness distinguishes committed, built, verified, merged and published.
A passing build is not qualification, and a tag is not a published release.
Use the recorded failed attempt and exact affected command for a correction;
do not restart the programme or reopen unrelated accepted work.

## Measure the improvement

Record job/step durations, queue time, actual build invocations, selected test
counts and first-failure diagnostic completeness before and after deployment.
The expected savings are removed duplicate native release builds, eliminated
whole-suite retry execution, earlier packaged failures and scoped diagnostic
compilation. A faster wall-clock release is not claimed until a real workflow
run measures it.
