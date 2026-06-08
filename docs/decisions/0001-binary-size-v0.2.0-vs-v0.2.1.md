# 0001 — Binary size delta v0.2.0 → v0.2.1

**Date:** 2026-05-15
**Wave:** Z0 — Build infrastructure remediation
**Closes:** Wave 3 H.6 carryover (was scoped out to W8c.3.D; now landed here)

## Measurement

Built `wcore-cli` release on macOS aarch64 (`darwin-arm64`), main + Z0 worktree, identical machine + cache state.

| Build | Profile | Binary size | Delta |
|---|---|---|---|
| `main @ 071c410` (v0.2.0-wayland-base) | default release (no explicit `[profile.release]`) | **23 MB** | baseline |
| `feat/wcore-Z0-build-infra` | `[profile.release]` with `lto = "thin"` + `codegen-units = 1` + `strip = "debuginfo"` | **19 MB** | **-4 MB (~17% smaller)** |

Build wall-clock time increased from 26s (baseline incremental) → 70s (Z0 clean), an expected tradeoff from `codegen-units = 1` + thin LTO. The penalty applies to release builds only; debug/test builds unaffected.

## Decision

Adopt the Z0 release profile for v0.2.1 and forward.

```toml
[profile.release]
lto           = "thin"
codegen-units = 1
strip         = "debuginfo"
```

### Why we did NOT add `panic = "abort"`

The original Z0 draft included `panic = "abort"`. Removed before merge. Reason: Wave RB introduces `std::panic::catch_unwind` boundaries around tool dispatch so a single tool panic degrades that tool call rather than crashing the session. `panic = "abort"` would make `catch_unwind` a no-op and tool panics fatal — losing the fault-isolation invariant RB depends on. The unwind-tables binary-size cost (a few hundred KB) is acceptable in exchange.

If a future profile needs `panic = "abort"` (e.g. a separate "embedded" or "fail-fast" target), define it as a non-default profile, not the default release.

## Verification

```bash
cd <engine-root>
git checkout main
vx cargo build --release -p wcore-cli
ls -lh target/release/wayland-core  # ~23 MB

git checkout feat/wcore-Z0-build-infra
vx cargo build --release -p wcore-cli
ls -lh target/release/wayland-core  # ~19 MB
```

## Future work

Track size at each minor-version tag. If `lto = "fat"` becomes worth the build-time cost (e.g. for shipping prebuilt binaries via release.yml), revisit. Document the trade in a follow-up decision.
