#!/usr/bin/env node
// Phase 24 Success Criterion 5 — the ONE ordered setup-to-recovery journey.
//
//   node scripts/f24-journey.mjs --platform <linux|macos|windows> \
//                                --run-dir <PATH> --binary <PATH>
//
// All three arguments are required and long-named. There are NO positional
// arguments and NO defaults: a default binary path is exactly how a journey ends
// up proving a stale build, and this program has already lost one measurement to
// a stale binary that cargo declined to rebuild.
//
// THE JOURNEY IS ONE SEQUENCE, NOT A TEST MATRIX. The step list is fixed and
// identical on every platform — it is asserted against the canonical list the
// receipt verifier enforces. A platform difference appears ONLY in the
// invocation table below: how a process is killed, how the platform's own
// service mechanism is queried, how a residual registration is detected. If a
// platform needed a different STEP, that would be a different journey and the
// comparison the criterion asks for would be meaningless.
//
// WHAT THIS WRITES: exactly five files into --run-dir and NOTHING into the
// repository —
//   <platform>-receipt.json    the machine-checkable receipt
//   <platform>-raw.txt         the verbatim pre-redaction capture
//   <platform>-secrets.txt     the synthetic credentials this journey seeded
//   <platform>-canary.txt      the sentinel, for the redaction positive control
//   <platform>-redacted.md     the only file ever offered to a planning document
//
// EVERY CREDENTIAL HERE IS SYNTHETIC AND MINTED BY THIS SCRIPT. Nothing reads a
// real credential, no vendor is contacted, and the seeded values exist so the
// redaction claim has something real to be proved against.

import { spawnSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));

// Must equal wcore_eval_scenarios::journey::CANONICAL_STEPS, in order. The
// driver asserts its own executed order against this before writing a receipt,
// so a step silently skipped by a control-flow mistake fails here rather than
// producing a receipt the verifier then has to catch.
export const CANONICAL_STEPS = [
  'preflight-clean',
  'binary-identity',
  'profile-setup',
  'sink-start',
  'gateway-install',
  'gateway-start',
  'status-running',
  'automation-add',
  'deliveries-submit',
  'arrival-before-kill',
  'hard-kill',
  'platform-recover',
  'delivery-reconcile',
  'upgrade-in-place',
  'rollback',
  'redaction-canary',
  'drain-uninstall-clean',
];

export const RECEIPT_SCHEMA = 'wayland.journey.receipt/1';
export const ARRIVAL_SOURCE = 'independent-sink';

/// The adapter population the coverage fraction is drawn from, as
/// `wcore_channels_registry::channel_factory_for` dispatches it. Mirrors
/// `wcore_eval_scenarios::journey::REGISTERED_ADAPTER_TOTAL`; the Rust verifier
/// refuses a receipt that disagrees, so the two cannot drift silently.
export const REGISTERED_ADAPTER_TOTAL = 10;

// ── the adapter table ───────────────────────────────────────────────────────
//
// Three of the ten, each on a DISTINCT sink endpoint. The endpoint is what the
// sink observed being called, so a per-adapter tally keyed on it cannot be
// satisfied by three channel configs that all funnel into one code path.
export const ADAPTERS = [
  {
    adapter: 'slack',
    channel: 'f24jsink',
    endpoint: 'chat.postMessage',
    config: (url) => [
      'name = "f24jsink"',
      'platform = "slack"',
      'enabled = true',
      '',
      '[options]',
      'workspace_name = "f24j-fixture"',
      'default_channel_id = "f24j-room"',
      'credential_handle_bot_token = "slack.f24j.bot_token"',
      'credential_handle_signing_secret = "slack.f24j.signing_secret"',
      `api_base_url = "${url}"`,
      'max_retry_attempts = 1',
      '',
    ],
  },
  {
    adapter: 'whatsapp',
    channel: 'f24jwa',
    endpoint: 'whatsapp.messages',
    config: (url) => [
      'name = "f24jwa"',
      'platform = "whatsapp"',
      'enabled = true',
      '',
      '[options]',
      'workspace_name = "f24j-fixture"',
      'phone_number_id = "f24j-phone"',
      'default_recipient = "f24j-recipient"',
      'credential_handle_access_token = "whatsapp.f24j.access_token"',
      'credential_handle_app_secret = "whatsapp.f24j.app_secret"',
      `api_base_url = "${url}"`,
      'graph_version = "v21.0"',
      'max_retry_attempts = 1',
      '',
    ],
  },
  {
    adapter: 'sms',
    channel: 'f24jsms',
    endpoint: 'twilio.messages',
    config: (url) => [
      'name = "f24jsms"',
      'platform = "sms"',
      'enabled = true',
      '',
      '[options]',
      'from_number = "+15005550006"',
      'credential_handle_account_sid = "sms.f24j.account_sid"',
      'credential_handle_auth_token = "sms.f24j.auth_token"',
      `api_base_url = "${url}"`,
      'max_retry_attempts = 1',
      '',
    ],
  },
];

const PROFILE = 'f24j';
const SERVICE_NAME = `wayland-core-gateway-${PROFILE}`;
const DELIVERY_COUNT = 12;
const RECOVER_BUDGET_MS = 120_000;
const ARRIVAL_BUDGET_MS = 180_000;

