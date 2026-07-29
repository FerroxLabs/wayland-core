#!/usr/bin/env node
// F24-C3 `media` clause — the POSITIVE direction, live.
//
// WHY THIS EXISTS. The predecessor lane (24-MEDIA-ACTIONS) proved `media` in the
// DEGRADED direction only: an inbound image on discord produced an honest "no
// vision backend is configured" notice in the turn prompt. That is correct
// behaviour, but it is NOT proof that enrichment works — every attachment
// producing "I cannot see this" would pass such a gate, which is the
// green-by-universal-denial failure in a new costume.
//
// This driver closes the other direction: a real audio attachment arrives on a
// reference adapter, is fetched by the connector, transcribed by a REAL
// transcription backend, and the RESULTING TEXT reaches the model.
//
// ── WHY TELEGRAM AND NOT DISCORD ──────────────────────────────────────────
// `wcore-channel-discord/src/rest.rs:337` pins media fetches to a CDN host
// allowlist (`cdn.discordapp.com`, `media.discordapp.net`) enforced at :349, so
// discord physically CANNOT fetch bytes from a localhost fixture. That is why
// the predecessor could only ever reach the degraded direction there.
// Telegram's `api::download_bytes` (`api.rs:898`) has no allowlist, and
// `TelegramConfig::api_base_url` feeds BOTH the bot-method base and the
// file-download base (`api.rs:658 file_download_url`) — so one fixture serves
// getUpdates, getFile and the media bytes.
//
// ── WHY THE CHAT PROVIDER IS `together` AND NOT `openai` ──────────────────
// THIS IS THE CRUX OF THE WHOLE MEASUREMENT. `build_transcription_backend`
// (`tool_backends/mod.rs`) resolves in order:
//     1. GROQ_API_KEY   2. OPENAI_API_KEY
//     3. the ACTIVE OpenAI-wire provider, from Config
//     4. FLUX_API_KEY
// and `openai_wire_media_base` (`tool_backends/shared.rs:56-77`) returns Some
// only for ProviderType::OpenAI and ProviderType::FluxRouter — `_ => None`.
//
// So if the local chat fixture is declared `provider = "openai"` (as the
// predecessor's harness declared it), ARM 3 CAPTURES TRANSCRIPTION and points it
// at the local chat fixture, which serves no /audio/transcriptions. Declaring it
// `together` — a Tier-2 OpenAI-compatible type (`config.rs:2415`) — keeps chat on
// the OpenAI wire to the local fixture while making arm 3 return None, so arm 4
// resolves transcription to the REAL FluxRouter.
//
// Net effect: chat -> local fixture (so the turn prompt is captured verbatim),
// transcription -> real provider (so the derived text is real). The credential
// is then the ONLY difference between leg A and leg B.
//
// ── THE THREE LEGS ────────────────────────────────────────────────────────
//   A  audio-1, FLUX_API_KEY present   POSITIVE
//   B  audio-1, FLUX_API_KEY ABSENT    NEGATIVE CONTROL (must redden)
//   C  audio-2, FLUX_API_KEY present   ANTI-ECHO
//
// Leg C is the control the predecessor's shape could not have. A backend that
// returned a fixed string would sail through a naive positive gate; C requires
// the derived text to TRACK THE AUDIO — different speech, different transcript.
//
// ── CREDENTIAL DISCIPLINE ─────────────────────────────────────────────────
// This script NEVER reads a secrets file, never writes a key to disk, never
// puts one in argv, and never logs one. It reads `FLUX_API_KEY` from its own
// environment (the caller sources the secrets file) and passes it to the child
// gateway through `env`. `--redact` is applied to every captured artefact.
//
// usage:
//   node scripts/f24-media-live.mjs --selftest
//   node scripts/f24-media-live.mjs --binary <wayland-core> --audio1 a1.wav \
//        --audio2 a2.wav --out <dir> [--budget-ms 120000]

import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));

