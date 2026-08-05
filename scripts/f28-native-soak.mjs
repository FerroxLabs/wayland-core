#!/usr/bin/env node
// f28-native-soak.mjs -- the F28-02 1,000-session soak EXECUTOR.
//
// WHY THIS IS NODE AND NOT A CARGO TEST
// ------------------------------------
// Identical to scripts/f28-native-matrix.mjs and forced by the same constraint: the
// certification Mac may run the CI-produced `wayland-core` binary but may NOT run cargo
// beyond `cargo fmt --all -- --check`. A soak implemented as a cargo-built harness cannot
// run there at all, and an observable whose only implementation is such a harness silently
// loses a whole OS family.
//
// The CANONICAL definitions -- geometry, the six canary channels, the per-platform census
// backends, the four observables and every VOID rule -- live in
// crates/wcore-eval-scenarios/src/e5_soak.rs. tests/e5_soak_contract.rs asserts this file
// agrees with them, so the executor cannot drift from the definition it implements.
//
// THE RULE THIS FILE EXISTS TO SERVE
// ----------------------------------
// A canary scan reporting zero detections and a canary scan that never ran produce
// identical output. So every observable carries a POSITIVE CONTROL and a run whose control
// was missed is VOID, never green.
//
// The canary control validates the DETECTOR, which is what the indistinguishability
// argument is about: the harness plants a control token into every one of the six scan
// targets and requires all six back. Real canaries are scanned from UNCONTAMINATED buffers
// in a separate pass, so the control can never mask or manufacture a real detection.

import { spawn, spawnSync, execFileSync } from 'node:child_process';
import {
  mkdtempSync, mkdirSync, writeFileSync, appendFileSync, readFileSync,
  readdirSync, statSync, existsSync, readlinkSync, rmSync,
} from 'node:fs';
import { tmpdir, platform as osPlatform, loadavg } from 'node:os';
import { join, basename, resolve } from 'node:path';
import { createHash } from 'node:crypto';

// ---------------------------------------------------------------------------------
// Canonical constants -- mirrored by tests/e5_soak_contract.rs
// ---------------------------------------------------------------------------------

const SESSION_TARGET = 1000;
const BLOCK_SIZE = 100;
const MIN_CONCURRENCY = 2;
const CHANNELS = ['protocol', 'stdout', 'stderr', 'files', 'logs', 'telemetry'];
const CENSUS_BACKEND = {
  linux: 'cgroup-v2',
  win32: 'windows-job-object',
  darwin: 'process-group-observed-nonauthoritative',
};
const AUTHORITATIVE = {
  'cgroup-v2': true,
  'windows-job-object': true,
  'process-group-observed-nonauthoritative': false,
};
const PANIC_RE = /panicked at|thread '.*' panicked|STATUS_ACCESS_VIOLATION|stack backtrace:/;
const FORBIDDEN_SENTINELS = ['panicked at', 'STATUS_ACCESS_VIOLATION', 'stack backtrace:'];

// Leaf verbs that only read. A soak that runs `self-update` or `plugin install` a thousand
// times is a hazard, not a measurement.
const SAFE_LEAF_VERBS = new Set([
  'list', 'status', 'orphans', 'scan', 'available', 'marketplace', 'identity',
  'attribution', 'logs', 'history', 'digest', 'validate', 'probe', 'receipt',
  'show', 'diff', 'verify', 'inspect', 'search', 'export',
]);

// A surface whose warm-up fails for one of these reasons has an UNSATISFIED PRECONDITION
// this soak deliberately does not satisfy -- it needs an argument the harness may not
// invent, or a credential the plan forbids embedding. That is not the product being
// broken, so it is classified apart from broken inventory and BOTH counts are reported.
// Recorded here rather than in bands.json because it is workload construction, not a band.
const PRECONDITION_SENTINELS = [
  'required arguments were not provided',
  'No API key found',
  'no profile specified',
  'needs at least two receipts',
  'unrecognized subcommand',
];

const CONTROL_TOKEN = 'WLSOAK-CONTROL-CANARY-0f1e2d3c4b5a69788796a5b4c3d2e1f0';
const SESSION_TIMEOUT_MS = 60_000; // the decided session_wall_ms_max floor

const IS_WINDOWS = process.platform === 'win32';

