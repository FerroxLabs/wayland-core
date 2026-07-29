#!/usr/bin/env node
// F24-RECONNECT — self-test for the reconnect instrument.
//
// The reconnect clause of 24-C3 asks whether an adapter recovers from an
// UPSTREAM disconnect *without losing or duplicating* messages delivered around
// the disconnect window. Every figure that answers it is a count of arrivals,
// and the headline claim is an ABSENCE — "nothing was lost". LANE-BRIEF §3b-i:
// an absence claim is self-passing on a dead instrument. A broken client, a
// fixture that never dispatched, a drop that never happened, a detector that
// always returns the same answer — every one of those produces "nothing was
// lost" for free, and three of them also produce "everything was lost".
//
// So this file exists to make the instrument fail on demand, and it carries the
// THIRD assertion LANE-BRIEF §6b-ii requires: not merely that the repaired
// instrument passes a positive and fails a negative, but that the PRE-REPAIR
// instrument is proven to get this scenario wrong. Without that third
// assertion a self-test passes just as happily on the broken instrument.
//
// The pre-repair fixture is not re-implemented here. It is extracted BYTE-EXACT
// from the merge-base commit with `git show <SHA>:scripts/f24-discord-fixture.mjs`
// and imported. A self-test that re-implements its instrument drifts away from
// it silently (24-H5 §6 made exactly this point), and a re-derived "legacy"
// allocator would be a re-derivation of the very bug being demonstrated.
//
// usage: node f24-reconnect-selftest.mjs [--base <sha>]
// exit:  0 GREEN, 1 RED, 2 USAGE

import { execFileSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { DiscordFixture } from './f24-discord-fixture.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, '..');

// Unproxied. `rtk` rewrites `git log` (drops merge commits) and `grep`; a
// merge-base or a file body taken through it is not a measurement.
const GIT = '/usr/bin/git';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ── tiny harness ────────────────────────────────────────────────────────────
// Rejects a thenable test function. An async test's assertions resolve AFTER
// check() returns, so a failure would be counted as a pass — the same class of
// self-passing gate this whole file exists to prevent.
let passed = 0;
let failed = 0;
const lines = [];
async function check(name, fn) {
  const r = fn();
  if (r && typeof r.then === 'function') {
    failed += 1;
    lines.push(`FAIL ${name}\n     test returned a thenable; await inside it instead`);
    return;
  }
  passed += 1;
  lines.push(`ok   ${name}`);
}
function fail(name, err) {
  failed += 1;
  lines.push(`FAIL ${name}\n     ${String(err?.message ?? err).split('\n').join('\n     ')}`);
}
function assert(cond, msg) {
  if (!cond) throw new Error(msg);
}

// ── a minimal gateway client, shared by every scenario ──────────────────────
// Deliberately dumb: it records what it is SENT and nothing else. It does not
// know what it is supposed to receive, so it cannot confirm a hypothesis.
class Client {
  constructor(url, token) {
    this.url = url;
    this.token = token;
    this.seen = [];
    this.seq = null;
    this.sessionId = null;
    this.resumed = false;
    this.ready = false;
  }

  async open({ resume = null } = {}) {
    this.ws = new WebSocket(this.url);
    await new Promise((res, rej) => {
      this.ws.addEventListener('open', res, { once: true });
      this.ws.addEventListener('error', rej, { once: true });
    });
    this.ws.addEventListener('message', (ev) => {
      const m = JSON.parse(typeof ev.data === 'string' ? ev.data : ev.data.toString());
      if (typeof m.s === 'number') this.seq = m.s;
      if (m.op === 10) {
        this.ws.send(
          resume
            ? JSON.stringify({ op: 6, d: { token: this.token, session_id: resume.sessionId, seq: resume.seq } })
            : JSON.stringify({ op: 2, d: { token: this.token, intents: 33280 } }),
        );
        return;
      }
      if (m.op === 0 && m.t === 'READY') {
        this.ready = true;
        this.sessionId = m.d.session_id;
      } else if (m.op === 0 && m.t === 'RESUMED') {
        this.resumed = true;
      } else if (m.op === 0 && m.t === 'MESSAGE_CREATE') {
        this.seen.push(m.d.id);
      }
    });
    return this;
  }
}

async function waitFor(pred, ms = 3000) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (pred()) return true;
    await sleep(25);
  }
  return false;
}