// ── ground truth ───────────────────────────────────────────────────────────
// The exact sentences synthesised into the two WAVs. Content words only: a
// transcript is scored on how many of these it recovers, NOT on string
// equality, because a real STT engine legitimately differs on casing,
// punctuation and the odd rare noun.
const GT = {
  a1: {
    sentence: 'The quantum ferret audited nineteen crimson bicycles on Thursday morning.',
    words: ['quantum', 'ferret', 'audited', 'nineteen', 'crimson', 'bicycles', 'thursday', 'morning'],
    minHits: 5,
  },
  a2: {
    sentence: 'Seventeen velvet lighthouses inspected the marmalade orchestra last winter.',
    words: ['seventeen', 'velvet', 'lighthouses', 'inspected', 'marmalade', 'orchestra', 'winter'],
    minHits: 5,
  },
};

// The runtime degraded notice for audio, transcribed from
// `channel_media.rs AUDIO_NO_TRANSCRIPTION_NOTICE`. NOTE the hazard the
// predecessor measured: the Rust source wraps this string with a `\`
// continuation, so copying it out of the .rs file verbatim embeds a newline and
// an indent INSIDE the phrase and can never match the runtime value. The needle
// below is the RUNTIME form (single spaces), and `--selftest` proves the broken
// form misses.
const AUDIO_NOTICE_NEEDLE = 'no transcription backend is configured';
const AUDIO_NOTICE_FULL =
  '[Inbound audio received but NOT transcribed: no transcription backend is configured, so ' +
  'the assistant cannot hear this audio. To enable transcription, set GROQ_API_KEY or ' +
  'OPENAI_API_KEY.]';
// The realistic mistake: the source's line-continuation newline+indent kept.
const AUDIO_NOTICE_BROKEN = 'no transcription backend is configured, so \n     the assistant';
const AUDIO_FAILED_NEEDLE = 'could not be transcribed';