// ── the invocation table: the ONLY place a platform difference may live ──────
const PLATFORMS = {
  linux: {
    family: 'systemd',
    kill: (pid) => ['kill', '-9', String(pid)],
    alive: (pid) => ['kill', '-0', String(pid)],
    platformQuery: () => ['systemctl', '--user', 'show', '-p', 'NRestarts', '--value', SERVICE_NAME],
    residual: () => ['systemctl', '--user', 'list-unit-files', `${SERVICE_NAME}.service`],
    // A residual registration is present when the query SUCCEEDS and names the
    // unit. After a clean uninstall the query must not name it.
    residualPresent: (r) => r.status === 0 && r.output.includes(SERVICE_NAME),
    postInstall: () => ['systemctl', '--user', 'daemon-reload'],
  },
  macos: {
    family: 'launchd',
    kill: (pid) => ['kill', '-9', String(pid)],
    alive: (pid) => ['kill', '-0', String(pid)],
    platformQuery: () => ['launchctl', 'list', SERVICE_NAME],
    residual: () => ['launchctl', 'list', SERVICE_NAME],
    residualPresent: (r) => r.status === 0 && r.output.includes(SERVICE_NAME),
    postInstall: null,
  },
  windows: {
    family: 'schtasks',
    kill: (pid) => ['taskkill', '/F', '/PID', String(pid)],
    alive: (pid) => ['tasklist', '/FI', `PID eq ${pid}`, '/NH'],
    // `tasklist /FI` exits 0 whether or not it matched, so aliveness is read
    // from the OUTPUT naming the pid, never from the status. This is the same
    // self-passing shape as a filter stealing an exit code.
    aliveFromOutput: (r, pid) => new RegExp(`\\b${pid}\\b`).test(r.output),
    platformQuery: () => ['schtasks', '/query', '/tn', SERVICE_NAME, '/v', '/fo', 'list'],
    residual: () => ['schtasks', '/query', '/tn', SERVICE_NAME],
    residualPresent: (r) => r.status === 0 && r.output.includes(SERVICE_NAME),
    postInstall: null,
  },
};

// ── plumbing ────────────────────────────────────────────────────────────────

class StepFailure extends Error {}

function parseArgs(argv) {
  const out = {};
  const known = new Set(['--platform', '--run-dir', '--binary']);
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (!known.has(arg)) throw new StepFailure(`unknown argument ${arg}`);
    const value = argv[++i];
    if (value === undefined) throw new StepFailure(`${arg} needs a value`);
    out[arg.replace(/^--/, '').replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = value;
  }
  for (const required of ['platform', 'runDir', 'binary']) {
    if (!out[required]) throw new StepFailure(`--${required} is required`);
  }
  if (!PLATFORMS[out.platform]) {
    throw new StepFailure(`--platform must be one of ${Object.keys(PLATFORMS).join('|')}`);
  }
  return out;
}

// Run an argv, never a shell string. Every argument this journey passes is a
// path or a profile name that crossed a boundary from the operator; in argv
// mode a metacharacter reaches the child as a literal byte.
export function run(argv, opts = {}) {
  const result = spawnSync(argv[0], argv.slice(1), {
    encoding: 'utf8',
    env: { ...process.env, ...(opts.env ?? {}) },
    timeout: opts.timeoutMs ?? 120_000,
    windowsHide: true,
  });
  // A command that could not be spawned at all is the single most dangerous
  // outcome here: `status` is null, `stdout` is empty, and any assertion
  // written as "output does not contain X" passes. It is turned into an
  // explicit failure rather than an empty success.
  if (result.error) {
    return {
      argv,
      status: null,
      output: `SPAWN FAILED: ${result.error.message}`,
      spawnFailed: true,
    };
  }
  return {
    argv,
    status: result.status,
    output: `${result.stdout ?? ''}${result.stderr ?? ''}`,
    spawnFailed: false,
  };
}

// `gateway status --json` PRETTY-PRINTS, so the projection spans many lines and
// may be preceded by unrelated output. Taking the last line yields `}` and the
// parse fails silently — which reads as "no live pid" for a gateway that was
// reporting `state: running` with a live pid the whole time. Measured on the
// Linux host: the first journey run failed its status step for exactly this
// reason while the projection it printed was correct.
export function parseStatusJson(output) {
  const text = String(output ?? '');
  const start = text.indexOf('{');
  const end = text.lastIndexOf('}');
  if (start === -1 || end <= start) return null;
  try {
    return JSON.parse(text.slice(start, end + 1));
  } catch {
    return null;
  }
}

function shellish(argv) {
  return argv.map((a) => (/\s/.test(a) ? JSON.stringify(a) : a)).join(' ');
}

// Deliberately synchronous: the journey is one ordered sequence and every wait
// in it is a wait for the PLATFORM to act, not for this script. A spurious early
// return from Atomics.wait would silently shorten a recovery budget, so the loop
// re-arms until the deadline actually passes.
function sleep(ms) {
  const until = Date.now() + ms;
  const buffer = new Int32Array(new SharedArrayBuffer(4));
  for (let remaining = ms; remaining > 0; remaining = until - Date.now()) {
    Atomics.wait(buffer, 0, 0, remaining);
  }
}

// ── the journey ─────────────────────────────────────────────────────────────

class Journey {
  constructor(args) {
    this.args = args;
    this.table = PLATFORMS[args.platform];
    this.runDir = path.resolve(args.runDir);
    this.binary = path.resolve(args.binary);
    this.steps = [];
    this.raw = [];
    this.startedAt = new Date().toISOString();
    this.home = path.join(this.runDir, `${args.platform}-home`);
    // Synthetic. Minted here, used nowhere else, never read from a keychain.
    this.canary = `WLJ-CANARY-${args.platform}-${crypto.randomBytes(16).toString('hex')}`;
    this.botToken = `xoxb-f24j-${this.canary}`;
    this.signingSecret = `f24j-signing-${crypto.randomBytes(12).toString('hex')}`;
    this.counts = { submitted: 0, arrived: 0, unique: 0, duplicates: 0, losses: 0 };
    // Initialised here, not lazily at step 9. `snapshot()` reads both, and a
    // field that only exists after a given step makes the reconciliation
    // depend on step ordering — which is the class of bug this file is about.
    this.bodies = [];
    this.bodyAdapter = new Map();
    this.finalSnapshot = null;
    this.candidateCommit = null;
    this.binaryVersion = null;
    this.sink = null;
    this.sinkUrl = null;
    this.journalPath = path.join(this.runDir, `${args.platform}-arrivals.jsonl`);
  }

