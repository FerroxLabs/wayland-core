#!/usr/bin/env node
// matrix-live-observer.mjs — an INDEPENDENT reader of one Matrix room.
//
// This process never drives the product. It exists to answer, from the
// homeserver, questions the product's own output cannot honestly answer about
// itself: how many events carrying nonce X exist, whether event E is really
// redacted, whether event E really has a replacement.
//
// ── the discipline this file is built around ────────────────────────────────
//
// LANE-BRIEF §3b-i: an absence is the easiest claim in the world to pass.
// "zero events carry that nonce" is confirmed by a broken token, a wrong room,
// a typo'd filter, a 403, and an empty response. So EVERY count command also
// runs a known-positive in the same invocation and refuses to report unless the
// instrument demonstrably saw something. `--count` therefore takes a
// `--control` nonce that MUST be present; if the control reads zero the command
// exits `3 INSTRUMENT-DEAD` rather than reporting the target's zero.
//
// ── scope ───────────────────────────────────────────────────────────────────
//
// Operates on exactly one room, supplied as MATRIX_ROOM_ID. It never lists
// joined rooms, never joins, never leaves, never invites, never touches account
// or room settings. `--redact` is the cleanup path and only accepts event ids
// this run produced, passed explicitly.
//
// Credentials come from the environment only and are never printed.
//
// usage:
//   node scripts/matrix-live-observer.mjs --selftest
//   ... --count <nonce> [--control <nonce-that-must-exist>]
//   ... --event <eventId>            print redaction/content state
//   ... --replacements <eventId>     print m.replace relations
//   ... --bodies                     print every m.room.message body (truncated)
//   ... --send <text>                post ONE message (observer-side, for the inbound leg)
//   ... --redact <eventId>           cleanup

import https from 'node:https';
import { URL } from 'node:url';

// ---------------------------------------------------------------------------
// pure helpers (self-tested)
// ---------------------------------------------------------------------------

/**
 * Count timeline events whose rendered body carries `nonce`.
 *
 * Counts the ORIGINAL m.room.message events only. An `m.replace` edit is a
 * second event that also carries the text, so counting every body would report
 * an edited message as two messages and turn a correct edit into a phantom
 * duplicate — the exact confusion the idempotency leg is measuring.
 */
export function countOriginals(chunk, nonce) {
  let n = 0;
  const ids = [];
  for (const ev of chunk || []) {
    if (ev.type !== 'm.room.message') continue;
    const rel = ev.content && ev.content['m.relates_to'];
    if (rel && rel.rel_type === 'm.replace') continue;
    const body = (ev.content && ev.content.body) || '';
    if (body.includes(nonce)) { n++; ids.push(ev.event_id); }
  }
  return { count: n, event_ids: ids };
}

/** Count every body carrying the nonce, edits included — the wider control. */
export function countAll(chunk, nonce) {
  let n = 0;
  for (const ev of chunk || []) {
    if (ev.type !== 'm.room.message') continue;
    if (((ev.content && ev.content.body) || '').includes(nonce)) n++;
  }
  return n;
}

/**
 * Is this fetched event really redacted?
 *
 * Graded from the BODY, never from an HTTP status. A redaction that returned
 * 200 and did nothing is precisely the failure this must be able to see, so a
 * present `content.body` reddens it no matter what the redact call returned.
 */
export function redactionState(ev) {
  if (!ev) return { state: 'ABSENT', body_present: false, redacted_because: false };
  const hasBody = !!(ev.content && typeof ev.content.body === 'string' && ev.content.body.length);
  const because = !!(ev.unsigned && ev.unsigned.redacted_because);
  if (!hasBody && because) return { state: 'REDACTED', body_present: false, redacted_because: true };
  if (hasBody) return { state: 'NOT-REDACTED', body_present: true, redacted_because: because };
  return { state: 'AMBIGUOUS', body_present: false, redacted_because: because };
}

/** The homeserver's own bundled statement that this event was replaced. */
export function bundledReplacement(ev) {
  const rel = ev && ev.unsigned && ev.unsigned['m.relations'] && ev.unsigned['m.relations']['m.replace'];
  if (!rel) return null;
  return {
    replacement_event_id: rel.event_id || null,
    new_body: (rel.content && rel.content['m.new_content'] && rel.content['m.new_content'].body) || null,
  };
}

// ---------------------------------------------------------------------------
// self-test
// ---------------------------------------------------------------------------