function hits(text, words) {
  const lower = (text || '').toLowerCase();
  return words.filter((w) => lower.includes(w));
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ── self-test: three assertions, not two ───────────────────────────────────
// (a) known-positive matches, (b) known-negative does not, and (c) THE OLD
// BROKEN MATCHER WOULD HAVE MISSED THE REAL NOTICE. Only (c) proves the repair
// does anything — without it the self-test passes on the broken instrument too.
function selftest() {
  const results = [];
  const push = (name, ok, detail) => results.push({ name, ok, detail });

  push(
    'known-positive: runtime notice matches the needle',
    AUDIO_NOTICE_FULL.includes(AUDIO_NOTICE_NEEDLE),
    { needle: AUDIO_NOTICE_NEEDLE },
  );
  push(
    'known-negative: a real transcript does NOT match the notice needle',
    !GT.a1.sentence.includes(AUDIO_NOTICE_NEEDLE),
    {},
  );
  push(
    'THIRD: the broken (source-verbatim) matcher MISSES the real runtime notice',
    !AUDIO_NOTICE_FULL.includes(AUDIO_NOTICE_BROKEN),
    { broken: JSON.stringify(AUDIO_NOTICE_BROKEN) },
  );
  // Scorer must separate the two ground truths, or the anti-echo gate is blind.
  push(
    'scorer: a1 sentence scores >= minHits on a1 words',
    hits(GT.a1.sentence, GT.a1.words).length >= GT.a1.minHits,
    { hits: hits(GT.a1.sentence, GT.a1.words) },
  );
  push(
    'scorer: a1 sentence scores ~0 on a2 words (anti-echo separability)',
    hits(GT.a1.sentence, GT.a2.words).length === 0,
    { crosshits: hits(GT.a1.sentence, GT.a2.words) },
  );
  push(
    'scorer: a2 sentence scores ~0 on a1 words',
    hits(GT.a2.sentence, GT.a1.words).length === 0,
    { crosshits: hits(GT.a2.sentence, GT.a1.words) },
  );

  const allOk = results.every((r) => r.ok);
  for (const r of results) {
    process.stderr.write(`${r.ok ? 'PASS' : 'FAIL'}  ${r.name}  ${JSON.stringify(r.detail)}\n`);
  }
  process.stderr.write(`\nselftest all_pass=${allOk}\n`);
  process.exit(allOk ? 0 : 1);
}

// ── telegram fixture ───────────────────────────────────────────────────────
// Standalone on purpose: `scripts/f24-tg-fixture.mjs` is a getUpdates-only
// concurrency instrument shared by other lanes, and it serves neither getFile
// nor the file-download route. Extending a shared file mid-flight is exactly
// the cross-lane collision a previous lane had to repair, so this driver
// carries its own minimal surface and touches nothing shared.
class TgFixture {
  constructor({ token, audioPath, audioMime }) {
    this.token = token;
    this.audioPath = audioPath;
    this.audioMime = audioMime;
    this.pending = [];
    this.nextUpdateId = 1000;
    this.replies = [];
    this.events = [];
    this.getFileCalls = 0;
    this.downloadCalls = 0;
    this.downloadBytes = 0;
    this.server = null;
    this.base = null;
  }

  record(kind, detail) {
    this.events.push({ at: new Date().toISOString(), kind, ...detail });
  }

  submitVoice({ chatId, userId, text, fileId }) {
    const id = (this.nextUpdateId += 1);
    this.pending.push({
      update_id: id,
      message: {
        message_id: id,
        date: Math.floor(Date.now() / 1000),
        chat: { id: Number(chatId), type: 'private' },
        from: { id: Number(userId), is_bot: false, first_name: 'F24ML', username: 'f24ml' },
        text,
        voice: { file_id: fileId, duration: 5, mime_type: this.audioMime, file_size: 0 },
      },
    });
    this.record('submit', { update_id: id, file_id: fileId });
    return id;
  }

  report() {
    return {
      get_file_calls: this.getFileCalls,
      download_calls: this.downloadCalls,
      download_bytes: this.downloadBytes,
      replies: this.replies,
      poll_count: this.events.filter((e) => e.kind === 'getUpdates').length,
      events: this.events,
    };
  }

  async start() {
    this.server = http.createServer((req, res) => {
      const chunks = [];
      req.on('data', (c) => chunks.push(c));
      req.on('end', () => this.handle(req, res, Buffer.concat(chunks).toString('utf8')));
    });
    await new Promise((resolve) => this.server.listen(0, '127.0.0.1', resolve));
    this.base = `http://127.0.0.1:${this.server.address().port}`;
    return this.base;
  }

  stop() {
    if (this.server) this.server.close();
  }

  handle(req, res, body) {
    const url = new URL(req.url, 'http://127.0.0.1');
    const p = url.pathname;
    const json = (obj, status = 200) => {
      res.writeHead(status, { 'content-type': 'application/json' });
      res.end(JSON.stringify(obj));
    };

    // media bytes: {base}/file/bot{token}/{file_path}
    const dl = new RegExp(`^/file/bot${this.token}/(.+)$`).exec(p);
    if (dl) {
      let bytes;
      try {
        bytes = fs.readFileSync(this.audioPath);
      } catch (e) {
        this.record('download_error', { error: String(e) });
        res.writeHead(500);
        res.end('read failed');
        return;
      }
      this.downloadCalls += 1;
      this.downloadBytes = bytes.length;
      this.record('download', { file_path: dl[1], bytes: bytes.length });
      res.writeHead(200, { 'content-type': this.audioMime, 'content-length': bytes.length });
      res.end(bytes);
      return;
    }

    const m = /^\/bot([^/]+)\/(\w+)$/.exec(p);
    if (!m) {
      this.record('unknown_endpoint', { path: p });
      json({ ok: false, error_code: 404, description: `unknown ${p}` }, 404);
      return;
    }
    const [, token, method] = m;
    if (token !== this.token) {
      this.record('bad_token', { method });
      json({ ok: false, error_code: 401, description: 'Unauthorized' }, 401);
      return;
    }

    if (method === 'getMe') {
      json({ ok: true, result: { id: 1, is_bot: true, first_name: 'f24ml', username: 'f24mlbot' } });
      return;
    }
    if (method === 'deleteWebhook' || method === 'setMyCommands') {
      json({ ok: true, result: true });
      return;
    }
    if (method === 'sendChatAction' || method === 'setMessageReaction') {
      this.record(method, {});
      json({ ok: true, result: true });
      return;
    }
    if (method === 'getUpdates') {
      let offset = 0;
      try {
        offset = Number(JSON.parse(body || '{}').offset || 0);
      } catch {
        offset = Number(url.searchParams.get('offset') || 0);
      }
      if (Number.isFinite(offset) && offset > 0) {
        this.pending = this.pending.filter((u) => u.update_id >= offset);
      }
      const result = this.pending.slice();
      this.record('getUpdates', { offset, served: result.map((u) => u.update_id) });
      json({ ok: true, result });
      return;
    }
    if (method === 'getFile') {
      let fileId = '';
      try {
        fileId = JSON.parse(body || '{}').file_id || '';
      } catch {
        fileId = url.searchParams.get('file_id') || '';
      }
      this.getFileCalls += 1;
      const filePath = `voice/${fileId}.wav`;
      this.record('getFile', { file_id: fileId, file_path: filePath });
      json({ ok: true, result: { file_id: fileId, file_unique_id: fileId, file_path: filePath } });
      return;
    }
    if (method === 'sendMessage') {
      let parsed = {};
      try {
        parsed = JSON.parse(body || '{}');
      } catch {
        /* record it as an empty reply rather than dropping the observation */
      }
      this.replies.push({ chat_id: String(parsed.chat_id ?? ''), text: String(parsed.text ?? '') });
      this.record('sendMessage', { chat_id: String(parsed.chat_id ?? '') });
      json({ ok: true, result: { message_id: 9000 + this.replies.length } });
      return;
    }
    this.record('unhandled_method', { method });
    json({ ok: true, result: true });
  }
}

// ── one leg ────────────────────────────────────────────────────────────────
async function runLeg({ label, binary, audioPath, withKey, fluxKey, outDir, budgetMs }) {
  const runDir = path.join(outDir, label);
  fs.mkdirSync(runDir, { recursive: true });
  const home = fs.mkdtempSync(path.join(os.tmpdir(), `f24ml-${label}-`));
  fs.mkdirSync(path.join(home, 'channels'), { recursive: true });

  const notes = [];
  const note = (m) => {
    notes.push(m);
    process.stderr.write(`[${label}] ${m}\n`);
  };

  // --- LLM fixture: captures the turn prompt ---
  const llmJournal = path.join(runDir, 'llm-journal.jsonl');
  fs.writeFileSync(llmJournal, '');
  const llmLog = path.join(runDir, 'llm.log');
  fs.writeFileSync(llmLog, '');
  const llmOut = fs.openSync(llmLog, 'a');
  const llm = spawn(
    process.execPath,
    [path.join(HERE, 'f24-llm-fixture.mjs'), '--port', '0', '--journal', llmJournal],
    { stdio: ['ignore', llmOut, llmOut] },
  );
  let llmUrl = null;
  for (let i = 0; i < 100 && !llmUrl; i += 1) {
    const mm = /http:\/\/127\.0\.0\.1:\d+/.exec(fs.readFileSync(llmLog, 'utf8'));
    if (mm) llmUrl = mm[0];
    else await sleep(100);
  }
  if (!llmUrl) throw new Error('llm fixture never announced a URL');
  note(`llm fixture at ${llmUrl}`);

  // --- telegram fixture ---
  const botToken = 'f24ml-bot-token';
  const chatId = '24090001';
  const fx = new TgFixture({ token: botToken, audioPath, audioMime: 'audio/wav' });
  await fx.start();
  note(`telegram fixture at ${fx.base} serving ${path.basename(audioPath)}`);

  // --- config ---
  fs.writeFileSync(
    path.join(home, 'credentials.toml'),
    ['[secrets]', `"telegram.f24ml.bot_token" = "${botToken}"`, ''].join('\n'),
    { mode: 0o600 },
  );
  // provider = "together": OpenAI-wire for chat, but NOT OpenAI/FluxRouter, so
  // `openai_wire_media_base` returns None and transcription arm 3 is skipped.
  fs.writeFileSync(
    path.join(home, 'config.toml'),
    [
      '[default]',
      'provider = "f24mlfixture"',
      '',
      '[providers.f24mlfixture]',
      'provider = "together"',
      'model = "f24ml-fixture"',
      'api_key = "f24ml-not-a-real-key"',
      `base_url = "${llmUrl}"`,
      '',
      '[inbound_webhook]',
      'enabled = false',
      '',
    ].join('\n'),
    { mode: 0o600 },
  );
  fs.writeFileSync(
    path.join(home, 'channels', 'f24ml.toml'),
    [
      'name = "f24ml"',
      'platform = "telegram"',
      'enabled = true',
      '',
      '[options]',
      'credential_handle = "telegram.f24ml.bot_token"',
      `api_base_url = "${fx.base}"`,
      'long_poll_timeout_secs = 1',
      'allowed_chat_ids = []',
      '',
      '[inbound]',
      'dm = "allowlist"',
      `dm_allowlist = ["${chatId}"]`,
      'group = "disabled"',
      'require_mention = false',
      'tools = "conversational"',
      '',
    ].join('\n'),
  );

  // --- gateway ---
  const gwLog = path.join(runDir, 'gateway.log');
  fs.writeFileSync(gwLog, '');
  const gwOut = fs.openSync(gwLog, 'a');
  // Build the child env explicitly. GROQ_API_KEY / OPENAI_API_KEY are deleted
  // unconditionally so arms 1 and 2 can never fire in ANY leg — that is what
  // makes leg B a total negative rather than a partial one.
  const childEnv = { ...process.env };
  delete childEnv.GROQ_API_KEY;
  delete childEnv.OPENAI_API_KEY;
  delete childEnv.ANTHROPIC_API_KEY;
  delete childEnv.GEMINI_API_KEY;
  delete childEnv.FLUX_API_KEY;
  if (withKey) {
    if (!fluxKey) throw new Error(`${label}: withKey leg but no FLUX_API_KEY in env`);
    childEnv.FLUX_API_KEY = fluxKey; // env only — never argv, never a file
  }
  childEnv.WAYLAND_HOME = home;
  childEnv.WAYLAND_VAULT_PASSPHRASE = 'f24ml-passphrase';
  childEnv.RUST_LOG =
    'info,wcore_agent::channel_media=debug,wcore_channel_telegram=debug,wcore_agent::tool_backends=debug';

  const child = spawn(binary, ['gateway', 'run'], { stdio: ['ignore', gwOut, gwOut], env: childEnv });

  // wait for the first poll — proves the adapter actually started
  let polled = false;
  for (let i = 0; i < budgetMs / 250 && !polled; i += 1) {
    if (fx.report().poll_count > 0) {
      polled = true;
      note(`telegram adapter polled after ~${i * 250}ms`);
    } else await sleep(250);
  }

  const probe = `f24ml probe ${label}`;
  let submittedId = null;
  if (polled) {
    submittedId = fx.submitVoice({
      chatId,
      userId: chatId,
      text: probe,
      fileId: `f24ml-voice-${label}`,
    });
    note(`submitted voice update ${submittedId}`);
  } else {
    note('adapter never polled — leg NOT MEASURED');
  }

  // wait for the turn to reach the model
  let turnRan = false;
  for (let i = 0; i < budgetMs / 250 && !turnRan; i += 1) {
    const raw = fs.readFileSync(llmJournal, 'utf8').trim();
    if (raw.length > 0) {
      turnRan = true;
      note(`turn reached the model after ~${i * 250}ms`);
    } else await sleep(250);
  }
  // let the reply drain
  await sleep(1500);

  child.kill('SIGTERM');
  await sleep(800);
  try {
    child.kill('SIGKILL');
  } catch {
    /* already gone */
  }
  llm.kill('SIGTERM');
  fx.stop();

  // --- extract the turn prompt ---
  const journalRaw = fs.readFileSync(llmJournal, 'utf8');
  let prompt = '';
  for (const line of journalRaw.split('\n')) {
    if (!line.trim()) continue;
    try {
      const rec = JSON.parse(line);
      const t = rec.user_text || rec.prompt || rec.last_user_text || '';
      if (t && t.includes(probe)) prompt = t;
      else if (t && !prompt) prompt = t;
    } catch {
      /* a partial line is not a turn */
    }
  }

  const fxReport = fx.report();
  const gwText = fs.existsSync(gwLog) ? fs.readFileSync(gwLog, 'utf8') : '';

  const out = {
    label,
    with_key: withKey,
    audio: path.basename(audioPath),
    polled,
    submitted_update_id: submittedId,
    turn_ran: turnRan,
    // capture_alive: the prompt must exist AND carry this leg's probe text
    // before ANY negative reading of it is allowed to count.
    capture_alive: prompt.includes(probe),
    prompt,
    prompt_bytes: Buffer.byteLength(prompt, 'utf8'),
    notice_present: prompt.includes(AUDIO_NOTICE_NEEDLE),
    failed_notice_present: prompt.includes(AUDIO_FAILED_NEEDLE),
    a1_hits: hits(prompt, GT.a1.words),
    a2_hits: hits(prompt, GT.a2.words),
    get_file_calls: fxReport.get_file_calls,
    download_calls: fxReport.download_calls,
    download_bytes: fxReport.download_bytes,
    poll_count: fxReport.poll_count,
    replies: fxReport.replies.length,
    enriched_log_line: /inbound media enriched/.test(gwText),
    transcription_resolver_line:
      (/transcription: [^\n]*/.exec(gwText) || [''])[0].slice(0, 200),
    notes,
  };
  fs.writeFileSync(path.join(runDir, 'result.json'), `${JSON.stringify(out, null, 2)}\n`);
  fs.writeFileSync(path.join(runDir, 'turn-prompt.txt'), prompt);
  fs.writeFileSync(path.join(runDir, 'fixture-events.json'), `${JSON.stringify(fxReport, null, 2)}\n`);
  return out;
}

// ── main ───────────────────────────────────────────────────────────────────
function parseArgs(argv) {
  const out = { budgetMs: 120000 };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--selftest') out.selftest = true;
    else if (a === '--binary') out.binary = argv[++i];
    else if (a === '--audio1') out.audio1 = argv[++i];
    else if (a === '--audio2') out.audio2 = argv[++i];
    else if (a === '--out') out.out = argv[++i];
    else if (a === '--budget-ms') out.budgetMs = Number(argv[++i]);
    else {
      process.stderr.write(`f24-media-live: unknown argument ${a}\n`);
      process.exit(2);
    }
  }
  return out;
}

