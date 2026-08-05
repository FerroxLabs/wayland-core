# FIX-EVIDENCE-AUDIT — lane summary

Lane `fix-evidence-audit`, branch `lane/fix-evidence-audit`, base integration
`e7bc6d88`. Read-and-audit lane: **no behaviour changed, no source file
modified.** Nothing was compiled — on the Mac or on hetzner.

Every number below was produced by redirecting an unproxied tool
(`/usr/bin/git`, `/usr/bin/grep`, `/usr/bin/env python3`) to a file and reading
that file with the Read tool, per LANE-BRIEF §3b. Both detectors and the
provenance metric are committed in this directory and are re-runnable.

---

# TASK 1 — the mock-evidence sweep

## 1.1 The method, and the control that says whether to trust it

Two committed, self-testing detectors:

- **`scan_declarations.py`** — a *capability declaration* is a function whose
  entire body is one literal. The literal is a claim about somebody else's
  system and nothing in the type system checks it. Self-test (3 assertions,
  §6b-ii): bool literal detected; `Some(N)` detected; multi-line body NOT
  detected; delegating call NOT detected. Result `PASS`.
  At `e7bc6d88`: **487** literal-bodied fns under `crates/`, **177**
  capability-named, **152** of those under `src/`.

- **`scan_invented_shapes.py`** — a consumer reads named fields out of a
  `serde_json::Value`; if no production code anywhere writes that field, the
  shape exists only in the consumer's head and in the tests its author wrote.
  Scanned 1771 `.rs` files: 491 keys read in production, **188** with no
  production writer, **111** whose only writer is a test.

### CONTROL — does the method rediscover the two KNOWN cases?

**Known case 1 — the exactly-once bits: rediscovered.** Detector 1 surfaces
`supports_outbound_idempotency` directly (6 sites).

**Known case 2 — the TUI bash formatter: MISSED on the first attempt.**
`exit_code` is written in production by
`wcore-agent/src/child_transaction/gate_executor.rs:423`, an unrelated
subsystem, and that single name collision bought the formatter a pass on the
exact key that names the defect.

**The instrument was repaired in-lane rather than written up (§6b-ii).**
Scoring moved from per-key to per-consumer-file. Re-run, printed by the tool on
every invocation:

```
CONTROL(known positive) crates/wcore-cli/src/tui/tool_formatters/bash.rs:
  2/3 unbacked, ratio=0.667, unbacked_keys=['cmd', 'stdout']
```

Known-negative side of the same run: **44** consumer files score `0.00` with ≥4
keys read (`protocol_bridge.rs` 0/15, `yuanbao_tools.rs` 0/15,
`cronjob_tools.rs` 0/15). The instrument discriminates; it is not flagging
everything.

**Both known cases are rediscovered by the repaired method.**

## 1.2 Premise check on the brief

| brief claim | verdict | evidence |
|---|---|---|
| exactly-once is 1 of 10 at HEAD | **HELD** | 5 overrides + trait default read at `/tmp/lfea-idem-bodies.txt`; only Matrix (`wcore-channel-matrix/src/lib.rs:294`) returns `true` |
| declared media limits were "purely decorative" | **NO LONGER TRUE — do not re-report** | all 9 adapters with a `media_bounds()` override now derive the streaming cap from the same `MEDIA_BOUNDS` constant (`discord/rest.rs:455`, `email/imap.rs:623`, `imessage/channel.rs:52`, `matrix/rest.rs:104`, `signal/lib.rs:132`, `slack/api.rs:478`, `sms/api.rs:68`, `telegram/api.rs:1023`, `whatsapp/api.rs:713`). MS Teams has no override and documents why. That prior fix is real and current. |

## 1.3 RANKED INVENTORY — ranked by "what breaks if wrong"

### H1 — `Tool::is_concurrency_safe` (97 declarations in `src/`, 45 unconditional `true`)

- **Declaration.** `wcore-tools/src/lib.rs:342`, a *required* trait method
  (no default — a new tool cannot inherit a green by staying silent, which is
  a genuine design strength). Doc is one line: *"Whether this tool is safe to
  run concurrently."*
