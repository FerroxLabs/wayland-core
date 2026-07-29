#!/usr/bin/env node
/**
 * f24-c3-clauses-selftest.mjs — prove the clause driver's graders can FAIL
 * before trusting anything they say.
 *
 * LANE-BRIEF §3.2: "Before you trust any gate you write or run, ask whether it
 * could fail." §6b-ii: every instrument repair gets THREE assertions — a
 * known-positive passes, a known-negative fails, **and the old broken matcher
 * would have missed it**. That third assertion is the only one that proves the
 * repair does anything; without it the self-test passes on the broken instrument
 * too.
 *
 * Exit 0 = all passed, 1 = any failed.
 */

import { classify, matches, instrumentFault, legacyMatches } from './f24-correlate.mjs';
// F24-C3-H5: the SHIPPED matchers, imported rather than re-implemented here.
import { observedPostureIn, denialsIn } from './f24-gateway-log.mjs';
import fs from 'node:fs';
import path from 'node:path';

let passed = 0;
let failed = 0;

/**
 * Assertion helper.
 *
 * # Why any thenable hard-fails
 *
 * A sibling self-test in this phase (`f24-discord-selftest.mjs`) had ONE test
 * written `async`. An async assertion failure REJECTS rather than throws, so the
 * checker saw no exception and incremented `passed`. Measured on node v22: a
 * deliberately false assertion printed `ok`, printed `passed=1 failed=0`, and
 * exited 0 — a self-passing gate inside the file whose entire job is to prove
 * nothing else self-passes. Repaired STRUCTURALLY there and structurally here.
 */
function check(name, fn) {
  let result;
  try {
    result = fn();
  } catch (e) {
    failed += 1;
    process.stdout.write(`FAIL ${name}: threw ${e && e.message ? e.message : e}\n`);
    return;
  }
  if (result && typeof result.then === 'function') {
    failed += 1;
    process.stdout.write(
      `FAIL ${name}: returned a thenable. An async assertion rejects rather than throws, ` +
        `so this checker would score a false assertion as a pass. Make the test synchronous.\n`,
    );
    return;
  }
  passed += 1;
  process.stdout.write(`ok   ${name}\n`);
}

