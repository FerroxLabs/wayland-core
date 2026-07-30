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
  VERDICT,
  classifyRepeats,
  hostPlatform,
  parseArgs,
  parseStatusJson,
  run,
  shellish,
  verdictFor,
} from './f24-journey.mjs';
import {
  buildQuadrants,
  fixturePath,
  serialise,
  verdictPath,
} from './f24-journey-quadrants.mjs';

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

test('--adapters defaults to all of them and narrowing must be typed out', () => {
  // CAN IT PASS / CAN IT FAIL, on the one knob that could be used to buy a
  // greener verdict with less coverage.
  const base = ['--platform', 'linux', '--run-dir', '/tmp/x', '--binary', '/bin/true'];

  // Omitted: the full table. This is the direction that matters — a default
  // that quietly narrowed to the keyed adapters would trade adapter coverage,
  // a separate criterion, for an easier exactly-once verdict.
  assert.deepEqual(parseArgs(base).adapters, ADAPTERS);
  assert.equal(ADAPTERS.length, 3);

  // Named: exactly those, in table order, with the table's endpoint bindings.
  const one = parseArgs([...base, '--adapters', 'slack']).adapters;
  assert.deepEqual(one.map((a) => a.adapter), ['slack']);
  assert.equal(one[0].endpoint, 'chat.postMessage');
  const two = parseArgs([...base, '--adapters', 'sms, slack']).adapters;
  assert.deepEqual(two.map((a) => a.adapter), ['slack', 'sms'], 'table order, not argument order');

  // An unknown name is refused rather than silently dropped — dropping it would
  // narrow the run further than the operator asked and still look successful.
  assert.throws(
    () => parseArgs([...base, '--adapters', 'slack,telegram']),
    (error) => /does not configure/.test(error.message) && /Known: slack,whatsapp,sms/.test(error.message),
  );
  assert.throws(
    () => parseArgs([...base, '--adapters', ' , ']),
    (error) => /no names/.test(error.message),
  );
});

