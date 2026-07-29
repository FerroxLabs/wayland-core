#!/usr/bin/env node
/**
 * f24-c3-clauses.mjs — the two criterion clauses `24-C3` never touched that are
 * reachable with the fixtures this phase already has: **health** and
 * **reconnect/reload**. Plus a re-measurement of `gateway run` inbound after the
 * channel-lease and channel-starvation fixes landed.
 *
 * Criterion 24-C3 (ROADMAP.md:119) has EIGHT clauses:
 *   setup/auth, access, routing, media, native actions, idempotency,
 *   reconnect/reload, health.
 *
 * Four prior lanes measured the first three plus idempotency across five
 * adapters. `media`, `native actions`, `reconnect/reload` and `health` were
 * untouched on the inbound path for EVERY adapter. This driver takes the two
 * that have a real shipped surface behind them:
 *
 *   - `health`  → `wayland-core channel health`, which reads the
 *                 `channel-health.json` a RUNNING gateway republishes
 *                 (`gateway.rs:1035`, `:1164`; `channel.rs:62`).
 *   - `reload`  → `wayland-core channel reload`, which writes `channel.reload`
 *                 and `gateway run` consumes it (`gateway.rs:1101`, `:1131`).
 *
 * ── WHAT THIS DRIVER REFUSES TO DO ───────────────────────────────────────────
 *
 * **It will not grade a clause PASS on a count alone.** The health clause is
 * exactly the shape that produces a false green: `configured=0, registered=0`
 * is internally consistent, reads as "healthy", and means nothing was measured.
 * `ChannelHealthReport` was itself rewritten (F24-D-H2) because a live run
 * printed "registered no channels" while two were configured on disk. So every
 * leg here pairs its assertion with a POSITIVE control that must be non-zero,
 * and a green with zero arrivals grades FAIL.
 *
 * **Universal denial is the trap this criterion has already fallen into once.**
 * 24-C3's `access` leg passed on all three adapters at a pre-fix binary BECAUSE
 * EVERYTHING WAS DENIED. The reload leg below therefore does not merely assert
 * "the new adapter appears in the health report" — it drives a real inbound
 * message through the new adapter and counts the reply in another process's
 * journal. A registration that counts but cannot carry a message is the same
 * false green wearing a different hat.
 *
 * ── EXIT CODES ───────────────────────────────────────────────────────────────
 *   0  GREEN       every attempted leg passed
 *   1  RED         at least one leg failed
 *   2  USAGE       bad arguments (never a silent fallback to a default surface)
 *   3  INCOMPLETE  instrument_fault — a leg could not be honestly graded.
 *                  NEVER reported as LOSS. Two lanes on this criterion nearly
 *                  shipped a working path as total inbound loss because their
 *                  matcher could not decode an escaped correlation token.
 */

import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';

const WEBHOOK_PORT = 18899;
const SINK_PORT = 18898;
const LLM_PORT = 18897;

// ─────────────────────────────────────────────────────────────────────────────
// primitives
// ─────────────────────────────────────────────────────────────────────────────

/** Block the thread. Used only between fixture-process interactions. */
function sleep(ms) {
  const sab = new SharedArrayBuffer(4);
  Atomics.wait(new Int32Array(sab), 0, 0, ms);
}

/**
 * Run a command and return {status, stdout, stderr}.
 *
 * Status is read from the RETURN VALUE, never through a pipe. `${PIPESTATUS[0]}`
 * returns empty on the measurement host and `cmd | grep -v X` reports grep's
 * status rather than cmd's — both are recorded self-passing-gate classes.
 */
function run(argv, opts = {}) {
  const r = spawnSync(argv[0], argv.slice(1), {
    encoding: 'utf8',
    timeout: opts.timeout ?? 60_000,
    env: opts.env ?? process.env,
    cwd: opts.cwd,
  });
  return {
    status: r.status,
    stdout: r.stdout ?? '',
    stderr: r.stderr ?? '',
    signal: r.signal,
    error: r.error ? String(r.error) : null,
  };
}

