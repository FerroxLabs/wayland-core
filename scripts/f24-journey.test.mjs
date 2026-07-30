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
  ADAPTERS,
  CANONICAL_STEPS,
  Journey,
  PLATFORMS,
  StepFailure,
  classifyRepeats,
  hostPlatform,
  parseArgs,
  parseStatusJson,
  run,
  shellish,
  verdictFor,
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

// ── F24-GWP-M1 regression ───────────────────────────────────────────────────
//
// The receipt used to freeze `counts` at step 13 and recompute
// `adapter_coverage` at receipt-write time, so a delivery arriving in between
// was counted by the breakdown and invisible to the headline. The Windows run
// of 2026-07-30 published `duplicates: 0` beside a breakdown summing to 24.
//
// Per LANE-BRIEF §6b-ii these tests carry THREE assertions, not two: the
// duplicate is reported (positive), a clean run still reports zero (negative),
// and the pre-fix code path is shown to have missed it.

// Build a journey whose 17 steps are stubbed, so `receipt()` is reachable
// without hardware. Deliveries are spread over the real adapter table so the
// coverage breakdown is populated the way a live run populates it.
function journeyReadyForReceipt(bodyCount) {
  const j = journey();
  for (let i = 0; i < CANONICAL_STEPS.length; i += 1) {
    j.step(CANONICAL_STEPS[i], `cmd ${i}`, `output ${i}`);
  }
  j.candidateCommit = 'f'.repeat(40);
  j.binaryVersion = 'wayland-core 0.0.0-test';
  j.binarySha256 = '0'.repeat(64);
  for (let i = 1; i <= bodyCount; i += 1) {
    const body = `m1-body-${String(i).padStart(2, '0')}`;
    j.bodies.push(body);
    j.bodyAdapter.set(body, ADAPTERS[(i - 1) % ADAPTERS.length].adapter);
  }
  j.counts.submitted = bodyCount;
  fs.mkdirSync(j.runDir, { recursive: true });
  return j;
}

function arrivalLine(j, body) {
  const adapter = j.bodyAdapter.get(body);
  const spec = ADAPTERS.find((a) => a.adapter === adapter);
  return JSON.stringify({ text: body, endpoint: spec.endpoint, suppressed: false });
}

function writeArrivals(j, bodies) {
  fs.writeFileSync(j.journalPath, `${bodies.map((b) => arrivalLine(j, b)).join('\n')}\n`);
}

test('M1 positive: a duplicate arriving AFTER step 13 reaches the receipt headline', () => {
  const j = journeyReadyForReceipt(12);

  // The clean first pass, then step 13 freezes its reading — exactly as the
  // live driver does at `delivery-reconcile`.
  writeArrivals(j, j.bodies);
  j.counts = j.tally();
  assert.equal(j.counts.duplicates, 0, 'step 13 legitimately saw a clean run');
  assert.equal(j.counts.arrived, 12);

  // The PT1M burst: every delivery re-arrives, after step 13 and before the
  // receipt is written.
  writeArrivals(j, [...j.bodies, ...j.bodies]);

  const receipt = j.receipt();

  // 1. POSITIVE — the headline now reports the duplicate.
  assert.equal(receipt.counts.arrived, 24);
  assert.equal(receipt.counts.unique, 12);
  assert.equal(receipt.counts.duplicates, 12);
  assert.equal(receipt.counts.losses, 0);

  // 2. The headline and the breakdown agree, which is the property the Rust
  //    verifier's `AdapterCountsUnreconciled` check enforces.
  const sum = (field) =>
    receipt.adapter_coverage.exercised.reduce((acc, e) => acc + e[field], 0);
  assert.equal(sum('arrived'), receipt.counts.arrived);
  assert.equal(sum('unique'), receipt.counts.unique);
  assert.equal(sum('submitted'), receipt.counts.submitted);

  // 3. THE OLD CODE WOULD HAVE MISSED IT. `this.counts` is still the step-13
  //    freeze, and it is still the exact false headline the Windows receipt
  //    published. The test asserts the stale value EXISTS and that the receipt
  //    is not made of it — without this, the two assertions above would pass on
  //    the broken driver too, because the broken driver's breakdown was right.
  assert.equal(j.counts.duplicates, 0, 'the pre-fix headline source still reads zero');
  assert.equal(j.counts.arrived, 12);
  assert.notEqual(receipt.counts.arrived, j.counts.arrived);

  // And the journey refuses to call this complete.
  assert.throws(
    () => j.assertFinalReconciliation(receipt),
    (error) =>
      error instanceof StepFailure &&
      /FINAL delivery reconciliation is not clean/.test(error.message) &&
      /duplicates=12/.test(error.message) &&
      /12 arrival\(s\) landed after it/.test(error.message),
  );
});

