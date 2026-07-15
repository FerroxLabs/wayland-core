# Wayland Core F11 Proof Harness

This harness closes the previously missing reproducible proof path for F11. It
does not change the fixed acceptance thresholds and does not claim a pass until
the generated JSON receipts say `"verdict": "pass"`.

## Ordinary deterministic corpus

The benchmark reuses
`wcore-cli::deterministic_openai_loop::packaged_f04_run_is_repeatable_and_content_addressed`.
That test existed at the exact pre-F11 base
`a51808821be49f2983529b343294b01d839fb004` and remains in the candidate. Each
execution drives the packaged Core binary through a deterministic coding task:
Read, Edit, a content-addressed repository mutation, an MCP call, and final
semantic request/behavior receipts. It internally runs the scenario twice in
fresh workspaces and proves repeatability.

The runner creates a detached baseline worktree at the frozen SHA, builds the
same test in isolated target directories, performs one warm-up per variant,
then alternates baseline/candidate order for at least 20 measured pairs. The
receipt fails closed unless:

- every paired execution completes (40 scenario runs per variant at 20 pairs);
- all four recorded digests are stable within each variant;
- the fixture, OpenAI semantic behavior, and final repository digests are
  identical between variants;
- candidate median wall time regresses by no more than 10%; and
- candidate nearest-rank p95 wall time regresses by no more than 15%.

Run on the Linux proof host from the candidate source tree:

```bash
export PATH="/root/.cargo/bin:/root/.local/bin:$PATH"
export WCORE_F11_CARGO=/root/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin/cargo
export RUSTC=/root/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin/rustc
export RUSTDOC=/root/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin/rustdoc
python3 scripts/f11-proof.py benchmark \
  --repo "$PWD" \
  --work-root /tmp/wayland-f11-benchmark \
  --output /tmp/wayland-f11-benchmark-receipt.json \
  --pairs 20
```

The receipt binds both variants to their exact source-tree SHA-256, records the
test executable digests and every paired duration, and fails if the candidate
source changes during the run.

`behavior_sha256` is deliberately not a cross-build equality oracle. The
receipt behavior projection includes the binary, resolved configuration, and
effective policy identities, all of which F11 changes by design. It remains a
required within-variant determinism check. The first calibration run used that
version-bound digest as a cross-build key and therefore failed despite equal
semantic outputs; its failed receipt is retained at SHA-256
`0dbd543fa01e0b358ca942606b64db2b52e8ba4e109e9c7e19395dd84fa75708`.

## Adversarial envelope proof

The adversarial mode prebuilds the relevant test binaries and invokes the
existing focused tests directly, plus the isolated
`f11_concurrent_reservation_proof` integration test because the pre-existing
reservation test was sequential. This proves the assertion-bearing paths
without including Cargo compilation time. It covers:

- zero provider sends after a reservation/admission block;
- zero unpriceable-provider sends while a USD cap is active;
- zero concurrent provider-reservation overshoot;
- zero process-spawning admission beyond the configured cap;
- zero aggregate tool-runtime reservation/concurrent-dispatch overshoot; and
- preemption at a 20 ms charged tool-runtime deadline.

The deadline test is sampled five times. Its conservative measurement includes
the prebuilt libtest process startup and must still complete within 120 ms: the
20 ms charged runtime plus the fixed 100 ms scheduler tolerance.

```bash
export PATH="/root/.cargo/bin:/root/.local/bin:$PATH"
export WCORE_F11_CARGO=/root/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin/cargo
export RUSTC=/root/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin/rustc
export RUSTDOC=/root/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin/rustdoc
python3 scripts/f11-proof.py adversarial \
  --repo "$PWD" \
  --target-dir /tmp/wayland-f11-adversarial-target \
  --output /tmp/wayland-f11-adversarial-receipt.json
```

These are Linux receipts. Native macOS and Windows evidence remains a separate
release-seal requirement and must not be inferred from this harness.
