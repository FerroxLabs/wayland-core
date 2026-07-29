#!/usr/bin/env node
// F24 KILL PROXY — make an upstream VANISH, as its own OS process.
//
// A TCP proxy in front of a fixture so a driver can induce a genuine upstream
// disappearance without touching the fixture: destroy every live connection and
// refuse new ones, then restore.
//
// ── WHY THIS IS A SEPARATE PROCESS, and it is not a style choice ────────────
//
// Every driver in this program sleeps with `Atomics.wait`, which blocks the
// WHOLE Node event loop. A proxy living inside the driver therefore cannot
// accept or forward a single byte while the driver waits — and the driver waits
// almost all the time.
//
// This is written from a measurement, not from the doc comment that already
// warned about it. The first version of `f24-reconnect-poll.mjs` embedded the
// proxy, and the run reported:
//
//     the binary never established a /sync loop (sync_total=0) — NOT MEASURED
//     WARN /sync failed; backing off
//          error=network: error sending request for url (http://127.0.0.1:41743/…)
//
// which reads as "the matrix adapter cannot reach its homeserver" — a PRODUCT
// defect. The truth was that the instrument was not listening. `f24-discord-
// inbound.mjs:55-62` documents this exact failure ("that is exactly how this
// driver's first two runs failed"), and it recurred anyway, in a new file, to a
// reader who had already read the warning. Which is the point LANE-BRIEF
// §6b-ii makes: a documented instrument defect is a defect you have agreed to
// keep. So the proxy is now structurally incapable of sharing an event loop
// with a sleeping driver.
//
// ── control plane ──────────────────────────────────────────────────────────
//   POST /__proxy/kill      destroy live conns, refuse new ones -> {killed}
//   POST /__proxy/restore   accept again
//   GET  /__proxy/stats     {accepted, refused, killed, up, live, bytes_*}
//
// The control plane is a SEPARATE listener on its own port, because the data
// port is the thing being killed — a control plane behind the kill switch could
// not be asked to restore.
//
// usage: f24-killproxy.mjs --upstream-port N
// prints: KILLPROXY_READY data=<port> control=<port>

import http from 'node:http';
import net from 'node:net';

const args = { upstreamPort: null, upstreamHost: '127.0.0.1' };
const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i++) {
  if (argv[i] === '--upstream-port') args.upstreamPort = Number(argv[++i]);
  else if (argv[i] === '--upstream-host') args.upstreamHost = argv[++i];
}
if (!args.upstreamPort) {
  process.stderr.write('usage: f24-killproxy.mjs --upstream-port N\n');
  process.exit(2);
}

const state = {
  up: true,
  accepted: 0,
  refused: 0,
  killed: 0,
  bytesToUpstream: 0,
  bytesToClient: 0,
};
const live = new Set();

const data = net.createServer((client) => {
  if (!state.up) {
    state.refused += 1;
    // destroy(), not end(): a RST is what a host that has gone away produces.
    // A clean FIN tells the client the server is present and declining, which
    // is a different event and one a reconnect path may treat differently.
    client.destroy();
    return;
  }
  state.accepted += 1;
  const upstream = net.connect(args.upstreamPort, args.upstreamHost);
  const pair = { client, upstream };
  live.add(pair);
  const done = () => {
    live.delete(pair);
    client.destroy();
    upstream.destroy();
  };
  client.on('error', done);
  upstream.on('error', done);
  client.on('close', done);
  upstream.on('close', done);
  client.on('data', (b) => {
    state.bytesToUpstream += b.length;
  });
  upstream.on('data', (b) => {
    state.bytesToClient += b.length;
  });
  client.pipe(upstream);
  upstream.pipe(client);
});

function killAll() {
  state.up = false;
  const n = live.size;
  for (const p of live) {
    try {
      p.client.destroy();
      p.upstream.destroy();
    } catch {
      /* noop */
    }
  }
  live.clear();
  state.killed += n;
  return n;
}

const ctl = http.createServer((req, res) => {
  const json = (o) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify(o));
  };
  if (req.url === '/__proxy/kill' && req.method === 'POST') return json({ killed: killAll() });
  if (req.url === '/__proxy/restore' && req.method === 'POST') {
    state.up = true;
    return json({ up: true });
  }
  if (req.url === '/__proxy/stats') return json({ ...state, live: live.size });
  res.writeHead(404);
  res.end('{}');
});

data.listen(0, '127.0.0.1', () => {
  ctl.listen(0, '127.0.0.1', () => {
    process.stdout.write(`KILLPROXY_READY data=${data.address().port} control=${ctl.address().port}\n`);
  });
});
