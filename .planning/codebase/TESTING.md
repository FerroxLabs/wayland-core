# Testing Patterns

**Analysis Date:** 2026-07-23

## Test Framework

**Runner:**
- `cargo nextest` — profiles configured in `.config/nextest.toml` (repo-root-level; not read directly this pass but referenced by `justfile`). All invocations go through `vx` (the pinned toolchain proxy from `vx.toml`).

**Run Commands (from `justfile`):**
```bash
just test                # nextest, --profile default (local dev, friendly output)
just test-verbose        # --profile default --no-capture (debug a failing test)
just test-one NAME       # single test by name: -E 'test(NAME)'
vx cargo nextest run --workspace --profile ci --no-fail-fast   # CI profile
just test-e2e            # --profile e2e --test e2e (requires ANTHROPIC_API_KEY/OPENAI_API_KEY)
just test-e2e-anthropic  # -p wcore-agent --profile e2e --test e2e -E 'test(anthropic)'
just test-e2e-openai     # -p wcore-agent --profile e2e --test e2e -E 'test(openai)'
just test-acceptance     # -p wcore-agent --profile e2e --test acceptance (evolution feature validation)
just desktop-contract-check  # regenerates + rejects contract drift, replays checked corpus
```

**Do NOT run `cargo`/`clippy`/`nextest` on this Mac worktree.** Per project convention this repo compiles ONLY on the Hetzner self-hosted runner (`hetzner-dsm`, `/root/wayland`) and the CI self-hosted Windows runner (`.github/workflows/ci.yml:45`, `[self-hosted, Windows, X64, msvc]` — chosen over `windows-latest` to avoid a toolchain-drift cascade and to keep a warm `target/` cache). `cargo fmt` is the one exception that does work locally on the Mac.

## nextest Profiles (`.config/nextest.toml`)

**`[profile.default]`** — local dev:
- `failure-output = "immediate"`, `success-output = "never"`, retries = 1 (absorbs local flakiness).
- `slow-timeout = { period = "30s", terminate-after = 2 }`, `test-threads = "num-cpus"`.
- Per-test override: the "full-engine-build cohort" — every test constructing a real `AgentBootstrap` (`bootstrap_*` binaries in `wcore-agent`, `bootstrap_with_ollama_*`, `*_is_registered_in_bootstrap`) or the full tool registry (`registry_inventory_snapshot` in `wcore-cli`) — gets `slow-timeout = 90s` because it legitimately runs 38-55s under load, matching CI's budget instead of false-failing a busy laptop (documented incident: 2026-06-01, a stray busy-loop pushed 10 tests past 60s and fail-fast aborted the whole run).

**`[profile.ci]`**:
- `failure-output = "immediate-final"`, shows all statuses at the end, retries = 2.
- `slow-timeout = { period = "90s", terminate-after = 2 }` (bumped from 60s after CI run 25950354044 — Ubuntu cold-cache pushes).
- Override: `scripted_run_writes_expected_markdown` (W12 tool-token bench) gets 180s/no-retry — shells to a pre-built `tool_token_bench` binary; retrying a slow cargo-run only compounds.
- Override: `release_binary_*` tests get 180s/no-retry — Windows mandatory file locks make target/ checks slower than Linux/macOS (CI run 25953795604).

**`[profile.e2e]`** — real API calls:
- `retries = 0` (failures are real), `slow-timeout = 120s`, **`test-threads = 1`** (sequential — avoid hammering provider APIs). Requires live `ANTHROPIC_API_KEY`/`OPENAI_API_KEY`.

**`[profile.eval]`** — used by `wcore-eval-scenarios` (`just eval`):
- `retries = 2` (absorbs transient provider noise, per cross-audit M-3), `slow-timeout = 300s`, `test-threads = 1` (serialize to avoid cross-provider rate-limit contamination, per cross-audit M-6).

## Test File Organization

| Location | What goes there |
|----------|------------------|
| Inline `#[cfg(test)]` in each `.rs` file | Unit tests for that module's internal logic |
| `crates/<crate>/tests/*.rs` | Integration tests for that crate's public API/functional requirements |

