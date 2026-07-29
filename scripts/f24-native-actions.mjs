#!/usr/bin/env node
// f24-native-actions.mjs — move the `24-C3` clause `native actions` from ONE
// adapter (discord) to as many as the fixtures genuinely support.
//
// STRICTLY ADDITIVE. This file edits nothing. It carries its OWN fixtures for
// telegram / matrix / slack / msteams so that `f24-tg-fixture.mjs`,
// `f24-matrix-fixture.mjs`, `f24-msteams-fixture.mjs` and `f24-inbound.mjs` —
// all shared with concurrently-running lanes — are not touched. It subclasses
// `DiscordFixture` for the discord leg, the way `f24-media-actions.mjs` did.
//
// ---------------------------------------------------------------------------
// WHAT A NATIVE ACTION IS
// ---------------------------------------------------------------------------
// The term does not exist in the Rust source. The CONCEPT is the ack state
// machine in `wcore-agent/src/channel_inbound.rs` `run_turn` (:512-555), gated
// by `AckMode`. Three affordances:
//
//   A1  receipt reaction   `react_on(.., "👀")` BEFORE dispatch
//   A2  typing keepalive   `spawn_typing_keepalive` under an `AbortOnDrop`
//                          guard: fires once immediately, then every 5s, and
//                          is aborted when the turn completes
//   A3  terminal reaction  `react_on(.., "✅")` on Ok / `"❌"` on Err
//
// TWO FACTS DECIDE THE METHOD, and both are re-confirmed from source here:
//
//   1. `AckMode` defaults to `Off` (`dispatch/access.rs:191`). Nothing fires
//      unless a channel config asks. Every leg below sets `ack` explicitly.
//   2. BOTH `react_on` failures are SWALLOWED (`tracing::debug!` at :523 and
//      `let _ =` at :552), and `send_typing_to` is `let _ =` at :632. So Core's
//      own logs CANNOT prove a native action happened. Counting must be done on
//      the PLATFORM side. Fixture-side counting here is the instrument, not a
//      convenience — a log-side count would measure intent, not effect.
//
// ---------------------------------------------------------------------------
// THE DECLARED PER-ADAPTER SURFACE (trait-override census, from source)
// ---------------------------------------------------------------------------
// Trait defaults, `wcore-channels/src/lib.rs`:
//   `react`       :294  -> Err(Unsupported)     — a LOUD default
//   `send_typing` :277  -> Ok(())  NO-OP        — a SILENT default
//
// The asymmetry matters and is the sharpest trap on this clause: an adapter
// with no typing override returns `Ok(())`, so a log-side or error-side
// instrument would read "typing succeeded" on a platform that has no typing
// API at all. Only a platform-side count can tell those apart, which is why
// A2 is graded `not supported` for slack/whatsapp rather than `fired`.
//
//   adapter   react                    send_typing
//   discord   YES lib.rs:444           YES lib.rs:435
//   telegram  YES lib.rs:373           YES lib.rs:360
//   matrix    YES lib.rs:324           YES lib.rs:305
//   slack     YES lib.rs:267           NO  -> silent no-op default
//   msteams   NO  -> Unsupported       YES lib.rs:381
//   email/signal/imessage/twilio-sms   NO / NO
//
// msteams is the exact inverse of slack, which is why both are measured: a
// matrix that only contained adapters supporting BOTH could not distinguish
// "not supported" from "fired nothing".
//
// ---------------------------------------------------------------------------
// GATES — one POSITIVE and one one-variable NEGATIVE CONTROL per adapter
// ---------------------------------------------------------------------------
// For each adapter X:
//   GX-P  ack="both": the affordances X declares are COUNTED at the platform,
//         with emoji IDENTITY asserted (not just a count: a run that fired two
//         👀 and no terminal reaction has the same total as a correct one).
//   GX-N  ack="off", EXACTLY ONE variable changed: zero reactions AND zero
//         typing, WHILE THE TURN STILL RAN. The `turn_ran` conjunct is what
//         defeats the "green by universal denial" failure this criterion has
//         already suffered on its access leg — if the binary never admitted the
//         message, GX-P's counts are zero and GX-P fails.
//
// Plus one keepalive-lifecycle gate (see `--keepalive`), which is the affordance
// A2 defect class this program keeps finding: a background task that outlives
// the work it was spawned for.
//
// usage:
//   node scripts/f24-native-actions.mjs --selftest
//   node scripts/f24-native-actions.mjs --binary <wayland-core> --out <dir> \
//        [--adapters telegram,matrix,slack,msteams,discord] [--keepalive]

import crypto from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import DiscordFixture from './f24-discord-fixture.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));

// Ports. Five other lanes are live. 18787 (f24-inbound), 18211 (discord),
// 19631-19633 (msteams-attach) are all taken; these are deliberately far from
// every one of them and from each other. Overridable for a re-run.
const PORTS = {
  webhook: Number(process.env.F24_WEBHOOK_PORT ?? 21473),
};

// The ack emoji the state machine sends, in the order `run_turn` sends them.
const EYES = '👀';
const OK = '✅';
const BAD = '❌';

