#!/usr/bin/env node
// An IMAP4rev1 server over implicit TLS, plus an SMTP server with STARTTLS, as
// one OS process with one journal — the fixture seam for the email adapter.
//
// WHY THIS EXISTS, AND WHY IT IS SHAPED THIS WAY.
//
// Email is the second POLLING adapter in the Phase 24 inbound matrix and it has
// never been driven at all. It matters more than the adapter count suggests,
// because it shares the DESTRUCTIVE-READ mechanism that lost 5 of 6 messages in
// steady state on Telegram (F24-C3-H4). For email the destruction is twofold:
//
//   * a real IMAP server sets `\Seen` on a non-PEEK `FETCH ... RFC822`, and
//   * `crates/wcore-channel-email/src/imap.rs` advances a persisted UID
//     watermark (`uid_store::save(host, user, mailbox, high_water)`, line ~290)
//     and thereafter searches `"<watermark+1>:*"`. Anything below the watermark
//     is never asked for again.
//
// The second is the load-bearing one: the watermark is keyed by
// (host, user, mailbox) and persisted OUTSIDE the session, so two pollers on
// one mailbox race on a shared file, and whatever the unsubscribed one advances
// past is gone. That is the email-shaped instance of the Telegram defect, and
// nothing has ever exercised it. This fixture models BOTH so a loss cannot hide
// behind the one that was not implemented.
//
// THE TLS SITUATION, WHICH IS NOT SYMMETRIC BETWEEN THE TWO PROTOCOLS.
//
//   IMAP  `crates/wcore-channel-email/Cargo.toml:13` pulls `native-tls`, and
//         `imap.rs:194` calls `native_tls::TlsConnector::new()`. On Linux that
//         is OpenSSL, and OpenSSL reads `SSL_CERT_FILE` at runtime. So pointing
//         a child-scoped `SSL_CERT_FILE` at this fixture's CA makes the SHIPPED
//         binary trust it, with no Rust change.
//
//         This does NOT work on macOS. `native-tls` there is
//         Security.framework, which resolves trust through the system keychain
//         and ignores `SSL_CERT_FILE` entirely. A macOS email leg needs a
//         different mechanism — see the summary.
//
//   SMTP  is NOT native-tls. `Cargo.toml:11` selects
//         `lettre/tokio1-rustls-tls`, and the resolved `lettre` node in
//         `Cargo.lock` depends on **`webpki-roots`**, not `rustls-native-certs`.
//         `webpki-roots` is a COMPILED-IN Mozilla root set: it reads no file and
//         no environment variable, on any platform. `SSL_CERT_FILE` therefore
//         cannot make the outbound path trust this fixture anywhere.
//
// So this fixture is deliberately asymmetric in what it claims. The IMAP half
// is a working seam. The SMTP half exists to be REACHED and REFUSED, so the
// refusal is a measured artifact — a certificate error in the binary's own log
// against a fixture that demonstrably completed its TCP accept and its
// STARTTLS offer — rather than an inference from a lockfile.
//
// usage:
//   f24-mail-fixture.mjs --journal <path> --cert <pem> --key <pem>
//                        [--imap-port 0] [--smtp-port 0] [--user u] [--pass p]

import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import tls from 'node:tls';

function parseArgs(argv) {
  const out = {
    journal: null,
    cert: null,
    key: null,
    imapPort: 0,
    smtpPort: 0,
    user: 'f24c3',
    pass: 'f24c3pass',
    mailbox: 'INBOX',
  };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--journal') out.journal = argv[++i];
    else if (a === '--cert') out.cert = argv[++i];
    else if (a === '--key') out.key = argv[++i];
    else if (a === '--imap-port') out.imapPort = Number(argv[++i]);
    else if (a === '--smtp-port') out.smtpPort = Number(argv[++i]);
    else if (a === '--user') out.user = argv[++i];
    else if (a === '--pass') out.pass = argv[++i];
    else {
      process.stderr.write(`f24-mail-fixture: unknown argument ${a}\n`);
      process.exit(2);
    }
  }
  for (const k of ['journal', 'cert', 'key']) {
    if (!out[k]) {
      process.stderr.write(`f24-mail-fixture: --${k} is required\n`);
      process.exit(2);
    }
  }
  return out;
}

