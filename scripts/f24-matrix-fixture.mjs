#!/usr/bin/env node
// A Matrix homeserver-shaped endpoint with REAL `/sync` cursor semantics, as
// its own OS process.
//
// WHY THIS EXISTS. `24-C3-FINISH.md` §4b costed matrix as a ZERO-RUST adapter:
// `MatrixConfig.homeserver_url` (config.rs:9) is required, has no
// `#[serde(default)]` and no production constant, `MatrixChannel::new`
// (lib.rs:61) copies it straight into `api_base`, and the registry's
// `make_matrix` (registry:173-184) calls `new()`. That is the SHIPPED
// construction path, not a `#[doc(hidden)]` test constructor — the distinction
// Discord's `with_token_url` proved is the whole game. Nobody had driven it.
//
// WHAT THIS SERVES.
//
//   GET  /_matrix/client/v3/sync?timeout=N[&since=sK]
//        Real cursor semantics, which is the entire point of this fixture:
//          (a) NO `since` -> an INITIAL SYNC. Returns each room's RECENT
//              timeline (the last `--initial-limit` events) plus a
//              `next_batch` cursor. A real homeserver does exactly this: an
//              initial sync is how a client learns the current state of the
//              rooms it is in, and it carries recent history.
//          (b) `since=sK` -> an INCREMENTAL SYNC. Returns only events with
//              seq > K, long-polling up to `timeout` (capped by --max-wait-ms)
//              when there are none.
//        Both forms carry `rooms.join[room].summary."m.joined_member_count"`,
//        because `sync.rs:328-331` maps ONLY the value 2 to `ChatType::Direct`
//        and treats an omitted summary as Group. Every channel config in this
//        matrix sets `group = "disabled"`, so a fixture that omitted the
//        summary would have every message dropped by GROUP policy and the run
//        would read as product inbound loss caused entirely by the fixture.
//
//   PUT  /_matrix/client/v3/rooms/{roomId}/send/m.room.message/{txnId}
//        The reply path (rest.rs:135). Journalled and answered with an
//        `event_id`. This is where the driver's ARRIVALS come from.
//   PUT  /_matrix/client/v3/rooms/{roomId}/typing/{userId}   -> {}
//   PUT  /_matrix/client/v3/rooms/{roomId}/send/m.reaction/{txnId} -> event_id
//
//   POST /__control/submit   inject an inbound event into a room
//   GET  /__control/report   the independent, out-of-process observable
//   GET  /__control/health
//
// THE OBSERVABLE THAT MATTERS, AND WHY IT IS COUNTED HERE.
//
// `sync.rs:190` holds the `since` cursor in a PROCESS-LOCAL variable, and
// `sync.rs:212-226` emits timeline events only when `since` is already set —
// the documented "initial-sync replay guard". Composed, a process restart
// resets `since` to `None`, so the first `/sync` after a restart is an initial
// sync, so its whole timeline is discarded.
//
// That predicts silent loss of everything delivered while the process was
// down. Predicting it is not measuring it, and two hypotheses fit any zero:
//   H1 (product) the restarted process discards the initial sync's timeline;
//   H2 (fixture) this fixture never PUT the gap event in that timeline, so
//                there was nothing to lose.
// So this fixture records, per sync response, whether it was initial and
// EXACTLY WHICH event ids its timeline carried — `initial_syncs[].served`. A
// probe that cannot show the gap event inside an initial sync's `served` list
// must grade INCOMPLETE, not LOSS. H2 is excluded by the fixture's own report
// or it is not excluded at all.
//
// usage: f24-matrix-fixture.mjs --journal <path> --token <access-token>
//                               --room <id>:<member-count> [--room ...]
//                               [--port 0] [--max-wait-ms 2000]
//                               [--initial-limit 20]

import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

