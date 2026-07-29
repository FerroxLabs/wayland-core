#!/usr/bin/env node
// F24-C3-DISCORD — a Discord Gateway (WebSocket) + REST fixture.
//
// THE FIXTURE IS THE API. No vendor credential is used, read, or required. The
// bot token is MINTED HERE at run time and accepted because this process is the
// thing that would otherwise validate it — exactly as `f24-tg-fixture.mjs` does
// for Telegram. Three prior lanes recorded that Discord inbound could only be
// proven with a real bot token belonging to a human; that is false, and this
// file is the counter-evidence.
//
// WHY DISCORD NEEDED A FIXTURE AND NOT JUST A SEAM. Slack/WhatsApp/SMS receive
// by WEBHOOK: the driver POSTs into the binary and no fixture server is needed
// on the inbound path at all. Telegram receives by POLLING an HTTP endpoint.
// Discord is the only adapter that receives by dialling OUT to a WebSocket and
// being pushed to, so nothing can drive its inbound path without a server that
// speaks enough of the Gateway protocol to complete a handshake.
//
// PROTOCOL SURFACE. Deliberately partial, and the limits are declared in
// `coverage()` rather than left for a reader to infer:
//   op 10 HELLO         sent on connect, carries heartbeat_interval
//   op 2  IDENTIFY      accepted; token + intents recorded per connection
//   op 0  READY         replied with session_id (+ resume_gateway_url)
//   op 1  HEARTBEAT     acked with op 11
//   op 6  RESUME        accepted; replays dispatches after `seq`
//   op 0  MESSAGE_CREATE dispatched on demand by the driver
// REST: GET /api/v10/users/@me, POST /api/v10/channels/{id}/messages,
//       POST /api/v10/channels/{id}/typing, PUT .../reactions/...
//
// WHAT IT IS NOT: no compression (zlib/etransport), no ETF encoding, no shard
// negotiation, no voice, no rate-limit buckets, no permission model, no guild
// state. It is enough to prove ARRIVALS and to expose a consumption race.
//
// RFC6455 is hand-rolled — this repo has no npm dependency tree and adding one
// to run a test is a supply-chain decision, not a convenience.

import crypto from 'node:crypto';
import http from 'node:http';

const WS_GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';

// ── RFC6455 frame codec ──────────────────────────────────────────────────────

function encodeFrame(payload, opcode = 0x1) {
  const data = Buffer.from(payload, 'utf8');
  const len = data.length;
  let header;
  if (len < 126) {
    header = Buffer.alloc(2);
    header[1] = len;
  } else if (len < 65536) {
    header = Buffer.alloc(4);
    header[1] = 126;
    header.writeUInt16BE(len, 2);
  } else {
    header = Buffer.alloc(10);
    header[1] = 127;
    header.writeBigUInt64BE(BigInt(len), 2);
  }
  header[0] = 0x80 | opcode; // FIN + opcode
  // Server->client frames are NOT masked (RFC6455 §5.1).
  return Buffer.concat([header, data]);
}

// Incremental decoder: a TCP read may split a frame or carry several.
// Getting this wrong shows up as randomly-dropped messages, which would look
// exactly like the inbound loss this driver exists to measure — so it is a
// place the instrument could manufacture its own finding.
function decodeFrames(buf) {
  const frames = [];
  let off = 0;
  for (;;) {
    if (buf.length - off < 2) break;
    const b0 = buf[off];
    const b1 = buf[off + 1];
    const opcode = b0 & 0x0f;
    const masked = (b1 & 0x80) !== 0;
    let len = b1 & 0x7f;
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
    let mask = null;
    if (masked) {
      if (buf.length - p < 4) break;
      mask = buf.subarray(p, p + 4);
      p += 4;
    }
    if (buf.length - p < len) break;
    const body = Buffer.from(buf.subarray(p, p + len));
    if (mask) for (let i = 0; i < body.length; i += 1) body[i] ^= mask[i % 4];
    frames.push({ opcode, body });
    off = p + len;
  }
  return { frames, rest: buf.subarray(off) };
}

