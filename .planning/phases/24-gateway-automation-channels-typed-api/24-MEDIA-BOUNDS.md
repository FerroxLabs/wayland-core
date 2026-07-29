---
lane: 24-media-bounds
finding-addressed: "Media bounds (open HIGH, HANDOFF-2026-07-29 §3) — declared media limits are decorative. Also filed independently as F24-C3-H6 (24-MEDIA-ACTIONS §4) and F24-MSTEAMS-H1 (24-MSTEAMS-ATTACH)."
grade: "`24-C3` NOT CLAIMED — this is the eighth lane to decline it. The `media` clause element this lane moved: the declared per-adapter bound is now READ AND ENFORCED in production, on every adapter, at both the byte and the attachment-count boundary, proven by mutation and by the real binary. That is a repair of the advertised-but-dead surface, NOT the live media round-trip the criterion asks for — no description or transcript is produced here and that half is untouched."
new-finding: "F24-MB-1 (MEDIUM) — the scope of the divergence was understated by every prior report: NINE adapters diverged, not two. Seven enforce an undeclared hardcoded cap (matrix/slack/telegram/whatsapp 100 MiB, imessage/signal 64 MiB, sms 16 MiB) while inheriting a 25 MiB trait default. Of every adapter that enforced any cap, ZERO enforced the one it advertised. FIXED in this lane."
fence-exposure: "21 files vs merge-base 35243f36: 19 source files across wcore-channels/wcore-agent + 9 adapter crates, 1 new test file, 1 new derive script. ZERO deletions of tracked files. Shared-file fence (crates/wcore-cli/src/{lib,main}.rs) NOT touched — empty diff, verified. No contract regeneration owed: MediaBounds is absent from every Desktop wire-contract fixture (checked with a live known-positive)."
status: complete
---

# 24-MEDIA-BOUNDS — the declaration is now the enforcement

**Verdict up front.** The finding I was handed was **true, and understated**. `media_bounds()` was
read at exactly one site and that site was a test; both cited divergence numbers were exact; and
the scope was **nine adapters, not two**. It is now fixed: every adapter's advertised media bound
is the number its own fetch path enforces, by construction, and the count bound has an enforcement
point for the first time. Proven by mutation at both enforcement points and by a live run of the
real `wayland-core` binary.

**I do not claim `24-C3`.** Seven lanes have declined it and this is the eighth.

---

## 1. What I re-measured, and whether the handed numbers held

I was told explicitly that findings handed to lanes on this program have been wrong in both
directions, so nothing below is carried forward from the assignment.

### 1a. The one-read claim — HELD at `15cda12d`, and is now STALE

Instrument: `/usr/bin/grep`, unproxied (`rtk` rewrites `grep` — LANE-BRIEF §3b), glob quoted
(`--include='*.rs'`; zsh eats it unquoted, which produced a free false-negative for a prior lane).

**Known-positive control, same invocation, same tool, same paths:** `max_message_len` → **24 hits**.
The instrument is alive, so a zero from it means something.

At `15cda12d`, `media_bounds` had **5 sites: 4 definitions and 1 read, and the read was in
`tests/`.** Claim confirmed exactly as handed.

**At the new base `35243f36` this is no longer true, and my report says so rather than repeating
the number I was given.** Lane `24-msteams-attach` merged the first production consumer:

```
/usr/bin/git grep -n "media_bounds" 35243f36 -- 'crates/**/*.rs'
  wcore-channel-discord/src/lib.rs:405     DECLARES
  wcore-channel-email/src/lib.rs:536       DECLARES
  wcore-channel-msteams/src/inbound.rs:204 doc
  wcore-channel-msteams/src/lib.rs:141     <- PRODUCTION READ (new)
  wcore-channel-msteams/src/lib.rs:375     doc
  wcore-channels/src/lib.rs:168            trait default
  wcore-channels/tests/framework_matrix.rs:156  test impl
  wcore-channels/tests/framework_matrix.rs:373  <- test read
```

**The count at my base is 2 reads: one production (msteams) and one test.** `normalize_all`
likewise went from 0 production callers to 1 (`msteams/src/inbound.rs:175`).

That consumer does **not** overlap with this lane's repair, and the distinction matters:

| half | what it bounds | consumer |
|---|---|---|
| **declaration-time** (`normalize_all`) | the size the PLATFORM reported, at parse | msteams, as of `35243f36` |
| **fetch-time** (this lane) | the bytes actually received | every adapter, as of this lane |

msteams leaves `media_bounds()` at the trait default and implements no `fetch_media`, and Bot
Framework reports no attachment size — so on msteams **only the count bound is reachable**, which
its own source says. It is a genuine known-positive that the surface can be consumed, and it is
not a substitute for enforcing the byte bound, which still had zero production readers.

### 1b. Both divergence numbers — HELD, exactly

| adapter | declared | enforced | site | ratio |
|---|---|---|---|---|
| discord | 25 MiB | **100 MiB** | `discord/src/rest.rs:370` | **4.0× larger** |
| email | 10 MiB | **2 MiB** | `email/src/imap.rs:619` | **5.0× smaller** |

Both directions of error present, as reported.

### 1c. NEW — the finding was understated. Seven more adapters. (F24-MB-1)

Assignment item 4 asked whether any *other* adapter enforces an undeclared cap. **Seven do.** Each
declared nothing, so each advertised the 25 MiB trait default while enforcing something else:

| adapter | enforced | site | vs the 25 MiB it advertised |
|---|---|---|---|
| matrix | 100 MiB | `matrix/src/rest.rs:100` | 4.0× larger |
| slack | 100 MiB | `slack/src/api.rs:354` | 4.0× larger |
| telegram | 100 MiB | `telegram/src/api.rs:916` | 4.0× larger |
| whatsapp | 100 MiB | `whatsapp/src/api.rs:619` | 4.0× larger |
| imessage | 64 MiB | `imessage/src/channel.rs:37` | 2.56× larger |
| signal | 64 MiB | `signal/src/lib.rs:116` | 2.56× larger |
| sms | 16 MiB | `sms/src/api.rs:29` | **0.64× — smaller** |

**Nine adapters diverged, not two. Of every adapter enforcing any cap, ZERO enforced the one it
advertised.** `max_attachments` was worse still: enforced **nowhere in the workspace**, its only
non-definition use being inside `normalize_all`, which had no production caller until msteams.

This is the same **advertised-but-dead** family as the fourth stale site lane `24-msteams-attach`
found in the operator-facing msteams config schema.

---

## 2. The decision that shaped the fix, and the evidence that settled it

"Make enforcement match the declaration" and "make the declaration match enforcement" are both
defensible and produce **opposite runtime behaviour**. Enforcement-follows-declaration would drop
discord's fetch cap 100 → 25 MiB, degrading boosted/Nitro uploads that legitimately exceed 25 MiB —
a functional regression I cannot validate against a live boosted server.

**Cross-audit panel (LANE-BRIEF §4): UNANIMOUS, 3/3, for declaration-follows-enforcement.**

| auditor | vote |
|---|---|
| codex `gpt-5.6-sol` | `PANEL_POSITION=B` — "B is contract repair; A is an undocumented policy change disguised as consistency" |
| gemini 3.1 pro | `PANEL_POSITION=B` — "prevents capability regressions … eliminating the drift" |
| kimi K3 | `PANEL_POSITION=B` — "A is the riskier option dressed up as the stricter one" |

All three independently raised the **same** objection, and it was the only one that mattered:
*B assumes today's enforcement numbers are intentional, and you have not checked.* Kimi named the
decisive test — blame the enforcement constants, not just the declaration.

**Result: the objection is falsified, and the causality runs the other way.**

| line | commit | date |
|---|---|---|
| discord / telegram / slack 100 MiB | `a1085393e` | **2026-06-12** |
| sms 16 MiB | `f638d68f5` | **2026-06-12** |
| signal 64 MiB | `da6a3c62a` | **2026-06-12** |
| imessage 64 MiB | `16d09fdba` | **2026-06-12** |
| email 2 MiB | `ce6e88e99` | **2026-06-12** |
| matrix 100 MiB | `8273b2ac1` | **2026-06-18** |
| discord + email **declarations** | `9b06a4778` | **2026-07-27** |
| `media.rs` itself (the whole `MediaBounds` module) | `de0367b0` | **2026-07-27** |

