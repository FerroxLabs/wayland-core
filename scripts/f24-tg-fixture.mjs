#!/usr/bin/env node
// A Telegram Bot API-shaped endpoint that CONSUMES ON READ, as its own OS
// process.
//
// WHY THIS EXISTS. F24-C3-H2 proved that `gateway run` registers and starts TWO
// `ChannelManager`s. For the three WEBHOOK adapters that is merely wasteful:
// an inbound POST is routed to whichever manager the webhook host holds, and
// the other manager never sees it. Its report said plainly that the POLLING
// half was a different question and that it could not answer it, because the
// polling adapters are exactly the ones with no fixture seam.
//
// This is that seam. Polling adapters DESTROY what they read:
//
//   * Telegram `getUpdates?offset=N` permanently deletes every update with
//     `update_id < N`. That is the documented confirm mechanism, and it is
//     why the bot API tolerates only one poller per token.
//   * IMAP `FETCH` sets `\Seen`.
//   * The Discord gateway holds one session per token.
//
// So a second manager polling the same account is not a duplicate — it is a
// COMPETITOR, and anything it wins is dropped on the floor because it has no
// subscriber attached. The failure is silent: no error, no log, no retry, the
// message is simply gone.
//
// WHAT THIS SERVES, AND HOW FAITHFULLY.
//
//   POST/GET /bot<token>/deleteWebhook  -> ok:true          (start() calls it)
//   GET/POST /bot<token>/getUpdates     -> Telegram's real offset semantics:
//        (a) offset > 0 CONFIRMS: every pending update with id < offset is
//            deleted and can never be served again, to anyone;
//        (b) the response carries every remaining pending update with
//            id >= offset;
//        (c) an empty result long-polls up to `timeout` seconds (capped here,
//            see --max-wait-ms) rather than returning immediately, which is
//            what makes two concurrent pollers observable as two concurrently
//            OPEN requests.
//   POST     /bot<token>/sendMessage    -> journalled and answered ok:true.
//
// WHAT IT DELIBERATELY DOES NOT DO. Real Telegram answers a second concurrent
// `getUpdates` on the same token with `409 Conflict: terminated by other
// getUpdates request`. This fixture does NOT 409, and that choice is
// conservative on purpose: 409ing would make the second poller fail loudly,
// which is the EASY failure. Serving both is the harder, quieter case, and it
// is the one that produces silent loss rather than a visible error. A fixture
// that only reproduces the loud failure would let the quiet one through.
//
// THE INDEPENDENT OBSERVABLE. `max_concurrent_getupdates` is counted here, in
// another process, from overlapping open requests. It is not a log line the
// binary prints about itself, and it cannot be satisfied by a status string:
// two managers polling one token show up as 2, one manager as 1, and a runtime
// that polls NOTHING shows up as 0 — which is a distinct, failing answer, so a
// fix that works by making nothing start cannot pass.
//
// usage: f24-tg-fixture.mjs --token <bot-token> --journal <path> [--port 0]
//                           [--max-wait-ms 2000]

import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

function parseArgs(argv) {
  const out = { port: 0, journal: null, token: null, maxWaitMs: 2000 };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--port') out.port = Number(argv[++i]);
    else if (arg === '--journal') out.journal = argv[++i];
    else if (arg === '--token') out.token = argv[++i];
    else if (arg === '--max-wait-ms') out.maxWaitMs = Number(argv[++i]);
    else {
      process.stderr.write(`f24-tg-fixture: unknown argument ${arg}\n`);
      process.exit(2);
    }
  }
  if (!out.journal) {
    process.stderr.write('f24-tg-fixture: --journal is required\n');
    process.exit(2);
  }
  if (!out.token) {
    process.stderr.write('f24-tg-fixture: --token is required\n');
    process.exit(2);
  }
  return out;
}

const args = parseArgs(process.argv.slice(2));
fs.mkdirSync(path.dirname(path.resolve(args.journal)), { recursive: true });
const journalFd = fs.openSync(args.journal, 'a');

let seq = 0;

