# PROVENANCE COMPARISON — the nine attribution notices

**Lane:** `lane/provenance-comparison` · **Base:** `57a41c7d` · **Date:** 2026-07-30

**What this document is.** Nine source comments in this repository state that code was
"ported from" OpenClaw (and in two cases, from Wayland Desktop's TypeScript, itself
said to be OpenClaw-derived). Each names Peter Steinberger as copyright holder. An
external audit read these as admissions that third-party code was taken. This
document establishes, per site, **what actually carried over**.

**What this document is NOT.** It reaches no legal conclusion. It does not say
whether anything here is or is not infringement. That is a lawyer's call, and
several of the findings below are precisely the kind a lawyer needs to weigh rather
than have pre-weighed. Where the evidence is genuinely two-sided I say so and put
the site in the "needs a human" bucket rather than resolving it in the direction
that flatters us.

---

## 0. Summary for a reader coming in cold

Nine notices. Measured against the peers at their pinned baselines, using a method
calibrated in both directions against controls:

- **Five notices are wrong and should be removed.** The code at those sites is not a
  port. In three of the five, the source comment actively **misdescribes** the peer —
  it attributes to OpenClaw a behaviour OpenClaw does not have. You do not get the
  other system's semantics wrong while transcribing from it; getting them wrong is
  evidence of writing from a half-remembered reading, not from the source.
- **One notice is accurate and narrow, and it should stay.** The `FailoverReason`
  taxonomy really does reproduce OpenClaw's vocabulary — same names, same wire
  strings, **same arbitrary order** — and our own code says it was matched on
  purpose. The *code* around it is unrelated to OpenClaw's; it is the word-list that
  carried.
- **Two notices (the channel crates) describe a real, documented derivation chain**,
  and the cheap resolution hoped for did not materialise — see §4.
- **One is genuinely unclear** and should go to a human.

**The single most important finding for a lawyer:** across all nine sites there is
**not one distinctive, non-dictated literal in common** — no copied comment, no
invented identifier, no shared magic constant — with two narrow exceptions, both
compounds of externally-fixed parts (§3.7, §3.5). Everything else that matches is
either the failover vocabulary (§3.1) or a string the provider's own API dictates.

**Bucket tally: (a) independent, strip — 5 · (b) derived, keep — 3 · (c) needs a human — 1.**

---

## 1. Method, and why you should believe the numbers

### 1.1 Peer baselines — the checkouts were at the WRONG commits

The brief pinned OpenClaw at `11a0ad10` and Hermes at `dbe734be`. Both working trees
were roughly a month ahead:

| repo | pinned baseline | working-tree HEAD when I started |
|------|-----------------|----------------------------------|
| `resources/openclaw` | `11a0ad10` (2026-06-16) | `3659c85e` (2026-07-18) |
| `resources/hermes-agent` | `dbe734be` (2026-06-27) | `d59b79fa` (2026-07-17) |

Reading the checked-out files would have compared against the wrong version. Both
pinned commits were confirmed present in the object stores (`git cat-file -t` →
`commit`), so **every peer excerpt below was extracted with
`git show <pinned-sha>:<path>`**, never from the working tree. Peer trees were read
only; nothing was mutated, built, or executed.

### 1.2 Instrument discipline

`rtk` proxies and silently re-renders shell tool output on this machine, including
machine-readable counts. Every number in this document was produced by an unproxied
absolute-path tool (`/usr/bin/git`, `/usr/bin/grep`) **redirected to a file and read
back with a file reader**, never from a shell's stdout.

Every absence claim below carries a **known-positive in the same invocation** to
prove the instrument was alive, and a known-negative to prove it could return zero.
Where a concept could be named several ways, I searched the concept under multiple
names rather than one keyword — this caught one of my own errors (§4.2).

### 1.3 The similarity metric, and its controls

`litcmp.py` (committed alongside this report): extract every quoted literal of
length ≥ 5 from each file, lowercase, drop punctuation-only, report the intersection
and Jaccard. Applied **identically** to every pairing.

A metric with no baseline is worthless, so it was calibrated in both directions.

**Can it find similarity?** Two pairs of our own provider adapters that are
evidently template copies of one another inside this repo:

| positive control | shared | Jaccard |
|------------------|--------|---------|
| `cerebras.rs` vs `moonshot.rs` (both ours) | 11 | **0.3438** |
| `deepseek.rs` vs `moonshot.rs` (both ours) | 13 | **0.2364** |

**Can it find none?** Modules of ours with no claimed counterpart, run against the
peer modules the sites point at:

| negative control | shared | Jaccard |
|------------------|--------|---------|
| `cooldown.rs` vs `errors.ts` | 0 | 0.0000 |
| `cooldown.rs` vs `failover-error.ts` | 0 | 0.0000 |
| `paste_detect.rs` vs `errors.ts` | 1 | 0.0037 |
| `fingerprint.rs` vs `errors.ts` | 2 | 0.0058 |
| `failover_policy.rs` vs `failover-policy.ts` — *same name, same domain, no header* | 1 | 0.0435 |

So: **genuine copying scores 0.24–0.34 on this metric. Independently-written Rust in
this codebase scores 0.000–0.043 against a peer module.** Those two bands are the
yardstick for everything in §3. The last negative control is the most useful one —
`failover_policy.rs` carries no attribution header, sits in the same crate, has the
*same filename* as an OpenClaw module in the same problem domain, and still scores
0.0435. Same name and same job do not produce similarity here.

**Where the metric fails, and I say so.** For the two channel crates (§4) the metric
cannot discriminate, because a Bot Framework or iMessage client is almost entirely
vendor wire constants. Even the hop that is *documented and self-attributed as
derived* scores low on it. I do not rely on it there.

### 1.4 The brief's premise, verified

`grep -r -i 'Steinberger' crates/` → **6 hits** (instrument alive: 925 files match
`pub fn`; gibberish needle returns 0). Those six plus `anthropic.rs:307` and the two
channel crates make **nine**. The premise holds.

One site the brief does not list turned up in the same sweep and belongs with the
channel crates: `crates/wcore-channels-registry/src/lib.rs:55` — *"F-045 (W7-M): new
channel adapters ported from desktop OpenClaw fork."* It names no copyright holder
but asserts the same chain as sites 8–9.

---

## 2. Per-site verdict table

Bucket key: **(a)** independent — the header is wrong, strip it · **(b)** derived —
keep attribution, consolidate into `THIRD-PARTY-NOTICES.md` · **(c)** genuinely
unclear — needs a human.

