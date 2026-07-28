# CI-UNBLOCK — working notes (append-only, committed continuously per LANE-BRIEF §6b-i)

Lane: `lane/ci-unblock`. Base: `plan/f20-unified-audit-repair` @ `ef1d97be`.
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-ci-unblock`.

## T+0 — established

**The blocker is identified and it is exactly as briefed.**

CI's clippy step is `vx just lint` (`.github/workflows/ci.yml:160`), which is
`vx cargo clippy --workspace --all-targets -- -D warnings` (`justfile:75-76`).
The linux-containerized job invokes it directly at `ci.yml:323-324` with the
identical flags. So the reproduction target is exactly:

```
cargo clippy --workspace --all-targets -- -D warnings
```

Pulled the real error text from failed run **30369041140** (both
`linux-containerized` and `macos-latest` legs, identical):

```
error: this call to `clone` can be replaced with `std::slice::from_ref`
   --> crates/wcore-eval-scenarios/src/journey.rs:683:13
683 |             &[canary.clone()],
    |             ^^^^^^^^^^^^^^^^^ help: try: `std::slice::from_ref(&canary)`
    = note: `-D clippy::cloned-ref-to-slice-refs` implied by `-D warnings`
```

Lint: **`clippy::cloned_ref_to_slice_refs`**, new in **Rust 1.95.0**. This is a
toolchain-bump lint, not a code regression — the code was fine until the pinned
Rust moved. That is why it appeared everywhere at once and why no single lane
felt ownership.

Sites: `crates/wcore-eval-scenarios/src/journey.rs` lines 683, 695, 707, 717 —
all four inside `#[cfg(test)]`, all four the argument
`canaries: &[String]` of `scan_canaries`.

## T+0 — the fix, and why it is not a suppression

`&[canary.clone()]` allocates a one-element array by cloning the String.
`std::slice::from_ref(&canary)` produces the same `&[String]` of length 1 by
borrowing. Same type, same value, no clone. The lint's own suggestion.

This is already the **established idiom in this workspace**, including in the
same crate:
- `crates/wcore-eval-scenarios/src/providers.rs:273,277,279,280`
- `crates/wcore-agent/src/compact/estimate.rs:239,245,251`
- `crates/wcore-acp/src/server.rs:1410`
- `crates/wcore-channel-email/src/smtp.rs:628`

No `#[allow(clippy::cloned_ref_to_slice_refs)]` exists anywhere in `crates/`
(grepped, zero hits) and I am adding none.

One site needed care. Line 707 was:

```rust
scan_canaries("doc", &[canary.clone()], &[("raw".into(), canary.clone())]),
...
Err(ScanError::CanaryTooShort(canary))
```

`canary` is moved into the expected value in the *same* `assert_eq!` statement,
so replacing the first clone with a borrow puts a live borrow and a move in one
statement. Expected to be fine under NLL (the borrow ends when `scan_canaries`
returns an owned `Result`), and `estimate.rs:239` is the same shape, but this
is the one of the four that could fail to borrow-check. **To be confirmed by a
real compile — not assumed.** The second clone on that line is a genuine clone
into a tuple and is untouched.

## What is still to establish

1. Clippy green across the workspace with CI's exact invocation. (hetzner)
2. `cargo nextest run --workspace --profile ci --no-fail-fast` — `just test-ci`.
   **This has never executed in CI on this tree**: clippy precedes it and clippy
   has failed on every one of 46 non-cancelled runs since 2026-07-25. Expect
   unseen failures. Triage, do not absorb.
3. A real CI run on `lane/ci-unblock` watched to completion, with its run id.

## Traps I am holding

- Byte-count every capture; `echo "EXIT=${PIPESTATUS[0]}"` after a pipeline
  returns empty in this environment. Use a status file.
- Run test targets by file, never by filter (brief §3.2 flavour (c)).
- Read `N passed` counts back; never trust exit status alone.
- One `FAILED` line in `wcore-cli --lib` is a nested `failing_fixture` that a
  test deliberately shells out to. Read the lines, do not count them.
- Runner capacity is saturated (~20 active lane runs, `ci.yml` triggers on
  `lane/**`). A long queue is not a failure. Push **once**; re-pushing cancels
  queued runs.
- Full-workspace runs under multi-lane hetzner load produce known contention
  artifacts (EMFILE in `wcore-skills` watcher tests; wall-clock-budgeted binary
  tests). Re-run a suspicious crate alone at the same commit before calling it
  a regression, and say which run each figure came from.
