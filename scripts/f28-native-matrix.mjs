#!/usr/bin/env node
// Phase 28 E5 native matrix — executor and marker verifier.
//
// ## Lineage
//
// The marker discipline is scaled from `scripts/f20-native-uat-proof.mjs`
// (`verifyNativeLog`) rather than invented a second time: exact-byte reads with an
// LF-only grammar, every marker binding candidate commit AND tree AND nonce, a declared
// ordering, exactly one final acceptance marker positioned after all cell markers, and
// fail-closed on anything absent, duplicate, reordered, foreign or out of position.
//
// That discipline exists because a proof log is the one artifact everybody trusts and
// nobody checks. It is worth restating what each rule stops:
//
//   * absent      — a leg that never ran reads as a leg that passed;
//   * duplicate   — one cell's marker copied over a cell that was never exercised;
//   * reordered   — a log assembled after the fact rather than emitted during the run;
//   * foreign     — a marker for a cell this matrix never generated;
//   * misordered  — a final acceptance marker written before the cells it accepts;
//   * unbound     — a marker from a different commit, tree or run.
//
// It also carries forward the 20A wrong-OS anti-drift lesson: a cell marker whose
// platform is not the platform under verification is foreign, not tolerable noise.
//
// ## Why the executor lives here and not in the Rust crate
//
// The certification Mac may run the shipped `wayland-core` binary and may NOT run
// cargo. A probe implemented as a cargo-built harness cannot run there, so a dimension
// covered only by such a probe silently loses a whole OS family. The probe DEFINITIONS
// are canonical in `crates/wcore-eval-scenarios/src/e5_cases.rs`; this file mirrors them
// and executes them with nothing but a Node runtime and the binary.
// `crates/wcore-eval-scenarios/tests/e5_native_matrix.rs` asserts the mirror below
// matches that table entry for entry, so the executor cannot drift from the definition
// it claims to implement.
//
// ## A measurement that cannot be taken must never render as 0
//
// Where a probe cannot take its measurement it reports `red` with the reason. It never
// reports a count of zero violations, and `activeness=none` on a sandbox cell is
// rejected by the verifier rather than rendered as a pass.