**Naming (integration tests):** one file per scenario/feature area, descriptive snake_case matching the behavior under test — not the module under test. Examples from `crates/wcore-tools/tests/`:
- `legacy_execute_path_validation_test.rs`, `file_cache_test.rs`, `sandbox_symlink_test.rs`, `bash_sandbox_routing_test.rs`, `bash_credential_exfil_test.rs`, `git_commit_message.rs`, `edit_write_cache_test.rs`, `cancel_subprocess_test.rs`, `script_e2e.rs`, `tool_description_test.rs`.

Note the naming already signals hostile/adversarial coverage as a first-class category, not an afterthought: `bash_credential_exfil_test.rs`, `sandbox_symlink_test.rs`, `legacy_execute_path_validation_test.rs`. Integration tests are written from the spec/functional requirement, not from reading the implementation — per AGENTS.md.

**Unit vs integration split:** unit tests (`#[cfg(test)]` inline) target internal logic/code paths; integration tests target functional requirements and the public API surface. Every test must verify a meaningful behavior or edge case — no trivial happy-path-only assertions (AGENTS.md rule).

## Hostile / Adversarial Test Naming Convention

Tests that specifically probe security boundaries or malicious/adversarial inputs use explicit `_exfil`, `_bypass`, `_injection`, `_adversarial` naming so their intent is unambiguous at a glance, e.g.:
- `crates/wcore-tools/tests/bash_credential_exfil_test.rs`
- `crates/wcore-protocol` — `desktop_contract_adversarial` (referenced in `justfile:48`, run alongside `desktop_contract_corpus` by `just desktop-contract-check`)
- `crates/wcore-agent/tests/dangerous_lease_e2e_test.rs` (dangerous-mode lease boundary)

## Native Proof Harnesses (`scripts/`)

The workspace supplements Rust tests with standalone native proof scripts that exercise built artifacts end-to-end rather than in-process:

- `scripts/f20-native-macos-proof.sh` (8.0K) — native macOS proof harness for the F20 feature set.
- `scripts/f20-native-uat-proof.mjs` (14.5K) + `scripts/f20-native-uat-proof.test.mjs` (11.9K) — Node-based UAT proof harness with its own test file.
- `scripts/f20-native-windows-proof.ps1` (4.2K) — PowerShell equivalent for Windows.
- `scripts/wayland-e2e-real-workload.sh` (14.1K) — real-workload e2e smoke against a built binary.
- `scripts/wayland-e2e-smoke.sh` (9.5K) — lighter e2e smoke script.
- `scripts/wayland-e2e-windows-soak.ps1` (7.2K) — Windows soak test, referenced by `.github/workflows/nightly-windows-soak.yml`.
- `scripts/smoke.sh` (5.6K) — general smoke script.
- `scripts/f11-proof.py` (21.1K) — Python-based proof harness for an earlier phase (F11).

These proof harnesses are invoked from dedicated CI workflows (`.github/workflows/e2e.yml`, `.github/workflows/nightly-windows-soak.yml`) rather than from `cargo nextest` directly — they validate built release/debug binaries against real or simulated workloads, not source-level unit/integration behavior.

## Coverage

**Tooling:** `cargo llvm-cov nextest` (`justfile:113`): `vx cargo llvm-cov nextest --workspace --profile ci --lcov --output-path lcov.info`.

## Environment / Build Constraints

- **This repo compiles ONLY on the Hetzner self-hosted runner** (`hetzner-dsm`, path `/root/wayland`) for local dev, and on the self-hosted Windows CI runner for Windows CI — never on the Mac used for planning/mapping work. `cargo fmt` is the sole exception that runs correctly on the Mac.
- CI matrix (`.github/workflows/ci.yml`) covers macOS, Linux (`ubuntu-latest`), and Windows (`[self-hosted, Windows, X64, msvc]`) — the self-hosted Windows runner keeps a warm `target/` cache and avoids `windows-latest` toolchain drift; a known intermittent issue is a locked-file cleanup on that runner (`ci.yml:69`).
- E2E and acceptance suites require live provider credentials (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or `AWS_PROFILE` + `CLAUDE_CODE_USE_BEDROCK=1`) and are not run as part of the default `just test` path.
- Additional CI workflows: `bench-regression.yml`, `marketplace-drift.yml`, `mutants-nightly.yml` (mutation testing), `osv-scan.yml` (dependency vuln scan), `release-please.yml`, `release.yml`.

---

*Testing analysis: 2026-07-23*
</content>
