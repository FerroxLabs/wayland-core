#!/usr/bin/env node
// Generate the npm publish tree for wayland-core: a platform-resolving launcher
// package (`@ferroxlabs/wayland-core`) plus one binary package per target
// (`@ferroxlabs/wayland-core-<os>-<cpu>`), using the `os`/`cpu` +
// optionalDependencies pattern (esbuild/Biome/swc). A consumer installs the
// launcher; npm pulls ONLY the one platform package matching their machine.
//
// Pure Node, zero dependencies. It consumes the per-target binaries that
// `.github/workflows/release.yml` already builds — extract each release archive
// to `<binaries>/<rust-triple>/wayland-core[.exe]` first (the CI job does this).
//
// Usage:
//   node npm/generate.mjs --version 0.9.5 --binaries ./binaries --out ./npm-dist
//   node npm/generate.mjs --version 0.9.5 --binaries ./binaries --out ./npm-dist --allow-missing
//
// `--allow-missing` skips (with a warning) any target whose binary is absent, so
// a partial/local run can produce a subset; CI runs WITHOUT it so a missing
// platform fails the release loudly.

import { existsSync, mkdirSync, copyFileSync, writeFileSync, chmodSync } from "node:fs";
import { join, dirname } from "node:path";

const SCOPE = "@ferroxlabs";
const LAUNCHER = `${SCOPE}/wayland-core`;
const LICENSE = "Apache-2.0";
// Canonical object form so npm doesn't auto-correct (string → object) at publish.
// The owner casing MUST exactly match the GitHub repo slug (`FerroxLabs`): npm
// provenance validates `repository.url` against the OIDC source claim and 422s
// on any case mismatch (npm/cli#8036). The OIDC claim reports the real casing.
const REPOSITORY = {
  type: "git",
  url: "git+https://github.com/FerroxLabs/wayland-core.git",
};

// rust triple → npm os/cpu (node's process.platform/process.arch vocabulary,
// which is ALSO the Wayland desktop's `${process.platform}-${process.arch}`
// bundled-wayland-core runtimeKey — they match 1:1 on purpose).
const TARGETS = [
  { triple: "aarch64-apple-darwin", os: "darwin", cpu: "arm64", exe: false },
  { triple: "x86_64-apple-darwin", os: "darwin", cpu: "x64", exe: false },
  { triple: "aarch64-unknown-linux-gnu", os: "linux", cpu: "arm64", exe: false },
  { triple: "x86_64-unknown-linux-gnu", os: "linux", cpu: "x64", exe: false },
  { triple: "aarch64-pc-windows-msvc", os: "win32", cpu: "arm64", exe: true },
  { triple: "x86_64-pc-windows-msvc", os: "win32", cpu: "x64", exe: true },
];

function parseArgs(argv) {
  const args = { allowMissing: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--version") args.version = argv[++i];
    else if (a === "--binaries") args.binaries = argv[++i];
    else if (a === "--out") args.out = argv[++i];
    else if (a === "--allow-missing") args.allowMissing = true;
    else throw new Error(`unknown argument: ${a}`);
  }
  for (const req of ["version", "binaries", "out"]) {
    if (!args[req]) throw new Error(`missing required --${req}`);
  }
  if (!/^\d+\.\d+\.\d+/.test(args.version)) {
    throw new Error(`--version must be a semver (got "${args.version}")`);
  }
  return args;
}

const pkgName = (t) => `${SCOPE}/wayland-core-${t.os}-${t.cpu}`;
const binName = (t) => (t.exe ? "wayland-core.exe" : "wayland-core");

function writeJson(path, obj) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, JSON.stringify(obj, null, 2) + "\n");
}

// --- platform package: the binary + an os/cpu-gated package.json ------------
function emitPlatformPackage(out, version, target, binaries, allowMissing) {
  const src = join(binaries, target.triple, binName(target));
  if (!existsSync(src)) {
    if (allowMissing) {
      console.warn(`! skip ${pkgName(target)} — binary missing at ${src}`);
      return null;
    }
    throw new Error(`binary missing for ${target.triple} at ${src}`);
  }
  const dir = join(out, `wayland-core-${target.os}-${target.cpu}`);
  const dest = join(dir, "bin", binName(target));
  mkdirSync(dirname(dest), { recursive: true });
  copyFileSync(src, dest);
  if (!target.exe) chmodSync(dest, 0o755);

  writeJson(join(dir, "package.json"), {
    name: pkgName(target),
    version,
    description: `wayland-core binary for ${target.os}-${target.cpu}`,
    license: LICENSE,
    repository: REPOSITORY,
    // npm installs this package ONLY on a matching machine; on any other
    // platform it is skipped (it is an optional dependency of the launcher).
    os: [target.os],
    cpu: [target.cpu],
    files: ["bin/"],
    publishConfig: { access: "public" },
  });
  return pkgName(target);
}