**The enforcement predates the declaration by six weeks, across six separate commits, each written
beside the download path it guards. The declaration is the late artifact, authored in one sweep,
and never wired to anything.** By kimi's own stated criterion — "if blame shows 100 MiB was
deliberate, B is unambiguously right" — B is unambiguously right.

This also sharpens the finding: the bounds API was **advertised-but-dead from birth**. No consumer
rotted away; there was never one. It shipped as contract documentation for enforcement that already
existed elsewhere, and was never connected to it.

**Mitigations adopted anyway, because the objection was good:** `DEFAULT_MAX_BYTES` stays at the
conservative 25 MiB so a NEW adapter declaring nothing still inherits the tight value; and discord's
platform fact (25 MiB non-boosted ceiling) is preserved in its doc comment rather than deleted.

---

## 3. What landed

**One `MEDIA_BOUNDS` constant per adapter crate.** `media_bounds()` returns it and that crate's own
fetch/inline cap is derived from it, so the advertised number and the enforced number are **the same
number by construction** — they cannot drift apart again, because there is only one.

Two enforcement points, because one is not enough:

- **`ChannelManager::fetch_media_on`** — the only production path to adapter media — checks every
  payload against the originating channel's declaration. This makes the declaration load-bearing for
  **all 11 adapters at once**, including those carrying no size check of their own.
- **Each adapter's own streamed/disk/inline cap**, so an oversize payload is refused *before* it is
  buffered rather than after.
- **`ChannelMediaEnricher::enrich`** applies `max_attachments`, which had no enforcement point
  anywhere. It is the only place the whole list is in scope. Past-bound attachments are **degraded
  with a reason, never removed** — a truncated list is a message the agent answers without knowing
  it was incomplete, which is the rule `media.rs`'s own header exists to enforce.

**No adapter's effective runtime limit changes.** Declared values are set to each adapter's
already-operative cap.

---

## 4. Proof — three assertions per enforcement point, the third executed not asserted

A known-negative is self-passing on a dead instrument, so every case carries a known-positive, a
known-negative, **and evidence the pre-fix shape would have missed it**.

### 4a. Byte bound — `wcore-channels --test media_bounds_enforced` (6 tests)

