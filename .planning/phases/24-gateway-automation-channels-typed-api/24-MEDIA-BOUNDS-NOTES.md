# 24-MEDIA-BOUNDS — working notes (append-only)

Lane `24-media-bounds`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24-media-bounds`,
branch `lane/24-media-bounds`, merge-base `15cda12d`.

Committed first, per LANE-BRIEF §6b-i. Appended after every measurement.

---

## T+0 — assignment

Handed finding: "declared media limits are decorative". `media_bounds()` declared on the
`Channel` trait, overridden by 2 adapters, read at exactly one site and that site is a test.
Where a cap IS enforced it is an unrelated hardcoded constant: discord declares 25 MiB /
enforces 100; email declares 10 / enforces 2.

Assignment explicitly says findings handed to lanes on this program have been wrong in both
directions. So step 1 is re-measurement, not action.

---

## T+5 — RE-MEASUREMENT 1: the one-read claim. HOLDS.

Instrument: `/usr/bin/grep` (unproxied — `rtk` rewrites `grep`, LANE-BRIEF §3b).
Glob quoted (`--include='*.rs'`) — zsh eats it unquoted, and that produced a free
false-negative for a prior lane (§3b-i defect 1).

**Known-positive control, same invocation, same tool, same paths:** `max_message_len`
→ **24 hits**. Instrument is alive. A zero from this tool would now be meaningful.

```
/usr/bin/grep -rn "media_bounds" crates --include='*.rs'
crates/wcore-channel-email/src/lib.rs:536:    fn media_bounds(...)          <- DECLARES
crates/wcore-channels/tests/framework_matrix.rs:156: fn media_bounds(...)   <- test impl
crates/wcore-channels/tests/framework_matrix.rs:373: let bounds = ch.media_bounds();  <- ONLY READ
crates/wcore-channels/src/lib.rs:168:  fn media_bounds(...)                 <- trait default
crates/wcore-channel-discord/src/lib.rs:405: fn media_bounds(...)           <- DECLARES
```

5 sites. 4 definitions, 1 read, and the read is in `tests/`. **Claim CONFIRMED.**

Concept search (not keyword — §3b-i rule 3): also swept `MediaBounds` (the type),
`max_attachments`, `normalize`/`normalize_all`, and
`max_(media|attachment|inline|file|download|body|blob|image|audio)[a-z_]*` case-insensitive
across all of `crates`. No production consumer of the declared bound appears under any
other name.

## T+8 — RE-MEASUREMENT 2: the two divergence numbers. BOTH HOLD.

| adapter | declares | enforces | file:line | ratio |
|---|---|---|---|---|
| discord | `25 * 1024 * 1024` (`discord/src/lib.rs:407`) | `100 * 1024 * 1024` (`discord/src/rest.rs:370` `MAX_MEDIA_BYTES`) | | **4.0× larger** |
| email | `10 * 1024 * 1024` (`email/src/lib.rs:538`) | `2 * 1024 * 1024` (`email/src/imap.rs:619` `MAX_INLINE_ATTACHMENT_BYTES`) | | **5.0× smaller** |

Both numbers read from source, both confirmed. Both directions of error are present exactly
as reported.

## T+10 — THE FINDING WAS UNDERSTATED. Seven more adapters, all undeclared.

Assignment item 4 asked whether any *other* adapter enforces an undeclared cap. **Yes — seven.**
Every one of these enforces a hardcoded cap while declaring nothing, so it inherits the trait
default of **25 MiB** (`MediaBounds::DEFAULT_MAX_BYTES`, `media.rs:40`) and diverges from it:

| adapter | enforced | file:line | vs inherited 25 MiB default |
|---|---|---|---|
| matrix | 100 MiB | `matrix/src/rest.rs:100` | 4.0× larger |
| slack | 100 MiB | `slack/src/api.rs:354` | 4.0× larger |
| telegram | 100 MiB | `telegram/src/api.rs:916` | 4.0× larger |
| whatsapp | 100 MiB | `whatsapp/src/api.rs:619` | 4.0× larger |
| imessage | 64 MiB | `imessage/src/channel.rs:37` | 2.56× larger |
| signal | 64 MiB | `signal/src/lib.rs:116` | 2.56× larger |
| sms | 16 MiB | `sms/src/api.rs:29` | 0.64× — SMALLER |

So the true scope is **9 adapters diverging, not 2**. Of the adapters that enforce anything,
**zero** enforce their declared bound. `max_attachments` is enforced nowhere at all (the only
non-definition use is inside dead `normalize_all`).

## T+12 — what "making it load-bearing" has to mean

`normalize`/`normalize_all` (`media.rs:150,193`) are the declared enforcement point — the trait
doc at `channels/src/lib.rs:166-167` literally says "Enforced by `media::normalize`". They have
no production caller. But note `normalize` only compares `raw.size_bytes` — a *platform-reported*
size — and its own doc (`media.rs:147-149`) says the caller must enforce `max_bytes` at fetch
time because an unreported size is not evidence of a small file.

So there are two enforcement points and they are NOT interchangeable:
- **declaration-time** (`normalize`): platform-reported size, before any fetch;
- **fetch-time** (`read_body_capped` and friends): actual bytes on the wire.

The fetch-time cap is the one that actually protects memory, and it is the one currently
hardcoded. **Routing the fetch-time cap through `self.media_bounds().max_bytes` is the change
that makes the declaration load-bearing.** Doing only the `normalize` half would leave the
real cap still hardcoded.

NEXT: read every `fetch_media` impl to see whether `&self` is in scope at the cap site
(several caps are in free functions in `rest.rs`/`api.rs`, which would need the cap threaded
as a parameter).

Open: whether to also delete/reconcile the hardcoded constants, or keep them as the
*declared* value's source. Leaning: the declaration becomes the single source, constants die.

## T+20 — choke point found, and it is single

`wcore-agent/src/channel_media.rs:106` → `ChannelManager::fetch_media_on`
(`manager.rs:774-785`) → `guard.fetch_media(attachment)`. That is the **only** production
path to adapter media. So there is exactly one central site where a declared bound could be
enforced for all adapters at once, and it already has `guard` — i.e. `media_bounds()` is in
scope there. It simply is not called.

## T+22 — contract exposure: NONE. Checked with a live control.

First control I tried was dead: `grep max_message_len` over non-`.rs` files returned empty,
so it could not have distinguished "absent" from "broken search". Replaced with a control I
could verify: `grep -rlc session crates/wcore-protocol/contracts/` →
`session_resync.genesis.json:1`. Instrument alive.

`grep -rn "MediaBounds|media_bounds|max_attachments" crates/wcore-protocol/contracts/` → **0**.

`MediaBounds` is `Serialize`/`Deserialize` but does not appear in any Desktop wire-contract
fixture. **No contract regeneration is owed by this lane** (LANE-BRIEF §0 fence). I am also
adding NO field to `MediaBounds`, only changing declared values and adding enforcement, so
the schema is untouched regardless.

## T+25 — the design decision, and why it went the way it did

The trap: "make enforcement match the declaration" and "make the declaration match
enforcement" are both defensible and they produce opposite runtime behaviour.

- **Enforcement follows declaration** would drop discord's fetch cap 100 MiB → 25 MiB. Real
  regression: boosted/Nitro Discord uploads legitimately exceed 25 MiB and would start
  degrading. I cannot validate that against a live boosted server, and inventing product
  policy I cannot test is out of scope for a divergence fix.
- **Declaration follows enforcement** relabels 100 MiB as declared. Feels like loosening.

Resolved by reading what the field actually means rather than what the comments imply.
`MediaBounds.max_bytes` doc (`media.rs:30-31`): *"Largest attachment this adapter will
normalise for **fetch**"*. That is an **intake policy**, not a platform upload ceiling.
Discord's `25 MiB` comment — *"Discord's per-attachment ceiling for a non-boosted upload"* —
records a **platform fact in a field that means an intake policy**. The declaration was
simply wrong about its own field's semantics.

So the fix is not a loosening: **the declarations are corrected to state the intake policy
each adapter has always had**, and enforcement is then wired to read them so they cannot
drift again. Zero effective runtime change; the divergence closes because the numbers become
true.

Same reasoning for email in the opposite direction: the operative intake cap genuinely IS
2 MiB (that is what the IMAP parser inlines, `imap.rs:847`); the declared 10 MiB was never
real. Declaration drops 10 → 2.

Next: cross-audit this before implementing (§4), because it is the lane's central call.

## T+35 — cross-audit panel: UNANIMOUS B (3/3), one shared objection, then FALSIFIED

| auditor | vote |
|---|---|
| codex `gpt-5.6-sol` | `PANEL_POSITION=B` — "B is contract repair; A is an undocumented policy change disguised as consistency" |
| gemini 3.1 pro | `PANEL_POSITION=B` — "aligning the declaration to established runtime reality prevents capability regressions" |
| kimi K3 | `PANEL_POSITION=B` — "A is the riskier option dressed up as the stricter one" |

All three independently raised the **same** objection, which is the only one that mattered:
*B assumes today's enforcement numbers are intentional, and you have not checked.* Kimi named
the decisive test — **blame the enforcement constants, not just the declaration.**

### Instrument defect #1 (mine), caught before it produced a false result

First blame sweep returned **eight empty results**. Read naively that is "no provenance
available", which would have let me proceed with the objection unresolved. Cause: zsh does
**not** word-split unquoted variables, so `set -- $spec` put the whole `"path 370"` string in
`$1` and left `$2` empty, making every `-L ,` malformed. `git blame` printed nothing and the
loop swallowed it. Repaired with a function taking two real arguments; verified against a
direct single invocation that returns a line.

### The result — the objection is FALSIFIED, and the causality is the reverse

| line | commit | date | author |
|---|---|---|---|
| discord `MAX_MEDIA_BYTES` 100 MiB | `a1085393e` | **2026-06-12** | Sean Donahoe |
| telegram `MAX_MEDIA_BYTES` 100 MiB | `a1085393e` | **2026-06-12** | Sean Donahoe |
| slack `MAX_MEDIA_BYTES` 100 MiB | `a1085393e` | **2026-06-12** | Sean Donahoe |
| sms `MAX_MEDIA_BYTES` 16 MiB | `f638d68f5` | **2026-06-12** | Sean Donahoe |
| signal `MAX_ATTACHMENT_BYTES` 64 MiB | `da6a3c62a` | **2026-06-12** | Sean Donahoe |
| imessage `MAX_ATTACHMENT_BYTES` 64 MiB | `16d09fdba` | **2026-06-12** | Sean Donahoe |
| email `MAX_INLINE_ATTACHMENT_BYTES` 2 MiB | `ce6e88e99` | **2026-06-12** | Sean Donahoe |
| matrix `MAX_MEDIA_BYTES` 100 MiB | `8273b2ac1` | **2026-06-18** | Sean Donahoe |
| — | | | |
| discord **declaration** 25 MiB | `9b06a4778` | **2026-07-27** | ci |
| email **declaration** 10 MiB | `9b06a4778` | **2026-07-27** | ci |

`crates/wcore-channels/src/media.rs` — the entire `MediaBounds` module — was **added
2026-07-27** (`de0367b0`, *"feat(24-03): complete the channel framework contract"*), and both
adapter declarations the same day (`9b06a4778`, *"feat(24-03): reference-adapter probes and
the shipped channel verbs"*).

**The enforcement predates the declaration by six weeks, across six separate commits, each
written next to the download path it guards. The declaration is the late artifact, authored
in one sweep, and never wired to anything.**

So the panel's worry — that B ratifies an accidental 100 MiB over a considered 25 MiB — is
backwards. The 100 MiB values ARE the considered, per-adapter, original engineering. The
25 MiB is the paperwork. By kimi's own stated criterion ("if blame shows 100 MiB was
deliberate, B is unambiguously right"), **B is unambiguously right.**

This also sharpens the finding itself: the bounds API was **advertised-but-dead from birth**.
It was not a consumer that rotted away — there was never a consumer. It shipped as contract
documentation for enforcement that already existed elsewhere and was never connected to it.

### Adopted mitigations for the objection anyway (it was a good objection)

1. `MediaBounds::DEFAULT_MAX_BYTES` stays at the conservative **25 MiB**, so a NEW adapter
   that declares nothing still inherits the tight value. Only adapters with a measured,
   already-enforced cap declare a larger one.
2. Discord's platform fact ("25 MiB non-boosted upload ceiling") is **preserved in the doc
   comment** rather than deleted, so the information the 25 survives even though it is no
   longer masquerading as the intake bound.

## T+70 — implemented, and the third assertion is PROVEN by mutation

Shape: **one `MEDIA_BOUNDS` constant per adapter crate.** `media_bounds()` returns it and the
crate's own fetch/inline cap is derived from it, so the advertised number and the enforced
number are the same number by construction. Plus `ChannelManager::fetch_media_on` — the only
production path to adapter media — checks every payload against the originating channel's
declaration, and `ChannelMediaEnricher::enrich` applies `max_attachments`, which previously had
no enforcement point anywhere in the workspace.

Declared values are set to each adapter's already-operative cap: **no adapter's effective
runtime limit changes.**

### Build (hetzner `hz/24-media-bounds` @ `dd14ebe8`)

All 10 touched crates compile: `wcore-channels`, `wcore-agent` deps, and the 9 adapters.

### Suite `wcore-channels --test media_bounds_enforced` — 6 tests

```
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