// Journal BEFORE answering, and fsync. Same discipline as the arrivals sink and
// for the same reason: a record still sitting in this process's page cache when
// the run ends is indistinguishable from an event that never happened.
function record(kind, detail) {
  seq += 1;
  const rec = { seq, kind, at: new Date().toISOString(), ...detail };
  fs.writeSync(journalFd, `${JSON.stringify(rec)}\n`);
  fs.fsyncSync(journalFd);
  return rec;
}

// ── the update queue ────────────────────────────────────────────────────────
// `pending` holds updates that have been submitted and NOT yet confirmed away.
// `deleted` records which poll confirmed each one, so the report can say who
// consumed what rather than only that something went missing.

let nextUpdateId = 1;
/** @type {{update_id:number, token:string, body:object}[]} */
let pending = [];
/** @type {Map<number,{token:string, deleted_by:number, served_to:number[]}>} */
const history = new Map();

let pollSeq = 0;
let openPolls = 0;
let maxConcurrentPolls = 0;
// Every distinct moment at which the number of simultaneously-open getUpdates
// requests changed, so a spike is legible after the fact rather than only as a
// single maximum.
const concurrencyTrace = [];

function submitUpdate({ token, chatId, senderId, username, text }) {
  const update_id = nextUpdateId;
  nextUpdateId += 1;
  const body = {
    update_id,
    message: {
      message_id: update_id,
      date: Math.floor(Date.now() / 1000),
      chat: { id: Number(chatId), type: 'private' },
      from: { id: Number(senderId), is_bot: false, first_name: username, username },
      text,
    },
  };
  pending.push({ update_id, token, body });
  history.set(update_id, { token, deleted_by: null, served_to: [] });
  record('submit', { update_id, token, chat_id: chatId, sender_id: senderId, text });
  return update_id;
}

/**
 * Telegram's confirm: `offset = N` deletes every pending update with id < N.
 * Returns the ids that were destroyed by THIS call.
 */
function confirm(offset, pollId) {
  if (!Number.isFinite(offset) || offset <= 0) return [];
  const gone = pending.filter((u) => u.update_id < offset).map((u) => u.update_id);
  if (gone.length > 0) {
    pending = pending.filter((u) => u.update_id >= offset);
    for (const id of gone) {
      const h = history.get(id);
      if (h && h.deleted_by === null) h.deleted_by = pollId;
    }
    record('confirm', { poll: pollId, offset, deleted: gone });
  }
  return gone;
}

function servableFor(offset) {
  const from = Number.isFinite(offset) && offset > 0 ? offset : 0;
  return pending.filter((u) => u.update_id >= from);
}

function sendJson(res, obj, status = 200) {
  const payload = JSON.stringify(obj);
  res.writeHead(status, { 'content-type': 'application/json' });
  res.end(payload);
}