function fail(msg) {
  console.error(`f28-native-soak: ${msg}`);
  process.exit(1);
}

function sha256File(p) {
  return createHash('sha256').update(readFileSync(p)).digest('hex');
}

// ---------------------------------------------------------------------------------
// Workload construction -- surfaces read from the candidate ledger AT RUN TIME
// ---------------------------------------------------------------------------------

export function surfacesFromCandidate(candidate) {
  const raw = candidate.surfaces ?? candidate.surface_inventory ?? [];
  return raw
    .map((s) => (typeof s === 'string' ? s : s.entrypoint ?? s.surface ?? ''))
    .map((s) => String(s).replace(/^wayland-core\s*/, '').trim())
    .filter(Boolean);
}

// Tier 1 = argument-free read-only ACTIONS: real config load, real state read, real output.
// Tier 3 = every other resolved surface, exercised through `--help`, which drives argv
//          parsing and command-tree dispatch on the real binary and nothing deeper. The
//          tiers are labelled and counted separately so nobody can read a tier-3 session
//          as though it were a tier-1 one.
export function buildWorkload(surfaces) {
  const tier1 = [];
  const tier3 = [];
  for (const s of surfaces) {
    const parts = s.split(/\s+/).filter(Boolean);
    if (!parts.length) continue;
    const leaf = parts[parts.length - 1];
    if (SAFE_LEAF_VERBS.has(leaf)) tier1.push({ tier: 1, id: s, argv: parts });
    else tier3.push({ tier: 3, id: `${s} --help`, argv: [...parts, '--help'] });
  }
  return { tier1, tier3 };
}

// Round-robin so every block carries the same surface mix. Without this the late window is
// a different workload from the early window and the drift measures the mix.
export function schedule(established, tier3, protocolEvery, total) {
  const out = [];
  let i1 = 0;
  let i3 = 0;
  for (let n = 0; n < total; n += 1) {
    if (protocolEvery > 0 && n % protocolEvery === protocolEvery - 1) {
      out.push({ tier: 2, id: '--json-stream', argv: ['--json-stream'] });
    } else if (n % 3 === 2 && tier3.length) {
      out.push(tier3[i3++ % tier3.length]);
    } else if (established.length) {
      out.push(established[i1++ % established.length]);
    } else if (tier3.length) {
      out.push(tier3[i3++ % tier3.length]);
    }
  }
  return out;
}

// ---------------------------------------------------------------------------------
// Session driving
// ---------------------------------------------------------------------------------