  // Record a step. `output` must be non-empty: the receipt verifier refuses an
  // empty capture, and it refuses it because an assertion against empty output
  // is a pass that means nothing.
  step(name, command, output) {
    const expected = CANONICAL_STEPS[this.steps.length];
    if (name !== expected) {
      throw new StepFailure(
        `step order violated: recorded ${name} where ${expected} was due`,
      );
    }
    const text = String(output ?? '').trim();
    if (!text) throw new StepFailure(`step ${name} captured no output`);
    this.steps.push({ name, command, output: text, ok: true });
    this.raw.push(`### ${name}\n$ ${command}\n${text}\n`);
    process.stdout.write(`[${this.steps.length}/${CANONICAL_STEPS.length}] ${name} OK\n`);
  }

  // Run and REQUIRE success. A spawn failure and a non-zero status are both
  // failures; neither is allowed to read as "nothing to object to".
  must(argv, opts = {}) {
    const r = run(argv, { env: this.env(), ...opts });
    if (r.spawnFailed || r.status !== 0) {
      throw new StepFailure(
        `${shellish(argv)} exited ${r.status === null ? 'SPAWN-FAILED' : r.status}\n${r.output}`,
      );
    }
    return r;
  }

  env() {
    return { WAYLAND_HOME: this.home, WAYLAND_PROFILE: PROFILE };
  }

  core(...argv) {
    return [this.binary, ...argv];
  }

  status() {
    const r = run(this.core('gateway', 'status', '--profile', PROFILE, '--json'), {
      env: this.env(),
    });
    return { raw: r, parsed: parseStatusJson(r.output) };
  }

  // ── step 1 ───────────────────────────────────────────────────────────────
  preflightClean() {
    fs.mkdirSync(this.runDir, { recursive: true });
    fs.rmSync(this.home, { recursive: true, force: true });
    const residual = run(this.table.residual(), { env: this.env() });
    if (this.table.residualPresent(residual)) {
      throw new StepFailure(
        `a ${this.table.family} registration for ${SERVICE_NAME} already exists; ` +
          `the journey refuses to start from a dirty machine\n${residual.output}`,
      );
    }
    const s = this.status();
    const state = s.parsed?.state ?? 'unparsable';
    if (typeof state === 'string' && state.toLowerCase() === 'running') {
      throw new StepFailure(`a gateway for profile ${PROFILE} is already running`);
    }
    this.step(
      'preflight-clean',
      shellish(this.table.residual()),
      `residual_query_status=${residual.status}\n${residual.output}\n` +
        `gateway_state=${JSON.stringify(state)}`,
    );
  }

  // ── step 2 ───────────────────────────────────────────────────────────────
  binaryIdentity() {
    if (!fs.existsSync(this.binary)) {
      throw new StepFailure(`--binary ${this.binary} does not exist`);
    }
    const r = this.must(this.core('--build-info'));
    // `wayland-core <version> (source <40-hex>)`. The commit comes out of the
    // BINARY, not out of whatever the host's checkout happens to be sitting at:
    // those two diverge exactly when a stale binary is being driven, and that
    // is the case worth catching. An artifact newer than its source is a build
    // that did not happen.
    const match = r.output.match(/^(wayland-core\s+\S+)\s+\(source\s+([0-9a-f]{40})\)/m);
    if (!match) {
      throw new StepFailure(`--build-info did not report a source commit:\n${r.output}`);
    }
    this.binaryVersion = match[1];
    this.candidateCommit = match[2];
    const digest = crypto.createHash('sha256').update(fs.readFileSync(this.binary)).digest('hex');
    this.binarySha256 = digest;
    this.step(
      'binary-identity',
      shellish(this.core('--build-info')),
      `${r.output.trim()}\nsha256=${digest}\npath=${this.binary}`,
    );
  }

  // ── step 3 ───────────────────────────────────────────────────────────────
  profileSetup() {
    fs.mkdirSync(path.join(this.home, 'channels'), { recursive: true });
    const credentials = [
      '[secrets]',
      `"slack.f24j.bot_token" = "${this.botToken}"`,
      `"slack.f24j.signing_secret" = "${this.signingSecret}"`,
      // The other two adapters' credential handles. Same synthetic values, so
      // the redaction sweep still has one secret set to hunt for.
      `"whatsapp.f24j.access_token" = "${this.botToken}"`,
      `"whatsapp.f24j.app_secret" = "${this.signingSecret}"`,
      `"sms.f24j.account_sid" = "${this.botToken}"`,
      `"sms.f24j.auth_token" = "${this.signingSecret}"`,
      '',
    ].join('\n');
    const credentialsPath = path.join(this.home, 'credentials.toml');
    fs.writeFileSync(credentialsPath, credentials, { mode: 0o600 });
    fs.writeFileSync(
      path.join(this.runDir, `${this.args.platform}-secrets.txt`),
      `${this.botToken}\n${this.signingSecret}\n`,
      { mode: 0o600 },
    );
    fs.writeFileSync(
      path.join(this.runDir, `${this.args.platform}-canary.txt`),
      `${this.canary}\n`,
      { mode: 0o600 },
    );
    this.step(
      'profile-setup',
      `write ${credentialsPath} + channels/f24jsink.toml (synthetic credentials)`,
      `home=${this.home}\nprofile=${PROFILE}\n` +
        `credentials_mode=0600\ncanary_bytes=${this.canary.length}\n` +
        `seeded_bot_token=${this.botToken}`,
    );
  }

