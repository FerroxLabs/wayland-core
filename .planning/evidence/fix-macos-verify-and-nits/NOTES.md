# NOTES — lane/fix-macos-verify-and-nits

Base: `e7bc6d883027102ff1e5bbaa2dd19f9265268cab` (integration `plan/f20-unified-audit-repair`).
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-fix-macos-verify-and-nits`.
Asserted at start: `git rev-parse HEAD` == base, `--show-toplevel` == the lane path.

Three tasks:
1. macOS verification of the three TUI fixes merged in `e7bc6d88` (Linux-only evidence today).
2. UAT-W1 — the non-TTY error advises `-p`, which is `--provider`.
3. Bare-letter shortcuts on a prose surface (residual from the merged TUI lane).

## T+0 — premise verification

### Task 1 premise: is a darwin binary obtainable at integration head?

**YES, and the brief's framing ("trigger a CI build and wait") understates what already
exists.** `.github/workflows/ci.yml:960` defines `build-darwin-selfhosted`:

```
name: Build (aarch64-apple-darwin) [self-hosted]
if: github.event_name == 'push' && startsWith(github.ref_name, 'lane/') && !contains(... '[ci-darwin]')
runs-on: [self-hosted, macOS, ARM64]
```

It compiles `-p wcore-cli --release --target aarch64-apple-darwin` and uploads artifact
`wayland-core-aarch64-apple-darwin` (ci.yml:1104-1111). So a **`lane/**` push is itself the
trigger** — no `workflow_dispatch` needed. Median ~16.5 min.

Two properties that constrain the plan:

- `concurrency: group: darwin-selfhosted-${{ github.ref }}, cancel-in-progress: true`
  (ci.yml:93-95). **One shot per push** — a second push cancels the first build. So the
  verification binary must be produced by a push I then do NOT supersede until it is
  downloaded.
- The job is `push`-only and `lane/**`-only, deliberately, because the runner is Sean's own
  desktop (ci.yml:56-78). The runner label `sean-mac-arm64` is **this machine** — consistent
  with LANE-BRIEF's "the Mac is NOT an ssh host, you are already on it".

Run `30546738547` exists for the base SHA itself but is on branch
`plan/f20-unified-audit-repair`, which is excluded from this job by gate 2 (integration keeps
the hermetic hosted runner). So the base SHA's own run will NOT produce an arm64 artifact.

**Plan:** push a docs-only commit first, so the built binary's `crates/` tree is byte-identical
to integration head. Assert that identity with `git diff e7bc6d88 <lane-sha> -- crates/`
being empty, and report the binary as "integration head's code, built from a lane commit that
adds only documentation" — not as a bare claim of head coverage.

### Task 2 premise: CONFIRMED at base

`crates/wcore-cli/src/main.rs:2032`:

```rust
"wayland-core: stdin is not a terminal and no prompt was given.\n\
 Use --json-stream for headless/piped use, or pass a prompt with -p."
```

`crates/wcore-cli/src/main.rs:268-269`:

```rust
/// Provider: "anthropic" or "openai"
#[arg(short, long, env = "PROVIDER")]
provider: Option<String>,
```

`short` with no value derives the first letter of the long name, so `-p` == `--provider`.
The advice is wrong. Line numbers in the orchestrator brief (2032 / 269) both HELD at base —
noted because LANE-BRIEF §"your brief's measurements are probably stale" says to expect
otherwise, and here they did not decay.

Still to establish: what the *correct* advice is (how a prompt is actually passed).

## Status log

- T+0 pushing this file to start the darwin build clock. Nothing else committed yet.