- **Evidence today.** Exactly **one** production consumer:
  `wcore-agent/src/orchestration/mod.rs:3390`, inside `partition()`, which
  merges adjacent calls into one concurrent batch when the bit is `true`.
  `partition()` itself has **no direct test**. Every other reference in the
  workspace is a test asserting the declaration back at itself —
  `assert!(tool.is_concurrency_safe(&json!({})))` — i.e. the test constructs
  the value the function returns and then asserts it. The one test that looks
  like behavioural proof, `orchestration_test.rs:86
  test_execute_concurrent_safe_tools`, registers two `MockTool`s and asserts
  both results come back. **A mock with no shared state cannot exhibit
  interference**, so the test cannot fail for the reason it exists — the §6a-i
  shape, and the same shape as the known exactly-once case.
- **A real proof would require** running two invocations of a `true`-declaring
  tool against genuinely shared state and asserting no interference, plus a
  mutation control (flip one `false` to `true` and watch the harness redden).
- **What breaks if wrong.** The orchestrator runs the tool in parallel with
  siblings. For a tool that mutates shared state that is a data race in the
  product's tool layer, silently, with no error surfaced to the user.
- **Rank rationale.** Largest population, most severe failure mode, weakest
  evidence. Nothing else in this inventory can corrupt state.

**Two sub-findings inside H1, opposite in direction:**

- **H1a (a real defect).** `wcore-tools/src/doc_tool.rs:215` declares `true`
  with the comment *"Read-only filesystem access — safe to run alongside other
  tools."* The same file, at line **363**, calls `write_doc_artifact`, which at
  line **389** does `std::fs::create_dir_all` + `std::fs::write` into a shared
  `std::env::temp_dir().join("wayland-doc-extract")`. `#[cfg(test)]` does not
  begin until line **911**, so this is production; the `doc-extract` feature is
  **default-on** (`wcore-tools/Cargo.toml:135`). The declaration's stated
  premise is contradicted 174 lines below it and no test checks the premise.
- **H1b (an honest negative — do NOT schedule these).** `kubectl_tool.rs:296`,
  `aws_cli_tool.rs:355`, `sql_query_tool.rs:297` and `gcloud_tool.rs:309` also
  declare unconditional `true` on a stated read-only premise, but that premise
  **is** enforced by closed allow-lists with both-direction tests
  (`READ_ONLY_VERBS` + `is_read_only_verb("delete") == false`;
  `s3_ls_allowed_but_s3_rm_rejected`; `reject_if_mutating` +
  `mutating_statements_are_rejected`). These are correctly evidenced. Reporting
  them would be a false positive and I am saying so explicitly.

### H2 — `Channel::max_message_len` silently disables the one working exactly-once guarantee

- **Declaration.** 9 sites. Slack `Some(39_000)`, MS Teams `Some(28_000)`,
  Matrix `Some(32_768)`, Telegram `Some(4096)`, Discord `Some(2000)`,
  Twilio SMS `Some(1600)`, WhatsApp, plus the trait's `None`.
- **Evidence today.** Per-adapter unit tests of the form
  `assert_eq!(ch.max_message_len(), Some(39_000))`
  (`wcore-channel-slack/src/lib.rs:841`, telegram `:599`, discord `:913`,
  sms `:500`, whatsapp `:940`, msteams `:695`). **Each asserts the literal the
  function three hundred lines above it returns.** The test cannot fail unless
  someone edits the declaration and forgets the test; it says nothing about the
  platform's real cap. This is the purest instance of the pattern in the
  workspace.
- **What breaks if wrong — and this is the part that has not been noticed.**
  `wcore-channels/src/manager.rs:776-789`: when the body chunks into more than
  one piece, `send_to_keyed` takes the multi-chunk arm and **the idempotency
  key is not passed at all** (deliberately, and the comment explains why — one
  key cannot identify N messages). So a `max_message_len` that is too LOW
  chunks a message that would have fitted, and the delivery loses its key.
  **On Matrix — the one adapter of ten with a real exactly-once guarantee —
  that converts an exactly-once delivery into an at-least-once one, for exactly
  the long messages most expensive to duplicate.** A cap that is too HIGH is
  the plain failure: the platform rejects the send.