  // ── step 4 ───────────────────────────────────────────────────────────────
  sinkStart() {
    fs.rmSync(this.journalPath, { force: true });
    const sinkScript = path.join(HERE, 'f24-sink.mjs');
    // The sink's output goes to a FILE, not to a pipe this process reads with
    // event callbacks. Every wait in this driver is a blocking one, so the event
    // loop is not running to drain a pipe — a piped child would deadlock on a
    // full buffer and the failure would look like a sink that never started.
    const sinkLog = path.join(this.runDir, `${this.args.platform}-sink.log`);
    fs.writeFileSync(sinkLog, '');
    const logFd = fs.openSync(sinkLog, 'a');
    const child = spawn(process.execPath, [sinkScript, '--journal', this.journalPath], {
      stdio: ['ignore', logFd, logFd],
      windowsHide: true,
    });
    this.sink = child;
    let banner = '';
    const deadline = Date.now() + 20_000;
    // Read the bound URL BEFORE writing the channel config. A gateway pointed
    // at an unbound port fails its sends for a reason indistinguishable from a
    // product defect.
    while (Date.now() < deadline) {
      banner = fs.readFileSync(sinkLog, 'utf8');
      if (banner.includes('SINK_READY')) break;
      sleep(200);
    }
    const match = banner.match(/SINK_READY url=(\S+) journal=(\S+)/);
    if (!match) {
      throw new StepFailure(`independent sink never signalled ready:\n${banner}`);
    }
    this.sinkUrl = match[1];

    // THREE adapters, not one.
    //
    // Every Phase 24 journey receipt published before this change carried
    // `submitted=12 arrived=12 unique=12 duplicates=0 losses=0` on all three
    // platforms — and every one of those twelve rode Slack, the single adapter
    // of ten that implements the property the tally is about. The receipt had
    // no field naming the adapter, so a one-adapter run and a ten-adapter run
    // produced identical numbers and the platform matrix read as a delivery
    // matrix it never was.
    //
    // The sink has served the WhatsApp and Twilio outbound endpoints since lane
    // 24-c3 landed them, with a comment noting "the journey never calls these".
    // It does now. Each adapter lands on a DISTINCT endpoint, which is what
    // makes the per-adapter tally an observation rather than a restatement of
    // the config.
    for (const spec of ADAPTERS) {
      fs.writeFileSync(
        path.join(this.home, 'channels', `${spec.channel}.toml`),
        spec.config(this.sinkUrl).join('\n'),
      );
    }

    // A positive control on the sink itself. If the health endpoint does not
    // answer, every later arrival count would be zero for a reason that has
    // nothing to do with the gateway.
    const health = run([
      process.execPath,
      '-e',
      `fetch(${JSON.stringify(`${this.sinkUrl}/_sink/health`)}).then(r=>r.text()).then(t=>{process.stdout.write(t)}).catch(e=>{process.stdout.write('HEALTH FAILED '+e.message);process.exit(1)})`,
    ]);
    if (health.status !== 0 || !health.output.includes('"ok":true')) {
      throw new StepFailure(`independent sink health check failed:\n${health.output}`);
    }
    this.step(
      'sink-start',
      `${process.execPath} scripts/f24-sink.mjs --journal ${this.journalPath}`,
      `${match[0]}\nsink_pid=${child.pid}\nhealth=${health.output.trim()}`,
    );
  }

  // ── step 5 ───────────────────────────────────────────────────────────────
  gatewayInstall() {
    const argv = this.core('gateway', 'install', '--profile', PROFILE);
    const r = this.must(argv);
    let post = '';
    if (this.table.postInstall) {
      const p = run(this.table.postInstall(), { env: this.env() });
      post = `\n$ ${shellish(this.table.postInstall())}\nstatus=${p.status}\n${p.output}`;
    }
    const registration = run(this.table.residual(), { env: this.env() });
    if (!this.table.residualPresent(registration)) {
      throw new StepFailure(
        `install reported success but ${this.table.family} does not know ${SERVICE_NAME}\n` +
          `status=${registration.status}\n${registration.output}`,
      );
    }
    this.step(
      'gateway-install',
      shellish(argv),
      `${r.output.trim()}${post}\n$ ${shellish(this.table.residual())}\n` +
        `registration_status=${registration.status}\n${registration.output}`,
    );
  }

  // ── step 6 ───────────────────────────────────────────────────────────────
  gatewayStart() {
    const argv = this.core('gateway', 'start', '--profile', PROFILE);
    const r = this.must(argv);
    this.step('gateway-start', shellish(argv), r.output || `exit=0 (${shellish(argv)})`);
  }

  // ── step 7 ───────────────────────────────────────────────────────────────
  statusRunning() {
    const deadline = Date.now() + 60_000;
    let last = null;
    while (Date.now() < deadline) {
      last = this.status();
      const pid = last.parsed?.pid;
      if (pid && this.pidAlive(pid)) {
        this.firstPid = pid;
        this.step(
          'status-running',
          shellish(this.core('gateway', 'status', '--profile', PROFILE, '--json')),
          `${last.raw.output.trim()}\nliveness_probe=${shellish(this.table.alive(pid))} -> alive`,
        );
        return;
      }
      sleep(1000);
    }
    throw new StepFailure(
      `gateway never reported a live pid within 60s\n${last?.raw.output ?? '(no output)'}`,
    );
  }

  pidAlive(pid) {
    const r = run(this.table.alive(pid));
    if (this.table.aliveFromOutput) return this.table.aliveFromOutput(r, pid);
    return r.status === 0;
  }

  // ── step 8 ───────────────────────────────────────────────────────────────
  automationAdd() {
    // Two trigger types, as the step list requires. `every:` is the one the
    // deliveries ride on; `cron:` is a second KIND, proving the automation
    // surface is not a single-shape special case.
    const every = this.core(
      'cron',
      'add',
      '--trigger',
      'every:15',
      '--channel',
      'f24jsink',
      '--text',
      'f24j-heartbeat',
    );
    const cron = this.core(
      'cron',
      'add',
      '--trigger',
      'cron:0 9 * * *',
      '--channel',
      'f24jsink',
      '--text',
      'f24j-daily',
    );
    const a = this.must(every);
    const b = this.must(cron);
    const list = this.must(this.core('cron', 'list'));
    this.step(
      'automation-add',
      `${shellish(every)} ; ${shellish(cron)}`,
      `${a.output.trim()}\n${b.output.trim()}\n$ ${shellish(this.core('cron', 'list'))}\n${list.output.trim()}`,
    );
  }

