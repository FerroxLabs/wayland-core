#!/usr/bin/env node
// Live inbound-attachment drive for the MS Teams adapter, through the SHIPPED
// `wayland-core` binary, against a hermetic Bot Framework fixture.
//
// WHY A DEDICATED DRIVER RATHER THAN A LEG IN f24-inbound.mjs. Teams is the
// only adapter in this program whose inbound gate is an asymmetric-signature
// check: `ingest_webhook` fetches a JWKS over the network and validates an
// RS256 JWT against it. Every other webhook adapter uses a shared secret the
// driver already holds. So the fixture here is not "one more signer" — it is an
// OpenID metadata document, a JWKS, an OAuth2 token endpoint and a Connector
// send sink, plus an RSA keypair minted per run. Bolting that onto a 2900-line
// shared file that four concurrent lanes also touch buys nothing and risks a
// merge conflict in the one file every lane needs.
//
// WHAT IS REAL AND WHAT IS FIXTURE. Real: the `wayland-core` binary, its
// webhook host, the msteams adapter's JWT validation, its Activity parse, the
// media normaliser, the dispatch kernel, the turn. Fixture: Microsoft's four
// endpoints, and the model. NO vendor credential is used or needed — the
// fixture IS the API. The RSA key is generated in-process and never leaves it.
//
// WHAT THIS MEASURES. Four clauses, each with a one-variable negative control
// that must redden:
//
//   M1 turn        a signed Teams activity with no attachments reaches a turn
//                  (the rig itself works, so a later zero means something)
//   M2 attach      an activity carrying a file attachment reaches a turn whose
//                  PROMPT names that attachment — kind, type and reference.
//                  Control: byte-identical activity minus `attachments[]`
//                  must produce a turn with NO attachment block.
//   M3 no-phantom  an activity whose only `attachments[]` entries are the
//                  message's own HTML rendering and an Adaptive Card must
//                  produce NO attachment block. Control: M2 (same matcher,
//                  same run, opposite verdict).
//   M4 auth        an activity signed by a DIFFERENT key must be refused, and
//                  must produce no turn. Control: M1/M2 (same endpoint, same
//                  shape, valid key, accepted).
//
// The observation point for M2/M3 is the LLM fixture's journal, whose
// `user_text` is verbatim what `build_turn_prompt` handed the model. Asserting
// on the Rust struct would prove the parse; asserting here proves the parse
// SURVIVED to the agent, which is the class of defect this program keeps
// finding (a populated field nothing downstream reads).

import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const HERE = path.dirname(new URL(import.meta.url).pathname);

// Ports. Four other lanes run concurrently and 18787 (f24-inbound's default)
// and 18211 (discord's) are taken. These are deliberately far from both.
const WEBHOOK_PORT = Number(process.env.F24_WEBHOOK_PORT ?? 19631);
const BF_PORT = Number(process.env.F24_MSTEAMS_BF_PORT ?? 19632);
const LLM_PORT = Number(process.env.F24_MSTEAMS_LLM_PORT ?? 19633);

const APP_ID = 'f24msteams-app-id';
const APP_PASSWORD = 'f24msteams-not-a-real-secret';
const CHANNEL = 'f24msteams';
const SENDER = '29:f24msteams-allowed-user';
const CONV = '19:f24msteams@thread.v2';
const BF_ISSUER = 'https://api.botframework.com';
// The fixture's Connector base. The adapter round-trips it through
// `conversation_id` as `{serviceUrl}|{conversationId}`, so it must end in `/`.
const SERVICE_URL = `http://127.0.0.1:${BF_PORT}/amer/`;

function sleep(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}
function note(s) {
  process.stdout.write(`[msteams] ${s}\n`);
}

// ── JWT (RS256, minted per run) ──────────────────────────────────────────────