- **A real proof would require** sending a body at `cap` and at `cap+1` against
  each real platform and reading the arrival count back at the destination —
  the same shape as the live Slack/Discord runs that corrected the idempotency
  bits.
- **Rank rationale.** Directly re-opens the defect this lane exists because of,
  on the single adapter that survived it.

### H3 — `Channel::native_actions` is proved against an unstarted, uncredentialled adapter

- **Declaration.** `wcore-channels/src/lib.rs:231`, overridden by 12 adapters.
  Read at `manager.rs:921 native_actions_on`, whose doc tells callers to read
  it **before** `edit_on`/`delete_on` *"when the answer matters and the call
  does not: a delete is a request a caller may not want to issue
  speculatively."*
- **Evidence today.** `wcore-channels-registry/tests/native_action_matrix.rs`
  is a genuinely well-built harness — production loader, anti-vacuity count,
  both branches asserted non-zero, and a three-assertion mutation control on
  its own assertion helper. **Its own docstring states the limit**
  (lines 101-106): *"No credential is ever resolved: nothing here is started,
  and every call the matrix makes is expected to fail at the auth/not-started
  boundary."* So `Implemented` is proved to mean **"an override exists and it
  does not return `Unsupported`"** — not that the edit/delete/react reaches the
  platform and succeeds. That is precisely the gap between "the key goes on the
  wire per mockito" and "the platform honours the key" that produced the known
  case. Live coverage exists for Slack, Discord and Matrix
  (`live_slack_actions.rs`, `live_discord_actions.rs`, `matrix_live_room.rs`);
  Telegram and MS Teams declare `Implemented` for edit/delete with **no live
  run located**.
- **What breaks if wrong.** A caller that consulted the declaration to avoid a
  speculative delete issues one anyway and it fails — or worse, appears to
  succeed. Bounded, recoverable, visible. Materially less severe than H1/H2.

### M4 — the invented-payload family is wider than the one known formatter

Not this lane's to fix — `crates/wcore-cli/src/tui/tool_formatters/**` and
`toolcard.rs` belong to another lane — but the sweep says the known defect is
**7 of 11 formatters**, not one, and that lane should know:

| formatter | unbacked / keys read | keys no production code writes |
|---|---|---|
| `bash.rs` | 2/3 | `cmd`, `stdout` |
| `file_ops.rs` | 3/6 | `added`, `lines`, `removed` |
| `discord.rs` | 2/6 | `channel_name`, `chars` |
| `web_fetch.rs` | 1/4 | `readability_score` |
| `tts.rs` | 1/4 | `chars` |
| `transcribe.rs` | 1/4 | `seconds` |
| `github.rs` | 1/5 | `html_url` |
| `web.rs`, `image_gen.rs`, `vision.rs`, `homeassistant.rs` | 0 | — (clean) |

**False-positive class for this detector, stated up front:** consumers reading
keys produced by a *remote* API are legitimately "unbacked" — OAuth token
fields (`access_token`, `refresh_token`), Gemini's `usageMetadata`, Slack
inbound event fields, Ollama's `eval_count`. Those scored 0.44-0.83 and are
**not** findings. The formatter family is different because the payload's
producer is our own tool.

### L5 — swept, examined, and NOT findings (recorded so nobody re-opens them)

- **`JobHandler::dispatch_is_idempotent`** — the retry/dedupe twin the brief
  points at. `wcore-agent/src/cron.rs:182` **delegates** to
  `ChannelManager::supports_outbound_idempotency`; `wcore-cron/src/runner.rs:279`
  defaults to `false` with the reasoning written out. Read at
  `wcore-gateway/src/automation.rs:155` to decide re-dispatch. It is not an
  independent claim — it inherits the channel bit's correctness, and its default
  fails in the safe direction. **But H2 propagates here**: this is the exact
  decision that a chunk-nullified key silently makes wrong on Matrix.