/**
 * The one scenario every assertion in this file runs, against whichever fixture
 * class it is handed. ONE variable: the fixture.
 *
 * before → drop → during → resume → after.
 *
 * `dropVia` is a parameter because the two fixtures do not share a drop
 * surface: `dropAllSockets()` is part of the repair. The legacy run drops the
 * socket from the CLIENT side instead, which is strictly more favourable to the
 * legacy fixture — if it still cannot replay, that is not an artefact of how it
 * was disconnected.
 */
async function runScenario(Fixture, { dropVia }) {
  const token = `f24-st-${crypto.randomBytes(6).toString('hex')}`;
  const fx = new Fixture({ botToken: token, heartbeatIntervalMs: 60_000 });
  await fx.start();
  const chan = '900000000000000001';
  const author = '900000000000000002';

  const c1 = await new Client(fx.gatewayUrl, token).open();
  const readied = await waitFor(() => c1.ready);

  // BEFORE — the known-positive. If these do not arrive, nothing downstream
  // means anything and the scenario reports it rather than scoring.
  fx.dispatchMessage({ id: 'BEFORE-1', channelId: chan, content: 'b1', authorId: author });
  fx.dispatchMessage({ id: 'BEFORE-2', channelId: chan, content: 'b2', authorId: author });
  await waitFor(() => c1.seen.length >= 2);
  const before = [...c1.seen];
  const resumeFrom = { sessionId: c1.sessionId, seq: c1.seq };

  // THE DROP.
  let dropped = 0;
  if (dropVia === 'fixture') {
    dropped = fx.dropAllSockets();
  } else {
    try {
      c1.ws.close();
    } catch {
      /* noop */
    }
    await waitFor(() => fx.conns.size === 0);
    dropped = 1;
  }
  await sleep(150);
  const liveAfterDrop = fx.conns.size;

  // DURING — dispatched into an empty connection set.
  const duringSockets = fx.dispatchMessage({ id: 'DURING-1', channelId: chan, content: 'd1', authorId: author });

  // RESUME.
  const c2 = await new Client(fx.gatewayUrl, token).open({ resume: resumeFrom });
  const resumed = await waitFor(() => c2.resumed);
  await sleep(150);

  // AFTER — the second known-positive. Proves c2 is alive, so "DURING-1
  // absent" cannot be explained by a dead second client.
  fx.dispatchMessage({ id: 'AFTER-1', channelId: chan, content: 'a1', authorId: author });
  await waitFor(() => c2.seen.includes('AFTER-1'));
  await sleep(100);

  const rep = fx.report();
  await fx.stop();

  const ledger = fx.dispatched.map((d) => ({ id: d.id, s: d.s }));
  const collisions = ledger.length - new Set(ledger.map((x) => x.s)).size;

  return {
    readied,
    before,
    resumeFrom,
    dropped,
    liveAfterDrop,
    duringSockets,
    resumed,
    afterSeen: [...c2.seen],
    ledger,
    collisions,
    report: rep,
    // The single derived verdict every assertion below keys on.
    duringReplayed: c2.seen.includes('DURING-1'),
    duringCount: c2.seen.filter((x) => x === 'DURING-1').length,
    afterAlive: c2.seen.includes('AFTER-1'),
  };
}

/** Extract the pre-repair fixture BYTE-EXACT from a commit and import it. */
function loadLegacyFixture(baseSha) {
  const src = execFileSync(GIT, ['show', `${baseSha}:scripts/f24-discord-fixture.mjs`], {
    cwd: REPO,
    encoding: 'utf8',
    maxBuffer: 32 * 1024 * 1024,
  });
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'f24-legacy-fixture-'));
  const file = path.join(dir, 'f24-discord-fixture.mjs');
  fs.writeFileSync(file, src);
  return { file, bytes: Buffer.byteLength(src, 'utf8'), dir };
}