| # | Site | Counterpart exists? | Idea or expression? | Key evidence | Confidence | Bucket |
|---|------|--------------------|--------------------|--------------|-----------|--------|
| 1 | `wcore-providers/src/failover.rs:1` | **Yes** — `embedded-agent-helpers/types.ts:5` | **Expression, narrowly**: the taxonomy's selection + arrangement. Nothing else. | 10 of our 11 variants are OpenClaw's, identical wire strings, **identical relative order**; our own doc comment says the match is deliberate. Surrounding code shares no structure. | High | **(b)** |
| 2 | `wcore-providers/src/key_rotation.rs:1` | Yes — `api-key-rotation.ts` | **Neither.** Architecturally opposite. | OpenClaw: stateless wrapper owning the retry loop, no memory. Ours: stateful pool with per-key cooldown timestamps, last-good stickiness, round-robin cursor; owns no loop. **0 shared literals.** OpenClaw has no "pool" at all. | High | **(a)** |
| 3 | `wcore-providers/src/classify.rs:1` | Yes — `errors.ts` | **Idea only.** | Tier precedence differs; body-arm order differs (1 of 11 coincides); status map differs at 403/503/413. **Our comment misdescribes OpenClaw's 503 semantics.** All 29 shared literals are vendor-dictated or the site-1 taxonomy. | High | **(a)** |
| 4 | `wcore-providers/src/cache_observation.rs:1` | Partial — retention type exists; the module's bulk does not | **Neither.** | `ContextEnginePromptCacheRetention` is `"none"\|"short"\|"long"\|"in_memory"\|"24h"`; ours is `"5m"\|"1h"\|"none"` — **one value in common**, and our comment claiming a match is false. `InvalidationCause`: **17 concept probes, zero hits.** 0 shared literals. | High | **(a)** |
| 5 | `wcore-pricing/src/refresh.rs:1` | Yes — `model-pricing-cache.ts` | **Idea only.** | Shared: OpenRouter's published endpoint (dictated), a 24 h TTL (conventional), and the constant *name* for that URL. Different purpose: theirs caches prices, ours diffs bundled-vs-live and emits `CatalogChange` for human review — no counterpart. Jaccard 0.0194, inside the negative band. | High | **(a)** |
| 6 | `wcore-providers/src/retry.rs:738` | Yes — and **named precisely**: `retry-policy.ts` → `getChannelApiRetryAfterMs` | **Idea only.** | Theirs: 3 probe shapes, field `retry_after` only, ×1000, **no cap**, for Telegram. Ours: 5 shapes incl. `retry_after_ms`, `body.*`, HTTP header, a 300 s cap, NaN/∞ rejection, for LLM providers. Jaccard 0.0226 **against its own named source** — inside the negative band. Our file cites RFC 9110/7231. | High | **(a)** |
| 7 | `wcore-providers/src/anthropic.rs:307` | OpenClaw yes; Hermes has an unrelated 79-line module | **Split — see §3.7.** | **Exact identifier match: `ANTHROPIC_CACHE_CONTROL_LIMIT = 4`** in both. But both halves are dictated (Anthropic's published hard limit; obvious name). The thing the header actually claims — the "moving-breakpoint layout" — returns **zero hits in both peers**. | Medium | **(c)** |
| 8 | `wcore-channel-imessage/src/lib.rs:16` | Yes, via a **documented** chain | **Upstream hop real; second hop unestablished — see §4.** | Desktop's own `MANIFEST-imessage.md` says "OpenClaw harvest" with per-file HARVEST/SKIP dispositions; Desktop mandates and carries a Steinberger MIT header + `LICENSES/openclaw.txt`. Our Rust↔Desktop shared literals are all macOS-dictated. | Medium | **(b)** |
| 9 | `wcore-channel-msteams/src/lib.rs:15` | Yes, via the same chain | **Same as 8.** | Desktop's `MsTeamsAdapter.ts` explicitly cites a carried constant (`textChunkLimit=4000`); **that constant did not survive into our Rust.** The one non-dictated convention we share with Desktop (`{serviceUrl}\|{conversationId}`) has **zero hits in OpenClaw** — it is Desktop's invention. | Medium | **(b)** |

---

## 3. Site-by-site evidence

### 3.1 Site 1 — `failover.rs`, the `FailoverReason` taxonomy — **(b) keep**

This is the one site where protected-expression-shaped material demonstrably carried.

OpenClaw at `11a0ad10`, `src/agents/embedded-agent-helpers/types.ts:5`:

```ts
export type FailoverReason =
  | "auth" | "auth_permanent" | "format" | "rate_limit" | "overloaded"
  | "billing" | "server_error" | "timeout" | "model_not_found"
  | "session_expired" | "empty_response" | "no_error_details"
  | "unclassified" | "unknown";
```

Ours, `failover.rs:20`: `Auth, AuthPermanent, Format, RateLimit, Overloaded,
Billing, Timeout, ModelNotFound, SessionExpired, ContextOverflow, Unknown` —
serialised `snake_case`, so identical wire strings.

**Delete OpenClaw's four that we lack (`server_error`, `empty_response`,
`no_error_details`, `unclassified`) and the remaining ten are our ten, in our exact
order, `unknown` last in both.** We add one they lack (`context_overflow`).

The order follows no external principle — not alphabetical (`format` before
`rate_limit` before `overloaded`), not severity, not status code. It is arbitrary.
Ten arbitrary items agreeing in order is the signal that matters here: this is
selection-and-arrangement, the one thing in a short vocabulary list that can be
expressive, and it survives a language change because a translation is ordinarily
still a derivative work.

It is also **admitted on its face**. `failover.rs:16-17`: *"String representations
match openclaw's TS string-union for cross-language log/telemetry compatibility."*
That is a contemporaneous statement of deliberate matching, with a stated and
legitimate engineering motive.

**What did NOT carry.** OpenClaw's `failover-error.ts` is 714 lines of recursive
cause-chain walking — `MAX_FAILOVER_CAUSE_DEPTH = 25`, cycle-detecting `seen` sets,
`findErrorProperty`, nested-format override logic, and a reason→HTTP-status map
(402/500/429/503/401/403/408/400/404/410). Our `failover.rs` is 339 lines of which
roughly 180 are tests; the body is an enum plus a builder-pattern struct and a
`std::error::Error::source` impl. **No shared function decomposition, no shared
control flow, no shared constant, and the status map has no counterpart in our file
at all.** The literal metric agrees independently: the ten shared literals *are* the
taxonomy and nothing else.

