#!/usr/bin/env node
// Two-PROCESS inbound consumption race, and the lease that closes it.
//
// WHY THIS EXISTS, AND WHY IT IS A SEPARATE FILE.
//
// F24-C3-H4 closed a double `ChannelManager` WITHIN ONE PROCESS: `gateway run`
// built its own manager and also let the cron handler build a second one, so
// one process registered every adapter twice and only one of the two managers
// carried a subscriber. Measured loss was 8/8 at startup and 5/6 in steady
// state, silently.
//
// That fix is in-process. It says nothing about a SECOND PROCESS. Three
// production sites each construct a `ChannelManager` and call `start_all()`:
//
//   bootstrap.rs   — EVERY ordinary `wayland-core` session
//   cron.rs        — `wayland-core cron daemon` (ships launchd + systemd units)
//   gateway.rs     — the installed service
//
// and there is no cross-process exclusion anywhere in the channel stack.
// Inbound polling is a DESTRUCTIVE READ — Telegram advances `offset=N`, IMAP
// sets `\Seen`, Discord allows one gateway session per token — so whichever
// process wins the poll DELETES the message for the other.
//
// `scripts/f24-tg-fixture.mjs` and `scripts/f24-inbound.mjs` belong to other
// concurrently-running lanes. This driver USES the tg fixture as an unmodified
// black box over its documented `/__control/*` surface and never writes to it.
// The LLM stub below is local to this file for the same reason, and because it
// needs a controllable hold time this driver alone cares about.
//
// ─────────────────────────────────────────────────────────────────────────────
// HOW ATTRIBUTION IS OBTAINED WITHOUT TRUSTING THE BINARY'S OWN LOGS
//
// The fixture sees TCP, not pids, so it cannot say which process issued a poll.
// Grepping the binary's stderr for its own claim about what it started would be
// the tautology this program keeps measuring. So attribution here is the DELTA
// BETWEEN PROCESS COUNTS, taken from an out-of-process counter:
//
//   `max_concurrent_getupdates`  2 = two pollers,  1 = one poller,  0 = NOTHING
//                                polls — a DISTINCT FAILING answer, so a "fix"
//                                that works by making nothing start cannot pass.
//
// and a second, independent signal for the same claim — the poll RATE over
// equal-length windows — so an alternating (never-overlapping) pair of pollers
// cannot silently read as 1.
//
// ─────────────────────────────────────────────────────────────────────────────
// INSTRUMENT DISCIPLINE (§6b-ii)
//
// A run this driver cannot vouch for is graded INCOMPLETE, never LOSS. See
// `gradeInstrument()`. The three-assertion self-test is `--self-test`.
//
// usage:
//   f24-channel-lease.mjs --binary <path> --run-dir <dir> --leg 1|2|4|all
//   f24-channel-lease.mjs --self-test

import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));

// ───────────────────────────────────────────────────────────────────────────
// tiny helpers
// ───────────────────────────────────────────────────────────────────────────

