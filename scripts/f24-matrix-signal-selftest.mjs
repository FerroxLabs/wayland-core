#!/usr/bin/env node
// Self-test for the 24-MATRIX-SIGNAL instruments.
//
// LANE-BRIEF §6b-ii: when you find a defect in your own instrument you repair it
// in the same lane, and the repaired instrument gets a self-test with THREE
// assertions, not two — known-positive passes, known-negative fails, AND the old
// broken instrument would have missed it. That third assertion is the only one
// that proves the repair does anything; without it the self-test passes on the
// broken instrument too.
//
// This lane adds three instruments, and each carries all three assertions:
//
//   1. the matrix homeserver fixture         (real process, real HTTP)
//   2. the fake signal-cli                   (real process, spawned the way the
//                                             product spawns it)
//   3. the `steady` leg + the restart grader (pure predicates, imported from the
//                                             driver so the tested code IS the
//                                             code that runs)
//
// It also asserts three PRODUCT CONTRACTS the run depends on, read from source.
// If someone fixes `sync.rs`'s process-local cursor, assertion R3 reddens and
// says so — rather than the restart probe quietly going green for a reason the
// report would have attributed to something else.
//
// Runs on macOS and Linux. Needs no cargo, no binary, no credential.

import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  gradeSteady,
  gradeRestart,
  naiveGradeRestart,
  servedAfterRestartFrom,
  legacyServedInInitialOnly,
  pidIsLive,
  LEGS,
  ADAPTERS,
  TRANSPORT,
} from './f24-inbound.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, '..');

let passed = 0;
let failed = 0;

/// Assert, and HARD-FAIL anything that returns a thenable.
///
/// Inherited from `f24-c3-clauses-selftest.mjs` and kept deliberately: an async
/// assertion REJECTS rather than throws, so a sibling self-test in this phase
/// scored a knowingly-false assertion as a pass and exited 0. A test harness
/// that cannot fail is the same defect class as a gate that cannot fail.
function test(name, fn) {
  let result;
  try {
    result = fn();
  } catch (e) {
    failed += 1;
    process.stdout.write(`FAIL ${name}\n  ${e.message}\n`);
    return;
  }
  if (result && typeof result.then === 'function') {
    failed += 1;
    process.stdout.write(
      `FAIL ${name}\n  test returned a thenable; an async assertion rejects instead of throwing ` +
        `and would be scored a pass\n`,
    );
    return;
  }
  passed += 1;
  process.stdout.write(`PASS ${name}\n`);
}

function assert(cond, msg) {
  if (!cond) throw new Error(msg);
}

function eq(got, want, msg) {
  const g = JSON.stringify(got);
  const w = JSON.stringify(want);
  if (g !== w) throw new Error(`${msg}: got ${g}, want ${w}`);
}

function sleep(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

/// Blocking HTTP via a child process — this file's own event loop is parked by
/// the blocking sleeps above, exactly as in the driver.
function http(url, opts) {
  const script = `
    fetch(${JSON.stringify(url)}, ${JSON.stringify(opts ?? {})})
      .then(async (r) => process.stdout.write(JSON.stringify({ status: r.status, body: await r.text() })))
      .catch((e) => process.stdout.write(JSON.stringify({ status: 0, body: String(e.message) })));
  `;
  const r = spawnSync(process.execPath, ['-e', script], { encoding: 'utf8', timeout: 30_000 });
  try {
    return JSON.parse(r.stdout);
  } catch {
    return { status: 0, body: r.stdout + r.stderr };
  }
}

function getJson(url) {
  const r = http(url);
  try {
    return { status: r.status, json: JSON.parse(r.body) };
  } catch {
    return { status: r.status, json: null, raw: r.body };
  }
}

function postJson(url, obj) {
  const r = http(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(obj),
  });
  try {
    return { status: r.status, json: JSON.parse(r.body) };
  } catch {
    return { status: r.status, json: null, raw: r.body };
  }
}

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'f24-ms-selftest-'));
const children = [];
function cleanup() {
  for (const c of children) {
    try {
      c.kill('SIGKILL');
    } catch {
      /* gone */
    }
  }
}
process.on('exit', cleanup);

// ═══════════════════════════════════════════════════════════════════════════
// 1. THE MATRIX HOMESERVER FIXTURE — real process, real HTTP
// ═══════════════════════════════════════════════════════════════════════════

const MX_TOKEN = 'syt_selftest_token';
const ROOM1 = '!r1:f24.invalid';
const ROOM2 = '!r2:f24.invalid';
const mxJournal = path.join(tmp, 'matrix.jsonl');
const mxLog = path.join(tmp, 'matrix.log');

let mxUrl = null;
{
  fs.writeFileSync(mxLog, '');
  const fd = fs.openSync(mxLog, 'a');
  const child = spawn(
    process.execPath,
    [
      path.join(HERE, 'f24-matrix-fixture.mjs'),
      '--journal', mxJournal,
      '--token', MX_TOKEN,
      '--room', `${ROOM1}:2`,
      '--room', `${ROOM2}:2`,
      '--max-wait-ms', '500',
    ],
    { stdio: ['ignore', fd, fd] },
  );
  children.push(child);
  for (let i = 0; i < 100; i += 1) {
    const banner = fs.readFileSync(mxLog, 'utf8');
    const m = /MXFIX_READY url=(\S+)/.exec(banner);
    if (m) {
      mxUrl = m[1];
      break;
    }
    sleep(100);
  }
}

const AUTH = { headers: { authorization: `Bearer ${MX_TOKEN}` } };
const sync = (since) =>
  getJson(
    `${mxUrl}/_matrix/client/v3/sync?timeout=500${since ? `&since=${encodeURIComponent(since)}` : ''}`,
  );
