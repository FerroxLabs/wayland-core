#!/usr/bin/env node
// The four quadrants of the delivery-leg gate, as REAL RECEIPT BYTES.
//
// # Why this file exists
//
// The journey has two gates over one receipt: `classifyVerdict` in
// `f24-journey.mjs` (JavaScript, run by the driver at the end of a live journey)
// and `verify_counts` in `crates/wcore-eval-scenarios/src/journey.rs` (Rust, run
// by `wayland-journey verify` in CI and by hand). A state where one passes and
// the other fails on the SAME receipt is worse than either being wrong alone:
// every downstream grade becomes unreadable, and whichever gate a reader happens
// to run decides the answer.
//
// "They agree" is therefore not something to assert in a summary. It is proved
// by driving both over the same bytes and comparing the `verdict=<TOKEN>` each
// emits. This module produces those bytes:
//
//   Q1  recurrence    12 repeats, every one under a DISTINCT delivery identity
//   Q2  replay        the same delivery identity twice
//   Q3  indeterminate a repeat whose arrivals carry no identity at all
//   Q4  clean         no repeats
//
// Q1 is the state the gate could not reach before this lane. On Windows the
// Task Scheduler minimum repetition interval is `PT1M`, which exceeds the 60 s
// rate floor `every:15` is clamped to (`trigger.rs:238`, applied at `:366`), so
// a Windows kill-and-recover leg ALWAYS crosses a trigger period while launchd
// and systemd restart inside it. Both gates refused any `duplicates != 0`, so
// the Windows journey had no achievable pass state at all.
//
// # These are not hand-written JSON
//
// Every receipt below is produced by the driver's own `Journey.receipt()` from a
// synthetic ARRIVAL JOURNAL — the same `snapshot()` → `classifyRepeats()` path a
// live run uses. Hand-writing the JSON would test the fixture, not the driver,
// and the previous defect in this area (F24-GWP-M1) lived precisely in the gap
// between what the driver computed and what the receipt said.
//
// Run it to regenerate the committed fixtures:
//
//   node scripts/f24-journey-quadrants.mjs --write
//
// `f24-journey.test.mjs` asserts the committed bytes still match what this
// produces, so a driver change that alters a receipt cannot land without the
// Rust side's fixtures moving with it.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { ADAPTERS, CANONICAL_STEPS, Journey, VERDICT, classifyVerdict } from './f24-journey.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));

/// Where the Rust contract test reads them from. Deliberately inside the
/// verifier's own crate: the fixtures are the verifier's inputs, and a fixture
/// that lives beside the producer is one nobody notices the consumer stopped
/// reading.
export const FIXTURE_DIR = path.join(
  HERE,
  '..',
  'crates',
  'wcore-eval-scenarios',
  'tests',
  'fixtures',
  'journey-quadrants',
);

// A pinned identity so the bytes are reproducible. A receipt whose content
// changed on every run could not be committed, and a fixture that is regenerated
// rather than compared proves nothing about drift.
const PINNED = {
  candidateCommit: 'a'.repeat(40),
  binaryVersion: 'wayland-core 0.12.25',
  binarySha256: 'b'.repeat(64),
  driverCommit: 'c'.repeat(40),
  startedAt: '2026-07-30T00:00:00.000Z',
  finishedAt: '2026-07-30T00:04:00.000Z',
};

const DELIVERIES = 12;

/// A journey with its seventeen steps stubbed and its identity pinned, so
/// `receipt()` is reachable without hardware and its output is byte-stable.
function stagedJourney(runDir) {
  const journey = new Journey({ platform: 'windows', binary: 'C:/f24j/wayland-core.exe', runDir });
  for (let i = 0; i < CANONICAL_STEPS.length; i += 1) {
    journey.step(CANONICAL_STEPS[i], `command for ${CANONICAL_STEPS[i]}`, `output ${i}`);
  }
  journey.candidateCommit = PINNED.candidateCommit;
  journey.binaryVersion = PINNED.binaryVersion;
  journey.binarySha256 = PINNED.binarySha256;
  journey.startedAt = PINNED.startedAt;
  for (let i = 1; i <= DELIVERIES; i += 1) {
    const body = `f24j-delivery-${String(i).padStart(2, '0')}`;
    journey.bodies.push(body);
    journey.bodyAdapter.set(body, ADAPTERS[(i - 1) % ADAPTERS.length].adapter);
  }
  journey.counts.submitted = DELIVERIES;
  fs.mkdirSync(journey.runDir, { recursive: true });
  return journey;
}

function endpointFor(journey, body) {
  const adapter = journey.bodyAdapter.get(body);
  return ADAPTERS.find((spec) => spec.adapter === adapter).endpoint;
}

/// One sink journal line, in the shape the independent sink actually writes.
/// `key` is the delivery identity `cron:{job}:{scheduled_millis}` the adapter put
/// on the wire, or `null` for the adapters that emit none.
function arrival(journey, body, key) {
  return JSON.stringify({
    text: body,
    endpoint: endpointFor(journey, body),
    idempotency_key: key,
    suppressed: false,
  });
}

/// The delivery identity a keyed adapter emits for one job at one instant.
const identityFor = (body, occurrence) =>
  `cron:${body}:${1_769_731_200_000 + occurrence * 60_000}`;

