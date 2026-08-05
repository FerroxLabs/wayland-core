#!/usr/bin/env node
// F24-RECONNECT (poll transports) — the upstream-drop half of 24-C3 for MATRIX.
//
// Discord is a push transport with a session-resume protocol, so its reconnect
// question is "does RESUME replay the window" (answered in `f24-reconnect.mjs`).
// Matrix is a POLL transport, and its reconnect question is different and, on
// this criterion, more dangerous:
//
//     the homeserver keeps receiving messages while OUR LINK to it is down.
//     When the link returns, does the adapter still ask for the window it
//     missed — or has its cursor moved past it?
//
// That is the exact shape of F24-C3-H6 (matrix lost everything delivered while
// the PROCESS was down, because the cursor was process-local). H6 fixed the
// cursor by persisting it to disk. **That fix does not answer this question**: a
// process that stays up never reads that file. The in-process path is separate
// code and has never been driven.
//
// ── how the upstream is dropped ─────────────────────────────────────────────
// A TCP kill-proxy sits between the binary and the homeserver fixture. The
// binary's `homeserver_url` points at the PROXY; the driver injects messages
// straight into the FIXTURE. So during the outage:
//
//   · the adapter's connections are destroyed mid-flight and new ones refused
//     — a genuine upstream disappearance, not a fixture that politely answers
//     an error;
//   · the homeserver keeps accepting messages, which is what a real homeserver
//     does and is the whole reason the window exists.
//
// This deliberately touches NO shared fixture. `f24-matrix-fixture.mjs` is
// depended on by four other drivers and 24-H6 declined to change it mid-flight
// for exactly that reason; a proxy in front needs none of that risk.
//
// usage: f24-reconnect-poll.mjs --binary <path> --run-dir <dir> [--control-no-recovery]
// exit:  0 GREEN, 1 RED, 2 USAGE, 3 NOT MEASURED

import { execFileSync, spawn } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { matchesToken } from './f24-discord-inbound.mjs';
import { census, mintToken } from './f24-reconnect.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));

export const RESULT_SCHEMA = 'wayland.reconnect.upstream-drop.poll/1';

const MX = {
  bot: '@f24bot:f24.invalid',
  allowed: '@f24allowed:f24.invalid',
  denied: '@f24denied:f24.invalid',
  room1: '!f24room1:f24.invalid',
};