// ── the fixture ──────────────────────────────────────────────────────────────

export class DiscordFixture {
  constructor(opts = {}) {
    // Minted here. This is the whole point: the fixture is the authority that
    // would reject it, so it cannot reject it.
    this.botToken = opts.botToken ?? `MTIz.f24c3.${crypto.randomBytes(16).toString('hex')}`;
    this.botId = opts.botId ?? '999000111222333444';
    this.heartbeatIntervalMs = opts.heartbeatIntervalMs ?? 5_000;

    this.port = null;
    this.server = null;

    // Every live gateway connection. The Discord analogue of Telegram's
    // `max_concurrent_getupdates`: two ChannelManagers show up as two
    // authenticated sockets on one bot token.
    this.conns = new Set();
    this.connSeq = 0;
    this.maxConcurrentConns = 0;
    this.totalConns = 0;
    this.identifyCount = 0;
    this.resumeCount = 0;
    this.heartbeats = 0;
    this.badTokenIdentifies = 0;

    // Dispatch journal: every MESSAGE_CREATE this fixture pushed, and to how
    // many sockets. deliveries > dispatched is the duplication signal.
    this.dispatched = [];

    // SESSION-global monotonic dispatch sequence. Discord numbers `s` per
    // SESSION, not per socket, and a RESUME continues that same numbering —
    // which is the whole reason `op 6` can replay.
    //
    // This used to be derived per-connection (`conn.seq += 1`) with a fallback
    // of `this.dispatched.length + 1` for a dispatch that reached no socket.
    // That fallback FORGOT THAT READY CONSUMED SEQUENCE 1, so a message
    // dispatched during a disconnect window was numbered one LOWER than it
    // should be and collided with an already-delivered sequence. Measured on
    // 2026-07-29:
    //
    //     PRE-1 s=2   PRE-2 s=3   GAP-1 s=3 (collision)   POST-1 s=4
    //
    // With the client resuming from `seq=3`, the replay filter `x.s > after`
    // then discarded the gap message BY ITS OWN SEQUENCE NUMBER. The fixture
    // was structurally incapable of expressing "delivered while disconnected,
    // replayed on RESUME" — so any reconnect probe built on it reported
    // inbound message LOSS for every product, including a correct one.
    //
    // A single allocator makes the two paths inexpressible separately.
    this.seq = 0;

    // Reconnect journals. Both exist so a driver never has to INFER that the
    // disconnect happened or that the replay happened — inferring either from
    // an arrival count is how "no messages were lost" becomes self-passing.
    this.drops = [];
    this.resumeReplays = [];
    // Outbound journal: every message the binary POSTed back (the replies).
    this.sent = [];
    this.typing = [];
    this.reactions = [];
    this.restHits = [];

    this.faults = [];
  }

  note(msg) {
    this.faults.push(msg);
  }

  /**
   * Allocate the next SESSION sequence number.
   *
   * The only place `s` is minted. Subclasses that dispatch their own frame
   * shapes (`f24-media-actions.mjs`) call this rather than re-deriving it —
   * the re-derivation is what carried the collision bug into a second file.
   */
  nextSeq() {
    this.seq += 1;
    return this.seq;
  }

  // What this fixture does and does not implement — reported into the result
  // so a consumer never has to infer coverage from a green.
  coverage() {
    return {
      implemented: [
        'op10_hello',
        'op2_identify',
        'op0_ready',
        'op1_heartbeat/op11_ack',
        'op6_resume_with_replay',
        'op0_message_create_dispatch',
        'rest_users_me',
        'rest_create_message',
        'rest_typing',
        'rest_reactions',
      ],
      not_implemented: [
        'zlib/etf_compression',
        'shard_negotiation',
        'rate_limit_buckets',
        'guild_or_permission_state',
        'voice',
        'op9_invalid_session_negotiation',
      ],
    };
  }

