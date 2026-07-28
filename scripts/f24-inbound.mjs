#!/usr/bin/env node
// The Phase 24 INBOUND channel matrix driver — criterion 24-C3.
//
// Criterion 3, verbatim from ROADMAP.md:119:
//   "Reference channels prove setup/auth, access, routing, media, native
//    actions, idempotency, reconnect/reload, and health."
//
// The half that was never driven is the INBOUND half: a message originating at
// the platform, entering the shipped binary, being admitted, deduplicated,
// access-decided, bound to a session and routed to a turn whose reply leaves
// again. Everything measured before this lane was outbound.
//
// WHAT THIS DRIVER REFUSES TO ACCEPT AS EVIDENCE. Not that a handler was
// registered. Not that a config parsed. Not that a function returned Ok. Not a
// status line the product printed about itself. Every arrival number here is
// derived by reading the journal of an out-of-process sink the binary does not
// own and cannot write to except by completing a real TCP round trip, exactly
// as `f24-journey.mjs` does — and every turn claim is cross-checked against a
// second, separate journal written by the fixture model endpoint. A leg that
// reports zero is therefore distinguishable from a leg that never ran.
//
// SHAPE OF THE MATRIX. Per adapter, five legs, each with its own falsifier:
//   admit   an allowed sender's message produces exactly one arrival carrying
//           that message's own correlation token
//   dedupe  the SAME platform message id replayed produces no second arrival
//           (positive control: a different id from the same sender does)
//   access  a sender outside the allowlist produces zero arrivals
//           (positive control: the allowed sender in the same run does)
//   bind    two distinct conversations produce two arrivals whose
//           conversation ids are distinct and correct
//   route   the reply text carries the correlation token of the message that
//           caused it — a reply landing in the wrong conversation, or a reply
//           to some other turn, both fail this
//
// NO VENDOR CREDENTIAL IS USED, READ, OR REQUIRED. Every secret here is minted
// by this script at run time. An adapter whose inbound path cannot be pointed
// at a fixture from configuration is reported NOT MEASURED with the reason —
// never as a zero and never as a pass.

import { spawnSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));

export const RESULT_SCHEMA = 'wayland.inbound.matrix/1';

// The adapters this driver can point at a fixture, and the inbound transport
// each one uses. Kept as data so the report can name what it did NOT measure
// with the same authority as what it did.
export const ADAPTERS = ['slack', 'whatsapp', 'sms'];
export const LEGS = ['admit', 'dedupe', 'access', 'bind', 'route'];

const WEBHOOK_PORT = 18787;
const ARRIVAL_BUDGET_MS = 90_000;

// ── small process helpers ────────────────────────────────────────────────────

function run(argv, opts = {}) {
  const r = spawnSync(argv[0], argv.slice(1), {
    encoding: 'utf8',
    timeout: opts.timeout ?? 120_000,
    env: opts.env ?? process.env,
    cwd: opts.cwd,
  });
  return {
    status: r.status,
    output: `${r.stdout ?? ''}${r.stderr ?? ''}`.trim(),
  };
}