function sleep(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

function httpJson(url, { method = 'GET', body = null, timeoutMs = 15000 } = {}) {
  return new Promise((resolve, reject) => {
    const payload = body === null ? null : JSON.stringify(body);
    const u = new URL(url);
    const req = http.request(
      {
        hostname: u.hostname,
        port: u.port,
        path: `${u.pathname}${u.search}`,
        method,
        timeout: timeoutMs,
        headers: payload
          ? { 'content-type': 'application/json', 'content-length': Buffer.byteLength(payload) }
          : {},
      },
      (res) => {
        let data = '';
        res.on('data', (c) => {
          data += c;
        });
        res.on('end', () => {
          try {
            resolve(JSON.parse(data));
          } catch (e) {
            reject(new Error(`bad json from ${url}: ${e.message}: ${data.slice(0, 200)}`));
          }
        });
      },
    );
    req.on('timeout', () => req.destroy(new Error(`timeout ${url}`)));
    req.on('error', reject);
    if (payload) req.write(payload);
    req.end();
  });
}

/** Byte-count every capture. A zero-byte log is an instrument fault, not a result. */
function byteLen(p) {
  try {
    return fs.statSync(p).size;
  } catch {
    return -1;
  }
}

// ───────────────────────────────────────────────────────────────────────────
// The concurrency reader.
//
// THE DEFECT THIS SHAPE AVOIDS. The obvious reader is `report.max_concurrent_
// getupdates` — a single global maximum over the WHOLE run. Used naively that
// silently destroys the measurement, because leg 2 starts a second process
// PART WAY THROUGH: a global max of 2 cannot distinguish "two processes polled
// concurrently" from "one process briefly overlapped its own retry at second
// 3". The window-scoped reader below recomputes the max from
// `concurrency_trace` between two wall-clock instants this driver recorded
// itself, so a spike is attributed to the window it happened in.
// ───────────────────────────────────────────────────────────────────────────

function maxOpenInWindow(trace, startIso, endIso) {
  const s = Date.parse(startIso);
  const e = Date.parse(endIso);
  let max = 0;
  for (const p of trace) {
    const t = Date.parse(p.at);
    if (t >= s && t <= e && p.open > max) max = p.open;
  }
  return max;
}

function pollsInWindow(trace, startIso, endIso) {
  const s = Date.parse(startIso);
  const e = Date.parse(endIso);
  const ids = new Set();
  for (const p of trace) {
    const t = Date.parse(p.at);
    if (t >= s && t <= e) ids.add(p.poll);
  }
  return ids.size;
}

// ───────────────────────────────────────────────────────────────────────────
// The instrument grader. INCOMPLETE, never LOSS, when the run is not vouchable.
// ───────────────────────────────────────────────────────────────────────────

/**
 * @returns {{fault: boolean, reasons: string[]}}
 */
function gradeInstrument(facts) {
  const reasons = [];
  if (!facts.fixture_reachable) reasons.push('tg fixture never answered /__control/health');
  if (facts.tg_journal_bytes <= 0) reasons.push(`tg journal is ${facts.tg_journal_bytes} bytes`);
  if (facts.core_log_bytes !== undefined && facts.core_log_bytes <= 0) {
    reasons.push(`core log is ${facts.core_log_bytes} bytes`);
  }
  // A process that never reached the LLM stub never finished booting, so
  // "it did not poll" tells us nothing about whether it WOULD have polled.
  if (facts.expect_boot && !facts.llm_hit) {
    reasons.push('session never reached the LLM stub — it did not finish booting');
  }
  if (facts.expect_polling && facts.polls_total === 0) {
    reasons.push('zero getUpdates in the whole run — nothing polled at all');
  }
  return { fault: reasons.length > 0, reasons };
}

// ───────────────────────────────────────────────────────────────────────────
// The local LLM stub.
//
// Two jobs: (1) let a one-shot session complete a turn rather than dying on a
// provider error, and (2) HOLD the response open for a controllable time, so
// this driver decides exactly how long an "ordinary session" stays alive and
// therefore how long its channel poller runs. `llm_hit` is also the
// independent proof that the session finished booting — used by the grader to
// tell a genuine refutation ("it booted and did not poll") apart from an
// instrument fault ("it never booted").
// ───────────────────────────────────────────────────────────────────────────

class LlmStub {
  constructor(holdMs, journalPath) {
    this.holdMs = holdMs;
    this.journalPath = journalPath;
    this.hits = 0;
  }

  start() {
    fs.writeFileSync(this.journalPath, '');
    this.server = http.createServer((req, res) => {
      let body = '';
      req.on('data', (c) => {
        body += c;
      });
      req.on('end', () => {
        this.hits += 1;
        fs.appendFileSync(
          this.journalPath,
          `${JSON.stringify({ at: new Date().toISOString(), path: req.url, hit: this.hits })}\n`,
        );
        // Hold, so the session (and therefore its channel poller) stays alive
        // for a duration this driver controls.
        setTimeout(() => {
          const payload = {
            id: 'f24cl',
            object: 'chat.completion',
            created: Math.floor(Date.now() / 1000),
            model: 'f24cl-stub',
            choices: [
              { index: 0, message: { role: 'assistant', content: 'ack' }, finish_reason: 'stop' },
            ],
            usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
          };
          const s = JSON.stringify(payload);
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(s);
        }, this.holdMs);
      });
    });
    this.server.listen(0, '127.0.0.1');
    // Block until bound.
    for (let i = 0; i < 200 && !this.server.address(); i += 1) sleep(25);
    const a = this.server.address();
    if (!a) throw new Error('llm stub never bound');
    this.url = `http://127.0.0.1:${a.port}/v1`;
    return this.url;
  }

  stop() {
    if (this.server) this.server.close();
  }
}

// ───────────────────────────────────────────────────────────────────────────
// The run
// ───────────────────────────────────────────────────────────────────────────

class Run {
  constructor(args) {
    this.args = args;
    this.runDir = path.resolve(args.runDir);
    this.home = path.join(this.runDir, 'home');
    this.runId = crypto.randomBytes(4).toString('hex');
    // Minted here, for this run only. Not a vendor credential; never printed.
    this.botToken = `881${crypto.randomInt(100000, 999999)}:F24CL-${this.runId}`;
    this.vaultPassphrase = crypto.randomBytes(24).toString('hex');
    this.chatId = '881001';
    this.senderId = '881001';
    this.children = [];
    this.notes = [];
    this.tgJournal = path.join(this.runDir, 'tg-fixture.jsonl');
    fs.mkdirSync(this.runDir, { recursive: true });
  }

  note(t) {
    const line = `[lease] ${t}`;
    this.notes.push(line);
    process.stdout.write(`${line}\n`);
  }

  // ── fixture + config ────────────────────────────────────────────────────

  startTgFixture() {
    const logPath = path.join(this.runDir, 'tg-fixture.log');
    fs.writeFileSync(logPath, '');
    const fd = fs.openSync(logPath, 'a');
    const child = spawn(
      process.execPath,
      [
        path.join(HERE, 'f24-tg-fixture.mjs'),
        '--token',
        this.botToken,
        '--journal',
        this.tgJournal,
        '--port',
        '0',
        '--max-wait-ms',
        '2000',
      ],
      { stdio: ['ignore', fd, fd], windowsHide: true },
    );
    this.children.push(child);
    const re = /(http:\/\/127\.0\.0\.1:\d+)/;
    let banner = '';
    for (let i = 0; i < 200; i += 1) {
      banner = fs.readFileSync(logPath, 'utf8');
      if (re.test(banner)) break;
      sleep(50);
    }
    const m = re.exec(banner);
    if (!m) throw new Error(`tg fixture never signalled ready:\n${banner}`);
    this.tgUrl = m[1];
    this.note(`tg fixture at ${this.tgUrl}`);
    return this.tgUrl;
  }

  writeConfig() {
    fs.mkdirSync(path.join(this.home, 'channels'), { recursive: true });
    fs.writeFileSync(
      path.join(this.home, 'credentials.toml'),
      ['[secrets]', `"telegram.f24cl.bot_token" = "${this.botToken}"`, ''].join('\n'),
      { mode: 0o600 },
    );
    fs.writeFileSync(
      path.join(this.home, 'config.toml'),
      [
        '[default]',
        'provider = "f24clstub"',
        '',
        '[providers.f24clstub]',
        'provider = "openai"',
        'model = "f24cl-stub"',
        'api_key = "f24cl-not-a-real-key"',
        `base_url = "${this.llmUrl}"`,
        '',
        // The POLLING path is what this measures. A bound webhook host would
        // also collide on a fixed port with the other lanes on this box.
        '[inbound_webhook]',
        'enabled = false',
        '',
      ].join('\n'),
      { mode: 0o600 },
    );
    fs.writeFileSync(
      path.join(this.home, 'channels', 'f24cltg.toml'),
      [
        'name = "f24cltg"',
        'platform = "telegram"',
        'enabled = true',
        '',
        '[options]',
        'credential_handle = "telegram.f24cl.bot_token"',
        `api_base_url = "${this.tgUrl}"`,
        // Short polls so two loops interleave many times inside the run. The
        // fixture still holds an empty result open up to --max-wait-ms, which
        // is what makes concurrent pollers observable as overlapping requests.
        'long_poll_timeout_secs = 1',
        '',
        '[inbound]',
        'dm = "allowlist"',
        `dm_allowlist = ["${this.senderId}"]`,
        'group = "disabled"',
        'require_mention = true',
        'tools = "conversational"',
        '',
      ].join('\n'),
    );
  }

  childEnv(extra = {}) {
    return {
      ...process.env,
      WAYLAND_HOME: this.home,
      // 24-C3-H2: an isolated profile with NO vault passphrase stores
      // credentials plaintext-0600 and then refuses EVERY turn host-wide with
      // "Session persistence authority unavailable". Without this a
      // credentials-posture refusal would be misattributed to the polling path.
      WAYLAND_VAULT_PASSPHRASE: this.vaultPassphrase,
      RUST_LOG:
        process.env.RUST_LOG ??
        'info,wcore_agent::bootstrap=debug,wcore_agent::channel_inbound=debug,wcore_channels=debug,wcore_channel_telegram=debug',
      ...extra,
    };
  }

  // ── processes under test ────────────────────────────────────────────────

  /** The installed service. Foreground, so this driver owns and can reap it. */
  startGateway(tag) {
    const logPath = path.join(this.runDir, `gateway-${tag}.log`);
    fs.writeFileSync(logPath, '');
    const fd = fs.openSync(logPath, 'a');
    const child = spawn(this.args.binary, ['gateway', 'run'], {
      stdio: ['pipe', fd, fd],
      env: this.childEnv(),
    });
    this.children.push(child);
    this.note(`gateway '${tag}' pid=${child.pid} log=${path.basename(logPath)}`);
    return { child, logPath };
  }

  /**
   * An ORDINARY session — a one-shot prompt. This is the process the brief's
   * headline scenario describes the user opening beside the installed service.
   * It is not a test harness path: `wayland-core [PROMPT]...` is the shipped
   * surface, and nothing in production ever sets `without_channels`.
   */
  startSession(tag) {
    const logPath = path.join(this.runDir, `session-${tag}.log`);
    fs.writeFileSync(logPath, '');
    const fd = fs.openSync(logPath, 'a');
    const child = spawn(this.args.binary, [`f24cl session ${tag} ${this.runId}`], {
      stdio: ['ignore', fd, fd],
      env: this.childEnv(),
    });
    this.children.push(child);
    this.note(`session '${tag}' pid=${child.pid} log=${path.basename(logPath)}`);
    return { child, logPath };
  }

  waitExit(child, budgetMs, label) {
    const deadline = Date.now() + budgetMs;
    while (Date.now() < deadline) {
      if (child.exitCode !== null || child.signalCode !== null) {
        this.note(`${label} exited code=${child.exitCode} signal=${child.signalCode}`);
        return true;
      }
      sleep(200);
    }
    this.note(`${label} still alive after ${budgetMs}ms`);
    return false;
  }

  // ── fixture control ─────────────────────────────────────────────────────

  async submit(text) {
    return httpJson(`${this.tgUrl}/__control/submit`, {
      method: 'POST',
      body: {
        token: this.botToken,
        chatId: this.chatId,
        senderId: this.senderId,
        username: 'f24cl',
        text,
      },
    });
  }

  async report() {
    return httpJson(`${this.tgUrl}/__control/report`);
  }

  async waitForPolls(minPolls, budgetMs, label) {
    const deadline = Date.now() + budgetMs;
    let last = 0;
    while (Date.now() < deadline) {
      const r = await this.report().catch(() => null);
      if (r) {
        last = r.poll_total;
        if (r.poll_total >= minPolls) {
          this.note(`${label}: poll_total=${r.poll_total} (>= ${minPolls})`);
          return true;
        }
      }
      // Emit every iteration — a silent loop is indistinguishable from a stall
      // and the stream watchdog kills at 600s of silence (§6b).
      this.note(`${label}: waiting, poll_total=${last}, ${new Date().toISOString()}`);
      sleep(2000);
    }
    this.note(`${label}: TIMEOUT at poll_total=${last}`);
    return false;
  }

  cleanup() {
    for (const c of this.children) {
      try {
        c.kill('SIGKILL');
      } catch {
        /* already gone */
      }
    }
    if (this.llm) this.llm.stop();
  }
}

// ───────────────────────────────────────────────────────────────────────────
// LEG 1 — startup / backlog theft. Sequential, so attribution is unambiguous.
// ───────────────────────────────────────────────────────────────────────────

async function leg1(run, n = 8) {
  run.note('=== LEG 1: backlog theft by an ordinary session ===');
  const submitted = [];
  for (let i = 0; i < n; i += 1) {
    const r = await run.submit(`F24CL-L1-${run.runId}-${i}`);
    submitted.push(r.update_id);
  }
  run.note(`submitted ${submitted.length} updates BEFORE anything started: ${submitted.join(',')}`);

  const pre = await run.report();
  if (pre.still_pending.length !== n) {
    throw new Error(`instrument: expected ${n} pending, saw ${pre.still_pending.length}`);
  }

  // Only the ORDINARY SESSION runs. The service is not up yet.
  const sStart = new Date().toISOString();
  const { child: sess } = run.startSession('l1');
  run.waitExit(sess, 90_000, 'session l1');
  const sEnd = new Date().toISOString();
  sleep(1000);

  const afterSession = await run.report();
  const stolen = afterSession.updates.filter(
    (u) => submitted.includes(u.update_id) && u.deleted_by !== null,
  );
  run.note(
    `after the session exited: still_pending=${afterSession.still_pending.length}/${n}, ` +
      `deleted=${stolen.length}/${n}, polls=${afterSession.poll_total}`,
  );

  // NOW the installed service starts — the process that was supposed to
  // receive these messages.
  const gStart = new Date().toISOString();
  const { child: gw, logPath: gwLog } = run.startGateway('l1');
  await run.waitForPolls(afterSession.poll_total + 3, 60_000, 'gateway l1');
  const gEnd = new Date().toISOString();
  const final = await run.report();

  const gatewayServed = final.updates.filter(
    (u) =>
      submitted.includes(u.update_id) &&
      u.served_to.some((pollId) => {
        const t = final.concurrency_trace.find((c) => c.poll === pollId);
        return t && Date.parse(t.at) >= Date.parse(gStart) && Date.parse(t.at) <= Date.parse(gEnd);
      }),
  );

  try {
    gw.kill('SIGKILL');
  } catch {
    /* ignore */
  }

  const facts = {
    fixture_reachable: true,
    tg_journal_bytes: byteLen(run.tgJournal),
    core_log_bytes: byteLen(gwLog),
    expect_boot: true,
    llm_hit: run.llm.hits > 0,
    expect_polling: true,
    polls_total: final.poll_total,
  };
  const grade = gradeInstrument(facts);

  return {
    leg: 1,
    name: 'backlog theft by an ordinary session',
    submitted: submitted.length,
    stolen_by_session: stolen.length,
    gateway_received: gatewayServed.length,
    still_pending_after_session: afterSession.still_pending.length,
    session_window: [sStart, sEnd],
    gateway_window: [gStart, gEnd],
    poll_total: final.poll_total,
    llm_hits: run.llm.hits,
    instrument: grade,
    // The verdict: the session destroyed messages server-side and then died
    // with them, so the service that was meant to receive them got none.
    verdict: grade.fault
      ? 'INCOMPLETE'
      : stolen.length > 0 && gatewayServed.length === 0
        ? 'LOSS REPRODUCED'
        : stolen.length === 0
          ? 'NO LOSS — the session did not consume the backlog'
          : 'PARTIAL',
  };
}

// ───────────────────────────────────────────────────────────────────────────
// LEG 2 — steady state. Two live processes, messages arriving throughout.
//
// Steady state is included deliberately: it is what raised the IN-PROCESS
// version of this finding from MEDIUM to HIGH, and a startup-only run would
// have missed it entirely.
// ───────────────────────────────────────────────────────────────────────────

async function leg2(run, windowMs = 20_000) {
  run.note('=== LEG 2: steady state, two live processes ===');

  const { child: gw, logPath: gwLog } = run.startGateway('l2');
  if (!(await run.waitForPolls(3, 90_000, 'gateway l2 warmup'))) {
    return {
      leg: 2,
      verdict: 'INCOMPLETE',
      instrument: { fault: true, reasons: ['gateway never polled during warmup'] },
    };
  }

  // ── window A: the service alone ──────────────────────────────────────────
  const aStart = new Date().toISOString();
  for (let i = 0; i < windowMs / 2000; i += 1) {
    await run.submit(`F24CL-L2A-${run.runId}-${i}`);
    run.note(`window A: submitted ${i}, ${new Date().toISOString()}`);
    sleep(2000);
  }
  const aEnd = new Date().toISOString();
  const midReport = await run.report();
  const aMaxOpen = maxOpenInWindow(midReport.concurrency_trace, aStart, aEnd);
  const aPolls = pollsInWindow(midReport.concurrency_trace, aStart, aEnd);
  run.note(`window A (service alone): max_open=${aMaxOpen}, polls=${aPolls}`);

  // ── window B: an ordinary session opens alongside it ─────────────────────
  const { child: sess } = run.startSession('l2');
  sleep(3000); // let it boot and arm its poller
  const bStart = new Date().toISOString();
  for (let i = 0; i < windowMs / 2000; i += 1) {
    await run.submit(`F24CL-L2B-${run.runId}-${i}`);
    run.note(`window B: submitted ${i}, ${new Date().toISOString()}`);
    sleep(2000);
  }
  const bEnd = new Date().toISOString();
  const finalReport = await run.report();
  const bMaxOpen = maxOpenInWindow(finalReport.concurrency_trace, bStart, bEnd);
  const bPolls = pollsInWindow(finalReport.concurrency_trace, bStart, bEnd);
  run.note(`window B (service + ordinary session): max_open=${bMaxOpen}, polls=${bPolls}`);

  for (const c of [gw, sess]) {
    try {
      c.kill('SIGKILL');
    } catch {
      /* ignore */
    }
  }

  const facts = {
    fixture_reachable: true,
    tg_journal_bytes: byteLen(run.tgJournal),
    core_log_bytes: byteLen(gwLog),
    expect_boot: false,
    llm_hit: run.llm.hits > 0,
    expect_polling: true,
    polls_total: finalReport.poll_total,
  };
  const grade = gradeInstrument(facts);

  // TWO independent signals for one claim. `max_open == 2` is direct. The poll
  // RATE roughly doubling is the backstop for an alternating pair that never
  // happens to overlap, which would otherwise read as a false 1.
  const rate = aPolls === 0 ? 0 : bPolls / aPolls;
  const twoPollers = bMaxOpen >= 2 || rate >= 1.6;

  return {
    leg: 2,
    name: 'steady state, two live processes',
    window_a: { start: aStart, end: aEnd, max_open: aMaxOpen, polls: aPolls, who: 'service only' },
    window_b: {
      start: bStart,
      end: bEnd,
      max_open: bMaxOpen,
      polls: bPolls,
      who: 'service + ordinary session',
    },
    poll_rate_ratio: Number(rate.toFixed(2)),
    two_pollers_detected: twoPollers,
    llm_hits: run.llm.hits,
    instrument: grade,
    verdict: grade.fault
      ? 'INCOMPLETE'
      : bMaxOpen === 0
        ? 'DENIAL — nothing polled in window B; a green here would be manufactured'
        : twoPollers
          ? 'RACE REPRODUCED — two processes poll one account'
          : 'NO RACE — exactly one poller with both processes live',
  };
}

// ───────────────────────────────────────────────────────────────────────────
// LEG 4 — ungraceful release. Only meaningful once a lease exists.
// ───────────────────────────────────────────────────────────────────────────

async function leg4(run) {
  run.note('=== LEG 4: ungraceful kill of the holder, takeover by the loser ===');

  const { child: gw, logPath: gwLog } = run.startGateway('l4-holder');
  if (!(await run.waitForPolls(3, 90_000, 'holder warmup'))) {
    return {
      leg: 4,
      verdict: 'INCOMPLETE',
      instrument: { fault: true, reasons: ['holder never polled'] },
    };
  }

  // The loser comes up while the holder is alive and must STAY alive.
  const { child: gw2, logPath: gw2Log } = run.startGateway('l4-loser');
  sleep(8000);

  const beforeKill = await run.report();
  const kStart = new Date().toISOString();
  run.note(`SIGKILL the holder pid=${gw.pid} — no drain, no release, no cleanup`);
  gw.kill('SIGKILL');

  // Takeover is observable purely from the fixture: polls must continue after
  // the holder's death. A lease that never releases converts message loss into
  // permanent unavailability — the exact failure the sandbox lane hit last
  // night with a stale lease. This leg is what stops that being reintroduced.
  const took = await run.waitForPolls(beforeKill.poll_total + 5, 90_000, 'takeover');
  const kEnd = new Date().toISOString();
  const after = await run.report();

  // The positive path, so takeover cannot be claimed by a process that polls
  // but delivers nothing.
  const probe = await run.submit(`F24CL-L4-TAKEOVER-${run.runId}`);
  const delivered = await (async () => {
    for (let i = 0; i < 20; i += 1) {
      const r = await run.report();
      const u = r.updates.find((x) => x.update_id === probe.update_id);
      if (u && u.serve_count > 0) return true;
      run.note(`takeover probe: waiting, ${new Date().toISOString()}`);
      sleep(2000);
    }
    return false;
  })();

  try {
    gw2.kill('SIGKILL');
  } catch {
    /* ignore */
  }

  const facts = {
    fixture_reachable: true,
    tg_journal_bytes: byteLen(run.tgJournal),
    core_log_bytes: byteLen(gw2Log),
    expect_boot: false,
    llm_hit: true,
    expect_polling: true,
    polls_total: after.poll_total,
  };
  const grade = gradeInstrument(facts);

  return {
    leg: 4,
    name: 'ungraceful kill of the holder',
    polls_before_kill: beforeKill.poll_total,
    polls_after_kill: after.poll_total,
    kill_window: [kStart, kEnd],
    takeover_polls_observed: took,
    post_takeover_message_delivered: delivered,
    holder_log_bytes: byteLen(gwLog),
    loser_log_bytes: byteLen(gw2Log),
    instrument: grade,
    verdict: grade.fault
      ? 'INCOMPLETE'
      : took && delivered
        ? 'TAKEOVER OK — lease released on SIGKILL and the survivor serves traffic'
        : took
          ? 'PARTIAL — polls resumed but no message was delivered after takeover'
          : 'WEDGED — no polls after the holder died; the lease did not release',
  };
}

// ───────────────────────────────────────────────────────────────────────────
// SELF-TEST (§6b-ii) — three assertions, not two.
//
// The third is the only one that proves the repair does anything: without it
// the self-test passes on the BROKEN instrument too.
// ───────────────────────────────────────────────────────────────────────────

function selfTest() {
  const results = [];
  const assert = (name, cond, detail) => {
    results.push({ name, pass: Boolean(cond), detail });
    process.stdout.write(`${cond ? 'PASS' : 'FAIL'}  ${name}${detail ? ` — ${detail}` : ''}\n`);
  };

  // The naive matcher this instrument replaced: a single global maximum over
  // the whole run, with no window scoping.
  const naiveMaxOpen = (trace) => trace.reduce((m, p) => Math.max(m, p.open), 0);

  // A trace where ONE process briefly overlapped its own retry early on, and
  // then a second window in which only ONE poller ever ran.
  const trace = [
    { at: '2026-07-29T10:00:00.000Z', open: 1, poll: 1 },
    { at: '2026-07-29T10:00:01.000Z', open: 2, poll: 2 }, // self-overlap, window A
    { at: '2026-07-29T10:00:02.000Z', open: 1, poll: 3 },
    { at: '2026-07-29T10:00:30.000Z', open: 1, poll: 4 }, // window B: one poller
    { at: '2026-07-29T10:00:31.000Z', open: 1, poll: 5 },
  ];
  const winB = ['2026-07-29T10:00:20.000Z', '2026-07-29T10:00:40.000Z'];

  // 1. KNOWN-POSITIVE: a genuine two-poller window is detected.
  const twoPollerTrace = [
    { at: '2026-07-29T10:00:25.000Z', open: 1, poll: 9 },
    { at: '2026-07-29T10:00:26.000Z', open: 2, poll: 10 },
  ];
  assert(
    'known-positive: window-scoped reader sees 2 when two pollers overlap IN the window',
    maxOpenInWindow(twoPollerTrace, winB[0], winB[1]) === 2,
    `got ${maxOpenInWindow(twoPollerTrace, winB[0], winB[1])}`,
  );

  // 2. KNOWN-NEGATIVE: a single-poller window reads 1, not 2.
  assert(
    'known-negative: window-scoped reader sees 1 when only one poller ran IN the window',
    maxOpenInWindow(trace, winB[0], winB[1]) === 1,
    `got ${maxOpenInWindow(trace, winB[0], winB[1])}`,
  );

  // 3. THE OLD MATCHER WOULD HAVE MISSED IT. The naive global maximum reports
  //    2 for that same single-poller window, because it cannot tell an
  //    out-of-window self-overlap from an in-window second process. Without
  //    this assertion the self-test passes on the broken instrument too.
  assert(
    'the OLD matcher (global max, no window) would have MISREAD this — reports 2 for a 1-poller window',
    naiveMaxOpen(trace) === 2 && maxOpenInWindow(trace, winB[0], winB[1]) === 1,
    `naive=${naiveMaxOpen(trace)} windowed=${maxOpenInWindow(trace, winB[0], winB[1])}`,
  );

  // Grader: a suspect run must grade INCOMPLETE, never LOSS.
  assert(
    'grader: zero-byte journal is an instrument fault',
    gradeInstrument({
      fixture_reachable: true,
      tg_journal_bytes: 0,
      expect_boot: false,
      expect_polling: true,
      polls_total: 5,
    }).fault === true,
  );
  assert(
    'grader: a booted-but-silent session is NOT a fault (it is a genuine refutation)',
    gradeInstrument({
      fixture_reachable: true,
      tg_journal_bytes: 4096,
      expect_boot: true,
      llm_hit: true,
      expect_polling: false,
      polls_total: 0,
    }).fault === false,
  );
  assert(
    'grader: a session that never reached the LLM stub IS a fault (it never booted)',
    gradeInstrument({
      fixture_reachable: true,
      tg_journal_bytes: 4096,
      expect_boot: true,
      llm_hit: false,
      expect_polling: false,
      polls_total: 0,
    }).fault === true,
  );

  const failed = results.filter((r) => !r.pass);
  process.stdout.write(
    `\nSELFTEST_TOTAL=${results.length} SELFTEST_PASSED=${results.length - failed.length} SELFTEST_FAILED=${failed.length}\n`,
  );
  return failed.length === 0 ? 0 : 1;
}

// ───────────────────────────────────────────────────────────────────────────

function parseArgs(argv) {
  const out = { binary: null, runDir: null, leg: 'all', selfTest: false, holdMs: 30_000 };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--binary') out.binary = argv[++i];
    else if (a === '--run-dir') out.runDir = argv[++i];
    else if (a === '--leg') out.leg = argv[++i];
    else if (a === '--hold-ms') out.holdMs = Number(argv[++i]);
    else if (a === '--self-test') out.selfTest = true;
    else {
      process.stderr.write(`f24-channel-lease: unknown argument ${a}\n`);
      process.exit(2);
    }
  }
  return out;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.selfTest) process.exit(selfTest());
  if (!args.binary || !args.runDir) {
    process.stderr.write('f24-channel-lease: --binary and --run-dir are required\n');
    process.exit(2);
  }

  const run = new Run(args);
  const out = { run_id: run.runId, legs: [], notes: [] };
  try {
    run.llm = new LlmStub(args.holdMs, path.join(run.runDir, 'llm.jsonl'));
    run.llmUrl = run.llm.start();
    run.note(`llm stub at ${run.llmUrl} holding ${args.holdMs}ms`);
    run.startTgFixture();
    run.writeConfig();

    const bi = spawnSync(args.binary, ['--build-info'], { encoding: 'utf8' });
    out.build_info = `${bi.stdout}${bi.stderr}`.trim().split('\n').slice(0, 3).join(' | ');
    run.note(`binary: ${out.build_info}`);

    const legs = args.leg === 'all' ? ['1', '2'] : [args.leg];
    for (const l of legs) {
      if (l === '1') out.legs.push(await leg1(run));
      else if (l === '2') out.legs.push(await leg2(run));
      else if (l === '4') out.legs.push(await leg4(run));
    }
  } catch (e) {
    out.error = e.message;
    out.legs.push({ verdict: 'INCOMPLETE', instrument: { fault: true, reasons: [e.message] } });
  } finally {
    out.notes = run.notes;
    out.tg_journal_bytes = byteLen(run.tgJournal);
    run.cleanup();
  }

  const reportPath = path.join(run.runDir, 'lease-report.json');
  fs.writeFileSync(reportPath, JSON.stringify(out, null, 2));
  process.stdout.write(`\n===== RESULT =====\n`);
  for (const l of out.legs) {
    process.stdout.write(`LEG${l.leg ?? '?'}_VERDICT=${l.verdict}\n`);
    if (l.instrument?.fault) {
      process.stdout.write(`LEG${l.leg ?? '?'}_INSTRUMENT_FAULT=${l.instrument.reasons.join('; ')}\n`);
    }
  }
  process.stdout.write(`REPORT=${reportPath} BYTES=${byteLen(reportPath)}\n`);
  process.stdout.write(`TG_JOURNAL_BYTES=${out.tg_journal_bytes}\n`);
  process.stdout.write(`F24CL_DONE\n`);
}

main().then(
  () => process.exit(0),
  (e) => {
    process.stderr.write(`f24-channel-lease: ${e.stack ?? e.message}\n`);
    process.exit(1);
  },
);
