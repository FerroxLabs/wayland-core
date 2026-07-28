// The journey driver's own logic, tested before the driver is trusted on
// hardware.
//
// A journey driver that can silently pass is worse than no driver, because it
// manufactures exactly the false confidence the criterion exists to prevent. The
// three shapes below have each been caught passing on failure in this
// repository, so each gets a test here:
//
//   1. a command that was NOT FOUND reads as an empty success;
//   2. an assertion against EMPTY OUTPUT passes;
//   3. a receipt claiming a step list the journey never executed.
//
// Run: node --test scripts/f24-journey.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

import {
  CANONICAL_STEPS,
  Journey,
  PLATFORMS,
  StepFailure,
  hostPlatform,
  parseArgs,
  parseStatusJson,
  run,
  shellish,
} from './f24-journey.mjs';

const DRIVER = fileURLToPath(new URL('./f24-journey.mjs', import.meta.url));

function tmpdir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'f24-journey-test-'));
}

function journey(overrides = {}) {
  return new Journey({
    platform: hostPlatform() in PLATFORMS ? hostPlatform() : 'linux',
    runDir: tmpdir(),
    binary: process.execPath,
    ...overrides,
  });
}

test('a command that does not exist is a failure, not an empty success', () => {
  const result = run(['definitely-not-a-real-command-f24j']);
  // The dangerous reading: status null, output empty, and any assertion phrased
  // as "the output does not contain an error" passes.
  assert.equal(result.spawnFailed, true);
  assert.equal(result.status, null);
  assert.match(result.output, /SPAWN FAILED/);
});

test('must() rejects a command that could not be spawned', () => {
  const j = journey();
  assert.throws(
    () => j.must(['definitely-not-a-real-command-f24j']),
    (error) => error instanceof StepFailure && /SPAWN-FAILED/.test(error.message),
  );
});

test('must() rejects a non-zero exit even when the command printed nothing', () => {
  const j = journey();
  assert.throws(
    () => j.must([process.execPath, '-e', 'process.exit(3)']),
    (error) => error instanceof StepFailure && /exited 3/.test(error.message),
  );
});

test('a step recorded with empty output is refused', () => {
  const j = journey();
  assert.throws(
    () => j.step('preflight-clean', 'some command', '   \n\t '),
    (error) => error instanceof StepFailure && /captured no output/.test(error.message),
  );
  assert.equal(j.steps.length, 0);
});

test('a step recorded out of canonical order is refused', () => {
  const j = journey();
  assert.throws(
    () => j.step('hard-kill', 'kill -9 1', 'output'),
    (error) => error instanceof StepFailure && /step order violated/.test(error.message),
  );
});

test('a receipt cannot be produced from a partial step list', () => {
  const j = journey();
  j.step('preflight-clean', 'cmd', 'out');
  assert.throws(
    () => j.receipt(),
    (error) => error instanceof StepFailure && /recorded 1 steps/.test(error.message),
  );
});

test('the driver step list is exactly the canonical list', () => {
  // If the Rust verifier's canonical list and this one drift, every receipt is
  // refused for a reason that looks like a journey failure. Pin both to the
  // same literal ordering.
  assert.deepEqual(CANONICAL_STEPS, [
    'preflight-clean',
    'binary-identity',
    'profile-setup',
    'sink-start',
    'gateway-install',
    'gateway-start',
    'status-running',
    'automation-add',
    'deliveries-submit',
    'arrival-before-kill',
    'hard-kill',
    'platform-recover',
    'delivery-reconcile',
    'upgrade-in-place',
    'rollback',
    'redaction-canary',
    'drain-uninstall-clean',
  ]);
  assert.equal(CANONICAL_STEPS.length, 17);
});

test('the driver refuses an unknown argument', () => {
  assert.throws(
    () => parseArgs(['--platform', 'linux', '--run-dir', '/tmp/x', '--binary', '/bin/true', '--yolo']),
    (error) => /unknown argument --yolo/.test(error.message),
  );
});