- **`Tool::supports_streaming`** — `plugin_tool_adapter.rs:127` returns
  unconditional `true`; the only behavioural test
  (`plugin_tool_delivery.rs:113`) exists because chunks *were* once dropped
  while the bit said `true`. Consumer: `orchestration/mod.rs:2151`. If wrong,
  output buffers instead of streaming. No corruption, no duplication,
  user-visible. Genuinely low.
- **`Channel::media_bounds`** — see §1.2. Previously decorative, now sound.
- **kubectl / aws / gcloud / sql read-only premises** — see H1b.

## 1.4 False-negative risk — what this sweep would have missed

Stated honestly, because an audit that cannot say what it missed is not
finished.

1. **Detector 1 only sees single-literal bodies.** A capability computed from a
   config field, a `match` on input, or a delegation is invisible to it. The
   per-op discriminating tools (`git.rs`, `github_tool.rs`, `discord_tool.rs`,
   `gitlab_tool.rs`, `yuanbao_tools.rs`, `homeassistant_tool.rs`) are exactly
   that shape — so **the tools most likely to have subtle per-op errors are the
   ones the detector cannot see.** This is the largest known gap.
2. **Detector 2 only sees dynamically-typed payloads.** A `serde`-derived
   struct with a wrong field is caught by the compiler on the *reader* side but
   not against a producer in another process (the Desktop wire contract).
   Nothing here inspects that seam.
3. **Neither detector reads non-Rust surface** — TOML schemas, JSON contract
   fixtures, docs. `docs/delivery-semantics.md` carries a per-adapter table
   that is a declaration in prose; it is bound by
   `delivery_semantics_declaration.rs`, which I read but did not re-derive.
4. **I compiled nothing.** Every claim is source-reading. A declaration could
   be `#[cfg]`-ed out on some target and my line-based reader would not know.
5. **Absence claims made here** — "`partition()` has no direct test",
   "no live run located for Telegram/MS Teams edit" — were run with the
   known-positive control alive (927 files match `pub fn`; a gibberish needle
   returns 0) and by concept rather than single keyword, but they remain the
   weakest claims in this report per §3b-i.

---

# TASK 2 — the Hermes attribution block

## 2.1 What the block is

`THIRD-PARTY-NOTICES.md:9-46`, *"Nous Research — Hermes (MIT)"*, naming **nine
modules**. The merged provenance pass (`11995d59`) put the **OpenClaw** block
through the literal-overlap comparison and left this one untouched.

## 2.2 Method — reused, not invented, and proved to be the same instrument

