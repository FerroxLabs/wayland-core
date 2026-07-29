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
