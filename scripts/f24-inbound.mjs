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

import {
  partition as correlate,
  instrumentFault,
  matches as correlationMatches,
} from './f24-correlate.mjs';

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
// `email` is the SECOND polling adapter and the one that had never been driven
// at all. It carries the same destructive-read hazard in two forms: a real IMAP
// server sets `\Seen` on a non-PEEK `FETCH ... RFC822`, and `imap.rs` advances a
// UID watermark persisted OUTSIDE the session and keyed by
// (host, user, mailbox), after which it only ever searches above it.
// `matrix` and `signal` join here in the 24-MATRIX-SIGNAL lane. 24-C3-FINISH
// §4b costed both as needing ZERO Rust, from the SHIPPED construction path
// (`registry::make_*` -> the adapter's `new()`), not from a `#[doc(hidden)]`
// test constructor:
//
//   matrix  `MatrixConfig.homeserver_url` (config.rs:9) is required, has no
//           `#[serde(default)]` and no production constant; `new()` copies it
//           into `api_base` (lib.rs:61). Inbound is an HTTP long-poll on
//           `/sync` with a `since` cursor — the THIRD polling transport here,
//           and the first NON-destructive one: a Matrix `/sync` does not
//           consume what it reads, unlike `getUpdates` and IMAP `FETCH`.
//   signal  `SignalConfig.signal_cli_path` (config.rs:18) reaches
//           `Command::new(cli_path).arg("-a").arg(account).arg("jsonRpc")`
//           (subprocess.rs:54) through `new()` (lib.rs:82). This is a
//           SUBPROCESS-PATH seam, not a base-URL one — every other adapter in
//           this matrix is fixtured by redirecting an HTTP base URL, so anyone
//           grepping the never-driven adapters for `*_base_url` finds nothing
//           in signal and concludes it has no seam. It has the cheapest one
//           here: an executable, no HTTP, no TLS, no port, no certificate.
export const ADAPTERS = ['slack', 'whatsapp', 'sms', 'telegram', 'email', 'matrix', 'signal'];

// How each adapter's inbound messages reach the binary. A `webhook` adapter
// needs the inbound webhook host to be bound; a `poll` adapter does not, and
// conflating the two is what made the old `failEveryLeg` over-report.
//
// `subprocess` is a third value rather than a second spelling of `poll`: the
// binary does not reach out over the network for it at all, it SPAWNS the peer
// and owns its stdio. Collapsing it into `poll` would make the transport column
// of the report claim something untrue about how the message travelled.
export const TRANSPORT = {
  slack: 'webhook',
  whatsapp: 'webhook',
  sms: 'webhook',
  telegram: 'poll',
  email: 'poll',
  matrix: 'poll',
  signal: 'subprocess',
};

// `steady` joins the five in the 24-MATRIX-SIGNAL lane, and it is the leg the
// other five cannot cover.
//
// Every one of `admit`/`dedupe`/`access`/`bind`/`route` fires inside the first
// seconds of a channel's life, in a continuous burst. F24-C3-H4 was raised from
// MEDIUM to HIGH precisely because a STEADY-STATE run — messages arriving after
// a quiet period, the way real traffic does — lost 5 of 6, while the startup
// burst looked healthy. A startup-only matrix cannot see that class at all: a
// poller that dies, desynchronises its cursor, or loses its subscriber after
// its first successful exchange passes all five legs above and then silently
// drops everything afterwards.
export const LEGS = ['admit', 'dedupe', 'access', 'bind', 'route', 'steady'];

// The steady-state leg's shape. `QUIET_MS` must be comfortably longer than any
// adapter's poll interval so the channel genuinely goes idle rather than merely
// pausing mid-cycle; `COUNT` messages then follow at `GAP_MS` so a single
// swallowed message is visible as a count, not as a boolean.
const STEADY_QUIET_MS = 30_000;
const STEADY_COUNT = 3;
const STEADY_GAP_MS = 4_000;

// ── matrix identities ───────────────────────────────────────────────────────
// Matrix is ROOM-keyed, like Slack and unlike telegram/whatsapp/sms:
// `sync.rs:355` binds `sender_id` to the event's mxid while `:363` binds
// `conversation_id` to the ROOM id. So the bind leg's second conversation is a
// second ROOM with the SAME sender, not a second person.
//
// Both rooms are declared to the fixture with TWO joined members, because
// `sync.rs:328-331` types a room `Direct` only on `m.joined_member_count == 2`
// and types an omitted summary `Group` — and every channel config in this
// matrix sets `group = "disabled"`. A fixture that omitted the summary would
// have every message dropped by group policy, and the run would read as product
// inbound loss that was entirely the fixture's doing.
export const MX = {
  server: 'f24.invalid',
  bot: '@f24bot:f24.invalid',
  allowed: '@f24allowed:f24.invalid',
  denied: '@f24denied:f24.invalid',
  room1: '!f24room1:f24.invalid',
  room2: '!f24room2:f24.invalid',
};

// ── signal identities ───────────────────────────────────────────────────────
// Peer-keyed. `subprocess.rs:281-287` binds `conversation_id` to
// `groupInfo.groupId ?? source ?? sourceUuid` and `:292-297` binds `sender_id`
// to `sourceUuid ?? source ?? sourceName`. The fixture emits only `source`, so
// both resolve to the same e164 string and the shape matches whatsapp/sms/
// telegram — which is what lets signal's access leg exercise the SAME shared
// gate the other four are measured on.
export const SIG = {
  account: '+15550240099',
  allowed: '+15552240001',
  denied: '+15559990001',
  second: '+15552240002',
};

const WEBHOOK_PORT = 18787;
const ARRIVAL_BUDGET_MS = 90_000;

// How long the shipped binary's inbound dedupe cache remembers a message id.
//
// Read from the product, not guessed: `bootstrap.rs:3234` and
// `channel_inbound_host.rs` both construct `InboundSubscriber::new(..., 60_000,
// 1024)`, and `DedupeCache` measures expiry from `first_seen`
// (`dispatch/dedupe.rs:107`), so a replay later than this is EXPECTED to produce
// a second turn — that is the cache working as designed, not a duplicate
// leaking through.
//
// This constant exists because the driver got it wrong first. Email's `admit`
// leg burns its full arrival budget (its reply can never arrive — see
// `mailFixtureSupported`), which pushed the dedupe replay to **90.1s** after the
// original. The leg dutifully reported FAIL, and the product was correct: the
// entry had expired 30 seconds earlier. A driver that cannot see its own
// timing writes product defects out of its own latency.
const DEDUPE_TTL_MS = 60_000;

// Per-adapter arrival budget. An adapter whose reply path is known-blocked must
// not spend 90s per leg waiting for an arrival that provably cannot come — the
// waiting is what pushed the dedupe replay outside the TTL above.
const ARRIVAL_BUDGET_BY_ADAPTER = {
  email: 20_000,
};

// ── grading predicates ───────────────────────────────────────────────────────
//
// Extracted as pure functions so the self-test exercises THE CODE THAT RUNS,
// not a transcription of it. A self-test that re-implements the predicate it is
// checking passes on a broken instrument, which is the class this phase has
// recorded eleven times.

/// Grade the steady-state leg. `counts` is per-message arrival counts.
///
/// Deliberately requires EVERY message, not a majority: F24-C3-H4 lost 5 of 6,
/// and a threshold that tolerated one loss would have graded a 1-in-6 silent
/// drop a PASS. It is also inherently self-controlling against the
/// universal-denial trap — it demands arrivals > 0, so a path that denies
/// everything scores 0 and FAILS where a negative leg would have "passed".
export function gradeSteady(counts, want) {
  const arrived = counts.filter((n) => n >= 1).length;
  return { ok: arrived === want && want > 0, arrived, want };
}

/// Grade the matrix restart probe into THREE states, not two.
///
///   PASS        the gap message survived the restart
///   LOSS        it did not, and every control held — attributable to the product
///   INCOMPLETE  a control did not hold, so a zero is not attributable at all
///
/// The INCOMPLETE state is the whole point. `servedAfterRestart` comes from the
/// FIXTURE's own report in another process: if the fixture never served the gap
/// event to the restarted process at all, then "it did not arrive" is a harness
/// fault and reporting it as product loss would be a fabricated HIGH against
/// working code — which this program has already come within one step of doing
/// (a dedupe FAIL that was really a 90s replay against a 60s TTL).
///
/// **F24-C3-H6 instrument repair (24-h6).** This control was `servedInInitial`:
/// it demanded the gap event appear in a post-restart INITIAL sync's timeline.
/// That is where the gap event lands only while the product is BROKEN. A fixed
/// adapter resumes from a persisted cursor, so after a restart it issues an
/// INCREMENTAL sync and never an initial one — and the old control was
/// therefore false on every correct run, forcing `INCOMPLETE` and making a PASS
/// unreachable by construction. The probe could report the defect but could not
/// report the fix. Widened to "the fixture served the gap event on SOME sync
/// after the restart", which is what the exclusion always meant: H2 is "the
/// fixture never served it", not "the fixture never served it on an initial
/// sync". Strictly stronger — it still excludes H2, and it now grades both
/// states of the product instead of one.
export function gradeRestart({ preArrivals, postArrivals, servedAfterRestart, gapArrivals }) {
  const controlsHeld = preArrivals >= 1 && postArrivals >= 1 && servedAfterRestart === true;
  if (!controlsHeld) return { state: 'INCOMPLETE', graded: false, ok: false };
  if (gapArrivals >= 1) return { state: 'PASS', graded: true, ok: true };
  return { state: 'LOSS', graded: true, ok: false };
}

/// Did the fixture serve `gapId` on any sync after the restart, and on which
/// KIND of sync? Reads the fixture's own per-sync record (`syncs[]`, another
/// process), which carries `initial`, `since` and `served` for every request.
///
/// `where` is the mechanism in one field, and it is the difference between the
/// two states of this defect:
///   'initial'      served only on a sync whose timeline the adapter discards
///                  — the message was offered and thrown away (the defect);
///   'incremental'  served on a resumed sync — the adapter asked for the window
///                  it missed and was given it (the fix).
export function servedAfterRestartFrom(allSyncs, syncsBeforeRestart, gapId) {
  const after = (allSyncs ?? []).filter((s) => s.sync > syncsBeforeRestart);
  const carrying = after.filter((s) => (s.served ?? []).includes(gapId));
  const onIncremental = carrying.some((s) => s.initial === false);
  const onInitial = carrying.some((s) => s.initial === true);
  return {
    served: carrying.length > 0,
    where: onIncremental ? 'incremental' : onInitial ? 'initial' : null,
    on_initial: onInitial,
    on_incremental: onIncremental,
    syncs_after_restart: after.length,
    served_lists: after.map((s) => ({ sync: s.sync, initial: s.initial, served: s.served })),
  };
}