The prior lane's `litcmp.py` was **not committed**. I reimplemented it to the
spec quoted verbatim in `PROVENANCE-COMPARISON-NOTES.md` MEASUREMENT 3
(*"extract every quoted literal of length >= 5, lowercase, drop punctuation-only,
report |A ∩ B| and Jaccard"*) and made its correctness criterion the
reproduction of that lane's published numbers. Peer source is read at the
**pinned** sha via `git show`, never from the working tree — both peer trees are
~1 month ahead of their baselines. **The peer trees were only read; nothing was
written or executed in them.**

| pairing | published | mine | verdict |
|---|---|---|---|
| POS cerebras/moonshot (our own template copy) | 11 / 0.3438 | 11 / 0.3438 | exact |
| POS deepseek/moonshot | 13 / 0.2364 | 13 / 0.2281 | Δ 0.008 |
| NEG cooldown.rs vs `errors.ts` | 0 / 0.0000 | 0 / 0.0000 | exact |
| NEG failover_policy.rs vs `failover-policy.ts` | 1 / 0.0435 | 1 / 0.0435 | exact |
| S1 failover.rs vs `failover-error.ts` | 10 / 0.1493 | 10 / 0.1471 | Δ 0.002 |
| S3 classify.rs vs `errors.ts` | 29 / 0.0948 | 29 / 0.0967 | Δ 0.002 |

**All six intersection counts reproduce exactly. Four of six Jaccards reproduce
exactly; two are within 0.008** — an order of magnitude smaller than the gaps
between the bands (0.0435 → 0.0948 → 0.2364). The calibration transfers, with
that tolerance stated. A quote-stripping variant was tried, scored worse
overall, and was discarded; both variants are recorded in
`capture-litcmp-control-reproduction.txt`.

Two additions I am flagging as **extensions, not substitutions**:

- **Fresh negative controls for THIS language pair.** The published band was
  Rust-vs-TypeScript. Six unattributed Core modules scored against Hermes
  Python give **Jaccard 0.0000-0.0260, containment 0.0000-0.1250**.
- **Containment `|A∩B| / |A|`.** Jaccard under-reads badly when the peer file is
  much larger — `send_message_tool.py` has 302 literals to our 81, which drags
  a heavy overlap down to 0.0943. Containment is reported alongside Jaccard,
  never instead of it, and has its own negative band above.

## 2.3 Per-site scores against the calibration

Peer at pinned `dbe734be`. Negative band for this language pair:
**J 0.0000-0.0260, C 0.0000-0.1250**. Known-copy band: **J 0.2364-0.3438**.

| # | site | Hermes counterpart | shared | Jaccard | containment | band | recommendation |
|---|---|---|---|---|---|---|---|
| 3 | `wcore-tools/src/discord_tool.rs` | `tools/discord_tool.py` | 87 | **0.2719** | **0.5506** | **copy** | **KEEP** |
| 1 | `wcore-tools/src/todo.rs` | `tools/todo_tool.py` | 25 | **0.2941** | **0.4386** | **copy** | **KEEP** |
| 2 | `wcore-tools/src/yuanbao_tools.rs` | `tools/yuanbao_tools.py` | 65 | **0.2462** | **0.3801** | **copy** | **KEEP** |
| 6 | `wcore-tools/src/homeassistant_tool.rs` | `tools/homeassistant_tool.py` | 49 | 0.2207 | **0.3828** | just under copy floor, 3× the negative ceiling | **KEEP** |
| 4 | `wcore-tools/src/send_message.rs` | `tools/send_message_tool.py` | 33 | 0.0943 | **0.4074** | elevated J, copy-level C | **KEEP** |
| 5 | `wcore-tools/src/session_search.rs` | `tools/session_search_tool.py` | 15 | 0.0593 | 0.3261 | above negative on both | **KEEP, narrow the scope** |
| 8 | `wcore-tools/src/vision_tools.rs` | `tools/vision_tools.py` | 22 | 0.0503 | **0.1272** | **containment inside the negative band** | **STRIP** |
| 7 | `wcore-tools/src/transcription_tools.rs` | `tools/transcription_tools.py` | 5 | **0.0113** | **0.0446** | **inside the negative band on both** | **STRIP** |
| 9 | `wcore-types/src/cache_tier.rs` | `agent/prompt_caching.py` | 2 | 0.0833 | 0.1000 | inside the negative band on containment | **UNDECIDED — see 2.5** |

### The qualitative half, which the prior pass insisted on

That pass's decisive sentence about OpenClaw was: *"There is not one distinctive,
non-dictated shared literal at any of the nine sites."* **For Hermes that is
false, and it is what settles the top six.**

- `todo.rs` shares `'[your active task list was preserved across context
  compression]'`, `'task items to write. omit to read current list.'`,
  `'unique item identifier'` — original English prose, dictated by nobody.
- `homeassistant_tool.rs` shares four **identical error-message templates**
  including the Python-style `{}` placeholder syntax carried straight into
  Rust — `'failed to call {domain}.{service}: {e}'`,
  `'failed to get state for {entity_id}: {e}'`, `'failed to list entities: {e}'`,
  `"invalid json string in 'data' parameter: {e}"` — plus the doc examples
  `'light.living_room'`, `'sensor.temperature_1'`, `'kitchen'`. Home Assistant
  dictates `entity_id` and `domain`; it does not dictate our error strings or
  our examples.
- `send_message.rs` shares **identical fabricated example IDs** —
  `'discord:999888777:555444333'`, `'telegram:-1001234567890:17585'`,
  `'matrix:!roomid:server.org'` — and the specific platform roster
  `bluebubbles / dingtalk / feishu / qqbot / wecom / weixin`. Arbitrary digits
  matching across two codebases is the strongest selection signal available.
  **This is why site 4 must not be judged on its 0.0943 Jaccard.**
- `discord_tool.rs` shares whole schema descriptions —
  `'create a public thread; optional message_id anchor'`,
  `'find members by name prefix'`, `"discord api 403 (forbidden) on '{action}'."`

Conversely, the two STRIP recommendations survive the same reading:

- `vision_tools.rs`'s 22 shared literals are almost entirely **format- and
  vendor-dictated**: `image/png`, `image/jpeg`, `image/webp`, `image/gif`,
  `image/bmp`, the PNG magic bytes `\x89png\r\n\x1a\n`, `gif89a`, `http://`,
  `https://`, `file://`, and JSON-Schema keywords. What remains is the tool
  name `vision_analyze` and the generic `analysis` / `question` / `error` /
  `success`. Any independent implementation of a vision tool contains these.
  Its containment (0.1272) is **above `markdown_tool.rs`'s negative control by
  0.0022** — indistinguishable.
- `transcription_tools.rs` shares **five** literals: `error`, `language`,
  `success`, `transcript`, and the tool name `transcribe_audio`. Its
  containment (0.0446) is **below** the `read.rs` negative control (0.0543).

## 2.4 The asymmetry the previous pass established, preserved

Wayland **Desktop** is a documented, self-attributed OpenClaw harvest with its
own licence file, which is why `wcore-channel-imessage` and
`wcore-channel-msteams` legitimately keep an OpenClaw notice even though
survival of expression into the Rust was never established. **Core is not that
situation, and I have not flattened the two.** The Hermes recommendations above
rest on *measured overlap in Core's own source*, not on an inherited chain:
six sites are kept because distinctive shared expression was found, and two are
stripped because it was looked for and was not there. No site here is kept or
dropped on the strength of a documented upstream derivation.

## 2.5 What I could not determine

- **Site 9, `wcore-types/src/cache_tier.rs` — UNDECIDED, escalating.** Its own
  header names the source function, and `apply_anthropic_cache_control` **does
  exist** at `agent/prompt_caching.py:49`, so the claim is not fabricated. But
  the two files share **two** literals, `cache_control` and `ephemeral`, both
  dictated by Anthropic's API — containment 0.10, inside the negative band. And
  the header's own description of the peer is **wrong**: it says *"The
  predecessor hard-codes `cache_ttl: "5m"` or `"1h"` at call sites"*, whereas
  the peer takes `cache_ttl: str = "5m"` as a named parameter with a
  `_build_marker(ttl)` helper (`prompt_caching.py:41-51`) and its caller passes
  a configured `agent._cache_ttl` (`conversation_loop.py:858`) — not a hard-coded
  literal. That is the same defect class the prior pass corrected in eight
  comments, and the same reason it said an inaccurate description of another
  project's behaviour *"is what let the audit read these as admissions."*
  The measurement says independent; the self-attribution says derived; the
  self-attribution is provably confused about the thing it names. **A
  self-attribution that is wrong about its own source is not evidence either
  way**, and removing a copyright notice is not a call an audit should make on
  a 2-literal overlap. `anthropic.rs:307` was correctly escalated by the prior
  pass on a similar footing; **this belongs in the same bucket — Sean's call.**
- **Whether the six KEEP sites' notice scope is correctly worded.** The OpenClaw
  block states scope precisely per site ("the `FailoverReason` taxonomy only").
  The Hermes block does not — it is a bare list of nine modules. Six of them
  earn a notice, but I have not established for any of them *how much* of the
  module derives, which is the same imprecision the prior pass called "its own
  problem". Worth a follow-up; not resolvable by literal overlap alone.
- **Legal sufficiency of anything above.** Whether a shared vocabulary or a
  shared error-string template clears an originality threshold is a legal
  judgement. What is technical and certain is reported; the disposition is not
  mine.