function parseArgs(argv) {
  const out = {
    port: 0,
    journal: null,
    token: null,
    maxWaitMs: 2000,
    initialLimit: 20,
    rooms: [],
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--port') out.port = Number(argv[++i]);
    else if (arg === '--journal') out.journal = argv[++i];
    else if (arg === '--token') out.token = argv[++i];
    else if (arg === '--max-wait-ms') out.maxWaitMs = Number(argv[++i]);
    else if (arg === '--initial-limit') out.initialLimit = Number(argv[++i]);
    else if (arg === '--room') {
      // `<roomId>:<memberCount>` — but a Matrix room id is `!local:server`, so
      // split on the LAST colon or a room id would be mangled into nonsense and
      // every event would land in a room the adapter never reports.
      const raw = String(argv[++i]);
      const cut = raw.lastIndexOf(':');
      if (cut <= 0) {
        process.stderr.write(`f24-matrix-fixture: --room needs <roomId>:<members>, got ${raw}\n`);
        process.exit(2);
      }
      out.rooms.push({ id: raw.slice(0, cut), members: Number(raw.slice(cut + 1)) });
    } else {
      process.stderr.write(`f24-matrix-fixture: unknown argument ${arg}\n`);
      process.exit(2);
    }
  }
  if (!out.journal) {
    process.stderr.write('f24-matrix-fixture: --journal is required\n');
    process.exit(2);
  }
  if (!out.token) {
    process.stderr.write('f24-matrix-fixture: --token is required\n');
    process.exit(2);
  }
  if (out.rooms.length === 0) {
    process.stderr.write('f24-matrix-fixture: at least one --room is required\n');
    process.exit(2);
  }
  return out;
}

const args = parseArgs(process.argv.slice(2));
fs.mkdirSync(path.dirname(path.resolve(args.journal)), { recursive: true });
const journalFd = fs.openSync(args.journal, 'a');

let seq = 0;

// Journal BEFORE answering, and fsync. Same discipline as the arrivals sink and
// the Telegram fixture: a record still sitting in this process's page cache when
// the run ends is indistinguishable from an event that never happened.
function record(kind, detail) {
  seq += 1;
  const rec = { seq, kind, at: new Date().toISOString(), ...detail };
  fs.writeSync(journalFd, `${JSON.stringify(rec)}\n`);
  fs.fsyncSync(journalFd);
  return rec;
}

// ── the event log ───────────────────────────────────────────────────────────
// One append-only log across all rooms. `cursor` is the monotonic sequence a
// `next_batch` token encodes. Nothing is ever deleted: unlike Telegram's
// `getUpdates`, a Matrix `/sync` is NOT a destructive read, and modelling it as
// one would fabricate a loss the product does not cause.

let cursor = 0;
/** @type {{cursor:number, room:string, event:object}[]} */
const log = [];

const roomMembers = new Map(args.rooms.map((r) => [r.id, r.members]));

function submitEvent({ room, sender, text, eventId, ts }) {
  if (!roomMembers.has(room)) {
    // Refuse rather than silently inventing a room. An event injected into a
    // room the fixture was never told about would carry no summary, so
    // `sync.rs:328` would type it Group, `group = "disabled"` would drop it,
    // and the leg would report product inbound loss caused by a typo here.
    return { ok: false, error: `unknown room ${room}; declared: ${[...roomMembers.keys()].join(',')}` };
  }
  cursor += 1;
  const event_id = eventId ?? `$f24evt${cursor}`;
  const event = {
    type: 'm.room.message',
    sender,
    event_id,
    origin_server_ts: ts ?? Date.now(),
    content: { msgtype: 'm.text', body: text },
  };
  log.push({ cursor, room, event });
  record('submit', { cursor, room, sender, event_id, text });
  return { ok: true, cursor, event_id };
}

/// Parse a `next_batch` token back into the sequence it encodes. Returns null
/// for anything this fixture did not mint, which is treated as an initial sync
/// — the same way a homeserver treats an unknown token.
function parseSince(since) {
  if (typeof since !== 'string') return null;
  const m = /^s(\d+)$/.exec(since);
  return m ? Number(m[1]) : null;
}

