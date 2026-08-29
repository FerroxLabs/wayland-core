# wayland-core justfile — run tasks with `vx just <recipe>`
# All commands route through `vx` so the correct tool versions are used.

# Cross-platform shell defaults for linewise recipes.
set shell := ["sh", "-cu"]
set windows-shell := ["pwsh", "-NoLogo", "-NoProfile", "-Command"]

# Default: list all recipes
default:
    @vx just --list

# ── Build ──────────────────────────────────────────────────────────────────
build:
    vx cargo build --workspace

build-release:
    vx cargo build --workspace --release

# ── Test ───────────────────────────────────────────────────────────────────

# Unit + integration tests with nextest (default profile — local dev)
#
# `scripts/fd-budget.sh` guarantees RLIMIT_NOFILE is large enough for
# `test-threads = "num-cpus"` before handing off, or refuses to run and names
# the resource. nextest holds ~3 pipe fds per concurrently-running test in the
# RUNNER process, so peak demand is ~3 x nproc; when that crosses the fd limit
# the spawn fails with EMFILE and nextest reports "exec failed" — which is
# counted like a test failure but means the test never ran, hits a different
# set of tests every run, and disappears under --test-threads=1. Measured
# 2026-07-29: 96 such failures at --test-threads=384, 0 real test failures.
# It is a no-op wherever the budget already fits.
#
# WINDOWS TAKES A SEPARATE RECIPE, NOT A PASS-THROUGH INSIDE THE SCRIPT.
# `is_windows()` in fd-budget.sh returns early on Windows, but that arm can
# only run if the script starts, and under `set windows-shell := pwsh` it never
# does: pwsh cannot execute a `.sh` file and dies with
# `Program 'fd-budget.sh' failed to run: ... The operation attempted is not
# supported`. That is a *recipe* failure, so nextest is never invoked, the JUnit
# report is never written, and the leg reports "the test step failed" while
# having run zero tests. Measured on CI run 30652437749 job 91228704700
# (2026-07-31) — the first Windows job in 40+ runs to get past clippy, which
# then died here in 0.4s. Windows has no RLIMIT_NOFILE governing process spawn,
# so there is nothing for the guard to do there anyway.
[unix]
test:
    scripts/fd-budget.sh vx cargo nextest run --workspace --profile default

[windows]
test:
    vx cargo nextest run --workspace --profile default

# Unit + integration tests with nextest (CI profile — used in GitHub Actions)
#
# --no-fail-fast: run EVERY test even after one fails, so a CI cycle
# surfaces all failures in one pass instead of one-per-cycle. Added
# v0.8.6 round 18 after 6 sequential Windows failures (rounds 11-17)
# cost a day chasing one root cause at a time — each fix exposed
# the next first-deterministic Windows failure. Costs an extra few
# minutes on cold cache when something fails; saves orders of
# magnitude on iteration loops.
#
# scripts/fd-budget.sh, and why Windows gets its own recipe rather than
# relying on the script's internal pass-through: see the `test` recipe above.
[unix]
test-ci:
    scripts/fd-budget.sh vx cargo nextest run --workspace --profile ci --no-fail-fast

[windows]
test-ci:
    vx cargo nextest run --workspace --profile ci --no-fail-fast

# Grade a local `just test-ci` run for retried failures (wayland#1169).
#
# `[profile.ci] retries = 2` means a test that fails and then passes on a retry
# is counted as PASSED and the run concludes SUCCESS — measured cost: the #1155
# data-loss race failed 14 of 48 runs (29 %) at `--retries 0` while the same
# defect reported as `FLAKY 2/3` inside a passing run. This is the same gate the
# aggregate `report` job runs, pointed at the JUnit your last local CI-profile
# run left behind, so a flake can be seen before it is pushed.
#
# Reads target/nextest/ci/junit.xml, which only `--profile ci` writes. With no
# such run in the tree it grades nothing and says so — see the comment in
# .github/scripts/grade-retry-flakes.sh about why absence is not this gate's
# failure to report.