function sleep(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

function control(url, method, body) {
  const args = ['-s', '-X', method, url];
  if (body !== undefined) args.push('-H', 'content-type: application/json', '-d', JSON.stringify(body));
  return JSON.parse(execFileSync('curl', args, { encoding: 'utf8', timeout: 15_000 }));
}

/**
 * Handle to the OUT-OF-PROCESS kill proxy (`f24-killproxy.mjs`).
 *
 * It is a separate OS process because this driver sleeps with `Atomics.wait`,
 * which blocks the whole Node event loop. An in-process proxy cannot forward a
 * byte while the driver waits — and the driver waits almost all the time.
 *
 * That is not a precaution, it is a repair: the first version of this file
 * embedded the proxy and the run reported `sync_total=0` with
 * `/sync failed; backing off — error sending request`, which reads as "the
 * matrix adapter cannot reach its homeserver". A product defect, from an
 * instrument that was not listening.
 */
class ProxyHandle {
  constructor(dataPort, controlPort) {
    this.dataPort = dataPort;
    this.controlPort = controlPort;
  }

  get url() {
    return `http://127.0.0.1:${this.dataPort}`;
  }

  kill() {
    return control(`http://127.0.0.1:${this.controlPort}/__proxy/kill`, 'POST', {}).killed;
  }

  restore() {
    return control(`http://127.0.0.1:${this.controlPort}/__proxy/restore`, 'POST', {});
  }

  stats() {
    return control(`http://127.0.0.1:${this.controlPort}/__proxy/stats`, 'GET');
  }

  stop() {
    /* the child process is killed by the driver's cleanup */
  }
}

class Driver {
  constructor(args) {
    this.args = args;
    this.home = path.join(args.runDir, 'home');
    this.results = [];
    this.notes = [];
    this.plan = [];
    this.children = [];
    this.tag = crypto.randomBytes(3).toString('hex');
    this.vaultPassphrase = crypto.randomBytes(24).toString('hex');
    this.mxToken = crypto.randomBytes(16).toString('hex');
    this.eventN = 0;
  }

  note(m) {
    this.notes.push(m);
    process.stdout.write(`  · ${m}\n`);
  }

  record(leg, ok, detail) {
    this.results.push({ leg, ok, detail });
    process.stdout.write(`${ok ? 'PASS' : 'FAIL'} ${leg} — ${detail}\n`);
  }

  startFixture(script, extraArgs, readyRe, logName) {
    const logPath = path.join(this.args.runDir, logName);
    fs.writeFileSync(logPath, '');
    const fd = fs.openSync(logPath, 'a');
    this.children.push(
      spawn(process.execPath, [path.join(HERE, script), ...extraArgs], { stdio: ['ignore', fd, fd] }),
    );
    for (let i = 0; i < 150; i += 1) {
      const m = readyRe.exec(fs.readFileSync(logPath, 'utf8'));
      if (m) return m;
      sleep(100);
    }
    throw new Error(`${script} never printed its ready banner`);
  }

  startLlm() {
    this.llmJournal = path.join(this.args.runDir, 'llm-journal.jsonl');
    fs.writeFileSync(this.llmJournal, '');
    const m = this.startFixture(
      'f24-llm-fixture.mjs',
      ['--port', '0', '--journal', this.llmJournal],
      /http:\/\/127\.0\.0\.1:\d+/,
      'llm.log',
    );
    this.llmUrl = m[0];
  }

  /** Same preflight contract as the discord driver — see f24-reconnect.mjs. */
  preflightCorrelation() {
    const good = mintToken(this.tag, 'preflight', 0);
    const ask = (t) =>
      String(
        control(`${this.llmUrl}/chat/completions`, 'POST', {
          model: 'x',
          stream: false,
          messages: [{ role: 'user', content: `hello ${t}` }],
        })?.choices?.[0]?.message?.content ?? '',
      );
    const echoed = matchesToken(ask(good), good);
    const wrong = `f24rc-${this.tag}-wrongshape`;
    const leaked = matchesToken(ask(wrong), wrong);
    this.note(`preflight correlation: echoed=${echoed} wrong-shape-echoed=${leaked} (must be false)`);
    if (!echoed) throw new Error('llm fixture did not echo this driver token shape — NOT MEASURED');
    if (leaked) throw new Error('llm fixture echoes anything — the correlation check cannot discriminate');
  }

  writeConfig() {
    fs.mkdirSync(path.join(this.home, 'channels'), { recursive: true });
    fs.writeFileSync(
      path.join(this.home, 'credentials.toml'),
      ['[secrets]', `"matrix.f24rc.access_token" = "${this.mxToken}"`, ''].join('\n'),
      { mode: 0o600 },
    );
    fs.writeFileSync(
      path.join(this.home, 'config.toml'),
      [
        '[default]',
        'provider = "f24rcfixture"',
        '',
        '[providers.f24rcfixture]',
        'provider = "openai"',
        'model = "f24rc-fixture"',
        'api_key = "f24rc-not-a-real-key"',
        `base_url = "${this.llmUrl}"`,
        '',
        '[inbound_webhook]',
        'enabled = false',
        '',
      ].join('\n'),
      { mode: 0o600 },
    );
    fs.writeFileSync(
      path.join(this.home, 'channels', 'f24rcmatrix.toml'),
      [
        'name = "f24rcmatrix"',
        'platform = "matrix"',
        'enabled = true',
        '',
        '[options]',
        // THE SEAM: points at the KILL PROXY, not at the fixture.
        `homeserver_url = "${this.proxy.url}"`,
        'credential_handle_access_token = "matrix.f24rc.access_token"',
        `user_id = "${MX.bot}"`,
        '',
        '[inbound]',
        'dm = "allowlist"',
        `dm_allowlist = ["${MX.allowed}"]`,
        'group = "disabled"',
        'require_mention = false',
        'tools = "conversational"',
        '',
      ].join('\n'),
    );
  }

  startGateway() {
    this.gwLog = path.join(this.args.runDir, 'gateway.log');
    fs.writeFileSync(this.gwLog, '');
    const out = fs.openSync(this.gwLog, 'a');
    this.child = spawn(this.args.binary, ['gateway', 'run'], {
      stdio: ['ignore', out, out],
      env: {
        ...process.env,
        WAYLAND_HOME: this.home,
        WAYLAND_VAULT_PASSPHRASE: this.vaultPassphrase,
        RUST_LOG: 'info,wcore_channel_matrix=debug',
      },
    });
  }

  mxReport() {
    return control(`${this.mxUrl}/__control/report`, 'GET');
  }

  /** Inject straight into the FIXTURE, bypassing the proxy — the homeserver
   *  keeps receiving while our link to it is down. That is the window. */
  submit({ phase, expect, sender }) {
    const token = mintToken(this.tag, phase, this.plan.length);
    this.plan.push({ token, phase, expect });
    this.eventN += 1;
    control(`${this.mxUrl}/__control/submit`, 'POST', {
      room: MX.room1,
      sender: sender ?? MX.allowed,
      text: `hello ${token}`,
      eventId: `$f24rc${this.tag}${this.eventN}`,
    });
    this.note(`submitted ${token} (phase=${phase} expect=${expect}) to the homeserver`);
    return token;
  }

  plantPhantom() {
    const token = mintToken(this.tag, 'phantom', 'never');
    this.plan.push({ token, phase: 'phantom', expect: 'silence' });
    return token;
  }

  /** Replies land in the LLM journal (matrix outbound goes to the homeserver,
   *  but the turn EXECUTING is what proves inbound delivery — and the journal
   *  is an independent OS process's record of it). */
  llmSeen() {
    if (!fs.existsSync(this.llmJournal)) return [];
    return fs
      .readFileSync(this.llmJournal, 'utf8')
      .split('\n')
      .filter(Boolean)
      .map((l) => {
        try {
          return JSON.parse(l);
        } catch {
          return null;
        }
      })
      .filter(Boolean)
      .map((r) => ({ content: String(r.user_text ?? '') }));
  }

  waitForTokens(tokens, budgetMs, label) {
    const deadline = Date.now() + budgetMs;
    let i = 0;
    for (;;) {
      i += 1;
      const seen = this.llmSeen();
      const got = tokens.filter((t) => seen.some((r) => matchesToken(r.content, t)));
      if (got.length === tokens.length) {
        this.note(`${label}: ${got.length}/${tokens.length} after ~${i * 500}ms`);
        return got.length;
      }
      if (Date.now() >= deadline) {
        this.note(`${label}: ${got.length}/${tokens.length} — BUDGET EXPIRED after ${budgetMs}ms`);
        return got.length;
      }
      if (i % 10 === 0) process.stdout.write(`  awaiting ${label}: ${got.length}/${tokens.length}\n`);
      sleep(500);
    }
  }

  waitForSyncs(baseline, delta, budgetMs, label) {
    const deadline = Date.now() + budgetMs;
    let i = 0;
    for (;;) {
      i += 1;
      const r = this.mxReport();
      if ((r.sync_total ?? 0) >= baseline + delta) return r;
      if (Date.now() >= deadline) return r;
      if (i % 10 === 0) process.stdout.write(`  awaiting ${label}: sync_total=${r.sync_total}\n`);
      sleep(500);
    }
  }

  cleanup() {
    try {
      this.proxy?.stop();
    } catch {
      /* noop */
    }
    for (const c of [this.child, ...this.children]) {
      try {
        c?.kill('SIGKILL');
      } catch {
        /* noop */
      }
    }
  }
}

function main() {
  const args = { binary: null, runDir: null, budgetMs: 60_000, outageMs: 12_000, controlNoRecovery: false };
  const argv = process.argv.slice(2);
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--binary') args.binary = argv[++i];
    else if (a === '--run-dir') args.runDir = argv[++i];
    else if (a === '--budget-ms') args.budgetMs = Number(argv[++i]);
    else if (a === '--outage-ms') args.outageMs = Number(argv[++i]);
    else if (a === '--control-no-recovery') args.controlNoRecovery = true;
    else {
      process.stderr.write(`unknown arg ${a}\n`);
      process.exit(2);
    }
  }
  if (!args.binary || !args.runDir) {
    process.stderr.write('usage: f24-reconnect-poll.mjs --binary <path> --run-dir <dir>\n');
    process.exit(2);
  }
  fs.mkdirSync(args.runDir, { recursive: true });

  const d = new Driver(args);
  main2(d, args).catch((e) => {
    d.cleanup();
    fs.writeFileSync(
      path.join(args.runDir, 'result.json'),
      `${JSON.stringify({ schema: RESULT_SCHEMA, verdict: 'NOT MEASURED', error: String(e?.message ?? e), notes: d.notes, results: d.results }, null, 2)}\n`,
    );
    process.stderr.write(`NOT MEASURED: ${String(e?.stack ?? e)}\n`);
    process.exit(3);
  });
}