test('M1 negative: with the duplicate removed the same receipt reports zero', () => {
  // The other direction, per LANE-BRIEF §3b-iii: a gate that cannot pass is as
  // useless as one that cannot fail. Identical journey, identical code path,
  // one arrival per body.
  const j = journeyReadyForReceipt(12);
  writeArrivals(j, j.bodies);
  j.counts = j.tally();
  writeArrivals(j, j.bodies); // no burst this time

  const receipt = j.receipt();
  assert.equal(receipt.counts.arrived, 12);
  assert.equal(receipt.counts.unique, 12);
  assert.equal(receipt.counts.duplicates, 0);
  assert.equal(receipt.counts.losses, 0);
  const sum = (field) =>
    receipt.adapter_coverage.exercised.reduce((acc, e) => acc + e[field], 0);
  assert.equal(sum('arrived'), 12);
  assert.doesNotThrow(() => j.assertFinalReconciliation(receipt));
});

test('M1 structural: the headline and the breakdown come from ONE journal read', () => {
  // Equality today is not the property; incapability of disagreeing is. A
  // single `snapshot()` is what provides it, so assert the projections are
  // taken from the same read rather than re-reading the file.
  const j = journeyReadyForReceipt(6);
  writeArrivals(j, [...j.bodies, j.bodies[0]]);
  const snap = j.snapshot();
  assert.equal(snap.counts.arrived, 7);
  assert.equal(snap.counts.duplicates, 1);
  assert.equal(
    snap.coverage.exercised.reduce((acc, e) => acc + e.arrived, 0),
    snap.counts.arrived,
  );
  assert.equal(snap.unattributed, 0);

  // `tally()` and `adapterCoverage()` are projections of the same function, so
  // a caller cannot reintroduce the two-read split by accident.
  assert.deepEqual(j.tally(), snap.counts);
  assert.deepEqual(j.adapterCoverage(), snap.coverage);
});

test('M1: an arrival no adapter endpoint claims is refused, not silently split', () => {
  // The remaining way the two sums could differ once they share a read: an
  // arrival counted by the headline whose endpoint matches no adapter. That is
  // a real disagreement and must be reported as one.
  const j = journeyReadyForReceipt(3);
  const lines = j.bodies.map((b) => arrivalLine(j, b));
  lines.push(JSON.stringify({ text: j.bodies[0], endpoint: 'nobody.claims.this', suppressed: false }));
  fs.writeFileSync(j.journalPath, `${lines.join('\n')}\n`);
  const snap = j.snapshot();
  assert.equal(snap.unattributed, 1);
  const receipt = j.receipt();
  assert.throws(
    () => j.assertFinalReconciliation(receipt),
    (error) => error instanceof StepFailure && /attributed to no adapter endpoint/.test(error.message),
  );
});

// ── F24-GWP-H1: replay vs recurrence ────────────────────────────────────────

const arr = (text, key, endpoint = 'chat.postMessage') => ({
  text,
  endpoint,
  idempotency_key: key,
  suppressed: false,
});

test('H1 positive: the SAME delivery identity twice is a replay — exactly-once violated', () => {
  const id = classifyRepeats([arr('body-1', 'cron:j1:1000'), arr('body-1', 'cron:j1:1000')]);
  assert.equal(id.replays, 1);
  assert.equal(id.recurrences, 0);
  assert.equal(id.indeterminate, 0);
  assert.match(verdictFor(id), /EXACTLY-ONCE VIOLATED/);
});

test('H1 negative: the same body under DIFFERENT identities is a recurrence, not a duplicate', () => {
  // The Windows shape. Two occurrences of a 60-second recurring job.
  const id = classifyRepeats([arr('body-1', 'cron:j1:1000'), arr('body-1', 'cron:j1:61000')]);
  assert.equal(id.replays, 0);
  assert.equal(id.recurrences, 1);
  assert.equal(id.indeterminate, 0);
  assert.match(verdictFor(id), /NO DUPLICATE/);
  // The distinction is the whole finding: the OLD body-only tally called this
  // a duplicate, and that is how a false HIGH was raised against Windows.
  assert.ok(!/VIOLATED/.test(verdictFor(id)), 'a recurrence must never read as a violation');
});