# Grade a local CI-profile run for retried failures (wayland#1169)
flake-gate:
    EVIDENCE_DIR=target/nextest/ci bash .github/scripts/grade-retry-flakes.sh

# Compare a local CI-profile run's failing-test SET against the named
# allowlist (wayland-core#367).
#
# This is the one to run before calling an integration branch clean. `N failed`
# is not `the known N failed`: a red-arm instrument reached integ/f13 and
# survived three commits because the count matched and nobody opened the names.
# Reads target/nextest/ci/junit.xml, which only `--profile ci` writes.
failing-set-gate:
    EVIDENCE_DIR=target/nextest/ci bash .github/scripts/grade-failing-set.sh

# Run a single test by name
test-one NAME:
    vx cargo nextest run --workspace -E 'test({{ NAME }})'

# Show test output (debug failing tests locally)
test-verbose:
    vx cargo nextest run --workspace --profile default --no-capture

# Regenerate in memory, reject contract drift, then replay the checked corpus.
desktop-contract-check:
    vx cargo run -p wcore-protocol --bin wcore-contract -- check
    vx cargo nextest run -p wcore-protocol --test desktop_contract_corpus --test desktop_contract_adversarial

# ── E2E Tests ──────────────────────────────────────────────────────────────
# Requires env vars: ANTHROPIC_API_KEY and/or OPENAI_API_KEY
# Uses the dedicated e2e nextest profile (sequential, long timeout, no retry)
test-e2e:
    vx cargo nextest run --workspace --profile e2e --test e2e

test-e2e-anthropic:
    vx cargo nextest run -p wcore-agent --profile e2e --test e2e -E 'test(anthropic)'

test-e2e-openai:
    vx cargo nextest run -p wcore-agent --profile e2e --test e2e -E 'test(openai)'

# ── Acceptance Tests (evolution feature validation) ───────────────────────
# Requires env vars: OPENAI_API_KEY and/or AWS_PROFILE + CLAUDE_CODE_USE_BEDROCK=1
# Reuses the e2e nextest profile (sequential, long timeout, no retry)
test-acceptance:
    vx cargo nextest run -p wcore-agent --profile e2e --test acceptance

test-acceptance-memory:
    vx cargo nextest run -p wcore-agent --profile e2e --test acceptance -E 'test(memory)'

test-acceptance-compact:
    vx cargo nextest run -p wcore-agent --profile e2e --test acceptance -E 'test(compact)'

# ── Lint / Format ─────────────────────────────────────────────────────────
lint:
    vx cargo clippy --workspace --all-targets -- -D warnings

lint-fix:
    vx cargo fix --allow-dirty --allow-staged
    vx cargo clippy --fix --workspace --all-targets --allow-dirty --allow-staged -- -D warnings

fmt:
    vx cargo fmt --all

[unix]
fmt-check:
    vx cargo fmt --all -- --check

# On Windows, `cargo fmt --all` builds a rustfmt command line that exceeds the
# OS command-line length limit on this 54-crate workspace and fails with
# os error 206 ("The filename or extension is too long") — a tooling limit,
# not a formatting problem. rustfmt's output is platform-independent, so the
# Unix + macOS fmt gates already fully enforce formatting; re-checking it on
# Windows adds nothing. Skip here to keep the Windows runner green without the
# cmdline-limit failure.
[windows]
fmt-check:
    @echo "fmt-check skipped on Windows (formatting is platform-independent and enforced by the Unix/macOS gates; cargo fmt --all hits os error 206 on this 54-crate workspace)."

# ── Workspace-hack (cargo-hakari) ─────────────────────────────────────────
hakari-generate:
    vx cargo hakari generate

hakari-verify:
    vx cargo hakari verify

# ── Security ──────────────────────────────────────────────────────────────
audit:
    vx cargo audit

