#!/usr/bin/env node
// F24-C3-DISCORD — self-test for the fixture and, more importantly, for the
// INSTRUMENT.
//
// §6b-ii of the lane brief: when you find a defect in your own instrument you
// repair it in the same lane, and the repaired instrument gets a self-test with
// THREE assertions, not two:
//   1. known-positive passes
//   2. known-negative fails
//   3. THE OLD BROKEN MATCHER WOULD HAVE MISSED IT
// The third is the only one that proves the repair does anything — without it
// the self-test passes on the broken instrument too, which is exactly how the
// eleventh instance of this defect class went unnoticed.
//
// It also proves the fixture can complete a real RFC6455 handshake and a
// Discord opcode exchange, using a hand-rolled client in a separate socket, so
// that a later run reporting zero arrivals is distinguishable from a fixture
// that never worked.
//
// usage: node scripts/f24-discord-selftest.mjs

import assert from 'node:assert';
import crypto from 'node:crypto';
import net from 'node:net';

import { DiscordFixture } from './f24-discord-fixture.mjs';
import { matchesToken, naiveMatch, normalizeForMatch } from './f24-discord-inbound.mjs';

let passed = 0;
let failed = 0;

// MEASURED DEFECT IN THIS FILE, FOUND AND FIXED IN THIS LANE.
//
// The first version of `check` was `try { fn(); passed += 1 } catch {...}`. If
// `fn` is an `async` arrow, its assertion failure rejects a promise instead of
// throwing, so `check` sees NO exception, prints `ok`, and increments `passed`.
// One test in this file (`fixture C`) was written `async` by accident and was
// therefore a tautology.
//
// It is fully silent, not merely noisy. Measured on node v22:
//   - without a trailing process.exit: prints `ok`, prints `passed=1 failed=0`,
//     then crashes on the unhandled rejection => rc=1 AFTER a green summary.
//   - WITH the trailing `process.exit(failed === 0 ? 0 : 1)` this file actually
//     has: prints `ok`, prints `passed=1 failed=0`, **rc=0**. The rejection
//     never surfaces because the process is already gone.
// So the shape this file had reported a deliberately-false assertion as a pass
// with a zero exit status. That is a self-passing gate of exactly the class the
// brief lists, sitting inside the very file whose job is to prove the other
// instruments cannot self-pass.
//
// The repair is structural rather than "make fixture C sync": any future async
// test added here is now a hard failure instead of a silent pass.
function check(name, fn) {
  try {
    const r = fn();
    if (r && typeof r.then === 'function') {
      // Swallow the eventual rejection so it cannot also crash the process
      // with a confusing second error; the FAIL below is the real report.
      r.then(
        () => {},
        () => {},
      );
      throw new Error(
        'check() received an async/thenable function. Its assertions resolve ' +
          'AFTER check() returns, so a failure would be reported as a pass. ' +
          'Make the test synchronous, or await the work before calling check().',
      );
    }
    passed += 1;
    process.stdout.write(`ok   ${name}\n`);
  } catch (e) {
    failed += 1;
    process.stdout.write(`FAIL ${name}\n     ${e?.message ?? e}\n`);
  }
}

// ── minimal WS client (masked frames, per RFC6455 client rules) ───────────────

function clientFrame(payload) {
  const data = Buffer.from(payload, 'utf8');
  const mask = crypto.randomBytes(4);
  const len = data.length;
  let header;
  if (len < 126) {
    header = Buffer.alloc(2);
    header[1] = 0x80 | len;
  } else {
    header = Buffer.alloc(4);
    header[1] = 0x80 | 126;
    header.writeUInt16BE(len, 2);
  }
  header[0] = 0x81; // FIN + text
  const masked = Buffer.from(data);
  for (let i = 0; i < masked.length; i += 1) masked[i] ^= mask[i % 4];
  return Buffer.concat([header, mask, masked]);
}

function readServerFrames(buf) {
  const out = [];
  let off = 0;
  for (;;) {
    if (buf.length - off < 2) break;
    let len = buf[off + 1] & 0x7f;
    let p = off + 2;
    if (len === 126) {
      if (buf.length - p < 2) break;
      len = buf.readUInt16BE(p);
      p += 2;
    } else if (len === 127) {
      if (buf.length - p < 8) break;
      len = Number(buf.readBigUInt64BE(p));
      p += 8;
    }
    if (buf.length - p < len) break;
    out.push(buf.subarray(p, p + len).toString('utf8'));
    off = p + len;
  }
  return { out, rest: buf.subarray(off) };
}

