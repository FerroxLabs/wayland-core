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
import os from 'node:os';
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

/** Raw-body client. `httpJson` cannot read an SSE stream — it is not JSON. */
function httpText(url, { method = 'GET', body = null, timeoutMs = 15000 } = {}) {
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
        res.on('end', () => resolve(data));
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
// The token matcher.
//
// THE DEFECT THIS SHAPE AVOIDS, measured twice on this program. A console line
// wrap puts a newline INSIDE the phrase a matcher searches for, so the matcher
// reports absence while the raw log contains the string several times. One lane
// wrote that defect up and moved on; the NEXT lane hit the identical defect
// again. So: strip all whitespace from both sides before comparing. The tokens
// this checks (`F24_CHANNEL_LEASE=observer`) contain no internal spaces, so
// stripping cannot create a false positive by joining two unrelated words that
// were separated only by a space.
// ───────────────────────────────────────────────────────────────────────────

function containsToken(haystack, token) {
  const strip = (x) => String(x).replace(/\s+/g, '');
  return strip(haystack).includes(strip(token));
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
// The LLM stub — runs as its OWN OS PROCESS (this file re-execs itself with
// `--llm-stub`).
//
// Two jobs: (1) let a one-shot session complete a turn rather than dying on a
// provider error, and (2) HOLD the response open for a controllable time, so
// this driver decides exactly how long an "ordinary session" stays alive and
// therefore how long its channel poller runs. The journal is also the
// independent proof that the session finished booting — used by the grader to
// tell a genuine refutation ("it booted and did not poll") apart from an
// instrument fault ("it never booted").
//
// WHY A SEPARATE PROCESS, MEASURED NOT ASSUMED. The first version ran the stub
// in this driver's own event loop and the very first live run graded
// INCOMPLETE with `llm stub never bound`. Node is single-threaded: the
// driver's `Atomics.wait` busy-wait blocks the same event loop that has to
// accept the socket, so `listen()` could never complete. The same deadlock
// would ALSO have silently starved every later request — the driver blocks for
// 2s between submissions in the steady-state leg, and an in-process stub would
// have been unable to answer the session for the whole of it. That would not
// have failed loudly; it would have looked like a session that booted and
// declined to poll, i.e. a FALSE REFUTATION of the very finding under test.
// Out-of-process removes the coupling entirely.
// ───────────────────────────────────────────────────────────────────────────

function runAsLlmStub(holdMs, journalPath) {
  fs.writeFileSync(journalPath, '');
  let hits = 0;
  const server = http.createServer((req, res) => {
    let body = '';
    req.on('data', (c) => {
      body += c;
    });
    req.on('end', () => {
      hits += 1;
      fs.appendFileSync(
        journalPath,
        `${JSON.stringify({ at: new Date().toISOString(), path: req.url, hit: hits })}\n`,
      );
      // SSE, not plain JSON. The first version answered a single JSON body and
      // the engine logged `OpenAI SSE stream closed before any terminal event
      // ([DONE] / finish_reason / error) — response truncated`, retried, and
      // tripped its circuit breaker — so the session hung ~90s instead of
      // completing a turn and exiting. Answering the real streaming shape is
      // what makes "an ordinary session" a faithful stand-in for one.
      res.writeHead(200, {
        'content-type': 'text/event-stream',
        'cache-control': 'no-cache',
        connection: 'keep-alive',
      });
      const base = {
        id: 'f24cl',
        object: 'chat.completion.chunk',
        created: Math.floor(Date.now() / 1000),
        model: 'f24cl-stub',
      };
      res.write(
        `data: ${JSON.stringify({ ...base, choices: [{ index: 0, delta: { role: 'assistant' }, finish_reason: null }] })}\n\n`,
      );
      // HOLD with the stream open, so the session — and therefore its channel
      // poller — stays alive for a duration this driver controls, and then
      // terminates CLEANLY rather than by timing out.
      setTimeout(() => {
        res.write(
          `data: ${JSON.stringify({ ...base, choices: [{ index: 0, delta: { content: 'ack' }, finish_reason: null }] })}\n\n`,
        );
        res.write(
          `data: ${JSON.stringify({ ...base, choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] })}\n\n`,
        );
        res.write('data: [DONE]\n\n');
        res.end();
      }, holdMs);
    });
  });
  server.listen(0, '127.0.0.1', () => {
    process.stdout.write(`LLMSTUB_READY url=http://127.0.0.1:${server.address().port}\n`);
  });
}

class LlmStub {
  constructor(holdMs, journalPath, runDir, children, noteFn) {
    this.holdMs = holdMs;
    this.journalPath = journalPath;
    this.runDir = runDir;
    this.children = children;
    this.note = noteFn;
  }

  /** Hits are counted from the stub's OWN journal, written by another process. */
  get hits() {
    try {
      return fs
        .readFileSync(this.journalPath, 'utf8')
        .split('\n')
        .filter((l) => l.trim().length > 0).length;
    } catch {
      return 0;
    }
  }

  start() {
    const logPath = path.join(this.runDir, 'llm-stub.log');
    fs.writeFileSync(logPath, '');
    const fd = fs.openSync(logPath, 'a');
    const child = spawn(
      process.execPath,
      [
        fileURLToPath(import.meta.url),
        '--llm-stub',
        '--hold-ms',
        String(this.holdMs),
        '--journal',
        this.journalPath,
      ],
      { stdio: ['ignore', fd, fd], windowsHide: true },
    );
    this.children.push(child);
    const re = /LLMSTUB_READY url=(http:\/\/127\.0\.0\.1:\d+)/;
    let banner = '';
    for (let i = 0; i < 200; i += 1) {
      banner = fs.readFileSync(logPath, 'utf8');
      if (re.test(banner)) break;
      sleep(50);
    }
    const m = re.exec(banner);
    if (!m) throw new Error(`llm stub never bound (log ${byteLen(logPath)} bytes): ${banner}`);
    // The engine appends its own `/v1/chat/completions`, so the base must NOT
    // carry `/v1` or the journal shows `/v1/v1/...`.
    this.url = m[1];
    this.child = child;
    return this.url;
  }

  stop() {
    if (this.child) {
      try {
        this.child.kill('SIGKILL');
      } catch {
        /* already gone */
      }
    }
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
      // stdin `ignore`, not `pipe`. A pipe this driver never writes to still
      // produced `write EPIPE` when the child went away mid-leg, aborting the
      // whole run at the point the measurement was about to be taken.
      stdio: ['ignore', fd, fd],
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
    let lastErr = null;
    for (let i = 0; i < 5; i += 1) {
      try {
        return await httpJson(`${this.tgUrl}/__control/report`);
      } catch (e) {
        lastErr = e;
        this.note(`report retry ${i + 1}/5 after ${e.message}`);
        sleep(1000);
      }
    }
    throw lastErr;
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
// LEG 3 — THE LEASE TEST. The service is already the owner; a session arrives.
//
// This is the leg leg 1 CANNOT be. Leg 1 runs the session on its own, so there
// is no contention and no lease decision to make — a session alone is the
// legitimate owner and will poll both before and after the fix. Leg 1 therefore
// characterises the destructive read; it does not test the exclusion. This does.
//
// Three claims, deliberately separated by SOURCE:
//   * "the loser does not poll"      — from the FIXTURE (another process)
//   * "the loser is not silent"      — from the LOSER'S OWN stderr, which is
//                                      the thing under test: whether it TELLS
//                                      the operator. Using the fixture for this
//                                      would be impossible; using the binary's
//                                      log for the polling claim would be the
//                                      tautology this program keeps measuring.
//   * "the holder gets EVERYTHING"   — from the FIXTURE, counted per message.
//     Without this last one a fix that stopped BOTH processes polling would
//     pass every "no duplicate consumption" check. `max_open == 0` is graded
//     DENIAL, which is a FAILURE, not a pass.
// ───────────────────────────────────────────────────────────────────────────

async function leg3(run, n = 8) {
  run.note('=== LEG 3: service owns the lease, an ordinary session arrives ===');

  const { child: gw, logPath: gwLog } = run.startGateway('l3');
  if (!(await run.waitForPolls(3, 90_000, 'service l3 warmup'))) {
    return {
      leg: 3,
      verdict: 'INCOMPLETE',
      instrument: { fault: true, reasons: ['service never polled during warmup'] },
    };
  }

  // The session arrives SECOND, which is the scenario: an installed service is
  // already receiving, and the user opens a normal session for unrelated work.
  const { child: sess, logPath: sessLog } = run.startSession('l3');
  sleep(6000); // let it boot and make its lease decision

  const wStart = new Date().toISOString();
  const submitted = [];
  for (let i = 0; i < n; i += 1) {
    const r = await run.submit(`F24CL-L3-${run.runId}-${i}`);
    submitted.push(r.update_id);
    run.note(`leg3: submitted ${i + 1}/${n}, ${new Date().toISOString()}`);
    sleep(2000);
  }
  sleep(4000); // let the owner drain the tail
  const wEnd = new Date().toISOString();

  const report = await run.report();
  const maxOpen = maxOpenInWindow(report.concurrency_trace, wStart, wEnd);
  const polls = pollsInWindow(report.concurrency_trace, wStart, wEnd);

  // THE POSITIVE PATH, counted per message.
  const delivered = report.updates.filter(
    (u) => submitted.includes(u.update_id) && u.serve_count > 0,
  );

  const sessText = fs.readFileSync(sessLog, 'utf8');
  const gwText = fs.readFileSync(gwLog, 'utf8');
  const loserWasLoud = containsToken(sessText, 'F24_CHANNEL_LEASE=observer');
  const holderClaimedOwner = containsToken(gwText, 'F24_CHANNEL_LEASE=owner');

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
    expect_boot: true,
    llm_hit: run.llm.hits > 0,
    expect_polling: true,
    polls_total: report.poll_total,
  };
  const grade = gradeInstrument(facts);

  let verdict;
  if (grade.fault) verdict = 'INCOMPLETE';
  else if (maxOpen === 0) verdict = 'DENIAL — nothing polled; a green here would be manufactured';
  else if (maxOpen > 1) verdict = 'STILL RACING — two processes polled with the lease in place';
  else if (delivered.length !== n)
    verdict = `HOLDER STARVED — only ${delivered.length}/${n} reached the owner`;
  else if (!loserWasLoud)
    verdict = 'SILENT LOSER — exactly one poller, but the other said nothing (a NEW silent failure)';
  else verdict = 'LEASE HOLDS — one poller, holder got everything, loser was loud';

  return {
    leg: 3,
    name: 'service owns the lease, an ordinary session arrives',
    submitted: submitted.length,
    delivered_to_holder: delivered.length,
    max_open_in_window: maxOpen,
    polls_in_window: polls,
    loser_emitted_observer_token: loserWasLoud,
    holder_emitted_owner_token: holderClaimedOwner,
    session_log_bytes: byteLen(sessLog),
    gateway_log_bytes: byteLen(gwLog),
    window: [wStart, wEnd],
    llm_hits: run.llm.hits,
    instrument: grade,
    verdict,
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

function selfTest(assert) {
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
}

/**
 * Self-test for the SECOND instrument repair — the LLM stub moved out of
 * process. Live, because the defect was a runtime deadlock that no amount of
 * pure-function testing would have shown.
 *
 * Three assertions again, and the third is the one that matters: it proves the
 * OLD in-process shape genuinely could not work, so the repair is not
 * decoration.
 */
async function selfTestStub(results, assert) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'f24cl-selftest-'));
  const journal = path.join(dir, 'llm.jsonl');
  const children = [];
  const stub = new LlmStub(50, journal, dir, children, () => {});

  // 1. KNOWN-POSITIVE: the out-of-process stub binds AND still answers after
  //    this driver has blocked its own event loop — the exact condition the
  //    steady-state leg creates for 2s at a time between submissions.
  let url = null;
  let sse = '';
  try {
    url = stub.start();
    sleep(1200); // block the driver's loop, as the real legs do
    sse = await httpText(`${url}/v1/chat/completions`, {
      method: 'POST',
      body: { model: 'x', messages: [] },
      timeoutMs: 8000,
    });
  } catch (e) {
    sse = `ERR:${e.message}`;
  }
  // Assert the TERMINAL event, not merely "some bytes came back". A stream that
  // ends without `[DONE]` is exactly what made the engine retry and trip its
  // circuit breaker in the first live run, so a weaker assertion here would
  // have passed on the broken stub.
  assert(
    'stub known-positive: out-of-process stub streams a terminated SSE turn after the driver blocks its own loop',
    sse.includes('data: [DONE]') && sse.includes('"finish_reason":"stop"'),
    `url=${url} bytes=${sse.length}`,
  );

  // 2. KNOWN-NEGATIVE: hits are read from the stub's own journal, written by
  //    another process — so a stub that was never called reads 0, and the
  //    grader can still tell "never booted" from "booted and silent".
  const freshJournal = path.join(dir, 'never-called.jsonl');
  fs.writeFileSync(freshJournal, '');
  const coldStub = new LlmStub(1, freshJournal, dir, [], () => {});
  assert(
    'stub known-negative: an uncalled stub reports 0 hits, so a fabricated hit count cannot pass',
    coldStub.hits === 0 && stub.hits >= 1,
    `cold=${coldStub.hits} live=${stub.hits}`,
  );

  // 3. THE OLD SHAPE WOULD HAVE MISSED IT. Reproduce the original in-process
  //    deadlock: listen(), then block the loop exactly as the old `start()`
  //    did. `address()` stays null forever, so the old code could only ever
  //    throw 'llm stub never bound' — which is precisely what the first live
  //    run reported. Without this assertion the repair is unproven.
  const inProc = http.createServer(() => {});
  inProc.listen(0, '127.0.0.1');
  let addrDuringBlock = inProc.address();
  for (let i = 0; i < 20 && !addrDuringBlock; i += 1) {
    sleep(25); // the OLD blocking busy-wait, verbatim in shape
    addrDuringBlock = inProc.address();
  }
  assert(
    'the OLD in-process stub would have MISSED it — listen() cannot complete while the loop is blocked',
    addrDuringBlock === null,
    `address during blocking wait = ${JSON.stringify(addrDuringBlock)}`,
  );
  inProc.close();

  stub.stop();
  for (const c of children) {
    try {
      c.kill('SIGKILL');
    } catch {
      /* ignore */
    }
  }
  fs.rmSync(dir, { recursive: true, force: true });
  return results;
}

/**
 * Self-test for the token matcher. Third assertion again: the NAIVE matcher
 * (plain `String.includes`) must be shown to MISS the wrapped case, or this
 * self-test would pass just as happily on the broken matcher.
 */
function selfTestMatcher(assert) {
  const naive = (h, t) => String(h).includes(t);
  const TOKEN = 'F24_CHANNEL_LEASE=observer';

  const plain = `prefix F24_CHANNEL_LEASE=observer owner_pid=42 suffix`;
  // A console line wrap puts a newline INSIDE the token. Measured twice on this
  // program; the second time because the first sighting was documented instead
  // of repaired.
  const wrapped = `prefix F24_CHANNEL_LE\nASE=observer owner_pid=42 suffix`;
  const absent = `prefix F24_CHANNEL_LEASE=owner owner_pid=42 suffix`;

  assert(
    'matcher known-positive: an unwrapped token is found',
    containsToken(plain, TOKEN),
  );
  assert(
    'matcher known-negative: a DIFFERENT token (=owner) is not mistaken for =observer',
    !containsToken(absent, TOKEN),
  );
  assert(
    'the NAIVE matcher would have MISSED it — a wrap inside the token reads as absence',
    naive(wrapped, TOKEN) === false && containsToken(wrapped, TOKEN) === true,
    `naive=${naive(wrapped, TOKEN)} robust=${containsToken(wrapped, TOKEN)}`,
  );
}

async function runSelfTests() {
  const results = [];
  const assert = (name, cond, detail) => {
    results.push({ name, pass: Boolean(cond), detail });
    process.stdout.write(`${cond ? 'PASS' : 'FAIL'}  ${name}${detail ? ` — ${detail}` : ''}\n`);
  };
  selfTest(assert);
  selfTestMatcher(assert);
  await selfTestStub(results, assert);
  const failed = results.filter((r) => !r.pass);
  process.stdout.write(
    `\nSELFTEST_TOTAL=${results.length} SELFTEST_PASSED=${results.length - failed.length} SELFTEST_FAILED=${failed.length}\n`,
  );
  return failed.length === 0 ? 0 : 1;
}

// ───────────────────────────────────────────────────────────────────────────

function parseArgs(argv) {
  const out = {
    binary: null,
    runDir: null,
    leg: 'all',
    selfTest: false,
    holdMs: 30_000,
    llmStub: false,
    journal: null,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--binary') out.binary = argv[++i];
    else if (a === '--run-dir') out.runDir = argv[++i];
    else if (a === '--leg') out.leg = argv[++i];
    else if (a === '--hold-ms') out.holdMs = Number(argv[++i]);
    else if (a === '--self-test') out.selfTest = true;
    else if (a === '--llm-stub') out.llmStub = true;
    else if (a === '--journal') out.journal = argv[++i];
    else {
      process.stderr.write(`f24-channel-lease: unknown argument ${a}\n`);
      process.exit(2);
    }
  }
  return out;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.llmStub) {
    // Runs as the out-of-process LLM stub and never returns.
    runAsLlmStub(args.holdMs, args.journal);
    return new Promise(() => {});
  }
  if (args.selfTest) process.exit(await runSelfTests());
  if (!args.binary || !args.runDir) {
    process.stderr.write('f24-channel-lease: --binary and --run-dir are required\n');
    process.exit(2);
  }

  const run = new Run(args);
  const out = { run_id: run.runId, legs: [], notes: [] };
  try {
    run.llm = new LlmStub(
      args.holdMs,
      path.join(run.runDir, 'llm.jsonl'),
      run.runDir,
      run.children,
      (t) => run.note(t),
    );
    run.llmUrl = run.llm.start();
    run.note(`llm stub at ${run.llmUrl} holding ${args.holdMs}ms`);
    run.startTgFixture();
    run.writeConfig();

    const bi = spawnSync(args.binary, ['--build-info'], { encoding: 'utf8' });
    out.build_info = `${bi.stdout}${bi.stderr}`.trim().split('\n').slice(0, 3).join(' | ');
    run.note(`binary: ${out.build_info}`);

    const legs = args.leg === 'all' ? ['1', '2', '3', '4'] : args.leg.split(',');
    for (const l of legs) {
      if (l === '1') out.legs.push(await leg1(run));
      else if (l === '2') out.legs.push(await leg2(run));
      else if (l === '3') out.legs.push(await leg3(run));
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
