#!/usr/bin/env node
// A hermetic Bot Framework, as its own OS process.
//
// WHY ITS OWN PROCESS — measured, not stylistic. The first version of this
// fixture lived inside the driver. It never answered a single request, and the
// binary logged `error sending request for url (.../token)` while the fixture's
// journal was EMPTY, which reads exactly like an adapter that cannot reach its
// token endpoint. The real cause was the driver's own `Atomics.wait` sleep:
// it blocks the main thread, and a blocked main thread means Node's event loop
// never accepts a connection. A fixture that shares a thread with a
// synchronously-sleeping driver is a fixture that is down whenever it matters.
// `f24-llm-fixture.mjs` is a separate process for the same reason.
//
// It serves the four endpoints the msteams adapter reaches:
//   GET  /openid  → { jwks_uri }        (BotFrameworkAuth::fetch_jwks step 1)
//   GET  /keys    → JWKS                (step 2 — the signing keys)
//   POST /token   → an access token     (MsTeamsChannel::start fail-fast)
//   POST /amer/v3/conversations/{id}/activities → the Connector send sink
//
// It holds NO private key. The driver mints the keypairs and hands this process
// the PUBLIC JWKS only, so nothing signable ever reaches the filesystem.
//
// Every request is journalled before it is answered, and fsynced, for the same
// reason the arrivals sink is: a buffered record lost in this process's page
// cache is indistinguishable from a request that was never made — and "the
// adapter never called us" is precisely the finding this file must be able to
// report truthfully.

import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';

function parseArgs(argv) {
  const out = { port: 0, journal: null, jwks: null };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--port') out.port = Number(argv[++i]);
    else if (a === '--journal') out.journal = argv[++i];
    else if (a === '--jwks') out.jwks = argv[++i];
    else {
      process.stderr.write(`f24-msteams-fixture: unknown argument ${a}\n`);
      process.exit(2);
    }
  }
  for (const k of ['journal', 'jwks']) {
    if (!out[k]) {
      process.stderr.write(`f24-msteams-fixture: --${k} is required\n`);
      process.exit(2);
    }
  }
  return out;
}

const args = parseArgs(process.argv.slice(2));
fs.mkdirSync(path.dirname(path.resolve(args.journal)), { recursive: true });
const journalFd = fs.openSync(args.journal, 'a');
const jwks = JSON.parse(fs.readFileSync(args.jwks, 'utf8'));

let seq = 0;
function record(kind, detail) {
  seq += 1;
  const rec = { seq, kind, at: new Date().toISOString(), ...detail };
  fs.writeSync(journalFd, `${JSON.stringify(rec)}\n`);
  fs.fsyncSync(journalFd);
}

const server = http.createServer((req, res) => {
  let body = '';
  req.on('data', (c) => {
    body += c;
  });
  req.on('end', () => {
    const url = new URL(req.url, 'http://127.0.0.1');
    const send = (code, obj) => {
      res.writeHead(code, { 'content-type': 'application/json' });
      res.end(JSON.stringify(obj));
    };

    if (url.pathname === '/_bf/health') return send(200, { ok: true, requests: seq });

    if (url.pathname === '/openid') {
      record('openid', {});
      return send(200, { jwks_uri: `http://127.0.0.1:${server.address().port}/keys` });
    }
    if (url.pathname === '/keys') {
      record('jwks', { kids: jwks.keys.map((k) => k.kid) });
      return send(200, jwks);
    }
    if (url.pathname === '/token') {
      record('token', {});
      return send(200, {
        access_token: 'f24msteams-fixture-connector-token',
        token_type: 'Bearer',
        expires_in: 3600,
      });
    }
    // Connector send sink. The adapter POSTs the bot's reply here, so a record
    // in this journal is the outbound half of the round trip.
    if (url.pathname.includes('/v3/conversations/') && url.pathname.endsWith('/activities')) {
      let parsed = {};
      try {
        parsed = JSON.parse(body);
      } catch {
        /* raw length is recorded regardless */
      }
      record('activity_out', {
        path: url.pathname,
        type: parsed.type ?? null,
        text: parsed.text ?? null,
        raw_len: body.length,
      });
      return send(200, { id: `f24msteams-out-${seq}` });
    }

    record('unknown', { method: req.method, path: url.pathname });
    return send(404, { error: `unknown ${url.pathname}` });
  });
});

server.listen(args.port, '127.0.0.1', () => {
  process.stdout.write(
    `BF_READY url=http://127.0.0.1:${server.address().port} journal=${path.resolve(args.journal)}\n`,
  );
});

process.on('SIGTERM', () => {
  server.close(() => process.exit(0));
});