const args = parseArgs(process.argv.slice(2));
fs.mkdirSync(path.dirname(path.resolve(args.journal)), { recursive: true });
const journalFd = fs.openSync(args.journal, 'a');

let seq = 0;

// Journal BEFORE answering, and fsync — same discipline as the arrivals sink.
// A record still in this process's page cache when the run ends is
// indistinguishable from an event that never happened.
function record(kind, detail) {
  seq += 1;
  const rec = { seq, kind, at: new Date().toISOString(), ...detail };
  fs.writeSync(journalFd, `${JSON.stringify(rec)}\n`);
  fs.fsyncSync(journalFd);
  return rec;
}

const tlsOptions = {
  cert: fs.readFileSync(args.cert),
  key: fs.readFileSync(args.key),
};

// ── the mailbox ──────────────────────────────────────────────────────────────
//
// `seen` is the destructive half a real IMAP server performs on a non-PEEK
// FETCH. It is recorded per message together with WHICH session set it, so the
// report can say who consumed what rather than only that something went missing
// — exactly as the Telegram fixture attributes each deletion to a poll.

let nextUid = 1000;
/** @type {{uid:number, raw:string, from:string, seen_by:number|null, fetched_by:number[]}[]} */
const messages = [];
let uidValidity = 1;

let sessionSeq = 0;
let openSessions = 0;
let maxConcurrentSessions = 0;
const sessionTrace = [];

/** @type {{seq:number, from:string, to:string[], data:string, at:string}[]} */
const delivered = [];
/** @type {{seq:number, stage:string, detail:string, at:string}[]} */
const smtpFailures = [];

function deliver({ from, to, subject, body, messageId }) {
  const uid = nextUid;
  nextUid += 1;
  const mid = messageId ?? `<${uid}.f24c3@fixture.invalid>`;
  const raw = [
    `Return-Path: <${from}>`,
    `From: ${from}`,
    `To: ${to}`,
    `Subject: ${subject}`,
    `Message-ID: ${mid}`,
    `Date: ${new Date().toUTCString()}`,
    'MIME-Version: 1.0',
    'Content-Type: text/plain; charset=utf-8',
    '',
    body,
    '',
  ].join('\r\n');
  messages.push({ uid, raw, from, message_id: mid, seen_by: null, fetched_by: [] });
  record('deliver', { uid, message_id: mid, from, to, subject, bytes: raw.length });
  return uid;
}

// ── IMAP ─────────────────────────────────────────────────────────────────────

function imapLine(sock, s) {
  sock.write(`${s}\r\n`);
}

/// `UID SEARCH <n>:*`.
///
/// Real servers return AT LEAST ONE result for `n:*` even when nothing is new —
/// the range is interpreted against the highest existing UID rather than as an
/// empty set. `imap.rs` explicitly defends against that (`if uid <= high_water
/// { continue }`), so the fixture MUST reproduce it. A fixture that returned an
/// empty set here would be easier to write and would silently stop exercising
/// that guard.
function searchFrom(low) {
  const hits = messages.filter((m) => m.uid >= low).map((m) => m.uid);
  if (hits.length > 0) return hits;
  if (messages.length === 0) return [];
  return [messages[messages.length - 1].uid];
}

