#!/usr/bin/env node
// f24-media-actions.mjs — drive the two `24-C3` clauses that have NEVER been
// measured on any adapter: `media` and `native actions`.
//
// STRICTLY ADDITIVE. This file edits nothing. It IMPORTS `DiscordFixture` from
// `f24-discord-fixture.mjs` and subclasses it; `scripts/f24-inbound.mjs` is
// shared with live lanes and is not touched.
//
// ---------------------------------------------------------------------------
// What each clause means (established from source, not from a summary)
// ---------------------------------------------------------------------------
// `native actions` is not a term in the Rust source (0 hits, instrument proven
// alive against a 23-file known-positive). The CONCEPT is the ack state machine
// in `wcore-agent/src/channel_inbound.rs:503-556`, gated by `AckMode`:
//     👀 reaction on receipt → typing keepalive while the turn runs → ✅/❌
// `AckMode` defaults to `Off` (`dispatch/access.rs:191`), which is why six
// consecutive inbound lanes never exercised it: the surface is off unless asked.
//
// BOTH `react_on` failures are SWALLOWED by `run_turn` (`tracing::debug!` and
// `let _ =`). So Core's own logs CANNOT prove a native action happened. It must
// be counted on the PLATFORM side. Fixture-side counting is the only valid
// instrument here, not a convenience.
//
// `media` is the inbound enrichment path `channel_media.rs:157` (`enrich`),
// reached in production from `channel_dispatch.rs:138` via the gateway inbound
// host (`channel_inbound_host.rs:220-240`). With no vision backend configured
// it writes an honest degraded notice into `Attachment::transcribed`
// (`channel_media.rs:165-168`) BEFORE attempting any fetch, and
// `build_turn_prompt` (`channel_dispatch.rs:278-297`) folds that text into the
// user prompt. That is what this driver observes, at the LLM fixture.
//
// ---------------------------------------------------------------------------
// Why the live vision leg is NOT attempted
// ---------------------------------------------------------------------------
// `build_vision_backend()` (`tool_backends/mod.rs:321-338`) consults ONLY
// ANTHROPIC_API_KEY / OPENAI_API_KEY / GEMINI_API_KEY. It never consults
// FLUX_API_KEY. The only credential available is FLUX_API_KEY, so
// image→description is UNREACHABLE, and this driver measures the degraded path
// that IS reachable rather than manufacturing a green.
//
// ---------------------------------------------------------------------------
// Gates, and the negative control for each
// ---------------------------------------------------------------------------
//   G1 native actions POSITIVE  run A (ack="both")  👀 and ✅ seen, typing ≥ 1
//   G2 native actions CONTROL   run B (ack="off")   reactions == 0, typing == 0
//   G3 media POSITIVE           run A              prompt carries the notice
//   G4 media CONTROL            run C (no attach)  notice absent AND the prompt
//                                                  capture proven alive on the
//                                                  same run (§3b-i)
//
// G2 is what stops the LANE-BRIEF §3.2 failure mode where a clause "passes"
// because everything was denied: if the binary never admitted the message at
// all, run A's counts are zero and G1 fails. G1 and G2 can only both pass if
// the difference is genuinely the `ack` setting.

import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import DiscordFixture from './f24-discord-fixture.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));

// ---------------------------------------------------------------------------
// The notice matcher, and its three-assertion self-test (LANE-BRIEF §6b-ii)
// ---------------------------------------------------------------------------

// A short invariant that lives on ONE source line of the Rust constant, so a
// line-continuation in the source cannot desynchronise it from the runtime
// string. See IMAGE_NO_VISION_NOTICE, `channel_media.rs:68-70`.
const NOTICE_NEEDLE = 'no vision backend is configured';

// The runtime value of IMAGE_NO_VISION_NOTICE. In Rust, a trailing `\` strips
// the newline AND the next line's leading whitespace, so the real string has
// single spaces where the source is wrapped and indented.
const NOTICE_RUNTIME =
  '[Inbound image received but NOT analyzed: no vision backend is configured, so the ' +
  'assistant cannot see this image. Do not guess its contents. To enable image ' +
  'understanding, set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY.]';