const syncAuthed = (since) => {
  const url = `${mxUrl}/_matrix/client/v3/sync?timeout=500${since ? `&since=${encodeURIComponent(since)}` : ''}`;
  const r = http(url, AUTH);
  try {
    return { status: r.status, json: JSON.parse(r.body) };
  } catch {
    return { status: r.status, json: null, raw: r.body };
  }
};

test('M0 the matrix fixture started and published a url', () => {
  assert(mxUrl, `fixture never signalled ready; log:\n${fs.readFileSync(mxLog, 'utf8')}`);
});

test('M1 KNOWN-NEGATIVE: a wrong access token is refused with 401', () => {
  // The instrument must be able to refuse. A fixture that answered everything
  // 200 would make a misconfigured run read as silence rather than as auth.
  const r = sync(null);
  eq(r.status, 401, 'unauthenticated /sync status');
  eq(r.json.errcode, 'M_UNKNOWN_TOKEN', 'errcode');
});

test('M2 KNOWN-POSITIVE: an initial sync (no since) returns a next_batch and the room summary', () => {
  const r = syncAuthed(null);
  eq(r.status, 200, 'initial sync status');
  assert(typeof r.json.next_batch === 'string' && r.json.next_batch.length > 0, 'next_batch');
  // LOAD-BEARING. sync.rs:328-331 types a room Direct ONLY on
  // m.joined_member_count == 2 and types an omitted summary Group. Every channel
  // config in the matrix sets group = "disabled", so a fixture that omitted this
  // would have every message dropped by group policy and the whole run would
  // read as product inbound loss caused entirely by the fixture.
  eq(r.json.rooms.join[ROOM1].summary['m.joined_member_count'], 2, 'room1 member count');
  eq(r.json.rooms.join[ROOM2].summary['m.joined_member_count'], 2, 'room2 member count');
});

test('M3 KNOWN-POSITIVE: an incremental sync returns ONLY events after its cursor', () => {
  const head = syncAuthed(null).json.next_batch;
  postJson(`${mxUrl}/__control/submit`, {
    room: ROOM1,
    sender: '@a:f24.invalid',
    text: 'first',
    eventId: '$e1',
  });
  postJson(`${mxUrl}/__control/submit`, {
    room: ROOM1,
    sender: '@a:f24.invalid',
    text: 'second',
    eventId: '$e2',
  });
  const r = syncAuthed(head);
  const ids = r.json.rooms.join[ROOM1].timeline.events.map((e) => e.event_id);
  eq(ids, ['$e1', '$e2'], 'events after the cursor');
  // And the summary survives on an incremental sync too — sync.rs re-reads it
  // per response, so dropping it only on incrementals would silently reclassify
  // the room as Group partway through a run.
  eq(r.json.rooms.join[ROOM1].summary['m.joined_member_count'], 2, 'incremental summary');
});

test('M4 KNOWN-NEGATIVE: an incremental sync at the head returns NOTHING', () => {
  // A fixture that always returned something would make every arrival count a
  // tautology. This is the assertion that proves it can be silent.
  const head = syncAuthed(null).json.next_batch;
  const r = syncAuthed(head);
  eq(r.json.rooms.join[ROOM1].timeline.events.length, 0, 'events at head');
  eq(r.json.rooms.join[ROOM2].timeline.events.length, 0, 'room2 events at head');
});

test('M5 THE H2 EXCLUSION IS ALIVE: an initial sync REPLAYS prior events and the report says so', () => {
  // This is the instrument the restart probe's honesty depends on. It must
  // (a) actually put earlier events in a fresh initial sync's timeline — which
  // is what a real homeserver does and what makes gap loss measurable at all —
  // and (b) report exactly which ids it served, from its own process.
  postJson(`${mxUrl}/__control/submit`, {
    room: ROOM1,
    sender: '@a:f24.invalid',
    text: 'gap',
    eventId: '$gap',
  });
  const before = getJson(`${mxUrl}/__control/report`).json.initial_sync_total;
  const r = syncAuthed(null);
  const ids = r.json.rooms.join[ROOM1].timeline.events.map((e) => e.event_id);
  assert(ids.includes('$gap'), `initial sync timeline must carry $gap, got ${JSON.stringify(ids)}`);
  const rep = getJson(`${mxUrl}/__control/report`).json;
  eq(rep.initial_sync_total, before + 1, 'initial sync counted');
  const latest = rep.initial_syncs[rep.initial_syncs.length - 1];
  assert(
    latest.served.includes('$gap'),
    `the fixture's own report must list $gap as served, got ${JSON.stringify(latest.served)}`,
  );
});

test('M6 KNOWN-NEGATIVE: an event injected into an undeclared room is REFUSED, not invented', () => {
  // A silently-invented room would carry no summary, sync.rs:328 would type it
  // Group, group = "disabled" would drop it, and the leg would report product
  // inbound loss caused by a typo in the driver.
  const r = postJson(`${mxUrl}/__control/submit`, {
    room: '!nope:f24.invalid',
    sender: '@a:f24.invalid',
    text: 'x',
  });
  eq(r.status, 400, 'undeclared room status');
  eq(r.json.ok, false, 'undeclared room ok');
});

test('M7 the reply path journals what the product PUT, keyed by room', () => {
  const room = encodeURIComponent(ROOM2);
  const r = http(`${mxUrl}/_matrix/client/v3/rooms/${room}/send/m.room.message/txn123`, {
    method: 'PUT',
    headers: { authorization: `Bearer ${MX_TOKEN}`, 'content-type': 'application/json' },
    body: JSON.stringify({ msgtype: 'm.text', body: 'a reply' }),
  });
  eq(r.status, 200, 'send status');
  assert(JSON.parse(r.body).event_id, 'send must return an event_id');
  const rep = getJson(`${mxUrl}/__control/report`).json;
  const reply = rep.replies[rep.replies.length - 1];
  eq(reply.room, ROOM2, 'reply room');
  eq(reply.text, 'a reply', 'reply text');
});

