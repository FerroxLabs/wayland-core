#!/usr/bin/env node
// A fake `signal-cli` that speaks JSON-RPC 2.0 on stdio — spawned BY THE
// PRODUCT, not by the driver.
//
// WHY THIS IS SHAPED DIFFERENTLY FROM EVERY OTHER FIXTURE IN THIS PHASE.
//
// Every prior adapter in Phase 24 was fixtured by redirecting an HTTP BASE URL:
// slack/whatsapp/sms/telegram all take an `api_base_url`, and 24-C3-FINISH's
// brief framed the remaining four adapters that way too. Signal does not have
// one, and a lane grepping the never-driven adapters for `*_base_url` finds
// nothing in signal and concludes it has no seam.
//
// It has the cheapest one in the phase, and it is a SUBPROCESS-PATH seam:
//
//   config.rs:16-18   `signal_cli_path: PathBuf`
//                     (`#[serde(default = "default_signal_cli_path")]` -> bare
//                      `signal-cli`, resolved on PATH, in production)
//   lib.rs:82-83      `SignalChannel::new` hardwires `Arc::new(RealLauncher)`
//   subprocess.rs:54  `Command::new(cli_path).arg("-a").arg(account).arg("jsonRpc")`
//   registry:157-169  `make_signal` calls `new()`               <- SHIPPED PATH
//
// So the fixture is an EXECUTABLE on a path, and there is no HTTP, no TLS, no
// port to bind and no certificate to mint. Note the seam that is NOT used:
// `with_launcher` is `#[doc(hidden)]` and test-only. This fixture deliberately
// goes through `new()` + `signal_cli_path`, because a `#[doc(hidden)]`
// constructor proves nothing about what an operator can configure — the exact
// distinction that left Discord's `with_token_url` unusable.
//
// THE CONTROL-PLANE PROBLEM, AND WHY IT IS A SOCKET.
//
// The product owns this process's stdin and stdout. The driver therefore cannot
// hand it a message the way it hands one to the Telegram fixture over HTTP. So
// this process opens its OWN TCP control listener on 127.0.0.1:0 and writes the
// bound port to the file named by `$F24_SIGNAL_CONTROL`. The driver reads that
// file and connects.
//
// EVERY OBSERVABLE IS IN THE JOURNAL, NOT IN THIS PROCESS'S MEMORY. That is
// deliberate: `supervisor.rs` RESPAWNS this executable when it dies, so a
// report served from one process's memory would silently omit whatever a prior
// incarnation saw. The journal is opened in append mode and carries the pid on
// every record, so a respawn is visible rather than invisible.
//
// STDOUT CARRIES JSON-RPC FRAMES AND NOTHING ELSE. `dispatch_line`
// (subprocess.rs:158) parses every line as JSON and logs a warning on anything
// else, so the ready banner goes to stderr (which the product drains into
// tracing::debug) and to the port file.
//
// env:
//   F24_SIGNAL_JOURNAL   required — append-mode JSONL journal
//   F24_SIGNAL_CONTROL   required — file this process writes `<port> <pid>` to
//
// argv (supplied by the product): -a <account> jsonRpc

import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import readline from 'node:readline';

const journalPath = process.env.F24_SIGNAL_JOURNAL;
const controlPath = process.env.F24_SIGNAL_CONTROL;

if (!journalPath || !controlPath) {
  process.stderr.write('f24-signal-fixture: F24_SIGNAL_JOURNAL and F24_SIGNAL_CONTROL are required\n');
  process.exit(2);
}

fs.mkdirSync(path.dirname(path.resolve(journalPath)), { recursive: true });
const journalFd = fs.openSync(journalPath, 'a');

let seq = 0;
const PID = process.pid;

function record(kind, detail) {
  seq += 1;
  const rec = { seq, pid: PID, kind, at: new Date().toISOString(), ...detail };
  fs.writeSync(journalFd, `${JSON.stringify(rec)}\n`);
  fs.fsyncSync(journalFd);
  return rec;
}

// The argv the product actually used. Recorded so the run can prove the SHIPPED
// launcher invoked this file with signal-cli's real argument shape, rather than
// the driver having reached in some other way.
record('spawn', { argv: process.argv.slice(2), cwd: process.cwd() });

// ── the JSON-RPC surface on stdio ───────────────────────────────────────────

function emit(frame) {
  process.stdout.write(`${JSON.stringify(frame)}\n`);
}