`0 ignored` and `0 filtered out` read back explicitly (LANE-BRIEF §3.2 / §3b) — the suite ran
6 tests, it did not exit 0 having run none. Output taken via `rtk proxy`, because plain `cargo`
in this environment **strips those exact two fields**.

### MUTATION PROOF — the third assertion, executed not asserted

Reverted `fetch_media_on` to return before any bounds check (the pre-fix two-line body), at the
identical commit, with an `assert` on the mutation anchor so a silent no-op revert could not
produce a fake proof:

| build | result |
|---|---|
| **pre-fix enforcement** | `FAILED. 1 passed; 5 failed; 0 ignored; 0 filtered out` |
| **fixed enforcement** (restored, control) | `ok. 6 passed; 0 failed; 0 ignored; 0 filtered out` |

**The one test that passes pre-fix is `a_payload_at_exactly_the_declared_bound_is_returned_intact`
— the known-positive.** That is exactly correct: the liveness control must pass on both builds,
and all five behavioural assertions must redden. If the known-positive had ALSO failed, the
suite would have been measuring a broken harness rather than the enforcement.

### Instrument defect #2 (mine) — found by the mutation run, repaired in-lane (§6b-ii)

The first mutation run produced **125 MB of output**. Cause: `expect_err` on a
`Result<Vec<u8>, _>` renders the ENTIRE vector into the panic message, and the default-bound
case has a 26 MiB Ok payload. Every other test's result was buried — a failure I could not
read is a failure I would have had to re-run blind, and on a slower link it would have looked
like a hung agent (§6b).

Repaired, not merely noted: every `expect_err`/`unwrap_err` on a byte-payload result now goes
through `.map(|b| b.len())` first, so a failure prints a length. Commit `dd14ebe8`. The second
mutation run above is the proof the repair works — same mutation, readable output.