function selftest() {
  let pass = 0, fail = 0;
  const t = (n, c) => { if (c) { pass++; console.log(`  ok   ${n}`); } else { fail++; console.log(`  FAIL ${n}`); } };

  const chunk = [
    { type: 'm.room.message', event_id: '$a', content: { body: 'probe NONCE1' } },
    { type: 'm.room.message', event_id: '$b', content: { body: '* probe NONCE1 edited', 'm.relates_to': { rel_type: 'm.replace', event_id: '$a' } } },
    { type: 'm.room.message', event_id: '$c', content: { body: 'probe NONCE2' } },
    { type: 'm.reaction', event_id: '$d', content: {} },
  ];
  t('originals ignore the m.replace edit', countOriginals(chunk, 'NONCE1').count === 1);
  t('originals return the id', countOriginals(chunk, 'NONCE1').event_ids[0] === '$a');
  t('countAll DOES see the edit (the two differ — so the filter does something)',
    countAll(chunk, 'NONCE1') === 2);
  t('known-negative: an absent nonce counts zero', countOriginals(chunk, 'NOPE').count === 0);
  t('a second distinct nonce counts one', countOriginals(chunk, 'NONCE2').count === 1);
  // The empty nonce matches every body, so this isolates the TYPE filter:
  // 4 events, of which $d is an m.reaction and $b is an m.replace edit -> 2.
  t('m.reaction and m.replace are both excluded from the original count',
    countOriginals(chunk, '').count === 2);

  t('redacted: empty content + redacted_because',
    redactionState({ content: {}, unsigned: { redacted_because: { type: 'm.room.redaction' } } }).state === 'REDACTED');
  t('NOT redacted: body still present even WITH redacted_because (status-code lie caught)',
    redactionState({ content: { body: 'still here' }, unsigned: { redacted_because: {} } }).state === 'NOT-REDACTED');
  t('not redacted: plain event', redactionState({ content: { body: 'x' }, unsigned: {} }).state === 'NOT-REDACTED');
  t('absent event is ABSENT not REDACTED', redactionState(null).state === 'ABSENT');
  t('empty content with NO redacted_because is AMBIGUOUS, not REDACTED',
    redactionState({ content: {}, unsigned: {} }).state === 'AMBIGUOUS');

  t('bundled replacement read', bundledReplacement({
    unsigned: { 'm.relations': { 'm.replace': { event_id: '$b', content: { 'm.new_content': { body: 'new' } } } } },
  }).new_body === 'new');
  t('known-negative: no bundle', bundledReplacement({ unsigned: {} }) === null);

  console.log(`SELFTEST ${fail === 0 ? 'PASS' : 'FAIL'} passed=${pass} failed=${fail}`);
  process.exit(fail === 0 ? 0 : 1);
}

const argv = process.argv.slice(2);
if (argv.includes('--selftest')) selftest();

// ---------------------------------------------------------------------------
// live
// ---------------------------------------------------------------------------

const TOKEN = process.env.MATRIX_ACCESS_TOKEN;
const ROOM = process.env.MATRIX_ROOM_ID;
const HS = process.env.MATRIX_HOMESERVER || 'https://matrix.org';
if (!TOKEN || !ROOM) { process.stderr.write('MATRIX_ACCESS_TOKEN and MATRIX_ROOM_ID must be set\n'); process.exit(2); }
const ENC = encodeURIComponent(ROOM);

function req(method, path, body) {
  return new Promise((resolve, reject) => {
    const u = new URL(HS + path);
    const data = body === undefined ? null : Buffer.from(JSON.stringify(body));
    const r = https.request({
      hostname: u.hostname, port: u.port || 443, path: u.pathname + u.search, method,
      headers: {
        authorization: `Bearer ${TOKEN}`,
        ...(data ? { 'content-type': 'application/json', 'content-length': data.length } : {}),
      },
    }, (res) => {
      const c = [];
      res.on('data', (d) => c.push(d));
      res.on('end', () => {
        const raw = Buffer.concat(c).toString('utf8');
        let json = null;
        try { json = JSON.parse(raw); } catch { /* non-JSON */ }
        resolve({ status: res.statusCode, json, raw: raw.slice(0, 500) });
      });
    });
    r.on('error', reject);
    if (data) r.write(data);
    r.end();
  });
}

async function timeline(limit = 200) {
  const out = [];
  let from = '';
  for (let page = 0; page < 4; page++) {
    const q = `/_matrix/client/v3/rooms/${ENC}/messages?dir=b&limit=${limit}${from ? `&from=${encodeURIComponent(from)}` : ''}`;
    const r = await req('GET', q);
    if (r.status !== 200) return { error: `messages HTTP ${r.status} ${r.raw}`, chunk: out };
    out.push(...(r.json.chunk || []));
    if (!r.json.end || !r.json.chunk || r.json.chunk.length === 0) break;
    from = r.json.end;
  }
  return { chunk: out };
}