# Execute the dependency policy declared in deny.toml (F29-02, closes census
# finding F29-CEN-04). `audit` above runs cargo-audit, which is the RUSTSEC
# advisory scanner ONLY — it evaluates no license, no ban and no source
# registry. deny.toml has declared a strict four-section policy since v0.6.2
# and, measured at 2fd771d2, NOTHING had ever executed it: cargo-deny appeared
# in zero files under .github/ and zero times in this justfile, leaving 1,017
# crates unevaluated against the licence allowlist.
#
# CHAINED into `check-all` since 2026-07-29 (lane 29-deny). It was deliberately
# left unchained while its first execution exited 5 — a red gate in the
# aggregate check would have broken every concurrent lane on a policy failure
# unrelated to their work (29-02-CLEANROOM-RESULTS.md). The verdict is now
# `advisories ok, bans ok, licenses ok, sources ok`, exit 0, so the condition
# that recipe's own comment set for chaining it — "chain it only once the
# verdict is clean" — is met.
#
# Local chaining is not merely a convenience duplicate of CI: the CI job in
# `.github/workflows/supply-chain.yml` sits behind a path-relevance guard and
# SKIPS on PRs that do not touch the policy paths, so on those branches this
# recipe is the only place the policy runs at all.
deny:
    vx cargo deny --manifest-path Cargo.toml check

# Re-derive every advisory suppression's parent trace from Cargo.lock and fail
# when the documented trace disagrees with the real graph (F29-02-H1).
#
# `audit` and `deny` above answer "is this advisory muted?". Neither checks
# whether the JUSTIFICATION for muting it is true. That is the gap F29-02-H1
# came through: `.cargo/audit.toml` silenced RUSTSEC-2026-0194/0195 on a stated
# "sole path" of quick-xml <- plist <- syntect <- wcore-cli when the lockfile had
# THREE direct parent edges. The reachability argument was correct about the path
# it named — and that was the only safe one of the three. 0194 was reachable from
# a user-supplied .xlsx: calamine 0.26.1's `next_cell()` calls
# `BytesStart::attributes()` with the default duplicate check on.
#
# It is not a one-off. Two more traces were found wrong by hand on 2026-07-29:
# `paste` named ratatui as a puller (false) and omitted the tokenizers root, and
# `rustls-pemfile` claimed "only via bollard", naming one of two direct edges.
# Three instances, three human re-derivations. This makes it mechanical.
#
# Runs the self-test first so the gate is proved able to fail before it is
# trusted to pass, and prints SUPPRESSIONS_EXAMINED so a run that checked
# nothing is distinguishable from a run that checked everything.
verify-suppressions:
    python3 scripts/verify-suppression-traces.py --self-test
    python3 scripts/verify-suppression-traces.py

# ── Coverage ──────────────────────────────────────────────────────────────
coverage:
    vx cargo llvm-cov nextest --workspace --profile ci --lcov --output-path lcov.info

# ── Release ───────────────────────────────────────────────────────────────
wcore_version := `vx cargo pkgid -p wcore-cli | sed 's/.*#//'`

version:
    @echo '{{ wcore_version }}'

# ── Clean ─────────────────────────────────────────────────────────────────
clean:
    vx cargo clean

# ── Pre-push gate (lint-fix, format, auto-commit fixes, test, then push) ─
push *ARGS: lint-fix fmt _auto-commit-fixes test
    git push {{ ARGS }}

_auto-commit-fixes:
    #!/usr/bin/env bash
    if [ -n "$(git diff --name-only)" ]; then
        git add -A
        git commit -m "chore: auto-commit lint/fmt fixes in just push recipe"
    fi