test('M8 the room id survives argument parsing (a Matrix room id contains colons)', () => {
  // `--room !r1:f24.invalid:2` splits on the LAST colon. A naive `split(':')`
  // would yield room `!r1` and every event would land in a room the adapter
  // never reports — a zero that looks exactly like product loss.
  const rep = getJson(`${mxUrl}/__control/report`).json;
  eq(rep.rooms.map((r) => r.id).sort(), [ROOM1, ROOM2].sort(), 'declared room ids');
});

// ═══════════════════════════════════════════════════════════════════════════
// 2. THE FAKE signal-cli — spawned exactly the way the product spawns it
// ═══════════════════════════════════════════════════════════════════════════

const sigJournal = path.join(tmp, 'signal.jsonl');
const sigControl = path.join(tmp, 'signal.port');
const sigCli = path.join(tmp, 'signal-cli');

fs.copyFileSync(path.join(HERE, 'f24-signal-fixture.mjs'), sigCli);
fs.chmodSync(sigCli, 0o755);

// SECOND INSTRUMENT DEFECT FOUND BY THIS FILE, recorded rather than quietly
// fixed (LANE-BRIEF §6b-ii).
//
// The first draft captured the fixture's stdout with `child.stdout.on('data')`
// and then waited with the `Atomics.wait` sleep above. That sleep BLOCKS THE
// EVENT LOOP, so the `data` handler never ran and every stdout assertion
// reported "no frame appeared on stdout" — i.e. the instrument reported the
// fixture emitting nothing while the fixture was emitting correctly. That is
// the eleven-times-recorded class again: an under-detecting instrument failing
// in the direction that blames the thing under test.
//
// The repair is structural, not a bigger timeout: every stdio interaction with
// the fixture happens inside ONE top-level-await block that yields to the event
// loop, and the observations are frozen into plain data. The synchronous
// `test()` calls below then assert on that data — which keeps the
// hard-fail-on-thenable guard intact, because no test function becomes async.
const delay = (ms) => new Promise((r) => setTimeout(r, ms));

/** @type {import('node:child_process').ChildProcess|null} */
let sigChild = null;
const sigStdout = [];
{
  // `subprocess.rs:54-62`: Command::new(cli_path).arg("-a").arg(account)
  // .arg("jsonRpc"), stdin/stdout/stderr all piped. Reproduced exactly, because
  // a fixture proven under a different invocation proves nothing about the one
  // the product performs.
  sigChild = spawn(sigCli, ['-a', '+15550000000', 'jsonRpc'], {
    stdio: ['pipe', 'pipe', 'pipe'],
    env: { ...process.env, F24_SIGNAL_JOURNAL: sigJournal, F24_SIGNAL_CONTROL: sigControl },
  });
  children.push(sigChild);
  let buf = '';
  sigChild.stdout.setEncoding('utf8');
  sigChild.stdout.on('data', (c) => {
    buf += c;
    let i;
    while ((i = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, i);
      buf = buf.slice(i + 1);
      if (line.trim()) sigStdout.push(line);
    }
  });
  sigChild.stderr.resume();
}

async function sigPort(budget = 60) {
  for (let i = 0; i < budget; i += 1) {
    if (fs.existsSync(sigControl)) {
      const [p] = fs.readFileSync(sigControl, 'utf8').trim().split(/\s+/);
      if (Number(p) > 0) return Number(p);
    }
    await delay(200);
  }
  return null;
}

async function sigCommand(obj) {
  const port = await sigPort();
  if (!port) return null;
  return new Promise((resolve) => {
    const s = net.connect(port, '127.0.0.1', () => s.write(`${JSON.stringify(obj)}\n`));
    let buf = '';
    s.setEncoding('utf8');
    s.on('data', (c) => {
      buf += c;
      if (buf.includes('\n')) {
        s.end();
        try {
          resolve(JSON.parse(buf.trim()));
        } catch {
          resolve({ ok: false, raw: buf });
        }
      }
    });
    s.on('error', (e) => resolve({ ok: false, error: e.message }));
  });
}

/// Wait until the fixture has emitted at least `n` stdout frames, yielding to
/// the event loop so the `data` handler can actually run.
async function awaitFrames(n, budgetMs = 10_000) {
  const deadline = Date.now() + budgetMs;
  while (sigStdout.length < n && Date.now() < deadline) await delay(50);
  return sigStdout.length;
}

function sigJournalRecords() {
  if (!fs.existsSync(sigJournal)) return [];
  return fs
    .readFileSync(sigJournal, 'utf8')
    .split('\n')
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l));
}

// ── ALL signal stdio interaction happens here, once, with the event loop free.
// The observations are frozen into `S` and every assertion below is synchronous.
const S = {};
{
  S.port = await sigPort();

  S.submit1 = await sigCommand({
    op: 'submit',
    account: '+15550000000',
    source: '+15551112222',
    sourceName: 'alice',
    text: 'hello f24c3-signal-selftest-aaaa1111',
    timestamp: 1_700_000_001_000,
  });
  await awaitFrames(1);
  S.receiveFrame = sigStdout.length > 0 ? sigStdout[sigStdout.length - 1] : null;

  // Two submissions under ONE timestamp — the replay shape the dedupe leg needs.
  S.replayTs = 1_700_000_009_000;
  await sigCommand({ op: 'submit', source: '+15551112222', text: 'replay-a', timestamp: S.replayTs });
  await sigCommand({ op: 'submit', source: '+15551112222', text: 'replay-b', timestamp: S.replayTs });
  await awaitFrames(3);

  const beforeSend = sigStdout.length;
  sigChild.stdin.write(
    `${JSON.stringify({
      jsonrpc: '2.0',
      id: 77,
      method: 'send',
      params: { recipient: ['+15551112222'], message: 'the reply' },
    })}\n`,
  );
  await awaitFrames(beforeSend + 1);
  S.sendResponse = sigStdout.length > beforeSend ? sigStdout[sigStdout.length - 1] : null;

  sigChild.stdin.write('this is not json\n');
  await delay(500);
  S.healthAfterGarbage = await sigCommand({ op: 'health' });

  S.stdout = [...sigStdout];
  S.journal = sigJournalRecords();
}

