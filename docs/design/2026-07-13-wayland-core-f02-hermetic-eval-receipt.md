# Wayland Core F02 Hermetic Evaluation Receipt

**Status:** portable hermetic/secret contract implemented; Linux authoritative process-tree path code-verified

**Implementation source:** `c642ce40dd2f556505d14204e0defe90a3503898` on `frontier/m0`

**Scope:** F02 only. This receipt proves host-state isolation, one-use credential delivery, bounded/redacted capture, setup/cleanup dispatch, artifact scanning, and Linux cgroup-v2 descendant cleanup. F03 owns the versioned authoritative receipt schema. F05/F28 own native macOS and Windows execution authority and release evidence.

## 1. Delivered contract

The evaluator now:

- clears the candidate environment and rebuilds it from an explicit allowlist with isolated home, config, data, cache, state, runtime, temp, Git, locale, terminal, and tool-path values;
- keeps provider credentials out of generated TOML, inherited environment variables, and credential values in argv;
- delivers a credential through a private mode-`0600` one-use file whose path is passed by hidden CLI option, then consumes and removes it before Core bootstrap;
- applies the same isolated environment to JSON-stream, cross-session, PTY, and auxiliary argument-only spawns;
- bounds one stdout protocol event and retained stderr to 64 KiB, including newline-free output;
- redacts exact provider material as parsed stdout values and stderr lines enter retained capture, including nested JSON keys and values;
- redacts rendered PTY text and converts any stdout, stderr, PTY, trace, failure, report, or artifact canary into a typed hard failure;
- scans the isolated worktree without following symlinks, with explicit file-count, per-file, and aggregate-byte ceilings, and unlinks contaminated files before evidence collection;
- runs scenario setup before spawn and cleanup after every returned success or failure path;
- creates a private Linux cgroup v2 before spawn and moves the candidate into it from `pre_exec`, before candidate code can fork;
- terminates the entire recursive cgroup with `cgroup.kill`, bounded-reaps the direct child, waits for `populated 0`, and removes the cgroup before successful cleanup; and
- makes `WCORE_EVAL_REQUIRE_CONTAINMENT=1` fail closed when authoritative process-tree ownership is unavailable, including current PTY, macOS, and Windows paths.

## 2. Red-to-green sequence

| Commits | Contract |
|---|---|
| `851f4f5`–`bd5dd81` | Reproduce credential/host-state leaks; isolate child configuration and environment. |
| `64766f2`–`9bf59e7` | Reproduce and close unbounded stdout/stderr capture. |
| `67985b0`–`632ac74` | Prove inherited credential variables are forbidden; replace them with one-use files. |
| `7c135ee`–`5a73430` | Reproduce detached listener/heartbeat survival; install and verify Linux cgroup ownership. |
| `3f48896`–`63f5945` | Redact and classify secrets at stdout/stderr capture, PTY/report boundaries, and artifact retention. |
| `e2f2f96`–`c642ce4` | Fail closed when the requested platform or PTY path lacks authoritative tree ownership. |

## 3. Verification evidence

Verification ran on the isolated Hetzner worktree at the exact implementation source, with Rust `1.95.0`:

- `WCORE_EVAL_REQUIRE_CONTAINMENT=1 cargo test -p wcore-eval-scenarios --all-targets`: 143 passed, 4 explicitly ignored live tests, 0 failed;
- the detached-process contract killed a descendant that created a fresh process group, loopback listener, and advancing external heartbeat: 1 passed in 0.52 seconds;
- the hermetic fixture observed `arg_secret=false config_secret=false key_env=false poison=false budget=true`, while its deliberate stdout canary was retained only as `[REDACTED]` and classified `SecretDetected { sink: "stdout" }`;
- nested stdout JSON key/value, stderr-line, generated-config, artifact-removal, setup/cleanup, output-bound, and auxiliary-spawn tests passed;
- Core's one-use credential tests passed: valid credentials are consumed and removed, oversized credentials are rejected and removed;
- `cargo clippy -p wcore-eval-scenarios -p wcore-cli --all-targets -- -D warnings` passed;
- `cargo fmt --all -- --check` passed locally; and
- post-suite inspection found no `wayland-eval-*` cgroup and no detached orphan-listener process.

The only emitted toolchain notice is the pre-existing future-incompatibility warning for `imap-proto 0.10.2`; it is not a clippy diagnostic from this change.

## 4. Authority boundaries and deferred native proof

| Platform/path | Current behavior | Authority claim |
|---|---|---|
| Linux JSON-stream runner with writable cgroup v2 | Race-free pre-exec cgroup attachment; recursive kill and empty/remove proof | Code-verified against normal descendants on Hetzner |
| Linux candidate running as the same root identity as evaluator | Cgroup ownership exists, but hostile root can attempt ancestor-cgroup escape | Not an L4 hostile-artifact boundary; disposable unprivileged worker remains required |
| macOS JSON-stream runner | Best-effort process group | Not authoritative; strict mode rejects it |
| Windows JSON-stream runner | Direct-child fallback | Not authoritative; strict mode rejects it until Job Object/gated launch lands |
| PTY/TUI runner | Hermetic environment and output redaction; direct child cleanup | Not authoritative process-tree ownership; strict mode rejects it |

These are explicit F05/F28 gates, not hidden passes. F02 permits F03 receipt work to proceed because unsupported authority fails closed; M0 and release closure remain forbidden until the required native receipts exist.

## 5. Remaining non-F02 work

- F03 must bind this result to a versioned, content-addressed, provenance-aware receipt instead of relying on this prose record.
- F04 must replace the transitional ambient fixture control with the deterministic fixture protocol.
- F05/F28 must run native macOS/Linux/Windows packaging and add Windows Job Object/gated launch plus disposable-worker authority for macOS and PTY paths.
- A hostile candidate must run under a dedicated unprivileged identity before Linux cgroup evidence is treated as an adversarial sandbox boundary.

This receipt does not claim that the Hetzner build host is safe for malicious repositories or that a process-group/direct-child fallback owns a descendant tree.