/// Emit a signal-cli `receive` notification.
///
/// `timestamp` is LOAD-BEARING and is not decoration: `subprocess.rs:277` sets
/// the message id to `format!("{ts_ms}")` from this very field, and the inbound
/// dedupe cache keys on that id. A replay under the SAME timestamp is what a
/// dedupe leg has to send; a fixture that stamped `Date.now()` itself would make
/// every replay a fresh message and the leg would be measuring nothing.
///
/// `sourceUuid` is deliberately omitted. `subprocess.rs:292-297` prefers it for
/// `sender_id` while `:281-287` prefers `source` for `conversation_id`; sending
/// only `source` makes both the same string, which is the PEER-KEYED shape
/// whatsapp/sms/telegram are already measured on, so signal's access leg
/// exercises the same shared gate rather than a signal-only asymmetry.
function emitReceive({ account, source, sourceName, text, timestamp }) {
  emit({
    jsonrpc: '2.0',
    method: 'receive',
    params: {
      account,
      envelope: {
        source,
        sourceName,
        timestamp,
        dataMessage: { message: text, timestamp },
      },
    },
  });
  record('receive.emitted', { source, source_name: sourceName, text, timestamp });
}

const rl = readline.createInterface({ input: process.stdin });
rl.on('line', (line) => {
  const trimmed = line.trim();
  if (!trimmed) return;
  let req;
  try {
    req = JSON.parse(trimmed);
  } catch {
    record('stdin.malformed', { line: trimmed.slice(0, 400) });
    return;
  }
  if (req.method === 'send') {
    const params = req.params ?? {};
    // `build_send_params` (jsonrpc.rs:216) emits EITHER `recipient: [..]` for a
    // direct message OR `groupId` for a group. Both are journalled so a run can
    // tell which shape the product chose rather than inferring it.
    const recipient = Array.isArray(params.recipient) ? params.recipient[0] : null;
    record('send', {
      id: req.id,
      recipient,
      group_id: params.groupId ?? null,
      message: String(params.message ?? ''),
    });
    // `classify_delivery` (jsonrpc.rs:191) reads `results[].type`; SUCCESS is
    // what makes the product treat the reply as delivered rather than retrying.
    emit({
      jsonrpc: '2.0',
      id: req.id,
      result: { timestamp: Date.now(), results: [{ type: 'SUCCESS' }] },
    });
    return;
  }
  record('stdin.unhandled', { method: req.method ?? null, id: req.id ?? null });
  if (req.id !== undefined && req.id !== null) {
    emit({ jsonrpc: '2.0', id: req.id, result: {} });
  }
});

rl.on('close', () => {
  // The product closes stdin to ask signal-cli to exit (lib.rs:196).
  record('stdin.closed', {});
  process.exit(0);
});

// ── the control plane ───────────────────────────────────────────────────────

const control = net.createServer((sock) => {
  let buf = '';
  sock.setEncoding('utf8');
  sock.on('data', (chunk) => {
    buf += chunk;
    let idx;
    while ((idx = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, idx).trim();
      buf = buf.slice(idx + 1);
      if (!line) continue;
      let msg;
      try {
        msg = JSON.parse(line);
      } catch {
        sock.write(`${JSON.stringify({ ok: false, error: 'bad json' })}\n`);
        continue;
      }
      if (msg.op === 'submit') {
        emitReceive({
          account: msg.account ?? null,
          source: msg.source,
          sourceName: msg.sourceName ?? msg.source,
          text: msg.text,
          timestamp: Number(msg.timestamp),
        });
        sock.write(`${JSON.stringify({ ok: true, timestamp: Number(msg.timestamp), pid: PID })}\n`);
        continue;
      }
      if (msg.op === 'health') {
        sock.write(`${JSON.stringify({ ok: true, pid: PID })}\n`);
        continue;
      }
      sock.write(`${JSON.stringify({ ok: false, error: `unknown op ${msg.op}` })}\n`);
    }
  });
  sock.on('error', () => {
    /* the driver hangs up between commands; not an event */
  });
});

control.listen(0, '127.0.0.1', () => {
  const port = control.address().port;
  // Written LAST, and atomically via rename, so a driver that sees the file at
  // all sees a complete `<port> <pid>` and never a half-written one. A partial
  // read here would send the driver at a port that does not exist and every
  // signal leg would report zero arrivals for a reason that is the driver's.
  const tmp = `${controlPath}.${PID}.tmp`;
  fs.writeFileSync(tmp, `${port} ${PID}\n`);
  fs.renameSync(tmp, controlPath);
  record('control.listening', { port });
  process.stderr.write(`SIGFIX_READY port=${port} pid=${PID}\n`);
});

for (const sig of ['SIGINT', 'SIGTERM']) {
  process.on(sig, () => {
    record('shutdown', { signal: sig });
    process.exit(0);
  });
}