```
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

`0 ignored` / `0 filtered out` read back explicitly — the suite ran 6 tests, it did not exit 0
having run none. Taken via `rtk proxy`, because plain `cargo` here **strips those exact two fields**.

**MUTATION PROOF.** `fetch_media_on` reverted to the pre-fix two-line body at the identical commit,
with an assert on the mutation anchor so a silent no-op revert could not fake the proof:

| build | result |
|---|---|
| **pre-fix enforcement** | `FAILED. 1 passed; 5 failed; 0 ignored; 0 filtered out` |
| **fixed** (restored, control) | `ok. 6 passed; 0 failed; 0 ignored; 0 filtered out` |

**The single test that passes pre-fix is the known-positive liveness control** — exactly correct.
Had it also failed, the suite would have been measuring a broken harness rather than the enforcement.

### 4b. Count bound — `wcore-agent --lib channel_media` (14 tests)

Mutation: count check disabled (`if false && …`), the pre-fix state.

| build | the count-bound test |
|---|---|
| **pre-fix** | `FAILED. 13 passed; 1 failed` |
| **fixed** | `ok. 14 passed; 0 failed` |

The within-bound control passes on **both** builds, as it must.

### 4c. LIVE — the real binary, `gateway run`, real discord gateway fixture

Standing rule §3.1: live testing ranks at least as high as green code. Driver
`scripts/f24-media-count-bound.mjs`, derived by asserted patch from the already-proven
`f24-media-actions.mjs` harness so the two cannot drift (`f24-media-count-bound-derive.py`; every
substitution asserts its anchor, and a no-op patch refuses to emit).

Two legs differing in **exactly one variable** — the attachment count. Two full runs, distinct
`shasum`s, `all_pass=true` both times.

| gate | kind | result | measured |
|---|---|---|---|
| **C1** | POSITIVE | **PASS** | `identified=true turn_ran=true notice=true hits=2 (want exactly 2)` |
| **C2** | NEGATIVE CONTROL | **PASS** | `turn_ran=true capture_alive=true notice=false` |
| **C3** | LIVENESS CONTROL | **PASS** | `turn_ran=true prompts=1 vision_notice=true` |

C2 requires `capture_alive=true` — the control prompt must be captured *and* contain the leg's probe
text — before its `notice=false` counts. A dead capture cannot pass it. C3 proves the first ten were
**processed, not skipped**, which is what stops C1 passing on an enricher that simply bailed out.

**The turn prompt, verbatim, from the real binary** (`run1-A-turn-prompt.txt`) — the declared bound
is 10, twelve arrived:

```
  10. Image (image/png) — description: [Inbound image received but NOT analyzed: no vision
backend is configured, …]

  11. Image (image/png) — description: [Inbound attachment 11 of 12 was NOT analyzed: it is
past this channel's declared bound of 10 attachments per message. The assistant has NOT seen
its contents; do not guess.]

  12. Image (image/png) — description: [Inbound attachment 12 of 12 was NOT analyzed: it is
past this channel's declared bound of 10 attachments per message. The assistant has NOT seen
its contents; do not guess.]
```

The boundary falls exactly where declared, and **all twelve survive in the prompt** — nothing was
truncated away.

The driver's matcher self-test carries the same third assertion and passed all three, including
*"the OLD BROKEN matcher (source-transcribed, newline inside the phrase) MISSES the real notice"* —
the hazard being that my Rust literal uses `\` line-continuations, so a phrase copied straight out
of the source carries a newline and an indent the runtime string does not have.

### 4d. Suites and lints at merged HEAD

`cargo test` per crate, all `0 ignored; 0 filtered out`:

| crate | passed | | crate | passed |
|---|---|---|---|---|
| wcore-channels | 114 + 17 + 6 | | wcore-channel-whatsapp | 38 |
| wcore-channel-discord | 58 | | wcore-channel-sms | 26 + 2 |
| wcore-channel-email | 83 + 1 | | wcore-channel-signal | 41 |
| wcore-channel-matrix | 36 | | **wcore-channel-msteams** | **38** |
| wcore-channel-telegram | 66 | | wcore-channel-slack | 45 |

**571 passed, 0 failed, 0 ignored.** msteams — the new production consumer — is green, so this
lane does not break it.

`cargo clippy --all-targets` over all 11 crates plus `wcore-agent`: **clean** (only a pre-existing
`imap-proto` dependency future-incompat notice). `cargo fmt --all -- --check`: clean.
`cargo build -p wcore-cli`: the real binary links.

### 4e. iMessage — the Darwin exception, used and disclosed

`wcore-channel-imessage` is `#[cfg(target_os = "macos")]`. On hetzner it compiles to an empty shell
and reports `0 passed; 0 filtered out` — **zero tests exist**, so hetzner cannot prove my change to
it at all. That is the narrow sanctioned exception, so I ran **one crate, on the Mac**:
`cargo test -p wcore-channel-imessage --lib` → **25 passed; 0 failed; 0 ignored; 0 filtered out**.
No workspace build, no clippy, no release build.

### 4f. The 21 `wcore-agent` failures are pre-existing AND flaky — measured, not assumed

`wcore-agent --lib` shows 21 failures at my HEAD. LANE-BRIEF §6 says re-run in isolation before
reporting a regression, so I did better than that and ran the **unchanged baseline `35243f36`
twice**:

| run, same unchanged commit | failures |
|---|---|
| baseline run 1 | **22** |
| baseline run 2 | **18** |

The sets differ **in both directions** between two runs of one commit. These are load-sensitive
flakes in `channel_lease` / `engine::audit_2026_05_22` / `session` / `orchestration` — the known
contention artifact — and **none touches media**. My media tests: **14/14 in isolation**. This lane
introduces no failure.

---

## 5. Instrument defects found in MY OWN instruments — two, both repaired in-lane

LANE-BRIEF §6b-ii: a written-up instrument defect is a defect you have agreed to keep.

**#1 — a blame sweep that returned eight empty results.** Read naively that is "no provenance
available", which would have let me leave the panel's objection unresolved and proceed. Cause: **zsh
does not word-split unquoted variables**, so `set -- $spec` put the whole `"path 370"` string in `$1`
and left `$2` empty, making every `-L ,` malformed; `git blame` printed nothing and the loop
swallowed it. Repaired with a function taking two real arguments, verified against a direct single
invocation that returns a line. This is §3b-i happening live: **the empty result would have confirmed
an absence for free.**