/// The extraction this repair replaces, kept executable so the self-test can
/// assert the repair actually changes an outcome. NEVER call this from the
/// driver. It looks ONLY inside post-restart initial syncs, so on a fixed
/// product — where there is no post-restart initial sync — it returns false and
/// forces INCOMPLETE.
export function legacyServedInInitialOnly(initialSyncs, initialsBefore, gapId) {
  return (initialSyncs ?? [])
    .slice(initialsBefore)
    .some((s) => (s.served ?? []).includes(gapId));
}

/// The grader this module replaces, kept executable so the self-test can assert
/// the repair actually changes an outcome. NEVER call this from the driver.
///
/// It is what a probe written without the H2 exclusion looks like: zero
/// arrivals means loss, full stop. On a run where the fixture never served the
/// gap event it reports a product defect that does not exist.
export function naiveGradeRestart({ gapArrivals }) {
  return { state: gapArrivals >= 1 ? 'PASS' : 'LOSS', graded: true, ok: gapArrivals >= 1 };
}

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

/// Is `pid` a LIVE process — as opposed to a zombie this driver has not reaped?
///
/// FOURTH INSTRUMENT DEFECT OF THIS LANE, measured rather than reasoned, and the
/// third that failed in the direction that blames the product.
///
/// `process.kill(pid, 0)` is the obvious liveness check and it is WRONG here.
/// Node reaps its children on the event loop; this driver's waits are blocking
/// (`Atomics.wait`), so the loop never turns, the child is never `wait()`ed, and
/// a process that died instantly stays a ZOMBIE — for which `kill(pid, 0)`
/// succeeds. Measured directly: SIGKILL a child, block 2s, and
/// `process.kill(pid,0)` reports ALIVE while `ps -o stat=` reports `Z`.
///
/// What that cost: run 1's restart probe reported `exit_secs=30 (SIGKILL)`,
/// which reads as "`--json-stream` ignored SIGTERM for 30 seconds". That is a
/// product claim, and it was very probably this bug — the binary may have
/// exited immediately and simply sat unreaped. Reporting it would have been a
/// fabricated finding against working shutdown code.
///
/// So liveness is read from the OS's own view of the process state, which does
/// distinguish a zombie.
function pidIsLive(pid) {
  try {
    process.kill(pid, 0);
  } catch {
    return false; // gone entirely
  }
  // Still in the table — but a zombie is not running. Linux first (this is
  // where every figure in this criterion is taken), `ps` as the portable
  // fallback for macOS/BSD.
  try {
    if (process.platform === 'linux' && fs.existsSync(`/proc/${pid}/stat`)) {
      const stat = fs.readFileSync(`/proc/${pid}/stat`, 'utf8');
      // The comm field is parenthesised and may itself contain spaces, so the
      // state is the first token AFTER the last ')'.
      const state = stat.slice(stat.lastIndexOf(')') + 1).trim().split(/\s+/)[0];
      return state !== 'Z';
    }
  } catch {
    /* fall through to ps */
  }
  const r = spawnSync('ps', ['-o', 'stat=', '-p', String(pid)], { encoding: 'utf8' });
  const state = (r.stdout ?? '').trim();
  if (!state) return false; // ps cannot see it either
  return !state.startsWith('Z');
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
    this.mailJournalPath = path.join(this.runDir, 'mail.jsonl');
    // Matrix replies leave by `PUT /rooms/{room}/send/m.room.message/{txn}`
    // (rest.rs:135), which is the fixture homeserver — same property as
    // telegram's: a journal owned by a process the binary can only write to by
    // completing a real TCP round trip.
    this.mxJournalPath = path.join(this.runDir, 'matrix.jsonl');
    // Signal is the one adapter whose reply does NOT cross a socket the binary
    // dialled: it is a JSON-RPC frame written to a child process's stdin. The
    // journal is still owned by another OS process — the child — and the binary
    // cannot write to it except by actually sending the frame, so the
    // load-bearing property survives the change of transport.
    this.sigJournalPath = path.join(this.runDir, 'signal.jsonl');
    // Written by the fake signal-cli AFTER it binds its control listener, and
    // re-written on every respawn the supervisor performs.
    this.sigControlPath = path.join(this.runDir, 'signal-control.port');
    this.sigCliPath = path.join(this.runDir, 'signal-cli');
    this.children = [];
    // Legs that could not be attempted, with the reason. Distinct from a FAIL
    // and distinct from a zero: this driver's header rule is that an adapter
    // whose inbound path cannot be pointed at a fixture is reported NOT
    // MEASURED with the reason, never as a zero and never as a pass.
    this.notMeasured = [];
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
    this.mailUser = 'f24c3';
    this.mailPass = crypto.randomBytes(18).toString('hex');
    // Minted here like every other secret in this file. A Matrix access token is
    // an opaque bearer string; the fixture 401s anything else, so a
    // misconfigured run fails as auth rather than as silence.
    this.mxAccessToken = `syt_f24c3_${crypto.randomBytes(20).toString('base64url')}`;
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

  /// Install the fake `signal-cli` where the config points, as an EXECUTABLE.
  ///
  /// It is not started here and must not be: the product spawns it
  /// (`subprocess.rs:54`), which is the entire point of the seam. What this
  /// does is put a runnable file at `signal_cli_path` and make it executable —
  /// a copy without the mode bit fails as `Spawn(Permission denied)` and every
  /// signal leg would read as zero arrivals for a reason that has nothing to do
  /// with the inbound path.
  installSignalCli() {
    const src = path.join(HERE, 'f24-signal-fixture.mjs');
    fs.copyFileSync(src, this.sigCliPath);
    fs.chmodSync(this.sigCliPath, 0o755);
    // Assert rather than assume. `fs.chmodSync` is a no-op for the owner-exec
    // bit on some filesystems (and on Windows entirely), and the failure mode
    // is a spawn error 30 seconds later attributed to the product.
    const mode = fs.statSync(this.sigCliPath).mode & 0o777;
    if ((mode & 0o100) === 0) {
      throw new Error(
        `f24-inbound: ${this.sigCliPath} is not owner-executable after chmod (mode=${mode.toString(8)}); ` +
          `the product's Command::new would fail to spawn it and the signal legs would ` +
          `report zero arrivals for a filesystem reason`,
      );
    }
    this.note(
      `signal-cli fixture installed ${this.sigCliPath} mode=${mode.toString(8)} ` +
        `bytes=${fs.statSync(this.sigCliPath).size}`,
    );
  }

  /// Read the control port the fake signal-cli published, waiting for the
  /// product to have spawned it. Returns `{port, pid}` or null.
  ///
  /// Re-read every time rather than cached: the supervisor respawns the
  /// executable on death, and a cached port would send the driver at a listener
  /// that closed with the previous incarnation — producing zero arrivals that
  /// look exactly like product loss.
  sigControl(budgetSecs = 60) {
    for (let i = 0; i < budgetSecs; i += 1) {
      if (fs.existsSync(this.sigControlPath)) {
        const raw = fs.readFileSync(this.sigControlPath, 'utf8').trim();
        const [port, pid] = raw.split(/\s+/);
        if (Number.isFinite(Number(port)) && Number(port) > 0) {
          return { port: Number(port), pid: Number(pid) };
        }
      }
      process.stdout.write(
        `[inbound] waiting for the product to spawn signal-cli: ${i}s ${new Date().toISOString()}\n`,
      );
      sleep(1000);
    }
    return null;
  }

  /// One line-delimited JSON command to the fake signal-cli's control socket.
  /// A child process for the same reason `post()` is one: this driver's waits
  /// are blocking, so its own event loop is parked.
  sigCommand(obj) {
    const ctl = this.sigControl(5);
    if (!ctl) return { status: 1, output: 'no signal-cli control port' };
    const script = `
      const net = require('node:net');
      const s = net.connect(${ctl.port}, '127.0.0.1', () => s.write(${JSON.stringify(`${JSON.stringify(obj)}\n`)}));
      let buf = '';
      s.setEncoding('utf8');
      s.on('data', (c) => {
        buf += c;
        if (buf.includes('\\n')) { process.stdout.write(buf.trim()); s.end(); }
      });
      s.on('error', (e) => { process.stdout.write('SIGCTL FAILED ' + e.message); process.exit(1); });
    `;
    return run([process.execPath, '-e', script], { timeout: 30_000 });
  }

  /// Talk to the matrix fixture's HTTP control plane.
  mxCommand(pathname, obj) {
    const script = `
      fetch(${JSON.stringify(`${this.mxUrl}${pathname}`)}, ${
        obj
          ? `{ method: 'POST', headers: { 'content-type': 'application/json' }, body: ${JSON.stringify(JSON.stringify(obj))} }`
          : '{}'
      })
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

  mxReport() {
    if (!this.mxUrl) return null;
    return this.mxCommand('/__control/report', null);
  }

  /// Mint a throwaway CA-less self-signed certificate for the mail fixture.
  ///
  /// Returns the cert path, or `null` with the reason recorded. `null` is NOT a
  /// failure of the product — it means this host cannot host the fixture, and
  /// the email legs are reported NOT MEASURED rather than FAILED.
  mintMailCert() {
    const dir = path.join(this.runDir, 'tls');
    fs.mkdirSync(dir, { recursive: true });
    const cert = path.join(dir, 'fixture-cert.pem');
    const key = path.join(dir, 'fixture-key.pem');
    const r = run([
      'openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes',
      '-keyout', key, '-out', cert,
      '-days', '1', '-subj', '/CN=localhost',
      '-addext', 'subjectAltName=DNS:localhost,IP:127.0.0.1',
    ]);
    if (r.status !== 0 || !fs.existsSync(cert)) {
      this.note(`openssl could not mint a fixture certificate (rc=${r.status}): ${r.output.slice(0, 400)}`);
      return null;
    }
    this.note(`mail fixture cert ${cert} (${fs.statSync(cert).size} bytes)`);
    return { cert, key };
  }

  /// Whether the shipped binary can be pointed at a self-signed IMAP fixture on
  /// THIS host, and why not when it cannot.
  ///
  /// `crates/wcore-channel-email/Cargo.toml:13` pulls `native-tls`, and
  /// `imap.rs:194` calls `native_tls::TlsConnector::new()`. That resolves to a
  /// different trust store per platform:
  ///
  ///   Linux    OpenSSL, which reads `SSL_CERT_FILE` at runtime  -> reachable
  ///   macOS    Security.framework / the system keychain, which ignores
  ///            `SSL_CERT_FILE` entirely                          -> NOT reachable
  ///   Windows  SChannel / the system cert store, same problem    -> NOT reachable
  ///
  /// Reported rather than silently skipped, because "email produced no
  /// arrivals" on a Mac would otherwise read as a product defect when it is a
  /// property of the platform's TLS trust store.
  mailFixtureSupported() {
    if (process.platform === 'linux') return { ok: true };
    return {
      ok: false,
      reason:
        `SSL_CERT_FILE cannot redirect native-tls trust on ${process.platform}: ` +
        `imap.rs:194 uses native_tls::TlsConnector::new(), which is ` +
        `${process.platform === 'darwin' ? 'Security.framework (system keychain)' : 'SChannel (system cert store)'} ` +
        `here and ignores the variable. Only the Linux/OpenSSL backend honours it. ` +
        `A ${process.platform} email leg needs a different mechanism (a trusted-root install, ` +
        `or a cert-source knob in the adapter) — it is NOT achievable from configuration alone.`,
    };
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
        `"email.f24c3.imap_user" = "${this.mailUser}"`,
        `"email.f24c3.imap_pass" = "${this.mailPass}"`,
        `"email.f24c3.smtp_user" = "${this.mailUser}"`,
        `"email.f24c3.smtp_pass" = "${this.mailPass}"`,
        `"matrix.f24c3.access_token" = "${this.mxAccessToken}"`,
        // Signal has NO entry here, and its absence is a fact about the product
        // rather than an omission: `make_signal` (registry:157-169) takes the
        // credentials store and deliberately ignores it — signal-cli owns its
        // own credential state. There is nothing for this file to hold.
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

    // ── matrix (POLLING, HTTP long-poll on /sync) ─────────────────────────
    //
    // `homeserver_url` is the whole seam, and it is stronger than telegram's:
    // `MatrixConfig.homeserver_url` (config.rs:9) is REQUIRED, carries no
    // `#[serde(default)]` and no production constant, and `MatrixChannel::new`
    // (lib.rs:61) copies it directly into `api_base`. There is therefore no
    // production default this run could be masking, and consequently no control
    // test to write for it — unlike signal below.
    //
    // `dm_allowlist` carries both room senders. Matrix is room-keyed, so the
    // bind leg's second conversation is a second ROOM from the SAME sender;
    // the denied mxid is the only identity the access leg needs to keep out.
    if (this.mxUrl) {
      fs.writeFileSync(
        path.join(this.home, 'channels', 'f24c3matrix.toml'),
        [
          'name = "f24c3matrix"',
          'platform = "matrix"',
          'enabled = true',
          '',
          '[options]',
          `homeserver_url = "${this.mxUrl}"`,
          'credential_handle_access_token = "matrix.f24c3.access_token"',
          `user_id = "${MX.bot}"`,
          '',
          '[inbound]',
          'dm = "allowlist"',
          `dm_allowlist = ["${MX.allowed}"]`,
          'group = "disabled"',
          'require_mention = true',
          'tools = "conversational"',
          '',
        ].join('\n'),
      );
    }

    // ── signal (SUBPROCESS, JSON-RPC over stdio) ──────────────────────────
    //
    // `signal_cli_path` points at the fixture executable. Note what this
    // deliberately does NOT do: `SignalChannel::with_launcher` is a real seam
    // and is `#[doc(hidden)]`, so pointing the test at it would prove nothing
    // an operator can reproduce. Going through `signal_cli_path` exercises
    // `new()` -> `Arc::new(RealLauncher)` -> `Command::new(cli_path)`, which is
    // the path `make_signal` actually takes in a shipped binary.
    //
    // CONTROL ASSERTION FOR THE DEFAULT. Unlike matrix, signal DOES carry a
    // production default: `#[serde(default = "default_signal_cli_path")]`
    // resolves to a bare `signal-cli` on `$PATH` (config.rs:16-18). That
    // default is asserted intact in the self-test rather than here, so this run
    // cannot be read as evidence that a config naming no path still works.
    fs.writeFileSync(
      path.join(this.home, 'channels', 'f24c3signal.toml'),
      [
        'name = "f24c3signal"',
        'platform = "signal"',
        'enabled = true',
        '',
        '[options]',
        `signal_cli_path = ${JSON.stringify(this.sigCliPath)}`,
        `account = "${SIG.account}"`,
        'send_timeout_secs = 20',
        '',
        '[inbound]',
        'dm = "allowlist"',
        // Peer-keyed, so the bind leg's second conversation is a second person
        // and the allowlist has to admit both or the leg would be re-measuring
        // the access gate. Same shape as whatsapp/sms/telegram.
        `dm_allowlist = ["${SIG.allowed}", "${SIG.second}"]`,
        'group = "disabled"',
        'require_mention = true',
        'tools = "conversational"',
        '',
      ].join('\n'),
    );

    // ── email (POLLING, IMAP inbound + SMTP outbound) ─────────────────────
    //
    // `[inbound]` is written BEFORE `[options]` because once `[options.smtp]`
    // is opened, every following table is nested under it in TOML unless a new
    // top-level table is declared — and a silently-nested `[inbound]` would
    // deserialise as an unknown field on `SmtpConfig`, which is
    // `deny_unknown_fields`. The channel would then be skipped at load and the
    // whole adapter would read as "no arrivals" for a reason that has nothing
    // to do with the inbound path.
    //
    // `poll_interval_secs = 2` because the matrix's settle windows are 20s: a
    // default 60s poll would make every negative leg trivially "pass" by never
    // having polled at all.
    if (this.mailPorts) {
      fs.writeFileSync(
        path.join(this.home, 'channels', 'f24c3email.toml'),
        [
          'name = "f24c3email"',
          'platform = "email"',
          'enabled = true',
          '',
          '[inbound]',
          'dm = "allowlist"',
          // Email is peer-keyed: imap.rs:465-471 sets BOTH `sender_id` and
          // `conversation_id` to the normalised bare addr-spec of `From:`.
          'dm_allowlist = ["allowed@fixture.invalid", "second@fixture.invalid"]',
          'group = "disabled"',
          'require_mention = true',
          'tools = "conversational"',
          '',
          '[options]',
          'from_address = "bot@fixture.invalid"',
          '',
          '[options.smtp]',
          // `localhost`, not `127.0.0.1`: the fixture certificate carries
          // `DNS:localhost` and an IP literal would be verified against an IP
          // SAN instead.
          'host = "localhost"',
          `port = ${this.mailPorts.smtp}`,
          'user_credential_handle = "email.f24c3.smtp_user"',
          'password_credential_handle = "email.f24c3.smtp_pass"',
          '',
          '[options.imap]',
          'host = "localhost"',
          `port = ${this.mailPorts.imap}`,
          'user_credential_handle = "email.f24c3.imap_user"',
          'password_credential_handle = "email.f24c3.imap_pass"',
          'mailbox = "INBOX"',
          'poll_interval_secs = 2',
          'allowed_senders = []',
          '',
        ].join('\n'),
      );
    }
  }

  // ── the binary ────────────────────────────────────────────────────────────

  buildInfo() {
    const r = run([this.args.binary, '--build-info']);
    if (r.status !== 0) throw new Error(`--build-info failed rc=${r.status}: ${r.output}`);
    return r.output;
  }

  /// Start the shipped binary.
  ///
  /// `tag` names the incarnation. The restart probe needs a SECOND start, and
  /// the original unconditionally truncated `core.log` — which would have
  /// destroyed the first incarnation's log at the exact moment the probe needed
  /// to compare the two. Each incarnation therefore gets its own log file.
  startBinary(tag = 'core') {
    const logPath = path.join(this.runDir, `${tag}.log`);
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
        // CHILD-SCOPED. This is the whole email seam on Linux: `imap.rs:194`
        // builds a `native_tls::TlsConnector`, which on Linux is OpenSSL, and
        // OpenSSL reads `SSL_CERT_FILE` at runtime for its default verify
        // paths. Setting it here — and only here, on this child — makes the
        // SHIPPED binary trust the run's throwaway fixture certificate without
        // touching Rust, without touching the host trust store, and without
        // outliving the process.
        //
        // It has NO effect on the SMTP path. `Cargo.toml:11` selects
        // `lettre/tokio1-rustls-tls`, whose resolved dependency set contains
        // `webpki-roots` and not `rustls-native-certs`; `webpki-roots` is a
        // compiled-in Mozilla root set that reads no file and no environment
        // variable, on any platform.
        ...(this.mailCert ? { SSL_CERT_FILE: this.mailCert } : {}),
        // INHERITED BY THE FAKE signal-cli, which is the point. The product
        // spawns it with `Command::new(cli_path)` and no `.env()` call
        // (subprocess.rs:54-62), so the child inherits this process's
        // environment — which is how the fixture learns where to journal and
        // where to publish its control port. Nothing here is a credential.
        F24_SIGNAL_JOURNAL: this.sigJournalPath,
        F24_SIGNAL_CONTROL: this.sigControlPath,
        RUST_LOG:
          'wcore_agent::bootstrap=info,wcore_agent::channel_inbound=debug,' +
          'wcore_channels=debug,wcore_channel_email=debug,' +
          'wcore_channel_matrix=debug,wcore_channel_signal=debug',
      },
      windowsHide: true,
    });
    this.coreChild = child;
    this.children.push(child);
    this.coreLog = logPath;
    this.coreLogs = [...(this.coreLogs ?? []), logPath];
    return logPath;
  }

  /// Stop the shipped binary and WAIT for the OS to reap it.
  ///
  /// Waiting is load-bearing for the restart probe. `SIGTERM` returning is not
  /// the process being gone, and a probe that injected the gap message while
  /// the old incarnation was still draining its final `/sync` would have the
  /// old process consume it — producing a PASS that proves the opposite of what
  /// the leg is asking. The fixture's `sync.open` records are the independent
  /// check that nothing polled during the gap.
  stopBinary(budgetSecs = 30) {
    const child = this.coreChild;
    if (!child) return { stopped: false, reason: 'no child' };
    try {
      child.kill('SIGTERM');
    } catch {
      /* already gone */
    }
    for (let i = 0; i < budgetSecs; i += 1) {
      // `pidIsLive`, NOT `process.kill(pid, 0)` — see that function. The child
      // was spawned by this process and this process's event loop is blocked,
      // so a dead child sits unreaped and `kill(pid, 0)` calls it alive.
      const alive = pidIsLive(child.pid);
      if (!alive) {
        this.note(`binary pid=${child.pid} exited after ${i}s`);
        return { stopped: true, secs: i, pid: child.pid };
      }
      process.stdout.write(
        `[inbound] waiting for the binary to exit: ${i}s pid=${child.pid} ${new Date().toISOString()}\n`,
      );
      sleep(1000);
    }
    // SIGKILL, then CONFIRM. The first live run hit this path — `--json-stream`
    // did not exit within 30s of SIGTERM — and the original returned
    // `stopped: true` the instant it sent SIGKILL, without checking. That would
    // report "the binary was down" for a process that might still have been
    // running, which is the one fact the gap leg depends on. The run's other
    // control (the fixture's sync_total staying flat) happened to hold, but a
    // probe must not rely on a second control to cover a claim it makes itself.
    try {
      child.kill('SIGKILL');
    } catch {
      /* already gone */
    }
    let reaped = false;
    for (let i = 0; i < 15; i += 1) {
      if (!pidIsLive(child.pid)) {
        reaped = true;
        break;
      }
      process.stdout.write(`[inbound] waiting for SIGKILL to land: ${i}s pid=${child.pid}\n`);
      sleep(1000);
    }
    this.note(
      `binary pid=${child.pid} did not exit within ${budgetSecs}s of SIGTERM; SIGKILLed, ` +
        `reaped=${reaped}`,
    );
    return { stopped: reaped, secs: budgetSecs, pid: child.pid, sigkill: true, reaped };
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

  /// Every record in the mail fixture's journal.
  mailJournal() {
    if (!fs.existsSync(this.mailJournalPath)) return [];
    return fs
      .readFileSync(this.mailJournalPath, 'utf8')
      .split('\n')
      .filter((l) => l.trim())
      .map((l) => {
        try {
          return JSON.parse(l);
        } catch {
          return null;
        }
      })
      .filter(Boolean);
  }

  /// Email replies, projected into the shared arrival shape. An email reply is
  /// an SMTP delivery; its single envelope recipient is the conversation, which
  /// matches `imap.rs:465-471` binding `conversation_id` to the peer address.
  mailArrivals() {
    return this.mailJournal()
      .filter((r) => r.kind === 'smtp.delivered')
      .map((r) => ({ text: r.data, conversation_id: (r.rcpt_to ?? [])[0] ?? null }));
  }

  /// Every record in the matrix fixture's journal.
  mxJournal() {
    return this.readJsonl(this.mxJournalPath);
  }

  /// Matrix replies, projected into the shared arrival shape. A Matrix reply is
  /// `PUT /rooms/{room}/send/m.room.message/{txn}` (rest.rs:135); the room is
  /// the conversation, matching `sync.rs:363` binding `conversation_id` to the
  /// room id on the way in.
  mxArrivals() {
    return this.mxJournal()
      .filter((r) => r.kind === 'sendMessage')
      .map((r) => ({ text: r.text, conversation_id: r.room }));
  }

  /// Every record the fake signal-cli wrote — across ALL of its incarnations.
  /// `supervisor.rs` respawns the executable when it dies, so each record
  /// carries the pid that wrote it and a respawn is legible rather than
  /// silently merged into one timeline.
  sigJournal() {
    return this.readJsonl(this.sigJournalPath);
  }

  /// Signal replies, projected into the shared arrival shape. A Signal reply is
  /// a JSON-RPC `send` frame on the child's stdin (`lib.rs:249`), whose single
  /// `recipient` is the conversation — matching `subprocess.rs:281-287` binding
  /// `conversation_id` to `source` on the way in.
  sigArrivals() {
    return this.sigJournal()
      .filter((r) => r.kind === 'send')
      .map((r) => ({ text: r.message, conversation_id: r.recipient ?? r.group_id ?? null }));
  }

  /// Shared JSONL reader. Extracted because there are now five journals and a
  /// per-journal copy of this loop is five places for a parse guard to be
  /// forgotten in.
  readJsonl(p) {
    if (!fs.existsSync(p)) return [];
    return fs
      .readFileSync(p, 'utf8')
      .split('\n')
      .filter((l) => l.trim())
      .map((l) => {
        try {
          return JSON.parse(l);
        } catch {
          return null;
        }
      })
      .filter(Boolean);
  }

  /// Which journal an adapter's replies land in.
  readerFor(adapter) {
    if (adapter === 'telegram') return () => this.tgArrivals();
    if (adapter === 'email') return () => this.mailArrivals();
    if (adapter === 'matrix') return () => this.mxArrivals();
    if (adapter === 'signal') return () => this.sigArrivals();
    return () => this.arrivals();
  }

  /// Record every leg of an adapter as NOT MEASURED, with the reason.
  ///
  /// Deliberately NOT a FAIL. A FAIL asserts the product did the wrong thing;
  /// this asserts the driver could not ask the question on this host. Conflating
  /// them is how a harness limitation gets filed as a product regression.
  recordNotMeasured(adapter, reason) {
    this.note(`NOT MEASURED ${adapter}: ${reason}`);
    for (const leg of LEGS) {
      this.notMeasured.push({ adapter, leg, reason });
    }
  }

  /// One leg that ran but cannot be graded, with the reason. Distinct from both
  /// PASS/FAIL and from NOT MEASURED: the question was asked, and the answer is
  /// not interpretable.
  recordIncomplete(adapter, leg, reason) {
    this.note(`INCOMPLETE ${adapter}/${leg}: ${reason}`);
    this.notMeasured.push({ adapter, leg, reason, ran: true });
  }

  /// A separately-named probe of the email INBOUND path, graded on the turns
  /// journal rather than on arrivals.
  ///
  /// THIS IS NOT THE FIVE LEGS AND IS NOT COUNTED AS THEM. 24-C3's legs are
  /// defined on ARRIVALS — a reply that leaves the binary and lands in a journal
  /// it does not own. Email's replies leave by SMTP, and SMTP cannot reach a
  /// fixture (see `mailFixtureSupported` and the webpki-roots note on
  /// `SSL_CERT_FILE`), so those five legs are recorded NOT MEASURED with the
  /// measured TLS error. Redefining them onto a weaker observable to reach a
  /// green would be exactly the "redefine success downward" move this program
  /// has already paid for once.
  ///
  /// What this probe establishes instead is narrower and stated as such: that a
  /// message delivered to a real IMAP mailbox is fetched by the shipped binary
  /// over TLS, admitted or refused by the shared access gate, deduplicated, and
  /// turned into a real model turn against the fixture endpoint. The turns
  /// journal is written by a DIFFERENT out-of-process fixture than the mailbox,
  /// so a claim here still cannot be satisfied by the binary talking to itself.
  runEmailAdmissionProbe() {
    const tag = crypto.randomBytes(4).toString('hex');
    const probe = [];
    const rec = (leg, ok, detail) => {
      probe.push({ leg, ok, detail });
      process.stdout.write(`[inbound] ${ok ? 'PASS' : 'FAIL'} email-admission/${leg} — ${detail}\n`);
    };
    const fetchCount = () => this.mailJournal().filter((r) => r.kind === 'imap.uid_fetch').length;
    const send = (sender, text, messageId) =>
      this.mailControl(
        JSON.stringify({
          op: 'deliver',
          from: sender,
          to: 'bot@fixture.invalid',
          subject: 'f24c3 inbound',
          body: text,
          messageId: `<${messageId}@fixture.invalid>`,
        }),
      );
    const awaitTurns = (corr, want, budgetSecs) => {
      for (let i = 0; i < budgetSecs; i += 1) {
        if (this.turnsFor(corr).length >= want) break;
        process.stdout.write(
          `[inbound] awaiting email turn ${corr}: ${this.turnsFor(corr).length}/${want} ` +
            `after ${i}s ${new Date().toISOString()}\n`,
        );
        sleep(1000);
      }
      return this.turnsFor(corr);
    };

    // ── fetch: the binary reads the mailbox over TLS at all ────────────────
    const fetchesBefore = fetchCount();
    const c1 = `f24c3-email-admit-${tag}`;
    const startedAt = Date.now();
    const s1 = send('allowed@fixture.invalid', `hello ${c1}`, `${tag}.0001`);
    const t1 = awaitTurns(c1, 1, 30);
    const fetchesAfter = fetchCount();
    rec(
      'fetch',
      fetchesAfter > fetchesBefore,
      `deliver=${s1.output.slice(0, 60)} | imap.uid_fetch before=${fetchesBefore} after=${fetchesAfter} ` +
        `(the shipped binary completed a TLS IMAP session against the fixture cert via SSL_CERT_FILE)`,
    );

    // ── admit ──────────────────────────────────────────────────────────────
    rec(
      'admit',
      t1.length === 1,
      `turns(fixture-journal)=${t1.length} want=1 for an allowlisted From:`,
    );

    // ── dedupe: SAME RFC Message-ID, new UID, inside the TTL ───────────────
    const before2 = this.turnsFor(c1).length;
    send('allowed@fixture.invalid', `hello ${c1}`, `${tag}.0001`);
    const replayDelayMs = Date.now() - startedAt;
    this.settle(15_000);
    const after2 = this.turnsFor(c1).length;
    const c1b = `f24c3-email-dedupe-control-${tag}`;
    send('allowed@fixture.invalid', `hello ${c1b}`, `${tag}.0002`);
    const control = awaitTurns(c1b, 1, 30);
    if (replayDelayMs >= DEDUPE_TTL_MS) {
      rec(
        'dedupe',
        false,
        `NOT GRADEABLE: replay landed +${replayDelayMs}ms, outside the ${DEDUPE_TTL_MS}ms TTL`,
      );
    } else {
      rec(
        'dedupe',
        after2 === before2 && control.length === 1,
        `replay of the SAME Message-ID at +${replayDelayMs}ms (inside the ${DEDUPE_TTL_MS}ms TTL) | ` +
          `turns before=${before2} after=${after2} (want equal) | ` +
          `positive-control fresh Message-ID turns=${control.length} want=1`,
      );
    }

    // ── access ─────────────────────────────────────────────────────────────
    const c3 = `f24c3-email-access-${tag}`;
    const fetchesBeforeDenied = fetchCount();
    send('denied@fixture.invalid', `hello ${c3}`, `${tag}.0003`);
    this.settle(20_000);
    const t3 = this.turnsFor(c3);
    const fetchesAfterDenied = fetchCount();
    rec(
      'access',
      t3.length === 0 && t1.length === 1 && fetchesAfterDenied > fetchesBeforeDenied,
      `denied From: turns=${t3.length} want=0 | CONTROL admit-turn=${t1.length} want=1 | ` +
        `CONTROL the denied message WAS fetched (uid_fetch ${fetchesBeforeDenied}->${fetchesAfterDenied}), ` +
        `so the zero is a refusal at the access gate and not an unread mailbox`,
    );

    // ── bind: explicitly NOT claimed ───────────────────────────────────────
    const c4 = `f24c3-email-bind-${tag}`;
    send('second@fixture.invalid', `hello ${c4}`, `${tag}.0004`);
    const t4 = awaitTurns(c4, 1, 30);
    rec(
      'second-sender-admitted',
      t4.length === 1,
      `turns=${t4.length} want=1 for the second allowlisted sender. NOTE this is NOT the ` +
        `24-C3 bind leg: the turns journal carries no conversation id, so it cannot show the two ` +
        `senders bound to DISTINCT sessions. That remains unproven for email.`,
    );

    // ── steady state ───────────────────────────────────────────────────────
    //
    // THE LEG THIS LANE EXISTS FOR. F24-C3-H4 lost 5 of 6 messages in steady
    // state on Telegram because two managers competed for one destructive read.
    // Email carries the same hazard in two forms — `\Seen` on a non-PEEK fetch,
    // and a UID watermark persisted OUTSIDE the session and keyed by
    // (host, user, mailbox), so two pollers race on a shared file. That root
    // cause is reported fixed, but email had never been driven at all, so
    // nothing had confirmed the fix reaches it or that email has no second loss
    // mode of its own.
    //
    // Six messages, delivered back-to-back rather than one at a time, because a
    // race needs concurrency to show. Graded on three independent counts:
    // deliveries the fixture accepted, fetches the fixture served, and turns a
    // DIFFERENT fixture journalled.
    const steadyN = 6;
    const steadyTokens = [];
    const fetchesBeforeSteady = fetchCount();
    for (let i = 0; i < steadyN; i += 1) {
      const c = `f24c3-email-steady${i}-${tag}`;
      steadyTokens.push(c);
      send('allowed@fixture.invalid', `hello ${c}`, `${tag}.10${i}`);
    }
    for (let i = 0; i < 60; i += 1) {
      const got = steadyTokens.filter((c) => this.turnsFor(c).length >= 1).length;
      if (got >= steadyN) break;
      process.stdout.write(
        `[inbound] steady state: ${got}/${steadyN} turns after ${i}s ${new Date().toISOString()}\n`,
      );
      sleep(1000);
    }
    const perToken = steadyTokens.map((c) => this.turnsFor(c).length);
    const turnsSeen = perToken.filter((n) => n >= 1).length;
    const duplicated = perToken.filter((n) => n > 1).length;
    const fetchesAfterSteady = fetchCount();
    const report = this.mailReport();
    const steadyMsgs =
      report && report.ok ? report.messages.filter((m) => m.uid >= 1000 + 5) : [];
    const multiFetched = steadyMsgs.filter((m) => m.fetch_count > 1).length;
    const unfetched = steadyMsgs.filter((m) => m.fetch_count === 0).length;
    rec(
      'steady-state',
      turnsSeen === steadyN && duplicated === 0 && unfetched === 0 && multiFetched === 0,
      `${steadyN} messages delivered back-to-back | turns per message=${JSON.stringify(perToken)} ` +
        `(want all 1) | delivered-and-never-fetched=${unfetched} want=0 | ` +
        `fetched-more-than-once=${multiFetched} want=0 | ` +
        `imap.uid_fetch ${fetchesBeforeSteady}->${fetchesAfterSteady} | ` +
        `max_concurrent_imap_sessions=${report && report.ok ? report.max_concurrent_imap_sessions : 'unread'} ` +
        `(2 would mean two pollers competing on one mailbox; 0 would mean nothing polled at all)`,
    );

    this.emailProbe = probe;
    return probe;
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
    const budget = ARRIVAL_BUDGET_BY_ADAPTER[adapter] ?? ARRIVAL_BUDGET_MS;
    const deliver = ({ sender, conversation, text, messageId }) =>
      cfg.inject
        ? cfg.inject({ sender, conversation, text, messageId })
        : post(cfg.build({ url, sender, conversation, text, messageId }));

    this.note(`matrix ${adapter} transport=${TRANSPORT[adapter]} tag=${tag}`);

    // ── admit + route ─────────────────────────────────────────────────────
    const c1 = `f24c3-${adapter}-admit-${tag}`;
    const r1 = deliver({ sender: cfg.allowed, conversation: cfg.conv1, text: `hello ${c1}`, messageId: `${tag}.0001` });
    const originalAt = Date.now();
    const seen1 = this.awaitArrivals(c1, 1, reader, budget);
    this.record(
      adapter,
      'admit',
      seen1.length === 1,
      `POST rc=${r1.status} ${r1.output.split('\n')[0]} | arrivals(journal)=${seen1.length} want=1 | turns(fixture-journal)=${this.turnsFor(c1).length}`,
    );
    // MUST go through the correlate module, not `String.includes`.
    //
    // This exact line survived the first pass of the matcher repair and the
    // live run caught it: `arrivalsFor` was fixed, so telegram/admit counted
    // its arrival correctly, while telegram/route re-tested the SAME arrival
    // with the OLD exact matcher and reported
    // `carries_correlation=false` against
    // `"F24C3\-REPLY f24c3\-telegram\-admit\-67ac190c"` — a reply that had
    // plainly arrived, in the right conversation, carrying the right token.
    //
    // Recorded because it is the sharpest available demonstration of
    // LANE-BRIEF §6b-ii: a PARTIAL instrument repair leaves the defect live at
    // whichever call site was missed, and the missed site fails in exactly the
    // direction that blames the product.
    const routed = seen1.length === 1 && correlationMatches(seen1[0].text, c1);
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
    const beforeTurns = this.turnsFor(c1).length;
    const r2 = deliver({ sender: cfg.allowed, conversation: cfg.conv1, text: `hello ${c1}`, messageId: `${tag}.0001` });
    // How far into the product's dedupe window this replay landed. Recorded
    // BEFORE the settle, because the settle is not part of the window that
    // matters — the entry's TTL runs from when the ORIGINAL was first seen.
    const replayDelayMs = Date.now() - originalAt;
    this.settle(20_000);
    const afterDedupe = this.arrivalsFor(c1, reader).length;
    // Positive control: a DIFFERENT id from the same sender in the same
    // conversation must still get through, or "no second arrival" would be
    // satisfied by an adapter that had simply stopped working.
    const c1b = `f24c3-${adapter}-dedupe-control-${tag}`;
    deliver({ sender: cfg.allowed, conversation: cfg.conv1, text: `hello ${c1b}`, messageId: `${tag}.0002` });
    const control = this.awaitArrivals(c1b, 1, reader, budget);
    const afterTurns = this.turnsFor(c1).length;

    // TIMING GUARD. The product's dedupe entry expires `DEDUPE_TTL_MS` after the
    // original was first seen. A replay that lands after that is SUPPOSED to
    // produce a second turn, so grading it FAIL would write a product defect out
    // of the driver's own latency — which is exactly what happened on the first
    // email run (replay at 90.1s against a 60s TTL, reported FAIL, product
    // correct). Not measurable is not the same as broken.
    if (replayDelayMs >= DEDUPE_TTL_MS) {
      this.recordIncomplete(
        adapter,
        'dedupe',
        `replay landed ${replayDelayMs}ms after the original, which is OUTSIDE the product's ` +
          `${DEDUPE_TTL_MS}ms dedupe TTL (bootstrap.rs:3234). A second delivery here is correct ` +
          `behaviour, so this leg cannot distinguish a dedupe defect from an expired entry. ` +
          `arrivals before=${beforeDedupe} after=${afterDedupe} | turns before=${beforeTurns} after=${afterTurns}`,
      );
    } else {
      this.record(
        adapter,
        'dedupe',
        afterDedupe === beforeDedupe && afterTurns === beforeTurns && control.length === 1,
        `replay rc=${r2.status} at +${replayDelayMs}ms (inside the ${DEDUPE_TTL_MS}ms TTL) | ` +
          `arrivals before=${beforeDedupe} after=${afterDedupe} (want equal) | ` +
          `turns before=${beforeTurns} after=${afterTurns} (want equal) | ` +
          `positive-control fresh-id arrivals=${control.length} want=1`,
      );
    }

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
    const seen4 = this.awaitArrivals(c4, 1, reader, budget);
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

    // ── steady ────────────────────────────────────────────────────────────
    // The leg the other five cannot cover.
    //
    // All five above fire in a continuous burst within the first seconds of the
    // channel's life. F24-C3-H4 was raised from MEDIUM to HIGH exactly because a
    // steady-state run lost 5 of 6 messages while the startup burst looked
    // perfect: a poller that dies, desynchronises its cursor, or loses its
    // subscriber AFTER its first successful exchange passes every startup leg
    // and then drops everything.
    //
    // So: go genuinely quiet for STEADY_QUIET_MS — longer than any adapter's
    // poll interval, so the channel idles rather than merely pausing mid-cycle —
    // then deliver STEADY_COUNT messages spaced STEADY_GAP_MS apart, each with
    // its OWN correlation token so a single swallowed message shows up as a
    // count rather than as a boolean.
    //
    // This leg is inherently self-controlling against the universal-denial trap:
    // it demands arrivals > 0. A path that denies everything scores 0/3 and
    // FAILS, where a negative leg would have "passed".
    this.note(
      `steady ${adapter}: going quiet for ${STEADY_QUIET_MS}ms before ${STEADY_COUNT} ` +
        `messages at ${STEADY_GAP_MS}ms spacing`,
    );
    this.settle(STEADY_QUIET_MS);
    const steadyTokens = [];
    for (let i = 0; i < STEADY_COUNT; i += 1) {
      const c = `f24c3-${adapter}-steady${i}-${tag}`;
      steadyTokens.push(c);
      deliver({
        sender: cfg.allowed,
        conversation: cfg.conv1,
        text: `hello ${c}`,
        messageId: `${tag}.02${i}0`,
      });
      if (i < STEADY_COUNT - 1) this.settle(STEADY_GAP_MS);
    }
    // Wait for the LAST token, then read them all. Waiting per-token would let
    // the first token's budget expire while the third was still in flight and
    // report a loss that was only a wait too short.
    this.awaitArrivals(steadyTokens[STEADY_COUNT - 1], 1, reader, budget);
    const steadySeen = steadyTokens.map((c) => this.arrivalsFor(c, reader).length);
    const steady = gradeSteady(steadySeen, STEADY_COUNT);
    this.record(
      adapter,
      'steady',
      steady.ok,
      `after ${STEADY_QUIET_MS}ms quiet: ${steady.arrived}/${STEADY_COUNT} steady-state messages arrived ` +
        `per-message=${JSON.stringify(steadySeen)} ` +
        `(a startup-only matrix cannot see this class — F24-C3-H4 lost 5 of 6 here while every ` +
        `startup leg passed)`,
    );
  }

  // ── orchestration ─────────────────────────────────────────────────────────

  execute() {
    fs.mkdirSync(this.runDir, { recursive: true });
    fs.rmSync(this.journalPath, { force: true });
    fs.rmSync(this.llmJournalPath, { force: true });
    fs.rmSync(this.tgJournalPath, { force: true });
    fs.rmSync(this.mxJournalPath, { force: true });
    fs.rmSync(this.sigJournalPath, { force: true });
    fs.rmSync(this.sigControlPath, { force: true });
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

    // The Matrix homeserver fixture. Started before the config, which has to
    // name its bound port as `homeserver_url`. Both rooms are declared with two
    // joined members so `sync.rs:328` types them Direct — see the MX constant.
    this.mxUrl = this.startFixture(
      'f24-matrix-fixture.mjs',
      [
        '--journal', this.mxJournalPath,
        '--token', this.mxAccessToken,
        '--room', `${MX.room1}:2`,
        '--room', `${MX.room2}:2`,
        '--max-wait-ms', '2000',
      ],
      /MXFIX_READY url=(\S+)/,
      'matrix.log',
    );
    this.note(`matrix fixture ${this.mxUrl} journal=${this.mxJournalPath}`);

    // The fake signal-cli is INSTALLED, not started. The product spawns it —
    // that is the seam.
    this.installSignalCli();

    // ── mail fixture ──────────────────────────────────────────────────────
    const supported = this.mailFixtureSupported();
    if (!supported.ok) {
      this.recordNotMeasured('email', supported.reason);
    } else {
      const pem = this.mintMailCert();
      if (!pem) {
        this.recordNotMeasured(
          'email',
          'openssl is not available on this host, so no fixture certificate could be minted',
        );
      } else {
        this.mailCert = pem.cert;
        const banner = this.startFixture(
          'f24-mail-fixture.mjs',
          [
            '--journal', this.mailJournalPath,
            '--cert', pem.cert,
            '--key', pem.key,
            '--user', this.mailUser,
            '--pass', this.mailPass,
          ],
          /MAILFIX_READY (imap=\S+ smtp=\S+ control=\S+)/,
          'mail.log',
        );
        const kv = Object.fromEntries(banner.split(' ').map((p) => p.split('=')));
        this.mailPorts = {
          imap: Number(kv.imap),
          smtp: Number(kv.smtp),
          control: Number(kv.control),
        };
        this.note(
          `mail fixture imap=${this.mailPorts.imap} smtp=${this.mailPorts.smtp} ` +
            `control=${this.mailPorts.control} journal=${this.mailJournalPath}`,
        );
      }
    }

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
      this.runMatrixAdapter();
      this.runSignalAdapter();
      this.runEmailMatrix();
      this.runMatrixRestartProbe();
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
    this.runMatrixAdapter();
    this.runSignalAdapter();
    this.runEmailMatrix();

    // LAST, and deliberately so: it stops and restarts the shipped binary, which
    // would disturb every other adapter's legs if it ran earlier.
    this.runMatrixRestartProbe();

    return this.finish(info, digest, healthz);
  }

  /// Talk to the mail fixture's control plane over a raw line-delimited TCP
  /// socket. A child process again, for the same reason `post()` is one: this
  /// driver's waits are blocking, so its own event loop is parked.
  mailControl(line) {
    if (!this.mailPorts) return { status: 1, output: 'no mail fixture' };
    const script = `
      const net = require('node:net');
      const s = net.connect(${this.mailPorts.control}, '127.0.0.1', () => s.write(${JSON.stringify(`${line}\n`)}));
      let buf = '';
      s.setEncoding('utf8');
      s.on('data', (c) => {
        buf += c;
        if (buf.includes('\\n')) { process.stdout.write(buf.trim()); s.end(); }
      });
      s.on('error', (e) => { process.stdout.write('MAILCTL FAILED ' + e.message); process.exit(1); });
    `;
    return run([process.execPath, '-e', script], { timeout: 30_000 });
  }

  mailReport() {
    if (!this.mailPorts) return null;
    const r = this.mailControl('report');
    try {
      return JSON.parse(r.output);
    } catch {
      return { ok: false, raw: r.output, rc: r.status };
    }
  }

  /// Wait until the binary's IMAP poller has SELECTed the mailbox at least once.
  ///
  /// This is not politeness, it is correctness. `imap.rs` seeds its watermark to
  /// `UIDNEXT - 1` on the first connect with no persisted watermark, precisely so
  /// that pre-existing mail is not replayed as new inbound. A message delivered
  /// to the fixture BEFORE that first SELECT would therefore be seeded past and
  /// never fetched — and every email leg would read as zero arrivals for a
  /// reason that has nothing to do with the inbound path.
  waitForImapSeed(budgetSecs = 60) {
    for (let i = 0; i < budgetSecs; i += 1) {
      const selects = this.mailJournal().filter((r) => r.kind === 'imap.select');
      if (selects.length > 0) {
        this.note(
          `imap poller seeded after ${i}s: select#1 exists=${selects[0].exists} ` +
            `uid_next=${selects[0].uid_next} (watermark seeds to uid_next-1)`,
        );
        return selects[0];
      }
      const logins = this.mailJournal().filter((r) => r.kind === 'imap.login');
      process.stdout.write(
        `[inbound] waiting for imap seed: ${i}s logins=${logins.length} ${new Date().toISOString()}\n`,
      );
      sleep(1000);
    }
    this.note('imap poller never SELECTed the mailbox within 60s');
    return null;
  }

  /// The email matrix. Runs only when the mail fixture is up on this host.
  runEmailMatrix() {
    if (!this.mailPorts) {
      // `recordNotMeasured` already fired at fixture-start time with the real
      // reason; do not add a second, vaguer one.
      return;
    }

    const seeded = this.waitForImapSeed();
    if (!seeded) {
      this.recordNotMeasured(
        'email',
        'the binary never completed an IMAP SELECT against the fixture within 60s — see ' +
          'mail.jsonl for whether TLS, LOGIN or SELECT is where it stopped',
      );
      return;
    }

    this.runEmailAdmissionProbe();

    // Only NOW decide how to record the five arrival legs, from what the fixture
    // actually observed rather than from what the lockfile predicts.
    const report = this.mailReport();
    const smtpDelivered = report && report.ok ? report.smtp_delivered_total : null;
    const smtpFailures = report && report.ok ? report.smtp_failures : [];
    if (smtpDelivered === 0 && smtpFailures.length > 0) {
      const err = String(smtpFailures[0].detail ?? '').split('\n')[0];
      this.recordNotMeasured(
        'email',
        `the reply could not leave: ${smtpFailures.length} SMTP session(s) reached the fixture, ` +
          `completed EHLO and STARTTLS, and were refused at certificate verification — "${err}". ` +
          `SMTP is lettre/rustls (Cargo.toml:11, tokio1-rustls-tls) whose resolved deps include ` +
          `webpki-roots and NOT rustls-native-certs; webpki-roots is a compiled-in root set that ` +
          `reads no file and no environment variable, so SSL_CERT_FILE cannot redirect it on ANY ` +
          `platform. The five legs of 24-C3 are defined on arrivals, so they are NOT MEASURED for ` +
          `email — see email_admission_probe for what WAS established about the inbound half.`,
      );
    } else if (smtpDelivered > 0) {
      // The prediction was wrong and the reply DID leave. Say so loudly rather
      // than keeping the NOT MEASURED path that was written for it.
      this.note(
        `UNEXPECTED: ${smtpDelivered} SMTP deliveries succeeded. The webpki-roots analysis is ` +
          `wrong and the email arrival legs should be run for real.`,
      );
      this.recordNotMeasured(
        'email',
        `SMTP unexpectedly delivered ${smtpDelivered} message(s); the arrival legs were not ` +
          `wired for this case and must be re-run rather than inferred`,
      );
    } else {
      this.recordNotMeasured(
        'email',
        `no SMTP delivery and no SMTP failure was observed at the fixture — the reply path was ` +
          `never even attempted, which is a different fact from being refused`,
      );
    }
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

  /// THE MATRIX INBOUND RESTART PROBE — separately named, separately graded.
  ///
  /// NOT ONE OF THE SIX LEGS AND NOT COUNTED AS THEM, for the same reason
  /// `runEmailAdmissionProbe` is separate: the legs are defined uniformly across
  /// every adapter so the columns compare, and this question exists only for
  /// matrix. Folding an adapter-specific probe into the shared leg set would
  /// make the leg count stop reconciling and would make one adapter's row mean
  /// something different from the others'.
  ///
  /// THE QUESTION. Matrix's outbound side had a defect where a transaction id
  /// reused after a restart made the homeserver answer HTTP 200 with the OLD
  /// event id, so a genuinely new message vanished while reporting success.
  /// Nothing had checked whether the INBOUND side has an equivalent. Reading
  /// says it does:
  ///
  ///   sync.rs:190      `let mut since: Option<String> = None;` — a
  ///                    PROCESS-LOCAL variable, never persisted anywhere.
  ///   sync.rs:212-226  events are emitted only when `!is_initial`; the initial
  ///                    sync's timeline is deliberately discarded (the
  ///                    documented "initial-sync replay guard", sync.rs:8-12).
  ///
  /// Composed: a restart resets `since` to `None`, so the first `/sync` after a
  /// restart is an initial sync, so its whole timeline is discarded — including
  /// everything that arrived while the process was down.
  ///
  /// AND IT IS NOT AN UNAVOIDABLE TRADEOFF. The sibling polling adapter in this
  /// same workspace solves exactly this: `wcore-channel-email/src/imap.rs:120`
  /// — "Resume the UID watermark from disk so a restart neither replays the
  /// [backlog] nor [loses the gap]". Matrix implements the replay-guard half and
  /// omits the resume half.
  ///
  /// WHY THIS IS NOT A FABRICATED HIGH. Two hypotheses fit a zero here:
  ///   H1 (product) the restarted process discards the initial sync's timeline;
  ///   H2 (fixture) the fixture never served the gap event in that timeline.
  /// A lane on this program once traced a dedupe FAIL to its own 90s replay
  /// against a 60s TTL — reporting it would have been a fabricated HIGH against
  /// working code. So H2 is excluded by the FIXTURE'S OWN report
  /// (`initial_syncs[].served`), from another process, and a probe that cannot
  /// point at the gap event inside an initial sync grades INCOMPLETE — NOT LOSS.
  ///
  /// Two live positive controls bracket the measurement, because a zero from a
  /// process that never came back up is a different fact from a zero from a
  /// process that came up and dropped the message.
  runMatrixRestartProbe() {
    if (!this.mxUrl) return;
    const tag = crypto.randomBytes(4).toString('hex');
    const probe = [];
    let fault = null;
    const rec = (leg, ok, detail) => {
      probe.push({ leg, ok, detail });
      process.stdout.write(`[inbound] ${ok ? 'PASS' : 'FAIL'} matrix-restart/${leg} — ${detail}\n`);
    };
    const reader = () => this.mxArrivals();
    const evId = (n) => `$f24${tag}${n}`;

    // ── 1. positive control: delivery works BEFORE the restart ────────────
    const cPre = `f24c3-matrix-restartpre-${tag}`;
    this.mxCommand('/__control/submit', {
      room: MX.room1,
      sender: MX.allowed,
      text: `hello ${cPre}`,
      eventId: evId('pre'),
    });
    const seenPre = this.awaitArrivals(cPre, 1, reader, ARRIVAL_BUDGET_MS);
    rec(
      'pre-restart-live-control',
      seenPre.length === 1,
      `arrivals=${seenPre.length} want=1 — establishes that this channel DOES receive on this ` +
        `run, so a zero after the restart cannot be explained by the channel never having worked`,
    );

    // ── 2. stop the binary and WAIT for it to be gone ─────────────────────
    const syncsBeforeStop = (this.mxReport()?.sync_total) ?? null;
    const stop = this.stopBinary();
    // Let any in-flight long-poll unwind before the gap message is injected. A
    // gap message consumed by the dying incarnation would produce a PASS that
    // proves the opposite of what this probe asks.
    this.settle(5_000);
    const syncsAfterStop = (this.mxReport()?.sync_total) ?? null;

    // ── 3. the GAP message, delivered while the process is down ───────────
    const cGap = `f24c3-matrix-restartgap-${tag}`;
    const gapSubmit = this.mxCommand('/__control/submit', {
      room: MX.room1,
      sender: MX.allowed,
      text: `hello ${cGap}`,
      eventId: evId('gap'),
    });
    this.note(`gap message injected while the binary was down: ${JSON.stringify(gapSubmit)}`);

    // Prove nothing polled during the gap. If a sync landed here, the message
    // was not delivered "while down" at all and the probe is measuring
    // something else.
    this.settle(5_000);
    const syncsAfterGap = (this.mxReport()?.sync_total) ?? null;
    const quietDuringGap =
      syncsAfterStop !== null && syncsAfterGap !== null && syncsAfterGap === syncsAfterStop;
    rec(
      'binary-down-and-quiet-during-the-gap',
      stop.stopped && quietDuringGap,
      `stopped=${stop.stopped} pid=${stop.pid} exit_secs=${stop.secs}${stop.sigkill ? ' (SIGKILL)' : ''} | ` +
        `fixture sync_total before_stop=${syncsBeforeStop} after_stop=${syncsAfterStop} ` +
        `after_gap=${syncsAfterGap} (want after_gap == after_stop: nothing polled while down)`,
    );

    // ── 4. restart, and wait for the adapter to actually sync again ───────
    //
    // Keyed on `sync_total`, NOT `initial_sync_total`. The old form waited for
    // an INITIAL sync, which a correctly-resuming adapter never issues after a
    // restart — so on fixed code it burned its full 90s budget and then failed
    // the liveness leg for the one reason that means the fix worked. `sync_total`
    // increments on every /sync of either kind, so this leg means "the restarted
    // process reached the homeserver" on both states of the product.
    const initialsBefore = (this.mxReport()?.initial_sync_total) ?? 0;
    const syncsBeforeRestart = (this.mxReport()?.sync_total) ?? 0;
    this.startBinary('core-restarted');
    let report = null;
    let cameBack = false;
    for (let i = 0; i < 90; i += 1) {
      report = this.mxReport();
      if (report && report.ok && report.sync_total > syncsBeforeRestart) {
        cameBack = true;
        this.note(
          `restarted binary issued a /sync after ${i}s ` +
            `(sync_total ${syncsBeforeRestart} -> ${report.sync_total}, ` +
            `initial_sync_total ${initialsBefore} -> ${report.initial_sync_total})`,
        );
        break;
      }
      process.stdout.write(
        `[inbound] waiting for the restarted binary to /sync: ${i}s ` +
          `syncs=${report?.sync_total ?? '?'} ${new Date().toISOString()}\n`,
      );
      sleep(1000);
    }
    rec(
      'restarted-binary-resyncs',
      cameBack,
      `sync_total ${syncsBeforeRestart} -> ${report?.sync_total ?? 'UNREADABLE'} ` +
        `(want an increase: the restarted process must reach the homeserver at all). ` +
        `initial_sync_total ${initialsBefore} -> ${report?.initial_sync_total ?? '?'} — an ` +
        `increase here means it re-seeded; no increase means it resumed a persisted cursor`,
    );

    // ── 5. THE H2 EXCLUSION — did the fixture SERVE the gap event? ────────
    // Read from the fixture's own report, in another process, listing exactly
    // which event ids every post-restart sync carried, and on which kind.
    // Deliberately NOT restricted to initial syncs: see `gradeRestart`. That
    // restriction made a PASS unreachable on a fixed adapter.
    const servedProbe = servedAfterRestartFrom(report?.syncs, syncsBeforeRestart, evId('gap'));
    const servedAfterRestart = servedProbe.served;
    rec(
      'gap-event-was-served-to-the-restarted-process',
      servedAfterRestart,
      `the fixture answered ${servedProbe.syncs_after_restart} sync(s) after the restart; ` +
        `they carried ${JSON.stringify(servedProbe.served_lists)}; ` +
        `looking for ${evId('gap')} -> ${servedAfterRestart} (on a ${servedProbe.where ?? 'n/a'} sync). ` +
        `THIS IS THE H2 EXCLUSION: without it a zero below could mean the fixture never ` +
        `served the message, which is a harness fault and not product loss. ` +
        `where=incremental means the adapter resumed and ASKED for the window it missed; ` +
        `where=initial means it was offered the window on a sync whose timeline it discards`,
    );
    if (!servedAfterRestart) {
      fault =
        `the fixture did not serve ${evId('gap')} on any sync after the restart, so ` +
        `"the gap message did not arrive" cannot be attributed to the product. Graded ` +
        `INCOMPLETE rather than LOSS.`;
    }

    // ── 6. positive control: the RESTARTED process receives normally ──────
    const cPost = `f24c3-matrix-restartpost-${tag}`;
    this.mxCommand('/__control/submit', {
      room: MX.room1,
      sender: MX.allowed,
      text: `hello ${cPost}`,
      eventId: evId('post'),
    });
    const seenPost = this.awaitArrivals(cPost, 1, reader, ARRIVAL_BUDGET_MS);
    rec(
      'post-restart-live-control',
      seenPost.length === 1,
      `arrivals=${seenPost.length} want=1 — a message sent AFTER the restart. Without this, a ` +
        `process that came up broken would look identical to one that dropped only the gap`,
    );

    // ── 7. THE LEG ────────────────────────────────────────────────────────
    const seenGap = this.arrivalsFor(cGap, reader);
    const verdict = gradeRestart({
      preArrivals: seenPre.length,
      postArrivals: seenPost.length,
      servedAfterRestart,
      gapArrivals: seenGap.length,
    });
    const graded = verdict.graded;
    if (!graded) {
      rec(
        'gap-message-survives-the-restart',
        false,
        `NOT GRADED — a control did not hold (pre=${seenPre.length} post=${seenPost.length} ` +
          `served_after_restart=${servedAfterRestart}). arrivals for the gap message=${seenGap.length}. ` +
          `${fault ?? 'A zero here is not attributable to the product while a control is down.'}`,
      );
    } else {
      rec(
        'gap-message-survives-the-restart',
        verdict.ok,
        `verdict=${verdict.state} arrivals=${seenGap.length} want>=1 for a message delivered to the ` +
          `homeserver while the binary was down and served back to the restarted process. ` +
          `CONTROLS HELD: pre-restart delivery=${seenPre.length}, post-restart delivery=` +
          `${seenPost.length}, fixture served the event after the restart=${servedAfterRestart} ` +
          `(on a ${servedProbe.where ?? 'n/a'} sync). ` +
          `A zero here is silent inbound loss across a restart: the cursor did not survive the ` +
          `process, so the first /sync after the restart was an initial sync whose timeline the ` +
          `replay guard discards — exactly the window that was missed.`,
      );
    }

    this.matrixRestartProbe = {
      verdict: verdict.state,
      graded,
      instrument_fault: fault,
      gap_event_id: evId('gap'),
      gap_arrivals: seenGap.length,
      served_after_restart: servedProbe,
      // The mechanism in one field: 'incremental' means the adapter resumed a
      // persisted cursor and asked for the window it missed; 'initial' means it
      // re-seeded and was handed that window on a timeline it discards.
      gap_served_on: servedProbe.where,
      initial_syncs_after_restart: (report?.initial_syncs ?? []).slice(initialsBefore),
      legs: probe,
    };
    return this.matrixRestartProbe;
  }

  /// The Matrix matrix. Room-keyed, so the bind leg's second conversation is a
  /// second ROOM from the SAME sender — the Slack shape, not the peer-keyed one.
  runMatrixAdapter() {
    if (!this.mxUrl) {
      this.recordNotMeasured('matrix', 'the matrix homeserver fixture did not start');
      return;
    }
    this.runMatrix('matrix', {
      channelName: 'f24c3matrix',
      allowed: MX.allowed,
      denied: MX.denied,
      conv1: MX.room1,
      conv2: MX.room2,
      expectConversation: MX.room1,
      expectConversation2: MX.room2,
      inject: ({ sender, conversation, text, messageId }) =>
        this.mxSubmit({ sender, conversation, text, messageId }),
    });
  }

  /// Inject an inbound event into the fixture homeserver. The binary then has to
  /// come and get it on its next `/sync`.
  ///
  /// `messageId` becomes the Matrix `event_id`, which is what `sync.rs:361`
  /// binds the message id to and therefore what the inbound dedupe cache keys
  /// on. A replay under the same `event_id` is the real shape of a Matrix
  /// redelivery: the event is re-served in a later sync batch under a NEW
  /// `next_batch` cursor, carrying the SAME event id.
  mxSubmit({ sender, conversation, text, messageId }) {
    const r = this.mxCommand('/__control/submit', {
      room: conversation,
      sender,
      text,
      eventId: `$f24${String(messageId).replace('.', '')}`,
    });
    return { status: r.ok ? 0 : 1, output: JSON.stringify(r) };
  }

  /// The Signal matrix. Peer-keyed, like whatsapp/sms/telegram.
  runSignalAdapter() {
    const ctl = this.sigControl();
    if (!ctl) {
      // The product never spawned the executable, or it never published a port.
      // NOT a zero and NOT a fail: the driver could not ask the question.
      const spawns = this.sigJournal().filter((r) => r.kind === 'spawn');
      this.recordNotMeasured(
        'signal',
        `the fake signal-cli never published a control port within 60s — spawn records in its ` +
          `journal=${spawns.length} (0 means the product never invoked ` +
          `${this.sigCliPath}; >0 means it spawned and then failed to bind). ` +
          `journal bytes=${fs.existsSync(this.sigJournalPath) ? fs.statSync(this.sigJournalPath).size : 'ABSENT'}`,
      );
      return;
    }
    this.note(`signal-cli fixture reached: control port=${ctl.port} pid=${ctl.pid}`);
    this.runMatrix('signal', {
      channelName: 'f24c3signal',
      allowed: SIG.allowed,
      denied: SIG.denied,
      conv1: SIG.allowed,
      conv2: SIG.second,
      convDenied: SIG.denied,
      secondSender: SIG.second,
      expectConversation: SIG.allowed,
      expectConversation2: SIG.second,
      inject: ({ sender, text, messageId }) => this.sigSubmit({ sender, text, messageId }),
    });
  }

  /// Hand a message to the fake signal-cli, which emits it as a JSON-RPC
  /// `receive` notification on the stdout the product is reading.
  ///
  /// The `timestamp` is derived deterministically from `messageId` and is
  /// LOAD-BEARING: `subprocess.rs:277` sets the message id to
  /// `format!("{ts_ms}")` from the envelope timestamp, and the inbound dedupe
  /// cache keys on that id. A fixture that stamped `Date.now()` itself would
  /// make every replay a fresh message and the dedupe leg would measure nothing.
  sigSubmit({ sender, text, messageId }) {
    const suffix = Number.parseInt(String(messageId).split('.').pop(), 10);
    // A plausible ms-epoch value that is stable per messageId. Distinct ids
    // therefore get distinct timestamps and a replay gets the identical one.
    const timestamp = 1_700_000_000_000 + suffix * 1000;
    const r = this.sigCommand({
      op: 'submit',
      account: SIG.account,
      source: sender,
      sourceName: `u${String(sender).replace(/\D/g, '')}`,
      text,
      timestamp,
    });
    return { status: r.status, output: r.output };
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
    const mailReport = this.mailReport();
    const mxReport = this.mxReport();
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
        mail: fs.existsSync(this.mailJournalPath) ? fs.statSync(this.mailJournalPath).size : null,
        matrix: fs.existsSync(this.mxJournalPath) ? fs.statSync(this.mxJournalPath).size : null,
        signal: fs.existsSync(this.sigJournalPath) ? fs.statSync(this.sigJournalPath).size : null,
        core_log: this.coreLog && fs.existsSync(this.coreLog) ? fs.statSync(this.coreLog).size : null,
      },
      telegram_journal: this.tgJournalPath,
      telegram_arrivals_total: this.tgArrivals().length,
      // The independent, out-of-process observable for F24-C3-H4.
      telegram_fixture_report: tgReport,
      mail_journal: this.mailJournalPath,
      mail_arrivals_total: this.mailArrivals().length,
      mail_fixture_report: mailReport,
      mail_ssl_cert_file: this.mailCert ?? null,
      matrix_journal: this.mxJournalPath,
      matrix_arrivals_total: this.mxArrivals().length,
      // The fixture homeserver's independent report, including `initial_syncs`
      // — which initial sync served which event ids. That list is the H2
      // exclusion for the restart probe and is counted in another process.
      matrix_fixture_report: mxReport,
      signal_journal: this.sigJournalPath,
      signal_arrivals_total: this.sigArrivals().length,
      // Signal's fixture has no HTTP report: every observable is in its journal,
      // because `supervisor.rs` respawns the executable and a report served from
      // one incarnation's memory would omit whatever a prior one saw. Each
      // record carries the pid that wrote it.
      signal_spawns: this.sigJournal()
        .filter((r) => r.kind === 'spawn')
        .map((r) => ({ pid: r.pid, at: r.at, argv: r.argv })),
      signal_cli_path: this.sigCliPath,
      // Deliberately a SEPARATE key from `results`. This is not the six legs.
      email_admission_probe: this.emailProbe ?? null,
      // Also deliberately separate, and for the same reason: the legs are
      // uniform across adapters so the columns compare, and this question exists
      // only for matrix.
      matrix_restart_probe: this.matrixRestartProbe ?? null,
      // Legs the driver could not ask about on this host, with the reason.
      // NOT failures — see `recordNotMeasured`.
      not_measured: this.notMeasured,
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
  // A NOT-MEASURED leg is accounted for, but it is not a leg that ran. The
  // total must reconcile or the driver is losing legs silently.
  const accounted = result.results.length + (result.not_measured ?? []).length;
  const ranEverything = accounted === expectedLegs && (result.not_measured ?? []).length === 0;
  // Three outcomes, not two. INCOMPLETE is what an instrument fault produces:
  // a token reached the journal in a form this driver cannot decode, so the
  // numbers are untrustworthy in BOTH directions and the run must not be read
  // as either a clean green or an honest red.
  // THE RESTART PROBE MUST BE ABLE TO TURN THE RUN RED.
  //
  // INSTRUMENT DEFECT FOUND BY RUNNING, AND REPAIRED HERE (LANE-BRIEF §6b-ii).
  // The first live run graded the restart probe LOSS — a genuine product
  // finding — while `failed.length` stayed 0, because the probe is recorded
  // outside `results` (as `email_admission_probe` is). That run happened to
  // exit RED anyway, for an unrelated reason: email's six legs were NOT
  // MEASURED. So the gate LOOKED correct while being incapable of failing on
  // the thing it had just found. The moment email becomes measurable, a silent
  // inbound loss across a restart would have exited 0 GREEN.
  //
  // That is exactly the self-passing-gate class in §3.2, and noting it without
  // fixing the instrument is how the one measured recurrence in this program
  // happened. Both probes now count.
  const restart = result.matrix_restart_probe;
  const restartLoss = restart ? restart.verdict === 'LOSS' : false;
  const restartIncomplete = restart ? restart.verdict === 'INCOMPLETE' : false;
  const probeFailed = (result.email_admission_probe ?? []).some((p) => !p.ok) || restartLoss;
  const verdict = result.instrument_fault || restartIncomplete
    ? 'INCOMPLETE'
    : failed.length === 0 && ranEverything && !probeFailed
      ? 'GREEN'
      : 'RED';
  process.stdout.write(
    `\nINBOUND MATRIX ${verdict} platform=${result.platform} ` +
      `runtime=${result.runtime} ` +
      `legs=${result.results.length}/${expectedLegs} failed=${failed.length} ` +
      `not_measured=${(result.not_measured ?? []).length} accounted=${accounted}/${expectedLegs} ` +
      `probe_failed=${probeFailed} restart_verdict=${restart?.verdict ?? 'n/a'} ` +
      `arrivals_total=${result.arrivals_total} telegram_arrivals=${result.telegram_arrivals_total} ` +
      `mail_arrivals=${result.mail_arrivals_total} matrix_arrivals=${result.matrix_arrivals_total} ` +
      `signal_arrivals=${result.signal_arrivals_total} ` +
      `turns_total=${result.turns_total} instrument_fault=${result.instrument_fault}\n`,
  );
  // Byte counts printed, not just recorded. An empty journal and an absent
  // journal both read as "0 arrivals" if only parsed records are counted, and
  // this lane's brief calls that out by name.
  process.stdout.write(`  journal bytes: ${JSON.stringify(result.journal_bytes)}\n`);
  if (result.matrix_fixture_report && result.matrix_fixture_report.ok) {
    const mx = result.matrix_fixture_report;
    process.stdout.write(
      `  matrix fixture: events=${mx.submitted_total} syncs=${mx.sync_total} ` +
        `initial_syncs=${mx.initial_sync_total} max_concurrent_sync=${mx.max_concurrent_sync} ` +
        `replies=${mx.replies.length}\n`,
    );
  }
  process.stdout.write(
    `  signal fixture: spawns=${(result.signal_spawns ?? []).length} ` +
      `${JSON.stringify((result.signal_spawns ?? []).map((s) => s.pid))} path=${result.signal_cli_path}\n`,
  );
  for (const p of result.matrix_restart_probe?.legs ?? []) {
    process.stdout.write(`  ${p.ok ? 'PASS' : 'FAIL'} matrix-restart/${p.leg}: ${p.detail}\n`);
  }
  if (result.matrix_restart_probe) {
    process.stdout.write(
      `  MATRIX-RESTART VERDICT=${result.matrix_restart_probe.verdict} ` +
        `gap_event=${result.matrix_restart_probe.gap_event_id} ` +
        `gap_arrivals=${result.matrix_restart_probe.gap_arrivals}\n`,
    );
  }
  if (result.matrix_restart_probe?.instrument_fault) {
    process.stdout.write(
      `  MATRIX-RESTART INCOMPLETE: ${result.matrix_restart_probe.instrument_fault}\n`,
    );
  }
  if (result.mail_fixture_report && result.mail_fixture_report.ok) {
    const m = result.mail_fixture_report;
    process.stdout.write(
      `  mail fixture: mailbox=${m.mailbox_total} imap_sessions=${m.imap_session_total} ` +
        `max_concurrent_imap=${m.max_concurrent_imap_sessions} ` +
        `smtp_delivered=${m.smtp_delivered_total} smtp_failures=${m.smtp_failures.length}\n`,
    );
    for (const msg of m.messages) {
      process.stdout.write(
        `    uid=${msg.uid} from=${msg.from} fetches=${msg.fetch_count} seen_by=${msg.seen_by}\n`,
      );
    }
  }
  for (const p of result.email_admission_probe ?? []) {
    process.stdout.write(`  ${p.ok ? 'PASS' : 'FAIL'} email-admission/${p.leg}: ${p.detail}\n`);
  }
  for (const n of result.not_measured ?? []) {
    process.stdout.write(`  NOT-MEASURED ${n.adapter}/${n.leg}: ${n.reason}\n`);
  }
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

export {
  InboundMatrix,
  pidIsLive,
  slackRequest,
  whatsappRequest,
  twilioRequest,
  STEADY_QUIET_MS,
  STEADY_COUNT,
  STEADY_GAP_MS,
};