async function main2(d, args) {
  d.mxJournal = path.join(args.runDir, 'matrix-journal.jsonl');
  const m = d.startFixture(
    'f24-matrix-fixture.mjs',
    ['--journal', d.mxJournal, '--token', d.mxToken, '--room', `${MX.room1}:2`, '--max-wait-ms', '2000'],
    /MXFIX_READY url=(\S+)/,
    'matrix.log',
  );
  d.mxUrl = m[1];
  const upstreamPort = Number(new URL(d.mxUrl).port);
  const pm = d.startFixture(
    'f24-killproxy.mjs',
    ['--upstream-port', String(upstreamPort)],
    /KILLPROXY_READY data=(\d+) control=(\d+)/,
    'killproxy.log',
  );
  d.proxy = new ProxyHandle(Number(pm[1]), Number(pm[2]));
  d.note(`homeserver fixture ${d.mxUrl}; binary will talk to the KILL PROXY at ${d.proxy.url}`);

  // PROVE the proxy forwards BEFORE the binary depends on it. Without this a
  // dead proxy presents as "the adapter cannot reach its homeserver" — the
  // exact misreading that cost this lane a run.
  const through = control(`${d.proxy.url}/__control/report`, 'GET');
  if (typeof through?.sync_total !== 'number') {
    throw new Error(`the kill proxy does not forward to the homeserver (got ${JSON.stringify(through)}) — NOT MEASURED`);
  }
  d.note(`proxy forwards: reached the homeserver report through it (sync_total=${through.sync_total})`);

  d.startLlm();
  d.preflightCorrelation();
  d.writeConfig();
  d.startGateway();

  // Wait for the adapter to be syncing at all.
  const up = d.waitForSyncs(0, 3, 60_000, 'initial syncs');
  if ((up.sync_total ?? 0) < 3) {
    throw new Error(`the binary never established a /sync loop (sync_total=${up.sync_total}) — NOT MEASURED`);
  }
  d.note(`adapter syncing: sync_total=${up.sync_total} proxy=${JSON.stringify(d.proxy.stats())}`);

  const phantom = d.plantPhantom();

  // ── BEFORE ────────────────────────────────────────────────────────────────
  const before = [d.submit({ phase: 'before', expect: 'reply' }), d.submit({ phase: 'before', expect: 'reply' })];
  const beforeGot = d.waitForTokens(before, args.budgetMs, 'pre-drop control');
  d.record(
    'pre-drop-control',
    beforeGot === before.length,
    `${beforeGot}/${before.length} produced a turn. KNOWN-POSITIVE: if this is not full, no zero elsewhere is readable.`,
  );

  // ── THE OUTAGE ────────────────────────────────────────────────────────────
  const preKill = d.mxReport();
  const killed = d.proxy.kill();
  d.note(`upstream KILLED: destroyed ${killed} live connection(s); new connections will be refused`);

  // Messages the homeserver receives while our link is down. This is the window.
  const during = [d.submit({ phase: 'during', expect: 'reply' }), d.submit({ phase: 'during', expect: 'reply' })];
  const decoy = d.submit({ phase: 'during-decoy', expect: 'silence', sender: MX.denied });

  sleep(args.outageMs);
  const outageStats = d.proxy.stats();
  const duringOutage = d.mxReport();
  d.record(
    'upstream-drop-really-happened',
    killed >= 1 && outageStats.refused >= 1 && duringOutage.sync_total === preKill.sync_total,
    `destroyed ${killed} connection(s); proxy refused ${outageStats.refused} reconnect attempt(s) during a ${args.outageMs}ms outage; ` +
      `homeserver sync_total FROZE at ${preKill.sync_total} -> ${duringOutage.sync_total} (the adapter really could not reach it). ` +
      'The refusal count is what proves the adapter kept TRYING rather than having quietly died.',
  );

  if (args.controlNoRecovery) {
    // NEGATIVE CONTROL: never restore the link. The window is unreachable, so
    // the loss detector MUST fire. A green here means the detector is dead.
    d.note('CONTROL MODE: the upstream is NOT restored — the gap must be reported LOST');
  } else {
    d.proxy.restore();
    d.note('upstream RESTORED');
  }

  // ── recovery ──────────────────────────────────────────────────────────────
  const recovered = d.waitForSyncs(duringOutage.sync_total ?? 0, 2, 60_000, 'post-outage syncs');
  const recoveredOk = (recovered.sync_total ?? 0) > (duringOutage.sync_total ?? 0);
  d.record(
    'adapter-reconnected',
    args.controlNoRecovery ? !recoveredOk : recoveredOk,
    `homeserver sync_total ${duringOutage.sync_total} -> ${recovered.sync_total}; proxy ${JSON.stringify(d.proxy.stats())}. ` +
      (args.controlNoRecovery ? 'CONTROL MODE: expected NO recovery.' : ''),
  );

  const duringGot = d.waitForTokens(during, args.budgetMs, 'gap messages');

  // ── AFTER ─────────────────────────────────────────────────────────────────
  const after = [d.submit({ phase: 'after', expect: 'reply' })];
  const afterGot = d.waitForTokens(after, args.budgetMs, 'post-recovery control');
  d.record(
    'post-reconnect-control',
    args.controlNoRecovery ? afterGot === 0 : afterGot === after.length,
    `${afterGot}/${after.length} produced a turn. ` +
      (args.controlNoRecovery
        ? 'CONTROL MODE: expected zero, because the link was never restored.'
        : 'KNOWN-POSITIVE #2: the adapter is ALIVE after the outage, so a zero on the gap cannot be a dead adapter.'),
  );

  sleep(6_000);
  const c = census(d.plan, d.llmSeen());
  const finalReport = d.mxReport();

  d.record(
    'gap-messages-survive-the-upstream-drop',
    args.controlNoRecovery ? c.lost.length >= 2 : duringGot === during.length && c.lost.length === 0,
    `${duringGot}/${during.length} of the messages the HOMESERVER received while our link was dead produced a turn. lost=[${c.lost.join(',')}]. ` +
      (args.controlNoRecovery ? 'CONTROL MODE: a loss here is the REQUIRED result.' : ''),
  );

  d.record(
    'no-duplicate-turns-around-the-window',
    c.duplicated.length === 0,
    `duplicated=[${c.duplicated.join(',')}] over ${d.plan.filter((p) => p.expect === 'reply').length} expected-reply tokens. ` +
      'Matrix re-serves an event under the SAME event_id in a later batch, so a cursor that rewinds too far shows here.',
  );

  d.record(
    'decoy-and-phantom-score-zero',
    c.leaked.length === 0,
    `decoy (denied mxid, submitted INSIDE the window) and phantom (never submitted) both scored zero: leaked=[${c.leaked.join(',')}].`,
  );

  const failedLegs = d.results.filter((r) => !r.ok);
  const out = {
    schema: RESULT_SCHEMA,
    adapter: 'matrix',
    mode: args.controlNoRecovery ? 'CONTROL-no-recovery' : 'measurement',
    binary: args.binary,
    legs_total: d.results.length,
    legs_failed: failedLegs.length,
    verdict: failedLegs.length === 0 ? 'PASS' : 'FAIL',
    census: c,
    plan: d.plan,
    phantom,
    proxy: d.proxy.stats(),
    killed_connections: killed,
    matrix_report: finalReport,
    results: d.results,
    notes: d.notes,
  };
  fs.writeFileSync(path.join(args.runDir, 'result.json'), `${JSON.stringify(out, null, 2)}\n`);
  process.stdout.write(
    `\nF24RECONNECT-POLL(matrix) ${out.verdict} legs=${d.results.length - failedLegs.length}/${d.results.length} ` +
      `lost=${c.lost.length} duplicated=${c.duplicated.length} leaked=${c.leaked.length} mode=${out.mode}\n`,
  );
  d.cleanup();
  process.exit(failedLegs.length === 0 ? 0 : 1);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main();
}
