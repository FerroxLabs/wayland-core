# npm distribution for wayland-core

Publishes the `wayland-core` binary to npm so the **Wayland** product line can
ship the right platform binary with zero friction:

- **AionCLI** (the Node CLI) declares it as a dependency — npm resolves the one
  matching binary automatically.
- **Wayland desktop** (Electron) resolves it from `node_modules` in
  `app/scripts/prepareWaylandCore.js` (the script's documented path 0) instead
  of hand-placing or downloading-by-tag.
- **End users** can `npx @ferroxlabs/wayland-core@latest …` or `npm i -g`.

## Layout — launcher + per-platform packages (the esbuild/Biome pattern)

One launcher package + six binary packages, each gated by `os`/`cpu`:

| Package | os / cpu | contains |
|---|---|---|
| `@ferroxlabs/wayland-core` | — | `index.js` (`binaryPath()`), `bin/wayland-core.js` shim, **optionalDependencies** on all six below |
| `@ferroxlabs/wayland-core-darwin-arm64` | darwin / arm64 | `bin/wayland-core` |
| `@ferroxlabs/wayland-core-darwin-x64` | darwin / x64 | `bin/wayland-core` |
| `@ferroxlabs/wayland-core-linux-arm64` | linux / arm64 | `bin/wayland-core` |
| `@ferroxlabs/wayland-core-linux-x64` | linux / x64 | `bin/wayland-core` |
| `@ferroxlabs/wayland-core-win32-arm64` | win32 / arm64 | `bin/wayland-core.exe` |
| `@ferroxlabs/wayland-core-win32-x64` | win32 / x64 | `bin/wayland-core.exe` |

Because each platform package declares `os`/`cpu`, npm installs **only the one**
matching the consumer's machine (the other five are skipped as optional deps).
The `<os>-<cpu>` keys are exactly node's `${process.platform}-${process.arch}` —
the same key the desktop uses for `bundled-wayland-core/<key>/`.

> **Linux is glibc** (`*-unknown-linux-gnu`), matching the desktop's
> AppImage/deb/rpm targets and AionCLI's (non-Docker) audience. No musl.

## Staleness self-heal (the npx cache trap, #126)

npx caches the resolved tree by **spec string** and never re-queries the
registry for an unpinned / `@latest` spec (npm/cli#2329) — a box that once ran
`npx @ferroxlabs/wayland-core` freezes on that first-resolved version forever.
The bin shim (`bin/wayland-core.js`) therefore ships a deliberately minimal
self-heal (`bin/stale-check.js`):

- on launch it reads a cached state file
  (`$WAYLAND_HOME/npx-update-check.json`, default `~/.wayland/`) and, if the
  running version is behind the registry's `latest`, prints a stderr warning
  with the **exact-version** `npx` command (an exact spec is the only
  guaranteed cache miss);
- it refreshes that state in a **detached background process** (5s fetch
  timeout), at most once per 24h — a launch is never blocked and never fails
  because of the check;
- warning and registry query are throttled to once per 24h; every path is
  fail-safe (any error → no warning, launch proceeds);
- opt out with `WAYLAND_CORE_SKIP_UPDATE_CHECK=1`; skipped automatically when
  `CI` is set. Hosts that spawn via `binaryPath()` bypass the shim and are
  unaffected.

## How consumers use it

```js
// AionCLI / any Node host: spawn the engine directly.
const { binaryPath } = require("@ferroxlabs/wayland-core");
const { spawn } = require("node:child_process");
const child = spawn(binaryPath(), ["--json-stream", "--provider", "anthropic"], {
  stdio: ["pipe", "pipe", "inherit"],
});
```

Desktop (`prepareWaylandCore.js`, cross-arch builds): install the **named**
platform package for the *target* arch — do **not** rely on `os`/`cpu`
auto-resolution, which keys off the *build host* and would put the wrong arch in
a cross-built installer:

```bash
npm install @ferroxlabs/wayland-core-darwin-x64@<version> --no-save
# then copy node_modules/@ferroxlabs/wayland-core-darwin-x64/bin/wayland-core
# into resources/bundled-wayland-core/darwin-x64/
```

## How it's built & published

`.github/workflows/release.yml` already cross-builds the six targets and uploads
them as release assets. The `publish-npm` job (gated on `post-tag-smoke`, so npm
only serves binaries that passed `--version` on their native OS):

1. downloads the six release archives,
2. extracts each to `binaries/<rust-triple>/wayland-core[.exe]`,
3. runs `node npm/generate.mjs --version <v> --binaries binaries --out npm-dist`,
4. `npm publish`es the six platform packages first, then the launcher.

### Prerequisites (one-time)

- Create the **`@ferroxlabs` npm org** (or claim the scope).
- Add an **`NPM_TOKEN`** automation token as a repo/org secret. Until it exists,
  the `publish-npm` job no-ops with a notice rather than failing the release.
- Optional: enable npm **provenance** by adding `permissions: { id-token: write }`
  to the job and `--provenance` to the publish step.

## Local verification

The generator is pure Node (no deps). Smoke it with a fake binary:

```bash
T=/tmp/wcore-npm-test; mkdir -p "$T/binaries/aarch64-apple-darwin"
printf '#!/bin/sh\necho "wayland-core $*"\n' > "$T/binaries/aarch64-apple-darwin/wayland-core"
chmod +x "$T/binaries/aarch64-apple-darwin/wayland-core"
node npm/generate.mjs --version 0.0.0 --binaries "$T/binaries" --out "$T/dist" --allow-missing
# Then symlink the two packages into a node_modules and run the bin shim.
```

`--allow-missing` lets a partial set publish locally; CI runs **without** it so a
missing platform fails the release loudly.