test('S0 the product-shaped spawn works and the argv is journalled', () => {
  assert(
    S.port,
    `the fixture never published a control port; journal:\n${fs.existsSync(sigJournal) ? fs.readFileSync(sigJournal, 'utf8') : 'ABSENT'}`,
  );
  const spawnRec = S.journal.find((r) => r.kind === 'spawn');
  assert(spawnRec, 'no spawn record');
  // The exact argv signal-cli receives in production. Asserted rather than
  // assumed: if `RealLauncher` ever changes shape, this reddens.
  eq(spawnRec.argv, ['-a', '+15550000000', 'jsonRpc'], 'spawn argv');
});

test('S1 KNOWN-POSITIVE: a submitted message becomes a well-formed `receive` frame on stdout', () => {
  assert(S.submit1 && S.submit1.ok, `submit failed: ${JSON.stringify(S.submit1)}`);
  assert(S.receiveFrame, 'no frame appeared on stdout');
  const frame = JSON.parse(S.receiveFrame);
  eq(frame.method, 'receive', 'method');
  eq(frame.params.envelope.source, '+15551112222', 'source');
  eq(frame.params.envelope.timestamp, 1_700_000_001_000, 'envelope timestamp');
  eq(frame.params.envelope.dataMessage.message, 'hello f24c3-signal-selftest-aaaa1111', 'body');
  // sourceUuid must be ABSENT: subprocess.rs:292-297 prefers it for sender_id
  // while :281-287 prefers source for conversation_id, so including it would
  // desynchronise the two and signal would stop being peer-keyed like the other
  // three adapters it is compared against.
  assert(
    frame.params.envelope.sourceUuid === undefined,
    'sourceUuid must be absent so sender_id and conversation_id resolve to the same string',
  );
});

test('S2 the dedupe leg is possible: the SAME timestamp yields the SAME product message id', () => {
  // subprocess.rs:277 — `let id = format!("{ts_ms}")`. The inbound dedupe cache
  // keys on that id, so a replay must reuse the timestamp. A fixture that
  // stamped Date.now() itself would make every replay a fresh message and the
  // dedupe leg would be measuring nothing at all.
  const emitted = S.journal.filter((r) => r.kind === 'receive.emitted' && r.timestamp === S.replayTs);
  eq(emitted.length, 2, 'two emissions under one timestamp');
  eq(
    emitted.map((e) => String(e.timestamp)),
    [String(S.replayTs), String(S.replayTs)],
    'the product message id (format!("{ts_ms}")) is identical for both',
  );
});

test('S3 KNOWN-POSITIVE: a `send` request is journalled and answered SUCCESS', () => {
  assert(S.sendResponse, 'the fixture never answered the send request');
  const frame = JSON.parse(S.sendResponse);
  eq(frame.id, 77, 'response id');
  // jsonrpc.rs:191 `classify_delivery` reads results[].type; anything but
  // SUCCESS makes the product treat the reply as undelivered.
  eq(frame.result.results[0].type, 'SUCCESS', 'delivery type');
  const sent = S.journal.filter((r) => r.kind === 'send');
  assert(sent.length >= 1, 'the send was not journalled');
  eq(sent[sent.length - 1].recipient, '+15551112222', 'journalled recipient');
  eq(sent[sent.length - 1].message, 'the reply', 'journalled message');
});

test('S4 KNOWN-NEGATIVE: stdout carries ONLY JSON frames', () => {
  // subprocess.rs:158 parses every stdout line as JSON and warns on anything
  // else. The ready banner therefore goes to stderr, not here. Every line must
  // decode — and the guard below stops this assertion being vacuous on a run
  // where the fixture emitted nothing at all, which is how it passed silently
  // in the first draft.
  assert(S.stdout.length >= 4, `expected at least 4 frames, got ${S.stdout.length} — assertion would be vacuous`);
  for (const line of S.stdout) {
    try {
      JSON.parse(line);
    } catch {
      throw new Error(`non-JSON line on stdout: ${JSON.stringify(line)}`);
    }
  }
});

test('S5 KNOWN-NEGATIVE: a malformed stdin line is journalled and does not kill the fixture', () => {
  const bad = S.journal.filter((r) => r.kind === 'stdin.malformed');
  assert(bad.length >= 1, 'malformed line was not journalled');
  assert(S.healthAfterGarbage && S.healthAfterGarbage.ok, 'fixture died on a malformed line');
});

// ═══════════════════════════════════════════════════════════════════════════
// 3. THE STEADY LEG AND THE RESTART GRADER — with the mandatory third assertion
// ═══════════════════════════════════════════════════════════════════════════

test('T0 the steady leg exists in the shared leg set and every adapter has a transport', () => {
  assert(LEGS.includes('steady'), 'steady must be one of the legs');
  for (const a of ADAPTERS) {
    assert(TRANSPORT[a], `adapter ${a} has no declared transport`);
  }
  assert(ADAPTERS.includes('matrix') && ADAPTERS.includes('signal'), 'both new adapters registered');
  eq(TRANSPORT.signal, 'subprocess', 'signal transport must not be mislabelled as poll');
});