**Against over-reading this.** The individual words (`auth`, `timeout`,
`rate_limit`) are the ordinary functional names for those conditions and carry
nothing alone. Whether a ten-item vocabulary clears the originality threshold for a
protectable compilation is a legal question I am not answering. What is settled as
fact: the selection matches, the order matches, and our own code says that was
intentional.

**Recommendation:** keep the attribution. As written the header claims exactly the
right thing — *"FailoverReason taxonomy ported from openclaw"*, the taxonomy
specifically and not the code. It is the one accurate notice of the nine. Move it
to `THIRD-PARTY-NOTICES.md` and keep a pointer.

### 3.2 Site 2 — `key_rotation.rs` — **(a) strip**

The header says "API key rotation pool — ported from openclaw". **OpenClaw has no
pool.**

`api-key-rotation.ts` exports `executeWithApiKeyRotation<T>` — a *stateless wrapper*
that takes a key array and an `execute` callback, loops keys itself, decides
rotate-vs-same-key-retry per failure, and sleeps with backoff. It has no persistent
state, no cooldown timestamps, no last-good stickiness, and no cursor; every call
starts at index 0.

Ours is a stateful `KeyPool` struct: `Vec<KeyState>` with `last_failed_at:
Option<Instant>` per key, a `last_good_idx` for stickiness, a `cursor` for
round-robin, a 60 s default cooldown, and three methods (`next_key`,
`mark_success`, `mark_failure`). **The caller drives the loop; the pool only answers
"which key next".**

These are inverted designs: theirs owns the loop and has no memory; ours has memory
and owns no loop. Measured shared literals: **0** — the floor of the negative
control band.

The only real connection is a doc comment noting that duplicate keys are filtered at
construction, *"matches openclaw's `dedupeApiKeys` invariant"*. `dedupeApiKeys` is a
genuine OpenClaw function name, so the author had read the source — but the
invariant itself (do not retry the same key twice) is close to inevitable, and we
implement it with an inline `HashSet::insert` where they call a shared normalisation
helper. That is a referenced idea, not carried expression.

### 3.3 Site 3 — `classify.rs`, the "3-tier classifier" — **(a) strip**

Our header claims a 3-tier precedence: status > body > sdk_code. **OpenClaw's
precedence is not that.** `classifyFailoverSignal` runs: an HTML-transport check
first, then a code check with an `auth_permanent` early return, then status, then
code, then message — and it *threads the message classification into the status
classifier as an argument* rather than consulting it as a later tier. Ours is a
clean early-return cascade; theirs is a merge.

The body-matching arms differ in order too. OpenClaw's message classifier has ~25
arms beginning image-dimension → image-size → session-expired → model-not-found →
context-overflow → …; ours has 11 beginning session-expired → billing → auth →
auth-permanent → rate-limit → …. Only the first coincides — **1 of 11**. OpenClaw
uses named predicates over compiled word-boundary regexes; we use inline
`contains()` on a lowercased string.

The status maps differ where it counts:

| status | OpenClaw (no-message path) | ours |
|--------|---------------------------|------|
| 403 | `auth` | `AuthPermanent` |
| 503 | **`timeout`** (overloaded only if the body says so) | **`Overloaded`** |
| 413 | *not handled* | `ContextOverflow` |
| 410 / 499 / 422 | handled | *absent* |

**And our comment misdescribes them.** `classify.rs:45-47` asserts an *"openclaw
semantic split: 503/529 are explicit 'overloaded' signals"*. At the pinned baseline
OpenClaw's bare 503 returns `timeout`; only 529 is unconditionally `overloaded`. A
transcriber does not invert the rule they are copying. This is the clearest single
indication at any site that the author was writing from memory of a reading rather
than from the file.