function runSession(bin, argv, env, cwd) {
  const started = process.hrtime.bigint();
  return new Promise((resolveP) => {
    let out = Buffer.alloc(0);
    let err = Buffer.alloc(0);
    let done = false;
    const child = spawn(bin, argv, { cwd, env, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
    const timer = setTimeout(() => {
      if (!done) { try { child.kill('SIGKILL'); } catch { /* already gone */ } }
    }, SESSION_TIMEOUT_MS);
    child.stdout.on('data', (d) => { out = Buffer.concat([out, d]); });
    child.stderr.on('data', (d) => { err = Buffer.concat([err, d]); });
    child.on('error', (e) => {
      if (done) return;
      done = true; clearTimeout(timer);
      resolveP({ status: null, spawnError: String(e), stdout: '', stderr: '', ms: 0, pid: null });
    });
    // stdin closed immediately: --json-stream emits its protocol events and exits.
    try { child.stdin.end(); } catch { /* closed */ }
    child.on('close', (status, signal) => {
      if (done) return;
      done = true; clearTimeout(timer);
      const ms = Number(process.hrtime.bigint() - started) / 1e6;
      resolveP({
        status, signal,
        stdout: out.toString('utf8'),
        stderr: err.toString('utf8'),
        ms,
        pid: child.pid ?? null,
      });
    });
  });
}

// ---------------------------------------------------------------------------------
// Quality invariant -- warm-up may bind a value, never define whatever happened as correct
// ---------------------------------------------------------------------------------

export function structuralSignature(text) {
  const t = text.trim();
  if (t.startsWith('{') || t.startsWith('[')) {
    try {
      const v = JSON.parse(t);
      if (v && typeof v === 'object' && !Array.isArray(v)) {
        return `json:${Object.keys(v).sort().join(',')}`;
      }
      return `json:${Array.isArray(v) ? 'array' : typeof v}`;
    } catch { /* not JSON after all */ }
  }
  return `lines:${t.split('\n').length}`;
}

export function warmupClass(result) {
  const text = `${result.stdout}${result.stderr}`;
  if (FORBIDDEN_SENTINELS.some((s) => text.includes(s))) return 'broken';
  if (result.status === 0 && text.trim().length > 0) return 'established';
  if (PRECONDITION_SENTINELS.some((s) => text.includes(s))) return 'precondition';
  return 'broken';
}

export function matchesInvariant(result, inv) {
  const text = `${result.stdout}${result.stderr}`;
  if (FORBIDDEN_SENTINELS.some((s) => text.includes(s))) return false;
  if (result.status !== inv.status) return false;
  if (inv.nonEmpty && text.trim().length === 0) return false;
  return structuralSignature(text) === inv.signature;
}

// ---------------------------------------------------------------------------------
// Statistics -- block aggregates, per the decided bands
// ---------------------------------------------------------------------------------

export function percentile(values, p) {
  if (!values.length) return 0;
  const s = [...values].sort((a, b) => a - b);
  const rank = Math.max(1, Math.ceil((p / 100) * s.length));
  return s[rank - 1];
}

export function median(values) {
  if (!values.length) return 0;
  const s = [...values].sort((a, b) => a - b);
  const m = Math.floor(s.length / 2);
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
}

export function driftFromBlocks(blocks, earlyBlocks, lateBlocks) {
  const pick = (nums) => blocks.filter((b) => nums.includes(b.block));
  const early = pick(earlyBlocks);
  const late = pick(lateBlocks);
  const all = blocks.flatMap((b) => b.latencies);
  const correctAll = blocks.reduce((a, b) => a + b.correct, 0);
  const totalAll = blocks.reduce((a, b) => a + b.sessions, 0);
  return [
    {
      metric: 'latency_p50_block_median_ms',
      early: median(early.map((b) => percentile(b.latencies, 50))),
      late: median(late.map((b) => percentile(b.latencies, 50))),
    },
    {
      metric: 'latency_p90_block_median_ms',
      early: median(early.map((b) => percentile(b.latencies, 90))),
      late: median(late.map((b) => percentile(b.latencies, 90))),
    },
    {
      metric: 'quality_correct_rate_block_mean',
      early: early.length ? early.reduce((a, b) => a + b.correct / b.sessions, 0) / early.length : 0,
      late: late.length ? late.reduce((a, b) => a + b.correct / b.sessions, 0) / late.length : 0,
    },
    // Floors are evaluated against `late`, so run-level floors carry the run-level value in
    // both terms; the validator reads `late`.
    {
      metric: 'quality_correct_rate_run',
      early: totalAll ? correctAll / totalAll : 0,
      late: totalAll ? correctAll / totalAll : 0,
    },
    { metric: 'session_wall_ms_max', early: Math.max(0, ...all), late: Math.max(0, ...all) },
    { metric: 'session_wall_ms_p95', early: percentile(all, 95), late: percentile(all, 95) },
  ];
}

// ---------------------------------------------------------------------------------
// Canary scanning -- six channels, per-channel counts, never a boolean
// ---------------------------------------------------------------------------------

export function countOccurrences(haystack, needles) {
  let n = 0;
  for (const needle of needles) {
    if (!needle) continue;
    let idx = haystack.indexOf(needle);
    while (idx !== -1) { n += 1; idx = haystack.indexOf(needle, idx + needle.length); }
  }
  return n;
}

function walkText(dir, cap = 4 * 1024 * 1024) {
  let text = '';
  const stack = [dir];
  while (stack.length && text.length < cap) {
    const d = stack.pop();
    let entries;
    try { entries = readdirSync(d, { withFileTypes: true }); } catch { continue; }
    for (const e of entries) {
      const p = join(d, e.name);
      if (e.isDirectory()) { stack.push(p); continue; }
      if (!e.isFile()) continue;
      try {
        const st = statSync(p);
        if (st.size > 8 * 1024 * 1024) continue;
        text += readFileSync(p, 'utf8');
      } catch { /* unreadable is not a detection */ }
      if (text.length >= cap) break;
    }
  }
  return text;
}

function dirBytes(dir) {
  let total = 0;
  const stack = [dir];
  while (stack.length) {
    const d = stack.pop();
    let entries;
    try { entries = readdirSync(d, { withFileTypes: true }); } catch { continue; }
    for (const e of entries) {
      const p = join(d, e.name);
      if (e.isDirectory()) { stack.push(p); continue; }
      try { total += statSync(p).size; } catch { /* vanished */ }
    }
  }
  return total;
}

// ---------------------------------------------------------------------------------
// Process census -- reuses the ownership DISCIPLINE of process_tree.rs, caveat and all
// ---------------------------------------------------------------------------------

export function censusBackend(plat) {
  return CENSUS_BACKEND[plat] ?? 'process-group-observed-nonauthoritative';
}

function enumerateProductProcesses(binPath) {
  const target = resolve(binPath);
  const name = basename(target).replace(/\.exe$/i, '');
  const found = [];
  if (process.platform === 'linux') {
    for (const ent of readdirSync('/proc')) {
      if (!/^\d+$/.test(ent)) continue;
      try {
        const exe = readlinkSync(`/proc/${ent}/exe`);
        if (resolve(exe) === target) found.push(Number(ent));
      } catch { /* not ours or gone */ }
    }
  } else if (process.platform === 'darwin') {
    const r = spawnSync('/bin/ps', ['-Ao', 'pid=,comm='], { encoding: 'utf8' });
    for (const line of (r.stdout ?? '').split('\n')) {
      const m = line.trim().match(/^(\d+)\s+(.*)$/);
      if (m && resolve(m[2]) === target) found.push(Number(m[1]));
    }
  } else if (IS_WINDOWS) {
    // Get-Process, not WMI: Win32_Process.CommandLine reads NULL on this box and that
    // already cost this program a misdiagnosis.
    const ps = spawnSync('powershell', ['-NoProfile', '-Command',
      `Get-Process -Name '${name}' -ErrorAction SilentlyContinue | ForEach-Object { "$($_.Id)|$($_.Path)" }`,
    ], { encoding: 'utf8' });
    for (const line of (ps.stdout ?? '').split('\n')) {
      const [pid, path] = line.trim().split('|');
      if (pid && path && resolve(path).toLowerCase() === target.toLowerCase()) found.push(Number(pid));
    }
  }
  return found;
}

function activeHandles() {
  const h = typeof process._getActiveHandles === 'function' ? process._getActiveHandles().length : 0;
  const r = typeof process._getActiveRequests === 'function' ? process._getActiveRequests().length : 0;
  return h + r;
}

// ---------------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------------

async function run(opts) {
  const bin = resolve(opts.bin);
  if (!existsSync(bin)) fail(`binary not found: ${bin}`);
  const candidate = JSON.parse(readFileSync(opts.candidate, 'utf8'));
  const bands = JSON.parse(readFileSync(opts.bands, 'utf8'));
  if (bands.schema !== 'f28-soak-bands/v1') fail('bands file carries no f28-soak-bands/v1 schema tag');

  const binarySha = sha256File(bin);
  const targets = candidate.targets ?? candidate.per_target ?? candidate.binaries ?? [];
  const entry = (Array.isArray(targets) ? targets : Object.values(targets))
    .find((t) => (t.target ?? t.triple) === opts.target);
  const ledgerSha = entry ? (entry.sha256 ?? entry.digest ?? '') : '';

  const root = mkdtempSync(join(tmpdir(), 'f28-soak-'));
  const home = join(root, 'home');
  const logs = join(root, 'logs');
  const telemetry = join(root, 'telemetry');
  for (const d of [home, logs, telemetry]) mkdirSync(d, { recursive: true });

  // Real canaries: synthetic by construction. They are planted where the product will read
  // them, and must never appear in anything the product emits or writes.
  const runNonce = createHash('sha256').update(`${Date.now()}:${process.pid}`).digest('hex').slice(0, 32);
  const canaries = [
    `WLSOAK-CANARY-ENV-${runNonce}`,
    `WLSOAK-CANARY-CFG-${runNonce}`,
  ];
  writeFileSync(join(home, 'soak-canary.toml'), `# soak fixture\nsecret = "${canaries[1]}"\n`);

  const env = {
    ...process.env,
    WAYLAND_HOME: home,
    HOME: home,
    USERPROFILE: home,
    WLSOAK_RUN: runNonce,
    WLSOAK_CANARY: canaries[0],
    WAYLAND_TELEMETRY_DIR: telemetry,
    WAYLAND_LOG_DIR: logs,
    NO_COLOR: '1',
  };

  const surfaces = surfacesFromCandidate(candidate);
  if (!surfaces.length) fail('candidate ledger exposes no surfaces; the workload cannot be guessed');
  const { tier1, tier3 } = buildWorkload(surfaces);

  // ---- warm-up: one occurrence of every tier-1 surface, classified against the committed
  // sanity schema. Warm-up may bind a value; it may never define whatever happened as
  // correct.
  const invariants = new Map();
  const warmup = { established: [], precondition: [], broken: [] };
  for (const s of tier1) {
    const r = await runSession(bin, s.argv, env, root);
    const cls = warmupClass(r);
    warmup[cls].push(s.id);
    if (cls === 'established') {
      const text = `${r.stdout}${r.stderr}`;
      invariants.set(s.id, { status: r.status, nonEmpty: true, signature: structuralSignature(text) });
    }
  }
  for (const s of tier3) {
    const r = await runSession(bin, s.argv, env, root);
    const cls = warmupClass(r);
    warmup[cls].push(s.id);
    if (cls === 'established') {
      const text = `${r.stdout}${r.stderr}`;
      invariants.set(s.id, { status: r.status, nonEmpty: true, signature: structuralSignature(text) });
    }
  }
  const established = tier1.filter((s) => invariants.has(s.id));
  const established3 = tier3.filter((s) => invariants.has(s.id));

  // ---- the schedule
  const total = opts.sessions ?? SESSION_TARGET;
  const plan = schedule(established, established3, 10, total);
  const concurrency = Math.max(MIN_CONCURRENCY, opts.concurrency ?? 4);

  // ---- buffers, kept apart so a control can never mask or manufacture a real detection
  const buf = { protocol: '', stdout: '', stderr: '' };
  const BUF_CAP = 32 * 1024 * 1024;
  const blocks = [];
  const samples = [];
  const covariates = [];
  const trackedPids = new Set();
  const controlLane = join(root, 'control-growth.bin');
  writeFileSync(controlLane, Buffer.alloc(4096));

  const sampleEvery = Number((bands.sampling ?? {}).resource_interval_sessions ?? 10);
  let completed = 0;

  const sampleResources = (index) => {
    appendFileSync(controlLane, Buffer.alloc(4096)); // the deliberately growing lane
    const live = enumerateProductProcesses(bin).filter((p) => !trackedPids.has(p));
    samples.push({
      session_index: index,
      metrics: {
        state_dir_bytes: dirBytes(home),
        live_product_processes: live.length,
        harness_active_handles: activeHandles(),
        harness_rss_bytes: process.memoryUsage().rss,
        control_growth_bytes: statSync(controlLane).size,
      },
    });
    covariates.push({
      session_index: index,
      host_load_1m: IS_WINDOWS ? null : loadavg()[0],
      live_soak_children: live.length,
    });
  };

  sampleResources(0);

  for (let b = 0; b < Math.ceil(total / BLOCK_SIZE); b += 1) {
    const slice = plan.slice(b * BLOCK_SIZE, (b + 1) * BLOCK_SIZE);
    const latencies = [];
    let correct = 0;
    let ran = 0;
    for (let i = 0; i < slice.length; i += concurrency) {
      const batch = slice.slice(i, i + concurrency);
      const results = await Promise.all(batch.map((s) => runSession(bin, s.argv, env, root)));
      for (let k = 0; k < batch.length; k += 1) {
        const s = batch[k];
        const r = results[k];
        if (r.pid) trackedPids.add(r.pid);
        ran += 1;
        completed += 1;
        latencies.push(r.ms);
        const inv = invariants.get(s.id);
        // A tier-2 protocol session has no warm-up invariant of its own; its correctness is
        // that it produced protocol bytes and did not panic.
        const ok = s.tier === 2
          ? (r.stdout.trim().length > 0 && !PANIC_RE.test(`${r.stdout}${r.stderr}`))
          : (inv ? matchesInvariant(r, inv) : false);
        if (ok) correct += 1;
        if (s.tier === 2) {
          if (buf.protocol.length < BUF_CAP) buf.protocol += r.stdout;
        } else if (buf.stdout.length < BUF_CAP) buf.stdout += r.stdout;
        if (buf.stderr.length < BUF_CAP) buf.stderr += r.stderr;
        if (completed % sampleEvery === 0) sampleResources(completed);
      }
    }
    blocks.push({ block: b + 1, sessions: ran, correct, latencies });
    process.stderr.write(`F28_SOAK block=${b + 1} sessions=${ran} correct=${correct} completed=${completed}\n`);
  }

  // ---- canary scan: real counts from UNCONTAMINATED buffers, then the control pass
  const fileText = walkText(home);
  const logText = walkText(logs);
  const telText = walkText(telemetry);
  // The fixture the harness itself planted is not a product leak; it is removed from the
  // files channel by construction so the scan measures the product, not the harness.
  const fixture = `secret = "${canaries[1]}"`;
  const fileTextClean = fileText.split(fixture).join('');

  const realTargets = {
    protocol: buf.protocol,
    stdout: buf.stdout,
    stderr: buf.stderr,
    files: fileTextClean,
    logs: logText,
    telemetry: telText,
  };
  const channels = {};
  for (const c of CHANNELS) channels[c] = countOccurrences(realTargets[c], canaries);

  // Positive control: plant the control token into EVERY channel's scan target and require
  // all six back. This validates the DETECTOR, which is what "absence of a detector and
  // absence of a detection are identical" is about.
  const controlPerChannel = {};
  for (const c of CHANNELS) {
    controlPerChannel[c] = countOccurrences(`${realTargets[c]}\n${CONTROL_TOKEN}\n`, [CONTROL_TOKEN]);
  }
  const controlDetected = CHANNELS.every((c) => controlPerChannel[c] >= 1);

  // ---- orphan census, with a deliberately orphaned control process
  //
  // Spawned AFTER the final resource sample so it cannot contaminate the series, and
  // detached so it is genuinely outside the harness's ownership rather than merely
  // untracked.
  // `mcp-serve` is the control because it is a PRODUCT process that blocks on stdin and
  // needs no provider, so it is genuinely alive when the census walks. `--json-stream` was
  // tried first and exits immediately without a credential; a control that has already
  // died proves nothing about whether the census can see a live one.
  let controlPid = null;
  let controlChild = null;
  try {
    controlChild = spawn(bin, ['mcp-serve'], {
      env, cwd: root, detached: true, windowsHide: true, stdio: ['pipe', 'ignore', 'ignore'],
    });
    controlPid = controlChild.pid ?? null;
    controlChild.unref();
  } catch { /* recorded below as an unfound control, which VOIDs -- never silently clean */ }
  await new Promise((r) => setTimeout(r, 2000));

  const enumerated = enumerateProductProcesses(bin);
  const controlFound = controlPid !== null && enumerated.includes(controlPid);
  const orphans = enumerated.filter((p) => p !== controlPid && !trackedPids.has(p));
  if (controlPid !== null) {
    try { process.kill(controlPid, 'SIGKILL'); } catch { /* gone */ }
    try { controlChild.stdin.end(); } catch { /* already closed */ }
  }

  // ---- resource verdict inputs
  const growth = (metric) => {
    const pts = samples.filter((s) => metric in s.metrics)
      .map((s) => [s.session_index, s.metrics[metric]]);
    if (!pts.length) return null;
    const [fi, fv] = pts[0];
    const [li, lv] = pts[pts.length - 1];
    const span = Math.max(li - fi, 1);
    const abs = (lv - fv) * (SESSION_TARGET / span);
    return { absolute: abs, ratio: Math.abs(fv) < 1e-12 ? (Math.abs(abs) < 1e-12 ? 0 : Infinity) : 1 + abs / fv };
  };
  const controlGrowth = growth('control_growth_bytes');
  const controlGrowthFlagged = controlGrowth !== null && controlGrowth.ratio > 1.0;

  const backend = censusBackend(process.platform);
  const record = {
    family: opts.family,
    host: opts.host,
    target: opts.target,
    binary_sha256: binarySha,
    ledger_sha256: ledgerSha,
    sessions_completed: completed,
    session_target: SESSION_TARGET,
    concurrency,
    workload: {
      candidate_surfaces: surfaces.length,
      tier1_candidates: tier1.length,
      tier3_candidates: tier3.length,
      established: warmup.established.length,
      precondition_unavailable: warmup.precondition.length,
      broken_inventory: warmup.broken.length,
      broken_inventory_ids: warmup.broken,
      precondition_ids: warmup.precondition,
      established_ids: warmup.established,
    },
    blocks: blocks.map((b) => ({
      block: b.block,
      sessions: b.sessions,
      correct: b.correct,
      p50_ms: percentile(b.latencies, 50),
      p90_ms: percentile(b.latencies, 90),
      p95_ms: percentile(b.latencies, 95),
    })),
    canary: {
      channels,
      channels_scanned: [...CHANNELS],
      control_detected: controlDetected,
      control_per_channel: controlPerChannel,
      control_channel: 'all-six',
    },
    census: {
      backend,
      authoritative: AUTHORITATIVE[backend],
      orphans_found: orphans.length,
      orphan_pids: orphans,
      control_orphan_found: controlFound,
      scope: `processes whose executable path resolves to ${bin}`,
      caveat: AUTHORITATIVE[backend] ? null
        : 'the fallback observes a process group and a hostile descendant can leave one, so a zero census here is a zero OBSERVATION rather than a containment guarantee',
    },
    resources: { samples, control_growth_flagged: controlGrowthFlagged },
    covariates,
    drift: driftFromBlocks(blocks, (bands.windows ?? {}).early_blocks ?? [1, 2, 3],
      (bands.windows ?? {}).late_blocks ?? [8, 9, 10]),
    reds: [],
  };

  try { rmSync(root, { recursive: true, force: true }); } catch { /* best effort */ }
  return record;
}

// ---------------------------------------------------------------------------------
// self-test
// ---------------------------------------------------------------------------------

function selfTest() {
  const results = [];
  const check = (name, cond) => results.push([Boolean(cond), name]);

  check('six channels, exactly the receipt model', CHANNELS.length === 6
    && ['protocol', 'stdout', 'stderr', 'files', 'logs', 'telemetry'].every((c) => CHANNELS.includes(c)));
  check('session target is 1000 and is not configurable downward here', SESSION_TARGET === 1000);
  check('minimum concurrency is 2', MIN_CONCURRENCY === 2);
  check('macOS census is non-authoritative', AUTHORITATIVE[censusBackend('darwin')] === false);
  check('linux census is authoritative', AUTHORITATIVE[censusBackend('linux')] === true);
  check('windows census is authoritative', AUTHORITATIVE[censusBackend('win32')] === true);

  // workload construction
  const w = buildWorkload(['session list', 'plugin install', 'self-update', 'node identity']);
  check('read-only leaves become tier 1', w.tier1.map((s) => s.id).sort().join(',') === 'node identity,session list');
  check('install/self-update never become tier-1 actions',
    !w.tier1.some((s) => /install|self-update/.test(s.id)));
  check('every non-tier-1 surface is still exercised through --help',
    w.tier3.every((s) => s.argv[s.argv.length - 1] === '--help'));

  // schedule mix
  const sch = schedule([{ tier: 1, id: 'a', argv: ['a'] }, { tier: 1, id: 'b', argv: ['b'] }],
    [{ tier: 3, id: 'c --help', argv: ['c', '--help'] }], 10, 200);
  check('schedule fills the whole run', sch.length === 200);
  const firstBlock = sch.slice(0, 100).map((s) => s.tier).sort().join('');
  const lastBlock = sch.slice(100).map((s) => s.tier).sort().join('');
  check('early and late blocks carry the same tier mix', firstBlock === lastBlock);
  check('protocol sessions are present', sch.some((s) => s.tier === 2));

  // warm-up classification
  check('a clean warm-up establishes',
    warmupClass({ status: 0, stdout: 'ok\n', stderr: '' }) === 'established');
  check('a panic is broken inventory even at exit 0',
    warmupClass({ status: 0, stdout: 'thread \'main\' panicked at x', stderr: '' }) === 'broken');
  check('a missing-argument surface is a precondition, not breakage',
    warmupClass({ status: 2, stdout: '', stderr: 'error: the following required arguments were not provided:' }) === 'precondition');
  check('an unexplained non-zero exit is broken inventory',
    warmupClass({ status: 3, stdout: '', stderr: 'kaboom' }) === 'broken');
  check('warm-up cannot establish an invariant from empty output',
    warmupClass({ status: 0, stdout: '', stderr: '' }) === 'broken');

  // invariant matching
  const inv = { status: 0, nonEmpty: true, signature: structuralSignature('{"a":1,"b":2}') };
  check('same JSON key set matches', matchesInvariant({ status: 0, stdout: '{"b":9,"a":8}', stderr: '' }, inv));
  check('a changed JSON key set does not match',
    !matchesInvariant({ status: 0, stdout: '{"a":1}', stderr: '' }, inv));
  check('a changed exit status does not match',
    !matchesInvariant({ status: 1, stdout: '{"a":1,"b":2}', stderr: '' }, inv));
  check('a panic never matches an invariant',
    !matchesInvariant({ status: 0, stdout: '{"a":1,"b":2}\npanicked at src/x.rs', stderr: '' }, inv));

  // canary scanning
  check('the scanner counts every occurrence, not just the first',
    countOccurrences('X--X--X', ['X']) === 3);
  check('an absent canary counts zero', countOccurrences('clean output', ['X']) === 0);
  check('the control token is found when planted',
    countOccurrences(`noise\n${CONTROL_TOKEN}\n`, [CONTROL_TOKEN]) === 1);

  // statistics
  check('percentile is nearest-rank', percentile([1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 90) === 9);
  check('median of an even set averages the middle pair', median([1, 2, 3, 4]) === 2.5);
  const blocks = [];
  for (let i = 1; i <= 10; i += 1) {
    blocks.push({ block: i, sessions: 100, correct: 100, latencies: Array(100).fill(i <= 3 ? 100 : 100) });
  }
  const flat = driftFromBlocks(blocks, [1, 2, 3], [8, 9, 10]);
  check('a flat run reports zero drift',
    flat.find((m) => m.metric === 'latency_p50_block_median_ms').early
    === flat.find((m) => m.metric === 'latency_p50_block_median_ms').late);
  const degraded = blocks.map((b) => ({ ...b, latencies: Array(100).fill(b.block <= 3 ? 100 : 400) }));
  const dd = degraded.map((b) => b);
  const drifted = driftFromBlocks(dd, [1, 2, 3], [8, 9, 10]);
  check('a degrading run reports drift the band can catch',
    drifted.find((m) => m.metric === 'latency_p50_block_median_ms').late === 400);
  const halfBroken = blocks.map((b) => ({ ...b, correct: 50 }));
  check('a uniformly broken run reports a run-level rate the floor can catch',
    driftFromBlocks(halfBroken, [1, 2, 3], [8, 9, 10])
      .find((m) => m.metric === 'quality_correct_rate_run').late === 0.5);

  for (const [ok, name] of results) console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}`);
  const failed = results.filter(([ok]) => !ok).length;
  console.log(`\n${results.length} assertions, ${failed} failed`);
  return failed ? 1 : 0;
}

// ---------------------------------------------------------------------------------

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  return i === -1 ? fallback : process.argv[i + 1];
}

async function main() {
  if (process.argv.includes('--self-test')) process.exit(selfTest());
  const opts = {
    bin: arg('bin'), candidate: arg('candidate'), bands: arg('bands'),
    family: arg('family'), host: arg('host'), target: arg('target'),
    out: arg('out'), sessions: Number(arg('sessions', String(SESSION_TARGET))),
    concurrency: Number(arg('concurrency', '4')),
  };
  for (const k of ['bin', 'candidate', 'bands', 'family', 'host', 'target', 'out']) {
    if (!opts[k]) fail(`--${k} is required`);
  }
  const record = await run(opts);
  writeFileSync(opts.out, `${JSON.stringify(record, null, 2)}\n`);
  console.log(`F28_SOAK_DONE family=${record.family} sessions=${record.sessions_completed} out=${opts.out}`);
}

if (process.argv[1] && process.argv[1].endsWith('f28-native-soak.mjs')) {
  main().catch((e) => fail(String(e && e.stack ? e.stack : e)));
}
