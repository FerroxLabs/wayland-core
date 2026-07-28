#!/usr/bin/env node
// F24-C3-H4 — does `gateway run` start TWO ChannelManagers, and does the second
// one STEAL inbound messages from a polling adapter?
//
// 24-C3-H2 proved the double registration from the gateway's own log and said
// plainly that it could NOT measure the consumption race, because the adapters
// the race affects are precisely the ones with no fixture seam. This driver is
// that measurement. It reports two things that are independent of each other
// and independent of anything the binary says about itself:
//
//   POLLERS  `max_concurrent_getupdates`, counted by the fixture from
//            overlapping open requests, in a different OS process. Two managers
//            polling one bot token show up as 2. One manager shows up as 1. A
//            runtime that polls NOTHING shows up as 0 — a DISTINCT answer, and
//            a failing one, so a "fix" that works by making nothing start
//            cannot pass this.
//
//   LOSS     of N submitted inbound messages, how many produced a reply back
//            through the SAME adapter carrying the correlation token of the
//            message that caused it. A missing token is a message that was
//            consumed and never answered. Both a lost token and a duplicated
//            one are failures, and they are reported separately, because a fix
//            that turns loss into duplication has not fixed anything.
//
// WHY THE PENDING QUEUE IS PRE-LOADED. The gateway's two managers do not start
// simultaneously: the cron handler's manager used to be built and `start_all`d
// at the TOP of `run_gateway`, and the gateway's own manager some way further
// down, after the credentials store opens, the adapters register and the
// inbound host builds a provider. Anything already queued at the account is
// therefore consumed by manager #1 during that window and confirmed away before
// manager #2 ever polls. That is not a contrived case: it is the ordinary
// restart of a gateway that was down while messages arrived, which is exactly
// what Telegram's ~24h retention of unconfirmed updates exists for. Messages
// are ALSO submitted after startup, so a steady-state loss would be caught too.
//
// The reply is produced by a real agent turn against a fixture model reached
// through a config alias's `base_url`, so no vendor credential is involved and
// the reply text is attributable to its cause.
//
// usage: f24-c3-h4-polling-race.mjs --binary <path> --run-dir <dir>
//                                   [--preload 4] [--live 4] [--budget-ms 120000]

import { spawn, spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));