# ── All checks (mirrors CI exactly) ───────────────────────────────────────
# `deny` added 2026-07-29 (lane 29-deny) — see the `deny` recipe above for why
# it was held out until the verdict went green.
# `verify-suppressions` added 2026-07-30 (lane f29-h1-advisories): audit and deny
# check whether an advisory is muted; this checks whether the stated reason for
# muting it is TRUE against the real graph. See the recipe above.
# `check-no-personal-identifiers` added 2026-08-02 (lane identifier-scrub): a
# ~3s pure text scan, no toolchain, so it runs first and costs nothing.
# `ledger-check` added 2026-08-29 (lane ledger): the OFFLINE arm only. The
# coverage arm needs `gh` against two repos and lives in `ledger-check-live`,
# because a network dependency inside `check-all` buys flakiness for nothing.
# `release-readiness-selftest` added 2026-08-29 (lane f13-relgate): the
# SELF-TEST only, deliberately not the gate. The gate is red by design while
# any defect is open, so putting it here would make every in-progress lane
# red, and a gate everyone bypasses is a gate nobody reads. The gate itself
# runs on the release path (`release-readiness-live`, and the
# `prepare-release` job in release.yml). This arm only proves the gate can
# still fail — which is the half that rots silently between releases.
check-all: check-no-personal-identifiers check-model-limits check-windows-attribution ledger-check release-readiness-selftest fmt-check lint test-ci hakari-verify audit deny verify-suppressions

# ── User-flow harness (CLI + TUI + failure injection) ────────────────────
# Drives the COMPILED wayland-core binary the way a user does:
#   Layer 1 — CLI surface (subcommands, stdout/stderr/exit codes)
#   Layer 2 — TUI flow via PTY (chrome, tab nav, /exit, resize)
#   Layer 3 — failure injection (wedged MCP, Ctrl+C mid-turn)
# Layer 3 is feature-gated because it waits out a real 30s MCP
# connect timeout. The ctrl_c sub-test in Layer 3 skips cleanly when
# neither ANTHROPIC_API_KEY nor API_KEY is set.
#
# All three layers expect a pre-built release binary in target/release/
# (release_binary_smoke.rs depends on it via WCORE_PREBUILD_REQUIRED).
#
# nextest, not `cargo test`: all three layers are file-level gated
# (`harness_tui_flow` is `#![cfg(unix)]`, `harness_failure_injection` is
# `#![cfg(feature = "harness-failure-injection")]`), so under the wrong
# platform or a dropped feature flag the binary compiles EMPTY and `cargo test`
# prints `test result: ok. 0 passed` and exits 0. `no-tests = "fail"` in
# .config/nextest.toml turns that into a hard failure.
#
# The recipe is SPLIT by platform rather than suppressed: `harness_tui_flow` is
# legitimately absent on Windows, so naming it there would be a false positive.
# Declaring the platform in the invocation — the pattern this justfile already
# uses for `f01-packaged-driver-gate` — keeps the emptiness honest instead of
# tolerated. Measured on Linux: 28 tests run, 28 passed.
[unix]
harness:
    vx cargo build --release -p wcore-cli
    vx cargo nextest run --no-tests=fail -p wcore-cli --test harness_cli_surface --test harness_tui_flow
    vx cargo nextest run --no-tests=fail -p wcore-cli --features harness-failure-injection \
        --test harness_failure_injection --test-threads=1

# Windows: `harness_tui_flow` is `#![cfg(unix)]` and does not exist here.
[windows]
harness:
    vx cargo build --release -p wcore-cli
    vx cargo nextest run --no-tests=fail -p wcore-cli --test harness_cli_surface
    vx cargo nextest run --no-tests=fail -p wcore-cli --features harness-failure-injection --test harness_failure_injection --test-threads=1

# ── W10A eval harness acceptance gate ─────────────────────────────────────
# Required to pass before F12 GEPA (W10B) can ship. Locked CLI invocation per
# W10A plan rev-2 LOCKED PUBLIC SURFACE.
eval-gate:
    vx cargo nextest run -p wcore-eval --features acceptance-gate acceptance_gate_meets_precision_recall_threshold --no-fail-fast --run-ignored only

