#!/usr/bin/env node
// F24-RECONNECT — the UPSTREAM-DROP half of 24-C3's `reconnect/reload` clause.
//
// The `reload` half (new config picked up by `channel reload`) is done: driven
// by 24-C3-FINISH, which found F24-C3-H5, and fixed by 24-H5. The `reconnect`
// half — the REMOTE side going away under a still-running process — has never
// been driven by any lane. 24-H6 restarted the PROCESS, which is a different
// event: a process restart can only recover from state on disk, whereas an
// in-process reconnect recovers from state in memory and may never touch a
// file. H6's cursor fix therefore does not imply this.
//
// THE QUESTION IS NOT "DOES IT RECONNECT". A health probe answers that, and
// answers it green for an adapter that reconnects and silently drops the window.
// F24-C3-H5 is precisely that shape: reload registered the adapter, reported it
// healthy, and denied every message to it. So the question is:
//
//     is every message delivered AROUND the disconnect window accounted for,
//     exactly once?
//
// Hence a census. N before, M during, K after, every one carrying its own
// correlation token, and losses/duplicates DERIVED here from the fixture's own
// journals. No figure the product reports about itself is trusted.
//
// ── why every leg carries a control ─────────────────────────────────────────
// The headline claim is an ABSENCE ("nothing was lost"). LANE-BRIEF §3b-i: an
// absence is the easiest assertion to pass without doing any work — a dead
// client, a fixture that never dispatched, a drop that never happened, a
// matcher that matches nothing, all produce it for free. So:
//
//   · every zero is paired with a known-positive in the SAME run;
//   · a DECOY token is planted that must score zero — if it scores, the
//     detector over-matches and no zero in the run is readable;
//   · a PHANTOM token is never dispatched at all and must score zero — if it
//     scores, the matcher answers yes to anything;
//   · the drop is CONFIRMED from the fixture's journal, never assumed;
//   · `--control-no-replay` is the one-variable negative control: the fixture
//     accepts RESUME and replays NOTHING, so the gap is genuinely unreachable
//     and the loss detector MUST fire. A run of this mode that reports no loss
//     means the detector is dead and the main run's green is worthless.
//
// usage:
//   f24-reconnect.mjs --binary <path> --run-dir <dir> [--budget-ms N]
//                     [--control-no-replay]
// exit: 0 GREEN, 1 RED, 2 USAGE, 3 NOT MEASURED (instrument could not run)

import { execFileSync, spawn } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { matchesToken } from './f24-discord-inbound.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));

export const RESULT_SCHEMA = 'wayland.reconnect.upstream-drop/1';

export const LEGS = [
  'pre-drop-control',
  'upstream-drop-really-happened',
  'adapter-reconnected',
  'gap-messages-survive-the-upstream-drop',
  'no-duplicate-turns-around-the-window',
  'post-reconnect-control',
  'decoy-and-phantom-score-zero',
];