// --- launcher package: resolver + bin shim + optionalDependencies -----------
const INDEX_JS = `"use strict";
// Resolve the platform-correct wayland-core binary that npm installed as an
// optional dependency. Programmatic entry point: a host (e.g. AionCLI) calls
// require("@ferroxlabs/wayland-core").binaryPath() and spawns it directly.
const fs = require("node:fs");
const path = require("node:path");

const PLATFORM_PACKAGES = {
  "darwin-arm64": "${SCOPE}/wayland-core-darwin-arm64",
  "darwin-x64": "${SCOPE}/wayland-core-darwin-x64",
  "linux-arm64": "${SCOPE}/wayland-core-linux-arm64",
  "linux-x64": "${SCOPE}/wayland-core-linux-x64",
  "win32-arm64": "${SCOPE}/wayland-core-win32-arm64",
  "win32-x64": "${SCOPE}/wayland-core-win32-x64",
};

function binaryName() {
  return process.platform === "win32" ? "wayland-core.exe" : "wayland-core";
}

/**
 * Absolute path to the platform-correct wayland-core binary.
 * Throws an actionable error if the platform is unsupported or its package was
 * not installed (e.g. install ran with --no-optional / --ignore-optional).
 */
function binaryPath() {
  const key = process.platform + "-" + process.arch;
  const pkg = PLATFORM_PACKAGES[key];
  if (!pkg) {
    throw new Error("wayland-core: unsupported platform " + key);
  }
  let pkgJson;
  try {
    pkgJson = require.resolve(pkg + "/package.json");
  } catch (e) {
    throw new Error(
      "wayland-core: platform package " + pkg + " is not installed. It should " +
        "have been pulled in automatically as an optional dependency — reinstall " +
        "without --no-optional / --ignore-optional."
    );
  }
  const p = path.join(path.dirname(pkgJson), "bin", binaryName());
  if (!fs.existsSync(p)) {
    throw new Error("wayland-core: binary missing at " + p);
  }
  return p;
}

module.exports = { binaryPath };
`;

const BIN_JS = `#!/usr/bin/env node
"use strict";
// Thin launcher for \`npx @ferroxlabs/wayland-core\` / global installs. Resolves
// the platform binary and execs it transparently (stdio inherited, exit code
// relayed). Hosts that embed the engine should call binaryPath() and spawn the
// binary directly instead of going through this shim.
const { spawnSync } = require("node:child_process");
const { binaryPath } = require("../index.js");
const staleCheck = require("./stale-check.js");

let bin;
try {
  bin = binaryPath();
} catch (err) {
  console.error(err.message);
  process.exit(1);
}

// npx caches this package by SPEC STRING and never re-queries the registry for
// an unpinned / @latest spec, so a box can silently freeze on an old engine
// forever (#126). Warn from cached state (never block, never fail the launch)
// and refresh that state in a detached background process at most once a day.
try {
  staleCheck.warnIfStale();
} catch (_e) {
  // fail-safe: the update check must never break a launch
}

const result = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error("wayland-core: failed to launch: " + result.error.message);
  process.exit(1);
}
process.exit(result.status === null ? 1 : result.status);
`;

