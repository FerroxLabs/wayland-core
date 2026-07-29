#!/usr/bin/env node
// Reproduction: does `f24-discord-fixture.mjs` allocate a sequence number that
// lets a message dispatched WHILE NO CLIENT IS CONNECTED be replayed on RESUME?
//
// This is the precondition for measuring the `reconnect` half of 24-C3 at all.
// If the FIXTURE cannot express "delivered during the disconnect window, then
// replayed", then any probe built on it reports LOSS for every product — a
// fabricated HIGH, and the fourth-plus instrument fault on this criterion that
// fails in the direction that blames the product.
//
// Run:  node fixture-seq-repro.mjs
// Exit: 0 = fixture can express the replay, 1 = it cannot (instrument defect)
//
// No cargo, no Rust, no product. This measures the INSTRUMENT only.

import { DiscordFixture } from '../../../../scripts/f24-discord-fixture.mjs';
import crypto from 'node:crypto';

const TOKEN = `f24-repro-${crypto.randomBytes(6).toString('hex')}`;
const CHAN = '900000000000000001';
const AUTHOR = '900000000000000002';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Minimal gateway client: IDENTIFY or RESUME, record every dispatch it sees. */
class Client {
  constructor(url, token) {
    this.url = url;
    this.token = token;
    this.seen = []; // [{s, id}]
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
        if (resume) {
          this.ws.send(
            JSON.stringify({ op: 6, d: { token: this.token, session_id: resume.sessionId, seq: resume.seq } }),
          );
        } else {
          this.ws.send(JSON.stringify({ op: 2, d: { token: this.token, intents: 33280 } }));
        }
        return;
      }
      if (m.op === 0 && m.t === 'READY') {
        this.ready = true;
        this.sessionId = m.d.session_id;
        return;
      }
      if (m.op === 0 && m.t === 'RESUMED') {
        this.resumed = true;
        return;
      }
      if (m.op === 0 && m.t === 'MESSAGE_CREATE') {
        this.seen.push({ s: m.s, id: m.d.id });
      }
    });
    return this;
  }

  hardDrop() {
    // Close without a WS close frame is closest to a real socket drop, but
    // `close()` is enough here: the fixture removes the conn either way.
    try {
      this.ws.close();
    } catch {
      /* noop */
    }
  }
}

const out = (o) => process.stdout.write(`${JSON.stringify(o)}\n`);

async function main() {
  const fx = new DiscordFixture({ botToken: TOKEN, heartbeatIntervalMs: 60_000 });
  await fx.start();

  const c1 = await new Client(fx.gatewayUrl, TOKEN).open();
  for (let i = 0; i < 40 && !c1.ready; i++) await sleep(25);
  if (!c1.ready) throw new Error('client never READYed — instrument dead, no conclusion possible');

  // ── KNOWN-POSITIVE: two messages while CONNECTED must be seen. ─────────────
  // Without this, a zero on the gap message is free: a dead client, a wrong
  // URL, a fixture that never dispatches, all produce it.
  fx.dispatchMessage({ id: 'PRE-1', channelId: CHAN, content: 'pre1', authorId: AUTHOR });
  fx.dispatchMessage({ id: 'PRE-2', channelId: CHAN, content: 'pre2', authorId: AUTHOR });
  await sleep(200);
  const preSeen = c1.seen.map((x) => x.id);
  const lastSeq = c1.seq;
  const sessionId = c1.sessionId;

  // ── the disconnect window ─────────────────────────────────────────────────
  c1.hardDrop();
  await sleep(200);
  const liveAfterDrop = fx.conns.size;

  const gapSockets = fx.dispatchMessage({ id: 'GAP-1', channelId: CHAN, content: 'gap1', authorId: AUTHOR });

  // ── RESUME from the last seq the client actually saw ───────────────────────
  const c2 = await new Client(fx.gatewayUrl, TOKEN).open({ resume: { sessionId, seq: lastSeq } });
  for (let i = 0; i < 60 && !c2.resumed; i++) await sleep(25);
  await sleep(200);

  // ── KNOWN-POSITIVE #2: a message after the resume must be seen. ───────────
  // Proves the SECOND client is alive, so "GAP-1 absent" cannot be explained
  // by c2 being dead.
  fx.dispatchMessage({ id: 'POST-1', channelId: CHAN, content: 'post1', authorId: AUTHOR });
  await sleep(250);

  const replayed = c2.seen.map((x) => x.id);
  const seqs = fx.dispatched.map((d) => ({ id: d.id, s: d.s }));
  const collisions = seqs.length - new Set(seqs.map((x) => x.s)).size;

  const gapReplayed = replayed.includes('GAP-1');
  const postSeen = replayed.includes('POST-1');

  out({
    fixture_seq_table: seqs,
    ready_seq_after_identify: 1,
    client1_last_seq: lastSeq,
    client1_saw: preSeen,
    live_conns_after_drop: liveAfterDrop,
    gap_dispatch_reached_sockets: gapSockets,
    client2_resumed: c2.resumed,
    client2_saw: replayed,
    duplicate_seq_numbers_in_fixture_table: collisions,
    KNOWN_POSITIVE_pre_messages_seen: preSeen.length === 2,
    KNOWN_POSITIVE_post_resume_message_seen: postSeen,
    GAP_REPLAYED_ON_RESUME: gapReplayed,
    verdict: gapReplayed
      ? 'FIXTURE CAN express a gap replay'
      : 'FIXTURE CANNOT express a gap replay — INSTRUMENT DEFECT',
  });

  await fx.stop();
  process.exit(gapReplayed ? 0 : 1);
}

main().catch((e) => {
  out({ error: String(e?.stack ?? e) });
  process.exit(2);
});
