#!/usr/bin/env node
// Lane A instrument — sample the SHIPPED health surface of a running gateway.
//
// Two independent instruments, deliberately, because they have different
// blind spots and the lane's claim depends on knowing which one saw what:
//
//   1. PRODUCT SAMPLER. Runs the real `wayland-core channel health --json`
//      verb in a child process on a fixed cadence. This is the surface an
//      operator actually reads, and it is what the brief's "healthy 2/45"
//      figure is denominated in. Its cost (~0.2s per spawn for a debug binary)
//      bounds the cadence.
//
//   2. TRANSIENT WATCHER. Polls `channel-health.json` directly at 50 ms and
//      records every CHANGE of state. The product sampler at a 2 s cadence
//      cannot see a flap shorter than 2 s; this one can see any flap the file
//      itself distinguishes.
//
//      The watcher's resolution is bounded by the PUBLISHER, not by the poll:
//      `gateway run` republishes the health file once per TICK_MS (1 s,
//      wcore-cli/src/gateway.rs:92). A Healthy state that begins and ends
//      inside one publish interval is invisible to BOTH instruments, and this
//      script reports that limit rather than implying a resolution it does not
//      have. That is why the lane also carries an in-process event-stream test
//      whose resolution is 10 ms.
//
// Exit status is deliberately 0 for any completed run: this script MEASURES,
// the caller GRADES. A script that decided the verdict itself would make the
// negative and positive controls impossible to run with the same command.

import { spawnSync } from 'node:child_process';
import fs from 'node:fs';

const args = Object.fromEntries(
  process.argv.slice(2).map((a) => {
    const i = a.indexOf('=');
    return i === -1 ? [a.replace(/^--/, ''), 'true'] : [a.slice(2, i), a.slice(i + 1)];
  }),
);

const BIN = args.bin;
const HOME_DIR = args.home;
const SAMPLES = Number(args.samples || 46);
const CADENCE_MS = Number(args['cadence-ms'] || 2000);
const OUT = args.out || 'sampler.json';
const LABEL = args.label || 'unlabelled';
const WATCH_MS = 50;

if (!BIN || !HOME_DIR) {
  console.error('usage: --bin=<wayland-core> --home=<WAYLAND_HOME> [--samples=N] [--cadence-ms=N]');
  process.exit(2);
}

const healthFile = `${HOME_DIR}/channel-health.json`;
const t0 = Date.now();

// ---- instrument 2: transient watcher -------------------------------------
const transitions = [];
let lastSeen = null;
const watcher = setInterval(() => {
  let raw;
  try {
    raw = fs.readFileSync(healthFile, 'utf8');
  } catch {
    return;
  }
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return; // torn read of a rename-in-flight; the next poll gets it
  }
  const sig = JSON.stringify(
    (parsed.channels || []).map((c) => [c.channel, c.state, c.reason ?? null]),
  );
  if (sig !== lastSeen) {
    lastSeen = sig;
    transitions.push({ ms: Date.now() - t0, states: JSON.parse(sig) });
  }
}, WATCH_MS);

// ---- instrument 1: product sampler ---------------------------------------
const samples = [];
function sampleOnce(i) {
  const started = Date.now();
  const r = spawnSync(BIN, ['channel', 'health', '--json'], {
    encoding: 'utf8',
    env: { ...process.env, WAYLAND_HOME: HOME_DIR },
    timeout: 20000,
  });
  let states = null;
  let parseError = null;
  try {
    const j = JSON.parse(r.stdout);
    states = (j.channels || []).map((c) => ({ name: c.channel, state: c.state }));
  } catch (e) {
    parseError = `${String(e).slice(0, 120)} | stdout=${(r.stdout || '').slice(0, 200)} | stderr=${(r.stderr || '').slice(0, 200)}`;
  }
  samples.push({
    i,
    ms: started - t0,
    took_ms: Date.now() - started,
    rc: r.status,
    states,
    parseError,
  });
}

async function main() {
  for (let i = 0; i < SAMPLES; i++) {
    sampleOnce(i);
    const target = t0 + (i + 1) * CADENCE_MS;
    const wait = target - Date.now();
    if (wait > 0) await new Promise((res) => setTimeout(res, wait));
  }
  clearInterval(watcher);

  // A sample counts as "healthy" only when EVERY channel it reported is
  // Healthy AND at least one channel was reported. A report with zero
  // channels is neither healthy nor degraded -- it is the false-zero shape
  // this repo has measured three times, and it is counted separately so it
  // can never be silently scored as a pass in either direction.
  let healthy = 0,
    degraded = 0,
    empty = 0,
    unreadable = 0;
  for (const s of samples) {
    if (s.states === null) unreadable++;
    else if (s.states.length === 0) empty++;
    else if (s.states.every((c) => c.state === 'healthy')) healthy++;
    else degraded++;
  }

  const summary = {
    label: LABEL,
    binary: BIN,
    home: HOME_DIR,
    samples: samples.length,
    cadence_ms: CADENCE_MS,
    window_ms: Date.now() - t0,
    counts: { healthy, degraded, empty, unreadable },
    publisher_resolution_ms: 1000,
    watcher_poll_ms: WATCH_MS,
    transitions,
    raw_samples: samples,
  };
  fs.writeFileSync(OUT, JSON.stringify(summary, null, 2));
  console.log(
    `[${LABEL}] healthy=${healthy}/${samples.length} degraded=${degraded} empty=${empty} unreadable=${unreadable} window=${summary.window_ms}ms transitions=${transitions.length}`,
  );
  for (const tr of transitions) {
    console.log(`  t+${tr.ms}ms ${JSON.stringify(tr.states)}`);
  }
}

main();