All 29 shared literals decompose into vendor-dictated strings — POSIX errno names
(`etimedout`, `econnreset`, `econnaborted`, `ehostunreach`, `eai_again`), Google
gRPC canonical codes (`resource_exhausted`, `invalid_argument`), Anthropic error
types (`invalid_request_error`, `overloaded_error`, `request_too_large`), OpenAI
codes (`rate_limit_exceeded`, `insufficient quota`), AWS `throttlingexception`,
Anthropic's own remediation sentence (`plans & billing`) — plus the site-1 taxonomy
names, which this module necessarily consumes because its whole job is to emit
`FailoverReason` values. **Any independent implementation classifying these same
five providers would contain substantially this list.**

### 3.4 Site 4 — `cache_observation.rs` — **(a) strip**

Two claims in this file; both fail.

**Claim 1**, at `cache_observation.rs:10-11`: our `CacheRetention` *"matches
openclaw's `ContextEnginePromptCacheRetention` shape"*. The type exists —
`src/context-engine/types.ts:156` — and is:

```ts
type ContextEnginePromptCacheRetention = "none" | "short" | "long" | "in_memory" | "24h";
```

Ours is `Ephemeral5m("5m") | Ephemeral1h("1h") | None("none")`. **One value in
common out of five and three.** Worse for the claim: `"5m"`/`"1h"` are what OpenClaw
treats as the *legacy* `cacheControlTtl` input, which it maps *away* to
`short`/`long`. We adopted their deprecated input vocabulary as our canonical
output. The comment is false.

**Claim 2**, the header. The bulk of the module is `InvalidationCause` — seven
variants (`system_prompt_drift`, `tool_definitions_changed`, `history_rewritten`,
`expired`, `provider_rejected`, `no_marker`, `unknown`). Searching the concept
rather than one keyword — **17 probes** across both snake_case and camelCase spellings
plus five broader concept names (`cacheMissReason`, `invalidationReason`,
`cacheInvalidation`, `promptCacheMiss`, `invalidation_cause`) — returned **zero hits**,
with the instrument proven alive in the same invocation (27 files match
`CacheRetention`; gibberish returns 0). Shared literals: **0**.

### 3.5 Site 5 — `wcore-pricing/refresh.rs` — **(a) strip**

Two coincidences, both externally fixed:

- Both define the constant `OPENROUTER_MODELS_URL =
  "https://openrouter.ai/api/v1/models"`. The value is OpenRouter's published
  endpoint; the name is about as forced as a name gets.
- Both use a 24 h TTL (`CACHE_TTL_MS = 24 * 60 * 60_000` vs `DEFAULT_TTL_SECONDS =
  24 * 60 * 60`). A round, conventional refresh period.

Beyond that they do different jobs. OpenClaw's is a 1428-line gateway-wide pricing
**cache** that also fetches LiteLLM, with a fetch timeout, a 5 MB catalog cap, and a
singleton lifecycle (`startGatewayModelPricingRefresh`,
`resetGatewayModelPricingCacheForTest`). Ours is a 641-line **diff-and-audit** layer:
a `PricingRefresher` that fetches live prices, diffs them against the bundled
catalog, and emits `CatalogChange { Added | Removed | Changed }` events for a human
to inspect, **with auto-application off by default**. OpenClaw has no counterpart to
the diff design at all. Jaccard 0.0194 — inside the negative-control band.

### 3.6 Site 6 — `retry.rs:738` — **(a) strip**

This is the most precisely-worded notice of the nine, which makes it the most
testable: it names `src/infra/retry-policy.ts` → `getChannelApiRetryAfterMs`.

| | OpenClaw `getChannelApiRetryAfterMs` | ours `extract_retry_after_ms_from_nested` |
|---|---|---|
| shapes probed | 3: `parameters`, `response.parameters`, `error.parameters` | 5: top-level `retry_after_ms`, top-level `retry_after`, `parameters.*`, `body.*`, `headers["retry-after"]` |
| field names | `retry_after` only | `retry_after_ms` **and** `retry_after` |
| cap | **none** | `RETRY_AFTER_CAP_MS = 300_000` |
| validation | finite number | rejects zero, negative, NaN, ∞ |
| form | nested ternary chain | `.or_else()` chain |
| domain | **Telegram channel APIs** | LLM providers |
| visibility | private to its module | `pub` |

One of our five probe shapes (`parameters.retry_after`) is one of their three; their
`response.parameters` shape is absent from ours. The ×1000 is arithmetic. The field
name `retry_after` is Telegram's; the header `Retry-After` is RFC 9110's — both
dictated. Our file cites **RFC 9110 §10.2.3 and RFC 7231**, i.e. it was written
against the HTTP specification.