test('T1 KNOWN-POSITIVE: every steady message arriving is a PASS', () => {
  eq(gradeSteady([1, 1, 1], 3).ok, true, 'all arrived');
});

test('T2 KNOWN-NEGATIVE: one silently swallowed steady message is a FAIL', () => {
  eq(gradeSteady([1, 0, 1], 3).ok, false, 'one lost');
  eq(gradeSteady([1, 0, 1], 3).arrived, 2, 'arrived count reported');
});

test('T3 KNOWN-NEGATIVE: universal denial cannot manufacture a steady green', () => {
  // The brief's central trap: this criterion's access leg once passed on all
  // three adapters BECAUSE EVERYTHING WAS DENIED. A leg whose pass condition
  // demands arrivals cannot be satisfied that way.
  eq(gradeSteady([0, 0, 0], 3).ok, false, 'total denial must FAIL, not pass');
});

test('T4 THIRD ASSERTION — the five ORIGINAL legs all pass on an adapter that goes deaf after the burst', () => {
  // This is the assertion that proves the steady leg does something. Without
  // it, the two above pass on a driver that never added the leg at all.
  //
  // The scenario is F24-C3-H4's exact shape: the startup burst is delivered
  // perfectly and then the adapter silently stops receiving. Below are the five
  // original legs' pass conditions, transcribed from `runMatrix`, evaluated
  // against that scenario.
  const obs = {
    seen1: [{ text: 'hello f24c3-x-admit-tag', conversation_id: 'C1' }], // admit arrived
    routedOk: true,
    convOk: true,
    beforeDedupe: 1,
    afterDedupe: 1,
    beforeTurns: 1,
    afterTurns: 1,
    dedupeControl: [{}], // fresh id still got through during the burst
    seen3: [], // denied sender correctly produced nothing
    turns3: [],
    seen4: [{ conversation_id: 'C2' }], // bind's second conversation arrived
  };
  const admit = obs.seen1.length === 1;
  const route = obs.routedOk && obs.convOk;
  const dedupe =
    obs.afterDedupe === obs.beforeDedupe &&
    obs.afterTurns === obs.beforeTurns &&
    obs.dedupeControl.length === 1;
  const access = obs.seen3.length === 0 && obs.turns3.length === 0 && obs.seen1.length === 1;
  const bind =
    obs.seen1.length === 1 &&
    obs.seen4.length === 1 &&
    obs.seen4[0].conversation_id !== obs.seen1[0].conversation_id;

  eq([admit, route, dedupe, access, bind], [true, true, true, true, true],
    'all five ORIGINAL legs pass on an adapter that has gone deaf');
  // And the new leg catches it.
  eq(gradeSteady([0, 0, 0], 3).ok, false, 'the steady leg must catch what the five miss');
});

test('T5 the five transcribed conditions still match the driver source (drift guard)', () => {
  // T4's transcription is only meaningful while it matches the code. These are
  // the literal condition fragments from `runMatrix`; if any is reworded the
  // transcription is stale and T4 stops proving anything.
  const src = fs.readFileSync(path.join(HERE, 'f24-inbound.mjs'), 'utf8');
  const fragments = [
    'seen1.length === 1',
    'routed && convOk',
    'afterDedupe === beforeDedupe && afterTurns === beforeTurns && control.length === 1',
    'seen3.length === 0 && turns3.length === 0 && accessControlHeld',
    "seen4[0].conversation_id !== seen1[0].conversation_id",
  ];
  for (const f of fragments) {
    assert(src.includes(f), `driver no longer contains the transcribed condition: ${f}`);
  }
});

test('R1 KNOWN-POSITIVE: gap message arrived with controls held is a PASS', () => {
  eq(
    gradeRestart({ preArrivals: 1, postArrivals: 1, servedAfterRestart: true, gapArrivals: 1 }).state,
    'PASS',
    'gap survived',
  );
});

test('R2 KNOWN-NEGATIVE: gap lost with every control held is LOSS', () => {
  const v = gradeRestart({ preArrivals: 1, postArrivals: 1, servedAfterRestart: true, gapArrivals: 0 });
  eq(v.state, 'LOSS', 'attributable loss');
  eq(v.graded, true, 'graded');
  eq(v.ok, false, 'not ok');
});

test('R3 THIRD ASSERTION — the NAIVE grader reports LOSS where this one reports INCOMPLETE', () => {
  // The instrument fault this probe is built to avoid: the fixture never served
  // the gap event in the post-restart initial sync, so there was nothing to
  // lose. A probe without the H2 exclusion calls that a product defect.
  const obs = { preArrivals: 1, postArrivals: 1, servedAfterRestart: false, gapArrivals: 0 };
  eq(gradeRestart(obs).state, 'INCOMPLETE', 'this grader refuses to attribute it');
  eq(gradeRestart(obs).graded, false, 'and does not grade it');
  // THE ASSERTION THAT PROVES THE REPAIR DOES SOMETHING:
  eq(naiveGradeRestart(obs).state, 'LOSS', 'the old grader would have fabricated a HIGH here');
});

// ── FIFTH INSTRUMENT DEFECT (found in lane 24-h6) ──────────────────────────
//
// The H2 exclusion demanded the gap event appear in a post-restart INITIAL
// sync. That is only where it lands while the product is BROKEN: an adapter
// that persists its cursor resumes with an INCREMENTAL sync after a restart and
// never issues an initial one. So the control was false on every correct run,
// `gradeRestart` returned INCOMPLETE, and the probe COULD NOT EXPRESS A PASS —
// the mirror image of the self-passing gate this same probe was repaired for
// once already. It could report the defect but not the fix.
//
// Repaired to "the fixture served the gap on SOME sync after the restart",
// which is what H2 always meant. Three assertions, per LANE-BRIEF §6b-ii.