/// Build the `rooms.join` block for a sync response.
///
/// `after === null` means INITIAL SYNC: each room carries its RECENT timeline,
/// capped at `--initial-limit`, which is what a real homeserver returns and is
/// precisely the payload `sync.rs:217` throws away.
function roomsBlock(after) {
  const join = {};
  const served = [];
  for (const [roomId, members] of roomMembers.entries()) {
    const all = log.filter((e) => e.room === roomId);
    const slice =
      after === null ? all.slice(-args.initialLimit) : all.filter((e) => e.cursor > after);
    for (const e of slice) served.push(e.event.event_id);
    join[roomId] = {
      summary: { 'm.joined_member_count': members },
      timeline: { events: slice.map((e) => e.event), limited: false },
    };
  }
  return { join, served };
}

function sendJson(res, obj, status = 200) {
  res.writeHead(status, { 'content-type': 'application/json' });
  res.end(JSON.stringify(obj));
}

// ── observables ─────────────────────────────────────────────────────────────

let syncSeq = 0;
let openSyncs = 0;
let maxConcurrentSyncs = 0;
const concurrencyTrace = [];
/** @type {{sync:number, at:string, served:string[], cursor_at_open:number}[]} */
const initialSyncs = [];
/** @type {{sync:number, initial:boolean, since:string|null, served:string[], at:string}[]} */
const syncs = [];
/** @type {{seq:number, room:string, text:string, txn_id:string, at:string}[]} */
const replies = [];