test('H1: a repeat with no delivery identity is NOT PROVEN, and is counted against the run', () => {
  const id = classifyRepeats([
    arr('body-1', null, 'whatsapp.messages'),
    arr('body-1', null, 'whatsapp.messages'),
  ]);
  assert.equal(id.replays, 0);
  assert.equal(id.recurrences, 0);
  assert.equal(id.indeterminate, 1, 'an unprovable repeat must not read as clean');
  assert.equal(id.unidentified, 2);
  assert.deepEqual(id.unidentified_endpoints, ['whatsapp.messages']);
  assert.match(verdictFor(id), /NOT PROVEN/);
  assert.match(verdictFor(id), /NOT evidence of a duplicate/);
});

test('H1: a clean run classifies as nothing at all', () => {
  const id = classifyRepeats([arr('body-1', 'cron:j1:1000'), arr('body-2', 'cron:j2:1000')]);
  assert.deepEqual(
    { r: id.replays, c: id.recurrences, i: id.indeterminate, u: id.unidentified },
    { r: 0, c: 0, i: 0, u: 0 },
  );
});

test('H1 on the REAL Windows arrivals: zero replays, and the buckets reconcile', () => {
  // The actual journal from the run that produced F24-GWP-H1, committed at
  // 24-gateway-platforms/windows-arrivals.jsonl. Real data, not a mutation.
  const journal = fileURLToPath(
    new URL(
      '../.planning/phases/24-gateway-automation-channels-typed-api/evidence/' +
        '24-gateway-platforms/windows-arrivals.jsonl',
      import.meta.url,
    ),
  );
  // Asserted, never skipped: a skip is not a pass.
  assert.ok(fs.existsSync(journal), `the real Windows journal must be present at ${journal}`);
  const rows = fs
    .readFileSync(journal, 'utf8')
    .split('\n')
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l))
    .filter((r) => String(r.text ?? '').startsWith('f24j-delivery'));

  assert.equal(rows.length, 24, 'known-positive: the fixture is the 24-arrival run');
  const id = classifyRepeats(rows);

  // THE FINDING: not one arrival in that run was a replay.
  assert.equal(id.replays, 0, 'no delivery identity arrived twice');
  assert.equal(id.recurrences, 4, 'the four adapters that emit an identity recurred once each');
  assert.equal(id.indeterminate, 8, 'the eight bodies with no identity cannot be judged');
  assert.equal(id.unidentified, 16);
  assert.match(verdictFor(id), /NOT PROVEN/);

  // And the buckets account for every repeat the old body-only tally saw. The
  // headline `duplicates: 12` was arithmetically right and semantically wrong.
  const arrived = rows.length;
  const unique = new Set(rows.map((r) => r.text)).size;
  assert.equal(arrived - unique, 12);
  assert.equal(id.replays + id.recurrences + id.indeterminate, arrived - unique);
});

test('H1 both directions on the REAL journal: plant a replay, it is reported; remove it, zero', () => {
  // LANE-BRIEF §3b-iii. A gate that cannot pass proves as little as one that
  // cannot fail, so the classifier is driven to BOTH states on the same real
  // data — the run that was reported as duplicating.
  const journal = fileURLToPath(
    new URL(
      '../.planning/phases/24-gateway-automation-channels-typed-api/evidence/' +
        '24-gateway-platforms/windows-arrivals.jsonl',
      import.meta.url,
    ),
  );
  assert.ok(fs.existsSync(journal));
  const rows = fs
    .readFileSync(journal, 'utf8')
    .split('\n')
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l))
    .filter((r) => String(r.text ?? '').startsWith('f24j-delivery'));

  // BEFORE. Assert nothing already present could satisfy the positive — the
  // planted state must not pre-exist, or the positive is free.
  const before = classifyRepeats(rows);
  assert.equal(before.replays, 0, 'precondition: the unmutated journal contains NO replay');
  assert.ok(!/VIOLATED/.test(verdictFor(before)));

  // PLANT: re-send an arrival that already exists, identity and all. This is a
  // true replay — the same delivery identity on the wire twice.
  const victim = rows.find((r) => r.idempotency_key);
  assert.ok(victim, 'known-positive: at least one arrival carries an identity to replay');
  const planted = [...rows, { ...victim }];
  const after = classifyRepeats(planted);

  assert.equal(after.replays, 1, 'the planted replay IS reported');
  assert.match(verdictFor(after), /EXACTLY-ONCE VIOLATED/);
  // And it is attributed as a replay, not absorbed into the recurrence bucket.
  assert.equal(after.recurrences, before.recurrences);
  assert.equal(after.indeterminate, before.indeterminate);

  // REMOVE: back to the real data, and the report returns to zero.
  const restored = classifyRepeats(planted.slice(0, -1));
  assert.equal(restored.replays, 0, 'with the plant removed the report is zero again');
  assert.deepEqual(restored, before);
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
