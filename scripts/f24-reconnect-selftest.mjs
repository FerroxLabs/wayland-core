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

import { execFileSync, spawn } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { DiscordFixture } from './f24-discord-fixture.mjs';
import { census, mintToken } from './f24-reconnect.mjs';

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
  // The try/catch is not decoration. Without it a thrown assertion escapes
  // main(), the process exits 2 (this file's USAGE code), `failed` is never
  // incremented and the `passed=N failed=M` verdict line is never printed —
  // so a genuine assertion failure is indistinguishable from a usage error and
  // the count a reader is told to read back does not exist. Found 2026-07-29 by
  // the mutation sweep in `24-RECONNECT-evidence/mutate-instrument.py`: all
  // three mutations "REDDENED" with rc=2 and `NO VERDICT LINE`, which grades a
  // failure correctly ONLY by accident of the exit code. Repaired rather than
  // noted (LANE-BRIEF §6b-ii).
  let r;
  try {
    r = fn();
  } catch (e) {
    fail(name, e);
    return;
  }
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

  // ── T  the token shape, against the SHARED fixture's real behaviour ───────
  //
  // Run 1 of this lane was destroyed by a token-shape mismatch: the shared
  // `f24-llm-fixture.mjs` echoes only `/f24c3-[a-z0-9-]+/i`, so every reply
  // read `no-correlation`, and the driver reported 0/2 on its own control for
  // 6 messages that had ALL arrived. It presented as total inbound message
  // loss. This is asserted against the LIVE fixture rather than against a copy
  // of its regex, because a re-implemented contract drifts away from the real
  // one silently.
  {
    const journal = path.join(os.tmpdir(), `f24rc-selftest-${crypto.randomBytes(4).toString('hex')}.jsonl`);
    const logFile = `${journal}.log`;
    fs.writeFileSync(logFile, '');
    const fd = fs.openSync(logFile, 'a');
    const llm = spawn(process.execPath, [path.join(HERE, 'f24-llm-fixture.mjs'), '--port', '0', '--journal', journal], {
      stdio: ['ignore', fd, fd],
    });
    let url = null;
    for (let i = 0; i < 120 && !url; i++) {
      const m = /http:\/\/127\.0\.0\.1:\d+/.exec(fs.readFileSync(logFile, 'utf8'));
      if (m) url = m[0];
      else await sleep(50);
    }
    const ask = (text) => {
      const r = JSON.parse(
        execFileSync(
          'curl',
          [
            '-s',
            '-X',
            'POST',
            `${url}/chat/completions`,
            '-H',
            'content-type: application/json',
            '-d',
            JSON.stringify({ model: 'x', stream: false, messages: [{ role: 'user', content: `hello ${text}` }] }),
          ],
          { encoding: 'utf8', timeout: 15_000 },
        ),
      );
      return String(r?.choices?.[0]?.message?.content ?? '');
    };

    try {
      assert(url, 'llm fixture never announced a URL');
      const good = mintToken('deadbe', 'before', 0);
      const legacyShape = 'f24rc-deadbe-before-0';
      const goodReply = ask(good);
      const legacyReply = ask(legacyShape);

      await check('T1 known-POSITIVE: a token this driver mints IS echoed by the shared llm fixture', () => {
        assert(goodReply.includes(good), `fixture answered "${goodReply}" for ${good}`);
      });

      await check('T2 known-NEGATIVE: the fixture does not echo a wrong-shaped token (so T1 discriminates)', () => {
        assert(!legacyReply.includes(legacyShape), `fixture echoed the wrong-shaped token: "${legacyReply}"`);
        assert(
          legacyReply.includes('no-correlation'),
          `expected the wrong shape to yield no-correlation, got "${legacyReply}"`,
        );
      });

      await check('T3 THE SHAPE THIS LANE FIRST USED WOULD HAVE MISSED IT — and would have read as total loss', () => {
        // The measured third assertion. The old prefix produces a reply the
        // census cannot match, so an entire live run of a WORKING product
        // grades as complete inbound message loss.
        const c = census(
          [{ token: legacyShape, phase: 'before', expect: 'reply' }],
          [{ content: legacyReply }],
        );
        assert(c.lost.length === 1, 'the old token shape should have produced a LOSS against a real fixture reply');
        const c2 = census([{ token: good, phase: 'before', expect: 'reply' }], [{ content: goodReply }]);
        assert(c2.lost.length === 0, 'the repaired token shape must NOT produce a loss against a real fixture reply');
      });
    } catch (e) {
      fail('T token-shape assertions', e);
    } finally {
      try {
        llm.kill('SIGKILL');
      } catch {
        /* noop */
      }
      fs.rmSync(journal, { force: true });
      fs.rmSync(logFile, { force: true });
    }
  }

  // ── R4  the negative control's own lever ──────────────────────────────────
  //
  // The live driver's `--control-no-replay` run is what proves its loss
  // detector fires. If the lever were inert, that run would replay normally,
  // report no loss, and the only honest reading would be "the detector is
  // dead" — while the tempting reading is "the product passed". So the lever
  // itself needs a measurement.
  class NoReplayFixture extends DiscordFixture {
    constructor(o) {
      super(o);
      this.replayOnResume = false;
    }
  }
  const N = await runScenario(NoReplayFixture, { dropVia: 'fixture' });
  await check('R4 the replay kill-switch actually suppresses the replay', () => {
    assert(N.before.length === 2, `control run pre-drop control: ${N.before.length} arrivals`);
    assert(N.resumed, 'control run: RESUME was not accepted — the suppression must be of the REPLAY, not of the resume');
    assert(N.afterAlive, 'control run: post-resume control did not arrive, so its zero would be free');
    assert(!N.duringReplayed, 'the kill-switch is INERT — the gap was replayed anyway');
    assert(N.collisions === 0, `the kill-switch must not reintroduce a collision; got ${N.collisions}`);
  });

  // ── C  the census itself ──────────────────────────────────────────────────
  //
  // The census is the thing that turns a pile of replies into the two numbers
  // this lane reports. It is pure, so it is exercised directly here rather than
  // only through a 3-minute live run — a detector that can only be reached
  // through a live run is a detector nobody proves can fail.
  const PLAN = [
    { token: 'tok-before-1', phase: 'before', expect: 'reply' },
    { token: 'tok-during-1', phase: 'during', expect: 'reply' },
    { token: 'tok-decoy-1', phase: 'during-decoy', expect: 'silence' },
    { token: 'tok-phantom-1', phase: 'phantom', expect: 'silence' },
  ];
  const reply = (t) => ({ content: `F24-REPLY ${t}` });

  await check('C1 census known-POSITIVE: a clean run reports no loss, no duplicate, no leak', () => {
    const c = census(PLAN, [reply('tok-before-1'), reply('tok-during-1')]);
    assert(c.lost.length === 0, `lost=[${c.lost}]`);
    assert(c.duplicated.length === 0, `duplicated=[${c.duplicated}]`);
    assert(c.leaked.length === 0, `leaked=[${c.leaked}]`);
  });

  await check('C2 census DETECTS A LOSS — the gap message missing is reported, not swallowed', () => {
    const c = census(PLAN, [reply('tok-before-1')]);
    assert(c.lost.length === 1 && c.lost[0] === 'tok-during-1', `expected the gap token lost, got [${c.lost}]`);
    // Attribution: the surviving control must NOT also be reported lost, or the
    // detector is failing everything rather than discriminating.
    assert(!c.lost.includes('tok-before-1'), 'the pre-drop control was also reported lost — detector is blanket-failing');
  });

  await check('C3 census DETECTS A DUPLICATE — two turns for one message is reported', () => {
    const c = census(PLAN, [reply('tok-before-1'), reply('tok-during-1'), reply('tok-during-1')]);
    assert(c.duplicated.length === 1 && c.duplicated[0] === 'tok-during-1', `duplicated=[${c.duplicated}]`);
    assert(c.lost.length === 0, `a duplicate must not also read as a loss; lost=[${c.lost}]`);
  });

  await check('C4 census DETECTS A LEAK — a decoy or phantom that replied invalidates the run', () => {
    const c = census(PLAN, [reply('tok-before-1'), reply('tok-during-1'), reply('tok-decoy-1')]);
    assert(c.leaked.length === 1 && c.leaked[0] === 'tok-decoy-1', `leaked=[${c.leaked}]`);
    const c2 = census(PLAN, [reply('tok-before-1'), reply('tok-during-1'), reply('tok-phantom-1')]);
    assert(c2.leaked.includes('tok-phantom-1'), 'a reply to a NEVER-DISPATCHED token must be reported');
  });

  await check('C5 census survives the mangling that has already destroyed one pass on this program', () => {
    // MarkdownV2 escaping turned a token into `f24c3\-h4\-pre\-0\-ab12` and a
    // driver reported 0/8 for eight replies that had all arrived. A console
    // line wrap did the same to a later lane. If the census used a naive
    // includes(), a WRAPPED reply would read as LOSS — a fabricated HIGH.
    const c = census(PLAN, [{ content: 'F24-REPLY tok\\-before\\-1' }, { content: 'F24-REPLY tok-during\n-1' }]);
    assert(c.lost.length === 0, `escaped/wrapped replies were read as losses: lost=[${c.lost}]`);
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
