#!/usr/bin/env node
// matrix-live-proxy.mjs — a plain-HTTP front door for a real Matrix homeserver,
// so a live run can (a) SEE the transaction ids the product puts on the wire and
// (b) induce a genuine OUTCOME-UNKNOWN send without faking anything.
//
// ── why this exists ─────────────────────────────────────────────────────────
//
// `docs/delivery-semantics.md` puts Matrix in the exactly-once column because
// `rest.rs:63 txn_id_for_key` derives the `{txnId}` path segment from the
// gateway's delivery key, so a replay after a restart carries the SAME id and
// the homeserver collapses it. Proving that live needs two things a direct
// HTTPS connection to matrix.org cannot give you:
//
//   1. an INDEPENDENT record of the txn id on the wire, in another OS process,
//      rather than the product's own log line about itself;
//   2. a send whose outcome is genuinely UNKNOWN TO THE CLIENT while the event
//      really did land at the homeserver — which is the only state that puts
//      the delivery spine's `Attempted` arm (`automation.rs:201-220`) in play.
//
// (2) is produced by forwarding the request upstream for real, reading the real
// response, and then never writing it back. The event exists at matrix.org; the
// product cannot know that. Killing the gateway there is a true reproduction of
// the crash-mid-send case, not a simulation of one.
//
// ── secret discipline ───────────────────────────────────────────────────────
//
// The `Authorization` header is forwarded and NEVER recorded. `redactHeaders`
// drops it before anything is written, and `--selftest` asserts that a header
// carrying a known token value cannot reach the journal.
//
// ── control plane is a FILE, not a port ─────────────────────────────────────
//
// The driver flips stalling on and off by creating/removing `--stall-file`.
// A file rather than a control port because the driver `kill -9`s the client
// mid-request and then restarts it; a control port adds a second liveness
// question to a run whose whole subject is liveness.
//
// usage:
//   node scripts/matrix-live-proxy.mjs --selftest
//   node scripts/matrix-live-proxy.mjs --port N --upstream https://matrix.org \
//        --journal /path/wire.jsonl --stall-file /path/STALL
// prints: MXPROXY_READY port=<port> upstream=<url>

import http from 'node:http';
import https from 'node:https';
import fs from 'node:fs';
import { URL } from 'node:url';

// ---------------------------------------------------------------------------
// pure helpers (self-tested)
// ---------------------------------------------------------------------------

/** Strip every header that could carry a credential. */
export function redactHeaders(h) {
  const out = {};
  for (const [k, v] of Object.entries(h || {})) {
    const lk = k.toLowerCase();
    if (lk === 'authorization' || lk === 'cookie' || lk === 'proxy-authorization') continue;
    out[lk] = v;
  }
  return out;
}

/**
 * The `{txnId}` path segment of a Matrix send/redact PUT, or null.
 *
 * Two shapes, and they are NOT the same position:
 *   /_matrix/client/v3/rooms/{room}/send/{eventType}/{txnId}
 *   /_matrix/client/v3/rooms/{room}/redact/{eventId}/{txnId}
 * A single "last segment" rule would silently conflate them the day a new
 * route appears, so each is matched explicitly.
 */
export function txnIdOf(pathname) {
  const send = /^\/_matrix\/client\/v3\/rooms\/([^/]+)\/send\/([^/]+)\/([^/?]+)$/.exec(pathname);
  if (send) return { kind: 'send', room: send[1], eventType: send[2], txnId: send[3] };
  const red = /^\/_matrix\/client\/v3\/rooms\/([^/]+)\/redact\/([^/]+)\/([^/?]+)$/.exec(pathname);
  if (red) return { kind: 'redact', room: red[1], eventId: red[2], txnId: red[3] };
  return null;
}

/** Whether this request is the one the stall applies to. */
export function isStallTarget(method, pathname) {
  const t = txnIdOf(pathname);
  return method === 'PUT' && !!t && t.kind === 'send';
}

/** Bodies are recorded only for the small, non-secret control responses. */
export function recordableBody(pathname, body) {
  const t = txnIdOf(pathname);
  if (!t) return null;
  return String(body).slice(0, 400);
}

// ---------------------------------------------------------------------------
// self-test — three assertions per §6b-ii: known-positive, known-negative, and
// "the naive version would have missed it".
// ---------------------------------------------------------------------------

