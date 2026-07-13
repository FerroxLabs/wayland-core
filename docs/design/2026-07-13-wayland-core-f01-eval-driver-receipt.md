# Wayland Core F01 Evaluation Driver Receipt

**Status:** implemented and code-verified

**Implementation source:** `a3a6bb6fde81f4991f5fd76e1e195fe26a347fb7` on `frontier/m0`

**Scope:** F01 only. This receipt proves selection, planning, execution, gating, and exact evaluated-byte identity. F02 owns hermetic execution and secret/process containment; F03 owns externally authoritative provenance and versioned evidence reports.

## 1. Delivered contract

`wayland-eval` now:

- selects canonical scenarios by identifier, category, or default suite;
- resolves provider plans with CLI-over-environment precedence, strict missing-provider behavior, and an offline `--dry` matrix;
- reports provider, approval posture, and target operating system for every planned and executed cell;
- skips unsupported operating systems leniently or rejects them under `--strict`;
- discovers or accepts one Core artifact, checks its full embedded source commit, seals it in private retained storage, and verifies its SHA-256 digest before and after every probe and scenario;
- executes a deterministic JSON-stream fixture and the real packaged Core binary through the same driver path;
- enforces scenario time, tool-step, cleanup, cost, canary, and aggregate budget gates;
- stops after a failed canary or exhausted conservative budget reservation and accounts for every unrun cell as `ABORTED`;
- exits nonzero when a hard evaluation gate fails; and
- writes the exact canonical console status stream atomically when `--output` is supplied.

## 2. Red-to-green sequence

The implementation is preserved as independent reproductions and fixes:

| Commits | Contract |
|---|---|
| `6ae2431`–`4d9daca` | Canonical catalog and CLI selection. |
| `13e071a`–`7dedca4` | Provider resolution and run planning. |
| `29b9a20`–`94064e6` | Full, non-truncated Core source identity. |
| `0cb57e7`–`9d67f34` | Exact artifact verification and locked digest dependency. |
| `7002f39`–`a1b928c` | Real fixture execution through the driver. |
| `4a0ffa3`–`3e7e8c2` | Packaged Core verification. |
| `3579402`–`68093e9` | Time, step, and cleanup runtime contracts. |
| `e042e98`–`48adc58` | Platform planning and posture metadata. |
| `87cb42a`–`a9ff655` | Offline provider matrix and dry planning. |
| `7355257`–`31bb66f` | Sealed evaluated artifact and per-run digest checks. |
| `00ce980`–`b8523c0` | Complete, finite, nonnegative cost evidence. |
| `4740057`–`eec8f4d` | Canary fail-fast and conservative budget gates. |
| `24f3232`–`a3a6bb6` | Atomic canonical output destination. |

## 3. Verification evidence

Verification ran only in the isolated Hetzner worktree, using pinned Rust `1.95.0` with locked/offline dependencies:

- all `wcore-eval-scenarios` tests passed: 133 executed, 4 skipped;
- focused CLI, artifact, provider, platform, runner, and cost regression suites passed;
- `cargo clippy -p wcore-eval-scenarios --all-targets --all-features -- -D warnings` passed;
- `cargo fmt --all -- --check` passed;
- a fresh debug `wayland-core` and `wayland-eval` were built from implementation source `a3a6bb6fde81f4991f5fd76e1e195fe26a347fb7`;
- Core reported `wayland-core 0.12.25 (source a3a6bb6fde81f4991f5fd76e1e195fe26a347fb7)`; and
- the sealed verifier accepted SHA-256 `a4be1835138d18cf4a8a464558e2e751aeb2b7d5e107df13ff3dde435b9e5fde` with version `0.12.25` and the exact expected source.

The digest proves which local bytes were evaluated. The embedded source identity is still self-attestation; it is not a signature or independent supply-chain authority.

## 4. Deferred boundaries

These are not F01 completion gaps:

| Owner | Deferred control |
|---|---|
| F02 | Remove credentials from argv/config; isolate inherited host state; bound and redact output; own descendant process trees; prove no residue. |
| F03 | Versioned JSON/JSONL, JUnit, Markdown, and signed or independently authoritative evidence receipts. |
| F04 | Portable deterministic provider, MCP, network, time, repository, and remote-executor fixtures. |
| F07 | Replace descriptive posture strings with the final typed authority profiles. |
| F28 | Native packaged certification on Linux, macOS, and Windows. |

F01 does not claim that a dirty source tree is reconstructible from its commit, that a self-reported commit is cryptographically authentic, or that spawned descendants are contained. Those claims remain forbidden until their owning tasks pass.