function handleImapCommand(sock, sessionId, tag, rest) {
  const upper = rest.toUpperCase();

  if (upper.startsWith('CAPABILITY')) {
    imapLine(sock, '* CAPABILITY IMAP4rev1');
    imapLine(sock, `${tag} OK CAPABILITY completed`);
    return;
  }

  if (upper.startsWith('LOGIN')) {
    // `LOGIN "user" "pass"` or bare atoms.
    const m = /^LOGIN\s+("?)([^"\s]+)\1\s+("?)([^"]*)\3\s*$/i.exec(rest.trim());
    const user = m ? m[2] : null;
    const pass = m ? m[4] : null;
    const ok = user === args.user && pass === args.pass;
    // Never journal the password. Journal only whether it matched — a fixture
    // that logged the secret it was given would be a worse problem than the one
    // it is here to solve.
    record('imap.login', { session: sessionId, user, ok });
    if (!ok) {
      imapLine(sock, `${tag} NO [AUTHENTICATIONFAILED] LOGIN failed`);
      return;
    }
    imapLine(sock, `${tag} OK LOGIN completed`);
    return;
  }

  if (upper.startsWith('SELECT') || upper.startsWith('EXAMINE')) {
    const uidNext = nextUid;
    record('imap.select', { session: sessionId, exists: messages.length, uid_next: uidNext });
    imapLine(sock, '* FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft)');
    imapLine(sock, `* ${messages.length} EXISTS`);
    imapLine(sock, '* 0 RECENT');
    imapLine(sock, `* OK [UIDVALIDITY ${uidValidity}] UIDs valid`);
    imapLine(sock, `* OK [UIDNEXT ${uidNext}] Predicted next UID`);
    imapLine(sock, '* OK [PERMANENTFLAGS (\\Seen \\Deleted)] Limited');
    imapLine(sock, `${tag} OK [READ-WRITE] SELECT completed`);
    return;
  }

  if (/^UID\s+SEARCH/i.test(rest)) {
    const q = rest.replace(/^UID\s+SEARCH\s*/i, '').trim();
    const m = /^(\d+):\*/.exec(q);
    const low = m ? Number(m[1]) : 1;
    const hits = searchFrom(low);
    record('imap.uid_search', { session: sessionId, query: q, low, hits });
    imapLine(sock, `* SEARCH${hits.length ? ` ${hits.join(' ')}` : ''}`);
    imapLine(sock, `${tag} OK UID SEARCH completed`);
    return;
  }

  if (/^UID\s+FETCH/i.test(rest)) {
    const q = rest.replace(/^UID\s+FETCH\s*/i, '').trim();
    const uid = Number(q.split(/\s+/)[0]);
    const msg = messages.find((x) => x.uid === uid);
    if (!msg) {
      record('imap.uid_fetch.miss', { session: sessionId, uid });
      imapLine(sock, `${tag} OK UID FETCH completed`);
      return;
    }
    // THE DESTRUCTIVE HALF. A non-PEEK `RFC822` fetch sets `\Seen` on a real
    // server. Attribute it to the session that caused it.
    const firstRead = msg.seen_by === null;
    if (firstRead) msg.seen_by = sessionId;
    msg.fetched_by.push(sessionId);
    record('imap.uid_fetch', {
      session: sessionId,
      uid,
      bytes: msg.raw.length,
      set_seen: firstRead,
      already_seen_by: firstRead ? null : msg.seen_by,
    });
    const seqNo = messages.indexOf(msg) + 1;
    sock.write(`* ${seqNo} FETCH (UID ${uid} RFC822 {${Buffer.byteLength(msg.raw)}}\r\n`);
    sock.write(msg.raw);
    sock.write(')\r\n');
    imapLine(sock, `${tag} OK UID FETCH completed`);
    return;
  }

  if (upper.startsWith('LOGOUT')) {
    record('imap.logout', { session: sessionId });
    imapLine(sock, '* BYE fixture signing off');
    imapLine(sock, `${tag} OK LOGOUT completed`);
    sock.end();
    return;
  }

  if (upper.startsWith('NOOP') || upper.startsWith('CLOSE')) {
    imapLine(sock, `${tag} OK completed`);
    return;
  }

  record('imap.unhandled', { session: sessionId, command: rest.slice(0, 120) });
  imapLine(sock, `${tag} BAD unsupported`);
}