**#2 — my own test suite emitted 125 MB on failure.** Found by the first mutation run. `expect_err`
on a `Result<Vec<u8>, _>` renders the **entire vector** into the panic message, and the
default-bound case has a 26 MiB Ok payload; every other test's result was buried. A failure I cannot
read is one I would have had to re-run blind, and over a slow link it looks like a hung agent
(§6b). Repaired, not noted: every `expect_err`/`unwrap_err` on a byte payload now goes through
`.map(|b| b.len())` first. The second mutation run — same mutation, readable output — is the proof
the repair works.

---

## 6. What I did NOT do

- **Did not claim `24-C3`.** Eighth lane to decline it, correctly.
- **Did not claim the `media` clause is MET.** I repaired an advertised-but-dead API. The criterion
  asks for media that works end to end; **no description or transcript is produced by this lane**,
  and the live-vision leg remains blocked exactly as `24-MEDIA-ACTIONS` §6 records. Grading a
  bounds repair as the media clause would be the same costume-change this program keeps catching.
- **Did not change any adapter's effective runtime limit.** Every declared value equals the cap
  that adapter already enforced. Whether discord's intake *should* be 25 rather than 100 is a
  product policy decision with its own evidence requirement — and it is now a one-line edit in one
  place, which is the point.
- **Did not rebase**, though the orchestrator asked for it: LANE-BRIEF §0 forbids `git rebase`
  because lanes share the object store. I **merged `35243f36` into my lane** instead, which puts my
  work on the current tree without the forbidden operation. Clean merge, no conflicts. If the
  orchestrator wants a linear history it should do that at integration time.
- **Did not run `wcore-contract generate`.** Verified none is owed: `MediaBounds` appears **0** times
  in `crates/wcore-protocol/contracts/`, with a live known-positive control (`session` →
  `session_resync.genesis.json:1`) proving that search alive. My first control attempt for this was
  **dead** — `max_message_len` over non-`.rs` files returned empty, so it could not have
  distinguished absence from a broken search — and I replaced it rather than reporting off it.
  I also added **no field** to `MediaBounds`, so the schema is untouched regardless.
- **Did not touch the shared-file fence.** `crates/wcore-cli/src/{lib,main}.rs`: empty diff vs base.
- **Did not run a full workspace test suite.** Under other lanes' load that is not a measurement.
- **Did not use any credential.** Nothing in this lane needs one. Spend: zero.
- **Did not fix `msteams`' undeclared bound** — it has none to fix: no `fetch_media`, and Bot
  Framework reports no size, so the byte bound cannot bind there. Its count bound works, via both
  its own `normalize_all` call and my enricher.

## 7. Open

- **`max_bytes` has no live leg.** The count bound is live-proven; the byte bound is proven by
  mutation and unit/integration test only. Driving it live needs a media backend configured so the
  enricher actually fetches — reachable with **zero spend** via the transcription route
  `24-MEDIA-ACTIONS` §6 documents. ~0.5 session.
- **`normalize` (declaration-time, platform-reported size) still has only msteams as a consumer.**
  The other adapters enforce at fetch time only. Not wrong — an unreported size is not a small file,
  which is why fetch-time enforcement is the load-bearing half — but the two halves are now
  asymmetric across adapters and that is worth a deliberate decision.

## 8. Evidence

`.planning/phases/24-gateway-automation-channels-typed-api/24-MEDIA-BOUNDS-evidence/`

| file | bytes | what |
|---|---|---|
| `24-MEDIA-BOUNDS-NOTES.md` | — | append-only record, first committed at T+13 before any implementation |
| `run1-summary.json` | 3361 | live run 1, three gates, `all_pass=true` |
| `run2-summary.json` | 3361 | live run 2, reproducibility, `all_pass=true`, distinct `shasum` |
| `run1-A-turn-prompt.txt` | 3261 | the real binary's turn prompt, 12 attachments, bound at 10 |
| `run1-B-control-turn-prompt.txt` | 924 | negative control, 3 attachments, notice absent |

Byte counts via `/usr/bin/stat -f%z` — not `wc`, which this program measured returning 0 for a
72-byte file. Re-run with:

```bash
python3 scripts/f24-media-count-bound-derive.py
node scripts/f24-media-count-bound.mjs --binary <wayland-core> --out <dir>
```