async function main() {
  let base = null;
  const argv = process.argv.slice(2);
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--base') base = argv[++i];
  }
  if (!base) {
    base = execFileSync(GIT, ['merge-base', 'HEAD', 'plan/f20-unified-audit-repair'], {
      cwd: REPO,
      encoding: 'utf8',
    }).trim();
  }

  // ── R1  known-POSITIVE ────────────────────────────────────────────────────
  const R = await runScenario(DiscordFixture, { dropVia: 'fixture' });
  await check('R1 known-POSITIVE: a message dispatched during the disconnect window IS replayed on RESUME', () => {
    assert(R.readied, 'client never READYed — instrument dead, no conclusion available');
    assert(R.before.length === 2, `pre-drop control: expected 2 arrivals, got ${R.before.length} [${R.before}]`);
    assert(R.duringReplayed, `DURING-1 was not replayed; c2 saw [${R.afterSeen}]`);
    assert(R.afterAlive, 'post-resume control did not arrive — c2 was not alive, so no absence here is readable');
  });

  await check('R1b the disconnect is CONFIRMED, not assumed', () => {
    assert(R.dropped === 1, `expected to drop exactly 1 socket, dropped ${R.dropped}`);
    assert(R.liveAfterDrop === 0, `expected 0 live sockets after the drop, saw ${R.liveAfterDrop}`);
    assert(
      R.duringSockets === 0,
      `DURING-1 must reach ZERO sockets — it reached ${R.duringSockets}, so there was no disconnect window and the whole scenario is void`,
    );
    assert(R.resumed, 'the second client never received RESUMED');
    assert(R.report.resume_count >= 1, `fixture recorded no RESUME (resume_count=${R.report.resume_count})`);
    assert(R.report.forced_drop_sockets === 1, `fixture drop journal says ${R.report.forced_drop_sockets}`);
  });

  await check('R1c NO DUPLICATE: the replayed message arrives exactly once', () => {
    assert(R.duringCount === 1, `DURING-1 arrived ${R.duringCount} times, expected exactly 1`);
    assert(R.collisions === 0, `fixture allocated ${R.collisions} colliding sequence number(s)`);
    assert(
      R.report.duplicate_seq_numbers === 0,
      `report.duplicate_seq_numbers=${R.report.duplicate_seq_numbers} — a non-zero here means NO reconnect verdict from a run is readable`,
    );
  });

  await check('R1d the replay is journalled, so a driver never has to infer it', () => {
    assert(R.report.resume_replayed_total === 1, `resume_replayed_total=${R.report.resume_replayed_total}`);
    const ids = R.report.resume_replays.flatMap((x) => x.replayed);
    assert(ids.length === 1 && ids[0] === 'DURING-1', `replay journal says [${ids}]`);
    const during = R.report.dispatch_ledger.find((d) => d.id === 'DURING-1');
    assert(during, 'DURING-1 missing from the dispatch ledger');
    assert(during.sockets === 1, `DURING-1 ledger sockets=${during.sockets}; a replay must count as a delivery`);
  });

  // ── R2  known-NEGATIVE ────────────────────────────────────────────────────
  // The detector must be able to say NO. Asked about a message the fixture was
  // never given, it must not answer yes — otherwise R1 is worth nothing.
  await check('R2 known-NEGATIVE: a message that was never dispatched is NOT reported as replayed', () => {
    assert(!R.afterSeen.includes('NEVER-DISPATCHED-1'), 'detector claimed a message that never existed');
    const ids = R.report.resume_replays.flatMap((x) => x.replayed);
    assert(!ids.includes('NEVER-DISPATCHED-1'), 'replay journal claimed a message that never existed');
    assert(
      !R.report.dispatch_ledger.some((d) => d.id === 'NEVER-DISPATCHED-1'),
      'dispatch ledger claimed a message that never existed',
    );
  });

  await check('R2b known-NEGATIVE: a message dispatched BEFORE the resume point is NOT replayed again', () => {
    // BEFORE-1/2 were already delivered and sit at or below `resumeFrom.seq`.
    // Replaying them would be a DUPLICATE, which is the other half of the
    // clause. A fixture that replayed its whole journal would pass R1 and be
    // useless.
    assert(!R.afterSeen.includes('BEFORE-1'), 'BEFORE-1 was replayed — that is a duplicate, not a recovery');
    assert(!R.afterSeen.includes('BEFORE-2'), 'BEFORE-2 was replayed — that is a duplicate, not a recovery');
  });

  // ── R3  THE THIRD ASSERTION ───────────────────────────────────────────────
  let legacy = null;
  try {
    legacy = loadLegacyFixture(base);
    const mod = await import(pathToFileURL(legacy.file).href);
    const LegacyFixture = mod.DiscordFixture ?? mod.default;
    const L = await runScenario(LegacyFixture, { dropVia: 'client' });

    await check('R3 THE PRE-REPAIR FIXTURE WOULD HAVE MISSED IT — same scenario, gap NOT replayed', () => {
      assert(L.readied, 'legacy client never READYed — the legacy run is void, not a result');
      assert(L.before.length === 2, `legacy pre-drop control: ${L.before.length} arrivals [${L.before}]`);
      assert(L.liveAfterDrop === 0, `legacy: expected 0 live sockets after drop, saw ${L.liveAfterDrop}`);
      assert(L.duringSockets === 0, `legacy: DURING-1 reached ${L.duringSockets} sockets — no disconnect window`);
      assert(L.resumed, 'legacy: the second client never received RESUMED — no replay was even attempted');
      assert(L.afterAlive, 'legacy: post-resume control did not arrive, so its zero would be free');
      // The measurement.
      assert(
        !L.duringReplayed,
        'the PRE-REPAIR fixture replayed the gap message — the repair is a no-op and R1 proves nothing',
      );
      assert(
        L.collisions >= 1,
        `expected the pre-repair allocator to collide; it produced ${L.collisions} collisions, ledger=${JSON.stringify(L.ledger)}`,
      );
    });

    await check('R3b the legacy failure is the SEQUENCE COLLISION, not a dead legacy client', () => {
      // Attribution. Without this, R3 passes for any reason the legacy run
      // went quiet — which is the same free-zero problem one level up.
      const during = L.ledger.find((x) => x.id === 'DURING-1');
      const before2 = L.ledger.find((x) => x.id === 'BEFORE-2');
      assert(during && before2, `legacy ledger incomplete: ${JSON.stringify(L.ledger)}`);
      assert(
        during.s === before2.s,
        `expected DURING-1 to collide with BEFORE-2; got during.s=${during.s} before2.s=${before2.s}`,
      );
      assert(
        during.s <= L.resumeFrom.seq,
        `expected the gap seq (${during.s}) to sort at or below the client's resume point (${L.resumeFrom.seq}), which is why 'x.s > after' discards it`,
      );
    });

    lines.push(`     legacy fixture: ${base.slice(0, 8)}:scripts/f24-discord-fixture.mjs, ${legacy.bytes} bytes`);
    lines.push(`     legacy ledger : ${JSON.stringify(L.ledger)}`);
    lines.push(`     repaired ledger: ${JSON.stringify(R.ledger)}`);
  } catch (e) {
    fail('R3 THE PRE-REPAIR FIXTURE WOULD HAVE MISSED IT', e);
  } finally {
    if (legacy) fs.rmSync(legacy.dir, { recursive: true, force: true });
  }

  process.stdout.write(`${lines.join('\n')}\n\n`);
  process.stdout.write(`F24RECONNECT SELFTEST ${failed === 0 ? 'GREEN' : 'RED'} passed=${passed} failed=${failed}\n`);
  process.exit(failed === 0 ? 0 : 1);
}

main().catch((e) => {
  process.stderr.write(`${String(e?.stack ?? e)}\n`);
  process.exit(2);
});