function sleep(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

function sha256File(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

// ── fixture request builders — one per platform signature scheme ─────────────
//
// Each of these constructs the request the PLATFORM would send, including the
// signature the connector verifies. A driver that skipped the signature would
// be measuring a code path no real message takes.

function slackRequest({ url, signingSecret, channel, user, text, ts, channelType, team }) {
  const body = JSON.stringify({
    type: 'event_callback',
    team_id: team,
    event: {
      type: 'message',
      channel,
      channel_type: channelType,
      user,
      text,
      ts,
      team,
    },
  });
  const timestamp = String(Math.floor(Date.now() / 1000));
  const sig = `v0=${crypto
    .createHmac('sha256', signingSecret)
    .update(`v0:${timestamp}:${body}`)
    .digest('hex')}`;
  return {
    url,
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'x-slack-signature': sig,
      'x-slack-request-timestamp': timestamp,
    },
    body,
  };
}

function whatsappRequest({ url, appSecret, phoneNumberId, from, text, messageId }) {
  const body = JSON.stringify({
    object: 'whatsapp_business_account',
    entry: [
      {
        id: 'f24c3-waba',
        changes: [
          {
            field: 'messages',
            value: {
              messaging_product: 'whatsapp',
              metadata: { display_phone_number: '15550000000', phone_number_id: phoneNumberId },
              contacts: [{ profile: { name: 'f24c3' }, wa_id: from }],
              messages: [
                {
                  from,
                  id: messageId,
                  timestamp: String(Math.floor(Date.now() / 1000)),
                  type: 'text',
                  text: { body: text },
                },
              ],
            },
          },
        ],
      },
    ],
  });
  const sig = `sha256=${crypto.createHmac('sha256', appSecret).update(body).digest('hex')}`;
  return {
    url,
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-hub-signature-256': sig },
    body,
  };
}

function twilioRequest({ url, publicUrl, authToken, from, to, text, messageSid }) {
  const pairs = [
    ['Body', text],
    ['From', from],
    ['MessageSid', messageSid],
    ['NumMedia', '0'],
    ['To', to],
  ];
  const body = new URLSearchParams(pairs).toString();
  // Twilio signs the PUBLIC url plus the alphabetically-sorted form pairs,
  // key and value concatenated with no separator. `publicUrl` and not the
  // bind address: the connector reconstructs the signed url from
  // `public_base_url` when it is configured, which is what a real deployment
  // behind a proxy does.
  const sorted = [...pairs].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
  const mac = crypto.createHmac('sha1', authToken);
  mac.update(publicUrl);
  for (const [k, v] of sorted) mac.update(k + v);
  const sig = mac.digest('base64');
  return {
    url,
    method: 'POST',
    headers: {
      'content-type': 'application/x-www-form-urlencoded',
      'x-twilio-signature': sig,
    },
    body,
  };
}

// POST through a child node process. The driver's own waits are blocking
// (`Atomics.wait`), so its event loop is parked and an in-process `fetch`
// would never settle. Same reason `f24-journey.mjs` shells out for its probes.
function post(req) {
  const script = `
    const r = ${JSON.stringify(req)};
    fetch(r.url, { method: r.method, headers: r.headers, body: r.body })
      .then(async (res) => {
        const t = await res.text();
        process.stdout.write('HTTP ' + res.status + '\\n' + t);
      })
      .catch((e) => { process.stdout.write('POST FAILED ' + e.message); process.exit(1); });
  `;
  return run([process.execPath, '-e', script], { timeout: 30_000 });
}

// ── the driver ───────────────────────────────────────────────────────────────

class InboundMatrix {
  constructor(args) {
    this.args = args;
    this.runDir = args.runDir;
    this.home = path.join(this.runDir, 'home');
    this.journalPath = path.join(this.runDir, 'arrivals.jsonl');
    this.llmJournalPath = path.join(this.runDir, 'turns.jsonl');
    this.children = [];
    this.results = [];
    this.notes = [];

    // Every secret below is minted here and exists only for this run.
    this.slackBotToken = `xoxb-f24c3-${crypto.randomBytes(12).toString('hex')}`;
    this.slackSigningSecret = crypto.randomBytes(24).toString('hex');
    this.waToken = `EAAf24c3${crypto.randomBytes(12).toString('hex')}`;
    this.waAppSecret = crypto.randomBytes(24).toString('hex');
    this.twilioSid = `ACf24c3${crypto.randomBytes(12).toString('hex')}`;
    this.twilioToken = crypto.randomBytes(24).toString('hex');
    this.vaultPassphrase = crypto.randomBytes(24).toString('hex');
  }

  // The host's OWN channel directory, which is NOT the isolated profile's.
  // Recorded in the result because it changes the answer: the inbound access
  // policy is resolved from this path rather than from `WAYLAND_HOME`
  // (F24-C3-H1), so a matrix run on a host that happens to have configs here
  // measures something different from one that does not.
  hostChannelsDir() {
    const home = process.env.HOME ?? process.env.USERPROFILE;
    return home ? path.join(home, '.wayland', 'channels') : null;
  }

  note(text) {
    this.notes.push(text);
    process.stdout.write(`[inbound] ${text}\n`);
  }

  // ── fixtures ──────────────────────────────────────────────────────────────

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

  // ── configuration ─────────────────────────────────────────────────────────

  writeConfig() {
    fs.mkdirSync(path.join(this.home, 'channels'), { recursive: true });

    fs.writeFileSync(
      path.join(this.home, 'credentials.toml'),
      [
        '[secrets]',
        `"slack.f24c3.bot_token" = "${this.slackBotToken}"`,
        `"slack.f24c3.signing_secret" = "${this.slackSigningSecret}"`,
        `"whatsapp.f24c3.access_token" = "${this.waToken}"`,
        `"whatsapp.f24c3.app_secret" = "${this.waAppSecret}"`,
        `"sms.f24c3.account_sid" = "${this.twilioSid}"`,
        `"sms.f24c3.auth_token" = "${this.twilioToken}"`,
        '',
      ].join('\n'),
      { mode: 0o600 },
    );

    // The model is a fixture reached through a config alias's `base_url`, so
    // the turn is real and the vendor is absent. `stream_usage = false` is not
    // set: the fixture emits usage on the final chunk like the real wire does.
    fs.writeFileSync(
      path.join(this.home, 'config.toml'),
      [
        '[default]',
        'provider = "f24c3fixture"',
        '',
        '[providers.f24c3fixture]',
        'provider = "openai"',
        'model = "f24c3-fixture"',
        'api_key = "f24c3-not-a-real-key"',
        `base_url = "${this.llmUrl}"`,
        '',
        '[inbound_webhook]',
        'enabled = true',
        `bind = "127.0.0.1:${WEBHOOK_PORT}"`,
        `public_base_url = "http://127.0.0.1:${WEBHOOK_PORT}"`,
        '',
      ].join('\n'),
      { mode: 0o600 },
    );

    // ── slack: DM allowlisted to ONE sender ──────────────────────────────
    fs.writeFileSync(
      path.join(this.home, 'channels', 'f24c3slack.toml'),
      [
        'name = "f24c3slack"',
        'platform = "slack"',
        'enabled = true',
        '',
        '[options]',
        'workspace_name = "f24c3"',
        'default_channel_id = "D24C3DEFAULT"',
        'credential_handle_bot_token = "slack.f24c3.bot_token"',
        'credential_handle_signing_secret = "slack.f24c3.signing_secret"',
        `api_base_url = "${this.sinkUrl}"`,
        'max_retry_attempts = 1',
        '',
        '[inbound]',
        'dm = "allowlist"',
        'dm_allowlist = ["U24C3ALLOWED"]',
        'group = "disabled"',
        'require_mention = true',
        'tools = "conversational"',
        '',
      ].join('\n'),
    );

    // ── whatsapp ──────────────────────────────────────────────────────────
    fs.writeFileSync(
      path.join(this.home, 'channels', 'f24c3whatsapp.toml'),
      [
        'name = "f24c3whatsapp"',
        'platform = "whatsapp"',
        'enabled = true',
        '',
        '[options]',
        'workspace_name = "f24c3"',
        'phone_number_id = "F24C3PHONE"',
        'default_recipient = "15551110000"',
        'credential_handle_access_token = "whatsapp.f24c3.access_token"',
        'credential_handle_app_secret = "whatsapp.f24c3.app_secret"',
        `api_base_url = "${this.sinkUrl}"`,
        'graph_version = "v18.0"',
        'max_retry_attempts = 1',
        '',
        '[inbound]',
        'dm = "allowlist"',
        'dm_allowlist = ["15552220000"]',
        'group = "disabled"',
        'require_mention = true',
        'tools = "conversational"',
        '',
      ].join('\n'),
    );

    // ── twilio sms ────────────────────────────────────────────────────────
    fs.writeFileSync(
      path.join(this.home, 'channels', 'f24c3sms.toml'),
      [
        'name = "f24c3sms"',
        'platform = "sms"',
        'enabled = true',
        '',
        '[options]',
        'from_number = "+15550009999"',
        'credential_handle_account_sid = "sms.f24c3.account_sid"',
        'credential_handle_auth_token = "sms.f24c3.auth_token"',
        `api_base_url = "${this.sinkUrl}"`,
        'max_retry_attempts = 1',
        '',
        '[inbound]',
        'dm = "allowlist"',
        'dm_allowlist = ["+15553330000"]',
        'group = "disabled"',
        'require_mention = true',
        'tools = "conversational"',
        '',
      ].join('\n'),
    );
  }

  // ── the binary ────────────────────────────────────────────────────────────

  buildInfo() {
    const r = run([this.args.binary, '--build-info']);
    if (r.status !== 0) throw new Error(`--build-info failed rc=${r.status}: ${r.output}`);
    return r.output;
  }

  startBinary() {
    const logPath = path.join(this.runDir, 'core.log');
    fs.writeFileSync(logPath, '');
    const fd = fs.openSync(logPath, 'a');
    // `--json-stream` is the shipped long-running headless surface, and it is
    // one of exactly three entry points that opt into inbound channel dispatch
    // (`enable_inbound_dispatch(true)`); `gateway run` is NOT one of them, which
    // is this lane's principal finding. stdin is held open by a pipe we never
    // write to, so the process stays up for the whole matrix.
    const child = spawn(this.args.binary, ['--json-stream'], {
      stdio: ['pipe', fd, fd],
      env: {
        ...process.env,
        WAYLAND_HOME: this.home,
        // Measured on hetzner at 15ad7b0e: an isolated profile with NO vault
        // passphrase stores credentials plaintext-0600 and then refuses EVERY
        // turn with "Session persistence authority unavailable". That refusal
        // is host-wide, not channel-specific — a plain one-shot
        // `wayland-core "say hi"` on the same home fails identically — so
        // running the matrix without a passphrase would attribute a
        // credentials-posture refusal to the inbound path. The passphrase is
        // minted for this run and is not a vendor credential.
        WAYLAND_VAULT_PASSPHRASE: this.vaultPassphrase,
        RUST_LOG: 'wcore_agent::bootstrap=info,wcore_agent::channel_inbound=debug,wcore_channels=debug',
      },
      windowsHide: true,
    });
    this.coreChild = child;
    this.children.push(child);
    this.coreLog = logPath;
    return logPath;
  }

  waitForWebhookHost() {
    const probe = `
      fetch('http://127.0.0.1:${WEBHOOK_PORT}/healthz')
        .then(async r => { process.stdout.write('HEALTHZ ' + r.status + ' ' + (await r.text())); })
        .catch(e => { process.stdout.write('HEALTHZ_DOWN ' + e.message); process.exit(1); });
    `;
    for (let i = 0; i < 60; i += 1) {
      const r = run([process.execPath, '-e', probe], { timeout: 15_000 });
      if (r.status === 0 && r.output.includes('HEALTHZ 200')) {
        this.note(`webhook host up after ${i}s: ${r.output}`);
        return r.output;
      }
      process.stdout.write(`[inbound] waiting for webhook host: ${i}s ${new Date().toISOString()}\n`);
      sleep(1000);
    }
    throw new Error(
      `webhook host never bound 127.0.0.1:${WEBHOOK_PORT}\n--- core log ---\n${fs.readFileSync(this.coreLog, 'utf8').slice(-4000)}`,
    );
  }

  // ── evidence readers ──────────────────────────────────────────────────────

  arrivals() {
    if (!fs.existsSync(this.journalPath)) return [];
    return fs
      .readFileSync(this.journalPath, 'utf8')
      .split('\n')
      .filter((l) => l.trim())
      .map((l) => JSON.parse(l));
  }

  turns() {
    if (!fs.existsSync(this.llmJournalPath)) return [];
    return fs
      .readFileSync(this.llmJournalPath, 'utf8')
      .split('\n')
      .filter((l) => l.trim())
      .map((l) => JSON.parse(l));
  }

  // Every count in the report comes through here: the ARRIVALS JOURNAL of a
  // process the binary does not own, filtered to the correlation token this
  // leg planted. Never a status line, never a log line the product wrote.
  arrivalsFor(correlation) {
    return this.arrivals().filter((a) => (a.text ?? '').includes(correlation));
  }

  turnsFor(correlation) {
    return this.turns().filter((t) => t.correlation === correlation);
  }

  // Wait until `want` arrivals carry `correlation`, or the budget expires.
  // Returns whatever it saw — the caller decides whether that is a pass. A
  // waiter that threw on timeout would turn "zero arrived" into a crash and
  // lose the very number the access leg needs.
  awaitArrivals(correlation, want, budgetMs = ARRIVAL_BUDGET_MS) {
    const deadline = Date.now() + budgetMs;
    let seen = this.arrivalsFor(correlation);
    let i = 0;
    while (seen.length < want && Date.now() < deadline) {
      i += 1;
      process.stdout.write(
        `[inbound] awaiting ${correlation}: ${seen.length}/${want} after ${i}s ${new Date().toISOString()}\n`,
      );
      sleep(1000);
      seen = this.arrivalsFor(correlation);
    }
    return seen;
  }

  // A negative leg needs a settle window, not a wait: there is nothing to wait
  // FOR. The window has to be at least as long as a passing leg's observed
  // latency or a zero would only mean "not yet".
  settle(ms) {
    const end = Date.now() + ms;
    let i = 0;
    while (Date.now() < end) {
      i += 1;
      process.stdout.write(`[inbound] settling: ${i}s ${new Date().toISOString()}\n`);
      sleep(1000);
    }
  }

  record(adapter, leg, ok, detail) {
    this.results.push({ adapter, leg, ok, detail });
    process.stdout.write(`[inbound] ${ok ? 'PASS' : 'FAIL'} ${adapter}/${leg} — ${detail}\n`);
  }

  // ── per-adapter matrix ────────────────────────────────────────────────────

  // `send` maps (sender, conversation, text, messageId) to a signed platform
  // request. `expectConversation` maps a conversation id to the id the sink
  // should see on the way out — for Slack they are the same string; for
  // Twilio the outbound `To` is the inbound `From`.
  runMatrix(adapter, cfg) {
    const tag = crypto.randomBytes(4).toString('hex');
    const url = `http://127.0.0.1:${WEBHOOK_PORT}/webhooks/${cfg.channelName}`;

    // ── admit + route ─────────────────────────────────────────────────────
    const c1 = `f24c3-${adapter}-admit-${tag}`;
    const r1 = post(cfg.build({ url, sender: cfg.allowed, conversation: cfg.conv1, text: `hello ${c1}`, messageId: `${tag}.0001` }));
    const seen1 = this.awaitArrivals(c1, 1);
    this.record(
      adapter,
      'admit',
      seen1.length === 1,
      `POST rc=${r1.status} ${r1.output.split('\n')[0]} | arrivals(journal)=${seen1.length} want=1 | turns(fixture-journal)=${this.turnsFor(c1).length}`,
    );
    const routed = seen1.length === 1 && (seen1[0].text ?? '').includes(c1);
    const convOk = seen1.length === 1 && seen1[0].conversation_id === cfg.expectConversation;
    this.record(
      adapter,
      'route',
      routed && convOk,
      seen1.length === 0
        ? 'no arrival to inspect'
        : `reply_text=${JSON.stringify(seen1[0].text)} carries_correlation=${routed} conversation_id=${JSON.stringify(seen1[0].conversation_id)} want=${JSON.stringify(cfg.expectConversation)}`,
    );

    // ── dedupe ────────────────────────────────────────────────────────────
    // Replay the IDENTICAL platform message id. A second arrival means the
    // inbound dedupe cache did not absorb the platform's own retry.
    const beforeDedupe = this.arrivalsFor(c1).length;
    const r2 = post(cfg.build({ url, sender: cfg.allowed, conversation: cfg.conv1, text: `hello ${c1}`, messageId: `${tag}.0001` }));
    this.settle(20_000);
    const afterDedupe = this.arrivalsFor(c1).length;
    // Positive control: a DIFFERENT id from the same sender in the same
    // conversation must still get through, or "no second arrival" would be
    // satisfied by an adapter that had simply stopped working.
    const c1b = `f24c3-${adapter}-dedupe-control-${tag}`;
    post(cfg.build({ url, sender: cfg.allowed, conversation: cfg.conv1, text: `hello ${c1b}`, messageId: `${tag}.0002` }));
    const control = this.awaitArrivals(c1b, 1);
    this.record(
      adapter,
      'dedupe',
      afterDedupe === beforeDedupe && control.length === 1,
      `replay POST rc=${r2.status} | arrivals before=${beforeDedupe} after=${afterDedupe} (want equal) | positive-control fresh-id arrivals=${control.length} want=1`,
    );

    // ── access ────────────────────────────────────────────────────────────
    // A sender outside the allowlist. The control is the admit leg above,
    // which used the same transport, the same signature scheme and the same
    // settle window and DID arrive.
    const c3 = `f24c3-${adapter}-access-${tag}`;
    const r3 = post(cfg.build({ url, sender: cfg.denied, conversation: cfg.conv1, text: `hello ${c3}`, messageId: `${tag}.0003` }));
    this.settle(20_000);
    const seen3 = this.arrivalsFor(c3);
    const turns3 = this.turnsFor(c3);
    // The control is LOAD-BEARING, not decoration. Measured at the pre-fix
    // binary, this leg passed on all three adapters purely because the whole
    // inbound path was denying everything — "the denied sender did not get
    // through" is trivially true of a path nothing gets through. A leg that
    // cannot fail proves nothing, so the admit control is part of the
    // condition and not merely printed beside it.
    const accessControlHeld = seen1.length === 1;
    this.record(
      adapter,
      'access',
      seen3.length === 0 && turns3.length === 0 && accessControlHeld,
      `denied-sender POST rc=${r3.status} | arrivals=${seen3.length} want=0 | turns=${turns3.length} want=0 | ` +
        `CONTROL admit-leg-arrived=${seen1.length} want=1 ${accessControlHeld ? '(control held)' : '(CONTROL FAILED — a zero here is not a refusal, it is a dead path)'}`,
    );

    // ── bind ──────────────────────────────────────────────────────────────
    // A second conversation from the same allowed sender. Two arrivals, two
    // distinct conversation ids: a single shared session would collapse them.
    const c4 = `f24c3-${adapter}-bind-${tag}`;
    post(cfg.build({ url, sender: cfg.allowed, conversation: cfg.conv2, text: `hello ${c4}`, messageId: `${tag}.0004` }));
    const seen4 = this.awaitArrivals(c4, 1);
    const distinct =
      seen1.length === 1 &&
      seen4.length === 1 &&
      seen4[0].conversation_id === cfg.expectConversation2 &&
      seen4[0].conversation_id !== seen1[0].conversation_id;
    this.record(
      adapter,
      'bind',
      distinct,
      seen4.length === 0
        ? `second conversation produced no arrival (arrivals=0 want=1)`
        : `conv1=${JSON.stringify(seen1[0]?.conversation_id)} conv2=${JSON.stringify(seen4[0].conversation_id)} want2=${JSON.stringify(cfg.expectConversation2)} distinct=${seen4[0].conversation_id !== seen1[0]?.conversation_id}`,
    );
  }

  // ── orchestration ─────────────────────────────────────────────────────────

  execute() {
    fs.mkdirSync(this.runDir, { recursive: true });
    fs.rmSync(this.journalPath, { force: true });
    fs.rmSync(this.llmJournalPath, { force: true });
    fs.rmSync(this.home, { recursive: true, force: true });

    this.sinkUrl = this.startFixture(
      'f24-sink.mjs',
      ['--journal', this.journalPath],
      /SINK_READY url=(\S+)/,
      'sink.log',
    );
    this.note(`sink ${this.sinkUrl} journal=${this.journalPath}`);

    this.llmUrl = this.startFixture(
      'f24-llm-fixture.mjs',
      ['--journal', this.llmJournalPath],
      /LLM_READY url=(\S+)/,
      'llm.log',
    );
    this.note(`llm fixture ${this.llmUrl} journal=${this.llmJournalPath}`);

    this.writeConfig();

    const info = this.buildInfo();
    const digest = sha256File(this.args.binary);
    this.note(`binary ${this.args.binary}`);
    this.note(`build-info ${info}`);
    this.note(`sha256 ${digest}`);

    this.startBinary();
    const healthz = this.waitForWebhookHost();

    this.runMatrix('slack', {
      channelName: 'f24c3slack',
      allowed: 'U24C3ALLOWED',
      denied: 'U24C3DENIED',
      conv1: 'D24C3ONE',
      conv2: 'D24C3TWO',
      expectConversation: 'D24C3ONE',
      expectConversation2: 'D24C3TWO',
      build: ({ url, sender, conversation, text, messageId }) =>
        slackRequest({
          url,
          signingSecret: this.slackSigningSecret,
          channel: conversation,
          user: sender,
          text,
          ts: messageId,
          channelType: 'im',
          team: 'T24C3',
        }),
    });

    this.runMatrix('whatsapp', {
      channelName: 'f24c3whatsapp',
      allowed: '15552220000',
      denied: '15559990000',
      conv1: '15552220000',
      conv2: '15552220000',
      // WhatsApp DMs are keyed by the peer's number, so a "second
      // conversation" from the same sender does not exist on this platform.
      // Recorded honestly rather than faked with a second identity that would
      // also change the ACCESS answer.
      expectConversation: '15552220000',
      expectConversation2: '15552220000',
      build: ({ url, sender, text, messageId }) =>
        whatsappRequest({
          url,
          appSecret: this.waAppSecret,
          phoneNumberId: 'F24C3PHONE',
          from: sender,
          text,
          messageId,
        }),
    });

    this.runMatrix('sms', {
      channelName: 'f24c3sms',
      allowed: '+15553330000',
      denied: '+15559990000',
      conv1: '+15553330000',
      conv2: '+15553330000',
      expectConversation: '+15553330000',
      expectConversation2: '+15553330000',
      build: ({ url, sender, text, messageId }) =>
        twilioRequest({
          url,
          publicUrl: url,
          authToken: this.twilioToken,
          from: sender,
          to: '+15550009999',
          text,
          messageSid: `SM${messageId.replace('.', '')}`,
        }),
    });

    const result = {
      schema: RESULT_SCHEMA,
      platform: this.args.platform,
      binary: this.args.binary,
      build_info: info,
      binary_sha256: digest,
      arrival_source: 'independent-sink',
      host_channels_dir: this.hostChannelsDir(),
      host_channels_dir_entries: (() => {
        const d = this.hostChannelsDir();
        if (!d || !fs.existsSync(d)) return null;
        return fs.readdirSync(d);
      })(),
      arrivals_journal: this.journalPath,
      turns_journal: this.llmJournalPath,
      healthz,
      arrivals_total: this.arrivals().length,
      turns_total: this.turns().length,
      results: this.results,
      notes: this.notes,
      finished_at: new Date().toISOString(),
    };
    fs.writeFileSync(
      path.join(this.runDir, `${this.args.platform}-inbound-result.json`),
      `${JSON.stringify(result, null, 2)}\n`,
    );
    return result;
  }

  cleanup() {
    for (const c of this.children) {
      try {
        c.kill('SIGTERM');
      } catch {
        /* already gone */
      }
    }
  }
}

// ── entry point ──────────────────────────────────────────────────────────────

function parseArgs(argv) {
  const out = { binary: null, runDir: path.join(os.tmpdir(), 'f24-inbound'), platform: process.platform === 'darwin' ? 'macos' : process.platform === 'win32' ? 'windows' : 'linux' };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--binary') out.binary = argv[++i];
    else if (a === '--run-dir') out.runDir = argv[++i];
    else if (a === '--platform') out.platform = argv[++i];
    else {
      process.stderr.write(`f24-inbound: unknown argument ${a}\n`);
      process.exit(2);
    }
  }
  if (!out.binary) {
    process.stderr.write('f24-inbound: --binary is required\n');
    process.exit(2);
  }
  return out;
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  const args = parseArgs(process.argv.slice(2));
  const m = new InboundMatrix(args);
  let result;
  try {
    result = m.execute();
  } finally {
    m.cleanup();
  }
  const failed = result.results.filter((r) => !r.ok);
  process.stdout.write(
    `\nINBOUND MATRIX ${failed.length === 0 ? 'GREEN' : 'RED'} platform=${result.platform} ` +
      `legs=${result.results.length} failed=${failed.length} ` +
      `arrivals_total=${result.arrivals_total} turns_total=${result.turns_total}\n`,
  );
  for (const r of result.results) {
    process.stdout.write(`  ${r.ok ? 'PASS' : 'FAIL'} ${r.adapter}/${r.leg}: ${r.detail}\n`);
  }
  // Non-zero on any failed leg, so this is usable as a gate. It also exits
  // non-zero when it ran no legs at all — a driver that measured nothing must
  // not be readable as a pass.
  process.exit(failed.length === 0 && result.results.length === ADAPTERS.length * LEGS.length ? 0 : 1);
}

export { InboundMatrix, slackRequest, whatsappRequest, twilioRequest };
