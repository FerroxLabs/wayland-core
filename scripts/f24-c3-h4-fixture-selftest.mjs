#!/usr/bin/env node
// Self-test for `f24-tg-fixture.mjs`. Runs the instrument against a
// KNOWN-POSITIVE and a KNOWN-NEGATIVE before anything is measured with it.
//
// The rule this exists for: "the instrument that hunts a defect class tends to
// carry it". A fixture that never actually destroys an update on confirm would
// report `lost=0` for every run, and the F24-C3-H4 measurement would read as a
// clean bill of health produced by an instrument incapable of saying anything
// else. Likewise a fixture that never counts overlapping requests would report
// `max_concurrent_getupdates=1` whether one manager polls or five.
//
// Four assertions, each of which fails if the fixture stops being able to see
// the thing it exists to see:
//
//   1 NEGATIVE  one poller, four updates -> it is served all four, none lost.
//   2 POSITIVE  a thief confirms first   -> the second poller is served ZERO,
//                                           and the fixture attributes each
//                                           deletion to the poll that caused it.
//   3 CONCURRENCY two overlapping long-polls -> max_concurrent_getupdates == 2.
//   4 CONCURRENCY-FLOOR a run with no poller at all -> 0, NOT 1. A fix that
//                                           makes nothing start must not be
//                                           able to pass as "one manager".
//
// Exits 0 only if all four hold, and prints each.

import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const TOKEN = '111:SELFTEST';
const runDir = fs.mkdtempSync(path.join(os.tmpdir(), 'f24-c3-h4-selftest-'));

let failures = 0;
function check(name, ok, detail) {
  process.stdout.write(`${ok ? 'ok  ' : 'FAIL'} ${name}${detail ? ` — ${detail}` : ''}\n`);
  if (!ok) failures += 1;
}

async function startFixture(tag) {
  const logPath = path.join(runDir, `${tag}.log`);
  fs.writeFileSync(logPath, '');
  const fd = fs.openSync(logPath, 'a');
  const child = spawn(
    process.execPath,
    [
      path.join(HERE, 'f24-tg-fixture.mjs'),
      '--journal',
      path.join(runDir, `${tag}.jsonl`),
      '--token',
      TOKEN,
      '--max-wait-ms',
      '1500',
    ],
    { stdio: ['ignore', fd, fd] },
  );
  for (let i = 0; i < 100; i += 1) {
    const banner = fs.readFileSync(logPath, 'utf8');
    const m = /TGFIX_READY url=(\S+)/.exec(banner);
    if (m) return { child, url: m[1] };
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`fixture ${tag} never came up`);
}

async function j(url, method = 'GET', body) {
  const res = await fetch(url, {
    method,
    headers: body ? { 'content-type': 'application/json' } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
  return res.json();
}

async function submit(url, token) {
  return j(`${url}/__control/submit`, 'POST', {
    token,
    chatId: '1',
    senderId: '1',
    username: 'u',
    text: `hello ${token}`,
  });
}

function getUpdates(url, offset, timeout) {
  return j(`${url}/bot${TOKEN}/getUpdates?offset=${offset}&timeout=${timeout}`);
}

// ── 1. NEGATIVE: a single poller loses nothing ────────────────────────────
{
  const { child, url } = await startFixture('negative');
  for (let i = 0; i < 4; i += 1) await submit(url, `t${i}`);
  const r1 = await getUpdates(url, 0, 0);
  const ids = r1.result.map((u) => u.update_id);
  const rep = await j(`${url}/__control/report`);
  check('1 NEGATIVE single poller is served all four', ids.length === 4, `served ${ids.join(',')}`);
  check(
    '1 NEGATIVE nothing was deleted before it was served',
    rep.updates.every((u) => u.serve_count >= 1),
    JSON.stringify(rep.updates.map((u) => u.serve_count)),
  );
  check(
    '1 NEGATIVE exactly one poller was seen',
    rep.max_concurrent_getupdates === 1,
    `max=${rep.max_concurrent_getupdates}`,
  );
  child.kill('SIGKILL');
}

// ── 2. POSITIVE: a competing poller destroys what the other never saw ─────
{
  const { child, url } = await startFixture('positive');
  for (let i = 0; i < 4; i += 1) await submit(url, `t${i}`);
  // The THIEF reads and then confirms — exactly what a second ChannelManager
  // with no subscriber does.
  const thief = await getUpdates(url, 0, 0);
  const maxId = Math.max(...thief.result.map((u) => u.update_id));
  await getUpdates(url, maxId + 1, 0); // the confirm that destroys them
  // The SUBSCRIBER's manager polls afterwards and finds nothing.
  const victim = await getUpdates(url, 0, 0);
  const rep = await j(`${url}/__control/report`);
  check(
    '2 POSITIVE the thief was served all four',
    thief.result.length === 4,
    `thief got ${thief.result.length}`,
  );
  check(
    '2 POSITIVE the victim is served ZERO after the confirm',
    victim.result.length === 0,
    `victim got ${victim.result.length} — the fixture is NOT destroying on confirm, so it could never detect loss`,
  );
  check(
    '2 POSITIVE every deletion is attributed to the poll that caused it',
    rep.updates.every((u) => u.deleted_by !== null),
    JSON.stringify(rep.updates.map((u) => u.deleted_by)),
  );
  child.kill('SIGKILL');
}

// ── 3. CONCURRENCY: two overlapping long-polls are counted as two ─────────
{
  const { child, url } = await startFixture('concurrency');
  // Empty queue + timeout=1 means both requests stay open together.
  const both = await Promise.all([getUpdates(url, 0, 1), getUpdates(url, 0, 1)]);
  const rep = await j(`${url}/__control/report`);
  check(
    '3 CONCURRENCY two overlapping getUpdates read as 2',
    rep.max_concurrent_getupdates === 2,
    `max=${rep.max_concurrent_getupdates}, both returned ${both.map((b) => b.result.length).join('/')}`,
  );
  child.kill('SIGKILL');
}

// ── 4. CONCURRENCY FLOOR: no poller at all reads as 0, not 1 ──────────────
{
  const { child, url } = await startFixture('floor');
  await submit(url, 'never-polled');
  const rep = await j(`${url}/__control/report`);
  check(
    '4 FLOOR a run with no poller reads as 0',
    rep.max_concurrent_getupdates === 0 && rep.poll_total === 0,
    `max=${rep.max_concurrent_getupdates} polls=${rep.poll_total}`,
  );
  child.kill('SIGKILL');
}

process.stdout.write(`\nSELFTEST failures=${failures} runDir=${runDir}\n`);
process.exit(failures === 0 ? 0 : 1);