import { openSync, fstatSync, readSync, closeSync, constants } from 'node:fs';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, existsSync, readFileSync, symlinkSync, chmodSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, sep } from 'node:path';
import { spawnSync, execFileSync } from 'node:child_process';
import { realpathSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// ---------------------------------------------------------------------------------
// The probe table — MIRROR of `e5_cases.rs::PROBES`, asserted equal by the Rust test.
// ---------------------------------------------------------------------------------

export const PROBES = [
  { id: 'sandbox-probes', dimension: 'sandbox-probes', families: ['linux', 'macos', 'windows'], cell_id: null, harness: 'black-box', emits_activeness: true },
  { id: 'unicode', dimension: 'unicode', families: ['linux', 'macos', 'windows'], cell_id: null, harness: 'black-box', emits_activeness: false },
  { id: 'long-paths', dimension: 'long-paths', families: ['linux', 'macos', 'windows'], cell_id: null, harness: 'black-box', emits_activeness: false },
  { id: 'unc-reparse-symlink', dimension: 'unc-reparse-symlink', families: ['linux', 'macos', 'windows'], cell_id: null, harness: 'black-box', emits_activeness: false },
  { id: 'process-cleanup', dimension: 'process-cleanup', families: ['linux', 'macos', 'windows'], cell_id: null, harness: 'black-box', emits_activeness: false },
  { id: 'suspend-resume', dimension: 'suspend-resume', families: ['linux', 'macos', 'windows'], cell_id: null, harness: 'black-box', emits_activeness: false },
  { id: 'offline', dimension: 'offline', families: ['linux', 'macos', 'windows'], cell_id: null, harness: 'black-box', emits_activeness: false },
  { id: 'disk-full-read-only', dimension: 'disk-full-read-only', families: ['linux', 'macos', 'windows'], cell_id: null, harness: 'black-box', emits_activeness: false },
  { id: 'hostile-inputs', dimension: 'hostile-inputs', families: ['linux', 'macos', 'windows'], cell_id: null, harness: 'black-box', emits_activeness: false },
  { id: 'w-sandbox-silent-disable', dimension: 'sandbox-probes', families: ['windows'], cell_id: 'w-sandbox-silent-disable', harness: 'black-box', emits_activeness: true },
  { id: 'w-process-cleanup-descendant-tree', dimension: 'process-cleanup', families: ['windows'], cell_id: 'w-process-cleanup-descendant-tree', harness: 'black-box', emits_activeness: false },
  { id: 'w-sandbox-observability-control', dimension: 'sandbox-probes', families: ['windows'], cell_id: 'w-sandbox-observability-control', harness: 'black-box', emits_activeness: true },
];

export const DIMENSIONS = [
  'sandbox-probes', 'unicode', 'long-paths', 'unc-reparse-symlink', 'process-cleanup',
  'suspend-resume', 'offline', 'disk-full-read-only', 'hostile-inputs',
];

const SANDBOX_DIMENSION = 'sandbox-probes';
const PLATFORMS = ['linux', 'macos', 'windows'];

// ---------------------------------------------------------------------------------
// Failure plumbing
// ---------------------------------------------------------------------------------

export class MatrixProofError extends Error {
  constructor(message) {
    super(message);
    this.name = 'MatrixProofError';
  }
}

function fail(message) {
  throw new MatrixProofError(message);
}

// ---------------------------------------------------------------------------------
// Exact-byte reader — no symlink follow, no reopen by pathname
// ---------------------------------------------------------------------------------

const NOFOLLOW = constants.O_NOFOLLOW ?? 0;

export function readExactBytesNoFollow(path) {
  let fd;
  try {
    fd = openSync(path, constants.O_RDONLY | NOFOLLOW);
  } catch (err) {
    if (err && (err.code === 'ELOOP' || err.code === 'EMLINK')) {
      fail(`refused to follow symlink at authority path: ${path}`);
    }
    if (err && err.code === 'ENOENT') fail(`missing authority artifact: ${path}`);
    throw err;
  }
  try {
    const st = fstatSync(fd);
    if (!st.isFile()) fail(`authority path is not a regular file: ${path}`);
    const buf = Buffer.allocUnsafe(st.size);
    let off = 0;
    while (off < st.size) {
      const n = readSync(fd, buf, off, st.size - off, off);
      if (n === 0) break;
      off += n;
    }
    if (off !== st.size) fail(`short read of authority artifact: ${path}`);
    return buf;
  } finally {
    closeSync(fd);
  }
}

export function parseExactLines(bytes) {
  if (!Buffer.isBuffer(bytes)) fail('expected retained Buffer');
  if (bytes.length === 0) fail('empty authority artifact');
  if (bytes.includes(0x0d)) fail('CR byte in authority artifact (LF grammar required)');
  if (bytes[bytes.length - 1] !== 0x0a) fail('authority artifact missing final newline');
  const lines = bytes.toString('utf8').slice(0, -1).split('\n');
  for (const line of lines) if (line.length === 0) fail('blank line in authority artifact');
  return lines;
}

// ---------------------------------------------------------------------------------
// Marker grammar
// ---------------------------------------------------------------------------------

const HEX40 = /^[0-9a-f]{40}$/;
const NONCE_RE = /^[0-9a-f]{32,64}$/;

const CELL_LINE_RE =
  /^F28_CELL platform=([a-z]+) cell=([A-Za-z0-9:_.\/-]+) probe=([a-z0-9-]+) outcome=(pass|red|skip) activeness=(none|observed) commit=([0-9a-f]{40}) tree=([0-9a-f]{40}) nonce=([0-9a-f]{32,64})$/;

const FINAL_PREFIX = {
  linux: 'F28_FINAL_LINUX',
  macos: 'F28_FINAL_MACOS',
  windows: 'F28_FINAL_WINDOWS',
};

export function cellMarker(platform, cell, probe, outcome, activeness, commit, tree, nonce) {
  return `F28_CELL platform=${platform} cell=${cell} probe=${probe} outcome=${outcome} activeness=${activeness} commit=${commit} tree=${tree} nonce=${nonce}`;
}

export function finalMarker(platform, count, commit, tree, nonce) {
  return `${FINAL_PREFIX[platform]} cells=${count} commit=${commit} tree=${tree} nonce=${nonce}`;
}

/**
 * Verify a matrix marker log, fail-closed.
 *
 * `expectedCells` is the DECLARED ORDER: every entry must appear exactly once, in this
 * order, and nothing else may appear. Each entry is `{ cell, dimension }` so the
 * activeness rule can be applied at the marker layer as well as at the results layer —
 * a sandbox cell claiming `pass` with `activeness=none` is rejected here, before any
 * results file is written.
 */
export function verifyMatrixLog(bytes, { platform, commit, tree, nonce, expectedCells }) {
  if (!PLATFORMS.includes(platform)) fail(`unknown platform: ${platform}`);
  if (!HEX40.test(commit ?? '')) fail('commit must be lowercase 40-hex');
  if (!HEX40.test(tree ?? '')) fail('tree must be lowercase 40-hex');
  if (!NONCE_RE.test(nonce ?? '')) fail('nonce required');
  if (!Array.isArray(expectedCells) || expectedCells.length === 0) {
    fail('expectedCells must be a non-empty declared ordering');
  }

  const order = expectedCells.map((c) => c.cell);
  const dimensionOf = new Map(expectedCells.map((c) => [c.cell, c.dimension]));
  if (new Set(order).size !== order.length) fail('expectedCells contains a duplicate cell id');

  const lines = parseExactLines(bytes);
  const seen = [];
  let finalSeen = false;
  const expectedFinal = finalMarker(platform, order.length, commit, tree, nonce);

  for (const line of lines) {
    const m = CELL_LINE_RE.exec(line);
    if (m) {
      if (finalSeen) fail('cell marker after final acceptance marker');
      const [, mPlatform, cell, probe, outcome, activeness, mCommit, mTree, mNonce] = m;
      if (mPlatform !== platform) fail(`foreign platform marker: ${mPlatform} (verifying ${platform})`);
      if (mCommit !== commit) fail(`cell ${cell} commit drift`);
      if (mTree !== tree) fail(`cell ${cell} tree drift`);
      if (mNonce !== nonce) fail(`cell ${cell} nonce drift`);
      if (!dimensionOf.has(cell)) fail(`foreign cell marker: ${cell}`);
      if (seen.includes(cell)) fail(`duplicate cell marker: ${cell}`);
      if (!PROBES.some((p) => p.id === probe)) fail(`cell ${cell} names unknown probe ${probe}`);
      if (dimensionOf.get(cell) === SANDBOX_DIMENSION && outcome === 'pass' && activeness === 'none') {
        fail(
          `cell ${cell} claims pass on a sandbox-dimension cell with no activeness observation; ` +
            'absence of an observed violation is not evidence of a sandbox',
        );
      }
      if (dimensionOf.get(cell) !== SANDBOX_DIMENSION && activeness === 'observed') {
        fail(`cell ${cell} reports activeness on a non-sandbox dimension`);
      }
      seen.push(cell);
      continue;
    }
    if (line.startsWith(FINAL_PREFIX[platform])) {
      if (line !== expectedFinal) fail('final acceptance marker does not bind exact commit/tree/nonce/count');
      if (finalSeen) fail('duplicate final acceptance marker');
      if (seen.length !== order.length) fail('final acceptance marker before all cells were recorded');
      finalSeen = true;
      continue;
    }
    // Any other F28_-prefixed line is a foreign or spoofed marker. A plain diagnostic
    // line carrying no marker prefix is allowed as interleaved output.
    if (line.startsWith('F28_')) fail(`unrecognized matrix marker: ${line}`);
  }

  if (seen.length !== order.length) {
    fail(`missing cell markers: expected ${order.length}, saw ${seen.length}`);
  }
  for (let i = 0; i < order.length; i++) {
    if (seen[i] !== order[i]) fail(`cell markers out of order at index ${i}: saw ${seen[i]}, expected ${order[i]}`);
  }
  if (!finalSeen) fail('missing final platform acceptance marker');
  return { platform, cells: seen, commit, tree, nonce };
}

export function verifyMatrixLogFile(path, expected) {
  return verifyMatrixLog(readExactBytesNoFollow(path), expected);
}

// ---------------------------------------------------------------------------------
// matrix.tsv reader
// ---------------------------------------------------------------------------------

export function readMatrixTsv(path) {
  const text = readFileSync(path, 'utf8');
  const rows = [];
  for (const line of text.split('\n')) {
    if (!line || line.startsWith('#')) continue;
    const p = line.split('\t');
    if (p.length !== 9) fail(`matrix row has ${p.length} columns, expected 9`);
    rows.push({
      cell: p[0], dimension: p[1], os: p[2], surface: p[3],
      criticality: p[4], applicability: p[5], activeness: p[8],
    });
  }
  if (rows.length === 0) fail('matrix declares no cells');
  return rows;
}

// ---------------------------------------------------------------------------------
// Probe execution — black-box against the shipped binary
// ---------------------------------------------------------------------------------

const IS_WINDOWS = process.platform === 'win32';
const BUDGET_MS = 60_000;

function verbOf(surface) {
  return surface.startsWith('cmd:') ? surface.slice(4).replace(/\//g, ' ') : surface;
}

function runBin(bin, args, { env = {}, cwd, timeout = BUDGET_MS } = {}) {
  const base = { ...process.env, ...env };
  for (const [k, v] of Object.entries(env)) if (v === null) delete base[k];
  const r = spawnSync(bin, args, {
    cwd, env: base, timeout, encoding: 'buffer', windowsHide: true,
  });
  const stdout = r.stdout ?? Buffer.alloc(0);
  const stderr = r.stderr ?? Buffer.alloc(0);
  return {
    status: r.status, signal: r.signal, error: r.error,
    timedOut: r.error && r.error.code === 'ETIMEDOUT',
    stdout, stderr,
    text: Buffer.concat([stdout, stderr]).toString('utf8'),
  };
}

function scratch(prefix) {
  return mkdtempSync(join(tmpdir(), `f28-${prefix}-`));
}

// Panic and lossy-decode signatures. A probe that only checked the exit status would
// miss a panic that the surface swallows into a zero exit.
const PANIC_RE = /panicked at|thread '.*' panicked|STATUS_ACCESS_VIOLATION|stack backtrace:/;

function red(reason) {
  return { outcome: 'red', observable: reason };
}
function pass(observable, activeness) {
  return { outcome: 'pass', observable, activeness: activeness ?? null };
}

// --- the nine dimension probes -----------------------------------------------------

const RUNNERS = {
  'unicode'(bin, verb) {
    const root = scratch('uni');
    const dir = join(root, 'ünïcode-é́-漢字-😀');
    mkdirSync(dir, { recursive: true });
    const r = runBin(bin, [...verb.split(' '), '--help'], { cwd: dir, env: { HOME: dir, USERPROFILE: dir } });
    rmSync(root, { recursive: true, force: true });
    if (r.timedOut) return red('timed out under a Unicode HOME/CWD');
    if (PANIC_RE.test(r.text)) return red(`panicked under a Unicode HOME/CWD: ${firstLine(r.text)}`);
    if (r.text.includes('�')) return red('emitted U+FFFD for well-formed input (lossy decode)');
    return pass(`exit=${r.status} under a Unicode HOME/CWD; stdout+stderr valid UTF-8, no U+FFFD, no panic`);
  },

  'long-paths'(bin, verb) {
    const root = scratch('long');
    let dir = root;
    while (dir.length < 300) dir = join(dir, 'c'.repeat(40));
    try {
      mkdirSync(dir, { recursive: true });
    } catch (e) {
      rmSync(root, { recursive: true, force: true });
      return red(`the host refused to create a ${dir.length}-char path: ${e.code ?? e.message}`);
    }
    const r = runBin(bin, [...verb.split(' '), '--help'], { cwd: dir, env: { HOME: dir, USERPROFILE: dir } });
    const len = dir.length;
    rmSync(root, { recursive: true, force: true });
    if (r.timedOut) return red(`timed out under a ${len}-char path`);
    if (PANIC_RE.test(r.text)) return red(`panicked under a ${len}-char path: ${firstLine(r.text)}`);
    if (/os error 206|ENAMETOOLONG|name too long|path too long/i.test(r.text)) {
      return red(`reported a path-length failure under a ${len}-char path: ${firstLine(r.text)}`);
    }
    return pass(`exit=${r.status} under a ${len}-char HOME/CWD; no path-length error`);
  },

  'unc-reparse-symlink'(bin, verb) {
    const root = scratch('link');
    const real = join(root, 'real');
    const link = join(root, 'link');
    mkdirSync(real, { recursive: true });
    try {
      symlinkSync(real, link, 'junction');
    } catch (e) {
      rmSync(root, { recursive: true, force: true });
      return red(`the host refused to create a reparse point: ${e.code ?? e.message}`);
    }
    const direct = runBin(bin, [...verb.split(' '), '--help'], { cwd: real, env: { HOME: real, USERPROFILE: real } });
    const viaLink = runBin(bin, [...verb.split(' '), '--help'], { cwd: link, env: { HOME: link, USERPROFILE: link } });
    rmSync(root, { recursive: true, force: true });
    if (viaLink.timedOut) return red('timed out when reached through a reparse point');
    if (PANIC_RE.test(viaLink.text)) return red(`panicked through a reparse point: ${firstLine(viaLink.text)}`);
    if (viaLink.status !== direct.status) {
      return red(`exit differs through the reparse point (link=${viaLink.status}, target=${direct.status})`);
    }
    return pass(`exit=${viaLink.status} through a reparse point, identical to the target (${direct.status})`);
  },

  'process-cleanup'(bin, verb) {
    const before = descendantSnapshot();
    const r = runBin(bin, [...verb.split(' '), '--help']);
    // The child is reaped by spawnSync's own wait; what matters is whether anything
    // it started outlived it. Give the OS a moment to reap, then enumerate again.
    sleep(1500);
    const after = descendantSnapshot();
    if (before === null || after === null) {
      return red('the host process table could not be enumerated, so no cleanup verdict is obtainable');
    }
    const survivors = [...after].filter((pid) => !before.has(pid) && isWaylandCore(pid));
    if (survivors.length > 0) {
      return red(`${survivors.length} wayland-core descendant(s) survived the invocation: ${survivors.join(',')}`);
    }
    return pass(`exit=${r.status}; the process table shows no surviving wayland-core descendant`);
  },

  // The suspension is performed by a SYNCHRONOUS wrapper rather than from this
  // process's event loop. An asynchronous child driven from a loop that this probe
  // then blocks never gets its exit delivered, and the probe reports a timeout that
  // belongs to the harness rather than to the product. That defect was found and
  // repaired during this plan's first harness iteration; the shape is recorded here
  // because "the measurement timed out" and "the product hung" are indistinguishable
  // from the outside and only one of them is a finding.
  'suspend-resume'(bin, verb) {
    const baseline = runBin(bin, [...verb.split(' '), '--help']);
    const args = verb.split(' ');

    if (IS_WINDOWS) {
      // NtSuspendProcess/NtResumeProcess through the in-box .NET compiler. This needs
      // no toolchain install and does not build the product, so the probe stays
      // black-box against the shipped binary.
      const ps = [
        '$ErrorActionPreference="Stop"',
        'Add-Type -Namespace F28 -Name P -MemberDefinition \'[DllImport("ntdll.dll")] public static extern int NtSuspendProcess(IntPtr h); [DllImport("ntdll.dll")] public static extern int NtResumeProcess(IntPtr h);\'',
        `$p = Start-Process -FilePath '${bin}' -ArgumentList ${args.map((a) => `'${a}'`).join(',')},'--help' -PassThru -NoNewWindow -RedirectStandardOutput $env:TEMP\\f28sr.out -RedirectStandardError $env:TEMP\\f28sr.err`,
        // Suspend at the earliest possible moment, with NO sleep first: a short-lived
        // invocation would otherwise finish before the suspension could be applied and
        // the probe would report a race as a product result.
        '$s = [F28.P]::NtSuspendProcess($p.Handle)',
        'Start-Sleep -Milliseconds 400',
        '$stopped = -not $p.HasExited',
        '$r = [F28.P]::NtResumeProcess($p.Handle)',
        '$p.WaitForExit(60000) | Out-Null',
        'Write-Output "F28SR suspend=$s resume=$r stopped=$stopped exit=$($p.ExitCode)"',
      ].join('; ');
      const w = runBin('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', ps], {
        timeout: 90_000,
      });
      if (w.timedOut) return red('the suspend/resume wrapper did not complete within its budget');
      const m = /F28SR suspend=(-?\d+) resume=(-?\d+) stopped=(\w+) exit=(-?\d+)/.exec(w.text);
      if (!m) {
        return red(`suspend/resume could not be driven on this host: ${firstLine(w.text)}`);
      }
      const [, s, r, stopped, exit] = m;
      if (s !== '0' || r !== '0') {
        return red(`NtSuspendProcess/NtResumeProcess refused (suspend=${s}, resume=${r}); no resume verdict is obtainable`);
      }
      if (stopped !== 'True') {
        return red(
          'the invocation completed before the suspension could be observed to hold; the ' +
            'measurement was not taken and is therefore not a pass',
        );
      }
      if (Number(exit) !== baseline.status) {
        return red(`exit differs across the suspension (suspended=${exit}, baseline=${baseline.status})`);
      }
      return pass(
        `exit=${exit} across NtSuspendProcess/NtResumeProcess, held suspended for 400ms ` +
          `(observed not-exited while suspended), identical to the unsuspended baseline`,
      );
    }

    // POSIX. The child is started ALREADY STOPPED — the wrapper shell signals itself
    // before `exec`, so the suspension is deterministic rather than a race against a
    // short-lived invocation. The stopped state is then OBSERVED in the process table
    // (`state` begins with `T`) rather than assumed, which is what makes this a
    // measurement instead of a hopeful sleep.
    const script =
      'sh -c \'kill -STOP $$; exec "$@"\' f28inner "$@" & p=$!; ' +
      'st=""; i=0; while [ $i -lt 60 ]; do st=$(ps -o state= -p "$p" 2>/dev/null | tr -d " "); ' +
      'case "$st" in T*) break;; esac; i=$((i+1)); sleep 0.05; done; ' +
      'echo "F28SR observed_state=${st:-gone}"; ' +
      'kill -CONT "$p" 2>/dev/null; wait "$p"; echo "F28SR exit=$?"';
    const w = runBin('/bin/sh', ['-c', script, 'f28', bin, ...args, '--help'], { timeout: 90_000 });
    if (w.timedOut) return red('the process did not complete within its budget after resume');
    const state = /F28SR observed_state=(\S+)/.exec(w.text);
    const m = /F28SR exit=(-?\d+)/.exec(w.text);
    if (!state || !m) return red(`the process could not be suspended and resumed: ${firstLine(w.text)}`);
    if (!state[1].startsWith('T')) {
      return red(
        `the process was never observed in a stopped state (process state was ` +
          `'${state[1]}'); the suspension was not established, so this is not a pass`,
      );
    }
    if (PANIC_RE.test(w.text)) return red(`panicked across the suspension: ${firstLine(w.text)}`);
    if (Number(m[1]) !== baseline.status) {
      return red(`exit differs across the suspension (suspended=${m[1]}, baseline=${baseline.status})`);
    }
    return pass(
      `exit=${m[1]} across SIGSTOP/SIGCONT with the stopped state OBSERVED in the process ` +
        `table (state=${state[1]}), identical to the unsuspended baseline`,
    );
  },

  'offline'(bin, verb) {
    // A closed local port: connect() fails immediately rather than hanging, so a hang
    // is the product's, not the fixture's.
    const blackhole = 'http://127.0.0.1:1/';
    const r = runBin(bin, [...verb.split(' '), '--help'], {
      env: {
        HTTP_PROXY: blackhole, HTTPS_PROXY: blackhole, ALL_PROXY: blackhole,
        http_proxy: blackhole, https_proxy: blackhole, all_proxy: blackhole,
        NO_PROXY: '', no_proxy: '',
      },
      timeout: 30_000,
    });
    if (r.timedOut) return red('hung past its 30s budget with egress refused');
    if (PANIC_RE.test(r.text)) return red(`panicked with egress refused: ${firstLine(r.text)}`);
    return pass(`exit=${r.status} with all egress pointed at a closed local port; completed within budget, no panic`);
  },

  // Two mechanisms, tried in order, and the one that ACTUALLY established the
  // condition is named in the observable. Permission bits alone are not enough: the
  // certification Linux host runs as root, and root bypasses them — a probe that
  // trusted `chmod 0500` there would report a pass over a fully writable directory.
  // The canary write is what catches that, and the fallback is what keeps the
  // dimension measurable rather than reporting 24 environment reds.
  'disk-full-read-only'(bin, verb) {
    const root = scratch('ro');
    const home = join(root, 'home');
    let mechanism = 'permission bits (0500)';
    mkdirSync(home, { recursive: true });
    try {
      chmodSync(home, 0o500);
    } catch {
      /* fall through to the canary check, which is what decides */
    }
    const canaryFails = (dir) => {
      try {
        writeFileSync(join(dir, '.f28-writable-canary'), 'x');
        return false;
      } catch {
        return true;
      }
    };
    let target = home;
    if (!canaryFails(home)) {
      // Root ignores the mode bits. Point HOME at a REGULAR FILE instead: every write
      // beneath it then fails with ENOTDIR for every uid, root included, so the
      // unwritable condition is established by the filesystem's own type system rather
      // than by a permission the caller can bypass.
      target = join(root, 'home-is-a-file');
      writeFileSync(target, 'f28: HOME is deliberately not a directory\n');
      mechanism = 'HOME bound to a regular file (ENOTDIR, unbypassable by root)';
      if (!canaryFails(target)) {
        rmSync(root, { recursive: true, force: true });
        return red(
          'neither permission bits nor a non-directory HOME made the fixture unwritable, ' +
            'so the read-only condition was never established and this is not a measurement',
        );
      }
    }
    const r = runBin(bin, [...verb.split(' '), '--help'], {
      env: { HOME: target, USERPROFILE: target },
    });
    try { chmodSync(home, 0o700); } catch { /* best effort */ }
    rmSync(root, { recursive: true, force: true });
    if (r.timedOut) return red('timed out with a read-only HOME');
    if (PANIC_RE.test(r.text)) return red(`panicked with a read-only HOME: ${firstLine(r.text)}`);
    return pass(
      `exit=${r.status} with an unwritable HOME established by ${mechanism} and proved by ` +
        `a canary write that was refused; no panic`,
    );
  },

  'hostile-inputs'(bin, verb) {
    const root = scratch('hostile');
    const canary = join(root, 'canary');
    const vector = [
      `; touch ${canary}`,
      `$(touch ${canary})`,
      '`touch ' + canary + '`',
      '&& echo pwned',
      '[31m[2J',
      'A'.repeat(65536),
      '--',
    ];
    const r = runBin(bin, [...verb.split(' '), ...vector], { cwd: root, timeout: 30_000 });
    const canaryExists = existsSync(canary);
    const raw = r.stdout.toString('latin1') + r.stderr.toString('latin1');
    rmSync(root, { recursive: true, force: true });
    if (r.timedOut) return red('hung past its 30s budget on a hostile argv vector');
    if (canaryExists) return red('a shell metacharacter was INTERPRETED — the command-substitution canary exists');
    if (PANIC_RE.test(r.text)) return red(`panicked on a hostile argv vector: ${firstLine(r.text)}`);
    if (/\[2J/.test(raw)) return red('an unescaped terminal-clearing control sequence was echoed back');
    return pass(`exit=${r.status} on a hostile argv vector; no canary, no unescaped control sequence, no panic`);
  },

  // The sandbox dimension probe. Its PASS requires a positive activeness observation;
  // where none can be captured it is a RED, never a green and never a skip.
  'sandbox-probes'(bin, verb, ctx) {
    const activeness = ctx && ctx.activeness;
    // Leg 1: the surface must not perform sandboxed work while the backend is refused.
    const refused = runBin(bin, [...verb.split(' '), '--help'], {
      env: { WAYLAND_SANDBOX: 'none', WAYLAND_ALLOW_NO_SANDBOX: null },
      timeout: 30_000,
    });
    if (refused.timedOut) return red('hung with the sandbox backend refused');
    if (PANIC_RE.test(refused.text)) return red(`panicked with the sandbox backend refused: ${firstLine(refused.text)}`);
    if (!activeness || activeness.observed !== true) {
      const reason = (activeness && activeness.reason) || 'no activeness observation was supplied for this run';
      return {
        outcome: 'red',
        observable:
          `the surface did not misbehave with the backend refused (exit=${refused.status}), but no positive ` +
          `activeness observation is available, so a green would be indistinguishable from a silently ` +
          `disabled sandbox: ${reason}`,
      };
    }
    return pass(
      `exit=${refused.status} with WAYLAND_SANDBOX=none and no opt-in; activeness observed independently`,
      activeness,
    );
  },
};

function firstLine(text) {
  return (text.split('\n').find((l) => l.trim().length > 0) ?? '').slice(0, 160);
}

// ---------------------------------------------------------------------------------
// --capture-activeness — the positive containment observation, measured DIFFERENTIALLY
// ---------------------------------------------------------------------------------
//
// The same probe script is run OUTSIDE the product and INSIDE a worker the product
// spawns through its own sandbox path. Activeness is asserted from the DIFFERENCE, not
// from the absence of a violation: the process-id namespace, the visible root, and DNS
// reachability all change when the sandbox is active and none of them changes when it
// is not. A detector that fired on the inside reading alone could not tell a contained
// child from an uncontained one, which is exactly the failure this rule exists to stop.
//
// It is a MEASUREMENT and it can fail. Where no worker can be spawned at all — as on
// macOS, where the delegated-execution path refuses because sandbox-exec does not meet
// the delegated admission contract and the Docker fallback is compiled out — this
// returns `observed: false` with the reason, and every sandbox cell on that family is
// then a RED. That is the honest outcome; a green there would be indistinguishable
// from a silently disabled sandbox.

const ACTIVENESS_PROBE = [
  'echo F28RAN',
  'echo F28_NSPID=$(grep -E "^NSpid" /proc/self/status 2>/dev/null | tr -s " \\t" ":" || echo none)',
  'echo F28_ROOTLS=$(ls / 2>/dev/null | tr "\\n" ",")',
  '(getent hosts github.com >/dev/null 2>&1 || nslookup github.com >/dev/null 2>&1) && echo F28_DNS=RESOLVES || echo F28_DNS=NO_DNS',
  // F-28-02-001. A filesystem-read signal, because the three signals above are
  // all namespace-derived and macOS's sandbox-exec has no PID or mount
  // namespace: NSpid and the root listing are identical inside and out there,
  // leaving DNS as the only differential. `/etc` is granted by neither the
  // `contained` workspace policy nor the macOS profile's read allowlist
  // (/usr, /System, /Library, /bin, /sbin), so it is denied inside and
  // readable outside. Inert on the other two families — Linux bwrap
  // read-binds /etc (readable both sides) and Windows has no /etc (denied
  // both sides) — so it can only ever ADD a difference, never remove one.
  '(head -c 1 /etc/hosts >/dev/null 2>&1 && echo F28_ETC=READ) || echo F28_ETC=DENIED',
  '(whoami /groups 2>&1 | head -3) 2>/dev/null || true',
].join('\n') + '\n';

function gitInit(dir, files) {
  mkdirSync(dir, { recursive: true });
  const g = (args) => spawnSync('git', args, { cwd: dir, encoding: 'utf8' });
  g(['init', '-q', '-b', 'main', '.']);
  g(['config', 'user.email', 'ci@f28.local']);
  g(['config', 'user.name', 'f28']);
  for (const [name, body] of Object.entries(files)) writeFileSync(join(dir, name), body);
  writeFileSync(join(dir, '.gitignore'), '.swarm-worktrees/\n');
  g(['add', '-A']);
  g(['-c', 'commit.gpgsign=false', 'commit', '-q', '-m', 'f28']);
}

function summarise(raw) {
  // The swarm reports the worker's stdout as a JSON string, so real newlines arrive as
  // the two characters `\` `n`. Un-escape before scanning, or a field regex runs past
  // the end of its own line and swallows the next three.
  const text = raw.replace(/\\r\\n|\\n/g, '\n');
  const g = (re) => (re.exec(text) ?? [])[1] ?? null;
  return {
    ran: /F28RAN/.test(text),
    nspid: g(/F28_NSPID=(\S*)/),
    rootls: g(/F28_ROOTLS=(\S*)/),
    dns: g(/F28_DNS=(\S*)/),
    etc: g(/F28_ETC=(\S*)/),
    appcontainer: /0xC0000022|BaseNamedObjects/.test(text),
    accessDenied: /Access is denied/.test(text),
    highIntegrity: /S-1-16-12288/.test(text),
  };
}

export function captureActiveness(bin) {
  const root = scratch('act');
  const repo = join(root, 'repo');
  gitInit(repo, { 'probe.sh': ACTIVENESS_PROBE, 'README.md': 'seed\n' });

  const outside = summarise(
    runBin('/bin/sh', ['probe.sh'], { cwd: repo, timeout: 30_000 }).text +
      (IS_WINDOWS ? runBin('cmd.exe', ['/c', 'whoami', '/groups'], { timeout: 30_000 }).text : ''),
  );
  const swarm = runBin(
    bin,
    ['swarm', '--workers', '1', '--worker-command', IS_WINDOWS ? 'cmd.exe /c echo F28RAN & whoami /groups' : '/bin/sh probe.sh',
     '--repo', repo, '--base-branch', 'main', '--timeout', '90s'],
    { env: { HOME: root, USERPROFILE: root }, timeout: 180_000 },
  );
  let inside = summarise(swarm.text);
  let via = 'swarm';
  let fallbackText = '';

  // F-28-02-001. The delegated path is not the only containment path the
  // product has. When it cannot spawn a worker AT ALL — as on macOS, where
  // sandbox-exec does not meet the delegated admission contract and the
  // Docker fallback is compiled out — take the inside reading through
  // `sandbox exec`, which runs the probe through the SAME backend selection
  // and the SAME shell tool the agent uses for every command.
  //
  // This does NOT relax the activeness rule. The differential and every
  // signal in it are unchanged; only the surface that produces the inside
  // reading differs, and it is recorded. A run that still shows no difference
  // is still `observed: false`, and a family with no obtainable difference is
  // still RED.
  if (!inside.ran) {
    const direct = runBin(
      bin,
      ['sandbox', 'exec', '--workspace', repo,
       IS_WINDOWS ? 'echo F28RAN & whoami /groups' : 'sh probe.sh'],
      { env: { HOME: root, USERPROFILE: root }, timeout: 180_000 },
    );
    fallbackText = direct.text;
    const alternate = summarise(direct.text);
    if (alternate.ran) {
      inside = alternate;
      via = 'sandbox-exec-surface';
    }
  }
  rmSync(root, { recursive: true, force: true });

  if (!inside.ran) {
    return {
      observed: false,
      reason:
        'no worker could be spawned through the product\'s own sandbox path, so no ' +
        'containment differential is obtainable: ' + firstLine(swarm.text) +
        (fallbackText ? '; nor through `sandbox exec`: ' + firstLine(fallbackText) : ''),
      raw: (swarm.text + '\n' + fallbackText).slice(0, 1200),
    };
  }

  const differences = [];
  if (inside.nspid && outside.nspid && inside.nspid !== outside.nspid) {
    differences.push(`process-id namespace changed (${outside.nspid} outside, ${inside.nspid} inside)`);
  }
  if (inside.rootls && outside.rootls && inside.rootls !== outside.rootls) {
    const o = outside.rootls.split(',').filter(Boolean).length;
    const i = inside.rootls.split(',').filter(Boolean).length;
    if (i < o) differences.push(`filesystem root reduced from ${o} entries to ${i} (mount namespace)`);
  }
  if (outside.dns === 'RESOLVES' && inside.dns === 'NO_DNS') {
    differences.push('DNS resolves outside and does not inside (network namespace)');
  }
  if (outside.etc === 'READ' && inside.etc === 'DENIED') {
    differences.push('/etc is readable outside and denied inside (filesystem read confined)');
  }
  if (inside.appcontainer && !outside.appcontainer) {
    differences.push('the child was refused \\BaseNamedObjects with 0xC0000022, which AppContainer confines by construction');
  }
  if (inside.accessDenied && outside.highIntegrity && !inside.highIntegrity) {
    differences.push('a System32 binary the uncontained context runs was refused to the child, and the child holds no High integrity label');
  }

  if (differences.length === 0) {
    return {
      observed: false,
      reason:
        'a worker ran but showed NO containment difference from the uncontained baseline; ' +
        'the sandbox cannot be evidenced active for this run',
      raw: swarm.text.slice(0, 1200),
    };
  }
  return {
    observed: true,
    probe: 'containment-differential',
    // The surface that produced the inside reading is part of the evidence:
    // a reader must be able to tell which containment path was exercised.
    detail: differences.join('; ') + ` [inside reading via ${via}]`,
  };
}

function sleep(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}


function descendantSnapshot() {
  try {
    if (IS_WINDOWS) {
      const out = execFileSync('tasklist', ['/FO', 'CSV', '/NH'], { encoding: 'utf8' });
      const pids = new Set();
      for (const line of out.split('\n')) {
        const m = /^"([^"]*)","(\d+)"/.exec(line.trim());
        if (m) pids.add(`${m[2]}:${m[1]}`);
      }
      return pids;
    }
    const out = execFileSync('ps', ['-A', '-o', 'pid=,comm='], { encoding: 'utf8' });
    const pids = new Set();
    for (const line of out.split('\n')) {
      const m = /^\s*(\d+)\s+(.*)$/.exec(line);
      if (m) pids.add(`${m[1]}:${m[2].trim()}`);
    }
    return pids;
  } catch {
    return null;
  }
}