  start() {
    return new Promise((resolve) => {
      this.server = http.createServer((req, res) => this.onRest(req, res));
      this.server.on('upgrade', (req, socket) => this.onUpgrade(req, socket));

      // INSTRUMENT REPAIR (found while diagnosing a real run). `totalConns`
      // only increments inside `onUpgrade`, so a client that opened a TCP
      // connection but whose HTTP request line Node rejected never appeared in
      // any counter — making "nothing ever dialled this port" and "something
      // dialled and the handshake was refused" the SAME observation, namely 0.
      // Those are opposite diagnoses (nothing started vs. a URL/protocol bug),
      // so counting raw sockets and malformed requests separately is the
      // difference between a usable measurement and a dead end.
      this.tcpConns = 0;
      this.clientErrors = [];
      this.server.on('connection', () => {
        this.tcpConns += 1;
      });
      this.server.on('clientError', (err, socket) => {
        this.clientErrors.push(String(err?.code ?? err?.message ?? err));
        try {
          socket.end('HTTP/1.1 400 Bad Request\r\n\r\n');
        } catch {
          /* noop */
        }
      });

      this.server.listen(0, '127.0.0.1', () => {
        this.port = this.server.address().port;
        resolve(this.port);
      });
    });
  }

  stop() {
    for (const c of this.conns) {
      try {
        c.socket.destroy();
      } catch {
        /* already gone */
      }
    }
    this.conns.clear();
    return new Promise((resolve) => (this.server ? this.server.close(resolve) : resolve()));
  }

  get apiBase() {
    return `http://127.0.0.1:${this.port}`;
  }

  get gatewayUrl() {
    return `ws://127.0.0.1:${this.port}`;
  }

  // ── REST half ──────────────────────────────────────────────────────────────

  onRest(req, res) {
    let body = '';
    req.on('data', (c) => (body += c));
    req.on('end', () => {
      const url = new URL(req.url, 'http://127.0.0.1');
      const p = url.pathname;
      const auth = req.headers.authorization ?? '';
      this.restHits.push({ method: req.method, path: p, at: Date.now() });

      const json = (code, obj) => {
        res.writeHead(code, { 'content-type': 'application/json' });
        res.end(JSON.stringify(obj));
      };

      // ── control plane (driver -> fixture), deliberately unauthenticated ────
      //
      // THIS FIXTURE MUST RUN AS ITS OWN OS PROCESS. Every driver in this
      // program sleeps with `Atomics.wait`, which blocks the whole Node event
      // loop; an in-process fixture therefore cannot accept a single TCP
      // connection while the driver waits, and the run reports "the binary
      // never connected" — a PRODUCT defect — when the truth is that the
      // instrument was not listening. That is exactly how this driver's first
      // two runs failed. Every other fixture here (`f24-tg-fixture.mjs`,
      // `f24-llm-fixture.mjs`) is spawned separately for the same reason.
      if (p === '/__control/dispatch' && req.method === 'POST') {
        const spec = JSON.parse(body || '{}');
        const sockets = this.dispatchMessage(spec);
        return json(200, { sockets });
      }
      if (p === '/__control/report' && req.method === 'GET') {
        return json(200, this.report());
      }
      // Induce a genuine upstream disconnect. The fixture runs as its own OS
      // process, so the driver cannot reach `dropAllSockets()` directly — this
      // is the only way to drop the socket from OUTSIDE the binary without
      // signalling or killing the binary itself, which would be a process
      // restart and a different event entirely.
      if (p === '/__control/drop' && req.method === 'POST') {
        return json(200, { dropped: this.dropAllSockets() });
      }
      if (p === '/__control/replies' && req.method === 'GET') {
        return json(200, { sent: this.sent });
      }

      // The fixture still ENFORCES its own minted token. A fixture that
      // accepted anything would pass an adapter that sent no credential at
      // all, which is a green by universal acceptance — the mirror image of
      // the universal-denial trap.
      if (auth !== `Bot ${this.botToken}`) {
        return json(401, { message: '401: Unauthorized', code: 0 });
      }

      if (req.method === 'GET' && p === '/api/v10/users/@me') {
        return json(200, { id: this.botId, username: 'f24c3-fixture-bot', bot: true });
      }

      let m = p.match(/^\/api\/v10\/channels\/([^/]+)\/messages$/);
      if (req.method === 'POST' && m) {
        let parsed = {};
        try {
          parsed = JSON.parse(body || '{}');
        } catch {
          return json(400, { message: 'bad json', code: 50109 });
        }
        const id = `${Date.now()}${this.sent.length}`;
        this.sent.push({
          id,
          channel_id: m[1],
          content: parsed.content ?? '',
          nonce: parsed.nonce ?? null,
          message_reference: parsed.message_reference ?? null,
          at: Date.now(),
        });
        return json(200, {
          id,
          channel_id: m[1],
          content: parsed.content ?? '',
          timestamp: new Date().toISOString(),
        });
      }

      m = p.match(/^\/api\/v10\/channels\/([^/]+)\/typing$/);
      if (req.method === 'POST' && m) {
        this.typing.push({ channel_id: m[1], at: Date.now() });
        res.writeHead(204);
        return res.end();
      }

      m = p.match(/^\/api\/v10\/channels\/([^/]+)\/messages\/([^/]+)\/reactions\/([^/]+)\/@me$/);
      if (req.method === 'PUT' && m) {
        this.reactions.push({
          channel_id: m[1],
          message_id: m[2],
          emoji: decodeURIComponent(m[3]),
          at: Date.now(),
        });
        res.writeHead(204);
        return res.end();
      }

      return json(404, { message: '404: Not Found', code: 0 });
    });
  }