/// Build one quadrant. `journalFor` returns the journal lines; the receipt is
/// then whatever the DRIVER makes of them.
function quadrant(name, expected, note, journalFor) {
  const runDir = fs.mkdtempSync(path.join(os.tmpdir(), 'f24j-quadrant-'));
  const journey = stagedJourney(runDir);
  fs.writeFileSync(journey.journalPath, `${journalFor(journey).join('\n')}\n`);
  // Step 13's frozen reading, exactly as a live run takes it: before the burst.
  journey.counts = journey.tally();
  const receipt = {
    ...journey.receipt(),
    // Pinned last so `receipt()` still computes everything else itself.
    driver_commit: PINNED.driverCommit,
    finished_at: PINNED.finishedAt,
  };
  const outcome = classifyVerdict(receipt.counts, receipt.delivery_identity);
  fs.rmSync(runDir, { recursive: true, force: true });
  return { name, expected, note, receipt, outcome };
}

export function buildQuadrants() {
  return [
    // ── Q1 ─────────────────────────────────────────────────────────────────
    // The state that was unreachable. Every body arrives twice, each occurrence
    // under its OWN delivery identity, because the 60 s recurring trigger fired
    // again while the platform was bringing the runtime back. Keyed adapters
    // only — see the note on Q3 for why that restriction is load-bearing and not
    // a convenience.
    quadrant(
      'q1-recurrence-passes',
      VERDICT.RECURRENCE,
      'an honest slow-platform run: 12 repeats, 12 distinct delivery identities, zero replays',
      (journey) => [
        ...journey.bodies.map((body) => arrival(journey, body, identityFor(body, 0))),
        ...journey.bodies.map((body) => arrival(journey, body, identityFor(body, 1))),
      ],
    ),

    // ── Q2 ─────────────────────────────────────────────────────────────────
    // Identical to Q1 except ONE body's second arrival reuses the first's
    // identity. Same headline, same partition sum, one bucket moved — so this
    // cannot fail for any reason other than the replay.
    quadrant(
      'q2-replay-fails',
      VERDICT.EXACTLY_ONCE_VIOLATED,
      'one planted true replay: the same delivery identity delivered twice',
      (journey) => [
        ...journey.bodies.map((body) => arrival(journey, body, identityFor(body, 0))),
        ...journey.bodies.map((body, index) =>
          arrival(journey, body, identityFor(body, index === 0 ? 0 : 1)),
        ),
      ],
    ),

    // ── Q3 ─────────────────────────────────────────────────────────────────
    // The real Windows shape. `twilio.messages` and `whatsapp.messages` emit no
    // idempotency key, so for those a replay is indistinguishable from a
    // recurrence IN PRINCIPLE. Passing them would publish an unmeasurable
    // property as a measured clean one, which is the F24-GWP-M1 defect one level
    // down, so they count AGAINST the run.
    quadrant(
      'q3-indeterminate-fails',
      VERDICT.NOT_PROVEN,
      'the real adapter mix: 8 of 24 arrivals carry an identity, so 8 repeats cannot be judged',
      (journey) => {
        const keyed = (body) => endpointFor(journey, body) === 'chat.postMessage';
        return [
          ...journey.bodies.map((body) =>
            arrival(journey, body, keyed(body) ? identityFor(body, 0) : null),
          ),
          ...journey.bodies.map((body) =>
            arrival(journey, body, keyed(body) ? identityFor(body, 1) : null),
          ),
        ];
      },
    ),

    // ── Q4 ─────────────────────────────────────────────────────────────────
    // The fast-platform run: launchd and systemd restart inside the 60 s window,
    // so no trigger period is crossed. This must still pass, or the change has
    // merely swapped one permanently-red gate for a gate that only knows how to
    // grade runs WITH repeats.
    quadrant(
      'q4-clean-passes',
      VERDICT.NO_REPEATS,
      'a fast-platform run that finished inside one trigger period: one arrival per body',
      (journey) => journey.bodies.map((body) => arrival(journey, body, identityFor(body, 0))),
    ),
  ];
}

/// Byte-exact serialisation, used by both the writer and the drift assertion.
export const serialise = (receipt) => `${JSON.stringify(receipt, null, 2)}\n`;

export function fixturePath(name) {
  return path.join(FIXTURE_DIR, `${name}.json`);
}

function main() {
  const write = process.argv.includes('--write');
  const quadrants = buildQuadrants();
  if (write) fs.mkdirSync(FIXTURE_DIR, { recursive: true });
  let drift = 0;
  for (const q of quadrants) {
    const target = fixturePath(q.name);
    const body = serialise(q.receipt);
    if (write) {
      fs.writeFileSync(target, body);
    } else if (!fs.existsSync(target) || fs.readFileSync(target, 'utf8') !== body) {
      drift += 1;
      process.stderr.write(`DRIFT ${q.name}: committed fixture differs from the driver's output\n`);
    }
    const agree = q.outcome.verdict === q.expected;
    if (!agree) drift += 1;
    process.stdout.write(
      `${q.name} expected=${q.expected} driver=${q.outcome.verdict} ` +
        `clean=${q.outcome.clean} duplicates=${q.receipt.counts.duplicates} ` +
        `replays=${q.receipt.delivery_identity.replays} ` +
        `recurrences=${q.receipt.delivery_identity.recurrences} ` +
        `indeterminate=${q.receipt.delivery_identity.indeterminate} ` +
        `unidentified=${q.receipt.delivery_identity.unidentified} ` +
        `${agree ? 'OK' : 'MISMATCH'}\n`,
    );
  }
  process.stdout.write(`QUADRANTS ${quadrants.length} drift=${drift}\n`);
  process.exitCode = drift === 0 ? 0 : 1;
}

if (process.argv[1] && fs.realpathSync(process.argv[1]) === fs.realpathSync(fileURLToPath(import.meta.url))) {
  main();
}