// `Atomics.wait` blocks the whole event loop, which is why every fixture here
// runs as its own OS process and is reached over HTTP.
function sleep(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

function control(url, method, body) {
  const args = ['-s', '-X', method, url];
  if (body !== undefined) {
    args.push('-H', 'content-type: application/json', '-d', JSON.stringify(body));
  }
  const out = execFileSync('curl', args, { encoding: 'utf8', timeout: 15_000 });
  return JSON.parse(out);
}

/**
 * The census. Given the planted plan and the fixture's outbound reply journal,
 * derive per-token reply counts, then losses and duplicates.
 *
 * Exported and pure so the self-test can drive it with synthetic journals —
 * a census that can only be exercised through a 3-minute live run is a census
 * nobody proves can fail.
 */
export function census(plan, replies) {
  const rows = plan.map((p) => ({
    token: p.token,
    phase: p.phase,
    expect: p.expect, // 'reply' | 'silence'
    replies: replies.filter((r) => matchesToken(r.content, p.token)).length,
  }));
  return {
    rows,
    // A token that should have produced a turn and produced none.
    lost: rows.filter((r) => r.expect === 'reply' && r.replies === 0).map((r) => r.token),
    // A token that produced more than one turn. The product's inbound dedupe
    // cache (60 s, `bootstrap.rs`) should collapse a replayed duplicate, so a
    // second turn here is a real duplicate escaping that cache.
    duplicated: rows.filter((r) => r.expect === 'reply' && r.replies > 1).map((r) => r.token),
    // A token that should have been SILENT and was not. This is the detector's
    // own failure mode, not the product's: it means the matcher over-matches
    // and every zero elsewhere in the run is unreadable.
    leaked: rows.filter((r) => r.expect === 'silence' && r.replies > 0).map((r) => r.token),
  };
}

class Driver {
  constructor(args) {
    this.args = args;
    this.home = path.join(args.runDir, 'home');
    this.results = [];
    this.notes = [];
    this.plan = [];
    this.children = [];
    this.child = null;
    this.tag = crypto.randomBytes(3).toString('hex');
    this.senderId = '5150001';
    this.strangerId = '7770002';
    this.chanA = '900000001';
    // Minted per run into a throwaway isolated profile; dies with the run.
    // Without it a headless host has no unlocked vault and EVERY inbound turn
    // dies with "Session persistence authority unavailable" AFTER the message
    // was admitted — the inbound path works and the run still reports zero
    // replies, which reads as total loss.
    this.vaultPassphrase = crypto.randomBytes(24).toString('hex');
    this.botToken = `f24rc-${crypto.randomBytes(12).toString('hex')}`;
  }

  note(m) {
    this.notes.push(m);
    process.stdout.write(`  · ${m}\n`);
  }

  record(leg, ok, detail) {
    this.results.push({ leg, ok, detail });
    process.stdout.write(`${ok ? 'PASS' : 'FAIL'} ${leg} — ${detail}\n`);
  }

  // ── setup ─────────────────────────────────────────────────────────────────

  startFixture() {
    const logPath = path.join(this.args.runDir, 'fixture.log');
    fs.writeFileSync(logPath, '');
    const fd = fs.openSync(logPath, 'a');
    const child = spawn(
      process.execPath,
      [path.join(HERE, 'f24-discord-fixture.mjs'), '--token', this.botToken, '--heartbeat-ms', '5000'],
      { stdio: ['ignore', fd, fd], windowsHide: true },
    );
    this.children.push(child);
    for (let i = 0; i < 120; i += 1) {
      const m = /http:\/\/127\.0\.0\.1:(\d+)/.exec(fs.readFileSync(logPath, 'utf8'));
      if (m) {
        this.fxApi = m[0];
        this.fxGateway = `ws://127.0.0.1:${m[1]}`;
        this.note(`fixture up on ${this.fxApi} (ephemeral port — no cross-lane collision)`);
        return;
      }
      sleep(100);
    }
    throw new Error('discord fixture never announced a URL');
  }

  startLlm() {
    this.llmJournal = path.join(this.args.runDir, 'llm-journal.jsonl');
    fs.writeFileSync(this.llmJournal, '');
    const logPath = path.join(this.args.runDir, 'llm.log');
    fs.writeFileSync(logPath, '');
    const out = fs.openSync(logPath, 'a');
    this.llm = spawn(
      process.execPath,
      [path.join(HERE, 'f24-llm-fixture.mjs'), '--port', '0', '--journal', this.llmJournal],
      { stdio: ['ignore', out, out] },
    );
    this.children.push(this.llm);
    for (let i = 0; i < 120; i += 1) {
      const m = /http:\/\/127\.0\.0\.1:\d+/.exec(fs.readFileSync(logPath, 'utf8'));
      if (m) {
        this.llmUrl = m[0];
        return;
      }
      sleep(100);
    }
    throw new Error('llm fixture never announced a URL');
  }

  writeConfig() {
    fs.mkdirSync(path.join(this.home, 'channels'), { recursive: true });
    fs.writeFileSync(
      path.join(this.home, 'credentials.toml'),
      ['[secrets]', `"discord.f24rc.bot_token" = "${this.botToken}"`, ''].join('\n'),
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
        // No webhook host is needed — discord is a gateway (WebSocket) adapter.
        // Leaving it disabled also means this driver binds NO fixed port at all
        // and is safe to run alongside other lanes.
        '[inbound_webhook]',
        'enabled = false',
        '',
      ].join('\n'),
      { mode: 0o600 },
    );
    fs.writeFileSync(
      path.join(this.home, 'channels', 'f24rcdisc.toml'),
      [
        'name = "f24rcdisc"',
        'platform = "discord"',
        'enabled = true',
        '',
        '[options]',
        'credential_handle = "discord.f24rc.bot_token"',
        `api_base_url = "${this.fxApi}"`,
        `gateway_url = "${this.fxGateway}"`,
        // Deliberately short. The heartbeat grace is how a destroyed socket is
        // NOTICED when the adapter happens to be idle between reads; a 30 s
        // grace would put the reconnect outside a sane budget and the run would
        // read as "never reconnected" when it simply had not looked yet.
        'heartbeat_grace_ms = 15000',
        '',
        '[inbound]',
        'dm = "allowlist"',
        `dm_allowlist = ["${this.senderId}"]`,
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
        RUST_LOG: 'info,wcore_channel_discord=debug',
      },
    });
  }

  report() {
    return control(`${this.fxApi}/__control/report`, 'GET');
  }

  replies() {
    return control(`${this.fxApi}/__control/replies`, 'GET').sent;
  }

  /** Plant a message and register it in the census plan. */
  send({ phase, expect, authorId }) {
    const token = `f24rc-${this.tag}-${phase}-${this.plan.length}`;
    this.plan.push({ token, phase, expect });
    const sockets = control(`${this.fxApi}/__control/dispatch`, 'POST', {
      id: `${Date.now()}${crypto.randomBytes(3).toString('hex')}`,
      channelId: this.chanA,
      content: `hello ${token}`,
      authorId: authorId ?? this.senderId,
    }).sockets;
    this.note(`sent ${token} (phase=${phase} expect=${expect}) -> ${sockets} socket(s)`);
    return { token, sockets };
  }

  /** Register a token that is NEVER dispatched. Must score zero. */
  plantPhantom() {
    const token = `f24rc-${this.tag}-phantom-never-dispatched`;
    this.plan.push({ token, phase: 'phantom', expect: 'silence' });
    return token;
  }

  waitForIdentify(budgetMs) {
    const deadline = Date.now() + budgetMs;
    let i = 0;
    while (Date.now() < deadline) {
      i += 1;
      const rep = this.report();
      if (rep.identify_count > 0 && rep.live_gateway_connections > 0) {
        this.note(`IDENTIFYed after ~${i * 250}ms (tcp=${rep.tcp_connections} live=${rep.live_gateway_connections})`);
        return true;
      }
      if (this.child && this.child.exitCode !== null) {
        this.note(`binary exited early rc=${this.child.exitCode}`);
        return false;
      }
      if (i % 20 === 0) process.stdout.write(`  waiting for IDENTIFY: ${i * 250}ms\n`);
      sleep(250);
    }
    return false;
  }

  /** Wait until every token in `tokens` has at least one reply, or the budget expires. */
  waitForReplies(tokens, budgetMs, label) {
    const deadline = Date.now() + budgetMs;
    let i = 0;
    for (;;) {
      i += 1;
      const sent = this.replies();
      const got = tokens.filter((t) => sent.some((r) => matchesToken(r.content, t)));
      if (got.length === tokens.length) {
        this.note(`${label}: ${got.length}/${tokens.length} replied after ~${i * 500}ms`);
        return got.length;
      }
      if (Date.now() >= deadline) {
        this.note(`${label}: ${got.length}/${tokens.length} replied — BUDGET EXPIRED after ${budgetMs}ms`);
        return got.length;
      }
      if (i % 10 === 0) process.stdout.write(`  awaiting ${label}: ${got.length}/${tokens.length}\n`);
      sleep(500);
    }
  }

  /** Wait for the adapter to come back after the drop. Returns the observation. */
  waitForReconnect(baseline, budgetMs) {
    const deadline = Date.now() + budgetMs;
    let i = 0;
    while (Date.now() < deadline) {
      i += 1;
      const rep = this.report();
      if (rep.live_gateway_connections > 0 && rep.total_gateway_connections > baseline.total_gateway_connections) {
        return {
          reconnected: true,
          ms: i * 250,
          resumes: rep.resume_count - baseline.resume_count,
          identifies: rep.identify_count - baseline.identify_count,
          report: rep,
        };
      }
      if (this.child && this.child.exitCode !== null) {
        return { reconnected: false, ms: i * 250, binaryExited: this.child.exitCode, report: rep };
      }
      if (i % 20 === 0) process.stdout.write(`  waiting for reconnect: ${i * 250}ms\n`);
      sleep(250);
    }
    return { reconnected: false, ms: budgetMs, report: this.report() };
  }

  cleanup() {
    for (const c of [this.child, ...this.children]) {
      try {
        c?.kill('SIGKILL');
      } catch {
        /* noop */
      }
    }
  }
}