const imapServer = tls.createServer(tlsOptions, (sock) => {
  sessionSeq += 1;
  const sessionId = sessionSeq;
  openSessions += 1;
  if (openSessions > maxConcurrentSessions) maxConcurrentSessions = openSessions;
  sessionTrace.push({ at: new Date().toISOString(), open: openSessions, session: sessionId });
  record('imap.session.open', { session: sessionId, open: openSessions });

  let buf = '';
  sock.setEncoding('utf8');
  imapLine(sock, '* OK [CAPABILITY IMAP4rev1] f24c3 imap fixture ready');

  sock.on('data', (chunk) => {
    buf += chunk;
    let idx;
    while ((idx = buf.indexOf('\r\n')) >= 0) {
      const line = buf.slice(0, idx);
      buf = buf.slice(idx + 2);
      if (!line.trim()) continue;
      const sp = line.indexOf(' ');
      if (sp < 0) {
        imapLine(sock, '* BAD malformed');
        continue;
      }
      const tag = line.slice(0, sp);
      const rest = line.slice(sp + 1);
      try {
        handleImapCommand(sock, sessionId, tag, rest);
      } catch (e) {
        record('imap.error', { session: sessionId, error: String(e && e.message) });
        imapLine(sock, `${tag} BAD internal`);
      }
    }
  });

  const close = () => {
    openSessions -= 1;
    sessionTrace.push({ at: new Date().toISOString(), open: openSessions, session: sessionId });
    record('imap.session.close', { session: sessionId, open: openSessions });
  };
  sock.on('close', close);
  sock.on('error', (e) => {
    record('imap.socket.error', { session: sessionId, error: String(e && e.message) });
  });
});

imapServer.on('tlsClientError', (e) => {
  // The client rejected US, or failed the handshake. Recorded because for the
  // SMTP half this is the EXPECTED, MEASURED outcome and it must be visible as
  // an event rather than as silence.
  record('imap.tls_client_error', { error: String(e && e.message) });
});

// ── SMTP (STARTTLS) ──────────────────────────────────────────────────────────
//
// Plain on accept, offers STARTTLS, upgrades in place. lettre's
// `starttls_relay` requires the upgrade — there is no plaintext fallback — so
// reaching the upgrade and being refused at certificate verification is the
// measurement this half exists to produce.

function driveSmtp(sock, sessionId, secure) {
  let buf = '';
  let inData = false;
  let dataLines = [];
  let envFrom = null;
  let envTo = [];

  sock.setEncoding('utf8');

  const say = (s) => sock.write(`${s}\r\n`);

  sock.on('data', (chunk) => {
    buf += chunk;
    let idx;
    while ((idx = buf.indexOf('\r\n')) >= 0) {
      const line = buf.slice(0, idx);
      buf = buf.slice(idx + 2);

      if (inData) {
        if (line === '.') {
          inData = false;
          const data = dataLines.join('\r\n');
          const rec = record('smtp.delivered', {
            session: sessionId,
            secure,
            mail_from: envFrom,
            rcpt_to: envTo,
            bytes: data.length,
            data,
          });
          delivered.push({ seq: rec.seq, from: envFrom, to: [...envTo], data, at: rec.at });
          dataLines = [];
          envTo = [];
          say('250 2.0.0 Ok: queued');
          continue;
        }
        dataLines.push(line.startsWith('..') ? line.slice(1) : line);
        continue;
      }

      const upper = line.toUpperCase();
      record('smtp.command', { session: sessionId, secure, verb: upper.split(/[\s:]/)[0] });

      if (upper.startsWith('EHLO') || upper.startsWith('HELO')) {
        say('250-f24c3.fixture.invalid');
        say('250-PIPELINING');
        say('250-8BITMIME');
        if (!secure) say('250-STARTTLS');
        say('250 AUTH PLAIN LOGIN');
      } else if (upper.startsWith('STARTTLS')) {
        say('220 2.0.0 Ready to start TLS');
        sock.removeAllListeners('data');
        const upgraded = new tls.TLSSocket(sock, { isServer: true, ...tlsOptions });
        upgraded.on('secure', () => {
          record('smtp.starttls.established', { session: sessionId });
          driveSmtp(upgraded, sessionId, true);
        });
        upgraded.on('_tlsError', (e) => {
          const detail = String(e && e.message);
          record('smtp.starttls.rejected', { session: sessionId, error: detail });
          smtpFailures.push({ seq, stage: 'starttls', detail, at: new Date().toISOString() });
        });
        upgraded.on('error', (e) => {
          const detail = String(e && e.message);
          record('smtp.starttls.error', { session: sessionId, error: detail });
          smtpFailures.push({ seq, stage: 'starttls', detail, at: new Date().toISOString() });
        });
        return;
      } else if (upper.startsWith('AUTH')) {
        say('235 2.7.0 Authentication successful');
      } else if (upper.startsWith('MAIL FROM')) {
        envFrom = (/<([^>]*)>/.exec(line) ?? [, line])[1];
        say('250 2.1.0 Ok');
      } else if (upper.startsWith('RCPT TO')) {
        envTo.push((/<([^>]*)>/.exec(line) ?? [, line])[1]);
        say('250 2.1.5 Ok');
      } else if (upper.startsWith('DATA')) {
        inData = true;
        say('354 End data with <CR><LF>.<CR><LF>');
      } else if (upper.startsWith('QUIT')) {
        say('221 2.0.0 Bye');
        sock.end();
      } else if (upper.startsWith('RSET') || upper.startsWith('NOOP')) {
        say('250 2.0.0 Ok');
      } else {
        say('502 5.5.2 Unsupported');
      }
    }
  });

  sock.on('error', (e) => {
    record('smtp.socket.error', { session: sessionId, secure, error: String(e && e.message) });
  });
}

