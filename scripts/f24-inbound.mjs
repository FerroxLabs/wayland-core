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

import { partition as correlate, instrumentFault } from './f24-correlate.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));

export const RESULT_SCHEMA = 'wayland.inbound.matrix/1';

// The adapters this driver can point at a fixture, and the inbound transport
// each one uses. Kept as data so the report can name what it did NOT measure
// with the same authority as what it did.
//
// `telegram` is the FIRST polling adapter in this matrix, and adding it is the
// point of the 24-C3-TG-EMAIL lane. Everything measured before it received by
// WEBHOOK — the platform POSTs, the binary's inbound webhook host answers. A
// polling adapter inverts that: the binary reaches OUT and, critically,
// CONSUMES WHAT IT READS. `getUpdates?offset=N` permanently destroys every
// update below N. That destructive read is the mechanism behind F24-C3-H4's
// steady-state loss of 5 of 6 messages, and no webhook leg can exercise it.
export const ADAPTERS = ['slack', 'whatsapp', 'sms', 'telegram'];

// How each adapter's inbound messages reach the binary. A `webhook` adapter
// needs the inbound webhook host to be bound; a `poll` adapter does not, and
// conflating the two is what made the old `failEveryLeg` over-report.
export const TRANSPORT = {
  slack: 'webhook',
  whatsapp: 'webhook',
  sms: 'webhook',
  telegram: 'poll',
};

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
    // The polling adapter's arrivals land in the TELEGRAM FIXTURE's own
    // journal, not the shared sink: a Telegram reply leaves through
    // `sendMessage` on the Bot API base, which is the fixture. That preserves
    // the load-bearing property unchanged — the journal belongs to a process
    // the binary does not own and can only write to by completing a real TCP
    // round trip.
    this.tgJournalPath = path.join(this.runDir, 'telegram.jsonl');
    this.children = [];
    this.results = [];
    this.notes = [];
    // Records that carry a correlation token in a form this driver cannot
    // decode. Never counted as arrivals, never counted as losses — see
    // `f24-correlate.mjs`.
    this.faults = [];
    this.faultKeys = new Set();

    // Every secret below is minted here and exists only for this run.
    this.slackBotToken = `xoxb-f24c3-${crypto.randomBytes(12).toString('hex')}`;
    this.slackSigningSecret = crypto.randomBytes(24).toString('hex');
    this.waToken = `EAAf24c3${crypto.randomBytes(12).toString('hex')}`;
    this.waAppSecret = crypto.randomBytes(24).toString('hex');
    this.twilioSid = `ACf24c3${crypto.randomBytes(12).toString('hex')}`;
    this.twilioToken = crypto.randomBytes(24).toString('hex');
    // Minted here exactly like the other four. A Telegram bot token's real
    // shape is `<bot_id>:<secret>`; the colon is reproduced because it is part
    // of the URL path the adapter builds (`/bot<token>/getUpdates`) and a
    // token without one would exercise a path shape no real token takes.
    this.tgBotToken = `70${crypto.randomInt(100000, 999999)}:AA${crypto.randomBytes(17).toString('base64url')}`;
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
        `"telegram.f24c3.bot_token" = "${this.tgBotToken}"`,
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
        // TWO allowed peers. On a peer-keyed platform the "second
        // conversation" of the bind leg is a second person, not a second room,
        // so the allowlist has to admit both or the leg would be measuring the
        // access gate again.
        'dm_allowlist = ["15552220000", "15552221111"]',
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
        'dm_allowlist = ["+15553330000", "+15553331111"]',
        'group = "disabled"',
        'require_mention = true',
        'tools = "conversational"',
        '',
      ].join('\n'),
    );

    // ── telegram (POLLING) ────────────────────────────────────────────────
    //
    // `api_base_url` is the whole reason this adapter can be measured without a
    // vendor credential: `TelegramChannel::new`
    // (crates/wcore-channel-telegram/src/lib.rs:68-75) reads it into `api_base`,
    // so the SHIPPED constructor the registry calls — not a `#[doc(hidden)]`
    // test-only one — points at the fixture.
    //
    // `allowed_chat_ids` is left EMPTY on purpose. Telegram carries a second,
    // adapter-local gate at the long-poll layer
    // (longpoll.rs:267) that the other three adapters do not have. Leaving it
    // empty means the `access` leg measures the SAME mechanism the other three
    // are measured on — `[inbound] dm_allowlist`, compared against
    // `msg.sender_id` in dispatch/access.rs:223 — so a green here means the
    // shared access path admits and refuses correctly on a polling transport,
    // not merely that a Telegram-only pre-filter worked. The adapter-local gate
    // is a separate question and is NOT claimed by this run.
    //
    // `long_poll_timeout_secs = 1` keeps the fixture's long-poll short so the
    // matrix's own settle windows stay meaningful; the fixture additionally
    // caps its wait via `--max-wait-ms`.
    fs.writeFileSync(
      path.join(this.home, 'channels', 'f24c3telegram.toml'),
      [
        'name = "f24c3telegram"',
        'platform = "telegram"',
        'enabled = true',
        '',
        '[options]',
        'credential_handle = "telegram.f24c3.bot_token"',
        `api_base_url = "${this.tgUrl}"`,
        'long_poll_timeout_secs = 1',
        'allowed_chat_ids = []',
        '',
        '[inbound]',
        'dm = "allowlist"',
        // Telegram private chats are keyed by the peer: `chat.id` equals the
        // sender's user id for a DM. The bind leg's second conversation is
        // therefore a second person, exactly as on WhatsApp and SMS.
        'dm_allowlist = ["24030001", "24030002"]',
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
    // `--json-stream` is the shipped long-running headless surface, and it was
    // one of exactly three entry points that opted into inbound channel
    // dispatch (`enable_inbound_dispatch(true)`); `gateway run` was NOT one of
    // them, which was 24-C3's principal open finding (F24-C3-H2).
    //
    // `gateway run` is the persistent runtime an operator installs. It is run
    // in the FOREGROUND here (no `--detach`): a detached gateway re-execs and
    // this driver would lose the child it must reap, and the run would then
    // measure a process it does not own. stdin is held open by a pipe we never
    // write to, so either surface stays up for the whole matrix.
    const argv = this.args.runtime === 'gateway' ? ['gateway', 'run'] : ['--json-stream'];
    this.note(`runtime=${this.args.runtime} argv=${JSON.stringify(argv)}`);
    const child = spawn(this.args.binary, argv, {
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
    // F24-C3-H2. Returning null rather than throwing. A throw produced no
    // result JSON and no leg table, so the single most important measurement
    // this driver can make — "the runtime bound NOTHING" — was the one outcome
    // it could not record. The caller turns this into 15 explicit FAILs with
    // the reason, which is both faster than waiting out 15 arrival budgets
    // against a dead socket and far better evidence than a stack trace.
    this.note(
      `webhook host never bound 127.0.0.1:${WEBHOOK_PORT} after 60s; ` +
        `runtime=${this.args.runtime} hosts no inbound listener`,
    );
    this.coreLogTail = fs.readFileSync(this.coreLog, 'utf8').slice(-4000);
    return null;
  }

  /// Every WEBHOOK leg, failed, with the one reason that caused all of them.
  /// Used when the runtime bound no inbound listener: each leg is reported
  /// individually so the count in the banner is the real leg count and not a
  /// zero that a reader could mistake for "nothing ran".
  ///
  /// Scoped to the webhook adapters on purpose. Before telegram joined the
  /// matrix every adapter was a webhook adapter, so failing "every leg" was the
  /// same set. It is not any more, and failing the polling legs for a missing
  /// webhook host would destroy the single most interesting measurement this
  /// driver can now make: whether a runtime that binds NO inbound listener
  /// (`gateway run`, per F24-C3-H2) nonetheless receives on a POLLING adapter.
  /// Those are independent paths, and collapsing them would answer a question
  /// nobody asked while burying the one that matters.
  failWebhookLegs(reason) {
    for (const adapter of ADAPTERS) {
      if (TRANSPORT[adapter] !== 'webhook') continue;
      for (const leg of LEGS) {
        this.record(adapter, leg, false, reason);
      }
    }
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

  // The telegram fixture's OWN journal, projected into the same
  // `{text, conversation_id}` shape the sink emits, so one set of leg logic
  // serves both transports. A Telegram reply is a `sendMessage` call; its
  // `chat_id` is the conversation.
  tgArrivals() {
    if (!fs.existsSync(this.tgJournalPath)) return [];
    return fs
      .readFileSync(this.tgJournalPath, 'utf8')
      .split('\n')
      .filter((l) => l.trim())
      .map((l) => {
        try {
          return JSON.parse(l);
        } catch {
          return null;
        }
      })
      .filter((r) => r && r.kind === 'sendMessage')
      .map((r) => ({ text: r.text, conversation_id: String(r.chat_id) }));
  }

  /// Which journal an adapter's replies land in.
  readerFor(adapter) {
    return TRANSPORT[adapter] === 'poll' ? () => this.tgArrivals() : () => this.arrivals();
  }

  // Every count in the report comes through here: the ARRIVALS JOURNAL of a
  // process the binary does not own, filtered to the correlation token this
  // leg planted. Never a status line, never a log line the product wrote.
  //
  // The filter is `f24-correlate.mjs`, NOT `String.includes`. The exact
  // substring test this replaced is wrong for any adapter that transforms
  // outbound text, and Telegram does: `parse_mode` defaults to MarkdownV2 and
  // the adapter escapes the full body, so `f24c3-telegram-admit-9f3a1c02`
  // reaches the journal as `f24c3\-telegram\-admit\-9f3a1c02`. An exact matcher
  // scores a perfectly delivered reply ZERO — which is how a lane on this
  // program came to report `replied=0` for eight replies that had all arrived.
  //
  // Anything carrying the token in a form the matcher cannot decode is recorded
  // as an INSTRUMENT FAULT and is counted as neither an arrival nor a loss.
  arrivalsFor(correlation, reader) {
    const records = (reader ?? (() => this.arrivals()))();
    const { arrivals, faults } = correlate(records, correlation);
    for (const f of faults) {
      const key = `${correlation} ${f.text}`;
      if (this.faultKeys.has(key)) continue;
      this.faultKeys.add(key);
      this.faults.push({
        correlation,
        conversation_id: f.conversation_id ?? null,
        text: f.text,
        why: 'token present in an encoding this driver does not model',
      });
      process.stdout.write(
        `[inbound] INSTRUMENT FAULT ${correlation}: journal record carries the token in an ` +
          `undecodable form — run is INCOMPLETE, not a loss. text=${JSON.stringify(f.text)}\n`,
      );
    }
    return arrivals;
  }

  turnsFor(correlation) {
    // The turns journal records the correlation the fixture extracted from the
    // INBOUND text, which no adapter escapes, so an exact compare is correct
    // here. The fault detector still runs so an unexpected transform on the
    // inbound side cannot pass silently either.
    const all = this.turns();
    const exact = all.filter((t) => t.correlation === correlation);
    if (exact.length === 0) {
      for (const t of all) {
        if (t.correlation && instrumentFault(t.correlation, correlation)) {
          const key = `turn ${correlation} ${t.correlation}`;
          if (this.faultKeys.has(key)) continue;
          this.faultKeys.add(key);
          this.faults.push({
            correlation,
            conversation_id: null,
            text: t.correlation,
            why: 'turn journal correlation is a mangled form of the planted token',
          });
        }
      }
    }
    return exact;
  }

  // Wait until `want` arrivals carry `correlation`, or the budget expires.
  // Returns whatever it saw — the caller decides whether that is a pass. A
  // waiter that threw on timeout would turn "zero arrived" into a crash and
  // lose the very number the access leg needs.
  awaitArrivals(correlation, want, reader, budgetMs = ARRIVAL_BUDGET_MS) {
    const deadline = Date.now() + budgetMs;
    let seen = this.arrivalsFor(correlation, reader);
    let i = 0;
    while (seen.length < want && Date.now() < deadline) {
      i += 1;
      process.stdout.write(
        `[inbound] awaiting ${correlation}: ${seen.length}/${want} after ${i}s ${new Date().toISOString()}\n`,
      );
      sleep(1000);
      seen = this.arrivalsFor(correlation, reader);
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

  // `build` maps (sender, conversation, text, messageId) to a signed platform
  // request for a WEBHOOK adapter. `expectConversation` maps a conversation id
  // to the id the sink should see on the way out — for Slack they are the same
  // string; for Twilio the outbound `To` is the inbound `From`.
  //
  // A POLLING adapter has no request to sign and no endpoint to POST to: the
  // platform does not push, the binary pulls. Such an adapter supplies `inject`
  // instead of `build`, which hands the message to the fixture's own control
  // plane and lets the binary come and get it. Everything downstream of
  // injection — the five legs, the falsifiers, the controls — is identical, and
  // that is deliberate: a polling adapter measured by a different yardstick
  // could not be compared against the webhook three.
  runMatrix(adapter, cfg) {
    const tag = crypto.randomBytes(4).toString('hex');
    const url = `http://127.0.0.1:${WEBHOOK_PORT}/webhooks/${cfg.channelName}`;
    const reader = this.readerFor(adapter);
    const deliver = ({ sender, conversation, text, messageId }) =>
      cfg.inject
        ? cfg.inject({ sender, conversation, text, messageId })
        : post(cfg.build({ url, sender, conversation, text, messageId }));

    this.note(`matrix ${adapter} transport=${TRANSPORT[adapter]} tag=${tag}`);

    // ── admit + route ─────────────────────────────────────────────────────
    const c1 = `f24c3-${adapter}-admit-${tag}`;
    const r1 = deliver({ sender: cfg.allowed, conversation: cfg.conv1, text: `hello ${c1}`, messageId: `${tag}.0001` });
    const seen1 = this.awaitArrivals(c1, 1, reader);
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
    const beforeDedupe = this.arrivalsFor(c1, reader).length;
    const r2 = deliver({ sender: cfg.allowed, conversation: cfg.conv1, text: `hello ${c1}`, messageId: `${tag}.0001` });
    this.settle(20_000);
    const afterDedupe = this.arrivalsFor(c1, reader).length;
    // Positive control: a DIFFERENT id from the same sender in the same
    // conversation must still get through, or "no second arrival" would be
    // satisfied by an adapter that had simply stopped working.
    const c1b = `f24c3-${adapter}-dedupe-control-${tag}`;
    deliver({ sender: cfg.allowed, conversation: cfg.conv1, text: `hello ${c1b}`, messageId: `${tag}.0002` });
    const control = this.awaitArrivals(c1b, 1, reader);
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
    const r3 = deliver({ sender: cfg.denied, conversation: cfg.convDenied ?? cfg.conv1, text: `hello ${c3}`, messageId: `${tag}.0003` });
    this.settle(20_000);
    const seen3 = this.arrivalsFor(c3, reader);
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
    deliver({
      sender: cfg.secondSender ?? cfg.allowed,
      conversation: cfg.conv2,
      text: `hello ${c4}`,
      messageId: `${tag}.0004`,
    });
    const seen4 = this.awaitArrivals(c4, 1, reader);
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
    fs.rmSync(this.tgJournalPath, { force: true });
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

    // The Telegram Bot API fixture. Started BEFORE the config is written
    // because the config has to name its bound port, and before the binary
    // because `TelegramChannel::start()` calls `deleteWebhook` immediately.
    this.tgUrl = this.startFixture(
      'f24-tg-fixture.mjs',
      ['--journal', this.tgJournalPath, '--token', this.tgBotToken, '--max-wait-ms', '1500'],
      /TGFIX_READY url=(\S+)/,
      'telegram.log',
    );
    this.note(`telegram fixture ${this.tgUrl} journal=${this.tgJournalPath}`);

    this.writeConfig();

    const info = this.buildInfo();
    const digest = sha256File(this.args.binary);
    this.note(`binary ${this.args.binary}`);
    this.note(`build-info ${info}`);
    this.note(`sha256 ${digest}`);

    this.startBinary();
    const healthz = this.waitForWebhookHost();

    // The runtime bound no inbound listener. Do NOT run the WEBHOOK legs: every
    // POST would go to a closed port and every leg would fail after its full
    // 90s arrival budget, reporting separate mysteries instead of the one fact
    // that explains all of them.
    //
    // The POLLING legs still run. They reach the binary by a path that never
    // touches the webhook host, so a missing host says nothing about them —
    // and "does a runtime with no webhook host still receive by polling?" is
    // exactly the question F24-C3-H2 left open.
    if (healthz === null) {
      this.failWebhookLegs(
        `no inbound listener on 127.0.0.1:${WEBHOOK_PORT} — runtime=${this.args.runtime} ` +
          `hosts no inbound webhook host`,
      );
      this.runTelegramMatrix();
      return this.finish(info, digest, null);
    }

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
      // WhatsApp DMs are keyed by the PEER, so a second conversation is a
      // second person. `sender` and `conversation` are therefore the same
      // string on this platform, and the bind leg's second identity is a
      // second allowlisted number rather than a second room.
      conv1: '15552220000',
      conv2: '15552221111',
      secondSender: '15552221111',
      expectConversation: '15552220000',
      expectConversation2: '15552221111',
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
      // Same peer-keyed shape as WhatsApp. Before F24-C3-H3 the outbound
      // `To` here was the BOT's number for every sender, which is what this
      // leg caught.
      conv1: '+15553330000',
      conv2: '+15553331111',
      secondSender: '+15553331111',
      expectConversation: '+15553330000',
      expectConversation2: '+15553331111',
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

    this.runTelegramMatrix();

    return this.finish(info, digest, healthz);
  }

  /// The polling adapter's matrix. Split out of `execute` so it can be reached
  /// from BOTH the healthy path and the no-webhook-host path without
  /// duplicating the descriptor.
  runTelegramMatrix() {
    this.runMatrix('telegram', {
      channelName: 'f24c3telegram',
      // Telegram private chats are peer-keyed: `chat.id == from.id` for a DM,
      // and `longpoll.rs:364` binds `conversation_id` to `chat.id`. So sender
      // and conversation are the same number, exactly as on WhatsApp and SMS.
      allowed: '24030001',
      denied: '24039999',
      conv1: '24030001',
      conv2: '24030002',
      convDenied: '24039999',
      secondSender: '24030002',
      expectConversation: '24030001',
      expectConversation2: '24030002',
      inject: ({ sender, conversation, text, messageId }) =>
        this.tgSubmit({ sender, conversation, text, messageId }),
    });
  }

  /// Hand a message to the Telegram fixture's queue. The binary then has to
  /// come and get it with `getUpdates`, destroying it as it confirms — which is
  /// the mechanism a webhook leg cannot exercise at all.
  ///
  /// `messageId` is passed through as the Telegram `message_id` so the dedupe
  /// leg can replay the SAME platform message under a fresh `update_id`. That
  /// is the real shape of a Telegram redelivery: `update_id` is the transport's
  /// cursor and always advances, while `message_id` identifies the message —
  /// and `message_id` is what the dedupe cache keys on.
  tgSubmit({ sender, conversation, text, messageId }) {
    // The matrix's ids are `<tag>.NNNN`; Telegram message ids are integers.
    // Map deterministically so a replay of `${tag}.0001` produces the SAME
    // integer and therefore genuinely tests dedupe rather than sending a new
    // message that happens to carry the same text.
    const numericId = Number.parseInt(String(messageId).split('.').pop(), 10);
    const payload = JSON.stringify({
      token: this.tgBotToken,
      chatId: String(conversation),
      senderId: String(sender),
      username: `u${sender}`,
      text,
      messageId: numericId,
    });
    const script = `
      fetch(${JSON.stringify(`${this.tgUrl}/__control/submit`)}, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: ${JSON.stringify(payload)},
      })
        .then(async (res) => {
          const t = await res.text();
          process.stdout.write('SUBMIT ' + res.status + ' ' + t);
        })
        .catch((e) => { process.stdout.write('SUBMIT FAILED ' + e.message); process.exit(1); });
    `;
    return run([process.execPath, '-e', script], { timeout: 30_000 });
  }

  /// The Telegram fixture's independent report: how many updates it was given,
  /// which poll consumed each one, and the maximum number of `getUpdates`
  /// requests it ever had open at once.
  ///
  /// `max_concurrent_getupdates` is the F24-C3-H4 observable and it is counted
  /// in ANOTHER PROCESS from overlapping open requests — it is not a log line
  /// the binary prints about itself. 2 means two managers are competing for one
  /// token (and whatever the unsubscribed one wins is destroyed on read); 1 is
  /// correct; 0 means the runtime polled NOTHING, which is a distinct failing
  /// answer, so a "fix" that works by starting nothing cannot pass here.
  tgReport() {
    if (!this.tgUrl) return null;
    const script = `
      fetch(${JSON.stringify(`${this.tgUrl}/__control/report`)})
        .then(async (r) => process.stdout.write(await r.text()))
        .catch((e) => { process.stdout.write(JSON.stringify({ ok: false, error: e.message })); });
    `;
    const r = run([process.execPath, '-e', script], { timeout: 30_000 });
    try {
      return JSON.parse(r.output);
    } catch {
      return { ok: false, raw: r.output, rc: r.status };
    }
  }

  /// Assemble, persist and return the result document. Split out of `execute`
  /// so the "runtime bound no listener" path produces the SAME document shape
  /// as a full run — a result a reader has to interpret differently depending
  /// on how the run ended is a result that gets misread.
  finish(info, digest, healthz) {
    // Read the fixture's report BEFORE the result document is assembled, so a
    // fixture that has already been reaped shows up as an explicit failure to
    // read rather than as a silently absent field.
    const tgReport = this.tgReport();
    const result = {
      schema: RESULT_SCHEMA,
      platform: this.args.platform,
      // Which runtime surface hosted this matrix. Written into the result JSON
      // and echoed in the banner so a saved result can never be mistaken for
      // the other surface's.
      runtime: this.args.runtime,
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
      // The observable that F24-C3-H2 turns on, recorded as its own field so a
      // reader never has to infer it from a leg detail string. False here means
      // the runtime under test binds no inbound webhook host at all.
      webhook_host_bound: healthz !== null,
      core_log_tail: this.coreLogTail ?? null,
      arrivals_total: this.arrivals().length,
      turns_total: this.turns().length,
      // Byte counts, because a journal that exists and is EMPTY is a different
      // fact from a journal that was never created, and both read as "0
      // arrivals" if you only count parsed records.
      journal_bytes: {
        arrivals: fs.existsSync(this.journalPath) ? fs.statSync(this.journalPath).size : null,
        turns: fs.existsSync(this.llmJournalPath) ? fs.statSync(this.llmJournalPath).size : null,
        telegram: fs.existsSync(this.tgJournalPath) ? fs.statSync(this.tgJournalPath).size : null,
        core_log: this.coreLog && fs.existsSync(this.coreLog) ? fs.statSync(this.coreLog).size : null,
      },
      telegram_journal: this.tgJournalPath,
      telegram_arrivals_total: this.tgArrivals().length,
      // The independent, out-of-process observable for F24-C3-H4.
      telegram_fixture_report: tgReport,
      // INCOMPLETE, not LOSS. See `f24-correlate.mjs`: a non-empty list here
      // means at least one journal record carried a planted token in a form
      // this driver cannot decode, so its numbers are not trustworthy in either
      // direction and the run must not be read as a clean result.
      instrument_fault: this.faults.length > 0,
      instrument_faults: this.faults,
      results: this.results,
      notes: this.notes,
      finished_at: new Date().toISOString(),
    };
    fs.writeFileSync(
      path.join(this.runDir, `${this.args.platform}-${this.args.runtime}-inbound-result.json`),
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
  const out = { binary: null, runDir: path.join(os.tmpdir(), 'f24-inbound'), platform: process.platform === 'darwin' ? 'macos' : process.platform === 'win32' ? 'windows' : 'linux', runtime: 'json-stream' };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--binary') out.binary = argv[++i];
    else if (a === '--run-dir') out.runDir = argv[++i];
    else if (a === '--platform') out.platform = argv[++i];
    // F24-C3-H2. Which RUNTIME SURFACE hosts the matrix. `json-stream` is the
    // headless host surface 24-C3 measured; `gateway` is the persistent runtime
    // an operator installs as a systemd unit / launchd plist / scheduled task.
    //
    // The two are not interchangeable and that is the whole point: at
    // `e88cf43f` the SAME binary scores 15/15 GREEN under `json-stream` and 0
    // arrivals under `gateway`, because `run_gateway` constructed no
    // InboundSubscriber and no webhook host. Running the identical driver,
    // fixtures and legs across the switch isolates the defect to the runtime
    // surface rather than to the binary, the config or the instrument.
    else if (a === '--runtime') out.runtime = argv[++i];
    else {
      process.stderr.write(`f24-inbound: unknown argument ${a}\n`);
      process.exit(2);
    }
  }
  if (!out.binary) {
    process.stderr.write('f24-inbound: --binary is required\n');
    process.exit(2);
  }
  // An unknown --runtime must NOT silently fall back to json-stream: a typo
  // would then measure the surface that already worked and report it as the
  // gateway's result.
  if (!['json-stream', 'gateway'].includes(out.runtime)) {
    process.stderr.write(`f24-inbound: --runtime must be json-stream|gateway, got ${out.runtime}\n`);
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
  const expectedLegs = ADAPTERS.length * LEGS.length;
  const ranEverything = result.results.length === expectedLegs;
  // Three outcomes, not two. INCOMPLETE is what an instrument fault produces:
  // a token reached the journal in a form this driver cannot decode, so the
  // numbers are untrustworthy in BOTH directions and the run must not be read
  // as either a clean green or an honest red.
  const verdict = result.instrument_fault
    ? 'INCOMPLETE'
    : failed.length === 0 && ranEverything
      ? 'GREEN'
      : 'RED';
  process.stdout.write(
    `\nINBOUND MATRIX ${verdict} platform=${result.platform} ` +
      `runtime=${result.runtime} ` +
      `legs=${result.results.length}/${expectedLegs} failed=${failed.length} ` +
      `arrivals_total=${result.arrivals_total} telegram_arrivals=${result.telegram_arrivals_total} ` +
      `turns_total=${result.turns_total} instrument_fault=${result.instrument_fault}\n`,
  );
  if (result.telegram_fixture_report && result.telegram_fixture_report.ok) {
    const t = result.telegram_fixture_report;
    process.stdout.write(
      `  telegram fixture: submitted=${t.submitted_total} still_pending=${JSON.stringify(t.still_pending)} ` +
        `polls=${t.poll_total} max_concurrent_getupdates=${t.max_concurrent_getupdates}\n`,
    );
  }
  for (const r of result.results) {
    process.stdout.write(`  ${r.ok ? 'PASS' : 'FAIL'} ${r.adapter}/${r.leg}: ${r.detail}\n`);
  }
  for (const f of result.instrument_faults ?? []) {
    process.stdout.write(`  INSTRUMENT-FAULT ${f.correlation}: ${f.why} — ${JSON.stringify(f.text)}\n`);
  }
  // Distinct exit codes so a caller can tell the three apart. Exit status alone
  // was never enough here anyway (see LANE-BRIEF §3.2), but collapsing
  // INCOMPLETE into RED would let a harness defect be filed as a product
  // regression — which is precisely the failure this state exists to prevent.
  //   0 = GREEN, 1 = RED, 3 = INCOMPLETE (instrument fault)
  process.exit(verdict === 'GREEN' ? 0 : verdict === 'INCOMPLETE' ? 3 : 1);
}

export { InboundMatrix, slackRequest, whatsappRequest, twilioRequest };