// ── the run ──────────────────────────────────────────────────────────────────

function main() {
  const args = { binary: null, runDir: null, budgetMs: 60_000, controlNoReplay: false };
  const argv = process.argv.slice(2);
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--binary') args.binary = argv[++i];
    else if (a === '--run-dir') args.runDir = argv[++i];
    else if (a === '--budget-ms') args.budgetMs = Number(argv[++i]);
    else if (a === '--control-no-replay') args.controlNoReplay = true;
    else {
      process.stderr.write(`unknown arg ${a}\n`);
      process.exit(2);
    }
  }
  if (!args.binary || !args.runDir) {
    process.stderr.write('usage: f24-reconnect.mjs --binary <path> --run-dir <dir>\n');
    process.exit(2);
  }
  fs.mkdirSync(args.runDir, { recursive: true });

  const d = new Driver(args);
  let notMeasured = null;

  try {
    d.startFixture();
    d.startLlm();
    d.writeConfig();
    d.startGateway();

    if (!d.waitForIdentify(60_000)) {
      notMeasured = 'the binary never IDENTIFYed on the gateway fixture — NOT MEASURED, nothing about reconnect is readable';
      throw new Error(notMeasured);
    }

    if (args.controlNoReplay) {
      // THE NEGATIVE CONTROL. RESUME is still accepted; the replay is not sent.
      // The gap is therefore genuinely unreachable and the census MUST report
      // it lost. A green here means the loss detector is dead.
      control(`${d.fxApi}/__control/replay`, 'POST', { enabled: false });
      d.note('CONTROL MODE: fixture will accept RESUME and replay NOTHING — the gap must be reported LOST');
    }

    const phantom = d.plantPhantom();

    // ── BEFORE ──────────────────────────────────────────────────────────────
    const before = [d.send({ phase: 'before', expect: 'reply' }), d.send({ phase: 'before', expect: 'reply' })];
    const beforeTokens = before.map((x) => x.token);
    const beforeGot = d.waitForReplies(beforeTokens, args.budgetMs, 'pre-drop control');
    d.record(
      'pre-drop-control',
      beforeGot === beforeTokens.length && before.every((x) => x.sockets === 1),
      `${beforeGot}/${beforeTokens.length} replied; each dispatch reached ${before.map((x) => x.sockets).join('/')} socket(s). ` +
        'KNOWN-POSITIVE: if this is not full, no zero anywhere else in this run is readable.',
    );

    const baseline = d.report();

    // ── THE DROP ────────────────────────────────────────────────────────────
    const dropped = control(`${d.fxApi}/__control/drop`, 'POST').dropped;
    sleep(500);
    const afterDrop = d.report();
    d.record(
      'upstream-drop-really-happened',
      dropped >= 1 && afterDrop.live_gateway_connections === 0,
      `destroyed ${dropped} socket(s) with no WS close frame; live_gateway_connections ${baseline.live_gateway_connections} -> ${afterDrop.live_gateway_connections}. ` +
        'Confirmed from the fixture journal, not assumed — a probe that grades a disconnect it cannot confirm grades an event that may never have occurred.',
    );

    // ── DURING — the window ────────────────────────────────────────────────
    const during = [d.send({ phase: 'during', expect: 'reply' }), d.send({ phase: 'during', expect: 'reply' })];
    const duringTokens = during.map((x) => x.token);
    // The DECOY: dispatched inside the same window from a sender the channel's
    // allowlist excludes. It must score zero. If it replies, the census
    // over-matches and every zero in this run is worthless.
    const decoy = d.send({ phase: 'during-decoy', expect: 'silence', authorId: d.strangerId });
    const windowReachedSockets = [...during, decoy].map((x) => x.sockets);

    // ── RECONNECT ───────────────────────────────────────────────────────────
    const rc = d.waitForReconnect(baseline, 60_000);
    d.record(
      'adapter-reconnected',
      rc.reconnected,
      rc.reconnected
        ? `back after ~${rc.ms}ms via ${rc.resumes > 0 ? `RESUME (+${rc.resumes})` : `IDENTIFY (+${rc.identifies})`}; ` +
          `total gateway connections ${baseline.total_gateway_connections} -> ${rc.report.total_gateway_connections}`
        : `NO reconnect within 60000ms (binaryExited=${rc.binaryExited ?? 'no'}, live=${rc.report.live_gateway_connections})`,
    );

    // ── the measurement ─────────────────────────────────────────────────────
    const duringGot = d.waitForReplies(duringTokens, args.budgetMs, 'gap messages');

    // ── AFTER ───────────────────────────────────────────────────────────────
    const after = [d.send({ phase: 'after', expect: 'reply' })];
    const afterTokens = after.map((x) => x.token);
    const afterGot = d.waitForReplies(afterTokens, args.budgetMs, 'post-reconnect control');
    d.record(
      'post-reconnect-control',
      afterGot === afterTokens.length,
      `${afterGot}/${afterTokens.length} replied. KNOWN-POSITIVE #2: proves the adapter is ALIVE after the drop, ` +
        'so a zero on the gap messages cannot be explained by a dead adapter.',
    );

    // Settle: give any late/duplicate turn a chance to land before counting.
    // Counting duplicates immediately would report zero for free.
    sleep(8_000);

    const replies = d.replies();
    const c = census(d.plan, replies);
    const finalReport = d.report();

    d.record(
      'gap-messages-survive-the-upstream-drop',
      duringGot === duringTokens.length && c.lost.filter((t) => duringTokens.includes(t)).length === 0,
      `${duringGot}/${duringTokens.length} of the messages delivered DURING the disconnect window produced a turn. ` +
        `Each reached ${windowReachedSockets.slice(0, 2).join('/')} live socket(s) at dispatch time (0 = there really was no connection). ` +
        `fixture replayed ${finalReport.resume_replayed_total} message(s) on RESUME. lost=[${c.lost.join(',')}]`,
    );

    d.record(
      'no-duplicate-turns-around-the-window',
      c.duplicated.length === 0,
      `duplicated=[${c.duplicated.join(',')}] over ${d.plan.filter((p) => p.expect === 'reply').length} expected-reply tokens. ` +
        `Fixture-side raw deliveries ${finalReport.dispatch_socket_deliveries} vs ${finalReport.dispatched_total} dispatches ` +
        '(a raw duplicate the product deduped is visible here even when the turn count is 1).',
    );

    d.record(
      'decoy-and-phantom-score-zero',
      c.leaked.length === 0,
      `decoy (non-allowlisted sender, dispatched INSIDE the window) and phantom (never dispatched) both scored zero: leaked=[${c.leaked.join(',')}]. ` +
        'This is what makes the zeros above measurements rather than free.',
    );

    const failedLegs = d.results.filter((r) => !r.ok);
    const out = {
      schema: RESULT_SCHEMA,
      mode: args.controlNoReplay ? 'CONTROL-no-replay' : 'measurement',
      binary: args.binary,
      legs_total: d.results.length,
      legs_failed: failedLegs.length,
      verdict: failedLegs.length === 0 ? 'PASS' : 'FAIL',
      census: c,
      plan: d.plan,
      phantom,
      dropped_sockets: dropped,
      reconnect: { ...rc, report: undefined },
      fixture_report: finalReport,
      results: d.results,
      notes: d.notes,
      replies_seen: replies.length,
    };
    fs.writeFileSync(path.join(args.runDir, 'result.json'), `${JSON.stringify(out, null, 2)}\n`);
    process.stdout.write(
      `\nF24RECONNECT ${out.verdict} legs=${d.results.length - failedLegs.length}/${d.results.length} ` +
        `lost=${c.lost.length} duplicated=${c.duplicated.length} leaked=${c.leaked.length} mode=${out.mode}\n`,
    );
    d.cleanup();
    process.exit(failedLegs.length === 0 ? 0 : 1);
  } catch (e) {
    d.cleanup();
    fs.writeFileSync(
      path.join(args.runDir, 'result.json'),
      `${JSON.stringify({ schema: RESULT_SCHEMA, verdict: 'NOT MEASURED', error: String(e?.message ?? e), notes: d.notes, results: d.results }, null, 2)}\n`,
    );
    process.stderr.write(`NOT MEASURED: ${String(e?.stack ?? e)}\n`);
    process.exit(3);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main();
}