function gatewayHandshake(port, token, { dispatchAfterReady } = {}) {
  return new Promise((resolve, reject) => {
    const sock = net.connect(port, '127.0.0.1');
    const key = crypto.randomBytes(16).toString('base64');
    const seen = [];
    let buf = Buffer.alloc(0);
    let upgraded = false;
    const timer = setTimeout(() => {
      sock.destroy();
      reject(new Error(`timeout; saw ${JSON.stringify(seen)}`));
    }, 8000);

    sock.on('connect', () => {
      sock.write(
        `GET /?v=10&encoding=json HTTP/1.1\r\nHost: 127.0.0.1:${port}\r\n` +
          `Upgrade: websocket\r\nConnection: Upgrade\r\n` +
          `Sec-WebSocket-Key: ${key}\r\nSec-WebSocket-Version: 13\r\n\r\n`,
      );
    });

    sock.on('data', (chunk) => {
      buf = Buffer.concat([buf, chunk]);
      if (!upgraded) {
        const idx = buf.indexOf('\r\n\r\n');
        if (idx === -1) return;
        const head = buf.subarray(0, idx).toString();
        if (!/101 Switching Protocols/.test(head)) {
          clearTimeout(timer);
          sock.destroy();
          return reject(new Error(`no 101: ${head.split('\r\n')[0]}`));
        }
        const expect = crypto
          .createHash('sha1')
          .update(key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11')
          .digest('base64');
        if (!head.includes(expect)) {
          clearTimeout(timer);
          sock.destroy();
          return reject(new Error('Sec-WebSocket-Accept mismatch'));
        }
        upgraded = true;
        buf = buf.subarray(idx + 4);
      }
      const { out, rest } = readServerFrames(buf);
      buf = rest;
      for (const text of out) {
        const m = JSON.parse(text);
        seen.push(m.t ? `op${m.op}:${m.t}` : `op${m.op}`);
        if (m.op === 10) {
          sock.write(clientFrame(JSON.stringify({ op: 2, d: { token, intents: 37376 } })));
          sock.write(clientFrame(JSON.stringify({ op: 1, d: null })));
        }
        if (m.op === 0 && m.t === 'READY' && dispatchAfterReady) {
          dispatchAfterReady();
        }
        if (m.op === 0 && m.t === 'MESSAGE_CREATE') {
          clearTimeout(timer);
          sock.destroy();
          return resolve({ seen, message: m });
        }
        if (!dispatchAfterReady && m.op === 11) {
          clearTimeout(timer);
          sock.destroy();
          return resolve({ seen, message: null });
        }
      }
    });
    sock.on('error', (e) => {
      clearTimeout(timer);
      reject(e);
    });
  });
}

// ── 1. the instrument: three assertions ──────────────────────────────────────

const TOKEN = 'f24c3-disc-admit-ab12';

check('instrument A: known-POSITIVE — a clean reply carrying the token matches', () => {
  assert.strictEqual(matchesToken(`F24C3-REPLY ${TOKEN}`, TOKEN), true);
});

check('instrument B: known-NEGATIVE — a reply carrying a DIFFERENT token does NOT match', () => {
  assert.strictEqual(matchesToken('F24C3-REPLY f24c3-disc-admit-zz99', TOKEN), false);
  // and a reply with no token at all
  assert.strictEqual(matchesToken('F24C3-REPLY no-correlation', TOKEN), false);
  // If this ever passes, every leg in the driver is a tautology.
});

check('instrument C: THE OLD MATCHER WOULD HAVE MISSED IT (the repair does something)', () => {
  // Exactly the two shapes that have already destroyed measurements on this
  // program: MarkdownV2 backslash escaping, and a console line wrap splicing a
  // newline into the middle of the token.
  const escaped = `F24C3\\-REPLY f24c3\\-disc\\-admit\\-ab12`;
  const wrapped = `F24C3-REPLY f24c3-disc-adm\nit-ab12`;

  for (const [label, text] of [
    ['escaped', escaped],
    ['wrapped', wrapped],
  ]) {
    assert.strictEqual(matchesToken(text, TOKEN), true, `repaired matcher must find the ${label} token`);
    assert.strictEqual(
      naiveMatch(text, TOKEN),
      false,
      `PRECONDITION: the pre-repair matcher must MISS the ${label} token — ` +
        `if it does not, this assertion proves nothing and the repair is untested`,
    );
  }
});

check('instrument D: normalization does not collapse distinct tokens into each other', () => {
  // A normalizer that stripped too much would make every leg pass.
  assert.notStrictEqual(normalizeForMatch('f24c3-a-1'), normalizeForMatch('f24c3-a-2'));
  assert.strictEqual(matchesToken('reply f24c3-steady1-aa', 'f24c3-steady2-aa'), false);
});

// ── 1b. the HARNESS itself: prove `check` cannot report an async pass ─────────
//
// Same three-assertion rule applied to the repair of `check`, because a guard
// that does not fire is indistinguishable from no guard at all.

check('harness A: known-POSITIVE — an ordinary passing sync test is still reported ok', () => {
  assert.strictEqual(1, 1);
});

{
  // known-NEGATIVE: a sync test that fails must be counted as a failure.
  const before = failed;
  check('harness B: (expected FAIL) a false sync assertion is counted', () => {
    assert.strictEqual(1, 2, 'intentional');
  });
  check('harness B-verify: known-NEGATIVE — the false sync test above was counted as FAIL', () => {
    assert.strictEqual(failed, before + 1, 'a failing sync test must increment `failed`');
  });
  failed = before; // un-count the intentional failure
}

{
  // THE THIRD ASSERTION: the OLD `check` would have reported this as a pass.
  const beforeFailed = failed;
  const beforePassed = passed;
  check('harness C: (expected FAIL) an async test with a FALSE assertion', async () => {
    assert.strictEqual(1, 2, 'this must not be reported as a pass');
  });
  check('harness C-verify: THE OLD HARNESS WOULD HAVE MISSED IT', () => {
    assert.strictEqual(
      failed,
      beforeFailed + 1,
      'the async test with a false assertion must be counted as a FAILURE',
    );
    assert.strictEqual(
      passed,
      beforePassed,
      'and it must NOT have been counted as a pass — the pre-repair harness ' +
        'incremented `passed` here and exited 0, which is the whole defect',
    );
  });
  failed = beforeFailed; // un-count the intentional failure
}

// ── 2. the fixture: protocol reality ─────────────────────────────────────────

const fx = new DiscordFixture();
await fx.start();

try {
  const r = await gatewayHandshake(fx.port, fx.botToken);
  check('fixture A: HELLO -> IDENTIFY -> READY -> HEARTBEAT_ACK completes', () => {
    assert.ok(r.seen.includes('op10'), `expected HELLO, saw ${r.seen}`);
    assert.ok(r.seen.includes('op0:READY'), `expected READY, saw ${r.seen}`);
    assert.ok(r.seen.includes('op11'), `expected HEARTBEAT_ACK, saw ${r.seen}`);
  });

  const r2 = await gatewayHandshake(fx.port, fx.botToken, {
    dispatchAfterReady: () =>
      setTimeout(
        () =>
          fx.dispatchMessage({
            id: 'm1',
            channelId: '900000001',
            content: `hello ${TOKEN}`,
            authorId: '5150001',
          }),
        50,
      ),
  });
  check('fixture B: MESSAGE_CREATE is dispatched and carries the required fields', () => {
    assert.ok(r2.message, 'no MESSAGE_CREATE arrived');
    assert.strictEqual(r2.message.t, 'MESSAGE_CREATE');
    // Only `id` and `channel_id` are non-defaulted in the Rust decoder; if the
    // fixture ever stops sending them the adapter drops the frame silently and
    // the run reads as inbound LOSS.
    assert.ok(r2.message.d.id, 'MESSAGE_CREATE.d.id is required by MessageCreate');
    assert.ok(r2.message.d.channel_id, 'MESSAGE_CREATE.d.channel_id is required');
    assert.ok(r2.message.d.content.includes(TOKEN));
    assert.ok(r2.message.s > 0, 'dispatch must carry a sequence number for RESUME');
  });

  check('fixture C: a token the fixture did NOT mint is refused (no green by universal acceptance)', () => {
    // Recorded rather than silently dropped — a wrong token must read as an
    // auth failure, not as inbound loss.
    // (This test was `async` in the first draft and was therefore a tautology;
    // see the note on `check` above.)
    assert.strictEqual(fx.badTokenIdentifies, 0, 'precondition: none yet');
  });

  const bad = await gatewayHandshake(fx.port, 'not-the-minted-token').catch((e) => ({
    seen: [],
    err: e.message,
  }));
  check('fixture D: wrong token yields op9 INVALID_SESSION and is COUNTED as auth failure', () => {
    assert.strictEqual(fx.badTokenIdentifies, 1, 'the bad IDENTIFY must be counted');
    assert.ok(
      bad.seen.includes('op9') || bad.err,
      `expected op9 or a clean failure, saw ${JSON.stringify(bad)}`,
    );
    assert.ok(fx.report().fixture_notes.some((n) => /did not mint/.test(n)));
  });

  check('fixture E: concurrent-connection counter is the race instrument and it moved', () => {
    // Three handshakes happened above; they were sequential, but the fixture
    // must have observed and counted every one of them.
    assert.ok(fx.totalConns >= 3, `expected >=3 total conns, got ${fx.totalConns}`);
    assert.ok(fx.maxConcurrentConns >= 1, 'max concurrent must be at least 1');
  });

  // The REST half, including that it enforces the minted token.
  const restOk = await fetch(`${fx.apiBase}/api/v10/users/@me`, {
    headers: { authorization: `Bot ${fx.botToken}` },
  });
  const restBad = await fetch(`${fx.apiBase}/api/v10/users/@me`, {
    headers: { authorization: 'Bot wrong' },
  });
  check('fixture F: REST /users/@me accepts the minted token and rejects any other', () => {
    assert.strictEqual(restOk.status, 200);
    assert.strictEqual(restBad.status, 401);
  });
} finally {
  await fx.stop();
}

process.stdout.write(`\nF24C3DISC SELFTEST passed=${passed} failed=${failed}\n`);
process.exit(failed === 0 ? 0 : 1);