const smtpServer = net.createServer((sock) => {
  sessionSeq += 1;
  const sessionId = sessionSeq;
  record('smtp.session.open', { session: sessionId });
  sock.write('220 f24c3.fixture.invalid ESMTP f24c3\r\n');
  driveSmtp(sock, sessionId, false);
});

// ── control plane ────────────────────────────────────────────────────────────

function report() {
  return {
    ok: true,
    mailbox_total: messages.length,
    messages: messages.map((m) => ({
      uid: m.uid,
      message_id: m.message_id,
      from: m.from,
      seen_by: m.seen_by,
      fetch_count: m.fetched_by.length,
      fetched_by: m.fetched_by,
    })),
    max_concurrent_imap_sessions: maxConcurrentSessions,
    imap_session_total: sessionSeq,
    session_trace: sessionTrace,
    smtp_delivered_total: delivered.length,
    smtp_delivered: delivered,
    smtp_failures: smtpFailures,
  };
}

const controlServer = net.createServer((sock) => {
  let buf = '';
  sock.setEncoding('utf8');
  sock.on('data', (c) => {
    buf += c;
    const idx = buf.indexOf('\n');
    if (idx < 0) return;
    const line = buf.slice(0, idx).trim();
    buf = buf.slice(idx + 1);
    let out;
    if (line === 'report') out = report();
    else {
      try {
        const req = JSON.parse(line);
        if (req.op === 'deliver') out = { ok: true, uid: deliver(req) };
        else out = { ok: false, error: `unknown op ${req.op}` };
      } catch (e) {
        out = { ok: false, error: String(e && e.message) };
      }
    }
    sock.write(`${JSON.stringify(out)}\n`);
  });
  sock.on('error', () => {});
});

let ready = 0;
function maybeBanner() {
  ready += 1;
  if (ready < 3) return;
  process.stdout.write(
    `MAILFIX_READY imap=${imapServer.address().port} smtp=${smtpServer.address().port} ` +
      `control=${controlServer.address().port} journal=${path.resolve(args.journal)}\n`,
  );
}

imapServer.listen(args.imapPort, '127.0.0.1', maybeBanner);
smtpServer.listen(args.smtpPort, '127.0.0.1', maybeBanner);
controlServer.listen(0, '127.0.0.1', maybeBanner);

for (const sig of ['SIGINT', 'SIGTERM']) {
  process.on(sig, () => {
    record('shutdown', { signal: sig });
    process.exit(0);
  });
}