  // ── Gateway half ───────────────────────────────────────────────────────────

  onUpgrade(req, socket) {
    const key = req.headers['sec-websocket-key'];
    if (!key) return socket.destroy();
    const accept = crypto
      .createHash('sha1')
      .update(key + WS_GUID)
      .digest('base64');
    socket.write(
      'HTTP/1.1 101 Switching Protocols\r\n' +
        'Upgrade: websocket\r\n' +
        'Connection: Upgrade\r\n' +
        `Sec-WebSocket-Accept: ${accept}\r\n\r\n`,
    );
    socket.setNoDelay(true);

    this.connSeq += 1;
    const conn = {
      n: this.connSeq,
      socket,
      buf: Buffer.alloc(0),
      identified: false,
      sessionId: null,
      seq: 0,
      url: req.url ?? '',
      openedAt: Date.now(),
      heartbeats: 0,
      delivered: 0,
    };
    this.conns.add(conn);
    this.totalConns += 1;
    this.maxConcurrentConns = Math.max(this.maxConcurrentConns, this.conns.size);

    socket.on('data', (chunk) => {
      conn.buf = Buffer.concat([conn.buf, chunk]);
      const { frames, rest } = decodeFrames(conn.buf);
      conn.buf = rest;
      for (const f of frames) {
        if (f.opcode === 0x8) {
          // close
          this.conns.delete(conn);
          try {
            socket.destroy();
          } catch {
            /* noop */
          }
          return;
        }
        if (f.opcode === 0x9) {
          socket.write(encodeFrame(f.body.toString('utf8'), 0xa)); // pong
          continue;
        }
        if (f.opcode !== 0x1) continue;
        this.onGatewayFrame(conn, f.body.toString('utf8'));
      }
    });
    const drop = () => this.conns.delete(conn);
    socket.on('close', drop);
    socket.on('error', drop);

    // op 10 HELLO
    this.send(conn, { op: 10, d: { heartbeat_interval: this.heartbeatIntervalMs } });
  }

  send(conn, obj) {
    try {
      conn.socket.write(encodeFrame(JSON.stringify(obj)));
      return true;
    } catch {
      return false;
    }
  }