const SYNCS_FIXED = [
  { sync: 10, initial: true, since: null, served: ['$pre'] }, // before the restart
  { sync: 11, initial: false, since: 's7', served: ['$gap'] }, // resumed: asked for the window
];
const SYNCS_BROKEN = [
  { sync: 10, initial: true, since: null, served: ['$pre'] },
  { sync: 11, initial: true, since: null, served: ['$gap'] }, // re-seeded: offered, then discarded
];

test('R5 KNOWN-POSITIVE: the gap served on a resumed INCREMENTAL sync is excluded-and-graded', () => {
  const probe = servedAfterRestartFrom(SYNCS_FIXED, 10, '$gap');
  eq(probe.served, true, 'the fixture did serve the gap event after the restart');
  eq(probe.where, 'incremental', 'and it did so on a resumed sync — the mechanism of the fix');
  eq(
    gradeRestart({ preArrivals: 1, postArrivals: 1, servedAfterRestart: probe.served, gapArrivals: 1 })
      .state,
    'PASS',
    'a fixed adapter can now reach PASS',
  );
});

test('R6 KNOWN-NEGATIVE: a gap the fixture never served is still INCOMPLETE, never LOSS', () => {
  const probe = servedAfterRestartFrom(
    [{ sync: 11, initial: false, since: 's7', served: [] }],
    10,
    '$gap',
  );
  eq(probe.served, false, 'the fixture served nothing — a harness fault, not product loss');
  eq(probe.where, null, 'and there is no kind of sync to name');
  const v = gradeRestart({
    preArrivals: 1,
    postArrivals: 1,
    servedAfterRestart: probe.served,
    gapArrivals: 0,
  });
  eq(v.state, 'INCOMPLETE', 'the widened control has NOT weakened the H2 exclusion');
  eq(v.graded, false, 'and still refuses to attribute it');
});

test('R7 THIRD ASSERTION — the OLD initial-only extraction misses the fixed product entirely', () => {
  // The repair must demonstrably change an outcome, or the self-test passes on
  // the broken instrument too.
  const repaired = servedAfterRestartFrom(SYNCS_FIXED, 10, '$gap');
  const legacy = legacyServedInInitialOnly(
    SYNCS_FIXED.filter((s) => s.initial),
    1,
    '$gap',
  );
  eq(repaired.served, true, 'the repaired extraction finds the gap event');
  eq(legacy, false, 'the OLD extraction does not — there is no post-restart initial sync');
  // ...and that difference is the difference between reporting the fix and
  // silently grading it unproven.
  eq(
    gradeRestart({ preArrivals: 1, postArrivals: 1, servedAfterRestart: legacy, gapArrivals: 1 })
      .state,
    'INCOMPLETE',
    'the old control would have called a working fix UNPROVEN despite the message arriving',
  );

  // On the BROKEN product both extractions agree — which is why the defect went
  // unnoticed: the instrument was only ever exercised against broken code.
  eq(servedAfterRestartFrom(SYNCS_BROKEN, 10, '$gap').served, true, 'repaired agrees on broken');
  eq(
    legacyServedInInitialOnly(
      SYNCS_BROKEN.filter((s) => s.initial),
      1,
      '$gap',
    ),
    true,
    'legacy agrees on broken — the two only diverge on a FIXED product',
  );
});

test('R4 a dead restart (the process never came back) is INCOMPLETE, not LOSS', () => {
  // Without the post-restart control, a process that came up broken is
  // indistinguishable from one that dropped only the gap message.
  eq(
    gradeRestart({ preArrivals: 1, postArrivals: 0, servedAfterRestart: true, gapArrivals: 0 }).state,
    'INCOMPLETE',
    'no post-restart control',
  );
  eq(
    gradeRestart({ preArrivals: 0, postArrivals: 1, servedAfterRestart: true, gapArrivals: 0 }).state,
    'INCOMPLETE',
    'no pre-restart control',
  );
});

// ── the run verdict must be able to fail on a restart LOSS ─────────────────
//
// THIRD INSTRUMENT DEFECT, found by RUNNING rather than by reading, and
// repaired in this lane (LANE-BRIEF §6b-ii, §3.2).
//
// The first live run graded the restart probe LOSS — a real product finding —
// while `failed.length` stayed 0, because the probe is recorded outside
// `results`. That run exited RED anyway, but for an unrelated reason (email's
// six legs were NOT MEASURED). So the gate looked correct while being
// INCAPABLE OF FAILING ON THE THING IT HAD JUST FOUND: the moment email becomes
// measurable, a silent inbound loss across a restart would exit 0 GREEN.
//
// The verdict is a top-level expression in the driver's entry point rather than
// an importable function, so these assertions transcribe it — and V3 is the
// drift guard that keeps the transcription honest.
function verdictOf({ instrumentFault, failedLegs, ranEverything, emailProbe, restartVerdict }) {
  const restartLoss = restartVerdict === 'LOSS';
  const restartIncomplete = restartVerdict === 'INCOMPLETE';
  const probeFailed = (emailProbe ?? []).some((p) => !p.ok) || restartLoss;
  return instrumentFault || restartIncomplete
    ? 'INCOMPLETE'
    : failedLegs === 0 && ranEverything && !probeFailed
      ? 'GREEN'
      : 'RED';
}

test('V1 KNOWN-POSITIVE: a clean run with a surviving gap message is GREEN', () => {
  eq(
    verdictOf({
      instrumentFault: false,
      failedLegs: 0,
      ranEverything: true,
      emailProbe: [{ ok: true }],
      restartVerdict: 'PASS',
    }),
    'GREEN',
    'clean run',
  );
});