function isWaylandCore(entry) {
  return /wayland-core/i.test(entry);
}

// ---------------------------------------------------------------------------------
// --run
// ---------------------------------------------------------------------------------

export function runMatrix({ bin, os, commit, tree, nonce, rows, activeness, emit }) {
  const results = [];
  let seq = 0;
  for (const row of rows) {
    if (row.os !== os) continue;
    const spec =
      PROBES.find((p) => p.cell_id === row.cell) ??
      PROBES.find((p) => p.cell_id === null && p.dimension === row.dimension && p.families.includes(row.os));
    if (!spec) {
      // A cell with no probe fails the run rather than being reported absent.
      fail(`cell ${row.cell} has no probe on ${row.os}`);
    }
    const runner = RUNNERS[spec.dimension];
    let out;
    if (!runner) {
      out = red(`no executor is implemented for probe ${spec.id}; the measurement was not taken`);
    } else {
      try {
        out = runner(bin, verbOf(row.surface), { activeness, row, spec });
      } catch (e) {
        out = red(`the probe threw: ${e && e.message ? e.message : String(e)}`);
      }
    }
    const activenessField =
      row.dimension === SANDBOX_DIMENSION && out.outcome === 'pass' && out.activeness ? 'observed' : 'none';
    const line = cellMarker(os, row.cell, spec.id, out.outcome, activenessField, commit, tree, nonce);
    emit(line);
    results.push({ ...row, probe: spec.id, seq: ++seq, ...out });
  }
  emit(finalMarker(os, results.length, commit, tree, nonce));
  return results;
}