const STALE_CHECK_JS = `"use strict";
// Staleness self-heal for the npx launcher (#126). npx caches the resolved
// package tree by SPEC STRING and does not reliably re-query the registry even
// for \`@latest\` (npm/cli#2329, npm/cli#7838, npm/rfcs#700), so a machine that
// once ran \`npx ${LAUNCHER}\` stays frozen on whatever \`latest\`
// was at first run — users keep hitting bugs the current engine already fixed.
// Only an EXACT-version spec is a guaranteed cache miss, so the warning below
// prints one.
//
// Design constraints (deliberate — this wraps every user invocation):
//   - never blocks the launch: the registry query runs in a detached child
//     that outlives this process; the foreground only reads the cached state
//   - fail-safe: every path is wrapped; any error means "no warning", never a
//     broken launch
//   - throttled: registry queried at most once per CHECK_INTERVAL_MS, warning
//     printed at most once per WARN_INTERVAL_MS
//   - opt-out: WAYLAND_CORE_SKIP_UPDATE_CHECK=1; also skipped when CI is set
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const PKG = "${LAUNCHER}";
const CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;
const WARN_INTERVAL_MS = 24 * 60 * 60 * 1000;
const FETCH_TIMEOUT_MS = 5000;

// Launcher-private state; co-located with the engine's home dir convention
// (WAYLAND_HOME override honored, matching wcore-config resolution order).
function stateFile() {
  const home = process.env.WAYLAND_HOME || path.join(os.homedir(), ".wayland");
  return path.join(home, "npx-update-check.json");
}

function readState() {
  try {
    const parsed = JSON.parse(fs.readFileSync(stateFile(), "utf8"));
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch (_e) {
    return {};
  }
}

function writeState(state) {
  try {
    const file = stateFile();
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, JSON.stringify(state));
  } catch (_e) {
    // best-effort
  }
}

function parseVersion(v) {
  const m = /^(\\d+)\\.(\\d+)\\.(\\d+)/.exec(String(v || ""));
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
}

function isBehind(current, latest) {
  const a = parseVersion(current);
  const b = parseVersion(latest);
  if (!a || !b) return false;
  for (let i = 0; i < 3; i++) {
    if (a[i] < b[i]) return true;
    if (a[i] > b[i]) return false;
  }
  return false;
}

function skipped() {
  return Boolean(process.env.WAYLAND_CORE_SKIP_UPDATE_CHECK || process.env.CI);
}

/** Foreground half: warn from cached state, kick off a background refresh. */
function warnIfStale() {
  if (skipped()) return;
  let current;
  try {
    current = require("../package.json").version;
  } catch (_e) {
    return;
  }
  const state = readState();
  const now = Date.now();

  const warnThrottled =
    typeof state.warnedAt === "number" && now - state.warnedAt < WARN_INTERVAL_MS;
  if (state.latest && isBehind(current, state.latest) && !warnThrottled) {
    console.error(
      "wayland-core: v" + current + " is stale — latest is v" + state.latest + ".\\n" +
        "  npx caches this package by spec string and never refreshes it; run the\\n" +
        "  exact version to bust the cache:  npx " + PKG + "@" + state.latest + "\\n" +
        "  (or: npm i -g " + PKG + "@latest — suppress this check with\\n" +
        "  WAYLAND_CORE_SKIP_UPDATE_CHECK=1)"
    );
    state.warnedAt = now;
    writeState(state);
  }

  const checkThrottled =
    typeof state.checkedAt === "number" && now - state.checkedAt < CHECK_INTERVAL_MS;
  if (!checkThrottled) {
    try {
      const { spawn } = require("node:child_process");
      const child = spawn(process.execPath, [__filename, "--refresh"], {
        detached: true,
        stdio: "ignore",
        windowsHide: true,
      });
      child.unref();
    } catch (_e) {
      // best-effort
    }
  }
}

/** Background half: query the registry dist-tags and persist the result. */
async function refresh() {
  const state = readState();
  state.checkedAt = Date.now();
  try {
    if (typeof fetch === "function") {
      const ctl = new AbortController();
      const timer = setTimeout(() => ctl.abort(), FETCH_TIMEOUT_MS);
      const res = await fetch(
        "https://registry.npmjs.org/-/package/" + PKG + "/dist-tags",
        { signal: ctl.signal, headers: { accept: "application/json" } }
      );
      clearTimeout(timer);
      if (res.ok) {
        const tags = await res.json();
        if (tags && parseVersion(tags.latest)) state.latest = tags.latest;
      }
    }
  } catch (_e) {
    // offline / registry down — keep the previous \`latest\`, still throttle
  }
  writeState(state);
}

module.exports = { warnIfStale };

if (require.main === module && process.argv[2] === "--refresh") {
  refresh();
}
`;

function emitLauncher(out, version, present) {
  const dir = join(out, "wayland-core");
  // Every platform package is an OPTIONAL dependency: npm installs only the one
  // matching the host's os/cpu and silently skips the rest.
  const optionalDependencies = {};
  for (const t of TARGETS) optionalDependencies[pkgName(t)] = version;

  writeJson(join(dir, "package.json"), {
    name: LAUNCHER,
    version,
    description:
      "wayland-core — multi-provider AI agent engine. Platform-resolving launcher; " +
      "the matching native binary installs automatically per os/cpu.",
    license: LICENSE,
    repository: REPOSITORY,
    bin: { "wayland-core": "bin/wayland-core.js" },
    main: "index.js",
    files: ["bin/", "index.js"],
    optionalDependencies,
    publishConfig: { access: "public" },
  });
  const binPath = join(dir, "bin", "wayland-core.js");
  mkdirSync(dirname(binPath), { recursive: true });
  writeFileSync(binPath, BIN_JS);
  chmodSync(binPath, 0o755);
  writeFileSync(join(dir, "bin", "stale-check.js"), STALE_CHECK_JS);
  writeFileSync(join(dir, "index.js"), INDEX_JS);

  if (present.length !== TARGETS.length) {
    console.warn(
      `! launcher optionalDependencies list all ${TARGETS.length} platforms but ` +
        `only ${present.length} package(s) were generated this run`
    );
  }
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  mkdirSync(args.out, { recursive: true });
  const present = [];
  for (const t of TARGETS) {
    const name = emitPlatformPackage(args.out, args.version, t, args.binaries, args.allowMissing);
    if (name) present.push(name);
  }
  emitLauncher(args.out, args.version, present);
  console.log(
    `Generated ${present.length} platform package(s) + launcher ${LAUNCHER}@${args.version} into ${args.out}`
  );
  for (const n of present) console.log(`  - ${n}@${args.version}`);
  console.log(`  - ${LAUNCHER}@${args.version}`);
}

main();