const args = parseArgs(process.argv.slice(2));
if (args.selftest) selftest();

if (!args.binary || !args.audio1 || !args.audio2 || !args.out) {
  process.stderr.write('f24-media-live: --binary, --audio1, --audio2 and --out are required\n');
  process.exit(2);
}

const fluxKey = process.env.FLUX_API_KEY || '';
if (!fluxKey) {
  process.stderr.write(
    'f24-media-live: FLUX_API_KEY is not in the environment. Legs A and C cannot run.\n',
  );
  process.exit(2);
}

fs.mkdirSync(args.out, { recursive: true });

const legs = [];
legs.push(
  await runLeg({
    label: 'A-key-audio1',
    binary: args.binary,
    audioPath: args.audio1,
    withKey: true,
    fluxKey,
    outDir: args.out,
    budgetMs: args.budgetMs,
  }),
);
legs.push(
  await runLeg({
    label: 'B-nokey-audio1',
    binary: args.binary,
    audioPath: args.audio1,
    withKey: false,
    fluxKey,
    outDir: args.out,
    budgetMs: args.budgetMs,
  }),
);
legs.push(
  await runLeg({
    label: 'C-key-audio2',
    binary: args.binary,
    audioPath: args.audio2,
    withKey: true,
    fluxKey,
    outDir: args.out,
    budgetMs: args.budgetMs,
  }),
);

