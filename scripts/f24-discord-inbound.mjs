#!/usr/bin/env node
// F24-C3-DISCORD — drive the SHIPPED binary's Discord inbound path across the
// same legs the other adapters are driven across, with no vendor credential.
//
// Discord is the last adapter in the Phase 24 matrix whose inbound path had
// never been driven at all. Three lanes recorded that it could not be, because
// it needed a bot token belonging to a human. What it actually needed was a
// config seam (now landed: `DiscordConfig::{api_base_url,gateway_url}`) and a
// Gateway fixture (`f24-discord-fixture.mjs`). The token is minted at run time
// and accepted because the fixture is the API.
//
// LEGS. The five the matrix defines, plus one this adapter specifically needs:
//   admit   an allowed sender's DM produces exactly one reply carrying that
//           message's own correlation token
//   dedupe  the SAME message id replayed produces no second reply
//           (positive control: a different id from the same sender does)
//   access  a sender outside the allowlist produces zero replies
//           (positive control: the allowed sender in the same run does)
//   bind    two distinct conversations produce two replies whose channel ids
//           are distinct and correct — a reply in the wrong channel fails
//   route   the reply text carries the token of the message that caused it
//   steady  messages submitted AFTER a settle period, not at startup
//
// WHY `steady` IS NOT OPTIONAL. F24-C3-H4 found 5-of-6 inbound messages lost on
// Telegram in STEADY STATE with no error logged anywhere, and that is precisely
// what lifted it from MEDIUM to HIGH. A startup-only run would have missed it.
// Discord has never been driven, so nothing yet says whether the double-manager
// fix covers it or whether Discord has a second loss mode of its own.
//
// THE RACE HAS THE OPPOSITE SHAPE HERE, AND THE NAIVE PORT WOULD MEASURE
// NOTHING. Telegram polls a destructive server-side queue, so two
// ChannelManagers => one STEALS => LOSS. Discord is pushed per session: two
// managers open two sockets and Discord delivers to BOTH => DUPLICATION. The
// instrument therefore counts concurrent authenticated gateway connections
// (the analogue of `max_concurrent_getupdates`) and grades loss and
// duplication SEPARATELY. A driver that only counted loss would report Discord
// clean under the exact defect it was built to find.
//
// GREEN BY UNIVERSAL DENIAL IS THE TOP TRAP. Every leg here has a positive
// control in the same run, and a run with zero arrivals grades FAIL, never
// pass. A runtime that connected nothing shows up as
// max_concurrent_gateway_connections=0, which is a distinct and failing answer.
//
// usage: f24-discord-inbound.mjs --binary <path> --run-dir <dir> [--budget-ms N]

import { spawn } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { DiscordFixture } from './f24-discord-fixture.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));

export const LEGS = ['admit', 'dedupe', 'access', 'bind', 'route', 'steady'];
export const RESULT_SCHEMA = 'wayland.discord.inbound/1';

