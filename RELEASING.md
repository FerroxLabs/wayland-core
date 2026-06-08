# Releasing wayland-core

This repo publishes pre-built binaries to GitHub Releases for the Wayland app
(`scripts/prepareWaylandCore.js`) to download. Releases are normally produced
by CI; this doc covers both the happy path and the manual fallback.

## Versioning

`Cargo.toml` `[workspace.package].version` is the source of truth.
Bumps are driven by [release-please](https://github.com/googleapis/release-please)
from conventional-commit messages on `main`:

- `feat(...): ...` → minor bump (pre-1.0: patch bump per
  `bump-patch-for-minor-pre-major: true`)
- `fix(...): ...` → patch bump
- `feat!: ...` or `BREAKING CHANGE:` footer → major bump (post-1.0)

`chore`, `ci`, `docs`, `style`, `test`, `build` types do **not** bump.

## Happy path — release via CI

1. Merge work into `main` using conventional-commit messages.
2. `release-please` opens a "Release" PR titled
   `chore(main): release X.Y.Z`. The PR updates `CHANGELOG.md`,
   `Cargo.toml` version, and `.release-please-manifest.json`.
3. Review the PR. When the version bump and changelog look right, merge it.
4. On merge, the `Release Please` workflow:
   - Creates git tag `vX.Y.Z` and a GitHub Release with auto-generated notes.
   - Calls the `Release` workflow via `workflow_call`.
5. The `Release` workflow builds `wayland-core` for six targets, packages each
   as `wayland-core-vX.Y.Z-<target>.{tar.gz,zip}`, generates
   `wayland-core-checksums.txt`, and uploads all artifacts to the GitHub
   Release created in step 4.
6. The app's `scripts/prepareWaylandCore.js` downloads the asset matching its
   host platform from `https://github.com/FerroxLabs/wayland-core/releases/`.

Targets built:

| OS      | Arch    | Rust target                  |
|---------|---------|------------------------------|
| Linux   | x86_64  | `x86_64-unknown-linux-gnu`   |
| Linux   | aarch64 | `aarch64-unknown-linux-gnu`  (cross) |
| macOS   | x86_64  | `x86_64-apple-darwin`        |
| macOS   | aarch64 | `aarch64-apple-darwin`       |
| Windows | x86_64  | `x86_64-pc-windows-msvc`     |
| Windows | aarch64 | `aarch64-pc-windows-msvc`    |

## Manual dispatch (CI is green but you want to re-run packaging)

```bash
gh workflow run release.yml \
  --repo FerroxLabs/wayland-core \
  --field tag_name=vX.Y.Z
```

The tag must already exist. Re-runs upload with `--clobber` and replace
prior assets on the same release.

## Manual fallback — CI broken, tag already cut

If the `Release` workflow fails partway and you need binaries before the fix
lands, build locally and upload by hand.

Per target, on the matching host (or via `cross` for Linux aarch64):

```bash
git checkout vX.Y.Z
cargo build --release --target <target> -p wcore-cli
cd target/<target>/release
tar -czf wayland-core-vX.Y.Z-<target>.tar.gz wayland-core   # or wayland-core.exe on Windows (use zip there)
```

Then:

```bash
gh release upload vX.Y.Z \
  wayland-core-vX.Y.Z-<target>.tar.gz \
  --repo FerroxLabs/wayland-core \
  --clobber
```

Regenerate checksums after all six artifacts are uploaded:

```bash
shasum -a 256 wayland-core-vX.Y.Z-* > wayland-core-checksums.txt
gh release upload vX.Y.Z wayland-core-checksums.txt --clobber
```

## Verifying a release

After publication, smoke-check the asset list:

```bash
gh release view vX.Y.Z --repo FerroxLabs/wayland-core --json assets \
  --jq '.assets[].name'
```

Expect six platform archives plus `wayland-core-checksums.txt`.
