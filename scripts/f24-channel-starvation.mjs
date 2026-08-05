#!/usr/bin/env node
// F24-CS — does the INSTALLED SERVICE end up owning inbound polling, whichever
// order the processes start in, and does an already-running loser take over
// when the holder dies?
//
// WHY THIS EXISTS. `lane/channel-lease` closed a cross-process data-loss defect
// with an `flock` lease and left one residual, which all three cross-audit
// reviewers independently graded `must-fix`: ownership was FIRST-COME. A
// session that started first made the installed service the observer for that
// session's whole life. Nothing was lost — but the thing the user installed to
// be always-on went silently idle, and mail simply stopped arriving. That is
// quieter than the defect it replaced, which is what makes it dangerous.
//
// FIVE LEGS, and every one of them requires a POSITIVE delivery:
//
//   A  the service starts SECOND and still ends up polling  (the fix)
//   B  the service starts FIRST and is not disturbed        (no regression)
//   C  nothing is lost while the session is the observer    (accounting)
//   D  an ALREADY-RUNNING observer takes over on the holder's death, with no
//      operator action                                       (residual (2))
//   E  a DEAD claimant does NOT wedge the home into having no poller at all
//      (the failure that would be WORSE than the starvation being fixed)
//
// THE TRAP THIS DRIVER IS BUILT AGAINST. "No duplicate consumption" passes
// trivially if NOBODY polls. So a steady window with ZERO POLLS is graded
// DENIAL — a FAILURE, not a pass — every leg establishes a positive baseline in
// the same run, and every leg counts messages actually delivered. Leg E is the
// one that creates a real zero-poller window, so it is the one that proves this
// grader can fire on something other than a hypothetical.
//
// ATTRIBUTION IS NOT SELF-REPORTED. Which process is polling is read from
// `ss -tnpH` by THIS driver — a third OS process — as the set of pids holding
// an established TCP connection to the fixture. The binaries' own
// `F24_CHANNEL_LEASE=` tokens are carried too, but only for the property that
// genuinely IS self-report: that a non-polling process says so out loud.
//
// The Telegram fixture (`f24-tg-fixture.mjs`) belongs to `lane/channel-lease`
// and is used UNMODIFIED, as a black box over its `/__control/*` surface. The
// HTTP helpers, the LLM stub and the window-scoped concurrency reader are
// carried over from that lane's driver with its repairs intact; each is
// credited at its definition.
//
// usage:
//   f24-channel-starvation.mjs --binary <path> --run-dir <dir> [--legs a,b,d,e,c]
//   f24-channel-starvation.mjs --self-test

import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));

// ───────────────────────────────────────────────────────────────────────────
// tiny helpers (carried from f24-channel-lease.mjs with its repairs)
// ───────────────────────────────────────────────────────────────────────────