  // ── step 9 ───────────────────────────────────────────────────────────────
  deliveriesSubmit() {
    this.bodies = [];
    // body -> the adapter that was asked to carry it. Recorded at SUBMIT time
    // from the channel argv, so the later tally can be cross-checked against
    // what the sink independently observed rather than assumed to agree.
    this.bodyAdapter = new Map();
    const lines = [];
    const perAdapter = new Map(ADAPTERS.map((s) => [s.adapter, 0]));
    for (let i = 1; i <= DELIVERY_COUNT; i += 1) {
      // Round-robin, so the split is even and no adapter can be starved by an
      // ordering accident into contributing zero and being silently dropped
      // from the coverage list.
      const spec = ADAPTERS[(i - 1) % ADAPTERS.length];
      const body = `f24j-delivery-${String(i).padStart(2, '0')}`;
      this.bodies.push(body);
      this.bodyAdapter.set(body, spec.adapter);
      perAdapter.set(spec.adapter, perAdapter.get(spec.adapter) + 1);
      const argv = this.core(
        'cron',
        'add',
        '--trigger',
        'every:15',
        '--channel',
        spec.channel,
        '--text',
        body,
      );
      const r = this.must(argv);
      lines.push(`${body} -> ${spec.adapter}: ${r.output.trim().split('\n').pop()}`);
    }
    this.counts.submitted = DELIVERY_COUNT;
    const split = [...perAdapter.entries()].map(([a, n]) => `${a}=${n}`).join(' ');
    this.step(
      'deliveries-submit',
      `${shellish(this.core('cron', 'add', '--trigger', 'every:15', '--channel', '<per-adapter>', '--text', 'f24j-delivery-NN'))} x${DELIVERY_COUNT} across ${ADAPTERS.length} adapters`,
      `submitted=${DELIVERY_COUNT}\nsubmitted_by_adapter=${split}\n${lines.join('\n')}`,
    );
  }

  // ── the ONE journal read ────────────────────────────────────────────────
  //
  // F24-GWP-M1. The headline counts and the per-adapter breakdown used to be
  // read from the journal at DIFFERENT TIMES: `counts` was frozen at step 13
  // (`delivery-reconcile`) and `adapter_coverage` was recomputed when the
  // receipt was written, four steps later. Anything arriving in between was
  // counted by the breakdown and structurally invisible to the headline.
  //
  // That is not hypothetical. `windows-receipt-attempt3.json` published
  // `counts.arrived = 12, duplicates: 0` beside a breakdown summing to 24,
  // because twelve deliveries re-arrived at 02:19:03 — after step 13, before
  // `finished_at` at 02:19:04. `duplicates = arrived - unique` was a correct
  // formula applied to a stale `arrived`. A criterion graded on the headline
  // would have called that run exactly-once.
  //
  // So the two are no longer two reads. `snapshot()` reads the journal ONCE
  // and projects the same `seen` array into both the headline and the
  // breakdown, which makes them incapable of disagreeing rather than merely
  // equal today. Nothing else may compute either number.
  snapshot() {
    const wanted = new Set(this.bodies);
    const lines = this.arrivals();
    const seen = lines.filter((a) => !a.suppressed && wanted.has(a.text));

    const perBody = new Map();
    for (const a of seen) perBody.set(a.text, (perBody.get(a.text) ?? 0) + 1);
    const arrived = seen.length;
    const unique = perBody.size;

    // Per-adapter coverage, keyed on the endpoint the SINK recorded rather than
    // on the channel this driver configured. Those two agreeing is the finding;
    // asserting the second and reporting it as the first would be exactly the
    // restatement this receipt exists to refuse.
    const exercised = [];
    let attributed = 0;
    for (const spec of ADAPTERS) {
      const mine = seen.filter((a) => a.endpoint === spec.endpoint);
      const submitted = [...this.bodyAdapter.entries()].filter(
        ([, adapter]) => adapter === spec.adapter,
      ).length;
      attributed += mine.length;
      exercised.push({
        adapter: spec.adapter,
        endpoint: spec.endpoint,
        submitted,
        arrived: mine.length,
        unique: new Set(mine.map((a) => a.text)).size,
      });
    }

    return {
      counts: {
        submitted: this.counts.submitted,
        arrived,
        unique,
        duplicates: arrived - unique,
        losses: this.counts.submitted - unique,
      },
      coverage: { registered_total: REGISTERED_ADAPTER_TOTAL, exercised },
      // Diagnostics, not receipt fields. `journal_lines` is every line the sink
      // wrote including the heartbeat; `unattributed` is arrivals the headline
      // counts that no adapter endpoint claimed, which would make the two sums
      // differ for a reason that is NOT a stale read.
      journal_lines: lines.length,
      unattributed: arrived - attributed,
    };
  }