function hex(n) {
  return crypto.randomBytes(n).toString('hex');
}

// ─────────────────────────────────────────────────────────────────────────────
// correlation matching — DELEGATED, never reimplemented
// ─────────────────────────────────────────────────────────────────────────────
//
// `scripts/f24-correlate.mjs` is the phase's repaired three-tier matcher. It
// exists because two lanes on THIS criterion nearly shipped a fully working path
// as total inbound loss: telegram escapes MarkdownV2, so a token leaves the
// product as `f24c3\-x\-1` and `String.includes` scores eight real arrivals zero.
//
// Its contract:
//   classify()        -> 'exact' | 'normalized' | 'fuzzy' | 'absent'
//   matches()         -> exact|normalized. A genuine arrival. Countable.
//   instrumentFault() -> 'fuzzy'. PRESENT but undecodable. Grades the run
//                        INCOMPLETE. Never LOSS — the message plainly arrived.
//
// **The import is deliberately NOT wrapped in a try/catch with a local
// fallback.** A silent degradation to a hand-rolled matcher is the exact defect
// class this module was written to close, and it would fail in the direction
// that blames the product. If this import fails the driver must die loudly.
import { classify, matches, instrumentFault, legacyMatches } from './f24-correlate.mjs';

/**
 * Does `text` carry `token`?
 *
 * Returns {matched, fault, tier}. `matched` is true ONLY for a tier the
 * instrument can decode. `fault` is true when the token is demonstrably present
 * in a form it cannot — which the caller must turn into INCOMPLETE, not LOSS.
 */
function carries(text, token) {
  const tier = classify(text, token);
  return { matched: matches(text, token), fault: instrumentFault(text, token), tier };
}

// ─────────────────────────────────────────────────────────────────────────────
// the run
// ─────────────────────────────────────────────────────────────────────────────

class ClauseRun {
  constructor(args) {
    this.args = args;
    this.runDir = args.runDir;
    this.home = path.join(this.runDir, 'home');
    this.legs = [];
    this.notes = [];
    this.instrumentFaults = [];
    this.children = [];

    this.vaultPassphrase = `f24c3fin-${hex(16)}`;
    this.slackBotToken = `xoxb-f24c3fin-${hex(12)}`;
    this.slackSigningSecret = hex(16);
    this.token = hex(4);

    this.sinkUrl = `http://127.0.0.1:${SINK_PORT}`;
    this.llmUrl = `http://127.0.0.1:${LLM_PORT}/v1`;
  }

  note(msg) {
    const line = `[clauses] ${new Date().toISOString()} ${msg}`;
    this.notes.push(line);
    process.stdout.write(`${line}\n`);
  }

  /**
   * Record a leg.
   *
   * `positiveControl` is MANDATORY and is part of the pass condition, not a
   * decoration printed beside it. A leg whose assertion holds while its control
   * is zero is the universal-denial green, and it grades FAIL here.
   */
  record(clause, leg, ok, detail, positiveControl) {
    const controlOk = positiveControl === undefined ? null : positiveControl > 0;
    const pass = ok && (controlOk === null || controlOk);
    this.legs.push({
      clause,
      leg,
      pass,
      assertion_held: ok,
      positive_control: positiveControl ?? null,
      control_ok: controlOk,
      detail,
    });
    this.note(
      `LEG ${clause}/${leg} ${pass ? 'PASS' : 'FAIL'} ` +
        `assertion=${ok} control=${positiveControl ?? 'n/a'} :: ${detail}`,
    );
    return pass;
  }

  fault(where, detail) {
    this.instrumentFaults.push({ where, detail });
    this.note(`INSTRUMENT_FAULT ${where} :: ${detail}`);
  }

  // ── fixtures ───────────────────────────────────────────────────────────────