// MUST be async everywhere. Fixtures below run IN THIS PROCESS; a synchronous
// sleep (`Atomics.wait`, `spawnSync`) blocks Node's event loop and the fixture
// can never accept the binary's connection. Two prior lanes lost whole runs to
// exactly this, each time looking like a product failure rather than a driver
// defect (`f24-media-actions.mjs` defect 5; `f24-msteams-fixture.mjs` header).
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function readJsonl(file) {
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
// The instrument: an ack ledger, and its THREE-assertion self-test
// ---------------------------------------------------------------------------
// LANE-BRIEF §6b-ii: known-positive passes, known-negative fails, AND the
// broken predecessor would have missed the positive. The third assertion is the
// only one that proves the repair does anything.
//
// The realistic mistake being repaired: grading A1/A3 on `reactions.length`.
// That is the matcher `f24-media-actions.mjs` shipped its first run with, and
// it produced a FALSE FAIL there; the same shape produces a FALSE PASS the
// moment an adapter emits the receipt reaction twice and never emits a terminal
// one — which is exactly what a keepalive-style retry bug looks like.

/** Grade one adapter's ack ledger into the three affordances. */
export function gradeAffordances({ reactions, typing, declares }) {
  const emojis = reactions.map((r) => r.emoji);
  return {
    // A1 requires the RECEIPT emoji specifically, not "≥1 reaction".
    a1_receipt: declares.react ? (emojis.includes(EYES) ? 'fired' : 'not-fired') : 'not-supported',
    // A2 is a COUNT at the platform. An adapter with no `send_typing` override
    // returns Ok(()) from the trait default and emits nothing on the wire, so a
    // zero here for a non-declaring adapter is `not-supported`, NOT `not-fired`.
    a2_typing: declares.typing ? (typing.length >= 1 ? 'fired' : 'not-fired') : 'not-supported',
    // A3 requires a TERMINAL emoji specifically. `emojis.length >= 2` is the
    // broken matcher: two receipts satisfy it.
    a3_terminal: declares.react
      ? emojis.includes(OK) || emojis.includes(BAD)
        ? 'fired'
        : 'not-fired'
      : 'not-supported',
    emojis,
    typing_count: typing.length,
  };
}

/** The matcher this replaces — count-only. Kept solely to prove the repair. */
function gradeAffordancesBroken({ reactions, typing, declares }) {
  return {
    a1_receipt: declares.react ? (reactions.length >= 1 ? 'fired' : 'not-fired') : 'not-supported',
    a2_typing: declares.typing ? (typing.length >= 1 ? 'fired' : 'not-fired') : 'not-supported',
    a3_terminal: declares.react ? (reactions.length >= 2 ? 'fired' : 'not-fired') : 'not-supported',
  };
}

function selfTest() {
  const out = [];
  const bothDeclared = { react: true, typing: true };

  const good = {
    reactions: [{ emoji: EYES }, { emoji: OK }],
    typing: [{ at: 1 }],
    declares: bothDeclared,
  };
  // The defect the repair exists for: a receipt reaction emitted TWICE and no
  // terminal reaction. Count-only grading calls this a complete ack cycle.
  const doubleReceipt = {
    reactions: [{ emoji: EYES }, { emoji: EYES }],
    typing: [{ at: 1 }],
    declares: bothDeclared,
  };
  const silent = { reactions: [], typing: [], declares: bothDeclared };
  const slackShaped = {
    reactions: [{ emoji: EYES }, { emoji: OK }],
    typing: [],
    declares: { react: true, typing: false },
  };

  const g = gradeAffordances(good);
  out.push({
    name: 'known-positive: 👀 then ✅ with typing grades all three FIRED',
    pass: g.a1_receipt === 'fired' && g.a2_typing === 'fired' && g.a3_terminal === 'fired',
  });

  const s = gradeAffordances(silent);
  out.push({
    name: 'known-negative: an empty ledger grades NOT-FIRED, never fired',
    pass: s.a1_receipt === 'not-fired' && s.a2_typing === 'not-fired' && s.a3_terminal === 'not-fired',
  });

  const d = gradeAffordances(doubleReceipt);
  const dBroken = gradeAffordancesBroken(doubleReceipt);
  out.push({
    name: 'THE REPAIR: two 👀 and no terminal reaction — identity grading says NOT-FIRED where the old count-only matcher says FIRED',
    pass: d.a3_terminal === 'not-fired' && dBroken.a3_terminal === 'fired',
  });

  // The silent-default trap: zero typing on an adapter that never declared it
  // must read `not-supported`, which is a different fact from `not-fired`.
  const sl = gradeAffordances(slackShaped);
  out.push({
    name: 'silent-default trap: zero typing on a non-declaring adapter grades NOT-SUPPORTED, not NOT-FIRED',
    pass: sl.a2_typing === 'not-supported' && sl.a1_receipt === 'fired',
  });

  // ── the ledger-read repair, and proof it does something (§6b-ii) ────────
  // Defect: `DiscordFixture` is not an `AckLedger` and has no `replies`, so an
  // unguarded `[...fx.replies]` threw and killed the whole discord leg. The
  // repair guards `replies`/`journal` — the two fields that only DESCRIBE — and
  // deliberately does NOT guard `reactions`/`typing`, the two that GRADE.
  const fixtureWithoutReplies = { reactions: [{ emoji: EYES }], typing: [] };
  const fixtureWithoutCounters = { replies: [] };

  let repairedOk = false;
  try {
    readLedger(fixtureWithoutReplies);
    repairedOk = true;
  } catch {
    repairedOk = false;
  }
  let brokenWouldThrow = false;
  try {
    // The pre-repair read, verbatim.
    // eslint-disable-next-line no-unused-vars
    const _ = [...fixtureWithoutReplies.replies];
  } catch {
    brokenWouldThrow = true;
  }
  out.push({
    name: 'THE REPAIR (ledger read): a fixture with no `replies` is read successfully — where the old unguarded read THREW and killed the leg',
    pass: repairedOk === true && brokenWouldThrow === true,
  });

  // The free-negative guard: a fixture missing the counters that GRADE must
  // throw, not quietly yield an empty ledger that reads as a clean `not-fired`.
  let gradingCountersStillFatal = false;
  try {
    readLedger(fixtureWithoutCounters);
  } catch {
    gradingCountersStillFatal = true;
  }
  out.push({
    name: 'free-negative guard: a fixture missing `reactions`/`typing` THROWS rather than grading a silent NOT-FIRED',
    pass: gradingCountersStillFatal === true,
  });

  // ── the keepalive-window repair, on the REAL measured timeline ──────────
  // These are the actual offsets from `/root/f24na-run7/telegram-K-keepalive`,
  // read back out of the fixture journal (seconds after the inbound submit):
  //   0.1 👀 | 0.1 typing | 5.1 typing | 10.1 typing | 12.7 ✅ + reply | watch to ~17
  // A correct instrument must call this ABORTED (3 during, 0 after).
  const realTimeline = {
    typingAt: [100, 5100, 10100],
    terminalAt: 12700,
    replyAt: 12700,
    watchEndAt: 17000,
  };
  const v = keepaliveVerdict(realTimeline);
  out.push({
    name: 'known-positive (real measured timeline): 3 refreshes during a 12.7s turn, 0 after — loop ran AND guard aborted',
    pass: v.gradeable && v.during === 3 && v.after === 0 && v.loop_ran === true && v.aborted === true,
  });

  // A genuine leak: the loop keeps firing past the terminal reaction. If the
  // gate cannot redden here it cannot detect the defect it exists for.
  const leaked = keepaliveVerdict({
    typingAt: [100, 5100, 10100, 15100, 20100],
    terminalAt: 12700,
    replyAt: 12700,
    watchEndAt: 27000,
  });
  out.push({
    name: 'known-negative (synthetic leak): refreshes continuing past the terminal reaction grade NOT-ABORTED',
    pass: leaked.aborted === false && leaked.after === 2,
  });

  // THE REPAIR, third assertion (§6b-ii). The old marker opened the window at
  // turn-OBSERVATION + 3s (= 3100ms here) instead of at the guard drop. On the
  // very same real timeline it counts 2 in-turn refreshes as leakage — the
  // false alarm this gate actually produced on its first run.
  const oldMarker = 3100;
  const oldAfter = realTimeline.typingAt.filter((t) => t > oldMarker).length;
  out.push({
    name: 'THE REPAIR (keepalive window): the OLD turn-observation marker reports 2 phantom post-turn signals on the same real timeline the repaired marker grades as 0',
    pass: oldAfter === 2 && v.after === 0,
  });

  return out;
}

// ---------------------------------------------------------------------------
// The keepalive-lifecycle verdict, and WHY its window marker is what it is
// ---------------------------------------------------------------------------
// FIRST VERSION OF THIS GATE WAS WRONG AND RAISED A FALSE LEAK ALARM. It marked
// the start of the "after the turn" window at `turn_ran` + 3s. But `turn_ran`
// is observed when the LLM fixture RECEIVES the request, and on the keepalive
// leg the fixture then holds the response open for 12s. So the window opened
// ~9s BEFORE the turn actually ended, and counted two in-turn keepalive
// refreshes as post-turn leakage. Measured verdict: `typing_after=2`, i.e. a
// reported product leak that did not exist.
//
// The faithful marker is PLATFORM-SIDE and comes from `run_turn` itself
// (`channel_inbound.rs:544-555`): the typing guard is dropped FIRST, and only
// then is the terminal reaction sent and the reply dispatched. So the first
// terminal reaction (or, on an adapter with no `react`, the reply) is the
// earliest observable that is guaranteed to be AFTER the guard drop. Anything
// counted after it is genuine leakage.
export function keepaliveVerdict({ typingAt, terminalAt, replyAt, watchEndAt }) {
  // Prefer the terminal reaction; fall back to the reply for adapters with no
  // `react` override. The reply is strictly LATER than the guard drop, so the
  // fallback shrinks the window — conservative, and cannot manufacture a leak.
  const end = terminalAt ?? replyAt ?? null;
  if (end === null) return { gradeable: false, reason: 'no post-guard-drop marker observed', during: 0, after: 0 };
  const during = typingAt.filter((t) => t <= end).length;
  const after = typingAt.filter((t) => t > end).length;
  return {
    gradeable: true,
    during,
    after,
    watch_ms_after_end: watchEndAt !== null && watchEndAt !== undefined ? watchEndAt - end : null,
    // The LOOP must be proven to iterate, not merely to have fired once: the
    // keepalive sends immediately and then every 5s, so `>= 2` is the weakest
    // assertion that distinguishes "the loop runs" from "the first send ran".
    loop_ran: during >= 2,
    aborted: after === 0,
  };
}

/** Read an adapter fixture's ack ledger. See the self-test above for why the
 * describing fields are guarded and the grading fields are not. */
export function readLedger(fx) {
  return {
    reactions: [...fx.reactions],
    typing: [...fx.typing],
    replies: Array.isArray(fx.replies) ? [...fx.replies] : [],
  };
}

// ---------------------------------------------------------------------------
// Shared fixture base — the ack ledger every adapter fixture fills
// ---------------------------------------------------------------------------
class AckLedger {
  constructor() {
    /** @type {{emoji:string, at:number, conv:string, msg:string}[]} */
    this.reactions = [];
    /** @type {{at:number, conv:string}[]} */
    this.typing = [];
    /** @type {{at:number, text:string}[]} */
    this.replies = [];
    /** @type {object[]} */
    this.journal = [];
  }

  rec(kind, detail) {
    this.journal.push({ kind, at: new Date().toISOString(), ...detail });
  }

  reaction(emoji, conv, msg) {
    this.reactions.push({ emoji, at: Date.now(), conv, msg });
    this.rec('reaction', { emoji, conv, msg });
  }

  typed(conv) {
    this.typing.push({ at: Date.now(), conv });
    this.rec('typing', { conv });
  }

  replied(text) {
    this.replies.push({ at: Date.now(), text });
    this.rec('reply', { text });
  }
}

function sendJson(res, obj, status = 200) {
  res.writeHead(status, { 'content-type': 'application/json' });
  res.end(JSON.stringify(obj));
}

function readBody(req) {
  return new Promise((resolve) => {
    let b = '';
    req.on('data', (c) => {
      b += c;
    });
    req.on('end', () => resolve(b));
  });
}

async function listen(server) {
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  return `http://127.0.0.1:${server.address().port}`;
}

// ---------------------------------------------------------------------------
// LLM fixture — in-process, with an optional per-turn DELAY
// ---------------------------------------------------------------------------
// `f24-llm-fixture.mjs` has no delay knob and is shared with live lanes, so it
// is not modified. The delay is what makes the keepalive-lifecycle gate
// possible: the keepalive refreshes every 5s, so a turn shorter than 5s emits
// exactly ONE typing signal and cannot distinguish "the loop ran once" from
// "the loop is running forever".
class LlmFixture extends AckLedger {
  constructor({ delayMs = 0 } = {}) {
    super();
    this.delayMs = delayMs;
    this.turns = [];
    this.server = http.createServer(async (req, res) => {
      const body = await readBody(req);
      const url = new URL(req.url, 'http://127.0.0.1');
      if (url.pathname.endsWith('/models')) {
        return sendJson(res, { object: 'list', data: [{ id: 'f24na-fixture', object: 'model' }] });
      }
      if (!url.pathname.endsWith('/chat/completions')) {
        return sendJson(res, { error: { message: `unknown ${url.pathname}` } }, 404);
      }
      let parsed = {};
      try {
        parsed = JSON.parse(body);
      } catch {
        /* recorded as an empty turn regardless */
      }
      const userText = lastUserText(parsed.messages);
      this.turns.push({ at: Date.now(), user_text: userText, model: parsed.model ?? null });
      // The delay happens AFTER the turn is recorded, so `turn_ran` is true from
      // the instant the dispatch begins — otherwise a slow leg would be
      // indistinguishable from a leg whose message was never admitted.
      if (this.delayMs > 0) await sleep(this.delayMs);
      const reply = `F24NA-REPLY ${correlationOf(userText)}`;
      if (!parsed.stream) {
        return sendJson(res, {
          id: 'f24na-1',
          object: 'chat.completion',
          created: Math.floor(Date.now() / 1000),
          model: parsed.model ?? 'f24na-fixture',
          choices: [{ index: 0, message: { role: 'assistant', content: reply }, finish_reason: 'stop' }],
          usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
        });
      }
      res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
      const base = { id: 'f24na-1', object: 'chat.completion.chunk', created: Math.floor(Date.now() / 1000), model: parsed.model ?? 'f24na-fixture' };
      res.write(`data: ${JSON.stringify({ ...base, choices: [{ index: 0, delta: { role: 'assistant' }, finish_reason: null }] })}\n\n`);
      res.write(`data: ${JSON.stringify({ ...base, choices: [{ index: 0, delta: { content: reply }, finish_reason: null }] })}\n\n`);
      res.write(`data: ${JSON.stringify({ ...base, choices: [{ index: 0, delta: {}, finish_reason: 'stop' }], usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 } })}\n\n`);
      res.write('data: [DONE]\n\n');
      res.end();
    });
  }

  async start() {
    this.url = await listen(this.server);
    return this.url;
  }

  async stop() {
    await new Promise((r) => this.server.close(r));
  }
}

function lastUserText(messages) {
  if (!Array.isArray(messages)) return '';
  for (let i = messages.length - 1; i >= 0; i -= 1) {
    const m = messages[i];
    if (!m || m.role !== 'user') continue;
    if (typeof m.content === 'string') return m.content;
    if (Array.isArray(m.content)) {
      const t = m.content.filter((p) => p && typeof p.text === 'string').map((p) => p.text).join(' ');
      if (t) return t;
    }
  }
  return '';
}

function correlationOf(text) {
  const m = /f24na-[a-z0-9-]+/i.exec(text ?? '');
  return m ? m[0] : 'no-correlation';
}

// ---------------------------------------------------------------------------
// TELEGRAM fixture — getUpdates long-poll + sendChatAction + setMessageReaction
// ---------------------------------------------------------------------------
// The adapter calls (`wcore-channel-telegram/src/lib.rs`):
//   send_typing :360 -> POST sendChatAction  {chat_id, action:"typing"}
//   react       :373 -> POST setMessageReaction {chat_id, message_id, reaction:[…]}
//
// The shared `f24-tg-fixture.mjs` would answer both through its catch-all
// (`unhandled_method` -> ok:true), so a COUNT would survive there — but the
// EMOJI would not, and this lane grades on emoji identity. Hence a local
// fixture rather than an edit to a file five other lanes are using.
class TelegramFixture extends AckLedger {
  constructor({ token }) {
    super();
    this.token = token;
    this.nextUpdateId = 1;
    this.pending = [];
    this.polls = 0;
    this.server = http.createServer(async (req, res) => {
      const body = await readBody(req);
      const url = new URL(req.url, 'http://127.0.0.1');
      const m = /^\/bot([^/]+)\/(\w+)$/.exec(url.pathname);
      if (!m) return sendJson(res, { ok: false, error_code: 404, description: 'unknown' }, 404);
      const [, tok, method] = m;
      // Answered the way Telegram answers it, so a misconfigured run fails as
      // auth rather than as silence.
      if (tok !== this.token) {
        this.rec('bad_token', { method });
        return sendJson(res, { ok: false, error_code: 401, description: 'Unauthorized' }, 401);
      }
      let parsed = {};
      try {
        parsed = JSON.parse(body);
      } catch {
        /* query-string form */
      }

      if (method === 'sendChatAction') {
        this.typed(String(parsed.chat_id ?? ''));
        return sendJson(res, { ok: true, result: true });
      }
      if (method === 'setMessageReaction') {
        // Telegram's shape: `reaction: [{type:"emoji", emoji:"👀"}]`.
        const first = Array.isArray(parsed.reaction) ? parsed.reaction[0] : null;
        const emoji = first?.emoji ?? first?.custom_emoji_id ?? '(unparsed)';
        this.reaction(emoji, String(parsed.chat_id ?? ''), String(parsed.message_id ?? ''));
        return sendJson(res, { ok: true, result: true });
      }
      if (method === 'sendMessage') {
        this.replied(String(parsed.text ?? ''));
        return sendJson(res, {
          ok: true,
          result: { message_id: 900000 + this.replies.length, date: Math.floor(Date.now() / 1000), chat: { id: Number(parsed.chat_id ?? 0) } },
        });
      }
      if (method === 'deleteWebhook' || method === 'getMe' || method === 'setMyCommands') {
        this.rec(method, {});
        return sendJson(res, { ok: true, result: true });
      }
      if (method !== 'getUpdates') {
        // Recorded, NOT silently swallowed: an affordance reaching an endpoint
        // this fixture does not model would otherwise vanish, and this lane
        // must be able to tell "no call" from "a call I failed to count".
        this.rec('unhandled_method', { method });
        return sendJson(res, { ok: true, result: [] });
      }

      // getUpdates: Telegram's real offset semantics — `offset = N` confirms
      // (destroys) every pending update with id < N.
      this.polls += 1;
      const qOffset = Number(url.searchParams.get('offset') ?? parsed.offset ?? 0);
      const qTimeout = Number(url.searchParams.get('timeout') ?? parsed.timeout ?? 0);
      const offset = Number.isFinite(qOffset) ? qOffset : 0;
      if (offset > 0) this.pending = this.pending.filter((u) => u.update_id >= offset);
      const timeoutMs = Math.min(Number.isFinite(qTimeout) ? qTimeout * 1000 : 0, 1500);
      const deadline = Date.now() + timeoutMs;
      let out = this.pending.filter((u) => u.update_id >= (offset > 0 ? offset : 0));
      while (out.length === 0 && Date.now() < deadline) {
        await sleep(25);
        out = this.pending.filter((u) => u.update_id >= (offset > 0 ? offset : 0));
      }
      return sendJson(res, { ok: true, result: out.map((u) => u.body) });
    });
  }

  submit({ chatId, senderId, text, messageId }) {
    const update_id = this.nextUpdateId;
    this.nextUpdateId += 1;
    const message_id = messageId ?? update_id;
    this.pending.push({
      update_id,
      body: {
        update_id,
        message: {
          message_id,
          date: Math.floor(Date.now() / 1000),
          chat: { id: Number(chatId), type: 'private' },
          from: { id: Number(senderId), is_bot: false, first_name: 'f24na', username: 'f24na' },
          text,
        },
      },
    });
    this.rec('submit', { update_id, message_id, text });
    return message_id;
  }

  async start() {
    this.url = await listen(this.server);
    return this.url;
  }

  async stop() {
    await new Promise((r) => this.server.close(r));
  }
}

// ---------------------------------------------------------------------------
// MATRIX fixture — /sync long-poll + typing PUT + m.reaction PUT
// ---------------------------------------------------------------------------
// The adapter calls (`wcore-channel-matrix/src/lib.rs`):
//   send_typing :305 -> PUT /_matrix/client/v3/rooms/{room}/typing/{userId}
//   react       :324 -> PUT /_matrix/client/v3/rooms/{room}/send/m.reaction/{txn}
//
// The reaction emoji rides in `content["m.relates_to"].key`, which the shared
// matrix fixture does NOT journal — again the reason for a local fixture.
class MatrixFixture extends AckLedger {
  constructor({ token, rooms }) {
    super();
    this.token = token;
    this.rooms = new Map(rooms.map((r) => [r.id, r.members]));
    this.cursor = 0;
    this.log = [];
    // Readiness observable: the adapter is attached once it has SERVED a
    // `/sync`. Counting opens (not closes) matters — an incremental sync
    // long-polls, so waiting for a close would wait a full timeout and could
    // read as "never attached" on a slow start.
    this.syncs = 0;
    this.server = http.createServer(async (req, res) => {
      const body = await readBody(req);
      const url = new URL(req.url, 'http://127.0.0.1');
      const p = url.pathname;

      const auth = req.headers.authorization ?? '';
      if (auth !== `Bearer ${this.token}`) {
        this.rec('bad_token', { path: p });
        return sendJson(res, { errcode: 'M_UNKNOWN_TOKEN', error: 'Invalid access token' }, 401);
      }

      let parsed = {};
      try {
        parsed = JSON.parse(body);
      } catch {
        /* GETs have no body */
      }

      const reaction = /^\/_matrix\/client\/v3\/rooms\/([^/]+)\/send\/m\.reaction\/([^/]+)$/.exec(p);
      if (reaction && req.method === 'PUT') {
        const rel = parsed['m.relates_to'] ?? {};
        this.cursor += 1;
        this.reaction(rel.key ?? '(unparsed)', decodeURIComponent(reaction[1]), rel.event_id ?? '');
        return sendJson(res, { event_id: `$f24nareact${this.cursor}` });
      }

      const typing = /^\/_matrix\/client\/v3\/rooms\/([^/]+)\/typing\/([^/]+)$/.exec(p);
      if (typing && req.method === 'PUT') {
        this.typed(decodeURIComponent(typing[1]));
        return sendJson(res, {});
      }

      const send = /^\/_matrix\/client\/v3\/rooms\/([^/]+)\/send\/m\.room\.message\/([^/]+)$/.exec(p);
      if (send && req.method === 'PUT') {
        this.cursor += 1;
        this.replied(String(parsed.body ?? ''));
        return sendJson(res, { event_id: `$f24nareply${this.cursor}` });
      }

      if (p === '/_matrix/client/v3/sync') {
        this.syncs += 1;
        const sinceRaw = url.searchParams.get('since');
        const sm = /^s(\d+)$/.exec(sinceRaw ?? '');
        const after = sm ? Number(sm[1]) : null;
        const qTimeout = Number(url.searchParams.get('timeout'));
        const timeoutMs = Math.min(Number.isFinite(qTimeout) ? qTimeout : 0, 1500);
        const block = () => {
          const join = {};
          for (const [roomId, members] of this.rooms.entries()) {
            const all = this.log.filter((e) => e.room === roomId);
            const slice = after === null ? [] : all.filter((e) => e.cursor > after);
            join[roomId] = {
              summary: { 'm.joined_member_count': members },
              timeline: { events: slice.map((e) => e.event), limited: false },
            };
          }
          const served = Object.values(join).flatMap((r) => r.timeline.events.length);
          return { join, count: served.reduce((a, b) => a + b, 0) };
        };
        let b = block();
        if (after !== null) {
          const deadline = Date.now() + timeoutMs;
          while (b.count === 0 && Date.now() < deadline) {
            await sleep(25);
            b = block();
          }
        }
        return sendJson(res, { next_batch: `s${this.cursor}`, rooms: { join: b.join } });
      }

      this.rec('unknown_endpoint', { method: req.method, path: p });
      return sendJson(res, {}, 404);
    });
  }

  submit({ room, sender, text, eventId }) {
    if (!this.rooms.has(room)) throw new Error(`unknown room ${room}`);
    this.cursor += 1;
    const event_id = eventId ?? `$f24naevt${this.cursor}`;
    this.log.push({
      cursor: this.cursor,
      room,
      event: {
        type: 'm.room.message',
        sender,
        event_id,
        origin_server_ts: Date.now(),
        content: { msgtype: 'm.text', body: text },
      },
    });
    this.rec('submit', { cursor: this.cursor, room, sender, event_id, text });
    return event_id;
  }

  async start() {
    this.url = await listen(this.server);
    return this.url;
  }

  async stop() {
    await new Promise((r) => this.server.close(r));
  }
}

// ---------------------------------------------------------------------------
// SLACK fixture — reactions.add + chat.postMessage
// ---------------------------------------------------------------------------
// `react` :267 maps the unicode ack emoji to a Slack SHORTCODE via
// `api::slack_emoji_name` (`api.rs:242`), which covers 👀/✅/❌. So the
// shortcode is what arrives on the wire; it is mapped BACK here so the ledger
// grades on the same emoji identity as every other adapter.
//
// Slack has NO bot-usable typing API and deliberately keeps the trait's no-op
// `send_typing` default (comment at `lib.rs:264-266`). Its A2 is therefore
// `not-supported` — and this fixture would COUNT a typing call if one were ever
// made, which is what makes that a measurement rather than an assumption.
const SLACK_SHORTCODE_TO_EMOJI = {
  eyes: EYES,
  white_check_mark: OK,
  x: BAD,
  '+1': '👍',
  tada: '🎉',
};

class SlackFixture extends AckLedger {
  constructor() {
    super();
    this.server = http.createServer(async (req, res) => {
      const body = await readBody(req);
      const url = new URL(req.url, 'http://127.0.0.1');
      const p = url.pathname;
      let parsed = {};
      try {
        parsed = JSON.parse(body);
      } catch {
        /* form-encoded fallback below */
      }
      if (!Object.keys(parsed).length && body.includes('=')) {
        parsed = Object.fromEntries(new URLSearchParams(body));
      }

      if (p === '/api/reactions.add') {
        const name = String(parsed.name ?? '');
        this.reaction(
          SLACK_SHORTCODE_TO_EMOJI[name] ?? `(shortcode:${name})`,
          String(parsed.channel ?? ''),
          String(parsed.timestamp ?? ''),
        );
        return sendJson(res, { ok: true });
      }
      // There is no Slack typing endpoint the adapter could call, but if one is
      // ever added this counts it rather than 404ing it into invisibility.
      if (p === '/api/users.setPresence' || p.includes('typing')) {
        this.typed(String(parsed.channel ?? ''));
        return sendJson(res, { ok: true });
      }
      if (p === '/api/chat.postMessage') {
        this.replied(String(parsed.text ?? ''));
        return sendJson(res, { ok: true, ts: `${Date.now() / 1000}`, channel: String(parsed.channel ?? '') });
      }
      if (p === '/api/auth.test') {
        return sendJson(res, { ok: true, user_id: 'U24NABOT', team: 'f24na', bot_id: 'B24NA' });
      }
      this.rec('unknown_endpoint', { method: req.method, path: p });
      return sendJson(res, { ok: false, error: 'unknown_method' }, 404);
    });
  }

  async start() {
    this.url = await listen(this.server);
    return this.url;
  }

  async stop() {
    await new Promise((r) => this.server.close(r));
  }
}

// ---------------------------------------------------------------------------
// MSTEAMS fixture — Bot Framework: openid + jwks + token + activities
// ---------------------------------------------------------------------------
// The adapter has NO `react` override, so A1/A3 are `not-supported` by the
// trait default (`Err(Unsupported)`). It DOES override `send_typing` :381,
// which POSTs `{"type":"typing"}` to the SAME connector endpoint the reply uses
// — so this fixture must split `activities` by `type`, or a typing signal and a
// reply would be one undifferentiated count.
class MsTeamsFixture extends AckLedger {
  constructor() {
    super();
    this.trusted = crypto.generateKeyPairSync('rsa', { modulusLength: 2048 });
    this.kid = 'f24na-kid-1';
    this.server = http.createServer(async (req, res) => {
      const body = await readBody(req);
      const url = new URL(req.url, 'http://127.0.0.1');
      const p = url.pathname;
      if (p === '/_bf/health') return sendJson(res, { ok: true });
      if (p === '/openid') {
        this.rec('openid', {});
        return sendJson(res, { jwks_uri: `${this.url}/keys` });
      }
      if (p === '/keys') {
        this.rec('jwks', {});
        return sendJson(res, this.jwks());
      }
      if (p === '/token') {
        this.rec('token', {});
        return sendJson(res, { access_token: 'f24na-connector-token', token_type: 'Bearer', expires_in: 3600 });
      }
      if (p.includes('/v3/conversations/') && p.endsWith('/activities')) {
        let parsed = {};
        try {
          parsed = JSON.parse(body);
        } catch {
          /* raw length recorded regardless */
        }
        // THE SPLIT. `send_typing` and the reply hit the same URL; only `type`
        // separates them. Conflating them would make A2 pass on a reply.
        if (parsed.type === 'typing') this.typed(p);
        else this.replied(String(parsed.text ?? ''));
        this.rec('activity_out', { type: parsed.type ?? null, raw_len: body.length });
        return sendJson(res, { id: `f24na-out-${this.journal.length}` });
      }
      this.rec('unknown_endpoint', { method: req.method, path: p });
      return sendJson(res, { error: 'unknown' }, 404);
    });
  }

  jwks() {
    const jwk = this.trusted.publicKey.export({ format: 'jwk' });
    return { keys: [{ kty: jwk.kty, kid: this.kid, use: 'sig', alg: 'RS256', n: jwk.n, e: jwk.e }] };
  }

  signJwt(claims) {
    const b64 = (b) => Buffer.from(b).toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
    const header = b64(JSON.stringify({ alg: 'RS256', typ: 'JWT', kid: this.kid }));
    const payload = b64(JSON.stringify(claims));
    const input = `${header}.${payload}`;
    return `${input}.${b64(crypto.sign('RSA-SHA256', Buffer.from(input), this.trusted.privateKey))}`;
  }

  async start() {
    this.url = await listen(this.server);
    return this.url;
  }

  async stop() {
    await new Promise((r) => this.server.close(r));
  }
}

// ---------------------------------------------------------------------------
// DISCORD — subclass the shared fixture (the pattern f24-media-actions used)
// ---------------------------------------------------------------------------
// `DiscordFixture` already counts `typing` and `reactions` with the emoji, so
// no local re-implementation is needed. It is included here so this lane's
// matrix has a row measured by the SAME instrument as the four new ones —
// otherwise discord's prior result and these would not be comparable.
class NaDiscordFixture extends DiscordFixture {
  dispatchPlain({ id, channelId, content, authorId }) {
    const targets = [...this.conns].filter((c) => c.identified);
    const payload = {
      id,
      channel_id: channelId,
      content,
      timestamp: new Date().toISOString(),
      author: { id: authorId, username: `u${authorId}`, bot: false },
      mentions: [],
      attachments: [],
    };
    const s = this.nextSeq();
    for (const c of targets) {
      c.seq = s;
      this.send(c, { op: 0, t: 'MESSAGE_CREATE', s, d: payload });
      c.delivered += 1;
    }
    this.dispatched.push({ id, s, payload, sockets: targets.length, at: Date.now() });
    return targets.length;
  }
}

// ---------------------------------------------------------------------------
// The per-adapter run
// ---------------------------------------------------------------------------

const DECLARES = {
  discord: { react: true, typing: true, react_site: 'wcore-channel-discord/src/lib.rs:444', typing_site: 'wcore-channel-discord/src/lib.rs:435' },
  telegram: { react: true, typing: true, react_site: 'wcore-channel-telegram/src/lib.rs:373', typing_site: 'wcore-channel-telegram/src/lib.rs:360' },
  matrix: { react: true, typing: true, react_site: 'wcore-channel-matrix/src/lib.rs:324', typing_site: 'wcore-channel-matrix/src/lib.rs:305' },
  slack: { react: true, typing: false, react_site: 'wcore-channel-slack/src/lib.rs:267', typing_site: 'TRAIT DEFAULT (silent no-op Ok(())) — wcore-channels/src/lib.rs:277' },
  msteams: { react: false, typing: true, react_site: 'TRAIT DEFAULT (Err(Unsupported)) — wcore-channels/src/lib.rs:294', typing_site: 'wcore-channel-msteams/src/lib.rs:381' },
};

function baseConfig({ home, llmUrl, webhook }) {
  const lines = [
    '[default]',
    'provider = "f24nafixture"',
    '',
    '[providers.f24nafixture]',
    'provider = "openai"',
    'model = "f24na-fixture"',
    'api_key = "f24na-not-a-real-key"',
    `base_url = "${llmUrl}"`,
    '',
    '[inbound_webhook]',
  ];
  if (webhook) {
    lines.push('enabled = true', `bind = "127.0.0.1:${PORTS.webhook}"`, `public_base_url = "http://127.0.0.1:${PORTS.webhook}"`);
  } else {
    lines.push('enabled = false');
  }
  lines.push('');
  fs.writeFileSync(path.join(home, 'config.toml'), lines.join('\n'), { mode: 0o600 });
}

async function waitFor(fn, budgetMs, stepMs = 250) {
  const deadline = Date.now() + budgetMs;
  while (Date.now() < deadline) {
    // eslint-disable-next-line no-await-in-loop
    if (await fn()) return true;
    // eslint-disable-next-line no-await-in-loop
    await sleep(stepMs);
  }
  return false;
}

/**
 * One leg: start fixtures, start `gateway run`, deliver ONE inbound message,
 * wait for the turn, let the ack machine finish, and read the platform-side
 * ledger. `ack` is THE variable — everything else is identical between the
 * positive leg and its control.
 */
async function runLeg({ adapter, ack, label, binary, rootDir, budgetMs, llmDelayMs = 0, postTurnWatchMs = 0 }) {
  const runDir = path.join(rootDir, `${adapter}-${label}`);
  const home = path.join(runDir, 'home');
  fs.mkdirSync(path.join(home, 'channels'), { recursive: true });
  const notes = [];
  const note = (m) => {
    notes.push(m);
    process.stderr.write(`[${adapter}/${label}] ${m}\n`);
  };

  const llm = new LlmFixture({ delayMs: llmDelayMs });
  const llmUrl = await llm.start();
  note(`llm fixture at ${llmUrl} (delay=${llmDelayMs}ms)`);

  const correlation = `f24na-${adapter}-${label}`;
  let fx;
  let deliver;
  let needsWebhook = false;
  let extraEnv = {};

  if (adapter === 'telegram') {
    const token = '24na:tg-bot-token';
    fx = new TelegramFixture({ token });
    const url = await fx.start();
    note(`telegram fixture at ${url}`);
    fs.writeFileSync(path.join(home, 'credentials.toml'), ['[secrets]', `"telegram.f24na.bot_token" = "${token}"`, ''].join('\n'), { mode: 0o600 });
    fs.writeFileSync(
      path.join(home, 'channels', 'f24na.toml'),
      [
        'name = "f24na"', 'platform = "telegram"', 'enabled = true', '',
        '[options]',
        'credential_handle = "telegram.f24na.bot_token"',
        `api_base_url = "${url}"`,
        'long_poll_timeout_secs = 1',
        'allowed_chat_ids = []', '',
        '[inbound]', 'dm = "allowlist"', 'dm_allowlist = ["24000001"]',
        'group = "disabled"', 'require_mention = false', 'tools = "conversational"',
        `ack = "${ack}"`, '',
      ].join('\n'),
    );
    deliver = () => fx.submit({ chatId: '24000001', senderId: '24000001', text: `probe ${correlation}`, messageId: 5551 });
  } else if (adapter === 'matrix') {
    const token = 'f24na-matrix-access-token';
    const room = '!f24naroom:fixture.invalid';
    const bot = '@f24nabot:fixture.invalid';
    const sender = '@f24nauser:fixture.invalid';
    fx = new MatrixFixture({ token, rooms: [{ id: room, members: 2 }] });
    const url = await fx.start();
    note(`matrix fixture at ${url}`);
    fs.writeFileSync(path.join(home, 'credentials.toml'), ['[secrets]', `"matrix.f24na.access_token" = "${token}"`, ''].join('\n'), { mode: 0o600 });
    fs.writeFileSync(
      path.join(home, 'channels', 'f24na.toml'),
      [
        'name = "f24na"', 'platform = "matrix"', 'enabled = true', '',
        '[options]',
        `homeserver_url = "${url}"`,
        'credential_handle_access_token = "matrix.f24na.access_token"',
        `user_id = "${bot}"`, '',
        '[inbound]', 'dm = "allowlist"', `dm_allowlist = ["${sender}"]`,
        'group = "disabled"', 'require_mention = false', 'tools = "conversational"',
        `ack = "${ack}"`, '',
      ].join('\n'),
    );
    deliver = () => fx.submit({ room, sender, text: `probe ${correlation}` });
  } else if (adapter === 'slack') {
    const botToken = 'xoxb-f24na-not-a-real-token';
    const signingSecret = crypto.randomBytes(24).toString('hex');
    needsWebhook = true;
    fx = new SlackFixture();
    const url = await fx.start();
    note(`slack fixture at ${url}`);
    fs.writeFileSync(
      path.join(home, 'credentials.toml'),
      ['[secrets]', `"slack.f24na.bot_token" = "${botToken}"`, `"slack.f24na.signing_secret" = "${signingSecret}"`, ''].join('\n'),
      { mode: 0o600 },
    );
    fs.writeFileSync(
      path.join(home, 'channels', 'f24na.toml'),
      [
        'name = "f24na"', 'platform = "slack"', 'enabled = true', '',
        '[options]',
        'workspace_name = "f24na"',
        'default_channel_id = "D24NADEFAULT"',
        'credential_handle_bot_token = "slack.f24na.bot_token"',
        'credential_handle_signing_secret = "slack.f24na.signing_secret"',
        `api_base_url = "${url}"`,
        'max_retry_attempts = 1', '',
        '[inbound]', 'dm = "allowlist"', 'dm_allowlist = ["U24NAALLOWED"]',
        'group = "disabled"', 'require_mention = false', 'tools = "conversational"',
        `ack = "${ack}"`, '',
      ].join('\n'),
    );
    deliver = async () => {
      const ts = `${Math.floor(Date.now() / 1000)}.000100`;
      const payload = JSON.stringify({
        type: 'event_callback',
        team_id: 'T24NA',
        event: { type: 'message', channel: 'D24NACHAN', channel_type: 'im', user: 'U24NAALLOWED', text: `probe ${correlation}`, ts, team: 'T24NA' },
      });
      const timestamp = String(Math.floor(Date.now() / 1000));
      const sig = `v0=${crypto.createHmac('sha256', signingSecret).update(`v0:${timestamp}:${payload}`).digest('hex')}`;
      const r = await fetch(`http://127.0.0.1:${PORTS.webhook}/webhooks/f24na`, {
        method: 'POST',
        headers: { 'content-type': 'application/json', 'x-slack-signature': sig, 'x-slack-request-timestamp': timestamp },
        body: payload,
      });
      note(`slack webhook POST -> ${r.status}`);
      return ts;
    };
  } else if (adapter === 'msteams') {
    const appId = 'f24na-app-id';
    const appPassword = 'f24na-not-a-real-secret';
    const sender = '29:f24na-allowed-user';
    const conv = '19:f24na@thread.v2';
    needsWebhook = true;
    fx = new MsTeamsFixture();
    const url = await fx.start();
    const serviceUrl = `${url}/amer/`;
    note(`msteams BF fixture at ${url}`);
    fs.writeFileSync(
      path.join(home, 'credentials.toml'),
      ['[secrets]', `"msteams.f24na.app_id" = "${appId}"`, `"msteams.f24na.app_password" = "${appPassword}"`, ''].join('\n'),
      { mode: 0o600 },
    );
    fs.writeFileSync(
      path.join(home, 'channels', 'f24na.toml'),
      [
        'name = "f24na"', 'platform = "msteams"', 'enabled = true', '',
        '[options]',
        'credential_handle_app_id = "msteams.f24na.app_id"',
        'credential_handle_app_password = "msteams.f24na.app_password"',
        `service_url = "${serviceUrl}"`,
        `token_url = "${url}/token"`,
        `openid_metadata_url = "${url}/openid"`, '',
        '[inbound]', 'dm = "allowlist"', `dm_allowlist = ["${sender}"]`,
        'group = "disabled"', 'require_mention = false', 'tools = "conversational"',
        `ack = "${ack}"`, '',
      ].join('\n'),
    );
    deliver = async () => {
      const now = Math.floor(Date.now() / 1000);
      const jwt = fx.signJwt({
        iss: 'https://api.botframework.com',
        aud: appId,
        iat: now - 10,
        nbf: now - 10,
        exp: now + 600,
        serviceurl: serviceUrl,
      });
      const activity = {
        type: 'message',
        id: 'f24na-activity-1',
        text: `probe ${correlation}`,
        serviceUrl,
        timestamp: new Date().toISOString(),
        from: { id: sender, name: 'F24NA User', role: 'user' },
        recipient: { id: '28:f24na-bot' },
        conversation: { id: conv, conversationType: 'personal', isGroup: false },
      };
      const r = await fetch(`http://127.0.0.1:${PORTS.webhook}/webhooks/f24na`, {
        method: 'POST',
        headers: { 'content-type': 'application/json', authorization: `Bearer ${jwt}` },
        body: JSON.stringify(activity),
      });
      note(`msteams webhook POST -> ${r.status}`);
      return activity.id;
    };
  } else if (adapter === 'discord') {
    const botToken = 'f24na-bot-token';
    fx = new NaDiscordFixture({ botToken, botId: '910000000000000001' });
    await fx.start();
    note(`discord fixture at ${fx.apiBase} (gateway ${fx.gatewayUrl})`);
    fs.writeFileSync(path.join(home, 'credentials.toml'), ['[secrets]', `"discord.f24na.bot_token" = "${botToken}"`, ''].join('\n'), { mode: 0o600 });
    fs.writeFileSync(
      path.join(home, 'channels', 'f24na.toml'),
      [
        'name = "f24na"', 'platform = "discord"', 'enabled = true', '',
        '[options]',
        'credential_handle = "discord.f24na.bot_token"',
        `api_base_url = "${fx.apiBase}"`,
        `gateway_url = "${fx.gatewayUrl}"`,
        'heartbeat_grace_ms = 30000', '',
        '[inbound]', 'dm = "allowlist"', 'dm_allowlist = ["910000000000000042"]',
        'group = "disabled"', 'require_mention = false', 'tools = "conversational"',
        `ack = "${ack}"`, '',
      ].join('\n'),
    );
    deliver = () => fx.dispatchPlain({ id: `f24na-${label}-msg1`, channelId: 'C24NA1', content: `probe ${correlation}`, authorId: '910000000000000042' });
  } else {
    throw new Error(`unknown adapter ${adapter}`);
  }

  baseConfig({ home, llmUrl, webhook: needsWebhook });

  const gwLog = path.join(runDir, 'gateway.log');
  fs.writeFileSync(gwLog, '');
  const gwOut = fs.openSync(gwLog, 'a');
  const child = spawn(binary, ['gateway', 'run'], {
    stdio: ['ignore', gwOut, gwOut],
    env: {
      ...process.env,
      WAYLAND_HOME: home,
      WAYLAND_VAULT_PASSPHRASE: 'f24na-passphrase',
      RUST_LOG: 'info,wcore_agent::channel_inbound=debug,wcore_channels=debug',
      ...extraEnv,
    },
    detached: false,
  });

  // Readiness. Each adapter has a DIFFERENT observable for "the runtime is
  // actually attached", and using the wrong one turns a race into a result.
  let ready = false;
  if (adapter === 'discord') {
    ready = await waitFor(async () => {
      const r = fx.report();
      return r.identify_count > 0 && r.live_gateway_connections > 0;
    }, budgetMs);
  } else if (adapter === 'telegram') {
    ready = await waitFor(async () => fx.polls > 0, budgetMs);
  } else if (adapter === 'matrix') {
    // At least one `/sync` reached the fixture. A `bad_token` journal entry is
    // NOT readiness — it means the adapter attached with the wrong credential,
    // which must fail the leg rather than let it proceed to a zero-count "pass".
    ready = await waitFor(async () => fx.syncs > 0, budgetMs);
    if (fx.journal.some((j) => j.kind === 'bad_token')) {
      note('matrix fixture saw a BAD TOKEN — config/credential mismatch, not a product result');
      ready = false;
    }
  } else if (needsWebhook) {
    // The webhook host binds a port; readiness is that port accepting.
    ready = await waitFor(async () => {
      try {
        const r = await fetch(`http://127.0.0.1:${PORTS.webhook}/webhooks/__f24na_probe__`, { method: 'POST', body: '{}' });
        return r.status > 0;
      } catch {
        return false;
      }
    }, budgetMs);
  }
  note(`readiness=${ready}`);

  let delivered = null;
  let turnRan = false;
  let turnEndedAt = null;
  if (ready) {
    delivered = await deliver();
    note(`delivered inbound (${JSON.stringify(delivered)})`);
    turnRan = await waitFor(async () => llm.turns.length > 0, budgetMs);
    note(`turn_ran=${turnRan} after dispatch`);
    // Let the ack state machine emit its terminal reaction.
    await sleep(3000);
    turnEndedAt = Date.now();
  } else {
    note('runtime never attached — this leg is NOT MEASURED, not a pass');
  }

  // ── the keepalive-lifecycle window ──────────────────────────────────────
  // The window is anchored to a PLATFORM-SIDE post-guard-drop marker, NOT to
  // when the turn was observed starting. See `keepaliveVerdict` for the false
  // leak alarm the turn-observation marker produced.
  let keepalive = null;
  if (postTurnWatchMs > 0 && ready) {
    // Wait for the terminal reaction (or the reply, for a no-`react` adapter)
    // — that is the guard drop becoming observable.
    const gotMarker = await waitFor(
      async () => fx.reactions.some((r) => r.emoji === OK || r.emoji === BAD) || (Array.isArray(fx.replies) && fx.replies.length > 0),
      budgetMs,
      500,
    );
    note(`post-guard-drop marker observed=${gotMarker}`);
    const until = Date.now() + postTurnWatchMs;
    while (Date.now() < until) {
      // Emits every iteration and is bounded — a silent poll is indistinguish-
      // able from a hung agent and the watchdog kills it (LANE-BRIEF §6b).
      // eslint-disable-next-line no-await-in-loop
      await sleep(2000);
      process.stderr.write(`[${adapter}/${label}] keepalive watch: ${Math.round((until - Date.now()) / 1000)}s left, typing=${fx.typing.length}\n`);
    }
    const terminal = fx.reactions.find((r) => r.emoji === OK || r.emoji === BAD);
    keepalive = keepaliveVerdict({
      typingAt: fx.typing.map((t) => t.at),
      terminalAt: terminal ? terminal.at : null,
      replyAt: Array.isArray(fx.replies) && fx.replies.length ? fx.replies[0].at : null,
      watchEndAt: Date.now(),
    });
    note(`keepalive verdict: during=${keepalive.during} after=${keepalive.after} loop_ran=${keepalive.loop_ran} aborted=${keepalive.aborted} (watched ${Math.round((keepalive.watch_ms_after_end ?? 0) / 1000)}s past the guard drop)`);
  }

  // `DiscordFixture` is the one fixture here NOT derived from `AckLedger` — it
  // carries `reactions` and `typing` (which is all the grading needs) but no
  // `replies`/`journal`. Reading them unguarded threw `fx.replies is not
  // iterable` and killed the discord leg outright. That failed LOUDLY rather
  // than grading a zero, which is the safe direction, but it is still an
  // instrument defect and is repaired here rather than written up (§6b-ii).
  //
  // The two counters that GRADE are read unguarded on purpose: if a fixture
  // ever lacks `reactions` or `typing`, this must throw rather than silently
  // substitute `[]` — an empty-array fallback would turn a missing instrument
  // into a clean `not-fired`, which is exactly the free-negative trap (§3b-i).
  const ledger = readLedger(fx);

  try {
    child.kill('SIGTERM');
  } catch {
    /* already gone */
  }
  await sleep(900);
  try {
    child.kill('SIGKILL');
  } catch {
    /* already gone */
  }
  await fx.stop();
  await llm.stop();

  const declares = DECLARES[adapter];
  const graded = gradeAffordances({ reactions: ledger.reactions, typing: ledger.typing, declares });

  const out = {
    adapter,
    label,
    ack,
    declares,
    ready,
    turn_ran: turnRan,
    turn_prompt: llm.turns[0]?.user_text ?? null,
    prompt_carries_probe: (llm.turns[0]?.user_text ?? '').includes(correlation),
    reactions_total: ledger.reactions.length,
    reaction_emojis: ledger.reactions.map((r) => r.emoji),
    typing_total: ledger.typing.length,
    typing_at_ms: ledger.typing.map((t) => t.at),
    reaction_at_ms: ledger.reactions.map((r) => r.at),
    keepalive,
    replies_total: ledger.replies.length,
    affordances: graded,
    gateway_log_bytes: fs.existsSync(gwLog) ? fs.statSync(gwLog).size : 0,
    fixture_journal: Array.isArray(fx.journal) ? fx.journal : null,
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
  const arg = (name, dflt = null) => {
    const i = argv.indexOf(name);
    return i >= 0 ? argv[i + 1] : dflt;
  };
  const selftestOnly = argv.includes('--selftest');
  const keepalive = argv.includes('--keepalive');
  const binary = arg('--binary');
  const outDir = arg('--out') ?? fs.mkdtempSync(path.join(os.tmpdir(), 'f24na-'));
  const adapters = (arg('--adapters') ?? 'telegram,matrix,slack,msteams,discord').split(',').map((s) => s.trim()).filter(Boolean);
  const budgetMs = Number(arg('--budget-ms') ?? 45000);

  const st = selfTest();
  for (const t of st) process.stderr.write(`[selftest] ${t.pass ? 'PASS' : 'FAIL'}  ${t.name}\n`);
  const stOk = st.every((t) => t.pass);
  if (selftestOnly) {
    process.stdout.write(`${JSON.stringify({ selftest: st, ok: stOk }, null, 2)}\n`);
    process.exit(stOk ? 0 : 1);
  }
  if (!stOk) {
    process.stderr.write('instrument self-test FAILED — refusing to measure with a broken instrument\n');
    process.exit(1);
  }
  if (!binary) {
    process.stderr.write('usage: f24-native-actions.mjs --binary <wayland-core> [--out <dir>] [--adapters a,b] [--keepalive]\n');
    process.exit(2);
  }

  fs.mkdirSync(outDir, { recursive: true });
  const legs = {};
  const gates = [];

  for (const adapter of adapters) {
    // POSITIVE: ack="both".
    // eslint-disable-next-line no-await-in-loop
    const P = await runLeg({ adapter, ack: 'both', label: 'P-ack-both', binary, rootDir: outDir, budgetMs });
    // NEGATIVE CONTROL: ack="off". EXACTLY ONE variable differs.
    // eslint-disable-next-line no-await-in-loop
    const N = await runLeg({ adapter, ack: 'off', label: 'N-ack-off', binary, rootDir: outDir, budgetMs });
    legs[adapter] = { P, N };

    const d = DECLARES[adapter];
    // The positive gate asserts ONLY what the adapter declares. Requiring a
    // reaction from msteams (which has no `react` override) would fail the
    // product for a platform capability it never claimed — and requiring typing
    // from slack would do the same.
    const wantReceipt = d.react ? P.affordances.a1_receipt === 'fired' : P.affordances.a1_receipt === 'not-supported';
    const wantTerminal = d.react ? P.affordances.a3_terminal === 'fired' : P.affordances.a3_terminal === 'not-supported';
    const wantTyping = d.typing ? P.affordances.a2_typing === 'fired' : P.affordances.a2_typing === 'not-supported';

    gates.push({
      id: `G-${adapter}-P`,
      adapter,
      kind: 'POSITIVE',
      desc: `ack="both": every affordance ${adapter} DECLARES is counted at the platform, by emoji identity`,
      pass: P.ready && P.turn_ran && wantReceipt && wantTerminal && wantTyping,
      detail: `ready=${P.ready} turn_ran=${P.turn_ran} emojis=${JSON.stringify(P.reaction_emojis)} typing=${P.typing_total} a1=${P.affordances.a1_receipt} a2=${P.affordances.a2_typing} a3=${P.affordances.a3_terminal}`,
    });

    gates.push({
      id: `G-${adapter}-N`,
      adapter,
      kind: 'NEGATIVE CONTROL',
      desc: 'ack="off", one variable changed: ZERO reactions and ZERO typing — WHILE THE TURN STILL RAN',
      pass: N.ready && N.turn_ran && N.reactions_total === 0 && N.typing_total === 0,
      detail: `ready=${N.ready} turn_ran=${N.turn_ran} reactions=${N.reactions_total} typing=${N.typing_total}`,
    });
  }

  // ── the keepalive-lifecycle gate ────────────────────────────────────────
  // A2's defect class: a background task that outlives the work it was spawned
  // for. The keepalive refreshes every 5s, so a turn held open for ~12s must
  // produce >= 2 typing signals (proving the LOOP runs, not just the first
  // send), and the 14s window after the turn completes must produce ZERO
  // (proving `AbortOnDrop` actually aborts). A gate asserting only ">= 1 typing"
  // — which is what every prior measurement of this clause asserted — passes
  // identically whether the guard works or leaks.
  let ka = null;
  if (keepalive) {
    const adapter = adapters.includes('telegram') ? 'telegram' : adapters[0];
    ka = await runLeg({
      adapter,
      ack: 'both',
      label: 'K-keepalive',
      binary,
      rootDir: outDir,
      budgetMs,
      llmDelayMs: 12000,
      postTurnWatchMs: 14000,
    });
    gates.push({
      id: `G-${adapter}-K`,
      adapter,
      kind: 'KEEPALIVE LIFECYCLE',
      desc: 'a ~12s turn refreshes typing at least twice (the LOOP runs, not just its first send), and ZERO typing arrives after the guard drop becomes observable (AbortOnDrop actually aborts)',
      pass: Boolean(ka.ready && ka.turn_ran && ka.keepalive?.gradeable && ka.keepalive.loop_ran && ka.keepalive.aborted),
      detail: `gradeable=${ka.keepalive?.gradeable} during=${ka.keepalive?.during} after=${ka.keepalive?.after} loop_ran=${ka.keepalive?.loop_ran} aborted=${ka.keepalive?.aborted} watched_past_end_ms=${ka.keepalive?.watch_ms_after_end}`,
    });
  }

  const summary = {
    generated_at: new Date().toISOString(),
    binary,
    out_dir: outDir,
    adapters,
    instrument_selftest: st,
    matrix: Object.fromEntries(
      Object.entries(legs).map(([a, { P }]) => [
        a,
        {
          declares: DECLARES[a],
          a1_receipt_reaction: P.affordances.a1_receipt,
          a2_typing_keepalive: P.affordances.a2_typing,
          a3_terminal_reaction: P.affordances.a3_terminal,
          emojis: P.reaction_emojis,
          typing_count: P.typing_total,
          turn_ran: P.turn_ran,
        },
      ]),
    ),
    legs,
    keepalive: ka,
    gates,
    all_pass: gates.every((g) => g.pass),
  };
  fs.writeFileSync(path.join(outDir, 'summary.json'), `${JSON.stringify(summary, null, 2)}\n`);

  process.stderr.write('\n================ GATES ================\n');
  for (const g of gates) {
    process.stderr.write(`${g.pass ? 'PASS' : 'FAIL'}  ${g.id} [${g.kind}]\n      ${g.desc}\n      ${g.detail}\n`);
  }
  process.stderr.write('\n================ MATRIX ================\n');
  for (const [a, m] of Object.entries(summary.matrix)) {
    process.stderr.write(`${a.padEnd(9)} A1=${String(m.a1_receipt_reaction).padEnd(13)} A2=${String(m.a2_typing_keepalive).padEnd(13)} A3=${m.a3_terminal_reaction}\n`);
  }
  process.stderr.write(`\nall_pass=${summary.all_pass}\nsummary: ${path.join(outDir, 'summary.json')}\n`);
  process.exit(summary.all_pass ? 0 : 1);
}

main().catch((e) => {
  process.stderr.write(`${e?.stack ?? e}\n`);
  process.exit(3);
});