# F01 E3: package the real Core CLI, then drive success and hard failure through
# wayland-eval while binding the run to the clean source commit and binary bytes.
[unix]
f01-packaged-driver-gate:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -n "$(git status --porcelain --untracked-files=normal)" ]; then
        echo "F01 packaged-driver gate requires a clean source tree" >&2
        exit 2
    fi
    export WAYLAND_BUILD_SOURCE_SHA="$(git rev-parse HEAD)"
    vx cargo build --locked -p wcore-cli --bin wayland-core
    target_dir="${CARGO_TARGET_DIR:-target}"
    if [[ "$target_dir" != /* ]]; then
        target_dir="$PWD/$target_dir"
    fi
    export WCORE_EVAL_BIN="$target_dir/debug/wayland-core"
    # nextest, not `cargo test`: packaged_driver_gate.rs is
    # `#![cfg(feature = "packaged-driver-gate")]`. Drop or rename that feature
    # and `cargo test` reports `ok. 0 passed` with rc=0 — a packaged-boundary
    # proof that proved nothing. Measured: rc=0 under cargo test, rc=4 under
    # nextest on the identical empty binary.
    vx cargo nextest run --no-tests=fail --locked -p wcore-eval-scenarios \
        --features packaged-driver-gate --test packaged_driver_gate

[windows]
f01-packaged-driver-gate:
    $dirty = git status --porcelain --untracked-files=normal; if ($dirty) { Write-Error "F01 packaged-driver gate requires a clean source tree"; exit 2 }; $env:WAYLAND_BUILD_SOURCE_SHA = (git rev-parse HEAD).Trim(); vx cargo build --locked -p wcore-cli --bin wayland-core; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }; $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "target" }; $env:WCORE_EVAL_BIN = [System.IO.Path]::GetFullPath((Join-Path $target "debug/wayland-core.exe")); vx cargo nextest run --no-tests=fail --locked -p wcore-eval-scenarios --features packaged-driver-gate --test packaged_driver_gate; exit $LASTEXITCODE

# ── Silent-pass CI gate (Wave 0) ───────────────────────────────────────────
# Fails if any functional todo!() exists in the eval-scenarios assertion/trace
# paths. Belt-and-suspenders: the primary gate is #![deny(clippy::todo)] in
# those source files; this grep catches any accidental bypass (e.g. allow attr).
# Excludes doc-comment lines (//! and //) so doc mentions of todo!() don't
# false-fire. grep output format is "file:line:content", so we filter on the
# content portion after the second colon.
# Run: `just check-no-assertion-todos`
check-no-assertion-todos:
    #!/usr/bin/env sh
    if grep -rn 'todo!' \
        crates/wcore-eval-scenarios/src/assertions.rs \
        crates/wcore-eval-scenarios/src/trace.rs \
        | grep -v '://!' | grep -v '://'; then
        echo "FAIL: todo!() found in eval-scenarios assertion paths — silent-pass gate tripped"
        exit 1
    fi
    echo "OK: no todo!() in eval-scenarios assertion paths"

# ── Criteria-ledger gate ──────────────────────────────────────────────────
# `.planning/ledger/<repo>-<number>.md` is one file per open issue, on BOTH
# trackers, saying what must be TRUE for that issue to close and pointing each
# `met` claim at something a machine can resolve. It exists because handoffs
# in this repo are narratives of what was DONE, so every session re-derives
# "is this done?" from prose and gets a different answer: v0.13.10 shipped
# claiming 22 issues closed and grading found 9.
#
# The largest contributor was structural. The sweep that produced 0.13.9
# filtered `FerroxLabs/wayland` on `area:core`, and the whole second tracker
# (`FerroxLabs/wayland-core`, 17 open issues) was invisible for a full
# release. Nothing went red, because nothing could.
#
# TWO recipes, for the same reason `check-model-limits` has two:
#   * `ledger-check`      — self-test THEN the structural gate. No network.
#     Catches the rot (a `met` criterion whose test was deleted), a malformed
#     entry, a `blocked` owned by core, and scanning nothing. Chained into
#     `check-all`, so it costs a second and cannot be shadowed.
#   * `ledger-check-live` — adds tracker COVERAGE and ledger/GitHub
#     DIVERGENCE. Needs `gh` with read access to BOTH repos. This is the arm
#     that catches a tracker going missing, so run it before any release.
#
# The offline arm prints, in as many words, that it did NOT check coverage.
# A skip that reads as a pass is the defect class this repo keeps finding.
# `--self-test` builds throwaway ledgers in a temp dir and proves the gate
# fires on each defect and stays silent on the control, both directions.
# Run: `just ledger-check`
ledger-check:
    python3 scripts/check-criteria-ledger.py --self-test
    python3 scripts/check-criteria-ledger.py --offline

# Run: `just ledger-check-live` — the coverage arm; needs gh on both trackers
ledger-check-live:
    python3 scripts/check-criteria-ledger.py --self-test
    python3 scripts/check-criteria-ledger.py

# ── Release-readiness gate ────────────────────────────────────────────────
# `ledger-check` above gates the BOOKKEEPING: a malformed entry, a `met` with
# no evidence, evidence that no longer resolves, a `blocked` owned by core, an
# open issue with no ledger file. Every one of those asks whether the RECORD is
# honest. NONE of them ask whether the WORK is done.
#
# On the tree this recipe was added to, 67 criteria were `not-met` and owned by
# `core`, and `just ledger-check` was completely green. It has never been
# possible for this repo to go red because a release was INCOMPLETE — only
# because a ledger lied about it. That is the mechanism behind every partial
# release here, v0.13.10 included: 22 issues claimed closed, 9 met on grading.
#
# This gate refuses to cut a release while any in-scope DEFECT still has
# core-owned work outstanding. Errors, problems and issues block; feature
# requests do not, and the split is a required `kind: defect|feature` field in
# each ledger file rather than a GitHub label, so the offline arm needs no
# network. A MISSING `kind` is a hard failure: a field that defaults is a field
# nobody ever types, and it would default into whichever bucket was convenient.
#
# It also refuses a remainder that was handed out and then lost. A criterion
# `blocked` or `not-met` under desktop/flux/maintainer must carry a
# `handoff: <owner>/<repo>#<number>` naming the ticket that now owns it. A
# ticket ends CLOSED or DECOMPOSED; "partial" is a ticket nobody split, and an
# untracked remainder is what makes a partial invisible.
#
# DELIBERATELY NOT IN `check-all`. Every lane runs `check-all`, and this gate is
# red BY DESIGN for as long as any defect is open — which is always, mid-cycle.
# A gate that is red on every in-progress lane gets bypassed, then gets ignored,
# then gets deleted, and that is how a ratchet dies. It belongs on the release
# path, where red means "do not cut", and it is wired into the `prepare-release`
# job in .github/workflows/release.yml so a FAIL actually stops the publish
# instead of being advisory. The SELF-TEST, by contrast, is cheap and always
# meaningful — see `release-readiness-selftest` below, which ci.yml runs, so the
# gate cannot rot in between releases.
#
# TWO recipes, the same split as `ledger-check`:
#   * `release-readiness`      — structure only, no network. Prints in as many
#     words that it did NOT resolve handoff targets and did NOT corroborate
#     `kind:` against tracker labels. A skip that reads as a pass is the exact
#     defect class this repo keeps finding.
#   * `release-readiness-live` — adds both. Fails when a `handoff:` names an
#     issue that is closed or does not exist, and when an entry marked
#     `kind: feature` is labelled `bug` on its tracker — the one direction of
#     misclassification that shrinks the blocking set.
#
# Run: `just release-readiness`
release-readiness:
    python3 scripts/check-release-readiness.py --self-test
    python3 scripts/check-release-readiness.py --offline

# Run: `just release-readiness-live` — before cutting. Needs gh on both trackers
release-readiness-live:
    python3 scripts/check-release-readiness.py --self-test
    python3 scripts/check-release-readiness.py

# Proves the gate can FAIL. No network, and it does not read .planning/ledger at
# all, so it is safe in `check-all` where the gate itself is not: it says nothing
# about whether a release is ready, only that the thing which would say so still
# works. That is the half which rots silently between releases.
# Run: `just release-readiness-selftest`
release-readiness-selftest:
    python3 scripts/check-release-readiness.py --self-test

# ── Vacuous-green gate ─────────────────────────────────────────────────────
# `cargo nextest` fails closed on a zero-test run (`no-tests = "fail"` in
# .config/nextest.toml). `cargo test` does NOT: measured on this checkout, a
# feature-gated target built without its feature prints `test result: ok.
# 0 passed` and exits 0. 44 test binaries here carry a file-level `#![cfg(...)]`
# and can compile to empty. This fails if a new bare `cargo test` appears in the
# justfile, a workflow, or a script without `--no-run` or an explicit
# executed-count assertion (`vacuity-checked:`).
# Run: `just check-no-vacuous-cargo-test`
check-no-vacuous-cargo-test:
    python3 scripts/check-no-vacuous-cargo-test.py --self-test
    python3 scripts/check-no-vacuous-cargo-test.py

# ── Pre-publish personal-identifier gate ──────────────────────────────────
# NOT a credential gate — credentials were swept three times and are clean.
# This one stops the maintainer's PERSONAL identifiers accumulating in
# committed evidence ahead of a public release: Matrix MXIDs and (joinable)
# room IDs transcribed out of live-channel proof runs, real phone numbers, and
# personal email. Shape-matched with a placeholder allowlist, so a NEW personal
# handle nobody denylisted still fires. Absolute home paths split two ways:
# inside `.planning/` they are REPORT-ONLY (2967 of them — evidence transcribes
# what a real machine printed, and blocking there goes red on any lane that
# merges evidence, which is how a ratchet dies); everywhere else they BLOCK
# against a baseline of 31, because a hardcoded /Users/<name> in source, CI or
# docs breaks on every other machine.
# `--self-test` proves both directions before it scans: it FIRES on the real
# pre-redaction values, stays SILENT on redacted evidence + fixture corpora, and
# drives the real scanner over throwaway git repos to prove a routine new
# evidence file does NOT fail the gate while one home path in crates/ does.
# Run: `just check-no-personal-identifiers`
check-no-personal-identifiers:
    python3 scripts/check-no-personal-identifiers.py --self-test
    python3 scripts/check-no-personal-identifiers.py

# ── Model-limits freshness gate ───────────────────────────────────────────
# `crates/wcore-config/src/limits.rs` is hand-maintained, and #165 is what
# happens when the world ships a model and this table does not hear about it:
# no error, just a silently wrong window and a run that dies mid-flight.
# `every_routed_catalog_model_has_a_known_window` covers our own catalog; this
# covers the catalogue moving without us.
#
# TWO recipes on purpose:
#   * `check-model-limits`           — SELF-TEST ONLY. No network, ~0.1s. Chained
#     into `check-all` so the checker itself cannot rot between releases.
#   * `check-model-limits-freshness` — self-test THEN the live models.dev scan.
#     Needs the network, so it runs at RELEASE time (release.yml
#     `prepare-release`), not on every CI run: a third-party catalogue in the
#     main test path buys flakiness for nothing.
#
# FAILS when an in-scope first-party model has no arm or an arm over-claims.
# REPORTS (exit 0) a brand-new family — failing a release on someone else's
# launch is not this gate's call, but the release owner must see it. If
# models.dev is unreachable it prints a SKIPPED banner and exits 0; the banner
# says "THIS IS NOT A PASS" in as many words, because a skip that reads as a
# pass is the defect class this repo keeps finding.
# Run: `just check-model-limits-freshness`
check-model-limits:
    python3 scripts/check-model-limits-freshness.py --self-test

check-model-limits-freshness:
    python3 scripts/check-model-limits-freshness.py --self-test
    python3 scripts/check-model-limits-freshness.py
# ── Windows-attribution gate (#1146) ──────────────────────────────────────
# A Windows verdict is worth nothing if it cannot be attributed to a tree. The
# Windows pool is two runner SERVICES on one host plus the hosted pool, the
# failure set churns between them on the same tree, and until this gate landed
# not one Windows job recorded which executor served it — so #1146's four runs
# across three executors could neither confirm a red nor earn a green.
# Two rules, both pure text scans: every Windows job in the Windows test
# workflows records its executor and fails closed when it cannot, and the three
# tests whose verdict churns are not laundered by `[profile.ci] retries = 2`.
# `--self-test` proves both directions (and the Windows/not-Windows classifier)
# before it scans, so a checker that has quietly stopped matching fails loudly
# instead of passing everything.
# Run: `just check-windows-attribution`
check-windows-attribution:
    python3 scripts/check-windows-attribution.py --self-test
    python3 scripts/check-windows-attribution.py

# Needs a built workspace, which is why it is not in `check-all` or the CI lint
# job, and why it is a separate recipe: it resolves every
# `[[profile.ci.overrides]]` filterset with `cargo nextest list` and checks the
# three #1146 tests really land at retries=0, with no earlier override winning
# first. Run it after renaming or moving any of the three.
# Run: `just check-windows-attribution-live` — proves the quarantine is not vacuous
check-windows-attribution-live:
    python3 scripts/check-windows-attribution.py --with-nextest
    # The #1146 red arm runs through this harness, and a misparsed target
    # selects zero tests and grades NOTRUN rather than failing, so its parser
    # is checked here too.
    python3 scripts/flake-ledger.py --self-test

# ── P0 smoke gate (pre-release) ───────────────────────────────────────────
# Runs the live P0 smoke suite (crates/wcore-cli/tests/smoke_p0.rs) via
# scripts/smoke.sh: hermetic engine-behavior checks that MUST be green to ship,
# plus the 7 currently-RED gap checks (D002/D009/D010/D011/D012/D013/D015) and
# the interactive-pending checks, REPORTED (never silently skipped). The runner
# exits non-zero if any hard-gate check fails. Pass SMOKE_LIVE=1 +
# ANTHROPIC_API_KEY to additionally run the one real-key happy path.
# Run: `just smoke`
smoke:
    scripts/smoke.sh

# Wayland Proving Ground — deterministic bug-sweep spine (unix, hermetic, $0).
# Drives the REAL binary over a PTY across throw-away homes and asserts
# deterministic invariants (config persistence, content reachability, replay
# determinism), plus the network-free provider-detection registry invariant and
# the build-provenance (stale-binary) check.
# Run: `just proving-ground`
proving-ground:
    vx cargo nextest run -p wcore-cli --test proving_ground --test build_provenance
    vx cargo nextest run -p wcore-providers --test detection_registry

# ── THE PLAN ───────────────────────────────────────────────────────────
# `.planning/THE-PLAN.md` is GENERATED, never written by hand. Every handoff this
# project produced was a narrative of what somebody did rather than a record of
# what is true, so each session re-derived "what is done" from prose and got a
# different answer: v0.13.10 shipped claiming 22 issues closed and grading found
# 9. A hand-maintained plan is that same failure with better formatting.
#
# It joins three sources and has no facts of its own: `.planning/ledger/` for
# criterion STATE, `plan-verification.json` for INDEPENDENT verification, and
# `PLAN-ROUTING.json` for ASSIGNMENT.
#
# `plan-check` FAILS on an unrouted criterion. That is the point: core#113 and
# wayland#863 sat outside every lane in this cycle purely because nothing forced
# them to be assigned, and an unrouted criterion is how work goes missing.
#
# `met` is NOT `done`. A criterion is DONE only when an independent adversarial
# verifier confirmed the lane; until then it renders CLAIMED, because a
# criterion written thin reads `met` while the reported bug is still live.
# Run: `just plan`
plan:
    python3 scripts/render-plan.py

# Run: `just plan-check` — fails on an unrouted criterion or outstanding defect work
plan-check:
    python3 scripts/render-plan.py --check