// The BROKEN matcher this lane must prove it is not using: transcribing the
// constant straight out of the .rs file keeps the source's newline + indent
// inside the phrase, so it can never match the runtime string.
const NOTICE_NEEDLE_BROKEN = 'no vision backend is configured, so the \n     assistant';

function noticeMatches(text) {
  return typeof text === 'string' && text.includes(NOTICE_NEEDLE);
}
function noticeMatchesBroken(text) {
  return typeof text === 'string' && text.includes(NOTICE_NEEDLE_BROKEN);
}

/**
 * Three assertions, not two. The third is the only one that proves the repair
 * does anything — without it the self-test passes on the broken matcher too.
 */
function selfTestMatcher() {
  const results = [];
  const positive = `some text\n\n[attachments received with this message:\n  1. Image (image/png) — description: ${NOTICE_RUNTIME}`;
  const negative = 'a plain text message with no attachments at all';

  results.push({
    name: 'known-positive: the real runtime notice MATCHES',
    pass: noticeMatches(positive) === true,
  });
  results.push({
    name: 'known-negative: a plain prompt DOES NOT match',
    pass: noticeMatches(negative) === false,
  });
  results.push({
    name: 'the OLD BROKEN matcher (source-transcribed, newline inside the phrase) MISSES the real notice',
    pass: noticeMatchesBroken(positive) === false && noticeMatches(positive) === true,
  });
  return results;
}