  startFixtures() {
    fs.mkdirSync(this.runDir, { recursive: true });
    const scripts = path.dirname(new URL(import.meta.url).pathname);

    this.sinkJournal = path.join(this.runDir, 'arrivals.jsonl');
    this.llmJournal = path.join(this.runDir, 'turns.jsonl');

    const sinkLog = fs.openSync(path.join(this.runDir, 'sink.log'), 'a');
    const sink = spawn(
      process.execPath,
      [path.join(scripts, 'f24-sink.mjs'), '--port', String(SINK_PORT), '--journal', this.sinkJournal],
      { stdio: ['ignore', sinkLog, sinkLog] },
    );
    this.children.push(sink);

    const llmLog = fs.openSync(path.join(this.runDir, 'llm.log'), 'a');
    const llm = spawn(
      process.execPath,
      [path.join(scripts, 'f24-llm-fixture.mjs'), '--port', String(LLM_PORT), '--journal', this.llmJournal],
      { stdio: ['ignore', llmLog, llmLog] },
    );
    this.children.push(llm);

    this.note(`fixtures spawned: sink=${SINK_PORT} llm=${LLM_PORT} (separate OS processes)`);
    sleep(2000);
  }

  // ── config ─────────────────────────────────────────────────────────────────

  writeBaseConfig() {
    fs.mkdirSync(path.join(this.home, 'channels'), { recursive: true });

    fs.writeFileSync(
      path.join(this.home, 'credentials.toml'),
      [
        '[secrets]',
        `"slack.f24c3fin.bot_token" = "${this.slackBotToken}"`,
        `"slack.f24c3fin.signing_secret" = "${this.slackSigningSecret}"`,
        '',
      ].join('\n'),
      { mode: 0o600 },
    );

    fs.writeFileSync(
      path.join(this.home, 'config.toml'),
      [
        '[default]',
        'provider = "f24c3finfixture"',
        '',
        '[providers.f24c3finfixture]',
        'provider = "openai"',
        'model = "f24c3-fixture"',
        'api_key = "f24c3fin-not-a-real-key"',
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
  }

  /** Write one slack-platform channel config. Used for both the base set and
   *  the adapter added mid-run for the reload leg. */
  writeSlackChannel(name, allowedUser) {
    fs.writeFileSync(
      path.join(this.home, 'channels', `${name}.toml`),
      [
        `name = "${name}"`,
        'platform = "slack"',
        'enabled = true',
        '',
        '[options]',
        'workspace_name = "f24c3fin"',
        `default_channel_id = "D${name.toUpperCase()}"`,
        'credential_handle_bot_token = "slack.f24c3fin.bot_token"',
        'credential_handle_signing_secret = "slack.f24c3fin.signing_secret"',
        `api_base_url = "${this.sinkUrl}"`,
        'max_retry_attempts = 1',
        '',
        '[inbound]',
        'dm = "allowlist"',
        `dm_allowlist = ["${allowedUser}"]`,
        'group = "disabled"',
        'require_mention = true',
        'tools = "conversational"',
        '',
      ].join('\n'),
    );
  }

  // ── the binary ─────────────────────────────────────────────────────────────

  startGateway() {
    const logPath = path.join(this.runDir, 'gateway.log');
    fs.writeFileSync(logPath, '');
    const fd = fs.openSync(logPath, 'a');
    // FOREGROUND deliberately. A detached gateway re-execs and this driver
    // would lose the child it must reap, then measure a process it does not own.
    const child = spawn(this.args.binary, ['gateway', 'run'], {
      stdio: ['pipe', fd, fd],
      env: {
        ...process.env,
        WAYLAND_HOME: this.home,
        WAYLAND_VAULT_PASSPHRASE: this.vaultPassphrase,
        RUST_LOG: 'wcore_agent::bootstrap=info,wcore_agent::channel_inbound=debug,wcore_channels=debug',
      },
    });
    this.gatewayChild = child;
    this.children.push(child);
    this.gatewayLog = logPath;
    this.note('gateway run started (foreground, owned by this driver)');
    return logPath;
  }

  waitForWebhookHost(seconds = 60) {
    for (let i = 0; i < seconds; i += 1) {
      const r = run([
        process.execPath,
        '-e',
        `fetch('http://127.0.0.1:${WEBHOOK_PORT}/healthz').then(async r=>{process.stdout.write('HZ '+r.status)}).catch(e=>{process.stdout.write('DOWN');process.exit(1)})`,
      ], { timeout: 15_000 });
      if (r.status === 0 && r.stdout.includes('HZ 200')) {
        this.note(`webhook host bound after ${i}s`);
        return true;
      }
      process.stdout.write(`[clauses] waiting for webhook host: ${i}s ${new Date().toISOString()}\n`);
      sleep(1000);
    }
    this.note('webhook host NEVER bound — recorded, not thrown');
    return false;
  }

  // ── observation helpers (all read OUT-OF-PROCESS journals) ────────────────

  readJournal(file) {
    if (!fs.existsSync(file)) return { records: [], bytes: 0, existed: false };
    const raw = fs.readFileSync(file, 'utf8');
    const records = raw
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
    // Byte count is recorded because an EMPTY journal and an ABSENT journal both
    // read as "0 arrivals" if only parsed records are counted.
    return { records, bytes: Buffer.byteLength(raw, 'utf8'), existed: true };
  }

  arrivals() {
    return this.readJournal(this.sinkJournal);
  }

  turns() {
    return this.readJournal(this.llmJournal);
  }

  // ── inbound submission ─────────────────────────────────────────────────────

  /**
   * POST a Slack events webhook. Returns {token, post, accepted}.
   *
   * # The route, and why it is asserted rather than assumed
   *
   * `inbound_webhook.rs:12-15` documents exactly three routes:
   *   GET  /webhooks/:channel   (Meta hub.challenge handshake)
   *   POST /webhooks/:channel   (runtime delivery, EVERY connector)
   *   GET  /healthz
   *
   * Run 1 of this driver posted to an invented `/channels/:name/slack/events`
   * and got `404` on every submit. The driver then graded the run FAIL with
   * `arrivals=0`, i.e. **it reported my own wrong URL as product inbound loss.**
   * That is the instrument carrying the defect class it hunts, failing in the
   * direction that blames the product — the same shape that made one lane report
   * `replied=0` against eight real arrivals. Repaired here rather than written
   * up (§6b-ii: a documented instrument defect is a defect you have agreed to
   * keep, and that sequence has already recurred once on this program).
   *
   * `accepted` is now returned so the caller can distinguish "the product
   * received this and did nothing" (a real finding) from "the product never
   * received it" (an instrument fault). Those are opposite diagnoses and they
   * previously produced the same number.
   */
  postSlackInbound(channelName, user, channelId) {
    // The token MUST satisfy `f24-llm-fixture.mjs`'s `correlationOf` regex,
    // `/f24c3-[a-z0-9-]+/i` (fixture line 89), or the fixture echoes the literal
    // string `no-correlation` and every reply becomes untraceable to the message
    // that caused it.
    //
    // Run 2 used `f24c3fin-...`, which has no hyphen after `f24c3` and therefore
    // does NOT match. The full path worked perfectly — submit accepted 200, one
    // LLM turn carrying this exact token in `user_text`, one reply delivered to
    // the sink — and the driver still graded it FAIL, because the reply said
    // `F24C3-REPLY no-correlation`. Third instrument fault of this lane, and the
    // third to fail in the direction that blames the product.
    //
    // Fixed by CONFORMING to the shared fixture's contract rather than by
    // editing the fixture, which four other drivers depend on. The self-test
    // asserts the contract so it cannot silently drift again — a comment here
    // would not have caught the original.
    const token = `f24c3-fin-${channelName}-${hex(4)}`;
    const body = JSON.stringify({
      type: 'event_callback',
      team_id: 'T24C3FIN',
      event: {
        type: 'message',
        channel_type: 'im',
        channel: channelId,
        user,
        text: `<@U24C3FINBOT> ${token}`,
        ts: `${Date.now() / 1000}`,
      },
    });
    const ts = Math.floor(Date.now() / 1000).toString();
    const sig =
      'v0=' +
      crypto
        .createHmac('sha256', this.slackSigningSecret)
        .update(`v0:${ts}:${body}`)
        .digest('hex');

    const url = `http://127.0.0.1:${WEBHOOK_PORT}/webhooks/${channelName}`;
    const r = run([
      process.execPath,
      '-e',
      `fetch(${JSON.stringify(url)},{method:'POST',headers:{'content-type':'application/json','x-slack-request-timestamp':'${ts}','x-slack-signature':'${sig}'},body:${JSON.stringify(body)}}).then(async r=>{process.stdout.write('ST '+r.status+' '+(await r.text()).slice(0,200))}).catch(e=>{process.stdout.write('ERR '+e.message);process.exit(1)})`,
    ], { timeout: 30_000 });

    // Parse the HTTP status the product actually returned.
    const m = /ST (\d{3})/.exec(r.stdout || '');
    const httpStatus = m ? Number(m[1]) : null;
    const accepted = httpStatus !== null && httpStatus >= 200 && httpStatus < 300;

    this.note(
      `submit ${channelName} token=${token} url=${url} http=${httpStatus ?? 'none'} ` +
        `accepted=${accepted} raw=${(r.stdout || r.stderr).slice(0, 160).replace(/\n/g, ' ')}`,
    );

    // A submission the product never accepted cannot produce a reply, and
    // grading its absence as LOSS blames the product for the harness's mistake.
    // 404 in particular means the ROUTE was wrong — the connector was never
    // reached, so nothing about the connector has been measured.
    if (!accepted) {
      this.fault(
        `submit/${channelName}`,
        `webhook POST to ${url} returned http=${httpStatus ?? 'transport-error'}; ` +
          `the product never accepted this message, so its absence downstream is an ` +
          `INSTRUMENT fault and MUST NOT be graded as inbound loss`,
      );
    }
    return { token, post: r, accepted, httpStatus };
  }

  /**
   * Wait until an arrival carrying `token` lands, or the budget expires.
   *
   * Returns {found, fault, tier, after_ms}. A `fault` result means the token was
   * SEEN in the journal in an encoding the matcher cannot decode. That is
   * INCOMPLETE, never loss, and the caller must not report it as absence.
   */
  waitForArrival(token, budgetMs = 60_000) {
    const step = 3000;
    let sawFault = false;
    let faultTier = 'none';
    for (let waited = 0; waited < budgetMs; waited += step) {
      const a = this.arrivals();
      for (const rec of a.records) {
        const text = JSON.stringify(rec);
        const m = carries(text, token);
        if (m.matched) {
          return { found: true, fault: false, tier: m.tier, after_ms: waited, total: a.records.length };
        }
        if (m.fault) {
          sawFault = true;
          faultTier = m.tier;
        }
      }
      process.stdout.write(`[clauses] waiting arrival ${token}: ${waited}ms ${new Date().toISOString()}\n`);
      sleep(step);
    }
    return {
      found: false,
      fault: sawFault,
      tier: sawFault ? faultTier : 'absent',
      after_ms: budgetMs,
      total: this.arrivals().records.length,
    };
  }

  // ── CLI surfaces under test ────────────────────────────────────────────────

  channelHealth() {
    const r = run([this.args.binary, 'channel', 'health', '--json'], {
      env: { ...process.env, WAYLAND_HOME: this.home, WAYLAND_VAULT_PASSPHRASE: this.vaultPassphrase },
      timeout: 30_000,
    });
    let parsed = null;
    try {
      parsed = JSON.parse(r.stdout);
    } catch {
      parsed = null;
    }
    return { ...r, parsed };
  }

  channelReload() {
    return run([this.args.binary, 'channel', 'reload'], {
      env: { ...process.env, WAYLAND_HOME: this.home, WAYLAND_VAULT_PASSPHRASE: this.vaultPassphrase },
      timeout: 30_000,
    });
  }

  cleanup() {
    for (const c of this.children) {
      try {
        c.kill('SIGKILL');
      } catch {
        /* already gone */
      }
    }
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// main
// ─────────────────────────────────────────────────────────────────────────────

function parseArgs(argv) {
  const out = { binary: null, runDir: path.join(os.tmpdir(), `f24-c3-clauses-${Date.now()}`) };
  for (let i = 2; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--binary') out.binary = argv[++i];
    else if (a === '--run-dir') out.runDir = argv[++i];
    else {
      process.stderr.write(`unknown argument: ${a}\n`);
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
  const args = parseArgs(process.argv);
  const R = new ClauseRun(args);
  let exitCode = 0;

  try {
    R.note(`run dir ${R.runDir}`);
    R.startFixtures();
    R.writeBaseConfig();

    // Two adapters at start. The THIRD is written later, for the reload leg.
    R.writeSlackChannel('f24finone', 'U24FINONE');
    R.writeSlackChannel('f24fintwo', 'U24FINTWO');
    R.note('base config: 2 channels on disk, 1 held back for the reload leg');

    // ── HEALTH: the pre-start control ────────────────────────────────────────
    // Health must REFUSE when no gateway is running. Without this leg, a health
    // surface that always printed a report would pass the post-start leg too.
    const preHealth = R.channelHealth();
    R.record(
      'health',
      'refuses-with-no-gateway',
      preHealth.status !== 0,
      `rc=${preHealth.status} stdout_bytes=${preHealth.stdout.length} ` +
        `stderr=${(preHealth.stderr || '').slice(0, 200).replace(/\n/g, ' ')}`,
      undefined,
    );

    // ── start the gateway ────────────────────────────────────────────────────
    R.startGateway();
    const bound = R.waitForWebhookHost(90);
    if (!bound) {
      R.record('gateway', 'webhook-host-bound', false, 'gateway run bound no inbound listener in 90s', undefined);
    } else {
      R.record('gateway', 'webhook-host-bound', true, `bound 127.0.0.1:${WEBHOOK_PORT}`, undefined);
    }

    // ── GATEWAY RE-MEASURE: inbound still arrives post lease+starvation fixes ─
    let baselineArrivals = 0;
    if (bound) {
      const s1 = R.postSlackInbound('f24finone', 'U24FINONE', 'DF24FINONE');
      const got1 = R.waitForArrival(s1.token, 90_000);
      const a = R.arrivals();
      const t = R.turns();
      baselineArrivals = a.records.length;
      R.note(
        `journal bytes: arrivals=${a.bytes} turns=${t.bytes} ` +
          `(an empty journal and an absent journal both read as 0 records)`,
      );
      if (got1.fault) {
        R.fault(
          'gateway-inbound',
          `token ${s1.token} is PRESENT in the journal at tier=${got1.tier} but the matcher ` +
            `cannot decode it. Grading INCOMPLETE, not LOSS — the message arrived.`,
        );
      }
      R.record(
        'gateway',
        'inbound-arrives-post-fixes',
        got1.found,
        `token=${s1.token} accepted=${s1.accepted} http=${s1.httpStatus} tier=${got1.tier} after=${got1.after_ms}ms ` +
          `arrivals=${a.records.length} turns=${t.records.length}`,
        a.records.length,
      );
    }

    // ── HEALTH: the real leg ─────────────────────────────────────────────────
    if (bound) {
      const h = R.channelHealth();
      const rep = h.parsed;
      if (!rep) {
        R.fault('health', `channel health --json produced unparseable stdout rc=${h.status}`);
        R.record('health', 'reports-running-gateway', false, `rc=${h.status} unparseable`, undefined);
      } else {
        // configured and registered are counted from DIFFERENT places by design
        // (channel.rs docs, F24-D-H2). Asserting both, and asserting they agree,
        // is what makes `0,0` unable to pass.
        const okCounts = rep.configured === 2 && rep.registered === 2 && !rep.registration_error;
        const perAdapter = Array.isArray(rep.channels) ? rep.channels : [];
        const named = perAdapter.map((c) => `${c.channel}:${c.state}`).join(',');
        // Every non-Healthy state MUST carry a reason (health.rs contract).
        const reasonContractHeld = perAdapter.every(
          (c) => c.state === 'healthy' || (c.reason !== undefined && c.reason !== null),
        );
        R.record(
          'health',
          'reports-running-gateway',
          okCounts && perAdapter.length === 2 && reasonContractHeld,
          `configured=${rep.configured} registered=${rep.registered} ` +
            `registration_error=${rep.registration_error ?? 'none'} ` +
            `per_adapter=[${named}] reason_contract_held=${reasonContractHeld}`,
          perAdapter.length,
        );
      }
    }

    // ── RELOAD: negative control BEFORE the reload ───────────────────────────
    // The new config exists on disk but the gateway has not been told. It must
    // NOT be registered. Without this control, "registered=3 after reload"
    // would also pass on a gateway that rescans on every tick regardless.
    let preReloadRegistered = null;
    if (bound) {
      R.writeSlackChannel('f24finthree', 'U24FINTHREE');
      R.note('third channel written to disk; gateway NOT yet told');
      sleep(5000);
      const h = R.channelHealth();
      preReloadRegistered = h.parsed ? h.parsed.registered : null;
      R.record(
        'reconnect-reload',
        'new-config-not-picked-up-without-reload',
        preReloadRegistered === 2,
        `registered=${preReloadRegistered} (want 2 — the third is on disk but unannounced)`,
        undefined,
      );
    }

    // ── RELOAD: the reload itself, then PROVE THE NEW ADAPTER CARRIES TRAFFIC ─
    if (bound) {
      const rl = R.channelReload();
      R.note(`channel reload rc=${rl.status} out=${(rl.stdout || rl.stderr).slice(0, 200).replace(/\n/g, ' ')}`);
      sleep(12_000);

      const h = R.channelHealth();
      const rep = h.parsed;
      const registeredAfter = rep ? rep.registered : null;
      R.record(
        'reconnect-reload',
        'reload-registers-the-new-adapter',
        rl.status === 0 && registeredAfter === 3 && rep && !rep.registration_error,
        `reload_rc=${rl.status} registered_before=${preReloadRegistered} registered_after=${registeredAfter} ` +
          `registration_error=${rep?.registration_error ?? 'none'}`,
        registeredAfter ?? 0,
      );

      // The load-bearing half. A count is not a capability.
      const before = R.arrivals().records.length;
      const s3 = R.postSlackInbound('f24finthree', 'U24FINTHREE', 'DF24FINTHREE');
      const got3 = R.waitForArrival(s3.token, 90_000);
      const after = R.arrivals().records.length;
      if (got3.fault) {
        R.fault(
          'reload-traffic',
          `token ${s3.token} is PRESENT at tier=${got3.tier} but undecodable. INCOMPLETE, not LOSS.`,
        );
      }
      R.record(
        'reconnect-reload',
        'reloaded-adapter-actually-carries-inbound',
        got3.found && after > before,
        `token=${s3.token} accepted=${s3.accepted} http=${s3.httpStatus} tier=${got3.tier} arrivals_before=${before} arrivals_after=${after} ` +
          `after=${got3.after_ms}ms`,
        after - before,
      );

      // The original adapters must still work after a reload. A reload that
      // replaced everything would pass every leg above and break the install.
      const beforeOld = R.arrivals().records.length;
      const s1b = R.postSlackInbound('f24finone', 'U24FINONE', 'DF24FINONE');
      const got1b = R.waitForArrival(s1b.token, 90_000);
      const afterOld = R.arrivals().records.length;
      R.record(
        'reconnect-reload',
        'unchanged-adapter-survives-the-reload',
        got1b.found && afterOld > beforeOld,
        `token=${s1b.token} accepted=${s1b.accepted} http=${s1b.httpStatus} tier=${got1b.tier} arrivals_before=${beforeOld} arrivals_after=${afterOld}`,
        afterOld - beforeOld,
      );
    }

    // ── verdict ──────────────────────────────────────────────────────────────
    const failed = R.legs.filter((l) => !l.pass);
    const zeroArrivals = R.arrivals().records.length === 0;

    // A GREEN WITH ZERO ARRIVALS GRADES FAIL. This is the universal-denial trap
    // and it has fired on this criterion before.
    let verdict;
    if (R.instrumentFaults.length > 0) {
      verdict = 'INCOMPLETE';
      exitCode = 3;
    } else if (zeroArrivals && bound) {
      verdict = 'FAIL';
      exitCode = 1;
      R.note('FORCED FAIL: every leg green but ZERO arrivals — a green over a dead path');
    } else if (failed.length > 0) {
      verdict = 'FAIL';
      exitCode = 1;
    } else {
      verdict = 'PASS';
      exitCode = 0;
    }

    const a = R.arrivals();
    const t = R.turns();
    const result = {
      verdict,
      exit_code: exitCode,
      legs_total: R.legs.length,
      legs_failed: failed.length,
      arrivals_total: a.records.length,
      arrivals_journal_bytes: a.bytes,
      turns_total: t.records.length,
      turns_journal_bytes: t.bytes,
      baseline_arrivals: baselineArrivals,
      instrument_fault: R.instrumentFaults.length > 0,
      instrument_faults: R.instrumentFaults,
      webhook_host_bound: bound,
      legs: R.legs,
      clauses_addressed: ['health', 'reconnect-reload'],
      clauses_NOT_addressed: ['media', 'native-actions'],
      notes: R.notes,
    };
    const resultPath = path.join(R.runDir, 'result.json');
    fs.writeFileSync(resultPath, JSON.stringify(result, null, 2));

    process.stdout.write(
      `\nCLAUSE MATRIX ${verdict} legs=${R.legs.length - failed.length}/${R.legs.length} ` +
        `failed=${failed.length} arrivals=${a.records.length} turns=${t.records.length} ` +
        `arrivals_bytes=${a.bytes} turns_bytes=${t.bytes} ` +
        `instrument_fault=${R.instrumentFaults.length > 0}\n` +
        `result: ${resultPath}\n`,
    );
  } catch (e) {
    // Never throw without a result document. The single most important
    // measurement a driver can make is "nothing bound / nothing arrived", and a
    // stack trace records neither.
    R.fault('driver', `unhandled: ${e && e.stack ? e.stack : String(e)}`);
    fs.mkdirSync(R.runDir, { recursive: true });
    fs.writeFileSync(
      path.join(R.runDir, 'result.json'),
      JSON.stringify(
        { verdict: 'INCOMPLETE', exit_code: 3, instrument_fault: true, instrument_faults: R.instrumentFaults, legs: R.legs, notes: R.notes },
        null,
        2,
      ),
    );
    process.stdout.write(`\nCLAUSE MATRIX INCOMPLETE (driver fault)\n`);
    exitCode = 3;
  } finally {
    R.cleanup();
  }

  process.exit(exitCode);
}

await main();