  onGatewayFrame(conn, text) {
    let msg;
    try {
      msg = JSON.parse(text);
    } catch {
      this.note(`unparseable client frame on conn#${conn.n}`);
      return;
    }

    if (msg.op === 2) {
      // IDENTIFY
      this.identifyCount += 1;
      if (msg.d?.token !== this.botToken) {
        // Record rather than silently dropping: an adapter sending the wrong
        // token would otherwise present as "no arrivals", i.e. as inbound
        // loss, which is a different defect entirely.
        this.badTokenIdentifies += 1;
        this.note(`conn#${conn.n} IDENTIFY presented a token the fixture did not mint`);
        this.send(conn, { op: 9, d: false });
        return;
      }
      conn.identified = true;
      conn.intents = msg.d?.intents ?? null;
      conn.sessionId = `f24c3-sess-${conn.n}-${crypto.randomBytes(4).toString('hex')}`;
      conn.seq = this.nextSeq();
      this.send(conn, {
        op: 0,
        t: 'READY',
        s: conn.seq,
        d: {
          session_id: conn.sessionId,
          resume_gateway_url: this.gatewayUrl,
          user: { id: this.botId, username: 'f24c3-fixture-bot', bot: true },
        },
      });
      return;
    }

    if (msg.op === 1) {
      // HEARTBEAT
      conn.heartbeats += 1;
      this.heartbeats += 1;
      this.send(conn, { op: 11 });
      return;
    }

    if (msg.op === 6) {
      // RESUME — replay everything after the client's last seq.
      this.resumeCount += 1;
      conn.identified = true;
      conn.sessionId = msg.d?.session_id ?? conn.sessionId;
      const after = Number(msg.d?.seq ?? 0);
      const replayed = this.dispatched.filter((x) => x.s > after);
      for (const d of replayed) {
        this.send(conn, { op: 0, t: 'MESSAGE_CREATE', s: d.s, d: d.payload });
        conn.delivered += 1;
        // A replayed frame reached one more socket. Without this the
        // duplication detector (`dispatch_socket_deliveries` vs
        // `dispatched_total`) is BLIND to a replay, so a product that resumed
        // twice and took the same message twice would look identical to one
        // that resumed once — and duplicate-freedom is half of what this
        // criterion's reconnect clause asks.
        d.sockets += 1;
      }
      this.resumeReplays.push({ conn: conn.n, after, replayed: replayed.map((x) => x.id), at: Date.now() });
      // The resumed socket is caught up to the SESSION, not merely to the seq
      // it asked from — otherwise the next dispatch's `s` would appear to jump.
      conn.seq = this.seq;
      this.send(conn, { op: 0, t: 'RESUMED', s: conn.seq, d: {} });
      return;
    }
  }

  /**
   * Push a MESSAGE_CREATE to every identified connection.
   *
   * Returns the number of SOCKETS it reached. That number is the duplication
   * detector: one logical message delivered to two sockets is what a second
   * ChannelManager looks like on a push transport.
   */
  dispatchMessage({ id, channelId, content, authorId, guildId, mentions }) {
    const targets = [...this.conns].filter((c) => c.identified);
    const payload = {
      id,
      channel_id: channelId,
      content,
      timestamp: new Date().toISOString(),
      author: { id: authorId, username: `u${authorId}`, bot: false },
      mentions: (mentions ?? []).map((mid) => ({ id: mid })),
      attachments: [],
    };
    if (guildId) payload.guild_id = guildId;

    // Minted ONCE, before the fan-out, and minted whether or not anyone is
    // listening. That is the repair: a dispatch into an empty connection set
    // still consumes a real sequence number, so it sorts strictly after
    // everything already delivered and RESUME can replay it.
    const s = this.nextSeq();
    for (const c of targets) {
      c.seq = s;
      this.send(c, { op: 0, t: 'MESSAGE_CREATE', s, d: payload });
      c.delivered += 1;
    }
    this.dispatched.push({ id, s, payload, sockets: targets.length, at: Date.now() });
    return targets.length;
  }