Measured against its own named source: **Jaccard 0.0226**, shared literals `error`,
`parameters`, `response` — the JSON field names being probed. That is inside the
negative-control band. Numeric constants shared: `1000`, `400`, `429`, `30_000` —
the ms multiplier and HTTP status codes.

The header's own qualifier — *"generalized to walk additional shapes seen across LLM
provider APIs"* — is honest. But what remains after the generalisation is the idea
"walk nested error shapes for a retry-after field and convert seconds to
milliseconds", which is a procedure, not expression.

### 3.7 Site 7 — `anthropic.rs:307`, cache zones — **(c) needs a human**

This is the one site I will not call, and it is worth being precise about why the
evidence points both ways.

**Pointing toward derivation — the only exact identifier match in the whole set.**
OpenClaw declares `const ANTHROPIC_CACHE_CONTROL_LIMIT = 4;` in two files
(`src/agents/anthropic-payload-policy.ts:30`, `src/llm/providers/anthropic.ts:69`).
We declare `pub const ANTHROPIC_CACHE_CONTROL_LIMIT: usize = 4;`. Identical name,
identical value. Both codebases also do remaining-budget accounting — count the
markers already spent on system and tools, pass the remainder to the message pass.

**Pointing away.** Both halves of that identifier are externally fixed: 4 is
Anthropic's published hard limit on `cache_control` blocks per request, and the name
is the obvious vendor + API-field + "limit" compound. The budget-accounting shape is
similarly constrained — system, tools and messages are the only three places the
Messages API permits a marker, so "mark them in that order and track the remainder"
is close to the only design available.

**And the specific thing the header claims is demonstrably absent from both peers.**
The notice reads *"(moving-breakpoint layout, ported from openclaw/hermes-agent)"*.
Our moving-breakpoint layout is the zone-3/zone-4 pair: turn *k* marks boundaries
{k−1, k} so consecutive turns always overlap on one marked boundary. Probing both
peers for `moving breakpoint` and `movingBreakpoint` returns **zero** (instruments
alive: OpenClaw 19 files match `breakpoint`, 30 match `cache_control`; Hermes 497
files match `export`, 21 match `cache_control`). OpenClaw marks three zones, not
four, and has no previous-user-boundary zone. Nor does either peer have our token
floor with its deliberately-low `BYTES_PER_TOKEN_ESTIMATE = 2` and documented CJK
rationale, or our strip-on-disable behaviour.

The Hermes half of the attribution is weakest: its prompt-cache module
(`agent/prompt_caching.py`) is 79 lines of Python sharing three literals with ours —
`cache_control`, `content`, `ephemeral`, all Anthropic wire fields. Literal
comparison against OpenClaw's payload policy gives Jaccard 0.0337, against Hermes
0.0182 — both inside the negative-control band, and every shared item
(`cache_control`, `ephemeral`, `system`, `assistant`, `content`, `stream`,
`thinking`) is an Anthropic API field name.

**Why this needs a human.** The distinctive part of our function has no counterpart,
so the header attributes the one component that is provably ours. That reads like
(a). But there is a genuine exact identifier match, which is the sort of thing a
reviewer should weigh personally rather than accept my characterisation of it as
"dictated on both halves". Sending it up.

---

## 4. Sites 8 & 9 — the channel crates, a different chain

The brief flagged these as *"potentially the cheapest resolution of the lot"*: if
Wayland Desktop's `ImessagePlugin`/`MsTeamsPlugin` are original, there is no
third-party question at all. **They are not, and this is the finding that runs
hardest against the comfortable answer.**

### 4.1 Hop 1, OpenClaw → Wayland Desktop: real, deliberate, and self-attributed

OpenClaw at the pinned baseline genuinely ships both plugins — `extensions/imessage/`
and `extensions/msteams/`, with the identifiers `ImessagePlugin` and `MsTeamsPlugin`
each appearing in 13 files.