const A = legs[0];
const B = legs[1];
const C = legs[2];

const gates = [];
const gate = (id, clause, kind, pass, measured) =>
  gates.push({ id, clause, kind, pass, measured });

// G1 POSITIVE — a real transcript of audio-1 reached the model.
gate(
  'G1',
  'media (positive)',
  'POSITIVE',
  A.turn_ran &&
    A.capture_alive &&
    A.download_calls > 0 &&
    A.a1_hits.length >= GT.a1.minHits &&
    !A.notice_present,
  {
    turn_ran: A.turn_ran,
    capture_alive: A.capture_alive,
    download_calls: A.download_calls,
    download_bytes: A.download_bytes,
    a1_hits: A.a1_hits,
    a1_hit_count: A.a1_hits.length,
    required: GT.a1.minHits,
    notice_present: A.notice_present,
  },
);

// G2 NEGATIVE CONTROL — same audio, same everything, credential removed.
// Requires turn_ran AND capture_alive so it cannot pass by the turn simply
// never happening: a leg that never dispatched would fail here, not pass.
gate(
  'G2',
  'media (negative control)',
  'NEGATIVE CONTROL',
  B.turn_ran &&
    B.capture_alive &&
    B.notice_present &&
    B.a1_hits.length === 0,
  {
    turn_ran: B.turn_ran,
    capture_alive: B.capture_alive,
    notice_present: B.notice_present,
    a1_hits: B.a1_hits,
  },
);