// ---------------------------------------------------------------------------------
// --self-test — every rejection tripped, and the good log accepted
// ---------------------------------------------------------------------------------

const C = 'a'.repeat(40);
const T = 'b'.repeat(40);
const N = 'c'.repeat(32);

function goodExpected() {
  return [
    { cell: 'sandbox-probes-linux-alpha', dimension: 'sandbox-probes' },
    { cell: 'unicode-linux-alpha', dimension: 'unicode' },
    { cell: 'offline-linux-alpha', dimension: 'offline' },
  ];
}

function goodLines() {
  return [
    'some interleaved diagnostic output that carries no marker prefix',
    cellMarker('linux', 'sandbox-probes-linux-alpha', 'sandbox-probes', 'pass', 'observed', C, T, N),
    cellMarker('linux', 'unicode-linux-alpha', 'unicode', 'pass', 'none', C, T, N),
    cellMarker('linux', 'offline-linux-alpha', 'offline', 'red', 'none', C, T, N),
    finalMarker('linux', 3, C, T, N),
  ];
}

function buf(lines) {
  return Buffer.from(lines.join('\n') + '\n', 'utf8');
}

function selfTest() {
  let passed = 0;
  const failures = [];
  const expected = { platform: 'linux', commit: C, tree: T, nonce: N, expectedCells: goodExpected() };

  const ok = (name, fn) => {
    try { fn(); passed++; } catch (e) { failures.push(`GOOD FIXTURE REJECTED [${name}]: ${e.message}`); }
  };
  const rejects = (name, needle, fn) => {
    try { fn(); } catch (e) {
      if (String(e.message).includes(needle)) { passed++; }
      else failures.push(`[${name}] expected a rejection mentioning "${needle}", got: ${e.message}`);
      return;
    }
    failures.push(`[${name}] expected a rejection mentioning "${needle}", but the fixture was ACCEPTED`);
  };

  ok('good log', () => verifyMatrixLog(buf(goodLines()), expected));

  // Two shapes of "absent", because they are caught at different points and a reader
  // who only saw one could believe the other was tolerated.
  rejects('absent cell, log truncated before the final marker', 'missing cell markers', () =>
    verifyMatrixLog(buf(goodLines().filter((l) => !l.includes('offline-linux-alpha') && !l.startsWith('F28_FINAL'))), expected));

  rejects('absent cell, final marker still claims the full count', 'final acceptance marker before all cells', () =>
    verifyMatrixLog(buf(goodLines().filter((l) => !l.includes('offline-linux-alpha'))), expected));

  rejects('duplicate cell', 'duplicate cell marker', () => {
    const l = goodLines();
    l.splice(3, 0, cellMarker('linux', 'unicode-linux-alpha', 'unicode', 'pass', 'none', C, T, N));
    return verifyMatrixLog(buf(l), expected);
  });

  rejects('reordered cells', 'out of order', () => {
    const l = goodLines();
    const tmp = l[1]; l[1] = l[2]; l[2] = tmp;
    return verifyMatrixLog(buf(l), expected);
  });

  rejects('foreign cell', 'foreign cell marker', () => {
    const l = goodLines();
    l.splice(4, 0, cellMarker('linux', 'unicode-linux-nosuch', 'unicode', 'pass', 'none', C, T, N));
    return verifyMatrixLog(buf(l), expected);
  });

  rejects('foreign platform', 'foreign platform marker', () => {
    const l = goodLines();
    l[2] = cellMarker('windows', 'unicode-linux-alpha', 'unicode', 'pass', 'none', C, T, N);
    return verifyMatrixLog(buf(l), expected);
  });

  rejects('final before cells', 'final acceptance marker before all cells', () => {
    const l = goodLines();
    l.splice(2, 0, finalMarker('linux', 3, C, T, N));
    return verifyMatrixLog(buf(l), expected);
  });

  rejects('cell after final', 'cell marker after final', () => {
    const l = goodLines();
    l.push(cellMarker('linux', 'unicode-linux-alpha', 'unicode', 'pass', 'none', C, T, N));
    return verifyMatrixLog(buf(l), expected);
  });

  rejects('duplicate final', 'duplicate final acceptance marker', () =>
    verifyMatrixLog(buf([...goodLines(), finalMarker('linux', 3, C, T, N)]), expected));

  rejects('missing final', 'missing final platform acceptance marker', () =>
    verifyMatrixLog(buf(goodLines().slice(0, -1)), expected));

  rejects('final count drift', 'does not bind exact', () => {
    const l = goodLines();
    l[l.length - 1] = finalMarker('linux', 2, C, T, N);
    return verifyMatrixLog(buf(l), expected);
  });

  rejects('commit drift', 'commit drift', () => {
    const l = goodLines();
    l[2] = cellMarker('linux', 'unicode-linux-alpha', 'unicode', 'pass', 'none', 'd'.repeat(40), T, N);
    return verifyMatrixLog(buf(l), expected);
  });

  rejects('tree drift', 'tree drift', () => {
    const l = goodLines();
    l[2] = cellMarker('linux', 'unicode-linux-alpha', 'unicode', 'pass', 'none', C, 'd'.repeat(40), N);
    return verifyMatrixLog(buf(l), expected);
  });

  rejects('nonce drift', 'nonce drift', () => {
    const l = goodLines();
    l[2] = cellMarker('linux', 'unicode-linux-alpha', 'unicode', 'pass', 'none', C, T, 'd'.repeat(32));
    return verifyMatrixLog(buf(l), expected);
  });

  rejects('unrecognized marker', 'unrecognized matrix marker', () =>
    verifyMatrixLog(buf([...goodLines().slice(0, 4), 'F28_SOMETHING_ELSE ok=1', goodLines()[4]]), expected));

  rejects('sandbox green without activeness', 'absence of an observed violation', () => {
    const l = goodLines();
    l[1] = cellMarker('linux', 'sandbox-probes-linux-alpha', 'sandbox-probes', 'pass', 'none', C, T, N);
    return verifyMatrixLog(buf(l), expected);
  });

  rejects('activeness on a non-sandbox cell', 'reports activeness on a non-sandbox', () => {
    const l = goodLines();
    l[2] = cellMarker('linux', 'unicode-linux-alpha', 'unicode', 'pass', 'observed', C, T, N);
    return verifyMatrixLog(buf(l), expected);
  });

  rejects('CRLF log', 'CR byte', () =>
    verifyMatrixLog(Buffer.from(goodLines().join('\r\n') + '\r\n', 'utf8'), expected));

  rejects('no trailing newline', 'missing final newline', () =>
    verifyMatrixLog(Buffer.from(goodLines().join('\n'), 'utf8'), expected));

  rejects('empty log', 'empty authority artifact', () => verifyMatrixLog(Buffer.alloc(0), expected));

  rejects('unknown probe', 'names unknown probe', () => {
    const l = goodLines();
    l[2] = cellMarker('linux', 'unicode-linux-alpha', 'made-up-probe', 'pass', 'none', C, T, N);
    return verifyMatrixLog(buf(l), expected);
  });

  rejects('empty expected set makes the gate vacuous', 'non-empty declared ordering', () =>
    verifyMatrixLog(buf(goodLines()), { ...expected, expectedCells: [] }));

  // The probe table itself: nine dimensions, verbatim and fixed.
  ok('nine dimensions present', () => {
    const declared = new Set(PROBES.map((p) => p.dimension));
    for (const d of DIMENSIONS) if (!declared.has(d)) fail(`dimension ${d} has no probe`);
    if (declared.size !== DIMENSIONS.length) fail('the probe table declares a dimension outside F28-01');
  });
  ok('sandbox probes emit activeness', () => {
    for (const p of PROBES) {
      if (p.dimension === SANDBOX_DIMENSION && !p.emits_activeness) fail(`${p.id} emits no activeness`);
    }
  });

  process.stdout.write(`self-test: ${passed} assertions passed, ${failures.length} failed\n`);
  for (const f of failures) process.stdout.write(`  FAIL ${f}\n`);
  return failures.length === 0 ? 0 : 1;
}