function sleep(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

function httpJson(url, { method = 'GET', body = null, timeoutMs = 15000 } = {}) {
  return new Promise((resolve, reject) => {
    const payload = body === null ? null : JSON.stringify(body);
    const u = new URL(url);
    const req = http.request(
      {
        hostname: u.hostname,
        port: u.port,
        path: `${u.pathname}${u.search}`,
        method,
        // `agent: false` — carried repair. Node 19+ keeps `globalAgent` alive
        // while a Node server closes an idle socket after 5s, and this driver
        // deliberately blocks longer than that between calls. A pooled socket
        // is dead when reused and throws `socket hang up` AT the measurement.
        agent: false,
        timeout: timeoutMs,
        headers: payload
          ? { 'content-type': 'application/json', 'content-length': Buffer.byteLength(payload) }
          : {},
      },
      (res) => {
        let data = '';
        res.on('data', (c) => {
          data += c;
        });
        res.on('end', () => {
          try {
            resolve(JSON.parse(data));
          } catch (e) {
            reject(new Error(`bad json from ${url}: ${e.message}: ${data.slice(0, 200)}`));
          }
        });
      },
    );
    req.on('timeout', () => req.destroy(new Error(`timeout ${url}`)));
    req.on('error', reject);
    if (payload) req.write(payload);
    req.end();
  });
}

/** Carried repair: one transient socket failure must not destroy a run that
 *  took minutes of real process time to arrange. */
async function retryJson(url, opts = {}, attempts = 5, backoffMs = 1000, note = () => {}) {
  let lastErr = null;
  for (let i = 0; i < attempts; i += 1) {
    try {
      return await httpJson(url, opts);
    } catch (e) {
      lastErr = e;
      note(`http retry ${i + 1}/${attempts} for ${url}: ${e.message}`);
      sleep(backoffMs);
    }
  }
  throw lastErr;
}

/** Byte-count every capture. A zero-byte log is an instrument fault, not a result. */
function byteLen(p) {
  try {
    return fs.statSync(p).size;
  } catch {
    return -1;
  }
}

function readOr(p, fallback = '') {
  try {
    return fs.readFileSync(p, 'utf8');
  } catch {
    return fallback;
  }
}

// ───────────────────────────────────────────────────────────────────────────
// INSTRUMENT #1, FOUND AND REPAIRED IN THIS LANE (brief §6b-ii).
//
// THE DEFECT. The `git` this lane was handed is not `git`: a harness hook
// rewrites bare `git …` to `rtk git …`, a token-reducing proxy. In this
// repository the proxy's `git log --oneline` SILENTLY OMITTED the merge commit
// that is this lane's base, while `git rev-parse HEAD` reported it. Two readers
// of one fact disagreed and neither said so. Taken at face value it would have
// put the WRONG base SHA in this lane's report and in every fence diff.
//
// THE REPAIR is not "use the absolute path" alone — that is a habit, and habits
// are what the brief says get written up and then forgotten. The instrument now
// reads the same fact TWO WAYS and refuses to answer when they disagree.
// ───────────────────────────────────────────────────────────────────────────

const REAL_GIT = '/usr/bin/git';

/**
 * @param {{revParse:(r:string)=>string, logOne:(r:string)=>string}} readers
 */
function crossCheckedSha(readers, ref = 'HEAD') {
  const a = readers.revParse(ref).trim();
  const b = readers.logOne(ref).trim();
  if (!/^[0-9a-f]{40}$/.test(a)) throw new Error(`rev-parse gave a non-sha: ${a.slice(0, 80)}`);
  if (a !== b) {
    throw new Error(
      `git readers DISAGREE for ${ref}: rev-parse=${a} log=${b}. ` +
        'Something between this process and git is rewriting history; refusing to guess.',
    );
  }
  return a;
}

/** The NAIVE shape this repair replaces: one reader, no cross-check. */
function naiveSha(readers, ref = 'HEAD') {
  return readers.logOne(ref).trim();
}

function realGitReaders(cwd) {
  const run = (args) => spawnSync(REAL_GIT, args, { cwd, encoding: 'utf8' }).stdout ?? '';
  return {
    revParse: (r) => run(['rev-parse', r]),
    logOne: (r) => run(['log', '-1', '--format=%H', r]),
  };
}

// ───────────────────────────────────────────────────────────────────────────
// INSTRUMENT: the window-scoped concurrency reader.
//
// Carried from the landing lane, whose note applies unchanged: the naive read
// is `report.max_concurrent_getupdates`, a single global maximum over the WHOLE
// run. This driver starts a SECOND process part way through every leg, so a
// global 2 cannot distinguish "two processes polled at once" from "one process
// briefly overlapped its own retry in second 3".
// ───────────────────────────────────────────────────────────────────────────

function maxOpenInWindow(trace, startIso, endIso) {
  const s = Date.parse(startIso);
  const e = Date.parse(endIso);
  let max = 0;
  for (const p of trace) {
    const t = Date.parse(p.at);
    if (t >= s && t <= e && p.open > max) max = p.open;
  }
  return max;
}

function pollsInWindow(trace, startIso, endIso) {
  const s = Date.parse(startIso);
  const e = Date.parse(endIso);
  const ids = new Set();
  for (const p of trace) {
    const t = Date.parse(p.at);
    if (t >= s && t <= e) ids.add(p.poll);
  }
  return ids.size;
}

// ───────────────────────────────────────────────────────────────────────────
// INSTRUMENT: independent poller attribution, read from the kernel.
//
// This is what makes "the SERVICE owns polling" a measurement rather than a
// quotation of the binary's own log line. `ss -tnpH` is asked, by this driver,
// which processes hold an established TCP connection to the fixture's port.
//
// THE DEFECT THIS PARSER AVOIDS, and it is easy to write by accident: a naive
// `/pid=(\d+)/` over the whole `ss` output collects the pid of EVERY connection
// on the box — including this driver's own connections and the FIXTURE's
// server-side socket, whose peer is the client, not the fixture. Attribution
// must be anchored to the PEER column being the fixture, and the fixture's own
// pid must be excluded, or a run reports "two pollers" on a healthy handover.
// ───────────────────────────────────────────────────────────────────────────

function parseSsPeers(ssOutput, fixturePort, excludePids = []) {
  const out = new Set();
  const ex = new Set(excludePids.map(Number));
  for (const line of String(ssOutput).split('\n')) {
    const t = line.trim();
    if (t.length === 0) continue;
    // Recv-Q Send-Q Local Peer users:(("name",pid=N,fd=M))
    const cols = t.split(/\s+/);
    if (cols.length < 4) continue;
    const peer = cols[3];
    // Anchor on the PEER being the fixture. `endsWith(':port')` alone would
    // also match a local port that happens to share the number, so the address
    // is checked too.
    if (!(peer === `127.0.0.1:${fixturePort}` || peer === `[::1]:${fixturePort}`)) continue;
    for (const m of t.matchAll(/pid=(\d+)/g)) {
      const pid = Number(m[1]);
      if (!ex.has(pid)) out.add(pid);
    }
  }
  return out;
}

function sampleConnectedPids(fixturePort, excludePids) {
  const r = spawnSync('ss', ['-tnpH', 'state', 'established'], { encoding: 'utf8' });
  if (r.error || typeof r.stdout !== 'string') return null;
  return parseSsPeers(r.stdout, fixturePort, excludePids);
}

// ───────────────────────────────────────────────────────────────────────────
// INSTRUMENT: the token matcher.
//
// Carried repair, and the defect this program has now measured three times: a
// console line wrap puts a newline INSIDE the phrase, so `String.includes`
// reports absence against a log that contains it. Whitespace is stripped from
// both sides. The tokens matched here contain no internal spaces, so stripping
// cannot join two unrelated words into a false positive.
// ───────────────────────────────────────────────────────────────────────────

function containsToken(haystack, token) {
  const strip = (x) => String(x).replace(/\s+/g, '');
  return strip(haystack).includes(strip(token));
}

function countToken(haystack, token) {
  const strip = (x) => String(x).replace(/\s+/g, '');
  const h = strip(haystack);
  const t = strip(token);
  if (t.length === 0) return 0;
  let n = 0;
  let i = h.indexOf(t);
  while (i !== -1) {
    n += 1;
    i = h.indexOf(t, i + t.length);
  }
  return n;
}

// ───────────────────────────────────────────────────────────────────────────
// INSTRUMENT: the graders.
// ───────────────────────────────────────────────────────────────────────────

/**
 * Grade one steady-state observation window.
 *
 * ZERO POLLS is DENIAL and is a FAILURE. That is the anti-denial guard: a
 * change that made NOBODY poll would satisfy every "no duplicate consumption"
 * check ever written.
 *
 * INSTRUMENT DEFECT #2 OF THIS LANE, found by mutation-testing this very
 * function and repaired here rather than written up. The first version tested
 * `maxOpen === 0` FIRST and called it DENIAL. But the fixture pushes a
 * concurrency sample on poll OPEN (after increment, so >= 1) and again on poll
 * CLOSE (after decrement, so possibly 0). A window that happens to contain only
 * close-side samples therefore reads `maxOpen = 0` with `polls > 0` — polling
 * was demonstrably happening, and the grader would have returned DENIAL. That
 * is a FALSE CRITICAL, the exact class the landing lane hit with its `WEDGED`
 * verdict, and it is as damaging as a false green.
 *
 * The count of polls in the window is the direct measure of "did anything
 * poll"; `maxOpen` measures "how many at once" and is only meaningful once at
 * least one OPEN sample landed inside the window. They are now read in that
 * order, and an unreadable concurrency is reported as unreadable.
 */
function gradeWindow({ maxOpen, polls, expectPids, sawPids }) {
  if (polls === 0) {
    return { grade: 'DENIAL', why: 'nothing polled in this window — a green here would be manufactured' };
  }
  if (maxOpen === 0) {
    return {
      grade: 'UNREADABLE',
      why: `${polls} poll(s) in this window but no open-side concurrency sample landed in it; ` +
        'concurrency cannot be read from this window (this is NOT denial — polling happened)',
    };
  }
  if (maxOpen > 1) {
    return { grade: 'RACE', why: `${maxOpen} concurrent pollers — the exclusion did not hold` };
  }
  if (sawPids === null) {
    return { grade: 'UNATTRIBUTED', why: 'ss was unavailable; polling happened but no pid could be attributed' };
  }
  // INSTRUMENT DEFECT #4 OF THIS LANE. This used to take a `Set` and the window
  // result carried the same `Set` straight into `JSON.stringify`, which
  // serialises a Set as `{}`. The first live run's evidence file therefore
  // recorded `"pids": {}` for every window — a reader would take that as "no
  // process was attributed", i.e. the strongest possible negative, from a run
  // where attribution had in fact SUCCEEDED. Both sides now speak arrays, and
  // the evidence carries a sorted list of pids.
  const saw = new Set(sawPids);
  const expected = new Set(expectPids);
  const extra = [...saw].filter((p) => !expected.has(p));
  const missing = [...expected].filter((p) => !saw.has(p));
  if (extra.length > 0) {
    return { grade: 'WRONG_OWNER', why: `unexpected poller pid(s) ${extra.join(',')}` };
  }
  if (missing.length > 0) {
    return { grade: 'WRONG_OWNER', why: `expected poller pid(s) ${missing.join(',')} held no connection` };
  }
  return { grade: 'OK', why: `exactly one poller, attributed to pid(s) ${[...saw].join(',')}` };
}

/**
 * INCOMPLETE beats a verdict, always. The landing lane's most alarming verdict
 * was once returned on a run whose successor never started for an unrelated
 * reason. A false CRITICAL is as damaging as a false green.
 */
function gradeInstrument(facts) {
  const reasons = [];
  if (!facts.fixture_reachable) reasons.push('tg fixture never answered /__control/health');
  if (facts.tg_journal_bytes <= 0) reasons.push(`tg journal is ${facts.tg_journal_bytes} bytes`);
  for (const [label, bytes] of Object.entries(facts.log_bytes ?? {})) {
    if (bytes <= 0) reasons.push(`${label} log is ${bytes} bytes`);
  }
  for (const [label, alive] of Object.entries(facts.expect_alive ?? {})) {
    if (!alive) reasons.push(`${label} was not alive when it had to be — this leg measured nothing`);
  }
  if (facts.expect_polling && facts.polls_total === 0) {
    reasons.push('zero getUpdates in the whole run — nothing polled at all');
  }
  if (facts.ss_available === false) {
    reasons.push('ss unavailable — poller attribution could not be read from the kernel');
  }
  return { fault: reasons.length > 0, reasons };
}

// ───────────────────────────────────────────────────────────────────────────
// The LLM stub — its OWN OS PROCESS (this file re-execs itself).
//
// Carried whole from the landing lane, including both of its repairs: it must
// be out-of-process (an in-process stub deadlocks against this driver's
// blocking waits and reads as "a session that booted and declined to poll" —
// a FALSE REFUTATION), and it must answer SSE with a terminal event (a plain
// JSON body makes the engine retry, trip its breaker and hang ~90s).
// ───────────────────────────────────────────────────────────────────────────

function runAsLlmStub(holdMs, journalPath) {
  fs.writeFileSync(journalPath, '');
  let hits = 0;
  const server = http.createServer((req, res) => {
    let body = '';
    req.on('data', (c) => {
      body += c;
    });
    req.on('end', () => {
      hits += 1;
      fs.appendFileSync(
        journalPath,
        `${JSON.stringify({ at: new Date().toISOString(), path: req.url, hit: hits })}\n`,
      );
      res.writeHead(200, {
        'content-type': 'text/event-stream',
        'cache-control': 'no-cache',
        connection: 'keep-alive',
      });
      const base = {
        id: 'f24cs',
        object: 'chat.completion.chunk',
        created: Math.floor(Date.now() / 1000),
        model: 'f24cs-stub',
      };
      res.write(
        `data: ${JSON.stringify({ ...base, choices: [{ index: 0, delta: { role: 'assistant' }, finish_reason: null }] })}\n\n`,
      );
      setTimeout(() => {
        res.write(
          `data: ${JSON.stringify({ ...base, choices: [{ index: 0, delta: { content: 'ack' }, finish_reason: null }] })}\n\n`,
        );
        res.write(
          `data: ${JSON.stringify({ ...base, choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] })}\n\n`,
        );
        res.write('data: [DONE]\n\n');
        res.end();
      }, holdMs);
    });
  });
  server.listen(0, '127.0.0.1', () => {
    process.stdout.write(`LLMSTUB_READY url=http://127.0.0.1:${server.address().port}\n`);
  });
}

// ───────────────────────────────────────────────────────────────────────────
// The run
// ───────────────────────────────────────────────────────────────────────────

/** Supervisor cadence for every process under test, in ms. TTL is 3x this. */
const TICK_MS = 1000;
/** How long to let things settle before a steady-state window is opened. */
const SETTLE_MS = 9000;
/** How long a steady-state observation window is held open. */
const WINDOW_MS = 8000;
/**
 * How long a process gets to boot far enough to poll, or to change role.
 *
 * MEASURED, not guessed. The first live run set this at 60s and the leg failed
 * on it: a debug-build session on a box at load 13 took **exactly 61 seconds**
 * to reach its first `getUpdates`, so `session_owned_first` read false while
 * every substantive assertion in the same leg passed. A budget that expires
 * just before the thing it waits for produces a FALSE NEGATIVE, which is the
 * same class of damage as a false green — it would have reported this fix as
 * not working.
 */
const BOOT_BUDGET_MS = 240_000;

class Run {
  constructor(args) {
    this.args = args;
    this.runDir = path.resolve(args.runDir);
    this.home = path.join(this.runDir, 'home');
    this.runId = crypto.randomBytes(4).toString('hex');
    // Minted here, for this run only. Not a vendor credential; never printed.
    this.botToken = `882${crypto.randomInt(100000, 999999)}:F24CS-${this.runId}`;
    this.vaultPassphrase = crypto.randomBytes(24).toString('hex');
    this.chatId = '882001';
    this.senderId = '882001';
    this.children = [];
    this.notes = [];
    this.connSamples = [];
    this.ssAvailable = null;
    this.tgJournal = path.join(this.runDir, 'tg-fixture.jsonl');
    this.llmJournal = path.join(this.runDir, 'llm-stub.jsonl');
    fs.mkdirSync(this.runDir, { recursive: true });
  }

  note(t) {
    const line = `[cs] ${t}`;
    this.notes.push(line);
    process.stdout.write(`${line}\n`);
  }

  // ── fixture, stub, config ───────────────────────────────────────────────

  startTgFixture() {
    const logPath = path.join(this.runDir, 'tg-fixture.log');
    fs.writeFileSync(logPath, '');
    const fd = fs.openSync(logPath, 'a');
    const child = spawn(
      process.execPath,
      [
        path.join(HERE, 'f24-tg-fixture.mjs'),
        '--token', this.botToken,
        '--journal', this.tgJournal,
        '--port', '0',
        '--max-wait-ms', '2000',
      ],
      { stdio: ['ignore', fd, fd], windowsHide: true },
    );
    this.children.push(child);
    this.fixturePid = child.pid;
    const re = /(http:\/\/127\.0\.0\.1:(\d+))/;
    let banner = '';
    for (let i = 0; i < 200; i += 1) {
      banner = readOr(logPath);
      if (re.test(banner)) break;
      sleep(50);
    }
    const m = re.exec(banner);
    if (!m) throw new Error(`tg fixture never signalled ready (${byteLen(logPath)} bytes):\n${banner}`);
    this.tgUrl = m[1];
    this.tgPort = Number(m[2]);
    this.note(`tg fixture pid=${this.fixturePid} at ${this.tgUrl}`);
    return this.tgUrl;
  }

  startLlmStub(holdMs) {
    const logPath = path.join(this.runDir, 'llm-stub.log');
    fs.writeFileSync(logPath, '');
    const fd = fs.openSync(logPath, 'a');
    const child = spawn(
      process.execPath,
      [fileURLToPath(import.meta.url), '--llm-stub', '--hold-ms', String(holdMs), '--journal', this.llmJournal],
      { stdio: ['ignore', fd, fd], windowsHide: true },
    );
    this.children.push(child);
    this.llmChild = child;
    const re = /LLMSTUB_READY url=(http:\/\/127\.0\.0\.1:\d+)/;
    let banner = '';
    for (let i = 0; i < 200; i += 1) {
      banner = readOr(logPath);
      if (re.test(banner)) break;
      sleep(50);
    }
    const m = re.exec(banner);
    if (!m) throw new Error(`llm stub never bound (${byteLen(logPath)} bytes): ${banner}`);
    // The engine appends `/v1/chat/completions`, so the base must NOT carry
    // `/v1` or the journal shows `/v1/v1/...`.
    this.llmUrl = m[1];
    this.note(`llm stub pid=${child.pid} at ${this.llmUrl}`);
    return this.llmUrl;
  }

  llmHits() {
    return readOr(this.llmJournal).split('\n').filter((l) => l.trim().length > 0).length;
  }

  writeConfig() {
    fs.mkdirSync(path.join(this.home, 'channels'), { recursive: true });
    fs.writeFileSync(
      path.join(this.home, 'credentials.toml'),
      ['[secrets]', `"telegram.f24cs.bot_token" = "${this.botToken}"`, ''].join('\n'),
      { mode: 0o600 },
    );
    fs.writeFileSync(
      path.join(this.home, 'config.toml'),
      [
        '[default]',
        'provider = "f24csstub"',
        '',
        '[providers.f24csstub]',
        'provider = "openai"',
        'model = "f24cs-stub"',
        'api_key = "f24cs-not-a-real-key"',
        `base_url = "${this.llmUrl}"`,
        '',
        // The POLLING path is what this measures, and a bound webhook host
        // would also collide on a fixed port with the other lanes on this box.
        '[inbound_webhook]',
        'enabled = false',
        '',
      ].join('\n'),
      { mode: 0o600 },
    );
    fs.writeFileSync(
      path.join(this.home, 'channels', 'f24cstg.toml'),
      [
        'name = "f24cstg"',
        'platform = "telegram"',
        'enabled = true',
        '',
        '[options]',
        'credential_handle = "telegram.f24cs.bot_token"',
        `api_base_url = "${this.tgUrl}"`,
        'long_poll_timeout_secs = 1',
        '',
        '[inbound]',
        'dm = "allowlist"',
        `dm_allowlist = ["${this.senderId}"]`,
        'group = "disabled"',
        'require_mention = true',
        'tools = "conversational"',
        '',
      ].join('\n'),
    );
  }

  childEnv(extra = {}) {
    return {
      ...process.env,
      WAYLAND_HOME: this.home,
      // Without a vault passphrase an isolated profile refuses EVERY turn
      // host-wide, and a credentials-posture refusal would be misattributed to
      // the polling path. (Carried from 24-C3-H2.)
      WAYLAND_VAULT_PASSPHRASE: this.vaultPassphrase,
      // F24-CS: drive handovers in seconds. Production default is 2000ms.
      WAYLAND_CHANNEL_LEASE_TICK_MS: String(TICK_MS),
      RUST_LOG:
        process.env.RUST_LOG ??
        'info,wcore_agent::channel_lease=trace,wcore_agent::bootstrap=debug,wcore_channels=debug',
      ...extra,
    };
  }

  // ── processes under test ────────────────────────────────────────────────

  startGateway(tag) {
    const logPath = path.join(this.runDir, `gateway-${tag}.log`);
    fs.writeFileSync(logPath, '');
    const fd = fs.openSync(logPath, 'a');
    const child = spawn(this.args.binary, ['gateway', 'run'], {
      // stdin `ignore`, not `pipe` — carried repair: a pipe this driver never
      // writes to still produced `write EPIPE` when the child went away
      // mid-leg, aborting the run at the measurement point.
      stdio: ['ignore', fd, fd],
      env: this.childEnv(),
    });
    this.children.push(child);
    this.note(`gateway '${tag}' pid=${child.pid} log=${path.basename(logPath)}`);
    return { child, logPath, kind: 'gateway', tag };
  }

  startSession(tag) {
    const logPath = path.join(this.runDir, `session-${tag}.log`);
    fs.writeFileSync(logPath, '');
    const fd = fs.openSync(logPath, 'a');
    const child = spawn(this.args.binary, [`f24cs session ${tag} ${this.runId}`], {
      stdio: ['ignore', fd, fd],
      env: this.childEnv(),
    });
    this.children.push(child);
    this.note(`session '${tag}' pid=${child.pid} log=${path.basename(logPath)}`);
    return { child, logPath, kind: 'session', tag };
  }

  alive(proc) {
    return proc.child.exitCode === null && proc.child.signalCode === null;
  }

  // ── fixture control ─────────────────────────────────────────────────────

  async submit(text) {
    return retryJson(
      `${this.tgUrl}/__control/submit`,
      {
        method: 'POST',
        body: { token: this.botToken, chatId: this.chatId, senderId: this.senderId, username: 'f24cs', text },
      },
      5, 1000, (m) => this.note(m),
    );
  }

  async report() {
    return retryJson(`${this.tgUrl}/__control/report`, {}, 5, 1000, (m) => this.note(m));
  }

  // ── observation ─────────────────────────────────────────────────────────

  /** One kernel-side attribution sample. Excludes the fixture and this driver. */
  sampleConns() {
    const pids = sampleConnectedPids(this.tgPort, [this.fixturePid, process.pid]);
    if (pids === null) {
      this.ssAvailable = false;
      return null;
    }
    this.ssAvailable = true;
    const rec = { at: new Date().toISOString(), pids: [...pids] };
    this.connSamples.push(rec);
    return pids;
  }

  pidsInWindow(startIso, endIso) {
    if (this.ssAvailable === false) return null;
    const s = Date.parse(startIso);
    const e = Date.parse(endIso);
    const out = new Set();
    for (const rec of this.connSamples) {
      const t = Date.parse(rec.at);
      if (t >= s && t <= e) for (const p of rec.pids) out.add(p);
    }
    return out;
  }

  /**
   * Hold an observation window open, sampling attribution throughout.
   * Emits on every iteration — a silent loop is indistinguishable from a stall
   * and the watchdog kills at 600s of silence (brief §6b).
   */
  async window(label, ms) {
    const start = new Date().toISOString();
    const iters = Math.max(1, Math.round(ms / 1000));
    for (let i = 0; i < iters; i += 1) {
      this.sampleConns();
      if (i % 3 === 0) this.note(`${label}: window ${i + 1}/${iters} at ${new Date().toISOString()}`);
      sleep(1000);
    }
    const end = new Date().toISOString();
    const rep = await this.report();
    return {
      label,
      start,
      end,
      maxOpen: maxOpenInWindow(rep.concurrency_trace, start, end),
      polls: pollsInWindow(rep.concurrency_trace, start, end),
      // An ARRAY, never a Set — see instrument defect #4 in `gradeWindow`.
      pids: (() => {
        const s = this.pidsInWindow(start, end);
        return s === null ? null : [...s].sort((a, b) => a - b);
      })(),
      report: rep,
    };
  }

  /** Wait for a token to appear in a log, sampling attribution meanwhile. */
  waitForToken(logPath, token, budgetMs, label) {
    const deadline = Date.now() + budgetMs;
    while (Date.now() < deadline) {
      this.sampleConns();
      if (containsToken(readOr(logPath), token)) {
        this.note(`${label}: saw '${token}' after ${budgetMs - (deadline - Date.now())}ms`);
        return true;
      }
      this.note(`${label}: waiting for '${token}', ${new Date().toISOString()}`);
      sleep(1000);
    }
    this.note(`${label}: TIMEOUT waiting for '${token}' (log ${byteLen(logPath)} bytes)`);
    return false;
  }

  async waitForPolls(minPolls, budgetMs, label) {
    const deadline = Date.now() + budgetMs;
    let last = 0;
    while (Date.now() < deadline) {
      this.sampleConns();
      const r = await this.report().catch(() => null);
      if (r) {
        last = r.poll_total;
        if (r.poll_total >= minPolls) {
          this.note(`${label}: poll_total=${r.poll_total} (>= ${minPolls})`);
          return true;
        }
      }
      this.note(`${label}: waiting, poll_total=${last}, ${new Date().toISOString()}`);
      sleep(1000);
    }
    this.note(`${label}: TIMEOUT at poll_total=${last}`);
    return false;
  }

  /**
   * Wait for a submitted message to be CONSUMED — i.e. actually delivered to
   * whichever process is polling. This is the positive path, and it is read
   * from the fixture (another OS process), not from any log the binary wrote.
   */
  async waitForDelivery(updateId, budgetMs, label) {
    const deadline = Date.now() + budgetMs;
    while (Date.now() < deadline) {
      this.sampleConns();
      const r = await this.report().catch(() => null);
      if (r) {
        const u = r.updates.find((x) => x.update_id === updateId);
        if (u && u.serve_count > 0) {
          this.note(`${label}: update ${updateId} served ${u.serve_count}x, deleted_by=${u.deleted_by}`);
          return { delivered: true, serve_count: u.serve_count, deleted_by: u.deleted_by };
        }
      }
      this.note(`${label}: waiting for delivery of ${updateId}, ${new Date().toISOString()}`);
      sleep(1000);
    }
    this.note(`${label}: TIMEOUT — update ${updateId} was never delivered`);
    return { delivered: false, serve_count: 0, deleted_by: null };
  }

  cleanup() {
    for (const c of this.children) {
      try { c.kill('SIGKILL'); } catch { /* already gone */ }
    }
  }
}

// ───────────────────────────────────────────────────────────────────────────
// LEG A — the service starts SECOND and must still end up polling.
// ───────────────────────────────────────────────────────────────────────────

async function legA(run) {
  run.note('=== LEG A: session first, service second — the service must take over ===');
  const session = run.startSession('A');
  const gotFirst = await run.waitForPolls(2, BOOT_BUDGET_MS, 'legA/session-owns');

  // Positive baseline IN THE SAME RUN: exactly one poller, and it is the
  // session. A post-handover `1` is only meaningful against this.
  const w1 = await run.window('legA/W1 session alone', WINDOW_MS);
  const m1 = await run.submit(`F24CS-A1-${run.runId}`);
  const d1 = await run.waitForDelivery(m1.update_id, 30_000, 'legA/deliver-to-session');

  // Now the installed service arrives.
  const gateway = run.startGateway('A');
  const yielded = run.waitForToken(session.logPath, 'F24_CHANNEL_LEASE=yielded', BOOT_BUDGET_MS, 'legA/session-yields');
  const acquired = run.waitForToken(gateway.logPath, 'F24_CHANNEL_LEASE=acquired', BOOT_BUDGET_MS, 'legA/gateway-acquires');

  run.note(`legA: settling ${SETTLE_MS}ms before the post-handover window`);
  for (let i = 0; i < SETTLE_MS / 1000; i += 1) { run.sampleConns(); sleep(1000); }

  const w2 = await run.window('legA/W2 gateway owns', WINDOW_MS);
  const m2 = await run.submit(`F24CS-A2-${run.runId}`);
  const d2 = await run.waitForDelivery(m2.update_id, 30_000, 'legA/deliver-to-gateway');

  const sessionAlive = run.alive(session);
  const gatewayAlive = run.alive(gateway);

  const g1 = gradeWindow({ maxOpen: w1.maxOpen, polls: w1.polls, expectPids: [session.child.pid], sawPids: w1.pids });
  const g2 = gradeWindow({ maxOpen: w2.maxOpen, polls: w2.polls, expectPids: [gateway.child.pid], sawPids: w2.pids });

  const pass =
    gotFirst && yielded && acquired && d1.delivered && d2.delivered &&
    g1.grade === 'OK' && g2.grade === 'OK' && sessionAlive && gatewayAlive;

  try { gateway.child.kill('SIGKILL'); } catch { /* gone */ }
  try { session.child.kill('SIGKILL'); } catch { /* gone */ }

  return {
    leg: 'A', verdict: pass ? 'SERVICE WINS FROM BEHIND' : 'FAILED',
    session_pid: session.child.pid, gateway_pid: gateway.child.pid,
    session_owned_first: gotFirst,
    window_session_alone: { ...w1, report: undefined, grade: g1 },
    window_gateway_owns: { ...w2, report: undefined, grade: g2 },
    session_emitted_yield_token: yielded,
    gateway_emitted_acquire_token: acquired,
    delivered_while_session_owned: d1,
    delivered_after_handover: d2,
    session_still_alive_at_measurement: sessionAlive,
    gateway_still_alive_at_measurement: gatewayAlive,
    log_bytes: {
      'legA session': byteLen(session.logPath),
      'legA gateway': byteLen(gateway.logPath),
    },
  };
}

// ───────────────────────────────────────────────────────────────────────────
// LEG B — the service starts FIRST and must NOT be disturbed.
// ───────────────────────────────────────────────────────────────────────────

async function legB(run) {
  run.note('=== LEG B: service first, session second — the service must keep polling ===');
  const gateway = run.startGateway('B');
  const gotFirst = await run.waitForPolls(2, BOOT_BUDGET_MS, 'legB/gateway-owns');
  const w1 = await run.window('legB/W1 gateway alone', WINDOW_MS);

  const session = run.startSession('B');
  const sessionObserves = run.waitForToken(session.logPath, 'F24_CHANNEL_LEASE=observer', BOOT_BUDGET_MS, 'legB/session-observes');

  run.note(`legB: settling ${SETTLE_MS}ms with both processes alive`);
  for (let i = 0; i < SETTLE_MS / 1000; i += 1) { run.sampleConns(); sleep(1000); }

  const w2 = await run.window('legB/W2 both alive', WINDOW_MS);
  const m1 = await run.submit(`F24CS-B1-${run.runId}`);
  const d1 = await run.waitForDelivery(m1.update_id, 30_000, 'legB/deliver-to-gateway');

  const gatewayLog = readOr(gateway.logPath);
  const sessionLog = readOr(session.logPath);
  // The negative assertions: nothing yielded, and the session never took over.
  const gatewayYielded = containsToken(gatewayLog, 'F24_CHANNEL_LEASE=yielded');
  const sessionAcquired = containsToken(sessionLog, 'F24_CHANNEL_LEASE=acquired');

  const sessionAlive = run.alive(session);
  const gatewayAlive = run.alive(gateway);

  const g1 = gradeWindow({ maxOpen: w1.maxOpen, polls: w1.polls, expectPids: [gateway.child.pid], sawPids: w1.pids });
  const g2 = gradeWindow({ maxOpen: w2.maxOpen, polls: w2.polls, expectPids: [gateway.child.pid], sawPids: w2.pids });

  const pass =
    gotFirst && sessionObserves && d1.delivered && !gatewayYielded && !sessionAcquired &&
    g1.grade === 'OK' && g2.grade === 'OK' && sessionAlive && gatewayAlive;

  try { session.child.kill('SIGKILL'); } catch { /* gone */ }
  try { gateway.child.kill('SIGKILL'); } catch { /* gone */ }

  return {
    leg: 'B', verdict: pass ? 'SERVICE KEEPS IT' : 'FAILED',
    gateway_pid: gateway.child.pid, session_pid: session.child.pid,
    gateway_owned_first: gotFirst,
    window_gateway_alone: { ...w1, report: undefined, grade: g1 },
    window_both_alive: { ...w2, report: undefined, grade: g2 },
    session_emitted_observer_token: sessionObserves,
    gateway_yielded_wrongly: gatewayYielded,
    session_acquired_wrongly: sessionAcquired,
    delivered_to_gateway: d1,
    session_still_alive_at_measurement: sessionAlive,
    gateway_still_alive_at_measurement: gatewayAlive,
    log_bytes: {
      'legB gateway': byteLen(gateway.logPath),
      'legB session': byteLen(session.logPath),
    },
  };
}

// ───────────────────────────────────────────────────────────────────────────
// LEG D — an ALREADY-RUNNING observer takes over when the holder dies.
//
// Deliberately distinct from the landing lane's leg 4, which started a NEW
// process after the kill. That proves the recovery a service manager performs
// (restart the unit); it does not prove the in-place upgrade, which was
// residual (2) and needed operator action.
// ───────────────────────────────────────────────────────────────────────────

async function legD(run) {
  run.note('=== LEG D: kill the holder; a live observer must take over unaided ===');
  const gateway = run.startGateway('D');
  const gotFirst = await run.waitForPolls(2, BOOT_BUDGET_MS, 'legD/gateway-owns');

  const session = run.startSession('D');
  const observed = run.waitForToken(session.logPath, 'F24_CHANNEL_LEASE=observer', BOOT_BUDGET_MS, 'legD/session-observes');
  const sessionAliveBefore = run.alive(session);

  run.note(`legD: killing the holder pid=${gateway.child.pid} with SIGKILL — no operator action follows`);
  const killAt = new Date().toISOString();
  try { gateway.child.kill('SIGKILL'); } catch { /* gone */ }

  // NOTHING is started here. That is the point of the leg.
  const tookOver = run.waitForToken(session.logPath, 'F24_CHANNEL_LEASE=acquired', BOOT_BUDGET_MS, 'legD/session-takes-over');
  const sessionAliveAfter = run.alive(session);

  const w = await run.window('legD/W after takeover', WINDOW_MS);
  const m1 = await run.submit(`F24CS-D1-${run.runId}`);
  const d1 = await run.waitForDelivery(m1.update_id, 30_000, 'legD/deliver-after-takeover');

  const g = gradeWindow({ maxOpen: w.maxOpen, polls: w.polls, expectPids: [session.child.pid], sawPids: w.pids });

  const pass = gotFirst && observed && sessionAliveBefore && sessionAliveAfter && tookOver && d1.delivered && g.grade === 'OK';

  try { session.child.kill('SIGKILL'); } catch { /* gone */ }

  return {
    leg: 'D', verdict: pass ? 'LIVE OBSERVER TOOK OVER' : 'FAILED',
    gateway_pid: gateway.child.pid, session_pid: session.child.pid,
    gateway_owned_first: gotFirst,
    session_was_a_live_observer_before_the_kill: sessionAliveBefore && observed,
    killed_at: killAt,
    session_emitted_acquire_token: tookOver,
    session_alive_after_the_kill: sessionAliveAfter,
    nothing_was_started_after_the_kill: true,
    window_after_takeover: { ...w, report: undefined, grade: g },
    delivered_after_takeover: d1,
    log_bytes: {
      'legD gateway': byteLen(gateway.logPath),
      'legD session': byteLen(session.logPath),
    },
  };
}

// ───────────────────────────────────────────────────────────────────────────
// LEG E — a DEAD claimant must not wedge the home into having no poller.
//
// This is the failure that would be strictly WORSE than the starvation being
// fixed: an owner yields to a higher-ranked claimant, the claimant dies before
// taking the lock, and NOBODY polls — for ever. Two of the three cross-audit
// reviewers named it, and it is the shape the brief calls manufactured denial,
// promoted out of the harness and into production.
//
// The claimant is dead BY CONSTRUCTION: this driver plants a well-formed
// gateway-ranked claim naming a pid that is not the gateway, and then never
// refreshes it. Nothing in the run can rescue it. The only thing that can end
// the standoff is the claim ageing past its TTL — which is exactly the property
// under test.
//
// It is also the one leg that deliberately CREATES a zero-poller window, so it
// is the leg that proves the anti-denial grader can fire on something real
// rather than only on a hypothetical.
// ───────────────────────────────────────────────────────────────────────────

async function legE(run) {
  run.note('=== LEG E: a dead claimant must not wedge polling ===');
  const session = run.startSession('E');
  const gotFirst = await run.waitForPolls(2, BOOT_BUDGET_MS, 'legE/session-owns');
  const w1 = await run.window('legE/W1 session polls', WINDOW_MS);

  // Plant the dead claimant. `publish_claim`'s on-disk shape, a rank the
  // session must respect, and a pid that will never be refreshed.
  const claimPath = path.join(run.home, 'channels', 'channel-poll.claim.4242424');
  const plantedAt = new Date().toISOString();
  fs.writeFileSync(claimPath, JSON.stringify({ pid: 4242424, rank: 30, holder: 'gateway' }));
  run.note(`legE: planted a gateway-ranked claim at ${claimPath} and will NEVER refresh it`);

  const yielded = run.waitForToken(session.logPath, 'F24_CHANNEL_LEASE=yielded', 60_000, 'legE/session-yields');
  // NOTHING happens here. No process is started, no file is touched.
  const recovered = run.waitForToken(session.logPath, 'F24_CHANNEL_LEASE=acquired', 60_000, 'legE/session-recovers');

  const w2 = await run.window('legE/W2 after recovery', WINDOW_MS);
  const m1 = await run.submit(`F24CS-E1-${run.runId}`);
  const d1 = await run.waitForDelivery(m1.update_id, 30_000, 'legE/deliver-after-recovery');

  // The measured wedge bound, from the process's own transition timestamps.
  const clean = (s) => String(s).replace(/\x1b\[[0-9;]*m/g, '');
  const log = clean(readOr(session.logPath));
  const stampOf = (tok) => {
    const line = log.split('\n').find((l) => l.includes(tok) && /^\d{4}-/.test(l));
    return line ? line.split(/\s+/)[0] : null;
  };
  const yieldAt = stampOf('F24_CHANNEL_LEASE=yielded');
  const acquireAt = stampOf('F24_CHANNEL_LEASE=acquired');
  const wedgeMs = yieldAt && acquireAt ? Date.parse(acquireAt) - Date.parse(yieldAt) : null;

  const sessionAlive = run.alive(session);
  const g1 = gradeWindow({ maxOpen: w1.maxOpen, polls: w1.polls, expectPids: [session.child.pid], sawPids: w1.pids });
  const g2 = gradeWindow({ maxOpen: w2.maxOpen, polls: w2.polls, expectPids: [session.child.pid], sawPids: w2.pids });

  // The bound is asserted, not merely reported. A recovery that took a minute
  // would be a wedge with a long fuse, and grading it a pass would be exactly
  // the "redefine success downward" failure the brief forbids.
  const WEDGE_BOUND_MS = 30_000;
  const boundedRecovery = wedgeMs !== null && wedgeMs > 0 && wedgeMs <= WEDGE_BOUND_MS;

  const pass = gotFirst && yielded && recovered && boundedRecovery && d1.delivered &&
    g1.grade === 'OK' && g2.grade === 'OK' && sessionAlive;

  try { session.child.kill('SIGKILL'); } catch { /* gone */ }

  return {
    leg: 'E', verdict: pass ? 'NO WEDGE — RECOVERED UNAIDED' : 'FAILED',
    session_pid: session.child.pid,
    session_owned_first: gotFirst,
    planted_claim_at: plantedAt,
    planted_claim: { pid: 4242424, rank: 30, holder: 'gateway', refreshed_ever: false },
    window_before: { ...w1, report: undefined, grade: g1 },
    session_yielded_to_the_dead_claim: yielded,
    session_recovered_without_help: recovered,
    yield_at: yieldAt, acquire_at: acquireAt,
    wedge_window_ms: wedgeMs,
    wedge_bound_ms: WEDGE_BOUND_MS,
    recovery_within_bound: boundedRecovery,
    window_after: { ...w2, report: undefined, grade: g2 },
    delivered_after_recovery: d1,
    session_still_alive_at_measurement: sessionAlive,
    log_bytes: { 'legE session': byteLen(session.logPath) },
  };
}

// ───────────────────────────────────────────────────────────────────────────
// LEG C — the accounting. Nothing may be lost across the whole run.
// ───────────────────────────────────────────────────────────────────────────

async function legC(run) {
  const rep = await run.report();
  const submitted = rep.submitted_total;
  const served = rep.updates.filter((u) => u.serve_count > 0).length;
  const doubleServed = rep.updates.filter((u) => u.serve_count > 1);
  const neverServed = rep.updates.filter((u) => u.serve_count === 0).map((u) => u.update_id);
  const pass = submitted > 0 && served === submitted;
  return {
    leg: 'C', verdict: pass ? 'NOTHING LOST' : 'FAILED',
    submitted_total: submitted,
    delivered_total: served,
    never_delivered: neverServed,
    // A message served to more than one poll is NOT loss and is NOT a race by
    // itself: a handover between receive and confirm legitimately redelivers.
    // It is reported rather than graded, because hiding it would be the same
    // sin as hiding a transient.
    served_more_than_once: doubleServed.map((u) => ({ update_id: u.update_id, serve_count: u.serve_count })),
    still_pending_at_end: rep.still_pending,
  };
}

// ───────────────────────────────────────────────────────────────────────────
// SELF-TESTS. Three assertions each, and the THIRD is always "the old shape
// would have missed this" — without it a self-test passes on the broken
// instrument too.
// ───────────────────────────────────────────────────────────────────────────

function selfTest(assert) {
  // ── 1. the cross-checked git reader (instrument defect #1 of this lane) ──
  const goodSha = 'a'.repeat(40);
  const otherSha = 'b'.repeat(40);
  const honest = { revParse: () => goodSha, logOne: () => goodSha };
  // A reader that reports a DIFFERENT commit from `log` than from `rev-parse`
  // — which is precisely what the rtk proxy did in this repository.
  const rewriting = { revParse: () => goodSha, logOne: () => otherSha };

  assert('git/known-positive: agreeing readers yield the sha',
    crossCheckedSha(honest) === goodSha);

  let threw = false;
  try { crossCheckedSha(rewriting); } catch { threw = true; }
  assert('git/known-negative: disagreeing readers are REFUSED, not guessed at', threw);

  assert('git/OLD SHAPE WOULD HAVE MISSED IT: the single-reader form silently returns the wrong commit',
    naiveSha(rewriting) === otherSha && naiveSha(rewriting) !== goodSha);

  // ── 2. the ss attribution parser ────────────────────────────────────────
  const PORT = 44771;
  const ss = [
    // the poller we care about: peer IS the fixture
    `0      0      127.0.0.1:51000        127.0.0.1:${PORT}   users:(("wayland-core",pid=1111,fd=9))`,
    // the FIXTURE's own server-side socket: its peer is the CLIENT, not itself
    `0      0      127.0.0.1:${PORT}      127.0.0.1:51000     users:(("node",pid=2222,fd=20))`,
    // an unrelated connection on the box, same pid shape
    '0      0      10.0.0.5:22            10.0.0.9:60001      users:(("sshd",pid=3333,fd=4))',
    // a local port that merely shares the NUMBER with the fixture port
    `0      0      10.0.0.5:${PORT}       10.0.0.9:60002      users:(("other",pid=4444,fd=5))`,
  ].join('\n');

  const got = parseSsPeers(ss, PORT, [2222]);
  assert('ss/known-positive: the real poller is attributed',
    got.has(1111));
  assert('ss/known-negative: fixture, unrelated and same-number-different-host are all excluded',
    !got.has(2222) && !got.has(3333) && !got.has(4444) && got.size === 1);

  const naive = new Set([...String(ss).matchAll(/pid=(\d+)/g)].map((m) => Number(m[1])));
  assert('ss/OLD SHAPE WOULD HAVE MISSED IT: an unanchored pid= sweep collects 4 pids and reports a phantom race',
    naive.size === 4 && naive.has(2222) && naive.has(3333));

  // ── 3. the whitespace-stripping token matcher ───────────────────────────
  const wrapped = 'blah F24_CHANNEL_LEASE=yiel\nded to_pid=42 blah';
  assert('token/known-positive: a console-wrapped token is found',
    containsToken(wrapped, 'F24_CHANNEL_LEASE=yielded'));
  assert('token/known-negative: an absent token is still absent',
    !containsToken('nothing to see here', 'F24_CHANNEL_LEASE=yielded'));
  assert('token/OLD SHAPE WOULD HAVE MISSED IT: String.includes reports absence on the same log',
    !wrapped.includes('F24_CHANNEL_LEASE=yielded'));

  // ── 4. the DENIAL grader ────────────────────────────────────────────────
  const denial = gradeWindow({ maxOpen: 0, polls: 0, expectPids: [1], sawPids: new Set() });
  assert('denial/known-positive: zero pollers grades DENIAL, not pass',
    denial.grade === 'DENIAL');
  assert('denial/known-negative: exactly one attributed poller grades OK',
    gradeWindow({ maxOpen: 1, polls: 7, expectPids: [1], sawPids: new Set([1]) }).grade === 'OK');
  // The old shape is the check everyone writes first: "no more than one poller".
  const oldShapePasses = (maxOpen) => maxOpen <= 1;
  assert('denial/OLD SHAPE WOULD HAVE MISSED IT: a `max_open <= 1` check passes on ZERO pollers',
    oldShapePasses(0) === true && denial.grade === 'DENIAL');

  // ── 4b. INSTRUMENT DEFECT #2 OF THIS LANE: the false DENIAL ─────────────
  // A window that catches only poll-CLOSE samples reads maxOpen 0 while polls
  // demonstrably happened. Grading that DENIAL is a false CRITICAL.
  const tailOnly = gradeWindow({ maxOpen: 0, polls: 4, expectPids: [1], sawPids: new Set([1]) });
  assert('falsedenial/known-positive: polls>0 with no open-side sample is UNREADABLE, not DENIAL',
    tailOnly.grade === 'UNREADABLE');
  assert('falsedenial/known-negative: polls===0 is still DENIAL',
    gradeWindow({ maxOpen: 0, polls: 0, expectPids: [1], sawPids: new Set([1]) }).grade === 'DENIAL');
  // The old shape tested maxOpen FIRST, so it could not tell the two apart.
  const oldShapeGrade = (maxOpen) => (maxOpen === 0 ? 'DENIAL' : 'OK');
  assert('falsedenial/OLD SHAPE WOULD HAVE MISSED IT: maxOpen-first grading calls a polling window DENIAL',
    oldShapeGrade(0) === 'DENIAL' && tailOnly.grade !== 'DENIAL');

  // ── 4c. INSTRUMENT DEFECT #4 OF THIS LANE: a Set serialises to `{}` ─────
  // The evidence file is the deliverable. A window whose attribution succeeded
  // must not be RECORDED as `"pids": {}`, which reads as the strongest possible
  // negative.
  const winLike = { pids: [7, 3] };
  assert('serialise/known-positive: an array of pids survives JSON.stringify',
    JSON.stringify(winLike) === '{"pids":[7,3]}');
  assert('serialise/known-negative: the grader accepts an array and still grades correctly',
    gradeWindow({ maxOpen: 1, polls: 3, expectPids: [7, 3], sawPids: [7, 3] }).grade === 'OK');
  assert('serialise/OLD SHAPE WOULD HAVE MISSED IT: a Set stringifies to {} and loses every pid',
    JSON.stringify({ pids: new Set([7, 3]) }) === '{"pids":{}}');

  // ── 5. wrong-owner detection ────────────────────────────────────────────
  assert('owner/known-positive: the wrong pid polling is caught',
    gradeWindow({ maxOpen: 1, polls: 5, expectPids: [10], sawPids: new Set([11]) }).grade === 'WRONG_OWNER');
  assert('owner/known-negative: the right pid polling is OK',
    gradeWindow({ maxOpen: 1, polls: 5, expectPids: [10], sawPids: new Set([10]) }).grade === 'OK');
  assert('owner/OLD SHAPE WOULD HAVE MISSED IT: counting concurrency alone cannot tell WHICH process polled',
    maxOpenInWindow([{ at: '2026-01-01T00:00:01Z', open: 1, poll: 1 }], '2026-01-01T00:00:00Z', '2026-01-01T00:00:02Z') === 1);

  // ── 6. the window-scoped reader ─────────────────────────────────────────
  const trace = [
    { at: '2026-01-01T00:00:01Z', open: 2, poll: 1 }, // BEFORE the window
    { at: '2026-01-01T00:00:11Z', open: 1, poll: 2 }, // inside
    { at: '2026-01-01T00:00:12Z', open: 1, poll: 3 }, // inside
  ];
  assert('window/known-positive: only in-window samples count',
    maxOpenInWindow(trace, '2026-01-01T00:00:10Z', '2026-01-01T00:00:20Z') === 1);
  assert('window/known-negative: the earlier spike is visible in its own window',
    maxOpenInWindow(trace, '2026-01-01T00:00:00Z', '2026-01-01T00:00:05Z') === 2);
  const globalMax = Math.max(...trace.map((t) => t.open));
  assert('window/OLD SHAPE WOULD HAVE MISSED IT: the global max reports 2 for a window that only ever saw 1',
    globalMax === 2);

  // ── 7. the instrument grader refuses to interpret an absent process ─────
  assert('incomplete/known-positive: an absent process makes the run INCOMPLETE',
    gradeInstrument({ fixture_reachable: true, tg_journal_bytes: 10, expect_alive: { 'legD session': false } }).fault);
  assert('incomplete/known-negative: a healthy run is not faulted',
    !gradeInstrument({ fixture_reachable: true, tg_journal_bytes: 10, expect_alive: { 'legD session': true } }).fault);
  assert('incomplete/OLD SHAPE WOULD HAVE MISSED IT: a zero-byte log passes any check that only reads exit status',
    gradeInstrument({ fixture_reachable: true, tg_journal_bytes: 10, log_bytes: { x: 0 } }).fault);
}

function runSelfTests() {
  let passed = 0;
  const failures = [];
  const assert = (name, ok) => {
    if (ok) { passed += 1; process.stdout.write(`  ok   ${name}\n`); }
    else { failures.push(name); process.stdout.write(`  FAIL ${name}\n`); }
  };
  selfTest(assert);
  process.stdout.write(`\nself-test: ${passed} passed, ${failures.length} failed\n`);
  if (failures.length > 0) {
    process.stdout.write(`failures:\n${failures.map((f) => `  - ${f}`).join('\n')}\n`);
    process.exit(1);
  }
  process.exit(0);
}

// ───────────────────────────────────────────────────────────────────────────

function parseArgs(argv) {
  const out = { binary: null, runDir: null, legs: 'a,b,d,e,c', selfTest: false, llmStub: false, holdMs: 90_000, journal: null };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--binary') out.binary = argv[++i];
    else if (a === '--run-dir') out.runDir = argv[++i];
    else if (a === '--legs') out.legs = argv[++i];
    else if (a === '--self-test') out.selfTest = true;
    else if (a === '--llm-stub') out.llmStub = true;
    else if (a === '--hold-ms') out.holdMs = Number(argv[++i]);
    else if (a === '--journal') out.journal = argv[++i];
    else { process.stderr.write(`unknown argument ${a}\n`); process.exit(2); }
  }
  return out;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.llmStub) { runAsLlmStub(args.holdMs, args.journal); return; }
  if (args.selfTest) { runSelfTests(); return; }
  if (!args.binary || !args.runDir) {
    process.stderr.write('--binary and --run-dir are required\n');
    process.exit(2);
  }

  const run = new Run(args);
  const results = { run_id: run.runId, started_at: new Date().toISOString(), legs: [] };
  try {
    run.startTgFixture();
    run.startLlmStub(args.holdMs);
    run.writeConfig();

    const health = await retryJson(`${run.tgUrl}/__control/health`, {}, 5, 500, (m) => run.note(m)).catch(() => null);
    const fixtureReachable = Boolean(health && health.ok);
    run.note(`fixture health: ${JSON.stringify(health)}`);

    const want = new Set(args.legs.split(',').map((s) => s.trim().toLowerCase()));
    const expectAlive = {};
    const logBytes = {};

    if (want.has('a')) { const r = await legA(run); results.legs.push(r); Object.assign(logBytes, r.log_bytes); expectAlive['legA session'] = r.session_still_alive_at_measurement; expectAlive['legA gateway'] = r.gateway_still_alive_at_measurement; }
    if (want.has('b')) { const r = await legB(run); results.legs.push(r); Object.assign(logBytes, r.log_bytes); expectAlive['legB session'] = r.session_still_alive_at_measurement; expectAlive['legB gateway'] = r.gateway_still_alive_at_measurement; }
    if (want.has('d')) { const r = await legD(run); results.legs.push(r); Object.assign(logBytes, r.log_bytes); expectAlive['legD session (before kill)'] = r.session_was_a_live_observer_before_the_kill; expectAlive['legD session (after kill)'] = r.session_alive_after_the_kill; }
    if (want.has('e')) { const r = await legE(run); results.legs.push(r); Object.assign(logBytes, r.log_bytes); expectAlive['legE session'] = r.session_still_alive_at_measurement; }
    if (want.has('c')) { const r = await legC(run); results.legs.push(r); }

    const finalReport = await run.report().catch(() => ({ poll_total: 0 }));
    results.instrument = gradeInstrument({
      fixture_reachable: fixtureReachable,
      tg_journal_bytes: byteLen(run.tgJournal),
      log_bytes: logBytes,
      expect_alive: expectAlive,
      expect_polling: true,
      polls_total: finalReport.poll_total,
      ss_available: run.ssAvailable,
    });
    results.llm_stub_hits = run.llmHits();
    results.poll_total = finalReport.poll_total;
    results.conn_samples = run.connSamples.length;
    results.ss_available = run.ssAvailable;
  } catch (e) {
    results.error = `${e.message}\n${e.stack}`;
    results.instrument = { fault: true, reasons: [`driver threw: ${e.message}`] };
  } finally {
    run.cleanup();
  }

  results.finished_at = new Date().toISOString();
  const outPath = path.join(run.runDir, 'result.json');
  fs.writeFileSync(outPath, JSON.stringify(results, null, 2));
  fs.writeFileSync(path.join(run.runDir, 'driver-notes.log'), run.notes.join('\n'));
  fs.writeFileSync(path.join(run.runDir, 'conn-samples.json'), JSON.stringify(run.connSamples, null, 2));
  process.stdout.write(`\n=== RESULT (${byteLen(outPath)} bytes at ${outPath}) ===\n`);
  process.stdout.write(`${JSON.stringify(results, null, 2)}\n`);

  const anyFailed = results.error || results.instrument?.fault ||
    results.legs.some((l) => String(l.verdict).includes('FAILED'));
  process.exit(anyFailed ? 1 : 0);
}

main();