// G3 ANTI-ECHO — different audio must produce a different transcript.
gate(
  'G3',
  'media (anti-echo)',
  'POSITIVE',
  C.turn_ran &&
    C.capture_alive &&
    C.a2_hits.length >= GT.a2.minHits &&
    C.a1_hits.length === 0 &&
    A.a2_hits.length === 0 &&
    C.prompt !== A.prompt,
  {
    c_a2_hits: C.a2_hits,
    c_a2_hit_count: C.a2_hits.length,
    required: GT.a2.minHits,
    c_a1_crosshits: C.a1_hits,
    a_a2_crosshits: A.a2_hits,
    prompts_differ: C.prompt !== A.prompt,
  },
);

// G4 FETCH PATH — the connector actually downloaded bytes in the positive legs
// and the enricher logged an enrichment. Distinguishes "transcript appeared"
// from "transcript appeared because the model invented it".
gate(
  'G4',
  'media (connector fetch)',
  'POSITIVE',
  A.get_file_calls > 0 &&
    A.download_calls > 0 &&
    A.download_bytes > 1000 &&
    C.download_calls > 0 &&
    A.enriched_log_line,
  {
    a_get_file_calls: A.get_file_calls,
    a_download_calls: A.download_calls,
    a_download_bytes: A.download_bytes,
    c_download_calls: C.download_calls,
    a_enriched_log_line: A.enriched_log_line,
  },
);

