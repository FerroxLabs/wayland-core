#!/usr/bin/env node
/**
 * f24-c3-reload-control.mjs — the control that decides whether the reload
 * finding is a PRODUCT defect or a bad fixture config.
 *
 * Run 3 of `f24-c3-clauses.mjs` measured: a channel added by `channel reload`
 * is registered (2→3), reports `healthy`, accepts its webhook with HTTP 200 —
 * and every message to it is denied `sender not in dm allowlist`, producing no
 * turn and no reply.
 *
 * Two hypotheses explain that identically:
 *
 *   H1 (product):  `channel reload` re-registers ADAPTERS but never reloads the
 *                  INBOUND ACCESS POLICY map, so the new channel has no policy
 *                  and every sender falls through to deny.
 *   H2 (fixture):  the third channel's config is simply wrong and would be
 *                  denied under any lifecycle.
 *
 * **Reporting H1 without excluding H2 would be a fabricated HIGH against
 * working code.** A lane on this program traced a dedupe FAIL to its own 90 s
 * replay against a 60 s TTL and would have filed exactly that.
 *
 * This control discriminates them with ONE variable changed. Same home, same
 * three config files on disk, same binary, same fixtures, same sender, same
 * token shape. The only difference is the lifecycle that brought the third
 * channel into the running process:
 *
 *   run 3    : started with 2, third added by `channel reload`   → measured DENY
 *   control  : started with 3 already on disk (fresh start)      → ?
 *
 * If the control ADMITS, H2 is excluded and the defect is the reload path.
 * If the control DENIES, the config is at fault, H1 is disproved, and this lane
 * reports no product defect. Either answer is a result.
 *
 * Exit: 0 = control ran and is readable, 1 = control could not be run.
 * The VERDICT is printed, not encoded in the exit status — a caller that wants
 * the answer must read it, which is deliberate.
 */

import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';

const WEBHOOK_PORT = 18899;
const SINK_PORT = 18898;
const LLM_PORT = 18897;

function sleep(ms) {
  const sab = new SharedArrayBuffer(4);
  Atomics.wait(new Int32Array(sab), 0, 0, ms);
}

function run(argv, opts = {}) {
  const r = spawnSync(argv[0], argv.slice(1), {
    encoding: 'utf8',
    timeout: opts.timeout ?? 60_000,
    env: opts.env ?? process.env,
  });
  return { status: r.status, stdout: r.stdout ?? '', stderr: r.stderr ?? '' };
}

const args = { binary: null, home: null, runDir: null };
for (let i = 2; i < process.argv.length; i += 1) {
  const a = process.argv[i];
  if (a === '--binary') args.binary = process.argv[++i];
  else if (a === '--home') args.home = process.argv[++i];
  else if (a === '--run-dir') args.runDir = process.argv[++i];
  else {
    process.stderr.write(`unknown arg ${a}\n`);
    process.exit(1);
  }
}
if (!args.binary || !args.home || !args.runDir) {
  process.stderr.write('--binary, --home and --run-dir are all required\n');
  process.exit(1);
}

// ── read back the EXACT config run 3 used. Not a re-derivation. ─────────────
const channelsDir = path.join(args.home, 'channels');
const onDisk = fs.readdirSync(channelsDir).filter((f) => f.endsWith('.toml')).sort();
process.stdout.write(`[control] channels on disk: ${JSON.stringify(onDisk)}\n`);
if (onDisk.length !== 3) {
  process.stdout.write(
    `[control] ABORT: expected the 3 configs run 3 left behind, found ${onDisk.length}. ` +
      `The control must reuse run 3's exact configs or it changes two variables.\n`,
  );
  process.exit(1);
}

const credsRaw = fs.readFileSync(path.join(args.home, 'credentials.toml'), 'utf8');
const sigMatch = /"slack\.f24c3fin\.signing_secret" = "([0-9a-f]+)"/.exec(credsRaw);
if (!sigMatch) {
  process.stdout.write('[control] ABORT: cannot recover the run signing secret from the home\n');
  process.exit(1);
}
const signingSecret = sigMatch[1];

// The vault passphrase was minted per run and is NOT on disk, so the control
// mints its own home-compatible one. Credentials were written plaintext-0600 by
// the driver, so a fresh passphrase still opens them.
const vaultPassphrase = `f24c3fin-control-${crypto.randomBytes(16).toString('hex')}`;

fs.mkdirSync(args.runDir, { recursive: true });

// ── fixtures, as separate OS processes (an in-process fixture cannot accept a
//    connection while this driver blocks in Atomics.wait) ────────────────────
const scripts = path.dirname(new URL(import.meta.url).pathname);
const sinkJournal = path.join(args.runDir, 'arrivals.jsonl');
const llmJournal = path.join(args.runDir, 'turns.jsonl');
const children = [];

const sinkLog = fs.openSync(path.join(args.runDir, 'sink.log'), 'a');
children.push(
  spawn(process.execPath, [path.join(scripts, 'f24-sink.mjs'), '--port', String(SINK_PORT), '--journal', sinkJournal], {
    stdio: ['ignore', sinkLog, sinkLog],
  }),
);
const llmLog = fs.openSync(path.join(args.runDir, 'llm.log'), 'a');
children.push(
  spawn(process.execPath, [path.join(scripts, 'f24-llm-fixture.mjs'), '--port', String(LLM_PORT), '--journal', llmJournal], {
    stdio: ['ignore', llmLog, llmLog],
  }),
);
process.stdout.write('[control] fixtures spawned\n');
sleep(2500);