test('V2 KNOWN-NEGATIVE: a restart LOSS turns an otherwise-perfect run RED', () => {
  // The exact shape the first live run would have had if email were measurable:
  // every leg passing, everything measured, and a silent inbound loss.
  eq(
    verdictOf({
      instrumentFault: false,
      failedLegs: 0,
      ranEverything: true,
      emailProbe: [{ ok: true }],
      restartVerdict: 'LOSS',
    }),
    'RED',
    'restart loss must redden the run',
  );
});

test('V3 THIRD ASSERTION — the OLD verdict expression would have called that same run GREEN', () => {
  // Kept executable so the repair is demonstrated, not asserted. This is the
  // expression the driver actually shipped during the first live run.
  const oldVerdict = ({ instrumentFault, failedLegs, ranEverything }) =>
    instrumentFault ? 'INCOMPLETE' : failedLegs === 0 && ranEverything ? 'GREEN' : 'RED';
  const obs = {
    instrumentFault: false,
    failedLegs: 0,
    ranEverything: true,
    emailProbe: [{ ok: true }],
    restartVerdict: 'LOSS',
  };
  eq(oldVerdict(obs), 'GREEN', 'the old gate exits 0 on a proven silent inbound loss');
  eq(verdictOf(obs), 'RED', 'the repaired gate does not');
});

test('V4 a restart INCOMPLETE is INCOMPLETE, never a green and never a loss', () => {
  eq(
    verdictOf({
      instrumentFault: false,
      failedLegs: 0,
      ranEverything: true,
      emailProbe: [{ ok: true }],
      restartVerdict: 'INCOMPLETE',
    }),
    'INCOMPLETE',
    'a probe that could not be attributed must not be graded either way',
  );
});

test('V5 the transcription still matches the driver source (drift guard)', () => {
  const s = fs.readFileSync(path.join(HERE, 'f24-inbound.mjs'), 'utf8');
  for (const f of [
    "restart.verdict === 'LOSS'",
    "restart.verdict === 'INCOMPLETE'",
    'failed.length === 0 && ranEverything && !probeFailed',
    'result.instrument_fault || restartIncomplete',
  ]) {
    assert(s.includes(f), `driver verdict no longer contains: ${f}`);
  }
});

// ── liveness must distinguish a zombie ─────────────────────────────────────
//
// FOURTH INSTRUMENT DEFECT OF THIS LANE, and the third that failed in the
// direction that blames the product.
//
// The restart probe's central claim is "the binary was down when the gap
// message was delivered". It checked that with `process.kill(pid, 0)`, which is
// WRONG under this driver: node reaps children on the event loop, the driver's
// waits are blocking, so a child that died instantly stays a ZOMBIE and
// `kill(pid, 0)` succeeds for it.
//
// What that cost: run 1 reported `exit_secs=30 (SIGKILL)`, which reads as
// "--json-stream ignored SIGTERM for 30 seconds" — a PRODUCT claim about
// shutdown behaviour. It was very probably this bug. Reporting it would have
// been a fabricated finding against working code, which is precisely what this
// lane's brief warns is the failure mode of an under-detecting instrument.
//
// Measured below rather than argued, with a real process.
const Z = {};
{
  const c = spawn(process.execPath, ['-e', 'setInterval(()=>{},1000)'], { stdio: 'ignore' });
  await delay(400);
  Z.liveSaysLive = pidIsLive(c.pid); // known-POSITIVE: a genuinely running process
  c.kill('SIGKILL');
  // Block the event loop exactly as the driver does, so node cannot reap it.
  sleep(1500);
  Z.zombiePid = c.pid;
  Z.repairedSaysDead = pidIsLive(c.pid) === false; // known-NEGATIVE
  let old = true;
  try {
    process.kill(c.pid, 0);
  } catch {
    old = false;
  }
  Z.oldCheckSaysAlive = old; // the THIRD assertion's observation
  const ps = spawnSync('ps', ['-o', 'stat=', '-p', String(c.pid)], { encoding: 'utf8' });
  Z.psState = (ps.stdout ?? '').trim();
  await delay(50);
}

test('Z1 KNOWN-POSITIVE: a genuinely running process is reported live', () => {
  eq(Z.liveSaysLive, true, 'a running child must be live');
});

test('Z2 KNOWN-NEGATIVE: a SIGKILLed, unreaped zombie is reported DEAD', () => {
  assert(
    Z.psState.startsWith('Z') || Z.psState === '',
    `the scenario did not actually produce a zombie (ps state=${JSON.stringify(Z.psState)}); ` +
      `this assertion would be vacuous`,
  );
  eq(Z.repairedSaysDead, true, 'a zombie is not a live process');
});

test('Z3 THIRD ASSERTION — the OLD check reports that same zombie as ALIVE', () => {
  // Only meaningful while the scenario really is a zombie, asserted in Z2.
  assert(Z.psState.startsWith('Z'), `not a zombie (ps=${JSON.stringify(Z.psState)}) — Z3 vacuous`);
  eq(
    Z.oldCheckSaysAlive,
    true,
    'process.kill(pid,0) must still report the zombie alive, or this repair changes nothing',
  );
  eq(pidIsLive(Z.zombiePid), false, 'and the repaired check must disagree with it');
});

// ═══════════════════════════════════════════════════════════════════════════
// 4. PRODUCT CONTRACTS THIS RUN DEPENDS ON — read from source
// ═══════════════════════════════════════════════════════════════════════════
//
// Every one of these is an assertion about the PRODUCT, not the harness. They
// exist so that a change in the product reddens here and says which one, rather
// than silently changing what a green run means.

function src(rel) {
  const p = path.join(REPO, rel);
  assert(fs.existsSync(p), `source file missing: ${rel}`);
  return fs.readFileSync(p, 'utf8');
}