function b64url(buf) {
  return Buffer.from(buf).toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/// Sign a Bot-Framework-shaped JWT. `kid` goes in the header because
/// `BotFrameworkAuth::validate` resolves the key by `kid` and refuses a token
/// without one.
function signJwt({ privateKey, kid, claims }) {
  const header = b64url(JSON.stringify({ alg: 'RS256', typ: 'JWT', kid }));
  const payload = b64url(JSON.stringify(claims));
  const signingInput = `${header}.${payload}`;
  const sig = crypto.sign('RSA-SHA256', Buffer.from(signingInput), privateKey);
  return `${signingInput}.${b64url(sig)}`;
}

function claimsFor(overrides = {}) {
  const now = Math.floor(Date.now() / 1000);
  return {
    iss: BF_ISSUER,
    aud: APP_ID,
    iat: now - 10,
    nbf: now - 10,
    exp: now + 600,
    serviceurl: SERVICE_URL,
    ...overrides,
  };
}

// ── the Bot Framework fixture (out of process — see f24-msteams-fixture.mjs) ─
//
// The keypairs are minted HERE and only the public JWKS is written to disk, so
// no signable key ever reaches the filesystem. The fixture runs as its own OS
// process because this driver sleeps synchronously (`Atomics.wait`), and a
// synchronously-blocked main thread cannot accept a connection — the first
// version of this file shared a process with the fixture and every request
// silently went unanswered.

class BotFrameworkFixture {
  constructor(dir, children) {
    this.dir = dir;
    this.children = children;
    this.journalPath = path.join(dir, 'bf-sink.jsonl');
    this.jwksPath = path.join(dir, 'bf-jwks.json');

    // The trusted keypair, and a SECOND one that is never published. M4 signs
    // with the second: a token that is well-formed, correctly claimed, and
    // signed by a key the JWKS does not contain.
    this.trusted = crypto.generateKeyPairSync('rsa', { modulusLength: 2048 });
    this.rogue = crypto.generateKeyPairSync('rsa', { modulusLength: 2048 });
    this.kid = 'f24msteams-kid-1';
  }

  jwks() {
    const jwk = this.trusted.publicKey.export({ format: 'jwk' });
    return { keys: [{ kty: jwk.kty, kid: this.kid, use: 'sig', alg: 'RS256', n: jwk.n, e: jwk.e }] };
  }

  start() {
    fs.writeFileSync(this.jwksPath, JSON.stringify(this.jwks()));
    const child = spawn(
      process.execPath,
      [path.join(HERE, 'f24-msteams-fixture.mjs'), '--port', String(BF_PORT), '--journal', this.journalPath, '--jwks', this.jwksPath],
      { stdio: ['ignore', 'pipe', 'pipe'] },
    );
    this.children.push(child);
    child.stdout.on('data', (d) => note(`bf: ${String(d).trim()}`));
    child.stderr.on('data', (d) => note(`bf-err: ${String(d).trim()}`));
    this.child = child;
    return this;
  }

  /// Block until the fixture answers its own health endpoint. Without this the
  /// binary can start, fail to mint a token against a socket that is not up
  /// yet, and the run measures a race rather than the adapter.
  waitReady(secs = 30) {
    for (let i = 0; i < secs; i += 1) {
      const r = spawnSync(
        process.execPath,
        ['-e', `fetch('http://127.0.0.1:${BF_PORT}/_bf/health').then(async r=>process.stdout.write('BF '+r.status)).catch(e=>{process.stdout.write('DOWN');process.exit(1);})`],
        { encoding: 'utf8', timeout: 10_000 },
      );
      if (r.status === 0 && `${r.stdout}`.includes('BF 200')) {
        note(`bf fixture ready after ${i}s`);
        return true;
      }
      process.stdout.write(`[msteams] waiting for bf fixture: ${i}s\n`);
      sleep(1000);
    }
    return false;
  }

  arrivals() {
    if (!fs.existsSync(this.journalPath)) return [];
    return fs
      .readFileSync(this.journalPath, 'utf8')
      .split('\n')
      .filter(Boolean)
      .map((l) => JSON.parse(l));
  }

  stop() {
    try {
      this.child?.kill('SIGTERM');
    } catch {
      /* already down */
    }
  }
}

// ── the run ──────────────────────────────────────────────────────────────────

class Run {
  constructor(args) {
    this.args = args;
    this.dir = fs.mkdtempSync(path.join(os.tmpdir(), 'f24-msteams-'));
    this.home = path.join(this.dir, 'home');
    this.llmJournal = path.join(this.dir, 'llm.jsonl');
    this.vaultPassphrase = crypto.randomBytes(16).toString('hex');
    this.children = [];
    this.results = [];
    this.notes = [];
  }

  note(s) {
    this.notes.push(s);
    note(s);
  }

  record(clause, ok, detail) {
    this.results.push({ clause, ok, detail });
    process.stdout.write(`[msteams] ${ok ? 'PASS' : 'FAIL'} ${clause} — ${detail}\n`);
  }

  writeConfig() {
    fs.mkdirSync(path.join(this.home, 'channels'), { recursive: true });

    fs.writeFileSync(
      path.join(this.home, 'credentials.toml'),
      ['[secrets]', `"msteams.${CHANNEL}.app_id" = "${APP_ID}"`, `"msteams.${CHANNEL}.app_password" = "${APP_PASSWORD}"`, ''].join('\n'),
      { mode: 0o600 },
    );

    fs.writeFileSync(
      path.join(this.home, 'config.toml'),
      [
        '[default]',
        'provider = "f24fixture"',
        '',
        '[providers.f24fixture]',
        'provider = "openai"',
        'model = "f24c3-fixture"',
        'api_key = "f24-not-a-real-key"',
        `base_url = "http://127.0.0.1:${LLM_PORT}/v1"`,
        '',
        '[inbound_webhook]',
        'enabled = true',
        `bind = "127.0.0.1:${WEBHOOK_PORT}"`,
        `public_base_url = "http://127.0.0.1:${WEBHOOK_PORT}"`,
        '',
      ].join('\n'),
      { mode: 0o600 },
    );

    // The two endpoint overrides are the whole reason this run is possible
    // without a vendor credential. They default to the live Microsoft hosts.
    fs.writeFileSync(
      path.join(this.home, 'channels', `${CHANNEL}.toml`),
      [
        `name = "${CHANNEL}"`,
        'platform = "msteams"',
        'enabled = true',
        '',
        '[options]',
        `credential_handle_app_id = "msteams.${CHANNEL}.app_id"`,
        `credential_handle_app_password = "msteams.${CHANNEL}.app_password"`,
        `service_url = "${SERVICE_URL}"`,
        `token_url = "http://127.0.0.1:${BF_PORT}/token"`,
        `openid_metadata_url = "http://127.0.0.1:${BF_PORT}/openid"`,
        '',
        '[inbound]',
        'dm = "allowlist"',
        `dm_allowlist = ["${SENDER}"]`,
        'group = "disabled"',
        'require_mention = false',
        'tools = "conversational"',
        '',
      ].join('\n'),
    );
  }

  startLlm() {
    const child = spawn(process.execPath, [path.join(HERE, 'f24-llm-fixture.mjs'), '--port', String(LLM_PORT), '--journal', this.llmJournal], {
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    this.children.push(child);
    child.stdout.on('data', (d) => this.note(`llm: ${String(d).trim()}`));
    child.stderr.on('data', (d) => this.note(`llm-err: ${String(d).trim()}`));
  }

  startBinary() {
    const logPath = path.join(this.dir, 'core.log');
    fs.writeFileSync(logPath, '');
    const fd = fs.openSync(logPath, 'a');
    // `gateway run` in the FOREGROUND: a detached gateway re-execs and this
    // driver would lose the child it must reap.
    const child = spawn(this.args.binary, ['gateway', 'run'], {
      stdio: ['pipe', fd, fd],
      env: {
        ...process.env,
        WAYLAND_HOME: this.home,
        // An isolated profile with no vault passphrase refuses EVERY turn with
        // "Session persistence authority unavailable" — a host-wide credentials
        // posture, not a channel fact. Minted for this run; not a vendor secret.
        WAYLAND_VAULT_PASSPHRASE: this.vaultPassphrase,
        RUST_LOG: 'wcore_agent::bootstrap=info,wcore_agent::channel_inbound=debug,wcore_channels=debug,wcore_channel_msteams=debug',
      },
      windowsHide: true,
    });
    this.coreChild = child;
    this.children.push(child);
    this.coreLog = logPath;
  }

  waitForWebhookHost() {
    for (let i = 0; i < 90; i += 1) {
      const r = spawnSync(
        process.execPath,
        ['-e', `fetch('http://127.0.0.1:${WEBHOOK_PORT}/healthz').then(async r=>process.stdout.write('HEALTHZ '+r.status)).catch(e=>{process.stdout.write('DOWN '+e.message);process.exit(1);})`],
        { encoding: 'utf8', timeout: 15_000 },
      );
      const out = `${r.stdout ?? ''}${r.stderr ?? ''}`;
      if (r.status === 0 && out.includes('HEALTHZ 200')) {
        this.note(`webhook host up after ${i}s`);
        return true;
      }
      process.stdout.write(`[msteams] waiting for webhook host: ${i}s ${new Date().toISOString()}\n`);
      sleep(1000);
    }
    this.note(`webhook host never bound 127.0.0.1:${WEBHOOK_PORT} after 90s`);
    this.coreLogTail = fs.readFileSync(this.coreLog, 'utf8').slice(-6000);
    return false;
  }

  /// POST a Bot Framework Activity to the binary's webhook host.
  post({ token, activity }) {
    const r = spawnSync(
      process.execPath,
      [
        '-e',
        `fetch(${JSON.stringify(`http://127.0.0.1:${WEBHOOK_PORT}/webhooks/${CHANNEL}`)},{method:'POST',headers:{'content-type':'application/json','authorization':${JSON.stringify(`Bearer ${token}`)}},body:${JSON.stringify(JSON.stringify(activity))}})
           .then(async r=>process.stdout.write('STATUS '+r.status+' '+(await r.text()).slice(0,200)))
           .catch(e=>{process.stdout.write('POST_ERR '+e.message);process.exit(1);})`,
      ],
      { encoding: 'utf8', timeout: 30_000 },
    );
    const out = `${r.stdout ?? ''}${r.stderr ?? ''}`.trim();
    const m = /STATUS (\d+)/.exec(out);
    return { status: m ? Number(m[1]) : null, output: out };
  }

  turns() {
    if (!fs.existsSync(this.llmJournal)) return [];
    return fs
      .readFileSync(this.llmJournal, 'utf8')
      .split('\n')
      .filter(Boolean)
      .map((l) => JSON.parse(l));
  }

  /// The turn whose prompt carries `correlation`, waited for up to `budgetMs`.
  awaitTurn(correlation, budgetMs = 45_000) {
    const deadline = Date.now() + budgetMs;
    let i = 0;
    while (Date.now() < deadline) {
      const hit = this.turns().find((t) => (t.user_text ?? '').includes(correlation));
      if (hit) return hit;
      i += 1;
      if (i % 5 === 0) process.stdout.write(`[msteams] awaiting turn ${correlation}: ${i}s ${new Date().toISOString()}\n`);
      sleep(1000);
    }
    return null;
  }

  stop() {
    for (const c of this.children) {
      try {
        c.kill('SIGTERM');
      } catch {
        /* already gone */
      }
    }
    sleep(1500);
    for (const c of this.children) {
      try {
        c.kill('SIGKILL');
      } catch {
        /* already gone */
      }
    }
  }
}

// ── activity builders ────────────────────────────────────────────────────────

function baseActivity({ id, text }) {
  return {
    type: 'message',
    id,
    text,
    serviceUrl: SERVICE_URL,
    timestamp: new Date().toISOString(),
    from: { id: SENDER, name: 'F24 Teams User', role: 'user' },
    recipient: { id: '28:f24msteams-bot' },
    conversation: { id: CONV, conversationType: 'personal', isGroup: false, name: 'F24 / Wayland' },
  };
}

// A real Teams file upload: the vendor WRAPPER content type plus contentUrl and
// name. Classification must come from the name, not the wrapper.
const FILE_ATTACHMENT = {
  contentType: 'application/vnd.microsoft.teams.file.download.info',
  contentUrl: 'https://contoso.sharepoint.com/personal/f24/quarterly-report.pdf',
  name: 'quarterly-report.pdf',
  content: { downloadUrl: 'https://contoso.sharepoint.com/download/opaque', fileType: 'pdf', uniqueId: 'f24-unique' },
};

// What Teams stamps onto EVERY formatted message, plus an Adaptive Card.
// Neither is a file; neither carries a contentUrl.
const NON_FILE_ATTACHMENTS = [
  { contentType: 'text/html', content: '<p>formatted <b>message</b></p>' },
  { contentType: 'application/vnd.microsoft.card.adaptive', content: { type: 'AdaptiveCard', version: '1.4', body: [] } },
];

// ── the matcher, and its self-test ───────────────────────────────────────────
//
// LANE-BRIEF §6b-ii: an instrument gets a self-test with THREE assertions —
// known-positive passes, known-negative fails, and the naive matcher this
// replaces would have MISSED the positive. The naive matcher here is
// `text.includes('attachments received')`, which a console-wrapped or
// re-indented log line defeats, and which cannot tell WHICH attachment arrived.

/// The attachment block `build_turn_prompt` appends, parsed into entries.
/// Returns `null` when there is no block at all — which is a different fact
/// from "a block with zero entries" and must not be conflated with it.
export function parseAttachmentBlock(prompt) {
  if (typeof prompt !== 'string') return null;
  const start = prompt.indexOf('[attachments received with this message:');
  if (start === -1) return null;
  const block = prompt.slice(start);
  const entries = [];
  // `\n  N. Kind (type) — rest`. Tolerates arbitrary intervening whitespace and
  // a hard-wrapped line, because the console wrapping that defeated a previous
  // lane's matcher inserts exactly that.
  const re = /\n\s*(\d+)\.\s+(\w+)\s+\(([^)]*)\)\s+—\s+([^\n\]]+)/g;
  let m;
  while ((m = re.exec(block)) !== null) {
    entries.push({ index: Number(m[1]), kind: m[2], type: m[3].trim(), rest: m[4].trim() });
  }
  return entries;
}

function selfTestMatcher() {
  const failures = [];

  // 1. known-positive
  const positive =
    'look at this\n\n[attachments received with this message:\n  1. Document (unknown type) — https://contoso.sharepoint.com/personal/f24/quarterly-report.pdf]';
  const p = parseAttachmentBlock(positive);
  if (!p || p.length !== 1 || p[0].kind !== 'Document') {
    failures.push(`known-positive not parsed: ${JSON.stringify(p)}`);
  }

  // 2. known-negative — a prompt with no block must yield null, NOT [].
  const negative = 'just text, no media here at all';
  if (parseAttachmentBlock(negative) !== null) {
    failures.push('known-negative produced a block');
  }

  // 3. the naive matcher would have MISSED this one. A console-wrapped prompt
  //    splits the phrase; `includes('attachments received')` returns false
  //    while the block is plainly present and parseable.
  const wrapped =
    'look\n\n[attachments received with this message:\n  1.\n     Image (image/png) — https://x/y.png]';
  const naive = wrapped.includes('attachments received with this message:\n  1. Image');
  const repaired = parseAttachmentBlock(wrapped);
  if (naive) failures.push('assertion 3 is vacuous: the naive matcher did NOT miss the wrapped case');
  if (!repaired || repaired.length !== 1 || repaired[0].kind !== 'Image') {
    failures.push(`repaired matcher missed the wrapped case: ${JSON.stringify(repaired)}`);
  }

  return failures;
}

// ── main ─────────────────────────────────────────────────────────────────────

function parseArgs(argv) {
  const out = { binary: null, out: null, selfTestOnly: false };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--binary') out.binary = argv[++i];
    else if (a === '--out') out.out = argv[++i];
    else if (a === '--self-test-only') out.selfTestOnly = true;
    else {
      process.stderr.write(`f24-msteams-attach: unknown argument ${a}\n`);
      process.exit(2);
    }
  }
  return out;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));

  const selfTestFailures = selfTestMatcher();
  note(`matcher self-test: ${selfTestFailures.length === 0 ? 'PASS (3/3)' : `FAIL — ${selfTestFailures.join('; ')}`}`);
  if (selfTestFailures.length > 0) process.exit(3);
  if (args.selfTestOnly) {
    note('self-test only; exiting 0');
    return;
  }
  if (!args.binary) {
    process.stderr.write('f24-msteams-attach: --binary is required\n');
    process.exit(2);
  }

  const run = new Run(args);
  const bf = new BotFrameworkFixture(run.dir, run.children).start();
  note(`dir=${run.dir} webhook=${WEBHOOK_PORT} bf=${BF_PORT} llm=${LLM_PORT}`);

  const goodToken = () => signJwt({ privateKey: bf.trusted.privateKey, kid: bf.kid, claims: claimsFor() });
  const rogueToken = () => signJwt({ privateKey: bf.rogue.privateKey, kid: bf.kid, claims: claimsFor() });

  try {
    run.writeConfig();
    run.startLlm();
    // Both fixtures must be ANSWERING before the binary starts: `start()` mints
    // a Connector token fail-fast, and a channel whose start() fails is skipped
    // for the whole run. A race here would be reported as an adapter defect.
    const bfReady = bf.waitReady();
    if (!bfReady) {
      run.record('M0-fixture', false, `bf fixture never answered 127.0.0.1:${BF_PORT}`);
    } else {
      run.note('bf fixture answering; starting binary');
    }
    run.startBinary();

    if (!run.waitForWebhookHost()) {
      run.record('M1-turn', false, 'webhook host never bound; every clause is unmeasurable');
      run.record('M2-attach', false, 'webhook host never bound');
      run.record('M3-no-phantom', false, 'webhook host never bound');
      run.record('M4-auth', false, 'webhook host never bound');
    } else {
      const tag = crypto.randomBytes(4).toString('hex');

      // ── M1: a plain signed activity reaches a turn ─────────────────────
      const c1 = `f24c3-msteams-plain-${tag}`;
      const r1 = run.post({ token: goodToken(), activity: { ...baseActivity({ id: `${tag}.1`, text: `hello ${c1}` }) } });
      const t1 = run.awaitTurn(c1);
      run.record('M1-turn', r1.status === 200 && t1 !== null, `POST=${r1.status} turn=${t1 ? 'yes' : 'NO'} | ${r1.output.slice(0, 120)}`);

      // NEGATIVE CONTROL for M2, run FIRST so it cannot be contaminated: the
      // byte-identical activity minus `attachments[]` must carry NO block.
      const b1 = t1 ? parseAttachmentBlock(t1.user_text) : undefined;
      run.record(
        'M2-control-no-attachments',
        t1 !== null && b1 === null,
        t1 === null ? 'no turn to inspect' : `block=${JSON.stringify(b1)} want=null`,
      );

      // ── M2: a file attachment reaches the agent's prompt ───────────────
      const c2 = `f24c3-msteams-file-${tag}`;
      const a2 = { ...baseActivity({ id: `${tag}.2`, text: `see this ${c2}` }), attachments: [FILE_ATTACHMENT] };
      const r2 = run.post({ token: goodToken(), activity: a2 });
      const t2 = run.awaitTurn(c2);
      const b2 = t2 ? parseAttachmentBlock(t2.user_text) : null;
      const okKind = Array.isArray(b2) && b2.length === 1 && b2[0].kind === 'Document';
      const okRef = Array.isArray(b2) && b2.length === 1 && b2[0].rest === FILE_ATTACHMENT.contentUrl;
      // The vendor wrapper must NOT be echoed as a media type.
      const okType = Array.isArray(b2) && b2.length === 1 && !b2[0].type.includes('vnd.microsoft.teams.file');
      run.record(
        'M2-attach',
        r2.status === 200 && okKind && okRef && okType,
        `POST=${r2.status} block=${JSON.stringify(b2)} kind_ok=${okKind} ref_ok=${okRef} wrapper_suppressed=${okType}`,
      );

      // ── M3: inline HTML / Adaptive Card are not attachments ────────────
      const c3 = `f24c3-msteams-nophantom-${tag}`;
      const a3 = { ...baseActivity({ id: `${tag}.3`, text: `formatted ${c3}` }), attachments: NON_FILE_ATTACHMENTS };
      const r3 = run.post({ token: goodToken(), activity: a3 });
      const t3 = run.awaitTurn(c3);
      const b3 = t3 ? parseAttachmentBlock(t3.user_text) : null;
      run.record(
        'M3-no-phantom',
        r3.status === 200 && t3 !== null && b3 === null,
        `POST=${r3.status} turn=${t3 ? 'yes' : 'NO'} block=${JSON.stringify(b3)} want=null`,
      );

      // ── M4: a token signed by an unpublished key is refused ────────────
      const c4 = `f24c3-msteams-rogue-${tag}`;
      const a4 = { ...baseActivity({ id: `${tag}.4`, text: `rogue ${c4}` }), attachments: [FILE_ATTACHMENT] };
      const r4 = run.post({ token: rogueToken(), activity: a4 });
      // Give a turn the same budget the accepted legs got, so "no turn" is a
      // measurement rather than impatience.
      const t4 = run.awaitTurn(c4, 15_000);
      // ANTI-VACUITY. On a dead rig every POST is non-200 and no turn ever
      // happens, so `status != 200 && no turn` passes for free — the first live
      // run of this driver scored M4 PASS against a binary whose msteams
      // channel had failed to start and answered 400 to everything. A rejection
      // is only evidence if the SAME endpoint, in the SAME run, accepted a
      // valid token. That is what `acceptsValid` requires.
      const acceptsValid = t1 !== null || t2 !== null;
      const kidsServed = bf.arrivals().some((a) => a.kind === 'jwks');
      run.record(
        'M4-auth',
        acceptsValid && kidsServed && r4.status !== null && r4.status !== 200 && t4 === null,
        `POST=${r4.status} (want non-200) turn=${t4 ? 'LEAKED' : 'none'} ` +
          `accepts_valid_in_same_run=${acceptsValid} jwks_was_fetched=${kidsServed} | ${r4.output.slice(0, 160)}`,
      );

      run.note(`bf fixture journal: ${JSON.stringify(bf.arrivals().map((a) => a.kind))}`);
      run.note(`turns observed: ${run.turns().length}`);
    }
  } finally {
    run.stop();
    bf.stop();
  }

  const summary = {
    at: new Date().toISOString(),
    host: os.hostname(),
    ports: { webhook: WEBHOOK_PORT, bf: BF_PORT, llm: LLM_PORT },
    binary: args.binary,
    matcher_self_test: 'PASS (3/3)',
    results: run.results,
    passed: run.results.filter((r) => r.ok).length,
    total: run.results.length,
    notes: run.notes,
    bf_journal: bf.arrivals(),
    turns: run.turns(),
    core_log_tail: run.coreLogTail ?? (run.coreLog && fs.existsSync(run.coreLog) ? fs.readFileSync(run.coreLog, 'utf8').slice(-6000) : null),
  };
  if (args.out) {
    fs.mkdirSync(path.dirname(path.resolve(args.out)), { recursive: true });
    fs.writeFileSync(args.out, `${JSON.stringify(summary, null, 2)}\n`);
    note(`wrote ${args.out}`);
  }
  note(`RESULT ${summary.passed}/${summary.total} clauses passed`);
  process.exit(summary.passed === summary.total ? 0 : 1);
}

main().catch((e) => {
  process.stderr.write(`f24-msteams-attach: ${e?.stack ?? e}\n`);
  process.exit(4);
});