function sleep(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

function sha256File(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

// Telegram's default parse mode is MarkdownV2, and the adapter escapes EVERY
// reserved character in the body before sending — so the correlation token
// `f24c3-h4-pre-0-ab12` leaves the product as `f24c3\-h4\-pre\-0\-ab12`.
//
// This cost a whole measurement pass. A naive `text.includes(token)` reported
// `replied=0/8` on a run whose eight replies had all arrived correctly, which
// would have been written up as total inbound loss on a WORKING path. The
// instrument was carrying the exact defect class it was hunting. Un-escape
// before matching, and see `instrument_fault` below for the check that now
// refuses to report that shape as loss ever again.
function unescapeMarkdownV2(s) {
  return String(s).replace(/\\(.)/g, '$1');
}

function httpJson(url, method, body) {
  return new Promise((resolve, reject) => {
    const u = new URL(url);
    const payload = body === undefined ? null : JSON.stringify(body);
    const req = http.request(
      {
        hostname: u.hostname,
        port: u.port,
        path: u.pathname + u.search,
        method,
        headers: payload
          ? { 'content-type': 'application/json', 'content-length': Buffer.byteLength(payload) }
          : {},
        timeout: 15_000,
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
            reject(new Error(`bad json from ${url}: ${data.slice(0, 200)}`));
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

class Race {
  constructor(args) {
    this.args = args;
    this.runDir = path.resolve(args.runDir);
    this.home = path.join(this.runDir, 'home');
    this.children = [];
    this.notes = [];
    this.runId = crypto.randomBytes(4).toString('hex');
    this.botToken = `770${crypto.randomInt(100000, 999999)}:F24C3H4-${this.runId}`;
    this.vaultPassphrase = crypto.randomBytes(24).toString('hex');
    this.chatId = '770001';
    this.senderId = '770001';
    this.tgJournal = path.join(this.runDir, 'tg-fixture.jsonl');
    this.llmJournal = path.join(this.runDir, 'llm.jsonl');
  }

  note(text) {
    this.notes.push(text);
    process.stdout.write(`[race] ${text}\n`);
  }

  startFixture(script, extraArgs, readyRe, logName) {
    const logPath = path.join(this.runDir, logName);
    fs.writeFileSync(logPath, '');
    const fd = fs.openSync(logPath, 'a');
    const child = spawn(process.execPath, [path.join(HERE, script), ...extraArgs], {
      stdio: ['ignore', fd, fd],
      windowsHide: true,
    });
    child.unref();
    this.children.push(child);
    let banner = '';
    for (let i = 0; i < 100; i += 1) {
      banner = fs.readFileSync(logPath, 'utf8');
      if (readyRe.test(banner)) break;
      sleep(100);
    }
    const m = readyRe.exec(banner);
    if (!m) throw new Error(`${script} never signalled ready:\n${banner}`);
    return m[1];
  }

  writeConfig() {
    fs.mkdirSync(path.join(this.home, 'channels'), { recursive: true });

    fs.writeFileSync(
      path.join(this.home, 'credentials.toml'),
      ['[secrets]', `"telegram.f24c3h4.bot_token" = "${this.botToken}"`, ''].join('\n'),
      { mode: 0o600 },
    );

    // The webhook host is deliberately DISABLED. This measurement is about the
    // POLLING inbound path, the subscriber is spawned either way (proven by
    // `channel_inbound_host::a_disabled_webhook_still_spawns_the_subscriber_for_polling_adapters`),
    // and not binding a fixed port keeps this runnable beside the other lanes
    // on the same box.
    fs.writeFileSync(
      path.join(this.home, 'config.toml'),
      [
        '[default]',
        'provider = "f24c3h4fixture"',
        '',
        '[providers.f24c3h4fixture]',
        'provider = "openai"',
        'model = "f24c3-fixture"',
        'api_key = "f24c3h4-not-a-real-key"',
        `base_url = "${this.llmUrl}"`,
        '',
        '[inbound_webhook]',
        'enabled = false',
        '',
      ].join('\n'),
      { mode: 0o600 },
    );

    fs.writeFileSync(
      path.join(this.home, 'channels', 'f24c3h4tg.toml'),
      [
        'name = "f24c3h4tg"',
        'platform = "telegram"',
        'enabled = true',
        '',
        '[options]',
        'credential_handle = "telegram.f24c3h4.bot_token"',
        // The seam. Without `api_base_url` on TelegramConfig this line is a
        // `deny_unknown_fields` parse error and the adapter can only ever reach
        // api.telegram.org — which is why the polling path had never been
        // driven end to end.
        `api_base_url = "${this.tgUrl}"`,
        // Short polls, so the two loops interleave many times inside the run
        // rather than sitting in one 30s long-poll each. The fixture still
        // holds an empty result open for up to --max-wait-ms, which is what
        // makes concurrent pollers observable.
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

  // THE OTHER HALF OF THE FIX'S PROOF. The cron handler's manager was removed;
  // if the scheduler quietly stopped being able to reach a channel, this lane
  // would have traded a message-loss risk for a dead scheduler and every
  // "no duplicate registration" check would still be green.
  //
  // `cron add` writes to `$WAYLAND_HOME/cron/jobs.json`, which is the same
  // store `run_gateway` opens, so this is the product's own surface rather than
  // a file this driver forged.
  addCronJob() {
    const text = `F24C3H4-CRON ${this.runId}`;
    const r = spawnSync(
      this.args.binary,
      ['cron', 'add', '* * * * *', '--channel', 'f24c3h4tg', '--text', text],
      { encoding: 'utf8', env: { ...process.env, WAYLAND_HOME: this.home } },
    );
    this.note(`cron add rc=${r.status}: ${`${r.stdout}${r.stderr}`.trim().split('\n')[0]}`);
    if (r.status !== 0) throw new Error(`cron add failed rc=${r.status}: ${r.stdout}${r.stderr}`);
    this.cronText = text;
    return text;
  }

  buildInfo() {
    const r = spawnSync(this.args.binary, ['--build-info'], { encoding: 'utf8' });
    if (r.status !== 0) throw new Error(`--build-info failed rc=${r.status}`);
    return `${r.stdout}${r.stderr}`.trim();
  }

  startBinary() {
    const logPath = path.join(this.runDir, 'core.log');
    fs.writeFileSync(logPath, '');
    const fd = fs.openSync(logPath, 'a');
    // Foreground `gateway run` — a detached gateway re-execs and this driver
    // would lose the child it must reap, and would then be measuring a process
    // it does not own.
    const child = spawn(this.args.binary, ['gateway', 'run'], {
      stdio: ['pipe', fd, fd],
      env: {
        ...process.env,
        WAYLAND_HOME: this.home,
        // 24-C3-H2 measured this: an isolated profile with NO vault passphrase
        // stores credentials plaintext-0600 and then refuses EVERY turn with
        // "Session persistence authority unavailable" — host-wide, not
        // channel-specific. Without it a credentials-posture refusal would be
        // attributed to the polling path. Minted for this run; not a vendor
        // credential and never printed.
        WAYLAND_VAULT_PASSPHRASE: this.vaultPassphrase,
        RUST_LOG:
          process.env.RUST_LOG ??
          'info,wcore_agent::channel_inbound=debug,wcore_channels=debug,wcore_channel_telegram=debug',
      },
    });
    this.children.push(child);
    this.binaryChild = child;
    this.coreLog = logPath;
  }

  submit(token) {
    return httpJson(`${this.tgUrl}/__control/submit`, 'POST', {
      token,
      chatId: this.chatId,
      senderId: this.senderId,
      username: 'f24c3h4user',
      // `f24c3-...` is the correlation shape the fixture model echoes back.
      text: `@bot please ack ${token}`,
    });
  }

  async report() {
    return httpJson(`${this.tgUrl}/__control/report`, 'GET');
  }

  // Turns that actually ran, counted from the model fixture's own journal in a
  // third OS process. "A turn ran" and "a reply came back out of the adapter"
  // are different claims and a run where they disagree is telling you
  // something, so they are never collapsed into one number.
  llmTurns() {
    try {
      return fs
        .readFileSync(this.llmJournal, 'utf8')
        .split('\n')
        .filter((l) => l.includes('"kind":"chat.completions"')).length;
    } catch {
      return 0;
    }
  }

  cleanup() {
    for (const c of this.children) {
      try {
        c.kill('SIGTERM');
      } catch {
        /* already gone */
      }
    }
    sleep(600);
    for (const c of this.children) {
      try {
        c.kill('SIGKILL');
      } catch {
        /* already gone */
      }
    }
  }

  async execute() {
    fs.mkdirSync(this.runDir, { recursive: true });
    fs.rmSync(this.home, { recursive: true, force: true });
    fs.rmSync(this.tgJournal, { force: true });
    fs.rmSync(this.llmJournal, { force: true });

    this.tgUrl = this.startFixture(
      'f24-tg-fixture.mjs',
      ['--journal', this.tgJournal, '--token', this.botToken, '--max-wait-ms', '2000'],
      /TGFIX_READY url=(\S+)/,
      'tg-fixture.log',
    );
    this.note(`telegram fixture ${this.tgUrl}`);

    this.llmUrl = this.startFixture(
      'f24-llm-fixture.mjs',
      ['--journal', this.llmJournal],
      /LLM_READY url=(\S+)/,
      'llm.log',
    );
    this.note(`llm fixture ${this.llmUrl}`);

    this.writeConfig();

    const info = this.buildInfo();
    const digest = sha256File(this.args.binary);
    this.note(`binary ${this.args.binary}`);
    this.note(`build-info ${info}`);
    this.note(`sha256 ${digest}`);

    // ── preload: messages already queued when the gateway comes up ─────────
    const tokens = [];
    for (let i = 0; i < this.args.preload; i += 1) {
      const t = `f24c3-h4-pre-${i}-${this.runId}`;
      const r = await this.submit(t);
      tokens.push({ token: t, phase: 'preload', update_id: r.update_id });
    }
    this.note(`preloaded ${this.args.preload} pending updates before start`);

    if (this.args.cron) this.addCronJob();

    this.startBinary();

    // ── live: messages arriving while the gateway is up ────────────────────
    // `--settle-ms` decides WHICH question this leg answers, and they are
    // different questions. A short settle keeps the live messages inside the
    // gateway's startup window, where the two managers start seconds apart and
    // the first one sweeps the queue. A long settle puts them in STEADY STATE,
    // where both loops are established and long-polling, and loss (if any) is
    // then an ongoing property rather than a startup artifact. Claiming the
    // first as evidence for the second would overstate the finding.
    this.note(`settling ${this.args.settleMs}ms before the live submissions`);
    for (let s = 0; s < this.args.settleMs; s += 10_000) {
      sleep(Math.min(10_000, this.args.settleMs - s));
      this.note(`settle ${Math.min(s + 10_000, this.args.settleMs)}/${this.args.settleMs}ms`);
    }
    for (let i = 0; i < this.args.live; i += 1) {
      const t = `f24c3-h4-live-${i}-${this.runId}`;
      const r = await this.submit(t);
      tokens.push({ token: t, phase: 'live', update_id: r.update_id });
      sleep(500);
    }
    this.note(`submitted ${this.args.live} live updates after start`);

    // ── wait for replies, noisily ─────────────────────────────────────────
    const deadline = Date.now() + this.args.budgetMs;
    let rep = null;
    let iteration = 0;
    while (Date.now() < deadline) {
      iteration += 1;
      rep = await this.report();
      const seen = new Set();
      for (const r of rep.replies) {
        const plain = unescapeMarkdownV2(r.text);
        for (const t of tokens) if (plain.includes(t.token)) seen.add(t.token);
      }
      this.note(
        `wait ${iteration}: replied=${seen.size}/${tokens.length} turns=${this.llmTurns()} ` +
          `pollers_max=${rep.max_concurrent_getupdates} polls=${rep.poll_total} ` +
          `pending=${rep.still_pending.length}`,
      );
      const cronDone =
        !this.args.cron ||
        rep.replies.some((r) => unescapeMarkdownV2(r.text).includes(this.cronText));
      if (seen.size === tokens.length && cronDone) break;
      sleep(5000);
    }

    rep = await this.report();
    const replyTexts = rep.replies.map((r) => unescapeMarkdownV2(r.text));
    const perToken = tokens.map((t) => {
      const hits = replyTexts.filter((x) => x.includes(t.token)).length;
      const h = rep.updates.find((u) => u.update_id === t.update_id);
      return {
        ...t,
        replies: hits,
        served_to_polls: h ? h.served_to : [],
        deleted_by_poll: h ? h.deleted_by : null,
      };
    });

    const lost = perToken.filter((t) => t.replies === 0);
    const duplicated = perToken.filter((t) => t.replies > 1);
    const turns = this.llmTurns();

    // THE INSTRUMENT-FAULT CHECK. The adapter physically delivered replies to
    // the fixture, but none of them matched a token we submitted. That is not
    // message loss — it is this driver failing to recognise the product's own
    // output, which is precisely what the MarkdownV2 escaping did on the first
    // pass. Reporting it as loss would have been a fabricated defect. Loss is
    // only loss when NOTHING came back.
    const instrumentFault =
      rep.replies.length > 0 && lost.length === perToken.length
        ? `the adapter delivered ${rep.replies.length} replies but none matched a submitted ` +
          `correlation token — this driver cannot read the product's output, so no loss ` +
          `claim may be made from this run. Sample: ${JSON.stringify(rep.replies[0]?.text ?? '')}`
        : null;
    if (instrumentFault) this.note(`INSTRUMENT FAULT: ${instrumentFault}`);

    const coreLog = fs.readFileSync(this.coreLog, 'utf8');
    // The gateway's own account of itself, kept ONLY as corroboration. The
    // load-bearing number is the fixture's, because it is measured in another
    // process from real HTTP traffic.
    const registrationLines = coreLog
      .split('\n')
      .filter((l) => /channel auto-registered|channels registered=|inbound:/.test(l));

    this.cleanup();

    const result = {
      finding: 'F24-C3-H4',
      binary: this.args.binary,
      binary_sha256: digest,
      build_info: info,
      run_id: this.runId,
      submitted_total: tokens.length,
      preload: this.args.preload,
      live: this.args.live,
      settle_ms: this.args.settleMs,
      replied_total: perToken.filter((t) => t.replies >= 1).length,
      turns_total: turns,
      raw_replies_total: rep.replies.length,
      instrument_fault: instrumentFault,
      cron_requested: Boolean(this.args.cron),
      cron_text: this.cronText ?? null,
      cron_fires: this.args.cron
        ? replyTexts.filter((t) => t.includes(this.cronText)).length
        : null,
      lost_total: lost.length,
      lost: lost.map((t) => ({ token: t.token, phase: t.phase, update_id: t.update_id })),
      duplicated_total: duplicated.length,
      duplicated: duplicated.map((t) => ({ token: t.token, replies: t.replies })),
      max_concurrent_getupdates: rep.max_concurrent_getupdates,
      poll_total: rep.poll_total,
      still_pending: rep.still_pending,
      per_token: perToken,
      replies: rep.replies,
      gateway_log_lines: registrationLines,
      notes: this.notes,
    };

    const out = path.join(this.runDir, 'f24-c3-h4-race-result.json');
    fs.writeFileSync(out, `${JSON.stringify(result, null, 2)}\n`);
    process.stdout.write(
      `\nF24C3H4 RACE submitted=${result.submitted_total} turns=${result.turns_total} ` +
        `replied=${result.replied_total} lost=${result.lost_total} ` +
        `duplicated=${result.duplicated_total} ` +
        `max_concurrent_getupdates=${result.max_concurrent_getupdates} polls=${result.poll_total} ` +
        `cron_fires=${result.cron_fires} instrument_fault=${result.instrument_fault ? 'YES' : 'no'}\n`,
    );
    process.stdout.write(`F24C3H4 RESULT ${out}\n`);
    return result;
  }
}

function parseArgs(argv) {
  const out = {
    binary: null,
    runDir: path.join(os.tmpdir(), 'f24-c3-h4-race'),
    preload: 4,
    live: 4,
    budgetMs: 120_000,
    settleMs: 4_000,
    cron: false,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--binary') out.binary = argv[++i];
    else if (a === '--run-dir') out.runDir = argv[++i];
    else if (a === '--preload') out.preload = Number(argv[++i]);
    else if (a === '--live') out.live = Number(argv[++i]);
    else if (a === '--budget-ms') out.budgetMs = Number(argv[++i]);
    else if (a === '--settle-ms') out.settleMs = Number(argv[++i]);
    else if (a === '--cron') out.cron = true;
    else {
      process.stderr.write(`f24-c3-h4-polling-race: unknown argument ${a}\n`);
      process.exit(2);
    }
  }
  if (!out.binary) {
    process.stderr.write('f24-c3-h4-polling-race: --binary is required\n');
    process.exit(2);
  }
  return out;
}

const race = new Race(parseArgs(process.argv.slice(2)));
try {
  await race.execute();
  process.exit(0);
} catch (e) {
  race.cleanup();
  process.stderr.write(`f24-c3-h4-polling-race: ${e && e.stack ? e.stack : e}\n`);
  process.exit(1);
}