function sleep(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

// ── the matcher (the instrument) ─────────────────────────────────────────────
//
// THE OLD MATCHER WAS `text.includes(token)`, AND IT HAS ALREADY DESTROYED ONE
// MEASUREMENT PASS ON THIS PROGRAM. On Telegram, MarkdownV2 escaping turned
// `f24c3-h4-pre-0-ab12` into `f24c3\-h4\-pre\-0\-ab12`, and the driver reported
// `replied=0/8` for eight replies that had all arrived. On top of that, a
// console line wrap put a newline INSIDE a searched phrase in a later lane and
// the same class of miss recurred.
//
// Discord does not escape outbound content today (`rest::CreateMessageBody`
// sends `content` raw), so the naive matcher would happen to work RIGHT NOW.
// That is not a reason to ship it: it would break silently the moment anything
// wraps, escapes or splits the text — including Discord's own 2000-char cap,
// which can cut a token in half. The instrument is repaired here rather than
// noted, because a written-up instrument defect is a defect you have agreed to
// keep (this is the 11th recorded instance of an instrument carrying the
// defect class it hunts, and the first proven to have RECURRED because the
// earlier sighting was documented instead of fixed).
export function normalizeForMatch(s) {
  return String(s ?? '')
    .replace(/\\(.)/g, '$1') // un-escape backslash escaping (MarkdownV2 etc.)
    .replace(/\s+/g, '') // defeat line wraps / newlines splicing a token
    .toLowerCase();
}

export function matchesToken(text, token) {
  return normalizeForMatch(text).includes(normalizeForMatch(token));
}

/** The pre-repair matcher, kept ONLY so the self-test can prove the repair does something. */
export function naiveMatch(text, token) {
  return String(text ?? '').includes(token);
}

// ── driver ───────────────────────────────────────────────────────────────────

class Driver {
  constructor(args) {
    this.args = args;
    this.home = path.join(args.runDir, 'home');
    this.results = [];
    this.notes = [];
    this.tag = crypto.randomBytes(3).toString('hex');
    this.senderId = '5150001';
    this.strangerId = '7770002';
    this.chanA = '900000001';
    this.chanB = '900000002';
    this.child = null;
    this.llm = null;
  }

  note(m) {
    this.notes.push(m);
    process.stdout.write(`  · ${m}\n`);
  }

  record(leg, ok, detail) {
    this.results.push({ leg, ok, detail });
    process.stdout.write(`${ok ? 'PASS' : 'FAIL'} ${leg} — ${detail}\n`);
  }

  // ── setup ──────────────────────────────────────────────────────────────────

  startLlm() {
    this.llmJournal = path.join(this.args.runDir, 'llm-journal.jsonl');
    fs.writeFileSync(this.llmJournal, '');
    const logPath = path.join(this.args.runDir, 'llm.log');
    fs.writeFileSync(logPath, '');
    const out = fs.openSync(logPath, 'a');
    this.llm = spawn(
      process.execPath,
      [path.join(HERE, 'f24-llm-fixture.mjs'), '--port', '0', '--journal', this.llmJournal],
      { stdio: ['ignore', out, out], detached: false },
    );
    // The fixture prints its listening URL on the first line of its log.
    for (let i = 0; i < 100; i += 1) {
      const t = fs.readFileSync(logPath, 'utf8');
      const m = /http:\/\/127\.0\.0\.1:\d+/.exec(t);
      if (m) return m[0];
      sleep(100);
    }
    throw new Error('llm fixture never announced a URL');
  }

  writeConfig() {
    fs.mkdirSync(path.join(this.home, 'channels'), { recursive: true });

    fs.writeFileSync(
      path.join(this.home, 'credentials.toml'),
      ['[secrets]', `"discord.f24c3.bot_token" = "${this.fx.botToken}"`, ''].join('\n'),
      { mode: 0o600 },
    );

    fs.writeFileSync(
      path.join(this.home, 'config.toml'),
      [
        '[default]',
        'provider = "f24c3discfixture"',
        '',
        '[providers.f24c3discfixture]',
        'provider = "openai"',
        'model = "f24c3-fixture"',
        'api_key = "f24c3-not-a-real-key"',
        `base_url = "${this.llmUrl}"`,
        '',
        '[inbound_webhook]',
        'enabled = false',
        '',
      ].join('\n'),
      { mode: 0o600 },
    );

    fs.writeFileSync(
      path.join(this.home, 'channels', 'f24c3disc.toml'),
      [
        'name = "f24c3disc"',
        'platform = "discord"',
        'enabled = true',
        '',
        '[options]',
        'credential_handle = "discord.f24c3.bot_token"',
        // THE SEAM. Both lines are new in this lane. Without them this file is
        // a `deny_unknown_fields` parse error and the adapter can only ever
        // reach discord.com / gateway.discord.gg — which is exactly why the
        // Discord inbound path had never been driven.
        `api_base_url = "${this.fx.apiBase}"`,
        `gateway_url = "${this.fx.gatewayUrl}"`,
        'heartbeat_grace_ms = 30000',
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
        RUST_LOG: 'info,wcore_channel_discord=debug',
      },
      detached: false,
    });
  }

  /** Wait until the binary has actually IDENTIFYed on the gateway. */
  waitForGateway(budgetMs) {
    const deadline = Date.now() + budgetMs;
    let i = 0;
    while (Date.now() < deadline) {
      i += 1;
      if (this.fx.identifyCount > 0 && [...this.fx.conns].some((c) => c.identified)) {
        this.note(`gateway IDENTIFYed after ~${i * 250}ms (conns=${this.fx.conns.size})`);
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

  /** Wait for a reply carrying `token` to be POSTed back through the fixture. */
  waitForReply(token, budgetMs) {
    const deadline = Date.now() + budgetMs;
    while (Date.now() < deadline) {
      const hit = this.fx.sent.filter((s) => matchesToken(s.content, token));
      if (hit.length > 0) return hit;
      sleep(200);
    }
    return this.fx.sent.filter((s) => matchesToken(s.content, token));
  }

  msg({ token, channelId, authorId, id }) {
    return this.fx.dispatchMessage({
      id: id ?? `${Date.now()}${crypto.randomBytes(2).toString('hex')}`,
      channelId: channelId ?? this.chanA,
      content: `hello ${token}`,
      authorId: authorId ?? this.senderId,
    });
  }

  // ── legs ───────────────────────────────────────────────────────────────────

  run() {
    const budget = this.args.budgetMs;

    // ADMIT
    const tAdmit = `f24c3-disc-admit-${this.tag}`;
    const sockets = this.msg({ token: tAdmit });
    const admit = this.waitForReply(tAdmit, budget);
    this.record(
      'admit',
      admit.length === 1,
      `replies=${admit.length} (want exactly 1), dispatched_to_sockets=${sockets}`,
    );

    // DEDUPE — same platform message id replayed.
    const tDup = `f24c3-disc-dedupe-${this.tag}`;
    const dupId = `dup${Date.now()}`;
    this.msg({ token: tDup, id: dupId });
    this.waitForReply(tDup, budget);
    const before = this.fx.sent.filter((s) => matchesToken(s.content, tDup)).length;
    this.msg({ token: tDup, id: dupId }); // identical id — must not produce a 2nd
    sleep(Math.min(8000, budget));
    const after = this.fx.sent.filter((s) => matchesToken(s.content, tDup)).length;
    // POSITIVE CONTROL: a DIFFERENT id from the same sender still gets through,
    // so "no second reply" cannot be satisfied by an adapter that simply died.
    const tCtl = `f24c3-disc-dedupectl-${this.tag}`;
    this.msg({ token: tCtl });
    const ctl = this.waitForReply(tCtl, budget);
    this.record(
      'dedupe',
      before === 1 && after === before && ctl.length === 1,
      `first=${before} after_replay=${after} (want unchanged), positive_control=${ctl.length} (want 1)`,
    );

    // ACCESS — a sender outside the DM allowlist.
    const tDeny = `f24c3-disc-access-${this.tag}`;
    this.msg({ token: tDeny, authorId: this.strangerId });
    sleep(Math.min(8000, budget));
    const denied = this.fx.sent.filter((s) => matchesToken(s.content, tDeny));
    const tAllow = `f24c3-disc-accessctl-${this.tag}`;
    this.msg({ token: tAllow });
    const allowed = this.waitForReply(tAllow, budget);
    this.record(
      'access',
      denied.length === 0 && allowed.length === 1,
      `stranger_replies=${denied.length} (want 0), allowed_control=${allowed.length} (want 1)`,
    );

    // BIND — two distinct conversations.
    const tA = `f24c3-disc-binda-${this.tag}`;
    const tB = `f24c3-disc-bindb-${this.tag}`;
    this.msg({ token: tA, channelId: this.chanA });
    this.msg({ token: tB, channelId: this.chanB });
    const rA = this.waitForReply(tA, budget);
    const rB = this.waitForReply(tB, budget);
    const okBind =
      rA.length === 1 &&
      rB.length === 1 &&
      rA[0].channel_id === this.chanA &&
      rB[0].channel_id === this.chanB;
    this.record(
      'bind',
      okBind,
      `A->${rA[0]?.channel_id ?? 'none'} (want ${this.chanA}), B->${rB[0]?.channel_id ?? 'none'} (want ${this.chanB})`,
    );

    // ROUTE — the reply must carry ITS OWN cause's token, not some other turn's.
    const okRoute =
      rA.length === 1 &&
      rB.length === 1 &&
      matchesToken(rA[0].content, tA) &&
      !matchesToken(rA[0].content, tB) &&
      matchesToken(rB[0].content, tB) &&
      !matchesToken(rB[0].content, tA);
    this.record(
      'route',
      okRoute,
      `A_reply_carries_A=${rA.length === 1 && matchesToken(rA[0].content, tA)}, ` +
        `A_reply_leaks_B=${rA.length === 1 && matchesToken(rA[0].content, tB)}`,
    );

    // STEADY STATE — the leg that raised the Telegram finding to HIGH.
    this.note(`settling ${this.args.settleMs}ms before the steady-state leg`);
    sleep(this.args.settleMs);
    const steadyTokens = [];
    for (let i = 0; i < this.args.steady; i += 1) {
      const t = `f24c3-disc-steady${i}-${this.tag}`;
      steadyTokens.push(t);
      this.msg({ token: t });
      sleep(1200);
    }
    const deadline = Date.now() + budget;
    while (Date.now() < deadline) {
      const got = steadyTokens.filter(
        (t) => this.fx.sent.filter((s) => matchesToken(s.content, t)).length > 0,
      ).length;
      if (got === steadyTokens.length) break;
      sleep(500);
    }
    const perSteady = steadyTokens.map((t) => ({
      token: t,
      replies: this.fx.sent.filter((s) => matchesToken(s.content, t)).length,
    }));
    const lost = perSteady.filter((p) => p.replies === 0);
    const duped = perSteady.filter((p) => p.replies > 1);
    this.record(
      'steady',
      lost.length === 0 && duped.length === 0,
      `submitted=${steadyTokens.length} answered=${perSteady.length - lost.length} ` +
        `lost=${lost.length} duplicated=${duped.length}`,
    );

    return { perSteady, lost, duped };
  }

  // ── grading ────────────────────────────────────────────────────────────────

  grade(steady) {
    const rep = this.fx.report();
    const llmTurns = fs.existsSync(this.llmJournal)
      ? fs.readFileSync(this.llmJournal, 'utf8').trim().split('\n').filter(Boolean).length
      : 0;

    // INSTRUMENT FAULT. A suspect run must grade INCOMPLETE, never LOSS.
    //
    // The distinction matters because "the product lost messages" and "my
    // matcher could not read the replies" look identical in a bare count, and
    // this program has already published the second as the first once.
    const faults = [];
    const repliesExist = rep.sent_total > 0;
    const anyMatched = this.results.some((r) => r.ok);
    if (repliesExist && !anyMatched) {
      faults.push(
        'replies were POSTed back but no leg matched a token — matcher is suspect, not the product',
      );
    }
    // The specific defect class that has bitten twice: raw match fails where
    // the normalized one succeeds. If that is happening, the naive matcher is
    // actively lying and any run graded with it is void.
    const mangled = this.fx.sent.filter((s) => {
      const m = /f24c3-[a-z0-9-]+/i.exec(normalizeForMatch(s.content));
      return m && !naiveMatch(s.content, m[0]);
    });
    if (mangled.length > 0) {
      faults.push(
        `${mangled.length} reply/replies are mangled (escaped, wrapped or split): the pre-repair matcher would have reported these as LOST`,
      );
    }
    if (rep.bad_token_identifies > 0) {
      faults.push(
        `${rep.bad_token_identifies} IDENTIFY(s) presented a token the fixture did not mint — auth failure, not inbound loss`,
      );
    }
    if (rep.max_concurrent_gateway_connections === 0) {
      // Distinguish the two very different zero-states.
      if ((rep.tcp_connections ?? 0) > 0) {
        faults.push(
          `the binary DIALLED the fixture (${rep.tcp_connections} TCP connection(s)) but no WebSocket ` +
            `handshake completed — a protocol/URL fault, NOT inbound loss and NOT "nothing started"` +
            (rep.client_errors?.length ? ` (client_errors: ${rep.client_errors.join(',')})` : ''),
        );
      } else {
        faults.push(
          'the binary never opened a TCP connection to the fixture — NOT MEASURED (nothing dialled)',
        );
      }
    }

    const zeroArrivals = rep.sent_total === 0;
    const legsPass = this.results.filter((r) => r.ok).length;

    let verdict;
    if (faults.length > 0) verdict = 'INCOMPLETE';
    else if (zeroArrivals) verdict = 'FAIL';
    else if (legsPass === this.results.length) verdict = 'PASS';
    else verdict = 'FAIL';

    const result = {
      schema: RESULT_SCHEMA,
      verdict,
      instrument_fault: faults.length > 0,
      instrument_faults: faults,
      legs: this.results,
      legs_passed: legsPass,
      legs_total: this.results.length,
      // The race instrument. 2 = the double-ChannelManager defect reaches
      // Discord (as DUPLICATION, not loss). 0 = nothing connected, a failing
      // answer so a "fix" that starts nothing cannot pass.
      max_concurrent_gateway_connections: rep.max_concurrent_gateway_connections,
      total_gateway_connections: rep.total_gateway_connections,
      identify_count: rep.identify_count,
      resume_count: rep.resume_count,
      heartbeats: rep.heartbeats,
      dispatched_total: rep.dispatched_total,
      dispatch_socket_deliveries: rep.dispatch_socket_deliveries,
      arrivals_total: rep.sent_total,
      llm_turns: llmTurns,
      steady_per_token: steady?.perSteady ?? [],
      steady_lost: steady?.lost ?? [],
      steady_duplicated: steady?.duped ?? [],
      fixture_coverage: rep.coverage,
      fixture_notes: rep.fixture_notes,
      notes: this.notes,
    };
    return result;
  }

  cleanup() {
    for (const p of [this.child, this.llm]) {
      try {
        p?.kill('SIGKILL');
      } catch {
        /* noop */
      }
    }
  }
}

// ── main ─────────────────────────────────────────────────────────────────────

function parseArgs(argv) {
  const out = {
    binary: null,
    runDir: path.join(os.tmpdir(), 'f24-c3-discord'),
    budgetMs: 45_000,
    settleMs: 15_000,
    steady: 6,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--binary') out.binary = argv[++i];
    else if (a === '--run-dir') out.runDir = argv[++i];
    else if (a === '--budget-ms') out.budgetMs = Number(argv[++i]);
    else if (a === '--settle-ms') out.settleMs = Number(argv[++i]);
    else if (a === '--steady') out.steady = Number(argv[++i]);
    else {
      process.stderr.write(`unknown arg ${a}\n`);
      process.exit(2);
    }
  }
  if (!out.binary) {
    process.stderr.write('--binary is required\n');
    process.exit(2);
  }
  return out;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  fs.rmSync(args.runDir, { recursive: true, force: true });
  fs.mkdirSync(args.runDir, { recursive: true });

  const d = new Driver(args);
  d.fx = new DiscordFixture();
  await d.fx.start();
  d.note(`discord fixture on ${d.fx.apiBase} (gateway ${d.fx.gatewayUrl})`);
  d.llmUrl = d.startLlm();
  d.note(`llm fixture ${d.llmUrl}`);
  d.writeConfig();
  d.startGateway();

  let steady = null;
  let up = false;
  try {
    up = d.waitForGateway(60_000);
    if (!up) {
      d.record('admit', false, 'binary never IDENTIFYed on the gateway — NOT MEASURED');
    } else {
      steady = d.run();
    }
  } catch (e) {
    d.note(`driver threw: ${e?.message ?? e}`);
  }

  const result = d.grade(steady);
  const outFile = path.join(args.runDir, 'discord-inbound-result.json');
  fs.writeFileSync(outFile, `${JSON.stringify(result, null, 2)}\n`);
  process.stdout.write(
    `\nF24C3DISC verdict=${result.verdict} legs=${result.legs_passed}/${result.legs_total} ` +
      `arrivals=${result.arrivals_total} conns_max=${result.max_concurrent_gateway_connections} ` +
      `instrument_fault=${result.instrument_fault ? 'YES' : 'no'}\n`,
  );
  process.stdout.write(`F24C3DISC RESULT ${outFile}\n`);
  d.cleanup();
  process.exit(result.verdict === 'PASS' ? 0 : 1);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((e) => {
    process.stderr.write(`${e?.stack ?? e}\n`);
    process.exit(3);
  });
}