const summary = {
  generated_at: new Date().toISOString(),
  out_dir: args.out,
  binary: args.binary,
  legs: legs.map((l) => ({
    label: l.label,
    with_key: l.with_key,
    audio: l.audio,
    turn_ran: l.turn_ran,
    capture_alive: l.capture_alive,
    prompt_bytes: l.prompt_bytes,
    notice_present: l.notice_present,
    failed_notice_present: l.failed_notice_present,
    a1_hits: l.a1_hits,
    a2_hits: l.a2_hits,
    get_file_calls: l.get_file_calls,
    download_calls: l.download_calls,
    download_bytes: l.download_bytes,
    poll_count: l.poll_count,
    replies: l.replies,
    enriched_log_line: l.enriched_log_line,
    transcription_resolver_line: l.transcription_resolver_line,
  })),
  gates,
  all_pass: gates.every((g) => g.pass),
};

fs.writeFileSync(path.join(args.out, 'summary.json'), `${JSON.stringify(summary, null, 2)}\n`);
for (const g of gates) {
  process.stderr.write(`${g.pass ? 'PASS' : 'FAIL'}  ${g.id} [${g.kind}] ${g.clause}\n`);
}
process.stderr.write(`\nall_pass=${summary.all_pass}\nsummary: ${path.join(args.out, 'summary.json')}\n`);
process.exit(summary.all_pass ? 0 : 1);