test('a narrowed run carries its narrowness into the receipt', () => {
  // The safeguard that makes the knob honest: whatever it is set to lands in
  // `adapter_coverage`, so `verify --min-adapters N` can still refuse it and a
  // reader of the success line sees `adapters=1/10`.
  const j = new Journey({
    platform: hostPlatform() in PLATFORMS ? hostPlatform() : 'linux',
    runDir: tmpdir(),
    binary: process.execPath,
    adapters: ADAPTERS.filter((a) => a.adapter === 'slack'),
  });
  for (const name of CANONICAL_STEPS) j.step(name, `cmd ${name}`, `out ${name}`);
  j.candidateCommit = 'f'.repeat(40);
  j.binaryVersion = 'x';
  j.binarySha256 = '0'.repeat(64);
  for (let i = 1; i <= 3; i += 1) {
    const body = `narrow-${i}`;
    j.bodies.push(body);
    j.bodyAdapter.set(body, 'slack');
  }
  j.counts.submitted = 3;
  fs.mkdirSync(j.runDir, { recursive: true });
  fs.writeFileSync(
    j.journalPath,
    `${j.bodies
      .map((b) =>
        JSON.stringify({ text: b, endpoint: 'chat.postMessage', idempotency_key: `cron:${b}:1`, suppressed: false }),
      )
      .join('\n')}\n`,
  );
  const receipt = j.receipt();
  assert.deepEqual(
    receipt.adapter_coverage.exercised.map((e) => e.adapter),
    ['slack'],
    'the receipt must NAME the one adapter, so nobody can read the run as three',
  );
  assert.equal(receipt.adapter_coverage.registered_total, 10);
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

  // And the journey refuses to call this complete. `writeArrivals` emits no
  // idempotency key, so every repeat here is UNJUDGEABLE — the refusal is
  // NOT-PROVEN, not a claim that a duplicate was observed.
  assert.throws(
    () => j.assertFinalReconciliation(receipt),
    (error) =>
      error instanceof StepFailure &&
      /verdict=NOT-PROVEN/.test(error.message) &&
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
  assert.match(verdictFor(id), /verdict=EXACTLY-ONCE-VIOLATED/);
});

test('H1 negative: the same body under DIFFERENT identities is a recurrence, not a duplicate', () => {
  // The Windows shape. Two occurrences of a 60-second recurring job.
  const id = classifyRepeats([arr('body-1', 'cron:j1:1000'), arr('body-1', 'cron:j1:61000')]);
  assert.equal(id.replays, 0);
  assert.equal(id.recurrences, 1);
  assert.equal(id.indeterminate, 0);
  assert.match(verdictFor(id), /verdict=RECURRENCE/);
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
  assert.match(verdictFor(id), /verdict=NOT-PROVEN/);
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
  assert.match(verdictFor(id), /verdict=NOT-PROVEN/);

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
  assert.match(verdictFor(after), /verdict=EXACTLY-ONCE-VIOLATED/);
  // And it is attributed as a replay, not absorbed into the recurrence bucket.
  assert.equal(after.recurrences, before.recurrences);
  assert.equal(after.indeterminate, before.indeterminate);

  // REMOVE: back to the real data, and the report returns to zero.
  const restored = classifyRepeats(planted.slice(0, -1));
  assert.equal(restored.replays, 0, 'with the plant removed the report is zero again');
  assert.deepEqual(restored, before);
});

// ── the four quadrants, driver side ─────────────────────────────────────────
//
// LANE-BRIEF §3b-iii: a gate must be driven in BOTH directions. Until this
// change the driver and the Rust verifier both refused any `duplicates != 0`,
// and on Windows a kill-and-recover leg ALWAYS crosses a 60 s trigger period
// (Task Scheduler's minimum repetition is `PT1M`), so the Windows journey had no
// achievable pass state whatsoever.
//
// The receipts come from `f24-journey-quadrants.mjs`, which builds them through
// the driver's own `receipt()` from synthetic arrival journals. q1, q2 and q3
// carry a BYTE-IDENTICAL headline, so nothing but the identity block can tell
// them apart — which is the whole reason that block had to become verified data
// rather than a decoration.

test('the four quadrants: the driver passes recurrences and refuses everything else', () => {
  const quadrants = buildQuadrants();
  assert.equal(quadrants.length, 4, 'known-positive: all four quadrants were built');

  const byName = new Map(quadrants.map((q) => [q.name, q]));
  const expect = (name, verdict, clean) => {
    const q = byName.get(name);
    assert.ok(q, `${name} must be present`);
    assert.equal(q.outcome.verdict, verdict, `${name}: ${q.outcome.reason}`);
    assert.equal(q.outcome.clean, clean, `${name}: ${q.outcome.reason}`);
    return q;
  };

  // Q1 — CAN IT PASS? This is the state that did not exist before.
  const q1 = expect('q1-recurrence-passes', VERDICT.RECURRENCE, true);
  assert.equal(q1.receipt.counts.duplicates, 12);
  assert.equal(q1.receipt.delivery_identity.replays, 0);
  assert.equal(q1.receipt.delivery_identity.recurrences, 12);

  // Q2 — CAN IT FAIL? Same headline as q1 to the byte.
  const q2 = expect('q2-replay-fails', VERDICT.EXACTLY_ONCE_VIOLATED, false);
  assert.deepEqual(q2.receipt.counts, q1.receipt.counts,
    'q1 and q2 must be indistinguishable by headline, or q2 could fail on the counts alone');
  assert.equal(q2.receipt.delivery_identity.replays, 1);

  // Q3 — an unprovable repeat is not a clean one.
  const q3 = expect('q3-indeterminate-fails', VERDICT.NOT_PROVEN, false);
  assert.deepEqual(q3.receipt.counts, q1.receipt.counts);
  assert.equal(q3.receipt.delivery_identity.indeterminate, 8);
  assert.equal(q3.receipt.delivery_identity.unidentified, 16);

  // Q4 — the gate must still grade a quiet run.
  const q4 = expect('q4-clean-passes', VERDICT.NO_REPEATS, true);
  assert.equal(q4.receipt.counts.duplicates, 0);
});

test('q3 reproduces the REAL Windows run, arrival for arrival', () => {
  // A synthetic fixture is only worth anything if it is faithful. These are the
  // numbers measured at the sink on 2026-07-30 and asserted against the
  // committed journal earlier in this file: 24 arrivals, 12 repeats, 4 of them
  // classifiable and 8 not, 16 arrivals carrying no key.
  const journal = fileURLToPath(
    new URL(
      '../.planning/phases/24-gateway-automation-channels-typed-api/evidence/' +
        '24-gateway-platforms/windows-arrivals.jsonl',
      import.meta.url,
    ),
  );
  assert.ok(fs.existsSync(journal), 'the real Windows journal must be present');
  const real = classifyRepeats(
    fs
      .readFileSync(journal, 'utf8')
      .split('\n')
      .filter((l) => l.trim())
      .map((l) => JSON.parse(l))
      .filter((r) => String(r.text ?? '').startsWith('f24j-delivery')),
  );
  const synthetic = buildQuadrants().find((q) => q.name === 'q3-indeterminate-fails')
    .receipt.delivery_identity;

  assert.deepEqual(
    {
      replays: synthetic.replays,
      recurrences: synthetic.recurrences,
      indeterminate: synthetic.indeterminate,
      unidentified: synthetic.unidentified,
    },
    {
      replays: real.replays,
      recurrences: real.recurrences,
      indeterminate: real.indeterminate,
      unidentified: real.unidentified,
    },
    'the q3 fixture must classify identically to the journal it stands in for',
  );
  assert.deepEqual(synthetic.unidentified_endpoints, real.unidentified_endpoints);
});

test('the committed quadrant fixtures still match what the driver produces', () => {
  // The drift guard. The Rust verifier grades committed BYTES, so a driver
  // change that alters a receipt must move those bytes with it — otherwise the
  // Rust side goes on grading a receipt the driver no longer emits, and the
  // cross-gate agreement test becomes a comparison against history.
  let checked = 0;
  for (const q of buildQuadrants()) {
    const receiptFile = fixturePath(q.name);
    const verdictFile = verdictPath(q.name);
    assert.ok(fs.existsSync(receiptFile), `${receiptFile} must be committed`);
    assert.ok(fs.existsSync(verdictFile), `${verdictFile} must be committed`);
    assert.equal(
      fs.readFileSync(receiptFile, 'utf8'),
      serialise(q.receipt),
      `${q.name}: committed receipt drifted — regenerate with ` +
        '`node scripts/f24-journey-quadrants.mjs --write`',
    );
    assert.equal(
      fs.readFileSync(verdictFile, 'utf8'),
      `${q.driverVerdict}\n`,
      `${q.name}: committed driver verdict drifted`,
    );
    checked += 1;
  }
  assert.equal(checked, 4, 'a loop that silently shortened would report a pass over nothing');
});

test('the driver verdict sidecar carries exactly one known token', () => {
  // The Rust cross-gate test extracts `verdict=<TOKEN>` from this file. An
  // extractor is only sound if there is exactly one token to find — zero would
  // make the comparison fall through to whatever matched next, and two would
  // make it arbitrary. That is the self-passing shape wearing a regex.
  const tokens = Object.values(VERDICT);
  for (const q of buildQuadrants()) {
    const found = tokens.filter((t) => q.driverVerdict.includes(`verdict=${t}`));
    assert.deepEqual(found, [q.expected], `${q.name}: ${q.driverVerdict}`);
  }
});

test('a passing journey still states its verdict — silence on green teaches the wrong lesson', () => {
  // `assertFinalReconciliation` returns its report on the clean path instead of
  // returning bare. A gate that is silent when it passes and eloquent when it
  // fails trains a reader to read `duplicates=12` as bad news unconditionally,
  // which is precisely the misreading that produced the F24-GWP-H1 finding.
  const q1 = buildQuadrants().find((q) => q.name === 'q1-recurrence-passes');
  assert.match(q1.driverVerdict, /verdict=RECURRENCE/);
  assert.match(q1.driverVerdict, /duplicates=12/);
  assert.ok(
    !/The receipt was written and records the true/.test(q1.driverVerdict),
    'the closing sentence belongs to a REFUSAL; a pass must not carry it',
  );
});

test('step 13 waits on losses, never on duplicates', () => {
  // The loop used to be `while (losses > 0 || duplicates > 0)`. `duplicates` is
  // `arrived - unique` over an APPEND-ONLY journal, so it is monotonically
  // non-decreasing: once one repeat landed the loop could not exit except by
  // timeout, and it then held the gateway alive for the full 180 s budget —
  // three more 60 s trigger periods — manufacturing the very repeats it was
  // waiting to see disappear.
  //
  // Asserted on the SOURCE rather than by timing the loop, because a timing
  // assertion would take three minutes to fail and would be flaky under load.
  //
  // Per LANE-BRIEF §6b-ii this self-test carries THREE assertions, not two:
  // the matcher finds the real loop (known-positive), the loop no longer polls
  // `duplicates` (the repair), AND the matcher would have CAUGHT the old code
  // (without which the whole test would pass just as happily on the bug).
  const condition = (source) => {
    const loop = source.match(/while \(Date\.now\(\) < deadline([\s\S]*?)\) \{/);
    return loop ? loop[1] : null;
  };
  // Scoped to `deliveryReconcile()`. The driver has OTHER deadline loops — the
  // recovery poll among them — and a whole-file search finds whichever comes
  // first in the file, which is not the one under test. That is the wrong-tree
  // search §3b-i warns about: it returns a confident answer about code nobody
  // asked after.
  const source = fs.readFileSync(DRIVER, 'utf8');
  const body = source.slice(source.indexOf('deliveryReconcile() {'));
  assert.ok(body.startsWith('deliveryReconcile() {'), 'known-positive: the step-13 method exists');

  const live = condition(body);
  assert.ok(live !== null, 'known-positive: the reconcile wait loop is present in the driver');
  assert.match(live, /losses > 0/, 'it must wait on the count that can actually fall');
  assert.ok(
    !/duplicates > 0/.test(live),
    `the wait loop must not poll a monotone non-decreasing count: ${live}`,
  );

  // The third assertion. A matcher that stopped at the first `)` returned
  // `while (Date.now() < deadline)` for BOTH the old and the new source, so it
  // reported the repair as present before the repair existed.
  const old = 'while (Date.now() < deadline && (t.losses > 0 || t.duplicates > 0)) {';
  const before = condition(old);
  assert.ok(before !== null, 'the matcher must also find the pre-fix loop');
  assert.match(before, /duplicates > 0/, 'and it must SEE the defect there, or it proves nothing');
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