test('P1 signal STILL has a production default for signal_cli_path (the seam control)', () => {
  // The control assertion for signal's seam. matrix has no production default,
  // so pointing homeserver_url at a fixture masks nothing; signal DOES, so this
  // run must not be readable as evidence that a config naming no path works.
  const s = src('crates/wcore-channel-signal/src/config.rs');
  assert(
    s.includes('#[serde(default = "default_signal_cli_path")]'),
    'signal_cli_path lost its serde default',
  );
  assert(
    s.includes('PathBuf::from("signal-cli")'),
    'the production default is no longer a bare `signal-cli` on PATH',
  );
});

test('P2 matrix homeserver_url STILL has NO default and NO production constant', () => {
  const s = src('crates/wcore-channel-matrix/src/config.rs');
  const line = s.split('\n').find((l) => l.includes('pub homeserver_url'));
  assert(line, 'homeserver_url field is gone');
  const idx = s.indexOf('pub homeserver_url');
  const before = s.slice(Math.max(0, idx - 200), idx);
  assert(
    !before.includes('serde(default'),
    'homeserver_url gained a serde default — a fixture run would now be masking one',
  );
});

test('P3 matrix now RESUMES the /sync cursor from disk (F24-C3-H6 fixed in lane 24-h6)', () => {
  // HISTORY, because a green here must not be misread. Until lane 24-h6 this
  // test asserted the OPPOSITE — that `sync.rs` still contained
  // `let mut since: Option<String> = None;` — as the standing precondition of
  // the restart finding, deliberately written to REDDEN AND NAME ITSELF the
  // moment someone repaired the product. It did exactly that, on the first run
  // after the fix landed. Inverted here rather than deleted, so the file still
  // records that the defect was real and is now closed, and so a REGRESSION
  // (someone dropping the persistence) reddens again.
  const s = src('crates/wcore-channel-matrix/src/sync.rs');
  assert(
    !s.includes('let mut since: Option<String> = None;'),
    'the /sync cursor is a process-local `None` seed again — F24-C3-H6 HAS REGRESSED',
  );
  assert(
    s.includes('sync_store::load_from(&state_path)'),
    'sync_loop no longer loads a persisted cursor — the downtime window is being lost again',
  );
  assert(
    s.includes('sync_store::save_to(&state_path, &next_batch)'),
    'sync_loop no longer persists the cursor — the NEXT restart will lose its window',
  );
  // The replay guard must SURVIVE the fix: resuming must not be achieved by
  // deleting the guard, which would replay the whole room backlog at boot.
  assert(s.includes('let is_initial = since.is_none();'), 'the initial-sync branch changed shape');
  assert(s.includes('if !is_initial {'), 'the replay guard was removed rather than complemented');
});

test('P4 CONTROL FOR P3 — the same search finds persistence in BOTH adapters now', () => {
  // LANE-BRIEF §3b-i: prove the instrument alive on a known-positive. Until
  // 24-h6 this asserted the vocabulary was found in email and ABSENT in matrix.
  // The absence is now closed, so the known-positive (email) is what keeps this
  // honest: if the regex stopped matching anything at all, both halves would
  // pass vacuously.
  const mx = src('crates/wcore-channel-matrix/src/sync_store.rs');
  const im = src('crates/wcore-channel-email/src/imap.rs');
  const rx = /persist|watermark|fs::write|fs::read_to_string/;
  assert(rx.test(im), 'the known-positive failed: no persistence vocabulary in imap.rs — INSTRUMENT DEAD');
  assert(
    rx.test(mx),
    'matrix has no persistence module content — F24-C3-H6 may have been reverted',
  );
  // And the shape is the sibling's, not a second invented mechanism: keyed
  // per-account under the same channel-state directory.
  assert(
    mx.includes("join(\"channel-state\")"),
    'the matrix cursor no longer lives beside the email watermark in channel-state/',
  );
});

test('P5 the shipped registry STILL builds both adapters through new(), not a hidden constructor', () => {
  const s = src('crates/wcore-channels-registry/src/lib.rs');
  assert(s.includes('MatrixChannel::new('), 'make_matrix no longer calls MatrixChannel::new');
  assert(s.includes('SignalChannel::new('), 'make_signal no longer calls SignalChannel::new');
  assert(s.includes('"matrix" => Some(make_matrix)'), 'matrix no longer registered');
  assert(s.includes('"signal" => Some(make_signal)'), 'signal no longer registered');
});

test('P6 signal STILL spawns the configured path with signal-cli argv', () => {
  const s = src('crates/wcore-channel-signal/src/subprocess.rs');
  assert(s.includes('Command::new(cli_path)'), 'the launcher no longer spawns the configured path');
  assert(s.includes('.arg("jsonRpc")'), 'the jsonRpc argv changed');
  assert(
    src('crates/wcore-channel-signal/src/lib.rs').includes('Arc::new(RealLauncher)'),
    'new() no longer hardwires RealLauncher — the fixture may no longer be exercising the shipped path',
  );
});

test('P7 matrix STILL types a room Direct only on m.joined_member_count == 2', () => {
  // The fixture's summary block depends on this. If the rule changes, the
  // fixture's rooms could silently become Group and every message would be
  // dropped by `group = "disabled"` — reported as inbound loss.
  const s = src('crates/wcore-channel-matrix/src/sync.rs');
  assert(s.includes('Some(2) => ChatType::Direct'), 'the Direct-chat rule changed');
});

test('P8 signal STILL derives the message id from the envelope timestamp', () => {
  // S2 depends on this: the dedupe leg replays by reusing the timestamp.
  const s = src('crates/wcore-channel-signal/src/subprocess.rs');
  assert(s.includes('let id = format!("{ts_ms}");'), 'the signal message id is no longer the timestamp');
});

cleanup();
process.stdout.write(`\nSELFTEST ${failed === 0 ? 'GREEN' : 'RED'} passed=${passed} failed=${failed}\n`);
process.exit(failed === 0 ? 0 : 1);
