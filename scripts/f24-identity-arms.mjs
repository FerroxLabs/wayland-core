#!/usr/bin/env node
// Grade an arrivals journal, ARM BY ARM, with the journey's OWN classifier.
//
// # Why this exists
//
// `lane/twilio-whatsapp-identity` changed what the Twilio and WhatsApp adapters
// put on the wire, and the claim being made is about a NUMBER the journey gate
// produces: `indeterminate`. A claim about a gate's output has to be measured by
// running that gate, not by reasoning about the change.
//
// So this imports `classifyRepeats` from `f24-journey.mjs` — the same function a
// live journey run calls, not a copy of it. A parallel reimplementation here
// would grade the reimplementation, which is precisely the shape
// `f24-journey-quadrants.mjs` was written to avoid one level up.
//
// # Both directions, in one run
//
// A gate that cannot fail proves as little as one that cannot pass
// (LANE-BRIEF §3.2 and §3b-iii). The Rust driver that produces the journal
// therefore emits three arms deliberately, and this script grades each
// separately rather than aggregating them into one comfortable total:
//
//   A-UNKEYED   a repeated body whose arrivals carry NO identity
//               -> indeterminate > 0, verdict NOT-PROVEN   (the gate CAN fail,
//                  and this reproduces the pre-change state on the same binary)
//   B-KEYED     the same body under two DISTINCT delivery ids
//               -> recurrences > 0, indeterminate 0, verdict RECURRENCE
//                  (the gate CAN pass — the state that was unreachable for
//                  these two adapters before, in principle and not by accident)
//   C-REPLAY    the same body under ONE delivery id, twice
//               -> replays > 0, verdict EXACTLY-ONCE-VIOLATED (the gate still
//                  catches the real violation; identity did not make it blind)
//
// Arm C matters most for trust. Adding an identity to arrivals makes repeats
// classifiable, and the cheap way for that to go wrong is for everything to
// classify as a benign recurrence. C is the control proving it does not.
//
// # Usage
//
//   node scripts/f24-identity-arms.mjs --journal /path/to/arrivals.jsonl
//
// Prints one JSON object on stdout: `{ "<arm>": { "<endpoint>": {tally} } }`,
// plus a human-readable table on stderr. The Rust driver parses the stdout.

import fs from 'node:fs';
import { classifyRepeats } from './f24-journey.mjs';

function parseArgs(argv) {
  let journal = null;
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === '--journal') journal = argv[++i];
    else {
      process.stderr.write(`f24-identity-arms: unknown argument ${argv[i]}\n`);
      process.exit(2);
    }
  }
  if (!journal) {
    process.stderr.write('f24-identity-arms: --journal is required\n');
    process.exit(2);
  }
  return { journal };
}

// Mirrors `classifyVerdict` in f24-journey.mjs for the identity buckets alone.
// Kept tiny and local because the full verdict needs the five headline counts,
// which an arm-scoped slice does not have; the bucket precedence is the part
// under test here and it is asserted against the Rust `DeliveryIdentity::
// verdict` by the driver.
function verdictFor(t) {
  if (t.replays > 0) return 'EXACTLY-ONCE-VIOLATED';
  if (t.indeterminate > 0) return 'NOT-PROVEN';
  if (t.recurrences > 0) return 'RECURRENCE';
  return 'NO-REPEATS';
}

const { journal } = parseArgs(process.argv.slice(2));
const raw = fs.readFileSync(journal, 'utf8');
const arrivals = raw
  .split('\n')
  .filter((l) => l.trim())
  .map((l) => JSON.parse(l));

if (arrivals.length === 0) {
  // A journal with no arrivals grades every arm clean, which is the classic
  // green-by-universal-denial. Refuse rather than report it.
  process.stderr.write(
    'f24-identity-arms: the journal is EMPTY. Every arm would grade clean over zero ' +
      'arrivals, which is a green manufactured by nothing having happened. Refusing.\n',
  );
  process.exit(2);
}

// The arm is encoded in the body text by the Rust driver: `<ARM>|<payload>`.
const arms = new Map();
for (const a of arrivals) {
  const arm = String(a.text ?? '').split('|')[0];
  const endpoint = a.endpoint ?? '(none)';
  if (!arms.has(arm)) arms.set(arm, new Map());
  const byEndpoint = arms.get(arm);
  if (!byEndpoint.has(endpoint)) byEndpoint.set(endpoint, []);
  byEndpoint.get(endpoint).push(a);
}

const out = {};
process.stderr.write(
  `\narrivals=${arrivals.length} arms=${arms.size}\n` +
    'arm         endpoint            arr  replay  recur  indet  unid  verdict\n' +
    '----------- ------------------- ---- ------- ------ ------ ----- --------------------\n',
);
for (const [arm, byEndpoint] of [...arms.entries()].sort()) {
  out[arm] = {};
  for (const [endpoint, seen] of [...byEndpoint.entries()].sort()) {
    const tally = classifyRepeats(seen);
    const verdict = verdictFor(tally);
    out[arm][endpoint] = { ...tally, arrived: seen.length, verdict };
    process.stderr.write(
      `${arm.padEnd(11)} ${endpoint.padEnd(19)} ${String(seen.length).padStart(4)} ` +
        `${String(tally.replays).padStart(7)} ${String(tally.recurrences).padStart(6)} ` +
        `${String(tally.indeterminate).padStart(6)} ${String(tally.unidentified).padStart(5)} ` +
        `${verdict}\n`,
    );
  }
}
process.stderr.write('\n');
process.stdout.write(`${JSON.stringify(out, null, 2)}\n`);
