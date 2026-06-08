# 0002 — Performance baseline for v0.2.1

Date: 2026-05-15

Captured as a v0.2.1 checkpoint so future drift is comparable against
a known-good measurement. Numbers below are local macOS only;
Linux + Windows columns are populated by the next CI matrix run.

## Local platform

- OS: `Darwin 25.3.0 arm64` (macOS, aarch64-apple-darwin)
- Toolchain: pinned via `vx.toml` (Rust stable + `just`)
- Repo SHA at measurement: `feat/wcore-V1-release-smoke` tip (V1 C1 + C2 applied)

## Workspace test runtime

Command: `vx cargo nextest run --workspace`

```
Summary [  30.451s] 2830 tests run: 2830 passed, 5 skipped
```

- 2830 tests, 5 skipped (= the standing `#[ignore]`d live-Ollama smokes
  + sibling waves' opt-in tests).
- 2830 = 2828 (V2 baseline) + 2 new release-binary smoke tests added
  in V1 C1.

## Release build

Command: `vx cargo build --release -p wcore-cli`

| Scenario | Wall-clock |
|----------|-----------|
| Cold (full workspace dep graph compile via nextest test build) | 17.90s |
| Cached (no source changes; cargo no-op re-finish) | 0.60s |

Cold number was observed inside the `cargo nextest run -p wcore-cli
--test release_binary_smoke` first run during V1 C1 verification —
`Finished 'test' profile [unoptimized + debuginfo] target(s) in 17.90s`.
A standalone cold release build will be longer in CI because the
`release` profile uses `lto = "thin"` + `codegen-units = 1` versus the
test build's `dev` profile.

## Release binary size

Command: `ls -lh target/release/wayland-core`

```
-rwxr-xr-x@ 1 seandonahoe  staff    20M May 15 21:42 target/release/wayland-core
```

Size: **20 MB** (20,971,520 ≈ 20 MiB; exact byte count varies with
linker version).

Reference: ADR 0001 captured the v0.2.0 → v0.2.1 binary-size delta;
this entry is the absolute baseline at v0.2.1.

## CLI cold-start

Command: `time target/release/wayland-core --help` (three runs, BSD
`/usr/bin/time`, 10 ms precision)

```
0.00 real         0.00 user         0.00 sys
0.00 real         0.00 user         0.00 sys
0.02 real         0.00 user         0.00 sys
```

Wall-clock range: **0.00s – 0.02s** (≤ 20 ms). clap-only path, no I/O,
no plugin discovery — this is the floor.

## CI matrix follow-up

The next push to `feat/wcore-V1-release-smoke` triggers the CI matrix
defined in `.github/workflows/ci.yml`. Once green, append the
Linux (ubuntu-latest, x86_64) and Windows (windows-latest, x86_64)
columns for each row above. Until then, those rows remain unset.
