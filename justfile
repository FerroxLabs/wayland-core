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
test-ci:
    vx cargo nextest run --workspace --profile ci --no-fail-fast

# Run a single test by name
test-one NAME:
    vx cargo nextest run --workspace -E 'test({{ NAME }})'

# Show test output (debug failing tests locally)
test-verbose:
    vx cargo nextest run --workspace --profile default --no-capture

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
check-all: fmt-check lint test-ci hakari-verify audit

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
harness:
    vx cargo build --release -p wcore-cli
    vx cargo test -p wcore-cli --test harness_cli_surface --test harness_tui_flow
    vx cargo test -p wcore-cli --features harness-failure-injection \
        --test harness_failure_injection -- --test-threads=1

# ── W10A eval harness acceptance gate ─────────────────────────────────────
# Required to pass before F12 GEPA (W10B) can ship. Locked CLI invocation per
# W10A plan rev-2 LOCKED PUBLIC SURFACE.
eval-gate:
    vx cargo nextest run -p wcore-eval --features acceptance-gate acceptance_gate_meets_precision_recall_threshold --no-fail-fast --run-ignored only

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
