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
      conn.seq += 1;
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
      for (const d of this.dispatched.filter((x) => x.s > after)) {
        this.send(conn, { op: 0, t: 'MESSAGE_CREATE', s: d.s, d: d.payload });
        conn.delivered += 1;
      }
      conn.seq = Math.max(conn.seq, after);
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

    let s = 0;
    for (const c of targets) {
      c.seq += 1;
      s = Math.max(s, c.seq);
      this.send(c, { op: 0, t: 'MESSAGE_CREATE', s: c.seq, d: payload });
      c.delivered += 1;
    }
    this.dispatched.push({ id, s: s || this.dispatched.length + 1, payload, sockets: targets.length, at: Date.now() });
    return targets.length;
  }

  report() {
    return {
      bot_id: this.botId,
      port: this.port,
      total_gateway_connections: this.totalConns,
      max_concurrent_gateway_connections: this.maxConcurrentConns,
      live_gateway_connections: this.conns.size,
      identify_count: this.identifyCount,
      resume_count: this.resumeCount,
      bad_token_identifies: this.badTokenIdentifies,
      heartbeats: this.heartbeats,
      dispatched_total: this.dispatched.length,
      dispatch_socket_deliveries: this.dispatched.reduce((a, d) => a + d.sockets, 0),
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