const server = http.createServer((req, res) => {
  let body = '';
  req.on('data', (c) => {
    body += c;
  });
  req.on('end', async () => {
    const url = new URL(req.url, 'http://127.0.0.1');
    const p = url.pathname;

    // ── control plane (not part of the Matrix surface) ────────────────────
    if (p === '/__control/health') {
      sendJson(res, { ok: true, events: log.length, syncs: syncSeq, cursor });
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
      const r = submitEvent(parsed);
      sendJson(res, r, r.ok ? 200 : 400);
      return;
    }
    if (p === '/__control/report') {
      sendJson(res, {
        ok: true,
        submitted_total: log.length,
        cursor,
        sync_total: syncSeq,
        max_concurrent_sync: maxConcurrentSyncs,
        concurrency_trace: concurrencyTrace,
        // THE H2 EXCLUSION. Each initial sync, with exactly which event ids its
        // timeline carried. A restart probe that cannot point at the gap event
        // inside one of these lists has not excluded "the fixture never served
        // it", and must grade INCOMPLETE rather than LOSS.
        initial_sync_total: initialSyncs.length,
        initial_syncs: initialSyncs,
        syncs,
        replies,
        rooms: [...roomMembers.entries()].map(([id, members]) => ({ id, members })),
      });
      return;
    }

    // ── auth ──────────────────────────────────────────────────────────────
    const auth = req.headers.authorization ?? '';
    if (auth !== `Bearer ${args.token}`) {
      // Answered the way a homeserver answers one, so a misconfigured run fails
      // as auth rather than as silence.
      record('bad_token', { path: p, auth_len: auth.length });
      sendJson(res, { errcode: 'M_UNKNOWN_TOKEN', error: 'Invalid access token' }, 401);
      return;
    }

    // ── the send path (rest.rs:135) ───────────────────────────────────────
    const send = /^\/_matrix\/client\/v3\/rooms\/([^/]+)\/send\/m\.room\.message\/([^/]+)$/.exec(p);
    if (send && req.method === 'PUT') {
      const room = decodeURIComponent(send[1]);
      const txnId = decodeURIComponent(send[2]);
      let parsed;
      try {
        parsed = JSON.parse(body);
      } catch {
        parsed = {};
      }
      cursor += 1;
      const eventId = `$f24reply${cursor}`;
      const rec = record('sendMessage', {
        room,
        txn_id: txnId,
        text: String(parsed.body ?? ''),
        event_id: eventId,
      });
      replies.push({ seq: rec.seq, room, text: rec.text, txn_id: txnId, at: rec.at });
      sendJson(res, { event_id: eventId });
      return;
    }

    const reaction = /^\/_matrix\/client\/v3\/rooms\/([^/]+)\/send\/m\.reaction\/([^/]+)$/.exec(p);
    if (reaction && req.method === 'PUT') {
      cursor += 1;
      record('sendReaction', { room: decodeURIComponent(reaction[1]) });
      sendJson(res, { event_id: `$f24react${cursor}` });
      return;
    }

    const typing = /^\/_matrix\/client\/v3\/rooms\/([^/]+)\/typing\/([^/]+)$/.exec(p);
    if (typing && req.method === 'PUT') {
      record('typing', { room: decodeURIComponent(typing[1]) });
      sendJson(res, {});
      return;
    }

    // ── /sync ─────────────────────────────────────────────────────────────
    if (p === '/_matrix/client/v3/sync') {
      syncSeq += 1;
      const syncId = syncSeq;
      openSyncs += 1;
      if (openSyncs > maxConcurrentSyncs) maxConcurrentSyncs = openSyncs;
      concurrencyTrace.push({ at: new Date().toISOString(), open: openSyncs, sync: syncId });

      const sinceRaw = url.searchParams.get('since');
      const after = parseSince(sinceRaw);
      const isInitial = after === null;
      const qTimeout = Number(url.searchParams.get('timeout'));
      const timeoutMs = Math.min(Number.isFinite(qTimeout) ? qTimeout : 0, args.maxWaitMs);

      record('sync.open', {
        sync: syncId,
        since: sinceRaw,
        initial: isInitial,
        timeout_ms: timeoutMs,
        open: openSyncs,
        cursor_at_open: cursor,
      });

      // An INITIAL sync returns immediately with the recent timeline — a real
      // homeserver does not long-poll a client that has no cursor. An
      // INCREMENTAL sync long-polls until something is newer than the cursor.
      let block = roomsBlock(after);
      if (!isInitial) {
        const deadline = Date.now() + timeoutMs;
        while (block.served.length === 0 && Date.now() < deadline) {
          await new Promise((r) => setTimeout(r, 25));
          block = roomsBlock(after);
        }
      }

      const nextBatch = `s${cursor}`;
      const entry = {
        sync: syncId,
        initial: isInitial,
        since: sinceRaw,
        served: block.served,
        next_batch: nextBatch,
        at: new Date().toISOString(),
      };
      syncs.push(entry);
      if (isInitial) {
        initialSyncs.push({
          sync: syncId,
          at: entry.at,
          served: block.served,
          cursor_at_open: cursor,
        });
      }
      record('sync.close', {
        sync: syncId,
        initial: isInitial,
        served: block.served,
        next_batch: nextBatch,
      });

      openSyncs -= 1;
      concurrencyTrace.push({ at: new Date().toISOString(), open: openSyncs, sync: syncId });
      // `rooms.join`, NOT `rooms`. The first draft of this fixture sent
      // `{ rooms: block.join }`, which puts the room map one level too high.
      // `sync.rs:61-65` deserialises `Rooms { #[serde(default)] join }`, so the
      // missing `join` key would default to an EMPTY map, `parse_sync_events`
      // would iterate nothing, and every matrix leg would have reported ZERO
      // ARRIVALS — a fabricated product defect caused entirely by this line.
      // Caught by the self-test (M2), not by the live run.
      sendJson(res, { next_batch: nextBatch, rooms: { join: block.join } });
      return;
    }

    record('unknown_endpoint', { path: p, method: req.method });
    sendJson(res, { errcode: 'M_UNRECOGNIZED', error: `unknown ${p}` }, 404);
  });
});

server.listen(args.port, '127.0.0.1', () => {
  const bound = server.address();
  process.stdout.write(
    `MXFIX_READY url=http://127.0.0.1:${bound.port} journal=${path.resolve(args.journal)}\n`,
  );
});

for (const sig of ['SIGINT', 'SIGTERM']) {
  process.on(sig, () => {
    record('shutdown', { signal: sig });
    server.close(() => process.exit(0));
    setTimeout(() => process.exit(0), 500);
  });
}