  arrivals() {
    if (!fs.existsSync(this.journalPath)) return [];
    return fs
      .readFileSync(this.journalPath, 'utf8')
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

  // Count only the bodies this journey submitted. The heartbeat job also
  // delivers, and folding it into the reconciliation would make `submitted`
  // and `arrived` disagree for a reason that has nothing to do with recovery.
  //
  // Kept as the headline projection of `snapshot()` so there is exactly one
  // place the numbers come from — see the F24-GWP-M1 note above `snapshot()`.
  tally() {
    return this.snapshot().counts;
  }

  // Retained for readability at the two call sites that want only the
  // breakdown. Same single read.
  adapterCoverage() {
    return this.snapshot().coverage;
  }

  // ── step 10 ──────────────────────────────────────────────────────────────
  arrivalBeforeKill() {
    const deadline = Date.now() + ARRIVAL_BUDGET_MS;
    let t = this.tally();
    while (Date.now() < deadline && t.unique < 1) {
      sleep(2000);
      t = this.tally();
    }
    if (t.unique < 1) {
      throw new StepFailure(
        `no delivery reached the independent sink within ${ARRIVAL_BUDGET_MS}ms; ` +
          `journal=${this.journalPath} lines=${this.arrivals().length}`,
      );
    }
    this.step(
      'arrival-before-kill',
      `read arrivals journal ${this.journalPath} (owned by the independent sink)`,
      `arrivals_total=${this.arrivals().length}\nunique_expected_bodies=${t.unique}\n` +
        `first_arrival=${JSON.stringify(this.arrivals()[0])}`,
    );
  }

  // ── step 11 ──────────────────────────────────────────────────────────────
  hardKill() {
    const s = this.status();
    const pid = s.parsed?.pid;
    if (!pid) throw new StepFailure(`no live pid to kill:\n${s.raw.output}`);
    this.killedPid = pid;
    const argv = this.table.kill(pid);
    const r = run(argv);
    // A hard kill gives the runtime no chance to drain. It is not `gateway
    // stop`, and it is not a signal the process can handle.
    let gone = false;
    const deadline = Date.now() + 20_000;
    while (Date.now() < deadline) {
      if (!this.pidAlive(pid)) {
        gone = true;
        break;
      }
      sleep(500);
    }
    if (!gone) throw new StepFailure(`pid ${pid} survived ${shellish(argv)}`);
    this.step(
      'hard-kill',
      shellish(argv),
      `killed_pid=${pid}\nkill_status=${r.status}\n${r.output}\n` +
        `liveness_after_kill=${shellish(this.table.alive(pid))} -> gone`,
    );
  }

  // ── step 12 ──────────────────────────────────────────────────────────────
  platformRecover() {
    // The PLATFORM brings it back. This journey does not run `gateway start`
    // here, and that omission is the whole point: restarting it by hand would
    // prove a process can be started twice, not that the service integration
    // recovers.
    const deadline = Date.now() + RECOVER_BUDGET_MS;
    let last = null;
    while (Date.now() < deadline) {
      last = this.status();
      const pid = last.parsed?.pid;
      if (pid && pid !== this.killedPid && this.pidAlive(pid)) {
        const platform = run(this.table.platformQuery(), { env: this.env() });
        this.recoveredPid = pid;
        this.step(
          'platform-recover',
          `${shellish(this.core('gateway', 'status', '--profile', PROFILE, '--json'))} (polled; NO manual start)`,
          `killed_pid=${this.killedPid}\nrecovered_pid=${pid}\n` +
            `${last.raw.output.trim()}\n$ ${shellish(this.table.platformQuery())}\n` +
            `status=${platform.status}\n${platform.output}`,
        );
        return;
      }
      sleep(2000);
    }
    const platform = run(this.table.platformQuery(), { env: this.env() });
    throw new StepFailure(
      `the ${this.table.family} mechanism did not bring the runtime back within ` +
        `${RECOVER_BUDGET_MS}ms after pid ${this.killedPid} was hard-killed.\n` +
        `last status: ${last?.raw.output ?? '(none)'}\n` +
        `$ ${shellish(this.table.platformQuery())}\nstatus=${platform.status}\n${platform.output}`,
    );
  }

  // ── step 13 ──────────────────────────────────────────────────────────────
  deliveryReconcile() {
    const deadline = Date.now() + ARRIVAL_BUDGET_MS;
    let snap = this.snapshot();
    let t = snap.counts;
    while (Date.now() < deadline && (t.losses > 0 || t.duplicates > 0)) {
      sleep(3000);
      snap = this.snapshot();
      t = snap.counts;
    }
    this.counts = t;
    if (t.duplicates !== 0 || t.losses !== 0) {
      throw new StepFailure(
        `delivery reconciliation is not clean across the kill and the recovery: ` +
          `submitted=${t.submitted} arrived=${t.arrived} unique=${t.unique} ` +
          `duplicates=${t.duplicates} losses=${t.losses}`,
      );
    }
    // Per-adapter, from the endpoint the SINK observed, out of the SAME
    // snapshot as `t`. A headline of `12/12/12/0/0` is the number that was
    // published three times over a one-adapter run; the breakdown is what makes
    // it interpretable — and it is only interpretable against that headline if
    // both were read at the same instant.
    const coverage = snap.coverage;
    const idle = coverage.exercised.filter((e) => e.arrived === 0);
    if (idle.length > 0) {
      // §6a-i: a participant that never started makes the run a different
      // experiment, not a negative result. An adapter that submitted but never
      // arrived means the multi-adapter claim is not the one being measured.
      throw new StepFailure(
        `adapter(s) ${idle.map((e) => e.adapter).join(',')} submitted but produced ZERO arrivals ` +
          `at the independent sink; this run did not exercise the adapter set it claims`,
      );
    }
    const perAdapter = coverage.exercised
      .map((e) => `  ${e.adapter} endpoint=${e.endpoint} submitted=${e.submitted} arrived=${e.arrived} unique=${e.unique}`)
      .join('\n');
    this.step(
      'delivery-reconcile',
      `tally the independent sink's journal ${this.journalPath}, per observed endpoint`,
      `arrival_source=${ARRIVAL_SOURCE}\nsubmitted=${t.submitted}\narrived=${t.arrived}\n` +
        `unique=${t.unique}\nduplicates=${t.duplicates}\nlosses=${t.losses}\n` +
        `journal_lines_total=${this.arrivals().length}\n` +
        `adapters_exercised=${coverage.exercised.length}/${REGISTERED_ADAPTER_TOTAL}\n${perAdapter}`,
    );
  }

  // ── step 14 ──────────────────────────────────────────────────────────────
  // Criterion 1 names UPGRADE and ROLLBACK and neither had ever been performed
  // on any platform. The projection carries `binary_path` and `binary_version`
  // precisely so the two are distinguishable, so the check is that the running
  // service reports the NEW binary after an upgrade and the OLD one after a
  // rollback.
  upgradeInPlace() {
    const upgraded = path.join(
      this.runDir,
      `${this.args.platform}-upgraded-core${path.extname(this.binary)}`,
    );
    fs.copyFileSync(this.binary, upgraded);
    if (this.args.platform !== 'windows') fs.chmodSync(upgraded, 0o755);
    this.upgradedPath = upgraded;
    this.step('upgrade-in-place', ...this.swapTo(upgraded, 'upgrade'));
  }

  // ── step 15 ──────────────────────────────────────────────────────────────
  rollback() {
    this.step('rollback', ...this.swapTo(this.binary, 'rollback'));
  }

  // Stop, re-register against `target`, start, and require the RUNNING service
  // to report that binary. `gateway install` derives the registration from the
  // binary that ran it, so invoking it from the target is the operator's own
  // upgrade path rather than a synthetic edit of a unit file.
  //
  // The uninstall is not cosmetic: launchd refuses to load a label it already
  // holds, so an install over a live registration would fail on macOS and pass
  // on the other two — a platform difference in a STEP, which is precisely what
  // the one-journey rule forbids.
  swapTo(target, label) {
    const stop = run(this.core('gateway', 'stop', '--profile', PROFILE), { env: this.env() });
    const uninstall = this.must(this.core('gateway', 'uninstall', '--profile', PROFILE));
    if (this.table.postInstall) run(this.table.postInstall(), { env: this.env() });
    const install = this.must([target, 'gateway', 'install', '--profile', PROFILE]);
    if (this.table.postInstall) run(this.table.postInstall(), { env: this.env() });
    const start = this.must(this.core('gateway', 'start', '--profile', PROFILE));
    const observed = this.awaitBinaryPath(target);
    return [
      `${shellish(this.core('gateway', 'uninstall', '--profile', PROFILE))} ; ` +
        `${shellish([target, 'gateway', 'install', '--profile', PROFILE])} ; ` +
        `${shellish(this.core('gateway', 'start', '--profile', PROFILE))}`,
      `${label}_target=${target}\n` +
        `stop_status=${stop.status}\n${stop.output.trim()}\n` +
        `${uninstall.output.trim()}\n${install.output.trim()}\n${start.output.trim()}\n` +
        `observed_binary_path=${observed.binaryPath}\nobserved_pid=${observed.pid}\n` +
        `${observed.raw}`,
    ];
  }

  awaitBinaryPath(expected) {
    const deadline = Date.now() + 90_000;
    let last = null;
    const wanted = path.resolve(expected);
    while (Date.now() < deadline) {
      last = this.status();
      const reported = last.parsed?.binary_path;
      if (reported && path.resolve(reported) === wanted && last.parsed?.pid) {
        return {
          binaryPath: reported,
          pid: last.parsed.pid,
          raw: last.raw.output.trim(),
        };
      }
      sleep(2000);
    }
    throw new StepFailure(
      `the running service never reported binary_path=${wanted}; ` +
        `last status:\n${last?.raw.output ?? '(none)'}`,
    );
  }

  // ── step 16 ──────────────────────────────────────────────────────────────
  // The redaction claim gets a POSITIVE CONTROL. A canary that was never
  // planted is trivially absent, so proving absence alone certifies nothing.
  // Both halves must hold: PRESENT in the pre-redaction capture (it really did
  // travel a capture path) and ABSENT from the redacted output.
  redactionCanary() {
    const rawPath = path.join(this.runDir, `${this.args.platform}-raw.txt`);
    const rawText = this.raw.join('\n');
    fs.writeFileSync(rawPath, rawText, { mode: 0o600 });
    if (!rawText.includes(this.canary)) {
      throw new StepFailure(
        'the canary is absent from the pre-redaction capture, so its absence from ' +
          'the redacted copy would prove nothing',
      );
    }
    const secrets = [this.botToken, this.signingSecret, this.canary];
    for (const secret of secrets) {
      if (secret.length < 8) throw new StepFailure(`refusing a ${secret.length}-byte secret`);
    }
    let redacted = rawText;
    for (const secret of secrets) redacted = redacted.split(secret).join('[REDACTED]');
    const redactedPath = path.join(this.runDir, `${this.args.platform}-redacted.md`);
    fs.writeFileSync(redactedPath, redacted);
    // Re-read what was WRITTEN, not what was computed in memory. The file on
    // disk is what a document quotes.
    const written = fs.readFileSync(redactedPath, 'utf8');
    for (const secret of secrets) {
      if (written.includes(secret)) {
        throw new StepFailure(`a secret survived into ${redactedPath}`);
      }
    }
    this.step(
      'redaction-canary',
      `redact ${rawPath} -> ${redactedPath} over ${secrets.length} seeded secrets`,
      `control=present published=absent secrets=${secrets.length}\n` +
        `raw_bytes=${rawText.length}\nredacted_bytes=${written.length}\n` +
        `redaction_markers=${(written.match(/\[REDACTED\]/g) ?? []).length}`,
    );
  }

  // ── step 17 ──────────────────────────────────────────────────────────────
  drainUninstallClean() {
    const drain = this.must(this.core('gateway', 'drain', '--profile', PROFILE, '--budget-ms', '15000'), {
      timeoutMs: 60_000,
    });
    const uninstall = this.must(this.core('gateway', 'uninstall', '--profile', PROFILE));
    if (this.table.postInstall) run(this.table.postInstall(), { env: this.env() });

    const residual = run(this.table.residual(), { env: this.env() });
    if (this.table.residualPresent(residual)) {
      throw new StepFailure(
        `uninstall left a residual ${this.table.family} registration for ${SERVICE_NAME}\n` +
          `${residual.output}`,
      );
    }
    // A residual PROCESS is a separate failure from a residual registration,
    // and reporting only the registration would let a still-running orphan
    // through.
    const pids = [this.killedPid, this.recoveredPid].filter(Boolean);
    const alive = pids.filter((p) => this.pidAlive(p));
    const finalStatus = this.status();
    if (alive.length) {
      throw new StepFailure(`residual gateway processes still alive: ${alive.join(',')}`);
    }
    this.step(
      'drain-uninstall-clean',
      `${shellish(this.core('gateway', 'drain', '--profile', PROFILE, '--budget-ms', '15000'))} ; ${shellish(this.core('gateway', 'uninstall', '--profile', PROFILE))}`,
      `${drain.output.trim()}\n${uninstall.output.trim()}\n` +
        `$ ${shellish(this.table.residual())}\nresidual_status=${residual.status}\n${residual.output}\n` +
        `residual_pids_alive=${alive.length}\nfinal_status=${finalStatus.raw.output.trim()}`,
    );
  }

  // F24-GWP-M1: the receipt's headline is taken from the SAME snapshot as its
  // breakdown, at receipt-write time — NOT from `this.counts`, which is the
  // step-13 freeze and is stale by four steps. `this.counts` survives only as
  // the step-13 record inside `steps[]`, where being a point-in-time reading is
  // what it is for.
  receipt() {
    if (this.steps.length !== CANONICAL_STEPS.length) {
      throw new StepFailure(
        `journey recorded ${this.steps.length} steps, the canonical list has ${CANONICAL_STEPS.length}`,
      );
    }
    const final = this.snapshot();
    this.finalSnapshot = final;
    return {
      schema: RECEIPT_SCHEMA,
      platform: this.args.platform,
      service_family: this.table.family,
      candidate_commit: this.candidateCommit,
      binary_version: this.binaryVersion,
      binary_sha256: this.binarySha256,
      driver_commit: driverCommit(),
      started_at: this.startedAt,
      finished_at: new Date().toISOString(),
      arrival_source: ARRIVAL_SOURCE,
      counts: final.counts,
      adapter_coverage: final.coverage,
      steps: this.steps,
    };
  }

  // The receipt is written before this runs, because a run that duplicated is
  // exactly the run whose evidence must survive. What this refuses is the
  // CLAIM, not the record: a dirty final snapshot means the journey does not
  // get to print `JOURNEY COMPLETE`, and the receipt it left behind is one the
  // Rust verifier rejects as `DirtyReconciliation`.
  //
  // Deliveries that land after step 13 are the F24-GWP-H1 shape — a restarted
  // Windows runtime re-firing already-fired cron jobs at the Task Scheduler
  // `PT1M` repetition boundary. Before this gate existed the journey exited 0
  // on such a run.
  assertFinalReconciliation(receipt) {
    const c = receipt.counts;
    const snap = this.finalSnapshot;
    const stepThirteen = this.counts;
    if (snap && snap.unattributed !== 0) {
      throw new StepFailure(
        `${snap.unattributed} arrival(s) in the headline are attributed to no adapter endpoint; ` +
          'the headline and the breakdown cannot be reconciled',
      );
    }
    if (c.duplicates !== 0 || c.losses !== 0) {
      throw new StepFailure(
        `FINAL delivery reconciliation is not clean: submitted=${c.submitted} ` +
          `arrived=${c.arrived} unique=${c.unique} duplicates=${c.duplicates} ` +
          `losses=${c.losses}\n` +
          `step-13 reconcile had read arrived=${stepThirteen.arrived} ` +
          `duplicates=${stepThirteen.duplicates}; ` +
          `${c.arrived - stepThirteen.arrived} arrival(s) landed after it. ` +
          'The receipt was written and records the true final counts.',
      );
    }
  }

  cleanup() {
    try {
      this.sink?.kill('SIGTERM');
    } catch {
      /* the sink is a child; a failure to signal it is not a journey result */
    }
  }
}

export function driverCommit() {
  const r = run(['git', '-C', path.dirname(HERE), 'rev-parse', 'HEAD']);
  const sha = r.output.trim();
  if (r.status !== 0 || !/^[0-9a-f]{40}$/.test(sha)) {
    throw new StepFailure(`cannot read the driver's own commit: ${r.output}`);
  }
  return sha;
}

function main() {
  let journey = null;
  try {
    const args = parseArgs(process.argv.slice(2));
    if (args.platform !== hostPlatform()) {
      throw new StepFailure(
        `--platform ${args.platform} but this host is ${hostPlatform()}; ` +
          'a macOS journey runs on macOS or it does not happen',
      );
    }
    journey = new Journey(args);
    journey.preflightClean();
    journey.binaryIdentity();
    journey.profileSetup();
    journey.sinkStart();
    journey.gatewayInstall();
    journey.gatewayStart();
    journey.statusRunning();
    journey.automationAdd();
    journey.deliveriesSubmit();
    journey.arrivalBeforeKill();
    journey.hardKill();
    journey.platformRecover();
    journey.deliveryReconcile();
    journey.upgradeInPlace();
    journey.rollback();
    journey.redactionCanary();
    journey.drainUninstallClean();

    const receipt = journey.receipt();
    const receiptPath = path.join(journey.runDir, `${args.platform}-receipt.json`);
    fs.writeFileSync(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
    // Written first, graded second — see `assertFinalReconciliation`.
    journey.assertFinalReconciliation(receipt);
    process.stdout.write(`JOURNEY COMPLETE platform=${args.platform} receipt=${receiptPath}\n`);
    journey.cleanup();
    process.exit(0);
  } catch (error) {
    process.stderr.write(`JOURNEY FAILED: ${error.message}\n`);
    if (journey) {
      // A failed journey still writes its raw capture. The steps that DID run
      // are the evidence for where it stopped, and discarding them would leave
      // a red with nothing behind it.
      try {
        fs.mkdirSync(journey.runDir, { recursive: true });
        fs.writeFileSync(
          path.join(journey.runDir, `${journey.args.platform}-raw.txt`),
          `${journey.raw.join('\n')}\n### FAILED\n${error.message}\n`,
          { mode: 0o600 },
        );
        fs.writeFileSync(
          path.join(journey.runDir, `${journey.args.platform}-failure.json`),
          `${JSON.stringify(
            {
              platform: journey.args.platform,
              completed_steps: journey.steps.map((s) => s.name),
              failed_at: CANONICAL_STEPS[journey.steps.length] ?? '(after last step)',
              error: error.message,
            },
            null,
            2,
          )}\n`,
        );
      } catch {
        /* nothing further to do; the message above is the result */
      }
      journey.cleanup();
    }
    process.exit(1);
  }
}

export function hostPlatform() {
  if (os.platform() === 'darwin') return 'macos';
  if (os.platform() === 'win32') return 'windows';
  if (os.platform() === 'linux') return 'linux';
  return os.platform();
}

// `node scripts/f24-journey.mjs` runs the journey; `import` from the test does
// not, so the test can exercise the driver's own logic without hardware.
if (process.argv[1] && path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url))) {
  main();
}

export { Journey, StepFailure, parseArgs, PLATFORMS, shellish };