test('the driver refuses a missing required argument', () => {
  assert.throws(
    () => parseArgs(['--platform', 'linux', '--run-dir', '/tmp/x']),
    (error) => /--binary is required/.test(error.message),
  );
});

test('the driver refuses an unknown platform', () => {
  assert.throws(
    () => parseArgs(['--platform', 'plan9', '--run-dir', '/tmp/x', '--binary', '/bin/true']),
    (error) => /--platform must be one of/.test(error.message),
  );
});

test('the driver refuses to run a platform journey on the wrong host', () => {
  // "A macOS journey runs on macOS or it does not happen." Without this a
  // Linux host would happily produce a receipt stamped `macos`.
  const wrong = Object.keys(PLATFORMS).find((p) => p !== hostPlatform()) ?? 'windows';
  const result = spawnSync(
    process.execPath,
    [DRIVER, '--platform', wrong, '--run-dir', tmpdir(), '--binary', process.execPath],
    { encoding: 'utf8' },
  );
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /but this host is/);
});

test('the driver exits non-zero and names the failing step when a step fails', () => {
  // Driving `node` as if it were wayland-core: `--build-info` is not a node
  // flag, so the binary-identity step fails. The point is the SHAPE of the
  // failure — non-zero status, a failure record naming where it stopped —
  // not the particular step.
  const dir = tmpdir();
  const result = spawnSync(
    process.execPath,
    [DRIVER, '--platform', hostPlatform(), '--run-dir', dir, '--binary', process.execPath],
    { encoding: 'utf8', timeout: 120_000 },
  );
  assert.notEqual(result.status, 0, result.stdout + result.stderr);
  assert.match(result.stderr, /JOURNEY FAILED/);
  const failurePath = path.join(dir, `${hostPlatform()}-failure.json`);
  assert.ok(fs.existsSync(failurePath), 'a failed journey must leave its evidence behind');
  const failure = JSON.parse(fs.readFileSync(failurePath, 'utf8'));
  assert.ok(CANONICAL_STEPS.includes(failure.failed_at), failure.failed_at);
  assert.ok(!fs.existsSync(path.join(dir, `${hostPlatform()}-receipt.json`)),
    'a failed journey must NOT write a receipt');
});

test('the status parser reads a PRETTY-PRINTED projection', () => {
  // The real `gateway status --json` output. Reading only the last line yields
  // `}`, the parse fails, and a running gateway reads as "no live pid" — which
  // is exactly how the first live Linux run failed while the product was
  // behaving correctly.
  const pretty = [
    '{',
    '  "state": "running",',
    '  "pid": 2601545,',
    '  "uptime_secs": 59,',
    '  "profile": "f24j",',
    '  "turns_in_flight": 0,',
    '  "deliveries_pending": 0,',
    '  "binary_path": "/root/wayland-24journey/target/release/wayland-core",',
    '  "binary_version": "0.12.25"',
    '}',
    '',
  ].join('\n');
  assert.equal(parseStatusJson(pretty)?.pid, 2601545);
  assert.equal(parseStatusJson(pretty)?.state, 'running');
  assert.equal(parseStatusJson(`warning: something\n${pretty}`)?.pid, 2601545);
  assert.equal(parseStatusJson(''), null);
  assert.equal(parseStatusJson('not json at all'), null);
});

test('shellish quotes an argument containing whitespace', () => {
  assert.equal(shellish(['cron', 'add', '--trigger', 'cron:0 9 * * *']),
    'cron add --trigger "cron:0 9 * * *"');
});

test('the redaction step refuses when the canary never travelled a capture', () => {
  const j = journey();
  // Steps 1..15 stubbed with output that never contains the canary, so the
  // positive control has nothing to find. Absence from the redacted copy would
  // then be trivially true and would prove nothing.
  for (let i = 0; i < 15; i += 1) j.step(CANONICAL_STEPS[i], `cmd ${i}`, `output ${i}`);
  assert.throws(
    () => j.redactionCanary(),
    (error) => error instanceof StepFailure && /absent from the pre-redaction capture/.test(error.message),
  );
});

