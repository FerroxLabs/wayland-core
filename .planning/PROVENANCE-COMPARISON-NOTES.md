# PROVENANCE COMPARISON — working notes (lane/provenance-comparison)

Started 2026-07-30. Base commit `57a41c7dcf2dcec63d1f631e9b8fbbefd01c6cfa`
(`revert(scrub): restore nine copyright attributions`).

Deliverable: `.planning/PROVENANCE-COMPARISON.md`. This lane changes NO source file.

## Instrument discipline in force

Per LANE-BRIEF §3b: every number that will appear in the report is produced by
redirecting an unproxied tool (`/usr/bin/git`, `/usr/bin/grep`) to a file and
reading the file with the Read tool. Nothing load-bearing is read from Bash stdout.

## Peer baseline assertion (done first — §3b-i)

| repo | pinned baseline | working-tree HEAD | pinned commit present in object store? |
|------|-----------------|-------------------|----------------------------------------|
| `resources/openclaw` | `11a0ad10` (2026-06-16, "test: make install-safe-path symlink tests compatible with Windows") | `3659c85e` (2026-07-18) | YES — `git cat-file -t` → `commit` |
| `resources/hermes-agent` | `dbe734be` (2026-06-27) | `d59b79fa` (2026-07-17) | YES — `git cat-file -t` → `commit` |

**The working trees are NOT at the pinned baselines** — both are ~1 month ahead.
Reading the checked-out files would compare against the wrong version. Mitigation:
all peer source is extracted with `git show <pinned-sha>:<path> > file` and read
from the file, never from the working tree. Every peer excerpt in the final report
is therefore at the pinned baseline, and I say so per-quote.

## Sites under examination (9)

1. `crates/wcore-providers/src/failover.rs:1` — FailoverReason taxonomy
2. `crates/wcore-providers/src/key_rotation.rs:1` — API key rotation pool
3. `crates/wcore-providers/src/classify.rs:1` — 3-tier failover classifier
4. `crates/wcore-providers/src/cache_observation.rs:1` — cache retention forensics
5. `crates/wcore-pricing/src/refresh.rs:1` — self-healing pricing layer
6. `crates/wcore-providers/src/retry.rs:738` — "Source: openclaw"
7. `crates/wcore-providers/src/anthropic.rs:307` — moving-breakpoint cache layout
8. `crates/wcore-channel-imessage/src/lib.rs:16` — via Wayland Desktop TS
9. `crates/wcore-channel-msteams/src/lib.rs:15` — via Wayland Desktop TS

## Control (required)

At least two `wcore-providers`/`wcore-pricing` modules with NO claimed peer
counterpart, run through the identical method, to calibrate what "independently
written Rust in this codebase" scores. Plus a deliberately-close pairing to prove
the method can FIND similarity (§3b-iii both-directions rule).

## MEASUREMENT 1 — Site 1 `failover.rs`, the FailoverReason taxonomy

Instrument alive: `git grep -c -l 'export' 11a0ad10 -- '*.ts'` → **10272 files**
(known-positive). Tree at `11a0ad10` has **20082** files.

Peer counterpart: **EXISTS.** `src/agents/embedded-agent-helpers/types.ts:5` holds
the `FailoverReason` string-union; `src/agents/failover-error.ts` holds the
`FailoverError` class. Both extracted at `11a0ad10` via `git show`.

### The two taxonomies side by side

| # | OpenClaw `FailoverReason` (14) | ours `FailoverReason` (11) |
|---|-------------------------------|-----------------------------|
| 1 | `auth` | `Auth` → `auth` |
| 2 | `auth_permanent` | `AuthPermanent` → `auth_permanent` |
| 3 | `format` | `Format` → `format` |
| 4 | `rate_limit` | `RateLimit` → `rate_limit` |
| 5 | `overloaded` | `Overloaded` → `overloaded` |
| 6 | `billing` | `Billing` → `billing` |
| 7 | `server_error` | — (absent) |
| 8 | `timeout` | `Timeout` → `timeout` |
| 9 | `model_not_found` | `ModelNotFound` → `model_not_found` |
| 10 | `session_expired` | `SessionExpired` → `session_expired` |
| 11 | `empty_response` | — (absent) |
| 12 | `no_error_details` | — (absent) |
| 13 | `unclassified` | — (absent) |
| — | — | `ContextOverflow` → `context_overflow` (ours only) |
| 14 | `unknown` | `Unknown` → `unknown` |

**Ten of our eleven variants are OpenClaw variants, with identical wire strings,
in identical relative order.** Delete OpenClaw's four (`server_error`,
`empty_response`, `no_error_details`, `unclassified`) and the remaining sequence is
our sequence exactly, `unknown` last in both. The ordering follows no external
principle — not alphabetical, not severity-ranked, not status-code-ranked — so it is
arbitrary, and arbitrary order matching across 10 elements is the classic
selection-and-arrangement signal.

**The header is corroborated by the code's own doc comment**, `failover.rs:16-17`:
"String representations match openclaw's TS string-union for cross-language
log/telemetry compatibility." That is a contemporaneous statement of deliberate
matching, not a stray note.

**Against over-reading it:** the individual words (`auth`, `timeout`, `rate_limit`)
are the ordinary functional names for those conditions and carry nothing on their
own. Whether a 10-item vocabulary list clears the originality threshold for a
compilation is a legal judgement, not a technical one. What is technical and
certain: the *selection* and the *order* match, and our own comment says that was
on purpose.

**Structure/SSO, by contrast, does NOT match.** OpenClaw's `failover-error.ts` is
714 lines of recursive cause-chain walking (`MAX_FAILOVER_CAUSE_DEPTH = 25`, cycle-
detecting `seen: Set<object>`, `findErrorProperty`, nested-format override logic).
Our `failover.rs` is 339 lines of which ~180 are tests; the non-test body is a
plain enum + a builder-pattern struct with `with_model`/`with_status`/`with_code`
and a `std::error::Error::source` impl. **No shared function decomposition, no
shared control flow, no shared constants.** `resolveFailoverStatus`'s reason→HTTP
map (402/500/429/503/401/403/408/400/404/410) has **no counterpart in our file at
all**.

## Status log

- [x] worktree created, toplevel asserted
- [x] peer baselines asserted (both stale — mitigated by `git show` at pinned sha)
- [x] site 1 (failover.rs) measured — taxonomy MATCHES, structure does NOT
- [ ] our nine sites read
- [ ] peer counterpart search
- [ ] per-site comparison
- [ ] control modules
- [ ] Wayland Desktop TS chain check (sites 8, 9)
- [ ] report written