Wayland Desktop contains a directory literally named
`.blackboard/openclaw-fork/`, holding a `WAVE-PLAN.md`, a 366-line
`TRANSLATION-GUIDE.md`, and thirteen per-channel manifests. `MANIFEST-imessage.md`
has a section headed **"OpenClaw harvest"** naming
`~/dev/openclaw/extensions/imessage/src/` and dispositioning each file — *"client.ts
— AppleScript invocation wrapper. MAIN HARVEST"*, *"conversation-id.ts … HARVEST"*,
*"channel.ts … SKIP"*.

The `TRANSLATION-GUIDE.md` states the goal as *"harvest the protocol-handling code
from an OpenClaw extension and rewrap it as a Wayland `BasePlugin` subclass"*, and
enumerates what to harvest: *"The SDK client wrapper … Format/normalization helpers
… Identity/auth verification helpers … Connection probes / lifecycle helpers"*.

Its §6 is headed **"License attribution (MANDATORY per harvested file)"** and
mandates a header naming *"Portions adapted from OpenClaw … Copyright (c) 2025 Peter
Steinberger … MIT License — see LICENSES/openclaw.txt"*.

**All four Desktop plugin files carry that header, and `app/LICENSES/openclaw.txt`
exists.** `MsTeamsAdapter.ts` goes further and cites a specific carried constant:
*"Harvested from openclaw/extensions/msteams/src/inbound.ts … and outbound.ts (chunk
limit, adaptive card shape)"*, with *"// Bot Framework text chunk limit (per OpenClaw
outbound.ts: textChunkLimit=4000)"*. I confirmed `textChunkLimit: 4000` at the pinned
OpenClaw baseline.

So hop 1 is not merely derived — it was *planned* as a derivation, *executed* as one,
and *attributed* as one by the Desktop project itself.

### 4.2 Hop 2, Desktop → our Rust: the specific carried marker did NOT survive

Our Rust `wcore-channel-msteams` **does not contain 4000 anywhere**, and has no
chunking logic at all. The one constant Desktop explicitly flagged as OpenClaw's
died at the language boundary.

The literals our Rust shares with Desktop are dictated:

- *imessage* (14 shared): macOS AppleEvent error codes `-1728`/`-1743`, TCC error
  text *"not allowed to send apple events"*, *"can't get chat id"*, `chat.db`,
  `osascript`, `applescript`, `library`, `messages`, plus test values (`hello`,
  `+15551234567`). All fixed by macOS.
- *msteams* (11 shared): `application/json`, `content-type`, `client_credentials`,
  `conversationupdate`, `https://api.botframework.com/.default`,
  `https://login.microsoftonline.com/botframework.com/oauth2/v2.0/token`, `typing`,
  `attachment`. All Microsoft Bot Framework and OAuth2 wire constants.

**The one non-dictated convention we share with Desktop turns out to be Desktop's
own.** Both encode a composite chat id as `{serviceUrl}|{conversationId}` and split
on the rightmost pipe. Probing OpenClaw's msteams extension for that convention
returned **zero** across seven spellings (instrument alive: 95 files in that
extension match `conversation`; gibberish returns 0). That design is Wayland
Desktop's invention, and it carried Desktop → Rust — a hop between two things Sean
owns, raising no third-party question.

*(A methodology note against myself: my first Hermes probe searched `*.ts` only and
reported `cache_control` absent. Hermes is 2596 Python files to 681 TypeScript; the
all-language probe found it in 21 files. The narrow probe would have produced a
false absence. Corrected before it reached any finding.)*

### 4.3 What this means, honestly

I established that hop 1 is a documented derivation. I did **not** establish that
OpenClaw expression survived hop 2 into the Rust — and the literal metric cannot
settle it, because these crates are nearly all vendor constants and even the
known-derived hop 1 scores low on it (containment 17 % for imessage, 33 % for
msteams). Settling it would need a structure-and-sequence comparison of 1296 lines of
Rust against 963 lines of Desktop TypeScript against OpenClaw's much larger
extension, which I did not complete.

**Recommendation is (b) keep attribution for both**, for a reason that does not
depend on that open question: the upstream hop is documented and self-attributed, the
Desktop project already concluded attribution was owed and shipped a licence file,
and our Rust crates describe themselves as ports of those Desktop plugins. Keeping
the notice is correct whichever way the residual question falls, and removing it
would put us out of step with the licence file Desktop already ships. Consolidate
into `THIRD-PARTY-NOTICES.md` together with `wcore-channels-registry/src/lib.rs:55`.