  /**
   * Force every live gateway socket down WITHOUT a WebSocket close frame.
   *
   * `socket.destroy()` rather than a clean op-8 close, because a clean close is
   * the easy case: the client is TOLD the session ended. A destroyed socket is
   * what a real network drop looks like — the peer learns about it from a read
   * error or from a heartbeat that never gets ACKed, which is the path
   * `gateway.rs`'s `Err(e) => ... ReconnectReason::Resumable` branch exists to
   * serve and the path that has never been driven.
   *
   * Returns how many sockets it dropped, so a driver can prove the drop
   * HAPPENED. A probe that induces a disconnect it cannot confirm grades the
   * product on an event that may never have occurred — the reconnect-clause
   * flavour of a self-passing gate.
   */
  dropAllSockets() {
    const n = this.conns.size;
    for (const c of this.conns) {
      try {
        c.socket.destroy();
      } catch {
        /* already gone */
      }
    }
    this.conns.clear();
    this.drops.push({ dropped: n, at: Date.now() });
    return n;
  }

  report() {
    return {
      bot_id: this.botId,
      port: this.port,
      // Raw TCP sockets accepted, vs. sockets that completed the WS upgrade.
      // tcp > 0 with upgrades == 0 means the client DIALLED and the handshake
      // failed — a different defect from "the client never started".
      tcp_connections: this.tcpConns ?? 0,
      client_errors: this.clientErrors ?? [],
      total_gateway_connections: this.totalConns,
      max_concurrent_gateway_connections: this.maxConcurrentConns,
      live_gateway_connections: this.conns.size,
      identify_count: this.identifyCount,
      resume_count: this.resumeCount,
      bad_token_identifies: this.badTokenIdentifies,
      heartbeats: this.heartbeats,
      dispatched_total: this.dispatched.length,
      dispatch_socket_deliveries: this.dispatched.reduce((a, d) => a + d.sockets, 0),
      // Per-message ledger, so a driver derives losses and duplicates from the
      // fixture's own journal rather than trusting an aggregate.
      dispatch_ledger: this.dispatched.map((d) => ({ id: d.id, s: d.s, sockets: d.sockets })),
      // Sequence collisions. Must be 0. A non-zero here means the fixture
      // cannot express a replay and NO reconnect verdict from this run is
      // trustworthy — it is a NOT-MEASURED signal, not a product FAIL.
      duplicate_seq_numbers: this.dispatched.length - new Set(this.dispatched.map((d) => d.s)).size,
      forced_drops: this.drops.length,
      forced_drop_sockets: this.drops.reduce((a, d) => a + d.dropped, 0),
      resume_replays: this.resumeReplays,
      resume_replayed_total: this.resumeReplays.reduce((a, r) => a + r.replayed.length, 0),
      sent_total: this.sent.length,
      sent: this.sent,
      typing_total: this.typing.length,
      reactions_total: this.reactions.length,
      rest_hits: this.restHits.length,
      coverage: this.coverage(),
      fixture_notes: this.faults,
    };
  }
}

export default DiscordFixture;

// ── standalone mode ──────────────────────────────────────────────────────────
//
// usage: node f24-discord-fixture.mjs [--token <t>] [--heartbeat-ms N]
// Prints a single ready banner the driver greps for, then serves until killed.
if (import.meta.url === `file://${process.argv[1]}`) {
  const argv = process.argv.slice(2);
  const opts = {};
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === '--token') opts.botToken = argv[++i];
    else if (argv[i] === '--heartbeat-ms') opts.heartbeatIntervalMs = Number(argv[++i]);
    else if (argv[i] === '--bot-id') opts.botId = argv[++i];
  }
  const fx = new DiscordFixture(opts);
  await fx.start();
  process.stdout.write(
    `DISCFIX_READY url=${fx.apiBase} gateway=${fx.gatewayUrl} bot_id=${fx.botId}\n`,
  );
  const bye = async () => {
    await fx.stop();
    process.exit(0);
  };
  process.on('SIGTERM', bye);
  process.on('SIGINT', bye);
}