const server = http.createServer((req, res) => {
  let body = '';
  req.on('data', (c) => {
    body += c;
  });
  req.on('end', async () => {
    const url = new URL(req.url, 'http://127.0.0.1');
    const p = url.pathname;

    // ── control plane (not part of the Telegram surface) ──────────────────
    if (p === '/__control/health') {
      sendJson(res, { ok: true, pending: pending.length, polls: pollSeq });
      return;
    }
    if (p === '/__control/submit') {
      let parsed;
      try {
        parsed = JSON.parse(body);
      } catch {
        sendJson(res, { ok: false, error: 'bad json' }, 400);
        return;
      }
      const id = submitUpdate(parsed);
      sendJson(res, { ok: true, update_id: id });
      return;
    }
    if (p === '/__control/report') {
      const served = [];
      for (const [id, h] of history.entries()) {
        served.push({
          update_id: id,
          token: h.token,
          served_to: h.served_to,
          serve_count: h.served_to.length,
          deleted_by: h.deleted_by,
        });
      }
      sendJson(res, {
        ok: true,
        submitted_total: history.size,
        still_pending: pending.map((u) => u.update_id),
        max_concurrent_getupdates: maxConcurrentPolls,
        concurrency_trace: concurrencyTrace,
        poll_total: pollSeq,
        updates: served,
        replies,
      });
      return;
    }

    // ── the Telegram surface ──────────────────────────────────────────────
    const m = /^\/bot([^/]+)\/(\w+)$/.exec(p);
    if (!m) {
      record('unknown_endpoint', { path: p });
      sendJson(res, { ok: false, error_code: 404, description: `unknown ${p}` }, 404);
      return;
    }
    const [, token, method] = m;
    if (token !== args.token) {
      // A wrong token is answered the way Telegram answers one, so a
      // misconfigured run fails as auth rather than as silence.
      record('bad_token', { method, token_len: token.length });
      sendJson(res, { ok: false, error_code: 401, description: 'Unauthorized' }, 401);
      return;
    }

    if (method === 'deleteWebhook' || method === 'getMe' || method === 'setMyCommands') {
      record(method, {});
      sendJson(res, { ok: true, result: true });
      return;
    }

    if (method === 'sendMessage') {
      let parsed;
      try {
        parsed = JSON.parse(body);
      } catch {
        parsed = {};
      }
      const rec = record('sendMessage', {
        chat_id: String(parsed.chat_id ?? ''),
        text: String(parsed.text ?? ''),
      });
      replies.push({ seq: rec.seq, chat_id: rec.chat_id, text: rec.text, at: rec.at });
      sendJson(res, {
        ok: true,
        result: {
          message_id: 900000 + replies.length,
          date: Math.floor(Date.now() / 1000),
          chat: { id: Number(parsed.chat_id ?? 0) },
        },
      });
      return;
    }

    if (method !== 'getUpdates') {
      record('unhandled_method', { method });
      sendJson(res, { ok: true, result: [] });
      return;
    }

    // ── getUpdates ────────────────────────────────────────────────────────
    pollSeq += 1;
    const pollId = pollSeq;
    openPolls += 1;
    if (openPolls > maxConcurrentPolls) maxConcurrentPolls = openPolls;
    concurrencyTrace.push({ at: new Date().toISOString(), open: openPolls, poll: pollId });

    // The adapter sends offset/timeout in the query string; tolerate a JSON
    // body too so a future change of transport does not silently read 0 and
    // make every confirm a no-op (which would hide the very race we measure).
    let qOffset = Number(url.searchParams.get('offset'));
    let qTimeout = Number(url.searchParams.get('timeout'));
    if (!Number.isFinite(qOffset) || url.searchParams.get('offset') === null) {
      try {
        const j = JSON.parse(body);
        qOffset = Number(j.offset);
        qTimeout = Number(j.timeout);
      } catch {
        /* query-string form is the normal case */
      }
    }
    const offset = Number.isFinite(qOffset) ? qOffset : 0;
    const timeoutMs = Math.min(
      Number.isFinite(qTimeout) ? qTimeout * 1000 : 0,
      args.maxWaitMs,
    );

    record('getUpdates.open', { poll: pollId, offset, timeout_ms: timeoutMs, open: openPolls });

    const deleted = confirm(offset, pollId);

    const deadline = Date.now() + timeoutMs;
    let out = servableFor(offset);
    // Long-poll. Both pollers are served the same pending set — see the header
    // comment on why this fixture does not 409 the second one.
    while (out.length === 0 && Date.now() < deadline) {
      await new Promise((r) => setTimeout(r, 25));
      out = servableFor(offset);
    }

    for (const u of out) {
      const h = history.get(u.update_id);
      if (h) h.served_to.push(pollId);
    }
    record('getUpdates.close', {
      poll: pollId,
      offset,
      deleted,
      served: out.map((u) => u.update_id),
      open_before_close: openPolls,
    });

    openPolls -= 1;
    concurrencyTrace.push({ at: new Date().toISOString(), open: openPolls, poll: pollId });
    sendJson(res, { ok: true, result: out.map((u) => u.body) });
  });
});

/** @type {{seq:number, chat_id:string, text:string, at:string}[]} */
const replies = [];

server.listen(args.port, '127.0.0.1', () => {
  const bound = server.address();
  process.stdout.write(
    `TGFIX_READY url=http://127.0.0.1:${bound.port} journal=${path.resolve(args.journal)}\n`,
  );
});

for (const sig of ['SIGINT', 'SIGTERM']) {
  process.on(sig, () => {
    record('shutdown', { signal: sig });
    server.close(() => process.exit(0));
    setTimeout(() => process.exit(0), 500);
  });
}