// ── start the gateway FRESH, with all three channels already on disk ────────
const gwLogPath = path.join(args.runDir, 'gateway.log');
fs.writeFileSync(gwLogPath, '');
const gwFd = fs.openSync(gwLogPath, 'a');
const gw = spawn(args.binary, ['gateway', 'run'], {
  stdio: ['pipe', gwFd, gwFd],
  env: {
    ...process.env,
    WAYLAND_HOME: args.home,
    WAYLAND_VAULT_PASSPHRASE: vaultPassphrase,
    RUST_LOG: 'wcore_agent::bootstrap=info,wcore_agent::channel_inbound=debug,wcore_channels=debug',
  },
});
children.push(gw);

let bound = false;
for (let i = 0; i < 90; i += 1) {
  const r = run([
    process.execPath,
    '-e',
    `fetch('http://127.0.0.1:${WEBHOOK_PORT}/healthz').then(async r=>{process.stdout.write('HZ '+r.status)}).catch(()=>{process.exit(1)})`,
  ], { timeout: 15_000 });
  if (r.status === 0 && r.stdout.includes('HZ 200')) {
    bound = true;
    process.stdout.write(`[control] webhook host bound after ${i}s\n`);
    break;
  }
  process.stdout.write(`[control] waiting for webhook host: ${i}s ${new Date().toISOString()}\n`);
  sleep(1000);
}

function submit(channelName, user, channelId) {
  const token = `f24c3-ctl-${channelName}-${crypto.randomBytes(4).toString('hex')}`;
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
  const sig = 'v0=' + crypto.createHmac('sha256', signingSecret).update(`v0:${ts}:${body}`).digest('hex');
  const url = `http://127.0.0.1:${WEBHOOK_PORT}/webhooks/${channelName}`;
  const r = run([
    process.execPath,
    '-e',
    `fetch(${JSON.stringify(url)},{method:'POST',headers:{'content-type':'application/json','x-slack-request-timestamp':'${ts}','x-slack-signature':'${sig}'},body:${JSON.stringify(body)}}).then(async r=>{process.stdout.write('ST '+r.status)}).catch(e=>{process.stdout.write('ERR '+e.message)})`,
  ], { timeout: 30_000 });
  const m = /ST (\d{3})/.exec(r.stdout || '');
  const http = m ? Number(m[1]) : null;
  process.stdout.write(`[control] submit ${channelName} token=${token} http=${http}\n`);
  return { token, http, accepted: http !== null && http >= 200 && http < 300 };
}

function journal(f) {
  if (!fs.existsSync(f)) return [];
  return fs
    .readFileSync(f, 'utf8')
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

let result = { bound, admitted: null, control_admitted: null };

if (bound) {
  // THE CONTROL'S OWN CONTROL. A pre-existing channel must still work in this
  // process, or a total failure here would masquerade as "the third is denied".
  const known = submit('f24finone', 'U24FINONE', 'DF24FINONE');
  // The channel under test.
  const third = submit('f24finthree', 'U24FINTHREE', 'DF24FINTHREE');

  for (let i = 0; i < 20; i += 1) {
    process.stdout.write(`[control] settling ${i * 3}s ${new Date().toISOString()}\n`);
    sleep(3000);
  }

  const turns = journal(llmJournal);
  const arrivals = journal(sinkJournal);
  const sawKnown = turns.some((t) => JSON.stringify(t).includes(known.token));
  const sawThird = turns.some((t) => JSON.stringify(t).includes(third.token));

  result = {
    bound,
    known_channel_token: known.token,
    known_channel_http: known.http,
    known_channel_reached_engine: sawKnown,
    third_channel_token: third.token,
    third_channel_http: third.http,
    third_channel_reached_engine: sawThird,
    turns_total: turns.length,
    arrivals_total: arrivals.length,
    turns_bytes: fs.existsSync(llmJournal) ? fs.statSync(llmJournal).size : 0,
    arrivals_bytes: fs.existsSync(sinkJournal) ? fs.statSync(sinkJournal).size : 0,
  };

  process.stdout.write('\n=== CONTROL RESULT ===\n');
  process.stdout.write(`known channel (f24finone)   reached engine: ${sawKnown}\n`);
  process.stdout.write(`third channel (f24finthree) reached engine: ${sawThird}\n`);
  process.stdout.write(`turns=${turns.length} arrivals=${arrivals.length}\n`);

  if (!sawKnown) {
    result.verdict =
      'UNREADABLE — the control\'s own control failed. Nothing works in this process, so ' +
      'nothing can be concluded about the third channel. NOT evidence for either hypothesis.';
  } else if (sawThird) {
    result.verdict =
      'H1 CONFIRMED — the SAME config that `channel reload` denies is ADMITTED after a fresh ' +
      'start. The config is fine; `channel reload` does not reload the inbound access policy.';
  } else {
    result.verdict =
      'H2 CONFIRMED — the third channel is denied after a fresh start too, so the config is at ' +
      'fault and there is NO product defect here. The run-3 FAIL is the fixture\'s.';
  }
  process.stdout.write(`VERDICT: ${result.verdict}\n`);
} else {
  result.verdict = 'INCOMPLETE — the webhook host never bound; the control did not run.';
  process.stdout.write(`VERDICT: ${result.verdict}\n`);
}

fs.writeFileSync(path.join(args.runDir, 'control-result.json'), JSON.stringify(result, null, 2));
for (const c of children) {
  try {
    c.kill('SIGKILL');
  } catch {
    /* gone */
  }
}
process.exit(0);