// ---------------------------------------------------------------------------
// Fixture subclass: dispatch a MESSAGE_CREATE that actually carries attachments
// ---------------------------------------------------------------------------
// `DiscordFixture.dispatchMessage` hardcodes `attachments: []`. Subclassing is
// how this lane adds media WITHOUT editing a file other lanes are using.
class MediaDiscordFixture extends DiscordFixture {
  dispatchMessageWithAttachments({ id, channelId, content, authorId, attachments }) {
    const targets = [...this.conns].filter((c) => c.identified);
    const payload = {
      id,
      channel_id: channelId,
      content,
      timestamp: new Date().toISOString(),
      author: { id: authorId, username: `u${authorId}`, bot: false },
      mentions: [],
      // Discord's MessageAttachment deserializes `url` + `content_type` only
      // (gateway.rs:129-135) — it does not parse `size`, which is why its
      // declared 25 MiB media_bounds is unenforceable by construction.
      attachments: attachments ?? [],
    };
    let s = 0;
    for (const c of targets) {
      c.seq += 1;
      s = Math.max(s, c.seq);
      this.send(c, { op: 0, t: 'MESSAGE_CREATE', s: c.seq, d: payload });
      c.delivered += 1;
    }
    this.dispatched.push({
      id,
      s: s || this.dispatched.length + 1,
      payload,
      sockets: targets.length,
      at: Date.now(),
    });
    return targets.length;
  }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------
// MUST be async. The DiscordFixture runs IN THIS PROCESS, so a synchronous
// sleep (e.g. `spawnSync`) blocks Node's event loop and the fixture can never
// accept the binary's connection — the dial happens and gets ECONNREFUSED
// while the driver sits in a "waiting for IDENTIFY" loop that cannot serve it.
// That produced a full three-leg NOT MEASURED run before it was repaired: an
// instrument defect masquerading as a product failure.
function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

function readJournal(file) {
  if (!fs.existsSync(file)) return [];
  return fs
    .readFileSync(file, 'utf8')
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

// ---------------------------------------------------------------------------
// one run
// ---------------------------------------------------------------------------
async function runLeg({ label, ack, withAttachment, binary, rootDir, budgetMs }) {
  const runDir = path.join(rootDir, label);
  const home = path.join(runDir, 'home');
  fs.mkdirSync(path.join(home, 'channels'), { recursive: true });

  const notes = [];
  const note = (m) => {
    notes.push(m);
    process.stderr.write(`[${label}] ${m}\n`);
  };

  // --- LLM fixture (captures the turn prompt) ---
  const llmJournal = path.join(runDir, 'llm-journal.jsonl');
  fs.writeFileSync(llmJournal, '');
  const llmLog = path.join(runDir, 'llm.log');
  fs.writeFileSync(llmLog, '');
  const llmOut = fs.openSync(llmLog, 'a');
  const llm = spawn(
    process.execPath,
    [path.join(HERE, 'f24-llm-fixture.mjs'), '--port', '0', '--journal', llmJournal],
    { stdio: ['ignore', llmOut, llmOut], detached: false },
  );
  let llmUrl = null;
  for (let i = 0; i < 100 && !llmUrl; i += 1) {
    const m = /http:\/\/127\.0\.0\.1:\d+/.exec(fs.readFileSync(llmLog, 'utf8'));
    if (m) llmUrl = m[0];
    else await sleep(100);
  }
  if (!llmUrl) throw new Error('llm fixture never announced a URL');
  note(`llm fixture at ${llmUrl}`);

  // --- discord fixture ---
  const botToken = 'f24ma-bot-token';
  const botId = '900000000000000001';
  const senderId = '900000000000000042';
  const channelId = 'C24MA1';
  const fx = new MediaDiscordFixture({ botToken, botId });
  await fx.start();
  note(`discord fixture at ${fx.apiBase} (gateway ${fx.gatewayUrl})`);

  // --- config ---
  fs.writeFileSync(
    path.join(home, 'credentials.toml'),
    ['[secrets]', `"discord.f24ma.bot_token" = "${botToken}"`, ''].join('\n'),
    { mode: 0o600 },
  );
  fs.writeFileSync(
    path.join(home, 'config.toml'),
    [
      '[default]',
      'provider = "f24mafixture"',
      '',
      '[providers.f24mafixture]',
      'provider = "openai"',
      'model = "f24ma-fixture"',
      'api_key = "f24ma-not-a-real-key"',
      `base_url = "${llmUrl}"`,
      '',
      '[inbound_webhook]',
      'enabled = false',
      '',
    ].join('\n'),
    { mode: 0o600 },
  );
  fs.writeFileSync(
    path.join(home, 'channels', 'f24ma.toml'),
    [
      'name = "f24ma"',
      'platform = "discord"',
      'enabled = true',
      '',
      '[options]',
      'credential_handle = "discord.f24ma.bot_token"',
      `api_base_url = "${fx.apiBase}"`,
      `gateway_url = "${fx.gatewayUrl}"`,
      'heartbeat_grace_ms = 30000',
      '',
      '[inbound]',
      'dm = "allowlist"',
      `dm_allowlist = ["${senderId}"]`,
      'group = "disabled"',
      'require_mention = false',
      'tools = "conversational"',
      // THE VARIABLE UNDER TEST. Everything else is identical across legs.
      `ack = "${ack}"`,
      '',
    ].join('\n'),
  );

  // --- gateway ---
  const gwLog = path.join(runDir, 'gateway.log');
  fs.writeFileSync(gwLog, '');
  const gwOut = fs.openSync(gwLog, 'a');
  const child = spawn(binary, ['gateway', 'run'], {
    stdio: ['ignore', gwOut, gwOut],
    env: {
      ...process.env,
      WAYLAND_HOME: home,
      WAYLAND_VAULT_PASSPHRASE: 'f24ma-passphrase',
      RUST_LOG: 'info,wcore_agent::channel_media=debug,wcore_channel_discord=debug',
    },
    detached: false,
  });

  // wait for IDENTIFY
  let identified = false;
  for (let i = 0; i < budgetMs / 250 && !identified; i += 1) {
    const rep = fx.report();
    if (rep.identify_count > 0 && rep.live_gateway_connections > 0) {
      identified = true;
      note(`gateway IDENTIFYed after ~${i * 250}ms`);
    } else await sleep(250);
  }

  let dispatched = 0;
  let replySeen = false;
  if (identified) {
    const attachments = withAttachment
      ? [
          {
            url: 'https://cdn.discordapp.com/attachments/1/2/f24ma-probe.png',
            content_type: 'image/png',
          },
        ]
      : [];
    dispatched = fx.dispatchMessageWithAttachments({
      id: `f24ma-${label}-msg1`,
      channelId,
      content: `f24ma probe ${label}`,
      authorId: senderId,
      attachments,
    });
    note(`dispatched MESSAGE_CREATE to ${dispatched} socket(s), attachments=${attachments.length}`);

    // wait for the turn to run (the LLM fixture journals it)
    for (let i = 0; i < budgetMs / 250; i += 1) {
      if (readJournal(llmJournal).some((r) => r.kind === 'chat.completions')) {
        replySeen = true;
        note(`turn observed at the LLM fixture after ~${i * 250}ms`);
        break;
      }
      await sleep(250);
    }
    // Let the ack state machine finish its completion reaction.
    await sleep(2500);
  } else {
    note('binary never IDENTIFYed — this leg is NOT MEASURED, not a pass');
  }

  const report = fx.report();
  // `report()` exposes `reactions_total` but NOT the reactions array, so the
  // emoji identities must be read off the fixture instance itself, BEFORE
  // `fx.stop()`. Reading `report.reactions` yields `undefined` and an empty
  // emoji list — which failed G1 while the counts were already correct. Kept
  // as an assertion on identity, not just count: a run that fired two 👀 and
  // no completion reaction has the same `reactions_total` as a correct one.
  const emojis = (fx.reactions ?? []).map((r) => r.emoji);
  const journal = readJournal(llmJournal);

  try {
    child.kill('SIGTERM');
  } catch {
    /* already gone */
  }
  await sleep(800);
  try {
    child.kill('SIGKILL');
  } catch {
    /* already gone */
  }
  await fx.stop();
  try {
    llm.kill('SIGTERM');
  } catch {
    /* already gone */
  }

  const turnPrompts = journal.filter((r) => r.kind === 'chat.completions').map((r) => r.user_text ?? '');

  const out = {
    label,
    ack,
    with_attachment: Boolean(withAttachment),
    identified,
    dispatched_sockets: dispatched,
    turn_ran: replySeen,
    reactions_total: report.reactions_total,
    reaction_emojis: emojis,
    typing_total: report.typing_total,
    turn_prompt_count: turnPrompts.length,
    prompt_carries_notice: turnPrompts.some(noticeMatches),
    prompt_carries_probe_text: turnPrompts.some((t) => t.includes(`f24ma probe ${label}`)),
    // byte counts via fs.statSync — NOT `wc`, which this lane measured
    // returning 0 for a 72-byte file under the shell proxy.
    gateway_log_bytes: fs.existsSync(gwLog) ? fs.statSync(gwLog).size : 0,
    llm_journal_bytes: fs.existsSync(llmJournal) ? fs.statSync(llmJournal).size : 0,
    notes,
  };
  fs.writeFileSync(path.join(runDir, 'result.json'), `${JSON.stringify(out, null, 2)}\n`);
  return out;
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------
async function main() {
  const argv = process.argv.slice(2);
  const selftestOnly = argv.includes('--selftest');
  const binIdx = argv.indexOf('--binary');
  const outIdx = argv.indexOf('--out');
  const binary = binIdx >= 0 ? argv[binIdx + 1] : null;
  const outDir = outIdx >= 0 ? argv[outIdx + 1] : fs.mkdtempSync(path.join(os.tmpdir(), 'f24ma-'));

  const selfTest = selfTestMatcher();
  for (const t of selfTest) {
    process.stderr.write(`[selftest] ${t.pass ? 'PASS' : 'FAIL'}  ${t.name}\n`);
  }
  const selfTestOk = selfTest.every((t) => t.pass);
  if (selftestOnly) {
    process.stdout.write(`${JSON.stringify({ selftest: selfTest, ok: selfTestOk }, null, 2)}\n`);
    process.exit(selfTestOk ? 0 : 1);
  }
  if (!selfTestOk) {
    process.stderr.write('matcher self-test FAILED — refusing to measure with a broken instrument\n');
    process.exit(1);
  }
  if (!binary) {
    process.stderr.write('usage: f24-media-actions.mjs --binary <wayland-core> [--out <dir>]\n');
    process.exit(2);
  }

  fs.mkdirSync(outDir, { recursive: true });
  const budgetMs = 45000;

  // A: the positive leg — native actions ON, image attachment present.
  const A = await runLeg({ label: 'A-ack-both-image', ack: 'both', withAttachment: true, binary, rootDir: outDir, budgetMs });
  // B: native-actions negative control — ONLY `ack` differs from A.
  const B = await runLeg({ label: 'B-ack-off-image', ack: 'off', withAttachment: true, binary, rootDir: outDir, budgetMs });
  // C: media negative control — ONLY the attachment differs from A.
  const C = await runLeg({ label: 'C-ack-both-noimage', ack: 'both', withAttachment: false, binary, rootDir: outDir, budgetMs });

  const sawEyes = A.reaction_emojis.includes('👀');
  const sawDone = A.reaction_emojis.includes('✅') || A.reaction_emojis.includes('❌');

  const gates = [
    {
      id: 'G1',
      clause: 'native actions',
      kind: 'POSITIVE',
      desc: 'ack="both": 👀 on receipt AND a completion reaction AND ≥1 typing indicator, counted at the platform',
      pass: A.identified && sawEyes && sawDone && A.reactions_total >= 2 && A.typing_total >= 1,
      detail: `identified=${A.identified} reactions=${A.reactions_total} emojis=${JSON.stringify(A.reaction_emojis)} typing=${A.typing_total}`,
    },
    {
      id: 'G2',
      clause: 'native actions',
      kind: 'NEGATIVE CONTROL',
      desc: 'ack="off", one variable changed: ZERO reactions and ZERO typing — while the turn still ran',
      pass: B.identified && B.turn_ran && B.reactions_total === 0 && B.typing_total === 0,
      detail: `identified=${B.identified} turn_ran=${B.turn_ran} reactions=${B.reactions_total} typing=${B.typing_total}`,
    },
    {
      id: 'G3',
      clause: 'media',
      kind: 'POSITIVE',
      desc: 'an inbound image reaches the turn prompt carrying the honest degraded notice',
      pass: A.turn_ran && A.prompt_carries_notice,
      detail: `turn_ran=${A.turn_ran} prompts=${A.turn_prompt_count} notice=${A.prompt_carries_notice}`,
    },
    {
      id: 'G4',
      clause: 'media',
      kind: 'NEGATIVE CONTROL',
      desc: 'no attachment: notice ABSENT — and the prompt capture proven alive on the same run (§3b-i)',
      pass: C.turn_ran && C.prompt_carries_probe_text && C.prompt_carries_notice === false,
      detail: `turn_ran=${C.turn_ran} capture_alive=${C.prompt_carries_probe_text} notice=${C.prompt_carries_notice}`,
    },
  ];

  const summary = {
    generated_at: new Date().toISOString(),
    binary,
    out_dir: outDir,
    matcher_selftest: selfTest,
    legs: { A, B, C },
    gates,
    all_pass: gates.every((g) => g.pass),
  };
  fs.writeFileSync(path.join(outDir, 'summary.json'), `${JSON.stringify(summary, null, 2)}\n`);

  process.stderr.write('\n================ GATES ================\n');
  for (const g of gates) {
    process.stderr.write(`${g.pass ? 'PASS' : 'FAIL'}  ${g.id} [${g.clause} / ${g.kind}]\n      ${g.desc}\n      ${g.detail}\n`);
  }
  process.stderr.write(`\nall_pass=${summary.all_pass}\nsummary: ${path.join(outDir, 'summary.json')}\n`);
  process.exit(summary.all_pass ? 0 : 1);
}

main().catch((e) => {
  process.stderr.write(`${e?.stack ?? e}\n`);
  process.exit(3);
});