function selftest() {
  let pass = 0;
  let fail = 0;
  const t = (name, cond) => {
    if (cond) { pass++; console.log(`  ok   ${name}`); }
    else { fail++; console.log(`  FAIL ${name}`); }
  };

  const SECRET = 'syt_THIS_IS_A_KNOWN_POSITIVE_TOKEN_VALUE';
  const red = redactHeaders({ Authorization: `Bearer ${SECRET}`, 'Content-Type': 'application/json' });
  t('redact drops authorization', !('authorization' in red));
  t('redact keeps content-type (known-positive: it is not dropping everything)',
    red['content-type'] === 'application/json');
  t('no serialization of a redacted header set contains the token',
    !JSON.stringify(red).includes(SECRET));
  // the naive version: JSON.stringify(headers) with no redaction — must contain it
  t('the UNREDACTED header set WOULD have leaked it (the repair does something)',
    JSON.stringify({ Authorization: `Bearer ${SECRET}` }).includes(SECRET));

  const sendPath = '/_matrix/client/v3/rooms/%21abc%3Amatrix.org/send/m.room.message/cron:job-a:1785121776528';
  const s = txnIdOf(sendPath);
  t('send txnId extracted', s && s.kind === 'send' && s.txnId === 'cron:job-a:1785121776528');
  const redactPath = '/_matrix/client/v3/rooms/%21abc%3Amatrix.org/redact/%24evt1/wl-u1234-5';
  const r = txnIdOf(redactPath);
  t('redact txnId extracted and NOT confused with the event id',
    r && r.kind === 'redact' && r.txnId === 'wl-u1234-5' && r.eventId === '%24evt1');
  t('known-negative: /sync is not a txn route', txnIdOf('/_matrix/client/v3/sync?timeout=30000') === null);
  t('known-negative: a truncated send path yields null',
    txnIdOf('/_matrix/client/v3/rooms/%21abc/send/m.room.message') === null);

  t('stall targets a send PUT', isStallTarget('PUT', sendPath));
  t('stall does NOT target a redact PUT', !isStallTarget('PUT', redactPath));
  t('stall does NOT target a GET /sync', !isStallTarget('GET', '/_matrix/client/v3/sync'));

  console.log(`SELFTEST ${fail === 0 ? 'PASS' : 'FAIL'} passed=${pass} failed=${fail}`);
  process.exit(fail === 0 ? 0 : 1);
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

const argv = process.argv.slice(2);
if (argv.includes('--selftest')) selftest();

const args = { port: 0, upstream: 'https://matrix.org', journal: null, stallFile: null };
for (let i = 0; i < argv.length; i++) {
  if (argv[i] === '--port') args.port = Number(argv[++i]);
  else if (argv[i] === '--upstream') args.upstream = argv[++i];
  else if (argv[i] === '--journal') args.journal = argv[++i];
  else if (argv[i] === '--stall-file') args.stallFile = argv[++i];
}
if (!args.journal) { process.stderr.write('--journal is required\n'); process.exit(2); }

const up = new URL(args.upstream);
const stalled = [];
let seq = 0;

function record(rec) {
  fs.appendFileSync(args.journal, JSON.stringify({ ts: new Date().toISOString(), ...rec }) + '\n');
}

const server = http.createServer((req, res) => {
  const n = ++seq;
  const u = new URL(req.url, 'http://x');
  const t = txnIdOf(u.pathname);
  const chunks = [];
  req.on('data', (c) => chunks.push(c));
  req.on('end', () => {
    const body = Buffer.concat(chunks);
    const willStall = !!args.stallFile && fs.existsSync(args.stallFile) && isStallTarget(req.method, u.pathname);

    record({
      ev: 'request', n, method: req.method, path: u.pathname,
      txn: t ? t.txnId : null, route: t ? t.kind : null,
      body_bytes: body.length, will_stall: willStall,
      headers: Object.keys(redactHeaders(req.headers)).sort(),
    });

    const headers = { ...req.headers, host: up.host };
    delete headers['content-length'];
    if (body.length) headers['content-length'] = String(body.length);

    const outReq = https.request(
      { hostname: up.hostname, port: up.port || 443, path: req.url, method: req.method, headers },
      (outRes) => {
        const rc = [];
        outRes.on('data', (c) => rc.push(c));
        outRes.on('end', () => {
          const rbody = Buffer.concat(rc);
          record({
            ev: 'upstream_response', n, method: req.method, path: u.pathname,
            txn: t ? t.txnId : null, status: outRes.statusCode,
            resp_bytes: rbody.length,
            resp_body: recordableBody(u.pathname, rbody.toString('utf8')),
            stalled: willStall,
          });
          if (willStall) {
            // The event is AT the homeserver. The client will never learn that.
            stalled.push(res);
            record({ ev: 'stalled_response_withheld', n, txn: t ? t.txnId : null });
            return;
          }
          res.writeHead(outRes.statusCode, redactHeaders(outRes.headers));
          res.end(rbody);
        });
      },
    );
    outReq.on('error', (e) => {
      record({ ev: 'upstream_error', n, path: u.pathname, error: String(e.message) });
      if (!res.headersSent) res.writeHead(502, { 'content-type': 'application/json' });
      res.end('{"errcode":"M_PROXY_UPSTREAM"}');
    });
    // /sync long-polls; do not impose a shorter deadline than the product's.
    outReq.setTimeout(120000, () => outReq.destroy(new Error('upstream timeout')));
    if (body.length) outReq.write(body);
    outReq.end();
  });
});

server.keepAliveTimeout = 120000;
server.headersTimeout = 125000;
server.requestTimeout = 0;
server.listen(args.port, '127.0.0.1', () => {
  const p = server.address().port;
  record({ ev: 'ready', port: p, upstream: args.upstream });
  console.log(`MXPROXY_READY port=${p} upstream=${args.upstream}`);
});

process.on('SIGTERM', () => { record({ ev: 'shutdown', stalled: stalled.length }); process.exit(0); });
