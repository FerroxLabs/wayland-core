#!/usr/bin/env node
// The independent delivery destination for the Phase 24 setup-to-recovery
// journey, as its OWN OS process.
//
// WHY THIS EXISTS IN NODE, when `wayland-channel-sink` already exists in Rust.
// The journey has to run identically on macOS, Linux and Windows, and this
// program's standing constraint is that Cargo may not run on the macOS host.
// Building the Rust sink for macOS is therefore not available at journey time,
// which would leave two platforms measured with one instrument and the third
// measured with another. An instrument that differs per platform is a confound
// in exactly the comparison the criterion is asking about, so the journey uses
// ONE instrument everywhere and it is this one. The journal record shape is
// byte-compatible with `wcore_eval_scenarios::fixtures::channel::Arrival`, so
// the Rust tally reader can still read a journal this process wrote.
//
// WHAT "INDEPENDENT" MEANS HERE, precisely, because the word is load-bearing:
// this is a separate operating-system process from the gateway. The gateway did
// not start it, cannot restart it, cannot write to its journal, and does not
// survive in it. The gateway's ONLY way to add a line to the arrivals journal is
// to complete a real TCP round trip to a listener it does not own. It outlives
// the gateway's hard kill and the platform's restart of it, which is the whole
// window the delivery reconciliation is computed over. It is NOT the gateway's
// own delivery ledger, and that is the distinction the criterion turns on.
//
// It is not a general-purpose Slack mock: it serves exactly the three endpoints
// the shipped Slack adapter calls, and it never journals a bearer token.

import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';

function parseArgs(argv) {
  const out = { port: 0, journal: null, stallAfter: null };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--port') out.port = Number(argv[++i]);
    else if (arg === '--journal') out.journal = argv[++i];
    else if (arg === '--stall-after') out.stallAfter = Number(argv[++i]);
    else {
      process.stderr.write(`f24-sink: unknown argument ${arg}\n`);
      process.exit(2);
    }
  }
  if (!out.journal) {
    process.stderr.write('f24-sink: --journal is required\n');
    process.exit(2);
  }
  return out;
}

const args = parseArgs(process.argv.slice(2));
fs.mkdirSync(path.dirname(path.resolve(args.journal)), { recursive: true });
const journalFd = fs.openSync(args.journal, 'a');

let seq = 0;
// Idempotency-Key -> the message identity created for it. Populated when the
// arrival is JOURNALLED, not when it is answered: a stalled arrival is a
// message the destination holds that the sender never heard about, so a replay
// of its key is still a replay of something already here.
const served = new Map();

function fingerprint(auth) {
  const token = auth.startsWith('Bearer ') ? auth.slice(7) : auth;
  if (!token) return 'none';
  const digest = crypto.createHash('sha256').update(token).digest('hex');
  return `sha256:${digest}`.slice(0, 19);
}

// Journal BEFORE answering, and fsync. A buffered arrival lost in this process's
// page cache would be indistinguishable from a delivery that never happened,
// which is the one confusion the whole count exists to rule out.
function record(endpoint, conversationId, text, auth, answered, key, suppressed) {
  seq += 1;
  const arrival = {
    seq,
    ts: `${seq}.000000`,
    endpoint,
    conversation_id: conversationId,
    text,
    auth_fingerprint: fingerprint(auth),
    answered,
    idempotency_key: key ?? null,
    suppressed,
    at: new Date().toISOString(),
  };
  if (!suppressed && key) served.set(key, arrival.ts);
  fs.writeSync(journalFd, `${JSON.stringify(arrival)}\n`);
  fs.fsyncSync(journalFd);
  return arrival;
}

function json(res, status, body) {
  const payload = JSON.stringify(body);
  res.writeHead(status, { 'content-type': 'application/json' });
  res.end(payload);
}