---

## 5. What a lawyer should take from this

1. **The nine notices are not nine of the same thing.** One is accurate (§3.1), two
   describe a real documented chain (§4), five are wrong, and one is genuinely
   arguable. Treating them as a single admission would be a mistake in both
   directions.

2. **Three of the five wrong ones are wrong in a specific, informative way**: the
   comment misstates what the peer does (§3.3's 503 semantics, §3.4's retention
   shape, §3.2's "pool"). Someone copying a file does not misdescribe it. These read
   as an author who had read OpenClaw, absorbed the concepts, written original Rust,
   and then over-credited the influence in a header — which is a documentation
   defect, not a provenance one.

3. **The one real taxonomy match is narrow and admitted** (§3.1). It is a
   vocabulary and its order, not code. Our own comment states the matching was
   deliberate and gives an engineering reason (cross-language log/telemetry
   compatibility). Whether a ten-item ordered vocabulary is protectable is the
   question to put to counsel; the facts underneath it are not in dispute.

4. **A great deal of what looks like overlap is the providers' own vocabulary.**
   `insufficient_quota`, `overloaded_error`, `throttlingexception`,
   `resource_exhausted`, `cache_control`, `ephemeral`, `client_credentials` and the
   Bot Framework endpoints are Anthropic's, OpenAI's, AWS's, Google's and
   Microsoft's. Two independent implementations of the same integrations must
   contain them. The control in §1.3 exists precisely so this cannot be mistaken for
   copying.

5. **The controls are the reason to trust any of this.** A method that only ever
   reports similarity, or only ever reports none, proves nothing. This one lights up
   at 0.24–0.34 on known copies and sits at 0.000–0.043 on known-independent
   modules — including one negative control with the *same filename and problem
   domain* as its peer.

6. **Where I could not settle it, I have said so** (§3.7, §4.3) rather than rounding
   toward the answer that suits us.

---

## 6. Recommended disposition

**(a) Independent — the header is wrong, strip it — 5 sites**
`key_rotation.rs:1` · `classify.rs:1` · `cache_observation.rs:1` ·
`wcore-pricing/refresh.rs:1` · `retry.rs:738`

Strip the copyright notices. Separately, **fix or delete the three factually false
comments** — `classify.rs:45-47`, `cache_observation.rs:10-11`, and
`key_rotation.rs:21-22` — regardless of what happens to the headers. Leaving an
inaccurate description of another project's behaviour in our source is its own
defect, and it is what let the audit read these as admissions. Note that
`classify.rs` legitimately consumes the site-1 taxonomy; that vocabulary stays
attributed at site 1.

**(b) Derived — keep attribution, consolidate into `THIRD-PARTY-NOTICES.md` — 3 sites**
`failover.rs:1` (taxonomy only — keep the wording as-is, it is precise) ·
`wcore-channel-imessage/src/lib.rs:16` · `wcore-channel-msteams/src/lib.rs:15`
Carry `wcore-channels-registry/src/lib.rs:55` along with the channel pair.

**(c) Genuinely unclear — needs a human — 1 site**
`anthropic.rs:307`. One exact identifier match (`ANTHROPIC_CACHE_CONTROL_LIMIT = 4`)
whose two components are each externally dictated, against a header that credits a
layout with zero counterpart in either named peer. A person should look at this, not
a metric.

**Not done, and worth doing if the question goes anywhere formal:** a
structure-and-sequence comparison for sites 8–9 across the full 1296 lines of Rust,
963 lines of Desktop TypeScript and OpenClaw's imessage/msteams extensions. The
literal metric is the wrong instrument there and I have not substituted a right one.

---

## Appendix — reproducing this

Peer sources were extracted at the pinned baselines, never read from the working
trees:

```
git -C ~/dev/resources/openclaw     show 11a0ad10:<path>
git -C ~/dev/resources/hermes-agent show dbe734be:<path>
```

The similarity harness is committed at `.planning/provenance-litcmp.py`:

```
python3 .planning/provenance-litcmp.py <file-a> <file-b> "<label>"
```

Every absence claim in this document states the query that produced it and was run
with a known-positive in the same invocation. The controls in §1.3 should be re-run
first by anyone checking this work — if they do not reproduce the two bands, nothing
downstream of them holds.