function argOf(flag) { const i = argv.indexOf(flag); return i >= 0 ? argv[i + 1] : null; }

(async () => {
  if (argv.includes('--count')) {
    const nonce = argOf('--count');
    const control = argOf('--control');
    const tl = await timeline();
    if (tl.error) { console.log(`OBSERVER=INSTRUMENT-DEAD reason=${tl.error}`); process.exit(3); }
    console.log(`timeline_events=${tl.chunk.length}`);
    if (control !== null) {
      const c = countOriginals(tl.chunk, control);
      console.log(`control_nonce=${control} control_count=${c.count}`);
      if (c.count < 1) {
        console.log('OBSERVER=INSTRUMENT-DEAD reason=known-positive control read zero; the target count below is NOT reportable');
        process.exit(3);
      }
    }
    const t = countOriginals(tl.chunk, nonce);
    console.log(`nonce=${nonce} originals=${t.count} all_bodies=${countAll(tl.chunk, nonce)} ids=${JSON.stringify(t.event_ids)}`);
    process.exit(0);
  }

  if (argv.includes('--event')) {
    const id = argOf('--event');
    const r = await req('GET', `/_matrix/client/v3/rooms/${ENC}/event/${encodeURIComponent(id)}`);
    console.log(`http_status=${r.status}`);
    if (r.status !== 200) { console.log(`event=${id} state=NOT-FOUND raw=${r.raw}`); process.exit(0); }
    const st = redactionState(r.json);
    console.log(`event=${id} state=${st.state} body_present=${st.body_present} redacted_because=${st.redacted_because}`);
    console.log(`body=${JSON.stringify((r.json.content && r.json.content.body) || null)}`);
    const b = bundledReplacement(r.json);
    console.log(`bundled_replacement=${JSON.stringify(b)}`);
    process.exit(0);
  }

  if (argv.includes('--replacements')) {
    const id = argOf('--replacements');
    // `/relations/` is a **v1** route. The first draft of this file used v3 and
    // got a flat 404 for an event that demonstrably HAD a replacement — a gate
    // with no reachable pass state (LANE-BRIEF §3b-iii), which would have
    // reported "no replacement" forever no matter what the product did. Caught
    // only because the bundled `unsigned.m.relations` on the original said
    // otherwise in the same capture.
    const r = await req('GET', `/_matrix/client/v1/rooms/${ENC}/relations/${encodeURIComponent(id)}/m.replace`);
    console.log(`http_status=${r.status}`);
    if (r.status !== 200) console.log(`RELATIONS_UNREADABLE raw=${r.raw}`);
    const chunk = (r.json && r.json.chunk) || [];
    console.log(`replacement_count=${chunk.length}`);
    for (const ev of chunk) {
      console.log(`  replacement id=${ev.event_id} rel_type=${ev.content?.['m.relates_to']?.rel_type} new_body=${JSON.stringify(ev.content?.['m.new_content']?.body)} fallback_body=${JSON.stringify(ev.content?.body)}`);
    }
    process.exit(0);
  }

  if (argv.includes('--bodies')) {
    const tl = await timeline();
    if (tl.error) { console.log(`OBSERVER=INSTRUMENT-DEAD reason=${tl.error}`); process.exit(3); }
    console.log(`timeline_events=${tl.chunk.length}`);
    for (const ev of tl.chunk) {
      if (ev.type !== 'm.room.message') continue;
      const rel = ev.content?.['m.relates_to']?.rel_type || '-';
      console.log(`  ${ev.event_id} sender=${ev.sender} rel=${rel} body=${JSON.stringify((ev.content?.body || '').slice(0, 120))}`);
    }
    process.exit(0);
  }

  if (argv.includes('--send')) {
    const text = argOf('--send');
    const txn = `observer-${Date.now()}`;
    const r = await req('PUT', `/_matrix/client/v3/rooms/${ENC}/send/m.room.message/${txn}`, { msgtype: 'm.text', body: text });
    console.log(`http_status=${r.status} event_id=${r.json?.event_id || null}`);
    process.exit(r.status === 200 ? 0 : 1);
  }

  if (argv.includes('--redact')) {
    const id = argOf('--redact');
    const txn = `observer-redact-${Date.now()}`;
    const r = await req('PUT', `/_matrix/client/v3/rooms/${ENC}/redact/${encodeURIComponent(id)}/${txn}`, {});
    console.log(`redact ${id} http_status=${r.status} event_id=${r.json?.event_id || null}`);
    process.exit(r.status === 200 ? 0 : 1);
  }

  process.stderr.write('no command given\n');
  process.exit(2);
})();