function assert(cond, msg) {
  if (!cond) throw new Error(msg);
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. The correlation matcher, three ways (§6b-ii)
// ─────────────────────────────────────────────────────────────────────────────

const TOKEN = 'f24c3fin-f24finone-a1b2c3d4';

check('known-positive: a raw token is an exact arrival and is countable', () => {
  const text = `F24C3-REPLY ${TOKEN} done`;
  assert(classify(text, TOKEN) === 'exact', `want exact, got ${classify(text, TOKEN)}`);
  assert(matches(text, TOKEN), 'must count as an arrival');
  assert(!instrumentFault(text, TOKEN), 'must not be a fault');
});

check('known-negative: an unrelated reply is absent and is NOT excused as a fault', () => {
  const text = 'F24C3-REPLY some other message entirely';
  assert(classify(text, TOKEN) === 'absent', `want absent, got ${classify(text, TOKEN)}`);
  assert(!matches(text, TOKEN), 'must not count as an arrival');
  // This is the load-bearing half of the negative. If a genuine absence were
  // graded as an instrument fault, every real LOSS would be laundered into
  // INCOMPLETE and the driver could never report a product defect.
  assert(!instrumentFault(text, TOKEN), 'a genuine absence must NOT be excused as INCOMPLETE');
});

check('THE THIRD ASSERTION: the old matcher misses what the repaired one catches', () => {
  // MarkdownV2 escaping, exactly as telegram emits it. This is the real shape
  // that made a lane report replied=0 against eight arrivals that had landed.
  const escaped = `F24C3\\-REPLY ${TOKEN.replace(/-/g, '\\-')}`;
  assert(
    legacyMatches(escaped, TOKEN) === false,
    'precondition: the OLD matcher must miss this, else the case proves nothing',
  );
  assert(
    matches(escaped, TOKEN),
    'the repaired matcher must catch it — otherwise the repair changes no outcome',
  );
  assert(classify(escaped, TOKEN) === 'normalized', `want normalized, got ${classify(escaped, TOKEN)}`);
});

check('a present-but-undecodable token grades FAULT, never absence', () => {
  // Percent-encoding inserts ALPHANUMERICS, so a skeleton-substring test alone
  // does not catch it — the bounded subsequence window is what does.
  const mangled = `F24C3-REPLY ${TOKEN.replace(/-/g, '%2D')}`;
  assert(!matches(mangled, TOKEN), 'must not be counted as a decoded arrival');
  assert(
    instrumentFault(mangled, TOKEN),
    'must be flagged as an instrument fault so the run grades INCOMPLETE rather than LOSS',
  );
});

// ─────────────────────────────────────────────────────────────────────────────
// 2. The leg grader — can it fail, and does the positive control bite?
// ─────────────────────────────────────────────────────────────────────────────
//
// Reimplemented here rather than imported, because the driver's `record` lives
// on a class that spawns processes. The LOGIC is what is under test and it is
// three lines; a divergence between these three lines and the driver's is
// caught by the source-scan below.

function grade(ok, positiveControl) {
  const controlOk = positiveControl === undefined ? null : positiveControl > 0;
  return ok && (controlOk === null || controlOk);
}

check('grader: a true assertion with a live control PASSES', () => {
  assert(grade(true, 3) === true, 'should pass');
});

check('grader: a false assertion FAILS even with a live control', () => {
  assert(grade(false, 3) === false, 'should fail');
});

check('THE UNIVERSAL-DENIAL TRAP: a true assertion with a ZERO control FAILS', () => {
  // This is the exact shape that let 24-C3's `access` leg pass on all three
  // adapters at a broken binary BECAUSE EVERYTHING WAS DENIED, and the shape
  // that let the Discord mutation run satisfy `stranger_replies=0` under total
  // inbound loss. The control is part of the pass condition, not a decoration.
  assert(
    grade(true, 0) === false,
    'a leg whose assertion holds while nothing arrived MUST fail — this is the manufactured green',
  );
});

check('grader: a control of exactly 1 is live and does not fail the leg', () => {
  assert(grade(true, 1) === true, 'off-by-one: 1 arrival is a live control');
});

// ─────────────────────────────────────────────────────────────────────────────
// 2b. Submit acceptance — "never received" vs "received and dropped"
// ─────────────────────────────────────────────────────────────────────────────
//
// FOUND LIVE IN RUN 1 OF THIS DRIVER, and repaired in this lane rather than
// written up. The driver posted to an invented route, every submit returned
// `404`, and the driver graded the resulting zero arrivals as FAIL — reporting
// **its own wrong URL as product inbound loss**. Two opposite diagnoses were
// producing the same number, and the collapse blamed the product.

function acceptedFrom(stdout) {
  const m = /ST (\d{3})/.exec(stdout || '');
  const httpStatus = m ? Number(m[1]) : null;
  return { httpStatus, accepted: httpStatus !== null && httpStatus >= 200 && httpStatus < 300 };
}

check('known-positive: a 200 submit is accepted', () => {
  const a = acceptedFrom('ST 200 ok');
  assert(a.accepted === true, 'want accepted');
  assert(a.httpStatus === 200, `want 200, got ${a.httpStatus}`);
});

check('known-negative: a 404 submit is NOT accepted and is an instrument fault', () => {
  const a = acceptedFrom('ST 404 ');
  assert(a.accepted === false, '404 must not be accepted');
  assert(a.httpStatus === 404, `want 404, got ${a.httpStatus}`);
});

check('a transport error is not silently read as accepted', () => {
  const a = acceptedFrom('ERR fetch failed');
  assert(a.httpStatus === null, 'no status parsed');
  assert(a.accepted === false, 'an unparseable submit must never count as accepted');
});

check('THE THIRD ASSERTION: the old driver had no acceptance check at all', () => {
  // The pre-repair driver returned only {token, post} and every caller read the
  // arrival journal directly. Under a 404 storm that path yields found=false,
  // which the leg grader turns into FAIL — a product accusation.
  const oldDriverWouldGrade = (arrivalFound) => (arrivalFound ? 'PASS' : 'FAIL — product lost it');
  const newDriverGrades = (arrivalFound, accepted) =>
    !accepted ? 'INCOMPLETE — instrument fault' : arrivalFound ? 'PASS' : 'FAIL — product lost it';

  const a = acceptedFrom('ST 404 ');
  assert(
    oldDriverWouldGrade(false) === 'FAIL — product lost it',
    'precondition: the old shape must blame the product, else this case proves nothing',
  );
  assert(
    newDriverGrades(false, a.accepted) === 'INCOMPLETE — instrument fault',
    'the repaired driver must grade a never-accepted submit INCOMPLETE, not LOSS',
  );
  // And the repair must NOT launder a genuine product loss into INCOMPLETE.
  assert(
    newDriverGrades(false, true) === 'FAIL — product lost it',
    'a submit the product ACCEPTED and then dropped must still grade FAIL',
  );
});

check('source scan: the driver posts to the documented route, not an invented one', () => {
  const driverPath = path.join(path.dirname(new URL(import.meta.url).pathname), 'f24-c3-clauses.mjs');
  const src = fs.readFileSync(driverPath, 'utf8');
  assert(src.length > 10_000, 'driver source implausibly small — scan would be vacuous');
  // inbound_webhook.rs:12-15 documents exactly: POST /webhooks/:channel
  assert(/\/webhooks\/\$\{channelName\}/.test(src), 'driver must POST to /webhooks/:channel');
  assert(
    !/\/slack\/events/.test(src.replace(/^\s*\*.*$/gm, '')),
    'the invented /slack/events route must not survive outside comments',
  );
  assert(
    /this\.fault\(\s*`submit\//.test(src),
    'an unaccepted submit must raise an instrument fault',
  );
});

// ─────────────────────────────────────────────────────────────────────────────
// 2c. Token/fixture contract — the driver's token must be one the fixture echoes
// ─────────────────────────────────────────────────────────────────────────────
//
// FOUND LIVE IN RUN 2, repaired in-lane. `f24-llm-fixture.mjs:88-91` extracts a
// correlation token with `/f24c3-[a-z0-9-]+/i` and echoes the literal
// `no-correlation` when nothing matches. Run 2's token was `f24c3fin-...` — no
// hyphen after `f24c3`, so no match. The product path was FLAWLESS (submit
// accepted 200, one turn carrying the exact token, one reply at the sink) and
// the driver still graded FAIL. Third instrument fault of this lane and the
// third that blamed the product.
//
// The durable repair is not "fix the token" — it is to ASSERT THE CONTRACT here,
// so a future edit to either side reddens instead of silently un-correlating.

const FIXTURE_CORRELATION_RE = /f24c3-[a-z0-9-]+/i;

function driverTokenSample(channelName = 'f24finone') {
  const driverPath = path.join(path.dirname(new URL(import.meta.url).pathname), 'f24-c3-clauses.mjs');
  const src = fs.readFileSync(driverPath, 'utf8');
  assert(src.length > 10_000, 'driver source implausibly small — scan would be vacuous');
  const m = /const token = `([^`]+)`/.exec(src);
  assert(m, 'could not find the token template in the driver');
  // Materialise the template with plausible values.
  return m[1].replace('${channelName}', channelName).replace('${hex(4)}', 'a1b2c3d4');
}

check("known-positive: the driver's token matches the fixture's correlation regex", () => {
  const tok = driverTokenSample();
  assert(
    FIXTURE_CORRELATION_RE.test(tok),
    `driver token ${tok} does not match the fixture's ${FIXTURE_CORRELATION_RE} — ` +
      `every reply would come back as "no-correlation"`,
  );
});

check('known-negative: the run-2 token shape is proven NOT to match', () => {
  // Kept executable so the repair is proven to change an outcome rather than to
  // restate the new behaviour.
  assert(
    FIXTURE_CORRELATION_RE.test('f24c3fin-f24finone-a1b2c3d4') === false,
    'precondition: the run-2 shape must fail the regex, else this case proves nothing',
  );
});

check('THE THIRD ASSERTION: the fixture regex is read from the fixture, not hardcoded blind', () => {
  const fixturePath = path.join(path.dirname(new URL(import.meta.url).pathname), 'f24-llm-fixture.mjs');
  const src = fs.readFileSync(fixturePath, 'utf8');
  assert(src.length > 1_000, 'fixture source implausibly small — scan would be vacuous');
  // If the fixture's regex is ever changed, this reddens and forces the driver's
  // token to be re-checked against it, rather than the two drifting apart.
  assert(
    src.includes('/f24c3-[a-z0-9-]+/i'),
    'the fixture correlation regex changed — re-verify the driver token against it',
  );
  // And the whole extracted token must survive, not just its prefix.
  const tok = driverTokenSample();
  const m = FIXTURE_CORRELATION_RE.exec(tok);
  assert(m && m[0] === tok, `fixture would extract "${m && m[0]}" from "${tok}" — must be the whole token`);
});

// ─────────────────────────────────────────────────────────────────────────────
// 3. Journal reading — an empty file and an absent file must not read alike
// ─────────────────────────────────────────────────────────────────────────────

function readJournal(file) {
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
  return { records, bytes: Buffer.byteLength(raw, 'utf8'), existed: true };
}

check('journal: an ABSENT file and an EMPTY file are distinguishable', () => {
  const dir = fs.mkdtempSync(path.join(process.env.TMPDIR || '/tmp', 'f24selftest-'));
  const empty = path.join(dir, 'empty.jsonl');
  const missing = path.join(dir, 'missing.jsonl');
  fs.writeFileSync(empty, '');

  const e = readJournal(empty);
  const m = readJournal(missing);
  // Both have zero records. That is precisely why byte count and existence are
  // recorded: "0 arrivals" from a fixture that never started and "0 arrivals"
  // from a product that dropped everything are opposite diagnoses.
  assert(e.records.length === 0 && m.records.length === 0, 'both should parse to zero records');
  assert(e.existed === true && m.existed === false, 'existence must distinguish them');
  fs.rmSync(dir, { recursive: true, force: true });
});

check('journal: byte count is non-zero for a populated journal', () => {
  const dir = fs.mkdtempSync(path.join(process.env.TMPDIR || '/tmp', 'f24selftest-'));
  const f = path.join(dir, 'j.jsonl');
  fs.writeFileSync(f, `${JSON.stringify({ text: 'hello' })}\n`);
  const j = readJournal(f);
  assert(j.records.length === 1, `want 1 record, got ${j.records.length}`);
  assert(j.bytes > 0, 'byte count must be non-zero');
  fs.rmSync(dir, { recursive: true, force: true });
});

// ─────────────────────────────────────────────────────────────────────────────
// 4. Source scan — the driver must not have reintroduced a bare includes()
// ─────────────────────────────────────────────────────────────────────────────
//
// A sibling lane's repair was PARTIAL: `arrivalsFor` was moved onto the shared
// module and `runMatrix`'s route check was NOT, so one call site stayed broken
// and reported `carries_correlation=false` about a reply that had arrived. A
// comment would not have caught it. This scan does.

check('source scan: the driver delegates correlation and never re-implements it', () => {
  const driverPath = path.join(path.dirname(new URL(import.meta.url).pathname), 'f24-c3-clauses.mjs');
  const src = fs.readFileSync(driverPath, 'utf8');

  // Guard against a VACUOUS pass. If the file were missing or truncated the
  // regexes below would all find nothing and this test would go green having
  // proven nothing at all.
  assert(src.length > 10_000, `driver source implausibly small (${src.length} bytes) — scan would be vacuous`);

  assert(
    /import \{[^}]*classify[^}]*\} from '\.\/f24-correlate\.mjs'/.test(src),
    'driver must import the shared matcher',
  );
  // The import must NOT be inside a try/catch with a local fallback: a silent
  // degradation to a hand-rolled matcher fails in the direction that blames the
  // product.
  assert(
    !/try\s*\{[^}]*await import\('\.\/f24-correlate/.test(src),
    'the correlate import must not be wrapped in a silent try/catch fallback',
  );
  // No bare `.includes(token)` anywhere in the driver.
  const bareIncludes = src.match(/\.includes\(\s*token\s*\)/g) || [];
  assert(
    bareIncludes.length === 0,
    `driver contains ${bareIncludes.length} bare .includes(token) call(s) — use the shared matcher`,
  );
});

check('source scan: every recorded leg passes a positive control', () => {
  const driverPath = path.join(path.dirname(new URL(import.meta.url).pathname), 'f24-c3-clauses.mjs');
  const src = fs.readFileSync(driverPath, 'utf8');
  assert(src.length > 10_000, 'driver source implausibly small — scan would be vacuous');
  // The grader must fold the control into the returned pass value. If someone
  // "simplifies" record() to `pass = ok`, the universal-denial trap reopens.
  assert(
    /const pass = ok && \(controlOk === null \|\| controlOk\)/.test(src),
    'record() must fold the positive control into the pass condition',
  );
  assert(
    /zeroArrivals && bound/.test(src),
    'the driver must force FAIL on a green with zero arrivals',
  );
});

// ─────────────────────────────────────────────────────────────────────────────
// 5. F24-C3-H5 — the POSTURE matcher and the DENIAL matcher
//
// These two are new instruments, so each gets the mandatory three assertions:
// a known-positive passes, a known-negative fails, and the OLD, weaker shape
// would have missed the thing the new one catches.
// ─────────────────────────────────────────────────────────────────────────────

/** The gateway log the driver parses, in the shape tracing actually writes. */
const LOG_REPAIRED = [
  '2026-07-29T10:00:00Z  INFO wcore_channels: channel auto-registered channel=f24finone platform=slack',
  '2026-07-29T10:00:01Z DEBUG wcore_agent::channel_dispatch: channel turn dispatch channel=f24finone posture=Conversational',
  '2026-07-29T10:00:05Z  INFO wcore_cli::gateway: [gateway] channel reload: added=["f24finthree"] policies=3',
  '2026-07-29T10:00:07Z DEBUG wcore_agent::channel_dispatch: channel turn dispatch channel=f24finthree posture=Workspace',
  '2026-07-29T10:00:09Z  INFO wcore_agent::channel_inbound: inbound denied channel=f24finthree reason=sender not in dm allowlist',
  '',
].join('\n');

/** The SAME log as the half-fix would produce: the message arrives (there IS a
 *  dispatch line) but the posture is the dispatcher's fallback. */
const LOG_HALF_FIX = LOG_REPAIRED.replace(
  'channel=f24finthree posture=Workspace',
  'channel=f24finthree posture=Conversational',
);

check('posture matcher, known-positive: the repaired log yields Workspace', () => {
  const p = observedPostureIn(LOG_REPAIRED, 'f24finthree');
  assert(p.posture === 'Workspace', `want Workspace, got ${p.posture}`);
  assert(p.tier === 'same-line', `want same-line tier, got ${p.tier}`);
  assert(p.sightings === 1, `want 1 sighting, got ${p.sightings}`);
});

check('posture matcher, known-negative: the HALF-FIX log yields Conversational', () => {
  const p = observedPostureIn(LOG_HALF_FIX, 'f24finthree');
  assert(
    p.posture === 'Conversational',
    `the half-fix must be VISIBLE as the fallback posture, got ${p.posture}`,
  );
  // And the driver's leg condition (=== 'Workspace') must therefore FAIL on it.
  assert(p.posture !== 'Workspace', 'the acceptance leg must not pass on the half-fix');
});

check('posture matcher: the OLD arrivals-only shape would have missed the half-fix', () => {
  // The pre-H5 acceptance shape was "did an arrival land". Both logs contain a
  // dispatch line for the reloaded channel — i.e. the message got through in
  // BOTH worlds — so an arrivals-only grader is identical across them and
  // cannot discriminate. The posture matcher is the only thing that can.
  const arrivedRepaired = /channel turn dispatch[^\n]*channel=f24finthree/.test(LOG_REPAIRED);
  const arrivedHalfFix = /channel turn dispatch[^\n]*channel=f24finthree/.test(LOG_HALF_FIX);
  assert(arrivedRepaired && arrivedHalfFix, 'both worlds deliver the message — that is the trap');
  assert(
    arrivedRepaired === arrivedHalfFix,
    'the old shape is blind to the difference (this is the point)',
  );
  assert(
    observedPostureIn(LOG_REPAIRED, 'f24finthree').posture !==
      observedPostureIn(LOG_HALF_FIX, 'f24finthree').posture,
    'the NEW matcher must discriminate where the old one could not',
  );
});

check('posture matcher: an unobserved channel returns null, never the fallback', () => {
  const p = observedPostureIn(LOG_REPAIRED, 'nosuchchannel');
  assert(p.posture === null, `want null, got ${p.posture}`);
  assert(p.tier === 'absent', `want absent tier, got ${p.tier}`);
  // Collapsing "never observed" into "Conversational" would make a dead
  // instrument report the safe-looking answer.
  assert(p.posture !== 'Conversational', 'null must not be collapsed into the fallback');
});

check('denial matcher, known-positive: a real denial line is SIGHTED', () => {
  assert(denialsIn(LOG_REPAIRED, 'f24finthree') === 1, 'want exactly 1 denial sighting');
});

check('denial matcher, known-negative: no denial for a channel that was not denied', () => {
  assert(denialsIn(LOG_REPAIRED, 'f24finone') === 0, 'the admitted channel must show 0 denials');
  // The instrument is proven ALIVE in the same test. A zero from a dead
  // matcher is otherwise indistinguishable from a genuine absence (§3b-i).
  assert(
    denialsIn(LOG_REPAIRED, 'f24finthree') === 1,
    'liveness control: the same matcher over the same log must still see the real denial',
  );
});

check('denial matcher: the OLD shape (absence of arrival) would have missed a dead pipe', () => {
  // The weaker assertion is "no new arrival landed". An EMPTY log satisfies it
  // perfectly — which is what a crashed gateway, a wrong URL or a typo'd
  // channel name produces. The sighting-based matcher reports 0 there and so
  // refuses to grade the leg a pass.
  const deadPipe = '';
  const arrivalsOnlyWouldPass = true; // "no arrival landed" is trivially true
  assert(arrivalsOnlyWouldPass, 'the old shape passes on an empty log');
  assert(
    denialsIn(deadPipe, 'f24finthree') === 0,
    'the new matcher must report 0 sightings on a dead pipe, so the leg cannot pass',
  );
  assert(
    denialsIn(LOG_REPAIRED, 'f24finthree') > denialsIn(deadPipe, 'f24finthree'),
    'and it must discriminate a real denial from a dead pipe, which the old shape could not',
  );
});

check('both matchers: a channel name is escaped, not injected, into the pattern', () => {
  // A name containing regex metacharacters must not silently widen the match —
  // that is how one channel's evidence gets attributed to another.
  assert(observedPostureIn(LOG_REPAIRED, 'f24fin.hree').posture === null, 'dot must not match "t"');
  assert(denialsIn(LOG_REPAIRED, 'f24fin.hree') === 0, 'dot must not match "t"');
  assert(
    observedPostureIn(LOG_REPAIRED, 'f24finthree').posture === 'Workspace',
    'liveness control: the exact name still matches',
  );
});

check('source scan: the driver asks for the log target that carries the posture', () => {
  const driverPath = path.join(path.dirname(new URL(import.meta.url).pathname), 'f24-c3-clauses.mjs');
  const src = fs.readFileSync(driverPath, 'utf8');
  assert(src.length > 10_000, 'driver source implausibly small — scan would be vacuous');
  // Without this RUST_LOG target the posture line is never emitted, and every
  // posture leg would grade `null` — an instrument fault dressed as a finding.
  assert(
    /wcore_agent::channel_dispatch=debug/.test(src),
    'the driver must enable the channel_dispatch debug target or no posture can be observed',
  );
  // The reloaded channel must be configured with a NON-DEFAULT posture, or the
  // half-fix and the repair produce identical logs and the leg proves nothing.
  assert(
    /writeSlackChannel\('f24finthree', 'U24FINTHREE', 'workspace'/.test(src),
    'the reloaded channel must be configured tools="workspace"; with the default the ' +
      'posture leg cannot distinguish the repair from the half-fix',
  );
  assert(
    /reloaded-adapter-still-denies-a-non-allowlisted-sender/.test(src),
    'fail-closed must be re-asserted after the repair',
  );
});

// ─────────────────────────────────────────────────────────────────────────────

process.stdout.write(`\nselftest: passed=${passed} failed=${failed}\n`);
process.exit(failed === 0 ? 0 : 1);