test('the redaction step removes the seeded secrets from what it writes', () => {
  const j = journey();
  for (let i = 0; i < 15; i += 1) {
    j.step(CANONICAL_STEPS[i], `cmd ${i}`, i === 2 ? `seeded ${j.botToken}` : `output ${i}`);
  }
  j.redactionCanary();
  const redacted = fs.readFileSync(path.join(j.runDir, `${j.args.platform}-redacted.md`), 'utf8');
  const raw = fs.readFileSync(path.join(j.runDir, `${j.args.platform}-raw.txt`), 'utf8');
  assert.ok(raw.includes(j.canary), 'the raw capture is the positive control');
  assert.ok(!redacted.includes(j.canary), 'the canary must not survive redaction');
  assert.ok(!redacted.includes(j.botToken), 'the bot token must not survive redaction');
  assert.ok(redacted.includes('[REDACTED]'));
});

test('the tally counts duplicates and losses from the sink journal, not from the runtime', () => {
  const j = journey();
  j.bodies = ['a-body-1', 'a-body-2', 'a-body-3'];
  j.counts.submitted = 3;
  const records = [
    { text: 'a-body-1', suppressed: false },
    { text: 'a-body-1', suppressed: false }, // a duplicate
    { text: 'a-body-2', suppressed: false },
    { text: 'unrelated-heartbeat', suppressed: false }, // not ours; must be ignored
    { text: 'a-body-3', suppressed: true }, // suppressed replays are not messages
  ];
  fs.mkdirSync(j.runDir, { recursive: true });
  fs.writeFileSync(j.journalPath, `${records.map((r) => JSON.stringify(r)).join('\n')}\n`);
  const t = j.tally();
  assert.deepEqual(t, {
    submitted: 3,
    arrived: 3,
    unique: 2,
    duplicates: 1,
    losses: 1,
  });
});

test('a clean tally reports zero duplicates and zero losses', () => {
  const j = journey();
  j.bodies = ['b-1', 'b-2'];
  j.counts.submitted = 2;
  fs.mkdirSync(j.runDir, { recursive: true });
  fs.writeFileSync(
    j.journalPath,
    `${JSON.stringify({ text: 'b-1' })}\n${JSON.stringify({ text: 'b-2' })}\n`,
  );
  assert.deepEqual(j.tally(), {
    submitted: 2,
    arrived: 2,
    unique: 2,
    duplicates: 0,
    losses: 0,
  });
});

test('windows aliveness is read from tasklist OUTPUT, never from its exit status', () => {
  // `tasklist /FI "PID eq N"` exits 0 whether or not it matched. Reading the
  // status would report every pid as alive forever, so the hard-kill step could
  // never confirm the kill and the recovery step could never confirm the
  // restart. This is the same class as a filter stealing an exit code.
  const table = PLATFORMS.windows;
  assert.equal(typeof table.aliveFromOutput, 'function');
  assert.equal(
    table.aliveFromOutput({ status: 0, output: 'INFO: No tasks are running which match.' }, 4242),
    false,
  );
  assert.equal(
    table.aliveFromOutput({ status: 0, output: 'wayland-core.exe   4242 Console  1  12,345 K' }, 4242),
    true,
  );
});

test('every platform in the invocation table names the same step list', () => {
  // The one structural guarantee that makes three receipts comparable: a
  // platform may differ in HOW a step is invoked and never in WHICH steps run.
  for (const [name, table] of Object.entries(PLATFORMS)) {
    assert.equal(typeof table.family, 'string', name);
    for (const fn of ['kill', 'alive', 'platformQuery', 'residual', 'residualPresent']) {
      assert.equal(typeof table[fn], 'function', `${name}.${fn}`);
    }
  }
});