const server = http.createServer((req, res) => {
  let body = '';
  req.on('data', (chunk) => {
    body += chunk;
  });
  req.on('end', () => {
    const url = new URL(req.url, 'http://127.0.0.1');
    const auth = req.headers.authorization ?? '';

    if (url.pathname === '/api/auth.test') {
      json(res, 200, { ok: true, url: 'http://127.0.0.1/', team: 'f24-fixture', user: 'f24-bot' });
      return;
    }

    if (url.pathname === '/_sink/health') {
      json(res, 200, { ok: true, arrivals: seq });
      return;
    }

    if (url.pathname === '/api/reactions.add') {
      json(res, 200, { ok: true });
      return;
    }

    if (url.pathname === '/api/chat.postMessage') {
      let parsed;
      try {
        parsed = JSON.parse(body);
      } catch {
        parsed = { channel: '', text: body };
      }
      const channel = parsed.channel ?? '';
      const text = parsed.text ?? '';
      const key = req.headers['idempotency-key'] ?? null;

      if (key && served.has(key)) {
        // The replay REACHED the destination and was absorbed there. It is
        // journalled as suppressed — hiding it would make the suppression
        // unfalsifiable — but it did not become a second message.
        record('chat.postMessage', channel, text, auth, true, key, true);
        json(res, 200, { ok: true, ts: served.get(key), channel });
        return;
      }

      const willAnswer = args.stallAfter === null || seq < args.stallAfter;
      const arrival = record('chat.postMessage', channel, text, auth, willAnswer, key, false);
      if (!willAnswer) {
        // Accept it, journal it, never answer it. This is the only way to place
        // a delivery in the sender's outcome-unknown class from outside the
        // sender's own process.
        return;
      }
      json(res, 200, { ok: true, ts: arrival.ts, channel });
      return;
    }

    // ── ADDITIVE (lane 24-c3): the outbound endpoints of the OTHER two
    // webhook-driven connectors. Purely additive — the journey never calls
    // these, so no count it takes can change. They exist because the inbound
    // matrix needs the SAME independent journal to be the arrival source for
    // every adapter it measures; measuring one adapter at this sink and
    // another somewhere else would make the per-adapter numbers
    // incomparable, which is the confound the criterion is asking about.
    //
    // Each records into the identical `Arrival` shape, so one tally reads all
    // of them and the `endpoint` field is what separates the adapters.

    // WhatsApp Cloud API: POST {base}/{graph_version}/{phone_number_id}/messages
    if (/^\/[^/]+\/[^/]+\/messages$/.test(url.pathname)) {
      let parsed;
      try {
        parsed = JSON.parse(body);
      } catch {
        parsed = {};
      }
      const to = parsed.to ?? '';
      const text = parsed.text?.body ?? '';
      const arrival = record('whatsapp.messages', to, text, auth, true, null, false);
      json(res, 200, {
        messaging_product: 'whatsapp',
        contacts: [{ input: to, wa_id: to }],
        messages: [{ id: `wamid.f24c3-${arrival.seq}` }],
      });
      return;
    }

    // Twilio: POST /2010-04-01/Accounts/<sid>/Messages.json (form-encoded)
    if (/^\/2010-04-01\/Accounts\/[^/]+\/Messages\.json$/.test(url.pathname)) {
      const form = new URLSearchParams(body);
      const to = form.get('To') ?? '';
      const text = form.get('Body') ?? '';
      // Twilio authenticates with HTTP Basic, so the token rides in the same
      // Authorization header the fingerprint already digests. It is never
      // journalled in the clear, same as the Slack bearer.
      const arrival = record('twilio.messages', to, text, auth, true, null, false);
      json(res, 201, {
        sid: `SMf24c3${String(arrival.seq).padStart(26, '0')}`,
        status: 'queued',
        to,
        body: text,
      });
      return;
    }

    json(res, 404, { ok: false, error: 'unknown_endpoint' });
  });
});

server.listen(args.port, '127.0.0.1', () => {
  const bound = server.address();
  // Printed and flushed so the driver can read the bound URL BEFORE it starts
  // the gateway. A gateway started against an unbound port fails its sends for
  // a reason that looks exactly like a product defect.
  process.stdout.write(
    `SINK_READY url=http://127.0.0.1:${bound.port} journal=${path.resolve(args.journal)}\n`,
  );
});

process.on('SIGTERM', () => {
  server.close(() => process.exit(0));
});