// ---------------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------------

function arg(argv, name) {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : undefined;
}

if (process.argv[1] && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))) {
  const argv = process.argv.slice(2);
  try {
    if (argv.includes('--self-test')) {
      process.exit(selfTest());
    } else if (argv.includes('--capture-activeness')) {
      const bin = arg(argv, '--bin');
      if (!bin) fail('--capture-activeness requires --bin');
      const result = captureActiveness(bin);
      const out = arg(argv, '--out');
      const text = JSON.stringify(result, null, 1) + '\n';
      if (out) writeFileSync(out, text, 'utf8');
      process.stdout.write(text);
      process.exit(0);
    } else if (argv.includes('--run')) {
      const bin = arg(argv, '--bin');
      const os = arg(argv, '--os');
      const commit = arg(argv, '--commit');
      const tree = arg(argv, '--tree');
      const nonce = arg(argv, '--nonce');
      const matrix = arg(argv, '--matrix');
      const activenessPath = arg(argv, '--activeness');
      if (!bin || !os || !commit || !tree || !nonce || !matrix) {
        fail('--run requires --bin --os --commit --tree --nonce --matrix');
      }
      const activeness = activenessPath ? JSON.parse(readFileSync(activenessPath, 'utf8')) : null;
      const rows = readMatrixTsv(matrix).filter((r) => r.os === os);
      const out = [];
      const results = runMatrix({
        bin, os, commit, tree, nonce, rows, activeness,
        emit: (line) => { out.push(line); process.stderr.write(line + '\n'); },
      });
      const logPath = arg(argv, '--log');
      if (logPath) writeFileSync(logPath, out.join('\n') + '\n', 'utf8');
      const jsonPath = arg(argv, '--json');
      if (jsonPath) writeFileSync(jsonPath, JSON.stringify(results, null, 1) + '\n', 'utf8');
      const bad = results.filter((r) => r.outcome === 'red').length;
      process.stdout.write(`cells=${results.length} red=${bad}\n`);
      process.exit(0);
    } else if (argv.includes('--verify')) {
      const log = arg(argv, '--verify');
      const matrix = arg(argv, '--matrix');
      const os = arg(argv, '--os');
      const rows = readMatrixTsv(matrix).filter((r) => r.os === os);
      const res = verifyMatrixLogFile(log, {
        platform: os,
        commit: arg(argv, '--commit'),
        tree: arg(argv, '--tree'),
        nonce: arg(argv, '--nonce'),
        expectedCells: rows.map((r) => ({ cell: r.cell, dimension: r.dimension })),
      });
      process.stdout.write(`VERIFIED platform=${res.platform} cells=${res.cells.length}\n`);
      process.exit(0);
    } else {
      process.stderr.write('usage: f28-native-matrix.mjs [--self-test | --run ... | --verify <log> ...]\n');
      process.exit(2);
    }
  } catch (err) {
    process.stderr.write(`${err && err.message ? err.message : err}\n`);
    process.exit(1);
  }
}
